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
    FeatureSelection, NamespaceAuthority, ProviderIdentity, ProviderPlan, ProviderProvenance, ProviderRecord,
};
use oven_model::manifest::ProjectManifest;
use oven_store::digest_bytes;
use oven_store::store::{OvenStore, OvenStoreLimits};

use super::{LibraryDependencyInput, PreparedLibraryDependencies};
use crate::build::library_metadata::requirements::{CheckedLibraryCapture, CheckedLibraryRequirements};
use crate::build::library_metadata::{
    LibraryMetadataDependency, LibraryMetadataRecipe, publish_library_metadata_with_requirements,
};
use crate::build::library_outputs::packaged_library_loaf_store_root;
use crate::build::package_loafs::{read_packaged_library_loaf_manifest, write_packaged_library_loaf_manifest};
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
        source_digest: package.source_authority_digest.clone(),
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

/// Reserved authority comes from an existing validated grant and stays bound to that exact admitted artifact.
#[test]
fn ordinary_admission_reserved_grant_cannot_be_transplanted_or_self_claimed() -> Result<(), Box<dyn std::error::Error>>
{
    let (_root, artifact, mut input) = checked_package()?;
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
    let prior = ProviderPlan::new(LibraryManifestIndex::default(), vec![record], std::iter::empty())?;
    let grant = SelectedProviderNamespace::from_plan(&prior, &identity.stable_key())?;
    input.import_alias = None;
    input.namespace = Some(grant.clone());
    let admitted = PreparedLibraryDependencies::admit(&[input.clone()], TARGET, TOOLCHAIN, limits())?;
    admitted.validate_module_usage(&BTreeSet::from([vec!["std".into(), "core".into()]]))?;
    assert!(
        admitted
            .validate_module_usage(&BTreeSet::from([vec!["std".into(), "unselected".into()]]))
            .is_err()
    );
    assert!(
        admitted
            .provider_plan()
            .active_records()
            .any(|record| record.namespace_claims.contains(&vec!["std".into(), "core".into()]))
    );
    let (_another, other_artifact, mut other) = checked_package()?;
    other.import_alias = None;
    other.namespace = Some(grant);
    assert_ne!(artifact.crate_root, other_artifact.crate_root);
    assert!(PreparedLibraryDependencies::admit(&[other], TARGET, TOOLCHAIN, limits()).is_err());
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
