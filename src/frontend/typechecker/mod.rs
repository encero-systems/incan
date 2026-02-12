//! Type checker for the Incan programming language.
//!
//! Validates types, mutability, trait conformance, and error-handling semantics for a parsed Incan program.
//! The checker runs in two passes over the AST and populates a [`SymbolTable`] with resolved type information.
//!
//! ## Notes
//!
//! - **Two-pass model**: The first pass ([`TypeChecker::check_program`]) collects all type and function declarations
//!   into the symbol table. The second pass validates bodies, expressions, and cross-references.
//! - **Import handling**: Call [`TypeChecker::import_module`] or use [`TypeChecker::check_with_imports`] to
//!   pre-populate symbols from dependency modules before checking the main module.
//! - **Error accumulation**: Errors are collected (not fatal) so the checker can report as many issues as possible in a
//!   single run.
//!
//! ## What is validated
//!
//! - All referenced types and symbols are known
//! - Type compatibility at assignments, calls, returns
//! - Mutability rules (`mut` vs immutable bindings)
//! - `?` operator only on `Result` types with compatible error types
//! - Trait conformance for `with` clauses and `@derive` decorators
//! - Newtype method signatures
//! - Match exhaustiveness for enums, `Result`, and `Option`
//!
//! ## Examples
//!
//! ```ignore
//! use incan::frontend::{parser, typechecker};
//!
//! let source = r#"
//! def greet(name: str) -> str:
//!     return f"Hello, {name}!"
//! "#;
//!
//! let ast = parser::parse(source).expect("parse failed");
//! let mut checker = typechecker::TypeChecker::new();
//! checker.check_program(&ast)?;
//! ```
//!
//! ## See also
//!
//! - [`symbols`](super::symbols) – symbol table and scope management
//! - [`diagnostics`](super::diagnostics) – error types and pretty printing

mod check_decl;
mod check_expr;
mod check_stmt;
mod collect;
mod const_eval;
mod helpers;
pub(crate) mod stdlib_loader;
mod validate_rust_module;

pub use const_eval::ConstValue;
#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};

use crate::frontend::ast::*;
use crate::frontend::diagnostics::{CompileError, errors};
use crate::frontend::module::{ExportedSymbol, exported_symbols};
use crate::frontend::symbols::*;
use helpers::{collection_type_id, stringlike_type_id};
use incan_core::lang::surface::types as surface_types;
use incan_core::lang::surface::types::SurfaceTypeKind;
use incan_core::lang::types::collections::CollectionTypeId;
use incan_core::lang::types::stringlike::StringLikeId;

/// Capture reusable typechecking output for later compiler stages.
///
/// This struct is the bridge that lets backend lowering/codegen **consume the typechecker’s view**
/// of the program, rather than re-deriving types and semantics from the AST.
///
/// ## Notes
/// - Expression types are keyed by `(span.start, span.end)` so downstream code can look them up without holding AST
///   node identities.
/// - Const classification is recorded to support RFC 008 “Rust-native vs Frozen” const emission.
///
/// ## Examples
/// ```ignore
/// use incan::frontend::{lexer, parser, typechecker};
///
/// let tokens = lexer::lex("def foo() -> int: return 1").unwrap();
/// let ast = parser::parse(&tokens).unwrap();
/// let mut tc = typechecker::TypeChecker::new();
/// tc.check_program(&ast).unwrap();
/// let info = tc.type_info();
/// // info.expr_type(...) can now be queried by spans.
/// ```
#[derive(Debug, Default, Clone)]
pub struct TypeCheckInfo {
    /// Map from expression span (start,end) -> resolved type.
    pub expr_types: HashMap<(usize, usize), ResolvedType>,
    /// Map from identifier expression span (start,end) -> how it resolved (value vs type vs module).
    ///
    /// This exists so downstream stages (IR lowering/codegen) can reliably distinguish:
    /// - `x.method(...)` where `x` is a value binding, from
    /// - `Type.method(...)` where `Type` is a type name (emits `Type::method(...)` in Rust), and
    /// - imported placeholders (e.g. `from rust::... import Foo`) which are not value bindings.
    pub ident_kinds: HashMap<(usize, usize), IdentKind>,
    /// Const category classification (RFC 008): const name -> kind.
    pub const_kinds: HashMap<String, const_eval::ConstKind>,
    /// Computed const values (when available), keyed by const name.
    pub const_values: HashMap<String, ConstValue>,
}

