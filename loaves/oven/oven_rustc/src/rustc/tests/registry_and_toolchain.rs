//! Registry-leaf selection, profile, and toolchain regression tests.

use super::*;

#[test]
fn selects_the_highest_sealed_registry_leaf_matching_the_declared_requirement() -> Result<(), Box<dyn std::error::Error>>
{
    let registry = tempfile::tempdir()?;
    let mut leaves = Vec::new();
    for version in ["1.0.8", "1.0.18", "2.0.0"] {
        let artifact = registry.path().join(format!("libitoa-{version}.rlib"));
        let bytes = format!("sealed itoa {version}").into_bytes();
        fs::write(&artifact, &bytes)?;
        leaves.push(OvenRustcRegistryLeaf {
            domain: Default::default(),
            crate_kind: Default::default(),
            selected_unit_identity: None,
            package: "itoa".to_string(),
            version: version.to_string(),
            crate_name: "itoa".to_string(),
            features: if version == "1.0.18" {
                vec!["std".to_string()]
            } else {
                Vec::new()
            },
            source: fixture_registry_source(),
            artifact: OvenRustcArtifactExtern {
                crate_name: "itoa".to_string(),
                relative_path: artifact
                    .file_name()
                    .and_then(|name| name.to_str())
                    .ok_or("registry artifact name")?
                    .to_string(),
                digest: digest_bytes(&bytes),
            },
        });
    }
    let authority = OvenRegistryLeafAuthority::new(registry.path().to_path_buf(), leaves);
    let dependency = DependencySpec {
        crate_name: "itoa".to_string(),
        version: Some("1".to_string()),
        features: vec!["std".to_string()],
        default_features: true,
        source: DependencySource::Registry,
        optional: false,
        package: None,
    };
    let selected = resolve_sealed_registry_leaf(&dependency, Some(&authority), "debug")?;
    assert_eq!(
        selected.file_name().and_then(|name| name.to_str()),
        Some("libitoa-1.0.18.rlib")
    );
    let unavailable_dependency = DependencySpec {
        crate_name: "itoa".to_string(),
        version: Some("1".to_string()),
        features: vec!["alloc".to_string()],
        default_features: true,
        source: DependencySource::Registry,
        optional: false,
        package: None,
    };
    let unavailable_feature = resolve_sealed_registry_leaf(&unavailable_dependency, Some(&authority), "debug");
    assert!(matches!(unavailable_feature, Err(OvenRustcError::InvalidInput { .. })));
    Ok(())
}

#[test]
fn host_and_proc_macro_leaves_sit_beside_target_leaves_without_being_selected() -> Result<(), Box<dyn std::error::Error>>
{
    let registry = tempfile::tempdir()?;
    let mut leaves = Vec::new();
    for (relative_path, domain, crate_kind, crate_name) in [
        (
            "target/x/debug/deps/libshared-target.rlib",
            super::super::OvenRustcRegistryLeafDomain::Target,
            super::super::OvenRustcRegistryLeafKind::Rlib,
            "shared",
        ),
        (
            "target/debug/deps/libshared-host.rlib",
            super::super::OvenRustcRegistryLeafDomain::Host,
            super::super::OvenRustcRegistryLeafKind::Rlib,
            "shared",
        ),
        (
            "target/debug/deps/libshared_derive.dylib",
            super::super::OvenRustcRegistryLeafDomain::Host,
            super::super::OvenRustcRegistryLeafKind::ProcMacro,
            "shared_derive",
        ),
    ] {
        let artifact = registry.path().join(relative_path);
        fs::create_dir_all(artifact.parent().ok_or("artifact parent")?)?;
        fs::write(&artifact, relative_path.as_bytes())?;
        leaves.push(OvenRustcRegistryLeaf {
            domain,
            crate_kind,
            selected_unit_identity: None,
            package: if crate_kind == super::super::OvenRustcRegistryLeafKind::ProcMacro {
                "shared_derive"
            } else {
                "shared"
            }
            .to_string(),
            version: "1.0.0".to_string(),
            crate_name: crate_name.to_string(),
            features: vec!["default".to_string()],
            source: fixture_registry_source(),
            artifact: OvenRustcArtifactExtern {
                crate_name: crate_name.to_string(),
                relative_path: relative_path.to_string(),
                digest: digest_bytes(relative_path.as_bytes()),
            },
        });
    }
    let authority = OvenRegistryLeafAuthority::new(registry.path().to_path_buf(), leaves);
    let dependency = DependencySpec {
        crate_name: "shared".to_string(),
        version: Some("1".to_string()),
        features: vec!["default".to_string()],
        default_features: true,
        source: DependencySource::Registry,
        optional: false,
        package: None,
    };
    let selected = super::super::select_sealed_registry_leaf(&dependency, Some(&authority), "debug")?;
    assert_eq!(selected.leaf.domain, super::super::OvenRustcRegistryLeafDomain::Target);
    assert_eq!(
        selected.leaf.artifact.relative_path,
        "target/x/debug/deps/libshared-target.rlib"
    );
    Ok(())
}

