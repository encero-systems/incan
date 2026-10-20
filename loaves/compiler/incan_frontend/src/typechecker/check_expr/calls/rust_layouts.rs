//! Retain metadata-selected native struct layouts and construction bindings for Body IR.

use std::collections::HashMap;

use incan_lang::interop::RustTypeInfo;
use incan_semantics_core::{SemanticSourceTargetKind, body_ir};

use super::TypeChecker;
use crate::ast::{CallArg, ParamKind, Span};
use crate::symbols::{CallableParam, ResolvedType};
use crate::typechecker::helpers::render_resolved_type_as_rust_arg;
use crate::typechecker::type_info::{ConstructorFieldBinding, RustCheckedLayout};

impl TypeChecker {
    /// Retain ordered public field templates once, keyed by their public native owner. No hidden-field access is
    /// inferred: the adapter must verify the complete native field count, visibility and instantiated types.
    pub(in crate::typechecker::check_expr) fn retain_rust_layout(&mut self, path: &str, info: &RustTypeInfo) {
        let path = Self::rust_generic_base_and_args(path).map_or(path, |(base, _)| base);
        let Some(owner) = body_ir::rust_item_identity(path) else {
            return;
        };
        let fields = info
            .fields
            .iter()
            .map(|field| {
                CallableParam::named(
                    &field.name,
                    self.rust_inference_type(
                        &self.native_rust_signature_type(&field.type_display, path),
                        &info.type_params,
                    ),
                    ParamKind::Normal,
                )
            })
            .collect();
        let field_identities = info
            .fields
            .iter()
            .map(|field| Self::synthetic_member_identity(&owner, &field.name, SemanticSourceTargetKind::Field))
            .collect();
        self.type_info.rust.checked_layouts.insert(
            path.to_owned(),
            RustCheckedLayout {
                type_parameters: info.type_params.clone(),
                fields,
                field_identities,
            },
        );
    }

    /// Close the native owner from the already checked field arguments and retain the ordinary constructor slots.
    /// A missing/default-filled field or unresolved type binder grants no executable construction.
    pub(super) fn retain_rust_construction(
        &mut self,
        path: &str,
        info: &RustTypeInfo,
        args: &[CallArg],
        selected: &[String],
        span: Span,
    ) -> Option<ResolvedType> {
        if args.len() != info.fields.len() || selected.len() != args.len() || info.fields.is_empty() {
            return None;
        }
        self.retain_rust_layout(path, info);
        let base = Self::rust_generic_base_and_args(path).map_or(path, |(base, _)| base);
        let layout = self.type_info.rust.checked_layouts.get(base)?;
        let mut bindings = HashMap::new();
        let mut slots = Vec::with_capacity(args.len());
        for (arg, selected) in args.iter().zip(selected) {
            let expression = match arg {
                CallArg::Positional(expr) | CallArg::Named(_, expr) => expr,
                _ => return None,
            };
            let slot = layout
                .fields
                .iter()
                .position(|field| field.name.as_ref() == Some(selected))?;
            let actual = self.type_info.expr_type(expression.span)?;
            self.infer_type_param_bindings(&layout.fields[slot].ty, actual, &mut bindings);
            slots.push(slot);
        }
        let arguments = layout
            .type_parameters
            .iter()
            .map(|name| bindings.get(name).cloned())
            .collect::<Option<Vec<_>>>()?;
        if arguments
            .iter()
            .any(|argument| matches!(argument, ResolvedType::Unknown | ResolvedType::CallSiteInfer))
        {
            return None;
        }
        let result = ResolvedType::Generic(base.to_owned(), arguments);
        self.type_info
            .calls
            .body_ir_constructor_types
            .insert((span.start, span.end), result.clone());
        self.type_info.calls.body_ir_native_constructor_bindings.insert(
            (span.start, span.end),
            (
                body_ir::rust_item_identity(base)?,
                ConstructorFieldBinding {
                    argument_slots: slots,
                    defaulted_slots: Vec::new(),
                    field_count: info.fields.len(),
                },
            ),
        );
        Some(ResolvedType::RustPath(render_resolved_type_as_rust_arg(&result)))
    }
}
