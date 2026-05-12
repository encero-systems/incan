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
//! - `program`: program-level emission and formatting
//! - `decls`: item/declaration emission
//! - `statements`: statement emission
//! - `expressions`: expression emission
//! - `types`: type/pattern/operator helpers
//! - `consts`: RFC-008 const validation and const-friendly helpers

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

use proc_macro2::TokenStream;
use quote::quote;

use super::decl::{IrDeclKind, VariantFields, Visibility};
use super::types::IrType;
use super::{FunctionRegistry, FunctionSignature, IrProgram};
use incan_core::lang::rust_keywords;

/// Usage facts collected before Rust emission.
///
/// This analysis is intentionally about generated Rust lints, not source-language reachability diagnostics. It records
/// which declarations, imports, methods, and fields the emitted Rust must retain so emission can prune avoidable unused
/// Rust items and narrowly mark unavoidable semantic retention points.
#[derive(Clone, Default)]
pub(super) struct GeneratedUseAnalysis {
    /// Top-level declaration names that must be emitted.
    pub(super) reachable_items: HashSet<String>,
    /// Import binding names that are referenced by emitted code.
    pub(super) used_imports: HashSet<String>,
    /// Rust trait imports that are used implicitly by extension-method lookup.
    pub(super) used_extension_trait_imports: HashSet<String>,
    /// Struct/class fields that are read by emitted code.
    pub(super) read_fields: HashSet<(String, String)>,
    /// Methods that are called by emitted code.
    pub(super) used_methods: HashSet<(String, String)>,
    /// Function-like constructor names that are called by emitted code.
    pub(super) used_constructors: HashSet<String>,
    /// Type names whose Rust visibility prevents private helper methods from warning when retained.
    pub(super) public_types: HashSet<String>,
    /// Whether emitted method calls require the stdlib `Error` trait in Rust scope.
    pub(super) uses_stdlib_error_trait: bool,
    /// Source-owned callable object types used as non-Copy `Result.inspect` / `inspect_err` observers.
    pub(super) result_observer_callable_types: HashSet<String>,
    /// Top-level function values adapted to a borrowed function-pointer parameter.
    pub(super) borrowed_function_adapters: HashSet<(String, Vec<usize>)>,
}

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
    emit_strict_generated_lint_denies: bool,
    /// Whether public source items should be emitted even when this crate does not reference them.
    preserve_public_items: bool,
    /// Private items that generated code outside the emitted IR body will call.
    externally_reachable_items: HashSet<String>,
    /// Pre-emission usage facts used to avoid generated `dead_code` and `unused_imports` suppressions.
    generated_use_analysis: RefCell<GeneratedUseAnalysis>,
    /// Whether to emit the Zen of Incan in main
    emit_zen_in_main: bool,
    /// Whether serde is needed for emitted Rust derives or helpers.
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
    /// Enum variant alias lookup: (EnumName, AliasName) -> CanonicalVariantName
    enum_variant_aliases: std::collections::HashMap<(String, String), String>,
    /// Struct field type lookup: (StructName, FieldName) -> IrType
    struct_field_types: std::collections::HashMap<(String, String), IrType>,
    /// Struct field visibility lookup: (StructName, FieldName) -> Visibility
    struct_field_visibilities: std::collections::HashMap<(String, String), Visibility>,
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
    /// Method signature lookup for Incan-owned nominal receivers, including imported modules.
    method_signatures: HashMap<(String, String), FunctionSignature>,
    /// Impl-level generic parameter order for method signatures.
    method_signature_type_params: HashMap<(String, String), Vec<String>>,
    /// Whether we're currently emitting a return expression (allows moves instead of clones)
    in_return_context: RefCell<bool>,
    /// Map of const string bindings to their literal values (for const folding of string adds)
    const_string_literals: std::collections::HashMap<String, String>,
    /// Map of type name -> module path segments for dependency modules.
    type_module_paths: HashMap<String, Vec<String>>,
    /// Type names that are declared in multiple modules (ambiguous).
    ambiguous_type_names: HashSet<String>,
    /// Imported enum type names discovered from dependency modules.
    ///
    /// Imported enums usually lower to `IrType::Struct(name)` in consumer modules, so for-loop emission needs this
    /// side-channel to recognize that `list[name]` elements should be iterated as owned enum values.
    dependency_enum_types: HashSet<String>,
    /// Imported stdlib error type names whose trait methods need Rust trait imports at call sites.
    external_error_trait_types: HashSet<String>,
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
    /// Newtype -> selected checked constructor method.
    ///
    /// Backend-generated newtype construction, such as lifted iterator sums, uses this to preserve normal checked
    /// construction behavior instead of directly invoking the tuple-struct constructor.
    newtype_checked_ctor: HashMap<String, String>,
    /// Whether the currently emitted module contains any local `static` declarations.
    module_has_local_statics: RefCell<bool>,
    /// Whether expression emission is currently inside a static initializer.
    ///
    /// Used to avoid recursively forcing the module-wide static init helper while generating static initializer code.
    in_static_initializer: RefCell<bool>,
    /// Whether canonical calls to internal modules should be emitted with explicit `crate::...` paths.
    ///
    /// Normal imported calls use ordinary local bindings and imports. Default argument expressions are different: they
    /// can be expanded at a caller outside the defining module, so imported helper calls inside those defaults need a
    /// durable crate-qualified path.
    qualify_internal_canonical_paths: RefCell<bool>,
    /// Whether anonymous ordinary union wrapper references should be emitted as crate-root paths.
    ///
    /// Multi-file source modules share generated ordinary union wrappers through the crate root so same-shaped unions
    /// remain one Rust nominal type across module boundaries.
    qualify_union_types_from_crate: bool,
    /// Extra anonymous union shapes that should be emitted in this module in addition to locally referenced shapes.
    generated_union_types: HashMap<String, IrType>,
    /// Whether this module should emit generated ordinary union wrapper definitions.
    emit_generated_union_definitions: bool,
    /// Stack of statement-slice analyses describing which local `StaticBinding` names need mutable Rust bindings.
    ///
    /// An Incan alias like `let live = ITEMS` is not source-level `mut`, but if later emitted Rust uses
    /// `live.with_mut(...)` the local wrapper still must be declared `mut`. This stack is pushed per emitted
    /// statement slice so `emit_stmt` can make that decision without reintroducing blanket `mut` noise.
    storage_binding_mut_names: RefCell<Vec<HashSet<String>>>,
    /// Source-owned callable object types used as non-Copy `Result.inspect` / `inspect_err` observers.
    result_observer_callable_types: RefCell<HashSet<String>>,
    /// Callable object types whose borrowed observer helper has already been emitted.
    emitted_result_observer_callable_helpers: RefCell<HashSet<String>>,
    /// Top-level function values adapted to a borrowed function-pointer parameter.
    borrowed_function_adapters: RefCell<HashSet<(String, Vec<usize>)>>,
}

