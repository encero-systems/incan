//! Shared CLI-session source-to-Body-IR glue. All Body-IR-to-plan decisions belong to the Incan lowering Loaf.

use std::path::{Path, PathBuf};

use incan_driver::{modules::collect_modules_detailed_with_session, session::CompilationSession};
use incan_frontend::ast::{DecoratorArg, Expr, Literal};
use incan_mir_lowering::caller::incan::{library_plan, program_plan, program_refusals};
use incan_semantics_core::body_ir::BodyIrModule;
use incan_semantics_core::{CanonicalSymbolId, SemanticSourceTargetKind, SymbolNamespace, SymbolOrigin};

/// Environment variable naming the file where the census receives every refusal of one compilation, one per line.
///
/// Unset, compilation reports only its first refusal, as it always has. Set, the driver also lowers every body on its
/// own and writes each body's first refusal there before compiling as usual, so one refused body cannot hide another.
const REFUSAL_REPORT: &str = "INCAN_DIRECT_REFUSAL_REPORT";

/// An optional side channel for semantic CLI failures, distinct from unsupported lowering or malformed plans.
const SEMANTIC_REPORT: &str = "INCAN_DIRECT_SEMANTIC_REPORT";

/// A checked program failure that occurs before an executable can exist.
#[derive(Debug, thiserror::Error)]
enum SourceFailure {
    /// Feature projection and checked lowering completed, but the selected entry module declares no main.
    #[error("selected program has no main entrypoint")]
    MissingEntrypoint,
}

/// Bind a genuine missing-entrypoint outcome to the exact input, keeping both program streams free of report bytes.
fn report_missing_entrypoint(source: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let Some(path) = std::env::var_os(SEMANTIC_REPORT) else {
        return Ok(());
    };
    let source = std::fs::read_to_string(source)?;
    let report = serde_json::json!({
        "schema_version": 1,
        "kind": "missing_entrypoint",
        "source_digest": oven_model::digest::digest_content(&source),
    });
    std::fs::write(path, serde_json::to_vec(&report)?)?;
    Ok(())
}

/// Write refusals to the file [`REFUSAL_REPORT`] names, one per line; without the variable this does nothing.
fn write_refusal_report(refusals: &[String]) -> Result<(), std::io::Error> {
    let Some(path) = std::env::var_os(REFUSAL_REPORT) else {
        return Ok(());
    };
    let mut text = String::new();
    for refusal in refusals {
        text.push_str(&refusal.replace('\n', " "));
        text.push('\n');
    }
    std::fs::write(path, text)
}

/// Checked owning modules and their source provenance, with one explicit native entry module.
struct CheckedProgram {
    modules: Vec<BodyIrModule>,
    sources: Vec<String>,
    files: Vec<String>,
    entry: usize,
}

/// Collect and analyze the original source through the CLI's compilation session, retaining canonical checked facts.
/// Native-only declaration refusals follow analysis; provider activation, desugaring, and stdlib loading remain owned
/// by the session. Private checked re-export calls add their exact package targets to executable requirements without
/// changing the ordinary reference graph or requesting a declaration by its facade spelling.
/// An explicit invocation package affects only checked registry materialization, independently of
/// the native crate's symbol identity.
fn checked_modules(path: &Path, package_override: Option<&str>) -> Result<CheckedProgram, String> {
    let _invocation = tracing::trace_span!(target: "incan::invocation", "incan_rustc_driver::checked_modules", input = %path.display()).entered();
    let path = path.canonicalize().map_err(|error| error.to_string())?;
    let session = CompilationSession::discover_with_feature_selection(&path, &Default::default())
        .map_err(|error| error.to_string())?;
    let modules =
        collect_modules_detailed_with_session(path.clone(), &session).map_err(|failure| failure.render_human())?;
    let inspection = session
        .prepare_check_rust_inspection(&path, &modules)
        .map_err(|error| error.to_string())?;
    if let Some(workspace) = inspection.as_ref() {
        workspace.verify_ordinary_native().map_err(|error| error.to_string())?;
    }
    let analysis = {
        let _invocation = tracing::trace_span!(target: "incan::invocation", "incan_rustc_driver::analyze_modules", modules = modules.len()).entered();
        session
            .analyze_modules(&modules, inspection.as_ref().map(|workspace| workspace.manifest_dir()))
            .map_err(|failure| failure.render_human())?
    };
    if let Some(workspace) = inspection.as_ref() {
        workspace.verify_ordinary_native().map_err(|error| error.to_string())?;
    }
    let entry = modules
        .iter()
        .position(|module| module.file_path == path)
        .ok_or("entry module is missing")?;
    let string_conversion_contract = incan_frontend::body_ir::compilation_string_conversion_contract(
        modules.iter().map(|module| &module.ast),
        session.provider_plan(),
        analysis.stdlib_cache(),
    )
    .ok_or("unsupported Body IR string conversion contract identity")?;
    let facts = modules
        .iter()
        .map(|module| {
            analysis
                .type_info_for_path(&module.file_path)
                .map(|info| info.semantic_fact_store(&module.path_segments))
                .ok_or("module analysis is missing")
        })
        .collect::<Result<Vec<_>, _>>()?;
    let roots = modules
        .iter()
        .filter_map(|module| analysis.type_info_for_path(&module.file_path))
        .flat_map(|info| info.declarations.declaration_identities.values().cloned());
    let required = incan_semantics_core::dependencies::CheckedDependencyGraph::from_fact_stores(&facts)
        .reachable_from(roots)
        .into_iter()
        .chain(
            modules
                .iter()
                .filter_map(|module| analysis.type_info_for_path(&module.file_path))
                .flat_map(|info| info.calls.body_ir_reexport_calls.values())
                .map(|call| call.canonical.clone()),
        )
        .filter(|identity| match &identity.origin {
            incan_semantics_core::SymbolOrigin::Package { library, .. } => !session
                .provider_plan()
                .active_sdk_records()
                .any(|provider| &provider.identity.name == library),
            _ => false,
        })
        .collect();
    let resolved =
        incan_frontend::executable_resolution::resolve_executable_requirements(session.provider_plan(), &required)
            .map_err(|error| format!("unsupported Body IR package executable representation: {error}"))?;
    let mut bodies = resolved.modules;

    // ---- Stdlib declarations published by the linked SDK packages ----
    // They are context for every module's Body IR, so stdlib enums, newtypes and layouts resolve by identity as package
    // declarations do, and declaration-only program modules the lowering names by their crate items.
    let mut library_identities = std::collections::BTreeSet::new();
    let mut constant_bindings = std::collections::BTreeMap::new();
    for module in &modules {
        let type_info = analysis
            .type_info_for_path(&module.file_path)
            .ok_or("module analysis is missing")?;
        library_identities.extend(incan_frontend::body_ir::library_type_identities(type_info));
        constant_bindings.extend(referenced_sdk_constants(type_info, session.provider_plan()));
    }
    library_identities.extend(constant_bindings.values().cloned());
    let published_declarations = published_library_modules(session.provider_plan(), &library_identities)?;
    let mut published_implementations = published_declarations
        .iter()
        .flat_map(|module| module.trait_implementations.iter().cloned())
        .collect::<Vec<_>>();
    bodies.extend(published_declarations);
    bodies.extend(sdk_constant_binding_modules(&bodies, &constant_bindings)?);
    // Published fragments retain canonical spans but do not carry source text. Do not attribute those spans to the
    // entry.
    let mut files = bodies
        .iter()
        .map(|module| module.module_id.path().to_owned())
        .collect::<Vec<_>>();
    let mut sources = vec![String::new(); bodies.len()];
    let mut entry = entry + bodies.len();
    let mut library_layouts = Vec::new();
    let mut library_enums = Vec::new();
    let mut library_calls = Vec::new();
    let mut factory_bodies = std::collections::BTreeSet::new();
    let package_identity = package_override.unwrap_or_else(|| registry_package_identity(&session, &path));
    for module in modules {
        let type_info = analysis
            .type_info_for_path(&module.file_path)
            .ok_or("module analysis is missing")?;
        validate_declarations(&module.ast, type_info)?;
        let (body_ir, layouts, calls) =
            incan_frontend::body_ir::build_body_ir_module_v0_with_sdk_linkage_and_registry_package(
                &module.ast,
                &module.path_segments,
                type_info,
                &bodies,
                package_identity,
            );
        validate_retained_storage(&module.ast, &body_ir)?;
        library_layouts.extend(layouts);
        library_calls.extend(calls);
        library_enums.extend(incan_frontend::body_ir::library_enum_declarations(type_info));
        factory_bodies.extend(incan_frontend::body_ir::library_factory_body_requirements(type_info));
        bodies.push(body_ir);
        sources.push(module.source);
        files.push(module.file_path.to_string_lossy().into_owned());
    }
    // ---- Stdlib trait defaults the program's adopters take, from the SDK's published representation ----
    for module in published_standard_methods(session.provider_plan(), &bodies, &factory_bodies)? {
        published_implementations.extend(module.trait_implementations.iter().cloned());
        carry_published_module(module, &mut bodies, &mut files, &mut sources, &mut entry);
    }
    retain_selected_published_implementations(&mut bodies, &published_implementations);
    // A proven transparent forwarder keeps its direct Rust callee; the crate item serves every other stdlib call.
    library_calls.retain(|call: &incan_semantics_core::body_ir::StdlibDelegation| {
        !bodies
            .iter()
            .flat_map(|module| &module.stdlib_delegations)
            .any(|forwarder| forwarder.canonical == call.canonical)
    });
    // A layout the SDK did not publish keeps the record built from its stdlib source.
    let published = bodies
        .iter()
        .flat_map(|module| &module.nominal_declarations)
        .map(|declaration| declaration.canonical.clone())
        .collect::<std::collections::BTreeSet<_>>();
    library_layouts
        .retain(|layout: &incan_semantics_core::body_ir::NominalDeclaration| !published.contains(&layout.canonical));
    let libraries = library_modules(library_layouts, library_enums, library_calls);
    for module in libraries {
        carry_published_module(module, &mut bodies, &mut files, &mut sources, &mut entry);
    }
    let native_unions = incan_frontend::body_ir::library_native_union_declarations(session.provider_plan(), &bodies)?;
    let entry_module = bodies.get_mut(entry).ok_or("selected Body IR entry is missing")?;
    entry_module.native_union_declarations = native_unions;
    entry_module.string_conversion_contract = string_conversion_contract;
    Ok(CheckedProgram {
        modules: bodies,
        sources,
        files,
        entry,
    })
}

