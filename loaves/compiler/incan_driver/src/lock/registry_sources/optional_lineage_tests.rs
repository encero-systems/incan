//! Genuine Store-backed optional inspection lineage controls for #1337/#1698.

use std::collections::BTreeMap;
use std::error::Error;
use std::fs::{self, File, FileTimes};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::{
    ProjectRegistrySourceAuthoritySelection, prepare_optional_project_registry_source_authorities_with_native_sdk,
    prepare_project_registry_source_authorities_with_native_sdk,
};
use crate::build::NativeSdkCommandContext;
use crate::build::native_sdk_plan::select_native_sdk_plan_with_context;
use oven_model::manifest::{DependencySource, DependencySpec};
use oven_rustc::rustc::{
    OVEN_PROJECT_INSPECTION_AUTHORITY_SCHEMA_VERSION, OvenLoadedProjectInspectionAuthority,
    OvenProjectInspectionAuthorityPayload, OvenProjectInspectionAuthorityRef, OvenProjectInspectionConstituent,
    OvenProjectInspectionTestDependencyEnvelope, load_project_inspection_authority,
};
use oven_rustc::sdk_closure::{SdkLockedUnit, SdkNativeArtifact};
use oven_store::store::{
    OvenArtifactKind, OvenArtifactMaterializedFile, OvenArtifactPublishRequest, OvenStore, OvenStoreLimits,
};
use oven_store::{
    OvenGeneratedProjectRequest, OvenReceipt, digest_bytes, digest_dependency_specs, receipt_generated_project,
    receipt_with_build_unit_input,
};

/// Source, independently admitted native generations and genuine published inspection lineage.
struct LineageFixture {
    root: tempfile::TempDir,
    store: OvenStore,
    old_context: Arc<NativeSdkCommandContext>,
    current_context: Arc<NativeSdkCommandContext>,
    receipt: OvenReceipt,
    reference: OvenProjectInspectionAuthorityRef,
    plan_identity: String,
    source_digest: String,
}

impl LineageFixture {
    /// Publish original native owners and use production plan preparation to construct the retained lineage.
    fn new() -> Result<Self, Box<dyn Error>> {
        let root = tempfile::tempdir()?;
        fs::create_dir_all(root.path().join("src"))?;
        fs::write(root.path().join("src/main.incn"), "def main():\n    pass\n")?;
        fs::write(
            root.path().join("loaf.toml"),
            "[project]\nname='lineage'\nversion='1.0.0'\n",
        )?;
        let old_context = native_context(root.path(), "old")?;
        let current_context = native_context(root.path(), "current")?;
        let store = OvenStore::new(root.path().join("project-store"), limits());
        let receipt = current_receipt(root.path(), &old_context)?;
        let prepared = select_native_sdk_plan_with_context(&store, &receipt, &[], Some(&old_context))?
            .ok_or("original native plan was not prepared")?;
        let plan_identity = prepared.plan_selection.report_identity();
        let source_digest = digest_bytes(&fs::read(root.path().join("src/main.incn"))?);
        let payload = OvenProjectInspectionAuthorityPayload {
            schema_version: OVEN_PROJECT_INSPECTION_AUTHORITY_SCHEMA_VERSION,
            project_identity: "lineage-project".to_string(),
            source_authority_digest: source_digest.clone(),
            compiler_version: "lineage-compiler".to_string(),
            registry_lock_digest: digest_bytes(&[]),
            registry_source_dependencies: Vec::new(),
            dev_registry_source_dependencies: Vec::new(),
            test_dependency_envelope: Some(OvenProjectInspectionTestDependencyEnvelope {
                constituent_index: 0,
                provider_constituents: Vec::new(),
                dependency_surface_digest: digest_dependency_specs(&[], incan_oven_facet::provider_hooks().as_ref())?,
                dependency_roots: BTreeMap::new(),
            }),
            constituents: vec![OvenProjectInspectionConstituent::Stored {
                identity: plan_identity.clone(),
                artifact_kind: OvenArtifactKind::DirectRustcPlan,
                receipt: receipt.clone(),
                base_loaf_identity: None,
            }],
            registry_sources: Vec::new(),
            generated_out_dirs: Vec::new(),
        };
        let published = store.publish(&OvenArtifactPublishRequest {
            receipt: receipt.clone(),
            domain: "lineage-control".to_string(),
            kind: OvenArtifactKind::ProjectInspectionAuthority,
            payload: serde_json::to_vec(&payload)?,
            materialized_files: Vec::new(),
            materialized_directories: Vec::new(),
        })?;
        let reference = OvenProjectInspectionAuthorityRef {
            identity: published.identity,
            receipt_identity: published.receipt_identity,
            build_unit_identity: published.build_unit_identity,
        };
        Ok(Self {
            root,
            store,
            old_context,
            current_context,
            receipt,
            reference,
            plan_identity,
            source_digest,
        })
    }

