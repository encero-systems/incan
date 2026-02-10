//! Emit Rust items from IR declarations.
//!
//! This module emits top-level Rust items for IR declarations (functions, structs/enums, consts,
//! imports, traits, and impl blocks).
//!
//! ## Notes
//!
//! - Visibility is emitted via [`crate::backend::ir::emit::types::IrEmitter::emit_visibility`].
//! - RFC-008 consts are validated and may be emitted via specialized frozen constructors.
//!
//! ## See also
//!
//! - [`crate::backend::ir::emit::consts`]
//! - [`crate::backend::ir::emit::types`]

use std::collections::HashSet;

use proc_macro2::{Literal, TokenStream};
use quote::{format_ident, quote};

use incan_core::lang::conventions;
use incan_core::lang::derives::{self, DeriveId};
use incan_core::lang::magic_methods;
use incan_core::lang::stdlib;

const ZEN_TEXT: &str = include_str!("../../../../stdlib/zen.txt");

use super::super::decl::{IrDecl, IrDeclKind, IrImportQualifier};
use super::super::expr::{IrExpr, IrExprKind, MethodKind, VarAccess};
use super::super::stmt::{AssignTarget, IrStmt, IrStmtKind};
use super::super::types::IrType;
use super::{EmitError, IrEmitter};

fn join_path_tokens(segments: &[TokenStream]) -> TokenStream {
    let mut ts = TokenStream::new();
    for (idx, seg) in segments.iter().enumerate() {
        if idx > 0 {
            ts.extend(quote! { :: });
        }
        ts.extend(seg.clone());
    }
    ts
}

impl<'a> IrEmitter<'a> {
    /// Collect the set of parameter names that are actually mutated in a function body.
    ///
    /// This is used to avoid emitting `mut`/`&mut` in Rust function signatures when the
    /// parameter is never written to, which would trigger Rust's "unused `mut`" warnings.
    fn collect_mutated_params(&self, func: &super::super::decl::IrFunction) -> HashSet<String> {
        let param_names: HashSet<String> = func.params.iter().map(|p| p.name.clone()).collect();
        let mut mutated: HashSet<String> = HashSet::new();

        for stmt in &func.body {
            self.scan_stmt_for_param_writes(stmt, &param_names, &mut mutated);
        }

        mutated
    }

