//! The stable diagnostic catalog entries and their lookup.

use super::*;

pub(super) const PARSER_SYNTAX: DiagnosticCatalogEntry = DiagnosticCatalogEntry {
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

pub(super) const TYPECHECK: DiagnosticCatalogEntry = DiagnosticCatalogEntry {
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

pub(super) const UNREACHABLE_CODE: DiagnosticCatalogEntry = DiagnosticCatalogEntry {
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

pub(super) const CALLABLE_MARKER_NOT_SUPPORTED: DiagnosticCatalogEntry = DiagnosticCatalogEntry {
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

pub(super) const SELF_MUTATION_REQUIRES_MUT_SELF: DiagnosticCatalogEntry = DiagnosticCatalogEntry {
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

pub(super) const VALUE_WITHOUT_PRINTED_FORM: DiagnosticCatalogEntry = DiagnosticCatalogEntry {
    code: "INCAN-T0103",
    title: "Value with no printed form",
    severity: "error",
    phase: "typecheck",
    summary: "A displayed value (a `print` or `println` argument, the argument of `str(...)`, an f-string `{value}` part, or a type argument for a `Display` bound) has no printed form: a union value, a generator, a function, `bytes`, a value of type `None`, or a model, class, enum or newtype that provides no `Display`. Or an f-string `{value:?}` part renders a value with no `Debug` form: a generator, a function, or a value whose type implements no `Debug`.",
    explanation: "`print`, `println`, `str`, an f-string `{value}` and a `Display` bound share one display rule. Scalars and `str` display their own text; a tuple, list, dict, set, `Option` or `Result` displays its structure (`(10, 20)`, `[1, 2, 3]`, `Some(1)`, `Err(\"bad\")`) in the four value positions. A model, class, enum or newtype displays through `Display`, which it provides by a `__str__(self) -> str` method, by each variant's value when it is an enum that declares values, or by `message()` when it adopts `Error` and has no `__str__`; `@derive(Display)` provides nothing. Every other value has no printed form, and displaying one in any position is refused: a union value until it is narrowed to one member, a generator until its items are collected, a function until it is called, `bytes` until they are decoded, a value of type `None` (what a function that returns `None` gives back, or a side of a `Result` that nothing gives another type), and a model, class, enum or newtype until it provides `Display`. `{value:?}` renders a value's structure through `Debug` instead; a generator, a function, and a value whose type, or a type inside it, implements no `Debug` (such as a `JoinHandle[T]`) have no such structure, and a `{value:?}` part over one is refused too.",
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

pub(super) const COLLECTION_ANNOTATION_REQUIRES_TYPE_ARGUMENTS: DiagnosticCatalogEntry = DiagnosticCatalogEntry {
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

pub(super) const ITERATED_PLACE_REPLACEMENT: DiagnosticCatalogEntry = DiagnosticCatalogEntry {
    code: "INCAN-T0121",
    title: "Replacement of an iterated collection",
    severity: "error",
    phase: "typecheck",
    summary: "A statement loop still borrows a list whose storage an assignment would replace.",
    explanation: "Direct list iteration borrows the existing collection. Replacing that collection or its parent invalidates the iterator and its item views. Duckborrowing cannot repair the conflict with an implicit clone because that would change which storage the loop observes.",
    examples: &["for item in client.pending:\n    client.pending = []"],
    common_causes: &["Replacing a list field inside a loop over the same field."],
    fixes: &["Finish the borrowed iteration before replacing the collection, or iterate an explicit clone."],
    docs_url: None,
};

pub(super) const RUST_BORROW_WITHOUT_LIFETIME_CONTRACT: DiagnosticCatalogEntry = DiagnosticCatalogEntry {
    code: "INCAN-T0120",
    title: "Returned or stored Rust borrow requires a lifetime contract",
    severity: "error",
    phase: "typecheck",
    summary: "A source return or field type names a lifetime-bearing Rust type without a retained lifetime contract.",
    explanation: "A lifetime-bearing Rust type borrows data from somewhere. A parameter of that type is accepted: each call binds its lifetimes independently. A returned value or a stored field needs the relationship between the value and the data it borrows, which no source type states, so DD-0006 refuses it before it reaches rustc.",
    examples: &[
        "from rust::regex import Captures, Regex\n\ndef first(pattern: &Regex, text: str) -> Option[Captures]:\n    return pattern.captures(text)",
    ],
    common_causes: &["Returning a regex capture value from a helper function."],
    fixes: &[
        "Keep the borrowed native value inside the function that creates it, or return owned data such as the captured text.",
    ],
    docs_url: None,
};

pub(super) const RUST_CALLEE_NOT_SELECTED: DiagnosticCatalogEntry = DiagnosticCatalogEntry {
    code: "INCAN-T0122",
    title: "Rust callee cannot be selected",
    severity: "error",
    phase: "typecheck",
    summary: "A Rust method call's arguments check, but no single native callee can be selected for it, so it cannot be compiled.",
    explanation: "A native method call compiles only through the one Rust function the compiler selects for it: an inherent method of the receiver's type, or the method of one trait implementation. Selection fails in three ways. A type parameter of the callee stays open, because nothing in the receiver, the arguments, the type the result is expected to have or the first use of the local the result is bound to fixes it. No admitted implementation applies, as when an imported trait declares the method but the receiver's type does not implement that trait. Or several implementations apply equally and nothing in the call chooses one. The message names the callee and which of the three it is.",
    examples: &[
        "from rust::std::io import Write\nfrom rust::std::string import String as RustString\n\ndef main() -> None:\n    mut text = RustString.from(\"a\")\n    text.flush()",
        "from rust::std::convert import AsRef\nfrom rust::std::path import PathBuf\n\ndef main() -> None:\n    path = PathBuf.from(\"a\")\n    path.as_ref()",
    ],
    common_causes: &[
        "Calling a method of an imported trait on a value whose type does not implement that trait.",
        "A generic method whose type parameter only its result names, with nothing fixing the result's type.",
        "A method that several trait implementations of the receiver's type provide, such as `as_ref()`.",
    ],
    fixes: &[
        "Write the type arguments in the call, or annotate the type the result is bound to.",
        "Call the method on a value whose type implements the trait that declares it.",
        "Call a method or conversion whose signature names the type it produces.",
    ],
    docs_url: Some("https://encero-systems.github.io/incan/language/how-to/rust_interop/"),
};

pub(super) const RUST_OWNER_TYPE_ARGS_NOT_INFERRED: DiagnosticCatalogEntry = DiagnosticCatalogEntry {
    code: "INCAN-T0105",
    title: "Rust type arguments cannot be inferred",
    severity: "error",
    phase: "typecheck",
    summary: "A Rust associated call leaves its owner's type arguments open and nothing later in the program fixes them.",
    explanation: "`HashMap.new()` on `rust::std::collections::HashMap` leaves `K` and `V` open. The first use whose checking needs those parameters must fix them: an insert, passing the binding to a typed parameter, a typed return, or assigning it to an annotated place. Reads whose checking needs no open parameter, such as `len(m)` or `m.is_empty()`, leave the binding available for a later fixing use. A discarded result or a binding with no type-determining use is refused here, naming the binding to annotate. A first use that needs the parameters but fixes nothing is refused with `INCAN-T0122`.",
    examples: &["from rust::std::collections import HashMap\n\ndef main() -> None:\n    mut untyped = HashMap.new()"],
    common_causes: &["A collection constructed and then never used."],
    fixes: &[
        "Write the type arguments in the call: `HashMap.new[str, int]()`.",
        "Annotate the binding: `untyped: HashMap[str, int] = HashMap.new()`.",
    ],
    docs_url: Some("https://encero-systems.github.io/incan/language/how-to/rust_interop/"),
};

pub(super) const ROUTE_HANDLER_RETURN_NOT_RESPONSE: DiagnosticCatalogEntry = DiagnosticCatalogEntry {
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

pub(super) const ROUTE_HANDLER_PARAMETER_UNBOUND: DiagnosticCatalogEntry = DiagnosticCatalogEntry {
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

pub(super) const OPERATOR_HAS_NO_TYPE_PARAMETER_BOUND: DiagnosticCatalogEntry = DiagnosticCatalogEntry {
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

pub(super) const METHOD_DECORATOR_RECEIVER_SPELLING: DiagnosticCatalogEntry = DiagnosticCatalogEntry {
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

pub(super) const METHOD_DECORATOR_RECEIVER_NOT_PLANNED: DiagnosticCatalogEntry = DiagnosticCatalogEntry {
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

pub(super) const RESERVED_COMPILER_NAME: DiagnosticCatalogEntry = DiagnosticCatalogEntry {
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

pub(super) const ROUTE_PAYLOAD_WITHOUT_JSON_FORM: DiagnosticCatalogEntry = DiagnosticCatalogEntry {
    code: "INCAN-T0112",
    title: "Route payload type has no JSON form",
    severity: "error",
    phase: "typecheck",
    summary: "A `@route` handler's `Json[T]`, `Query[T]` or `Path[T]` payload type does not derive `json`, or holds `decimal` or a frozen type, which have no JSON form.",
    explanation: "The route wrappers convert their payload at the handler boundary through serde: a `Json[T]` parameter deserializes the request body as JSON, a `Query[T]` parameter the URL-encoded query string, and a `Path[T]` parameter the path segments; a `Json[T]` return serializes `T` as the JSON response body. A model or class gets that serde support from `@derive(json)` (or by adopting the needed `std.serde.json` trait), and so does every model or class the payload is built from, such as `Search` in `Json[list[Search]]`. A payload declared without it checked before but could not be built: the route registration needed the conversion and had none. `decimal` and the frozen types (`FrozenStr`, `FrozenBytes`, `FrozenList`, `FrozenSet`, `FrozenDict`) have no JSON form at all, so a payload that holds one is refused too. The check refuses only a model or class whose declaration certainly has no JSON form, and those builtin types; any other payload is left to the build.",
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

pub(super) const FIELD_LACKS_AUTOMATIC_DERIVES: DiagnosticCatalogEntry = DiagnosticCatalogEntry {
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

pub(super) const HASHED_MEMBER_LACKS_EQ_HASH: DiagnosticCatalogEntry = DiagnosticCatalogEntry {
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

pub(super) const ARGUMENT_IS_NOT_A_TASK: DiagnosticCatalogEntry = DiagnosticCatalogEntry {
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

pub(super) const IMMUTABLE_ARGUMENT_TO_MUT_PARAMETER: DiagnosticCatalogEntry = DiagnosticCatalogEntry {
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

pub(super) const KEPT_DICT_LOOKUP_VALUE_CANNOT_BE_COPIED: DiagnosticCatalogEntry = DiagnosticCatalogEntry {
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

pub(super) const TAKEN_LIST_USED_AGAIN: DiagnosticCatalogEntry = DiagnosticCatalogEntry {
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

pub(super) const IMPORT: DiagnosticCatalogEntry = DiagnosticCatalogEntry {
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

pub(super) const SDK_COMPONENT_DISABLED: DiagnosticCatalogEntry = DiagnosticCatalogEntry {
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

pub(super) const SDK_COMPONENT_UNAVAILABLE: DiagnosticCatalogEntry = DiagnosticCatalogEntry {
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

pub(super) const PACKAGE_FEATURE_DISABLED: DiagnosticCatalogEntry = DiagnosticCatalogEntry {
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

pub(super) const IMPORT_LINK_LOOP: DiagnosticCatalogEntry = DiagnosticCatalogEntry {
    code: "INCAN-I0104",
    title: "Imported name link loop",
    severity: "error",
    phase: "import",
    summary: "Following an imported or re-exported name returns to a link already on the same lookup walk.",
    explanation: "Imports and re-exports are links to declarations. A lookup must reach a declaration before it can inspect the name or its members; a loop of links has no declaration to inspect, so the compiler reports every link in traversal order.",
    examples: &["# a.incn\npub from b import X\n\n# b.incn\npub from a import X"],
    common_causes: &[
        "Two facade modules re-export the same name from each other.",
        "A longer re-export chain returns to an earlier module without declaring the name anywhere.",
    ],
    fixes: &[
        "Make one module in the loop declare the value or type instead of re-exporting it.",
        "Point one re-export at the module that owns the declaration.",
    ],
    docs_url: Some("https://encero-systems.github.io/incan/language/reference/imports_and_modules/"),
};

pub(super) const TOOLING: DiagnosticCatalogEntry = DiagnosticCatalogEntry {
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

pub(super) const UNKNOWN: DiagnosticCatalogEntry = DiagnosticCatalogEntry {
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

pub(super) const OWNERSHIP_BOUNDARY_UNPROVABLE: DiagnosticCatalogEntry = DiagnosticCatalogEntry {
    code: "INCAN-T0123",
    title: "Ownership boundary cannot be proved",
    severity: "error",
    phase: "typecheck",
    summary: "An explicit ownership obligation at this boundary cannot be expressed or proved, so the program is refused instead of copying, retaining or weakening the access.",
    explanation: "Borrowed, BorrowedMut, Owned and Reference are canonical prelude contracts, and an explicit mode is an obligation for both the caller and the implementation. A stored field cannot declare an ownership mode, an annotation is not a constructor, a borrow cannot outlive its source, and a callable annotation or native lifetime relationship must be able to represent each dependency it carries. A user declaration shadowing a mode name remains an ordinary declaration.",
    examples: &[
        "model Page:\n    title: Borrowed[str]",
        "def main() -> None:\n    value = Owned(\"text\")",
    ],
    common_causes: &[
        "Declaring a stored field with an ownership mode.",
        "Returning or storing a closure, task or view that keeps a borrow after its source ends.",
        "Calling a native function whose result borrows from more than one input.",
    ],
    fixes: &[
        "Store an owned value, or expose the field through a method whose result names its source with `from`.",
        "End the borrow before its source ends, or transfer an owned value explicitly with `Owned[T]` or a clone.",
    ],
    docs_url: Some("https://encero-systems.github.io/incan/language/reference/explicit_ownership/"),
};

pub(super) const RESULT_SOURCE_INVALID: DiagnosticCatalogEntry = DiagnosticCatalogEntry {
    code: "INCAN-T0124",
    title: "Borrowed result source is missing or invalid",
    severity: "error",
    phase: "typecheck",
    summary: "An explicit borrowed result must name its source parameter or receiver.",
    explanation: "The from clause names one parameter or receiver followed by static field projections. It is required even with a single possible input; a global, local, call or index cannot stand in for that declaration relationship.",
    examples: &["def view(value: Borrowed[str]) -> Borrowed[str] from value:\n    return value"],
    common_causes: &["Omitting the source or naming a binding outside the callable signature."],
    fixes: &["Add from followed by the source parameter or receiver and any static field path."],
    docs_url: None,
};

pub(super) const PLACE_BINDING_MIGRATION: DiagnosticCatalogEntry = DiagnosticCatalogEntry {
    code: "INCAN-T0125",
    title: "Local binding copies a place that a place binding borrows",
    severity: "warning",
    phase: "typecheck",
    summary: "This local receives an independent value from a place that a read-only or writable place binding borrows.",
    explanation: "Under place bindings, bare and let declarations from a place borrow it read-only and mut declarations borrow it with write-through replacement. This declaration receives an independent value, and the warning does not change runtime behavior. Type annotations do not request copies; an explicit clone states an independent value.",
    examples: &["mut title: str = page.title", "let saved = page.title"],
    common_causes: &["Creating a local from an existing local, parameter, stored field or collection element."],
    fixes: &[
        "Use an explicit clone for an independent snapshot, or preserve the source through the borrow's last use.",
    ],
    docs_url: None,
};

pub(super) const EXPLICIT_OWNERSHIP_CONFLICT: DiagnosticCatalogEntry = DiagnosticCatalogEntry {
    code: "INCAN-T0126",
    title: "Explicit ownership access conflicts with a source dependency",
    severity: "error",
    phase: "typecheck",
    summary: "An operation lacks the requested permission, overlaps a live borrow, or uses unavailable storage.",
    explanation: "Checked control flow retains borrow origins and ownership transfers across branches and loops. The diagnostic identifies the conflicting operation, its source grant or earlier move, and a reachable dependent use when one keeps a loan live. This source check does not itself enable executable ownership annotations.",
    examples: &[
        "def update(mut value: Owned[int]) -> int:\n    let view: Borrowed[int] = value\n    value = 2\n    return view",
    ],
    common_causes: &[
        "Replacing or moving a source while a borrow remains live.",
        "Writing through a shared or read-only input.",
        "Using a moved value on one branch or a later loop iteration.",
    ],
    fixes: &[
        "Finish dependent uses before accessing overlapping storage.",
        "Request the access permission the operation needs at its declaration boundary.",
        "Reinitialize a moved place before using it again.",
    ],
    docs_url: None,
};

pub(super) const STORE_RESERVED_MODEL_FIELD: DiagnosticCatalogEntry = DiagnosticCatalogEntry {
    code: "INCAN-T0127",
    title: "Model field uses the store-reserved prefix",
    severity: "error",
    phase: "typecheck",
    summary: "A model field starts with `___`, which typed record stores reserve for record facts.",
    explanation: "RFC 165 reserves every model-field spelling that starts with three underscores. The rule applies to every model, whether or not it is currently stored, so authored data can never collide with a store-owned fact during inspection or export. The closed store-owned set is `___id`, `___revision`, `___model_hash`, `___canonical_hash`, and `___object_hash`; any other imported `___` fact is refused rather than treated as an unknown field.",
    examples: &["model Page:\n    ___id: int"],
    common_causes: &["A field was named to look private or to mirror a store inspection record."],
    fixes: &["Rename the model field so it does not start with three underscores."],
    docs_url: None,
};

pub(super) const RECORD_MEMBER_IDENTITY_COLLISION: DiagnosticCatalogEntry = DiagnosticCatalogEntry {
    code: "INCAN-T0128",
    title: "Record member identities collide",
    severity: "error",
    phase: "typecheck",
    summary: "Two fields in one model, or two variants in one enum, have the same compiler-derived record identity.",
    explanation: "RFC 165 derives a field or enum variant identity from the configured leading bytes of the SHA-256 digest of its earliest UTF-8 name. The identity needs to be unique only inside its model or enum. When two members collide, the diagnostic labels both declarations; authors do not assign an identity directly.",
    examples: &["model CollidingFields:\n    fieldbbea15ef9eff: int\n    field3d4624c2d3a2: int"],
    common_causes: &["Two distinct member names happen to share the same shortened SHA-256 identity."],
    fixes: &["Rename the newer field or variant so the compiler derives a different identity."],
    docs_url: None,
};

pub(super) const CATALOG: &[DiagnosticCatalogEntry] = &[
    OWNERSHIP_BOUNDARY_UNPROVABLE,
    RESULT_SOURCE_INVALID,
    PLACE_BINDING_MIGRATION,
    EXPLICIT_OWNERSHIP_CONFLICT,
    STORE_RESERVED_MODEL_FIELD,
    RECORD_MEMBER_IDENTITY_COLLISION,
    PARSER_SYNTAX,
    TYPECHECK,
    UNREACHABLE_CODE,
    SELF_MUTATION_REQUIRES_MUT_SELF,
    VALUE_WITHOUT_PRINTED_FORM,
    COLLECTION_ANNOTATION_REQUIRES_TYPE_ARGUMENTS,
    RUST_OWNER_TYPE_ARGS_NOT_INFERRED,
    RUST_BORROW_WITHOUT_LIFETIME_CONTRACT,
    RUST_CALLEE_NOT_SELECTED,
    ITERATED_PLACE_REPLACEMENT,
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
    IMPORT_LINK_LOOP,
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
