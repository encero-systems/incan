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
            IrExprKind::MethodCall { receiver, args, .. } => {
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
            if let IrDeclKind::Const { name, ty, value, .. } = &decl.kind {
                if matches!(ty, IrType::StaticStr) {
                    static_str_const_exprs.insert(name.clone(), value.clone());
                }
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
            items.push(quote! { use tokio::time::{sleep, timeout, Duration}; });
            items.push(quote! { use tokio::sync::{mpsc, Mutex, RwLock}; });
            items.push(quote! { use tokio::task::JoinHandle; });
        }

        if self.needs_axum {
            items.push(quote! {
                use axum::{
                    Router,
                    routing::{get, post, put, delete, patch},
                    extract::{Path, Query, State}
                };
            });
        }

        // Web router glue (only when web is detected and we have collected routes).
        if self.needs_axum && !self.routes.is_empty() {
            items.push(self.emit_web_router_fn()?);
            items.extend(self.emit_web_route_wrappers()?);
        }

        for decl in &program.declarations {
            items.push(self.emit_decl(decl)?);
        }

        Ok(quote! {
            #(#items)*
        })
    }

    fn emit_web_route_wrappers(&self) -> Result<Vec<TokenStream>, EmitError> {
        let mut out = Vec::new();
        for r in &self.routes {
            let wrapper_name = format_ident!("__incan_web_{}", r.handler_name);
            let handler_ident = format_ident!("{}", Self::escape_keyword(&r.handler_name));

            let sig_opt = self.function_registry.get(&r.handler_name);
            let params = sig_opt.map(|s| &s.params[..]).unwrap_or(&[]);

            // For now: support 0 or 1 path params (enough for hello_web).
            let args_pat = if params.is_empty() {
                quote! {}
            } else if params.len() == 1 {
                let p = &params[0];
                let pname = format_ident!("{}", Self::escape_keyword(&p.name));
                let pty = self.emit_type(&p.ty);
                quote! { axum::extract::Path(#pname): axum::extract::Path<#pty> }
            } else {
                return Err(EmitError::Unsupported(
                    "web routes with multiple path params not yet supported".to_string(),
                ));
            };

            let call = if params.is_empty() {
                quote! { #handler_ident().await }
            } else {
                let pname = format_ident!("{}", Self::escape_keyword(&params[0].name));
                quote! { #handler_ident(#pname).await }
            };

            out.push(quote! {
                async fn #wrapper_name(#args_pat) -> impl axum::response::IntoResponse {
                    #call
                }
            });
        }
        Ok(out)
    }

    /// Emit the axum router builder for collected `@route` handlers.
    fn emit_web_router_fn(&self) -> Result<TokenStream, EmitError> {
        let mut router = quote! { axum::Router::new() };

        for r in &self.routes {
            if let Some(bad) = r.unknown_methods.first() {
                return Err(EmitError::Unsupported(format!("unsupported web method '{}'", bad)));
            }
            let path = Self::to_axum_path(&r.path);
            let path_lit = proc_macro2::Literal::string(&path);
            let wrapper_name = format_ident!("__incan_web_{}", r.handler_name);

            // For now: only support GET/POST/PUT/DELETE/PATCH single-method routes.
            let method = r.methods.first().copied().unwrap_or(HttpMethodId::Get);
            let route_layer = match method {
                HttpMethodId::Get => quote! { axum::routing::get(#wrapper_name) },
                HttpMethodId::Post => quote! { axum::routing::post(#wrapper_name) },
                HttpMethodId::Put => quote! { axum::routing::put(#wrapper_name) },
                HttpMethodId::Delete => quote! { axum::routing::delete(#wrapper_name) },
                HttpMethodId::Patch => quote! { axum::routing::patch(#wrapper_name) },
            };

            router = quote! { #router.route(#path_lit, #route_layer) };
        }

        Ok(quote! {
            fn __incan_web_router() -> axum::Router {
                #router
            }
        })
    }

    /// Convert `{param}` placeholders to axum `:param` path segments.
    fn to_axum_path(path: &str) -> String {
        // Convert `/api/{name}` → `/api/:name` (axum path params)
        let mut out = String::new();
        let mut chars = path.chars().peekable();
        while let Some(ch) = chars.next() {
            if ch == '{' {
                let mut name = String::new();
                for c in chars.by_ref() {
                    if c == '}' {
                        break;
                    }
                    name.push(c);
                }
                out.push(':');
                out.push_str(&name);
            } else {
                out.push(ch);
            }
        }
        out
    }
}
