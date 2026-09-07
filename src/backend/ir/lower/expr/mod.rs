//! Expression lowering for AST to IR conversion.
//!
//! This module handles lowering of all expression types: literals, identifiers, binary/unary operations, function
//! calls, method calls, comprehensions, etc.
//!
//! Large helpers (calls, patterns, comprehensions, pow helpers) are split into submodules; all methods live on `impl
//! AstLowering`.

mod calls;
mod comprehensions;
mod helpers;
mod patterns;

use std::collections::HashMap;

use super::super::decl::{FunctionParamDefault, IrTraitBound, IrTraitBoundOrigin, IrTypeParam};
use super::super::expr::{
    BuiltinFn, CollectionMethodKind, IrCallArg, IrCallArgKind, IrDictEntry, IrExpr, IrExprKind, IrListEntry,
    IrMethodDispatch, IrTraitDispatch, Literal as IrLiteral, MethodCallArgPolicy, MethodKind, NumericResizePolicy,
    RaceArm, UnaryOp, VarAccess, VarRefKind,
};
use super::super::types::IrType;
use super::super::{IrCheckedCFunction, IrCheckedCType, IrStmt, IrStmtKind, Mutability, TypedExpr};
use super::AstLowering;
use super::errors::LoweringError;
use crate::frontend::ast::{self, Spanned};
use crate::frontend::library_manifest_index::LibraryManifestIndexEntry;
use crate::frontend::partial_projection::{PartialPresetRef, merge_named_partial_args};
use crate::frontend::symbols::{ImplementationTraitBoundOriginInfo, ImplementationTypeParamInfo, ResolvedType};
use crate::frontend::typechecker::{
    CAbiSpanAccessKind, IdentKind, PartialProjectionTargetKind, ResolvedMethodDispatch, ResolvedOperatorKind,
    RustArgCoercionKind,
};
use incan_core::interop::RustCollectionFamily;
use incan_core::lang::builtins::BuiltinFnId;
use incan_core::lang::magic_methods::{self, MagicMethodId};
use incan_core::lang::surface::collection_helpers::{self, BuiltinCollectionHelperId};
use incan_core::lang::surface::result_methods::ResultMethodId;
use incan_core::lang::surface::types::{self as surface_types, SurfaceTypeId, TASK_JOIN_ERROR_TYPE_NAME};
use incan_core::lang::traits::{self as builtin_traits, TraitId};
use incan_core::lang::types::collections::{self as collection_types, CollectionTypeId};
use incan_core::lang::{stdlib, trait_bounds};
use incan_semantics_core::SurfaceExprLoweringAction;

/// Return the trait's declaration name from however the call site's module spelled it.
///
/// A trait imported directly is spelled `Serialize`; the same declaration reached through its owning module is
/// spelled `json.Serialize`. Registries keyed on the declaration name must see one string for both.
fn trait_declaration_name(dispatch: &IrTraitDispatch) -> &str {
    let name = dispatch.trait_source_name.as_str();
    name.rsplit('.').next().unwrap_or(name)
}

/// Whether a call receiver has a statically nameable source-owned method projection.
///
/// Bare generic and trait-object receivers must keep trait dispatch because no inherent owner is statically nameable.
/// Nominal generics can use their owner's projection, while Rust-native traits keep their stable Rust ABI slot even
/// when the source declaration has a recoverable Incan identity beside it.
fn can_use_source_method_projection(receiver: &TypedExpr, dispatch: Option<&IrMethodDispatch>) -> bool {
    let mut receiver_ty = &receiver.ty;
    while let IrType::Ref(inner) | IrType::RefMut(inner) = receiver_ty {
        receiver_ty = inner.as_ref();
    }
    let dispatch_uses_rust_native_trait_slot = matches!(
        dispatch,
        Some(IrMethodDispatch::Trait(trait_dispatch))
            if trait_bounds::incan_to_rust(trait_declaration_name(trait_dispatch)).is_some()
    );

    !dispatch_uses_rust_native_trait_slot
        && (matches!(
            receiver_ty,
            IrType::Struct(_) | IrType::Enum(_) | IrType::NamedGeneric(_, _) | IrType::SelfType
        ) || (!matches!(receiver_ty, IrType::Generic(_) | IrType::Trait(_) | IrType::Unknown)
            && matches!(
                &receiver.kind,
                IrExprKind::Var {
                    ref_kind: VarRefKind::TypeName,
                    ..
                }
            )))
}

impl AstLowering {
    /// Select the physical method target while retaining any checked trait evidence needed after lowering.
    pub(super) fn project_resolved_method_target(
        &self,
        call_span: ast::Span,
        source_method: &str,
        receiver: &TypedExpr,
        dispatch: Option<IrMethodDispatch>,
    ) -> (String, Option<IrMethodDispatch>) {
        if !can_use_source_method_projection(receiver, dispatch.as_ref())
            || self.method_belongs_to_an_imported_type(call_span)
            || !self.receiver_adopts_the_dispatched_trait(receiver, dispatch.as_ref())
        {
            return (source_method.to_string(), dispatch);
        }
        let rebase_source_stdlib = !matches!(dispatch, Some(IrMethodDispatch::Trait(_)));
        let Some(projection) = self
            .compiled_provider_method_reference_name(call_span, &receiver.ty, source_method)
            .or_else(|| self.emitted_method_reference_name(call_span, source_method, rebase_source_stdlib))
        else {
            return (source_method.to_string(), dispatch);
        };
        let dispatch = match dispatch {
            Some(IrMethodDispatch::Trait(trait_dispatch)) => Some(IrMethodDispatch::SourceProjection(trait_dispatch)),
            other => other,
        };
        (projection, dispatch)
    }

    /// Whether a trait-dispatched call reaches a trait the receiver's own type adopts.
    ///
    /// A recoverable projection names a wrapper emitted beside a declaration, and a local type gets one for each
    /// trait it adopts. A trait method the type never adopted has no such slot: `Score.default()` resolves to
    /// `std.derives.copying.default`, but `model Score with Serialize` adopts only `Serialize`, so the only wrapper
    /// on `Score` is the one for `to_json`. Projecting the other named a slot that was never emitted, and the
    /// generated Rust did not compile.
    ///
    /// Only trait dispatch is decided here. Everything else -- inherent methods, provider references, and the
    /// non-dispatched cases -- is already settled by the checks beside this one, so a receiver whose type is not a
    /// locally declared nominal type keeps whatever those decided.
    fn receiver_adopts_the_dispatched_trait(&self, receiver: &TypedExpr, dispatch: Option<&IrMethodDispatch>) -> bool {
        let Some(IrMethodDispatch::Trait(trait_dispatch)) = dispatch else {
            return true;
        };
        let mut receiver_ty = &receiver.ty;
        while let IrType::Ref(inner) | IrType::RefMut(inner) = receiver_ty {
            receiver_ty = inner.as_ref();
        }
        let type_name = match receiver_ty {
            IrType::Struct(name) | IrType::Enum(name) | IrType::NamedGeneric(name, _) => name.as_str(),
            _ => return true,
        };
        // A receiver typed as the dispatched trait itself names no implementation. The trait's own declaration is an
        // abstract slot with no wrapper beside it -- only the types adopting it emit one -- so projecting a call on
        // `data: DataSet[T]` named the trait's declaration span while every wrapper carries an implementation's.
        if type_name == trait_declaration_name(trait_dispatch) || self.declared_trait_names.contains(type_name) {
            return false;
        }
        let Some(adopted) = self.adopted_traits_by_type.get(type_name) else {
            return true;
        };
        adopted.contains(trait_declaration_name(trait_dispatch))
    }

    /// Whether this call reaches an inherent method declared by a package rather than by this compilation.
    ///
    /// A recoverable projection is a wrapper emitted beside a declaration, and only the compilation that declares the
    /// type emits one. A package's inherent method therefore has no wrapper this compilation can name -- and when the
    /// package's type is itself a newtype over a Rust type, as `std.async.sync.MutexGuard` is over
    /// `incan_stdlib`'s `MutexGuard`, no wrapper exists at all, because Rust forbids an inherent `impl` on a foreign
    /// type. Those methods are facades over the Rust ones and the call has to reach the Rust slot.
    ///
    /// Trait dispatch is decided here too. It used to be excluded, on the reasoning that a local type adopting a
    /// package's trait has its wrapper emitted locally; but `can_use_source_method_projection` never asks which
    /// library declares the method, so a package trait method reached through a package type -- `BytesIO(..).read(..)`
    /// dispatching through `BinaryRead` -- passed every check and projected a name only the provider emits. Declining
    /// the projection keeps the fully-qualified trait call, which also preserves the type arguments that pin an
    /// otherwise-inferred width.
    ///
    /// A package origin names *which library declares the method*, not that the method is foreign to this build. When
    /// that library is the one this compilation is producing, the wrapper is emitted right here, and suppressing the
    /// projection would decline to name a slot that does exist. Only another library's method has no wrapper this
    /// compilation can name.
    fn method_belongs_to_an_imported_type(&self, call_span: ast::Span) -> bool {
        let Some(identity) = self
            .type_info
            .as_ref()
            .and_then(|info| info.resolved_identity(call_span))
        else {
            return false;
        };
        let incan_semantics_core::SymbolOrigin::Package { library, .. } = &identity.origin else {
            return false;
        };
        self.produced_library_identity() != Some(library.as_str())
    }

    /// Convert a contained checked-C value contract into its private generated-Rust carrier.
    fn checked_c_value_ir_type(binding: &str, ty: &IrCheckedCType) -> IrType {
        match ty {
            IrCheckedCType::Scalar(scalar) => incan_core::lang::c_abi::scalar_numeric_type(*scalar)
                .map(IrType::Numeric)
                .unwrap_or(IrType::Int),
            IrCheckedCType::Pointer { mutable, pointee } => Self::checked_c_pointer_ir_type(*mutable, pointee),
            IrCheckedCType::Resource { resource, .. } => {
                IrType::Struct(IrCheckedCFunction::resource_rust_type_name(binding, resource))
            }
            IrCheckedCType::Nullable(value) => IrType::Option(Box::new(Self::checked_c_value_ir_type(binding, value))),
            IrCheckedCType::Void => IrType::Unit,
            IrCheckedCType::Output { .. } => IrType::Unknown,
        }
    }

    /// Convert the bounded pointer subset into its exact private Rust carrier.
    fn checked_c_pointer_ir_type(mutable: bool, pointee: &IrCheckedCType) -> IrType {
        let pointee = match pointee {
            IrCheckedCType::Scalar(scalar) => match scalar {
                incan_core::lang::c_abi::ScalarTypeId::I8 => "i8",
                incan_core::lang::c_abi::ScalarTypeId::U8 => "u8",
                incan_core::lang::c_abi::ScalarTypeId::I16 => "i16",
                incan_core::lang::c_abi::ScalarTypeId::U16 => "u16",
                incan_core::lang::c_abi::ScalarTypeId::I32 => "i32",
                incan_core::lang::c_abi::ScalarTypeId::U32 => "u32",
                incan_core::lang::c_abi::ScalarTypeId::I64 => "i64",
                incan_core::lang::c_abi::ScalarTypeId::U64 => "u64",
                incan_core::lang::c_abi::ScalarTypeId::I128 => "i128",
                incan_core::lang::c_abi::ScalarTypeId::U128 => "u128",
                incan_core::lang::c_abi::ScalarTypeId::F32 => "f32",
                incan_core::lang::c_abi::ScalarTypeId::F64 => "f64",
                incan_core::lang::c_abi::ScalarTypeId::Size => "usize",
                incan_core::lang::c_abi::ScalarTypeId::CChar => "::std::os::raw::c_char",
                incan_core::lang::c_abi::ScalarTypeId::CInt => "::std::os::raw::c_int",
            },
            _ => return IrType::Unknown,
        };
        let qualifier = if mutable { "mut" } else { "const" };
        IrType::RustDisplay(format!("*{qualifier} {pointee}"))
    }

    /// Convert one checked-C parameter to its call-site carrier, preserving compiler-managed output storage.
    fn checked_c_parameter_ir_type(function: &IrCheckedCFunction, index: usize, parameter: &IrCheckedCType) -> IrType {
        match parameter {
            IrCheckedCType::Output { .. } => IrType::RefMut(Box::new(IrType::Struct(
                IrCheckedCFunction::output_slot_rust_type_name(
                    &function.binding,
                    &function.symbol,
                    function.parameter_names.get(index).map(String::as_str).unwrap_or("arg"),
                ),
            ))),
            _ => Self::checked_c_value_ir_type(&function.binding, parameter),
        }
    }

    /// Keep the IR ownership operation aligned with the source-checked C parameter mode.
    fn order_checked_c_call_args(function: &IrCheckedCFunction, args: Vec<IrCallArg>) -> Vec<IrCallArg> {
        if !args.iter().any(|argument| argument.name.is_some()) {
            return args;
        }

        let mut positional = args.iter().filter(|argument| argument.name.is_none()).cloned();
        function
            .parameter_names
            .iter()
            .filter_map(|parameter_name| {
                args.iter()
                    .find(|argument| argument.name.as_deref() == Some(parameter_name))
                    .cloned()
                    .or_else(|| positional.next())
            })
            .collect()
    }

    /// Apply the binding's ownership and output-storage access modes to each lowered raw-call argument.
    fn apply_checked_c_argument_accesses(function: &IrCheckedCFunction, args: &mut [IrCallArg]) {
        for (index, argument) in args.iter_mut().enumerate() {
            let Some(parameter) = function.parameters.get(index) else {
                continue;
            };
            let access = match parameter {
                IrCheckedCType::Resource { access, .. } => Some(*access),
                IrCheckedCType::Output { .. } => {
                    Self::set_checked_c_argument_access(&mut argument.expr, VarAccess::BorrowMut);
                    None
                }
                _ => None,
            };
            let Some(access) = access else {
                continue;
            };
            let variable_access = match access {
                crate::frontend::typechecker::CResourceAccess::Owned => VarAccess::Move,
                crate::frontend::typechecker::CResourceAccess::Borrowed => VarAccess::CAbiBorrow,
                crate::frontend::typechecker::CResourceAccess::BorrowedMut => VarAccess::BorrowMut,
            };
            Self::set_checked_c_argument_access(&mut argument.expr, variable_access);
        }
    }

    /// Set an ownership access through the narrow coercion wrappers used by ordinary expression lowering.
    fn set_checked_c_argument_access(expr: &mut TypedExpr, access: VarAccess) {
        match &mut expr.kind {
            IrExprKind::Var { access: current, .. } => *current = access,
            IrExprKind::InteropCoerce { expr, .. } => Self::set_checked_c_argument_access(expr, access),
            _ => {}
        }
    }

