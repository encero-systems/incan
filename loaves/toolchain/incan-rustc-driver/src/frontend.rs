//! Shared CLI-session source-to-Body-IR glue. All Body-IR-to-plan decisions belong to the Incan lowering Loaf.

use std::path::{Path, PathBuf};

use incan_driver::{modules::collect_modules_detailed_with_session, session::CompilationSession};
use incan_mir_lowering::caller::incan::{lower_program, program_refusals};

/// Environment variable naming the file where the census receives every refusal of one compilation, one per line.
///
/// Unset, compilation reports only its first refusal, as it always has. Set, the driver also lowers every body on its
/// own and writes each body's first refusal there before compiling as usual, so one refused body cannot hide another.
const REFUSAL_REPORT: &str = "INCAN_DIRECT_REFUSAL_REPORT";

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
use incan_semantics_core::body_ir::BodyIrModule;

/// Checked owning modules and their source provenance, with one explicit native entry module.
struct CheckedProgram {
    modules: Vec<BodyIrModule>,
    sources: Vec<String>,
    files: Vec<String>,
    entry: usize,
}

/// Collect and analyze the original source through the CLI's compilation session, retaining canonical checked facts.
/// Native-only declaration refusals follow analysis; provider activation, desugaring, and stdlib loading remain owned
/// by the session.
fn checked_modules(path: &Path) -> Result<CheckedProgram, String> {
    let _invocation = tracing::trace_span!(target: "incan::invocation", "incan_rustc_driver::checked_modules", input = %path.display()).entered();
    let path = path.canonicalize().map_err(|error| error.to_string())?;
    let session = CompilationSession::discover_with_feature_selection(&path, &Default::default())
        .map_err(|error| error.to_string())?;
    let modules =
        collect_modules_detailed_with_session(path.clone(), &session).map_err(|failure| failure.render_human())?;
    let inspection = session
        .prepare_check_rust_inspection(&path, &modules)
        .map_err(|error| error.to_string())?;
    let analysis = {
        let _invocation = tracing::trace_span!(target: "incan::invocation", "incan_rustc_driver::analyze_modules", modules = modules.len()).entered();
        session
            .analyze_modules(&modules, inspection.as_ref().map(|workspace| workspace.manifest_dir()))
            .map_err(|failure| failure.render_human())?
    };
    let entry = modules
        .iter()
        .position(|module| module.file_path == path)
        .ok_or("entry module is missing")?;
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
        .filter(|identity| match &identity.origin {
            incan_semantics_core::SymbolOrigin::Package { library, .. } => !session
                .provider_plan
                .active_sdk_records()
                .any(|provider| &provider.identity.name == library),
            _ => false,
        })
        .collect();
    let resolved =
        incan_frontend::executable_resolution::resolve_executable_requirements(&session.provider_plan, &required)
            .map_err(|error| format!("unsupported Body IR package executable representation: {error}"))?;
    let mut bodies = resolved.modules;

    // ---- Stdlib declarations published by the linked SDK packages ----
    // They are context for every module's Body IR, so stdlib enums, newtypes and layouts resolve by identity as package
    // declarations do, and declaration-only program modules the lowering names by their crate items.
    let mut library_identities = std::collections::BTreeSet::new();
    for module in &modules {
        let type_info = analysis
            .type_info_for_path(&module.file_path)
            .ok_or("module analysis is missing")?;
        library_identities.extend(incan_frontend::body_ir::library_type_identities(type_info));
    }
    bodies.extend(published_library_modules(&session.provider_plan, &library_identities));
    // Published fragments retain canonical spans but do not carry source text. Do not attribute those spans to the
    // entry.
    let mut files = bodies
        .iter()
        .map(|module| module.module_id.path().to_owned())
        .collect::<Vec<_>>();
    let mut sources = vec![String::new(); bodies.len()];
    let mut entry = entry + bodies.len();
    let mut library_layouts = Vec::new();
    let mut library_calls = Vec::new();
    for module in modules {
        let type_info = analysis
            .type_info_for_path(&module.file_path)
            .ok_or("module analysis is missing")?;
        validate_declarations(&module.ast, type_info)?;
        let body_ir = incan_frontend::body_ir::build_body_ir_module_v0_with_executable_context(
            &module.ast,
            &module.path_segments,
            type_info,
            &bodies,
        );
        validate_retained_storage(&module.ast, &body_ir)?;
        library_layouts.extend(incan_frontend::body_ir::library_nominal_declarations(type_info));
        library_calls.extend(incan_frontend::body_ir::library_call_delegations(type_info));
        bodies.push(body_ir);
        sources.push(module.source);
        files.push(module.file_path.to_string_lossy().into_owned());
    }
    // ---- Stdlib trait defaults the program's adopters take, from the SDK's published representation ----
    for module in published_standard_defaults(&session.provider_plan, &bodies) {
        carry_published_module(module, &mut bodies, &mut files, &mut sources, &mut entry);
    }
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
    let libraries = library_modules(library_layouts, library_calls);
    let entry = entry + libraries.len();
    files.splice(0..0, libraries.iter().map(|module| module.module_id.path().to_owned()));
    sources.splice(0..0, libraries.iter().map(|_| String::new()));
    bodies.splice(0..0, libraries);
    Ok(CheckedProgram {
        modules: bodies,
        sources,
        files,
        entry,
    })
}

