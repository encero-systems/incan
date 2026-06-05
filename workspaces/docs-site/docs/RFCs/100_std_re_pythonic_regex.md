# RFC 100: `std.re` — Pythonic regular expressions

- **Status:** Draft
- **Created:** 2026-05-18
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 022 (namespaced stdlib modules and compiler handoff)
    - RFC 023 (compilable stdlib and Rust module binding)
    - RFC 059 (`std.regex`)
    - RFC 070 (Result combinators)
- **Issue:** https://github.com/dannys-code-corner/incan/issues/668
- **RFC PR:** —
- **Written against:** v0.3
- **Shipped in:** —

## Summary

This RFC proposes `std.re` as a Pythonic regular-expression module built on the same internal regex engine family as `std.regex` but with a distinct public contract: `std.regex` remains the safe, predictable Rust-regex/RE2-style module, while `std.re` accepts Python-like syntax and behavior that require backtracking semantics, including lookaround and pattern backreferences. The feature gives users an explicit choice between predictable regex matching and Python portability without overloading one API with incompatible safety and compatibility expectations.

## Core model

1. **One engine family, two module contracts:** Incan should share parsing, diagnostics, capture representation, and backend infrastructure where possible, but `std.regex` and `std.re` expose different public promises.
2. **`std.regex` remains the safe contract:** patterns accepted by `std.regex` must stay inside the predictable regular-feature subset and must reject backtracking-only constructs.
3. **`std.re` is the Pythonic contract:** patterns accepted by `std.re` may use Python-style lookaround, pattern backreferences, conditional subpatterns, Python replacement templates, and module-level helper functions.
4. **Backend selection is semantic, not hidden API magic:** the engine may classify patterns as safe-regular or backtracking-required, but users choose the contract by importing `std.regex` or `std.re`.
5. **Backtracking risk must be visible:** `std.re` must document that accepted patterns can be superlinear and should provide guardrails such as match limits, step budgets, or time budgets where the implementation can enforce them.
6. **Python compatibility is a goal, not an excuse to weaken Incan:** the API should feel familiar to Python users while preserving Incan's explicit `Result` and `Option` error/nullability model.

## Motivation

RFC 059 deliberately made `std.regex` a safe default. That choice is still correct: many Incan programs will scan logs, validate structured text, clean data, or process large files where regex should not introduce catastrophic backtracking risk. The cost is that users cannot port common Python `re` patterns that rely on lookaround, pattern backreferences, conditional groups, or Python replacement-template syntax.

The obvious answer is not to make `std.regex` silently accept those features. Doing so would break the core mental model of the module: users would no longer know whether a regex is safe for large inputs by looking at the import. A single overloaded API would also make docs worse because every method would need to explain two execution contracts.

`std.re` gives the project a clean story. Users who want predictable matching import `std.regex`. Users who want Python-like expressiveness import `std.re`. The implementation can still share an engine family internally, but the public modules keep their contracts separate and readable.

Python compatibility matters because Incan is intentionally Python-shaped in many places. Regex snippets are one of the most frequently copied pieces of code between Python programs, shell scripts, notebooks, CLIs, and data-cleaning utilities. If Incan requires users to rewrite every lookaround or backreference pattern before they can port a script, `std.regex` alone will feel arbitrarily limited rather than deliberately safe.

There is strong prior art for a Rust-hosted Python-like regex engine. RustPython's SRE crate describes itself as "A low-level implementation of Python's SRE regex engine" and pairs a Python-facing `re` module with a Rust matcher. Incan should not copy RustPython's object model wholesale, but the architectural lesson is useful: Pythonic regex behavior is better treated as a dedicated SRE-like engine surface than as a thin wrapper over Rust's safe regex crate.

## Goals

