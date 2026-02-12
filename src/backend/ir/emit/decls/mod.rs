//! Emit Rust items from IR declarations.
//!
//! This module emits top-level Rust items for IR declarations (functions, structs/enums, consts, imports, traits, and
//! impl blocks).
//!
//! ## Submodules
//!
//! - [`mutation_scan`] — parameter mutation analysis for `mut`/`&mut` emission
//! - [`functions`] — function, method, trait, and `@rust.extern` delegation emission
//! - [`impls`] — impl block emission (serde, `@derive(Validate)`, reflection)
//! - [`structures`] — struct and enum emission
//!
//! ## See also
//!
//! - [`crate::backend::ir::emit::consts`]
//! - [`crate::backend::ir::emit::types`]

mod functions;
mod impls;
mod mutation_scan;
mod structures;

use proc_macro2::{Literal, TokenStream};
use quote::{format_ident, quote};

use incan_core::lang::derives::{self, DeriveId};
use incan_core::lang::stdlib;

use super::super::decl::{IrDecl, IrDeclKind, IrImportQualifier};
use super::super::expr::IrExprKind;
use super::super::types::IrType;
use super::{EmitError, IrEmitter};

const ZEN_TEXT: &str = include_str!("../../../../../stdlib/zen.txt");

/// Join a slice of `TokenStream` path segments with `::` separators.
pub(in crate::backend::ir::emit) fn join_path_tokens(segments: &[TokenStream]) -> TokenStream {
    let mut ts = TokenStream::new();
    for (idx, seg) in segments.iter().enumerate() {
        if idx > 0 {
            ts.extend(quote! { :: });
        }
        ts.extend(seg.clone());
    }
    ts
}

impl<'a> IrEmitter<'a> {
    /// Emit a declaration as Rust tokens.
    pub(super) fn emit_decl(&self, decl: &IrDecl) -> Result<TokenStream, EmitError> {
        match &decl.kind {
            IrDeclKind::Function(func) => self.emit_function(func),
            IrDeclKind::Struct(s) => self.emit_struct(s),
            IrDeclKind::Enum(e) => self.emit_enum(e),
            IrDeclKind::TypeAlias { name, ty } => {
                let name_ident = format_ident!("{}", name);
                let ty_tokens = self.emit_type(ty);
                Ok(quote! {
                    type #name_ident = #ty_tokens;
                })
            }
            IrDeclKind::Const {
                visibility,
                name,
                ty,
                value,
            } => self.emit_const(visibility, name, ty, value),
            IrDeclKind::Import {
                qualifier,
                path,
                alias,
                items,
            } => self.emit_import(qualifier, path, alias, items),
            IrDeclKind::Impl(impl_block) => self.emit_impl(impl_block),
            IrDeclKind::Trait(trait_decl) => self.emit_trait(trait_decl),
        }
    }

    // ---- Const emission (RFC 008) ----

