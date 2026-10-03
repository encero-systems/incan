//! Declaration formatting: models and `pub` field visibility, traits with supertraits and abstract methods, decorators
//! and decorator factories, long function, method and class headers, generic methods, method trait targets, constrained
//! primitive newtypes, `mut` parameters, `rust.module()` and enum methods, class, model, enum and trait body
//! docstrings, value enums, module docstrings before `const` / `static`, rich newtypes, `rusttype` associated types,
//! computed properties and partial declarations.

use super::*;

fn assert_decl_block_docstring_markers(doc: Option<&str>, context: &str) -> Result<(), FormatError> {
    let Some(doc) = doc else {
        return Err(FormatError::SyntaxError(format!(
            "{context}: expected declaration body docstring for API/tooling extraction"
        )));
    };
    let trimmed = doc.trim();
    if !trimmed.contains("Line A documents the class API.") {
        return Err(FormatError::SyntaxError(format!(
            "{context}: docstring missing marker line A: {trimmed:?}"
        )));
    }
    if !trimmed.contains("Line B keeps interior newlines after trim().") {
        return Err(FormatError::SyntaxError(format!(
            "{context}: docstring missing marker line B: {trimmed:?}"
        )));
    }
    Ok(())
}

fn assert_first_class_decl_has_marker_docstring(program: &Program, context: &str) -> Result<(), FormatError> {
    let class = match &program.declarations[0].node {
        Declaration::Class(c) => c,
        other => {
            return Err(FormatError::SyntaxError(format!(
                "{context}: expected class declaration, got {other:?}"
            )));
        }
    };
    assert_decl_block_docstring_markers(class.docstring.as_deref(), context)
}

fn assert_first_model_decl_has_marker_docstring(program: &Program, context: &str) -> Result<(), FormatError> {
    let model = match &program.declarations[0].node {
        Declaration::Model(m) => m,
        other => {
            return Err(FormatError::SyntaxError(format!(
                "{context}: expected model declaration, got {other:?}"
            )));
        }
    };
    assert_decl_block_docstring_markers(model.docstring.as_deref(), context)
}

fn assert_first_enum_decl_has_marker_docstring(program: &Program, context: &str) -> Result<(), FormatError> {
    let en = match &program.declarations[0].node {
        Declaration::Enum(e) => e,
        other => {
            return Err(FormatError::SyntaxError(format!(
                "{context}: expected enum declaration, got {other:?}"
            )));
        }
    };
    assert_decl_block_docstring_markers(en.docstring.as_deref(), context)
}

fn assert_first_trait_decl_has_marker_docstring(program: &Program, context: &str) -> Result<(), FormatError> {
    let tr = match &program.declarations[0].node {
        Declaration::Trait(t) => t,
        other => {
            return Err(FormatError::SyntaxError(format!(
                "{context}: expected trait declaration, got {other:?}"
            )));
        }
    };
    assert_decl_block_docstring_markers(tr.docstring.as_deref(), context)
}

#[test]
fn test_format_source_preserves_registry_description_decorator() -> Result<(), FormatError> {
    let source = r#"from std.registry import Registry, SubjectKind, describe

type FunctionId = newtype str

@derive(Descriptor)
model FunctionSpec:
  summary: str

static functions: Registry[FunctionId, FunctionSpec] = Registry.define(subjects=[SubjectKind.Function])

@describe(functions,FunctionId("normalize"),FunctionSpec(summary="Normalize text"))
def normalize(value: str) -> str:
  return value
"#;
    let formatted = format_source(source)?;
    assert!(
        formatted.contains("@describe(functions, FunctionId(\"normalize\"), FunctionSpec(summary=\"Normalize text\"))"),
        "expected @describe to retain its ordinary typed arguments, got:\n{formatted}"
    );
    assert_eq!(format_source(&formatted)?, formatted);
    Ok(())
}

#[test]
fn test_format_source_model() {
    let source = r#"model User:
  name: str
  age: int
"#;
    let result = format_source(source);
    assert!(result.is_ok());
}

#[test]
fn test_format_source_preserves_ordinary_pub_model_field_visibility_issue884() -> Result<(), FormatError> {
    let source = r#"pub model SealedEnvelope:
  body: str
  pub envelope_id: str
"#;
    let formatted = format_source(source)?;
    let expected = r#"pub model SealedEnvelope:
    body: str
    pub envelope_id: str
"#;
    assert_eq!(formatted, expected);
    assert_eq!(format_source(&formatted)?, formatted);
    Ok(())
}

