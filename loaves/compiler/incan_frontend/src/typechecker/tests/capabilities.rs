//! RFC 104 capability declarations: description and `requires` checks, provider operations, host capabilities, and
//! module-boundary visibility.

use super::*;

/// A `capability` declaration must reach the symbol table with its RFC 104 clauses intact.
///
/// `requires` is deliberately checked as unresolved path segments: collection records what was written so that a
/// capability may reference one declared later in the same module.
#[test]
fn capability_declarations_collect_their_description_scope_and_requires() -> Result<(), String> {
    let source = r#"
capability http_request:
    description = "Issue an outbound HTTP request"
    scope:
        host: str
    requires = [process.spawn]
"#;
    let tokens = lexer::lex(source).map_err(|errs| format!("lex failed: {errs:?}"))?;
    let program = parser::parse(&tokens).map_err(|errs| format!("parse failed: {errs:?}"))?;
    let mut checker = TypeChecker::new();
    let _ = checker.check_program(&program);

    let symbol_id = checker
        .symbols
        .lookup("http_request")
        .ok_or("the capability declaration was not collected into the symbol table")?;
    let symbol = checker
        .symbols
        .get(symbol_id)
        .ok_or("the collected symbol id did not resolve")?;
    let SymbolKind::Capability(info) = &symbol.kind else {
        return Err(format!("expected a capability symbol, got {:?}", symbol.kind));
    };

    assert_eq!(info.description.as_deref(), Some("Issue an outbound HTTP request"));
    assert_eq!(info.scope.len(), 1, "one declared scope dimension");
    assert_eq!(info.scope[0].0, "host");
    assert_eq!(
        info.scope[0].1,
        ResolvedType::Str,
        "scope dimension types are resolved at collection"
    );
    assert_eq!(info.requires.len(), 1);
    assert_eq!(info.requires[0].path, vec!["process".to_string(), "spawn".to_string()]);
    Ok(())
}

/// A capability must carry RFC 120's `Capability` identity kind rather than falling through to a gap marker.
#[test]
fn a_capability_symbol_carries_the_canonical_capability_identity_kind() {
    use incan_semantics_core::SemanticSourceTargetKind;

    let kind = SymbolKind::Capability(CapabilityInfo {
        description: None,
        scope: Vec::new(),
        requires: Vec::new(),
        is_public: false,
    });

    let spelling = TypeChecker::source_target_kind_for_symbol(&kind);
    assert_eq!(spelling, Some("capability"));
    assert_eq!(
        SemanticSourceTargetKind::from_kind_str("capability"),
        SemanticSourceTargetKind::Capability,
        "the frontend spelling must decode to the canonical kind, not to Other",
    );
}

/// Provider-operation metadata is a checked declaration fact, not a naming convention reconstructed by Body IR.
#[test]
fn provider_operation_decorator_resolves_and_records_its_capability_identity() -> Result<(), String> {
    let source = r#"
capability charge_card:
    description = "Charge one approved card"

@provider_operation(charge_card)
pub def charge(amount: int) -> int:
    return amount
"#;
    let tokens = lexer::lex(source).map_err(|errs| format!("lex failed: {errs:?}"))?;
    let program = parser::parse(&tokens).map_err(|errs| format!("parse failed: {errs:?}"))?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["ledger".to_string(), "api".to_string()]));
    checker
        .check_program(&program)
        .map_err(|errs| format!("provider operation should typecheck: {errs:?}"))?;

    let operations = &checker.type_info().declarations.provider_operations;
    let collected = operations.values().collect::<Vec<_>>();
    let [operation] = collected.as_slice() else {
        return Err(format!("expected one checked provider operation, got {operations:?}"));
    };
    assert_eq!(operation.operation.kind, SemanticSourceTargetKind::Function);
    assert_eq!(
        operation.operation.module_path(),
        Some(vec!["ledger".to_string(), "api".to_string()].as_slice())
    );
    assert_eq!(operation.operation.declaration_name, "charge");
    assert_eq!(operation.required_capability.kind, SemanticSourceTargetKind::Capability);
    assert_eq!(
        operation.required_capability.module_path(),
        Some(vec!["ledger".to_string(), "api".to_string()].as_slice())
    );
    assert_eq!(operation.required_capability.declaration_name, "charge_card");
    assert!(operation.runtime_requirements.is_empty());
    Ok(())
}

