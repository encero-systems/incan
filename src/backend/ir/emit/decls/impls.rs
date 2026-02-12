//! Impl block emission.
//!
//! Handles `emit_impl` (including serde convenience methods, `@derive(Validate)`, trait impls, and `__fields__`
//! reflection).

use proc_macro2::{Literal, TokenStream};
use quote::{format_ident, quote};

use incan_core::lang::conventions;
use incan_core::lang::derives::{self, DeriveId};
use incan_core::lang::magic_methods;

use super::super::super::types::IrType;
use super::super::{EmitError, IrEmitter};

impl<'a> IrEmitter<'a> {
    pub(in crate::backend::ir::emit) fn emit_impl(
        &self,
        impl_block: &super::super::super::decl::IrImpl,
    ) -> Result<TokenStream, EmitError> {
        let target_type = format_ident!("{}", &impl_block.target_type);

        let mut regular_methods = Vec::new();
        let mut trait_impls = Vec::new();

        for method in &impl_block.methods {
            match magic_methods::from_str(method.name.as_str()) {
                Some(magic_methods::MagicMethodId::Eq) => {
                    let body_stmts: Vec<TokenStream> = method
                        .body
                        .iter()
                        .map(|s| self.emit_stmt(s))
                        .collect::<Result<_, _>>()?;
                    trait_impls.push(quote! {
                        impl PartialEq for #target_type {
                            fn eq(&self, other: &Self) -> bool {
                                #(#body_stmts)*
                            }
                        }
                    });
                }
                Some(magic_methods::MagicMethodId::Str) => {
                    regular_methods.push(self.emit_method(method)?);
                    trait_impls.push(quote! {
                        impl std::fmt::Display for #target_type {
                            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                                write!(f, "{}", self.__str__())
                            }
                        }
                    });
                }
                Some(magic_methods::MagicMethodId::ClassName) | Some(magic_methods::MagicMethodId::Fields) => {
                    regular_methods.push(self.emit_method(method)?)
                }
                _ => regular_methods.push(self.emit_method(method)?),
            }
        }

        let fields_name = magic_methods::as_str(magic_methods::MagicMethodId::Fields);
        let has_fields_method = impl_block.methods.iter().any(|m| m.name == fields_name);
        if impl_block.trait_name.is_none()
            && !has_fields_method
            && let Some(fields_method) = self.emit_fields_method(&impl_block.target_type)?
        {
            regular_methods.push(fields_method);
        }

        // serde-derived convenience methods (legacy behavior)
        if impl_block.trait_name.is_none()
            && let Some(derives) = self.struct_derives.get(&impl_block.target_type)
        {
            let has_serialize = derives
                .iter()
                .any(|d| derives::from_str(d.as_str()) == Some(DeriveId::Serialize));
            let has_deserialize = derives
                .iter()
                .any(|d| derives::from_str(d.as_str()) == Some(DeriveId::Deserialize));

            if has_serialize {
                regular_methods.push(quote! {
                    /// Serialize this model to a JSON string
                    pub fn to_json(&self) -> String {
                        serde_json::to_string(self).unwrap_or_else(|_| {
                            incan_stdlib::errors::raise_json_serialization_error(stringify!(#target_type))
                        })
                    }
                });
            }
            if has_deserialize {
                regular_methods.push(quote! {
                    /// Deserialize a JSON string into this model
                    pub fn from_json(json_str: String) -> Result<Self, String> {
                        serde_json::from_str(&json_str)
                            .map_err(|e| incan_stdlib::errors::json_decode_error_string(e))
                    }
                });
            }
        }

        // @derive(Validate): generate `TypeName::new(...) -> Result[TypeName, E]` that calls `validate()`.
        if impl_block.trait_name.is_none()
            && let Some(derives) = self.struct_derives.get(&impl_block.target_type)
        {
            let has_validate = derives
                .iter()
                .any(|d| derives::from_str(d.as_str()) == Some(DeriveId::Validate));
            if has_validate
                && !impl_block.methods.iter().any(|m| m.name == conventions::NEW_METHOD)
                && let Some(validate_fn) = impl_block
                    .methods
                    .iter()
                    .find(|m| m.name == conventions::VALIDATE_METHOD)
            {
                let ret_ty = self.emit_type(&validate_fn.return_type);

                let field_names = self
                    .struct_field_names
                    .get(&impl_block.target_type)
                    .cloned()
                    .unwrap_or_default();

                let mut params: Vec<TokenStream> = Vec::new();
                let mut init_fields: Vec<TokenStream> = Vec::new();

                for fname in field_names {
                    let f_ident = format_ident!("{}", fname);
                    if let Some(default_expr) = self
                        .struct_field_defaults
                        .get(&(impl_block.target_type.clone(), fname.clone()))
                    {
                        let default_tokens = self.emit_expr(default_expr)?;
                        init_fields.push(quote! { #f_ident: #default_tokens });
                    } else {
                        let f_ty = self
                            .struct_field_types
                            .get(&(impl_block.target_type.clone(), fname.clone()))
                            .cloned()
                            .unwrap_or(IrType::Unknown);
                        let f_ty_tokens = self.emit_type(&f_ty);
                        params.push(quote! { #f_ident: #f_ty_tokens });
                        init_fields.push(quote! { #f_ident });
                    }
                }

                regular_methods.push(quote! {
                    /// Construct a validated instance of this model.
                    pub fn new(#(#params),*) -> #ret_ty {
                        let tmp = Self { #(#init_fields),* };
                        tmp.validate()
                    }
                });
            }
        }

        let main_impl = if !regular_methods.is_empty() || impl_block.trait_name.is_none() {
            if let Some(trait_name) = &impl_block.trait_name {
                let trait_methods: Vec<TokenStream> = impl_block
                    .methods
                    .iter()
                    .filter(|m| {
                        !matches!(
                            magic_methods::from_str(m.name.as_str()),
                            Some(
                                magic_methods::MagicMethodId::Eq
                                    | magic_methods::MagicMethodId::Str
                                    | magic_methods::MagicMethodId::ClassName
                                    | magic_methods::MagicMethodId::Fields
                            )
                        )
                    })
                    .map(|m| self.emit_trait_method(m))
                    .collect::<Result<_, _>>()?;
                let trait_ident = format_ident!("{}", trait_name);
                quote! {
                    impl #trait_ident for #target_type {
                        #(#trait_methods)*
                    }
                }
            } else if !regular_methods.is_empty() {
                quote! {
                    impl #target_type {
                        #(#regular_methods)*
                    }
                }
            } else {
                quote! {}
            }
        } else if let Some(trait_name) = &impl_block.trait_name {
            let trait_ident = format_ident!("{}", trait_name);
            quote! {
                impl #trait_ident for #target_type {}
            }
        } else {
            quote! {}
        };

        Ok(quote! {
            #main_impl
            #(#trait_impls)*
        })
    }

    fn emit_fields_method(&self, struct_name: &str) -> Result<Option<TokenStream>, EmitError> {
        let Some(field_names) = self.struct_field_names.get(struct_name) else {
            return Ok(None);
        };
        let mut field_infos = Vec::new();

        for field_name in field_names {
            let key = (struct_name.to_string(), field_name.clone());
            let ty = self.struct_field_types.get(&key).ok_or_else(|| {
                EmitError::Unsupported(format!(
                    "missing field type metadata for '{}.{}'",
                    struct_name, field_name
                ))
            })?;
            let alias = self.struct_field_aliases.get(&key).and_then(|v| v.clone());
            let description = self.struct_field_descriptions.get(&key).and_then(|v| v.clone());
            let has_default = self.struct_field_defaults.contains_key(&key);

            let alias_token = alias
                .as_ref()
                .map(|a| quote! { Some(incan_stdlib::frozen::FrozenStr::new(#a)) })
                .unwrap_or_else(|| quote! { None });
            let description_token = description
                .as_ref()
                .map(|d| quote! { Some(incan_stdlib::frozen::FrozenStr::new(#d)) })
                .unwrap_or_else(|| quote! { None });
            let wire_name = alias.as_deref().unwrap_or(field_name);
            // RFC 021: Use Incan-style type name, not Rust type name
            let type_name = ty.incan_name();

            field_infos.push(quote! {
                incan_stdlib::reflection::FieldInfo {
                    name: incan_stdlib::frozen::FrozenStr::new(#field_name),
                    alias: #alias_token,
                    description: #description_token,
                    wire_name: incan_stdlib::frozen::FrozenStr::new(#wire_name),
                    type_name: incan_stdlib::frozen::FrozenStr::new(#type_name),
                    has_default: #has_default,
                    extra: incan_stdlib::frozen::FrozenDict::new(&[]),
                }
            });
        }

        let field_count = Literal::usize_unsuffixed(field_infos.len());
        Ok(Some(quote! {
            /// Returns field metadata for this type.
            pub fn __fields__(&self) -> incan_stdlib::frozen::FrozenList<incan_stdlib::reflection::FieldInfo> {
                static __INCAN_FIELDS: [incan_stdlib::reflection::FieldInfo; #field_count] = [#(#field_infos),*];
                incan_stdlib::frozen::FrozenList::new(&__INCAN_FIELDS)
            }
        }))
    }
}
