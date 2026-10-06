//! Shared CLI-session source-to-Body-IR glue. All Body-IR-to-plan decisions belong to the Incan lowering Loaf.

use std::{
    fs,
    path::{Path, PathBuf},
};

use incan_driver::{modules::collect_modules_detailed_with_session, session::CompilationSession};
use incan_frontend::body_ir::build_body_ir_module_v0;
use incan_mir_lowering::caller::incan::lower_module;
use incan_semantics_core::body_ir::BodyIrModule;

/// Collect and analyze the original source through the CLI's compilation session, retaining canonical checked facts.
/// Native-only declaration refusals follow analysis; provider activation, desugaring, and stdlib loading remain owned
/// by the session.
fn checked_module(path: &Path) -> Result<BodyIrModule, String> {
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
    if modules.len() != 1 {
        return Err("unsupported source multi-module graph on the native route".to_owned());
    }
    let module = modules
        .iter()
        .find(|module| module.file_path == path)
        .ok_or("entry module is missing")?;
    let program = &module.ast;
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
            Declaration::Static(_) => "Static",
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
            Declaration::Newtype(_) => "Newtype",
            Declaration::Alias(_) => "Alias",
            Declaration::Partial(_) => "Partial",
            Declaration::TypeAlias(_) => "TypeAlias",
            _ => "top-level declaration",
        };
        return Err(format!("unsupported source {kind} on the native route"));
    }
    let type_info = analysis.type_info_for_path(&path).ok_or("entry analysis is missing")?;
    Ok(build_body_ir_module_v0(program, &module.path_segments, type_info))
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
    let source = fs::read_to_string(&arguments[0])?;
    let source_path = PathBuf::from(&arguments[0]);
    let module = incan_frontend::compiler_stack::run_on_compiler_stack(move || checked_module(&source_path))?;
    let plan = lower_module(&module, source, arguments[0].clone())?;

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
