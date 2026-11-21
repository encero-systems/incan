//! Bind runtime operations to authenticated compiled frames or ordinary governed body instances.

use std::collections::BTreeMap;

use incan_mir_plan_native::{
    CalleeKind, EnumDeclaration, ExternalFunction, Function, SourceSpan, TerminatorKind, source_span,
};
use incan_semantics_core::body_ir::{Body, BodyIrModule};
use incan_semantics_core::{CanonicalSymbolId, IncanType, SymbolOrigin};

use crate::classes::function_name;
use crate::frame_instances::merge_externals;
use crate::function_frames::frame_parameter_type;
use crate::layout_types::native_source_type;
use crate::native_type_identity::ConstructorIndex;
use crate::pending::untranslated;
use crate::runtime_frames::runtime_frame_adapter;
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

#[cfg(test)]
mod tests {
    use incan_mir_plan_native::{ExternalFunction, PlanType, source_span};
    use incan_semantics_core::body_ir::{BodyIrModule, RuntimeCallableBinding};
    use incan_semantics_core::{CanonicalSymbolId, HirSourceSpan, SemanticSourceTargetKind};

    use super::{layout_runtime_requirements, selected_runtime_requirements};

    /// The entry's runtime facade table names `initialize_binding`, but nothing in the program calls it.
    #[test]
    fn undecorated_program_reads_no_binding_state_layouts() {
        let mut entry = BodyIrModule::declarations_only("main".into());
        entry.runtime_callable_bindings.push(RuntimeCallableBinding {
            path: "incan_native_runtime::initialize_binding".into(),
            canonical: CanonicalSymbolId::module_declaration(
                vec!["generic_decorators".into()],
                "initialize_binding",
                SemanticSourceTargetKind::Function,
                HirSourceSpan::new(0, 0),
            ),
        });
        let requirements = layout_runtime_requirements(&[entry], "main.incn");
        assert!(
            requirements.is_empty(),
            "an undecorated program loaded the generic-decorator binding layouts"
        );
    }

    /// Unreached paths do no work. A selected native-only path is queried once and remains external on retry.
    #[test]
    fn planning_requests_the_one_selected_sealed_runtime_path() -> Result<(), String> {
        let mut entry = BodyIrModule::declarations_only("main".into());
        let used = "incan_native_runtime::used";
        let external = "incan_native_runtime::unpublished";
        let span = source_span("main.incn", 1, 1);
        let calls: Vec<ExternalFunction> = [used, used, external, "unrelated::external"]
            .iter()
            .map(|path| ExternalFunction {
                path: (*path).into(),
                type_arguments: Vec::new(),
                parameters: Vec::new(),
                return_type: PlanType::Unit,
                span: span.clone(),
            })
            .collect();
        entry.runtime_binding_scopes = vec![("incan_native_runtime".into(), Vec::new())];
        let first = selected_runtime_requirements(&entry, &calls, &[])?;
        let paths: Vec<&str> = first.iter().map(|request| request.path.as_str()).collect();
        assert_eq!(paths, vec![used, external]);
        entry.runtime_binding_scopes = vec![("incan_native_runtime".into(), vec![external.into()])];
        let retry = selected_runtime_requirements(&entry, &calls, &[])?;
        assert_eq!(retry.len(), 1);
        assert!(retry[0].path == used && retry[0].canonical.is_none());
        Ok(())
    }
}

/// The unit's retained inventories that runtime retention reads and extends.
pub struct RuntimeInventories<'a> {
    pub functions: &'a mut Vec<Function>,
    pub externals: &'a mut Vec<ExternalFunction>,
    pub intrinsics: &'a mut Vec<EnumDeclaration>,
    pub unions: &'a [IncanType],
}