    /// Lower one source `slot.take()` as a consuming read from compiler-managed checked-C output storage.
    fn lower_checked_c_output_slot_take(
        &mut self,
        call_span: ast::Span,
        receiver: &ast::Spanned<ast::Expr>,
        method: &str,
        type_args: &[ast::Spanned<ast::Type>],
        args: &[ast::CallArg],
    ) -> Result<Option<TypedExpr>, LoweringError> {
        if method != "take" || !type_args.is_empty() || !args.is_empty() {
            return Ok(None);
        }
        let Some(ResolvedType::Named(identity)) = self
            .type_info
            .as_ref()
            .and_then(|info| info.expr_type(receiver.span))
            .cloned()
        else {
            return Ok(None);
        };
        let Some((binding, symbol, parameter)) = incan_core::lang::c_abi::parse_output_slot_type_identity(&identity)
        else {
            return Ok(None);
        };
        let slot = self
            .type_info
            .as_ref()
            .and_then(|info| info.c_abi.output_slots.iter().find(|slot| slot.identity == identity))
            .cloned()
            .ok_or_else(|| LoweringError {
                message: format!(
                    "checked C output slot `{binding}.{symbol}.{parameter}` is absent from lowering facts"
                ),
                span: call_span.into(),
            })?;
        let value = Self::checked_c_ir_type(&slot.value).ok_or_else(|| LoweringError {
            message: format!("checked C output slot `{binding}.{symbol}.{parameter}` has an unsupported value carrier"),
            span: call_span.into(),
        })?;
        let return_type = Self::checked_c_value_ir_type(binding, &value);
        let mut receiver = self.lower_expr_spanned(receiver)?;
        Self::set_checked_c_argument_access(&mut receiver, VarAccess::Move);
        let slot_type = IrCheckedCFunction::output_slot_rust_type_name(binding, symbol, parameter);
        Ok(Some(TypedExpr::new(
            IrExprKind::Call {
                func: Box::new(TypedExpr::new(
                    IrExprKind::AssociatedFunction {
                        type_name: slot_type,
                        function_name: "take".to_string(),
                    },
                    IrType::Unknown,
                )),
                type_args: Vec::new(),
                args: vec![IrCallArg {
                    name: None,
                    kind: IrCallArgKind::Positional,
                    expr: receiver,
                }],
                callable_signature: None,
                canonical_path: None,
            },
            return_type,
        )))
    }

    /// Lower the sole approved raw extraction from a checked C string temporary.
    fn lower_checked_c_string_pointer(
        &mut self,
        receiver: &ast::Spanned<ast::Expr>,
        method: &str,
        type_args: &[ast::Spanned<ast::Type>],
        args: &[ast::CallArg],
    ) -> Result<Option<TypedExpr>, LoweringError> {
        if method != "as_const_ptr" || !type_args.is_empty() || !args.is_empty() {
            return Ok(None);
        }
        let is_checked_c_string = self
            .type_info
            .as_ref()
            .and_then(|info| info.expr_type(receiver.span))
            .is_some_and(|ty| matches!(ty, ResolvedType::Named(identity) if identity == incan_core::lang::c_abi::C_STRING_TYPE_ID));
        if !is_checked_c_string {
            return Ok(None);
        }
        Ok(Some(TypedExpr::new(
            IrExprKind::MethodCall {
                receiver: Box::new(self.lower_expr_spanned(receiver)?),
                method: "as_ptr".to_string(),
                dispatch: None,
                type_args: Vec::new(),
                args: Vec::new(),
                callable_signature: None,
                arg_policy: MethodCallArgPolicy::Default,
            },
            Self::checked_c_pointer_ir_type(
                false,
                &IrCheckedCType::Scalar(incan_core::lang::c_abi::ScalarTypeId::CChar),
            ),
        )))
    }

    /// Lower closed typed-span bridge methods to direct owned-vector operations and bounded finish helpers.
    fn lower_checked_c_span_method(
        &mut self,
        call_span: ast::Span,
        receiver: &ast::Spanned<ast::Expr>,
        method: &str,
        type_args: &[ast::Spanned<ast::Type>],
        args: &[ast::CallArg],
    ) -> Result<Option<TypedExpr>, LoweringError> {
        if !type_args.is_empty() {
            return Ok(None);
        }
        let Some(span_access) = self
            .type_info
            .as_ref()
            .and_then(|info| info.c_abi.span_accesses.iter().find(|access| access.span == call_span))
            .copied()
        else {
            return Ok(None);
        };
        let pointer = match (span_access.access, method) {
            (CAbiSpanAccessKind::ConstPointer, "as_const_ptr") if args.is_empty() => Some(("as_ptr", false)),
            (CAbiSpanAccessKind::MutPointer, "as_mut_ptr") if args.is_empty() => Some(("as_mut_ptr", true)),
            _ => None,
        };
        if let Some((rust_method, mutable_pointer)) = pointer {
            return Ok(Some(TypedExpr::new(
                IrExprKind::MethodCall {
                    receiver: Box::new(self.lower_expr_spanned(receiver)?),
                    method: rust_method.to_string(),
                    dispatch: None,
                    type_args: Vec::new(),
                    args: Vec::new(),
                    callable_signature: None,
                    arg_policy: MethodCallArgPolicy::Default,
                },
                Self::checked_c_pointer_ir_type(
                    mutable_pointer,
                    &IrCheckedCType::Scalar(span_access.span_kind.element),
                ),
            )));
        }
        if matches!(
            (span_access.access, method),
            (CAbiSpanAccessKind::ElementCount, _) | (CAbiSpanAccessKind::ElementCapacity, _)
        ) && args.is_empty()
        {
            return Ok(Some(TypedExpr::new(
                IrExprKind::BuiltinCall {
                    func: BuiltinFn::Len,
                    args: vec![self.lower_expr_spanned(receiver)?],
                },
                IrType::Numeric(incan_core::lang::types::numerics::NumericTypeId::USize),
            )));
        }
        if span_access.access != CAbiSpanAccessKind::Finish || args.len() != 1 {
            return Ok(None);
        }
        let mut receiver = self.lower_expr_spanned(receiver)?;
        Self::set_checked_c_argument_access(&mut receiver, VarAccess::Move);
        let mut lowered_args = self.lower_call_args(args)?;
        let Some(written) = lowered_args.pop() else {
            return Err(LoweringError {
                message: "checked C span finish lost its written-count argument".to_string(),
                span: call_span.into(),
            });
        };
        if !lowered_args.is_empty() {
            return Err(LoweringError {
                message: "checked C span finish retained an unexpected argument".to_string(),
                span: call_span.into(),
            });
        }
        let storage = match span_access.span_kind.element {
            incan_core::lang::c_abi::ScalarTypeId::U8 => IrType::Bytes,
            incan_core::lang::c_abi::ScalarTypeId::F32 => IrType::List(Box::new(IrType::Numeric(
                incan_core::lang::types::numerics::NumericTypeId::F32,
            ))),
            _ => return Ok(None),
        };
        let helper = incan_core::lang::c_abi::MUTABLE_SPAN_FINISH_RUST_NAME;
        let return_type = IrType::Result(Box::new(storage.clone()), Box::new(IrType::String));
        Ok(Some(TypedExpr::new(
            IrExprKind::Call {
                func: Box::new(TypedExpr::new(
                    IrExprKind::Var {
                        name: helper.to_string(),
                        access: VarAccess::Copy,
                        ref_kind: VarRefKind::Value,
                    },
                    IrType::Function {
                        params: vec![
                            storage,
                            IrType::Numeric(incan_core::lang::types::numerics::NumericTypeId::USize),
                        ],
                        ret: Box::new(return_type.clone()),
                    },
                )),
                type_args: Vec::new(),
                args: vec![
                    IrCallArg {
                        name: None,
                        kind: IrCallArgKind::Positional,
                        expr: receiver,
                    },
                    IrCallArg {
                        name: None,
                        kind: IrCallArgKind::Positional,
                        expr: written.expr,
                    },
                ],
                callable_signature: None,
                canonical_path: None,
            },
            return_type,
        )))
    }

    /// Lower a bounded owning copy from a returned scoped C string view.
    fn lower_checked_c_scoped_string_copy(
        &mut self,
        receiver: &ast::Spanned<ast::Expr>,
        method: &str,
        type_args: &[ast::Spanned<ast::Type>],
        args: &[ast::CallArg],
    ) -> Result<Option<TypedExpr>, LoweringError> {
        if method != "copy_utf8" || !type_args.is_empty() || args.len() != 1 {
            return Ok(None);
        }
        let is_scoped_c_string_view = self
            .type_info
            .as_ref()
            .and_then(|info| info.expr_type(receiver.span))
            .is_some_and(|ty| {
                matches!(ty, ResolvedType::Named(identity) if identity == incan_core::lang::c_abi::SCOPED_C_STRING_VIEW_TYPE_ID)
            });
        if !is_scoped_c_string_view {
            return Ok(None);
        }
        let return_type = IrType::Result(Box::new(IrType::String), Box::new(IrType::String));
        let function = TypedExpr::new(
            IrExprKind::Var {
                name: incan_core::lang::c_abi::SCOPED_C_STRING_COPY_UTF8_RUST_NAME.to_string(),
                access: VarAccess::Copy,
                ref_kind: VarRefKind::Value,
            },
            IrType::Function {
                params: vec![
                    Self::checked_c_pointer_ir_type(
                        false,
                        &IrCheckedCType::Scalar(incan_core::lang::c_abi::ScalarTypeId::CChar),
                    ),
                    IrType::Int,
                ],
                ret: Box::new(return_type.clone()),
            },
        );
        let max_bytes = self.lower_call_args(args)?;
        Ok(Some(TypedExpr::new(
            IrExprKind::Call {
                func: Box::new(function),
                type_args: Vec::new(),
                args: vec![
                    IrCallArg {
                        name: None,
                        kind: IrCallArgKind::Positional,
                        expr: self.lower_expr_spanned(receiver)?,
                    },
                    IrCallArg {
                        name: None,
                        kind: IrCallArgKind::Positional,
                        expr: max_bytes
                            .into_iter()
                            .next()
                            .ok_or_else(|| LoweringError {
                                message: "checked C scoped string copy lost its max_bytes argument".to_string(),
                                span: receiver.span.into(),
                            })?
                            .expr,
                    },
                ],
                callable_signature: None,
                canonical_path: None,
            },
            return_type,
        )))
    }

    /// Lower compiler-managed output storage and direct checked C symbols outside the recursive expression frame.
    ///
    /// Large ordinary source expressions lower recursively. Keeping this bounded checked-C plan in a separate frame
    /// prevents descriptor and argument-plan locals from increasing the stack required by unrelated source programs.
    #[inline(never)]
    fn lower_checked_c_method_call(
        &mut self,
        call_span: ast::Span,
        receiver: &ast::Spanned<ast::Expr>,
        method: &str,
        type_args: &[ast::Spanned<ast::Type>],
        args: &[ast::CallArg],
    ) -> Result<Option<TypedExpr>, LoweringError> {
        if let Some((kind, ty)) = self.lower_checked_c_output_slot_constructor(call_span, args)? {
            return Ok(Some(TypedExpr::new(kind, ty)));
        }
        if let Some((kind, ty)) = self.lower_checked_c_string_constructor(call_span, args)? {
            return Ok(Some(TypedExpr::new(kind, ty)));
        }
        if let Some((kind, ty)) = self.lower_checked_c_span_constructor(call_span, args)? {
            return Ok(Some(TypedExpr::new(kind, ty)));
        }
        if let Some(lowered) = self.lower_checked_c_output_slot_take(call_span, receiver, method, type_args, args)? {
            return Ok(Some(lowered));
        }
        if let Some(lowered) = self.lower_checked_c_string_pointer(receiver, method, type_args, args)? {
            return Ok(Some(lowered));
        }
        if let Some(lowered) = self.lower_checked_c_span_method(call_span, receiver, method, type_args, args)? {
            return Ok(Some(lowered));
        }
        if let Some(lowered) = self.lower_checked_c_scoped_string_copy(receiver, method, type_args, args)? {
            return Ok(Some(lowered));
        }
        let Some(c_function) = self.checked_c_function_for_call(call_span) else {
            return Ok(None);
        };
        let mut args = Self::order_checked_c_call_args(&c_function, self.lower_call_args(args)?);
        Self::apply_checked_c_argument_accesses(&c_function, &mut args);
        let parameter_types = c_function
            .parameters
            .iter()
            .enumerate()
            .map(|(index, parameter)| Self::checked_c_parameter_ir_type(&c_function, index, parameter))
            .collect::<Vec<_>>();
        let return_type = Self::checked_c_value_ir_type(&c_function.binding, &c_function.return_type);
        Ok(Some(TypedExpr::new(
            IrExprKind::Call {
                func: Box::new(TypedExpr::new(
                    IrExprKind::Var {
                        name: c_function.rust_name(),
                        access: VarAccess::Copy,
                        ref_kind: VarRefKind::Value,
                    },
                    IrType::Function {
                        params: parameter_types,
                        ret: Box::new(return_type.clone()),
                    },
                )),
                type_args: self.lower_call_site_type_args(call_span, type_args),
                args,
                callable_signature: None,
                canonical_path: None,
            },
            return_type,
        )))
    }

    /// Lower backend-neutral trait ownership into the Rust-visible dispatch path used by the current backend.
    pub(super) fn lower_resolved_method_dispatch(
        &self,
        dispatch: ResolvedMethodDispatch,
        receiver: &TypedExpr,
    ) -> IrMethodDispatch {
        match dispatch {
            ResolvedMethodDispatch::Trait {
                trait_name,
                module_path,
                type_args,
                implementation_type_params,
                receiver_is_mutable,
            } => {
                let trait_source_name = trait_name.clone();
                let trait_module_path = module_path.clone();
                let stdlib_module = module_path
                    .as_deref()
                    .filter(|segments| segments.first().map(String::as_str) == Some(stdlib::STDLIB_ROOT));
                let source_json_module = stdlib_module.filter(|segments| {
                    stdlib::is_stdlib_json_trait_module_path(segments)
                        && stdlib::stdlib_json_trait_id(&trait_name).is_some()
                });
                let trait_path = if let Some(segments) = source_json_module {
                    self.lower_stdlib_trait_dispatch_path(segments, &trait_name, receiver)
                } else if stdlib::stdlib_json_trait_scope_import_id(&trait_name).is_some() {
                    if stdlib::is_canonical_stdlib_json_trait_name(&trait_name) {
                        let canonical_module = vec![
                            stdlib::STDLIB_ROOT.to_string(),
                            stdlib::STDLIB_SERDE.to_string(),
                            stdlib::STDLIB_JSON.to_string(),
                        ];
                        let short_name = trait_name.rsplit('.').next().unwrap_or(&trait_name);
                        self.lower_stdlib_trait_dispatch_path(&canonical_module, short_name, receiver)
                    } else {
                        trait_name.replace('.', "::")
                    }
                } else if stdlib::stdlib_json_trait_id(&trait_name).is_some() {
                    trait_name
                } else if let Some(rust_path) = trait_bounds::incan_to_rust(&trait_name) {
                    rust_path.to_string()
                } else if let Some(segments) = stdlib_module {
                    self.lower_stdlib_trait_dispatch_path(segments, &trait_name, receiver)
                } else {
                    trait_name
                };
                IrMethodDispatch::Trait(Box::new(IrTraitDispatch {
                    trait_source_name,
                    trait_module_path,
                    implementation_type_params: self.lower_implementation_type_params(&implementation_type_params),
                    trait_path,
                    type_args: type_args
                        .iter()
                        .map(|ty| self.lower_resolved_method_type_arg(ty))
                        .collect(),
                    receiver_is_mutable,
                }))
            }
        }
    }

