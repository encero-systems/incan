//! The `incan.metadata.model.emit` command: its payload returns Incan source for a project model bundle and its
//! argument parser accepts an object argument.

use super::{ContractModelCommandFormat, emit_contract_model_command_payload, parse_emit_contract_model_command_args};

fn write_project_bundle(root: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(root.join("contracts"))?;
    std::fs::write(
        root.join("loaf.toml"),
        r#"[project]
name = "lsp_contract_model"
version = "0.1.0"

[tool.incan.metadata]
model-bundles = ["contracts/order_summary.json"]
"#,
    )?;
    std::fs::write(
        root.join("contracts").join("order_summary.json"),
        r#"{
  "schema_version": 1,
  "stable_model_id": "orders.summary",
  "logical_type_name": "OrderSummary",
  "publishable": true,
  "fields": [
    {
      "name": "order_id",
      "type": "str",
      "alias": "orderId"
    }
  ]
}
"#,
    )?;
    Ok(())
}

#[test]
fn emit_contract_model_command_payload_returns_incan_source() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    write_project_bundle(tmp.path())?;

    let payload = emit_contract_model_command_payload(tmp.path(), "orders.summary", ContractModelCommandFormat::Incan)?;

    assert_eq!(
        payload.pointer("/format").and_then(serde_json::Value::as_str),
        Some("incan")
    );
    assert_eq!(
        payload.pointer("/model").and_then(serde_json::Value::as_str),
        Some("OrderSummary")
    );
    let source = payload
        .pointer("/source")
        .and_then(serde_json::Value::as_str)
        .ok_or("expected source payload")?;
    assert!(
        source.contains("pub model OrderSummary:"),
        "expected model source payload, got:\n{source}"
    );
    assert!(
        source.contains("order_id as \"orderId\": str") || source.contains("order_id [alias=\"orderId\"]: str"),
        "expected alias-preserving field source, got:\n{source}"
    );
    Ok(())
}

#[test]
fn parse_emit_contract_model_command_args_accepts_object_argument() -> Result<(), Box<dyn std::error::Error>> {
    let args = parse_emit_contract_model_command_args(vec![serde_json::json!({
        "uri": "file:///tmp/project/src/main.incn",
        "model": "OrderSummary",
        "format": "json"
    })])?;

    assert_eq!(args.uri.as_deref(), Some("file:///tmp/project/src/main.incn"));
    assert_eq!(args.model, "OrderSummary");
    assert_eq!(args.format.as_deref(), Some("json"));
    Ok(())
}
