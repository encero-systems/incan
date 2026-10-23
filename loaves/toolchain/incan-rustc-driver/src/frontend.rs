//! Shared CLI-session source-to-Body-IR glue. All Body-IR-to-plan decisions belong to the Incan lowering Loaf.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use incan_driver::loaf_publication::{PublicationFacts, publish_native_unions, write_loaf};
use incan_driver::modules::{collect_library_modules_detailed_with_session, collect_modules_detailed_with_session};
use incan_driver::session::{CompilationAnalysis, CompilationSession, RustFactsSetup};
use incan_driver::unit::{UnitDescription, UnitKind};
use incan_frontend::ast::{DecoratorArg, Expr, Literal};
use incan_frontend::library_manifest::LibraryManifest;
use incan_frontend::loaf_index::{LoafIndex, LoafNativeClosure};
use incan_frontend::parsed_module::ParsedModule;
use incan_mir_lowering::{program_refusals, unit_plan};
use incan_semantics_core::body_ir::{BodyIrModule, LoafCallable, NominalDeclaration};
use incan_semantics_core::{CanonicalSymbolId, SemanticSourceTargetKind, SymbolOrigin};

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
        "source_digest": format!("sha256:{}", hex::encode(Sha256::digest(source.as_bytes()))),
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
    /// The first of the program's own modules; the Loaf declarations it reaches come before it.
    own: usize,
    /// A library unit's manifest, which its executable representation completes once the unit compiles (DD-0007).
    manifest: Option<LibraryManifest>,
    /// The owning library's portable graph bytes, encoded while its checked source context is still owned here.
    graph: Vec<u8>,
    /// Session-collected tests; lowering receives their names and Oven receives their outcome policy.
    tests: Vec<crate::test_unit::CollectedTest>,
}

