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
#[cfg(feature = "rust-metadata")]
use std::path::PathBuf;
use std::sync::Arc;

use crate::frontend::ast::*;
use crate::frontend::diagnostics::{CompileError, ErrorKind, errors};
use crate::frontend::library_manifest_index::LibraryManifestIndex;
use crate::frontend::module::{ExportedSymbol, exported_symbols};
use crate::frontend::surface_semantics::SurfaceContext;
use crate::frontend::symbols::*;
#[cfg(feature = "rust-metadata")]
use crate::rust_metadata::RustMetadataCache;
use helpers::{collection_type_id, stringlike_type_id};
use incan_core::interop::{CoercionPolicy, RustFunctionSig, RustItemKind, RustItemMetadata, RustParam};
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
    /// RFC 042: Direct supertraits per trait name, copied from [`TraitInfo::supertraits`] for IR lowering.
    ///
    /// Lowering does not retain the typechecker symbol table; this snapshot supplies resolved supertrait type
    /// arguments after a successful check.
    pub trait_direct_supertraits: HashMap<String, Vec<(String, Vec<ResolvedType>)>>,
    /// RFC 042: Trait type parameter names keyed by trait name for lowering-time generic substitution.
    ///
    /// Includes locally-declared and imported traits so backend lowering can handle cross-module trait hierarchies
    /// without relying on local AST declarations.
    pub trait_type_params: HashMap<String, Vec<String>>,
    /// `rusttype` Incan name → canonical Rust path string (`substrait::proto::type::Binary`), when the checker
    /// resolved the underlying type to [`ResolvedType::RustPath`]. Used by lowering so `m::T` spellings emit full
    /// paths without re-running import resolution.
    pub rusttype_canonical_rust_paths: HashMap<String, String>,
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
    /// Rust-boundary coercion decisions keyed by argument expression span.
    pub rust_arg_coercions: HashMap<(usize, usize), RustArgCoercionInfo>,
    /// Rust-boundary coercion decisions for method return values, keyed by the call expression span.
    ///
    /// Populated when metadata shows a `rusttype` method's actual Rust return type requires coercion to the
    /// Incan-declared type (e.g. `&str` → `String` for a method declared `-> str`).
    pub rust_return_coercions: HashMap<(usize, usize), RustArgCoercionInfo>,
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

/// Coercion category selected by the typechecker for a Rust-boundary call argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RustArgCoercionKind {
    /// Builtin scalar matrix coercion (`float -> f32`, `str -> &str`, ...).
    Builtin(CoercionPolicy),
    /// Rusttype alias can flow to its backing Rust type without an explicit adapter call.
    RustTypeUnwrap,
    /// Rusttype alias uses a declared `interop:` adapter edge.
    RustTypeInterop,
}

/// Lowering metadata for one Rust-boundary call argument.
#[derive(Debug, Clone, PartialEq)]
pub struct RustArgCoercionInfo {
    /// Normalized Rust parameter type display from metadata (e.g. `f32`, `&str`).
    pub rust_target_type: String,
    /// Resolved target type for lowering IR typing.
    pub target_type: ResolvedType,
    /// Coercion strategy to apply.
    pub kind: RustArgCoercionKind,
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

    pub fn rust_arg_coercion(&self, span: Span) -> Option<&RustArgCoercionInfo> {
        self.rust_arg_coercions.get(&(span.start, span.end))
    }

