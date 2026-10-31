//! Checked Rust callees projected into the existing external callable and named-call machinery (DD-0006).

use super::args::DeclaredSlot;
use super::{BodyBuilder, IncanType, TypeCheckInfo, ast, bir, hir_span};
use crate::typechecker::RustCheckedCall;

/// Declare each checked Rust instantiation once, keeping native argument order and reference passing modes.
/// Signatures and nominal argument owners are already structural and fully qualified in the accepting checker.
pub(super) fn declarations(facts: &TypeCheckInfo) -> Vec<bir::LoafCallable> {
    let mut calls = Vec::new();
    let argument_calls = facts
        .rust
        .argument_adjustments
        .iter()
        .flat_map(|(span, steps)| steps.iter().map(move |step| (span, step)));
    let field_calls = facts
        .rust
        .field_adjustments
        .iter()
        .flat_map(|(span, steps)| steps.iter().map(move |step| (span, step)));
    let mut checked_calls = facts
        .rust
        .checked_calls
        .iter()
        .chain(&facts.rust.await_conversions)
        .chain(argument_calls)
        .chain(facts.rust.element_adjustments.iter())
        .chain(field_calls)
        .chain(facts.rust.checked_function_values.iter())
        .map(|(span, call)| (*span, call))
        .chain(
            facts
                .rust
                .iteration_hooks
                .iter()
                .flat_map(|(span, hooks)| [(*span, &hooks.iter), (*span, &hooks.next)]),
        )
        .collect::<Vec<_>>();
    checked_calls.sort_by_key(|(span, _)| *span);
    for call in checked_calls
        .into_iter()
        .map(|(_, call)| call)
        .flat_map(|call| call.receiver_adjustments.iter().chain(std::iter::once(call)))
    {
        if call.path == "std::panic::set_hook" {
            continue;
        }
        let Some(canonical) = bir::rust_item_identity(&call.path) else {
            continue;
        };
        let Some(storage) = &call.storage else {
            continue;
        };
        let declaration = bir::LoafCallable {
            canonical,
            item: call.path.clone(),
            parameters: storage.parameters.clone(),
            return_type: storage.return_type.clone(),
            type_parameters: Vec::new(),
            native_type_arguments: storage.type_arguments.clone(),
            native_callable_parameters: facts
                .rust
                .native_callable_parameters
                .get(&call.path)
                .into_iter()
                .flatten()
                .map(|&(parameter_slot, type_argument_slot)| bir::NativeCallableParameter {
                    parameter_slot,
                    type_argument_slot,
                })
                .collect(),
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
    /// Materialize the shared reference layers inside a checked parameter before its final argument loan.
    /// Each temporary carries an explicit borrow fact and the retained signature's exact type; a foreign `&T`
    /// whose selected T is `&Owner` therefore receives `&&Owner` without downstream type or trait inference.
    /// A different referent or an exclusive inner reference keeps the ordinary boundary's existing validation.
    pub(super) fn materialize_argument_loans(
        &mut self,
        expression: &ast::Spanned<ast::Expr>,
        mut value: bir::Operand,
        referent: &IncanType,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let source = self.resolve_ty(expression.span);
        let mut target = referent;
        let mut loans = Vec::new();
        while target != &source {
            let IncanType::Ref(inner) = target else {
                return value;
            };
            loans.push(target.clone());
            target = inner;
        }
        for loan in loans.into_iter().rev() {
            if let bir::Operand::Place(read) = &mut value {
                self.retain_argument_borrow(read, false);
            }
            value = self.push_assign_temp(bir::Rvalue::Use(value), loan, scope, hir_span(expression.span), out);
        }
        value
    }

    /// Evaluate a source list once, convert each element through its exact checked native call, and collect owned
    /// results in order. This uses ordinary Body IR iteration and list growth; no mapping policy reaches lowering.
    fn lower_native_list_conversion(
        &mut self,
        expression: &ast::Spanned<ast::Expr>,
        conversion: &RustCheckedCall,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> Result<bir::Operand, String> {
        // ---- Checked input and output carriers ----
        let storage = conversion
            .storage
            .as_ref()
            .ok_or("native element conversion without checked storage")?;
        let [element] = storage.parameters.as_slice() else {
            return Err("native element conversion requires one input".into());
        };
        let ty = IncanType::Generic {
            base: "List".into(),
            args: vec![storage.return_type.clone()],
        };
        let span = hir_span(expression.span);

        // ---- Owned result list ----
        let result = self.new_temp(ty.clone(), scope, span);
        out.push(bir::Statement {
            kind: bir::StatementKind::Assign {
                place: bir::Place::from_local(result),
                rvalue: bir::Rvalue::Aggregate(bir::AggregateKind::List, Vec::new()),
            },
            span,
        });
        self.record_runtime_requirement(incan_semantics_core::AbiV0RuntimeRequirement::Allocator);

        // ---- One checked conversion per source element ----
        let loop_scope = self.new_scope(Some(scope), span);
        let item = self.new_temp(element.clone(), loop_scope, span);
        self.lower_general_iteration(
            expression,
            item,
            scope,
            loop_scope,
            span,
            out,
            |builder, scope, body| {
                let input = builder.temp_operand(item, element);
                let Some(value) = builder.emit_checked_rust_call(
                    conversion,
                    vec![bir::ArgumentElement::One(input)],
                    bir::ArgumentBinding::resolved_positional(1),
                    expression.span,
                    scope,
                    body,
                ) else {
                    builder.push_unsupported_stmt(
                        "native element conversion without checked identity".into(),
                        span,
                        body,
                    );
                    return;
                };
                body.push(bir::Statement {
                    kind: bir::StatementKind::Call {
                        destination: None,
                        callee: bir::Callee::Method(bir::MethodTarget {
                            native_trait_delegation: None,
                            canonical: Some(crate::symbols::canonical_builtin_member_identity("List", "append")),
                            ..bir::MethodTarget::synthesized("push")
                        }),
                        args: super::args::fixed_elements(vec![
                            bir::Operand::place(bir::Place::from_local(result), bir::OwnershipFact::MutBorrow, false),
                            value,
                        ]),
                        may_panic: false,
                    },
                    span,
                });
            },
        );
        Ok(self.temp_operand(result, &ty))
    }

    /// Evaluate one argument and its retained native dereference steps in source order. The initial owner is borrowed
    /// according to the checked adjustment signature; each result keeps its reference carrier for the source callee.
    pub(super) fn lower_adjusted_argument(
        &mut self,
        expr: &ast::Spanned<ast::Expr>,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> Option<Result<bir::Operand, String>> {
        if let Some(conversion) = self.type_info.rust.element_adjustments.get(&expr.span).cloned() {
            return Some(self.lower_native_list_conversion(expr, &conversion, scope, out));
        }
        let steps = self.type_info.rust.argument_adjustments.get(&expr.span)?.clone();
        Some(self.lower_native_adjustment_value(expr, &steps, scope, out))
    }

    /// Evaluate one checked native adjustment chain once. Reference parameters borrow the original place; owned
    /// storage constructions consume their input instead of silently borrowing it.
    pub(super) fn lower_native_adjustment_value(
        &mut self,
        expr: &ast::Spanned<ast::Expr>,
        steps: &[RustCheckedCall],
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> Result<bir::Operand, String> {
        let mut value = self.lower_expr_to_operand(expr, scope, out);
        for step in steps {
            if let bir::Operand::Place(read) = &mut value
                && let Some(parameter) = step.parameters.first()
            {
                match parameter.ty {
                    crate::symbols::ResolvedType::Ref(_) => self.retain_argument_borrow(read, false),
                    crate::symbols::ResolvedType::RefMut(_) => self.retain_argument_borrow(read, true),
                    _ => {}
                }
            }
            let Some(adjusted) = self.emit_checked_rust_call(
                step,
                vec![bir::ArgumentElement::One(value)],
                bir::ArgumentBinding::resolved_positional(1),
                expr.span,
                scope,
                out,
            ) else {
                return Err("native adjustment without an identity".into());
            };
            value = adjusted;
        }
        Ok(value)
    }

    /// Materialize the checked shared native field base before ordinary canonical field projection. Other fields
    /// preserve their existing place path, and a missing adjustment identity remains an explicit unsupported operand.
    /// The final place copies the accepting checker's frozen result storage rather than reconstructing its signature.
    pub(super) fn lower_checked_field_base(
        &mut self,
        base: &ast::Spanned<ast::Expr>,
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Place {
        let Some(steps) = self.type_info.rust.field_adjustments.get(&span).cloned() else {
            return self.lower_expr_to_place(base, scope, out);
        };
        let value = match self.lower_native_adjustment_value(base, &steps, scope, out) {
            Ok(value) => value,
            Err(reason) => self.unsupported_operand(reason, scope, hir_span(span), out),
        };
        let ty = steps
            .last()
            .and_then(|step| step.storage.as_ref())
            .map_or(IncanType::Unknown, |storage| storage.return_type.clone());
        self.materialize_operand_to_place(value, ty, scope, hir_span(span), out)
    }

    /// Project a checked native function read into the existing capture-free named function-item representation.
    /// The namespace is never evaluated; parameters and the forwarding target come only from the published native
    /// signature.
    pub(super) fn lower_checked_rust_function_value(
        &mut self,
        expr: &ast::Spanned<ast::Expr>,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> Option<bir::Operand> {
        let call = self.type_info.rust.checked_function_values.get(&expr.span)?.clone();
        let span = hir_span(expr.span);
        let storage = call.storage.as_ref()?;
        if call.has_receiver || !call.receiver_adjustments.is_empty() || !storage.type_arguments.is_empty() {
            return Some(self.unsupported_operand(
                format!(
                    "native function value `{}` is not a closed associated or free function",
                    call.path
                ),
                scope,
                span,
                out,
            ));
        }
        let child = self.new_scope(Some(scope), span);
        let mut parameters = Vec::new();
        let mut arguments = Vec::new();
        for (index, ty) in storage.parameters.iter().enumerate() {
            let local = self.new_temp(ty.clone(), child, span);
            self.locals[local.index()].origin = bir::LocalOrigin::Parameter;
            parameters.push(bir::CallableParam {
                local,
                name: format!("__native_argument_{index}"),
                ty: ty.clone(),
                kind: super::parameter_types::source_parameter_kind(ast::ParamKind::Normal),
                span,
                default: bir::CallableParamDefault::Required,
                mutable: false,
                native_owned: super::runtime_nominals::owned_collection_storage(ty),
            });
            arguments.push(bir::ArgumentElement::One(self.temp_operand(local, ty)));
        }
        let binding = bir::ArgumentBinding::resolved_positional(parameters.len());
        let mut statements = Vec::new();
        let result = self.emit_checked_rust_call(&call, arguments, binding, expr.span, child, &mut statements)?;
        Some(self.push_assign_temp(
            bir::Rvalue::Closure {
                params: parameters,
                captured_operands: Vec::new(),
                body: Box::new(bir::ClosureBody {
                    native_storage: None,
                    function_item: true,
                    consumes_environment: false,
                    original_callable: None,
                    capture_locals: Vec::new(),
                    stmts: statements,
                    result,
                }),
            },
            self.resolve_ty(expr.span),
            scope,
            span,
            out,
        ))
    }

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
        let call = self.type_info.rust.checked_calls.get(&span)?.clone();
        if call.path == "std::panic::set_hook" {
            let (operands, _binding) = match self.bind_declared_args(&call.path, None, args, scope, out) {
                Ok(bound) => bound,
                Err(reason) => return Some(self.unsupported_operand(reason, scope, hir_span(span), out)),
            };
            return Some(self.push_call_temp(
                bir::Callee::Helper(bir::HelperOp::InstallPanicHook),
                operands,
                self.checked_type(&call.return_type),
                scope,
                hir_span(span),
                false,
                out,
            ));
        }
        let storage = call.storage.as_ref()?;
        if !call.receiver_adjustments.is_empty() {
            let (receiver, args) = if call.has_receiver {
                (receiver?, args)
            } else {
                let ast::CallArg::Positional(receiver) = args.first()? else {
                    return None;
                };
                (receiver, &args[1..])
            };
            let value = match self.lower_adjusted_rust_call(&call, receiver, args, span, scope, out) {
                Ok(value) => value,
                Err(reason) => self.unsupported_operand(reason, scope, hir_span(span), out),
            };
            return Some(self.retain_checked_numeric_result(value, span, scope, out));
        }
        let mut arguments = Vec::new();
        if call.has_receiver {
            arguments.push(ast::CallArg::Positional(receiver?.clone()));
        }
        arguments.extend_from_slice(args);
        let slots = call
            .parameters
            .iter()
            .zip(&storage.parameters)
            .map(|(parameter, ty)| DeclaredSlot {
                ty: ty.clone(),
                ..DeclaredSlot::from_checked_param(parameter)
            })
            .collect();
        let (mut operands, binding) = match self.bind_declared_args(&call.path, Some(slots), &arguments, scope, out) {
            Ok(bound) => bound,
            Err(reason) => {
                return Some(self.unsupported_operand(reason, scope, hir_span(span), out));
            }
        };
        if self.type_info.rust.copy_receivers.contains_key(&span)
            && let Some(bir::ArgumentElement::One(bir::Operand::Place(read))) = operands.first_mut()
        {
            read.fact = bir::OwnershipFact::Copy;
            read.last_use = false;
            if let Some(local) = read.place.local_id() {
                self.moved_out.remove(&local);
            }
        }
        let value = self.emit_checked_rust_call(&call, operands, binding, span, scope, out)?;
        Some(self.retain_checked_numeric_result(value, span, scope, out))
    }

    /// Emit an already bound call through the shared canonical external target; no method resolution occurs here.
    pub(super) fn emit_checked_rust_call(
        &mut self,
        call: &RustCheckedCall,
        operands: Vec<bir::ArgumentElement>,
        binding: bir::ArgumentBinding,
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> Option<bir::Operand> {
        let canonical = bir::rust_item_identity(&call.path)?;
        let storage = call.storage.as_ref()?;
        let type_args = storage.type_arguments.clone();
        let result = storage.return_type.clone();
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
            ty: call
                .receiver_adjustments
                .first()
                .and_then(|step| step.storage.as_ref())
                .and_then(|storage| storage.parameters.first())
                .ok_or("native receiver adjustment without checked storage")?
                .clone(),
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
            .zip(
                &call
                    .storage
                    .as_ref()
                    .ok_or("native method without checked storage")?
                    .parameters,
            )
            .skip(1)
            .map(|(parameter, ty)| DeclaredSlot {
                ty: ty.clone(),
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