/// How an identifier expression resolved in the symbol table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentKind {
    /// A value binding (variable/field), or a callable value (function).
    Value,
    /// A type name (models/classes/enums/newtypes).
    TypeName,
    /// An enum variant constructor identifier.
    Variant,
    /// A module-like namespace (e.g. imported module placeholders).
    Module,
    /// A Rust import placeholder (`import rust::...` / `from rust::... import ...`).
    RustImport,
    /// A trait name (may be used as a type-like namespace).
    Trait,
}

impl TypeCheckInfo {
    pub fn expr_type(&self, span: Span) -> Option<&ResolvedType> {
        self.expr_types.get(&(span.start, span.end))
    }

    pub fn ident_kind(&self, span: Span) -> Option<IdentKind> {
        self.ident_kinds.get(&(span.start, span.end)).copied()
    }

    pub fn const_value(&self, name: &str) -> Option<&ConstValue> {
        self.const_values.get(name)
    }
}

/// Type checker state.
///
/// Holds the symbol table, accumulated errors, and context needed for validation.
/// Create with [`TypeChecker::new`], then call [`check_program`](Self::check_program) or
/// [`check_with_imports`](Self::check_with_imports).
pub struct TypeChecker {
    /// Symbol table populated during the first pass.
    pub(crate) symbols: SymbolTable,
    /// Accumulated compile errors (non-fatal).
    pub(crate) errors: Vec<CompileError>,
    /// Track which bindings are mutable for mutation checks.
    pub(crate) mutable_bindings: HashSet<String>,
    /// Current function's error type for `?` operator compatibility.
    pub(crate) current_return_error_type: Option<ResolvedType>,
    /// Active trait @requires context for default method bodies.
    pub(crate) current_trait_requires: Option<HashMap<String, ResolvedType>>,
    /// Active trait name for default method diagnostics.
    pub(crate) current_trait_name: Option<String>,
    /// Deduplicate missing-`@requires` diagnostics within a single trait default method body.
    pub(crate) current_trait_missing_requires_emitted: Option<HashSet<String>>,
    /// Collected module-level const declarations (for rich const-eval + cycle detection).
    pub(crate) const_decls: HashMap<String, (ConstDecl, Span)>,
    /// Const evaluation state machine.
    pub(crate) const_eval_state: HashMap<String, const_eval::ConstEvalState>,
    /// Cached const evaluation results.
    pub(crate) const_eval_cache: HashMap<String, const_eval::ConstEvalResult>,
    /// Reusable typechecker output for downstream stages.
    pub(crate) type_info: TypeCheckInfo,
    /// Public exports for imported dependency modules, keyed by module name.
    pub(crate) dependency_exports: HashMap<String, Vec<ExportedSymbol>>,
    /// Module path for the program being checked (if known).
    pub(crate) current_module_path: Option<Vec<String>>,
    /// Declared Rust crate names from `incan.toml [dependencies]` (RFC 023 / RFC 013).
    ///
    /// Used to validate that `rust.module()` paths reference known crates. When `None`, crate validation is skipped
    /// (e.g. single-file mode without a manifest).
    pub(crate) declared_crate_names: Option<HashSet<String>>,
    /// RFC 023: Cached stdlib function signatures loaded from `.incn` files.
    ///
    /// Used by `collect_import` to derive function signatures from parsed stdlib source instead of hardcoded
    /// registries. See [`stdlib_loader::StdlibAstCache`] for details.
    pub(crate) stdlib_cache: stdlib_loader::StdlibAstCache,
}

