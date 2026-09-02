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
use std::collections::{BTreeMap, HashMap, HashSet};

use crate::frontend::ast::TypeConstraintKey;
use crate::frontend::symbols::{NewtypePrimitiveConstraint, overload_emitted_name_prefix};
use crate::provider::SDK_PROVIDER_BUILD_ENV;
use incan_core::lang::c_abi::{LinkCapabilityId, ScalarTypeId};
use incan_core::lang::surface::result_methods::ResultMethodId;
use incan_core::lang::types::numerics::{self, NumericFamily};
use incan_core::lang::{conventions, keywords, magic_methods, stdlib as core_stdlib, trait_capabilities};

use super::super::decl::{
    FunctionParamDefault, IrDeclKind, IrEnum, IrEnumValue, IrEnumValueType, IrFunction, IrImportOrigin,
    IrImportQualifier, IrRustTraitImport, IrStaticProvenance, IrTraitBound, IrTypeParam, Visibility,
};
use super::super::expr::{
    IrCallArg, IrDictEntry, IrExprKind, IrGeneratorClause, IrListEntry, IrMethodDispatch, MethodKind, Pattern,
    VarRefKind,
};
use super::super::stmt::AssignTarget;
use super::super::types::{IR_UNION_TYPE_NAME, IrType};
use super::super::{
    FunctionRegistry, FunctionSignature, IrCheckedCFunction, IrCheckedCResource, IrCheckedCType, IrDecl, IrProgram,
    IrStmt, IrStmtKind, TypedExpr,
};
use super::{CallableNameUseFacts, EmitError, GeneratedUseAnalysis, IrEmitter, SERDE_DESERIALIZE_DERIVE};

struct OrdinalValueEnumBridgeSpec {
    type_path: TokenStream,
    display_name: String,
    encoding: String,
    value_type: IrEnumValueType,
    trait_path: TokenStream,
    error_path: TokenStream,
}