/// Project authenticated SDK relations only after executable demand has reached its fixed point. Declaration-only
/// imports retain layouts and adoption facts, but cannot introduce an owned implementation whose body was never
/// selected. Source records are unchanged; protocol defaults keep their lazy slot identity, and every selected or
/// malformed retained body still passes the ordinary Body IR validator.
fn retain_selected_published_implementations(
    modules: &mut [BodyIrModule],
    published: &[incan_semantics_core::body_ir::TraitImplementation],
) {
    let bodies = modules
        .iter()
        .flat_map(|module| &module.bodies)
        .filter_map(|body| body.canonical.clone())
        .collect::<std::collections::BTreeSet<_>>();
    for module in modules {
        module.trait_implementations.retain(|record| {
            !published.contains(record)
                || record.implementation == record.method
                || bodies.contains(&record.implementation)
        });
    }
}

/// Select the same manifest-or-entrypoint package name as legacy project preparation, independently of the native crate
/// name.
fn registry_package_identity<'a>(session: &'a CompilationSession, path: &'a Path) -> &'a str {
    session
        .manifest
        .as_ref()
        .and_then(|manifest| manifest.project.as_ref())
        .and_then(|project| project.name.as_deref())
        .or_else(|| path.file_stem().and_then(|stem| stem.to_str()))
        .unwrap_or("incan_project")
}

/// Request integer constants by their checked canonical identities from active SDK providers, preserving published
/// values. Constants already retained by the source-validated Body IR collector keep that literal route; every
/// unretained identity still requires the existing package publication checks.
fn referenced_sdk_constants(
    type_info: &incan_frontend::typechecker::TypeCheckInfo,
    plan: &incan_frontend::provider::ProviderPlan,
) -> std::collections::BTreeMap<incan_semantics_core::CanonicalSymbolId, incan_semantics_core::CanonicalSymbolId> {
    let retained = incan_frontend::body_ir::library_constant_declarations(type_info)
        .into_iter()
        .map(|declaration| declaration.canonical)
        .collect::<std::collections::BTreeSet<_>>();
    type_info
        .references
        .resolved_identities
        .values()
        .filter(|identity| identity.kind == incan_semantics_core::SemanticSourceTargetKind::Const)
        .filter(|identity| !retained.contains(*identity))
        .filter_map(|identity| {
            let mut published = identity.clone();
            match &identity.origin {
                incan_semantics_core::SymbolOrigin::Package { library, .. }
                    if plan
                        .active_sdk_records()
                        .any(|provider| &provider.identity.name == library) => {}
                incan_semantics_core::SymbolOrigin::Module(path) => {
                    let provider = plan.active_sdk_provider_for_module(path)?;
                    let (root, module_path) = path.split_first()?;
                    if root != "std" {
                        return None;
                    }
                    published.origin = incan_semantics_core::SymbolOrigin::Package {
                        library: provider.identity.name.clone(),
                        module_path: module_path.to_vec(),
                    };
                }
                _ => return None,
            }
            Some((identity.clone(), published))
        })
        .collect()
}

