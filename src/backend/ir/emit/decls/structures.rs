//! Struct and enum emission.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use incan_core::lang::derives::{self, DeriveId};

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
                    if let Some(module_path) = s.derive_rust_modules.get(d) {
                        let mut segs: Vec<TokenStream> = module_path
                            .split("::")
                            .map(Self::rust_ident)
                            .map(|id| quote! { #id })
                            .collect();
                        let d_ident = Self::rust_ident(d);
                        segs.push(quote! { #d_ident });
                        super::join_path_tokens(&segs)
                    } else {
                        let d_ident = format_ident!("{}", d);
                        quote! { #d_ident }
                    }
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

            Ok(quote! {
                #struct_def
                #constructor_impl
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
                    if let Some(module_path) = e.derive_rust_modules.get(d) {
                        let mut segs: Vec<TokenStream> = module_path
                            .split("::")
                            .map(Self::rust_ident)
                            .map(|id| quote! { #id })
                            .collect();
                        let d_ident = Self::rust_ident(d);
                        segs.push(quote! { #d_ident });
                        super::join_path_tokens(&segs)
                    } else {
                        let d_ident = format_ident!("{}", d);
                        quote! { #d_ident }
                    }
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
}
