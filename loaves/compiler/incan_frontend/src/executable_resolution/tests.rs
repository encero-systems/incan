//! Executable public closures keep the selected SDK artifact boundary and its admission checks.

use std::collections::{BTreeSet, HashMap};
use std::error::Error;
use std::fs;
use std::path::Path;
use std::sync::Arc;

use incan_semantics_core::CanonicalSymbolId;
use incan_semantics_core::executable_representation::{EXECUTABLE_REPRESENTATION_VERSION, build_surface};
use sha2::{Digest, Sha256};

use super::{ExecutableResolutionError, resolve_executable_requirements};
use crate::body_ir::build_body_ir_module_v0;
use crate::library_exports::collect_checked_public_exports;
use crate::library_manifest::published_layout::{executable_surface_path, public_executable_identities};
use crate::library_manifest::{ExecutableRepresentationExport, LibraryManifest, digest_provider_artifact};
use crate::library_manifest_index::{LibraryArtifactMetadata, LibraryManifestIndex, LibraryManifestIndexEntry};
use crate::provider::{NamespaceAuthority, ProviderIdentity, ProviderPlan, ProviderProvenance, ProviderRecord};
use crate::typechecker::TypeChecker;
use crate::{lexer, parser};

type TestResult = Result<(), Box<dyn Error>>;

/// Publish checked source through the real executable codec without claiming a native library build.
fn artifact(root: &Path, name: &str, source: &str, plan: ProviderPlan) -> Result<LibraryManifest, Box<dyn Error>> {
    let tokens = lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let module_path = vec!["lib".to_string()];
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(module_path.clone()));
    checker.set_current_package_identity(Some(name.to_string()));
    checker.set_provider_plan(Arc::new(plan));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let exports = collect_checked_public_exports(&program, &checker);
    let mut manifest = LibraryManifest::from_checked_exports(name, "1.2.3", &exports);
    let module = build_body_ir_module_v0(&program, &module_path, checker.type_info());
    let unrepresentable = module
        .bodies
        .iter()
        .filter(|body| body.first_representation_gap().is_some())
        .filter_map(|body| body.canonical.clone())
        .collect();
    let bytes = build_surface(
        &[module],
        name,
        "1.2.3",
        &public_executable_identities(&manifest),
        &unrepresentable,
    )?;
    manifest.contract_metadata.executable_representation = Some(ExecutableRepresentationExport {
        representation_version: EXECUTABLE_REPRESENTATION_VERSION,
        content_digest: hex::encode(Sha256::digest(&bytes)),
    });
    fs::create_dir_all(root.join("src"))?;
    fs::write(root.join("src/lib.rs"), "// Semantic artifact fixture only.\n")?;
    fs::write(
        root.join("Cargo.toml"),
        format!("[package]\nname = '{name}'\nversion = '1.2.3'\n"),
    )?;
    let manifest_path = root.join(format!("{name}.incnlib"));
    let surface = executable_surface_path(&manifest_path, &manifest).ok_or("surface path missing")?;
    fs::create_dir_all(surface.parent().ok_or("surface directory missing")?)?;
    fs::write(surface, bytes)?;
    manifest.write_to_path(&manifest_path)?;
    Ok(manifest)
}

/// Retain an independently selected SDK artifact, with reserved authority and a physical manifest.
fn sdk_record(root: &Path, manifest: &LibraryManifest) -> Result<ProviderRecord, Box<dyn Error>> {
    Ok(ProviderRecord {
        identity: ProviderIdentity {
            name: manifest.name.clone(),
            version: manifest.version.clone(),
            digest: digest_provider_artifact(root)?,
            feature_projection: BTreeSet::new(),
        },
        provenance: ProviderProvenance::Sdk {
            sdk_identity: "test-sdk".into(),
            component_id: "fixture".into(),
            inventory_path: None,
        },
        authority: NamespaceAuthority::SdkReserved,
        namespace_claims: BTreeSet::from([vec!["std".into(), "fixture".into()]]),
        available: true,
        enabled: true,
        manifest: Some(Arc::new(manifest.clone())),
        artifact: Some(LibraryArtifactMetadata::from_manifest_path(
            &manifest.name,
            &manifest.name,
            root.join(format!("{}.incnlib", manifest.name)),
            root.to_path_buf(),
        )),
        implementation_facets: Vec::new(),
    })
}

/// Resolve an ordinary declaration whose only transitive type context belongs to the selected SDK.
fn package_with_sdk(root: &Path) -> Result<(ProviderPlan, CanonicalSymbolId, ProviderRecord), Box<dyn Error>> {
    let sdk_root = root.join("sdk");
    let sdk = artifact(
        &sdk_root,
        "sdk_fixture",
        "pub model Payload:\n    pub value: int\n",
        ProviderPlan::default(),
    )?;
    let sdk_record = sdk_record(&sdk_root, &sdk)?;
    let producer_index = LibraryManifestIndex::from_entries(HashMap::from([(
        "fixture".into(),
        LibraryManifestIndexEntry::Loaded {
            metadata: LibraryArtifactMetadata::from_manifest_path(
                "fixture",
                &sdk.name,
                sdk_root.join("sdk_fixture.incnlib"),
                sdk_root,
            ),
            manifest: Box::new(sdk),
        },
    )]));
    let producer_plan = ProviderPlan::from_resolved_inputs(producer_index, None, None, None, [])?;
    let package_root = root.join("package");
    let package = artifact(
        &package_root,
        "reader",
        "from pub::fixture import Payload\n\npub def read(value: Payload) -> int:\n    return value.value\n",
        producer_plan,
    )?;
    let identity = package
        .contract_metadata
        .identity_graph
        .canonical_for_public_name("read")
        .ok_or("read identity missing")?;
    let index = LibraryManifestIndex::from_entries(HashMap::from([(
        "reader".into(),
        LibraryManifestIndexEntry::Loaded {
            metadata: LibraryArtifactMetadata::from_manifest_path(
                "reader",
                "reader",
                package_root.join("reader.incnlib"),
                package_root,
            ),
            manifest: Box::new(package),
        },
    )]));
    let ordinary = ProviderPlan::from_resolved_inputs(index.clone(), None, None, None, [])?;
    let mut records = ordinary.records().cloned().collect::<Vec<_>>();
    records.push(sdk_record.clone());
    Ok((ProviderPlan::new(index, records, [])?, identity, sdk_record))
}

