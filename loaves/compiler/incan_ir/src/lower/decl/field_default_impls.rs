//! `Default` for a model or class that derives it and declares field defaults (#1879).
//!
//! Rust's `#[derive(Default)]` gives every field its type's default and ignores the defaults the source declares, so
//! `Type.default()` would not be the value `Type()` constructs. A model or class that derives `Default` and declares a
//! field default therefore lowers to an explicit `impl Default` instead: its `default()` constructs the value the
//! ordinary way, passing the type's default for each field that declares none, so every declared default applies as it
//! does at any other construction. A declaration without field defaults keeps the derive, whose result is the same.

use super::super::super::decl::{IrFunction, IrImpl, IrStruct, IrTraitBound, StructField, Visibility};
use super::super::super::expr::IrExprKind;
use super::super::super::stmt::{IrStmt, IrStmtKind};
use super::super::super::types::IrType;
use super::super::super::{IrSpan, TypedExpr};
use super::super::AstLowering;
use incan_frontend::ast::{self, Spanned};
use incan_lang::lang::derives::{self, DeriveId};
use incan_lang::lang::stdlib;
use incan_lang::lang::trait_bounds;

impl AstLowering {
    /// Whether a model's or class's derive list asks for `Default` while one of its fields declares a default, so the
    /// derive must give way to [`Self::lower_field_default_impl`].
    pub(in crate::lower) fn default_derive_over_field_defaults(derives: &[String], fields: &[StructField]) -> bool {
        let default = derives::as_str(DeriveId::Default);
        derives.iter().any(|derive| Self::same_derive(derive, default))
            && fields.iter().any(|field| field.default.is_some())
    }

    /// Remove `Default` from a model's or class's derive list when [`Self::lower_field_default_impl`] implements it.
    pub(in crate::lower) fn defer_default_derive_to_field_defaults(derives: &mut Vec<String>, fields: &[StructField]) {
        if Self::default_derive_over_field_defaults(derives, fields) {
            let default = derives::as_str(DeriveId::Default);
            derives.retain(|derive| !Self::same_derive(derive, default));
        }
    }

    /// Lower the `impl Default` of a model or class that derives `Default` and declares field defaults, or `None` when
    /// it keeps the derive.
    ///
    /// `default()` returns the struct constructed with `Default::default()` for each field that declares no default;
    /// construction fills the declared ones. A derive bounds every type parameter by the derived trait, so the impl
    /// bounds each by `Default` too.
    pub(in crate::lower) fn lower_field_default_impl(
        &mut self,
        lowered: &IrStruct,
        decorators: &[Spanned<ast::Decorator>],
    ) -> Option<IrImpl> {
        let (derives, _) = self.extract_derives(decorators);
        if !Self::default_derive_over_field_defaults(&derives, &lowered.fields) {
            return None;
        }
        let default_trait = derives::as_str(DeriveId::Default);
        let rust_default = trait_bounds::incan_to_rust(default_trait).unwrap_or(default_trait);
        let mut type_params = lowered.type_params.clone();
        for param in &mut type_params {
            if !param.bounds.iter().any(|bound| bound.trait_path == rust_default) {
                param.bounds.push(IrTraitBound::simple(rust_default));
            }
        }
        let self_ty = if lowered.type_params.is_empty() {
            IrType::Struct(lowered.name.clone())
        } else {
            IrType::NamedGeneric(
                lowered.name.clone(),
                lowered
                    .type_params
                    .iter()
                    .map(|param| IrType::Generic(param.name.clone()))
                    .collect(),
            )
        };
        let fields = lowered
            .fields
            .iter()
            .filter(|field| field.default.is_none())
            .map(|field| (field.name.clone(), Self::type_default_value(default_trait, &field.ty)))
            .collect();
        let value = TypedExpr::new(
            IrExprKind::Struct {
                name: lowered.name.clone(),
                type_args: Vec::new(),
                fields,
                fill_defaults: false,
            },
            self_ty.clone(),
        );
        let default_fn = IrFunction {
            name: "default".to_string(),
            docstring: None,
            params: Vec::new(),
            return_type: self_ty,
            body: vec![IrStmt {
                kind: IrStmtKind::Return(Some(value)),
                span: IrSpan::default(),
            }],
            is_async: false,
            is_generator: false,
            visibility: Visibility::Private,
            type_params: Vec::new(),
            is_extern: false,
            rust_extern_name: None,
            rust_attributes: Vec::new(),
            lint_allows: Vec::new(),
        };
        let module_path = stdlib::trait_method_module_segments(default_trait);
        Some(IrImpl {
            target_type: lowered.name.clone(),
            type_params,
            trait_name: Some(default_trait.to_string()),
            trait_module_path: module_path,
            trait_source_name: Some(default_trait.to_string()),
            trait_type_args: Vec::new(),
            associated_types: Vec::new(),
            methods: vec![default_fn],
            method_projections: Vec::new(),
            source_method_projections: Vec::new(),
        })
    }

    /// Return `Default::default()` typed as `ty`: the default of a field that declares none.
    fn type_default_value(default_trait: &str, ty: &IrType) -> TypedExpr {
        let function = TypedExpr::new(
            IrExprKind::AssociatedFunction {
                type_name: default_trait.to_string(),
                function_name: "default".to_string(),
            },
            IrType::Function {
                params: Vec::new(),
                ret: Box::new(ty.clone()),
            },
        );
        TypedExpr::new(
            IrExprKind::Call {
                func: Box::new(function),
                type_args: Vec::new(),
                args: Vec::new(),
                callable_signature: None,
                canonical_path: None,
            },
            ty.clone(),
        )
    }
}
