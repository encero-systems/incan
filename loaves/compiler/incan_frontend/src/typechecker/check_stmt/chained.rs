//! Checking a chained assignment, `x = y = value` (#1806).

use crate::ast::*;
use crate::diagnostics::errors;
use crate::symbols::ResolvedType;
use crate::typechecker::TypeChecker;

impl TypeChecker {
    /// Check `x = y = value` the way a single assignment checks each target.
    ///
    /// The value is checked against the type the already-bound targets share, as `x = value` checks against `x`'s
    /// type, so a literal adapts to it. When the bound targets disagree:
    ///
    /// - a value built only from literals (see [`Expr::is_literal_construction`]) is checked against each target's type
    ///   in turn, since each target gets its own copy of it (`a = b = 5` over an `i8` and an `int`);
    /// - any other value is checked once, and refused when its type is not fully known, because there is no one type
    ///   for it to take (`a = b = make_empty()` returning `list[_]` over a `list[int]` and a `list[str]`).
    pub(in crate::typechecker) fn check_chained_assignment(&mut self, chain: &ChainedAssignmentStmt, stmt_span: Span) {
        let target_types = chain
            .targets
            .iter()
            .map(|target| self.chained_target_type(chain.binding, target))
            .collect::<Vec<_>>();
        let mut bound = target_types.iter().flatten();
        let first_bound = bound.next().cloned();
        let targets_agree = bound.all(|ty| Some(ty) == first_bound.as_ref());

        let per_target_types: Vec<ResolvedType> = if targets_agree {
            let value_ty = self.check_expr_with_expected(&chain.value, first_bound.as_ref());
            vec![value_ty; chain.targets.len()]
        } else if chain.value.node.is_literal_construction() {
            target_types
                .iter()
                .map(|target_ty| self.check_expr_with_expected(&chain.value, target_ty.as_ref()))
                .collect()
        } else {
            let value_ty = self.check_expr(&chain.value);
            if Self::type_has_open_part(&value_ty) {
                self.errors.push(errors::chained_assignment_value_has_no_one_type(
                    &value_ty.to_string(),
                    chain.value.span,
                ));
                return;
            }
            vec![value_ty; chain.targets.len()]
        };

        // Each target has the same declaration/reassignment distinction as a single assignment's target.
        for (index, (target, value_ty)) in chain.targets.iter().zip(per_target_types).enumerate() {
            let target_span = chain.target_spans.get(index).copied().unwrap_or(stmt_span);
            self.check_unannotated_assignment_target(target, chain.binding, value_ty, target_span, chain.value.span);
        }
    }

    /// Return the type an existing binding gives a chain target, or `None` for a target the chain declares.
    fn chained_target_type(&self, binding: BindingKind, name: &str) -> Option<ResolvedType> {
        if Self::binding_introduces_name(binding) {
            return None;
        }
        self.lookup_variable_info_in_scope_chain(name)
            .map(|info| info.ty.clone())
            .or_else(|| self.lookup_static_info(name).map(|info| info.ty.clone()))
    }

    /// Return whether a checked type still has a part only a destination could fix (`None`'s `Option[_]`, `list()`).
    fn type_has_open_part(ty: &ResolvedType) -> bool {
        match ty {
            ResolvedType::Unknown | ResolvedType::CallSiteInfer | ResolvedType::TypeVar(_) => true,
            ResolvedType::Generic(_, args) | ResolvedType::Tuple(args) => args.iter().any(Self::type_has_open_part),
            ResolvedType::FrozenList(inner)
            | ResolvedType::FrozenSet(inner)
            | ResolvedType::Ref(inner)
            | ResolvedType::RefMut(inner) => Self::type_has_open_part(inner),
            ResolvedType::FrozenDict(key, value) => Self::type_has_open_part(key) || Self::type_has_open_part(value),
            _ => false,
        }
    }
}
