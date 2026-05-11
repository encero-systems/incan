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
//! let ast = parser::parse(source)?;
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
mod type_info;
mod validate_rust_module;

pub use const_eval::ConstValue;
pub use type_info::{
    ComputedPropertyAccessInfo, DecoratedFunctionBindingInfo, DecoratedMethodBindingInfo, FixedUnpackPlan, IdentKind,
    ProtocolIterationInfo, ResolvedMethodCall, ResolvedMethodDispatch, ResolvedOperatorCall, ResolvedOperatorKind,
    RustArgCoercionInfo, RustArgCoercionKind, StaticBindingInfo, TestingFixtureInfo, TypeCheckInfo,
    ValidatedNewtypeCoercionInfo, ValidatedNewtypeCoercionMode, ValidatedNewtypeCoercionStep,
};
#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};
#[cfg(feature = "rust_inspect")]
use std::path::PathBuf;
use std::sync::Arc;

use crate::frontend::ast::*;
use crate::frontend::diagnostics::{CompileError, ErrorKind, errors};
use crate::frontend::library_manifest_index::LibraryManifestIndex;
use crate::frontend::module::{ExportedSymbol, exported_symbols};
use crate::frontend::surface_semantics::SurfaceContext;
use crate::frontend::symbols::*;
#[cfg(feature = "rust_inspect")]
use crate::rust_inspect::RustMetadataCache;
use helpers::{collection_type_id, render_resolved_type_as_rust_arg, stringlike_type_id};
use incan_core::interop::{RustFunctionSig, RustItemKind, RustItemMetadata, RustParam, RustTypeShape};
use incan_core::lang::conventions;
use incan_core::lang::decorators::{self as core_decorators, DecoratorId};
use incan_core::lang::surface::types as surface_types;
use incan_core::lang::surface::types::SurfaceTypeKind;
use incan_core::lang::traits::{self as builtin_traits, TraitId};
use incan_core::lang::types::collections::CollectionTypeId;
use incan_core::lang::types::numerics::{self, NumericFamily, NumericTypeId};
use incan_core::lang::types::stringlike::StringLikeId;

/// Type checker state.
///
/// Holds the symbol table, accumulated errors, and context needed for validation.
/// Create with [`TypeChecker::new`], then call [`check_program`](Self::check_program) or
/// [`check_with_imports`](Self::check_with_imports).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LoopContextKind {
    /// A statement-form loop (`for`, `while`, or `loop:`) where `break` cannot yield a value.
    Statement,
    /// An expression-form `loop:` whose `break` statements must unify to one result type.
    Expression,
}

/// Semantic state for the innermost loop currently being type-checked.
///
/// Each active loop records whether it is statement- or expression-oriented, any contextual expected type for
/// `break <value>`, and the concrete break result types seen while checking the body.
#[derive(Debug, Clone)]
pub(crate) struct LoopContext {
    /// Whether this loop is a statement loop or a value-producing `loop:` expression.
    pub kind: LoopContextKind,
    /// Type that outer context expects this loop expression to produce, when known.
    pub expected_break_ty: Option<ResolvedType>,
    /// Types observed from `break` statements that contribute to the loop result.
    pub break_types: Vec<(ResolvedType, Span)>,
}

/// Declaration context that determines how `yield` expressions are interpreted.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum YieldContext {
    /// `yield` is not valid in the current body.
    Disallowed,
    /// `yield` belongs to `std.testing.fixture` lifecycle semantics.
    Fixture,
    /// `yield value` produces one item of the active generator's element type.
    Generator { element_ty: ResolvedType },
}

pub struct TypeChecker {
    /// Symbol table populated during the first pass.
    pub(crate) symbols: SymbolTable,
    /// Accumulated compile errors (non-fatal).
    pub(crate) errors: Vec<CompileError>,
    /// Accumulated non-fatal diagnostics (warnings/lints).
    ///
    /// These are produced during typechecking but do not cause `check_*` to fail.
    pub(crate) warnings: Vec<CompileError>,
    /// Track which bindings are mutable for mutation checks.
    pub(crate) mutable_bindings: HashSet<String>,
    /// Iterator bindings consumed by terminal RFC 088 methods in the current local checking flow.
    pub(crate) consumed_iterator_bindings: HashMap<String, Span>,
    /// Current function's error type for `?` operator compatibility.
    pub(crate) current_return_error_type: Option<ResolvedType>,
    /// Active declaration-level interpretation for `yield` expressions.
    pub(crate) current_yield_context: YieldContext,
    /// Whether the body currently being checked belongs to an `async def` or async method.
    pub(crate) in_async_body: bool,
    /// Expression span currently being typechecked as an `await` operand.
    pub(crate) await_operand_span: Option<(usize, usize)>,
    /// Nesting depth for expressions being checked as call arguments.
    pub(crate) call_argument_depth: usize,
    /// Stack of active loop contexts, innermost last.
    pub(crate) loop_stack: Vec<LoopContext>,
    /// Active trait @requires context for default method bodies.
    pub(crate) current_trait_requires: Option<HashMap<String, ResolvedType>>,
    /// Active trait computed-property contract for default member bodies.
    pub(crate) current_trait_properties: Option<HashMap<String, PropertyInfo>>,
    /// Active trait name for default method diagnostics.
    pub(crate) current_trait_name: Option<String>,
    /// Active nominal owner while checking a method body.
    pub(crate) current_method_owner: Option<String>,
    /// Active `@classmethod` owner type exposed to the method body as `cls`.
    pub(crate) current_classmethod_self_ty: Option<ResolvedType>,
    /// In-scope generic type-parameter trait bounds, preserving generic arguments for RFC 025 dispatch.
    pub(crate) current_type_param_bound_details: Vec<HashMap<String, Vec<TypeBoundInfo>>>,
    /// Deduplicate missing-`@requires` diagnostics within a single trait default method body.
    pub(crate) current_trait_missing_requires_emitted: Option<HashSet<String>>,
    /// Collected module-level const declarations (for rich const-eval + cycle detection).
    pub(crate) const_decls: HashMap<String, (ConstDecl, Span)>,
    /// Collected module-level static declarations in source order.
    pub(crate) static_decls: Vec<(StaticDecl, Span)>,
    /// Collected module-level function declarations for static dependency analysis.
    pub(crate) local_function_decls: HashMap<String, FunctionDecl>,
    /// Declaration-order index for each local static binding.
    pub(crate) static_decl_positions: HashMap<String, usize>,
    /// Const evaluation state machine.
    pub(crate) const_eval_state: HashMap<String, const_eval::ConstEvalState>,
    /// Cached const evaluation results.
    pub(crate) const_eval_cache: HashMap<String, const_eval::ConstEvalResult>,
    /// Reusable typechecker output for downstream stages.
    pub(crate) type_info: TypeCheckInfo,
    /// Public exports for imported dependency modules, keyed by module name.
    pub(crate) dependency_exports: HashMap<String, Vec<ExportedSymbol>>,
    /// RFC 024 derivable-module metadata from imported source modules, keyed by module path.
    pub(crate) dependency_derivable_modules: HashMap<String, Vec<String>>,
    /// RFC 024/user-module trait metadata from imported source modules, keyed by module-qualified trait name.
    pub(crate) dependency_module_traits: HashMap<String, TraitInfo>,
    /// RFC 024 trait-level Rust derive metadata from imported source modules, keyed by module-qualified trait name.
    pub(crate) dependency_trait_rust_derive_paths: HashMap<String, Vec<String>>,
    /// Consumer-side dependency library manifests (`pub::`) keyed by library name.
    pub(crate) library_manifests: Arc<LibraryManifestIndex>,
    /// Internal semantic type cache for `pub::` exports referenced transitively by imported signatures.
    ///
    /// These entries are intentionally **not** source-visible names: they exist so values returned from imported
    /// functions/methods can still participate in method lookup and trait compatibility even when the consumer did not
    /// explicitly import every referenced carrier type.
    pub(crate) transitive_pub_types: HashMap<String, Vec<TypeInfo>>,
    /// Internal semantic trait cache for `pub::` exports referenced transitively by imported signatures.
    ///
    /// This keeps trait/supertrait compatibility available for imported function signatures without making those trait
    /// names ambient in user source.
    pub(crate) transitive_pub_traits: HashMap<String, Vec<TraitInfo>>,
    /// Tracks which `pub::` libraries have already seeded the internal transitive semantic caches for this checker
    /// run.
    pub(crate) cached_pub_libraries: HashSet<String>,
    /// Module path for the program being checked (if known).
    pub(crate) current_module_path: Option<Vec<String>>,
    /// Declared Rust crate names from `incan.toml [rust-dependencies]` (RFC 023 / RFC 013).
    ///
    /// Used to validate that `rust.module()` paths reference known crates. When `None`, crate validation is skipped
    /// (e.g. single-file mode without a manifest).
    pub(crate) declared_crate_names: Option<HashSet<String>>,
    /// RFC 023: Cached stdlib function signatures loaded from `.incn` files.
    ///
    /// Used by `collect_import` to derive function signatures from parsed stdlib source instead of hardcoded
    /// registries. See [`stdlib_loader::StdlibAstCache`] for details.
    pub(crate) stdlib_cache: stdlib_loader::StdlibAstCache,
    /// Local names bound to `std.testing` marker decorators via imports.
    ///
    /// These names are disallowed in runtime call expressions; markers are decorator-only semantics consumed by the
    /// test runner.
    pub(crate) testing_marker_import_bindings: HashSet<String>,
    /// Fixture function names collected before body checking so dependency metadata is order-independent.
    pub(crate) testing_fixture_names: HashSet<String>,
    /// Import aliases collected from `import` / `from ... import` declarations.
    ///
    /// Maps each local binding name to the fully qualified module path segments. Used as a fallback in
    /// [`validate_decorators`](Self::validate_decorators) when the SymbolTable-based
    /// [`DecoratorPrefixLookup`](crate::frontend::decorator_resolution::DecoratorPrefixLookup) cannot resolve a
    /// decorator path (e.g. functions imported via `from std.testing import parametrize`).
    pub(crate) import_aliases: HashMap<String, Vec<String>>,
    /// Unified import-driven activation and strategy context for soft-keyword/decorator semantics.
    pub(crate) surface_context: SurfaceContext,
    /// RFC 042: transitive supertrait closure for each trait, keyed by trait name.
    ///
    /// Values list every reachable supertrait `(name, type_arguments)` after applying generic substitution along the
    /// chain. Filled at the end of the collection pass (before the second typecheck pass runs).
    pub(crate) supertrait_closure: HashMap<String, Vec<(String, Vec<ResolvedType>)>>,
    /// Trait `with` bounds queued during collection and resolved once all declarations are registered (RFC 042).
    ///
    /// Ensures supertrait names are not mistaken for free type parameters when the supertrait is declared later in the
    /// same module.
    pub(crate) pending_trait_supertraits: Vec<(String, Vec<Spanned<TraitBound>>)>,
    /// Feature-gated cache for rust-inspect semantic metadata extraction (RFC 041).
    #[cfg(feature = "rust_inspect")]
    pub(crate) rust_inspect_cache: RustMetadataCache,
    /// Manifest/workspace root used for rust-analyzer metadata extraction.
    #[cfg(feature = "rust_inspect")]
    pub(crate) rust_inspect_manifest_dir: Option<PathBuf>,
}

impl TypeChecker {
    /// Create an empty typechecker with fresh symbol, diagnostic, import, and lowering-metadata state.
    pub fn new() -> Self {
        Self {
            symbols: SymbolTable::new(),
            errors: Vec::new(),
            warnings: Vec::new(),
            mutable_bindings: HashSet::new(),
            consumed_iterator_bindings: HashMap::new(),
            current_return_error_type: None,
            current_yield_context: YieldContext::Disallowed,
            in_async_body: false,
            await_operand_span: None,
            call_argument_depth: 0,
            loop_stack: Vec::new(),
            current_trait_requires: None,
            current_trait_properties: None,
            current_trait_name: None,
            current_method_owner: None,
            current_classmethod_self_ty: None,
            current_type_param_bound_details: Vec::new(),
            current_trait_missing_requires_emitted: None,
            const_decls: HashMap::new(),
            static_decls: Vec::new(),
            local_function_decls: HashMap::new(),
            static_decl_positions: HashMap::new(),
            const_eval_state: HashMap::new(),
            const_eval_cache: HashMap::new(),
            type_info: TypeCheckInfo::default(),
            dependency_exports: HashMap::new(),
            dependency_derivable_modules: HashMap::new(),
            dependency_module_traits: HashMap::new(),
            dependency_trait_rust_derive_paths: HashMap::new(),
            library_manifests: Arc::new(LibraryManifestIndex::default()),
            transitive_pub_types: HashMap::new(),
            transitive_pub_traits: HashMap::new(),
            cached_pub_libraries: HashSet::new(),
            current_module_path: None,
            declared_crate_names: None,
            stdlib_cache: stdlib_loader::StdlibAstCache::new(),
            testing_marker_import_bindings: HashSet::new(),
            testing_fixture_names: HashSet::new(),
            import_aliases: HashMap::new(),
            surface_context: SurfaceContext::default(),
            supertrait_closure: HashMap::new(),
            pending_trait_supertraits: Vec::new(),
            #[cfg(feature = "rust_inspect")]
            rust_inspect_cache: RustMetadataCache::new(),
            #[cfg(feature = "rust_inspect")]
            rust_inspect_manifest_dir: None,
        }
    }

    /// Push a new loop context before checking a loop body.
    ///
    /// Statement loops pass `None` for `expected_break_ty`; expression loops forward whatever result type the
    /// surrounding context expects.
    pub(crate) fn push_loop_context(&mut self, kind: LoopContextKind, expected_break_ty: Option<ResolvedType>) {
        self.loop_stack.push(LoopContext {
            kind,
            expected_break_ty,
            break_types: Vec::new(),
        });
    }

    /// Pop the innermost loop context once the corresponding body has been checked.
    pub(crate) fn pop_loop_context(&mut self) -> Option<LoopContext> {
        self.loop_stack.pop()
    }

    /// Borrow the innermost active loop so `break` checking can append inferred break types.
    pub(crate) fn current_loop_context_mut(&mut self) -> Option<&mut LoopContext> {
        self.loop_stack.last_mut()
    }

    /// Return whether this call expression is inside the active `await` operand.
    pub(crate) fn is_in_await_operand(&self, span: Span) -> bool {
        self.await_operand_span
            .is_some_and(|(start, end)| start <= span.start && span.end <= end)
    }

    /// Emit the missing-await warning for a direct async call when the current expression context should report it.
    pub(crate) fn warn_if_unawaited_async_call(&mut self, callable: &str, span: Span) {
        if !self.is_in_await_operand(span) && self.call_argument_depth == 0 {
            self.errors.push(errors::async_call_without_await(callable, span));
        }
    }

    /// Resolve the final type of a `loop:` expression from the `break` types observed in its body.
    ///
    /// When an outer expected type exists, every `break` must be compatible with it. Otherwise this picks the
    /// narrowest compatible type seen across all `break` statements and emits a type mismatch when no single result
    /// type can satisfy every branch.
    pub(crate) fn resolve_loop_break_result_type(
        &mut self,
        loop_span: Span,
        expected_break_ty: Option<&ResolvedType>,
        break_types: &[(ResolvedType, Span)],
    ) -> ResolvedType {
        let Some((first_ty, _)) = break_types.first() else {
            self.errors.push(errors::loop_expression_requires_break(loop_span));
            return ResolvedType::Unknown;
        };

        // ---- Context: outer expression already constrains the loop result type ----
        if let Some(expected) = expected_break_ty {
            for (ty, span) in break_types {
                if !self.types_compatible(ty, expected) {
                    self.errors
                        .push(errors::type_mismatch(&expected.to_string(), &ty.to_string(), *span));
                    return ResolvedType::Unknown;
                }
            }
            return expected.clone();
        }

        // ---- Context: infer a common result type from the observed `break` values ----
        let mut result_ty = first_ty.clone();
        for (ty, span) in break_types.iter().skip(1) {
            if self.types_compatible(ty, &result_ty) {
                continue;
            }
            if self.types_compatible(&result_ty, ty) {
                result_ty = ty.clone();
                continue;
            }
            self.errors
                .push(errors::type_mismatch(&result_ty.to_string(), &ty.to_string(), *span));
            return ResolvedType::Unknown;
        }

        result_ty
    }

    /// Opt into semantic rust-inspect extraction for this checker.
    ///
    /// The checker stays metadata-free by default so plain semantic/unit-test paths do not accidentally load an
    /// external Rust workspace. Callers that own project context, such as the CLI, test runner, and LSP, must set the
    /// generated metadata workspace explicitly.
    #[cfg(feature = "rust_inspect")]
    pub fn set_rust_inspect_manifest_dir(&mut self, dir: PathBuf) {
        self.rust_inspect_manifest_dir = Some(dir);
    }

    #[cfg(feature = "rust_inspect")]
    /// Return Rust item metadata, preferring shipped dependency ABI over rust-inspect cache reads.
    pub(crate) fn rust_item_metadata_for_path(&self, canonical_path: &str) -> Option<RustItemMetadata> {
        let canonical_path = Self::normalize_rust_namespace_path(canonical_path);
        let lookup_path = Self::rust_metadata_lookup_path(canonical_path)?;
        if let Some(metadata) = self.library_manifests.rust_abi_item(lookup_path) {
            return Some(metadata);
        }
        let dir = self.rust_inspect_manifest_dir.as_ref()?;
        match self.rust_inspect_cache.get_cached(dir, lookup_path) {
            Ok(Some(hit)) => Some((*hit.metadata).clone()),
            Err(err) => {
                tracing::debug!(
                    "rust-inspect cache lookup failed for `{}` (query `{}`): {err}",
                    canonical_path,
                    lookup_path
                );
                None
            }
            Ok(None) => None,
        }
    }