    fn emit_const(
        &self,
        visibility: &super::super::decl::Visibility,
        name: &str,
        ty: &IrType,
        value: &super::super::TypedExpr,
    ) -> Result<TokenStream, EmitError> {
        self.validate_const_emittable(name, ty, value)?;

        let vis = self.emit_visibility(visibility);
        let name_ident = format_ident!("{}", name);
        let ty_tokens = self.emit_type(ty);

        // If this is a FrozenList/Set/Dict with literal initializer, emit via FrozenX::new(&[...]).
        use super::super::types::IrType as T;
        use incan_core::lang::types::collections::{self, CollectionTypeId};
        let specialized_tokens: Option<TokenStream> = match (ty, &value.kind) {
            (T::NamedGeneric(n, args), IrExprKind::List(items))
                if n == collections::as_str(CollectionTypeId::FrozenList) && args.len() == 1 =>
            {
                let elems: Result<Vec<_>, EmitError> = items.iter().map(|i| self.emit_expr(i)).collect();
                let elems = elems?;
                Some(quote! { FrozenList::new(&[ #(#elems),* ]) })
            }
            (T::NamedGeneric(n, args), IrExprKind::Set(items))
                if n == collections::as_str(CollectionTypeId::FrozenSet) && args.len() == 1 =>
            {
                let elems: Result<Vec<_>, EmitError> = items.iter().map(|i| self.emit_expr(i)).collect();
                let elems = elems?;
                Some(quote! { FrozenSet::new(&[ #(#elems),* ]) })
            }
            (T::NamedGeneric(n, args), IrExprKind::Dict(pairs))
                if n == collections::as_str(CollectionTypeId::FrozenDict) && args.len() == 2 =>
            {
                let kvs: Result<Vec<_>, EmitError> = pairs
                    .iter()
                    .map(|(k, v)| {
                        let kk = self.emit_expr(k)?;
                        let vv = self.emit_expr(v)?;
                        Ok(quote! { ( #kk , #vv ) })
                    })
                    .collect();
                let kvs = kvs?;
                Some(quote! { FrozenDict::new(&[ #(#kvs),* ]) })
            }
            _ => None,
        };

        let value_tokens = if let Some(tok) = specialized_tokens {
            tok
        } else {
            match (ty, &value.kind) {
                // RFC 008: frozen scalars.
                (T::FrozenStr, IrExprKind::String(s)) => {
                    quote! { FrozenStr::new(#s) }
                }
                (T::FrozenBytes, IrExprKind::Bytes(bytes)) => {
                    let lit = Literal::byte_string(bytes);
                    quote! { FrozenBytes::new(#lit) }
                }
                _ => self.emit_expr(value)?,
            }
        };

        Ok(quote! {
            #vis const #name_ident: #ty_tokens = #value_tokens;
        })
    }

    // ---- Import emission ----

    fn emit_import(
        &self,
        qualifier: &IrImportQualifier,
        path: &[String],
        alias: &Option<String>,
        items: &[super::super::decl::IrImportItem],
    ) -> Result<TokenStream, EmitError> {
        // Skip serde imports if we're already importing them automatically
        if self.needs_serde && path.len() == 1 && path[0] == "serde" {
            let is_serde_trait = items.iter().any(|item| {
                matches!(
                    derives::from_str(item.name.as_str()),
                    Some(DeriveId::Serialize | DeriveId::Deserialize)
                )
            });
            if is_serde_trait {
                return Ok(quote! {});
            }
        }

        // Special-case stdlib shims:
        // - `std.web` maps to `incan_stdlib::web`
        // - `std.testing` maps to `incan_stdlib::testing`
        let is_stdlib_web = stdlib::is_stdlib_module(path, stdlib::STDLIB_WEB);
        let is_stdlib_testing = stdlib::is_stdlib_module(path, stdlib::STDLIB_TESTING);
        let mapped_path_tokens: Vec<_> = if is_stdlib_web {
            vec![quote! { incan_stdlib }, quote! { web }]
        } else if is_stdlib_testing {
            vec![quote! { incan_stdlib }, quote! { testing }]
        } else {
            path.iter()
                .map(|s| {
                    let ident = format_ident!("{}", Self::escape_keyword(s));
                    quote! { #ident }
                })
                .collect()
        };
        let mut path_tokens: Vec<TokenStream> = Vec::new();
        let apply_prefix = !(is_stdlib_web || is_stdlib_testing);
        if apply_prefix {
            match qualifier {
                IrImportQualifier::Auto => {
                    if self.is_internal_module_path(path) {
                        path_tokens.push(quote! { crate });
                    }
                }
                IrImportQualifier::Crate => path_tokens.push(quote! { crate }),
                IrImportQualifier::Super(levels) => {
                    for _ in 0..*levels {
                        path_tokens.push(quote! { super });
                    }
                }
                IrImportQualifier::None => {}
            }
        }
        path_tokens.extend(mapped_path_tokens);
        let path_ts = join_path_tokens(&path_tokens);

        if let Some(alias_name) = alias {
            let alias_ident = format_ident!("{}", Self::escape_keyword(alias_name));
            Ok(quote! {
                use #path_ts as #alias_ident;
            })
        } else if !items.is_empty() {
            let item_stmts: Vec<TokenStream> = items
                .iter()
                .map(|item| {
                    let name_ident = format_ident!("{}", Self::escape_keyword(&item.name));
                    let path_tokens_clone = path_tokens.clone();
                    let path_ts_clone = join_path_tokens(&path_tokens_clone);
                    if let Some(alias) = &item.alias {
                        let alias_ident = format_ident!("{}", Self::escape_keyword(alias));
                        quote! { use #path_ts_clone :: #name_ident as #alias_ident; }
                    } else {
                        quote! { use #path_ts_clone :: #name_ident; }
                    }
                })
                .collect();
            Ok(quote! { #(#item_stmts)* })
        } else if path.len() == 1 && !is_stdlib_web && !is_stdlib_testing {
            Ok(quote! {})
        } else {
            Ok(quote! {
                use #path_ts;
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ZEN_TEXT;

    #[test]
    fn zen_text_contains_one_obvious_way_once() {
        let count = ZEN_TEXT.matches("One obvious way").count();
        assert_eq!(
            count, 1,
            "Zen text should contain 'One obvious way' once, found {}",
            count
        );
    }
}
