use super::GraphBodyRecordPages;
use incan_frontend::checker_records::GraphBodyRecordSource;
use sha2::{Digest, Sha256};

#[test]
fn keyed_body_pages_do_not_scan_unrequested_owners() -> Result<(), Box<dyn std::error::Error>> {
    let root = std::env::temp_dir().join(format!("incan-graph-body-pages-{}", std::process::id()));
    if root.exists() {
        std::fs::remove_dir_all(&root)?;
    }
    std::fs::create_dir_all(&root)?;
    let unrelated = root.join(hex::encode(Sha256::digest(b"unrelated-owner")));
    std::fs::write(&unrelated, b"malformed unrelated page")?;
    let source = GraphBodyRecordPages {
        root: root.clone(),
        bakery: None,
        namespaces: Default::default(),
        reopened: Default::default(),
    };

    assert!(source.read("requested-owner")?.is_none());
    assert!(source.read("unrelated-owner").is_err());

    std::fs::remove_dir_all(root)?;
    Ok(())
}
