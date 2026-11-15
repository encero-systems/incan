use super::{
    CODEGRAPH_SCHEMA_VERSION, CodegraphCanonicalSymbolId, CodegraphDiagnosticRecord,
    CodegraphDiagnosticRelatedDeclaration, CodegraphFileRecord, CodegraphHeaderRecord, CodegraphIdentitySpan,
    CodegraphLanguage, CodegraphMode, CodegraphNamespaceRecord, CodegraphProvenance, CodegraphRecord,
    CodegraphReferenceRecord, CodegraphSourceSpan, CodegraphSymbolOrigin, to_jsonl,
};

#[test]
fn jsonl_emits_header_then_facts() -> Result<(), Box<dyn std::error::Error>> {
    let records = vec![
        CodegraphRecord::Header(CodegraphHeaderRecord {
            schema_version: CODEGRAPH_SCHEMA_VERSION,
            compiler_version: "0.4.0-dev.5".to_string(),
            mode: CodegraphMode::Strict,
            root_path: "src/main.incn".to_string(),
            languages: vec![CodegraphLanguage::Incan],
            package: None,
            semantic_contexts: Vec::new(),
            degraded: false,
        }),
        CodegraphRecord::File(CodegraphFileRecord {
            id: "file:src/main.incn".to_string(),
            language: CodegraphLanguage::Incan,
            path: "src/main.incn".to_string(),
            size_bytes: 12,
            provenance: CodegraphProvenance::Source,
            degraded: false,
        }),
    ];

    let jsonl = to_jsonl(&records)?;
    let lines = jsonl.lines().collect::<Vec<_>>();

    assert_eq!(lines.len(), 2);
    assert!(lines[0].contains("\"record\":\"header\""));
    assert!(lines[0].contains(&format!("\"schema_version\":{CODEGRAPH_SCHEMA_VERSION}")));
    assert!(lines[1].contains("\"record\":\"file\""));
    Ok(())
}

#[test]
fn canonical_identity_is_structured_and_independent_of_reference_spelling() -> Result<(), Box<dyn std::error::Error>> {
    let identity = CodegraphCanonicalSymbolId {
        namespace: "ordinary_lexical".to_string(),
        origin: CodegraphSymbolOrigin::Module {
            path: vec!["provider".to_string()],
        },
        declaration_name: "helper".to_string(),
        declaration_kind: "function".to_string(),
        scope_discriminant: None,
        declaration_span: CodegraphIdentitySpan { start: 4, end: 31 },
    };
    let record = CodegraphRecord::Reference(CodegraphReferenceRecord {
        id: "reference:consumer:0".to_string(),
        language: CodegraphLanguage::Incan,
        module_id: "module:consumer".to_string(),
        owner_id: None,
        name: "renamed_helper".to_string(),
        kind: "identifier".to_string(),
        target_id: None,
        canonical_identity: Some(identity.clone()),
        canonical_owner: None,
        stable_identity: None,
        span: None,
        provenance: CodegraphProvenance::Checked,
        degraded: false,
    });

    let encoded = serde_json::to_value(&record)?;
    assert_eq!(encoded["name"], "renamed_helper");
    assert_eq!(encoded["canonical_identity"]["declaration_name"], "helper");
    assert_eq!(encoded["canonical_identity"]["origin"]["kind"], "module");
    assert_eq!(serde_json::from_value::<CodegraphRecord>(encoded)?, record);
    Ok(())
}

#[test]
fn reference_records_without_checked_owner_remain_readable() -> Result<(), Box<dyn std::error::Error>> {
    let value = serde_json::json!({
        "record": "reference", "id": "reference:main:1", "language": "incan", "module_id": "module:main",
        "owner_id": null, "name": "alias", "kind": "identifier", "target_id": null,
        "canonical_identity": null, "span": null, "provenance": "source", "degraded": false
    });
    let record: CodegraphRecord = serde_json::from_value(value)?;
    let CodegraphRecord::Reference(reference) = record else {
        return Err("expected reference".into());
    };
    assert_eq!(reference.canonical_owner, None);
    Ok(())
}

#[test]
fn diagnostic_records_without_origin_remain_readable() -> Result<(), Box<dyn std::error::Error>> {
    let record = CodegraphRecord::Diagnostic(CodegraphDiagnosticRecord {
        id: "diagnostic:0".to_string(),
        language: CodegraphLanguage::Incan,
        code: "INCAN-T0001".to_string(),
        severity: "error".to_string(),
        phase: "typecheck".to_string(),
        origin: "typechecker".to_string(),
        message: "type mismatch".to_string(),
        primary_span: CodegraphSourceSpan {
            file: "main.incn".to_string(),
            start: 0,
            end: 1,
            start_line: 1,
            start_column: 1,
            end_line: 1,
            end_column: 2,
        },
        notes: Vec::new(),
        hints: Vec::new(),
        expected: None,
        actual: None,
        related_spans: Vec::new(),
        related_declarations: vec![CodegraphDiagnosticRelatedDeclaration {
            identity: CodegraphCanonicalSymbolId {
                namespace: "ordinary_lexical".to_string(),
                origin: CodegraphSymbolOrigin::Package {
                    library: "provider".to_string(),
                    module_path: vec!["api".to_string()],
                },
                declaration_name: "expected_value".to_string(),
                declaration_kind: "function".to_string(),
                scope_discriminant: None,
                declaration_span: CodegraphIdentitySpan { start: 7, end: 42 },
            },
            label: "previous declaration".to_string(),
        }],
        explain: "incan explain INCAN-T0001".to_string(),
        provenance: CodegraphProvenance::Diagnostic,
        degraded: true,
    });
    let mut legacy = serde_json::to_value(record)?;
    legacy.as_object_mut().ok_or("expected record object")?.remove("origin");

    let parsed: CodegraphRecord = serde_json::from_value(legacy)?;
    let CodegraphRecord::Diagnostic(diagnostic) = parsed else {
        return Err("expected diagnostic record".into());
    };
    assert_eq!(diagnostic.origin, "unknown");
    assert_eq!(diagnostic.related_declarations.len(), 1);
    assert_eq!(
        diagnostic.related_declarations[0].identity.declaration_name,
        "expected_value"
    );
    Ok(())
}

/// The namespace record round-trips under its own `record` tag.
///
/// The JSONL export is a published contract, so a new variant has to be checked at the wire rather than only
/// through the producer that builds it: the tag is what a consumer matches on, and nothing else in the schema
/// would fail if it were wrong.
#[test]
fn namespace_records_round_trip_under_their_own_tag() -> Result<(), Box<dyn std::error::Error>> {
    let record = CodegraphRecord::Namespace(CodegraphNamespaceRecord {
        id: "namespace:std.encoding".to_string(),
        language: CodegraphLanguage::Incan,
        namespace_path: vec!["std".to_string(), "encoding".to_string()],
        name: "encoding".to_string(),
        provenance: CodegraphProvenance::Syntax,
        degraded: false,
    });
    let encoded = serde_json::to_value(&record)?;
    assert_eq!(encoded["record"], "namespace");
    assert_eq!(encoded["namespace_path"][1], "encoding");
    assert_eq!(serde_json::from_value::<CodegraphRecord>(encoded)?, record);
    Ok(())
}