/// Collect and analyze the described unit's source through its compilation session, retaining canonical checked facts.
///
/// Native-only declaration refusals follow analysis; desugaring and the Loaves the program imports from are owned by
/// the session. The declarations the program reaches in its Loaves (DD-0007) come from their `.incnbody` records, and
/// every module lowers against them. Private checked re-export calls and implicit newtype coercions add their exact
/// package targets to the request: their selected owners and hooks need not appear as written source references.
/// The unit's package is its registry package. Every source declaration, including an uncalled method, seeds
/// dependency discovery because all source bodies compile.
///
/// A library unit collects every authored module, checks each with its package identity and builds its manifest
/// from the same pass. Bodies with function or nominal-owner type parameters remain governed; other linked
/// callables name their compiled items.
fn checked_modules(unit: &UnitDescription, rust: &RustFactsSetup) -> Result<CheckedProgram, String> {
    let _invocation = tracing::trace_span!(target: "incan::invocation", "incan_rustc_driver::checked_modules", input = %unit.source.display()).entered();
    let path = unit.source.canonicalize().map_err(|error| error.to_string())?;
    let mut session = CompilationSession::new(unit)
        .and_then(|session| session.with_rust_facts(rust))
        .map_err(|error| error.to_string())?;
    let library = unit.kind == UnitKind::Library;
    let modules = if library {
        collect_library_modules_detailed_with_session(path.clone(), &session)
    } else {
        collect_modules_detailed_with_session(path.clone(), &session)
    }
    .map_err(|failure| failure.render_human())?;
    session
        .resolve_rust_facts(&modules)
        .map_err(|error| error.to_string())?;
    let tests = crate::test_unit::collect(unit, &session)?;
    let package_identity = unit.package.as_str();
    let mut publication = library.then(|| PublicationFacts::new(package_identity));
    let analysis = {
        let _invocation = tracing::trace_span!(target: "incan::invocation", "incan_rustc_driver::analyze_modules", modules = modules.len()).entered();
        session
            .analyze_unit(&modules, library.then_some(package_identity), &mut |module, checker| {
                if let Some(publication) = publication.as_mut() {
                    publication.visit(module, checker, module.file_path == path);
                }
            })
            .map_err(|failure| failure.render_human())?
    };
    let entry = modules
        .iter()
        .position(|module| module.file_path == path)
        .ok_or("entry module is missing")?;
    let manifest = match publication {
        Some(publication) => {
            let manifest = publication
                .library_manifest(&modules, &modules[entry], unit, &session.contract_model_bundles)
                .map_err(|error| error.to_string())?;
            Some(manifest)
        }
        None => None,
    };
    let string_conversion_contract = incan_frontend::body_ir::compilation_string_conversion_contract(
        modules.iter().map(|module| &module.ast),
        session.loaves(),
    )
    .ok_or("unsupported Body IR string conversion contract identity")?;

    // ---- Declarations the program reaches in its Loaves ----
    let requested = required_loaf_identities(&modules, &analysis, package_identity)?;
    let mut bodies = load_loaf_modules(session.loaves(), &requested)?;
    // Loaf declarations retain canonical spans but carry no source text. Do not attribute those spans to the entry.
    let mut files = bodies
        .iter()
        .map(|module| module.module_id.path().to_owned())
        .collect::<Vec<_>>();
    let mut sources = vec![String::new(); bodies.len()];
    let loaf_modules = bodies.len();
    let entry = entry + loaf_modules;

    // ---- The program's own modules ----
    let provider_operations = incan_frontend::body_ir::ProviderOperationCatalog::from_loaves(session.loaves())?;
    for module in &modules {
        let type_info = analysis
            .type_info_for_path(&module.file_path)
            .ok_or("module analysis is missing")?;
        validate_declarations(&module.ast, type_info)?;
        let body_ir = incan_frontend::body_ir::build_body_ir_module(
            &module.ast,
            &module.path_segments,
            type_info,
            &incan_frontend::body_ir::BodyIrContext {
                modules: &bodies,
                provider_operations: &provider_operations,
                registry_package: Some(package_identity),
                library_package: library.then_some(package_identity),
            },
        );
        validate_retained_storage(&module.ast, &body_ir)?;
        bodies.push(body_ir);
        files.push(module.file_path.to_string_lossy().into_owned());
    }
    let graph = if library {
        let records = incan_driver::inspect::codegraph::collect_checked_unit_codegraph(
            unit,
            &session,
            &modules,
            &analysis,
            &bodies[loaf_modules..],
        );
        incan_codegraph::artifact::encode_graph(&records, &unit.project_root).map_err(|error| error.to_string())?
    } else {
        Vec::new()
    };
    sources.extend(modules.into_iter().map(|module| module.source));
    let entry_module = bodies.get_mut(entry).ok_or("selected Body IR entry is missing")?;
    entry_module.string_conversion_contract = string_conversion_contract;
    complete_dependency_context(
        CheckedProgram {
            modules: bodies,
            sources,
            files,
            entry,
            own: loaf_modules,
            manifest,
            graph,
            tests,
        },
        session.loaves(),
        requested,
        package_identity,
    )
}

