//! Stable diagnostic codes and machine-readable projection helpers.
//!
//! The compiler still has many specialized human diagnostic constructors. This module is the stable reporting layer
//! used by CLI JSON output and explain/help surfaces. It intentionally starts with broad phase-level codes, then can
//! grow narrower codes without making callers scrape terminal prose.

use incan_semantics_core::CanonicalSymbolId;
use serde::Serialize;

use crate::ast::{Declaration, Program, Span};

use super::{CompileError, ErrorKind};

/// Schema version for machine-readable diagnostic reports.
pub const DIAGNOSTIC_SCHEMA_VERSION: u32 = 2;

/// Pipeline phase that produced a diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticPhase {
    Lex,
    Parse,
    Typecheck,
    Import,
    Tooling,
    Unknown,
}

/// Compiler subsystem that produced a diagnostic fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticOrigin {
    /// Lexer tokenization.
    Lexer,
    /// Parser or syntax validation.
    Parser,
    /// Import, module, or package resolution.
    ImportResolver,
    /// Type checking and semantic validation.
    Typechecker,
    /// CLI or surrounding tooling.
    Tooling,
    /// The producer is not known precisely enough yet.
    Unknown,
}

impl DiagnosticOrigin {
    /// Return the stable lowercase origin label for non-Serde consumers.
    pub fn as_str(self) -> &'static str {
        match self {
            DiagnosticOrigin::Lexer => "lexer",
            DiagnosticOrigin::Parser => "parser",
            DiagnosticOrigin::ImportResolver => "import_resolver",
            DiagnosticOrigin::Typechecker => "typechecker",
            DiagnosticOrigin::Tooling => "tooling",
            DiagnosticOrigin::Unknown => "unknown",
        }
    }
}

impl DiagnosticPhase {
    /// Return the stable lowercase phase label used by human text and non-Serde call sites.
    pub fn as_str(self) -> &'static str {
        match self {
            DiagnosticPhase::Lex => "lex",
            DiagnosticPhase::Parse => "parse",
            DiagnosticPhase::Typecheck => "typecheck",
            DiagnosticPhase::Import => "import",
            DiagnosticPhase::Tooling => "tooling",
            DiagnosticPhase::Unknown => "unknown",
        }
    }
}

/// Public diagnostic catalog entry returned by `incan explain`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct DiagnosticCatalogEntry {
    /// Stable public diagnostic code, such as `INCAN-T0001`.
    pub code: &'static str,
    /// Short human-readable title for the diagnostic family.
    pub title: &'static str,
    /// Default severity label exposed by the diagnostic catalog.
    pub severity: &'static str,
    /// Compiler or tooling phase that owns this catalog entry.
    pub phase: &'static str,
    /// One-sentence description of the problem class.
    pub summary: &'static str,
    /// Longer explanation printed by `incan explain`.
    pub explanation: &'static str,
    /// Small source or command examples that can produce this diagnostic family.
    pub examples: &'static [&'static str],
    /// Common root causes shown in text and JSON explain output.
    pub common_causes: &'static [&'static str],
    /// Suggested remediation steps for this diagnostic family.
    pub fixes: &'static [&'static str],
    /// Optional documentation URL with deeper guidance.
    pub docs_url: Option<&'static str>,
}

/// 1-based source position plus original byte offset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiagnosticPosition {
    /// 1-based source line.
    pub line: usize,
    /// 1-based source column counted in Unicode scalar values.
    pub column: usize,
    /// Original UTF-8 byte offset into the source text.
    pub offset: usize,
}

/// Primary diagnostic span.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiagnosticSpan {
    /// Source file path used for this diagnostic projection.
    pub file: String,
    /// Inclusive start position.
    pub start: DiagnosticPosition,
    /// Exclusive end position.
    pub end: DiagnosticPosition,
}

/// One compiler-owned related location in a structured diagnostic fact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiagnosticRelatedSpan {
    /// The secondary source span.
    pub span: DiagnosticSpan,
    /// Why this source span is related to the primary diagnostic.
    pub label: String,
}

/// One declaration related to a diagnostic, retaining its canonical source origin and provider-local byte span.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiagnosticRelatedDeclaration {
    pub identity: CanonicalSymbolId,
    pub label: String,
}

/// Machine-readable diagnostic payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StableDiagnostic {
    /// Stable public diagnostic code selected from the catalog.
    pub code: &'static str,
    /// Concrete severity for this diagnostic instance.
    pub severity: &'static str,
    /// Compiler or tooling phase that produced this diagnostic.
    pub phase: DiagnosticPhase,
    /// Compiler subsystem that produced the fact.
    pub origin: DiagnosticOrigin,
    /// User-facing diagnostic message.
    pub message: String,
    /// Primary source span for editors and structured tooling.
    pub primary_span: DiagnosticSpan,
    /// Additional explanatory notes carried by the compiler diagnostic.
    pub notes: Vec<String>,
    /// Suggested fixes or hints carried by the compiler diagnostic.
    pub hints: Vec<String>,
    /// Structured expected value or type when available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected: Option<String>,
    /// Structured actual value or type when available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual: Option<String>,
    /// Related compiler-owned source locations.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub related_spans: Vec<DiagnosticRelatedSpan>,
    /// Related declarations whose offsets are not projected into the primary file's coordinate system.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub related_declarations: Vec<DiagnosticRelatedDeclaration>,
    /// Command users can run to read the catalog explanation for `code`.
    pub explain: String,
}

const PARSER_SYNTAX: DiagnosticCatalogEntry = DiagnosticCatalogEntry {
    code: "INCAN-P0001",
    title: "Syntax error",
    severity: "error",
    phase: "parse",
    summary: "The source text does not match Incan syntax.",
    explanation: "The lexer or parser could not turn the source into a valid Incan AST. The primary span points at the token or source region where parsing stopped.",
    examples: &["def broken(:", "if value"],
    common_causes: &[
        "A missing expression, colon, delimiter, or indentation boundary.",
        "Using vocabulary syntax without the required imported vocabulary surface.",
    ],
    fixes: &[
        "Check the source around the highlighted span.",
        "Run `incan fmt --check` after the file parses if the intended syntax is valid.",
    ],
    docs_url: Some("https://encero-systems.github.io/incan/language/reference/syntax/"),
};

const TYPECHECK: DiagnosticCatalogEntry = DiagnosticCatalogEntry {
    code: "INCAN-T0001",
    title: "Type checking error",
    severity: "error",
    phase: "typecheck",
    summary: "A parsed program violates Incan's type, symbol, or semantic rules.",
    explanation: "The type checker resolved declarations and expressions but found an invalid symbol, type mismatch, unsupported call shape, or related semantic issue.",
    examples: &["value: int = \"text\"", "unknown_name()"],
    common_causes: &[
        "A missing import or definition.",
        "A value passed to a function, assignment, or return position does not match the expected type.",
    ],
    fixes: &[
        "Read the message, notes, and hints in the diagnostic payload.",
        "Prefer fixing the source contract rather than adding casts or wrappers that hide the mismatch.",
    ],
    docs_url: Some("https://encero-systems.github.io/incan/language/reference/types/"),
};

const UNREACHABLE_CODE: DiagnosticCatalogEntry = DiagnosticCatalogEntry {
    code: "INCAN-T0101",
    title: "Unreachable code",
    severity: "warning",
    phase: "typecheck",
    summary: "Statements in a block can never run because the block already returned.",
    explanation: "The type checker found statements that follow a `return` in the same block, so nothing can reach them. This is a warning, not an error: the program still compiles and the unreachable statements are still type-checked, but they are dead code. The check is block-local — it follows a `return` statement within one block and does not try to prove divergence through `if`/`else`, `match`, or loops.",
    examples: &["def f() -> int:\n    return 1\n    println(\"never runs\")"],
    common_causes: &[
        "An early `return` left behind during a refactor.",
        "A debugging `return` added above code that was meant to keep running.",
    ],
    fixes: &[
        "Delete the unreachable statements.",
        "Move the statements above the `return` if they were meant to run.",
    ],
    docs_url: Some("https://encero-systems.github.io/incan/language/reference/functions/"),
};

const CALLABLE_MARKER_NOT_SUPPORTED: DiagnosticCatalogEntry = DiagnosticCatalogEntry {
    code: "INCAN-T0106",
    title: "Callable marker cannot be spelled here",
    severity: "error",
    phase: "typecheck",
    summary: "An `Fn`, `FnMut` or `FnOnce` marker from `std.rust` names more than two parameters, or bounds a type parameter of a nominal declaration.",
    explanation: "A callable marker names a callable's parameter list and learns its return type from a function or method call. It currently supports at most two parameters and cannot complete a nominal declaration's type parameter.",
    examples: &["from std.rust import Fn\n\ndef run[F with Fn[int, int, int]](f: F) -> None:\n    pass"],
    common_causes: &[
        "A callback with three or more arguments.",
        "A nominal declaration bounded with a marker instead of a callable trait.",
    ],
    fixes: &[
        "Write at most two parameters, or gather them into one model.",
        "Use `Callable1[int, R]` from `std.traits.callable` on a nominal declaration.",
    ],
    docs_url: Some("https://encero-systems.github.io/incan/language/how-to/rust_interop/"),
};

