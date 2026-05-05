//! Function and method emission.
//!
//! Handles `emit_function`, `emit_extern_function` (RFC 023), `emit_method`, `emit_trait`, and `emit_trait_method`.

use std::collections::HashSet;

use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use incan_core::lang::conventions;

use super::super::super::decl::{IrRustAttrArg, IrRustLintAllow};
use super::super::super::expr::{IrCallArg, IrDictEntry, IrExprKind, IrListEntry, MatchArm, Pattern};
use super::super::super::stmt::{AssignTarget, IrStmt, IrStmtKind};
use super::super::super::types::IrType;
use super::super::{EmitError, IrEmitter};
use super::{ZEN_TEXT, join_path_tokens};

impl<'a> IrEmitter<'a> {
    /// Rust trait methods that return `Self` from an associated function position need `where Self: Sized`.
    ///
    /// Walk the emitted return type recursively so wrappers like `Result<Self, E>` or function types preserve the same
    /// constraint.
    fn trait_method_return_mentions_self(ty: &IrType) -> bool {
        match ty {
            IrType::SelfType => true,
            IrType::List(inner)
            | IrType::Set(inner)
            | IrType::Option(inner)
            | IrType::Ref(inner)
            | IrType::RefMut(inner) => Self::trait_method_return_mentions_self(inner),
            IrType::Dict(key, value) | IrType::Result(key, value) => {
                Self::trait_method_return_mentions_self(key) || Self::trait_method_return_mentions_self(value)
            }
            IrType::Tuple(items) | IrType::NamedGeneric(_, items) => {
                items.iter().any(Self::trait_method_return_mentions_self)
            }
            IrType::Function { params, ret } => {
                params.iter().any(Self::trait_method_return_mentions_self)
                    || Self::trait_method_return_mentions_self(ret)
            }
            IrType::ImplTrait(bound) => bound.type_args.iter().any(Self::trait_method_return_mentions_self),
            _ => false,
        }
    }

    /// Emit a top-level generated Rust function, including entrypoint handling and scoped lint metadata.
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
        let used_names = Self::collect_function_used_names(func);

        let vis = if is_main {
            quote! {}
        } else {
            self.emit_visibility(&func.visibility)
        };

