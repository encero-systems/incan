//! An exact input roster authenticates only the contributors the parser and package resolver must discover.
use super::{LibraryManifest, RecordEntry, RecordSet, content_name, thread_selection};
use crate::library_manifest::capabilities::{self, CapabilityCatalog};
use crate::loaf_index::LoafIndex;
use std::collections::BTreeSet;
use std::fs;

#[test]
fn complete_and_partial_package_rosters_read_only_their_own_inventory() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let payloads = root.path().join("payloads");
    fs::create_dir(&payloads)?;
    let profile = "release";
    let dir = root.path().join(profile);
    let names = ["incan_stdlib_primitives", "incan_stdlib_web"];
    let location = |name: &str| (name.to_owned(), name.to_owned(), dir.clone(), true);
    let mut catalog = CapabilityCatalog::default();
    let mut entries = Vec::new();
    for name in names {
        let manifest = LibraryManifest::new(name, "0.1.0");
        let record = manifest
            .compiler_records(&content_name(name.as_bytes()))?
            .records
            .remove(0);
        catalog.insert(name, &record.value)?;
        let digest = content_name(&record.value);
        fs::write(payloads.join(&digest), &record.value)?;
        entries.push(RecordEntry {
            key: format!("loaf/{profile}/{name}/{}", record.key()),
            revision: digest.clone(),
            value: digest,
        });
    }
    let key = catalog.key(profile);
    let value = catalog.encode(profile)?;
    let digest = content_name(&value);
    fs::write(payloads.join(&digest), &value)?;
    entries.push(RecordEntry {
        key: key.clone(),
        revision: digest.clone(),
        value: digest.clone(),
    });
    {
        let _selected = thread_selection::select(RecordSet::new(root.path().into(), entries.clone()));
        let index = LoafIndex::deferred(names.map(location))?;
        assert_eq!(index.identities().count(), 2);
        assert_eq!(thread_selection::read_keys(), [key.clone()]);
        let _second = LoafIndex::deferred(names.map(location))?;
        assert_eq!(thread_selection::read_keys(), [key.clone()]);
    }
    {
        let _selected = thread_selection::select(RecordSet::new(root.path().into(), entries.clone()));
        let _subset = LoafIndex::deferred([location(names[0])])?;
        assert_eq!(thread_selection::read_keys(), [entries[0].key.clone()]);
    }
    // A new content-named record placed under a different roster must still pass embedded membership verification.
    let subset_key = capabilities::key(profile, &BTreeSet::from([names[0].to_owned()]));
    entries.push(RecordEntry {
        key: subset_key,
        revision: digest.clone(),
        value: digest,
    });
    let _selected = thread_selection::select(RecordSet::new(root.path().into(), entries));
    assert!(LoafIndex::deferred([location(names[0])]).is_err());
    Ok(())
}