#[test]
fn preferred_runtime_cohort_replaces_only_overlapping_target_registry_units() -> Result<(), Box<dyn std::error::Error>>
{
    let leaf = |package: &str, version: &str, domain, crate_kind, suffix: &str| OvenRustcRegistryLeaf {
        domain,
        crate_kind,
        selected_unit_identity: Some(format!("sha256:{suffix}-unit")),
        package: package.to_string(),
        version: version.to_string(),
        crate_name: package.replace('-', "_"),
        features: vec!["default".to_string()],
        source: fixture_registry_source(),
        artifact: OvenRustcArtifactExtern {
            crate_name: package.replace('-', "_"),
            relative_path: format!("target/debug/deps/lib{package}-{suffix}.rlib"),
            digest: format!("sha256:{suffix}"),
        },
    };
    let project = OvenRegistryLeafAuthority::new(
        PathBuf::from("/project"),
        vec![
            leaf(
                "serde",
                "1.0.229",
                OvenRustcRegistryLeafDomain::Target,
                OvenRustcRegistryLeafKind::Rlib,
                "project-serde",
            ),
            leaf(
                "project-only",
                "2.0.0",
                OvenRustcRegistryLeafDomain::Target,
                OvenRustcRegistryLeafKind::Rlib,
                "project-only",
            ),
            leaf(
                "serde",
                "1.0.229",
                OvenRustcRegistryLeafDomain::Host,
                OvenRustcRegistryLeafKind::Rlib,
                "host-serde",
            ),
        ],
    );
    let release = OvenRegistryLeafAuthority::new(
        PathBuf::from("/release"),
        vec![leaf(
            "serde",
            "1.0.228",
            OvenRustcRegistryLeafDomain::Target,
            OvenRustcRegistryLeafKind::Rlib,
            "release-serde",
        )],
    );
    let coherent = project.with_preferred_target_cohort(&release);
    let dependency = |crate_name: &str| DependencySpec {
        crate_name: crate_name.to_string(),
        version: Some("*".to_string()),
        features: vec!["default".to_string()],
        default_features: true,
        source: DependencySource::Registry,
        optional: false,
        package: None,
    };

    assert_eq!(
        super::super::select_sealed_registry_leaf(&dependency("serde"), Some(&coherent), "debug")?
            .leaf
            .version,
        "1.0.228",
        "the linked release runtime and serde remain one compiled cohort"
    );
    assert_eq!(
        super::super::select_sealed_registry_leaf(&dependency("project-only"), Some(&coherent), "debug")?
            .leaf
            .version,
        "2.0.0",
        "packages outside the release cohort remain supplied by the project"
    );
    Ok(())
}

#[test]
fn validates_the_exact_selected_registry_extern_instead_of_reselecting_highest_semver()
-> Result<(), Box<dyn std::error::Error>> {
    let registry = tempfile::tempdir()?;
    let dependency_directory = registry.path().join("target/debug/deps");
    fs::create_dir_all(&dependency_directory)?;
    let mut leaves = Vec::new();
    let mut artifacts = BTreeMap::new();
    for version in ["1.2.0", "1.8.0"] {
        let relative_path = format!("target/debug/deps/libshared-{version}.rlib");
        let artifact = registry.path().join(&relative_path);
        let bytes = format!("sealed shared {version}").into_bytes();
        fs::write(&artifact, &bytes)?;
        artifacts.insert(version, fs::canonicalize(&artifact)?);
        leaves.push(OvenRustcRegistryLeaf {
            domain: Default::default(),
            crate_kind: Default::default(),
            selected_unit_identity: None,
            package: "shared".to_string(),
            version: version.to_string(),
            crate_name: "shared".to_string(),
            features: vec!["default".to_string()],
            source: fixture_registry_source(),
            artifact: OvenRustcArtifactExtern {
                crate_name: "shared".to_string(),
                relative_path,
                digest: digest_bytes(&bytes),
            },
        });
    }
    let authority = OvenRegistryLeafAuthority::new(registry.path().to_path_buf(), leaves);
    let dependency = DependencySpec {
        crate_name: "shared_old".to_string(),
        version: Some("1".to_string()),
        features: vec!["default".to_string()],
        default_features: true,
        source: DependencySource::Registry,
        optional: false,
        package: Some("shared".to_string()),
    };

    assert_eq!(
        super::super::select_sealed_registry_leaf(&dependency, Some(&authority), "debug")?
            .leaf
            .version,
        "1.8.0",
        "the compatibility catalog retains its existing highest-semver behavior"
    );
    super::super::validate_selected_sealed_registry_leaf(
        &dependency,
        artifacts.get("1.2.0").ok_or("old artifact missing")?,
        Some(&authority),
        "debug",
    )?;
    let wrong_profile = super::super::validate_selected_sealed_registry_leaf(
        &dependency,
        artifacts.get("1.2.0").ok_or("old artifact missing")?,
        Some(&authority),
        "release",
    );
    assert!(matches!(wrong_profile, Err(OvenRustcError::InvalidInput { .. })));
    Ok(())
}

#[test]
fn aggregates_compatible_registry_catalogs_without_losing_the_leaf_root() -> Result<(), Box<dyn std::error::Error>> {
    let narrow = tempfile::tempdir()?;
    let broad = tempfile::tempdir()?;
    let narrow_artifact = narrow.path().join("libbitflags-v2.rlib");
    let narrow_bytes = b"sealed bitflags 2.13.1";
    fs::write(&narrow_artifact, narrow_bytes)?;
    let broad_artifact = broad.path().join("libbitflags-v1.rlib");
    let broad_bytes = b"sealed bitflags 1.3.2";
    fs::write(&broad_artifact, broad_bytes)?;
    let authority = OvenRegistryLeafAuthority::aggregate([
        OvenRegistryLeafAuthority::new(
            narrow.path().to_path_buf(),
            vec![OvenRustcRegistryLeaf {
                domain: Default::default(),
                crate_kind: Default::default(),
                selected_unit_identity: None,
                package: "bitflags".to_string(),
                version: "2.13.1".to_string(),
                crate_name: "bitflags".to_string(),
                features: Vec::new(),
                source: fixture_registry_source(),
                artifact: OvenRustcArtifactExtern {
                    crate_name: "bitflags".to_string(),
                    relative_path: "libbitflags-v2.rlib".to_string(),
                    digest: digest_bytes(narrow_bytes),
                },
            }],
        ),
        OvenRegistryLeafAuthority::new(
            broad.path().to_path_buf(),
            vec![OvenRustcRegistryLeaf {
                domain: Default::default(),
                crate_kind: Default::default(),
                selected_unit_identity: None,
                package: "bitflags".to_string(),
                version: "1.3.2".to_string(),
                crate_name: "bitflags".to_string(),
                features: Vec::new(),
                source: fixture_registry_source(),
                artifact: OvenRustcArtifactExtern {
                    crate_name: "bitflags".to_string(),
                    relative_path: "libbitflags-v1.rlib".to_string(),
                    digest: digest_bytes(broad_bytes),
                },
            }],
        ),
    ]);
    let dependency = DependencySpec {
        crate_name: "bitflags".to_string(),
        version: Some("=1.3.2".to_string()),
        features: Vec::new(),
        default_features: true,
        source: DependencySource::Registry,
        optional: false,
        package: None,
    };

    assert_eq!(
        resolve_sealed_registry_leaf(&dependency, Some(&authority), "debug")?,
        fs::canonicalize(broad_artifact)?
    );
    Ok(())
}

