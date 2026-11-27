//! Declaration formatting: models and `pub` field visibility, traits with supertraits and abstract methods, decorators
//! and decorator factories, long function, method and class headers, generic methods, method trait targets, constrained
//! primitive newtypes, `mut` parameters, testing markers and enum methods, class, model, enum and trait body
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

/// Ownership sources survive both wrapped headers and abstract methods without changing their field path.
#[test]
fn explicit_result_sources_round_trip() -> Result<(), FormatError> {
    let source = "def read(value: Borrowed[str]) -> Result[Borrowed[str], str] from value:\n    return Ok(value)\n\ntrait View:\n    def view(self) -> Borrowed[str] from self.name: ...\n";
    let formatted = assert_format_round_trip_lex_parse(source)?;
    assert!(formatted.contains("from value:"));
    assert!(formatted.contains("from self.name\n"));
    assert_eq!(format_source(&formatted)?, formatted);
    let config = FormatConfig {
        line_length: 45,
        ..FormatConfig::default()
    };
    let wrapped = format_source_with_config(source, config.clone())?;
    program_from_source(&wrapped)?;
    assert!(wrapped.contains(") -> Result[Borrowed[str], str] from value:"));
    assert_eq!(format_source_with_config(&wrapped, config)?, wrapped);
    Ok(())
}

#[test]
fn declaration_formatting_cases() {
    run_format_cases(DECLARATION_CASES);
}