/// Render the one checked native-link declaration retained from the C binding descriptor.
///
/// Framework linkage is not a linker-search fallback. The checked descriptor owns its framework link while the
/// locked Oven target plan owns its explicit toolchain and SDK requirements. A later packaging adapter must validate
/// their declared correspondence before producing a target package; this emitter does not invent that correspondence.
fn checked_c_link_attribute(library: &str, capability: LinkCapabilityId) -> TokenStream {
    match capability {
        LinkCapabilityId::SystemLibrary => quote! { #[link(name = #library)] },
        LinkCapabilityId::Framework => quote! { #[link(name = #library, kind = "framework")] },
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
    rust_extension_trait_imports: HashMap<String, IrRustTraitImport>,
    preserve_public_items: bool,
    variable_types: HashMap<String, IrType>,
    current_impl_target: Option<String>,
    struct_field_aliases: HashMap<(String, String), String>,
    analysis: GeneratedUseAnalysis,
    pending: Vec<String>,
}

impl<'program> GeneratedUseAnalyzer<'program> {
    /// Analyze one lowered IR program for generated Rust usage facts.
    fn analyze(
        program: &'program IrProgram,
        externally_reachable_items: &HashSet<String>,
        preserve_public_items: bool,
    ) -> GeneratedUseAnalysis {
        let mut analyzer = Self {
            declarations_by_name: HashMap::new(),
            function_registry: &program.function_registry,
            impls_by_target: HashMap::new(),
            rust_extension_trait_imports: HashMap::new(),
            preserve_public_items,
            variable_types: HashMap::new(),
            current_impl_target: None,
            struct_field_aliases: HashMap::new(),
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
                    for field in &s.fields {
                        if let Some(alias) = &field.alias
                            && alias != &field.name
                        {
                            analyzer
                                .struct_field_aliases
                                .insert((s.name.clone(), alias.clone()), field.name.clone());
                        }
                    }
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
                        let Some(import) = &item.rust_trait_import else {
                            continue;
                        };
                        let binding = item.emitted_binding_name();
                        analyzer.rust_extension_trait_imports.insert(binding, import.clone());
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

        // Checked Serde deserialization is compiler-generated rather than visible in the source IR. Retain and scan
        // the canonical validation hook exactly as if the generated implementation had called it in source.
        for (name, plan) in &program.newtype_construction {
            let Some(constructor) = &plan.checked_constructor else {
                continue;
            };
            let derives_deserialize = analyzer.declarations_by_name.get(name).is_some_and(|decl| {
                matches!(
                    &decl.kind,
                    IrDeclKind::Struct(strukt)
                        if strukt.derives.iter().any(|derive| derive == SERDE_DESERIALIZE_DERIVE)
                )
            });
            if derives_deserialize {
                analyzer
                    .analysis
                    .used_methods
                    .insert((name.clone(), constructor.clone()));
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
            analyzer.mark_reachable_overload_items(name);
        }

        analyzer.scan_stmt_list(&program.module_init);

        while let Some(name) = analyzer.pending.pop() {
            if let Some(decl) = analyzer.declarations_by_name.get(&name).copied() {
                analyzer.scan_decl(decl);
            }
            if let Some(impls) = analyzer.impls_by_target.get(&name).cloned() {
                analyzer.scan_impl_blocks(impls.as_slice());
            }
        }

        analyzer.analysis
    }

    /// Mark a top-level generated item or import binding as referenced by emitted Rust.
    fn mark_reachable_item(&mut self, name: &str) {
        self.analysis.used_imports.insert(name.to_string());
        let declaration_name = self.function_registry.registry_key(name);
        if self.declarations_by_name.contains_key(declaration_name)
            && self.analysis.reachable_items.insert(declaration_name.to_string())
        {
            self.pending.push(declaration_name.to_string());
        }
    }

    /// Mark a top-level generated type declaration as semantically reachable without retaining a Rust `use` binding.
    ///
    /// Type annotations keep local declarations alive. Imported type names still need their Rust `use` binding because
    /// the current type emitter prints their local binding name in signatures.
    fn mark_reachable_type(&mut self, name: &str) {
        if self.declarations_by_name.contains_key(name) {
            if self.analysis.reachable_items.insert(name.to_string()) {
                self.pending.push(name.to_string());
            }
        } else {
            self.analysis.used_imports.insert(name.to_string());
        }
    }

    /// Mark concrete Rust implementation items for one source overload binding as reachable.
    fn mark_reachable_overload_items(&mut self, source_name: &str) {
        let prefix = overload_emitted_name_prefix(source_name);
        let overload_names = self
            .declarations_by_name
            .keys()
            .filter(|name| name.starts_with(&prefix))
            .cloned()
            .collect::<Vec<_>>();
        for overload_name in overload_names {
            self.mark_reachable_item(&overload_name);
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
        let previous_impl_target = self.current_impl_target.replace(impl_block.target_type.clone());
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
        self.current_impl_target = previous_impl_target;
    }

    /// Scan every impl for one target until helper reachability is stable across impl boundaries.
    ///
    /// Source-owned inherent helpers and imported-trait implementations lower into separate [`IrImpl`] blocks. A
    /// trait callback can therefore discover an inherent helper only after that helper's block has already been
    /// scanned once. Revisit the target's blocks when method reachability grows so the generated Rust retains the
    /// complete source-owned helper graph.
    fn scan_impl_blocks(&mut self, impl_blocks: &[&'program super::super::decl::IrImpl]) {
        loop {
            let used_method_count = self.analysis.used_methods.len();
            for impl_block in impl_blocks {
                self.scan_impl(impl_block);
            }
            if self.analysis.used_methods.len() == used_method_count {
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
                self.analysis.should_retain_method(
                    self.preserve_public_items,
                    &impl_block.target_type,
                    &method.name,
                    &method.visibility,
                )
            }
            _ if impl_block.trait_name.is_some() => true,
            _ => self.analysis.should_retain_method(
                self.preserve_public_items,
                &impl_block.target_type,
                &method.name,
                &method.visibility,
            ),
        }
    }

    /// Scan a function signature, defaults, and body for generated Rust dependencies.
    fn scan_function(&mut self, func: &IrFunction) {
        let outer_variable_types = std::mem::take(&mut self.variable_types);
        self.scan_type_params(&func.type_params);
        self.scan_type(&func.return_type);
        for param in &func.params {
            self.scan_type(&param.ty);
            if !param.is_self {
                self.variable_types.insert(param.name.clone(), param.ty.clone());
            }
            if let Some(FunctionParamDefault::Source(default)) = &param.default {
                self.scan_expr(default);
            }
        }
        for stmt in &func.body {
            self.scan_stmt(stmt);
        }
        self.variable_types = outer_variable_types;
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
            IrStmtKind::Let { name, ty, value, .. } => {
                self.scan_type(ty);
                self.scan_expr(value);
                let binding_ty = if matches!(ty, IrType::Unknown) {
                    Self::inferred_binding_type(value).unwrap_or_else(|| value.ty.clone())
                } else {
                    ty.clone()
                };
                self.variable_types.insert(name.clone(), binding_ty);
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
                    for binding in &arm.bindings {
                        self.scan_type(&binding.ty);
                        self.scan_expr(&binding.value);
                        if let Some(guard_value) = &binding.guard_value {
                            self.scan_expr(guard_value);
                        }
                    }
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
            AssignTarget::Var(name) | AssignTarget::StaticBinding(name) => {
                self.mark_reachable_item(name);
            }
            AssignTarget::Static { name, .. } => self.mark_reachable_item(name),
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
        self.scan_non_textual_type(&expr.ty);
        match &expr.kind {
            IrExprKind::Var { name, .. }
            | IrExprKind::StaticRead { name, .. }
            | IrExprKind::StaticBinding { name, .. } => {
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
            IrExprKind::FunctionItem { name, type_args } => {
                self.mark_reachable_item(name);
                for ty in type_args {
                    self.scan_type(ty);
                }
            }
            IrExprKind::RegisterCallableName { callable, .. } => {
                self.scan_expr(callable);
                if let IrType::Function { params, ret } = &callable.ty
                    && let Some(key) = IrEmitter::callable_name_signature_key(params, ret)
                {
                    self.analysis.callable_name_signature_keys.insert(key);
                }
            }
            IrExprKind::CacheGenericDecoratedFunction { value, .. } => {
                self.scan_expr(value);
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
                if !Self::call_emits_via_canonical_callee_path(func, canonical_path.as_deref()) {
                    self.scan_expr(func);
                }
                for ty in type_args {
                    self.scan_type(ty);
                }
                for arg in args {
                    for key in self.callable_name_function_arg_signature_keys(&arg.expr) {
                        self.analysis.callable_name_function_arg_signature_keys.insert(key);
                    }
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
                dispatch,
                ..
            } => {
                self.scan_expr(receiver);
                self.mark_rust_extension_trait_imports(receiver, method, dispatch.as_ref());
                if let Some(type_name) = self.object_nominal_type_name(receiver) {
                    self.analysis.used_methods.insert((type_name, method.clone()));
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
                if field == "__name__"
                    && let IrType::Function { params, ret } = &object.ty
                    && let Some(key) = IrEmitter::callable_name_signature_key(params, ret)
                {
                    self.analysis.callable_name_signature_keys.insert(key);
                }
                if field == "__name__" && matches!(object.ty, IrType::Generic(_)) {
                    self.analysis.uses_generic_callable_name_trait = true;
                }
                if let Some(type_name) = self.object_nominal_type_name(object) {
                    let field = self
                        .struct_field_aliases
                        .get(&(type_name.clone(), field.clone()))
                        .map(String::as_str)
                        .unwrap_or(field.as_str());
                    self.analysis.read_fields.insert((type_name, field.to_string()));
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
            IrExprKind::Struct { name, fields, .. } => {
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
                    for binding in &arm.bindings {
                        self.scan_type(&binding.ty);
                        self.scan_expr(&binding.value);
                        if let Some(guard_value) = &binding.guard_value {
                            self.scan_expr(guard_value);
                        }
                    }
                    if let Some(guard) = &arm.guard {
                        self.scan_expr(guard);
                    }
                    self.scan_expr(&arm.body);
                }
            }
            IrExprKind::Race { arms, .. } => {
                for arm in arms {
                    self.scan_expr(&arm.awaitable);
                    self.scan_expr(&arm.body);
                }
            }
            IrExprKind::Closure {
                params, body, captures, ..
            } => {
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
                    if let super::super::expr::FormatPart::Expr { expr, .. } = part {
                        self.scan_expr(expr);
                    }
                }
            }
            IrExprKind::SerdeFromJson(type_name) => self.mark_reachable_item(type_name),
            IrExprKind::TypeToken { ty } => self.scan_type(ty),
            IrExprKind::Unit
            | IrExprKind::None
            | IrExprKind::Bool(_)
            | IrExprKind::Int(_)
            | IrExprKind::IntLiteral(_)
            | IrExprKind::Float(_)
            | IrExprKind::Decimal(_)
            | IrExprKind::String(_)
            | IrExprKind::Bytes(_)
            | IrExprKind::Literal(_)
            | IrExprKind::FieldsList(_)
            | IrExprKind::SerdeToJson => {}
        }
    }

    /// Collect callable-name signature keys required by function arguments.
    fn callable_name_function_arg_signature_keys(&self, expr: &TypedExpr) -> Vec<String> {
        match &expr.kind {
            IrExprKind::Var { name, .. } => {
                let mut keys = HashSet::new();
                if let IrType::Function { params, ret } = &expr.ty
                    && let Some(key) = IrEmitter::callable_name_signature_key(params, ret)
                {
                    keys.insert(key);
                }
                if let Some(signature) = self.function_registry.get(name) {
                    let params = signature
                        .params
                        .iter()
                        .map(|param| param.ty.clone())
                        .collect::<Vec<_>>();
                    if let Some(key) = IrEmitter::callable_name_signature_key(&params, &signature.return_type) {
                        keys.insert(key);
                    }
                }
                let Some(IrDecl {
                    kind: IrDeclKind::Function(func),
                    ..
                }) = self.declarations_by_name.get(name).copied()
                else {
                    let mut keys = keys.into_iter().collect::<Vec<_>>();
                    keys.sort();
                    return keys;
                };
                if func.is_async || !func.type_params.is_empty() {
                    return Vec::new();
                }
                let params = func.params.iter().map(|param| param.ty.clone()).collect::<Vec<_>>();
                if let Some(key) = IrEmitter::callable_name_signature_key(&params, &func.return_type) {
                    keys.insert(key);
                }
                let mut keys = keys.into_iter().collect::<Vec<_>>();
                keys.sort();
                keys
            }
            IrExprKind::FunctionItem { .. } => {
                let mut keys = HashSet::new();
                if let IrType::Function { params, ret } = &expr.ty
                    && let Some(key) = IrEmitter::callable_name_signature_key(params, ret)
                {
                    keys.insert(key);
                }
                let mut keys = keys.into_iter().collect::<Vec<_>>();
                keys.sort();
                keys
            }
            IrExprKind::InteropCoerce { expr, .. }
            | IrExprKind::NumericResize { expr, .. }
            | IrExprKind::Cast { expr, .. } => self.callable_name_function_arg_signature_keys(expr),
            IrExprKind::CacheGenericDecoratedFunction { value, .. } => {
                self.callable_name_function_arg_signature_keys(value)
            }
            _ => Vec::new(),
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
        FunctionRegistry::effective_call_signature(
            self.function_registry,
            self.function_registry,
            local_name,
            canonical_path.as_deref(),
            callable_signature,
            Some(&func.ty),
        )
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

    /// Return whether call emission will use the resolved canonical path instead of the source callee expression.
    ///
    /// The use analyzer must mirror that emission choice. Otherwise imported aliases, especially dependency-provided
    /// vocab helper aliases, are retained as Rust `use` bindings even though the emitted call is already qualified
    /// through the owning crate or generated stdlib module.
    fn call_emits_via_canonical_callee_path(func: &TypedExpr, canonical_path: Option<&[String]>) -> bool {
        let Some(path) = canonical_path else {
            return false;
        };

        match path.first().map(String::as_str) {
            Some("pub") => !Self::callee_is_imported_module_path(func),
            Some(core_stdlib::STDLIB_ROOT | core_stdlib::INCAN_STD_NAMESPACE) => true,
            _ => false,
        }
    }

    /// Return whether the callee is already spelled as an imported module-rooted path in source, such as
    /// `lib.function`. Public package calls keep that source path so package module imports remain reachable.
    fn callee_is_imported_module_path(func: &TypedExpr) -> bool {
        match &func.kind {
            IrExprKind::Field { object, .. } => Self::callee_is_imported_module_path(object),
            IrExprKind::Var { ref_kind, .. } => {
                matches!(ref_kind, VarRefKind::ExternalName | VarRefKind::ExternalRustName)
            }
            _ => false,
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

    /// Mark the Rust trait import selected for an observed extension-method call.
    fn mark_rust_extension_trait_imports(
        &mut self,
        receiver: &TypedExpr,
        method: &str,
        dispatch: Option<&IrMethodDispatch>,
    ) {
        let Some(IrMethodDispatch::RustExtensionTraitImport { binding }) = dispatch else {
            if self.receiver_can_use_rust_extension_trait(receiver) {
                self.mark_unambiguous_rust_extension_trait_import(method);
            }
            return;
        };
        if self.rust_extension_trait_imports.contains_key(binding) {
            self.analysis.used_extension_trait_imports.insert(binding.clone());
        }
    }

    /// Mark a trait import for metadata-free fallback only when the method has one possible imported trait.
    fn mark_unambiguous_rust_extension_trait_import(&mut self, method: &str) {
        let mut matches = self
            .rust_extension_trait_imports
            .iter()
            .filter(|(_, import)| import.methods.iter().any(|candidate| candidate == method))
            .map(|(binding, _)| binding.clone());
        let Some(binding) = matches.next() else {
            return;
        };
        if matches.next().is_none() {
            self.analysis.used_extension_trait_imports.insert(binding);
        }
    }

    /// Return whether a method receiver may depend on Rust extension-trait lookup.
    fn receiver_can_use_rust_extension_trait(&self, receiver: &TypedExpr) -> bool {
        if matches!(
            &receiver.kind,
            IrExprKind::Var {
                ref_kind: VarRefKind::ExternalRustName,
                ..
            }
        ) {
            return true;
        }
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
            | IrType::RefMut(inner)
            | IrType::TypeToken(inner) => self.scan_type(inner),
            IrType::ExternalUnion { union, .. } => self.scan_type(union),
            IrType::Dict(key, value) | IrType::Result(key, value) => {
                self.scan_type(key);
                self.scan_type(value);
            }
            IrType::Tuple(items) => {
                for item in items {
                    self.scan_type(item);
                }
            }
            IrType::NamedGeneric(name, args) if name == IR_UNION_TYPE_NAME => {
                for arg in args {
                    self.scan_non_textual_type(arg);
                }
            }
            IrType::Struct(name) | IrType::Enum(name) | IrType::Trait(name) | IrType::NamedGeneric(name, _) => {
                self.mark_reachable_type(name);
                if let IrType::NamedGeneric(_, args) = ty {
                    for arg in args {
                        self.scan_type(arg);
                    }
                }
            }
            IrType::ImplTrait(bound) => {
                self.mark_reachable_type(&bound.trait_path);
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
            | IrType::RustDisplay(_)
            | IrType::SelfType
            | IrType::Unknown => {}
        }
    }

    /// Scan types that are semantically relevant but not printed through the current module's Rust type syntax.
    ///
    /// This keeps local declarations reachable while avoiding `use` bindings for imported types whose names appear only
    /// in inferred expression types or crate-root generated union payloads.
    fn scan_non_textual_type(&mut self, ty: &IrType) {
        match ty {
            IrType::List(inner)
            | IrType::Set(inner)
            | IrType::Option(inner)
            | IrType::Ref(inner)
            | IrType::RefMut(inner)
            | IrType::TypeToken(inner)
            | IrType::ExternalUnion { union: inner, .. } => self.scan_non_textual_type(inner),
            IrType::Dict(key, value) | IrType::Result(key, value) => {
                self.scan_non_textual_type(key);
                self.scan_non_textual_type(value);
            }
            IrType::Tuple(items) | IrType::NamedGeneric(_, items) => {
                if let Some(name) = ty.nominal_type_name()
                    && self.declarations_by_name.contains_key(name)
                    && self.analysis.reachable_items.insert(name.to_string())
                {
                    self.pending.push(name.to_string());
                }
                for item in items {
                    self.scan_non_textual_type(item);
                }
            }
            IrType::Struct(name) | IrType::Enum(name) | IrType::Trait(name) => {
                if self.declarations_by_name.contains_key(name)
                    && self.analysis.reachable_items.insert(name.to_string())
                {
                    self.pending.push(name.to_string());
                }
            }
            IrType::ImplTrait(bound) => {
                for arg in &bound.type_args {
                    self.scan_non_textual_type(arg);
                }
                for (_, ty) in &bound.assoc_types {
                    self.scan_non_textual_type(ty);
                }
            }
            IrType::Function { params, ret } => {
                for param in params {
                    self.scan_non_textual_type(param);
                }
                self.scan_non_textual_type(ret);
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
            | IrType::RustDisplay(_)
            | IrType::SelfType
            | IrType::Unknown => {}
        }
    }

    /// Infer a binding type from constructor-shaped values when lowering left the expression typed as unknown.
    fn inferred_binding_type(value: &TypedExpr) -> Option<IrType> {
        if !matches!(value.ty, IrType::Unknown) {
            return Some(value.ty.clone());
        }
        match &value.kind {
            IrExprKind::Struct { name, .. } => Some(IrType::Struct(name.clone())),
            IrExprKind::Call { func, .. } => {
                let IrExprKind::Var { name, ref_kind, .. } = &func.kind else {
                    return None;
                };
                if matches!(ref_kind, VarRefKind::TypeName) {
                    Some(IrType::Struct(name.clone()))
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Return the nominal type name after peeling explicit reference wrappers.
    fn object_nominal_type_name(&self, object: &TypedExpr) -> Option<String> {
        Self::nominal_type_name(&object.ty).map(str::to_string).or_else(|| {
            let name = match &object.kind {
                IrExprKind::Var { name, .. }
                | IrExprKind::StaticRead { name, .. }
                | IrExprKind::StaticBinding { name, .. } => name,
                _ => return None,
            };
            self.variable_types
                .get(name)
                .and_then(Self::nominal_type_name)
                .map(str::to_string)
                .or_else(|| {
                    (name == keywords::as_str(keywords::KeywordId::SelfKw))
                        .then(|| self.current_impl_target.clone())
                        .flatten()
                })
        })
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
    /// Emit compiler-private FFI declarations and checked scalar, resource, and output wrappers for direct C calls.
    ///
    /// Lowering supplies these only from the descriptor and recorded `unsafe:` call sites. The emitter therefore
    /// receives exact native symbol names, ownership modes, output contracts, and link capabilities rather than
    /// rediscovering a C signature from source syntax or a header.
    fn emit_checked_c_functions(functions: &[IrCheckedCFunction]) -> Vec<TokenStream> {
        let mut items = Self::emit_checked_c_resources(functions);
        items.extend(Self::emit_checked_c_output_slots(functions));
        items.extend(functions.iter().map(|function| {
            let wrapper = format_ident!("{}", function.rust_name());
            let ffi = format_ident!("{}", function.ffi_rust_name());
            let library = &function.system_library;
            let link = checked_c_link_attribute(library, function.link_capability);
            let native_symbol = &function.native_symbol;
            let scalar_generic_bounds = function
                .parameters
                .iter()
                .enumerate()
                .filter_map(|(index, parameter)| {
                    let IrCheckedCType::Scalar(scalar) = parameter else {
                        return None;
                    };
                    let generic = format_ident!("__IncanCheckedCArg{index}");
                    let carrier = Self::checked_c_scalar_rust_type(*scalar);
                    Some(quote! { #generic: ::core::convert::TryInto<#carrier> })
                })
                .collect::<Vec<_>>();
            let scalar_generic_clause =
                (!scalar_generic_bounds.is_empty()).then(|| quote! { <#(#scalar_generic_bounds),*> });
            let wrapper_params = function
                .parameters
                .iter()
                .enumerate()
                .map(|(index, parameter)| {
                    let name = format_ident!("__incan_arg_{index}");
                    let ty = match parameter {
                        IrCheckedCType::Scalar(_) => {
                            let generic = format_ident!("__IncanCheckedCArg{index}");
                            quote! { #generic }
                        }
                        _ => Self::checked_c_wrapper_parameter_type(function, index, parameter),
                    };
                    quote! { #name: #ty }
                })
                .collect::<Vec<_>>();
            let ffi_params = function
                .parameters
                .iter()
                .enumerate()
                .map(|(index, parameter)| {
                    let name = format_ident!("__incan_arg_{index}");
                    let ty = Self::checked_c_ffi_type(parameter);
                    quote! { #name: #ty }
                })
                .collect::<Vec<_>>();
            let ffi_args = function
                .parameters
                .iter()
                .enumerate()
                .map(|(index, parameter)| {
                    let name = format_ident!("__incan_arg_{index}");
                    Self::checked_c_ffi_argument(function, index, parameter, &name)
                })
                .collect::<Vec<_>>();
            let foreign = match &function.return_type {
                IrCheckedCType::Void => quote! {
                    #link
                    unsafe extern "C" {
                        #[link_name = #native_symbol]
                        fn #ffi(#(#ffi_params),*);
                    }
                },
                return_type => {
                    let return_type = Self::checked_c_ffi_type(return_type);
                    quote! {
                        #link
                        unsafe extern "C" {
                            #[link_name = #native_symbol]
                            fn #ffi(#(#ffi_params),*) -> #return_type;
                        }
                    }
                }
            };
            let wrapper_return = Self::checked_c_value_rust_type(&function.binding, &function.return_type);
            let wrapper_item = match &function.return_type {
                IrCheckedCType::Void => quote! {
                    #[inline]
                    fn #wrapper #scalar_generic_clause (#(#wrapper_params),*) {
                        unsafe { #ffi(#(#ffi_args),*); }
                    }
                },
                return_type => {
                    let conversion = Self::checked_c_wrapper_return(function, return_type);
                    quote! {
                        #[inline]
                        fn #wrapper #scalar_generic_clause (#(#wrapper_params),*) -> #wrapper_return {
                            let __incan_result = unsafe { #ffi(#(#ffi_args),*) };
                            #conversion
                        }
                    }
                }
            };
            quote! {
                #foreign

                #wrapper_item
            }
        }));
        items
    }

    /// Emit the private fallible constructor used by checked C string temporaries in this module.
    fn emit_checked_c_string_constructor() -> TokenStream {
        let constructor = format_ident!("{}", incan_core::lang::c_abi::C_STRING_CONSTRUCTOR_RUST_NAME);
        quote! {
            #[inline]
            fn #constructor(value: String) -> Result<::std::ffi::CString, String> {
                ::std::ffi::CString::new(value)
                    .map_err(|_| "C strings cannot contain an interior NUL byte".to_string())
            }
        }
    }

    /// Emit the private bounded owning conversion for one returned C string view.
    fn emit_checked_c_scoped_string_copy() -> TokenStream {
        let helper = format_ident!("{}", incan_core::lang::c_abi::SCOPED_C_STRING_COPY_UTF8_RUST_NAME);
        quote! {
            #[inline]
            fn #helper(value: *const ::std::os::raw::c_char, max_bytes: i64) -> Result<String, String> {
                if value.is_null() {
                    return Err("cannot copy text from a null C string view".to_string());
                }
                let limit = match usize::try_from(max_bytes) {
                    Ok(limit) if limit > 0 => limit,
                    Ok(_) => return Err("copy_utf8(max_bytes=...) requires a positive bound".to_string()),
                    Err(_) => return Err("copy_utf8(max_bytes=...) exceeds the platform addressable range".to_string()),
                };
                // SAFETY: the explicit source bound limits the scan; the checked C declaration and enclosing unsafe
                // acknowledgement remain responsible for the foreign view being readable for that bounded range.
                let bytes = unsafe { ::std::slice::from_raw_parts(value.cast::<u8>(), limit) };
                let Some(terminator) = bytes.iter().position(|byte| *byte == 0) else {
                    return Err("C string view has no terminator within max_bytes".to_string());
                };
                ::std::str::from_utf8(&bytes[..terminator])
                    .map(str::to_owned)
                    .map_err(|_| "C string view is not valid UTF-8".to_string())
            }
        }
    }

    /// Emit the private validation boundary that returns an existing caller-owned typed allocation after a C write.
    fn emit_checked_c_span_finish() -> TokenStream {
        let helper = format_ident!("{}", incan_core::lang::c_abi::MUTABLE_SPAN_FINISH_RUST_NAME);
        quote! {
            #[inline]
            fn #helper<T, W>(mut value: Vec<T>, written: W) -> Result<Vec<T>, String>
            where
                W: ::core::convert::TryInto<usize>,
            {
                let written = match <W as ::core::convert::TryInto<usize>>::try_into(written) {
                    Ok(written) => written,
                    Err(_) => return Err("C span result count is outside the platform addressable range".to_string()),
                };
                if written > value.len() {
                    return Err("C span result count exceeds the declared caller-owned capacity".to_string());
                }
                value.truncate(written);
                Ok(value)
            }
        }
    }

    /// Emit one non-cloneable release-guard wrapper for each opaque C resource used by emitted calls.
    fn emit_checked_c_resources(functions: &[IrCheckedCFunction]) -> Vec<TokenStream> {
        let mut resources = BTreeMap::<String, IrCheckedCResource>::new();
        for resource in functions.iter().flat_map(|function| function.resources.iter()) {
            resources
                .entry(IrCheckedCFunction::resource_rust_type_name(
                    &resource.binding,
                    &resource.resource,
                ))
                .or_insert_with(|| resource.clone());
        }
        resources
            .into_iter()
            .map(|(name, resource)| {
                let wrapper = format_ident!("{name}");
                let release = format_ident!("{name}__release");
                let library = resource.system_library;
                let link = checked_c_link_attribute(&library, resource.link_capability);
                let native = resource.release_native_symbol;
                let release_return = Self::checked_c_ffi_type(&resource.release_return_type);
                quote! {
                    #[repr(transparent)]
                    struct #wrapper {
                        ptr: *mut ::core::ffi::c_void,
                    }

                    #link
                    unsafe extern "C" {
                        #[link_name = #native]
                        fn #release(ptr: *mut ::core::ffi::c_void) -> #release_return;
                    }

                    impl #wrapper {
                        #[inline]
                        fn from_raw(ptr: *mut ::core::ffi::c_void) -> Self {
                            if ptr.is_null() {
                                panic!("checked C resource constructor returned a null owned pointer");
                            }
                            Self { ptr }
                        }

                        #[inline]
                        fn as_raw(&self) -> *mut ::core::ffi::c_void {
                            self.ptr
                        }

                        #[inline]
                        fn into_raw(self) -> *mut ::core::ffi::c_void {
                            let resource = ::core::mem::ManuallyDrop::new(self);
                            resource.ptr
                        }
                    }

                    impl ::core::ops::Drop for #wrapper {
                        fn drop(&mut self) {
                            if !self.ptr.is_null() {
                                unsafe { #release(self.ptr); }
                                self.ptr = ::core::ptr::null_mut();
                            }
                        }
                    }
                }
            })
            .collect()
    }

    /// Emit the private storage wrapper for every raw C output parameter selected by lowering.
    fn emit_checked_c_output_slots(functions: &[IrCheckedCFunction]) -> Vec<TokenStream> {
        functions
            .iter()
            .flat_map(|function| {
                function.parameters.iter().enumerate().filter_map(move |(index, parameter)| {
                    let IrCheckedCType::Output { value, .. } = parameter else {
                        return None;
                    };
                    let parameter = function.parameter_names.get(index)?;
                    let slot = format_ident!(
                        "{}",
                        IrCheckedCFunction::output_slot_rust_type_name(&function.binding, &function.symbol, parameter)
                    );
                    match value.as_ref() {
                        IrCheckedCType::Scalar(scalar) => {
                            let carrier = Self::checked_c_scalar_rust_type(*scalar);
                            let source_carrier = Self::checked_c_source_scalar_rust_type(*scalar);
                            let argument_message = format!(
                                "checked C in/out value for {}.{}.{parameter} is outside the declared {} range",
                                function.binding,
                                function.symbol,
                                incan_core::lang::c_abi::scalar_type_as_str(*scalar)
                            );
                            let result_message = format!(
                                "checked C output value for {}.{}.{parameter} cannot be represented by Incan int",
                                function.binding, function.symbol
                            );
                            let from_incan_value = if incan_core::lang::c_abi::scalar_numeric_type(*scalar).is_some() {
                                quote! { value }
                            } else {
                                quote! {
                                    match <#carrier>::try_from(value) {
                                        Ok(value) => value,
                                        Err(_) => panic!(#argument_message),
                                    }
                                }
                            };
                            let take_value = if incan_core::lang::c_abi::scalar_numeric_type(*scalar).is_some() {
                                quote! { value }
                            } else {
                                quote! {
                                    match i64::try_from(value) {
                                        Ok(value) => value,
                                        Err(_) => panic!(#result_message),
                                    }
                                }
                            };
                            Some(quote! {
                                struct #slot { value: ::core::mem::MaybeUninit<#carrier> }

                                impl #slot {
                                    #[inline]
                                    fn uninit() -> Self { Self { value: ::core::mem::MaybeUninit::uninit() } }

                                    #[inline]
                                    fn from_incan_value(value: #source_carrier) -> Self {
                                        let value = #from_incan_value;
                                        Self { value: ::core::mem::MaybeUninit::new(value) }
                                    }

                                    #[inline]
                                    fn as_mut_ptr(&mut self) -> *mut #carrier { self.value.as_mut_ptr() }

                                    #[inline]
                                    fn take(self) -> #source_carrier {
                                        let value = unsafe { self.value.assume_init() };
                                        #take_value
                                    }
                                }
                            })
                        }
                        IrCheckedCType::Resource { resource, .. } => {
                            let resource = format_ident!("{}", IrCheckedCFunction::resource_rust_type_name(&function.binding, resource));
                            Some(quote! {
                                struct #slot { value: ::core::mem::MaybeUninit<*mut ::core::ffi::c_void> }

                                impl #slot {
                                    #[inline]
                                    fn uninit() -> Self { Self { value: ::core::mem::MaybeUninit::uninit() } }

                                    #[inline]
                                    fn as_mut_ptr(&mut self) -> *mut *mut ::core::ffi::c_void { self.value.as_mut_ptr() }

                                    #[inline]
                                    fn take(self) -> #resource {
                                        #resource::from_raw(unsafe { self.value.assume_init() })
                                    }
                                }
                            })
                        }
                        _ => None,
                    }
                })
            })
            .collect()
    }

    /// Return the generated wrapper parameter type for one bounded C ABI contract.
    fn checked_c_wrapper_parameter_type(
        function: &IrCheckedCFunction,
        index: usize,
        ty: &IrCheckedCType,
    ) -> TokenStream {
        match ty {
            IrCheckedCType::Scalar(scalar) => Self::checked_c_source_scalar_rust_type(*scalar),
            IrCheckedCType::Resource { access, resource } => {
                let resource = format_ident!(
                    "{}",
                    IrCheckedCFunction::resource_rust_type_name(&function.binding, resource)
                );
                match access {
                    crate::frontend::typechecker::CResourceAccess::Owned => quote! { #resource },
                    crate::frontend::typechecker::CResourceAccess::Borrowed => quote! { &#resource },
                    crate::frontend::typechecker::CResourceAccess::BorrowedMut => quote! { &mut #resource },
                }
            }
            IrCheckedCType::Output { .. } => {
                let parameter = function.parameter_names.get(index).map(String::as_str).unwrap_or("arg");
                let slot = format_ident!(
                    "{}",
                    IrCheckedCFunction::output_slot_rust_type_name(&function.binding, &function.symbol, parameter)
                );
                quote! { &mut #slot }
            }
            IrCheckedCType::Pointer { .. } => Self::checked_c_ffi_type(ty),
            IrCheckedCType::Nullable(_) | IrCheckedCType::Void => quote! { () },
        }
    }

    /// Return the exact Rust ABI type for one bounded checked-C contract.
    fn checked_c_ffi_type(ty: &IrCheckedCType) -> TokenStream {
        match ty {
            IrCheckedCType::Scalar(scalar) => Self::checked_c_scalar_rust_type(*scalar),
            IrCheckedCType::Pointer { mutable, pointee } => {
                let pointee = Self::checked_c_ffi_type(pointee);
                if *mutable {
                    quote! { *mut #pointee }
                } else {
                    quote! { *const #pointee }
                }
            }
            IrCheckedCType::Resource { .. } => quote! { *mut ::core::ffi::c_void },
            IrCheckedCType::Output { value, .. } => {
                let value = Self::checked_c_ffi_type(value);
                quote! { *mut #value }
            }
            IrCheckedCType::Nullable(value) => Self::checked_c_ffi_type(value),
            IrCheckedCType::Void => quote! { () },
        }
    }

    /// Convert one generated wrapper argument into its exact foreign ABI carrier.
    fn checked_c_ffi_argument(
        function: &IrCheckedCFunction,
        index: usize,
        ty: &IrCheckedCType,
        name: &proc_macro2::Ident,
    ) -> TokenStream {
        match ty {
            IrCheckedCType::Scalar(scalar) => {
                let carrier = Self::checked_c_scalar_rust_type(*scalar);
                let message = format!(
                    "checked C argument {index} for {}.{} is outside the declared {} range",
                    function.binding,
                    function.symbol,
                    incan_core::lang::c_abi::scalar_type_as_str(*scalar),
                );
                quote! {
                    match <_ as ::core::convert::TryInto<#carrier>>::try_into(#name) {
                        Ok(value) => value,
                        Err(_) => panic!(#message),
                    }
                }
            }
            IrCheckedCType::Resource { access, .. } => match access {
                crate::frontend::typechecker::CResourceAccess::Owned => quote! { #name.into_raw() },
                crate::frontend::typechecker::CResourceAccess::Borrowed
                | crate::frontend::typechecker::CResourceAccess::BorrowedMut => quote! { #name.as_raw() },
            },
            IrCheckedCType::Output { .. } => quote! { #name.as_mut_ptr() },
            IrCheckedCType::Pointer { .. } => quote! { #name },
            IrCheckedCType::Nullable(_) | IrCheckedCType::Void => quote! { () },
        }
    }

    /// Return the generated ordinary-Rust result carrier for one bounded checked-C value contract.
    fn checked_c_value_rust_type(binding: &str, ty: &IrCheckedCType) -> TokenStream {
        match ty {
            IrCheckedCType::Scalar(scalar) => Self::checked_c_source_scalar_rust_type(*scalar),
            IrCheckedCType::Pointer { .. } => Self::checked_c_ffi_type(ty),
            IrCheckedCType::Resource { resource, .. } => {
                let resource = format_ident!("{}", IrCheckedCFunction::resource_rust_type_name(binding, resource));
                quote! { #resource }
            }
            IrCheckedCType::Nullable(value) => {
                let value = Self::checked_c_value_rust_type(binding, value);
                quote! { Option<#value> }
            }
            IrCheckedCType::Void | IrCheckedCType::Output { .. } => quote! { () },
        }
    }

    /// Convert an exact foreign return value into the ordinary checked Incan carrier.
    fn checked_c_wrapper_return(function: &IrCheckedCFunction, ty: &IrCheckedCType) -> TokenStream {
        match ty {
            IrCheckedCType::Scalar(scalar) => {
                if incan_core::lang::c_abi::scalar_numeric_type(*scalar).is_some() {
                    return quote! { __incan_result };
                }
                let message = format!(
                    "checked C result for {}.{} cannot be represented by Incan int ({})",
                    function.binding,
                    function.symbol,
                    incan_core::lang::c_abi::scalar_type_as_str(*scalar),
                );
                quote! { match i64::try_from(__incan_result) { Ok(value) => value, Err(_) => panic!(#message) } }
            }
            IrCheckedCType::Resource { resource, .. } => {
                let resource = format_ident!(
                    "{}",
                    IrCheckedCFunction::resource_rust_type_name(&function.binding, resource)
                );
                quote! { #resource::from_raw(__incan_result) }
            }
            IrCheckedCType::Nullable(value) => match value.as_ref() {
                IrCheckedCType::Resource { resource, .. } => {
                    let resource = format_ident!(
                        "{}",
                        IrCheckedCFunction::resource_rust_type_name(&function.binding, resource)
                    );
                    quote! {
                        if __incan_result.is_null() { None } else { Some(#resource::from_raw(__incan_result)) }
                    }
                }
                _ => quote! { () },
            },
            IrCheckedCType::Pointer { .. } => quote! { __incan_result },
            IrCheckedCType::Void | IrCheckedCType::Output { .. } => quote! { () },
        }
    }

    /// Return the Rust ABI carrier for one source-checked C scalar.
    fn checked_c_scalar_rust_type(scalar: ScalarTypeId) -> TokenStream {
        match scalar {
            ScalarTypeId::I8 => quote! { i8 },
            ScalarTypeId::U8 => quote! { u8 },
            ScalarTypeId::I16 => quote! { i16 },
            ScalarTypeId::U16 => quote! { u16 },
            ScalarTypeId::I32 => quote! { i32 },
            ScalarTypeId::U32 => quote! { u32 },
            ScalarTypeId::I64 => quote! { i64 },
            ScalarTypeId::U64 => quote! { u64 },
            ScalarTypeId::I128 => quote! { i128 },
            ScalarTypeId::U128 => quote! { u128 },
            ScalarTypeId::F32 => quote! { f32 },
            ScalarTypeId::F64 => quote! { f64 },
            ScalarTypeId::Size => quote! { usize },
            ScalarTypeId::CChar => quote! { ::std::os::raw::c_char },
            ScalarTypeId::CInt => quote! { ::std::os::raw::c_int },
        }
    }

    /// Return the generated source-visible Rust representation for one checked C scalar.
    ///
    /// Fixed-width and selected-target `size_t` carriers retain their exact Incan numeric representation. The two
    /// target-defined C aliases remain on the legacy checked `i64` façade until receipt-selected target layout facts
    /// can name a stable Incan numeric identity.
    fn checked_c_source_scalar_rust_type(scalar: ScalarTypeId) -> TokenStream {
        if incan_core::lang::c_abi::scalar_numeric_type(scalar).is_some() {
            Self::checked_c_scalar_rust_type(scalar)
        } else {
            quote! { i64 }
        }
    }

    /// Collect imported static bindings that need generated init calls.
    fn collect_imported_static_init_bindings(&self, declarations: &[&IrDecl]) -> (HashSet<String>, Vec<String>) {
        let mut access_bindings = HashSet::new();
        let mut module_init_bindings = HashSet::new();
        for decl in declarations {
            let IrDeclKind::Import {
                visibility,
                origin,
                qualifier,
                path,
                items,
                ..
            } = &decl.kind
            else {
                continue;
            };
            if matches!(origin, IrImportOrigin::PubLibrary { .. }) || matches!(qualifier, IrImportQualifier::None) {
                continue;
            }
            let is_incan_source_stdlib = Self::is_incan_source_stdlib_import(origin, qualifier, path);
            let is_public_reexport = !matches!(visibility, Visibility::Private);
            for item in items {
                if !item.is_static {
                    continue;
                }
                let binding = item.alias.as_ref().unwrap_or(&item.name);
                if self.should_emit_import_binding(binding) {
                    access_bindings.insert(binding.clone());
                }
                if is_public_reexport && !(is_incan_source_stdlib && binding.starts_with('_')) {
                    module_init_bindings.insert(binding.clone());
                }
            }
        }
        let mut module_init_bindings: Vec<_> = module_init_bindings.into_iter().collect();
        module_init_bindings.sort();
        (access_bindings, module_init_bindings)
    }

    /// Return whether the current emitted module defines one registry-backed temporary capability trait contract.
    fn emitted_declarations_define_capability_trait(
        program: &IrProgram,
        emitted_declarations: &[&IrDecl],
        capability: &trait_capabilities::TraitCapabilityInfo,
    ) -> bool {
        let Some(source_module_name) = program.source_module_name.as_deref() else {
            return false;
        };
        let canonical_module = capability.module_path.join(".");
        let relative_module = capability
            .module_path
            .strip_prefix(&["std"])
            .map(|tail| tail.join("."))
            .unwrap_or_else(|| canonical_module.clone());
        let generated_module = capability
            .module_path
            .strip_prefix(&["std"])
            .map(|tail| format!("{}.{}", core_stdlib::INCAN_STD_NAMESPACE, tail.join(".")))
            .unwrap_or_else(|| canonical_module.clone());
        if source_module_name != canonical_module
            && source_module_name != relative_module
            && source_module_name != generated_module
        {
            return false;
        }
        emitted_declarations.iter().any(|decl| {
            matches!(
                &decl.kind,
                IrDeclKind::Trait(trait_decl) if trait_decl.name == capability.trait_name
                    && capability.required_methods.iter().all(|required| {
                        trait_decl.methods.iter().any(|method| method.name == *required)
                    })
            )
        })
    }

    /// Return whether a registered generated-support hook should be spliced into this generated module.
    fn emits_registered_support_module(
        program: &IrProgram,
        support: &incan_core::lang::generated_support::GeneratedModuleSupport,
    ) -> bool {
        matches!(
            program.source_module_name.as_deref(),
            Some(module_name)
                if module_name == support.source_module
                    || module_name == support.generated_module
                    || (std::env::var_os(SDK_PROVIDER_BUILD_ENV).is_some()
                        && support
                            .source_module
                            .strip_prefix("std.")
                            .is_some_and(|artifact_module| module_name == artifact_module))
        )
    }

    /// Emit a macro invocation from a registered support path.
    fn emit_support_macro_invocation(macro_path: &str) -> TokenStream {
        let mut segments = macro_path.split("::").map(Self::rust_ident);
        let Some(first) = segments.next() else {
            return quote! {};
        };
        let path = segments.fold(quote! { #first }, |acc, segment| quote! { #acc :: #segment });
        quote! { #path!(); }
    }

    /// Splice registered generated-code support into generated modules.
    fn emit_registered_generated_module_supports(program: &IrProgram) -> Vec<TokenStream> {
        incan_core::lang::generated_support::generated_module_supports()
            .iter()
            .filter(|support| Self::emits_registered_support_module(program, support))
            .map(|support| Self::emit_support_macro_invocation(support.macro_path))
            .collect()
    }

    /// Emit temporary RFC 101 adapter impls for deterministic builtin `OrdinalKey` families.
    ///
    /// Native helper behavior lives in `incan_stdlib::collections::__private`; this emitter only places impls at the
    /// crate boundary where Rust coherence requires them until RFC 098/099 can model trait-owned capability families
    /// in source.
    fn emit_builtin_ordinal_key_impls(&self) -> TokenStream {
        quote! {
            fn __incan_ordinal_key_invalid_record(detail: String) -> OrdinalMapError {
                OrdinalMapError::invalid_key_record(detail, -1i64)
            }

            macro_rules! __incan_ordinal_key_int_impl {
                ($ty:ty, $encoding:expr, $width:expr) => {
                    impl OrdinalKey for $ty {
                        fn ordinal_bytes(&self) -> Vec<u8> {
                            (*self).to_le_bytes().to_vec()
                        }

                        fn ordinal_hash(&self) -> i64 {
                            incan_stdlib::collections::__private::ordinal_key_hash_bytes(&(*self).to_le_bytes())
                        }

                        fn ordinal_bytes_equal(&self, data: Vec<u8>) -> bool {
                            data.as_slice() == (*self).to_le_bytes().as_slice()
                        }

                        fn ordinal_encoding() -> String {
                            $encoding
                        }

                        fn from_ordinal_bytes(data: Vec<u8>) -> Result<Self, OrdinalMapError> {
                            let encoding = $encoding;
                            Ok(<$ty>::from_le_bytes(
                                incan_stdlib::collections::__private::ordinal_key_exact_bytes::<$width>(
                                    data,
                                    encoding.as_str(),
                                )
                                .map_err(__incan_ordinal_key_invalid_record)?,
                            ))
                        }
                    }
                };
            }

            impl OrdinalKey for String {
                fn ordinal_bytes(&self) -> Vec<u8> {
                    self.as_bytes().to_vec()
                }

                fn ordinal_hash(&self) -> i64 {
                    incan_stdlib::collections::__private::ordinal_key_hash_bytes(self.as_bytes())
                }

                fn ordinal_bytes_equal(&self, data: Vec<u8>) -> bool {
                    self.as_bytes() == data.as_slice()
                }

                fn ordinal_encoding() -> String {
                    incan_stdlib::collections::__private::ordinal_key_encoding_str()
                }

                fn from_ordinal_bytes(data: Vec<u8>) -> Result<Self, OrdinalMapError> {
                    incan_stdlib::collections::__private::ordinal_key_string_from_bytes(data)
                        .map_err(__incan_ordinal_key_invalid_record)
                }
            }

            impl OrdinalKey for Vec<u8> {
                fn ordinal_bytes(&self) -> Vec<u8> {
                    self.clone()
                }

                fn ordinal_hash(&self) -> i64 {
                    incan_stdlib::collections::__private::ordinal_key_hash_bytes(self.as_slice())
                }

                fn ordinal_bytes_equal(&self, data: Vec<u8>) -> bool {
                    self.as_slice() == data.as_slice()
                }

                fn ordinal_encoding() -> String {
                    incan_stdlib::collections::__private::ordinal_key_encoding_bytes()
                }

                fn from_ordinal_bytes(data: Vec<u8>) -> Result<Self, OrdinalMapError> {
                    Ok(data)
                }
            }

            impl OrdinalKey for bool {
                fn ordinal_bytes(&self) -> Vec<u8> {
                    vec![*self as u8]
                }

                fn ordinal_hash(&self) -> i64 {
                    incan_stdlib::collections::__private::ordinal_key_hash_bytes(&[*self as u8])
                }

                fn ordinal_bytes_equal(&self, data: Vec<u8>) -> bool {
                    data.as_slice() == [*self as u8].as_slice()
                }

                fn ordinal_encoding() -> String {
                    incan_stdlib::collections::__private::ordinal_key_encoding_bool()
                }

                fn from_ordinal_bytes(data: Vec<u8>) -> Result<Self, OrdinalMapError> {
                    incan_stdlib::collections::__private::ordinal_key_bool_from_bytes(data)
                        .map_err(__incan_ordinal_key_invalid_record)
                }
            }

            impl OrdinalKey for incan_stdlib::num::Decimal128 {
                fn ordinal_bytes(&self) -> Vec<u8> {
                    incan_stdlib::collections::__private::ordinal_key_decimal_bytes(self).to_vec()
                }

                fn ordinal_hash(&self) -> i64 {
                    let out = incan_stdlib::collections::__private::ordinal_key_decimal_bytes(self);
                    incan_stdlib::collections::__private::ordinal_key_hash_bytes(&out)
                }

                fn ordinal_bytes_equal(&self, data: Vec<u8>) -> bool {
                    data.as_slice()
                        == incan_stdlib::collections::__private::ordinal_key_decimal_bytes(self).as_slice()
                }

                fn ordinal_encoding() -> String {
                    incan_stdlib::collections::__private::ordinal_key_encoding_decimal()
                }

                fn from_ordinal_bytes(data: Vec<u8>) -> Result<Self, OrdinalMapError> {
                    incan_stdlib::collections::__private::ordinal_key_decimal_from_bytes(data)
                        .map_err(__incan_ordinal_key_invalid_record)
                }
            }

            __incan_ordinal_key_int_impl!(i8, incan_stdlib::collections::__private::ordinal_key_encoding_int(8u16), 1usize);
            __incan_ordinal_key_int_impl!(i16, incan_stdlib::collections::__private::ordinal_key_encoding_int(16u16), 2usize);
            __incan_ordinal_key_int_impl!(i32, incan_stdlib::collections::__private::ordinal_key_encoding_int(32u16), 4usize);
            __incan_ordinal_key_int_impl!(i64, incan_stdlib::collections::__private::ordinal_key_encoding_int(64u16), 8usize);
            __incan_ordinal_key_int_impl!(i128, incan_stdlib::collections::__private::ordinal_key_encoding_int(128u16), 16usize);
            __incan_ordinal_key_int_impl!(u8, incan_stdlib::collections::__private::ordinal_key_encoding_uint(8u16), 1usize);
            __incan_ordinal_key_int_impl!(u16, incan_stdlib::collections::__private::ordinal_key_encoding_uint(16u16), 2usize);
            __incan_ordinal_key_int_impl!(u32, incan_stdlib::collections::__private::ordinal_key_encoding_uint(32u16), 4usize);
            __incan_ordinal_key_int_impl!(u64, incan_stdlib::collections::__private::ordinal_key_encoding_uint(64u16), 8usize);
            __incan_ordinal_key_int_impl!(u128, incan_stdlib::collections::__private::ordinal_key_encoding_uint(128u16), 16usize);
        }
    }

    /// Emit the temporary primitive implementations behind the source-owned `Sum[T]` contract.
    ///
    /// RFC 088 keeps the public iteration and summation protocol in Incan. These implementations are a deliberately
    /// narrow compiler bridge until ordinary primitive operation declarations can express the same loops in source.
    fn emit_builtin_iterator_sum_impls(&self, emitted_declarations: &[&IrDecl], program: &IrProgram) -> TokenStream {
        let defines_sum_trait = Self::emitted_declarations_define_capability_trait(
            program,
            emitted_declarations,
            trait_capabilities::iterator_sum(),
        );
        if !defines_sum_trait {
            return quote! {};
        }
        quote! {
            impl Sum<i64> for i64 {
                fn sum<I: Iterator<i64>>(mut items: I) -> Self {
                    let mut total = 0i64;
                    loop {
                        match <I as Iterator<i64>>::__next__(&mut items) {
                            Some(item) => total += item,
                            None => break total,
                        }
                    }
                }
            }

            impl Sum<f64> for f64 {
                fn sum<I: Iterator<f64>>(mut items: I) -> Self {
                    let mut total = 0.0f64;
                    loop {
                        match <I as Iterator<f64>>::__next__(&mut items) {
                            Some(item) => total += item,
                            None => break total,
                        }
                    }
                }
            }
        }
    }

    /// Emit local newtype `Sum` implementations when the source imports the iterator contract.
    ///
    /// The primitive arithmetic loop remains a temporary backend bridge, but the public capability is the source-owned
    /// `Sum[T]` trait. Checked newtypes reconstruct through their canonical validator exactly as the former special
    /// iterator lowering did.
    fn emit_local_newtype_iterator_sum_impls(&self, imports_iterator_contract: bool) -> TokenStream {
        if !imports_iterator_contract {
            return quote! {};
        }
        let sum_trait = quote! { crate::__incan_std::derives::collection::Sum };
        let iterator_trait = quote! { crate::__incan_std::derives::collection::Iterator };
        let mut plans = self.newtype_construction.iter().collect::<Vec<_>>();
        plans.sort_by_key(|(name, _)| (*name).clone());
        let mut impls = Vec::new();
        for (source_name, plan) in plans {
            if !plan.type_params.is_empty() || !matches!(plan.underlying, IrType::Int | IrType::Float) {
                continue;
            }
            let name = Self::rust_ident(source_name);
            let underlying = self.emit_type(&plan.underlying);
            let zero = if matches!(plan.underlying, IrType::Float) {
                quote! { 0.0f64 }
            } else {
                quote! { 0i64 }
            };
            let construction = if let Some(constructor) = &plan.checked_constructor {
                let constructor = Self::rust_ident(constructor);
                let message = format!("validated newtype construction failed: {source_name}::{constructor}");
                quote! {
                    match #name::#constructor(total) {
                        Ok(value) => value,
                        Err(_) => panic!(#message),
                    }
                }
            } else {
                quote! { #name(total) }
            };
            impls.push(quote! {
                impl #sum_trait<#name> for #name {
                    fn sum<I: #iterator_trait<#name>>(mut items: I) -> Self {
                        let mut total: #underlying = #zero;
                        loop {
                            match <I as #iterator_trait<#name>>::__next__(&mut items) {
                                Some(item) => total += item.0,
                                None => break #construction,
                            }
                        }
                    }
                }
            });
        }
        quote! { #(#impls)* }
    }

    /// Emit compiler-provided `TryFrom[str]` implementations for RFC 089 primitive targets.
    ///
    /// `TryFrom` remains a source-owned Incan trait. These impls are emitted next to that trait declaration, where
    /// Rust coherence permits the generated crate to implement it for language primitive representations.
    fn emit_builtin_string_try_from_impls(&self) -> TokenStream {
        let capability = trait_capabilities::string_try_from();
        let parse_impls = numerics::NUMERIC_TYPES
            .iter()
            .filter(|info| {
                let capability_type = if info.family == NumericFamily::Bool {
                    trait_capabilities::TraitCapabilityType::Bool
                } else {
                    trait_capabilities::TraitCapabilityType::Numeric(info.id)
                };
                trait_capabilities::supports_type(capability, capability_type)
            })
            .map(|info| {
                let ty = Self::rust_ident(info.canonical);
                quote! { __incan_try_from_string_parse_impl!(#ty); }
            })
            .collect::<Vec<_>>();
        quote! {
            macro_rules! __incan_try_from_string_parse_impl {
                ($ty:ty) => {
                    impl TryFrom<String> for $ty {
                        fn try_from(value: String) -> Result<Self, String> {
                            value.parse::<$ty>().map_err(|error| error.to_string())
                        }
                    }
                };
            }

            impl TryFrom<String> for String {
                fn try_from(value: String) -> Result<Self, String> {
                    Ok(value)
                }
            }

            #(#parse_impls)*
        }
    }

    /// Return local types that already provide the canonical `std.traits.convert.TryFrom[str]` implementation.
    fn explicit_string_try_from_types(emitted_declarations: &[&IrDecl]) -> HashSet<String> {
        let capability = trait_capabilities::string_try_from();
        emitted_declarations
            .iter()
            .filter_map(|decl| {
                let IrDeclKind::Impl(impl_block) = &decl.kind else {
                    return None;
                };
                let source_name = impl_block
                    .trait_source_name
                    .as_deref()
                    .or(impl_block.trait_name.as_deref())?;
                (source_name == capability.trait_name
                    && impl_block
                        .trait_module_path
                        .as_deref()
                        .is_some_and(|path| trait_capabilities::module_path_matches(capability, path))
                    && impl_block.trait_type_args.as_slice() == [IrType::String])
                .then(|| impl_block.target_type.clone())
            })
            .collect()
    }

    /// Emit one constrained-newtype predicate against the parsed underlying value.
    pub(in crate::backend::ir::emit) fn newtype_constraint_predicate(
        constraint: &NewtypePrimitiveConstraint,
        float_underlying: bool,
        value: TokenStream,
    ) -> TokenStream {
        let literal = if float_underlying {
            let value = proc_macro2::Literal::f64_unsuffixed(constraint.value as f64);
            quote! { #value }
        } else {
            let value = proc_macro2::Literal::i64_unsuffixed(constraint.value);
            quote! { #value }
        };
        match constraint.key {
            TypeConstraintKey::Ge => quote! { #value >= #literal },
            TypeConstraintKey::Gt => quote! { #value > #literal },
            TypeConstraintKey::Le => quote! { #value <= #literal },
            TypeConstraintKey::Lt => quote! { #value < #literal },
        }
    }

    /// Return whether a newtype underlying type uses a floating-point Rust representation.
    pub(in crate::backend::ir::emit) fn newtype_underlying_is_float(underlying: &IrType) -> bool {
        matches!(underlying, IrType::Float)
            || matches!(
                underlying,
                IrType::Numeric(id) if numerics::info_for(*id).family == NumericFamily::BinaryFloat
            )
    }

    /// Return the source-owned `TryFrom[str]` trait identity for the current generated crate.
    ///
    /// A normal consumer must implement the trait exported by the compiled provider; only the provider build
    /// itself owns the compatibility facade under `crate::__incan_std`.
    fn string_try_from_trait_path() -> TokenStream {
        quote! { crate::__incan_std::traits::convert::TryFrom }
    }

    /// Return the `OrdinalKey` contract and error identities for the current generated crate.
    ///
    /// Consumers implement the trait exported by the compiled stdlib artifact; the artifact build itself keeps using
    /// its crate-local compatibility facade while source modules are compiled together.
    fn ordinal_key_contract_paths() -> (TokenStream, TokenStream) {
        (
            quote! { crate::__incan_std::collections::OrdinalKey },
            quote! { crate::__incan_std::collections::OrdinalMapError },
        )
    }

    /// Emit compiler-provided `TryFrom[str]` implementations from local newtype construction plans.
    fn emit_local_newtype_string_try_from_impls(&self, emitted_declarations: &[&IrDecl]) -> TokenStream {
        if !self.emit_std_string_try_from_newtype_impls {
            return quote! {};
        }

        let explicit = Self::explicit_string_try_from_types(emitted_declarations);
        let trait_path = Self::string_try_from_trait_path();
        let mut impls = Vec::new();
        let mut plans = self.newtype_construction.iter().collect::<Vec<_>>();
        plans.sort_by_key(|(name, _)| (*name).clone());
        for (source_name, plan) in plans {
            if !plan.supports_string_conversion || explicit.contains(source_name) {
                continue;
            }

            let name = Self::rust_ident(source_name);
            let generics = self.emit_type_params(&plan.type_params);
            let generics_bare = self.emit_type_params_bare(&plan.type_params);
            let underlying = self.emit_type(&plan.underlying);
            let construction = if let Some(constructor) = &plan.checked_constructor {
                let constructor = Self::rust_ident(constructor);
                quote! {
                    #name::#constructor(parsed)
                        .map_err(|_| format!("{} validation failed", stringify!(#name)))
                }
            } else if !plan.constraints.is_empty() {
                let float_underlying = Self::newtype_underlying_is_float(&plan.underlying);
                let predicates = plan
                    .constraints
                    .iter()
                    .map(|constraint| {
                        Self::newtype_constraint_predicate(constraint, float_underlying, quote! { parsed })
                    })
                    .collect::<Vec<_>>();
                quote! {
                    if #(#predicates)&&* {
                        Ok(#name(parsed))
                    } else {
                        Err(format!("{} validation failed", stringify!(#name)))
                    }
                }
            } else {
                quote! { Ok(#name(parsed)) }
            };

            impls.push(quote! {
                impl #generics #trait_path<String> for #name #generics_bare
                where
                    #underlying: #trait_path<String>,
                {
                    fn try_from(value: String) -> Result<Self, String> {
                        let parsed = <#underlying as #trait_path<String>>::try_from(value)?;
                        #construction
                    }
                }
            });
        }
        quote! { #(#impls)* }
    }

    /// Return whether the current module imports the stdlib ordinal-map contract surface.
    fn emitted_declarations_import_std_collections_ordinal_contract(emitted_declarations: &[&IrDecl]) -> bool {
        let capability = trait_capabilities::stable_ordinal_key();
        emitted_declarations.iter().any(|decl| {
            let IrDeclKind::Import { path, items, .. } = &decl.kind else {
                return false;
            };
            if !trait_capabilities::module_path_matches(capability, path) {
                return false;
            }
            items
                .iter()
                .any(|item| trait_capabilities::import_triggers_capability(capability, item.name.as_str()))
        })
    }

    /// Build the stable public/source identity for a string or integer value enum.
    fn value_enum_ordinal_type_identity(&self, e: &IrEnum, source_module_name: Option<&str>) -> String {
        let source_identity = format!(
            "{}.{}",
            source_module_name.filter(|name| !name.is_empty()).unwrap_or("local"),
            e.name
        );
        self.public_ordinal_type_identities
            .get(&source_identity)
            .cloned()
            .unwrap_or(source_identity)
    }

    /// Build the stable `OrdinalKey.ordinal_encoding()` identifier for a string or integer value enum.
    fn value_enum_ordinal_encoding(&self, e: &IrEnum, source_module_name: Option<&str>) -> Option<String> {
        let value_type = e.value_type?;
        let values = e
            .variants
            .iter()
            .map(|variant| variant.raw_value.clone())
            .collect::<Option<Vec<_>>>()?;
        Self::value_enum_ordinal_encoding_from_values(
            value_type,
            &self.value_enum_ordinal_type_identity(e, source_module_name),
            &values,
        )
    }

    /// Build the stable `OrdinalKey.ordinal_encoding()` identifier for an external scalar value enum.
    fn external_value_enum_ordinal_encoding(e: &super::ExternalOrdinalValueEnum) -> Option<String> {
        Self::value_enum_ordinal_encoding_from_values(e.value_type, &e.type_identity, &e.values)
    }

    /// Build a stable value-enum encoding string from exported raw variant values.
    fn value_enum_ordinal_encoding_from_values(
        value_type: IrEnumValueType,
        type_identity: &str,
        values: &[IrEnumValue],
    ) -> Option<String> {
        let mut records = String::new();
        match value_type {
            IrEnumValueType::String => {
                for value in values {
                    let IrEnumValue::String(raw) = value else {
                        return None;
                    };
                    records.push_str(&format!("{}:{};", raw.len(), raw));
                }
                Some(format!("value-enum:str:{}:{}:v1", type_identity, records))
            }
            IrEnumValueType::Int => {
                for value in values {
                    let IrEnumValue::Int(raw) = value else {
                        return None;
                    };
                    records.push_str(&format!("{raw};"));
                }
                Some(format!("value-enum:int:{}:{}:v1", type_identity, records))
            }
        }
    }

    /// Emit one generated `OrdinalKey` impl for a scalar value enum.
    fn emit_ordinal_value_enum_bridge_impl(spec: OrdinalValueEnumBridgeSpec) -> TokenStream {
        let type_path = spec.type_path;
        let display_name = spec.display_name;
        let encoding = spec.encoding;
        let trait_path = spec.trait_path;
        let error_path = spec.error_path;
        let invalid_record = |detail: TokenStream| {
            quote! {
                #error_path::invalid_key_record(#detail, -1i64)
            }
        };
        let invalid_utf8 = invalid_record(quote! { err.to_string() });
        let invalid_value = invalid_record(quote! {
            format!("invalid value for {}: {}", #display_name, value)
        });
        let invalid_length = invalid_record(quote! {
            format!("{} OrdinalMap key bytes must be 8 bytes", #display_name)
        });

        match spec.value_type {
            IrEnumValueType::String => quote! {
                impl #trait_path for #type_path {
                    fn ordinal_bytes(&self) -> Vec<u8> {
                        self.value().as_bytes().to_vec()
                    }

                    fn ordinal_hash(&self) -> i64 {
                        incan_stdlib::collections::__private::ordinal_key_hash_bytes(self.value().as_bytes())
                    }

                    fn ordinal_bytes_equal(&self, data: Vec<u8>) -> bool {
                        self.value().as_bytes() == data.as_slice()
                    }

                    fn ordinal_encoding() -> String {
                        #encoding.to_string()
                    }

                    fn from_ordinal_bytes(data: Vec<u8>) -> Result<Self, #error_path> {
                        let value = String::from_utf8(data).map_err(|err| #invalid_utf8)?;
                        Self::from_value(value.as_str()).ok_or_else(|| #invalid_value)
                    }
                }
            },
            IrEnumValueType::Int => quote! {
                impl #trait_path for #type_path {
                    fn ordinal_bytes(&self) -> Vec<u8> {
                        self.value().to_le_bytes().to_vec()
                    }

                    fn ordinal_hash(&self) -> i64 {
                        incan_stdlib::collections::__private::ordinal_key_hash_bytes(&self.value().to_le_bytes())
                    }

                    fn ordinal_bytes_equal(&self, data: Vec<u8>) -> bool {
                        data.as_slice() == self.value().to_le_bytes().as_slice()
                    }

                    fn ordinal_encoding() -> String {
                        #encoding.to_string()
                    }

                    fn from_ordinal_bytes(data: Vec<u8>) -> Result<Self, #error_path> {
                        if data.len() != 8 {
                            return Err(#invalid_length);
                        }
                        let mut bytes = [0u8; 8];
                        bytes.copy_from_slice(data.as_slice());
                        let value = i64::from_le_bytes(bytes);
                        Self::from_value(value).ok_or_else(|| #invalid_value)
                    }
                }
            },
        }
    }

    /// Emit `OrdinalKey` impls for value enums when the ordinal-map contract is in scope.
    fn emit_value_enum_ordinal_key_impls(
        &self,
        emitted_declarations: &[&IrDecl],
        local_ordinal_key_trait: bool,
        source_module_name: Option<&str>,
        emit_local: bool,
    ) -> TokenStream {
        let local_trait_path = if local_ordinal_key_trait {
            quote! { OrdinalKey }
        } else {
            Self::ordinal_key_contract_paths().0
        };
        let local_error_path = if local_ordinal_key_trait {
            quote! { OrdinalMapError }
        } else {
            Self::ordinal_key_contract_paths().1
        };

        let mut specs = Vec::new();
        if emit_local {
            for decl in emitted_declarations {
                let IrDeclKind::Enum(e) = &decl.kind else {
                    continue;
                };
                let Some(value_type) = e.value_type else {
                    continue;
                };
                let Some(encoding) = self.value_enum_ordinal_encoding(e, source_module_name) else {
                    continue;
                };
                let name = Self::rust_ident(&e.name);
                specs.push(OrdinalValueEnumBridgeSpec {
                    type_path: quote! { #name },
                    display_name: e.name.clone(),
                    encoding,
                    value_type,
                    trait_path: local_trait_path.clone(),
                    error_path: local_error_path.clone(),
                });
            }
        }

        if !local_ordinal_key_trait {
            let (external_trait_path, external_error_path) = Self::ordinal_key_contract_paths();
            for external in &self.external_ordinal_value_enums {
                let Some(encoding) = Self::external_value_enum_ordinal_encoding(external) else {
                    continue;
                };
                let dependency = Self::rust_ident(&external.dependency_key);
                let name = Self::rust_ident(&external.name);
                specs.push(OrdinalValueEnumBridgeSpec {
                    type_path: quote! { :: #dependency :: #name },
                    display_name: external.name.clone(),
                    encoding,
                    value_type: external.value_type,
                    trait_path: external_trait_path.clone(),
                    error_path: external_error_path.clone(),
                });
            }
        }

        let impls = specs
            .into_iter()
            .map(Self::emit_ordinal_value_enum_bridge_impl)
            .collect::<Vec<_>>();

        quote! { #(#impls)* }
    }

    /// Emit consumer-side `OrdinalKey` impls for user-authored key adopters imported from `.incnlib` dependencies.
    fn emit_external_custom_ordinal_key_impls(&self) -> TokenStream {
        if self.external_ordinal_custom_keys.is_empty() {
            return quote! {};
        }
        let (trait_path, error_path) = Self::ordinal_key_contract_paths();
        let mut impls = Vec::new();
        for external in &self.external_ordinal_custom_keys {
            let dependency = Self::rust_ident(&external.dependency_key);
            let name = Self::rust_ident(&external.name);
            let type_path = quote! { :: #dependency :: #name };
            let hash_body = if external.has_ordinal_hash {
                quote! { #type_path::ordinal_hash(self) }
            } else {
                quote! {
                    incan_stdlib::collections::__private::ordinal_key_hash_bytes(&#type_path::ordinal_bytes(self))
                }
            };
            let bytes_equal_body = if external.has_ordinal_bytes_equal {
                quote! { #type_path::ordinal_bytes_equal(self, data) }
            } else {
                quote! { #type_path::ordinal_bytes(self) == data }
            };
            impls.push(quote! {
                impl #trait_path for #type_path {
                    fn ordinal_bytes(&self) -> Vec<u8> {
                        #type_path::ordinal_bytes(self)
                    }

                    fn ordinal_hash(&self) -> i64 {
                        #hash_body
                    }

                    fn ordinal_bytes_equal(&self, data: Vec<u8>) -> bool {
                        #bytes_equal_body
                    }

                    fn ordinal_encoding() -> String {
                        #type_path::ordinal_encoding()
                    }

                    fn from_ordinal_bytes(data: Vec<u8>) -> Result<Self, #error_path> {
                        match #type_path::from_ordinal_bytes(data) {
                            Ok(value) => Ok(value),
                            Err(err) => Err(#error_path::invalid_key_record(err.message(), err.index())),
                        }
                    }
                }
            });
        }

        quote! { #(#impls)* }
    }

    /// Return the anonymous union shape needed by generated field overlay methods for a concrete struct.
    ///
    /// This mirrors `emit_field_overlay_methods_for_struct()` so the crate-level union definitions are available
    /// before generated impls are emitted. Generic field shapes are skipped because anonymous union definitions are
    /// currently monomorphic.
    fn field_overlay_value_type_from_struct(strukt: &super::super::decl::IrStruct) -> Option<IrType> {
        let mut value_types: Vec<IrType> = strukt
            .fields
            .iter()
            .filter(|field| !field.is_type_private)
            .map(|field| field.ty.clone())
            .collect();
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
    pub(crate) fn collect_union_types_from_type(ty: &IrType, out: &mut HashMap<String, IrType>) {
        if !matches!(ty, IrType::ExternalUnion { .. })
            && let Some(name) = ty.union_type_name()
        {
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
            IrType::ExternalUnion { .. } => {}
            IrType::TypeToken(inner) => Self::collect_union_types_from_type(inner, out),
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
            | IrType::RustDisplay(_)
            | IrType::SelfType
            | IrType::Unknown => {}
        }
    }

    /// Collect anonymous union shapes referenced by an expression tree.
    fn collect_union_types_from_expr(expr: &TypedExpr, out: &mut HashMap<String, IrType>) {
        Self::collect_union_types_from_expr_inner(expr, out, None);
    }

    /// Collect union shapes for an expression while honoring the type position the expression is emitted into.
    ///
    /// Public dependency calls can target provider-owned anonymous unions nested inside containers such as
    /// `list[provider::Union]`. In that case the target type owns the generated wrapper, and the expression's local
    /// semantic union shape must not be collected as a consumer-owned enum definition.
    fn collect_union_types_from_expr_for_target(
        expr: &TypedExpr,
        target_ty: Option<&IrType>,
        out: &mut HashMap<String, IrType>,
    ) {
        if let Some(target_ty) = target_ty {
            Self::collect_union_types_from_type(target_ty, out);
        }
        Self::collect_union_types_from_expr_inner(expr, out, target_ty);
    }

    /// Collect union shapes inside an expression type while preserving provider-owned target ownership.
    fn collect_union_types_from_type_for_target(
        expr_ty: &IrType,
        target_ty: Option<&IrType>,
        out: &mut HashMap<String, IrType>,
    ) {
        let Some(target_ty) = target_ty else {
            Self::collect_union_types_from_type(expr_ty, out);
            return;
        };
        if Self::target_external_union_covers_expr(target_ty, expr_ty) {
            return;
        }
        match (target_ty, expr_ty) {
            (IrType::List(target), IrType::List(expr)) | (IrType::Set(target), IrType::Set(expr)) => {
                Self::collect_union_types_from_type_for_target(expr, Some(target), out);
            }
            (IrType::Option(target), IrType::Option(expr))
            | (IrType::Ref(target), IrType::Ref(expr))
            | (IrType::RefMut(target), IrType::RefMut(expr)) => {
                Self::collect_union_types_from_type_for_target(expr, Some(target), out);
            }
            (IrType::Option(target), expr) => {
                Self::collect_union_types_from_type_for_target(expr, Some(target), out);
            }
            (IrType::Dict(target_key, target_value), IrType::Dict(expr_key, expr_value)) => {
                Self::collect_union_types_from_type_for_target(expr_key, Some(target_key), out);
                Self::collect_union_types_from_type_for_target(expr_value, Some(target_value), out);
            }
            (IrType::Result(target_ok, target_err), IrType::Result(expr_ok, expr_err)) => {
                Self::collect_union_types_from_type_for_target(expr_ok, Some(target_ok), out);
                Self::collect_union_types_from_type_for_target(expr_err, Some(target_err), out);
            }
            (IrType::Tuple(target_items), IrType::Tuple(expr_items)) if target_items.len() == expr_items.len() => {
                for (target, expr) in target_items.iter().zip(expr_items) {
                    Self::collect_union_types_from_type_for_target(expr, Some(target), out);
                }
            }
            _ => Self::collect_union_types_from_type(expr_ty, out),
        }
    }

    /// Return whether a target type covers an expression's union shape through a provider-owned external union.
    ///
    /// The check is structural rather than top-level only so containers, options, tuples, dictionaries, and result
    /// payloads can preserve public dependency ownership across nested argument and collection positions.
    fn target_external_union_covers_expr(target_ty: &IrType, expr_ty: &IrType) -> bool {
        match (target_ty, expr_ty) {
            (
                IrType::ExternalUnion { library, .. },
                IrType::ExternalUnion {
                    library: expr_library, ..
                },
            ) if library != expr_library => false,
            (IrType::ExternalUnion { library, .. }, _) => {
                target_ty.union_type_name() == expr_ty.provider_localized(library).union_type_name()
            }
            (IrType::List(target), IrType::List(expr)) | (IrType::Set(target), IrType::Set(expr)) => {
                Self::target_external_union_covers_expr(target, expr)
            }
            (IrType::Option(target), IrType::Option(expr))
            | (IrType::Ref(target), IrType::Ref(expr))
            | (IrType::RefMut(target), IrType::RefMut(expr)) => Self::target_external_union_covers_expr(target, expr),
            (IrType::Option(target), expr) => Self::target_external_union_covers_expr(target, expr),
            (IrType::Dict(target_key, target_value), IrType::Dict(expr_key, expr_value)) => {
                Self::target_external_union_covers_expr(target_key, expr_key)
                    && Self::target_external_union_covers_expr(target_value, expr_value)
            }
            (IrType::Result(target_ok, target_err), IrType::Result(expr_ok, expr_err)) => {
                Self::target_external_union_covers_expr(target_ok, expr_ok)
                    && Self::target_external_union_covers_expr(target_err, expr_err)
            }
            (IrType::Tuple(target_items), IrType::Tuple(expr_items)) if target_items.len() == expr_items.len() => {
                target_items
                    .iter()
                    .zip(expr_items)
                    .all(|(target, expr)| Self::target_external_union_covers_expr(target, expr))
            }
            _ => false,
        }
    }

    /// Return the target type for one list or set element when a collection expression has an expected type.
    fn list_element_target_type(target_ty: Option<&IrType>) -> Option<&IrType> {
        match target_ty {
            Some(IrType::List(inner) | IrType::Set(inner)) => Some(inner),
            _ => None,
        }
    }

    /// Return expected key and value types for a dictionary expression when the surrounding type provides them.
    fn dict_target_types(target_ty: Option<&IrType>) -> (Option<&IrType>, Option<&IrType>) {
        match target_ty {
            Some(IrType::Dict(key, value)) => (Some(key), Some(value)),
            _ => (None, None),
        }
    }

    /// Return the expected item type for one tuple position when the surrounding type provides it.
    fn tuple_item_target_type(target_ty: Option<&IrType>, index: usize) -> Option<&IrType> {
        match target_ty {
            Some(IrType::Tuple(items)) => items.get(index),
            _ => None,
        }
    }

    /// Return the expected argument type from a callable signature for a positional or named call argument.
    fn call_arg_target_type<'sig>(
        arg: &IrCallArg,
        index: usize,
        signature: Option<&'sig FunctionSignature>,
    ) -> Option<&'sig IrType> {
        let signature = signature?;
        if let Some(name) = &arg.name
            && let Some(param) = signature.params.iter().find(|param| param.name == *name)
        {
            return Some(&param.ty);
        }
        signature.params.get(index).map(|param| &param.ty)
    }

    /// Collect union shapes from callable defaults that may be emitted as missing call arguments.
    fn collect_union_types_from_signature_defaults(signature: &FunctionSignature, out: &mut HashMap<String, IrType>) {
        for param in &signature.params {
            if let Some(FunctionParamDefault::Source(default)) = &param.default {
                Self::collect_union_types_from_expr_for_target(default, Some(&param.ty), out);
            }
        }
    }

    /// Collect union shapes from an expression tree, optionally using an expected target type to keep ownership stable.
    fn collect_union_types_from_expr_inner(
        expr: &TypedExpr,
        out: &mut HashMap<String, IrType>,
        target_ty: Option<&IrType>,
    ) {
        Self::collect_union_types_from_type_for_target(&expr.ty, target_ty, out);
        match &expr.kind {
            IrExprKind::Call {
                func,
                args,
                callable_signature,
                ..
            } => {
                if let Some(callable_signature) = callable_signature {
                    Self::collect_union_types_from_signature_defaults(callable_signature, out);
                } else {
                    Self::collect_union_types_from_call_callee(func, out);
                }
                for (index, arg) in args.iter().enumerate() {
                    Self::collect_union_types_from_expr_for_target(
                        &arg.expr,
                        Self::call_arg_target_type(arg, index, callable_signature.as_ref()),
                        out,
                    );
                }
            }
            IrExprKind::BuiltinCall { args, .. } => {
                for arg in args {
                    Self::collect_union_types_from_expr(arg, out);
                }
            }
            IrExprKind::MethodCall {
                receiver,
                args,
                callable_signature,
                ..
            } => {
                Self::collect_union_types_from_expr(receiver, out);
                for (index, arg) in args.iter().enumerate() {
                    Self::collect_union_types_from_expr_for_target(
                        &arg.expr,
                        Self::call_arg_target_type(arg, index, callable_signature.as_ref()),
                        out,
                    );
                }
            }
            IrExprKind::KnownMethodCall { receiver, args, .. } => {
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
            IrExprKind::RegisterCallableName { callable, .. } => Self::collect_union_types_from_expr(callable, out),
            IrExprKind::CacheGenericDecoratedFunction { value, .. } => {
                Self::collect_union_types_from_expr(value, out);
            }
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
                let element_target_ty = Self::list_element_target_type(target_ty);
                for item in items {
                    match item {
                        IrListEntry::Element(value) | IrListEntry::Spread(value) => {
                            let item_target_ty = match item {
                                IrListEntry::Element(_) => element_target_ty,
                                IrListEntry::Spread(_) => target_ty,
                            };
                            Self::collect_union_types_from_expr_for_target(value, item_target_ty, out);
                        }
                    }
                }
            }
            IrExprKind::Dict(entries) => {
                let (key_target_ty, value_target_ty) = Self::dict_target_types(target_ty);
                for entry in entries {
                    match entry {
                        IrDictEntry::Pair(key, value) => {
                            Self::collect_union_types_from_expr_for_target(key, key_target_ty, out);
                            Self::collect_union_types_from_expr_for_target(value, value_target_ty, out);
                        }
                        IrDictEntry::Spread(value) => {
                            Self::collect_union_types_from_expr_for_target(value, target_ty, out)
                        }
                    }
                }
            }
            IrExprKind::Set(items) => {
                let element_target_ty = Self::list_element_target_type(target_ty);
                for item in items {
                    Self::collect_union_types_from_expr_for_target(item, element_target_ty, out);
                }
            }
            IrExprKind::Tuple(items) => {
                for (index, item) in items.iter().enumerate() {
                    Self::collect_union_types_from_expr_for_target(
                        item,
                        Self::tuple_item_target_type(target_ty, index),
                        out,
                    );
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
                    for binding in &arm.bindings {
                        Self::collect_union_types_from_type(&binding.ty, out);
                        Self::collect_union_types_from_expr(&binding.value, out);
                        if let Some(guard_value) = &binding.guard_value {
                            Self::collect_union_types_from_expr(guard_value, out);
                        }
                    }
                    if let Some(guard) = &arm.guard {
                        Self::collect_union_types_from_expr(guard, out);
                    }
                    Self::collect_union_types_from_expr(&arm.body, out);
                }
            }
            IrExprKind::Race { arms, .. } => {
                for arm in arms {
                    Self::collect_union_types_from_expr(&arm.awaitable, out);
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
                    if let super::super::expr::FormatPart::Expr { expr, .. } = part {
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
            | IrExprKind::IntLiteral(_)
            | IrExprKind::Float(_)
            | IrExprKind::Decimal(_)
            | IrExprKind::String(_)
            | IrExprKind::Bytes(_)
            | IrExprKind::AssociatedFunction { .. }
            | IrExprKind::FunctionItem { .. }
            | IrExprKind::TypeToken { .. }
            | IrExprKind::Var { .. }
            | IrExprKind::StaticRead { .. }
            | IrExprKind::StaticBinding { .. }
            | IrExprKind::Literal(_)
            | IrExprKind::FieldsList(_)
            | IrExprKind::SerdeToJson
            | IrExprKind::SerdeFromJson(_) => {}
        }
    }

    /// Collect anonymous unions needed by a call callee expression without treating the callee's own function type as
    /// an emitted type position.
    ///
    /// Imported public helpers can carry function signatures that mention dependency-owned anonymous unions. Those
    /// signatures guide argument planning, but the function type itself is not printed into the generated Rust call.
    /// Only nested value expressions inside the callee need collection.
    fn collect_union_types_from_call_callee(expr: &TypedExpr, out: &mut HashMap<String, IrType>) {
        match &expr.kind {
            IrExprKind::Field { object, .. } => Self::collect_union_types_from_expr(object, out),
            IrExprKind::Index { object, index } => {
                Self::collect_union_types_from_expr(object, out);
                Self::collect_union_types_from_expr(index, out);
            }
            IrExprKind::Call { func, args, .. } => {
                Self::collect_union_types_from_call_callee(func, out);
                for arg in args {
                    Self::collect_union_types_from_expr(&arg.expr, out);
                }
            }
            IrExprKind::MethodCall { receiver, args, .. } => {
                Self::collect_union_types_from_expr(receiver, out);
                for arg in args {
                    Self::collect_union_types_from_expr(&arg.expr, out);
                }
            }
            IrExprKind::Var { .. } | IrExprKind::Literal(_) => {}
            _ => Self::collect_union_types_from_expr(expr, out),
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
                    for binding in &arm.bindings {
                        Self::collect_union_types_from_type(&binding.ty, out);
                        Self::collect_union_types_from_expr(&binding.value, out);
                        if let Some(guard_value) = &binding.guard_value {
                            Self::collect_union_types_from_expr(guard_value, out);
                        }
                    }
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
                    if let Some(FunctionParamDefault::Source(default)) = &param.default {
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
                for associated_type in &impl_block.associated_types {
                    Self::collect_union_types_from_type(&associated_type.ty, out);
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
        let ty = self.resolve_type_aliases_for_emit(ty);
        if matches!(ty, IrType::ExternalUnion { .. }) {
            return None;
        }
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
            #[derive(Debug, Clone)]
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
                .emit_dependency_type_path(name)
                .or_else(|| self.emit_public_dependency_type_path(name))
                .unwrap_or_else(|| self.emit_type(ty)),
            IrType::NamedGeneric(name, args) if name == super::super::types::IR_UNION_TYPE_NAME => {
                self.emit_union_type_path(ty)
            }
            IrType::NamedGeneric(name, args) => {
                let base = self
                    .emit_dependency_type_path(name)
                    .or_else(|| self.emit_public_dependency_type_path(name))
                    .unwrap_or_else(|| {
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
            IrType::TypeToken(inner) => {
                let inner = self.emit_generated_union_member_type(inner);
                quote! { incan_stdlib::reflection::TypeToken<#inner> }
            }
            IrType::ExternalUnion { .. } => self.emit_type(ty),
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
            | IrType::RustDisplay(_)
            | IrType::SelfType
            | IrType::Unknown => self.emit_type(ty),
        }
    }

    /// Emit a complete IR program to formatted Rust code.
    #[tracing::instrument(skip_all, fields(decl_count = program.declarations.len()))]
    pub fn emit_program(&mut self, program: &IrProgram) -> Result<String, EmitError> {
        self.iterator_sum_used.replace(false);
        self.const_bindings.clear();
        self.local_nominal_type_names = program
            .declarations
            .iter()
            .filter_map(|decl| match &decl.kind {
                IrDeclKind::Struct(strukt) => Some(strukt.name.clone()),
                IrDeclKind::Enum(enum_) => Some(enum_.name.clone()),
                IrDeclKind::Trait(trait_decl) => Some(trait_decl.name.clone()),
                IrDeclKind::TypeAlias { name, .. } | IrDeclKind::SymbolAlias { name, .. } => Some(name.clone()),
                IrDeclKind::Function(_)
                | IrDeclKind::Const { .. }
                | IrDeclKind::Static { .. }
                | IrDeclKind::Import { .. }
                | IrDeclKind::Impl(_) => None,
            })
            .collect();
        // RFC 023: propagate rust.module() path from IR to emitter for @rust.extern delegation.
        if self.rust_module_path.is_none() {
            self.rust_module_path = program.rust_module_path.clone();
        }
        self.bind_source_dependency_constructor_metadata(program);
        self.bind_public_dependency_constructor_metadata(program);
        self.seed_nominal_metadata_from_program(program);
        self.newtype_construction = program.newtype_construction.clone();

        // First pass: collect struct derives, struct field types, and enum variant typing
        let mut static_str_const_exprs: HashMap<String, TypedExpr> = HashMap::new();
        for decl in &program.declarations {
            if let IrDeclKind::Struct(s) = &decl.kind {
                self.register_struct_constructor_metadata(s);
                if !s.derives.is_empty() {
                    self.struct_derives.insert(s.name.clone(), s.derives.clone());
                }
                self.struct_field_names
                    .insert(s.name.clone(), s.fields.iter().map(|f| f.name.clone()).collect());
                for field in &s.fields {
                    let key = (s.name.clone(), field.name.clone());
                    self.struct_field_types.insert(key.clone(), field.ty.clone());
                    self.struct_field_surface_type_names
                        .insert(key.clone(), field.surface_type_name.clone());
                    if field.is_type_private {
                        self.struct_type_private_fields.insert(key.clone());
                    }
                    self.struct_field_aliases.insert(key.clone(), field.alias.clone());
                    self.struct_field_descriptions
                        .insert(key.clone(), field.description.clone());
                    if let Some(default) = &field.default {
                        self.struct_field_defaults.insert(key, default.clone());
                    }
                }
            }
            if let IrDeclKind::Enum(e) = &decl.kind {
                for v in &e.variants {
                    self.enum_variant_fields
                        .insert((e.name.clone(), v.name.clone()), v.fields.clone());
                }
                for alias in &e.variant_aliases {
                    self.enum_variant_aliases
                        .insert((e.name.clone(), alias.name.clone()), alias.target.clone());
                }
            }
            if let IrDeclKind::TypeAlias {
                name,
                type_params,
                ty,
                is_rusttype,
                ..
            } = &decl.kind
                && type_params.is_empty()
                && !is_rusttype
            {
                self.type_aliases.insert(name.clone(), ty.clone());
            }
            if let IrDeclKind::TypeAlias {
                name,
                is_rusttype: true,
                ..
            } = &decl.kind
            {
                self.rusttype_alias_names.insert(name.clone());
            }
            // Collect const initializer expressions before emission. String folding and const representability both
            // need the declared target type when one checked const flows through another model constant.
            if let IrDeclKind::Const { name, ty, value, .. } = &decl.kind {
                self.const_bindings.insert(name.clone(), (ty.clone(), value.clone()));
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

    /// Collect callable-name use facts for a whole IR program.
    pub(crate) fn callable_name_use_facts_for_program(
        program: &IrProgram,
        externally_reachable_items: &HashSet<String>,
        preserve_public_items: bool,
    ) -> CallableNameUseFacts {
        let analysis = GeneratedUseAnalyzer::analyze(program, externally_reachable_items, preserve_public_items);
        CallableNameUseFacts {
            signature_keys: analysis.callable_name_signature_keys,
            function_arg_signature_keys: analysis.callable_name_function_arg_signature_keys,
            generic_trait_used: analysis.uses_generic_callable_name_trait,
        }
    }

    /// Return the callable-name signature metadata for a helper key.
    fn callable_name_signature_for_key(&self, key: &str) -> Option<(Vec<IrType>, IrType)> {
        self.callable_name_local_registry()
            .iter()
            .find_map(|(_, signature)| {
                let params = signature
                    .params
                    .iter()
                    .map(|param| param.ty.clone())
                    .collect::<Vec<_>>();
                (Self::callable_name_signature_key(&params, &signature.return_type).as_deref() == Some(key))
                    .then(|| (params, signature.return_type.clone()))
            })
            .or_else(|| {
                self.callable_name_resolutions
                    .get(key)
                    .map(|resolution| (resolution.params.clone(), resolution.ret.clone()))
            })
    }

    /// Return helper keys needed for callable-name resolution.
    fn callable_name_helper_keys(
        &self,
        local_callable_name_signature_keys: &HashSet<String>,
        include_generic_callable_signatures: bool,
    ) -> Vec<String> {
        let mut keys = local_callable_name_signature_keys.clone();
        if include_generic_callable_signatures {
            keys.extend(self.callable_name_used_signature_keys.iter().filter_map(|key| {
                self.callable_name_signature_for_key(key)
                    .is_some()
                    .then_some(key.clone())
            }));
        }
        for (key, resolution) in &self.callable_name_resolutions {
            if self.callable_name_used_signature_keys.contains(key)
                && resolution
                    .module_paths
                    .contains(&self.callable_name_current_module_path)
            {
                keys.insert(key.clone());
            }
        }
        let mut keys = keys.into_iter().collect::<Vec<_>>();
        keys.sort();
        keys
    }

    /// Build a callable-name resolution expression with a source-name fallback.
    fn callable_name_resolution_expr_with_fallback(
        &self,
        key: &str,
        callable_tokens: TokenStream,
        fallback: TokenStream,
    ) -> TokenStream {
        let helper = Self::callable_name_helper_ident(key);
        let mut helper_calls = Vec::new();
        helper_calls.push(quote! { #helper(#callable_tokens) });
        if let Some(resolution) = self.callable_name_resolutions.get(key) {
            for module_path in &resolution.module_paths {
                if module_path == &self.callable_name_current_module_path {
                    continue;
                }
                if module_path.is_empty() && !self.callable_name_current_module_path.is_empty() {
                    continue;
                }
                let helper_path = self.emit_callable_name_helper_path(module_path, key);
                helper_calls.push(quote! { #helper_path(#callable_tokens) });
            }
        }
        let mut resolved = fallback;
        for helper_call in helper_calls.into_iter().rev() {
            resolved = quote! {
                if let Some(__incan_name) = #helper_call {
                    __incan_name.to_string()
                } else {
                    #resolved
                }
            };
        }
        resolved
    }

    /// Emit the trait used for generic callable-name reflection.
    fn emit_generic_callable_name_trait(&self, keys: &[String]) -> Option<TokenStream> {
        if keys.is_empty() {
            return None;
        }
        let trait_ident = Self::rust_ident("__IncanCallableName");
        let mut grouped_keys: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for key in keys {
            let Some((params, ret)) = self.callable_name_signature_for_key(key) else {
                continue;
            };
            let resolved_params = params
                .iter()
                .map(|param| self.resolve_type_aliases_for_emit(param))
                .collect::<Vec<_>>();
            let resolved_ret = self.resolve_type_aliases_for_emit(&ret);
            let Some(resolved_key) = Self::callable_name_signature_key(&resolved_params, &resolved_ret) else {
                continue;
            };
            grouped_keys.entry(resolved_key).or_default().push(key.clone());
        }

        let impls = grouped_keys
            .values_mut()
            .filter_map(|keys| {
                keys.sort();
                let primary_key = keys.first()?;
                let (params, ret) = self.callable_name_signature_for_key(primary_key)?;
                let fn_ty = self.emit_callable_fn_type(&params, &ret);
                let fallback = proc_macro2::Literal::string("<callable>");
                let mut resolved = quote! { #fallback.to_string() };
                for key in keys.iter().rev() {
                    resolved =
                        self.callable_name_resolution_expr_with_fallback(key, quote! { __incan_callable }, resolved);
                }
                Some(quote! {
                    impl #trait_ident for #fn_ty {
                        fn __incan_callable_name(&self) -> String {
                            let __incan_callable: #fn_ty = *self;
                            #resolved
                        }
                    }
                })
            })
            .collect::<Vec<_>>();
        if impls.is_empty() {
            return None;
        }
        Some(quote! {
            pub trait #trait_ident {
                fn __incan_callable_name(&self) -> String;
            }

            #(#impls)*
        })
    }

    /// Emit generated callable-name helper functions.
    fn emit_callable_name_helpers(
        &self,
        emitted_callable_names: &HashSet<String>,
        dynamic_only_callable_names: &HashSet<String>,
        keys: &[String],
    ) -> Vec<TokenStream> {
        keys.iter()
            .filter_map(|key| {
                let (params, ret) = self.callable_name_signature_for_key(key)?;
                let helper = Self::callable_name_helper_ident(key);
                let registry = Self::callable_name_registry_ident(key);
                let register = Self::callable_name_register_ident(key);
                let fn_ty = self.emit_callable_fn_type(&params, &ret);
                let mut candidates = self
                    .callable_name_local_registry()
                    .iter()
                    .filter(|(name, signature)| {
                        emitted_callable_names.contains(*name)
                            && !dynamic_only_callable_names.contains(*name)
                            && signature.params.len() == params.len()
                            && signature.params.iter().map(|param| &param.ty).eq(params.iter())
                            && signature.return_type == ret
                    })
                    .map(|(name, _)| {
                        let source_name = self.callable_name_local_registry().source_name(name).unwrap_or(name);
                        (name.clone(), source_name.to_string())
                    })
                    .collect::<Vec<_>>();
                candidates.sort_by(|left, right| left.0.cmp(&right.0));

                let dynamic_lookup = quote! {{
                    let __incan_entries = #registry()
                        .lock()
                        .unwrap_or_else(|__incan_poisoned| __incan_poisoned.into_inner());
                    __incan_entries.iter().rev().find_map(|(__incan_registered, __incan_name)| {
                        if std::ptr::fn_addr_eq(*__incan_registered, callable) {
                            Some(*__incan_name)
                        } else {
                            None
                        }
                    })
                }};
                let mut body = dynamic_lookup;
                for (candidate, source_name) in candidates.into_iter().rev() {
                    let candidate_ident = self.rust_function_ident(&candidate);
                    let source_literal = proc_macro2::Literal::string(&source_name);
                    body = quote! {
                        if std::ptr::fn_addr_eq(callable, #candidate_ident as #fn_ty) {
                            Some(#source_literal)
                        } else {
                            #body
                        }
                    };
                }

                let visibility = if self.callable_name_resolutions.get(key).is_some_and(|resolution| {
                    self.callable_name_used_signature_keys.contains(key)
                        && resolution
                            .module_paths
                            .contains(&self.callable_name_current_module_path)
                }) {
                    quote! { pub(crate) }
                } else {
                    quote! {}
                };
                let private_interfaces_allow = (!visibility.is_empty()).then(|| {
                    quote! { #[allow(private_interfaces)] }
                });

                Some(quote! {
                    fn #registry() -> &'static std::sync::Mutex<Vec<(#fn_ty, &'static str)>> {
                        static __INCAN_CALLABLE_NAMES:
                            std::sync::OnceLock<std::sync::Mutex<Vec<(#fn_ty, &'static str)>>> =
                            std::sync::OnceLock::new();
                        __INCAN_CALLABLE_NAMES.get_or_init(|| std::sync::Mutex::new(Vec::new()))
                    }

                    fn #register(callable: #fn_ty, source_name: &'static str) {
                        let mut __incan_entries = #registry()
                            .lock()
                            .unwrap_or_else(|__incan_poisoned| __incan_poisoned.into_inner());
                        if let Some((_, __incan_name)) = __incan_entries
                            .iter_mut()
                            .find(|(__incan_registered, _)| std::ptr::fn_addr_eq(*__incan_registered, callable))
                        {
                            *__incan_name = source_name;
                        } else {
                            __incan_entries.push((callable, source_name));
                        }
                    }

                    #private_interfaces_allow
                    #visibility fn #helper(callable: #fn_ty) -> Option<&'static str> {
                        #body
                    }
                })
            })
            .collect()
    }

    /// Emit a program to TokenStream (without formatting).
    pub fn emit_program_tokens(&self, program: &IrProgram) -> Result<TokenStream, EmitError> {
        self.set_static_projections(program)?;
        let mut items = Vec::new();
        let analysis =
            GeneratedUseAnalyzer::analyze(program, &self.externally_reachable_items, self.preserve_public_items);
        let result_observer_callable_types = analysis.result_observer_callable_types.clone();
        let borrowed_function_adapters = analysis.borrowed_function_adapters.clone();
        let local_callable_name_signature_keys = analysis.callable_name_signature_keys.clone();
        let uses_generic_callable_name_trait = analysis.uses_generic_callable_name_trait;
        self.set_result_observer_callable_types(result_observer_callable_types);
        self.set_borrowed_function_adapters(borrowed_function_adapters);
        self.set_generated_use_analysis(analysis);
        let callable_name_helper_keys =
            self.callable_name_helper_keys(&local_callable_name_signature_keys, uses_generic_callable_name_trait);

        let emitted_declarations: Vec<&IrDecl> = program
            .declarations
            .iter()
            .filter(|decl| self.should_emit_decl(decl))
            .collect();
        let static_declarations: Vec<(&str, &IrStaticProvenance)> = emitted_declarations
            .iter()
            .filter_map(|decl| match &decl.kind {
                IrDeclKind::Static { name, provenance, .. } => Some((name.as_str(), provenance)),
                _ => None,
            })
            .collect();
        // `program.module_init` is compiler-generated work such as `@describe` registration. It uses the same
        // once-only helper as static initialization, and must run when a callable from an otherwise static-free
        // contributing module is invoked.
        *self.module_needs_initialization.borrow_mut() =
            !static_declarations.is_empty() || !program.module_init.is_empty();
        let (imported_static_init_bindings, imported_static_module_init_bindings) =
            self.collect_imported_static_init_bindings(&emitted_declarations);
        self.set_imported_static_init_bindings(imported_static_init_bindings);
        self.set_imported_static_module_init_bindings(imported_static_module_init_bindings);

        if self.emit_strict_generated_lint_denies {
            items.push(quote! {
                #![deny(unused_imports, dead_code, unused_variables)]
            });
        }

        let compiler_version = crate::version::INCAN_VERSION;
        items.push(quote! { incan_stdlib::__incan_stdlib_version_check!(#compiler_version); });
        if program.uses_checked_c_strings {
            items.push(Self::emit_checked_c_string_constructor());
        }
        if program.uses_scoped_c_string_views {
            items.push(Self::emit_checked_c_scoped_string_copy());
        }
        if program.uses_checked_c_span_buffers {
            items.push(Self::emit_checked_c_span_finish());
        }
        items.extend(Self::emit_checked_c_functions(&program.checked_c_functions));

        let needs_json_serialize_trait_scope = emitted_declarations.iter().any(|decl| {
            matches!(
                &decl.kind,
                IrDeclKind::Impl(impl_block)
                    if impl_block.trait_name
                        .as_deref()
                        .and_then(incan_core::lang::stdlib::stdlib_json_trait_scope_import_id)
                        == Some(incan_core::lang::stdlib::StdlibJsonTraitId::Serialize)
            )
        });
        let needs_json_deserialize_trait_scope = emitted_declarations.iter().any(|decl| {
            matches!(
                &decl.kind,
                IrDeclKind::Impl(impl_block)
                    if impl_block.trait_name
                        .as_deref()
                        .and_then(incan_core::lang::stdlib::stdlib_json_trait_scope_import_id)
                        == Some(incan_core::lang::stdlib::StdlibJsonTraitId::Deserialize)
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
            let mut canonical_union_types = HashMap::new();
            for (_, union_ty) in union_types {
                let union_ty = self.resolve_type_aliases_for_emit(&union_ty);
                if matches!(union_ty, IrType::ExternalUnion { .. }) {
                    continue;
                }
                if let Some(name) = union_ty.union_type_name() {
                    canonical_union_types.insert(name, union_ty);
                }
            }
            let mut union_type_items: Vec<_> = canonical_union_types.into_iter().collect();
            union_type_items.sort_by(|(left, _), (right, _)| left.cmp(right));
            for (_, union_ty) in union_type_items {
                if let Some(item) = self.emit_generated_union_type(&union_ty) {
                    items.push(item);
                }
            }
        }

        // RFC 052: force declaration-order static initialization once per module before any static access helper call.
        let imported_static_init_calls: Vec<TokenStream> = self
            .imported_static_module_init_bindings
            .borrow()
            .iter()
            .map(|name| {
                let ident = Self::imported_static_init_ident(name);
                quote! { #ident(); }
            })
            .collect();
        let previous_static_initializer = self.in_static_initializer.replace(true);
        let module_init_stmts = self.emit_stmts(&program.module_init)?;
        self.in_static_initializer.replace(previous_static_initializer);
        if !static_declarations.is_empty() || !imported_static_init_calls.is_empty() || !module_init_stmts.is_empty() {
            let force_calls: Vec<TokenStream> = static_declarations
                .iter()
                .map(|(name, provenance)| {
                    self.rust_static_declaration_ident(name, provenance)
                        .map(|ident| quote! { std::sync::LazyLock::force(&#ident); })
                })
                .collect::<Result<_, _>>()?;
            items.push(quote! {
                #[inline(always)]
                pub(crate) fn __incan_init_module_statics() {
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
                        #(#imported_static_init_calls)*
                        #(#force_calls)*
                        #(#module_init_stmts)*
                    });
                }
            });
        }

        let emitted_callable_names: HashSet<String> = emitted_declarations
            .iter()
            .filter_map(|decl| match &decl.kind {
                IrDeclKind::Function(func) => Some(func.name.clone()),
                IrDeclKind::SymbolAlias { name, .. } => Some(name.clone()),
                _ => None,
            })
            .collect();
        let dynamic_only_callable_names: HashSet<String> = emitted_declarations
            .iter()
            .filter_map(|decl| match &decl.kind {
                IrDeclKind::Function(func) if func.is_async || !func.type_params.is_empty() => Some(func.name.clone()),
                _ => None,
            })
            .collect();
        items.extend(self.emit_callable_name_helpers(
            &emitted_callable_names,
            &dynamic_only_callable_names,
            &callable_name_helper_keys,
        ));
        if uses_generic_callable_name_trait
            && let Some(trait_item) = self.emit_generic_callable_name_trait(&callable_name_helper_keys)
        {
            items.push(trait_item);
        }

        // Emit all declarations.
        let defines_ordinal_key_trait = Self::emitted_declarations_define_capability_trait(
            program,
            &emitted_declarations,
            trait_capabilities::stable_ordinal_key(),
        );
        let defines_string_try_from_trait = Self::emitted_declarations_define_capability_trait(
            program,
            &emitted_declarations,
            trait_capabilities::string_try_from(),
        );
        let imports_std_ordinal_contract =
            Self::emitted_declarations_import_std_collections_ordinal_contract(&emitted_declarations);
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
        for decl in &emitted_declarations {
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
        if defines_string_try_from_trait {
            items.push(self.emit_builtin_string_try_from_impls());
        }
        items.push(self.emit_builtin_iterator_sum_impls(&emitted_declarations, program));
        items.push(self.emit_local_newtype_iterator_sum_impls(*self.iterator_sum_used.borrow()));
        items.push(self.emit_local_newtype_string_try_from_impls(&emitted_declarations));
        if defines_ordinal_key_trait {
            items.push(self.emit_builtin_ordinal_key_impls());
        }
        let emit_local_ordinal_value_enums =
            defines_ordinal_key_trait || imports_std_ordinal_contract || self.emit_std_ordinal_value_enum_impls;
        items.push(self.emit_value_enum_ordinal_key_impls(
            &emitted_declarations,
            defines_ordinal_key_trait,
            program.source_module_name.as_deref(),
            emit_local_ordinal_value_enums,
        ));
        if !defines_ordinal_key_trait {
            items.push(self.emit_external_custom_ordinal_key_impls());
        }
        items.extend(Self::emit_registered_generated_module_supports(program));

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

#[cfg(test)]
mod tests {
    use super::IrEmitter;
    use crate::backend::ir::types::{IR_UNION_TYPE_NAME, IrType};
    use crate::backend::ir::{IrCheckedCFunction, IrCheckedCType, IrProgram};
    use crate::frontend::typechecker::COutputMode;
    use incan_core::lang::c_abi::{LinkCapabilityId, ScalarTypeId};
    use std::collections::HashMap;

    fn union(members: Vec<IrType>) -> IrType {
        IrType::NamedGeneric(IR_UNION_TYPE_NAME.to_string(), members)
    }

    fn provider_union() -> IrType {
        union(vec![IrType::Struct("ProviderColumn".to_string()), IrType::Int])
    }

    fn external_provider_union() -> IrType {
        IrType::ExternalUnion {
            library: "provider".to_string(),
            union: Box::new(provider_union()),
        }
    }

    fn local_union() -> IrType {
        union(vec![IrType::Struct("LocalValue".to_string()), IrType::String])
    }

    #[test]
    fn external_union_coverage_requires_all_compound_slots_to_match() {
        assert!(IrEmitter::target_external_union_covers_expr(
            &IrType::Tuple(vec![external_provider_union(), external_provider_union()]),
            &IrType::Tuple(vec![provider_union(), provider_union()])
        ));
        assert!(!IrEmitter::target_external_union_covers_expr(
            &IrType::Tuple(vec![external_provider_union(), IrType::String]),
            &IrType::Tuple(vec![provider_union(), local_union()])
        ));
        assert!(!IrEmitter::target_external_union_covers_expr(
            &IrType::Dict(Box::new(external_provider_union()), Box::new(IrType::String)),
            &IrType::Dict(Box::new(provider_union()), Box::new(local_union()))
        ));
        assert!(!IrEmitter::target_external_union_covers_expr(
            &IrType::Result(Box::new(external_provider_union()), Box::new(IrType::String)),
            &IrType::Result(Box::new(provider_union()), Box::new(local_union()))
        ));
    }

    #[test]
    fn external_union_coverage_localizes_only_the_target_provider_issue892() {
        let qualified_provider_union = union(vec![
            IrType::Struct("provider::ProviderColumn".to_string()),
            IrType::Int,
        ]);
        let foreign_union = IrType::ExternalUnion {
            library: "other".to_string(),
            union: Box::new(qualified_provider_union.clone()),
        };

        assert!(IrEmitter::target_external_union_covers_expr(
            &external_provider_union(),
            &qualified_provider_union
        ));
        assert!(!IrEmitter::target_external_union_covers_expr(
            &external_provider_union(),
            &foreign_union
        ));
    }

    #[test]
    fn targeted_union_collection_keeps_uncovered_local_compound_slots() -> Result<(), String> {
        let target = IrType::Tuple(vec![external_provider_union(), IrType::String]);
        let expr_ty = IrType::Tuple(vec![provider_union(), local_union()]);
        let mut collected = HashMap::new();

        IrEmitter::collect_union_types_from_type_for_target(&expr_ty, Some(&target), &mut collected);
        let provider_name = provider_union()
            .union_type_name()
            .ok_or("provider union should have a generated name")?;
        let local_name = local_union()
            .union_type_name()
            .ok_or("local union should have a generated name")?;

        assert!(
            !collected.contains_key(&provider_name),
            "provider-owned union should not be re-collected locally"
        );
        assert!(
            collected.contains_key(&local_name),
            "uncovered local union sibling should still be collected"
        );
        Ok(())
    }

    /// Regression for #1203: crate-root union definitions keep public-provider payloads qualified without an import.
    #[test]
    fn generated_union_qualifies_public_provider_payloads_issue1203() -> Result<(), String> {
        let program = IrProgram::new();
        let mut emitter = IrEmitter::new(&program.function_registry);
        emitter.public_dependency_type_paths.insert(
            "ProviderPayload".to_string(),
            vec!["provider".to_string(), "public_types".to_string()],
        );
        let rendered = emitter
            .emit_generated_union_type(&union(vec![
                IrType::Struct("ProviderPayload".to_string()),
                IrType::NamedGeneric(
                    "Envelope".to_string(),
                    vec![IrType::Struct("ProviderPayload".to_string())],
                ),
            ]))
            .ok_or("expected an anonymous union definition")?
            .to_string();

        assert!(
            rendered.contains("provider :: public_types :: ProviderPayload"),
            "expected the public provider payload to stay qualified: {rendered}"
        );
        assert!(
            rendered.contains("Envelope < provider :: public_types :: ProviderPayload >"),
            "expected nested public provider payloads to stay qualified: {rendered}"
        );
        Ok(())
    }

    #[test]
    fn checked_c_function_emits_exact_ffi_signature_and_private_wrapper() -> Result<(), String> {
        let mut program = IrProgram::new();
        program.checked_c_functions.push(IrCheckedCFunction {
            binding: "Fixture".to_string(),
            symbol: "absolute".to_string(),
            native_symbol: "abs".to_string(),
            system_library: "c".to_string(),
            link_capability: LinkCapabilityId::SystemLibrary,
            parameters: vec![IrCheckedCType::Scalar(ScalarTypeId::I32)],
            parameter_names: vec!["value".to_string()],
            return_type: IrCheckedCType::Scalar(ScalarTypeId::I32),
            resources: Vec::new(),
        });
        let mut emitter = IrEmitter::new(&program.function_registry);
        let generated = emitter.emit_program(&program).map_err(|error| error.to_string())?;
        let normalized = generated.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();
        assert!(normalized.contains("#[link(name=\"c\")]unsafeextern\"C\""));
        assert!(normalized.contains("#[link_name=\"abs\"]fn__incan_c_Fixture__absolute__ffi(__incan_arg_0:i32)->i32;"));
        assert!(normalized.contains("fn__incan_c_Fixture__absolute<__IncanCheckedCArg0:"));
        assert!(normalized.contains("::core::convert::TryInto<i32>"));
        assert!(normalized.contains("(__incan_arg_0:__IncanCheckedCArg0,)->i32"));
        assert!(normalized.contains("TryInto<i32>>::try_into(__incan_arg_0)"));
        assert!(
            !normalized.contains("try_from(__incan_arg_0)"),
            "an exact c.i32 carrier must not normalize its argument through another integer type"
        );
        assert!(
            !normalized.contains("try_from(__incan_result)"),
            "an exact c.i32 carrier must not normalize its result through another integer type"
        );
        assert!(!normalized.contains(".expect("));
        Ok(())
    }

    #[test]
    fn checked_c_output_slots_preserve_every_exact_scalar_carrier() -> Result<(), String> {
        let mut program = IrProgram::new();
        let scalars = [
            (ScalarTypeId::I8, "i8"),
            (ScalarTypeId::U8, "u8"),
            (ScalarTypeId::I16, "i16"),
            (ScalarTypeId::U16, "u16"),
            (ScalarTypeId::I32, "i32"),
            (ScalarTypeId::U32, "u32"),
            (ScalarTypeId::I64, "i64"),
            (ScalarTypeId::U64, "u64"),
            (ScalarTypeId::I128, "i128"),
            (ScalarTypeId::U128, "u128"),
            (ScalarTypeId::F32, "f32"),
            (ScalarTypeId::F64, "f64"),
            (ScalarTypeId::Size, "usize"),
        ];
        for (index, (scalar, _)) in scalars.iter().enumerate() {
            program.checked_c_functions.push(IrCheckedCFunction {
                binding: "Fixture".to_string(),
                symbol: format!("write_scalar_{index}"),
                native_symbol: format!("fixture_write_scalar_{index}"),
                system_library: "fixture".to_string(),
                link_capability: LinkCapabilityId::SystemLibrary,
                parameters: vec![IrCheckedCType::Output {
                    mode: COutputMode::InOut,
                    value: Box::new(IrCheckedCType::Scalar(*scalar)),
                }],
                parameter_names: vec!["value".to_string()],
                return_type: IrCheckedCType::Void,
                resources: Vec::new(),
            });
        }

        let mut emitter = IrEmitter::new(&program.function_registry);
        let generated = emitter.emit_program(&program).map_err(|error| error.to_string())?;
        let normalized = generated.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();
        for (_, rust_type) in scalars {
            assert!(
                normalized.contains(&format!("fnfrom_incan_value(value:{rust_type})->Self")),
                "checked C output storage must accept exact {rust_type} values"
            );
            assert!(
                normalized.contains(&format!("fntake(self)->{rust_type}")),
                "checked C output storage must return exact {rust_type} values"
            );
        }
        assert!(
            !normalized.contains("i64::try_from(value)"),
            "exact C output carriers must not normalize results through i64"
        );
        Ok(())
    }

    #[test]
    fn checked_c_string_input_emits_private_cstring_and_const_character_pointer_bridge() -> Result<(), String> {
        let mut program = IrProgram::new();
        program.uses_checked_c_strings = true;
        program.checked_c_functions.push(IrCheckedCFunction {
            binding: "LibC".to_string(),
            symbol: "string_length".to_string(),
            native_symbol: "strlen".to_string(),
            system_library: "c".to_string(),
            link_capability: LinkCapabilityId::SystemLibrary,
            parameters: vec![IrCheckedCType::Pointer {
                mutable: false,
                pointee: Box::new(IrCheckedCType::Scalar(ScalarTypeId::CChar)),
            }],
            parameter_names: vec!["value".to_string()],
            return_type: IrCheckedCType::Scalar(ScalarTypeId::Size),
            resources: Vec::new(),
        });

        let mut emitter = IrEmitter::new(&program.function_registry);
        let generated = emitter.emit_program(&program).map_err(|error| error.to_string())?;
        let normalized = generated.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();

        assert!(normalized.contains("fn__incan_checked_c_cstr(value:String)->Result<::std::ffi::CString,String>"));
        assert!(normalized.contains("CString::new(value)"));
        assert!(
            normalized.contains(
                "fn__incan_c_LibC__string_5flength__ffi(__incan_arg_0:*const::std::os::raw::c_char,)->usize;"
            )
        );
        assert!(
            normalized
                .contains("fn__incan_c_LibC__string_5flength(__incan_arg_0:*const::std::os::raw::c_char,)->usize")
        );
        Ok(())
    }

    #[test]
    fn checked_c_string_view_emits_bounded_utf8_copy_helper() -> Result<(), String> {
        let mut program = IrProgram::new();
        program.uses_scoped_c_string_views = true;
        let mut emitter = IrEmitter::new(&program.function_registry);
        let generated = emitter.emit_program(&program).map_err(|error| error.to_string())?;
        let normalized = generated.chars().filter(|ch| !ch.is_whitespace()).collect::<String>();

        assert!(normalized.contains("fn__incan_checked_c_copy_utf8"));
        assert!(normalized.contains("max_bytes:i64"));
        assert!(normalized.contains("value.is_null()"));
        assert!(normalized.contains("usize::try_from(max_bytes)"));
        assert!(normalized.contains("from_raw_parts(value.cast::<u8>(),limit)"));
        assert!(normalized.contains("bytes.iter().position(|byte|*byte==0)"));
        assert!(normalized.contains("::std::str::from_utf8(&bytes[..terminator])"));
        assert!(!normalized.contains(".expect("));
        Ok(())
    }
}