#[test]
fn aggregate_admits_a_provider_package_the_consumer_never_declared() -> Result<(), Box<dyn std::error::Error>> {
    let consumer_root = tempfile::tempdir()?;
    let provider_root = tempfile::tempdir()?;
    let provider_artifact = provider_root.path().join("libdatafusion.rlib");
    let provider_bytes = b"sealed datafusion 53.1.0";
    fs::write(&provider_artifact, provider_bytes)?;
    let consumer_authority = OvenRegistryLeafAuthority::new(consumer_root.path().to_path_buf(), Vec::new());
    let provider_authority = OvenRegistryLeafAuthority::new(
        provider_root.path().to_path_buf(),
        vec![OvenRustcRegistryLeaf {
            domain: Default::default(),
            crate_kind: Default::default(),
            selected_unit_identity: None,
            package: "datafusion".to_string(),
            version: "53.1.0".to_string(),
            crate_name: "datafusion".to_string(),
            features: Vec::new(),
            source: fixture_registry_source(),
            artifact: OvenRustcArtifactExtern {
                crate_name: "datafusion".to_string(),
                relative_path: "libdatafusion.rlib".to_string(),
                digest: digest_bytes(provider_bytes),
            },
        }],
    );
    let joined = OvenRegistryLeafAuthority::aggregate([consumer_authority, provider_authority]);
    let dependency = DependencySpec {
        crate_name: "datafusion".to_string(),
        version: Some("53".to_string()),
        features: Vec::new(),
        default_features: true,
        source: DependencySource::Registry,
        optional: false,
        package: None,
    };
    assert_eq!(
        resolve_sealed_registry_leaf(&dependency, Some(&joined), "debug")?,
        fs::canonicalize(provider_artifact)?
    );
    Ok(())
}

#[test]
fn aggregate_does_not_block_an_unrelated_lookup_when_a_never_requested_package_conflicts()
-> Result<(), Box<dyn std::error::Error>> {
    // Regression coverage for a real false positive: a provider and consumer can each carry their own build of
    // some common transitive crate (for example `memchr`, pulled in independently by unrelated dependencies on
    // each side) that nobody ever actually resolves through this authority. Joining the two catalogs must not
    // block resolution of a package that IS actually requested and IS only on one side.
    let consumer_root = tempfile::tempdir()?;
    let provider_root = tempfile::tempdir()?;
    let consumer_bytes = b"sealed memchr 2.8.0 consumer build";
    let provider_memchr_bytes = b"sealed memchr 2.8.0 provider build";
    let provider_datafusion_bytes = b"sealed datafusion 53.1.0";
    fs::write(consumer_root.path().join("libmemchr.rlib"), consumer_bytes)?;
    fs::write(provider_root.path().join("libmemchr.rlib"), provider_memchr_bytes)?;
    let provider_datafusion = provider_root.path().join("libdatafusion.rlib");
    fs::write(&provider_datafusion, provider_datafusion_bytes)?;
    let memchr_leaf = |bytes: &[u8]| OvenRustcRegistryLeaf {
        domain: Default::default(),
        crate_kind: Default::default(),
        selected_unit_identity: None,
        package: "memchr".to_string(),
        version: "2.8.0".to_string(),
        crate_name: "memchr".to_string(),
        features: Vec::new(),
        source: fixture_registry_source(),
        artifact: OvenRustcArtifactExtern {
            crate_name: "memchr".to_string(),
            relative_path: "libmemchr.rlib".to_string(),
            digest: digest_bytes(bytes),
        },
    };
    let consumer_authority =
        OvenRegistryLeafAuthority::new(consumer_root.path().to_path_buf(), vec![memchr_leaf(consumer_bytes)]);
    let provider_authority = OvenRegistryLeafAuthority::new(
        provider_root.path().to_path_buf(),
        vec![
            memchr_leaf(provider_memchr_bytes),
            OvenRustcRegistryLeaf {
                domain: Default::default(),
                crate_kind: Default::default(),
                selected_unit_identity: None,
                package: "datafusion".to_string(),
                version: "53.1.0".to_string(),
                crate_name: "datafusion".to_string(),
                features: Vec::new(),
                source: fixture_registry_source(),
                artifact: OvenRustcArtifactExtern {
                    crate_name: "datafusion".to_string(),
                    relative_path: "libdatafusion.rlib".to_string(),
                    digest: digest_bytes(provider_datafusion_bytes),
                },
            },
        ],
    );
    let joined = OvenRegistryLeafAuthority::aggregate([consumer_authority, provider_authority]);
    let datafusion_dependency = DependencySpec {
        crate_name: "datafusion".to_string(),
        version: Some("53".to_string()),
        features: Vec::new(),
        default_features: true,
        source: DependencySource::Registry,
        optional: false,
        package: None,
    };
    assert_eq!(
        resolve_sealed_registry_leaf(&datafusion_dependency, Some(&joined), "debug")?,
        fs::canonicalize(&provider_datafusion)?,
        "an unrelated conflicting memchr entry must not block resolving datafusion"
    );
    Ok(())
}

