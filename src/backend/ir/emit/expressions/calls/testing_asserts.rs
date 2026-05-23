use proc_macro2::TokenStream;
use quote::quote;

use crate::backend::ir::emit::{EmitError, IrEmitter};
use crate::backend::ir::expr::{IrCallArg, IrExprKind, TypedExpr};
use crate::backend::ir::types::IrType;
use incan_core::lang::surface::constructors::{self, ConstructorId};
use incan_core::lang::testing::{self, TestingAssertHelperId};

impl<'a> IrEmitter<'a> {
    /// Emit canonical RFC 018 assertion helper calls without requiring a source-level `std.testing` import.
    ///
    /// Plain `assert` is a language primitive, so its lowered helper calls must remain available even when the explicit
    /// stdlib testing module was not imported into the user's source file.
    pub(super) fn try_emit_testing_assert_call(
        &self,
        canonical_path: Option<&[String]>,
        args: &[IrCallArg],
    ) -> Result<Option<TokenStream>, EmitError> {
        let Some(path) = canonical_path else {
            return Ok(None);
        };
        let Some(helper_id) = testing::assert_helper_id_from_std_path(path) else {
            return Ok(None);
        };

        match helper_id {
            TestingAssertHelperId::Assert => {
                let condition = Self::canonical_assert_arg(helper_id, args, 0)?;
                let failure = self.emit_assert_failure(
                    Self::assert_failure_message(helper_id)?,
                    args.get(1).map(|arg| &arg.expr),
                )?;
                if Self::constant_bool(condition) == Some(false) {
                    return Ok(Some(failure));
                }
                let condition_tokens = self.emit_expr(condition)?;
                Ok(Some(quote! {
                    if !(#condition_tokens) {
                        #failure
                    }
                }))
            }
            TestingAssertHelperId::AssertFalse => {
                let condition = Self::canonical_assert_arg(helper_id, args, 0)?;
                let failure = self.emit_assert_failure(
                    Self::assert_failure_message(helper_id)?,
                    args.get(1).map(|arg| &arg.expr),
                )?;
                if Self::constant_bool(condition) == Some(true) {
                    return Ok(Some(failure));
                }
                let condition_tokens = self.emit_expr(condition)?;
                Ok(Some(quote! {
                    if #condition_tokens {
                        #failure
                    }
                }))
            }
            TestingAssertHelperId::AssertEq | TestingAssertHelperId::AssertNe => {
                self.emit_assert_comparison(helper_id, args).map(Some)
            }
            TestingAssertHelperId::AssertIsSome => self.emit_assert_option_some(args).map(Some),
            TestingAssertHelperId::AssertIsNone => self.emit_assert_option_none(args).map(Some),
            TestingAssertHelperId::AssertIsOk => self.emit_assert_result_ok(args).map(Some),
            TestingAssertHelperId::AssertIsErr => self.emit_assert_result_err(args).map(Some),
            TestingAssertHelperId::AssertRaises => self.emit_assert_raises(args).map(Some),
        }
    }

    fn constant_bool(expr: &TypedExpr) -> Option<bool> {
        match &expr.kind {
            IrExprKind::Bool(value) => Some(*value),
            IrExprKind::InteropCoerce { expr, .. } => Self::constant_bool(expr),
            _ => None,
        }
    }

    fn canonical_assert_arg(
        helper_id: TestingAssertHelperId,
        args: &[IrCallArg],
        index: usize,
    ) -> Result<&TypedExpr, EmitError> {
        let helper_name = testing::assert_helper_as_str(helper_id);
        args.get(index).map(|arg| &arg.expr).ok_or_else(|| {
            EmitError::Unsupported(format!(
                "canonical std.testing.{helper_name} call missing argument {}",
                index + 1
            ))
        })
    }

    fn assert_failure_message(helper_id: TestingAssertHelperId) -> Result<&'static str, EmitError> {
        testing::assert_helper_default_failure_message(helper_id).ok_or_else(|| {
            EmitError::Unsupported(format!(
                "std.testing.{} does not have a fixed assertion failure message",
                testing::assert_helper_as_str(helper_id)
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

    /// Emit canonical `std.testing.assert_eq` / `assert_ne` calls with expression operands isolated.
    fn emit_assert_comparison(
        &self,
        helper_id: TestingAssertHelperId,
        args: &[IrCallArg],
    ) -> Result<TokenStream, EmitError> {
        let name = testing::assert_helper_as_str(helper_id);
        let left = Self::canonical_assert_arg(helper_id, args, 0)?;
        let right = Self::canonical_assert_arg(helper_id, args, 1)?;
        let left_tokens = self.emit_expr(left)?;
        let right_tokens = self.emit_expr(right)?;
        let message = args.get(2).map(|arg| &arg.expr);
        let failure_kind = testing::assert_comparison_failure_kind(helper_id).ok_or_else(|| {
            EmitError::Unsupported(format!("std.testing.{name} is not a comparison assertion helper"))
        })?;
        if helper_id == TestingAssertHelperId::AssertEq {
            let failure = self.emit_assert_comparison_failure(failure_kind, message)?;
            Ok(quote! {
                if (#left_tokens) != (#right_tokens) {
                    #failure
                }
            })
        } else {
            let failure = self.emit_assert_comparison_failure(failure_kind, message)?;
            Ok(quote! {
                if (#left_tokens) == (#right_tokens) {
                    #failure
                }
            })
        }
    }

    fn emit_assert_option_some(&self, args: &[IrCallArg]) -> Result<TokenStream, EmitError> {
        let option = Self::canonical_assert_arg(TestingAssertHelperId::AssertIsSome, args, 0)?;
        let option_tokens = self.emit_expr(option)?;
        let failure = self.emit_assert_failure(
            Self::assert_failure_message(TestingAssertHelperId::AssertIsSome)?,
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
        let option = Self::canonical_assert_arg(TestingAssertHelperId::AssertIsNone, args, 0)?;
        if matches!(option.kind, IrExprKind::None) {
            return Ok(quote! { () });
        }
        let option_tokens = self.emit_expr(option)?;
        let failure = self.emit_assert_failure(
            Self::assert_failure_message(TestingAssertHelperId::AssertIsNone)?,
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
        let result = Self::canonical_assert_arg(TestingAssertHelperId::AssertIsOk, args, 0)?;
        if let Some(payload) = Self::result_constructor_payload(result, ConstructorId::Ok) {
            let payload_tokens = Self::emit_result_payload_tokens(payload, self.emit_expr(payload)?);
            return Ok(quote! { #payload_tokens });
        }
        let result_tokens = self.emit_expr(result)?;
        let failure = self.emit_assert_failure(
            Self::assert_failure_message(TestingAssertHelperId::AssertIsOk)?,
            args.get(1).map(|arg| &arg.expr),
        )?;
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
        let result = Self::canonical_assert_arg(TestingAssertHelperId::AssertIsErr, args, 0)?;
        if let Some(payload) = Self::result_constructor_payload(result, ConstructorId::Err) {
            let payload_tokens = Self::emit_result_payload_tokens(payload, self.emit_expr(payload)?);
            return Ok(quote! { #payload_tokens });
        }
        let result_tokens = self.emit_expr(result)?;
        let failure = self.emit_assert_failure(
            Self::assert_failure_message(TestingAssertHelperId::AssertIsErr)?,
            args.get(1).map(|arg| &arg.expr),
        )?;
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
        let call = Self::canonical_assert_arg(TestingAssertHelperId::AssertRaises, args, 0)?;
        let expected = Self::canonical_assert_arg(TestingAssertHelperId::AssertRaises, args, 1)?;
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
}