/// Project exact public package requirements reachable from accepted module facts, including private re-export calls.
/// The current package's declarations compile from this unit and never resolve through a dependency artifact.
/// Trait-wide default inventories are not execution roots: unused adaptor bodies must not instantiate for every
/// adopter. Native comparison bridges add their selected slots from checked Body IR in `complete_dependency_context`.
fn required_loaf_identities(
    modules: &[ParsedModule],
    analysis: &CompilationAnalysis,
    package_identity: &str,
) -> Result<BTreeSet<CanonicalSymbolId>, String> {
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
        .flat_map(|info| {
            info.declarations
                .declaration_identities
                .values()
                .chain(info.declarations.member_declaration_identities.values())
                .cloned()
        });
    let requested = incan_semantics_core::dependencies::CheckedDependencyGraph::from_fact_stores(&facts)
        .reachable_from(roots)
        .into_iter()
        .chain(
            modules
                .iter()
                .filter_map(|module| analysis.type_info_for_path(&module.file_path))
                .flat_map(|info| info.calls.body_ir_reexport_calls.values())
                .map(|call| call.canonical.clone()),
        )
        .chain(
            modules
                .iter()
                .filter_map(|module| analysis.type_info_for_path(&module.file_path))
                .flat_map(|info| info.calls.body_ir_async_call_targets.values().cloned()),
        )
        .chain(
            modules
                .iter()
                .filter_map(|module| analysis.type_info_for_path(&module.file_path))
                .flat_map(|info| info.declarations.body_ir_trait_adoptions.values())
                .flatten()
                .flat_map(|adoption| adoption.methods.iter().flatten().cloned()),
        )
        .chain(
            modules
                .iter()
                .filter_map(|module| analysis.type_info_for_path(&module.file_path))
                .flat_map(|info| info.expressions.validated_newtype_coercions.values())
                .flat_map(|coercion| &coercion.steps)
                .flat_map(|step| step.owner_identity.iter().chain(step.ctor_identity.iter()).cloned()),
        )
        .chain(
            modules
                .iter()
                .filter_map(|module| analysis.type_info_for_path(&module.file_path))
                .flat_map(|info| info.declarations.body_ir_native_type_identities.values().cloned()),
        )
        .chain(
            modules
                .iter()
                .filter_map(|module| analysis.type_info_for_path(&module.file_path))
                .filter_map(incan_frontend::body_ir::registry_description_helper_identity),
        )
        .filter(
            |identity| matches!(&identity.origin, SymbolOrigin::Package { library, .. } if library != package_identity),
        )
        .collect();
    Ok(requested)
}

/// Read only the selected Loaves' authenticated declaration closure; absence never authorizes a synthesized body.
fn load_loaf_modules(loaves: &LoafIndex, requested: &BTreeSet<CanonicalSymbolId>) -> Result<Vec<BodyIrModule>, String> {
    incan_frontend::executable_resolution::resolve_loaf_declarations(loaves, requested)
        .map(|resolved| resolved.modules)
        .map_err(|error| format!("unsupported Body IR Loaf declaration: {error}"))
}

/// Complete the dependency context with the checked comparison slots needed by native bridges, then retain linked
/// contracts and held Rust layouts. Install the sole callable inventory before collecting its native signature
/// carriers. The added defaults introduce no new source checking or invented implementations.
fn complete_dependency_context(
    mut program: CheckedProgram,
    loaves: &LoafIndex,
    mut requested: BTreeSet<CanonicalSymbolId>,
    package_identity: &str,
) -> Result<CheckedProgram, String> {
    let defaults = incan_frontend::body_ir::required_comparison_defaults(&program.modules[program.own..])
        .into_iter()
        .filter(
            |identity| matches!(&identity.origin, SymbolOrigin::Package { library, .. } if library != package_identity),
        )
        .collect::<BTreeSet<_>>();
    if !defaults.is_subset(&requested) {
        requested.extend(defaults);
        let mut expanded = load_loaf_modules(loaves, &requested)?;
        let own = program.modules.split_off(program.own);
        let mut files = program.files.split_off(program.own);
        let mut sources = program.sources.split_off(program.own);
        program.entry = program.entry - program.own + expanded.len();
        program.own = expanded.len();
        program.files = expanded
            .iter()
            .map(|module| module.module_id.path().to_owned())
            .collect();
        program.files.append(&mut files);
        program.sources = vec![String::new(); expanded.len()];
        program.sources.append(&mut sources);
        expanded.extend(own);
        program.modules = expanded;
    }
    let loaf_callables = retain_linked_loaf_declarations(&mut program.modules, program.own);
    program
        .modules
        .get_mut(program.entry)
        .ok_or("selected Body IR entry is missing")?
        .loaf_callables = loaf_callables;
    let opaque = incan_semantics_core::body_ir::held_rust_items(&program.modules);
    let layouts = take_rust_layouts(&mut program.modules)?;
    let native_enums = take_rust_enums(&mut program.modules)?;
    let owners: std::collections::HashSet<_> = layouts
        .iter()
        .map(|declaration| declaration.canonical.clone())
        .chain(native_enums.iter().map(|declaration| declaration.canonical.clone()))
        .collect();
    let held = rust_value_modules(
        layouts
            .into_iter()
            .chain(
                opaque
                    .into_iter()
                    .filter(|declaration| !owners.contains(&declaration.canonical)),
            )
            .collect(),
        native_enums,
    );
    let held_modules = held.len();
    program.files.splice(
        program.own..program.own,
        held.iter().map(|module| module.module_id.path().to_owned()),
    );
    program
        .sources
        .splice(program.own..program.own, held.iter().map(|_| String::new()));
    program.modules.splice(program.own..program.own, held);
    program.entry += held_modules;
    program.own += held_modules;
    let entry_module = program
        .modules
        .get_mut(program.entry)
        .ok_or("selected Body IR entry is missing")?;
    entry_module
        .native_union_declarations
        .extend(native_union_layouts(loaves));
    Ok(program)
}

