//! RFC 120 conformance: canonical symbol identity at declaration sites and on resolved references.
//!
//! These tests pin the identity contract itself rather than any consumer: one compiler-owned identity is minted at
//! each declaration site, an import/alias/re-export binding carries its *target's* identity, same-spelled bindings
//! in different scopes stay distinct, and reference-side recording answers "do these two references mean the same
//! thing" structurally. Body IR's consumption of these facts is pinned separately in
//! `crate::frontend::body_ir::tests`.

use incan_core::lang::surface::constructors::{self, ConstructorId};
use incan_core::lang::traits::{self, TraitId};
use incan_semantics_core::{CanonicalSymbolId, SemanticSourceTargetKind, SymbolNamespace, SymbolOrigin};

use super::{CompileError, TypeChecker};
use crate::frontend::ast::{Declaration, Program, Span};
use crate::frontend::symbols::{SymbolKind, TypeInfo};
use crate::frontend::{lexer, parser};
use crate::provider::ProviderPlan;
use std::sync::Arc;

/// Parse one test program and preserve fixture failures as ordinary test errors.
fn parse(source: &str, context: &str) -> Result<Program, String> {
    let tokens = lexer::lex(source).map_err(|errors| format!("{context} lex failed: {errors:?}"))?;
    parser::parse(&tokens).map_err(|errors| format!("{context} parse failed: {errors:?}"))
}

/// Check one standalone program and return the checker for identity inspection.
fn check(source: &str, context: &str) -> Result<TypeChecker, String> {
    let program = parse(source, context)?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["conformance".to_string()]));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{context} should typecheck: {errors:?}"))?;
    Ok(checker)
}

/// Run a program that is expected to fail and return its structured diagnostics without a panic-based extractor.
fn check_errors(checker: &mut TypeChecker, program: &Program, context: &str) -> Result<Vec<CompileError>, String> {
    match checker.check_program(program) {
        Ok(()) => Err(format!("{context}: program unexpectedly typechecked")),
        Err(errors) => Ok(errors),
    }
}

/// Return the span of the `occurrence`-th appearance (0-based) of `needle` in `source`.
fn nth_span(source: &str, needle: &str, occurrence: usize) -> Result<Span, String> {
    source
        .match_indices(needle)
        .nth(occurrence)
        .map(|(start, matched)| Span::new(start, start + matched.len()))
        .ok_or_else(|| format!("occurrence {occurrence} of `{needle}` not found"))
}

/// Return the recorded reference identity at `span`, or an error naming the missing case.
fn identity_at(checker: &TypeChecker, span: Span, context: &str) -> Result<CanonicalSymbolId, String> {
    checker.type_info().resolved_identity(span).cloned().ok_or_else(|| {
        format!(
            "{context}: no resolved identity recorded at {}..{}",
            span.start, span.end
        )
    })
}

/// Return the canonical declaration selected for one source binding write.
fn write_identity_at(
    checker: &TypeChecker,
    span: Span,
    name: &str,
    context: &str,
) -> Result<CanonicalSymbolId, String> {
    checker
        .type_info()
        .resolved_write_identity(span, name)
        .cloned()
        .ok_or_else(|| {
            format!(
                "{context}: no resolved write identity recorded for `{name}` at {}..{}",
                span.start, span.end
            )
        })
}

/// Imported trait defaults retain their declaration identity for lowering into a local adopter.
#[test]
fn imported_trait_default_method_identity_survives_for_lowering() -> Result<(), String> {
    let checker = check(
        r#"
from std.traits.error import Error

model LocalError with Error:
  detail: str

  def message(self) -> str:
    return self.detail
"#,
        "imported trait default identity",
    )?;

    let identity = checker
        .type_info()
        .traits
        .method_identities
        .get(&(traits::as_str(TraitId::Error).to_string(), "source".to_string()))
        .ok_or_else(|| {
            format!(
                "missing Error.source identity in lowering artifacts: {:?}",
                checker.type_info().traits.method_identities
            )
        })?;
    assert_eq!(identity.declaration_name, "source");
    assert_eq!(identity.kind, SemanticSourceTargetKind::Method);
    assert_eq!(
        identity.origin,
        SymbolOrigin::Module(vec!["std".to_string(), "traits".to_string(), "error".to_string(),])
    );
    Ok(())
}

/// Dependency-only trait metadata uses its canonical module-qualified key when no local symbol carries the trait.
#[test]
fn dependency_trait_default_method_identity_survives_private_module_checking() -> Result<(), String> {
    let provider = parse(
        r#"
pub trait Contract:
  def required(self) -> str: ...

  def fallback(self) -> str:
    return "fallback"
"#,
        "dependency trait provider",
    )?;
    let consumer = parse(
        r#"
from provider import Contract

model Implementation with Contract:
  def required(self) -> str:
    return "implemented"
"#,
        "dependency trait consumer",
    )?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["consumer".to_string()]));
    checker
        .check_with_imports_allow_private(&consumer, &[("provider", &provider)])
        .map_err(|errors| format!("dependency trait consumer should typecheck: {errors:?}"))?;

    let identity = checker
        .type_info()
        .traits
        .method_identities
        .get(&("provider.Contract".to_string(), "fallback".to_string()))
        .ok_or_else(|| {
            format!(
                "missing provider.Contract.fallback identity in lowering artifacts: {:?}",
                checker.type_info().traits.method_identities
            )
        })?;
    assert_eq!(identity.declaration_name, "fallback");
    assert_eq!(identity.kind, SemanticSourceTargetKind::Method);
    assert_eq!(identity.origin, SymbolOrigin::Module(vec!["provider".to_string()]));
    let adopted_identity = checker
        .type_info()
        .traits
        .method_identities
        .get(&("Contract".to_string(), "fallback".to_string()))
        .ok_or_else(|| "missing locally-adopted Contract.fallback identity in lowering artifacts".to_string())?;
    assert_eq!(adopted_identity, identity);
    Ok(())
}

/// A module-level declaration's identity is minted once and is independent of how often it is referenced.
#[test]
fn module_declaration_identity_is_reference_independent() -> Result<(), String> {
    let source = r#"
def helper() -> int:
  return 1

def first() -> int:
  value = helper
  return 1

def second() -> int:
  again = helper
  return 2
"#;
    let checker = check(source, "reference independence")?;
    let first_ref = identity_at(&checker, nth_span(source, "helper", 1)?, "first reference")?;
    let second_ref = identity_at(&checker, nth_span(source, "helper", 2)?, "second reference")?;
    assert_eq!(first_ref, second_ref, "two references must record one identity");
    assert_eq!(first_ref.kind, SemanticSourceTargetKind::Function);
    assert_eq!(first_ref.declaration_name, "helper");
    assert_eq!(
        first_ref.origin,
        SymbolOrigin::Module(vec!["conformance".to_string()]),
        "a module declaration is owned by its module"
    );
    assert_eq!(
        first_ref.scope_discriminant, None,
        "module-level declarations are module-unique and carry no discriminant"
    );

    let declaration = checker
        .type_info()
        .declarations
        .declaration_identities
        .values()
        .find(|identity| identity.declaration_name == "helper")
        .ok_or("declaration identity for `helper` must be exported")?;
    assert_eq!(
        declaration, &first_ref,
        "references resolve to the declaration's identity"
    );
    Ok(())
}

/// Two same-spelled bindings in sibling blocks are different declarations with different identities.
#[test]
fn sibling_block_locals_get_distinct_identities() -> Result<(), String> {
    let source = r#"
def run() -> None:
  if true:
    left = 1
    _ = left
  if true:
    left = 2
    _ = left
"#;
    let checker = check(source, "sibling blocks")?;
    // Occurrences: 0 = first binding, 1 = first reference, 2 = second binding, 3 = second reference.
    let first = identity_at(&checker, nth_span(source, "left", 1)?, "first block reference")?;
    let second = identity_at(&checker, nth_span(source, "left", 3)?, "second block reference")?;
    assert_eq!(first.kind, SemanticSourceTargetKind::Local);
    assert_eq!(second.kind, SemanticSourceTargetKind::Local);
    assert_ne!(
        first, second,
        "same-spelled locals in sibling blocks must not collapse to one identity"
    );
    assert_ne!(
        first.scope_discriminant, second.scope_discriminant,
        "sibling blocks are different scopes, so the discriminants must differ"
    );
    Ok(())
}

/// `let` introduces a new binding with a fresh identity over an active outer binding; the outer binding's identity
/// is unchanged and visible again after the block.
#[test]
fn let_shadowing_mints_a_new_identity_and_restores_the_outer_one() -> Result<(), String> {
    let source = r#"
def run() -> None:
  mut shade = 1
  first = shade
  if true:
    let shade = 2
    second = shade
  third = shade
"#;
    let checker = check(source, "let shadowing")?;
    // Occurrences: 0 = outer binding, 1 = outer reference, 2 = `let` binding, 3 = shadowed reference,
    // 4 = post-block reference.
    let outer = identity_at(&checker, nth_span(source, "shade", 1)?, "outer reference")?;
    let shadowed = identity_at(&checker, nth_span(source, "shade", 3)?, "shadowed reference")?;
    let restored = identity_at(&checker, nth_span(source, "shade", 4)?, "post-block reference")?;
    assert_ne!(outer, shadowed, "`let` must mint a fresh identity for the new binding");
    assert_eq!(
        outer, restored,
        "the outer binding's identity is visible again after the block"
    );
    Ok(())
}

/// `mut` is also an explicit binding form: it mints a mutable inner declaration and restores the outer identity on
/// scope exit rather than changing which declaration the outer spelling names.
#[test]
fn mut_shadowing_mints_a_new_identity_and_restores_the_outer_one() -> Result<(), String> {
    let source = r#"
def run() -> None:
  mut shade = 1
  first = shade
  if true:
    mut shade = 2
    second = shade
  third = shade
"#;
    let checker = check(source, "mut shadowing")?;
    let outer = identity_at(&checker, nth_span(source, "shade", 1)?, "outer reference")?;
    let shadowed = identity_at(&checker, nth_span(source, "shade", 3)?, "shadowed reference")?;
    let restored = identity_at(&checker, nth_span(source, "shade", 4)?, "post-block reference")?;
    assert_ne!(outer, shadowed, "`mut` must mint a fresh identity for the new binding");
    assert_eq!(outer, restored, "the outer identity must be restored after the block");
    Ok(())
}

/// Plain assignment inside a nested block reassigns the outer binding: later references still carry the outer
/// declaration's identity, not a new one.
#[test]
fn plain_assignment_preserves_the_target_binding_identity() -> Result<(), String> {
    let source = r#"
def run() -> None:
  mut total = 1
  first = total
  if true:
    total = 2
  second = total
"#;
    let checker = check(source, "plain reassignment")?;
    let before = identity_at(&checker, nth_span(source, "total", 1)?, "reference before block")?;
    let after = identity_at(&checker, nth_span(source, "total", 3)?, "reference after block")?;
    assert_eq!(
        before, after,
        "plain assignment reassigns the active binding and must not change its identity"
    );
    Ok(())
}

/// Every assignment form records the selected declaration at the exact authored target span, including multiple
/// targets in one statement.
#[test]
fn assignment_forms_record_exact_distinct_target_spans() -> Result<(), String> {
    let source = r#"
def run() -> None:
  let single = 1
  mut counter = 0
  counter += 1
  let left, right = (2, 3)
  let first = second = 4
  mut swap_left = 5
  mut swap_right = 6
  swap_left, swap_right = (swap_right, swap_left)
"#;
    let checker = check(source, "assignment target spans")?;

    for (name, occurrence, context) in [
        ("single", 0, "single declaration"),
        ("counter", 0, "compound declaration"),
        ("counter", 1, "compound write"),
        ("left", 0, "tuple-unpack left target"),
        ("right", 0, "tuple-unpack right target"),
        ("first", 0, "chained first target"),
        ("second", 0, "chained second target"),
        ("swap_left", 1, "tuple assignment left target"),
        ("swap_right", 1, "tuple assignment right target"),
    ] {
        let span = nth_span(source, name, occurrence)?;
        let identity = write_identity_at(&checker, span, name, context)?;
        if occurrence == 0 {
            assert_eq!(
                identity.declaration_span,
                incan_semantics_core::HirSourceSpan::new(span.start, span.end),
                "{context} must be anchored to its exact target"
            );
        }
    }

    let left = write_identity_at(&checker, nth_span(source, "left", 0)?, "left", "tuple-unpack left")?;
    let right = write_identity_at(&checker, nth_span(source, "right", 0)?, "right", "tuple-unpack right")?;
    assert_ne!(
        left, right,
        "tuple-unpack targets must not collapse onto the statement span"
    );

    let first = write_identity_at(&checker, nth_span(source, "first", 0)?, "first", "chained first")?;
    let second = write_identity_at(&checker, nth_span(source, "second", 0)?, "second", "chained second")?;
    assert_ne!(
        first, second,
        "chained targets must not collapse onto the statement span"
    );
    Ok(())
}