    /// Use the actual canonical loader, including original authority and constituent validation.
    fn load(&self) -> Result<OvenLoadedProjectInspectionAuthority, Box<dyn Error>> {
        Ok(load_project_inspection_authority(
            &self.store,
            &self.reference,
            "lineage-project",
            &self.source_digest,
            "lineage-compiler",
        )?)
    }
}

/// Bound policy leaves both generations and the project authority available throughout these controls.
fn limits() -> OvenStoreLimits {
    OvenStoreLimits::new(1_000_000, 1_000_000, 1_000_000)
}

/// Publish a real Engine owner and canonically admit its exact source/output/receipt descriptors.
fn native_context(root: &Path, generation: &str) -> Result<Arc<NativeSdkCommandContext>, Box<dyn Error>> {
    let authority = root.join(generation);
    let version = if generation == "old" { "1.0.0" } else { "2.0.0" };
    fs::create_dir(&authority)?;
    let source = authority.join("unit.rs");
    fs::write(&source, format!("pub const GENERATION: &str = {generation:?};\n"))?;
    let receipt = receipt_generated_project(
        &OvenGeneratedProjectRequest::new(
            &authority,
            "fixture",
            version,
            "fixture-target",
            "fixture-toolchain",
            "debug",
            Vec::new(),
        )
        .with_generated_source("generated-root", &source),
    )?;
    let binding = SdkLockedUnit {
        loaf: "crates-io/fixture".to_string(),
        version: version.to_string(),
        archive_digest: digest_bytes(&fs::read(&source)?),
        domain: "target".to_string(),
        features: Vec::new(),
        target_predicates: Vec::new(),
        edges: None,
    };
    let output = authority.join("libfixture.rlib");
    fs::write(&output, format!("opaque native owner bytes for {generation}"))?;
    let store = OvenStore::new(authority.join("native-store"), limits());
    let published = store.publish(&OvenArtifactPublishRequest {
        receipt: receipt.clone(),
        domain: "sdk-source-unit-target".to_string(),
        kind: OvenArtifactKind::Engine,
        payload: serde_json::to_vec(&binding.identity_binding())?,
        materialized_files: vec![OvenArtifactMaterializedFile {
            source_path: output.clone(),
            relative_path: "libfixture.rlib".to_string(),
        }],
        materialized_directories: Vec::new(),
    })?;
    let unit = SdkNativeArtifact {
        binding: binding.clone(),
        store_identity: published.identity,
        receipt_identity: receipt.identity.clone(),
        relative_path: "libfixture.rlib".to_string(),
        digest: digest_bytes(&fs::read(output)?),
    };
    fs::write(
        authority.join(".sealed-native-units.json"),
        serde_json::to_vec(&serde_json::json!({
            "schema_version": 1, "store": store.root(), "units": [unit],
        }))?,
    )?;
    fs::write(
        authority.join(".sealed-native-receipts.json"),
        serde_json::to_vec(&BTreeMap::from([(
            serde_json::to_string(&binding.identity_binding())?,
            receipt.identity,
        )]))?,
    )?;
    Ok(NativeSdkCommandContext::from_inventory(Arc::new(
        incan_provider::SdkInventory {
            root: authority,
            sdk_id: generation.to_string(),
            sdk_version: "1.0.0".to_string(),
            compiler_requirement: "*".to_string(),
            provider_codegen_revision: incan_lang::version::SDK_PROVIDER_CODEGEN_REVISION,
            components: BTreeMap::new(),
            profiles: BTreeMap::new(),
        },
    ))?)
}

