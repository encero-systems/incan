//! Generated Rust projects driven from checked programs: the codegen tests that need the project generator, kept
//! beside it because the generator sits above the emission crate.

use crate::backend::ir::test_support::{
    assert_no_generated_unused_lint_allows, generate, must_ok, must_some, parse_program, parse_program_result,
    prewarm_metadata,
};
use crate::backend::ir::{AstLowering, IrCodegen, IrEmitter};
use crate::frontend::{lexer, parser};

#[test]
fn generated_rust_warning_clean() -> Result<(), Box<dyn std::error::Error>> {
    use crate::backend::project::ProjectGenerator;
    use std::process::Command;

    let code = generate(
        r#"
import rust::std::f64::consts as consts

model User:
  name: str
  age: int

def helper(value: int) -> int:
  return value

def main() -> None:
  let user = User(name="Ada", age=42)
  print(user.name)
  print(helper(1))
  _ = consts.PI
"#,
    );
    assert_no_generated_unused_lint_allows(&code);

    let tmp = tempfile::tempdir()?;
    let generator = ProjectGenerator::new(tmp.path(), "warning_clean_codegen", true);
    generator.generate(&code)?;

    let capability = crate::oven::compiler_suite_env::OvenCompilerSuiteCapability::from_environment(
        crate::oven::compiler_suite_env::OVEN_COMPILER_SUITE_CAPABILITY_ENV,
    )
    .map_err(std::io::Error::other)?;
    let output = if let Some(capability) = capability {
        if !capability.externs.contains_key("incan_stdlib") {
            return Err("stored Oven compiler suite direct-rustc capability omitted incan_stdlib".into());
        }
        let mut command = Command::new(&capability.rustc);
        command
            .arg("--edition=2024")
            .arg("--crate-name")
            .arg("warning_clean_codegen")
            .arg("--emit=metadata")
            .arg("--out-dir")
            .arg(tmp.path().join("oven-warning-check"))
            .arg("-Dwarnings")
            .env("CARGO_MANIFEST_DIR", tmp.path())
            .env("CARGO_PKG_NAME", "warning_clean_codegen")
            .env("CARGO_PKG_VERSION", "0.1.0");
        for dependency_path in &capability.dependency_search_paths {
            command
                .arg("-L")
                .arg(format!("dependency={}", dependency_path.display()));
        }
        for (crate_name, path) in &capability.externs {
            command.arg("--extern").arg(format!("{crate_name}={}", path.display()));
        }
        command.arg(generator.crate_root_path()).output()?
    } else {
        Command::new("cargo")
            .arg("check")
            .current_dir(tmp.path())
            .env("CARGO_NET_OFFLINE", "true")
            .env("RUSTFLAGS", "-Dwarnings")
            .output()?
    };

    assert!(
        output.status.success(),
        "generated Rust should pass the configured warning check with -Dwarnings\nstderr:\n{}\nstdout:\n{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    Ok(())
}

#[cfg(feature = "rust_inspect")]
#[test]
fn test_codegen_borrows_retained_rustix_file_for_as_fd_generic_free_function() -> Result<(), Box<dyn std::error::Error>>
{
    use crate::backend::project::ProjectGenerator;
    use crate::manifest::{DependencySource, DependencySpec};
    use crate::rust_inspect::write_rustix_as_fd_probe_crate;

    let source = r#"
from rust::rustix::fs import File, FlockOperation, flock

pub def retain(file: File) -> File:
  flock(file, FlockOperation.LockExclusive)
  file.sync_all()
  return file
"#;
    let tmp = tempfile::tempdir()?;
    write_rustix_as_fd_probe_crate(tmp.path())?;

    let ast = parse_program_result(source)?;
    let mut codegen = IrCodegen::new();
    codegen.set_rust_inspect_manifest_dir(tmp.path().to_path_buf());
    let code = codegen
        .try_generate(&ast)
        .map_err(|error| std::io::Error::other(format!("codegen failed: {error}")))?;

    assert!(
        code.contains("flock(&file, FlockOperation::LockExclusive);"),
        "expected rustix::fs::flock to borrow the retained File through its AsFd generic; got:\n{code}"
    );
    assert!(
        code.contains("file.sync_all();") && code.contains("return file;"),
        "the retained file must remain available for sync_all and return after flock; got:\n{code}"
    );

    let generated_root = tmp.path().join("generated");
    let mut generator = ProjectGenerator::new(&generated_root, "retained_rustix_file", false);
    generator.set_dependencies(vec![DependencySpec {
        crate_name: "rustix".to_string(),
        version: None,
        features: Vec::new(),
        default_features: true,
        source: DependencySource::Path {
            path: tmp.path().to_path_buf(),
        },
        optional: false,
        package: None,
    }]);
    generator.generate(&code)?;
    let capability = crate::oven::compiler_suite_env::OvenCompilerSuiteCapability::from_environment(
        crate::oven::compiler_suite_env::OVEN_COMPILER_SUITE_CAPABILITY_ENV,
    )
    .map_err(std::io::Error::other)?;
    let output = if let Some(capability) = capability {
        let fixture_output = tmp.path().join("oven-rustix-fixture");
        std::fs::create_dir_all(&fixture_output)?;
        let fixture = std::process::Command::new(&capability.rustc)
            .args(["--edition=2021", "--crate-name", "rustix", "--crate-type=rlib"])
            .arg(tmp.path().join("src/lib.rs"))
            .arg("--out-dir")
            .arg(&fixture_output)
            .output()?;
        assert!(
            fixture.status.success(),
            "expected the direct-rustc rustix fixture to compile. stderr:\n{}\nstdout:\n{}",
            String::from_utf8_lossy(&fixture.stderr),
            String::from_utf8_lossy(&fixture.stdout)
        );

        let rustix_rlib = fixture_output.join("librustix.rlib");
        let mut command = std::process::Command::new(&capability.rustc);
        command
            .args([
                "--edition=2024",
                "--crate-name",
                "retained_rustix_file",
                "--crate-type=lib",
                "--emit=metadata",
            ])
            .arg("--out-dir")
            .arg(generated_root.join("oven-check"))
            .env("CARGO_MANIFEST_DIR", &generated_root)
            .env("CARGO_PKG_NAME", "retained_rustix_file")
            .env("CARGO_PKG_VERSION", "0.1.0");
        for dependency_path in &capability.dependency_search_paths {
            command
                .arg("-L")
                .arg(format!("dependency={}", dependency_path.display()));
        }
        for (crate_name, path) in &capability.externs {
            command.arg("--extern").arg(format!("{crate_name}={}", path.display()));
        }
        command
            .arg("--extern")
            .arg(format!("rustix={}", rustix_rlib.display()))
            .arg(generator.crate_root_path())
            .output()?
    } else {
        std::process::Command::new("cargo")
            .args(["check", "--offline"])
            .current_dir(&generated_root)
            .output()?
    };
    assert!(
        output.status.success(),
        "expected the generated retained-file Rust to compile. stderr:\n{}\nstdout:\n{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    Ok(())
}

#[cfg(feature = "rust_inspect")]
#[test]
fn test_codegen_borrows_async_rust_backed_free_function_args_from_generated_lock_workspace()
-> Result<(), Box<dyn std::error::Error>> {
    use crate::backend::project::ProjectGenerator;
    use crate::frontend::typechecker::TypeChecker;
    use crate::manifest::{DependencySource, DependencySpec};
    use crate::rust_inspect::write_hyphenated_function_probe_crate;

    let source = r#"
from std.async import sleep
from rust::foo_bar import State
from rust::foo_bar import Plan
from rust::foo_bar::consumer import consume

pub async def run(state: State, plan: Plan) -> None:
  await sleep(0.01)
  await consume(state, plan)
"#;
    let tokens = must_ok(lexer::lex(source));
    let ast = must_ok(parser::parse(&tokens));

    let tmp = tempfile::tempdir()?;
    let dep_root = tmp.path().join("foo-bar-dep");
    write_hyphenated_function_probe_crate(&dep_root)?;

    let lock_root = tmp.path().join("generated_lock");
    let mut generator = ProjectGenerator::new(&lock_root, "lock_probe", true);
    generator.set_dependencies(vec![DependencySpec {
        crate_name: "foo-bar".to_string(),
        version: None,
        features: vec![],
        default_features: true,
        source: DependencySource::Path { path: dep_root.clone() },
        optional: false,
        package: None,
    }]);
    generator.generate("fn main() {}\n")?;

    let mut tc = TypeChecker::new();
    tc.set_rust_inspect_manifest_dir(lock_root.clone());
    prewarm_metadata(
        &lock_root,
        &["foo_bar::State", "foo_bar::Plan", "foo_bar::consumer::consume"],
    )?;
    tc.check_program(&ast)
        .map_err(|errs| std::io::Error::other(format!("typecheck failed: {errs:?}")))?;

    let mut lowering = AstLowering::new_with_type_info(tc.type_info().clone());
    let ir_program = lowering
        .lower_program(&ast)
        .map_err(|err| std::io::Error::other(format!("lowering failed: {err:?}")))?;

    let mut codegen = IrCodegen::new();
    codegen.collect_external_rust_functions(&ast);

    let mut emitter = IrEmitter::new(&ir_program.function_registry);
    emitter.set_external_rust_functions(codegen.external_rust_functions.clone());
    let code = emitter
        .emit_program(&ir_program)
        .map_err(|err| std::io::Error::other(format!("emit failed: {err:?}")))?;

    assert!(
        code.contains("consume(&state, &plan).await"),
        "expected borrowed async rust free-function args from generated lock workspace; got:\n{code}"
    );
    Ok(())
}

#[cfg(feature = "rust_inspect")]
#[test]
fn test_nested_module_codegen_borrows_async_rust_args_from_generated_lock_workspace()
-> Result<(), Box<dyn std::error::Error>> {
    use crate::backend::project::ProjectGenerator;
    use crate::manifest::{DependencySource, DependencySpec};
    use crate::rust_inspect::write_hyphenated_function_probe_crate;

    let main_module = parse_program(
        r#"
def main() -> None:
  return
"#,
    );
    let dep_module = parse_program(
        r#"
from std.async import sleep
from rust::foo_bar import State
from rust::foo_bar import Plan
from rust::foo_bar::consumer import consume

pub async def run(state: State, plan: Plan) -> None:
  await sleep(0.01)
  await consume(state, plan)
"#,
    );

    let tmp = tempfile::tempdir()?;
    let dep_root = tmp.path().join("foo-bar-dep");
    write_hyphenated_function_probe_crate(&dep_root)?;

    let lock_root = tmp.path().join("generated_lock");
    let mut generator = ProjectGenerator::new(&lock_root, "lock_probe", true);
    generator.set_dependencies(vec![DependencySpec {
        crate_name: "foo-bar".to_string(),
        version: None,
        features: vec![],
        default_features: true,
        source: DependencySource::Path { path: dep_root.clone() },
        optional: false,
        package: None,
    }]);
    generator.generate("fn main() {}\n")?;

    let worker_path = vec!["worker".to_string()];
    let mut codegen = IrCodegen::new();
    codegen.set_rust_inspect_manifest_dir(lock_root);
    codegen.add_module_with_path_segments("worker", &dep_module, worker_path.clone());

    let (_main_code, rust_modules) =
        must_ok(codegen.try_generate_multi_file_nested(&main_module, std::slice::from_ref(&worker_path)));
    let worker_code = must_some(rust_modules.get(&worker_path), "missing generated worker module");

    assert!(
        worker_code.contains("consume(&state, &plan).await"),
        "expected borrowed async rust free-function args in generated nested module; got:\n{worker_code}"
    );
    Ok(())
}
