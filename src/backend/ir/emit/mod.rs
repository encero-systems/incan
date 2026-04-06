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
    needs_serde: RefCell<bool>,
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
    /// Incan `rusttype` aliases that should use compiler-owned call conversion rules at the surface boundary.
    rusttype_alias_names: HashSet<String>,
    /// Whether we're currently emitting a return expression (allows moves instead of clones)
    in_return_context: RefCell<bool>,
    /// Map of const string bindings to their literal values (for const folding of string adds)
    const_string_literals: std::collections::HashMap<String, String>,
    /// Map of type name -> module path segments for dependency modules.
    type_module_paths: HashMap<String, Vec<String>>,
    /// Type names that are declared in multiple modules (ambiguous).
    ambiguous_type_names: HashSet<String>,
    /// Known internal module roots for this compilation unit (e.g. {"db", "store"}).
    ///
    /// Used to disambiguate crate-internal module imports vs external crate imports when emitting `use` paths.
    internal_module_roots: HashSet<String>,
    /// RFC 023: The `rust.module("path::to::module")` Rust backing path, if declared.
    ///
    /// When set, `@rust.extern` functions emit delegation calls to `<rust_module_path>::<fn_name>()` instead of
    /// compiling their Incan bodies.
    rust_module_path: Option<String>,
    /// Rust import path tracking: maps imported type names (incl. aliases) to their original module paths.
    ///
    /// Key: type name as seen in Incan code (e.g., "AxumResponse" for `import Response as AxumResponse`)
    /// Value: original module path (e.g., ["axum", "response"])
    ///
    /// Used by derive passthrough and newtype emission to locate the original Rust crate path for
    /// imported types.
    rust_import_paths: RefCell<std::collections::HashMap<String, Vec<String>>>,
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
            needs_serde: RefCell::new(false),
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
            rusttype_alias_names: HashSet::new(),
            in_return_context: RefCell::new(false),
            const_string_literals: std::collections::HashMap::new(),
            type_module_paths: HashMap::new(),
            ambiguous_type_names: HashSet::new(),
            internal_module_roots: HashSet::new(),
            rust_module_path: None,
            rust_import_paths: RefCell::new(std::collections::HashMap::new()),
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
    pub(crate) fn set_needs_serde(&mut self, needs: bool) {
        *self.needs_serde.borrow_mut() = needs;
    }

    /// Create a Rust identifier for emission, using raw identifiers for keywords.
    ///
    /// This is the only safe way to emit segments like `r#async`:
    /// - `proc_macro2::Ident::new_raw("async", ..)` emits `r#async`
    /// - string-based escaping + `format_ident!("{}", "r#async")` relies on macro parsing quirks and is easy to misuse
    ///   (and `syn::Ident::new("r#async", ..)` is invalid and will panic).
    fn rust_ident(name: &str) -> proc_macro2::Ident {
        let span = proc_macro2::Span::call_site();
        if matches!(name, "self" | "Self") {
            return proc_macro2::Ident::new(name, span);
        }
        if rust_keywords::is_keyword(name) {
            return proc_macro2::Ident::new_raw(name, span);
        }
        proc_macro2::Ident::new(name, span)
    }

    /// RFC 023: Set the `rust.module()` Rust backing path for this program.
    ///
    /// When set, `@rust.extern` functions delegate to `<path>::<fn_name>()`.
    pub fn set_rust_module_path(&mut self, path: Option<String>) {
        self.rust_module_path = path;
    }

    /// Disable clippy allows (for strict warning-free codegen).
    pub fn without_clippy_allows(mut self) -> Self {
        self.add_clippy_allows = false;
        self
    }

    /// Set whether to emit file-level clippy allows.
    ///
    /// Module files generated for the multi-file pipeline must NOT emit `#![allow(...)]` because the project generator
    /// prepends `pub mod` declarations before the emitted code. Inner attributes are only valid at the start of a
    /// file/module, so emitting them after `pub mod` lines causes a Rust compile error.
    pub fn set_add_clippy_allows(&mut self, enabled: bool) {
        self.add_clippy_allows = enabled;
    }

    /// Set whether to emit the Zen of Incan in main.
    pub fn set_emit_zen(&mut self, emit: bool) {
        self.emit_zen_in_main = emit;
    }

    /// Set type-to-module path mappings for qualifying route wrapper types.
    pub fn set_type_module_paths(&mut self, paths: HashMap<String, Vec<String>>, ambiguous: HashSet<String>) {
        self.type_module_paths = paths;
        self.ambiguous_type_names = ambiguous;
    }

    /// True if `ty` is a user-defined Incan enum in IR.
    ///
    /// Named enums lower to [`IrType::Struct`] (see `lower_resolved_type`); [`IrType::Enum`] is also treated as enum.
    /// Used by for-loop emission to iterate with `.iter().cloned()` so the loop variable is an owned `E`, matching the
    /// typechecker and `PartialEq` (#195).
    pub(super) fn type_is_user_enum(&self, ty: &IrType) -> bool {
        match ty {
            IrType::Enum(_) => true,
            IrType::Struct(name) => self.enum_variant_fields.keys().any(|(enum_name, _)| enum_name == name),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::IrEmitter;

    #[test]
    fn rust_ident_uses_raw_idents_for_keywords() {
        let ident = IrEmitter::rust_ident("async");
        let rendered = quote::quote! { #ident }.to_string();
        assert_eq!(rendered, "r#async");
    }
}