impl<'a> IrEmitter<'a> {
    /// Create an emitter using the function registry that drives call-site default argument filling and type-aware
    /// argument conversion.
    pub fn new(function_registry: &'a FunctionRegistry) -> Self {
        Self {
            emit_strict_generated_lint_denies: false,
            preserve_public_items: true,
            externally_reachable_items: HashSet::new(),
            generated_use_analysis: RefCell::new(GeneratedUseAnalysis::default()),
            emit_zen_in_main: false,
            needs_serde: RefCell::new(false),
            function_registry,
            struct_derives: std::collections::HashMap::new(),
            current_function_return_type: RefCell::new(None),
            external_rust_functions: std::collections::HashSet::new(),
            enum_variant_fields: std::collections::HashMap::new(),
            enum_variant_aliases: std::collections::HashMap::new(),
            struct_field_types: std::collections::HashMap::new(),
            struct_field_visibilities: std::collections::HashMap::new(),
            struct_field_names: std::collections::HashMap::new(),
            struct_field_aliases: std::collections::HashMap::new(),
            struct_field_descriptions: std::collections::HashMap::new(),
            struct_field_defaults: std::collections::HashMap::new(),
            rusttype_alias_names: HashSet::new(),
            method_signatures: HashMap::new(),
            method_signature_type_params: HashMap::new(),
            in_return_context: RefCell::new(false),
            const_string_literals: std::collections::HashMap::new(),
            type_module_paths: HashMap::new(),
            ambiguous_type_names: HashSet::new(),
            dependency_enum_types: HashSet::new(),
            external_error_trait_types: HashSet::new(),
            internal_module_roots: HashSet::new(),
            rust_module_path: None,
            rust_import_paths: RefCell::new(std::collections::HashMap::new()),
            newtype_checked_ctor: HashMap::new(),
            module_has_local_statics: RefCell::new(false),
            in_static_initializer: RefCell::new(false),
            qualify_internal_canonical_paths: RefCell::new(false),
            qualify_union_types_from_crate: false,
            generated_union_types: HashMap::new(),
            emit_generated_union_definitions: true,
            storage_binding_mut_names: RefCell::new(Vec::new()),
            result_observer_callable_types: RefCell::new(HashSet::new()),
            emitted_result_observer_callable_helpers: RefCell::new(HashSet::new()),
            borrowed_function_adapters: RefCell::new(HashSet::new()),
        }
    }

