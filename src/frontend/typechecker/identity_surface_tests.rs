//! Focused canonical-identity coverage for path-directed and compiler-owned frontend surfaces.

use super::*;
use crate::frontend::{lexer, parser};
use incan_core::lang::traits::{self, TraitId};
use incan_semantics_core::{SemanticSourceTargetKind, SymbolNamespace, SymbolOrigin};

/// Parse one focused identity fixture and preserve compiler diagnostics on failure.
fn parse(source: &str) -> Result<Program, String> {
    let tokens = lexer::lex(source).map_err(|errors| format!("lex failed: {errors:?}"))?;
    parser::parse(&tokens).map_err(|errors| format!("parse failed: {errors:?}"))
}

/// Return the exact source span of the indexed fixture spelling.
fn nth_span(source: &str, needle: &str, index: usize) -> Result<Span, String> {
    source
        .match_indices(needle)
        .nth(index)
        .map(|(start, value)| Span::new(start, start + value.len()))
        .ok_or_else(|| format!("missing occurrence {index} of `{needle}`"))
}

/// Module aliases resolve to the provider module's single canonical path identity.
#[test]
fn module_aliases_preserve_one_resolved_module_path_identity() -> Result<(), String> {
    let provider = parse("pub def answer() -> int:\n  return 42\n")?;
    let consumer = parse("import helpers as first\nimport helpers as second\n")?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["app".to_string(), "main".to_string()]));
    checker.register_dependency_module_path_segments("helpers", vec!["shared".to_string(), "helpers".to_string()]);
    checker
        .check_with_imports(&consumer, &[("helpers", &provider)])
        .map_err(|errors| format!("module aliases should typecheck: {errors:?}"))?;

    let first_id = checker
        .symbols
        .lookup("first")
        .and_then(|id| checker.symbols.identity_of(id))
        .ok_or("first alias has no module identity")?;
    let second_id = checker
        .symbols
        .lookup("second")
        .and_then(|id| checker.symbols.identity_of(id))
        .ok_or("second alias has no module identity")?;
    assert_eq!(first_id, second_id);
    assert_eq!(first_id.namespace, SymbolNamespace::ModulePath);
    assert_eq!(
        first_id.origin,
        SymbolOrigin::Module(vec!["shared".to_string(), "helpers".to_string()])
    );
    assert_eq!(first_id.declaration_name, "helpers");
    assert_eq!(first_id.kind, SemanticSourceTargetKind::Module);
    Ok(())
}

/// Standard-library constant access retains the declaration identity owned by its module.
#[test]
fn stdlib_module_constant_access_retains_its_declaration_identity() -> Result<(), String> {
    let source = "import std.math as math\n\ndef circle_constant() -> float:\n  return math.PI\n";
    let program = parse(source)?;
    let mut checker = TypeChecker::new();
    checker
        .check_program(&program)
        .map_err(|errors| format!("stdlib constant access should typecheck: {errors:?}"))?;

    let access = nth_span(source, "math.PI", 0)?;
    let identity = checker
        .type_info()
        .resolved_identity(access)
        .ok_or("stdlib constant access has no identity")?;
    assert_eq!(
        identity.origin,
        SymbolOrigin::Module(vec!["std".to_string(), "math".to_string()])
    );
    assert_eq!(identity.declaration_name, "PI");
    assert_eq!(identity.kind, SemanticSourceTargetKind::Const);
    assert_ne!(identity.declaration_span.start, identity.declaration_span.end);
    Ok(())
}

/// A qualified source type alias resolves to its provider declaration rather than its local spelling.
#[test]
fn dotted_source_type_alias_resolves_to_the_provider_declaration() -> Result<(), String> {
    let provider = parse("pub model Payload:\n  value: int\n")?;
    let source = "import schemas as schema\n\ndef echo(value: schema.Payload) -> schema.Payload:\n  return value\n";
    let consumer = parse(source)?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["app".to_string()]));
    checker
        .check_with_imports(&consumer, &[("schemas", &provider)])
        .map_err(|errors| format!("dotted source type should typecheck: {errors:?}"))?;

    let parameter = checker
        .type_info()
        .resolved_identity(nth_span(source, "schema.Payload", 0)?)
        .ok_or("dotted parameter type has no identity")?;
    let returned = checker
        .type_info()
        .resolved_identity(nth_span(source, "schema.Payload", 1)?)
        .ok_or("dotted return type has no identity")?;
    assert_eq!(parameter, returned);
    assert_eq!(parameter.origin, SymbolOrigin::Module(vec!["schemas".to_string()]));
    assert_eq!(parameter.declaration_name, "Payload");
    assert_eq!(parameter.kind, SemanticSourceTargetKind::Model);
    Ok(())
}

/// Qualified and direct Rust imports retain one resolved crate-item identity.
#[test]
fn rust_qualified_type_uses_the_resolved_crate_path_not_the_alias() -> Result<(), String> {
    let source = "import rust::std::path as filesystem\nfrom rust::std::path import Path as DirectPath\n\ndef echo(value: filesystem::Path) -> DirectPath:\n  return value\n";
    let program = parse(source)?;
    let mut checker = TypeChecker::new();
    checker
        .check_program(&program)
        .map_err(|errors| format!("qualified Rust type should typecheck: {errors:?}"))?;

    let identity = checker
        .type_info()
        .resolved_identity(nth_span(source, "filesystem::Path", 0)?)
        .ok_or("qualified Rust type has no identity")?;
    assert_eq!(
        identity.origin,
        SymbolOrigin::RustCrate(vec!["std".to_string(), "path".to_string(), "Path".to_string()])
    );
    assert_eq!(identity.declaration_name, "Path");
    assert_eq!(identity.kind, SemanticSourceTargetKind::RustItem);
    assert_eq!(identity.namespace, SymbolNamespace::OrdinaryLexical);
    let direct = checker
        .type_info()
        .resolved_identity(nth_span(source, "DirectPath", 1)?)
        .ok_or("direct Rust import type has no identity")?;
    assert_eq!(
        identity, direct,
        "qualified and direct imports of one Rust item must agree"
    );
    Ok(())
}

