//! Check indexing, slicing, field access, and method calls.
//!
//! These helpers validate access patterns like `xs[i]`, `xs[a:b]`, `obj.field`, and `obj.method(...)`, emitting
//! diagnostics for missing fields/methods and incompatible uses.

use std::collections::HashMap;
use std::sync::Arc;

use crate::ast::*;
use crate::diagnostics::errors::SelfMutation;
use crate::diagnostics::errors::TypeArgumentOrigin;
use crate::diagnostics::{CompileError, errors};
use crate::resolved_type_subst::{map_resolved_type_leaves, substitute_resolved_type, type_param_subst_map};
use crate::symbols::*;
use crate::typechecker::derive_requirements::DeriveSupport;
use crate::typechecker::helpers::{
    collection_name, collection_type_id, generator_ty, is_frozen_bytes, is_frozen_str, is_intlike_for_index,
    is_str_like, list_ty, option_ty, render_resolved_type_as_rust_arg, runtime_string_method_identity_and_return,
    string_method_identity_and_return,
};
use crate::typechecker::receiver_change::ReceiverChange;
use crate::typechecker::type_info::{
    CBindingEnumAccess, ResolvedMethodCall, ResolvedOperatorKind, RustCheckedFieldValue, RustMethodTraitImportUse,
    RustTraitImportInfo,
};
use crate::typechecker::{IdentKind, MemberBindingSurface, canonical_public_library_type_name};
use incan_lang::interop::{
    RustCollectionFamily, RustFieldInfo, RustFunctionSig, RustItemKind, RustItemMetadata, RustTraitAssoc,
    RustTraitInfo, RustTypeInfo, RustVisibility,
};
use incan_lang::lang::builtins::BuiltinMethodId;
use incan_lang::lang::derives::DeriveId;
use incan_lang::lang::magic_methods;
use incan_lang::lang::surface::collection_helpers::{self, BuiltinCollectionHelperId};
use incan_lang::lang::surface::constructors::{self, ConstructorId};
use incan_lang::lang::surface::iterator_methods::IteratorMethodId;
use incan_lang::lang::surface::result_methods::ResultMethodId;
use incan_lang::lang::surface::string_methods::{self, SelectedStringMethodArgumentKind, StringMethodId};
use incan_lang::lang::surface::types as surface_types;
use incan_lang::lang::surface::types::{SEMAPHORE_ACQUIRE_ERROR_TYPE_NAME, SEMAPHORE_PERMIT_TYPE_NAME, SurfaceTypeId};
use incan_lang::lang::surface::{
    bytes_methods, dict_methods, float_methods, frozen_bytes_methods, frozen_dict_methods, frozen_list_methods,
    frozen_set_methods, iterator_methods, list_methods, result_methods, set_methods,
};
use incan_lang::lang::text_codecs::{self, DecodeErrorsPolicy};
use incan_lang::lang::trait_bounds;
use incan_lang::lang::traits::{self as core_traits, TraitId};
use incan_lang::lang::types::collections::CollectionTypeId;
use incan_lang::lang::types::numerics::{
    self as numerics, IntegerHelperFamily, IntegerHelperOperation, NumericFamily, NumericResizeMethodId,
};
use incan_lang::lang::{conventions, stdlib};
use incan_lang::lang::{enum_helpers, surface::option_methods};
use incan_semantics_core::body_ir::HelperOp;
use incan_semantics_core::{CanonicalSymbolId, SemanticSourceTargetKind, SymbolNamespace, SymbolOrigin};
use quote::ToTokens;
use syn::{GenericArgument, PathArguments, ReturnType, Type as SynType, TypeParamBound};

use super::calls::PublicModuleConstructorContext;

use super::{GenericPartialTarget, TypeChecker};

mod builtin_helper_calls;
mod builtin_receiver_methods;
mod copy_and_self;
mod declared_edges;
mod field_access;
mod generated_members;
mod indexing;
mod iterator_calls;
mod member_access;
mod method_call_targets;
mod method_calls;
mod method_resolution;
mod native_methods;
mod receiver_method_fallbacks;
mod result_combinators;
mod rust_callables;
mod rust_deref;
mod rust_methods;
mod rust_projections;
mod rust_receivers;
mod rust_traits;
mod web_extractors;

