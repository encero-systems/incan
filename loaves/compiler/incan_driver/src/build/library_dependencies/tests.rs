//! Actual ordinary metadata owners and session admission, with profile authority checked independently.
//!
//! Profile fixture outputs are receipted test bytes, not a native compiler execution claim. All metadata selection,
//! frontend checking, handoff validation and session construction call production boundaries.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::sync::Arc;

use incan_frontend::library_exports::collect_checked_public_exports;
use incan_frontend::library_manifest::{LibraryManifest, ProviderModuleClaim, digest_provider_artifact};
use incan_frontend::library_manifest_index::{LibraryArtifactMetadata, LibraryManifestIndex};
use incan_frontend::provider::namespaces::SelectedProviderNamespace;
use incan_frontend::typechecker::TypeChecker;
use incan_provider::requirements::ProjectRequirements;
use incan_provider::{
    FeatureSelection, NamespaceAuthority, PackageFeaturePlan, ProviderIdentity, ProviderPlan, ProviderProvenance,
    ProviderRecord, SdkComponent, SdkInventory, SdkProviderDescriptor,
};
use oven_model::manifest::ProjectManifest;
use oven_store::digest_bytes;
use oven_store::store::{OvenStore, OvenStoreLimits};

use super::{LibraryDependencyInput, PreparedLibraryDependencies, selected_artifact_root};
use crate::build::library_generation::publish_library_generation;
use crate::build::library_metadata::requirements::{CheckedLibraryCapture, CheckedLibraryRequirements};
use crate::build::library_metadata::{
    LibraryMetadataDependency, LibraryMetadataRecipe, publish_library_metadata_with_requirements,
};
use crate::build::library_outputs::packaged_library_loaf_store_root;
use crate::build::library_project::metadata_replay::{current_source_digest, observe_library_source_digest};
use crate::build::package_loafs::{read_packaged_library_loaf_manifest, write_packaged_library_loaf_manifest};
use crate::build::source_authority::digest_baked_project_source_authority;
use crate::build::test_support::packaged_provider_authority_fixture;
use crate::session::CompilationSession;

const TARGET: &str = "aarch64-apple-darwin";
const TOOLCHAIN: &str = "rustc fixture";

/// Bound actual immutable test owners without reading an ambient user Store.
fn limits() -> OvenStoreLimits {
    OvenStoreLimits::new(32 * 1024 * 1024, 32 * 1024 * 1024, 32 * 1024 * 1024)
}

/// Publish real checked planning requirements beside an existing complete package-profile fixture.
fn checked_package()
-> Result<(tempfile::TempDir, LibraryArtifactMetadata, LibraryDependencyInput), Box<dyn std::error::Error>> {
    checked_package_with_requirements(true)
}

