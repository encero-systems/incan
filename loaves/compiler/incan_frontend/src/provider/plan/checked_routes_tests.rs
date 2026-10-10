//! Semantic routes across checked source and admitted materialized dependency owners (#1337/#1698).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::Path;
use std::sync::Arc;

use sha2::{Digest, Sha256};

use super::{ProviderModuleResolution, ProviderPlan};
use crate::library_manifest::{
    LibraryManifest, NativeProviderArtifact, NativeProviderOutput, NominalTypeOriginExport, ProviderDependencyKind,
    ProviderDependencyMetadata, ProviderModuleClaim, digest_provider_artifact,
};
use crate::library_manifest_index::{
    LibraryArtifactMetadata, LibraryManifestIndex, LibraryManifestIndexEntry, load_provider_dependency_artifact,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Publish real checked nominal exports with exact package/declaration provenance and a root module claim.
fn nominal_manifest(name: &str) -> Result<LibraryManifest, Box<dyn std::error::Error>> {
    let source = "pub model Widget:\n    pub value: int\n\npub newtype Tag = int\n";
    let tokens = crate::lexer::lex(source).map_err(|errors| format!("lex: {errors:?}"))?;
    let program = crate::parser::parse(&tokens).map_err(|errors| format!("parse: {errors:?}"))?;
    let mut checker = crate::typechecker::TypeChecker::new();
    checker.set_current_package_identity(Some(name.to_string()));
    checker.set_current_module_path(Some(vec!["lib".to_string()]));
    checker
        .check_program(&program)
        .map_err(|errors| format!("check: {errors:?}"))?;
    let exports = crate::library_exports::collect_checked_public_exports(&program, &checker);
    let mut manifest = LibraryManifest::from_checked_exports(name, "1.0.0", &exports);
    manifest
        .contract_metadata
        .provider
        .namespace_claims
        .push(ProviderModuleClaim {
            module_path: Vec::new(),
            required_features: BTreeSet::new(),
        });
    Ok(manifest)
}

/// Exercise the real catalog loader with native descriptor bytes; no native executable authority is claimed.
fn materialized_entry(
    root: &Path,
    alias: &str,
    mut manifest: LibraryManifest,
) -> Result<LibraryManifestIndexEntry, Box<dyn std::error::Error>> {
    fs::create_dir_all(root.join("src"))?;
    fs::create_dir_all(root.join("native"))?;
    fs::write(root.join("src/lib.rs"), "pub struct Widget;\n")?;
    let output = b"catalog-only native fixture";
    fs::write(root.join("native/facade.rlib"), output)?;
    manifest.contract_metadata.provider.semantic_source_digest = Some(format!("sha256:{}", "a".repeat(64)));
    manifest.write_to_path(&root.join(format!("{}.incnlib", manifest.name)))?;
    let native = NativeProviderArtifact {
        schema_version: 1,
        name: manifest.name.clone(),
        version: manifest.version.clone(),
        receipts: BTreeMap::from([("fixture".to_string(), format!("sha256:{}", "b".repeat(64)))]),
        output: NativeProviderOutput {
            crate_name: manifest.name.replace('-', "_"),
            relative_path: "native/facade.rlib".to_string(),
            digest: format!("sha256:{}", hex::encode(Sha256::digest(output))),
        },
    };
    fs::write(root.join("native-provider.json"), serde_json::to_vec(&native)?)?;
    let entry = load_provider_dependency_artifact(alias, root);
    if let LibraryManifestIndexEntry::Failed(failure) = &entry {
        return Err(failure.message.clone().into());
    }
    Ok(entry)
}

/// Retain a source contract without generating any facade or executable artifact.
fn source_entry(root: &Path, alias: &str, manifest: LibraryManifest) -> LibraryManifestIndexEntry {
    let metadata = LibraryArtifactMetadata::for_checked_source(alias, &manifest.name, root);
    LibraryManifestIndexEntry::Loaded {
        manifest: Box::new(manifest),
        metadata,
    }
}

/// Use the ordinary provider projector so the test's identities, alias grants and graph come from admission.
fn plan(entries: Vec<(&str, LibraryManifestIndexEntry)>) -> Result<ProviderPlan, Box<dyn std::error::Error>> {
    let index = LibraryManifestIndex::from_entries(
        entries
            .into_iter()
            .map(|(alias, entry)| (alias.to_string(), entry))
            .collect(),
    );
    Ok(ProviderPlan::from_resolved_inputs(index, None, None, None, [])?)
}

/// Read an exact nominal origin from one selected owner, rather than inventing a matching digest or package name.
fn origin(plan: &ProviderPlan, name: &str) -> Result<NominalTypeOriginExport, Box<dyn std::error::Error>> {
    let selected = plan
        .records()
        .find(|record| record.identity.name == name)
        .ok_or("selected owner missing")?;
    let canonical = selected
        .manifest
        .as_ref()
        .ok_or("selected manifest missing")?
        .contract_metadata
        .identity_graph
        .exports
        .iter()
        .find(|export| export.public_name == "Widget")
        .and_then(|export| export.canonical.clone())
        .ok_or("selected nominal identity missing")?;
    Ok(NominalTypeOriginExport {
        provider: selected.identity.clone(),
        canonical,
    })
}

/// A fresh local container reaches an installed nominal semantically, without exposing its private namespace/native
/// route.
#[test]
fn dev7_mixed_nominal_routes_source_to_materialized() -> TestResult {
    let root = tempfile::tempdir()?;
    let child = plan(vec![(
        "selected_leaf",
        materialized_entry(&root.path().join("leaf"), "selected_leaf", nominal_manifest("leaf")?)?,
    )])?;
    let leaf = origin(&child, "leaf")?;
    let mut consumer = plan(vec![(
        "middle",
        source_entry(&root.path().join("middle"), "middle", nominal_manifest("middle")?),
    )])?;
    consumer.retain_checked_source_dependencies("middle", &child)?;
    let (metadata, export, route) = consumer.public_nominal_metadata_projection("middle", &leaf)?;
    assert_eq!(metadata.identity, leaf.provider);
    assert_eq!(export.canonical, Some(leaf.canonical.clone()));
    assert_eq!(route, ["selected_leaf"]);
    assert!(!metadata.materialized);
    let original_owner = child
        .public_artifacts()
        .find(|artifact| artifact.identity == leaf.provider)
        .ok_or("original materialized owner missing")?;
    assert!(Arc::ptr_eq(&metadata.manifest, &original_owner.manifest));
    let tag = NominalTypeOriginExport {
        provider: leaf.provider.clone(),
        canonical: original_owner
            .manifest
            .contract_metadata
            .identity_graph
            .exports
            .iter()
            .find(|export| export.public_name == "Tag")
            .and_then(|export| export.canonical.clone())
            .ok_or("checked newtype missing")?,
    };
    assert_eq!(
        consumer.public_nominal_metadata_projection("middle", &tag)?.2,
        ["selected_leaf"]
    );

    assert_eq!(
        consumer.declaring_public_provider("middle", &leaf.canonical.hydrate().ok_or("invalid canonical")?)?,
        leaf.provider
    );
    assert_eq!(consumer.public_artifacts().count(), 0);
    assert!(consumer.public_nominal_projection("middle", &leaf).is_err());
    assert!(consumer.public_nominal_declaration(&leaf).is_err());
    assert!(consumer.library_manifest_index().get("selected_leaf").is_none());
    for path in [vec!["pub", "selected_leaf"], vec!["pub", "middle", "selected_leaf"]] {
        assert!(matches!(
            consumer.resolve_module(&path.into_iter().map(String::from).collect::<Vec<_>>()),
            ProviderModuleResolution::Unknown
        ));
    }
    assert!(consumer.records().all(|record| record.artifact.is_none()));
    let projected = ProviderPlan::default().with_checked_source_graph(&consumer);
    let rebuilt = ProviderPlan::from_resolved_inputs(consumer.library_manifest_index().clone(), None, None, None, [])?
        .with_checked_source_graph(&projected)
        .project_module_usage(BTreeSet::new());
    assert_eq!(
        rebuilt.public_nominal_metadata_projection("middle", &leaf)?.2,
        ["selected_leaf"]
    );
    Ok(())
}

/// A direct physical alias in the parent cannot replace the route or layout authority of a source-origin lookup.
#[test]
fn dev7_mixed_nominal_routes_direct_owner_does_not_override_source_route() -> TestResult {
    let root = tempfile::tempdir()?;
    // Source metadata has this conventional coordinate too: coincident paths must not create native authority.
    let leaf_root = root.path().join("target/lib");
    let selected = materialized_entry(&leaf_root, "selected_leaf", nominal_manifest("leaf")?)?;
    let child = plan(vec![("selected_leaf", selected)])?;
    let leaf = origin(&child, "leaf")?;
    let parent_entry = materialized_entry(&root.path().join("parent-owner"), "direct", nominal_manifest("leaf")?)?;
    let parent_owner = plan(vec![("direct", parent_entry.clone())])?;
    assert_eq!(leaf, origin(&parent_owner, "leaf")?);
    let mut consumer = plan(vec![
        (
            "middle",
            source_entry(root.path(), "middle", nominal_manifest("middle")?),
        ),
        ("direct", parent_entry),
    ])?;
    assert!(consumer.public_nominal_metadata_projection("middle", &leaf).is_err());
    consumer.retain_checked_source_dependencies("middle", &child)?;
    let (semantic, _, route) = consumer.public_nominal_metadata_projection("middle", &leaf)?;
    assert_eq!(route, ["selected_leaf"]);
    assert!(!semantic.materialized);
    let child_owner = child
        .public_artifacts()
        .find(|artifact| artifact.identity == leaf.provider)
        .ok_or("child owner missing")?;
    let direct_owner = consumer
        .public_artifacts()
        .find(|artifact| artifact.identity == leaf.provider)
        .ok_or("direct owner missing")?;
    assert!(Arc::ptr_eq(&semantic.manifest, &child_owner.manifest));
    assert!(!Arc::ptr_eq(&semantic.manifest, &direct_owner.manifest));

    assert!(consumer.public_nominal_projection("middle", &leaf).is_err());
    let (physical, _, route) = consumer.public_nominal_metadata_projection("direct", &leaf)?;
    assert!(physical.materialized);
    assert!(route.is_empty());
    assert!(consumer.public_nominal_projection("direct", &leaf).is_ok());
    assert_eq!(consumer.public_artifacts().count(), 1);
    // Coincident source/physical coordinates are independently refused by the native traversal.
    let mut coincident = plan(vec![
        (
            "middle",
            source_entry(root.path(), "middle", nominal_manifest("middle")?),
        ),
        ("direct", load_provider_dependency_artifact("direct", &leaf_root)),
    ])?;
    coincident.retain_checked_source_dependencies("middle", &child)?;
    assert!(coincident.public_nominal_projection("middle", &leaf).is_err());
    assert!(coincident.public_nominal_projection("direct", &leaf).is_ok());

    Ok(())
}

/// Existing physical public edges remain usable through a semantic source prefix, without adding physical owners.
#[test]
fn dev7_mixed_nominal_routes_materialized_public_chain() -> TestResult {
    let root = tempfile::tempdir()?;
    let leaf_root = root.path().join("leaf");
    let leaf_entry = materialized_entry(&leaf_root, "leaf", nominal_manifest("leaf")?)?;
    let leaf_plan = plan(vec![("leaf", leaf_entry)])?;
    let leaf = origin(&leaf_plan, "leaf")?;
    let mut bridge = nominal_manifest("bridge")?;
    bridge
        .contract_metadata
        .provider
        .provider_dependencies
        .push(ProviderDependencyMetadata {
            kind: ProviderDependencyKind::PublicPackage,
            dependency_key: "inner_leaf".to_string(),
            provider_name: "leaf".to_string(),
            provider_version: "1.0.0".to_string(),
            artifact_digest: digest_provider_artifact(&leaf_root)?,
            relative_artifact_path: "../leaf".to_string(),
            requested_features: BTreeSet::new(),
            default_features: false,
            optional: false,
        });
    let child = plan(vec![(
        "installed_alias",
        materialized_entry(&root.path().join("bridge"), "installed_alias", bridge)?,
    )])?;
    assert_eq!(
        child.public_nominal_metadata_projection("installed_alias", &leaf)?.2,
        ["inner_leaf"]
    );
    assert!(child.public_nominal_projection("installed_alias", &leaf).is_ok());
    let mut consumer = plan(vec![(
        "middle",
        source_entry(root.path(), "middle", nominal_manifest("middle")?),
    )])?;
    consumer.retain_checked_source_dependencies("middle", &child)?;
    assert_eq!(
        consumer.public_nominal_metadata_projection("middle", &leaf)?.2,
        ["installed_alias", "inner_leaf"]
    );
    assert_eq!(
        consumer.declaring_public_provider("middle", &leaf.canonical.hydrate().ok_or("invalid canonical")?)?,
        leaf.provider
    );
    assert_eq!(consumer.public_artifacts().count(), 0);
    assert!(consumer.public_nominal_projection("middle", &leaf).is_err());
    Ok(())
}

/// Source-only chains and exact dual materialized aliases preserve their independently declared routes.
#[test]
fn dev7_mixed_nominal_routes_source_chain_and_dual_alias() -> TestResult {
    let root = tempfile::tempdir()?;
    let mut middle = plan(vec![(
        "inner",
        source_entry(root.path(), "inner", nominal_manifest("inner")?),
    )])?;
    let child = plan(vec![(
        "leaf",
        source_entry(root.path(), "leaf", nominal_manifest("leaf")?),
    )])?;
    let leaf = origin(&child, "leaf")?;
    middle.retain_checked_source_dependencies("inner", &child)?;
    let mut consumer = plan(vec![(
        "middle",
        source_entry(root.path(), "middle", nominal_manifest("middle")?),
    )])?;
    consumer.retain_checked_source_dependencies("middle", &middle)?;
    assert_eq!(
        consumer.public_nominal_metadata_projection("middle", &leaf)?.2,
        ["inner", "leaf"]
    );
    let first = materialized_entry(&root.path().join("owned"), "first", nominal_manifest("owned")?)?;
    let second = load_provider_dependency_artifact("second", &root.path().join("owned"));
    let aliases = plan(vec![("first", first), ("second", second)])?;
    let owned = origin(&aliases, "owned")?;
    consumer.retain_checked_source_dependencies("middle", &aliases)?;
    assert_eq!(
        consumer.public_nominal_metadata_projection("middle", &owned)?.2,
        ["first"]
    );
    let edges = consumer
        .checked_source_dependencies
        .get(&consumer.checked_source_import_identity("middle")?.stable_key())
        .ok_or("missing retained edges")?;
    assert_eq!(
        edges.iter().map(|(alias, _)| alias.as_str()).collect::<Vec<_>>(),
        ["first", "second"]
    );
    Ok(())
}

/// Merely retaining an unrelated sibling's selected metadata cannot authorize a missing directed dependency edge.
#[test]
fn dev7_mixed_nominal_routes_refuse_sibling_and_missing_edge() -> TestResult {
    let root = tempfile::tempdir()?;
    let child = plan(vec![(
        "leaf",
        materialized_entry(&root.path().join("leaf"), "leaf", nominal_manifest("leaf")?)?,
    )])?;
    let leaf = origin(&child, "leaf")?;
    let mut consumer = plan(vec![
        (
            "middle",
            source_entry(root.path(), "middle", nominal_manifest("middle")?),
        ),
        (
            "sibling",
            source_entry(root.path(), "sibling", nominal_manifest("sibling")?),
        ),
    ])?;
    consumer.retain_checked_source_dependencies("sibling", &child)?;
    assert!(consumer.public_nominal_metadata(&leaf).is_ok());
    assert!(consumer.public_nominal_metadata_projection("middle", &leaf).is_err());
    assert!(
        consumer
            .declaring_public_provider("middle", &leaf.canonical.hydrate().ok_or("invalid canonical")?)
            .is_err()
    );
    consumer.retain_checked_source_dependencies("middle", &child)?;
    assert!(consumer.public_nominal_metadata_projection("middle", &leaf).is_ok());
    consumer.checked_source_dependencies.clear();
    assert!(consumer.public_nominal_metadata_projection("middle", &leaf).is_err());
    Ok(())
}

/// Exact selected owners and canonical declaration identities refuse substituted generations and private members.
#[test]
fn dev7_mixed_nominal_routes_refuse_wrong_owner_and_declaration() -> TestResult {
    let root = tempfile::tempdir()?;
    let child = plan(vec![(
        "leaf",
        materialized_entry(&root.path().join("leaf"), "leaf", nominal_manifest("leaf")?)?,
    )])?;
    let leaf = origin(&child, "leaf")?;
    let mut consumer = plan(vec![(
        "middle",
        source_entry(root.path(), "middle", nominal_manifest("middle")?),
    )])?;
    consumer.retain_checked_source_dependencies("middle", &child)?;
    let mut forged = leaf.clone();
    forged.provider.digest = format!("sha256:{}", "c".repeat(64));
    assert!(consumer.public_nominal_metadata_projection("middle", &forged).is_err());
    let mut forged = leaf.clone();
    forged.provider.feature_projection.insert("not-selected".to_string());
    assert!(consumer.public_nominal_metadata_projection("middle", &forged).is_err());
    forged = leaf.clone();
    forged.canonical.declaration_span.start += 1;
    assert!(consumer.public_nominal_metadata_projection("middle", &forged).is_err());
    forged = leaf.clone();
    forged.canonical.declaration_name = "PrivateWidget".to_string();
    assert!(consumer.public_nominal_metadata_projection("middle", &forged).is_err());
    Ok(())
}

/// An index alias, coordinate or manifest substituted after plan construction cannot manufacture a semantic edge.
#[test]
fn dev7_mixed_nominal_routes_refuse_alias_and_coordinate_substitution() -> TestResult {
    let root = tempfile::tempdir()?;
    let mut child = plan(vec![(
        "leaf",
        materialized_entry(&root.path().join("leaf"), "leaf", nominal_manifest("leaf")?)?,
    )])?;
    let original = child
        .library_manifest_index
        .get("leaf")
        .ok_or("missing leaf entry")?
        .clone();
    let mut consumer = plan(vec![(
        "middle",
        source_entry(root.path(), "middle", nominal_manifest("middle")?),
    )])?;
    let mut aliased = original.clone();
    if let LibraryManifestIndexEntry::Loaded { metadata, .. } = &mut aliased {
        metadata.dependency_key = "forged".to_string();
    }
    child.library_manifest_index = LibraryManifestIndex::from_entries(HashMap::from([("forged".to_string(), aliased)]));
    assert!(consumer.retain_checked_source_dependencies("middle", &child).is_err());
    let mut moved = original.clone();
    if let LibraryManifestIndexEntry::Loaded { metadata, .. } = &mut moved {
        metadata.crate_root = root.path().join("other-owner");
    }
    child.library_manifest_index = LibraryManifestIndex::from_entries(HashMap::from([("leaf".to_string(), moved)]));
    assert!(consumer.retain_checked_source_dependencies("middle", &child).is_err());
    let mut changed = original.clone();
    if let LibraryManifestIndexEntry::Loaded { manifest, .. } = &mut changed {
        manifest.contract_metadata.identity_graph.exports.clear();
    }
    child.library_manifest_index = LibraryManifestIndex::from_entries(HashMap::from([("leaf".to_string(), changed)]));
    assert!(consumer.retain_checked_source_dependencies("middle", &child).is_err());
    assert!(consumer.checked_source_materialized.is_empty());
    child.library_manifest_index = LibraryManifestIndex::from_entries(HashMap::from([("leaf".to_string(), original)]));
    consumer.retain_checked_source_dependencies("middle", &child)?;
    assert_eq!(consumer.checked_source_materialized.len(), 1);
    Ok(())
}

/// Equal content at another physical owner cannot overwrite the original retained semantic owner coordinate.
#[test]
fn dev7_mixed_nominal_routes_refuse_equal_bytes_other_coordinate() -> TestResult {
    let root = tempfile::tempdir()?;
    let original = plan(vec![(
        "leaf",
        materialized_entry(&root.path().join("original"), "leaf", nominal_manifest("leaf")?)?,
    )])?;
    let substitute = plan(vec![(
        "leaf",
        materialized_entry(&root.path().join("substitute"), "leaf", nominal_manifest("leaf")?)?,
    )])?;
    let selected = origin(&original, "leaf")?;
    assert_eq!(selected, origin(&substitute, "leaf")?);
    let mut consumer = plan(vec![(
        "middle",
        source_entry(root.path(), "middle", nominal_manifest("middle")?),
    )])?;
    consumer.retain_checked_source_dependencies("middle", &original)?;
    let retained = consumer
        .checked_source_materialized
        .get(&selected.provider.stable_key())
        .ok_or("original retained owner missing")?
        .artifact
        .clone();
    assert!(
        consumer
            .retain_checked_source_dependencies("middle", &substitute)
            .is_err()
    );
    assert_eq!(
        consumer
            .checked_source_materialized
            .get(&selected.provider.stable_key())
            .ok_or("original owner lost")?
            .artifact,
        retained
    );
    assert_eq!(
        consumer.public_nominal_metadata_projection("middle", &selected)?.2,
        ["leaf"]
    );
    Ok(())
}
