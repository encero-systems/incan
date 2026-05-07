//! Procedural macros for the transitional Incan web runtime.
//!
//! This crate is toolchain-locked to `incan_stdlib::web` and compiler-generated Rust. It is not a standalone routing
//! framework API; macro output may change whenever the compiler/runtime contract changes.

use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
use syn::{
    Data, DeriveInput, Expr, FnArg, GenericParam, Generics, Ident, ItemFn, LitStr, Pat, PathArguments, Token, Type,
    TypePath, parse_macro_input,
};

struct RouteArgs {
    path: LitStr,
    methods: Vec<LitStr>,
}

impl Parse for RouteArgs {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let path: LitStr = input.parse()?;
        let mut methods = Vec::new();
        if input.peek(Token![,]) {
            let _ = input.parse::<Token![,]>();
            if input.peek(Ident) {
                let key: Ident = input.parse()?;
                let _ = input.parse::<Token![=]>()?;
                if key == "method" {
                    methods.push(input.parse()?);
                } else if key == "methods" {
                    let list: syn::ExprArray = input.parse()?;
                    for expr in list.elems {
                        match expr {
                            Expr::Lit(expr_lit) => {
                                if let syn::Lit::Str(method) = expr_lit.lit {
                                    methods.push(method);
                                } else {
                                    return Err(syn::Error::new(
                                        expr_lit.span(),
                                        "methods entries must be string literals",
                                    ));
                                }
                            }
                            Expr::Path(path) => {
                                // Be permissive for hand-written Rust usage: methods=[GET, POST]
                                let Some(ident) = path.path.get_ident() else {
                                    return Err(syn::Error::new(
                                        path.span(),
                                        "methods entries must be simple identifiers or string literals",
                                    ));
                                };
                                methods.push(LitStr::new(&ident.to_string(), ident.span()));
                            }
                            other => {
                                return Err(syn::Error::new(other.span(), "methods entries must be string literals"));
                            }
                        }
                    }
                } else {
                    return Err(syn::Error::new(key.span(), "unsupported route argument"));
                }
            }
        }
        Ok(Self { path, methods })
    }
}

