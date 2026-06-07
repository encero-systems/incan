//! Centralized ownership and coercion planning for IR emission.
//!
//! This module is the backend's dedicated decision layer for "duckborrowing":
//! given a typed IR expression and a Rust sink/source boundary, decide whether emission should move, clone, borrow, or
//! materialize an owned string.
//!
//! Keep emitter modules calling this planner instead of open-coding ad hoc `.clone()`, `&`, `&mut`, `.to_string()`, or
//! `.into()` decisions.

use incan_core::interop::{RustTypeShape, RustTypeShapePathFallback, parse_rust_type_shape_text};
use proc_macro2::TokenStream;
use quote::quote;

use super::conversions::{
    Conversion as OwnershipPlan, ConversionContext, determine_conversion, determine_conversion_for_incan_call,
    incan_mutable_param_passed_as_rust_mut_ref,
};
use super::decl::FunctionParam;
use super::expr::{IrExpr, IrExprKind, MethodCallArgPolicy, VarAccess, VarRefKind};
use super::types::IrType;

/// A typed sink/source boundary that needs an ownership/coercion decision.
#[derive(Debug, Clone, Copy)]
pub enum ValueUseSite<'a> {
    /// Argument passed to an Incan-defined callable.
    ///
    /// Incan call boundaries normally expect owned values, but mutable aggregate parameters and return-position calls
    /// have special move/reborrow behavior.
    IncanCallArg {
        /// The callee parameter type after typechecking, when known.
        target_ty: Option<&'a IrType>,
        /// Full parameter metadata, used for mutable aggregate reborrow decisions.
        callee_param: Option<&'a FunctionParam>,
        /// Whether this call appears directly inside a `return`, allowing last-use moves to be preserved more
        /// aggressively.
        in_return: bool,
    },
    /// Argument passed to an external Rust callable.
    ///
    /// Rust interop boundaries preserve Rust API shapes: borrowed arguments stay borrowed and string-like values may
    /// use `.into()` rather than forcing Incan-owned `String` storage.
    ExternalCallArg {
        /// The external parameter type when Rust inspection provided one.
        target_ty: Option<&'a IrType>,
    },
    /// Value stored into a generated struct field.
    StructField {
        /// The declared field type, when available.
        target_ty: Option<&'a IrType>,
    },
    /// Value stored into an owned collection or tuple slot.
    CollectionElement {
        /// The element/key/value/tuple-slot type, when available.
        target_ty: Option<&'a IrType>,
    },
    /// Value assigned to a local binding or assignment target.
    Assignment {
        /// The assigned target type, when known.
        target_ty: Option<&'a IrType>,
    },
    /// Value returned from an Incan function.
    ReturnValue {
        /// The declared return type, when available.
        target_ty: Option<&'a IrType>,
    },
    /// Scrutinee consumed by a generated Rust `match`.
    MatchScrutinee {
        /// The scrutinee type, used to materialize owned values before pattern matching when necessary.
        target_ty: Option<&'a IrType>,
    },
    /// Method-style argument boundary where the method implementation controls final borrowing.
    MethodArg,
}

/// Receiver and lookup facts needed to choose the value-use site for one ordinary method-call argument.
///
/// This keeps clone-bound inference and method emission on the same method-argument boundary decision instead of
/// letting each phase classify receiver ownership independently.
#[derive(Debug, Clone, Copy)]
pub struct RegularMethodArgumentContext {
    pub arg_policy: MethodCallArgPolicy,
    pub receiver_ref_kind: Option<VarRefKind>,
    pub has_incan_method_signature: bool,
    pub is_incan_owned_nominal_receiver: bool,
    pub is_rusttype_alias_receiver: bool,
    pub preserves_lookup_arg_shape: bool,
    pub in_return: bool,
}

/// Receiver and lookup facts needed to choose the value-use site for a type-style associated function argument.
///
/// These calls all share the same source shape, `Type.function(arg)`, but the ownership boundary differs. Incan-owned
/// type methods expect ordinary Incan argument conversion, while inspected Rust associated functions must preserve Rust
/// API shapes such as `impl Buf`.
#[derive(Debug, Clone, Copy)]
pub struct AssociatedFunctionArgumentContext {
    pub receiver_ref_kind: Option<VarRefKind>,
    pub is_incan_owned_nominal_receiver: bool,
    pub in_return: bool,
}

/// Choose the value-use site for an ordinary method-call argument from shared receiver facts.
pub fn regular_method_argument_use_site<'a>(
    context: RegularMethodArgumentContext,
    callee_param: Option<&'a FunctionParam>,
) -> ValueUseSite<'a> {
    let target_ty = callee_param.map(|param| &param.ty);
    if context.receiver_ref_kind != Some(VarRefKind::ExternalRustName)
        && (context.has_incan_method_signature
            || (context.is_incan_owned_nominal_receiver && !context.is_rusttype_alias_receiver))
    {
        ValueUseSite::IncanCallArg {
            target_ty,
            callee_param,
            in_return: false,
        }
    } else if context.receiver_ref_kind == Some(VarRefKind::ExternalName) {
        ValueUseSite::IncanCallArg {
            target_ty,
            callee_param,
            in_return: context.in_return,
        }
    } else if matches!(context.arg_policy, MethodCallArgPolicy::PreserveShape) || context.preserves_lookup_arg_shape {
        ValueUseSite::MethodArg
    } else {
        ValueUseSite::ExternalCallArg { target_ty }
    }
}

/// Choose the value-use site for an associated function call argument from shared receiver facts.
pub fn associated_function_argument_use_site<'a>(
    context: AssociatedFunctionArgumentContext,
    callee_param: Option<&'a FunctionParam>,
) -> ValueUseSite<'a> {
    let target_ty = callee_param.map(|param| &param.ty);
    if context.receiver_ref_kind != Some(VarRefKind::ExternalRustName) && context.is_incan_owned_nominal_receiver {
        return ValueUseSite::IncanCallArg {
            target_ty,
            callee_param,
            in_return: false,
        };
    }

    if context.receiver_ref_kind == Some(VarRefKind::TypeName) {
        return ValueUseSite::IncanCallArg {
            target_ty,
            callee_param,
            in_return: context.in_return,
        };
    }

    if context.receiver_ref_kind == Some(VarRefKind::ExternalName) {
        return ValueUseSite::IncanCallArg {
            target_ty,
            callee_param,
            in_return: context.in_return,
        };
    }

    ValueUseSite::ExternalCallArg { target_ty }
}

