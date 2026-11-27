//! Checker construction, the default checker, and module, package and dependency configuration setters.

use super::*;

impl TypeChecker {
    /// Create a checker with builtin symbols and fresh checking state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Install producer-validated bodies for one exact program. Declaration collection and final publication still
    /// run; only admitted scalar bodies with complete source-call witnesses skip checking and feed Body IR directly.
    pub fn set_callable_body_reuse(&mut self, products: crate::declaration_interface::ValidatedCallableBodies) {
        self.callable_body_reuse = Some(products);
    }

    /// Install graph-admitted checker values for identity-based selection at the free-function body boundary.
    pub fn set_graph_body_records(&mut self, records: crate::checker_records::GraphBodyRecords) {
        self.graph_body_records = records;
    }

    /// Set the declared Rust crate names from `loaf.toml [rust-dependencies]` for checked direct imports and derives.
    pub fn set_declared_crate_names(&mut self, names: HashSet<String>) {
        self.declared_crate_names = Some(names);
    }

    /// Set the compiled Loaves this program imports from: its dependencies and the standard packages (DD-0007).
    pub fn set_loaves(&mut self, loaves: Arc<LoafIndex>) {
        self.loaves = loaves;
    }

    /// Name dependency bindings whose manifests are unavailable because unattended preparation stopped before
    /// untrusted compile-time code. Imports from these bindings remain opaque and report one stable informational
    /// diagnostic instead of being misclassified as unknown packages.
    pub fn set_not_prepared_loaves(&mut self, names: HashSet<String>) {
        self.not_prepared_loaves = names;
    }

    /// Mark the program being checked as the standard package `policy` names.
    pub fn set_own_standard_package(&mut self, policy: &'static StandardPackageNamespacePolicy) {
        self.own_standard_package = Some(policy);
    }

    /// Record the module path owning this checker's declarations.
    ///
    /// Also forwarded to the symbol table, whose RFC 120 identity minting uses it as the [`SymbolOrigin`] of every
    /// locally-declared symbol; `None` resets to the anonymous-module origin.
    pub fn set_current_module_path(&mut self, path: Option<Vec<String>>) {
        self.symbols.set_module_path(path.clone().unwrap_or_default());
        self.current_module_path = path;
    }

    /// Set the defining package for declarations checked for a compiled-library artifact.
    pub fn set_current_package_identity(&mut self, package_identity: Option<String>) {
        self.symbols.set_package_identity(package_identity);
    }

    /// Name the project library this unit links, which makes it a unit of that library's own package (DD-0007).
    pub fn set_project_library(&mut self, library: Option<String>) {
        self.project_library = library;
    }

    /// Install the already admitted owning package fact without changing symbol ownership or asking a registry.
    pub fn set_record_package_identity(
        &mut self,
        identity: Option<incan_semantics_core::record_identity::RecordPackageIdentity>,
    ) {
        self.record_package_identity = identity;
    }

    /// Return accumulated non-fatal diagnostics (warnings/lints) from the last check.
    pub fn warnings(&self) -> &[CompileError] {
        &self.warnings
    }

    /// Return accumulated type information for reuse by later stages (lowering/codegen).
    pub fn type_info(&self) -> &TypeCheckInfo {
        &self.type_info
    }

    /// Hand the accumulated type information to a later stage once checking is over. The opt-in Body IR baseline also
    /// retains indexed immutable owner references; checking and settlement are never repeated.
    pub fn into_type_info(mut self) -> TypeCheckInfo {
        if std::env::var_os("INCAN_BODY_IR_FREE_BASELINE").is_some() {
            self.type_info.body_ir_baseline_inputs = Some(self.body_ir_baseline_inputs());
        }
        self.type_info
    }

    /// Return the checked source path for one active import-derived binding.
    pub fn import_binding_path(&self, local_name: &str) -> Option<&[String]> {
        self.symbols.import_binding_path(local_name)
    }
}