    /// Return the recorded return coercion for the call expression at `span`, if any.
    pub fn rust_return_coercion(&self, span: Span) -> Option<&RustArgCoercionInfo> {
        self.rust_return_coercions.get(&(span.start, span.end))
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
    /// Accumulated non-fatal diagnostics (warnings/lints).
    ///
    /// These are produced during typechecking but do not cause `check_*` to fail.
    pub(crate) warnings: Vec<CompileError>,
    /// Track which bindings are mutable for mutation checks.
    pub(crate) mutable_bindings: HashSet<String>,
    /// Current function's error type for `?` operator compatibility.
    pub(crate) current_return_error_type: Option<ResolvedType>,
    /// Whether the body currently being checked belongs to an `async def` or async method.
    pub(crate) in_async_body: bool,
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
    /// Consumer-side dependency library manifests (`pub::`) keyed by library name.
    pub(crate) library_manifests: Arc<LibraryManifestIndex>,
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
    /// Feature-gated cache for Rust semantic metadata extraction (RFC 041).
    #[cfg(feature = "rust-metadata")]
    pub(crate) rust_metadata_cache: RustMetadataCache,
    /// Manifest/workspace root used for rust-analyzer metadata extraction.
    #[cfg(feature = "rust-metadata")]
    pub(crate) rust_metadata_manifest_dir: Option<PathBuf>,
}

impl TypeChecker {
    pub fn new() -> Self {
        Self {
            symbols: SymbolTable::new(),
            errors: Vec::new(),
            warnings: Vec::new(),
            mutable_bindings: HashSet::new(),
            current_return_error_type: None,
            in_async_body: false,
            current_trait_requires: None,
            current_trait_name: None,
            current_trait_missing_requires_emitted: None,
            const_decls: HashMap::new(),
            const_eval_state: HashMap::new(),
            const_eval_cache: HashMap::new(),
            type_info: TypeCheckInfo::default(),
            dependency_exports: HashMap::new(),
            library_manifests: Arc::new(LibraryManifestIndex::default()),
            current_module_path: None,
            declared_crate_names: None,
            stdlib_cache: stdlib_loader::StdlibAstCache::new(),
            testing_marker_import_bindings: HashSet::new(),
            import_aliases: HashMap::new(),
            surface_context: SurfaceContext::default(),
            supertrait_closure: HashMap::new(),
            pending_trait_supertraits: Vec::new(),
            #[cfg(feature = "rust-metadata")]
            rust_metadata_cache: RustMetadataCache::new(),
            #[cfg(feature = "rust-metadata")]
            rust_metadata_manifest_dir: std::env::current_dir().ok(),
        }
    }

    #[cfg(feature = "rust-metadata")]
    pub fn set_rust_metadata_manifest_dir(&mut self, dir: PathBuf) {
        self.rust_metadata_manifest_dir = Some(dir);
    }

    #[cfg(feature = "rust-metadata")]
    pub(crate) fn rust_item_metadata_for_path(&self, canonical_path: &str) -> Option<RustItemMetadata> {
        let dir = self.rust_metadata_manifest_dir.as_ref()?;
        match self.rust_metadata_cache.get_or_extract(dir, canonical_path, &|_| ()) {
            Ok(meta) => Some((*meta).clone()),
            Err(_) => None,
        }
    }