/// Whether an unresolved registered surface can defer its method set to a runtime or stdlib provider.
///
/// Validation errors have a compiler-known constructor and data surface, so an absent method is a source error. The
/// other categories retain their existing open method policy until their provider declarations carry complete facts.
fn surface_accepts_unknown_methods(id: SurfaceTypeId) -> bool {
    surface_types::category(id) != surface_types::SurfaceTypeCategory::Validation
}

/// The method the stdlib `Hash` trait declares, which no type meeting a `Hash` bound provides (#1822).
const HASH_DUNDER: &str = "__hash__";

/// The hook `obj[start:end:step]` calls on a user type, which `Sliceable[T]` declares.
const SLICE_HOOK: &str = "__getslice__";

/// One `Enum.Variant(...)` construction resolved against the destination type it is being built for.
///
/// Both halves are kept together because they are needed at different points of the same method-call pass: the
/// substituted payload types must exist before the arguments are checked, since that is the only time they are
/// checked, while the enum's own arguments are what the construction's result type reports afterwards.
#[derive(Debug, Clone)]
struct EnumVariantInstantiation {
    /// The enum's type arguments, supplied explicitly or taken from a same-declaration destination.
    arguments: Vec<ResolvedType>,
    /// The variant's declared payload types, substituted through those arguments.
    payloads: Vec<ResolvedType>,
}

#[derive(Debug, Clone)]
struct MethodCandidate {
    info: MethodInfo,
    dispatch: Option<crate::typechecker::ResolvedMethodDispatch>,
}

/// Source method lookup context; shared borrowed receivers exclude mutable declarations before arguments are checked.
#[derive(Clone, Copy)]
struct SourceMethodPrepass<'a> {
    method: &'a str,
    receiver_surface: MemberBindingSurface,
    type_args: &'a [Spanned<Type>],
    args: &'a [CallArg],
    span: Span,
    receiver_ty: &'a ResolvedType,
    expected_return_ty: Option<&'a ResolvedType>,
    shared_receiver: bool,
}

/// Borrowed source inputs shared by the ordered stages of one method-call check.
///
/// This carries no inferred facts: receiver and argument types are checked once by their owning stages.
struct MethodCallSite<'a> {
    base: &'a Spanned<Expr>,
    method: &'a str,
    type_args: &'a [Spanned<Type>],
    args: &'a [CallArg],
    span: Span,
    expected_return_ty: Option<&'a ResolvedType>,
}

struct ValueEnumGeneratedCall<'a> {
    enum_name: &'a str,
    value_enum: &'a ValueEnumInfo,
    method: &'a str,
    base_is_type_name: bool,
    type_args: &'a [Spanned<Type>],
    args: &'a [CallArg],
    arg_types: &'a [ResolvedType],
    span: Span,
}

struct RustTraitMethodCall<'a> {
    rust_path: &'a str,
    method: &'a str,
    sig: &'a RustFunctionSig,
    type_args: &'a [Spanned<Type>],
    args: &'a [CallArg],
    arg_types: &'a [ResolvedType],
    preserves_lookup_arg_shape: bool,
    span: Span,
}

/// A trait-qualified call `Trait.method(receiver, args...)` on an imported Rust trait with inspected metadata.
struct RustTraitQualifiedCall<'a> {
    rust_path: &'a str,
    trait_info: &'a RustTraitInfo,
    method: &'a str,
    type_args: &'a [Spanned<Type>],
    args: &'a [CallArg],
    arg_types: &'a [ResolvedType],
    expected_return_ty: Option<&'a ResolvedType>,
    span: Span,
}

/// The checked receiver, written arguments and expected result supplied to native method resolution.
#[derive(Clone, Copy)]
struct RustPathMethodCall<'a> {
    rust_path: &'a str,
    method: &'a str,
    receiver_metadata: Option<&'a Arc<RustItemMetadata>>,
    /// The import binding the receiver expression names, when the receiver is a bare imported Rust item such as
    /// the `Mac` in `Mac.update(handle, data)`; `None` for value receivers.
    receiver_binding: Option<&'a str>,
    receiver_expr: &'a Spanned<Expr>,
    type_args: &'a [Spanned<Type>],
    args: &'a [CallArg],
    arg_types: &'a [ResolvedType],
    receiver_span: Span,
    expected_return_ty: Option<&'a ResolvedType>,
    span: Span,
}