const SELF_MUTATION_REQUIRES_MUT_SELF: DiagnosticCatalogEntry = DiagnosticCatalogEntry {
    code: "INCAN-T0102",
    title: "Method changes the object but takes `self`",
    severity: "error",
    phase: "typecheck",
    summary: "A method assigns to a field, or calls a method that changes one, while its receiver is a plain `self`.",
    explanation: "The receiver spelling is a contract the compiled code keeps literally: `self` reads the object, `mut self` may change it. A body that assigns to `self.field`, writes `self.items[i]`, or calls a changing method such as `self.items.append(...)` or a `mut self` method of its own therefore needs a `mut self` receiver. The check applies to classes, models, trait default methods and trait implementations alike, and follows field and index chains rooted at `self`.",
    examples: &[
        "class Stack:\n    items: list[int]\n\n    def pop(self) -> int:\n        return self.items.pop()",
        "class Carton with Resizable:\n    width: float\n\n    def resize(self, factor: float) -> None:\n        self.width *= factor",
    ],
    common_causes: &[
        "A method written with `self` that grew a field assignment or a changing collection call.",
        "A trait implementation whose trait declares the method with `self` while the implementation needs to write.",
    ],
    fixes: &[
        "Declare the receiver as `mut self`: `def pop(mut self) -> int`.",
        "When the method implements a trait method, declare `mut self` in the trait as well so the signatures match.",
    ],
    docs_url: Some("https://encero-systems.github.io/incan/language/explanation/models_and_classes/classes/"),
};

const VALUE_WITHOUT_PRINTED_FORM: DiagnosticCatalogEntry = DiagnosticCatalogEntry {
    code: "INCAN-T0103",
    title: "Value with no printed form",
    severity: "error",
    phase: "typecheck",
    summary: "A displayed value (a `print` or `println` argument, the argument of `str(...)`, an f-string `{value}` part, or a type argument for a `Display` bound) has no printed form: a union value, a generator, a function, `bytes`, or a model, class, enum or newtype that provides no `Display`.",
    explanation: "`print`, `println`, `str`, an f-string `{value}` and a `Display` bound share one display rule. Scalars and `str` display their own text; a tuple, list, dict, set, `Option` or `Result` displays its structure (`(10, 20)`, `[1, 2, 3]`, `Some(1)`, `Err(\"bad\")`) in the four value positions. A model, class, enum or newtype displays through `Display`, which it provides by a `__str__(self) -> str` method, by each variant's value when it is an enum that declares values, or by `message()` when it adopts `Error` and has no `__str__`; `@derive(Display)` provides nothing. Every other value has no printed form, and displaying one in any position is refused: a union value until it is narrowed to one member, a generator until its items are collected, a function until it is called, `bytes` until they are decoded, and a model, class, enum or newtype until it provides `Display`. `{value:?}` renders a value's structure through `Debug` instead.",
    examples: &[
        "def show(value: int | str) -> None:\n    println(value)",
        "model Point:\n    x: int\n    y: int\n\ndef main() -> None:\n    println(Point(x=1, y=2))",
    ],
    common_causes: &[
        "Printing a union value before narrowing it.",
        "Printing a model, class, plain enum or newtype that defines no `__str__`.",
        "Printing a generator instead of its collected items.",
    ],
    fixes: &[
        "Narrow a union first: `match value:` with a type pattern per member, then display the member.",
        "Define `__str__(self) -> str` on the type, or interpolate its structure with `f\"{point:?}\"`.",
        "Collect a generator first: `println(list(numbers))`.",
    ],
    docs_url: Some("https://encero-systems.github.io/incan/language/reference/strings/"),
};

const COLLECTION_ANNOTATION_REQUIRES_TYPE_ARGUMENTS: DiagnosticCatalogEntry = DiagnosticCatalogEntry {
    code: "INCAN-T0104",
    title: "Collection annotation without type arguments",
    severity: "error",
    phase: "typecheck",
    summary: "A `list`, `dict`, `set`, `tuple`, `Option` or `Result` annotation (or a frozen collection or `Generator`) names no type arguments.",
    explanation: "Each builtin collection name is a family of types, one per argument list, so the bare word names no type: nothing can be emitted for it and the build stops. The refusal applies wherever the bare word stands, alone or inside another annotation (`list[Dict]`, `Option[List]`). A source type that takes one of these spellings for itself is an ordinary type and is not affected.",
    examples: &[
        "multiple: Tuple = (\"a\", 1)",
        "items: List = [1, 2]",
        "maybe: Option = None",
    ],
    common_causes: &[
        "An annotation written with the bare family name, such as `items: List`.",
        "An annotation left incomplete while the value's shape was still changing.",
    ],
    fixes: &[
        "Write the type arguments: `list[int]`, `dict[str, int]`, `Option[int]`, `Result[int, str]`.",
        "Write one type per tuple element: `tuple[str, int]`.",
    ],
    docs_url: Some("https://encero-systems.github.io/incan/language/reference/language/"),
};

const RUST_OWNER_TYPE_ARGS_NOT_INFERRED: DiagnosticCatalogEntry = DiagnosticCatalogEntry {
    code: "INCAN-T0105",
    title: "Rust type arguments cannot be inferred",
    severity: "error",
    phase: "typecheck",
    summary: "A Rust associated call leaves its owner's type arguments open and nothing later in the program fixes them.",
    explanation: "`HashMap.new()` on `rust::std::collections::HashMap` leaves `K` and `V` open. Rust fills them from a later use of the binding, such as an insert, a typed return, or an annotation on the binding; a binding that is never read again, or a result that is not bound at all, gives it nothing to work with, and the build stops on the call. The check fires only when the compiler can see the owner's type parameters and the value has no later reader.",
    examples: &["from rust::std::collections import HashMap\n\ndef main() -> None:\n    mut untyped = HashMap.new()"],
    common_causes: &[
        "A collection constructed and then never used.",
        "A binding whose only later uses do not mention the element types, such as `len(m)`.",
    ],
    fixes: &[
        "Write the type arguments in the call: `HashMap.new[str, int]()`.",
        "Annotate the binding: `untyped: HashMap[str, int] = HashMap.new()`.",
    ],
    docs_url: Some("https://encero-systems.github.io/incan/language/how-to/rust_interop/"),
};

const ROUTE_HANDLER_RETURN_NOT_RESPONSE: DiagnosticCatalogEntry = DiagnosticCatalogEntry {
    code: "INCAN-T0107",
    title: "Route handler returns a non-response type",
    severity: "error",
    phase: "typecheck",
    summary: "A `@route` handler's declared return type is not a response type, so the route has nothing to send.",
    explanation: "A route handler's return value is the HTTP response. The response types are `str`, `None`, `Json[...]`, `Html`, `Response`, a `Result` whose both sides are response types, and a wrapper type that derives `IntoResponse` from `std.web.macros`. A handler declared with any other return type, such as `int`, `float`, a tuple, a list or a plain model, checked before but could not be built: the route registration needs a response and had none. The check reads the declared return type only; a type the compiler cannot classify, such as a Rust-origin type, is left to the build.",
    examples: &[
        "from std.web import route\nimport std.async\n\n@route(\"/users/{id}\")\nasync def create_user(id: int) -> int:\n    return id",
    ],
    common_causes: &[
        "Returning a number or a computed value directly instead of its text or JSON form.",
        "Returning a model without wrapping it in `Json(...)`.",
    ],
    fixes: &[
        "Return the value as text: `-> str` and `return str(id)`.",
        "Return JSON: `-> Json[User]` and `return Json(user)`, with `@derive(json)` on the model.",
        "Return `Html(...)` or a `Response` builder result for other bodies.",
    ],
    docs_url: Some("https://encero-systems.github.io/incan/language/tutorials/web_framework/"),
};

const ROUTE_HANDLER_PARAMETER_UNBOUND: DiagnosticCatalogEntry = DiagnosticCatalogEntry {
    code: "INCAN-T0108",
    title: "Route handler parameter is not bound by the route",
    severity: "error",
    phase: "typecheck",
    summary: "A `@route` handler has a parameter that no `{segment}` of the path binds and no extractor supplies.",
    explanation: "A route handler receives its parameters from the request in two ways. A `{name}` segment in the path binds the parameter named `name`, and a typed extractor parameter reads the request itself: `Json[T]` the body, `Query[T]` the query string, `Path[T]` the path, or a wrapper type deriving `FromRequestParts` from `std.web.macros`. A parameter that is neither has no value to receive, so the route cannot be registered and the build stopped on it. The check reads the path when it is a string literal in the decorator; a parameter whose type the compiler cannot classify is left to the build.",
    examples: &[
        "from std.web import route, POST\nimport std.async\n\n@route(\"/things\", methods=[POST])\nasync def create(id: int) -> str:\n    return str(id)",
    ],
    common_causes: &[
        "A `{segment}` left out of the path, or spelled differently from the parameter.",
        "A value meant to come from the query string or the body declared as a plain scalar parameter.",
    ],
    fixes: &[
        "Add the segment to the path: `@route(\"/things/{id}\")`.",
        "Read the value from the request: `params: Query[Params]` or `body: Json[Payload]`.",
    ],
    docs_url: Some("https://encero-systems.github.io/incan/language/tutorials/web_framework/"),
};

