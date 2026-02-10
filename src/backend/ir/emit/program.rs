//! Emit a full IR program to formatted Rust source.
//!
//! This module implements the program-level API for the IR emitter:
//!
//! - scanning for required imports/features,
//! - collecting metadata needed by downstream emission (struct/enum shape, const string folding),
//! - generating Rust items and formatting them.
//!
//! ## Notes
//!
//! - The output is formatted using `prettyplease` after parsing the generated tokens with `syn`.
//! - Emission is codegen-only: it does not read/write files or access the network.
//!
//! ## See also
//!
//! - [`crate::backend::ir::emit::IrEmitter`]
//! - [`crate::backend::ir::emit::decls`]
//! - [`crate::backend::ir::emit::expressions`]
//! - [`crate::backend::ir::emit::statements`]

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use std::collections::{HashMap, HashSet};

use super::super::decl::IrDeclKind;
use super::super::expr::IrExprKind;
use super::super::types::IrType;
use super::super::{IrDecl, IrProgram, IrStmt, IrStmtKind, TypedExpr};
use super::{EmitError, IrEmitter};
use incan_core::lang::http::HttpMethodId;

/// Import tracking for warning-free codegen.
#[derive(Default)]
struct ImportTracker {
    needs_hashmap: bool,
    needs_hashset: bool,
}

impl ImportTracker {
    fn scan_program(&mut self, program: &IrProgram) {
        for decl in &program.declarations {
            self.scan_decl(decl);
        }
    }

    fn scan_decl(&mut self, decl: &IrDecl) {
        match &decl.kind {
            IrDeclKind::Function(f) => self.scan_function(f),
            IrDeclKind::Impl(impl_block) => {
                for method in &impl_block.methods {
                    self.scan_function(method);
                }
            }
            _ => {}
        }
    }

    fn scan_function(&mut self, f: &super::super::decl::IrFunction) {
        for stmt in &f.body {
            self.scan_stmt(stmt);
        }
    }

    fn scan_stmt(&mut self, stmt: &IrStmt) {
        match &stmt.kind {
            IrStmtKind::Let { value, .. } => self.scan_expr(value),
            IrStmtKind::Expr(e) => self.scan_expr(e),
            IrStmtKind::Return(Some(e)) => self.scan_expr(e),
            IrStmtKind::Assign { value, .. } => self.scan_expr(value),
            IrStmtKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                self.scan_expr(condition);
                for s in then_branch {
                    self.scan_stmt(s);
                }
                if let Some(else_stmts) = else_branch {
                    for s in else_stmts {
                        self.scan_stmt(s);
                    }
                }
            }
            IrStmtKind::While { condition, body, .. } => {
                self.scan_expr(condition);
                for s in body {
                    self.scan_stmt(s);
                }
            }
            IrStmtKind::For { iterable, body, .. } => {
                self.scan_expr(iterable);
                for s in body {
                    self.scan_stmt(s);
                }
            }
            IrStmtKind::Match { scrutinee, arms } => {
                self.scan_expr(scrutinee);
                for arm in arms {
                    if let Some(guard) = &arm.guard {
                        self.scan_expr(guard);
                    }
                    self.scan_expr(&arm.body);
                }
            }
            _ => {}
        }
    }

    fn scan_expr(&mut self, expr: &TypedExpr) {
        match &expr.kind {
            IrExprKind::Dict(pairs) => {
                self.needs_hashmap = true;
                for (k, v) in pairs {
                    self.scan_expr(k);
                    self.scan_expr(v);
                }
            }
            IrExprKind::Set(items) => {
                self.needs_hashset = true;
                for item in items {
                    self.scan_expr(item);
                }
            }
            IrExprKind::List(items) => {
                for item in items {
                    self.scan_expr(item);
                }
            }
            IrExprKind::Call { func, args } => {
                self.scan_expr(func);
                for arg in args {
                    self.scan_expr(&arg.expr);
                }
            }
            IrExprKind::BuiltinCall { args, .. } => {
                for arg in args {
                    self.scan_expr(arg);
                }
            }
            IrExprKind::MethodCall { receiver, args, .. } => {
                self.scan_expr(receiver);
                for arg in args {
                    self.scan_expr(&arg.expr);
                }
            }
            IrExprKind::KnownMethodCall { receiver, args, .. } => {
                self.scan_expr(receiver);
                for arg in args {
                    self.scan_expr(&arg.expr);
                }
            }
            IrExprKind::BinOp { left, right, .. } => {
                self.scan_expr(left);
                self.scan_expr(right);
            }
            IrExprKind::UnaryOp { operand, .. } => self.scan_expr(operand),
            IrExprKind::Index { object, index } => {
                self.scan_expr(object);
                self.scan_expr(index);
            }
            IrExprKind::Field { object, .. } => self.scan_expr(object),
            IrExprKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                self.scan_expr(condition);
                self.scan_expr(then_branch);
                if let Some(e) = else_branch {
                    self.scan_expr(e);
                }
            }
            IrExprKind::Block { stmts, value } => {
                for s in stmts {
                    self.scan_stmt(s);
                }
                if let Some(v) = value {
                    self.scan_expr(v);
                }
            }
            IrExprKind::Struct { fields, .. } => {
                for (_, e) in fields {
                    self.scan_expr(e);
                }
            }
            _ => {}
        }
    }
}

