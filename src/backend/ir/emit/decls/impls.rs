//! Impl block emission.
//!
//! Handles `emit_impl` (including `@derive(Validate)`, trait impls, and `__fields__` reflection).

use proc_macro2::{Literal, TokenStream};
use quote::{format_ident, quote};
use std::collections::HashSet;

use incan_core::lang::conventions;
use incan_core::lang::derives::{self, DeriveId};
use incan_core::lang::magic_methods;
use incan_core::lang::traits::{self as core_traits, TraitId};

use super::super::super::types::{IR_UNION_TYPE_NAME, IrType};
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
        let mut borrowed_observer_methods = Vec::new();
        let mut trait_impls = Vec::new();

        for method in &impl_block.methods {
            if self.needs_result_observer_callable_helper(&impl_block.target_type)
                && let Some(helper) = self.emit_result_observer_borrowed_method(method)?
                && self.claim_result_observer_callable_helper(&impl_block.target_type)
            {
                borrowed_observer_methods.push(helper);
            }

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
            let associated_types: Vec<TokenStream> = impl_block
                .associated_types
                .iter()
                .map(|associated_type| {
                    let name = format_ident!("{}", associated_type.name);
                    let ty = self.emit_type(&associated_type.ty);
                    quote! { type #name = #ty; }
                })
                .collect();
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
                                | magic_methods::MagicMethodId::FieldValue
                                | magic_methods::MagicMethodId::FieldItems
                        )
                    )
                })
                .map(|m| self.emit_trait_method(m))
                .collect::<Result<_, _>>()?;
            if incan_core::lang::stdlib::is_stdlib_json_serialize_trait_name(trait_name)
                && !impl_block.methods.iter().any(|method| method.name == "to_json")
            {
                trait_methods.push(quote! {
                    fn to_json(&self) -> String {
                        incan_stdlib::json::__private::stringify_or_raise(self, stringify!(#target_type))
                    }
                });
            }
            if incan_core::lang::stdlib::is_stdlib_json_deserialize_trait_name(trait_name)
                && !impl_block.methods.iter().any(|method| method.name == "from_json")
            {
                trait_methods.push(quote! {
                    fn from_json(json_str: String) -> Result<Self, String> {
                        incan_stdlib::json::__private::parse_or_error(&json_str)
                    }
                });
            }
            if Self::is_convert_from_trait_name(trait_name)
                && !impl_block.methods.iter().any(|method| method.name == "from")
                && let Some(source_ty) = impl_block.trait_type_args.first()
            {
                let source_ty = self.emit_type(source_ty);
                trait_methods.push(quote! {
                    fn from(value: #source_ty) -> Self {
                        #target_type::from(value)
                    }
                });
            }
            if Self::is_convert_try_from_trait_name(trait_name)
                && !impl_block.methods.iter().any(|method| method.name == "try_from")
                && let Some(source_ty) = impl_block.trait_type_args.first()
            {
                let source_ty = self.emit_type(source_ty);
                trait_methods.push(quote! {
                    fn try_from(value: #source_ty) -> Result<Self, String> {
                        #target_type::try_from(value)
                    }
                });
            }
            let trait_tokens = self.emit_supertrait_bound_path(trait_name, &impl_block.trait_type_args);
            quote! {
                impl #generics #trait_tokens for #target_type #generics_bare {
                    #(#associated_types)*
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

        let borrowed_observer_impl = if borrowed_observer_methods.is_empty() {
            quote! {}
        } else {
            quote! {
                impl #generics #target_type #generics_bare {
                    #(#borrowed_observer_methods)*
                }
            }
        };

        Ok(quote! {
            #main_impl
            #borrowed_observer_impl
            #(#trait_impls)*
        })
    }

    /// Return the final path segment of a trait name.
    fn trait_short_name(trait_name: &str) -> &str {
        trait_name
            .rsplit(['.', ':'])
            .find(|segment| !segment.is_empty())
            .unwrap_or(trait_name)
    }

    /// Resolve a trait impl target through the builtin trait registry.
    fn builtin_trait_id(trait_name: &str) -> Option<TraitId> {
        core_traits::from_str(trait_name).or_else(|| core_traits::from_str(Self::trait_short_name(trait_name)))
    }

    /// Return whether a trait impl target names the stdlib conversion `From` trait.
    fn is_convert_from_trait_name(trait_name: &str) -> bool {
        Self::builtin_trait_id(trait_name) == Some(TraitId::From)
    }

    /// Return whether a trait impl target names the stdlib conversion `TryFrom` trait.
    fn is_convert_try_from_trait_name(trait_name: &str) -> bool {
        Self::builtin_trait_id(trait_name) == Some(TraitId::TryFrom)
    }

    /// Emit compiler-generated field overlay methods for a struct, independent of source impl blocks.
    pub(in crate::backend::ir::emit) fn emit_field_overlay_methods_for_struct(
        &self,
        strukt: &super::super::super::decl::IrStruct,
        explicit_method_names: &HashSet<String>,
    ) -> Result<Option<TokenStream>, EmitError> {
        let field_value_name = magic_methods::as_str(magic_methods::MagicMethodId::FieldValue);
        let field_items_name = magic_methods::as_str(magic_methods::MagicMethodId::FieldItems);
        let mut methods = Vec::new();
        let used_methods = &self.generated_use_analysis.borrow().used_methods;

        if !explicit_method_names.contains(field_value_name)
            && used_methods.contains(&(strukt.name.clone(), field_value_name.to_string()))
            && let Some(field_value_method) = self.emit_field_value_method(&strukt.name)?
        {
            methods.push(field_value_method);
        }

        if !explicit_method_names.contains(field_items_name)
            && used_methods.contains(&(strukt.name.clone(), field_items_name.to_string()))
            && let Some(field_items_method) = self.emit_field_items_method(&strukt.name)?
        {
            methods.push(field_items_method);
        }

        if methods.is_empty() {
            return Ok(None);
        }

        let target_type = Self::rust_ident(&strukt.name);
        let generics = self.emit_type_params(&strukt.type_params);
        let generics_bare = self.emit_type_params_bare(&strukt.type_params);
        Ok(Some(quote! {
            impl #generics #target_type #generics_bare {
                #(#methods)*
            }
        }))
    }

    pub(in crate::backend::ir::emit) fn reflection_field_info_entries(
        &self,
        struct_name: &str,
    ) -> Result<Option<(Literal, Vec<TokenStream>)>, EmitError> {
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
        Ok(Some((field_count, field_infos)))
    }

    /// Emit the generated `__fields__` reflection method for a struct when field metadata is available.
    fn emit_fields_method(&self, struct_name: &str) -> Result<Option<TokenStream>, EmitError> {
        let Some((field_count, field_infos)) = self.reflection_field_info_entries(struct_name)? else {
            return Ok(None);
        };
        Ok(Some(quote! {
            /// Returns field metadata for this type.
            pub fn __fields__(&self) -> incan_stdlib::frozen::FrozenList<incan_stdlib::reflection::FieldInfo> {
                static __INCAN_FIELDS: [incan_stdlib::reflection::FieldInfo; #field_count] = [#(#field_infos),*];
                incan_stdlib::frozen::FrozenList::new(&__INCAN_FIELDS)
            }
        }))
    }

    /// Resolve the Rust value type shared by generated field overlay methods for a lowered struct.
    ///
    /// The generated `__field_value__()` and `__field_items__()` methods expose a single value slot type. Homogeneous
    /// fields use their common type directly; heterogeneous concrete fields use an anonymous union. Generic field
    /// shapes are skipped because anonymous union definitions are monomorphic today.
    fn field_overlay_value_type_for_struct(&self, struct_name: &str) -> Result<Option<IrType>, EmitError> {
        let Some(field_names) = self.struct_field_names.get(struct_name) else {
            return Ok(None);
        };
        let mut value_types = Vec::new();
        for field_name in field_names {
            let key = (struct_name.to_string(), field_name.clone());
            let ty = self.struct_field_types.get(&key).ok_or_else(|| {
                EmitError::Unsupported(format!(
                    "missing field type metadata for '{}.{}'",
                    struct_name, field_name
                ))
            })?;
            if ty.contains_generic_parameter() {
                return Ok(None);
            }
            value_types.push(ty.clone());
        }
        if value_types.is_empty() {
            return Ok(Some(IrType::Unit));
        }
        value_types.sort_by_key(IrType::rust_name);
        value_types.dedup();
        if value_types.len() == 1 {
            Ok(value_types.pop())
        } else {
            Ok(Some(IrType::NamedGeneric(IR_UNION_TYPE_NAME.to_string(), value_types)))
        }
    }

    /// Emit the Rust expression that converts one struct field into the overlay value slot type.
    ///
    /// Heterogeneous overlays wrap cloned field values in the generated union variant that corresponds to the field's
    /// concrete type. Homogeneous overlays can clone the field value directly.
    fn field_overlay_value_expr(&self, value_ty: &IrType, field_ty: &IrType, field_name: &str) -> TokenStream {
        let field_ident = format_ident!("{}", field_name);
        if value_ty.is_union()
            && let Some(variant_index) = value_ty.union_variant_index_for_member(field_ty)
        {
            let variant_ident = format_ident!("{}", IrType::union_variant_name(variant_index));
            let union_path = self.emit_union_type_path(value_ty);
            return quote! { #union_path :: #variant_ident(self.#field_ident.clone()) };
        }
        quote! { self.#field_ident.clone() }
    }

    /// Emit the compiler-provided `__field_value__` method for a concrete model or class struct.
    ///
    /// The method accepts canonical field names and model aliases, returning `None` for unknown names. It is omitted
    /// when field metadata is missing or when the field value shape cannot be represented as a concrete Rust type.
    fn emit_field_value_method(&self, struct_name: &str) -> Result<Option<TokenStream>, EmitError> {
        let Some(field_names) = self.struct_field_names.get(struct_name) else {
            return Ok(None);
        };
        let Some(value_ty) = self.field_overlay_value_type_for_struct(struct_name)? else {
            return Ok(None);
        };
        let value_ty_tokens = self.emit_type(&value_ty);
        let mut arms = Vec::new();
        for field_name in field_names {
            let key = (struct_name.to_string(), field_name.clone());
            let field_ty = self.struct_field_types.get(&key).ok_or_else(|| {
                EmitError::Unsupported(format!(
                    "missing field type metadata for '{}.{}'",
                    struct_name, field_name
                ))
            })?;
            let value = self.field_overlay_value_expr(&value_ty, field_ty, field_name);
            let mut keys = vec![field_name.clone()];
            if let Some(Some(alias)) = self.struct_field_aliases.get(&key)
                && alias != field_name
            {
                keys.push(alias.clone());
            }
            for lookup_key in keys {
                arms.push(quote! { #lookup_key => Some(#value) });
            }
        }

        Ok(Some(quote! {
            /// Return a public field value by canonical field name or model alias.
            pub fn __field_value__(&self, name: String) -> Option<#value_ty_tokens> {
                match name.as_str() {
                    #(#arms,)*
                    _ => None,
                }
            }
        }))
    }

    /// Emit the compiler-provided `__field_items__` method for a concrete model or class struct.
    ///
    /// The returned vector preserves lowered field order, including inherited class fields that lowering already
    /// prepended before child fields.
    fn emit_field_items_method(&self, struct_name: &str) -> Result<Option<TokenStream>, EmitError> {
        let Some(field_names) = self.struct_field_names.get(struct_name) else {
            return Ok(None);
        };
        let Some(value_ty) = self.field_overlay_value_type_for_struct(struct_name)? else {
            return Ok(None);
        };
        let value_ty_tokens = self.emit_type(&value_ty);
        let mut items = Vec::new();
        for field_name in field_names {
            let key = (struct_name.to_string(), field_name.clone());
            let field_ty = self.struct_field_types.get(&key).ok_or_else(|| {
                EmitError::Unsupported(format!(
                    "missing field type metadata for '{}.{}'",
                    struct_name, field_name
                ))
            })?;
            let value = self.field_overlay_value_expr(&value_ty, field_ty, field_name);
            items.push(quote! { (#field_name.to_string(), #value) });
        }

        Ok(Some(quote! {
            /// Return public field name/value pairs in declaration order.
            pub fn __field_items__(&self) -> Vec<(String, #value_ty_tokens)> {
                vec![#(#items),*]
            }
        }))
    }
}
