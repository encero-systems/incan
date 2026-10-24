//! Retain metadata-selected native struct layouts and construction bindings for Body IR.

use std::collections::HashMap;

use incan_lang::interop::{RustItemKind, RustTypeInfo};
use incan_semantics_core::{SemanticSourceTargetKind, body_ir};

use super::TypeChecker;
use crate::ast::{CallArg, Expr, ParamKind, Span, Spanned};
use crate::symbols::{CallableParam, ResolvedType, SymbolKind};
use crate::typechecker::helpers::render_resolved_type_as_rust_arg;
use crate::typechecker::type_info::{ConstructorFieldBinding, RustCheckedCall, RustCheckedEnum, RustCheckedLayout};

impl TypeChecker {
    /// Select the shared public carrier for native signatures, constructors, patterns and coverage. Lexical imports
    /// first select their admitted Rust path, then use the same definition-based binding as qualified spellings;
    /// collected public reexports reuse one carrier for the same native definition.
    pub(in crate::typechecker) fn rust_public_type_path(&self, path: &str) -> String {
        let path = self
            .lookup_symbol(path)
            .and_then(|symbol| match &symbol.kind {
                SymbolKind::RustItem(info) => Some(info.path.as_str()),
                _ => None,
            })
            .unwrap_or(path);
        let path = Self::normalize_rust_namespace_path(path);
        self.type_info
            .rust
            .item_public_paths
            .get(path)
            .map_or_else(|| path.to_owned(), Clone::clone)
    }

    /// Read a complete native enum template for both construction and matching. Exact field displays retain Box and
    /// reference storage; named slots follow metadata order. Neither caller may invent a partial executable layout.
    /// Unconditional Copy capability remains a separate checked fact from the open payload templates.
    pub(in crate::typechecker::check_expr) fn checked_rust_enum_layout(
        &self,
        path: &str,
        info: &RustTypeInfo,
    ) -> Option<RustCheckedEnum> {
        if info.variants.is_empty()
            || info.variants.iter().any(|variant| {
                variant.fields.len() != variant.field_type_displays.len()
                    || (!variant.field_names.is_empty() && variant.fields.len() != variant.field_names.len())
            })
        {
            return None;
        }
        Some(RustCheckedEnum {
            is_copy: self.derive_support(
                &ResolvedType::RustPath(path.into()),
                incan_lang::lang::derives::DeriveId::Copy,
            ) == crate::typechecker::derive_requirements::DeriveSupport::Supported,
            type_parameters: info.type_params.clone(),
            variants: info
                .variants
                .iter()
                .map(|variant| {
                    (
                        variant.name.clone(),
                        variant
                            .field_type_displays
                            .iter()
                            .map(|field| {
                                self.rust_inference_type(
                                    &self.native_rust_signature_type(field, path),
                                    &info.type_params,
                                )
                            })
                            .collect(),
                        variant.field_names.clone(),
                    )
                })
                .collect(),
        })
    }

