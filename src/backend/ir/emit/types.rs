//! Type emission for IR to Rust code generation
//!
//! This module handles emitting Rust type tokens from IR types,
//! as well as visibility, operators, and pattern matching.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use super::super::decl::Visibility;
use super::super::expr::{IrExprKind, MatchArm, Pattern};
use super::super::types::IrType;
use super::{EmitError, IrEmitter};
use incan_core::lang::surface::types::{self as surface_types, SurfaceTypeId};
use incan_core::lang::types::collections::{self, CollectionTypeId};
use incan_core::lang::types::numerics;

impl<'a> IrEmitter<'a> {
    /// Emit the generated Rust type path for an anonymous ordinary union.
    pub(super) fn emit_union_type_path(&self, ty: &IrType) -> TokenStream {
        self.emit_union_type_path_with_qualifier(ty, None)
    }

    /// Emit the generated Rust type path for an anonymous ordinary union with an optional explicit module qualifier.
    pub(super) fn emit_union_type_path_with_qualifier(&self, ty: &IrType, qualifier: Option<&[String]>) -> TokenStream {
        let ty = self.resolve_type_aliases_for_emit(ty);
        let (semantic_ty, external_qualifier) = match &ty {
            IrType::ExternalUnion { library, union } => (union.as_ref(), Some(vec![library.clone()])),
            _ => (&ty, None),
        };
        let union_name = semantic_ty
            .union_type_name()
            .unwrap_or_else(|| super::super::types::IR_UNION_TYPE_NAME.to_string());
        let n = Self::rust_ident(&union_name);
        let qualifier = qualifier.or(external_qualifier.as_deref());
        if let Some(qualifier) = qualifier
            && let Some((first, rest)) = qualifier.split_first()
        {
            let first = if first == "crate" {
                quote! { crate }
            } else {
                let ident = Self::rust_ident(first);
                quote! { #ident }
            };
            let path = rest.iter().fold(first, |acc, segment| {
                let ident = Self::rust_ident(segment);
                quote! { #acc :: #ident }
            });
            return quote! { #path :: #n };
        }
        if self.qualify_union_types_from_crate {
            quote! { crate :: #n }
        } else {
            quote! { #n }
        }
    }

    fn emit_path_ident(path: &str) -> TokenStream {
        if path.contains("::") {
            let segments: Vec<TokenStream> = path
                .split("::")
                .filter(|s| !s.is_empty())
                .map(|seg| {
                    let ident = Self::rust_ident(seg);
                    quote! { #ident }
                })
                .collect();
            let mut iter = segments.into_iter();
            let Some(first) = iter.next() else {
                return quote! { _ };
            };
            iter.fold(first, |acc, seg| quote! { #acc :: #seg })
        } else {
            let ident = Self::rust_ident(path);
            quote! { #ident }
        }
    }

    /// Emit a type as Rust tokens.
    #[allow(clippy::only_used_in_recursion)]
    pub(super) fn emit_type(&self, ty: &IrType) -> TokenStream {
        match ty {
            IrType::Unit => quote! { () },
            IrType::Bool => quote! { bool },
            IrType::Int => quote! { i64 },
            IrType::Float => quote! { f64 },
            IrType::Numeric(id) => {
                let ident = format_ident!("{}", numerics::rust_name(*id));
                quote! { #ident }
            }
            IrType::Decimal { .. } => quote! { incan_stdlib::num::Decimal128 },
            IrType::String => quote! { String },
            IrType::Bytes => quote! { Vec<u8> },
            IrType::StaticStr => quote! { &'static str },
            IrType::StaticBytes => quote! { &'static [u8] },
            IrType::FrozenStr => quote! { incan_stdlib::frozen::FrozenStr },
            IrType::FrozenBytes => quote! { incan_stdlib::frozen::FrozenBytes },
            IrType::StrRef => quote! { &str },
            IrType::List(elem) => {
                let e = self.emit_type(elem);
                quote! { Vec<#e> }
            }
            IrType::Dict(k, v) => {
                let kk = self.emit_type(k);
                let vv = self.emit_type(v);
                quote! { std::collections::HashMap<#kk, #vv> }
            }
            IrType::Set(elem) => {
                let e = self.emit_type(elem);
                quote! { std::collections::HashSet<#e> }
            }
            IrType::Tuple(types) => {
                let ts: Vec<_> = types.iter().map(|t| self.emit_type(t)).collect();
                quote! { (#(#ts),*) }
            }
            IrType::Option(inner) => {
                let i = self.emit_type(inner);
                quote! { Option<#i> }
            }
            IrType::Result(ok, err) => {
                let o = self.emit_type(ok);
                let e = self.emit_type(err);
                quote! { Result<#o, #e> }
            }
            IrType::Struct(name) | IrType::Enum(name) | IrType::Trait(name) => {
                if name == surface_types::as_str(SurfaceTypeId::FieldInfo) {
                    return quote! { incan_stdlib::reflection::FieldInfo };
                }
                if name == surface_types::as_str(SurfaceTypeId::ValidationError) {
                    return quote! { incan_stdlib::validation::ValidationError };
                }
                if *self.qualify_internal_canonical_paths.borrow()
                    && let Some(path) = self.emit_dependency_type_path(name)
                {
                    return path;
                }
                Self::emit_path_ident(name)
            }
            IrType::RustDisplay(display) => display.parse().unwrap_or_else(|_| quote! { _ }),
            IrType::ExternalUnion { .. } => self.emit_union_type_path(ty),
            IrType::NamedGeneric(name, _) if name == super::super::types::IR_UNION_TYPE_NAME => {
                self.emit_union_type_path(ty)
            }
            IrType::NamedGeneric(name, args) => {
                let frozen_name = match collections::from_str(name) {
                    Some(CollectionTypeId::FrozenList) => Some(quote! { incan_stdlib::frozen::FrozenList }),
                    Some(CollectionTypeId::FrozenSet) => Some(quote! { incan_stdlib::frozen::FrozenSet }),
                    Some(CollectionTypeId::FrozenDict) => Some(quote! { incan_stdlib::frozen::FrozenDict }),
                    Some(CollectionTypeId::Generator) => Some(quote! { incan_stdlib::iter::Generator }),
                    _ => None,
                };
                let ts: Vec<_> = args.iter().map(|t| self.emit_type(t)).collect();
                if let Some(n) = frozen_name {
                    quote! { #n < #(#ts),* > }
                } else if *self.qualify_internal_canonical_paths.borrow()
                    && let Some(n) = self.emit_dependency_type_path(name)
                {
                    quote! { #n < #(#ts),* > }
                } else {
                    let n = Self::emit_path_ident(name);
                    quote! { #n < #(#ts),* > }
                }
            }
            IrType::TypeToken(inner) => {
                let inner_ty = self.emit_type(inner);
                quote! { incan_stdlib::reflection::TypeToken<#inner_ty> }
            }
            IrType::ImplTrait(bound) => {
                let bound_tokens = self.emit_trait_bound(bound);
                quote! { impl #bound_tokens }
            }
            IrType::SelfType => {
                quote! { Self }
            }
            IrType::Function { params, ret } => {
                let ps: Vec<_> = params.iter().map(|p| self.emit_type(p)).collect();
                let r = self.emit_type(ret);
                quote! { fn(#(#ps),*) -> #r }
            }
            IrType::Generic(name) => {
                let n = format_ident!("{}", name);
                quote! { #n }
            }
            IrType::Ref(inner) => {
                let i = self.emit_type(inner);
                quote! { &#i }
            }
            IrType::RefMut(inner) => {
                let i = self.emit_type(inner);
                quote! { &mut #i }
            }
            IrType::Unknown => quote! { _ },
        }
    }

    /// Emit the Rust function type for a callable value.
    pub(in crate::backend::ir::emit) fn emit_callable_fn_type(&self, params: &[IrType], ret: &IrType) -> TokenStream {
        let previous = self.qualify_internal_canonical_paths.replace(true);
        let param_tokens = params.iter().map(|param| self.emit_type(param)).collect::<Vec<_>>();
        let ret_tokens = self.emit_type(ret);
        self.qualify_internal_canonical_paths.replace(previous);
        quote! { fn(#(#param_tokens),*) -> #ret_tokens }
    }

    // ========================================================================
    // RFC 023: Type parameter emission with trait bounds
    // ========================================================================

    /// Emit generic type parameter list with trait bounds: `<T: Bound1 + Bound2, E>`.
    ///
    /// Returns empty tokens if there are no type parameters.
    pub(super) fn emit_type_params(&self, type_params: &[super::super::decl::IrTypeParam]) -> TokenStream {
        if type_params.is_empty() {
            return quote! {};
        }

        let params: Vec<TokenStream> = type_params
            .iter()
            .map(|tp| {
                let name = format_ident!("{}", &tp.name);
                if tp.bounds.is_empty() {
                    quote! { #name }
                } else {
                    let bounds: Vec<TokenStream> = tp.bounds.iter().map(|b| self.emit_trait_bound(b)).collect();
                    quote! { #name: #(#bounds)+* }
                }
            })
            .collect();

        quote! { < #(#params),* > }
    }

    /// Emit bare type parameter names without bounds: `<T, E>`.
    ///
    /// Used in type-application positions (return types, `impl Foo<T>`) where Rust does not allow trait bounds — only
    /// declaration positions (`fn foo<T: Clone>`, `impl<T: Clone>`) allow them.
    ///
    /// Returns empty tokens if there are no type parameters.
    pub(super) fn emit_type_params_bare(&self, type_params: &[super::super::decl::IrTypeParam]) -> TokenStream {
        if type_params.is_empty() {
            return quote! {};
        }

        let names: Vec<TokenStream> = type_params
            .iter()
            .map(|tp| {
                let name = format_ident!("{}", &tp.name);
                quote! { #name }
            })
            .collect();

        quote! { < #(#names),* > }
    }

    /// Emit a single trait bound as Rust tokens.
    ///
    /// Handles simple bounds like `PartialEq` and bounds with associated types like `std::ops::Add<Output = T>`.
    fn emit_trait_bound(&self, bound: &super::super::decl::IrTraitBound) -> TokenStream {
        if matches!(bound.origin, super::super::decl::IrTraitBoundOrigin::RustCapability)
            && bound.trait_path == "Static"
        {
            return quote! { 'static };
        }

        // Parse the trait path into segments.
        let segments: Vec<_> = bound
            .trait_path
            .split("::")
            .flat_map(|segment| segment.split('.'))
            .collect();
        let path_tokens: Vec<TokenStream> = segments
            .iter()
            .map(|seg| {
                let ident = Self::rust_ident(seg);
                quote! { #ident }
            })
            .collect();
        let path = super::decls::join_path_tokens(&path_tokens);

        if bound.type_args.is_empty() && bound.assoc_types.is_empty() {
            path
        } else {
            let type_args: Vec<TokenStream> = bound.type_args.iter().map(|t| self.emit_type(t)).collect();
            let assocs: Vec<TokenStream> = bound
                .assoc_types
                .iter()
                .map(|(name, ty)| {
                    let name_ident = format_ident!("{}", name);
                    let ty_tokens = self.emit_type(ty);
                    quote! { #name_ident = #ty_tokens }
                })
                .collect();
            let generic_items: Vec<TokenStream> = type_args.into_iter().chain(assocs).collect();
            quote! { #path < #(#generic_items),* > }
        }
    }

    /// Emit a supertrait reference for a trait definition header (`Bar`, `DataSet<T>`), RFC 042.
    ///
    /// Delegates to [`Self::emit_trait_bound`] so path splitting and generic rendering are not duplicated.
    pub(super) fn emit_supertrait_bound_path(&self, trait_path: &str, type_args: &[IrType]) -> TokenStream {
        let bound = super::super::decl::IrTraitBound::with_type_args(trait_path, type_args.to_vec());
        self.emit_trait_bound(&bound)
    }

    /// Emit visibility modifier.
    pub(super) fn emit_visibility(&self, vis: &Visibility) -> TokenStream {
        match vis {
            Visibility::Private => quote! {},
            Visibility::Public => quote! { pub },
            Visibility::Crate => quote! { pub(crate) },
        }
    }

    /// Emit a pattern for match expressions.
    pub(super) fn emit_pattern(&self, pattern: &Pattern) -> TokenStream {
        match pattern {
            Pattern::Wildcard => quote! { _ },
            Pattern::Var(name) => {
                let n = Self::rust_ident(name);
                quote! { #n }
            }
            Pattern::Literal(lit) => {
                // Pattern literals must be emitted without .to_string() or other conversions
                match &lit.kind {
                    IrExprKind::Unit => quote! { () },
                    IrExprKind::None => quote! { None },
                    IrExprKind::Bool(b) => {
                        if *b {
                            quote! { true }
                        } else {
                            quote! { false }
                        }
                    }
                    IrExprKind::Int(n) => {
                        let lit_tok = if *n >= 0 {
                            proc_macro2::Literal::u64_unsuffixed(*n as u64)
                        } else {
                            proc_macro2::Literal::i64_unsuffixed(*n)
                        };
                        quote! { #lit_tok }
                    }
                    IrExprKind::Float(f) => {
                        let lit_tok = proc_macro2::Literal::f64_unsuffixed(*f);
                        quote! { #lit_tok }
                    }
                    IrExprKind::String(s) => {
                        // String patterns must be &str literals, not String values
                        quote! { #s }
                    }
                    _ => self.emit_expr(lit).unwrap_or(quote! { _ }),
                }
            }
            Pattern::Tuple(pats) => {
                let ps: Vec<_> = pats.iter().map(|p| self.emit_pattern(p)).collect();
                quote! { (#(#ps),*) }
            }
            Pattern::Struct { name, fields } => {
                let n = format_ident!("{}", name);
                let fs: Vec<_> = fields
                    .iter()
                    .map(|(fname, fpat)| {
                        let fn_ident = format_ident!("{}", fname);
                        let fp = self.emit_pattern(fpat);
                        quote! { #fn_ident: #fp }
                    })
                    .collect();
                quote! { #n { #(#fs),* } }
            }
            Pattern::Enum {
                name: _,
                variant,
                fields,
            } => {
                // Handle qualified enum variants like "Shape::Circle"
                let v: TokenStream = if variant.contains("::") {
                    // Parse as a path
                    let segments: Vec<_> = variant.split("::").collect();
                    let idents: Vec<_> = segments.iter().map(|s| format_ident!("{}", s)).collect();
                    if segments
                        .first()
                        .is_some_and(|segment| segment.starts_with("__IncanUnion"))
                    {
                        if self.qualify_union_types_from_crate {
                            quote! { crate :: #(#idents)::* }
                        } else {
                            quote! { #(#idents)::* }
                        }
                    } else {
                        quote! { #(#idents)::* }
                    }
                } else {
                    let v_ident = format_ident!("{}", variant);
                    quote! { #v_ident }
                };
                if fields.is_empty() {
                    quote! { #v }
                } else {
                    let fs: Vec<_> = fields.iter().map(|p| self.emit_pattern(p)).collect();
                    quote! { #v(#(#fs),*) }
                }
            }
            Pattern::Or(pats) => {
                let ps: Vec<_> = pats.iter().map(|p| self.emit_pattern(p)).collect();
                quote! { #(#ps)|* }
            }
        }
    }

    /// Collect string literal patterns from a match pattern tree.
    fn collect_string_literal_patterns<'p>(pattern: &'p Pattern, values: &mut Vec<&'p str>) -> bool {
        match pattern {
            Pattern::Literal(lit) => {
                if let IrExprKind::String(value) = &lit.kind {
                    values.push(value);
                    true
                } else {
                    false
                }
            }
            Pattern::Or(patterns) => patterns
                .iter()
                .all(|pattern| Self::collect_string_literal_patterns(pattern, values)),
            _ => false,
        }
    }

    /// Emit a pattern plus an optional guard required by the scrutinee's Rust representation.
    ///
    /// Incan `str` lowers to Rust `String`. Rust cannot directly match `String` with a string-literal pattern, so
    /// string literal arms become guarded reference patterns while fallback bindings still receive the original
    /// `String` value.
    pub(super) fn emit_pattern_for_scrutinee(
        &self,
        pattern: &Pattern,
        scrutinee_ty: &IrType,
    ) -> (TokenStream, Option<TokenStream>) {
        if matches!(scrutinee_ty, IrType::String) {
            if let Pattern::Literal(lit) = pattern
                && let IrExprKind::String(value) = &lit.kind
            {
                let binding = Self::rust_ident("__incan_match_string_literal");
                return (quote! { ref #binding }, Some(quote! { #binding.as_str() == #value }));
            }
            let mut string_values = Vec::new();
            if matches!(pattern, Pattern::Or(_))
                && Self::collect_string_literal_patterns(pattern, &mut string_values)
                && !string_values.is_empty()
            {
                let binding = Self::rust_ident("__incan_match_string_literal");
                let patterns: Vec<_> = string_values.iter().map(|value| quote! { #value }).collect();
                return (
                    quote! { ref #binding },
                    Some(quote! { matches!(#binding.as_str(), #(#patterns)|*) }),
                );
            }
        }

        (self.emit_pattern(pattern), None)
    }

    /// Emit compiler-introduced match-arm bindings.
    fn emit_match_arm_binding_lets(&self, arm: &MatchArm, for_guard: bool) -> Result<Vec<TokenStream>, EmitError> {
        let mut bindings = Vec::new();
        for binding in &arm.bindings {
            let value_expr = if for_guard {
                let guard_uses_binding = arm
                    .guard
                    .as_ref()
                    .is_some_and(|guard| super::super::scanners::expr_uses_binding_name(guard, &binding.name));
                if !guard_uses_binding {
                    continue;
                }
                binding.guard_value.as_ref().unwrap_or(&binding.value)
            } else {
                &binding.value
            };
            let name = Self::rust_ident(&binding.name);
            let ty = self.emit_type(&binding.ty);
            let value = self.emit_expr(value_expr)?;
            bindings.push(quote! { let #name: #ty = #value; });
        }
        Ok(bindings)
    }

    /// Emit a match arm guard, including compiler-introduced bindings required by narrowed pattern lowering.
    pub(super) fn emit_match_arm_guard(
        &self,
        arm: &MatchArm,
        pattern_guard: Option<TokenStream>,
    ) -> Result<Option<TokenStream>, EmitError> {
        let arm_guard = arm.guard.as_ref().map(|guard| self.emit_expr(guard)).transpose()?;
        let guard = match (pattern_guard, arm_guard) {
            (Some(pattern_guard), Some(arm_guard)) => Some(quote! { (#pattern_guard) && (#arm_guard) }),
            (Some(pattern_guard), None) => Some(pattern_guard),
            (None, Some(arm_guard)) => Some(arm_guard),
            (None, None) => None,
        };
        let Some(guard) = guard else {
            return Ok(None);
        };
        if arm.bindings.is_empty() || arm.guard.is_none() {
            return Ok(Some(guard));
        }
        let bindings = self.emit_match_arm_binding_lets(arm, true)?;
        if bindings.is_empty() {
            return Ok(Some(guard));
        }
        Ok(Some(quote! {{
            #(#bindings)*
            #guard
        }}))
    }

    /// Emit a match arm body, including compiler-introduced bindings required by narrowed pattern lowering.
    pub(super) fn emit_match_arm_body(&self, arm: &MatchArm) -> Result<TokenStream, EmitError> {
        let body = self.emit_expr(&arm.body)?;
        if arm.bindings.is_empty() {
            return Ok(body);
        }

        let bindings = self.emit_match_arm_binding_lets(arm, false)?;

        Ok(quote! {{
            #(#bindings)*
            #body
        }})
    }
}