/// Local nominal annotations resolve to the declaration identity rather than a spelling-derived type token.
#[test]
fn local_type_annotations_record_the_nominal_identity() -> Result<(), String> {
    let source = r#"
model Record:
  value: int

def keep(value: Record) -> Record:
  return value
"#;
    let checker = check(source, "local type annotations")?;
    let parameter = identity_at(&checker, nth_span(source, "Record", 1)?, "parameter annotation")?;
    let returned = identity_at(&checker, nth_span(source, "Record", 2)?, "return annotation")?;
    let declaration = checker
        .type_info()
        .declarations
        .declaration_identities
        .values()
        .find(|identity| identity.declaration_name == "Record")
        .ok_or("Record declaration identity must be exported")?;
    assert_eq!(&parameter, declaration);
    assert_eq!(returned, parameter);
    Ok(())
}

/// Each constructor and argument in a nested generic annotation keeps its own reference span and identity.
#[test]
fn nested_generic_type_annotations_record_every_resolved_reference() -> Result<(), String> {
    let source = r#"
model Item:
  value: int

def consume(value: list[dict[str, Item]]) -> None:
  pass
"#;
    let checker = check(source, "nested generic type annotations")?;
    let expected = [
        ("list[dict[str, Item]]", 0, "List"),
        ("dict[str, Item]", 0, "Dict"),
        ("str", 0, "str"),
        ("Item", 1, "Item"),
    ];
    for (source_reference, occurrence, declaration_name) in expected {
        let identity = identity_at(
            &checker,
            nth_span(source, source_reference, occurrence)?,
            &format!("nested `{source_reference}` reference"),
        )?;
        assert_eq!(
            identity.declaration_name, declaration_name,
            "`{source_reference}` must resolve independently"
        );
    }
    Ok(())
}

/// A generic binder has its own identity, scoped to the declaration that introduces it, distinct from any
/// same-spelled concrete type and from another declaration's binder.
#[test]
fn generic_binder_identity_is_declaration_scoped() -> Result<(), String> {
    let source = r#"
model Holder:
  value: int

def wrap[T](value: T) -> T:
  return value

def echo[T](value: T) -> T:
  return value
"#;
    let checker = check(source, "generic binders")?;
    let binder_identities: Vec<CanonicalSymbolId> = checker
        .symbols
        .all_symbols()
        .iter()
        .enumerate()
        .filter(|(_, symbol)| symbol.name == "T")
        .filter_map(|(id, _)| checker.symbols.identity_of(id).cloned())
        .filter(|identity| identity.kind == SemanticSourceTargetKind::GenericBinder)
        .collect();
    assert!(
        binder_identities.len() >= 2,
        "both binder declarations must carry GenericBinder identities, got {binder_identities:?}"
    );
    assert_ne!(
        binder_identities[0], binder_identities[1],
        "two declarations' binders are distinct declarations"
    );
    for binder in &binder_identities {
        assert!(
            binder.scope_discriminant.is_some(),
            "a binder is bounded to its declaration's scope, so it must carry a discriminant"
        );
    }

    let holder = checker
        .type_info()
        .declarations
        .declaration_identities
        .values()
        .find(|identity| identity.declaration_name == "Holder")
        .ok_or("model declaration identity must be exported")?;
    assert_eq!(holder.kind, SemanticSourceTargetKind::Model);
    assert!(
        binder_identities.iter().all(|binder| binder != holder),
        "a binder never compares equal to a concrete type declaration"
    );

    let wrap_parameter = identity_at(&checker, nth_span(source, "T", 1)?, "wrap parameter annotation")?;
    let wrap_return = identity_at(&checker, nth_span(source, "T", 2)?, "wrap return annotation")?;
    let echo_parameter = identity_at(&checker, nth_span(source, "T", 4)?, "echo parameter annotation")?;
    let echo_return = identity_at(&checker, nth_span(source, "T", 5)?, "echo return annotation")?;
    assert_eq!(
        wrap_parameter, wrap_return,
        "one function's annotations share its binder"
    );
    assert_eq!(
        echo_parameter, echo_return,
        "one function's annotations share its binder"
    );
    assert_ne!(wrap_parameter, echo_parameter, "each function owns a distinct binder");
    Ok(())
}

/// `Self` annotations name their enclosing nominal or trait declaration.
#[test]
fn concrete_self_type_annotation_records_the_nominal_identity() -> Result<(), String> {
    let source = r#"
model Node:
  value: int

  def duplicate(self) -> Self:
    return self
"#;
    let checker = check(source, "concrete Self annotation")?;
    let self_identity = identity_at(&checker, nth_span(source, "Self", 0)?, "concrete Self annotation")?;
    let node_identity = checker
        .type_info()
        .declarations
        .declaration_identities
        .values()
        .find(|identity| identity.declaration_name == "Node")
        .ok_or("Node declaration identity must be exported")?;
    assert_eq!(&self_identity, node_identity);

    let trait_source = "trait Cloneable:\n  def duplicate(self) -> Self: ...\n";
    let trait_checker = check(trait_source, "abstract trait Self annotation")?;
    let cloneable_identity = trait_checker
        .type_info()
        .declarations
        .declaration_identities
        .values()
        .find(|identity| identity.declaration_name == "Cloneable")
        .ok_or("Cloneable declaration identity must be exported")?;
    assert_eq!(
        trait_checker
            .type_info()
            .resolved_identity(nth_span(trait_source, "Self", 0)?),
        Some(cloneable_identity),
        "trait Self must retain its enclosing trait declaration"
    );
    Ok(())
}

/// Duplicate generic binders retain their exact declaration-token spans and therefore remain distinct candidates in
/// the shared collision diagnostic.
#[test]
fn duplicate_generic_binders_have_distinct_declaration_site_identities() -> Result<(), String> {
    let source = "def choose[T, T](value: T) -> T:\n  return value\n";
    let first = nth_span(source, "T", 0)?;
    let second = nth_span(source, "T", 1)?;
    let program = parse(source, "duplicate generic binders")?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["conformance".to_string()]));
    let errors = match checker.check_program(&program) {
        Ok(()) => return Err("duplicate generic binders were accepted".to_string()),
        Err(errors) => errors,
    };
    let duplicate = errors
        .iter()
        .find(|error| error.message == "Duplicate definition of 'T'")
        .ok_or_else(|| format!("missing duplicate-binder diagnostic: {errors:?}"))?;
    assert_eq!(duplicate.span, second, "the later binder must be the primary span");
    assert_eq!(
        duplicate.related_spans().first().map(|related| related.span),
        Some(first),
        "the first binder must be retained as the related declaration site"
    );
    assert_eq!(duplicate.notes.len(), 2, "both binder identities must be named");
    assert_ne!(
        duplicate.notes[0].replace("First canonical identity: ", ""),
        duplicate.notes[1].replace("Second canonical identity: ", ""),
        "the two binder declarations must never collapse to one canonical identity"
    );
    Ok(())
}

/// Parameters and receivers carry their own declaration categories, and a parameter's identity differs from a
/// same-spelled local in another scope.
#[test]
fn parameter_and_receiver_identities_carry_their_categories() -> Result<(), String> {
    let source = r#"
class Greeter:
  name: str

  def greet(self, message: str) -> str:
    observed = self
    return message
"#;
    let checker = check(source, "parameters and receivers")?;
    let message = identity_at(&checker, nth_span(source, "message", 1)?, "parameter reference")?;
    assert_eq!(message.kind, SemanticSourceTargetKind::Parameter);
    assert!(message.scope_discriminant.is_some(), "parameters are scope-bounded");

    let receiver = checker
        .symbols
        .all_symbols()
        .iter()
        .enumerate()
        .filter(|(_, symbol)| symbol.name == "self")
        .filter_map(|(id, _)| checker.symbols.identity_of(id))
        .find(|identity| identity.kind == SemanticSourceTargetKind::Receiver)
        .ok_or("the receiver binding must carry a Receiver-kind identity")?;
    assert!(receiver.scope_discriminant.is_some(), "receivers are scope-bounded");
    assert_eq!(
        receiver.declaration_span,
        incan_semantics_core::HirSourceSpan::new(nth_span(source, "self", 0)?.start, nth_span(source, "self", 0)?.end,),
        "the receiver identity must retain the exact declaration token"
    );
    assert_eq!(
        &identity_at(&checker, nth_span(source, "self", 1)?, "self reference")?,
        receiver,
        "a self expression must record the receiver declaration identity"
    );
    Ok(())
}

/// A classmethod `cls` binding has exact declaration provenance and calls through that binding record its identity.
#[test]
fn class_receiver_identity_is_recorded_at_declaration_and_use() -> Result<(), String> {
    let source = r#"
model Token:
  value: int

  @classmethod
  def create(cls, value: int) -> Self:
    return cls(value=value)
"#;
    let checker = check(source, "class receiver identity")?;
    let declaration_span = nth_span(source, "cls", 0)?;
    let receiver = checker
        .symbols
        .all_symbols()
        .iter()
        .enumerate()
        .filter(|(_, symbol)| symbol.name == "cls")
        .filter_map(|(id, _)| checker.symbols.identity_of(id))
        .find(|identity| identity.kind == SemanticSourceTargetKind::Receiver)
        .ok_or("the cls binding must carry a Receiver-kind identity")?;
    assert_eq!(
        receiver.declaration_span,
        incan_semantics_core::HirSourceSpan::new(declaration_span.start, declaration_span.end)
    );
    assert_eq!(
        &identity_at(&checker, nth_span(source, "cls", 1)?, "cls call")?,
        receiver,
        "a cls constructor call must record the receiver identity"
    );
    Ok(())
}

/// A classmethod written with `self` has only the authored instance receiver; checking must not invent `cls`.
#[test]
fn classmethod_self_does_not_invent_a_cls_receiver() -> Result<(), String> {
    let source = r#"
model Token:
  value: int

  @classmethod
  def inspect(self) -> Self:
    return self
"#;
    let checker = check(source, "classmethod self receiver")?;
    let receiver = identity_at(&checker, nth_span(source, "self", 1)?, "self return")?;
    assert_eq!(receiver.kind, SemanticSourceTargetKind::Receiver);
    assert_eq!(receiver.declaration_name, "self");
    assert!(
        checker
            .symbols
            .all_symbols()
            .iter()
            .enumerate()
            .filter_map(|(id, _)| checker.symbols.identity_of(id))
            .all(|identity| {
                identity.kind != SemanticSourceTargetKind::Receiver || identity.declaration_name != "cls"
            }),
        "a self-authored classmethod must not gain a synthetic cls receiver"
    );
    Ok(())
}

/// An ordinary local named `cls` shadows the receiver and therefore disables class-constructor dispatch.
#[test]
fn local_cls_shadow_disables_classmethod_constructor_dispatch() -> Result<(), String> {
    let source = r#"
model Token:
  value: int

  @classmethod
  def create(cls, value: int) -> Self:
    if true:
      let cls = value
      shadowed = cls(value=value)
    return cls(value=value)
"#;
    let checker = check(source, "shadowed cls constructor")?;
    let shadowed = identity_at(&checker, nth_span(source, "cls", 2)?, "shadowed cls call")?;
    let restored = identity_at(&checker, nth_span(source, "cls", 3)?, "restored cls call")?;
    assert_eq!(shadowed.kind, SemanticSourceTargetKind::Local);
    assert_eq!(restored.kind, SemanticSourceTargetKind::Receiver);
    assert_ne!(
        shadowed, restored,
        "the inner local must disable receiver-constructor dispatch without replacing the outer receiver"
    );
    Ok(())
}

/// The class receiver remains call-only; recording its identity must not make bare `cls` a runtime value.
#[test]
fn bare_cls_preserves_the_prior_unknown_symbol_rejection() -> Result<(), String> {
    let source = r#"
model Token:
  value: int

  @classmethod
  def create(cls, value: int) -> Self:
    observed = cls
    return cls(value=value)
"#;
    let program = parse(source, "bare cls")?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["conformance".to_string()]));
    let errors = check_errors(&mut checker, &program, "bare cls must retain its prior rejection")?;
    let bare_span = nth_span(source, "cls", 1)?;
    assert!(
        errors
            .iter()
            .any(|error| error.span == bare_span && error.message.contains("Unknown symbol 'cls'")),
        "expected the bare cls rejection at its exact source span: {errors:?}"
    );
    assert!(
        checker
            .type_info()
            .resolved_identity(nth_span(source, "cls", 2)?)
            .is_some(),
        "the later constructor call must still record the class receiver identity"
    );
    Ok(())
}

