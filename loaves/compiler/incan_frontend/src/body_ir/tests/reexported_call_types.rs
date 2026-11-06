//! Closed published function contracts through source facade rename links, without legacy resolver changes.

use super::{TypeChecker, ast, bir, body_named, build_body_ir_module_v0, lexer, parser};
use crate::library_exports::{checked_exports_by_name, collect_checked_public_exports};
use crate::library_manifest::LibraryManifest;
use crate::library_manifest_index::{LibraryArtifactMetadata, LibraryManifestIndex, LibraryManifestIndexEntry};
use crate::symbols::ResolvedType;
use crate::typechecker::TypeCheckInfo;
use std::collections::HashMap;

const ORIGINAL: &str = include_str!(
    "../../../../incan_test_support/fixtures/behavior/cli_dependencies/dependency_renamed_export_reexported_under_a_new_name/src/main.incn"
);
const FACADE: &str = include_str!(
    "../../../../incan_test_support/fixtures/behavior/cli_dependencies/dependency_renamed_export_reexported_under_a_new_name/src/facade.incn"
);
const PROVIDER: &str = include_str!(
    "../../../../incan_test_support/fixtures/behavior/cli_dependencies/dependency_renamed_export_reexported_under_a_new_name/deps/calc_lib/src/helpers.incn"
);
const PROVIDER_ROOT: &str = include_str!(
    "../../../../incan_test_support/fixtures/behavior/cli_dependencies/dependency_renamed_export_reexported_under_a_new_name/deps/calc_lib/src/lib.incn"
);

/// Check the real provider's renamed export, then the consumer and source facade against its manifest.
fn checked_facade(
    provider_source: &str,
    source: &str,
) -> Result<(TypeCheckInfo, bir::BodyIrModule), Box<dyn std::error::Error>> {
    checked_facade_with_identity(provider_source, source, true)
}

/// Optionally omit the published identity to exercise the refusal boundary without inventing a replacement.
fn checked_facade_with_identity(
    provider_source: &str,
    source: &str,
    retain_identity: bool,
) -> Result<(TypeCheckInfo, bir::BodyIrModule), Box<dyn std::error::Error>> {
    let provider = parse_program(provider_source)?;
    let entry = parse_program(PROVIDER_ROOT)?;
    let mut declaration_checker = TypeChecker::new();
    declaration_checker.set_current_package_identity(Some("calc_lib".to_owned()));
    declaration_checker.set_current_module_path(Some(vec!["helpers".to_owned()]));
    declaration_checker
        .check_program(&provider)
        .map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let declarations = collect_checked_public_exports(&provider, &declaration_checker);
    let mut provider_checker = TypeChecker::new();
    provider_checker.set_current_package_identity(Some("calc_lib".to_owned()));
    provider_checker.set_current_module_path(Some(Vec::new()));
    provider_checker.register_dependency_module_path_segments("helpers", vec!["helpers".to_owned()]);
    provider_checker
        .check_with_imports(&entry, &[("helpers", &provider)])
        .map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let entrypoint_exports = checked_exports_by_name(collect_checked_public_exports(&entry, &provider_checker));
    let exports = declarations
        .iter()
        .map(|declaration| declaration.projected_through_reexport("facade_calculate", Some(&entrypoint_exports)))
        .collect::<Vec<_>>();
    let mut manifest = LibraryManifest::from_checked_exports("calc_lib", "0.1.0", &exports);
    assert_eq!(manifest.exports.functions.len(), 1, "{exports:?}");
    assert!(
        manifest
            .contract_metadata
            .identity_graph
            .canonical_for_public_name("facade_calculate")
            .is_some(),
        "the fixture must publish a checked declaration identity"
    );
    if !retain_identity {
        manifest.contract_metadata.identity_graph = Default::default();
    }
    let index = LibraryManifestIndex::from_entries(HashMap::from([(
        "calc_lib".to_owned(),
        LibraryManifestIndexEntry::Loaded {
            manifest: Box::new(manifest),
            metadata: LibraryArtifactMetadata::from_crate_root(
                "calc_lib",
                "calc_lib",
                std::path::PathBuf::from("tests/renamed-calc-artifact"),
            ),
        },
    )]));
    let program = parse_program(source)?;
    let facade = parse_program(FACADE)?;
    let module_path = vec!["consumer".to_owned()];
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(module_path.clone()));
    checker.set_library_manifest_index(index);
    checker.register_dependency_module_path_segments("facade", vec!["facade".to_owned()]);
    checker
        .check_with_imports(&program, &[("facade", &facade)])
        .map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let module = build_body_ir_module_v0(&program, &module_path, checker.type_info());
    Ok((checker.type_info().clone(), module))
}

