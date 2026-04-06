//! Emit Rust code for method calls.
//!
//! This module handles emission of both known methods (enum-based dispatch via `MethodKind`) and ordinary method calls
//! that should remain Rust method syntax.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use super::super::super::conversions::{ConversionContext, determine_conversion};
use super::super::super::expr::{
    InternalMethodKind, IrCallArg, IrExprKind, MethodCallArgPolicy, MethodKind, TypedExpr, VarRefKind,
};
use super::super::super::types::IrType;
use super::super::{EmitError, IrEmitter};

mod collection_methods;
mod string_methods;

use collection_methods::emit_collection_method;
use string_methods::emit_string_method;

/// Compute common receiver setup for method emission.
///
/// This deduplicates the pattern of:
/// - Detecting `FrozenStr` receivers
/// - Unwrapping them via `.as_str()`
pub(super) struct ReceiverInfo {
    /// The receiver token stream (possibly wrapped in `.as_str()` for FrozenStr).
    pub(super) r: TokenStream,
    /// A borrow of the receiver: `&#r`.
    pub(super) r_borrow: TokenStream,
}

impl ReceiverInfo {
    /// Build receiver info from the receiver type and emitted receiver tokens.
    fn new(receiver_ty: &IrType, emitted: TokenStream) -> Self {
        let is_frozen_str = matches!(receiver_ty, IrType::FrozenStr);
        let r = if is_frozen_str {
            quote! { #emitted.as_str() }
        } else {
            emitted
        };
        let r_borrow = quote! { &#r };
        Self { r, r_borrow }
    }
}

impl<'a> IrEmitter<'a> {
    /// Strip reference wrappers from a receiver type before builtin-family or ownership-sensitive dispatch.
    ///
    /// Method emission cares about the underlying receiver family (`Dict`, `Struct`, `Trait`, ...) rather than whether
    /// lowering represented the value as `T`, `&T`, or `&mut T`.
    fn receiver_type_for_method_dispatch(receiver_ty: &IrType) -> &IrType {
        let mut receiver_ty = receiver_ty;
        while let IrType::Ref(inner) | IrType::RefMut(inner) = receiver_ty {
            receiver_ty = inner.as_ref();
        }
        receiver_ty
    }

    /// Whether a receiver is a nominal type owned by this compilation unit rather than an external Rust surface.
    ///
    /// Incan-owned structs, enums, traits, and `rusttype` surface aliases use compiler-controlled argument conversion
    /// rules. External nominal types may share the same IR shape (`Struct(name)`) but must usually preserve Rust call
    /// semantics instead.
    fn is_incan_owned_nominal_receiver(&self, receiver_ty: &IrType) -> bool {
        match Self::receiver_type_for_method_dispatch(receiver_ty) {
            IrType::Struct(name) => {
                self.struct_field_names.contains_key(name) || self.rusttype_alias_names.contains(name)
            }
            IrType::Enum(name) => self.enum_variant_fields.keys().any(|(enum_name, _)| enum_name == name),
            IrType::Trait(_) => true,
            _ => false,
        }
    }

    /// Emit a known method call using enum-based dispatch.
    ///
    /// This handles calls that have been lowered to `IrExprKind::KnownMethodCall`.
    ///
    /// ## Parameters
    ///
    /// - `receiver`: The receiver expression
    /// - `kind`: The method kind enum variant
    /// - `args`: The method call arguments
    ///
    /// ## Returns
    ///
    /// - A Rust `TokenStream` for the method call
    pub(in super::super) fn emit_known_method_call(
        &self,
        receiver: &TypedExpr,
        kind: &MethodKind,
        args: &[IrCallArg],
    ) -> Result<TokenStream, EmitError> {
        let r0 = self.emit_expr(receiver)?;
        let info = ReceiverInfo::new(&receiver.ty, r0);
        let arg_exprs: Vec<TypedExpr> = args.iter().map(|a| a.expr.clone()).collect();
        match kind {
            MethodKind::String(kind) => emit_string_method(self, &info, kind, &arg_exprs),
            MethodKind::Collection(kind) => emit_collection_method(self, receiver, &info, kind, &arg_exprs),
            MethodKind::Internal(InternalMethodKind::Slice) => self.emit_runtime_str_slice(&info, &arg_exprs),
        }
    }

    /// Emit a method call expression that remains a regular Rust method call.
    ///
    /// This handles `IrExprKind::MethodCall` when lowering did not classify the method as a builtin-family method.
    pub(in super::super) fn emit_method_call_expr(
        &self,
        receiver: &TypedExpr,
        method: &str,
        args: &[IrCallArg],
        arg_policy: MethodCallArgPolicy,
    ) -> Result<TokenStream, EmitError> {
        let r0 = self.emit_expr(receiver)?;
        let info = ReceiverInfo::new(&receiver.ty, r0);
        let r = &info.r;
        let arg_exprs: Vec<TypedExpr> = args.iter().map(|a| a.expr.clone()).collect();

        // Check if this is an enum variant construction.
        //
        // Important: do NOT treat any uppercase variable as a type name. Only rewrite when we actually know this
        // (Type, Variant) pair exists in the enum variant registry.
        if let IrExprKind::Var { name, .. } = &receiver.kind {
            let key = (name.to_string(), method.to_string());
            if self.enum_variant_fields.contains_key(&key) {
                return self.emit_enum_variant_call(name, method, &arg_exprs);
            }
        }

        // Associated function call on a type: `Type.method(...)` → `Type::method(...)`
        //
        // This is needed for external Rust types like `Uuid`, `Instant`, `HashMap`, and also for
        // Incan-generated impl methods called in a "static" style (e.g. `User.from_json(...)`).
        if let IrExprKind::Var { name, .. } = &receiver.kind {
            // Rewrite `Type.method(...)` to `Type::method(...)` only when we have explicit metadata that this is
            // a type-like identifier (type name or external import placeholder).
            //
            // This avoids capitalization heuristics that can mis-emit runtime variables named `TitleCase`.
            if Self::expr_is_type_like(receiver) {
                let type_ident = format_ident!("{}", name);
                let m = format_ident!("{}", method);
                let in_return = *self.in_return_context.borrow();
                let receiver_ref_kind = match &receiver.kind {
                    IrExprKind::Var { ref_kind, .. } => Some(*ref_kind),
                    _ => None,
                };
                // Apply Incan-style argument conversions when calling associated functions on Incan-owned types
                // (structs/enums/traits). This is important for `str` literals which are emitted as `&'static str`,
                // but many Incan-level signatures expect owned `String` in Rust (e.g., newtype `from_underlying(v:
                // str)`).
                //
                // `VarRefKind::TypeName` covers imported Incan types like `std.web.Response` (typechecker
                // `IdentKind::TypeName`). Those calls must use Incan arg rules — `ExternalFunctionArg`
                // would borrow `String` as `&String` and break signatures such as
                // `Response::html(content: String)` in generated stdlib.
                //
                // For external Rust types (VarRefKind::ExternalRustName), use ExternalFunctionArg conversions so that
                // string literals get `.into()` — this lets the Rust compiler resolve the target type via the Into
                // trait (e.g., Polars' PlSmallStr, sqlx identifiers, etc.).
                let conversion_context = if receiver_ref_kind != Some(VarRefKind::ExternalRustName)
                    && self.is_incan_owned_nominal_receiver(&receiver.ty)
                {
                    ConversionContext::IncanFunctionArg
                } else if matches!(receiver_ref_kind, Some(VarRefKind::ExternalName | VarRefKind::TypeName)) {
                    if in_return {
                        ConversionContext::IncanFunctionArgInReturn
                    } else {
                        ConversionContext::IncanFunctionArg
                    }
                } else {
                    ConversionContext::ExternalFunctionArg
                };

                let arg_tokens: Vec<TokenStream> = if matches!(
                    conversion_context,
                    ConversionContext::IncanFunctionArg | ConversionContext::IncanFunctionArgInReturn
                ) {
                    arg_exprs
                        .iter()
                        .map(|a| {
                            let emitted = self.emit_expr(a)?;
                            let conv = determine_conversion(a, None, conversion_context);
                            Ok(conv.apply(emitted))
                        })
                        .collect::<Result<_, _>>()?
                } else {
                    // External types: apply ExternalFunctionArg conversions so that string literals get `.into()`
                    // (resolves to the correct target type via Rust's Into trait — e.g., Polars' PlSmallStr, sqlx
                    // identifiers, etc.)
                    arg_exprs
                        .iter()
                        .map(|a| {
                            let emitted = self.emit_expr(a)?;
                            let conv = determine_conversion(a, None, conversion_context);
                            Ok(conv.apply(emitted))
                        })
                        .collect::<Result<_, _>>()?
                };
                return Ok(quote! { #type_ident::#m(#(#arg_tokens),*) });
            }
        }

        // Regular method call
        let m = format_ident!("{}", method);
        // Apply Incan-style argument conversions for methods on nominal types emitted by this compilation unit.
        // This is important for `str` literals: we often emit `"x"` as `&'static str`, but many Incan-level method
        // signatures expect owned `String` in Rust.
        //
        // Do not key this off `IrType::Struct` alone: Rust interop types such as `HashMap` also lower to nominal IR
        // types, but they should still use Rust call semantics rather than compiler-owned cloning/to_string policies.
        let in_return = *self.in_return_context.borrow();
        let receiver_ref_kind = match &receiver.kind {
            IrExprKind::Var { ref_kind, .. } => Some(*ref_kind),
            _ => None,
        };
        let conversion_context = if receiver_ref_kind != Some(VarRefKind::ExternalRustName)
            && self.is_incan_owned_nominal_receiver(&receiver.ty)
        {
            ConversionContext::IncanFunctionArg
        } else if receiver_ref_kind == Some(VarRefKind::ExternalName) {
            // Module-qualified calls like `widgets.make_widget(...)` are function namespace lookups, not external Rust
            // methods. They should keep ordinary Incan/public-function conversions instead of Rust interop coercions.
            if in_return {
                ConversionContext::IncanFunctionArgInReturn
            } else {
                ConversionContext::IncanFunctionArg
            }
        } else if matches!(arg_policy, MethodCallArgPolicy::PreserveShape) {
            // Lowering recorded that this ordinary Rust method call must keep borrow-sensitive lookup semantics, so do
            // not apply function-style coercions such as `.to_string()` / `.into()`.
            ConversionContext::MethodArg
        } else {
            ConversionContext::ExternalFunctionArg
        };
        let arg_tokens: Vec<TokenStream> = arg_exprs
            .iter()
            .map(|a| {
                let emitted = self.emit_expr(a)?;
                let conv = determine_conversion(a, None, conversion_context);
                Ok(conv.apply(emitted))
            })
            .collect::<Result<_, _>>()?;
        Ok(quote! { #r.#m(#(#arg_tokens),*) })
    }

    /// Emit a runtime string slice call using shared stdlib/semantics helpers.
    ///
    /// This ensures emitted Rust uses the same Unicode/panic behavior as runtime and avoids drift
    /// from direct range slicing on Rust strings.
    fn emit_runtime_str_slice(&self, info: &ReceiverInfo, args: &[TypedExpr]) -> Result<TokenStream, EmitError> {
        let r_borrow = &info.r_borrow;

        let start_tokens = if let Some(arg0) = args.first() {
            let start = self.emit_expr(arg0)?;
            quote! { Some((#start) as i64) }
        } else {
            quote! { None }
        };

        let end_tokens = if let Some(arg1) = args.get(1) {
            if matches!(arg1.kind, IrExprKind::Int(-1)) {
                quote! { None }
            } else {
                let end = self.emit_expr(arg1)?;
                quote! { Some((#end) as i64) }
            }
        } else {
            quote! { None }
        };

        Ok(quote! { incan_stdlib::strings::str_slice(#r_borrow, #start_tokens, #end_tokens, None) })
    }

    /// Emit an enum variant construction call (Type.Variant(...) -> Type::Variant(...)).
    pub(in super::super) fn emit_enum_variant_call(
        &self,
        type_name: &str,
        variant: &str,
        args: &[TypedExpr],
    ) -> Result<TokenStream, EmitError> {
        let variant_key = (type_name.to_string(), variant.to_string());
        let arg_tokens: Vec<TokenStream> = if let Some(fields) = self.enum_variant_fields.get(&variant_key) {
            match fields {
                super::super::super::decl::VariantFields::Unit => Vec::new(),
                super::super::super::decl::VariantFields::Tuple(field_tys) => args
                    .iter()
                    .zip(field_tys.iter())
                    .map(|(a, ty)| {
                        let emitted = self.emit_expr(a)?;
                        let conv = determine_conversion(a, Some(ty), ConversionContext::IncanFunctionArg);
                        Ok(conv.apply(emitted))
                    })
                    .collect::<Result<_, _>>()?,
                super::super::super::decl::VariantFields::Struct(_) => args
                    .iter()
                    .map(|a| {
                        let emitted = self.emit_expr(a)?;
                        let conv = determine_conversion(a, None, ConversionContext::IncanFunctionArg);
                        Ok(conv.apply(emitted))
                    })
                    .collect::<Result<_, _>>()?,
            }
        } else {
            args.iter()
                .map(|a| {
                    let emitted = self.emit_expr(a)?;
                    let conv = determine_conversion(a, Some(&IrType::String), ConversionContext::IncanFunctionArg);
                    Ok(conv.apply(emitted))
                })
                .collect::<Result<_, _>>()?
        };

        let type_ident = format_ident!("{}", type_name);
        let m = format_ident!("{}", variant);
        Ok(quote! { #type_ident::#m(#(#arg_tokens),*) })
    }
}