/// Compute fresh source and native build-unit inputs using the command's actual admitted generation.
fn current_receipt(root: &Path, context: &NativeSdkCommandContext) -> Result<OvenReceipt, Box<dyn Error>> {
    let generated = root.join("runner.rs");
    fs::write(&generated, "fn main() {}\n")?;
    Ok(receipt_generated_project(
        &OvenGeneratedProjectRequest::new(
            root,
            "lineage",
            "1.0.0",
            "fixture-target",
            "fixture-toolchain",
            "debug",
            Vec::new(),
        )
        .with_generated_source("generated-root", generated)
        .with_build_unit_input("sdk-native-closure", digest_bytes(context.receipt_catalog()?))
        .with_build_unit_input("sdk-native-consumer-plan", "v1"),
    )?)
}

/// Locate the primary file of a genuinely admitted owner for out-of-band mutation tests only.
fn primary_payload(artifact_root: &Path) -> Result<PathBuf, Box<dyn Error>> {
    Ok(artifact_root
        .parent()
        .ok_or("admitted root has no entry parent")?
        .join("payload"))
}

/// Damage equal-length bytes while restoring permissions and timestamp, so a metadata-only check cannot pass.
fn damage_preserving_metadata(path: &Path) -> Result<(), Box<dyn Error>> {
    let metadata = fs::metadata(path)?;
    let mut bytes = fs::read(path)?;
    let first = bytes.first_mut().ok_or("mutation fixture is empty")?;
    *first ^= 1;
    let mut writable = metadata.permissions();
    writable.set_readonly(false);
    fs::set_permissions(path, writable)?;
    let mut file = File::options().write(true).open(path)?;
    file.write_all(&bytes)?;
    file.set_times(FileTimes::new().set_modified(metadata.modified()?))?;
    fs::set_permissions(path, metadata.permissions())?;
    Ok(())
}

/// Matching first and repeat selections retain the real project authority and its exact test plan.
#[test]
fn optional_lineage_matching_first_repeat_retains_exact_test_plan() -> Result<(), Box<dyn Error>> {
    let fixture = LineageFixture::new()?;
    for _ in 0..2 {
        let ProjectRegistrySourceAuthoritySelection::Prepared(prepared) =
            prepare_optional_project_registry_source_authorities_with_native_sdk(
                fixture.load()?,
                Some(Arc::clone(&fixture.old_context)),
            )?
        else {
            return Err("matching native lineage was classified stale".into());
        };
        assert_eq!(prepared.authority_identity(), fixture.reference.identity);
        let plan = prepared
            .test_dependency_plan(&[])?
            .ok_or("matching lineage lost its test plan")?;
        assert_eq!(plan.report_identity(), fixture.plan_identity);
        assert_eq!(fixture.old_context.admitted_owner_count(), 1);
    }
    Ok(())
}

/// Intact stale lineage is optional, while fresh current planning and strict historical receipt checks remain exact.
#[test]
fn optional_lineage_stale_generation_prepares_current_first_repeat_without_deletion() -> Result<(), Box<dyn Error>> {
    let fixture = LineageFixture::new()?;
    let old_bytes = serde_json::to_vec(&fixture.receipt)?;
    let mut current_identity = None;
    for _ in 0..2 {
        assert!(matches!(
            prepare_optional_project_registry_source_authorities_with_native_sdk(
                fixture.load()?,
                Some(Arc::clone(&fixture.current_context)),
            )?,
            ProjectRegistrySourceAuthoritySelection::StaleNativeGeneration
        ));
        let receipt = current_receipt(fixture.root.path(), &fixture.current_context)?;
        assert_ne!(receipt.build_unit_identity, fixture.receipt.build_unit_identity);
        let prepared =
            select_native_sdk_plan_with_context(&fixture.store, &receipt, &[], Some(&fixture.current_context))?
                .ok_or("current native generation did not prepare a real plan")?;
        if let Some(first) = &current_identity {
            assert_eq!(&prepared.plan_selection.report_identity(), first);
        } else {
            current_identity = Some(prepared.plan_selection.report_identity());
        }
        assert_eq!(serde_json::to_vec(&fixture.receipt)?, old_bytes);
        fixture.load()?.verify()?;
    }
    let error = fixture
        .current_context
        .owners_for_receipt(&fixture.receipt)
        .err()
        .ok_or("strict owner selection accepted an old generation")?;
    assert_eq!(
        error.message,
        "receipt-selected native SDK differs from command admission"
    );
    assert!(
        prepare_project_registry_source_authorities_with_native_sdk(
            fixture.load()?,
            Some(Arc::clone(&fixture.current_context)),
        )
        .is_err()
    );
    Ok(())
}

