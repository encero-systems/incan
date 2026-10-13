//! Explicit positional and keyword rest packs from the checker's declared call surface.

use super::args::{DeclaredSlot, fixed_elements};
use super::*;

/// Destination of one written value, before any expression is evaluated.
enum Destination {
    /// One ordinary declaration slot.
    Fixed(usize),
    /// One element of the owned positional pack.
    Positional,
    /// One named member of the owned keyword pack.
    Keyword(String),
}

/// Validate the bounded explicit-value profile before emitting ownership reads or effects.
fn plan<'a>(
    parameters: &[DeclaredSlot],
    arguments: &'a [ast::CallArg],
) -> Result<Vec<(Destination, &'a ast::Spanned<ast::Expr>)>, String> {
    let normal = parameters
        .iter()
        .take_while(|parameter| parameter.kind == IncanCallableParamKind::Normal)
        .count();
    if parameters[normal..].iter().any(|parameter| {
        parameter.kind == IncanCallableParamKind::Normal
            || parameter.is_mut
            || !matches!(parameter.ty, IncanType::Primitive(_))
    }) {
        return Err("rest call with an unretained concrete pack element or parameter order".to_string());
    }
    let positional = parameters
        .iter()
        .any(|parameter| parameter.kind == IncanCallableParamKind::RestPositional);
    let keyword = parameters
        .iter()
        .any(|parameter| parameter.kind == IncanCallableParamKind::RestKeyword);
    if parameters[..normal].iter().any(|parameter| {
        parameter.is_mut || parameter.is_partial_preset || !matches!(parameter.ty, IncanType::Primitive(_))
    }) {
        return Err("rest call with unretained ordinary argument passing".to_string());
    }
    let mut supplied = vec![false; normal];
    let mut next = 0;
    let mut planned = Vec::new();
    for argument in arguments {
        let (destination, expression) = match argument {
            ast::CallArg::Positional(expression) if next < normal => {
                let index = next;
                next += 1;
                (Destination::Fixed(index), expression)
            }
            ast::CallArg::Positional(expression) if positional => (Destination::Positional, expression),
            ast::CallArg::Named(name, expression) => {
                if let Some(index) = parameters[..normal]
                    .iter()
                    .position(|parameter| parameter.name.as_deref() == Some(name.node.as_str()))
                {
                    (Destination::Fixed(index), expression)
                } else if keyword {
                    (Destination::Keyword(name.node.clone()), expression)
                } else {
                    return Err("rest call with an unbound named argument".to_string());
                }
            }
            _ => return Err("rest call with an unretained spread or positional binding".to_string()),
        };
        if let Destination::Fixed(index) = &destination {
            if supplied[*index] {
                return Err("rest call with a repeated ordinary argument".to_string());
            }
            supplied[*index] = true;
        }
        planned.push((destination, expression));
    }
    if supplied.iter().any(|supplied| !supplied) {
        return Err("rest call with an omitted ordinary argument".to_string());
    }
    Ok(planned)
}

impl BodyBuilder<'_, '_> {
    /// Freeze supplied values in written order, then materialize exactly one owned pack per declared rest role.
    /// Checked spreads and ordinary borrowed inputs retain the existing generalized planner; open or mutable pack
    /// elements remain refused.
    pub(super) fn bind_rest_args(
        &mut self,
        callee: &str,
        parameters: &[DeclaredSlot],
        arguments: &[ast::CallArg],
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> Result<(Vec<bir::ArgumentElement>, bir::ArgumentBinding), String> {
        if parameters
            .iter()
            .filter(|parameter| parameter.kind != IncanCallableParamKind::Normal)
            .any(|parameter| parameter.is_mut || !matches!(parameter.ty, IncanType::Primitive(_)))
        {
            return Err(format!("{callee}: rest call with an unretained concrete pack element"));
        }
        if arguments.iter().any(|argument| {
            matches!(
                argument,
                ast::CallArg::PositionalUnpack(_) | ast::CallArg::KeywordUnpack(_)
            )
        }) || parameters
            .iter()
            .filter(|parameter| parameter.kind == IncanCallableParamKind::Normal)
            .any(|parameter| {
                parameter.is_mut || parameter.is_partial_preset || !matches!(parameter.ty, IncanType::Primitive(_))
            })
        {
            return self.bind_general_rest_args(callee, parameters, arguments, scope, out);
        }
        let planned = plan(parameters, arguments).map_err(|description| format!("{callee}: {description}"))?;
        let span = self.scope_span(scope);
        let mut fixed = vec![None; parameters.len()];
        let mut positional = Vec::new();
        let mut keyword = Vec::new();

        // ---- Context: each source value is evaluated before the next source value ----
        for (destination, expression) in planned {
            let value = self.lower_expr_to_operand(expression, scope, out);
            let value = self.owned_parameter_operand(expression, value);
            let value = self.push_assign_temp(
                bir::Rvalue::Use(value),
                self.resolve_ty(expression.span),
                scope,
                hir_span(expression.span),
                out,
            );
            match destination {
                Destination::Fixed(index) => fixed[index] = Some(value),
                Destination::Positional => positional.push(value),
                Destination::Keyword(name) => keyword.push(bir::DictEntry::Pair(
                    bir::Operand::Constant(bir::Constant::Str(name)),
                    Box::new(value),
                )),
            }
        }

        // ---- Context: packed operands occupy declaration order at the physical call boundary ----
        let mut operands = Vec::with_capacity(parameters.len());
        for (index, parameter) in parameters.iter().enumerate() {
            let value = match parameter.kind {
                IncanCallableParamKind::Normal => {
                    fixed[index].take().ok_or("rest call missing a checked ordinary slot")?
                }
                IncanCallableParamKind::RestPositional => self.push_assign_temp(
                    bir::Rvalue::Aggregate(
                        bir::AggregateKind::List,
                        fixed_elements(std::mem::take(&mut positional)),
                    ),
                    IncanType::Generic {
                        base: "List".to_string(),
                        args: vec![parameter.ty.clone()],
                    },
                    scope,
                    span,
                    out,
                ),
                IncanCallableParamKind::RestKeyword => self.push_assign_temp(
                    bir::Rvalue::Dict(std::mem::take(&mut keyword)),
                    IncanType::Generic {
                        base: "Dict".to_string(),
                        args: vec![IncanType::Primitive(IncanPrimitiveType::Str), parameter.ty.clone()],
                    },
                    scope,
                    span,
                    out,
                ),
            };
            operands.push(value);
        }
        self.record_runtime_requirement(AbiV0RuntimeRequirement::Allocator);
        Ok((
            fixed_elements(operands),
            bir::ArgumentBinding::resolved_positional(parameters.len()),
        ))
    }
}
