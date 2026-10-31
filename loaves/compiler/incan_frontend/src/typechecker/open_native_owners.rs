//! Fix a native call's open result arguments from the first type-dependent use of its local (#1720, #2481).
//!
//! `mut table = HashMap.new()` leaves `K` and `V` open, and the local is watched from its declaration. Its first
//! type-dependent read must be a use that determines them: a native method call whose retained callee fixes the
//! receiver, such as `table.insert("a", 1)`, or a typed destination: a parameter, a typed return or an annotated place.
//! That use fixes them at once: the initializer is checked again with the fixed type as its destination, as an
//! annotation on the local would make it, so the call retains its callee once and everything checked after the use sees
//! the fixed type. The end of a statement holding a type-dependent read that fixed nothing refuses the call.
//! Type-independent reads leave the watch active, and a method's own open result parameters use the same
//! typed-destination rule.

use std::collections::HashMap;

use super::{OpenRustGenericBinding, OpenRustPassiveUse, TypeChecker};
use crate::ast::{Expr, Span, Spanned};
use crate::resolved_type_subst::{map_resolved_type_leaves, substitute_resolved_type};
use crate::symbols::{ResolvedType, SymbolKind};
use incan_lang::interop::{RustFunctionSig, RustItemKind, RustTypeInfo};

impl TypeChecker {
    /// Whether every open callee parameter occurs structurally in its native result. Only those parameters can
    /// be determined by a later destination of that result; an input-only binder must be refused at the call.
    pub(in crate::typechecker) fn native_result_carries_parameters(
        result: &ResolvedType,
        parameters: &[String],
    ) -> bool {
        parameters.iter().all(|parameter| {
            let present = std::cell::Cell::new(false);
            map_resolved_type_leaves(result, &|leaf| {
                if matches!(leaf, ResolvedType::TypeVar(name) | ResolvedType::RustPath(name) | ResolvedType::Named(name) if name == parameter) {
                    present.set(true);
                }
                None
            });
            present.get()
        })
    }

    /// Defer a native receiver read when its operation's inputs, result and predicates need no open parameter.
    /// The receiver's layout still needs the eventual owner arguments, so retain the source operation for checking
    /// again after a fixing use. The decision follows the selected signature, never the method's spelling.
    pub(in crate::typechecker) fn defer_open_rust_receiver_read(
        &mut self,
        receiver: &Spanned<Expr>,
        signature: &RustFunctionSig,
        expression: Spanned<Expr>,
    ) -> bool {
        let Expr::Ident(name) = &receiver.node else {
            return false;
        };
        let Some((index, _)) = self.open_rust_binding_read(name, receiver.span) else {
            return false;
        };
        let holes = &self.open_rust_generic_bindings[index].call.type_params;
        let needs_hole = |display: &str| {
            display
                .split(|ch: char| !(ch.is_alphanumeric() || ch == '_'))
                .any(|token| token == "Self" || holes.iter().any(|hole| hole == token))
        };
        if needs_hole(&signature.return_type)
            || signature
                .params
                .iter()
                .skip(1)
                .any(|parameter| needs_hole(&parameter.type_display))
            || signature.bounds.iter().any(|bound| {
                self.rust_trait_identity(&bound.path) != self.rust_trait_identity("std::marker::Sized")
                    && (needs_hole(&bound.self_type)
                        || bound.type_arguments.iter().any(|argument| needs_hole(argument))
                        || bound
                            .associated_type_bindings
                            .iter()
                            .any(|binding| needs_hole(&binding.value_path)))
            })
        {
            if crate::trace_flags::trace_rust_facts() {
                eprintln!("rust open owner read needs parameters: {name} {signature:?}");
            }
            return false;
        }
        self.defer_open_rust_read(receiver, Some(expression))
    }

    /// Remove only this type-independent read from a watched local's fixing statement. Other reads in the same
    /// expression remain fixing uses, and a later statement can still determine the initializer's arguments.
    pub(in crate::typechecker) fn defer_open_rust_read(
        &mut self,
        receiver: &Spanned<Expr>,
        expression: Option<Spanned<Expr>>,
    ) -> bool {
        let Expr::Ident(name) = &receiver.node else {
            return false;
        };
        let Some((index, _)) = self.open_rust_binding_read(name, receiver.span) else {
            return false;
        };
        let binding = &mut self.open_rust_generic_bindings[index];
        binding.active_reads.retain(|span| *span != receiver.span);
        if binding.active_reads.is_empty() {
            binding.read_in = None;
        }
        if let Some(expression) = expression {
            binding.passive_uses.push(OpenRustPassiveUse::Expression(expression));
        }
        true
    }

