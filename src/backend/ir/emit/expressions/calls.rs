//! Emit Rust code for function calls and binary operations.
//!
//! This module handles emission of regular function calls (user-defined functions) and binary operator expressions.

mod testing_asserts;

use proc_macro2::TokenStream;
use quote::quote;

use super::super::super::conversions::{BinOpEmitKind, determine_binop_plan};
use super::super::super::decl::FunctionParam;
use super::super::super::expr::{BinOp, IrCallArg, IrCallArgKind, IrExprKind, TypedExpr, VarRefKind};
use super::super::super::ownership::{ArgumentPassingPlan, ValueUseSite};
use super::super::super::types::IrType;
use super::super::super::{FunctionRegistry, FunctionSignature};
use super::super::{EmitError, IrEmitter};
use crate::frontend::ast::ParamKind;
use incan_core::lang::stdlib;
use incan_core::lang::surface::constructors::{self, ConstructorId};

const INTERNAL_PANIC_FN: &str = "__incan_internal_panic";

impl<'a> IrEmitter<'a> {
    /// Return the borrowed helper function item for a named function argument when the target parameter expects one.
    fn borrowed_function_adapter_arg(&self, arg: &TypedExpr, target_ty: Option<&IrType>) -> Option<TokenStream> {
        let IrType::Function { params, .. } = target_ty? else {
            return None;
        };
        let borrowed_indices: Vec<usize> = params
            .iter()
            .enumerate()
            .filter_map(|(idx, ty)| matches!(ty, IrType::Ref(_)).then_some(idx))
            .collect();
        if borrowed_indices.is_empty() {
            return None;
        }
        let IrExprKind::Var {
            name,
            ref_kind: VarRefKind::Value,
            ..
        } = &arg.kind
        else {
            return None;
        };
        if !matches!(arg.ty, IrType::Function { .. }) || !self.needs_borrowed_function_adapter(name, &borrowed_indices)
        {
            return None;
        }
        let helper_name = Self::borrowed_function_adapter_name(name, &borrowed_indices);
        let helper = Self::rust_ident(&helper_name);
        Some(quote! { #helper })
    }

    /// Heuristic: detect whether a type still has unresolved generic parts.
    ///
    /// This is used when seeding emitted literals (`None`, `Ok`, `Err`) with explicit Rust type arguments to help
    /// inference in generic call sites. When a type is still unresolved, callers use conservative placeholders (`_` or
    /// `()`) instead of over-constraining the generated code.
    ///
    /// ## Parameters
    /// - `ty`: Type to inspect recursively.
    ///
    /// ## Returns
    /// - (`bool`): `true` if `ty` (or any nested component) appears unresolved.
    pub(super) fn is_unresolved_type(ty: &IrType) -> bool {
        match ty {
            IrType::Unknown => true,
            IrType::Generic(_) => false,
            IrType::Ref(inner) | IrType::RefMut(inner) | IrType::Option(inner) | IrType::List(inner) => {
                Self::is_unresolved_type(inner)
            }
            IrType::Set(inner) => Self::is_unresolved_type(inner),
            IrType::Dict(k, v) | IrType::Result(k, v) => Self::is_unresolved_type(k) || Self::is_unresolved_type(v),
            IrType::Tuple(items) => items.iter().any(Self::is_unresolved_type),
            IrType::NamedGeneric(_, args) => args.iter().any(Self::is_unresolved_type),
            IrType::Function { params, ret } => {
                params.iter().any(Self::is_unresolved_type) || Self::is_unresolved_type(ret)
            }
            IrType::Struct(_) | IrType::Enum(_) | IrType::Trait(_) => false,
            _ => false,
        }
    }

    /// Stricter variant used only for call-site literal seeding.
    ///
    /// Generic placeholders coming from the callee signature (`Option[T]`, `Result[T, E]`) are not in scope at the
    /// caller, so they must still be treated as unresolved here even though they are perfectly valid inside the callee
    /// body or an enclosing generic impl/function.
    pub(super) fn is_unresolved_call_seed_type(ty: &IrType) -> bool {
        match ty {
            IrType::Unknown | IrType::Generic(_) => true,
            IrType::Ref(inner) | IrType::RefMut(inner) | IrType::Option(inner) | IrType::List(inner) => {
                Self::is_unresolved_call_seed_type(inner)
            }
            IrType::Set(inner) => Self::is_unresolved_call_seed_type(inner),
            IrType::Dict(k, v) | IrType::Result(k, v) => {
                Self::is_unresolved_call_seed_type(k) || Self::is_unresolved_call_seed_type(v)
            }
            IrType::Tuple(items) => items.iter().any(Self::is_unresolved_call_seed_type),
            IrType::NamedGeneric(_, args) => args.iter().any(Self::is_unresolved_call_seed_type),
            IrType::Function { params, ret } => {
                params.iter().any(Self::is_unresolved_call_seed_type) || Self::is_unresolved_call_seed_type(ret)
            }
            IrType::Struct(_) | IrType::Enum(_) | IrType::Trait(_) => false,
            _ => false,
        }
    }

    /// Promote string literals used as `Result` payloads to owned `String` tokens.
    ///
    /// Incan `str` values lower to owned Rust `String` in `Result[T, E]` payload positions. This helper keeps `Ok` and
    /// `Err` constructor emission aligned across the different seeding paths.
    fn emit_result_payload_tokens(inner_expr: &TypedExpr, inner_tokens: TokenStream) -> TokenStream {
        if matches!(inner_expr.kind, IrExprKind::String(_)) {
            quote! { (#inner_tokens).to_string() }
        } else {
            inner_tokens
        }
    }

    /// Return whether an argument can be wrapped directly as `Some(inner)`.
    fn option_payload_type_matches(arg_ty: &IrType, inner_ty: &IrType) -> bool {
        arg_ty == inner_ty
            || matches!(
                (inner_ty, arg_ty),
                (IrType::String, IrType::StaticStr | IrType::StrRef | IrType::FrozenStr)
            )
    }

    /// Emit a concrete payload argument for an `Option[T]` parameter as `Some(...)`.
    fn emit_option_payload_arg(
        &self,
        arg: &TypedExpr,
        inner_ty: &IrType,
        union_qualifier: Option<&[String]>,
    ) -> Result<Option<TokenStream>, EmitError> {
        let (source_ty, _) = self.union_widening_source_for_expr(arg);
        let source_ty_resolved = self.resolve_type_aliases_for_emit(&source_ty);
        let inner_ty_resolved = self.resolve_type_aliases_for_emit(inner_ty);
        if source_ty_resolved.union_members().is_some()
            && inner_ty_resolved.union_members().is_some()
            && (source_ty_resolved == inner_ty_resolved || self.union_widening_needed(&source_ty, inner_ty))
        {
            let emitted = self.emit_expr_for_use_with_union_qualifier(
                arg,
                ValueUseSite::IncanCallArg {
                    target_ty: Some(inner_ty),
                    callee_param: None,
                    in_return: false,
                },
                union_qualifier,
            )?;
            return Ok(Some(quote! { Some(#emitted) }));
        }

        if let Some(variant_index) = inner_ty.union_variant_index_for_member(&arg.ty) {
            let Some(members) = inner_ty.union_members() else {
                return Ok(None);
            };
            let Some(member_ty) = members.get(variant_index) else {
                return Ok(None);
            };
            let variant_ident = quote::format_ident!("{}", IrType::union_variant_name(variant_index));
            let union_path = self.emit_union_type_path_with_qualifier(inner_ty, union_qualifier);
            let emitted = self.emit_expr_for_use(
                arg,
                ValueUseSite::IncanCallArg {
                    target_ty: Some(member_ty),
                    callee_param: None,
                    in_return: false,
                },
            )?;
            return Ok(Some(quote! { Some(#union_path :: #variant_ident(#emitted)) }));
        }

        if Self::option_payload_type_matches(&arg.ty, inner_ty) {
            let emitted = self.emit_expr_for_use(
                arg,
                ValueUseSite::IncanCallArg {
                    target_ty: Some(inner_ty),
                    callee_param: None,
                    in_return: false,
                },
            )?;
            return Ok(Some(quote! { Some(#emitted) }));
        }

        Ok(None)
    }

    /// Emit a concrete payload argument for a `Union[...]` parameter as the generated enum variant.
    pub(super) fn emit_union_payload_arg(
        &self,
        arg: &TypedExpr,
        target_ty: &IrType,
        union_qualifier: Option<&[String]>,
    ) -> Result<Option<TokenStream>, EmitError> {
        self.emit_union_payload_arg_for_site(
            arg,
            target_ty,
            union_qualifier,
            ValueUseSite::IncanCallArg {
                target_ty: None,
                callee_param: None,
                in_return: false,
            },
        )
    }

    /// Emit a concrete payload argument for a `Union[...]` target while preserving the caller's ownership site.
    pub(super) fn emit_union_payload_arg_for_site(
        &self,
        arg: &TypedExpr,
        target_ty: &IrType,
        union_qualifier: Option<&[String]>,
        site: ValueUseSite<'_>,
    ) -> Result<Option<TokenStream>, EmitError> {
        let Some(value_ty) = self.union_payload_candidate_type(arg, target_ty) else {
            return Ok(None);
        };
        let Some(variant_index) = target_ty.union_variant_index_for_member(&value_ty) else {
            return Ok(None);
        };
        let Some(members) = target_ty.union_members() else {
            return Ok(None);
        };
        let Some(member_ty) = members.get(variant_index) else {
            return Ok(None);
        };
        let variant_ident = quote::format_ident!("{}", IrType::union_variant_name(variant_index));
        let union_path = self.emit_union_type_path_with_qualifier(target_ty, union_qualifier);
        let emitted = self.emit_expr_for_use(arg, Self::retarget_value_use_site(site, Some(member_ty)))?;
        Ok(Some(quote! { #union_path :: #variant_ident(#emitted) }))
    }

    /// Return the concrete union-member payload type for an argument that may already be typed as the target union.
    fn union_payload_candidate_type(&self, arg: &TypedExpr, target_ty: &IrType) -> Option<IrType> {
        if !arg.ty.is_union() {
            return Some(arg.ty.clone());
        }

        let candidate_name = match &arg.kind {
            IrExprKind::Struct { name, .. } => Some(name.as_str()),
            IrExprKind::Call { func, .. } => match &func.kind {
                IrExprKind::Var {
                    name,
                    ref_kind: VarRefKind::TypeName,
                    ..
                } => Some(name.as_str()),
                _ => None,
            },
            _ => None,
        }?;
        target_ty
            .union_members()?
            .iter()
            .find(|member| member.nominal_type_name() == Some(candidate_name))
            .cloned()
    }

    /// Return whether a source anonymous union can be widened into the target anonymous union.
    pub(in super::super) fn union_widening_needed(&self, source_ty: &IrType, target_ty: &IrType) -> bool {
        let source_ty = self.resolve_type_aliases_for_emit(source_ty);
        let target_ty = self.resolve_type_aliases_for_emit(target_ty);
        source_ty != target_ty
            && source_ty.union_members().is_some_and(|members| !members.is_empty())
            && target_ty.union_members().is_some_and(|members| !members.is_empty())
            && Self::union_widening_variant_map(&source_ty, &target_ty).is_some()
    }

    /// Return the semantic union type and owner qualifier that should drive wrapper widening for an expression.
    ///
    /// Imported public call results are carried as exact Rust display types so consumers do not define their own copy
    /// of provider-owned wrappers. Their callable metadata still carries the semantic union shape needed for
    /// widening.
    pub(in super::super) fn union_widening_source_for_expr(&self, expr: &TypedExpr) -> (IrType, Option<Vec<String>>) {
        match &expr.kind {
            IrExprKind::Call {
                callable_signature: Some(signature),
                canonical_path,
                ..
            } if self
                .resolve_type_aliases_for_emit(&signature.return_type)
                .union_members()
                .is_some() =>
            {
                (
                    signature.return_type.clone(),
                    Self::pub_library_union_qualifier(canonical_path.as_deref()),
                )
            }
            IrExprKind::MethodCall {
                callable_signature: Some(signature),
                ..
            } if self
                .resolve_type_aliases_for_emit(&signature.return_type)
                .union_members()
                .is_some() =>
            {
                (signature.return_type.clone(), None)
            }
            _ => (expr.ty.clone(), None),
        }
    }

    /// Emit a conversion from one generated anonymous union wrapper to a wider generated anonymous union wrapper.
    pub(in super::super) fn emit_union_widening_value(
        &self,
        source_ty: &IrType,
        target_ty: &IrType,
        source_tokens: TokenStream,
        source_qualifier: Option<&[String]>,
        target_qualifier: Option<&[String]>,
    ) -> Result<Option<TokenStream>, EmitError> {
        let source_ty = self.resolve_type_aliases_for_emit(source_ty);
        let target_ty = self.resolve_type_aliases_for_emit(target_ty);
        let Some(variant_map) = Self::union_widening_variant_map(&source_ty, &target_ty) else {
            return Ok(None);
        };

        let source_path = self.emit_union_type_path_with_qualifier(&source_ty, source_qualifier);
        let target_path = self.emit_union_type_path_with_qualifier(&target_ty, target_qualifier);
        let arms = variant_map.into_iter().map(|(source_idx, target_idx)| {
            let source_variant = quote::format_ident!("{}", IrType::union_variant_name(source_idx));
            let target_variant = quote::format_ident!("{}", IrType::union_variant_name(target_idx));
            quote! {
                #source_path :: #source_variant(__incan_union_value) => {
                    #target_path :: #target_variant(__incan_union_value)
                }
            }
        });

        Ok(Some(quote! {
            match #source_tokens {
                #(#arms),*
            }
        }))
    }

    /// Return the dependency qualifier for generated anonymous union wrappers referenced through a public library call.
    pub(in super::super) fn pub_library_union_qualifier(canonical_path: Option<&[String]>) -> Option<Vec<String>> {
        canonical_path.and_then(|path| {
            if path.first().map(String::as_str) == Some("pub") {
                path.get(1).map(|library| vec![library.clone()])
            } else {
                None
            }
        })
    }

    /// Map each source union variant onto the matching target variant for a widening conversion.
    ///
    /// This is stricter than ordinary value-to-union injection: the payload already lives inside the source wrapper,
    /// so the generated conversion can only move it into a target variant with the same emitted payload type.
    fn union_widening_variant_map(source_ty: &IrType, target_ty: &IrType) -> Option<Vec<(usize, usize)>> {
        if source_ty == target_ty {
            return None;
        }
        let source_members = source_ty.union_members()?;
        let target_members = target_ty.union_members()?;
        if source_members.is_empty() || target_members.is_empty() {
            return None;
        }

        source_members
            .iter()
            .enumerate()
            .map(|(source_idx, source_member)| {
                target_members
                    .iter()
                    .position(|target_member| target_member == source_member)
                    .map(|target_idx| (source_idx, target_idx))
            })
            .collect()
    }

    /// Emit a type-seeded literal argument for `None`/`Ok`/`Err` when possible.
    ///
    /// This helper rewrites constructor-shaped arguments into explicit generic forms (for example `None::<T>`, `Ok::<T,
    /// E>(x)`, `Err::<T, E>(e)`) based on the expected parameter type. It prevents Rust from failing inference in calls
    /// where the callee alone does not provide enough type context.
    ///
    /// For `Result[str, E]`, string-literal payloads in both `Ok` and `Err` constructors are emitted as owned `String`
    /// values so generated Rust matches Incan string ownership semantics.
    ///
    /// If a fully-informed rewrite is not possible, this returns `Ok(None)` and the normal expression emission path is
    /// used.
    ///
    /// ## Parameters
    /// - `arg`: Source argument expression from IR.
    /// - `target_ty`: Expected type of the callee parameter at this position.
    ///
    /// ## Returns
    /// - (`Result<Option<TokenStream>, EmitError>`): Seeded token stream when a rewrite applies, otherwise `None`.
    pub(in super::super) fn emit_inference_seeded_literal_arg(
        &self,
        arg: &TypedExpr,
        target_ty: &IrType,
    ) -> Result<Option<TokenStream>, EmitError> {
        self.emit_inference_seeded_literal_arg_with_union_qualifier(arg, target_ty, None)
    }

    /// Emit inference-seeded constructor or union payload arguments with an optional explicit union path qualifier.
    ///
    /// Source modules normally reference generated ordinary union wrappers through the current module or crate root.
    /// Imported `pub::library` calls may need to wrap member literals with a library-qualified union wrapper instead,
    /// so this helper keeps the target type logic shared while letting callers control only the wrapper path.
    fn emit_inference_seeded_literal_arg_with_union_qualifier(
        &self,
        arg: &TypedExpr,
        target_ty: &IrType,
        union_qualifier: Option<&[String]>,
    ) -> Result<Option<TokenStream>, EmitError> {
        if let Some(wrapped) = self.emit_union_payload_arg(arg, target_ty, union_qualifier)? {
            return Ok(Some(wrapped));
        }

        // ---- Context: constructor seeding from an expected parameter type ----
        match (&arg.kind, target_ty) {
            // ---- Context: seed `None` from the target `Option[T]` ----
            (IrExprKind::None, IrType::Option(inner)) => {
                let inner_ty = if Self::is_unresolved_call_seed_type(inner) {
                    quote! { () }
                } else {
                    self.emit_type(inner)
                };
                Ok(Some(quote! { None::<#inner_ty> }))
            }

            (_, IrType::Option(inner)) => self.emit_option_payload_arg(arg, inner, union_qualifier),

            // ---- Context: seed `Ok`/`Err` constructors spelled as calls ----
            (IrExprKind::Call { func, args, .. }, IrType::Result(ok_ty, err_ty)) => {
                let IrExprKind::Var { name, .. } = &func.kind else {
                    return Ok(None);
                };
                let Some(first_arg) = args.first() else {
                    return Ok(None);
                };
                let inner = Self::emit_result_payload_tokens(&first_arg.expr, self.emit_expr(&first_arg.expr)?);

                if name == constructors::as_str(ConstructorId::Ok) {
                    // For `Ok`, keep unresolved `T` as `_` so Rust can infer it
                    // from usage while still stabilizing `E`.
                    let ok_tokens = if Self::is_unresolved_call_seed_type(ok_ty) {
                        quote! { _ }
                    } else {
                        self.emit_type(ok_ty)
                    };
                    // Default unresolved error type to `()` for deterministic
                    // fallback in assertion/helper-oriented paths.
                    let err_tokens = if Self::is_unresolved_call_seed_type(err_ty) {
                        quote! { () }
                    } else {
                        self.emit_type(err_ty)
                    };
                    return Ok(Some(quote! { Ok::<#ok_tokens, #err_tokens>(#inner) }));
                }

                if name == constructors::as_str(ConstructorId::Err) {
                    // Mirror `Ok` strategy: anchor the opposite side with `()`
                    // and leave the payload side as `_` when unresolved.
                    let ok_tokens = if Self::is_unresolved_call_seed_type(ok_ty) {
                        quote! { () }
                    } else {
                        self.emit_type(ok_ty)
                    };
                    let err_tokens = if Self::is_unresolved_call_seed_type(err_ty) {
                        quote! { _ }
                    } else {
                        self.emit_type(err_ty)
                    };
                    return Ok(Some(quote! { Err::<#ok_tokens, #err_tokens>(#inner) }));
                }

                Ok(None)
            }
            // ---- Context: seed `Ok`/`Err` constructors lowered as struct-like IR ----
            (IrExprKind::Struct { name, fields }, IrType::Result(ok_ty, err_ty)) => {
                let Some((_, first_arg)) = fields.first() else {
                    return Ok(None);
                };
                let inner = Self::emit_result_payload_tokens(first_arg, self.emit_expr(first_arg)?);

                if name == constructors::as_str(ConstructorId::Ok) {
                    let ok_tokens = if Self::is_unresolved_call_seed_type(ok_ty) {
                        quote! { _ }
                    } else {
                        self.emit_type(ok_ty)
                    };
                    let err_tokens = if Self::is_unresolved_call_seed_type(err_ty) {
                        quote! { () }
                    } else {
                        self.emit_type(err_ty)
                    };
                    return Ok(Some(quote! { Ok::<#ok_tokens, #err_tokens>(#inner) }));
                }

                if name == constructors::as_str(ConstructorId::Err) {
                    let ok_tokens = if Self::is_unresolved_call_seed_type(ok_ty) {
                        quote! { () }
                    } else {
                        self.emit_type(ok_ty)
                    };
                    let err_tokens = if Self::is_unresolved_call_seed_type(err_ty) {
                        quote! { _ }
                    } else {
                        self.emit_type(err_ty)
                    };
                    return Ok(Some(quote! { Err::<#ok_tokens, #err_tokens>(#inner) }));
                }

                Ok(None)
            }
            _ => Ok(None),
        }
    }

    /// Emit `Ok`/`Err` constructors with explicit generic context from an expected `Result<T, E>` type.
    ///
    /// String literals in `Ok` and `Err` payload positions are promoted to owned `String` values when emitted to Rust.
    pub(super) fn emit_result_constructor_with_context(
        &self,
        constructor_name: &str,
        inner_expr: &TypedExpr,
        ok_ty: &IrType,
        err_ty: &IrType,
    ) -> Result<Option<TokenStream>, EmitError> {
        // ---- Context: normalize payload before we seed constructor generics ----
        let inner = if matches!(inner_expr.kind, IrExprKind::None) && matches!(ok_ty, IrType::Unit) {
            quote! { () }
        } else {
            self.emit_expr(inner_expr)?
        };
        let inner = Self::emit_result_payload_tokens(inner_expr, inner);

        // ---- Context: seed `Ok` using the expected result type ----
        if constructor_name == constructors::as_str(ConstructorId::Ok) {
            let ok_tokens = if Self::is_unresolved_type(ok_ty) {
                quote! { _ }
            } else {
                self.emit_type(ok_ty)
            };
            let err_tokens = if Self::is_unresolved_type(err_ty) {
                quote! { () }
            } else {
                self.emit_type(err_ty)
            };
            return Ok(Some(quote! { Ok::<#ok_tokens, #err_tokens>(#inner) }));
        }

        // ---- Context: seed `Err` using the expected result type ----
        if constructor_name == constructors::as_str(ConstructorId::Err) {
            let ok_tokens = if Self::is_unresolved_type(ok_ty) {
                quote! { () }
            } else {
                self.emit_type(ok_ty)
            };
            let err_tokens = if Self::is_unresolved_type(err_ty) {
                quote! { _ }
            } else {
                self.emit_type(err_ty)
            };
            return Ok(Some(quote! { Err::<#ok_tokens, #err_tokens>(#inner) }));
        }

        Ok(None)
    }

    /// Emit a function call expression.
    ///
    /// Handles regular function calls (user-defined functions).
    /// Built-in functions are handled by `emit_builtin_call` or `try_emit_builtin_call`.
    pub(in super::super) fn emit_call_expr(
        &self,
        func: &TypedExpr,
        type_args: &[IrType],
        args: &[IrCallArg],
        callable_signature: Option<&FunctionSignature>,
        canonical_path: Option<&[String]>,
    ) -> Result<TokenStream, EmitError> {
        self.emit_call_expr_with_result_use(func, type_args, args, callable_signature, canonical_path, None)
    }

    /// Emit a call while preserving the surrounding value-use target for argument shaping.
    pub(in super::super) fn emit_call_expr_for_use(
        &self,
        func: &TypedExpr,
        type_args: &[IrType],
        args: &[IrCallArg],
        callable_signature: Option<&FunctionSignature>,
        canonical_path: Option<&[String]>,
        result_use_site: ValueUseSite<'_>,
    ) -> Result<TokenStream, EmitError> {
        self.emit_call_expr_with_result_use(
            func,
            type_args,
            args,
            callable_signature,
            canonical_path,
            Some(result_use_site),
        )
    }

    /// Shared call emitter used by plain and target-aware call emission.
    fn emit_call_expr_with_result_use(
        &self,
        func: &TypedExpr,
        type_args: &[IrType],
        args: &[IrCallArg],
        callable_signature: Option<&FunctionSignature>,
        canonical_path: Option<&[String]>,
        result_use_site: Option<ValueUseSite<'_>>,
    ) -> Result<TokenStream, EmitError> {
        if let Some(tokens) = self.try_emit_testing_assert_call(canonical_path, args)? {
            return Ok(tokens);
        }

        let canonical_name = canonical_path.and_then(|path| path.last()).map(|s| s.as_str());
        let local_name = if let IrExprKind::Var { name, .. } = &func.kind {
            Some(name.as_str())
        } else {
            None
        };
        let result_target_ty = result_use_site.and_then(Self::use_site_target_ty);
        let associated_target_ty = match result_target_ty {
            Some(IrType::Result(ok_ty, _)) => Some(ok_ty.as_ref()),
            other => other,
        };
        let associated_signature = match &func.kind {
            IrExprKind::AssociatedFunction { function_name, .. } => {
                associated_target_ty.and_then(|ty| self.specialized_method_signature_for_receiver(ty, function_name))
            }
            _ => None,
        };
        let callee_name = local_name.or(canonical_name);
        let merged_signature = FunctionRegistry::effective_call_signature_by(
            self.function_registry,
            self.canonical_function_registry(),
            local_name,
            canonical_path,
            callable_signature,
            Some(&func.ty),
            |left, right| self.call_signature_type_matches(left, right),
        );
        let result_specialized_signature = merged_signature.as_ref().and_then(|signature| {
            result_target_ty.and_then(|target_ty| Self::specialize_signature_by_result_target(signature, target_ty))
        });
        let function_sig = associated_signature
            .as_ref()
            .or_else(|| result_specialized_signature.as_ref().or(merged_signature.as_ref()));
        // The checked-newtype lowering path emits a compiler-internal panic marker call. This remains the narrow,
        // explicitly-tracked generated `panic!` exemption that issue #351 left to a separate follow-up. Render it as
        // the Rust `panic!` macro so generated code stays valid without colliding with user-defined functions that may
        // also be named `panic`.
        if matches!(callee_name, Some(name) if name == INTERNAL_PANIC_FN)
            && canonical_path.is_none()
            && args.len() == 1
            && matches!(
                &args[0].expr.kind,
                super::super::super::expr::IrExprKind::Literal(super::super::super::expr::Literal::StaticStr(_))
            )
        {
            let panic_args: Vec<TokenStream> =
                args.iter().map(|a| self.emit_expr(&a.expr)).collect::<Result<_, _>>()?;
            return Ok(quote! { panic!(#(#panic_args),*) });
        }

        // Handle builtin functions specially only when the callee did not resolve to a real function signature.
        if canonical_path.is_none()
            && function_sig.is_none()
            && let Some(name) = callee_name
        {
            let positional: Vec<TypedExpr> = args.iter().map(|a| a.expr.clone()).collect();
            if let Some(result) = self.try_emit_builtin_call(name, &positional)? {
                return Ok(result);
            }

            if let Some(IrType::Result(ok_ty, err_ty)) = self.current_function_return_type.borrow().as_ref()
                && let Some(first_arg) = positional.first()
                && let Some(result) = self.emit_result_constructor_with_context(name, first_arg, ok_ty, err_ty)?
            {
                return Ok(result);
            }
        }

        let f = if canonical_path.is_some_and(|path| path.first().map(String::as_str) == Some("pub"))
            && Self::callee_is_imported_module_path(func)
        {
            self.emit_expr(func)?
        } else if let Some(path) = canonical_path {
            self.emit_canonical_callee_path(path)?.unwrap_or(self.emit_expr(func)?)
        } else {
            self.emit_expr(func)?
        };
        let pub_library_union_qualifier: Option<Vec<String>> = Self::pub_library_union_qualifier(canonical_path);
        let turbofish = if type_args.is_empty() {
            quote! {}
        } else {
            let emitted: Vec<TokenStream> = type_args.iter().map(|ty| self.emit_type(ty)).collect();
            quote! { ::<#(#emitted),*> }
        };

        if matches!(
            &func.kind,
            IrExprKind::Var {
                ref_kind: VarRefKind::TypeName,
                ..
            }
        ) && function_sig.is_none()
            && !args.is_empty()
            && args.iter().all(|arg| arg.name.is_some())
            && let Some(target_name) = result_target_ty.and_then(|ty| match ty {
                IrType::Ref(inner) | IrType::RefMut(inner) => inner.nominal_type_name(),
                _ => ty.nominal_type_name(),
            })
        {
            let fields = args
                .iter()
                .filter_map(|arg| arg.name.as_ref().map(|name| (name.clone(), arg.expr.clone())))
                .collect::<Vec<_>>();
            if let Some(metadata) = self
                .struct_constructor_metadata_for_fields(target_name, &fields)
                .or_else(|| self.unique_struct_constructor_metadata_for_fields(&fields))
            {
                let mut provided: std::collections::HashMap<&str, &TypedExpr> = std::collections::HashMap::new();
                for (name, expr) in &fields {
                    if let Some(canonical) = metadata.canonical_field_name(name) {
                        provided.insert(canonical, expr);
                    }
                }

                let mut out_fields = Vec::new();
                for field_name in &metadata.fields {
                    let field_ident = Self::rust_ident(field_name);
                    let target_ty = metadata.field_types.get(field_name);
                    if let Some(value) = provided.get(field_name.as_str()) {
                        let value = self.emit_expr_for_use(value, ValueUseSite::StructField { target_ty })?;
                        out_fields.push(quote! { #field_ident: #value });
                    } else if let Some(default_expr) = metadata.field_defaults.get(field_name) {
                        let value = self.emit_expr_for_use(default_expr, ValueUseSite::StructField { target_ty })?;
                        out_fields.push(quote! { #field_ident: #value });
                    } else {
                        return Err(EmitError::Unsupported(format!(
                            "missing required field '{}' when constructing '{}'",
                            field_name, target_name
                        )));
                    }
                }

                return Ok(quote! { #f { #(#out_fields),* } });
            }
        }

        if let Some(sig) = function_sig
            && sig.params.iter().any(|param| param.kind != ParamKind::Normal)
        {
            let f = Self::call_callee_tokens(func, f, type_args);
            let arg_tokens = self.emit_rest_aware_call_args(func, args, sig)?;
            return Ok(quote! { #f #turbofish (#(#arg_tokens),*) });
        }

        // Order arguments only when keyword args are present (positional-only calls preserve previous behavior,
        // which is important for snapshots + for default-arg lowering work that happens elsewhere).
        let has_named_args = args.iter().any(|a| a.name.is_some());
        let ordered_args: Vec<(TypedExpr, bool)> = if has_named_args {
            if let Some(sig) = function_sig {
                let mut positional: Vec<TypedExpr> = Vec::new();
                let mut named: std::collections::HashMap<&str, TypedExpr> = std::collections::HashMap::new();
                for a in args {
                    if let Some(name) = a.name.as_deref() {
                        named.insert(name, a.expr.clone());
                    } else {
                        positional.push(a.expr.clone());
                    }
                }

                let mut pos_idx = 0usize;
                let mut out: Vec<(TypedExpr, bool)> = Vec::new();
                for p in &sig.params {
                    if let Some(v) = named.get(p.name.as_str()) {
                        out.push((v.clone(), false));
                    } else if pos_idx < positional.len() {
                        out.push((positional[pos_idx].clone(), false));
                        pos_idx += 1;
                    } else if let Some(default_arg) = &p.default {
                        out.push((default_arg.clone(), true));
                    }
                }
                out
            } else {
                args.iter().map(|a| (a.expr.clone(), false)).collect()
            }
        } else {
            let mut out: Vec<(TypedExpr, bool)> = args.iter().map(|a| (a.expr.clone(), false)).collect();
            if let Some(sig) = function_sig {
                for p in sig.params.iter().skip(out.len()) {
                    if let Some(default_arg) = &p.default {
                        out.push((default_arg.clone(), true));
                    } else {
                        break;
                    }
                }
            }
            out
        };

        // Handle argument passing with signature-based borrow insertion
        let arg_tokens: Vec<TokenStream> = ordered_args
            .iter()
            .enumerate()
            .map(|(idx, (a, from_default))| {
                let target_ty = function_sig
                    .and_then(|sig| sig.params.get(idx))
                    .map(|param| &param.ty)
                    .or_else(|| match &func.ty {
                        IrType::Function { params, .. } => params.get(idx),
                        _ => None,
                    });
                let sig_param = function_sig.and_then(|sig| sig.params.get(idx));
                let in_return = *self.in_return_context.borrow();
                let use_site = if let IrExprKind::Var { name, ref_kind, .. } = &func.kind {
                    if matches!(ref_kind, VarRefKind::ExternalRustName) || self.external_rust_functions.contains(name) {
                        ValueUseSite::ExternalCallArg { target_ty }
                    } else {
                        ValueUseSite::IncanCallArg {
                            target_ty,
                            callee_param: sig_param,
                            in_return,
                        }
                    }
                } else {
                    ValueUseSite::IncanCallArg {
                        target_ty,
                        callee_param: sig_param,
                        in_return,
                    }
                };
                let aggregate_literal_arg = match &a.kind {
                    IrExprKind::List(_) | IrExprKind::Dict(_) | IrExprKind::Set(_) | IrExprKind::Tuple(_) => true,
                    IrExprKind::InteropCoerce { expr, .. } => {
                        matches!(
                            expr.kind,
                            IrExprKind::List(_) | IrExprKind::Dict(_) | IrExprKind::Set(_) | IrExprKind::Tuple(_)
                        )
                    }
                    _ => false,
                };
                let target_aware_aggregate_literal_arg =
                    aggregate_literal_arg && !matches!(use_site, ValueUseSite::ExternalCallArg { .. });
                let (widening_source_ty, _) = self.union_widening_source_for_expr(a);
                let target_aware_union_widening_arg = target_ty
                    .is_some_and(|target_ty| self.union_widening_needed(&widening_source_ty, target_ty))
                    && !matches!(use_site, ValueUseSite::ExternalCallArg { .. });
                let (list_widening_source_ty, _) = self.list_element_widening_source_for_expr(a);
                let target_aware_list_element_widening_arg = target_ty
                    .is_some_and(|target_ty| self.list_element_widening_needed(&list_widening_source_ty, target_ty))
                    && !matches!(use_site, ValueUseSite::ExternalCallArg { .. });
                let target_aware_value_arg = target_aware_aggregate_literal_arg
                    || target_aware_union_widening_arg
                    || target_aware_list_element_widening_arg;
                let arg_plan = ArgumentPassingPlan::for_use_site(a, use_site);
                let previous_qualify = if *from_default {
                    Some(self.qualify_internal_canonical_paths.replace(true))
                } else {
                    None
                };
                let emitted = (|| {
                    let mut emitted_from_seed = false;
                    let emitted = if let Some(target_ty) = target_ty {
                        if let Some(seed) = self.emit_inference_seeded_literal_arg_with_union_qualifier(
                            a,
                            target_ty,
                            pub_library_union_qualifier.as_deref(),
                        )? {
                            emitted_from_seed = true;
                            seed
                        } else if Self::is_unresolved_call_seed_type(target_ty) {
                            // Signature exists but leaves generics unresolved: fallback to the argument's own inferred
                            // IR type to seed constructor literals.
                            if let Some(seed) = self.emit_inference_seeded_literal_arg_with_union_qualifier(
                                a,
                                &a.ty,
                                pub_library_union_qualifier.as_deref(),
                            )? {
                                emitted_from_seed = true;
                                seed
                            } else if target_aware_value_arg {
                                self.emit_expr_for_use_with_union_qualifier(
                                    a,
                                    use_site,
                                    pub_library_union_qualifier.as_deref(),
                                )?
                            } else {
                                self.emit_expr(a)?
                            }
                        } else if target_aware_value_arg {
                            self.emit_expr_for_use_with_union_qualifier(
                                a,
                                use_site,
                                pub_library_union_qualifier.as_deref(),
                            )?
                        } else {
                            self.emit_expr(a)?
                        }
                    } else {
                        // No parameter type available (e.g. heavily generic paths): use the argument's own type as a
                        // best-effort inference seed source.
                        if let Some(seed) = self.emit_inference_seeded_literal_arg_with_union_qualifier(
                            a,
                            &a.ty,
                            pub_library_union_qualifier.as_deref(),
                        )? {
                            emitted_from_seed = true;
                            seed
                        } else if target_aware_value_arg {
                            self.emit_expr_for_use_with_union_qualifier(
                                a,
                                use_site,
                                pub_library_union_qualifier.as_deref(),
                            )?
                        } else {
                            self.emit_expr(a)?
                        }
                    };
                    Ok::<(TokenStream, bool), EmitError>((emitted, emitted_from_seed))
                })();
                if let Some(previous) = previous_qualify {
                    self.qualify_internal_canonical_paths.replace(previous);
                }
                let (emitted, emitted_from_seed) = emitted?;

                if let Some(adapter) = self.borrowed_function_adapter_arg(a, target_ty) {
                    return Ok(adapter);
                }

                let tokens = if emitted_from_seed || target_aware_value_arg {
                    arg_plan.apply_after_value_plan(emitted)
                } else {
                    arg_plan.apply_full(emitted)
                };
                Ok(tokens)
            })
            .collect::<Result<_, _>>()?;

        let f = Self::call_callee_tokens(func, f, type_args);
        Ok(quote! { #f #turbofish (#(#arg_tokens),*) })
    }

    /// Parenthesize call targets whose emitted Rust is an expression block rather than a path/call expression.
    ///
    /// Storage-rooted method calls materialize arguments and enter `StaticCell::with_ref` / `with_mut`, so their
    /// emitted callee has block shape. Calling that result requires `({ ... })(arg)` in Rust.
    fn call_callee_tokens(func: &TypedExpr, emitted: TokenStream, type_args: &[IrType]) -> TokenStream {
        if !type_args.is_empty() {
            return emitted;
        }
        match &func.kind {
            IrExprKind::MethodCall { receiver, .. } if Self::expr_is_storage_rooted(receiver) => {
                quote! { ({ #emitted }) }
            }
            IrExprKind::If { .. }
            | IrExprKind::Match { .. }
            | IrExprKind::Closure { .. }
            | IrExprKind::Block { .. }
            | IrExprKind::Loop { .. } => quote! { ({ #emitted }) },
            _ => emitted,
        }
    }

    /// Return whether the callee is already spelled as a module-rooted path in source, such as `lib.function`.
    fn callee_is_imported_module_path(func: &TypedExpr) -> bool {
        match &func.kind {
            IrExprKind::Field { object, .. } => Self::callee_is_imported_module_path(object),
            IrExprKind::Var { ref_kind, .. } => {
                matches!(ref_kind, VarRefKind::ExternalName | VarRefKind::ExternalRustName)
            }
            _ => false,
        }
    }

    /// Emit call arguments while preserving rest-argument expansion semantics.
    pub(in super::super) fn emit_rest_aware_call_args(
        &self,
        func: &TypedExpr,
        args: &[IrCallArg],
        sig: &FunctionSignature,
    ) -> Result<Vec<TokenStream>, EmitError> {
        let normal_param_positions: Vec<usize> = sig
            .params
            .iter()
            .enumerate()
            .filter_map(|(idx, param)| (param.kind == ParamKind::Normal).then_some(idx))
            .collect();
        let mut normal_bindings: Vec<Option<&IrCallArg>> = vec![None; normal_param_positions.len()];
        let mut rest_positional_args: Vec<&IrCallArg> = Vec::new();
        let mut rest_keyword_args: Vec<&IrCallArg> = Vec::new();
        let mut positional_index = 0usize;

        for arg in args {
            match arg.kind {
                IrCallArgKind::Positional => {
                    if positional_index < normal_bindings.len() {
                        normal_bindings[positional_index] = Some(arg);
                        positional_index += 1;
                    } else {
                        rest_positional_args.push(arg);
                    }
                }
                IrCallArgKind::PositionalUnpack => rest_positional_args.push(arg),
                IrCallArgKind::Named => {
                    let Some(name) = arg.name.as_deref() else {
                        rest_keyword_args.push(arg);
                        continue;
                    };
                    if let Some((binding_idx, _)) = normal_param_positions
                        .iter()
                        .enumerate()
                        .find(|(_, param_idx)| sig.params[**param_idx].name == name)
                    {
                        normal_bindings[binding_idx] = Some(arg);
                    } else {
                        rest_keyword_args.push(arg);
                    }
                }
                IrCallArgKind::KeywordUnpack => rest_keyword_args.push(arg),
            }
        }

        let mut normal_binding_index = 0usize;
        let mut out = Vec::with_capacity(sig.params.len());
        for (param_idx, param) in sig.params.iter().enumerate() {
            match param.kind {
                ParamKind::Normal => {
                    if let Some(arg) = normal_bindings.get(normal_binding_index).and_then(|binding| *binding) {
                        out.push(self.emit_regular_call_arg(func, &arg.expr, param_idx, param)?);
                    } else if let Some(default_arg) = &param.default {
                        out.push(self.emit_regular_call_arg(func, default_arg, param_idx, param)?);
                    }
                    normal_binding_index += 1;
                }
                ParamKind::RestPositional => {
                    let element_ty = match &param.ty {
                        IrType::List(element_ty) => element_ty.as_ref(),
                        _ => &param.ty,
                    };
                    out.push(self.emit_rest_positional_arg(&rest_positional_args, element_ty)?);
                }
                ParamKind::RestKeyword => {
                    let value_ty = match &param.ty {
                        IrType::Dict(_, value_ty) => value_ty.as_ref(),
                        _ => &param.ty,
                    };
                    out.push(self.emit_rest_keyword_arg(&rest_keyword_args, value_ty)?);
                }
            }
        }

        Ok(out)
    }

    /// Emit one positional argument that may include rest expansion.
    fn emit_rest_positional_arg(&self, args: &[&IrCallArg], element_ty: &IrType) -> Result<TokenStream, EmitError> {
        let mut statements = Vec::with_capacity(args.len());
        for arg in args {
            let emitted = match arg.kind {
                IrCallArgKind::Positional => {
                    let item = self.emit_expr_for_use(
                        &arg.expr,
                        ValueUseSite::CollectionElement {
                            target_ty: Some(element_ty),
                        },
                    )?;
                    quote! { __incan_rest_args.push(#item); }
                }
                IrCallArgKind::PositionalUnpack => {
                    let unpacked = self.emit_expr(&arg.expr)?;
                    quote! { __incan_rest_args.extend(#unpacked); }
                }
                _ => continue,
            };
            statements.push(emitted);
        }
        Ok(quote! {{
            let mut __incan_rest_args = Vec::new();
            #(#statements)*
            __incan_rest_args
        }})
    }

    /// Emit the synthetic `**kwargs` map argument for a rest-aware call.
    fn emit_rest_keyword_arg(&self, args: &[&IrCallArg], value_ty: &IrType) -> Result<TokenStream, EmitError> {
        let mut statements = Vec::with_capacity(args.len());
        for arg in args {
            let emitted = match arg.kind {
                IrCallArgKind::Named => {
                    let Some(name) = arg.name.as_deref() else {
                        continue;
                    };
                    let value = self.emit_expr_for_use(
                        &arg.expr,
                        ValueUseSite::CollectionElement {
                            target_ty: Some(value_ty),
                        },
                    )?;
                    quote! { __incan_rest_kwargs.insert(#name.to_string(), #value); }
                }
                IrCallArgKind::KeywordUnpack => {
                    let unpacked = self.emit_expr(&arg.expr)?;
                    quote! { __incan_rest_kwargs.extend(#unpacked); }
                }
                _ => continue,
            };
            statements.push(emitted);
        }
        Ok(quote! {{
            let mut __incan_rest_kwargs = std::collections::HashMap::new();
            #(#statements)*
            __incan_rest_kwargs
        }})
    }

    /// Emit one positional or named argument for a non-rest call.
    ///
    /// The caller supplies the selected parameter so this path can apply literal inference, union wrapping, and borrow
    /// preservation with the same target type information used by ordinary Incan calls.
    fn emit_regular_call_arg(
        &self,
        func: &TypedExpr,
        arg: &TypedExpr,
        idx: usize,
        param: &FunctionParam,
    ) -> Result<TokenStream, EmitError> {
        let target_ty = Some(&param.ty);
        if let Some(adapter) = self.borrowed_function_adapter_arg(arg, target_ty) {
            return Ok(adapter);
        }
        let in_return = *self.in_return_context.borrow();
        let use_site = if let IrExprKind::Var { name, ref_kind, .. } = &func.kind {
            if matches!(ref_kind, VarRefKind::ExternalRustName) || self.external_rust_functions.contains(name) {
                ValueUseSite::ExternalCallArg { target_ty }
            } else {
                ValueUseSite::IncanCallArg {
                    target_ty,
                    callee_param: Some(param),
                    in_return,
                }
            }
        } else {
            ValueUseSite::IncanCallArg {
                target_ty,
                callee_param: Some(param),
                in_return,
            }
        };
        let arg_plan = ArgumentPassingPlan::for_use_site(arg, use_site);
        let emitted = if let Some(seed) = self.emit_inference_seeded_literal_arg(arg, &param.ty)? {
            arg_plan.apply_after_value_plan(seed)
        } else if Self::is_unresolved_call_seed_type(&param.ty) {
            if let Some(seed) = self.emit_inference_seeded_literal_arg(arg, &arg.ty)? {
                arg_plan.apply_after_value_plan(seed)
            } else {
                arg_plan.apply_after_value_plan(self.emit_expr_for_use(arg, use_site)?)
            }
        } else {
            arg_plan.apply_after_value_plan(self.emit_expr_for_use(arg, use_site)?)
        };
        let _ = idx;
        Ok(emitted)
    }

    /// Emit a canonical callee path when the compiler knows how to materialize that namespace at the current call
    /// site.
    ///
    /// Canonical stdlib calls route through the generated `crate::__incan_std` module. Canonical calls to internal
    /// source modules route through an explicit `crate::...` path so imported helper calls remain valid when default
    /// argument expressions are expanded outside the defining module.
    fn emit_canonical_callee_path(&self, canonical_path: &[String]) -> Result<Option<TokenStream>, EmitError> {
        if canonical_path.len() < 2 {
            return Ok(None);
        }

        let module_path: Vec<String> = canonical_path[..canonical_path.len() - 1].to_vec();
        let Some(function_name) = canonical_path.last() else {
            return Ok(None);
        };
        let mut segments: Vec<TokenStream> = if module_path.first().map(String::as_str) == Some("incan_stdlib") {
            let mut segments = vec![quote! { incan_stdlib }];
            for seg in module_path.iter().skip(1) {
                let ident = Self::rust_ident(seg);
                segments.push(quote! { #ident });
            }
            segments
        } else if module_path.first().map(String::as_str) == Some(stdlib::STDLIB_ROOT) {
            if canonical_path.len() < 3 || !stdlib::is_known_stdlib_module(&module_path) {
                return Ok(None);
            }
            let ns = Self::rust_ident(stdlib::INCAN_STD_NAMESPACE);
            let mut segments = vec![quote! { crate }, quote! { #ns }];
            for seg in module_path.iter().skip(1) {
                let ident = Self::rust_ident(seg);
                segments.push(quote! { #ident });
            }
            segments
        } else if module_path.first().map(String::as_str) == Some("pub") {
            let mut segments = Vec::new();
            for seg in module_path.iter().skip(1) {
                let ident = Self::rust_ident(seg);
                segments.push(quote! { #ident });
            }
            if segments.is_empty() {
                return Ok(None);
            }
            segments
        } else if *self.qualify_internal_canonical_paths.borrow() && self.is_internal_module_path(&module_path) {
            let mut segments = vec![quote! { crate }];
            for seg in &module_path {
                let ident = Self::rust_ident(seg);
                segments.push(quote! { #ident });
            }
            segments
        } else {
            return Ok(None);
        };

        let fn_ident = Self::rust_ident(function_name);
        segments.push(quote! { #fn_ident });

        let mut iter = segments.into_iter();
        let Some(first) = iter.next() else {
            return Ok(None);
        };
        let path_tokens = iter.fold(first, |acc, seg| quote! { #acc :: #seg });
        Ok(Some(path_tokens))
    }

    /// Emit a binary operation expression.
    pub(in super::super) fn emit_binop_expr(
        &self,
        op: &BinOp,
        left: &TypedExpr,
        right: &TypedExpr,
    ) -> Result<TokenStream, EmitError> {
        // Special-case: const-fold string additions using literals/known consts
        if matches!(op, BinOp::Add)
            && let Some(tokens) = self.try_emit_static_str_add(left, right)?
        {
            return Ok(tokens);
        }

        let l_raw = self.emit_expr(left)?;
        let r_raw = self.emit_expr(right)?;

        // Determine binop plan (conversions + emit strategy)
        let plan = determine_binop_plan(op, left, right);
        let mut l = plan.lhs_conv.apply(l_raw);
        let mut r = plan.rhs_conv.apply(r_raw);

        match plan.emit {
            BinOpEmitKind::StdlibCall { path, borrow_args } => {
                if borrow_args {
                    Ok(quote! { #path(&#l, &#r) })
                } else {
                    Ok(quote! { #path(#l, #r) })
                }
            }
            BinOpEmitKind::Pow { result_is_int } => {
                if result_is_int {
                    Ok(quote! { #l.pow(#r as u32) })
                } else {
                    Ok(quote! { #l.powf(#r) })
                }
            }
            BinOpEmitKind::Infix { token } => {
                let op_tokens = token;
                if Self::binop_operand_needs_parens(op, left, false) {
                    l = quote! { (#l) };
                }
                if Self::binop_operand_needs_parens(op, right, true) {
                    r = quote! { (#r) };
                }

                // Handle reference vs value comparisons
                let is_comparison = matches!(
                    op,
                    BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge
                );

                if is_comparison {
                    let left_is_ref = matches!(&left.ty, IrType::Ref(_) | IrType::RefMut(_));
                    let right_is_value = !matches!(&right.ty, IrType::Ref(_) | IrType::RefMut(_));

                    if left_is_ref && right_is_value {
                        return Ok(quote! { *(#l) #op_tokens #r });
                    }
                }

                Ok(quote! { #l #op_tokens #r })
            }
        }
    }

    /// Return whether a nested binary operand must be parenthesized to preserve Incan precedence.
    fn binop_operand_needs_parens(parent: &BinOp, operand: &TypedExpr, is_right: bool) -> bool {
        let IrExprKind::BinOp { op: child, .. } = &operand.kind else {
            return false;
        };

        let parent_precedence = Self::binop_precedence(parent);
        let child_precedence = Self::binop_precedence(child);
        if child_precedence < parent_precedence {
            return true;
        }
        if child_precedence > parent_precedence {
            return false;
        }

        if Self::is_comparison_binop(parent) || Self::is_comparison_binop(child) {
            return true;
        }

        is_right && (parent != child || Self::right_same_precedence_needs_parens(parent))
    }

    /// Return the relative precedence rank used when lowering nested Incan binary operations to Rust.
    fn binop_precedence(op: &BinOp) -> u8 {
        match op {
            BinOp::Or => 1,
            BinOp::And => 2,
            BinOp::BitOr => 3,
            BinOp::BitXor => 4,
            BinOp::BitAnd => 5,
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => 6,
            BinOp::Shl | BinOp::Shr => 7,
            BinOp::Add | BinOp::Sub => 8,
            BinOp::Mul | BinOp::Div | BinOp::FloorDiv | BinOp::Mod => 9,
            BinOp::Pow => 10,
        }
    }

    /// Return whether an operator is a non-associative comparison.
    fn is_comparison_binop(op: &BinOp) -> bool {
        matches!(
            op,
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge
        )
    }

    /// Return whether a same-precedence right operand changes semantics without parentheses.
    fn right_same_precedence_needs_parens(op: &BinOp) -> bool {
        matches!(
            op,
            BinOp::Sub | BinOp::Div | BinOp::FloorDiv | BinOp::Mod | BinOp::Pow | BinOp::Shl | BinOp::Shr
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::ir::decl::FunctionParam;
    use crate::backend::ir::expr::{
        IrCallArg, IrCallArgKind, IrInteropCoercionKind, Literal as IrLiteral, VarAccess, VarRefKind,
    };
    use crate::backend::ir::types::{IR_UNION_TYPE_NAME, IrType, Mutability};
    use crate::backend::ir::{FunctionRegistry, IrEmitter, TypedExpr};
    use incan_core::lang::types::numerics::NumericTypeId;

    fn render(tokens: TokenStream) -> String {
        tokens.to_string().replace(' ', "")
    }

    fn rust_call_target(name: &str) -> TypedExpr {
        TypedExpr::new(
            IrExprKind::Var {
                name: name.to_string(),
                access: VarAccess::Copy,
                ref_kind: VarRefKind::ExternalRustName,
            },
            IrType::Function {
                params: Vec::new(),
                ret: Box::new(IrType::Unit),
            },
        )
    }

    fn local_arg(name: &str, ty: IrType) -> TypedExpr {
        TypedExpr::new(
            IrExprKind::Var {
                name: name.to_string(),
                access: VarAccess::Read,
                ref_kind: VarRefKind::Value,
            },
            ty,
        )
    }

    fn pos_arg(expr: TypedExpr) -> IrCallArg {
        IrCallArg {
            name: None,
            kind: IrCallArgKind::Positional,
            expr,
        }
    }

    fn typed_rust_call_target(name: &str, params: Vec<IrType>) -> TypedExpr {
        TypedExpr::new(
            IrExprKind::Var {
                name: name.to_string(),
                access: VarAccess::Copy,
                ref_kind: VarRefKind::ExternalRustName,
            },
            IrType::Function {
                params,
                ret: Box::new(IrType::Unit),
            },
        )
    }

    fn result_constructor_call(constructor: ConstructorId, payload: TypedExpr, ty: IrType) -> TypedExpr {
        TypedExpr::new(
            IrExprKind::Struct {
                name: constructors::as_str(constructor).to_string(),
                fields: vec![(String::new(), payload)],
            },
            ty,
        )
    }

    fn canonical_testing_path(name: &str) -> Vec<String> {
        vec!["std".to_string(), "testing".to_string(), name.to_string()]
    }

    #[test]
    fn emit_internal_canonical_call_preserves_local_binding_without_default_context()
    -> Result<(), Box<dyn std::error::Error>> {
        let registry = FunctionRegistry::new();
        let mut emitter = IrEmitter::new(&registry);
        emitter.set_internal_module_roots(std::collections::HashSet::from(["defaults".to_string()]));
        let func = rust_call_target("fallback");
        let path = vec!["defaults".to_string(), "fallback".to_string()];
        let tokens = emitter
            .emit_call_expr(&func, &[], &[], None, Some(&path))
            .map_err(|err| std::io::Error::other(format!("canonical internal call should emit: {err:?}")))?;
        assert_eq!(render(tokens), "fallback()");
        Ok(())
    }

    #[test]
    fn emit_default_arg_internal_canonical_call_uses_crate_qualified_path() -> Result<(), Box<dyn std::error::Error>> {
        let default_expr = TypedExpr::new(
            IrExprKind::Call {
                func: Box::new(rust_call_target("fallback")),
                type_args: Vec::new(),
                args: Vec::new(),
                callable_signature: None,
                canonical_path: Some(vec!["defaults".to_string(), "fallback".to_string()]),
            },
            IrType::Int,
        );
        let mut registry = FunctionRegistry::new();
        registry.register(
            "combine".to_string(),
            vec![
                FunctionParam {
                    name: "left".to_string(),
                    ty: IrType::Int,
                    mutability: Mutability::Immutable,
                    is_self: false,
                    kind: ParamKind::Normal,
                    default: None,
                },
                FunctionParam {
                    name: "middle".to_string(),
                    ty: IrType::Int,
                    mutability: Mutability::Immutable,
                    is_self: false,
                    kind: ParamKind::Normal,
                    default: Some(default_expr),
                },
            ],
            IrType::Int,
        );
        let mut emitter = IrEmitter::new(&registry);
        emitter.set_internal_module_roots(std::collections::HashSet::from(["defaults".to_string()]));
        let func = local_arg(
            "combine",
            IrType::Function {
                params: vec![IrType::Int, IrType::Int],
                ret: Box::new(IrType::Int),
            },
        );
        let tokens = emitter
            .emit_call_expr(
                &func,
                &[],
                &[pos_arg(TypedExpr::new(IrExprKind::Int(1), IrType::Int))],
                None,
                None,
            )
            .map_err(|err| std::io::Error::other(format!("default arg call should emit: {err:?}")))?;
        assert_eq!(render(tokens), "combine(1,crate::defaults::fallback())");
        Ok(())
    }

    #[test]
    fn emit_canonical_assert_eq_uses_plain_comparison() -> Result<(), Box<dyn std::error::Error>> {
        let registry = FunctionRegistry::new();
        let emitter = IrEmitter::new(&registry);
        let func = rust_call_target("assert_eq");
        let left = local_arg("left", IrType::Int);
        let right = local_arg("right", IrType::Int);
        let path = canonical_testing_path("assert_eq");
        let tokens = emitter
            .emit_call_expr(&func, &[], &[pos_arg(left), pos_arg(right)], None, Some(&path))
            .map_err(|err| std::io::Error::other(format!("canonical assert_eq should emit: {err:?}")))?;
        assert_eq!(render(tokens), "ifleft!=right{panic!(\"AssertionError:left!=right\");}");
        Ok(())
    }

    #[test]
    fn emit_canonical_assert_eq_message_preserves_empty_message_semantics() -> Result<(), Box<dyn std::error::Error>> {
        let registry = FunctionRegistry::new();
        let emitter = IrEmitter::new(&registry);
        let func = rust_call_target("assert_eq");
        let left = local_arg("left", IrType::Int);
        let right = local_arg("right", IrType::Int);
        let msg = local_arg("msg", IrType::String);
        let path = canonical_testing_path("assert_eq");
        let tokens = emitter
            .emit_call_expr(
                &func,
                &[],
                &[pos_arg(left), pos_arg(right), pos_arg(msg)],
                None,
                Some(&path),
            )
            .map_err(|err| std::io::Error::other(format!("canonical assert_eq with message should emit: {err:?}")))?;
        assert_eq!(
            render(tokens),
            "ifleft!=right{{let__incan_assert_msg=msg;if__incan_assert_msg.is_empty(){panic!(\"AssertionError:left!=right\");}else{panic!(\"AssertionError:{};{}\",__incan_assert_msg,\"left!=right\");}}}"
        );
        Ok(())
    }

    #[test]
    fn emit_canonical_assert_eq_parenthesizes_comparison_operands() -> Result<(), Box<dyn std::error::Error>> {
        let registry = FunctionRegistry::new();
        let emitter = IrEmitter::new(&registry);
        let func = rust_call_target("assert_eq");
        let comparison = TypedExpr::new(
            IrExprKind::BinOp {
                op: BinOp::Gt,
                left: Box::new(local_arg("encoded", IrType::Int)),
                right: Box::new(TypedExpr::new(IrExprKind::Int(0), IrType::Int)),
            },
            IrType::Bool,
        );
        let path = canonical_testing_path("assert_eq");
        let tokens = emitter
            .emit_call_expr(
                &func,
                &[],
                &[
                    pos_arg(comparison),
                    pos_arg(TypedExpr::new(IrExprKind::Bool(true), IrType::Bool)),
                ],
                None,
                Some(&path),
            )
            .map_err(|err| {
                std::io::Error::other(format!(
                    "canonical assert_eq with comparison operand should emit: {err:?}"
                ))
            })?;
        assert_eq!(
            render(tokens),
            "if(encoded>0)!=true{panic!(\"AssertionError:left!=right\");}"
        );
        Ok(())
    }

    #[test]
    fn emit_canonical_assert_ne_reuses_string_binop_plan() -> Result<(), Box<dyn std::error::Error>> {
        let registry = FunctionRegistry::new();
        let emitter = IrEmitter::new(&registry);
        let func = rust_call_target("assert_ne");
        let left = local_arg("value", IrType::Ref(Box::new(IrType::String)));
        let right = local_arg("target", IrType::String);
        let path = canonical_testing_path("assert_ne");
        let tokens = emitter
            .emit_call_expr(&func, &[], &[pos_arg(left), pos_arg(right)], None, Some(&path))
            .map_err(|err| std::io::Error::other(format!("canonical assert_ne should emit: {err:?}")))?;
        assert_eq!(
            render(tokens),
            "ifincan_stdlib::strings::str_eq(&value,&target){panic!(\"AssertionError:left==right\");}"
        );
        Ok(())
    }

    #[test]
    fn emit_binop_parenthesizes_lower_precedence_right_operand() -> Result<(), Box<dyn std::error::Error>> {
        let registry = FunctionRegistry::new();
        let emitter = IrEmitter::new(&registry);
        let right = TypedExpr::new(
            IrExprKind::BinOp {
                op: BinOp::Or,
                left: Box::new(local_arg("right_a", IrType::Bool)),
                right: Box::new(local_arg("right_b", IrType::Bool)),
            },
            IrType::Bool,
        );
        let tokens = emitter
            .emit_binop_expr(&BinOp::And, &local_arg("left", IrType::Bool), &right)
            .map_err(|err| std::io::Error::other(format!("logical binop should emit: {err:?}")))?;

        assert_eq!(render(tokens), "left&&(right_a||right_b)");
        Ok(())
    }

    #[test]
    fn emit_canonical_assert_is_some_returns_unwrapped_value() -> Result<(), Box<dyn std::error::Error>> {
        let registry = FunctionRegistry::new();
        let emitter = IrEmitter::new(&registry);
        let func = rust_call_target("assert_is_some");
        let option = local_arg("maybe", IrType::Option(Box::new(IrType::Int)));
        let path = canonical_testing_path("assert_is_some");
        let tokens = emitter
            .emit_call_expr(&func, &[], &[pos_arg(option)], None, Some(&path))
            .map_err(|err| std::io::Error::other(format!("canonical assert_is_some should emit: {err:?}")))?;
        let rendered = render(tokens);
        assert!(
            rendered.contains("match__incan_assert_value{Some(__incan_assert_inner)=>__incan_assert_inner"),
            "Expected assert_is_some match expression, got {rendered}"
        );
        assert!(
            rendered.contains("panic!(\"AssertionError:expectedSome,gotNone\")"),
            "Expected default assertion failure, got {rendered}"
        );
        Ok(())
    }

    #[test]
    fn emit_canonical_assert_is_none_accepts_bare_none_literal() -> Result<(), Box<dyn std::error::Error>> {
        let registry = FunctionRegistry::new();
        let emitter = IrEmitter::new(&registry);
        let func = rust_call_target("assert_is_none");
        let none = TypedExpr::new(IrExprKind::None, IrType::Option(Box::new(IrType::Unknown)));
        let path = canonical_testing_path("assert_is_none");
        let tokens = emitter
            .emit_call_expr(&func, &[], &[pos_arg(none)], None, Some(&path))
            .map_err(|err| std::io::Error::other(format!("canonical assert_is_none should emit: {err:?}")))?;
        assert_eq!(render(tokens), "()");
        Ok(())
    }

    #[test]
    fn emit_canonical_assert_is_ok_accepts_bare_ok_literal() -> Result<(), Box<dyn std::error::Error>> {
        let registry = FunctionRegistry::new();
        let emitter = IrEmitter::new(&registry);
        let func = rust_call_target("assert_is_ok");
        let ok = result_constructor_call(
            ConstructorId::Ok,
            TypedExpr::new(IrExprKind::Int(42), IrType::Int),
            IrType::Result(Box::new(IrType::Int), Box::new(IrType::Unknown)),
        );
        let ok = TypedExpr::new(
            IrExprKind::InteropCoerce {
                expr: Box::new(ok),
                from_ty: IrType::Result(Box::new(IrType::Int), Box::new(IrType::Unknown)),
                to_ty: IrType::Result(Box::new(IrType::Int), Box::new(IrType::Unknown)),
                kind: IrInteropCoercionKind::RustTypeUnwrap,
            },
            IrType::Result(Box::new(IrType::Int), Box::new(IrType::Unknown)),
        );
        let path = canonical_testing_path("assert_is_ok");
        let tokens = emitter
            .emit_call_expr(&func, &[], &[pos_arg(ok)], None, Some(&path))
            .map_err(|err| std::io::Error::other(format!("canonical assert_is_ok should emit: {err:?}")))?;
        assert_eq!(render(tokens), "42");
        Ok(())
    }

    #[test]
    fn emit_canonical_assert_is_err_accepts_bare_err_literal() -> Result<(), Box<dyn std::error::Error>> {
        let registry = FunctionRegistry::new();
        let emitter = IrEmitter::new(&registry);
        let func = rust_call_target("assert_is_err");
        let err = result_constructor_call(
            ConstructorId::Err,
            TypedExpr::new(IrExprKind::String("boom".to_string()), IrType::String),
            IrType::Result(Box::new(IrType::Unknown), Box::new(IrType::String)),
        );
        let path = canonical_testing_path("assert_is_err");
        let tokens = emitter
            .emit_call_expr(&func, &[], &[pos_arg(err)], None, Some(&path))
            .map_err(|err| std::io::Error::other(format!("canonical assert_is_err should emit: {err:?}")))?;
        assert_eq!(render(tokens), "(\"boom\").to_string()");
        Ok(())
    }

    #[test]
    fn emit_call_expr_keeps_return_context_union_string_seed_as_union_value() -> Result<(), Box<dyn std::error::Error>>
    {
        let union_ty = IrType::NamedGeneric(
            IR_UNION_TYPE_NAME.to_string(),
            vec![IrType::String, IrType::Bool, IrType::Float, IrType::Int],
        );
        let mut registry = FunctionRegistry::new();
        registry.register(
            "lit".to_string(),
            vec![FunctionParam {
                name: "value".to_string(),
                ty: union_ty.clone(),
                mutability: Mutability::Immutable,
                is_self: false,
                kind: ParamKind::Normal,
                default: None,
            }],
            IrType::String,
        );
        let emitter = IrEmitter::new(&registry);
        emitter.in_return_context.replace(true);
        let func = TypedExpr::new(
            IrExprKind::Var {
                name: "lit".to_string(),
                access: VarAccess::Copy,
                ref_kind: VarRefKind::Value,
            },
            IrType::Function {
                params: vec![union_ty],
                ret: Box::new(IrType::String),
            },
        );
        let arg = TypedExpr::new(IrExprKind::String("open".to_string()), IrType::String);
        let tokens = emitter
            .emit_call_expr(&func, &[], &[pos_arg(arg)], None, None)
            .map_err(|err| {
                std::io::Error::other(format!(
                    "union string literal call should emit without post-wrapper coercion: {err:?}"
                ))
            })?;

        assert_eq!(
            render(tokens),
            "lit(__IncanUnion43fbd19e99c1db05::V0(\"open\".to_string()))"
        );
        Ok(())
    }

    #[test]
    fn emit_public_union_call_result_as_option_payload_issue745() -> Result<(), Box<dyn std::error::Error>> {
        let union_ty = IrType::NamedGeneric(IR_UNION_TYPE_NAME.to_string(), vec![IrType::Int, IrType::String]);
        let public_union_ty = IrType::RustDisplay(format!(
            "querykit::{}",
            union_ty.union_type_name().ok_or("expected anonymous union type name")?
        ));
        let lit_signature = FunctionSignature {
            params: Vec::new(),
            return_type: union_ty.clone(),
        };
        let lit = TypedExpr::new(
            IrExprKind::Call {
                func: Box::new(local_arg(
                    "lit",
                    IrType::Function {
                        params: Vec::new(),
                        ret: Box::new(public_union_ty.clone()),
                    },
                )),
                type_args: Vec::new(),
                args: Vec::new(),
                callable_signature: Some(lit_signature),
                canonical_path: Some(vec!["pub".to_string(), "querykit".to_string(), "lit".to_string()]),
            },
            public_union_ty.clone(),
        );
        let accept_signature = FunctionSignature {
            params: vec![FunctionParam {
                name: "value".to_string(),
                ty: IrType::Option(Box::new(union_ty)),
                mutability: Mutability::Immutable,
                is_self: false,
                kind: ParamKind::Normal,
                default: None,
            }],
            return_type: IrType::Unit,
        };
        let registry = FunctionRegistry::new();
        let emitter = IrEmitter::new(&registry);
        let accept = local_arg(
            "accept_optional",
            IrType::Function {
                params: vec![IrType::Option(Box::new(public_union_ty))],
                ret: Box::new(IrType::Unit),
            },
        );
        let path = vec!["pub".to_string(), "querykit".to_string(), "accept_optional".to_string()];
        let tokens = emitter
            .emit_call_expr(&accept, &[], &[pos_arg(lit)], Some(&accept_signature), Some(&path))
            .map_err(|err| {
                std::io::Error::other(format!(
                    "public union call result should emit as an Option payload: {err:?}"
                ))
            })?;

        assert_eq!(render(tokens), "querykit::accept_optional(Some(querykit::lit()))");
        Ok(())
    }

    #[test]
    fn emit_call_expr_borrows_struct_arg_for_rust_ref_param() -> Result<(), Box<dyn std::error::Error>> {
        let mut registry = FunctionRegistry::new();
        registry.register(
            "takes_ref".to_string(),
            vec![FunctionParam {
                name: "value".to_string(),
                ty: IrType::Ref(Box::new(IrType::Struct("demo::Thing".to_string()))),
                mutability: Mutability::Immutable,
                is_self: false,
                kind: ParamKind::Normal,
                default: None,
            }],
            IrType::Unit,
        );
        let emitter = IrEmitter::new(&registry);
        let func = rust_call_target("takes_ref");
        let arg = local_arg("thing", IrType::Struct("demo::Thing".to_string()));
        let tokens = emitter
            .emit_call_expr(
                &func,
                &[],
                &[IrCallArg {
                    name: None,
                    kind: IrCallArgKind::Positional,
                    expr: arg,
                }],
                None,
                None,
            )
            .map_err(|err| {
                std::io::Error::other(format!(
                    "emit_call_expr should succeed for borrowed rust arg regression: {err:?}"
                ))
            })?;
        assert_eq!(render(tokens), "takes_ref(&thing)");
        Ok(())
    }

    #[test]
    fn emit_call_expr_borrows_mutably_for_rust_refmut_param() -> Result<(), Box<dyn std::error::Error>> {
        let mut registry = FunctionRegistry::new();
        registry.register(
            "takes_ref_mut".to_string(),
            vec![FunctionParam {
                name: "value".to_string(),
                ty: IrType::RefMut(Box::new(IrType::Struct("demo::Thing".to_string()))),
                mutability: Mutability::Mutable,
                is_self: false,
                kind: ParamKind::Normal,
                default: None,
            }],
            IrType::Unit,
        );
        let emitter = IrEmitter::new(&registry);
        let func = rust_call_target("takes_ref_mut");
        let arg = local_arg("thing", IrType::Struct("demo::Thing".to_string()));
        let tokens = emitter
            .emit_call_expr(
                &func,
                &[],
                &[IrCallArg {
                    name: None,
                    kind: IrCallArgKind::Positional,
                    expr: arg,
                }],
                None,
                None,
            )
            .map_err(|err| {
                std::io::Error::other(format!(
                    "emit_call_expr should succeed for mutable borrowed rust arg regression: {err:?}"
                ))
            })?;
        assert_eq!(render(tokens), "takes_ref_mut(&mutthing)");
        Ok(())
    }

    #[test]
    fn emit_canonical_call_prefers_callable_signature_over_local_registry() -> Result<(), Box<dyn std::error::Error>> {
        let byte_list = IrType::List(Box::new(IrType::Numeric(NumericTypeId::U8)));
        let mut registry = FunctionRegistry::new();
        registry.register(
            "_append_bytes".to_string(),
            vec![FunctionParam {
                name: "out".to_string(),
                ty: byte_list.clone(),
                mutability: Mutability::Immutable,
                is_self: false,
                kind: ParamKind::Normal,
                default: None,
            }],
            IrType::Unit,
        );
        let signature = FunctionSignature {
            params: vec![
                FunctionParam {
                    name: "out".to_string(),
                    ty: byte_list.clone(),
                    mutability: Mutability::Mutable,
                    is_self: false,
                    kind: ParamKind::Normal,
                    default: None,
                },
                FunctionParam {
                    name: "data".to_string(),
                    ty: IrType::Bytes,
                    mutability: Mutability::Immutable,
                    is_self: false,
                    kind: ParamKind::Normal,
                    default: None,
                },
            ],
            return_type: IrType::Unit,
        };
        let emitter = IrEmitter::new(&registry);
        let func = local_arg(
            "_append_bytes",
            IrType::Function {
                params: vec![byte_list, IrType::Bytes],
                ret: Box::new(IrType::Unit),
            },
        );
        let out = local_arg("out", IrType::List(Box::new(IrType::Numeric(NumericTypeId::U8))));
        let data = local_arg("data", IrType::Bytes);
        let path = vec![
            "std".to_string(),
            "collections".to_string(),
            "_append_bytes".to_string(),
        ];
        let tokens = emitter
            .emit_call_expr(
                &func,
                &[],
                &[pos_arg(out), pos_arg(data)],
                Some(&signature),
                Some(&path),
            )
            .map_err(|err| std::io::Error::other(format!("canonical mutable stdlib call should emit: {err:?}")))?;
        assert_eq!(
            render(tokens),
            "crate::__incan_std::collections::_append_bytes(&mutout,data.clone())"
        );
        Ok(())
    }

    #[test]
    fn emit_call_expr_borrows_copy_arg_for_rust_ref_param() -> Result<(), Box<dyn std::error::Error>> {
        let mut registry = FunctionRegistry::new();
        registry.register(
            "takes_ref".to_string(),
            vec![FunctionParam {
                name: "value".to_string(),
                ty: IrType::Ref(Box::new(IrType::Int)),
                mutability: Mutability::Immutable,
                is_self: false,
                kind: ParamKind::Normal,
                default: None,
            }],
            IrType::Unit,
        );
        let emitter = IrEmitter::new(&registry);
        let func = rust_call_target("takes_ref");
        let arg = local_arg("value", IrType::Int);
        let tokens = emitter
            .emit_call_expr(
                &func,
                &[],
                &[IrCallArg {
                    name: None,
                    kind: IrCallArgKind::Positional,
                    expr: arg,
                }],
                None,
                None,
            )
            .map_err(|err| {
                std::io::Error::other(format!("emit_call_expr should borrow copy args for rust refs: {err:?}"))
            })?;
        assert_eq!(render(tokens), "takes_ref(&value)");
        Ok(())
    }

    #[test]
    fn emit_call_expr_borrows_args_from_typed_rust_callee_without_registry() -> Result<(), Box<dyn std::error::Error>> {
        let registry = FunctionRegistry::new();
        let emitter = IrEmitter::new(&registry);
        let func = typed_rust_call_target(
            "consume",
            vec![
                IrType::Ref(Box::new(IrType::Struct("demo::State".to_string()))),
                IrType::Ref(Box::new(IrType::Struct("demo::Plan".to_string()))),
            ],
        );
        let state = local_arg("state", IrType::Struct("demo::State".to_string()));
        let plan = local_arg("plan", IrType::Struct("demo::Plan".to_string()));
        let tokens = emitter
            .emit_call_expr(
                &func,
                &[],
                &[
                    IrCallArg {
                        name: None,
                        kind: IrCallArgKind::Positional,
                        expr: state,
                    },
                    IrCallArg {
                        name: None,
                        kind: IrCallArgKind::Positional,
                        expr: plan,
                    },
                ],
                None,
                None,
            )
            .map_err(|err| {
                std::io::Error::other(format!(
                    "emit_call_expr should borrow args from typed rust callees: {err:?}"
                ))
            })?;
        assert_eq!(render(tokens), "consume(&state,&plan)");
        Ok(())
    }

    #[test]
    fn rest_aware_call_arg_uses_argument_plan_without_double_borrow() -> Result<(), Box<dyn std::error::Error>> {
        let registry = FunctionRegistry::new();
        let emitter = IrEmitter::new(&registry);
        let func = rust_call_target("takes_ref_rest");
        let signature = FunctionSignature {
            params: vec![
                FunctionParam {
                    name: "value".to_string(),
                    ty: IrType::Ref(Box::new(IrType::Struct("demo::Thing".to_string()))),
                    mutability: Mutability::Immutable,
                    is_self: false,
                    kind: ParamKind::Normal,
                    default: None,
                },
                FunctionParam {
                    name: "rest".to_string(),
                    ty: IrType::List(Box::new(IrType::Int)),
                    mutability: Mutability::Immutable,
                    is_self: false,
                    kind: ParamKind::RestPositional,
                    default: None,
                },
            ],
            return_type: IrType::Unit,
        };
        let arg = local_arg("value", IrType::Struct("demo::Thing".to_string()));
        let tokens = emitter
            .emit_call_expr(&func, &[], &[pos_arg(arg)], Some(&signature), None)
            .map_err(|err| std::io::Error::other(format!("rest-aware call should emit borrowed arg: {err:?}")))?;
        let rendered = render(tokens);
        assert!(rendered.starts_with("takes_ref_rest(&value,"));
        assert!(
            !rendered.contains("&&value"),
            "argument plan must not add a second borrow after emit_expr_for_use: {rendered}"
        );
        Ok(())
    }

    #[test]
    fn emit_canonical_assert_raises_catches_panic_payloads() -> Result<(), Box<dyn std::error::Error>> {
        let registry = FunctionRegistry::new();
        let emitter = IrEmitter::new(&registry);
        let func = rust_call_target("assert_raises");
        let raising_call = TypedExpr::new(
            IrExprKind::Call {
                func: Box::new(rust_call_target("explode")),
                type_args: Vec::new(),
                args: Vec::new(),
                callable_signature: None,
                canonical_path: None,
            },
            IrType::Unit,
        );
        let expected = TypedExpr::new(
            IrExprKind::Literal(IrLiteral::StaticStr("ValueError".to_string())),
            IrType::StaticStr,
        );
        let path = canonical_testing_path("assert_raises");
        let tokens = emitter
            .emit_call_expr(
                &func,
                &[],
                &[pos_arg(raising_call), pos_arg(expected)],
                None,
                Some(&path),
            )
            .map_err(|err| std::io::Error::other(format!("canonical assert_raises should emit: {err:?}")))?;
        let rendered = render(tokens);
        assert!(rendered.contains("std::panic::catch_unwind"));
        assert!(rendered.contains("\"ValueError\""));
        assert!(rendered.contains("starts_with"));
        assert!(rendered.contains("AssertionError:expected{}toberaised"));
        Ok(())
    }

    #[test]
    fn emit_canonical_assert_raises_invokes_zero_arg_function_argument() -> Result<(), Box<dyn std::error::Error>> {
        let registry = FunctionRegistry::new();
        let emitter = IrEmitter::new(&registry);
        let func = rust_call_target("assert_raises");
        let block = local_arg(
            "bad_parse",
            IrType::Function {
                params: Vec::new(),
                ret: Box::new(IrType::Unit),
            },
        );
        let expected = TypedExpr::new(
            IrExprKind::Literal(IrLiteral::StaticStr("ValueError".to_string())),
            IrType::StaticStr,
        );
        let path = canonical_testing_path("assert_raises");
        let tokens = emitter
            .emit_call_expr(&func, &[], &[pos_arg(block), pos_arg(expected)], None, Some(&path))
            .map_err(|err| std::io::Error::other(format!("canonical assert_raises should emit: {err:?}")))?;
        assert!(render(tokens).contains("bad_parse()"));
        Ok(())
    }
}