/// Parse source without hiding lexer or parser diagnostics behind a panic helper.
fn parse_program(source: &str) -> Result<ast::Program, Box<dyn std::error::Error>> {
    let tokens = lexer::lex(source).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    Ok(parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?)
}

/// The entire unchanged original closes the result of the twice-renamed published function call.
#[test]
fn complete_renamed_reexport_retains_closed_result() -> Result<(), Box<dyn std::error::Error>> {
    let (_, module) = checked_facade(PROVIDER, ORIGINAL)?;
    let body = body_named(&module, "main")?;
    assert!(
        !serde_json::to_string(&body.locals)?.contains("\"Unknown\""),
        "{}",
        body.render_snapshot()
    );
    let target = super::named_targets(&module, "main")
        .into_iter()
        .find(|target| target.name == "b_calculate")
        .ok_or("missing re-exported call")?;
    assert_eq!(
        target
            .canonical
            .as_ref()
            .map(|identity| identity.declaration_name.as_str()),
        Some("calculate")
    );
    Ok(())
}

/// Private closed results leave the frozen ordinary call snapshot and reference selection unchanged.
#[test]
fn renamed_reexport_preserves_legacy_snapshot() -> Result<(), Box<dyn std::error::Error>> {
    let (facts, _) = checked_facade(PROVIDER, ORIGINAL)?;
    let start = ORIGINAL.find("b_calculate(41)").ok_or("missing original call")?;
    let key = (start, start + "b_calculate(41)".len());
    assert_eq!(facts.expressions.expr_types.get(&key), Some(&ResolvedType::Unknown));
    assert_eq!(
        facts.calls.body_ir_call_result_types.get(&key),
        Some(&ResolvedType::Int)
    );
    assert!(
        facts
            .resolved_identity(ast::Span::new(start, start + "b_calculate".len()))
            .is_none()
    );
    Ok(())
}

/// Unvalidated argument counts and types cannot acquire a published function's result merely through its path.
#[test]
fn invalid_reexport_arguments_keep_result_unproven() -> Result<(), Box<dyn std::error::Error>> {
    for arguments in ["", "41, 42", "\"forty-one\""] {
        let source =
            format!("from facade import b_calculate\ndef main() -> None:\n    println(b_calculate({arguments}))\n");
        let (facts, module) = checked_facade(PROVIDER, &source)?;
        assert!(facts.calls.body_ir_call_result_types.is_empty());
        assert!(serde_json::to_string(&body_named(&module, "main")?.locals)?.contains("\"Unknown\""));
    }
    Ok(())
}

/// A published function with open type parameters does not receive this fixed scalar contract.
#[test]
fn generic_reexport_keeps_result_unproven() -> Result<(), Box<dyn std::error::Error>> {
    let (facts, module) = checked_facade("pub def calculate[T](value: T) -> T:\n    return value\n", ORIGINAL)?;
    assert!(facts.calls.body_ir_call_result_types.is_empty());
    assert!(serde_json::to_string(&body_named(&module, "main")?.locals)?.contains("\"Unknown\""));
    Ok(())
}