/// A property's implicit receiver is anchored to the property declaration instead of the zero/default span.
#[test]
fn implicit_property_self_has_property_declaration_provenance() -> Result<(), String> {
    let source = r#"
model Token:
  value: int

  property observed -> int:
    return self.value
"#;
    let program = parse(source, "property receiver provenance")?;
    let property_span = match &program.declarations[0].node {
        Declaration::Model(model) => {
            model
                .properties
                .first()
                .ok_or("property declaration missing from fixture")?
                .span
        }
        other => return Err(format!("expected model declaration, got {other:?}")),
    };
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["conformance".to_string()]));
    checker
        .check_program(&program)
        .map_err(|errors| format!("property receiver provenance should typecheck: {errors:?}"))?;
    let receiver = identity_at(&checker, nth_span(source, "self", 0)?, "property self")?;
    assert_eq!(receiver.kind, SemanticSourceTargetKind::Receiver);
    assert_eq!(receiver.declaration_name, "self");
    assert_eq!(
        receiver.declaration_span,
        incan_semantics_core::HirSourceSpan::new(property_span.start, property_span.end),
        "the implicit receiver must use the enclosing property declaration as provenance"
    );
    assert_ne!(property_span, Span::default());
    Ok(())
}

/// A method declaration's identity lives in the member namespace; two owners' same-named methods stay distinct.
#[test]
fn member_method_identities_are_owner_distinct() -> Result<(), String> {
    let source = r#"
model First:
  value: int

  def describe(self) -> str:
    return "first"

model Second:
  value: int

  def describe(self) -> str:
    return "second"
"#;
    let checker = check(source, "member methods")?;
    let describe_identities: Vec<&CanonicalSymbolId> = checker
        .type_info()
        .declarations
        .method_bindings_by_span
        .values()
        .filter_map(|binding| binding.identity.as_ref())
        .filter(|identity| identity.declaration_name == "describe")
        .collect();
    assert_eq!(
        describe_identities.len(),
        2,
        "both method declarations must carry identities"
    );
    assert_eq!(describe_identities[0].namespace, SymbolNamespace::Member);
    assert_eq!(describe_identities[1].namespace, SymbolNamespace::Member);
    assert_eq!(describe_identities[0].kind, SemanticSourceTargetKind::Method);
    assert_ne!(
        describe_identities[0], describe_identities[1],
        "two owners' same-named methods are different declarations"
    );
    Ok(())
}

/// Declared fields, properties, and selected methods publish the same member identity at their use sites.
#[test]
fn member_accesses_retain_the_selected_declaration_identity() -> Result<(), String> {
    let source = r#"
model Account:
  cents: int

  property dollars -> int:
    return self.cents

  def total(self) -> int:
    return self.cents

def inspect(account: Account) -> int:
  raw = account.cents
  converted = account.dollars
  return account.total()
"#;
    let checker = check(source, "member reference identities")?;
    let account_id = checker.symbols.lookup("Account").ok_or("missing Account symbol")?;
    let account = checker.symbols.get(account_id).ok_or("missing Account metadata")?;
    let SymbolKind::Type(TypeInfo::Model(info)) = &account.kind else {
        return Err("Account should retain model metadata".to_string());
    };

    let field_identity = info
        .fields
        .get("cents")
        .and_then(|field| field.identity.as_ref())
        .ok_or("cents must carry a source member identity")?;
    let property_identity = info
        .properties
        .get("dollars")
        .and_then(|property| property.identity.as_ref())
        .ok_or("dollars must carry a source member identity")?;
    let method_identity = info
        .methods
        .get("total")
        .and_then(|method| method.identity.as_ref())
        .ok_or("total must carry a source member identity")?;

    assert_eq!(field_identity.kind, SemanticSourceTargetKind::Field);
    assert_eq!(property_identity.kind, SemanticSourceTargetKind::Property);
    assert_eq!(method_identity.kind, SemanticSourceTargetKind::Method);
    assert_eq!(
        checker
            .type_info()
            .resolved_identity(nth_span(source, "account.cents", 0)?),
        Some(field_identity)
    );
    assert_eq!(
        checker
            .type_info()
            .resolved_identity(nth_span(source, "account.dollars", 0)?),
        Some(property_identity)
    );
    assert_eq!(
        checker
            .type_info()
            .resolved_identity(nth_span(source, "account.total()", 0)?),
        Some(method_identity)
    );
    Ok(())
}

/// Method aliases and inherited member surfaces retain the declaration identity they project.
#[test]
fn aliased_and_inherited_method_accesses_preserve_origin_identity() -> Result<(), String> {
    let source = r#"
class Parent:
  value: int

  def read(self) -> int:
    return self.value

  fetch = read

class Child extends Parent:
  marker: int

def inspect(child: Child) -> int:
  first = child.fetch()
  return child.read()
"#;
    let checker = check(source, "aliased inherited member identities")?;
    let child_id = checker.symbols.lookup("Child").ok_or("missing Child symbol")?;
    let child = checker.symbols.get(child_id).ok_or("missing Child metadata")?;
    let SymbolKind::Type(TypeInfo::Class(info)) = &child.kind else {
        return Err("Child should retain class metadata".to_string());
    };
    let read_identity = info
        .methods
        .get("read")
        .and_then(|method| method.identity.as_ref())
        .ok_or("inherited read must retain its source identity")?;
    let fetch_identity = info
        .methods
        .get("fetch")
        .and_then(|method| method.identity.as_ref())
        .ok_or("method alias must retain its target identity")?;
    assert_eq!(fetch_identity, read_identity);
    assert_eq!(
        checker
            .type_info()
            .resolved_identity(nth_span(source, "child.fetch()", 0)?),
        Some(read_identity)
    );
    assert_eq!(
        checker
            .type_info()
            .resolved_identity(nth_span(source, "child.read()", 0)?),
        Some(read_identity)
    );
    assert_eq!(read_identity.declaration_name, "read");
    Ok(())
}

/// Overload resolution publishes only the declaration it actually selected; a diagnostic fallback is not a target.
#[test]
fn selected_method_overload_records_identity_but_no_viable_fallback_does_not() -> Result<(), String> {
    let source = r#"
trait Convert[T]:
  def convert(self) -> T: ...

model Converter with Convert[int], Convert[str]:
  def convert(self) -> int:
    return 1

  def convert(self) -> str:
    return "one"

def accepted(converter: Converter) -> int:
  return converter.convert()

def rejected(converter: Converter) -> bool:
  return converter.convert()
"#;
    let program = parse(source, "selected method overload identity")?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["conformance".to_string()]));
    let errors = check_errors(
        &mut checker,
        &program,
        "the bool return hint must leave the overload call unresolved",
    )?;
    assert!(
        errors.iter().any(|error| error.message.contains("convert")),
        "expected the invalid overload call to be diagnosed: {errors:?}"
    );

    let converter_id = checker.symbols.lookup("Converter").ok_or("missing Converter symbol")?;
    let converter = checker.symbols.get(converter_id).ok_or("missing Converter metadata")?;
    let SymbolKind::Type(TypeInfo::Model(info)) = &converter.kind else {
        return Err("Converter should retain model metadata".to_string());
    };
    let int_overload = info
        .method_overloads
        .get("convert")
        .and_then(|overloads| {
            overloads
                .iter()
                .find(|method| method.return_type == crate::frontend::symbols::ResolvedType::Int)
        })
        .and_then(|method| method.identity.as_ref())
        .ok_or("the selected int overload must carry an identity")?;
    assert_eq!(
        checker
            .type_info()
            .resolved_identity(nth_span(source, "converter.convert()", 0)?),
        Some(int_overload)
    );
    assert_eq!(
        checker
            .type_info()
            .resolved_identity(nth_span(source, "converter.convert()", 1)?),
        None,
        "the first candidate used only to produce diagnostics is not a resolved target"
    );
    Ok(())
}

/// An import, its alias, and a re-export are bindings to one declaration: every spelling records one identity.
#[test]
fn import_alias_and_reexport_share_the_declaration_identity() -> Result<(), String> {
    let lib = parse(
        r#"
pub def helper() -> int:
  return 1
"#,
        "identity lib",
    )?;
    let api = parse(
        r#"
pub from lib import helper as h
"#,
        "identity api facade",
    )?;
    let consumer_source = r#"
from lib import helper
from lib import helper as h
from api import h as run

def use_all() -> None:
  a = helper
  b = h
  c = run
"#;
    let consumer = parse(consumer_source, "identity consumer")?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["consumer".to_string()]));
    checker
        .check_with_imports(&consumer, &[("lib", &lib), ("api", &api)])
        .map_err(|errors| format!("identity consumer should typecheck: {errors:?}"))?;
    assert!(
        checker
            .dependency_member_reexports
            .get("api")
            .is_some_and(|exports| exports.contains_key("h")),
        "the facade must record `h` as an actual public re-export"
    );

    let direct = checker
        .type_info()
        .resolved_import_identity("helper")
        .ok_or("direct import must prove an identity")?
        .clone();
    let aliased = checker
        .type_info()
        .resolved_import_identity("h")
        .ok_or("aliased import must prove an identity")?
        .clone();
    let reexported = checker
        .type_info()
        .resolved_import_identity("run")
        .ok_or("re-exported import must prove an identity")?
        .clone();

    assert_eq!(direct, aliased, "an alias binds the same declaration");
    assert_eq!(
        direct, reexported,
        "a re-export resolves to the declaring module, never the facade"
    );
    assert_eq!(
        direct.declaration_name, "helper",
        "the declaration-site spelling survives every alias"
    );
    assert_eq!(direct.origin, SymbolOrigin::Module(vec!["lib".to_string()]));

    // Reference-side recording sees the same identity through every spelling.
    let helper_ref = identity_at(&checker, nth_span(consumer_source, "helper", 2)?, "direct reference")?;
    let h_ref = identity_at(
        &checker,
        nth_span(consumer_source, "b = h", 0).map(|span| Span::new(span.end - 1, span.end))?,
        "alias reference",
    )?;
    let run_ref = identity_at(&checker, nth_span(consumer_source, "run", 1)?, "re-export reference")?;
    assert_eq!(helper_ref, direct);
    assert_eq!(h_ref, direct);
    assert_eq!(run_ref, direct);
    Ok(())
}

/// Static imports keep the provider declaration identity even though statics are not codegraph source targets.
#[test]
fn static_import_alias_and_reexport_share_the_declaration_identity() -> Result<(), String> {
    let provider = parse("pub static counter: int = 1\n", "static identity provider")?;
    let facade = parse("pub from provider import counter\n", "static identity facade")?;
    let consumer_source = r#"
from facade import counter as shared

def read() -> int:
  return shared
"#;
    let consumer = parse(consumer_source, "static identity consumer")?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["consumer".to_string()]));
    checker
        .check_with_imports(&consumer, &[("provider", &provider), ("facade", &facade)])
        .map_err(|errors| format!("static identity consumer should typecheck: {errors:?}"))?;

    let imported = checker
        .type_info()
        .resolved_import_identity("shared")
        .ok_or("re-exported static import must prove an identity")?
        .clone();
    assert_eq!(imported.kind, SemanticSourceTargetKind::Static);
    assert_eq!(imported.declaration_name, "counter");
    assert_eq!(imported.origin, SymbolOrigin::Module(vec!["provider".to_string()]));
    assert_eq!(
        identity_at(
            &checker,
            nth_span(consumer_source, "shared", 1)?,
            "re-exported static read",
        )?,
        imported
    );
    Ok(())
}

/// An SDK component's own `std.*` spelling resolves to the exact physical source declaration covered by its
/// bootstrap grant. The public spelling remains a binding path; it must not mint a second provider identity.
#[test]
fn bootstrap_std_import_uses_physical_provider_source_identity() -> Result<(), String> {
    let registry = parse(
        r#"
pub model RegistrySubject:
  label: str

  @staticmethod
  def current_unit() -> Self:
    return RegistrySubject(label="unit")

pub class Registry:
  def entry(self) -> int:
    return 1
"#,
        "bootstrap registry provider",
    )?;
    let source = r#"
from std.registry import Registry, RegistrySubject

def read(registry: Registry) -> int:
  return registry.entry()

def subject() -> RegistrySubject:
  return RegistrySubject.current_unit()
"#;
    let consumer = parse(source, "bootstrap registry consumer")?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["features".to_string()]));
    checker.register_dependency_module_path_segments("registry", vec!["registry".to_string()]);
    checker.set_provider_plan(Arc::new(
        ProviderPlan::default().with_bootstrap_sdk_namespace_roots(["registry".to_string()]),
    ));
    checker
        .check_with_imports_allow_private(&consumer, &[("registry", &registry)])
        .map_err(|errors| format!("bootstrap registry consumer should typecheck: {errors:?}"))?;

    for imported_name in ["Registry", "RegistrySubject"] {
        let identity = checker
            .type_info()
            .resolved_import_identity(imported_name)
            .ok_or_else(|| format!("missing resolved import identity for {imported_name}"))?;
        assert_eq!(identity.origin, SymbolOrigin::Module(vec!["registry".to_string()]));
    }
    let entry = identity_at(
        &checker,
        nth_span(source, "registry.entry()", 0)?,
        "Registry.entry call",
    )?;
    assert_eq!(entry.declaration_name, "entry");
    assert_eq!(entry.kind, SemanticSourceTargetKind::Method);
    assert_eq!(entry.origin, SymbolOrigin::Module(vec!["registry".to_string()]));
    let current_unit = identity_at(
        &checker,
        nth_span(source, "RegistrySubject.current_unit()", 0)?,
        "RegistrySubject.current_unit call",
    )?;
    assert_eq!(current_unit.declaration_name, "current_unit");
    assert_eq!(current_unit.kind, SemanticSourceTargetKind::Method);
    assert_eq!(current_unit.origin, SymbolOrigin::Module(vec!["registry".to_string()]));
    Ok(())
}