- Add a `std.re` module for Pythonic regular-expression matching, searching, splitting, and substitution.
- Keep `std.regex` and `std.re` as separate stdlib contracts with separate public types.
- Support Python-like pattern features that `std.regex` intentionally excludes, including lookahead, fixed-width lookbehind, pattern backreferences, named backreferences, and conditional subpatterns.
- Support Python-like module helpers such as `re.compile`, `re.search`, `re.match`, `re.fullmatch`, `re.sub`, `re.subn`, `re.split`, `re.findall`, `re.finditer`, `re.escape`, and `re.purge` where their behavior can be expressed cleanly in Incan.
- Support Python-like flags such as `ASCII`, `IGNORECASE`, `MULTILINE`, `DOTALL`, and `VERBOSE`, with common aliases such as `A`, `I`, `M`, `S`, and `X`.
- Preserve Incan's explicit `Result` and `Option` flow for pattern errors and absent matches.
- Define a shared internal engine-family boundary so `std.regex` and `std.re` can share diagnostics, capture storage, and safe-subset execution where appropriate.
- Provide a compatibility and migration story for users choosing between `std.regex` and `std.re`.

## Non-Goals

- This RFC does not change the semantics of `std.regex`.
- This RFC does not make backtracking regex the default Incan regex behavior.
- This RFC does not require byte-for-byte compatibility with every CPython `re` edge case in the first implementation slice.
- This RFC does not standardize the third-party Python `regex` package or PCRE2 as Incan's public contract.
- This RFC does not make `std.re.Pattern` and `std.regex.Regex` the same public type.
- This RFC does not require regex literals or new parser-level string syntax.
- This RFC does not require locale-dependent matching in the initial slice.
- This RFC does not require a public API for arbitrary engine selection through one overloaded constructor.

## Guide-level explanation

Use `std.regex` when the pattern should be safe and predictable for large inputs. Use `std.re` when the pattern is Python-like and depends on features outside the safe regular subset.

```incan
from std.regex import Regex

word = Regex(r"\w+")?
assert word.is_match("hello")
```

The Pythonic module is imported separately:

```incan
from std.re import PatternError
import std.re as re


def main() -> Result[None, PatternError]:
    repeated = re.compile(r"(?P<word>\w+)\s+(?P=word)", re.IGNORECASE)?
    match repeated.search("Echo echo"):
        Some(found) => println(found.group("word").unwrap_or(""))
        None => println("no duplicate word")
    return Ok(None)
```

Patterns that are not valid in `std.regex` are valid in `std.re` when they are part of the Pythonic contract:

```incan
from std.re import PatternError
import std.re as re


def main() -> Result[None, PatternError]:
    preceded = re.compile(r"(?<=\$)\d+(?:\.\d+)?")?
    price = preceded.search("total: $19.95")
    assert price.is_some()
    return Ok(None)
```

The module-level helpers mirror Python's `re` style while still using Incan's error flow. A helper that compiles a pattern must return `Result[...]` because invalid patterns are ordinary recoverable errors in Incan:

```incan
from std.re import PatternError
import std.re as re


def normalize(text: str) -> Result[str, PatternError]:
    return re.sub(r"\s+", " ", text)?
```

Substitution templates follow Python's backslash-based replacement style. Use a callable replacement when the replacement depends on code:

```incan
from std.re import PatternError
import std.re as re


def main() -> Result[None, PatternError]:
    swapped = re.sub(r"(?P<first>\w+)\s+(?P<last>\w+)", r"\g<last>, \g<first>", "Ada Lovelace")?
    assert swapped == "Lovelace, Ada"
    return Ok(None)
```

The mental model is deliberately simple: `regex` means safe regex; `re` means Pythonic regex. The implementation may share machinery, but users should not have to understand backend selection to choose the right module.

## Reference-level explanation

### Module boundary

`std.re` must be a separate stdlib module from `std.regex`. The module must not re-export `std.regex.Regex` as its primary pattern type. The primary compiled pattern type must be named `Pattern`, and the primary match type must be named `Match`. The module should expose `PatternError` as the pattern compilation and template error type and may expose `error` as a Python-compatible alias for `PatternError`.

