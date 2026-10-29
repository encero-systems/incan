//! Parsed source retained once while module interfaces and bodies are checked against the same unit input.

use std::collections::{HashMap, hash_map::Entry};
use std::sync::Arc;

use crate::ast::{Declaration, FunctionDecl, Program, Span};

use super::TypeChecker;

type FunctionSources = HashMap<String, HashMap<Span, Option<Arc<FunctionDecl>>>>;

/// Immutable parsed declarations shared by the checkers of one collected compilation unit.
/// These are producer inputs, not accepted signatures or checked bodies. Full module paths, names and source spans
/// locate the original declaration; an ambiguous locator is omitted rather than choosing another declaration.
#[derive(Debug, Default)]
pub struct SourceInterfaceInputs {
    modules: HashMap<Vec<String>, FunctionSources>,
}

impl SourceInterfaceInputs {
    /// Retain each top-level function once from the immutable programs used by this checking pass.
    /// A fresh input index is required after parsing another source generation.
    pub fn from_programs<'a>(programs: impl IntoIterator<Item = (&'a [String], &'a Program)>) -> Self {
        let _span =
            tracing::trace_span!(target: "incan::invocation", "incan_frontend::source_interface_inputs").entered();
        let mut inputs = Self::default();
        let mut module_inputs = 0;
        let mut function_inputs_built = 0;
        for (path, program) in programs {
            module_inputs += 1;
            let functions = inputs.modules.entry(path.to_vec()).or_default();
            for declaration in &program.declarations {
                let Declaration::Function(function) = &declaration.node else {
                    continue;
                };
                match functions
                    .entry(function.name.clone())
                    .or_default()
                    .entry(declaration.span)
                {
                    Entry::Vacant(entry) => {
                        entry.insert(Some(Arc::new(function.clone())));
                        function_inputs_built += 1;
                    }
                    Entry::Occupied(mut entry) => {
                        entry.insert(None);
                    }
                }
            }
        }
        tracing::trace!(target: "incan::invocation", module_inputs, function_inputs_built, "retained source interface inputs");
        inputs
    }

    /// Borrow the exact parsed declaration without allocating a lookup key or cloning its body.
    fn function(&self, module: &[String], name: &str, span: Span) -> Option<Arc<FunctionDecl>> {
        self.modules.get(module)?.get(name)?.get(&span)?.clone()
    }
}

impl TypeChecker {
    /// Install producer-validated bodies for one exact program. Declaration collection and final publication still
    /// run; only admitted scalar bodies with complete source-call witnesses skip checking and feed Body IR directly.
    pub fn set_callable_body_reuse(&mut self, products: crate::declaration_interface::ValidatedCallableBodies) {
        self.callable_body_reuse = Some(products);
    }

    /// Use the immutable parse inputs of this collected unit. The index supplies source only; all semantic checking
    /// still runs, and declarations generated outside this index retain their ordinary owned fallback.
    pub fn set_source_interface_inputs(&mut self, inputs: Arc<SourceInterfaceInputs>) {
        self.source_interface_inputs = Some(inputs);
    }

    /// Retain source for this module's local analyses. Dependency signatures do not consume function bodies, and
    /// program reset discards their transient local-body map before any static-initializer traversal runs.
    pub(super) fn source_function_declaration(&self, function: &FunctionDecl, span: Span) -> Option<Arc<FunctionDecl>> {
        if self.symbols.collecting_dependency_interface() {
            return None;
        }
        let original = self
            .source_interface_inputs
            .as_ref()
            .and_then(|inputs| inputs.function(self.current_module_path.as_deref()?, &function.name, span));
        Some(original.unwrap_or_else(|| Arc::new(function.clone())))
    }
}