    pub(super) fn emit_module_static_init_call(&self) -> TokenStream {
        if *self.module_has_local_statics.borrow() {
            let init_fn = Self::rust_ident("__incan_init_module_statics");
            quote! { #init_fn(); }
        } else {
            quote! {}
        }
    }

    /// Return the private helper method name used to call callable-object observers through a borrowed payload.
    pub(super) fn result_observer_borrowed_method_name() -> &'static str {
        "__incan_result_observer_borrow___call__"
    }

    /// Return the private helper name used to adapt a named function to a borrowed function-pointer parameter.
    pub(super) fn borrowed_function_adapter_name(name: &str, indices: &[usize]) -> String {
        let suffix = indices.iter().map(usize::to_string).collect::<Vec<_>>().join("_");
        format!("__incan_borrow_adapter_{name}_{suffix}")
    }

    /// Store pre-emission facts describing which observer callbacks need borrowed helper emission.
    pub(super) fn set_result_observer_callable_types(&self, callable_types: HashSet<String>) {
        *self.result_observer_callable_types.borrow_mut() = callable_types;
    }

    /// Store pre-emission facts for named function values that need borrowed function-pointer adapters.
    pub(super) fn set_borrowed_function_adapters(&self, adapters: HashSet<(String, Vec<usize>)>) {
        *self.borrowed_function_adapters.borrow_mut() = adapters;
    }

    /// Return whether a source-owned callable object type needs a borrowed observer helper.
    pub(super) fn needs_result_observer_callable_helper(&self, type_name: &str) -> bool {
        self.result_observer_callable_types.borrow().contains(type_name)
    }

    /// Mark a callable-object borrowed observer helper as emitted, returning false if it was already emitted.
    pub(super) fn claim_result_observer_callable_helper(&self, type_name: &str) -> bool {
        self.emitted_result_observer_callable_helpers
            .borrow_mut()
            .insert(type_name.to_string())
    }

    /// Return whether `name` needs a borrowed adapter for the selected parameter indices.
    pub(super) fn needs_borrowed_function_adapter(&self, name: &str, indices: &[usize]) -> bool {
        self.borrowed_function_adapters
            .borrow()
            .contains(&(name.to_string(), indices.to_vec()))
    }

