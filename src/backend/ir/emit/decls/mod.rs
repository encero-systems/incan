//! Emit Rust items from IR declarations.
//!
//! This module emits top-level Rust items for IR declarations (functions, structs/enums, consts, imports, traits, and
//! impl blocks).
//!
//! ## Submodules
//!
//! - [`mutation_scan`] — parameter mutation analysis for `mut`/`&mut` emission
//! - [`functions`] — function, method, trait, and `@rust.extern` delegation emission
//! - [`impls`] — impl block emission (`@derive(Validate)`, trait impls, reflection)
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
use quote::quote;

use crate::frontend::symbols::is_overload_emitted_name;
use incan_core::lang::{stdlib, surface::types as surface_types};

use super::super::decl::{IrDecl, IrDeclKind, IrImportOrigin, IrImportQualifier, Visibility};
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
    /// Emit source docstrings as Rust doc attributes on generated public/backend items.
    fn emit_rustdoc_attrs(&self, docstring: Option<&str>) -> Vec<TokenStream> {
        normalized_rustdoc_lines(docstring.unwrap_or_default())
            .into_iter()
            .map(|line| {
                let doc_line = if line.is_empty() { line } else { format!(" {line}") };
                let literal = Literal::string(&doc_line);
                quote! { #[doc = #literal] }
            })
            .collect()
    }

    /// Emit source docstrings only for public generated Rust items.
    fn emit_public_rustdoc_attrs(&self, visibility: &Visibility, docstring: Option<&str>) -> Vec<TokenStream> {
        if matches!(visibility, Visibility::Public) {
            self.emit_rustdoc_attrs(docstring)
        } else {
            Vec::new()
        }
    }

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
                let name_ident = Self::rust_ident(name);
                let ty_tokens = self.emit_type(ty);
                let generics = self.emit_type_params(type_params);
                Ok(quote! {
                    #vis type #name_ident #generics = #ty_tokens;
                })
            }
            IrDeclKind::SymbolAlias {
                visibility,
                name,
                target_path,
                target_origin,
                target_qualifier,
            } => {
                let vis = self.emit_visibility(visibility);
                let name_ident = Self::rust_ident(name);
                let target =
                    self.emit_symbol_alias_target_path(target_origin.as_ref(), target_qualifier.as_ref(), target_path);
                Ok(quote! {
                    #vis use #target as #name_ident;
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
        let name_ident = Self::rust_ident(name);
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
            (T::Struct(_), IrExprKind::Struct { name, fields }) => self.emit_const_struct_value(name, fields),
            (T::StaticStr, IrExprKind::String(s)) => Ok(quote! { #s }),
            (T::StaticBytes, IrExprKind::Bytes(bytes)) => {
                let lit = Literal::byte_string(bytes);
                Ok(quote! { #lit })
            }
            (T::FrozenStr, IrExprKind::String(s)) => Ok(quote! { incan_stdlib::frozen::FrozenStr::new(#s) }),
            (T::FrozenBytes, IrExprKind::Bytes(bytes)) => {
                let lit = Literal::byte_string(bytes);
                Ok(quote! { incan_stdlib::frozen::FrozenBytes::new(#lit) })
            }
            _ => self.emit_expr(value),
        }
    }

    /// Emit a struct/model literal in a Rust const initializer without applying runtime ownership conversions.
    fn emit_const_struct_value(
        &self,
        name: &str,
        fields: &[(String, super::super::TypedExpr)],
    ) -> Result<TokenStream, EmitError> {
        let n = Self::rust_ident(name);
        let Some(metadata) = self.struct_constructor_metadata_for_fields(name, fields) else {
            let field_tokens: Result<Vec<_>, EmitError> = fields
                .iter()
                .map(|(field_name, field_value)| {
                    let field_ident = Self::rust_ident(field_name);
                    let value = self.emit_const_value_for_type(&field_value.ty, field_value)?;
                    Ok(quote! { #field_ident: #value })
                })
                .collect();
            let field_tokens = field_tokens?;
            return Ok(quote! { #n { #(#field_tokens),* } });
        };

        let mut provided: std::collections::HashMap<&str, &super::super::TypedExpr> = std::collections::HashMap::new();
        for (field_name, field_value) in fields {
            if let Some(canonical) = metadata.canonical_field_name(field_name) {
                provided.insert(canonical, field_value);
            }
        }

        let mut out_fields = Vec::new();
        for field_name in &metadata.fields {
            let field_ident = Self::rust_ident(field_name);
            let Some(target_ty) = metadata.field_types.get(field_name) else {
                return Err(EmitError::Unsupported(format!(
                    "missing field type metadata for const field '{}.{}'",
                    name, field_name
                )));
            };
            let Some(field_value) = provided.get(field_name.as_str()) else {
                return Err(EmitError::Unsupported(format!(
                    "const model constructor '{}' must provide field '{}' explicitly",
                    name, field_name
                )));
            };
            let value = self.emit_const_value_for_type(target_ty, field_value)?;
            out_fields.push(quote! { #field_ident: #value });
        }

        Ok(quote! { #n { #(#out_fields),* } })
    }

    // ---- Import emission ----

    /// Return whether an import path refers to the source-authored Incan stdlib namespace.
    pub(super) fn is_incan_source_stdlib_import(
        origin: &IrImportOrigin,
        qualifier: &IrImportQualifier,
        path: &[String],
    ) -> bool {
        !matches!(origin, IrImportOrigin::PubLibrary { .. })
            && !matches!(qualifier, IrImportQualifier::None)
            && stdlib::is_any_stdlib_path(path)
    }

    /// Convert an IR import path into Rust path segments using the same qualification rules for imports and aliases.
    fn import_path_tokens(
        &self,
        origin: &IrImportOrigin,
        qualifier: &IrImportQualifier,
        path: &[String],
    ) -> Vec<TokenStream> {
        let is_pub_library_import = matches!(origin, IrImportOrigin::PubLibrary { .. });
        let is_stdlib = Self::is_incan_source_stdlib_import(origin, qualifier, path);

        if is_stdlib {
            let mut tokens = vec![quote! { crate }];
            let std_namespace = Self::rust_ident(stdlib::INCAN_STD_NAMESPACE);
            tokens.push(quote! { #std_namespace });
            for seg in path.iter().skip(1) {
                let ident = Self::rust_ident(seg);
                tokens.push(quote! { #ident });
            }
            return tokens;
        }

        if is_pub_library_import {
            return path
                .iter()
                .map(|segment| {
                    let ident = Self::rust_ident(segment);
                    quote! { #ident }
                })
                .collect();
        }

        let mut tokens: Vec<TokenStream> = Vec::new();
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
        tokens.extend(path.iter().map(|segment| {
            let ident = Self::rust_ident(segment);
            quote! { #ident }
        }));
        tokens
    }

    /// Return an absolute runtime path for a stdlib facade re-export of a runtime-owned surface type.
    ///
    /// Some stdlib source modules expose runtime-owned types such as `JoinHandle[T]` through ordinary private
    /// `rust::...` imports because they need those names in signatures. A higher-level stdlib facade can then re-export
    /// that type from the source module. Rust cannot re-export through the generated module's private `use`, so the
    /// facade must point at the real runtime definition path instead.
    fn stdlib_runtime_surface_type_reexport_path(&self, path: &[String], item_name: &str) -> Option<TokenStream> {
        let id = surface_types::from_str(item_name)?;
        if surface_types::owner(id) != surface_types::SurfaceTypeOwner::Runtime {
            return None;
        }
        let module_path = surface_types::stdlib_module_path(id)?;
        let module_segments = module_path.split('.').collect::<Vec<_>>();
        if path.iter().map(String::as_str).ne(module_segments.iter().copied()) {
            return None;
        }

        let mut tokens = vec![quote! { incan_stdlib }];
        for segment in module_segments.into_iter().skip(1) {
            let ident = Self::rust_ident(segment);
            tokens.push(quote! { #ident });
        }
        let item_ident = Self::rust_ident(surface_types::as_str(id));
        tokens.push(quote! { #item_ident });
        Some(join_path_tokens(&tokens))
    }

    /// Emit the Rust path used by a module-level symbol alias target.
    ///
    /// Imported targets use their original import path so public aliases re-export public items directly instead of
    /// re-exporting a private local `use` binding.
    fn emit_symbol_alias_target_path(
        &self,
        target_origin: Option<&IrImportOrigin>,
        target_qualifier: Option<&IrImportQualifier>,
        target_path: &[String],
    ) -> TokenStream {
        let Some(origin) = target_origin else {
            let target_segments = target_path
                .iter()
                .map(|segment| {
                    let ident = Self::rust_ident(segment);
                    quote! { #ident }
                })
                .collect::<Vec<_>>();
            return join_path_tokens(&target_segments);
        };
        let Some(qualifier) = target_qualifier else {
            let target_segments = target_path
                .iter()
                .map(|segment| {
                    let ident = Self::rust_ident(segment);
                    quote! { #ident }
                })
                .collect::<Vec<_>>();
            return join_path_tokens(&target_segments);
        };

        let path_tokens = self.import_path_tokens(origin, qualifier, target_path);
        let path = join_path_tokens(&path_tokens);
        if matches!(qualifier, IrImportQualifier::None) && !matches!(origin, IrImportOrigin::PubLibrary { .. }) {
            quote! { :: #path }
        } else {
            path
        }
    }

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
        let is_incan_source_stdlib = Self::is_incan_source_stdlib_import(origin, qualifier, path);

        let path_tokens = self.import_path_tokens(origin, qualifier, path);

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

            let export_item_import = export_module_import;
            // If rust-inspect metadata is unavailable for an extension trait, codegen cannot map the later method call
            // back to its trait import. Preserve only the common Rust pattern `from crate import Trait, function` when
            // reachable code uses a lowercase function from the same import group. This keeps `rand::Rng` for
            // `thread_rng().gen_range(...)` without retaining unrelated dead type imports such as `Path` in pruned
            // helper-only code.
            let preserve_metadata_missing_trait_candidate =
                if matches!(qualifier, IrImportQualifier::None) && !is_pub_library_import {
                    let analysis = self.generated_use_analysis.borrow();
                    items.iter().any(|item| {
                        let binding = item.alias.as_ref().unwrap_or(&item.name);
                        item.name.chars().next().is_some_and(|ch| ch.is_ascii_lowercase())
                            && analysis.used_imports.contains(binding)
                    })
                } else {
                    false
                };
            let should_reexport_item = |item: &super::super::decl::IrImportItem| {
                let binding = item.alias.as_ref().unwrap_or(&item.name);
                if is_incan_source_stdlib && binding.starts_with('_') {
                    return false;
                }
                export_item_import || item.force_reexport
            };
            let item_stmts: Vec<TokenStream> = items
                .iter()
                .filter(|item| {
                    let binding = item.alias.as_ref().unwrap_or(&item.name);
                    let private_type_like_binding = binding
                        .trim_start_matches('_')
                        .chars()
                        .next()
                        .is_some_and(|ch| ch.is_ascii_uppercase());
                    if is_incan_source_stdlib && binding.starts_with('_') && !private_type_like_binding {
                        return self.should_emit_extension_trait_import(binding);
                    }
                    should_reexport_item(item)
                        || self.should_emit_import_binding(binding)
                        || self.should_emit_extension_trait_import(binding)
                        || (preserve_metadata_missing_trait_candidate
                            && item.rust_trait_import.is_none()
                            && item.name.chars().next().is_some_and(|ch| ch.is_ascii_uppercase()))
                })
                .map(|item| {
                    let binding = item.alias.as_ref().unwrap_or(&item.name);
                    let name_ident = if item.is_static {
                        Self::rust_static_ident(&item.name)
                    } else {
                        Self::rust_ident(&item.name)
                    };
                    let runtime_surface_reexport_path = if should_reexport_item(item) && is_incan_source_stdlib {
                        self.stdlib_runtime_surface_type_reexport_path(path, &item.name)
                    } else {
                        None
                    };
                    let path_tokens_clone = path_tokens.clone();
                    let path_ts_clone = join_path_tokens(&path_tokens_clone);
                    let absolute_path = matches!(qualifier, IrImportQualifier::None) && !is_pub_library_import;
                    let static_init_import = if item.is_static && self.static_needs_imported_init_import(binding) {
                        let init_ident = Self::rust_ident("__incan_init_module_statics");
                        let init_alias = Self::imported_static_init_ident(binding);
                        if absolute_path {
                            quote! { use :: #path_ts_clone :: #init_ident as #init_alias; }
                        } else {
                            quote! { use #path_ts_clone :: #init_ident as #init_alias; }
                        }
                    } else {
                        quote! {}
                    };
                    if item.alias.is_none()
                        && is_overload_emitted_name(&item.name)
                        && !self
                            .emitted_overload_import_bindings
                            .borrow_mut()
                            .insert(item.name.clone())
                    {
                        return quote! {};
                    }

                    let item_import = if let Some(alias) = &item.alias {
                        let alias_ident = if item.is_static {
                            Self::rust_static_ident(alias)
                        } else {
                            Self::rust_ident(alias)
                        };
                        if let Some(runtime_path) = &runtime_surface_reexport_path {
                            quote! { pub use :: #runtime_path as #alias_ident; }
                        } else if should_reexport_item(item) {
                            if absolute_path {
                                quote! { pub use :: #path_ts_clone :: #name_ident as #alias_ident; }
                            } else {
                                quote! { pub use #path_ts_clone :: #name_ident as #alias_ident; }
                            }
                        } else {
                            if absolute_path {
                                quote! { use :: #path_ts_clone :: #name_ident as #alias_ident; }
                            } else {
                                quote! { use #path_ts_clone :: #name_ident as #alias_ident; }
                            }
                        }
                    } else {
                        if let Some(runtime_path) = &runtime_surface_reexport_path {
                            quote! { pub use :: #runtime_path; }
                        } else if should_reexport_item(item) {
                            if absolute_path {
                                quote! { pub use :: #path_ts_clone :: #name_ident; }
                            } else {
                                quote! { pub use #path_ts_clone :: #name_ident; }
                            }
                        } else {
                            if absolute_path {
                                quote! { use :: #path_ts_clone :: #name_ident; }
                            } else {
                                quote! { use #path_ts_clone :: #name_ident; }
                            }
                        }
                    };
                    quote! { #static_init_import #item_import }
                })
                .collect();
            Ok(quote! { #(#item_stmts)* })
        } else if path.len() == 1 && !is_incan_source_stdlib {
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

/// Normalize docstring body lines for Rust doc attributes without wrapping or editing prose.
fn normalized_rustdoc_lines(docstring: &str) -> Vec<String> {
    let lines: Vec<&str> = docstring.lines().collect();
    let Some(start) = lines.iter().position(|line| !line.trim().is_empty()) else {
        return Vec::new();
    };
    let end = lines
        .iter()
        .rposition(|line| !line.trim().is_empty())
        .map(|idx| idx + 1)
        .unwrap_or(start);
    let body = &lines[start..end];
    let dedent_start = if body.len() > 1 && leading_indent(body[0]) == 0 {
        1
    } else {
        0
    };
    let common_indent = body[dedent_start..]
        .iter()
        .filter_map(|line| {
            if line.trim().is_empty() {
                None
            } else {
                Some(leading_indent(line))
            }
        })
        .min()
        .unwrap_or(0);

    let mut normalized = Vec::with_capacity(body.len());
    let mut in_fenced_block = false;
    for (idx, line) in body.iter().enumerate() {
        let line = if idx < dedent_start {
            line.trim_start().to_string()
        } else if line.len() >= common_indent {
            line[common_indent..].to_string()
        } else {
            line.trim_start().to_string()
        };
        normalized.push(rustdoc_safe_doc_line(&line, &mut in_fenced_block));
    }
    normalized
}

/// Count leading whitespace bytes for docstring indentation normalization.
fn leading_indent(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// Keep Incan-authored examples readable in generated Rustdoc without letting Rustdoc compile them as Rust doctests.
fn rustdoc_safe_doc_line(line: &str, in_fenced_block: &mut bool) -> String {
    let trimmed = line.trim_start();
    if !trimmed.starts_with("```") {
        return line.to_string();
    }

    if *in_fenced_block {
        *in_fenced_block = false;
        return line.to_string();
    }

    *in_fenced_block = true;
    let leading_len = line.len() - trimmed.len();
    format!("{}{}", &line[..leading_len], "```ignore")
}

#[cfg(test)]
mod tests {
    use super::{ZEN_TEXT, normalized_rustdoc_lines};

    #[test]
    fn zen_text_contains_one_obvious_way_once() {
        let count = ZEN_TEXT.matches("One obvious way").count();
        assert_eq!(
            count, 1,
            "Zen text should contain 'One obvious way' once, found {}",
            count
        );
    }

    #[test]
    fn rustdoc_lines_dedent_body_after_unindented_summary() {
        let lines = normalized_rustdoc_lines(
            r#"Explicitly fail a test with a message.

    Fail makes the program raise a panic.

    Example:

    ```incan
    fail("this test should not reach here")
    ```"#,
        );

        assert_eq!(
            lines,
            vec![
                "Explicitly fail a test with a message.",
                "",
                "Fail makes the program raise a panic.",
                "",
                "Example:",
                "",
                "```ignore",
                "fail(\"this test should not reach here\")",
                "```",
            ]
        );
    }

    #[test]
    fn rustdoc_lines_render_source_fences_as_ignored_blocks() {
        let lines = normalized_rustdoc_lines(
            r#"
                Summary.

                ```incan
                print("hello")
                ```

                ```
                also source, not Rust
                ```
            "#,
        );

        assert_eq!(
            lines,
            vec![
                "Summary.",
                "",
                "```ignore",
                "print(\"hello\")",
                "```",
                "",
                "```ignore",
                "also source, not Rust",
                "```",
            ]
        );
    }
}