impl TypeChecker {
    pub fn new() -> Self {
        Self {
            symbols: SymbolTable::new(),
            errors: Vec::new(),
            mutable_bindings: HashSet::new(),
            current_return_error_type: None,
            current_trait_requires: None,
            current_trait_name: None,
            current_trait_missing_requires_emitted: None,
            const_decls: HashMap::new(),
            const_eval_state: HashMap::new(),
            const_eval_cache: HashMap::new(),
            type_info: TypeCheckInfo::default(),
            dependency_exports: HashMap::new(),
            current_module_path: None,
            declared_crate_names: None,
            stdlib_cache: stdlib_loader::StdlibAstCache::new(),
        }
    }

    /// Set the declared Rust crate names from `incan.toml [dependencies]`.
    ///
    /// When set, `rust.module()` path validation will check that the first segment of the path is either `incan_stdlib`
    /// or a crate declared here.
    pub fn set_declared_crate_names(&mut self, names: HashSet<String>) {
        self.declared_crate_names = Some(names);
    }

    pub fn set_current_module_path(&mut self, path: Option<Vec<String>>) {
        self.current_module_path = path;
    }

    /// Return accumulated type information for reuse by later stages (lowering/codegen).
    pub fn type_info(&self) -> &TypeCheckInfo {
        &self.type_info
    }

    pub(crate) fn record_expr_type(&mut self, span: Span, ty: ResolvedType) {
        self.type_info.expr_types.insert((span.start, span.end), ty);
    }

    /// Look up a type by name and return its [`TypeInfo`], if known.
    ///
    /// ## Parameters
    /// - `name`: The type name to look up.
    ///
    /// ## Returns
    /// - `Some(&TypeInfo)`: If `name` resolves to a type symbol.
    /// - `None`: If the symbol is missing or not a type.
    ///
    /// ## Notes
    /// - This helper exists to flatten the common pattern:
    ///   - `symbols.lookup(name)` → `Option<SymbolId>`
    ///   - `symbols.get(id)` → `Option<&Symbol>`
    ///   - `match sym.kind { SymbolKind::Type(info) => ... }`
    pub(crate) fn lookup_type_info(&self, name: &str) -> Option<&TypeInfo> {
        let id = self.symbols.lookup(name)?;
        let sym = self.symbols.get(id)?;
        match &sym.kind {
            SymbolKind::Type(info) => Some(info),
            _ => None,
        }
    }

    /// Look up a symbol by name and return a reference to it, if found.
    ///
    /// Collapses the common two-step pattern:
    ///   - `symbols.lookup(name)` → `Option<SymbolId>`
    ///   - `symbols.get(id)` → `Option<&Symbol>`
    pub(crate) fn lookup_symbol(&self, name: &str) -> Option<&Symbol> {
        let id = self.symbols.lookup(name)?;
        self.symbols.get(id)
    }

    /// Look up a variable binding by name (in any scope) and return its [`VariableInfo`].
    ///
    /// Returns `None` if the symbol is missing or isn't a variable.
    pub(crate) fn lookup_variable_info(&self, name: &str) -> Option<&VariableInfo> {
        let sym = self.lookup_symbol(name)?;
        match &sym.kind {
            SymbolKind::Variable(info) => Some(info),
            _ => None,
        }
    }

    /// Look up a variable binding by name **in the current scope only** and return its [`VariableInfo`].
    ///
    /// Returns `None` if the symbol is missing, not local, or isn't a variable.
    pub(crate) fn lookup_local_variable_info(&self, name: &str) -> Option<&VariableInfo> {
        let id = self.symbols.lookup_local(name)?;
        let sym = self.symbols.get(id)?;
        match &sym.kind {
            SymbolKind::Variable(info) => Some(info),
            _ => None,
        }
    }