    /// Preserve builtin length's argument shape when its native receiver read was deferred as a method lookup.
    /// No lexical name is resolved again: a qualified or aliased builtin keeps its original selected authority.
    pub(in crate::typechecker) fn retain_open_rust_length_use(&mut self, receiver: &Spanned<Expr>, span: Span) {
        let Some(identity) = self.type_info.resolved_identity(receiver.span) else {
            return;
        };
        let Some(binding) = self
            .open_rust_generic_bindings
            .iter_mut()
            .find(|binding| binding.identity.as_ref() == Some(identity))
        else {
            return;
        };
        for operation in &mut binding.passive_uses {
            if matches!(operation, OpenRustPassiveUse::Expression(expression) if expression.span == span) {
                *operation = OpenRustPassiveUse::Length {
                    receiver: receiver.clone(),
                    span,
                };
            }
        }
    }

    /// Fix a watched binding from the receiver its native method call retained: `table.insert("a", 1)` retains
    /// `&mut HashMap<str, int>` as its receiver, whose owner arguments close `K` and `V`.
    pub(in crate::typechecker) fn fix_open_rust_binding_by_receiver(
        &mut self,
        receiver: &Spanned<Expr>,
        owner: &RustTypeInfo,
        path: &str,
        span: Span,
    ) {
        let Expr::Ident(name) = &receiver.node else {
            return;
        };
        let Some((index, template)) = self.open_rust_binding_read(name, receiver.span) else {
            return;
        };
        let native = self
            .type_info
            .rust
            .checked_calls
            .get(&span)
            .filter(|checked| checked.has_receiver)
            .and_then(|checked| checked.parameters.first())
            .map(|parameter| match &parameter.ty {
                ResolvedType::Ref(inner) | ResolvedType::RefMut(inner) => inner.as_ref().clone(),
                other => other.clone(),
            });
        let Some(arguments) = native.and_then(|native| self.rust_owner_type_args(&native, path)) else {
            return;
        };
        let arguments = owner
            .type_params
            .iter()
            .cloned()
            .zip(arguments)
            .collect::<HashMap<_, _>>();
        self.fix_open_rust_binding(index, &template, &arguments);
    }

    /// Fix a watched binding from the typed destination its read is checked against, and return the type the read
    /// then has: a parameter's declared type, a typed return, or an annotated place. A destination that names the
    /// native owner (`Vec[int]`) gives its owner arguments; any other is unified with the binding's type.
    pub(in crate::typechecker) fn fix_open_rust_binding_by_destination(
        &mut self,
        expr: &Spanned<Expr>,
        expected: &ResolvedType,
    ) -> Option<ResolvedType> {
        let Expr::Ident(name) = &expr.node else {
            return None;
        };
        let (index, template) = self.open_rust_binding_read(name, expr.span)?;
        let owner = self.open_rust_generic_bindings[index]
            .call
            .native_item
            .clone()
            .and_then(|item| Some((self.rust_item_metadata_for_path(&item)?, item)));
        let owner_arguments = owner.and_then(|(metadata, item)| {
            let RustItemKind::Type(owner) = &metadata.kind else {
                return None;
            };
            let arguments = self.rust_owner_type_args(expected, &item)?;
            Some(
                owner
                    .type_params
                    .iter()
                    .cloned()
                    .zip(arguments)
                    .collect::<HashMap<_, _>>(),
            )
        });
        let arguments = owner_arguments.unwrap_or_else(|| {
            let mut arguments = HashMap::new();
            self.infer_type_param_bindings(&template, expected, &mut arguments);
            arguments
        });
        self.fix_open_rust_binding(index, &template, &arguments)
    }

    /// Find the watched binding that the read at `read` reached first. Return its index and its checked type with each
    /// open parameter as a type variable of the same name. A read inside a closure fixes nothing: checking the
    /// initializer again there would count its reads as captures.
    fn open_rust_binding_read(&self, name: &str, read: Span) -> Option<(usize, ResolvedType)> {
        if self.closure_depth != 0 {
            return None;
        }
        let index = self
            .open_rust_generic_bindings
            .iter()
            .position(|binding| binding.name == name && binding.read_in.is_some())?;
        let binding = &self.open_rust_generic_bindings[index];
        let identity = binding.identity.as_ref()?;
        if self.type_info.resolved_identity(read) != Some(identity) {
            return None;
        }
        let symbol = self.symbols.lookup(name)?;
        let SymbolKind::Variable(info) = &self.symbols.get(symbol)?.kind else {
            return None;
        };
        let holes = &binding.call.type_params;
        let carrier = self.rust_inference_type(&info.ty, holes);
        let template = map_resolved_type_leaves(&carrier, &|leaf| match leaf {
            ResolvedType::RustPath(parameter) | ResolvedType::TypeVar(parameter) if holes.contains(parameter) => {
                Some(ResolvedType::TypeVar(parameter.clone()))
            }
            _ => None,
        });
        Some((index, template))
    }

