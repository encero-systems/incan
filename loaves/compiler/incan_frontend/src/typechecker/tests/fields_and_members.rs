//! RFC 021 field metadata and aliases, computed properties, private field access for classes and `pub` models (#884),
//! and how static, instance and enum-variant member surfaces stay distinct.

use super::*;

#[test]
fn test_class_private_field_access_rejected_outside_owner() {
    let source = r#"
pub class LazyFrame:
  _cursor: int
  pub schema: str

def leak(frame: LazyFrame) -> int:
  return frame._cursor
"#;
    let errors = check_str_err(source, "private class field access should fail typechecking");
    assert!(
        has_private_field_error(&errors, "LazyFrame", "_cursor"),
        "expected private field error, got: {:?}",
        errors.iter().map(|error| &error.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_class_private_field_access_allowed_inside_owner_method() {
    let source = r#"
pub class LazyFrame:
  _cursor: int

  def cursor(self) -> int:
    return self._cursor
"#;
    assert_check_ok(source);
}

#[test]
fn test_pub_model_private_field_access_is_type_private_issue884() {
    let source = r#"
pub model Vault:
  secret: str
  pub label: str

  def reveal(self) -> str:
    return self.secret

pub model Inspector:
  pub name: str

  def leak(self, vault: Vault) -> str:
    return vault.secret

def leak(vault: Vault) -> str:
  return vault.secret
"#;
    let errors = check_str_err(
        source,
        "private model field access should fail outside the declaring type",
    );
    let private_errors = errors
        .iter()
        .filter(|error| error.message.contains("Field 'secret' on 'Vault' is private"))
        .count();
    assert_eq!(
        private_errors,
        2,
        "expected outside function and different model method to fail, got: {:?}",
        errors.iter().map(|error| &error.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_private_model_fields_keep_existing_module_private_semantics_issue884() {
    let source = r#"
model Internal:
  value: int

def read(value: Internal) -> int:
  return value.value
"#;
    assert_check_ok(source);
}

#[test]
fn test_pub_model_private_field_is_available_inside_owner_methods_issue884() {
    let source = r#"
pub model Vault:
  secret: str
  pub label: str

  @staticmethod
  def create(label: str) -> Vault:
    return Vault(secret="sealed", label=label)

  def reveal(self) -> str:
    return self.secret

  def unpack(self) -> str:
    match self:
      Vault(secret=value) =>
        return value
"#;
    assert_check_ok(source);
}

#[test]
fn test_pub_model_private_fields_are_rejected_in_external_construction_and_patterns_issue884() {
    let source = r#"
pub model Vault:
  secret [alias="wire_secret"]: str = "sealed"
  pub label: str

def construct() -> Vault:
  return Vault(wire_secret="leaked", label="outside")

def unpack(vault: Vault) -> str:
  match vault:
    Vault(secret=value) =>
      return value
"#;
    let errors = check_str_err(
        source,
        "private model fields should fail in external named construction and constructor patterns",
    );
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("Field 'wire_secret' on 'Vault' is private")),
        "expected aliased private constructor field error, got: {:?}",
        errors.iter().map(|error| &error.message).collect::<Vec<_>>()
    );
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("Field 'secret' on 'Vault' is private")),
        "expected private pattern field error, got: {:?}",
        errors.iter().map(|error| &error.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_class_private_field_settable_via_external_named_construction() {
    // Unlike `pub model`, a class's private fields remain settable through ordinary named-argument construction
    // from outside the class, even across modules (see
    // issue886/`loaves/toolchain/incan-cli/tests/integration_tests.rs`,
    // `test_imported_private_class_constructor_compile_and_run_issue886`, and the stdlib's `tempfile.incn`
    // module-level factory functions, which both rely on this). Only member access after construction is private.
    let source = r#"
class Vault:
  secret: str = "sealed"
  pub label: str

def construct() -> Vault:
  return Vault(secret="leaked", label="outside")
"#;
    assert_check_ok(source);
}

#[test]
fn test_class_private_field_pattern_match_extraction_is_rejected() {
    let source = r#"
class Vault:
  secret: str = "sealed"
  pub label: str

def unpack(vault: Vault) -> str:
  match vault:
    Vault(secret=value) =>
      return value
"#;
    let errors = check_str_err(
        source,
        "extracting a private class field through a constructor pattern should fail like a direct field read",
    );
    assert!(
        has_private_field_error(&errors, "Vault", "secret"),
        "expected private pattern field error, got: {:?}",
        errors.iter().map(|error| &error.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_class_private_parent_field_access_rejected_in_child_method() {
    let source = r#"
class Parent:
  private_value: int

class Child extends Parent:
  def expose(self) -> int:
    return self.private_value
"#;
    let errors = check_str_err(source, "private parent field access should fail in child method");
    assert!(
        has_private_field_error(&errors, "Child", "private_value"),
        "expected private field error, got: {:?}",
        errors.iter().map(|error| &error.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_alias_resolution_member_and_constructor() {
    let source = r#"
model Account:
  type_ [alias="type"]: str

def f(a: Account) -> str:
  let x = Account(type="premium")
  return a.type
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_canonical_wins_over_alias_resolution() {
    // RFC 021: When typechecking a field key, canonical name is checked first, then alias.
    // This test verifies that accessing by canonical name works even when the same model
    // has aliases, and that the type is correctly resolved from the canonical field.
    let source = r#"
model Data:
    foo [alias="wire_foo"]: str
    bar: int

def test_canonical_access(d: Data) -> str:
    # Accessing by canonical name should work and return the correct type
    return d.foo

def test_alias_access(d: Data) -> str:
    # Accessing by alias should also work
    return d.wire_foo

def test_constructor_canonical(name: str) -> Data:
    # Constructor with canonical name
    return Data(foo=name, bar=42)

def test_constructor_alias(name: str) -> Data:
    # Constructor with alias
    return Data(wire_foo=name, bar=42)
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_canonical_takes_precedence_in_mixed_access() {
    // RFC 021: Canonical name takes precedence. If a field has both canonical name
    // and alias, both should work independently with correct type resolution.
    let source = r#"
model Account:
    name: str
    type_ [alias="type"]: str
    balance: int

def access_all(a: Account) -> str:
    # Access fields by canonical name
    let n = a.name       # canonical, no alias
    let t = a.type_      # canonical (has alias "type")
    let b = a.balance    # canonical, no alias

    # Access field by alias
    let t2 = a.type      # alias for type_

    # Both t and t2 should have type str
    return f"{n} {t} {t2} {b}"
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_alias_resolution_in_pattern() {
    let source = r#"
model Account:
  type_ [alias="type"]: str

def f(a: Account) -> str:
  match a:
    Account(type="premium") => return "premium"
    Account(type="basic") => return "basic"
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_duplicate_alias_error() {
    let source = r#"
model Account:
  a [alias="wire"]: str
  b [alias="wire"]: int
"#;
    let Err(err) = check_str(source) else {
        panic!("Expected duplicate alias error");
    };
    assert!(err.iter().any(|e| e.message.contains("Duplicate alias")));
}

#[test]
fn test_alias_collides_with_canonical_error() {
    let source = r#"
model Account:
  type_: str
  kind [alias="type_"]: str
"#;
    let Err(err) = check_str(source) else {
        panic!("Expected alias collision error");
    };
    assert!(
        err.iter()
            .any(|e| e.message.contains("collides with a canonical field name"))
    );
}

#[test]
fn test_alias_collides_with_method_error() {
    let source = r#"
model Account:
  type_ [alias="describe"]: str

  def describe(self) -> str:
    return self.type_
"#;
    let Err(err) = check_str(source) else {
        panic!("Expected alias/method collision error");
    };
    assert!(err.iter().any(|e| e.message.contains("collides with a method name")));
}

#[test]
fn test_empty_alias_error() {
    let source = r#"
model Account:
  type_ [alias=""]: str
"#;
    let Err(err) = check_str(source) else {
        panic!("Expected empty alias error");
    };
    assert!(err.iter().any(|e| e.message.contains("non-empty")));
}

#[test]
fn test_whitespace_alias_error() {
    let source = r#"
model Account:
  type_ [alias="   "]: str
"#;
    let Err(err) = check_str(source) else {
        panic!("Expected whitespace alias error");
    };
    assert!(err.iter().any(|e| e.message.contains("non-empty")));
}

#[test]
fn test_alias_and_canonical_in_constructor_error() {
    let source = r#"
model Account:
  type_ [alias="type"]: str

def f() -> Account:
  return Account(type="x", type_="y")
"#;
    let Err(err) = check_str(source) else {
        panic!("Expected duplicate field error");
    };
    assert!(err.iter().any(|e| e.message.contains("Duplicate constructor argument")));
}

#[test]
fn test_non_identifier_alias_allowed() {
    let source = r#"
model Weird:
  one_ [alias="1"]: int

def f(w: Weird) -> int:
  return w.one_
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_alias_not_supported_on_class() {
    // RFC 021: Field aliases are only supported on `model`, not `class`
    let source = r#"
class Account:
  type_ [alias="type"]: str
"#;
    let Err(err) = check_str(source) else {
        panic!("Expected class alias error");
    };
    assert!(err.iter().any(|e| e.message.contains("not supported on class")));
}

#[test]
fn test_numeric_alias_member_access_error() {
    let source = r#"
model Weird:
  one_ [alias="1"]: int

def f(w: Weird) -> int:
  return w.1
"#;
    let Err(err) = check_str(source) else {
        panic!("Expected error for numeric access");
    };
    assert!(err.iter().any(|e| e.message.contains("no field '1'")));
}

#[test]
fn test_alias_collides_with_builtin_error() {
    let source = r#"
model Account:
  fields_ [alias="__fields__"]: str
"#;
    let Err(err) = check_str(source) else {
        panic!("Expected builtin collision error");
    };
    assert!(err.iter().any(|e| e.message.contains("builtin member")));
}

#[test]
fn test_alias_and_canonical_in_pattern_error() {
    let source = r#"
model Account:
  type_ [alias="type"]: str

def f(a: Account) -> str:
  match a:
    Account(type="x", type_="y") => return "x"
"#;
    let Err(err) = check_str(source) else {
        panic!("Expected duplicate pattern field error");
    };
    assert!(err.iter().any(|e| e.message.contains("Duplicate pattern field")));
}

#[test]
fn test_unicode_alias_allowed() {
    let source = r#"
model Intl:
  name_ [alias="名前"]: str

def f(i: Intl) -> str:
  return i.name_
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_computed_property_read_typechecks_and_records_access() -> Result<(), String> {
    let source = r#"
model Account:
  cents: int

  property dollars -> int:
    return self.cents

def f(account: Account) -> int:
  return account.dollars
"#;
    let tokens = lexer::lex(source).map_err(|errs| format!("{errs:?}"))?;
    let ast = parser::parse(&tokens).map_err(|errs| format!("{errs:?}"))?;
    let mut checker = TypeChecker::new();
    checker.check_program(&ast).map_err(|errs| format!("{errs:?}"))?;
    assert_eq!(checker.type_info.expressions.computed_property_accesses.len(), 1);
    let access = checker
        .type_info
        .expressions
        .computed_property_accesses
        .values()
        .next()
        .ok_or_else(|| "expected computed property access metadata".to_string())?;
    assert_eq!(access.owner_type, "Account");
    assert_eq!(access.property, "dollars");
    Ok(())
}

#[test]
fn test_computed_property_call_syntax_is_rejected() {
    let source = r#"
model Account:
  cents: int

  property dollars -> int:
    return self.cents

def f(account: Account) -> int:
  return account.dollars()
"#;
    let errors = check_str_err(source, "expected computed property call error");
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("Computed property 'dollars' is not callable")),
        "expected property call diagnostic, got {errors:?}"
    );
}

#[test]
fn test_computed_property_body_return_type_is_checked() {
    let source = r#"
model Account:
  cents: int

  property dollars -> int:
    return "free"
"#;
    let errors = check_str_err(source, "expected computed property return mismatch");
    assert!(
        errors.iter().any(|error| error
            .message
            .contains("Return type mismatch: expected 'int', found 'str'")),
        "expected property return mismatch diagnostic, got {errors:?}"
    );
}

#[test]
fn test_trait_computed_property_requirement_must_be_implemented() {
    let source = r#"
trait Named:
  property label -> str

class Person with Named:
  name: str
"#;
    let errors = check_str_err(source, "expected missing trait property error");
    assert!(
        errors.iter().any(|error| error
            .message
            .contains("Trait 'Named' requires property 'label' to be implemented")),
        "expected missing trait property diagnostic, got {errors:?}"
    );
}

#[test]
fn test_trait_computed_property_requirement_accepts_matching_property() {
    let source = r#"
trait Named:
  property label -> str

class Person with Named:
  name: str

  property label -> str:
    return self.name
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_trait_computed_property_body_is_rejected() {
    let source = r#"
trait Named:
  property label -> str:
    return "name"
"#;
    let errors = check_str_err(source, "expected trait property body error");
    assert!(
        errors.iter().any(|error| error
            .message
            .contains("Trait 'Named' property 'label' cannot define a body")),
        "expected trait property body diagnostic, got {errors:?}"
    );
}

#[test]
fn test_property_member_name_collision_is_rejected() {
    let source = r#"
class Account:
  cents: int

  property cents -> int:
    return self.cents
"#;
    let errors = check_str_err(source, "expected duplicate property member error");
    assert!(
        errors.iter().any(|error| error
            .message
            .contains("Duplicate member 'Account.cents' declared as both field and property")),
        "expected duplicate member diagnostic, got {errors:?}"
    );
}

#[test]
fn test_property_method_name_collision_is_rejected() {
    let source = r#"
class Account:
  cents: int

  def total(self) -> int:
    return self.cents

  property total -> int:
    return self.cents
"#;
    let errors = check_str_err(source, "expected duplicate property method member error");
    assert!(
        errors.iter().any(|error| error
            .message
            .contains("Duplicate member 'Account.total' declared as both method and property")),
        "expected duplicate member diagnostic, got {errors:?}"
    );
}

/// A type-owned factory and stored instance field may share a spelling because the receiver determines which
/// declaration can be selected. The factory parameter remains nominally typed; its underlying integer argument is
/// admitted through the ordinary validated-newtype coercion path.
#[test]
fn static_factory_and_instance_field_have_distinct_member_surfaces() -> Result<(), String> {
    let source = r#"
type Days = newtype int

model TimeDelta:
  days: Days

  @staticmethod
  def days(value: Days) -> TimeDelta:
    return TimeDelta(days=value)

def selected() -> TimeDelta:
  return TimeDelta.days(-7)
"#;
    let tokens = lexer::lex(source).map_err(|errors| format!("member-surface source should lex: {errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("member-surface source should parse: {errors:?}"))?;
    let Some(Declaration::Model(model)) = program.declarations.get(1).map(|declaration| &declaration.node) else {
        return Err("expected TimeDelta model declaration".to_string());
    };
    let field_span = model.fields[0].span;
    let method_span = model.methods[0].span;

    let mut checker = TypeChecker::new();
    checker
        .check_program(&program)
        .map_err(|errors| format!("type-owned factory and instance field should coexist: {errors:?}"))?;
    let declarations = &checker.type_info().declarations.member_declaration_identities;
    assert!(
        declarations.contains_key(&(field_span.start, field_span.end)),
        "the instance field must retain its declaration identity"
    );
    assert!(
        declarations.contains_key(&(method_span.start, method_span.end)),
        "the type-owned factory must retain its declaration identity"
    );
    Ok(())
}

#[test]
fn static_factory_cannot_be_called_through_an_instance() {
    let source = r#"
model TimeDelta:
  days: int

  @staticmethod
  def days(value: int) -> TimeDelta:
    return TimeDelta(days=value)

def invalid(delta: TimeDelta) -> TimeDelta:
  return delta.days(-7)
"#;
    let errors = check_str_err(source, "staticmethod instance receiver should fail");
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("Type 'TimeDelta' has no method 'days(...)'")),
        "a stored field must not make the type-owned factory callable through an instance: {errors:?}"
    );
}

#[test]
fn static_and_instance_methods_may_share_a_spelling() {
    let source = r#"
model Counter:
  value: int

  @staticmethod
  def next(value: int) -> Counter:
    return Counter(value=value)

  def next(self) -> int:
    return self.value + 1

def selected() -> int:
  counter = Counter.next(4)
  return counter.next()
"#;
    assert!(
        check_str(source).is_ok(),
        "receiver ownership must disambiguate type-owned and instance-owned methods"
    );
}

#[test]
fn stored_field_static_factory_and_instance_accumulator_support_fluent_chaining() {
    let source = r#"
model TimeDelta:
  days: int
  seconds: int

  @staticmethod
  def days(value: int) -> TimeDelta:
    return TimeDelta(days=value, seconds=0)

  def days(self, value: int) -> TimeDelta:
    return TimeDelta(days=self.days + value, seconds=self.seconds)

  def hours(self, value: int) -> TimeDelta:
    return TimeDelta(days=self.days, seconds=self.seconds + value * 3600)

def selected() -> TimeDelta:
  return TimeDelta.days(-7).hours(20).days(2)
"#;
    assert!(
        check_str(source).is_ok(),
        "field access, type-owned construction, and fluent instance accumulation must remain distinct"
    );
}

#[test]
fn instance_method_cannot_be_called_through_its_type() {
    let source = r#"
model Counter:
  value: int

  def next(self) -> int:
    return self.value + 1

def invalid() -> int:
  return Counter.next()
"#;
    let errors = check_str_err(source, "instance method type receiver should fail");
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("Type 'Counter' has no method 'next(...)'")),
        "an instance method must not resolve through its type: {errors:?}"
    );
}

#[test]
fn test_colliding_member_bindings_keep_distinct_field_and_method_declarations() -> Result<(), String> {
    let source = r#"
class Surface:
  clash: int

  def source(self) -> int:
    return 1

  def clash(self) -> int:
    return 2

  property clash -> int:
    return 3

  clash = source
  clash = partial source()
"#;
    let tokens = lexer::lex(source).map_err(|errors| format!("member collision source should lex: {errors:?}"))?;
    let program =
        parser::parse(&tokens).map_err(|errors| format!("member collision source should parse: {errors:?}"))?;
    let Some(Declaration::Class(class)) = program.declarations.first().map(|declaration| &declaration.node) else {
        return Err("expected class declaration".to_string());
    };
    let first_span = class.fields[0].span;
    let method_span = class
        .methods
        .iter()
        .find(|method| method.node.name == "clash")
        .ok_or("missing clashing method")?
        .span;
    let rejected_spans = [
        class.properties[0].span,
        class.method_aliases[0].span,
        class.method_partials[0].span,
    ];

    let mut checker = TypeChecker::new();
    let errors = match checker.check_program(&program) {
        Ok(()) => return Err("cross-kind member collisions should fail".to_string()),
        Err(errors) => errors,
    };
    let collision_errors = errors
        .iter()
        .filter(|error| {
            error.message.starts_with("Duplicate member")
                || error.message.starts_with("Duplicate method alias")
                || error.message.starts_with("Duplicate method partial")
        })
        .collect::<Vec<_>>();
    assert_eq!(
        collision_errors.len(),
        3,
        "property, alias, and partial must each collide with the first field while the callable method remains distinct: {errors:?}"
    );
    let first_note = format!("First declaration span: {}..{}", first_span.start, first_span.end);
    assert!(
        collision_errors
            .iter()
            .all(|error| error.notes.iter().any(|note| note == &first_note)),
        "every collision must retain the source-first field: {collision_errors:?}"
    );

    let exported = &checker.type_info().declarations.member_declaration_identities;
    assert!(
        exported.contains_key(&(first_span.start, first_span.end)),
        "the accepted field declaration must be exported"
    );
    assert!(
        exported.contains_key(&(method_span.start, method_span.end)),
        "the separately callable method declaration must be exported"
    );
    assert!(
        rejected_spans
            .iter()
            .all(|span| !exported.contains_key(&(span.start, span.end))),
        "rejected member bindings must not be exported as declarations"
    );
    Ok(())
}

#[test]
fn test_duplicate_enum_variants_keep_first_metadata_and_export_only_the_accepted_declaration() -> Result<(), String> {
    let source = r#"
enum Status:
  Ready
  Ready
"#;
    let tokens = lexer::lex(source).map_err(|errors| format!("duplicate enum source should lex: {errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("duplicate enum source should parse: {errors:?}"))?;
    let Some(Declaration::Enum(en)) = program.declarations.first().map(|declaration| &declaration.node) else {
        return Err("expected enum declaration".to_string());
    };
    let first_span = en.variants[0].span;
    let rejected_span = en.variants[1].span;

    let mut checker = TypeChecker::new();
    let errors = match checker.check_program(&program) {
        Ok(()) => return Err("duplicate enum variant was accepted".to_string()),
        Err(errors) => errors,
    };
    let duplicates = errors
        .iter()
        .filter(|error| error.message == "Duplicate definition of 'Ready'")
        .collect::<Vec<_>>();
    assert_eq!(
        duplicates.len(),
        1,
        "expected one shared-registry diagnostic: {errors:?}"
    );
    assert_eq!(duplicates[0].span, rejected_span);
    assert_eq!(
        duplicates[0].related_spans().first().map(|related| related.span),
        Some(first_span)
    );

    let status_id = checker.symbols.lookup("Status").ok_or("missing Status symbol")?;
    let status = checker.symbols.get(status_id).ok_or("missing Status metadata")?;
    let SymbolKind::Type(TypeInfo::Enum(info)) = &status.kind else {
        return Err("Status should retain enum metadata".to_string());
    };
    assert_eq!(info.variants, vec!["Ready"]);
    let retained = info
        .variant_identities
        .get("Ready")
        .ok_or("accepted Ready variant has no canonical identity")?;
    assert_eq!(retained.declaration_span.start, first_span.start);
    assert_eq!(retained.declaration_span.end, first_span.end);

    let exported = &checker.type_info().declarations.member_declaration_identities;
    assert!(exported.contains_key(&(first_span.start, first_span.end)));
    assert!(!exported.contains_key(&(rejected_span.start, rejected_span.end)));
    Ok(())
}

#[test]
fn enum_variant_and_instance_method_have_distinct_member_surfaces() -> Result<(), String> {
    let source = r#"
enum Signal:
  Ready

  def Ready(self) -> int:
    return 1
"#;
    let tokens = lexer::lex(source).map_err(|errors| format!("enum collision source should lex: {errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("enum collision source should parse: {errors:?}"))?;
    let Some(Declaration::Enum(en)) = program.declarations.first().map(|declaration| &declaration.node) else {
        return Err("expected enum declaration".to_string());
    };
    let variant_span = en.variants[0].span;
    let method_span = en.methods[0].span;

    let mut checker = TypeChecker::new();
    checker
        .check_program(&program)
        .map_err(|errors| format!("type-owned enum variant and instance method should coexist: {errors:?}"))?;

    let signal_id = checker.symbols.lookup("Signal").ok_or("missing Signal symbol")?;
    let signal = checker.symbols.get(signal_id).ok_or("missing Signal metadata")?;
    let SymbolKind::Type(TypeInfo::Enum(info)) = &signal.kind else {
        return Err("Signal should retain enum metadata".to_string());
    };
    assert!(info.variant_identities.contains_key("Ready"));
    assert!(info.method_overloads.contains_key("Ready"));

    let exported = &checker.type_info().declarations.member_declaration_identities;
    assert!(exported.contains_key(&(variant_span.start, variant_span.end)));
    assert!(exported.contains_key(&(method_span.start, method_span.end)));
    Ok(())
}

#[test]
fn enum_variant_and_static_method_still_collide_on_the_type_surface() {
    let source = r#"
enum Signal:
  Ready

  @staticmethod
  def Ready() -> int:
    return 1
"#;
    let errors = check_str_err(source, "enum type-surface collision should fail");
    assert!(
        errors.iter().any(|error| error
            .message
            .contains("Duplicate member 'Signal.Ready' declared as both variant and method")),
        "a static method and enum variant must still collide on the type surface: {errors:?}"
    );
}

#[test]
fn test_alias_self_keyword() {
    let source = r#"
model Data:
  self_ [alias="self"]: str

def f(d: Data) -> str:
  return d.self
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_alias_super_keyword_member_access() {
    let source = r#"
model Data:
  super_ [alias="super"]: str

def f(d: Data) -> str:
  return d.super
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_alias_super_keyword_constructor_key() {
    let source = r#"
model Data:
  super_ [alias="super"]: str

def f() -> Data:
  return Data(super="x")
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_alias_super_keyword_pattern_key() {
    let source = r#"
model Data:
  super_ [alias="super"]: str

def f(d: Data) -> str:
  match d:
    Data(super=x) => return x
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_alias_underscore_member_access() {
    let source = r#"
model Data:
  under_ [alias="_"]: str

def f(d: Data) -> str:
  return d._
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_alias_underscore_constructor_key() {
    let source = r#"
model Data:
  under_ [alias="_"]: str

def f() -> Data:
  return Data(_="x")
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_alias_underscore_pattern_key() {
    let source = r#"
model Data:
  under_ [alias="_"]: str

def f(d: Data) -> str:
  match d:
    Data(_=x) => return x
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_alias_unicode_normalization_variants_treated_as_distinct() {
    // RFC 021: alias matching uses exact string equality; no Unicode normalization is performed.
    // Example: NFC "é" vs NFD "e\u{301}" must be treated as distinct aliases.
    let source = r#"
model Data:
  nfc_ [alias="é"]: str
  nfd_ [alias="e\u{301}"]: str

def f(d: Data) -> str:
  return d.nfc_
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_alias_case_variants_treated_as_distinct() {
    // RFC 021: no case-folding is performed for alias matching.
    let source = r#"
model Data:
  lower_ [alias="type"]: str
  upper_ [alias="Type"]: str

def f(d: Data) -> str:
  return d.lower_
"#;
    assert!(check_str(source).is_ok());
}

// ---- #1723: a plain-`self` method must not write through the receiver ----

const SELF_MUTATION_CODE: &str = "INCAN-T0102";

/// The `INCAN-T0102` diagnostics of a refused program, by message, so a test can name what it expects.
fn self_mutation_messages(source: &str, context: &str) -> Vec<String> {
    check_str_err(source, context)
        .iter()
        .filter(|error| error.stable_code() == Some(SELF_MUTATION_CODE))
        .map(|error| error.message.clone())
        .collect()
}

#[test]
fn class_method_assigning_to_a_field_through_plain_self_is_refused_issue1723() {
    // `self.count += 1` inside `def bump(self)`: the generated receiver is a shared borrow, so the assignment can
    // only fail in the build. The checker names the method, the place it writes and the receiver to declare.
    let source = r#"
class Counter:
    count: int

    def bump(self) -> None:
        self.count += 1

    def reset(self) -> None:
        self.count = 0
"#;
    let messages = self_mutation_messages(source, "a plain-self method assigning to a field must be refused");
    assert_eq!(
        messages,
        vec![
            "Method 'bump' assigns to 'self.count' but takes 'self'".to_string(),
            "Method 'reset' assigns to 'self.count' but takes 'self'".to_string(),
        ]
    );
    let errors = check_str_err(source, "the refusal carries its remedy");
    assert!(
        errors
            .iter()
            .any(|error| error.hints.iter().any(|hint| hint.contains("def bump(mut self, ...)"))),
        "expected the hint to spell the receiver to declare, got: {:?}",
        errors.iter().map(|error| &error.hints).collect::<Vec<_>>()
    );
}

#[test]
fn class_method_calling_a_changing_collection_method_through_plain_self_is_refused_issue1723() {
    // `self.items.pop()` and `self.items.append(...)` change the list the receiver owns; `len(self.items)`,
    // `self.items.contains(...)` and an index read do not and stay accepted in a plain-`self` method.
    let source = r#"
class Stack:
    items: list[int]

    def pop(self) -> int:
        return self.items.pop()

    def push(self, item: int) -> None:
        self.items.append(item)

    def peek(self) -> int:
        return self.items[len(self.items) - 1]

    def has(self, item: int) -> bool:
        return self.items.contains(item)
"#;
    let messages = self_mutation_messages(source, "a plain-self method changing a field's list must be refused");
    assert_eq!(
        messages,
        vec![
            "Method 'pop' calls 'self.items.pop()', which changes 'self.items', but takes 'self'".to_string(),
            "Method 'push' calls 'self.items.append()', which changes 'self.items', but takes 'self'".to_string(),
        ]
    );
}

#[test]
fn index_and_tuple_writes_through_plain_self_are_refused_issue1723() {
    let source = r#"
class Grid:
    cells: list[int]
    width: int
    height: int

    def clear_first(self) -> None:
        self.cells[0] = 0

    def swap_dimensions(self) -> None:
        self.width, self.height = (self.height, self.width)
"#;
    let messages = self_mutation_messages(
        source,
        "index and tuple-target writes through plain self must be refused",
    );
    assert_eq!(
        messages,
        vec![
            "Method 'clear_first' assigns to 'self.cells[...]' but takes 'self'".to_string(),
            "Method 'swap_dimensions' assigns to 'self.width' but takes 'self'".to_string(),
            "Method 'swap_dimensions' assigns to 'self.height' but takes 'self'".to_string(),
        ]
    );
}

#[test]
fn mut_self_methods_and_local_collections_keep_writing_freely_issue1723() {
    // The same bodies under `mut self` are the documented form; a local collection in a plain-`self` method is not
    // a write through the receiver, and a `mut self` helper called from a `mut self` method is fine.
    assert_check_ok(
        r#"
class Stack:
    items: list[int]
    count: int

    def push(mut self, item: int) -> None:
        self.items.append(item)
        self.count += 1

    def pop(mut self) -> int:
        self.count -= 1
        return self.items.pop()

    def drain(mut self) -> int:
        mut total: int = 0
        while self.count > 0:
            total += self.pop()
        return total

    def doubled(self) -> list[int]:
        mut copy: list[int] = self.items.clone()
        copy.append(0)
        return copy

    def size(self) -> int:
        return self.count
"#,
    );
}

#[test]
fn calling_a_mut_self_method_through_plain_self_is_refused_issue1723() {
    let source = r#"
class Counter:
    count: int

    def bump(mut self) -> None:
        self.count += 1

    def bump_twice(self) -> None:
        self.bump()
        self.bump()
"#;
    let messages = self_mutation_messages(source, "a plain-self method calling a mut-self method must be refused");
    assert_eq!(
        messages,
        vec![
            "Method 'bump_twice' calls 'self.bump()', which changes 'self', but takes 'self'".to_string(),
            "Method 'bump_twice' calls 'self.bump()', which changes 'self', but takes 'self'".to_string(),
        ]
    );
}
