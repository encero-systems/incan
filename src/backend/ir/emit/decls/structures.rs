//! Struct and enum emission.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use incan_core::lang::derives::{self, DeriveId};
use incan_core::lang::trait_bridges;

use super::super::{EmitError, IrEmitter};

impl<'a> IrEmitter<'a> {
    pub(in crate::backend::ir::emit) fn emit_struct(
        &self,
        s: &super::super::super::decl::IrStruct,
    ) -> Result<TokenStream, EmitError> {
        let name = Self::rust_ident(&s.name);
        let vis = self.emit_visibility(&s.visibility);

        let derives: Vec<TokenStream> = s
            .derives
            .iter()
            // `Validate` is an Incan semantic derive (not a Rust derive macro).
            .filter(|d| derives::from_str(d.as_str()) != Some(DeriveId::Validate))
            .map(|d| match derives::from_str(d.as_str()) {
                Some(DeriveId::Serialize) => quote! { serde::Serialize },
                Some(DeriveId::Deserialize) => quote! { serde::Deserialize },
                _ => {
                    let d_ident = format_ident!("{}", d);
                    quote! { #d_ident }
                }
            })
            .collect();

        let derive_attr = if derives.is_empty() {
            quote! {}
        } else {
            quote! { #[derive(#(#derives),*)] }
        };

        let has_serde = s.derives.iter().any(|d| {
            matches!(
                derives::from_str(d.as_str()),
                Some(DeriveId::Serialize) | Some(DeriveId::Deserialize)
            )
        });

        let is_tuple_struct =
            !s.fields.is_empty() && s.fields.iter().all(|f| f.name.chars().all(|c| c.is_ascii_digit()));

        // RFC 023: emit generic type parameters with trait bounds (declaration) and bare names (type positions).
        let generics = self.emit_type_params(&s.type_params);
        let generics_bare = self.emit_type_params_bare(&s.type_params);

        if is_tuple_struct {
            let tuple_fields: Vec<TokenStream> = s
                .fields
                .iter()
                .map(|f| {
                    let fty = self.emit_type(&f.ty);
                    let fvis = self.emit_visibility(&f.visibility);
                    quote! { #fvis #fty }
                })
                .collect();

            // Emit struct definition
            let struct_def = quote! {
                #derive_attr
                #vis struct #name #generics (#(#tuple_fields),*);
            };

            // Note: Constructor generation for newtypes is deferred until trait bound propagation
            // is implemented properly. For now, users must construct newtypes directly.
            let constructor_impl = quote! {};

            // Emit trait delegations for newtypes (single-field tuple structs)
            let trait_impls = self.emit_newtype_trait_delegations(s)?;

            Ok(quote! {
                #struct_def
                #constructor_impl
                #trait_impls
            })
        } else {
            let fields: Vec<TokenStream> = s
                .fields
                .iter()
                .map(|f| {
                    let fname = format_ident!("{}", &f.name);
                    let fty = self.emit_type(&f.ty);
                    let fvis = self.emit_visibility(&f.visibility);
                    let serde_attr = if has_serde {
                        f.alias
                            .as_ref()
                            .map(|alias| quote! { #[serde(rename = #alias)] })
                            .unwrap_or_else(|| quote! {})
                    } else {
                        quote! {}
                    };
                    quote! { #serde_attr #fvis #fname: #fty }
                })
                .collect();

            let constructor = if !s.fields.is_empty() {
                let param_tokens: Vec<TokenStream> = s
                    .fields
                    .iter()
                    .map(|f| {
                        let fname = format_ident!("{}", &f.name);
                        let fty = self.emit_type(&f.ty);
                        quote! { #fname: #fty }
                    })
                    .collect();
                let field_assigns: Vec<TokenStream> = s
                    .fields
                    .iter()
                    .map(|f| {
                        let fname = format_ident!("{}", &f.name);
                        quote! { #fname }
                    })
                    .collect();

                quote! {
                    #[allow(non_snake_case, clippy::too_many_arguments)]
                    #vis fn #name #generics (#(#param_tokens),*) -> #name #generics_bare {
                        #name {
                            #(#field_assigns),*
                        }
                    }
                }
            } else {
                quote! {}
            };

            Ok(quote! {
                #derive_attr
                #vis struct #name #generics {
                    #(#fields),*
                }

                #constructor
            })
        }
    }

    pub(in crate::backend::ir::emit) fn emit_enum(
        &self,
        e: &super::super::super::decl::IrEnum,
    ) -> Result<TokenStream, EmitError> {
        let name = format_ident!("{}", &e.name);
        let vis = self.emit_visibility(&e.visibility);

        let variants: Vec<TokenStream> = e
            .variants
            .iter()
            .map(|v| {
                let vname = format_ident!("{}", &v.name);
                match &v.fields {
                    super::super::super::decl::VariantFields::Unit => quote! { #vname },
                    super::super::super::decl::VariantFields::Tuple(types) => {
                        let type_tokens: Vec<_> = types.iter().map(|t| self.emit_type(t)).collect();
                        quote! { #vname(#(#type_tokens),*) }
                    }
                    super::super::super::decl::VariantFields::Struct(fields) => {
                        let field_tokens: Vec<_> = fields
                            .iter()
                            .map(|f| {
                                let fname = format_ident!("{}", &f.name);
                                let fty = self.emit_type(&f.ty);
                                quote! { #fname: #fty }
                            })
                            .collect();
                        quote! { #vname { #(#field_tokens),* } }
                    }
                }
            })
            .collect();

        let derives: Vec<TokenStream> = e
            .derives
            .iter()
            .map(|d| match derives::from_str(d.as_str()) {
                Some(DeriveId::Serialize) => quote! { serde::Serialize },
                Some(DeriveId::Deserialize) => quote! { serde::Deserialize },
                _ => {
                    let d_ident = format_ident!("{}", d);
                    quote! { #d_ident }
                }
            })
            .collect();

        let derive_attr = if derives.is_empty() {
            quote! {}
        } else {
            quote! { #[derive(#(#derives),*)] }
        };

        let variant_match_arms: Vec<TokenStream> = e
            .variants
            .iter()
            .map(|v| {
                let vname = format_ident!("{}", &v.name);
                let vname_str = &v.name;
                match &v.fields {
                    super::super::super::decl::VariantFields::Unit => {
                        quote! { Self::#vname => #vname_str.to_string() }
                    }
                    super::super::super::decl::VariantFields::Tuple(types) => {
                        let wildcards: Vec<_> = (0..types.len()).map(|_| quote! { _ }).collect();
                        quote! { Self::#vname(#(#wildcards),*) => #vname_str.to_string() }
                    }
                    super::super::super::decl::VariantFields::Struct(_) => {
                        quote! { Self::#vname { .. } => #vname_str.to_string() }
                    }
                }
            })
            .collect();

        // RFC 023: emit generic type parameters with trait bounds (declaration) and bare names (type positions).
        let generics = self.emit_type_params(&e.type_params);
        let generics_bare = self.emit_type_params_bare(&e.type_params);

        Ok(quote! {
            #derive_attr
            #vis enum #name #generics {
                #(#variants),*
            }

            impl #generics #name #generics_bare {
                pub fn message(&self) -> String {
                    match self {
                        #(#variant_match_arms),*
                    }
                }
            }
        })
    }

    /// Emit automatic trait delegation implementations for newtypes.
    ///
    /// For a newtype (single-field tuple struct), this generates trait implementations that delegate to the wrapped
    /// type (`self.0.trait_method()`). This enables transparent usage of newtypes with external traits (e.g.,
    /// `axum::response::IntoResponse`) without manual forwarding.
    ///
    /// The trait bridges are defined in `incan_core::lang::trait_bridges::TRAIT_BRIDGES`. Each entry maps a dunder
    /// method name (e.g., `__into_response__`) to a Rust trait and method, along with a type path pattern that
    /// determines when auto-delegation applies.
    ///
    /// **Type-based applicability**: Auto-delegation only occurs when the wrapped type's path matches the bridge's
    /// pattern. For example, IntoResponse is only emitted for types from `axum::response::`. This prevents
    /// incorrect trait impls (e.g., `UserId(i64)` won't get IntoResponse).
    ///
    /// **Phase 3 complete**: Override detection is now implemented. If a user provides a dunder method (e.g.,
    /// `__into_response__`), the auto-delegation for that trait is skipped, allowing full customization.
    ///
    /// # Examples
    ///
    /// **Auto-delegation** (for types from matching modules):
    /// ```incan
    /// type Response = newtype AxumResponse:
    /// ```
    /// Emits:
    /// ```rust,ignore
    /// impl axum::response::IntoResponse for Response {
    ///     fn into_response(self) -> axum::response::Response {
    ///         self.0.into_response()
    ///     }
    /// }
    /// ```
    ///
    /// **User override** (custom implementation):
    /// ```incan
    /// type Response = newtype AxumResponse:
    ///     def __into_response__(self) -> AxumResponse:
    ///         # Custom logic here
    ///         return self.0
    /// ```
    /// No auto-delegation is emitted; user's trait impl is used instead.
    ///
    /// **No delegation** (type doesn't match pattern):
    /// ```incan
    /// type UserId = newtype i64:
    /// ```
    /// No trait impl generated (i64 is not from `axum::response::`).
    ///
    /// # Parameters
    ///
    /// - `s`: The struct definition (must be a newtype: single-field tuple struct)
    ///
    /// # Returns
    ///
    /// - `Ok(TokenStream)`: Generated trait impl blocks, or empty if no applicable traits
    ///
    /// # Errors
    ///
    /// Currently does not return errors, but signature allows for future validation.
    pub(in crate::backend::ir::emit) fn emit_newtype_trait_delegations(
        &self,
        s: &super::super::super::decl::IrStruct,
    ) -> Result<TokenStream, EmitError> {
        // Only newtypes (single-field tuple structs) get trait delegations
        let is_newtype = s.fields.len() == 1 && s.fields[0].name == "0";
        if !is_newtype {
            return Ok(quote! {});
        }

        let name = Self::rust_ident(&s.name);
        let generics = self.emit_type_params(&s.type_params);
        let generics_bare = self.emit_type_params_bare(&s.type_params);

        // ---- Extract wrapped type path for applicability checking ----
        let wrapped_type = &s.fields[0].ty;
        let base_type_name = match wrapped_type {
            super::super::super::types::IrType::Struct(name) | super::super::super::types::IrType::Enum(name) => {
                name.as_str()
            }
            super::super::super::types::IrType::NamedGeneric(name, _) => name.as_str(),
            // Primitives/collections don't have delegation yet (Phase 3)
            _ => "",
        };

        // ---- Resolve Rust import aliases for pattern matching ----
        // For Rust imports like `from rust::axum::response import Response as AxumResponse`,
        // we need to resolve "AxumResponse" → "axum::response::Response" to match applicability
        // patterns like "axum::response::".
        let type_path = if let Some(import_path) = self.rust_import_paths.borrow().get(base_type_name) {
            // Build full module path: ["axum", "response", "Response"] → "axum::response::Response"
            import_path.join("::")
        } else {
            // Not a Rust import alias, use type name as-is
            base_type_name.to_string()
        };

        // ---- Check TRAIT_BRIDGES registry for applicable delegations ----
        let mut impls = Vec::new();

        // Get user-provided overrides for this type (if any)
        let overrides = self
            .trait_bridge_overrides
            .borrow()
            .get(&s.name)
            .cloned()
            .unwrap_or_default();

        for bridge in trait_bridges::TRAIT_BRIDGES.iter() {
            // Skip if wrapped type doesn't match this bridge's applicability pattern
            if !trait_bridges::bridge_applies_to_type(bridge, &type_path) {
                continue;
            }

            // Phase 3: Skip if user provided an override for this trait
            if overrides.contains(bridge.dunder_method) {
                continue;
            }

            // ---- Register required imports for this trait bridge ----
            for import in bridge.required_imports {
                self.trait_bridge_imports.borrow_mut().insert(import.to_string());
                // If we're importing from serde, mark that we need serde in Cargo.toml
                if import.starts_with("serde::") {
                    *self.needs_serde.borrow_mut() = true;
                }
            }

            // ---- Emit delegation impl ----
            let trait_method = format_ident!("{}", bridge.trait_method);

            // Parse trait path and return type for quote interpolation
            // Note: unwrap is safe here because TRAIT_BRIDGES entries are validated at compile time
            let trait_path_tokens: syn::Path = syn::parse_str(bridge.trait_path).unwrap_or_else(|_| {
                // Fallback to generate a compile error in the emitted Rust code
                syn::parse_quote! { compile_error!("Invalid trait path") }
            });

            // Build trait name with generics: e.g., FromRequest<S>
            // Parse trait_generics as TokenStream to handle complex types like ()
            let trait_name_with_generics = if bridge.trait_generics.is_empty() {
                quote! { #trait_path_tokens }
            } else {
                let generics_str = bridge
                    .trait_generics
                    .trim_start_matches('<')
                    .trim_end_matches('>')
                    .trim();

                // Parse as TokenStream to handle unit type () and other complex generic args
                let generics_tokens: proc_macro2::TokenStream = generics_str.parse().unwrap_or_else(|_| {
                    quote! { /* invalid trait generics */ }
                });

                quote! { #trait_path_tokens < #generics_tokens > }
            };

            let return_type_tokens: syn::Type = syn::parse_str(bridge.return_type).unwrap_or_else(|_| {
                syn::parse_quote! { compile_error!("Invalid return type") }
            });

            // Parse wrapped type path for delegation
            // For generics, we need to reconstruct the full type: e.g., "AxumQuery<T>"
            let wrapped_type_base_tokens: syn::Path = syn::parse_str(&type_path).unwrap_or_else(|_| {
                syn::parse_str(base_type_name).unwrap_or_else(|_| {
                    syn::parse_quote! { compile_error!("Invalid wrapped type") }
                })
            });

            // Build full wrapped type with generics: e.g., Query<T>
            let wrapped_type_with_generics = if s.type_params.is_empty() {
                quote! { #wrapped_type_base_tokens }
            } else {
                // Extract just the param names as TokenStream
                let param_names: Vec<_> = s
                    .type_params
                    .iter()
                    .map(|tp| {
                        let name = format_ident!("{}", &tp.name);
                        quote! { #name }
                    })
                    .collect();
                quote! { #wrapped_type_base_tokens < #(#param_names),* > }
            };

            // ---- Build impl generics (newtype's generics + extra generics from bridge) ----

            // Build a list of generic parameter NAMES (identifiers) without angle brackets
            let impl_generic_params: Vec<proc_macro2::Ident> = if bridge.extra_impl_generics.is_empty() {
                // No extra generics, just use the newtype's type params
                s.type_params.iter().map(|tp| format_ident!("{}", &tp.name)).collect()
            } else {
                // Combine newtype type params + extra params from bridge
                let mut params = Vec::new();

                // Add existing type params from the newtype
                for tp in &s.type_params {
                    let name = format_ident!("{}", &tp.name);
                    params.push(name);
                }

                // Add extra params from the bridge
                for param in bridge.extra_impl_generics {
                    let ident = format_ident!("{}", param);
                    params.push(ident);
                }

                params
            };

            // ---- Where clause ----
            let where_clause_tokens = if !bridge.where_clause.is_empty() {
                // Substitute {wrapped_type} placeholder before parsing
                let wrapped_type_str = wrapped_type_with_generics.to_string().replace(" ", "");
                let where_clause_substituted = bridge.where_clause.replace("{wrapped_type}", &wrapped_type_str);

                // Parse the where clause text (must include "where" keyword)
                match syn::parse_str::<syn::WhereClause>(&where_clause_substituted) {
                    Ok(wc) => quote! { #wc },
                    Err(_e) => {
                        // Fallback: emit as comment if parse fails
                        quote! { /* invalid where clause: #where_clause_substituted */ }
                    }
                }
            } else {
                quote! {}
            };

            // Note: required_imports would need to be emitted at module level, not in impl blocks
            // For now, we rely on existing imports (Future is in std::prelude for async functions)

            // ---- Generate associated types ----
            let associated_type_decls: Vec<_> = bridge
                .associated_types
                .iter()
                .map(|(name, value)| {
                    let type_name = format_ident!("{}", name);
                    // Replace {wrapped_type} placeholder with actual wrapped type (with generics)
                    let wrapped_type_str = wrapped_type_with_generics.to_string().replace(" ", "");

                    // For associated types coming from traits, we need qualified syntax:
                    // <Type as Trait>::AssociatedType
                    // Check if value references {wrapped_type} - if so, it's likely from the wrapped type's impl
                    let type_value_str = if let Some(assoc_type_name) = value.strip_prefix("{wrapped_type}::") {
                        // Pattern: {wrapped_type}::AssociatedType
                        // Need to use qualified syntax: <WrappedType as Trait>::AssociatedType
                        // Build qualified path with trait generics if present
                        if bridge.trait_generics.is_empty() {
                            format!("<{} as {}>::{}", wrapped_type_str, bridge.trait_path, assoc_type_name)
                        } else {
                            let trait_gen = bridge.trait_generics.trim_start_matches('<').trim_end_matches('>');
                            format!(
                                "<{} as {}<{}>>::{}",
                                wrapped_type_str, bridge.trait_path, trait_gen, assoc_type_name
                            )
                        }
                    } else {
                        // Simple replacement
                        value.replace("{wrapped_type}", &wrapped_type_str)
                    };

                    let type_value_tokens: syn::Type = syn::parse_str(&type_value_str).unwrap_or_else(|_| {
                        syn::parse_quote! { compile_error!("Invalid associated type") }
                    });
                    quote! { type #type_name = #type_value_tokens; }
                })
                .collect();

            // ---- Generate function signature ----
            let async_token = if bridge.is_async {
                quote! { async }
            } else {
                quote! {}
            };

            let (fn_params, call_args): (Vec<_>, Vec<_>) = if bridge.is_static {
                // Static function: use explicit parameters
                let params: Vec<_> = bridge
                    .parameters
                    .iter()
                    .map(|(name, ty)| {
                        let param_name = format_ident!("{}", name);
                        let param_type: syn::Type = syn::parse_str(ty).unwrap_or_else(|_| {
                            syn::parse_quote! { compile_error!("Invalid parameter type") }
                        });
                        (quote! { #param_name: #param_type }, format_ident!("{}", name))
                    })
                    .collect();
                params.into_iter().unzip()
            } else {
                // Instance method: just `self`
                (vec![quote! { self }], vec![format_ident!("self")])
            };

            // ---- Generate delegation body ----
            let delegation_body = if !bridge.delegation_pattern.is_empty() {
                // Custom delegation pattern
                let wrapped_type_str = wrapped_type_with_generics.to_string().replace(" ", "");

                // In expression context, generic types need turbofish syntax
                // Convert Query<T> to Query::<T> for use in paths like Query::<T>::from_request
                let wrapped_type_expr = if wrapped_type_str.contains('<') {
                    wrapped_type_str.replacen('<', "::<", 1)
                } else {
                    wrapped_type_str.clone()
                };

                let pattern = bridge
                    .delegation_pattern
                    .replace("{wrapped_type}", &wrapped_type_expr)
                    .replace("{method}", bridge.trait_method)
                    .replace(
                        "{params}",
                        &call_args.iter().map(|a| a.to_string()).collect::<Vec<_>>().join(", "),
                    );

                let body_tokens: proc_macro2::TokenStream = pattern.parse().unwrap_or_else(|_| {
                    quote! { compile_error!("Invalid delegation pattern") }
                });
                quote! { #body_tokens }
            } else if bridge.is_static {
                // Static function: call wrapped type's static method
                // Need turbofish syntax for generics in expression context
                let wrapped_type_full = wrapped_type_with_generics.clone();
                quote! { #wrapped_type_full::#trait_method(#(#call_args),*) }
            } else {
                // Instance method: simple delegation
                quote! { self.0.#trait_method() }
            };

            // Build impl block - construct the entire signature coherently to avoid angle bracket parsing issues
            let impl_block = if bridge.extra_impl_generics.is_empty() {
                // Simple case: no extra generics
                quote! {
                    impl #generics #trait_name_with_generics for #name #generics_bare #where_clause_tokens {
                        #(#associated_type_decls)*

                        #async_token fn #trait_method(#(#fn_params),*) -> #return_type_tokens {
                            #delegation_body
                        }
                    }
                }
            } else {
                // Complex case: extra generics - build the full signature coherently
                // Extract just the newtype's type param names for the "for Type<T>" part
                let newtype_params: Vec<_> = s.type_params.iter().map(|tp| format_ident!("{}", &tp.name)).collect();

                // Build trait generics from bridge.trait_generics
                // Parse as TokenStream to handle complex types like () or type-level values
                let trait_generics_tokens: Option<proc_macro2::TokenStream> = if bridge.trait_generics.is_empty() {
                    None
                } else {
                    let generics_str = bridge
                        .trait_generics
                        .trim_start_matches('<')
                        .trim_end_matches('>')
                        .trim();
                    Some(generics_str.parse().unwrap_or_else(|_| {
                        quote! { /* invalid trait generics */ }
                    }))
                };

                // Now build the impl with properly separated generics
                if let Some(tg_tokens) = trait_generics_tokens {
                    quote! {
                        impl< #(#impl_generic_params),* > #trait_path_tokens< #tg_tokens > for #name< #(#newtype_params),* > #where_clause_tokens {
                            #(#associated_type_decls)*

                            #async_token fn #trait_method(#(#fn_params),*) -> #return_type_tokens {
                                #delegation_body
                            }
                        }
                    }
                } else {
                    quote! {
                        impl< #(#impl_generic_params),* > #trait_path_tokens for #name< #(#newtype_params),* > #where_clause_tokens {
                            #(#associated_type_decls)*

                            #async_token fn #trait_method(#(#fn_params),*) -> #return_type_tokens {
                                #delegation_body
                            }
                        }
                    }
                }
            };

            impls.push(impl_block);
        }

        Ok(quote! {
            #(#impls)*
        })
    }
}