/// Preserve checked source-cache identities as declaration-only views of the exact selected SDK constant.
///
/// The SDK decoder validates coverage, spans, digests and values under the package identity first. This bridge changes
/// only the origin to the original checked binding, whose canonical module is the SDK provider's admitted module.
/// It never changes names, kinds, spans or values, and absent publication remains a named refusal.
fn sdk_constant_binding_modules(
    modules: &[BodyIrModule],
    bindings: &std::collections::BTreeMap<
        incan_semantics_core::CanonicalSymbolId,
        incan_semantics_core::CanonicalSymbolId,
    >,
) -> Result<Vec<BodyIrModule>, String> {
    let constants = modules
        .iter()
        .flat_map(|module| &module.constant_declarations)
        .map(|declaration| (&declaration.canonical, declaration))
        .collect::<std::collections::BTreeMap<_, _>>();
    let mut owners = std::collections::BTreeMap::<String, BodyIrModule>::new();
    for (binding, published) in bindings {
        let declaration = constants.get(published).ok_or_else(|| {
            format!(
                "unsupported Body IR selected SDK constant `{}` has no retained value",
                published.declaration_name,
            )
        })?;
        let supported = match &declaration.value {
            incan_semantics_core::body_ir::Constant::Int(_) => true,
            incan_semantics_core::body_ir::Constant::NumericNewtype(value) => {
                let layouts: Vec<_> = modules
                    .iter()
                    .flat_map(|module| &module.nominal_declarations)
                    .filter(|layout| value.constructor.canonical.as_ref() == Some(&layout.canonical))
                    .collect();
                layouts.len() == 1 && layouts.first().is_some_and(|layout| value.matches_layout(layout))
            }
            _ => false,
        };
        if !supported {
            return Err(format!(
                "unsupported Body IR selected SDK constant `{}` outside authenticated integer or numeric newtype representation",
                published.declaration_name,
            ));
        }
        if binding == published {
            continue;
        }
        let owner = incan_semantics_core::canonical_module_identity(binding)
            .ok_or("unsupported Body IR SDK constant binding without a module")?;
        let mut view = (*declaration).clone();
        view.canonical = binding.clone();
        owners
            .entry(owner.clone())
            .or_insert_with(|| declaration_module(owner))
            .constant_declarations
            .push(view);
    }
    Ok(owners.into_values().collect())
}

/// Carry checked stdlib layouts, enums, and crate calls once into declaration-only modules grouped by their declaring
/// module. Each layout validates against its retained module identity; lowering decides whether to link it or declare
/// it locally.
fn library_modules(
    layouts: Vec<incan_semantics_core::body_ir::NominalDeclaration>,
    enums: Vec<incan_semantics_core::body_ir::EnumDeclaration>,
    calls: Vec<incan_semantics_core::body_ir::StdlibDelegation>,
) -> Vec<BodyIrModule> {
    let mut owners = std::collections::BTreeMap::<String, BodyIrModule>::new();
    let mut carried = std::collections::BTreeSet::new();
    for declaration in layouts {
        if let Some(module) = library_module(&mut owners, &mut carried, &declaration.canonical) {
            module.nominal_declarations.push(declaration);
        }
    }
    for call in calls {
        if let Some(module) = library_module(&mut owners, &mut carried, &call.canonical) {
            module.stdlib_delegations.push(call);
        }
    }
    for declaration in enums {
        if let Some(module) = library_module(&mut owners, &mut carried, &declaration.canonical) {
            module.enum_declarations.push(declaration);
        }
    }
    owners.into_values().collect()
}

/// Resolve the stdlib types the program uses from the representations its linked SDK packages published, one
/// declaration-only module per stdlib module.
///
/// Each identity resolves on its own, so a type a package does not publish, or one that is not an SDK type at all,
/// is simply absent and keeps its source record or refusal. Only declaration records are kept: layouts, enums,
/// constants, aliases and their checked protocol implementation relations. Published bodies stay out here because
/// ordinary SDK calls use compiled items; [`published_standard_methods`] selects their executable protocol bodies.
fn published_library_modules(
    plan: &incan_frontend::provider::ProviderPlan,
    identities: &std::collections::BTreeSet<incan_semantics_core::CanonicalSymbolId>,
) -> Result<Vec<BodyIrModule>, String> {
    use std::collections::BTreeSet;
    let mut owners = std::collections::BTreeMap::<String, BodyIrModule>::new();
    let (mut nominals, mut enums, mut fieldless, mut values) =
        (BTreeSet::new(), BTreeSet::new(), BTreeSet::new(), BTreeSet::new());
    let (mut constants, mut aliases) = (BTreeSet::new(), BTreeSet::new());
    for identity in identities {
        let required = std::iter::once(identity.clone()).collect();
        let resolved = match incan_frontend::executable_resolution::resolve_sdk_requirements(plan, &required) {
            Ok(resolved) => resolved,
            Err(error) if identity.kind == incan_semantics_core::SemanticSourceTargetKind::Const => {
                return Err(format!(
                    "unsupported Body IR selected SDK constant publication: {error}"
                ));
            }
            Err(_) => continue,
        };
        for module in resolved.modules {
            let path = module.module_id.path().to_owned();
            let owner = owners.entry(path.clone()).or_insert_with(|| declaration_module(path));
            for declaration in module.nominal_declarations {
                if nominals.insert(declaration.canonical.clone()) {
                    owner.nominal_declarations.push(declaration);
                }
            }
            for declaration in module.enum_declarations {
                if enums.insert(declaration.canonical.clone()) {
                    owner.enum_declarations.push(declaration);
                }
            }
            for declaration in module.fieldless_enum_declarations {
                if fieldless.insert(declaration.canonical.clone()) {
                    owner.fieldless_enum_declarations.push(declaration);
                }
            }
            for declaration in module.value_enum_declarations {
                if values.insert(declaration.canonical.clone()) {
                    owner.value_enum_declarations.push(declaration);
                }
            }
            for declaration in module.constant_declarations {
                if constants.insert(declaration.canonical.clone()) {
                    owner.constant_declarations.push(declaration);
                }
            }
            for declaration in module.type_alias_declarations {
                if aliases.insert(declaration.canonical.clone()) {
                    owner.type_alias_declarations.push(declaration);
                }
            }
            for implementation in module.trait_implementations {
                if !owner.trait_implementations.contains(&implementation) {
                    owner.trait_implementations.push(implementation);
                }
            }
        }
    }
    Ok(owners.into_values().collect())
}