/// An ordinary public body loads its SDK type context selectively from the active artifact.
#[test]
fn package_closure_reaches_the_selected_sdk_artifact() -> TestResult {
    let temporary = tempfile::tempdir()?;
    let (plan, identity, _) = package_with_sdk(temporary.path())?;
    let resolved = resolve_executable_requirements(&plan, &BTreeSet::from([identity]))?;
    assert!(
        resolved
            .modules
            .iter()
            .any(|module| module.bodies.iter().any(|body| body.name == "read"))
    );
    assert!(resolved.modules.iter().any(|module| {
        module
            .nominal_declarations
            .iter()
            .any(|declaration| declaration.name == "Payload")
    }));
    assert_eq!(
        resolved.package_versions.get("sdk_fixture").map(String::as_str),
        Some("1.2.3")
    );
    Ok(())
}

/// A disabled provider cannot satisfy a public closure just because its bytes are installed.
#[test]
fn package_closure_rejects_a_disabled_sdk_artifact() -> TestResult {
    let temporary = tempfile::tempdir()?;
    let (plan, identity, _) = package_with_sdk(temporary.path())?;
    let records = plan
        .records()
        .cloned()
        .map(|mut record| {
            if matches!(record.authority, NamespaceAuthority::SdkReserved) {
                record.enabled = false;
            }
            record
        })
        .collect::<Vec<_>>();
    let disabled = ProviderPlan::new(plan.library_manifest_index().clone(), records, [])?;
    let result = resolve_executable_requirements(&disabled, &BTreeSet::from([identity]));
    assert!(matches!(result, Err(ExecutableResolutionError::UnknownPackage { library }) if library == "sdk_fixture"));
    Ok(())
}

/// SDK context retains the executable descriptor's digest check when reached through an ordinary package.
#[test]
fn package_closure_rejects_altered_sdk_surface_bytes() -> TestResult {
    let temporary = tempfile::tempdir()?;
    let (plan, identity, sdk) = package_with_sdk(temporary.path())?;
    let manifest = sdk.manifest.as_ref().ok_or("SDK manifest missing")?;
    let metadata = sdk.artifact.as_ref().ok_or("SDK artifact missing")?;
    let surface = executable_surface_path(&metadata.manifest_path, manifest).ok_or("SDK surface missing")?;
    let mut bytes = fs::read(&surface)?;
    let last = bytes.last_mut().ok_or("SDK surface empty")?;
    *last ^= 1;
    fs::write(surface, bytes)?;
    let result = resolve_executable_requirements(&plan, &BTreeSet::from([identity]));
    assert!(
        matches!(result, Err(ExecutableResolutionError::UnusableRepresentation { library, .. }) if library == "sdk_fixture")
    );
    Ok(())
}

/// Canonical package names cannot select between different active SDK projections.
#[test]
fn package_closure_rejects_conflicting_sdk_instances() -> TestResult {
    let temporary = tempfile::tempdir()?;
    let (plan, identity, mut sdk) = package_with_sdk(temporary.path())?;
    sdk.namespace_claims.clear();
    sdk.identity.feature_projection.insert("different".into());
    let mut records = plan.records().cloned().collect::<Vec<_>>();
    records.push(sdk);
    let conflicting = ProviderPlan::new(plan.library_manifest_index().clone(), records, [])?;
    let result = resolve_executable_requirements(&conflicting, &BTreeSet::from([identity]));
    assert!(
        matches!(result, Err(ExecutableResolutionError::AmbiguousPackage { library, .. }) if library == "sdk_fixture")
    );
    Ok(())
}

/// An ordinary package with the SDK's name cannot replace its selected artifact by map insertion.
#[test]
fn package_closure_rejects_conflicting_package_and_sdk_instances() -> TestResult {
    let temporary = tempfile::tempdir()?;
    let (_, _, sdk) = package_with_sdk(temporary.path())?;
    let root = temporary.path().join("conflicting-package");
    let manifest = artifact(
        &root,
        "sdk_fixture",
        "pub model Payload:\n    pub value: int\n    pub other: int\n",
        ProviderPlan::default(),
    )?;
    let identity = manifest
        .contract_metadata
        .identity_graph
        .canonical_for_public_name("Payload")
        .ok_or("Payload identity missing")?;
    let index = LibraryManifestIndex::from_entries(HashMap::from([(
        "different".into(),
        LibraryManifestIndexEntry::Loaded {
            metadata: LibraryArtifactMetadata::from_manifest_path(
                "different",
                &manifest.name,
                root.join("sdk_fixture.incnlib"),
                root,
            ),
            manifest: Box::new(manifest),
        },
    )]));
    let ordinary = ProviderPlan::from_resolved_inputs(index.clone(), None, None, None, [])?;
    let mut records = ordinary.records().cloned().collect::<Vec<_>>();
    records.push(sdk);
    let plan = ProviderPlan::new(index, records, [])?;
    let result = resolve_executable_requirements(&plan, &BTreeSet::from([identity]));
    assert!(
        matches!(result, Err(ExecutableResolutionError::AmbiguousPackage { library, .. }) if library == "sdk_fixture")
    );
    Ok(())
}
