//! What `from std import …` may bind: the standard-library root exports its submodules and nothing else.

use incan_lang::lang::{callables, stdlib, traits};

use crate::ast::{ImportItem, ImportPath, Span};
use crate::diagnostics::errors;
use crate::module::canonicalize_source_module_segments;
use crate::typechecker::TypeChecker;

/// Standard-library namespaces whose modules declare the traits the root's own source imports.
///
/// The root reaches its prelude traits through imports from these families only, so they are the modules worth asking
/// when a refusal names where a trait is declared.
const PRELUDE_TRAIT_NAMESPACES: &[&str] = &["derives", "traits"];

impl TypeChecker {
    /// Refuse an item of `from std import …` that is not a standard-library submodule, returning whether it did.
    ///
    /// The root (`std`, and `std.prelude`, the file that implements it) binds its submodules, so `from std import toml`
    /// imports the `std.toml` module. The root's source also imports the prelude traits (`Debug`, `Eq`, `Clone`,
    /// `From`, `Add`, `Error`, `Index`, `Callable1`, …), but that does not make them members of the root: each is
    /// declared in a `std.*` module, and nothing the generated program can reach binds them at the root, so accepting
    /// the import left a program that checked and could not build (#1767). The refusal names the declaring module when
    /// the compiler knows it.
    ///
    /// Called for an item the submodule binding declined, so a real submodule never reaches it. A name the stdlib
    /// registry knows as a module is left to the existing handling, so a registered module that the active provider
    /// plan does not serve keeps its own diagnostic.
    pub(super) fn refuse_std_root_member_import(&mut self, module: &ImportPath, item: &ImportItem, span: Span) -> bool {
        if module.parent_levels != 0 || module.is_absolute {
            return false;
        }
        if canonicalize_source_module_segments(&module.segments) != [stdlib::STDLIB_ROOT] {
            return false;
        }
        if stdlib::is_known_stdlib_module(&[stdlib::STDLIB_ROOT.to_string(), item.name.clone()]) {
            return false;
        }
        let declaring_module = self.prelude_trait_declaring_module(&item.name);
        self.errors.push(errors::std_root_member_not_exported(
            &item.name,
            declaring_module.as_deref(),
            span,
        ));
        true
    }

    /// Return the dotted `std.*` module that declares a trait the standard-library root imports, if one does.
    ///
    /// The builtin trait and callable registries name the owning module directly. Any other prelude trait (`Add`,
    /// `Index`, `Copy`, …) is found by asking each module of the prelude's trait families, first the active SDK
    /// provider's published members and then the stdlib source metadata.
    fn prelude_trait_declaring_module(&mut self, name: &str) -> Option<String> {
        if let Some(module) = traits::from_str(name).and_then(traits::source_module) {
            return Some(module.to_string());
        }
        if callables::from_str(name).is_some() {
            return Some(callables::MODULE_PATH.join("."));
        }
        for namespace in PRELUDE_TRAIT_NAMESPACES {
            let Some(registered) = stdlib::find_namespace(namespace) else {
                continue;
            };
            for submodule in registered.submodules {
                let module_path = [stdlib::STDLIB_ROOT, *namespace, *submodule]
                    .iter()
                    .map(|segment| segment.to_string())
                    .collect::<Vec<_>>();
                let dotted = module_path.join(".");
                let provider_declares = self
                    .dependency_direct_member_symbols
                    .get(&dotted)
                    .is_some_and(|members| members.contains_key(name));
                if provider_declares
                    || self.stdlib_cache.lookup_trait(&module_path, name).is_some()
                    || self.stdlib_cache.lookup_type(&module_path, name).is_some()
                {
                    return Some(dotted);
                }
            }
        }
        None
    }
}