const OPERATOR_HAS_NO_TYPE_PARAMETER_BOUND: DiagnosticCatalogEntry = DiagnosticCatalogEntry {
    code: "INCAN-T0109",
    title: "Numeric operator on a type parameter",
    severity: "error",
    phase: "typecheck",
    summary: "`/`, `//`, `%` or `**` is applied to values of a type parameter, which no bound can support.",
    explanation: "A generic function's arithmetic on a type parameter becomes an inferred bound: `+`, `-` and `*` require the type argument to support that operator. True division, floor division, modulo and power do not work that way. They follow the language's numeric rules, which the concrete numeric types carry: `/` yields `float` for any operands, `//` and `%` round toward negative infinity, `**` picks its result type from the exponent. No trait stands for those rules, so a function that applies one of these operators to values of a type parameter has no bound a type argument could satisfy and could never be compiled for any argument. The operator still resolves through a bound trait that defines its hook (`__div__`, `__floordiv__`, `__mod__`, `__pow__`), so a type parameter bounded by such a trait is accepted.",
    examples: &["def modulo[T](a: T, b: T) -> T:\n    return a % b"],
    common_causes: &[
        "A numeric helper written generically when its operands are always `int` or `float`.",
        "Expecting `/` on a type parameter to become a `Div` bound the way `+` becomes an `Add` bound.",
    ],
    fixes: &[
        "Declare the operands as `int` or `float`: `def modulo(a: int, b: int) -> int`.",
        "Bound the parameter by a trait that defines the operator's hook, `def modulo[T with Remainder](a: T, b: T) -> T`, and implement `__mod__` on the trait.",
    ],
    docs_url: Some("https://encero-systems.github.io/incan/language/reference/numeric_semantics/"),
};

const METHOD_DECORATOR_RECEIVER_SPELLING: DiagnosticCatalogEntry = DiagnosticCatalogEntry {
    code: "INCAN-T0110",
    title: "Method decorator spells the receiver with `&`",
    severity: "error",
    phase: "typecheck",
    summary: "A method decorator's shape writes the decorated method's receiver as `&Owner` or `&mut Owner` instead of the way the method writes it.",
    explanation: "A decorator on a method receives the method as a callable whose first parameter is the receiver, and returns the callable that takes the method's place. Incan source spells that receiver the way the method does. A decorator on `def label(self, value: int) -> str` accepts `(Box, int) -> str`; a decorator on `def bump(mut self, value: int) -> int` accepts `(mut Box, int) -> int`, where `mut` marks the parameter whose changes the caller sees, as it does on a `def` parameter. A function a decorator returns in the method's place is declared the same way: `def parse(box: Box, value: int) -> int`, or `def grow(mut box: Box, value: int) -> int` for a `mut self` method. The compiler decides how the receiver is passed, so the `&Owner` and `&mut Owner` spellings are refused in that position; `&T` and `&mut T` stay available where a Rust signature needs them.",
    examples: &[
        "class Box:\n    value: int\n\n    @as_int\n    def label(self, value: int) -> str:\n        return \"value\"\n\ndef parse(box: &Box, value: int) -> int:\n    return value\n\ndef as_int(func: (&Box, int) -> str) -> (&Box, int) -> int:\n    return parse",
    ],
    common_causes: &[
        "A method decorator written before the receiver spelling changed, when RFC 036 required `&Owner` and `&mut Owner`.",
        "Carrying a Rust `&self` or `&mut self` signature over into an Incan callable type.",
    ],
    fixes: &[
        "For a `self` method, write the owner type: `(Box, int) -> str`, and `def parse(box: Box, value: int) -> int` for a function returned in the method's place.",
        "For a `mut self` method, mark it `mut`: `(mut Box, int) -> int`, and `def grow(mut box: Box, value: int) -> int` for a function returned in the method's place.",
    ],
    docs_url: Some("https://encero-systems.github.io/incan/language/reference/language/"),
};

const METHOD_DECORATOR_RECEIVER_NOT_PLANNED: DiagnosticCatalogEntry = DiagnosticCatalogEntry {
    code: "INCAN-T0116",
    title: "Method decorator chain cannot take the receiver",
    severity: "error",
    phase: "typecheck",
    summary: "A decorator whose shapes name the receiver of a `self` method, or a function it returns in the method's place, is declared or used where the receiver cannot be passed the way the method passes it.",
    explanation: "A decorator on a `self` method whose shapes name the receiver, such as `def as_int(func: (Box, int) -> str) -> (Box, int) -> int`, takes that receiver the way the method's generated wrapper passes it, and so does every function it returns in the method's place, such as `def parse(box: Box, value: int) -> int`. That holds for a chain whose declarations meet every rule: each is a private function of the module that declares the method's type; the shapes that hold the receiver are written as callable types, not through a type alias; a decorator returns the decorated callable or a private function of that module, named directly, and uses the decorated callable only to return it; a factory returns such a decorator named directly; and each is named only in the decorator chain, except that a returned function is also called directly in its module. A declaration or use that breaks a rule is refused. A `mut self` method's chain is not subject to these rules: its receiver is written `mut Box`, which says how it is passed.",
    examples: &[
        "class Box:\n    value: int\n\n    @as_int\n    def label(self, value: int) -> str:\n        return \"value\"\n\ndef parse(box: Box, value: int) -> int:\n    return value\n\ndef as_int(func: (Box, int) -> str) -> (Box, int) -> int:\n    return parse\n\ndef apply(f: (Box, int) -> int, box: Box) -> int:\n    return f(box, 1)\n\ndef main() -> None:\n    println(apply(parse, Box(value=1)))",
    ],
    common_causes: &[
        "Passing a function that a `self`-method decorator returns as a value, or applying that decorator to a function.",
        "A decorator imported from another module, reached through a value, or declared `pub`.",
        "A decorator that returns a closure, a call result or a conditional expression instead of a named function.",
        "A decorator that calls the callable it decorates, or passes it on, instead of only returning it.",
    ],
    fixes: &[
        "Declare the decorator and the functions it returns as private functions beside the method's type, with callable-type shapes, and return them by name.",
        "Give the other use its own function instead of the one the decorator returns.",
    ],
    docs_url: Some("https://encero-systems.github.io/incan/language/reference/language/"),
};

const RESERVED_COMPILER_NAME: DiagnosticCatalogEntry = DiagnosticCatalogEntry {
    code: "INCAN-T0111",
    title: "Name uses the compiler's reserved `__incan_` prefix",
    severity: "error",
    phase: "typecheck",
    summary: "A declaration, binding, parameter or import alias is named with the `__incan_` prefix, which is reserved for the names the compiler generates.",
    explanation: "The compiler names the items and locals it generates with the `__incan_` prefix, such as the original function a decorator wraps. A source name with the same prefix could collide with one of them and stop the build on a duplicate definition, so every name a program declares or binds must start with something else: functions, statics, constants, types, fields, methods, parameters, type parameters, local and pattern bindings, and import aliases. A method named `__incan_new` is exempt on any type: it is the type's constructor hook, which the compiler looks up by that name.",
    examples: &[
        "@preserve()\npub def target() -> int:\n    return 40\n\npub def __incan_original_target() -> int:\n    return 2",
    ],
    common_causes: &[
        "A name copied from generated code or from an earlier error message.",
        "A helper named to look internal.",
    ],
    fixes: &[
        "Rename it without the prefix: `original_target`.",
        "Use a single leading underscore for a module-private helper: `_original_target`.",
    ],
    docs_url: Some("https://encero-systems.github.io/incan/language/reference/imports_and_modules/"),
};

const ROUTE_PAYLOAD_WITHOUT_JSON_FORM: DiagnosticCatalogEntry = DiagnosticCatalogEntry {
    code: "INCAN-T0112",
    title: "Route payload type has no JSON form",
    severity: "error",
    phase: "typecheck",
    summary: "A `@route` handler's `Json[T]`, `Query[T]` or `Path[T]` payload type does not derive `json`.",
    explanation: "The route wrappers convert their payload at the handler boundary through serde: a `Json[T]` parameter deserializes the request body as JSON, a `Query[T]` parameter the URL-encoded query string, and a `Path[T]` parameter the path segments; a `Json[T]` return serializes `T` as the JSON response body. A model or class gets that serde support from `@derive(json)` (or by adopting the needed `std.serde.json` trait), and so does every model or class the payload is built from, such as `Search` in `Json[list[Search]]`. A payload declared without it checked before but could not be built: the route registration needed the conversion and had none. The check refuses only a model or class whose declaration certainly has no JSON form; any other payload is left to the build.",
    examples: &[
        "from std.web import route, Json, Query\nimport std.async\n\nmodel Search:\n    q: str\n\n@route(\"/search\")\nasync def search(query: Query[Search]) -> Json[Search]:\n    return Json(query.value)",
    ],
    common_causes: &[
        "A payload model declared without `@derive(json)`.",
        "A payload model that carries only builtin derives such as `Clone` or `Eq`, none of which gives it a JSON form.",
    ],
    fixes: &[
        "Add `@derive(json)` above the model, with `from std.serde import json`.",
        "Read or return a type that already derives `json`.",
    ],
    docs_url: Some("https://encero-systems.github.io/incan/language/tutorials/web_framework/"),
};

