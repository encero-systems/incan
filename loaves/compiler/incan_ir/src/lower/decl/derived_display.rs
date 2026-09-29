//! RFC 000 `@derive(Display)`: a type that derives `Display` displays as its `{value:?}` structure.
//!
//! Rust has no `Display` derive (#1872), so the derive is not passed on as one. The typechecker records each model,
//! class, enum or newtype this module declares whose derived `Display` gives it its display form (see
//! `TypeCheckInfo::type_derives_display`), and lowering gives such a type a private `__str__` that returns the value's
//! `Debug` rendering: the text `f"{value:?}"` produces, which is the form the display rule gives a model, class or enum
//! value inside a collection. The emitter implements `Display` for every type with a `__str__`, so each display
//! position and each `Display` bound writes that text. The `Debug` the rendering needs is the one `Display` implies
//! (see `incan_lang::lang::derives::DERIVE_IMPLICATIONS`); for a generic type it holds for type arguments that
//! implement `Debug`, so the impl block that carries the method bounds each of the type's own parameters by it.

use super::super::super::decl::{FunctionParam, IrFunction, IrTraitBound, IrTypeParam, Visibility};
use super::super::super::expr::{FormatPart, FormatStyle, IrExprKind, VarAccess, VarRefKind};
use super::super::super::stmt::{IrStmt, IrStmtKind};
use super::super::super::types::IrType;
use super::super::super::{Mutability, TypedExpr};
use super::super::AstLowering;
use incan_frontend::ast;
use incan_lang::lang::keywords::{self, KeywordId};
use incan_lang::lang::magic_methods::{self, MagicMethodId};
use incan_lang::lang::trait_bounds::{self, TraitBoundId};

impl AstLowering {
    /// Whether the typechecker recorded that the declared type `type_name` displays through its derived `Display`.
    pub(in crate::lower) fn type_derives_display(&self, type_name: &str) -> bool {
        self.type_info
            .as_ref()
            .is_some_and(|info| info.type_derives_display(type_name))
    }

    /// Return the `__str__` a declared type with a derived `Display` is given, or `None` for any other type.
    ///
    /// The method returns `f"{self:?}"`. The receiver is typed as the declared type with its own type parameters, as
    /// in [`Self::error_message_str_method`]. The method is private: nothing but the type's `Display` calls it, and the
    /// type declares no `__str__` of its own for it to shadow, which the typechecker refuses beside the derive.
    pub(in crate::lower) fn derived_display_str_method(
        &self,
        type_name: &str,
        type_params: &[ast::TypeParam],
    ) -> Option<IrFunction> {
        if !self.type_derives_display(type_name) {
            return None;
        }
        let self_name = keywords::as_str(KeywordId::SelfKw).to_string();
        let receiver = TypedExpr::new(
            IrExprKind::Var {
                name: self_name.clone(),
                access: VarAccess::Borrow,
                ref_kind: VarRefKind::Value,
            },
            Self::trait_impl_owner_type(type_name, type_params),
        );
        let rendered = TypedExpr::new(
            IrExprKind::Format {
                parts: vec![FormatPart::Expr {
                    expr: receiver,
                    style: FormatStyle::Debug,
                }],
            },
            IrType::String,
        );
        Some(IrFunction {
            name: magic_methods::as_str(MagicMethodId::Str).to_string(),
            docstring: None,
            params: vec![FunctionParam {
                name: self_name,
                // A method's `self` is typed by its impl, as in every lowered method.
                ty: IrType::Unknown,
                mutability: Mutability::Immutable,
                is_self: true,
                kind: ast::ParamKind::Normal,
                default: None,
            }],
            return_type: IrType::String,
            body: vec![IrStmt::new(IrStmtKind::Return(Some(rendered)))],
            is_async: false,
            is_generator: false,
            visibility: Visibility::Private,
            type_params: Vec::new(),
            is_extern: false,
            rust_extern_name: None,
            rust_attributes: Vec::new(),
            lint_allows: Vec::new(),
        })
    }

    /// Bound each type parameter of the inherent impl block of a type with a derived `Display` by `Debug`, which the
    /// `Debug` rendering its `__str__` returns needs of them; the block of any other type is left as it is.
    pub(in crate::lower) fn bound_type_params_for_derived_display(
        &self,
        type_name: &str,
        type_params: &mut [IrTypeParam],
    ) {
        if !self.type_derives_display(type_name) {
            return;
        }
        let Some(debug) = trait_bounds::rust_path(TraitBoundId::Debug) else {
            return;
        };
        for type_param in type_params {
            if !type_param.bounds.iter().any(|bound| bound.trait_path == debug) {
                type_param.bounds.push(IrTraitBound::simple(debug));
            }
        }
    }
}
