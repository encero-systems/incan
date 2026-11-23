//! A Loaf's public namespaces, read by name from the installed set (RFC 179).

use super::typechecker_reads::catalog;
use super::{installed_library, read_declaration_in, read_summary};

/// After inventory admission, installed and path Loaves supply the same namespace record without the full manifest.
#[test]
fn a_consumer_reads_one_public_namespace_record_alone() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let mut manifest = catalog()?;
    let set = installed_library(root.path(), &manifest)?;
    let dir = root.path().join("debug");
    read_summary(&set, &dir, "prices")?.ok_or("the Loaf is installed")?;
    let admitted = set.reads();
    let (namespaces, _) = read_declaration_in(&set, &dir, "prices", "public-namespace", "money")?
        .ok_or("the namespace record is installed")?;
    assert_eq!(namespaces.len(), 1, "one namespace at the path `money`");
    assert_eq!(namespaces[0]["path"], serde_json::json!(["money"]));
    let records = set.reads().records - admitted.records;
    assert_eq!(
        records, 1,
        "the consumer read {records} records; it needs the namespace's record alone"
    );

    let directory = root.path().join("path-dependency");
    std::fs::create_dir(&directory)?;
    let private = tempfile::tempdir()?;
    let _home = super::super::super::manifest_input::select_test_home(private.path());
    let _records = super::thread_selection::select(super::RecordSet::new(
        root.path().join("unrelated-installation"),
        Vec::new(),
    ));
    let declared = crate::typechecker::TypeChecker::new().loaf_install_record("prices", &manifest);
    let install = manifest.prepare_install_record(&declared)?;
    let content = manifest.to_json_string()?;
    std::fs::write(directory.join("prices.incnlib"), &content)?;
    std::fs::write(
        directory.join("prices.incnfacts"),
        manifest.encode_loaf_facts(&content, Some(install))?,
    )?;
    let open = || {
        super::super::super::lazy_entry::IndexedLoaf::open(
            "prices".into(),
            crate::loaf_index::LoafNamespace::Dependency { key: "prices".into() },
            directory.clone(),
            "prices",
        )
    };
    let cold = open()?;
    assert!(cold.was_demanded(), "the first admission establishes the inventory");
    let warm = open()?;
    let selected = warm.public_namespace(&["money".into()])?;
    assert_eq!(selected.as_ref(), namespaces.first());
    assert!(
        !warm.was_demanded(),
        "namespace selection reread the whole path manifest"
    );
    Ok(())
}
