//! Checked source hook ingress and ordered fail-fast newtype chains.

use super::{body_named, build};

/// Builtin error storage and construction carry a distinct checked token, including inside intrinsic Results.
#[test]
fn validation_error_retains_builtin_type_and_constructor() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "def make() -> Result[int, ValidationError]:\n    return Err(ValidationError(message=\"failure\", code=\"E\"))\n\ndef main() -> None:\n    pass\n",
        &["m", "validation_error_carrier"],
    )?;
    let snapshot = body_named(&module, "make")?.render_snapshot();
    assert!(snapshot.contains("runtime::ValidationError"), "{snapshot}");
    assert!(snapshot.contains("helper:validation_error_with_code"), "{snapshot}");
    assert!(!snapshot.contains("unsupported("), "{snapshot}");
    let make = body_named(&module, "make")?;
    let mut results = 0;
    for statement in &make.block.stmts {
        if let super::bir::StatementKind::Assign {
            rvalue: super::bir::Rvalue::ResultVariant(result),
            ..
        } = &statement.kind
        {
            results += 1;
            assert!(result.has_complete_canonical_types(), "{snapshot}");
            let mut missing = result.clone();
            missing.canonical_types.clear();
            assert!(!missing.has_complete_canonical_types());
        }
    }
    assert_eq!(results, 1, "{snapshot}");
    Ok(())
}

/// Generic call settlement keeps the exact builtin proof for both a written and a newly settled error side.
#[test]
fn settled_results_preserve_validation_error_identity() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "def pick[T](result: Result[T, ValidationError], fallback: T) -> Result[T, ValidationError]:\n    return result\n\ndef main() -> None:\n    failure = pick(Err(ValidationError(\"failure\")), 1)\n    success = pick(Ok(2), 3)\n",
        &["m", "settled_validation_error"],
    )?;
    let main = body_named(&module, "main")?;
    let mut results = 0;
    for statement in &main.block.stmts {
        if let super::bir::StatementKind::Assign {
            rvalue: super::bir::Rvalue::ResultVariant(result),
            ..
        } = &statement.kind
        {
            assert_eq!(
                result.error_type.to_string(),
                "runtime::ValidationError",
                "{}",
                main.render_snapshot()
            );
            assert!(result.has_complete_canonical_types(), "{}", main.render_snapshot());
            results += 1;
        }
    }
    assert_eq!(results, 2);
    Ok(())
}

/// Provider types with the builtin's spelling require exact scope provenance rather than the ambient root binding.
#[test]
fn provider_validation_error_names_never_become_builtin_storage() -> Result<(), Box<dyn std::error::Error>> {
    let source = "pub model ValidationError:\n    pub message: str\n\npub def make() -> ValidationError:\n    return ValidationError(message=\"source\")\n";
    let provider = build(source, &["provider"])?;
    for (consumer, body) in [
        (
            "import provider\n\ndef consume(value: provider.ValidationError) -> None:\n    pass\n\ndef main() -> None:\n    pass\n",
            "consume",
        ),
        (
            "from provider import make\n\ndef main() -> None:\n    value = make()\n",
            "main",
        ),
    ] {
        let module = super::identities_and_imports::build_with_published_imports(
            consumer,
            &["consumer"],
            &[("provider", &["provider"], source)],
            std::slice::from_ref(&provider),
        )?;
        let snapshot = body_named(&module, body)?.render_snapshot();
        assert!(!snapshot.contains("runtime::ValidationError"), "{snapshot}");
        assert!(
            snapshot.contains("ambiguous builtin ValidationError type provenance"),
            "{snapshot}"
        );
    }
    let alias = super::identities_and_imports::build_with_published_imports(
        "from provider import ValidationError as DomainError\n\ndef main() -> None:\n    value = DomainError(message=\"source\")\n",
        &["consumer"],
        &[("provider", &["provider"], source)],
        &[provider],
    )?;
    let snapshot = body_named(&alias, "main")?.render_snapshot();
    assert!(snapshot.contains("constructor(DomainError)"), "{snapshot}");
    assert!(!snapshot.contains("runtime::ValidationError"), "{snapshot}");
    assert!(!snapshot.contains("ambiguous builtin"), "{snapshot}");
    let source = "pub type ValidationError = newtype float[ge=0, le=1]\n\npub def make() -> ValidationError:\n    return ValidationError(0.5)\n";
    let provider = build(source, &["provider"])?;
    assert!(
        provider
            .nominal_declarations
            .iter()
            .all(|declaration| declaration.name != "ValidationError")
    );
    let identity = body_named(&provider, "make")?
        .named_type_identities
        .get("ValidationError")
        .ok_or("missing source newtype identity")?;
    assert_eq!(identity.kind, super::SemanticSourceTargetKind::Newtype);
    let consumer = super::identities_and_imports::build_with_published_imports(
        "from provider import make\n\ndef main() -> None:\n    value = make()\n",
        &["consumer"],
        &[("provider", &["provider"], source)],
        &[provider],
    )?;
    let snapshot = body_named(&consumer, "main")?.render_snapshot();
    assert!(!snapshot.contains("runtime::ValidationError"), "{snapshot}");
    assert!(
        snapshot.contains("ambiguous builtin ValidationError type provenance"),
        "{snapshot}"
    );
    Ok(())
}