/// Nested field method lookup during provider bootstrap must use the physical source dependency metadata too. A
/// direct import of `Date` would hide the stale-stub precedence bug this test guards against.
#[test]
fn bootstrap_nested_member_uses_physical_provider_source_identity() -> Result<(), String> {
    let naive = parse(
        r#"
pub model Date:
  day: int

  def add_months(self, months: int) -> Self:
    return self

pub model DateTime:
  pub date: Date
"#,
        "bootstrap datetime provider",
    )?;
    let source = r#"
from std.datetime.civil.naive import DateTime

def shift(value: DateTime) -> None:
  _ = value.date.add_months(1)
"#;
    let consumer = parse(source, "bootstrap datetime consumer")?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec![
        "datetime".to_string(),
        "civil".to_string(),
        "offset".to_string(),
    ]));
    checker.set_current_package_identity(Some("incan_stdlib_data".to_string()));
    checker.register_dependency_module_path_segments(
        "datetime_civil_naive",
        vec!["datetime".to_string(), "civil".to_string(), "naive".to_string()],
    );
    checker.set_provider_plan(Arc::new(
        ProviderPlan::default().with_bootstrap_sdk_namespace_roots(["datetime".to_string()]),
    ));
    checker
        .check_with_imports_allow_private(&consumer, &[("datetime_civil_naive", &naive)])
        .map_err(|errors| format!("bootstrap nested datetime consumer should typecheck: {errors:?}"))?;

    let add_months = identity_at(
        &checker,
        nth_span(source, "value.date.add_months(1)", 0)?,
        "nested Date.add_months call",
    )?;
    assert_eq!(add_months.declaration_name, "add_months");
    assert_eq!(add_months.kind, SemanticSourceTargetKind::Method);
    assert_eq!(
        add_months.origin,
        SymbolOrigin::Package {
            library: "incan_stdlib_data".to_string(),
            module_path: vec!["datetime".to_string(), "civil".to_string(), "naive".to_string()],
        }
    );
    Ok(())
}

/// Provider bootstrap resolution remains physical when a public `std.*` facade re-exports another source module from
/// the same component. Following the facade through its written public path must not mint a second member identity.
#[test]
fn bootstrap_std_facade_reexport_uses_physical_member_identity() -> Result<(), String> {
    let file = parse(
        r#"
pub class File:
  def flush(self) -> None:
    pass
"#,
        "bootstrap fs.file provider",
    )?;
    let facade = parse("from std.fs.file import File\n", "bootstrap fs facade")?;
    let source = r#"
from std.fs import File

def flush(file: File) -> None:
  file.flush()
"#;
    let consumer = parse(source, "bootstrap tempfile consumer")?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["tempfile".to_string()]));
    checker.register_dependency_module_path_segments("fs_file", vec!["fs".to_string(), "file".to_string()]);
    checker.register_dependency_module_path_segments("fs", vec!["fs".to_string()]);
    checker.set_provider_plan(Arc::new(
        ProviderPlan::default().with_bootstrap_sdk_namespace_roots(["fs".to_string()]),
    ));
    checker
        .check_with_imports_allow_private(&consumer, &[("fs_file", &file), ("fs", &facade)])
        .map_err(|errors| format!("bootstrap tempfile consumer should typecheck: {errors:?}"))?;

    let imported = checker
        .type_info()
        .resolved_import_identity("File")
        .ok_or("facade-imported File must prove its physical declaration identity")?;
    assert_eq!(
        imported.origin,
        SymbolOrigin::Module(vec!["fs".to_string(), "file".to_string()])
    );
    let flush = identity_at(&checker, nth_span(source, "file.flush()", 0)?, "File.flush call")?;
    assert_eq!(flush.declaration_name, "flush");
    assert_eq!(flush.kind, SemanticSourceTargetKind::Method);
    assert_eq!(
        flush.origin,
        SymbolOrigin::Module(vec!["fs".to_string(), "file".to_string()])
    );
    Ok(())
}

/// A dependency symbol without its compiler-retained canonical identity is unproven; lookup must fail closed rather
/// than reconstructing an identity from the dependency key, source spelling, kind, and span.
#[test]
fn dependency_member_identity_does_not_reconstruct_missing_canonical_data() -> Result<(), String> {
    let provider = parse(
        "pub def helper() -> int:\n  return 1\n",
        "fail-closed identity provider",
    )?;
    let consumer = parse(
        "from provider import helper\n\ndef run() -> int:\n  return helper()\n",
        "fail-closed identity consumer",
    )?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["consumer".to_string()]));
    checker.register_dependency_module_path_segments("provider", vec!["provider".to_string()]);
    checker
        .check_with_imports(&consumer, &[("provider", &provider)])
        .map_err(|errors| format!("fail-closed identity consumer should typecheck: {errors:?}"))?;

    let path = crate::frontend::ast::ImportPath::simple(vec!["provider".to_string()]);
    assert!(
        checker.dependency_member_identity(&path, "helper").is_some(),
        "the intact compiler-owned dependency cache must prove the declaration"
    );
    checker
        .dependency_direct_member_identities
        .get_mut("provider")
        .ok_or("provider identity cache must exist")?
        .remove("helper");
    assert_eq!(
        checker.dependency_member_identity(&path, "helper"),
        None,
        "missing canonical data must remain absent rather than being reconstructed"
    );
    Ok(())
}

/// Diagnostics keep the consumer spelling primary while carrying the imported declaration's own source identity.
#[test]
fn imported_alias_call_error_retains_original_declaration_location() -> Result<(), String> {
    let provider_source = r#"
pub def parse(value: int) -> int:
  return value
"#;
    let provider = parse(provider_source, "diagnostic provider")?;
    let consumer_source = r#"
from provider import parse as alias

def run() -> int:
  return alias("bad")
"#;
    let consumer = parse(consumer_source, "diagnostic consumer")?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["consumer".to_string()]));
    checker.register_dependency_module_path_segments("provider", vec!["provider".to_string()]);
    let errors = match checker.check_with_imports(&consumer, &[("provider", &provider)]) {
        Ok(()) => return Err("alias call with a str argument unexpectedly typechecked".to_string()),
        Err(errors) => errors,
    };
    let mismatch = errors
        .iter()
        .find(|error| error.message.contains("alias") && error.message.contains("Argument"))
        .ok_or_else(|| format!("missing alias argument diagnostic: {errors:?}"))?;
    assert_eq!(mismatch.span, nth_span(consumer_source, "\"bad\"", 0)?);
    let related = mismatch
        .related_declarations()
        .first()
        .ok_or("alias diagnostic must carry the provider declaration")?;
    assert_eq!(
        related.identity.origin,
        SymbolOrigin::Module(vec!["provider".to_string()])
    );
    assert_eq!(related.identity.declaration_name, "parse");
    assert_eq!(
        related.identity.declaration_span.start,
        provider_source
            .find("pub def parse")
            .ok_or("provider declaration missing")?
    );
    assert!(mismatch.related_spans().is_empty());
    Ok(())
}

/// Type annotations reached through an import alias and a re-export retain the original type declaration identity.
#[test]
fn imported_alias_and_reexport_type_annotations_share_the_declaration_identity() -> Result<(), String> {
    let provider = parse("pub model Item:\n  value: int\n", "type identity provider")?;
    let facade = parse("pub from provider import Item as PublicItem\n", "type identity facade")?;
    let consumer_source = r#"
from facade import PublicItem as LocalItem

def keep(value: LocalItem) -> LocalItem:
  return value
"#;
    let consumer = parse(consumer_source, "type identity consumer")?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["consumer".to_string()]));
    checker
        .check_with_imports(&consumer, &[("provider", &provider), ("facade", &facade)])
        .map_err(|errors| format!("type identity consumer should typecheck: {errors:?}"))?;

    let imported = checker
        .type_info()
        .resolved_import_identity("LocalItem")
        .ok_or("LocalItem import must prove its target identity")?
        .clone();
    let parameter = identity_at(
        &checker,
        nth_span(consumer_source, "LocalItem", 1)?,
        "imported parameter annotation",
    )?;
    let returned = identity_at(
        &checker,
        nth_span(consumer_source, "LocalItem", 2)?,
        "imported return annotation",
    )?;
    assert_eq!(parameter, imported);
    assert_eq!(returned, imported);
    assert_eq!(imported.declaration_name, "Item");
    assert_eq!(imported.origin, SymbolOrigin::Module(vec!["provider".to_string()]));
    Ok(())
}

/// Failed simple and qualified type resolution never manufactures a reference identity from source spelling.
#[test]
fn unresolved_and_qualified_type_annotations_do_not_fabricate_identities() -> Result<(), String> {
    let source = r#"
model Known:
  value: int

def invalid(first: MissingLeaf, second: Missing[Known], third: absent::Thing, fourth: absent.Thing) -> None:
  pass
"#;
    let program = parse(source, "unresolved type references")?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["conformance".to_string()]));
    if checker.check_program(&program).is_ok() {
        return Err("unknown type annotations unexpectedly typechecked".to_string());
    }

    for spelling in ["MissingLeaf", "Missing[Known]", "absent::Thing", "absent.Thing"] {
        assert_eq!(
            checker.type_info().resolved_identity(nth_span(source, spelling, 0)?),
            None,
            "`{spelling}` must not acquire a fabricated identity"
        );
    }
    let nested_known = identity_at(&checker, nth_span(source, "Known", 1)?, "known nested argument")?;
    assert_eq!(nested_known.declaration_name, "Known");
    Ok(())
}

/// Rebinding a core builtin-function spelling is not a collision, and the rebound declaration's identity differs
/// from the builtin registry identity (#1116's settled contract as an identity fact).
#[test]
fn rebound_builtin_spelling_and_registry_builtin_are_distinct_identities() -> Result<(), String> {
    let source = r#"
def len(value: int) -> int:
  return value + 1

def shadowed() -> int:
  return len(4)

def explicit(values: list[int]) -> int:
  return std.builtins.len(values)
"#;
    let checker = check(source, "builtin rebinding")?;

    let local_len = checker
        .type_info()
        .declarations
        .declaration_identities
        .values()
        .find(|identity| identity.declaration_name == "len")
        .ok_or("the local `len` declaration must carry an identity")?;
    assert_eq!(local_len.kind, SemanticSourceTargetKind::Function);
    assert_eq!(local_len.origin, SymbolOrigin::Module(vec!["conformance".to_string()]));

    let registry_len = checker
        .symbols
        .all_symbols()
        .iter()
        .enumerate()
        .filter(|(_, symbol)| symbol.name == "len")
        .filter_map(|(id, _)| checker.symbols.identity_of(id))
        .find(|identity| identity.origin == SymbolOrigin::Builtin)
        .ok_or("the builtin registry identity for `len` must still exist")?;
    assert_eq!(registry_len.kind, SemanticSourceTargetKind::Builtin);
    assert_ne!(
        registry_len, local_len,
        "the rebound spelling and the registry builtin are two different canonical identities"
    );
    let call_len = identity_at(&checker, nth_span(source, "len", 1)?, "shadowed len call")?;
    assert_eq!(
        &call_len, local_len,
        "the call must resolve to the active source declaration"
    );
    Ok(())
}

/// Output builtins are immutable even though ordinary builtins remain in the shadowable fallback tier.
#[test]
fn print_and_println_cannot_be_redefined() -> Result<(), String> {
    let cases = [
        ("print", "def print(value: int) -> int:\n  return value\n"),
        ("println", "def println(value: int) -> int:\n  return value\n"),
        ("print", "def run() -> None:\n  let print = 1\n"),
        ("println", "def run() -> None:\n  mut println = 1\n"),
    ];
    for (name, source) in cases {
        let program = parse(source, "immutable output builtin")?;
        let mut checker = TypeChecker::new();
        let errors = match checker.check_program(&program) {
            Ok(()) => return Err(format!("immutable output builtin {name} was replaced")),
            Err(errors) => errors,
        };
        assert!(
            errors
                .iter()
                .any(|error| error.message == format!("Cannot redefine immutable built-in function '{name}'")),
            "expected immutable-builtin diagnostic for {name}, got: {errors:?}"
        );
    }

    let provider = parse("pub def render() -> None:\n  pass\n", "output alias provider")?;
    for name in ["print", "println"] {
        let consumer = parse(
            &format!("from helpers import render as {name}\n"),
            "output alias consumer",
        )?;
        let mut checker = TypeChecker::new();
        let import_errors = match checker.check_with_imports(&consumer, &[("helpers", &provider)]) {
            Ok(()) => return Err(format!("an import replaced immutable {name}")),
            Err(errors) => errors,
        };
        assert!(
            import_errors
                .iter()
                .any(|error| error.message == format!("Cannot redefine immutable built-in function '{name}'")),
            "expected immutable-builtin import diagnostic for {name}, got: {import_errors:?}"
        );
        assert!(
            !checker.type_info().declarations.function_bindings.contains_key(name)
                && !checker
                    .type_info()
                    .declarations
                    .resolved_import_identities
                    .contains_key(name)
                && !checker.source_import_targets.contains_key(name),
            "a rejected immutable-builtin import must not populate semantic side tables for {name}"
        );
    }
    Ok(())
}

