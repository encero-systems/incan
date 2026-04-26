//! Emit Rust code for function calls and binary operations.
//!
//! This module handles emission of regular function calls (user-defined functions) and binary operator expressions.

use proc_macro2::TokenStream;
use quote::quote;

use super::super::super::conversions::{BinOpEmitKind, determine_binop_plan};
use super::super::super::expr::{BinOp, IrCallArg, IrExprKind, TypedExpr, VarAccess, VarRefKind};
use super::super::super::ownership::{ValueUseSite, incan_call_arg_needs_rust_mut_borrow, plan_value_use};
use super::super::super::types::IrType;
use super::super::{EmitError, IrEmitter};
use incan_core::lang::stdlib;
use incan_core::lang::surface::constructors::{self, ConstructorId};

const INTERNAL_PANIC_FN: &str = "__incan_internal_panic";

impl<'a> IrEmitter<'a> {
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
    fn is_unresolved_call_seed_type(ty: &IrType) -> bool {
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
        canonical_path: Option<&[String]>,
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
        let callee_name = local_name.or(canonical_name);
        let function_sig = local_name
            .and_then(|name| self.function_registry.get(name))
            .or_else(|| canonical_name.and_then(|name| self.function_registry.get(name)));

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

        let f = if let Some(path) = canonical_path {
            self.emit_canonical_callee_path(path)?.unwrap_or(self.emit_expr(func)?)
        } else {
            self.emit_expr(func)?
        };
        let turbofish = if type_args.is_empty() {
            quote! {}
        } else {
            let emitted: Vec<TokenStream> = type_args.iter().map(|ty| self.emit_type(ty)).collect();
            quote! { ::<#(#emitted),*> }
        };

        // Order arguments only when keyword args are present (positional-only calls preserve previous behavior,
        // which is important for snapshots + for default-arg lowering work that happens elsewhere).
        let has_named_args = args.iter().any(|a| a.name.is_some());
        let ordered_args: Vec<TypedExpr> = if has_named_args {
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
                let mut out: Vec<TypedExpr> = Vec::new();
                for p in &sig.params {
                    if let Some(v) = named.get(p.name.as_str()) {
                        out.push(v.clone());
                    } else if pos_idx < positional.len() {
                        out.push(positional[pos_idx].clone());
                        pos_idx += 1;
                    } else if let Some(default_arg) = &p.default {
                        out.push(default_arg.clone());
                    }
                }
                out
            } else {
                args.iter().map(|a| a.expr.clone()).collect()
            }
        } else {
            let mut out: Vec<TypedExpr> = args.iter().map(|a| a.expr.clone()).collect();
            if let Some(sig) = function_sig {
                for p in sig.params.iter().skip(out.len()) {
                    if let Some(default_arg) = &p.default {
                        out.push(default_arg.clone());
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
            .map(|(idx, a)| {
                let target_ty = function_sig
                    .and_then(|sig| sig.params.get(idx))
                    .map(|param| &param.ty)
                    .or_else(|| match &func.ty {
                        IrType::Function { params, .. } => params.get(idx),
                        _ => None,
                    });
                let emitted = if let Some(target_ty) = target_ty {
                    if let Some(seed) = self.emit_inference_seeded_literal_arg(a, target_ty)? {
                        seed
                    } else if Self::is_unresolved_call_seed_type(target_ty) {
                        // Signature exists but leaves generics unresolved: fallback to the argument's own inferred IR
                        // type to seed constructor literals.
                        if let Some(seed) = self.emit_inference_seeded_literal_arg(a, &a.ty)? {
                            seed
                        } else {
                            self.emit_expr(a)?
                        }
                    } else {
                        self.emit_expr(a)?
                    }
                } else {
                    // No parameter type available (e.g. heavily generic paths): use the argument's own type as a
                    // best-effort inference seed source.
                    if let Some(seed) = self.emit_inference_seeded_literal_arg(a, &a.ty)? {
                        seed
                    } else {
                        self.emit_expr(a)?
                    }
                };

                // Check VarAccess for explicit borrow requirements
                if let IrExprKind::Var { access, .. } = &a.kind {
                    match access {
                        VarAccess::BorrowMut => return Ok(quote! { &mut #emitted }),
                        VarAccess::Borrow => return Ok(quote! { &#emitted }),
                        _ => {}
                    }
                }

                // Prefer explicit lowering access decisions, then derive obvious borrow requirements from parameter
                // typing information.
                let sig_param = function_sig.and_then(|sig| sig.params.get(idx));
                if let Some(param) = sig_param {
                    match &param.ty {
                        IrType::Ref(_) => match &a.ty {
                            IrType::Ref(_) | IrType::RefMut(_) => return Ok(emitted),
                            _ => return Ok(quote! { &#emitted }),
                        },
                        IrType::RefMut(_) => match &a.ty {
                            IrType::Ref(_) | IrType::RefMut(_) => return Ok(emitted),
                            _ => return Ok(quote! { &mut #emitted }),
                        },
                        _ => {}
                    }
                } else if let Some(target_ty) = target_ty {
                    // Toward #121: when registry metadata is unavailable, use the call expression's function type as a
                    // borrow hint.
                    match target_ty {
                        IrType::RefMut(_) => match &a.ty {
                            IrType::Ref(_) | IrType::RefMut(_) => return Ok(emitted),
                            _ => return Ok(quote! { &mut #emitted }),
                        },
                        IrType::Ref(_) => match &a.ty {
                            IrType::Ref(_) | IrType::RefMut(_) => return Ok(emitted),
                            _ => return Ok(quote! { &#emitted }),
                        },
                        _ => {}
                    }
                }

                // Determine conversion context based on whether this is an Incan or Rust function
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

                let mut tokens = plan_value_use(a, use_site).apply(emitted);
                if let Some(param) = sig_param
                    && incan_call_arg_needs_rust_mut_borrow(param)
                {
                    match &a.ty {
                        IrType::Ref(_) | IrType::RefMut(_) => {}
                        _ => tokens = quote! { &mut #tokens },
                    }
                }
                Ok(tokens)
            })
            .collect::<Result<_, _>>()?;

        Ok(quote! { #f #turbofish (#(#arg_tokens),*) })
    }

    /// Emit canonical RFC 018 assertion helper calls without requiring a source-level `std.testing` import.
    ///
    /// Plain `assert` is a language primitive, so its lowered helper calls must remain available even when the
    /// explicit stdlib testing module was not imported into the user's source file.
    fn try_emit_testing_assert_call(
        &self,
        canonical_path: Option<&[String]>,
        args: &[IrCallArg],
    ) -> Result<Option<TokenStream>, EmitError> {
        let Some(path) = canonical_path else {
            return Ok(None);
        };
        if path.len() != 3
            || path.first().map(String::as_str) != Some(stdlib::STDLIB_ROOT)
            || path.get(1).map(String::as_str) != Some("testing")
        {
            return Ok(None);
        }
        let Some(name) = path.last().map(String::as_str) else {
            return Ok(None);
        };

        match name {
            "assert" => {
                let condition = Self::canonical_assert_arg(name, args, 0)?;
                let condition_tokens = self.emit_expr(condition)?;
                let failure = self.emit_assert_failure("AssertionError", args.get(1).map(|arg| &arg.expr))?;
                Ok(Some(quote! {
                    if !(#condition_tokens) {
                        #failure
                    }
                }))
            }
            "assert_false" => {
                let condition = Self::canonical_assert_arg(name, args, 0)?;
                let condition_tokens = self.emit_expr(condition)?;
                let failure = self.emit_assert_failure("AssertionError", args.get(1).map(|arg| &arg.expr))?;
                Ok(Some(quote! {
                    if #condition_tokens {
                        #failure
                    }
                }))
            }
            "assert_eq" | "assert_ne" => self.emit_assert_comparison(name, args).map(Some),
            "assert_is_some" => self.emit_assert_option_some(args).map(Some),
            "assert_is_none" => self.emit_assert_option_none(args).map(Some),
            "assert_is_ok" => self.emit_assert_result_ok(args).map(Some),
            "assert_is_err" => self.emit_assert_result_err(args).map(Some),
            "assert_raises" => self.emit_assert_raises(args).map(Some),
            _ => Ok(None),
        }
    }

    fn canonical_assert_arg<'b>(
        helper_name: &str,
        args: &'b [IrCallArg],
        index: usize,
    ) -> Result<&'b TypedExpr, EmitError> {
        args.get(index).map(|arg| &arg.expr).ok_or_else(|| {
            EmitError::Unsupported(format!(
                "canonical std.testing.{helper_name} call missing argument {}",
                index + 1
            ))
        })
    }

    fn result_constructor_payload(expr: &TypedExpr, constructor: ConstructorId) -> Option<&TypedExpr> {
        let expr = match &expr.kind {
            IrExprKind::InteropCoerce { expr, .. } => expr.as_ref(),
            _ => expr,
        };
        if let IrExprKind::Struct { name, fields } = &expr.kind
            && name == constructors::as_str(constructor)
        {
            return fields.first().map(|(_, payload)| payload);
        }
        let IrExprKind::Call { func, args, .. } = &expr.kind else {
            return None;
        };
        let IrExprKind::Var { name, .. } = &func.kind else {
            return None;
        };
        if name != constructors::as_str(constructor) {
            return None;
        }
        args.first().map(|arg| &arg.expr)
    }

    fn emit_assert_failure(
        &self,
        default_message: &'static str,
        message: Option<&TypedExpr>,
    ) -> Result<TokenStream, EmitError> {
        if let Some(message) = message {
            let message_tokens = self.emit_expr(message)?;
            return Ok(quote! {{
                let __incan_assert_msg = #message_tokens;
                if __incan_assert_msg.is_empty() {
                    panic!(#default_message);
                } else {
                    panic!("AssertionError: {}", __incan_assert_msg);
                }
            }});
        }
        Ok(quote! { panic!(#default_message); })
    }

    fn emit_assert_raises_failure(
        &self,
        default_message: TokenStream,
        message: Option<&TypedExpr>,
    ) -> Result<TokenStream, EmitError> {
        if let Some(message) = message {
            let message_tokens = self.emit_expr(message)?;
            return Ok(quote! {{
                let __incan_assert_msg = #message_tokens;
                if __incan_assert_msg.is_empty() {
                    #default_message
                } else {
                    panic!("AssertionError: {}", __incan_assert_msg);
                }
            }});
        }
        Ok(default_message)
    }

    fn emit_assert_comparison_failure(
        &self,
        failure_kind: &'static str,
        message: Option<&TypedExpr>,
    ) -> Result<TokenStream, EmitError> {
        let default_message = format!("AssertionError: {failure_kind}");
        if let Some(message) = message {
            let message_tokens = self.emit_expr(message)?;
            return Ok(quote! {{
                let __incan_assert_msg = #message_tokens;
                if __incan_assert_msg.is_empty() {
                    panic!(#default_message);
                } else {
                    panic!("AssertionError: {}; {}", __incan_assert_msg, #failure_kind);
                }
            }});
        }
        Ok(quote! { panic!(#default_message); })
    }

    fn emit_assert_comparison(&self, name: &str, args: &[IrCallArg]) -> Result<TokenStream, EmitError> {
        let left = Self::canonical_assert_arg(name, args, 0)?;
        let right = Self::canonical_assert_arg(name, args, 1)?;
        let left_tokens = self.emit_expr(left)?;
        let right_tokens = self.emit_expr(right)?;
        let message = args.get(2).map(|arg| &arg.expr);
        if name == "assert_eq" {
            let failure = self.emit_assert_comparison_failure("left != right", message)?;
            Ok(quote! {
                if #left_tokens != #right_tokens {
                    #failure
                }
            })
        } else {
            let failure = self.emit_assert_comparison_failure("left == right", message)?;
            Ok(quote! {
                if #left_tokens == #right_tokens {
                    #failure
                }
            })
        }
    }

    fn emit_assert_option_some(&self, args: &[IrCallArg]) -> Result<TokenStream, EmitError> {
        let option = Self::canonical_assert_arg("assert_is_some", args, 0)?;
        let option_tokens = self.emit_expr(option)?;
        let failure = self.emit_assert_failure(
            "AssertionError: expected Some, got None",
            args.get(1).map(|arg| &arg.expr),
        )?;
        Ok(quote! {{
            let __incan_assert_value = #option_tokens;
            match __incan_assert_value {
                Some(__incan_assert_inner) => __incan_assert_inner,
                None => {
                    #failure
                }
            }
        }})
    }

    fn emit_assert_option_none(&self, args: &[IrCallArg]) -> Result<TokenStream, EmitError> {
        let option = Self::canonical_assert_arg("assert_is_none", args, 0)?;
        if matches!(option.kind, IrExprKind::None) {
            return Ok(quote! { () });
        }
        let option_tokens = self.emit_expr(option)?;
        let failure = self.emit_assert_failure(
            "AssertionError: expected None, got Some",
            args.get(1).map(|arg| &arg.expr),
        )?;
        Ok(quote! {{
            let __incan_assert_value = #option_tokens;
            if __incan_assert_value.is_some() {
                #failure
            }
        }})
    }

    fn emit_assert_result_ok(&self, args: &[IrCallArg]) -> Result<TokenStream, EmitError> {
        let result = Self::canonical_assert_arg("assert_is_ok", args, 0)?;
        if let Some(payload) = Self::result_constructor_payload(result, ConstructorId::Ok) {
            let payload_tokens = Self::emit_result_payload_tokens(payload, self.emit_expr(payload)?);
            return Ok(quote! { #payload_tokens });
        }
        let result_tokens = self.emit_expr(result)?;
        let failure =
            self.emit_assert_failure("AssertionError: expected Ok, got Err", args.get(1).map(|arg| &arg.expr))?;
        Ok(quote! {{
            let __incan_assert_value = #result_tokens;
            match __incan_assert_value {
                Ok(__incan_assert_inner) => __incan_assert_inner,
                Err(_) => {
                    #failure
                }
            }
        }})
    }

    fn emit_assert_result_err(&self, args: &[IrCallArg]) -> Result<TokenStream, EmitError> {
        let result = Self::canonical_assert_arg("assert_is_err", args, 0)?;
        if let Some(payload) = Self::result_constructor_payload(result, ConstructorId::Err) {
            let payload_tokens = Self::emit_result_payload_tokens(payload, self.emit_expr(payload)?);
            return Ok(quote! { #payload_tokens });
        }
        let result_tokens = self.emit_expr(result)?;
        let failure =
            self.emit_assert_failure("AssertionError: expected Err, got Ok", args.get(1).map(|arg| &arg.expr))?;
        Ok(quote! {{
            let __incan_assert_value = #result_tokens;
            match __incan_assert_value {
                Err(__incan_assert_inner) => __incan_assert_inner,
                Ok(_) => {
                    #failure
                }
            }
        }})
    }

    fn emit_assert_raises(&self, args: &[IrCallArg]) -> Result<TokenStream, EmitError> {
        let call = Self::canonical_assert_arg("assert_raises", args, 0)?;
        let expected = Self::canonical_assert_arg("assert_raises", args, 1)?;
        let call_tokens = self.emit_expr(call)?;
        let invocation_tokens = if matches!(
            &call.ty,
            IrType::Function { params, ret } if params.is_empty() && matches!(ret.as_ref(), IrType::Unit)
        ) {
            quote! { #call_tokens() }
        } else {
            quote! { #call_tokens }
        };
        let expected_tokens = self.emit_expr(expected)?;
        let no_raise = self.emit_assert_raises_failure(
            quote! { panic!("AssertionError: expected {} to be raised", __incan_expected_error); },
            args.get(2).map(|arg| &arg.expr),
        )?;
        let wrong_error = self.emit_assert_raises_failure(
            quote! {
                panic!(
                    "AssertionError: expected {} to be raised, got {}",
                    __incan_expected_error,
                    __incan_panic_message
                );
            },
            args.get(2).map(|arg| &arg.expr),
        )?;

        Ok(quote! {{
            let __incan_expected_error = #expected_tokens;
            let __incan_raises_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                #invocation_tokens;
            }));
            match __incan_raises_result {
                Ok(_) => {
                    #no_raise
                }
                Err(__incan_payload) => {
                    let __incan_panic_message = if let Some(message) = __incan_payload.downcast_ref::<String>() {
                        message.as_str()
                    } else if let Some(message) = __incan_payload.downcast_ref::<&str>() {
                        *message
                    } else {
                        ""
                    };
                    let __incan_expected_prefix = format!("{}:", __incan_expected_error);
                    if __incan_panic_message != __incan_expected_error
                        && !__incan_panic_message.starts_with(&__incan_expected_prefix)
                    {
                        #wrong_error
                    }
                }
            }
        }})
    }

    fn emit_canonical_callee_path(&self, canonical_path: &[String]) -> Result<Option<TokenStream>, EmitError> {
        if canonical_path.len() < 3 || canonical_path.first().map(String::as_str) != Some(stdlib::STDLIB_ROOT) {
            return Ok(None);
        }

        let module_path: Vec<String> = canonical_path[..canonical_path.len() - 1].to_vec();
        let Some(function_name) = canonical_path.last() else {
            return Ok(None);
        };
        if !stdlib::is_known_stdlib_module(&module_path) {
            return Ok(None);
        }

        let ns = Self::rust_ident(stdlib::INCAN_STD_NAMESPACE);
        let mut segments: Vec<TokenStream> = vec![quote! { crate }, quote! { #ns }];

        for seg in module_path.iter().skip(1) {
            let ident = Self::rust_ident(seg);
            segments.push(quote! { #ident });
        }
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
        let l = plan.lhs_conv.apply(l_raw);
        let r = plan.rhs_conv.apply(r_raw);

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

                // Handle reference vs value comparisons
                let is_comparison = matches!(
                    op,
                    BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge
                );

                if is_comparison {
                    let left_is_ref = matches!(&left.ty, IrType::Ref(_) | IrType::RefMut(_));
                    let right_is_value = !matches!(&right.ty, IrType::Ref(_) | IrType::RefMut(_));

                    if left_is_ref && right_is_value {
                        return Ok(quote! { *#l #op_tokens #r });
                    }
                }

                Ok(quote! { #l #op_tokens #r })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::ir::decl::FunctionParam;
    use crate::backend::ir::expr::{IrCallArg, IrInteropCoercionKind, Literal as IrLiteral, VarAccess, VarRefKind};
    use crate::backend::ir::types::{IrType, Mutability};
    use crate::backend::ir::{FunctionRegistry, IrEmitter, TypedExpr};

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
    fn emit_canonical_assert_eq_uses_plain_comparison() -> Result<(), Box<dyn std::error::Error>> {
        let registry = FunctionRegistry::new();
        let emitter = IrEmitter::new(&registry);
        let func = rust_call_target("assert_eq");
        let left = local_arg("left", IrType::Int);
        let right = local_arg("right", IrType::Int);
        let path = canonical_testing_path("assert_eq");
        let tokens = emitter
            .emit_call_expr(
                &func,
                &[],
                &[
                    IrCallArg { name: None, expr: left },
                    IrCallArg {
                        name: None,
                        expr: right,
                    },
                ],
                Some(&path),
            )
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
                &[
                    IrCallArg { name: None, expr: left },
                    IrCallArg {
                        name: None,
                        expr: right,
                    },
                    IrCallArg { name: None, expr: msg },
                ],
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
    fn emit_canonical_assert_is_some_returns_unwrapped_value() -> Result<(), Box<dyn std::error::Error>> {
        let registry = FunctionRegistry::new();
        let emitter = IrEmitter::new(&registry);
        let func = rust_call_target("assert_is_some");
        let option = local_arg("maybe", IrType::Option(Box::new(IrType::Int)));
        let path = canonical_testing_path("assert_is_some");
        let tokens = emitter
            .emit_call_expr(
                &func,
                &[],
                &[IrCallArg {
                    name: None,
                    expr: option,
                }],
                Some(&path),
            )
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
            .emit_call_expr(&func, &[], &[IrCallArg { name: None, expr: none }], Some(&path))
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
            .emit_call_expr(&func, &[], &[IrCallArg { name: None, expr: ok }], Some(&path))
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
            .emit_call_expr(&func, &[], &[IrCallArg { name: None, expr: err }], Some(&path))
            .map_err(|err| std::io::Error::other(format!("canonical assert_is_err should emit: {err:?}")))?;
        assert_eq!(render(tokens), "(\"boom\").to_string()");
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
                default: None,
            }],
            IrType::Unit,
        );
        let emitter = IrEmitter::new(&registry);
        let func = rust_call_target("takes_ref");
        let arg = local_arg("thing", IrType::Struct("demo::Thing".to_string()));
        let tokens = emitter
            .emit_call_expr(&func, &[], &[IrCallArg { name: None, expr: arg }], None)
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
                default: None,
            }],
            IrType::Unit,
        );
        let emitter = IrEmitter::new(&registry);
        let func = rust_call_target("takes_ref_mut");
        let arg = local_arg("thing", IrType::Struct("demo::Thing".to_string()));
        let tokens = emitter
            .emit_call_expr(&func, &[], &[IrCallArg { name: None, expr: arg }], None)
            .map_err(|err| {
                std::io::Error::other(format!(
                    "emit_call_expr should succeed for mutable borrowed rust arg regression: {err:?}"
                ))
            })?;
        assert_eq!(render(tokens), "takes_ref_mut(&mutthing)");
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
                default: None,
            }],
            IrType::Unit,
        );
        let emitter = IrEmitter::new(&registry);
        let func = rust_call_target("takes_ref");
        let arg = local_arg("value", IrType::Int);
        let tokens = emitter
            .emit_call_expr(&func, &[], &[IrCallArg { name: None, expr: arg }], None)
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
                        expr: state,
                    },
                    IrCallArg { name: None, expr: plan },
                ],
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
    fn emit_canonical_assert_raises_catches_panic_payloads() -> Result<(), Box<dyn std::error::Error>> {
        let registry = FunctionRegistry::new();
        let emitter = IrEmitter::new(&registry);
        let func = rust_call_target("assert_raises");
        let raising_call = TypedExpr::new(
            IrExprKind::Call {
                func: Box::new(rust_call_target("explode")),
                type_args: Vec::new(),
                args: Vec::new(),
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
                &[
                    IrCallArg {
                        name: None,
                        expr: raising_call,
                    },
                    IrCallArg {
                        name: None,
                        expr: expected,
                    },
                ],
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
            .emit_call_expr(
                &func,
                &[],
                &[
                    IrCallArg {
                        name: None,
                        expr: block,
                    },
                    IrCallArg {
                        name: None,
                        expr: expected,
                    },
                ],
                Some(&path),
            )
            .map_err(|err| std::io::Error::other(format!("canonical assert_raises should emit: {err:?}")))?;
        assert!(render(tokens).contains("bad_parse()"));
        Ok(())
    }
}
