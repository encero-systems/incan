//! Generate the checked standard-library capability inventory reference.

use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let output = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("workspaces/docs-site/docs/language/reference/feature_inventory.md");
    incan::cli::commands::tools::write_feature_inventory_reference(&output)?;
    Ok(())
}
