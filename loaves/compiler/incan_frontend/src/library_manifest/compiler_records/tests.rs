//! The two digests of a Loaf's installed compiler records (RFC 179).

use super::{FactDigests, RecordFamily};
use crate::api_metadata::{
    CHECKED_API_METADATA_SCHEMA_VERSION, CheckedApiMetadataPackage, collect_checked_api_metadata,
    materialize_checked_api_public_namespaces,
};
use crate::library_exports::collect_checked_public_exports;
use crate::library_manifest::LibraryManifest;
use crate::{lexer, parser, typechecker};

/// The manifest a library publishing `source` as version `version` would carry, with its checked API.
fn manifest(source: &str, version: &str) -> Result<LibraryManifest, String> {
    let tokens = lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = typechecker::TypeChecker::new();
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let exports = collect_checked_public_exports(&program, &checker);
    let mut manifest = LibraryManifest::from_checked_exports("prices", version, &exports);
    manifest.contract_metadata.api = Some(CheckedApiMetadataPackage {
        schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
        package: None,
        modules: vec![collect_checked_api_metadata(
            &program,
            &checker,
            vec!["prices".to_owned()],
        )],
        public_namespaces: Vec::new(),
    });
    Ok(manifest)
}

/// The digests of the records of `source` published as `version`; `manifest_digest` stands for its file's digest.
fn digests(source: &str, version: &str, manifest_digest: &str) -> Result<FactDigests, String> {
    let set = manifest(source, version)?
        .compiler_records(manifest_digest)
        .map_err(|error| error.to_string())?;
    Ok(set.digests)
}

const PRICE: &str = "pub def price() -> int:\n    \"\"\"The list price.\"\"\"\n    return 1999\n";
// The same length as `PRICE`'s docstring: declaration identities carry source spans, so a longer docstring moves them.
const PRICE_REDOCUMENTED: &str = "pub def price() -> int:\n    \"\"\"The sale price.\"\"\"\n    return 1999\n";
const PRICE_AS_TEXT: &str = "pub def price() -> str:\n    \"\"\"The list price.\"\"\"\n    return \"1999\"\n";
const FIRST_FILE: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const SECOND_FILE: &str = "2222222222222222222222222222222222222222222222222222222222222222";

#[test]
fn a_documentation_only_change_keeps_the_semantic_facts_digest() -> Result<(), String> {
    let before = digests(PRICE, "0.1.0", FIRST_FILE)?;
    let after = digests(PRICE_REDOCUMENTED, "0.1.0", SECOND_FILE)?;
    assert_eq!(
        before.semantic_facts, after.semantic_facts,
        "a docstring edit changes no compile fact"
    );
    assert_ne!(
        before.record_content, after.record_content,
        "the records' content did change"
    );
    Ok(())
}

#[test]
fn republishing_under_another_version_keeps_the_semantic_facts_digest() -> Result<(), String> {
    let before = digests(PRICE, "0.1.0", FIRST_FILE)?;
    let after = digests(PRICE, "0.2.0", SECOND_FILE)?;
    assert_eq!(
        before.semantic_facts, after.semantic_facts,
        "a version is a publication field"
    );
    assert_ne!(
        before.record_content, after.record_content,
        "the metadata record's content did change"
    );
    Ok(())
}

#[test]
fn a_signature_change_changes_both_digests() -> Result<(), String> {
    let before = digests(PRICE, "0.1.0", FIRST_FILE)?;
    let after = digests(PRICE_AS_TEXT, "0.1.0", FIRST_FILE)?;
    assert_ne!(
        before.semantic_facts, after.semantic_facts,
        "a return type is a compile fact"
    );
    assert_ne!(before.record_content, after.record_content);
    Ok(())
}

#[test]
fn equal_manifests_give_equal_records_and_digests() -> Result<(), String> {
    let first = manifest(PRICE, "0.1.0")?
        .compiler_records(FIRST_FILE)
        .map_err(|error| error.to_string())?;
    let second = manifest(PRICE, "0.1.0")?
        .compiler_records(FIRST_FILE)
        .map_err(|error| error.to_string())?;
    assert_eq!(first.records, second.records);
    assert_eq!(first.digests, second.digests);
    let families = first.records.iter().map(|record| record.family).collect::<Vec<_>>();
    assert_eq!(
        families,
        [
            RecordFamily::Metadata,
            RecordFamily::Function,
            RecordFamily::ContractMetadata,
            RecordFamily::DeclarationSignature,
            RecordFamily::TypeScope,
            RecordFamily::Documentation,
            RecordFamily::PublicIdentities,
            RecordFamily::RuntimeBindings
        ],
        "one metadata record, one record per declaration, the contract facts, the declaration's signature, its module's type scope, their documentation, the public identities and the runtime bindings"
    );
    Ok(())
}