/// Exercise genuine old/incomplete metadata owners without manufacturing a checked requirement record.
fn checked_package_with_requirements(
    include_requirements: bool,
) -> Result<(tempfile::TempDir, LibraryArtifactMetadata, LibraryDependencyInput), Box<dyn std::error::Error>> {
    let (root, artifact) = packaged_provider_authority_fixture(&["debug", "release"])?;
    let mut package = read_packaged_library_loaf_manifest(&artifact)?.ok_or("fixture handoff absent")?;
    let mut manifest = LibraryManifest::read_from_path(&artifact.manifest_path)?;
    manifest.contract_metadata.provider.namespace_claims = vec![ProviderModuleClaim {
        module_path: vec!["core".into()],
        required_features: BTreeSet::new(),
    }];
    manifest.contract_metadata.provider.implementation_facets.push(
        incan_frontend::library_manifest::ProviderImplementationFacet {
            id: "core-runtime".into(),
            required_modules: BTreeSet::from([vec!["core".into()]]),
            required_features: BTreeSet::new(),
            cargo_features: BTreeMap::new(),
            cargo_dependencies: Vec::new(),
        },
    );
    manifest.write_to_path(&artifact.manifest_path)?;
    let source = fs::read_to_string(root.path().join("src/lib.incn"))?;
    let tokens = incan_frontend::lexer::lex(&source).map_err(|error| format!("lex: {error:?}"))?;
    let program = incan_frontend::parser::parse(&tokens).map_err(|error| format!("parse: {error:?}"))?;
    let mut checker = TypeChecker::new();
    checker.set_current_package_identity(Some("provider".into()));
    checker.set_current_module_path(Some(vec!["lib".into()]));
    checker
        .check_program(&program)
        .map_err(|error| format!("check: {error:?}"))?;
    let exports = collect_checked_public_exports(&program, &checker);
    let project = ProjectManifest::load(&root.path().join("loaf.toml"))?;
    let contract = CheckedLibraryRequirements::capture(CheckedLibraryCapture {
        project: &project,
        index: &LibraryManifestIndex::default(),
        requirements: &ProjectRequirements::default(),
        imports: &[],
        exports: &exports,
        version: "0.1.0",
        used_module_paths: BTreeSet::new(),
        source_modules: BTreeMap::from([("src/lib.incn".into(), vec!["lib".into()])]),
        entry_module: vec!["lib".into()],
        rust_abi_queries: BTreeSet::new(),
        rust_extern_paths: Vec::new(),
        backend: None,
    })?;
    let recipe = LibraryMetadataRecipe {
        name: "provider".into(),
        version: "0.1.0".into(),
        source_digest: observe_library_source_digest(root.path(), &[])?,
        producer_digest: digest_bytes(b"actual test metadata producer"),
        semantic_authority_digest: digest_bytes(b"complete test semantic closure"),
        dependencies: BTreeMap::new(),
        policy_digest: digest_bytes(b"ordinary test policy"),
        target: TARGET.into(),
        toolchain: TOOLCHAIN.into(),
        features: Vec::new(),
    };
    let store = OvenStore::with_release(
        packaged_library_loaf_store_root(&artifact.crate_root),
        limits(),
        &incan_oven_facet::compiler_identity(),
    );
    let selected = publish_library_metadata_with_requirements(
        &store,
        &recipe,
        &recipe.receipt(root.path())?,
        &artifact.crate_root,
        &artifact.manifest_path,
        BTreeSet::new(),
        include_requirements.then_some(contract),
    )?;
    package.metadata_files = selected.checked_files().to_vec();
    assert_ne!(selected.recipe().source_digest, package.source_authority_digest);
    let generation = publish_library_generation(&store, root.path(), &package.source_authority_digest, &selected)?;
    package.checked_generation = Some(generation.reference());
    package.checked_metadata = Some(selected.reference());
    write_packaged_library_loaf_manifest(&artifact.crate_root, &package)?;
    let input = LibraryDependencyInput {
        artifact_root: artifact.crate_root.clone(),
        handoff_digest: handoff_digest(&artifact)?,
        features: BTreeSet::new(),
        import_alias: Some("geometry".into()),
        namespace: None,
    };
    Ok((root, artifact, input))
}

/// Select the exact handoff as a trusted fixture producer; production resolver adapters must supply this reference.
fn handoff_digest(artifact: &LibraryArtifactMetadata) -> Result<String, Box<dyn std::error::Error>> {
    Ok(digest_bytes(&fs::read(
        incan_frontend::library_manifest::published_layout::packaged_library_loaf_manifest_path(&artifact.crate_root),
    )?))
}

/// Source and installed packages use the identical metadata path; actual profiles remain explicit and separate.
#[test]
fn ordinary_admission_source_free_repeat_and_profiles() -> Result<(), Box<dyn std::error::Error>> {
    let (root, artifact, input) = checked_package()?;
    let first = PreparedLibraryDependencies::admit(&[input.clone()], TARGET, TOOLCHAIN, limits())?;
    let repeat = PreparedLibraryDependencies::admit(&[input.clone()], TARGET, TOOLCHAIN, limits())?;
    assert_eq!(
        first.nodes.keys().collect::<Vec<_>>(),
        repeat.nodes.keys().collect::<Vec<_>>()
    );
    assert!(
        first
            .execution_profiles(&["debug"])?
            .values()
            .all(|profiles| profiles.len() == 1 && profiles[0].source_available)
    );
    // Remove only this disposable fixture's authored inputs; its sealed package and Store remain installed.
    fs::remove_file(root.path().join("loaf.toml"))?;
    fs::remove_file(root.path().join("src/lib.incn"))?;
    assert!(first.verify().is_err());
    let installed = PreparedLibraryDependencies::admit(&[input], TARGET, TOOLCHAIN, limits())?;
    assert!(
        installed
            .execution_profiles(&["release"])?
            .values()
            .all(|profiles| profiles.len() == 1 && !profiles[0].source_available)
    );
    assert_eq!(installed.nodes.len(), 1);
    assert!(artifact.crate_root.is_dir());
    Ok(())
}

