//! Path-library materialization and invocation regression tests.

use super::*;

#[test]
fn materializes_recursive_path_rust_libraries_without_cargo() -> Result<(), Box<dyn std::error::Error>> {
    let workspace = tempfile::tempdir()?;
    let helper = workspace.path().join("helper");
    let wrapper = workspace.path().join("wrapper");
    fs::create_dir_all(helper.join("src"))?;
    fs::create_dir_all(wrapper.join("src"))?;
    fs::write(
        helper.join("Cargo.toml"),
        "[package]\nname = \"helper\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )?;
    fs::write(helper.join("src/lib.rs"), "pub fn value() -> i64 { 41 }\n")?;
    fs::write(
        wrapper.join("Cargo.toml"),
        "[package]\nname = \"wrapper\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nhelper = { path = \"../helper\" }\n",
    )?;
    fs::write(
        wrapper.join("src/lib.rs"),
        "pub fn value() -> i64 { helper::value() + 1 }\n",
    )?;
    let rustc = rustc_path()?;
    let target = rustc_host_target(&rustc)?;
    let libraries = materialize_declared_rust_libraries(
        &workspace.path().join("oven-output"),
        &rustc,
        &target,
        "debug",
        &[DependencySpec {
            crate_name: "wrapper".to_string(),
            version: None,
            features: Vec::new(),
            default_features: true,
            source: DependencySource::Path { path: wrapper.clone() },
            optional: false,
            package: None,
        }],
        None,
    )?;
    assert_eq!(libraries.len(), 2, "the direct closure should retain the nested helper");
    assert!(libraries.iter().all(|library| library.output.is_file()));
    let consumer_source = workspace.path().join("consumer.rs");
    let consumer_output = workspace.path().join("consumer");
    fs::write(&consumer_source, "fn main() { assert_eq!(wrapper::value(), 42); }\n")?;
    let mut command = Command::new(&rustc);
    command.arg("--edition=2021");
    for library in &libraries {
        command
            .arg("--extern")
            .arg(format!("{}={}", library.crate_name, library.output.display()));
        let parent = library.output.parent().ok_or("materialized library has no parent")?;
        command.arg("-L").arg(format!("dependency={}", parent.display()));
    }
    let status = command.arg(&consumer_source).arg("-o").arg(&consumer_output).status()?;
    assert!(
        status.success(),
        "direct-rustc consumer should link the materialized path crate"
    );
    assert!(Command::new(consumer_output).status()?.success());
    Ok(())
}

#[test]
fn selected_path_authority_prefers_a_compilation_equivalent_sealed_registry_artifact()
-> Result<(), Box<dyn std::error::Error>> {
    let workspace = tempfile::tempdir()?;
    let catalog = workspace.path().join("catalog");
    let selected = workspace.path().join("selected/deps");
    fs::create_dir_all(&catalog)?;
    fs::create_dir_all(&selected)?;
    let sealed = catalog.join("libserde-verified.rlib");
    let selected_copy = selected.join("libserde-verified.rlib");
    fs::write(&sealed, b"receipt-bound serde")?;
    fs::write(&selected_copy, b"receipt-bound serde")?;
    let plan = OvenRustcArtifactPlan {
        source_path_projection: None,
        dependency_search_paths: vec![selected.clone()],
        native_search_paths: Vec::new(),
        externs: Vec::new(),
        compile_environment: BTreeMap::new(),
        caller_owned_library_digests: BTreeMap::new(),
    };
    let authority = OvenSelectedPathRustcAuthority::new(&[], &plan);

    assert_eq!(
        authority.matching_sealed_registry_artifact(&sealed),
        Some(selected_copy)
    );
    fs::write(selected.join("libserde-verified.rlib"), b"different serde")?;
    assert_eq!(
        authority.matching_sealed_registry_artifact(&sealed),
        Some(selected.join("libserde-verified.rlib"))
    );
    Ok(())
}

#[test]
fn materializes_a_path_library_with_a_selected_compiler_runtime_child_without_reparsing_features()
-> Result<(), Box<dyn std::error::Error>> {
    let workspace = tempfile::tempdir()?;
    let runtime_root = workspace.path().join("leased-runtime");
    let runtime = runtime_root.join("incan_std_core");
    let wrapper = workspace.path().join("caller-wrapper");
    let sealed_dependencies = workspace.path().join("selected-plan/deps");
    fs::create_dir_all(runtime.join("src"))?;
    fs::create_dir_all(wrapper.join("src"))?;
    fs::create_dir_all(&sealed_dependencies)?;
    fs::write(
        runtime.join("Cargo.toml"),
        "[package]\nname = \"incan_std_core\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[features]\nfull = []\n",
    )?;
    fs::write(runtime.join("src/lib.rs"), "pub fn value() -> i64 { 41 }\n")?;
    fs::write(
        wrapper.join("Cargo.toml"),
        "[package]\nname = \"caller_wrapper\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nincan_std_core = { path = \"../leased-runtime/incan_std_core\" }\n",
    )?;
    fs::write(
        wrapper.join("src/lib.rs"),
        "pub fn value() -> i64 { incan_std_core::value() + 1 }\n",
    )?;
    let rustc = rustc_path()?;
    let target = rustc_host_target(&rustc)?;
    let selected_runtime = sealed_dependencies.join("libincan_std_core.rlib");
    let runtime_status = Command::new(&rustc)
        .args(["--crate-type", "lib", "--crate-name", "incan_std_core"])
        .arg("--target")
        .arg(&target)
        .arg("--edition=2021")
        .arg(runtime.join("src/lib.rs"))
        .arg("-o")
        .arg(&selected_runtime)
        .status()?;
    assert!(runtime_status.success(), "selected runtime fixture should compile");
    let plan = OvenRustcArtifactPlan {
        source_path_projection: None,
        dependency_search_paths: vec![sealed_dependencies.clone()],
        native_search_paths: Vec::new(),
        externs: vec![("incan_std_core".to_string(), selected_runtime.clone())],
        compile_environment: BTreeMap::new(),
        caller_owned_library_digests: BTreeMap::new(),
    };
    let authority = OvenSelectedPathRustcAuthority::new(&[fs::canonicalize(&runtime_root)?], &plan);
    let libraries = materialize_declared_rust_libraries_with_selected_path_authority(
        &workspace.path().join("oven-output"),
        &rustc,
        &target,
        "debug",
        &[DependencySpec {
            crate_name: "caller_wrapper".to_string(),
            version: None,
            features: Vec::new(),
            default_features: true,
            source: DependencySource::Path { path: wrapper },
            optional: false,
            package: None,
        }],
        None,
        Some(&authority),
    )?;
    assert_eq!(libraries.len(), 1, "the selected runtime remains plan-owned");
    assert_eq!(libraries[0].crate_name, "caller_wrapper");

    let consumer_source = workspace.path().join("consumer.rs");
    let consumer_output = workspace.path().join("consumer");
    fs::write(
        &consumer_source,
        "fn main() { assert_eq!(caller_wrapper::value(), 42); }\n",
    )?;
    let wrapper_output = &libraries[0].output;
    let wrapper_parent = wrapper_output.parent().ok_or("wrapper output parent")?;
    let status = Command::new(&rustc)
        .arg("--edition=2021")
        .arg("-L")
        .arg(format!("dependency={}", wrapper_parent.display()))
        .arg("-L")
        .arg(format!("dependency={}", sealed_dependencies.display()))
        .arg("--extern")
        .arg(format!("caller_wrapper={}", wrapper_output.display()))
        .arg("--extern")
        .arg(format!("incan_std_core={}", selected_runtime.display()))
        .arg(&consumer_source)
        .arg("-o")
        .arg(&consumer_output)
        .status()?;
    assert!(
        status.success(),
        "direct-rustc consumer should link the selected runtime"
    );
    assert!(Command::new(consumer_output).status()?.success());
    Ok(())
}

#[test]
fn materializes_a_path_library_that_disables_default_features_without_enabling_any_feature()
-> Result<(), Box<dyn std::error::Error>> {
    let workspace = tempfile::tempdir()?;
    let component = workspace.path().join("component");
    let wrapper = workspace.path().join("wrapper");
    fs::create_dir_all(component.join("src"))?;
    fs::create_dir_all(wrapper.join("src"))?;
    fs::write(
        component.join("Cargo.toml"),
        "[package]\nname = \"feature_component\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[features]\ndefault = [\"unavailable\"]\nunavailable = []\n",
    )?;
    fs::write(
        component.join("src/lib.rs"),
        "#[cfg(feature = \"unavailable\")] compile_error!(\"default Cargo feature must stay disabled\");\npub fn value() -> i64 { 41 }\n",
    )?;
    fs::write(
        wrapper.join("Cargo.toml"),
        "[package]\nname = \"feature_wrapper\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nfeature_component = { path = \"../component\", default-features = false }\n",
    )?;
    fs::write(
        wrapper.join("src/lib.rs"),
        "pub fn value() -> i64 { feature_component::value() + 1 }\n",
    )?;
    let rustc = rustc_path()?;
    let target = rustc_host_target(&rustc)?;
    let libraries = materialize_declared_rust_libraries(
        &workspace.path().join("oven-output"),
        &rustc,
        &target,
        "debug",
        &[DependencySpec {
            crate_name: "feature_wrapper".to_string(),
            version: None,
            features: Vec::new(),
            default_features: true,
            source: DependencySource::Path { path: wrapper.clone() },
            optional: false,
            package: None,
        }],
        None,
    )?;
    assert_eq!(
        libraries.len(),
        2,
        "the feature-disabled component remains a direct Rustc dependency"
    );
    let consumer_source = workspace.path().join("consumer.rs");
    let consumer_output = workspace.path().join("consumer");
    fs::write(
        &consumer_source,
        "fn main() { assert_eq!(feature_wrapper::value(), 42); }\n",
    )?;
    let mut command = Command::new(&rustc);
    command.arg("--edition=2021");
    for library in &libraries {
        command
            .arg("--extern")
            .arg(format!("{}={}", library.crate_name, library.output.display()));
        let parent = library.output.parent().ok_or("materialized library has no parent")?;
        command.arg("-L").arg(format!("dependency={}", parent.display()));
    }
    let status = command.arg(&consumer_source).arg("-o").arg(&consumer_output).status()?;
    assert!(
        status.success(),
        "direct-rustc consumer should link a default-feature-disabled component"
    );
    assert!(Command::new(consumer_output).status()?.success());

    let error = match materialize_declared_rust_libraries(
        &workspace.path().join("default-feature-output"),
        &rustc,
        &target,
        "debug",
        &[DependencySpec {
            crate_name: "feature_component".to_string(),
            version: None,
            features: Vec::new(),
            default_features: true,
            source: DependencySource::Path { path: component },
            optional: false,
            package: None,
        }],
        None,
    ) {
        Ok(_) => return Err("a default Cargo feature activation remains unsupported".into()),
        Err(error) => error,
    };
    assert!(error.to_string().contains("activates default Cargo features"));
    Ok(())
}

#[test]
fn materializes_a_path_proc_macro_with_a_sealed_registry_child_without_cargo() -> Result<(), Box<dyn std::error::Error>>
{
    let workspace = tempfile::tempdir()?;
    let registry = workspace.path().join("registry");
    let macro_package = workspace.path().join("macro-package");
    let registry_deps = registry.join("target/aarch64-apple-darwin/debug/deps");
    fs::create_dir_all(&registry_deps)?;
    fs::create_dir_all(macro_package.join("src"))?;
    let rustc = rustc_path()?;
    let target = rustc_host_target(&rustc)?;
    let registry_base_source = registry.join("registry_base.rs");
    let registry_base_artifact = registry_deps.join("libregistry_base.rlib");
    fs::write(&registry_base_source, "pub fn marker() {}\n")?;
    let registry_base_status = Command::new(&rustc)
        .args(["--crate-type", "lib", "--crate-name", "registry_base"])
        .arg("--target")
        .arg(&target)
        .arg("--edition=2021")
        .arg(&registry_base_source)
        .arg("-o")
        .arg(&registry_base_artifact)
        .status()?;
    assert!(
        registry_base_status.success(),
        "fixture registry transitive leaf should compile"
    );
    let registry_source = registry.join("registry_helper.rs");
    let registry_artifact = registry_deps.join("libregistry_helper.rlib");
    fs::write(&registry_source, "pub fn marker() { registry_base::marker(); }\n")?;
    let registry_status = Command::new(&rustc)
        .args(["--crate-type", "lib", "--crate-name", "registry_helper"])
        .arg("--target")
        .arg(&target)
        .arg("--edition=2021")
        .arg("-L")
        .arg(format!("dependency={}", registry_deps.display()))
        .arg("--extern")
        .arg(format!("registry_base={}", registry_base_artifact.display()))
        .arg(&registry_source)
        .arg("-o")
        .arg(&registry_artifact)
        .status()?;
    assert!(registry_status.success(), "fixture registry leaf should compile");
    fs::write(
        macro_package.join("Cargo.toml"),
        "[package]\nname = \"proc_fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[lib]\nproc-macro = true\n\n[dependencies]\nregistry_helper = \"1\"\n",
    )?;
    fs::write(
        macro_package.join("src/lib.rs"),
        "use proc_macro::{Literal, TokenStream, TokenTree};\nuse registry_helper::marker;\n#[proc_macro]\npub fn answer(_input: TokenStream) -> TokenStream { marker(); TokenStream::from(TokenTree::Literal(Literal::u32_unsuffixed(42))) }\n",
    )?;
    let registry_relative = "target/aarch64-apple-darwin/debug/deps/libregistry_helper.rlib";
    let authority = OvenRegistryLeafAuthority::new_with_trusted_dependency_search_paths(
        registry.clone(),
        vec![OvenRustcRegistryLeaf {
            domain: Default::default(),
            crate_kind: Default::default(),
            selected_unit_identity: None,
            package: "registry_helper".to_string(),
            version: "1.0.0".to_string(),
            crate_name: "registry_helper".to_string(),
            features: Vec::new(),
            source: fixture_registry_source(),
            artifact: OvenRustcArtifactExtern {
                crate_name: "registry_helper".to_string(),
                relative_path: registry_relative.to_string(),
                digest: digest_bytes(&fs::read(&registry_artifact)?),
            },
        }],
        vec![registry_deps],
    );
    let libraries = materialize_declared_rust_libraries(
        &workspace.path().join("oven-output"),
        &rustc,
        &target,
        "debug",
        &[DependencySpec {
            crate_name: "proc_fixture".to_string(),
            version: None,
            features: Vec::new(),
            default_features: true,
            source: DependencySource::Path {
                path: macro_package.clone(),
            },
            optional: false,
            package: None,
        }],
        Some(&authority),
    )?;
    let proc_macro = libraries
        .iter()
        .find(|library| library.crate_name == "proc_fixture")
        .ok_or("materialized proc macro")?;
    assert_eq!(
        proc_macro.output.extension().and_then(|extension| extension.to_str()),
        Some(std::env::consts::DLL_SUFFIX.trim_start_matches('.'))
    );
    let consumer_source = workspace.path().join("consumer.rs");
    let consumer_output = workspace.path().join("consumer");
    fs::write(
        &consumer_source,
        "use proc_fixture::answer;\nfn main() { assert_eq!(answer!(), 42); }\n",
    )?;
    let status = Command::new(&rustc)
        .arg("--edition=2021")
        .arg("--extern")
        .arg(format!("proc_fixture={}", proc_macro.output.display()))
        .arg("-L")
        .arg(format!(
            "dependency={}",
            proc_macro.output.parent().ok_or("proc macro parent")?.display()
        ))
        .arg(&consumer_source)
        .arg("-o")
        .arg(&consumer_output)
        .status()?;
    assert!(
        status.success(),
        "direct-rustc consumer should load the materialized proc macro"
    );
    assert!(Command::new(consumer_output).status()?.success());
    Ok(())
}