/// Resolve called SDK protocol implementations and explicit factory roots through their authenticated executable
/// closures.
///
/// A selected factory or default can itself call another checked protocol slot. Its concrete adopter's implementation
/// is a distinct canonical root, so selection continues until newly retained bodies introduce no further roots. Each
/// identity is requested once, including cycles. Called slots and native ordering defaults select additional roots;
/// their declaration dependency closure remains owned by the SDK. Unavailable selected fragments retain their exact
/// executable representation failure instead of guessing a body or dispatching by spelling.
fn published_standard_methods(
    plan: &incan_frontend::provider::ProviderPlan,
    modules: &[BodyIrModule],
    factories: &std::collections::BTreeSet<incan_semantics_core::CanonicalSymbolId>,
) -> Result<Vec<BodyIrModule>, String> {
    let mut pending = factories.clone();
    let mut requested = std::collections::BTreeSet::new();
    let mut owners = std::collections::BTreeMap::<String, BodyIrModule>::new();

    // ---- Called slots in the original program and newly retained SDK bodies ----
    loop {
        let mut called = std::collections::BTreeSet::new();
        for body in modules.iter().chain(owners.values()).flat_map(|module| &module.bodies) {
            collect_called_methods(&body.block.stmts, &mut called);
        }
        retain_native_ordering_defaults(modules, &mut called);
        retain_native_fallible_defaults(plan, modules.iter().chain(owners.values()), &mut called);
        pending.extend(
            modules
                .iter()
                .chain(owners.values())
                .flat_map(|module| &module.trait_implementations)
                .filter(|record| called.contains(&record.method))
                .filter(|record| {
                    matches!(&record.implementation.origin, incan_semantics_core::SymbolOrigin::Package { library, .. }
                        if plan.active_sdk_records().any(|provider| &provider.identity.name == library))
                })
                .map(|record| record.implementation.clone()),
        );
        pending.retain(|identity| !requested.contains(identity));
        if pending.is_empty() {
            break;
        }

        // ---- Exact provider-validated roots, with no repeated request in a cycle ----
        for identity in std::mem::take(&mut pending) {
            requested.insert(identity.clone());
            let required = std::iter::once(identity).collect();
            let resolved = incan_frontend::executable_resolution::resolve_sdk_requirements(plan, &required)
                .map_err(|error| format!("unsupported Body IR package executable representation: {error}"))?;
            for module in resolved.modules {
                let path = module.module_id.path().to_owned();
                let owner = owners.entry(path.clone()).or_insert_with(|| declaration_module(path));
                join_published_module(owner, module);
            }
        }
    }
    Ok(owners.into_values().collect())
}

/// Native SDK adapter storage requires the complete standard fallible protocol for its checked source poll owner.
/// Source-only adopters keep lazy defaults: only a carrier occurring inside an actual SDK type instantiation selects
/// these additional roots, and provider resolution still proves each requested body's executable coverage.
/// Every selection iteration includes already-retained SDK owners, because a factory may introduce the protocol
/// declaration and concrete storage that authenticate its source poll owner after the original modules were checked.
fn retain_native_fallible_defaults<'module>(
    plan: &incan_frontend::provider::ProviderPlan,
    modules: impl Iterator<Item = &'module BodyIrModule> + Clone,
    required: &mut std::collections::BTreeSet<CanonicalSymbolId>,
) {
    for module in modules.clone() {
        for source in &module.trait_implementations {
            if source.implementation == source.method || !standard_fallible_slot(&source.method, "__next__") {
                continue;
            }
            let Some(protocol) = modules
                .clone()
                .flat_map(|module| &module.trait_declarations)
                .find(|protocol| {
                    protocol.kind == SemanticSourceTargetKind::Trait
                        && protocol.declaration_name == "FallibleIterator"
                        && protocol.origin == source.method.origin
                        && protocol.declaration_span.start <= source.method.declaration_span.start
                        && protocol.declaration_span.end >= source.method.declaration_span.end
                })
            else {
                continue;
            };
            if !modules.clone().flat_map(|module| &module.bodies).any(|body| {
                body.locals
                    .iter()
                    .any(|local| native_type_uses_owner(plan, &local.ty, &body.named_type_identities, &source.owner))
            }) {
                continue;
            }
            for record in &module.trait_implementations {
                if record.owner == source.owner
                    && record.implementation == record.method
                    && protocol.declaration_span.start <= record.method.declaration_span.start
                    && protocol.declaration_span.end >= record.method.declaration_span.end
                    && [
                        "__iter__",
                        "map",
                        "filter",
                        "flat_map",
                        "take",
                        "inspect",
                        "map_err",
                        "inspect_err",
                        "collect",
                        "fold",
                    ]
                    .iter()
                    .any(|name| standard_fallible_slot(&record.method, name))
                {
                    required.insert(record.method.clone());
                }
            }
        }
    }
}

/// Identify a standard fallible member before requesting its published default; source spelling grants no authority.
fn standard_fallible_slot(identity: &CanonicalSymbolId, name: &str) -> bool {
    identity.kind == SemanticSourceTargetKind::Method
        && identity.namespace == SymbolNamespace::Member
        && identity.declaration_name == name
        && matches!(&identity.origin, SymbolOrigin::Package { library, module_path }
            if library == "incan_stdlib_core" && module_path.iter().map(String::as_str).eq(["derives", "collection"]))
}

/// Find an exact checked source owner inside an active SDK generic, before nominal specialization rewrites names.
fn native_type_uses_owner(
    plan: &incan_frontend::provider::ProviderPlan,
    ty: &incan_semantics_core::IncanType,
    identities: &std::collections::BTreeMap<String, CanonicalSymbolId>,
    owner: &CanonicalSymbolId,
) -> bool {
    use incan_semantics_core::IncanType;
    match ty {
        IncanType::Generic { base, args } => {
            let identity = checked_carrier_identity(base, identities);
            let sdk = identity.is_some_and(|identity| {
                matches!(&identity.origin, SymbolOrigin::Package { library, .. }
                if plan.active_sdk_records().any(|provider| &provider.identity.name == library))
            });
            (sdk && args
                .iter()
                .any(|argument| type_contains_owner(argument, identities, owner)))
                || args
                    .iter()
                    .any(|argument| native_type_uses_owner(plan, argument, identities, owner))
        }
        IncanType::Ref(inner) | IncanType::RefMut(inner) => native_type_uses_owner(plan, inner, identities, owner),
        IncanType::Tuple(items) => items
            .iter()
            .any(|item| native_type_uses_owner(plan, item, identities, owner)),
        _ => false,
    }
}