/// Session discovery consumes admitted checked metadata, retains the catalog, and revalidates even cached usage.
#[test]
fn ordinary_admitted_session_reuses_projection_and_refuses_changed_source() -> Result<(), Box<dyn std::error::Error>> {
    let (provider, _artifact, input) = checked_package()?;
    let consumer = tempfile::tempdir()?;
    fs::create_dir(consumer.path().join("src"))?;
    fs::write(
        consumer.path().join("loaf.toml"),
        format!(
            "[project]\nname='consumer'\n[dependencies]\ngeometry={{path={:?}}}\n",
            provider.path().to_string_lossy()
        ),
    )?;
    let entry = consumer.path().join("src/main.incn");
    fs::write(&entry, "def main():\n    pass\n")?;
    let prepared = Arc::new(PreparedLibraryDependencies::admit(
        &[input],
        TARGET,
        TOOLCHAIN,
        limits(),
    )?);
    let session = CompilationSession::discover_with_admitted_library_dependencies(
        &entry,
        &FeatureSelection::default(),
        prepared,
    )?;
    assert!(session.sdk_inventory.is_none());
    assert!(session.sdk_components.is_none());
    assert!(
        session
            .provider_plan_for_used_module_paths(BTreeSet::from([vec!["std".into(), "unselected".into()]]))
            .is_err()
    );
    let usage = BTreeSet::from([vec!["pub".into(), "geometry".into(), "core".into()]]);
    let first = session.provider_plan_for_used_module_paths(usage.clone())?;
    let repeat = session.provider_plan_for_used_module_paths(usage.clone())?;
    assert!(Arc::ptr_eq(&first, &repeat));
    let record = first.active_records().next().ok_or("project provider absent")?;
    assert_eq!(first.used_modules(record), usage);
    assert_ne!(
        first.semantic_projection_identity(),
        session.provider_plan.semantic_projection_identity()
    );
    assert_eq!(
        first.semantic_projection_persistent_key()?,
        session.provider_plan.semantic_projection_persistent_key()?
    );
    let source = provider.path().join("src/lib.incn");
    let previous_modified = fs::metadata(&source)?.modified()?;
    fs::write(&source, "pub def provider() -> int:\n    return 2\n")?;
    fs::File::options()
        .write(true)
        .open(&source)?
        .set_modified(previous_modified)?;
    assert!(session.provider_plan_for_used_module_paths(usage).is_err());
    Ok(())
}

/// Selected handoff identity, current target/features and actual checked owner are mandatory admission inputs.
#[test]
fn ordinary_admission_refuses_substituted_or_legacy_authority() -> Result<(), Box<dyn std::error::Error>> {
    let (_root, artifact, input) = checked_package()?;
    let mut wrong = input.clone();
    wrong.handoff_digest = digest_bytes(b"another selected handoff");
    assert!(PreparedLibraryDependencies::admit(&[wrong], TARGET, TOOLCHAIN, limits()).is_err());
    assert!(PreparedLibraryDependencies::admit(&[input.clone()], "another-target", TOOLCHAIN, limits()).is_err());
    let mut wrong = input.clone();
    wrong.features.insert("injected".into());
    assert!(PreparedLibraryDependencies::admit(&[wrong], TARGET, TOOLCHAIN, limits()).is_err());
    let original = read_packaged_library_loaf_manifest(&artifact)?.ok_or("handoff absent")?;
    let mut legacy = original.clone();
    legacy.schema_version = 6;
    legacy.checked_metadata = None;
    write_packaged_library_loaf_manifest(&artifact.crate_root, &legacy)?;
    let mut selected_legacy = input.clone();
    selected_legacy.handoff_digest = handoff_digest(&artifact)?;
    assert!(PreparedLibraryDependencies::admit(&[selected_legacy], TARGET, TOOLCHAIN, limits()).is_err());
    let mut missing = original;
    missing.checked_metadata.as_mut().ok_or("owner absent")?.owner_identity = digest_bytes(b"absent owner");
    write_packaged_library_loaf_manifest(&artifact.crate_root, &missing)?;
    let mut selected_missing = input;
    selected_missing.handoff_digest = handoff_digest(&artifact)?;
    assert!(PreparedLibraryDependencies::admit(&[selected_missing], TARGET, TOOLCHAIN, limits()).is_err());
    Ok(())
}

/// Mutable handoff/profile substitution refuses; changing only native output does not force semantic reinspection.
#[test]
fn ordinary_admission_separates_metadata_from_execution_integrity() -> Result<(), Box<dyn std::error::Error>> {
    let (_root, artifact, input) = checked_package()?;
    let selected = PreparedLibraryDependencies::admit(&[input.clone()], TARGET, TOOLCHAIN, limits())?;
    fs::write(
        artifact.crate_root.join("oven/debug/libprovider.rlib"),
        "changed output",
    )?;
    let metadata = PreparedLibraryDependencies::admit(&[input], TARGET, TOOLCHAIN, limits())?;
    metadata.verify()?;
    assert!(metadata.execution_profiles(&["debug"]).is_err());
    assert!(metadata.execution_profiles(&["not-published"]).is_err());
    let mut package = read_packaged_library_loaf_manifest(&artifact)?.ok_or("handoff absent")?;
    package.profiles.remove("release");
    write_packaged_library_loaf_manifest(&artifact.crate_root, &package)?;
    assert!(selected.verify().is_err());
    Ok(())
}

