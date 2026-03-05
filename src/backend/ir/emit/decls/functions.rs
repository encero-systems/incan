//! Function and method emission.
//!
//! Handles `emit_function`, `emit_extern_function` (RFC 023), `emit_method`, `emit_trait`, and `emit_trait_method`.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use incan_core::lang::conventions;

use super::super::super::decl::IrRustAttrArg;
use super::super::super::types::IrType;
use super::super::{EmitError, IrEmitter};
use super::{ZEN_TEXT, join_path_tokens};

impl<'a> IrEmitter<'a> {
    pub(in crate::backend::ir::emit) fn emit_function(
        &self,
        func: &super::super::super::decl::IrFunction,
    ) -> Result<TokenStream, EmitError> {
        // ---- RFC 023: @rust.extern delegation ----
        if func.is_extern {
            return self.emit_extern_function(func);
        }

        let name = format_ident!("{}", &func.name);
        let is_main = func.name == conventions::ENTRYPOINT_NAME;
        let mutated_params = self.collect_mutated_params(func);

        let vis = if is_main {
            quote! {}
        } else {
            self.emit_visibility(&func.visibility)
        };

        let params: Vec<TokenStream> = func
            .params
            .iter()
            .map(|p| {
                let pname = Self::rust_ident(&p.name);
                let pty = self.emit_type(&p.ty);
                if p.is_self {
                    if matches!(p.mutability, super::super::super::types::Mutability::Mutable) {
                        quote! { &mut self }
                    } else {
                        quote! { &self }
                    }
                } else if mutated_params.contains(&p.name)
                    || matches!(p.mutability, super::super::super::types::Mutability::Mutable)
                {
                    match &p.ty {
                        IrType::Int | IrType::Float | IrType::Bool => quote! { mut #pname: #pty },
                        _ => quote! { #pname: &mut #pty },
                    }
                } else {
                    quote! { #pname: #pty }
                }
            })
            .collect();

        *self.current_function_return_type.borrow_mut() = Some(func.return_type.clone());
        let body_stmts: Vec<TokenStream> = func.body.iter().map(|s| self.emit_stmt(s)).collect::<Result<_, _>>()?;
        *self.current_function_return_type.borrow_mut() = None;

        let async_kw = if func.is_async {
            quote! { async }
        } else {
            quote! {}
        };

        let zen_stmt = if is_main && self.emit_zen_in_main {
            quote! { println!(#ZEN_TEXT); }
        } else {
            quote! {}
        };

        let tokio_main_attr = if is_main && func.is_async && self.needs_tokio {
            quote! { #[incan_stdlib::__private::tokio::main] }
        } else {
            quote! {}
        };
        let rust_attrs = self.emit_rust_attributes(&func.rust_attributes);

        // RFC 023: emit generic type parameters with inferred/explicit trait bounds.
        let generics = self.emit_type_params(&func.type_params);

        let ret_ty_is_unit = matches!(func.return_type, IrType::Unit);
        if is_main || ret_ty_is_unit {
            Ok(quote! {
                #tokio_main_attr
                #(#rust_attrs)*
                #vis #async_kw fn #name #generics (#(#params),*) {
                    #zen_stmt
                    #(#body_stmts)*
                }
            })
        } else {
            let ret_ty = self.emit_type(&func.return_type);
            Ok(quote! {
                #tokio_main_attr
                #(#rust_attrs)*
                #vis #async_kw fn #name #generics (#(#params),*) -> #ret_ty {
                    #(#body_stmts)*
                }
            })
        }
    }

    /// RFC 023: Emit a `@rust.extern` function as a thin wrapper delegating to the Rust backing module.
    ///
    /// Given `rust.module("incan_stdlib::testing")` and `@rust.extern def fail(msg: str) -> None`, emits:
    ///
    /// ```rust,ignore
    /// pub fn fail(msg: String) {
    ///     incan_stdlib::testing::fail(msg)
    /// }
    /// ```
    fn emit_extern_function(&self, func: &super::super::super::decl::IrFunction) -> Result<TokenStream, EmitError> {
        let Some(ref module_path) = self.rust_module_path else {
            return Err(EmitError::Unsupported(format!(
                "@rust.extern function '{}' has no rust.module() path — cannot emit delegation call",
                func.name
            )));
        };

        let name = Self::rust_ident(&func.name);
        let vis = self.emit_visibility(&func.visibility);

        // Build parameter list (same as normal functions, but simpler: no mutation tracking needed).
        let params: Vec<TokenStream> = func
            .params
            .iter()
            .map(|p| {
                let pname = Self::rust_ident(&p.name);
                let pty = self.emit_type(&p.ty);
                quote! { #pname: #pty }
            })
            .collect();

        // Build the fully-qualified call path: `incan_stdlib::testing::fail`.
        let path_segments: Vec<_> = module_path.split("::").collect();
        let mut call_path_tokens: Vec<TokenStream> = path_segments
            .iter()
            .map(|seg| {
                let ident = Self::rust_ident(seg);
                quote! { #ident }
            })
            .collect();
        call_path_tokens.push(quote! { #name });
        let call_path = join_path_tokens(&call_path_tokens);

        // Build argument list (forward all params by name).
        let args: Vec<TokenStream> = func
            .params
            .iter()
            .map(|p| {
                let pname = Self::rust_ident(&p.name);
                quote! { #pname }
            })
            .collect();

        let async_kw = if func.is_async {
            quote! { async }
        } else {
            quote! {}
        };

        // Proc-macro crates expose macros, not callable Rust functions. Keep decorator marker declarations compilable
        // by emitting a panic stub instead of a delegation call.
        if module_path == "incan_web_macros" {
            let generics = self.emit_type_params(&func.type_params);
            let panic_message = format!(
                "decorator marker '{}::{}' cannot be called at runtime",
                module_path, func.name
            );
            let ret_ty_is_unit = matches!(func.return_type, IrType::Unit);
            if ret_ty_is_unit {
                return Ok(quote! {
                    #vis #async_kw fn #name #generics (#(#params),*) {
                        panic!(#panic_message)
                    }
                });
            }

            let ret_ty = self.emit_type(&func.return_type);
            return Ok(quote! {
                #vis #async_kw fn #name #generics (#(#params),*) -> #ret_ty {
                    panic!(#panic_message)
                }
            });
        }

        let await_kw = if func.is_async {
            quote! { .await }
        } else {
            quote! {}
        };

        // RFC 023: emit generic type parameters with trait bounds.
        let generics = self.emit_type_params(&func.type_params);

        // Build turbofish (Rust's name for the ::< > syntax) for the delegation call if there are type params.
        let turbofish = if func.type_params.is_empty() {
            quote! {}
        } else {
            let tp_idents: Vec<TokenStream> = func
                .type_params
                .iter()
                .map(|tp| {
                    let ident = format_ident!("{}", &tp.name);
                    quote! { #ident }
                })
                .collect();
            quote! { :: < #(#tp_idents),* > }
        };

        let ret_ty_is_unit = matches!(func.return_type, IrType::Unit);
        if ret_ty_is_unit {
            Ok(quote! {
                #vis #async_kw fn #name #generics (#(#params),*) {
                    #call_path #turbofish (#(#args),*) #await_kw
                }
            })
        } else {
            let ret_ty = self.emit_type(&func.return_type);
            Ok(quote! {
                #vis #async_kw fn #name #generics (#(#params),*) -> #ret_ty {
                    #call_path #turbofish (#(#args),*) #await_kw
                }
            })
        }
    }

    pub(in crate::backend::ir::emit) fn emit_method(
        &self,
        func: &super::super::super::decl::IrFunction,
    ) -> Result<TokenStream, EmitError> {
        // RFC 023: @rust.extern delegation for methods (used for trait default methods expanded into impl blocks).
        if func.is_extern {
            return self.emit_extern_method(func);
        }

        let name = format_ident!("{}", &func.name);
        let vis = self.emit_visibility(&func.visibility);
        let mutated_params = self.collect_mutated_params(func);

        let params: Vec<TokenStream> = func
            .params
            .iter()
            .map(|p| {
                if p.is_self {
                    match p.mutability {
                        super::super::super::types::Mutability::Mutable => quote! { &mut self },
                        super::super::super::types::Mutability::Immutable => quote! { &self },
                    }
                } else {
                    let pname = format_ident!("{}", &p.name);
                    let pty = self.emit_type(&p.ty);
                    let needs_mut = mutated_params.contains(&p.name)
                        || matches!(p.mutability, super::super::super::types::Mutability::Mutable);
                    if needs_mut {
                        match &p.ty {
                            IrType::Int | IrType::Float | IrType::Bool => quote! { mut #pname: #pty },
                            _ => quote! { #pname: &mut #pty },
                        }
                    } else {
                        quote! { #pname: #pty }
                    }
                }
            })
            .collect();

        let ret = match &func.return_type {
            IrType::Unit => quote! {},
            ty => {
                let t = self.emit_type(ty);
                quote! { -> #t }
            }
        };

        // RFC 023: emit generic type parameters with trait bounds.
        let generics = self.emit_type_params(&func.type_params);

        *self.current_function_return_type.borrow_mut() = Some(func.return_type.clone());
        let body_stmts: Vec<TokenStream> = func.body.iter().map(|s| self.emit_stmt(s)).collect::<Result<_, _>>()?;
        *self.current_function_return_type.borrow_mut() = None;
        let rust_attrs = self.emit_rust_attributes(&func.rust_attributes);

        Ok(quote! {
            #(#rust_attrs)*
            #vis fn #name #generics (#(#params),*) #ret {
                #(#body_stmts)*
            }
        })
    }

    /// RFC 023: Emit a `@rust.extern` method as a thin wrapper delegating to the Rust backing module.
    ///
    /// This is primarily used for trait default methods that are expanded into `impl Trait for Type` blocks during
    /// lowering (RFC 000). Instance methods on classes/models/newtypes are rejected by the typechecker.
    fn emit_extern_method(&self, func: &super::super::super::decl::IrFunction) -> Result<TokenStream, EmitError> {
        let Some(ref module_path) = self.rust_module_path else {
            return Err(EmitError::Unsupported(format!(
                "@rust.extern method '{}' has no rust.module() path — cannot emit delegation call",
                func.name
            )));
        };

        let name = Self::rust_ident(&func.name);
        let vis = self.emit_visibility(&func.visibility);
        let mutated_params = self.collect_mutated_params(func);

        let params: Vec<TokenStream> = func
            .params
            .iter()
            .map(|p| {
                if p.is_self {
                    match p.mutability {
                        super::super::super::types::Mutability::Mutable => quote! { &mut self },
                        super::super::super::types::Mutability::Immutable => quote! { &self },
                    }
                } else {
                    let pname = Self::rust_ident(&p.name);
                    let pty = self.emit_type(&p.ty);
                    let needs_mut = mutated_params.contains(&p.name)
                        || matches!(p.mutability, super::super::super::types::Mutability::Mutable);
                    if needs_mut {
                        match &p.ty {
                            IrType::Int | IrType::Float | IrType::Bool => quote! { mut #pname: #pty },
                            _ => quote! { #pname: &mut #pty },
                        }
                    } else {
                        quote! { #pname: #pty }
                    }
                }
            })
            .collect();

        // Build the fully-qualified call path: `<rust.module path>::<method_name>`.
        let path_segments: Vec<_> = module_path.split("::").collect();
        let mut call_path_tokens: Vec<TokenStream> = path_segments
            .iter()
            .map(|seg| {
                let ident = Self::rust_ident(seg);
                quote! { #ident }
            })
            .collect();
        call_path_tokens.push(quote! { #name });
        let call_path = join_path_tokens(&call_path_tokens);

        // Forward all params, including `self`.
        let args: Vec<TokenStream> = func
            .params
            .iter()
            .map(|p| {
                if p.is_self {
                    quote! { self }
                } else {
                    let pname = Self::rust_ident(&p.name);
                    quote! { #pname }
                }
            })
            .collect();

        let async_kw = if func.is_async {
            quote! { async }
        } else {
            quote! {}
        };
        let await_kw = if func.is_async {
            quote! { .await }
        } else {
            quote! {}
        };

        // RFC 023: emit generic type parameters with trait bounds.
        let generics = self.emit_type_params(&func.type_params);
        let turbofish = if func.type_params.is_empty() {
            quote! {}
        } else {
            let tp_idents: Vec<TokenStream> = func
                .type_params
                .iter()
                .map(|tp| {
                    let ident = format_ident!("{}", &tp.name);
                    quote! { #ident }
                })
                .collect();
            quote! { :: < #(#tp_idents),* > }
        };

        let ret_ty_is_unit = matches!(func.return_type, IrType::Unit);
        if ret_ty_is_unit {
            Ok(quote! {
                #vis #async_kw fn #name #generics (#(#params),*) {
                    #call_path #turbofish (#(#args),*) #await_kw
                }
            })
        } else {
            let ret_ty = self.emit_type(&func.return_type);
            Ok(quote! {
                #vis #async_kw fn #name #generics (#(#params),*) -> #ret_ty {
                    #call_path #turbofish (#(#args),*) #await_kw
                }
            })
        }
    }

    pub(in crate::backend::ir::emit) fn emit_trait(
        &self,
        trait_decl: &super::super::super::decl::IrTrait,
    ) -> Result<TokenStream, EmitError> {
        let name = format_ident!("{}", &trait_decl.name);
        let methods: Vec<TokenStream> = trait_decl
            .methods
            .iter()
            .map(|m| self.emit_trait_method(m))
            .collect::<Result<_, _>>()?;

        Ok(quote! {
            pub trait #name {
                #(#methods)*
            }
        })
    }

    pub(in crate::backend::ir::emit) fn emit_trait_method(
        &self,
        func: &super::super::super::decl::IrFunction,
    ) -> Result<TokenStream, EmitError> {
        let name = format_ident!("{}", &func.name);

        let params: Vec<TokenStream> = func
            .params
            .iter()
            .map(|p| {
                if p.is_self {
                    match p.mutability {
                        super::super::super::types::Mutability::Mutable => quote! { &mut self },
                        super::super::super::types::Mutability::Immutable => quote! { &self },
                    }
                } else {
                    let pname = format_ident!("{}", &p.name);
                    let pty = self.emit_type(&p.ty);
                    quote! { #pname: #pty }
                }
            })
            .collect();

        let ret = match &func.return_type {
            IrType::Unit => quote! {},
            ty => {
                let t = self.emit_type(ty);
                quote! { -> #t }
            }
        };

        // RFC 023: emit generic type parameters with trait bounds.
        let generics = self.emit_type_params(&func.type_params);

        if func.body.is_empty() {
            Ok(quote! {
                fn #name #generics (#(#params),*) #ret;
            })
        } else {
            *self.current_function_return_type.borrow_mut() = Some(func.return_type.clone());
            let body_stmts: Vec<TokenStream> = func.body.iter().map(|s| self.emit_stmt(s)).collect::<Result<_, _>>()?;
            *self.current_function_return_type.borrow_mut() = None;

            Ok(quote! {
                fn #name #generics (#(#params),*) #ret {
                    #(#body_stmts)*
                }
            })
        }
    }

    /// Emit `IrRustAttribute`s as Rust `#[module::path::name(args)]` attribute tokens.
    ///
    /// Shared between `emit_function` and `emit_method` to avoid duplicating the attribute rendering logic.
    fn emit_rust_attributes(&self, attributes: &[super::super::super::decl::IrRustAttribute]) -> Vec<TokenStream> {
        attributes
            .iter()
            .map(|a| {
                let mut path_tokens: Vec<TokenStream> = a
                    .module_path
                    .split("::")
                    .map(Self::rust_ident)
                    .map(|ident| quote! { #ident })
                    .collect::<Vec<_>>();
                let name = Self::rust_ident(&a.name);
                path_tokens.push(quote! { #name });
                let full_path = join_path_tokens(&path_tokens);
                let args = a.args.iter().map(|arg| match arg {
                    IrRustAttrArg::Positional(value) => {
                        let tokens: TokenStream = value.parse().unwrap_or_default();
                        quote! { #tokens }
                    }
                    IrRustAttrArg::Named { name, value } => {
                        let n = Self::rust_ident(name);
                        let tokens: TokenStream = value.parse().unwrap_or_default();
                        quote! { #n = #tokens }
                    }
                });
                quote! { #[#full_path(#(#args),*)] }
            })
            .collect()
    }
}