#[test]
fn aggregate_fails_closed_when_the_actually_requested_package_conflicts() -> Result<(), Box<dyn std::error::Error>> {
    let consumer_root = tempfile::tempdir()?;
    let provider_root = tempfile::tempdir()?;
    let consumer_bytes = b"sealed tokio 1.52.3 rt-multi-thread,macros,time,sync,net";
    let provider_bytes = b"sealed tokio 1.52.3 full";
    // Real rustc output embeds a metadata hash in the filename that differs whenever the compiled configuration
    // differs, which is what `same_compilation` actually keys off; give the two conflicting artifacts distinct
    // names here so this fixture matches that shape instead of coincidentally looking like the same compilation.
    fs::write(consumer_root.path().join("libtokio-consumer1234.rlib"), consumer_bytes)?;
    fs::write(provider_root.path().join("libtokio-provider5678.rlib"), provider_bytes)?;
    let leaf = |features: &[&str], bytes: &[u8], relative_path: &str| OvenRustcRegistryLeaf {
        domain: Default::default(),
        crate_kind: Default::default(),
        selected_unit_identity: None,
        package: "tokio".to_string(),
        version: "1.52.3".to_string(),
        crate_name: "tokio".to_string(),
        features: features.iter().map(|feature| feature.to_string()).collect(),
        source: fixture_registry_source(),
        artifact: OvenRustcArtifactExtern {
            crate_name: "tokio".to_string(),
            relative_path: relative_path.to_string(),
            digest: digest_bytes(bytes),
        },
    };
    let consumer_authority = OvenRegistryLeafAuthority::new(
        consumer_root.path().to_path_buf(),
        vec![leaf(
            &["rt-multi-thread", "macros", "time", "sync", "net"],
            consumer_bytes,
            "libtokio-consumer1234.rlib",
        )],
    );
    let provider_authority = OvenRegistryLeafAuthority::new(
        provider_root.path().to_path_buf(),
        vec![leaf(&["full"], provider_bytes, "libtokio-provider5678.rlib")],
    );
    let joined = OvenRegistryLeafAuthority::aggregate([consumer_authority, provider_authority]);
    let dependency = DependencySpec {
        crate_name: "tokio".to_string(),
        version: Some("1".to_string()),
        features: Vec::new(),
        default_features: true,
        source: DependencySource::Registry,
        optional: false,
        package: None,
    };
    let error = resolve_sealed_registry_leaf(&dependency, Some(&joined), "debug")
        .expect_err("resolving a package that genuinely disagrees between two joined authorities must fail closed");
    assert!(matches!(error, OvenRustcError::InvalidInput { .. }));
    Ok(())
}

#[test]
fn first_conflicting_package_with_reports_a_provider_leaf_that_disagrees_with_an_existing_extern()
-> Result<(), Box<dyn std::error::Error>> {
    // Reproduces the exact real-world defect this check exists for: a caller-owned provider's own registry
    // closure (a query-engine library's own DataFusion/Tokio dependency graph) resolves a package the consumer
    // already links explicitly (the SDK's own `block_on` support) to a different compiled artifact. Left
    // unchecked, both get linked into one binary as two distinct compiled `tokio` instances -- confirmed by
    // inspecting a real built executable's symbol table -- and the async runtime state silently splits across
    // them, producing a "no reactor running" panic at runtime instead of a build failure.
    let consumer_root = tempfile::tempdir()?;
    let provider_root = tempfile::tempdir()?;
    let consumer_bytes = b"sealed tokio compiled for the SDK's own block_on closure";
    let provider_bytes = b"sealed tokio compiled for the provider's own DataFusion closure";
    let consumer_artifact = consumer_root.path().join("libtokio-sdk1234.rlib");
    let provider_artifact = provider_root.path().join("libtokio-provider5678.rlib");
    fs::write(&consumer_artifact, consumer_bytes)?;
    fs::write(&provider_artifact, provider_bytes)?;
    let plan = OvenRustcArtifactPlan {
        source_path_projection: None,
        dependency_search_paths: Vec::new(),
        native_search_paths: Vec::new(),
        externs: vec![("tokio".to_string(), consumer_artifact)],
        compile_environment: BTreeMap::new(),
        caller_owned_library_digests: BTreeMap::new(),
    };
    let provider_authority = OvenRegistryLeafAuthority::new(
        provider_root.path().to_path_buf(),
        vec![OvenRustcRegistryLeaf {
            domain: Default::default(),
            crate_kind: Default::default(),
            selected_unit_identity: None,
            package: "tokio".to_string(),
            version: "1.52.3".to_string(),
            crate_name: "tokio".to_string(),
            features: Vec::new(),
            source: fixture_registry_source(),
            artifact: OvenRustcArtifactExtern {
                crate_name: "tokio".to_string(),
                relative_path: "libtokio-provider5678.rlib".to_string(),
                digest: digest_bytes(provider_bytes),
            },
        }],
    );
    assert_eq!(
        provider_authority.first_conflicting_package_with(&plan)?,
        Some("tokio".to_string())
    );
    let host_provider_authority = OvenRegistryLeafAuthority::new(
        provider_root.path().to_path_buf(),
        vec![OvenRustcRegistryLeaf {
            domain: OvenRustcRegistryLeafDomain::Host,
            crate_kind: OvenRustcRegistryLeafKind::Rlib,
            selected_unit_identity: None,
            package: "tokio".to_string(),
            version: "1.52.3".to_string(),
            crate_name: "tokio".to_string(),
            features: Vec::new(),
            source: fixture_registry_source(),
            artifact: OvenRustcArtifactExtern {
                crate_name: "tokio".to_string(),
                relative_path: "libtokio-provider5678.rlib".to_string(),
                digest: digest_bytes(provider_bytes),
            },
        }],
    );
    assert_eq!(
        host_provider_authority.first_conflicting_package_with(&plan)?,
        None,
        "a host library is not linked into the target artifact even when its crate name matches a target extern"
    );
    Ok(())
}