const PRICES: &str = "pub def price() -> int:\n    \"\"\"The list price.\"\"\"\n    return 1999\n\npub def discount() -> int:\n    return 10\n";
// A longer docstring moves every later declaration's source span.
const PRICES_LONGER_DOCUMENTATION: &str = "pub def price() -> int:\n    \"\"\"The list price before any discount.\"\"\"\n    return 1999\n\npub def discount() -> int:\n    return 10\n";

#[test]
fn a_longer_docstring_keeps_the_semantic_facts_digest() -> Result<(), String> {
    let before = digests(PRICES, "0.1.0", FIRST_FILE)?;
    let after = digests(PRICES_LONGER_DOCUMENTATION, "0.1.0", SECOND_FILE)?;
    assert_eq!(
        before.semantic_facts, after.semantic_facts,
        "source positions are not compile facts"
    );
    assert_ne!(before.record_content, after.record_content);
    Ok(())
}

#[test]
fn a_decorator_anchor_is_a_source_position() {
    // Decorator metadata names its source location `anchor`, a span without the `span` name; Oven's own
    // `incan_platform` carries five of them, and a longer docstring above one moved it.
    let mut value = serde_json::json!({
        "name": "traced",
        "decorators": [{"path": ["probe"], "anchor": {"start": 10, "end": 16, "expansion": null}}],
        "anchor": {"id": "probe::traced", "span": {"start": 0, "end": 40, "expansion": null}},
    });
    super::without_positions(&mut value);
    assert_eq!(
        value,
        serde_json::json!({
            "name": "traced",
            "decorators": [{"path": ["probe"]}],
            "anchor": {"id": "probe::traced"},
        }),
        "every source position is left out, whatever field holds it"
    );
}

/// The digests of `PRICE`'s records when its governed bodies sidecar has `content_digest` under `version`.
fn digests_with_bodies(version: u32, content_digest: &str) -> Result<FactDigests, String> {
    let mut manifest = manifest(PRICE, "0.1.0")?;
    manifest.contract_metadata.executable_representation =
        Some(crate::library_manifest::ExecutableRepresentationExport {
            representation_version: version,
            content_digest: content_digest.to_owned(),
        });
    Ok(manifest
        .compiler_records(FIRST_FILE)
        .map_err(|error| error.to_string())?
        .digests)
}

#[test]
fn moved_governed_bodies_keep_the_semantic_facts_digest() -> Result<(), String> {
    // A longer docstring in Oven's `incan_platform` moved the spans its Body IR carries, so its `.incnbody` bytes, and
    // the descriptor's digest of them, changed while every other compile fact stayed.
    let before = digests_with_bodies(55, FIRST_FILE)?;
    let after = digests_with_bodies(55, SECOND_FILE)?;
    assert_eq!(
        before.semantic_facts, after.semantic_facts,
        "the bodies' byte digest is not a compile fact"
    );
    assert_ne!(
        before.record_content, after.record_content,
        "the records name the bodies' bytes"
    );
    let other_version = digests_with_bodies(56, FIRST_FILE)?;
    assert_ne!(
        before.semantic_facts, other_version.semantic_facts,
        "the bodies' representation version is a compile fact"
    );
    Ok(())
}

const PRICED: &str = "pub model Price:\n    \"\"\"A price.\"\"\"\n    pub cents: int = 0\n\npub def total(price: Price, count: int = 1) -> int:\n    \"\"\"The total.\"\"\"\n    return price.cents * count\n";
// A longer docstring on each declaration moves every later span.
const PRICED_LONGER_DOCUMENTATION: &str = "pub model Price:\n    \"\"\"A price, in cents.\"\"\"\n    pub cents: int = 0\n\npub def total(price: Price, count: int = 1) -> int:\n    \"\"\"The total of `count` prices.\"\"\"\n    return price.cents * count\n";

/// The records of `source` whose family is spelled `family` and whose name is `name`.
fn named_records(source: &str, family: &str, name: &str) -> Result<Vec<super::CompilerRecord>, String> {
    let set = manifest(source, "0.1.0")?
        .compiler_records(FIRST_FILE)
        .map_err(|error| error.to_string())?;
    Ok(set
        .records
        .into_iter()
        .filter(|record| record.family.as_str() == family && record.name == name)
        .collect())
}

