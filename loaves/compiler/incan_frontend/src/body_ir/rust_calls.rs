//! Checked Rust callees projected into the existing external callable and named-call machinery (DD-0006).

use super::args::DeclaredSlot;
use super::{BodyBuilder, IncanType, TypeCheckInfo, ast, bir, hir_span, nominal_carriers, parameter_types};
use crate::typechecker::RustCheckedCall;

/// Declare each checked Rust instantiation once, keeping native argument order and reference passing modes.
/// The signature is already instantiated by checking. Source-local declarations supply the canonical carrier for an
/// inferred model argument even when no annotation introduced that name into the checker's type-position map.
/// Rust metadata supplies its complete native signature and has no authenticated Incan declaration frame.
pub(super) fn declarations(facts: &TypeCheckInfo, locals: &[bir::NominalDeclaration]) -> Vec<bir::LoafCallable> {
    let mut identities = parameter_types::retained_type_identities(facts);
    for declaration in locals {
        let name = declaration.canonical.declaration_name.clone();
        if identities
            .get(&name)
            .is_some_and(|identity| identity != &declaration.canonical)
        {
            identities.remove(&name);
        } else {
            identities.insert(name, declaration.canonical.clone());
        }
    }
    let mut calls = Vec::new();
    let mut checked_calls = facts.rust.checked_calls.iter().collect::<Vec<_>>();
    checked_calls.sort_by_key(|(span, _)| **span);
    for call in checked_calls
        .into_iter()
        .flat_map(|(_, call)| call.receiver_adjustments.iter().chain(std::iter::once(call)))
    {
        let Some(canonical) = bir::rust_item_identity(&call.path) else {
            continue;
        };
        let checked = |ty| {
            let mut projected = parameter_types::checked_semantic_type(ty, facts);
            nominal_carriers::retain_checked_type(&mut projected, ty, facts);
            nominal_carriers::retain_declared_type(&mut projected, &identities);
            projected
        };
        let declaration = bir::LoafCallable {
            canonical,
            item: call.path.clone(),
            parameters: call.parameters.iter().map(|parameter| checked(&parameter.ty)).collect(),
            return_type: checked(&call.return_type),
            type_parameters: Vec::new(),
            native_type_arguments: call.type_arguments.iter().map(checked).collect(),
            parameter_defaults: Vec::new(),
            declaration_frame: None,
        };
        if !calls.contains(&declaration) {
            calls.push(declaration);
        }
    }
    calls
}