/// Replace standalone package bodies with linked contracts. Body IR keeps governed templates, including defaults
/// with an open Self receiver, available for checked adopter specialization. Move existing contracts into the
/// returned inventory so joining declaration modules cannot publish the same compiled callable twice.
fn retain_linked_loaf_declarations(modules: &mut [BodyIrModule], linked: usize) -> Vec<LoafCallable> {
    let mut loaf_callables = modules
        .iter_mut()
        .flat_map(|module| std::mem::take(&mut module.loaf_callables))
        .collect::<Vec<_>>();
    for module in modules.iter_mut().take(linked) {
        for declaration in &mut module.nominal_declarations {
            if declaration.native_item.is_empty()
                && let Some(item) = linked_type_item(&declaration.canonical, &declaration.type_parameters)
            {
                declaration.native_item = item;
            }
        }
        for declaration in &mut module.enum_declarations {
            if let Some(item) = linked_type_item(&declaration.canonical, &declaration.type_parameters) {
                declaration.native_item = item;
            }
        }
        let governed = module
            .bodies
            .iter()
            .filter(|body| module.body_requires_instantiation(body))
            .filter_map(|body| body.canonical.clone())
            .collect::<std::collections::BTreeSet<_>>();
        module.bodies.retain(|body| {
            if body
                .canonical
                .as_ref()
                .is_some_and(|identity| governed.contains(identity))
            {
                return true;
            }
            match incan_semantics_core::body_ir::LoafCallable::from_body(body) {
                Some(callable) => {
                    loaf_callables.push(callable);
                    false
                }
                None => true,
            }
        });
    }
    loaf_callables
}

/// Move metadata-selected native layouts to their physical declaration scopes, deduplicating identical selections
/// across source modules. Conflicting layouts cannot authorize construction or projection.
fn take_rust_layouts(modules: &mut [BodyIrModule]) -> Result<Vec<NominalDeclaration>, String> {
    let mut layouts = std::collections::BTreeMap::<String, NominalDeclaration>::new();
    for module in modules {
        for declaration in std::mem::take(&mut module.nominal_declarations) {
            if !matches!(
                declaration.canonical.origin,
                incan_semantics_core::SymbolOrigin::RustCrate(_)
            ) || (declaration.fields.is_empty() && declaration.derives.is_empty())
            {
                module.nominal_declarations.push(declaration);
                continue;
            }
            if let Some(previous) = layouts.get(&declaration.native_item) {
                if previous != &declaration {
                    return Err(format!(
                        "conflicting native field layouts for {}",
                        declaration.native_item
                    ));
                }
            } else {
                layouts.insert(declaration.native_item.clone(), declaration);
            }
        }
    }
    Ok(layouts.into_values().collect())
}