#[test]
fn test_format_source_trait_with_supertraits() -> Result<(), FormatError> {
    let source = r#"trait OrderedCollection[T] with Collection[T], Serializable:
  def sorted(self) -> OrderedCollection[T]: ...
"#;
    let formatted = format_source(source)?;
    let expected = r#"trait OrderedCollection[T] with Collection[T], Serializable:
    def sorted(self) -> OrderedCollection[T]
"#;
    assert_eq!(formatted, expected);
    Ok(())
}

#[test]
fn test_format_source_rust_allow_decorator() -> Result<(), FormatError> {
    let source = r#"@rust.allow("dead_code","clippy::too_many_arguments")
def MixedName() -> int:
  return 1
"#;
    let formatted = format_source(source)?;
    let expected = r#"@rust.allow("dead_code", "clippy::too_many_arguments")
def MixedName() -> int:
    return 1
"#;
    assert_eq!(formatted, expected);
    Ok(())
}

#[test]
fn test_format_source_decorator_factory_type_args() -> Result<(), FormatError> {
    let source = r#"@registered[(str)->ColumnExpr]("incql.functions.col")
def col(name: str) -> ColumnExpr:
  return ColumnExpr(name=name)
"#;
    let formatted = format_source(source)?;
    let expected = r#"@registered[(str) -> ColumnExpr]("incql.functions.col")
def col(name: str) -> ColumnExpr:
    return ColumnExpr(name=name)
"#;
    assert_eq!(formatted, expected);
    Ok(())
}

#[test]
fn test_format_source_wraps_long_function_signature() -> Result<(), FormatError> {
    let source = r#"def append_node(store_id: int, kind: PrismNodeKind, input_ids: list[int], named_table: str, predicate: bool, limit_count: int) -> int:
  return 1
"#;
    let formatted = format_source(source)?;
    let expected = r#"def append_node(
    store_id: int,
    kind: PrismNodeKind,
    input_ids: list[int],
    named_table: str,
    predicate: bool,
    limit_count: int,
) -> int:
    return 1
"#;
    assert_eq!(formatted, expected);
    assert!(
        formatted.lines().all(|line| line.len() <= 120),
        "expected wrapped signature to stay within 120 columns; got:\n{formatted}"
    );
    Ok(())
}

#[test]
fn test_format_source_wraps_long_method_signature() -> Result<(), FormatError> {
    let source = r#"class Store:
  def append_node(self, store_id: int, kind: PrismNodeKind, input_ids: list[int], named_table: str, predicate: bool, limit_count: int) -> int:
    return 1
"#;
    let formatted = format_source(source)?;
    let expected = r#"class Store:
    def append_node(
        self,
        store_id: int,
        kind: PrismNodeKind,
        input_ids: list[int],
        named_table: str,
        predicate: bool,
        limit_count: int,
    ) -> int:
        return 1
"#;
    assert_eq!(formatted, expected);
    assert!(
        formatted.lines().all(|line| line.len() <= 120),
        "expected wrapped method signature to stay within 120 columns; got:\n{formatted}"
    );
    Ok(())
}

#[test]
fn test_format_source_wraps_long_class_trait_adoption_header() -> Result<(), FormatError> {
    let source = r#"pub class _BytesIO with BinaryReader, BinaryRead[u8], BinaryRead[i8], BinaryRead[u16], BinaryRead[i16], BinaryRead[u32], BinaryRead[i32], BinaryRead[u64], BinaryRead[i64], BinaryWrite[u8], BinaryWrite[i8], BinaryWrite[u16], BinaryWrite[i16], BinaryWrite[u32], BinaryWrite[i32], BinaryWrite[u64], BinaryWrite[i64]:
  handle: Cursor[bytes]
"#;
    let formatted = format_source(source)?;
    let expected = r#"pub class _BytesIO with (
    BinaryReader,
    BinaryRead[u8],
    BinaryRead[i8],
    BinaryRead[u16],
    BinaryRead[i16],
    BinaryRead[u32],
    BinaryRead[i32],
    BinaryRead[u64],
    BinaryRead[i64],
    BinaryWrite[u8],
    BinaryWrite[i8],
    BinaryWrite[u16],
    BinaryWrite[i16],
    BinaryWrite[u32],
    BinaryWrite[i32],
    BinaryWrite[u64],
    BinaryWrite[i64],
):
    handle: Cursor[bytes]
"#;
    assert_eq!(formatted, expected);
    assert_eq!(format_source(&formatted)?, expected);
    assert!(
        formatted.lines().all(|line| line.len() <= 120),
        "expected wrapped class trait adoption header to stay within 120 columns; got:\n{formatted}"
    );
    Ok(())
}