/// A user declaration and a generic binder named ValidationError keep their original source categories.
#[test]
fn validation_error_runtime_token_does_not_replace_source_types() -> Result<(), Box<dyn std::error::Error>> {
    let source = build(
        "model ValidationError:\n    message: str\n\ndef main() -> None:\n    value = ValidationError(message=\"source\")\n    println(value.message)\n",
        &["m", "shadowed_validation_error"],
    )?;
    let snapshot = source.render_snapshot();
    assert!(!snapshot.contains("runtime::ValidationError"), "{snapshot}");
    assert!(!snapshot.contains("helper:validation_error"), "{snapshot}");
    assert!(snapshot.contains("constructor(ValidationError)"), "{snapshot}");
    let generic = build(
        "def identity[ValidationError](value: ValidationError) -> ValidationError:\n    return value\n\ndef main() -> None:\n    pass\n",
        &["m", "validation_error_binder"],
    )?;
    let body = body_named(&generic, "identity")?;
    assert_eq!(body.return_type, super::IncanType::TypeVar("ValidationError".into()));
    assert!(!body.render_snapshot().contains("runtime::ValidationError"));
    Ok(())
}

/// Imported checked layouts never authorize a raw aggregate; imported raw layouts retain their checked ingress.
#[test]
fn imported_newtype_hooks_remain_refused_without_source_binding() -> Result<(), Box<dyn std::error::Error>> {
    let provider_source = "pub type Checked = newtype int:\n    def from_underlying(value: int) -> Result[Self, ValidationError]:\n        return Ok(Checked(value))\n\npub type Raw = newtype int\n";
    let provider = build(provider_source, &["provider"])?;
    let module = super::identities_and_imports::build_with_published_imports(
        "from provider import Checked as Alias, Raw\n\ndef main() -> None:\n    checked = Alias(0)\n    raw = Raw(1)\n",
        &["consumer"],
        &[("provider", &["provider"], provider_source)],
        &[provider],
    )?;
    let snapshot = body_named(&module, "main")?.render_snapshot();
    assert!(snapshot.contains("checked-newtype imported hook body"), "{snapshot}");
    assert!(!snapshot.contains("constructor(Alias)"), "{snapshot}");
    assert!(snapshot.contains("constructor(Raw)"), "{snapshot}");
    Ok(())
}