#[test]
fn first_conflicting_package_with_allows_a_byte_identical_provider_leaf() -> Result<(), Box<dyn std::error::Error>> {
    let consumer_root = tempfile::tempdir()?;
    let provider_root = tempfile::tempdir()?;
    let shared_bytes = b"sealed tokio, compiled identically for both closures";
    let consumer_artifact = consumer_root.path().join("libtokio-shared.rlib");
    let provider_artifact = provider_root.path().join("libtokio-shared.rlib");
    fs::write(&consumer_artifact, shared_bytes)?;
    fs::write(&provider_artifact, shared_bytes)?;
    let plan = OvenRustcArtifactPlan {
        source_path_projection: None,
        dependency_search_paths: Vec::new(),
        native_search_paths: Vec::new(),
        externs: vec![("tokio".to_string(), consumer_artifact)],
        compile_environment: BTreeMap::new(),
        caller_owned_library_digests: BTreeMap::new(),
    };
    let provider_authority = OvenRegistryLeafAuthority::new(
        provider_root.path().to_path_buf(),
        vec![OvenRustcRegistryLeaf {
            domain: Default::default(),
            crate_kind: Default::default(),
            selected_unit_identity: None,
            package: "tokio".to_string(),
            version: "1.52.3".to_string(),
            crate_name: "tokio".to_string(),
            features: Vec::new(),
            source: fixture_registry_source(),
            artifact: OvenRustcArtifactExtern {
                crate_name: "tokio".to_string(),
                relative_path: "libtokio-shared.rlib".to_string(),
                digest: digest_bytes(shared_bytes),
            },
        }],
    );
    assert_eq!(
        provider_authority.first_conflicting_package_with(&plan)?,
        None,
        "a provider leaf compiled to the exact same bytes as the existing extern must not be rejected"
    );
    Ok(())
}

#[test]
fn first_conflicting_package_with_follows_the_selected_version_when_crate_names_repeat()
-> Result<(), Box<dyn std::error::Error>> {
    let consumer_root = tempfile::tempdir()?;
    let provider_root = tempfile::tempdir()?;
    let older_bytes = b"sealed widget graph version 7";
    let selected_bytes = b"sealed widget graph version 8";
    let consumer_artifact = consumer_root.path().join("libwidget_graph-consumer8.rlib");
    fs::write(&consumer_artifact, selected_bytes)?;
    fs::write(provider_root.path().join("libwidget_graph-provider7.rlib"), older_bytes)?;
    fs::write(
        provider_root.path().join("libwidget_graph-provider8.rlib"),
        selected_bytes,
    )?;
    let plan = OvenRustcArtifactPlan {
        source_path_projection: None,
        dependency_search_paths: Vec::new(),
        native_search_paths: Vec::new(),
        externs: vec![("widget_graph".to_string(), consumer_artifact)],
        compile_environment: BTreeMap::new(),
        caller_owned_library_digests: BTreeMap::new(),
    };
    let leaf = |version: &str, identity: &str, relative_path: &str, bytes: &[u8]| OvenRustcRegistryLeaf {
        domain: Default::default(),
        crate_kind: Default::default(),
        selected_unit_identity: Some(identity.to_string()),
        package: "widget-graph".to_string(),
        version: version.to_string(),
        crate_name: "widget_graph".to_string(),
        features: Vec::new(),
        source: fixture_registry_source(),
        artifact: OvenRustcArtifactExtern {
            crate_name: "widget_graph".to_string(),
            relative_path: relative_path.to_string(),
            digest: digest_bytes(bytes),
        },
    };
    let consumer_authority = OvenRegistryLeafAuthority::new(
        consumer_root.path().to_path_buf(),
        vec![leaf(
            "8.0.0",
            "sha256:selected-v8",
            "libwidget_graph-consumer8.rlib",
            selected_bytes,
        )],
    );
    let provider_authority = OvenRegistryLeafAuthority::new(
        provider_root.path().to_path_buf(),
        vec![
            leaf(
                "7.1.0",
                "sha256:older-v7",
                "libwidget_graph-provider7.rlib",
                older_bytes,
            ),
            leaf(
                "8.0.0",
                "sha256:selected-v8",
                "libwidget_graph-provider8.rlib",
                selected_bytes,
            ),
        ],
    );

    assert_eq!(
        provider_authority.first_conflicting_package_with_reconciled_authority(&plan, Some(&consumer_authority),)?,
        None,
        "an older same-name unit must not be compared with the exact version selected by the extern edge"
    );
    Ok(())
}

#[test]
fn first_conflicting_package_with_ignores_an_unrelated_package() -> Result<(), Box<dyn std::error::Error>> {
    let consumer_root = tempfile::tempdir()?;
    let provider_root = tempfile::tempdir()?;
    let consumer_bytes = b"sealed tokio for the consumer";
    let provider_bytes = b"sealed datafusion for the provider";
    let consumer_artifact = consumer_root.path().join("libtokio.rlib");
    let provider_artifact = provider_root.path().join("libdatafusion.rlib");
    fs::write(&consumer_artifact, consumer_bytes)?;
    fs::write(&provider_artifact, provider_bytes)?;
    let plan = OvenRustcArtifactPlan {
        source_path_projection: None,
        dependency_search_paths: Vec::new(),
        native_search_paths: Vec::new(),
        externs: vec![("tokio".to_string(), consumer_artifact)],
        compile_environment: BTreeMap::new(),
        caller_owned_library_digests: BTreeMap::new(),
    };
    let provider_authority = OvenRegistryLeafAuthority::new(
        provider_root.path().to_path_buf(),
        vec![OvenRustcRegistryLeaf {
            domain: Default::default(),
            crate_kind: Default::default(),
            selected_unit_identity: None,
            package: "datafusion".to_string(),
            version: "53.1.0".to_string(),
            crate_name: "datafusion".to_string(),
            features: Vec::new(),
            source: fixture_registry_source(),
            artifact: OvenRustcArtifactExtern {
                crate_name: "datafusion".to_string(),
                relative_path: "libdatafusion.rlib".to_string(),
                digest: digest_bytes(provider_bytes),
            },
        }],
    );
    assert_eq!(provider_authority.first_conflicting_package_with(&plan)?, None);
    Ok(())
}

#[test]
fn incan_owned_rustup_home_is_absent_for_a_development_checkout() -> Result<(), Box<dyn std::error::Error>> {
    // A checkout has no `<root>/rust/toolchains`, so the compiler must keep resolving through the ambient
    // Rustup default. Regressing this would break every contributor's `make test`.
    let checkout = tempfile::tempdir()?;
    let executable = checkout.path().join("target").join("debug").join("incan");
    fs::create_dir_all(executable.parent().ok_or("executable has no parent")?)?;
    fs::write(&executable, b"")?;
    let home = tempfile::tempdir()?;
    assert_eq!(
        super::super::incan_owned_rustup_home_in(
            Some(checkout.path().as_os_str().to_os_string()),
            Some(executable),
            Some(home.path().as_os_str().to_os_string()),
        ),
        None
    );
    Ok(())
}