#[derive(Debug, Clone)]
struct RustCallableAliasParam {
    rust_display: String,
    resolved_ty: ResolvedType,
}

#[derive(Debug, Clone)]
struct RustCallableAliasSignature {
    params: Vec<RustCallableAliasParam>,
    return_ty: ResolvedType,
}

/// Context selected once before a method's arguments are checked in source order.
///
/// Native callable bounds may defer closure typing, while enum payloads and Result combinators supply expectations.
/// Constructor facts remain available after argument checking to determine the call's result and retained signature.
struct MethodArgumentContext {
    contextual_rust_callable: Option<RustCallableAliasSignature>,
    result_callback: Option<(ResolvedType, ResolvedType)>,
    rust_receiver_path: Option<String>,
    enum_variant_construction: Option<EnumVariantInstantiation>,
    enum_variant_parameters: Option<Vec<CallableParam>>,
}

/// Canonical provenance and the closed operation selected for one compiler-owned method.
struct CompilerBuiltinMethodSelection {
    identity: CanonicalSymbolId,
    operation: Option<BuiltinMethodId>,
}

/// Diagnostic label for a Rust path receiver in type errors (`rust::{path}`).
fn rust_receiver_display(path: &str) -> String {
    format!("rust::{path}")
}

impl TypeChecker {
    /// Record `settled`, the type a `match` or `loop:` value's open `Result` sides were settled to, as the type of the
    /// value and, with its open parts filled from it, of each of its arm and `break` values, at any depth (#1561).
    ///
    /// Source collection and intrinsic constructor literals also retain their settled parts at descendant spans.
    /// Lowering spells each value's recorded type on the `Ok(...)` or `Err(...)` it builds, so the arms of `r = match
    /// c:` that all build `Ok(...)` build the same `Result[int, None]` the binding has. Any other value records
    /// nothing unless it is such a literal.
    pub(in crate::typechecker) fn record_settled_branch_values(
        &mut self,
        value: &Spanned<Expr>,
        settled: &ResolvedType,
    ) {
        self.retain_literal_carrier_types(value, settled);
        match &value.node {
            Expr::Paren(inner) => self.record_settled_branch_values(inner, settled),
            Expr::Match(_, _) | Expr::Loop(_) => {
                self.record_expr_type(value.span, settled.clone());
                for branch in branch_values(value) {
                    let Some(mut filled) = self.type_info.expr_type(branch.span).cloned() else {
                        continue;
                    };
                    let recorded = filled.clone();
                    super::fill_open_result_parts(&mut filled, settled, false);
                    if filled != recorded {
                        self.record_expr_type(branch.span, filled.clone());
                        self.record_settled_branch_values(branch, &filled);
                    }
                }
            }
            _ => {}
        }
    }