/// A stale optional lineage grants no authority to compile or select a missing current dependency.
#[test]
fn optional_lineage_stale_generation_refuses_missing_current_dependency() -> Result<(), Box<dyn Error>> {
    let fixture = LineageFixture::new()?;
    assert!(matches!(
        prepare_optional_project_registry_source_authorities_with_native_sdk(
            fixture.load()?,
            Some(Arc::clone(&fixture.current_context)),
        )?,
        ProjectRegistrySourceAuthoritySelection::StaleNativeGeneration
    ));
    let dependency = DependencySpec {
        crate_name: "fixture".to_string(),
        version: Some("1".to_string()),
        features: Vec::new(),
        default_features: true,
        source: DependencySource::Registry,
        optional: false,
        package: None,
    };
    assert!(
        crate::build::native_sdk_plan::project_dependencies_without_sdk_registry_inputs_with_context(
            std::slice::from_ref(&dependency),
            Some(&fixture.receipt),
            Some(&fixture.old_context),
        )?
        .is_empty(),
        "the historical generation must actually cover the now-missing root"
    );
    let before = fs::read_dir(fixture.store.root().join("entries"))?.count();
    let error = select_native_sdk_plan_with_context(
        &fixture.store,
        &current_receipt(fixture.root.path(), &fixture.current_context)?,
        &[dependency],
        Some(&fixture.current_context),
    )
    .err()
    .ok_or("missing current dependency was admitted")?;
    assert!(error.message.contains("does not cover dependency `fixture`"));
    assert_eq!(fs::read_dir(fixture.store.root().join("entries"))?.count(), before);
    Ok(())
}

/// Source-owner damage after canonical loading is an integrity refusal, even when its generation is stale.
#[test]
fn optional_lineage_refuses_original_authority_damage_after_load() -> Result<(), Box<dyn Error>> {
    let fixture = LineageFixture::new()?;
    let authority = fixture.load()?;
    damage_preserving_metadata(&primary_payload(authority.artifact_root())?)?;
    assert!(
        prepare_optional_project_registry_source_authorities_with_native_sdk(
            authority,
            Some(Arc::clone(&fixture.current_context)),
        )
        .is_err()
    );
    Ok(())
}

/// Constituent corruption and disappearance after canonical loading cannot become stale-generation misses.
#[test]
fn optional_lineage_refuses_constituent_damage_and_missing_original_after_load() -> Result<(), Box<dyn Error>> {
    for remove in [false, true] {
        let fixture = LineageFixture::new()?;
        let authority = fixture.load()?;
        let constituent = authority
            .stored_constituents
            .first()
            .ok_or("fixture lost its constituent")?;
        let payload = primary_payload(&constituent.artifact_root)?;
        if remove {
            fs::remove_file(payload)?;
        } else {
            damage_preserving_metadata(&payload)?;
        }
        assert!(
            prepare_optional_project_registry_source_authorities_with_native_sdk(
                authority,
                Some(Arc::clone(&fixture.current_context)),
            )
            .is_err()
        );
    }
    Ok(())
}

/// A public decoded receipt cannot replace the original authority's immutable admitted bytes.
#[test]
fn optional_lineage_refuses_decoded_receipt_substitution_after_load() -> Result<(), Box<dyn Error>> {
    let fixture = LineageFixture::new()?;
    let mut authority = fixture.load()?;
    let OvenProjectInspectionConstituent::Stored { receipt, .. } = authority
        .payload
        .constituents
        .first_mut()
        .ok_or("fixture lost its decoded constituent")?
    else {
        return Err("fixture constituent is not stored".into());
    };
    *receipt = current_receipt(fixture.root.path(), &fixture.current_context)?;
    assert!(
        prepare_optional_project_registry_source_authorities_with_native_sdk(
            authority,
            Some(Arc::clone(&fixture.current_context)),
        )
        .is_err()
    );
    Ok(())
}