#[test]
fn incan_owned_tool_resolves_the_only_toolchain_without_a_pointer() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let rust_root = provisioned_rust_root(root.path(), &["1.98.0-aarch64-apple-darwin"])?;
    assert_eq!(
        super::super::incan_owned_tool(&rust_root, "rustc"),
        Some(
            rust_root
                .join("toolchains")
                .join("1.98.0-aarch64-apple-darwin")
                .join("bin")
                .join("rustc")
        )
    );
    Ok(())
}

#[test]
fn incan_owned_tool_uses_the_pointer_when_an_upgrade_left_an_older_channel() -> Result<(), Box<dyn std::error::Error>> {
    // An upgrade can leave the previous channel installed. Choosing between them by sort order would silently
    // pick the wrong compiler, so the installer's recorded channel decides.
    let root = tempfile::tempdir()?;
    let rust_root = provisioned_rust_root(
        root.path(),
        &["1.97.0-aarch64-apple-darwin", "1.98.0-aarch64-apple-darwin"],
    )?;
    fs::write(rust_root.join(super::super::INCAN_OWNED_CHANNEL_POINTER), "1.98.0\n")?;
    assert_eq!(
        super::super::incan_owned_tool(&rust_root, "rustc"),
        Some(
            rust_root
                .join("toolchains")
                .join("1.98.0-aarch64-apple-darwin")
                .join("bin")
                .join("rustc")
        )
    );
    Ok(())
}

#[test]
fn incan_owned_tool_declines_an_ambiguous_home_without_a_pointer() -> Result<(), Box<dyn std::error::Error>> {
    // Guessing here would bind the build to an arbitrary compiler; falling back to the ambient default is the
    // honest outcome.
    let root = tempfile::tempdir()?;
    let rust_root = provisioned_rust_root(
        root.path(),
        &["1.97.0-aarch64-apple-darwin", "1.98.0-aarch64-apple-darwin"],
    )?;
    assert_eq!(super::super::incan_owned_tool(&rust_root, "rustc"), None);
    Ok(())
}

#[test]
fn incan_owned_tool_resolves_cargo_from_the_same_toolchain_as_rustc() -> Result<(), Box<dyn std::error::Error>> {
    // The baker's Cargo and the compiler's rustc must come from one toolchain, or the isolation is pointless.
    let root = tempfile::tempdir()?;
    let rust_root = provisioned_rust_root(root.path(), &["1.98.0-host"])?;
    fs::write(rust_root.join(super::super::INCAN_OWNED_CHANNEL_POINTER), "1.98.0\n")?;
    let rustc = super::super::incan_owned_tool(&rust_root, "rustc").ok_or("rustc did not resolve")?;
    let cargo = super::super::incan_owned_tool(&rust_root, "cargo").ok_or("cargo did not resolve")?;
    assert_eq!(rustc.parent(), cargo.parent());
    Ok(())
}

#[test]
fn incan_owned_tool_declines_a_home_whose_pointer_names_a_missing_channel() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let rust_root = provisioned_rust_root(root.path(), &["1.97.0-host"])?;
    fs::write(rust_root.join(super::super::INCAN_OWNED_CHANNEL_POINTER), "1.98.0\n")?;
    // The single-toolchain fallback still answers, because that home is unambiguous.
    assert_eq!(
        super::super::incan_owned_tool(&rust_root, "rustc"),
        Some(
            rust_root
                .join("toolchains")
                .join("1.97.0-host")
                .join("bin")
                .join("rustc")
        )
    );
    Ok(())
}

#[test]
fn incan_owned_target_membership_reads_the_toolchain_layout() -> Result<(), Box<dyn std::error::Error>> {
    // The installer adds targets to Incan's own toolchain and leaves the user's Rustup alone, so membership
    // has to be read where the target actually lands.
    let root = tempfile::tempdir()?;
    let rust_root = provisioned_rust_root(root.path(), &["1.98.0-host"])?;
    let toolchain = rust_root.join("toolchains").join("1.98.0-host");
    fs::create_dir_all(toolchain.join("lib").join("rustlib").join("wasm32-wasip1"))?;
    let rustc = super::super::incan_owned_tool(&rust_root, "rustc").ok_or("rustc did not resolve")?;
    let toolchain_root = rustc.parent().and_then(Path::parent).ok_or("no toolchain root")?;
    assert!(
        toolchain_root
            .join("lib")
            .join("rustlib")
            .join("wasm32-wasip1")
            .is_dir()
    );
    assert!(
        !toolchain_root
            .join("lib")
            .join("rustlib")
            .join("aarch64-unknown-none")
            .is_dir()
    );
    Ok(())
}

#[test]
fn incan_owned_rustup_home_prefers_an_explicitly_named_incan_home() -> Result<(), Box<dyn std::error::Error>> {
    let named = tempfile::tempdir()?;
    fs::create_dir_all(named.path().join("rust").join("toolchains").join("1.98.0-host"))?;
    assert_eq!(
        super::super::incan_owned_rustup_home_in(Some(named.path().as_os_str().to_os_string()), None, None),
        Some(named.path().join("rust"))
    );
    Ok(())
}

#[test]
fn incan_owned_rustup_home_follows_the_executable_for_shim_installations() -> Result<(), Box<dyn std::error::Error>> {
    // The npm and pip shims install below their own package directory and do not set `INCAN_HOME` when they
    // spawn the compiler, so the executable's own layout is the only thing that identifies its provisioning.
    let package = tempfile::tempdir()?;
    let incan_home = package.path().join(".incan").join("home");
    fs::create_dir_all(incan_home.join("rust").join("toolchains").join("1.98.0-host"))?;
    let executable = incan_home.join("toolchains").join("0.5.0").join("bin").join("incan");
    fs::create_dir_all(executable.parent().ok_or("executable has no parent")?)?;
    fs::write(&executable, b"")?;
    assert_eq!(
        super::super::incan_owned_rustup_home_in(None, Some(executable), None),
        Some(incan_home.join("rust"))
    );
    Ok(())
}

