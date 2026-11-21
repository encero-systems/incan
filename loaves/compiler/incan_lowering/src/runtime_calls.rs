//! Bind runtime operations to authenticated compiled frames or ordinary governed body instances.

use std::collections::BTreeMap;

use incan_mir_plan_native::{CalleeKind, ExternalFunction, Function, SourceSpan, source_span};
use incan_semantics_core::body_ir::{Body, BodyIrModule};
use incan_semantics_core::{CanonicalSymbolId, SymbolOrigin};

use crate::runtime_requirements::RuntimeRequirement;

/// One authenticated external signature and its selected compiled or governed implementation.
#[derive(Clone, Debug)]
pub struct RuntimeRedirect {
    pub selected: CalleeKind,
    pub replacement: CalleeKind,
    pub original: String,
    pub path: String,
}

/// Settled runtime signatures, indexed without repeatedly walking already-built function plans.
#[derive(Default)]
pub struct RuntimeRedirects {
    pub entries: BTreeMap<String, Vec<i64>>,
    pub bindings: Vec<RuntimeRedirect>,
    pub native: BTreeMap<String, bool>,
    pub deferred: BTreeMap<String, bool>,
    pub missing: Vec<RuntimeRequirement>,
}

pub fn runtime_redirects() -> RuntimeRedirects {
    RuntimeRedirects::default()
}

/// Collect only declarations demanded by the external calls and deferred coroutines already lowered for a unit.
pub fn selected_runtime_requirements(
    module: &BodyIrModule,
    externals: &[ExternalFunction],
    functions: &[Function],
) -> Result<Vec<RuntimeRequirement>, String> {
    let mut redirects = runtime_redirects();
    defer_missing_runtime(module, externals, functions, &mut redirects)?;
    Ok(redirects.missing)
}

/// Load generic-callable state layouts before any frame publishes combined enum indices.
///
/// Only a decorated binding's frames call `initialize_binding`, and runtime facades declare none, so a program whose
/// held modules retain no decorated binding never reads that state or its native layouts.
pub fn layout_runtime_requirements(modules: &[BodyIrModule], file: &str) -> Vec<RuntimeRequirement> {
    if !modules.iter().any(|module| !module.decorated_bindings.is_empty()) {
        return Vec::new();
    }
    let path = "incan_native_runtime::initialize_binding";
    for module in modules {
        for binding in &module.runtime_callable_bindings {
            if binding.path != path {
                continue;
            }
            if runtime_declaration_loaded(modules, &binding.canonical) {
                return Vec::new();
            }
            return vec![RuntimeRequirement {
                canonical: Some(binding.canonical.clone()),
                path: binding.path.clone(),
                span: source_span(file, 1, 1),
            }];
        }
        if sealed_runtime_path(module, path) {
            return vec![RuntimeRequirement {
                canonical: None,
                path: path.to_owned(),
                span: source_span(file, 1, 1),
            }];
        }
    }
    Vec::new()
}

/// Request this exact path once from its admitted package. A proven native path keeps its existing ABI.
pub fn sealed_runtime_path(module: &BodyIrModule, path: &str) -> bool {
    for (package, native_paths) in &module.runtime_binding_scopes {
        if path.starts_with(&format!("{package}::")) {
            return !native_paths.iter().any(|native| native == path);
        }
    }
    false
}

/// Whether the resolved project already retains a checked or compiled body for one runtime declaration.
fn runtime_declaration_loaded(modules: &[BodyIrModule], canonical: &CanonicalSymbolId) -> bool {
    modules.iter().any(|module| {
        module
            .bodies
            .iter()
            .any(|body| body.canonical.as_ref() == Some(canonical))
            || module
                .loaf_callables
                .iter()
                .any(|callable| callable.canonical == *canonical)
    })
}