/// Retain runtime selections: 1. Retain coroutines. 2. Close frames and register implementations. 3. Retain native
/// calls.
///
/// A Rust runtime contributes no bodies and keeps its external ABI. A published Incan body has one implementation: its
/// closed frame joins the ordinary queue, and every call with that exact runtime signature names that frame. A call
/// whose runtime declaration this attempt has not loaded stays unbound: it is neither redirected nor settled as native
/// linkage, and its requirement waits in `redirects.missing` until the queue drains. A generic runtime function whose
/// runtime publishes no governed body is a native generic item, and its call keeps the instantiated external ABI.
pub fn retain_runtime_instances(
    constructors: &ConstructorIndex,
    module: &BodyIrModule,
    inventories: RuntimeInventories<'_>,
    redirects: &mut RuntimeRedirects,
) -> Result<(), String> {
    let RuntimeInventories {
        functions,
        externals,
        intrinsics,
        unions,
    } = inventories;
    // Defer missing declarations first, so no step below binds a call to a declaration this attempt lacks.
    defer_missing_runtime(module, externals, functions, redirects)?;
    if !redirects.missing.is_empty() {
        return Ok(());
    }
    // 1. Retain coroutine instances before ordinary call frames.
    for function in functions.iter() {
        if let Some(callee) = &function.future_callee
            && let CalleeKind::Instantiated(path, _) = &callee.kind
            && path.starts_with("incan_native_runtime::")
            && !redirects.deferred.contains_key(path)
        {
            return Err(untranslated("runtime_calls::retain_runtime_future_instances"));
        }
    }
    // 2. Register each implementation; each exact signature settles once.
    let mut keep: Vec<bool> = Vec::with_capacity(externals.len());
    let mut dropped = false;
    let mut linked: Vec<ExternalFunction> = Vec::new();
    for position in 0..externals.len() {
        let external = externals[position].clone();
        if settled_native(redirects, &external) || redirects.deferred.contains_key(&external.path) {
            keep.push(true);
            continue;
        }
        if runtime_redirect(redirects, &runtime_selection(&external)).is_some() {
            keep.push(false);
            dropped = true;
            continue;
        }
        if runtime_body(module, &external.path)?.is_some() {
            return Err(untranslated(
                "runtime_calls::retain_runtime_instances governed generic body",
            ));
        }
        let frame = RuntimeFrameCall {
            constructors,
            module,
            intrinsics,
            unions,
        };
        if retain_runtime_frame_call(&frame, functions, &mut linked, &external, redirects)? {
            keep.push(false);
            dropped = true;
            continue;
        }
        settle_native(redirects, &external);
        keep.push(true);
    }
    // 3. Keep only the external ABI calls that still need native linkage, then the compiled callees frames link.
    if dropped {
        let retained: Vec<ExternalFunction> = externals
            .iter()
            .zip(&keep)
            .filter(|(_, kept)| **kept)
            .map(|(external, _)| external.clone())
            .collect();
        *externals = retained;
    }
    merge_externals(externals, linked)
}

/// The callee kind a call with this exact external signature names, as its redirect binding selects it.
fn runtime_selection(external: &ExternalFunction) -> CalleeKind {
    if external.type_arguments.is_empty() {
        return CalleeKind::External(external.path.clone());
    }
    CalleeKind::Instantiated(external.path.clone(), external.type_arguments.clone())
}

/// Record a signature that needs native linkage, so later queue requests keep it without examining it.
fn settle_native(redirects: &mut RuntimeRedirects, external: &ExternalFunction) {
    if external.type_arguments.is_empty() {
        redirects.native.insert(external.path.clone(), true);
    }
}

/// The checked tables a runtime frame call reads its declared storage from.
struct RuntimeFrameCall<'a> {
    constructors: &'a ConstructorIndex,
    module: &'a BodyIrModule,
    intrinsics: &'a [EnumDeclaration],
    unions: &'a [IncanType],
}

/// Forward a runtime call: 1. Read its public frame. 2. Adapt its ABI. 3. Retain its canonical callee.
fn retain_runtime_frame_call(
    tables: &RuntimeFrameCall<'_>,
    functions: &mut Vec<Function>,
    externals: &mut Vec<ExternalFunction>,
    external: &ExternalFunction,
    redirects: &mut RuntimeRedirects,
) -> Result<bool, String> {
    // 1. Read the governed frame selected by the public facade's canonical identity.
    let module = tables.module;
    let Some((frame, callee)) = runtime_callable_frame(module, &external.path)? else {
        return Ok(false);
    };
    let nominals = &module.nominal_declarations;
    let enums = &module.enum_declarations;
    let file = external.span.file.to_string();
    let mut parameters = Vec::with_capacity(frame.params.len());
    for parameter in &frame.params {
        parameters.push(frame_parameter_type(
            tables.constructors,
            nominals,
            enums,
            &frame,
            parameter,
            tables.intrinsics,
            &file,
        )?);
    }
    let result = native_source_type(
        tables.constructors,
        nominals,
        enums,
        tables.unions,
        &frame.return_type,
        tables.intrinsics,
        "",
    )?;
    let symbol = function_name(module, &frame);
    // 2. Adapt declared storage; an exact ABI calls the compiled item directly.
    let mut target = callee.clone();
    if let Some(adapter) = runtime_frame_adapter(module, external, &parameters, &result, &symbol, &callee)? {
        target = CalleeKind::Planned(adapter.name.clone());
        retain_runtime_adapter(functions, adapter)?;
    }
    // 3. Retain compiled callees for metadata verification; a producer's own frame already compiles here.
    retain_runtime_redirect(redirects, external, target, &symbol)?;
    if let CalleeKind::External(path) = callee {
        let compiled = ExternalFunction {
            path,
            type_arguments: Vec::new(),
            parameters,
            return_type: result,
            span: external.span.clone(),
        };
        merge_externals(externals, vec![compiled])?;
    }
    Ok(true)
}

