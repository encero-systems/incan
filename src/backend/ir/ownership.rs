//! Centralized ownership and coercion planning for IR emission.
//!
//! This module is the backend's dedicated decision layer for "duckborrowing":
//! given a typed IR expression and a Rust sink/source boundary, decide whether emission should move, clone, borrow, or
//! materialize an owned string.
//!
//! Keep emitter modules calling this planner instead of open-coding ad hoc `.clone()`, `&`, `&mut`, `.to_string()`, or
//! `.into()` decisions.

use proc_macro2::TokenStream;
use quote::quote;

use super::conversions::{
    Conversion as OwnershipPlan, ConversionContext, determine_conversion, determine_conversion_for_incan_call,
    incan_mutable_param_passed_as_rust_mut_ref,
};
use super::decl::FunctionParam;
use super::expr::IrExpr;
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
        // Match scrutinees consume a value into pattern matching. Reuse owned-result materialization semantics so
        // borrowed/shared non-Copy values are cloned before the match.
        ValueUseSite::MatchScrutinee { target_ty } => {
            determine_conversion(expr, target_ty, ConversionContext::ReturnValue)
        }
        ValueUseSite::MethodArg => determine_conversion(expr, None, ConversionContext::MethodArg),
    }
}

/// Wrapper predicate for mutable aggregate Incan parameters at Rust call sites.
pub fn incan_call_arg_needs_rust_mut_borrow(param: &FunctionParam) -> bool {
    incan_mutable_param_passed_as_rust_mut_ref(param)
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
            IrType::List(elem_ty) => match elem_ty.as_ref() {
                IrType::Int | IrType::Float | IrType::Bool => LoopIterationPlan::IterCopied,
                _ => LoopIterationPlan::IterMut,
            },
            IrType::Set(_) | IrType::Dict(_, _) => LoopIterationPlan::IterMut,
            _ => LoopIterationPlan::AsIs,
        },
        IrType::Ref(inner) => match inner.as_ref() {
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
    /// Iterate non-range input by cloning yielded values.
    IterCloned,
    /// Filter borrowed input and clone the binding before projection.
    FilterMapCloneBinding,
}

/// Plan iteration for a list comprehension.
pub fn plan_list_comprehension_iteration(is_range: bool, filtered: bool) -> ComprehensionIterationPlan {
    match (is_range, filtered) {
        (true, true) => ComprehensionIterationPlan::RangeFilter,
        (true, false) => ComprehensionIterationPlan::RangeDirect,
        (false, true) => ComprehensionIterationPlan::FilterMapCloneBinding,
        (false, false) => ComprehensionIterationPlan::IterCloned,
    }
}

/// Plan iteration for a dict comprehension.
pub fn plan_dict_comprehension_iteration(filtered: bool) -> ComprehensionIterationPlan {
    if filtered {
        ComprehensionIterationPlan::FilterMapCloneBinding
    } else {
        ComprehensionIterationPlan::IterCloned
    }
}

/// Whether a dict comprehension key must be cloned before reusing it in the value expression.
pub fn dict_comprehension_key_needs_clone(key_ty: &IrType) -> bool {
    !key_ty.is_copy()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::ir::expr::{IrExpr, IrExprKind, VarAccess, VarRefKind};
    use crate::backend::ir::types::Mutability;

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
            default: None,
        };
        assert!(incan_call_arg_needs_rust_mut_borrow(&param));
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
            plan_list_comprehension_iteration(false, true),
            ComprehensionIterationPlan::FilterMapCloneBinding
        );
    }

    #[test]
    fn dict_comprehension_marks_noncopy_keys_for_clone() {
        assert!(dict_comprehension_key_needs_clone(&IrType::String));
        assert!(!dict_comprehension_key_needs_clone(&IrType::Int));
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