const FIELD_LACKS_AUTOMATIC_DERIVES: DiagnosticCatalogEntry = DiagnosticCatalogEntry {
    code: "INCAN-T0113",
    title: "Field type cannot carry the automatic derives",
    severity: "error",
    phase: "typecheck",
    summary: "A `model` or `class` field, or an `enum` variant payload, has a type that does not implement `Clone` and `Debug`.",
    explanation: "A `model`, `class` or `enum` always derives `Clone` and `Debug`, and a derive holds only when every field type supports it. A field whose type implements neither, such as the task handle `JoinHandle[T]` or the race arm `RaceArm[R]`, or a `list`, `dict`, `Option`, `Result` or tuple holding one, leaves the declaration without a build. The diagnostic names the field, its type, and the derives the type lacks.",
    examples: &[
        "import std.async\nfrom std.async.task import JoinHandle\n\nmodel Pending:\n    handle: JoinHandle[int]",
    ],
    common_causes: &[
        "Storing a spawned task's handle next to its bookkeeping in a model.",
        "A newtype over a type without `Clone` used as a field type.",
    ],
    fixes: &[
        "Keep the value in a local variable, or pass it as a parameter.",
        "Store a value that supports both derives, such as the result the task produces.",
    ],
    docs_url: Some("https://encero-systems.github.io/incan/language/reference/derives_and_traits/"),
};

const HASHED_MEMBER_LACKS_EQ_HASH: DiagnosticCatalogEntry = DiagnosticCatalogEntry {
    code: "INCAN-T0114",
    title: "Set element or dict key without Eq and Hash",
    severity: "error",
    phase: "typecheck",
    summary: "A `set` element type or `dict` key type does not implement `Eq` and `Hash`.",
    explanation: "A set's elements and a dict's keys are compared and hashed. A declared `model`, `class`, `enum` or `newtype` implements `Eq` through `@derive(Eq)` or `@derive(Ord)` and `Hash` through `@derive(Hash)`; `float`, a `set`, a `dict` and a frozen collection do not implement `Hash`. The type is refused in an annotation, a set or dict literal, a dict comprehension, a `set(...)` call, and as the type argument of a generic function or method whose body uses its parameter as a set element or dict key, in this module, a source module it imports or a compiled library, which carries the requirement as inferred `Eq` and `Hash` bounds. A caller's own type parameter passed to a compiled library's such function is refused unless it is declared `with (Eq, Hash)`.",
    examples: &["enum Tag:\n    A\n\ndef main() -> None:\n    tags: set[Tag] = {Tag.A}"],
    common_causes: &[
        "An enum or model used as a set element or dict key without `@derive(Eq, Hash)`.",
        "A `float` or a collection used as a dict key.",
    ],
    fixes: &[
        "Add `@derive(Eq, Hash)` to the declaration the diagnostic names.",
        "Key by a hashable field of the value, such as an `int` or `str` identifier.",
    ],
    docs_url: Some("https://encero-systems.github.io/incan/language/reference/derives/comparison/"),
};

const ARGUMENT_IS_NOT_A_TASK: DiagnosticCatalogEntry = DiagnosticCatalogEntry {
    code: "INCAN-T0115",
    title: "Argument is not a task",
    severity: "error",
    phase: "typecheck",
    summary: "A function value, or a value of a type that is not awaitable, is passed where a task is required.",
    explanation: "`spawn`, `timeout`, `timeout_ms`, `race_timeout` and `arm` take a task: a direct call of an `async def` written as the argument, a `JoinHandle[T]`, or another awaitable value. An `async def` named without being called is the function, not the task; a name bound to a call's result is typed as that result, not as a task; a function that is not `async def`, or a value such as an `int`, is not awaitable at all.",
    examples: &[
        "import std.async\nfrom std.async.task import spawn\n\nasync def work() -> int:\n    return 41\n\nasync def main() -> None:\n    handle = spawn(work)",
    ],
    common_causes: &[
        "Passing an async function by name instead of calling it.",
        "Passing the result of a function that is not `async def`.",
    ],
    fixes: &[
        "Call the async function where it is passed: `spawn(work())`.",
        "Declare the function with `async def`, or run blocking work with `spawn_blocking`.",
    ],
    docs_url: Some("https://encero-systems.github.io/incan/language/reference/stdlib/async/"),
};

const IMMUTABLE_ARGUMENT_TO_MUT_PARAMETER: DiagnosticCatalogEntry = DiagnosticCatalogEntry {
    code: "INCAN-T0117",
    title: "Immutable argument for a changed `mut` parameter",
    severity: "error",
    phase: "typecheck",
    summary: "A `mut` parameter whose changes reach the caller receives an immutable binding, a field of one, a collection element or a static, and the callee changes it or the value cannot be copied.",
    explanation: "A parameter declared `mut` is a mutable binding inside its function. When its type is not `int`, `float`, `bool` or a Rust type, and it is not a `*args` or `**kwargs` parameter, the function's changes to it are visible to the caller after the call. When the call changes such a parameter, the argument has to be a place the caller may change: a binding or parameter declared `mut`, `self` in a `mut self` method, or a field of one of those. An immutable binding or a field of one is refused, and so are an element of a list or dict and a static, whose change would reach only a copy. A literal or a call result is accepted, and so is any argument for a parameter the call never changes, except one the caller may not change whose type cannot be copied, such as a `Generator`: such an argument reaches the parameter as a copy, and there is none. A call changes the parameter when the body that runs assigns to its elements or fields, calls a method that takes `mut self` or a builtin collection method that adds, removes or reorders elements, uses it when it is a `Generator` (iterating it, calling its methods and passing it on all advance it), or passes it on to a parameter that is changed; a change through the variable of a `for` loop over a list parameter itself, over a field of it, or over the variable of an enclosing such loop, or through a name a `match`, `if let` or `while let` pattern binds from the parameter, is a change to the parameter, and a `for` loop variable passed to such a parameter is refused like an immutable binding. A method called through a type parameter's bound, on `self` in a trait default or on a trait-typed value counts as changing the parameter, because any adopter's method may run; so does a callee whose body the check does not read, such as a compiled library's function or a callable known only by a type that marks the parameter `mut`. A call through a local bound to a function is checked as a call of that function while the local is never reassigned; a call through a reassigned local counts as changing each parameter its type marks `mut`.",
    examples: &[
        "def extend(mut items: list[int]) -> None:\n    items.append(9)\n\ndef main() -> None:\n    items: list[int] = [1, 2]\n    extend(items)",
        "def extend(mut items: list[int]) -> None:\n    items.append(9)\n\ndef main() -> None:\n    mut rows: list[list[int]] = [[1]]\n    extend(rows[0])",
    ],
    common_causes: &[
        "A binding declared without `mut` passed to a function that changes it.",
        "A list element, dict value or static passed straight to a function that changes it.",
        "A generator bound without `mut` passed to a `mut` `Generator` parameter.",
    ],
    fixes: &[
        "Declare the binding with `mut`: `mut items: list[int] = [1, 2]`.",
        "Bind the element or static to a `mut` variable, pass the variable, and store it back.",
    ],
    docs_url: Some("https://encero-systems.github.io/incan/language/reference/functions/"),
};

const KEPT_DICT_LOOKUP_VALUE_CANNOT_BE_COPIED: DiagnosticCatalogEntry = DiagnosticCatalogEntry {
    code: "INCAN-T0118",
    title: "Dict lookup of a value that cannot be copied",
    severity: "error",
    phase: "typecheck",
    summary: "A `dict.get(key)` result is a copy of the stored value, and the dict's value type cannot be copied.",
    explanation: "`get(key)` returns `Some` holding a copy of the value stored for the key, or `None` when the key is absent. A value type that cannot be copied, such as a `Generator`, a Rust type without `Clone`, or a type that holds one, has no copy, so the lookup is refused.",
    examples: &[
        "def numbers() -> Generator[int]:\n    yield 1\n\ndef pick(streams: dict[str, Generator[int]], key: str) -> Option[Generator[int]]:\n    return streams.get(key)",
    ],
    common_causes: &[
        "Returning or binding the result of `get` on a dict of generators or of Rust values without `Clone`.",
    ],
    fixes: &["Store a value type that can be copied, or keep the value outside the dict."],
    docs_url: Some("https://encero-systems.github.io/incan/language/reference/language/"),
};

