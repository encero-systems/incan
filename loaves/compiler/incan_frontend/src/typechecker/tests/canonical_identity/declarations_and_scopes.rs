//! Declaration-site identities: module declarations, sibling-block and shadowed bindings, assignment target spans, type
//! annotations and generic binders, parameters and receivers (`self`, `cls`), member methods and accesses, consts and
//! statics, and the builtin tiers.

use super::*;

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
                .find(|method| method.return_type == crate::symbols::ResolvedType::Int)
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