        let params: Vec<TokenStream> = func
            .params
            .iter()
            .map(|p| {
                let param_is_used = used_names.contains(&p.name);
                let pname = Self::emit_param_name(&p.name, &used_names);
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
                    if !param_is_used {
                        match &p.ty {
                            IrType::Int | IrType::Float | IrType::Bool => quote! { _: #pty },
                            _ => quote! { _: &mut #pty },
                        }
                    } else {
                        match &p.ty {
                            IrType::Int | IrType::Float | IrType::Bool => quote! { mut #pname: #pty },
                            _ => quote! { #pname: &mut #pty },
                        }
                    }
                } else {
                    quote! { #pname: #pty }
                }
            })
            .collect();

        *self.current_function_return_type.borrow_mut() = Some(func.return_type.clone());
        let body_stmts = self.emit_stmts(&func.body)?;
        *self.current_function_return_type.borrow_mut() = None;

        let async_kw = if func.is_async {
            quote! { async }
        } else {
            quote! {}
        };
        let static_init_stmt = self.emit_module_static_init_call();

        let zen_stmt = if is_main && self.emit_zen_in_main {
            quote! { println!(#ZEN_TEXT); }
        } else {
            quote! {}
        };
        // Generated entrypoints install a minimal panic hook so runtime helper panics surface only the canonical
        // payload, not Rust's default `thread 'main' panicked at ...` wrapper.
        let panic_hook_stmt = if is_main {
            quote! {
                std::panic::set_hook(std::boxed::Box::new(|panic_info| {
                    if let Some(message) = panic_info.payload().downcast_ref::<&str>() {
                        eprintln!("{message}");
                    } else if let Some(message) = panic_info.payload().downcast_ref::<String>() {
                        eprintln!("{message}");
                    } else {
                        eprintln!("generated program panicked");
                    }
                }));
            }
        } else {
            quote! {}
        };

        let lint_allows = self.emit_rust_lint_allows(&func.lint_allows);
        let rust_attrs = self.emit_rust_attributes(&func.rust_attributes);

        // RFC 023: emit generic type parameters with inferred/explicit trait bounds.
        let generics = self.emit_type_params(&func.type_params);

        if is_main && func.is_async {
            return Ok(quote! {
                #(#lint_allows)*
                #(#rust_attrs)*
                #vis fn #name #generics (#(#params),*) {
                    #static_init_stmt
                    #panic_hook_stmt
                    #zen_stmt
                    if let Err(error) = incan_stdlib::r#async::runtime::block_on(async move {
                        #(#body_stmts)*
                    }) {
                        eprintln!("{error}");
                        std::process::exit(1);
                    }
                }
            });
        }

        let ret_ty_is_unit = matches!(func.return_type, IrType::Unit);
        if is_main || ret_ty_is_unit {
            Ok(quote! {
                #(#lint_allows)*
                #(#rust_attrs)*
                #vis #async_kw fn #name #generics (#(#params),*) {
                    #static_init_stmt
                    #panic_hook_stmt
                    #zen_stmt
                    #(#body_stmts)*
                }
            })
        } else {
            let ret_ty = self.emit_type(&func.return_type);
            Ok(quote! {
                #(#lint_allows)*
                #(#rust_attrs)*
                #vis #async_kw fn #name #generics (#(#params),*) -> #ret_ty {
                    #static_init_stmt
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
        let static_init_stmt = self.emit_module_static_init_call();
        let lint_allows = self.emit_rust_lint_allows(&func.lint_allows);

        // Proc-macro crates expose macros, not callable Rust functions. Keep these decorator placeholders compilable,
        // but route runtime misuse through a named internal stdlib helper instead of emitting an open-coded `panic!`
        // stub.
        if module_path == "incan_web_macros" {
            let generics = self.emit_type_params(&func.type_params);
            let panic_message = format!(
                "decorator marker '{}::{}' cannot be called at runtime",
                module_path, func.name
            );
            let ret_ty_is_unit = matches!(func.return_type, IrType::Unit);
            if ret_ty_is_unit {
                return Ok(quote! {
                    #(#lint_allows)*
                    #vis #async_kw fn #name #generics (#(#params),*) {
                        incan_stdlib::errors::__private::raise_runtime_misuse(#panic_message)
                    }
                });
            }

            let ret_ty = self.emit_type(&func.return_type);
            return Ok(quote! {
                #(#lint_allows)*
                #vis #async_kw fn #name #generics (#(#params),*) -> #ret_ty {
                    incan_stdlib::errors::__private::raise_runtime_misuse(#panic_message)
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
                #(#lint_allows)*
                #vis #async_kw fn #name #generics (#(#params),*) {
                    #static_init_stmt
                    #call_path #turbofish (#(#args),*) #await_kw
                }
            })
        } else {
            let ret_ty = self.emit_type(&func.return_type);
            Ok(quote! {
                #(#lint_allows)*
                #vis #async_kw fn #name #generics (#(#params),*) -> #ret_ty {
                    #static_init_stmt
                    #call_path #turbofish (#(#args),*) #await_kw
                }
            })
        }
    }

    /// Emit an inherent method body for an impl block, preserving generated lint and Rust attribute metadata.
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
        let used_names = Self::collect_function_used_names(func);

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
                    let param_is_used = used_names.contains(&p.name);
                    let pname = Self::emit_param_name(&p.name, &used_names);
                    let pty = self.emit_type(&p.ty);
                    let needs_mut = mutated_params.contains(&p.name)
                        || matches!(p.mutability, super::super::super::types::Mutability::Mutable);
                    if needs_mut {
                        if !param_is_used {
                            match &p.ty {
                                IrType::Int | IrType::Float | IrType::Bool => quote! { _: #pty },
                                _ => quote! { _: &mut #pty },
                            }
                        } else {
                            match &p.ty {
                                IrType::Int | IrType::Float | IrType::Bool => quote! { mut #pname: #pty },
                                _ => quote! { #pname: &mut #pty },
                            }
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
        let async_kw = if func.is_async {
            quote! { async }
        } else {
            quote! {}
        };
        let static_init_stmt = self.emit_module_static_init_call();

        *self.current_function_return_type.borrow_mut() = Some(func.return_type.clone());
        let body_stmts = self.emit_stmts(&func.body)?;
        *self.current_function_return_type.borrow_mut() = None;
        let lint_allows = self.emit_rust_lint_allows(&func.lint_allows);
        let rust_attrs = self.emit_rust_attributes(&func.rust_attributes);

        Ok(quote! {
            #(#lint_allows)*
            #(#rust_attrs)*
            #vis #async_kw fn #name #generics (#(#params),*) #ret {
                #static_init_stmt
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
        let static_init_stmt = self.emit_module_static_init_call();
        let await_kw = if func.is_async {
            quote! { .await }
        } else {
            quote! {}
        };
        let lint_allows = self.emit_rust_lint_allows(&func.lint_allows);

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
                #(#lint_allows)*
                #vis #async_kw fn #name #generics (#(#params),*) {
                    #static_init_stmt
                    #call_path #turbofish (#(#args),*) #await_kw
                }
            })
        } else {
            let ret_ty = self.emit_type(&func.return_type);
            Ok(quote! {
                #(#lint_allows)*
                #vis #async_kw fn #name #generics (#(#params),*) -> #ret_ty {
                    #static_init_stmt
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

        // RFC 023 / RFC 042: trait-level generics and direct supertrait bounds.
        let generics = self.emit_type_params(&trait_decl.type_params);
        let supertrait_colon: TokenStream = if trait_decl.supertraits.is_empty() {
            quote! {}
        } else {
            let bound_tokens: Vec<TokenStream> = trait_decl
                .supertraits
                .iter()
                .map(|(path, args)| self.emit_supertrait_bound_path(path, args))
                .collect();
            let first = bound_tokens.first().cloned().unwrap_or_else(|| quote! {});
            let rest = bound_tokens.iter().skip(1).map(|b| quote! { + #b });
            quote! { : #first #(#rest)* }
        };

        // Note: trait items are emitted as `pub trait` regardless of Incan visibility so generated single-file crates
        // keep stdlib and user traits addressable at crate root (matches pre–RFC-042 emission).
        Ok(quote! {
            pub trait #name #generics #supertrait_colon {
                #(#methods)*
            }
        })
    }

    /// Emit a trait method signature or default body with any required `Self: Sized` bound.
    pub(in crate::backend::ir::emit) fn emit_trait_method(
        &self,
        func: &super::super::super::decl::IrFunction,
    ) -> Result<TokenStream, EmitError> {
        let name = format_ident!("{}", &func.name);
        let used_names = Self::collect_function_used_names(func);

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
                    let pname = if func.body.is_empty() {
                        let ident = Self::rust_ident(&p.name);
                        quote! { #ident }
                    } else {
                        Self::emit_param_name(&p.name, &used_names)
                    };
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
        let has_self_receiver = func.params.iter().any(|param| param.is_self);
        let sized_where = if !has_self_receiver && Self::trait_method_return_mentions_self(&func.return_type) {
            quote! { where Self: Sized }
        } else {
            quote! {}
        };

        if func.body.is_empty() {
            let lint_allows = self.emit_rust_lint_allows(&func.lint_allows);
            Ok(quote! {
                #(#lint_allows)*
                fn #name #generics (#(#params),*) #ret #sized_where;
            })
        } else {
            *self.current_function_return_type.borrow_mut() = Some(func.return_type.clone());
            let body_stmts = self.emit_stmts(&func.body)?;
            *self.current_function_return_type.borrow_mut() = None;

            let lint_allows = self.emit_rust_lint_allows(&func.lint_allows);
            Ok(quote! {
                #(#lint_allows)*
                fn #name #generics (#(#params),*) #ret #sized_where {
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

    /// Emit RFC 057 lint suppressions as Rust item attributes.
    ///
    /// The typechecker has already rejected broad groups and malformed paths; emission parses each preserved lint path
    /// into tokens so `clippy::lint_name` remains a Rust path rather than a string literal.
    pub(in crate::backend::ir::emit) fn emit_rust_lint_allows(&self, allows: &[IrRustLintAllow]) -> Vec<TokenStream> {
        if allows.is_empty() {
            return Vec::new();
        }

        let lint_paths = allows.iter().map(|allow| {
            let segments: Vec<TokenStream> = allow
                .lint
                .split("::")
                .map(Self::rust_ident)
                .map(|ident| quote! { #ident })
                .collect();
            join_path_tokens(&segments)
        });
        vec![quote! { #[allow(#(#lint_paths),*)] }]
    }

    /// Emit `_` for parameters that are provably unused in a generated body.
    ///
    /// This keeps normal generated Rust warning-clean without moving the former blanket `unused_variables` allow to
    /// every function item. Parameters that are read, assigned, forwarded, or otherwise referenced keep their authored
    /// name so the body continues to compile.
    fn emit_param_name(name: &str, used_names: &HashSet<String>) -> TokenStream {
        if used_names.contains(name) {
            let ident = Self::rust_ident(name);
            quote! { #ident }
        } else {
            quote! { _ }
        }
    }

    /// Collect non-`self` parameter names that the lowered body actually references.
    fn collect_function_used_names(func: &super::super::super::decl::IrFunction) -> HashSet<String> {
        let param_names = func
            .params
            .iter()
            .filter(|param| !param.is_self)
            .map(|param| param.name.clone())
            .collect::<HashSet<_>>();
        let mut used_names = HashSet::new();
        let mut shadowed_names = HashSet::new();
        Self::collect_stmt_list_used_names(&func.body, &param_names, &mut shadowed_names, &mut used_names);
        used_names
    }

    /// Record a parameter reference unless a nearer local binding has shadowed that name.
    fn note_param_use(
        name: &str,
        param_names: &HashSet<String>,
        shadowed_names: &HashSet<String>,
        used_names: &mut HashSet<String>,
    ) {
        if param_names.contains(name) && !shadowed_names.contains(name) {
            used_names.insert(name.to_string());
        }
    }

    /// Add names bound by a pattern to the current shadow set.
    fn shadow_pattern_bindings(pattern: &Pattern, shadowed_names: &mut HashSet<String>) {
        match pattern {
            Pattern::Var(name) => {
                shadowed_names.insert(name.clone());
            }
            Pattern::Tuple(items) | Pattern::Enum { fields: items, .. } | Pattern::Or(items) => {
                for item in items {
                    Self::shadow_pattern_bindings(item, shadowed_names);
                }
            }
            Pattern::Struct { fields, .. } => {
                for (_, pattern) in fields {
                    Self::shadow_pattern_bindings(pattern, shadowed_names);
                }
            }
            Pattern::Wildcard | Pattern::Literal(_) => {}
        }
    }

    /// Walk a sequential statement list while preserving lexical shadowing across later statements.
    fn collect_stmt_list_used_names(
        stmts: &[IrStmt],
        param_names: &HashSet<String>,
        shadowed_names: &mut HashSet<String>,
        used_names: &mut HashSet<String>,
    ) {
        for stmt in stmts {
            Self::collect_stmt_used_names(stmt, param_names, shadowed_names, used_names);
        }
    }

    /// Collect parameter references from a statement.
    fn collect_stmt_used_names(
        stmt: &IrStmt,
        param_names: &HashSet<String>,
        shadowed_names: &mut HashSet<String>,
        used_names: &mut HashSet<String>,
    ) {
        match &stmt.kind {
            IrStmtKind::Expr(expr) => Self::collect_expr_used_names(expr, param_names, shadowed_names, used_names),
            IrStmtKind::Let { name, value, .. } => {
                Self::collect_expr_used_names(value, param_names, shadowed_names, used_names);
                shadowed_names.insert(name.clone());
            }
            IrStmtKind::Assign { target, value } => {
                Self::collect_assign_target_used_names(target, param_names, shadowed_names, used_names);
                Self::collect_expr_used_names(value, param_names, shadowed_names, used_names);
            }
            IrStmtKind::CompoundAssign { target, value, .. } => {
                Self::collect_assign_target_used_names(target, param_names, shadowed_names, used_names);
                Self::collect_expr_used_names(value, param_names, shadowed_names, used_names);
            }
            IrStmtKind::Return(Some(expr)) | IrStmtKind::Break { value: Some(expr), .. } => {
                Self::collect_expr_used_names(expr, param_names, shadowed_names, used_names);
            }
            IrStmtKind::Return(None) | IrStmtKind::Break { value: None, .. } | IrStmtKind::Continue(_) => {}
            IrStmtKind::While { condition, body, .. } => {
                Self::collect_expr_used_names(condition, param_names, shadowed_names, used_names);
                let mut body_shadowed = shadowed_names.clone();
                Self::collect_stmt_list_used_names(body, param_names, &mut body_shadowed, used_names);
            }
            IrStmtKind::For {
                pattern,
                iterable,
                body,
                ..
            } => {
                Self::collect_expr_used_names(iterable, param_names, shadowed_names, used_names);
                let mut body_shadowed = shadowed_names.clone();
                Self::collect_pattern_used_names(pattern, param_names, &body_shadowed, used_names);
                Self::shadow_pattern_bindings(pattern, &mut body_shadowed);
                Self::collect_stmt_list_used_names(body, param_names, &mut body_shadowed, used_names);
            }
            IrStmtKind::Loop { body, .. } | IrStmtKind::Block(body) => {
                let mut body_shadowed = shadowed_names.clone();
                Self::collect_stmt_list_used_names(body, param_names, &mut body_shadowed, used_names);
            }
            IrStmtKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                Self::collect_expr_used_names(condition, param_names, shadowed_names, used_names);
                let mut then_shadowed = shadowed_names.clone();
                Self::collect_stmt_list_used_names(then_branch, param_names, &mut then_shadowed, used_names);
                if let Some(branch) = else_branch {
                    let mut else_shadowed = shadowed_names.clone();
                    Self::collect_stmt_list_used_names(branch, param_names, &mut else_shadowed, used_names);
                }
            }
            IrStmtKind::Match { scrutinee, arms } => {
                Self::collect_expr_used_names(scrutinee, param_names, shadowed_names, used_names);
                for arm in arms {
                    Self::collect_match_arm_used_names(arm, param_names, shadowed_names, used_names);
                }
            }
        }
    }

    /// Collect parameter references needed by an assignment target.
    fn collect_assign_target_used_names(
        target: &AssignTarget,
        param_names: &HashSet<String>,
        shadowed_names: &HashSet<String>,
        used_names: &mut HashSet<String>,
    ) {
        match target {
            AssignTarget::Var(name) | AssignTarget::StaticBinding(name) => {
                Self::note_param_use(name, param_names, shadowed_names, used_names);
            }
            AssignTarget::Static(_) => {}
            AssignTarget::Field { object, .. } => {
                Self::collect_expr_used_names(object, param_names, shadowed_names, used_names);
            }
            AssignTarget::Index { object, index } => {
                Self::collect_expr_used_names(object, param_names, shadowed_names, used_names);
                Self::collect_expr_used_names(index, param_names, shadowed_names, used_names);
            }
        }
    }

    /// Collect parameter references from a call argument expression.
    fn collect_call_arg_used_names(
        arg: &IrCallArg,
        param_names: &HashSet<String>,
        shadowed_names: &HashSet<String>,
        used_names: &mut HashSet<String>,
    ) {
        Self::collect_expr_used_names(&arg.expr, param_names, shadowed_names, used_names);
    }

    /// Collect parameter references from one match arm with pattern bindings scoped to that arm.
    fn collect_match_arm_used_names(
        arm: &MatchArm,
        param_names: &HashSet<String>,
        shadowed_names: &HashSet<String>,
        used_names: &mut HashSet<String>,
    ) {
        let mut arm_shadowed = shadowed_names.clone();
        Self::collect_pattern_used_names(&arm.pattern, param_names, &arm_shadowed, used_names);
        Self::shadow_pattern_bindings(&arm.pattern, &mut arm_shadowed);
        if let Some(guard) = &arm.guard {
            Self::collect_expr_used_names(guard, param_names, &arm_shadowed, used_names);
        }
        Self::collect_expr_used_names(&arm.body, param_names, &arm_shadowed, used_names);
    }

    /// Collect parameter references embedded in non-binding pattern expressions.
    fn collect_pattern_used_names(
        pattern: &Pattern,
        param_names: &HashSet<String>,
        shadowed_names: &HashSet<String>,
        used_names: &mut HashSet<String>,
    ) {
        match pattern {
            Pattern::Literal(expr) => Self::collect_expr_used_names(expr, param_names, shadowed_names, used_names),
            Pattern::Tuple(items) | Pattern::Enum { fields: items, .. } | Pattern::Or(items) => {
                for item in items {
                    Self::collect_pattern_used_names(item, param_names, shadowed_names, used_names);
                }
            }
            Pattern::Struct { fields, .. } => {
                for (_, pattern) in fields {
                    Self::collect_pattern_used_names(pattern, param_names, shadowed_names, used_names);
                }
            }
            Pattern::Wildcard | Pattern::Var(_) => {}
        }
    }

    /// Collect parameter references from an expression.
    fn collect_expr_used_names(
        expr: &super::super::super::TypedExpr,
        param_names: &HashSet<String>,
        shadowed_names: &HashSet<String>,
        used_names: &mut HashSet<String>,
    ) {
        match &expr.kind {
            IrExprKind::Var { name, .. } | IrExprKind::StaticRead { name } | IrExprKind::StaticBinding { name } => {
                Self::note_param_use(name, param_names, shadowed_names, used_names);
            }
            IrExprKind::BinOp { left, right, .. } => {
                Self::collect_expr_used_names(left, param_names, shadowed_names, used_names);
                Self::collect_expr_used_names(right, param_names, shadowed_names, used_names);
            }
            IrExprKind::UnaryOp { operand, .. }
            | IrExprKind::Await(operand)
            | IrExprKind::Try(operand)
            | IrExprKind::Cast { expr: operand, .. }
            | IrExprKind::NumericResize { expr: operand, .. }
            | IrExprKind::InteropCoerce { expr: operand, .. } => {
                Self::collect_expr_used_names(operand, param_names, shadowed_names, used_names);
            }
            IrExprKind::Call { func, args, .. } => {
                Self::collect_expr_used_names(func, param_names, shadowed_names, used_names);
                for arg in args {
                    Self::collect_call_arg_used_names(arg, param_names, shadowed_names, used_names);
                }
            }
            IrExprKind::BuiltinCall { args, .. } | IrExprKind::Set(args) | IrExprKind::Tuple(args) => {
                for arg in args {
                    Self::collect_expr_used_names(arg, param_names, shadowed_names, used_names);
                }
            }
            IrExprKind::MethodCall { receiver, args, .. } | IrExprKind::KnownMethodCall { receiver, args, .. } => {
                Self::collect_expr_used_names(receiver, param_names, shadowed_names, used_names);
                for arg in args {
                    Self::collect_call_arg_used_names(arg, param_names, shadowed_names, used_names);
                }
            }
            IrExprKind::Field { object, .. } => {
                Self::collect_expr_used_names(object, param_names, shadowed_names, used_names);
            }
            IrExprKind::Index { object, index } => {
                Self::collect_expr_used_names(object, param_names, shadowed_names, used_names);
                Self::collect_expr_used_names(index, param_names, shadowed_names, used_names);
            }
            IrExprKind::Slice {
                target,
                start,
                end,
                step,
            } => {
                Self::collect_expr_used_names(target, param_names, shadowed_names, used_names);
                if let Some(expr) = start {
                    Self::collect_expr_used_names(expr, param_names, shadowed_names, used_names);
                }
                if let Some(expr) = end {
                    Self::collect_expr_used_names(expr, param_names, shadowed_names, used_names);
                }
                if let Some(expr) = step {
                    Self::collect_expr_used_names(expr, param_names, shadowed_names, used_names);
                }
            }
            IrExprKind::ListComp {
                element,
                pattern,
                iterable,
                filter,
                ..
            } => {
                Self::collect_expr_used_names(iterable, param_names, shadowed_names, used_names);
                let mut comp_shadowed = shadowed_names.clone();
                Self::shadow_pattern_bindings(pattern, &mut comp_shadowed);
                Self::collect_expr_used_names(element, param_names, &comp_shadowed, used_names);
                if let Some(expr) = filter {
                    Self::collect_expr_used_names(expr, param_names, &comp_shadowed, used_names);
                }
            }
            IrExprKind::DictComp {
                key,
                value,
                pattern,
                iterable,
                filter,
                ..
            } => {
                Self::collect_expr_used_names(iterable, param_names, shadowed_names, used_names);
                let mut comp_shadowed = shadowed_names.clone();
                Self::shadow_pattern_bindings(pattern, &mut comp_shadowed);
                Self::collect_expr_used_names(key, param_names, &comp_shadowed, used_names);
                Self::collect_expr_used_names(value, param_names, &comp_shadowed, used_names);
                if let Some(expr) = filter {
                    Self::collect_expr_used_names(expr, param_names, &comp_shadowed, used_names);
                }
            }
            IrExprKind::List(entries) => {
                for entry in entries {
                    match entry {
                        IrListEntry::Element(expr) | IrListEntry::Spread(expr) => {
                            Self::collect_expr_used_names(expr, param_names, shadowed_names, used_names);
                        }
                    }
                }
            }
            IrExprKind::Dict(entries) => {
                for entry in entries {
                    match entry {
                        IrDictEntry::Pair(key, value) => {
                            Self::collect_expr_used_names(key, param_names, shadowed_names, used_names);
                            Self::collect_expr_used_names(value, param_names, shadowed_names, used_names);
                        }
                        IrDictEntry::Spread(expr) => {
                            Self::collect_expr_used_names(expr, param_names, shadowed_names, used_names);
                        }
                    }
                }
            }
            IrExprKind::Struct { fields, .. } => {
                for (_, expr) in fields {
                    Self::collect_expr_used_names(expr, param_names, shadowed_names, used_names);
                }
            }
            IrExprKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                Self::collect_expr_used_names(condition, param_names, shadowed_names, used_names);
                let then_shadowed = shadowed_names.clone();
                Self::collect_expr_used_names(then_branch, param_names, &then_shadowed, used_names);
                if let Some(expr) = else_branch {
                    let else_shadowed = shadowed_names.clone();
                    Self::collect_expr_used_names(expr, param_names, &else_shadowed, used_names);
                }
            }
            IrExprKind::Match { scrutinee, arms } => {
                Self::collect_expr_used_names(scrutinee, param_names, shadowed_names, used_names);
                for arm in arms {
                    Self::collect_match_arm_used_names(arm, param_names, shadowed_names, used_names);
                }
            }
            IrExprKind::Closure { params, body, captures } => {
                for capture in captures {
                    Self::note_param_use(capture, param_names, shadowed_names, used_names);
                }
                let mut closure_shadowed = shadowed_names.clone();
                for (name, _) in params {
                    closure_shadowed.insert(name.clone());
                }
                Self::collect_expr_used_names(body, param_names, &closure_shadowed, used_names);
            }
            IrExprKind::Block { stmts, value } => {
                let mut block_shadowed = shadowed_names.clone();
                Self::collect_stmt_list_used_names(stmts, param_names, &mut block_shadowed, used_names);
                if let Some(expr) = value {
                    Self::collect_expr_used_names(expr, param_names, &block_shadowed, used_names);
                }
            }
            IrExprKind::Loop { body } => {
                let mut loop_shadowed = shadowed_names.clone();
                Self::collect_stmt_list_used_names(body, param_names, &mut loop_shadowed, used_names);
            }
            IrExprKind::Range { start, end, .. } => {
                if let Some(expr) = start {
                    Self::collect_expr_used_names(expr, param_names, shadowed_names, used_names);
                }
                if let Some(expr) = end {
                    Self::collect_expr_used_names(expr, param_names, shadowed_names, used_names);
                }
            }
            IrExprKind::Format { parts } => {
                for part in parts {
                    if let super::super::super::expr::FormatPart::Expr(expr) = part {
                        Self::collect_expr_used_names(expr, param_names, shadowed_names, used_names);
                    }
                }
            }
            IrExprKind::Unit
            | IrExprKind::None
            | IrExprKind::Bool(_)
            | IrExprKind::Int(_)
            | IrExprKind::Float(_)
            | IrExprKind::Decimal(_)
            | IrExprKind::String(_)
            | IrExprKind::Bytes(_)
            | IrExprKind::Literal(_)
            | IrExprKind::FieldsList(_)
            | IrExprKind::SerdeToJson
            | IrExprKind::SerdeFromJson(_) => {}
        }
    }
}
