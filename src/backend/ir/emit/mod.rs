//! IR (Intermediate Representation) → Rust code emission.
//!
//! This module defines [`IrEmitter`] and wires together the focused submodules that implement IR → Rust emission.
//! The heavy lifting lives in those submodules; `mod.rs` is intentionally thin.
//!
//! ## Notes
//! - Emission produces a Rust syntax tree (`syn`) and formats it via `prettyplease`.
//! - Ownership/borrow/string conversions are centralized in `backend::ir::conversions` and should not be reimplemented
//!   ad-hoc in emission code.
//!
//! ## See also
//! - [`crate::backend::ir::conversions`]: conversion policy for emitted Rust
//! - [`program`]: program-level emission and formatting
//! - [`decls`]: item/declaration emission
//! - [`statements`]: statement emission
//! - [`expressions`]: expression emission
//! - [`types`]: type/pattern/operator helpers
//! - [`consts`]: RFC-008 const validation and const-friendly helpers

mod consts;
mod decls;
mod errors;
mod expressions;
mod program;
mod statements;
mod types;

pub use errors::EmitError;

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use super::FunctionRegistry;
use super::decl::VariantFields;
use super::types::IrType;
use incan_core::lang::rust_keywords;

/// Emit Rust source code from typed IR.
///
/// This is the main entry point for the IR → Rust backend stage. It is stateful because it:
/// - tracks which imports/features are required,
/// - records auxiliary typing metadata needed for emission (e.g. enum variant fields),
/// - caches resolvable const string values to emit `concat!(...)` in const contexts.
///
/// ## Notes
/// - The public API is `emit_program()` (implemented in `program.rs`).
/// - Most emission helpers are implemented on this type across submodules.
pub struct IrEmitter<'a> {
    /// Whether to add clippy allows (should be false for warning-free codegen)
    add_clippy_allows: bool,
    /// Whether to emit the Zen of Incan in main
    emit_zen_in_main: bool,
    /// Whether serde is needed (for Serialize/Deserialize derives)
    needs_serde: bool,
    /// Whether tokio is needed (for async runtime)
    needs_tokio: bool,
    /// Whether axum web framework is needed
    needs_axum: bool,
    /// Function registry for call-site type checking
    function_registry: &'a FunctionRegistry,
    /// Track struct derives for generating serde methods in impl blocks
    struct_derives: std::collections::HashMap<String, Vec<String>>,
    /// Current function's return type (for applying conversions in return statements)
    current_function_return_type: RefCell<Option<IrType>>,
    /// Functions imported from external Rust crates
    external_rust_functions: std::collections::HashSet<String>,
    /// Enum variant field typing lookup: (EnumName, VariantName) -> VariantFields
    enum_variant_fields: std::collections::HashMap<(String, String), VariantFields>,
    /// Struct field type lookup: (StructName, FieldName) -> IrType
    struct_field_types: std::collections::HashMap<(String, String), IrType>,
    /// Struct field name order (as declared): StructName -> [FieldName...]
    struct_field_names: std::collections::HashMap<String, Vec<String>>,
    /// Struct field alias lookup: (StructName, FieldName) -> alias
    struct_field_aliases: std::collections::HashMap<(String, String), Option<String>>,
    /// Struct field description lookup: (StructName, FieldName) -> description
    struct_field_descriptions: std::collections::HashMap<(String, String), Option<String>>,
    /// Struct field default expressions: (StructName, FieldName) -> default expr
    struct_field_defaults: std::collections::HashMap<(String, String), super::IrExpr>,
    /// Whether we're currently emitting a return expression (allows moves instead of clones)
    in_return_context: RefCell<bool>,
    /// Map of const string bindings to their literal values (for const folding of string adds)
    const_string_literals: std::collections::HashMap<String, String>,
    /// Collected routes for web emission
    routes: Vec<RouteSpec>,
    /// Map of type name -> module path segments for dependency modules.
    type_module_paths: HashMap<String, Vec<String>>,
    /// Type names that are declared in multiple modules (ambiguous).
    ambiguous_type_names: HashSet<String>,
    /// Known internal module roots for this compilation unit (e.g. {"db", "store"}).
    ///
    /// Used to disambiguate crate-internal module imports vs external crate imports when emitting `use` paths.
    internal_module_roots: HashSet<String>,
}

