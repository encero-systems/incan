//! What `from std import …` may bind: the standard-library root exports its namespaces and nothing else.

use incan_lang::lang::{callables, stdlib, traits};

use crate::ast::{ImportItem, Span};
use crate::diagnostics::errors;
use crate::typechecker::TypeChecker;

/// Standard-library namespaces whose modules declare the traits the root's own source imports.
///
/// The root reaches its prelude traits through imports from these families only, so they are the modules worth asking
/// when a refusal names where a trait is declared.
const PRELUDE_TRAIT_NAMESPACES: &[&str] = &["derives", "traits"];

impl TypeChecker {
    /// Refuse an item of `from std import …` that names no standard-library namespace.
    ///
    /// The root's source imports the prelude traits (`Debug`, `Eq`, `Clone`, `From`, `Add`, `Error`, `Index`,
    /// `Callable1`, …), but that does not make them members of the root: each is declared in a `std.*` module (#1767).
    /// The refusal names the declaring module when the compiler knows it.
    pub(super) fn refuse_std_root_member_import(&mut self, item: &ImportItem, span: Span) {
        let declaring_module = self.prelude_trait_declaring_module(&item.name);
        self.errors.push(errors::std_root_member_not_exported(
            &item.name,
            declaring_module.as_deref(),
            span,
        ));
    }

    /// Return the dotted `std.*` module that declares a trait the standard-library root imports, if one does.
    ///
    /// The builtin trait and callable registries name the owning module directly. Any other prelude trait (`Add`,
    /// `Index`, `Copy`, …) is found in the manifests of the standard packages owning the prelude's trait families.
    fn prelude_trait_declaring_module(&self, name: &str) -> Option<String> {
        if let Some(module) = traits::from_str(name).and_then(traits::source_module) {
            return Some(module.to_string());
        }
        if callables::from_str(name).is_some() {
            return Some(callables::MODULE_PATH.join("."));
        }
        for namespace in PRELUDE_TRAIT_NAMESPACES {
            let root = [stdlib::STDLIB_ROOT.to_string(), namespace.to_string()];
            let Some(entry) = self.loaves.owner_of(&root) else {
                continue;
            };
            let manifest = &entry.artifact.manifest;
            for module_path in Self::manifest_public_module_paths(manifest) {
                if module_path.first().map(String::as_str) != Some(*namespace) {
                    continue;
                }
                if Self::public_module_member_names(manifest, &module_path)
                    .iter()
                    .any(|member| member == name)
                {
                    return Some(format!("{}.{}", stdlib::STDLIB_ROOT, module_path.join(".")));
                }
            }
        }
        None
    }
}