/// Match nested checked arguments to the selected source declaration without equating same-spelled foreign owners.
fn type_contains_owner(
    ty: &incan_semantics_core::IncanType,
    identities: &std::collections::BTreeMap<String, CanonicalSymbolId>,
    owner: &CanonicalSymbolId,
) -> bool {
    use incan_semantics_core::IncanType;
    match ty {
        IncanType::Named(name) => checked_carrier_identity(name, identities) == Some(owner),
        IncanType::Generic { base, args } => {
            checked_carrier_identity(base, identities) == Some(owner)
                || args
                    .iter()
                    .any(|argument| type_contains_owner(argument, identities, owner))
        }
        IncanType::Ref(inner) | IncanType::RefMut(inner) => type_contains_owner(inner, identities, owner),
        IncanType::Tuple(items) => items.iter().any(|item| type_contains_owner(item, identities, owner)),
        _ => false,
    }
}

/// Resolve a checked leaf or canonical carrier key, refusing a competing direct binding.
fn checked_carrier_identity<'facts>(
    name: &str,
    identities: &'facts std::collections::BTreeMap<String, CanonicalSymbolId>,
) -> Option<&'facts CanonicalSymbolId> {
    let direct = identities.get(name);
    let encoded = identities
        .values()
        .find(|identity| incan_semantics_core::body_ir::nominal_carrier_key(identity).as_deref() == Some(name));
    match encoded {
        Some(identity) if direct.is_none_or(|binding| binding == identity) => Some(identity),
        Some(_) => None,
        None => direct,
    }
}

/// Request checked standard ordering defaults needed by native trait forwarding, including implicit operator calls.
/// A concrete source less-than implementation identifies the adopter; matching standard slot identities supply its
/// other methods.
fn retain_native_ordering_defaults(
    modules: &[BodyIrModule],
    required: &mut std::collections::BTreeSet<incan_semantics_core::CanonicalSymbolId>,
) {
    for module in modules {
        for source in &module.trait_implementations {
            if source.implementation == source.method || !standard_ordering_slot(&source.method, "__lt__") {
                continue;
            }
            for record in &module.trait_implementations {
                if record.owner == source.owner
                    && record.method == record.implementation
                    && ["__le__", "__gt__", "__ge__"]
                        .iter()
                        .any(|name| standard_ordering_slot(&record.method, name))
                {
                    required.insert(record.method.clone());
                }
            }
        }
    }
}

/// Require a standard comparison member's exact source or SDK canonical identity before requesting its default body.
fn standard_ordering_slot(identity: &CanonicalSymbolId, name: &str) -> bool {
    if identity.kind != SemanticSourceTargetKind::Method
        || identity.namespace != SymbolNamespace::Member
        || identity.declaration_name != name
    {
        return false;
    }
    match &identity.origin {
        SymbolOrigin::Module(path) => path.iter().map(String::as_str).eq(["std", "derives", "comparison"]),
        SymbolOrigin::Package { library, module_path } => {
            library == "incan_stdlib_core" && module_path.iter().map(String::as_str).eq(["derives", "comparison"])
        }
        _ => false,
    }
}

/// Collect the checked slots and methods that statements call, through nested blocks, match arms and closures.
///
/// A call this walk does not reach only leaves a default unresolved, and a call to that default then refuses where it
/// is lowered.
fn collect_called_methods(
    statements: &[incan_semantics_core::body_ir::Statement],
    called: &mut std::collections::BTreeSet<incan_semantics_core::CanonicalSymbolId>,
) {
    use incan_semantics_core::body_ir::{Callee, Rvalue, StatementKind};
    for statement in statements {
        match &statement.kind {
            StatementKind::Call {
                callee: Callee::Method(target),
                ..
            } => called.extend(target.canonical.iter().cloned()),
            StatementKind::If {
                then_block, else_block, ..
            } => {
                collect_called_methods(&then_block.stmts, called);
                if let Some(block) = else_block {
                    collect_called_methods(&block.stmts, called);
                }
            }
            StatementKind::Loop { body } => collect_called_methods(&body.stmts, called),
            StatementKind::Assign { rvalue, .. } => match rvalue {
                Rvalue::Match { arms, .. } => {
                    for arm in arms {
                        collect_called_methods(&arm.guard_stmts, called);
                        collect_called_methods(&arm.body_stmts, called);
                    }
                }
                Rvalue::Closure { body, .. } => collect_called_methods(&body.stmts, called),
                _ => {}
            },
            _ => {}
        }
    }
}

/// Carry one published stdlib module into the program: it joins the module already carried for the same owner, or
/// travels as a module of its own ahead of the program's modules, as a published package's declarations do.
fn carry_published_module(
    module: BodyIrModule,
    bodies: &mut Vec<BodyIrModule>,
    files: &mut Vec<String>,
    sources: &mut Vec<String>,
    entry: &mut usize,
) {
    if let Some(carried) = bodies.iter_mut().find(|carried| carried.module_id == module.module_id) {
        join_published_module(carried, module);
        return;
    }
    files.insert(0, module.module_id.path().to_owned());
    sources.insert(0, String::new());
    bodies.insert(0, module);
    *entry += 1;
}

/// Join published declarations, default bodies and crate-call records for the same stdlib owner, retaining each
/// canonical identity once so lowering never visits an owner's bodies through several module fragments.
fn join_published_module(owner: &mut BodyIrModule, module: BodyIrModule) {
    for delegation in module.stdlib_delegations {
        if !owner
            .stdlib_delegations
            .iter()
            .any(|carried| carried.canonical == delegation.canonical)
        {
            owner.stdlib_delegations.push(delegation);
        }
    }
    for body in module.bodies {
        if !owner.bodies.iter().any(|carried| carried.canonical == body.canonical) {
            owner.bodies.push(body);
        }
    }
    for declaration in module.trait_declarations {
        if !owner.trait_declarations.contains(&declaration) {
            owner.trait_declarations.push(declaration);
        }
    }
    for declaration in module.nominal_declarations {
        if !owner
            .nominal_declarations
            .iter()
            .any(|carried| carried.canonical == declaration.canonical)
        {
            owner.nominal_declarations.push(declaration);
        }
    }
    for declaration in module.enum_declarations {
        if !owner
            .enum_declarations
            .iter()
            .any(|carried| carried.canonical == declaration.canonical)
        {
            owner.enum_declarations.push(declaration);
        }
    }
    for implementation in module.trait_implementations {
        if !owner.trait_implementations.contains(&implementation) {
            owner.trait_implementations.push(implementation);
        }
    }
}