    /// Lower checked implementation-header parameters into dispatch-owned IR metadata.
    fn lower_implementation_type_params(&self, type_params: &[ImplementationTypeParamInfo]) -> Vec<IrTypeParam> {
        type_params
            .iter()
            .map(|type_param| IrTypeParam {
                name: type_param.name.clone(),
                bounds: type_param
                    .bounds
                    .iter()
                    .map(|bound| IrTraitBound {
                        trait_path: bound.trait_path.clone(),
                        type_args: bound
                            .type_args
                            .iter()
                            .map(|ty| self.lower_resolved_method_type_arg(ty))
                            .collect(),
                        assoc_types: bound
                            .associated_types
                            .iter()
                            .map(|(name, ty)| (name.clone(), self.lower_resolved_method_type_arg(ty)))
                            .collect(),
                        origin: match bound.origin {
                            ImplementationTraitBoundOriginInfo::Standard => IrTraitBoundOrigin::Standard,
                            ImplementationTraitBoundOriginInfo::RustCapability => IrTraitBoundOrigin::RustCapability,
                            ImplementationTraitBoundOriginInfo::SourceCallable => IrTraitBoundOrigin::SourceCallable,
                        },
                    })
                    .collect(),
            })
            .collect()
    }

    /// Resolve one source-owned stdlib trait through the provider, public package, or provider-local facade that owns
    /// the receiver at the current compilation boundary.
    fn lower_stdlib_trait_dispatch_path(&self, segments: &[String], trait_name: &str, receiver: &TypedExpr) -> String {
        let module = segments.iter().skip(1).cloned().collect::<Vec<_>>().join("::");
        let trait_name = trait_name
            .rsplit(['.', ':'])
            .find(|segment| !segment.is_empty())
            .unwrap_or(trait_name);
        if self.sdk_provider_build {
            return format!("crate::{}::{module}::{trait_name}", stdlib::INCAN_STD_NAMESPACE);
        }
        if let Some(library) = self.public_library_for_method_receiver(receiver) {
            return format!("{library}::{}::{module}::{trait_name}", stdlib::INCAN_STD_NAMESPACE);
        }
        match self
            .provider_plan
            .as_deref()
            .and_then(|plan| plan.active_sdk_provider_for_module(segments))
            .and_then(|provider| provider.artifact.as_ref())
        {
            Some(artifact) => format!(
                "{}::{}::{module}::{trait_name}",
                artifact.dependency_key,
                stdlib::INCAN_STD_NAMESPACE
            ),
            None => format!("crate::{}::{module}::{trait_name}", stdlib::INCAN_STD_NAMESPACE),
        }
    }

    /// Return the public dependency owner for a method receiver when lowering can prove one.
    fn public_library_for_method_receiver(&self, receiver: &TypedExpr) -> Option<String> {
        match &receiver.kind {
            IrExprKind::Call {
                canonical_path: Some(path),
                ..
            } => Self::public_library_from_canonical_path(path),
            IrExprKind::MethodCall { receiver, .. } | IrExprKind::KnownMethodCall { receiver, .. } => {
                self.public_library_for_method_receiver(receiver)
            }
            IrExprKind::InteropCoerce { expr, .. } => self.public_library_for_method_receiver(expr),
            _ => self.public_library_for_nominal_receiver_type(&receiver.ty),
        }
    }

    /// Return the library key from a canonical `pub::<library>::...` path.
    fn public_library_from_canonical_path(path: &[String]) -> Option<String> {
        if path.first().map(String::as_str) == Some("pub") {
            path.get(1).cloned()
        } else {
            None
        }
    }

    /// Return the public dependency owner for an explicitly imported nominal receiver type.
    ///
    /// If the receiver type is not directly imported, this falls back only when exactly one loaded public dependency
    /// exports that nominal type and no local declaration shadows it.
    fn public_library_for_nominal_receiver_type(&self, ty: &IrType) -> Option<String> {
        let name = match ty {
            IrType::Struct(name) | IrType::Enum(name) | IrType::NamedGeneric(name, _) => {
                name.rsplit("::").next().unwrap_or(name)
            }
            IrType::Ref(inner) | IrType::RefMut(inner) => {
                return self.public_library_for_nominal_receiver_type(inner);
            }
            _ => return None,
        };

        if let Some(path) = self.import_aliases.get(name)
            && path.first().map(String::as_str) == Some("pub")
        {
            return path.get(1).cloned();
        }

        if self.struct_names.contains_key(name) || self.enum_names.contains_key(name) {
            return None;
        }

        let manifest_index = self.provider_plan.as_deref()?.library_manifest_index();
        let matches = manifest_index
            .known_libraries()
            .into_iter()
            .filter(|library| {
                let Some(LibraryManifestIndexEntry::Loaded { manifest, .. }) = manifest_index.get(library) else {
                    return false;
                };
                manifest.exports.models.iter().any(|item| item.name == name)
                    || manifest.exports.classes.iter().any(|item| item.name == name)
                    || manifest.exports.newtypes.iter().any(|item| item.name == name)
                    || manifest.exports.enums.iter().any(|item| item.name == name)
            })
            .collect::<Vec<_>>();
        match matches.as_slice() {
            [library] => Some(library.clone()),
            _ => None,
        }
    }

    /// Return the source-defined `std.logging.Logger.<method>` signature, including default expressions.
    fn std_logging_logger_method_signature(
        &mut self,
        method: &str,
    ) -> Result<Option<super::super::FunctionSignature>, LoweringError> {
        self.callable_signature_for_imported_stdlib_type_method_path(
            &["std".to_string(), "logging".to_string(), "Logger".to_string()],
            method,
        )
    }

    /// Merge typechecker call-site metadata with the source-defined `std.logging.Logger` method declaration.
    ///
    /// The call-site snapshot carries the selected parameter types; the stdlib declaration carries source defaults such
    /// as `fields={}`. Keeping the merge here lets emission stay independent from logging-specific method names.
    fn std_logging_callable_signature_for_call(
        &mut self,
        span: ast::Span,
        method: &str,
    ) -> Result<Option<super::super::FunctionSignature>, LoweringError> {
        let call_site = self.callable_signature_for_call_span(span);
        let stdlib = self.std_logging_logger_method_signature(method)?;
        match (call_site, stdlib) {
            (Some(mut call_site), Some(stdlib)) => {
                for (param, stdlib_param) in call_site.params.iter_mut().zip(stdlib.params.iter()) {
                    if param.default.is_none() {
                        param.default = stdlib_param.default.clone();
                    }
                }
                Ok(Some(call_site))
            }
            (Some(call_site), None) => Ok(Some(call_site)),
            (None, stdlib) => Ok(stdlib),
        }
    }

    /// Lower `race for value:` to the IR race expression used by Rust emission.
    fn lower_race_for_expr(
        &mut self,
        race: &ast::RaceForExpr,
        expr_span: ast::Span,
    ) -> Result<TypedExpr, LoweringError> {
        let result_ty = self
            .type_info
            .as_ref()
            .and_then(|info| info.expr_type(expr_span))
            .map(|ty| self.lower_resolved_type(ty))
            .unwrap_or(IrType::Unknown);

        let mut arms = Vec::with_capacity(race.arms.len());
        for arm in &race.arms {
            let awaitable = self.lower_awaitable_operand(&arm.awaitable)?;
            let binding_ty = Self::race_binding_type_for_awaitable(&awaitable);

            self.push_scope();
            self.define_local_binding(race.binding.node.clone(), binding_ty, false);
            let body_result = self.lower_race_arm_body(&arm.body);
            self.pop_scope();

            arms.push(RaceArm {
                awaitable,
                body: body_result?,
            });
        }

        Ok(TypedExpr::new(
            IrExprKind::Race {
                binding: race.binding.node.clone(),
                arms,
            },
            result_ty,
        ))
    }

    /// Lower one race arm body with the arm-local winner binding already in scope.
    fn lower_race_arm_body(&mut self, body: &ast::RaceForBody) -> Result<TypedExpr, LoweringError> {
        match body {
            ast::RaceForBody::Expr(expr) => self.lower_expr_spanned(expr),
            ast::RaceForBody::Block(stmts) => self.lower_race_arm_block_body(stmts),
        }
    }

    /// Lower an await operand, applying typechecker-proven wrapper delegation when a concrete `Awaitable[T]` wrapper
    /// delegates to one awaitable field.
    fn lower_awaitable_operand(&mut self, operand: &Spanned<ast::Expr>) -> Result<TypedExpr, LoweringError> {
        let lowered = self.lower_expr_spanned(operand)?;
        let Some(field) = self.awaitable_delegation_field_for_span(operand.span) else {
            return Ok(lowered);
        };
        Ok(TypedExpr::new(
            IrExprKind::Field {
                object: Box::new(lowered),
                field,
            },
            IrType::Unknown,
        ))
    }

    /// Return the delegated field name for an expression whose resolved type is a wrapper `Awaitable[T]`.
    fn awaitable_delegation_field_for_span(&self, span: ast::Span) -> Option<String> {
        let type_info = self.type_info.as_ref()?;
        let expr_ty = type_info.expr_type(span)?;
        let type_name = match expr_ty {
            crate::frontend::symbols::ResolvedType::Named(name)
            | crate::frontend::symbols::ResolvedType::Generic(name, _) => name,
            crate::frontend::symbols::ResolvedType::Ref(inner)
            | crate::frontend::symbols::ResolvedType::RefMut(inner) => match inner.as_ref() {
                crate::frontend::symbols::ResolvedType::Named(name)
                | crate::frontend::symbols::ResolvedType::Generic(name, _) => name,
                _ => return None,
            },
            _ => return None,
        };
        type_info
            .expressions
            .awaitable_delegation_fields
            .get(type_name)
            .cloned()
    }

    /// Lower a block race arm, treating a trailing expression statement as the arm value.
    fn lower_race_arm_block_body(&mut self, stmts: &[Spanned<ast::Statement>]) -> Result<TypedExpr, LoweringError> {
        let Some((last, prefix)) = stmts.split_last() else {
            return Ok(TypedExpr::new(
                IrExprKind::Block {
                    stmts: Vec::new(),
                    value: None,
                },
                IrType::Unit,
            ));
        };

        let lowered_stmts = self.lower_statements(prefix)?;
        if let ast::Statement::Expr(expr) = &last.node {
            let value = self.lower_expr_spanned(expr)?;
            let ty = value.ty.clone();
            Ok(TypedExpr::new(
                IrExprKind::Block {
                    stmts: lowered_stmts,
                    value: Some(Box::new(value)),
                },
                ty,
            ))
        } else {
            let mut lowered_stmts = lowered_stmts;
            lowered_stmts.push(self.lower_statement(&last.node, last.span)?);
            Ok(TypedExpr::new(
                IrExprKind::Block {
                    stmts: lowered_stmts,
                    value: None,
                },
                IrType::Unit,
            ))
        }
    }

    /// Infer the arm-local winner binding type from a lowered awaitable expression.
    fn race_binding_type_for_awaitable(awaitable: &TypedExpr) -> IrType {
        match &awaitable.ty {
            IrType::NamedGeneric(name, args)
                if surface_types::from_str(name.as_str()) == Some(SurfaceTypeId::JoinHandle) && args.len() == 1 =>
            {
                IrType::Result(
                    Box::new(args[0].clone()),
                    Box::new(IrType::Struct(TASK_JOIN_ERROR_TYPE_NAME.to_string())),
                )
            }
            IrType::NamedGeneric(name, args)
                if builtin_traits::from_str(name.as_str()) == Some(TraitId::Awaitable) && args.len() == 1 =>
            {
                args[0].clone()
            }
            _ => awaitable.ty.clone(),
        }
    }

    /// Lower `list.repeat(...)` arguments in canonical helper-parameter order.
    ///
    /// The surface helper accepts named arguments, but `BuiltinFn::ListRepeat` stores arguments positionally for
    /// emission, so lowering must bind `value` before `count` instead of preserving source order.
    fn lower_builtin_list_repeat_args(&mut self, args: &[ast::CallArg]) -> Result<Vec<TypedExpr>, LoweringError> {
        let mut value = None;
        let mut count = None;
        let mut positional_index = 0usize;

        for arg in args {
            match arg {
                ast::CallArg::Positional(expr) => {
                    match positional_index {
                        0 if value.is_none() => value = Some(expr),
                        1 if count.is_none() => count = Some(expr),
                        _ => {}
                    }
                    positional_index += 1;
                }
                ast::CallArg::Named(name, expr) => match name.node.as_str() {
                    "value" if value.is_none() => value = Some(expr),
                    "count" if count.is_none() => count = Some(expr),
                    _ => {}
                },
                ast::CallArg::PositionalUnpack(_) | ast::CallArg::KeywordUnpack(_) => {}
            }
        }

        let mut lowered = Vec::with_capacity(2);
        if let Some(value) = value {
            lowered.push(self.lower_expr_spanned(value)?);
        }
        if let Some(count) = count {
            lowered.push(self.lower_expr_spanned(count)?);
        }
        Ok(lowered)
    }

    /// Return whether a concrete receiver type explicitly adopts the Incan `Iterator` protocol.
    fn receiver_adopts_iterator_protocol(&self, ty: &IrType) -> bool {
        let mut ty = ty;
        while let IrType::Ref(inner) | IrType::RefMut(inner) = ty {
            ty = inner.as_ref();
        }
        match ty {
            IrType::Struct(name) | IrType::NamedGeneric(name, _) => self.iterator_adopter_names.contains(name),
            _ => false,
        }
    }

    /// Return whether a generic type parameter should keep Rust's comparison operators instead of lowering to a
    /// dunder-style method call.
    ///
    /// Generic `T with Ord`/`PartialOrd` bounds lower to Rust trait bounds such as `PartialOrd`; they do not introduce
    /// inherent `__lt__`/`__le__` methods on `T`. Keeping the operator form lets Rust type-check the generic bound.
    fn generic_comparison_uses_rust_operator(&self, left: &Spanned<ast::Expr>, method: &str) -> bool {
        if !matches!(method, "__ne__" | "__lt__" | "__le__" | "__gt__" | "__ge__") {
            return false;
        }
        self.type_info
            .as_ref()
            .and_then(|info| info.expr_type(left.span))
            .map(|ty| self.lower_resolved_type(ty))
            .is_some_and(|ty| matches!(ty, IrType::Generic(_)))
    }

    /// Lower a control-flow condition, rewriting validated `__bool__` hooks into direct method calls.
    pub(in crate::backend::ir::lower) fn lower_condition_expr(
        &mut self,
        expr: &Spanned<ast::Expr>,
    ) -> Result<TypedExpr, LoweringError> {
        let receiver = self.lower_expr_spanned(expr)?;
        if let Some(resolved_operator) = self
            .type_info
            .as_ref()
            .and_then(|info| info.resolved_operator_call(expr.span).cloned())
            && resolved_operator.kind == ResolvedOperatorKind::Truthiness
        {
            let dispatch = self
                .type_info
                .as_ref()
                .and_then(|info| info.resolved_method_call(expr.span).cloned())
                .map(|resolved| self.lower_resolved_method_dispatch(resolved.dispatch, &receiver));
            let (method, dispatch) =
                self.project_resolved_method_target(expr.span, &resolved_operator.method, &receiver, dispatch);
            return Ok(TypedExpr::new(
                IrExprKind::MethodCall {
                    receiver: Box::new(receiver),
                    method,
                    dispatch,
                    type_args: Vec::new(),
                    args: Vec::new(),
                    callable_signature: self.callable_signature_for_call_span(expr.span),
                    arg_policy: MethodCallArgPolicy::Default,
                },
                IrType::Bool,
            ));
        }
        Ok(receiver)
    }