    #[cfg(not(feature = "rust-metadata"))]
    pub(crate) fn rust_item_metadata_for_path(&self, _canonical_path: &str) -> Option<RustItemMetadata> {
        None
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

    /// Build a conservative function type from Rust metadata.
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
            .map(|p| self.resolved_type_from_rust_display(p.type_display.as_str()))
            .collect();
        let ret = self.resolved_type_from_rust_display(sig.return_type.as_str());
        ResolvedType::Function(params, Box::new(ret))
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
    /// RFC 041: intentionally best-effort — only common std-ish spellings and simple `Option`/`Result` wrappers are
    /// recognized. Nested generics, lifetimes, and crate paths otherwise become [`ResolvedType::RustPath`] (or
    /// [`ResolvedType::Unknown`] when empty); lowering relies on rustc for fidelity.
    ///
    /// ## `Result<T, E>` parsing
    ///
    /// `Result<…>` is split on the **first** top-level comma only. Nested generics that contain commas (for example
    /// `Result<Vec<(i32, i32)>, String>`) are therefore parsed incorrectly and may degrade to [`ResolvedType::Unknown`]
    /// for one or both type arguments. Prefer precise typing from Incan surfaces over relying on this heuristic.
    pub(crate) fn resolved_type_from_rust_display(&self, rust_ty: &str) -> ResolvedType {
        let trimmed = rust_ty.trim();
        let no_lifetimes = trimmed
            .replace("'static ", "")
            .replace("'_", "")
            .replace("&mut ", "&")
            .replace(' ', "");
        let normalized = no_lifetimes.trim_start_matches("::").to_string();
        match normalized.as_str() {
            "bool" => ResolvedType::Bool,
            "f32" | "f64" => ResolvedType::Float,
            "i8" | "i16" | "i32" | "i64" | "i128" | "isize" | "u8" | "u16" | "u32" | "u64" | "u128" | "usize" => {
                ResolvedType::Int
            }
            "str" | "&str" | "String" | "std::string::String" | "alloc::string::String" => ResolvedType::Str,
            "Vec<u8>" | "std::vec::Vec<u8>" | "alloc::vec::Vec<u8>" | "&[u8]" => ResolvedType::Bytes,
            "()" => ResolvedType::Unit,
            _ if (normalized.starts_with("Option<")
                || normalized.starts_with("std::option::Option<")
                || normalized.starts_with("core::option::Option<"))
                && normalized.ends_with('>') =>
            {
                let inner = normalized
                    .trim_start_matches("Option<")
                    .trim_start_matches("std::option::Option<")
                    .trim_start_matches("core::option::Option<")
                    .trim_end_matches('>');
                ResolvedType::Generic("Option".to_string(), vec![self.resolved_type_from_rust_display(inner)])
            }
            _ if (normalized.starts_with("Result<")
                || normalized.starts_with("std::result::Result<")
                || normalized.starts_with("core::result::Result<"))
                && normalized.ends_with('>') =>
            {
                let inner = normalized
                    .trim_start_matches("Result<")
                    .trim_start_matches("std::result::Result<")
                    .trim_start_matches("core::result::Result<")
                    .trim_end_matches('>');
                let mut parts = inner.splitn(2, ',');
                let ok_ty = parts
                    .next()
                    .map(|p| self.resolved_type_from_rust_display(p))
                    .unwrap_or(ResolvedType::Unknown);
                // Malformed `Result<…>` display from metadata: keep going with `Unknown` error arm.
                let err_ty = parts
                    .next()
                    .map(|p| self.resolved_type_from_rust_display(p))
                    .unwrap_or(ResolvedType::Unknown);
                ResolvedType::Generic("Result".to_string(), vec![ok_ty, err_ty])
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

    /// Infer the concrete instantiation of `trait_name` for `type_name`, if the type adopts that trait.
    ///
    /// RFC 042 currently uses the same implicit positional mapping as trait-annotation compatibility: a concrete
    /// adopter's leading type arguments instantiate the adopted trait's type parameters, and transitive supertrait
    /// arguments are substituted through the recorded closure.
    fn instantiated_trait_args_for_type(
        &self,
        type_name: &str,
        concrete_type_args: &[ResolvedType],
        trait_name: &str,
    ) -> Option<Vec<ResolvedType>> {
        let adopted = match self.lookup_type_info(type_name) {
            Some(TypeInfo::Model(model)) => &model.traits,
            Some(TypeInfo::Class(class)) => &class.traits,
            _ => return None,
        };

        for adopted_trait in adopted {
            let Some(adopted_info) = self.lookup_trait_info(adopted_trait) else {
                continue;
            };
            let direct_args: Vec<ResolvedType> = concrete_type_args
                .iter()
                .take(adopted_info.type_params.len())
                .cloned()
                .collect();
            if direct_args.len() != adopted_info.type_params.len() {
                continue;
            }

            if adopted_trait == trait_name {
                return Some(direct_args);
            }

            let Some(closure) = self.supertrait_closure.get(adopted_trait) else {
                continue;
            };
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
        self.supertrait_closure
            .get(subtrait_name)
            .is_some_and(|closure| closure.iter().any(|(n, _)| n == supertrait_name))
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
        let sub_info = self.lookup_trait_info(subtrait_name)?;
        if subtrait_args.len() != sub_info.type_params.len() {
            return None;
        }
        let subst = crate::frontend::resolved_type_subst::type_param_subst_map(&sub_info.type_params, subtrait_args);
        if subtrait_name == supertrait_name {
            return Some(subtrait_args.to_vec());
        }
        let closure = self.supertrait_closure.get(subtrait_name)?;
        for (name, args) in closure {
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
        let Some(info) = self.lookup_type_info(type_name) else {
            return false;
        };
        let (adopted, derives) = match info {
            TypeInfo::Model(m) => (m.traits.as_slice(), Some(m.derives.as_slice())),
            TypeInfo::Class(c) => (c.traits.as_slice(), Some(c.derives.as_slice())),
            TypeInfo::Enum(e) => (&[][..], Some(e.derives.as_slice())),
            _ => return false,
        };
        for t in adopted {
            if t == trait_name {
                return true;
            }
            if let Some(closure) = self.supertrait_closure.get(t)
                && closure.iter().any(|(n, _)| n == trait_name)
            {
                return true;
            }
        }
        if let Some(derives) = derives
            && derives.iter().any(|d| d == trait_name)
            && self.lookup_trait_info(trait_name).is_some()
        {
            return true;
        }
        false
    }

    /// Explicit `with Trait` names plus `@derive` entries that name a registered trait, for instance method lookup.
    pub(crate) fn trait_names_for_type_methods(&self, adopted: &[String], derives: &[String]) -> Vec<String> {
        let mut out = adopted.to_vec();
        for d in derives {
            if self.lookup_trait_info(d).is_some() && !out.iter().any(|t| t == d) {
                out.push(d.clone());
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

    /// RFC 042: Snapshot trait metadata into [`TypeCheckInfo`] for backend lowering.
    ///
    /// This records all visible trait symbols (local and imported), not just traits declared in the current module,
    /// so lowering can resolve supertrait graphs and generic trait arity across module boundaries.
    fn record_trait_metadata_for_lowering(&mut self) {
        self.type_info.trait_direct_supertraits.clear();
        self.type_info.trait_type_params.clear();
        for sym in self.symbols.all_symbols() {
            if let SymbolKind::Trait(info) = &sym.kind {
                self.type_info
                    .trait_direct_supertraits
                    .insert(sym.name.clone(), info.supertraits.clone());
                self.type_info
                    .trait_type_params
                    .insert(sym.name.clone(), info.type_params.clone());
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
        self.warnings.clear();
        self.errors.clear();
        self.testing_marker_import_bindings.clear();
        self.surface_context = SurfaceContext::from_program(program);
        self.import_aliases = self.surface_context.import_aliases().clone();
        self.supertrait_closure.clear();

        // `check_with_imports` / `import_module` can queue supertrait bounds while collecting dependency ASTs.
        // Resolve those queued bounds into trait symbols before we collect and resolve the current program.
        if !self.pending_trait_supertraits.is_empty() {
            self.resolve_pending_trait_supertraits();
        }

        // First pass: collect type declarations
        for decl in &program.declarations {
            self.collect_declaration(decl);
        }

        self.resolve_pending_trait_supertraits();
        self.finalize_supertrait_graph();
        self.merge_supertrait_requires_into_traits();

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
        let has_generic_params = |name: &str| -> bool {
            match self.lookup_type_info(name) {
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

        match (actual, expected) {
            (ResolvedType::Unknown, _) | (_, ResolvedType::Unknown) => true,
            (ResolvedType::TypeVar(_), _) | (_, ResolvedType::TypeVar(_)) => true,

            // ---- Context: RFC 042 — `expected` is a trait reference (`Named` or nullary trait on RHS) ----
            (ResolvedType::Named(type_name), ResolvedType::Named(trait_name))
                if self.lookup_trait_info(trait_name).is_some() =>
            {
                if self.lookup_trait_info(type_name).is_some() {
                    self.trait_is_supertrait_of(type_name, trait_name)
                } else {
                    self.type_implements_trait(type_name, trait_name)
                }
            }
            (ResolvedType::Generic(type_name, actual_args), ResolvedType::Named(trait_name))
                if self.lookup_trait_info(trait_name).is_some() =>
            {
                if self.lookup_trait_info(type_name).is_some() {
                    let Some(super_info) = self.lookup_trait_info(trait_name) else {
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
            ) if self.lookup_trait_info(subtrait_name).is_some()
                && self.lookup_trait_info(supertrait_name).is_some() =>
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

            // RFC 042: `Concrete[T]` assignable to generic trait annotation `Trait[T]` (and similar).
            (ResolvedType::Generic(type_name, actual_args), ResolvedType::Generic(trait_name, expected_args))
                if self.lookup_trait_info(trait_name).is_some()
                    && self.lookup_trait_info(type_name).is_none()
                    && self.lookup_type_info(type_name).is_some() =>
            {
                let Some(instantiated_args) = self.instantiated_trait_args_for_type(type_name, actual_args, trait_name)
                else {
                    return false;
                };
                let concrete_arity_ok = match self.lookup_type_info(type_name) {
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
                if self.lookup_trait_info(trait_name).is_some() =>
            {
                if self.lookup_trait_info(type_name).is_some() {
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

fn is_public_decl(decl: &Spanned<Declaration>) -> bool {
    match &decl.node {
        Declaration::Const(c) => matches!(c.visibility, Visibility::Public),
        Declaration::Model(m) => matches!(m.visibility, Visibility::Public),
        Declaration::Class(c) => matches!(c.visibility, Visibility::Public),
        Declaration::Enum(e) => matches!(e.visibility, Visibility::Public),
        Declaration::TypeAlias(a) => matches!(a.visibility, Visibility::Public),
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