#[test]
fn test_format_source_generic_method_round_trip() -> Result<(), FormatError> {
    let source = r#"class Box:
    def get[T with Clone](self, value: T) -> T:
        return value
"#;
    let formatted = assert_format_round_trip_lex_parse(source)?;
    assert!(
        formatted.contains("def get[T with Clone](self, value: T) -> T:"),
        "expected method type params preserved by formatter; got: {formatted}"
    );
    Ok(())
}

#[test]
fn test_format_source_method_trait_target_round_trip() -> Result<(), FormatError> {
    let source = r#"type UserId = rusttype i64 with Display:
    def fmt(self, f: Formatter) for Display -> Result[None, FmtError]:
        pass
"#;
    let formatted = assert_format_round_trip_lex_parse(source)?;
    assert_eq!(formatted, source);
    Ok(())
}

#[test]
fn test_format_source_constrained_primitive_newtype_round_trip() -> Result<(), FormatError> {
    let source = "type Digit = newtype int[gt=-1, lt=10]\n";
    let formatted = assert_format_round_trip_lex_parse(source)?;
    assert_eq!(formatted, source);
    Ok(())
}

#[test]
fn test_format_source_constrained_primitive_spacing() -> Result<(), FormatError> {
    let source = "type Ratio = newtype float[ge = 0,le=1]\n";
    let formatted = format_source(source)?;
    let expected = "type Ratio = newtype float[ge=0, le=1]\n";
    assert_eq!(formatted, expected);
    assert_eq!(assert_format_round_trip_lex_parse(&formatted)?, expected);
    Ok(())
}

#[test]
fn test_format_source_preserves_mut_function_params() -> Result<(), FormatError> {
    let source = r#"def bump(mut session: Session, mut count: int) -> int:
    return count
"#;
    let formatted = assert_format_round_trip_lex_parse(source)?;
    assert!(
        formatted.contains("def bump(mut session: Session, mut count: int) -> int:"),
        "expected mut parameter markers preserved by formatter; got: {formatted}"
    );
    Ok(())
}

#[test]
fn test_format_source_preserves_pub_class_field_visibility() -> Result<(), FormatError> {
    let source = r#"pub class LazyFrame:
    _cursor: int
    pub schema: str
"#;
    let formatted = format_source(source)?;
    assert_eq!(formatted, source);
    Ok(())
}

#[test]
fn test_format_source_preserves_rust_module_and_enum_methods() -> Result<(), FormatError> {
    let source = r#""""Runtime module docstring."""

rust.module("incan_std_testing")

from std.traits.error import Error


pub enum EnvironErrorKind(str):
    """Stable categories for environment read failures."""

    Missing = "missing"
    Other = "other"

    def as_str(self) -> str:
        """Return the stable error category spelling."""
        return self.value()
"#;
    let formatted = format_source(source)?;
    assert_eq!(formatted, source);
    Ok(())
}

