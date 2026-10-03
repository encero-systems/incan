//! Reference-side recording: direct, qualified and constructor calls, overload selection, closure parameters, match
//! narrowing, or-pattern and race bindings, constructor and pattern labels, `assert ... is`, fieldless enum values,
//! module-qualified type annotations (#1437), and unresolved annotations that must not fabricate an identity.

use super::*;

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
            binding.return_type == crate::symbols::ResolvedType::Int
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

/// A module-qualified type annotation (`errors.TomlError`) resolves to the declaration a direct import of the same
/// name binds, and records that proof for lowering (#1437).
///
/// The two spellings must agree in both facts a consumer reads: the resolved type of the annotated signature, so a
/// value checked against one spelling is compatible with the other, and the canonical identity at the reference
/// site, so nothing downstream has to re-resolve the dotted spelling. The recorded fact carries the module path the
/// spelling walked, which is what places the type in generated code without an import binding for its bare name.
#[test]
fn a_module_qualified_type_annotation_resolves_like_the_direct_import_issue1437() -> Result<(), String> {
    let errors = parse(
        "pub model TomlError:\n  pub message: str\n",
        "qualified type annotation provider",
    )?;
    let qualified_source = "import errors\n\npub def result() -> Result[None, errors.TomlError]:\n  return Ok(None)\n";
    let direct_source =
        "from errors import TomlError\n\npub def result() -> Result[None, TomlError]:\n  return Ok(None)\n";

    let mut qualified = TypeChecker::new();
    qualified.set_current_module_path(Some(vec!["consumer".to_string()]));
    qualified
        .check_with_imports(
            &parse(qualified_source, "qualified type annotation consumer")?,
            &[("errors", &errors)],
        )
        .map_err(|errors| format!("the qualified annotation should typecheck: {errors:?}"))?;
    let mut direct = TypeChecker::new();
    direct.set_current_module_path(Some(vec!["consumer".to_string()]));
    direct
        .check_with_imports(&parse(direct_source, "direct import consumer")?, &[("errors", &errors)])
        .map_err(|errors| format!("the direct import control should typecheck: {errors:?}"))?;

    let return_type = |checker: &TypeChecker, context: &str| {
        checker
            .type_info()
            .declarations
            .function_bindings
            .get("result")
            .map(|binding| binding.return_type.clone())
            .ok_or_else(|| format!("{context}: `result` must record a checked signature"))
    };
    assert_eq!(
        return_type(&qualified, "qualified")?,
        return_type(&direct, "direct")?,
        "both spellings must resolve the signature to the same checked type"
    );

    let reference = qualified
        .type_info()
        .qualified_type_reference("errors.TomlError")
        .ok_or("the checker must record the proof under the dotted spelling")?;
    let imported = direct
        .type_info()
        .resolved_import_identity("TomlError")
        .ok_or("the direct import must prove an identity")?;
    assert_eq!(&reference.identity, imported, "both spellings select one declaration");
    assert_eq!(reference.identity.kind, SemanticSourceTargetKind::Model);
    assert_eq!(
        reference.identity.origin,
        SymbolOrigin::Module(vec!["errors".to_string()])
    );
    assert_eq!(reference.module_path, vec!["errors".to_string()]);
    assert_eq!(
        identity_at(
            &qualified,
            nth_span(qualified_source, "errors.TomlError", 0)?,
            "qualified annotation reference"
        )?,
        *imported,
        "the reference site carries the same identity"
    );
    Ok(())
}

