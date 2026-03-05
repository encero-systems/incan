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

const ZEN_TEXT: &str = include_str!("../../../../../crates/incan_stdlib/stdlib/zen.txt");

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
        // Skip serde imports if we're already importing them automatically.
        // Covers both `from serde import ...` and `from std.serde.json import Serialize, Deserialize`.
        if *self.needs_serde.borrow() {
            let is_serde_trait = items.iter().any(|item| {
                matches!(
                    derives::from_str(item.name.as_str()),
                    Some(DeriveId::Serialize | DeriveId::Deserialize)
                )
            });
            let is_serde_import_path = (path.len() == 1 && path[0] == "serde")
                || (path.len() >= 2 && path[0] == stdlib::STDLIB_ROOT && path[1] == "serde");
            if is_serde_trait && is_serde_import_path {
                return Ok(quote! {});
            }
        }

        // RFC 023: map Incan `std.*` imports based on per-namespace implementation mode.
        //
        // `std.testing.assert_eq` (Incan-source mode) → `crate::__incan_std::testing::assert_eq`
        // `std.async.time.sleep` → `incan_stdlib::r#async::time::sleep`
        // `std.web` → `incan_stdlib::web`
        //
        // Incan-source stdlib namespaces are emitted under `crate::__incan_std::*`, while runtime-facade namespaces are
        // emitted under `incan_stdlib::*`.
        //
        // Only Incan stdlib imports (qualifier `Auto`) are mapped. Rust crate imports like
        // `from rust::std::collections import HashMap` (qualifier `None`) are left as-is.
        let is_stdlib = !matches!(qualifier, IrImportQualifier::None) && stdlib::is_any_stdlib_path(path);
        let is_incan_source_stdlib = is_stdlib
            && matches!(
                stdlib::stdlib_impl_mode_for(path),
                Some(stdlib::StdlibImplMode::IncanSource)
            );

        let path_tokens: Vec<TokenStream> = if is_incan_source_stdlib {
            let mut tokens = vec![quote! { crate }];
            let std_namespace = Self::rust_ident(stdlib::INCAN_STD_NAMESPACE);
            tokens.push(quote! { #std_namespace });
            for seg in path.iter().skip(1) {
                let ident = Self::rust_ident(seg);
                tokens.push(quote! { #ident });
            }
            tokens
        } else {
            let mut tokens: Vec<TokenStream> = Vec::new();
            let mapped_path_tokens: Vec<_> = if is_stdlib {
                let mut mapped = vec![quote! { incan_stdlib }];
                // Skip the `std` root, map the rest with keyword escaping.
                for seg in path.iter().skip(1) {
                    let ident = Self::rust_ident(seg);
                    mapped.push(quote! { #ident });
                }
                mapped
            } else {
                path.iter()
                    .map(|s| {
                        let ident = Self::rust_ident(s);
                        quote! { #ident }
                    })
                    .collect()
            };
            let apply_prefix = !is_stdlib;
            if apply_prefix {
                match qualifier {
                    IrImportQualifier::Auto => {
                        if self.is_internal_module_path(path) {
                            tokens.push(quote! { crate });
                        }
                    }
                    IrImportQualifier::Crate => tokens.push(quote! { crate }),
                    IrImportQualifier::Super(levels) => {
                        for _ in 0..*levels {
                            tokens.push(quote! { super });
                        }
                    }
                    IrImportQualifier::None => {}
                }
            }
            tokens.extend(mapped_path_tokens);
            tokens
        };

        let path_ts = join_path_tokens(&path_tokens);
        let export_import = is_incan_source_stdlib;

        if let Some(alias_name) = alias {
            let alias_ident = Self::rust_ident(alias_name);
            if export_import {
                Ok(quote! {
                    pub use #path_ts as #alias_ident;
                })
            } else {
                Ok(quote! {
                    use #path_ts as #alias_ident;
                })
            }
        } else if !items.is_empty() {
            // ---- Track Rust import paths for alias resolution ----
            // When emitting Rust imports (qualifier=None), record the mapping from alias/name → full module path.
            // This enables newtype trait delegation to resolve "AxumResponse" back to "axum::response::Response" for
            // pattern matching.
            if matches!(qualifier, IrImportQualifier::None) {
                for item in items {
                    let key = item.alias.as_ref().unwrap_or(&item.name).clone();
                    let mut full_path = path.to_vec();
                    full_path.push(item.name.clone());
                    self.rust_import_paths.borrow_mut().insert(key, full_path);
                }
            }

            let item_stmts: Vec<TokenStream> = items
                .iter()
                .map(|item| {
                    let name_ident = Self::rust_ident(&item.name);
                    let path_tokens_clone = path_tokens.clone();
                    let path_ts_clone = join_path_tokens(&path_tokens_clone);
                    if let Some(alias) = &item.alias {
                        let alias_ident = Self::rust_ident(alias);
                        if export_import {
                            quote! { pub use #path_ts_clone :: #name_ident as #alias_ident; }
                        } else {
                            quote! { use #path_ts_clone :: #name_ident as #alias_ident; }
                        }
                    } else if export_import {
                        quote! { pub use #path_ts_clone :: #name_ident; }
                    } else {
                        quote! { use #path_ts_clone :: #name_ident; }
                    }
                })
                .collect();
            Ok(quote! { #(#item_stmts)* })
        } else if path.len() == 1 && !is_stdlib {
            Ok(quote! {})
        } else if export_import {
            Ok(quote! {
                pub use #path_ts;
            })
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