/// A parameter shadowing the imported facade binding retains its own callable contract.
#[test]
fn shadowed_reexport_keeps_local_callable_result() -> Result<(), Box<dyn std::error::Error>> {
    let source =
        "from facade import b_calculate\ndef main(b_calculate: (int) -> str) -> None:\n    println(b_calculate(41))\n";
    let (facts, module) = checked_facade(PROVIDER, source)?;
    assert!(facts.calls.body_ir_call_result_types.is_empty());
    let body = body_named(&module, "main")?;
    assert!(
        body.locals
            .iter()
            .any(|local| local.ty == super::IncanType::Primitive(super::IncanPrimitiveType::Str)),
        "{}",
        body.render_snapshot()
    );
    Ok(())
}

/// A published signature without its declaring identity cannot authorize execution under a guessed target.
#[test]
fn reexport_without_published_identity_keeps_result_unproven() -> Result<(), Box<dyn std::error::Error>> {
    let (facts, module) = checked_facade_with_identity(PROVIDER, ORIGINAL, false)?;
    assert!(facts.calls.body_ir_call_result_types.is_empty());
    assert!(serde_json::to_string(&body_named(&module, "main")?.locals)?.contains("\"Unknown\""));
    Ok(())
}

/// A contextual annotation retains the same private result while ordinary inference remains frozen.
#[test]
fn contextual_reexport_retains_closed_result() -> Result<(), Box<dyn std::error::Error>> {
    let source =
        "from facade import b_calculate\ndef main() -> None:\n    answer: int = b_calculate(41)\n    println(answer)\n";
    let (facts, module) = checked_facade(PROVIDER, source)?;
    let start = source.find("b_calculate(41)").ok_or("missing annotated call")?;
    let key = (start, start + "b_calculate(41)".len());
    assert_eq!(facts.expressions.expr_types.get(&key), Some(&ResolvedType::Unknown));
    assert_eq!(
        facts.calls.body_ir_call_result_types.get(&key),
        Some(&ResolvedType::Int)
    );
    assert!(!serde_json::to_string(&body_named(&module, "main")?.locals)?.contains("\"Unknown\""));
    Ok(())
}

/// Synthetic protocol probes preserve the whole private re-export contract without rewriting legacy snapshots.
#[test]
fn reexport_contract_survives_call_site_transactions() -> Result<(), Box<dyn std::error::Error>> {
    let (mut facts, _) = checked_facade(PROVIDER, ORIGINAL)?;
    let start = ORIGINAL.find("b_calculate(41)").ok_or("missing original call")?;
    let span = ast::Span::new(start, start + "b_calculate(41)".len());
    let key = (span.start, span.end);
    let contract = facts
        .calls
        .body_ir_reexport_calls
        .get(&key)
        .ok_or("missing private re-export contract")?
        .clone();
    assert_eq!(contract.canonical.declaration_name, "calculate");
    assert!(
        matches!(&contract.canonical.origin, incan_semantics_core::SymbolOrigin::Package { library, module_path } if library == "calc_lib" && module_path == &["helpers"])
    );
    assert_eq!(contract.parameters.len(), 1);
    assert_eq!(contract.parameters[0].ty, ResolvedType::Int);
    let ordinary = facts.expressions.expr_types.clone();
    let references = facts.references.resolved_identities.clone();
    let saved = facts.take_call_site_facts(span);
    assert!(!facts.calls.body_ir_reexport_calls.contains_key(&key));
    facts.calls.body_ir_reexport_calls.insert(key, contract.clone());
    facts.clear_body_ir_call_site_facts(span);
    assert!(!facts.calls.body_ir_reexport_calls.contains_key(&key));
    facts.restore_call_site_facts(span, saved);
    assert_eq!(facts.calls.body_ir_reexport_calls.get(&key), Some(&contract));
    assert_eq!(
        facts.calls.body_ir_call_result_types.get(&key),
        Some(&ResolvedType::Int)
    );
    assert_eq!(facts.expressions.expr_types, ordinary);
    assert_eq!(facts.references.resolved_identities, references);
    Ok(())
}