    #[cfg(feature = "rust_inspect")]
    /// Return Rust item metadata, falling back to extraction only after shipped ABI and cache-only reads miss.
    pub(crate) fn rust_item_metadata_for_path_blocking(&self, canonical_path: &str) -> Option<RustItemMetadata> {
        let canonical_path = Self::normalize_rust_namespace_path(canonical_path);
        let lookup_path = Self::rust_metadata_lookup_path(canonical_path)?;
        if let Some(metadata) = self.library_manifests.rust_abi_item(lookup_path) {
            return Some(metadata);
        }
        let dir = self.rust_inspect_manifest_dir.as_ref()?;
        // stdlib interop paths are conventionally stable and intentionally stay cache-only.
        if lookup_path.starts_with("incan_stdlib::") {
            return self.rust_item_metadata_for_path(lookup_path);
        }
        match self.rust_inspect_cache.get_cached(dir, lookup_path) {
            Ok(Some(hit)) => Some((*hit.metadata).clone()),
            Ok(None) => match self.rust_inspect_cache.get_or_extract(dir, lookup_path, &|_| ()) {
                Ok(hit) => Some((*hit).clone()),
                Err(err) => {
                    tracing::debug!(
                        "rust-inspect extraction failed for `{}` (query `{}`): {err}",
                        canonical_path,
                        lookup_path
                    );
                    None
                }
            },
            Err(err) => {
                tracing::debug!(
                    "rust-inspect cache lookup failed for `{}` (query `{}`): {err}",
                    canonical_path,
                    lookup_path
                );
                None
            }
        }
    }

    #[cfg(not(feature = "rust_inspect"))]
    /// Return Rust item metadata from shipped dependency ABI when rust-inspect support is not compiled in.
    pub(crate) fn rust_item_metadata_for_path(&self, canonical_path: &str) -> Option<RustItemMetadata> {
        let canonical_path = Self::normalize_rust_namespace_path(canonical_path);
        let lookup_path = Self::rust_metadata_lookup_path(canonical_path)?;
        self.library_manifests.rust_abi_item(lookup_path)
    }

    #[cfg(not(feature = "rust_inspect"))]
    /// Return Rust item metadata from shipped dependency ABI when rust-inspect support is not compiled in.
    pub(crate) fn rust_item_metadata_for_path_blocking(&self, canonical_path: &str) -> Option<RustItemMetadata> {
        self.rust_item_metadata_for_path(canonical_path)
    }

    fn split_top_level_generic_args(args: &str) -> Vec<&str> {
        let mut parts = Vec::new();
        let mut depth = 0usize;
        let mut start = 0usize;
        for (idx, ch) in args.char_indices() {
            match ch {
                '<' | '(' | '[' => depth += 1,
                '>' | ')' | ']' => depth = depth.saturating_sub(1),
                ',' if depth == 0 => {
                    parts.push(args[start..idx].trim());
                    start = idx + ch.len_utf8();
                }
                _ => {}
            }
        }
        let tail = args[start..].trim();
        if !tail.is_empty() {
            parts.push(tail);
        }
        parts
    }

    /// Normalize a rust-inspect lookup path down to the nominal item path.
    ///
    /// rust-analyzer metadata is keyed by the item path (`foo::Bar`), not by instantiated spellings like
    /// `foo::Bar<T>` or placeholder displays like `{unknown}`. This strips outer generic instantiation from a Rust
    /// path while rejecting obviously non-item spellings before hitting the metadata cache/extractor.
    fn rust_metadata_lookup_path(canonical_path: &str) -> Option<&str> {
        let trimmed = canonical_path.trim();
        if trimmed.is_empty() || trimmed == "{unknown}" {
            return None;
        }
        let had_generics = trimmed.contains('<');
        let base = trimmed.split_once('<').map_or(trimmed, |(base, _)| base);
        if had_generics && !base.contains("::") {
            return None;
        }
        if base.is_empty() || base.contains(['{', '}', '(', ')', '[', ']', ',', ' ']) {
            return None;
        }
        Some(base)
    }

    /// Strip the synthetic `rust::` namespace prefix used in Incan source paths.
    ///
    /// rust-inspect canonical item keys are crate-rooted Rust paths (`crate_name::...`), so compatibility checks and
    /// metadata lookups should normalize away the language surface namespace first.
    fn normalize_rust_namespace_path(path: &str) -> &str {
        path.strip_prefix("rust::").unwrap_or(path)
    }

    fn rust_path_base_and_args(&self, path: &str) -> (String, Vec<ResolvedType>) {
        let trimmed = path.trim();
        if let Some(start) = trimmed.find('<')
            && trimmed.ends_with('>')
        {
            let base = Self::normalize_rust_namespace_path(trimmed[..start].trim()).to_string();
            let inner = &trimmed[start + 1..trimmed.len() - 1];
            let args = Self::split_top_level_generic_args(inner)
                .into_iter()
                .map(|arg| self.resolved_type_from_rust_display(arg))
                .collect();
            return (base, args);
        }
        (Self::normalize_rust_namespace_path(trimmed).to_string(), Vec::new())
    }

    fn attached_rust_definition_for_path(&self, canonical_path: &str) -> Option<String> {
        let canonical_path = Self::normalize_rust_namespace_path(canonical_path);
        self.symbols.all_symbols().iter().find_map(|sym| {
            let SymbolKind::RustItem(info) = &sym.kind else {
                return None;
            };
            if Self::normalize_rust_namespace_path(info.path.as_str()) != canonical_path {
                return None;
            }
            info.metadata.as_ref().and_then(|meta| meta.definition_path.clone())
        })
    }

    /// Extract the cheap Rust identity already known to the checker for compatibility checks.
    ///
    /// This must stay metadata-light. `types_compatible(...)` calls it frequently, so it may only use symbol-local
    /// metadata already attached during import collection. Fresh rust-inspect extraction from this path would leak a
    /// heavy workspace/indexing concern into ordinary semantic checks.
    fn rust_identity_for_type(&self, ty: &ResolvedType) -> Option<(String, Option<String>, Vec<ResolvedType>)> {
        match ty {
            ResolvedType::RustPath(path) => {
                let (base, args) = self.rust_path_base_and_args(path);
                let definition = self.attached_rust_definition_for_path(base.as_str());
                Some((base, definition, args))
            }
            ResolvedType::Named(name) => {
                let SymbolKind::RustItem(info) = &self.lookup_symbol(name)?.kind else {
                    return None;
                };
                let definition = info.metadata.as_ref().and_then(|meta| meta.definition_path.clone());
                Some((
                    Self::normalize_rust_namespace_path(info.path.as_str()).to_string(),
                    definition,
                    Vec::new(),
                ))
            }
            ResolvedType::Generic(name, args) => {
                let SymbolKind::RustItem(info) = &self.lookup_symbol(name)?.kind else {
                    return None;
                };
                let definition = info.metadata.as_ref().and_then(|meta| meta.definition_path.clone());
                Some((
                    Self::normalize_rust_namespace_path(info.path.as_str()).to_string(),
                    definition,
                    args.clone(),
                ))
            }
            _ => None,
        }
    }

    /// Resolve a source-level rusttype backing type to its canonical Rust path spelling.
    pub(crate) fn rust_path_for_rusttype_underlying(&self, ty: &ResolvedType) -> Option<String> {
        if let ResolvedType::RustPath(path) = ty {
            return Some(path.clone());
        }

        let (base, _definition, args) = self.rust_identity_for_type(ty)?;
        if args.is_empty() {
            return Some(base);
        }

        let rendered_args = args
            .iter()
            .map(render_resolved_type_as_rust_arg)
            .collect::<Vec<_>>()
            .join(", ");
        Some(format!("{base}<{rendered_args}>"))
    }

    fn rust_type_identities_compatible(&self, actual: &ResolvedType, expected: &ResolvedType) -> Option<bool> {
        if let ResolvedType::Ref(inner) | ResolvedType::RefMut(inner) = expected
            && let Some(matches) = self.rust_type_identities_compatible(actual, inner)
        {
            return Some(matches);
        }
        let (actual_path, actual_def, actual_args) = self.rust_identity_for_type(actual)?;
        let (expected_path, expected_def, expected_args) = self.rust_identity_for_type(expected)?;
        let same_base = actual_path == expected_path;
        let same_definition =
            actual_def.is_some() && expected_def.is_some() && actual_def.as_ref() == expected_def.as_ref();
        let actual_resolves_to_expected = actual_def.as_deref() == Some(expected_path.as_str());
        let expected_resolves_to_actual = expected_def.as_deref() == Some(actual_path.as_str());
        if !same_base && !same_definition && !actual_resolves_to_expected && !expected_resolves_to_actual {
            // Without concrete definition metadata for both sides, cross-crate Rust paths can still be equivalent
            // through re-exports. Keep the Rust-path boundary permissive instead of reporting a false mismatch.
            if actual_def.is_none() || expected_def.is_none() {
                return None;
            }
            return Some(false);
        }
        if actual_args.len() != expected_args.len() {
            return Some(actual_args.is_empty() && expected_args.is_empty());
        }
        Some(
            actual_args
                .iter()
                .zip(expected_args.iter())
                .all(|(actual, expected)| self.types_compatible(actual, expected)),
        )
    }

    /// Whether a Rust signature parameter is the implicit receiver (`self`/`&self`/`&mut self`).
    pub(crate) fn rust_param_is_receiver(param: &RustParam) -> bool {
        if param.name.as_deref() == Some("self") {
            return true;
        }
        let normalized = param.type_display.replace(' ', "");
        matches!(
            normalized.as_str(),
            "self" | "&self" | "&mutself" | "Self" | "&Self" | "&mutSelf"
        )
    }

    /// Whether a Rust function signature starts with an implicit receiver parameter.
    pub(crate) fn rust_signature_has_receiver(sig: &RustFunctionSig) -> bool {
        sig.params.first().is_some_and(Self::rust_param_is_receiver)
    }

    /// Build a conservative function type from rust-inspect metadata.
    ///
    /// When `drop_receiver` is true and the Rust signature starts with `self`, that first parameter is omitted because
    /// method-call syntax already supplies the receiver expression.
    pub(crate) fn resolved_function_type_from_rust_sig(
        &self,
        sig: &RustFunctionSig,
        drop_receiver: bool,
    ) -> ResolvedType {
        let skip = usize::from(drop_receiver && Self::rust_signature_has_receiver(sig));
        let params = sig
            .params
            .iter()
            .skip(skip)
            .map(|p| CallableParam::positional(self.resolved_param_type_from_rust_display(p.type_display.as_str())))
            .collect();
        let ret = self.resolved_type_from_rust_display(sig.return_type.as_str());
        ResolvedType::Function(params, Box::new(ret))
    }

    /// Render `path` with generic arguments as `path<A, B, ...>` for embedding in [`ResolvedType::RustPath`].
    ///
    /// When `args` is empty, returns `path` unchanged (no angle brackets).
    fn render_rust_shape_path(path: &str, args: &[RustTypeShape]) -> String {
        if args.is_empty() {
            return path.to_string();
        }
        let rendered_args: Vec<String> = args.iter().map(Self::render_rust_shape_type).collect();
        format!("{path}<{}>", rendered_args.join(", "))
    }

    /// Pretty-print a [`RustTypeShape`] as a stable Rust-like type string.
    ///
    /// Feeds [`ResolvedType::RustPath`] strings. Scalar widths are normalized (`f64`, `i64`, `String`, `Vec<u8>`) to
    /// match [`Self::resolved_type_from_rust_shape`], not to recover the exact original Rust spelling from metadata.
    fn render_rust_shape_type(shape: &RustTypeShape) -> String {
        match shape {
            RustTypeShape::Bool => "bool".to_string(),
            RustTypeShape::Float => "f64".to_string(),
            RustTypeShape::Int => "i64".to_string(),
            RustTypeShape::Str => "String".to_string(),
            RustTypeShape::Bytes => "Vec<u8>".to_string(),
            RustTypeShape::Unit => "()".to_string(),
            RustTypeShape::Option(inner) => format!("Option<{}>", Self::render_rust_shape_type(inner)),
            RustTypeShape::Result(ok, err) => {
                format!(
                    "Result<{}, {}>",
                    Self::render_rust_shape_type(ok),
                    Self::render_rust_shape_type(err)
                )
            }
            RustTypeShape::Tuple(items) => {
                let rendered: Vec<String> = items.iter().map(Self::render_rust_shape_type).collect();
                format!("({})", rendered.join(", "))
            }
            RustTypeShape::Ref(inner) => format!("&{}", Self::render_rust_shape_type(inner)),
            RustTypeShape::RustPath { path, args } => Self::render_rust_shape_path(path, args),
            RustTypeShape::TypeParam(name) => name.clone(),
            RustTypeShape::Unknown => "?".to_string(),
        }
    }

    /// Detect whether a normalized Rust display type starts with `&T` or `&mut T`.
    ///
    /// Returns the mutability flag plus the remaining inner type spelling so [`Self::resolved_type_from_rust_display`]
    /// can preserve borrow semantics for Rust-backed values instead of collapsing them into plain path types.
    fn rust_display_borrow_kind(normalized: &str) -> Option<(bool, &str)> {
        if let Some(inner) = normalized.strip_prefix("&mut") {
            return Some((true, inner));
        }
        normalized.strip_prefix('&').map(|inner| (false, inner))
    }

    /// Map structured rust-inspect [`RustTypeShape`] into a [`ResolvedType`] for field access and pattern typing.
    ///
    /// `Option`/`Result` become [`ResolvedType::Generic`] with constructor names `Option` and `Result`. Concrete paths
    /// use [`Self::render_rust_shape_path`] so generic arguments stay attached to [`ResolvedType::RustPath`].
    pub(crate) fn resolved_type_from_rust_shape(&self, shape: &RustTypeShape) -> ResolvedType {
        match shape {
            RustTypeShape::Bool => ResolvedType::Bool,
            RustTypeShape::Float => ResolvedType::Float,
            RustTypeShape::Int => ResolvedType::Int,
            RustTypeShape::Str => ResolvedType::Str,
            RustTypeShape::Bytes => ResolvedType::Bytes,
            RustTypeShape::Unit => ResolvedType::Unit,
            RustTypeShape::Option(inner) => {
                ResolvedType::Generic("Option".to_string(), vec![self.resolved_type_from_rust_shape(inner)])
            }
            RustTypeShape::Result(ok, err) => ResolvedType::Generic(
                "Result".to_string(),
                vec![
                    self.resolved_type_from_rust_shape(ok),
                    self.resolved_type_from_rust_shape(err),
                ],
            ),
            RustTypeShape::Tuple(items) => ResolvedType::Tuple(
                items
                    .iter()
                    .map(|item| self.resolved_type_from_rust_shape(item))
                    .collect(),
            ),
            RustTypeShape::Ref(inner) => ResolvedType::Ref(Box::new(self.resolved_type_from_rust_shape(inner))),
            RustTypeShape::RustPath { path, args } => ResolvedType::RustPath(Self::render_rust_shape_path(path, args)),
            RustTypeShape::TypeParam(name) => ResolvedType::TypeVar(name.clone()),
            RustTypeShape::Unknown => ResolvedType::Unknown,
        }
    }

    /// Resolve a Rust-origin method signature from cached metadata.
    pub(crate) fn rust_method_signature(&self, rust_path: &str, method: &str) -> Option<RustFunctionSig> {
        let metadata = self.rust_item_metadata_for_path(rust_path)?;
        if let RustItemKind::Type(info) = &metadata.kind {
            return info
                .methods
                .iter()
                .find(|m| m.name == method)
                .map(|m| m.signature.clone());
        }
        None
    }

    /// Resolve a Rust-origin associated function signature (must not take `self`) from cached metadata.
    pub(crate) fn rust_associated_function_signature(&self, rust_path: &str, method: &str) -> Option<RustFunctionSig> {
        let sig = self.rust_method_signature(rust_path, method)?;
        if Self::rust_signature_has_receiver(&sig) {
            return None;
        }
        Some(sig)
    }

