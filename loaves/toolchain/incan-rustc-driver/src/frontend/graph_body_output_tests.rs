//! Graph body output and provenance fixture controls.

use super::{GraphBodyOutput, canonical_lowering_inputs, fallback_identity};

/// Explicit sidecar roots resolve aliases and refuse missing roots; an inactive collector needs neither.
#[test]
fn sidecar_root_resolves_aliases_and_refuses_missing_roots() -> Result<(), Box<dyn std::error::Error>> {
    let root = std::env::temp_dir().join(format!("incan-graph-root-{}", std::process::id()));
    std::fs::create_dir(&root)?;
    let actual = root.join("actual");
    let alias = root.join("alias");
    std::fs::create_dir(&actual)?;
    std::os::unix::fs::symlink(&actual, &alias)?;
    let output = Some(root.join("bodies"));
    let collector = GraphBodyOutput::prepare(output.clone(), alias)?;
    assert_eq!(collector.project_root, actual.canonicalize()?);
    assert!(!collector.namespace_proofs);
    let current_output = root.join(format!("units{}", internal_models::namespace_proof_output_suffix()));
    assert!(GraphBodyOutput::prepare(Some(current_output), actual.clone())?.namespace_proofs);
    let missing = root.join("missing");
    let refused = match GraphBodyOutput::prepare(output, missing.clone()) {
        Ok(_) => return Err("an explicit missing graph body root was accepted".into()),
        Err(message) => message,
    };
    assert!(refused.contains("cannot resolve graph body project root"));
    assert!(GraphBodyOutput::prepare(None, missing).is_ok());
    std::fs::remove_dir_all(root)?;
    Ok(())
}

/// Framing coalesces equal owner evidence and names disagreement before the Incan codec sees it.
#[test]
fn lowering_inputs_are_unique_by_rendered_owner() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(
        canonical_lowering_inputs(vec![
            vec!["main#decl.0..12".to_owned(), "body:a".to_owned()],
            vec!["main#decl.0..12".to_owned(), "body:a".to_owned()],
        ])?,
        vec![vec!["main#decl.0..12".to_owned(), "body:a".to_owned()]]
    );
    assert_eq!(
        canonical_lowering_inputs(vec![
            vec!["main#decl.0..12".to_owned(), "body:a".to_owned()],
            vec!["main#decl.0..12".to_owned(), "body:b".to_owned()],
        ]),
        Err("conflicting lowering witnesses for Body IR owner main#decl.0..12".to_owned())
    );
    Ok(())
}

/// A checker fallback without an identity is encoded as absence rather than an invalid present-empty value.
#[test]
fn fallback_identity_normalizes_empty_presence() {
    assert_eq!(fallback_identity(Some(String::new())), None);
    assert_eq!(fallback_identity(None), None);
    assert_eq!(fallback_identity(Some("owner".to_owned())), Some("owner".to_owned()));
}

/// Check one module `source`, lower its Body IR and attach it to a graph-body collector, as a compile does.
fn attached_output(
    source: &str,
    test: &str,
    typed: bool,
) -> Result<(GraphBodyOutput, std::path::PathBuf), Box<dyn std::error::Error>> {
    let scratch = std::env::temp_dir().join(format!("incan-graph-{test}-{}", std::process::id()));
    std::fs::create_dir_all(scratch.join("src"))?;
    let root = scratch.canonicalize()?;
    let tokens = incan_frontend::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let ast = incan_frontend::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = incan_frontend::typechecker::TypeChecker::new();
    checker.set_current_module_path(Some(vec!["main".into()]));
    checker.check_program(&ast).map_err(|errors| format!("{errors:?}"))?;
    let providers = incan_frontend::body_ir::ProviderOperationCatalog::new();
    let mut lowered = incan_frontend::body_ir::build_body_ir_module(
        &ast,
        &["main".into()],
        checker.type_info(),
        &incan_frontend::body_ir::BodyIrContext {
            modules: incan_frontend::body_ir::ContextModules::default(),
            provider_operations: &providers,
            registry_package: None,
            library_package: None,
            include_inline_tests: false,
        },
    );
    let module = incan_frontend::parsed_module::ParsedModule {
        name: "main".into(),
        path_segments: vec!["main".into()],
        file_path: root.join("src/main.incn"),
        source_positions: incan_frontend::diagnostics::SourcePositionIndex::new(source),
        source: source.to_owned(),
        ast,
        gated_exports: Vec::new(),
    };
    let name = if typed {
        format!(
            "bodies{}.published{}",
            internal_models::executable_graph_output_marker(),
            internal_models::scope_output_suffix()
        )
    } else {
        "bodies".to_owned()
    };
    let mut output = GraphBodyOutput::prepare(Some(root.join(name)), root.clone())?;
    let provider = typed.then(|| {
        checker.seal_current_module_dependency_interface(
            "main",
            &module.ast,
            std::sync::Arc::from(incan_frontend::module::exported_symbols(&module.ast)),
            true,
        )
    });
    output.observe(&module, &checker, provider.as_deref(), &[], "app");
    if let Some(error) = output.error.take() {
        return Err(error.into());
    }
    output.attach_body_ir(
        std::slice::from_ref(&module),
        std::slice::from_mut(&mut lowered),
        &[],
        &std::collections::BTreeMap::new(),
        None,
    )?;
    Ok((output, scratch))
}

