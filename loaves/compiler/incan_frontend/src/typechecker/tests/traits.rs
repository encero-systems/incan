//! Trait conformance for models and classes (#42): required fields and methods, default methods, signature mismatches,
//! supertrait closure and cycles, trait-typed receivers and upcasts, abstract methods, and the provider-metadata
//! recovery of an empty trait stub.

use super::*;

/// An empty trait stub must not hide the method contract that cached provider metadata carries.
///
/// An installed toolchain resolves a compiler-owned trait such as `Clone` to a symbol-table stub with no methods.
/// That stub shadows the real trait, so `@derive(Clone)` is left without a callable `clone` and every `.clone()` call
/// fails to resolve. Provider metadata still carries the signature, and method resolution recovers it from there.
///
/// The trait here is deliberately outside [`stdlib::STDLIB_TRAIT_METHOD_MODULES`]. The stdlib-source fallback covers
/// only registered traits and only inside a source checkout, so exercising `Clone` in-repo would resolve through that
/// fallback and pass whether or not the recovery works — which is exactly how this defect stayed invisible to a suite
/// that always runs against stdlib source. Keep this trait unregistered so the assertion has only one way to succeed.
#[test]
fn empty_trait_stub_recovers_its_method_contract_from_provider_metadata() {
    let mut checker = TypeChecker::new();
    let shadowed_trait = "ProviderOnlyContract".to_string();
    let span = Span::new(0, shadowed_trait.len());
    assert!(
        stdlib::trait_method_module_segments(&shadowed_trait).is_none(),
        "this trait must stay outside the stdlib-source fallback registry",
    );

    let empty_stub = TraitInfo {
        type_params: Vec::new(),
        supertraits: Vec::new(),
        methods: HashMap::new(),
        method_aliases: HashMap::new(),
        properties: HashMap::new(),
        requires: Vec::new(),
    };
    checker.symbols.define(Symbol {
        name: shadowed_trait.clone(),
        kind: SymbolKind::Trait(empty_stub.clone()),
        span,
        scope: 0,
    });
    assert!(
        checker
            .lookup_semantic_trait_info(&shadowed_trait)
            .is_some_and(|info| info.methods.is_empty()),
        "the shadowing stub must be what trait lookup sees, or this test proves nothing",
    );

    let mut provider_methods = HashMap::new();
    provider_methods.insert(
        "materialize".to_string(),
        MethodInfo {
            identity: None,
            type_params: Vec::new(),
            type_param_bounds: HashMap::new(),
            type_param_bound_details: HashMap::new(),
            trait_target: None,
            receiver: Some(Receiver::Immutable),
            params: Vec::new(),
            return_type: ResolvedType::SelfType,
            is_async: false,
            has_body: false,
            alias_of: None,
        },
    );
    checker.transitive_stdlib_stub_traits.insert(
        shadowed_trait.clone(),
        TraitInfo {
            methods: provider_methods,
            ..empty_stub
        },
    );

    let resolved = checker.trait_method_info_resolved(&shadowed_trait, "materialize", span);
    assert!(
        resolved.is_some_and(|info| info.return_type == ResolvedType::SelfType),
        "provider metadata must supply the contract the empty stub lacks",
    );
}

#[test]
fn test_ellipsis_abstract_method_outside_trait_is_type_error() {
    let source = r#"
model User:
  def name(self) -> str: ...
"#;
    let errs = check_str_err(source, "abstract concrete method should fail typechecking");
    assert!(
        errs.iter().any(|err| err
            .message
            .contains("Method 'name' must have a body outside trait declarations")),
        "expected concrete method body diagnostic, got: {errs:?}"
    );
}

#[test]
fn test_model_trait_requires_missing_field_errors() {
    let source = r#"
@requires(name: str)
trait Loggable:
  def log(self, msg: str) -> None:
    println(f"[{self.name}] {msg}")

model User with Loggable:
  id: int
"#;
    assert!(check_str(source).is_err());
}

#[test]
fn test_class_trait_requires_missing_field_errors() {
    let source = r#"
@requires(name: str)
trait Loggable:
  def log(self, msg: str) -> None:
    println(f"[{self.name}] {msg}")

class Service with Loggable:
  id: int
"#;
    assert!(check_str(source).is_err());
}