    /// Resolve a builtin method through its receiver and method registries, preserving owner discrimination.
    fn compiler_builtin_method_selection(
        base_ty: &ResolvedType,
        method: &str,
    ) -> Option<CompilerBuiltinMethodSelection> {
        if let Some(selection) = Self::compiler_builtin_collection_method_selection(base_ty, method) {
            return Some(selection);
        }
        let (owner, member, operation) = match base_ty {
            ResolvedType::Int | ResolvedType::Float | ResolvedType::Numeric(_) | ResolvedType::Bool => {
                let owner_id = super::super::numeric_type_id_for_compat(base_ty)?;
                let owner = incan_lang::lang::types::numerics::as_str(owner_id);
                if let Some(resize) = numerics::resize_method_from_str(method) {
                    return Some(CompilerBuiltinMethodSelection {
                        identity: Self::compiler_builtin_member_identity(owner, numerics::resize_method_as_str(resize)),
                        operation: Some(BuiltinMethodId::NumericResize(owner_id, resize)),
                    });
                }
                if matches!(
                    incan_lang::lang::types::numerics::info_for(owner_id).family,
                    NumericFamily::BinaryFloat
                ) {
                    let method = float_methods::from_str(method)?;
                    (
                        owner,
                        float_methods::as_str(method),
                        Some(BuiltinMethodId::Float(owner_id, method)),
                    )
                } else {
                    let helper = numerics::integer_helper_from_str(method)?;
                    if !numerics::supports_integer_helper(owner_id, helper) {
                        return None;
                    }
                    (
                        owner,
                        helper.canonical,
                        Some(BuiltinMethodId::IntegerHelper(
                            owner_id,
                            helper.family,
                            helper.operation,
                        )),
                    )
                }
            }
            ResolvedType::Str => (
                incan_lang::lang::types::stringlike::as_str(incan_lang::lang::types::stringlike::StringLikeId::Str),
                string_methods::from_str(method).map(string_methods::as_str)?,
                string_methods::from_str(method).map(BuiltinMethodId::Str),
            ),
            ResolvedType::FrozenStr => {
                let method = string_methods::from_str(method)?;
                (
                    incan_lang::lang::types::stringlike::as_str(
                        incan_lang::lang::types::stringlike::StringLikeId::FrozenStr,
                    ),
                    string_methods::as_str(method),
                    Some(BuiltinMethodId::FrozenStr(method)),
                )
            }
            ResolvedType::Bytes => {
                let method = bytes_methods::from_str(method)?;
                (
                    incan_lang::lang::types::stringlike::as_str(
                        incan_lang::lang::types::stringlike::StringLikeId::Bytes,
                    ),
                    bytes_methods::as_str(method),
                    Some(BuiltinMethodId::Bytes(method)),
                )
            }
            ResolvedType::FrozenBytes => {
                let method = frozen_bytes_methods::from_str(method).or_else(|| {
                    bytes_methods::from_str(method).and_then(|method| {
                        (method == bytes_methods::BytesMethodId::Decode)
                            .then_some(frozen_bytes_methods::FrozenBytesMethodId::Decode)
                    })
                })?;
                (
                    incan_lang::lang::types::stringlike::as_str(
                        incan_lang::lang::types::stringlike::StringLikeId::FrozenBytes,
                    ),
                    frozen_bytes_methods::as_str(method),
                    Some(BuiltinMethodId::FrozenBytes(method)),
                )
            }
            _ => return None,
        };
        Some(CompilerBuiltinMethodSelection {
            identity: Self::compiler_builtin_member_identity(owner, member),
            operation,
        })
    }

    /// Resolve collection and iterator methods while retaining the receiver family in the selected operation.
    fn compiler_builtin_collection_method_selection(
        base_ty: &ResolvedType,
        method: &str,
    ) -> Option<CompilerBuiltinMethodSelection> {
        let (owner, member, operation) = match base_ty {
            ResolvedType::FrozenList(_) => {
                let method = frozen_list_methods::from_str(method)?;
                (
                    incan_lang::lang::types::collections::as_str(CollectionTypeId::FrozenList),
                    frozen_list_methods::as_str(method),
                    Some(BuiltinMethodId::FrozenList(method)),
                )
            }
            ResolvedType::FrozenSet(_) => {
                let method = frozen_set_methods::from_str(method)?;
                (
                    incan_lang::lang::types::collections::as_str(CollectionTypeId::FrozenSet),
                    frozen_set_methods::as_str(method),
                    Some(BuiltinMethodId::FrozenSet(method)),
                )
            }
            ResolvedType::FrozenDict(_, _) => {
                let method = frozen_dict_methods::from_str(method)?;
                (
                    incan_lang::lang::types::collections::as_str(CollectionTypeId::FrozenDict),
                    frozen_dict_methods::as_str(method),
                    Some(BuiltinMethodId::FrozenDict(method)),
                )
            }
            // An `Iterator[T]` value, such as an adapter's result or a list's `iter()`, has the RFC 088 surface as its
            // own members, as a `Generator[T]` does.
            ResolvedType::Generic(name, _) if Self::is_iterator_protocol_name(name) => (
                Self::iterator_protocol_name(),
                iterator_methods::from_str(method).map(iterator_methods::as_str)?,
                iterator_methods::from_str(method).map(BuiltinMethodId::Iterator),
            ),
            ResolvedType::Generic(name, _) => match collection_type_id(name) {
                Some(CollectionTypeId::List) => match list_methods::from_str(method) {
                    Some(method) => (
                        incan_lang::lang::types::collections::as_str(CollectionTypeId::List),
                        list_methods::as_str(method),
                        Some(BuiltinMethodId::List(method)),
                    ),
                    None => (
                        incan_lang::lang::types::collections::as_str(CollectionTypeId::List),
                        Self::iterable_iter_member(method)?,
                        Some(BuiltinMethodId::ListIter),
                    ),
                },
                Some(CollectionTypeId::Dict) => {
                    let method = dict_methods::from_str(method)?;
                    (
                        incan_lang::lang::types::collections::as_str(CollectionTypeId::Dict),
                        dict_methods::as_str(method),
                        Some(BuiltinMethodId::Dict(method)),
                    )
                }
                Some(CollectionTypeId::Set) => {
                    let method = set_methods::from_str(method)?;
                    (
                        incan_lang::lang::types::collections::as_str(CollectionTypeId::Set),
                        set_methods::as_str(method),
                        Some(BuiltinMethodId::Set(method)),
                    )
                }
                Some(CollectionTypeId::Option) => {
                    let method = option_methods::from_str(method)?;
                    (
                        incan_lang::lang::types::collections::as_str(CollectionTypeId::Option),
                        option_methods::as_str(method),
                        Some(BuiltinMethodId::Option(method)),
                    )
                }
                Some(CollectionTypeId::Result) => {
                    let method = result_methods::from_str(method)?;
                    (
                        incan_lang::lang::types::collections::as_str(CollectionTypeId::Result),
                        result_methods::as_str(method),
                        Some(BuiltinMethodId::Result(method)),
                    )
                }
                Some(CollectionTypeId::Generator) => {
                    let method = iterator_methods::from_str(method)?;
                    (
                        incan_lang::lang::types::collections::as_str(CollectionTypeId::Generator),
                        iterator_methods::as_str(method),
                        Some(BuiltinMethodId::Generator(method)),
                    )
                }
                _ => return None,
            },
            _ => return None,
        };
        Some(CompilerBuiltinMethodSelection {
            identity: Self::compiler_builtin_member_identity(owner, member),
            operation,
        })
    }