/// Member spellings do not replace the immutable lexical output bindings.
#[test]
fn member_print_and_println_names_remain_available() -> Result<(), String> {
    check(
        r#"
model Printer:
  print: str

  def println(self) -> None:
    pass
"#,
        "output member namespace",
    )?;
    Ok(())
}

/// Builtin alias spellings share one canonical registry identity instead of minting one identity per spelling.
#[test]
fn builtin_alias_spellings_share_one_registry_identity() -> Result<(), String> {
    let checker = check("def noop() -> None:\n  pass\n", "builtin aliases")?;
    let mut int_identities = Vec::new();
    for (id, symbol) in checker.symbols.all_symbols().iter().enumerate() {
        if (symbol.name == "int" || symbol.name == "i64")
            && let Some(identity) = checker.symbols.identity_of(id)
            && identity.origin == SymbolOrigin::Builtin
        {
            int_identities.push(identity.clone());
        }
    }
    assert!(
        int_identities.len() >= 2,
        "expected canonical and alias spellings of the int builtin, got {int_identities:?}"
    );
    assert!(
        int_identities.iter().all(|identity| identity == &int_identities[0]),
        "every alias spelling must carry the one canonical registry identity: {int_identities:?}"
    );
    Ok(())
}

/// Consts and statics carry their declaration categories from the one mint point.
#[test]
fn const_and_static_identities_carry_their_categories() -> Result<(), String> {
    let source = r#"
const LIMIT: int = 10

static counter: int = 0

def read() -> int:
  return LIMIT
"#;
    let checker = check(source, "const and static")?;
    let limit = identity_at(&checker, nth_span(source, "LIMIT", 1)?, "const reference")?;
    assert_eq!(limit.kind, SemanticSourceTargetKind::Const);
    assert_eq!(limit.scope_discriminant, None);

    let counter = checker
        .type_info()
        .declarations
        .declaration_identities
        .values()
        .find(|identity| identity.declaration_name == "counter")
        .ok_or("static declaration identity must be exported")?;
    assert_eq!(counter.kind, SemanticSourceTargetKind::Static);
    Ok(())
}

/// Two same-spelled module declarations are diagnosed by the shared registration mechanism while both declaration
/// sites retain distinct identities for the diagnostic and later inspection.
#[test]
fn duplicate_module_declarations_keep_distinct_identities() -> Result<(), String> {
    let source = r#"
model User:
  name: str

model User:
  age: int
"#;
    let program = parse(source, "duplicate declarations")?;
    let second_span = program.declarations[1].span;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["conformance".to_string()]));
    let errors = match checker.check_program(&program) {
        Ok(()) => return Err("duplicate module declarations were accepted".to_string()),
        Err(errors) => errors,
    };
    let duplicate = errors
        .iter()
        .find(|error| error.message == "Duplicate definition of 'User'")
        .ok_or_else(|| format!("missing duplicate-binding diagnostic: {errors:?}"))?;
    assert_eq!(
        duplicate.span, second_span,
        "the duplicate declaration is the primary span"
    );
    assert_eq!(
        duplicate
            .notes
            .iter()
            .filter(|note| note.contains("canonical identity"))
            .count(),
        2,
        "the diagnostic must name both canonical identities: {duplicate:?}"
    );
    let user_identities: Vec<&CanonicalSymbolId> = checker
        .type_info()
        .declarations
        .declaration_identities
        .values()
        .filter(|identity| identity.declaration_name == "User")
        .collect();
    assert_eq!(
        user_identities.len(),
        2,
        "both declaration sites must keep their own exported identity"
    );
    assert_ne!(
        user_identities[0], user_identities[1],
        "two declaration sites are two identities, never one merged winner"
    );
    let active = checker
        .symbols
        .lookup("User")
        .ok_or("the first User binding must remain active")?;
    let active_identity = checker
        .symbols
        .identity_of(active)
        .ok_or("the active User binding must retain its identity")?;
    assert_eq!(
        active_identity.declaration_span,
        incan_semantics_core::HirSourceSpan::new(program.declarations[0].span.start, program.declarations[0].span.end),
        "a rejected duplicate cannot change the active lookup identity"
    );
    Ok(())
}

/// Rejected nominal declarations retain evidence but cannot mutate metadata owned by the active first declaration.
#[test]
fn rejected_nominal_duplicates_cannot_mutate_the_first_binding() -> Result<(), String> {
    let cases = [
        (
            r#"
newtype Token = int:
  def first(self) -> int:
    return 1

newtype Token = str:
  def second(self) -> int:
    return 2
"#,
            "Token",
            "first",
            "second",
            "newtype",
        ),
        (
            r#"
enum State:
  First

  def first(self) -> int:
    return 1

enum State:
  SecondOnly

  def second(self) -> int:
    return 2
"#,
            "State",
            "first",
            "second",
            "enum",
        ),
        (
            r#"
trait Root:
  def root(self) -> int: ...

trait Other:
  def other(self) -> int: ...

trait Contract with Root:
  def first(self) -> int: ...

trait Contract with Other:
  def second(self) -> int: ...
"#,
            "Contract",
            "first",
            "second",
            "trait",
        ),
    ];

    for (source, name, first_method, rejected_method, kind) in cases {
        let program = parse(source, &format!("duplicate {kind}"))?;
        let mut checker = TypeChecker::new();
        let errors = match checker.check_program(&program) {
            Ok(()) => return Err(format!("duplicate {kind} declaration was accepted")),
            Err(errors) => errors,
        };
        assert!(
            errors
                .iter()
                .any(|error| error.message == format!("Duplicate definition of '{name}'")),
            "missing duplicate {kind} diagnostic: {errors:?}"
        );
        let active = checker
            .symbols
            .lookup(name)
            .ok_or_else(|| format!("active {kind} missing"))?;
        let (methods, supertraits) = match checker.symbols.get(active).map(|symbol| &symbol.kind) {
            Some(SymbolKind::Type(TypeInfo::Newtype(info))) => (&info.methods, None),
            Some(SymbolKind::Type(TypeInfo::Enum(info))) => (&info.methods, None),
            Some(SymbolKind::Trait(info)) => (&info.methods, Some(&info.supertraits)),
            other => return Err(format!("unexpected active {kind} symbol: {other:?}")),
        };
        assert!(
            methods.contains_key(first_method),
            "first {kind} API must remain active"
        );
        assert!(
            !methods.contains_key(rejected_method),
            "rejected {kind} API mutated the first binding"
        );
        if kind == "enum" {
            assert!(
                !checker.symbols.all_symbols().iter().any(|symbol| {
                    symbol.name == "SecondOnly"
                        && matches!(&symbol.kind, SymbolKind::Variant(info) if info.enum_name == "State")
                }),
                "variants from a rejected enum must not enter the symbol arena"
            );
        }
        if let Some(supertraits) = supertraits {
            assert_eq!(
                supertraits.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>(),
                vec!["Root"],
                "the rejected trait's supertraits must not overwrite the first trait"
            );
        }
    }
    Ok(())
}

/// A local declaration over an imported binding is a duplicate active binding, not implicit shadowing.
#[test]
fn local_declaration_over_import_is_diagnosed() -> Result<(), String> {
    let provider = parse(
        "pub def helper(value: str) -> str:\n  return value\n",
        "collision provider",
    )?;
    let consumer_source = r#"
from lib import helper

def helper(value: int) -> int:
  return value

def read() -> None:
  observed = helper
"#;
    let consumer = parse(consumer_source, "collision consumer")?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["consumer".to_string()]));
    let errors = match checker.check_with_imports(&consumer, &[("lib", &provider)]) {
        Ok(()) => return Err("a declaration implicitly replaced an imported binding".to_string()),
        Err(errors) => errors,
    };
    assert!(
        errors
            .iter()
            .any(|error| error.message == "Duplicate definition of 'helper'"),
        "expected a shared binding collision, got: {errors:?}"
    );
    let imported = checker
        .type_info()
        .declarations
        .resolved_import_identities
        .get("helper")
        .ok_or("the imported helper identity must be retained")?;
    let observed = identity_at(
        &checker,
        nth_span(consumer_source, "helper", 2)?,
        "post-collision reference",
    )?;
    assert_eq!(
        &observed, imported,
        "the first import remains active after the local declaration is rejected"
    );
    let callable = checker
        .type_info()
        .declarations
        .function_bindings
        .get("helper")
        .ok_or("the active import must retain its callable metadata")?;
    assert_eq!(
        callable.params.first().map(|param| &param.ty),
        Some(&crate::frontend::symbols::ResolvedType::Str)
    );
    assert_eq!(callable.return_type, crate::frontend::symbols::ResolvedType::Str);
    Ok(())
}

/// Two unaliased imports with the same local spelling and different declaration identities are ambiguous.
#[test]
fn same_spelled_imports_from_different_modules_are_ambiguous() -> Result<(), String> {
    let left = parse("pub model Item:\n  left: int\n", "left provider")?;
    let right = parse("pub model Item:\n  right: str\n", "right provider")?;
    let consumer = parse(
        "from left import Item\nfrom right import Item\n",
        "ambiguous import consumer",
    )?;
    let first = consumer.declarations[0].span;
    let second = consumer.declarations[1].span;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["consumer".to_string()]));
    let errors = match checker.check_with_imports(&consumer, &[("left", &left), ("right", &right)]) {
        Ok(()) => return Err("different declarations shared one imported spelling".to_string()),
        Err(errors) => errors,
    };
    let ambiguous = errors
        .iter()
        .find(|error| error.message == "Ambiguous import binding 'Item'")
        .ok_or_else(|| format!("expected the shared ambiguity diagnostic, got: {errors:?}"))?;
    assert_eq!(ambiguous.span, second);
    assert_eq!(
        ambiguous.related_spans().first().map(|related| related.span),
        Some(first)
    );
    let canonical_notes = ambiguous
        .notes
        .iter()
        .filter(|note| note.contains("canonical identity"))
        .collect::<Vec<_>>();
    assert_eq!(canonical_notes.len(), 2);
    assert_ne!(canonical_notes[0], canonical_notes[1]);
    assert_eq!(
        ambiguous.hints,
        vec!["Use an explicit import alias so each declaration has a distinct local spelling"]
    );
    let target = checker
        .source_import_targets
        .get("Item")
        .ok_or("the active import must retain its source target")?;
    assert_eq!(target.module_path, vec!["left".to_string()]);
    let active_identity = checker
        .type_info()
        .declarations
        .resolved_import_identities
        .get("Item")
        .ok_or("the active import must retain its resolved identity")?;
    assert_eq!(active_identity.origin, SymbolOrigin::Module(vec!["left".to_string()]));
    Ok(())
}

/// A target-specific source identity cannot leak into a later stdlib import through their shared local spelling.
#[test]
fn source_and_stdlib_import_collision_is_order_independent() -> Result<(), String> {
    let provider = parse("pub def value() -> int:\n  return 1\n", "source import provider")?;
    for source in [
        "from lib import value as shared\nfrom std.collections import OrdinalKey as shared\n",
        "from std.collections import OrdinalKey as shared\nfrom lib import value as shared\n",
    ] {
        let consumer = parse(source, "source and stdlib import collision")?;
        let mut checker = TypeChecker::new();
        let errors = match checker.check_with_imports(&consumer, &[("lib", &provider)]) {
            Ok(()) => return Err("source/stdlib collision was accepted".to_string()),
            Err(errors) => errors,
        };
        let ambiguous = errors
            .iter()
            .find(|error| error.message == "Ambiguous import binding 'shared'")
            .ok_or_else(|| format!("collision classification depended on import order: {errors:?}"))?;
        assert_eq!(
            ambiguous
                .notes
                .iter()
                .filter(|note| note.contains("canonical identity"))
                .count(),
            2,
            "both checked import targets must be proven in either order: {ambiguous:?}"
        );
    }
    Ok(())
}

/// Source aliases preserve target identity without being mislabeled as imports in collision diagnostics.
#[test]
fn local_alias_collisions_are_duplicates_not_ambiguous_imports() -> Result<(), String> {
    let source = r#"
def left() -> int:
  return 1

def right() -> int:
  return 2

same = alias left
same = alias right
"#;
    let program = parse(source, "local alias collision")?;
    let mut checker = TypeChecker::new();
    let errors = match checker.check_program(&program) {
        Ok(()) => return Err("two local aliases shared one name".to_string()),
        Err(errors) => errors,
    };
    assert!(
        errors
            .iter()
            .any(|error| error.message == "Duplicate definition of 'same'"),
        "expected a local duplicate diagnostic: {errors:?}"
    );
    assert!(
        errors
            .iter()
            .all(|error| error.message != "Ambiguous import binding 'same'"),
        "a local alias is not an import: {errors:?}"
    );
    Ok(())
}