    /// Set the internal module roots (top-level module names) for a multi-file compilation.
    pub fn set_internal_module_roots(&mut self, roots: HashSet<String>) {
        self.internal_module_roots = roots;
    }

    /// Configure whether anonymous union wrappers are addressed through the crate root.
    pub fn set_qualify_union_types_from_crate(&mut self, enabled: bool) {
        self.qualify_union_types_from_crate = enabled;
    }

    /// Add generated union wrapper definitions that should be emitted by this module.
    pub fn set_generated_union_types(&mut self, types: HashMap<String, IrType>) {
        self.generated_union_types = types;
    }

    /// Configure whether this module emits generated union wrapper definitions.
    pub fn set_emit_generated_union_definitions(&mut self, enabled: bool) {
        self.emit_generated_union_definitions = enabled;
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
        if matches!(name, "self" | "Self" | "crate" | "super") {
            return proc_macro2::Ident::new(name, span);
        }
        if rust_keywords::is_keyword(name) {
            return proc_macro2::Ident::new_raw(name, span);
        }
        proc_macro2::Ident::new(name, span)
    }

    /// Create a Rust identifier for compiler-emitted `static` items.
    ///
    /// Incan static names follow source-language naming, but generated Rust `static` items should use
    /// `SCREAMING_SNAKE_CASE` to avoid `non_upper_case_globals` warnings.
    fn rust_static_ident(name: &str) -> proc_macro2::Ident {
        let mut rendered = String::with_capacity(name.len().max(1));
        for ch in name.chars() {
            if ch.is_ascii_alphanumeric() {
                rendered.push(ch.to_ascii_uppercase());
            } else {
                rendered.push('_');
            }
        }
        if rendered.is_empty() {
            rendered.push('_');
        }
        proc_macro2::Ident::new(&rendered, proc_macro2::Span::call_site())
    }

    /// RFC 023: Set the `rust.module()` Rust backing path for this program.
    ///
    /// When set, `@rust.extern` functions delegate to `<path>::<fn_name>()`.
    pub fn set_rust_module_path(&mut self, path: Option<String>) {
        self.rust_module_path = path;
    }

    /// Deprecated compatibility shim: generated unused/dead lint allows are no longer emitted.
    pub fn without_clippy_allows(self) -> Self {
        self
    }

    /// Deprecated compatibility shim: generated unused/dead lint allows are no longer emitted.
    pub fn set_add_clippy_allows(&mut self, enabled: bool) {
        let _ = enabled;
    }

    /// Enable strict generated Rust lint validation.
    pub fn set_strict_generated_lints(&mut self, enabled: bool) {
        self.emit_strict_generated_lint_denies = enabled;
    }

    /// Set whether public source items are treated as externally reachable during emission.
    pub fn set_preserve_public_items(&mut self, enabled: bool) {
        self.preserve_public_items = enabled;
    }

    /// Set private items that are called by compiler-generated code injected after IR emission.
    pub fn set_externally_reachable_items(&mut self, names: HashSet<String>) {
        self.externally_reachable_items = names;
    }

    /// Replace pre-emission usage facts for the program currently being emitted.
    pub(super) fn set_generated_use_analysis(&self, analysis: GeneratedUseAnalysis) {
        *self.generated_use_analysis.borrow_mut() = analysis;
    }

    /// True when a top-level declaration with `name` should be emitted.
    pub(super) fn should_emit_decl_name(&self, name: &str, visibility: &Visibility) -> bool {
        (self.preserve_public_items && !matches!(visibility, Visibility::Private))
            || self.generated_use_analysis.borrow().reachable_items.contains(name)
    }

    /// True when an import binding should be emitted because generated code references it.
    pub(super) fn should_emit_import_binding(&self, name: &str) -> bool {
        self.generated_use_analysis.borrow().used_imports.contains(name)
    }