impl<'a> IrEmitter<'a> {
    /// Emit a complete IR program to formatted Rust code.
    #[tracing::instrument(skip_all, fields(decl_count = program.declarations.len()))]
    pub fn emit_program(&mut self, program: &IrProgram) -> Result<String, EmitError> {
        // First pass: collect struct derives, struct field types, and enum variant typing
        let mut static_str_const_exprs: HashMap<String, TypedExpr> = HashMap::new();
        for decl in &program.declarations {
            if let IrDeclKind::Struct(s) = &decl.kind {
                if !s.derives.is_empty() {
                    self.struct_derives.insert(s.name.clone(), s.derives.clone());
                }
                self.struct_field_names
                    .insert(s.name.clone(), s.fields.iter().map(|f| f.name.clone()).collect());
                for field in &s.fields {
                    self.struct_field_types
                        .insert((s.name.clone(), field.name.clone()), field.ty.clone());
                    self.struct_field_aliases
                        .insert((s.name.clone(), field.name.clone()), field.alias.clone());
                    self.struct_field_descriptions
                        .insert((s.name.clone(), field.name.clone()), field.description.clone());
                    if let Some(default) = &field.default {
                        self.struct_field_defaults
                            .insert((s.name.clone(), field.name.clone()), default.clone());
                    }
                }
            }
            if let IrDeclKind::Enum(e) = &decl.kind {
                for v in &e.variants {
                    self.enum_variant_fields
                        .insert((e.name.clone(), v.name.clone()), v.fields.clone());
                }
            }
            // Collect static-str const initializer expressions for later resolution.
            if let IrDeclKind::Const { name, ty, value, .. } = &decl.kind
                && matches!(ty, IrType::StaticStr)
            {
                static_str_const_exprs.insert(name.clone(), value.clone());
            }
        }

        // Second pass: resolve all &'static str consts into full literal values (when possible).
        if !static_str_const_exprs.is_empty() {
            let mut visiting: HashSet<String> = HashSet::new();
            let mut cache: HashMap<String, String> = HashMap::new();
            for name in static_str_const_exprs.keys() {
                let _ = Self::resolve_static_str_const(name, &static_str_const_exprs, &mut visiting, &mut cache);
            }
            self.const_string_literals.extend(cache);
        }

        let tokens = self.emit_program_tokens(program)?;
        let syntax_tree = syn::parse2(tokens).map_err(|e| EmitError::SynParse(e.to_string()))?;
        let formatted = prettyplease::unparse(&syntax_tree);

        // Prepend version header, inner attributes, then mod insertion marker
        let header = format!(
            "// Generated by the Incan compiler v{}\n\n",
            crate::version::INCAN_VERSION
        );

        // Find the end of the inner attribute block and insert marker after it
        let with_marker = if formatted.contains("]\nuse ") {
            formatted.replacen("]\nuse ", "]\n\n// __INCAN_INSERT_MODS__\n\nuse ", 1)
        } else if formatted.contains("]\n\nuse ") {
            formatted.replacen("]\n\nuse ", "]\n\n// __INCAN_INSERT_MODS__\n\nuse ", 1)
        } else {
            formatted.replacen("]\n", "]\n\n// __INCAN_INSERT_MODS__\n\n", 1)
        };

        Ok(format!("{}{}", header, with_marker))
    }