const TAKEN_LIST_USED_AGAIN: DiagnosticCatalogEntry = DiagnosticCatalogEntry {
    code: "INCAN-T0119",
    title: "List used again after a `for` loop took its items",
    severity: "error",
    phase: "typecheck",
    summary: "A `for` loop took the task handles out of a list, and the list is then read inside or after the loop, a closure captured it before the loop, or an enclosing loop repeats the loop; or a loop that hands each handle on iterates something it cannot take them out of.",
    explanation: "A `for` loop over a list reads each item where it stays, so the list keeps its items, and an item the loop body hands on by value is copied out of it. A `JoinHandle[T]` can be neither copied nor cloned, so when the body hands a handle on by value, by awaiting it, returning or yielding it, breaking with it, assigning it to another name, passing it to a call, placing it in a new tuple, list, set or dict, or iterating it (a list of handles) in a nested `for` loop that takes its items, the loop takes every handle out of the list instead; a tuple item that contains a handle counts the same way. That is only possible when nothing reads the list again, so these are refused: a read of the list inside the loop or after it, until an assignment gives the name a new list on every path after the loop, with no branch, `break` or `continue` able to skip it; a closure that captured the list before the loop; and an enclosing loop that runs the `for` loop again over a list defined outside it, unless each pass assigns the list a new one before the loop and before any read of the list in that pass. The loop can take the handles out of a local list, the variable of an enclosing loop that owns its items (it takes them, or iterates a list literal, a comprehension or a call result), or a parameter not marked `mut`, and a loop over a list literal, a comprehension or a call result receives each handle by value already; a loop that hands each handle on over anything else, such as a list element, a field, a `list`, `dict` or `set` method like `values()`, `enumerate(...)`, `zip(...)`, a `mut` parameter or the variable of a loop that reads its items in place, can take them out of neither it nor a copy, and is refused.",
    examples: &[
        "from std.async import spawn\n\nasync def work() -> int:\n    return 1\n\nasync def main() -> None:\n    handles = [spawn(work()), spawn(work())]\n    for handle in handles:\n        match await handle:\n            Ok(value) => println(value)\n            Err(_) => println(\"join failed\")\n    println(len(handles))",
    ],
    common_causes: &[
        "Reading the length or the items of a list of task handles after awaiting each handle in a loop.",
        "Awaiting the handles of a list built outside an enclosing `while` or `loop:`.",
        "A closure made before the loop that reads the list.",
        "Awaiting the handles of a list element, a field, a dict's `values()` or a `mut` parameter.",
    ],
    fixes: &[
        "Read what the later code needs from the list before the loop, such as `count = len(handles)`.",
        "Collect the results in a new list inside the loop and use that list afterwards.",
        "Build the list inside the enclosing loop, so each pass iterates a list of its own.",
        "Have the closure read a value taken from the list before the loop instead of the list itself.",
        "Iterate a local list that holds the handles, or take them out one at a time with `pop()`.",
    ],
    docs_url: Some("https://encero-systems.github.io/incan/language/reference/stdlib/async/"),
};

const IMPORT: DiagnosticCatalogEntry = DiagnosticCatalogEntry {
    code: "INCAN-I0001",
    title: "Import or module resolution error",
    severity: "error",
    phase: "import",
    summary: "The compiler could not resolve or load a source, stdlib, Rust, or public package import.",
    explanation: "Import diagnostics cover missing source modules, private exports, unresolved `pub::` libraries, invalid dependency manifests, and Rust bridge resolution failures.",
    examples: &["from missing import value", "from pub::unknown import helper"],
    common_causes: &[
        "The imported module does not exist relative to the source root.",
        "A dependency library has not been built with `incan build --lib`.",
        "The symbol exists but is not exported publicly.",
    ],
    fixes: &[
        "Check the import path and public exports.",
        "For `pub::` imports, build the dependency library and verify `loaf.toml` dependencies.",
    ],
    docs_url: Some("https://encero-systems.github.io/incan/language/reference/modules/"),
};

const SDK_COMPONENT_DISABLED: DiagnosticCatalogEntry = DiagnosticCatalogEntry {
    code: "INCAN-I0101",
    title: "SDK component disabled",
    severity: "error",
    phase: "import",
    summary: "A known SDK provider owns the imported module, but the project did not enable its component.",
    explanation: "The active SDK knows which component owns the module, but the resolved project profile and component refinements exclude that component. Imports report use; they do not change project SDK composition.",
    examples: &["from std.web import App  # with the minimal SDK profile"],
    common_causes: &[
        "The project selected `minimal` without adding the required component.",
        "The component was explicitly excluded under `[sdk]`.",
    ],
    fixes: &[
        "Add the named component to `[sdk].components`.",
        "Select an SDK profile that enables the component.",
    ],
    docs_url: Some("https://encero-systems.github.io/incan/tooling/reference/sdk_components_and_package_features/"),
};

const SDK_COMPONENT_UNAVAILABLE: DiagnosticCatalogEntry = DiagnosticCatalogEntry {
    code: "INCAN-I0102",
    title: "SDK component unavailable",
    severity: "error",
    phase: "import",
    summary: "The project enabled an SDK component whose provider payload is absent from the active installation.",
    explanation: "Project enablement and installed availability are separate. Compilation never downloads a missing component, so the active SDK installation must already contain the named provider payload.",
    examples: &["from std.fs import Path  # with a minimal SDK installation that omits stdlib-system"],
    common_causes: &[
        "The installed SDK distribution does not contain the selected component.",
        "A project created against a fuller SDK is being built with a smaller installation.",
    ],
    fixes: &["Install an SDK distribution containing the named component, then rerun the command."],
    docs_url: Some("https://encero-systems.github.io/incan/tooling/reference/sdk_components_and_package_features/"),
};

const PACKAGE_FEATURE_DISABLED: DiagnosticCatalogEntry = DiagnosticCatalogEntry {
    code: "INCAN-I0103",
    title: "Package feature disabled",
    severity: "error",
    phase: "import",
    summary: "The imported package declaration exists only in a public feature projection that is not active.",
    explanation: "Incan package features are additive package-owned selections. The provider manifest retains the conditioned fact so the compiler can name the required feature without reparsing dependency source.",
    examples: &["from pub::reporting import JsonReport  # reporting/json is disabled"],
    common_causes: &[
        "The dependency disabled default features and did not request the required feature.",
        "The requested declaration belongs to another additive feature set.",
    ],
    fixes: &["Add the suggested public feature set to the dependency declaration in `loaf.toml`."],
    docs_url: Some("https://encero-systems.github.io/incan/tooling/reference/sdk_components_and_package_features/"),
};

const TOOLING: DiagnosticCatalogEntry = DiagnosticCatalogEntry {
    code: "INCAN-C0001",
    title: "CLI or tooling error",
    severity: "error",
    phase: "tooling",
    summary: "The compiler command could not read inputs or complete a tooling operation.",
    explanation: "Tooling diagnostics are produced before or around the compiler pipeline, such as missing files, unreadable inputs, invalid command targets, or toolchain setup failures.",
    examples: &["incan check missing.incn"],
    common_causes: &[
        "The path does not exist.",
        "The file is too large or cannot be read.",
        "The local toolchain is missing a required target or dependency.",
    ],
    fixes: &[
        "Verify the command path and filesystem permissions.",
        "Run `incan tools doctor` for local toolchain problems.",
    ],
    docs_url: Some("https://encero-systems.github.io/incan/tooling/reference/cli_reference/"),
};

const UNKNOWN: DiagnosticCatalogEntry = DiagnosticCatalogEntry {
    code: "INCAN-U0001",
    title: "Unknown diagnostic code",
    severity: "error",
    phase: "unknown",
    summary: "The requested diagnostic code is not in this compiler's catalog.",
    explanation: "Diagnostic codes are versioned with the compiler. A code may be misspelled, from a newer compiler, or not yet assigned to a catalog entry.",
    examples: &["incan explain INCAN-NOPE"],
    common_causes: &[
        "Typo in the diagnostic code.",
        "Using docs from a different compiler version.",
    ],
    fixes: &[
        "Check the code printed by `incan check --format json`.",
        "Upgrade the compiler if the code comes from newer documentation.",
    ],
    docs_url: Some("https://encero-systems.github.io/incan/tooling/reference/cli_reference/"),
};

const CATALOG: &[DiagnosticCatalogEntry] = &[
    PARSER_SYNTAX,
    TYPECHECK,
    UNREACHABLE_CODE,
    SELF_MUTATION_REQUIRES_MUT_SELF,
    VALUE_WITHOUT_PRINTED_FORM,
    COLLECTION_ANNOTATION_REQUIRES_TYPE_ARGUMENTS,
    RUST_OWNER_TYPE_ARGS_NOT_INFERRED,
    CALLABLE_MARKER_NOT_SUPPORTED,
    ROUTE_HANDLER_RETURN_NOT_RESPONSE,
    ROUTE_HANDLER_PARAMETER_UNBOUND,
    OPERATOR_HAS_NO_TYPE_PARAMETER_BOUND,
    METHOD_DECORATOR_RECEIVER_SPELLING,
    ROUTE_PAYLOAD_WITHOUT_JSON_FORM,
    METHOD_DECORATOR_RECEIVER_NOT_PLANNED,
    RESERVED_COMPILER_NAME,
    FIELD_LACKS_AUTOMATIC_DERIVES,
    HASHED_MEMBER_LACKS_EQ_HASH,
    ARGUMENT_IS_NOT_A_TASK,
    IMMUTABLE_ARGUMENT_TO_MUT_PARAMETER,
    KEPT_DICT_LOOKUP_VALUE_CANNOT_BE_COPIED,
    TAKEN_LIST_USED_AGAIN,
    IMPORT,
    SDK_COMPONENT_DISABLED,
    SDK_COMPONENT_UNAVAILABLE,
    PACKAGE_FEATURE_DISABLED,
    TOOLING,
    UNKNOWN,
];

/// Look up a public diagnostic explanation entry.
pub fn explain(code: &str) -> Option<&'static DiagnosticCatalogEntry> {
    CATALOG.iter().find(|entry| entry.code.eq_ignore_ascii_case(code))
}

/// Return every public catalog entry in deterministic order.
pub fn catalog_entries() -> &'static [DiagnosticCatalogEntry] {
    CATALOG
}