    /// Convert a Rust display type string into a conservative [`ResolvedType`].
    ///
    /// RFC 041: intentionally best-effort. Common primitive spellings and `Option` / `Result` wrappers are
    /// recognized, including namespaced aliases whose trailing segment is `Option` or `Result`. Nested generics,
    /// lifetimes, and crate paths otherwise become [`ResolvedType::RustPath`] (or [`ResolvedType::Unknown`] when
    /// empty); lowering relies on rustc for fidelity.
    ///
    /// ## `Result<T, E>` parsing
    ///
    /// `Result<…>` is split on the **first** top-level comma only. Nested generics that contain commas (for example
    /// `Result<Vec<(i32, i32)>, String>`) are therefore parsed incorrectly and may degrade to [`ResolvedType::Unknown`]
    /// for one or both type arguments. Prefer precise typing from Incan surfaces over relying on this heuristic.
    pub(crate) fn resolved_type_from_rust_display(&self, rust_ty: &str) -> ResolvedType {
        let trimmed = rust_ty.trim();
        let no_lifetimes = trimmed.replace("'static ", "").replace("'_", "").replace(' ', "");
        let normalized = no_lifetimes.trim_start_matches("::").to_string();
        match normalized.as_str() {
            "&str" => return ResolvedType::Str,
            "&[u8]" => return ResolvedType::Bytes,
            _ => {}
        }
        if let Some((is_mut, inner)) = Self::rust_display_borrow_kind(normalized.as_str()) {
            let inner_ty = self.resolved_type_from_rust_display(inner);
            return if is_mut {
                ResolvedType::RefMut(Box::new(inner_ty))
            } else {
                ResolvedType::Ref(Box::new(inner_ty))
            };
        }
        match normalized.as_str() {
            "bool" => ResolvedType::Bool,
            "f64" => ResolvedType::Float,
            "i64" => ResolvedType::Int,
            "f32" => ResolvedType::Numeric(NumericTypeId::F32),
            "i8" => ResolvedType::Numeric(NumericTypeId::I8),
            "i16" => ResolvedType::Numeric(NumericTypeId::I16),
            "i32" => ResolvedType::Numeric(NumericTypeId::I32),
            "i128" => ResolvedType::Numeric(NumericTypeId::I128),
            "isize" => ResolvedType::Numeric(NumericTypeId::ISize),
            "u8" => ResolvedType::Numeric(NumericTypeId::U8),
            "u16" => ResolvedType::Numeric(NumericTypeId::U16),
            "u32" => ResolvedType::Numeric(NumericTypeId::U32),
            "u64" => ResolvedType::Numeric(NumericTypeId::U64),
            "u128" => ResolvedType::Numeric(NumericTypeId::U128),
            "usize" => ResolvedType::Numeric(NumericTypeId::USize),
            "str" | "&str" | "String" | "std::string::String" | "alloc::string::String" => ResolvedType::Str,
            "Vec<u8>" | "std::vec::Vec<u8>" | "alloc::vec::Vec<u8>" | "&[u8]" => ResolvedType::Bytes,
            "()" => ResolvedType::Unit,
            _ if normalized.ends_with('>') => {
                if let Some((base, inner)) = normalized.split_once('<') {
                    let base = base.trim_end_matches('>');
                    let inner = inner.trim_end_matches('>');
                    let tail = base.rsplit("::").next().unwrap_or(base);
                    match collection_type_id(tail) {
                        Some(CollectionTypeId::Option) => {
                            return ResolvedType::Generic(
                                "Option".to_string(),
                                vec![self.resolved_type_from_rust_display(inner)],
                            );
                        }
                        Some(CollectionTypeId::Result) => {
                            let mut parts = inner.splitn(2, ',');
                            let ok_ty = parts
                                .next()
                                .map(|p| self.resolved_type_from_rust_display(p))
                                .unwrap_or(ResolvedType::Unknown);
                            // Result aliases such as `datafusion_common::error::Result<T>` often erase the concrete
                            // error arm from the display. Keep the success path semantic and degrade only the missing
                            // error arm.
                            let err_ty = parts
                                .next()
                                .map(|p| self.resolved_type_from_rust_display(p))
                                .unwrap_or(ResolvedType::Unknown);
                            return ResolvedType::Generic("Result".to_string(), vec![ok_ty, err_ty]);
                        }
                        _ => {}
                    }
                }
                if self.lookup_type_info(normalized.as_str()).is_some() {
                    ResolvedType::Named(normalized)
                } else {
                    ResolvedType::RustPath(normalized)
                }
            }
            _ if !normalized.is_empty() => {
                if self.lookup_type_info(normalized.as_str()).is_some() {
                    ResolvedType::Named(normalized)
                } else {
                    ResolvedType::RustPath(normalized)
                }
            }
            _ => ResolvedType::Unknown,
        }
    }

    /// Return a Rust generic type-parameter name when the display is the simple identifier form rust-analyzer uses
    /// for params like `T` or `U`.
    pub(crate) fn rust_display_type_var_name(normalized: &str) -> Option<&str> {
        if normalized.len() == 1 && normalized.chars().next().is_some_and(|ch| ch.is_ascii_uppercase()) {
            Some(normalized)
        } else {
            None
        }
    }

    /// Convert a Rust parameter display type into a [`ResolvedType`] while preserving borrow shape.
    ///
    /// `resolved_type_from_rust_display()` intentionally maps borrowed scalar-like returns such as `&str` and `&[u8]`
    /// into owned Incan value types. Parameters need the opposite treatment: the callable signature must remember the
    /// borrowed Rust boundary so emission can pass `&arg` instead of moving an owned `String` or `Vec<u8>`.
    pub(crate) fn resolved_param_type_from_rust_display(&self, rust_ty: &str) -> ResolvedType {
        let trimmed = rust_ty.trim();
        let no_lifetimes = trimmed.replace("'static ", "").replace("'_", "").replace(' ', "");
        let normalized = no_lifetimes.trim_start_matches("::").to_string();
        if let Some(name) = Self::rust_display_type_var_name(normalized.as_str()) {
            return ResolvedType::TypeVar(name.to_string());
        }
        if let Some((is_mut, inner)) = Self::rust_display_borrow_kind(normalized.as_str()) {
            let inner_ty = match inner {
                "str" => ResolvedType::Str,
                "[u8]" => ResolvedType::Bytes,
                _ => self.resolved_type_from_rust_display(inner),
            };
            return if is_mut {
                ResolvedType::RefMut(Box::new(inner_ty))
            } else {
                ResolvedType::Ref(Box::new(inner_ty))
            };
        }
        self.resolved_type_from_rust_display(normalized.as_str())
    }

    /// Set the declared Rust crate names from `incan.toml [rust-dependencies]`.
    ///
    /// When set, `rust.module()` path validation will check that the first segment of the path is either `incan_stdlib`
    /// or a crate declared here.
    pub fn set_declared_crate_names(&mut self, names: HashSet<String>) {
        self.declared_crate_names = Some(names);
    }

    /// Set the loaded dependency library manifests used for `pub::` import resolution.
    pub fn set_library_manifest_index(&mut self, index: LibraryManifestIndex) {
        self.library_manifests = Arc::new(index);
    }

    /// Set shared dependency library manifests used for `pub::` import resolution.
    pub fn set_library_manifest_index_shared(&mut self, index: Arc<LibraryManifestIndex>) {
        self.library_manifests = index;
    }

    pub fn set_current_module_path(&mut self, path: Option<Vec<String>>) {
        self.current_module_path = path;
    }

    /// Return accumulated non-fatal diagnostics (warnings/lints) from the last check.
    pub fn warnings(&self) -> &[CompileError] {
        &self.warnings
    }

    /// Take accumulated non-fatal diagnostics (warnings/lints) from the last check.
    pub fn take_warnings(&mut self) -> Vec<CompileError> {
        std::mem::take(&mut self.warnings)
    }

    /// Return accumulated type information for reuse by later stages (lowering/codegen).
    pub fn type_info(&self) -> &TypeCheckInfo {
        &self.type_info
    }

    /// Record the resolved type for one expression span in the lowering-facing expression artifact map.
    pub(crate) fn record_expr_type(&mut self, span: Span, ty: ResolvedType) {
        self.type_info.expressions.expr_types.insert((span.start, span.end), ty);
    }