/// A module import cannot skip the shared registry merely because another concrete binding arrived first.
#[test]
fn module_import_after_concrete_bindings_reports_the_collision() -> Result<(), String> {
    let left = parse("pub model Item:\n  value: int\n", "left provider")?;
    let right = parse("pub def value() -> int:\n  return 1\n", "right provider")?;
    let local_first = parse(
        "def shared() -> int:\n  return 1\n\nimport right as shared\n",
        "local before module import",
    )?;
    let mut checker = TypeChecker::new();
    let errors = match checker.check_with_imports(&local_first, &[("right", &right)]) {
        Ok(()) => return Err("a module import silently skipped a preceding local binding".to_string()),
        Err(errors) => errors,
    };
    assert!(
        errors
            .iter()
            .any(|error| error.message == "Duplicate definition of 'shared'"),
        "expected local/module collision, got: {errors:?}"
    );

    let item_first = parse(
        "from left import Item\nimport right as Item\n",
        "item before module import",
    )?;
    let first = item_first.declarations[0].span;
    let second = item_first.declarations[1].span;
    let mut checker = TypeChecker::new();
    let errors = match checker.check_with_imports(&item_first, &[("left", &left), ("right", &right)]) {
        Ok(()) => return Err("a module import silently skipped a preceding item import".to_string()),
        Err(errors) => errors,
    };
    let ambiguous = errors
        .iter()
        .find(|error| error.message == "Ambiguous import binding 'Item'")
        .ok_or_else(|| format!("expected item/module ambiguity, got: {errors:?}"))?;
    assert_eq!(ambiguous.span, second);
    assert_eq!(
        ambiguous.related_spans().first().map(|related| related.span),
        Some(first)
    );
    assert_eq!(
        ambiguous
            .notes
            .iter()
            .filter(|note| note.contains("canonical identity"))
            .count(),
        2,
        "the item and module imports must both report their proven identities"
    );
    assert_eq!(
        ambiguous.hints,
        vec!["Use an explicit import alias so each declaration has a distinct local spelling"]
    );
    Ok(())
}

/// Module aliases obey the same builtin tiers as every other source binding.
#[test]
fn module_aliases_cannot_replace_output_builtins_but_can_shadow_ordinary_builtins() -> Result<(), String> {
    let provider = parse("pub def value() -> int:\n  return 1\n", "module alias provider")?;
    for name in ["print", "println"] {
        let consumer = parse(&format!("import provider as {name}\n"), "immutable module alias")?;
        let mut checker = TypeChecker::new();
        let errors = match checker.check_with_imports(&consumer, &[("provider", &provider)]) {
            Ok(()) => return Err(format!("module alias replaced immutable {name}")),
            Err(errors) => errors,
        };
        assert!(
            errors
                .iter()
                .any(|error| error.message == format!("Cannot redefine immutable built-in function '{name}'")),
            "expected immutable-builtin diagnostic for module alias {name}, got: {errors:?}"
        );
    }

    let consumer = parse("import provider as len\n", "ordinary builtin module alias")?;
    let mut checker = TypeChecker::new();
    checker
        .check_with_imports(&consumer, &[("provider", &provider)])
        .map_err(|errors| format!("ordinary builtin alias should typecheck: {errors:?}"))?;
    let binding = checker.symbols.lookup("len").ok_or("the module alias must be active")?;
    assert!(
        matches!(
            checker.symbols.get(binding).map(|symbol| &symbol.kind),
            Some(crate::frontend::symbols::SymbolKind::Module(_))
        ),
        "the module alias must replace the ordinary builtin fallback"
    );
    Ok(())
}

/// Repeating one proven import is a duplicate, not an ambiguity, and the diagnostic retains both exact sites and the
/// one shared target identity.
#[test]
fn repeated_proven_import_is_a_duplicate_with_complete_evidence() -> Result<(), String> {
    let provider = parse("pub model Item:\n  value: int\n", "repeat provider")?;
    let consumer = parse("from lib import Item\nfrom lib import Item\n", "repeat consumer")?;
    let first = consumer.declarations[0].span;
    let second = consumer.declarations[1].span;
    let mut checker = TypeChecker::new();
    let errors = match checker.check_with_imports(&consumer, &[("lib", &provider)]) {
        Ok(()) => return Err("a repeated proven import was accepted".to_string()),
        Err(errors) => errors,
    };
    let duplicate = errors
        .iter()
        .find(|error| error.message == "Duplicate definition of 'Item'")
        .ok_or_else(|| format!("missing duplicate-import diagnostic: {errors:?}"))?;
    assert_eq!(duplicate.span, second);
    assert_eq!(
        duplicate.related_spans().first().map(|related| related.span),
        Some(first)
    );
    let identity = checker
        .type_info()
        .declarations
        .resolved_import_identities
        .get("Item")
        .ok_or("the proven import identity must be retained")?
        .render_compact();
    assert_eq!(
        duplicate.notes,
        vec![
            format!("First canonical identity: {identity}"),
            format!("Second canonical identity: {identity}"),
        ]
    );
    Ok(())
}

/// Enum alias collection and semantic validation consume one shared collision answer, so a duplicate alias produces
/// one diagnostic rather than one from each phase.
#[test]
fn duplicate_enum_variant_alias_is_reported_once() -> Result<(), String> {
    let source = r#"
enum Level:
  Info
  Warn
  Warning = alias Warn
  Warning = alias Info
"#;
    let program = parse(source, "duplicate enum variant alias")?;
    let Declaration::Enum(en) = &program.declarations[0].node else {
        return Err("expected enum declaration".to_string());
    };
    let first_alias_span = en.variant_aliases[0].span;
    let rejected_alias_span = en.variant_aliases[1].span;
    let mut checker = TypeChecker::new();
    let errors = match checker.check_program(&program) {
        Ok(()) => return Err("duplicate enum variant alias was accepted".to_string()),
        Err(errors) => errors,
    };
    let duplicates = errors
        .iter()
        .filter(|error| error.message == "Duplicate definition of 'Warning'")
        .collect::<Vec<_>>();
    assert_eq!(
        duplicates.len(),
        1,
        "the shared registry must emit exactly one alias collision: {errors:?}"
    );
    assert_eq!(duplicates[0].related_spans().len(), 1);
    let warn = checker.symbols.lookup("Warn").ok_or("Warn binding missing")?;
    let info = checker.symbols.lookup("Info").ok_or("Info binding missing")?;
    let warning = checker.symbols.lookup("Warning").ok_or("Warning alias missing")?;
    assert_eq!(checker.symbols.identity_of(warning), checker.symbols.identity_of(warn));
    assert_ne!(checker.symbols.identity_of(warning), checker.symbols.identity_of(info));
    let level = checker.symbols.lookup("Level").ok_or("Level binding missing")?;
    let Some(SymbolKind::Type(TypeInfo::Enum(level))) = checker.symbols.get(level).map(|symbol| &symbol.kind) else {
        return Err("Level should retain enum metadata".to_string());
    };
    assert_eq!(level.variant_aliases.get("Warning").map(String::as_str), Some("Warn"));
    let exported = &checker.type_info().declarations.member_declaration_identities;
    assert!(!exported.contains_key(&(first_alias_span.start, first_alias_span.end)));
    assert!(!exported.contains_key(&(rejected_alias_span.start, rejected_alias_span.end)));
    Ok(())
}

/// A valid enum alias is a second binding to the target variant, not a second variant declaration identity.
#[test]
fn enum_variant_alias_preserves_the_target_identity() -> Result<(), String> {
    let checker = check(
        r#"
enum Level:
  Warn
  Warning = alias Warn
"#,
        "enum variant alias identity",
    )?;
    let warn = checker.symbols.lookup("Warn").ok_or("Warn binding missing")?;
    let warning = checker.symbols.lookup("Warning").ok_or("Warning alias missing")?;
    assert_eq!(
        checker.symbols.identity_of(warn),
        checker.symbols.identity_of(warning),
        "an enum alias must carry its target variant's canonical identity"
    );
    Ok(())
}

/// Repeating one checked module import is a duplicate of the same path-namespace declaration, not an ambiguity.
#[test]
fn repeated_module_import_has_one_deterministic_identity() -> Result<(), String> {
    let provider = parse("pub def value() -> int:\n  return 1\n", "module provider")?;
    let consumer = parse(
        "import provider as shared\nimport provider as shared\n",
        "module consumer",
    )?;
    let first = consumer.declarations[0].span;
    let second = consumer.declarations[1].span;
    let mut checker = TypeChecker::new();
    let errors = match checker.check_with_imports(&consumer, &[("provider", &provider)]) {
        Ok(()) => return Err("a repeated module import was accepted".to_string()),
        Err(errors) => errors,
    };
    let duplicate = errors
        .iter()
        .find(|error| error.message == "Duplicate definition of 'shared'")
        .ok_or_else(|| format!("expected repeated-module duplicate, got: {errors:?}"))?;
    assert_eq!(duplicate.span, second);
    assert_eq!(
        duplicate.related_spans().first().map(|related| related.span),
        Some(first)
    );
    let active = checker
        .symbols
        .lookup("shared")
        .ok_or("shared module binding missing")?;
    let identity = checker
        .symbols
        .identity_of(active)
        .ok_or("checked module binding must carry its path identity")?;
    let expected = crate::frontend::symbols::SymbolTable::module_path_identity(&["provider".to_string()])
        .ok_or("provider path must produce a module identity")?;
    assert_eq!(identity, &expected);
    assert_eq!(
        duplicate.notes,
        vec![
            format!("First canonical identity: {}", identity.render_compact()),
            format!("Second canonical identity: {}", identity.render_compact()),
        ],
        "the duplicate diagnostic must prove both bindings target the same module"
    );
    Ok(())
}

/// Preparing a dependency records its interface for explicit import resolution without placing its declarations in
/// the consumer's lexical scope.
#[test]
fn dependency_interfaces_are_not_ambient_consumer_bindings() -> Result<(), String> {
    let provider = parse(
        "pub model Hidden:\n  value: int\n\npub trait Contract:\n  def read(self) -> int\n\npub def helper() -> int:\n  return 1\n",
        "dependency interface provider",
    )?;
    let consumer = parse(
        "def read(value: Hidden) -> int:\n  observed = helper\n  return 1\n",
        "dependency interface consumer",
    )?;
    let mut checker = TypeChecker::new();
    let errors = match checker.check_with_imports(&consumer, &[("provider", &provider)]) {
        Ok(()) => return Err("unimported dependency declarations leaked into consumer scope".to_string()),
        Err(errors) => errors,
    };
    assert!(
        errors.iter().any(|error| error.message == "Unknown symbol 'Hidden'"),
        "expected unimported type diagnostic, got: {errors:?}"
    );
    assert!(
        errors.iter().any(|error| error.message == "Unknown symbol 'helper'"),
        "expected unimported value diagnostic, got: {errors:?}"
    );
    for name in ["Hidden", "Contract", "helper"] {
        assert_eq!(
            checker.symbols.lookup(name),
            None,
            "dependency interface binding `{name}` survived into consumer lookup"
        );
    }
    Ok(())
}

/// An explicit import creates exactly one consumer binding carrying the provider declaration's identity; sibling
/// exports remain absent.
#[test]
fn explicit_dependency_import_materializes_only_its_aliased_binding() -> Result<(), String> {
    let provider = parse(
        "pub model Imported:\n  value: int\n\npub model Unimported:\n  value: int\n",
        "explicit import provider",
    )?;
    let consumer_source =
        "from provider import Imported as Local\n\ndef read(value: Local) -> Local:\n  return value\n";
    let consumer = parse(consumer_source, "explicit import consumer")?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["consumer".to_string()]));
    checker
        .check_with_imports(&consumer, &[("provider", &provider)])
        .map_err(|errors| format!("explicit import should typecheck: {errors:?}"))?;
    assert!(
        checker.symbols.lookup("Local").is_some(),
        "the explicit alias must be active"
    );
    assert_eq!(
        checker.symbols.lookup("Unimported"),
        None,
        "an unimported sibling declaration must remain absent"
    );
    let identity = checker
        .type_info()
        .declarations
        .resolved_import_identities
        .get("Local")
        .ok_or("the explicit alias must carry a resolved identity")?;
    assert_eq!(identity.origin, SymbolOrigin::Module(vec!["provider".to_string()]));
    assert_eq!(identity.declaration_name, "Imported");
    Ok(())
}