/// Select the stable public code for a compiler diagnostic.
pub fn code_for_error(error: &CompileError, phase: DiagnosticPhase) -> &'static str {
    if let Some(code) = error.stable_code() {
        return code;
    }
    match phase {
        DiagnosticPhase::Lex | DiagnosticPhase::Parse => PARSER_SYNTAX.code,
        DiagnosticPhase::Typecheck => TYPECHECK.code,
        DiagnosticPhase::Import => IMPORT.code,
        DiagnosticPhase::Tooling => TOOLING.code,
        DiagnosticPhase::Unknown => match error.kind {
            ErrorKind::Syntax => PARSER_SYNTAX.code,
            ErrorKind::Type => TYPECHECK.code,
            ErrorKind::Error | ErrorKind::Warning | ErrorKind::Lint => TOOLING.code,
        },
    }
}

/// Classify diagnostics that are emitted during typechecking but originate from import declaration spans.
pub fn phase_for_typecheck_span(program: &Program, span: Span) -> DiagnosticPhase {
    if program
        .declarations
        .iter()
        .any(|declaration| matches!(declaration.node, Declaration::Import(_)) && spans_overlap(span, declaration.span))
    {
        DiagnosticPhase::Import
    } else {
        DiagnosticPhase::Typecheck
    }
}

/// Convert a compiler diagnostic into the stable JSON-ready representation.
pub fn stable_diagnostic(
    file_name: &str,
    source: &str,
    error: &CompileError,
    phase: DiagnosticPhase,
) -> StableDiagnostic {
    let code = code_for_error(error, phase);
    let severity = match error.kind {
        ErrorKind::Error | ErrorKind::Syntax | ErrorKind::Type => "error",
        ErrorKind::Warning => "warning",
        ErrorKind::Lint => "hint",
    };
    StableDiagnostic {
        code,
        severity,
        phase,
        origin: origin_for_phase(phase),
        message: error.message.clone(),
        primary_span: diagnostic_span(file_name, source, error.span),
        notes: error.notes.clone(),
        hints: error.hints.clone(),
        expected: error.expected().map(str::to_owned),
        actual: error.actual().map(str::to_owned),
        related_spans: error
            .related_spans()
            .iter()
            .map(|related| DiagnosticRelatedSpan {
                span: diagnostic_span(file_name, source, related.span),
                label: related.label.clone(),
            })
            .collect(),
        related_declarations: error
            .related_declarations()
            .iter()
            .map(|related| DiagnosticRelatedDeclaration {
                identity: related.identity.clone(),
                label: related.label.clone(),
            })
            .collect(),
        explain: format!("incan explain {code}"),
    }
}

/// Select the stable producer identity for one compiler pipeline phase.
fn origin_for_phase(phase: DiagnosticPhase) -> DiagnosticOrigin {
    match phase {
        DiagnosticPhase::Lex => DiagnosticOrigin::Lexer,
        DiagnosticPhase::Parse => DiagnosticOrigin::Parser,
        DiagnosticPhase::Typecheck => DiagnosticOrigin::Typechecker,
        DiagnosticPhase::Import => DiagnosticOrigin::ImportResolver,
        DiagnosticPhase::Tooling => DiagnosticOrigin::Tooling,
        DiagnosticPhase::Unknown => DiagnosticOrigin::Unknown,
    }
}

/// Convert a compiler byte span into the public source-span projection.
fn diagnostic_span(file_name: &str, source: &str, span: Span) -> DiagnosticSpan {
    DiagnosticSpan {
        file: file_name.to_string(),
        start: position_for_offset(source, span.start),
        end: position_for_offset(source, span.end.max(span.start + 1)),
    }
}

/// Convert a byte offset into the 1-based line and column shape used by the stable diagnostic schema.
fn position_for_offset(source: &str, offset: usize) -> DiagnosticPosition {
    let offset = offset.min(source.len());
    let mut line = 1usize;
    let mut column = 1usize;
    for (idx, ch) in source.char_indices() {
        if idx >= offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            column = 1;
        } else {
            column += 1;
        }
    }
    DiagnosticPosition { line, column, offset }
}

/// Return whether two source spans overlap after treating zero-width spans as one-byte spans.
fn spans_overlap(left: Span, right: Span) -> bool {
    let left_end = left.end.max(left.start.saturating_add(1));
    let right_end = right.end.max(right.start.saturating_add(1));
    left.start < right_end && right.start < left_end
}

#[cfg(test)]
mod tests {
    use crate::ast::{ImportDecl, ImportKind, Spanned, Visibility};
    use crate::diagnostics::{errors, lints};

    use super::*;

    #[test]
    fn typecheck_phase_uses_import_span_for_import_diagnostics() {
        let program = Program {
            declarations: vec![
                Spanned::new(
                    Declaration::Import(ImportDecl {
                        visibility: Visibility::Private,
                        kind: ImportKind::PubLibrary {
                            library: "missing".to_string(),
                            path: Vec::new(),
                        },
                        alias: None,
                    }),
                    Span::new(4, 24),
                ),
                Spanned::new(Declaration::Docstring("body".to_string()), Span::new(40, 46)),
            ],
            ..Program::default()
        };

        assert_eq!(
            phase_for_typecheck_span(&program, Span::new(8, 12)),
            DiagnosticPhase::Import
        );
        assert_eq!(
            phase_for_typecheck_span(&program, Span::new(42, 44)),
            DiagnosticPhase::Typecheck
        );
    }

    #[test]
    fn stable_diagnostic_preserves_structured_facts_and_related_spans() {
        let error = CompileError::type_error("type mismatch".to_string(), Span::new(8, 13))
            .with_expected_actual("int", "str")
            .with_related_span(Span::new(0, 7), "Function parameter declared here");
        let diagnostic = stable_diagnostic("main.incn", "takes_x\n\"text\"\n", &error, DiagnosticPhase::Typecheck);

        assert_eq!(diagnostic.origin, DiagnosticOrigin::Typechecker);
        assert_eq!(diagnostic.expected.as_deref(), Some("int"));
        assert_eq!(diagnostic.actual.as_deref(), Some("str"));
        assert_eq!(diagnostic.related_spans.len(), 1);
        assert_eq!(diagnostic.related_spans[0].label, "Function parameter declared here");
        assert_eq!(diagnostic.related_spans[0].span.start.offset, 0);
    }

    #[test]
    fn stable_diagnostic_keeps_cross_file_declarations_out_of_primary_source_coordinates()
    -> Result<(), Box<dyn std::error::Error>> {
        use incan_semantics_core::{CanonicalSymbolId, HirSourceSpan, SemanticSourceTargetKind};

        let declaration = CanonicalSymbolId::module_declaration(
            vec!["provider".to_string()],
            "parse",
            SemanticSourceTargetKind::Function,
            HirSourceSpan::new(120, 180),
        );
        let error = CompileError::type_error("bad call through alias".to_string(), Span::new(0, 5))
            .with_related_declaration(declaration.clone(), "declaration of `parse`");
        let diagnostic = stable_diagnostic("consumer.incn", "alias()\n", &error, DiagnosticPhase::Typecheck);

        assert!(diagnostic.related_spans.is_empty());
        let [related] = diagnostic.related_declarations.as_slice() else {
            return Err(format!("expected one related declaration: {diagnostic:?}").into());
        };
        assert_eq!(related.identity, declaration);
        assert_eq!(related.identity.declaration_span.start, 120);
        Ok(())
    }

    #[test]
    fn provider_import_failures_use_distinct_stable_codes() {
        let disabled = errors::sdk_component_disabled("std.web", "stdlib-web", Span::default());
        let unavailable = errors::sdk_component_unavailable("std.web", "stdlib-web", "incan@0.5.0", Span::default());
        let gated = errors::pub_library_symbol_requires_features(
            "JsonReport",
            "reporting",
            &[vec!["json".to_string()]],
            Span::default(),
        );

        assert_eq!(code_for_error(&disabled, DiagnosticPhase::Import), "INCAN-I0101");
        assert_eq!(code_for_error(&unavailable, DiagnosticPhase::Import), "INCAN-I0102");
        assert_eq!(code_for_error(&gated, DiagnosticPhase::Import), "INCAN-I0103");
        for code in ["INCAN-I0101", "INCAN-I0102", "INCAN-I0103"] {
            assert!(explain(code).is_some(), "{code} must have a catalog explanation");
        }
    }