/// Carry the stdlib layouts and crate calls the checked modules use into lowering as declaration-only modules, one per
/// declaring stdlib module, so each layout validates against its own module identity as a published package layout
/// does. Each layout and call travels once; whether a layout is linked rather than declared is the lowering's decision.
fn library_modules(
    layouts: Vec<incan_semantics_core::body_ir::NominalDeclaration>,
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
    owners.into_values().collect()
}

/// Resolve the stdlib types the program uses from the representations its linked SDK packages published, one
/// declaration-only module per stdlib module.
///
/// Each identity resolves on its own, so a type a package does not publish, or one that is not an SDK type at all,
/// is simply absent and keeps its source record or refusal. Only declaration records are kept: layouts, enums,
/// constants and aliases. Published bodies stay out, because the program calls the compiled items instead.
fn published_library_modules(
    plan: &incan_frontend::provider::ProviderPlan,
    identities: &std::collections::BTreeSet<incan_semantics_core::CanonicalSymbolId>,
) -> Vec<BodyIrModule> {
    use std::collections::BTreeSet;
    let mut owners = std::collections::BTreeMap::<String, BodyIrModule>::new();
    let (mut nominals, mut enums, mut fieldless, mut values) =
        (BTreeSet::new(), BTreeSet::new(), BTreeSet::new(), BTreeSet::new());
    let (mut constants, mut aliases) = (BTreeSet::new(), BTreeSet::new());
    for identity in identities {
        let required = std::iter::once(identity.clone()).collect();
        let Ok(resolved) = incan_frontend::executable_resolution::resolve_sdk_requirements(plan, &required) else {
            continue;
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
        }
    }
    owners.into_values().collect()
}

