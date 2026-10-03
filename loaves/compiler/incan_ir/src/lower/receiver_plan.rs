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