/// Whether this exact signature already settled as native linkage; a signature with type arguments is reexamined.
fn settled_native(redirects: &RuntimeRedirects, external: &ExternalFunction) -> bool {
    external.type_arguments.is_empty() && redirects.native.contains_key(&external.path)
}

/// Demand only selected external calls and deferred coroutine callees, before mutating their frame inventories.
fn defer_missing_runtime(
    module: &BodyIrModule,
    externals: &[ExternalFunction],
    functions: &[Function],
    redirects: &mut RuntimeRedirects,
) -> Result<(), String> {
    for external in externals {
        if settled_native(redirects, external) {
            continue;
        }
        retain_runtime_requirement(module, &external.path, &external.span, redirects)?;
    }
    for function in functions {
        if let Some(callee) = &function.future_callee
            && let CalleeKind::Instantiated(path, _) = &callee.kind
        {
            retain_runtime_requirement(module, path, &callee.span, redirects)?;
        }
    }
    Ok(())
}

/// Bind a demand through the checked public facade; a missing facade never acquires an invented identity.
fn retain_runtime_requirement(
    module: &BodyIrModule,
    path: &str,
    span: &SourceSpan,
    redirects: &mut RuntimeRedirects,
) -> Result<(), String> {
    if redirects.deferred.contains_key(path) {
        return Ok(());
    }
    match runtime_canonical(module, path)? {
        None => {
            if sealed_runtime_path(module, path) {
                redirects.deferred.insert(path.to_owned(), true);
                redirects.missing.push(RuntimeRequirement {
                    canonical: None,
                    path: path.to_owned(),
                    span: span.clone(),
                });
            }
            Ok(())
        }
        Some(identity) => {
            if module
                .bodies
                .iter()
                .any(|body| body.canonical.as_ref() == Some(&identity))
                || module
                    .loaf_callables
                    .iter()
                    .any(|declaration| declaration.canonical == identity)
            {
                return Ok(());
            }
            redirects.deferred.insert(path.to_owned(), true);
            if redirects
                .missing
                .iter()
                .any(|request| request.canonical.as_ref() == Some(&identity))
            {
                return Ok(());
            }
            redirects.missing.push(RuntimeRequirement {
                canonical: Some(identity),
                path: path.to_owned(),
                span: span.clone(),
            });
            Ok(())
        }
    }
}

/// Resolve an admitted lowering facade from its checked public export map, retaining provider ownership.
pub fn runtime_canonical(module: &BodyIrModule, path: &str) -> Result<Option<CanonicalSymbolId>, String> {
    let owner = if path.starts_with("incan_native_runtime::") {
        "incan_native_runtime"
    } else if path.starts_with("incan_stdlib_primitives::") {
        "incan_stdlib_primitives"
    } else {
        return Ok(None);
    };
    let mut selected: Option<CanonicalSymbolId> = None;
    for binding in &module.runtime_callable_bindings {
        if binding.path != path {
            continue;
        }
        match &binding.canonical.origin {
            SymbolOrigin::Package { library, .. } if library == owner => {}
            _ => return Err(format!("invalid runtime facade owner for {path}")),
        }
        match &selected {
            Some(previous) => {
                if *previous != binding.canonical {
                    return Err(format!("ambiguous public runtime facade {path}"));
                }
            }
            None => selected = Some(binding.canonical.clone()),
        }
    }
    Ok(selected)
}

/// Select a unique published runtime declaration; another package's same-named body is never a candidate.
pub fn runtime_body(module: &BodyIrModule, path: &str) -> Result<Option<Body>, String> {
    let Some(canonical) = runtime_canonical(module, path)? else {
        return Ok(None);
    };
    let mut found: Option<Body> = None;
    for body in &module.bodies {
        if body.type_parameters.is_empty() || body.canonical.as_ref() != Some(&canonical) {
            continue;
        }
        if found.is_some() {
            return Err(format!("ambiguous governed runtime declaration {path}"));
        }
        found = Some(body.clone());
    }
    Ok(found)
}
