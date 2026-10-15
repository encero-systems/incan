//! Generated Rust projects driven from checked programs: the codegen tests that need the project generator, kept
//! beside it because the generator sits above the emission crate.

use crate::backend::ir::test_support::{
    assert_no_generated_unused_lint_allows, generate, must_ok, must_some, parse_program, parse_program_result,
    prewarm_metadata,
};
use crate::backend::ir::{AstLowering, IrCodegen, IrEmitter};
use incan_frontend::{lexer, parser};

/// Ordinary namespace projection follows the already linked artifact even without an authored standard import.
#[test]
fn ordinary_linked_core_projects_generated_namespace_without_source_imports() -> Result<(), Box<dyn std::error::Error>>
{
    use crate::backend::project::ProjectGenerator;
    use incan_frontend::library_manifest::LibraryManifest;
    use incan_frontend::library_manifest_index::LibraryArtifactMetadata;
    use incan_provider::{NamespaceAuthority, ProviderIdentity, ProviderPlan, ProviderProvenance, ProviderRecord};
    use std::collections::{BTreeSet, HashMap};
    use std::sync::Arc;

    let output = tempfile::tempdir()?;
    let artifact = LibraryArtifactMetadata::from_crate_root(
        "ordinary-core-alias",
        "incan_stdlib_core",
        output.path().join("artifact"),
    );
    // This isolates generator projection; the publication control separately proves genuine source admission.
    let mut record = ProviderRecord {
        identity: ProviderIdentity {
            name: "incan_stdlib_core".into(),
            version: "0.6.0".into(),
            digest: "sha256:ordinary-generator-projection".into(),
            feature_projection: BTreeSet::new(),
        },
        provenance: ProviderProvenance::StandardPublication {
            package_name: "incan_stdlib_core".into(),
            manifest_path: output.path().join("loaf.toml"),
        },
        authority: NamespaceAuthority::SdkReserved,
        namespace_claims: [vec!["std".into(), "derives".into(), "collection".into()]].into(),
        available: true,
        enabled: true,
        manifest: Some(Arc::new(LibraryManifest::new("incan_stdlib_core", "0.6.0"))),
        artifact: Some(artifact.clone()),
        implementation_facets: Vec::new(),
    };
    let plan = ProviderPlan::new(Default::default(), vec![record.clone()], std::iter::empty())?;
    assert!(
        plan.sdk_link_roots().is_empty(),
        "no authored module or synthetic prelude use"
    );
    assert!(
        crate::build::caller_owned::oven_caller_owned_libraries(&plan, "debug", "target", "toolchain", None)
            .is_err_and(|error| error.message.contains("no retained producer issuer")),
        "a public namespace descriptor cannot authorize a native library attachment"
    );
    for binary in [false, true] {
        for nested in [false, true] {
            let path = output.path().join(format!("{binary}-{nested}"));
            let mut generator = ProjectGenerator::new(&path, "ordinary_consumer", binary);
            generator.set_provider_plan(&plan);
            generator.set_dependencies(vec![artifact.to_dependency_spec()]);
            let root = "// __INCAN_INSERT_MODS__\npub fn answer() -> i64 { 42 }\n";
            if nested {
                generator.generate_nested(root, &HashMap::new())?;
            } else {
                generator.generate(root)?;
            }
            let generated = std::fs::read_to_string(generator.crate_root_path())?;
            assert!(
                generated.contains("pub use ordinary_core_alias::__incan_std::*;"),
                "{generated}"
            );
            assert!(!path.join("src/__incan_std").exists());
            assert!(
                generator.sdk_path_dependencies.is_empty(),
                "ordinary artifacts are not SDK paths"
            );
        }
    }

    record.provenance = ProviderProvenance::Sdk {
        sdk_identity: "legacy-test".into(),
        component_id: "stdlib-core".into(),
        inventory_path: None,
    };
    let unused_sdk = ProviderPlan::new(Default::default(), vec![record], std::iter::empty())?;
    let mut generator = ProjectGenerator::new(output.path().join("unused-sdk"), "unused", false);
    generator.set_provider_plan(&unused_sdk);
    generator.generate("pub fn answer() -> i64 { 42 }\n")?;
    assert!(!std::fs::read_to_string(generator.crate_root_path())?.contains("mod __incan_std"));
    Ok(())
}