/// Reuse one canonical forwarder across call-site spans, refusing conflicting parameter or result storage.
fn retain_runtime_adapter(functions: &mut Vec<Function>, adapter: Function) -> Result<(), String> {
    for function in functions.iter() {
        if function.name != adapter.name {
            continue;
        }
        if function.parameters.len() != adapter.parameters.len() || function.return_type != adapter.return_type {
            return Err(format!(
                "invalid governed runtime adapter signature for {}",
                adapter.name
            ));
        }
        for (retained, added) in function.parameters.iter().zip(&adapter.parameters) {
            if retained.ty != added.ty {
                return Err(format!(
                    "invalid governed runtime adapter parameter for {}",
                    adapter.name
                ));
            }
        }
        return Ok(());
    }
    functions.push(adapter);
    Ok(())
}

/// Select an authenticated own or compiled frame; private same-named dependencies never grant its ABI.
fn runtime_callable_frame(module: &BodyIrModule, path: &str) -> Result<Option<(Body, CalleeKind)>, String> {
    let Some(identity) = runtime_canonical(module, path)? else {
        return Ok(None);
    };
    if let Some(frame) = own_runtime_frame(module, &identity)? {
        let symbol = function_name(module, &frame);
        return Ok(Some((frame, CalleeKind::Planned(symbol))));
    }
    let mut found: Option<(Body, String)> = None;
    for declaration in &module.loaf_callables {
        if !declaration.type_parameters.is_empty() || declaration.canonical != identity {
            continue;
        }
        let (Some(native), Some(candidate)) = (declaration.rust_path(), declaration.declaration_frame()) else {
            continue;
        };
        match &found {
            Some((previous, selected)) => {
                if previous != candidate || *selected != native {
                    return Err(format!("ambiguous compiled runtime frame for {path}"));
                }
            }
            None => found = Some((candidate.clone(), native)),
        }
    }
    Ok(found.map(|(frame, native)| (frame, CalleeKind::External(native))))
}

/// A runtime producer uses its own checked callable body, without importing an artifact of itself.
fn own_runtime_frame(module: &BodyIrModule, identity: &CanonicalSymbolId) -> Result<Option<Body>, String> {
    let mut found: Option<&Body> = None;
    for body in &module.bodies {
        if !body.type_parameters.is_empty() || body.canonical.as_ref() != Some(identity) {
            continue;
        }
        if found.is_some() {
            return Err("ambiguous runtime producer callable frame".into());
        }
        found = Some(body);
    }
    Ok(found.cloned())
}

/// Settle each exact external signature once; contradictory implementation selections remain refused.
fn retain_runtime_redirect(
    redirects: &mut RuntimeRedirects,
    external: &ExternalFunction,
    replacement: CalleeKind,
    original: &str,
) -> Result<(), String> {
    let selected = runtime_selection(external);
    if let Some(index) = runtime_redirect(redirects, &selected) {
        let retained = &redirects.bindings[index];
        if retained.replacement != replacement || retained.original != original {
            return Err(format!(
                "conflicting governed runtime implementation for {}",
                external.path
            ));
        }
        return Ok(());
    }
    let index = redirects.bindings.len() as i64;
    redirects.entries.entry(external.path.clone()).or_default().push(index);
    redirects.bindings.push(RuntimeRedirect {
        selected,
        replacement,
        original: original.to_string(),
        path: external.path.clone(),
    });
    Ok(())
}

/// Rewrite every redirected runtime call, preserving operands and edges and refusing reentry.
pub fn redirect_runtime_calls(functions: &mut [Function], redirects: &RuntimeRedirects) -> Result<(), String> {
    for function in functions.iter_mut() {
        for block in function.blocks.iter_mut() {
            let TerminatorKind::Call(callee, ..) = &block.terminator.kind else {
                continue;
            };
            let Some(index) = runtime_redirect(redirects, &callee.kind) else {
                continue;
            };
            let binding = &redirects.bindings[index];
            if function.name == binding.original {
                return Err(format!(
                    "unsupported governed runtime body reenters {}; its primitive must use Rust interop",
                    binding.path
                ));
            }
            if let TerminatorKind::Call(callee, ..) = &mut block.terminator.kind {
                callee.kind = binding.replacement.clone();
            }
        }
    }
    Ok(())
}

/// Use the path index; exact native type arguments still select the binding.
fn runtime_redirect(redirects: &RuntimeRedirects, selected: &CalleeKind) -> Option<usize> {
    let path = match selected {
        CalleeKind::External(name) | CalleeKind::Instantiated(name, _) => name,
        _ => return None,
    };
    let indices = redirects.entries.get(path)?;
    indices
        .iter()
        .map(|index| *index as usize)
        .find(|position| redirects.bindings[*position].selected == *selected)
}
