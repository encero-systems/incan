//! Shared CLI-session source-to-Body-IR glue. All Body-IR-to-plan decisions belong to the Incan lowering Loaf.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use incan_driver::loaf_publication::{PublicationFacts, write_loaf};
use incan_driver::modules::{collect_library_modules_detailed_with_session, collect_modules_detailed_with_session};
use incan_driver::session::CompilationSession;
use incan_driver::unit::UnitDescription;
use incan_frontend::ast::{DecoratorArg, Expr, Literal};
use incan_frontend::library_manifest::LibraryManifest;
use incan_frontend::loaf_index::LoafNativeClosure;
use incan_mir_lowering::caller::incan::{library_plan, program_plan, program_refusals};
use incan_semantics_core::body_ir::{BodyIrModule, NominalDeclaration};
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
}

/// Collect and analyze the described unit's source through its compilation session, retaining canonical checked facts.
///
/// Native-only declaration refusals follow analysis; desugaring and the Loaves the program imports from are owned by
/// the session. The declarations the program reaches in its Loaves (DD-0007) come from their `.incnbody` records, and
/// every module lowers against them. Private checked re-export calls add their exact package targets to the request
/// without changing the ordinary reference graph. The unit's package is its registry package.
///
/// A library unit collects every authored module, checks each with its package identity and builds its manifest
/// from the same pass.
fn checked_modules(unit: &UnitDescription, library: bool, rust: &RustFactsInputs) -> Result<CheckedProgram, String> {
    let _invocation = tracing::trace_span!(target: "incan::invocation", "incan_rustc_driver::checked_modules", input = %unit.source.display()).entered();
    let path = unit.source.canonicalize().map_err(|error| error.to_string())?;
    let mut session = CompilationSession::new(unit).map_err(|error| error.to_string())?;
    let modules = if library {
        collect_library_modules_detailed_with_session(path.clone(), &session)
    } else {
        collect_modules_detailed_with_session(path.clone(), &session)
    }
    .map_err(|failure| failure.render_human())?;
    add_rust_facts(&mut session, &modules, rust)?;
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
                .library_manifest(
                    &modules,
                    &modules[entry],
                    &unit.version,
                    &session.contract_model_bundles,
                )
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
        // A library unit's own declarations carry its package identity; they compile here, never from a Loaf.
        .filter(
            |identity| matches!(&identity.origin, SymbolOrigin::Package { library, .. } if library != package_identity),
        )
        .collect();
    let mut bodies = incan_frontend::executable_resolution::resolve_loaf_declarations(session.loaves(), &requested)
        .map_err(|error| format!("unsupported Body IR Loaf declaration: {error}"))?
        .modules;
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
    for module in modules {
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
        sources.push(module.source);
        files.push(module.file_path.to_string_lossy().into_owned());
    }
    // ---- Loaf declarations: named, not compiled here (DD-0007) ----
    // A Loaf's layouts name its compiled items, and a non-generic Loaf body is its declaration record: the program
    // calls the Loaf's compiled item. A governed body, one with type parameters, stays and is instantiated where it
    // is called.
    let mut loaf_callables = Vec::new();
    for module in bodies.iter_mut().take(loaf_modules) {
        for declaration in &mut module.nominal_declarations {
            if declaration.native_item.is_empty()
                && let Some(item) = incan_semantics_core::body_ir::loaf_item_path(&declaration.canonical)
            {
                declaration.native_item = item;
            }
        }
        for declaration in &mut module.enum_declarations {
            if let Some(item) = incan_semantics_core::body_ir::loaf_item_path(&declaration.canonical) {
                declaration.native_item = item;
            }
        }
        module.bodies.retain(|body| {
            if !body.type_parameters.is_empty() {
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
    // ---- Rust values the program holds: declaration-only modules under each item's native module (DD-0006) ----
    let held = rust_value_modules(incan_semantics_core::body_ir::held_rust_items(&bodies));
    let held_modules = held.len();
    files.splice(loaf_modules..loaf_modules, held.iter().map(|module| module.module_id.path().to_owned()));
    sources.splice(loaf_modules..loaf_modules, held.iter().map(|_| String::new()));
    bodies.splice(loaf_modules..loaf_modules, held);
    let entry = entry + held_modules;
    let entry_module = bodies.get_mut(entry).ok_or("selected Body IR entry is missing")?;
    entry_module.string_conversion_contract = string_conversion_contract;
    entry_module.loaf_callables = loaf_callables;
    Ok(CheckedProgram {
        modules: bodies,
        sources,
        files,
        entry,
        own: loaf_modules + held_modules,
        manifest,
    })
}

/// The crates and sysroot a unit's Rust facts are read from: the unit's own native closure.
struct RustFactsInputs {
    sysroot: PathBuf,
    externs: Vec<(String, PathBuf)>,
    search: Vec<PathBuf>,
}

/// Read Rust facts for the unit's `rust::` imports from rustc's crate metadata (H9) and add them to the session.
fn add_rust_facts(
    session: &mut CompilationSession,
    modules: &[incan_frontend::parsed_module::ParsedModule],
    rust: &RustFactsInputs,
) -> Result<(), String> {
    let paths = imported_rust_paths(modules);
    if paths.is_empty() {
        return Ok(());
    }
    let started = std::time::Instant::now();
    let facts = crate::rust_facts::extract(crate::rust_facts::RustFactsRequest {
        sysroot: &rust.sysroot,
        externs: &rust.externs,
        search: &rust.search,
        paths,
        depth: 1,
    })?;
    if std::env::var_os("INCAN_TRACE_RUST_FACTS").is_some() {
        eprintln!("rust facts: {} items in {:?}", facts.len(), started.elapsed());
        if let Some(directory) = std::env::var_os("INCAN_TRACE_RUST_FACTS_DIR") {
            let path = PathBuf::from(directory).join("rust-facts.json");
            let _ = std::fs::write(&path, serde_json::to_vec_pretty(&facts).unwrap_or_default());
        }
    }
    session.add_rust_facts(facts);
    Ok(())
}

/// The Rust paths a unit's modules import (`rust::…`), which its checking asks Rust facts for.
fn imported_rust_paths(modules: &[incan_frontend::parsed_module::ParsedModule]) -> BTreeSet<String> {
    use incan_frontend::ast::{Declaration, ImportKind};
    let mut paths = BTreeSet::new();
    for module in modules {
        for declaration in &module.ast.declarations {
            let Declaration::Import(import) = &declaration.node else {
                continue;
            };
            match &import.kind {
                ImportKind::RustCrate { crate_name, path, .. } => {
                    paths.insert(std::iter::once(crate_name.clone()).chain(path.iter().cloned()).collect::<Vec<_>>().join("::"));
                }
                ImportKind::RustFrom {
                    crate_name, path, items, ..
                } => {
                    for item in items {
                        let segments = std::iter::once(crate_name.clone())
                            .chain(path.iter().cloned())
                            .chain(std::iter::once(item.name.clone()));
                        paths.insert(segments.collect::<Vec<_>>().join("::"));
                    }
                }
                _ => {}
            }
        }
    }
    paths
}

/// Declare each Rust item the program holds in a declaration-only module under the item's native module: the
/// lowering lays a value of it out as the item itself.
fn rust_value_modules(held: Vec<NominalDeclaration>) -> Vec<BodyIrModule> {
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
    owners.into_values().collect()
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
    /// Compile a library unit (`--library`): an rlib with the entry module's public names, no `main` required.
    library: bool,
    /// Arguments the Oven passes to rustc unchanged (`--rustc-arg`): the pinned target's link policy (DD-0008).
    rustc_args: Vec<String>,
}

/// Parse declared native externs, search directories and rustc arguments; no ambient crate discovery is performed.
fn dependencies(arguments: &[String]) -> Result<Dependencies, String> {
    let mut externs = Vec::new();
    let mut directories = Vec::new();
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
            "--extern" => {
                let (name, path) = value.split_once('=').ok_or("--extern requires CRATE=RLIB")?;
                externs.push((name.to_owned(), PathBuf::from(path)));
            }
            "--search" => directories.push(PathBuf::from(value)),
            "--rustc-arg" => rustc_args.push(value.clone()),
            _ => return Err(format!("unknown native source option {option}")),
        }
    }
    Ok(Dependencies {
        externs,
        directories,
        library,
        rustc_args,
    })
}

/// Run source, frontend, canonical Body IR, Incan lowering, plan validation, adapter, and pinned native compilation.
/// Adapter errors cross the executable boundary as their display text so named unsupported gaps retain the CLI's
/// refusal classification instead of appearing as debug-form plan errors.
pub fn compile(arguments: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    if arguments.len() < 4 {
        return Err(
            "usage: --unit UNIT CRATE OUTPUT SYSROOT [--extern CRATE=RLIB] [--search DIR] [--rustc-arg ARG] [--library]"
                .into(),
        );
    }

    // ---- Checked source and Incan plan ----
    let unit = UnitDescription::read(Path::new(&arguments[0])).map_err(|error| error.message)?;
    let source_path = unit.source.clone();
    let _invocation = tracing::trace_span!(target: "incan::invocation", "incan_rustc_driver::compile_unit", input = %source_path.display()).entered();
    crate::identity::verify(Path::new(&arguments[3]))?;
    let output = Path::new(&arguments[2]);
    let dependencies = dependencies(&arguments[4..])?;
    let checked_library = dependencies.library;
    let rust = RustFactsInputs {
        sysroot: PathBuf::from(&arguments[3]),
        externs: dependencies.externs.clone(),
        search: dependencies.directories.clone(),
    };
    let parent = tracing::Span::current();
    let checked = incan_frontend::compiler_stack::run_on_compiler_stack(move || {
        let _parent = parent.enter();
        checked_modules(&unit, checked_library, &rust)
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
            return Err(format!("unsupported Body IR native dependency `{root}` for Loaf layout `{item}`").into());
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
        &dependencies.rustc_args,
        dependencies.library,
    )
    .map_err(|error| error.to_string())?;

    // ---- The Loaf beside a library unit's rlib (DD-0007) ----
    if let Some(manifest) = program.manifest {
        let directory = output.parent().ok_or("library output has no directory")?;
        let closure = LoafNativeClosure {
            externs: dependencies.externs.clone(),
            search: dependencies.directories.clone(),
        };
        write_loaf(directory, manifest, &program.modules[program.own..], &closure)
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}