/// A public source alias remains a binding to its target declaration after crossing a dependency boundary.
#[test]
fn dependency_public_alias_preserves_its_target_identity() -> Result<(), String> {
    let provider_source = r#"
pub def helper() -> int:
  return 1

pub run = alias helper
"#;
    let provider = parse(provider_source, "public alias provider")?;
    let consumer = parse(
        "from provider import run\n\ndef read() -> int:\n  return run()\n",
        "public alias consumer",
    )?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["consumer".to_string()]));
    checker
        .check_with_imports(&consumer, &[("provider", &provider)])
        .map_err(|errors| format!("public alias import should typecheck: {errors:?}"))?;
    let identity = checker
        .type_info()
        .declarations
        .resolved_import_identities
        .get("run")
        .ok_or("the imported public alias must retain its target identity")?;
    assert_eq!(identity.origin, SymbolOrigin::Module(vec!["provider".to_string()]));
    assert_eq!(identity.declaration_name, "helper");
    let helper_span = provider.declarations[0].span;
    assert_eq!(
        identity.declaration_span,
        incan_semantics_core::HirSourceSpan::new(helper_span.start, helper_span.end),
        "the alias import must remain anchored at the target declaration"
    );
    Ok(())
}

/// Dependency-owned transparent aliases do not rewrite a same-spelled local nominal declaration, while an explicit
/// aliased import receives the exact provider-owned target.
#[test]
fn dependency_type_alias_targets_are_isolated_until_explicitly_imported() -> Result<(), String> {
    let provider = parse("pub type Payload = str\n", "type alias provider")?;
    let local = parse(
        "model Payload:\n  value: int\n\ndef keep(value: Payload) -> Payload:\n  return value\n",
        "local nominal consumer",
    )?;
    let mut checker = TypeChecker::new();
    checker
        .check_with_imports(&local, &[("provider", &provider)])
        .map_err(|errors| format!("local nominal type should remain independent: {errors:?}"))?;
    assert!(
        !checker.type_aliases.contains_key("Payload"),
        "an unimported dependency alias target leaked into the consumer"
    );

    let imported = parse(
        "from provider import Payload as TextPayload\n\ndef read(value: TextPayload) -> str:\n  return value\n",
        "explicit type alias consumer",
    )?;
    let mut checker = TypeChecker::new();
    checker
        .check_with_imports(&imported, &[("provider", &provider)])
        .map_err(|errors| format!("explicit type alias import should typecheck: {errors:?}"))?;
    let target = checker
        .type_aliases
        .get("TextPayload")
        .ok_or("the explicit type alias import must retain its target")?;
    assert_eq!(target.target, crate::frontend::symbols::ResolvedType::Str);
    Ok(())
}

/// A rejected duplicate type alias cannot overwrite the active alias-expansion target.
#[test]
fn duplicate_type_alias_side_table_keeps_the_first_target() -> Result<(), String> {
    let source = "type Payload = int\ntype Payload = str\n";
    let program = parse(source, "duplicate type alias target")?;
    let mut checker = TypeChecker::new();
    let errors = match checker.check_program(&program) {
        Ok(()) => return Err("duplicate type aliases were accepted".to_string()),
        Err(errors) => errors,
    };
    assert!(
        errors
            .iter()
            .any(|error| error.message == "Duplicate definition of 'Payload'"),
        "expected duplicate type-alias diagnostic, got: {errors:?}"
    );
    let target = checker
        .type_aliases
        .get("Payload")
        .ok_or("the first type alias target must remain active")?;
    assert_eq!(target.target, crate::frontend::symbols::ResolvedType::Int);
    Ok(())
}

/// Per-module predeclaration keeps equal spellings from sibling dependencies independent even when the importing
/// bridge is collected before either provider.
#[test]
fn dependency_predeclaration_is_module_exact_and_order_independent() -> Result<(), String> {
    let left = parse("pub model Contract[T]:\n  value: T\n", "left contract")?;
    let right = parse("pub model Contract[A, B]:\n  first: A\n  second: B\n", "right contract")?;
    let bridge = parse(
        "from right import Contract\n\npub Selected = alias Contract\n\npub def pair(value: Contract[int, str]) -> Contract[int, str]:\n  return value\n",
        "contract bridge",
    )?;
    let consumer = parse("def noop() -> None:\n  pass\n", "order consumer")?;
    for dependencies in [
        [("bridge", &bridge), ("left", &left), ("right", &right)],
        [("bridge", &bridge), ("right", &right), ("left", &left)],
    ] {
        let mut checker = TypeChecker::new();
        checker
            .check_with_imports(&consumer, &dependencies)
            .map_err(|errors| format!("module-exact dependency collection failed: {errors:?}"))?;
        let selected = checker
            .dependency_member_symbols
            .get("bridge")
            .and_then(|members| members.get("Selected"))
            .ok_or("bridge.Selected was not cached")?;
        let crate::frontend::symbols::SymbolKind::Type(crate::frontend::symbols::TypeInfo::Model(info)) = selected
        else {
            return Err(format!("bridge.Selected is not a model alias: {selected:?}"));
        };
        assert_eq!(
            info.type_params,
            vec!["A".to_string(), "B".to_string()],
            "bridge.Selected must carry right.Contract's two-parameter interface regardless of collection order"
        );
    }
    Ok(())
}

/// The non-canonical same-leaf fallback fails closed when more than one dependency module can answer it.
#[test]
fn ambiguous_dependency_leaf_fallback_does_not_select_by_order() -> Result<(), String> {
    let first = parse("pub model Contract:\n  first: int\n", "first helpers")?;
    let second = parse("pub model Contract:\n  second: str\n", "second helpers")?;
    let consumer = parse("def noop() -> None:\n  pass\n", "ambiguous leaf consumer")?;
    let mut checker = TypeChecker::new();
    checker.register_dependency_module_path_segments("pkg_helpers", vec!["pkg".to_string(), "helpers".to_string()]);
    checker.register_dependency_module_path_segments("other_helpers", vec!["other".to_string(), "helpers".to_string()]);
    checker
        .check_with_imports(&consumer, &[("pkg_helpers", &first), ("other_helpers", &second)])
        .map_err(|errors| format!("dependency cache setup should typecheck: {errors:?}"))?;
    let ambiguous_path = crate::frontend::ast::ImportPath::simple(vec!["helpers".to_string()]);
    assert!(
        checker
            .dependency_member_symbol_for_path(&ambiguous_path, "Contract")
            .is_none(),
        "same-leaf fallback must not select whichever dependency was collected first"
    );
    Ok(())
}

/// Module imports participate in the same ambiguity mechanism as item imports.
#[test]
fn same_alias_for_different_module_imports_is_ambiguous() -> Result<(), String> {
    let left = parse("pub def left_value() -> int:\n  return 1\n", "left module")?;
    let right = parse("pub def right_value() -> int:\n  return 2\n", "right module")?;
    let consumer = parse(
        "import left as shared\nimport right as shared\n",
        "module alias consumer",
    )?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["consumer".to_string()]));
    let errors = match checker.check_with_imports(&consumer, &[("left", &left), ("right", &right)]) {
        Ok(()) => return Err("different modules shared one local import alias".to_string()),
        Err(errors) => errors,
    };
    assert!(
        errors
            .iter()
            .any(|error| error.message == "Ambiguous import binding 'shared'"),
        "expected shared module-import ambiguity diagnostic, got: {errors:?}"
    );
    assert_eq!(
        checker.type_info().import_binding_path("shared"),
        Some(["left".to_string()].as_slice()),
        "decorator/module routing must retain the first accepted import"
    );
    let active = checker
        .symbols
        .lookup("shared")
        .ok_or("shared module binding must remain active")?;
    let Some(crate::frontend::symbols::SymbolKind::Module(module)) =
        checker.symbols.get(active).map(|symbol| &symbol.kind)
    else {
        return Err("shared did not remain a module binding".to_string());
    };
    assert_eq!(module.path, vec!["left".to_string()]);
    Ok(())
}

/// A reference that resolves to a local overload set must not inherit a separately-aliased import's identity.
///
/// Overload sets deliberately carry no set-level identity. An adjacent import must use an explicit alias under RFC
/// 120's collision contract, and reference-side recording must stay empty rather than borrow that import proof — an
/// identity fact for the wrong binding is worse than none.
#[test]
fn overload_set_beside_an_aliased_import_records_no_identity() -> Result<(), String> {
    let provider = parse("pub def helper() -> int:\n  return 1\n", "overload-shadow provider")?;
    let consumer_source = r#"
from lib import helper as imported_helper

def helper(value: int) -> int:
  return value

def helper(value: str) -> str:
  return value

def read() -> None:
  observed = helper
"#;
    let consumer = parse(consumer_source, "overload-shadow consumer")?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["consumer".to_string()]));
    // The reference errors ("cannot use overloaded function as a value"), which is expected and not the subject
    // here; the recorded identity map must still be inspectable and must not carry the import's identity.
    let _ = checker.check_with_imports(&consumer, &[("lib", &provider)]);

    let reference_span = nth_span(consumer_source, "helper", 3)?;
    assert_eq!(
        checker.type_info().resolved_identity(reference_span),
        None,
        "an overload-set reference has no set-level identity and must not borrow the aliased import's"
    );
    Ok(())
}

/// Direct-call fast paths preserve the declaration identity already proven for local, imported, aliased, re-exported,
/// and builtin bindings.
#[test]
fn direct_function_calls_record_the_resolved_binding_identity() -> Result<(), String> {
    let local_source = r#"
def helper() -> int:
  return 1

def use_helper() -> int:
  return helper()
"#;
    let local_checker = check(local_source, "local direct function call")?;
    let local_identity = identity_at(
        &local_checker,
        nth_span(local_source, "helper", 2)?,
        "local direct call",
    )?;
    assert_eq!(local_identity.kind, SemanticSourceTargetKind::Function);
    assert_eq!(local_identity.declaration_name, "helper");

    let provider = parse("pub def compute() -> int:\n  return 1\n", "direct-call provider")?;
    let facade = parse("pub from provider import compute as exposed\n", "direct-call facade")?;
    let consumer_source = r#"
from provider import compute
from provider import compute as renamed
from facade import exposed as execute

def use_all() -> int:
  first = compute()
  second = renamed()
  return execute()
"#;
    let consumer = parse(consumer_source, "direct-call consumer")?;
    let mut import_checker = TypeChecker::new();
    import_checker.set_current_module_path(Some(vec!["consumer".to_string()]));
    import_checker
        .check_with_imports(&consumer, &[("provider", &provider), ("facade", &facade)])
        .map_err(|errors| format!("direct imported calls should typecheck: {errors:?}"))?;
    let imported_identity = import_checker
        .type_info()
        .resolved_import_identity("compute")
        .ok_or("the direct import must prove its declaration identity")?
        .clone();
    for (needle, occurrence, context) in [
        ("compute", 2, "direct imported call"),
        ("renamed", 1, "aliased imported call"),
        ("execute", 1, "re-exported imported call"),
    ] {
        assert_eq!(
            identity_at(&import_checker, nth_span(consumer_source, needle, occurrence)?, context)?,
            imported_identity,
            "{context} must retain the provider declaration identity"
        );
    }

    let builtin_source = r#"
def emit() -> None:
  print("first")
  println("second")
"#;
    let builtin_checker = check(builtin_source, "direct builtin calls")?;
    let print_identity = identity_at(
        &builtin_checker,
        nth_span(builtin_source, "print", 0)?,
        "canonical print call",
    )?;
    let println_identity = identity_at(
        &builtin_checker,
        nth_span(builtin_source, "println", 0)?,
        "print alias call",
    )?;
    assert_eq!(
        print_identity, println_identity,
        "builtin aliases share one registry identity"
    );
    assert_eq!(print_identity.origin, SymbolOrigin::Builtin);
    assert_eq!(print_identity.kind, SemanticSourceTargetKind::Builtin);
    assert_eq!(print_identity.declaration_name, "print");
    Ok(())
}

/// Qualified calls retain the declaration selected by the checker instead of asking downstream consumers to parse
/// the dotted source spelling.
#[test]
fn qualified_calls_record_builtin_and_variant_identities() -> Result<(), String> {
    let source = r#"
enum Signal:
  Ready

def use_all() -> int:
  count = std.builtins.len([1, 2])
  signal = Signal.Ready()
  return count
"#;
    let checker = check(source, "qualified builtin and variant calls")?;

    let builtin_identity = identity_at(
        &checker,
        nth_span(source, "std.builtins.len([1, 2])", 0)?,
        "qualified builtin call",
    )?;
    assert_eq!(builtin_identity.origin, SymbolOrigin::Builtin);
    assert_eq!(builtin_identity.declaration_name, "len");
    assert_eq!(builtin_identity.kind, SemanticSourceTargetKind::Builtin);

    let variant_identity = identity_at(
        &checker,
        nth_span(source, "Signal.Ready()", 0)?,
        "qualified enum variant call",
    )?;
    assert_eq!(
        variant_identity.origin,
        SymbolOrigin::Module(vec!["conformance".to_string()])
    );
    assert_eq!(variant_identity.declaration_name, "Ready");
    assert_eq!(variant_identity.kind, SemanticSourceTargetKind::Variant);
    assert_eq!(variant_identity.namespace, SymbolNamespace::Member);
    Ok(())
}