    /// Look up a trait by name and return its [`TraitInfo`], if known.
    ///
    /// Returns `None` if the symbol is missing or isn't a trait.
    pub(crate) fn lookup_trait_info(&self, name: &str) -> Option<&TraitInfo> {
        let sym = self.lookup_symbol(name)?;
        match &sym.kind {
            SymbolKind::Trait(info) => Some(info),
            _ => None,
        }
    }

    /// Mutable variant of [`lookup_local_variable_info`](Self::lookup_local_variable_info).
    ///
    /// Used by const-eval to update inferred types after evaluation.
    pub(crate) fn lookup_local_variable_info_mut(&mut self, name: &str) -> Option<&mut VariableInfo> {
        let id = self.symbols.lookup_local(name)?;
        let sym = self.symbols.get_mut(id)?;
        match &mut sym.kind {
            SymbolKind::Variable(info) => Some(info),
            _ => None,
        }
    }

    /// Resolve a required field type for the active trait default method, if any.
    ///
    /// Emits a dedicated diagnostic if a trait body accesses an undeclared required field.
    pub(crate) fn trait_required_field_type(&mut self, field: &str, span: Span) -> Option<ResolvedType> {
        let requires = self.current_trait_requires.as_ref()?;
        if let Some(ty) = requires.get(field) {
            return Some(ty.clone());
        }
        if let Some(seen) = self.current_trait_missing_requires_emitted.as_ref()
            && seen.contains(field)
        {
            return None;
        }
        let trait_name = self.current_trait_name.as_deref().unwrap_or("<trait>");
        self.errors
            .push(errors::trait_requires_missing_field(trait_name, field, span));
        if let Some(seen) = self.current_trait_missing_requires_emitted.as_mut() {
            seen.insert(field.to_string());
        }
        None
    }

    fn validate_stdlib_type_usage(&mut self, ty: &Spanned<Type>) {
        self.validate_stdlib_type_usage_inner(&ty.node, ty.span);
    }

    fn validate_stdlib_type_usage_inner(&mut self, ty: &Type, span: Span) {
        match ty {
            Type::Simple(name) => self.validate_stdlib_type_name(name, span),
            Type::Generic(name, args) => {
                self.validate_stdlib_type_name(name, span);
                for arg in args {
                    self.validate_stdlib_type_usage_inner(&arg.node, arg.span);
                }
            }
            Type::Function(params, ret) => {
                for param in params {
                    self.validate_stdlib_type_usage_inner(&param.node, param.span);
                }
                self.validate_stdlib_type_usage_inner(&ret.node, ret.span);
            }
            Type::Tuple(elems) => {
                for elem in elems {
                    self.validate_stdlib_type_usage_inner(&elem.node, elem.span);
                }
            }
            Type::Unit | Type::SelfType => {}
        }
    }

    fn validate_stdlib_type_name(&mut self, name: &str, span: Span) {
        let Some(id) = surface_types::from_str(name) else {
            return;
        };
        if surface_types::stdlib_module_path(id).is_some() && self.symbols.lookup(name).is_none() {
            self.errors.push(errors::unknown_symbol(name, span));
        }
    }

    fn resolve_type_checked(&mut self, ty: &Spanned<Type>) -> ResolvedType {
        self.validate_stdlib_type_usage(ty);
        resolve_type(&ty.node, &self.symbols)
    }