    #[test]
    fn checker_refusals_of_unbuildable_programs_use_distinct_stable_codes() -> Result<(), Box<dyn std::error::Error>> {
        let self_mutation = errors::self_mutation_requires_mut_self(
            "pop",
            "self.items",
            errors::SelfMutation::MutatingCall { callee: "pop" },
            Span::default(),
        );
        let print_union = errors::value_has_no_printed_form(
            errors::DisplayPosition::Print { builtin: "print" },
            Some("value"),
            errors::UnprintableValue::Union,
            Span::default(),
        );
        let str_of_model = errors::value_has_no_printed_form(
            errors::DisplayPosition::Str,
            None,
            errors::UnprintableValue::Nominal { type_name: "Account" },
            Span::default(),
        );
        let interpolated_union = errors::value_has_no_printed_form(
            errors::DisplayPosition::Interpolation,
            Some("value"),
            errors::UnprintableValue::Union,
            Span::default(),
        );
        let bound_enum = errors::value_has_no_printed_form(
            errors::DisplayPosition::Bound {
                callee: "show",
                type_param: "T",
            },
            None,
            errors::UnprintableValue::Nominal { type_name: "Color" },
            Span::default(),
        );
        let bare_tuple = errors::tuple_annotation_requires_element_types("Tuple", Span::default());
        let bare_option = errors::collection_annotation_requires_type_arguments("Option", Span::default());
        let open_generics = errors::rust_owner_type_args_not_inferred(
            "HashMap",
            "new",
            &["K".to_string(), "V".to_string()],
            Some("untyped"),
            Span::default(),
        );

        assert_eq!(
            code_for_error(&self_mutation, DiagnosticPhase::Typecheck),
            "INCAN-T0102"
        );
        for display in [&print_union, &str_of_model, &interpolated_union, &bound_enum] {
            assert_eq!(code_for_error(display, DiagnosticPhase::Typecheck), "INCAN-T0103");
        }
        assert_eq!(code_for_error(&bare_tuple, DiagnosticPhase::Typecheck), "INCAN-T0104");
        assert_eq!(code_for_error(&bare_option, DiagnosticPhase::Typecheck), "INCAN-T0104");
        assert_eq!(
            code_for_error(&open_generics, DiagnosticPhase::Typecheck),
            "INCAN-T0105"
        );
        for code in ["INCAN-T0102", "INCAN-T0103", "INCAN-T0104", "INCAN-T0105"] {
            let Some(entry) = explain(code) else {
                return Err(format!("{code} must have a catalog explanation").into());
            };
            assert_eq!(entry.severity, "error");
            assert_eq!(entry.phase, "typecheck");
        }
        assert!(
            open_generics
                .hints
                .iter()
                .any(|hint| hint.contains("HashMap.new[str, int]()") && hint.contains("untyped: HashMap[str, int]")),
            "the remedy must spell both the call and the binding form, got {:?}",
            open_generics.hints
        );
        assert_eq!(print_union.message, "'print' cannot print the union value 'value'");
        assert_eq!(
            str_of_model.message,
            "'str' cannot convert a value of type 'Account' to text"
        );
        assert_eq!(
            interpolated_union.message,
            "f-string cannot interpolate the union value 'value'"
        );
        assert_eq!(
            bound_enum.message,
            "Call to 'show' cannot bind a value of type 'Color' to 'T', which requires 'Display'"
        );
        assert!(
            bound_enum
                .hints
                .iter()
                .all(|hint| hint.contains("__str__") && !hint.contains(":?")),
            "a bound's remedy names __str__ and no f-string part the caller does not have, got {:?}",
            bound_enum.hints
        );
        assert_eq!(
            bare_option.message,
            "Option annotation 'Option' is missing its value type"
        );
        assert!(
            bare_option.hints.iter().any(|hint| hint.contains("'Option[int]'")),
            "the remedy must spell the parameterized annotation, got {:?}",
            bare_option.hints
        );
        Ok(())
    }

    #[test]
    fn reserved_compiler_name_refusal_uses_its_own_stable_code() -> Result<(), Box<dyn std::error::Error>> {
        let function = errors::reserved_compiler_name("__incan_original_target", "function", Span::default());
        let bare_prefix = errors::reserved_compiler_name("__incan_", "binding", Span::default());

        assert_eq!(code_for_error(&function, DiagnosticPhase::Typecheck), "INCAN-T0111");
        let Some(entry) = explain("INCAN-T0111") else {
            return Err("INCAN-T0111 must have a catalog explanation".into());
        };
        assert_eq!(entry.severity, "error");
        assert_eq!(entry.phase, "typecheck");
        assert!(
            function
                .hints
                .iter()
                .any(|hint| hint.contains("for example 'original_target'")),
            "the remedy must suggest the name without the prefix, got {:?}",
            function.hints
        );
        assert!(
            bare_prefix.hints.iter().all(|hint| !hint.contains("for example")),
            "a name that is only the prefix has nothing left to suggest, got {:?}",
            bare_prefix.hints
        );
        Ok(())
    }

    #[test]
    fn kept_dict_lookup_refusal_uses_its_stable_code_followups_a() -> Result<(), Box<dyn std::error::Error>> {
        let refusal = errors::kept_dict_lookup_value_cannot_be_copied("JoinHandle[int]", Span::default());
        assert_eq!(code_for_error(&refusal, DiagnosticPhase::Typecheck), "INCAN-T0118");
        let entry = explain("INCAN-T0118").ok_or("INCAN-T0118 must have a catalog explanation")?;
        assert_eq!(entry.severity, "error");
        assert_eq!(entry.phase, "typecheck");
        assert!(
            refusal.message.contains("JoinHandle[int]"),
            "the refusal must name the stored value type, got {:?}",
            refusal.message
        );
        Ok(())
    }

    #[test]
    fn immutable_argument_to_mut_parameter_uses_its_stable_code_issue1773() -> Result<(), String> {
        let binding = errors::immutable_argument_to_mut_parameter(
            errors::MutParameterLabel::Named("items"),
            "extend",
            errors::MutParameterChange::Changes,
            errors::MutArgumentPlace::Binding("items".to_string()),
            Span::default(),
        );
        let element = errors::immutable_argument_to_mut_parameter(
            errors::MutParameterLabel::Named("items"),
            "extend",
            errors::MutParameterChange::Changes,
            errors::MutArgumentPlace::Element,
            Span::default(),
        );
        let positional = errors::immutable_argument_to_mut_parameter(
            errors::MutParameterLabel::Position(1),
            "step",
            errors::MutParameterChange::MayChange,
            errors::MutArgumentPlace::Binding("counter".to_string()),
            Span::default(),
        );
        assert_eq!(
            positional.message,
            "Argument for the 'mut' parameter at position 1 of 'step' must be a mutable binding"
        );
        assert!(
            positional
                .notes
                .iter()
                .any(|note| note.starts_with("'step' may change")),
            "a callee whose body is not known is said to possibly change the parameter, got {:?}",
            positional.notes
        );
        assert!(
            binding.notes.iter().any(|note| note.starts_with("'extend' changes")),
            "a callee known to change the parameter is said to change it, got {:?}",
            binding.notes
        );
        for error in [&binding, &element, &positional] {
            assert_eq!(code_for_error(error, DiagnosticPhase::Typecheck), "INCAN-T0117");
        }
        let entry = explain("INCAN-T0117").ok_or("INCAN-T0117 must have a catalog explanation")?;
        assert_eq!(entry.severity, "error");
        assert_eq!(entry.phase, "typecheck");
        let uncopyable = errors::uncopyable_argument_to_mut_parameter(
            errors::MutParameterLabel::Named("g"),
            "ignore",
            "Generator[int]",
            errors::MutArgumentPlace::Binding("g".to_string()),
            Span::default(),
        );
        assert_eq!(code_for_error(&uncopyable, DiagnosticPhase::Typecheck), "INCAN-T0117");
        assert!(
            uncopyable
                .notes
                .iter()
                .any(|note| note.contains("does not change") && note.contains("'Generator[int]' cannot be copied")),
            "a refusal for an unchanged parameter says the argument cannot be copied, got {:?}",
            uncopyable.notes
        );
        assert!(
            binding.hints.iter().any(|hint| hint.contains("mut items = ...")),
            "an immutable binding's remedy names the binding to declare 'mut', got {:?}",
            binding.hints
        );
        assert!(
            element.hints.iter().any(|hint| hint.contains("store it back")),
            "an element's remedy says to pass a 'mut' variable and store it back, got {:?}",
            element.hints
        );
        Ok(())
    }

    #[test]
    fn kept_dict_lookup_of_a_value_that_cannot_be_copied_uses_its_stable_code() {
        let refusal = errors::kept_dict_lookup_value_cannot_be_copied("Generator[int]", Span::default());
        assert_eq!(code_for_error(&refusal, DiagnosticPhase::Typecheck), "INCAN-T0118");
        let Some(entry) = explain("INCAN-T0118") else {
            panic!("INCAN-T0118 must have a catalog explanation");
        };
        assert_eq!(entry.severity, "error");
        assert_eq!(entry.phase, "typecheck");
        assert!(
            refusal.message.contains("Generator[int]"),
            "the refusal names the value type"
        );
        assert!(
            refusal
                .hints
                .iter()
                .any(|hint| hint.contains("`len`, `print` or `println`") && hint.contains("f-string")),
            "the hint names the uses that count as reading, got {:?}",
            refusal.hints
        );
        assert!(
            refusal.hints.iter().any(|hint| {
                hint.contains("a Rust method with a shared receiver whose result is not kept")
                    && hint.contains("an Incan method taking `self` does not count")
            }),
            "the hint names exactly which method calls count as reading, got {:?}",
            refusal.hints
        );
    }