/// An empty module for one stdlib module identity, holding only the declarations the program needs from it.
fn declaration_module(owner: String) -> BodyIrModule {
    BodyIrModule {
        string_conversion_contract: incan_semantics_core::body_ir::StringConversionContract::Unselected,
        decorated_bindings: Vec::new(),
        entrypoint_effects: Vec::new(),
        constant_declarations: Vec::new(),
        type_alias_declarations: Vec::new(),
        enum_declarations: Vec::new(),
        stdlib_delegations: Vec::new(),
        public_exports: Vec::new(),
        native_union_declarations: Vec::new(),
        static_declarations: Vec::new(),
        module_id: incan_semantics_core::CompilerNodeId::module(owner),
        nominal_declarations: Vec::new(),
        fieldless_enum_declarations: Vec::new(),
        value_enum_declarations: Vec::new(),
        trait_declarations: Vec::new(),
        trait_implementations: Vec::new(),
        bodies: Vec::new(),
    }
}

/// The declaration-only module owning one stdlib identity, or nothing when that identity already travels.
fn library_module<'modules>(
    owners: &'modules mut std::collections::BTreeMap<String, BodyIrModule>,
    carried: &mut std::collections::BTreeSet<incan_semantics_core::CanonicalSymbolId>,
    identity: &incan_semantics_core::CanonicalSymbolId,
) -> Option<&'modules mut BodyIrModule> {
    let owner = incan_semantics_core::body_ir::native_nominal_module(identity)?;
    if !carried.insert(identity.clone()) {
        return None;
    }
    Some(owners.entry(owner.clone()).or_insert_with(|| declaration_module(owner)))
}

/// Refuse source storage whose checked layout was deliberately omitted from Body IR.
///
/// In particular, a checked newtype constructor must not reach body validation as if it were an ordinary raw wrapper.
/// The frontend declaration collector owns that decision; this boundary only checks whether its layout was retained.
/// Decorated callable replacements share the storage table but are validated under their original function identity,
/// so they do not count as source `static` declarations.
fn validate_retained_storage(program: &incan_frontend::ast::Program, module: &BodyIrModule) -> Result<(), String> {
    use incan_frontend::ast::Declaration;
    use incan_semantics_core::SemanticSourceTargetKind;

    let static_count = program
        .declarations
        .iter()
        .filter(|declaration| matches!(declaration.node, Declaration::Static(_)))
        .count();
    let retained_statics = module
        .static_declarations
        .iter()
        .filter(|declaration| {
            declaration.canonical.kind == SemanticSourceTargetKind::Static
                && incan_semantics_core::canonical_module_identity(&declaration.canonical).as_deref()
                    == Some(module.module_id.path())
                && program.declarations.iter().any(|source| {
                    matches!(&source.node, Declaration::Static(storage)
                        if storage.name == declaration.canonical.declaration_name
                            && source.span.start == declaration.canonical.declaration_span.start
                            && source.span.end == declaration.canonical.declaration_span.end)
                })
        })
        .count();
    if static_count != retained_statics {
        return Err("unsupported source Static initializer or carrier on the native route".to_owned());
    }
    let newtype_count = program
        .declarations
        .iter()
        .filter(|declaration| matches!(declaration.node, Declaration::Newtype(_)))
        .count();
    let retained_newtypes = module
        .nominal_declarations
        .iter()
        .filter(|declaration| declaration.canonical.kind == SemanticSourceTargetKind::Newtype)
        .count();
    if newtype_count != retained_newtypes {
        return Err("unsupported source checked Newtype construction on the native route".to_owned());
    }
    Ok(())
}

/// Refuse declarations whose observable behavior the direct route cannot retain, independently for each module.
fn validate_declarations(
    program: &incan_frontend::ast::Program,
    type_info: &incan_frontend::typechecker::TypeCheckInfo,
) -> Result<(), String> {
    for declaration in &program.declarations {
        use incan_frontend::ast::Declaration;
        let kind = match &declaration.node {
            Declaration::Function(_) | Declaration::Docstring(_) | Declaration::Import(_) | Declaration::Const(_) => {
                continue;
            }
            Declaration::Static(_) => continue,
            Declaration::Model(model) => {
                if !incan_frontend::body_ir::is_direct_replacement_checked_model(model, type_info) {
                    return Err(format!(
                        "unsupported source Model {} on {} on the native route",
                        model_refusal_feature(model),
                        model.name
                    ));
                }
                validate_generic_adoptions(&model.traits, program, type_info)?;
                validate_trait_partial_overrides(&model.traits, &model.methods, program)?;
                continue;
            }
            Declaration::Class(class) => {
                if !incan_frontend::body_ir::is_direct_replacement_checked_class(class, type_info) {
                    return Err(format!(
                        "unsupported source Class {} on {} on the native route",
                        class_refusal_feature(class),
                        class.name
                    ));
                }
                validate_generic_adoptions(&class.traits, program, type_info)?;
                validate_trait_partial_overrides(&class.traits, &class.methods, program)?;
                continue;
            }
            Declaration::Enum(value) => {
                if !incan_frontend::body_ir::is_direct_native_enum(value) {
                    return Err(format!("unsupported source Enum {} on the native route", value.name));
                }
                if !incan_frontend::body_ir::has_direct_native_enum_derives(value, type_info) {
                    return Err(format!(
                        "unsupported source Enum derives on {} on the native route",
                        value.name
                    ));
                }
                validate_generic_adoptions(&value.traits, program, type_info)?;
                continue;
            }
            Declaration::Trait(item) => {
                if let Some(feature) = trait_refusal_feature(item) {
                    return Err(format!("unsupported source Trait {feature} on the native route"));
                }
                continue;
            }
            Declaration::Newtype(newtype)
                if incan_frontend::body_ir::is_direct_replacement_checked_newtype(newtype, type_info) =>
            {
                validate_generic_adoptions(&newtype.traits, program, type_info)?;
                continue;
            }
            Declaration::Newtype(_) => "nonplain Newtype",
            Declaration::Alias(_) => continue,
            Declaration::Partial(_) => continue,
            // Type aliases have no runtime declaration; checked Body IR carries their resolved uses.
            Declaration::TypeAlias(_) => continue,
            _ => "top-level declaration",
        };
        return Err(format!("unsupported source {kind} on the native route"));
    }
    Ok(())
}

/// Name the first unsupported model feature without relaxing the frontend's declaration admission predicate.
fn model_refusal_feature(model: &incan_frontend::ast::ModelDecl) -> String {
    // ---- Member binding features ----
    if !incan_frontend::body_ir::has_direct_replacement_method_partials(
        &model.type_params,
        &model.methods,
        &model.method_aliases,
        &model.method_partials,
    ) {
        return "method partials".to_owned();
    }
    if model.properties.iter().any(|property| property.node.body.is_none()) {
        return "abstract properties".to_owned();
    }

    // ---- Method facts ----
    if has_refused_method_decorator(&model.methods) {
        return "decorated methods".to_owned();
    }

    // ---- Decorator expansion ----
    refused_decorator_feature(&model.decorators).unwrap_or_else(|| "unsupported declaration shape".to_owned())
}