/// Public records and inventories cannot issue reserved authority; ordinary aliases retain no reserved grant.
#[test]
fn ordinary_admission_reserved_grant_cannot_be_transplanted_or_self_claimed() -> Result<(), Box<dyn std::error::Error>>
{
    let (_root, artifact, input) = checked_package()?;
    let manifest = LibraryManifest::read_from_path(&artifact.manifest_path)?;
    let identity = ProviderIdentity {
        name: manifest.name.clone(),
        version: manifest.version.clone(),
        digest: digest_provider_artifact(&artifact.crate_root)?,
        feature_projection: BTreeSet::new(),
    };
    let record = ProviderRecord {
        identity: identity.clone(),
        provenance: ProviderProvenance::Sdk {
            sdk_identity: "validated-test-selection".into(),
            component_id: "test-grant".into(),
            inventory_path: None,
        },
        authority: NamespaceAuthority::SdkReserved,
        namespace_claims: BTreeSet::from([vec!["std".into(), "core".into()]]),
        available: true,
        enabled: true,
        manifest: Some(Arc::new(manifest)),
        artifact: Some(artifact.clone()),
        implementation_facets: Vec::new(),
    };
    let mut altered = record.clone();
    altered.namespace_claims.insert(vec!["std".into(), "unowned".into()]);
    let altered_plan = ProviderPlan::new(LibraryManifestIndex::default(), vec![altered], std::iter::empty())?;
    assert!(SelectedProviderNamespace::from_plan(&altered_plan, &identity.stable_key()).is_err());
    let synthetic = ProviderPlan::new(
        LibraryManifestIndex::default(),
        vec![record.clone()],
        std::iter::empty(),
    )?;
    assert!(SelectedProviderNamespace::from_plan(&synthetic, &identity.stable_key()).is_err());
    let inventory = SdkInventory {
        root: artifact.crate_root.clone(),
        sdk_id: "incan".into(),
        sdk_version: "0.1.0".into(),
        compiler_requirement: ">=0.5.0-dev.7,<0.6.0".into(),
        provider_codegen_revision: incan_lang::version::SDK_PROVIDER_CODEGEN_REVISION,
        components: BTreeMap::from([(
            "test-grant".into(),
            SdkComponent {
                id: "test-grant".into(),
                version: "0.1.0".into(),
                mandatory: true,
                available: true,
                dependencies: BTreeSet::new(),
                providers: vec![SdkProviderDescriptor {
                    name: identity.name.clone(),
                    version: identity.version.clone(),
                    digest: identity.digest.clone(),
                    namespace_claims: record.namespace_claims.clone(),
                    manifest_path: Some(artifact.manifest_path.clone()),
                    crate_root: Some(artifact.crate_root.clone()),
                }],
            },
        )]),
        profiles: BTreeMap::new(),
    };
    // Descriptor validity preserves legacy providers, but a public DTO proves no trusted namespace issuer.
    let prior = ProviderPlan::from_resolved_inputs(
        LibraryManifestIndex::default(),
        None,
        Some(&inventory),
        None,
        std::iter::empty(),
    )?;
    assert!(
        prior
            .active_records()
            .any(|record| record.authority == NamespaceAuthority::SdkReserved)
    );
    assert!(SelectedProviderNamespace::from_plan(&prior, &identity.stable_key()).is_err());
    let copied = ProviderPlan::new(LibraryManifestIndex::default(), vec![record], std::iter::empty())?;
    assert!(SelectedProviderNamespace::from_plan(&copied, &identity.stable_key()).is_err());
    let mut ungranted = input;
    ungranted.import_alias = Some("std".into());
    ungranted.namespace = None;
    let ordinary = PreparedLibraryDependencies::admit(&[ungranted], TARGET, TOOLCHAIN, limits())?;
    assert!(
        ordinary
            .provider_plan()
            .active_records()
            .all(|record| !matches!(record.authority, NamespaceAuthority::SdkReserved))
    );
    let ordinary_identity = ordinary
        .provider_plan()
        .active_records()
        .next()
        .ok_or("ordinary provider absent")?
        .identity
        .stable_key();
    assert!(SelectedProviderNamespace::from_plan(ordinary.provider_plan(), &ordinary_identity).is_err());
    Ok(())
}

