# RFC 174: Overloaded functions and methods

- **Status:** Planned
- **Created:** 2026-11-10
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 025 (multi-instantiation trait dispatch)
    - RFC 028 (trait-based operator overloading)
    - RFC 035 (first-class function references)
    - RFC 038 (variadic positional arguments and keyword capture)
    - RFC 054 (explicit call-site generics)
    - RFC 082 (checked API documentation generation)
    - RFC 102 (semantic layer inspection surface)
    - RFC 106 (declaration identity in the compiler-backed context graph)
    - RFC 122 (model construction)
    - RFC 128 (one `Callable` trait over function types)
- **Issue:** [#2973](https://github.com/encero-systems/incan/issues/2973) (implementation and conformance); [#2890](https://github.com/encero-systems/incan/issues/2890) (completed regression fix)
- **RFC PR:** —
- **Written against:** v0.6
- **Shipped in:** —

## Summary

A function or method name may have several declarations with different parameter lists. Together they form an overload set, and each call selects exactly one of them at compile time from what the call itself writes: how many arguments it passes, their types, the names of its named arguments and its explicit type arguments. A call that no overload accepts, or that two accept equally well, is refused, and the diagnostic lists the candidates. The same rule covers functions, methods, local partials and function references.

Incan has accepted same-name functions and methods with different parameter lists since 0.5. This RFC states that behavior as a contract and settles its edges.

## Motivation

Some operations have one meaning and more than one natural input. A writer writes bytes or text, a parser reads a string or a byte buffer, and a reader returns untyped rows or rows of a model the caller names. Giving each input its own name (`write_bytes`, `write_text`) makes callers learn a vocabulary for one idea. RFC 025 rejected separate names for the same reason when it allowed one subscript to take a `str` or an `int`.

RFC 054 relies on overloading too. Its `read_csv` and `read_csv[T]` pair gives an untyped entry point and a typed one under one name. RFC 028 set general callable overloading aside as a separate follow-on, and RFC 054 describes the pair as that follow-on, but no RFC defined it.

The behavior shipped regardless. 0.5 accepted `Writer.write(self, data: bytes)` beside `Writer.write(self, value: u8, endian: str)`, and published functions overload in 0.6: an overloaded function keeps its selected declaration through re-exports and aliases, and a call no overload accepts lists every candidate with the reason it failed. Without a contract, the rules drifted. The 0.6 checker refuses two ordinary methods with one name unless each satisfies a different trait instantiation (#2890), while functions still overload, and the language explanation tells readers to give ordinary methods distinct names. One rule should hold for both.

RFC 025's trait instantiations aren't a substitute. They select among implementations of one trait. Requiring a trait for every overload adds a declaration that names nothing the user cares about.

## Guide-level explanation

Declare the same name more than once, with parameters that tell the calls apart:

```incan
class Writer:
    pub def write(self, data: bytes) -> int:
        """Write raw bytes and return how many were written."""
        ...

    pub def write(self, text: str, encoding: str = "utf-8") -> int:
        """Encode text and write it."""
        ...


writer = Writer()
writer.write(b"\x00\x01")                # bytes overload
writer.write("héllo")                    # text overload
writer.write("héllo", encoding="latin-1")
```

Functions overload the same way:

```incan
def parse(text: str) -> Config:
    ...

def parse(data: bytes) -> Config:
    ...
```

Explicit type arguments select by the number of type parameters, so an untyped entry point and a typed one can share a name:

```incan
def read_csv(path: str) -> list[JsonValue]:
    ...

def read_csv[T](path: str) -> list[T]:
    ...


rows = read_csv("orders.csv")             # list[JsonValue]
orders = read_csv[Order]("orders.csv")    # list[Order]
```

When more than one overload accepts a call, the one with the narrowest parameter types is selected. When none is narrowest, the call is refused:

```incan
def area(shape: Circle | Square) -> float:
    ...

def area(shape: Circle | Triangle) -> float:
    ...


area(circle)    # refused: both accept a `Circle`, and neither is narrower
```

```text
error: ambiguous call to `area`
 --> shapes.incn:9:1
  |
9 | area(circle)
  | ^^^^^^^^^^^^ two overloads accept a `Circle`
  |
note: candidate `area(shape: Circle | Square)`
 --> shapes.incn:1:1
note: candidate `area(shape: Circle | Triangle)`
 --> shapes.incn:4:1
help: give the argument a type only one overload accepts, such as `shape: Circle | Square = circle`
```

Two overloads that the same call could never tell apart are refused where the second is declared:

```incan
def scale(x: int, factor: float = 1.0) -> float:
    ...

def scale(x: int, offset: int = 0) -> float:    # refused: `scale(1)` fits both
    ...
```

A local partial selects its overload from the arguments it binds. Only the text overload has an `encoding` parameter, so this partial takes `text: str`:

```incan
write_latin1 = partial writer.write(encoding="latin-1")
write_latin1("héllo")
```

A function reference names the whole set. It is accepted where the expected callable type selects one overload:

```incan
sink: Callable[(bytes) -> int] = writer.write    # the bytes overload
handler = writer.write                           # refused: `write` has two overloads; annotate the callable type
```

## Reference-level explanation

### Overload sets

- Two or more `def` declarations with one name in one module, or in one type's declaration, form an overload set. Each declaration is an overload.
- Overloads may differ in their parameters, their defaults, their type parameters, their return types, their docstrings and their visibility.
- All overloads of a method declare the same receiver: each takes `self`, each takes `mut self`, or none takes a receiver.
- An overload's decorators apply to that overload alone.

### Distinct overloads

- For calls using only positional arguments, an overload accepts a range of counts: from the number of its parameters without default values to the number of all its parameters, or with no upper bound when it has `*args`. This range is used for declaration-conflict checks below; it is not a minimum positional-argument requirement on a call that also supplies named arguments.
- Two overloads in one set conflict when there is a count that both accept and their parameter types are the same at every position up to that count. The later declaration is refused, and the diagnostic labels the earlier one. There is one fixed-versus-variadic exception: when exactly one declaration has `*args`, a shared count does not cause a conflict if at least one supplied position binds to an ordinary parameter in the fixed declaration and to `*args` in the variadic declaration. The fixed-parameter preference below resolves that overlap. A shared count with no such position still conflicts; merely adding an unused rest parameter does not distinguish two declarations. This check must preserve RFC 025's exception for methods implementing distinct instantiations of the same generic trait: those implementations may share parameter types and differ in their result types, as specified under "Methods and trait instantiations".
- Types compare after aliases resolve. A newtype is its own type. Type parameters compare by their position in the type-parameter list.
- Two overloads with different numbers of type parameters never conflict, because a call's explicit type arguments select by that number.
- Parameter names never make two overloads distinct. Return types do not distinguish ordinary overloads; only the same-generic-trait exception above permits that distinction.

### Variadic capture and fixed parameters

The concise spelling `*values: int` stays unchanged: `int` describes each supplied argument. It is syntax sugar for a rest parameter whose collected value is a homogeneous tuple of integers, including an empty tuple when no values are supplied. The tuple's length may be determined at runtime; this does not require a separate overload for each length. The checked signature retains both the element type and the rest-parameter kind, rather than treating the declaration as one fixed `int` parameter or one fixed tuple parameter.

```incan
def total(value: int) -> int:
    return value

def total(*values: int) -> int:
    mut result: int = 0
    for value in values:
        result += value
    return result


total(5)          # fixed parameter wins; both declarations are valid
total(5, 7, 9)    # variadic overload; values is (5, 7, 9)
total()           # variadic overload; values is ()
```

The tuple describes the collected value inside the function, not an extra tuple argument at the call site. When the element type is itself a tuple, each argument is a tuple and the capture is a tuple of those tuples; the annotation still describes one element. This clarification introduces no alternative container-annotation spelling. `**kwargs` retains RFC 038's dictionary capture.

This section amends [RFC 038](closed/implemented/038_variadic_positional_args_and_keyword_capture.md), which remains closed as Implemented and records the original list-based capture. Tuple capture is proposed here, not claimed as already shipped. Existing iteration and indexing retain their meaning; mutation of the captured container and APIs requiring that container to be a list need an explicit list construction. The tuple does not make each contained value deeply immutable. Existing call-site unpacking and shape checks remain governed by RFC 038.

### Selecting an overload

A call selects in three steps.

1. **Candidates.** An overload is a candidate when all of these hold:
    - it is visible where the call is;
    - positional arguments bind to parameters in declaration order, with any excess accepted only by `*args`;
    - each named argument binds to its matching parameter or, if no parameter has that name, to `**kwargs`; an unsupported name is refused;
    - no parameter receives an argument twice, and no name is supplied twice to `**kwargs`; a named argument cannot be diverted into `**kwargs` when a positional argument already filled its matching parameter;
    - after both positional and named arguments bind, every required parameter receives an argument; omitted parameters must have defaults or be variadic captures, which may be empty;
    - when the call writes explicit type arguments, their number equals its number of type parameters, with `_` placeholders inferred;
    - when the call writes none, the arguments determine all of its type parameters.
2. **Types.** A candidate remains when each argument's type is accepted by the type of the parameter it fills, inferring the candidate's type parameters as for any call.
3. **Choice.**
    - When one candidate remains, it is selected.
    - When none remains, the call is refused, and the diagnostic lists every overload with the reason it failed: the argument count, a named argument, the type at a position, or the number of type arguments.
    - When several remain, one is selected if it is narrower than each of the others. Overload A is narrower than B for a call when each of A's parameter types filled by the call is accepted by B's, and at least one of them is not the same. If those types are the same, an overload without type parameters is narrower than one with them, and an overload that takes an argument in a declared parameter is narrower than one that captures it in `*args`. Otherwise the call is refused as ambiguous, and the diagnostic lists the tied candidates.

For ordinary overloads, the type the call's result is expected to have plays no part in selection. The same-generic-trait exception below retains RFC 025's result-based selection. Declaration order never breaks a tie.

### Defaults and named arguments

- A default value belongs to its overload, and it is evaluated only when that overload is selected and the call omits the argument.
- A named argument removes every candidate that neither declares that parameter nor captures the name in `**kwargs`, so `writer.write("x", encoding="latin-1")` has one candidate. Names never make two declarations distinct.

Positional and named arguments satisfy the same required parameters:

```incan
def describe(value: int) -> str:
    return f"Number: {value}"

def describe(value: str) -> str:
    return f"Text: {value}"


describe(42)                 # int overload
describe(value=42)           # the same int overload, with no positional arguments
describe(value="hello")      # str overload
describe()                  # refused: required value is missing
describe(42, value=43)       # refused: value is supplied twice
describe(unknown=42)         # refused: neither overload accepts unknown
```

The checker binds arguments separately for each candidate before testing for missing required parameters. It must not discard `describe(value=42)` for supplying zero positional arguments.

### Methods and trait instantiations

- A method's overload set on a type holds the type's own overloads of that name and the same-name methods it has through trait adoptions, and one selection covers them all.
- A declared method whose signature matches a trait method's signature implements that trait method, as RFC 025 matches it, rather than adding an overload.
- When only instantiations of one generic trait remain tied and their parameter types are the same, the expected result type chooses among them, as RFC 025 specifies. That is the only place a result type takes part in selection.
- This replaces RFC 025's rule that same-name methods are permitted only when they satisfy different trait adoptions. Its dispatch rules otherwise stand.
- Operator dunders keep RFC 028's rules, `__call__` stays single (RFC 128), and construction stays with RFC 122. This RFC overloads none of them.

For example, implementations of `Convert[int]` and `Convert[str]` may both declare `convert(self)` with their respective result types. Both declarations are accepted. `number: int = value.convert()` selects `Convert[int]`, and `text: str = value.convert()` selects `Convert[str]`; without a selecting context, the call is ambiguous. Two ordinary `convert(value: Reading)` functions differing only in those result types are still refused at declaration.

### Local partials

`partial f(…)` selects its overload where the partial is written. The candidates are the overloads that accept the bound arguments and explicit type arguments, under the rules above, while leaving parameters without default values unbound. Exactly one must be selected; otherwise the partial is refused like a call, with the same diagnostic. The partial takes the selected overload's remaining parameters.

### Function references

- A reference to an overloaded function or method, such as `parse`, `writer.write` or `Writer.write`, names the set.
- It is accepted where an expected callable type selects one overload: the overload whose parameter types equal that type's parameter types, and whose return type that type accepts. Explicit type arguments on the reference select by type-parameter count first.
- Anywhere else, such as an unannotated binding, it is refused. The diagnostic lists the overloads and shows the annotation that would select each.
- A reference to a name that is not overloaded is unchanged.

### Visibility, re-exports and published libraries

- Each overload has its own visibility, and a call sees only the overloads visible to it. Conflicts are checked across the whole set, whatever the visibility.
- An import, re-export or alias of an overloaded name carries every overload visible to it, so a call through it selects among the same overloads as a call through the original declaration.
- Each overload is a separate declaration with its own declaration identity (RFC 106). Library metadata, checked API documentation (RFC 082) and the semantic inspection surface (RFC 102) record each overload with its own parameter types and docstring, and record which overload each call selected.
- Adding an overload to a published name is a compatible change only when no call that the earlier set accepted can select it or tie with it. Otherwise it is a breaking change, as is removing an overload or narrowing its parameters.

### Reflection

`ShapeInfo`, `FieldInfo` and `VariantInfo` (RFC 173) describe stored state, and overloads add nothing to them. Where reflection or inspection lists callable members, each overload is its own entry with its parameter types.

### Compatibility

- Apart from the variadic capture amendment above, every program 0.5.1 accepted with same-name functions or methods stays valid, except one where two overloads conflict under "Distinct overloads". Such a pair could never be called unambiguously at the count they share, and the refusal names both declarations.
- The 0.6 refusal of ordinary same-name methods (#2890) is withdrawn.
- RFC 054's `read_csv` and `read_csv[T]` pair works as that RFC writes it.

## Alternatives considered

- **Distinct names only.** This contradicts shipped behavior, and it makes callers learn a name per input shape.
- **Python's `@overload` over one implementation.** RFC 025 rejected runtime dispatch in a compile-time language, and stub signatures over one body split the contract from the code.
- **A trait for every overload.** This is RFC 025's mechanism stretched past its purpose, with one declaration per overload that names nothing.
- **Selection by expected result type.** The same call would select differently depending on where its result goes. RFC 025 keeps this to instantiations of one generic trait, and so does this RFC.
- **Parameter names as a distinction.** Every positional call at the shared count would be ambiguous for every caller.
- **Declaration order as a tiebreak.** The tie would be settled silently, so a reordered file would change behavior.

## Drawbacks

- Adding an overload can change which overload an existing call selects. The compatibility rule makes that a breaking change, but authors must know to check for it.
- A refused call over a large set lists many candidates.
- A reference to an overloaded name needs an annotation where one name would not.
- Hover, go-to-definition and completion must follow the selected overload instead of the name.

## Layers affected

- **Parser:** none. Same-name declarations already parse.
- **Typechecker:** overload sets, conflict checks, selection at calls, partials and references, the method-set merge with trait adoptions, and tuple typing for positional rest captures.
- **Lowering / IR emission:** each overload lowers to its own function, and each call reaches the overload it selected. Positional rest captures construct the homogeneous tuple described above, including when unpacked inputs have runtime lengths.
- **Stdlib:** none.
- **CLI / tooling:** checked API documentation and library metadata list each overload.
- **LSP / formatter:** hover, go-to-definition and signature help follow the selected overload, and completion lists every overload. The formatter is unchanged.

## Inspectability and tooling surface

- **Artifact or metadata:** checked library metadata records each overload with its parameter types and docstring, and each call records the overload it selected.
- **Inspection command:** the existing API documentation and semantic inspection commands (RFC 082, RFC 102).
- **Diagnostics:** conflicting overloads, a call no overload accepts, an ambiguous call, an ambiguous partial and a reference with no expected callable type. Each has its code, labels the call and every candidate, and gives a hint that names the fix.
- **Provenance:** each overload's declaration span, and the link from each call to the overload it selected.
- **Not implicit:** no runtime dispatch, no selection by result type outside RFC 025's case and no declaration-order tiebreak.

## Design Decisions

- **Selection is static and local.** A call is decided by what it writes, so reading the call and the set is enough to know which overload runs.
- **Narrowest wins, otherwise refuse.** A narrower overload is the specific case its author wrote. When neither of two is narrower, guessing would hide a design question in the overload set.
- **Ordinary overloads are distinguished by inputs.** Parameter names and result types do not distinguish ordinary declarations. Methods implementing distinct instantiations of the same generic trait retain RFC 025's narrow exception at both declaration checking and call selection.
- **One rule for functions and methods.** The split between them in 0.6 had no reason behind it.
- **Visibility filters candidates, but conflicts span the whole set.** A library's private overloads never change what an outside call selects, and they can't hide a conflict from the library's author.