#[test]
fn signatures_layouts_and_defaults_are_their_own_records() -> Result<(), String> {
    for (family, name) in [
        ("declaration-signature", "prices::total"),
        ("declaration-signature", "prices::Price"),
        ("layout", "prices::Price"),
        ("default", "prices::total"),
        ("default", "prices::Price"),
    ] {
        let before = named_records(PRICED, family, name)?;
        assert_eq!(before.len(), 1, "one `{family}` record for `{name}`");
        let after = named_records(PRICED_LONGER_DOCUMENTATION, family, name)?;
        assert_eq!(after.len(), 1, "one `{family}` record for `{name}` after the edit");
        let canonical = |record: &super::CompilerRecord| super::canonical_value(record.family, &record.value);
        assert_eq!(
            canonical(&before[0]).map_err(|error| error.to_string())?,
            canonical(&after[0]).map_err(|error| error.to_string())?,
            "the `{family}` record of `{name}` is digested without documentation or positions"
        );
    }
    let signature = named_records(PRICED, "declaration-signature", "prices::total")?;
    let value: serde_json::Value =
        LibraryManifest::decode_compiler_record_value(&signature[0].value).map_err(|error| error.to_string())?;
    let text = value.to_string();
    assert!(
        !text.contains("has_default") && !text.contains("docstring"),
        "a signature carries neither defaults nor documentation: {text}"
    );
    Ok(())
}

/// The manifest of a Loaf `prices` whose one module, `money`, publishes `source`, with its public namespaces and
/// declaration identities materialized as an installed Loaf carries them.
fn namespaced(source: &str) -> Result<LibraryManifest, String> {
    let tokens = lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let module = vec!["money".to_owned()];
    let mut checker = typechecker::TypeChecker::new();
    checker.set_current_package_identity(Some("prices".into()));
    checker.set_current_module_path(Some(module.clone()));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let exports = collect_checked_public_exports(&program, &checker);
    let mut api = CheckedApiMetadataPackage {
        schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
        package: None,
        modules: vec![collect_checked_api_metadata(&program, &checker, module.clone())],
        public_namespaces: Vec::new(),
    };
    materialize_checked_api_public_namespaces(&mut api).map_err(|error| error.to_string())?;
    let mut manifest = LibraryManifest::new("prices", "0.1.0");
    manifest
        .contract_metadata
        .identity_graph
        .extend_checked_api_exports("prices", &api, &[(module, exports)])
        .map_err(|error| error.to_string())?;
    manifest.contract_metadata.api = Some(api);
    Ok(manifest)
}

#[test]
fn each_public_namespace_is_a_record_of_its_own() -> Result<(), String> {
    let records = |source: &str| -> Result<Vec<super::CompilerRecord>, String> {
        Ok(namespaced(source)?
            .compiler_records(FIRST_FILE)
            .map_err(|error| error.to_string())?
            .records
            .into_iter()
            .filter(|record| record.family.as_str() == "public-namespace")
            .collect())
    };
    let before = records(PRICED)?;
    assert_eq!(
        before.iter().map(|record| record.name.as_str()).collect::<Vec<_>>(),
        ["money"],
        "one `public-namespace` record, named by the namespace's path"
    );
    let value: serde_json::Value =
        LibraryManifest::decode_compiler_record_value(&before[0].value).map_err(|error| error.to_string())?;
    let mut members = value["members"]
        .as_array()
        .ok_or("the record lists the namespace's members")?
        .iter()
        .map(|member| member["name"].as_str().unwrap_or_default())
        .collect::<Vec<_>>();
    members.sort_unstable();
    assert_eq!(members, ["Price", "total"], "{value}");
    let identities = value["identities"]
        .as_array()
        .ok_or("the record carries the identities of its public paths")?;
    assert!(
        identities
            .iter()
            .any(|identity| identity["public_path"] == serde_json::json!(["prices", "money", "total"])),
        "the identity of `prices.money.total`: {value}"
    );
    // Positions and documentation stay out of its digest, as for every other record.
    let after = records(PRICED_LONGER_DOCUMENTATION)?;
    let canonical = |record: &super::CompilerRecord| super::canonical_value(record.family, &record.value);
    assert_eq!(
        canonical(&before[0]).map_err(|error| error.to_string())?,
        canonical(&after[0]).map_err(|error| error.to_string())?,
        "a namespace's record is digested without documentation or positions"
    );
    Ok(())
}