#[test]
fn test_model_trait_requires_field_type_mismatch_errors() {
    let source = r#"
@requires(name: str)
trait Loggable:
  def log(self, msg: str) -> None:
    println(f"[{self.name}] {msg}")

model User with Loggable:
  name: int
"#;
    assert!(check_str(source).is_err());
}

#[test]
fn test_model_trait_default_method_call_typechecks() {
    let source = r#"
@requires(name: str)
trait Loggable:
  def log(self, msg: str) -> None:
    println(f"[{self.name}] {msg}")

model User with Loggable:
  name: str

def main() -> None:
  u = User(name="Ada")
  u.log("hello")
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_class_trait_default_method_call_typechecks() {
    let source = r#"
@requires(name: str)
trait Loggable:
  def log(self, msg: str) -> None:
    println(f"[{self.name}] {msg}")

class Service with Loggable:
  name: str

def main() -> None:
  s = Service(name="svc")
  s.log("hello")
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_trait_duplicate_requires_errors() {
    let source = r#"
@requires(name: str, name: str)
trait Dup:
  def get(self) -> str:
    return self.name
"#;
    assert!(check_str(source).is_err());
}

#[test]
fn test_trait_default_method_assignment_requires_declared_field() {
    let source = r#"
trait Counter:
  def bump(mut self) -> None:
    self.count += 1

class Thing with Counter:
  count: int = 0
"#;
    assert!(check_str(source).is_err());
}

#[test]
fn test_trait_default_method_requires_declared_field() {
    let source = r#"
trait Greeter:
  def greet(self) -> str:
    return self.name

class User with Greeter:
  name: str
"#;
    assert!(check_str(source).is_err());
}

#[test]
fn test_trait_default_method_allows_required_field_assignment() {
    let source = r#"
@requires(count: int)
trait Counter:
  def bump(mut self) -> None:
    self.count = self.count + 1

class CounterImpl with Counter:
  count: int

def main() -> None:
  c = CounterImpl(count=1)
  c.bump()
"#;
    assert_check_ok(source);
}

#[test]
fn test_trait_required_method_signature_mismatch_receiver() {
    let source = r#"
trait Inc:
  def inc(mut self, by: int) -> int: ...

class Bad with Inc:
  value: int

  def inc(self, by: int) -> int:
    return self.value
"#;
    assert!(check_str(source).is_err());
}

#[test]
fn test_trait_required_method_signature_mismatch_param_type() {
    let source = r#"
trait Inc:
  def inc(mut self, by: int) -> int: ...

class Bad with Inc:
  value: int

  def inc(mut self, by: str) -> int:
    return self.value
"#;
    assert!(check_str(source).is_err());
}

#[test]
fn test_trait_required_method_signature_mismatch_return_type() {
    let source = r#"
trait Inc:
  def inc(mut self, by: int) -> int: ...

class Bad with Inc:
  value: int

  def inc(mut self, by: int) -> None:
    return None
"#;
    assert!(check_str(source).is_err());
}

#[test]
fn test_trait_required_method_signature_mismatch_async() {
    let source = r#"
trait Inc:
  async def inc(mut self, by: int) -> int: ...

class Bad with Inc:
  value: int

  def inc(mut self, by: int) -> int:
    return self.value
"#;
    assert!(check_str(source).is_err());
}

#[test]
fn test_trait_required_method_alias_with_exact_signature_is_accepted_issue1055() {
    let source = r#"
trait Renamable:
  def where(self, value: int) -> int: ...

class Example with Renamable:
  where = alias filter

  def filter(self, value: int) -> int:
    return value
"#;
    assert_check_ok(source);
}

#[test]
fn test_trait_required_method_alias_with_incompatible_signature_is_rejected_issue1055() {
    let source = r#"
trait Renamable:
  def where(self, value: int) -> int: ...

class BadExample with Renamable:
  where = alias filter

  def filter(self, value: str) -> int:
    return 0
"#;
    assert!(check_str(source).is_err());
}

#[test]
fn test_trait_required_method_allows_nested_self_return_for_generic_model() {
    let source = r#"
model Wrapper[T]:
  value: T

trait Wraps:
  def wrap(self) -> Wrapper[Self]: ...

model Reader[T] with Wraps:
  value: T

  def wrap(self) -> Wrapper[Reader[T]]:
    return Wrapper(value=self)
"#;
    assert_check_ok(source);
}

#[test]
fn test_trait_conformance_allows_inherited_members() {
    let source = r#"
@requires(name: str)
trait Named:
  def get_name(self) -> str: ...

class Base:
  name: str

  def get_name(self) -> str:
    return self.name

class Child extends Base with Named:
  name: str
"#;
    assert_check_ok(source);
}

#[test]
fn test_trait_requires_field_type_checked_for_class() {
    let source = r#"
@requires(name: str)
trait Named:
  def get_name(self) -> str: ...

class Bad with Named:
  name: int

  def get_name(self) -> str:
    return "x"
"#;
    assert!(check_str(source).is_err());
}

// RFC 042: supertrait graph (symbol collection + transitive closure)
#[test]
fn test_supertrait_cycle_is_diagnosed() -> Result<(), Vec<CompileError>> {
    let source = r#"
trait A with B:
  def fa(self) -> int: ...

trait B with A:
  def fb(self) -> int: ...
"#;
    let tokens = lexer::lex(source)?;
    let ast = parser::parse(&tokens)?;
    let mut checker = TypeChecker::new();
    let Err(errs) = checker.check_program(&ast) else {
        panic!("expected supertrait cycle to be rejected");
    };
    assert!(
        errs.iter().any(|e| e.message.contains("Supertrait cycle")),
        "unexpected errors: {:?}",
        errs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
    Ok(())
}

#[test]
fn test_supertrait_transitive_closure() -> Result<(), Vec<CompileError>> {
    let source = r#"
trait Root:
  def root_m(self) -> int: ...

trait Mid with Root:
  def mid_m(self) -> int: ...

trait Leaf with Mid:
  def leaf_m(self) -> int: ...
"#;
    let tokens = lexer::lex(source)?;
    let ast = parser::parse(&tokens)?;
    let mut checker = TypeChecker::new();
    checker.check_program(&ast)?;
    let Some(leaf) = checker.supertrait_closure.get("Leaf") else {
        return Err(vec![CompileError::type_error(
            "Leaf should have a supertrait closure".to_string(),
            Span::default(),
        )]);
    };
    assert!(
        leaf.iter().any(|(n, _)| n == "Mid"),
        "expected Mid in Leaf closure, got {:?}",
        leaf
    );
    assert!(
        leaf.iter().any(|(n, _)| n == "Root"),
        "expected Root in Leaf closure, got {:?}",
        leaf
    );
    Ok(())
}

#[test]
fn test_supertrait_bound_rejects_non_trait_type() -> Result<(), Vec<CompileError>> {
    let source = r#"
model M:
  x: int

trait T with M:
  def f(self) -> int: ...
"#;
    let tokens = lexer::lex(source)?;
    let ast = parser::parse(&tokens)?;
    let mut checker = TypeChecker::new();
    let Err(errs) = checker.check_program(&ast) else {
        panic!("expected errors for non-trait supertrait bound");
    };
    assert!(
        errs.iter().any(|e| e.message.contains("is not a trait")),
        "unexpected errors: {:?}",
        errs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
    Ok(())
}

#[test]
fn test_supertrait_bound_rejects_arity_mismatch() -> Result<(), Vec<CompileError>> {
    let source = r#"
trait Boxed[T]:
  def get(self) -> T: ...

trait Bad with Boxed:
  def run(self) -> int: ...
"#;
    let tokens = lexer::lex(source)?;
    let ast = parser::parse(&tokens)?;
    let mut checker = TypeChecker::new();
    let Err(errs) = checker.check_program(&ast) else {
        panic!("expected errors for supertrait arity mismatch");
    };
    assert!(
        errs.iter().any(|e| e.message.contains("expects 1 type argument")),
        "unexpected errors: {:?}",
        errs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
    Ok(())
}

#[test]
fn test_generic_supertrait_cycle_is_diagnosed() -> Result<(), Vec<CompileError>> {
    let source = r#"
trait A[T] with A[list[T]]:
  def f(self) -> int: ...
"#;
    let tokens = lexer::lex(source)?;
    let ast = parser::parse(&tokens)?;
    let mut checker = TypeChecker::new();
    let Err(errs) = checker.check_program(&ast) else {
        panic!("expected generic supertrait cycle to be rejected");
    };
    assert!(
        errs.iter().any(|e| e.message.contains("Supertrait cycle")),
        "unexpected errors: {:?}",
        errs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
    Ok(())
}

// RFC 042 Phase 3: assignability, conformance, `@requires` merge, trait construction, diamond diagnostics
#[test]
fn test_type_implements_trait_includes_transitive_supertraits() -> Result<(), Vec<CompileError>> {
    let source = r#"
trait Root:
  def r(self) -> int: ...

trait Mid with Root:
  def m(self) -> int: ...

trait Leaf with Mid:
  def l(self) -> int: ...

model M with Leaf:
  def r(self) -> int:
    return 0
  def m(self) -> int:
    return 0
  def l(self) -> int:
    return 0
"#;
    let tokens = lexer::lex(source)?;
    let ast = parser::parse(&tokens)?;
    let mut checker = TypeChecker::new();
    checker.check_program(&ast)?;
    assert!(checker.type_implements_trait("M", "Leaf"));
    assert!(checker.type_implements_trait("M", "Mid"));
    assert!(checker.type_implements_trait("M", "Root"));
    Ok(())
}

#[test]
fn test_types_compatible_generic_trait_annotation() -> Result<(), Vec<CompileError>> {
    let source = r#"
trait Boxed[T]:
  def get(self) -> T: ...

model Cell[T] with Boxed:
  value: T

  def get(self) -> T:
    return self.value
"#;
    let tokens = lexer::lex(source)?;
    let ast = parser::parse(&tokens)?;
    let mut checker = TypeChecker::new();
    checker.check_program(&ast)?;
    let actual = ResolvedType::Generic("Cell".to_string(), vec![ResolvedType::Int]);
    let expected = ResolvedType::Generic("Boxed".to_string(), vec![ResolvedType::Int]);
    assert!(
        checker.types_compatible(&actual, &expected),
        "Generic concrete type should be assignable to matching generic trait annotation (RFC 042)"
    );
    Ok(())
}

#[test]
fn test_trait_typed_local_annotation_is_rejected() {
    let source = r#"
trait Boxed[T]:
  def get(self) -> T: ...
  def keep(self) -> Self: ...

model Item:
  value: int

class ValueBox[T] with Boxed:
  value: T

  def get(self) -> T:
    return self.value

  def keep(self) -> Self:
    return self

def use_trait_typed_value() -> Item:
  concrete: ValueBox[Item] = ValueBox[Item](value=Item(value=7))
  boxed: Boxed[Item] = concrete
  return boxed.get()
"#;

    let errs = check_str_err(
        source,
        "trait-typed local annotation should be rejected before Rust codegen",
    );
    assert!(
        errs.iter().any(|e| e
            .message
            .contains("Trait-typed local annotation 'Boxed[Item]' is not supported")),
        "expected unsupported trait-typed local diagnostic, got {:?}",
        errs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_types_compatible_generic_trait_annotation_extra_concrete_type_params() -> Result<(), Vec<CompileError>> {
    let source = r#"
trait Boxed[T]:
  def get(self) -> T: ...

model Pair[A, B] with Boxed:
  first: A
  second: B

  def get(self) -> A:
    return self.first
"#;
    let tokens = lexer::lex(source)?;
    let ast = parser::parse(&tokens)?;
    let mut checker = TypeChecker::new();
    checker.check_program(&ast)?;

    let ok_actual = ResolvedType::Generic("Pair".to_string(), vec![ResolvedType::Int, ResolvedType::Str]);
    let expected = ResolvedType::Generic("Boxed".to_string(), vec![ResolvedType::Int]);
    assert!(
        checker.types_compatible(&ok_actual, &expected),
        "Concrete type with more type parameters than the trait should still match when leading args align (RFC 042)"
    );

    let bad_actual = ResolvedType::Generic("Pair".to_string(), vec![ResolvedType::Str, ResolvedType::Int]);
    assert!(
        !checker.types_compatible(&bad_actual, &expected),
        "First concrete type parameter must be compatible with the trait's type argument"
    );

    let short_actual = ResolvedType::Generic("Pair".to_string(), vec![ResolvedType::Int]);
    assert!(
        !checker.types_compatible(&short_actual, &expected),
        "Concrete type must supply at least as many type arguments as the trait annotation"
    );
    Ok(())
}

#[test]
fn test_types_compatible_generic_supertrait_annotation() -> Result<(), Vec<CompileError>> {
    let source = r#"
trait Collection[T]:
  def first(self) -> T: ...

trait OrderedCollection[T] with Collection[T]:
  def sorted(self) -> Self: ...

model BoxedValue[T] with OrderedCollection:
  value: T

  def first(self) -> T:
    return self.value

  def sorted(self) -> Self:
    return self
"#;
    let tokens = lexer::lex(source)?;
    let ast = parser::parse(&tokens)?;
    let mut checker = TypeChecker::new();
    checker.check_program(&ast)?;
    let actual = ResolvedType::Generic("BoxedValue".to_string(), vec![ResolvedType::Int]);
    let expected = ResolvedType::Generic("Collection".to_string(), vec![ResolvedType::Int]);
    assert!(
        checker.types_compatible(&actual, &expected),
        "Generic adopters should satisfy transitive generic supertrait annotations with substituted args"
    );
    Ok(())
}

#[test]
fn test_trait_typed_receiver_exposes_trait_and_supertrait_methods_issue817() -> Result<(), Vec<CompileError>> {
    let source = r#"
trait DataSet[T]:
  def filter(self) -> Self: ...

trait BoundedDataSet[T] with DataSet[T]:
  def limit(self, n: int) -> Self: ...

trait UnboundedDataSet[T] with DataSet[T]:
  pass

model Event:
  id: int

def valid_root_call(events: DataSet[Event]) -> DataSet[Event]:
  return events.filter()

def valid_subtrait_call(events: BoundedDataSet[Event]) -> BoundedDataSet[Event]:
  return events.limit(10)

def valid_subtrait_supertrait_call(events: UnboundedDataSet[Event]) -> UnboundedDataSet[Event]:
  return events.filter()
"#;

    check_str(source)
}

#[test]
fn test_trait_typed_receiver_rejects_subtrait_only_method_issue817() {
    let source = r#"
trait DataSet[T]:
  def filter(self) -> Self: ...

trait BoundedDataSet[T] with DataSet[T]:
  def limit(self, n: int) -> Self: ...

model Event:
  id: int

def invalid_root_call(events: DataSet[Event]) -> DataSet[Event]:
  return events.limit(10)
"#;

    let errs = check_str_err(
        source,
        "trait-typed receiver should not expose methods declared only on narrower subtraits",
    );
    assert!(
        errs.iter()
            .any(|e| e.message.contains("DataSet[Event]") && e.message.contains("limit")),
        "expected missing method diagnostic for subtrait-only method, got {:?}",
        errs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_types_compatible_named_concrete_rejects_mismatched_generic_trait_annotation() -> Result<(), Vec<CompileError>> {
    let source = r#"
trait Boxed[T]:
  def get(self) -> T: ...

model IntBox with Boxed:
  value: int

  def get(self) -> int:
    return self.value
"#;
    let tokens = lexer::lex(source)?;
    let ast = parser::parse(&tokens)?;
    let mut checker = TypeChecker::new();
    checker.check_program(&ast)?;
    let actual = ResolvedType::Named("IntBox".to_string());
    let expected = ResolvedType::Generic("Boxed".to_string(), vec![ResolvedType::Str]);
    assert!(
        !checker.types_compatible(&actual, &expected),
        "Non-generic adopters must not silently satisfy arbitrary generic trait instantiations"
    );
    Ok(())
}

// RFC 042: trait-typed value assignable to supertrait (trait-to-trait upcasts)
#[test]
fn test_types_compatible_trait_to_supertrait_named() -> Result<(), Vec<CompileError>> {
    let source = r#"
trait Root:
  def root_m(self) -> int: ...

trait Mid with Root:
  def mid_m(self) -> int: ...
"#;
    let tokens = lexer::lex(source)?;
    let ast = parser::parse(&tokens)?;
    let mut checker = TypeChecker::new();
    checker.check_program(&ast)?;
    let actual = ResolvedType::Named("Mid".to_string());
    let expected = ResolvedType::Named("Root".to_string());
    assert!(
        checker.types_compatible(&actual, &expected),
        "Named subtrait should be assignable to supertrait (RFC 042)"
    );
    Ok(())
}

#[test]
fn test_types_compatible_trait_to_supertrait_generic() -> Result<(), Vec<CompileError>> {
    let source = r#"
trait DataSet[T]:
  def id(self) -> T: ...

trait BoundedDataSet[T] with DataSet[T]:
  def bounded(self) -> T: ...
"#;
    let tokens = lexer::lex(source)?;
    let ast = parser::parse(&tokens)?;
    let mut checker = TypeChecker::new();
    checker.check_program(&ast)?;
    let actual = ResolvedType::Generic("BoundedDataSet".to_string(), vec![ResolvedType::Int]);
    let expected = ResolvedType::Generic("DataSet".to_string(), vec![ResolvedType::Int]);
    assert!(
        checker.types_compatible(&actual, &expected),
        "Generic subtrait[T] should be assignable to supertrait[T] with compatible args"
    );
    Ok(())
}

#[test]
fn test_types_compatible_trait_to_supertrait_transitive_generic() -> Result<(), Vec<CompileError>> {
    let source = r#"
trait Root[T]:
  def r(self) -> T: ...

trait Mid[T] with Root[T]:
  def m(self) -> T: ...

trait Leaf[T] with Mid[T]:
  def l(self) -> T: ...
"#;
    let tokens = lexer::lex(source)?;
    let ast = parser::parse(&tokens)?;
    let mut checker = TypeChecker::new();
    checker.check_program(&ast)?;
    let actual = ResolvedType::Generic("Leaf".to_string(), vec![ResolvedType::Int]);
    let expected = ResolvedType::Generic("Root".to_string(), vec![ResolvedType::Int]);
    assert!(
        checker.types_compatible(&actual, &expected),
        "Transitive supertrait generics should substitute through the chain"
    );
    Ok(())
}

#[test]
fn test_types_compatible_concrete_to_transitive_supertrait_generic() -> Result<(), Vec<CompileError>> {
    let source = r#"
trait DataSet[T]:
  def filter(self, _p: bool) -> Self: ...

trait BoundedDataSet[T] with DataSet[T]:
  def bounded_marker(self) -> T: ...

class DataFrame[T] with BoundedDataSet:
  _row_schema_marker: T

  def filter(self, _p: bool) -> Self:
    return self

  def bounded_marker(self) -> T:
    return self._row_schema_marker

model Order:
  id: int
"#;
    let tokens = lexer::lex(source)?;
    let ast = parser::parse(&tokens)?;
    let mut checker = TypeChecker::new();
    checker.check_program(&ast)?;
    let order = ResolvedType::Named("Order".to_string());
    let actual = ResolvedType::Generic("DataFrame".to_string(), vec![order.clone()]);
    let expected = ResolvedType::Generic("DataSet".to_string(), vec![order]);
    assert!(
        checker.types_compatible(&actual, &expected),
        "Concrete class through intermediate trait should satisfy transitive supertrait"
    );
    Ok(())
}

#[test]
fn test_types_compatible_trait_to_supertrait_identity() -> Result<(), Vec<CompileError>> {
    let source = r#"
trait DataSet[T]:
  def id(self) -> T: ...
"#;
    let tokens = lexer::lex(source)?;
    let ast = parser::parse(&tokens)?;
    let mut checker = TypeChecker::new();
    checker.check_program(&ast)?;
    let actual = ResolvedType::Generic("DataSet".to_string(), vec![ResolvedType::Str]);
    let expected = ResolvedType::Generic("DataSet".to_string(), vec![ResolvedType::Str]);
    assert!(checker.types_compatible(&actual, &expected));
    Ok(())
}

#[test]
fn test_types_compatible_trait_to_supertrait_wrong_args() -> Result<(), Vec<CompileError>> {
    let source = r#"
trait DataSet[T]:
  def id(self) -> T: ...

trait BoundedDataSet[T] with DataSet[T]:
  def b(self) -> T: ...
"#;
    let tokens = lexer::lex(source)?;
    let ast = parser::parse(&tokens)?;
    let mut checker = TypeChecker::new();
    checker.check_program(&ast)?;
    let actual = ResolvedType::Generic("BoundedDataSet".to_string(), vec![ResolvedType::Int]);
    let expected = ResolvedType::Generic("DataSet".to_string(), vec![ResolvedType::Str]);
    assert!(
        !checker.types_compatible(&actual, &expected),
        "Mismatched type arguments across trait upcast must be rejected"
    );
    Ok(())
}

#[test]
fn test_types_compatible_unrelated_traits_rejected() -> Result<(), Vec<CompileError>> {
    let source = r#"
trait Apple[T]:
  def a(self) -> T: ...

trait Orange[T]:
  def o(self) -> T: ...
"#;
    let tokens = lexer::lex(source)?;
    let ast = parser::parse(&tokens)?;
    let mut checker = TypeChecker::new();
    checker.check_program(&ast)?;
    let actual = ResolvedType::Generic("Apple".to_string(), vec![ResolvedType::Int]);
    let expected = ResolvedType::Generic("Orange".to_string(), vec![ResolvedType::Int]);
    assert!(!checker.types_compatible(&actual, &expected));
    Ok(())
}

#[test]
fn test_types_compatible_wrong_direction_supertrait_to_subtrait_rejected() -> Result<(), Vec<CompileError>> {
    let source = r#"
trait DataSet[T]:
  def id(self) -> T: ...

trait BoundedDataSet[T] with DataSet[T]:
  def b(self) -> T: ...
"#;
    let tokens = lexer::lex(source)?;
    let ast = parser::parse(&tokens)?;
    let mut checker = TypeChecker::new();
    checker.check_program(&ast)?;
    let actual = ResolvedType::Generic("DataSet".to_string(), vec![ResolvedType::Int]);
    let expected = ResolvedType::Generic("BoundedDataSet".to_string(), vec![ResolvedType::Int]);
    assert!(
        !checker.types_compatible(&actual, &expected),
        "Supertrait must not be assignable to subtrait"
    );
    Ok(())
}

#[test]
fn test_supertrait_requires_merge_conflict() -> Result<(), Vec<CompileError>> {
    let source = r#"
@requires(x: int)
trait A:
  def fa(self) -> int: ...

@requires(x: str)
trait B:
  def fb(self) -> str: ...

trait C with A, B:
  def fc(self) -> int: ...
"#;
    let tokens = lexer::lex(source)?;
    let ast = parser::parse(&tokens)?;
    let mut checker = TypeChecker::new();
    let Err(errs) = checker.check_program(&ast) else {
        panic!("expected @requires merge conflict");
    };
    assert!(
        errs.iter().any(|e| e.message.contains("merges conflicting @requires")),
        "unexpected errors: {:?}",
        errs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
    Ok(())
}

#[test]
fn test_cannot_instantiate_trait() {
    let source = r#"
trait T:
  def f(self) -> int: ...

def main() -> int:
  let _x = T()
  return 0
"#;
    let err = check_str_err(source, "trait constructor should be rejected");
    assert!(
        err.iter().any(|e| e.message.contains("Cannot construct trait")),
        "unexpected errors: {:?}",
        err.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_supertrait_incompatible_method_conflict() -> Result<(), Vec<CompileError>> {
    let source = r#"
trait A:
  def m(self) -> int: ...

trait B:
  def m(self) -> str: ...

trait C with A, B:
  def c(self) -> int: ...

model M with C:
  def c(self) -> int:
    return 0
"#;
    let tokens = lexer::lex(source)?;
    let ast = parser::parse(&tokens)?;
    let mut checker = TypeChecker::new();
    let Err(errs) = checker.check_program(&ast) else {
        panic!("expected conflicting supertrait method requirements");
    };
    assert!(
        errs.iter().any(|e| e.message.contains("Conflicting implementations")),
        "unexpected errors: {:?}",
        errs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
    Ok(())
}

#[test]
fn test_supertrait_method_ambiguity_param_name_only() -> Result<(), Vec<CompileError>> {
    let source = r#"
trait A:
  def m(self, a: int) -> int: ...

trait B:
  def m(self, b: int) -> int: ...

trait C with A, B:
  def c(self) -> int: ...

model M with C:
  def c(self) -> int:
    return 0
"#;
    let tokens = lexer::lex(source)?;
    let ast = parser::parse(&tokens)?;
    let mut checker = TypeChecker::new();
    let Err(errs) = checker.check_program(&ast) else {
        panic!("expected ambiguous supertrait method");
    };
    assert!(
        errs.iter().any(|e| e.message.contains("Ambiguous trait method")),
        "unexpected errors: {:?}",
        errs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
    Ok(())
}

#[test]
fn test_transitive_supertrait_abstract_method_required() -> Result<(), Vec<CompileError>> {
    let source = r#"
trait Root:
  def root_only(self) -> int: ...

trait Leaf with Root:
  def leaf_m(self) -> int: ...

model M with Leaf:
  def leaf_m(self) -> int:
    return 1
"#;
    let tokens = lexer::lex(source)?;
    let ast = parser::parse(&tokens)?;
    let mut checker = TypeChecker::new();
    let Err(errs) = checker.check_program(&ast) else {
        panic!("expected missing transitive supertrait method");
    };
    assert!(
        errs.iter().any(|e| e.message.contains("requires method 'root_only'")),
        "unexpected errors: {:?}",
        errs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
    Ok(())
}

// ---- #1723: trait implementations and default methods honor the receiver the same way ----

#[test]
fn trait_implementation_mutating_through_plain_self_is_refused_issue1723() {
    // The program from #1723: the trait declares `resize(self, ...)`, the class implements it and scales its
    // fields. The class method is the one that writes, so it is the one refused; the trait declaration itself
    // has no body to refuse.
    let source = r#"
trait Resizable:
    def resize(self, factor: float) -> None

class Carton with Resizable:
    width: float

    def resize(self, factor: float) -> None:
        self.width *= factor

def main() -> None:
    mut carton: Carton = Carton(width=2.0)
    carton.resize(3.0)
    println(carton.width)
"#;
    let errors = check_str_err(
        source,
        "a trait implementation writing through plain self must be refused",
    );
    let refused = errors
        .iter()
        .filter(|error| error.stable_code() == Some("INCAN-T0102"))
        .map(|error| error.message.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        refused,
        vec!["Method 'resize' assigns to 'self.width' but takes 'self'"]
    );

    // Declaring `mut self` on both the trait and the implementation is the accepted form. `width` is `pub` here
    // because `main` reads it, and a class field is private to the class's own methods by default.
    assert_check_ok(
        r#"
trait Resizable:
    def resize(mut self, factor: float) -> None

class Carton with Resizable:
    pub width: float

    def resize(mut self, factor: float) -> None:
        self.width *= factor

def main() -> None:
    mut carton: Carton = Carton(width=2.0)
    carton.resize(3.0)
    println(carton.width)
"#,
    );
}

#[test]
fn trait_default_method_mutating_a_required_field_through_plain_self_is_refused_issue1723() {
    let source = r#"
@requires(count: int)
trait Countable:
    def bump(self) -> None:
        self.count += 1

    def bump_twice(mut self) -> None:
        self.bump()
"#;
    let errors = check_str_err(
        source,
        "a trait default method writing through plain self must be refused",
    );
    let refused = errors
        .iter()
        .filter(|error| error.stable_code() == Some("INCAN-T0102"))
        .map(|error| error.message.as_str())
        .collect::<Vec<_>>();
    assert_eq!(refused, vec!["Method 'bump' assigns to 'self.count' but takes 'self'"]);
}
