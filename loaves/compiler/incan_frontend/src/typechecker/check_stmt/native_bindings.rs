//! Assignments that bind C ABI slots, spans and raw results, and open native generic call bindings.

use super::*;

impl TypeChecker {
    /// Give a compiler-managed C output constructor the ordinary local name supplied by its enclosing assignment.
    pub(super) fn bind_c_abi_output_slot_assignment(&mut self, name: &str, value_span: Span) {
        let Some(slot) = self.unbound_c_abi_output_slot_constructors.remove(&value_span) else {
            return;
        };
        self.pending_c_abi_output_slots.insert(name.to_string(), slot);
    }

    /// Bind an opaque checked typed span only when its constructor is the exact value of one ordinary local.
    pub(super) fn bind_c_abi_span_assignment(&mut self, name: &str, value_span: Span) {
        let Some(kind) = self.unbound_c_abi_span_constructors.remove(&value_span) else {
            return;
        };
        let Some(binding_span) = self
            .symbols
            .lookup(name)
            .and_then(|symbol_id| self.symbols.get(symbol_id))
            .map(|symbol| symbol.span)
        else {
            return;
        };
        self.c_abi_span_bindings
            .insert(name.to_string(), CAbiSpanLocal { kind, binding_span });
    }

    /// Reject constructors that were nested in a return, collection, field, callback, or ordinary call instead of
    /// receiving the compiler-tracked local owner required by the checked bridge.
    pub(super) fn reject_unbound_c_abi_span_constructors(&mut self) {
        let constructors = std::mem::take(&mut self.unbound_c_abi_span_constructors);
        for (span, _kind) in constructors {
            self.errors.push(crate::diagnostics::CompileError::type_error(
                "a checked byte carrier must be assigned directly to one local before its closed bridge methods are used"
                    .to_string(),
                span,
            ));
        }
    }

    /// Bind a scoped C text view to the new local whose value it is, at the current closure depth (RFC 116).
    ///
    /// The local may then be read as the receiver of `copy_utf8` only; see `check_ident`.
    pub(super) fn bind_scoped_c_string_view_assignment(&mut self, binding_span: Span, value_span: Span) {
        if self.unbound_scoped_c_string_views.remove(&value_span) {
            self.scoped_c_string_view_bindings
                .insert(binding_span, self.closure_depth);
        }
    }

    /// Refuse each scoped C text view the statement just checked neither bound to a local nor copied (RFC 116).
    ///
    /// A view that is returned, stored, passed on or placed in a value would outlive the memory it points into. A bare
    /// expression statement that only discards the view is not an escape.
    pub(super) fn reject_unbound_scoped_c_string_views(&mut self, statement: &Statement) {
        if let Statement::Expr(expr) = statement {
            self.unbound_scoped_c_string_views.remove(&expr.span);
        }
        let mut escaped = std::mem::take(&mut self.unbound_scoped_c_string_views)
            .into_iter()
            .collect::<Vec<_>>();
        escaped.sort_unstable();
        for span in escaped {
            self.errors.push(errors::scoped_c_string_view_escapes(span));
        }
    }

    /// Give a checked C raw call result the ordinary local name supplied by its enclosing assignment.
    pub(super) fn bind_c_abi_raw_result_assignment(&mut self, name: &str, value_span: Span) {
        let Some(mut result) = self.unbound_c_abi_raw_call_results.remove(&value_span) else {
            return;
        };
        result.local_symbol_span = self
            .symbols
            .lookup(name)
            .and_then(|symbol_id| self.symbols.get(symbol_id))
            .map(|symbol| symbol.span);
        result.local_name = Some(name.to_string());
        self.c_abi_raw_call_results.push(result);
    }

    /// Attach the open native call result a new binding received to that binding, or refuse it at once when
    /// the binding is the discard `_` (#1720).
    ///
    /// The call was recorded by its span when the method call resolved. A direct `name = call()` binding is watched
    /// until its first use fixes the parameters, or until its block ends, so the refusal can name the binding to
    /// annotate if nothing reads it. `_` is never read, so the call is refused at once.
    pub(super) fn bind_open_rust_generic_assignment(&mut self, assign: &AssignmentStmt, statement: Span) {
        let value_span = assign.value.span;
        let Some(call) = self.open_rust_generic_calls.remove(&value_span) else {
            return;
        };
        if assign.name == "_" {
            if !call.owner_parameters {
                self.refuse_unselected_open_rust_generic_call(&call, value_span);
                return;
            }
            self.errors.push(errors::rust_owner_type_args_not_inferred(
                &call.owner,
                &call.method,
                &call.type_params,
                None,
                value_span,
            ));
            return;
        }
        let identity = self
            .type_info
            .resolved_write_identity(assign.name_span, &assign.name)
            .cloned();
        // The open native carrier and its item let the binding's methods dispatch to the native owner, so a method
        // call can be the use that fixes the parameters (#2481).
        if let Some(identity) = &identity {
            if let Some(native) = &call.native {
                self.type_info
                    .rust
                    .value_native_types
                    .insert(identity.clone(), native.clone());
            }
            if let Some(item) = &call.native_item {
                self.type_info
                    .rust
                    .value_item_paths
                    .insert(identity.clone(), item.clone());
            }
        }
        self.open_rust_generic_bindings.push(super::OpenRustGenericBinding {
            name: assign.name.clone(),
            block: self.statement_blocks.current,
            span: value_span,
            call,
            assignment: assign.clone(),
            statement,
            identity,
            read_in: None,
            active_reads: Vec::new(),
            passive_uses: Vec::new(),
        });
    }