    /// Return canonical provenance for a compiler-owned method after the registry selected it.
    fn compiler_builtin_method_identity(base_ty: &ResolvedType, method: &str) -> Option<CanonicalSymbolId> {
        Self::compiler_builtin_method_selection(base_ty, method).map(|selection| selection.identity)
    }

    /// Typecheck built-in numeric resize helpers and retain their concrete target for downstream call lowering.
    fn check_numeric_resize_method(
        &mut self,
        base_ty: &ResolvedType,
        method: &str,
        type_args: &[Spanned<Type>],
        args: &[CallArg],
        span: Span,
        expected_return_ty: Option<&ResolvedType>,
    ) -> Option<ResolvedType> {
        let source = super::super::numeric_type_id_for_compat(base_ty)?;
        let policy = numerics::resize_method_from_str(method)?;
        let explicit_target = match type_args {
            [] => None,
            [target] if !matches!(policy, NumericResizeMethodId::Lossless) => Some(self.resolve_type_checked(target)),
            [_] => {
                self.errors.push(errors::type_mismatch(
                    "no explicit target on resize()",
                    "explicit resize target",
                    span,
                ));
                return Some(ResolvedType::Unknown);
            }
            _ => {
                self.errors.push(errors::type_mismatch(
                    "at most one explicit resize target",
                    "multiple type arguments",
                    span,
                ));
                return Some(ResolvedType::Unknown);
            }
        };
        if !args.is_empty() {
            self.errors
                .push(errors::type_mismatch("no arguments", "arguments", span));
            return Some(ResolvedType::Unknown);
        }

        let contextual_target = match policy {
            NumericResizeMethodId::Try => match expected_return_ty {
                Some(ResolvedType::Generic(name, args))
                    if collection_type_id(name.as_str()) == Some(CollectionTypeId::Option) && args.len() == 1 =>
                {
                    Some(args[0].clone())
                }
                Some(found) if explicit_target.is_none() => {
                    self.errors.push(errors::type_mismatch(
                        "contextual Option[numeric] target",
                        &found.to_string(),
                        span,
                    ));
                    return Some(ResolvedType::Unknown);
                }
                _ => None,
            },
            _ => match expected_return_ty {
                Some(ty) => Some(ty.clone()),
                None if explicit_target.is_none() => {
                    self.errors.push(errors::type_mismatch(
                        "contextual numeric target",
                        "unknown target",
                        span,
                    ));
                    return Some(ResolvedType::Unknown);
                }
                None => None,
            },
        };
        let target_ty = explicit_target
            .or_else(|| contextual_target.clone())
            .unwrap_or(ResolvedType::Unknown);
        if let Some(contextual_target) = contextual_target
            && super::super::numeric_type_id_for_compat(&target_ty)
                != super::super::numeric_type_id_for_compat(&contextual_target)
        {
            let contextual_result = match policy {
                NumericResizeMethodId::Try => option_ty(contextual_target),
                _ => contextual_target,
            };
            self.errors.push(errors::type_mismatch(
                &format!("explicit resize target compatible with {contextual_result}"),
                &target_ty.to_string(),
                span,
            ));
            return Some(ResolvedType::Unknown);
        }
        let Some(target) = super::super::numeric_type_id_for_compat(&target_ty) else {
            self.errors
                .push(errors::type_mismatch("numeric target", &target_ty.to_string(), span));
            return Some(ResolvedType::Unknown);
        };

        let monomorph_target = target_ty.clone();
        let Some(result) = self.checked_numeric_resize_result(policy, source, target, target_ty, span) else {
            return Some(ResolvedType::Unknown);
        };
        self.type_info
            .calls
            .call_site_monomorph_type_args
            .insert(span, vec![monomorph_target]);
        Some(result)
    }

