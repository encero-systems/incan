use std::fs;

use incan_driver::build_report::BUILD_REPORT_SCHEMA_VERSION;
use incan_test_support as support;

use incan_test_support::cli_project;

use cli_project::*;

/// Select checked references to `value` contained by one named declaration.
fn value_references_owned_by<'a>(records: &'a [serde_json::Value], owner_name: &str) -> Vec<&'a serde_json::Value> {
    records
        .iter()
        .filter(|record| {
            record["record"] == serde_json::json!("reference")
                && record["name"] == serde_json::json!("value")
                && record["owner_id"].as_str().is_some_and(|owner_id| {
                    records.iter().any(|owner| {
                        owner["record"] == serde_json::json!("declaration")
                            && owner["id"] == serde_json::json!(owner_id)
                            && owner["name"] == serde_json::json!(owner_name)
                    })
                })
        })
        .collect()
}