    /// Retain a metadata-selected native variant and its storage adaptations. Named arguments select the same
    /// declaration slots as ordinary callable binding, independently of written order. Boxed fields carry an explicit
    /// owned Box construction into Body IR's argument boundary while the source signature stays semantic.
    pub(in crate::typechecker::check_expr) fn retain_rust_variant_construction(
        &mut self,
        path: &str,
        variant: &str,
        args: &[CallArg],
        params: &[CallableParam],
        span: Span,
    ) {
        // ---- Complete native layout ----
        let Some(metadata) = self.rust_item_metadata_for_complete_type(path) else {
            return;
        };
        let incan_lang::interop::RustItemKind::Type(info) = &metadata.kind else {
            return;
        };
        let Some(layout) = self.checked_rust_enum_layout(path, &info) else {
            return;
        };

        // ---- Variant identity ----
        let ancestral = incan_lang::interop::ancestral_rust_path(path);
        if matches!(ancestral.as_str(), "core::option::Option" | "core::result::Result") {
            let Some(constructor) = incan_lang::lang::surface::constructors::from_str(variant) else {
                return;
            };
            self.type_info.record_resolved_identity(
                span,
                crate::symbols::canonical_builtin_constructor_identity(constructor),
            );
        } else {
            let public_path = self.rust_public_type_path(path);
            let Some((identity, _)) = body_ir::native_enum_variant(&public_path, variant) else {
                return;
            };
            self.type_info.record_resolved_identity(span, identity);
            self.type_info.rust.checked_enums.insert(public_path, layout);
        }

        // ---- Written payload adjustments ----
        let Some(carriers) = self.rust_variant_payload_carriers(path, variant) else {
            return;
        };
        for (position, arg) in args.iter().enumerate() {
            let (slot, value) = match arg {
                CallArg::Positional(value) => (position, value),
                CallArg::Named(name, value) => {
                    let Some(slot) = params
                        .iter()
                        .position(|param| param.name.as_deref() == Some(name.node.as_str()))
                    else {
                        continue;
                    };
                    (slot, value)
                }
                _ => continue,
            };
            if carriers.get(slot) != Some(&incan_lang::interop::RustPayloadCarrier::Boxed) {
                continue;
            }
            let Some(param) = params.get(slot) else {
                continue;
            };
            let storage = self.native_signature_storage_owner(ResolvedType::Generic(
                "std::boxed::Box".into(),
                vec![param.ty.clone()],
            ));
            self.type_info.rust.argument_adjustments.insert(
                value.span,
                vec![RustCheckedCall {
                    storage: None,
                    source_type_parameters: self.native_call_type_parameters(),
                    receiver_adjustments: Vec::new(),
                    path: "std::boxed::Box::new".into(),
                    type_arguments: vec![param.ty.clone()],
                    parameters: vec![CallableParam::positional(param.ty.clone())],
                    return_type: storage,
                    result_item_path: Some("std::boxed::Box".into()),
                    has_receiver: false,
                }],
            );
        }
    }

    /// Retain a half-open integer range literal as the metadata-validated public native constructor selected by its
    /// argument slot. Source-only ranges keep their four-field layout; Body IR consumes these ordinary constructor
    /// facts without native lookup.
    pub(in crate::typechecker) fn retain_rust_range_argument(
        &mut self,
        expression: &Spanned<Expr>,
        target: &ResolvedType,
    ) -> bool {
        if let Expr::Paren(inner) = &expression.node {
            return self.retain_rust_range_argument(inner, target);
        }
        let Expr::Range {
            start,
            end,
            inclusive: false,
        } = &expression.node
        else {
            return false;
        };
        let Some(native) = self.rust_argument_native_type(expression) else {
            return false;
        };
        if !self.rust_trait_carriers_match(&native, target) {
            return false;
        }
        let path = "std::ops::Range";
        let Some(metadata) = self.rust_item_metadata_for_path(path) else {
            return false;
        };
        let RustItemKind::Type(info) = &metadata.kind else {
            return false;
        };
        let arguments = [
            CallArg::Positional(start.as_ref().clone()),
            CallArg::Positional(end.as_ref().clone()),
        ];
        let fields = ["start".to_owned(), "end".to_owned()];
        self.retain_rust_construction(path, &info, &arguments, &fields, expression.span)
            .is_some()
    }

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
                is_copy: self.derive_support(
                    &ResolvedType::RustPath(path.into()),
                    incan_lang::lang::derives::DeriveId::Copy,
                ) == crate::typechecker::derive_requirements::DeriveSupport::Supported,
                type_parameters: info.type_params.clone(),
                fields,
                field_identities,
            },
        );
    }

    /// Close the native owner from the already checked field arguments and retain the ordinary constructor slots.
    /// A missing/default-filled field or unresolved type binder grants no executable construction. A nongeneric
    /// declaration retains its bare owner identity rather than acquiring an empty generic application.
    pub(super) fn retain_rust_construction(
        &mut self,
        path: &str,
        info: &RustTypeInfo,
        args: &[CallArg],
        selected: &[String],
        span: Span,
    ) -> Option<ResolvedType> {
        if args.len() != info.fields.len() || selected.len() != args.len() {
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
        let result = if arguments.is_empty() {
            ResolvedType::RustPath(base.to_owned())
        } else {
            ResolvedType::Generic(base.to_owned(), arguments)
        };
        self.type_info
            .calls
            .body_ir_constructor_types
            .insert(span, result.clone());
        self.type_info.calls.body_ir_native_constructor_bindings.insert(
            span,
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