#[proc_macro_attribute]
pub fn route(args: TokenStream, input: TokenStream) -> TokenStream {
    let route_args = parse_macro_input!(args as RouteArgs);
    let func = parse_macro_input!(input as ItemFn);
    match expand_route(route_args, func) {
        Ok(ts) => ts.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

fn expand_route(args: RouteArgs, func: ItemFn) -> syn::Result<proc_macro2::TokenStream> {
    let methods: Vec<String> = if args.methods.is_empty() {
        vec!["GET".to_string()]
    } else {
        args.methods.iter().map(|m| m.value().to_ascii_uppercase()).collect()
    };
    let original_path = args.path.value();
    let axum_path = original_path.replace('{', ":").replace('}', "");
    let fn_name = &func.sig.ident;
    let wrapper_name = format_ident!("__incan_route_{}", fn_name);

    let mut wrapper_params = Vec::new();
    let mut call_args = Vec::new();
    for arg in &func.sig.inputs {
        let FnArg::Typed(pat_ty) = arg else {
            return Err(syn::Error::new(arg.span(), "route only supports free functions"));
        };
        let Pat::Ident(pat_ident) = pat_ty.pat.as_ref() else {
            return Err(syn::Error::new(
                pat_ty.pat.span(),
                "unsupported route parameter pattern",
            ));
        };
        let name = &pat_ident.ident;
        let ty = &pat_ty.ty;
        if original_path.contains(&format!("{{{name}}}")) {
            wrapper_params.push(quote! { axum::extract::Path(#name): axum::extract::Path<#ty> });
        } else if is_generic_wrapper(ty, "Json") {
            wrapper_params.push(quote! { axum::extract::Json(#name): axum::extract::Json<_> });
        } else if is_generic_wrapper(ty, "Query") {
            wrapper_params.push(quote! { axum::extract::Query(#name): axum::extract::Query<_> });
        } else {
            wrapper_params.push(quote! { #name: #ty });
        }
        call_args.push(quote! { #name });
    }

    let mut submits = Vec::new();
    for method in methods {
        let router_method = match method.as_str() {
            "GET" => quote! { get },
            "POST" => quote! { post },
            "PUT" => quote! { put },
            "PATCH" => quote! { patch },
            "DELETE" => quote! { delete },
            "HEAD" => quote! { head },
            "OPTIONS" => quote! { options },
            _ => quote! { get },
        };
        submits.push(quote! {
            inventory::submit! {
                incan_stdlib::web::RouteEntry::new(
                    #axum_path,
                    #method,
                    |router| router.route(#axum_path, axum::routing::#router_method(#wrapper_name)),
                )
            }
        });
    }

    Ok(quote! {
        #func

        async fn #wrapper_name(#(#wrapper_params),*) -> impl axum::response::IntoResponse {
            #fn_name(#(#call_args),*).await
        }

        #(#submits)*
    })
}

#[proc_macro_derive(IntoResponse)]
pub fn derive_into_response(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match tuple_struct_inner(&input) {
        Ok(inner_ty) => {
            let name = &input.ident;
            let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();
            quote! {
                impl #impl_generics axum::response::IntoResponse for #name #ty_generics #where_clause
                where
                    #inner_ty: axum::response::IntoResponse
                {
                    fn into_response(self) -> axum::response::Response {
                        self.0.into_response()
                    }
                }
            }
            .into()
        }
        Err(err) => err.to_compile_error().into(),
    }
}

#[proc_macro_derive(FromRequestParts)]
pub fn derive_from_request_parts(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match tuple_struct_inner(&input) {
        Ok(inner_ty) => {
            let name = &input.ident;
            let mut impl_generics: Generics = input.generics.clone();
            impl_generics.params.push(GenericParam::Type(syn::parse_quote! { S }));
            let (impl_g, _ty_g, where_clause) = impl_generics.split_for_impl();
            let (_, original_ty_g, _) = input.generics.split_for_impl();
            let mut where_preds: Punctuated<syn::WherePredicate, Token![,]> = Punctuated::new();
            where_preds.push(syn::parse_quote! { S: Send + Sync });
            where_preds.push(syn::parse_quote! { #inner_ty: axum::extract::FromRequestParts<S> });
            if let Some(clause) = where_clause {
                for pred in clause.predicates.clone() {
                    where_preds.push(pred);
                }
            }
            quote! {
                impl #impl_g axum::extract::FromRequestParts<S> for #name #original_ty_g
                where
                    #where_preds
                {
                    type Rejection = <#inner_ty as axum::extract::FromRequestParts<S>>::Rejection;

                    async fn from_request_parts(
                        parts: &mut http::request::Parts,
                        state: &S,
                    ) -> Result<Self, Self::Rejection> {
                        <#inner_ty as axum::extract::FromRequestParts<S>>::from_request_parts(parts, state)
                            .await
                            .map(Self)
                    }
                }
            }
            .into()
        }
        Err(err) => err.to_compile_error().into(),
    }
}

fn tuple_struct_inner(input: &DeriveInput) -> syn::Result<Type> {
    let Data::Struct(data) = &input.data else {
        return Err(syn::Error::new(
            input.ident.span(),
            "derive is only supported on tuple structs",
        ));
    };
    let syn::Fields::Unnamed(fields) = &data.fields else {
        return Err(syn::Error::new(
            input.ident.span(),
            "derive is only supported on tuple structs with one field",
        ));
    };
    if fields.unnamed.len() != 1 {
        return Err(syn::Error::new(
            input.ident.span(),
            "derive is only supported on tuple structs with one field",
        ));
    }
    match fields.unnamed.first() {
        Some(field) => Ok(field.ty.clone()),
        None => Err(syn::Error::new(
            input.ident.span(),
            "derive is only supported on tuple structs with one field",
        )),
    }
}

fn is_generic_wrapper(ty: &Type, wrapper_name: &str) -> bool {
    let Type::Path(TypePath { path, .. }) = ty else {
        return false;
    };
    let Some(seg) = path.segments.last() else {
        return false;
    };
    if seg.ident != wrapper_name {
        return false;
    }
    matches!(seg.arguments, PathArguments::AngleBracketed(_))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_methods_array_collects_all_methods() -> Result<(), Box<dyn std::error::Error>> {
        let args: RouteArgs = syn::parse_str("\"/items\", methods=[\"GET\", \"DELETE\"]")?;
        assert_eq!(args.path.value(), "/items");
        let collected: Vec<String> = args.methods.iter().map(|m| m.value()).collect();
        assert_eq!(collected, vec!["GET".to_string(), "DELETE".to_string()]);
        Ok(())
    }

    #[test]
    fn expand_route_emits_submit_for_each_method() -> Result<(), Box<dyn std::error::Error>> {
        let args: RouteArgs = syn::parse_str("\"/items/{id}\", methods=[\"GET\", \"DELETE\"]")?;
        let func: ItemFn = syn::parse_str(
            r#"
            async fn get_item(id: i64) -> i64 {
                id
            }
        "#,
        )?;
        let expanded = expand_route(args, func)?;
        let expanded_src = expanded.to_string();
        let submit_count = expanded_src.matches("inventory :: submit !").count();
        assert_eq!(submit_count, 2);
        assert!(expanded_src.contains("\"GET\""));
        assert!(expanded_src.contains("\"DELETE\""));
        Ok(())
    }
}