/// Missing ingress metadata cannot turn a published raw layout into unchecked constructor authorization.
#[test]
fn imported_newtype_without_ingress_metadata_stays_refused() -> Result<(), Box<dyn std::error::Error>> {
    let provider_source = "pub type Raw = newtype int\n";
    let provider = build(provider_source, &["provider"])?;
    let tokens = super::lexer::lex(provider_source).map_err(|errors| format!("{errors:?}"))?;
    let imported = super::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let tokens = super::lexer::lex("from provider import Raw as Alias\n\ndef main() -> None:\n    value = Alias(1)\n")
        .map_err(|errors| format!("{errors:?}"))?;
    let program = super::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let path = vec!["consumer".to_owned()];
    let mut checker = super::TypeChecker::new();
    checker.set_current_module_path(Some(path.clone()));
    checker
        .check_with_imports(&program, &[("provider", &imported)])
        .map_err(|errors| format!("{errors:?}"))?;
    let mut facts = checker.type_info().clone();
    assert!(facts.declarations.newtype_construction.remove("Alias").is_some());
    let module = super::build_body_ir_module_v0_with_executable_context(&program, &path, &facts, &[provider]);
    let snapshot = body_named(&module, "main")?.render_snapshot();
    assert!(
        snapshot.contains("newtype construction without checked ingress metadata"),
        "{snapshot}"
    );
    assert!(!snapshot.contains("constructor(Alias)"), "{snapshot}");
    Ok(())
}

/// A source hook cannot authorize ordinary serde reconstruction, which would bypass its validation.
#[test]
fn checked_newtype_deserialize_stays_outside_raw_layout_admission() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "from std.serde.json import Deserialize\n\n@derive(Deserialize)\ntype Checked = newtype int:\n    def from_underlying(value: int) -> Result[Self, ValidationError]:\n        return Ok(Checked(value))\n\ndef main() -> None:\n    pass\n",
        &["m", "newtype_hook_deserialize"],
    )?;
    assert!(module.nominal_declarations.is_empty());
    Ok(())
}

/// Explicit and implicit outside-owner construction call the canonical hook before extracting its checked result.
#[test]
fn newtype_hook_ingress_retains_checked_method_identity() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "type Positive = newtype int:\n    def from_underlying(value: int) -> Result[Self, ValidationError]:\n        if value <= 0:\n            return Err(ValidationError(\"positive required\"))\n        return Ok(Positive(value))\n\ndef take(value: Positive) -> None:\n    pass\n\ndef main() -> None:\n    explicit = Positive(1)\n    take(2)\n",
        &["m", "newtype_hook_ingress"],
    )?;
    assert_eq!(module.nominal_declarations.len(), 1);
    let hook = body_named(&module, "from_underlying")?;
    let raw = hook.render_snapshot();
    assert!(raw.contains("constructor(Positive)"), "{raw}");
    assert!(!raw.contains("helper:newtype_hook_extract"), "{raw}");
    let main = body_named(&module, "main")?;
    let snapshot = main.render_snapshot();
    assert_eq!(snapshot.matches("helper:newtype_hook_extract").count(), 2, "{snapshot}");
    let mut calls = 0;
    for statement in &main.block.stmts {
        if let super::bir::StatementKind::Call {
            callee: super::bir::Callee::Function(super::bir::CallableTarget::Named(target)),
            args,
            ..
        } = &statement.kind
            && target.canonical == hook.canonical
        {
            assert_eq!(args.len(), 1);
            assert_eq!(target.direct_call_id.as_ref(), Some(&hook.direct_call_id));
            calls += 1;
        }
    }
    assert_eq!(calls, 2);
    Ok(())
}

/// The checker-selected inner hook executes before the outer raw newtype wrapper in a fail-fast coercion chain.
#[test]
fn newtype_hook_chain_keeps_underlying_to_target_order() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "type Positive = newtype int:\n    def from_underlying(value: int) -> Result[Self, ValidationError]:\n        return Ok(Positive(value))\n\ntype Wrapped = newtype Positive\n\ndef main() -> None:\n    value: Wrapped = 2\n    inner = value.0\n    println(inner.0)\n",
        &["m", "newtype_hook_chain"],
    )?;
    assert_eq!(module.nominal_declarations.len(), 2);
    let snapshot = body_named(&module, "main")?.render_snapshot();
    let hook = snapshot
        .find("helper:newtype_hook_extract")
        .ok_or("missing hook extraction")?;
    let outer = snapshot.find("constructor(Wrapped)").ok_or("missing outer wrapper")?;
    assert!(hook < outer, "{snapshot}");
    assert!(!snapshot.contains("unsupported("), "{snapshot}");
    Ok(())
}
