//! Type emission for IR to Rust code generation
//!
//! This module handles emitting Rust type tokens from IR types,
//! as well as visibility, operators, and pattern matching.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use super::super::decl::Visibility;
use super::super::expr::{IrExprKind, Pattern};
use super::super::types::IrType;
use super::IrEmitter;
use incan_core::lang::surface::types::{self as surface_types, SurfaceTypeId};

impl<'a> IrEmitter<'a> {
    /// Emit a type as Rust tokens.
    #[allow(clippy::only_used_in_recursion)]
    pub(super) fn emit_type(&self, ty: &IrType) -> TokenStream {
        match ty {
            IrType::Unit => quote! { () },
            IrType::Bool => quote! { bool },
            IrType::Int => quote! { i64 },
            IrType::Float => quote! { f64 },
            IrType::String => quote! { String },
            IrType::StaticStr => quote! { &'static str },
            IrType::StaticBytes => quote! { &'static [u8] },
            IrType::FrozenStr => quote! { FrozenStr },
            IrType::FrozenBytes => quote! { FrozenBytes },
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
                quote! { HashSet<#e> }
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
                let n = Self::rust_ident(name);
                quote! { #n }
            }
            IrType::NamedGeneric(name, args) => {
                let n = Self::rust_ident(name);
                let ts: Vec<_> = args.iter().map(|t| self.emit_type(t)).collect();
                quote! { #n < #(#ts),* > }
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
        // Parse the trait path into segments.
        let segments: Vec<_> = bound.trait_path.split("::").collect();
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
    pub(super) fn emit_supertrait_bound_path(&self, trait_path: &str, type_args: &[IrType]) -> TokenStream {
        let segments: Vec<_> = trait_path.split("::").collect();
        let path_tokens: Vec<TokenStream> = segments
            .iter()
            .map(|seg| {
                let ident = Self::rust_ident(seg);
                quote! { #ident }
            })
            .collect();
        let base = super::decls::join_path_tokens(&path_tokens);
        if type_args.is_empty() {
            base
        } else {
            let ts: Vec<_> = type_args.iter().map(|t| self.emit_type(t)).collect();
            quote! { #base < #(#ts),* > }
        }
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
                    quote! { #(#idents)::* }
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
}