/// Model, newtype, and enum-variant constructor calls record their nominal declaration binding at the callee token.
#[test]
fn direct_constructor_calls_record_nominal_and_variant_identities() -> Result<(), String> {
    let source = r#"
model Parcel:
  value: int

type Ticket = newtype int:
  def unwrap(self) -> int:
    return self.0

enum Message:
  Count(int)

def build() -> None:
  parcel = Parcel(value=1)
  ticket = Ticket(2)
  message = Count(3)
"#;
    let checker = check(source, "direct constructor identities")?;
    for (name, occurrence, expected_kind) in [
        ("Parcel", 1, SemanticSourceTargetKind::Model),
        ("Ticket", 1, SemanticSourceTargetKind::Newtype),
        ("Count", 1, SemanticSourceTargetKind::Variant),
    ] {
        let declaration = checker
            .symbols
            .lookup(name)
            .and_then(|symbol_id| checker.symbols.identity_of(symbol_id))
            .ok_or_else(|| format!("{name} declaration must carry an identity"))?;
        assert_eq!(declaration.kind, expected_kind);
        assert_eq!(
            checker
                .type_info()
                .resolved_identity(nth_span(source, name, occurrence)?),
            Some(declaration),
            "{name} constructor must record the resolved declaration at its callee token"
        );
    }
    Ok(())
}

/// A direct top-level overload call records the one selected declaration; failed and ambiguous resolution record none.
#[test]
fn direct_function_overloads_record_only_a_unique_selected_declaration() -> Result<(), String> {
    let source = r#"
def convert(value: int) -> int:
  return value

def convert(value: str) -> str:
  return value

def accepted() -> int:
  return convert(1)

def rejected() -> bool:
  return convert(true)
"#;
    let program = parse(source, "direct function overload identity")?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["conformance".to_string()]));
    let errors = check_errors(
        &mut checker,
        &program,
        "the bool return hint must leave the direct overload unresolved",
    )?;
    assert!(
        errors.iter().any(|error| error.message.contains("convert")),
        "expected the failed overload call to be diagnosed: {errors:?}"
    );
    let selected_identity = checker
        .type_info()
        .declarations
        .function_bindings_by_span
        .values()
        .find(|binding| {
            binding.return_type == crate::frontend::symbols::ResolvedType::Int
                && binding
                    .identity
                    .as_ref()
                    .is_some_and(|identity| identity.declaration_name == "convert")
        })
        .and_then(|binding| binding.identity.as_ref())
        .ok_or("the selected int overload must carry a declaration identity")?;
    assert_eq!(
        checker.type_info().resolved_identity(nth_span(source, "convert", 2)?),
        Some(selected_identity)
    );
    assert_eq!(
        checker.type_info().resolved_identity(nth_span(source, "convert", 3)?),
        None,
        "the candidate used only for a failed-call diagnostic is not a resolved target"
    );

    let ambiguous_source = r#"
def choose(value: int) -> int:
  return value

def choose(value: int) -> str:
  return "chosen"

def ambiguous() -> None:
  observed = choose(1)
"#;
    let ambiguous_program = parse(ambiguous_source, "ambiguous direct function overload")?;
    let mut ambiguous_checker = TypeChecker::new();
    ambiguous_checker.set_current_module_path(Some(vec!["conformance".to_string()]));
    let errors = check_errors(
        &mut ambiguous_checker,
        &ambiguous_program,
        "the same-parameter overload call must be ambiguous without a return hint",
    )?;
    assert!(
        errors.iter().any(|error| error.message.contains("ambiguous")),
        "expected overload ambiguity diagnostic: {errors:?}"
    );
    assert_eq!(
        ambiguous_checker
            .type_info()
            .resolved_identity(nth_span(ambiguous_source, "choose", 2)?),
        None,
        "an ambiguous call has no selected declaration identity"
    );
    Ok(())
}

/// Closure parameters are callable parameters, not ordinary locals, and body reads retain that declaration.
#[test]
fn closure_parameters_use_parameter_identities() -> Result<(), String> {
    let source = r#"
def build() -> None:
  callback = (value) => value
"#;
    let checker = check(source, "closure parameter identity")?;
    let parameter_span = nth_span(source, "value", 0)?;
    let body_identity = identity_at(&checker, nth_span(source, "value", 1)?, "closure parameter body read")?;
    assert_eq!(body_identity.kind, SemanticSourceTargetKind::Parameter);
    assert_eq!(body_identity.declaration_name, "value");
    assert_eq!(
        body_identity.declaration_span,
        incan_semantics_core::HirSourceSpan::new(parameter_span.start, parameter_span.end),
        "the parameter identity must be anchored to the closure parameter declaration"
    );
    Ok(())
}

/// F-string interpolation parses in a temporary coordinate space; both the closure parameter and body read must be
/// rebased into the containing source before the parameter identity is minted.
#[test]
fn f_string_closure_parameter_identity_uses_outer_source_span() -> Result<(), String> {
    let source = r#"
def render() -> str:
  return f"{((value) => value)(1)}"
"#;
    let checker = check(source, "f-string closure parameter identity")?;
    let parameter_span = nth_span(source, "value", 0)?;
    let body_identity = identity_at(&checker, nth_span(source, "value", 1)?, "f-string closure body read")?;
    assert_eq!(body_identity.kind, SemanticSourceTargetKind::Parameter);
    assert_eq!(body_identity.declaration_name, "value");
    assert_eq!(
        body_identity.declaration_span,
        incan_semantics_core::HirSourceSpan::new(parameter_span.start, parameter_span.end),
        "the f-string closure parameter must use outer-source coordinates"
    );
    Ok(())
}

/// Match-arm type refinement changes only the subject type; it must not replace the subject declaration identity.
#[test]
fn match_narrowing_preserves_the_subject_identity() -> Result<(), String> {
    let source = r#"
def observe(value: int | str) -> None:
  match value:
    _ =>
      captured = value
"#;
    let checker = check(source, "match subject narrowing identity")?;
    let subject = identity_at(&checker, nth_span(source, "value", 1)?, "match subject")?;
    let narrowed = identity_at(&checker, nth_span(source, "value", 2)?, "narrowed subject read")?;
    assert_eq!(subject, narrowed, "narrowing must preserve the parameter declaration");
    assert_eq!(narrowed.kind, SemanticSourceTargetKind::Parameter);
    Ok(())
}

/// All writes of a valid OR-pattern introduce the one binding that its arm body reads.
#[test]
fn or_pattern_writes_share_the_final_body_binding_identity() -> Result<(), String> {
    let source = r#"
def unwrap(result: Result[int, int]) -> int:
  match result:
    Ok(value) | Err(value) =>
      return value
"#;
    let checker = check(source, "OR-pattern binding identity")?;
    let first_span = nth_span(source, "value", 0)?;
    let second_span = nth_span(source, "value", 1)?;
    let first = write_identity_at(&checker, first_span, "value", "first OR-pattern write")?;
    let second = write_identity_at(&checker, second_span, "value", "second OR-pattern write")?;
    let body = identity_at(&checker, nth_span(source, "value", 2)?, "OR-pattern body read")?;
    assert_eq!(first, second, "both alternatives must write the same declaration");
    assert_eq!(
        second, body,
        "the body must read the declaration written by either alternative"
    );
    Ok(())
}

/// A race header is one source declaration even though every winner arm refines it to its own awaited output type.
#[test]
fn race_arms_share_the_exact_header_binding_identity() -> Result<(), String> {
    let source = r#"
import std.async

async def fast() -> int:
  return 1

async def slow() -> str:
  return "ready"

async def choose() -> int | str:
  return race for value:
    await fast() => value
    await slow() => value
"#;
    let checker = check(source, "race header binding identity")?;
    let header_span = nth_span(source, "value", 0)?;
    let header = write_identity_at(&checker, header_span, "value", "race header write")?;
    let first = identity_at(&checker, nth_span(source, "value", 1)?, "first race-arm read")?;
    let second = identity_at(&checker, nth_span(source, "value", 2)?, "second race-arm read")?;
    assert_eq!(header, first);
    assert_eq!(first, second);
    assert_eq!(header.kind, SemanticSourceTargetKind::Local);
    assert_eq!(
        header.declaration_span,
        incan_semantics_core::HirSourceSpan::new(header_span.start, header_span.end)
    );
    Ok(())
}

/// Constructor and pattern labels resolve at their exact authored token spans, including canonical field members.
#[test]
fn constructor_and_pattern_labels_record_nominal_and_field_identities() -> Result<(), String> {
    let source = r#"
model Parcel:
  count: int

def build() -> Parcel:
  return Parcel(count=1)

def unpack(parcel: Parcel) -> int:
  match parcel:
    Parcel(count=value) =>
      return value
"#;
    let checker = check(source, "constructor and pattern label identities")?;
    let parcel_id = checker.symbols.lookup("Parcel").ok_or("missing Parcel symbol")?;
    let parcel_symbol = checker.symbols.get(parcel_id).ok_or("missing Parcel metadata")?;
    let parcel_identity = checker
        .symbols
        .identity_of(parcel_id)
        .ok_or("Parcel must have a canonical declaration identity")?;
    let SymbolKind::Type(TypeInfo::Model(model)) = &parcel_symbol.kind else {
        return Err("Parcel must retain model metadata".to_string());
    };
    let count_identity = model
        .fields
        .get("count")
        .and_then(|field| field.identity.as_ref())
        .ok_or("count must have a canonical member identity")?;

    assert_eq!(
        checker.type_info().resolved_identity(nth_span(source, "Parcel", 2)?),
        Some(parcel_identity),
        "the ordinary constructor callee must name the model declaration"
    );
    assert_eq!(
        checker.type_info().resolved_identity(nth_span(source, "count", 1)?),
        Some(count_identity),
        "the constructor keyword label must name the field declaration"
    );
    assert_eq!(
        checker.type_info().resolved_identity(nth_span(source, "Parcel", 4)?),
        Some(parcel_identity),
        "the pattern constructor label must name the model declaration"
    );
    assert_eq!(
        checker.type_info().resolved_identity(nth_span(source, "count", 2)?),
        Some(count_identity),
        "the pattern field label must name the field declaration"
    );
    Ok(())
}

/// The restricted `assert value is Some(binding)` path records the same builtin constructor identity as an ordinary
/// checked pattern, but only when the scrutinee proves that the constructor is compatible.
#[test]
fn assert_is_pattern_records_compatible_constructor_identity_at_its_exact_span() -> Result<(), String> {
    let source = r#"
import std.testing

def unwrap(value: Option[int]) -> int:
  assert value is Some(inner)
  return inner
"#;
    let checker = check(source, "assert is-pattern constructor identity")?;
    let some = constructors::as_str(ConstructorId::Some);
    let constructor_span = nth_span(source, some, 0)?;
    let identity = identity_at(&checker, constructor_span, "assert is-pattern constructor")?;
    assert_eq!(identity.origin, SymbolOrigin::Builtin);
    assert_eq!(identity.kind, SemanticSourceTargetKind::Builtin);
    assert_eq!(identity.declaration_name, some);
    assert_eq!(identity.namespace, SymbolNamespace::OrdinaryLexical);

    let unresolved_source = r#"
import std.testing

def unresolved(value: MissingType) -> None:
  assert value is Some(inner)
"#;
    let unresolved_program = parse(unresolved_source, "unresolved assert is-pattern scrutinee")?;
    let mut unresolved_checker = TypeChecker::new();
    unresolved_checker.set_current_module_path(Some(vec!["conformance".to_string()]));
    let errors = check_errors(
        &mut unresolved_checker,
        &unresolved_program,
        "unresolved assert is-pattern scrutinee",
    )?;
    assert!(
        errors.iter().any(|error| error.message.contains("MissingType")),
        "the unresolved annotation must remain an error: {errors:?}"
    );
    assert_eq!(
        unresolved_checker
            .type_info()
            .resolved_identity(nth_span(unresolved_source, some, 0)?),
        None,
        "recovery from an unresolved scrutinee must not mint constructor authority"
    );
    Ok(())
}

/// A fieldless enum variant is a member use even when it is read as a value rather than called as a constructor.
#[test]
fn qualified_fieldless_enum_value_records_variant_identity() -> Result<(), String> {
    let source = r#"
enum Signal:
  Ready

def current() -> Signal:
  return Signal.Ready

def is_ready(signal: Signal) -> bool:
  match signal:
    Signal.Ready => return true
"#;
    let checker = check(source, "qualified fieldless enum value identity")?;
    let identity = identity_at(
        &checker,
        nth_span(source, "Signal.Ready", 0)?,
        "qualified fieldless enum value",
    )?;
    assert_eq!(identity.kind, SemanticSourceTargetKind::Variant);
    assert_eq!(identity.declaration_name, "Ready");
    assert_eq!(identity.namespace, SymbolNamespace::Member);
    assert_eq!(
        checker
            .type_info()
            .resolved_identity(nth_span(source, "Signal.Ready", 1)?),
        Some(&identity),
        "the qualified variant pattern must retain the same canonical variant at its exact label span"
    );
    Ok(())
}
