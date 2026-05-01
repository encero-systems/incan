# RFC 083: Symbol and method aliases

- **Status:** Planned
- **Created:** 2026-04-29
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 031 (library system phase 1)
    - RFC 035 (first-class named function references)
    - RFC 038 (variadic positional args and keyword capture)
    - RFC 048 (checked contract metadata)
    - RFC 054 (explicit call-site generics)
    - RFC 082 (checked API documentation generation)
    - RFC 084 (RHS partial callable presets)
- **Issue:** [#437](https://github.com/dannys-code-corner/incan/issues/437)
- **RFC PR:** —
- **Written against:** v0.3
- **Shipped in:** —

## Summary

This RFC adds alias declarations for existing top-level symbols and same-type methods. At module level, `Name = Target` and `Name = alias Target` are equivalent declaration forms for aliasing a supported existing symbol without executing arbitrary top-level code. Inside a type body, `method_alias = method_name` and `method_alias = alias method_name` define another method name on the same receiver surface. Aliases preserve identity for imports, diagnostics, documentation, and metadata, but they do not clone declarations, create forwarding wrappers, or introduce a general module-level assignment model.

## Core model

1. **Alias-shaped assignment is declaration syntax:** at module level and in type bodies, `Name = Target` is parsed as an alias declaration only when the right-hand side is a syntactic symbol path accepted by this RFC.
2. **The `alias` marker is optional and RHS-oriented:** `Name = alias Target` is the explicit spelling of the same declaration; it does not create a different kind of alias.
3. **Aliases point at existing symbols:** the target must resolve to a supported declaration symbol or another acyclic alias.
4. **Top-level aliases and method aliases are separate surfaces:** a top-level alias gives a module symbol another name; a method alias gives an existing method another name on the same owning type.
5. **Aliases preserve identity:** tools, manifests, documentation, diagnostics, imports, and refactoring should be able to distinguish the alias name from the canonical target name.
6. **Aliases are not wrappers:** an alias must not create a new function body, duplicate signature text, change receiver binding, or add runtime behavior.
7. **No arbitrary top-level execution:** literals, calls, closures, comprehensions, mutable globals, and other executable expressions remain invalid at module top level unless another feature explicitly admits them.

## Motivation

Incan already treats named functions as values inside function bodies. RFC 035 says a function name can be passed by name, stored in a variable, and called through that variable. However, the same ergonomic naming shape is not available at module level. A library author cannot currently write a real top-level alias such as `mean = avg`; the parser rejects it because top-level bare identifiers are not declarations.

Library and catalog-style APIs often need multiple public names for the same operation or type. A canonical name may be mathematically precise while an alias may be familiar to another audience. Types can have the same issue: one internal or canonical type name may need a shorter, domain-facing, or compatibility name without introducing a second runtime type.

Methods have the same problem. If a type naturally offers both `avg()` and `mean()`, the author should not need to write a second forwarding method that repeats the receiver, parameters, generic parameters, return type, docs, and body. The clean model is a method alias on the same receiver surface.

The end-state should be a language-level alias: users can import and call the alias as an ordinary name, while tools can still report that the alias points at a canonical symbol. That is different from a wrapper function or method, where the alias has a separate body, separate documentation drift risk, and potentially separate runtime behavior.

## Goals

- Add top-level alias declaration syntax with both bare and explicit forms: `Name = Target` and `Name = alias Target`.
- Add same-type method alias declarations inside method-bearing type bodies.
- Preserve first-class alias metadata for semantic analysis, library manifests, checked API metadata, documentation generation, and editor tooling.
- Allow top-level aliases to target supported callable and type-like symbols, including aliases that resolve acyclically to those symbols.
- Allow method aliases to target methods on the same declaring type surface.
- Preserve callable behavior for function and method aliases, including parameters, default metadata, rest metadata, return type, async calling convention, receiver kind, and explicit call-site generic behavior.
- Reject arbitrary top-level assignment, arbitrary expression aliases, unresolved targets, unsupported target kinds, duplicate alias names, alias cycles, and public aliases that expose non-public targets.
- Keep public behavior minimal: a public top-level alias may re-export an already exportable target, but this RFC does not introduce public facade aliases over private declarations.

## Non-Goals

- General module-level mutable or immutable assignment.
- Runtime-mutating module globals.
- LHS-oriented alias syntax such as `alias Name = Target` or `pub alias Name = Target`.
- Aliasing literals, calls, closures, comprehensions, field accesses, partial applications, bound method values, or arbitrary expressions. RFC 084 defines partial callable presets separately.
- Top-level aliases to unbound methods such as `Alias = Type.method`.
- Public facade aliases over private targets.
- Curated member projections such as `pub PublicClass = alias PrivateClass: pub method`.
- New method-level visibility syntax such as `pub def` inside type bodies.
- Trait-set aliases or compound aliases that combine multiple traits.
- Overload sets or multi-target aliases.
- Reopening generic function references in ordinary local value position. This RFC only allows generic callable aliasing through declaration-level symbol relationships.
- Replacing import `as` aliases, type aliases over type expressions, field metadata aliases, or public re-export syntax.

## Guide-level explanation

A top-level alias can be written in the concise form:

```incan
def avg(x: int) -> int:
    return x

mean = avg

def main() -> None:
    println(mean(10))
```

The explicit form is equivalent and can be used when the author wants the declaration kind to be visually obvious:

```incan
average = alias avg
```

Both aliases are real symbols. Users call `mean` and `average` the same way they call `avg`, while tools can still show that both names alias `avg`.

Aliases are not limited to functions. A type-like symbol can also have another top-level name:

```incan
enum ExampleEnum:
    Value
    ValueB

AnotherExample = ExampleEnum
YetAnotherExample = alias ExampleEnum
```

`AnotherExample` and `YetAnotherExample` are aliases for the same enum symbol. They do not create new enum types.

Public top-level aliases are allowed only as ordinary re-exports of already exportable targets:

```incan
pub def avg(expr: ScalarExpr[number]) -> AggregateMeasure[number]:
    return AVG.call(expr)

pub mean = avg
pub average = alias avg
```

A consumer can import the alias by name:

```incan
from stats import mean

def report(expr: ScalarExpr[number]) -> AggregateMeasure[number]:
    return mean(expr)
```

Method aliases live inside the owning type:

```incan
class Stats:
    value: int

    def avg(self) -> int:
        return self.value

    mean = avg
    average = alias avg
```

Both method aliases stay on the receiver surface:

```incan
def main(stats: Stats) -> int:
    return stats.mean() + stats.average()
```

A method alias does not define a free function. The following remains out of scope because it is an unbound method reference:

```incan
mean = Stats.avg  # rejected
```

Top-level assignment remains declaration-only and alias-shaped. The following still do not execute at module load time:

```incan
x = 1
y = make_value()
f = (x) => x + 1
```

Those forms are rejected with diagnostics that point to `const`, `static`, `def`, or an alias declaration depending on the shape.

## Reference-level explanation

### Syntax

The top-level alias declaration forms are:

```text
TopLevelAliasDecl ::= Visibility? Ident "=" TopLevelAliasRhs
Visibility        ::= "pub"
TopLevelAliasRhs  ::= AliasMarker? TopLevelAliasTarget
AliasMarker       ::= "alias"
TopLevelAliasTarget ::= QualifiedName
QualifiedName ::= Ident (("." | "::") Ident)*
```

The method alias declaration forms are:

```text
MethodAliasDecl ::= Ident "=" MethodAliasRhs
MethodAliasRhs  ::= AliasMarker? Ident
AliasMarker     ::= "alias"
```

At module level, `Name = Target` and `Name = alias Target` are equivalent. `pub Name = Target` and `pub Name = alias Target` are equivalent public alias declarations.

Inside a type body, `method_alias = method_name` and `method_alias = alias method_name` are equivalent method alias declarations. This RFC does not add `pub` before method aliases.

The `alias` token is a contextual RHS marker. It is only treated as an alias marker immediately after `=` in a syntactically valid top-level alias declaration or type-body method alias declaration.

These examples show where the alias grammar intentionally stops:

LHS-oriented aliases are rejected. The `alias` marker belongs on the right-hand side when the author wants the explicit form:

```incan
alias local_name = target        # rejected
pub alias public_name = target   # rejected
```

Alias declarations cannot introduce their own type annotation or type parameter list. They inherit the target symbol's callable or type-like shape:

```incan
local_name: Callable[int, int] = alias target  # rejected
local_name[T] = alias target                   # rejected
```

Alias right-hand sides must be symbol paths, not expressions that create new values or wrappers:

```incan
local_name = alias (x) => target(x)  # rejected
local_name = alias target(1)         # rejected
```

Method aliases are same-type member aliases only. They do not define unbound method references or public projection surfaces:

```incan
method_alias = alias OtherType.method  # rejected

pub PublicClass = alias PrivateClass:  # rejected
    pub example_method
```

### Supported top-level targets

A top-level alias target must resolve to one of:

- a top-level function declaration;
- a method-independent imported callable symbol;
- a model, class, enum, newtype, type alias, or trait symbol;
- another alias that resolves acyclically to one of the supported target kinds above.

This RFC does not support aliases to consts, statics, module namespaces, fields, enum variants as standalone values, Rust items that are not surfaced as supported Incan symbols, unbound methods, or arbitrary expressions.

If a future RFC adds a stable reason to alias consts, statics, enum variants, modules, or unbound methods, it must define that target kind explicitly rather than inheriting it from this RFC.

### Supported method targets

A method alias target must resolve to a method on the same declaring type surface.

For example:

```incan
class Stats:
    def avg(self) -> int:
        return 0

    mean = avg
```

The alias `mean` targets the `avg` method on `Stats`.

The target must not resolve through a qualified path, imported type, field, const, static, closure, call expression, local variable, or method on another type.

Method aliases are valid in method-bearing type bodies: classes, models, newtypes, and traits. A trait method alias is an alias for a trait method requirement or default method on the same trait surface; it does not create a compound trait alias.

### Name resolution

An alias declaration introduces a symbol named by the alias identifier in the relevant declaration scope.

Top-level aliases participate in declaration collection. A top-level alias may refer to a supported symbol declared later in the same module, provided the final alias graph is valid.

Method aliases participate in collection of the owning type body. A method alias may refer to a method declared later in the same type body, provided the final method-alias graph is valid.

An alias name must not duplicate or collide with any existing declaration in the same relevant namespace. For top-level aliases, that is the module root namespace. For method aliases, that is the owning type's member namespace.

Alias cycles are rejected. This includes direct cycles such as `a = a` and indirect cycles such as `a = b` plus `b = a`.

### Type checking

The semantic kind of an alias is the semantic kind of its resolved target.

A function alias has the callable type of the resolved function target. It preserves the target's parameters, keyword/default metadata, rest parameter metadata, return type, async calling convention, and type parameter contract.

A type-like alias resolves as the same type-like symbol as its target. It must not create a distinct nominal type, duplicate enum variants, duplicate constructors, or create a new trait identity.

A method alias has the method type of the resolved method target. It preserves receiver kind, decorators that affect call semantics, type parameters, parameters, keyword/default metadata, rest metadata, return type, async calling convention, and any method-level generic behavior.

Calling a function alias must typecheck as if the call targeted the resolved function directly, except diagnostics should use the alias name at the use site and may include the canonical target as secondary context.

Calling a method alias must typecheck as if the call targeted the resolved method directly with the same receiver binding. A method alias must not be callable as a top-level free function unless another declaration explicitly defines such a function.

If the target is generic, the alias is generic in exactly the same way as the target. Explicit call-site generics apply to the alias name:

```incan
def identity[T](value: T) -> T:
    return value

id = identity

def main() -> int:
    return id[int](1)
```

The alias declaration itself must not introduce new type parameters or rewrite the target's type parameter names. It is a symbolic alias, not a generic adapter.

### Visibility and imports

Private top-level aliases are visible inside their declaring module according to normal module scope rules.

Public top-level aliases are exported symbols. A consumer may import a public alias by the alias name and use it according to the target's semantic kind.

A public top-level alias must not expose a target that is private to the declaring module or otherwise unavailable to consumers. If a library wants to publish both the canonical name and one or more aliases, the canonical target should be public and each public alias should target that public symbol.

This RFC does not define public facade aliases over private targets. It also does not define alias blocks that curate or project selected members from a private type.

Method alias export behavior follows the owning type's existing method export model. This RFC does not add method-level `pub` syntax. If a public type exposes a method, and an alias to that method is part of the type's public metadata, documentation and tooling should preserve the alias relationship rather than present it as a second method body.

### Metadata

Library manifests and checked API metadata must preserve aliases as aliases, not flatten them into independent declarations.

Top-level alias metadata must include at least:

- alias name;
- visibility;
- target symbol kind;
- target module path or target symbol path where known;
- target symbol name;
- projected type or callable signature sufficient for import-time checking;
- target type parameter metadata where present;
- enough provenance for documentation and tooling to say that the alias points at a canonical symbol.

Method alias metadata must include at least:

- owning type;
- alias method name;
- target method name;
- projected callable signature sufficient for call-site checking and documentation;
- receiver kind;
- target type parameter metadata where present;
- enough provenance for documentation and tooling to say that the alias points at a canonical method.

Generated API documentation should list aliases as aliases, not as duplicate declarations. It may show the target signature for readability, but the documentation model must preserve the alias relationship.

### Runtime behavior and emission

Using an alias has the same observable result as using the target symbol.

Backends should emit an alias or re-export when the target language supports that representation. When no public symbol is required, a backend may lower use sites to the resolved target name. When a public alias is required, emitted code must expose an importable symbol with the alias name without adding an observable wrapper when the backend can avoid it.

Method aliases should emit as method aliases or method re-exports when the backend supports them. If the backend has no native representation, the implementation may lower call sites to the canonical method target or generate a wrapper method as a backend detail, provided language-level metadata, docs identity, call typing, receiver binding, and diagnostics still preserve the alias relationship.

If a backend cannot represent an alias without a wrapper, the wrapper must be treated as a backend implementation detail. It must not change language-level alias metadata, docs identity, call typing, or diagnostics.

### Diagnostics

The compiler must emit targeted diagnostics for at least:

- unresolved alias target;
- target kind is not supported for aliases;
- method alias target is not a same-type method;
- duplicate alias name;
- alias name collides with an existing declaration or member;
- alias cycle;
- public alias targets a private or non-exportable symbol;
- unsupported alias target expression;
- top-level assignment that is not alias-shaped.

Diagnostics should mention the alias name and the target spelling. For call-site errors through an alias, diagnostics should report the alias name at the use site and may add a note naming the canonical target.

## Design details

### Why bare `Name = Target` is allowed

The RFC allows bare alias-shaped assignment because it is the most natural spelling for Python-shaped APIs. It reads as a simple rebinding of a name without forcing authors to introduce ceremony for every synonym.

The important constraint is semantic, not visual: at module level, `Name = Target` is accepted only as an alias declaration. It does not execute arbitrary code, initialize mutable globals, call functions, allocate values, or create runtime module state.

This lets Incan keep a declaration-only module top level while still accepting the Pythonic alias spelling for the narrow symbol-alias case.

### Why `alias` remains available

The optional `alias` marker gives authors a more explicit form when clarity matters, especially around public APIs or dense declaration lists:

```incan
mean = alias avg
```

The marker does not affect semantics. A formatter should preserve the author's chosen spelling unless the style guide later decides to normalize one form.

### Relationship to method aliases

Method aliases are defined inside the owning type because receiver binding is part of method meaning. A same-type method alias keeps the receiver surface intact:

```incan
stats.mean()
```

This RFC intentionally does not allow top-level aliases to `Type.method`. That form is an unbound method reference and was deferred by RFC 035. It needs a separate design because it must decide how receivers are represented in the resulting callable.

### Relationship to generic function references

RFC 035 deferred generic function references in ordinary value position. Aliases do not reopen that entire design. A generic alias is a declaration-time symbolic relationship to a generic callable, not a runtime value capture.

This means the following can be valid:

```incan
id = identity
id[int](1)
```

while the following can remain invalid until generic function values receive a separate design:

```incan
f = identity
```

### Relationship to type aliases

`type Name = TypeExpression` remains the syntax for aliases over type expressions.

`Name = TargetType` is a symbol alias over an existing type-like symbol. It does not introduce a new structural type expression, does not create a new nominal type, and does not replace `type` for complex type expressions.

For example:

```incan
User = AccountUser          # symbol alias
type UserId = int           # type alias over a type expression
```

### Relationship to import aliases

Import aliases rename symbols at the import site:

```incan
from stats import avg as mean
```

Symbol aliases declare a symbol in the producer or re-exporting module:

```incan
pub mean = avg
```

Both are useful. Import aliases are consumer-local conveniences. Symbol aliases are part of the module's API and appear in manifests, docs, and tooling.

### Relationship to wrappers

A forwarding function or forwarding method remains valid when the author wants new behavior:

```incan
def mean(x: int) -> int:
    return avg(x)
```

That is not an alias. It owns a body, can add validation or coercion, has distinct documentation, and may have different runtime behavior. `mean = avg` says there is no new behavior.

### Relationship to partial callable presets

Partial callable presets are not aliases. An alias preserves the identity and callable surface of an existing symbol. A partial preset creates a derived callable with some arguments pre-filled and a projected callable surface.

This RFC does not define partial syntax or partial target eligibility. RFC 084 defines that feature separately and must explicitly state any extension that lets aliases target partial declarations.

### Compatibility / migration

This RFC is additive. Existing valid programs continue to parse and typecheck the same way.

Programs that currently fail with top-level `mean = avg` can migrate by relying on the newly defined alias declaration semantics.

Forwarding functions and forwarding methods can migrate to aliases when they are purely mechanical wrappers and when their public behavior and documentation should be identical to the target symbol.

## Alternatives considered

1. **LHS-oriented alias syntax: `alias mean = avg`**
   - Rejected because prefix syntax is less consistent with existing RHS-marked derived declaration forms such as `type UserId = newtype int`. The explicit marker remains available only on the RHS as `mean = alias avg` when authors want clarity.

2. **General top-level assignment**
   - Rejected because it would define module initialization order, mutable global behavior, and executable module statements. This RFC only admits alias-shaped declarations.

3. **Public facade aliases over private targets**
   - Deferred because `pub PublicName = alias PrivateName` as a public facade requires API projection, private-name leak prevention, docs rewriting, and backend visibility rules. That should be designed separately.

4. **Curated member projection blocks**
   - Deferred because forms such as `pub PublicClass = alias PrivateClass: pub method` introduce member visibility and export projection. That is larger than symbol aliasing.

5. **Top-level unbound method aliases**
   - Deferred because `mean = alias Stats.avg` needs unbound method reference semantics, including receiver representation and call syntax. Same-type method aliases solve the ergonomic method synonym case without reopening that design.

6. **Const alias: `const mean = avg`**
   - Rejected because aliases are symbol relationships, not const values. Treating functions or types as const initializers would mix separate contracts and weaken const-evaluation diagnostics.

7. **Import-only aliases**
   - Rejected because they solve only consumer-local renaming. The producer cannot publish a stable alias identity that documentation, manifests, and package consumers can share.

8. **String metadata**
   - Rejected because strings are not resolved symbols. They are fragile under refactoring and cannot participate in imports, typechecking, or compiler diagnostics without rebuilding a parallel symbol-resolution system.

## Drawbacks

Aliases add new declaration forms and therefore require parser, formatter, symbol-resolution, metadata, manifest, documentation, and import/export support.

Allowing bare `Name = Target` at top level may create user expectations that `x = 1` or `x = make_value()` should also work. Diagnostics must be explicit that top-level assignment is only admitted for alias declarations and should direct users to `const`, `static`, or `def` for other cases.

Preserving alias identity in metadata is more work than flattening aliases into target declarations. Flattening would be simpler in the short term, but it would lose the main value of the feature for documentation, refactoring, package APIs, and diagnostics.

Method aliases add member-surface complexity. They are still worth including because they avoid the worse alternative: treating `Type.method` as a top-level unbound method reference before that feature has a proper receiver design.

## Implementation architecture (non-normative)

An implementation should treat aliases as declaration-level symbol relationships collected before declaration checking. Alias validation can then resolve targets after declarations, imports, and type-body members have populated the relevant symbol surfaces.

The frontend should keep alias metadata separate from target declarations so downstream layers can distinguish `mean = avg` from `def mean(...)` and `mean = avg` inside a type from a second method body.

Manifest and checked API metadata should grow explicit alias representations rather than duplicating function, type, or method entries. Import-time typechecking can project the target kind and signature through that alias entry.

## Layers affected

- **Parser / AST**: must recognize bare and RHS-explicit alias declarations at module top level and same-type method aliases inside method-bearing type bodies.
- **Typechecker / Symbol resolution**: must collect aliases, resolve targets after relevant symbols are known, reject invalid target kinds and cycles, and expose alias behavior at use sites.
- **IR Lowering**: must preserve enough alias information for backend emission when a public top-level alias or method alias needs a generated symbol, while allowing private alias uses to resolve to the canonical target when appropriate.
- **Emission**: should emit backend-native aliases or re-exports when available and must avoid changing language-level aliases into independently documented wrappers.
- **Library manifests**: must export public aliases as alias metadata with projected semantic kind, signatures when callable, and target provenance.
- **Checked API metadata**: must represent top-level aliases and method aliases distinctly so documentation and package tooling can show alias identity.
- **Formatter**: must format alias declarations deterministically and should preserve whether the author used the explicit RHS `alias` marker unless a later style rule says otherwise.
- **LSP / Tooling**: should surface alias hover, go-to-definition, rename, completion, and diagnostics using both the alias name and canonical target where useful.
- **Docs**: should document aliases as aliases rather than duplicate function, type, or method declarations.

## Design Decisions

- Public aliases support both `pub Name = Target` and `pub Name = alias Target` immediately. The two spellings are equivalent, matching the private alias rule.
- The planned target set includes all listed top-level function and type-like symbols: functions, method-independent imported callables, models, classes, enums, newtypes, type aliases, traits, and aliases that resolve acyclically to those supported targets.
- Formatter behavior should preserve whether the author used the explicit RHS `alias` marker unless a later style RFC changes that rule.
- Trait method aliases are in scope for this RFC. Implementation may stage concrete type-body aliases before trait method aliases, but the language contract includes same-trait method aliases.
