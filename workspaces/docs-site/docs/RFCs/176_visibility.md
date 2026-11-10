# RFC 176: Visibility

- **Status:** Draft
- **Created:** 2026-11-10
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 031 (library system)
    - RFC 083 (symbol and method aliases)
    - RFC 084 (callable presets)
    - RFC 085 (field metadata and rename continuity)
    - RFC 122 (model construction)
    - RFC 125 (`pub::` namespaces)
    - RFC 137 (named enum payload fields)
    - RFC 149 (non-exhaustive public contracts)
    - RFC 174 (overloaded functions and methods)
- **Issue:** [#2570](https://github.com/encero-systems/incan/issues/2570), [#2812](https://github.com/encero-systems/incan/issues/2812)
- **RFC PR:** —
- **Written against:** v0.6
- **Shipped in:** —

## Summary

A declaration is private unless it is marked `pub`. An unmarked top-level declaration is private to its module. An unmarked field or method of a class, or of a model, enum or newtype marked `pub`, is private to its type. Privacy holds everywhere a declaration can be reached: in calls, construction, patterns, aliases, re-exports, compiled libraries and generated output. There are two levels, exported and private, and no third.

## Motivation

The rule shipped one piece at a time. Module exports came with the library system (RFC 031), private model fields in 0.5, private methods in 0.6 (#2570), and the rule that only a class sets its own private fields in 0.6 (#2812). Each piece is in a release note, and the member rule is in the language reference, but no RFC states the whole design or why it has this shape. Without one, a later change can loosen a piece without noticing what it breaks, as the 0.5 checker did when it let methods ignore the default that fields followed.

A library author needs to know that what they don't mark stays theirs to change. A consumer needs to know that what they can call is what the author promised.

## Guide-level explanation

```incan
pub class Ledger:
    pub total: int
    entries: list[int] = []

    pub def report(self) -> str:
        return f"checksum {self.checksum()}"

    def checksum(self) -> int:
        return self.total * 31


def audit(ledger: Ledger) -> int:
    return ledger.checksum()    # refused: checksum is private to Ledger
```

`total` and `report` are exported with the class. `entries` and `checksum` belong to `Ledger`: only its own methods use them. A function in the same file is outside the class.

A type without `pub` is already private to its module, so its unmarked members are visible wherever the type is:

```incan
model Point:
    x: int
    y: int

def norm(p: Point) -> int:
    return p.x * p.x + p.y * p.y    # accepted: Point and its fields are module-private together
```

## Reference-level explanation

### Modules

- A top-level declaration marked `pub` is exported from its module. An unmarked one is private to its module.
- `from M import X` and `import M::X` of a declaration `M` does not export are refused (`INCAN-I0001`).
- `pub from M import X` re-exports `X`. A re-export or a public alias of `X` exposes `X` and no more.
- A library's public surface is what its root exports and the `pub::` namespaces RFC 125 derives from them.

### Members

One rule covers fields and methods.

- An unmarked member of a class, or of a model, enum or newtype marked `pub`, is private to its type. A member marked `pub` is visible wherever its type is.
- An unmarked member of a model, enum or newtype without `pub` is visible wherever the type is, which is its own module.
- A member private to its type is reachable only from the methods and computed properties that type declares, in the module that declares it. A module-level function is outside the type, even in the same file. A subclass's methods do not reach the private members they inherit.
- A method declared in a trait, and a method that implements one, takes the visibility of the trait that declares it. `pub` on either is refused (`INCAN-P0001` in a trait, `INCAN-T0001` on an implementation).
- A language hook, such as `__eq__`, `__str__`, `__incan_new`, a newtype's `from_underlying` or `validate` on a type deriving `Validate`, is callable wherever its type is visible, and `pub` on one is refused.
- A method alias or method partial takes the visibility of its target. Each overload (RFC 174) has its own visibility.
- An enum's variants and their payload fields are visible wherever the enum is (RFC 137).
- A call or read of a member private to its type from outside that type is refused (`INCAN-T0001`).

### Construction and patterns

- A class's private fields are set only by the class's own methods. Constructing the class with a private field from anywhere else, including a function in the same module and a subclass, is refused (`INCAN-T0135`). A private field the construction omits takes its declared default.
- A model's private fields are constructed through its own construction contract (RFC 122). A model with a required private field is constructible outside its type only through a factory the model declares.
- A constructor pattern names only fields visible where the pattern is written, and the fields it leaves out match any value. Naming a private field there is refused.

### Compiled libraries and generated output

- Library metadata records the visibility of every declaration and member. A consumer of a compiled library sees exactly what a consumer of its source would, through direct imports, aliases and any number of re-exports.
- A compiled class with private fields is constructed through the package's own construction path, so a consumer cannot set those fields.
- Generated output never makes a private declaration or member reachable: no public constructor that takes private fields, and no public accessor that the source lacks.
- Compiled libraries must carry the visibility metadata required by the current toolchain contract. Missing or unsupported visibility metadata makes the artifact incompatible: the compiler must refuse it with a diagnostic naming the library and requiring a rebuild with a supported toolchain. It must not infer public visibility or provide a legacy-artifact compatibility fallback.
- Field metadata may record `made_private` when a field changes from public to private, so tools can show the migration (RFC 085).

### Standard library

The standard library follows the same rule. Its implementation modules and private helpers are private like any other library's.

### Compatibility

The 0.5 release made unmarked model fields private, and 0.6 extended the rule to methods and to class field construction. Each change has a migration note and a diagnostic that names the member and its type. Widening a declaration to `pub` is a compatible change for consumers; narrowing it is a breaking change.

## Alternatives considered

- **Public by default.** Every unmarked member would become a promise the author never chose to make, and a library could not change its internals without breaking someone.
- **Separate rules for fields and methods.** This is what 0.5 had by accident, and it let code call methods that the fields beside them kept private.
- **A package-internal level between private and exported.** It is not part of this design, which keeps two levels.
- **Letting module-level functions in the same file reach a class's private members.** The class's invariants would then depend on code outside the class, so a class-owned factory is the way in.

## Drawbacks

- Code that relied on reaching unmarked members must add `pub` or move into the type.
- A type without `pub` exposes its members within its module, which a reader must remember when reading a module-private type.

## Layers affected

- **Parser:** refuses `pub` on trait methods.
- **Typechecker:** export checks, the member rule, construction and pattern checks.
- **Lowering / IR emission:** emits no public constructor or accessor for a private member.
- **Stdlib:** follows the rule like any library.
- **CLI / tooling:** library metadata records visibility, and API documentation lists only exported declarations and members.
- **LSP / formatter:** completion offers only reachable names. The formatter is unchanged.

## Inspectability and tooling surface

- **Artifact or metadata:** compiled library metadata records the visibility of each declaration and member.
- **Inspection command:** checked API documentation (RFC 082) and `incan check`.
- **Diagnostics:** `INCAN-I0001` for importing an unexported name, `INCAN-T0001` for reaching a private member, `INCAN-T0135` for setting a private class field, and `INCAN-P0001` for `pub` in a trait. Each names the declaration and the type or module it is private to.
- **Provenance:** the declaration's span and its `pub` marker, or the absence of one.
- **Not implicit:** nothing becomes public without `pub`, and nothing private becomes reachable through generated output.

## Design Decisions

- **Private by default.** What an author exports is a decision, so leaving `pub` off keeps the freedom to change.
- **One member rule.** Fields and methods of one type follow one rule, so a reader learns it once.
- **A module-private type contains its members.** Marking each member of a type nobody outside the module can name would add nothing.
- **Trait methods follow their trait.** Calling a trait method is a promise the trait makes, not the implementing type.
- **Privacy survives compilation.** A compiled library's consumer sees what a source consumer sees. Missing metadata is an incompatible artifact, never permission to expose members. This RFC provides no compatibility path for older compiled artifacts that do not satisfy the current contract.
