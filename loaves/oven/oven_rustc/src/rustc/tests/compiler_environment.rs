//! Compiler environment and just-equal compilation regression tests.

use super::*;

#[test]
fn inherited_compiler_controls_are_cleared_before_every_direct_rustc_launch() {
    for ambient in [
        "CARGO",
        "CARGO_HOME",
        "CARGO_ENCODED_RUSTFLAGS",
        "RUSTC_WRAPPER",
        "RUSTC_WORKSPACE_WRAPPER",
        "RUSTFLAGS",
        "RUSTC_BOOTSTRAP",
    ] {
        assert!(
            super::super::direct_rustc_excludes_inherited_environment(std::ffi::OsStr::new(ambient)),
            "`{ambient}` changes what the compiler does without appearing in the recorded invocation"
        );
    }
    for kept in ["RUSTC", "RUSTUP_HOME", "RUSTUP_TOOLCHAIN", "PATH", "HOME", "INCAN_HOME"] {
        assert!(
            !super::super::direct_rustc_excludes_inherited_environment(std::ffi::OsStr::new(kept)),
            "`{kept}` selects or locates the compiler and must survive the scrub"
        );
    }
}
#[test]
fn direct_rustc_launches_remove_an_explicit_cargo_home() {
    let mut command = Command::new("rustc");
    command.env("CARGO_HOME", "custom-cargo-home");

    super::super::clear_inherited_cargo_environment(&mut command);

    assert!(
        command
            .get_envs()
            .any(|(name, value)| { name == OsStr::new("CARGO_HOME") && value.is_none() })
    );
}
#[test]
fn jec_frozen_environment_contains_only_admitted_plan_values() -> Result<(), Box<dyn std::error::Error>> {
    let inherited = [
        (OsString::from("CARGO"), OsString::from("cargo")),
        (OsString::from("CARGO_HOME"), OsString::from("cargo-home")),
        (OsString::from("RUSTC_WRAPPER"), OsString::from("wrapper")),
        (
            OsString::from("RUSTC_WORKSPACE_WRAPPER"),
            OsString::from("workspace-wrapper"),
        ),
        (OsString::from("RUSTFLAGS"), OsString::from("flags")),
        (OsString::from("RUSTC_BOOTSTRAP"), OsString::from("1")),
        (OsString::from("DYLD_LIBRARY_PATH"), OsString::from("ambient-loader")),
        (OsString::from("KEEP"), OsString::from("ambient")),
        (OsString::from("OVERRIDE"), OsString::from("ambient")),
    ];
    let compile_environment = BTreeMap::from([
        ("CARGO_EXPLICIT".to_string(), PathBuf::from("admitted")),
        ("OVERRIDE".to_string(), PathBuf::from("plan")),
    ]);

    let frozen = super::super::direct_compiler::freeze_direct_rustc_environment(inherited, &compile_environment)
        .ok_or("test environment was not representable")?;

    assert_eq!(frozen.get("OVERRIDE"), Some(&PathBuf::from("plan")));
    assert_eq!(frozen.get("CARGO_EXPLICIT"), Some(&PathBuf::from("admitted")));
    for ambient in [
        "CARGO",
        "CARGO_HOME",
        "RUSTC_WRAPPER",
        "RUSTC_WORKSPACE_WRAPPER",
        "RUSTFLAGS",
        "RUSTC_BOOTSTRAP",
        "DYLD_LIBRARY_PATH",
        "KEEP",
    ] {
        assert!(!frozen.contains_key(ambient));
    }
    Ok(())
}
#[cfg(unix)]
#[test]
fn jec_ignores_unrepresentable_ambient_names_and_clears_ambient_execution() -> Result<(), Box<dyn std::error::Error>> {
    use std::os::unix::ffi::OsStringExt;

    let inherited = [(OsString::from_vec(vec![0xff]), OsString::from("value"))];
    assert_eq!(
        super::super::direct_compiler::freeze_direct_rustc_environment(inherited, &BTreeMap::new()),
        Some(BTreeMap::new())
    );
    let mut command = Command::new("/usr/bin/env");
    command.env("AMBIENT_VALUE", "ambient");
    super::super::direct_compiler::apply_direct_rustc_execution_environment(&mut command, Some(&BTreeMap::new()))?;
    let output = String::from_utf8(command.output()?.stdout)?;
    assert!(!output.lines().any(|line| line.starts_with("AMBIENT_VALUE=")));
    Ok(())
}
#[cfg(unix)]
#[test]
fn jec_execution_environment_is_identical_with_or_without_cache_observation() -> Result<(), Box<dyn std::error::Error>>
{
    let compile_environment = BTreeMap::from([("PLAN_VALUE".to_string(), PathBuf::from("plan"))]);
    let frozen_environment = super::super::direct_compiler::freeze_direct_rustc_environment(
        [(OsString::from("FROZEN_VALUE"), OsString::from("frozen"))],
        &compile_environment,
    )
    .ok_or("test environment was not representable")?;

    let mut cacheable = Command::new("/usr/bin/env");
    cacheable.env("LEGACY_VALUE", "ambient");
    super::super::direct_compiler::apply_direct_rustc_execution_environment(&mut cacheable, Some(&frozen_environment))?;
    let cacheable_output = String::from_utf8(cacheable.output()?.stdout)?;
    assert!(cacheable_output.lines().any(|line| line == "PLAN_VALUE=plan"));
    assert!(!cacheable_output.lines().any(|line| line.starts_with("FROZEN_VALUE=")));
    assert!(!cacheable_output.lines().any(|line| line.starts_with("LEGACY_VALUE=")));

    let mut uncacheable = Command::new("/usr/bin/env");
    uncacheable.env("LEGACY_VALUE", "ambient");
    super::super::direct_compiler::apply_direct_rustc_execution_environment(
        &mut uncacheable,
        Some(&frozen_environment),
    )?;
    let uncacheable_output = String::from_utf8(uncacheable.output()?.stdout)?;
    assert!(uncacheable_output.lines().any(|line| line == "PLAN_VALUE=plan"));
    assert!(!uncacheable_output.lines().any(|line| line.starts_with("FROZEN_VALUE=")));
    assert!(!uncacheable_output.lines().any(|line| line.starts_with("LEGACY_VALUE=")));
    assert_eq!(cacheable_output, uncacheable_output);
    Ok(())
}
#[test]
fn jec_dep_info_keeps_only_logical_files_and_secret_digests() -> Result<(), Box<dyn std::error::Error>> {
    let environment = BTreeMap::from([("PRIVATE_VALUE".to_string(), PathBuf::from("do-not-retain-me"))]);
    let bytes = b"libfixture.rlib: incan-source/src/lib.rs incan-source/src/space\\ file.rs \\\n incan-source/src/dollar$$file.rs\n\n# env-dep:ABSENT\n# env-dep:PRIVATE_VALUE=do-not-retain-me\n";
    let observed = super::super::direct_compiler::parse_rustc_dep_info(bytes, &environment)?;
    assert_eq!(
        observed.files,
        [
            PathBuf::from("incan-source/src/dollar$file.rs"),
            PathBuf::from("incan-source/src/lib.rs"),
            PathBuf::from("incan-source/src/space file.rs"),
        ]
    );
    assert_eq!(observed.environment.len(), 2);
    assert_eq!(observed.environment[0].name, "ABSENT");
    assert_eq!(observed.environment[0].value_digest, None);
    assert_eq!(observed.environment[1].name, "PRIVATE_VALUE");
    assert_eq!(
        observed.environment[1].value_digest.as_deref(),
        Some(digest_bytes(b"do-not-retain-me").as_str())
    );
    assert!(!format!("{observed:?}").contains("do-not-retain-me"));

    let mismatched = b"libfixture.rlib: incan-source/src/lib.rs\n# env-dep:PRIVATE_VALUE=different\n";
    assert!(super::super::direct_compiler::parse_rustc_dep_info(mismatched, &environment).is_err());
    Ok(())
}
#[test]
fn jec_compiler_evidence_hashes_the_selected_target_closure() -> Result<(), Box<dyn std::error::Error>> {
    let rustc = rustc_path()?;
    let target = rustc_host_target(&rustc)?;
    let evidence = super::super::direct_compiler::retention::direct_rustc_compiler_evidence(&rustc, &target)?;
    assert_eq!(evidence.host(), target);
    assert_eq!(evidence.target(), target);
    assert_eq!(evidence.binary_digest(), digest_bytes(&fs::read(&rustc)?));
    assert!(evidence.closure_digest().starts_with("sha256:"));
    assert_eq!(evidence.closure_digest().len(), 71);
    Ok(())
}
#[cfg(unix)]
#[test]
fn jec_compiler_owner_is_reused_across_receipts_and_lease_protected() -> Result<(), Box<dyn std::error::Error>> {
    use std::os::unix::fs::PermissionsExt;

    let root = tempfile::tempdir()?;
    let sysroot = root.path().join("toolchain");
    let rustc = sysroot.join("bin/rustc");
    fs::create_dir_all(sysroot.join("bin"))?;
    fs::create_dir_all(sysroot.join("lib/rustlib/test-target/lib"))?;
    fs::write(
        &rustc,
        r#"#!/bin/sh
if [ "$1" = "--print" ] && [ "$2" = "sysroot" ]; then
  cd "$(dirname "$0")/.." || exit 1
  pwd -P
elif [ "$1" = "--version" ]; then
  printf '%s\n' 'rustc 1.99.0-test'
elif [ "$1" = "-vV" ]; then
  printf '%s\n' 'rustc 1.99.0-test' 'host: test-target'
else
  exit 2
fi
"#,
    )?;
    fs::set_permissions(&rustc, fs::Permissions::from_mode(0o755))?;
    let ambient_driver = sysroot.join("lib/libdriver.dylib");
    fs::write(&ambient_driver, b"driver bytes")?;
    fs::write(
        sysroot.join("lib/rustlib/test-target/lib/libstd-test.rlib"),
        b"standard-library bytes",
    )?;
    let source = root.path().join("generated.rs");
    fs::write(&source, "pub fn generated() {}\n")?;
    let receipt = |version: &str| {
        oven_store::receipt_generated_project(
            &oven_store::OvenGeneratedProjectRequest::new(
                root.path(),
                "compiler-owner-fixture",
                version,
                "test-target",
                "rustc 1.99.0-test",
                "debug",
                Vec::new(),
            )
            .with_generated_source("generated-root", &source),
        )
    };
    let store = OvenStore::new(
        root.path().join("store"),
        OvenStoreLimits::new(4 * 1024 * 1024, 4 * 1024 * 1024, 4 * 1024 * 1024),
    );

    let no_capacity = OvenStore::new(root.path().join("no-capacity"), OvenStoreLimits::new(0, 0, 0));
    let unavailable = super::super::direct_compiler::retention::retain_direct_rustc_compiler(
        &no_capacity,
        &receipt("0.1.0")?,
        &rustc,
        "test-target",
    )?;
    let super::super::direct_compiler::OvenDirectRustcCompilerRetention::Unavailable { reason } = unavailable else {
        return Err("zero-capacity Store unexpectedly retained a compiler owner".into());
    };
    assert!(
        reason.contains("cannot publish the selected compiler closure"),
        "{reason}"
    );

    let super::super::direct_compiler::OvenDirectRustcCompilerRetention::Retained(first) =
        super::super::direct_compiler::retention::retain_direct_rustc_compiler(
            &store,
            &receipt("0.1.0")?,
            &rustc,
            "test-target",
        )?
    else {
        return Err("compiler owner was not retained".into());
    };
    assert_ne!(first.rustc(), rustc);
    assert!(first.rustc().starts_with(store.root().canonicalize()?));
    assert_eq!(
        super::super::rustc_sysroot(first.rustc())?.canonicalize()?,
        first.evidence.sysroot
    );
    let first_identity = first.owner.manifest.identity.clone();

    // A warm Store compiler is the batch authority. A mutable ambient sysroot that still reports the same
    // host/toolchain must neither be rescanned nor silently replace the already admitted closure for a new
    // project-version receipt.
    fs::write(&ambient_driver, b"different ambient driver bytes")?;
    let changed_ambient =
        super::super::direct_compiler::retention::direct_rustc_compiler_evidence(&rustc, "test-target")?;
    assert_ne!(changed_ambient.closure_digest(), first.evidence.closure_digest());
    fs::write(&rustc, "#!/bin/sh\nexit 91\n")?;

    let super::super::direct_compiler::OvenDirectRustcCompilerRetention::Retained(second) =
        super::super::direct_compiler::retention::retain_direct_rustc_compiler(
            &store,
            &receipt("0.2.0")?,
            &rustc,
            "test-target",
        )?
    else {
        return Err("compatible compiler owner was not reused".into());
    };
    assert_eq!(second.owner.manifest.identity, first_identity);
    let inspection = store.inspect()?;
    assert_eq!(inspection.entries.len(), 1);
    assert!(inspection.active_lease_physical_bytes > 0);

    let bounded = OvenStore::new(store.root(), OvenStoreLimits::new(0, 0, 0));
    let blocked = bounded.prune()?;
    assert!(blocked.skipped_active_entries.contains(&first_identity));
    assert_eq!(bounded.inspect()?.entries.len(), 1);

    drop(first);
    drop(second);
    let reclaimed = bounded.prune()?;
    assert!(reclaimed.removed_entries.contains(&first_identity));
    assert!(bounded.inspect()?.entries.is_empty());
    Ok(())
}
#[test]
fn jec_bound_rlib_is_byte_identical_after_source_relocation() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let rustc = rustc_path()?;
    let target = rustc_host_target(&rustc)?;
    let rustc_digest = digest_bytes(&fs::read(&rustc)?);
    let sysroot = super::super::rustc_sysroot(&rustc)?.canonicalize()?;
    let source = "pub fn answer() -> u32 { 42 }\n";
    let source_digest = digest_bytes(source.as_bytes());
    let compiler = super::super::direct_compiler::OvenDirectRustcCompilerEvidence {
        binary_digest: rustc_digest,
        closure_digest: digest_bytes(b"test compiler closure"),
        host: target.clone(),
        target: target.clone(),
        sysroot,
        members: Vec::new(),
    };
    let bindings = super::super::direct_compiler::OvenDirectRustcJecBindings {
        logical_source_root: "incan-source".to_string(),
        logical_entrypoint: "src/lib.rs".to_string(),
        externs: Vec::new(),
        dependency_searches: Vec::new(),
        native_searches: Vec::new(),
    };
    let mut outputs = Vec::new();
    let mut observations = Vec::new();
    for name in ["first", "second"] {
        let source_root = root.path().join(name);
        let artifact_root = root.path().join(format!("{name}-artifacts"));
        let output = root.path().join(format!("{name}-output/libfixture.rlib"));
        fs::create_dir_all(source_root.join("src"))?;
        fs::create_dir(&artifact_root)?;
        fs::write(source_root.join("src/lib.rs"), source)?;
        let prepared = super::super::direct_compiler::OvenPreparedDirectRustcLibrary {
            rustc: rustc.clone(),
            compiler: Some(compiler.clone()),
            compiler_owner: None,
            source: source_root.join("src/lib.rs").canonicalize()?,
            source_root: source_root.canonicalize()?,
            source_digest: source_digest.clone(),
            artifact_root: artifact_root.canonicalize()?,
            selected_artifacts: OvenRustcArtifactManifest {
                schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
                intent: oven_store::OvenBuildIntent {
                    target: target.clone(),
                    toolchain: rustc_identity(&rustc)?,
                    profile: "debug".to_string(),
                    features: Vec::new(),
                },
                dependency_search_paths: Vec::new(),
                native_search_paths: Vec::new(),
                externs: Vec::new(),
                entrypoint_externs: BTreeMap::new(),
                registry_leaves: Vec::new(),
                registry_sources: Vec::new(),
                compile_environment: BTreeMap::new(),
                vocab_auxiliary_targets: Vec::new(),
                supporting_artifacts: Vec::new(),
                entrypoint_dependency_search_paths: Default::default(),
            },
            plan: OvenRustcArtifactPlan {
                dependency_search_paths: Vec::new(),
                native_search_paths: Vec::new(),
                externs: Vec::new(),
                compile_environment: BTreeMap::new(),
                caller_owned_library_digests: BTreeMap::new(),
                source_path_projection: None,
            },
            target: target.clone(),
            profile: "debug".to_string(),
            crate_name: "fixture".to_string(),
            edition: "2021".to_string(),
            features: Vec::new(),
            frozen_environment: Some(
                super::super::direct_compiler::freeze_direct_rustc_environment(std::env::vars_os(), &BTreeMap::new())
                    .ok_or("test environment was not representable")?,
            ),
        };
        let compiled = prepared.bind(&output, &bindings)?.compile()?;
        outputs.push(fs::read(&compiled.bake.output)?);
        let super::super::direct_compiler::OvenRustcDepInfoOutcome::Observed(observation) = compiled.observation else {
            return Err("relocated Rustc compilation did not provide dep-info".into());
        };
        observations.push(observation.files);
    }
    assert_eq!(outputs[0], outputs[1]);
    assert_ne!(observations[0], observations[1]);
    assert!(
        observations
            .iter()
            .all(|paths| paths.len() == 1 && paths[0].ends_with(Path::new("src/lib.rs")))
    );
    Ok(())
}
#[test]
fn jec_bound_rlib_is_byte_identical_after_dependency_relocation() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let rustc = rustc_path()?;
    let target = rustc_host_target(&rustc)?;
    let dependency_source = root.path().join("dependency.rs");
    let dependency_output = root.path().join("libfixture_dep.rlib");
    fs::write(&dependency_source, "pub fn answer() -> u32 { 42 }\n")?;
    let status = Command::new(&rustc)
        .env_clear()
        .args([
            "--crate-name",
            "fixture_dep",
            "--crate-type",
            "rlib",
            "--edition=2021",
            "--target",
            &target,
        ])
        .arg(&dependency_source)
        .arg("-o")
        .arg(&dependency_output)
        .status()?;
    if !status.success() {
        return Err("fixture dependency compilation failed".into());
    }
    let source = "pub fn answer() -> u32 { fixture_dep::answer() }\n";
    let source_digest = digest_bytes(source.as_bytes());
    let compiler = super::super::direct_compiler::OvenDirectRustcCompilerEvidence {
        binary_digest: digest_bytes(&fs::read(&rustc)?),
        closure_digest: digest_bytes(b"test compiler closure"),
        host: target.clone(),
        target: target.clone(),
        sysroot: super::super::rustc_sysroot(&rustc)?.canonicalize()?,
        members: Vec::new(),
    };
    let bindings = super::super::direct_compiler::OvenDirectRustcJecBindings {
        logical_source_root: "incan-source".to_string(),
        logical_entrypoint: "src/lib.rs".to_string(),
        externs: vec!["extern/0000".to_string()],
        dependency_searches: vec!["dependency/0000".to_string()],
        native_searches: Vec::new(),
    };
    let mut outputs = Vec::new();
    for name in ["first", "second"] {
        let source_root = root.path().join(name).join("source");
        let artifact_root = root.path().join(name).join("artifacts");
        let dependency_root = artifact_root.join("deps");
        let relocated_dependency = dependency_root.join("libfixture_dep.rlib");
        let output = root.path().join(name).join("output/libfixture.rlib");
        fs::create_dir_all(source_root.join("src"))?;
        fs::create_dir_all(&dependency_root)?;
        fs::write(source_root.join("src/lib.rs"), source)?;
        fs::copy(&dependency_output, &relocated_dependency)?;
        let dependency_root = dependency_root.canonicalize()?;
        let relocated_dependency = relocated_dependency.canonicalize()?;
        let prepared = super::super::direct_compiler::OvenPreparedDirectRustcLibrary {
            rustc: rustc.clone(),
            compiler: Some(compiler.clone()),
            compiler_owner: None,
            source: source_root.join("src/lib.rs").canonicalize()?,
            source_root: source_root.canonicalize()?,
            source_digest: source_digest.clone(),
            artifact_root: artifact_root.canonicalize()?,
            selected_artifacts: OvenRustcArtifactManifest {
                schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
                intent: oven_store::OvenBuildIntent {
                    target: target.clone(),
                    toolchain: rustc_identity(&rustc)?,
                    profile: "debug".to_string(),
                    features: Vec::new(),
                },
                dependency_search_paths: Vec::new(),
                native_search_paths: Vec::new(),
                externs: Vec::new(),
                entrypoint_externs: BTreeMap::new(),
                registry_leaves: Vec::new(),
                registry_sources: Vec::new(),
                compile_environment: BTreeMap::new(),
                vocab_auxiliary_targets: Vec::new(),
                supporting_artifacts: Vec::new(),
                entrypoint_dependency_search_paths: Default::default(),
            },
            plan: OvenRustcArtifactPlan {
                dependency_search_paths: vec![dependency_root],
                native_search_paths: Vec::new(),
                externs: vec![("fixture_dep".to_string(), relocated_dependency)],
                compile_environment: BTreeMap::new(),
                caller_owned_library_digests: BTreeMap::new(),
                source_path_projection: None,
            },
            target: target.clone(),
            profile: "debug".to_string(),
            crate_name: "fixture".to_string(),
            edition: "2021".to_string(),
            features: Vec::new(),
            frozen_environment: Some(
                super::super::direct_compiler::freeze_direct_rustc_environment(std::env::vars_os(), &BTreeMap::new())
                    .ok_or("test environment was not representable")?,
            ),
        };
        let compiled = prepared.bind(&output, &bindings)?.compile()?;
        outputs.push(fs::read(&compiled.bake.output)?);
    }
    assert_eq!(outputs[0], outputs[1]);
    Ok(())
}
