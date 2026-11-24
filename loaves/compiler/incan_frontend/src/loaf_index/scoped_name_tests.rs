use super::{LoafArtifact, LoafIdentity, LoafIndex, LoafNamespace};
use crate::library_manifest::LibraryManifest;
use crate::loaf_index::lazy_entry::IndexedLoaf;
use incan_loaf_names::artifact_stem;
use std::collections::BTreeSet;

/// The ordinary retained reader opens encoded filenames while retaining the exact scoped semantic owner.
#[test]
fn scoped_artifacts_retain_their_producer_name() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    for name in ["alpha/catalog", "beta/catalog", "scope-a/catalog", "scope/a-catalog"] {
        let manifest = LibraryManifest::new(name, "1.0.0");
        let stem = artifact_stem(name);
        let path = directory.path().join(format!("{stem}.incnlib"));
        std::fs::write(&path, manifest.to_json_string()?)?;
        for _ in 0..2 {
            let artifact = LoafArtifact::read(directory.path(), name)?;
            assert_eq!(artifact.manifest.name, name);
            assert_eq!(artifact.identity.name, name);
            assert_eq!(artifact.manifest_path(), path);
            assert_eq!(
                artifact.bodies_path(),
                directory.path().join(format!("{stem}.incnbody"))
            );
            assert_eq!(
                artifact.native_closure_path(),
                directory.path().join(format!("{stem}.native.json"))
            );
        }
    }
    Ok(())
}

/// Header-only consumers observe admitted identity records without demanding declaration manifests.
#[test]
fn admitted_identities_keep_manifests_lazy() -> Result<(), Box<dyn std::error::Error>> {
    let key = "catalog";
    let identity = LoafIdentity {
        name: "alpha/catalog".to_owned(),
        version: "1.0.0".to_owned(),
        digest: "admitted-digest".to_owned(),
        feature_projection: BTreeSet::new(),
    };
    let indexed = IndexedLoaf::summary_only(
        key.to_owned(),
        LoafNamespace::Dependency { key: key.to_owned() },
        identity.clone(),
    );
    let observation = indexed.clone();
    let mut index = LoafIndex::default();
    index.entries.insert(key.to_owned(), indexed);

    let entries = index.admitted_identities().collect::<Vec<_>>();

    assert_eq!(entries.len(), 1);
    let (observed_key, observed_identity, observed_namespace) = entries
        .first()
        .copied()
        .ok_or("the admitted identity projection is empty")?;
    assert_eq!(observed_key, key);
    assert_eq!(observed_identity, &identity);
    assert_eq!(observed_namespace, &LoafNamespace::Dependency { key: key.to_owned() });
    assert!(!observation.was_demanded());
    Ok(())
}

/// A request's decoded Loaf manifests are released when it ends: a resident worker keeps no owned copy of its
/// dependency closure between requests, and the next request reads the Loaf again.
#[test]
fn released_loaf_readings_do_not_outlive_their_request() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let name = "released/catalog";
    let manifest = LibraryManifest::new(name, "1.0.0");
    let path = directory.path().join(format!("{}.incnlib", artifact_stem(name)));
    std::fs::write(&path, manifest.to_json_string()?)?;
    LoafArtifact::read(directory.path(), name)?;
    assert!(super::retained::retains_artifact(directory.path(), name));
    super::release_retained_loaves();
    assert!(!super::retained::retains_artifact(directory.path(), name));
    assert_eq!(LoafArtifact::read(directory.path(), name)?.manifest.name, name);
    Ok(())
}