    /// Return the element type carried by a lowered list spread operand.
    fn lowered_list_spread_element_type(ty: &IrType) -> Option<IrType> {
        match ty {
            IrType::List(elem) => Some((**elem).clone()),
            _ => None,
        }
    }

    /// Return the key/value types carried by a lowered dict spread operand.
    fn lowered_dict_spread_entry_types(ty: &IrType) -> Option<(IrType, IrType)> {
        match ty {
            IrType::Dict(key, value) => Some(((**key).clone(), (**value).clone())),
            _ => None,
        }
    }

    /// Classify an IR type as a Rust collection family.
    fn rust_collection_family_for_ir_type(ty: &IrType) -> Option<RustCollectionFamily> {
        match ty {
            IrType::Struct(name) | IrType::NamedGeneric(name, _) => {
                RustCollectionFamily::for_canonical_path(name).or(RustCollectionFamily::for_type_name(name))
            }
            IrType::Ref(inner) | IrType::RefMut(inner) => Self::rust_collection_family_for_ir_type(inner),
            _ => None,
        }
    }

    /// Return the ordinary argument policy for a method call.
    fn regular_method_call_arg_policy(
        &self,
        receiver_span: crate::frontend::ast::Span,
        receiver: &TypedExpr,
        method: &str,
        args: &[IrCallArg],
    ) -> MethodCallArgPolicy {
        if self
            .type_info
            .as_ref()
            .is_some_and(|info| info.preserves_regular_method_arg_shape(receiver_span, method))
        {
            return MethodCallArgPolicy::PreserveShape;
        }

        if Self::rust_collection_family_for_ir_type(&receiver.ty)
            .is_some_and(|family| family.preserves_lookup_arg_shape(method))
        {
            return MethodCallArgPolicy::PreserveShape;
        }

        // Fallback for unresolved Rust-interop receivers when optional rust-inspect metadata is unavailable or local
        // type inference did not retain the receiver family. Keep lookup calls like `counts.get(word)` borrow-shaped
        // rather than forcing an extra `&`/`.into()` conversion on already string-like probe values.
        if matches!(receiver.ty, IrType::Unknown)
            && matches!(method, "get" | "contains" | "contains_key")
            && args.first().is_some_and(|arg| {
                matches!(
                    arg.expr.ty,
                    IrType::String | IrType::StrRef | IrType::StaticStr | IrType::FrozenStr
                )
            })
        {
            return MethodCallArgPolicy::PreserveShape;
        }

        MethodCallArgPolicy::Default
    }

    /// Lower an expression using the available typechecker output (if present).
    ///
    /// This wraps [`Self::lower_expr`] and then overrides the inferred IR type using the typechecker span-to-type map.
    /// This is a stepping stone toward fully typed lowering.
    pub fn lower_expr_spanned(&mut self, expr: &Spanned<ast::Expr>) -> Result<TypedExpr, LoweringError> {
        if let Some((kind, ty)) = self.lower_partial_constructor_call(expr)? {
            return Ok(TypedExpr::new(kind, ty));
        }
        let mut lowered = self.lower_expr(&expr.node, expr.span)?;
        if let Some(info) = &self.type_info
            && let Some(res_ty) = info.expr_type(expr.span)
        {
            // Preserve reference wrappers introduced by lowering (e.g. mutable parameters are tracked as
            // `RefMut(T)` in IR), while still benefiting from the typechecker's inner type information.
            //
            // The frontend type system does not model references, so `expr_type` typically returns `T` where
            // lowering may have already marked the same binding as `Ref(T)`/`RefMut(T)`.
            //
            // Likewise, RFC-008 const lowering may have already refined `str`/`bytes` to their static IR forms.
            // Keep those backend-specific const representations intact so later emission can materialize owned
            // values only when required.
            let inferred = self.lower_resolved_type(res_ty);
            lowered.ty = match &lowered.ty {
                IrType::Ref(existing_inner) => {
                    IrType::Ref(Box::new(Self::merge_inferred_ir_type(existing_inner, inferred)))
                }
                IrType::RefMut(existing_inner) => {
                    IrType::RefMut(Box::new(Self::merge_inferred_ir_type(existing_inner, inferred)))
                }
                IrType::StaticStr => IrType::StaticStr,
                IrType::StaticBytes => IrType::StaticBytes,
                existing => Self::merge_inferred_ir_type(existing, inferred),
            };
        }
        if matches!(expr.node, ast::Expr::Ident(_))
            && let IrType::TypeToken(inner) = &lowered.ty
        {
            lowered.kind = IrExprKind::TypeToken {
                ty: inner.as_ref().clone(),
            };
        }
        if let Some(kind) = self.ident_kind_for_lowering(expr) {
            match (&expr.node, &mut lowered.kind) {
                (ast::Expr::Ident(name), _) if matches!(kind, IdentKind::Static) => {
                    lowered.kind = IrExprKind::StaticRead {
                        name: name.clone(),
                        reference_kind: super::super::expr::IrStaticReferenceKind::Source,
                    };
                }
                (ast::Expr::Ident(name), IrExprKind::Var { ref_kind, .. }) => {
                    *ref_kind = match kind {
                        IdentKind::Value => *ref_kind,
                        IdentKind::Static => *ref_kind,
                        IdentKind::TypeName => VarRefKind::TypeName,
                        IdentKind::Variant => VarRefKind::TypeName,
                        IdentKind::Module => VarRefKind::ExternalName,
                        IdentKind::RustImport => VarRefKind::ExternalRustName,
                        IdentKind::RustValue => VarRefKind::Value,
                        IdentKind::Trait => VarRefKind::TypeName,
                    };
                    if matches!(kind, IdentKind::TypeName | IdentKind::Variant | IdentKind::Trait)
                        && matches!(lowered.ty, IrType::Unknown)
                        && let Some(ty) = self.synthetic_type_ident_ir_type(name)
                    {
                        lowered.ty = ty;
                    }
                }
                (_, IrExprKind::Var { ref_kind, .. }) => {
                    *ref_kind = match kind {
                        IdentKind::Value => *ref_kind,
                        IdentKind::Static => *ref_kind,
                        IdentKind::TypeName => VarRefKind::TypeName,
                        IdentKind::Variant => VarRefKind::TypeName,
                        IdentKind::Module => VarRefKind::ExternalName,
                        IdentKind::RustImport => VarRefKind::ExternalRustName,
                        IdentKind::RustValue => VarRefKind::Value,
                        IdentKind::Trait => VarRefKind::TypeName,
                    };
                }
                _ => {}
            }
        }
        // Apply any rusttype method return coercion recorded by the typechecker (e.g. &str → String).
        lowered = self.wrap_with_rust_return_coercion(lowered, expr.span)?;
        // Apply RFC 017 implicit validated-newtype coercions at typechecker-approved destination sites.
        lowered = self.wrap_with_validated_newtype_coercion(lowered, expr.span)?;
        Ok(lowered)
    }

    /// Lower a known model-constructor partial call through the ordinary constructor lowering path.
    fn lower_partial_constructor_call(
        &mut self,
        expr: &Spanned<ast::Expr>,
    ) -> Result<Option<(IrExprKind, IrType)>, LoweringError> {
        let ast::Expr::Call(callee, type_args, args) = &expr.node else {
            return Ok(None);
        };
        if !type_args.is_empty() {
            return Ok(None);
        }
        let ast::Expr::Ident(callee_name) = &callee.node else {
            return Ok(None);
        };
        let Some(projection) = self
            .type_info
            .as_ref()
            .and_then(|info| info.partial_projection(callee_name))
            .cloned()
        else {
            return Ok(None);
        };
        if projection.target_kind != PartialProjectionTargetKind::ModelConstructor {
            return Ok(None);
        }
        let Some(target_name) = projection.target_path.last() else {
            return Ok(None);
        };
        if !self.struct_names.contains_key(target_name) && !self.import_aliases.contains_key(target_name) {
            return Ok(None);
        }
        let Some(merged_args) = merge_named_partial_args(
            projection.presets.iter().map(|preset| PartialPresetRef {
                name: preset.name.as_str(),
                value: &preset.value,
            }),
            args,
        ) else {
            return Ok(None);
        };
        self.lower_constructor_call(target_name, &[], &merged_args, expr.span)
            .map(Some)
    }

    /// Return the identifier classification that lowering should use for this expression.
    ///
    /// Most source expressions use span-keyed frontend metadata. Synthetic expressions created by lowering, such as
    /// user-defined decorator factory calls, intentionally use the default span so they do not collide with call-site
    /// expression types. Those synthetic nodes still need metadata-backed classification for type names and module
    /// statics; otherwise they fall back to value-shaped Rust emission.
    fn ident_kind_for_lowering(&self, expr: &Spanned<ast::Expr>) -> Option<IdentKind> {
        if let Some(kind) = self.type_info.as_ref().and_then(|info| info.ident_kind(expr.span)) {
            return Some(kind);
        }
        if expr.span != ast::Span::default() {
            return None;
        }
        let ast::Expr::Ident(name) = &expr.node else {
            return None;
        };
        if self
            .type_info
            .as_ref()
            .is_some_and(|info| info.static_binding(name).is_some())
        {
            return Some(IdentKind::Static);
        }
        if self.synthetic_type_ident_ir_type(name).is_some() {
            return Some(IdentKind::TypeName);
        }
        None
    }

    /// Return the known IR type for a synthetic type-like identifier.
    fn synthetic_type_ident_ir_type(&self, name: &str) -> Option<IrType> {
        self.struct_names
            .get(name)
            .cloned()
            .or_else(|| self.enum_names.get(name).cloned())
            .or_else(|| {
                self.class_decls
                    .contains_key(name)
                    .then(|| IrType::Struct(name.to_string()))
            })
            .or_else(|| {
                self.trait_decls
                    .contains_key(name)
                    .then(|| IrType::Struct(name.to_string()))
            })
    }

