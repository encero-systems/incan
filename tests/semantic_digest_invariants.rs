//! Behavioural invariants for the semantic digest.
//!
//! These are the acceptance contract from RFC 106, asserted at **declaration granularity**. That granularity is
//! deliberate and was learned the hard way: an implementation leaking traversal order still moves the digest of the
//! declaration actually edited, so a test asserting only "the module digest changed" passes a broken digest. Only
//! the count of *untouched* declarations that also moved exposes it.
//!
//! Two fixture conditions are mandatory, and a fixture omitting them is untested rather than passing:
//!
//! - every fixture keeps a declaration positioned **after** the edit, because positional leakage contaminates only what
//!   is traversed later;
//! - that trailing declaration owns a **nested scope**, because a declaration with no scope discriminant cannot detect
//!   traversal-order renumbering;
//! - a body-change fixture additionally **introduces a scope**, since an edit leaving the scope count unchanged shifts
//!   no later index and cannot detect the leak at all.

use incan::frontend::body_ir::{apply_body_ir_input_contract, build_body_ir_module_v0};
use incan::frontend::hir::build_hir_v0;
use incan::frontend::typechecker::TypeChecker;
use incan::frontend::{lexer, parser};
use incan_semantics_core::CanonicalSymbolId;
use incan_semantics_core::body_ir::Body;
use incan_semantics_core::semantic_digest::{body_without_docstring, semantic_digest};
use incan_semantics_core::stable_identity::{DeclarationSignature, StableDeclarationId};
use std::collections::BTreeMap;
use std::path::Path;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// The before-and-after digests of one fixture pair, keyed by declaration identity.
type DigestPair = (BTreeMap<String, String>, BTreeMap<String, String>);

