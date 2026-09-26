//! Receiver planning for RFC 036 method decorators (#1790).
//!
//! Source spells a method decorator's receiver the way the method does. A decorator on `def label(self, value: int) ->
//! str` is declared `def as_int(func: (Box, int) -> str) -> (Box, int) -> int`, and a function it returns in the
//! method's place `def parse(box: Box, value: int) -> int`. The method's generated wrapper hands a `self` receiver on
//! without giving it up, so those declarations must take the receiver the same way.
//!
//! The checker names each such declaration and the positions of its signature that hold the receiver
//! ([`MethodDecoratorReceiverSlot`]). This pass rewrites those positions to the shared receiver form before any
//! signature is registered, so every later lowering step sees the declarations the former `&Box` spelling produced and
//! the emitter receives the same IR. A `mut self` method's receiver is written `mut Box` in a callable type and `mut
//! box: Box` on a replacement; both already lower to the form the wrapper passes, so those declarations stay as
//! written.

use std::collections::HashMap;

use incan_frontend::ast;
use incan_frontend::typechecker::{MethodDecoratorReceiverRole, MethodDecoratorReceiverSlot};

/// Return `program` with the receiver positions of `self`-method decorator declarations planned, or `None` when no
/// declaration needs it.
///
/// `slots` is keyed by declaration span, as the checker recorded it for this program; a slot whose declaration is not
/// a function of `program` is ignored.
pub(super) fn plan_shared_method_decorator_receivers(
    program: &ast::Program,
    slots: &HashMap<(usize, usize), MethodDecoratorReceiverSlot>,
) -> Option<ast::Program> {
    if slots.values().all(|slot| slot.mutable) {
        return None;
    }
    let mut planned = program.clone();
    for decl in &mut planned.declarations {
        let Some(slot) = slots.get(&(decl.span.start, decl.span.end)) else {
            continue;
        };
        let ast::Declaration::Function(function) = &mut decl.node else {
            continue;
        };
        if slot.mutable {
            continue;
        }
        match slot.role {
            MethodDecoratorReceiverRole::Decorator => {
                plan_decorator_shapes(&mut function.params, &mut function.return_type)
            }
            MethodDecoratorReceiverRole::Factory => {
                if let ast::Type::Function(params, ret) = &mut function.return_type.node {
                    if let Some(accepted) = params.first_mut() {
                        share_callable_receiver(accepted);
                    }
                    share_callable_receiver(ret);
                }
            }
            MethodDecoratorReceiverRole::Replacement => {
                if let Some(receiver) = function.params.first_mut() {
                    share_receiver(&mut receiver.node.ty);
                }
            }
        }
    }
    Some(planned)
}

/// Plan the receiver of the callable type a decorator accepts first and of the callable type it returns.
fn plan_decorator_shapes(params: &mut [ast::Spanned<ast::Param>], return_type: &mut ast::Spanned<ast::Type>) {
    if let Some(accepted) = params.first_mut() {
        share_callable_receiver(&mut accepted.node.ty);
    }
    share_callable_receiver(return_type);
}

/// Plan the receiver of one callable-type annotation, `(Box, int) -> str`; any other annotation is left as written.
fn share_callable_receiver(shape: &mut ast::Spanned<ast::Type>) {
    if let ast::Type::Function(params, _) = &mut shape.node
        && let Some(receiver) = params.first_mut()
    {
        share_receiver(receiver);
    }
}