    /// Lower an expression to IR.
    ///
    /// Handles all expression types including:
    /// - Literals (int, float, string, bool)
    /// - Identifiers (variable references)
    /// - Binary and unary operations
    /// - Function and method calls
    /// - Field and index access
    /// - Control flow expressions (if, match)
    /// - Collections (list, dict, set, tuple)
    /// - Comprehensions (list, dict)
    /// - Closures and async/await
    ///
    /// `expr_span` must be the span of the whole `expr` node (as in [`Self::lower_expr_spanned`]). It is required for
    /// [`Expr::Call`](ast::Expr::Call) and [`Expr::MethodCall`](ast::Expr::MethodCall) so lowering can align with the
    /// typechecker’s span-keyed metadata (RFC 054 monomorph snapshots).
    pub fn lower_expr(&mut self, expr: &ast::Expr, expr_span: ast::Span) -> Result<TypedExpr, LoweringError> {
        let (kind, ty) = match expr {
            // ---- Identifiers ----
            ast::Expr::Ident(name) => {
                let lowered_name = self.symbol_aliases.get(name).cloned().unwrap_or_else(|| name.clone());
                let ty = self.lookup_var(&lowered_name);
                let emitted_reference_name = self
                    .emitted_function_reference_name(expr_span)
                    .unwrap_or_else(|| lowered_name.clone());
                if self
                    .type_info
                    .as_ref()
                    .is_some_and(|info| info.is_ambient_logger_binding(expr_span))
                {
                    let logger_name = self.current_default_logger_name();
                    let func = TypedExpr::new(
                        IrExprKind::Var {
                            name: "get_logger".to_string(),
                            access: VarAccess::Copy,
                            ref_kind: VarRefKind::Value,
                        },
                        IrType::Unknown,
                    );
                    let arg = IrCallArg {
                        name: None,
                        kind: IrCallArgKind::Positional,
                        expr: TypedExpr::new(
                            IrExprKind::Literal(IrLiteral::StaticStr(logger_name)),
                            IrType::StaticStr,
                        ),
                    };
                    return Ok(TypedExpr::new(
                        IrExprKind::Call {
                            func: Box::new(func),
                            type_args: Vec::new(),
                            args: vec![arg],
                            callable_signature: self.callable_signature_for_imported_stdlib_path(&[
                                "std".to_string(),
                                "logging".to_string(),
                                "get_logger".to_string(),
                            ])?,
                            canonical_path: Some(vec![
                                "std".to_string(),
                                "logging".to_string(),
                                "get_logger".to_string(),
                            ]),
                        },
                        IrType::Struct("Logger".to_string()),
                    ));
                }
                // Imported string-like bindings are dependency-owned path references, not local owned strings that can
                // be consumed by the current block's last-use analysis.
                let inferred_import_ty = self
                    .type_info
                    .as_ref()
                    .and_then(|info| info.expr_type(expr_span).cloned())
                    .map(|ty| self.lower_resolved_type(&ty));
                let access = if self.import_aliases.contains_key(name)
                    && matches!(
                        inferred_import_ty.as_ref().unwrap_or(&ty),
                        IrType::String | IrType::StaticStr | IrType::StrRef | IrType::FrozenStr
                    ) {
                    VarAccess::Read
                } else {
                    self.select_var_access_for_ident(&lowered_name, &ty)
                };
                (
                    IrExprKind::Var {
                        name: emitted_reference_name,
                        access,
                        ref_kind: if self.is_static_binding(&lowered_name) {
                            VarRefKind::StaticBinding
                        } else {
                            VarRefKind::Value
                        },
                    },
                    ty,
                )
            }

            // ---- Literals ----
            ast::Expr::Literal(lit) => match lit {
                ast::Literal::Int(il) if il.fits_i64() => (IrExprKind::Int(il.value), IrType::Int),
                ast::Literal::Int(il) => (IrExprKind::IntLiteral(il.repr.clone()), IrType::Int),
                ast::Literal::Float(fl) => (IrExprKind::Float(fl.value), IrType::Float),
                ast::Literal::Decimal(dl) => (IrExprKind::Decimal(dl.repr.clone()), IrType::Unknown),
                ast::Literal::String(s) => {
                    let ty = self
                        .type_info
                        .as_ref()
                        .and_then(|info| info.expr_type(expr_span))
                        .map(|ty| self.lower_resolved_type(ty))
                        .filter(|ty| matches!(ty, IrType::FrozenStr | IrType::StaticStr | IrType::StrRef))
                        .unwrap_or(IrType::String);
                    (IrExprKind::String(s.clone()), ty)
                }
                ast::Literal::Bytes(bytes) => {
                    let ty = self
                        .type_info
                        .as_ref()
                        .and_then(|info| info.expr_type(expr_span))
                        .map(|ty| self.lower_resolved_type(ty))
                        .filter(|ty| matches!(ty, IrType::FrozenBytes | IrType::StaticBytes))
                        .unwrap_or(IrType::Bytes);
                    (IrExprKind::Bytes(bytes.clone()), ty)
                }
                ast::Literal::Bool(b) => (IrExprKind::Bool(*b), IrType::Bool),
                ast::Literal::None => (IrExprKind::None, IrType::Option(Box::new(IrType::Unknown))),
            },

            // ---- Self expression ----
            ast::Expr::SelfExpr => (
                IrExprKind::Var {
                    name: "self".to_string(),
                    access: VarAccess::Borrow,
                    ref_kind: VarRefKind::Value,
                },
                self.lookup_var("self"),
            ),

            // ---- Binary operations ----
            ast::Expr::Binary(l, op, r) => {
                if let Some(resolved_operator) = self
                    .type_info
                    .as_ref()
                    .and_then(|info| info.resolved_operator_call(expr_span).cloned())
                    && resolved_operator.kind == ResolvedOperatorKind::Binary
                    // `__eq__` is represented in generated Rust as `PartialEq::eq`, not as an inherent method.
                    && resolved_operator.method != magic_methods::as_str(MagicMethodId::Eq)
                    && !self.generic_comparison_uses_rust_operator(l, &resolved_operator.method)
                {
                    let receiver = self.lower_expr_spanned(l)?;
                    let arg_expr = self.lower_expr_spanned(r)?;
                    let dispatch = self
                        .type_info
                        .as_ref()
                        .and_then(|info| info.resolved_method_call(expr_span).cloned())
                        .map(|resolved| self.lower_resolved_method_dispatch(resolved.dispatch, &receiver));
                    let (method, dispatch) =
                        self.project_resolved_method_target(expr_span, &resolved_operator.method, &receiver, dispatch);
                    let provider_crate = AstLowering::nominal_receiver_type_name(&receiver.ty)
                        .and_then(|type_name| self.sdk_provider_crate_for_type(type_name));
                    let result_ty = self
                        .type_info
                        .as_ref()
                        .and_then(|info| info.expr_type(expr_span))
                        .map(|ty| self.lower_resolved_type(ty))
                        .unwrap_or(IrType::Unknown);
                    let result_ty = provider_crate
                        .as_deref()
                        .map(|provider_crate| self.pub_external_type(provider_crate, result_ty.clone()))
                        .unwrap_or(result_ty);
                    let callable_signature = self.callable_signature_for_call_span(expr_span);
                    let callable_signature = match (provider_crate.as_deref(), callable_signature) {
                        (Some(provider_crate), Some(signature)) => {
                            Some(self.compiled_provider_external_signature(provider_crate, signature))
                        }
                        (_, signature) => signature,
                    };
                    (
                        IrExprKind::MethodCall {
                            receiver: Box::new(receiver),
                            method,
                            dispatch,
                            type_args: Vec::new(),
                            args: vec![IrCallArg {
                                name: None,
                                kind: IrCallArgKind::Positional,
                                expr: arg_expr,
                            }],
                            callable_signature,
                            arg_policy: MethodCallArgPolicy::Default,
                        },
                        result_ty,
                    )
                } else if let Some(resolved_operator) = self
                    .type_info
                    .as_ref()
                    .and_then(|info| info.resolved_operator_call(expr_span).cloned())
                    && resolved_operator.kind == ResolvedOperatorKind::Contains
                {
                    let item = self.lower_expr_spanned(l)?;
                    let receiver = self.lower_expr_spanned(r)?;
                    let dispatch = self
                        .type_info
                        .as_ref()
                        .and_then(|info| info.resolved_method_call(expr_span).cloned())
                        .map(|resolved| self.lower_resolved_method_dispatch(resolved.dispatch, &receiver));
                    let (method, dispatch) =
                        self.project_resolved_method_target(expr_span, &resolved_operator.method, &receiver, dispatch);
                    let contains_call = IrExprKind::MethodCall {
                        receiver: Box::new(receiver),
                        method,
                        dispatch,
                        type_args: Vec::new(),
                        args: vec![IrCallArg {
                            name: None,
                            kind: IrCallArgKind::Positional,
                            expr: item,
                        }],
                        callable_signature: self.callable_signature_for_call_span(expr_span),
                        arg_policy: MethodCallArgPolicy::Default,
                    };
                    if matches!(op, ast::BinaryOp::NotIn) {
                        (
                            IrExprKind::UnaryOp {
                                op: UnaryOp::Not,
                                operand: Box::new(IrExpr::new(contains_call, IrType::Bool)),
                            },
                            IrType::Bool,
                        )
                    } else {
                        (contains_call, IrType::Bool)
                    }
                } else {
                    // Special handling for `in` and `not in` operators
                    // - `x in collection` → builtin-aware `collection.contains(x)`
                    // - `x not in collection` → `!collection.contains(x)`
                    match op {
                        ast::BinaryOp::In | ast::BinaryOp::NotIn => {
                            let item = self.lower_expr_spanned(l)?;
                            let collection = self.lower_expr_spanned(r)?;

                            // Generate `collection.contains(item)` using the same receiver-aware classification path as
                            // ordinary method syntax so containment keeps builtin semantics for strings, lists, sets,
                            // and dicts without emitter-side name guessing.
                            let contains_args = vec![IrCallArg {
                                name: None,
                                kind: IrCallArgKind::Positional,
                                expr: item,
                            }];
                            let contains_kind = MethodKind::for_receiver(&collection.ty, "contains").or_else(|| {
                                let mut receiver_ty = &collection.ty;
                                while let IrType::Ref(inner) | IrType::RefMut(inner) = receiver_ty {
                                    receiver_ty = inner.as_ref();
                                }
                                matches!(receiver_ty, IrType::Dict(_, _))
                                    .then_some(MethodKind::Collection(CollectionMethodKind::Contains))
                            });
                            let contains_call = if let Some(kind) = contains_kind {
                                IrExprKind::KnownMethodCall {
                                    receiver: Box::new(collection),
                                    kind,
                                    args: contains_args,
                                }
                            } else {
                                let arg_policy = self.regular_method_call_arg_policy(
                                    r.span,
                                    &collection,
                                    "contains",
                                    &contains_args,
                                );
                                IrExprKind::MethodCall {
                                    receiver: Box::new(collection),
                                    method: "contains".to_string(),
                                    dispatch: None,
                                    type_args: Vec::new(),
                                    args: contains_args,
                                    callable_signature: None,
                                    arg_policy,
                                }
                            };

                            if matches!(op, ast::BinaryOp::NotIn) {
                                // Wrap in negation for `not in`
                                (
                                    IrExprKind::UnaryOp {
                                        op: UnaryOp::Not,
                                        operand: Box::new(IrExpr::new(contains_call, IrType::Bool)),
                                    },
                                    IrType::Bool,
                                )
                            } else {
                                (contains_call, IrType::Bool)
                            }
                        }
                        _ => {
                            let left = self.lower_expr_spanned(l)?;
                            let right = self.lower_expr_spanned(r)?;
                            // For Pow, compute exponent kind for policy-based result type
                            let pow_exp_kind = if matches!(op, ast::BinaryOp::Pow) {
                                Some(Self::pow_exponent_kind(r, &right.ty))
                            } else {
                                None
                            };
                            let result_ty = self.binary_result_type(&left.ty, &right.ty, op, pow_exp_kind);
                            (
                                IrExprKind::BinOp {
                                    op: self.lower_binop(op, expr_span)?,
                                    left: Box::new(left),
                                    right: Box::new(right),
                                },
                                result_ty,
                            )
                        }
                    }
                }
            }

            // ---- Unary operations ----
            ast::Expr::Unary(op, e) => {
                let operand = self.lower_expr_spanned(e)?;
                if let Some(resolved_operator) = self
                    .type_info
                    .as_ref()
                    .and_then(|info| info.resolved_operator_call(expr_span).cloned())
                    && resolved_operator.kind == ResolvedOperatorKind::Unary
                {
                    let result_ty = self
                        .type_info
                        .as_ref()
                        .and_then(|info| info.expr_type(expr_span))
                        .map(|ty| self.lower_resolved_type(ty))
                        .unwrap_or(IrType::Unknown);
                    let dispatch = self
                        .type_info
                        .as_ref()
                        .and_then(|info| info.resolved_method_call(expr_span).cloned())
                        .map(|resolved| self.lower_resolved_method_dispatch(resolved.dispatch, &operand));
                    let (method, dispatch) =
                        self.project_resolved_method_target(expr_span, &resolved_operator.method, &operand, dispatch);
                    (
                        IrExprKind::MethodCall {
                            receiver: Box::new(operand),
                            method,
                            dispatch,
                            type_args: Vec::new(),
                            args: Vec::new(),
                            callable_signature: self.callable_signature_for_call_span(expr_span),
                            arg_policy: MethodCallArgPolicy::Default,
                        },
                        result_ty,
                    )
                } else {
                    let ty = operand.ty.clone();
                    (
                        IrExprKind::UnaryOp {
                            op: match op {
                                ast::UnaryOp::Neg => UnaryOp::Neg,
                                ast::UnaryOp::Not => UnaryOp::Not,
                                ast::UnaryOp::Invert => UnaryOp::Not,
                            },
                            operand: Box::new(operand),
                        },
                        ty,
                    )
                }
            }

            // ---- Function / constructor calls (delegated to calls submodule) ----
            ast::Expr::Call(f, type_args, args) => {
                return self
                    .lower_call_expr(f, type_args, args, expr_span)
                    .map(|(k, t)| TypedExpr::new(k, t));
            }

            // ---- Method calls ----
            ast::Expr::MethodCall(o, m, type_args, args) => {
                if let Some(lowered) = self.lower_checked_c_method_call(expr_span, o, m, type_args, args)? {
                    return Ok(lowered);
                }
                if self
                    .type_info
                    .as_ref()
                    .and_then(|info| info.resolved_builtin_call(expr_span))
                    == Some(BuiltinFnId::IsInstance)
                {
                    let callee = ast::Spanned::new(ast::Expr::Field(o.clone(), m.clone()), expr_span);
                    return self
                        .lower_call_expr(&callee, type_args, args, expr_span)
                        .map(|(kind, ty)| TypedExpr::new(kind, ty));
                }
                let is_public_module_constructor = self
                    .type_info
                    .as_ref()
                    .and_then(|info| info.source_target(expr_span))
                    .is_some_and(|target| {
                        target.module_path.first().map(String::as_str) == Some("pub")
                            && matches!(target.kind.as_str(), "model" | "class" | "newtype" | "rusttype")
                    });
                if self
                    .imported_module_function_callee_path(&o.node, m, expr_span)
                    .is_some()
                    || is_public_module_constructor
                {
                    let callee = ast::Spanned::new(ast::Expr::Field(o.clone(), m.clone()), expr_span);
                    return self
                        .lower_call_expr(&callee, type_args, args, expr_span)
                        .map(|(kind, ty)| TypedExpr::new(kind, ty));
                }

                if Self::is_explicit_builtin_namespace_expr(o)
                    && let Some(builtin) = BuiltinFn::from_name(m)
                {
                    let args_ir = self.lower_call_args(args)?.into_iter().map(|a| a.expr).collect();
                    let result_ty = self.lowered_builtin_call_type(builtin, expr_span);
                    return Ok(TypedExpr::new(
                        IrExprKind::BuiltinCall {
                            func: builtin,
                            args: args_ir,
                        },
                        result_ty,
                    ));
                }

                if matches!(&o.node, ast::Expr::Ident(name)
                    if collection_helpers::from_parts(name, m) == Some(BuiltinCollectionHelperId::ListRepeat)
                        && collection_types::from_str(name.as_str()) == Some(CollectionTypeId::List))
                    && self
                        .type_info
                        .as_ref()
                        .is_some_and(|info| matches!(info.ident_kind(o.span), Some(IdentKind::TypeName)))
                {
                    let args_ir = self.lower_builtin_list_repeat_args(args)?;
                    let expr_ty = self
                        .type_info
                        .as_ref()
                        .and_then(|info| info.expr_type(expr_span))
                        .map(|ty| self.lower_resolved_type(ty))
                        .unwrap_or(IrType::Unknown);
                    return Ok(TypedExpr::new(
                        IrExprKind::BuiltinCall {
                            func: BuiltinFn::ListRepeat,
                            args: args_ir,
                        },
                        expr_ty,
                    ));
                }

                let receiver = if let ast::Expr::Index(base, _) = &o.node
                    && let ast::Expr::Ident(name) = &base.node
                    && self.type_info.as_ref().is_some_and(|info| {
                        matches!(info.ident_kind(base.span), Some(IdentKind::TypeName | IdentKind::Trait))
                    }) {
                    let ty = self
                        .type_info
                        .as_ref()
                        .and_then(|info| info.expr_type(o.span))
                        .map(|ty| self.lower_resolved_type(ty))
                        .unwrap_or_else(|| self.struct_names.get(name).cloned().unwrap_or(IrType::Unknown));
                    TypedExpr::new(
                        IrExprKind::Var {
                            name: name.clone(),
                            access: VarAccess::Copy,
                            ref_kind: VarRefKind::TypeName,
                        },
                        ty,
                    )
                } else {
                    self.lower_expr_spanned(o)?
                };
                let mut args_ir = self.lower_call_args(args)?;
                let lowered_type_args = self.lower_call_site_type_args(expr_span, type_args);
                let method_name = self.resolve_method_rebinding(&receiver.ty, m);
                let arg_policy = self.regular_method_call_arg_policy(o.span, &receiver, &method_name, &args_ir);
                for (arg_ir, arg_ast) in args_ir.iter_mut().zip(args.iter()) {
                    let arg_span = match arg_ast {
                        ast::CallArg::Positional(expr)
                        | ast::CallArg::Named(_, expr)
                        | ast::CallArg::PositionalUnpack(expr)
                        | ast::CallArg::KeywordUnpack(expr) => expr.span,
                    };
                    // A concrete borrow or a boxed variant payload is a Rust storage fact, not a shape preference:
                    // the emitted constructor is wrong without it, whatever the receiver's argument policy says.
                    let has_required_concrete_borrow = self.type_info.as_ref().is_some_and(|info| {
                        matches!(
                            info.rust_arg_coercion(arg_span).map(|coercion| coercion.kind),
                            Some(RustArgCoercionKind::Borrow { .. } | RustArgCoercionKind::BoxPayload)
                        )
                    });
                    if !matches!(arg_policy, MethodCallArgPolicy::PreserveShape) || has_required_concrete_borrow {
                        arg_ir.expr = self.wrap_with_rust_arg_coercion(arg_ir.expr.clone(), arg_span)?;
                    }
                }
                let mut expr_ty = self
                    .type_info
                    .as_ref()
                    .and_then(|info| info.expr_type(expr_span))
                    .map(|ty| self.lower_resolved_type(ty))
                    .unwrap_or(IrType::Unknown);
                if magic_methods::from_str(&method_name) == Some(MagicMethodId::ClassName)
                    && matches!(expr_ty, IrType::String)
                {
                    expr_ty = IrType::StaticStr;
                }
                let dispatch = self
                    .type_info
                    .as_ref()
                    .and_then(|info| info.resolved_method_call(expr_span).cloned())
                    .map(|resolved| self.lower_resolved_method_dispatch(resolved.dispatch, &receiver))
                    .or_else(|| {
                        self.type_info
                            .as_ref()
                            .and_then(|info| info.rust_method_trait_import_use(expr_span))
                            .map(|import_use| IrMethodDispatch::RustExtensionTraitImport {
                                binding: import_use.binding.clone(),
                            })
                    });

                if let Some(policy) = numeric_resize_policy(&method_name)
                    && args_ir.is_empty()
                    && lowered_type_args.is_empty()
                {
                    let target_ty = match (policy, &expr_ty) {
                        (NumericResizePolicy::Try, IrType::Option(inner)) => (**inner).clone(),
                        _ => expr_ty.clone(),
                    };
                    (
                        IrExprKind::NumericResize {
                            expr: Box::new(receiver),
                            policy,
                            to_type: target_ty,
                        },
                        expr_ty,
                    )
                } else if let Some(kind) = dispatch
                    .is_none()
                    .then(|| {
                        MethodKind::for_receiver(&receiver.ty, &method_name).or_else(|| {
                            if self.receiver_adopts_iterator_protocol(&receiver.ty) {
                                MethodKind::for_iterator_method_name(&method_name)
                            } else if matches!(
                                MethodKind::for_result_method_name(&method_name),
                                Some(MethodKind::Result(ResultMethodId::Inspect | ResultMethodId::InspectErr))
                            ) {
                                MethodKind::for_result_method_name(&method_name)
                            } else {
                                None
                            }
                        })
                    })
                    .flatten()
                {
                    (
                        IrExprKind::KnownMethodCall {
                            receiver: Box::new(receiver),
                            kind,
                            args: args_ir,
                        },
                        expr_ty,
                    )
                } else {
                    let imported_type_method_signature = match &o.node {
                        ast::Expr::Ident(name) => match self.import_aliases.get(name).cloned() {
                            Some(path) => self.callable_signature_for_imported_stdlib_type_method_path(&path, m)?,
                            None => None,
                        },
                        _ => None,
                    };
                    let public_receiver_library = self.public_library_for_method_receiver(&receiver);
                    let imported_pub_method_signature = public_receiver_library.as_deref().and_then(|library| {
                        self.callable_signature_for_imported_pub_type_method(library, &receiver.ty, m)
                    });
                    let compiled_provider_crate = AstLowering::nominal_receiver_type_name(&receiver.ty)
                        .and_then(|type_name| self.sdk_provider_crate_for_type(type_name));
                    let compiled_provider_method_signature =
                        self.callable_signature_for_compiled_provider_type_method(&receiver.ty, m);
                    let call_site_signature = self.callable_signature_for_call_span(expr_span);
                    let std_logging_signature = if matches!(
                        &receiver.ty,
                        IrType::Struct(name) | IrType::NamedGeneric(name, _) if name.rsplit("::").next() == Some("Logger")
                    ) {
                        self.std_logging_callable_signature_for_call(expr_span, m)?
                    } else {
                        None
                    };
                    let callable_signature = match (
                        std_logging_signature.or(call_site_signature),
                        imported_type_method_signature
                            .or(imported_pub_method_signature)
                            .or(compiled_provider_method_signature),
                    ) {
                        (Some(mut call_site), Some(imported)) => {
                            for (param, imported_param) in call_site.params.iter_mut().zip(imported.params.iter()) {
                                if param.default.is_none() {
                                    param.default = imported_param.default.clone();
                                }
                            }
                            Some(call_site)
                        }
                        (Some(call_site), None) => Some(call_site),
                        (None, imported) => imported,
                    };
                    let callable_signature = match (public_receiver_library.clone(), callable_signature) {
                        (Some(library), Some(signature)) => Some(self.pub_external_signature(&library, signature)),
                        (None, Some(signature)) => match compiled_provider_crate.as_deref() {
                            Some(provider_crate) => {
                                Some(self.compiled_provider_external_signature(provider_crate, signature))
                            }
                            None => Some(signature),
                        },
                        (_, None) => None,
                    };
                    let expr_ty = compiled_provider_crate
                        .as_deref()
                        .map(|provider_crate| self.pub_external_type(provider_crate, expr_ty.clone()))
                        .unwrap_or(expr_ty);
                    // Concrete Incan receivers use the compiler-proved recoverable projection even when semantic
                    // resolution found the method through a trait. Bare generic and trait-object receivers keep the
                    // Rust ABI slot because no inherent owner is statically nameable there.
                    let (emitted_method_name, dispatch) =
                        self.project_resolved_method_target(expr_span, &method_name, &receiver, dispatch);
                    (
                        IrExprKind::MethodCall {
                            receiver: Box::new(receiver),
                            method: emitted_method_name,
                            dispatch,
                            type_args: lowered_type_args,
                            args: args_ir,
                            callable_signature,
                            arg_policy,
                        },
                        expr_ty,
                    )
                }
            }

            // ---- Index access ----
            ast::Expr::Index(o, i) => {
                let obj = self.lower_expr_spanned(o)?;
                let idx = self.lower_expr_spanned(i)?;
                if let Some(resolved_operator) = self
                    .type_info
                    .as_ref()
                    .and_then(|info| info.resolved_operator_call(expr_span).cloned())
                    && resolved_operator.kind == ResolvedOperatorKind::Index
                {
                    let dispatch = self
                        .type_info
                        .as_ref()
                        .and_then(|info| info.resolved_method_call(expr_span).cloned())
                        .map(|resolved| self.lower_resolved_method_dispatch(resolved.dispatch, &obj));
                    let result_ty = self
                        .type_info
                        .as_ref()
                        .and_then(|info| info.expr_type(expr_span))
                        .map(|ty| self.lower_resolved_type(ty))
                        .unwrap_or(IrType::Unknown);
                    let (method, dispatch) =
                        self.project_resolved_method_target(expr_span, &resolved_operator.method, &obj, dispatch);
                    (
                        IrExprKind::MethodCall {
                            receiver: Box::new(obj),
                            method,
                            dispatch,
                            type_args: Vec::new(),
                            args: vec![IrCallArg {
                                name: None,
                                kind: IrCallArgKind::Positional,
                                expr: idx,
                            }],
                            callable_signature: self.callable_signature_for_call_span(expr_span),
                            arg_policy: MethodCallArgPolicy::Default,
                        },
                        result_ty,
                    )
                } else if let IrType::Tuple(items) = &obj.ty {
                    let index = Self::extract_int_literal(i)
                        .and_then(|raw| {
                            let len = i64::try_from(items.len()).ok()?;
                            let normalized = if raw < 0 { len.checked_add(raw)? } else { raw };
                            usize::try_from(normalized).ok()
                        })
                        .filter(|index| *index < items.len())
                        .ok_or_else(|| LoweringError {
                            message: "typechecked tuple index was not a statically resolved field".to_string(),
                            span: super::super::IrSpan::default(),
                        })?;
                    let elem_ty = items.get(index).cloned().ok_or_else(|| LoweringError {
                        message: "typechecked tuple index did not resolve to a tuple field".to_string(),
                        span: super::super::IrSpan::default(),
                    })?;
                    (
                        IrExprKind::Field {
                            object: Box::new(obj),
                            field: index.to_string(),
                        },
                        elem_ty,
                    )
                } else {
                    let elem_ty = match &obj.ty {
                        IrType::List(e) => (**e).clone(),
                        IrType::Dict(_, v) => (**v).clone(),
                        IrType::String => IrType::String,
                        _ => IrType::Unknown,
                    };
                    (
                        IrExprKind::Index {
                            object: Box::new(obj),
                            index: Box::new(idx),
                        },
                        elem_ty,
                    )
                }
            }

            // ---- Field access ----
            ast::Expr::Field(o, f) => {
                if let Some(value) = self
                    .type_info
                    .as_ref()
                    .and_then(|info| info.c_abi.enum_value_for_access(expr_span))
                {
                    return Ok(TypedExpr::new(IrExprKind::Int(value), IrType::Int));
                }
                if let ast::Expr::Ident(type_name) = &o.node
                    && f.starts_with("__incan_original_")
                {
                    return Ok(TypedExpr::new(
                        IrExprKind::AssociatedFunction {
                            type_name: type_name.clone(),
                            function_name: f.clone(),
                        },
                        IrType::Unknown,
                    ));
                }
                // Prefer spanned lowering so typechecker output can drive the receiver type.
                // This is important for RFC 021 alias-aware field access, especially for `self.<alias>`.
                let obj = self.lower_expr_spanned(o)?;
                if let Some(access) = self
                    .type_info
                    .as_ref()
                    .and_then(|info| info.computed_property_access(expr_span))
                {
                    let result_ty = self
                        .type_info
                        .as_ref()
                        .and_then(|info| info.expr_type(expr_span))
                        .map(|ty| self.lower_resolved_type(ty))
                        .unwrap_or(IrType::Unknown);
                    let property_name = if matches!(
                        &obj.ty,
                        IrType::Struct(_) | IrType::Enum(_) | IrType::NamedGeneric(_, _) | IrType::SelfType
                    ) {
                        self.project_resolved_method_target(expr_span, &access.property, &obj, None)
                            .0
                    } else {
                        access.property.clone()
                    };
                    (
                        IrExprKind::MethodCall {
                            receiver: Box::new(obj),
                            method: property_name,
                            type_args: Vec::new(),
                            args: Vec::new(),
                            dispatch: None,
                            callable_signature: None,
                            arg_policy: MethodCallArgPolicy::Default,
                        },
                        result_ty,
                    )
                } else {
                    if let Some(rust_field) = self
                        .type_info
                        .as_ref()
                        .and_then(|info| info.rust_field_access_name(expr_span))
                    {
                        return Ok(TypedExpr::new(
                            IrExprKind::Field {
                                object: Box::new(obj),
                                field: rust_field.to_string(),
                            },
                            IrType::Unknown,
                        ));
                    }
                    // RFC 021: resolve field alias to canonical name if object is a known struct type
                    let struct_name = obj.ty.nominal_type_name().or_else(|| match &obj.kind {
                        IrExprKind::Var { name, .. } if name == "self" => self.current_impl_type.as_deref(),
                        _ => None,
                    });
                    let field = match struct_name {
                        Some(struct_name) => self.resolve_field_alias(struct_name, f),
                        None => f.clone(),
                    };
                    (
                        IrExprKind::Field {
                            object: Box::new(obj),
                            field,
                        },
                        IrType::Unknown,
                    )
                }
            }

            // ---- Surface expressions (routed through semantics registry) ----
            ast::Expr::Surface(surface_expr) => {
                use crate::semantics_registry::semantics_registry;

                let action = semantics_registry()
                    .lower_surface_expr_action(&surface_expr.key)
                    .ok_or_else(|| LoweringError {
                        message: format!(
                            "no lowering action registered for surface expression {:?}",
                            surface_expr.key
                        ),
                        span: super::super::IrSpan::default(),
                    })?;

                match (action, &surface_expr.payload) {
                    (SurfaceExprLoweringAction::Await, ast::SurfaceExprPayload::PrefixUnary(inner)) => {
                        // Preserve explicit grouping: `await (x?)` should keep the grouped `Try` operand shape
                        // instead of applying await/try normalization for the unparenthesized `await x()?` case.
                        let parenthesized_operand = matches!(&inner.node, ast::Expr::Paren(_));
                        let lowered_inner = self.lower_awaitable_operand(inner)?;
                        if parenthesized_operand {
                            let ty = lowered_inner.ty.clone();
                            (IrExprKind::Await(Box::new(lowered_inner)), ty)
                        } else {
                            super::super::surface_semantics::lower_await_expression(lowered_inner)
                        }
                    }
                    (SurfaceExprLoweringAction::RaceFor, ast::SurfaceExprPayload::RaceFor(race)) => {
                        let lowered = self.lower_race_for_expr(race, expr_span)?;
                        (lowered.kind, lowered.ty)
                    }
                    _ => {
                        return Err(LoweringError {
                            message: format!(
                                "surface expression {:?} has an unsupported payload for lowering",
                                surface_expr.key
                            ),
                            span: super::super::IrSpan::default(),
                        });
                    }
                }
            }

            ast::Expr::VocabBlock(block) => {
                return Err(LoweringError {
                    message: format!(
                        "vocab expression declaration `{}` reached lowering before desugaring",
                        block.keyword
                    ),
                    span: super::super::IrSpan::default(),
                });
            }

            // ---- Descriptor-gated embedded fragment (RFC 081, #1023) ----
            //
            // Unlike `Surface`/`VocabBlock` above, this node is *expected* to reach lowering as itself — its
            // expression holes must lower like any other Incan expression. See `IrExprKind::EmbeddedFragment`'s
            // rustdoc for why the DSL-owned structural content does not get a mirrored IR tree here.
            ast::Expr::Embedded(fragment) => {
                let mut holes = Vec::new();
                for node in &fragment.nodes {
                    self.collect_lowered_embedded_holes(node, &mut holes)?;
                }
                (
                    IrExprKind::EmbeddedFragment {
                        submode: fragment.submode,
                        source_text: fragment.source_text.clone(),
                        holes,
                    },
                    IrType::Unknown,
                )
            }

            // ---- Try (?) ----
            ast::Expr::Try(e) => {
                let inner = self.lower_expr_spanned(e)?;
                let ty = match &inner.ty {
                    IrType::Result(ok, _) => (**ok).clone(),
                    _ => inner.ty.clone(),
                };
                (IrExprKind::Try(Box::new(inner)), ty)
            }

            // ---- Match expressions (delegated to patterns submodule) ----
            ast::Expr::Match(s, arms) => {
                let scrutinee = self.lower_expr_spanned(s)?;
                let arms_ir = self.lower_match_arms(arms, &scrutinee)?;
                let ty = arms_ir.first().map(|a| a.body.ty.clone()).unwrap_or(IrType::Unknown);
                (
                    IrExprKind::Match {
                        scrutinee: Box::new(scrutinee),
                        arms: arms_ir,
                    },
                    ty,
                )
            }

            // ---- If expressions ----
            ast::Expr::If(i) => {
                let cond = self.lower_condition_expr(&i.condition)?;
                let then_stmts = self.lower_statements(&i.then_body)?;
                let then_expr = TypedExpr::new(
                    IrExprKind::Block {
                        stmts: then_stmts,
                        value: None,
                    },
                    IrType::Unit,
                );
                let else_expr = i
                    .else_body
                    .as_ref()
                    .map(|b| {
                        self.lower_statements(b)
                            .map(|stmts| TypedExpr::new(IrExprKind::Block { stmts, value: None }, IrType::Unit))
                    })
                    .transpose()?;
                (
                    IrExprKind::If {
                        condition: Box::new(cond),
                        then_branch: Box::new(then_expr),
                        else_branch: else_expr.map(Box::new),
                    },
                    IrType::Unit,
                )
            }

            ast::Expr::Loop(loop_expr) => {
                self.push_scope();
                self.non_linear_context_depth += 1;
                let body_result = self.lower_statements(&loop_expr.body);
                self.non_linear_context_depth -= 1;
                let body = body_result?;
                self.pop_scope();
                (IrExprKind::Loop { body }, IrType::Unknown)
            }

            // ---- Closures ----
            ast::Expr::Closure(params, body) => {
                let recorded_param_types = self
                    .type_info
                    .as_ref()
                    .and_then(|info| match info.expr_type(expr_span) {
                        Some(crate::frontend::symbols::ResolvedType::Function(callable_params, _)) => Some(
                            callable_params
                                .iter()
                                .map(|param| self.lower_resolved_type(&param.ty))
                                .collect::<Vec<_>>(),
                        ),
                        _ => None,
                    });
                let exact_rust_param_types = self
                    .type_info
                    .as_ref()
                    .and_then(|info| info.closure_param_type_displays(expr_span))
                    .filter(|displays| displays.len() == params.len())
                    .map(|displays| {
                        displays
                            .iter()
                            .map(|display| IrType::RustDisplay(display.clone()))
                            .collect::<Vec<_>>()
                    });
                let param_pairs: Vec<(String, IrType)> = params
                    .iter()
                    .enumerate()
                    .map(|(idx, p)| {
                        let ty = exact_rust_param_types
                            .as_ref()
                            .and_then(|types| types.get(idx).cloned())
                            .or_else(|| recorded_param_types.as_ref().and_then(|types| types.get(idx).cloned()))
                            .unwrap_or_else(|| self.lower_type(&p.node.ty.node));
                        (p.node.name.clone(), ty)
                    })
                    .collect();
                let mut closure_read_counts = HashMap::new();
                self.count_expr_ident_reads(&body.node, &mut closure_read_counts);
                self.remaining_ident_reads.push(closure_read_counts);
                self.non_linear_context_depth += 1;
                self.push_scope();
                for (name, ty) in &param_pairs {
                    self.define_local_binding(name.clone(), ty.clone(), false);
                }
                self.closure_param_scopes.push((
                    self.non_linear_context_depth,
                    param_pairs.iter().map(|(name, _)| name.clone()).collect(),
                ));
                let body_ir_result = self.lower_expr_spanned(body);
                let _ = self.closure_param_scopes.pop();
                self.pop_scope();
                self.non_linear_context_depth -= 1;
                let _ = self.remaining_ident_reads.pop();
                let body_ir = body_ir_result?;
                let ret_ty = body_ir.ty.clone();
                let param_tys: Vec<IrType> = param_pairs.iter().map(|(_, t)| t.clone()).collect();
                let annotate_param_types = self
                    .type_info
                    .as_ref()
                    .is_some_and(|info| info.is_source_callable_closure(expr_span));
                (
                    IrExprKind::Closure {
                        params: param_pairs,
                        body: Box::new(body_ir),
                        captures: vec![],
                        annotate_param_types,
                    },
                    IrType::Function {
                        params: param_tys,
                        ret: Box::new(ret_ty),
                    },
                )
            }

            // ---- Collection literals ----
            ast::Expr::Tuple(items) => {
                let items_ir: Vec<TypedExpr> = items
                    .iter()
                    .map(|i| self.lower_expr_spanned(i))
                    .collect::<Result<_, _>>()?;
                let tys: Vec<IrType> = items_ir.iter().map(|i| i.ty.clone()).collect();
                (IrExprKind::Tuple(items_ir), IrType::Tuple(tys))
            }

            ast::Expr::List(items) => {
                let items_ir: Vec<IrListEntry> = items
                    .iter()
                    .map(|i| match i {
                        ast::ListEntry::Element(value) => self.lower_expr_spanned(value).map(IrListEntry::Element),
                        ast::ListEntry::Spread(value) => self.lower_expr_spanned(value).map(IrListEntry::Spread),
                    })
                    .collect::<Result<_, _>>()?;
                let elem = items_ir
                    .iter()
                    .find_map(|entry| match entry {
                        IrListEntry::Element(value) => Some(value.ty.clone()),
                        IrListEntry::Spread(value) => Self::lowered_list_spread_element_type(&value.ty),
                    })
                    .unwrap_or(IrType::Unknown);
                (IrExprKind::List(items_ir), IrType::List(Box::new(elem)))
            }

            ast::Expr::Dict(pairs) => {
                let pairs_ir: Vec<IrDictEntry> = pairs
                    .iter()
                    .map(|entry| match entry {
                        ast::DictEntry::Pair(k, v) => Ok(IrDictEntry::Pair(
                            self.lower_expr_spanned(k)?,
                            Box::new(self.lower_expr_spanned(v)?),
                        )),
                        ast::DictEntry::Spread(value) => self.lower_expr_spanned(value).map(IrDictEntry::Spread),
                    })
                    .collect::<Result<_, LoweringError>>()?;
                let (k, v) = pairs_ir
                    .iter()
                    .find_map(|entry| match entry {
                        IrDictEntry::Pair(key, value) => Some((key.ty.clone(), value.ty.clone())),
                        IrDictEntry::Spread(value) => Self::lowered_dict_spread_entry_types(&value.ty),
                    })
                    .unwrap_or((IrType::Unknown, IrType::Unknown));
                (IrExprKind::Dict(pairs_ir), IrType::Dict(Box::new(k), Box::new(v)))
            }

            ast::Expr::Set(items) => {
                let items_ir: Vec<TypedExpr> = items
                    .iter()
                    .map(|i| self.lower_expr_spanned(i))
                    .collect::<Result<_, _>>()?;
                let elem = items_ir.first().map(|i| i.ty.clone()).unwrap_or(IrType::Unknown);
                (IrExprKind::Set(items_ir), IrType::Set(Box::new(elem)))
            }

            // ---- Parenthesized expression (transparent) ----
            ast::Expr::Paren(e) => return self.lower_expr_spanned(e),

            // ---- Constructor (variant / struct literal) ----
            ast::Expr::Constructor(name, args) => {
                let fields: Vec<(String, TypedExpr)> = args
                    .iter()
                    .map(|arg| match arg {
                        ast::CallArg::Named(n, e) => Ok((n.node.clone(), self.lower_expr_spanned(e)?)),
                        ast::CallArg::Positional(e)
                        | ast::CallArg::PositionalUnpack(e)
                        | ast::CallArg::KeywordUnpack(e) => Ok((String::new(), self.lower_expr_spanned(e)?)),
                    })
                    .collect::<Result<_, LoweringError>>()?;
                (
                    IrExprKind::Struct {
                        name: name.clone(),
                        fields,
                        fill_defaults: false,
                    },
                    IrType::Struct(name.clone()),
                )
            }

            // ---- Range expressions ----
            ast::Expr::Range { start, end, inclusive } => {
                let s = self.lower_expr_spanned(start)?;
                let e = self.lower_expr_spanned(end)?;
                (
                    IrExprKind::Range {
                        start: Some(Box::new(s)),
                        end: Some(Box::new(e)),
                        inclusive: *inclusive,
                    },
                    IrType::Unknown,
                )
            }

            // ---- F-strings ----
            ast::Expr::FString(parts) => {
                let ir_parts: Vec<super::super::expr::FormatPart> = parts
                    .iter()
                    .map(|part| match part {
                        ast::FStringPart::Literal(s) => Ok(super::super::expr::FormatPart::Literal(s.clone())),
                        ast::FStringPart::Expr { expr, format } => {
                            let lowered = self.lower_expr_spanned(expr)?;
                            let style = match format {
                                ast::FStringFormat::Display => super::super::expr::FormatStyle::Display,
                                ast::FStringFormat::Debug => super::super::expr::FormatStyle::Debug,
                            };
                            Ok(super::super::expr::FormatPart::Expr { expr: lowered, style })
                        }
                    })
                    .collect::<Result<Vec<_>, LoweringError>>()?;
                (IrExprKind::Format { parts: ir_parts }, IrType::String)
            }

            // ---- Slice expressions ----
            ast::Expr::Slice(target, slice) => {
                let target_expr = self.lower_expr_spanned(target)?;
                let start = slice
                    .start
                    .as_ref()
                    .map(|s| Ok(Box::new(self.lower_expr_spanned(s)?)))
                    .transpose()?;
                let end = slice
                    .end
                    .as_ref()
                    .map(|e| Ok(Box::new(self.lower_expr_spanned(e)?)))
                    .transpose()?;
                let step = slice
                    .step
                    .as_ref()
                    .map(|st| Ok(Box::new(self.lower_expr_spanned(st)?)))
                    .transpose()?;

                let result_ty = match &target_expr.ty {
                    IrType::List(inner) => IrType::List(inner.clone()),
                    IrType::String => IrType::String,
                    _ => IrType::Unknown,
                };

                (
                    IrExprKind::Slice {
                        target: Box::new(target_expr),
                        start,
                        end,
                        step,
                    },
                    result_ty,
                )
            }

            // ---- Comprehensions (delegated to comprehensions submodule) ----
            ast::Expr::Generator(generator) => self.lower_generator_expr(generator)?,
            ast::Expr::ListComp(comp) => self.lower_list_comp(comp)?,
            ast::Expr::DictComp(comp) => self.lower_dict_comp(comp)?,

            // ---- Yield (placeholder) ----
            ast::Expr::Yield(_) => (IrExprKind::Unit, IrType::Unknown),
            ast::Expr::Partial(partial) => {
                let Some(crate::frontend::symbols::ResolvedType::Function(target_params, _)) = self
                    .type_info
                    .as_ref()
                    .and_then(|info| info.expr_type(partial.target.span).cloned())
                else {
                    return Err(LoweringError {
                        message: "Partial callable target is missing typechecker signature metadata".to_string(),
                        span: partial.target.span.into(),
                    });
                };
                let signature = self
                    .partial_expr_callable_signature(partial, expr_span)?
                    .ok_or_else(|| LoweringError {
                        message: "Partial callable preset expression is missing typechecker projection metadata"
                            .to_string(),
                        span: expr_span.into(),
                    })?;
                let target = self.lower_expr_spanned(&partial.target)?;

                // Evaluate every preset exactly once before the closure is constructed. The generated closure is
                // `move`, so a later mutation of the source local cannot change an omitted preset argument.
                let mut capture_stmts = Vec::with_capacity(partial.args.len());
                let mut captures = HashMap::with_capacity(partial.args.len());
                let mut capture_names = Vec::with_capacity(partial.args.len());
                for (index, preset) in partial.args.iter().enumerate() {
                    let value = self.lower_expr_spanned(&preset.value)?;
                    let ty = value.ty.clone();
                    let capture_name = format!("__incan_partial_preset_{index}_{}", preset.name);
                    capture_stmts.push(IrStmt::new(IrStmtKind::Let {
                        name: capture_name.clone(),
                        ty: ty.clone(),
                        type_annotation: None,
                        mutability: Mutability::Immutable,
                        value,
                    }));
                    captures.insert(preset.name.clone(), (capture_name.clone(), ty));
                    capture_names.push(capture_name);
                }

                let closure_params: Vec<(String, IrType)> = signature
                    .params
                    .iter()
                    .map(|param| (param.name.clone(), param.ty.clone()))
                    .collect();
                let mut forward_args = Vec::with_capacity(target_params.len());
                for (idx, target_param) in target_params.iter().enumerate() {
                    let Some(name) = target_param.name.as_ref() else {
                        return Err(LoweringError {
                            message: format!(
                                "Partial callable target has unsupported anonymous parameter at index {idx}"
                            ),
                            span: partial.target.span.into(),
                        });
                    };
                    let target_ty =
                        Self::lower_param_container_type(target_param.kind, self.lower_resolved_type(&target_param.ty));
                    let parameter = signature.params.get(idx).ok_or_else(|| LoweringError {
                        message: format!(
                            "Partial callable target parameter '{name}' is absent from its callable signature"
                        ),
                        span: partial.target.span.into(),
                    })?;
                    let value = if matches!(
                        parameter.default.as_ref(),
                        Some(FunctionParamDefault::CapturedPartialPreset)
                    ) {
                        let (capture_name, capture_ty) = captures.get(name).ok_or_else(|| LoweringError {
                            message: format!("Partial callable preset '{name}' has no construction-time capture"),
                            span: expr_span.into(),
                        })?;
                        let fallback = TypedExpr::new(
                            IrExprKind::Closure {
                                params: Vec::new(),
                                body: Box::new(TypedExpr::new(
                                    IrExprKind::MethodCall {
                                        receiver: Box::new(TypedExpr::new(
                                            IrExprKind::Var {
                                                name: capture_name.clone(),
                                                access: VarAccess::Read,
                                                ref_kind: VarRefKind::Value,
                                            },
                                            capture_ty.clone(),
                                        )),
                                        method: "clone".to_string(),
                                        dispatch: None,
                                        type_args: Vec::new(),
                                        args: Vec::new(),
                                        callable_signature: None,
                                        arg_policy: MethodCallArgPolicy::Default,
                                    },
                                    capture_ty.clone(),
                                )),
                                captures: Vec::new(),
                                annotate_param_types: false,
                            },
                            IrType::Function {
                                params: Vec::new(),
                                ret: Box::new(capture_ty.clone()),
                            },
                        );
                        TypedExpr::new(
                            IrExprKind::MethodCall {
                                receiver: Box::new(TypedExpr::new(
                                    IrExprKind::Var {
                                        name: name.clone(),
                                        access: VarAccess::Read,
                                        ref_kind: VarRefKind::Value,
                                    },
                                    parameter.ty.clone(),
                                )),
                                method: "unwrap_or_else".to_string(),
                                dispatch: None,
                                type_args: Vec::new(),
                                args: vec![IrCallArg {
                                    name: None,
                                    kind: IrCallArgKind::Positional,
                                    expr: fallback,
                                }],
                                callable_signature: None,
                                arg_policy: MethodCallArgPolicy::Default,
                            },
                            target_ty,
                        )
                    } else {
                        TypedExpr::new(
                            IrExprKind::Var {
                                name: name.clone(),
                                access: VarAccess::Read,
                                ref_kind: VarRefKind::Value,
                            },
                            target_ty,
                        )
                    };
                    forward_args.push(IrCallArg {
                        name: Some(name.clone()),
                        kind: IrCallArgKind::Named,
                        expr: value,
                    });
                }
                let body = TypedExpr::new(
                    IrExprKind::Call {
                        func: Box::new(target),
                        type_args: self.lower_call_site_type_args(expr_span, &partial.type_args),
                        args: forward_args,
                        callable_signature: None,
                        canonical_path: None,
                    },
                    signature.return_type.clone(),
                );
                let closure = TypedExpr::new(
                    IrExprKind::Closure {
                        params: closure_params.clone(),
                        body: Box::new(body),
                        captures: capture_names,
                        // A local partial has no surrounding Rust callable type to infer its parameters. Emit their
                        // source-checked IR types, including `Option<T>` for overrideable preset slots.
                        annotate_param_types: true,
                    },
                    IrType::Function {
                        params: closure_params.into_iter().map(|(_, ty)| ty).collect(),
                        ret: Box::new(signature.return_type.clone()),
                    },
                );
                (
                    IrExprKind::Block {
                        stmts: capture_stmts,
                        value: Some(Box::new(closure)),
                    },
                    IrType::Function {
                        params: signature.params.into_iter().map(|param| param.ty).collect(),
                        ret: Box::new(signature.return_type),
                    },
                )
            }
        };
        Ok(TypedExpr::new(kind, ty))
    }

