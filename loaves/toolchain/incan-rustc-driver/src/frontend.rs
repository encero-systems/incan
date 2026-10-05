//! Minimal source-to-Body-IR glue. All Body-IR-to-plan decisions belong to the Incan lowering Loaf.

use std::{fs, path::{Path, PathBuf}};

use incan_frontend::{body_ir::build_body_ir_module_v0, lexer, parser, typechecker::TypeChecker};
use incan_mir_lowering::caller::incan::lower_module;
use incan_semantics_core::body_ir::BodyIrModule;

/// Check the original source and retain the frontend's canonical facts without a serialization boundary.
fn checked_module(source: &str, name: &str) -> Result<BodyIrModule, String> {
    let tokens = lexer::lex(source).map_err(|errors| format!("lexing failed: {errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("parsing failed: {errors:?}"))?;
    for declaration in &program.declarations {
        use incan_frontend::ast::Declaration;
        let kind = match &declaration.node {
            Declaration::Function(_) | Declaration::Docstring(_) => continue,
            Declaration::Import(_) => "Import",
            Declaration::Const(_) => "Const",
            Declaration::Static(_) => "Static",
            Declaration::Model(_) => "Model",
            Declaration::Class(_) => "Class",
            Declaration::Enum(_) => "Enum",
            Declaration::Trait(_) => "Trait",
            Declaration::Newtype(_) => "Newtype",
            Declaration::Alias(_) => "Alias",
            Declaration::Partial(_) => "Partial",
            Declaration::TypeAlias(_) => "TypeAlias",
            _ => "top-level declaration",
        };
        return Err(format!("unsupported source {kind} on the native route"));
    }
    let module_path = vec![name.to_owned()];
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(module_path.clone()));
    checker.check_program(&program).map_err(|errors| format!("checking failed: {errors:?}"))?;
    Ok(build_body_ir_module_v0(&program, &module_path, checker.type_info()))
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
    let source = fs::read_to_string(&arguments[0])?;
    let module = checked_module(&source, &arguments[1])?;
    let plan = lower_module(&module, source, arguments[0].clone())?;
    let dependencies = dependencies(&arguments[4..])?;
    crate::adapter::compile(plan, &arguments[1], Path::new(&arguments[2]), Path::new(&arguments[3]), &dependencies.externs, &dependencies.directories)?;
    Ok(())
}
