//! Preserve already checked SDK task outputs through their containing Body IR storage.

use super::{BodyBuilder, IncanType, ast, bir};

impl BodyBuilder<'_, '_> {
    /// Read an evaluated whole-local operand's retained storage; projections require their own checked layout.
    pub(super) fn retained_operand_type(&self, operand: &bir::Operand) -> Option<IncanType> {
        let bir::Operand::Place(read) = operand else {
            return None;
        };
        if !read.place.projection.is_empty() {
            return None;
        }
        self.locals
            .get(read.place.local_id()?.index())
            .map(|local| local.ty.clone())
    }

    /// Carry a concrete checked SDK task argument through matching list shapes, preserving closed contextual types.
    /// Numeric promotion, unions, foreign owners, and source-owned equal spellings remain the checker's contract.
    pub(super) fn refine_task_storage(&self, checked: &IncanType, actual: &IncanType) -> IncanType {
        let (
            IncanType::Generic { base, args },
            IncanType::Generic {
                base: actual_base,
                args: actual_args,
            },
        ) = (checked, actual)
        else {
            return checked.clone();
        };
        if base != actual_base || args.len() != actual_args.len() {
            return checked.clone();
        }
        if base == "List" && args.len() == 1 {
            return IncanType::Generic {
                base: base.clone(),
                args: vec![self.refine_task_storage(&args[0], &actual_args[0])],
            };
        }
        if super::runtime_nominals::retains_task_storage(self.type_info, actual)
            && args.iter().all(|arg| self.leaves_type_open(arg))
            && actual_args.iter().all(|arg| !self.leaves_type_open(arg))
        {
            return actual.clone();
        }
        checked.clone()
    }

    /// Recover the checked value's task storage before evaluation, without inventing a call or its output.
    pub(super) fn retained_expression_type(&self, expression: &ast::Spanned<ast::Expr>) -> IncanType {
        let checked = self
            .published_expression_type(expression)
            .unwrap_or_else(|| self.resolve_ty(expression.span));
        let actual = match &expression.node {
            ast::Expr::Ident(name) => self
                .type_info
                .resolved_identity(expression.span)
                .map_or_else(
                    || self.bindings.get(name),
                    |identity| self.identity_bindings.get(identity),
                )
                .and_then(|local| self.locals.get(local.index()))
                .map(|local| local.ty.clone()),
            ast::Expr::Paren(inner) => Some(self.retained_expression_type(inner)),
            ast::Expr::List(entries) => {
                let types: Option<Vec<_>> = entries
                    .iter()
                    .map(|entry| match entry {
                        ast::ListEntry::Element(item) => Some(self.retained_expression_type(item)),
                        ast::ListEntry::Spread(_) => None,
                    })
                    .collect();
                types.and_then(|types| Self::homogeneous_list_type(&types))
            }
            ast::Expr::ListComp(comp) => Some(IncanType::Generic {
                base: "List".into(),
                args: vec![self.retained_expression_type(&comp.expr)],
            }),
            _ => None,
        };
        actual.map_or(checked.clone(), |actual| self.refine_task_storage(&checked, &actual))
    }

    /// Preserve homogeneous evaluated task elements; empty or mixed literals keep their checked contextual type.
    pub(super) fn retained_list_type(&self, checked: &IncanType, elements: &[bir::ArgumentElement]) -> IncanType {
        let types: Option<Vec<_>> = elements
            .iter()
            .map(|element| {
                let bir::ArgumentElement::One(operand) = element else {
                    return None;
                };
                self.retained_operand_type(operand)
            })
            .collect();
        types
            .and_then(|types| Self::homogeneous_list_type(&types))
            .map_or_else(|| checked.clone(), |actual| self.refine_task_storage(checked, &actual))
    }

    /// Form a list only when every retained element has exactly the same checked storage.
    fn homogeneous_list_type(types: &[IncanType]) -> Option<IncanType> {
        let first = types.first()?;
        types.iter().all(|ty| ty == first).then(|| IncanType::Generic {
            base: "List".into(),
            args: vec![first.clone()],
        })
    }

    /// Retain the output of an already typed source future or registry-owned native task handle.
    pub(super) fn retained_await_output(&self, operand: &bir::Operand) -> Option<IncanType> {
        let ty = self.retained_operand_type(operand)?;
        let IncanType::Generic { base, args } = &ty else {
            return None;
        };
        let [output] = args.as_slice() else {
            return None;
        };
        if base == "Awaitable" {
            return Some(output.clone());
        }
        if base == "JoinHandle" && super::runtime_nominals::retains_task_storage(self.type_info, &ty) {
            return Some(IncanType::Generic {
                base: "Result".into(),
                args: vec![output.clone(), IncanType::Named("TaskJoinError".into())],
            });
        }
        None
    }
}