/// Name the first unsupported class feature without relaxing the frontend's declaration admission predicate.
fn class_refusal_feature(class: &incan_frontend::ast::ClassDecl) -> String {
    // ---- Structural declaration features ----
    if class.extends.is_some() {
        return "inheritance".to_owned();
    }
    // ---- Member binding features ----
    if !incan_frontend::body_ir::has_direct_replacement_method_partials(
        &class.type_params,
        &class.methods,
        &class.method_aliases,
        &class.method_partials,
    ) {
        return "method partials".to_owned();
    }
    if class.properties.iter().any(|property| property.node.body.is_none()) {
        return "abstract properties".to_owned();
    }
    if !class.declarative_members.is_empty() {
        return "declarative members".to_owned();
    }

    // ---- Method facts ----
    if has_refused_method_decorator(&class.methods) {
        return "decorated methods".to_owned();
    }

    // ---- Decorator expansion ----
    refused_decorator_feature(&class.decorators).unwrap_or_else(|| "unsupported declaration shape".to_owned())
}

/// Report whether any method carries a decorator other than the builtin markers the direct route admits.
fn has_refused_method_decorator(methods: &[incan_frontend::ast::Spanned<incan_frontend::ast::MethodDecl>]) -> bool {
    methods.iter().any(|method| {
        method
            .node
            .decorators
            .iter()
            .any(|decorator| !incan_frontend::body_ir::is_direct_replacement_method_decorator(&decorator.node))
    })
}