    /// Record a typechecker-proven unpack binding plan for backend lowering.
    pub(crate) fn record_fixed_unpack_plan(&mut self, span: Span, plan: FixedUnpackPlan) {
        self.type_info
            .calls
            .fixed_unpack_plans
            .insert((span.start, span.end), plan);
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

    /// Return the generic-parameter name for a type-like placeholder visible in the current scope.
    ///
    /// During declaration collection, generic parameters are recorded as `TypeVar`, but inside function / method bodies
    /// they are introduced into the symbol table as scoped `Type(Builtin)` placeholders so annotations keep resolving.
    /// For operator and method permissiveness (RFC 023 additive backend inference), both representations should behave
    /// as "typevar-like" generic placeholders.
    pub(crate) fn generic_placeholder_name<'a>(&self, ty: &'a ResolvedType) -> Option<&'a str> {
        match ty {
            ResolvedType::TypeVar(name) => Some(name.as_str()),
            ResolvedType::Named(name) => {
                let sym = self.lookup_symbol(name)?;
                match &sym.kind {
                    SymbolKind::Type(TypeInfo::Builtin) if sym.scope > 0 => Some(name.as_str()),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// Whether `ty` is a generic placeholder (`TypeVar`) or an in-scope type-parameter placeholder.
    pub(crate) fn is_generic_placeholder_type(&self, ty: &ResolvedType) -> bool {
        self.generic_placeholder_name(ty).is_some()
    }

    /// Return declared type parameters and explicit trait adoptions for a semantic type.
    fn semantic_type_params_and_adoptions(&self, type_name: &str) -> Option<(&[String], &[TypeBoundInfo])> {
        match self.lookup_semantic_type_info(type_name)? {
            TypeInfo::Model(model) => Some((&model.type_params, &model.trait_adoptions)),
            TypeInfo::Class(class) => Some((&class.type_params, &class.trait_adoptions)),
            TypeInfo::Enum(enum_info) => Some((&enum_info.type_params, &enum_info.trait_adoptions)),
            TypeInfo::Newtype(newtype) => Some((&newtype.type_params, &newtype.trait_adoptions)),
            _ => None,
        }
    }

    /// Infer the concrete instantiation of `trait_name` for `type_name`, if the type adopts that trait.
    ///
    /// Explicit `with Trait[...]` arguments are authoritative. The older positional mapping remains as a fallback for
    /// nullary adoption metadata and derives.
    pub(crate) fn instantiated_trait_args_for_type(
        &self,
        type_name: &str,
        concrete_type_args: &[ResolvedType],
        trait_name: &str,
    ) -> Option<Vec<ResolvedType>> {
        let (type_params, adopted) = self.semantic_type_params_and_adoptions(type_name)?;
        let concrete_subst =
            crate::frontend::resolved_type_subst::type_param_subst_map(type_params, concrete_type_args);

        for adopted_trait in adopted {
            let Some(adopted_info) = self.lookup_semantic_trait_info(&adopted_trait.name) else {
                continue;
            };
            let direct_args: Vec<ResolvedType> = if adopted_trait.type_args.is_empty() {
                concrete_type_args
                    .iter()
                    .take(adopted_info.type_params.len())
                    .cloned()
                    .collect()
            } else {
                adopted_trait
                    .type_args
                    .iter()
                    .map(|arg| crate::frontend::resolved_type_subst::substitute_resolved_type(arg, &concrete_subst))
                    .collect()
            };
            if direct_args.len() != adopted_info.type_params.len() {
                continue;
            }

            if adopted_trait.name == trait_name {
                return Some(direct_args);
            }

            let closure = self.semantic_supertrait_closure(&adopted_trait.name);
            let subst =
                crate::frontend::resolved_type_subst::type_param_subst_map(&adopted_info.type_params, &direct_args);
            for (supertrait_name, supertrait_args) in closure {
                if supertrait_name != trait_name {
                    continue;
                }
                let instantiated = supertrait_args
                    .iter()
                    .map(|arg| crate::frontend::resolved_type_subst::substitute_resolved_type(arg, &subst))
                    .collect();
                return Some(instantiated);
            }
        }
        None
    }

    /// Whether `supertrait_name` is `subtrait_name` itself or appears in its RFC 042 transitive supertrait closure.
    fn trait_is_supertrait_of(&self, subtrait_name: &str, supertrait_name: &str) -> bool {
        if subtrait_name == supertrait_name {
            return true;
        }
        self.semantic_supertrait_closure(subtrait_name)
            .iter()
            .any(|(name, _)| name == supertrait_name)
    }

    /// Instantiate `supertrait_name`'s type arguments when `subtrait_name` is known with `subtrait_args`.
    ///
    /// Used for trait-to-supertrait compatibility: a value typed `Subtrait[A1,...]` may appear where
    /// `Supertrait[B1,...]` is required when the supertrait edge is satisfied after substitution.
    fn instantiated_supertrait_args(
        &self,
        subtrait_name: &str,
        subtrait_args: &[ResolvedType],
        supertrait_name: &str,
    ) -> Option<Vec<ResolvedType>> {
        let sub_info = self.lookup_semantic_trait_info(subtrait_name)?;
        if subtrait_args.len() != sub_info.type_params.len() {
            return None;
        }
        let subst = crate::frontend::resolved_type_subst::type_param_subst_map(&sub_info.type_params, subtrait_args);
        if subtrait_name == supertrait_name {
            return Some(subtrait_args.to_vec());
        }
        let closure = self.semantic_supertrait_closure(subtrait_name);
        for (name, args) in &closure {
            if name != supertrait_name {
                continue;
            }
            let instantiated: Vec<ResolvedType> = args
                .iter()
                .map(|a| crate::frontend::resolved_type_subst::substitute_resolved_type(a, &subst))
                .collect();
            return Some(instantiated);
        }
        None
    }

    /// Whether a concrete named type declares adoption of `trait_name` (RFC 042: includes transitive supertraits).
    ///
    /// Also treats `@derive(T)` as implementing trait `T` when `T` is a visible trait symbol (e.g. `Clone`).
    pub(crate) fn type_implements_trait(&self, type_name: &str, trait_name: &str) -> bool {
        let Some(info) = self.lookup_semantic_type_info(type_name) else {
            return false;
        };
        let (adopted, derives) = match info {
            TypeInfo::Model(m) => (m.trait_adoptions.as_slice(), Some(m.derives.as_slice())),
            TypeInfo::Class(c) => (c.trait_adoptions.as_slice(), Some(c.derives.as_slice())),
            TypeInfo::Enum(e) => (e.trait_adoptions.as_slice(), Some(e.derives.as_slice())),
            TypeInfo::Newtype(n) => (n.trait_adoptions.as_slice(), None),
            _ => return false,
        };
        for t in adopted {
            if t.name == trait_name {
                return true;
            }
            if self
                .semantic_supertrait_closure(&t.name)
                .iter()
                .any(|(name, _)| name == trait_name)
            {
                return true;
            }
        }
        if let Some(derives) = derives
            && derives.iter().any(|d| d == trait_name)
            && self.lookup_semantic_trait_info(trait_name).is_some()
        {
            return true;
        }
        false
    }

    /// Explicit `with Trait[...]` entries plus trait-like `@derive` entries for method lookup.
    pub(crate) fn trait_adoptions_for_type_methods(
        &self,
        adopted: &[TypeBoundInfo],
        derives: &[String],
    ) -> Vec<TypeBoundInfo> {
        let mut out = adopted.to_vec();
        for d in derives {
            if self.lookup_semantic_trait_info(d).is_some() && !out.iter().any(|t| t.name == *d) {
                out.push(TypeBoundInfo {
                    name: d.clone(),
                    type_args: Vec::new(),
                });
            }
        }
        out
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

    /// Look up a module static binding by name (in any scope) and return its [`StaticInfo`].
    pub(crate) fn lookup_static_info(&self, name: &str) -> Option<&StaticInfo> {
        let sym = self.lookup_symbol(name)?;
        match &sym.kind {
            SymbolKind::Static(info) => Some(info),
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

    /// Look up semantic type metadata, including transitive `pub::` exports referenced only through imported
    /// signatures.
    ///
    /// This is intentionally narrower than [`Self::lookup_type_info`]: it is for internal semantic reasoning such as
    /// method lookup and trait compatibility, not for making unimported provider types available in user source.
    pub(crate) fn lookup_semantic_type_info(&self, name: &str) -> Option<&TypeInfo> {
        if let Some(info) = self.lookup_type_info(name) {
            return Some(info);
        }
        let infos = self.transitive_pub_types.get(name)?;
        (infos.len() == 1).then(|| &infos[0])
    }

    /// Look up semantic trait metadata, including transitive `pub::` exports referenced only through imported
    /// signatures.
    ///
    /// This keeps imported trait contracts available for internal compatibility checks without widening source-visible
    /// name resolution.
    pub(crate) fn lookup_semantic_trait_info(&self, name: &str) -> Option<&TraitInfo> {
        if let Some(info) = self.lookup_trait_info(name) {
            return Some(info);
        }
        let infos = self.transitive_pub_traits.get(name)?;
        (infos.len() == 1).then(|| &infos[0])
    }

    /// Return the transitive supertrait closure for one trait using visible symbols first, then cached `pub::`
    /// semantic metadata.
    ///
    /// The returned `(trait_name, type_arguments)` pairs preserve generic substitution along the supertrait chain just
    /// like the local collection-phase closure used for in-module traits.
    pub(crate) fn semantic_supertrait_closure(&self, trait_name: &str) -> Vec<(String, Vec<ResolvedType>)> {
        if let Some(closure) = self.supertrait_closure.get(trait_name) {
            return closure.clone();
        }
        self.expand_semantic_supertraits_transitively(trait_name)
    }

    /// Expand transitive supertraits for imported trait metadata that never entered the local declaration collector.
    fn expand_semantic_supertraits_transitively(&self, trait_name: &str) -> Vec<(String, Vec<ResolvedType>)> {
        let mut result = Vec::new();
        let mut seen = HashSet::new();
        let mut work = Vec::new();
        let Some(root) = self.lookup_semantic_trait_info(trait_name) else {
            return result;
        };
        work.extend(root.supertraits.clone());

        while let Some((supertrait_name, supertrait_args)) = work.pop() {
            let key = format!(
                "{supertrait_name}<{}>",
                supertrait_args
                    .iter()
                    .map(std::string::ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(",")
            );
            if !seen.insert(key) {
                continue;
            }
            result.push((supertrait_name.clone(), supertrait_args.clone()));
            let Some(supertrait_info) = self.lookup_semantic_trait_info(supertrait_name.as_str()) else {
                continue;
            };
            let subst = crate::frontend::resolved_type_subst::type_param_subst_map(
                &supertrait_info.type_params,
                &supertrait_args,
            );
            for (nested_name, nested_args) in &supertrait_info.supertraits {
                let mapped = nested_args
                    .iter()
                    .map(|arg| crate::frontend::resolved_type_subst::substitute_resolved_type(arg, &subst))
                    .collect();
                work.push((nested_name.clone(), mapped));
            }
        }

        result
    }

    /// RFC 042: Snapshot trait metadata into [`TypeCheckInfo`] for backend lowering.
    ///
    /// This records all visible trait symbols (local and imported), not just traits declared in the current module,
    /// so lowering can resolve supertrait graphs and generic trait arity across module boundaries.
    fn record_trait_metadata_for_lowering(&mut self) {
        self.type_info.traits.direct_supertraits.clear();
        self.type_info.traits.type_params.clear();
        self.type_info.derivations.derivable_modules = self.dependency_derivable_modules.clone();
        self.type_info.derivations.trait_rust_derive_paths = self.dependency_trait_rust_derive_paths.clone();
        for sym in self.symbols.all_symbols() {
            if let SymbolKind::Trait(info) = &sym.kind {
                self.type_info
                    .traits
                    .direct_supertraits
                    .insert(sym.name.clone(), info.supertraits.clone());
                self.type_info
                    .traits
                    .type_params
                    .insert(sym.name.clone(), info.type_params.clone());
            }
        }
    }

    /// Validate local static dependencies before declaration checking.
    ///
    /// `static` initializers may only reference earlier local statics, and dependency cycles are rejected.
    fn validate_static_dependencies(&mut self) {
        if self.static_decls.is_empty() {
            return;
        }

        let mut graph: HashMap<String, Vec<String>> = HashMap::new();
        let static_spans: HashMap<String, Span> = self
            .static_decls
            .iter()
            .map(|(static_decl, span)| (static_decl.name.clone(), *span))
            .collect();

        for (static_decl, _) in self.static_decls.clone() {
            let mut deps = HashSet::new();
            let mut visiting_functions = HashSet::new();
            self.collect_static_dependencies_from_expr(&static_decl.value.node, &mut deps, &mut visiting_functions);
            self.collect_static_initializer_static_writes_from_expr(
                &static_decl.value,
                &static_decl.name,
                &mut HashSet::new(),
            );

            if let Some(current_idx) = self.static_decl_positions.get(&static_decl.name).copied() {
                for dep in &deps {
                    if let Some(dep_idx) = self.static_decl_positions.get(dep).copied()
                        && dep_idx >= current_idx
                    {
                        self.errors.push(errors::static_initializer_requires_earlier_static(
                            dep,
                            &static_decl.name,
                            static_decl.value.span,
                        ));
                    }
                }
            }

            graph.insert(static_decl.name.clone(), deps.into_iter().collect());
        }

        #[derive(Clone, Copy, PartialEq, Eq)]
        enum VisitState {
            Visiting,
            Done,
        }

        fn visit(
            name: &str,
            graph: &HashMap<String, Vec<String>>,
            spans: &HashMap<String, Span>,
            state: &mut HashMap<String, VisitState>,
            stack: &mut Vec<String>,
            collected_errors: &mut Vec<CompileError>,
        ) {
            match state.get(name).copied() {
                Some(VisitState::Done) => return,
                Some(VisitState::Visiting) => {
                    if let Some(start_idx) = stack.iter().position(|item| item == name) {
                        let mut cycle = stack[start_idx..].to_vec();
                        cycle.push(name.to_string());
                        collected_errors.push(errors::static_dependency_cycle(
                            &cycle.join(" -> "),
                            spans.get(name).copied().unwrap_or_default(),
                        ));
                    }
                    return;
                }
                None => {}
            }

            state.insert(name.to_string(), VisitState::Visiting);
            stack.push(name.to_string());
            if let Some(deps) = graph.get(name) {
                for dep in deps {
                    visit(dep, graph, spans, state, stack, collected_errors);
                }
            }
            stack.pop();
            state.insert(name.to_string(), VisitState::Done);
        }

        let mut state: HashMap<String, VisitState> = HashMap::new();
        let mut stack = Vec::new();
        for name in graph.keys() {
            visit(name, &graph, &static_spans, &mut state, &mut stack, &mut self.errors);
        }
    }

    /// Recursively collect the names of local static dependencies from `expr` into `deps`.
    fn collect_static_dependencies_from_expr(
        &self,
        expr: &Expr,
        deps: &mut HashSet<String>,
        visiting_functions: &mut HashSet<String>,
    ) {
        match expr {
            Expr::Ident(name) => {
                if self.static_decl_positions.contains_key(name) {
                    deps.insert(name.clone());
                }
            }
            Expr::Literal(_) | Expr::SelfExpr => {}
            Expr::Binary(left, _, right) => {
                self.collect_static_dependencies_from_expr(&left.node, deps, visiting_functions);
                self.collect_static_dependencies_from_expr(&right.node, deps, visiting_functions);
            }
            Expr::Unary(_, inner) | Expr::Try(inner) | Expr::Paren(inner) | Expr::Yield(Some(inner)) => {
                self.collect_static_dependencies_from_expr(&inner.node, deps, visiting_functions);
            }
            Expr::Yield(None) => {}
            Expr::Call(func, _type_args, args) => {
                self.collect_static_dependencies_from_expr(&func.node, deps, visiting_functions);
                self.collect_static_dependencies_from_call_args(args, deps, visiting_functions);
                if let Expr::Ident(function_name) = &func.node
                    && let Some(function_decl) = self.local_function_decls.get(function_name)
                    && visiting_functions.insert(function_name.clone())
                {
                    for stmt in &function_decl.body {
                        self.collect_static_dependencies_from_statement(&stmt.node, deps, visiting_functions);
                    }
                    visiting_functions.remove(function_name);
                }
            }
            Expr::Index(object, index) => {
                self.collect_static_dependencies_from_expr(&object.node, deps, visiting_functions);
                self.collect_static_dependencies_from_expr(&index.node, deps, visiting_functions);
            }
            Expr::Slice(target, slice) => {
                self.collect_static_dependencies_from_expr(&target.node, deps, visiting_functions);
                if let Some(start) = &slice.start {
                    self.collect_static_dependencies_from_expr(&start.node, deps, visiting_functions);
                }
                if let Some(end) = &slice.end {
                    self.collect_static_dependencies_from_expr(&end.node, deps, visiting_functions);
                }
                if let Some(step) = &slice.step {
                    self.collect_static_dependencies_from_expr(&step.node, deps, visiting_functions);
                }
            }
            Expr::Field(object, _) => {
                self.collect_static_dependencies_from_expr(&object.node, deps, visiting_functions);
            }
            Expr::MethodCall(object, _, _type_args, args) => {
                self.collect_static_dependencies_from_expr(&object.node, deps, visiting_functions);
                self.collect_static_dependencies_from_call_args(args, deps, visiting_functions);
            }
            Expr::Partial(partial) => {
                self.collect_static_dependencies_from_expr(&partial.target.node, deps, visiting_functions);
                for arg in &partial.args {
                    self.collect_static_dependencies_from_expr(&arg.value.node, deps, visiting_functions);
                }
            }
            Expr::Match(scrutinee, arms) => {
                self.collect_static_dependencies_from_expr(&scrutinee.node, deps, visiting_functions);
                for arm in arms {
                    if let Some(guard) = &arm.node.guard {
                        self.collect_static_dependencies_from_expr(&guard.node, deps, visiting_functions);
                    }
                    match &arm.node.body {
                        MatchBody::Expr(expr) => {
                            self.collect_static_dependencies_from_expr(&expr.node, deps, visiting_functions);
                        }
                        MatchBody::Block(stmts) => {
                            for stmt in stmts {
                                self.collect_static_dependencies_from_statement(&stmt.node, deps, visiting_functions);
                            }
                        }
                    }
                }
            }
            Expr::If(if_expr) => {
                self.collect_static_dependencies_from_expr(&if_expr.condition.node, deps, visiting_functions);
                for stmt in &if_expr.then_body {
                    self.collect_static_dependencies_from_statement(&stmt.node, deps, visiting_functions);
                }
                if let Some(else_body) = &if_expr.else_body {
                    for stmt in else_body {
                        self.collect_static_dependencies_from_statement(&stmt.node, deps, visiting_functions);
                    }
                }
            }
            Expr::Loop(loop_expr) => {
                for stmt in &loop_expr.body {
                    self.collect_static_dependencies_from_statement(&stmt.node, deps, visiting_functions);
                }
            }
            Expr::Generator(generator) => {
                self.collect_static_dependencies_from_expr(&generator.expr.node, deps, visiting_functions);
                for clause in &generator.clauses {
                    match clause {
                        ComprehensionClause::For { iter, .. } => {
                            self.collect_static_dependencies_from_expr(&iter.node, deps, visiting_functions);
                        }
                        ComprehensionClause::If(condition) => {
                            self.collect_static_dependencies_from_expr(&condition.node, deps, visiting_functions);
                        }
                    }
                }
            }
            Expr::ListComp(list_comp) => {
                self.collect_static_dependencies_from_expr(&list_comp.expr.node, deps, visiting_functions);
                self.collect_static_dependencies_from_expr(&list_comp.iter.node, deps, visiting_functions);
                if let Some(filter) = &list_comp.filter {
                    self.collect_static_dependencies_from_expr(&filter.node, deps, visiting_functions);
                }
            }
            Expr::DictComp(dict_comp) => {
                self.collect_static_dependencies_from_expr(&dict_comp.key.node, deps, visiting_functions);
                self.collect_static_dependencies_from_expr(&dict_comp.value.node, deps, visiting_functions);
                self.collect_static_dependencies_from_expr(&dict_comp.iter.node, deps, visiting_functions);
                if let Some(filter) = &dict_comp.filter {
                    self.collect_static_dependencies_from_expr(&filter.node, deps, visiting_functions);
                }
            }
            Expr::Closure(_, _) => {}
            Expr::Tuple(items) | Expr::Set(items) => {
                for item in items {
                    self.collect_static_dependencies_from_expr(&item.node, deps, visiting_functions);
                }
            }
            Expr::List(items) => {
                for item in items {
                    match item {
                        ListEntry::Element(value) | ListEntry::Spread(value) => {
                            self.collect_static_dependencies_from_expr(&value.node, deps, visiting_functions);
                        }
                    }
                }
            }
            Expr::Dict(items) => {
                for item in items {
                    match item {
                        DictEntry::Pair(key, value) => {
                            self.collect_static_dependencies_from_expr(&key.node, deps, visiting_functions);
                            self.collect_static_dependencies_from_expr(&value.node, deps, visiting_functions);
                        }
                        DictEntry::Spread(value) => {
                            self.collect_static_dependencies_from_expr(&value.node, deps, visiting_functions);
                        }
                    }
                }
            }
            Expr::Constructor(_, args) => {
                self.collect_static_dependencies_from_call_args(args, deps, visiting_functions);
            }
            Expr::FString(parts) => {
                for part in parts {
                    if let FStringPart::Expr(expr) = part {
                        self.collect_static_dependencies_from_expr(&expr.node, deps, visiting_functions);
                    }
                }
            }
            Expr::Range { start, end, .. } => {
                self.collect_static_dependencies_from_expr(&start.node, deps, visiting_functions);
                self.collect_static_dependencies_from_expr(&end.node, deps, visiting_functions);
            }
            Expr::Surface(surface) => match &surface.payload {
                SurfaceExprPayload::PrefixUnary(expr) => {
                    self.collect_static_dependencies_from_expr(&expr.node, deps, visiting_functions);
                }
                SurfaceExprPayload::RaceFor(race) => {
                    for arm in &race.arms {
                        self.collect_static_dependencies_from_expr(&arm.awaitable.node, deps, visiting_functions);
                        match &arm.body {
                            RaceForBody::Expr(expr) => {
                                self.collect_static_dependencies_from_expr(&expr.node, deps, visiting_functions);
                            }
                            RaceForBody::Block(stmts) => {
                                for stmt in stmts {
                                    self.collect_static_dependencies_from_statement(
                                        &stmt.node,
                                        deps,
                                        visiting_functions,
                                    );
                                }
                            }
                        }
                    }
                }
                SurfaceExprPayload::LeadingDotPath { .. } => {}
                SurfaceExprPayload::ScopedGlyph { left, right, .. } => {
                    self.collect_static_dependencies_from_expr(&left.node, deps, visiting_functions);
                    self.collect_static_dependencies_from_expr(&right.node, deps, visiting_functions);
                }
                SurfaceExprPayload::ScopedSymbolCall { args, .. } => {
                    self.collect_static_dependencies_from_call_args(args, deps, visiting_functions);
                }
            },
        }
    }

    /// Recursively collect the names of local static dependencies from `args` into `deps`.
    fn collect_static_dependencies_from_call_args(
        &self,
        args: &[CallArg],
        deps: &mut HashSet<String>,
        visiting_functions: &mut HashSet<String>,
    ) {
        for arg in args {
            match arg {
                CallArg::Positional(expr)
                | CallArg::Named(_, expr)
                | CallArg::PositionalUnpack(expr)
                | CallArg::KeywordUnpack(expr) => {
                    self.collect_static_dependencies_from_expr(&expr.node, deps, visiting_functions);
                }
            }
        }
    }

    /// Validate RFC 052 "no static assignment in static initializers" by walking expression-driven call graphs.
    ///
    /// This intentionally follows local helper function calls reachable from the initializer expression so hidden
    /// `static` writes in helper bodies are rejected before runtime emission.
    fn collect_static_initializer_static_writes_from_expr(
        &mut self,
        expr: &Spanned<Expr>,
        current_static: &str,
        visiting_functions: &mut HashSet<String>,
    ) {
        match &expr.node {
            Expr::Literal(_) | Expr::Ident(_) | Expr::SelfExpr | Expr::Yield(None) => {}
            Expr::Binary(left, _, right) => {
                self.collect_static_initializer_static_writes_from_expr(left, current_static, visiting_functions);
                self.collect_static_initializer_static_writes_from_expr(right, current_static, visiting_functions);
            }
            Expr::Unary(_, inner) | Expr::Try(inner) | Expr::Paren(inner) | Expr::Yield(Some(inner)) => {
                self.collect_static_initializer_static_writes_from_expr(inner, current_static, visiting_functions);
            }
            Expr::Call(func, _type_args, args) => {
                self.collect_static_initializer_static_writes_from_expr(func, current_static, visiting_functions);
                self.collect_static_initializer_static_writes_from_call_args(args, current_static, visiting_functions);
                if let Expr::Ident(function_name) = &func.node
                    && let Some(function_decl) = self.local_function_decls.get(function_name).cloned()
                    && visiting_functions.insert(function_name.clone())
                {
                    for stmt in &function_decl.body {
                        self.collect_static_initializer_static_writes_from_stmt(
                            stmt,
                            current_static,
                            visiting_functions,
                        );
                    }
                    visiting_functions.remove(function_name);
                }
            }
            Expr::MethodCall(object, _, _type_args, args) => {
                self.collect_static_initializer_static_writes_from_expr(object, current_static, visiting_functions);
                self.collect_static_initializer_static_writes_from_call_args(args, current_static, visiting_functions);
            }
            Expr::Partial(partial) => {
                self.collect_static_initializer_static_writes_from_expr(
                    &partial.target,
                    current_static,
                    visiting_functions,
                );
                for arg in &partial.args {
                    self.collect_static_initializer_static_writes_from_expr(
                        &arg.value,
                        current_static,
                        visiting_functions,
                    );
                }
            }
            Expr::Index(object, index) => {
                self.collect_static_initializer_static_writes_from_expr(object, current_static, visiting_functions);
                self.collect_static_initializer_static_writes_from_expr(index, current_static, visiting_functions);
            }
            Expr::Slice(target, slice) => {
                self.collect_static_initializer_static_writes_from_expr(target, current_static, visiting_functions);
                if let Some(start) = &slice.start {
                    self.collect_static_initializer_static_writes_from_expr(start, current_static, visiting_functions);
                }
                if let Some(end) = &slice.end {
                    self.collect_static_initializer_static_writes_from_expr(end, current_static, visiting_functions);
                }
                if let Some(step) = &slice.step {
                    self.collect_static_initializer_static_writes_from_expr(step, current_static, visiting_functions);
                }
            }
            Expr::Field(object, _) => {
                self.collect_static_initializer_static_writes_from_expr(object, current_static, visiting_functions);
            }
            Expr::Tuple(items) | Expr::Set(items) => {
                for item in items {
                    self.collect_static_initializer_static_writes_from_expr(item, current_static, visiting_functions);
                }
            }
            Expr::List(items) => {
                for item in items {
                    match item {
                        ListEntry::Element(value) | ListEntry::Spread(value) => self
                            .collect_static_initializer_static_writes_from_expr(
                                value,
                                current_static,
                                visiting_functions,
                            ),
                    }
                }
            }
            Expr::Dict(items) => {
                for item in items {
                    match item {
                        DictEntry::Pair(key, value) => {
                            self.collect_static_initializer_static_writes_from_expr(
                                key,
                                current_static,
                                visiting_functions,
                            );
                            self.collect_static_initializer_static_writes_from_expr(
                                value,
                                current_static,
                                visiting_functions,
                            );
                        }
                        DictEntry::Spread(value) => self.collect_static_initializer_static_writes_from_expr(
                            value,
                            current_static,
                            visiting_functions,
                        ),
                    }
                }
            }
            Expr::FString(parts) => {
                for part in parts {
                    if let FStringPart::Expr(inner) = part {
                        self.collect_static_initializer_static_writes_from_expr(
                            inner,
                            current_static,
                            visiting_functions,
                        );
                    }
                }
            }
            Expr::Range { start, end, .. } => {
                self.collect_static_initializer_static_writes_from_expr(start, current_static, visiting_functions);
                self.collect_static_initializer_static_writes_from_expr(end, current_static, visiting_functions);
            }
            Expr::Constructor(_, args) => {
                self.collect_static_initializer_static_writes_from_call_args(args, current_static, visiting_functions);
            }
            Expr::Match(scrutinee, arms) => {
                self.collect_static_initializer_static_writes_from_expr(scrutinee, current_static, visiting_functions);
                for arm in arms {
                    if let Some(guard) = &arm.node.guard {
                        self.collect_static_initializer_static_writes_from_expr(
                            guard,
                            current_static,
                            visiting_functions,
                        );
                    }
                    match &arm.node.body {
                        MatchBody::Expr(inner) => {
                            self.collect_static_initializer_static_writes_from_expr(
                                inner,
                                current_static,
                                visiting_functions,
                            );
                        }
                        MatchBody::Block(stmts) => {
                            for stmt in stmts {
                                self.collect_static_initializer_static_writes_from_stmt(
                                    stmt,
                                    current_static,
                                    visiting_functions,
                                );
                            }
                        }
                    }
                }
            }
            Expr::If(if_expr) => {
                self.collect_static_initializer_static_writes_from_expr(
                    &if_expr.condition,
                    current_static,
                    visiting_functions,
                );
                for stmt in &if_expr.then_body {
                    self.collect_static_initializer_static_writes_from_stmt(stmt, current_static, visiting_functions);
                }
                if let Some(else_body) = &if_expr.else_body {
                    for stmt in else_body {
                        self.collect_static_initializer_static_writes_from_stmt(
                            stmt,
                            current_static,
                            visiting_functions,
                        );
                    }
                }
            }
            Expr::Loop(loop_expr) => {
                for stmt in &loop_expr.body {
                    self.collect_static_initializer_static_writes_from_stmt(stmt, current_static, visiting_functions);
                }
            }
            Expr::Generator(generator) => {
                self.collect_static_initializer_static_writes_from_expr(
                    &generator.expr,
                    current_static,
                    visiting_functions,
                );
                for clause in &generator.clauses {
                    match clause {
                        ComprehensionClause::For { iter, .. } => {
                            self.collect_static_initializer_static_writes_from_expr(
                                iter,
                                current_static,
                                visiting_functions,
                            );
                        }
                        ComprehensionClause::If(condition) => {
                            self.collect_static_initializer_static_writes_from_expr(
                                condition,
                                current_static,
                                visiting_functions,
                            );
                        }
                    }
                }
            }
            Expr::ListComp(list_comp) => {
                self.collect_static_initializer_static_writes_from_expr(
                    &list_comp.expr,
                    current_static,
                    visiting_functions,
                );
                self.collect_static_initializer_static_writes_from_expr(
                    &list_comp.iter,
                    current_static,
                    visiting_functions,
                );
                if let Some(filter) = &list_comp.filter {
                    self.collect_static_initializer_static_writes_from_expr(filter, current_static, visiting_functions);
                }
            }
            Expr::DictComp(dict_comp) => {
                self.collect_static_initializer_static_writes_from_expr(
                    &dict_comp.key,
                    current_static,
                    visiting_functions,
                );
                self.collect_static_initializer_static_writes_from_expr(
                    &dict_comp.value,
                    current_static,
                    visiting_functions,
                );
                self.collect_static_initializer_static_writes_from_expr(
                    &dict_comp.iter,
                    current_static,
                    visiting_functions,
                );
                if let Some(filter) = &dict_comp.filter {
                    self.collect_static_initializer_static_writes_from_expr(filter, current_static, visiting_functions);
                }
            }
            Expr::Closure(_, _) => {}
            Expr::Surface(surface) => match &surface.payload {
                SurfaceExprPayload::PrefixUnary(inner) => {
                    self.collect_static_initializer_static_writes_from_expr(inner, current_static, visiting_functions);
                }
                SurfaceExprPayload::RaceFor(race) => {
                    for arm in &race.arms {
                        self.collect_static_initializer_static_writes_from_expr(
                            &arm.awaitable,
                            current_static,
                            visiting_functions,
                        );
                        match &arm.body {
                            RaceForBody::Expr(expr) => self.collect_static_initializer_static_writes_from_expr(
                                expr,
                                current_static,
                                visiting_functions,
                            ),
                            RaceForBody::Block(stmts) => {
                                for stmt in stmts {
                                    self.collect_static_initializer_static_writes_from_stmt(
                                        stmt,
                                        current_static,
                                        visiting_functions,
                                    );
                                }
                            }
                        }
                    }
                }
                SurfaceExprPayload::LeadingDotPath { .. } => {}
                SurfaceExprPayload::ScopedGlyph { left, right, .. } => {
                    self.collect_static_initializer_static_writes_from_expr(left, current_static, visiting_functions);
                    self.collect_static_initializer_static_writes_from_expr(right, current_static, visiting_functions);
                }
                SurfaceExprPayload::ScopedSymbolCall { args, .. } => {
                    self.collect_static_initializer_static_writes_from_call_args(
                        args,
                        current_static,
                        visiting_functions,
                    );
                }
            },
        }
    }

    /// Recurse through call arguments while checking initializer-reachable static writes.
    fn collect_static_initializer_static_writes_from_call_args(
        &mut self,
        args: &[CallArg],
        current_static: &str,
        visiting_functions: &mut HashSet<String>,
    ) {
        for arg in args {
            match arg {
                CallArg::Positional(expr)
                | CallArg::Named(_, expr)
                | CallArg::PositionalUnpack(expr)
                | CallArg::KeywordUnpack(expr) => {
                    self.collect_static_initializer_static_writes_from_expr(expr, current_static, visiting_functions);
                }
            }
        }
    }

    /// Recurse through statements reachable from an initializer expression and reject any write to static storage.
    fn collect_static_initializer_static_writes_from_stmt(
        &mut self,
        stmt: &Spanned<Statement>,
        current_static: &str,
        visiting_functions: &mut HashSet<String>,
    ) {
        match &stmt.node {
            Statement::Assignment(assign) => {
                if self.static_decl_positions.contains_key(&assign.name) {
                    self.errors.push(errors::static_initializer_static_write_not_allowed(
                        current_static,
                        &assign.name,
                        stmt.span,
                    ));
                }
                self.collect_static_initializer_static_writes_from_expr(
                    &assign.value,
                    current_static,
                    visiting_functions,
                );
            }
            Statement::CompoundAssignment(assign) => {
                if self.static_decl_positions.contains_key(&assign.name) {
                    self.errors.push(errors::static_initializer_static_write_not_allowed(
                        current_static,
                        &assign.name,
                        stmt.span,
                    ));
                }
                self.collect_static_initializer_static_writes_from_expr(
                    &assign.value,
                    current_static,
                    visiting_functions,
                );
            }
            Statement::FieldAssignment(assign) => {
                if let Some(target_static) = self.static_assignment_target_name(&assign.object) {
                    self.errors.push(errors::static_initializer_static_write_not_allowed(
                        current_static,
                        target_static.as_str(),
                        assign.target_span,
                    ));
                }
                self.collect_static_initializer_static_writes_from_expr(
                    &assign.object,
                    current_static,
                    visiting_functions,
                );
                self.collect_static_initializer_static_writes_from_expr(
                    &assign.value,
                    current_static,
                    visiting_functions,
                );
            }
            Statement::IndexAssignment(assign) => {
                if let Some(target_static) = self.static_assignment_target_name(&assign.object) {
                    self.errors.push(errors::static_initializer_static_write_not_allowed(
                        current_static,
                        target_static.as_str(),
                        stmt.span,
                    ));
                }
                self.collect_static_initializer_static_writes_from_expr(
                    &assign.object,
                    current_static,
                    visiting_functions,
                );
                self.collect_static_initializer_static_writes_from_expr(
                    &assign.index,
                    current_static,
                    visiting_functions,
                );
                self.collect_static_initializer_static_writes_from_expr(
                    &assign.value,
                    current_static,
                    visiting_functions,
                );
            }
            Statement::TupleAssign(assign) => {
                for target in &assign.targets {
                    if let Some(target_static) = self.static_assignment_target_name(target) {
                        self.errors.push(errors::static_initializer_static_write_not_allowed(
                            current_static,
                            target_static.as_str(),
                            target.span,
                        ));
                    }
                    self.collect_static_initializer_static_writes_from_expr(target, current_static, visiting_functions);
                }
                self.collect_static_initializer_static_writes_from_expr(
                    &assign.value,
                    current_static,
                    visiting_functions,
                );
            }
            Statement::TupleUnpack(assign) => {
                for target in &assign.names {
                    if self.static_decl_positions.contains_key(target) {
                        self.errors.push(errors::static_initializer_static_write_not_allowed(
                            current_static,
                            target,
                            stmt.span,
                        ));
                    }
                }
                self.collect_static_initializer_static_writes_from_expr(
                    &assign.value,
                    current_static,
                    visiting_functions,
                );
            }
            Statement::ChainedAssignment(assign) => {
                for target in &assign.targets {
                    if self.static_decl_positions.contains_key(target) {
                        self.errors.push(errors::static_initializer_static_write_not_allowed(
                            current_static,
                            target,
                            stmt.span,
                        ));
                    }
                }
                self.collect_static_initializer_static_writes_from_expr(
                    &assign.value,
                    current_static,
                    visiting_functions,
                );
            }
            Statement::Assert(assert_stmt) => {
                match &assert_stmt.kind {
                    AssertKind::Condition(condition) => self.collect_static_initializer_static_writes_from_expr(
                        condition,
                        current_static,
                        visiting_functions,
                    ),
                    AssertKind::IsPattern { value, .. } => self.collect_static_initializer_static_writes_from_expr(
                        value,
                        current_static,
                        visiting_functions,
                    ),
                    AssertKind::Raises { call, .. } => self.collect_static_initializer_static_writes_from_expr(
                        call,
                        current_static,
                        visiting_functions,
                    ),
                }
                if let Some(message) = &assert_stmt.message {
                    self.collect_static_initializer_static_writes_from_expr(
                        message,
                        current_static,
                        visiting_functions,
                    );
                }
            }
            Statement::Return(Some(expr)) | Statement::Expr(expr) => {
                self.collect_static_initializer_static_writes_from_expr(expr, current_static, visiting_functions);
            }
            Statement::If(if_stmt) => {
                self.collect_static_initializer_static_writes_from_condition(
                    &if_stmt.condition,
                    current_static,
                    visiting_functions,
                );
                for inner in &if_stmt.then_body {
                    self.collect_static_initializer_static_writes_from_stmt(inner, current_static, visiting_functions);
                }
                for (condition, body) in &if_stmt.elif_branches {
                    self.collect_static_initializer_static_writes_from_expr(
                        condition,
                        current_static,
                        visiting_functions,
                    );
                    for inner in body {
                        self.collect_static_initializer_static_writes_from_stmt(
                            inner,
                            current_static,
                            visiting_functions,
                        );
                    }
                }
                if let Some(else_body) = &if_stmt.else_body {
                    for inner in else_body {
                        self.collect_static_initializer_static_writes_from_stmt(
                            inner,
                            current_static,
                            visiting_functions,
                        );
                    }
                }
            }
            Statement::Loop(loop_stmt) => {
                for inner in &loop_stmt.body {
                    self.collect_static_initializer_static_writes_from_stmt(inner, current_static, visiting_functions);
                }
            }
            Statement::While(while_stmt) => {
                self.collect_static_initializer_static_writes_from_condition(
                    &while_stmt.condition,
                    current_static,
                    visiting_functions,
                );
                for inner in &while_stmt.body {
                    self.collect_static_initializer_static_writes_from_stmt(inner, current_static, visiting_functions);
                }
            }
            Statement::For(for_stmt) => {
                self.collect_static_initializer_static_writes_from_expr(
                    &for_stmt.iter,
                    current_static,
                    visiting_functions,
                );
                for inner in &for_stmt.body {
                    self.collect_static_initializer_static_writes_from_stmt(inner, current_static, visiting_functions);
                }
            }
            Statement::Break(Some(expr)) => {
                self.collect_static_initializer_static_writes_from_expr(expr, current_static, visiting_functions);
            }
            Statement::Return(None)
            | Statement::Pass
            | Statement::Break(None)
            | Statement::Continue
            | Statement::Surface(_)
            | Statement::VocabBlock(_) => {}
        }
    }

    fn collect_static_initializer_static_writes_from_condition(
        &mut self,
        condition: &Condition,
        current_static: &str,
        visiting_functions: &mut HashSet<String>,
    ) {
        match condition {
            Condition::Expr(expr) => {
                self.collect_static_initializer_static_writes_from_expr(expr, current_static, visiting_functions);
            }
            Condition::Let { value, .. } => {
                self.collect_static_initializer_static_writes_from_expr(value, current_static, visiting_functions);
            }
        }
    }

    /// Resolve the static root name for assignment targets such as `S`, `S.field`, or `S[idx]`.
    fn static_assignment_target_name(&self, expr: &Spanned<Expr>) -> Option<String> {
        match &expr.node {
            Expr::Ident(name) => self.static_decl_positions.contains_key(name).then(|| name.clone()),
            Expr::Field(object, _) | Expr::Index(object, _) | Expr::Paren(object) => {
                self.static_assignment_target_name(object)
            }
            _ => None,
        }
    }

    /// Recursively collect the names of local static dependencies from `stmt` into `deps`.
    fn collect_static_dependencies_from_statement(
        &self,
        stmt: &Statement,
        deps: &mut HashSet<String>,
        visiting_functions: &mut HashSet<String>,
    ) {
        match stmt {
            Statement::Assignment(assign) => {
                self.collect_static_dependencies_from_expr(&assign.value.node, deps, visiting_functions);
            }
            Statement::FieldAssignment(assign) => {
                self.collect_static_dependencies_from_expr(&assign.object.node, deps, visiting_functions);
                self.collect_static_dependencies_from_expr(&assign.value.node, deps, visiting_functions);
            }
            Statement::IndexAssignment(assign) => {
                self.collect_static_dependencies_from_expr(&assign.object.node, deps, visiting_functions);
                self.collect_static_dependencies_from_expr(&assign.index.node, deps, visiting_functions);
                self.collect_static_dependencies_from_expr(&assign.value.node, deps, visiting_functions);
            }
            Statement::Return(Some(expr)) | Statement::Expr(expr) => {
                self.collect_static_dependencies_from_expr(&expr.node, deps, visiting_functions);
            }
            Statement::Break(Some(expr)) => {
                self.collect_static_dependencies_from_expr(&expr.node, deps, visiting_functions);
            }
            Statement::Return(None) | Statement::Pass | Statement::Break(None) | Statement::Continue => {}
            Statement::If(if_stmt) => {
                self.collect_static_dependencies_from_condition(&if_stmt.condition, deps, visiting_functions);
                for stmt in &if_stmt.then_body {
                    self.collect_static_dependencies_from_statement(&stmt.node, deps, visiting_functions);
                }
                for (condition, body) in &if_stmt.elif_branches {
                    self.collect_static_dependencies_from_expr(&condition.node, deps, visiting_functions);
                    for stmt in body {
                        self.collect_static_dependencies_from_statement(&stmt.node, deps, visiting_functions);
                    }
                }
                if let Some(else_body) = &if_stmt.else_body {
                    for stmt in else_body {
                        self.collect_static_dependencies_from_statement(&stmt.node, deps, visiting_functions);
                    }
                }
            }
            Statement::Loop(loop_stmt) => {
                for stmt in &loop_stmt.body {
                    self.collect_static_dependencies_from_statement(&stmt.node, deps, visiting_functions);
                }
            }
            Statement::While(while_stmt) => {
                self.collect_static_dependencies_from_condition(&while_stmt.condition, deps, visiting_functions);
                for stmt in &while_stmt.body {
                    self.collect_static_dependencies_from_statement(&stmt.node, deps, visiting_functions);
                }
            }
            Statement::For(for_stmt) => {
                self.collect_static_dependencies_from_expr(&for_stmt.iter.node, deps, visiting_functions);
                for stmt in &for_stmt.body {
                    self.collect_static_dependencies_from_statement(&stmt.node, deps, visiting_functions);
                }
            }
            Statement::CompoundAssignment(assign) => {
                self.collect_static_dependencies_from_expr(&assign.value.node, deps, visiting_functions);
            }
            Statement::TupleUnpack(assign) => {
                self.collect_static_dependencies_from_expr(&assign.value.node, deps, visiting_functions);
            }
            Statement::TupleAssign(assign) => {
                self.collect_static_dependencies_from_expr(&assign.value.node, deps, visiting_functions);
                for target in &assign.targets {
                    self.collect_static_dependencies_from_expr(&target.node, deps, visiting_functions);
                }
            }
            Statement::ChainedAssignment(assign) => {
                self.collect_static_dependencies_from_expr(&assign.value.node, deps, visiting_functions);
            }
            Statement::Assert(assert_stmt) => {
                match &assert_stmt.kind {
                    AssertKind::Condition(condition) => {
                        self.collect_static_dependencies_from_expr(&condition.node, deps, visiting_functions);
                    }
                    AssertKind::IsPattern { value, .. } => {
                        self.collect_static_dependencies_from_expr(&value.node, deps, visiting_functions);
                    }
                    AssertKind::Raises { call, .. } => {
                        self.collect_static_dependencies_from_expr(&call.node, deps, visiting_functions);
                    }
                }
                if let Some(message) = &assert_stmt.message {
                    self.collect_static_dependencies_from_expr(&message.node, deps, visiting_functions);
                }
            }
            Statement::Surface(_) | Statement::VocabBlock(_) => {}
        }
    }

    fn collect_static_dependencies_from_condition(
        &self,
        condition: &Condition,
        deps: &mut HashSet<String>,
        visiting_functions: &mut HashSet<String>,
    ) {
        match condition {
            Condition::Expr(expr) => self.collect_static_dependencies_from_expr(&expr.node, deps, visiting_functions),
            Condition::Let { value, .. } => {
                self.collect_static_dependencies_from_expr(&value.node, deps, visiting_functions);
            }
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

    /// Validate stdlib-owned type names recursively inside a type annotation.
    fn validate_stdlib_type_usage_inner(&mut self, ty: &Type, span: Span) {
        match ty {
            Type::Simple(name) => self.validate_stdlib_type_name(name, span),
            Type::Qualified(segments) => {
                if let Some(first) = segments.first() {
                    self.validate_stdlib_type_name(first, span);
                }
            }
            Type::Generic(name, args) => {
                self.validate_stdlib_type_name(name, span);
                for arg in args {
                    self.validate_stdlib_type_usage_inner(&arg.node, arg.span);
                }
            }
            Type::ConstrainedPrimitive(name, _) => self.validate_stdlib_type_name(name, span),
            Type::Function(params, ret) => {
                for param in params {
                    self.validate_stdlib_type_usage_inner(&param.node, param.span);
                }
                self.validate_stdlib_type_usage_inner(&ret.node, ret.span);
            }
            Type::Ref(inner) | Type::RefMut(inner) => {
                self.validate_stdlib_type_usage_inner(&inner.node, inner.span);
            }
            Type::Tuple(elems) => {
                for elem in elems {
                    self.validate_stdlib_type_usage_inner(&elem.node, elem.span);
                }
            }
            Type::IntLiteral(_) | Type::Unit | Type::SelfType | Type::Infer => {}
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

    /// Resolve a type annotation and emit diagnostics for reserved or invalid type spellings.
    fn resolve_type_checked(&mut self, ty: &Spanned<Type>) -> ResolvedType {
        self.validate_stdlib_type_usage(ty);
        if let Type::Simple(name) = &ty.node
            && Self::reserved_numeric_type_name(name)
        {
            self.errors.push(CompileError::type_error(
                format!(
                    "`{name}` is reserved for numeric types; use `decimal[p, s]`/`numeric[p, s]` for decimals or an exact-width integer type"
                ),
                ty.span,
            ));
            return ResolvedType::Unknown;
        }
        if let Some(decimal_ty) = self.resolve_decimal_type_checked(ty) {
            return decimal_ty;
        }
        if let Type::Simple(name) = &ty.node
            && let Some(sym) = self.lookup_symbol(name.as_str())
            && let SymbolKind::RustItem(info) = &sym.kind
            && info.binding == RustImportBindingKind::CrateRoot
        {
            self.errors
                .push(errors::rust_crate_root_used_as_type(name.as_str(), &info.path, ty.span));
            return ResolvedType::Unknown;
        }
        resolve_type(&ty.node, &self.symbols)
    }

    /// Return whether a simple type name is reserved for a parameterized numeric family.
    fn reserved_numeric_type_name(name: &str) -> bool {
        matches!(name, "decimal" | "numeric")
    }

    /// Resolve and validate a parameterized decimal type annotation.
    fn resolve_decimal_type_checked(&mut self, ty: &Spanned<Type>) -> Option<ResolvedType> {
        let Type::Generic(name, args) = &ty.node else {
            return None;
        };
        let constructor = numerics::decimal_constructor_from_str(name.as_str())?;
        if args.len() != 2 {
            self.errors.push(CompileError::type_error(
                format!("{name}[...] expects exactly 2 integer parameters: precision and scale"),
                ty.span,
            ));
            return Some(ResolvedType::Unknown);
        }
        let Some(precision) = decimal_type_int_arg(&args[0]) else {
            self.errors.push(CompileError::type_error(
                "Decimal precision must be an integer literal".to_string(),
                args[0].span,
            ));
            return Some(ResolvedType::Unknown);
        };
        let Some(scale) = decimal_type_int_arg(&args[1]) else {
            self.errors.push(CompileError::type_error(
                "Decimal scale must be an integer literal".to_string(),
                args[1].span,
            ));
            return Some(ResolvedType::Unknown);
        };
        if !(1..=38).contains(&precision) {
            self.errors.push(CompileError::type_error(
                format!("Decimal precision must be between 1 and 38, found {precision}"),
                args[0].span,
            ));
            return Some(ResolvedType::Unknown);
        }
        if scale < 0 || scale > precision {
            self.errors.push(CompileError::type_error(
                format!("Decimal scale must be between 0 and precision {precision}, found {scale}"),
                args[1].span,
            ));
            return Some(ResolvedType::Unknown);
        }
        let canonical = numerics::decimal_constructor_info_for(constructor).canonical;
        Some(ResolvedType::Generic(
            canonical.to_string(),
            vec![
                ResolvedType::TypeVar(precision.to_string()),
                ResolvedType::TypeVar(scale.to_string()),
            ],
        ))
    }

    /// Validate alias and partial relationships that need declaration-level context before symbol collection flattens
    /// aliases and projected callable presets.
    fn validate_alias_declarations(&mut self, program: &Program) {
        let mut public_decls = HashMap::new();
        let mut alias_targets = HashMap::new();
        let mut partial_targets = HashMap::new();
        let mut newtype_names = HashSet::new();
        let mut newtype_underlyings = HashMap::new();

        for decl in &program.declarations {
            if let Some(name) = declaration_name(decl) {
                public_decls.insert(name.to_string(), is_public_decl(decl));
            }
            if let Declaration::Newtype(nt) = &decl.node {
                newtype_names.insert(nt.name.clone());
            }
        }

        for decl in &program.declarations {
            match &decl.node {
                Declaration::Alias(alias) => {
                    if let [target] = alias.target.segments.as_slice() {
                        alias_targets.insert(alias.name.clone(), (target.clone(), decl.span));
                        if matches!(alias.visibility, Visibility::Public)
                            && public_decls.get(target).is_some_and(|is_public| !*is_public)
                        {
                            self.errors.push(CompileError::type_error(
                                format!(
                                    "Public alias '{}' targets private symbol '{}'",
                                    alias.name,
                                    alias.target.segments.join(".")
                                ),
                                decl.span,
                            ));
                        }
                    }
                }
                Declaration::Partial(partial) => {
                    if let [target] = partial.target.segments.as_slice() {
                        partial_targets.insert(partial.name.clone(), (target.clone(), decl.span));
                        if matches!(partial.visibility, Visibility::Public)
                            && public_decls.get(target).is_some_and(|is_public| !*is_public)
                        {
                            self.errors.push(CompileError::type_error(
                                format!(
                                    "Public partial '{}' targets private symbol '{}'",
                                    partial.name,
                                    partial.target.segments.join(".")
                                ),
                                decl.span,
                            ));
                        }
                    }
                    if matches!(partial.visibility, Visibility::Public) {
                        self.validate_public_partial_presets_do_not_reference_private_symbols(
                            &partial.name,
                            &partial.args,
                            &public_decls,
                        );
                    }
                }
                Declaration::Model(model) => {
                    self.validate_member_name_collisions(
                        &model.name,
                        &model.fields,
                        &model.method_aliases,
                        &model.method_partials,
                        &model.properties,
                        &model.methods,
                    );
                    self.validate_method_alias_declarations(
                        &model.name,
                        &model.method_aliases,
                        &model.properties,
                        &model.methods,
                    );
                    self.validate_method_partial_declarations(
                        &model.name,
                        &model.method_aliases,
                        &model.method_partials,
                        &model.properties,
                        &model.methods,
                    );
                }
                Declaration::Class(class) => {
                    self.validate_member_name_collisions(
                        &class.name,
                        &class.fields,
                        &class.method_aliases,
                        &class.method_partials,
                        &class.properties,
                        &class.methods,
                    );
                    self.validate_method_alias_declarations(
                        &class.name,
                        &class.method_aliases,
                        &class.properties,
                        &class.methods,
                    );
                    self.validate_method_partial_declarations(
                        &class.name,
                        &class.method_aliases,
                        &class.method_partials,
                        &class.properties,
                        &class.methods,
                    );
                }
                Declaration::Trait(tr) => {
                    self.validate_member_name_collisions(
                        &tr.name,
                        &[],
                        &tr.method_aliases,
                        &tr.method_partials,
                        &tr.properties,
                        &tr.methods,
                    );
                    self.validate_method_alias_declarations(&tr.name, &tr.method_aliases, &tr.properties, &tr.methods);
                    self.validate_method_partial_declarations(
                        &tr.name,
                        &tr.method_aliases,
                        &tr.method_partials,
                        &tr.properties,
                        &tr.methods,
                    );
                }
                Declaration::Newtype(nt) => {
                    self.validate_method_alias_declarations(&nt.name, &nt.method_aliases, &[], &nt.methods);
                    self.validate_method_partial_declarations(
                        &nt.name,
                        &nt.method_aliases,
                        &nt.method_partials,
                        &[],
                        &nt.methods,
                    );
                    if let Type::Simple(target) = &nt.underlying.node {
                        newtype_underlyings.insert(nt.name.clone(), (target.clone(), nt.underlying.span));
                    }
                }
                _ => {}
            }
        }

        self.validate_top_level_alias_cycles(&alias_targets);
        self.validate_top_level_partial_cycles(&alias_targets, &partial_targets);
        self.validate_newtype_underlying_cycles(&newtype_underlyings, &newtype_names);
    }

    /// Reject direct and indirect cycles between top-level aliases.
    fn validate_top_level_alias_cycles(&mut self, alias_targets: &HashMap<String, (String, Span)>) {
        let mut reported = HashSet::new();
        for (name, (_, span)) in alias_targets {
            let mut path = Vec::new();
            let mut current = name.clone();
            while let Some((target, _)) = alias_targets.get(&current) {
                if let Some(cycle_start) = path.iter().position(|seen| seen == &current) {
                    let mut cycle = path[cycle_start..].to_vec();
                    cycle.push(current.clone());
                    let key = cycle.join(" -> ");
                    if reported.insert(key.clone()) {
                        self.errors
                            .push(CompileError::type_error(format!("Alias cycle detected: {key}"), *span));
                    }
                    break;
                }
                path.push(current);
                current = target.clone();
            }
        }
    }

    /// Reject direct and indirect cycles in top-level partial declarations, including cycles that route through
    /// aliases.
    fn validate_top_level_partial_cycles(
        &mut self,
        alias_targets: &HashMap<String, (String, Span)>,
        partial_targets: &HashMap<String, (String, Span)>,
    ) {
        let mut edges: HashMap<String, (String, Span)> = alias_targets.clone();
        edges.extend(partial_targets.clone());

        let mut reported = HashSet::new();
        for (name, (_, span)) in partial_targets {
            let mut path = Vec::new();
            let mut current = name.clone();
            while let Some((target, _)) = edges.get(&current) {
                if let Some(cycle_start) = path.iter().position(|seen| seen == &current) {
                    let mut cycle = path[cycle_start..].to_vec();
                    cycle.push(current.clone());
                    let key = cycle.join(" -> ");
                    if cycle.iter().any(|segment| partial_targets.contains_key(segment)) && reported.insert(key.clone())
                    {
                        self.errors.push(CompileError::type_error(
                            format!("Partial cycle detected: {key}"),
                            *span,
                        ));
                    }
                    break;
                }
                path.push(current);
                current = target.clone();
            }
        }
    }

    /// Public partial provenance must remain checkable for consumers without private module state.
    fn validate_public_partial_presets_do_not_reference_private_symbols(
        &mut self,
        partial_name: &str,
        args: &[PartialArg],
        public_decls: &HashMap<String, bool>,
    ) {
        for arg in args {
            self.validate_public_partial_preset_expr(partial_name, &arg.name, &arg.value, public_decls);
        }
    }

    /// Recursively reject private roots used by public partial preset values.
    fn validate_public_partial_preset_expr(
        &mut self,
        partial_name: &str,
        preset_name: &str,
        expr: &Spanned<Expr>,
        public_decls: &HashMap<String, bool>,
    ) {
        if let Some(root) = preset_private_root(expr, public_decls) {
            self.errors.push(CompileError::type_error(
                format!("Public partial '{partial_name}' preset '{preset_name}' references private symbol '{root}'"),
                expr.span,
            ));
        }

        match &expr.node {
            Expr::Paren(inner) => {
                self.validate_public_partial_preset_expr(partial_name, preset_name, inner, public_decls)
            }
            Expr::List(entries) => {
                for entry in entries {
                    match entry {
                        ListEntry::Element(value) | ListEntry::Spread(value) => {
                            self.validate_public_partial_preset_expr(partial_name, preset_name, value, public_decls);
                        }
                    }
                }
            }
            Expr::Dict(entries) => {
                for entry in entries {
                    match entry {
                        DictEntry::Pair(key, value) => {
                            self.validate_public_partial_preset_expr(partial_name, preset_name, key, public_decls);
                            self.validate_public_partial_preset_expr(partial_name, preset_name, value, public_decls);
                        }
                        DictEntry::Spread(value) => {
                            self.validate_public_partial_preset_expr(partial_name, preset_name, value, public_decls);
                        }
                    }
                }
            }
            Expr::Call(_, _, args) => {
                for arg in args {
                    match arg {
                        CallArg::Positional(value)
                        | CallArg::Named(_, value)
                        | CallArg::PositionalUnpack(value)
                        | CallArg::KeywordUnpack(value) => {
                            self.validate_public_partial_preset_expr(partial_name, preset_name, value, public_decls)
                        }
                    }
                }
            }
            Expr::Constructor(_, args) => {
                for arg in args {
                    match arg {
                        CallArg::Positional(value)
                        | CallArg::Named(_, value)
                        | CallArg::PositionalUnpack(value)
                        | CallArg::KeywordUnpack(value) => {
                            self.validate_public_partial_preset_expr(partial_name, preset_name, value, public_decls)
                        }
                    }
                }
            }
            _ => {}
        }
    }

    /// Reject direct and indirect cycles in newtype underlying declarations before coercion planning.
    fn validate_newtype_underlying_cycles(
        &mut self,
        newtype_underlyings: &HashMap<String, (String, Span)>,
        newtype_names: &HashSet<String>,
    ) {
        let mut reported = HashSet::new();
        for (name, (_, span)) in newtype_underlyings {
            let mut path = Vec::new();
            let mut current = name.clone();
            while let Some((target, _)) = newtype_underlyings.get(&current) {
                if !newtype_names.contains(target) {
                    break;
                }
                if let Some(cycle_start) = path.iter().position(|seen| seen == &current) {
                    let mut cycle = path[cycle_start..].to_vec();
                    cycle.push(current.clone());
                    let key = cycle.join(" -> ");
                    if reported.insert(key.clone()) {
                        self.errors.push(errors::newtype_coercion_cycle(&key, *span));
                    }
                    break;
                }
                path.push(current);
                current = target.clone();
            }
        }
    }

    /// Validate same-type method alias names and targets for one method-bearing declaration.
    fn validate_method_alias_declarations(
        &mut self,
        owner: &str,
        aliases: &[Spanned<MethodAliasDecl>],
        properties: &[Spanned<PropertyDecl>],
        methods: &[Spanned<MethodDecl>],
    ) {
        let method_names: HashSet<&str> = methods.iter().map(|method| method.node.name.as_str()).collect();
        let property_names: HashSet<&str> = properties.iter().map(|property| property.node.name.as_str()).collect();
        let mut alias_targets = HashMap::new();
        let mut alias_names = HashSet::new();

        for alias in aliases {
            if !alias_names.insert(alias.node.name.as_str())
                || method_names.contains(alias.node.name.as_str())
                || property_names.contains(alias.node.name.as_str())
            {
                self.errors.push(CompileError::type_error(
                    format!("Duplicate method alias '{}' on '{}'", alias.node.name, owner),
                    alias.span,
                ));
            }
            if !method_names.contains(alias.node.target.as_str()) {
                self.errors.push(CompileError::type_error(
                    format!(
                        "Method alias '{}.{}' targets unknown method '{}'",
                        owner, alias.node.name, alias.node.target
                    ),
                    alias.span,
                ));
            }
            alias_targets.insert(alias.node.name.clone(), (alias.node.target.clone(), alias.span));
        }

        self.validate_method_alias_cycles(owner, &alias_targets);
    }

    /// Reject computed property names that collide with storage fields, aliases, properties, or methods.
    fn validate_member_name_collisions(
        &mut self,
        owner: &str,
        fields: &[Spanned<FieldDecl>],
        aliases: &[Spanned<MethodAliasDecl>],
        partials: &[Spanned<MethodPartialDecl>],
        properties: &[Spanned<PropertyDecl>],
        methods: &[Spanned<MethodDecl>],
    ) {
        let mut seen: HashMap<&str, (&str, Span)> = HashMap::new();
        for field in fields {
            seen.entry(field.node.name.as_str()).or_insert(("field", field.span));
        }
        for alias in aliases {
            seen.entry(alias.node.name.as_str())
                .or_insert(("method alias", alias.span));
        }
        for partial in partials {
            seen.entry(partial.node.name.as_str())
                .or_insert(("method partial", partial.span));
        }
        let mut method_seen: HashMap<&str, Span> = HashMap::new();
        for method in methods {
            if method_seen.insert(method.node.name.as_str(), method.span).is_none()
                && !seen.contains_key(method.node.name.as_str())
            {
                seen.insert(method.node.name.as_str(), ("method", method.span));
            }
        }
        for property in properties {
            self.validate_one_member_name(owner, "property", property.node.name.as_str(), property.span, &mut seen);
        }
    }

    /// Validate one property name against the member names already used in the same owner declaration.
    fn validate_one_member_name<'a>(
        &mut self,
        owner: &str,
        kind: &'static str,
        name: &'a str,
        span: Span,
        seen: &mut HashMap<&'a str, (&'static str, Span)>,
    ) {
        if let Some((previous_kind, previous_span)) = seen.insert(name, (kind, span)) {
            self.errors.push(
                CompileError::type_error(
                    format!(
                        "Duplicate member '{}.{}' declared as both {} and {}",
                        owner, name, previous_kind, kind
                    ),
                    span,
                )
                .with_note(format!(
                    "First declaration span: {}..{}",
                    previous_span.start, previous_span.end
                )),
            );
        }
    }

    /// Reject direct and indirect cycles between method aliases on one owner type.
    fn validate_method_alias_cycles(&mut self, owner: &str, alias_targets: &HashMap<String, (String, Span)>) {
        let mut reported = HashSet::new();
        for (name, (_, span)) in alias_targets {
            let mut path = Vec::new();
            let mut current = name.clone();
            while let Some((target, _)) = alias_targets.get(&current) {
                if let Some(cycle_start) = path.iter().position(|seen| seen == &current) {
                    let mut cycle = path[cycle_start..].to_vec();
                    cycle.push(current.clone());
                    let key = cycle.join(" -> ");
                    if reported.insert(key.clone()) {
                        self.errors.push(CompileError::type_error(
                            format!("Method alias cycle detected on '{owner}': {key}"),
                            *span,
                        ));
                    }
                    break;
                }
                path.push(current);
                current = target.clone();
            }
        }
    }

    /// Validate same-type method partial names and targets before collection turns them into method metadata.
    fn validate_method_partial_declarations(
        &mut self,
        owner: &str,
        aliases: &[Spanned<MethodAliasDecl>],
        partials: &[Spanned<MethodPartialDecl>],
        properties: &[Spanned<PropertyDecl>],
        methods: &[Spanned<MethodDecl>],
    ) {
        let method_names: HashSet<&str> = methods.iter().map(|method| method.node.name.as_str()).collect();
        let alias_names: HashSet<&str> = aliases.iter().map(|alias| alias.node.name.as_str()).collect();
        let property_names: HashSet<&str> = properties.iter().map(|property| property.node.name.as_str()).collect();
        let mut partial_names = HashSet::new();

        for partial in partials {
            if !partial_names.insert(partial.node.name.as_str())
                || method_names.contains(partial.node.name.as_str())
                || alias_names.contains(partial.node.name.as_str())
                || property_names.contains(partial.node.name.as_str())
            {
                self.errors.push(CompileError::type_error(
                    format!("Duplicate method partial '{}.{}'", owner, partial.node.name),
                    partial.span,
                ));
            }
            if !method_names.contains(partial.node.target.as_str())
                && !alias_names.contains(partial.node.target.as_str())
            {
                self.errors.push(CompileError::type_error(
                    format!(
                        "Method partial '{}.{}' targets unknown method '{}'",
                        owner, partial.node.name, partial.node.target
                    ),
                    partial.span,
                ));
            }
        }
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
        self.static_decls.clear();
        self.local_function_decls.clear();
        self.static_decl_positions.clear();
        self.const_eval_state.clear();
        self.const_eval_cache.clear();
        self.type_info = TypeCheckInfo::default();
        self.warnings.clear();
        self.errors.clear();
        self.testing_marker_import_bindings.clear();
        self.testing_fixture_names.clear();
        self.surface_context = SurfaceContext::from_program(program);
        self.import_aliases = self.surface_context.import_aliases().clone();
        self.supertrait_closure.clear();
        self.transitive_pub_types.clear();
        self.transitive_pub_traits.clear();
        self.cached_pub_libraries.clear();
        self.validate_alias_declarations(program);

        // `check_with_imports` / `import_module` can queue supertrait bounds while collecting dependency ASTs.
        // Resolve those queued bounds into trait symbols before we collect and resolve the current program.
        if !self.pending_trait_supertraits.is_empty() {
            self.resolve_pending_trait_supertraits();
        }

        // First pass: collect concrete declarations, then aliases and partials after their possible targets are
        // available.
        for decl in &program.declarations {
            if !matches!(decl.node, Declaration::Alias(_) | Declaration::Partial(_)) {
                self.collect_declaration(decl);
            }
        }
        for decl in &program.declarations {
            if matches!(decl.node, Declaration::Alias(_)) {
                self.collect_declaration(decl);
            }
        }
        for decl in &program.declarations {
            if matches!(decl.node, Declaration::Partial(_)) {
                self.collect_declaration(decl);
            }
        }

        self.resolve_pending_trait_supertraits();
        self.finalize_supertrait_graph();
        self.merge_supertrait_requires_into_traits();
        self.validate_static_dependencies();
        self.collect_testing_fixture_names(program);

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

        self.record_trait_metadata_for_lowering();

        // ---- RFC 023: validate rust.module() and @rust.extern rules ----
        self.validate_rust_module_and_extern(program);

        // Split fatal errors from non-fatal diagnostics.
        let all = std::mem::take(&mut self.errors);
        let (fatal, non_fatal): (Vec<_>, Vec<_>) = all
            .into_iter()
            .partition(|e| !matches!(e.kind, ErrorKind::Warning | ErrorKind::Lint));
        self.warnings.extend(non_fatal);

        if fatal.is_empty() { Ok(()) } else { Err(fatal) }
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
            if is_public_decl(decl) && !matches!(decl.node, Declaration::Alias(_) | Declaration::Partial(_)) {
                self.collect_declaration(decl);
            }
        }
        for decl in &module_ast.declarations {
            if is_public_decl(decl) && matches!(decl.node, Declaration::Alias(_)) {
                self.collect_declaration(decl);
            }
        }
        for decl in &module_ast.declarations {
            if is_public_decl(decl) && matches!(decl.node, Declaration::Partial(_)) {
                self.collect_declaration(decl);
            }
        }
        self.register_dependency_derivable_metadata(_module_name, module_ast);
    }

    /// Import all symbols from another module's AST into the symbol table.
    ///
    /// This is used for internal compiler passes that need type information across modules
    /// without enforcing `pub` visibility (e.g. codegen-only validation for dependencies).
    pub fn import_module_all(&mut self, module_ast: &Program, _module_name: &str) {
        for decl in &module_ast.declarations {
            if !matches!(decl.node, Declaration::Alias(_) | Declaration::Partial(_)) {
                self.collect_declaration(decl);
            }
        }
        for decl in &module_ast.declarations {
            if matches!(decl.node, Declaration::Alias(_)) {
                self.collect_declaration(decl);
            }
        }
        for decl in &module_ast.declarations {
            if matches!(decl.node, Declaration::Partial(_)) {
                self.collect_declaration(decl);
            }
        }
        self.register_dependency_derivable_metadata(_module_name, module_ast);
    }

    /// Register RFC 024 metadata exported by a dependency source module.
    ///
    /// `__derives__` is compiler metadata rather than a public const, so consumers need this side channel even when the
    /// declaration itself is not imported as a symbol.
    fn register_dependency_derivable_metadata(&mut self, module_name: &str, module_ast: &Program) {
        let mut module_paths = vec![module_name.to_string()];
        let dotted_module_path = module_name.replace('_', ".");
        if dotted_module_path != module_name {
            module_paths.push(dotted_module_path);
        }
        let traits = Self::derivable_traits_from_program(module_ast);
        if !traits.is_empty() {
            for module_path in &module_paths {
                self.dependency_derivable_modules
                    .insert(module_path.clone(), traits.clone());
            }
        }

        for decl in &module_ast.declarations {
            let Declaration::Trait(tr) = &decl.node else {
                continue;
            };
            if let Some(info) = self.lookup_trait_info(&tr.name).cloned() {
                for module_path in &module_paths {
                    self.dependency_module_traits
                        .insert(format!("{module_path}.{}", tr.name), info.clone());
                }
            }
            let paths = Self::rust_derive_paths_from_trait(tr);
            if paths.is_empty() {
                continue;
            }
            for module_path in &module_paths {
                let qualified = format!("{module_path}.{}", tr.name);
                self.dependency_trait_rust_derive_paths.insert(qualified, paths.clone());
            }
        }
    }

    /// Extract RFC 024 `__derives__` trait names from an imported dependency AST.
    fn derivable_traits_from_program(program: &Program) -> Vec<String> {
        for decl in &program.declarations {
            let Declaration::Const(konst) = &decl.node else {
                continue;
            };
            if konst.name != "__derives__" {
                continue;
            }
            let Expr::List(entries) = &konst.value.node else {
                return Vec::new();
            };
            return entries
                .iter()
                .filter_map(|entry| match entry {
                    ListEntry::Element(expr) => match &expr.node {
                        Expr::Ident(name) => Some(name.clone()),
                        _ => None,
                    },
                    ListEntry::Spread(_) => None,
                })
                .collect();
        }
        Vec::new()
    }

    /// Extract `@rust.derive(...)` path strings from one imported trait declaration.
    fn rust_derive_paths_from_trait(tr: &TraitDecl) -> Vec<String> {
        let mut paths = Vec::new();
        for decorator in &tr.decorators {
            let path = decorator.node.path.segments.join(".");
            if core_decorators::from_str(&path) != Some(DecoratorId::RustDerive) {
                continue;
            }
            for arg in &decorator.node.args {
                let DecoratorArg::Positional(expr) = arg else {
                    continue;
                };
                let Expr::Literal(Literal::String(path)) = &expr.node else {
                    continue;
                };
                if !paths.iter().any(|existing| existing == path) {
                    paths.push(path.clone());
                }
            }
        }
        paths
    }

    /// Predeclare dependency interface names before collecting imported declarations.
    ///
    /// Cross-module public signatures may reference types and traits from other dependency modules, including cyclic
    /// interfaces such as `session -> dataset -> session`. A predeclaration pass breaks that order sensitivity by
    /// making type- and trait-like names resolvable before method and function signatures are collected.
    fn predeclare_dependency_interfaces(&mut self, dependencies: &[(&str, &Program)], public_only: bool) {
        for (_, dep_ast) in dependencies {
            for decl in &dep_ast.declarations {
                if public_only && !is_public_decl(decl) {
                    continue;
                }
                self.predeclare_dependency_decl(decl);
            }
        }
    }

    /// Seed the symbol table with a minimal placeholder for one dependency declaration.
    ///
    /// The subsequent `import_module*` pass overwrites these placeholders with full collected metadata. These shells
    /// exist only so `resolve_type_checked` can keep interface types concrete instead of degrading them to `TypeVar` or
    /// `Unknown` during cyclic or transitive dependency import collection.
    fn predeclare_dependency_decl(&mut self, decl: &Spanned<Declaration>) {
        match &decl.node {
            Declaration::Model(model) if self.lookup_symbol(model.name.as_str()).is_none() => {
                self.symbols.define(Symbol {
                    name: model.name.clone(),
                    kind: SymbolKind::Type(TypeInfo::Model(ModelInfo {
                        type_params: model.type_params.iter().map(|tp| tp.name.clone()).collect(),
                        traits: Vec::new(),
                        trait_adoptions: Vec::new(),
                        derives: Vec::new(),
                        fields: HashMap::new(),
                        properties: HashMap::new(),
                        methods: HashMap::new(),
                        method_overloads: HashMap::new(),
                        method_aliases: HashMap::new(),
                    })),
                    span: decl.span,
                    scope: 0,
                });
            }
            Declaration::Class(class) if self.lookup_symbol(class.name.as_str()).is_none() => {
                self.symbols.define(Symbol {
                    name: class.name.clone(),
                    kind: SymbolKind::Type(TypeInfo::Class(ClassInfo {
                        type_params: class.type_params.iter().map(|tp| tp.name.clone()).collect(),
                        extends: class.extends.clone(),
                        traits: Vec::new(),
                        trait_adoptions: Vec::new(),
                        derives: Vec::new(),
                        fields: HashMap::new(),
                        properties: HashMap::new(),
                        methods: HashMap::new(),
                        method_overloads: HashMap::new(),
                        method_aliases: HashMap::new(),
                    })),
                    span: decl.span,
                    scope: 0,
                });
            }
            Declaration::Trait(tr) if self.lookup_symbol(tr.name.as_str()).is_none() => {
                self.symbols.define(Symbol {
                    name: tr.name.clone(),
                    kind: SymbolKind::Trait(TraitInfo {
                        type_params: tr.type_params.iter().map(|tp| tp.name.clone()).collect(),
                        methods: HashMap::new(),
                        method_aliases: HashMap::new(),
                        properties: HashMap::new(),
                        requires: Vec::new(),
                        supertraits: Vec::new(),
                    }),
                    span: decl.span,
                    scope: 0,
                });
            }
            Declaration::TypeAlias(alias) if self.lookup_symbol(alias.name.as_str()).is_none() => {
                self.symbols.define(Symbol {
                    name: alias.name.clone(),
                    kind: SymbolKind::Type(TypeInfo::TypeAlias),
                    span: decl.span,
                    scope: 0,
                });
            }
            Declaration::Newtype(nt) if self.lookup_symbol(nt.name.as_str()).is_none() => {
                self.symbols.define(Symbol {
                    name: nt.name.clone(),
                    kind: SymbolKind::Type(TypeInfo::Newtype(NewtypeInfo {
                        type_params: nt.type_params.iter().map(|tp| tp.name.clone()).collect(),
                        is_rusttype: nt.is_rusttype,
                        has_interop: !nt.interop_edges.is_empty(),
                        underlying: ResolvedType::Unknown,
                        constraints: Vec::new(),
                        implicit_coercion_enabled: true,
                        method_rebindings: HashMap::new(),
                        traits: nt.traits.iter().map(|trait_ref| trait_ref.node.name.clone()).collect(),
                        trait_adoptions: Vec::new(),
                        method_aliases: HashMap::new(),
                        methods: HashMap::new(),
                        method_overloads: HashMap::new(),
                    })),
                    span: decl.span,
                    scope: 0,
                });
            }
            Declaration::Enum(en) if self.lookup_symbol(en.name.as_str()).is_none() => {
                self.symbols.define(Symbol {
                    name: en.name.clone(),
                    kind: SymbolKind::Type(TypeInfo::Enum(EnumInfo {
                        type_params: en.type_params.iter().map(|tp| tp.name.clone()).collect(),
                        traits: en.traits.iter().map(|t| t.node.name.clone()).collect(),
                        trait_adoptions: Vec::new(),
                        variants: en.variants.iter().map(|v| v.node.name.clone()).collect(),
                        variant_aliases: en
                            .variant_aliases
                            .iter()
                            .map(|alias| (alias.node.name.clone(), alias.node.target.clone()))
                            .collect(),
                        value_enum: None,
                        derives: Vec::new(),
                        methods: HashMap::new(),
                        method_overloads: HashMap::new(),
                    })),
                    span: decl.span,
                    scope: 0,
                });
            }
            _ => {}
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
        self.dependency_derivable_modules.clear();
        self.dependency_module_traits.clear();
        self.dependency_trait_rust_derive_paths.clear();
        for (name, dep_ast) in dependencies {
            self.dependency_exports
                .insert(name.to_string(), exported_symbols(dep_ast));
        }
        self.predeclare_dependency_interfaces(dependencies, true);
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
        self.dependency_derivable_modules.clear();
        self.dependency_module_traits.clear();
        self.dependency_trait_rust_derive_paths.clear();
        self.predeclare_dependency_interfaces(dependencies, false);
        for (name, dep_ast) in dependencies {
            self.import_module_all(dep_ast, name);
        }
        self.check_program(program)
    }

    // ========================================================================
    // Type compatibility (shared helper)
    // ========================================================================

    /// Record an RFC 017 validated-newtype coercion when `actual` may flow into `expected` at an approved site.
    ///
    /// This deliberately stays outside [`Self::types_compatible`]: ordinary compatibility queries have no source span
    /// and are used in contexts where implicit coercion must not be inserted.
    pub(crate) fn record_validated_newtype_coercion_if_possible(
        &mut self,
        actual: &ResolvedType,
        expected: &ResolvedType,
        span: Span,
    ) -> bool {
        let Some(coercion) = self.validated_newtype_coercion(actual, expected, span) else {
            return false;
        };
        self.type_info.record_validated_newtype_coercion(span, coercion);
        true
    }

    /// Record an RFC 017 model/class field coercion that should aggregate validation errors.
    pub(crate) fn record_validated_newtype_field_coercion_if_possible(
        &mut self,
        actual: &ResolvedType,
        expected: &ResolvedType,
        field_name: &str,
        span: Span,
    ) -> bool {
        let Some(mut coercion) = self.validated_newtype_coercion(actual, expected, span) else {
            return false;
        };
        coercion.mode = ValidatedNewtypeCoercionMode::AggregateField {
            field_name: field_name.to_string(),
        };
        self.type_info.record_validated_newtype_coercion(span, coercion);
        true
    }

    /// Return the coercion metadata needed to convert `actual` into expected newtype `expected`.
    fn validated_newtype_coercion(
        &mut self,
        actual: &ResolvedType,
        expected: &ResolvedType,
        span: Span,
    ) -> Option<ValidatedNewtypeCoercionInfo> {
        if actual == expected {
            return None;
        }
        let steps = self.validated_newtype_coercion_steps(actual, expected, &mut HashSet::new(), span)?;
        Some(ValidatedNewtypeCoercionInfo {
            steps,
            target_type: expected.clone(),
            mode: ValidatedNewtypeCoercionMode::FailFast,
        })
    }

    /// Build the ordered underlying-to-target chain for one validated-newtype coercion.
    fn validated_newtype_coercion_steps(
        &mut self,
        actual: &ResolvedType,
        expected: &ResolvedType,
        visiting: &mut HashSet<String>,
        span: Span,
    ) -> Option<Vec<ValidatedNewtypeCoercionStep>> {
        let target_name = match expected {
            ResolvedType::Named(name) => name,
            // Generic newtypes need type-parameter substitution for their underlying type. Keep that out of the
            // initial implicit path instead of silently guessing.
            ResolvedType::Generic(_, _) => return None,
            _ => return None,
        };
        if !visiting.insert(target_name.clone()) {
            self.errors.push(errors::newtype_coercion_cycle(target_name, span));
            return None;
        }

        let Some(TypeInfo::Newtype(newtype)) = self.lookup_semantic_type_info(target_name).cloned() else {
            visiting.remove(target_name);
            return None;
        };
        if newtype.is_rusttype {
            visiting.remove(target_name);
            return None;
        }

        let mut steps = if Self::validated_newtype_source_matches_underlying(actual, &newtype.underlying) {
            Vec::new()
        } else {
            self.validated_newtype_coercion_steps(actual, &newtype.underlying, visiting, span)?
        };
        if !newtype.implicit_coercion_enabled {
            self.errors
                .push(errors::implicit_newtype_coercion_disabled(target_name, span));
            visiting.remove(target_name);
            return Some(Vec::new());
        }
        steps.push(ValidatedNewtypeCoercionStep {
            newtype_name: target_name.clone(),
            ctor: self.validated_newtype_ctor_name(target_name, &newtype),
            constraints: if self.validated_newtype_ctor_name(target_name, &newtype).is_some() {
                Vec::new()
            } else {
                newtype.constraints.clone()
            },
        });
        visiting.remove(target_name);
        Some(steps)
    }

    /// Return whether a source value can be passed to a newtype's underlying constructor without primitive parsing.
    fn validated_newtype_source_matches_underlying(actual: &ResolvedType, underlying: &ResolvedType) -> bool {
        matches!(actual, ResolvedType::Unknown | ResolvedType::TypeVar(_)) || actual == underlying
    }

    /// Return the canonical checked-construction hook for a newtype when the collected method shape is usable.
    fn validated_newtype_ctor_name(&self, newtype_name: &str, newtype: &NewtypeInfo) -> Option<String> {
        let method = newtype.methods.get(conventions::NEWTYPE_FROM_UNDERLYING_METHOD)?;
        if method.receiver.is_some() || method.params.len() != 1 {
            return None;
        }
        if !Self::validated_newtype_source_matches_underlying(&method.params[0].ty, &newtype.underlying) {
            return None;
        }
        let ResolvedType::Generic(result_name, result_args) = &method.return_type else {
            return None;
        };
        if collection_type_id(result_name.as_str()) != Some(CollectionTypeId::Result) {
            return None;
        }
        let ok_ty = result_args.first()?;
        if !(matches!(ok_ty, ResolvedType::Named(name) if name == newtype_name)
            || matches!(ok_ty, ResolvedType::SelfType))
        {
            return None;
        }
        let err_ty = result_args.get(1)?;
        if !matches!(err_ty, ResolvedType::Named(name) if name == "ValidationError") {
            return None;
        }
        Some(conventions::NEWTYPE_FROM_UNDERLYING_METHOD.to_string())
    }

    /// Check if two types are compatible for assignment or comparison.
    ///
    /// Returns `true` if `actual` can be used where `expected` is required. Handles `Unknown` (error recovery), type
    /// variables (generics), and recursive checks for generics, functions, and tuples.
    #[allow(clippy::only_used_in_recursion)]
    pub(crate) fn types_compatible(&self, actual: &ResolvedType, expected: &ResolvedType) -> bool {
        let has_generic_params = |name: &str| -> bool {
            match self.lookup_semantic_type_info(name) {
                Some(TypeInfo::Model(model)) => !model.type_params.is_empty(),
                Some(TypeInfo::Class(class)) => !class.type_params.is_empty(),
                Some(TypeInfo::Newtype(newtype)) => !newtype.type_params.is_empty(),
                Some(TypeInfo::Enum(enum_info)) => !enum_info.type_params.is_empty(),
                _ => false,
            }
        };

        if actual == expected {
            return true;
        }

        if let Some(matches) = self.rust_type_identities_compatible(actual, expected) {
            return matches;
        }

        match (actual, expected) {
            (ResolvedType::Unknown, _) | (_, ResolvedType::Unknown) => true,
            (ResolvedType::TypeVar(_), _) | (_, ResolvedType::TypeVar(_)) => true,
            (ResolvedType::CallSiteInfer, _) | (_, ResolvedType::CallSiteInfer) => true,
            (ResolvedType::SelfType, ResolvedType::Generic(trait_name, _))
                if self.current_trait_name.as_deref() == Some(trait_name.as_str()) =>
            {
                true
            }
            (actual, expected) if numeric_lossless_compatible(actual, expected) => true,
            (
                ResolvedType::Generic(actual_name, actual_members),
                ResolvedType::Generic(expected_name, expected_members),
            ) if actual_name == UNION_TYPE_NAME && expected_name == UNION_TYPE_NAME => {
                actual_members.iter().all(|actual_member| {
                    expected_members
                        .iter()
                        .any(|expected_member| self.types_compatible(actual_member, expected_member))
                })
            }
            (ResolvedType::Generic(name, members), expected) if name == UNION_TYPE_NAME => {
                members.iter().all(|member| self.types_compatible(member, expected))
            }
            (actual, ResolvedType::Generic(name, members)) if name == UNION_TYPE_NAME => {
                members.iter().any(|member| self.types_compatible(actual, member))
            }
            (actual, expected) if expected.is_option() && !actual.is_option() => {
                let Some(inner) = expected.option_inner_type() else {
                    return false;
                };
                self.types_compatible(actual, inner)
            }
            (ResolvedType::Generic(name, actual_args), ResolvedType::Generic(trait_name, expected_args))
                if collection_type_id(name.as_str()) == Some(CollectionTypeId::Generator)
                    && expected_args.len() == 1
                    && (trait_name == builtin_traits::as_str(TraitId::Iterable)
                        || trait_name == builtin_traits::as_str(TraitId::Iterator)
                        || trait_name == builtin_traits::as_str(TraitId::IntoIterator)) =>
            {
                actual_args.len() == 1 && self.types_compatible(&actual_args[0], &expected_args[0])
            }

            // ---- Context: RFC 042 — `expected` is a trait reference (`Named` or nullary trait on RHS) ----
            (ResolvedType::Named(type_name), ResolvedType::Named(trait_name))
                if self.lookup_semantic_trait_info(trait_name).is_some() =>
            {
                if self.lookup_semantic_trait_info(type_name).is_some() {
                    self.trait_is_supertrait_of(type_name, trait_name)
                } else {
                    self.type_implements_trait(type_name, trait_name)
                }
            }
            (ResolvedType::Generic(type_name, actual_args), ResolvedType::Named(trait_name))
                if self.lookup_semantic_trait_info(trait_name).is_some() =>
            {
                if self.lookup_semantic_trait_info(type_name).is_some() {
                    let Some(super_info) = self.lookup_semantic_trait_info(trait_name) else {
                        return false;
                    };
                    if !super_info.type_params.is_empty() {
                        return false;
                    }
                    self.trait_is_supertrait_of(type_name, trait_name)
                        && self
                            .instantiated_supertrait_args(type_name, actual_args, trait_name)
                            .is_some_and(|inst| inst.is_empty())
                } else {
                    self.type_implements_trait(type_name, trait_name)
                }
            }

            // ---- Context: RFC 042 — `Subtrait[T…]` assignable to `Supertrait[T…]` (trait upcast) ----
            (
                ResolvedType::Generic(subtrait_name, actual_args),
                ResolvedType::Generic(supertrait_name, expected_args),
            ) if self.lookup_semantic_trait_info(subtrait_name).is_some()
                && self.lookup_semantic_trait_info(supertrait_name).is_some() =>
            {
                let Some(instantiated) = self.instantiated_supertrait_args(subtrait_name, actual_args, supertrait_name)
                else {
                    return false;
                };
                if instantiated.len() != expected_args.len() {
                    return false;
                }
                expected_args
                    .iter()
                    .zip(instantiated.iter())
                    .all(|(e, a)| self.types_compatible(a, e))
            }

            // RFC 088: builtin collection iterables and `Iterator[T]` values satisfy `Iterable[T]`.
            (ResolvedType::Generic(actual_name, actual_args), ResolvedType::Generic(trait_name, expected_args))
                if trait_name == builtin_traits::as_str(TraitId::Iterable)
                    && expected_args.len() == 1
                    && actual_args.len() == 1 =>
            {
                let actual_is_iterable = actual_name == builtin_traits::as_str(TraitId::Iterator)
                    || matches!(
                        collection_type_id(actual_name.as_str()),
                        Some(
                            CollectionTypeId::List
                                | CollectionTypeId::Set
                                | CollectionTypeId::FrozenList
                                | CollectionTypeId::FrozenSet
                        )
                    );
                actual_is_iterable && self.types_compatible(&actual_args[0], &expected_args[0])
            }
            (ResolvedType::FrozenList(actual), ResolvedType::Generic(trait_name, expected_args))
                if trait_name == builtin_traits::as_str(TraitId::Iterable) && expected_args.len() == 1 =>
            {
                self.types_compatible(actual, &expected_args[0])
            }
            (ResolvedType::FrozenSet(actual), ResolvedType::Generic(trait_name, expected_args))
                if trait_name == builtin_traits::as_str(TraitId::Iterable) && expected_args.len() == 1 =>
            {
                self.types_compatible(actual, &expected_args[0])
            }

            // RFC 042: `Concrete[T]` assignable to generic trait annotation `Trait[T]` (and similar).
            (ResolvedType::Generic(type_name, actual_args), ResolvedType::Generic(trait_name, expected_args))
                if self.lookup_semantic_trait_info(trait_name).is_some()
                    && self.lookup_semantic_trait_info(type_name).is_none()
                    && self.lookup_semantic_type_info(type_name).is_some() =>
            {
                let Some(instantiated_args) = self.instantiated_trait_args_for_type(type_name, actual_args, trait_name)
                else {
                    return false;
                };
                let concrete_arity_ok = match self.lookup_semantic_type_info(type_name) {
                    Some(TypeInfo::Model(m)) => actual_args.len() == m.type_params.len(),
                    Some(TypeInfo::Class(c)) => actual_args.len() == c.type_params.len(),
                    Some(TypeInfo::Newtype(n)) => actual_args.len() == n.type_params.len(),
                    Some(TypeInfo::Enum(e)) => actual_args.len() == e.type_params.len(),
                    _ => true,
                };
                if !concrete_arity_ok {
                    return false;
                }
                if instantiated_args.len() != expected_args.len() {
                    return false;
                }
                expected_args
                    .iter()
                    .zip(instantiated_args.iter())
                    .all(|(e, a)| self.types_compatible(a, e))
            }

            // ---- Context: RFC 042 — `Named` actual vs `Trait` / `Trait[T…]` expected (incl. trait upcast) ----
            (ResolvedType::Named(type_name), ResolvedType::Generic(trait_name, expected_args))
                if self.lookup_semantic_trait_info(trait_name).is_some() =>
            {
                if self.lookup_semantic_trait_info(type_name).is_some() {
                    let Some(instantiated_args) = self.instantiated_supertrait_args(type_name, &[], trait_name) else {
                        return false;
                    };
                    if instantiated_args.len() != expected_args.len() {
                        return false;
                    }
                    expected_args
                        .iter()
                        .zip(instantiated_args.iter())
                        .all(|(e, a)| self.types_compatible(a, e))
                } else {
                    let Some(instantiated_args) = self.instantiated_trait_args_for_type(type_name, &[], trait_name)
                    else {
                        return false;
                    };
                    if instantiated_args.len() != expected_args.len() {
                        return false;
                    }
                    expected_args
                        .iter()
                        .zip(instantiated_args.iter())
                        .all(|(expected, actual)| self.types_compatible(actual, expected))
                }
            }
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
            (ResolvedType::Named(name), ResolvedType::Generic(generic_name, _))
                if name == generic_name && has_generic_params(name) =>
            {
                true
            }
            (ResolvedType::Generic(generic_name, _), ResolvedType::Named(name))
                if name == generic_name && has_generic_params(name) =>
            {
                true
            }
            // Internal references: `&mut T` may satisfy `&T`, but not the reverse.
            (ResolvedType::Ref(a), ResolvedType::Ref(b))
            | (ResolvedType::RefMut(a), ResolvedType::Ref(b))
            | (ResolvedType::RefMut(a), ResolvedType::RefMut(b)) => self.types_compatible(a, b),
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
                    && p1
                        .iter()
                        .zip(p2.iter())
                        .all(|(t1, t2)| t1.kind == t2.kind && self.types_compatible(&t1.ty, &t2.ty))
                    && self.types_compatible(r1, r2)
            }
            (ResolvedType::Tuple(e1), ResolvedType::Tuple(e2)) => {
                e1.len() == e2.len() && e1.iter().zip(e2.iter()).all(|(t1, t2)| self.types_compatible(t1, t2))
            }
            (ResolvedType::RustPath(a), ResolvedType::RustPath(b)) => a == b,
            // Without full Rust type knowledge, treat any `RustPath` as compatible with non-Rust surfaces so mixed
            // Incan/Rust-typed expressions stay checkable (RFC 005/041 permissive model).
            (ResolvedType::RustPath(_), _) | (_, ResolvedType::RustPath(_)) => true,
            _ => false,
        }
    }
}

impl Default for TypeChecker {
    fn default() -> Self {
        Self::new()
    }
}

/// Return whether a declaration participates in public module import collection.
fn is_public_decl(decl: &Spanned<Declaration>) -> bool {
    match &decl.node {
        Declaration::Const(c) => matches!(c.visibility, Visibility::Public),
        Declaration::Static(s) => matches!(s.visibility, Visibility::Public),
        Declaration::Model(m) => matches!(m.visibility, Visibility::Public),
        Declaration::Class(c) => matches!(c.visibility, Visibility::Public),
        Declaration::Enum(e) => matches!(e.visibility, Visibility::Public),
        Declaration::Alias(a) => matches!(a.visibility, Visibility::Public),
        Declaration::TypeAlias(a) => matches!(a.visibility, Visibility::Public),
        Declaration::Newtype(n) => matches!(n.visibility, Visibility::Public),
        Declaration::Trait(t) => matches!(t.visibility, Visibility::Public),
        Declaration::Function(f) => matches!(f.visibility, Visibility::Public),
        Declaration::Partial(partial) => matches!(partial.visibility, Visibility::Public),
        Declaration::Import(_) | Declaration::Docstring(_) | Declaration::TestModule(_) => false,
    }
}

/// Return the root symbol name declared by a declaration, when it has one.
fn declaration_name(decl: &Spanned<Declaration>) -> Option<&str> {
    match &decl.node {
        Declaration::Const(c) => Some(c.name.as_str()),
        Declaration::Static(s) => Some(s.name.as_str()),
        Declaration::Model(m) => Some(m.name.as_str()),
        Declaration::Class(c) => Some(c.name.as_str()),
        Declaration::Enum(e) => Some(e.name.as_str()),
        Declaration::Alias(a) => Some(a.name.as_str()),
        Declaration::TypeAlias(a) => Some(a.name.as_str()),
        Declaration::Newtype(n) => Some(n.name.as_str()),
        Declaration::Trait(t) => Some(t.name.as_str()),
        Declaration::Function(f) => Some(f.name.as_str()),
        Declaration::Partial(partial) => Some(partial.name.as_str()),
        Declaration::Import(_) | Declaration::Docstring(_) | Declaration::TestModule(_) => None,
    }
}

/// Return the private top-level root named by a declaration-safe preset expression, if any.
fn preset_private_root(expr: &Spanned<Expr>, public_decls: &HashMap<String, bool>) -> Option<String> {
    match &expr.node {
        Expr::Ident(name) => public_decls
            .get(name)
            .is_some_and(|is_public| !*is_public)
            .then(|| name.clone()),
        Expr::Field(base, _) => preset_private_root(base, public_decls),
        Expr::Paren(inner) => preset_private_root(inner, public_decls),
        Expr::Call(callee, _, _) => preset_private_root(callee, public_decls),
        Expr::Constructor(name, _) => public_decls
            .get(name)
            .is_some_and(|is_public| !*is_public)
            .then(|| name.clone()),
        _ => None,
    }
}

/// Extract a decimal precision or scale argument from a type-position integer literal.
fn decimal_type_int_arg(arg: &Spanned<Type>) -> Option<i64> {
    match &arg.node {
        Type::IntLiteral(value) => Some(value.value),
        _ => None,
    }
}

/// Return whether two resolved numeric types are compatible through lossless widening.
fn numeric_lossless_compatible(actual: &ResolvedType, expected: &ResolvedType) -> bool {
    let Some(actual_id) = numeric_type_id_for_compat(actual) else {
        return false;
    };
    let Some(expected_id) = numeric_type_id_for_compat(expected) else {
        return false;
    };
    numeric_type_losslessly_widens_to(actual_id, expected_id)
}

/// Map an ordinary or exact numeric type to its canonical numeric id for compatibility checks.
pub(crate) fn numeric_type_id_for_compat(ty: &ResolvedType) -> Option<NumericTypeId> {
    match ty {
        ResolvedType::Int => Some(NumericTypeId::I64),
        ResolvedType::Float => Some(NumericTypeId::F64),
        ResolvedType::Numeric(id) => Some(*id),
        _ => None,
    }
}

/// Return whether one numeric id can widen to another without value loss under RFC 009 rules.
pub(crate) fn numeric_type_losslessly_widens_to(actual: NumericTypeId, expected: NumericTypeId) -> bool {
    if actual == expected {
        return true;
    }
    let actual_info = numerics::info_for(actual);
    let expected_info = numerics::info_for(expected);
    match (actual_info.family, expected_info.family) {
        (NumericFamily::SignedInteger, NumericFamily::SignedInteger) => {
            width_at_least(expected_info.bit_width, actual_info.bit_width)
        }
        (NumericFamily::UnsignedInteger, NumericFamily::UnsignedInteger) => {
            width_at_least(expected_info.bit_width, actual_info.bit_width)
        }
        (NumericFamily::UnsignedInteger, NumericFamily::SignedInteger) => {
            match (actual_info.bit_width, expected_info.bit_width) {
                (Some(actual_bits), Some(expected_bits)) => expected_bits > actual_bits,
                _ => false,
            }
        }
        (NumericFamily::BinaryFloat, NumericFamily::BinaryFloat) => {
            width_at_least(expected_info.bit_width, actual_info.bit_width)
        }
        _ => false,
    }
}

/// Compare optional bit widths for fixed-width widening decisions.
fn width_at_least(expected: Option<u16>, actual: Option<u16>) -> bool {
    match (expected, actual) {
        (Some(expected), Some(actual)) => expected >= actual,
        _ => false,
    }
}

/// Convenience function to type-check an AST
#[tracing::instrument(skip_all, fields(decl_count = program.declarations.len()))]
pub fn check(program: &Program) -> Result<(), Vec<CompileError>> {
    TypeChecker::new().check_program(program)
}