/// Two overloads whose source signatures differ only in their bounds share one stable identity, so neither can
/// own a graph-body row: their spelling is one fallback owner, and the output keeps one row per identity.
#[test]
fn overloads_sharing_one_source_signature_are_one_fallback_owner() -> Result<(), Box<dyn std::error::Error>> {
    let source = concat!(
        "model Left:\n    value: int\n\n",
        "model Right:\n    value: int\n\n",
        "def kind[T with Left]() -> str:\n    return \"left\"\n\n",
        "def kind[T with Right]() -> str:\n    return \"right\"\n\n",
        "def main() -> None:\n    println(kind[Right]())\n",
    );
    let (output, scratch) = attached_output(source, "shared-signature", false)?;
    let mut identities = std::collections::BTreeSet::new();
    assert!(
        output
            .values
            .iter()
            .all(|value| identities.insert((value.module.clone(), value.identity.clone()))),
        "two graph-body values share one identity"
    );
    let kinds = output
        .fallbacks
        .iter()
        .filter(|fallback| fallback.name == "kind")
        .count();
    assert_eq!(kinds, 1, "the shared spelling is not one fallback owner");
    std::fs::remove_dir_all(&scratch)?;
    Ok(())
}

/// A checked body the module's Body IR gives no row of its own, a trait's abstract method or a top-level partial's
/// generated function, is published as a fallback owner with its reason instead of refusing the whole output.
#[test]
fn bodies_without_their_own_body_ir_row_are_fallback_owners() -> Result<(), Box<dyn std::error::Error>> {
    let source = concat!(
        "trait Named:\n    def label(self) -> str:\n        \"\"\"Return the value label.\"\"\"\n        ...\n\n",
        "model One with Named:\n    number: int\n\n    def label(self) -> str:\n        return str(self.number)\n\n",
        "pub def route(method: str, path: str) -> str:\n    return method + path\n\n",
        "pub get = partial route(method=\"GET\")\n\n",
        "def main() -> None:\n    println(get(path=\"/\"))\n",
    );
    let (output, scratch) = attached_output(source, "fallbacks", true)?;
    let fallbacks: Vec<(&str, &str)> = output
        .fallbacks
        .iter()
        .map(|fallback| (fallback.name.as_str(), fallback.reason.as_str()))
        .collect();
    assert!(
        fallbacks
            .iter()
            .any(|(name, reason)| *name == "label" && reason.contains("no Body IR row")),
        "{fallbacks:?}"
    );
    assert!(
        fallbacks
            .iter()
            .any(|(name, reason)| *name == "get" && reason.contains("partial")),
        "{fallbacks:?}"
    );
    assert!(output.values.iter().all(|value| value.body_ir.starts_with(b"CRMB")));
    assert_eq!(output.namespaces.len(), 1);
    assert!(output.namespaces[0].body_ir.starts_with(b"CRMB"));
    assert!(output.values.iter().any(|value| value.name == "route"));
    std::fs::remove_dir_all(&scratch)?;
    Ok(())
}
