//! ABI publication parity across clean and downstream-loaded original native Loaf graphs.

use std::collections::BTreeMap;
use std::fs;
use std::sync::Arc;

use incan_frontend::library_manifest::LibraryRustAbi;

use super::collect_library_rust_abi;

/// Proves complete published ABI extraction is independent of unrelated downstream impl crates in the graph.
#[test]
fn library_rust_abi_ignores_loaded_downstream_trait_impls_issue924() -> Result<(), Box<dyn std::error::Error>> {
    use incan_provider::dependency_resolver::ResolvedDependencies;
    use incan_provider::requirements::ProjectRequirements;
    use oven_model::manifest::{DependencySource, DependencySpec};
    use oven_rustc::native_loaf::{NativeLoafFacet, NativeLoafPreparationRequest, prepare_native_loafs};
    use oven_rustc::rustc::{
        prepare_rust_inspection_toolchain, resolve_active_rustc, rustc_host_target, rustc_identity,
    };

    // ---- Fixture: clean and downstream-loaded views of one Rust surface ----
    let workspace = tempfile::tempdir()?;
    let trait_api = workspace.path().join("trait-api");
    let surface_api = workspace.path().join("surface-api");
    let downstream_api = workspace.path().join("downstream-api");
    let clean_probe = workspace.path().join("clean-probe");
    let polluted_probe = workspace.path().join("polluted-probe");
    for root in [&trait_api, &surface_api, &downstream_api, &clean_probe, &polluted_probe] {
        fs::create_dir_all(root.join("src"))?;
    }
    fs::write(
        trait_api.join("loaf.toml"),
        "[project]\nname='abi_trait_api'\nversion='0.1.0'\n[rust]\nname='abi_trait_api'\ntype='lib'\nedition='2021'\n",
    )?;
    fs::write(trait_api.join("src/lib.rs"), "pub trait Intrinsic {}\n")?;
    fs::write(
        surface_api.join("loaf.toml"),
        "[project]\nname='abi_surface_api'\nversion='0.1.0'\n[rust]\nname='abi_surface_api'\ntype='lib'\nedition='2021'\n[dependencies]\nabi_trait_api={loaf='abi_trait_api',path='../trait-api'}\n",
    )?;
    fs::write(
        surface_api.join("src/lib.rs"),
        "pub struct Thing;\n\nimpl abi_trait_api::Intrinsic for Thing {}\n",
    )?;
    fs::write(
        downstream_api.join("loaf.toml"),
        "[project]\nname='abi_downstream_api'\nversion='0.1.0'\n[rust]\nname='abi_downstream_api'\ntype='lib'\nedition='2021'\n[dependencies]\nabi_surface_api={loaf='abi_surface_api',path='../surface-api'}\n",
    )?;
    fs::write(
        downstream_api.join("src/lib.rs"),
        "pub trait Ambient {}\n\nimpl Ambient for abi_surface_api::Thing {}\n",
    )?;

    // ---- Preparation: each view retains its original native request and compiler source owner ----
    let rustc = resolve_active_rustc()?;
    let target = rustc_host_target(&rustc)?;
    let toolchain = rustc_identity(&rustc)?;
    let source_owner = Arc::new(prepare_rust_inspection_toolchain(
        &crate::oven_store::open_default_oven_store()?,
        &rustc,
        &target,
    )?);
    let lock = workspace.path().join("lock.json");
    fs::write(&lock, r#"{"schema":"incan.oven.loaf-resolution/2","units":[]}"#)?;
    let query_paths = vec!["abi_surface_api::Thing".to_string()];
    let mut published = Vec::new();
    for (probe, include_downstream) in [(&clean_probe, false), (&polluted_probe, true)] {
        let mut projects = vec![(&trait_api, "abi_trait_api"), (&surface_api, "abi_surface_api")];
        if include_downstream {
            projects.push((&downstream_api, "abi_downstream_api"));
        }
        let facets = projects
            .iter()
            .map(|(project, _)| NativeLoafFacet {
                project: (*project).clone(),
                features: Vec::new(),
                domain: "target".to_string(),
            })
            .collect::<Vec<_>>();
        let native = prepare_native_loafs(&NativeLoafPreparationRequest {
            lock: &lock,
            blobs: workspace.path(),
            output: &probe.join("native"),
            rustc: &rustc,
            index: workspace.path(),
            index_commit: "0000000000000000000000000000000000000000",
            target: &target,
            profile: "debug",
            facet_owner: workspace.path(),
            facets: &facets,
        })?;
        let dependencies = projects
            .iter()
            .filter(|(_, name)| *name != "abi_trait_api")
            .map(|(project, name)| DependencySpec {
                crate_name: (*name).to_string(),
                version: Some("=0.1.0".to_string()),
                features: Vec::new(),
                default_features: true,
                source: DependencySource::Path {
                    path: (*project).clone(),
                },
                optional: false,
                package: Some((*name).to_string()),
            })
            .collect::<Vec<_>>();
        let resolved = ResolvedDependencies {
            dependencies: dependencies.clone(),
            dev_dependencies: Vec::new(),
        };
        let prepared = crate::lock::rust_inspect::prepare_rust_inspect_workspace_with_ordinary_native(
            crate::lock::RustInspectWorkspaceRequest {
                project_root: probe,
                project_name: "abi_probe",
                cargo_package_name: "abi_probe",
                rust_edition: Some("2021".to_string()),
                resolved: &resolved,
                project_requirements: &ProjectRequirements::default(),
                lock_payload: None,
                cargo_lock_projection_root: None,
                clear_cargo_lock: false,
                cargo_policy_flags: Vec::new(),
                cargo_target_dir: &probe.join("inspection"),
                rust_inspect_query_paths: &query_paths,
                rust_derive_probe_paths: &[],
                prepare_when_empty: true,
                direct_oven_inspection: true,
                force_direct_prewarm: false,
                oven_source_authority: Some(crate::lock::OvenRustInspectSourceAuthorityRequest {
                    project_version: "0.1.0",
                    target: &target,
                    toolchain: &toolchain,
                    profile: "debug",
                    features: &[],
                    build_unit_inputs: &BTreeMap::new(),
                    registry_dependencies: &dependencies,
                }),
                prepared_project_source_authorities: None,
                explicit_oven_bake: false,
            },
            native.request_observation()?,
            &rustc,
            Some(Arc::clone(&source_owner)),
        )?
        .ok_or("ordinary ABI inspection was not prepared")?;
        prepared.verify_ordinary_native()?;
        if include_downstream {
            let ambient_path = "abi_downstream_api::Ambient".to_string();
            let ambient = collect_library_rust_abi(prepared.manifest_dir(), std::slice::from_ref(&ambient_path))?
                .ok_or("expected loaded downstream trait ABI")?;
            assert!(
                ambient.get(&ambient_path).is_some(),
                "downstream trait must actually be loaded"
            );
        }
        published.push(
            collect_library_rust_abi(prepared.manifest_dir(), &query_paths)?
                .ok_or("expected published library Rust ABI")?,
        );
    }
    let [clean, polluted]: [LibraryRustAbi; 2] = published.try_into().map_err(|_| "expected two ABI views")?;

    // ---- Publication: complete ABI must converge across both loaded graphs ----
    assert_eq!(
        clean, polluted,
        "published library ABI must not depend on unrelated downstream impl crates"
    );

    // ---- Contract: preserve intrinsic traits and exclude downstream-only traits ----
    let thing = polluted
        .get("abi_surface_api::Thing")
        .ok_or("expected Thing ABI item")?;
    let incan_lang::interop::RustItemKind::Type(thing_type) = &thing.kind else {
        return Err("expected Thing ABI type metadata".into());
    };
    assert!(
        thing_type
            .implemented_traits
            .iter()
            .any(|implemented| implemented.path == "abi_trait_api::Intrinsic")
    );
    assert!(
        thing_type
            .implemented_traits
            .iter()
            .all(|implemented| implemented.path != "abi_downstream_api::Ambient")
    );
    Ok(())
}