/// A genuine immutable metadata owner lacking planning knowledge cannot act as an authenticated empty contract.
#[test]
fn ordinary_admission_refuses_missing_checked_requirements() -> Result<(), Box<dyn std::error::Error>> {
    let (_root, _artifact, input) = checked_package_with_requirements(false)?;
    assert!(PreparedLibraryDependencies::admit(&[input], TARGET, TOOLCHAIN, limits()).is_err());
    Ok(())
}

/// The current root manifest must select the exact admitted alias and artifact, not a similarly named package.
#[test]
fn ordinary_admitted_session_refuses_alias_or_coordinate_substitution() -> Result<(), Box<dyn std::error::Error>> {
    let (provider, _artifact, input) = checked_package()?;
    let (other, _other_artifact, _other_input) = checked_package()?;
    let consumer = tempfile::tempdir()?;
    fs::create_dir(consumer.path().join("src"))?;
    let entry = consumer.path().join("src/main.incn");
    fs::write(&entry, "def main():\n    pass\n")?;
    let admitted = Arc::new(PreparedLibraryDependencies::admit(
        &[input],
        TARGET,
        TOOLCHAIN,
        limits(),
    )?);
    for (alias, root) in [("other", provider.path()), ("geometry", other.path())] {
        fs::write(
            consumer.path().join("loaf.toml"),
            format!(
                "[project]\nname='consumer'\n[dependencies]\n{alias}={{path={:?}}}\n",
                root.to_string_lossy()
            ),
        )?;
        assert!(
            CompilationSession::discover_with_admitted_library_dependencies(
                &entry,
                &FeatureSelection::default(),
                Arc::clone(&admitted)
            )
            .is_err()
        );
    }
    Ok(())
}

/// A receipt names the exact checked child owner; absent knowledge cannot become an empty retained dependency set.
#[test]
fn ordinary_admission_refuses_missing_original_dependency_owner() -> Result<(), Box<dyn std::error::Error>> {
    let (root, artifact, mut input) = checked_package()?;
    let baseline = PreparedLibraryDependencies::admit(&[input.clone()], TARGET, TOOLCHAIN, limits())?;
    let owner = &baseline.nodes.values().next().ok_or("fixture owner absent")?.metadata;
    let mut recipe = owner.recipe().clone();
    recipe.dependencies.insert(
        "child".into(),
        LibraryMetadataDependency {
            name: "child".into(),
            version: "1.0.0".into(),
            owner_identity: digest_bytes(b"absent original child"),
            receipt_identity: digest_bytes(b"absent child receipt"),
            checked_digest: digest_bytes(b"absent checked contract"),
        },
    );
    let store = OvenStore::with_release(
        packaged_library_loaf_store_root(&artifact.crate_root),
        limits(),
        &incan_oven_facet::compiler_identity(),
    );
    let selected = publish_library_metadata_with_requirements(
        &store,
        &recipe,
        &recipe.receipt(root.path())?,
        &artifact.crate_root,
        &artifact.manifest_path,
        BTreeSet::new(),
        owner.checked_requirements().cloned(),
    )?;
    let mut package = read_packaged_library_loaf_manifest(&artifact)?.ok_or("handoff absent")?;
    assert_ne!(selected.recipe().source_digest, package.source_authority_digest);
    let generation = publish_library_generation(&store, root.path(), &package.source_authority_digest, &selected)?;
    package.checked_generation = Some(generation.reference());
    package.checked_metadata = Some(selected.reference());
    write_packaged_library_loaf_manifest(&artifact.crate_root, &package)?;
    input.handoff_digest = handoff_digest(&artifact)?;
    assert!(PreparedLibraryDependencies::admit(&[input], TARGET, TOOLCHAIN, limits()).is_err());
    Ok(())
}

/// Equal-length facade edits with preserved timestamps cannot replace the checked owner's generated file closure.
#[test]
fn ordinary_admission_refuses_changed_materialized_facade() -> Result<(), Box<dyn std::error::Error>> {
    let (_root, artifact, input) = checked_package()?;
    let admitted = PreparedLibraryDependencies::admit(&[input.clone()], TARGET, TOOLCHAIN, limits())?;
    let facade = artifact.crate_root.join("src/lib.rs");
    let previous_modified = fs::metadata(&facade)?.modified()?;
    let original = fs::read_to_string(&facade)?;
    let changed = original.replace("provider", "impostor");
    assert_ne!(original, changed);
    assert_eq!(original.len(), changed.len());
    fs::write(&facade, changed)?;
    fs::File::options()
        .write(true)
        .open(&facade)?
        .set_modified(previous_modified)?;
    assert!(admitted.verify().is_err());
    assert!(PreparedLibraryDependencies::admit(&[input], TARGET, TOOLCHAIN, limits()).is_err());
    Ok(())
}

