//! Struct and enum emission.

use proc_macro2::{Ident, Literal, TokenStream};
use quote::{format_ident, quote};

use incan_core::lang::derives::{self, DeriveId};
use incan_core::lang::surface::constructors::{self, ConstructorId};

use super::super::super::decl::{
    IrEnum, IrEnumValue, IrEnumValueType, IrStruct, IrTypeParam, StructField, VariantFields,
};
use super::super::super::types::IrType;
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
                _ if d == derives::FIELD_INFO_DERIVE_NAME => quote! { incan_derive::FieldInfo },
                _ if d == derives::INCAN_CLASS_DERIVE_NAME => quote! { incan_derive::IncanClass },
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
        let doc_attrs = self.emit_public_rustdoc_attrs(&s.visibility, s.docstring.as_deref());

        let has_serde = s
            .derives
            .iter()
            .any(|d| d == SERDE_SERIALIZE_DERIVE || d == SERDE_DESERIALIZE_DERIVE);

        let is_tuple_struct =
            !s.fields.is_empty() && s.fields.iter().all(|f| f.name.chars().all(|c| c.is_ascii_digit()));

        // RFC 023: emit generic type parameters with trait bounds (declaration) and bare names (type positions).
        let generics = self.emit_type_params(&s.type_params);
        let generics_bare = self.emit_type_params_bare(&s.type_params);
        let reflection_impls = self.emit_struct_reflection_trait_impls(s)?;
        let field_value_reflection_reads_fields = Self::struct_emits_field_value_reflection(s);

        if is_tuple_struct {
            let tuple_fields: Vec<TokenStream> = s
                .fields
                .iter()
                .map(|f| {
                    let fty = self.emit_type(&f.ty);
                    let fvis = self.emit_visibility(&f.visibility);
                    let dead_code_expect = if field_value_reflection_reads_fields {
                        quote! {}
                    } else {
                        self.private_field_dead_code_expect(&s.name, &f.name, &f.visibility)
                    };
                    quote! { #dead_code_expect #fvis #fty }
                })
                .collect();

            // Emit struct definition
            let struct_def = quote! {
                #(#doc_attrs)*
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
                #reflection_impls
            })
        } else {
            let fields: Vec<TokenStream> = s
                .fields
                .iter()
                .map(|f| {
                    let fname = format_ident!("{}", &f.name);
                    let fty = self.emit_type(&f.ty);
                    let fvis = self.emit_visibility(&f.visibility);
                    let dead_code_expect = if field_value_reflection_reads_fields {
                        quote! {}
                    } else {
                        self.private_field_dead_code_expect(&s.name, &f.name, &f.visibility)
                    };
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

            let constructor = if !s.fields.is_empty() && self.should_emit_struct_constructor(s) {
                let mut param_tokens = Vec::with_capacity(s.fields.len());
                let mut field_assigns = Vec::with_capacity(s.fields.len());
                for field in &s.fields {
                    let field_name = format_ident!("{}", &field.name);
                    let field_ty = self.emit_type(&field.ty);
                    if let Some(default) = field.default.as_ref() {
                        let default = if matches!(field.ty, IrType::String)
                            && let crate::backend::ir::expr::IrExprKind::BinOp {
                                op: crate::backend::ir::expr::BinOp::Add,
                                left,
                                right,
                            } = &default.kind
                            && let Some(static_default) = self.try_emit_static_str_add(left, right)?
                        {
                            quote! { #static_default.to_string() }
                        } else {
                            self.emit_expr_for_use(
                                default,
                                crate::backend::ir::ownership::ValueUseSite::StructField {
                                    target_ty: Some(&field.ty),
                                },
                            )?
                        };
                        param_tokens.push(quote! { #field_name: Option<#field_ty> });
                        field_assigns.push(quote! { #field_name: #field_name.unwrap_or_else(|| #default) });
                    } else {
                        param_tokens.push(quote! { #field_name: #field_ty });
                        field_assigns.push(quote! { #field_name });
                    }
                }

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
                #(#doc_attrs)*
                #(#lint_allows)*
                #derive_attr
                #vis struct #name #generics {
                    #(#fields),*
                }

                #constructor
                #reflection_impls
            })
        }
    }

    /// Emit the Rust traits that make compiler-provided reflection available through generic bounds.
    fn emit_struct_reflection_trait_impls(&self, s: &IrStruct) -> Result<TokenStream, EmitError> {
        let name = Self::rust_ident(&s.name);
        let generics = self.emit_type_params(&s.type_params);
        let generics_bare = self.emit_type_params_bare(&s.type_params);
        let has_class_name = s
            .derives
            .iter()
            .any(|derive| derive == derives::INCAN_CLASS_DERIVE_NAME);
        let has_type_class_name = has_class_name || self.newtype_construction.contains_key(&s.name);
        let has_field_metadata = s.derives.iter().any(|derive| derive == derives::FIELD_INFO_DERIVE_NAME);

        let value_class_name_impl = if has_class_name {
            quote! {
                impl #generics incan_stdlib::reflection::HasClassName for #name #generics_bare {
                    fn __class_name__(&self) -> &'static str {
                        <Self as incan_stdlib::reflection::HasTypeClassName>::__class_name__()
                    }
                }
            }
        } else {
            quote! {}
        };
        let type_class_name_impl = if has_type_class_name {
            let class_name = s.name.as_str();
            quote! {
                impl #generics incan_stdlib::reflection::HasTypeClassName for #name #generics_bare {
                    fn __class_name__() -> &'static str {
                        #class_name
                    }
                }
            }
        } else {
            quote! {}
        };

        let field_metadata_impl = if has_field_metadata {
            if let Some((field_count, field_infos)) = self.reflection_field_info_entries(&s.name)? {
                quote! {
                    impl #generics incan_stdlib::reflection::HasFieldMetadata for #name #generics_bare {
                        fn __fields__(&self) -> incan_stdlib::frozen::FrozenList<incan_stdlib::reflection::FieldInfo> {
                            <Self as incan_stdlib::reflection::HasTypeFieldMetadata>::__fields__()
                        }
                    }

                    impl #generics incan_stdlib::reflection::HasTypeFieldMetadata for #name #generics_bare {
                        fn __fields__() -> incan_stdlib::frozen::FrozenList<incan_stdlib::reflection::FieldInfo> {
                            static __INCAN_FIELDS: [incan_stdlib::reflection::FieldInfo; #field_count] = [#(#field_infos),*];
                            incan_stdlib::frozen::FrozenList::new(&__INCAN_FIELDS)
                        }
                    }
                }
            } else {
                quote! {}
            }
        } else {
            quote! {}
        };

        let field_value_reflection_impl = if Self::struct_emits_field_value_reflection(s) {
            let mut value_arms = Vec::new();
            let mut items = Vec::new();
            for field in &s.fields {
                let value = Self::field_reflection_string_expr(field);
                let mut keys = vec![field.name.clone()];
                if let Some(alias) = &field.alias
                    && alias != &field.name
                {
                    keys.push(alias.clone());
                }
                for lookup_key in keys {
                    value_arms.push(quote! { #lookup_key => Some(#value) });
                }

                let field_name = field.name.as_str();
                items.push(quote! { (#field_name.to_string(), #value) });
            }

            quote! {
                impl #generics incan_stdlib::reflection::HasFieldValueReflection for #name #generics_bare {
                    fn __field_value__(&self, name: &str) -> Option<String> {
                        match name {
                            #(#value_arms,)*
                            _ => None,
                        }
                    }

                    fn __field_items__(&self) -> Vec<(String, String)> {
                        vec![#(#items),*]
                    }
                }
            }
        } else {
            quote! {}
        };

        Ok(quote! {
            #value_class_name_impl
            #type_class_name_impl
            #field_metadata_impl
            #field_value_reflection_impl
        })
    }

    /// Return whether this struct can emit the generic value-level field reflection trait.
    fn struct_emits_field_value_reflection(s: &IrStruct) -> bool {
        let is_tuple_struct =
            !s.fields.is_empty() && s.fields.iter().all(|f| f.name.chars().all(|c| c.is_ascii_digit()));
        !is_tuple_struct
            && s.derives.iter().any(|derive| derive == derives::FIELD_INFO_DERIVE_NAME)
            && !s
                .fields
                .iter()
                .any(|field| Self::field_type_mentions_type_param(&field.ty, &s.type_params))
            && s.fields
                .iter()
                .all(|field| Self::field_type_supports_value_reflection(&field.ty))
    }

    /// Return whether a field type mentions one of the owning struct's type parameters.
    fn field_type_mentions_type_param(ty: &IrType, type_params: &[IrTypeParam]) -> bool {
        let is_type_param = |name: &str| type_params.iter().any(|param| param.name == name);
        match ty {
            IrType::Generic(name) | IrType::Struct(name) | IrType::Enum(name) | IrType::Trait(name) => {
                is_type_param(name)
            }
            IrType::NamedGeneric(name, args) => {
                is_type_param(name)
                    || args
                        .iter()
                        .any(|arg| Self::field_type_mentions_type_param(arg, type_params))
            }
            IrType::List(inner)
            | IrType::Set(inner)
            | IrType::Option(inner)
            | IrType::Ref(inner)
            | IrType::RefMut(inner)
            | IrType::TypeToken(inner) => Self::field_type_mentions_type_param(inner, type_params),
            IrType::Dict(key, value) | IrType::Result(key, value) => {
                Self::field_type_mentions_type_param(key, type_params)
                    || Self::field_type_mentions_type_param(value, type_params)
            }
            IrType::Tuple(items) => items
                .iter()
                .any(|item| Self::field_type_mentions_type_param(item, type_params)),
            IrType::Function { params, ret } => {
                params
                    .iter()
                    .any(|param| Self::field_type_mentions_type_param(param, type_params))
                    || Self::field_type_mentions_type_param(ret, type_params)
            }
            IrType::ExternalUnion { union, .. } => Self::field_type_mentions_type_param(union, type_params),
            IrType::ImplTrait(bound) => bound
                .type_args
                .iter()
                .chain(bound.assoc_types.iter().map(|(_, ty)| ty))
                .any(|arg| Self::field_type_mentions_type_param(arg, type_params)),
            _ => false,
        }
    }

    /// Emit the string value used by generic field-value reflection for one concrete field.
    fn field_reflection_string_expr(field: &StructField) -> TokenStream {
        let field_ident = format_ident!("{}", field.name);
        let none = constructors::as_str(ConstructorId::None);
        match &field.ty {
            IrType::Option(inner) if Self::field_type_supports_scalar_value_reflection(inner) => quote! {
                match &self.#field_ident {
                    Some(value) => format!("{}", value),
                    None => #none.to_string(),
                }
            },
            _ => quote! { format!("{}", self.#field_ident) },
        }
    }

    /// Return whether generic field-value reflection can stringify this type without extra Rust trait bounds.
    fn field_type_supports_value_reflection(ty: &IrType) -> bool {
        if Self::field_type_supports_scalar_value_reflection(ty) {
            return true;
        }

        matches!(ty, IrType::Option(inner) if Self::field_type_supports_scalar_value_reflection(inner))
    }

    /// Return whether a scalar field type can be reflected through Rust's built-in Display implementations.
    fn field_type_supports_scalar_value_reflection(ty: &IrType) -> bool {
        matches!(
            ty,
            IrType::Bool
                | IrType::Int
                | IrType::Float
                | IrType::Numeric(_)
                | IrType::Decimal { .. }
                | IrType::String
                | IrType::StaticStr
                | IrType::FrozenStr
                | IrType::StrRef
        )
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
                _ if d == derives::FIELD_INFO_DERIVE_NAME => quote! { incan_derive::FieldInfo },
                _ if d == derives::INCAN_CLASS_DERIVE_NAME => quote! { incan_derive::IncanClass },
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
        let doc_attrs = self.emit_public_rustdoc_attrs(&e.visibility, e.docstring.as_deref());

        // RFC 023: emit generic type parameters with trait bounds (declaration) and bare names (type positions).
        let generics = self.emit_type_params(&e.type_params);
        let generics_bare = self.emit_type_params_bare(&e.type_params);
        let class_name = e.name.as_str();
        let type_class_name_impl = quote! {
            impl #generics incan_stdlib::reflection::HasTypeClassName for #name #generics_bare {
                fn __class_name__() -> &'static str {
                    #class_name
                }
            }
        };
        let value_enum_helpers = self.emit_value_enum_helpers(e, &name, &generics, &generics_bare)?;
        let message_impl = if self.should_emit_enum_message_method(&e.name, &e.visibility) {
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

            quote! {
                impl #generics #name #generics_bare {
                    pub fn message(&self) -> String {
                        match self {
                            #(#variant_match_arms),*
                        }
                    }
                }
            }
        } else {
            quote! {}
        };

        Ok(quote! {
            #(#doc_attrs)*
            #(#lint_allows)*
            #derive_attr
            #vis enum #name #generics {
                #(#variants),*
            }

            #type_class_name_impl
            #message_impl
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
    use std::collections::{HashMap, HashSet};

    use super::*;
    use crate::backend::ir::decl::{EnumVariant, IrEnum, IrEnumValue, IrEnumValueType, IrTypeParam, Visibility};
    use crate::backend::ir::emit::GeneratedUseAnalysis;
    use crate::backend::ir::{FunctionRegistry, IrType};
    use incan_core::lang::surface::constructors::{self, ConstructorId};

    fn render_enum(e: &IrEnum) -> Result<String, String> {
        render_enum_with_used_methods(e, HashSet::new())
    }

    fn render_enum_with_used_methods(e: &IrEnum, used_methods: HashSet<(String, String)>) -> Result<String, String> {
        let registry = FunctionRegistry::new();
        let emitter = IrEmitter::new(&registry);
        emitter.set_generated_use_analysis(GeneratedUseAnalysis {
            used_methods,
            ..GeneratedUseAnalysis::default()
        });
        let tokens = emitter.emit_enum(e).map_err(|err| err.to_string())?;
        let file = syn::parse2::<syn::File>(tokens).map_err(|err| err.to_string())?;
        Ok(prettyplease::unparse(&file))
    }

    fn base_enum(name: &str, visibility: Visibility) -> IrEnum {
        IrEnum {
            name: name.to_string(),
            docstring: None,
            variants: vec![EnumVariant {
                name: "Ready".to_string(),
                fields: VariantFields::Unit,
                raw_value: None,
            }],
            variant_aliases: Vec::new(),
            value_type: None,
            derives: vec![
                derives::as_str(DeriveId::Debug).to_string(),
                derives::as_str(DeriveId::Clone).to_string(),
                derives::as_str(DeriveId::PartialEq).to_string(),
            ],
            visibility,
            type_params: Vec::<IrTypeParam>::new(),
            derive_rust_modules: HashMap::new(),
            lint_allows: Vec::new(),
        }
    }

    fn base_value_enum(name: &str, value_type: IrEnumValueType, variants: Vec<EnumVariant>) -> IrEnum {
        IrEnum {
            name: name.to_string(),
            docstring: None,
            variants,
            variant_aliases: Vec::new(),
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
    fn private_enum_omits_unused_message_helper() -> Result<(), String> {
        let rendered = render_enum(&base_enum("PrivateState", Visibility::Private))?;

        assert!(!rendered.contains("fn message(&self)"), "{rendered}");
        Ok(())
    }

    #[test]
    fn crate_visible_enum_omits_unused_message_helper() -> Result<(), String> {
        let rendered = render_enum(&base_enum("CrateState", Visibility::Crate))?;

        assert!(!rendered.contains("fn message(&self)"), "{rendered}");
        Ok(())
    }

    #[test]
    fn private_enum_keeps_used_message_helper() -> Result<(), String> {
        let rendered = render_enum_with_used_methods(
            &base_enum("PrivateState", Visibility::Private),
            HashSet::from([("PrivateState".to_string(), "message".to_string())]),
        )?;

        assert!(rendered.contains("pub fn message(&self) -> String"), "{rendered}");
        assert!(rendered.contains("Self::Ready => \"Ready\".to_string()"), "{rendered}");
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
