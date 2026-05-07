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

use incan_core::lang::surface::result_methods::ResultMethodId;
use incan_core::lang::{conventions, magic_methods};

use super::super::decl::{
    IrDeclKind, IrFunction, IrImportOrigin, IrImportQualifier, IrTraitBound, IrTypeParam, Visibility,
};
use super::super::expr::{IrDictEntry, IrExprKind, IrGeneratorClause, IrListEntry, MethodKind, Pattern, VarRefKind};
use super::super::stmt::AssignTarget;
use super::super::types::{IR_UNION_TYPE_NAME, IrType};
use super::super::{FunctionRegistry, FunctionSignature, IrDecl, IrProgram, IrStmt, IrStmtKind, TypedExpr};
use super::{EmitError, GeneratedUseAnalysis, IrEmitter};

/// Import tracking for warning-free codegen.
#[derive(Default)]
struct ImportTracker {
    needs_hashmap: bool,
    needs_hashset: bool,
}

impl ImportTracker {
    /// Scan emitted declarations for standard collection support imports.
    fn scan_decls(&mut self, declarations: &[&IrDecl]) {
        for decl in declarations {
            self.scan_decl(decl);
        }
    }

    fn scan_decl(&mut self, decl: &IrDecl) {
        match &decl.kind {
            IrDeclKind::Function(f) => self.scan_function(f),
            IrDeclKind::Static { value, .. } => self.scan_expr(value),
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

    /// Scan one statement for generated Rust dependencies needed by import tracking.
    fn scan_stmt(&mut self, stmt: &IrStmt) {
        match &stmt.kind {
            IrStmtKind::Let { value, .. } => self.scan_expr(value),
            IrStmtKind::Expr(e) | IrStmtKind::Return(Some(e)) | IrStmtKind::Yield(e) => self.scan_expr(e),
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

    /// Scan an IR expression tree for emitted support imports such as `HashMap` and `HashSet`.
    fn scan_expr(&mut self, expr: &TypedExpr) {
        match &expr.kind {
            IrExprKind::Dict(pairs) => {
                self.needs_hashmap = true;
                for entry in pairs {
                    match entry {
                        IrDictEntry::Pair(k, v) => {
                            self.scan_expr(k);
                            self.scan_expr(v);
                        }
                        IrDictEntry::Spread(value) => self.scan_expr(value),
                    }
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
                    match item {
                        IrListEntry::Element(value) | IrListEntry::Spread(value) => self.scan_expr(value),
                    }
                }
            }
            IrExprKind::Call { func, args, .. } => {
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
            IrExprKind::Generator { element, clauses } => {
                self.scan_expr(element);
                for clause in clauses {
                    match clause {
                        IrGeneratorClause::For { iterable, .. } => self.scan_expr(iterable),
                        IrGeneratorClause::If(condition) => self.scan_expr(condition),
                    }
                }
            }
            IrExprKind::InteropCoerce { expr, .. } => self.scan_expr(expr),
            _ => {}
        }
    }
}

/// Builder for generated Rust item/import usage facts.
///
/// This walks the typed IR before token emission so the backend can emit only Rust items that are reachable from the
/// generated entrypoints/public surface and can avoid generated `unused_imports`/`dead_code` suppressions.
struct GeneratedUseAnalyzer<'program> {
    declarations_by_name: HashMap<String, &'program IrDecl>,
    function_registry: &'program FunctionRegistry,
    impls_by_target: HashMap<String, Vec<&'program super::super::decl::IrImpl>>,
    rust_extension_trait_imports_by_method: HashMap<String, Vec<String>>,
    external_error_trait_types: HashSet<String>,
    preserve_public_items: bool,
    analysis: GeneratedUseAnalysis,
    pending: Vec<String>,
}

impl<'program> GeneratedUseAnalyzer<'program> {
    /// Analyze one lowered IR program for generated Rust usage facts.
    fn analyze(
        program: &'program IrProgram,
        externally_reachable_items: &HashSet<String>,
        preserve_public_items: bool,
        external_error_trait_types: &HashSet<String>,
    ) -> GeneratedUseAnalysis {
        let mut analyzer = Self {
            declarations_by_name: HashMap::new(),
            function_registry: &program.function_registry,
            impls_by_target: HashMap::new(),
            rust_extension_trait_imports_by_method: HashMap::new(),
            external_error_trait_types: external_error_trait_types.clone(),
            preserve_public_items,
            analysis: GeneratedUseAnalysis::default(),
            pending: Vec::new(),
        };

        for decl in &program.declarations {
            match &decl.kind {
                IrDeclKind::Function(func) => {
                    analyzer.declarations_by_name.insert(func.name.clone(), decl);
                }
                IrDeclKind::Struct(s) => {
                    analyzer.declarations_by_name.insert(s.name.clone(), decl);
                    if preserve_public_items && !matches!(s.visibility, Visibility::Private) {
                        analyzer.analysis.public_types.insert(s.name.clone());
                    }
                }
                IrDeclKind::Enum(e) => {
                    analyzer.declarations_by_name.insert(e.name.clone(), decl);
                    if preserve_public_items && !matches!(e.visibility, Visibility::Private) {
                        analyzer.analysis.public_types.insert(e.name.clone());
                    }
                }
                IrDeclKind::Trait(trait_decl) => {
                    analyzer.declarations_by_name.insert(trait_decl.name.clone(), decl);
                    if preserve_public_items && !matches!(trait_decl.visibility, Visibility::Private) {
                        analyzer.analysis.public_types.insert(trait_decl.name.clone());
                    }
                }
                IrDeclKind::TypeAlias { name, visibility, .. } => {
                    analyzer.declarations_by_name.insert(name.clone(), decl);
                    if preserve_public_items && !matches!(visibility, Visibility::Private) {
                        analyzer.analysis.public_types.insert(name.clone());
                    }
                }
                IrDeclKind::SymbolAlias { name, visibility, .. } => {
                    analyzer.declarations_by_name.insert(name.clone(), decl);
                    let _ = visibility;
                }
                IrDeclKind::Const { name, .. } | IrDeclKind::Static { name, .. } => {
                    analyzer.declarations_by_name.insert(name.clone(), decl);
                }
                IrDeclKind::Import {
                    origin,
                    qualifier,
                    items,
                    ..
                } if matches!(origin, IrImportOrigin::Standard) && matches!(qualifier, IrImportQualifier::None) => {
                    for item in items {
                        if item.rust_trait_methods.is_empty() {
                            continue;
                        }
                        let binding = item.alias.as_ref().unwrap_or(&item.name).clone();
                        for method in &item.rust_trait_methods {
                            analyzer
                                .rust_extension_trait_imports_by_method
                                .entry(method.clone())
                                .or_default()
                                .push(binding.clone());
                        }
                    }
                }
                IrDeclKind::Import { .. } => {}
                IrDeclKind::Impl(impl_block) => {
                    analyzer
                        .impls_by_target
                        .entry(impl_block.target_type.clone())
                        .or_default()
                        .push(impl_block);
                }
            }
        }

        for decl in &program.declarations {
            match &decl.kind {
                IrDeclKind::Function(func) if func.name == conventions::ENTRYPOINT_NAME => {
                    analyzer.mark_reachable_item(&func.name);
                }
                IrDeclKind::Function(func)
                    if preserve_public_items && !matches!(func.visibility, Visibility::Private) =>
                {
                    analyzer.mark_reachable_item(&func.name);
                }
                IrDeclKind::Function(func)
                    if (preserve_public_items && !func.rust_attributes.is_empty()) || !func.lint_allows.is_empty() =>
                {
                    analyzer.mark_reachable_item(&func.name);
                }
                IrDeclKind::Struct(s)
                    if (preserve_public_items && !matches!(s.visibility, Visibility::Private))
                        || !s.lint_allows.is_empty() =>
                {
                    analyzer.mark_reachable_item(&s.name);
                }
                IrDeclKind::Enum(e)
                    if (preserve_public_items && !matches!(e.visibility, Visibility::Private))
                        || !e.lint_allows.is_empty() =>
                {
                    analyzer.mark_reachable_item(&e.name);
                }
                IrDeclKind::Trait(trait_decl)
                    if preserve_public_items && !matches!(trait_decl.visibility, Visibility::Private) =>
                {
                    analyzer.mark_reachable_item(&trait_decl.name);
                }
                IrDeclKind::TypeAlias { name, visibility, .. }
                    if preserve_public_items && !matches!(visibility, Visibility::Private) =>
                {
                    analyzer.mark_reachable_item(name);
                }
                IrDeclKind::SymbolAlias { name, visibility, .. }
                    if preserve_public_items && !matches!(visibility, Visibility::Private) =>
                {
                    analyzer.mark_reachable_item(name);
                }
                IrDeclKind::Const { name, visibility, .. }
                    if preserve_public_items && !matches!(visibility, Visibility::Private) =>
                {
                    analyzer.mark_reachable_item(name);
                }
                IrDeclKind::Static { name, .. } => {
                    analyzer.mark_reachable_item(name);
                }
                IrDeclKind::Import { .. } | IrDeclKind::Impl(_) | IrDeclKind::Function(_) => {}
                IrDeclKind::Struct(_)
                | IrDeclKind::Enum(_)
                | IrDeclKind::Trait(_)
                | IrDeclKind::TypeAlias { .. }
                | IrDeclKind::SymbolAlias { .. }
                | IrDeclKind::Const { .. } => {}
            }
        }

        for name in externally_reachable_items {
            analyzer.mark_reachable_item(name);
        }

        while let Some(name) = analyzer.pending.pop() {
            if let Some(decl) = analyzer.declarations_by_name.get(&name).copied() {
                analyzer.scan_decl(decl);
            }
            if let Some(impls) = analyzer.impls_by_target.get(&name).cloned() {
                for impl_block in impls {
                    analyzer.scan_impl(impl_block);
                }
            }
        }

        analyzer.analysis
    }

    /// Mark a top-level generated item or import binding as referenced by emitted Rust.
    fn mark_reachable_item(&mut self, name: &str) {
        self.analysis.used_imports.insert(name.to_string());
        if self.declarations_by_name.contains_key(name) && self.analysis.reachable_items.insert(name.to_string()) {
            self.pending.push(name.to_string());
        }
    }

    /// Scan one reachable declaration for further declaration, import, field, and method uses.
    fn scan_decl(&mut self, decl: &'program IrDecl) {
        match &decl.kind {
            IrDeclKind::Function(func) => self.scan_function(func),
            IrDeclKind::Struct(s) => {
                self.scan_type_params(&s.type_params);
                for field in &s.fields {
                    self.scan_type(&field.ty);
                    if let Some(default) = &field.default {
                        self.scan_expr(default);
                    }
                }
            }
            IrDeclKind::Enum(e) => {
                self.scan_type_params(&e.type_params);
                for variant in &e.variants {
                    match &variant.fields {
                        super::super::decl::VariantFields::Unit => {}
                        super::super::decl::VariantFields::Tuple(types) => {
                            for ty in types {
                                self.scan_type(ty);
                            }
                        }
                        super::super::decl::VariantFields::Struct(fields) => {
                            for field in fields {
                                self.scan_type(&field.ty);
                            }
                        }
                    }
                }
            }
            IrDeclKind::Trait(trait_decl) => {
                self.scan_type_params(&trait_decl.type_params);
                for (trait_path, type_args) in &trait_decl.supertraits {
                    self.mark_trait_path_binding(trait_path);
                    for ty in type_args {
                        self.scan_type(ty);
                    }
                }
                for method in &trait_decl.methods {
                    self.scan_function(method);
                }
            }
            IrDeclKind::TypeAlias { type_params, ty, .. } => {
                self.scan_type_params(type_params);
                self.scan_type(ty);
            }
            IrDeclKind::SymbolAlias { target_path, .. } => {
                if let [target] = target_path.as_slice() {
                    self.mark_reachable_item(target);
                }
            }
            IrDeclKind::Const { ty, value, .. } | IrDeclKind::Static { ty, value, .. } => {
                self.scan_type(ty);
                self.scan_expr(value);
            }
            IrDeclKind::Import { .. } => {}
            IrDeclKind::Impl(impl_block) => self.scan_impl(impl_block),
        }
    }

    /// Scan an impl block attached to a reachable nominal type.
    fn scan_impl(&mut self, impl_block: &'program super::super::decl::IrImpl) {
        self.mark_reachable_item(&impl_block.target_type);
        self.scan_type_params(&impl_block.type_params);
        if let Some(trait_name) = &impl_block.trait_name {
            self.mark_trait_path_binding(trait_name);
        }
        for type_arg in &impl_block.trait_type_args {
            self.scan_type(type_arg);
        }

        let mut scanned_methods = HashSet::new();
        loop {
            let mut progressed = false;
            for method in &impl_block.methods {
                if scanned_methods.contains(&method.name) || !self.impl_method_body_is_emitted(impl_block, method) {
                    continue;
                }
                self.scan_function(method);
                scanned_methods.insert(method.name.clone());
                progressed = true;
            }
            if !progressed {
                break;
            }
        }
    }

    /// Return whether the current generated-use facts mean this lowered impl method body will be emitted.
    fn impl_method_body_is_emitted(
        &self,
        impl_block: &'program super::super::decl::IrImpl,
        method: &IrFunction,
    ) -> bool {
        if !method.lint_allows.is_empty() || !method.rust_attributes.is_empty() {
            return true;
        }

        match magic_methods::from_str(method.name.as_str()) {
            Some(magic_methods::MagicMethodId::Eq | magic_methods::MagicMethodId::Str) => true,
            Some(magic_methods::MagicMethodId::ClassName | magic_methods::MagicMethodId::Fields) => {
                self.method_is_needed(&impl_block.target_type, method)
            }
            _ if impl_block.trait_name.is_some() => true,
            _ => self.method_is_needed(&impl_block.target_type, method),
        }
    }

    /// Mirror the emitter's method-retention predicate for generated-use analysis.
    fn method_is_needed(&self, target_type: &str, method: &IrFunction) -> bool {
        self.analysis.public_types.contains(target_type)
            || (!self.preserve_public_items
                && !matches!(method.visibility, Visibility::Private)
                && self.analysis.reachable_items.contains(target_type))
            || self
                .analysis
                .used_methods
                .contains(&(target_type.to_string(), method.name.clone()))
    }

    /// Scan a function signature, defaults, and body for generated Rust dependencies.
    fn scan_function(&mut self, func: &IrFunction) {
        self.scan_type_params(&func.type_params);
        self.scan_type(&func.return_type);
        for param in &func.params {
            self.scan_type(&param.ty);
            if let Some(default) = &param.default {
                self.scan_expr(default);
            }
        }
        for stmt in &func.body {
            self.scan_stmt(stmt);
        }
    }

    /// Scan generic parameters and their trait bounds for imports used only in Rust generic syntax.
    fn scan_type_params(&mut self, type_params: &[IrTypeParam]) {
        for type_param in type_params {
            for bound in &type_param.bounds {
                self.scan_trait_bound(bound);
            }
        }
    }

    /// Scan a Rust trait bound path plus any type arguments or associated type constraints it carries.
    fn scan_trait_bound(&mut self, bound: &IrTraitBound) {
        self.mark_trait_path_binding(&bound.trait_path);
        for ty in &bound.type_args {
            self.scan_type(ty);
        }
        for (_, ty) in &bound.assoc_types {
            self.scan_type(ty);
        }
    }

    /// Mark both a full Rust path and its final segment as used so imports can satisfy generic bounds.
    fn mark_trait_path_binding(&mut self, trait_path: &str) {
        self.mark_reachable_item(trait_path);
        if let Some(binding) = trait_path.rsplit("::").next()
            && binding != trait_path
        {
            self.mark_reachable_item(binding);
        }
    }

    /// Scan one IR statement for generated Rust dependencies.
    fn scan_stmt(&mut self, stmt: &IrStmt) {
        match &stmt.kind {
            IrStmtKind::Expr(expr) | IrStmtKind::Yield(expr) => self.scan_expr(expr),
            IrStmtKind::Let { ty, value, .. } => {
                self.scan_type(ty);
                self.scan_expr(value);
            }
            IrStmtKind::Assign { target, value } | IrStmtKind::CompoundAssign { target, value, .. } => {
                self.scan_assign_target(target);
                self.scan_expr(value);
            }
            IrStmtKind::Return(Some(expr)) => self.scan_expr(expr),
            IrStmtKind::Return(None) | IrStmtKind::Continue(_) => {}
            IrStmtKind::Break { value, .. } => {
                if let Some(expr) = value {
                    self.scan_expr(expr);
                }
            }
            IrStmtKind::While { condition, body, .. } => {
                self.scan_expr(condition);
                self.scan_stmt_list(body);
            }
            IrStmtKind::For {
                pattern,
                iterable,
                body,
                ..
            } => {
                self.scan_pattern(pattern);
                self.scan_expr(iterable);
                self.scan_stmt_list(body);
            }
            IrStmtKind::Loop { body, .. } | IrStmtKind::Block(body) => self.scan_stmt_list(body),
            IrStmtKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                self.scan_expr(condition);
                self.scan_stmt_list(then_branch);
                if let Some(branch) = else_branch {
                    self.scan_stmt_list(branch);
                }
            }
            IrStmtKind::Match { scrutinee, arms } => {
                self.scan_expr(scrutinee);
                for arm in arms {
                    self.scan_pattern(&arm.pattern);
                    if let Some(guard) = &arm.guard {
                        self.scan_expr(guard);
                    }
                    self.scan_expr(&arm.body);
                }
            }
        }
    }

    /// Scan a sequential statement slice.
    fn scan_stmt_list(&mut self, stmts: &[IrStmt]) {
        for stmt in stmts {
            self.scan_stmt(stmt);
        }
    }

    /// Scan an assignment target without treating field writes as field reads.
    fn scan_assign_target(&mut self, target: &AssignTarget) {
        match target {
            AssignTarget::Var(name) | AssignTarget::StaticBinding(name) | AssignTarget::Static(name) => {
                self.mark_reachable_item(name);
            }
            AssignTarget::Field { object, .. } => self.scan_expr(object),
            AssignTarget::Index { object, index } => {
                self.scan_expr(object);
                self.scan_expr(index);
            }
        }
    }

    /// Scan a pattern for nominal type references and nested literal expressions.
    fn scan_pattern(&mut self, pattern: &Pattern) {
        match pattern {
            Pattern::Var(_) => {}
            Pattern::Tuple(items) | Pattern::Or(items) => {
                for item in items {
                    self.scan_pattern(item);
                }
            }
            Pattern::Struct { name, fields } => {
                self.mark_reachable_item(name);
                for (_, pattern) in fields {
                    self.scan_pattern(pattern);
                }
            }
            Pattern::Enum { name, variant, fields } => {
                self.mark_reachable_item(name);
                if let Some((binding, _)) = variant.split_once("::") {
                    self.mark_reachable_item(binding);
                }
                for field in fields {
                    self.scan_pattern(field);
                }
            }
            Pattern::Literal(expr) => self.scan_expr(expr),
            Pattern::Wildcard => {}
        }
    }

    /// Scan an expression tree for generated Rust dependencies and observed field/method uses.
    fn scan_expr(&mut self, expr: &TypedExpr) {
        self.scan_type(&expr.ty);
        match &expr.kind {
            IrExprKind::Var { name, .. } | IrExprKind::StaticRead { name } | IrExprKind::StaticBinding { name } => {
                self.mark_reachable_item(name);
            }
            IrExprKind::AssociatedFunction {
                type_name,
                function_name,
            } => {
                self.mark_reachable_item(type_name);
                self.analysis
                    .used_methods
                    .insert((type_name.clone(), function_name.clone()));
                if let Some(original_name) = function_name.strip_suffix("_adapter") {
                    self.analysis
                        .used_methods
                        .insert((type_name.clone(), original_name.to_string()));
                }
            }
            IrExprKind::BinOp { left, right, .. } => {
                self.scan_expr(left);
                self.scan_expr(right);
            }
            IrExprKind::UnaryOp { operand, .. }
            | IrExprKind::Await(operand)
            | IrExprKind::Try(operand)
            | IrExprKind::InteropCoerce { expr: operand, .. }
            | IrExprKind::NumericResize { expr: operand, .. }
            | IrExprKind::Cast { expr: operand, .. } => self.scan_expr(operand),
            IrExprKind::Call {
                func,
                args,
                type_args,
                callable_signature,
                canonical_path,
            } => {
                if let IrExprKind::Var { name, .. } = &func.kind {
                    self.analysis.used_constructors.insert(name.clone());
                }
                self.record_borrowed_function_value_adapters(func, args, callable_signature.as_ref(), canonical_path);
                self.scan_expr(func);
                for ty in type_args {
                    self.scan_type(ty);
                }
                for arg in args {
                    self.scan_expr(&arg.expr);
                }
            }
            IrExprKind::BuiltinCall { args, .. } => {
                for arg in args {
                    self.scan_expr(arg);
                }
            }
            IrExprKind::MethodCall {
                receiver,
                method,
                args,
                type_args,
                ..
            } => {
                self.scan_expr(receiver);
                self.mark_rust_extension_trait_imports(receiver, method);
                self.mark_stdlib_error_trait_import(receiver, method);
                if let Some(type_name) = Self::nominal_type_name(&receiver.ty) {
                    self.analysis
                        .used_methods
                        .insert((type_name.to_string(), method.clone()));
                } else if let IrExprKind::Var {
                    name,
                    ref_kind: VarRefKind::TypeName,
                    ..
                } = &receiver.kind
                {
                    self.analysis.used_methods.insert((name.clone(), method.clone()));
                }
                for ty in type_args {
                    self.scan_type(ty);
                }
                for arg in args {
                    self.scan_expr(&arg.expr);
                }
            }
            IrExprKind::KnownMethodCall { receiver, kind, args } => {
                self.scan_expr(receiver);
                if let MethodKind::Result(kind @ (ResultMethodId::Inspect | ResultMethodId::InspectErr)) = kind {
                    self.record_result_observer_callback(*kind, &receiver.ty, args.first().map(|arg| &arg.expr));
                }
                for arg in args {
                    self.scan_expr(&arg.expr);
                }
            }
            IrExprKind::Field { object, field } => {
                self.scan_expr(object);
                if let Some(type_name) = Self::nominal_type_name(&object.ty) {
                    self.analysis.read_fields.insert((type_name.to_string(), field.clone()));
                }
            }
            IrExprKind::Index { object, index } => {
                self.scan_expr(object);
                self.scan_expr(index);
            }
            IrExprKind::Slice {
                target,
                start,
                end,
                step,
            } => {
                self.scan_expr(target);
                for expr in [start, end, step].into_iter().flatten() {
                    self.scan_expr(expr);
                }
            }
            IrExprKind::ListComp {
                element,
                iterable,
                filter,
                ..
            } => {
                self.scan_expr(iterable);
                self.scan_expr(element);
                if let Some(expr) = filter {
                    self.scan_expr(expr);
                }
            }
            IrExprKind::DictComp {
                key,
                value,
                iterable,
                filter,
                ..
            } => {
                self.scan_expr(iterable);
                self.scan_expr(key);
                self.scan_expr(value);
                if let Some(expr) = filter {
                    self.scan_expr(expr);
                }
            }
            IrExprKind::Generator { element, clauses } => {
                self.scan_expr(element);
                for clause in clauses {
                    match clause {
                        IrGeneratorClause::For { iterable, .. } => self.scan_expr(iterable),
                        IrGeneratorClause::If(condition) => self.scan_expr(condition),
                    }
                }
            }
            IrExprKind::List(items) => {
                for item in items {
                    match item {
                        IrListEntry::Element(value) | IrListEntry::Spread(value) => self.scan_expr(value),
                    }
                }
            }
            IrExprKind::Dict(items) => {
                for item in items {
                    match item {
                        IrDictEntry::Pair(key, value) => {
                            self.scan_expr(key);
                            self.scan_expr(value);
                        }
                        IrDictEntry::Spread(value) => self.scan_expr(value),
                    }
                }
            }
            IrExprKind::Set(items) | IrExprKind::Tuple(items) => {
                for item in items {
                    self.scan_expr(item);
                }
            }
            IrExprKind::Struct { name, fields } => {
                self.mark_reachable_item(name);
                for (_, expr) in fields {
                    self.scan_expr(expr);
                }
            }
            IrExprKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                self.scan_expr(condition);
                self.scan_expr(then_branch);
                if let Some(expr) = else_branch {
                    self.scan_expr(expr);
                }
            }
            IrExprKind::Match { scrutinee, arms } => {
                self.scan_expr(scrutinee);
                for arm in arms {
                    self.scan_pattern(&arm.pattern);
                    if let Some(guard) = &arm.guard {
                        self.scan_expr(guard);
                    }
                    self.scan_expr(&arm.body);
                }
            }
            IrExprKind::Closure { params, body, captures } => {
                for (_, ty) in params {
                    self.scan_type(ty);
                }
                for capture in captures {
                    self.mark_reachable_item(capture);
                }
                self.scan_expr(body);
            }
            IrExprKind::Block { stmts, value } => {
                self.scan_stmt_list(stmts);
                if let Some(expr) = value {
                    self.scan_expr(expr);
                }
            }
            IrExprKind::Loop { body } => self.scan_stmt_list(body),
            IrExprKind::Range { start, end, .. } => {
                if let Some(expr) = start {
                    self.scan_expr(expr);
                }
                if let Some(expr) = end {
                    self.scan_expr(expr);
                }
            }
            IrExprKind::Format { parts } => {
                for part in parts {
                    if let super::super::expr::FormatPart::Expr(expr) = part {
                        self.scan_expr(expr);
                    }
                }
            }
            IrExprKind::SerdeFromJson(type_name) => self.mark_reachable_item(type_name),
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
            | IrExprKind::SerdeToJson => {}
        }
    }

    /// Record non-Copy observer callbacks that need generated borrowed helper items.
    fn record_result_observer_callback(
        &mut self,
        method: ResultMethodId,
        receiver_ty: &IrType,
        callback: Option<&TypedExpr>,
    ) {
        let Some(callback) = callback else {
            return;
        };
        let Some(observed_ty) = Self::result_observed_type(method, receiver_ty, callback) else {
            return;
        };
        if observed_ty.is_copy() {
            return;
        }

        match &callback.kind {
            IrExprKind::Var {
                name,
                ref_kind: VarRefKind::Value,
                ..
            } if matches!(callback.ty, IrType::Function { .. }) => {
                self.analysis.borrowed_function_adapters.insert((name.clone(), vec![0]));
            }
            _ if !matches!(callback.ty, IrType::Function { .. }) => {
                if let Some(type_name) = callback.ty.nominal_type_name() {
                    self.analysis
                        .result_observer_callable_types
                        .insert(type_name.to_string());
                }
            }
            _ => {}
        }
    }

    /// Resolve the most precise callable signature available for adapter analysis at a call site.
    fn function_signature_for_call(
        &self,
        func: &TypedExpr,
        callable_signature: Option<&FunctionSignature>,
        canonical_path: &Option<Vec<String>>,
    ) -> Option<FunctionSignature> {
        let local_name = match &func.kind {
            IrExprKind::Var { name, .. } => Some(name.as_str()),
            _ => None,
        };
        let canonical_name = canonical_path.as_ref().and_then(|path| path.last()).map(String::as_str);
        local_name
            .and_then(|name| self.function_registry.get(name).cloned())
            .or_else(|| canonical_name.and_then(|name| self.function_registry.get(name).cloned()))
            .or_else(|| callable_signature.cloned())
            .or_else(|| match &func.ty {
                IrType::Function { params, ret } => Some(FunctionSignature {
                    params: params
                        .iter()
                        .enumerate()
                        .map(|(idx, ty)| super::super::decl::FunctionParam {
                            name: format!("__incan_arg_{idx}"),
                            ty: ty.clone(),
                            mutability: super::super::types::Mutability::Immutable,
                            is_self: false,
                            kind: crate::frontend::ast::ParamKind::Normal,
                            default: None,
                        })
                        .collect(),
                    return_type: ret.as_ref().clone(),
                }),
                _ => None,
            })
    }

    /// Record named function arguments that need private adapters for borrowed function-pointer parameters.
    fn record_borrowed_function_value_adapters(
        &mut self,
        func: &TypedExpr,
        args: &[super::super::expr::IrCallArg],
        callable_signature: Option<&FunctionSignature>,
        canonical_path: &Option<Vec<String>>,
    ) {
        let Some(signature) = self.function_signature_for_call(func, callable_signature, canonical_path) else {
            return;
        };
        for (idx, arg) in args.iter().enumerate() {
            let Some(param) = signature.params.get(idx) else {
                continue;
            };
            let IrType::Function { params, .. } = &param.ty else {
                continue;
            };
            let borrowed_indices: Vec<usize> = params
                .iter()
                .enumerate()
                .filter_map(|(param_idx, ty)| matches!(ty, IrType::Ref(_)).then_some(param_idx))
                .collect();
            if borrowed_indices.is_empty() {
                continue;
            }
            if let IrExprKind::Var {
                name,
                ref_kind: VarRefKind::Value,
                ..
            } = &arg.expr.kind
                && matches!(arg.expr.ty, IrType::Function { .. })
            {
                self.analysis
                    .borrowed_function_adapters
                    .insert((name.clone(), borrowed_indices));
            }
        }
    }

    /// Return the branch payload type observed by `inspect` or `inspect_err` during generated-use analysis.
    fn result_observed_type(method: ResultMethodId, receiver_ty: &IrType, callback: &TypedExpr) -> Option<IrType> {
        match (method, receiver_ty) {
            (ResultMethodId::Inspect, IrType::Result(ok, _)) => Some(ok.as_ref().clone()),
            (ResultMethodId::InspectErr, IrType::Result(_, err)) => Some(err.as_ref().clone()),
            (ResultMethodId::Inspect | ResultMethodId::InspectErr, _) => match &callback.ty {
                IrType::Function { params, .. } => params.first().cloned(),
                _ => None,
            },
            _ => None,
        }
    }

    /// Mark Rust trait imports that can satisfy an observed extension-method call.
    fn mark_rust_extension_trait_imports(&mut self, receiver: &TypedExpr, method: &str) {
        if !self.receiver_can_use_rust_extension_trait(receiver) {
            return;
        }
        let Some(bindings) = self.rust_extension_trait_imports_by_method.get(method).cloned() else {
            return;
        };
        self.analysis.used_extension_trait_imports.extend(bindings);
    }

    /// Mark the stdlib `Error` trait import required for Rust method lookup on imported error types.
    fn mark_stdlib_error_trait_import(&mut self, receiver: &TypedExpr, method: &str) {
        if !matches!(method, "message" | "source") {
            return;
        }
        let Some(type_name) = Self::nominal_type_name(&receiver.ty) else {
            return;
        };
        if self.external_error_trait_types.contains(type_name) {
            self.analysis.uses_stdlib_error_trait = true;
        }
    }

    /// Return whether a method receiver may depend on Rust extension-trait lookup.
    fn receiver_can_use_rust_extension_trait(&self, receiver: &TypedExpr) -> bool {
        if matches!(
            &receiver.kind,
            IrExprKind::Var {
                ref_kind: VarRefKind::ExternalName | VarRefKind::ExternalRustName | VarRefKind::TypeName,
                ..
            }
        ) {
            return false;
        }
        if matches!(receiver.ty, IrType::Unknown) {
            return true;
        }
        let Some(type_name) = Self::nominal_type_name(&receiver.ty) else {
            return false;
        };
        !self.declarations_by_name.contains_key(type_name)
    }

    /// Scan an IR type for nominal declarations or imported type names that must remain visible.
    fn scan_type(&mut self, ty: &IrType) {
        match ty {
            IrType::List(inner)
            | IrType::Set(inner)
            | IrType::Option(inner)
            | IrType::Ref(inner)
            | IrType::RefMut(inner) => self.scan_type(inner),
            IrType::Dict(key, value) | IrType::Result(key, value) => {
                self.scan_type(key);
                self.scan_type(value);
            }
            IrType::Tuple(items) => {
                for item in items {
                    self.scan_type(item);
                }
            }
            IrType::Struct(name) | IrType::Enum(name) | IrType::Trait(name) | IrType::NamedGeneric(name, _) => {
                self.mark_reachable_item(name);
                if let IrType::NamedGeneric(_, args) = ty {
                    for arg in args {
                        self.scan_type(arg);
                    }
                }
            }
            IrType::ImplTrait(bound) => {
                self.mark_reachable_item(&bound.trait_path);
                for arg in &bound.type_args {
                    self.scan_type(arg);
                }
                for (_, ty) in &bound.assoc_types {
                    self.scan_type(ty);
                }
            }
            IrType::Function { params, ret } => {
                for param in params {
                    self.scan_type(param);
                }
                self.scan_type(ret);
            }
            IrType::Unit
            | IrType::Bool
            | IrType::Int
            | IrType::Float
            | IrType::Numeric(_)
            | IrType::Decimal { .. }
            | IrType::String
            | IrType::Bytes
            | IrType::StaticStr
            | IrType::StaticBytes
            | IrType::FrozenStr
            | IrType::FrozenBytes
            | IrType::StrRef
            | IrType::Generic(_)
            | IrType::SelfType
            | IrType::Unknown => {}
        }
    }

    /// Return the nominal type name after peeling explicit reference wrappers.
    fn nominal_type_name(ty: &IrType) -> Option<&str> {
        match ty {
            IrType::Ref(inner) | IrType::RefMut(inner) => Self::nominal_type_name(inner),
            _ => ty.nominal_type_name(),
        }
    }
}

impl<'a> IrEmitter<'a> {
    /// Return the anonymous union shape needed by generated field overlay methods for a concrete struct.
    ///
    /// This mirrors `emit_field_overlay_methods_for_struct()` so the crate-level union definitions are available
    /// before generated impls are emitted. Generic field shapes are skipped because anonymous union definitions are
    /// currently monomorphic.
    fn field_overlay_value_type_from_struct(strukt: &super::super::decl::IrStruct) -> Option<IrType> {
        let mut value_types: Vec<IrType> = strukt.fields.iter().map(|field| field.ty.clone()).collect();
        if value_types.iter().any(IrType::contains_generic_parameter) {
            return None;
        }
        if value_types.is_empty() {
            return None;
        }
        value_types.sort_by_key(IrType::rust_name);
        value_types.dedup();
        if value_types.len() == 1 {
            value_types.pop()
        } else {
            Some(IrType::NamedGeneric(IR_UNION_TYPE_NAME.to_string(), value_types))
        }
    }

    /// Collect anonymous union shapes that appear inside a type.
    fn collect_union_types_from_type(ty: &IrType, out: &mut HashMap<String, IrType>) {
        if let Some(name) = ty.union_type_name() {
            out.insert(name, ty.clone());
        }

        match ty {
            IrType::List(inner)
            | IrType::Set(inner)
            | IrType::Option(inner)
            | IrType::Ref(inner)
            | IrType::RefMut(inner) => Self::collect_union_types_from_type(inner, out),
            IrType::Dict(key, value) | IrType::Result(key, value) => {
                Self::collect_union_types_from_type(key, out);
                Self::collect_union_types_from_type(value, out);
            }
            IrType::Tuple(items) | IrType::NamedGeneric(_, items) => {
                for item in items {
                    Self::collect_union_types_from_type(item, out);
                }
            }
            IrType::ImplTrait(bound) => {
                for item in &bound.type_args {
                    Self::collect_union_types_from_type(item, out);
                }
                for (_, item) in &bound.assoc_types {
                    Self::collect_union_types_from_type(item, out);
                }
            }
            IrType::Function { params, ret } => {
                for param in params {
                    Self::collect_union_types_from_type(param, out);
                }
                Self::collect_union_types_from_type(ret, out);
            }
            IrType::Unit
            | IrType::Bool
            | IrType::Int
            | IrType::Float
            | IrType::Numeric(_)
            | IrType::Decimal { .. }
            | IrType::String
            | IrType::Bytes
            | IrType::StaticStr
            | IrType::StaticBytes
            | IrType::FrozenStr
            | IrType::FrozenBytes
            | IrType::StrRef
            | IrType::Struct(_)
            | IrType::Enum(_)
            | IrType::Trait(_)
            | IrType::Generic(_)
            | IrType::SelfType
            | IrType::Unknown => {}
        }
    }

    /// Collect anonymous union shapes referenced by an expression tree.
    fn collect_union_types_from_expr(expr: &TypedExpr, out: &mut HashMap<String, IrType>) {
        Self::collect_union_types_from_type(&expr.ty, out);
        match &expr.kind {
            IrExprKind::Call { func, args, .. } => {
                Self::collect_union_types_from_expr(func, out);
                for arg in args {
                    Self::collect_union_types_from_expr(&arg.expr, out);
                }
            }
            IrExprKind::BuiltinCall { args, .. } => {
                for arg in args {
                    Self::collect_union_types_from_expr(arg, out);
                }
            }
            IrExprKind::MethodCall { receiver, args, .. } | IrExprKind::KnownMethodCall { receiver, args, .. } => {
                Self::collect_union_types_from_expr(receiver, out);
                for arg in args {
                    Self::collect_union_types_from_expr(&arg.expr, out);
                }
            }
            IrExprKind::BinOp { left, right, .. } => {
                Self::collect_union_types_from_expr(left, out);
                Self::collect_union_types_from_expr(right, out);
            }
            IrExprKind::UnaryOp { operand, .. }
            | IrExprKind::Try(operand)
            | IrExprKind::Await(operand)
            | IrExprKind::Cast { expr: operand, .. }
            | IrExprKind::NumericResize { expr: operand, .. }
            | IrExprKind::InteropCoerce { expr: operand, .. } => Self::collect_union_types_from_expr(operand, out),
            IrExprKind::Index { object, index } => {
                Self::collect_union_types_from_expr(object, out);
                Self::collect_union_types_from_expr(index, out);
            }
            IrExprKind::Slice {
                target,
                start,
                end,
                step,
            } => {
                Self::collect_union_types_from_expr(target, out);
                for part in [start, end, step].into_iter().flatten() {
                    Self::collect_union_types_from_expr(part, out);
                }
            }
            IrExprKind::Field { object, .. } => Self::collect_union_types_from_expr(object, out),
            IrExprKind::List(items) => {
                for item in items {
                    match item {
                        IrListEntry::Element(value) | IrListEntry::Spread(value) => {
                            Self::collect_union_types_from_expr(value, out);
                        }
                    }
                }
            }
            IrExprKind::Dict(entries) => {
                for entry in entries {
                    match entry {
                        IrDictEntry::Pair(key, value) => {
                            Self::collect_union_types_from_expr(key, out);
                            Self::collect_union_types_from_expr(value, out);
                        }
                        IrDictEntry::Spread(value) => Self::collect_union_types_from_expr(value, out),
                    }
                }
            }
            IrExprKind::Set(items) | IrExprKind::Tuple(items) => {
                for item in items {
                    Self::collect_union_types_from_expr(item, out);
                }
            }
            IrExprKind::Struct { fields, .. } => {
                for (_, value) in fields {
                    Self::collect_union_types_from_expr(value, out);
                }
            }
            IrExprKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                Self::collect_union_types_from_expr(condition, out);
                Self::collect_union_types_from_expr(then_branch, out);
                if let Some(else_branch) = else_branch {
                    Self::collect_union_types_from_expr(else_branch, out);
                }
            }
            IrExprKind::Match { scrutinee, arms } => {
                Self::collect_union_types_from_expr(scrutinee, out);
                for arm in arms {
                    if let Some(guard) = &arm.guard {
                        Self::collect_union_types_from_expr(guard, out);
                    }
                    Self::collect_union_types_from_expr(&arm.body, out);
                }
            }
            IrExprKind::Closure { params, body, .. } => {
                for (_, ty) in params {
                    Self::collect_union_types_from_type(ty, out);
                }
                Self::collect_union_types_from_expr(body, out);
            }
            IrExprKind::Block { stmts, value } => {
                for stmt in stmts {
                    Self::collect_union_types_from_stmt(stmt, out);
                }
                if let Some(value) = value {
                    Self::collect_union_types_from_expr(value, out);
                }
            }
            IrExprKind::Loop { body } => {
                for stmt in body {
                    Self::collect_union_types_from_stmt(stmt, out);
                }
            }
            IrExprKind::Range { start, end, .. } => {
                if let Some(start) = start {
                    Self::collect_union_types_from_expr(start, out);
                }
                if let Some(end) = end {
                    Self::collect_union_types_from_expr(end, out);
                }
            }
            IrExprKind::Format { parts } => {
                for part in parts {
                    if let super::super::expr::FormatPart::Expr(expr) = part {
                        Self::collect_union_types_from_expr(expr, out);
                    }
                }
            }
            IrExprKind::ListComp {
                element,
                iterable,
                filter,
                ..
            } => {
                Self::collect_union_types_from_expr(element, out);
                Self::collect_union_types_from_expr(iterable, out);
                if let Some(filter) = filter {
                    Self::collect_union_types_from_expr(filter, out);
                }
            }
            IrExprKind::DictComp {
                key,
                value,
                iterable,
                filter,
                ..
            } => {
                Self::collect_union_types_from_expr(key, out);
                Self::collect_union_types_from_expr(value, out);
                Self::collect_union_types_from_expr(iterable, out);
                if let Some(filter) = filter {
                    Self::collect_union_types_from_expr(filter, out);
                }
            }
            IrExprKind::Generator { element, clauses } => {
                Self::collect_union_types_from_expr(element, out);
                for clause in clauses {
                    match clause {
                        IrGeneratorClause::For { iterable, .. } => Self::collect_union_types_from_expr(iterable, out),
                        IrGeneratorClause::If(condition) => Self::collect_union_types_from_expr(condition, out),
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
            | IrExprKind::AssociatedFunction { .. }
            | IrExprKind::Var { .. }
            | IrExprKind::StaticRead { .. }
            | IrExprKind::StaticBinding { .. }
            | IrExprKind::Literal(_)
            | IrExprKind::FieldsList(_)
            | IrExprKind::SerdeToJson
            | IrExprKind::SerdeFromJson(_) => {}
        }
    }

    /// Collect anonymous union shapes referenced by a statement tree.
    fn collect_union_types_from_stmt(stmt: &IrStmt, out: &mut HashMap<String, IrType>) {
        match &stmt.kind {
            IrStmtKind::Let { ty, value, .. } => {
                Self::collect_union_types_from_type(ty, out);
                Self::collect_union_types_from_expr(value, out);
            }
            IrStmtKind::Expr(expr) | IrStmtKind::Return(Some(expr)) | IrStmtKind::Yield(expr) => {
                Self::collect_union_types_from_expr(expr, out);
            }
            IrStmtKind::Assign { value, .. } => Self::collect_union_types_from_expr(value, out),
            IrStmtKind::CompoundAssign { value, lhs_ty, .. } => {
                Self::collect_union_types_from_type(lhs_ty, out);
                Self::collect_union_types_from_expr(value, out);
            }
            IrStmtKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                Self::collect_union_types_from_expr(condition, out);
                for stmt in then_branch {
                    Self::collect_union_types_from_stmt(stmt, out);
                }
                if let Some(else_branch) = else_branch {
                    for stmt in else_branch {
                        Self::collect_union_types_from_stmt(stmt, out);
                    }
                }
            }
            IrStmtKind::While { condition, body, .. } => {
                Self::collect_union_types_from_expr(condition, out);
                for stmt in body {
                    Self::collect_union_types_from_stmt(stmt, out);
                }
            }
            IrStmtKind::For {
                pattern: _,
                iterable,
                body,
                ..
            } => {
                Self::collect_union_types_from_expr(iterable, out);
                for stmt in body {
                    Self::collect_union_types_from_stmt(stmt, out);
                }
            }
            IrStmtKind::Match { scrutinee, arms } => {
                Self::collect_union_types_from_expr(scrutinee, out);
                for arm in arms {
                    if let Some(guard) = &arm.guard {
                        Self::collect_union_types_from_expr(guard, out);
                    }
                    Self::collect_union_types_from_expr(&arm.body, out);
                }
            }
            IrStmtKind::Block(stmts) | IrStmtKind::Loop { body: stmts, .. } => {
                for stmt in stmts {
                    Self::collect_union_types_from_stmt(stmt, out);
                }
            }
            IrStmtKind::Break { value, .. } => {
                if let Some(value) = value {
                    Self::collect_union_types_from_expr(value, out);
                }
            }
            IrStmtKind::Return(None) | IrStmtKind::Continue(_) => {}
        }
    }

    /// Collect anonymous union shapes referenced by a declaration.
    fn collect_union_types_from_decl(decl: &IrDecl, out: &mut HashMap<String, IrType>) {
        match &decl.kind {
            IrDeclKind::Function(func) => {
                for param in &func.params {
                    Self::collect_union_types_from_type(&param.ty, out);
                    if let Some(default) = &param.default {
                        Self::collect_union_types_from_expr(default, out);
                    }
                }
                Self::collect_union_types_from_type(&func.return_type, out);
                for stmt in &func.body {
                    Self::collect_union_types_from_stmt(stmt, out);
                }
            }
            IrDeclKind::Struct(strukt) => {
                for field in &strukt.fields {
                    Self::collect_union_types_from_type(&field.ty, out);
                    if let Some(default) = &field.default {
                        Self::collect_union_types_from_expr(default, out);
                    }
                }
            }
            IrDeclKind::Enum(_) | IrDeclKind::Trait(_) | IrDeclKind::Import { .. } | IrDeclKind::SymbolAlias { .. } => {
            }
            IrDeclKind::TypeAlias { ty, interop_edges, .. } => {
                Self::collect_union_types_from_type(ty, out);
                for edge in interop_edges {
                    Self::collect_union_types_from_type(&edge.ty, out);
                    Self::collect_union_types_from_expr(&edge.adapter, out);
                }
            }
            IrDeclKind::Const { ty, value, .. } | IrDeclKind::Static { ty, value, .. } => {
                Self::collect_union_types_from_type(ty, out);
                Self::collect_union_types_from_expr(value, out);
            }
            IrDeclKind::Impl(impl_block) => {
                for ty in &impl_block.trait_type_args {
                    Self::collect_union_types_from_type(ty, out);
                }
                for method in &impl_block.methods {
                    for param in &method.params {
                        Self::collect_union_types_from_type(&param.ty, out);
                    }
                    Self::collect_union_types_from_type(&method.return_type, out);
                    for stmt in &method.body {
                        Self::collect_union_types_from_stmt(stmt, out);
                    }
                }
            }
        }
    }

    /// Collect anonymous ordinary union shapes referenced anywhere in a program.
    pub(crate) fn collect_union_types_from_program(program: &IrProgram) -> HashMap<String, IrType> {
        let mut union_types = HashMap::new();
        for decl in &program.declarations {
            Self::collect_union_types_from_decl(decl, &mut union_types);
        }
        union_types
    }

    /// Emit the generated Rust enum for one normalized anonymous union shape.
    fn emit_generated_union_type(&self, ty: &IrType) -> Option<TokenStream> {
        let name = ty.union_type_name()?;
        let members = ty.union_members()?;
        let name_ident = format_ident!("{}", name);
        let variants: Vec<TokenStream> = members
            .iter()
            .enumerate()
            .map(|(index, member)| {
                let variant = format_ident!("{}", IrType::union_variant_name(index));
                let member_ty = self.emit_generated_union_member_type(member);
                quote! { #variant(#member_ty) }
            })
            .collect();
        Some(quote! {
            #[derive(Clone)]
            pub enum #name_ident {
                #(#variants),*
            }
        })
    }

    /// Emit a payload type for a crate-root anonymous union definition.
    ///
    /// Shared union wrappers are emitted before ordinary `use` items in `main.rs`. When a shared wrapper is collected
    /// from a dependency module, its payloads may mention dependency-local types that the main module never imported
    /// directly. Qualify those nominal payloads through their generated module path so wrapper emission does not depend
    /// on incidental source imports.
    fn emit_generated_union_member_type(&self, ty: &IrType) -> TokenStream {
        match ty {
            IrType::Struct(name) | IrType::Enum(name) | IrType::Trait(name) => self
                .emit_dependency_nominal_type_path(name)
                .unwrap_or_else(|| self.emit_type(ty)),
            IrType::NamedGeneric(name, args) if name == super::super::types::IR_UNION_TYPE_NAME => {
                self.emit_union_type_path(ty)
            }
            IrType::NamedGeneric(name, args) => {
                let base = self.emit_dependency_nominal_type_path(name).unwrap_or_else(|| {
                    let ident = Self::rust_ident(name);
                    quote! { #ident }
                });
                let args: Vec<_> = args
                    .iter()
                    .map(|arg| self.emit_generated_union_member_type(arg))
                    .collect();
                quote! { #base < #(#args),* > }
            }
            IrType::List(inner) => {
                let inner = self.emit_generated_union_member_type(inner);
                quote! { Vec<#inner> }
            }
            IrType::Dict(key, value) => {
                let key = self.emit_generated_union_member_type(key);
                let value = self.emit_generated_union_member_type(value);
                quote! { std::collections::HashMap<#key, #value> }
            }
            IrType::Set(inner) => {
                let inner = self.emit_generated_union_member_type(inner);
                quote! { std::collections::HashSet<#inner> }
            }
            IrType::Tuple(items) => {
                let items: Vec<_> = items
                    .iter()
                    .map(|item| self.emit_generated_union_member_type(item))
                    .collect();
                quote! { (#(#items),*) }
            }
            IrType::Option(inner) => {
                let inner = self.emit_generated_union_member_type(inner);
                quote! { Option<#inner> }
            }
            IrType::Result(ok, err) => {
                let ok = self.emit_generated_union_member_type(ok);
                let err = self.emit_generated_union_member_type(err);
                quote! { Result<#ok, #err> }
            }
            IrType::Function { params, ret } => {
                let params: Vec<_> = params
                    .iter()
                    .map(|param| self.emit_generated_union_member_type(param))
                    .collect();
                let ret = self.emit_generated_union_member_type(ret);
                quote! { fn(#(#params),*) -> #ret }
            }
            IrType::Ref(inner) => {
                let inner = self.emit_generated_union_member_type(inner);
                quote! { &#inner }
            }
            IrType::RefMut(inner) => {
                let inner = self.emit_generated_union_member_type(inner);
                quote! { &mut #inner }
            }
            IrType::Unit
            | IrType::Bool
            | IrType::Int
            | IrType::Float
            | IrType::Numeric(_)
            | IrType::Decimal { .. }
            | IrType::String
            | IrType::Bytes
            | IrType::StaticStr
            | IrType::StaticBytes
            | IrType::FrozenStr
            | IrType::FrozenBytes
            | IrType::StrRef
            | IrType::ImplTrait(_)
            | IrType::Generic(_)
            | IrType::SelfType
            | IrType::Unknown => self.emit_type(ty),
        }
    }

    /// Emit a crate-qualified path for an unambiguous nominal type declared in a dependency module.
    fn emit_dependency_nominal_type_path(&self, name: &str) -> Option<TokenStream> {
        if name.contains("::") || self.ambiguous_type_names.contains(name) {
            return None;
        }
        let module_path = self.type_module_paths.get(name)?;
        let mut segments = vec![quote! { crate }];
        for segment in module_path {
            let ident = Self::rust_ident(segment);
            segments.push(quote! { #ident });
        }
        let name_ident = Self::rust_ident(name);
        segments.push(quote! { #name_ident });

        let mut iter = segments.into_iter();
        let first = iter.next()?;
        Some(iter.fold(first, |acc, segment| quote! { #acc :: #segment }))
    }

    /// Emit a complete IR program to formatted Rust code.
    #[tracing::instrument(skip_all, fields(decl_count = program.declarations.len()))]
    pub fn emit_program(&mut self, program: &IrProgram) -> Result<String, EmitError> {
        // RFC 023: propagate rust.module() path from IR to emitter for @rust.extern delegation.
        if self.rust_module_path.is_none() {
            self.rust_module_path = program.rust_module_path.clone();
        }
        self.seed_nominal_metadata_from_program(program);
        self.newtype_checked_ctor = program.newtype_checked_ctor.clone();

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
                    self.struct_field_visibilities
                        .insert((s.name.clone(), field.name.clone()), field.visibility);
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
            if let IrDeclKind::TypeAlias {
                name,
                is_rusttype: true,
                ..
            } = &decl.kind
            {
                self.rusttype_alias_names.insert(name.clone());
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

        // Find the end of the inner attribute block and insert marker after it. Normal generated Rust no longer emits
        // inner lint attributes, so files without an attribute block place the marker before the first Rust item.
        let with_marker = if !formatted.starts_with("#![") {
            format!("// __INCAN_INSERT_MODS__\n\n{formatted}")
        } else if formatted.contains("]\nuse ") {
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
        let analysis = GeneratedUseAnalyzer::analyze(
            program,
            &self.externally_reachable_items,
            self.preserve_public_items,
            &self.external_error_trait_types,
        );
        let uses_stdlib_error_trait = analysis.uses_stdlib_error_trait;
        let result_observer_callable_types = analysis.result_observer_callable_types.clone();
        let borrowed_function_adapters = analysis.borrowed_function_adapters.clone();
        self.set_result_observer_callable_types(result_observer_callable_types);
        self.set_borrowed_function_adapters(borrowed_function_adapters);
        self.set_generated_use_analysis(analysis);

        let emitted_declarations: Vec<&IrDecl> = program
            .declarations
            .iter()
            .filter(|decl| self.should_emit_decl(decl))
            .collect();
        let static_names: Vec<String> = emitted_declarations
            .iter()
            .filter_map(|decl| match &decl.kind {
                IrDeclKind::Static { name, .. } => Some(name.clone()),
                _ => None,
            })
            .collect();
        *self.module_has_local_statics.borrow_mut() = !static_names.is_empty();

        if self.emit_strict_generated_lint_denies {
            items.push(quote! {
                #![deny(unused_imports, dead_code, unused_variables)]
            });
        }

        let mut tracker = ImportTracker::default();
        tracker.scan_decls(&emitted_declarations);

        let compiler_version = crate::version::INCAN_VERSION;
        items.push(quote! { incan_stdlib::__incan_stdlib_version_check!(#compiler_version); });

        match (tracker.needs_hashmap, tracker.needs_hashset) {
            (true, true) => items.push(quote! { use std::collections::{HashMap, HashSet}; }),
            (true, false) => items.push(quote! { use std::collections::HashMap; }),
            (false, true) => items.push(quote! { use std::collections::HashSet; }),
            (false, false) => {}
        }
        if uses_stdlib_error_trait {
            let std_namespace = Self::rust_ident(incan_core::lang::stdlib::INCAN_STD_NAMESPACE);
            items.push(quote! { use crate::#std_namespace::traits::error::Error; });
        }
        let needs_json_serialize_trait_scope = emitted_declarations.iter().any(|decl| {
            matches!(
                &decl.kind,
                IrDeclKind::Impl(impl_block)
                    if impl_block.trait_name.as_deref() == Some("json.Serialize")
                        || impl_block.trait_name.as_deref() == Some("std.serde.json.Serialize")
            )
        });
        let needs_json_deserialize_trait_scope = emitted_declarations.iter().any(|decl| {
            matches!(
                &decl.kind,
                IrDeclKind::Impl(impl_block)
                    if impl_block.trait_name.as_deref() == Some("json.Deserialize")
                        || impl_block.trait_name.as_deref() == Some("std.serde.json.Deserialize")
            )
        });
        match (needs_json_serialize_trait_scope, needs_json_deserialize_trait_scope) {
            (true, true) => items.push(quote! { use json::{Deserialize as _, Serialize as _}; }),
            (true, false) => items.push(quote! { use json::Serialize as _; }),
            (false, true) => items.push(quote! { use json::Deserialize as _; }),
            (false, false) => {}
        }

        let mut explicit_methods_by_type: HashMap<String, HashSet<String>> = HashMap::new();
        for decl in &emitted_declarations {
            if let IrDeclKind::Impl(impl_block) = &decl.kind
                && impl_block.trait_name.is_none()
            {
                explicit_methods_by_type
                    .entry(impl_block.target_type.clone())
                    .or_default()
                    .extend(impl_block.methods.iter().map(|method| method.name.clone()));
            }
        }

        if self.emit_generated_union_definitions {
            let mut union_types = self.generated_union_types.clone();
            for decl in &emitted_declarations {
                Self::collect_union_types_from_decl(decl, &mut union_types);
            }
            let field_value_name = magic_methods::as_str(magic_methods::MagicMethodId::FieldValue);
            let field_items_name = magic_methods::as_str(magic_methods::MagicMethodId::FieldItems);
            let empty_methods = HashSet::new();
            let used_methods = &self.generated_use_analysis.borrow().used_methods;
            for decl in &emitted_declarations {
                if let IrDeclKind::Struct(strukt) = &decl.kind {
                    let explicit_methods = explicit_methods_by_type.get(&strukt.name).unwrap_or(&empty_methods);
                    let needs_field_value = !explicit_methods.contains(field_value_name)
                        && used_methods.contains(&(strukt.name.clone(), field_value_name.to_string()));
                    let needs_field_items = !explicit_methods.contains(field_items_name)
                        && used_methods.contains(&(strukt.name.clone(), field_items_name.to_string()));
                    if (needs_field_value || needs_field_items)
                        && let Some(value_ty) = Self::field_overlay_value_type_from_struct(strukt)
                    {
                        Self::collect_union_types_from_type(&value_ty, &mut union_types);
                    }
                }
            }
            let mut union_type_items: Vec<_> = union_types.into_iter().collect();
            union_type_items.sort_by(|(left, _), (right, _)| left.cmp(right));
            for (_, union_ty) in union_type_items {
                if let Some(item) = self.emit_generated_union_type(&union_ty) {
                    items.push(item);
                }
            }
        }

        // RFC 052: force declaration-order static initialization once per module before any static access helper call.
        if !static_names.is_empty() {
            let force_calls: Vec<TokenStream> = static_names
                .iter()
                .map(|name| {
                    let ident = Self::rust_static_ident(name);
                    quote! { std::sync::LazyLock::force(&#ident); }
                })
                .collect();
            items.push(quote! {
                #[inline(always)]
                fn __incan_init_module_statics() {
                    static __INCAN_STATIC_INIT_RUNNING: std::sync::atomic::AtomicBool =
                        std::sync::atomic::AtomicBool::new(false);
                    if __INCAN_STATIC_INIT_RUNNING.load(std::sync::atomic::Ordering::Acquire) {
                        return;
                    }
                    static __INCAN_STATIC_INIT_ONCE: std::sync::OnceLock<()> = std::sync::OnceLock::new();
                    __INCAN_STATIC_INIT_ONCE.get_or_init(|| {
                        struct __IncanStaticInitGuard<'a>(&'a std::sync::atomic::AtomicBool);
                        impl Drop for __IncanStaticInitGuard<'_> {
                            fn drop(&mut self) {
                                self.0.store(false, std::sync::atomic::Ordering::Release);
                            }
                        }
                        __INCAN_STATIC_INIT_RUNNING.store(true, std::sync::atomic::Ordering::Release);
                        let _guard = __IncanStaticInitGuard(&__INCAN_STATIC_INIT_RUNNING);
                        #(#force_calls)*
                    });
                }
            });
        }

        // Emit all declarations.
        let mut decl_items = Vec::new();
        for decl in &emitted_declarations {
            decl_items.push(self.emit_decl(decl)?);
            if let IrDeclKind::Function(func) = &decl.kind {
                let adapters = self.borrowed_function_adapters.borrow();
                let mut matching_adapters: Vec<Vec<usize>> = adapters
                    .iter()
                    .filter_map(|(name, indices)| (name == &func.name).then_some(indices.clone()))
                    .collect();
                drop(adapters);
                matching_adapters.sort();
                for indices in matching_adapters {
                    if let Some(helper) = self.emit_borrowed_function_adapter(func, &indices)? {
                        decl_items.push(helper);
                    }
                }
            }
        }
        let empty_methods = HashSet::new();
        for decl in emitted_declarations {
            if let IrDeclKind::Struct(strukt) = &decl.kind
                && let Some(overlay_impl) = self.emit_field_overlay_methods_for_struct(
                    strukt,
                    explicit_methods_by_type.get(&strukt.name).unwrap_or(&empty_methods),
                )?
            {
                decl_items.push(overlay_impl);
            }
        }

        // Add the declarations after imports
        items.extend(decl_items);

        Ok(quote! {
            #(#items)*
        })
    }

    /// Return whether a lowered declaration should be emitted after generated-use analysis.
    fn should_emit_decl(&self, decl: &IrDecl) -> bool {
        match &decl.kind {
            IrDeclKind::Function(func) => self.should_emit_decl_name(&func.name, &func.visibility),
            IrDeclKind::Struct(s) => self.should_emit_decl_name(&s.name, &s.visibility),
            IrDeclKind::Enum(e) => self.should_emit_decl_name(&e.name, &e.visibility),
            IrDeclKind::Trait(trait_decl) => self.should_emit_decl_name(&trait_decl.name, &trait_decl.visibility),
            IrDeclKind::TypeAlias { name, visibility, .. } => self.should_emit_decl_name(name, visibility),
            IrDeclKind::SymbolAlias { name, visibility, .. } => self.should_emit_decl_name(name, visibility),
            IrDeclKind::Const { name, visibility, .. } => self.should_emit_decl_name(name, visibility),
            IrDeclKind::Static { name, visibility, .. } => self.should_emit_decl_name(name, visibility),
            IrDeclKind::Import { .. } => true,
            IrDeclKind::Impl(impl_block) => self
                .generated_use_analysis
                .borrow()
                .reachable_items
                .contains(&impl_block.target_type),
        }
    }
}