/// Two direct aliases share one exact original owner while retaining both namespace and public type routes.
#[test]
fn ordinary_admission_shares_exact_owner_across_aliases() -> Result<(), Box<dyn std::error::Error>> {
    let (provider, _artifact, first) = checked_package()?;
    let mut second = first.clone();
    second.import_alias = Some("shape".into());
    let forward = PreparedLibraryDependencies::admit(&[first.clone(), second.clone()], TARGET, TOOLCHAIN, limits())?;
    let reverse = PreparedLibraryDependencies::admit(&[second, first], TARGET, TOOLCHAIN, limits())?;
    assert_eq!(forward.nodes.len(), 1);
    assert_eq!(forward.checked_packages().count(), 1);
    assert_eq!(forward.direct_aliases().len(), 2);
    assert_eq!(forward.provider_plan().records().count(), 1);
    assert_eq!(
        forward.provider_plan().semantic_projection_persistent_key()?,
        reverse.provider_plan().semantic_projection_persistent_key()?
    );
    let record = forward
        .provider_plan()
        .records()
        .next()
        .ok_or("shared provider absent")?;
    for alias in ["geometry", "shape"] {
        assert!(
            record
                .namespace_claims
                .contains(&vec!["pub".into(), alias.into(), "core".into()])
        );
        assert_eq!(
            forward.provider_plan().public_artifact_route(alias, &record.identity)?,
            Vec::<String>::new()
        );
        let usage = forward.provider_plan().project_module_usage(BTreeSet::from([vec![
            "pub".into(),
            alias.into(),
            "core".into(),
        ]]));
        assert_eq!(
            usage
                .selected_implementation_facets(record)
                .iter()
                .map(|facet| facet.id.as_str())
                .collect::<Vec<_>>(),
            vec!["core-runtime"]
        );
    }
    let consumer = tempfile::tempdir()?;
    fs::create_dir(consumer.path().join("src"))?;
    fs::write(
        consumer.path().join("loaf.toml"),
        format!(
            "[project]\nname='consumer'\n[dependencies]\ngeometry={{path={:?}}}\nshape={{path={:?}}}\n",
            provider.path().to_string_lossy(),
            provider.path().to_string_lossy()
        ),
    )?;
    let entry = consumer.path().join("src/main.incn");
    fs::write(&entry, "def main():\n    pass\n")?;
    let session = CompilationSession::discover_with_admitted_library_dependencies(
        &entry,
        &FeatureSelection::default(),
        Arc::new(forward),
    )?;
    assert!(
        session
            .provider_plan_for_used_module_paths(BTreeSet::from([
                vec!["pub".into(), "geometry".into(), "core".into()],
                vec!["pub".into(), "shape".into(), "core".into()],
            ]))
            .is_ok()
    );
    Ok(())
}

/// Re-sealing a mutable handoff cannot associate an old checked owner with current authored or installed source.
#[test]
fn ordinary_admission_refuses_stale_checked_generation() -> Result<(), Box<dyn std::error::Error>> {
    let (root, artifact, mut input) = checked_package()?;
    let admitted = PreparedLibraryDependencies::admit(&[input.clone()], TARGET, TOOLCHAIN, limits())?;
    let metadata = &admitted.nodes.values().next().ok_or("metadata absent")?.metadata;
    fs::write(
        root.path().join("src/lib.incn"),
        "pub def provider() -> int:\n    return 2\n",
    )?;
    let current = digest_baked_project_source_authority(root.path())?;
    let store = OvenStore::with_release(
        packaged_library_loaf_store_root(&artifact.crate_root),
        limits(),
        &incan_oven_facet::compiler_identity(),
    );
    // A genuine publisher must refuse, even though the public checked contract can remain byte-identical.
    assert!(publish_library_generation(&store, root.path(), &current, metadata).is_err());
    let path =
        incan_frontend::library_manifest::published_layout::packaged_library_loaf_manifest_path(&artifact.crate_root);
    let mut package: crate::build::OvenPackagedLibraryLoafManifest = serde_json::from_slice(&fs::read(&path)?)?;
    assert_ne!(current, package.source_authority_digest);
    package.source_authority_digest = current;
    write_packaged_library_loaf_manifest(&artifact.crate_root, &package)?;
    input.handoff_digest = handoff_digest(&artifact)?;
    assert!(PreparedLibraryDependencies::admit(&[input.clone()], TARGET, TOOLCHAIN, limits()).is_err());
    fs::remove_file(root.path().join("loaf.toml"))?;
    fs::remove_file(root.path().join("src/lib.incn"))?;
    assert!(PreparedLibraryDependencies::admit(&[input], TARGET, TOOLCHAIN, limits()).is_err());
    assert!(publish_library_generation(&store, root.path(), &package.source_authority_digest, metadata).is_err());
    Ok(())
}