    /// True when a Rust trait import should be emitted for extension-method lookup.
    pub(super) fn should_emit_extension_trait_import(&self, name: &str) -> bool {
        self.generated_use_analysis
            .borrow()
            .used_extension_trait_imports
            .contains(name)
    }

    /// True when a method should be emitted for a preserved public surface or an observed generated-use call.
    pub(super) fn should_emit_method(&self, target_type: &str, method_name: &str, visibility: &Visibility) -> bool {
        let analysis = self.generated_use_analysis.borrow();
        analysis.public_types.contains(target_type)
            || (!self.preserve_public_items
                && !matches!(visibility, Visibility::Private)
                && analysis.reachable_items.contains(target_type))
            || analysis
                .used_methods
                .contains(&(target_type.to_string(), method_name.to_string()))
    }

    /// True when the generated free constructor function for a struct should be retained.
    pub(super) fn should_emit_struct_constructor(&self, struct_name: &str) -> bool {
        let analysis = self.generated_use_analysis.borrow();
        analysis.used_constructors.contains(struct_name)
    }

    /// True when a generated private field needs a narrow `dead_code` expectation because Rust cannot see an
    /// Incan-level semantic use for it in the emitted program.
    pub(super) fn should_expect_private_field_dead_code(
        &self,
        struct_name: &str,
        field_name: &str,
        visibility: &Visibility,
    ) -> bool {
        matches!(visibility, Visibility::Private)
            && !self
                .generated_use_analysis
                .borrow()
                .read_fields
                .contains(&(struct_name.to_string(), field_name.to_string()))
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

    /// Set imported enum type names discovered during codegen setup.
    pub fn set_dependency_enum_types(&mut self, enum_type_names: HashSet<String>) {
        self.dependency_enum_types = enum_type_names;
    }

    /// Set imported stdlib error types whose trait methods may be called from this module.
    pub fn set_external_error_trait_types(&mut self, type_names: HashSet<String>) {
        self.external_error_trait_types = type_names;
    }

    /// Seed nominal declaration metadata from another lowered module.
    ///
    /// Multi-file emission creates one Rust module at a time, but constructor/default emission still needs the
    /// declared field list and default expressions for imported Incan types used by the current module.
    pub(crate) fn seed_nominal_metadata_from_program(&mut self, program: &IrProgram) {
        for decl in &program.declarations {
            match &decl.kind {
                IrDeclKind::Struct(s) => {
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
                IrDeclKind::Enum(e) => {
                    for v in &e.variants {
                        self.enum_variant_fields
                            .insert((e.name.clone(), v.name.clone()), v.fields.clone());
                    }
                    for alias in &e.variant_aliases {
                        self.enum_variant_aliases
                            .insert((e.name.clone(), alias.name.clone()), alias.target.clone());
                    }
                }
                IrDeclKind::TypeAlias {
                    name,
                    is_rusttype: true,
                    ..
                } => {
                    self.rusttype_alias_names.insert(name.clone());
                }
                IrDeclKind::Impl(i) => {
                    for method in &i.methods {
                        let params = method.params.iter().filter(|param| !param.is_self).cloned().collect();
                        let key = (i.target_type.clone(), method.name.clone());
                        self.method_signatures.insert(
                            key.clone(),
                            FunctionSignature {
                                params,
                                return_type: method.return_type.clone(),
                            },
                        );
                        self.method_signature_type_params
                            .insert(key, i.type_params.iter().map(|param| param.name.clone()).collect());
                    }
                }
                _ => {}
            }
        }
    }

    /// Return an Incan-owned method signature for a receiver type when typechecker call-site metadata is unavailable.
    pub(super) fn method_signature_for_receiver(
        &self,
        receiver_ty: &IrType,
        method_name: &str,
    ) -> Option<&FunctionSignature> {
        match receiver_ty {
            IrType::Struct(name) | IrType::NamedGeneric(name, _) => self
                .method_signatures
                .get(&(name.clone(), method_name.to_string()))
                .or_else(|| {
                    name.rsplit("::").next().and_then(|short_name| {
                        self.method_signatures
                            .get(&(short_name.to_string(), method_name.to_string()))
                    })
                }),
            IrType::Ref(inner) | IrType::RefMut(inner) => self.method_signature_for_receiver(inner, method_name),
            _ => None,
        }
    }

    /// Return a method signature specialized through a concrete generic receiver target.
    ///
    /// Associated constructors such as `OrderedDict.from_items(...)` can be checked from the assignment target
    /// (`OrderedDict[String, Int]`) even when the callee expression itself still carries generic impl parameters
    /// (`K`, `V`). Specializing the raw impl signature lets aggregate literal emission materialize owned element
    /// shapes before Rust typechecking sees the generated call.
    pub(super) fn specialized_method_signature_for_receiver(
        &self,
        receiver_ty: &IrType,
        method_name: &str,
    ) -> Option<FunctionSignature> {
        let IrType::NamedGeneric(type_name, args) = receiver_ty else {
            return None;
        };
        let (signature_key, signature) = self
            .method_signatures
            .get_key_value(&(type_name.clone(), method_name.to_string()))
            .or_else(|| {
                type_name.rsplit("::").next().and_then(|short_name| {
                    self.method_signatures
                        .get_key_value(&(short_name.to_string(), method_name.to_string()))
                })
            })?;
        let type_params = self.method_signature_type_params.get(signature_key)?;
        if type_params.len() != args.len() {
            return None;
        }
        let subst: HashMap<&str, &IrType> = type_params.iter().map(String::as_str).zip(args.iter()).collect();
        Some(FunctionSignature {
            params: signature
                .params
                .iter()
                .map(|param| {
                    let mut param = param.clone();
                    param.ty = Self::substitute_signature_type(&param.ty, &subst);
                    param
                })
                .collect(),
            return_type: Self::substitute_signature_type(&signature.return_type, &subst),
        })
    }

    /// Substitute generic placeholders inside a method signature type.
    fn substitute_signature_type(ty: &IrType, subst: &HashMap<&str, &IrType>) -> IrType {
        match ty {
            IrType::Generic(name) => subst.get(name.as_str()).copied().cloned().unwrap_or_else(|| ty.clone()),
            IrType::Struct(name) if Self::is_signature_placeholder_name(name) => {
                subst.get(name.as_str()).copied().cloned().unwrap_or_else(|| ty.clone())
            }
            IrType::List(inner) => IrType::List(Box::new(Self::substitute_signature_type(inner, subst))),
            IrType::Dict(key, value) => IrType::Dict(
                Box::new(Self::substitute_signature_type(key, subst)),
                Box::new(Self::substitute_signature_type(value, subst)),
            ),
            IrType::Set(inner) => IrType::Set(Box::new(Self::substitute_signature_type(inner, subst))),
            IrType::Tuple(items) => IrType::Tuple(
                items
                    .iter()
                    .map(|item| Self::substitute_signature_type(item, subst))
                    .collect(),
            ),
            IrType::Option(inner) => IrType::Option(Box::new(Self::substitute_signature_type(inner, subst))),
            IrType::Result(ok, err) => IrType::Result(
                Box::new(Self::substitute_signature_type(ok, subst)),
                Box::new(Self::substitute_signature_type(err, subst)),
            ),
            IrType::NamedGeneric(name, args) => IrType::NamedGeneric(
                name.clone(),
                args.iter()
                    .map(|arg| Self::substitute_signature_type(arg, subst))
                    .collect(),
            ),
            IrType::Function { params, ret } => IrType::Function {
                params: params
                    .iter()
                    .map(|param| Self::substitute_signature_type(param, subst))
                    .collect(),
                ret: Box::new(Self::substitute_signature_type(ret, subst)),
            },
            IrType::Ref(inner) => IrType::Ref(Box::new(Self::substitute_signature_type(inner, subst))),
            IrType::RefMut(inner) => IrType::RefMut(Box::new(Self::substitute_signature_type(inner, subst))),
            _ => ty.clone(),
        }
    }

    /// Best-effort specialization for call-site signatures that still expose receiver generics.
    pub(super) fn specialize_signature_by_receiver_args(
        signature: &FunctionSignature,
        receiver_ty: &IrType,
    ) -> Option<FunctionSignature> {
        let IrType::NamedGeneric(_, args) = receiver_ty else {
            return None;
        };
        let mut generic_names = Vec::new();
        for param in &signature.params {
            Self::collect_signature_generics(&param.ty, &mut generic_names);
        }
        if generic_names.is_empty() || generic_names.len() > args.len() {
            return None;
        }
        let subst: HashMap<&str, &IrType> = generic_names.iter().map(String::as_str).zip(args.iter()).collect();
        Some(FunctionSignature {
            params: signature
                .params
                .iter()
                .map(|param| {
                    let mut param = param.clone();
                    param.ty = Self::substitute_signature_type(&param.ty, &subst);
                    param
                })
                .collect(),
            return_type: Self::substitute_signature_type(&signature.return_type, &subst),
        })
    }

    /// Collect generic placeholder names from a signature type in first-use order.
    fn collect_signature_generics(ty: &IrType, out: &mut Vec<String>) {
        match ty {
            IrType::Generic(name) if !out.contains(name) => out.push(name.clone()),
            IrType::Struct(name) if Self::is_signature_placeholder_name(name) && !out.contains(name) => {
                out.push(name.clone());
            }
            IrType::List(inner)
            | IrType::Set(inner)
            | IrType::Option(inner)
            | IrType::Ref(inner)
            | IrType::RefMut(inner) => {
                Self::collect_signature_generics(inner, out);
            }
            IrType::Dict(key, value) | IrType::Result(key, value) => {
                Self::collect_signature_generics(key, out);
                Self::collect_signature_generics(value, out);
            }
            IrType::Tuple(items) | IrType::NamedGeneric(_, items) => {
                for item in items {
                    Self::collect_signature_generics(item, out);
                }
            }
            IrType::Function { params, ret } => {
                for param in params {
                    Self::collect_signature_generics(param, out);
                }
                Self::collect_signature_generics(ret, out);
            }
            _ => {}
        }
    }

    /// Return whether a struct-shaped name is really a lowered generic placeholder.
    fn is_signature_placeholder_name(name: &str) -> bool {
        !name.is_empty() && name.len() <= 2 && name.chars().all(|ch| ch.is_ascii_uppercase())
    }

    /// True if `ty` is a user-defined Incan enum in IR, including imported enums.
    ///
    /// Named enums lower to [`IrType::Struct`] (see `lower_resolved_type`); [`IrType::Enum`] is also treated as enum.
    /// Imported enums are tracked separately because consumer modules only carry the short nominal type name after
    /// typechecking/lowering. Used by for-loop emission to iterate with `.iter().cloned()` so the loop variable is an
    /// owned `E`, matching the typechecker and `PartialEq` for both local and cross-module enum loops (#195, #372).
    pub(super) fn type_is_user_enum(&self, ty: &IrType) -> bool {
        match ty {
            IrType::Enum(_) => true,
            IrType::Struct(name) | IrType::NamedGeneric(name, _) => {
                self.enum_variant_fields.keys().any(|(enum_name, _)| enum_name == name)
                    || self.dependency_enum_types.contains(name)
            }
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

    #[test]
    fn rust_static_ident_uses_uppercase_global_style() {
        let ident = IrEmitter::rust_static_ident("_active_sessions");
        let rendered = quote::quote! { #ident }.to_string();
        assert_eq!(rendered, "_ACTIVE_SESSIONS");
    }
}