/// A genuine source owner maps only emitted owned roots; a default plan and conflicting roots cannot grant it.
#[cfg(unix)]
#[test]
fn ordinary_source_namespace_projection_preserves_original_roots() -> Result<(), Box<dyn std::error::Error>> {
    use crate::backend::project::ProjectGenerator;
    use incan_frontend::provider::source_policy::TrustedStandardSourcePublication;
    use incan_provider::ProviderPlan;
    use std::collections::HashMap;
    use std::sync::Arc;

    const CHILD: &str = "INCAN_ORDINARY_NAMESPACE_PROJECTION_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let layout = tempfile::tempdir()?;
        let executable = layout.path().join("bin/incan");
        std::fs::create_dir_all(executable.parent().ok_or("missing executable parent")?)?;
        if std::fs::hard_link(std::env::current_exe()?, &executable).is_err() {
            std::fs::copy(std::env::current_exe()?, &executable)?;
        }
        let policy = incan_lang::lang::standard_packages::standard_package_namespace_policy("incan_stdlib_core")
            .ok_or("missing pinned core policy")?;
        let package = layout.path().join("stdlib").join(policy.source_directory);
        std::fs::create_dir_all(&package)?;
        std::fs::write(package.join("loaf.toml"), policy.declaration)?;
        let output = std::process::Command::new(executable)
            .args([
                "--exact",
                "backend::project::tests::codegen_generator::ordinary_source_namespace_projection_preserves_original_roots",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(CHILD, "1")
            .output()?;
        assert!(
            output.status.success(),
            "stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
        return Ok(());
    }

    let source = Arc::new(
        TrustedStandardSourcePublication::discover("incan_stdlib_core")?.ok_or("missing genuine core source")?,
    );
    let root = source.verified_package_root()?;
    let plan = ProviderPlan::default().with_standard_source_publication(
        Arc::clone(&source),
        root,
        "incan_stdlib_core",
        "0.6.0-dev.6",
    )?;
    let output = tempfile::tempdir()?;
    let mut generator = ProjectGenerator::new(output.path(), "ordinary_core", false);
    let mut modules: HashMap<Vec<String>, String> = [
        (vec!["traits".into(), "callable".into()], "pub trait Callable {}".into()),
        (vec!["application".into()], "pub struct Unowned;".into()),
    ]
    .into();
    let original = "// __INCAN_INSERT_MODS__\n";
    let mut unowned = original.to_string();
    generator.project_owned_standard_namespace(&mut unowned, &modules, &ProviderPlan::default())?;
    assert_eq!(unowned, original);

    generator
        .compiled_provider_modules
        .insert("published_dependency".into(), ["external".into()].into());
    let mut projected = original.to_string();
    generator.project_owned_standard_namespace(&mut projected, &modules, &plan)?;
    assert!(projected.contains("pub use crate::traits;"), "{projected}");
    assert!(!projected.contains("pub use crate::application;"), "{projected}");
    assert!(!projected.contains("pub use crate::derives;"), "{projected}");
    generator.generate_nested(&projected, &modules)?;
    let emitted = std::fs::read_to_string(generator.crate_root_path())?;
    assert_eq!(emitted.matches("pub mod __incan_std").count(), 1, "{emitted}");
    assert!(
        emitted.contains("pub use published_dependency::__incan_std::*;"),
        "{emitted}"
    );
    assert!(emitted.contains("pub mod application;"), "{emitted}");

    modules.insert(vec!["__incan_std".into()], String::new());
    assert!(
        generator
            .project_owned_standard_namespace(&mut String::new(), &modules, &plan)
            .is_err()
    );
    Ok(())
}

/// Generated model code must compile without warnings through the suite's selected native dependency closure.
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

    let capability = oven_store::compiler_suite_env::OvenCompilerSuiteCapability::from_environment(
        oven_store::compiler_suite_env::OVEN_COMPILER_SUITE_CAPABILITY_ENV,
    )
    .map_err(std::io::Error::other)?
    .ok_or("generated warning check requires the suite's selected direct-rustc capability")?;
    let output = {
        if !capability.externs.contains_key("incan_std_core") {
            return Err("stored Oven compiler suite direct-rustc capability omitted incan_std_core".into());
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
    };

    assert!(
        output.status.success(),
        "generated Rust should pass the configured warning check with -Dwarnings\nstderr:\n{}\nstdout:\n{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    Ok(())
}

/// Retained Rustix files must satisfy the generated generic borrow through the selected native dependency closure.
#[cfg(feature = "rust_inspect")]
#[test]
fn test_codegen_borrows_retained_rustix_file_for_as_fd_generic_free_function() -> Result<(), Box<dyn std::error::Error>>
{
    use crate::backend::project::ProjectGenerator;
    use ::rust_inspect::test_fixtures::write_rustix_as_fd_probe_crate;
    use oven_model::manifest::{DependencySource, DependencySpec};

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
    let capability = oven_store::compiler_suite_env::OvenCompilerSuiteCapability::from_environment(
        oven_store::compiler_suite_env::OVEN_COMPILER_SUITE_CAPABILITY_ENV,
    )
    .map_err(std::io::Error::other)?
    .ok_or("generated borrow check requires the suite's selected direct-rustc capability")?;
    let output = {
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
            // This case owns a deliberately small Rustix fixture; only that original artifact may supply its alias.
            if crate_name != "rustix" {
                command.arg("--extern").arg(format!("{crate_name}={}", path.display()));
            }
        }
        command
            .arg("--extern")
            .arg(format!("rustix={}", rustix_rlib.display()))
            .arg(generator.crate_root_path())
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
    use ::rust_inspect::test_fixtures::write_hyphenated_function_probe_crate;
    use incan_frontend::typechecker::TypeChecker;
    use oven_model::manifest::{DependencySource, DependencySpec};

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
    use ::rust_inspect::test_fixtures::write_hyphenated_function_probe_crate;
    use oven_model::manifest::{DependencySource, DependencySpec};

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
