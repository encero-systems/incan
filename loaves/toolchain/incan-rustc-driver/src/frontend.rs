//! Shared CLI-session source-to-Body-IR glue. All Body-IR-to-plan decisions belong to the Incan lowering Loaf.

use std::path::{Path, PathBuf};

use crate::plan::{Plan, PlanType};
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
    // Published executable fragments lack original source text for the adapter diagnostic source map.
    // Refuse before planning rather than treating their canonical module identities as filesystem paths.
    if resolved.modules.iter().any(|module| !module.bodies.is_empty()) {
        return Err("unsupported Body IR published executable source provenance".to_owned());
    }
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
        validate_declarations(&module.ast)?;
        let type_info = analysis
            .type_info_for_path(&module.file_path)
            .ok_or("module analysis is missing")?;
        let body_ir = incan_frontend::body_ir::build_body_ir_module_v0_with_executable_context(
            &module.ast,
            &module.path_segments,
            type_info,
            &bodies,
        );
        let static_count = module
            .ast
            .declarations
            .iter()
            .filter(|declaration| matches!(declaration.node, incan_frontend::ast::Declaration::Static(_)))
            .count();
        if static_count != body_ir.static_declarations.len() {
            return Err("unsupported source Static initializer or carrier on the native route".to_owned());
        }
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

/// Refuse declarations whose observable behavior the direct route cannot retain, independently for each module.
fn validate_declarations(program: &incan_frontend::ast::Program) -> Result<(), String> {
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
                if !incan_frontend::body_ir::is_direct_replacement_plain_model(model) {
                    return Err(format!(
                        "unsupported source nonplain Model {} on the native route",
                        model.name
                    ));
                }
                if model.fields.iter().any(|field| field.node.default.is_some()) {
                    return Err(format!(
                        "unsupported source Model defaults on {} on the native route",
                        model.name
                    ));
                }
                continue;
            }
            Declaration::Class(class) => {
                if !incan_frontend::body_ir::is_direct_replacement_class(class) {
                    return Err(format!(
                        "unsupported source Class generic trait adoptions, inheritance, type parameters, decorators, aliases, properties or defaults on {} on the native route",
                        class.name
                    ));
                }
                continue;
            }
            Declaration::Enum(value) => {
                if !incan_frontend::body_ir::is_direct_native_enum(value) {
                    return Err(format!("unsupported source Enum {} on the native route", value.name));
                }
                continue;
            }
            Declaration::Trait(item) => {
                if !item.type_params.is_empty()
                    || !item.traits.is_empty()
                    || !item.decorators.is_empty()
                    || !item.method_aliases.is_empty()
                    || !item.method_partials.is_empty()
                    || !item.properties.is_empty()
                    || item
                        .methods
                        .iter()
                        .any(|method| !method.node.type_params.is_empty() || !method.node.decorators.is_empty())
                {
                    return Err("unsupported source generic Trait, supertraits, decorators, aliases or properties on the native route".to_owned());
                }
                continue;
            }
            Declaration::Newtype(newtype) if incan_frontend::body_ir::is_direct_replacement_plain_newtype(newtype) => {
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
    if requires_async_sdk(&plan) && !dependencies.externs.iter().any(|(name, _)| name == "incan_std_async") {
        return Err("unsupported Body IR native dependency `incan_std_async` for TaskJoinError carrier".into());
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
    )?;
    Ok(())
}

/// Require the canonical SDK dependency for error carriers, including payload-only uses in standard Result aliases.
fn requires_async_sdk(plan: &Plan) -> bool {
    let sdk_error = |ty: &PlanType| matches!(ty, PlanType::TaskJoinError | PlanType::TaskJoinErrorRef);
    plan.functions
        .iter()
        .flat_map(|function| &function.locals)
        .any(|local| sdk_error(&local.ty))
        || plan
            .models
            .iter()
            .flat_map(|model| &model.fields)
            .any(|field| sdk_error(&field.ty))
        || plan
            .enums
            .iter()
            .flat_map(|declaration| &declaration.variants)
            .flat_map(|variant| &variant.fields)
            .any(sdk_error)
}