/// Missing retained children and a different genuine selected plan cannot replace the sealed original constituent.
#[test]
fn optional_lineage_refuses_retained_constituent_substitution_after_load() -> Result<(), Box<dyn Error>> {
    for substitute in [false, true] {
        let fixture = LineageFixture::new()?;
        let mut authority = fixture.load()?;
        authority.stored_constituents.clear();
        if substitute {
            let prepared = select_native_sdk_plan_with_context(
                &fixture.store,
                &current_receipt(fixture.root.path(), &fixture.current_context)?,
                &[],
                Some(&fixture.current_context),
            )?
            .ok_or("current native plan was not prepared")?;
            authority.stored_constituents = fixture
                .store
                .select_payloads_for_execution(&[prepared.plan_selection.report_identity()])?;
        }
        assert!(
            prepare_optional_project_registry_source_authorities_with_native_sdk(
                authority,
                Some(Arc::clone(&fixture.current_context)),
            )
            .is_err()
        );
    }
    Ok(())
}

/// Changed current publication and a malformed later receipt remain errors after an earlier genuine mismatch.
#[test]
fn optional_lineage_refuses_changed_command_admission_and_malformed_receipt() -> Result<(), Box<dyn Error>> {
    let fixture = LineageFixture::new()?;
    let malformed = receipt_with_build_unit_input(&fixture.receipt, "sdk-native-closure", "malformed")?;
    assert!(
        fixture
            .current_context
            .receipts_match_admission(&[&fixture.receipt, &malformed])
            .is_err()
    );
    let mut invalid_identity = fixture.receipt.clone();
    invalid_identity.identity = digest_bytes(b"substituted receipt identity");
    assert!(
        fixture
            .current_context
            .receipts_match_admission(&[&fixture.receipt, &invalid_identity])
            .is_err()
    );
    let authority = fixture.load()?;
    fs::write(fixture.root.path().join("current/.sealed-native-receipts.json"), "{}")?;
    let error = prepare_optional_project_registry_source_authorities_with_native_sdk(
        authority,
        Some(Arc::clone(&fixture.current_context)),
    )
    .err()
    .ok_or("changed command publication became a stale-generation miss")?;
    assert!(error.message.contains("publication changed after command admission"));
    Ok(())
}

/// Byte-identical original identities in another Store do not replace the authority's retained physical child.
#[test]
fn optional_lineage_refuses_equal_identity_at_different_store_after_load() -> Result<(), Box<dyn Error>> {
    let fixture = LineageFixture::new()?;
    let mut authority = fixture.load()?;
    let original = authority
        .stored_constituents
        .first()
        .ok_or("fixture lost its constituent")?;
    assert!(original.admitted_materialized_files().is_empty());
    assert!(original.admitted_materialized_directories().is_empty());
    let destination = OvenStore::new(fixture.root.path().join("different-store"), limits());
    let copied = destination.publish_verified_import(
        &OvenArtifactPublishRequest {
            receipt: fixture.receipt.clone(),
            domain: original.manifest.domain.clone(),
            kind: original.manifest.kind,
            payload: original.payload.clone(),
            materialized_files: Vec::new(),
            materialized_directories: Vec::new(),
        },
        original.admitted_materialized_files(),
        original.admitted_materialized_directories(),
    )?;
    assert_eq!(copied.identity, original.manifest.identity);
    let replacement = destination.select_payloads_for_execution(&[copied.identity])?;
    let different_root = &replacement
        .first()
        .ok_or("copied owner was not admitted")?
        .artifact_root;
    assert_ne!(different_root, &original.artifact_root);
    authority.stored_constituents = replacement;
    let error = prepare_optional_project_registry_source_authorities_with_native_sdk(
        authority,
        Some(Arc::clone(&fixture.current_context)),
    )
    .err()
    .ok_or("a different physical child became a stale-generation miss")?;
    assert!(error.message.contains("original admitted coordinate"));
    Ok(())
}