    /// Refuse an open native call whose value a bare expression statement throws away (#1720, #2481).
    ///
    /// `HashMap.new()` on its own line binds nothing, so no later statement can supply the type arguments Rust
    /// would need; the call is refused where it stands.
    pub(super) fn reject_discarded_open_rust_generic_call(&mut self, expr_span: Span) {
        let Some(call) = self.open_rust_generic_calls.remove(&expr_span) else {
            return;
        };
        if !call.owner_parameters {
            self.refuse_unselected_open_rust_generic_call(&call, expr_span);
            return;
        }
        self.errors.push(errors::rust_owner_type_args_not_inferred(
            &call.owner,
            &call.method,
            &call.type_params,
            None,
            expr_span,
        ));
    }

    /// Note the first read of the open Rust generic bindings named `name`, by a read or a reassignment (#1720).
    ///
    /// The use that read is the one that must fix the owner's type arguments, and it fixes them while it is checked;
    /// the end of the statement holding the read refuses a binding it left open (#2481).
    pub(in crate::typechecker) fn mark_open_rust_generic_binding_read(&mut self, name: &str, span: Span) {
        let block = self.statement_blocks.current;
        for binding in &mut self.open_rust_generic_bindings {
            if binding.name == name {
                binding.read_in.get_or_insert(block);
                if !binding.active_reads.contains(&span) {
                    binding.active_reads.push(span);
                }
            }
        }
    }

    /// Refuse an open Rust associated call that is no longer watched when checking retained no callee for it.
    pub(in crate::typechecker) fn refuse_unselected_open_rust_generic_call(
        &mut self,
        call: &super::OpenRustGenericCall,
        span: Span,
    ) {
        if self.type_info.rust.checked_calls.contains_key(&span) {
            return;
        }
        let callee = format!("{}.{}", call.owner, call.method);
        self.errors.push(errors::rust_callee_not_selected(
            &callee,
            errors::NativeCalleeGap::OpenTypeParameters(&call.type_params),
            span,
        ));
    }

    /// Refuse every open native generic binding that block `block` declared and no fixing use settled (#1720).
    ///
    /// Called when that block ends, after settling a read the block's last statement did not. Bindings of enclosing
    /// blocks stay watched: one after this block can still read them.
    pub(in crate::typechecker) fn reject_unread_open_rust_generic_bindings_of_block(&mut self, block: usize) {
        self.settle_read_open_rust_generic_bindings(block);
        let (left, still_watched): (Vec<_>, Vec<_>) = std::mem::take(&mut self.open_rust_generic_bindings)
            .into_iter()
            .partition(|binding| binding.block == block);
        self.open_rust_generic_bindings = still_watched;
        let (read, left_unread): (Vec<_>, Vec<_>) = left.into_iter().partition(|binding| binding.read_in.is_some());
        for binding in read {
            self.refuse_unselected_open_rust_generic_call(&binding.call, binding.span);
        }
        for binding in left_unread {
            if !binding.call.owner_parameters {
                self.refuse_unselected_open_rust_generic_call(&binding.call, binding.span);
                continue;
            }
            self.errors.push(errors::rust_owner_type_args_not_inferred(
                &binding.call.owner,
                &binding.call.method,
                &binding.call.type_params,
                Some(binding.name.as_str()),
                binding.span,
            ));
        }
    }

    /// Record the existing native usize-to-int return conversion only for a retained Rust call. The exact native
    /// signature remains unchanged; Body IR consumes this decision without widening ordinary source compatibility.
    pub(super) fn record_native_usize_return(
        &mut self,
        expr: Option<&Spanned<Expr>>,
        actual: &ResolvedType,
        expected: &ResolvedType,
    ) -> bool {
        if *expected != ResolvedType::Int
            || *actual != ResolvedType::Numeric(incan_lang::lang::types::numerics::NumericTypeId::USize)
        {
            return false;
        }
        let Some(mut expr) = expr else { return false };
        let boundary_span = expr.span;
        while let Expr::Paren(inner) = &expr.node {
            expr = inner;
        }
        let accepted = self
            .type_info
            .rust
            .checked_calls
            .get(&expr.span)
            .is_some_and(|call| &call.return_type == actual);
        if accepted {
            self.type_info
                .expressions
                .numeric_conversion_destinations
                .insert(boundary_span, expected.clone());
        }
        accepted
    }
}