/// Plan how one IR expression should be emitted at a specific ownership boundary.
pub fn plan_value_use(expr: &IrExpr, site: ValueUseSite<'_>) -> OwnershipPlan {
    match site {
        ValueUseSite::IncanCallArg {
            target_ty,
            callee_param,
            in_return,
        } => determine_conversion_for_incan_call(
            expr,
            target_ty,
            if in_return {
                ConversionContext::IncanFunctionArgInReturn
            } else {
                ConversionContext::IncanFunctionArg
            },
            callee_param,
        ),
        ValueUseSite::ExternalCallArg { target_ty } => {
            determine_conversion(expr, target_ty, ConversionContext::ExternalFunctionArg)
        }
        ValueUseSite::StructField { target_ty } => {
            determine_conversion(expr, target_ty, ConversionContext::StructField)
        }
        ValueUseSite::CollectionElement { target_ty } => {
            determine_conversion(expr, target_ty, ConversionContext::CollectionElement)
        }
        ValueUseSite::Assignment { target_ty } => determine_conversion(expr, target_ty, ConversionContext::Assignment),
        ValueUseSite::ReturnValue { target_ty } => {
            determine_conversion(expr, target_ty, ConversionContext::ReturnValue)
        }
        // Match scrutinees consume a value into pattern matching. Keep this as a dedicated conversion context because
        // Rust interop results can carry non-Clone values that must be moved into the match.
        ValueUseSite::MatchScrutinee { target_ty } => {
            determine_conversion(expr, target_ty, ConversionContext::MatchScrutinee)
        }
        ValueUseSite::MethodArg => determine_conversion(expr, None, ConversionContext::MethodArg),
    }
}

/// Return whether the shared value-use planner requires a backend `.clone()` at this use site.
///
/// Trait-bound inference uses this as a query-only view of the same ownership decision that expression emission uses
/// before applying a conversion. Keep clone-bound inference going through this API instead of duplicating conversion
/// heuristics in the inference pass.
#[must_use]
pub fn value_use_requires_clone_bound(expr: &IrExpr, site: ValueUseSite<'_>) -> bool {
    matches!(plan_value_use(expr, site), OwnershipPlan::Clone)
}

/// Return the target type carried by a value-use site, if the site has one.
pub fn value_use_site_target_ty<'a>(site: ValueUseSite<'a>) -> Option<&'a IrType> {
    match site {
        ValueUseSite::IncanCallArg { target_ty, .. }
        | ValueUseSite::ExternalCallArg { target_ty }
        | ValueUseSite::StructField { target_ty }
        | ValueUseSite::CollectionElement { target_ty }
        | ValueUseSite::Assignment { target_ty }
        | ValueUseSite::ReturnValue { target_ty }
        | ValueUseSite::MatchScrutinee { target_ty } => target_ty,
        ValueUseSite::MethodArg => None,
    }
}

/// Value-level coercion selected for a callable argument before the final pass-by shape is applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArgumentValuePlan {
    /// Apply the ordinary ownership/coercion conversion for this value-use site.
    Ownership(OwnershipPlan),
    /// Convert `Vec<T>` into `Vec<U>` at an external Rust call boundary.
    ExternalListElementInto,
    /// Pass an owned byte buffer to a Rust `Buf`-like parameter as a shared byte slice.
    ExternalBytesAsBufSlice,
}

impl ArgumentValuePlan {
    /// Return whether this plan applies a Rust-target-specific value adapter before final argument passing.
    fn has_external_value_adapter(&self) -> bool {
        matches!(self, Self::ExternalListElementInto | Self::ExternalBytesAsBufSlice)
    }