const DECLARATION_CASES: &[FormatCase] = &[
    FormatCase {
        name: "test_format_source_preserves_registry_description_decorator",
        source: r#"from std.registry import Registry, SubjectKind, describe

type FunctionId = newtype str

@derive(Descriptor)
model FunctionSpec:
  summary: str

static functions: Registry[FunctionId, FunctionSpec] = Registry.define(subjects=[SubjectKind.Function])

@describe(functions,FunctionId("normalize"),FunctionSpec(summary="Normalize text"))
def normalize(value: str) -> str:
  return value
"#,
        checks: &[
            Check::Has("@describe(functions, FunctionId(\"normalize\"), FunctionSpec(summary=\"Normalize text\"))"),
            Check::Stable,
        ],
        ..FormatCase::DEFAULT
    },
    FormatCase {
        name: "test_format_source_model",
        source: r#"model User:
  name: str
  age: int
"#,
        checks: &[],
        ..FormatCase::DEFAULT
    },
    FormatCase {
        name: "test_format_source_preserves_ordinary_pub_model_field_visibility_issue884",
        source: r#"pub model SealedEnvelope:
  body: str
  pub envelope_id: str
"#,
        checks: &[
            Check::Is(
                r#"pub model SealedEnvelope:
    body: str
    pub envelope_id: str
"#,
            ),
            Check::Stable,
        ],
        ..FormatCase::DEFAULT
    },
    FormatCase {
        name: "field_metadata_keeps_registry_order_and_canonical_value_forms",
        source: r#"model Customer:
  email_address [canonical=false, renamed_from="email"] as "email": str
  display_name [renamed_from=["name"], canonical=true, description="Visible name"]: str
  hidden [made_private { replacement=Customer.display_name,message="Use display_name",since="0.6" }]: str
  archived [made_private="Use display_name"]: str
  internal [made_private=true]: str
  current [renamed_from { issue="migration",name=["old"],since="0.6",message="Line\n\"quoted\"" }]: str
  label [renamed_from { name="caption" }]: str
"#,
        checks: &[
            Check::Is(
                r#"model Customer:
    email_address [renamed_from="email", canonical=false] as "email": str
    display_name [description="Visible name", renamed_from=["name"], canonical=true]: str
    hidden [made_private { since="0.6", message="Use display_name", replacement=Customer.display_name }]: str
    archived [made_private="Use display_name"]: str
    internal [made_private=true]: str
    current [renamed_from { name=["old"], since="0.6", message="Line\n\"quoted\"", issue="migration" }]: str
    label [renamed_from { name="caption" }]: str
"#,
            ),
            Check::Stable,
        ],
        ..FormatCase::DEFAULT
    },
    FormatCase {
        name: "test_format_source_trait_with_supertraits",
        source: r#"trait OrderedCollection[T] with Collection[T], Serializable:
  def sorted(self) -> OrderedCollection[T]: ...
"#,
        checks: &[Check::Is(
            r#"trait OrderedCollection[T] with Collection[T], Serializable:
    def sorted(self) -> OrderedCollection[T]
"#,
        )],
        ..FormatCase::DEFAULT
    },
    FormatCase {
        name: "test_format_source_testing_marker_decorator",
        source: r#"@marker(kind="skip",runner_only=true)
def skipped() -> None:
  pass
"#,
        checks: &[Check::Is(
            r#"@marker(kind="skip", runner_only=true)
def skipped() -> None:
    pass
"#,
        )],
        ..FormatCase::DEFAULT
    },
    FormatCase {
        name: "test_format_source_decorator_factory_type_args",
        source: r#"@registered[(str)->ColumnExpr]("incql.functions.col")
def col(name: str) -> ColumnExpr:
  return ColumnExpr(name=name)
"#,
        checks: &[Check::Is(
            r#"@registered[(str) -> ColumnExpr]("incql.functions.col")
def col(name: str) -> ColumnExpr:
    return ColumnExpr(name=name)
"#,
        )],
        ..FormatCase::DEFAULT
    },
    FormatCase {
        name: "test_format_source_wraps_long_function_signature",
        source: r#"def append_node(store_id: int, kind: PrismNodeKind, input_ids: list[int], named_table: str, predicate: bool, limit_count: int) -> int:
  return 1
"#,
        checks: &[
            Check::Is(
                r#"def append_node(
    store_id: int,
    kind: PrismNodeKind,
    input_ids: list[int],
    named_table: str,
    predicate: bool,
    limit_count: int,
) -> int:
    return 1
"#,
            ),
            Check::Width(120),
        ],
        ..FormatCase::DEFAULT
    },
    FormatCase {
        name: "test_format_source_wraps_long_method_signature",
        source: r#"class Store:
  def append_node(self, store_id: int, kind: PrismNodeKind, input_ids: list[int], named_table: str, predicate: bool, limit_count: int) -> int:
    return 1
"#,
        checks: &[
            Check::Is(
                r#"class Store:
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
"#,
            ),
            Check::Width(120),
        ],
        ..FormatCase::DEFAULT
    },
    FormatCase {
        name: "test_format_source_wraps_long_class_trait_adoption_header",
        source: r#"pub class _BytesIO with BinaryReader, BinaryRead[u8], BinaryRead[i8], BinaryRead[u16], BinaryRead[i16], BinaryRead[u32], BinaryRead[i32], BinaryRead[u64], BinaryRead[i64], BinaryWrite[u8], BinaryWrite[i8], BinaryWrite[u16], BinaryWrite[i16], BinaryWrite[u32], BinaryWrite[i32], BinaryWrite[u64], BinaryWrite[i64]:
  handle: Cursor[bytes]
"#,
        checks: &[
            Check::Is(
                r#"pub class _BytesIO with (
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
"#,
            ),
            Check::Stable,
            Check::Width(120),
        ],
        ..FormatCase::DEFAULT
    },
    FormatCase {
        name: "test_format_source_generic_method_round_trip",
        source: r#"class Box:
    def get[T with Clone](self, value: T) -> T:
        return value
"#,
        checks: &[Check::Parses, Check::Has("def get[T with Clone](self, value: T) -> T:")],
        ..FormatCase::DEFAULT
    },
    FormatCase {
        name: "test_format_source_method_trait_target_round_trip",
        source: r#"type UserId = rusttype i64 with Display:
    def fmt(self, f: Formatter) for Display -> Result[None, FmtError]:
        pass
"#,
        checks: &[Check::Parses, Check::Unchanged],
        ..FormatCase::DEFAULT
    },
    FormatCase {
        name: "test_format_source_constrained_primitive_newtype_round_trip",
        source: "type Digit = newtype int[gt=-1, lt=10]\n",
        checks: &[Check::Parses, Check::Unchanged],
        ..FormatCase::DEFAULT
    },
    FormatCase {
        name: "test_format_source_constrained_primitive_spacing",
        source: "type Ratio = newtype float[ge = 0,le=1]\n",
        checks: &[
            Check::Is("type Ratio = newtype float[ge=0, le=1]\n"),
            Check::Parses,
            Check::Stable,
        ],
        ..FormatCase::DEFAULT
    },
    FormatCase {
        name: "test_format_source_preserves_mut_function_params",
        source: r#"def bump(mut session: Session, mut count: int) -> int:
    return count
"#,
        checks: &[
            Check::Parses,
            Check::Has("def bump(mut session: Session, mut count: int) -> int:"),
        ],
        ..FormatCase::DEFAULT
    },
    FormatCase {
        name: "test_format_source_preserves_pub_class_field_visibility",
        source: r#"pub class LazyFrame:
    _cursor: int
    pub schema: str
"#,
        checks: &[Check::Unchanged],
        ..FormatCase::DEFAULT
    },
    FormatCase {
        name: "test_format_source_preserves_enum_methods",
        source: r#""""Runtime module docstring."""

from std.traits.error import Error


pub enum EnvironErrorKind(str):
    """Stable categories for environment read failures."""

    Missing = "missing"
    Other = "other"

    def as_str(self) -> str:
        """Return the stable error category spelling."""
        return self.value()
"#,
        checks: &[Check::Unchanged],
        ..FormatCase::DEFAULT
    },
    FormatCase {
        name: "test_format_source_preserves_value_enum_declarations",
        source: r#"enum Color(str):
  Red="red"
  Blue = "blue"
  Quote = "say \"hi\""
  Path = "c:\\tmp"
  Tabs = "a\tb"

enum Status(int):
    Pending=1
    Done = 2
"#,
        checks: &[
            Check::Is(
                r#"enum Color(str):
    Red = "red"
    Blue = "blue"
    Quote = "say \"hi\""
    Path = "c:\\tmp"
    Tabs = "a\tb"


enum Status(int):
    Pending = 1
    Done = 2
"#,
            ),
            Check::Stable,
        ],
        ..FormatCase::DEFAULT
    },
    // A newtype's alias is authored once even when parser views share its declaration.
    FormatCase {
        name: "test_format_source_preserves_rich_newtype_round_trip",
        source: r#"@derive(Clone)
pub type MutexGuard[T with Clone] = newtype RawMutexGuard[T]:
    # XXX: keep this comment anchored to the type docstring
    """
    Guard providing access to mutex-protected data.
    The lock is released when the guard goes out of scope.
    """

    read = get

    def get(self) -> T:
        """Get the current value (by reference)"""
        return value

    def example(self) -> None:
        shared_counter = Mutex.new(0)  # XXX: constructor lives on Mutex
        return None
"#,
        checks: &[Check::Unchanged, Check::Stable],
        ..FormatCase::DEFAULT
    },
    FormatCase {
        name: "test_format_source_associated_type_in_rusttype_body",
        source: r#"type UserId = rusttype i64 with Add[int, UserId]:
    type Output for Add[int] = UserId

    def add(self, rhs: int) for Add[int] -> UserId:
        pass
"#,
        checks: &[Check::Parses, Check::Unchanged],
        ..FormatCase::DEFAULT
    },
    FormatCase {
        name: "test_format_trait_abstract_methods_prefer_bodyless_form_before_default_method",
        source: r#"trait Service:
  def connect(self) -> None: ...
  def close(self) -> None: ...
  def reset(self) -> None:
    pass
"#,
        checks: &[Check::Is(
            r#"trait Service:
    def connect(self) -> None
    def close(self) -> None

    def reset(self) -> None:
        pass
"#,
        )],
        ..FormatCase::DEFAULT
    },
    FormatCase {
        name: "test_format_source_computed_properties",
        source: r#"class Dataset:
  fields: list[str]
  property schema_fields -> list[str]:
    return self.fields
  def len(self) -> int:
    return 0
"#,
        checks: &[Check::Is(
            r#"class Dataset:
    fields: list[str]

    property schema_fields -> list[str]:
        return self.fields

    def len(self) -> int:
        return 0
"#,
        )],
        ..FormatCase::DEFAULT
    },
    FormatCase {
        name: "test_format_source_trait_abstract_properties_stay_tight",
        source: r#"trait HasShape:
  property area -> float: ...
  property perimeter -> float: ...
  def describe(self) -> str:
    return "shape"
"#,
        checks: &[Check::Is(
            r#"trait HasShape:
    property area -> float
    property perimeter -> float

    def describe(self) -> str:
        return "shape"
"#,
        )],
        ..FormatCase::DEFAULT
    },
    FormatCase {
        name: "test_format_source_partial_declarations_and_expression",
        source: r#"pub BronzeReader = partial readers.TableReader(layer="bronze", format="delta")
JsonRoute = partial web.route(method="GET")

def reader_for(layer: str) -> Reader:
  return partial make_factory().reader(layer=layer, options={"format": "delta"})
"#,
        checks: &[
            Check::Is(
                r#"pub BronzeReader = partial readers.TableReader(layer="bronze", format="delta")
JsonRoute = partial web.route(method="GET")


def reader_for(layer: str) -> Reader:
    return partial make_factory().reader(layer=layer, options={"format": "delta"})
"#,
            ),
            Check::Stable,
        ],
        ..FormatCase::DEFAULT
    },
    FormatCase {
        name: "test_format_source_method_and_trait_partials",
        source: r#"model Cell:
  alive: bool
  set_alive = partial set_state(state=true)
  def set_state(mut self, state: bool) -> None:
    self.alive = state

trait Named:
  display = partial name(prefix="name")
  def name(self, prefix: str) -> str
"#,
        checks: &[
            Check::Is(
                r#"model Cell:
    alive: bool
    set_alive = partial set_state(state=true)

    def set_state(mut self, state: bool) -> None:
        self.alive = state


trait Named:
    display = partial name(prefix="name")

    def name(self, prefix: str) -> str
"#,
            ),
            Check::Stable,
        ],
        ..FormatCase::DEFAULT
    },
    // A property's borrowed result names its source as a method's does: the field the view borrows, or the receiver.
    FormatCase {
        name: "property_result_sources_round_trip",
        source: "model Frame:\n    nominals: list[int]\n\n    property nominals_view -> Borrowed[list[int]] from self.nominals:\n        return self.nominals\n\n    property frame -> Borrowed[Frame] from self:\n        return self\n\ntrait Viewed:\n    property names -> Borrowed[list[str]] from self.names\n",
        checks: &[
            Check::Parses,
            Check::Has("property nominals_view -> Borrowed[list[int]] from self.nominals:\n"),
            Check::Has("property frame -> Borrowed[Frame] from self:\n"),
            Check::Has("property names -> Borrowed[list[str]] from self.names\n"),
            Check::Stable,
        ],
        ..FormatCase::DEFAULT
    },
];