    /// Validate a selected resize policy before its concrete target can be retained for lowering.
    ///
    /// Failure records the existing numeric diagnostic and grants no monomorphized target. Try-resize wraps the valid
    /// target in Option; the other policies retain the written or inferred target type.
    fn checked_numeric_resize_result(
        &mut self,
        policy: NumericResizeMethodId,
        source: numerics::NumericTypeId,
        target: numerics::NumericTypeId,
        target_ty: ResolvedType,
        span: Span,
    ) -> Option<ResolvedType> {
        let source_info = numerics::info_for(source);
        let target_info = numerics::info_for(target);
        let integer_to_integer = matches!(
            (source_info.family, target_info.family),
            (
                NumericFamily::SignedInteger | NumericFamily::UnsignedInteger,
                NumericFamily::SignedInteger | NumericFamily::UnsignedInteger
            )
        );
        match policy {
            NumericResizeMethodId::Lossless => {
                if !incan_lang::numeric_values::numeric_type_losslessly_widens_to(source, target) {
                    self.errors.push(
                        errors::type_mismatch("lossless numeric resize target", &target_ty.to_string(), span)
                            .with_hint(
                                "Use try_resize(), wrapping_resize(), or saturating_resize() for explicit lossy integer resizing.",
                            ),
                    );
                    return None;
                }
                Some(target_ty)
            }
            NumericResizeMethodId::Try | NumericResizeMethodId::Wrapping | NumericResizeMethodId::Saturating => {
                if !integer_to_integer {
                    self.errors.push(errors::type_mismatch(
                        "integer resize target",
                        &target_ty.to_string(),
                        span,
                    ));
                    return None;
                }
                match policy {
                    NumericResizeMethodId::Try => Some(option_ty(target_ty)),
                    _ => Some(target_ty),
                }
            }
        }
    }

    /// Typecheck RFC 009 integer overflow helpers from the language-owned numeric registry.
    fn check_integer_overflow_helper(
        &mut self,
        base_ty: &ResolvedType,
        method: &str,
        type_args: &[Spanned<Type>],
        args: &[CallArg],
        arg_types: &[ResolvedType],
        span: Span,
    ) -> Option<ResolvedType> {
        let helper = numerics::integer_helper_from_str(method)?;
        let source = super::super::numeric_type_id_for_compat(base_ty)?;
        if !numerics::supports_integer_helper(source, helper) {
            return None;
        }
        if !type_args.is_empty() {
            self.errors
                .push(errors::type_mismatch("no type arguments", "type arguments", span));
            return Some(ResolvedType::Unknown);
        }
        if args.len() != 1 || arg_types.len() != 1 {
            self.errors
                .push(errors::builtin_arity(helper.canonical, 1, args.len(), span));
            return Some(ResolvedType::Unknown);
        }

        let expected_operand = match helper.operation {
            IntegerHelperOperation::Add | IntegerHelperOperation::Sub | IntegerHelperOperation::Mul => source,
            IntegerHelperOperation::Pow => numerics::NumericTypeId::U32,
        };
        if super::super::numeric_type_id_for_compat(&arg_types[0]) != Some(expected_operand) {
            self.errors.push(errors::type_mismatch(
                numerics::as_str(expected_operand),
                &arg_types[0].to_string(),
                span,
            ));
            return Some(ResolvedType::Unknown);
        }

        match helper.family {
            IntegerHelperFamily::Checked => Some(option_ty(base_ty.clone())),
            IntegerHelperFamily::Wrapping | IntegerHelperFamily::Saturating => Some(base_ty.clone()),
        }
    }