`std.regex` must continue to reject backtracking-only constructs. `std.re` may accept those constructs. If both modules share internal code, the shared implementation must not weaken the public guarantees of `std.regex`.

### Pattern compilation

`re.compile(pattern: str, flags: int = 0) -> Result[Pattern, PatternError]` must compile a Pythonic regex pattern. Compilation must return `Err(PatternError)` for invalid syntax, unsupported constructs, invalid flags, invalid group references, invalid lookbehind width, and invalid replacement templates when a template is compiled with a pattern.

`PatternError` must expose at least a stable `kind() -> str` category and a human-readable `message() -> str`. Error messages should include enough location context to explain the rejected part of the pattern when the parser can determine it.

### Pattern syntax

`std.re` must support the safe regular subset already available through `std.regex`, including literals, character classes, quantifiers, alternation, grouping, anchors, indexed captures, named captures, inline flags, and Unicode-aware string matching.

`std.re` should additionally support Python-like constructs including positive lookahead `(?=...)`, negative lookahead `(?!...)`, fixed-width positive lookbehind `(?<=...)`, fixed-width negative lookbehind `(?<!...)`, numbered pattern backreferences such as `\1`, named pattern backreferences such as `(?P=name)`, non-capturing groups `(?:...)`, comments `(?#...)`, named captures `(?P<name>...)`, conditional subpatterns `(?(id/name)yes|no)`, greedy quantifiers, lazy quantifiers, and atomic groups when the accepted Python baseline includes them.

Lookbehind must require statically fixed width unless a later RFC explicitly chooses a different compatibility target. Backreferences must refer to existing capturing groups and must be rejected when their target is invalid or semantically impossible under the accepted baseline.

### Flags

`std.re` must expose Python-like flag constants. The initial flag set should include `NOFLAG`, `ASCII`, `IGNORECASE`, `MULTILINE`, `DOTALL`, `VERBOSE`, and `UNICODE`, plus aliases `A`, `I`, `M`, `S`, `X`, and `U`. `UNICODE` should be accepted for compatibility on string patterns but should not be required to enable Unicode matching when Unicode is already the default. `LOCALE` may be omitted from the first implementation slice unless the module can define a stable locale contract.

Flags should compose with bitwise OR. APIs accepting `flags` must reject flag combinations that the module does not support.

### Pattern methods

`Pattern` must expose at least these methods:

```incan
def match(self, string: str, pos: int = 0, endpos: int = sys.maxsize) -> Option[Match]: ...
def fullmatch(self, string: str, pos: int = 0, endpos: int = sys.maxsize) -> Option[Match]: ...
def search(self, string: str, pos: int = 0, endpos: int = sys.maxsize) -> Option[Match]: ...
def finditer(self, string: str, pos: int = 0, endpos: int = sys.maxsize) -> Iterator[Match]: ...
def split(self, string: str, maxsplit: int = 0) -> list[str | None]: ...
def sub(self, repl: str | Callable[Match, str], string: str, count: int = 0) -> Result[str, PatternError]: ...
def subn(self, repl: str | Callable[Match, str], string: str, count: int = 0) -> Result[Tuple[str, int], PatternError]: ...
```

`match` must attempt a match at `pos`. `search` must scan from `pos` through `endpos`. `fullmatch` must require the whole selected range to match. `finditer` must yield non-overlapping matches from left to right and must make progress after empty matches.

`split` must follow Python's important captured-separator behavior: if the separator pattern contains capturing groups, captured separator text must appear in the returned list, and unmatched optional separator groups must appear as `None`. If the pattern contains no capturing groups, only the split fields should appear.

`sub` must return the substituted string. `subn` must return the substituted string and the number of substitutions performed. A `count` of `0` must mean no replacement limit, matching Python's convention.

### Module helper functions

The module helper functions must compile the pattern and then delegate to the corresponding `Pattern` method. Their signatures should follow this shape:

```incan
def match(pattern: str | Pattern, string: str, flags: int = 0) -> Result[Option[Match], PatternError]: ...
def fullmatch(pattern: str | Pattern, string: str, flags: int = 0) -> Result[Option[Match], PatternError]: ...
def search(pattern: str | Pattern, string: str, flags: int = 0) -> Result[Option[Match], PatternError]: ...
def finditer(pattern: str | Pattern, string: str, flags: int = 0) -> Result[Iterator[Match], PatternError]: ...
def split(pattern: str | Pattern, string: str, maxsplit: int = 0, flags: int = 0) -> Result[list[str | None], PatternError]: ...
def sub(pattern: str | Pattern, repl: str | Callable[Match, str], string: str, count: int = 0, flags: int = 0) -> Result[str, PatternError]: ...
def subn(pattern: str | Pattern, repl: str | Callable[Match, str], string: str, count: int = 0, flags: int = 0) -> Result[Tuple[str, int], PatternError]: ...
```

If `pattern` is already a `Pattern`, helper functions must not recompile it and must reject nonzero `flags` unless this RFC's final design chooses Python's exact error wording. If `pattern` is a `str`, helper functions must compile it with the provided flags and return the compile error as `Err(PatternError)`.

`re.escape(text: str) -> str` must return a string that matches `text` literally when used as a pattern. `re.purge() -> None` may clear any module-level pattern cache if the implementation has one; it must be harmless if there is no cache.

### `Match`

`Match` must expose group access and span information. It should provide Python-like names where they fit Incan:

```incan
def group(self, key: int | str = 0) -> Option[str]: ...
def groups(self, default: str | None = None) -> list[str | None]: ...
def groupdict(self, default: str | None = None) -> dict[str, str | None]: ...
def start(self, key: int | str = 0) -> int: ...
def end(self, key: int | str = 0) -> int: ...
def span(self, key: int | str = 0) -> Tuple[int, int]: ...
```

Group `0` must be the full match. Numbered groups must start at `1`. Named groups must be addressable by name. `group` should return `None` for unmatched optional groups and should return `None` for invalid group references unless the final design chooses Python's exception behavior for invalid references. `start`, `end`, and `span` should follow Python's `-1` / `(-1, -1)` behavior for groups that exist but did not participate, because this is part of the Pythonic match-object contract.

Match offsets must be documented. The preferred Incan contract is to expose offsets in the same unit used by ordinary Incan string slicing. If the engine internally uses byte offsets, the public `std.re` surface must either convert them or clearly document why this Pythonic module differs from Python.

### Replacement templates

String replacements in `std.re` must use Python-style replacement syntax rather than Rust-style `$1` / `${name}` syntax. The module should support numbered references such as `\1`, named references such as `\g<name>`, escaped backslashes, and ordinary literal text. Invalid replacement templates must produce `PatternError` through `Result` rather than panic.

Callable replacements must receive a `Match` for the current match and return the replacement string. The callable replacement path must not interpret the returned string as a replacement template.

### `findall`

`findall` is part of Python's public `re` API, but its return type depends on capture-group shape. `std.re` should include `findall` only after the accepted design chooses a typed representation for that shape. The preferred direction is a tagged result type that can represent whole-match strings, one captured string, or a tuple/list of captured groups without weakening the rest of the module to untyped values.

### Limits and safety

`std.re` must not claim the same safety profile as `std.regex`. The module should support explicit match guardrails when practical. Guardrails may include a maximum backtracking step count, a maximum recursion depth, a maximum input length accepted by a compiled pattern, or a timeout-like budget when the runtime can enforce it deterministically.

If a guardrail is configured and exceeded, matching must fail with a documented runtime error type or a `Result`-returning checked API. The final design must choose whether ordinary `Pattern.search` returns `Option[Match]` or `Result[Option[Match], ReRuntimeError]` when runtime budgets can fail.

### Relationship to `std.regex`

