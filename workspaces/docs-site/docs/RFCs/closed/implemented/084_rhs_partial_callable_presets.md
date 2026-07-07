# RFC 084: RHS partial callable presets

- **Status:** Implemented
- **Created:** 2026-04-29
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 017 (validated newtypes with implicit coercion)
    - RFC 021 (model field metadata and aliases)
    - RFC 035 (first-class named function references)
    - RFC 036 (user-defined decorators)
    - RFC 038 (variadic positional args and keyword capture)
    - RFC 054 (explicit call-site generics)
    - RFC 083 (symbol and method aliases)
- **Issue:** [#453](https://github.com/encero-systems/incan/issues/453)
- **RFC PR:** —
- **Written against:** v0.3
- **Shipped in:** v0.3

## Summary

This RFC introduces RHS-oriented partial callable presets with the syntax `Name = partial Target(...)`. A partial callable preset creates a named or local callable that invokes an existing callable with some named arguments pre-filled, while leaving the remaining callable surface available to the caller. At module level, `pub BronzeReader = partial TableReader(layer="bronze", format="delta")` is a declaration, not arbitrary top-level execution. Inside functions, `partial Target(...)` is an ordinary expression that creates a callable value under normal runtime evaluation rules. This gives Incan a single syntax family for aliases, newtypes, and callable presets: `Name = alias Target`, `type Name = newtype Target`, and `Name = partial Target(...)`.

## Core model

1. **`partial` is a RHS marker:** the explicit syntax is `Name = partial Target(...)`, not `partial Name = Target(...)`.
2. **Partials are callable presets, not aliases:** an alias preserves symbol identity; a partial creates a derived callable with a projected signature.
3. **Top-level partials are declarations:** at module level, `Name = partial Target(...)` is accepted only as a restricted declaration form with a statically checkable target and statically safe preset values.
4. **Local partials are expressions:** inside executable code, `partial Target(...)` evaluates under normal expression rules and may capture runtime values.
5. **Constructor presets are first-class:** model, class, and newtype constructors can be partially applied just like functions when their constructor surface is statically known.
6. **Keyword presets act like overridable defaults:** a keyword argument supplied by a partial can be overridden by a later keyword argument at the call site, matching configuration-specialization use cases.
7. **Receiver binding is preserved for same-type method partials:** `set_alive = partial set_state(state=true)` inside a type body creates another method on the same receiver surface.
8. **No top-level execution loophole:** `Name = Target(...)`, `Name = partial make_target()(...)`, closures, comprehensions, and arbitrary expressions remain invalid at module top level unless another RFC admits them.

## Motivation

Aliases solve the case where two names mean the same symbol. They do not solve the case where a library author wants to publish a specialized callable. `mean = alias avg` is symbol identity. `BronzeReader = partial TableReader(layer="bronze")` is a new callable preset that still constructs `TableReader`.

The strongest motivating case is configuration specialization. Data, web, CLI, and workflow libraries often define a general callable with a broad constructor surface, then want named presets for common configurations:

```incan
model TableReader:
    layer: str
    format: str
    path: str

pub BronzeReader = partial TableReader(layer="bronze", format="delta")
pub SilverReader = partial TableReader(layer="silver", format="delta")
```

Users get stable names for common variants without creating subclasses, wrapper functions, or duplicate model declarations:

```incan
bronze = BronzeReader(path="tables/orders")
adhoc = BronzeReader(path="tables/orders", format="csv")
```

This is the same practical niche as Koheesio's `BaseModel.partial`, which documents partials as changing defaults "without needing to create another class" and says "newly provided defaults can always be overridden" ([Koheesio models documentation](https://engineering.nike.com/koheesio/0.10.0/api_reference/models/index.html#koheesio.models.BaseModel.partial)). The point is not just currying; it is reusable, named configuration.

Function partials matter too. Python documents `functools.partial` as freezing "some portion of a function's arguments and/or keywords" into a new callable ([Python functools documentation](https://docs.python.org/3/library/functools.html#functools.partial)). Incan should have a language-level version that participates in signatures, docs, manifests, and compiler diagnostics instead of requiring ad hoc wrapper functions.

RFC 036 also identifies partial application as a missing piece for decorator ergonomics. Decorator factories often require an extra level of nesting because configuration is supplied before the decorated function exists. Callable presets do not replace decorators, but they provide the same underlying tool: derive a callable by fixing part of another callable's parameter surface.

## Goals

- Add RHS-oriented partial syntax: `Name = partial Target(...)`.
- Allow public top-level partial declarations with `pub Name = partial Target(...)`.
- Allow local partial expressions inside executable code.
- Allow model, class, newtype, and function constructor/callable presets when the target callable surface is statically known.
- Allow same-type method partial declarations inside concrete method-bearing type bodies and traits.
- Support keyword presets that remain overrideable by later keyword arguments at call sites.
- Limit partial presets in this RFC to named arguments only.
- Preserve the target callable's return type, async calling convention, relevant generic constraints, default metadata, and documentation provenance where applicable.
- Make partial-provided defaults display no differently from ordinary callable defaults in hover, completion details, signature help, checked API metadata, and generated docs; defaulted parameters must be visually distinct in a consistent way across normal defaults and partial-projected defaults.
- Preserve partial metadata in library manifests, checked API metadata, generated docs, and editor tooling.
- Keep module top level declaration-only by restricting top-level partial targets and preset values while still admitting declaration-safe collection and model literals.

## Non-Goals

- General top-level assignment or top-level expression execution.
- Treating partials as aliases or preserving symbol identity with the target.
- Creating new nominal types, subclasses, or model declarations.
- Allowing `Name = Target(...)` without the `partial` marker at module top level.
- Allowing arbitrary top-level preset expressions such as function calls, closures, comprehensions, object construction, or I/O.
- Top-level partials over unbound methods such as `Name = partial Type.method(value=...)`.
- Partial application of overload sets or multi-target dispatch.
- Positional partial application.
- Partial application of callables with unconstrained `*args`, `**kwargs`, or keyword-capture surfaces in this RFC.
- Defining decorator semantics. This RFC only defines callable presets that decorators and decorator utilities may later use.

## Guide-level explanation

A partial callable preset is written on the right-hand side:

```incan
def route(method: str, path: str, handler: Handler) -> Route:
    return Route(method=method, path=path, handler=handler)

pub get = partial route(method="GET")
pub post = partial route(method="POST")
```

The resulting names are callable:

```incan
users = get(path="/users", handler=list_users)
create_user_route = post(path="/users", handler=create_user)
```

The target callable still owns behavior. A partial only supplies preset arguments. It is roughly equivalent to a wrapper, but the language and tooling remember that the wrapper is a partial:

```incan
pub def get(path: str, handler: Handler) -> Route:
    return route(method="GET", path=path, handler=handler)
```

Constructor presets use the same syntax:

```incan
model TableReader:
    layer: str
    format: str
    path: str

pub BronzeReader = partial TableReader(layer="bronze", format="delta")

def main() -> None:
    reader = BronzeReader(path="warehouse/orders")
```

Preset keyword arguments can be overridden by name:

```incan
reader = BronzeReader(path="warehouse/orders", format="csv")
```

Inside a function, `partial` is a local expression and may capture runtime values:

```incan
def reader_for(layer: str) -> (str) -> TableReader:
    return partial TableReader(layer=layer, format="delta")
```

Inside a type body, a method partial preserves receiver binding:

```incan
class Cell:
    alive: bool

    def set_state(mut self, state: bool) -> None:
        self.alive = state

    set_alive = partial set_state(state=true)
    set_dead = partial set_state(state=false)
```

Calling the method partial uses the same receiver surface as the original method:

```incan
def reset(cell: Cell) -> None:
    cell.set_dead()
```

## Reference-level explanation

### Syntax

The top-level partial declaration form is:

```text
TopLevelPartialDecl ::= Visibility? Ident "=" PartialExpr
Visibility          ::= "pub"
PartialExpr         ::= "partial" PartialCallTemplate
PartialCallTemplate ::= PartialTarget "(" PartialArgList? ")"
PartialTarget       ::= QualifiedName
QualifiedName       ::= Ident (("." | "::") Ident)*
```

Inside executable expressions, `partial` introduces a partial expression:

```text
PartialExpr         ::= "partial" CallableExpr "(" PartialArgList? ")"
```

Inside method-bearing type bodies, the method partial declaration form is:

```text
MethodPartialDecl ::= Ident "=" "partial" Ident "(" PartialKeywordArgList? ")"
```

The `partial` token is a contextual RHS marker. At module level it is only valid immediately after `=` in a syntactically valid partial declaration. Inside executable code it is an expression introducer. Inside type bodies it is only valid immediately after `=` in a same-type method partial declaration unless the type body also admits ordinary executable statements in a future RFC.

A partial argument list in this RFC is keyword-only:

```text
PartialArgList        ::= PartialKeywordArgList
PartialKeywordArgList ::= PartialKeywordArg ("," PartialKeywordArg)* ","?
PartialKeywordArg     ::= Ident "=" Expr
```

At module top level, `Expr` in a partial argument must be a statically safe preset expression as defined by this RFC. Inside executable code, `Expr` follows normal expression rules.

### Supported top-level targets

A top-level partial target must resolve to one of:

- a top-level function declaration;
- a method-independent imported callable symbol;
- a model, class, or newtype constructor surface;
- another public or private partial declaration whose projected callable surface is known;
- an alias from RFC 083 that resolves acyclically to one of the supported target kinds above.

This RFC does not support top-level partials over consts, statics, module namespaces, enum variants as standalone values, arbitrary call expressions, local variables, closures, fields, or unbound methods.

### Supported method targets

A method partial target must resolve to a method on the same declaring type surface. The target spelling must be an unqualified method name.

For example:

```incan
class Cell:
    def set_state(mut self, state: bool) -> None:
        ...

    set_alive = partial set_state(state=true)
```

The method partial `set_alive` targets `Cell.set_state`, keeps the same receiver kind, and presets the `state` parameter by name.

Method partials are valid in concrete method-bearing type bodies: classes, models, and newtypes. Method partials are also valid in traits. A trait method partial defines a default method on the trait surface whose body is equivalent to forwarding to another same-trait method with the preset keyword arguments applied. The generated default method typechecks against the trait's `Self` surface, must obey the same `@requires(...)`, supertrait, override, collision, and ambiguity rules as ordinary trait default methods, and must lower through the existing trait-default-method expansion path for each concrete adopter.

### Preset mapping

The compiler maps a partial call template onto the target callable's parameter list.

Each preset fills a target parameter by name. Parameters not filled by a preset remain parameters of the generated callable. Required target parameters remain required. Optional target parameters remain optional. Keyword-preset parameters become optional override parameters on the generated callable when the target supports named calls. Tooling and documentation must display these parameters the same way they display ordinary defaulted parameters. A partial-provided default is not a special display category; it is a defaulted callable parameter with partial provenance attached elsewhere.

For example:

```incan
def route(method: str, path: str, handler: Handler) -> Route:
    ...

pub get = partial route(method="GET")
```

The generated callable requires `path` and `handler`, and it may accept `method` by keyword to override the preset default.

A partial call template must contain at least one preset value. A template containing no preset keywords is a no-op and must be rejected with a diagnostic suggesting an alias or direct callable reference.

### Top-level preset expressions

At module top level, preset expressions must be statically safe. The statically safe set defined by this RFC is:

- scalar literals;
- string literals;
- collection literals whose elements, keys, and values are recursively statically safe preset expressions;
- model literals whose model type is statically known and whose field values are recursively statically safe preset expressions;
- enum variant paths when the variant is a compile-time value and requires no construction arguments;
- const identifiers or qualified const paths whose values are themselves accepted as statically safe presets;
- aliases to statically safe consts, enum variants, collection literals, or model literals.

Top-level preset expressions must not include function calls, constructor calls, closures, comprehensions, local bindings, field access on runtime values, mutation, I/O, async operations, or any expression that requires module initialization order. A declaration-safe model literal is not a constructor call for this rule: it is accepted only when it can be checked and serialized as compile-time preset metadata without running user code, defaults, validation hooks, or conversion logic.

Inside executable code, preset expressions evaluate left-to-right under normal expression rules when the partial expression is evaluated.

### Type checking

The type of a partial callable is the projected callable type produced by removing or defaulting the preset target parameters and preserving all unfilled parameters.

The return type is the target callable's return type. If the target callable is async, the partial callable has the same async calling convention. If the target callable has default parameters, rest metadata, keyword metadata, or named-argument metadata that remains visible through the projection, the partial callable must preserve that metadata.

Keyword presets must typecheck against the corresponding target parameter type.

Duplicate fills are rejected. This includes repeating the same keyword parameter within one partial template.

Generic callable partials are allowed. The resulting partial remains generic over any target type parameters that remain free after applying the preset arguments. If the target supports explicit call-site generics, the projected partial surface must support the same call-site generic behavior where it remains meaningful after projection.

### Calls through partials

Calling a partial callable typechecks as if the compiler expanded the call into the target callable with preset arguments merged with call-site arguments.

Call-site keyword arguments override keyword presets. Other non-preset parameters remain callable according to the target's projected parameter surface.

For example:

```incan
pub BronzeReader = partial TableReader(layer="bronze", format="delta")

reader = BronzeReader(path="orders")
csv_reader = BronzeReader(path="orders", format="csv")
```

The first call expands to `TableReader(layer="bronze", format="delta", path="orders")`. The second expands to `TableReader(layer="bronze", format="csv", path="orders")`.

### Name resolution and cycles

A top-level partial declaration introduces a symbol named by the declaration identifier in the module namespace.

A method partial declaration introduces a member named by the declaration identifier on the owning type's method surface.

Partial declarations participate in declaration collection. A top-level partial may refer to a supported symbol declared later in the same module, provided the final partial and alias graph is acyclic and all target surfaces are known before type checking use sites.

Partial cycles are rejected. This includes direct cycles such as `a = partial a(value)` and indirect cycles through aliases or other partials.

### Visibility and imports

Private top-level partials are visible inside their declaring module according to normal module scope rules.

Public top-level partials are exported callable symbols. A public partial must not depend on private targets or private preset values in a way that prevents consumers from typechecking or calling the exported symbol.

Library manifests and checked API metadata must preserve public partials as partials, not flatten them into unrelated function declarations.

### Runtime behavior and emission

At language level, a partial behaves like a generated callable wrapper with preserved partial metadata. The generated callable must call the target exactly once per invocation after merging preset arguments with call-site arguments.

Backends may emit an actual wrapper function, closure, function object, or backend-native partial representation. The chosen representation must preserve observable call behavior, projected type information, public symbol availability, diagnostics, and metadata.

Top-level partial declarations must not run target call expressions during module initialization. They describe how to call the target later.

### Diagnostics

The compiler must emit targeted diagnostics for at least:

- unresolved partial target;
- target kind is not supported for partials;
- top-level preset expression is not statically safe;
- partial template contains no preset values;
- duplicate fill for the same target parameter;
- keyword argument names an unknown target parameter;
- method partial target is not a same-type method;
- partial name collides with an existing declaration or member;
- partial cycle;
- public partial depends on a private or non-exportable target;
- positional partial application is not supported by this RFC.

Diagnostics should mention the partial name, the target spelling, and the argument or parameter that failed projection where applicable.

## Design details

### Why RHS syntax

RHS syntax matches the declaration family already used by aliases and newtypes:

```incan
Name = alias Target
type UserId = newtype int
Name = partial Target(...)
```

The left side introduces the name. The right side says what kind of derived declaration or value is being created. Prefix syntax such as `partial Name = Target(...)` would work mechanically, but it makes partials look like a new declaration head rather than a derived callable expression.

### Relationship to aliases

Aliases preserve identity:

```incan
Mean = alias Avg
```

Partials create callable presets:

```incan
BronzeReader = partial TableReader(layer="bronze")
```

The two can compose:

```incan
DefaultBronzeReader = alias BronzeReader
```

This RFC extends RFC 083 by allowing aliases to target partial declarations whose projected callable surface is known. But a partial must not be treated as an alias to its target. It has a different callable surface.

### Relationship to newtypes

The syntax is intentionally similar to `type UserId = newtype int`, but the semantics are different. A newtype creates a new nominal type. A partial creates a callable value or callable declaration that returns whatever the target callable returns.

For example:

```incan
type UserId = newtype int
BronzeReader = partial TableReader(layer="bronze")
```

`UserId` is a distinct type. `BronzeReader` is not a distinct reader type; it is a callable preset that constructs `TableReader`.

### Relationship to wrappers

Authors can always write a wrapper manually:

```incan
def BronzeReader(path: str) -> TableReader:
    return TableReader(layer="bronze", format="delta", path=path)
```

That remains useful when the author wants custom validation, logging, coercion, side effects, or documentation. A partial says there is no new behavior beyond preset argument binding.

### Relationship to decorators

RFC 036 notes that decorator factories can become verbose when configuration must be supplied before the decorated function exists. Partials do not define decorator application, but they provide a primitive that decorator libraries can use to publish preconfigured decorators or reduce wrapper boilerplate.

For example:

```incan
retry_network = partial retry(attempts=3, on=NetworkError)
```

Whether that partial is later usable directly as a decorator depends on RFC 036's decorator callable rules, not on this RFC.

### Relationship to imports and API documentation

An imported partial is a callable symbol with partial provenance:

```incan
from readers import BronzeReader
```

Generated API documentation should list the projected callable signature and show the target/preset relationship. Documentation must not present a public partial as a hand-written independent function unless the source declaration actually is a hand-written wrapper.

### Compatibility / migration

This RFC is additive. Existing valid programs continue to parse and typecheck the same way.

Pure forwarding functions can migrate to partial declarations when they only call another callable with fixed arguments and do not add behavior.

## Alternatives considered

1. **Declaration-head syntax: `partial BronzeReader = TableReader(...)`**
   - Rejected because it is less consistent with RHS-marked derived forms such as `Name = alias Target` and `type Name = newtype Target`.

2. **Function-call syntax: `BronzeReader = partial(TableReader, layer="bronze")`**
   - Rejected for the language surface because it looks like ordinary top-level function execution. A stdlib helper may still exist later for dynamic use, but the declaration form should remain compiler-recognized.

3. **Require the target kind on the left: `class BronzeReader = partial TableReader(...)` or `def get = partial route(...)`**
   - Rejected because the resulting symbol is a callable preset, not necessarily a class or a function declaration. The target's semantic kind should be inferred from the target surface.

4. **Allow only explicit wrapper functions**
   - Rejected because it repeats signatures and bodies, hides preset intent from tooling, and creates documentation drift for a common configuration-specialization pattern.

5. **Make partials aliases with arguments**
   - Rejected because aliases preserve symbol identity while partials change callable signatures and call behavior.

6. **Allow arbitrary top-level partial expressions**
   - Rejected because it would reopen module initialization order, side effects, and executable module statements. Top-level partials must be declaration-safe.

7. **Support positional placeholders in this RFC**
   - Rejected because the strongest motivating use cases are keyword-shaped configuration presets. Positional partial application would introduce placeholder syntax, parameter-order projection rules, and mixed positional/keyword edge cases that this RFC intentionally avoids.

## Drawbacks

Partials add another callable-producing construct to the language. Users must learn the difference between aliases, wrappers, and partials.

Projected signatures are not trivial, especially when keyword presets remain overrideable. Tooling must explain the generated callable surface clearly or users will be surprised by which arguments remain available.

Top-level partials require a restricted expression subset for preset values. That restriction is necessary to preserve declaration-only module semantics, but it creates another place where an expression valid locally may be invalid at module top level.

Backend implementations may need to generate wrapper symbols even though the source does not contain a function body. That is acceptable, but metadata must continue to represent the declaration as a partial rather than a hand-written function.

## Implementation architecture (non-normative)

An implementation should represent partials as declaration-level and expression-level callable projections. The frontend should preserve the target path, preset argument list, projected signature, and provenance rather than lowering immediately to an opaque wrapper.

Top-level partial validation should run after declaration collection so forward references and aliases can resolve before the compiler projects the callable surface.

Local partial expressions can lower similarly to closures, but they should retain enough metadata for diagnostics and tooling to explain the target/preset relationship when possible.

Manifest and checked API metadata should grow explicit partial entries rather than duplicating function declarations or pretending the partial is an alias.

## Layers affected

- **Parser / AST**: must recognize `partial` as a RHS marker for top-level partial declarations, method partial declarations, and local partial expressions.
- **Typechecker / Symbol resolution**: must resolve partial targets, project callable signatures, typecheck preset expressions, reject unsupported targets, and detect cycles across aliases and partials.
- **IR Lowering**: must preserve enough partial metadata for emitted wrappers or callable values while allowing private use sites to call the resolved target through projected arguments.
- **Emission**: must emit callable wrappers, closures, or backend-native partials that preserve observable behavior and public symbol availability.
- **Library manifests**: must export public partials with target provenance, preset metadata, projected callable signatures, and visibility.
- **Checked API metadata**: must represent partials distinctly from aliases and hand-written functions.
- **Formatter**: must format `Name = partial Target(...)` and local `partial Target(...)` expressions deterministically.
- **LSP / Tooling**: should surface hover, completion, go-to-definition, signature help, rename behavior, and diagnostics using both the partial name and canonical target where useful.
- **Docs**: should document public partials as callable presets with projected signatures and target/preset provenance.

## Implementation Plan

### Phase 1: Syntax, AST, and formatting

- Add parser and AST support for module-level partial declarations, same-type method partial declarations, trait method partial declarations, and local partial expressions.
- Preserve target path, preset keyword arguments, visibility, spans, and partial provenance in the AST.
- Extend formatter support for `Name = partial Target(...)` and local `partial Target(...)`, including stable formatting for collection and model literal preset values.

### Phase 2: Typechecker and projected callable surfaces

- Resolve partial targets across functions, constructors, aliases, partials, methods, trait methods, imports, and forward declarations.
- Project callable signatures so preset keyword arguments become ordinary defaulted override parameters while unfilled required parameters stay required.
- Validate statically safe top-level preset expressions, including recursively safe collection and model literals.
- Detect duplicate fills, unknown preset keywords, unsupported targets, empty templates, public/private export leaks, cycles, rest/keyword-capture exclusions, trait partial collisions, and ambiguous trait partial inheritance.

### Phase 3: Lowering and emission

- Lower top-level partial declarations and method partials into callable wrapper metadata without running target calls at module initialization.
- Lower local partial expressions into callable values with runtime capture semantics.
- Emit wrappers, closures, or backend-native callable values that call the target exactly once and merge preset and call-site keyword arguments correctly.
- Reuse existing trait-default-method expansion for trait method partials so adopter-specific `Self`, `@requires(...)`, and supertrait behavior remains consistent.

### Phase 4: Manifests, checked API metadata, and LSP/tooling

- Represent public partials distinctly in library manifests and checked API metadata, including target provenance, preset metadata, projected signatures, visibility, and docs provenance.
- Add or improve LSP signature help and hover/completion rendering so ordinary default parameters and partial-projected default parameters are visually distinct in the same consistent way.
- Support go-to-definition, rename, completion, and diagnostics for partial names, targets, and preset arguments where existing LSP architecture supports the behavior.

### Phase 5: Docs, tests, and release integration

- Add parser, formatter, typechecker, lowering, codegen, trait-default, manifest, checked API, LSP, and integration tests for the accepted surface.
- Update authored language docs for callable presets and default-display behavior.
- Add release notes and bump the active development version.

## Progress Checklist

### Spec / design

- [x] Resolve projected default display: partial-projected defaults use the same visual model as ordinary defaults.
- [x] Resolve top-level preset coverage: declaration-safe collection and model literals are in scope.
- [x] Resolve trait method partials: same-trait method partials are in scope and behave as generated trait default methods.

### Parser / AST / formatter

- [x] Parser: parse module-level partial declarations.
- [x] Parser: parse local partial expressions.
- [x] Parser: parse concrete method and trait method partial declarations.
- [x] AST: represent partial declarations, method partials, and partial expressions with spans and preset metadata.
- [x] Formatter: round-trip partial declarations and partial expressions.
- [x] Formatter: preserve stable formatting for declaration-safe collection and model literal preset values.

### Typechecker

- [x] Resolve supported top-level partial targets and aliases.
- [x] Resolve constructor partials for models, classes, and newtypes.
- [x] Resolve concrete method partials and trait method partials.
- [x] Project callable signatures with preset keywords as ordinary defaulted override parameters.
- [x] Validate declaration-safe top-level preset expressions, including collection and model literals.
- [x] Reject unsupported targets, empty templates, duplicate fills, unknown keywords, cycles, visibility leaks, positional partials, and rest/keyword-capture targets.
- [x] Diagnose trait partial collisions, override conflicts, and ambiguous inherited partials.

### Lowering / IR / emission

- [x] IR: preserve projected callable metadata for partial defaults and calls.
- [x] Lower top-level partial declarations without introducing top-level execution.
- [x] Lower local partial expressions with callable default metadata for runtime calls.
- [x] Emit callable wrappers or callable values that merge preset and call-site keyword arguments correctly.
- [x] Reuse trait-default-method expansion for trait method partials.

### Manifests / checked API / tooling

- [x] Library manifests: export public partials with target provenance, preset metadata, projected signatures, and visibility.
- [x] Checked API metadata: represent partials distinctly from aliases and hand-written functions.
- [x] LSP: improve ordinary default-argument display where needed.
- [x] LSP: add signature help coverage for callable signatures and defaulted parameters.
- [x] LSP: partial hover, completion, go-to-definition, document symbols, and diagnostics use the partial name, target, preset arguments, and projected default display where the current LSP architecture supports the behavior. Rename remains outside the current advertised LSP capability set.
- [x] Generated docs: render partials as callable presets with projected signatures and provenance.

### Tests

- [x] Parser and formatter tests for all partial syntactic forms.
- [x] Typechecker valid and invalid tests for target resolution, projection, diagnostics, and statically safe presets.
- [x] Trait method partial tests for default expansion, adopter behavior, collisions, overrides, and ambiguity.
- [x] Codegen snapshot tests for top-level partial declarations and local partial expressions.
- [x] Integration tests for callable behavior, keyword override behavior, constructor presets, trait partials, and manifest consumers.
- [x] LSP/unit tests for ordinary defaults and partial-projected default display.

### Docs / release

- [x] Update authored language reference/explanation docs for callable presets.
- [x] Update tooling docs if signature help/default display behavior changes.
- [x] Add release notes entry for RFC 084.
- [x] Bump the active `0.3.0-dev.N` version.

## Design Decisions

- Keyword presets remain overrideable by later keyword arguments at the call site. This matches the configuration-specialization use case where a partial supplies a named default rather than permanently freezing that parameter.
- Partial-projected defaults use the same display and signature model as ordinary callable defaults. The default source is partial metadata; the displayed callable surface should not invent a separate partial-only calling convention.
- Top-level statically safe preset expressions include declaration-safe collection and model literals, provided every nested value is recursively statically safe and no constructor/default/validation/runtime code executes during module initialization.
- Trait method partials are in scope. A same-trait method partial is a generated trait default method that forwards to another same-trait method with preset keyword arguments, subject to existing trait default, `@requires(...)`, supertrait, override, collision, and ambiguity rules.