/// Regression (GitHub #247): class body docstrings must round-trip for several body shapes **and** stay attached
/// on [`ClassDecl::docstring`] after lex+parse of the formatted source (tooling / API extraction path).
///
/// Covers decorators, `extends`, `with` bounds, fields-only, methods-only, multiple methods, and mixed
/// field+method layouts. Omits `pass`-only class bodies: unlike traits, class bodies do not parse a bare
/// `pass` statement today.
///
/// Model, enum, and trait body docstrings use the same AST and formatter rules (see
/// `test_format_source_preserves_model_enum_body_docstrings_ast` and
/// `test_format_source_preserves_trait_body_docstring_ast` below).
///
/// Two non-empty lines so `trim()` retains an interior newline; `format_docstring` keeps multi-line form (same
/// constraint as newtype docstring tests).
#[test]
fn test_format_source_preserves_class_docstring_common_shapes() -> Result<(), FormatError> {
    const DOC: &str = r#"    """
    Line A documents the class API.
    Line B keeps interior newlines after trim().
    """"#;

    let cases: &[(&str, String)] = &[
        (
            "decorator_generic_field",
            format!(
                r#"@derive(Clone)
pub class WithDecorator[T]:
{DOC}

    pub cell: T
"#
            ),
        ),
        (
            "generic_with_trait_bound_fields_and_method",
            format!(
                r#"pub class PrismCursor[T with Clone]:
{DOC}

    pub row_schema_marker: T

    def clone(self) -> Self:
        """Method doc."""
        pass
"#
            ),
        ),
        (
            "fields_only",
            format!(
                r#"pub class FieldBucket[T]:
{DOC}

    pub first: T
    pub second: int
"#
            ),
        ),
        (
            "private_class_private_fields",
            format!(
                r#"class InternalState:
{DOC}

    x: int
    y: int
"#
            ),
        ),
        (
            "methods_only",
            format!(
                r#"class MethodsOnly:
{DOC}

    def one(self) -> int:
        return 1
"#
            ),
        ),
        (
            "two_methods",
            format!(
                r#"class TwoMethods:
{DOC}

    def a(self) -> None:
        pass

    def b(self) -> None:
        pass
"#
            ),
        ),
        (
            "extends_and_field",
            format!(
                r#"class Pup extends Animal:
{DOC}

    tag: str
"#
            ),
        ),
    ];

    for (label, source) in cases {
        let formatted = format_source(source)?;
        assert_eq!(formatted, *source, "class docstring round-trip ({label})");
        let program = program_from_source(&formatted)?;
        assert_first_class_decl_has_marker_docstring(&program, &format!("formatted source, case {label}"))?;
    }
    Ok(())
}

/// Body docstrings on `model` and `enum` use the same storage and formatting path as `class` (GitHub #247).
#[test]
fn test_format_source_preserves_model_enum_body_docstrings_ast() -> Result<(), FormatError> {
    const DOC: &str = r#"    """
    Line A documents the class API.
    Line B keeps interior newlines after trim().
    """"#;

    let model_src = format!(
        r#"model LedgerEntry:
{DOC}

    id: int
    name: str
"#
    );
    let formatted_model = format_source(&model_src)?;
    assert_eq!(formatted_model, model_src);
    let prog_m = program_from_source(&formatted_model)?;
    assert_first_model_decl_has_marker_docstring(&prog_m, "model + fields after format")?;

    let enum_src = format!(
        r#"enum JobState:
{DOC}

    Pending
    Running
    Done
"#
    );
    let formatted_enum = format_source(&enum_src)?;
    assert_eq!(formatted_enum, enum_src);
    let prog_e = program_from_source(&formatted_enum)?;
    assert_first_enum_decl_has_marker_docstring(&prog_e, "enum + variants after format")?;

    Ok(())
}

#[test]
fn test_format_source_preserves_value_enum_declarations() -> Result<(), FormatError> {
    let source = r#"enum Color(str):
  Red="red"
  Blue = "blue"
  Quote = "say \"hi\""
  Path = "c:\\tmp"
  Tabs = "a\tb"

enum Status(int):
    Pending=1
    Done = 2
"#;

    let expected = r#"enum Color(str):
    Red = "red"
    Blue = "blue"
    Quote = "say \"hi\""
    Path = "c:\\tmp"
    Tabs = "a\tb"


enum Status(int):
    Pending = 1
    Done = 2
"#;

    assert_eq!(format_source(source)?, expected);
    assert_eq!(format_source(expected)?, expected);
    Ok(())
}

#[test]
fn test_format_source_preserves_trait_body_docstring_ast() -> Result<(), FormatError> {
    const DOC: &str = r#"    """
    Line A documents the class API.
    Line B keeps interior newlines after trim().
    """"#;
    let source = format!(
        r#"trait Described:
{DOC}

    def tag(self) -> str: ...
"#
    );
    let formatted = format_source(&source)?;
    let expected = format!(
        r#"trait Described:
{DOC}

    def tag(self) -> str
"#
    );
    assert_eq!(formatted, expected);
    let prog = program_from_source(&formatted)?;
    assert_first_trait_decl_has_marker_docstring(&prog, "trait + method after format")?;
    Ok(())
}

/// `const` / `static` have no inline body docstring field; module-level `"""..."""` is a separate
/// [`Declaration::Docstring`] and must round-trip before those items.
#[test]
fn test_format_source_preserves_module_docstring_before_const_and_static() -> Result<(), FormatError> {
    let source = r#""""Module-level API notes."""

const ANSWER: int = 42
static COUNTER: int = 0
"#;
    let formatted = format_source(source)?;
    assert_eq!(formatted, source);
    let prog = program_from_source(&formatted)?;
    let doc = match &prog.declarations[0].node {
        Declaration::Docstring(doc) => doc,
        other => {
            return Err(FormatError::SyntaxError(format!(
                "expected leading module docstring declaration, got {other:?}"
            )));
        }
    };
    if !doc.contains("Module-level API notes.") {
        return Err(FormatError::SyntaxError(format!("docstring text lost: {doc:?}")));
    }
    let c = match &prog.declarations[1].node {
        Declaration::Const(c) => c,
        other => {
            return Err(FormatError::SyntaxError(format!("expected const: {other:?}")));
        }
    };
    assert_eq!(c.name, "ANSWER");
    let s = match &prog.declarations[2].node {
        Declaration::Static(s) => s,
        other => {
            return Err(FormatError::SyntaxError(format!("expected static: {other:?}")));
        }
    };
    assert_eq!(s.name, "COUNTER");
    Ok(())
}

