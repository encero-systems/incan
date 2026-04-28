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

use super::super::decl::{IrDecl, IrDeclKind, IrImportOrigin, IrImportQualifier};
use super::super::expr::{IrDictEntry, IrExprKind, IrListEntry};
use super::super::ownership::{ValueUseSite, plan_value_use};
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
            IrDeclKind::TypeAlias {
                visibility,
                name,
                type_params,
                ty,
                is_rusttype: _,
                interop_edges: _,
            } => {
                let vis = self.emit_visibility(visibility);
                let name_ident = format_ident!("{}", name);
                let ty_tokens = self.emit_type(ty);
                let generics = self.emit_type_params(type_params);
                Ok(quote! {
                    #vis type #name_ident #generics = #ty_tokens;
                })
            }
            IrDeclKind::Const {
                visibility,
                name,
                ty,
                value,
            } => self.emit_const(visibility, name, ty, value),
            IrDeclKind::Static {
                visibility,
                name,
                ty,
                value,
            } => self.emit_static(visibility, name, ty, value),
            IrDeclKind::Import {
                visibility,
                origin,
                qualifier,
                path,
                alias,
                items,
            } => self.emit_import(visibility, origin, qualifier, path, alias, items),
            IrDeclKind::Impl(impl_block) => self.emit_impl(impl_block),
            IrDeclKind::Trait(trait_decl) => self.emit_trait(trait_decl),
        }
    }

    /// Emit a module-level static binding backed by a generated `StaticCell`.
    fn emit_static(
        &self,
        visibility: &super::super::decl::Visibility,
        name: &str,
        ty: &IrType,
        value: &super::super::TypedExpr,
    ) -> Result<TokenStream, EmitError> {
        let vis = self.emit_visibility(visibility);
        let name_ident = Self::rust_static_ident(name);
        let ty_tokens = self.emit_type(ty);
        let previous = self.in_static_initializer.replace(true);
        let emitted_value = self.emit_expr(value);
        self.in_static_initializer.replace(previous);
        let emitted_value = emitted_value?;
        let converted_value =
            plan_value_use(value, ValueUseSite::Assignment { target_ty: Some(ty) }).apply(emitted_value);

        Ok(quote! {
            #vis static #name_ident: std::sync::LazyLock<incan_stdlib::storage::StaticCell<#ty_tokens>> =
                std::sync::LazyLock::new(|| incan_stdlib::storage::StaticCell::new(#converted_value));
        })
    }

    // ---- Const emission (RFC 008) ----

    /// Emit one module-level constant declaration after validating the value is const-emittable.
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
        let value_tokens = self.emit_const_value_for_type(ty, value)?;

        Ok(quote! {
            #vis const #name_ident: #ty_tokens = #value_tokens;
        })
    }

    /// Emit a const initializer using the declared target type to qualify frozen collection constructors.
    fn emit_const_value_for_type(
        &self,
        ty: &IrType,
        value: &super::super::TypedExpr,
    ) -> Result<TokenStream, EmitError> {
        use super::super::types::IrType as T;
        use incan_core::lang::types::collections::{self, CollectionTypeId};

        match (ty, &value.kind) {
            (T::NamedGeneric(n, args), IrExprKind::List(items))
                if n == collections::as_str(CollectionTypeId::FrozenList) && args.len() == 1 =>
            {
                let elems: Result<Vec<_>, EmitError> = items
                    .iter()
                    .map(|entry| match entry {
                        IrListEntry::Element(value) => self.emit_const_value_for_type(&args[0], value),
                        IrListEntry::Spread(_) => Err(EmitError::Unsupported(
                            "FrozenList const spread emission is not supported".to_string(),
                        )),
                    })
                    .collect();
                let elems = elems?;
                Ok(quote! { incan_stdlib::frozen::FrozenList::new(&[ #(#elems),* ]) })
            }
            (T::NamedGeneric(n, args), IrExprKind::Set(items))
                if n == collections::as_str(CollectionTypeId::FrozenSet) && args.len() == 1 =>
            {
                let elems: Result<Vec<_>, EmitError> = items
                    .iter()
                    .map(|item| self.emit_const_value_for_type(&args[0], item))
                    .collect();
                let elems = elems?;
                Ok(quote! { incan_stdlib::frozen::FrozenSet::new(&[ #(#elems),* ]) })
            }
            (T::NamedGeneric(n, args), IrExprKind::Dict(pairs))
                if n == collections::as_str(CollectionTypeId::FrozenDict) && args.len() == 2 =>
            {
                let kvs: Result<Vec<_>, EmitError> = pairs
                    .iter()
                    .map(|entry| match entry {
                        IrDictEntry::Pair(k, v) => {
                            let kk = self.emit_const_value_for_type(&args[0], k)?;
                            let vv = self.emit_const_value_for_type(&args[1], v)?;
                            Ok(quote! { ( #kk , #vv ) })
                        }
                        IrDictEntry::Spread(_) => Err(EmitError::Unsupported(
                            "FrozenDict const spread emission is not supported".to_string(),
                        )),
                    })
                    .collect();
                let kvs = kvs?;
                Ok(quote! { incan_stdlib::frozen::FrozenDict::new(&[ #(#kvs),* ]) })
            }
            (T::Tuple(types), IrExprKind::Tuple(items)) if types.len() == items.len() => {
                let elems: Result<Vec<_>, EmitError> = types
                    .iter()
                    .zip(items.iter())
                    .map(|(ty, item)| self.emit_const_value_for_type(ty, item))
                    .collect();
                let elems = elems?;
                Ok(quote! { (#(#elems),*) })
            }
            (T::FrozenStr, IrExprKind::String(s)) => Ok(quote! { incan_stdlib::frozen::FrozenStr::new(#s) }),
            (T::FrozenBytes, IrExprKind::Bytes(bytes)) => {
                let lit = Literal::byte_string(bytes);
                Ok(quote! { incan_stdlib::frozen::FrozenBytes::new(#lit) })
            }
            _ => self.emit_expr(value),
        }
    }

    // ---- Import emission ----

    /// Emit a Rust import or re-export after generated-use analysis prunes private unused bindings.
    fn emit_import(
        &self,
        visibility: &super::super::decl::Visibility,
        origin: &IrImportOrigin,
        qualifier: &IrImportQualifier,
        path: &[String],
        alias: &Option<String>,
        items: &[super::super::decl::IrImportItem],
    ) -> Result<TokenStream, EmitError> {
        // Skip redundant external serde imports when derive emission already uses fully qualified `serde::...` paths.
        //
        // Do not skip `std.serde.json` imports here: explicit trait adoption (`with Serialize`) needs the Incan stdlib
        // trait in scope as a distinct item from Rust's `serde::Serialize`.
        if *self.needs_serde.borrow() {
            let is_serde_trait = items.iter().any(|item| {
                matches!(
                    derives::from_str(item.name.as_str()),
                    Some(DeriveId::Serialize | DeriveId::Deserialize)
                )
            });
            let is_serde_import_path = path.len() == 1 && path[0] == "serde";
            if is_serde_trait && is_serde_import_path {
                return Ok(quote! {});
            }
        }

        // Typechecker-only namespaces (e.g. `std.rust`) have no corresponding Rust module.
        // Capability bounds are folded into generic type parameter bounds by the lowering layer.
        if stdlib::is_typechecker_only_stdlib(path) {
            return Ok(quote! {});
        }

        // RFC 023: map all Incan `std.*` imports to emitted `crate::__incan_std::*` modules.
        //
        // `std.testing.assert_eq` (Incan-source mode) → `crate::__incan_std::testing::assert_eq`
        // `std.async.time.sleep` → `crate::__incan_std::r#async::time::sleep`
        // `std.web` → `crate::__incan_std::web`
        //
        // Only Incan stdlib imports (qualifier `Auto`) are mapped. Rust crate imports like
        // `from rust::std::collections import HashMap` (qualifier `None`) are left as-is.
        let is_pub_library_import = matches!(origin, IrImportOrigin::PubLibrary { .. });
        let is_stdlib =
            !is_pub_library_import && !matches!(qualifier, IrImportQualifier::None) && stdlib::is_any_stdlib_path(path);
        let is_incan_source_stdlib = is_stdlib;

        let path_tokens: Vec<TokenStream> = if is_incan_source_stdlib {
            let mut tokens = vec![quote! { crate }];
            let std_namespace = Self::rust_ident(stdlib::INCAN_STD_NAMESPACE);
            tokens.push(quote! { #std_namespace });
            for seg in path.iter().skip(1) {
                let ident = Self::rust_ident(seg);
                tokens.push(quote! { #ident });
            }
            tokens
        } else if is_pub_library_import {
            path.iter()
                .map(|segment| {
                    let ident = Self::rust_ident(segment);
                    quote! { #ident }
                })
                .collect()
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
        // Public source imports, stdlib facades, and rust.module imports are re-exported. Private `pub::` library
        // imports behave like ordinary private imports: emit them only when generated Rust references the binding.
        let is_rust_crate_reexport =
            matches!(qualifier, IrImportQualifier::None) && self.rust_module_path.is_some() && !is_pub_library_import;
        let is_public_reexport = !matches!(visibility, super::super::decl::Visibility::Private);
        let export_module_import = is_public_reexport || is_incan_source_stdlib || is_rust_crate_reexport;

        if let Some(alias_name) = alias {
            let alias_ident = Self::rust_ident(alias_name);
            if export_module_import {
                Ok(quote! {
                    pub use #path_ts as #alias_ident;
                })
            } else if self.should_emit_import_binding(alias_name) {
                Ok(quote! {
                    use #path_ts as #alias_ident;
                })
            } else {
                Ok(quote! {})
            }
        } else if !items.is_empty() {
            // ---- Track Rust import paths for alias resolution ----
            // When emitting Rust imports (qualifier=None), record the mapping from alias/name → full module path.
            // This enables newtype trait delegation to resolve "AxumResponse" back to "axum::response::Response" for
            // pattern matching.
            if matches!(qualifier, IrImportQualifier::None) && !is_pub_library_import {
                for item in items {
                    let key = item.alias.as_ref().unwrap_or(&item.name).clone();
                    let mut full_path = path.to_vec();
                    full_path.push(item.name.clone());
                    self.rust_import_paths.borrow_mut().insert(key, full_path);
                }
            }

            let preserves_stdlib_rust_facade = matches!(qualifier, IrImportQualifier::None)
                && path.first().is_some_and(|segment| segment == "incan_stdlib");
            let export_item_import = export_module_import || preserves_stdlib_rust_facade;
            let item_stmts: Vec<TokenStream> = items
                .iter()
                .filter(|item| {
                    let binding = item.alias.as_ref().unwrap_or(&item.name);
                    export_item_import
                        || self.should_emit_import_binding(binding)
                        || self.should_emit_extension_trait_import(binding)
                })
                .map(|item| {
                    let name_ident = Self::rust_ident(&item.name);
                    let path_tokens_clone = path_tokens.clone();
                    let path_ts_clone = join_path_tokens(&path_tokens_clone);
                    if let Some(alias) = &item.alias {
                        let alias_ident = Self::rust_ident(alias);
                        if export_item_import {
                            quote! { pub use #path_ts_clone :: #name_ident as #alias_ident; }
                        } else {
                            quote! { use #path_ts_clone :: #name_ident as #alias_ident; }
                        }
                    } else {
                        if export_item_import {
                            quote! { pub use #path_ts_clone :: #name_ident; }
                        } else {
                            quote! { use #path_ts_clone :: #name_ident; }
                        }
                    }
                })
                .collect();
            Ok(quote! { #(#item_stmts)* })
        } else if path.len() == 1 && !is_stdlib {
            Ok(quote! {})
        } else if export_module_import {
            Ok(quote! {
                pub use #path_ts;
            })
        } else if let Some(binding) = path.last()
            && self.should_emit_import_binding(binding)
        {
            Ok(quote! {
                use #path_ts;
            })
        } else {
            Ok(quote! {})
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