/// Rewrite one receiver annotation to the shared receiver form the wrapper passes, keeping the annotation's span.
///
/// The span stays on both the rewritten annotation and the type inside it, because checker facts about the annotation
/// are keyed by the span the source wrote.
fn share_receiver(receiver: &mut ast::Spanned<ast::Type>) {
    if matches!(
        receiver.node,
        ast::Type::Ref(_) | ast::Type::RefMut(_) | ast::Type::MutParam(_)
    ) {
        return;
    }
    let span = receiver.span;
    let written = std::mem::replace(&mut receiver.node, ast::Type::Unit);
    receiver.node = ast::Type::Ref(Box::new(ast::Spanned::new(written, span)));
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use incan_frontend::ast;
    use incan_frontend::{lexer, parser};

    use super::{MethodDecoratorReceiverRole, MethodDecoratorReceiverSlot, plan_shared_method_decorator_receivers};

    /// A parsed program with its function declarations' spans keyed by function name.
    type ProgramWithFunctionSpans = (ast::Program, HashMap<String, (usize, usize)>);

    /// Parse `source` and return its declarations' spans keyed by function name.
    fn parse_with_function_spans(source: &str) -> Result<ProgramWithFunctionSpans, Box<dyn std::error::Error>> {
        let tokens = lexer::lex(source).map_err(|errors| format!("lexer failed: {errors:?}"))?;
        let program = parser::parse(&tokens).map_err(|errors| format!("parser failed: {errors:?}"))?;
        let spans = program
            .declarations
            .iter()
            .filter_map(|decl| match &decl.node {
                ast::Declaration::Function(function) => Some((function.name.clone(), (decl.span.start, decl.span.end))),
                _ => None,
            })
            .collect();
        Ok((program, spans))
    }

    /// Return the planned function declaration named `name`.
    fn planned_function<'p>(
        program: &'p ast::Program,
        name: &str,
    ) -> Result<&'p ast::FunctionDecl, Box<dyn std::error::Error>> {
        program
            .declarations
            .iter()
            .find_map(|decl| match &decl.node {
                ast::Declaration::Function(function) if function.name == name => Some(function),
                _ => None,
            })
            .ok_or_else(|| format!("missing function `{name}`").into())
    }

    /// Issue #1790: a `self` method's decorator shapes and replacement are planned to the form the former `&Box`
    /// spelling wrote, and nothing else in the program changes.
    #[test]
    fn shared_receiver_slots_are_planned_to_the_wrapper_form() -> Result<(), Box<dyn std::error::Error>> {
        let source = r#"
class Box:
    value: int

    @as_int
    def label(self, value: int) -> str:
        return "value"

def parse(box: Box, value: int) -> int:
    return value

def as_int(func: (Box, int) -> str) -> (Box, int) -> int:
    return parse

def unrelated(box: Box, func: (Box, int) -> int) -> int:
    return func(box, 1)
"#;
        let (program, spans) = parse_with_function_spans(source)?;
        let span_of = |name: &str| {
            spans
                .get(name)
                .copied()
                .ok_or_else(|| format!("missing span for `{name}`"))
        };
        let slots = HashMap::from([
            (
                span_of("as_int")?,
                MethodDecoratorReceiverSlot {
                    mutable: false,
                    role: MethodDecoratorReceiverRole::Decorator,
                },
            ),
            (
                span_of("parse")?,
                MethodDecoratorReceiverSlot {
                    mutable: false,
                    role: MethodDecoratorReceiverRole::Replacement,
                },
            ),
        ]);
        let planned = plan_shared_method_decorator_receivers(&program, &slots).ok_or("expected a planned program")?;

        let as_int = planned_function(&planned, "as_int")?;
        assert_eq!(as_int.params[0].node.ty.node.to_string(), "(&Box, int) -> str");
        assert_eq!(as_int.return_type.node.to_string(), "(&Box, int) -> int");
        let parse = planned_function(&planned, "parse")?;
        assert_eq!(parse.params[0].node.ty.node.to_string(), "&Box");
        assert_eq!(parse.params[1].node.ty.node.to_string(), "int");
        let unrelated = planned_function(&planned, "unrelated")?;
        assert_eq!(unrelated.params[0].node.ty.node.to_string(), "Box");
        assert_eq!(unrelated.params[1].node.ty.node.to_string(), "(Box, int) -> int");
        Ok(())
    }

    /// Issue #1790: a `mut self` method's shapes already say how the receiver is passed, so a program whose slots are
    /// all mutable is not copied.
    #[test]
    fn mutable_receiver_slots_leave_the_program_as_written() -> Result<(), Box<dyn std::error::Error>> {
        let source = r#"
class Counter:
    value: int

    @keep
    def bump(mut self, by: int) -> int:
        self.value = self.value + by
        return self.value

def keep(func: (mut Counter, int) -> int) -> (mut Counter, int) -> int:
    return func
"#;
        let (program, spans) = parse_with_function_spans(source)?;
        let keep = spans.get("keep").copied().ok_or("missing span for `keep`")?;
        let slots = HashMap::from([(
            keep,
            MethodDecoratorReceiverSlot {
                mutable: true,
                role: MethodDecoratorReceiverRole::Decorator,
            },
        )]);
        assert!(plan_shared_method_decorator_receivers(&program, &slots).is_none());
        Ok(())
    }
}
