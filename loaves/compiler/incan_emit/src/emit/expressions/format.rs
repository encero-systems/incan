//! Emit Rust code for format strings and range expressions.
//!
//! This module handles:
//! - Format string expressions (f-strings): `f"Hello {name}"`
//! - Range expressions: `start..end`, `start..=end`, `..end`, `start..`

use proc_macro2::{Literal as TokenLiteral, TokenStream};
use quote::quote;

use super::super::{EmitError, IrEmitter};
use crate::conversions::exact_float_value_validation;
use incan_ir::expr::{FormatPart, TypedExpr};
use incan_ir::types::IrType;

/// Whether a value of `ty` renders through the runtime's float spelling in a display position.
///
/// Rust's `Display for f64` prints `100.0` as `100`, so every display position that spelled `format!("{}", x)` for a
/// `float` wrote an integer into the program's text output. The rendering rule belongs to the runtime:
/// `incan_std_core::strings::float_to_string` (backed by `incan_lang::numeric_strings::float_to_string`, which the
/// replacement executor shares) spells a float the way Python does, and the emitter only routes the three display
/// positions -- f-string interpolation, `str(x)`, and `print`/`println` -- to it through [`float_display_text`].
/// Exact `f32`/`f64` carriers are deliberately not routed: they keep their native Rust spelling, which the
/// replacement profile pins as their checked-carrier behaviour.
///
/// - Compatibility issue: #1372.
/// - Behavior evidence: `codegen_snapshot_tests` (`float_display`), `incan_std_core::strings` unit tests, and the
///   `cli_float_display_tests` end-to-end root.
/// - Semantic owner: runtime-service facts (`incan_std_core::strings::float_to_string`) over an `IncanType` `float`;
///   the emitter consumes the helper and decides nothing about the spelling.
/// - Retirement condition: the Rust-emission backend's removal (#654); the replacement executor already renders through
///   the same `incan_lang` function.
pub(in super::super) fn renders_as_python_float(ty: &IrType) -> bool {
    matches!(ty, IrType::Float)
}

/// The runtime call that renders a `float` value as display text; see [`renders_as_python_float`].
pub(in super::super) fn float_display_text(value: TokenStream) -> TokenStream {
    quote! { incan_std_core::strings::float_to_string(#value) }
}

impl<'a> IrEmitter<'a> {
    /// Emit a format string expression.
    ///
    /// Converts an f-string into a call to `incan_std_core::strings::fstring(...)`.
    ///
    /// ## Parameters
    ///
    /// - `parts`: lowered format parts (literal segments + expression segments).
    ///
    /// ## Returns
    ///
    /// - A Rust `TokenStream` that evaluates to an owned `String`.
    ///
    /// ## Notes
    ///
    /// - Literal segments are passed through verbatim: the lexer already collapsed `{{` and `}}` to one brace, and
    ///   `incan_std_core::strings::fstring` concatenates rather than interpreting a format string, so escaping them
    ///   again would print both characters.
    /// - Display expression segments are formatted via `format!("{}", expr)`, except a `float`, which renders through
    ///   [`float_display_text`] (see [`renders_as_python_float`]).
    /// - Debug expression segments are formatted via `format!("{:?}", expr)`.
    pub(in super::super) fn emit_format_expr(&self, parts: &[FormatPart]) -> Result<TokenStream, EmitError> {
        // Build literal parts (length = args + 1) and a parallel list of formatted args.
        let mut literal_parts: Vec<String> = Vec::new();
        let mut current = String::new();
        let mut args: Vec<TokenStream> = Vec::new();

        for part in parts {
            match part {
                FormatPart::Literal(s) => {
                    current.push_str(s);
                }
                FormatPart::Expr { expr, style } => {
                    literal_parts.push(current.clone());
                    current.clear();
                    let arg_expr = exact_float_value_validation(&expr.ty).apply(self.emit_expr(expr)?);
                    if style.emits_rust_debug(&expr.ty) {
                        args.push(quote! { format!("{:?}", #arg_expr) });
                    } else if renders_as_python_float(&expr.ty) {
                        args.push(float_display_text(arg_expr));
                    } else {
                        args.push(quote! { format!("{}", #arg_expr) });
                    }
                }
            }
        }
        literal_parts.push(current);

        let parts_tokens: Vec<TokenStream> = literal_parts
            .iter()
            .map(|s| {
                let lit = TokenLiteral::string(s);
                quote! { #lit }
            })
            .collect();
        let parts_len = parts_tokens.len();

        Ok(quote! {{
            let __parts: [&str; #parts_len ] = [#(#parts_tokens),*];
            let __args: Vec<String> = vec![#(#args),*];
            incan_std_core::strings::fstring(&__parts, &__args)
        }})
    }

    /// Emit a range expression.
    ///
    /// Converts Incan range syntax to Rust range expressions:
    /// - `start..end` (exclusive)
    /// - `start..=end` (inclusive)
    /// - `start..` (open-ended)
    /// - `..end` (from zero)
    /// - `..=end` (from zero, inclusive)
    pub(in super::super) fn emit_range_expr(
        &self,
        start: Option<&TypedExpr>,
        end: Option<&TypedExpr>,
        inclusive: bool,
    ) -> Result<TokenStream, EmitError> {
        match (start, end, inclusive) {
            (Some(s), Some(e), false) => {
                let ss = self.emit_expr(s)?;
                let ee = self.emit_expr(e)?;
                Ok(quote! { #ss..#ee })
            }
            (Some(s), Some(e), true) => {
                let ss = self.emit_expr(s)?;
                let ee = self.emit_expr(e)?;
                Ok(quote! { #ss..=#ee })
            }
            (Some(s), None, _) => {
                let ss = self.emit_expr(s)?;
                Ok(quote! { #ss.. })
            }
            (None, Some(e), false) => {
                let ee = self.emit_expr(e)?;
                Ok(quote! { ..#ee })
            }
            (None, Some(e), true) => {
                let ee = self.emit_expr(e)?;
                Ok(quote! { ..=#ee })
            }
            (None, None, _) => Ok(quote! { .. }),
        }
    }
}