/// Select a native carrier or provider-owned Loaf layout. Generic models and classes publish one parameterized ADT;
/// generic enum and newtype declarations keep their governed source storage until their native templates are supported.
fn linked_type_item(identity: &CanonicalSymbolId, parameters: &[String]) -> Option<String> {
    incan_semantics_core::body_ir::native_nominal_item(identity).or_else(|| {
        (parameters.is_empty()
            || matches!(
                identity.kind,
                SemanticSourceTargetKind::Model | SemanticSourceTargetKind::Class
            ))
        .then(|| incan_semantics_core::body_ir::loaf_item_path(identity))
        .flatten()
    })
}

/// Read each selected Loaf's published native union table, preserving its semantic carrier and physical variant
/// order. The index already selects exact artifacts; the adapter subsequently verifies each layout against metadata.
fn native_union_layouts(
    loaves: &incan_frontend::loaf_index::LoafIndex,
) -> Vec<incan_semantics_core::body_ir::NativeUnionDeclaration> {
    use incan_frontend::library_manifest::resolved_type_from_manifest_type_ref;
    use incan_frontend::typechecker::semantic_type_from_resolved;
    loaves
        .entries()
        .flat_map(|entry| {
            entry
                .artifact
                .manifest
                .contract_metadata
                .native_unions
                .iter()
                .filter_map(|native| {
                    let mut callables = native.closure_members.iter();
                    let members = native
                        .members
                        .iter()
                        .map(|member| {
                            if matches!(member, incan_frontend::library_manifest::TypeRef::Function { .. }) {
                                callables.next().cloned()
                            } else {
                                Some(semantic_type_from_resolved(&resolved_type_from_manifest_type_ref(
                                    member,
                                )))
                            }
                        })
                        .collect::<Option<Vec<_>>>()?;
                    Some(incan_semantics_core::body_ir::NativeUnionDeclaration {
                        ty: native.semantic_type.clone()?,
                        rust_item: format!(
                            "{}::{}",
                            incan_semantics_core::body_ir::loaf_crate_name(&entry.artifact.manifest.name),
                            native.rust_name
                        ),
                        members,
                        closure_members: native.closure_members.clone(),
                        closure_items: native
                            .closure_items
                            .iter()
                            .map(|item| {
                                format!(
                                    "{}::{item}",
                                    incan_semantics_core::body_ir::loaf_crate_name(&entry.artifact.manifest.name),
                                )
                            })
                            .collect(),
                    })
                })
        })
        .collect()
}

/// Declare each Rust item the program holds in a declaration-only module under the item's native module: the
/// lowering lays a value of it out as the item itself.
fn rust_value_modules(
    held: Vec<NominalDeclaration>,
    enums: Vec<incan_semantics_core::body_ir::EnumDeclaration>,
) -> Vec<BodyIrModule> {
    let mut owners = std::collections::BTreeMap::<String, BodyIrModule>::new();
    for declaration in held {
        let Some(owner) = incan_semantics_core::body_ir::native_nominal_module(&declaration.canonical) else {
            continue;
        };
        owners
            .entry(owner.clone())
            .or_insert_with(|| BodyIrModule::declarations_only(owner))
            .nominal_declarations
            .push(declaration);
    }
    for declaration in enums {
        let Some(owner) = incan_semantics_core::body_ir::native_nominal_module(&declaration.canonical) else {
            continue;
        };
        owners
            .entry(owner.clone())
            .or_insert_with(|| BodyIrModule::declarations_only(owner))
            .enum_declarations
            .push(declaration);
    }
    owners.into_values().collect()
}

/// Hoist checked native enum layouts once per foreign owner, refusing conflicting templates from separate modules.
fn take_rust_enums(
    modules: &mut [BodyIrModule],
) -> Result<Vec<incan_semantics_core::body_ir::EnumDeclaration>, String> {
    let mut layouts = std::collections::BTreeMap::new();
    for module in modules {
        for declaration in std::mem::take(&mut module.enum_declarations) {
            if !matches!(
                declaration.canonical.origin,
                incan_semantics_core::SymbolOrigin::RustCrate(_)
            ) {
                module.enum_declarations.push(declaration);
                continue;
            }
            if let Some(previous) = layouts.get(&declaration.native_item) {
                if previous != &declaration {
                    return Err(format!(
                        "conflicting native enum layouts for {}",
                        declaration.native_item
                    ));
                }
            } else {
                layouts.insert(declaration.native_item.clone(), declaration);
            }
        }
    }
    Ok(layouts.into_values().collect())
}