    /// Return the type a builtin receiver's method takes its argument at `index` as, where the method stores the
    /// argument in the receiver, looks it up among the receiver's elements or keys, or returns it as the receiver's
    /// payload (#1859).
    ///
    /// `list[T].append(x)` stores `x` as a `T` and `extend` takes a `list[T]`, `dict[K, V].insert(k, v)` and
    /// `set[T].add(x)` store their arguments as the key, value and element types, `list[T].contains(x)`, `count(x)`
    /// and `index(x)`, `set[T].contains(x)`, and `dict[K, V].get(k)` and `contains_key(k)` look `x` or `k` up as an
    /// element or a key (`get`'s default is a `V`), `Option[T].unwrap_or(x)` and `Result[T, E].unwrap_or(x)` return
    /// `x` as the `T`, and `powf` on a `float` or `f32` takes an exponent of the receiver's own type. The argument is
    /// checked against that type, so an integer literal there takes the float type a float slot expects, as it does in
    /// a declaration, and a numeric argument of a narrower type is widened to it (RFC 009). An `Option` of a reference
    /// (the `Option[&V]` of `dict.get`) and every other method give no expectation.
    fn builtin_method_argument_expectation(base_ty: &ResolvedType, method: &str, index: usize) -> Option<ResolvedType> {
        use incan_lang::lang::types::numerics::NumericTypeId;
        let base_ty = match base_ty {
            ResolvedType::Ref(inner) | ResolvedType::RefMut(inner) => inner.as_ref(),
            other => other,
        };
        if let Some(source) = super::super::numeric_type_id_for_compat(base_ty)
            && let Some(helper) = numerics::integer_helper_from_str(method)
            && index == 0
            && numerics::supports_integer_helper(source, helper)
        {
            return Some(match helper.operation {
                IntegerHelperOperation::Add | IntegerHelperOperation::Sub | IntegerHelperOperation::Mul => {
                    base_ty.clone()
                }
                IntegerHelperOperation::Pow => ResolvedType::Numeric(numerics::NumericTypeId::U32),
            });
        }
        match base_ty {
            ResolvedType::Float | ResolvedType::Numeric(NumericTypeId::F32) => (index == 0
                && float_methods::from_str(method) == Some(float_methods::FloatMethodId::Powf))
            .then(|| base_ty.clone()),
            ResolvedType::Generic(name, args) => match (collection_type_id(name.as_str()), args.as_slice()) {
                (Some(CollectionTypeId::List), [element]) => match (list_methods::from_str(method)?, index) {
                    (list_methods::ListMethodId::Append, 0) => Some(element.clone()),
                    (list_methods::ListMethodId::Extend, 0) => Some(list_ty(element.clone())),
                    (
                        list_methods::ListMethodId::Contains
                        | list_methods::ListMethodId::Count
                        | list_methods::ListMethodId::Index,
                        0,
                    ) => Some(element.clone()),
                    _ => None,
                },
                (Some(CollectionTypeId::Dict), [key, value]) => match (dict_methods::from_str(method)?, index) {
                    (
                        dict_methods::DictMethodId::Insert
                        | dict_methods::DictMethodId::Get
                        | dict_methods::DictMethodId::ContainsKey,
                        0,
                    ) => Some(key.clone()),
                    (dict_methods::DictMethodId::Insert | dict_methods::DictMethodId::Get, 1) => Some(value.clone()),
                    _ => None,
                },
                (Some(CollectionTypeId::Set), [element]) => (index == 0
                    && matches!(
                        set_methods::from_str(method),
                        Some(set_methods::SetMethodId::Add | set_methods::SetMethodId::Contains)
                    ))
                .then(|| element.clone()),
                (Some(CollectionTypeId::Option), [payload]) => (index == 0
                    && option_methods::from_str(method) == Some(option_methods::OptionMethodId::UnwrapOr)
                    && !matches!(payload, ResolvedType::Ref(_) | ResolvedType::RefMut(_)))
                .then(|| payload.clone()),
                (Some(CollectionTypeId::Result), [ok, _]) => (index == 0
                    && result_methods::from_str(method) == Some(ResultMethodId::UnwrapOr))
                .then(|| ok.clone()),
                _ => None,
            },
            _ => None,
        }
    }