/// RFC 104 maps stdlib boundaries onto host capabilities, so an operation must be able to declare that it needs one.
/// The reserved set lives in the compiler's own bundled `std.runtime` source rather than in a package dependency, and
/// dependency resolution alone cannot see it.
#[test]
fn provider_operation_decorator_resolves_a_host_capability() -> Result<(), String> {
    let source = r#"
from std.runtime import host

@provider_operation(host.fs.read)
pub def read_bytes(path: str) -> int:
    return 0
"#;
    let tokens = lexer::lex(source).map_err(|errs| format!("lex failed: {errs:?}"))?;
    let program = parser::parse(&tokens).map_err(|errs| format!("parse failed: {errs:?}"))?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["storage".to_string()]));
    checker
        .check_program(&program)
        .map_err(|errs| format!("a host-capability provider operation should typecheck: {errs:?}"))?;

    let operations = &checker.type_info().declarations.provider_operations;
    let collected = operations.values().collect::<Vec<_>>();
    let [operation] = collected.as_slice() else {
        return Err(format!("expected one checked provider operation, got {operations:?}"));
    };
    assert_eq!(operation.required_capability.kind, SemanticSourceTargetKind::Capability);
    assert_eq!(operation.required_capability.declaration_name, "read");
    assert_eq!(
        operation.required_capability.module_path(),
        Some(
            vec![
                "std".to_string(),
                "runtime".to_string(),
                "host".to_string(),
                "fs".to_string()
            ]
            .as_slice()
        ),
        "the requirement must carry the host capability's own declaring module, not the consumer's"
    );
    Ok(())
}

/// A provider operation may only name a resolved RFC 104 capability, never a callable or a stringly value.
#[test]
fn provider_operation_decorator_rejects_a_non_capability_reference() -> Result<(), String> {
    let source = r#"
def not_a_capability() -> int:
    return 1

@provider_operation(not_a_capability)
def charge(amount: int) -> int:
    return amount
"#;
    let tokens = lexer::lex(source).map_err(|errs| format!("lex failed: {errs:?}"))?;
    let program = parser::parse(&tokens).map_err(|errs| format!("parse failed: {errs:?}"))?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["ledger".to_string(), "api".to_string()]));
    let errors = checker
        .check_program(&program)
        .err()
        .ok_or("a non-capability provider-operation reference must fail")?;
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("does not name a capability")),
        "unexpected diagnostics: {errors:?}"
    );
    assert!(checker.type_info().declarations.provider_operations.is_empty());
    Ok(())
}

/// A capability names an authority, so a bare reference to one is never a value use.
#[test]
fn referencing_a_capability_as_a_value_is_rejected() {
    let source = r#"
capability refund:
    description = "Refund a captured charge"

def use_it() -> int:
    x = refund
    return 1
"#;
    let Err(errs) = check_str(source) else {
        panic!("referencing a capability as a value must fail");
    };
    assert!(
        errs.iter()
            .any(|e| e.message.contains("names a runtime authority, not a value")),
        "expected the capability-as-value diagnostic; got: {errs:?}"
    );
}

/// `capability` is contextual, so every non-declaration position keeps it an ordinary identifier.
#[test]
fn capability_remains_usable_as_an_ordinary_identifier() -> Result<(), String> {
    let source = r#"
model Meter:
    capability: int

def read(m: Meter) -> int:
    capability = m.capability
    return capability
"#;
    check_str(source).map_err(|errs| format!("`capability` must stay an ordinary identifier: {errs:?}"))
}

/// RFC 104 treats the description as part of the authority's public contract, so its absence is a checked error
/// rather than a parse failure — the grammar accepts the omission precisely so this can report it by name.
#[test]
fn a_capability_without_a_description_is_rejected() {
    let errs = check_str_err(
        r#"
capability refund:
    scope:
        tenant: str
"#,
        "capability without a description",
    );
    assert!(
        errs.iter().any(|e| e.message.contains("has no `description`")),
        "expected the missing-description diagnostic; got: {errs:?}"
    );
}

/// A `requires` entry is a checked symbol reference, so an expression that cannot name a declaration is refused at
/// the entry's own span rather than being dropped during collection.
#[test]
fn a_requires_entry_that_is_not_a_reference_is_rejected() {
    let errs = check_str_err(
        r#"
capability refund:
    description = "Refund a captured charge"
    requires = ["host.http.request"]
"#,
        "capability requiring a string",
    );
    assert!(
        errs.iter()
            .any(|e| e.message.contains("must be a capability reference")),
        "expected the non-reference diagnostic; got: {errs:?}"
    );
}