    /// Emit a program to TokenStream (without formatting).
    pub fn emit_program_tokens(&self, program: &IrProgram) -> Result<TokenStream, EmitError> {
        let mut items = Vec::new();

        if self.add_clippy_allows {
            items.push(quote! {
                #![allow(unused_imports, dead_code, unused_variables)]
            });
        }

        let mut tracker = ImportTracker::default();
        tracker.scan_program(program);

        let compiler_version = crate::version::INCAN_VERSION;
        items.push(quote! { incan_stdlib::__incan_stdlib_version_check!(#compiler_version); });
        items.push(quote! { use incan_stdlib::prelude::*; });
        items.push(quote! { use incan_derive::{FieldInfo, IncanClass}; });

        match (tracker.needs_hashmap, tracker.needs_hashset) {
            (true, true) => items.push(quote! { use std::collections::{HashMap, HashSet}; }),
            (true, false) => items.push(quote! { use std::collections::HashMap; }),
            (false, true) => items.push(quote! { use std::collections::HashSet; }),
            (false, false) => {}
        }

        if self.needs_serde {
            items.push(quote! { use serde::{Serialize, Deserialize}; });
        }

        if self.needs_tokio {
            items.push(quote! { use incan_stdlib::__private::tokio::time::{sleep, timeout, Duration}; });
            items.push(quote! { use incan_stdlib::__private::tokio::sync::{mpsc, Mutex, RwLock}; });
            items.push(quote! { use incan_stdlib::__private::tokio::task::JoinHandle; });
        }

        // Web router glue (only when web is detected and we have collected routes).
        if self.needs_axum && !self.routes.is_empty() {
            items.push(self.emit_web_router_macro()?);
        }

        for decl in &program.declarations {
            items.push(self.emit_decl(decl)?);
        }

        Ok(quote! {
            #(#items)*
        })
    }

    /// Emit the web router macro.
    fn emit_web_router_macro(&self) -> Result<TokenStream, EmitError> {
        let wrappers = self.emit_web_route_wrappers()?;
        let mut routes = Vec::new();

        for r in &self.routes {
            if let Some(bad) = r.unknown_methods.first() {
                return Err(EmitError::Unsupported(format!("unsupported web method '{}'", bad)));
            }
            let path = Self::to_axum_path(&r.path)?;
            let path_lit = proc_macro2::Literal::string(&path);
            let wrapper_name = format_ident!("__incan_web_{}", r.handler_name);

            for method in &r.methods {
                let method_fn = match method {
                    HttpMethodId::Get => quote! { get },
                    HttpMethodId::Post => quote! { post },
                    HttpMethodId::Put => quote! { put },
                    HttpMethodId::Delete => quote! { delete },
                    HttpMethodId::Patch => quote! { patch },
                };
                routes.push(quote! { (#path_lit, #method_fn, #wrapper_name) });
            }
        }

        Ok(quote! {
            incan_stdlib::web::__incan_router! {
                wrappers: [ #(#wrappers)* ],
                routes: [ #(#routes),* ]
            }
        })
    }

    fn emit_web_route_wrappers(&self) -> Result<Vec<TokenStream>, EmitError> {
        let mut out = Vec::new();
        for r in &self.routes {
            let wrapper_name = format_ident!("__incan_web_{}", r.handler_name);
            let handler_ident = format_ident!("{}", Self::escape_keyword(&r.handler_name));

            // Build fully qualified handler path if in a nested module.
            let handler_call_path: TokenStream = if let Some(mod_segs) = &r.module_path_segments {
                // Build `crate::a::b::handler` without string parsing.
                let mut segs: Vec<syn::PathSegment> = Vec::with_capacity(mod_segs.len() + 2);
                segs.push(syn::PathSegment::from(syn::Ident::new(
                    "crate",
                    proc_macro2::Span::call_site(),
                )));
                for s in mod_segs {
                    let seg = Self::escape_keyword(s);
                    segs.push(syn::PathSegment::from(syn::Ident::new(
                        &seg,
                        proc_macro2::Span::call_site(),
                    )));
                }
                segs.push(syn::PathSegment::from(handler_ident.clone()));
                let full_path = syn::Path {
                    leading_colon: None,
                    segments: segs.into_iter().collect(),
                };
                quote! { #full_path }
            } else {
                quote! { #handler_ident }
            };

            let sig_opt = self.function_registry.get(&r.handler_name);
            let params = sig_opt.map(|s| &s.params[..]).unwrap_or(&[]);
            let qualify_types = r.module_path_segments.is_some();

            let path_params = Self::path_params(&r.path)?;
            let mut params_by_name: HashMap<&str, &super::super::decl::FunctionParam> = HashMap::new();
            for p in params {
                params_by_name.insert(p.name.as_str(), p);
            }

            let mut path_param_idents = Vec::new();
            let mut path_param_types = Vec::new();
            for name in &path_params {
                let Some(param) = params_by_name.get(name.as_str()) else {
                    return Err(EmitError::Unsupported(format!(
                        "web route param '{}' has no matching handler parameter",
                        name
                    )));
                };
                if Self::named_generic_arg(&param.ty, "Json").is_some()
                    || Self::named_generic_arg(&param.ty, "Query").is_some()
                {
                    return Err(EmitError::Unsupported(format!(
                        "web route param '{}' cannot use Json/Query extractor types",
                        name
                    )));
                }
                path_param_idents.push(format_ident!("{}", Self::escape_keyword(name)));
                let ty = self.emit_type_qualified_for_module(&param.ty, qualify_types)?;
                path_param_types.push(ty);
            }

            let mut args_parts: Vec<TokenStream> = Vec::new();
            if !path_param_idents.is_empty() {
                let path_arg = if path_param_idents.len() == 1 {
                    let pname = &path_param_idents[0];
                    let pty = &path_param_types[0];
                    quote! {
                        ::incan_stdlib::web::__private::extract::Path(#pname):
                            ::incan_stdlib::web::__private::extract::Path<#pty>
                    }
                } else {
                    quote! {
                        ::incan_stdlib::web::__private::extract::Path(
                            (#(#path_param_idents),*)
                        ): ::incan_stdlib::web::__private::extract::Path<(
                            #(#path_param_types),*
                        )>
                    }
                };
                args_parts.push(path_arg);
            }

            let path_param_set: HashSet<&str> = path_params.iter().map(|s| s.as_str()).collect();
            for p in params {
                if path_param_set.contains(p.name.as_str()) {
                    continue;
                }
                if let Some(inner) = Self::named_generic_arg(&p.ty, "Json") {
                    let pname = format_ident!("{}", Self::escape_keyword(&p.name));
                    let pty = self.emit_type_qualified_for_module(inner, qualify_types)?;
                    args_parts.push(quote! {
                        ::incan_stdlib::web::__private::extract::Json(#pname):
                            ::incan_stdlib::web::__private::extract::Json<#pty>
                    });
                } else if let Some(inner) = Self::named_generic_arg(&p.ty, "Query") {
                    let pname = format_ident!("{}", Self::escape_keyword(&p.name));
                    let pty = self.emit_type_qualified_for_module(inner, qualify_types)?;
                    args_parts.push(quote! {
                        ::incan_stdlib::web::__private::extract::Query(#pname):
                            ::incan_stdlib::web::__private::extract::Query<#pty>
                    });
                } else {
                    return Err(EmitError::Unsupported(format!(
                        "unsupported web handler param '{}': only path params, Json[T], and Query[T] are supported",
                        p.name
                    )));
                }
            }

            let call_args: Vec<TokenStream> = params
                .iter()
                .map(|p| {
                    let pname = format_ident!("{}", Self::escape_keyword(&p.name));
                    if path_param_set.contains(p.name.as_str()) {
                        quote! { #pname }
                    } else if Self::named_generic_arg(&p.ty, "Json").is_some() {
                        quote! { incan_stdlib::web::Json::new(#pname) }
                    } else if Self::named_generic_arg(&p.ty, "Query").is_some() {
                        quote! { incan_stdlib::web::Query::new(#pname) }
                    } else {
                        // Unreachable: the args_parts loop above returns Err for unsupported
                        // param types, so we never reach this branch during successful emission.
                        unreachable!(
                            "unsupported param '{}' should have been rejected by args_parts validation",
                            p.name
                        )
                    }
                })
                .collect();

            let call = if call_args.is_empty() {
                quote! { #handler_call_path().await }
            } else {
                quote! { #handler_call_path(#(#call_args),*).await }
            };

            out.push(quote! {
                async fn #wrapper_name(#(#args_parts),*) -> impl ::incan_stdlib::web::__private::response::IntoResponse {
                    #call
                }
            });
        }
        Ok(out)
    }

    /// Emit a type, qualifying user-defined types based on their declaration module.
    ///
    /// This is used for route wrapper parameters where the type may be declared in a dependency module.
    fn emit_type_qualified_for_module(&self, ty: &IrType, qualify_named: bool) -> Result<TokenStream, EmitError> {
        match ty {
            IrType::Unit
            | IrType::Bool
            | IrType::Int
            | IrType::Float
            | IrType::String
            | IrType::StaticStr
            | IrType::StaticBytes
            | IrType::FrozenStr
            | IrType::FrozenBytes
            | IrType::StrRef
            | IrType::Unknown
            | IrType::Generic(_)
            | IrType::SelfType => Ok(self.emit_type(ty)),
            IrType::Struct(name) | IrType::Enum(name) | IrType::Trait(name) => {
                if qualify_named {
                    self.emit_qualified_named_type(name)
                } else {
                    Ok(self.emit_type(ty))
                }
            }
            IrType::NamedGeneric(name, args) => {
                let base = if qualify_named {
                    self.emit_qualified_named_type(name)?
                } else {
                    let ident = format_ident!("{}", Self::escape_keyword(name));
                    quote! { #ident }
                };
                let inner: Vec<TokenStream> = args
                    .iter()
                    .map(|t| self.emit_type_qualified_for_module(t, qualify_named))
                    .collect::<Result<_, _>>()?;
                Ok(quote! { #base < #(#inner),* > })
            }
            IrType::List(elem) => {
                let e = self.emit_type_qualified_for_module(elem, qualify_named)?;
                Ok(quote! { Vec<#e> })
            }
            IrType::Dict(k, v) => {
                let kk = self.emit_type_qualified_for_module(k, qualify_named)?;
                let vv = self.emit_type_qualified_for_module(v, qualify_named)?;
                Ok(quote! { std::collections::HashMap<#kk, #vv> })
            }
            IrType::Set(elem) => {
                let e = self.emit_type_qualified_for_module(elem, qualify_named)?;
                Ok(quote! { std::collections::HashSet<#e> })
            }
            IrType::Tuple(types) => {
                let ts: Vec<_> = types
                    .iter()
                    .map(|t| self.emit_type_qualified_for_module(t, qualify_named))
                    .collect::<Result<_, _>>()?;
                Ok(quote! { (#(#ts),*) })
            }
            IrType::Option(inner) => {
                let i = self.emit_type_qualified_for_module(inner, qualify_named)?;
                Ok(quote! { Option<#i> })
            }
            IrType::Result(ok, err) => {
                let o = self.emit_type_qualified_for_module(ok, qualify_named)?;
                let e = self.emit_type_qualified_for_module(err, qualify_named)?;
                Ok(quote! { Result<#o, #e> })
            }
            IrType::Function { params, ret } => {
                let ps: Vec<_> = params
                    .iter()
                    .map(|p| self.emit_type_qualified_for_module(p, qualify_named))
                    .collect::<Result<_, _>>()?;
                let r = self.emit_type_qualified_for_module(ret, qualify_named)?;
                Ok(quote! { fn(#(#ps),*) -> #r })
            }
            IrType::Ref(inner) => {
                let i = self.emit_type_qualified_for_module(inner, qualify_named)?;
                Ok(quote! { &#i })
            }
            IrType::RefMut(inner) => {
                let i = self.emit_type_qualified_for_module(inner, qualify_named)?;
                Ok(quote! { &mut #i })
            }
        }
    }

    fn emit_qualified_named_type(&self, name: &str) -> Result<TokenStream, EmitError> {
        use incan_core::lang::surface::types::{self as surface_types, SurfaceTypeId};

        if name == surface_types::as_str(SurfaceTypeId::FieldInfo) {
            return Ok(quote! { incan_stdlib::reflection::FieldInfo });
        }
        if self.ambiguous_type_names.contains(name) {
            return Err(EmitError::Unsupported(format!(
                "type '{}' is declared in multiple modules; cannot qualify route wrapper type",
                name
            )));
        }
        if let Some(segs) = self.type_module_paths.get(name) {
            let mut path_segs: Vec<syn::PathSegment> = Vec::with_capacity(segs.len() + 2);
            path_segs.push(syn::PathSegment::from(syn::Ident::new(
                "crate",
                proc_macro2::Span::call_site(),
            )));
            for s in segs {
                let seg = Self::escape_keyword(s);
                path_segs.push(syn::PathSegment::from(syn::Ident::new(
                    &seg,
                    proc_macro2::Span::call_site(),
                )));
            }
            let name = Self::escape_keyword(name);
            path_segs.push(syn::PathSegment::from(syn::Ident::new(
                &name,
                proc_macro2::Span::call_site(),
            )));
            let full_path = syn::Path {
                leading_colon: None,
                segments: path_segs.into_iter().collect(),
            };
            return Ok(quote! { #full_path });
        }
        let ident = format_ident!("{}", Self::escape_keyword(name));
        Ok(quote! { #ident })
    }

    /// Convert `{param}` placeholders to axum `:param` path segments.
    fn to_axum_path(path: &str) -> Result<String, EmitError> {
        Ok(Self::parse_route_path(path)?.0)
    }

    /// Collect path parameter names in order of appearance.
    fn path_params(path: &str) -> Result<Vec<String>, EmitError> {
        Ok(Self::parse_route_path(path)?.1)
    }

    fn parse_route_path(path: &str) -> Result<(String, Vec<String>), EmitError> {
        let mut params = Vec::new();
        let mut seen = HashSet::new();
        let mut out = String::new();
        let mut chars = path.chars().peekable();
        while let Some(ch) = chars.next() {
            match ch {
                '{' => {
                    let mut name = String::new();
                    let mut closed = false;
                    for c in chars.by_ref() {
                        if c == '}' {
                            closed = true;
                            break;
                        }
                        name.push(c);
                    }
                    if !closed {
                        return Err(EmitError::Unsupported(format!(
                            "unterminated web route param in path '{}'",
                            path
                        )));
                    }
                    if name.is_empty() {
                        return Err(EmitError::Unsupported(format!(
                            "web route param name cannot be empty in path '{}'",
                            path
                        )));
                    }
                    if !Self::is_valid_param_name(&name) {
                        return Err(EmitError::Unsupported(format!(
                            "web route param '{}' is not a valid identifier in path '{}'",
                            name, path
                        )));
                    }
                    if !seen.insert(name.clone()) {
                        return Err(EmitError::Unsupported(format!(
                            "duplicate web route param '{}' in path '{}'",
                            name, path
                        )));
                    }
                    out.push(':');
                    out.push_str(&name);
                    params.push(name);
                }
                '}' => {
                    return Err(EmitError::Unsupported(format!(
                        "unmatched '}}' in web route path '{}'",
                        path
                    )));
                }
                _ => out.push(ch),
            }
        }
        Ok((out, params))
    }

    fn is_valid_param_name(name: &str) -> bool {
        let mut chars = name.chars();
        let Some(first) = chars.next() else {
            return false;
        };
        if !(first.is_ascii_alphabetic() || first == '_') {
            return false;
        }
        chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
    }

    fn named_generic_arg<'b>(ty: &'b IrType, name: &str) -> Option<&'b IrType> {
        match ty {
            IrType::NamedGeneric(type_name, args) if type_name == name && args.len() == 1 => Some(&args[0]),
            _ => None,
        }
    }
}