/// A dotted type spelling the checker cannot prove is refused at the annotation instead of resolving to `Unknown`.
///
/// Before #1437 such a spelling passed the checker silently and reached Rust emission as a dotted identifier, which
/// panicked. Each unsupported shape names what was wrong: a module that declares no such type, a module member that
/// is a function rather than a type, and a root that is not a module at all. The module is named by the path the
/// root is bound to, so an aliased import (`import errors as e`) is reported and hinted as `errors`, the spelling an
/// author can actually import from.
#[test]
fn a_module_qualified_spelling_that_names_no_type_is_refused_issue1437() -> Result<(), String> {
    let errors = parse(
        "pub model TomlError:\n  pub message: str\n\npub def make() -> TomlError:\n  return TomlError(message=\"x\")\n",
        "refused qualified type provider",
    )?;
    let cases = [
        (
            "import errors\n\ndef result() -> Result[None, errors.Missing]:\n  return Ok(None)\n",
            "`errors.Missing` is not a type: module `errors` declares no type or trait named `Missing`",
            Some("`from errors import Missing`"),
        ),
        (
            "import errors as e\n\ndef result() -> Result[None, e.Missing]:\n  return Ok(None)\n",
            "`e.Missing` is not a type: module `errors` declares no type or trait named `Missing`",
            Some("`from errors import Missing`"),
        ),
        (
            "import errors\n\ndef result() -> errors.make:\n  return errors.make()\n",
            "`errors.make` is not a type: module `errors` declares no type or trait named `make`",
            None,
        ),
        (
            "model Config:\n  value: int\n\ndef result() -> Config.Value:\n  return 1\n",
            "`Config.Value` is not a type: `Config` is not a module binding",
            None,
        ),
    ];
    for (source, expected, hint) in cases {
        let program = parse(source, "refused qualified type consumer")?;
        let mut checker = TypeChecker::new();
        checker.set_current_module_path(Some(vec!["consumer".to_string()]));
        let diagnostics = match checker.check_with_imports(&program, &[("errors", &errors)]) {
            Ok(()) => return Err(format!("the annotation must be refused:\n{source}")),
            Err(diagnostics) => diagnostics,
        };
        let refusals = diagnostics
            .iter()
            .filter(|error| error.message == expected)
            .collect::<Vec<_>>();
        assert_eq!(
            refusals.len(),
            1,
            "expected the refusal `{expected}` exactly once per annotation, got {diagnostics:?}"
        );
        if let Some(hint) = hint {
            assert!(
                refusals[0].hints.iter().any(|candidate| candidate.contains(hint)),
                "the hint must import from the bound module: {refusals:?}"
            );
        }
    }
    Ok(())
}

/// A module-qualified spelling of a source type alias denotes the alias target, as the direct import does (#1437).
///
/// `from errors import Id` registers the alias so expansion replaces `Id` with `int`; the qualified spelling has no
/// local registration, so the recorded fact carries the target itself. Without that, `errors.Id` stayed a nominal
/// `Id` and a value of type `int` was refused against it.
#[test]
fn a_module_qualified_type_alias_denotes_its_target_issue1437() -> Result<(), String> {
    let errors = parse("pub type Id = int\n", "qualified alias provider")?;
    let source = "import errors\n\ndef ident(value: errors.Id) -> int:\n  return value\n\ndef call() -> int:\n  return ident(1)\n";
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["consumer".to_string()]));
    checker
        .check_with_imports(&parse(source, "qualified alias consumer")?, &[("errors", &errors)])
        .map_err(|errors| format!("the qualified alias must check like the direct import: {errors:?}"))?;
    let reference = checker
        .type_info()
        .qualified_type_reference("errors.Id")
        .ok_or("the checker must record the proof under the dotted spelling")?;
    assert_eq!(reference.identity.kind, SemanticSourceTargetKind::TypeAlias);
    assert_eq!(
        reference.resolved,
        ResolvedType::Int,
        "the fact carries the alias target"
    );
    Ok(())
}

/// The C interop namespace is the one dotted root that is not a module, and its vocabulary spellings stay accepted
/// in ordinary annotations after #1437 refuses unprovable qualified types.
///
/// `from std.interop import c` binds `c` to the namespace's marker type, not to a module, and `c.i32` or
/// `c.ConstPtr[c.u8]` names a carrier the checked-binding facet interprets rather than a module member. A safe
/// facade over a binding writes those spellings in its own signature, so the refusal must not reach them; before
/// the qualified-type resolution they passed the checker, and they still must.
#[test]
fn c_vocabulary_spellings_in_ordinary_annotations_stay_accepted_issue1437() -> Result<(), String> {
    let source = concat!(
        "from std.interop import c\n",
        "\n",
        "def carrier(value: c.i32) -> c.i32:\n",
        "  return value\n",
        "\n",
        "def view(pointer: c.ConstPtr[c.u8]) -> None:\n",
        "  pass\n",
    );
    let program = parse(source, "C vocabulary facade")?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["consumer".to_string()]));
    checker
        .check_program(&program)
        .map_err(|errors| format!("the C vocabulary annotations must keep checking: {errors:?}"))?;
    assert!(
        checker.type_info().qualified_type_reference("c.i32").is_none(),
        "a vocabulary carrier is not a module member and records no qualified type proof"
    );
    Ok(())
}
