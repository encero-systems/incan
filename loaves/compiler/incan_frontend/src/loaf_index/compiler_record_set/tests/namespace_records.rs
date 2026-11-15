//! A Loaf's public namespaces, read by name from the installed set (RFC 179).

use super::typechecker_reads::catalog;
use super::{installed_library, read_declaration_in, read_summary};

/// After the index's summary, reading the namespace `money` by its path opens its one record and nothing else.
#[test]
fn a_consumer_reads_one_public_namespace_record_alone() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let set = installed_library(root.path(), &catalog()?)?;
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
    Ok(())
}