    #[test]
    fn route_handler_and_type_parameter_operator_refusals_use_distinct_stable_codes() {
        let return_type = errors::route_handler_return_not_response("create_user", "int", Span::default());
        let unbound = errors::route_handler_parameter_unbound(
            "create",
            "id",
            "/things",
            &["year".to_string(), "month".to_string()],
            Span::default(),
        );
        let operator = errors::operator_has_no_type_parameter_bound("%", "T", "__mod__", Span::default());
        let payload = errors::route_payload_without_json_form("search", "Query[Search]", "Search", Span::default());

        assert_eq!(code_for_error(&return_type, DiagnosticPhase::Typecheck), "INCAN-T0107");
        assert_eq!(code_for_error(&unbound, DiagnosticPhase::Typecheck), "INCAN-T0108");
        assert_eq!(code_for_error(&operator, DiagnosticPhase::Typecheck), "INCAN-T0109");
        assert_eq!(code_for_error(&payload, DiagnosticPhase::Typecheck), "INCAN-T0112");
        assert!(
            payload.hints.iter().any(|hint| hint.contains("'@derive(json)'")),
            "the remedy must name the JSON derive, got {:?}",
            payload.hints
        );
        for code in ["INCAN-T0107", "INCAN-T0108", "INCAN-T0109", "INCAN-T0112"] {
            let Some(entry) = explain(code) else {
                panic!("{code} must have a catalog explanation");
            };
            assert_eq!(entry.severity, "error");
            assert_eq!(entry.phase, "typecheck");
        }
        assert!(
            unbound.hints.iter().any(|hint| hint.contains("'/things/{id}'")),
            "the remedy must spell the path with the segment added, got {:?}",
            unbound.hints
        );
        assert!(
            unbound
                .notes
                .iter()
                .any(|note| note == "The path binds 'year', 'month'"),
            "the captures the path does bind must be listed, got {:?}",
            unbound.notes
        );
        assert!(
            operator.hints.iter().any(|hint| hint.contains("'__mod__'")),
            "the remedy must name the operator's trait hook, got {:?}",
            operator.hints
        );
    }

    /// Issue #1790: the refused receiver spelling on a method decorator has its own explainable code, and its remedy
    /// names the spelling to write instead.
    #[test]
    fn method_decorator_receiver_spelling_refusal_uses_a_distinct_stable_code() -> Result<(), Box<dyn std::error::Error>>
    {
        let shared = errors::method_decorator_receiver_spelling(
            "Method decorator '@as_int'",
            "label",
            "&Box",
            "(Box, int) -> str",
            false,
            Span::default(),
        );
        let mutable = errors::method_decorator_receiver_spelling(
            "Method decorator '@keep'",
            "bump",
            "&mut Counter",
            "(mut Counter, int) -> int",
            true,
            Span::default(),
        );

        for (refusal, shape, receiver) in [
            (&shared, "`(Box, int) -> str`", "`self`"),
            (&mutable, "`(mut Counter, int) -> int`", "`mut self`"),
        ] {
            assert_eq!(code_for_error(refusal, DiagnosticPhase::Typecheck), "INCAN-T0110");
            assert!(
                refusal.hints.iter().any(|hint| hint.contains(shape)
                    && hint.contains(receiver)
                    && hint.contains("the compiler decides how the receiver is passed")),
                "the remedy must spell the replacement shape, got {:?}",
                refusal.hints
            );
        }
        let Some(entry) = explain("INCAN-T0110") else {
            return Err("INCAN-T0110 must have a catalog explanation".into());
        };
        assert_eq!(entry.severity, "error");
        assert_eq!(entry.phase, "typecheck");
        Ok(())
    }

    /// Issue #1790: a `self`-method decorator chain the receiver cannot be passed through has its own explainable
    /// code, and the refusal says what is refused, why, and what to write instead.
    #[test]
    fn method_decorator_receiver_plan_refusal_uses_a_distinct_stable_code() -> Result<(), Box<dyn std::error::Error>> {
        let refusal = errors::method_decorator_receiver_not_planned(
            "'parse' cannot be used here",
            "it takes the place of `self` method 'label' through '@as_int', so it is only returned there or called \
             directly",
            "Call 'parse' directly, or give this use a function of its own",
            Span::default(),
        );
        assert_eq!(code_for_error(&refusal, DiagnosticPhase::Typecheck), "INCAN-T0116");
        assert_eq!(
            refusal.message,
            "'parse' cannot be used here: it takes the place of `self` method 'label' through '@as_int', so it is \
             only returned there or called directly"
        );
        assert!(
            refusal
                .hints
                .iter()
                .any(|hint| hint == "Call 'parse' directly, or give this use a function of its own"),
            "the refusal must say what to write instead, got {:?}",
            refusal.hints
        );
        let Some(entry) = explain("INCAN-T0116") else {
            return Err("INCAN-T0116 must have a catalog explanation".into());
        };
        assert_eq!(entry.severity, "error");
        assert_eq!(entry.phase, "typecheck");
        Ok(())
    }

    /// Issue #1844: a list used again after a `for` loop took its items has its own explainable code, and the
    /// refusal names the list, points back at the loop and says what to write instead.
    #[test]
    fn taken_list_reuse_refusal_uses_a_distinct_stable_code() -> Result<(), Box<dyn std::error::Error>> {
        let refusal = errors::taken_list_used_again(
            "handles",
            "JoinHandle[int]",
            errors::TakenListReuse::AfterLoop,
            Span::new(90, 97),
            Span::new(40, 47),
        );
        assert_eq!(code_for_error(&refusal, DiagnosticPhase::Typecheck), "INCAN-T0119");
        assert_eq!(
            refusal.message,
            "`handles` is used after the `for` loop that took its items"
        );
        assert_eq!(
            refusal.related_spans().len(),
            1,
            "the refusal must point back at the loop"
        );
        assert!(
            refusal.hints.iter().any(|hint| hint.contains("before the loop")),
            "the refusal must say what to write instead, got {:?}",
            refusal.hints
        );
        let Some(entry) = explain("INCAN-T0119") else {
            return Err("INCAN-T0119 must have a catalog explanation".into());
        };
        assert_eq!(entry.severity, "error");
        assert_eq!(entry.phase, "typecheck");
        let untakeable = errors::loop_cannot_take_items(
            errors::UntakeableIterable::MutParameter("handles"),
            "JoinHandle[int]",
            Span::new(40, 47),
        );
        assert_eq!(code_for_error(&untakeable, DiagnosticPhase::Typecheck), "INCAN-T0119");
        assert_eq!(
            untakeable.message,
            "the `for` loop cannot take its `JoinHandle[int]` items out of `handles`, a `mut` parameter whose items belong to the caller"
        );
        Ok(())
    }

    #[test]
    fn capability_requirement_refusals_use_distinct_stable_codes() -> Result<(), Box<dyn std::error::Error>> {
        let field = errors::member_type_lacks_automatic_derives(
            "model",
            "Pending",
            errors::DerivedMember::Field("handle"),
            "JoinHandle[int]",
            "JoinHandle[int]",
            &["Clone", "Debug"],
            Span::default(),
        );
        let key = errors::collection_member_lacks_hash_derives(
            errors::HashedCollectionRole::SetElement,
            "Tag",
            "Tag",
            &["Eq", "Hash"],
            errors::HashRemedy::AddDerives,
            Span::default(),
        );
        let instantiation = errors::type_argument_lacks_hash_derives(
            "unique",
            "T",
            "Tag",
            "Tag",
            &["Hash"],
            errors::HashRemedy::AddDerives,
            Span::default(),
        );
        let task =
            errors::argument_is_not_a_task("spawn", errors::TaskArgument::AsyncFunction("work"), Span::default());

        assert_eq!(code_for_error(&field, DiagnosticPhase::Typecheck), "INCAN-T0113");
        assert_eq!(code_for_error(&key, DiagnosticPhase::Typecheck), "INCAN-T0114");
        assert_eq!(
            code_for_error(&instantiation, DiagnosticPhase::Typecheck),
            "INCAN-T0114"
        );
        assert_eq!(code_for_error(&task, DiagnosticPhase::Typecheck), "INCAN-T0115");
        for code in ["INCAN-T0113", "INCAN-T0114", "INCAN-T0115"] {
            let Some(entry) = explain(code) else {
                return Err(format!("{code} must have a catalog explanation").into());
            };
            assert_eq!(entry.severity, "error");
            assert_eq!(entry.phase, "typecheck");
        }
        assert!(
            key.hints.iter().any(|hint| hint.contains("@derive(Eq, Hash)")),
            "the remedy must name the derives to add, got {:?}",
            key.hints
        );
        assert!(
            task.hints
                .iter()
                .any(|hint| hint.contains("'work()' in place of 'work'")),
            "the remedy must rewrite only the offending argument, got {:?}",
            task.hints
        );
        Ok(())
    }

    #[test]
    fn unreachable_code_warning_projects_as_an_explainable_typecheck_warning() {
        let warning = lints::unreachable_code_after_return(Span::new(20, 40), Span::new(8, 16));
        let diagnostic = stable_diagnostic(
            "main.incn",
            "def f() -> int:\n    return 1\n    println(\"dead\")\n",
            &warning,
            DiagnosticPhase::Typecheck,
        );

        assert_eq!(diagnostic.code, "INCAN-T0101");
        assert_eq!(diagnostic.severity, "warning");
        assert_eq!(diagnostic.phase, DiagnosticPhase::Typecheck);
        assert_eq!(diagnostic.origin, DiagnosticOrigin::Typechecker);
        assert_eq!(diagnostic.explain, "incan explain INCAN-T0101");
        assert_eq!(
            diagnostic.related_spans.len(),
            1,
            "the projection must keep the originating `return` location"
        );
        assert!(
            explain("INCAN-T0101").is_some(),
            "INCAN-T0101 must have a catalog explanation"
        );
    }
}
