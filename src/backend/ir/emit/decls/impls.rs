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
    /// Emit an impl block, including generated convenience methods and trait impl adapters.
    pub(in crate::backend::ir::emit) fn emit_impl(
        &self,
        impl_block: &super::super::super::decl::IrImpl,
    ) -> Result<TokenStream, EmitError> {
        let target_type = format_ident!("{}", &impl_block.target_type);

        // RFC 023: emit generic type parameters with trait bounds (declaration) and bare names (type positions).
        let generics = self.emit_type_params(&impl_block.type_params);
        let generics_bare = self.emit_type_params_bare(&impl_block.type_params);

        let mut regular_methods = Vec::new();
        let mut trait_impls = Vec::new();

        for method in &impl_block.methods {
            let method_is_needed = self.should_emit_method(&impl_block.target_type, &method.name, &method.visibility)
                || !method.lint_allows.is_empty()
                || !method.rust_attributes.is_empty();
            match magic_methods::from_str(method.name.as_str()) {
                Some(magic_methods::MagicMethodId::Eq) => {
                    let body_stmts = self.emit_stmts(&method.body)?;
                    trait_impls.push(quote! {
                        impl #generics PartialEq for #target_type #generics_bare {
                            fn eq(&self, other: &Self) -> bool {
                                #(#body_stmts)*
                            }
                        }
                    });
                }
                Some(magic_methods::MagicMethodId::Str) => {
                    regular_methods.push(self.emit_method(method)?);
                    trait_impls.push(quote! {
                        impl #generics std::fmt::Display for #target_type #generics_bare {
                            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                                write!(f, "{}", self.__str__())
                            }
                        }
                    });
                }
                Some(magic_methods::MagicMethodId::ClassName) | Some(magic_methods::MagicMethodId::Fields)
                    if method_is_needed =>
                {
                    regular_methods.push(self.emit_method(method)?);
                }
                _ if method_is_needed => {
                    regular_methods.push(self.emit_method(method)?);
                }
                _ => {}
            }
        }

        let fields_name = magic_methods::as_str(magic_methods::MagicMethodId::Fields);
        let has_fields_method = impl_block.methods.iter().any(|m| m.name == fields_name);
        if impl_block.trait_name.is_none()
            && !has_fields_method
            && self.should_emit_method(
                &impl_block.target_type,
                fields_name,
                &super::super::super::decl::Visibility::Private,
            )
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

            if has_serialize
                && self.should_emit_method(
                    &impl_block.target_type,
                    "to_json",
                    &super::super::super::decl::Visibility::Private,
                )
            {
                regular_methods.push(quote! {
                    /// Serialize this model to a JSON string
                    pub fn to_json(&self) -> String {
                        incan_stdlib::json::__private::stringify_or_raise(self, stringify!(#target_type))
                    }
                });
            }
            if has_deserialize
                && self.should_emit_method(
                    &impl_block.target_type,
                    "from_json",
                    &super::super::super::decl::Visibility::Private,
                )
            {
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
                && self.should_emit_method(
                    &impl_block.target_type,
                    conventions::NEW_METHOD,
                    &super::super::super::decl::Visibility::Private,
                )
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

        let main_impl = if let Some(trait_name) = &impl_block.trait_name {
            let mut trait_methods: Vec<TokenStream> = impl_block
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
            if Self::is_serde_serialize_trait_name(trait_name)
                && !impl_block.methods.iter().any(|method| method.name == "to_json")
            {
                trait_methods.push(quote! {
                    fn to_json(&self) -> String {
                        incan_stdlib::json::__private::stringify_or_raise(self, stringify!(#target_type))
                    }
                });
            }
            if Self::is_serde_deserialize_trait_name(trait_name)
                && !impl_block.methods.iter().any(|method| method.name == "from_json")
            {
                trait_methods.push(quote! {
                    fn from_json(json_str: String) -> Result<Self, String> {
                        serde_json::from_str(&json_str)
                            .map_err(|e| incan_stdlib::errors::json_decode_error_string(e))
                    }
                });
            }
            let trait_tokens = self.emit_supertrait_bound_path(trait_name, &impl_block.trait_type_args);
            quote! {
                impl #generics #trait_tokens for #target_type #generics_bare {
                    #(#trait_methods)*
                }
            }
        } else if !regular_methods.is_empty() {
            quote! {
                impl #generics #target_type #generics_bare {
                    #(#regular_methods)*
                }
            }
        } else {
            quote! {}
        };

        Ok(quote! {
            #main_impl
            #(#trait_impls)*
        })
    }

    /// Return whether a trait impl target names the stdlib JSON serialization trait or an imported alias of it.
    fn is_serde_serialize_trait_name(trait_name: &str) -> bool {
        matches!(
            trait_name,
            "Serialize" | "JsonSerialize" | "json.Serialize" | "std.serde.json.Serialize"
        )
    }

    /// Return whether a trait impl target names the stdlib JSON deserialization trait or an imported alias of it.
    fn is_serde_deserialize_trait_name(trait_name: &str) -> bool {
        matches!(
            trait_name,
            "Deserialize" | "JsonDeserialize" | "json.Deserialize" | "std.serde.json.Deserialize"
        )
    }

    /// Emit the generated `__fields__` reflection method for a struct when field metadata is available.
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