/// Refuse source storage whose checked layout was deliberately omitted from Body IR.
///
/// In particular, a checked newtype constructor must not reach body validation as if it were an ordinary raw wrapper.
/// Transparent rusttypes already passed declaration admission and introduce no physical wrapper to inventory here.
/// The frontend declaration collector owns that decision; this boundary only checks whether its layout was retained.
/// Decorated callable replacements share the storage table but are validated under their original function identity,
/// so they do not count as source `static` declarations.
fn validate_retained_storage(program: &incan_frontend::ast::Program, module: &BodyIrModule) -> Result<(), String> {
    use incan_frontend::ast::Declaration;
    use incan_semantics_core::SemanticSourceTargetKind;

    let unretained = program
        .declarations
        .iter()
        .filter_map(|source| match &source.node {
            Declaration::Static(storage) => Some((storage, source.span)),
            _ => None,
        })
        .filter(|(storage, span)| {
            !module.static_declarations.iter().any(|declaration| {
                declaration.canonical.kind == SemanticSourceTargetKind::Static
                    && incan_semantics_core::canonical_module_identity(&declaration.canonical).as_deref()
                        == Some(module.module_id.path())
                    && storage.name == declaration.canonical.declaration_name
                    && span.start == declaration.canonical.declaration_span.start
                    && span.end == declaration.canonical.declaration_span.end
            })
        })
        .map(|(storage, _)| storage.name.as_str())
        .collect::<Vec<_>>();
    if !unretained.is_empty() {
        return Err(format!(
            "unsupported source Static initializer or carrier on the native route: `{}` in {}",
            unretained.join("`, `"),
            module.module_id.path()
        ));
    }
    let newtype_count = program
        .declarations
        .iter()
        .filter(|declaration| matches!(&declaration.node, Declaration::Newtype(item) if !item.is_rusttype))
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
                if incan_frontend::body_ir::is_direct_native_rusttype_alias(newtype, type_info) =>
            {
                continue;
            }
            Declaration::Newtype(newtype)
                if incan_frontend::body_ir::is_direct_replacement_checked_newtype(newtype, type_info) =>
            {
                validate_generic_adoptions(&newtype.traits, program, type_info)?;
                continue;
            }
            Declaration::Newtype(newtype) => {
                return Err(format!(
                    "unsupported source nonplain Newtype {} on the native route",
                    newtype.name
                ));
            }
            Declaration::Alias(_) => continue,
            Declaration::Partial(_) => continue,
            // Type aliases have no runtime declaration; checked Body IR carries their resolved uses.
            Declaration::TypeAlias(_) => continue,
            // A capability declares authority, not code: consumers reach it through its checked identity.
            Declaration::Capability(_) => continue,
            // An inline test module is compiled by the test runner, never into the production unit.
            Declaration::TestModule(_) => continue,
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
    if has_refused_method_decorator(&model.methods, false) {
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
    if has_refused_method_decorator(&class.methods, false) {
        return "decorated methods".to_owned();
    }

    // ---- Decorator expansion ----
    refused_decorator_feature(&class.decorators).unwrap_or_else(|| "unsupported declaration shape".to_owned())
}

/// Report executable method decorators outside the admitted builtin markers. A trait may also retain a plain
/// `@rust.extern` abstract slot: it has no body or initialization effect, and using it still requires a checked
/// implementation or native intrinsic. Models and classes receive no such exemption.
fn has_refused_method_decorator(
    methods: &[incan_frontend::ast::Spanned<incan_frontend::ast::MethodDecl>],
    allow_abstract_extern: bool,
) -> bool {
    methods.iter().any(|method| {
        method.node.decorators.iter().any(|decorator| {
            if incan_frontend::body_ir::is_direct_replacement_method_decorator(&decorator.node) {
                return false;
            }
            !(allow_abstract_extern
                && method.node.body.is_none()
                && decorator.node.args.is_empty()
                && decorator.node.type_args.is_empty()
                && incan_lang::lang::decorators::from_segments(&decorator.node.path.segments)
                    == Some(incan_lang::lang::decorators::DecoratorId::RustExtern))
        })
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
    // Ordinary markers follow nominal rules; a trait's abstract extern slot carries no executable decorator effect.
    if has_refused_method_decorator(&item.methods, true) {
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
pub(crate) struct Dependencies {
    pub(crate) externs: Vec<(String, PathBuf)>,
    pub(crate) directories: Vec<PathBuf>,
    /// The ABI caches Oven names in the unit description: the toolchain's standard crates and each compiled unit's.
    pub(crate) rust_abi: Vec<PathBuf>,
    /// Compatibility for bootstrap invocations that select a library with `--library`.
    library: bool,
    /// Arguments Oven passes to rustc unchanged (`--rustc-arg`): the pinned target's link policy (DD-0008).
    rustc_args: Vec<String>,
}

impl Dependencies {
    /// The unit's Rust facts: its crates' ABI caches, then rustc's crate metadata for paths they leave uncovered.
    pub(crate) fn rust_facts(&self, sysroot: &Path) -> RustFactsSetup {
        RustFactsSetup {
            caches: self.rust_abi.clone(),
            source: Some(std::sync::Arc::new(crate::rust_facts::RustcFacts {
                sysroot: sysroot.to_path_buf(),
                externs: self.externs.clone(),
                search: self.directories.clone(),
            })),
        }
    }
}

/// Read exact artifacts from Oven's unit description and parse compilation options. Native inputs cannot be
/// overridden by command-line flags, so checking, facts extraction, and compilation share one source.
pub(crate) fn dependencies(unit: &UnitDescription, arguments: &[String]) -> Result<Dependencies, String> {
    let mut names = std::collections::BTreeSet::new();
    let mut externs = Vec::new();
    for native in &unit.externs {
        if !names.insert(&native.name) {
            return Err(format!("unit names more than one exact `{}` artifact", native.name));
        }
        if !native.path.is_file() {
            return Err(format!(
                "unit's `{}` artifact does not exist: {}",
                native.name,
                native.path.display()
            ));
        }
        externs.push((native.name.clone(), native.path.clone()));
    }
    let mut library = false;
    let mut rustc_args = Vec::new();
    let mut remaining = arguments.iter();
    while let Some(option) = remaining.next() {
        if option == "--library" {
            library = true;
            continue;
        }
        let value = remaining.next().ok_or_else(|| format!("missing value for {option}"))?;
        match option.as_str() {
            "--rustc-arg" => rustc_args.push(value.clone()),
            _ => return Err(format!("unknown native source option {option}")),
        }
    }
    Ok(Dependencies {
        externs,
        directories: unit.search.clone(),
        rust_abi: unit.rust_abi.clone(),
        library,
        rustc_args,
    })
}

/// Run source, frontend, canonical Body IR, Incan lowering, plan validation, adapter, and pinned native compilation.
/// Adapter errors cross the executable boundary as their display text so named unsupported gaps retain the CLI's
/// refusal classification instead of appearing as debug-form plan errors.
pub fn compile(arguments: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    if arguments.len() < 4 {
        return Err("usage: --unit UNIT CRATE OUTPUT SYSROOT [--rustc-arg ARG] [--library]".into());
    }

    // ---- Checked source and Incan plan ----
    let mut unit = UnitDescription::read(Path::new(&arguments[0])).map_err(|error| error.message)?;
    let source_path = unit.source.clone();
    let _invocation = tracing::trace_span!(target: "incan::invocation", "incan_rustc_driver::compile_unit", input = %source_path.display()).entered();
    let identity = crate::identity::verify(Path::new(&arguments[3]))?;
    let output = Path::new(&arguments[2]);
    let dependencies = dependencies(&unit, &arguments[4..])?;
    if dependencies.library {
        if unit.kind == UnitKind::Test {
            return Err("a test unit cannot also be a library".into());
        }
        unit.kind = UnitKind::Library;
    }
    let kind = unit.kind;
    let loaves = unit.loaves.clone();
    let loaf_links: std::collections::BTreeSet<String> =
        unit.loaves.iter().map(|loaf| loaf.name.replace('-', "_")).collect();
    let rust = dependencies.rust_facts(Path::new(&arguments[3]));
    let parent = tracing::Span::current();
    let checked = incan_frontend::compiler_stack::run_on_compiler_stack(move || {
        let _parent = parent.enter();
        checked_modules(&unit, &rust)
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
    let mut published_unions = Vec::new();
    let plan = {
        tracing::debug!(target: "incan::invocation", modules = ?program.modules, "checked direct-route Body IR");
        let _invocation = tracing::trace_span!(target: "incan::invocation", "incan_rustc_driver::lower_program", modules = program.modules.len()).entered();
        let entry = program.entry.try_into()?;
        unit_plan(
            &program.modules,
            program.sources,
            program.files,
            entry,
            kind.as_str().to_owned(),
            program.tests.iter().map(|test| test.name.clone()).collect(),
            &mut published_unions,
        )?
    };
    let Some(plan) = plan else {
        report_missing_entrypoint(&source_path)?;
        return Err(SourceFailure::MissingEntrypoint.into());
    };
    let tests = crate::test_unit::report(&program.tests, &plan.tests)?;

    // ---- Exact native dependencies, including checked Loaf implementation layouts ----
    let native_externs = crate::native_crates::link_externs(&plan, &dependencies.externs, &loaves)?;
    crate::adapter::compile(
        plan,
        &arguments[1],
        Path::new(&arguments[2]),
        &identity,
        &native_externs,
        &dependencies.directories,
        &dependencies.rustc_args,
        kind,
    )
    .map_err(|error| error.to_string())?;
    if kind == UnitKind::Test {
        std::fs::write(output.with_extension("tests.json"), serde_json::to_vec(&tests)?)?;
    }

    // ---- The Loaf beside a library unit's rlib (DD-0007) ----
    if let Some(mut manifest) = program.manifest {
        publish_native_unions(&mut manifest, published_unions).map_err(|error| error.to_string())?;
        let directory = output.parent().ok_or("library output has no directory")?;
        let closure = LoafNativeClosure {
            externs: native_externs,
            search: dependencies.directories.clone(),
            macro_externs: macro_externs(&manifest, &dependencies.externs, &loaf_links),
        };
        write_loaf(
            directory,
            manifest,
            &program.modules[program.own..],
            &closure,
            &program.graph,
        )
        .map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// The crates a consumer must name directly when this library exports attribute macros, else none.
///
/// rustc expands an attribute macro in the crate that applies it, and the macro's output names the exporting library's
/// own Rust crates. Those are the library's links except its Loaf links, which a consumer binds as Loaves; the
/// toolchain's crates are already bound for every unit and are left out by the consumer's Oven.
fn macro_externs(
    manifest: &LibraryManifest,
    externs: &[(String, PathBuf)],
    loaf_links: &BTreeSet<String>,
) -> Vec<(String, PathBuf)> {
    let exports_attribute_macro = manifest.contract_metadata.api.as_ref().is_some_and(|api| {
        api.modules
            .iter()
            .flat_map(|module| &module.declarations)
            .any(|declaration| {
                matches!(declaration, incan_frontend::api_metadata::ApiDeclaration::Function(function)
                    if function.native_attribute.is_some())
            })
    });
    if !exports_attribute_macro {
        return Vec::new();
    }
    externs
        .iter()
        .filter(|(name, _)| !loaf_links.contains(name))
        .cloned()
        .collect()
}