impl<'a> IrEmitter<'a> {
    pub fn new(function_registry: &'a FunctionRegistry) -> Self {
        Self {
            // Enable minimal allows for patterns that can't easily be made warning-free:
            // - dead_code: library modules export functions that may not be used by main
            // - unused_imports: user imports may not all be used
            // - unused_variables: pattern bindings like `_x` in destructuring
            add_clippy_allows: true,
            emit_zen_in_main: false,
            needs_serde: false,
            needs_tokio: false,
            needs_axum: false,
            function_registry,
            struct_derives: std::collections::HashMap::new(),
            current_function_return_type: RefCell::new(None),
            external_rust_functions: std::collections::HashSet::new(),
            enum_variant_fields: std::collections::HashMap::new(),
            struct_field_types: std::collections::HashMap::new(),
            struct_field_names: std::collections::HashMap::new(),
            struct_field_aliases: std::collections::HashMap::new(),
            struct_field_descriptions: std::collections::HashMap::new(),
            struct_field_defaults: std::collections::HashMap::new(),
            in_return_context: RefCell::new(false),
            const_string_literals: std::collections::HashMap::new(),
            routes: Vec::new(),
            type_module_paths: HashMap::new(),
            ambiguous_type_names: HashSet::new(),
            internal_module_roots: HashSet::new(),
        }
    }

    /// Set the internal module roots (top-level module names) for a multi-file compilation.
    pub fn set_internal_module_roots(&mut self, roots: HashSet<String>) {
        self.internal_module_roots = roots;
    }

    /// Check if a top-level name is a known internal module root.
    pub(crate) fn is_internal_module_root(&self, name: &str) -> bool {
        self.internal_module_roots.contains(name)
    }

    /// Check if a full module path is known internally.
    pub(crate) fn is_internal_module_path(&self, segments: &[String]) -> bool {
        if let Some(first) = segments.first()
            && self.is_internal_module_root(first)
        {
            return true;
        }
        if segments.is_empty() {
            return false;
        }
        let joined = segments.join("_");
        self.internal_module_roots.contains(&joined)
    }

    /// Set external rust functions.
    pub fn set_external_rust_functions(&mut self, funcs: std::collections::HashSet<String>) {
        self.external_rust_functions = funcs;
    }

    /// Set whether serde is needed.
    pub fn set_needs_serde(&mut self, needs: bool) {
        self.needs_serde = needs;
    }

    /// Set whether tokio is needed.
    pub fn set_needs_tokio(&mut self, needs: bool) {
        self.needs_tokio = needs;
    }

    /// Set whether axum is needed.
    pub fn set_needs_axum(&mut self, needs: bool) {
        self.needs_axum = needs;
    }

    /// Escape Rust keywords by adding `r#` prefix.
    ///
    /// Note: `self` and `Self` cannot be raw identifiers.
    fn escape_keyword(name: &str) -> String {
        if matches!(name, "self" | "Self") {
            return name.to_string();
        }
        // Strict + reserved keywords
        if rust_keywords::is_keyword(name) {
            return format!("r#{}", name);
        }
        name.to_string()
    }

    /// Disable clippy allows (for strict warning-free codegen).
    pub fn without_clippy_allows(mut self) -> Self {
        self.add_clippy_allows = false;
        self
    }

    /// Set whether to emit the Zen of Incan in main.
    pub fn set_emit_zen(&mut self, emit: bool) {
        self.emit_zen_in_main = emit;
    }

    /// Set collected routes for web emission.
    ///
    /// This should be called by codegen before emitting the program so the router wrapper and `App::run` wiring are
    /// generated when web routes exist.
    pub fn set_routes(&mut self, routes: Vec<RouteSpec>) {
        self.routes = routes;
    }

    /// Set type-to-module path mappings for qualifying route wrapper types.
    pub fn set_type_module_paths(&mut self, paths: HashMap<String, Vec<String>>, ambiguous: HashSet<String>) {
        self.type_module_paths = paths;
        self.ambiguous_type_names = ambiguous;
    }
}

/// Web route info collected during codegen for web emission.
///
/// This mirrors route metadata gathered from `@route(...)` decorators.
#[derive(Debug, Clone)]
pub struct RouteSpec {
    /// Handler function name.
    pub handler_name: String,
    /// Route path (Incan-style, e.g. `/api/{id}`).
    pub path: String,
    /// HTTP methods (e.g. GET, POST).
    pub methods: Vec<incan_core::lang::http::HttpMethodId>,
    /// Any unrecognized method spellings collected from decorators.
    pub unknown_methods: Vec<String>,
    /// Whether the handler is async.
    pub is_async: bool,
    /// Module path segments for nested multi-file projects.
    ///
    /// Example: `Some(vec!["api", "routes"])` means the handler lives in `crate::api::routes`.
    /// `None` means the handler is in the crate root (main module).
    ///
    /// This is carried structurally (segments) to avoid brittle string parsing.
    pub module_path_segments: Option<Vec<String>>,
}