    /// Recursively lower every expression hole nested inside one embedded-fragment node, appending each lowered
    /// hole to `holes` in source order.
    ///
    /// Mirrors `TypeChecker::check_embedded_fragment_node_holes`'s traversal shape: structural node kinds with no
    /// possible nested hole are no-ops, `Hole` lowers via ordinary `lower_expr_spanned`, and container kinds
    /// (`Element`, `StyleRule`, `Declaration`) recurse into their children/attrs/selectors/declarations.
    fn collect_lowered_embedded_holes(
        &mut self,
        node: &Spanned<ast::EmbeddedNode>,
        holes: &mut Vec<TypedExpr>,
    ) -> Result<(), LoweringError> {
        match &node.node {
            ast::EmbeddedNode::Text(_)
            | ast::EmbeddedNode::EntityRef(_)
            | ast::EmbeddedNode::Comment(_)
            | ast::EmbeddedNode::Value(_)
            | ast::EmbeddedNode::Regex { .. }
            | ast::EmbeddedNode::TypeShape(_) => Ok(()),
            ast::EmbeddedNode::Hole(expr) => {
                holes.push(self.lower_expr_spanned(expr)?);
                Ok(())
            }
            ast::EmbeddedNode::Element(element) => {
                for attr in &element.attrs {
                    if let Some(value) = &attr.value {
                        self.collect_lowered_embedded_holes(value, holes)?;
                    }
                }
                for child in &element.children {
                    self.collect_lowered_embedded_holes(child, holes)?;
                }
                Ok(())
            }
            ast::EmbeddedNode::StyleRule(rule) => {
                for selector in &rule.selectors {
                    self.collect_lowered_embedded_holes(selector, holes)?;
                }
                for declaration in &rule.declarations {
                    self.collect_lowered_embedded_holes(declaration, holes)?;
                }
                Ok(())
            }
            ast::EmbeddedNode::Declaration(declaration) => {
                for value in &declaration.value {
                    self.collect_lowered_embedded_holes(value, holes)?;
                }
                Ok(())
            }
        }
    }
}

