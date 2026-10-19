//! `incan build` and `incan run`: one unit through [`compile_unit`], which first compiles every Loaf the unit reaches
//! (DD-0005, DD-0007); `incan run` then executes the program.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::{CliError, CliResult, ExitCode};
use incan_driver::build::inline_command::{inline_command_project, wrap_inline_command_source};
use incan_driver::compile::{UnitKind, UnitOutput, UnitRequest, compile_unit, library_entry};
use incan_driver::features::FeatureSelection;
use oven_rustc::rustc::clear_inherited_cargo_environment;

/// One `incan build` request.
pub struct BuildRequest {
    /// The program's entry file, or a path inside the Loaf project when `library` is set.
    pub source: PathBuf,
    /// Compile the project's Loaf as a library unit instead of a program.
    pub library: bool,
    /// Where the program lands, instead of `<project>/target/incan/<name>/<profile>/`.
    pub output_dir: Option<PathBuf>,
    /// The root package's feature selection.
    pub features: FeatureSelection,
    /// The build profile.
    pub profile: &'static str,
}

/// Compile one program or one Loaf.
pub fn build(request: &BuildRequest) -> CliResult<ExitCode> {
    let (source, kind) = if request.library {
        (library_entry(&request.source)?, UnitKind::Library)
    } else {
        (request.source.clone(), UnitKind::Executable)
    };
    compile_unit(
        &UnitRequest {
            source: &source,
            kind,
            profile: request.profile,
            features: &request.features,
            output_dir: request.output_dir.as_deref(),
        },
        &mut |message| eprintln!("{message}"),
    )?;
    Ok(ExitCode::SUCCESS)
}

/// Compile and run the program whose entry is `file`, passing it `program_args`.
pub fn run_file(
    file: &Path,
    features: &FeatureSelection,
    profile: &str,
    program_args: &[String],
) -> CliResult<ExitCode> {
    let unit = compile_program(file, None, features, profile)?;
    execute(&unit, program_args)
}

/// Compile and run inline source from `incan run -c`, through a temporary project.
pub fn run_inline_source(
    source: &str,
    features: &FeatureSelection,
    profile: &str,
    program_args: &[String],
) -> CliResult<ExitCode> {
    let wrapped = wrap_inline_command_source(source);
    let project = inline_command_project(&wrapped)?;
    let directory = project.source_path.parent().ok_or_else(|| {
        CliError::failure(format!(
            "no directory for the inline command source {}",
            project.source_path.display()
        ))
    })?;
    fs::create_dir_all(directory)
        .map_err(|error| CliError::failure(format!("cannot create {}: {error}", directory.display())))?;
    fs::write(&project.source_path, wrapped)
        .map_err(|error| CliError::failure(format!("cannot write {}: {error}", project.source_path.display())))?;
    let result = compile_program(
        &project.source_path,
        Some(Path::new(&project.output_dir)),
        features,
        profile,
    )
    .and_then(|unit| execute(&unit, program_args));
    let _ = fs::remove_file(&project.source_path);
    result
}

/// Compile one program unit.
fn compile_program(
    source: &Path,
    output_dir: Option<&Path>,
    features: &FeatureSelection,
    profile: &str,
) -> CliResult<UnitOutput> {
    compile_unit(
        &UnitRequest {
            source,
            kind: UnitKind::Executable,
            profile,
            features,
            output_dir,
        },
        &mut |message| eprintln!("{message}"),
    )
}

/// Run a compiled program from its project root with the program's arguments; its exit code becomes the command's.
fn execute(unit: &UnitOutput, program_args: &[String]) -> CliResult<ExitCode> {
    let mut command = Command::new(&unit.artifact);
    command.current_dir(&unit.project_root).args(program_args);
    clear_inherited_cargo_environment(&mut command);
    let status = command
        .status()
        .map_err(|error| CliError::failure(format!("cannot run {}: {error}", unit.artifact.display())))?;
    Ok(ExitCode(status.code().unwrap_or(ExitCode::FAILURE.0)))
}