/// Resolve the bodies the linked SDK packages publish for the standard-library trait defaults the program's
/// implementation records name, one module per declaring stdlib module.
///
/// A record whose implementation is the slot itself takes the protocol's default. Only the defaults the program calls
/// are taken, so a default nothing calls is never lowered. Each resolves on its own, with its published closure (the
/// trait around a slot it calls, for one), so a default the SDK does not publish is simply absent: its record stays,
/// and a call to it refuses where it is lowered.
fn published_standard_defaults(
    plan: &incan_frontend::provider::ProviderPlan,
    modules: &[BodyIrModule],
) -> Vec<BodyIrModule> {
    let mut called = std::collections::BTreeSet::new();
    for body in modules.iter().flat_map(|module| &module.bodies) {
        collect_called_methods(&body.block.stmts, &mut called);
    }
    let defaults = modules
        .iter()
        .flat_map(|module| &module.trait_implementations)
        .filter(|record| record.method == record.implementation && called.contains(&record.method))
        .filter(|record| {
            matches!(&record.implementation.origin, incan_semantics_core::SymbolOrigin::Package { library, .. }
                if plan.active_sdk_records().any(|provider| &provider.identity.name == library))
        })
        .map(|record| record.implementation.clone())
        .collect::<std::collections::BTreeSet<_>>();
    let mut owners = std::collections::BTreeMap::<String, BodyIrModule>::new();
    for identity in defaults {
        let required = std::iter::once(identity).collect();
        let Ok(resolved) = incan_frontend::executable_resolution::resolve_sdk_requirements(plan, &required) else {
            continue;
        };
        for module in resolved.modules {
            let path = module.module_id.path().to_owned();
            let owner = owners.entry(path.clone()).or_insert_with(|| declaration_module(path));
            join_published_module(owner, module);
        }
    }
    owners.into_values().collect()
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

/// Join one resolved published module into the module carried for the same stdlib owner, each declaration and body
/// once by its canonical identity.
fn join_published_module(owner: &mut BodyIrModule, module: BodyIrModule) {
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
        constant_declarations: Vec::new(),
        type_alias_declarations: Vec::new(),
        enum_declarations: Vec::new(),
        stdlib_delegations: Vec::new(),
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
    let owner = incan_semantics_core::canonical_module_identity(identity)?;
    if !carried.insert(identity.clone()) {
        return None;
    }
    Some(owners.entry(owner.clone()).or_insert_with(|| declaration_module(owner)))
}

/// Refuse source storage whose checked layout was deliberately omitted from Body IR.
///
/// In particular, a checked newtype constructor must not reach body validation as if it were an ordinary raw wrapper.
/// The frontend declaration collector owns that decision; this boundary only checks whether its layout was retained.
fn validate_retained_storage(program: &incan_frontend::ast::Program, module: &BodyIrModule) -> Result<(), String> {
    use incan_frontend::ast::Declaration;
    use incan_semantics_core::SemanticSourceTargetKind;

    let static_count = program
        .declarations
        .iter()
        .filter(|declaration| matches!(declaration.node, Declaration::Static(_)))
        .count();
    if static_count != module.static_declarations.len() {
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
    // Legacy's `incan_ir::check_for_this_import` injects entrypoint output for this exact module import.
    // Until Body IR carries that effect, accepting the declaration would silently erase observable behavior.
    for declaration in &program.declarations {
        if let incan_frontend::ast::Declaration::Import(import) = &declaration.node
            && let incan_frontend::ast::ImportKind::Module(path) = &import.kind
            && path.segments.len() == 1
            && path.segments[0] == "this"
        {
            return Err("unsupported source import this entrypoint effect on the native route".to_owned());
        }
    }
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
            Declaration::Newtype(newtype) if incan_frontend::body_ir::is_direct_replacement_plain_newtype(newtype) => {
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
    if !incan_frontend::body_ir::has_direct_replacement_properties(&model.type_params, &model.properties) {
        return "properties".to_owned();
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
    let open_features = [
        !class.type_params.is_empty(),
        class.traits.iter().any(|adoption| !adoption.node.type_args.is_empty()),
        !class.method_aliases.is_empty(),
    ];
    if open_features.iter().filter(|present| **present).count() > 1 {
        return "type parameters, generic trait adoption and method aliases combined".to_owned();
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
    if !incan_frontend::body_ir::has_direct_replacement_properties(&class.type_params, &class.properties) {
        return "properties".to_owned();
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

/// Name the first trait feature the direct route cannot retain.
///
/// Type parameters and supertraits declared in this module are retained: Body IR records each adopter's checked
/// instantiation and the supertrait slots it fills. A method's own type parameters are retained like any method's:
/// each checked call schedules the adopter's default at its call-site arguments. A supertrait from elsewhere adds no
/// declaration behavior of its own: the checker records the serde derives a standard JSON supertrait implies, and a
/// call of one of its slots that has no retained body refuses by name where it is lowered.
fn trait_refusal_feature(item: &incan_frontend::ast::TraitDecl) -> Option<&'static str> {
    if !item.decorators.is_empty() {
        return Some("decorators");
    }
    if !item.method_aliases.is_empty() {
        return Some("method aliases");
    }
    if !item.method_partials.is_empty() {
        return Some("method partials");
    }
    if !item.properties.is_empty() {
        return Some("properties");
    }
    if item.methods.iter().any(|method| !method.node.decorators.is_empty()) {
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
}

/// Parse declared native externs and search directories; no ambient crate discovery is performed.
fn dependencies(arguments: &[String]) -> Result<Dependencies, String> {
    let mut externs = Vec::new();
    let mut directories = Vec::new();
    let mut remaining = arguments.iter();
    while let Some(option) = remaining.next() {
        let value = remaining.next().ok_or_else(|| format!("missing value for {option}"))?;
        match option.as_str() {
            "--extern" => {
                let (name, path) = value.split_once('=').ok_or("--extern requires CRATE=RLIB")?;
                externs.push((name.to_owned(), PathBuf::from(path)));
            }
            "--search" => directories.push(PathBuf::from(value)),
            _ => return Err(format!("unknown native source option {option}")),
        }
    }
    Ok(Dependencies { externs, directories })
}

/// Run source, frontend, canonical Body IR, Incan lowering, plan validation, adapter, and pinned native compilation.
pub fn compile(arguments: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    if arguments.len() < 4 {
        return Err("usage: --source SOURCE CRATE OUTPUT SYSROOT [--extern CRATE=RLIB] [--search DIR]".into());
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
    let parent = tracing::Span::current();
    let checked = incan_frontend::compiler_stack::run_on_compiler_stack(move || {
        let _parent = parent.enter();
        checked_modules(&checked_source)
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
        let _invocation = tracing::trace_span!(target: "incan::invocation", "incan_rustc_driver::lower_program", modules = program.modules.len()).entered();
        lower_program(
            &program.modules,
            program.sources,
            program.files,
            program.entry.try_into()?,
        )?
    };

    // ---- Explicit native dependencies ----
    let library_items = plan
        .models
        .iter()
        .map(|model| &model.library_item)
        .chain(plan.enums.iter().map(|value| &value.library_item))
        .filter(|item| !item.is_empty());
    for item in library_items {
        let root = item.split("::").next().unwrap_or_default();
        if !dependencies.externs.iter().any(|(name, _)| name == root) {
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
    )?;
    if let Some(reuse) = reuse {
        if crate::native_link(&identity)?.1 != link_identity {
            return Err("native link inputs changed during compilation; completed output was not published".into());
        }
        reuse.publish()?
    }
    Ok(())
}