#[test]
fn incan_owned_rustup_home_falls_back_to_the_user_home_default() -> Result<(), Box<dyn std::error::Error>> {
    let home = tempfile::tempdir()?;
    fs::create_dir_all(
        home.path()
            .join(".incan")
            .join("rust")
            .join("toolchains")
            .join("1.98.0-host"),
    )?;
    assert_eq!(
        super::super::incan_owned_rustup_home_in(None, None, Some(home.path().as_os_str().to_os_string())),
        Some(home.path().join(".incan").join("rust"))
    );
    Ok(())
}

#[test]
fn first_diverging_shared_package_reports_a_same_version_byte_distinct_overlap()
-> Result<(), Box<dyn std::error::Error>> {
    let leaf =
        |package: &str, version: &str, digest: &str, identity: Option<&str>, features: &[&str]| OvenRustcRegistryLeaf {
            domain: Default::default(),
            crate_kind: Default::default(),
            selected_unit_identity: identity.map(str::to_string),
            package: package.to_string(),
            version: version.to_string(),
            crate_name: package.replace('-', "_"),
            features: features.iter().map(|feature| (*feature).to_string()).collect(),
            source: fixture_registry_source(),
            artifact: OvenRustcArtifactExtern {
                crate_name: package.replace('-', "_"),
                relative_path: format!("lib{package}.rlib"),
                digest: digest.to_string(),
            },
        };
    let consumer = OvenRegistryLeafAuthority::new(
        PathBuf::from("/consumer"),
        vec![leaf("tokio", "1.52.3", "sha256:consumer-tokio", None, &[])],
    );
    let provider = OvenRegistryLeafAuthority::new(
        PathBuf::from("/provider"),
        vec![leaf("tokio", "1.52.3", "sha256:provider-tokio", None, &[])],
    );
    assert_eq!(
        consumer.first_diverging_shared_package_pin(&provider),
        Some(("tokio".to_string(), PathBuf::from("/provider"))),
        "one package at one version with two byte-distinct compiled artifacts is the exact dangerous shape, and \
         the pin names the already-compiled contributor that would have to be rebuilt to agree"
    );

    let identical = OvenRegistryLeafAuthority::new(
        PathBuf::from("/provider"),
        vec![leaf("tokio", "1.52.3", "sha256:consumer-tokio", None, &[])],
    );
    assert_eq!(
        consumer.first_diverging_shared_package_pin(&identical),
        None,
        "the same compiled bytes on both sides is the harmless shared case"
    );

    let different_version = OvenRegistryLeafAuthority::new(
        PathBuf::from("/provider"),
        vec![leaf("tokio", "1.51.0", "sha256:provider-tokio", None, &[])],
    );
    assert_eq!(
        consumer.first_diverging_shared_package_pin(&different_version),
        None,
        "distinct versions are ordinary Cargo semver coexistence, not one diverging shared unit"
    );

    let unrelated = OvenRegistryLeafAuthority::new(
        PathBuf::from("/provider"),
        vec![leaf("datafusion", "53.1.0", "sha256:provider-datafusion", None, &[])],
    );
    assert_eq!(consumer.first_diverging_shared_package_pin(&unrelated), None);

    let mut host_tokio = leaf(
        "tokio",
        "1.52.3",
        "sha256:provider-host-tokio",
        Some("sha256:provider-host-unit"),
        &["macros"],
    );
    host_tokio.domain = crate::rustc::OvenRustcRegistryLeafDomain::Host;
    let host_provider = OvenRegistryLeafAuthority::new(PathBuf::from("/provider"), vec![host_tokio.clone()]);
    assert_eq!(
        consumer.first_diverging_shared_package_pin(&host_provider),
        None,
        "a host unit is not linked into the target artifact and cannot collide with its target-domain namesake"
    );
    let host_consumer = OvenRegistryLeafAuthority::new(
        PathBuf::from("/consumer"),
        vec![{
            host_tokio.selected_unit_identity = Some("sha256:consumer-host-unit".to_string());
            host_tokio.artifact.digest = "sha256:consumer-host-tokio".to_string();
            host_tokio
        }],
    );
    assert_eq!(
        host_consumer.first_diverging_shared_package_pin(&host_provider),
        None,
        "independently compiled host units from separate closures are not target-link collisions"
    );

    let portable_consumer = OvenRegistryLeafAuthority::new(
        PathBuf::from("/consumer"),
        vec![leaf(
            "cpufeatures",
            "0.2.17",
            "sha256:consumer-cpufeatures",
            Some("sha256:portable-unit"),
            &["default"],
        )],
    );
    let portable_provider = OvenRegistryLeafAuthority::new(
        PathBuf::from("/provider"),
        vec![leaf(
            "cpufeatures",
            "0.2.17",
            "sha256:provider-cpufeatures",
            Some("sha256:portable-unit"),
            &["default"],
        )],
    );
    assert_eq!(
        portable_consumer.first_diverging_shared_package_pin(&portable_provider),
        None,
        "one portable unit identity reconciles publisher-local payload differences"
    );

    let incompatible_features = OvenRegistryLeafAuthority::new(
        PathBuf::from("/provider"),
        vec![leaf(
            "cpufeatures",
            "0.2.17",
            "sha256:provider-cpufeatures",
            Some("sha256:different-unit"),
            &["default", "std"],
        )],
    );
    assert_eq!(
        portable_consumer.first_diverging_shared_package_pin(&incompatible_features),
        Some(("cpufeatures".to_string(), PathBuf::from("/provider"))),
        "a feature or unit-identity difference remains fail-closed"
    );
    let divergence = portable_consumer
        .first_diverging_shared_package_pin_detail(&incompatible_features)
        .map(|(_, _, divergence)| divergence)
        .unwrap_or_default();
    assert!(
        divergence.contains("the provider's unit sha256:different-unit") && divergence.contains("features"),
        "the refusal detail names both units and the facts that differ: {divergence}"
    );

    // Two units recording the same facts differ through a dependency; the detail names the dependency the two
    // closures carry at different units.
    let graph_consumer = OvenRegistryLeafAuthority::new(
        PathBuf::from("/consumer"),
        vec![
            leaf(
                "cpufeatures",
                "0.2.17",
                "sha256:consumer-cpufeatures",
                Some("sha256:consumer-unit"),
                &[],
            ),
            leaf(
                "libc",
                "0.2.189",
                "sha256:consumer-libc",
                Some("sha256:consumer-libc-unit"),
                &["extra_traits", "std"],
            ),
        ],
    );
    let graph_provider = OvenRegistryLeafAuthority::new(
        PathBuf::from("/provider"),
        vec![
            leaf(
                "cpufeatures",
                "0.2.17",
                "sha256:provider-cpufeatures",
                Some("sha256:provider-unit"),
                &[],
            ),
            leaf(
                "libc",
                "0.2.189",
                "sha256:provider-libc",
                Some("sha256:provider-libc-unit"),
                &["std"],
            ),
        ],
    );
    let (package, _, divergence) = graph_consumer
        .first_diverging_shared_package_pin_detail(&graph_provider)
        .ok_or("the graph split is refused")?;
    assert_eq!(package, "cpufeatures");
    assert!(
        divergence.contains("no recorded fact besides the selected-unit identity")
            && divergence.contains(r#"libc 0.2.189 features ["extra_traits", "std"] vs ["std"]"#),
        "the detail names the dependency the closures split on: {divergence}"
    );

    let multi_version_consumer = OvenRegistryLeafAuthority::new(
        PathBuf::from("/consumer"),
        vec![
            leaf("syn", "2.0.119", "sha256:consumer-syn-2", Some("sha256:syn-2"), &[]),
            leaf("syn", "3.0.6", "sha256:consumer-syn-3", Some("sha256:syn-3"), &[]),
            leaf(
                "cpufeatures",
                "0.2.17",
                "sha256:consumer-cpufeatures",
                Some("sha256:consumer-root"),
                &[],
            ),
        ],
    );
    let multi_version_provider = OvenRegistryLeafAuthority::new(
        PathBuf::from("/provider"),
        vec![
            leaf("syn", "2.0.119", "sha256:consumer-syn-2", Some("sha256:syn-2"), &[]),
            leaf("syn", "3.0.6", "sha256:consumer-syn-3", Some("sha256:syn-3"), &[]),
            leaf(
                "cpufeatures",
                "0.2.17",
                "sha256:provider-cpufeatures",
                Some("sha256:provider-root"),
                &[],
            ),
        ],
    );
    let (_, _, divergence) = multi_version_consumer
        .first_diverging_shared_package_pin_detail(&multi_version_provider)
        .ok_or("the root graph split is refused")?;
    assert!(
        !divergence.contains("syn 2.0.119 vs 3.0.6") && !divergence.contains("syn 3.0.6 vs 2.0.119"),
        "versions present in both closures are not falsely cross-paired: {divergence}"
    );
    Ok(())
}

#[test]
fn selects_one_profile_matched_copy_of_an_equivalent_sealed_registry_leaf() -> Result<(), Box<dyn std::error::Error>> {
    let first = tempfile::tempdir()?;
    let second = tempfile::tempdir()?;
    let relative_path = "target/aarch64-apple-darwin/debug/deps/libfixture_registry-1234.rlib";
    let first_artifact = first.path().join(relative_path);
    let second_artifact = second.path().join(relative_path);
    fs::create_dir_all(first_artifact.parent().ok_or("first registry parent")?)?;
    fs::create_dir_all(second_artifact.parent().ok_or("second registry parent")?)?;
    let first_bytes = b"first sealed copy";
    let second_bytes = b"second sealed copy";
    fs::write(&first_artifact, first_bytes)?;
    fs::write(&second_artifact, second_bytes)?;
    let leaf = |digest| OvenRustcRegistryLeaf {
        domain: Default::default(),
        crate_kind: Default::default(),
        selected_unit_identity: Some("sha256:portable-fixture-unit".to_string()),
        package: "fixture-registry".to_string(),
        version: "1.0.0".to_string(),
        crate_name: "fixture_registry".to_string(),
        features: vec!["derive".to_string()],
        source: fixture_registry_source(),
        artifact: OvenRustcArtifactExtern {
            crate_name: "fixture_registry".to_string(),
            relative_path: relative_path.to_string(),
            digest,
        },
    };
    let authority = OvenRegistryLeafAuthority::aggregate([
        OvenRegistryLeafAuthority::new(first.path().to_path_buf(), vec![leaf(digest_bytes(first_bytes))]),
        OvenRegistryLeafAuthority::new(second.path().to_path_buf(), vec![leaf(digest_bytes(second_bytes))]),
    ]);
    let dependency = DependencySpec {
        crate_name: "fixture_registry".to_string(),
        version: Some("1".to_string()),
        features: vec!["derive".to_string()],
        default_features: true,
        source: DependencySource::Registry,
        optional: false,
        package: Some("fixture-registry".to_string()),
    };

    let selected = resolve_sealed_registry_leaf(&dependency, Some(&authority), "debug")?;
    assert_eq!(selected, fs::canonicalize(first_artifact)?);
    Ok(())
}

#[test]
fn compiler_test_profile_has_an_explicit_direct_rustc_contract() {
    let mut command = Command::new("rustc");
    apply_oven_profile(&mut command, OVEN_COMPILER_TEST_PROFILE);
    let arguments = command
        .get_args()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert_eq!(
        arguments,
        vec![
            "-C",
            "debuginfo=0",
            "-C",
            "strip=debuginfo",
            "-C",
            "debug-assertions=on",
            "-C",
            "overflow-checks=on",
        ]
    );

    let mut developer_profile = Command::new("rustc");
    apply_oven_profile(&mut developer_profile, "debug");
    let developer_arguments = developer_profile
        .get_args()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert_eq!(developer_arguments, vec!["-C", "opt-level=0"]);
}