/// RFC 104's authority contract has to be inspectable as static facts, without executing source. The declaration is
/// validated at its own site, so its resolved requirement identities are known there and nowhere else; a consumer
/// that had to re-resolve a dotted reference would be re-implementing the check it is meant to be reading.
#[test]
fn a_capability_declaration_is_retained_as_a_checked_fact() -> Result<(), String> {
    let source = r#"
from std.runtime import host

capability load_policy:
    description = "Load a policy document from disk"
    scope:
        tenant: str
    requires = [host.fs.read]
"#;
    let tokens = lexer::lex(source).map_err(|errs| format!("lex failed: {errs:?}"))?;
    let program = parser::parse(&tokens).map_err(|errs| format!("parse failed: {errs:?}"))?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["policy".to_string()]));
    checker
        .check_program(&program)
        .map_err(|errs| format!("the capability should typecheck: {errs:?}"))?;

    let capabilities = &checker.type_info().declarations.capabilities;
    let collected = capabilities.values().collect::<Vec<_>>();
    let [declaration] = collected.as_slice() else {
        return Err(format!("expected one checked capability, got {capabilities:?}"));
    };
    assert_eq!(declaration.capability.declaration_name, "load_policy");
    assert_eq!(
        declaration.description.as_deref(),
        Some("Load a policy document from disk")
    );
    assert_eq!(
        declaration.scope,
        vec![("tenant".to_string(), "str".to_string())],
        "the typed scope dimensions should be retained in declaration order"
    );

    let [requirement] = declaration.requires.as_slice() else {
        return Err(format!(
            "expected one resolved requirement, got {:?}",
            declaration.requires
        ));
    };
    assert_eq!(requirement.declaration_name, "read");
    assert_eq!(
        requirement.module_path(),
        Some(
            vec![
                "std".to_string(),
                "runtime".to_string(),
                "host".to_string(),
                "fs".to_string()
            ]
            .as_slice()
        ),
        "a requirement must carry the capability's own declaring module, never the consumer's spelling"
    );
    Ok(())
}

/// A requirement that failed to resolve already reported its own error, so recording a placeholder edge would let a
/// consumer read an unproven relationship as a checked one.
#[test]
fn an_unresolved_requirement_is_absent_from_the_checked_fact() {
    let source = r#"
capability load_policy:
    description = "Load a policy document from disk"
    requires = [nonexistent]
"#;
    let Ok(tokens) = lexer::lex(source) else {
        panic!("lex failed");
    };
    let Ok(program) = parser::parse(&tokens) else {
        panic!("parse failed");
    };
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["policy".to_string()]));
    let errors = checker.check_program(&program).err().unwrap_or_default();
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("Unknown capability 'nonexistent'")),
        "expected the unresolved-requirement diagnostic; got: {errors:?}"
    );

    let capabilities = &checker.type_info().declarations.capabilities;
    let collected = capabilities.values().collect::<Vec<_>>();
    let [declaration] = collected.as_slice() else {
        panic!("expected the capability itself to still be recorded, got {capabilities:?}");
    };
    assert!(
        declaration.requires.is_empty(),
        "an unproven requirement must not be recorded: {:?}",
        declaration.requires
    );
}

/// RFC 104 reserves `host.*` for the toolchain's own authority and declares it through the same mechanism a package
/// uses, in the compiler's bundled `std.runtime` source. A `requires` entry naming one therefore has to resolve like
/// any other capability reference, rather than being a spelling the compiler has no declaration for.
#[test]
fn a_requires_entry_naming_a_host_capability_resolves() -> Result<(), String> {
    check_str(
        r#"
from std.runtime import host

capability load_policy:
    description = "Load a policy document from disk"
    requires = [host.fs.read]
"#,
    )
    .map_err(|errs| format!("host.fs.read should resolve: {errs:?}"))
}

/// Only `std.runtime`'s own bundled source declares under `host.*`, so a reference that names a spelling it does not
/// declare stays an unresolved-symbol error rather than being admitted because the namespace looks familiar.
#[test]
fn a_requires_entry_naming_an_undeclared_host_capability_is_rejected() {
    let errs = check_str_err(
        r#"
from std.runtime import host

capability load_policy:
    description = "Load a policy document from disk"
    requires = [host.fs.chmod]
"#,
        "capability requiring an undeclared host capability",
    );
    assert!(
        errs.iter()
            .any(|e| e.message.contains("Unknown capability 'host.fs.chmod'")),
        "expected the unresolved-requirement diagnostic; got: {errs:?}"
    );
}

/// RFC 104 is explicit that a misspelled capability reference must fail at compile time rather than surviving to
/// become a runtime denial reported far from the declaration that caused it.
#[test]
fn an_unknown_capability_requirement_is_rejected() {
    let errs = check_str_err(
        r#"
capability refund:
    description = "Refund a captured charge"
    requires = [nonexistent]
"#,
        "capability requiring an unknown name",
    );
    assert!(
        errs.iter()
            .any(|e| e.message.contains("Unknown capability 'nonexistent'")),
        "expected the unresolved-requirement diagnostic; got: {errs:?}"
    );
}

