//! Emit Rust code for format strings and range expressions.
//!
//! This module handles:
//! - Format string expressions (f-strings): `f"Hello {name}"`
//! - Range expressions: `start..end`, `start..=end`, `..end`, `start..`

use incan_core::strings::escape_format_literal;
use proc_macro2::{Literal as TokenLiteral, TokenStream};
use quote::quote;

use super::super::super::expr::{FormatPart, TypedExpr};
use super::super::{EmitError, IrEmitter};

impl<'a> IrEmitter<'a> {
    /// Emit a format string expression.
    ///
    /// Converts an f-string into a call to `incan_stdlib::strings::fstring(...)`.
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
    /// - Literal segments are brace-escaped via `incan_core::strings::escape_format_literal`.
    /// - Display expression segments are formatted via `format!("{}", expr)`.
    /// - Debug expression segments are formatted via `format!("{:?}", expr)`.
    pub(in super::super) fn emit_format_expr(&self, parts: &[FormatPart]) -> Result<TokenStream, EmitError> {
        // Build literal parts (length = args + 1) and a parallel list of formatted args.
        let mut literal_parts: Vec<String> = Vec::new();
        let mut current = String::new();
        let mut args: Vec<TokenStream> = Vec::new();

        for part in parts {
            match part {
                FormatPart::Literal(s) => {
                    current.push_str(&escape_format_literal(s));
                }
                FormatPart::Expr { expr, style } => {
                    literal_parts.push(current.clone());
                    current.clear();
                    let arg_expr = self.emit_expr(expr)?;
                    if style.emits_rust_debug(&expr.ty) {
                        args.push(quote! { format!("{:?}", #arg_expr) });
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
            incan_stdlib::strings::fstring(&__parts, &__args)
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