    /// Try a final native numeric member and diagnose missing methods only after external/generic fallback declined.
    fn check_missing_method(
        &mut self,
        call: &MethodCallSite<'_>,
        base_ty: &ResolvedType,
        arg_types: &[ResolvedType],
    ) -> ResolvedType {
        if let Some(numeric) = super::super::numeric_type_id_for_compat(base_ty) {
            let errors_before = self.errors.len();
            if let Some(result) = self.resolve_rust_path_method_call(RustPathMethodCall {
                rust_path: numerics::as_str(numeric),
                method: call.method,
                receiver_metadata: None,
                receiver_binding: None,
                receiver_expr: call.base,
                type_args: call.type_args,
                args: call.args,
                arg_types,
                receiver_span: call.base.span,
                expected_return_ty: call.expected_return_ty,
                span: call.span,
            }) && (self.errors.len() != errors_before || self.type_info.rust.checked_calls.contains_key(&call.span))
            {
                return result;
            }
        }
        let display = base_ty.to_string();
        let surface = surface_types::from_str(display.as_str());
        let open_runtime = surface.is_some_and(surface_accepts_unknown_methods);
        let unresolved_external = matches!(
            base_ty,
            ResolvedType::Named(name) if self.symbols.lookup(name).is_none() && surface.is_none()
        );
        if !(unresolved_external || open_runtime) {
            self.errors
                .push(errors::missing_method(&base_ty.to_string(), call.method, call.span));
        }
        ResolvedType::Unknown
    }
}

/// Return the values a `match` or `loop:` expression can produce: the expression body of each arm, and each value a
/// `break` of the loop itself gives, in source order (#1561).
///
/// A block arm produces no value, and a `break` inside a nested `loop:`, `while` or `for` leaves that inner loop, so
/// neither is one of them. Any other expression has none.
fn branch_values(value: &Spanned<Expr>) -> Vec<&Spanned<Expr>> {
    match &value.node {
        Expr::Match(_, arms) => arms
            .iter()
            .filter_map(|arm| match &arm.node.body {
                MatchBody::Expr(body) => Some(body),
                MatchBody::Block(_) => None,
            })
            .collect(),
        Expr::Loop(loop_expr) => {
            let mut values = Vec::new();
            collect_break_values(&loop_expr.body, &mut values);
            values
        }
        _ => Vec::new(),
    }
}

/// Collect the values the `break` statements of one loop body give, entering `if` branches, `unsafe` blocks and the
/// block arms of a `match` statement, but no nested loop, whose `break` statements leave that loop.
fn collect_break_values<'a>(body: &'a [Spanned<Statement>], values: &mut Vec<&'a Spanned<Expr>>) {
    for statement in body {
        match &statement.node {
            Statement::Break(Some(value)) => values.push(value),
            Statement::If(if_stmt) => {
                collect_break_values(&if_stmt.then_body, values);
                for (_, branch) in &if_stmt.elif_branches {
                    collect_break_values(branch, values);
                }
                if let Some(else_body) = &if_stmt.else_body {
                    collect_break_values(else_body, values);
                }
            }
            Statement::Unsafe(unsafe_stmt) => collect_break_values(&unsafe_stmt.body, values),
            Statement::Expr(expr) => {
                if let Expr::Match(_, arms) = &expr.node {
                    for arm in arms {
                        if let MatchBody::Block(block) = &arm.node.body {
                            collect_break_values(block, values);
                        }
                    }
                }
            }
            _ => {}
        }
    }
}
