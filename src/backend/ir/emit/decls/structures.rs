//! Struct and enum emission.

use proc_macro2::{Ident, Literal, TokenStream};
use quote::{format_ident, quote};

use incan_core::lang::derives::{self, DeriveId};

use super::super::super::decl::{IrEnum, IrEnumValue, IrEnumValueType, IrStruct, VariantFields};
use super::super::{EmitError, IrEmitter};

const SERDE_SERIALIZE_DERIVE: &str = "serde::Serialize";
const SERDE_DESERIALIZE_DERIVE: &str = "serde::Deserialize";

impl<'a> IrEmitter<'a> {
    /// Emit a field-level expectation for private generated fields that must remain present for Incan semantics even
    /// when Rust cannot observe a read in the generated program.
    fn private_field_dead_code_expect(
        &self,
        struct_name: &str,
        field_name: &str,
        visibility: &super::super::super::decl::Visibility,
    ) -> TokenStream {
        if self.should_expect_private_field_dead_code(struct_name, field_name, visibility) {
            quote! { #[expect(dead_code, reason = "retained for Incan private field semantics")] }
        } else {
            quote! {}
        }
    }

    /// Emit a Rust struct definition and any supported constructor surface.
    pub(in crate::backend::ir::emit) fn emit_struct(&self, s: &IrStruct) -> Result<TokenStream, EmitError> {
        let name = Self::rust_ident(&s.name);
        let vis = self.emit_visibility(&s.visibility);

        let derives: Vec<TokenStream> = s
            .derives
            .iter()
            // `Validate` is an Incan semantic derive (not a Rust derive macro).
            .filter(|d| derives::from_str(d.as_str()) != Some(DeriveId::Validate))
            .map(|d| match derives::from_str(d.as_str()) {
                _ if d == "FieldInfo" => quote! { incan_derive::FieldInfo },
                _ if d == "IncanClass" => quote! { incan_derive::IncanClass },
                _ if d.contains("::") => {
                    let segs: Vec<TokenStream> = d.split("::").map(Self::rust_ident).map(|id| quote! { #id }).collect();
                    super::join_path_tokens(&segs)
                }
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
        let lint_allows = self.emit_rust_lint_allows(&s.lint_allows);

        let has_serde = s
            .derives
            .iter()
            .any(|d| d == SERDE_SERIALIZE_DERIVE || d == SERDE_DESERIALIZE_DERIVE);

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
                    let dead_code_expect = self.private_field_dead_code_expect(&s.name, &f.name, &f.visibility);
                    quote! { #dead_code_expect #fvis #fty }
                })
                .collect();

            // Emit struct definition
            let struct_def = quote! {
                #(#lint_allows)*
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
                    let dead_code_expect = self.private_field_dead_code_expect(&s.name, &f.name, &f.visibility);
                    let serde_attr = if has_serde {
                        f.alias
                            .as_ref()
                            .map(|alias| quote! { #[serde(rename = #alias)] })
                            .unwrap_or_else(|| quote! {})
                    } else {
                        quote! {}
                    };
                    quote! { #dead_code_expect #serde_attr #fvis #fname: #fty }
                })
                .collect();

            let constructor = if !s.fields.is_empty() && self.should_emit_struct_constructor(&s.name) {
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
                #(#lint_allows)*
                #derive_attr
                #vis struct #name #generics {
                    #(#fields),*
                }

                #constructor
            })
        }
    }

    /// Emit a Rust enum definition plus shared and value-enum-specific helper implementations.
    pub(in crate::backend::ir::emit) fn emit_enum(&self, e: &IrEnum) -> Result<TokenStream, EmitError> {
        let name = format_ident!("{}", &e.name);
        let vis = self.emit_visibility(&e.visibility);
        let is_value_enum = e.value_type.is_some();

        let variants: Vec<TokenStream> = e
            .variants
            .iter()
            .map(|v| {
                let vname = format_ident!("{}", &v.name);
                match &v.fields {
                    VariantFields::Unit => quote! { #vname },
                    VariantFields::Tuple(types) => {
                        let type_tokens: Vec<_> = types.iter().map(|t| self.emit_type(t)).collect();
                        quote! { #vname(#(#type_tokens),*) }
                    }
                    VariantFields::Struct(fields) => {
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
            .filter(|d| {
                if !is_value_enum {
                    return true;
                }
                d.as_str() != SERDE_SERIALIZE_DERIVE
                    && d.as_str() != SERDE_DESERIALIZE_DERIVE
                    && derives::from_str(d.as_str()) != Some(DeriveId::Display)
            })
            .map(|d| match derives::from_str(d.as_str()) {
                _ if d == "FieldInfo" => quote! { incan_derive::FieldInfo },
                _ if d == "IncanClass" => quote! { incan_derive::IncanClass },
                _ if d.contains("::") => {
                    let segs: Vec<TokenStream> = d.split("::").map(Self::rust_ident).map(|id| quote! { #id }).collect();
                    super::join_path_tokens(&segs)
                }
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
        let lint_allows = self.emit_rust_lint_allows(&e.lint_allows);

        let variant_match_arms: Vec<TokenStream> = e
            .variants
            .iter()
            .map(|v| {
                let vname = format_ident!("{}", &v.name);
                let vname_str = &v.name;
                match &v.fields {
                    VariantFields::Unit => {
                        quote! { Self::#vname => #vname_str.to_string() }
                    }
                    VariantFields::Tuple(types) => {
                        let wildcards: Vec<_> = (0..types.len()).map(|_| quote! { _ }).collect();
                        quote! { Self::#vname(#(#wildcards),*) => #vname_str.to_string() }
                    }
                    VariantFields::Struct(_) => {
                        quote! { Self::#vname { .. } => #vname_str.to_string() }
                    }
                }
            })
            .collect();

        // RFC 023: emit generic type parameters with trait bounds (declaration) and bare names (type positions).
        let generics = self.emit_type_params(&e.type_params);
        let generics_bare = self.emit_type_params_bare(&e.type_params);
        let value_enum_helpers = self.emit_value_enum_helpers(e, &name, &generics, &generics_bare)?;

        Ok(quote! {
            #(#lint_allows)*
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

            #value_enum_helpers
        })
    }

    /// Emit `value()`, `from_value(...)`, display, parsing, and serde helpers for value enums.
    fn emit_value_enum_helpers(
        &self,
        e: &IrEnum,
        name: &Ident,
        generics: &TokenStream,
        generics_bare: &TokenStream,
    ) -> Result<TokenStream, EmitError> {
        let Some(value_type) = e.value_type else {
            return Ok(quote! {});
        };
        if !e.type_params.is_empty() {
            return Err(EmitError::Unsupported(format!(
                "value enum '{}' cannot have type parameters",
                e.name
            )));
        }

        match value_type {
            IrEnumValueType::String => {
                let mut value_arms = Vec::new();
                let mut from_value_arms = Vec::new();
                let mut display_arms = Vec::new();
                let mut serialize_arms = Vec::new();

                for variant in &e.variants {
                    Self::validate_value_enum_variant_is_unit(e, variant)?;
                    let Some(IrEnumValue::String(raw)) = &variant.raw_value else {
                        return Err(EmitError::Unsupported(format!(
                            "string value enum '{}.{}' is missing a string raw value",
                            e.name, variant.name
                        )));
                    };
                    let pat = Self::enum_variant_match_pattern(variant);
                    let vname = Self::rust_ident(&variant.name);
                    value_arms.push(quote! { #pat => #raw.to_string() });
                    from_value_arms.push(quote! { #raw => Some(Self::#vname) });
                    display_arms.push(quote! { #pat => formatter.write_str(#raw) });
                    serialize_arms.push(quote! { #pat => serializer.serialize_str(#raw) });
                }

                let serialize_impl = if e.derives.iter().any(|d| d == SERDE_SERIALIZE_DERIVE) {
                    quote! {
                        impl #generics serde::Serialize for #name #generics_bare {
                            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
                            where
                                S: serde::Serializer,
                            {
                                match self {
                                    #(#serialize_arms),*
                                }
                            }
                        }
                    }
                } else {
                    quote! {}
                };

                let deserialize_impl = if e.derives.iter().any(|d| d == SERDE_DESERIALIZE_DERIVE) {
                    quote! {
                        impl<'de> serde::Deserialize<'de> for #name {
                            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
                            where
                                D: serde::Deserializer<'de>,
                            {
                                let value = <String as serde::Deserialize>::deserialize(deserializer)?;
                                Self::from_value(value.as_str()).ok_or_else(|| {
                                    serde::de::Error::custom(format!(
                                        "invalid value for {}: {}",
                                        stringify!(#name),
                                        value
                                    ))
                                })
                            }
                        }
                    }
                } else {
                    quote! {}
                };

                Ok(quote! {
                    impl #generics #name #generics_bare {
                        pub fn value(&self) -> String {
                            match self {
                                #(#value_arms),*
                            }
                        }

                        pub fn from_value(value: impl AsRef<str>) -> Option<Self> {
                            match value.as_ref() {
                                #(#from_value_arms),*,
                                _ => None,
                            }
                        }
                    }

                    impl #generics std::fmt::Display for #name #generics_bare {
                        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                            match self {
                                #(#display_arms),*
                            }
                        }
                    }

                    impl std::str::FromStr for #name {
                        type Err = String;

                        fn from_str(value: &str) -> Result<Self, Self::Err> {
                            Self::from_value(value).ok_or_else(|| {
                                format!("invalid value for {}: {}", stringify!(#name), value)
                            })
                        }
                    }

                    #serialize_impl
                    #deserialize_impl
                })
            }
            IrEnumValueType::Int => {
                let mut value_arms = Vec::new();
                let mut from_value_arms = Vec::new();
                let mut display_arms = Vec::new();
                let mut serialize_arms = Vec::new();

                for variant in &e.variants {
                    Self::validate_value_enum_variant_is_unit(e, variant)?;
                    let Some(IrEnumValue::Int(raw)) = &variant.raw_value else {
                        return Err(EmitError::Unsupported(format!(
                            "integer value enum '{}.{}' is missing an integer raw value",
                            e.name, variant.name
                        )));
                    };
                    let raw_lit = Literal::i64_unsuffixed(*raw);
                    let pat = Self::enum_variant_match_pattern(variant);
                    let vname = Self::rust_ident(&variant.name);
                    value_arms.push(quote! { #pat => #raw_lit });
                    from_value_arms.push(quote! { #raw_lit => Some(Self::#vname) });
                    display_arms.push(quote! { #pat => formatter.write_str(&#raw_lit.to_string()) });
                    serialize_arms.push(quote! { #pat => serializer.serialize_i64(#raw_lit) });
                }

                let serialize_impl = if e.derives.iter().any(|d| d == SERDE_SERIALIZE_DERIVE) {
                    quote! {
                        impl #generics serde::Serialize for #name #generics_bare {
                            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
                            where
                                S: serde::Serializer,
                            {
                                match self {
                                    #(#serialize_arms),*
                                }
                            }
                        }
                    }
                } else {
                    quote! {}
                };

                let deserialize_impl = if e.derives.iter().any(|d| d == SERDE_DESERIALIZE_DERIVE) {
                    quote! {
                        impl<'de> serde::Deserialize<'de> for #name {
                            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
                            where
                                D: serde::Deserializer<'de>,
                            {
                                let value = <i64 as serde::Deserialize>::deserialize(deserializer)?;
                                Self::from_value(value).ok_or_else(|| {
                                    serde::de::Error::custom(format!(
                                        "invalid value for {}: {}",
                                        stringify!(#name),
                                        value
                                    ))
                                })
                            }
                        }
                    }
                } else {
                    quote! {}
                };

                Ok(quote! {
                    impl #generics #name #generics_bare {
                        pub fn value(&self) -> i64 {
                            match self {
                                #(#value_arms),*
                            }
                        }

                        pub fn from_value(value: i64) -> Option<Self> {
                            match value {
                                #(#from_value_arms),*,
                                _ => None,
                            }
                        }
                    }

                    impl #generics std::fmt::Display for #name #generics_bare {
                        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                            match self {
                                #(#display_arms),*
                            }
                        }
                    }

                    impl std::str::FromStr for #name {
                        type Err = String;

                        fn from_str(value: &str) -> Result<Self, Self::Err> {
                            let parsed = value.parse::<i64>().map_err(|err| err.to_string())?;
                            Self::from_value(parsed).ok_or_else(|| {
                                format!("invalid value for {}: {}", stringify!(#name), value)
                            })
                        }
                    }

                    #serialize_impl
                    #deserialize_impl
                })
            }
        }
    }

    /// Reject malformed IR where a value enum variant still carries payload fields.
    fn validate_value_enum_variant_is_unit(
        e: &IrEnum,
        variant: &super::super::super::decl::EnumVariant,
    ) -> Result<(), EmitError> {
        if matches!(variant.fields, VariantFields::Unit) {
            return Ok(());
        }
        Err(EmitError::Unsupported(format!(
            "value enum '{}.{}' cannot carry payload fields",
            e.name, variant.name
        )))
    }

    /// Build a match pattern for a generated helper arm over an enum variant.
    fn enum_variant_match_pattern(variant: &super::super::super::decl::EnumVariant) -> TokenStream {
        let vname = Self::rust_ident(&variant.name);
        match &variant.fields {
            VariantFields::Unit => quote! { Self::#vname },
            VariantFields::Tuple(types) => {
                let wildcards: Vec<_> = (0..types.len()).map(|_| quote! { _ }).collect();
                quote! { Self::#vname(#(#wildcards),*) }
            }
            VariantFields::Struct(_) => quote! { Self::#vname { .. } },
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::backend::ir::decl::{EnumVariant, IrEnum, IrEnumValue, IrEnumValueType, IrTypeParam, Visibility};
    use crate::backend::ir::{FunctionRegistry, IrType};
    use incan_core::lang::surface::constructors::{self, ConstructorId};

    fn render_enum(e: &IrEnum) -> Result<String, String> {
        let registry = FunctionRegistry::new();
        let emitter = IrEmitter::new(&registry);
        let tokens = emitter.emit_enum(e).map_err(|err| err.to_string())?;
        let file = syn::parse2::<syn::File>(tokens).map_err(|err| err.to_string())?;
        Ok(prettyplease::unparse(&file))
    }

    fn base_value_enum(name: &str, value_type: IrEnumValueType, variants: Vec<EnumVariant>) -> IrEnum {
        IrEnum {
            name: name.to_string(),
            variants,
            value_type: Some(value_type),
            derives: vec![
                derives::as_str(DeriveId::Debug).to_string(),
                derives::as_str(DeriveId::Clone).to_string(),
                derives::as_str(DeriveId::PartialEq).to_string(),
            ],
            visibility: Visibility::Public,
            type_params: Vec::<IrTypeParam>::new(),
            derive_rust_modules: HashMap::new(),
            lint_allows: Vec::new(),
        }
    }

    #[test]
    fn string_value_enum_emits_value_lookup_and_display() -> Result<(), String> {
        let rendered = render_enum(&base_value_enum(
            "Env",
            IrEnumValueType::String,
            vec![
                EnumVariant {
                    name: "Dev".to_string(),
                    fields: VariantFields::Unit,
                    raw_value: Some(IrEnumValue::String("development".to_string())),
                },
                EnumVariant {
                    name: "Prod".to_string(),
                    fields: VariantFields::Unit,
                    raw_value: Some(IrEnumValue::String("production".to_string())),
                },
            ],
        ))?;

        assert!(rendered.contains("pub fn value(&self) -> String"), "{rendered}");
        assert!(
            rendered.contains("Self::Dev => \"development\".to_string()")
                && rendered.contains("\"production\" => Some(Self::Prod)"),
            "{rendered}"
        );
        assert!(rendered.contains("impl std::fmt::Display for Env"), "{rendered}");
        assert!(
            rendered.contains("Self::Dev => \"Dev\".to_string()"),
            "message() must stay variant-name based:\n{rendered}"
        );
        Ok(())
    }

    #[test]
    fn integer_value_enum_emits_value_lookup_and_from_str() -> Result<(), String> {
        let rendered = render_enum(&base_value_enum(
            "HttpStatus",
            IrEnumValueType::Int,
            vec![
                EnumVariant {
                    name: constructors::as_str(ConstructorId::Ok).to_string(),
                    fields: VariantFields::Unit,
                    raw_value: Some(IrEnumValue::Int(200)),
                },
                EnumVariant {
                    name: "NotFound".to_string(),
                    fields: VariantFields::Unit,
                    raw_value: Some(IrEnumValue::Int(404)),
                },
            ],
        ))?;

        assert!(rendered.contains("pub fn value(&self) -> i64"), "{rendered}");
        assert!(
            rendered.contains("Self::Ok => 200") && rendered.contains("404 => Some(Self::NotFound)"),
            "{rendered}"
        );
        assert!(
            rendered.contains("let parsed = value.parse::<i64>()"),
            "integer FromStr should parse then use from_value():\n{rendered}"
        );
        Ok(())
    }

    #[test]
    fn serde_value_enum_uses_raw_value_impls_not_serde_derives() -> Result<(), String> {
        let mut enum_decl = base_value_enum(
            "Env",
            IrEnumValueType::String,
            vec![EnumVariant {
                name: "Prod".to_string(),
                fields: VariantFields::Unit,
                raw_value: Some(IrEnumValue::String("production".to_string())),
            }],
        );
        enum_decl.derives.push(SERDE_SERIALIZE_DERIVE.to_string());
        enum_decl.derives.push(SERDE_DESERIALIZE_DERIVE.to_string());

        let rendered = render_enum(&enum_decl)?;

        assert!(
            !rendered.contains("#[derive(Debug, Clone, PartialEq, serde::Serialize")
                && !rendered.contains("#[derive(Debug, Clone, PartialEq, serde::Deserialize"),
            "value enums should not derive serde's variant-name representation:\n{rendered}"
        );
        assert!(rendered.contains("impl serde::Serialize for Env"), "{rendered}");
        assert!(
            rendered.contains("serializer.serialize_str(\"production\")"),
            "{rendered}"
        );
        assert!(
            rendered.contains("impl<'de> serde::Deserialize<'de> for Env"),
            "{rendered}"
        );
        Ok(())
    }

    #[test]
    fn serde_integer_value_enum_uses_raw_value_impls() -> Result<(), String> {
        let mut enum_decl = base_value_enum(
            "HttpStatus",
            IrEnumValueType::Int,
            vec![EnumVariant {
                name: "NotFound".to_string(),
                fields: VariantFields::Unit,
                raw_value: Some(IrEnumValue::Int(404)),
            }],
        );
        enum_decl.derives.push(SERDE_SERIALIZE_DERIVE.to_string());
        enum_decl.derives.push(SERDE_DESERIALIZE_DERIVE.to_string());

        let rendered = render_enum(&enum_decl)?;

        assert!(
            !rendered.contains("#[derive(Debug, Clone, PartialEq, serde::Serialize")
                && !rendered.contains("#[derive(Debug, Clone, PartialEq, serde::Deserialize"),
            "integer value enums should not derive serde's variant-name representation:\n{rendered}"
        );
        assert!(rendered.contains("serializer.serialize_i64(404)"), "{rendered}");
        assert!(
            rendered.contains("let value = <i64 as serde::Deserialize>::deserialize(deserializer)?"),
            "{rendered}"
        );
        Ok(())
    }

    #[test]
    fn value_enum_payload_variant_is_rejected() -> Result<(), String> {
        let result = render_enum(&base_value_enum(
            "Bad",
            IrEnumValueType::Int,
            vec![EnumVariant {
                name: "Payload".to_string(),
                fields: VariantFields::Tuple(vec![IrType::Int]),
                raw_value: Some(IrEnumValue::Int(1)),
            }],
        ));
        let Err(err) = result else {
            return Err("value enum tuple variants must be rejected before Rust emission".to_string());
        };

        assert!(
            err.contains("value enum 'Bad.Payload' cannot carry payload fields"),
            "{err}"
        );
        Ok(())
    }
}