/// Return the IR resize policy represented by a built-in numeric resize helper name.
fn numeric_resize_policy(method: &str) -> Option<NumericResizePolicy> {
    match method {
        "resize" => Some(NumericResizePolicy::Lossless),
        "try_resize" => Some(NumericResizePolicy::Try),
        "wrapping_resize" => Some(NumericResizePolicy::Wrapping),
        "saturating_resize" => Some(NumericResizePolicy::Saturating),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trait_dispatch(source_name: &str) -> IrMethodDispatch {
        IrMethodDispatch::Trait(Box::new(IrTraitDispatch {
            trait_source_name: source_name.to_string(),
            trait_module_path: None,
            implementation_type_params: Vec::new(),
            trait_path: source_name.to_string(),
            type_args: Vec::new(),
            receiver_is_mutable: false,
        }))
    }

    /// Build a lowering whose checked facts resolve one method call to a declaration owned by `library`.
    fn lowering_resolving_a_package_method(
        library: &str,
        produced_library: Option<&str>,
        call_span: ast::Span,
    ) -> AstLowering {
        let mut type_info = crate::frontend::typechecker::TypeCheckInfo::default();
        type_info.record_resolved_identity(
            call_span,
            incan_semantics_core::CanonicalSymbolId {
                namespace: incan_semantics_core::SymbolNamespace::Member,
                origin: incan_semantics_core::SymbolOrigin::Package {
                    library: library.to_string(),
                    module_path: vec!["registry".to_string()],
                },
                declaration_name: "entry".to_string(),
                kind: incan_semantics_core::SemanticSourceTargetKind::Method,
                scope_discriminant: None,
                declaration_span: incan_semantics_core::HirSourceSpan::new(0, 0),
            },
        );
        let mut lowering = AstLowering::new_with_type_info(type_info);
        lowering.set_registry_package_identity(produced_library.map(str::to_string));
        lowering
    }

    /// A package's own inherent method keeps its projection while that package is the one being compiled.
    ///
    /// Regression for #1174: the suppression rule matched any `SymbolOrigin::Package`, so once a package's modules
    /// called into each other, the package's own declarations looked foreign to its own build. Building
    /// `incan_stdlib_core` then emitted a raw `entry` where the checked registry lowering required the projected
    /// name, and every `Registry.entry` in the component failed to lower.
    #[test]
    fn a_package_projects_its_own_inherent_method_while_building_itself() {
        let call_span = ast::Span { start: 10, end: 20 };
        let lowering = lowering_resolving_a_package_method("incan_stdlib_core", Some("incan_stdlib_core"), call_span);

        assert!(
            !lowering.method_belongs_to_an_imported_type(call_span),
            "a package's own declaration is emitted by this build, so its wrapper can be named"
        );
    }

    /// Another package's inherent method still has no wrapper this compilation can name.
    ///
    /// The other half of #1174, pinned so narrowing the rule does not restore the original defect: a consumer calling
    /// a method on a dependency's type must reach the dependency's own slot, because no wrapper is emitted here --
    /// and for a newtype over a `rust::` type none exists anywhere, since Rust forbids a foreign inherent `impl`.
    #[test]
    fn another_packages_inherent_method_is_still_not_projected() {
        let call_span = ast::Span { start: 10, end: 20 };
        let consumer = lowering_resolving_a_package_method("incan_stdlib_core", Some("my_app"), call_span);
        assert!(
            consumer.method_belongs_to_an_imported_type(call_span),
            "a dependency's inherent method has no wrapper in the consumer's compilation"
        );

        // No project owns an ad-hoc single-file build, and every package identity is then genuinely foreign.
        let unowned = lowering_resolving_a_package_method("incan_stdlib_core", None, call_span);
        assert!(
            unowned.method_belongs_to_an_imported_type(call_span),
            "without a produced library every package identity stays foreign"
        );
    }

    /// A dispatched call to another package's method is suppressed too, on the declaring package's terms.
    ///
    /// This rule used to exempt trait dispatch, leaving those cases to `can_use_source_method_projection`. That check
    /// never asks which library declares the method, so `BytesIO(..).read(..)` dispatching through `BinaryRead`
    /// passed everything and projected a name only `incan_stdlib_system` emits. Declining keeps the fully-qualified
    /// trait call, which is also what pins the width that `Result[u8, IoError]` would otherwise only infer.
    #[test]
    fn a_dispatched_call_to_another_packages_method_is_not_projected() {
        let call_span = ast::Span { start: 10, end: 20 };
        let lowering = lowering_resolving_a_package_method("incan_stdlib_core", Some("my_app"), call_span);

        assert!(
            lowering.method_belongs_to_an_imported_type(call_span),
            "a dependency's method has no wrapper here, dispatched or not"
        );
    }

    #[test]
    fn generic_receiver_keeps_trait_abi_dispatch() {
        let receiver = TypedExpr::new(IrExprKind::Unit, IrType::Generic("R".to_string()));

        assert!(!can_use_source_method_projection(
            &receiver,
            Some(&trait_dispatch("FallibleIterator"))
        ));
    }

    #[test]
    fn nominal_generic_receiver_can_use_source_projection() {
        let receiver = TypedExpr::new(
            IrExprKind::Unit,
            IrType::NamedGeneric("ReaderChunks".to_string(), vec![IrType::Generic("R".to_string())]),
        );

        assert!(can_use_source_method_projection(
            &receiver,
            Some(&trait_dispatch("FallibleIterator"))
        ));
    }

    #[test]
    fn rust_native_trait_keeps_abi_dispatch_for_concrete_receiver() {
        let receiver = TypedExpr::new(IrExprKind::Unit, IrType::Struct("OrderedDict".to_string()));

        assert!(!can_use_source_method_projection(
            &receiver,
            Some(&trait_dispatch(builtin_traits::as_str(TraitId::Clone)))
        ));
    }

    #[test]
    fn a_receiver_typed_as_a_supertrait_adopter_keeps_the_trait_slot_spelling() {
        // `take_sorted(..)` returns `OrderedCollection[int]`, and calling `.first()` on it dispatches `Collection`.
        // The receiver is still a trait, so it names no implementation even though the two trait names differ.
        let mut lowering = AstLowering::new();
        lowering.declared_trait_names.insert("Collection".to_string());
        lowering.declared_trait_names.insert("OrderedCollection".to_string());
        let receiver = TypedExpr::new(
            IrExprKind::Unit,
            IrType::NamedGeneric("OrderedCollection".to_string(), vec![IrType::Int]),
        );

        assert!(
            !lowering.receiver_adopts_the_dispatched_trait(&receiver, Some(&trait_dispatch("Collection"))),
            "a supertrait-typed receiver still names no implementation"
        );
    }

    #[test]
    fn a_receiver_typed_as_the_dispatched_trait_keeps_the_trait_slot_spelling() {
        // `display[T](data: DataSet[T])` calls `data.to_substrait_plan()`. The trait's own declaration is an abstract
        // slot with no wrapper beside it -- only adopting types emit one -- so projecting named the trait
        // declaration's span while every emitted wrapper carries an implementation's.
        let lowering = AstLowering::new();
        let receiver = TypedExpr::new(
            IrExprKind::Unit,
            IrType::NamedGeneric("DataSet".to_string(), vec![IrType::Int]),
        );

        assert!(
            !lowering.receiver_adopts_the_dispatched_trait(&receiver, Some(&trait_dispatch("DataSet"))),
            "a value typed as the trait itself names no implementation to project"
        );
    }

    #[test]
    fn concrete_source_trait_can_use_recoverable_projection() {
        let receiver = TypedExpr::new(IrExprKind::Unit, IrType::Struct("Item".to_string()));

        assert!(can_use_source_method_projection(
            &receiver,
            Some(&trait_dispatch("Labelled"))
        ));
    }

    /// A trait inherited through a compiled package must be addressed through that linked package's canonical stdlib
    /// re-export, not through a transitive provider crate that the consumer does not link directly.
    #[test]
    fn imported_method_chain_dispatches_trait_through_public_library() {
        let lowering = AstLowering::new();
        let package_call = TypedExpr::new(
            IrExprKind::Call {
                func: Box::new(TypedExpr::new(IrExprKind::Unit, IrType::Unknown)),
                type_args: Vec::new(),
                args: Vec::new(),
                callable_signature: None,
                canonical_path: Some(vec![
                    "pub".to_string(),
                    "fallible_streams".to_string(),
                    "numbers".to_string(),
                ]),
            },
            IrType::Struct("fallible_streams::NumberStream".to_string()),
        );
        let method_chain = TypedExpr::new(
            IrExprKind::MethodCall {
                receiver: Box::new(package_call),
                method: "map".to_string(),
                dispatch: None,
                type_args: Vec::new(),
                args: Vec::new(),
                callable_signature: None,
                arg_policy: MethodCallArgPolicy::Default,
            },
            IrType::Unknown,
        );
        let dispatch = lowering.lower_resolved_method_dispatch(
            ResolvedMethodDispatch::Trait {
                trait_name: "FallibleIterator".to_string(),
                module_path: Some(vec!["std".to_string(), "derives".to_string(), "collection".to_string()]),
                type_args: vec![
                    crate::frontend::symbols::ResolvedType::Int,
                    crate::frontend::symbols::ResolvedType::Str,
                ],
                implementation_type_params: Vec::new(),
                receiver_is_mutable: false,
            },
            &method_chain,
        );

        assert!(matches!(
            dispatch,
            IrMethodDispatch::Trait(dispatch)
                if dispatch.trait_path
                    == "fallible_streams::__incan_std::derives::collection::FallibleIterator"
        ));
    }

    /// Compiled provider source must use its crate-local compatibility facade rather than a sibling provider crate that
    /// may be reachable only transitively through the selected component graph.
    #[test]
    fn sdk_provider_dispatches_std_trait_through_local_facade() {
        let mut lowering = AstLowering::new();
        lowering.set_sdk_provider_build(true);
        let receiver = TypedExpr::new(IrExprKind::Unit, IrType::Unknown);
        let dispatch = lowering.lower_resolved_method_dispatch(
            ResolvedMethodDispatch::Trait {
                trait_name: "FallibleIterator".to_string(),
                module_path: Some(vec!["std".to_string(), "derives".to_string(), "collection".to_string()]),
                type_args: vec![
                    crate::frontend::symbols::ResolvedType::Int,
                    crate::frontend::symbols::ResolvedType::Str,
                ],
                implementation_type_params: Vec::new(),
                receiver_is_mutable: false,
            },
            &receiver,
        );

        assert!(matches!(
            dispatch,
            IrMethodDispatch::Trait(dispatch)
                if dispatch.trait_path == "crate::__incan_std::derives::collection::FallibleIterator"
        ));
    }

    /// Rust-native traits keep their backend mapping even when semantic resolution records a stdlib source owner.
    #[test]
    fn rust_native_trait_dispatch_keeps_native_method_lookup() {
        let lowering = AstLowering::new();
        let receiver = TypedExpr::new(IrExprKind::Unit, IrType::Unknown);
        let clone_trait = builtin_traits::as_str(TraitId::Clone);
        assert_eq!(trait_bounds::incan_to_rust(clone_trait), Some(clone_trait));
        let dispatch = lowering.lower_resolved_method_dispatch(
            ResolvedMethodDispatch::Trait {
                trait_name: clone_trait.to_string(),
                module_path: Some(vec!["std".to_string(), "derives".to_string(), "copying".to_string()]),
                type_args: Vec::new(),
                implementation_type_params: Vec::new(),
                receiver_is_mutable: false,
            },
            &receiver,
        );

        assert!(matches!(
            dispatch,
            IrMethodDispatch::Trait(dispatch) if dispatch.trait_path == clone_trait
        ));
    }

    /// A source-owned JSON protocol shadows the same-named Rust serde derive when semantic resolution records its
    /// canonical stdlib owner.
    #[test]
    fn stdlib_json_trait_owner_precedes_native_serde_mapping() {
        let lowering = AstLowering::new();
        let receiver = TypedExpr::new(IrExprKind::Unit, IrType::Unknown);
        for trait_name in ["Serialize", "json.Serialize"] {
            let dispatch = lowering.lower_resolved_method_dispatch(
                ResolvedMethodDispatch::Trait {
                    trait_name: trait_name.to_string(),
                    module_path: Some(vec!["std".to_string(), "serde".to_string(), "json".to_string()]),
                    type_args: Vec::new(),
                    implementation_type_params: Vec::new(),
                    receiver_is_mutable: false,
                },
                &receiver,
            );

            assert!(matches!(
                dispatch,
                IrMethodDispatch::Trait(dispatch)
                    if dispatch.trait_path == "crate::__incan_std::serde::json::Serialize"
            ));
        }
    }

    /// Imported source JSON traits can lose their module hint after adoption, but their protocol methods must still
    /// dispatch through the imported source binding rather than Rust serde's derive trait.
    #[test]
    fn unqualified_stdlib_json_trait_precedes_native_serde_mapping() {
        let lowering = AstLowering::new();
        let receiver = TypedExpr::new(IrExprKind::Unit, IrType::Unknown);
        let dispatch = lowering.lower_resolved_method_dispatch(
            ResolvedMethodDispatch::Trait {
                trait_name: "Serialize".to_string(),
                module_path: None,
                type_args: Vec::new(),
                implementation_type_params: Vec::new(),
                receiver_is_mutable: false,
            },
            &receiver,
        );

        assert!(matches!(
            dispatch,
            IrMethodDispatch::Trait(dispatch) if dispatch.trait_path == "Serialize"
        ));
    }

    /// Qualified stdlib JSON protocols retain source method lookup rather than being mistaken for Rust serde traits.
    #[test]
    fn stdlib_json_trait_dispatch_keeps_source_protocol_lookup() {
        let lowering = AstLowering::new();
        let receiver = TypedExpr::new(IrExprKind::Unit, IrType::Unknown);
        let dispatch = lowering.lower_resolved_method_dispatch(
            ResolvedMethodDispatch::Trait {
                trait_name: "json.Serialize".to_string(),
                module_path: None,
                type_args: Vec::new(),
                implementation_type_params: Vec::new(),
                receiver_is_mutable: false,
            },
            &receiver,
        );

        assert!(matches!(
            dispatch,
            IrMethodDispatch::Trait(dispatch) if dispatch.trait_path == "json::Serialize"
        ));
    }
}