/// A `requires` list naming a function describes an authority relationship that cannot exist, and a policy reading
/// it would draw a false conclusion — so the wrong kind is refused as firmly as a missing one.
#[test]
fn a_requires_entry_naming_a_non_capability_is_rejected() {
    let errs = check_str_err(
        r#"
def helper() -> int:
    return 1

capability refund:
    description = "Refund a captured charge"
    requires = [helper]
"#,
        "capability requiring a function",
    );
    assert!(
        errs.iter()
            .any(|e| e.message.contains("is a function, not a capability")),
        "expected the wrong-kind diagnostic; got: {errs:?}"
    );
}

/// Capabilities may reference each other in any order, which is why resolution is a checking pass rather than part
/// of collection: requiring one declared further down the file must not depend on declaration order.
#[test]
fn a_capability_may_require_another_declared_later_in_the_module() -> Result<(), String> {
    check_str(
        r#"
capability refund:
    description = "Refund a captured charge"
    requires = [audit_write]

capability audit_write:
    description = "Append to the audit log"
"#,
    )
    .map_err(|errs| format!("declaration order must not matter for `requires`: {errs:?}"))
}

/// A `pub capability` is exported and referenceable across a module boundary.
///
/// The stdlib is only the first capability publisher: a library author declares domain capabilities in their own
/// module and consumers reference them by import. Withholding capabilities from the export set would make `pub`
/// meaningless on the one declaration form whose entire purpose is to be referenced from elsewhere.
#[test]
fn a_public_capability_crosses_a_module_boundary() -> Result<(), Box<dyn std::error::Error>> {
    let dependency = parse_program(
        r#"
pub capability audit_write:
    description = "Append to the audit log"
"#,
        "capability publisher",
    );
    let consumer = parse_program(
        r#"
capability refund:
    description = "Refund a captured charge"
    requires = [publisher.audit_write]
"#,
        "capability consumer",
    );

    let mut checker = TypeChecker::new();
    checker
        .check_with_imports(&consumer, &[("publisher", &dependency)])
        .map_err(|errs| format!("a `pub capability` must be referenceable across modules: {errs:?}"))?;
    Ok(())
}

/// A capability that is *not* `pub` stays private, so a consumer referencing it fails to resolve.
#[test]
fn a_private_capability_does_not_cross_a_module_boundary() {
    let dependency = parse_program(
        r#"
capability audit_write:
    description = "Append to the audit log"
"#,
        "private capability publisher",
    );
    let consumer = parse_program(
        r#"
capability refund:
    description = "Refund a captured charge"
    requires = [publisher.audit_write]
"#,
        "private capability consumer",
    );

    let mut checker = TypeChecker::new();
    let Err(errs) = checker.check_with_imports(&consumer, &[("publisher", &dependency)]) else {
        panic!("a private capability must not be referenceable from another module");
    };
    assert!(
        errs.iter()
            .any(|e| e.message.contains("Unknown capability 'publisher.audit_write'")),
        "expected the unresolved-requirement diagnostic; got: {errs:?}"
    );
}

/// A `description` clause that is not compile-time text is rejected rather than silently stored as nothing.
///
/// Presence alone was not enough: collection keeps only text it can read, so a clause it cannot read left the
/// declaration carrying no description while appearing to have one.
#[test]
fn a_capability_description_that_is_not_text_is_rejected() {
    for source in [
        "\ncapability refund:\n    description = 42\n",
        "\ncapability refund:\n    description = [\"a\", \"b\"]\n",
    ] {
        let errs = check_str_err(source, "capability with a non-text description");
        assert!(
            errs.iter()
                .any(|e| e.message.contains("`description` that is not a text literal")),
            "expected the non-text description diagnostic for {source:?}; got: {errs:?}"
        );
    }
}

/// Collection and checking must agree about what counts as a description, so a rejected clause never reaches the
/// symbol table as a silently absent one while the declaration still typechecks.
#[test]
fn a_rejected_description_never_leaves_a_capability_looking_documented() -> Result<(), String> {
    let source = "\ncapability refund:\n    description = 42\n";
    let tokens = lexer::lex(source).map_err(|errs| format!("lex failed: {errs:?}"))?;
    let program = parser::parse(&tokens).map_err(|errs| format!("parse failed: {errs:?}"))?;
    let mut checker = TypeChecker::new();
    let result = checker.check_program(&program);

    assert!(result.is_err(), "a non-text description must not typecheck");

    let symbol_id = checker
        .symbols
        .lookup("refund")
        .ok_or("the capability was not collected")?;
    let symbol = checker.symbols.get(symbol_id).ok_or("the symbol id did not resolve")?;
    let SymbolKind::Capability(info) = &symbol.kind else {
        return Err(format!("expected a capability symbol, got {:?}", symbol.kind));
    };
    assert_eq!(
        info.description, None,
        "collection stores nothing it cannot read, which is exactly why checking must reject the clause"
    );
    Ok(())
}