    /// Scan an IR statement and record any writes that target a function parameter.
    fn scan_stmt_for_param_writes(&self, stmt: &IrStmt, param_names: &HashSet<String>, mutated: &mut HashSet<String>) {
        match &stmt.kind {
            IrStmtKind::Let { value, .. } => self.scan_expr_for_param_writes(value, param_names, mutated),
            IrStmtKind::Assign { target, value } => {
                if let Some(name) = self.assign_target_hits_param(target, param_names) {
                    mutated.insert(name);
                }
                self.scan_expr_for_param_writes(value, param_names, mutated);
            }
            IrStmtKind::CompoundAssign { target, value, .. } => {
                if let Some(name) = self.assign_target_hits_param(target, param_names) {
                    mutated.insert(name);
                }
                self.scan_expr_for_param_writes(value, param_names, mutated);
            }
            IrStmtKind::Expr(e) => self.scan_expr_for_param_writes(e, param_names, mutated),
            IrStmtKind::Return(Some(e)) => self.scan_expr_for_param_writes(e, param_names, mutated),
            IrStmtKind::Return(None) | IrStmtKind::Break(_) | IrStmtKind::Continue(_) => {}
            IrStmtKind::While { condition, body, .. } => {
                self.scan_expr_for_param_writes(condition, param_names, mutated);
                for s in body {
                    self.scan_stmt_for_param_writes(s, param_names, mutated);
                }
            }
            IrStmtKind::For { iterable, body, .. } => {
                self.scan_expr_for_param_writes(iterable, param_names, mutated);
                for s in body {
                    self.scan_stmt_for_param_writes(s, param_names, mutated);
                }
            }
            IrStmtKind::Loop { body, .. } => {
                for s in body {
                    self.scan_stmt_for_param_writes(s, param_names, mutated);
                }
            }
            IrStmtKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                self.scan_expr_for_param_writes(condition, param_names, mutated);
                for s in then_branch {
                    self.scan_stmt_for_param_writes(s, param_names, mutated);
                }
                if let Some(else_branch) = else_branch {
                    for s in else_branch {
                        self.scan_stmt_for_param_writes(s, param_names, mutated);
                    }
                }
            }
            IrStmtKind::Match { scrutinee, arms } => {
                self.scan_expr_for_param_writes(scrutinee, param_names, mutated);
                for arm in arms {
                    if let Some(guard) = &arm.guard {
                        self.scan_expr_for_param_writes(guard, param_names, mutated);
                    }
                    self.scan_expr_for_param_writes(&arm.body, param_names, mutated);
                }
            }
            IrStmtKind::Block(stmts) => {
                for s in stmts {
                    self.scan_stmt_for_param_writes(s, param_names, mutated);
                }
            }
        }
    }

    /// Scan an IR expression and record any writes that target a function parameter.
    fn scan_expr_for_param_writes(&self, expr: &IrExpr, param_names: &HashSet<String>, mutated: &mut HashSet<String>) {
        match &expr.kind {
            IrExprKind::Var { name, access, .. } => {
                if *access == VarAccess::BorrowMut && param_names.contains(name) {
                    mutated.insert(name.clone());
                }
            }
            IrExprKind::BinOp { left, right, .. } => {
                self.scan_expr_for_param_writes(left, param_names, mutated);
                self.scan_expr_for_param_writes(right, param_names, mutated);
            }
            IrExprKind::UnaryOp { operand, .. } => {
                self.scan_expr_for_param_writes(operand, param_names, mutated);
            }
            IrExprKind::Call { func, args } => {
                self.scan_expr_for_param_writes(func, param_names, mutated);
                for arg in args {
                    self.scan_expr_for_param_writes(&arg.expr, param_names, mutated);
                }
            }
            IrExprKind::BuiltinCall { args, .. } => {
                for arg in args {
                    self.scan_expr_for_param_writes(arg, param_names, mutated);
                }
            }
            IrExprKind::MethodCall { receiver, method, args } => {
                if let Some(name) = self.expr_is_param_var(receiver, param_names)
                    && Self::is_mutating_method_name(method)
                {
                    mutated.insert(name);
                }
                self.scan_expr_for_param_writes(receiver, param_names, mutated);
                for arg in args {
                    self.scan_expr_for_param_writes(&arg.expr, param_names, mutated);
                }
            }
            IrExprKind::KnownMethodCall {
                receiver, kind, args, ..
            } => {
                if let Some(name) = self.expr_is_param_var(receiver, param_names)
                    && Self::is_mutating_method_kind(kind)
                {
                    mutated.insert(name);
                }
                self.scan_expr_for_param_writes(receiver, param_names, mutated);
                for arg in args {
                    self.scan_expr_for_param_writes(&arg.expr, param_names, mutated);
                }
            }
            IrExprKind::Field { object, .. } => {
                self.scan_expr_for_param_writes(object, param_names, mutated);
            }
            IrExprKind::Index { object, index } => {
                self.scan_expr_for_param_writes(object, param_names, mutated);
                self.scan_expr_for_param_writes(index, param_names, mutated);
            }
            IrExprKind::Slice {
                target,
                start,
                end,
                step,
            } => {
                self.scan_expr_for_param_writes(target, param_names, mutated);
                if let Some(s) = start {
                    self.scan_expr_for_param_writes(s, param_names, mutated);
                }
                if let Some(e) = end {
                    self.scan_expr_for_param_writes(e, param_names, mutated);
                }
                if let Some(st) = step {
                    self.scan_expr_for_param_writes(st, param_names, mutated);
                }
            }
            IrExprKind::ListComp {
                element,
                iterable,
                filter,
                ..
            } => {
                self.scan_expr_for_param_writes(element, param_names, mutated);
                self.scan_expr_for_param_writes(iterable, param_names, mutated);
                if let Some(f) = filter {
                    self.scan_expr_for_param_writes(f, param_names, mutated);
                }
            }
            IrExprKind::DictComp {
                key,
                value,
                iterable,
                filter,
                ..
            } => {
                self.scan_expr_for_param_writes(key, param_names, mutated);
                self.scan_expr_for_param_writes(value, param_names, mutated);
                self.scan_expr_for_param_writes(iterable, param_names, mutated);
                if let Some(f) = filter {
                    self.scan_expr_for_param_writes(f, param_names, mutated);
                }
            }
            IrExprKind::List(items) | IrExprKind::Tuple(items) | IrExprKind::Set(items) => {
                for i in items {
                    self.scan_expr_for_param_writes(i, param_names, mutated);
                }
            }
            IrExprKind::Dict(pairs) => {
                for (k, v) in pairs {
                    self.scan_expr_for_param_writes(k, param_names, mutated);
                    self.scan_expr_for_param_writes(v, param_names, mutated);
                }
            }
            IrExprKind::Struct { fields, .. } => {
                for (_, v) in fields {
                    self.scan_expr_for_param_writes(v, param_names, mutated);
                }
            }
            IrExprKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                self.scan_expr_for_param_writes(condition, param_names, mutated);
                self.scan_expr_for_param_writes(then_branch, param_names, mutated);
                if let Some(e) = else_branch {
                    self.scan_expr_for_param_writes(e, param_names, mutated);
                }
            }
            IrExprKind::Match { scrutinee, arms } => {
                self.scan_expr_for_param_writes(scrutinee, param_names, mutated);
                for arm in arms {
                    if let Some(guard) = &arm.guard {
                        self.scan_expr_for_param_writes(guard, param_names, mutated);
                    }
                    self.scan_expr_for_param_writes(&arm.body, param_names, mutated);
                }
            }
            IrExprKind::Closure { body, .. } => {
                self.scan_expr_for_param_writes(body, param_names, mutated);
            }
            IrExprKind::Block { stmts, value } => {
                for s in stmts {
                    self.scan_stmt_for_param_writes(s, param_names, mutated);
                }
                if let Some(v) = value {
                    self.scan_expr_for_param_writes(v, param_names, mutated);
                }
            }
            IrExprKind::Await(inner) | IrExprKind::Try(inner) | IrExprKind::Cast { expr: inner, .. } => {
                self.scan_expr_for_param_writes(inner, param_names, mutated);
            }
            IrExprKind::Range { start, end, .. } => {
                if let Some(s) = start {
                    self.scan_expr_for_param_writes(s, param_names, mutated);
                }
                if let Some(e) = end {
                    self.scan_expr_for_param_writes(e, param_names, mutated);
                }
            }
            IrExprKind::Format { parts } => {
                for part in parts {
                    if let super::super::expr::FormatPart::Expr(e) = part {
                        self.scan_expr_for_param_writes(e, param_names, mutated);
                    }
                }
            }
            // Literals and variants without child expressions
            _ => {}
        }
    }

    /// Check if an assignment target hits a function parameter.
    fn assign_target_hits_param(&self, target: &AssignTarget, param_names: &HashSet<String>) -> Option<String> {
        match target {
            AssignTarget::Var(name) if param_names.contains(name) => Some(name.clone()),
            AssignTarget::Field { object, .. } | AssignTarget::Index { object, .. } => {
                self.expr_is_param_var(object, param_names)
            }
            _ => None,
        }
    }

    /// Check if an expression is a function parameter.
    fn expr_is_param_var(&self, expr: &IrExpr, param_names: &HashSet<String>) -> Option<String> {
        if let IrExprKind::Var { name, .. } = &expr.kind
            && param_names.contains(name)
        {
            return Some(name.clone());
        }
        None
    }

    /// Check if a method name is mutating.
    fn is_mutating_method_name(name: &str) -> bool {
        matches!(
            name,
            "append" | "pop" | "insert" | "remove" | "swap" | "reserve" | "reserve_exact" | "push" | "clear"
        )
    }

    /// Check if a method kind is mutating.
    fn is_mutating_method_kind(kind: &MethodKind) -> bool {
        matches!(
            kind,
            MethodKind::Append
                | MethodKind::Pop
                | MethodKind::Insert
                | MethodKind::Remove
                | MethodKind::Swap
                | MethodKind::Reserve
                | MethodKind::ReserveExact
        )
    }

    /// Emit a declaration as Rust tokens.
    pub(super) fn emit_decl(&self, decl: &IrDecl) -> Result<TokenStream, EmitError> {
        match &decl.kind {
            IrDeclKind::Function(func) => self.emit_function(func),
            IrDeclKind::Struct(s) => self.emit_struct(s),
            IrDeclKind::Enum(e) => self.emit_enum(e),
            IrDeclKind::TypeAlias { name, ty } => {
                let name_ident = format_ident!("{}", name);
                let ty_tokens = self.emit_type(ty);
                Ok(quote! {
                    type #name_ident = #ty_tokens;
                })
            }
            IrDeclKind::Const {
                visibility,
                name,
                ty,
                value,
            } => {
                // RFC 008: Only emit representable consts; otherwise, error out.
                self.validate_const_emittable(name, ty, value)?;

                let vis = self.emit_visibility(visibility);
                let name_ident = format_ident!("{}", name);
                let ty_tokens = self.emit_type(ty);

                // If this is a FrozenList/Set/Dict with literal initializer, emit via FrozenX::new(&[...]).
                use super::super::types::IrType as T;
                use incan_core::lang::types::collections::{self, CollectionTypeId};
                let specialized_tokens: Option<TokenStream> = match (ty, &value.kind) {
                    (T::NamedGeneric(n, args), IrExprKind::List(items))
                        if n == collections::as_str(CollectionTypeId::FrozenList) && args.len() == 1 =>
                    {
                        let elems: Result<Vec<_>, EmitError> = items.iter().map(|i| self.emit_expr(i)).collect();
                        let elems = elems?;
                        Some(quote! { FrozenList::new(&[ #(#elems),* ]) })
                    }
                    (T::NamedGeneric(n, args), IrExprKind::Set(items))
                        if n == collections::as_str(CollectionTypeId::FrozenSet) && args.len() == 1 =>
                    {
                        let elems: Result<Vec<_>, EmitError> = items.iter().map(|i| self.emit_expr(i)).collect();
                        let elems = elems?;
                        Some(quote! { FrozenSet::new(&[ #(#elems),* ]) })
                    }
                    (T::NamedGeneric(n, args), IrExprKind::Dict(pairs))
                        if n == collections::as_str(CollectionTypeId::FrozenDict) && args.len() == 2 =>
                    {
                        let kvs: Result<Vec<_>, EmitError> = pairs
                            .iter()
                            .map(|(k, v)| {
                                let kk = self.emit_expr(k)?;
                                let vv = self.emit_expr(v)?;
                                Ok(quote! { ( #kk , #vv ) })
                            })
                            .collect();
                        let kvs = kvs?;
                        Some(quote! { FrozenDict::new(&[ #(#kvs),* ]) })
                    }
                    _ => None,
                };

                let value_tokens = if let Some(tok) = specialized_tokens {
                    tok
                } else {
                    match (ty, &value.kind) {
                        // RFC 008: frozen scalars.
                        // These types are wrappers around `'static` data and must be constructed explicitly.
                        (T::FrozenStr, IrExprKind::String(s)) => {
                            quote! { FrozenStr::new(#s) }
                        }
                        (T::FrozenBytes, IrExprKind::Bytes(bytes)) => {
                            let lit = Literal::byte_string(bytes);
                            quote! { FrozenBytes::new(#lit) }
                        }
                        _ => self.emit_expr(value)?,
                    }
                };

                Ok(quote! {
                    #vis const #name_ident: #ty_tokens = #value_tokens;
                })
            }
            IrDeclKind::Import {
                qualifier,
                path,
                alias,
                items,
            } => {
                // Skip serde imports if we're already importing them automatically
                if self.needs_serde && path.len() == 1 && path[0] == "serde" {
                    let is_serde_trait = items.iter().any(|item| {
                        matches!(
                            derives::from_str(item.name.as_str()),
                            Some(DeriveId::Serialize | DeriveId::Deserialize)
                        )
                    });
                    if is_serde_trait {
                        return Ok(quote! {});
                    }
                }

                // Special-case stdlib shims:
                // - `std.web` maps to `incan_stdlib::web`
                // - `std.testing` maps to `incan_stdlib::testing`
                let is_stdlib_web = stdlib::is_stdlib_module(path, stdlib::STDLIB_WEB);
                let is_stdlib_testing = stdlib::is_stdlib_module(path, stdlib::STDLIB_TESTING);
                let mapped_path_tokens: Vec<_> = if is_stdlib_web {
                    vec![quote! { incan_stdlib }, quote! { web }]
                } else if is_stdlib_testing {
                    vec![quote! { incan_stdlib }, quote! { testing }]
                } else {
                    path.iter()
                        .map(|s| {
                            let ident = format_ident!("{}", s);
                            quote! { #ident }
                        })
                        .collect()
                };
                let mut path_tokens: Vec<TokenStream> = Vec::new();
                let apply_prefix = !(is_stdlib_web || is_stdlib_testing);
                if apply_prefix {
                    match qualifier {
                        IrImportQualifier::Auto => {
                            if self.is_internal_module_path(path) {
                                path_tokens.push(quote! { crate });
                            }
                        }
                        IrImportQualifier::Crate => path_tokens.push(quote! { crate }),
                        IrImportQualifier::Super(levels) => {
                            for _ in 0..*levels {
                                path_tokens.push(quote! { super });
                            }
                        }
                        IrImportQualifier::None => {}
                    }
                }
                path_tokens.extend(mapped_path_tokens);
                let path_ts = join_path_tokens(&path_tokens);

                if let Some(alias_name) = alias {
                    let alias_ident = format_ident!("{}", alias_name);
                    Ok(quote! {
                        use #path_ts as #alias_ident;
                    })
                } else if !items.is_empty() {
                    let item_stmts: Vec<TokenStream> = items
                        .iter()
                        .map(|item| {
                            let name_ident = format_ident!("{}", &item.name);
                            let path_tokens_clone = path_tokens.clone();
                            let path_ts_clone = join_path_tokens(&path_tokens_clone);
                            if let Some(alias) = &item.alias {
                                let alias_ident = format_ident!("{}", alias);
                                quote! { use #path_ts_clone :: #name_ident as #alias_ident; }
                            } else {
                                quote! { use #path_ts_clone :: #name_ident; }
                            }
                        })
                        .collect();
                    Ok(quote! { #(#item_stmts)* })
                } else if path.len() == 1 && !is_stdlib_web && !is_stdlib_testing {
                    Ok(quote! {})
                } else {
                    Ok(quote! {
                        use #path_ts;
                    })
                }
            }
            IrDeclKind::Impl(impl_block) => self.emit_impl(impl_block),
            IrDeclKind::Trait(trait_decl) => self.emit_trait(trait_decl),
        }
    }

    fn emit_trait(&self, trait_decl: &super::super::decl::IrTrait) -> Result<TokenStream, EmitError> {
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

    fn emit_trait_method(&self, func: &super::super::decl::IrFunction) -> Result<TokenStream, EmitError> {
        let name = format_ident!("{}", &func.name);

        let params: Vec<TokenStream> = func
            .params
            .iter()
            .map(|p| {
                if p.is_self {
                    match p.mutability {
                        super::super::types::Mutability::Mutable => quote! { &mut self },
                        super::super::types::Mutability::Immutable => quote! { &self },
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

        if func.body.is_empty() {
            Ok(quote! {
                fn #name(#(#params),*) #ret;
            })
        } else {
            *self.current_function_return_type.borrow_mut() = Some(func.return_type.clone());
            let body_stmts: Vec<TokenStream> = func.body.iter().map(|s| self.emit_stmt(s)).collect::<Result<_, _>>()?;
            *self.current_function_return_type.borrow_mut() = None;

            Ok(quote! {
                fn #name(#(#params),*) #ret {
                    #(#body_stmts)*
                }
            })
        }
    }

    fn emit_fields_method(&self, struct_name: &str) -> Result<Option<TokenStream>, EmitError> {
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
        Ok(Some(quote! {
            /// Returns field metadata for this type.
            pub fn __fields__(&self) -> incan_stdlib::frozen::FrozenList<incan_stdlib::reflection::FieldInfo> {
                static __INCAN_FIELDS: [incan_stdlib::reflection::FieldInfo; #field_count] = [#(#field_infos),*];
                incan_stdlib::frozen::FrozenList::new(&__INCAN_FIELDS)
            }
        }))
    }

    fn emit_impl(&self, impl_block: &super::super::decl::IrImpl) -> Result<TokenStream, EmitError> {
        let target_type = format_ident!("{}", &impl_block.target_type);

        let mut regular_methods = Vec::new();
        let mut trait_impls = Vec::new();

        for method in &impl_block.methods {
            match magic_methods::from_str(method.name.as_str()) {
                Some(magic_methods::MagicMethodId::Eq) => {
                    let body_stmts: Vec<TokenStream> = method
                        .body
                        .iter()
                        .map(|s| self.emit_stmt(s))
                        .collect::<Result<_, _>>()?;
                    trait_impls.push(quote! {
                        impl PartialEq for #target_type {
                            fn eq(&self, other: &Self) -> bool {
                                #(#body_stmts)*
                            }
                        }
                    });
                }
                Some(magic_methods::MagicMethodId::Str) => {
                    regular_methods.push(self.emit_method(method)?);
                    trait_impls.push(quote! {
                        impl std::fmt::Display for #target_type {
                            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                                write!(f, "{}", self.__str__())
                            }
                        }
                    });
                }
                Some(magic_methods::MagicMethodId::ClassName) | Some(magic_methods::MagicMethodId::Fields) => {
                    regular_methods.push(self.emit_method(method)?)
                }
                _ => regular_methods.push(self.emit_method(method)?),
            }
        }

        let has_fields_method = impl_block.methods.iter().any(|m| m.name == "__fields__");
        if impl_block.trait_name.is_none()
            && !has_fields_method
            && let Some(fields_method) = self.emit_fields_method(&impl_block.target_type)?
        {
            regular_methods.push(fields_method);
        }

        // serde-derived convenience methods (legacy behavior)
        if impl_block.trait_name.is_none()
            && let Some(derives) = self.struct_derives.get(&impl_block.target_type)
        {
            let has_serialize = derives
                .iter()
                .any(|d| derives::from_str(d.as_str()) == Some(DeriveId::Serialize));
            let has_deserialize = derives
                .iter()
                .any(|d| derives::from_str(d.as_str()) == Some(DeriveId::Deserialize));

            if has_serialize {
                regular_methods.push(quote! {
                    /// Serialize this model to a JSON string
                    pub fn to_json(&self) -> String {
                        serde_json::to_string(self).unwrap_or_else(|_| {
                            incan_stdlib::errors::raise_json_serialization_error(stringify!(#target_type))
                        })
                    }
                });
            }
            if has_deserialize {
                regular_methods.push(quote! {
                    /// Deserialize a JSON string into this model
                    pub fn from_json(json_str: String) -> Result<Self, String> {
                        serde_json::from_str(&json_str)
                            .map_err(|e| incan_stdlib::errors::json_decode_error_string(e))
                    }
                });
            }
        }

        // @derive(Validate): generate `TypeName::new(...) -> Result[TypeName, E]` that calls `validate()`.
        //
        // The typechecker injects the method signature; the backend generates the actual Rust implementation here.
        if impl_block.trait_name.is_none()
            && let Some(derives) = self.struct_derives.get(&impl_block.target_type)
        {
            let has_validate = derives
                .iter()
                .any(|d| derives::from_str(d.as_str()) == Some(DeriveId::Validate));
            if has_validate
                && !impl_block.methods.iter().any(|m| m.name == conventions::NEW_METHOD)
                && let Some(validate_fn) = impl_block
                    .methods
                    .iter()
                    .find(|m| m.name == conventions::VALIDATE_METHOD)
            {
                let ret_ty = self.emit_type(&validate_fn.return_type);

                // Required fields are those without defaults; defaults are in `struct_field_defaults`.
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

        let main_impl = if !regular_methods.is_empty() || impl_block.trait_name.is_none() {
            if let Some(trait_name) = &impl_block.trait_name {
                let trait_methods: Vec<TokenStream> = impl_block
                    .methods
                    .iter()
                    .filter(|m| !matches!(m.name.as_str(), "__eq__" | "__str__" | "__class_name__" | "__fields__"))
                    .map(|m| self.emit_trait_method(m))
                    .collect::<Result<_, _>>()?;
                let trait_ident = format_ident!("{}", trait_name);
                quote! {
                    impl #trait_ident for #target_type {
                        #(#trait_methods)*
                    }
                }
            } else if !regular_methods.is_empty() {
                quote! {
                    impl #target_type {
                        #(#regular_methods)*
                    }
                }
            } else {
                quote! {}
            }
        } else if let Some(trait_name) = &impl_block.trait_name {
            let trait_ident = format_ident!("{}", trait_name);
            quote! {
                impl #trait_ident for #target_type {}
            }
        } else {
            quote! {}
        };

        Ok(quote! {
            #main_impl
            #(#trait_impls)*
        })
    }

    fn emit_method(&self, func: &super::super::decl::IrFunction) -> Result<TokenStream, EmitError> {
        let name = format_ident!("{}", &func.name);
        let vis = self.emit_visibility(&func.visibility);
        let mutated_params = self.collect_mutated_params(func);

        let params: Vec<TokenStream> = func
            .params
            .iter()
            .map(|p| {
                if p.is_self {
                    match p.mutability {
                        super::super::types::Mutability::Mutable => quote! { &mut self },
                        super::super::types::Mutability::Immutable => quote! { &self },
                    }
                } else {
                    let pname = format_ident!("{}", &p.name);
                    let pty = self.emit_type(&p.ty);
                    let needs_mut = mutated_params.contains(&p.name)
                        || matches!(p.mutability, super::super::types::Mutability::Mutable);
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

        *self.current_function_return_type.borrow_mut() = Some(func.return_type.clone());
        let body_stmts: Vec<TokenStream> = func.body.iter().map(|s| self.emit_stmt(s)).collect::<Result<_, _>>()?;
        *self.current_function_return_type.borrow_mut() = None;

        Ok(quote! {
            #vis fn #name(#(#params),*) #ret {
                #(#body_stmts)*
            }
        })
    }

    fn emit_function(&self, func: &super::super::decl::IrFunction) -> Result<TokenStream, EmitError> {
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
                let pname = format_ident!("{}", Self::escape_keyword(&p.name));
                let pty = self.emit_type(&p.ty);
                if p.is_self {
                    if matches!(p.mutability, super::super::types::Mutability::Mutable) {
                        quote! { &mut self }
                    } else {
                        quote! { &self }
                    }
                } else if mutated_params.contains(&p.name)
                    || matches!(p.mutability, super::super::types::Mutability::Mutable)
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

        let web_stmt = if is_main && self.needs_axum && !self.routes.is_empty() {
            quote! {
                let __router = __incan_web_router();
                incan_stdlib::web::set_router(__router);
            }
        } else {
            quote! {}
        };

        let tokio_main_attr = if is_main && func.is_async && self.needs_tokio {
            quote! { #[incan_stdlib::__private::tokio::main] }
        } else {
            quote! {}
        };

        let ret_ty_is_unit = matches!(func.return_type, IrType::Unit);
        if is_main || ret_ty_is_unit {
            Ok(quote! {
                #tokio_main_attr
                #vis #async_kw fn #name(#(#params),*) {
                    #zen_stmt
                    #web_stmt
                    #(#body_stmts)*
                }
            })
        } else {
            let ret_ty = self.emit_type(&func.return_type);
            Ok(quote! {
                #tokio_main_attr
                #vis #async_kw fn #name(#(#params),*) -> #ret_ty {
                    #(#body_stmts)*
                }
            })
        }
    }

    fn emit_struct(&self, s: &super::super::decl::IrStruct) -> Result<TokenStream, EmitError> {
        let name = format_ident!("{}", Self::escape_keyword(&s.name));
        let vis = self.emit_visibility(&s.visibility);

        let derives: Vec<TokenStream> = s
            .derives
            .iter()
            // `Validate` is an Incan semantic derive (not a Rust derive macro).
            .filter(|d| derives::from_str(d.as_str()) != Some(DeriveId::Validate))
            .map(|d| match derives::from_str(d.as_str()) {
                Some(DeriveId::Serialize) => quote! { serde::Serialize },
                Some(DeriveId::Deserialize) => quote! { serde::Deserialize },
                _ => {
                    let d_ident = format_ident!("{}", d);
                    quote! { #d_ident }
                }
            })
            .collect();

        let derive_attr = if derives.is_empty() {
            quote! {}
        } else {
            quote! { #[derive(#(#derives),*)] }
        };

        let has_serde = s.derives.iter().any(|d| {
            matches!(
                derives::from_str(d.as_str()),
                Some(DeriveId::Serialize) | Some(DeriveId::Deserialize)
            )
        });

        let is_tuple_struct =
            !s.fields.is_empty() && s.fields.iter().all(|f| f.name.chars().all(|c| c.is_ascii_digit()));

        if is_tuple_struct {
            let tuple_fields: Vec<TokenStream> = s
                .fields
                .iter()
                .map(|f| {
                    let fty = self.emit_type(&f.ty);
                    let fvis = self.emit_visibility(&f.visibility);
                    quote! { #fvis #fty }
                })
                .collect();
            Ok(quote! {
                #derive_attr
                #vis struct #name(#(#tuple_fields),*);
            })
        } else {
            let fields: Vec<TokenStream> = s
                .fields
                .iter()
                .map(|f| {
                    let fname = format_ident!("{}", &f.name);
                    let fty = self.emit_type(&f.ty);
                    let fvis = self.emit_visibility(&f.visibility);
                    let serde_attr = if has_serde {
                        f.alias
                            .as_ref()
                            .map(|alias| quote! { #[serde(rename = #alias)] })
                            .unwrap_or_else(|| quote! {})
                    } else {
                        quote! {}
                    };
                    quote! { #serde_attr #fvis #fname: #fty }
                })
                .collect();

            let constructor = if !s.fields.is_empty() {
                let param_tokens: Vec<TokenStream> = s
                    .fields
                    .iter()
                    .map(|f| {
                        let fname = format_ident!("{}", &f.name);
                        let fty = self.emit_type(&f.ty);
                        quote! { #fname: #fty }
                    })
                    .collect();
                let field_assigns: Vec<TokenStream> = s
                    .fields
                    .iter()
                    .map(|f| {
                        let fname = format_ident!("{}", &f.name);
                        quote! { #fname }
                    })
                    .collect();

                quote! {
                    #[allow(non_snake_case, clippy::too_many_arguments)]
                    #vis fn #name(#(#param_tokens),*) -> #name {
                        #name {
                            #(#field_assigns),*
                        }
                    }
                }
            } else {
                quote! {}
            };

            Ok(quote! {
                #derive_attr
                #vis struct #name {
                    #(#fields),*
                }

                #constructor
            })
        }
    }

    fn emit_enum(&self, e: &super::super::decl::IrEnum) -> Result<TokenStream, EmitError> {
        let name = format_ident!("{}", &e.name);
        let vis = self.emit_visibility(&e.visibility);

        let variants: Vec<TokenStream> = e
            .variants
            .iter()
            .map(|v| {
                let vname = format_ident!("{}", &v.name);
                match &v.fields {
                    super::super::decl::VariantFields::Unit => quote! { #vname },
                    super::super::decl::VariantFields::Tuple(types) => {
                        let type_tokens: Vec<_> = types.iter().map(|t| self.emit_type(t)).collect();
                        quote! { #vname(#(#type_tokens),*) }
                    }
                    super::super::decl::VariantFields::Struct(fields) => {
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
            .map(|d| match derives::from_str(d.as_str()) {
                Some(DeriveId::Serialize) => quote! { serde::Serialize },
                Some(DeriveId::Deserialize) => quote! { serde::Deserialize },
                _ => {
                    let d_ident = format_ident!("{}", d);
                    quote! { #d_ident }
                }
            })
            .collect();

        let derive_attr = if derives.is_empty() {
            quote! {}
        } else {
            quote! { #[derive(#(#derives),*)] }
        };

        let variant_match_arms: Vec<TokenStream> = e
            .variants
            .iter()
            .map(|v| {
                let vname = format_ident!("{}", &v.name);
                let vname_str = &v.name;
                match &v.fields {
                    super::super::decl::VariantFields::Unit => {
                        quote! { Self::#vname => #vname_str.to_string() }
                    }
                    super::super::decl::VariantFields::Tuple(types) => {
                        let wildcards: Vec<_> = (0..types.len()).map(|_| quote! { _ }).collect();
                        quote! { Self::#vname(#(#wildcards),*) => #vname_str.to_string() }
                    }
                    super::super::decl::VariantFields::Struct(_) => {
                        quote! { Self::#vname { .. } => #vname_str.to_string() }
                    }
                }
            })
            .collect();

        Ok(quote! {
            #derive_attr
            #vis enum #name {
                #(#variants),*
            }

            impl #name {
                pub fn message(&self) -> String {
                    match self {
                        #(#variant_match_arms),*
                    }
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::ZEN_TEXT;

    #[test]
    fn zen_text_contains_one_obvious_way_once() {
        let count = ZEN_TEXT.matches("One obvious way").count();
        assert_eq!(
            count, 1,
            "Zen text should contain 'One obvious way' once, found {}",
            count
        );
    }
}