/// Name the first declaration decorator outside the bare builtin derive spelling, with its identifier arguments.
fn refused_decorator_feature(
    decorators: &[incan_frontend::ast::Spanned<incan_frontend::ast::Decorator>],
) -> Option<String> {
    let decorator = decorators.iter().find(|decorator| {
        !incan_frontend::body_ir::is_direct_replacement_model_derive(&decorator.node)
            && !incan_frontend::body_ir::is_lint_suppression_decorator(&decorator.node)
    })?;
    let arguments = decorator
        .node
        .args
        .iter()
        .filter_map(|argument| match argument {
            incan_frontend::ast::DecoratorArg::Positional(value) => match &value.node {
                incan_frontend::ast::Expr::Ident(name) => Some(name.as_str()),
                _ => None,
            },
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(", ");
    Some(format!("decorator @{}({arguments})", decorator.node.name))
}

/// Admit only builtin native derive markers with no executable decorator behavior; adopter effects are checked
/// separately.
fn retained_trait_derive_marker(decorator: &incan_frontend::ast::Decorator) -> bool {
    decorator.path.segments == ["rust", "derive"] && decorator.type_args.is_empty() && !decorator.args.is_empty()
        && decorator.args.iter().all(|argument| matches!(argument,
            DecoratorArg::Positional(value) if matches!(&value.node,
                Expr::Literal(Literal::String(path)) if matches!(path.as_str(), "Debug" | "Clone" | "serde::Serialize" | "serde::Deserialize"))))
}

/// Name the first trait feature the direct route cannot retain.
///
/// Type parameters and supertraits declared in this module are retained: Body IR records each adopter's checked
/// instantiation and the supertrait slots it fills. A method's own type parameters are retained like any method's:
/// each checked call schedules the adopter's default at its call-site arguments. A supertrait from elsewhere adds no
/// declaration behavior of its own: the checker records the serde derives a standard JSON supertrait implies, and a
/// call of one of its slots that has no retained body refuses by name where it is lowered.
/// Literal presets on source-local method partials become checked call arguments; computed presets remain refused until
/// they have a declaration-owned execution frame.
fn trait_refusal_feature(item: &incan_frontend::ast::TraitDecl) -> Option<&'static str> {
    if item
        .decorators
        .iter()
        .any(|decorator| !retained_trait_derive_marker(&decorator.node))
    {
        return Some("decorators");
    }
    if !item.method_aliases.is_empty() {
        return Some("method aliases");
    }
    if !incan_frontend::body_ir::has_direct_replacement_literal_trait_partials(item) {
        return Some("method partials");
    }
    if item.properties.iter().any(|property| property.node.body.is_some()) {
        return Some("property defaults");
    }
    // Method markers follow the rule models and classes use: `@staticmethod`, `@classmethod`, lint suppressions.
    if has_refused_method_decorator(&item.methods) {
        return Some("decorated methods");
    }
    None
}

/// Whether this module declares a trait under `name`.
fn declares_trait(program: &incan_frontend::ast::Program, name: &str) -> bool {
    program.declarations.iter().any(
        |declaration| matches!(&declaration.node, incan_frontend::ast::Declaration::Trait(item) if item.name == name),
    )
}

/// Preserve the frozen legacy failure for a trait partial whose concrete adopter replaces its forwarding target.
/// Inherited target bodies are admitted; the legacy emitter does not publish the trait target bridge after an override,
/// so this shape has no runtime parity authority.
fn validate_trait_partial_overrides(
    adoptions: &[incan_frontend::ast::Spanned<incan_frontend::ast::TraitBound>],
    methods: &[incan_frontend::ast::Spanned<incan_frontend::ast::MethodDecl>],
    program: &incan_frontend::ast::Program,
) -> Result<(), String> {
    for adoption in adoptions {
        let Some(source) = program
            .declarations
            .iter()
            .find_map(|declaration| match &declaration.node {
                incan_frontend::ast::Declaration::Trait(source) if source.name == adoption.node.name => Some(source),
                _ => None,
            })
        else {
            continue;
        };
        if source
            .method_partials
            .iter()
            .any(|partial| methods.iter().any(|method| method.node.name == partial.node.target))
        {
            return Err(
                "unsupported source Trait method partial with an adopter target override on the native route".into(),
            );
        }
    }
    Ok(())
}

/// Refuse a generic adoption of a trait this module does not declare, unless the standard library declares it.
///
/// Body IR instantiates only source-local trait defaults; another module's generic trait's implementation facts are
/// not retained, so admitting the adoption could lose behavior. A standard-library or SDK protocol's slots are retained
/// where the adopter's own method fills them, and a call that needs one of its defaults refuses by name where lowered.
fn validate_generic_adoptions(
    adoptions: &[incan_frontend::ast::Spanned<incan_frontend::ast::TraitBound>],
    program: &incan_frontend::ast::Program,
    type_info: &incan_frontend::typechecker::TypeCheckInfo,
) -> Result<(), String> {
    if adoptions.iter().any(|adoption| {
        !adoption.node.type_args.is_empty()
            && !declares_trait(program, &adoption.node.name)
            && !type_info.traits.is_standard_trait(&adoption.node.name)
    }) {
        return Err("unsupported source generic adoption of an imported trait on the native route".to_owned());
    }
    Ok(())
}

/// Exact caller-declared native libraries and their dependency search directories.
struct Dependencies {
    externs: Vec<(String, PathBuf)>,
    directories: Vec<PathBuf>,
    /// Explicit logical package selected by the source caller, independent of the native crate's name.
    package_identity: Option<String>,
    /// Compile a library unit (`--library`): an rlib with the entry module's public names, no `main` required.
    library: bool,
}

/// Parse declared native externs, search directories and a logical package override; no ambient crate discovery is
/// performed.
fn dependencies(arguments: &[String]) -> Result<Dependencies, String> {
    let mut externs = Vec::new();
    let mut directories = Vec::new();
    let mut package_identity = None;
    let mut library = false;
    let mut remaining = arguments.iter();
    while let Some(option) = remaining.next() {
        if option == "--library" {
            library = true;
            continue;
        }
        let value = remaining.next().ok_or_else(|| format!("missing value for {option}"))?;
        match option.as_str() {
            "--extern" => {
                let (name, path) = value.split_once('=').ok_or("--extern requires CRATE=RLIB")?;
                externs.push((name.to_owned(), PathBuf::from(path)));
            }
            "--search" => directories.push(PathBuf::from(value)),
            "--package-name" => {
                if value.is_empty() || package_identity.is_some() {
                    return Err("--package-name requires one nonempty package identity".to_owned());
                }
                package_identity = Some(value.clone());
            }
            _ => return Err(format!("unknown native source option {option}")),
        }
    }
    Ok(Dependencies {
        externs,
        directories,
        package_identity,
        library,
    })
}

/// Run source, frontend, canonical Body IR, Incan lowering, plan validation, adapter, and pinned native compilation.
/// Adapter errors cross the executable boundary as their display text so named unsupported gaps retain the CLI's
/// refusal classification instead of appearing as debug-form plan errors.
pub fn compile(arguments: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    if arguments.len() < 4 {
        return Err(
            "usage: --source SOURCE CRATE OUTPUT SYSROOT [--extern CRATE=RLIB] [--search DIR] [--package-name NAME] [--library]"
                .into(),
        );
    }

    // ---- Checked source and Incan plan ----
    let source_path = PathBuf::from(&arguments[0]);
    let _invocation = tracing::trace_span!(target: "incan::invocation", "incan_rustc_driver::compile_source", input = %source_path.display()).entered();
    let identity = crate::identity::verify(Path::new(&arguments[3]))?;
    let (native_link, link_identity) = crate::native_link(&identity)?;
    let output = Path::new(&arguments[2]);
    let dependencies = dependencies(&arguments[4..])?;
    let reuse = incan_driver::build::native_source_output::NativeSourceOutput::observe(
        incan_driver::build::native_source_output::NativeSourceOutputRequest {
            source: &source_path,
            output,
            crate_name: &arguments[1],
            package_identity: dependencies.package_identity.as_deref(),
            sysroot: &identity.sysroot,
            target: &identity.target,
            toolchain: &identity.toolchain,
            link_identity: &link_identity,
            externs: &dependencies.externs,
            search_paths: &dependencies.directories,
        },
    )?;
    if let Some(reuse) = &reuse
        && reuse.try_reuse()?
    {
        return Ok(());
    }
    if std::env::var_os("INCAN_TEST_REQUIRE_COMPLETED_PROJECT_REUSE").is_some() {
        return Err("required completed project reuse missed before frontend preparation".into());
    }
    crate::relaunch_with_native_environment(&native_link)?;
    let checked_source = source_path.clone();
    let checked_package = dependencies.package_identity.clone();
    let parent = tracing::Span::current();
    let checked = incan_frontend::compiler_stack::run_on_compiler_stack(move || {
        let _parent = parent.enter();
        checked_modules(&checked_source, checked_package.as_deref())
    });
    let program = match checked {
        Ok(program) => program,
        Err(error) => {
            write_refusal_report(std::slice::from_ref(&error))?;
            return Err(error.into());
        }
    };
    if std::env::var_os(REFUSAL_REPORT).is_some() {
        let refusals = program_refusals(
            &program.modules,
            program.sources.clone(),
            program.files.clone(),
            program.entry.try_into()?,
        );
        write_refusal_report(&refusals)?;
    }
    let plan = {
        tracing::debug!(target: "incan::invocation", modules = ?program.modules, "checked direct-route Body IR");
        let _invocation = tracing::trace_span!(target: "incan::invocation", "incan_rustc_driver::lower_program", modules = program.modules.len()).entered();
        let entry = program.entry.try_into()?;
        if dependencies.library {
            Some(library_plan(&program.modules, program.sources, program.files, entry)?)
        } else {
            program_plan(&program.modules, program.sources, program.files, entry)?
        }
    };
    let Some(plan) = plan else {
        report_missing_entrypoint(&source_path)?;
        return Err(SourceFailure::MissingEntrypoint.into());
    };

    // ---- Explicit native dependencies ----
    if let Some(root) = crate::marker_traits::native_root(&plan) {
        if !dependencies.externs.iter().any(|(name, _)| name == root) {
            return Err(format!("unsupported Body IR native dependency `{root}` for a checked marker trait").into());
        }
    }
    let library_items = plan
        .models
        .iter()
        .map(|model| &model.library_item)
        .chain(plan.enums.iter().map(|value| &value.library_item))
        .chain(plan.models.iter().map(|model| &model.string_conversion_trait))
        .filter(|item| !item.is_empty());
    for item in library_items {
        let root = item.split("::").next().unwrap_or_default();
        // The standard library's own crates are always linked, as for external calls below.
        if !matches!(root, "std" | "core" | "alloc") && !dependencies.externs.iter().any(|(name, _)| name == root) {
            return Err(format!("unsupported Body IR native dependency `{root}` for stdlib layout `{item}`").into());
        }
    }
    for external in &plan.externals {
        let root = external
            .path
            .split("::")
            .next()
            .ok_or("native external path has no crate")?;
        if !matches!(root, "std" | "core" | "alloc") && !dependencies.externs.iter().any(|(name, _)| name == root) {
            return Err(format!(
                "unsupported Body IR native dependency `{root}` for canonical call `{}`",
                external.path
            )
            .into());
        }
    }
    crate::adapter::compile(
        plan,
        &arguments[1],
        Path::new(&arguments[2]),
        Path::new(&arguments[3]),
        &dependencies.externs,
        &dependencies.directories,
        &native_link,
        dependencies.library,
    )
    .map_err(|error| error.to_string())?;
    if let Some(reuse) = reuse {
        if crate::native_link(&identity)?.1 != link_identity {
            return Err("native link inputs changed during compilation; completed output was not published".into());
        }
        reuse.publish()?
    }
    Ok(())
}
