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
        let system_policy =
            incan_lang::lang::standard_packages::standard_package_namespace_policy("incan_stdlib_system")
                .ok_or("missing pinned system policy")?;
        let system = layout.path().join("stdlib").join(system_policy.source_directory);
        std::fs::create_dir_all(&system)?;
        std::fs::write(system.join("loaf.toml"), system_policy.declaration)?;
        let data_policy = incan_lang::lang::standard_packages::standard_package_namespace_policy("incan_stdlib_data")
            .ok_or("missing pinned data policy")?;
        let data = layout.path().join("stdlib").join(data_policy.source_directory);
        std::fs::create_dir_all(&data)?;
        std::fs::write(data.join("loaf.toml"), data_policy.declaration)?;
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
    // Catalog-only control: the original source grants namespaces, never native execution authority.
    let artifact = root.join("target/lib");
    std::fs::create_dir_all(artifact.join("src"))?;
    std::fs::write(
        artifact.join("src/lib.rs"),
        "// Catalog facade; no native execution claim.\n",
    )?;
    std::fs::write(
        artifact.join("Cargo.toml"),
        "[package]\nname='incan_stdlib_core'\nversion='0.6.0-dev.6'\n[lib]\npath='src/lib.rs'\n",
    )?;
    let mut manifest = incan_frontend::library_manifest::LibraryManifest::new("incan_stdlib_core", "0.6.0-dev.6");
    manifest.contract_metadata.provider.namespace_claims =
        vec![incan_frontend::library_manifest::ProviderModuleClaim {
            module_path: vec!["traits".into(), "callable".into()],
            required_features: Default::default(),
        }];
    manifest.write_to_path(&artifact.join("incan_stdlib_core.incnlib"))?;
    let namespace = incan_frontend::provider::namespaces::SelectedProviderNamespace::from_source_publication(
        Arc::clone(&source),
        &artifact,
    )?;
    let index = incan_frontend::library_manifest_index::LibraryManifestIndex::from_entries(
        [(
            "core_alias".into(),
            incan_frontend::library_manifest_index::load_provider_dependency_artifact("core_alias", &artifact),
        )]
        .into(),
    );
    let explicit = ProviderPlan::from_resolved_inputs(index.clone(), None, None, None, std::iter::empty())?;
    let mut public_records = explicit.records().cloned().collect::<Vec<_>>();
    public_records.push(namespace.record().clone());
    assert!(matches!(
        ProviderPlan::new(index.clone(), public_records, std::iter::empty()),
        Err(incan_provider::ProviderPlanError::DuplicateIdentity { .. })
    ));
    let joined = ProviderPlan::from_admitted_libraries(index, &[namespace], std::iter::empty())?;
    assert_eq!(joined.records().count(), 1);
    assert_eq!(joined.active_sdk_records().count(), 1);
    let owner = joined.records().next().ok_or("missing shared namespace owner")?;
    for module in [
        vec!["std".into(), "traits".into(), "callable".into()],
        vec!["pub".into(), "core_alias".into(), "traits".into(), "callable".into()],
    ] {
        assert!(
            matches!(joined.resolve_module(&module), incan_provider::ProviderModuleResolution::Active(record) if record.identity == owner.identity)
        );
    }
    assert_eq!(
        joined.public_artifact_route("core_alias", &owner.identity)?,
        Vec::<String>::new()
    );
    assert!(joined.sdk_dependency_rebindings().is_empty());
    assert!(joined.sdk_artifact_projections().is_empty());
    joined.verify_namespace_grants()?;
    // Ordinary consumers cannot recover source defaults from namespace metadata alone. A genuine standard source
    // publisher retains the original dependency issuer and member, so lowering can expand inherited methods.
    let error_path = vec!["std".into(), "traits".into(), "callable".into()];
    let error_file = root.join("src/traits/callable.incn");
    std::fs::create_dir_all(error_file.parent().ok_or("missing trait source parent")?)?;
    std::fs::write(
        &error_file,
        "pub trait OriginalDefault:\n    def answer(self) -> int:\n        return 42\n",
    )?;
    let mut consumer_cache = incan_frontend::typechecker::stdlib_loader::StdlibAstCache::new();
    consumer_cache.bind_provider_plan(&joined);
    assert!(
        consumer_cache
            .lookup_trait_decl(&error_path, "OriginalDefault")
            .is_none()
    );
    let system_source = Arc::new(
        TrustedStandardSourcePublication::discover("incan_stdlib_system")?.ok_or("missing genuine system source")?,
    );
    let system_plan = joined.clone().with_standard_source_publication(
        Arc::clone(&system_source),
        system_source.verified_package_root()?,
        "incan_stdlib_system",
        system_source.package_identity().1,
    )?;
    let mut publisher_cache = incan_frontend::typechecker::stdlib_loader::StdlibAstCache::new();
    publisher_cache.bind_provider_plan(&system_plan);
    let inherited = publisher_cache
        .lookup_trait_decl(&error_path, "OriginalDefault")
        .ok_or("missing inherited source default")?;
    assert!(
        inherited
            .methods
            .first()
            .is_some_and(|method| method.node.body.is_some())
    );
    let mut declaration_path = error_path.clone();
    declaration_path.push("OriginalDefault".into());
    assert!(publisher_cache.lookup_trait_decl(&declaration_path, "answer").is_none());
    publisher_cache.verify_retained_sources()?;
    let mut broken_registered = incan_frontend::typechecker::stdlib_loader::StdlibAstCache::new();
    broken_registered.bind_provider_plan(&system_plan);
    assert!(
        broken_registered
            .lookup_trait_decl(&["std".into(), "derives".into(), "copying".into()], "Clone")
            .is_none()
    );
    assert!(
        broken_registered.verify_retained_sources().is_err(),
        "a missing registered module remains a hard failure"
    );
    std::fs::write(
        &error_file,
        "pub trait OriginalDefault:\n    def answer(self) -> int:\n        return 43\n",
    )?;
    assert!(publisher_cache.verify_retained_sources().is_err());
    // Keep the remaining independent generator controls on their own source-only core plan below.
    // Parsed own-facet coverage permits exactly its inspected extern labels, without certifying a native owner.
    let text = "rust.module(\"incan_std_core::errors\")\n@rust.extern\npub def raise_value_error(msg: str) -> None:\n    ...\n";
    std::fs::create_dir_all(root.join("src"))?;
    std::fs::write(root.join("src/lib.incn"), text)?;
    let tokens = incan_frontend::lexer::lex(text).map_err(|errors| format!("lex: {errors:?}"))?;
    let ast = incan_frontend::parser::parse(&tokens).map_err(|errors| format!("parse: {errors:?}"))?;
    let parsed = incan_frontend::ParsedModule {
        name: "lib".into(),
        path_segments: vec!["lib".into()],
        file_path: root.join("src/lib.incn"),
        source: text.into(),
        ast,
    };
    let plan = ProviderPlan::default().with_standard_source_publication(
        Arc::clone(&source),
        root,
        "incan_stdlib_core",
        "0.6.0-dev.6",
    )?;
    let project = oven_model::manifest::ProjectManifest::load(&root.join("loaf.toml"))?;
    let queries = std::collections::BTreeSet::from(["incan_std_core::errors::raise_value_error".into()]);
    let demands = crate::build::library_metadata::requirements::capture_checked_native_demands(
        &project,
        std::slice::from_ref(&parsed),
        &Default::default(),
        &plan,
        &queries,
    )?;
    demands.require_ordinary_source_inspection(None)?;
    demands.require_ordinary_extern_paths(&["incan_std_core::errors::raise_value_error".into()])?;
    assert!(
        demands
            .require_ordinary_extern_paths(&["incan_std_core::errors::foreign".into()])
            .is_err()
    );
    assert!(demands.require_ordinary_extern_paths(&[]).is_err());
    let missing = crate::build::library_metadata::requirements::capture_checked_native_demands(
        &project,
        &[parsed],
        &Default::default(),
        &plan,
        &Default::default(),
    )?;
    assert!(
        missing
            .require_ordinary_extern_paths(&["incan_std_core::errors::raise_value_error".into()])
            .is_err()
    );
    // The compiler supplies the canonical JSON trait default through a real native helper, not a serde free
    // function. Only the original Data member can bind that ABI; the human-readable extern label stays stable.
    let data_source = Arc::new(
        TrustedStandardSourcePublication::discover("incan_stdlib_data")?.ok_or("missing genuine data source")?,
    );
    let data_root = data_source.verified_package_root()?;
    let json_file = data_root.join("src/serde/json.incn");
    std::fs::create_dir_all(json_file.parent().ok_or("missing JSON source parent")?)?;
    let json_text = "rust.module(\"incan_std_data::serde\")\npub trait Deserialize:\n    @rust.extern\n    def from_json(json_str: str) -> Result[Self, str]:\n        ...\n";
    std::fs::write(&json_file, json_text)?;
    let tokens = incan_frontend::lexer::lex(json_text).map_err(|errors| format!("JSON lex: {errors:?}"))?;
    let ast = incan_frontend::parser::parse(&tokens).map_err(|errors| format!("JSON parse: {errors:?}"))?;
    let json_module = incan_frontend::ParsedModule {
        name: "serde.json".into(),
        path_segments: vec!["serde".into(), "json".into()],
        file_path: json_file,
        source: json_text.into(),
        ast,
    };
    let data_plan = ProviderPlan::default().with_standard_source_publication(
        Arc::clone(&data_source),
        data_root,
        data_source.package_identity().0,
        data_source.package_identity().1,
    )?;
    let contexts = crate::build::rust_extern::collect_rust_extern_contexts(std::slice::from_ref(&json_module));
    let labels = crate::build::rust_extern::rust_extern_report_paths(&contexts);
    let label = "incan_std_data::serde::from_json".to_string();
    let helper = "incan_std_data::json::__private::parse_or_error".to_string();
    assert_eq!(labels, std::slice::from_ref(&label));
    assert_eq!(
        crate::build::library_exports::collect_library_rust_abi_query_paths(
            std::slice::from_ref(&json_module),
            &contexts,
        ),
        std::slice::from_ref(&label),
        "legacy ABI labels stay unchanged"
    );
    let queries = crate::build::library_exports::collect_ordinary_library_rust_abi_query_paths(
        std::slice::from_ref(&json_module),
        &contexts,
        &data_plan,
    )?;
    assert_eq!(queries, [helper]);
    assert_eq!(
        crate::build::library_exports::collect_ordinary_library_rust_abi_query_paths(
            std::slice::from_ref(&json_module),
            &contexts,
            &ProviderPlan::default(),
        )?,
        std::slice::from_ref(&label),
        "same spelling without an original source issuer grants no mapping"
    );
    let data_project = oven_model::manifest::ProjectManifest::load(&data_root.join("loaf.toml"))?;
    let demands = crate::build::library_metadata::requirements::capture_checked_native_demands(
        &data_project,
        std::slice::from_ref(&json_module),
        &Default::default(),
        &data_plan,
        &queries.iter().cloned().collect(),
    )?;
    demands.require_ordinary_source_inspection(None)?;
    demands.require_ordinary_extern_paths(&labels)?;
    let decoded: crate::build::library_metadata::requirements::CheckedNativeDemands =
        serde_json::from_slice(&serde_json::to_vec(&demands)?)?;
    decoded.require_ordinary_extern_paths(&labels)?;
    let missing_helper = crate::build::library_metadata::requirements::capture_checked_native_demands(
        &data_project,
        std::slice::from_ref(&json_module),
        &Default::default(),
        &data_plan,
        &[label].into(),
    )?;
    assert!(missing_helper.require_ordinary_source_inspection(None).is_err());
    assert!(missing_helper.require_ordinary_extern_paths(&labels).is_err());
    let mut foreign = contexts[0].clone();
    foreign.file_path = data_root.join("src/foreign.incn");
    assert_eq!(
        crate::build::rust_extern::ordinary_rust_extern_abi_path(
            &foreign,
            std::slice::from_ref(&json_module),
            &data_plan
        )?,
        labels[0],
        "a foreign member cannot borrow the canonical helper ABI"
    );
    std::fs::write(
        &json_module.file_path,
        json_text.replace("Deserialize", "ForeignDeserialize"),
    )?;
    assert!(
        crate::build::rust_extern::ordinary_rust_extern_abi_path(
            &contexts[0],
            std::slice::from_ref(&json_module),
            &data_plan,
        )
        .is_err(),
        "changed original source invalidates its helper binding"
    );
    let output = tempfile::tempdir()?;
    let mut generator = ProjectGenerator::new(output.path(), "ordinary_core", false);
    generator.compiled_provider_modules.insert(
        "ordinary_dependency".into(),
        std::collections::BTreeSet::from(["testing".into()]),
    );
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
    assert!(emitted.contains("pub mod __incan_std"), "{emitted}");
    assert_eq!(emitted.matches("pub mod __incan_std").count(), 1, "{emitted}");
    assert!(
        emitted.contains("pub use ordinary_dependency::__incan_std::*;"),
        "{emitted}"
    );
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