#[test]
fn test_format_source_preserves_rich_newtype_round_trip() -> Result<(), FormatError> {
    let source = r#"@derive(Clone)
pub type MutexGuard[T with Clone] = newtype RawMutexGuard[T]:
    # XXX: keep this comment anchored to the type docstring
    """
    Guard providing access to mutex-protected data.
    The lock is released when the guard goes out of scope.
    """

    def get(self) -> T:
        """Get the current value (by reference)"""
        return value

    def example(self) -> None:
        shared_counter = Mutex.new(0)  # XXX: constructor lives on Mutex
        return None
"#;
    let formatted = format_source(source)?;
    assert_eq!(formatted, source);
    Ok(())
}

#[test]
fn test_format_source_associated_type_in_rusttype_body() -> Result<(), FormatError> {
    let source = r#"type UserId = rusttype i64 with Add[int, UserId]:
    type Output for Add[int] = UserId

    def add(self, rhs: int) for Add[int] -> UserId:
        pass
"#;
    let formatted = assert_format_round_trip_lex_parse(source)?;
    assert_eq!(formatted, source);
    Ok(())
}

#[test]
fn test_format_trait_abstract_methods_prefer_bodyless_form_before_default_method() -> Result<(), FormatError> {
    let source = r#"trait Service:
  def connect(self) -> None: ...
  def close(self) -> None: ...
  def reset(self) -> None:
    pass
"#;
    let result = format_source(source)?;

    let expected = r#"trait Service:
    def connect(self) -> None
    def close(self) -> None

    def reset(self) -> None:
        pass
"#;
    assert_eq!(result, expected);
    Ok(())
}

#[test]
fn test_format_source_computed_properties() -> Result<(), FormatError> {
    let source = r#"class Dataset:
  fields: list[str]
  property schema_fields -> list[str]:
    return self.fields
  def len(self) -> int:
    return 0
"#;
    let result = format_source(source)?;

    let expected = r#"class Dataset:
    fields: list[str]

    property schema_fields -> list[str]:
        return self.fields

    def len(self) -> int:
        return 0
"#;
    assert_eq!(result, expected);
    Ok(())
}

#[test]
fn test_format_source_trait_abstract_properties_stay_tight() -> Result<(), FormatError> {
    let source = r#"trait HasShape:
  property area -> float: ...
  property perimeter -> float: ...
  def describe(self) -> str:
    return "shape"
"#;
    let result = format_source(source)?;

    let expected = r#"trait HasShape:
    property area -> float
    property perimeter -> float

    def describe(self) -> str:
        return "shape"
"#;
    assert_eq!(result, expected);
    Ok(())
}

#[test]
fn test_format_source_partial_declarations_and_expression() -> Result<(), FormatError> {
    let source = r#"pub BronzeReader = partial readers.TableReader(layer="bronze", format="delta")
JsonRoute = partial web.route(method="GET")

def reader_for(layer: str) -> Reader:
  return partial make_factory().reader(layer=layer, options={"format": "delta"})
"#;
    let formatted = format_source(source)?;

    let expected = r#"pub BronzeReader = partial readers.TableReader(layer="bronze", format="delta")
JsonRoute = partial web.route(method="GET")


def reader_for(layer: str) -> Reader:
    return partial make_factory().reader(layer=layer, options={"format": "delta"})
"#;
    assert_eq!(formatted, expected);
    assert_eq!(format_source(&formatted)?, expected);
    Ok(())
}

#[test]
fn test_format_source_method_and_trait_partials() -> Result<(), FormatError> {
    let source = r#"model Cell:
  alive: bool
  set_alive = partial set_state(state=true)
  def set_state(mut self, state: bool) -> None:
    self.alive = state

trait Named:
  display = partial name(prefix="name")
  def name(self, prefix: str) -> str
"#;
    let formatted = format_source(source)?;

    let expected = r#"model Cell:
    alive: bool
    set_alive = partial set_state(state=true)

    def set_state(mut self, state: bool) -> None:
        self.alive = state


trait Named:
    display = partial name(prefix="name")

    def name(self, prefix: str) -> str
"#;
    assert_eq!(formatted, expected);
    assert_eq!(format_source(&formatted)?, expected);
    Ok(())
}