    /// Fix the binding when `arguments` closes every open parameter: no unknown part and no type variable other than
    /// an enclosing source type parameter. Return the binding's fixed type, or `None` when the use leaves one open. A
    /// binding whose initializer cannot take the fixed type is refused for its open parameters.
    fn fix_open_rust_binding(
        &mut self,
        index: usize,
        template: &ResolvedType,
        arguments: &HashMap<String, ResolvedType>,
    ) -> Option<ResolvedType> {
        let source_parameters = self.native_call_type_parameters();
        let closed = self.open_rust_generic_bindings[index]
            .call
            .type_params
            .iter()
            .all(|parameter| {
                arguments.get(parameter).is_some_and(|argument| {
                    !Self::mentions_unknown_type(argument)
                        && !Self::mentions_foreign_type_variable(argument, &source_parameters)
                })
            });
        if !closed {
            return None;
        }
        let fixed = substitute_resolved_type(template, arguments);
        let binding = self.open_rust_generic_bindings.remove(index);
        if crate::trace_flags::trace_rust_facts() {
            eprintln!(
                "rust open owner fixed: {} = {}.{} => {fixed:?}",
                binding.name, binding.call.owner, binding.call.method
            );
        }
        let settled = self.settle_fixed_open_rust_binding(&binding, &fixed);
        if settled.is_none() {
            self.refuse_unselected_open_rust_generic_call(&binding.call, binding.span);
        }
        settled
    }

    /// Return whether `ty` has an unknown or still-inferred part anywhere.
    fn mentions_unknown_type(ty: &ResolvedType) -> bool {
        let unknown = std::cell::Cell::new(false);
        map_resolved_type_leaves(ty, &|leaf| {
            if matches!(leaf, ResolvedType::Unknown | ResolvedType::CallSiteInfer) {
                unknown.set(true);
            }
            None
        });
        unknown.get()
    }

    /// Give a binding the type its first use fixed. The initializer is checked again with that type as its
    /// destination, so the call retains its callee as an annotated binding's would; the checked type then replaces
    /// the binding's, and the reads checked before take it too. Return the checked type, or `None` when the
    /// initializer cannot take it.
    fn settle_fixed_open_rust_binding(
        &mut self,
        binding: &OpenRustGenericBinding,
        fixed: &ResolvedType,
    ) -> Option<ResolvedType> {
        let identity = binding.identity.clone()?;
        let symbol = self
            .symbols
            .lookup(&binding.name)
            .filter(|symbol| self.symbols.identity_of(*symbol) == Some(&identity))?;
        // ---- Initializer against the fixed type ----
        let value = &binding.assignment.value;
        let errors_before = self.errors.len();
        let value_ty = self.check_expr_with_expected(value, Some(fixed));
        self.open_rust_generic_calls.remove(&binding.span);
        if self.errors.len() != errors_before || !self.type_info.rust.checked_calls.contains_key(&binding.span) {
            return None;
        }
        // ---- The binding's checked type ----
        if let Some(symbol) = self.symbols.get_mut(symbol)
            && let SymbolKind::Variable(info) = &mut symbol.kind
        {
            info.ty = value_ty.clone();
        }
        self.record_assignment_binding_type(binding.statement, value_ty.clone());
        self.record_write_target_identity(binding.assignment.name_span, &binding.name);
        self.retain_rust_value_binding(&binding.name, value);
        let carrier = self
            .body_ir_checked_value_type(value.span)
            .cloned()
            .unwrap_or_else(|| value_ty.clone());
        self.type_info
            .expressions
            .body_ir_binding_types
            .insert(identity.clone(), carrier.clone());
        // ---- Reads checked before the binding was fixed ----
        let reads = self
            .type_info
            .references
            .resolved_identities
            .iter()
            .filter(|(_, selected)| **selected == identity)
            .map(|(span, _)| *span)
            .collect::<Vec<_>>();
        for read in reads {
            if self.type_info.expressions.expr_types.contains_key(&read) {
                self.record_expr_type(read, value_ty.clone());
            }
            if self.type_info.expressions.body_ir_expr_types.contains_key(&read) {
                self.type_info
                    .expressions
                    .body_ir_expr_types
                    .insert(read, carrier.clone());
            }
        }
        for operation in &binding.passive_uses {
            match operation {
                OpenRustPassiveUse::Expression(expression) => {
                    self.check_expr(expression);
                }
                OpenRustPassiveUse::Length { receiver, span } => {
                    let ty = self.check_expr(receiver);
                    self.retain_native_length_call(receiver, &ty, *span);
                }
            }
        }
        Some(value_ty)
    }
}