/// Default and explicitly reconstructed exact features observe the same semantics; changed inputs do not.
#[test]
fn ordinary_metadata_source_observer_normalizes_feature_reasons() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let child = tempfile::tempdir()?;
    for (directory, name) in [(root.path(), "root"), (child.path(), "child")] {
        fs::create_dir(directory.join("src"))?;
        fs::write(
            directory.join("src/lib.incn"),
            "pub def value() -> int:\n    return 1\n",
        )?;
        fs::write(directory.join("loaf.toml"), format!("[project]\nname='{name}'\n"))?;
    }
    fs::write(
        root.path().join("loaf.toml"),
        format!(
            "[project]\nname='root'\n[project.features]\ndefault=['enabled']\nenabled=[]\nextra=[]\n[dependencies]\nchild={{path={:?}}}\n",
            child.path().to_string_lossy()
        ),
    )?;
    let manifest = ProjectManifest::load(&root.path().join("loaf.toml"))?;
    let default = PackageFeaturePlan::resolve(&manifest, &FeatureSelection::default())?;
    let exact = vec!["default".into(), "enabled".into()];
    let baseline = current_source_digest(root.path(), &default)?;
    assert_eq!(baseline, observe_library_source_digest(root.path(), &exact)?);
    assert_ne!(
        baseline,
        observe_library_source_digest(root.path(), &["default".into(), "enabled".into(), "extra".into()])?
    );
    fs::write(
        child.path().join("src/lib.incn"),
        "pub def value() -> int:\n    return 2\n",
    )?;
    assert_ne!(baseline, observe_library_source_digest(root.path(), &exact)?);
    Ok(())
}

/// A real compiled transitive feature edge retains its selected relocated artifact coordinate.
#[test]
fn ordinary_feature_coordinates_preserve_relocated_transitive_artifact() -> Result<(), Box<dyn std::error::Error>> {
    let workspace = tempfile::tempdir()?;
    let (provider, artifact, mut input) = checked_package()?;
    let installed = workspace.path().join("installed-catalog");
    fs::rename(&artifact.crate_root, &installed)?;
    input.artifact_root = installed.clone();
    input.import_alias = None;
    // Installed admission uses the original generation association; relocation must not remint that owner.
    let admitted = PreparedLibraryDependencies::admit(&[input], TARGET, TOOLCHAIN, limits())?;
    assert_eq!(admitted.nodes.len(), 1);
    assert!(provider.path().join("loaf.toml").is_file());
    let facade = workspace.path().join("facade");
    let root = workspace.path().join("consumer");
    fs::create_dir_all(facade.join("target/lib/src"))?;
    fs::create_dir(&root)?;
    let facade_artifact = facade.join("target/lib");
    fs::write(facade_artifact.join("src/lib.rs"), "pub fn marker() {}\n")?;
    fs::write(
        facade_artifact.join("Cargo.toml"),
        "[package]\nname='facade'\nversion='0.1.0'\n",
    )?;
    let mut manifest = LibraryManifest::new("facade", "0.1.0");
    manifest.contract_metadata.provider.provider_dependencies.push(
        incan_frontend::library_manifest::ProviderDependencyMetadata {
            kind: incan_frontend::library_manifest::ProviderDependencyKind::PublicPackage,
            dependency_key: "catalog".into(),
            provider_name: "provider".into(),
            provider_version: "0.1.0".into(),
            artifact_digest: digest_provider_artifact(&installed)?,
            relative_artifact_path: "../../../installed-catalog".into(),
            requested_features: BTreeSet::new(),
            default_features: true,
            optional: false,
        },
    );
    manifest.write_to_path(&facade_artifact.join("facade.incnlib"))?;
    fs::write(
        root.join("loaf.toml"),
        "[project]\nname='consumer'\n[dependencies]\nfacade={path='../facade'}\n",
    )?;
    let project = ProjectManifest::load(&root.join("loaf.toml"))?;
    let features = PackageFeaturePlan::resolve(&project, &FeatureSelection::default())?;
    let state = features
        .package(&installed)
        .ok_or("relocated transitive package absent")?;
    assert!(state.manifest.is_none());
    assert_eq!(selected_artifact_root(state)?, fs::canonicalize(&installed)?);
    let edge = features
        .edges()
        .find(|edge| edge.dependency_key == "catalog")
        .ok_or("compiled edge absent")?;
    assert_eq!(edge.to, installed);
    assert!(!installed.join("target/lib").exists());
    let mut injected = state.clone();
    injected.feature_manifest_path = workspace.path().join("missing/provider.incnlib");
    assert!(selected_artifact_root(&injected).is_err());
    Ok(())
}