`std.re.Pattern` and `std.regex.Regex` must not be implicitly interchangeable. A library API that accepts `std.regex.Regex` is saying it accepts the safe regex contract. A library API that accepts `std.re.Pattern` is saying it accepts the Pythonic backtracking-capable contract. Any explicit conversion API must preserve that distinction and must fail when converting a Pythonic pattern that requires backtracking-only features into a safe regex pattern.

## Design details

### One shared engine family

The implementation should be structured as one engine family with two public stdlib contracts. This allows shared parser utilities, diagnostics, capture storage, replacement-template parsing, and tests while keeping user-facing APIs separate. A pattern classifier can decide whether a pattern belongs to the safe regular subset or requires the backtracking backend. `std.regex` should accept only the safe subset. `std.re` should accept both the safe subset and the Pythonic backtracking subset.

The phrase "same engine" must not mean "same public behavior." It means the project should avoid duplicating all regex infrastructure when common pieces are real. The public modules still document different capabilities and risks.

### Python compatibility baseline

The compatibility baseline should be CPython's standard `re` module rather than the third-party `regex` package. CPython `re` is the surface users mean when they ask for Pythonic regex, and it gives Incan a bounded target.

The first implementation slice may choose a smaller supported subset, but unsupported Python features must be documented explicitly. The module should prefer a clear `PatternError` over accepting syntax with subtly different semantics.

### Cache behavior

Python's module-level helpers cache compiled patterns as an implementation detail. `std.re` may do the same. If a cache exists, `re.purge()` must clear it. Code must not rely on object identity or cache retention for correctness.

### Bytes and locale behavior

This RFC focuses on `str` patterns and `str` inputs. Python `re` also supports bytes patterns, bytes inputs, and locale-sensitive matching. Those features can be added later, but they should not block the first `std.re` slice. If bytes support is added, `bytes` patterns and `str` patterns must remain separate and must not be mixed in one call.

### Diagnostics and docs

Docs must explain why both modules exist. The recommended summary is: `std.regex` is for predictable matching; `std.re` is for Python portability and expressive patterns. Diagnostics should help users move in either direction: a `std.regex` compile error for lookaround can suggest `std.re` when the feature is intentionally outside the safe subset, while `std.re` docs should suggest `std.regex` for large untrusted inputs when a pattern does not need Pythonic features.

## Alternatives considered

Expanding `std.regex` to accept Pythonic features was rejected because it would erase the safe-default contract established by RFC 059. Users should not need to inspect a pattern's internals to know whether a module has predictable matching semantics.

Adding a `mode="python"` or `engine="backtracking"` argument to `Regex(...)` was rejected because it overloads one type with incompatible promises. It also makes library APIs ambiguous: accepting `Regex` would no longer reveal whether callers may pass a backtracking-capable pattern.

Creating a third-party package instead of `std.re` was rejected for the long-term design because Pythonic regex is central enough to the Python-shaped Incan story. Third-party packages can still explore broader engines such as PCRE2 or the Python `regex` package, but the standard Python-like surface should be stable in stdlib.

Using an existing Rust crate such as a PCRE binding was considered. It may be useful for experimentation, but a direct dependency on a native engine can complicate portability, sandboxing, licensing review, and WASM support. A custom Incan-owned engine family gives the project stronger control over diagnostics, limits, and integration with `Result` and `Option`.

Copying RustPython's `re` implementation wholesale was rejected. RustPython is valuable prior art, but its implementation is tied to Python object representations, CPython library layout, and interpreter behavior. Incan should learn from the SRE-style architecture without inheriting a foreign VM object model.

## Drawbacks

Two regex modules require documentation discipline. Users will ask why both exist, and the answer must be short and consistent: `std.regex` is predictable, `std.re` is Pythonic. If docs hedge, users will see the split as accidental duplication.

`std.re` adds meaningful implementation complexity. Supporting lookaround, pattern backreferences, conditional groups, Python replacement templates, and Python-like match objects requires a backtracking engine, parser, bytecode or equivalent intermediate form, and compatibility tests.