/// Compiler-owned builtin members carry owner-discriminated canonical identities.
#[test]
fn compiler_builtin_members_are_canonicalized_and_owner_discriminated() -> Result<(), String> {
    let source = r#"
def inspect(values: List[int], seen: Set[int], text: str) -> bool:
  let list_has = values.contains(1)
  let text_has = text.starts_with("x")
  return list_has and text_has and seen.contains(1)
"#;
    let program = parse(source)?;
    let mut checker = TypeChecker::new();
    checker
        .check_program(&program)
        .map_err(|errors| format!("builtin member calls should typecheck: {errors:?}"))?;

    let list = checker
        .type_info()
        .resolved_identity(nth_span(source, "values.contains(1)", 0)?)
        .ok_or("List.contains call has no identity")?;
    let set = checker
        .type_info()
        .resolved_identity(nth_span(source, "seen.contains(1)", 0)?)
        .ok_or("Set.contains call has no identity")?;
    let string = checker
        .type_info()
        .resolved_identity(nth_span(source, "text.starts_with(\"x\")", 0)?)
        .ok_or("str.startswith alias call has no identity")?;
    assert_eq!(list.declaration_name, "List.contains");
    assert_eq!(set.declaration_name, "Set.contains");
    assert_ne!(
        list, set,
        "same-spelled members on different builtin owners must stay distinct"
    );
    assert_eq!(string.declaration_name, "str.startswith");
    assert_eq!(string.origin, SymbolOrigin::Builtin);
    assert_eq!(string.namespace, SymbolNamespace::Member);
    Ok(())
}

/// Synthetic enum members with equal spellings remain distinct across nominal owners.
#[test]
fn compiler_synthetic_enum_members_are_owner_discriminated() -> Result<(), String> {
    let source = r#"
enum First:
  One

enum Second:
  Two

def first_message(value: First) -> str:
  return value.message()

def second_message(value: Second) -> str:
  return value.message()
"#;
    let program = parse(source)?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["messages".to_string()]));
    checker
        .check_program(&program)
        .map_err(|errors| format!("synthetic enum helpers should typecheck: {errors:?}"))?;

    let first = checker
        .type_info()
        .resolved_identity(nth_span(source, "value.message()", 0)?)
        .ok_or("First.message has no synthetic identity")?;
    let second = checker
        .type_info()
        .resolved_identity(nth_span(source, "value.message()", 1)?)
        .ok_or("Second.message has no synthetic identity")?;
    assert_eq!(first.declaration_name, "message");
    assert_eq!(second.declaration_name, "message");
    assert_ne!(first, second, "synthetic members must retain their nominal owner");
    assert_eq!(first.kind, SemanticSourceTargetKind::Method);
    assert_eq!(first.namespace, SymbolNamespace::Member);
    Ok(())
}

/// Decorator and provider-operation arguments retain compiler-owned target identities.
#[test]
fn decorator_and_provider_operation_arguments_record_compiler_owned_targets() -> Result<(), String> {
    let source = r#"
capability charge_card:
  description = "Charge a card"

@derive(Clone)
model Receipt:
  amount: int

@provider_operation(charge_card)
def charge(amount: int) -> int:
  return amount
"#;
    let program = parse(source)?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["ledger".to_string()]));
    checker
        .check_program(&program)
        .map_err(|errors| format!("decorated provider operation should typecheck: {errors:?}"))?;

    let capability = checker
        .type_info()
        .resolved_identity(nth_span(source, "charge_card", 1)?)
        .ok_or("provider-operation capability argument has no identity")?;
    assert_eq!(capability.declaration_name, "charge_card");
    assert_eq!(capability.kind, SemanticSourceTargetKind::Capability);

    let clone = traits::as_str(TraitId::Clone);
    let derive = checker
        .type_info()
        .resolved_identity(nth_span(source, clone, 0)?)
        .ok_or("derive argument has no identity")?;
    assert_eq!(derive.declaration_name, clone);
    assert_eq!(derive.origin, SymbolOrigin::Builtin);
    Ok(())
}

/// Qualified provider-operation arguments preserve the imported capability's identity.
#[test]
fn qualified_provider_operation_argument_preserves_the_imported_capability_identity() -> Result<(), String> {
    let provider = parse("pub capability audit_write:\n  description = \"Append to the audit log\"\n")?;
    let source = r#"
import publisher as authority

@provider_operation(authority.audit_write)
def append_audit(value: int) -> int:
  return value
"#;
    let consumer = parse(source)?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["consumer".to_string()]));
    checker
        .check_with_imports(&consumer, &[("publisher", &provider)])
        .map_err(|errors| format!("qualified provider operation should typecheck: {errors:?}"))?;

    let identity = checker
        .type_info()
        .resolved_identity(nth_span(source, "authority.audit_write", 0)?)
        .ok_or("qualified provider-operation argument has no identity")?;
    assert_eq!(identity.origin, SymbolOrigin::Module(vec!["publisher".to_string()]));
    assert_eq!(identity.declaration_name, "audit_write");
    assert_eq!(identity.kind, SemanticSourceTargetKind::Capability);
    Ok(())
}
