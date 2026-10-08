//! Shared CLI-session source-to-Body-IR glue. All Body-IR-to-plan decisions belong to the Incan lowering Loaf.

use std::path::{Path, PathBuf};

use incan_driver::{modules::collect_modules_detailed_with_session, session::CompilationSession};
use incan_mir_lowering::caller::incan::lower_program;
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
    let path = path.canonicalize().map_err(|error| error.to_string())?;
    let session = CompilationSession::discover_with_feature_selection(&path, &Default::default())
        .map_err(|error| error.to_string())?;
    let modules =
        collect_modules_detailed_with_session(path.clone(), &session).map_err(|failure| failure.render_human())?;
    let inspection = session
        .prepare_check_rust_inspection(&path, &modules)
        .map_err(|error| error.to_string())?;
    let analysis = session
        .analyze_modules(&modules, inspection.as_ref().map(|workspace| workspace.manifest_dir()))
        .map_err(|failure| failure.render_human())?;
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
    // Published fragments retain canonical spans but do not carry source text. Do not attribute those spans to the
    // entry.
    let mut files = bodies
        .iter()
        .map(|module| module.module_id.path().to_owned())
        .collect::<Vec<_>>();
    let mut sources = vec![String::new(); bodies.len()];
    let entry = entry + bodies.len();
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
        bodies.push(body_ir);
        sources.push(module.source);
        files.push(module.file_path.to_string_lossy().into_owned());
    }
    Ok(CheckedProgram {
        modules: bodies,
        sources,
        files,
        entry,
    })
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
                if type_info.declarations.model_derives.get(&model.name).is_some_and(|names| names.iter().any(|name| name.starts_with("serde::")))
                    && model.fields.iter().any(|field| field.node.metadata.alias.is_some())
                {
                    return Err(format!("unsupported source Model serde field aliases on {} on the native route", model.name));
                }
                if !incan_frontend::body_ir::is_direct_replacement_checked_model(model, type_info) {
                    return Err(format!(
                        "unsupported source Model {} on {} on the native route",
                        model_refusal_feature(model), model.name
                    ));
                }
                if model.fields.iter().any(|field| field.node.default.is_some())
                    && type_info.declarations.model_derives.get(&model.name).is_some_and(|derives| derives.iter().any(|derive| derive == "Default"))
                {
                    return Err(format!("unsupported source Model derived Default over field defaults on {} on the native route", model.name));
                }
                validate_generic_adoptions(&model.traits, program)?;
                continue;
            }
            Declaration::Class(class) => {
                if !incan_frontend::body_ir::is_direct_replacement_class(class) {
                    return Err(format!(
                        "unsupported source Class inheritance, type parameters, decorators, aliases, properties or defaults on {} on the native route",
                        class.name
                    ));
                }
                validate_generic_adoptions(&class.traits, program)?;
                continue;
            }
            Declaration::Enum(value) => {
                if !incan_frontend::body_ir::is_direct_native_enum(value) {
                    return Err(format!("unsupported source Enum {} on the native route", value.name));
                }
                continue;
            }
            Declaration::Trait(item) => {
                if let Some(feature) = trait_refusal_feature(item, program) {
                    return Err(format!("unsupported source Trait {feature} on the native route"));
                }
                continue;
            }
            Declaration::Newtype(newtype) if incan_frontend::body_ir::is_direct_replacement_plain_newtype(newtype) => {
                validate_generic_adoptions(&newtype.traits, program)?;
                continue;
            }
            Declaration::Newtype(_) => "nonplain Newtype",
            Declaration::Alias(_) => continue,
            Declaration::Partial(_) => "Partial",
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

    // ---- Structural declaration features ----
    if !model.type_params.is_empty() {
        return "type parameters".to_owned();
    }

    // ---- Member binding features ----
    if !model.method_aliases.is_empty() {
        return "method aliases".to_owned();
    }
    if !model.method_partials.is_empty() {
        return "method partials".to_owned();
    }
    if !model.properties.is_empty() {
        return "properties".to_owned();
    }

    // ---- Method facts ----
    if model.methods.iter().any(|method| !method.node.type_params.is_empty() || !method.node.decorators.is_empty()) {
        return "generic or decorated methods".to_owned();
    }

    // ---- Decorator expansion ----
    if let Some(decorator) = model.decorators.iter().find(|decorator| !incan_frontend::body_ir::is_direct_replacement_model_derive(&decorator.node)) {
        let arguments = decorator.node.args.iter().filter_map(|argument| {
            match argument {
                incan_frontend::ast::DecoratorArg::Positional(value) => match &value.node {
                    incan_frontend::ast::Expr::Ident(name) => Some(name.as_str()),
                    _ => None,
                },
                _ => None,
            }
        }).collect::<Vec<_>>().join(", ");
        return format!("decorator @{}({arguments})", decorator.node.name);
    }
    "unsupported declaration shape".to_owned()
}

/// Name the first trait feature the direct route cannot retain.
///
/// Type parameters and supertraits declared in this module are retained: Body IR records each adopter's checked
/// instantiation and the supertrait slots it fills. A supertrait from elsewhere is an obligation whose implementation
/// facts Body IR does not carry, so it stays refused.
fn trait_refusal_feature(item: &incan_frontend::ast::TraitDecl, program: &incan_frontend::ast::Program) -> Option<&'static str> {
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
    if item.methods.iter().any(|method| !method.node.type_params.is_empty()) {
        return Some("generic methods");
    }
    if item.methods.iter().any(|method| !method.node.decorators.is_empty()) {
        return Some("decorated methods");
    }
    if item.traits.iter().any(|supertrait| !declares_trait(program, &supertrait.node.name)) {
        return Some("imported supertraits");
    }
    None
}

/// Whether this module declares a trait under `name`.
fn declares_trait(program: &incan_frontend::ast::Program, name: &str) -> bool {
    program.declarations.iter().any(|declaration| {
        matches!(&declaration.node, incan_frontend::ast::Declaration::Trait(item) if item.name == name)
    })
}

/// Refuse a generic adoption of a trait this module does not declare.
///
/// Body IR instantiates only source-local trait defaults; an imported generic trait's implementation facts are not
/// retained, so admitting the adoption could lose behavior.
fn validate_generic_adoptions(
    adoptions: &[incan_frontend::ast::Spanned<incan_frontend::ast::TraitBound>],
    program: &incan_frontend::ast::Program,
) -> Result<(), String> {
    if adoptions
        .iter()
        .any(|adoption| !adoption.node.type_args.is_empty() && !declares_trait(program, &adoption.node.name))
    {
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
    let program = incan_frontend::compiler_stack::run_on_compiler_stack(move || checked_modules(&source_path))?;
    let plan = lower_program(
        &program.modules,
        program.sources,
        program.files,
        program.entry.try_into()?,
    )?;

    // ---- Explicit native dependencies ----
    let dependencies = dependencies(&arguments[4..])?;
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
    )?;
    Ok(())
}