Backtracking regex can be unsafe on adversarial inputs. Even with guardrails, `std.re` will have a risk profile that `std.regex` intentionally avoids. The stdlib must not hide that risk behind a friendly Pythonic name.

Python compatibility also creates pressure to reproduce awkward behavior. APIs such as `findall` have shape-dependent return values, and `Match.start()` uses sentinel values for unmatched groups. Incan should be compatible where it matters, but it should document intentional typed deviations rather than silently producing surprising results.

## Implementation architecture

This section is non-normative. A practical implementation should introduce an Incan-owned regex engine family with a parser, a feature classifier, a safe-regular backend, and a Pythonic backtracking backend. The safe backend can continue to use Rust-regex-style semantics where that remains the right implementation choice, while the Pythonic backend should use an SRE-like intermediate representation that can express lookaround, backreferences, captures, conditional groups, and replacement templates.

RustPython is useful prior art because it separates the Python-facing `re` surface from a low-level SRE engine and describes that engine as "A low-level implementation of Python's SRE regex engine." Incan should adapt the architectural idea rather than the Python object model: compile Pythonic pattern syntax into an internal representation, execute it through a controlled matcher, and surface owned Incan `Pattern`, `Match`, and error values.

The engine family should expose enough metadata for `std.regex` to reject backtracking-only constructs with targeted diagnostics. It should also expose enough budget controls for `std.re` to enforce match limits when the accepted API includes them.

## Layers affected

- **Parser / AST**: no new Incan syntax is required for the first slice, but examples and tooling must accept normal imports such as `import std.re as re`.
- **Typechecker / Symbol resolution**: stdlib module loading must expose the `std.re` module, its aliases, constants, functions, `Pattern`, `Match`, and error types with precise `Result` and `Option` types.
- **IR Lowering**: calls into `std.re` must lower like ordinary stdlib calls and must preserve callable replacement functions without treating them as string templates.
- **Emission**: generated Rust must link the selected engine-family runtime and must keep `std.regex` and `std.re` public types distinct.
- **Stdlib / Runtime (`incan_stdlib`)**: the runtime must provide the Pythonic engine backend, owned pattern and match values, replacement-template expansion, split/substitution helpers, and diagnostics.
- **Formatter**: no new syntax is required, but examples with `std.re` imports, raw strings, and callable replacements should format stably.
- **LSP / Tooling**: completion and hover should distinguish `std.regex` from `std.re`, show pattern/error types, and surface docs that explain safe versus Pythonic matching.
- **Documentation**: stdlib reference docs must explain when to choose `std.regex` and when to choose `std.re`, including the backtracking risk and portability motivation.

## Unresolved questions

- Should the initial `std.re` slice target "Python-inspired" behavior or a named CPython version's `re` behavior closely enough to run imported compatibility tests?
- What should the exact runtime budget API be for backtracking patterns: compile-time options, pattern methods with checked variants, ambient runtime policy, or no budget surface in the first slice?
- Should ordinary `Pattern.search` return `Option[Match]` while budgeted variants return `Result[Option[Match], ReRuntimeError]`, or should all matching APIs be `Result`-returning from the start?
- Should `findall` be included in the first public slice, and if so, what typed representation should model Python's shape-dependent return value?
- Should invalid group references in `Match.group`, `Match.start`, `Match.end`, and `Match.span` return `None`/sentinels, or should they use checked errors to match Python's failure behavior more closely?
- Should `std.re` expose `error` as a lowercase alias for `PatternError`, even though Incan type names are normally capitalized?
- Should bytes patterns and bytes inputs be deferred, or are they necessary for a credible Pythonic regex module?
- Should `LOCALE` be omitted permanently, accepted as a no-op compatibility flag, or implemented only for bytes patterns if bytes support lands?
- Should direct conversion from `std.re.Pattern` to `std.regex.Regex` exist for patterns classified as safe-regular, or should users recompile the pattern explicitly through `std.regex`?

<!-- Rename this section to "Design Decisions" once all questions have been resolved.
     An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