    /// Check a program and return errors if any.
    ///
    /// Runs the two-pass type-checking algorithm:
    /// 1. **Collect**: register all type/function declarations in the symbol table.
    /// 2. **Check**: validate bodies, expressions, and cross-references.
    ///
    /// ## Parameters
    ///
    /// - `program`: the parsed AST to validate.
    ///
    /// ## Returns
    ///
    /// - `Ok(())` if type checking succeeds.
    /// - `Err(Vec<CompileError>)` containing all accumulated errors.
    ///
    /// ## Notes
    ///
    /// For multi-module projects, call [`import_module`](Self::import_module) first to populate dependency symbols,
    /// or use [`check_with_imports`](Self::check_with_imports) to import dependencies first.
    pub fn check_program(&mut self, program: &Program) -> Result<(), Vec<CompileError>> {
        // Reset per-run caches.
        self.const_decls.clear();
        self.const_eval_state.clear();
        self.const_eval_cache.clear();
        self.type_info = TypeCheckInfo::default();

        // First pass: collect type declarations
        for decl in &program.declarations {
            self.collect_declaration(decl);
        }

        // Second pass: check consts first so their resolved types are available to later checks.
        for decl in &program.declarations {
            if matches!(decl.node, Declaration::Const(_)) {
                self.check_declaration(decl);
            }
        }
        for decl in &program.declarations {
            if !matches!(decl.node, Declaration::Const(_)) {
                self.check_declaration(decl);
            }
        }

        // ---- RFC 023: validate rust.module() and @rust.extern rules ----
        self.validate_rust_module_and_extern(program);

        if self.errors.is_empty() {
            Ok(())
        } else {
            Err(std::mem::take(&mut self.errors))
        }
    }

    /// Import symbols from another module's AST into the symbol table.
    ///
    /// Call this before [`check_program`](Self::check_program) so the main module can reference types and functions
    /// defined in dependencies.
    ///
    /// ## Parameters
    ///
    /// - `module_ast`: parsed AST of the dependency module.
    /// - `_module_name`: reserved for future namespacing (currently unused).
    pub fn import_module(&mut self, module_ast: &Program, _module_name: &str) {
        // Collect only public declarations from the imported module.
        for decl in &module_ast.declarations {
            if is_public_decl(decl) {
                self.collect_declaration(decl);
            }
        }
    }

    /// Import all symbols from another module's AST into the symbol table.
    ///
    /// This is used for internal compiler passes that need type information across modules
    /// without enforcing `pub` visibility (e.g. codegen-only validation for dependencies).
    pub fn import_module_all(&mut self, module_ast: &Program, _module_name: &str) {
        for decl in &module_ast.declarations {
            self.collect_declaration(decl);
        }
    }

    /// Check a program that may have dependencies on other modules.
    ///
    /// Convenience wrapper that calls [`import_module`](Self::import_module) for each dependency, then
    /// [`check_program`](Self::check_program).
    ///
    /// ## Parameters
    ///
    /// - `program`: parsed AST of the main module.
    /// - `dependencies`: list of `(module_name, ast)` pairs to import first.
    ///
    /// ## Returns
    ///
    /// - `Ok(())` if type checking succeeds.
    /// - `Err(Vec<CompileError>)` containing all accumulated errors.
    #[tracing::instrument(skip_all, fields(decl_count = program.declarations.len(), dep_count = dependencies.len()))]
    pub fn check_with_imports(
        &mut self,
        program: &Program,
        dependencies: &[(&str, &Program)],
    ) -> Result<(), Vec<CompileError>> {
        self.dependency_exports.clear();
        for (name, dep_ast) in dependencies {
            self.dependency_exports
                .insert(name.to_string(), exported_symbols(dep_ast));
        }
        // First: import all dependencies
        for (name, dep_ast) in dependencies {
            self.import_module(dep_ast, name);
        }

        // Then check the main program
        self.check_program(program)
    }

    /// Check a program with dependencies, but allow importing private items.
    ///
    /// This is intended for internal compiler stages (like IR codegen) where we need type information across modules
    /// without enforcing visibility restrictions.
    pub fn check_with_imports_allow_private(
        &mut self,
        program: &Program,
        dependencies: &[(&str, &Program)],
    ) -> Result<(), Vec<CompileError>> {
        // Skip populating dependency exports so visibility checks are bypassed.
        self.dependency_exports.clear();
        for (name, dep_ast) in dependencies {
            self.import_module_all(dep_ast, name);
        }
        self.check_program(program)
    }

