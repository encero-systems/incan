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

/// Register one route directly when its signature already contains Axum extractors, or synthesize exactly one adapter
/// that groups source-level scalar captures into Axum's ordered `Path` extractor.
fn expand_route(args: RouteArgs, func: ItemFn) -> syn::Result<proc_macro2::TokenStream> {
    let methods: Vec<String> = if args.methods.is_empty() {
        vec!["GET".to_string()]
    } else {
        args.methods.iter().map(|m| m.value().to_ascii_uppercase()).collect()
    };
    let original_path = args.path.value();
    let capture_names = route_capture_names(&original_path);
    let fn_name = &func.sig.ident;
    let wrapper_name = format_ident!("__incan_route_{}", fn_name);

    // ---- Capture planning ----
    let mut scalar_path_params = Vec::new();
    for (input_index, arg) in func.sig.inputs.iter().enumerate() {
        let FnArg::Typed(pat_ty) = arg else {
            return Err(syn::Error::new(arg.span(), "route only supports free functions"));
        };

        let Pat::Ident(pat_ident) = pat_ty.pat.as_ref() else {
            continue;
        };
        if is_axum_extractor(&pat_ty.ty) {
            continue;
        }
        let authored_name = pat_ident.ident.to_string();
        let Some(capture_index) = capture_names.iter().position(|capture| capture == &authored_name) else {
            continue;
        };
        let binding = format_ident!("{}", authored_name);
        scalar_path_params.push((input_index, capture_index, binding, pat_ty.ty.as_ref()));
    }
    scalar_path_params.sort_by_key(|(_, capture_index, _, _)| *capture_index);

    // ---- Handler adaptation ----
    let (route_handler, wrapper) = if scalar_path_params.is_empty() {
        (quote! { #fn_name }, quote! {})
    } else {
        let path_bindings = scalar_path_params
            .iter()
            .map(|(_, _, binding, _)| binding)
            .collect::<Vec<_>>();
        let path_types = scalar_path_params.iter().map(|(_, _, _, ty)| ty).collect::<Vec<_>>();
        let path_param = if path_bindings.len() == 1 {
            let binding = path_bindings[0];
            let ty = path_types[0];
            quote! { axum::extract::Path(#binding): axum::extract::Path<#ty> }
        } else {
            quote! {
                axum::extract::Path((#(#path_bindings),*)):
                    axum::extract::Path<(#(#path_types),*)>
            }
        };

        let mut wrapper_params = vec![path_param];
        let mut call_args = Vec::new();
        for (input_index, arg) in func.sig.inputs.iter().enumerate() {
            let FnArg::Typed(pat_ty) = arg else {
                return Err(syn::Error::new(arg.span(), "route only supports free functions"));
            };
            if let Some((_, _, binding, _)) = scalar_path_params
                .iter()
                .find(|(scalar_input_index, _, _, _)| *scalar_input_index == input_index)
            {
                call_args.push(quote! { #binding });
                continue;
            }

            let binding = format_ident!("__incan_arg_{input_index}");
            let ty = &pat_ty.ty;
            wrapper_params.push(quote! { #binding: #ty });
            call_args.push(quote! { #binding });
        }

        (
            quote! { #wrapper_name },
            quote! {
                async fn #wrapper_name(#(#wrapper_params),*) -> impl axum::response::IntoResponse {
                    #fn_name(#(#call_args),*).await
                }
            },
        )
    };

    // ---- Route registration ----
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
                    #original_path,
                    #method,
                    |router| router.route(#original_path, axum::routing::#router_method(#route_handler)),
                )
            }
        });
    }

    Ok(quote! {
        #func

        #wrapper

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

/// Return whether a handler parameter is already one of Axum's typed extractor wrappers.
fn is_axum_extractor(ty: &Type) -> bool {
    ["Json", "Query", "Path"]
        .iter()
        .any(|wrapper_name| is_generic_wrapper(ty, wrapper_name))
}

/// Extract ordered Axum capture names from an Incan route path without changing its runtime spelling.
fn route_capture_names(path: &str) -> Vec<String> {
    path.split('/')
        .filter_map(|segment| segment.strip_prefix('{')?.strip_suffix('}'))
        .map(|capture| capture.strip_prefix('*').unwrap_or(capture).to_string())
        .collect()
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

    #[test]
    fn expand_route_registers_typed_extractors_without_an_inferred_wrapper() -> Result<(), Box<dyn std::error::Error>> {
        let args: RouteArgs = syn::parse_str("\"/search\", methods=[\"GET\"]")?;
        let func: ItemFn = syn::parse_str(
            r#"
            async fn search(query: Query<Params>) -> Json<Reply> {
                Json(Reply { query: query.q })
            }
        "#,
        )?;

        let expanded = expand_route(args, func)?;
        let expanded_src = expanded.to_string();
        assert!(
            expanded_src.contains("axum :: routing :: get (search)"),
            "typed Axum extractors should register the source handler directly: {expanded_src}"
        );
        assert!(
            !expanded_src.contains("__incan_route_search") && !expanded_src.contains("Query < _ >"),
            "typed extractors must not gain an inferred item-signature wrapper: {expanded_src}"
        );
        Ok(())
    }

    #[test]
    fn expand_route_accepts_an_unused_typed_path_parameter() -> Result<(), Box<dyn std::error::Error>> {
        let args: RouteArgs = syn::parse_str("\"/users/{id}\", methods=[\"GET\"]")?;
        let func: ItemFn = syn::parse_str(
            r#"
            async fn get_user(_: Path<i64>) -> Json<Reply> {
                Json(Reply { id: 1 })
            }
        "#,
        )?;

        let expanded = expand_route(args, func)?;
        let expanded_src = expanded.to_string();
        assert!(
            expanded_src.contains("router . route (\"/users/{id}\" , axum :: routing :: get (get_user))"),
            "typed Path handlers should retain Axum 0.8 captures and wildcard patterns: {expanded_src}"
        );
        assert!(
            !expanded_src.contains("__incan_route_get_user"),
            "a typed Path handler should not be double-wrapped: {expanded_src}"
        );
        Ok(())
    }

    #[test]
    fn expand_route_groups_multiple_scalar_path_captures() -> Result<(), Box<dyn std::error::Error>> {
        let args: RouteArgs = syn::parse_str("\"/posts/{year}/{month}\", methods=[\"GET\"]")?;
        let func: ItemFn = syn::parse_str(
            r#"
            async fn get_posts(year: i64, month: i64) -> i64 {
                year + month
            }
        "#,
        )?;

        let expanded = expand_route(args, func)?;
        let expanded_src = expanded.to_string();
        assert!(
            expanded_src.contains("Path ((year , month)) : axum :: extract :: Path < (i64 , i64) >"),
            "multiple scalar captures should deserialize through one ordered Path tuple: {expanded_src}"
        );
        assert!(
            expanded_src.contains("router . route (\"/posts/{year}/{month}\""),
            "Axum 0.8 capture syntax must remain unchanged: {expanded_src}"
        );
        Ok(())
    }

    #[test]
    fn expand_route_mixes_scalar_paths_with_typed_extractors() -> Result<(), Box<dyn std::error::Error>> {
        let args: RouteArgs = syn::parse_str("\"/users/{id}\", methods=[\"POST\"]")?;
        let func: ItemFn = syn::parse_str(
            r#"
            async fn update_user(id: i64, query: Query<Params>, body: Json<Update>) -> Json<Reply> {
                apply(id, query, body)
            }
        "#,
        )?;

        let expanded = expand_route(args, func)?;
        let expanded_src = expanded.to_string();
        assert!(
            expanded_src.contains("Path (id) : axum :: extract :: Path < i64 >"),
            "the scalar capture should be extracted by the generated adapter: {expanded_src}"
        );
        assert!(
            expanded_src.contains("__incan_arg_1 : Query < Params >")
                && expanded_src.contains("__incan_arg_2 : Json < Update >"),
            "typed extractors should pass through the scalar-path adapter unchanged: {expanded_src}"
        );
        assert!(
            !expanded_src.contains("Query < _ >") && !expanded_src.contains("Json < _ >"),
            "mixed adapters must not emit inferred item-signature types: {expanded_src}"
        );
        Ok(())
    }
}