impl BodyBuilder<'_, '_> {
    /// Lower a metadata-selected Rust call through ordinary declared-slot binding and a named external target.
    /// An absent record grants no foreign call behavior; existing source and builtin resolution keeps priority.
    pub(super) fn lower_checked_rust_call(
        &mut self,
        receiver: Option<&ast::Spanned<ast::Expr>>,
        args: &[ast::CallArg],
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> Option<bir::Operand> {
        let call = self.type_info.rust.checked_calls.get(&(span.start, span.end))?.clone();
        if !call.receiver_adjustments.is_empty() {
            let (receiver, args) = if call.has_receiver {
                (receiver?, args)
            } else {
                let ast::CallArg::Positional(receiver) = args.first()? else {
                    return None;
                };
                (receiver, &args[1..])
            };
            return Some(
                match self.lower_adjusted_rust_call(&call, receiver, args, span, scope, out) {
                    Ok(value) => value,
                    Err(reason) => self.unsupported_operand(reason, scope, hir_span(span), out),
                },
            );
        }
        let mut arguments = Vec::new();
        if call.has_receiver {
            arguments.push(ast::CallArg::Positional(receiver?.clone()));
        }
        arguments.extend_from_slice(args);
        let slots = call
            .parameters
            .iter()
            .map(|parameter| DeclaredSlot {
                ty: self.checked_type(&parameter.ty),
                ..DeclaredSlot::from_checked_param(parameter)
            })
            .collect();
        let (operands, binding) = match self.bind_declared_args(&call.path, Some(slots), &arguments, scope, out) {
            Ok(bound) => bound,
            Err(reason) => return Some(self.unsupported_operand(reason, scope, hir_span(span), out)),
        };
        self.emit_checked_rust_call(&call, operands, binding, span, scope, out)
    }

    /// Emit an already bound call through the shared canonical external target; no method resolution occurs here.
    fn emit_checked_rust_call(
        &mut self,
        call: &RustCheckedCall,
        operands: Vec<bir::ArgumentElement>,
        binding: bir::ArgumentBinding,
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> Option<bir::Operand> {
        let canonical = bir::rust_item_identity(&call.path)?;
        let type_args = call
            .type_arguments
            .iter()
            .map(|argument| self.checked_type(argument))
            .collect();
        let result: IncanType = self.checked_type(&call.return_type);
        Some(self.push_call_temp(
            bir::Callee::Function(bir::CallableTarget::Named(bir::NamedCallableTarget {
                name: call.path.clone(),
                direct_call_id: None,
                builtin: None,
                type_args,
                binding,
                canonical: Some(canonical),
                receiver_type: None,
            })),
            operands,
            result,
            scope,
            hir_span(span),
            false,
            out,
        ))
    }

    /// Evaluate the receiver and its checked dereference chain before binding the written ordinary arguments.
    fn lower_adjusted_rust_call(
        &mut self,
        call: &RustCheckedCall,
        receiver: &ast::Spanned<ast::Expr>,
        args: &[ast::CallArg],
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> Result<bir::Operand, String> {
        let initial = call
            .receiver_adjustments
            .first()
            .and_then(|step| step.parameters.first())
            .ok_or("native receiver adjustment without an input")?;
        let slot = DeclaredSlot {
            ty: self.checked_type(&initial.ty),
            ..DeclaredSlot::from_checked_param(initial)
        };
        let (mut inputs, _) = self.bind_declared_args(
            "native receiver",
            Some(vec![slot]),
            &[ast::CallArg::Positional(receiver.clone())],
            scope,
            out,
        )?;
        let mut adjusted = inputs.pop().ok_or("native receiver adjustment without a value")?;
        for step in &call.receiver_adjustments {
            let value = self
                .emit_checked_rust_call(
                    step,
                    vec![adjusted],
                    bir::ArgumentBinding::resolved_positional(1),
                    span,
                    scope,
                    out,
                )
                .ok_or("native receiver adjustment without an identity")?;
            adjusted = bir::ArgumentElement::One(value);
        }
        let slots = call
            .parameters
            .iter()
            .skip(1)
            .map(|parameter| DeclaredSlot {
                ty: self.checked_type(&parameter.ty),
                ..DeclaredSlot::from_checked_param(parameter)
            })
            .collect();
        let (mut operands, mut binding) = self.bind_declared_args(&call.path, Some(slots), args, scope, out)?;
        prepend_receiver_binding(&mut binding)?;
        operands.insert(0, adjusted);
        self.emit_checked_rust_call(call, operands, binding, span, scope, out)
            .ok_or_else(|| "native method without an identity".into())
    }
}

/// Shift a resolved ordinary binding once when a pre-evaluated receiver takes native slot zero.
fn prepend_receiver_binding(binding: &mut bir::ArgumentBinding) -> Result<(), String> {
    let (arguments, defaults) = match binding {
        bir::ArgumentBinding::Resolved {
            arguments,
            defaulted_slots,
        }
        | bir::ArgumentBinding::ResolvedDeclarationOrder {
            arguments,
            defaulted_slots,
        } => (arguments, defaulted_slots),
        bir::ArgumentBinding::UnresolvedPositional => {
            return Err("native adjusted call without a resolved binding".into());
        }
    };
    for argument in arguments.iter_mut() {
        argument.slot += 1;
        argument.written_position += 1;
    }
    for slot in defaults {
        *slot += 1;
    }
    arguments.insert(
        0,
        bir::BoundArgument {
            slot: 0,
            written_position: 0,
        },
    );
    Ok(())
}