    // ========================================================================
    // Type compatibility (shared helper)
    // ========================================================================

    /// Check if two types are compatible for assignment or comparison.
    ///
    /// Returns `true` if `actual` can be used where `expected` is required. Handles `Unknown` (error recovery), type
    /// variables (generics), and recursive checks for generics, functions, and tuples.
    #[allow(clippy::only_used_in_recursion)]
    pub(crate) fn types_compatible(&self, actual: &ResolvedType, expected: &ResolvedType) -> bool {
        if actual == expected {
            return true;
        }

        match (actual, expected) {
            (ResolvedType::Unknown, _) | (_, ResolvedType::Unknown) => true,
            (ResolvedType::TypeVar(_), _) | (_, ResolvedType::TypeVar(_)) => true,
            // Allow bare surface generic types (e.g. `Json`) to match `Json[T]` when used without args.
            (ResolvedType::Named(name), ResolvedType::Generic(generic_name, _))
                if name == generic_name
                    && self.symbols.lookup(name).is_some()
                    && surface_types::from_str(name.as_str())
                        .is_some_and(|id| surface_types::info_for(id).kind == SurfaceTypeKind::Generic) =>
            {
                true
            }
            (ResolvedType::Generic(generic_name, _), ResolvedType::Named(name))
                if name == generic_name
                    && self.symbols.lookup(name).is_some()
                    && surface_types::from_str(name.as_str())
                        .is_some_and(|id| surface_types::info_for(id).kind == SurfaceTypeKind::Generic) =>
            {
                true
            }
            // Internal references: allow exact ref compatibility and allow `&FrozenStr` where `&str` is expected.
            (ResolvedType::Ref(a), ResolvedType::Ref(b)) => self.types_compatible(a, b),
            (ResolvedType::FrozenStr, ResolvedType::Str | ResolvedType::FrozenStr) => true,
            // `FrozenStr` is a read-only string wrapper; allow it where `str` is expected.
            (ResolvedType::Named(name), ResolvedType::Str)
                if stringlike_type_id(name.as_str()) == Some(StringLikeId::FrozenStr) =>
            {
                true
            }
            (ResolvedType::FrozenBytes, ResolvedType::Bytes | ResolvedType::FrozenBytes) => true,
            // Allow `FrozenBytes` where `bytes` is expected.
            (ResolvedType::Named(name), ResolvedType::Bytes)
                if stringlike_type_id(name.as_str()) == Some(StringLikeId::FrozenBytes) =>
            {
                true
            }
            (ResolvedType::FrozenList(a), ResolvedType::FrozenList(b)) => self.types_compatible(a, b),
            (ResolvedType::FrozenList(a), ResolvedType::Generic(name, args))
                if collection_type_id(name.as_str()) == Some(CollectionTypeId::FrozenList) && args.len() == 1 =>
            {
                self.types_compatible(a, &args[0])
            }
            (ResolvedType::Generic(name, args), ResolvedType::FrozenList(b))
                if collection_type_id(name.as_str()) == Some(CollectionTypeId::FrozenList) && args.len() == 1 =>
            {
                self.types_compatible(&args[0], b)
            }
            (ResolvedType::FrozenSet(a), ResolvedType::FrozenSet(b)) => self.types_compatible(a, b),
            (ResolvedType::FrozenSet(a), ResolvedType::Generic(name, args))
                if collection_type_id(name.as_str()) == Some(CollectionTypeId::FrozenSet) && args.len() == 1 =>
            {
                self.types_compatible(a, &args[0])
            }
            (ResolvedType::Generic(name, args), ResolvedType::FrozenSet(b))
                if collection_type_id(name.as_str()) == Some(CollectionTypeId::FrozenSet) && args.len() == 1 =>
            {
                self.types_compatible(&args[0], b)
            }
            (ResolvedType::FrozenDict(k1, v1), ResolvedType::FrozenDict(k2, v2)) => {
                self.types_compatible(k1, k2) && self.types_compatible(v1, v2)
            }
            (ResolvedType::FrozenDict(k1, v1), ResolvedType::Generic(name, args))
                if collection_type_id(name.as_str()) == Some(CollectionTypeId::FrozenDict) && args.len() >= 2 =>
            {
                self.types_compatible(k1, &args[0]) && self.types_compatible(v1, &args[1])
            }
            (ResolvedType::Generic(name, args), ResolvedType::FrozenDict(k2, v2))
                if collection_type_id(name.as_str()) == Some(CollectionTypeId::FrozenDict) && args.len() >= 2 =>
            {
                self.types_compatible(&args[0], k2) && self.types_compatible(&args[1], v2)
            }
            // Treat `Tuple` as both:
            // - a concrete tuple type: `Tuple[T1, T2, ...]`
            // - a supertype for any tuple when used without args: `Tuple`
            //
            // This matches snapshot tests that use `tuple[int, str]` and `Tuple` as "any tuple".
            (ResolvedType::Tuple(_), ResolvedType::Named(name))
                if collection_type_id(name.as_str()) == Some(CollectionTypeId::Tuple) =>
            {
                true
            }
            (ResolvedType::Tuple(elems), ResolvedType::Generic(name, args))
                if collection_type_id(name.as_str()) == Some(CollectionTypeId::Tuple) =>
            {
                elems.len() == args.len()
                    && elems
                        .iter()
                        .zip(args.iter())
                        .all(|(t1, t2)| self.types_compatible(t1, t2))
            }
            (ResolvedType::Generic(name, args), ResolvedType::Tuple(elems))
                if collection_type_id(name.as_str()) == Some(CollectionTypeId::Tuple) =>
            {
                elems.len() == args.len()
                    && elems
                        .iter()
                        .zip(args.iter())
                        .all(|(t1, t2)| self.types_compatible(t1, t2))
            }
            (ResolvedType::Generic(n1, a1), ResolvedType::Generic(n2, a2)) => {
                n1 == n2
                    && a1.len() == a2.len()
                    && a1.iter().zip(a2.iter()).all(|(t1, t2)| self.types_compatible(t1, t2))
            }
            (ResolvedType::Function(p1, r1), ResolvedType::Function(p2, r2)) => {
                p1.len() == p2.len()
                    && p1.iter().zip(p2.iter()).all(|(t1, t2)| self.types_compatible(t1, t2))
                    && self.types_compatible(r1, r2)
            }
            (ResolvedType::Tuple(e1), ResolvedType::Tuple(e2)) => {
                e1.len() == e2.len() && e1.iter().zip(e2.iter()).all(|(t1, t2)| self.types_compatible(t1, t2))
            }
            _ => false,
        }
    }
}

impl Default for TypeChecker {
    fn default() -> Self {
        Self::new()
    }
}

fn is_public_decl(decl: &Spanned<Declaration>) -> bool {
    match &decl.node {
        Declaration::Const(c) => matches!(c.visibility, Visibility::Public),
        Declaration::Model(m) => matches!(m.visibility, Visibility::Public),
        Declaration::Class(c) => matches!(c.visibility, Visibility::Public),
        Declaration::Enum(e) => matches!(e.visibility, Visibility::Public),
        Declaration::Newtype(n) => matches!(n.visibility, Visibility::Public),
        Declaration::Trait(t) => matches!(t.visibility, Visibility::Public),
        Declaration::Function(f) => matches!(f.visibility, Visibility::Public),
        Declaration::Import(_) | Declaration::Docstring(_) => false,
    }
}

/// Convenience function to type-check an AST
#[tracing::instrument(skip_all, fields(decl_count = program.declarations.len()))]
pub fn check(program: &Program) -> Result<(), Vec<CompileError>> {
    TypeChecker::new().check_program(program)
}