    /// Apply the value-level plan to an unplanned emitted argument expression.
    fn apply_full(&self, tokens: TokenStream) -> TokenStream {
        match self {
            Self::Ownership(plan) => plan.apply(tokens),
            Self::ExternalListElementInto => quote! {
                (#tokens).into_iter().map(|__incan_item| ::std::convert::Into::into(__incan_item)).collect::<Vec<_>>()
            },
            Self::ExternalBytesAsBufSlice => quote! { (#tokens).as_slice() },
        }
    }

    /// Apply only value-level work that is not already handled by [`plan_value_use`].
    fn apply_after_value_plan(&self, tokens: TokenStream) -> TokenStream {
        match self {
            Self::Ownership(_) => tokens,
            Self::ExternalListElementInto | Self::ExternalBytesAsBufSlice => self.apply_full(tokens),
        }
    }
}

/// Final Rust argument passing shape after value-level ownership/coercion has been handled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgumentPassingMode {
    /// Pass the value expression directly.
    ByValue,
    /// Pass the value expression as `&value`.
    SharedBorrow,
    /// Pass the value expression as `&mut value`.
    MutableBorrow,
}

impl ArgumentPassingMode {
    /// Apply the final argument passing shape.
    fn apply(self, tokens: TokenStream) -> TokenStream {
        match self {
            Self::ByValue => tokens,
            Self::SharedBorrow => quote! { &#tokens },
            Self::MutableBorrow => quote! { &mut #tokens },
        }
    }
}

/// Explicit argument-passing plan for a callable argument.
///
/// Argument emission is intentionally two-stage because some Incan calls need both value-level materialization and a
/// final Rust borrow shape, for example `mut s: str` lowering to `&mut "x".to_string()`. Call emitters should build one
/// of these plans, emit the argument expression, then apply the plan once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArgumentPassingPlan {
    value: ArgumentValuePlan,
    passing: ArgumentPassingMode,
}

impl ArgumentPassingPlan {
    /// Plan one argument at the given use site.
    pub fn for_use_site(expr: &IrExpr, site: ValueUseSite<'_>) -> Self {
        let mut value = match site {
            ValueUseSite::ExternalCallArg { target_ty } if external_list_arg_needs_element_into(expr, target_ty) => {
                ArgumentValuePlan::ExternalListElementInto
            }
            ValueUseSite::ExternalCallArg { target_ty } if external_buf_arg_needs_bytes_as_slice(expr, target_ty) => {
                ArgumentValuePlan::ExternalBytesAsBufSlice
            }
            _ => ArgumentValuePlan::Ownership(plan_value_use(expr, site)),
        };
        let mut passing = ArgumentPassingMode::ByValue;

        if let IrExprKind::Var { access, .. } = &expr.kind {
            match access {
                VarAccess::BorrowMut => {
                    passing = ArgumentPassingMode::MutableBorrow;
                    value = ArgumentValuePlan::Ownership(OwnershipPlan::None);
                }
                VarAccess::Borrow if value_use_site_target_ty(site).is_none() => {
                    passing = ArgumentPassingMode::SharedBorrow;
                    value = ArgumentValuePlan::Ownership(OwnershipPlan::None);
                }
                _ => {}
            }
        }

        if let ValueUseSite::IncanCallArg {
            callee_param: Some(param),
            ..
        } = site
            && incan_mutable_param_passed_as_rust_mut_ref(param)
            && !matches!(expr.ty, IrType::Ref(_) | IrType::RefMut(_))
        {
            passing = ArgumentPassingMode::MutableBorrow;
        }

        Self { value, passing }
    }

    /// Apply the complete plan to an argument that was emitted without value-use planning.
    pub fn apply_full(&self, tokens: TokenStream) -> TokenStream {
        self.passing.apply(self.value.apply_full(tokens))
    }

    /// Return whether this plan carries a Rust-target-specific value adapter.
    pub fn has_external_value_adapter(&self) -> bool {
        self.value.has_external_value_adapter()
    }

    /// Apply only the portion of the plan that remains after `emit_expr_for_use` or literal seeding already shaped the
    /// value.
    pub fn apply_after_value_plan(&self, tokens: TokenStream) -> TokenStream {
        self.passing.apply(self.value.apply_after_value_plan(tokens))
    }
}

/// Wrapper predicate for mutable aggregate Incan parameters at Rust call sites.
pub fn incan_call_arg_needs_rust_mut_borrow(param: &FunctionParam) -> bool {
    incan_mutable_param_passed_as_rust_mut_ref(param)
}

/// Return whether an external Rust list argument needs element-wise `Into` coercion.
fn external_list_arg_needs_element_into(expr: &IrExpr, target_ty: Option<&IrType>) -> bool {
    if matches!(&expr.kind, IrExprKind::List(items) if items.is_empty()) {
        return false;
    }
    let Some(IrType::List(target_elem)) = target_ty else {
        return false;
    };
    let IrType::List(source_elem) = &expr.ty else {
        return false;
    };
    source_elem != target_elem
        && !is_unresolved_call_seed_type(source_elem)
        && !is_unresolved_call_seed_type(target_elem)
}

/// Return whether an external Rust buffer argument needs `Vec<u8>`/`bytes` to become `&[u8]`.
fn external_buf_arg_needs_bytes_as_slice(expr: &IrExpr, target_ty: Option<&IrType>) -> bool {
    target_ty.is_some_and(is_rust_buf_like_target) && is_byte_buffer_type(&expr.ty) && !is_explicit_as_slice_call(expr)
}

/// Whether a target type is the Rust `Buf` shape produced by inspection for `impl Buf` parameters.
fn is_rust_buf_like_target(ty: &IrType) -> bool {
    match ty {
        IrType::Generic(name) | IrType::Struct(name) | IrType::Trait(name) | IrType::RustDisplay(name) => {
            rust_type_shape_is_buf(&rust_display_type_shape(name))
        }
        IrType::ImplTrait(bound) => rust_type_shape_is_buf(&rust_display_type_shape(&bound.trait_path)),
        _ => false,
    }
}

/// Return whether an IR type is represented as a byte buffer at Rust boundaries.
pub fn is_byte_buffer_type(ty: &IrType) -> bool {
    match ty {
        IrType::Bytes | IrType::FrozenBytes => true,
        IrType::Struct(name) | IrType::RustDisplay(name) => {
            rust_type_shape_is_byte_buffer(&rust_display_type_shape(name))
        }
        IrType::NamedGeneric(name, args)
            if matches!(name.as_str(), "Vec" | "std::vec::Vec" | "alloc::vec::Vec")
                && matches!(
                    args.as_slice(),
                    [IrType::Int | IrType::Numeric(incan_core::lang::types::numerics::NumericTypeId::U8)]
                ) =>
        {
            true
        }
        _ => false,
    }
}

/// Return whether an IR type is represented as an owned mutable Rust string buffer at Rust boundaries.
pub fn is_string_buffer_type(ty: &IrType) -> bool {
    matches!(ty, IrType::String)
        || matches!(
            ty,
            IrType::Struct(name) | IrType::RustDisplay(name)
                if matches!(name.as_str(), "String" | "std::string::String" | "alloc::string::String")
        )
}

/// Parse a Rust display type through the same shared shape parser rust-inspect uses for textual fallbacks.
fn rust_display_type_shape(name: &str) -> RustTypeShape {
    parse_rust_type_shape_text(name, |_| None, RustTypeShapePathFallback::RustPath)
}

/// Return whether a parsed Rust display type is the `Buf` trait shape used by inspected decode APIs.
fn rust_type_shape_is_buf(shape: &RustTypeShape) -> bool {
    let Some(path) = rust_type_shape_path(shape) else {
        return false;
    };
    let path = path.strip_prefix("impl ").unwrap_or(path);
    let leaf = path.rsplit("::").next().unwrap_or(path);
    leaf == "Buf" || leaf == "implBuf"
}

/// Return whether a parsed Rust display type denotes an owned byte-buffer value.
fn rust_type_shape_is_byte_buffer(shape: &RustTypeShape) -> bool {
    matches!(shape, RustTypeShape::Bytes)
}

/// Return the display path from a path-like Rust type shape.
fn rust_type_shape_path(shape: &RustTypeShape) -> Option<&str> {
    match shape {
        RustTypeShape::RustPath { path, .. } | RustTypeShape::TypeParam(path) => Some(path.as_str()),
        _ => None,
    }
}

/// Return whether the source already called `.as_slice()` for this argument.
fn is_explicit_as_slice_call(expr: &IrExpr) -> bool {
    matches!(
        &expr.kind,
        IrExprKind::MethodCall {
            method,
            args,
            ..
        } if method == "as_slice" && args.is_empty()
    )
}

/// Return whether a call-seed target still contains unresolved generic or unknown parts.
fn is_unresolved_call_seed_type(ty: &IrType) -> bool {
    match ty {
        IrType::Unknown | IrType::Generic(_) => true,
        IrType::Ref(inner) | IrType::RefMut(inner) | IrType::Option(inner) | IrType::List(inner) => {
            is_unresolved_call_seed_type(inner)
        }
        IrType::Set(inner) => is_unresolved_call_seed_type(inner),
        IrType::Dict(key, value) | IrType::Result(key, value) => {
            is_unresolved_call_seed_type(key) || is_unresolved_call_seed_type(value)
        }
        IrType::Tuple(items) => items.iter().any(is_unresolved_call_seed_type),
        IrType::NamedGeneric(_, args) => args.iter().any(is_unresolved_call_seed_type),
        IrType::Function { params, ret } => {
            params.iter().any(is_unresolved_call_seed_type) || is_unresolved_call_seed_type(ret)
        }
        IrType::Struct(_) | IrType::Enum(_) | IrType::Trait(_) => false,
        _ => false,
    }
}

/// Whether a collection receiver should be passed through, borrowed, or mutably borrowed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollectionReceiverPlan {
    /// Receiver already has the helper's expected reference/value shape.
    AsIs,
    /// Emit `&receiver`.
    BorrowShared,
    /// Emit `&mut receiver`.
    BorrowMut,
}

impl CollectionReceiverPlan {
    /// Apply the receiver plan to an already-emitted receiver token stream.
    pub fn apply(&self, tokens: TokenStream) -> TokenStream {
        match self {
            Self::AsIs => tokens,
            Self::BorrowShared => quote! { &#tokens },
            Self::BorrowMut => quote! { &mut #tokens },
        }
    }
}

/// Plan how a list-like receiver should be passed to a helper that expects a shared or mutable borrow.
pub fn plan_collection_receiver(receiver_ty: &IrType, mutable: bool) -> CollectionReceiverPlan {
    if mutable {
        match receiver_ty {
            IrType::RefMut(_) => CollectionReceiverPlan::AsIs,
            _ => CollectionReceiverPlan::BorrowMut,
        }
    } else {
        match receiver_ty {
            IrType::Ref(_) | IrType::RefMut(_) => CollectionReceiverPlan::AsIs,
            _ => CollectionReceiverPlan::BorrowShared,
        }
    }
}

/// How a dictionary lookup-style probe key should be shaped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DictLookupKeyPlan {
    /// Probe already has the lookup helper's expected shape.
    AsIs,
    /// Emit `&probe` for ordinary borrowed lookup.
    BorrowShared,
    /// Emit an `AsRef<str>` probe for owned string-key dictionaries.
    BorrowAsRefStr,
}

impl DictLookupKeyPlan {
    /// Apply the key-probe plan to an already-emitted lookup argument.
    pub fn apply(&self, tokens: TokenStream) -> TokenStream {
        match self {
            Self::AsIs => tokens,
            Self::BorrowShared => quote! { &#tokens },
            Self::BorrowAsRefStr => quote! { <_ as AsRef<str>>::as_ref(&#tokens) },
        }
    }
}

/// Plan the borrow shape for a dictionary lookup key probe.
pub fn plan_dict_lookup_key(receiver_ty: &IrType, arg_ty: &IrType) -> DictLookupKeyPlan {
    match receiver_ty {
        IrType::Dict(key_ty, _)
            if matches!(
                key_ty.as_ref(),
                IrType::String | IrType::StrRef | IrType::StaticStr | IrType::FrozenStr
            ) =>
        {
            match arg_ty {
                IrType::Ref(_) | IrType::RefMut(_) | IrType::StrRef | IrType::StaticStr => DictLookupKeyPlan::AsIs,
                _ => DictLookupKeyPlan::BorrowAsRefStr,
            }
        }
        _ => match arg_ty {
            IrType::Ref(_) | IrType::RefMut(_) | IrType::StrRef | IrType::StaticStr => DictLookupKeyPlan::AsIs,
            _ => DictLookupKeyPlan::BorrowShared,
        },
    }
}

/// How a `for` loop should traverse its iterable at the Rust level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopIterationPlan {
    /// Emit the iterable expression directly.
    AsIs,
    /// Borrow the whole iterable with `&iterable`.
    BorrowWhole,
    /// Iterate by shared reference with `.iter()`.
    Iter,
    /// Iterate by shared reference and copy scalar items with `.iter().copied()`.
    IterCopied,
    /// Iterate by shared reference and clone items with `.iter().cloned()`.
    IterCloned,
    /// Iterate by mutable reference with `.iter_mut()`.
    IterMut,
    /// Iterate a string-like value as owned one-character strings.
    StringChars,
    /// Iterate a frozen string wrapper as owned one-character strings.
    FrozenStringChars,
    /// Iterate bytes as Incan `int` values.
    BytesAsInts,
    /// Iterate a frozen bytes wrapper as Incan `int` values.
    FrozenBytesAsInts,
}

impl LoopIterationPlan {
    /// Apply the loop iteration adapter to an already-emitted iterable expression.
    pub fn apply(&self, tokens: TokenStream) -> TokenStream {
        match self {
            Self::AsIs => tokens,
            Self::BorrowWhole => quote! { &#tokens },
            Self::Iter => quote! { #tokens.iter() },
            Self::IterCopied => quote! { #tokens.iter().copied() },
            Self::IterCloned => quote! { #tokens.iter().cloned() },
            Self::IterMut => quote! { #tokens.iter_mut() },
            Self::StringChars => quote! { (#tokens).chars().map(|__incan_ch| __incan_ch.to_string()) },
            Self::FrozenStringChars => {
                quote! { (#tokens).as_str().chars().map(|__incan_ch| __incan_ch.to_string()) }
            }
            Self::BytesAsInts => quote! { (#tokens).iter().map(|__incan_byte| (*__incan_byte) as i64) },
            Self::FrozenBytesAsInts => {
                quote! { (#tokens).as_slice().iter().map(|__incan_byte| (*__incan_byte) as i64) }
            }
        }
    }
}

/// Plan the Rust iterator adapter for a lowered `for` loop.
pub fn plan_for_loop_iteration(
    iterable_ty: &IrType,
    borrowable_lvalue: bool,
    needs_mut_items: bool,
    item_is_user_enum: bool,
) -> LoopIterationPlan {
    match iterable_ty {
        IrType::RefMut(inner) => match inner.as_ref() {
            IrType::String | IrType::StaticStr | IrType::StrRef => LoopIterationPlan::StringChars,
            IrType::FrozenStr => LoopIterationPlan::FrozenStringChars,
            IrType::Bytes | IrType::StaticBytes => LoopIterationPlan::BytesAsInts,
            IrType::FrozenBytes => LoopIterationPlan::FrozenBytesAsInts,
            IrType::List(elem_ty) => match elem_ty.as_ref() {
                IrType::Int | IrType::Float | IrType::Bool => LoopIterationPlan::IterCopied,
                _ => LoopIterationPlan::IterMut,
            },
            IrType::Set(_) | IrType::Dict(_, _) => LoopIterationPlan::IterMut,
            _ => LoopIterationPlan::AsIs,
        },
        IrType::Ref(inner) => match inner.as_ref() {
            IrType::String | IrType::StaticStr | IrType::StrRef => LoopIterationPlan::StringChars,
            IrType::FrozenStr => LoopIterationPlan::FrozenStringChars,
            IrType::Bytes | IrType::StaticBytes => LoopIterationPlan::BytesAsInts,
            IrType::FrozenBytes => LoopIterationPlan::FrozenBytesAsInts,
            IrType::List(elem_ty) => match elem_ty.as_ref() {
                IrType::Int | IrType::Float | IrType::Bool => LoopIterationPlan::IterCopied,
                _ if item_is_user_enum => LoopIterationPlan::IterCloned,
                _ => LoopIterationPlan::Iter,
            },
            IrType::Set(_) | IrType::Dict(_, _) => LoopIterationPlan::Iter,
            _ => LoopIterationPlan::AsIs,
        },
        IrType::List(elem_ty) => {
            if !borrowable_lvalue {
                return LoopIterationPlan::AsIs;
            }
            match elem_ty.as_ref() {
                IrType::Int | IrType::Float | IrType::Bool => LoopIterationPlan::IterCopied,
                _ if item_is_user_enum => {
                    if needs_mut_items {
                        LoopIterationPlan::IterMut
                    } else {
                        LoopIterationPlan::IterCloned
                    }
                }
                _ => {
                    if needs_mut_items {
                        LoopIterationPlan::IterMut
                    } else {
                        LoopIterationPlan::Iter
                    }
                }
            }
        }
        IrType::String | IrType::StaticStr | IrType::StrRef => LoopIterationPlan::StringChars,
        IrType::FrozenStr => LoopIterationPlan::FrozenStringChars,
        IrType::Bytes | IrType::StaticBytes => LoopIterationPlan::BytesAsInts,
        IrType::FrozenBytes => LoopIterationPlan::FrozenBytesAsInts,
        IrType::Set(_) | IrType::Dict(_, _) => {
            if borrowable_lvalue {
                LoopIterationPlan::BorrowWhole
            } else {
                LoopIterationPlan::AsIs
            }
        }
        _ => LoopIterationPlan::AsIs,
    }
}

/// How a comprehension should traverse its input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComprehensionIterationPlan {
    /// Use a range expression directly.
    RangeDirect,
    /// Iterate a range through `.filter(...)`.
    RangeFilter,
    /// Iterate borrowed input and copy scalar/Copy values.
    IterCopied,
    /// Iterate non-range input by cloning yielded values.
    IterCloned,
    /// Filter borrowed input and copy the binding before projection.
    FilterMapCopyBinding,
    /// Filter borrowed input and clone the binding before projection.
    FilterMapCloneBinding,
}

/// Plan iteration for a list comprehension.
pub fn plan_list_comprehension_iteration(
    iterable_item_ty: Option<&IrType>,
    is_range: bool,
    filtered: bool,
) -> ComprehensionIterationPlan {
    match (
        is_range,
        filtered,
        iterable_item_ty.is_some_and(comprehension_item_can_copy_from_shared_ref),
    ) {
        (true, true, _) => ComprehensionIterationPlan::RangeFilter,
        (true, false, _) => ComprehensionIterationPlan::RangeDirect,
        (false, true, true) => ComprehensionIterationPlan::FilterMapCopyBinding,
        (false, true, false) => ComprehensionIterationPlan::FilterMapCloneBinding,
        (false, false, true) => ComprehensionIterationPlan::IterCopied,
        (false, false, false) => ComprehensionIterationPlan::IterCloned,
    }
}

/// Plan iteration for a dict comprehension.
pub fn plan_dict_comprehension_iteration(
    iterable_item_ty: Option<&IrType>,
    filtered: bool,
) -> ComprehensionIterationPlan {
    match (
        filtered,
        iterable_item_ty.is_some_and(comprehension_item_can_copy_from_shared_ref),
    ) {
        (true, true) => ComprehensionIterationPlan::FilterMapCopyBinding,
        (true, false) => ComprehensionIterationPlan::FilterMapCloneBinding,
        (false, true) => ComprehensionIterationPlan::IterCopied,
        (false, false) => ComprehensionIterationPlan::IterCloned,
    }
}

/// Return whether a comprehension item can be copied out of a shared iterator reference.
fn comprehension_item_can_copy_from_shared_ref(ty: &IrType) -> bool {
    match ty {
        IrType::RefMut(_) => false,
        IrType::Tuple(items) => items.iter().all(comprehension_item_can_copy_from_shared_ref),
        IrType::Option(inner) => comprehension_item_can_copy_from_shared_ref(inner),
        IrType::Result(ok, err) => {
            comprehension_item_can_copy_from_shared_ref(ok) && comprehension_item_can_copy_from_shared_ref(err)
        }
        _ => ty.is_copy(),
    }
}

/// Whether a dict comprehension key must be cloned before reusing it in the value expression.
pub fn dict_comprehension_key_needs_clone(key_ty: &IrType) -> bool {
    !key_ty.is_copy()
}

/// How an owned iterator adapter should materialize its collection source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnedIteratorSourcePlan {
    /// Move the source expression into the adapter.
    Move,
    /// Clone the source expression before moving it into the adapter.
    Clone,
}

impl OwnedIteratorSourcePlan {
    /// Apply the source materialization plan to an already-emitted source expression.
    pub fn apply(&self, tokens: TokenStream) -> TokenStream {
        match self {
            Self::Move => quote! { (#tokens) },
            Self::Clone => quote! { (#tokens).clone() },
        }
    }
}

/// Plan how an adapter that owns its collection source should materialize that source.
///
/// This intentionally stops short of borrowed iterator architecture. It only avoids whole-collection clones when the
/// lowered expression already proves that moving the value is safe: a last-use variable read or a one-shot expression
/// whose emitted Rust produces an owned value.
pub fn plan_owned_iterator_source(expr: &IrExpr) -> OwnedIteratorSourcePlan {
    if matches!(expr.ty, IrType::Ref(_) | IrType::RefMut(_)) {
        return OwnedIteratorSourcePlan::Clone;
    }

    if expr_can_move_into_owned_iterator(expr) {
        OwnedIteratorSourcePlan::Move
    } else {
        OwnedIteratorSourcePlan::Clone
    }
}

/// Return whether an expression can be moved into an adapter-owned iterator source.
fn expr_can_move_into_owned_iterator(expr: &IrExpr) -> bool {
    match &expr.kind {
        IrExprKind::Var { access, .. } => matches!(access, VarAccess::Move | VarAccess::Copy),
        IrExprKind::StaticRead { .. } | IrExprKind::StaticBinding { .. } | IrExprKind::AssociatedFunction { .. } => {
            false
        }
        IrExprKind::Field { .. } => false,
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::ir::expr::{IrExpr, IrExprKind, MethodCallArgPolicy, VarAccess, VarRefKind};
    use crate::backend::ir::types::Mutability;

    fn render(tokens: TokenStream) -> String {
        tokens.to_string().replace(' ', "")
    }

    #[test]
    fn incan_call_string_literal_plans_owned_string() {
        let expr = IrExpr::new(IrExprKind::String("x".to_string()), IrType::String);
        let plan = plan_value_use(
            &expr,
            ValueUseSite::IncanCallArg {
                target_ty: Some(&IrType::String),
                callee_param: None,
                in_return: false,
            },
        );
        assert_eq!(plan, OwnershipPlan::ToString);
    }

    #[test]
    fn mutable_list_param_requires_rust_mut_borrow() {
        let param = FunctionParam {
            name: "items".to_string(),
            ty: IrType::List(Box::new(IrType::Int)),
            mutability: Mutability::Mutable,
            is_self: false,
            kind: crate::frontend::ast::ParamKind::Normal,
            default: None,
        };
        assert!(incan_call_arg_needs_rust_mut_borrow(&param));
    }

    #[test]
    fn argument_plan_mutable_list_param_reborrows_without_value_clone() {
        let expr = IrExpr::new(
            IrExprKind::Var {
                name: "items".to_string(),
                access: VarAccess::Read,
                ref_kind: VarRefKind::Value,
            },
            IrType::List(Box::new(IrType::Int)),
        );
        let param = FunctionParam {
            name: "items".to_string(),
            ty: IrType::List(Box::new(IrType::Int)),
            mutability: Mutability::Mutable,
            is_self: false,
            kind: crate::frontend::ast::ParamKind::Normal,
            default: None,
        };
        let plan = ArgumentPassingPlan::for_use_site(
            &expr,
            ValueUseSite::IncanCallArg {
                target_ty: Some(&param.ty),
                callee_param: Some(&param),
                in_return: false,
            },
        );
        assert_eq!(render(plan.apply_after_value_plan(quote! { items })), "&mutitems");
    }

    #[test]
    fn argument_plan_mutable_string_literal_materializes_then_reborrows() {
        let expr = IrExpr::new(IrExprKind::String("x".to_string()), IrType::String);
        let param = FunctionParam {
            name: "s".to_string(),
            ty: IrType::String,
            mutability: Mutability::Mutable,
            is_self: false,
            kind: crate::frontend::ast::ParamKind::Normal,
            default: None,
        };
        let plan = ArgumentPassingPlan::for_use_site(
            &expr,
            ValueUseSite::IncanCallArg {
                target_ty: Some(&param.ty),
                callee_param: Some(&param),
                in_return: false,
            },
        );
        assert_eq!(render(plan.apply_full(quote! { "x" })), "&mut\"x\".to_string()");
    }

    #[test]
    fn argument_plan_external_ref_param_borrows_once() {
        let expr = IrExpr::new(
            IrExprKind::Var {
                name: "thing".to_string(),
                access: VarAccess::Read,
                ref_kind: VarRefKind::Value,
            },
            IrType::Struct("demo::Thing".to_string()),
        );
        let target = IrType::Ref(Box::new(IrType::Struct("demo::Thing".to_string())));
        let plan = ArgumentPassingPlan::for_use_site(
            &expr,
            ValueUseSite::ExternalCallArg {
                target_ty: Some(&target),
            },
        );
        assert_eq!(render(plan.apply_full(quote! { thing })), "&thing");
        assert_eq!(render(plan.apply_after_value_plan(quote! { &thing })), "&thing");
    }

    #[test]
    fn argument_plan_external_list_element_into_is_value_plan() {
        let expr = IrExpr::new(
            IrExprKind::Var {
                name: "items".to_string(),
                access: VarAccess::Move,
                ref_kind: VarRefKind::Value,
            },
            IrType::List(Box::new(IrType::String)),
        );
        let target = IrType::List(Box::new(IrType::Struct("demo::Name".to_string())));
        let plan = ArgumentPassingPlan::for_use_site(
            &expr,
            ValueUseSite::ExternalCallArg {
                target_ty: Some(&target),
            },
        );
        let rendered = render(plan.apply_full(quote! { items }));
        assert!(rendered.contains("items).into_iter().map"));
        assert!(rendered.contains("Into::into(__incan_item)"));
    }

    #[test]
    fn argument_plan_external_list_element_into_skips_unresolved_source_elements() {
        let expr = IrExpr::new(IrExprKind::List(Vec::new()), IrType::List(Box::new(IrType::Unknown)));
        let target = IrType::List(Box::new(IrType::Struct("demo::Name".to_string())));
        let plan = ArgumentPassingPlan::for_use_site(
            &expr,
            ValueUseSite::ExternalCallArg {
                target_ty: Some(&target),
            },
        );
        let rendered = render(plan.apply_full(quote! { vec![] }));
        assert!(!rendered.contains("into_iter"));
        assert!(!rendered.contains("Into::into"));
    }

    #[test]
    fn argument_plan_external_list_element_into_skips_empty_list_literals() {
        let expr = IrExpr::new(IrExprKind::List(Vec::new()), IrType::List(Box::new(IrType::String)));
        let target = IrType::List(Box::new(IrType::Struct("demo::Name".to_string())));
        let plan = ArgumentPassingPlan::for_use_site(
            &expr,
            ValueUseSite::ExternalCallArg {
                target_ty: Some(&target),
            },
        );
        let rendered = render(plan.apply_full(quote! { vec![] }));
        assert!(!rendered.contains("into_iter"));
        assert!(!rendered.contains("Into::into"));
    }

    #[test]
    fn argument_plan_external_buf_param_lends_incan_bytes_as_slice() {
        let expr = IrExpr::new(
            IrExprKind::Var {
                name: "encoded".to_string(),
                access: VarAccess::Read,
                ref_kind: VarRefKind::Value,
            },
            IrType::Bytes,
        );
        let target = IrType::Generic("Buf".to_string());
        let plan = ArgumentPassingPlan::for_use_site(
            &expr,
            ValueUseSite::ExternalCallArg {
                target_ty: Some(&target),
            },
        );
        assert_eq!(render(plan.apply_full(quote! { encoded })), "(encoded).as_slice()");
    }

    #[test]
    fn argument_plan_external_buf_param_lends_rust_vec_u8_as_slice() {
        let expr = IrExpr::new(
            IrExprKind::Var {
                name: "encoded".to_string(),
                access: VarAccess::Read,
                ref_kind: VarRefKind::Value,
            },
            IrType::Struct("alloc::vec::Vec<u8>".to_string()),
        );
        let target = IrType::Generic("implBuf".to_string());
        let plan = ArgumentPassingPlan::for_use_site(
            &expr,
            ValueUseSite::ExternalCallArg {
                target_ty: Some(&target),
            },
        );
        assert_eq!(render(plan.apply_full(quote! { encoded })), "(encoded).as_slice()");
    }

    #[test]
    fn argument_plan_external_buf_param_lends_generic_rust_vec_u8_as_slice() {
        let expr = IrExpr::new(
            IrExprKind::Var {
                name: "encoded".to_string(),
                access: VarAccess::Read,
                ref_kind: VarRefKind::Value,
            },
            IrType::NamedGeneric(
                "Vec".to_string(),
                vec![IrType::Numeric(incan_core::lang::types::numerics::NumericTypeId::U8)],
            ),
        );
        let target = IrType::Generic("implBuf".to_string());
        let plan = ArgumentPassingPlan::for_use_site(
            &expr,
            ValueUseSite::ExternalCallArg {
                target_ty: Some(&target),
            },
        );
        assert_eq!(render(plan.apply_full(quote! { encoded })), "(encoded).as_slice()");
    }

    #[test]
    fn rust_display_byte_buffer_detection_uses_shared_type_shape_parser() {
        assert!(is_byte_buffer_type(&IrType::RustDisplay("Vec < u8 >".to_string())));
        assert!(is_byte_buffer_type(&IrType::RustDisplay(
            "std::vec::Vec<u8>".to_string()
        )));
        assert!(is_byte_buffer_type(&IrType::RustDisplay(
            "alloc::vec::Vec<u8>".to_string()
        )));
        assert!(!is_byte_buffer_type(&IrType::RustDisplay(
            "std::io::Cursor<Vec<u8>>".to_string()
        )));
    }

    #[test]
    fn rust_display_buf_detection_uses_shared_type_shape_parser() {
        assert!(is_rust_buf_like_target(&IrType::RustDisplay("prost::Buf".to_string())));
        assert!(is_rust_buf_like_target(&IrType::RustDisplay(
            "impl prost::Buf".to_string()
        )));
        assert!(!is_rust_buf_like_target(&IrType::RustDisplay(
            "demo::Buffer".to_string()
        )));
    }

    #[test]
    fn rust_string_buffer_detection_is_owned_string_only() {
        assert!(is_string_buffer_type(&IrType::String));
        assert!(is_string_buffer_type(&IrType::RustDisplay(
            "std::string::String".to_string()
        )));
        assert!(is_string_buffer_type(&IrType::Struct(
            "alloc::string::String".to_string()
        )));
        assert!(!is_string_buffer_type(&IrType::StrRef));
        assert!(!is_string_buffer_type(&IrType::FrozenStr));
    }

    #[test]
    fn argument_plan_external_buf_param_keeps_explicit_as_slice_shape() {
        let expr = IrExpr::new(
            IrExprKind::MethodCall {
                receiver: Box::new(IrExpr::new(
                    IrExprKind::Var {
                        name: "encoded".to_string(),
                        access: VarAccess::Read,
                        ref_kind: VarRefKind::Value,
                    },
                    IrType::Struct("alloc::vec::Vec<u8>".to_string()),
                )),
                method: "as_slice".to_string(),
                dispatch: None,
                type_args: Vec::new(),
                args: Vec::new(),
                callable_signature: None,
                arg_policy: MethodCallArgPolicy::Default,
            },
            IrType::Bytes,
        );
        let target = IrType::Generic("Buf".to_string());
        let plan = ArgumentPassingPlan::for_use_site(
            &expr,
            ValueUseSite::ExternalCallArg {
                target_ty: Some(&target),
            },
        );
        assert_eq!(
            render(plan.apply_full(quote! { encoded.as_slice() })),
            "encoded.as_slice()"
        );
    }

    #[test]
    fn argument_plan_external_buf_param_keeps_non_byte_buf_implementors_by_value() {
        let expr = IrExpr::new(
            IrExprKind::Var {
                name: "cursor".to_string(),
                access: VarAccess::Move,
                ref_kind: VarRefKind::Value,
            },
            IrType::Struct("std::io::Cursor<Vec<u8>>".to_string()),
        );
        let target = IrType::Generic("Buf".to_string());
        let plan = ArgumentPassingPlan::for_use_site(
            &expr,
            ValueUseSite::ExternalCallArg {
                target_ty: Some(&target),
            },
        );
        assert_eq!(render(plan.apply_full(quote! { cursor })), "cursor");
    }

    #[test]
    fn argument_plan_clone_bound_query_follows_shared_incan_arg_policy() {
        let receiver = IrExpr::new(
            IrExprKind::Var {
                name: "other".to_string(),
                access: VarAccess::Read,
                ref_kind: VarRefKind::Value,
            },
            IrType::Struct("Wrapper".to_string()),
        );
        let expr = IrExpr::new(
            IrExprKind::Field {
                object: Box::new(receiver),
                field: "_cursor".to_string(),
            },
            IrType::Generic("T".to_string()),
        );

        assert!(value_use_requires_clone_bound(
            &expr,
            ValueUseSite::IncanCallArg {
                target_ty: Some(&IrType::Generic("T".to_string())),
                callee_param: None,
                in_return: false,
            }
        ));
        assert!(!value_use_requires_clone_bound(&expr, ValueUseSite::MethodArg));
    }

    #[test]
    fn list_shared_receiver_borrows_plain_list() {
        assert_eq!(
            plan_collection_receiver(&IrType::List(Box::new(IrType::Int)), false),
            CollectionReceiverPlan::BorrowShared
        );
    }

    #[test]
    fn dict_string_lookup_uses_as_ref_for_owned_probe() {
        assert_eq!(
            plan_dict_lookup_key(
                &IrType::Dict(Box::new(IrType::String), Box::new(IrType::Int)),
                &IrType::String
            ),
            DictLookupKeyPlan::BorrowAsRefStr
        );
    }

    #[test]
    fn for_loop_on_borrowable_string_list_uses_iter() {
        assert_eq!(
            plan_for_loop_iteration(&IrType::List(Box::new(IrType::String)), true, false, false),
            LoopIterationPlan::Iter
        );
    }

    #[test]
    fn for_loop_on_string_iterates_owned_character_strings() {
        assert_eq!(
            plan_for_loop_iteration(&IrType::String, true, false, false),
            LoopIterationPlan::StringChars
        );
    }

    #[test]
    fn for_loop_on_bytes_iterates_incan_ints() {
        assert_eq!(
            plan_for_loop_iteration(&IrType::Bytes, true, false, false),
            LoopIterationPlan::BytesAsInts
        );
    }

    #[test]
    fn for_loop_on_user_enum_list_clones_items_when_not_mutating() {
        assert_eq!(
            plan_for_loop_iteration(
                &IrType::List(Box::new(IrType::Enum("Node".to_string()))),
                true,
                false,
                true
            ),
            LoopIterationPlan::IterCloned
        );
    }

    #[test]
    fn filtered_list_comprehension_uses_filter_map_clone_plan() {
        assert_eq!(
            plan_list_comprehension_iteration(Some(&IrType::String), false, true),
            ComprehensionIterationPlan::FilterMapCloneBinding
        );
    }

    #[test]
    fn copy_list_comprehension_uses_copy_iteration_plans() {
        assert_eq!(
            plan_list_comprehension_iteration(Some(&IrType::Int), false, false),
            ComprehensionIterationPlan::IterCopied
        );
        assert_eq!(
            plan_dict_comprehension_iteration(Some(&IrType::Int), true),
            ComprehensionIterationPlan::FilterMapCopyBinding
        );
    }

    #[test]
    fn mutable_ref_comprehension_item_does_not_use_copied_plan() {
        assert_eq!(
            plan_list_comprehension_iteration(Some(&IrType::RefMut(Box::new(IrType::Int))), false, false),
            ComprehensionIterationPlan::IterCloned
        );
    }

    #[test]
    fn dict_comprehension_marks_noncopy_keys_for_clone() {
        assert!(dict_comprehension_key_needs_clone(&IrType::String));
        assert!(!dict_comprehension_key_needs_clone(&IrType::Int));
    }

    #[test]
    fn owned_iterator_source_moves_last_use_list_var() {
        let expr = IrExpr::new(
            IrExprKind::Var {
                name: "items".to_string(),
                access: VarAccess::Move,
                ref_kind: VarRefKind::Value,
            },
            IrType::List(Box::new(IrType::Int)),
        );

        assert_eq!(plan_owned_iterator_source(&expr), OwnedIteratorSourcePlan::Move);
    }

    #[test]
    fn owned_iterator_source_clones_reused_list_var() {
        let expr = IrExpr::new(
            IrExprKind::Var {
                name: "items".to_string(),
                access: VarAccess::Read,
                ref_kind: VarRefKind::Value,
            },
            IrType::List(Box::new(IrType::Int)),
        );

        assert_eq!(plan_owned_iterator_source(&expr), OwnedIteratorSourcePlan::Clone);
    }

    #[test]
    fn owned_iterator_source_moves_one_shot_list_expression() {
        let expr = IrExpr::new(IrExprKind::List(Vec::new()), IrType::List(Box::new(IrType::Int)));

        assert_eq!(plan_owned_iterator_source(&expr), OwnedIteratorSourcePlan::Move);
    }

    #[test]
    fn external_call_string_var_borrows() {
        let expr = IrExpr::new(
            IrExprKind::Var {
                name: "name".to_string(),
                access: VarAccess::Move,
                ref_kind: VarRefKind::Value,
            },
            IrType::String,
        );
        let plan = plan_value_use(&expr, ValueUseSite::ExternalCallArg { target_ty: None });
        assert_eq!(plan, OwnershipPlan::Borrow);
    }

    #[test]
    fn incan_call_field_backed_noncopy_arg_clones_for_by_value_methods_issue241() {
        let receiver = IrExpr::new(
            IrExprKind::Var {
                name: "other".to_string(),
                access: VarAccess::Read,
                ref_kind: VarRefKind::Value,
            },
            IrType::Struct("Wrapper".to_string()),
        );
        let expr = IrExpr::new(
            IrExprKind::Field {
                object: Box::new(receiver),
                field: "_cursor".to_string(),
            },
            IrType::Struct("Cursor".to_string()),
        );

        let plan = plan_value_use(
            &expr,
            ValueUseSite::IncanCallArg {
                target_ty: Some(&IrType::Struct("Cursor".to_string())),
                callee_param: None,
                in_return: false,
            },
        );
        assert_eq!(plan, OwnershipPlan::Clone);
    }

    #[test]
    fn return_value_noncopy_self_read_clones_to_materialize_owned_result() {
        let expr = IrExpr::new(
            IrExprKind::Var {
                name: "self".to_string(),
                access: VarAccess::Read,
                ref_kind: VarRefKind::Value,
            },
            IrType::SelfType,
        );

        let plan = plan_value_use(
            &expr,
            ValueUseSite::ReturnValue {
                target_ty: Some(&IrType::SelfType),
            },
        );
        assert_eq!(plan, OwnershipPlan::Clone);
    }
}