/// Byte-identical artifacts at different original coordinates cannot share one canonical provider owner.
#[test]
fn ordinary_alias_sharing_refuses_different_physical_owners() -> Result<(), Box<dyn std::error::Error>> {
    let (_first_root, first_artifact, first) = checked_package()?;
    let (_second_root, second_artifact, mut second) = checked_package()?;
    second.import_alias = Some("shape".into());
    assert_eq!(
        digest_provider_artifact(&first_artifact.crate_root)?,
        digest_provider_artifact(&second_artifact.crate_root)?
    );
    assert_ne!(first_artifact.crate_root, second_artifact.crate_root);
    assert!(PreparedLibraryDependencies::admit(&[first, second], TARGET, TOOLCHAIN, limits()).is_err());
    Ok(())
}

/// Genuine same-byte checked outputs do not authorize substituting another original generation or dropping it.
#[test]
fn ordinary_generation_refuses_substitution_and_missing_authority() -> Result<(), Box<dyn std::error::Error>> {
    let (root, artifact, mut input) = checked_package()?;
    let (_other_root, _other_artifact, other_input) = checked_package()?;
    let first = PreparedLibraryDependencies::admit(&[input.clone()], TARGET, TOOLCHAIN, limits())?;
    let other = PreparedLibraryDependencies::admit(&[other_input], TARGET, TOOLCHAIN, limits())?;
    let first_node = first.nodes.values().next().ok_or("first node absent")?;
    let other_node = other.nodes.values().next().ok_or("other node absent")?;
    assert_eq!(first_node.metadata.checked_files(), other_node.metadata.checked_files());
    assert_ne!(
        first_node.metadata.reference().owner_identity,
        other_node.metadata.reference().owner_identity
    );
    let store = OvenStore::with_release(
        packaged_library_loaf_store_root(&artifact.crate_root),
        limits(),
        &incan_oven_facet::compiler_identity(),
    );
    let exported = other_node.generation.export_into(&store)?;
    assert_eq!(
        exported.owner_identity,
        other_node.generation.reference().owner_identity
    );
    let original = first_node.package.clone();
    for replacement in [None, Some(exported)] {
        let mut package = original.clone();
        package.checked_generation = replacement;
        write_packaged_library_loaf_manifest(&artifact.crate_root, &package)?;
        input.handoff_digest = handoff_digest(&artifact)?;
        assert!(PreparedLibraryDependencies::admit(&[input.clone()], TARGET, TOOLCHAIN, limits()).is_err());
    }
    fs::remove_file(root.path().join("loaf.toml"))?;
    fs::remove_file(root.path().join("src/lib.incn"))?;
    assert!(PreparedLibraryDependencies::admit(&[input], TARGET, TOOLCHAIN, limits()).is_err());
    Ok(())
}

/// Portable token authority can remain equal while changed raw checked-source generation must still refuse.
#[test]
fn ordinary_generation_checks_correct_raw_source_domain_and_restoration() -> Result<(), Box<dyn std::error::Error>> {
    let (root, artifact, input) = checked_package()?;
    let admitted = PreparedLibraryDependencies::admit(&[input.clone()], TARGET, TOOLCHAIN, limits())?;
    let node = admitted.nodes.values().next().ok_or("checked node absent")?;
    let source = root.path().join("src/lib.incn");
    let original = fs::read_to_string(&source)?;
    let modified = fs::metadata(&source)?.modified()?;
    fs::write(
        &source,
        original.replace("return 1", "return 1  # changed raw generation"),
    )?;
    fs::File::options().write(true).open(&source)?.set_modified(modified)?;
    assert_eq!(
        digest_baked_project_source_authority(root.path())?,
        node.package.source_authority_digest
    );
    assert_ne!(
        observe_library_source_digest(root.path(), &[])?,
        node.metadata.recipe().source_digest
    );
    assert!(admitted.verify().is_err());
    assert!(PreparedLibraryDependencies::admit(&[input.clone()], TARGET, TOOLCHAIN, limits()).is_err());
    let store = OvenStore::with_release(
        packaged_library_loaf_store_root(&artifact.crate_root),
        limits(),
        &incan_oven_facet::compiler_identity(),
    );
    assert!(
        publish_library_generation(
            &store,
            root.path(),
            &node.package.source_authority_digest,
            &node.metadata
        )
        .is_err()
    );
    fs::write(&source, original)?;
    fs::File::options().write(true).open(&source)?.set_modified(modified)?;
    admitted.verify()?;
    PreparedLibraryDependencies::admit(&[input], TARGET, TOOLCHAIN, limits())?;
    Ok(())
}