/// Digest every declaration in one module, keyed by its edit-stable identity.
fn digest_declarations(source: &str) -> Result<BTreeMap<String, String>, String> {
    let tokens = lexer::lex(source).map_err(|errors| format!("lex: {errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("parse: {errors:?}"))?;
    let program =
        apply_body_ir_input_contract(program, Path::new("fixture.incn")).map_err(|errors| format!("{errors:?}"))?;
    let module_path = vec!["fixture".to_string()];
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(module_path.clone()));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;

    let hir = build_hir_v0(&program, &module_path, checker.type_info());
    let body_ir = build_body_ir_module_v0(&program, &module_path, checker.type_info());

    let mut bodies: BTreeMap<CanonicalSymbolId, &Body> = BTreeMap::new();
    for body in &body_ir.bodies {
        if let Some(canonical) = body.canonical.as_ref() {
            bodies.insert(canonical.clone(), body);
        }
    }

    let mut digests = BTreeMap::new();
    for declaration in &hir.declarations {
        let Some(canonical) = declaration.canonical.as_ref() else {
            continue;
        };
        let body = bodies.get(canonical).copied();
        let signature = body.map(|body| {
            DeclarationSignature::from_callable_types(body.params.iter().map(|param| &param.ty), &body.return_type)
        });
        let identity = StableDeclarationId::from_canonical(canonical, signature);
        // Digest the declaration record and its body together, both from checked values.
        let declaration_digest = semantic_digest(&(declaration.kind, &declaration.name, declaration.visibility))
            .map_err(|error| error.to_string())?;
        let body_digest = match body {
            Some(body) => semantic_digest(&body_without_docstring(body)).map_err(|error| error.to_string())?,
            None => "sha256:none".to_string(),
        };
        digests.insert(
            identity.render_compact(),
            semantic_digest(&(declaration_digest, body_digest)).map_err(|error| error.to_string())?,
        );
    }
    Ok(digests)
}

fn digest_pair(before: &str, after: &str) -> Result<DigestPair, Box<dyn std::error::Error>> {
    let before = before.to_string();
    let after = after.to_string();
    incan::compiler_stack::run_on_compiler_stack(move || {
        Ok::<_, String>((digest_declarations(&before)?, digest_declarations(&after)?))
    })
    .map_err(Box::<dyn std::error::Error>::from)
}

/// Assert not one declaration moved.
fn assert_nothing_moved(before: &str, after: &str) -> TestResult {
    let (before, after) = digest_pair(before, after)?;
    let moved: Vec<_> = after
        .iter()
        .filter(|(key, digest)| before.get(*key).map(|old| old != *digest).unwrap_or(true))
        .map(|(key, _)| key.as_str())
        .collect();
    if !moved.is_empty() {
        return Err(format!("expected nothing to move, these did: {moved:?}").into());
    }
    if before.len() != after.len() {
        return Err(format!("declaration count changed: {} -> {}", before.len(), after.len()).into());
    }
    Ok(())
}

/// Assert exactly the named declarations moved and no others.
fn assert_only_these_moved(before: &str, after: &str, expected: &[&str]) -> TestResult {
    let (before, after) = digest_pair(before, after)?;
    let moved: Vec<String> = after
        .iter()
        .filter(|(key, digest)| before.get(*key).map(|old| old != *digest).unwrap_or(false))
        .map(|(key, _)| key.clone())
        .collect();
    let unexpected: Vec<_> = moved
        .iter()
        .filter(|m| !expected.iter().any(|e| m.contains(e)))
        .collect();
    let missing: Vec<_> = expected
        .iter()
        .filter(|e| !moved.iter().any(|m| m.contains(*e)))
        .collect();
    if !unexpected.is_empty() {
        return Err(format!("untouched declarations moved: {unexpected:?}").into());
    }
    if !missing.is_empty() {
        return Err(format!("expected to move but did not: {missing:?}").into());
    }
    Ok(())
}

/// `tail_helper` sits after every edit and owns a nested scope, so traversal-order leakage is detectable.
const BASE: &str = r#"
def head_helper(value: int) -> int:
    return value + 1


def edited(value: int) -> int:
    return value * 2


def tail_helper(value: int) -> int:
    if value > 0:
        inner = value + 1
        return inner
    other = value - 1
    return other
"#;

#[test]
fn adding_documentation_moves_nothing() -> TestResult {
    let after = BASE.replace(
        "def edited(value: int) -> int:\n    return value * 2",
        "def edited(value: int) -> int:\n    \"\"\"\n    Added documentation.\n    \"\"\"\n    return value * 2",
    );
    assert_nothing_moved(BASE, &after)
}

#[test]
fn removing_documentation_moves_nothing() -> TestResult {
    let documented = BASE.replace(
        "def edited(value: int) -> int:\n    return value * 2",
        "def edited(value: int) -> int:\n    \"\"\"\n    Present here, absent in the pair.\n    \"\"\"\n    return value * 2",
    );
    assert_nothing_moved(&documented, BASE)
}

#[test]
fn adding_a_comment_moves_nothing() -> TestResult {
    let after = BASE.replace("def edited", "# a standalone comment\ndef edited");
    assert_nothing_moved(BASE, &after)
}

#[test]
fn reordering_declarations_moves_nothing() -> TestResult {
    let after = r#"
def edited(value: int) -> int:
    return value * 2


def head_helper(value: int) -> int:
    return value + 1


def tail_helper(value: int) -> int:
    if value > 0:
        inner = value + 1
        return inner
    other = value - 1
    return other
"#;
    assert_nothing_moved(BASE, after)
}

#[test]
fn reformatting_moves_nothing() -> TestResult {
    let after = BASE.replace(
        "    if value > 0:\n        inner = value + 1",
        "    if value > 0:\n\n        inner = value + 1",
    );
    assert_nothing_moved(BASE, &after)
}

#[test]
fn inserting_a_declaration_moves_no_existing_declaration() -> TestResult {
    let after = BASE.replace(
        "def edited",
        "def inserted_helper(value: int) -> int:\n    return value\n\n\ndef edited",
    );
    assert_only_these_moved(BASE, &after, &[])
}

#[test]
fn changing_a_body_moves_only_that_declaration() -> TestResult {
    // The edit introduces a scope. A body change that adds none shifts no later scope index and therefore cannot
    // detect traversal-order leakage at all.
    let after = BASE.replace(
        "def edited(value: int) -> int:\n    return value * 2",
        "def edited(value: int) -> int:\n    if value > 1:\n        doubled = value * 2\n        return doubled\n    return value * 4",
    );
    assert_only_these_moved(BASE, &after, &["edited"])
}

/// A signature change replaces a declaration's identity rather than moving its digest.
///
/// The signature is part of the identity, because without it overloads collide — a corpus of one standard library
/// has two such pairs. The consequence is that changing a signature is a *delete plus an add*, not a modification:
/// the old identity disappears and a new one appears.
///
/// For invalidation that is exactly right; both are "changed". For a consumer tracing one declaration across
/// versions it is a real limitation, and it is the price of separating overloads. This test pins the behaviour so
/// the trade-off is visible rather than discovered.
#[test]
fn changing_a_signature_replaces_the_identity_and_leaves_siblings_alone() -> TestResult {
    let after = BASE.replace(
        "def edited(value: int) -> int:\n    return value * 2",
        "def edited(value: int) -> str:\n    return \"changed\"",
    );
    let (before, after) = digest_pair(BASE, &after)?;

    let gone: Vec<_> = before.keys().filter(|key| !after.contains_key(*key)).collect();
    let appeared: Vec<_> = after.keys().filter(|key| !before.contains_key(*key)).collect();
    assert_eq!(gone.len(), 1, "exactly one identity should disappear, got {gone:?}");
    assert_eq!(
        appeared.len(),
        1,
        "exactly one identity should appear, got {appeared:?}"
    );
    assert!(
        gone[0].contains("edited") && appeared[0].contains("edited"),
        "{gone:?} -> {appeared:?}"
    );

    // Every declaration that kept its identity must also have kept its digest.
    let moved: Vec<_> = after
        .iter()
        .filter(|(key, digest)| before.get(*key).map(|old| old != *digest).unwrap_or(false))
        .map(|(key, _)| key.as_str())
        .collect();
    assert!(moved.is_empty(), "siblings must be untouched, these moved: {moved:?}");
    Ok(())
}

/// A digest that is not deterministic for identical input is not a digest.
#[test]
fn digest_is_deterministic_for_identical_source() -> TestResult {
    let (first, second) = digest_pair(BASE, BASE)?;
    let differing: Vec<_> = first
        .iter()
        .filter(|(key, digest)| second.get(*key) != Some(*digest))
        .map(|(key, _)| key.as_str())
        .collect();
    if !differing.is_empty() {
        return Err(format!("identical source produced different digests for: {differing:?}").into());
    }
    Ok(())
}
