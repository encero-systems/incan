# RFC 100: `std.re` — Pythonic regular expressions

- **Status:** Planned
- **Created:** 2026-05-18
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 022 (namespaced stdlib modules and compiler handoff)
    - RFC 023 (compilable stdlib and Rust module binding)
    - RFC 059 (`std.regex`)
    - RFC 070 (Result combinators)
- **Issue:** https://github.com/encero-systems/incan/issues/668
- **RFC PR:** —
- **Written against:** v0.3
- **Shipped in:** —

## Summary

This RFC proposes `std.re` as a Pythonic regular-expression module built on the same internal regex engine family as `std.regex` but with a distinct public contract: `std.regex` remains the safe, predictable Rust-regex/RE2-style module, while `std.re` accepts Python-like syntax and behavior that require backtracking semantics, including lookaround and pattern backreferences. The feature gives users an explicit choice between predictable regex matching and Python portability without overloading one API with incompatible safety and compatibility expectations.

## Core model

1. **One engine family, two module contracts:** Incan shares parsing, diagnostics, capture representation, and backend infrastructure where possible, but `std.regex` and `std.re` expose different public promises and execution guarantees.
2. **`std.regex` remains the safe contract:** patterns accepted by `std.regex` must stay inside the predictable regular-feature subset and must reject backtracking-only constructs.
3. **`std.re` is the Pythonic contract:** patterns accepted by `std.re` may use Python-style lookaround, pattern backreferences, conditional subpatterns, Python replacement templates, and module-level helper functions.
4. **Backend selection follows the imported contract:** `std.regex` uses its predictable safe backend, while every `std.re` pattern executes in one instrumented Python-compatible backtracking engine so budget accounting cannot change with an internal safe-subset optimization.
5. **Backtracking risk must be bounded and visible:** `std.re` must document that accepted patterns can be superlinear, apply finite deterministic step and materialized-result budgets by default, and return a typed runtime error when matching exhausts either limit.
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
- Support Python-like module helpers such as `re.compile`, `re.search`, `re.match`, `re.fullmatch`, `re.sub`, `re.subn`, `re.split`, `re.findall`, `re.finditer`, `re.escape`, and `re.purge` through explicit Incan types.
- Support Python-like flags such as `ASCII`, `IGNORECASE`, `MULTILINE`, `DOTALL`, and `VERBOSE`, with common aliases such as `A`, `I`, `M`, `S`, and `X`.
- Preserve Incan's explicit `Result` and `Option` flow for pattern errors and absent matches.
- Define a shared internal engine-family boundary so `std.regex` and `std.re` can share parsing, diagnostics, capture storage, and compilation facts while retaining distinct execution guarantees.
- Provide a compatibility and migration story for users choosing between `std.regex` and `std.re`.

## Non-Goals

- This RFC does not change the semantics of `std.regex`.
- This RFC does not make backtracking regex the default Incan regex behavior.
- This RFC does not require byte-for-byte compatibility with CPython error messages, undocumented implementation behavior, or object identity.
- This RFC does not standardize the third-party Python `regex` package or PCRE2 as Incan's public contract.
- This RFC does not make `std.re.Pattern` and `std.regex.Regex` the same public type.
- This RFC does not require regex literals or new parser-level string syntax.
- This RFC does not include bytes patterns, bytes inputs, or locale-dependent matching. A later RFC may add a separate typed bytes-pattern surface.
- This RFC does not expose CPython's diagnostic-only `DEBUG` flag or print backend matcher internals as a side effect of compilation.
- This RFC does not require a public API for arbitrary engine selection through one overloaded constructor.

## Guide-level explanation

Use `std.regex` when the pattern should be safe and predictable for large inputs. Use `std.re` when the pattern is Python-like and depends on features outside the safe regular subset.

```incan
from std.regex import Regex, RegexError


def main() -> Result[None, RegexError]:
    word = Regex(r"\w+")?
    assert word.is_match("hello")
    return Ok(None)
```

The Pythonic module is imported separately:

```incan
from std.re import ReError
import std.re as re


def main() -> Result[None, ReError]:
    repeated = re.compile(r"(?P<word>\w+)\s+(?P=word)", re.IGNORECASE)?
    match repeated.search("Echo echo")?:
        Some(found) => println(found.group("word")?.unwrap_or(""))
        None => println("no duplicate word")
    return Ok(None)
```

Patterns that are not valid in `std.regex` are valid in `std.re` when they are part of the Pythonic contract:

```incan
from std.re import ReError
import std.re as re


def main() -> Result[None, ReError]:
    preceded = re.compile(r"(?<=\$)\d+(?:\.\d+)?")?
    price = preceded.search("total: $19.95")?
    match price:
        Some(found) => println(found.as_str())
        None => println("no price")
    return Ok(None)
```

The module-level helpers mirror Python's `re` style while still using Incan's error flow. A helper that compiles a pattern must return `Result[...]` because invalid patterns are ordinary recoverable errors in Incan:

```incan
from std.re import ReError
import std.re as re


def normalize(text: str) -> Result[str, ReError]:
    return re.sub(r"\s+", " ", text)?
```

Substitution templates follow Python's backslash-based replacement style. Use a callable replacement when the replacement depends on code:

```incan
from std.re import Match, ReError
import std.re as re


def bracket(found: Match) -> str:
    return f"[{found.as_str()}]"


def main() -> Result[None, ReError]:
    swapped = re.sub(r"(?P<first>\w+)\s+(?P<last>\w+)", r"\g<last>, \g<first>", "Ada Lovelace")?
    assert swapped == "Lovelace, Ada"
    marked = re.sub(r"\w+", bracket, "Ada Lovelace")?
    assert marked == "[Ada] [Lovelace]"
    return Ok(None)
```

The mental model is deliberately simple: `regex` means safe regex; `re` means Pythonic regex. The implementation may share machinery, but users should not have to understand backend selection to choose the right module.

Every `std.re` match uses a finite deterministic step budget. Most code uses the default; callers processing untrusted or unusually expensive input can lower or raise it explicitly and handle exhaustion through `ReError`:

```incan
from std.re import MatchBudget, ReError
import std.re as re


def contains_repeated_word(text: str) -> Result[bool, ReError]:
    pattern = re.compile(r"(?P<word>\w+)\s+(?P=word)", budget=MatchBudget(max_steps=250_000, max_results=10_000))?
    match pattern.search(text)?:
        Some(_) => return Ok(true)
        None => return Ok(false)
```

## Reference-level explanation

### Module boundary

`std.re` must be a separate stdlib module from `std.regex`. The module must not re-export `std.regex.Regex` as its primary pattern type. The primary compiled pattern type must be named `Pattern`, and the primary match type must be named `Match`. The module must expose `MatchBudget`, `FindAll`, and one canonical `ReError` type for every recoverable failure. `PatternError` and lowercase `error` must be public aliases for `ReError` so Python-oriented code can use the familiar name without creating a second error type. Documentation must prefer `ReError` in Incan APIs and explain the compatibility aliases.

`std.regex` must continue to reject backtracking-only constructs. `std.re` may accept those constructs. If both modules share internal code, the shared implementation must not weaken the public guarantees of `std.regex`.

### Pattern compilation

`re.compile(pattern: str, flags: int = 0, budget: MatchBudget = MatchBudget.default()) -> Result[Pattern, ReError]` must compile a Pythonic regex pattern and store the budget used when a matching call does not provide an override. Compilation must return `Err(ReError)` for invalid syntax, unsupported constructs, invalid flags, invalid group references, invalid lookbehind width, and an invalid budget. Replacement templates are compiled by substitution calls rather than by `compile`, and invalid templates must likewise return `Err(ReError)`.

`ReError` must expose a stable `kind() -> str` category, a human-readable `message() -> str`, and optional source-location metadata through `pattern() -> Option[str]`, `position() -> Option[int]`, `line() -> Option[int]`, and `column() -> Option[int]`. The metadata methods are the typed equivalents of CPython `PatternError.pattern`, `pos`, `lineno`, and `colno`; `message()` maps `PatternError.msg`. Pattern and replacement-template failures must populate every value the parser can determine, while runtime failures may return `None` for source-location fields that do not apply. The required kinds are `invalid_pattern`, `invalid_flag`, `invalid_template`, `invalid_group`, `invalid_range`, `invalid_budget`, `budget_exceeded`, and `result_limit_exceeded`. Budget failures must report the configured limit.

### Pattern syntax

`std.re` must support the safe regular subset already available through `std.regex`, including literals, character classes, quantifiers, alternation, grouping, anchors, indexed captures, named captures, inline flags, and Unicode-aware string matching.

`std.re` must additionally support the CPython 3.14 `re` string-pattern contract, including positive lookahead `(?=...)`, negative lookahead `(?!...)`, fixed-width positive lookbehind `(?<=...)`, fixed-width negative lookbehind `(?<!...)`, numbered pattern backreferences such as `\1`, named pattern backreferences such as `(?P=name)`, non-capturing groups `(?:...)`, comments `(?#...)`, named captures `(?P<name>...)`, conditional subpatterns `(?(id/name)yes|no)`, greedy and lazy quantifiers, and atomic groups. Unsupported CPython 3.14 syntax must fail with `ReError(kind="invalid_pattern")`; it must not be accepted with different semantics.

Lookbehind must require statically fixed width unless a later RFC explicitly chooses a different compatibility target. Backreferences must refer to existing capturing groups and must be rejected when their target is invalid or semantically impossible under the accepted baseline.

### Flags

`std.re` must expose `NOFLAG`, `ASCII`, `IGNORECASE`, `MULTILINE`, `DOTALL`, `VERBOSE`, and `UNICODE`, plus aliases `A`, `I`, `M`, `S`, `X`, and `U`. `UNICODE` must be accepted as a no-op compatibility flag for string patterns because Unicode matching is already the default. `LOCALE`, `L`, and `DEBUG` must not be exposed; passing their CPython numeric bits must return `ReError(kind="invalid_flag")` rather than silently acting as a no-op or printing backend internals. A future bytes-pattern RFC may define locale behavior under a separate typed contract.

Flags must compose with bitwise OR. APIs accepting `flags` must reject unknown bits and combinations that the module does not support.

### Pattern methods

`Pattern` must expose at least these methods:

```incan
def match(self, string: str, pos: int = 0, endpos: int | None = None, budget: MatchBudget | None = None) -> Result[Option[Match], ReError]: ...
def fullmatch(self, string: str, pos: int = 0, endpos: int | None = None, budget: MatchBudget | None = None) -> Result[Option[Match], ReError]: ...
def search(self, string: str, pos: int = 0, endpos: int | None = None, budget: MatchBudget | None = None) -> Result[Option[Match], ReError]: ...
def finditer(self, string: str, pos: int = 0, endpos: int | None = None, budget: MatchBudget | None = None) -> Result[Iterator[Match], ReError]: ...
def findall(self, string: str, pos: int = 0, endpos: int | None = None, budget: MatchBudget | None = None) -> Result[FindAll, ReError]: ...
def split(self, string: str, maxsplit: int = 0, budget: MatchBudget | None = None) -> Result[list[str | None], ReError]: ...
def sub(self, repl: str | Callable[Match, str], string: str, count: int = 0, budget: MatchBudget | None = None) -> Result[str, ReError]: ...
def subn(self, repl: str | Callable[Match, str], string: str, count: int = 0, budget: MatchBudget | None = None) -> Result[Tuple[str, int], ReError]: ...
def source(self) -> str: ...
def flags(self) -> int: ...
def group_count(self) -> int: ...
def groupindex(self) -> dict[str, int]: ...
def budget(self) -> MatchBudget: ...
```

`match` must attempt a match at `pos`. `search` must scan from `pos` through `endpos`. `fullmatch` must require the whole selected range to match. `finditer` must yield non-overlapping matches from left to right and must make progress after empty matches. `pos` and `endpos` are Unicode-scalar indices; `None` means the end of the string. Invalid ranges must return `ReError(kind="invalid_range")`.

`split` must follow Python's important captured-separator behavior: if the separator pattern contains capturing groups, captured separator text must appear in the returned list, and unmatched optional separator groups must appear as `None`. If the pattern contains no capturing groups, only the split fields should appear.

`sub` must return the substituted string. `subn` must return the substituted string and the number of substitutions performed. A `count` of `0` must mean no replacement limit, matching Python's convention.

`source`, `flags`, `group_count`, `groupindex`, and `budget` provide the typed equivalents of CPython's `Pattern.pattern`, `Pattern.flags`, `Pattern.groups`, and `Pattern.groupindex` metadata plus Incan's explicit budget. `flags` must include compile-time, inline, and implicit Unicode flags. These immutable values support diagnostics and explicit migration without making patterns from the two regex modules interchangeable.

### Module helper functions

The module helper functions must compile the pattern and then delegate to the corresponding `Pattern` method. Their signatures must follow this shape:

```incan
def match(pattern: str | Pattern, string: str, flags: int = 0, budget: MatchBudget | None = None) -> Result[Option[Match], ReError]: ...
def fullmatch(pattern: str | Pattern, string: str, flags: int = 0, budget: MatchBudget | None = None) -> Result[Option[Match], ReError]: ...
def search(pattern: str | Pattern, string: str, flags: int = 0, budget: MatchBudget | None = None) -> Result[Option[Match], ReError]: ...
def finditer(pattern: str | Pattern, string: str, flags: int = 0, budget: MatchBudget | None = None) -> Result[Iterator[Match], ReError]: ...
def findall(pattern: str | Pattern, string: str, flags: int = 0, budget: MatchBudget | None = None) -> Result[FindAll, ReError]: ...
def split(pattern: str | Pattern, string: str, maxsplit: int = 0, flags: int = 0, budget: MatchBudget | None = None) -> Result[list[str | None], ReError]: ...
def sub(pattern: str | Pattern, repl: str | Callable[Match, str], string: str, count: int = 0, flags: int = 0, budget: MatchBudget | None = None) -> Result[str, ReError]: ...
def subn(pattern: str | Pattern, repl: str | Callable[Match, str], string: str, count: int = 0, flags: int = 0, budget: MatchBudget | None = None) -> Result[Tuple[str, int], ReError]: ...
```

If `pattern` is already a `Pattern`, helper functions must not recompile it and must return `ReError(kind="invalid_flag")` when `flags` is nonzero. A provided budget overrides the pattern's stored default for that call. If `pattern` is a `str`, helper functions must compile it with the provided flags and either the provided budget or `MatchBudget.default()`, then return every failure through `ReError`.

`re.escape(text: str) -> str` must return a string that matches `text` literally when used as a pattern. `re.purge() -> None` may clear any module-level pattern cache if the implementation has one; it must be harmless if there is no cache.

### `Match`

`Match` must expose group access and span information through these Python-like names:

```incan
def group(self, key: int | str = 0) -> Result[Option[str], ReError]: ...
def group_many(self, keys: list[int | str]) -> Result[list[Option[str]], ReError]: ...
def groups(self, default: str | None = None) -> list[str | None]: ...
def groupdict(self, default: str | None = None) -> dict[str, str | None]: ...
def start(self, key: int | str = 0) -> Result[int, ReError]: ...
def end(self, key: int | str = 0) -> Result[int, ReError]: ...
def span(self, key: int | str = 0) -> Result[Tuple[int, int], ReError]: ...
def expand(self, template: str) -> Result[str, ReError]: ...
def as_str(self) -> str: ...
def pos(self) -> int: ...
def endpos(self) -> int: ...
def lastindex(self) -> Option[int]: ...
def lastgroup(self) -> Option[str]: ...
def pattern(self) -> Pattern: ...
def string(self) -> str: ...
```

Group `0` must be the full match. Numbered groups must start at `1`. Named groups must be addressable by name. A group that exists but did not participate must produce `Ok(None)` from `group`, `Ok(-1)` from `start` and `end`, and `Ok((-1, -1))` from `span`. A negative, out-of-range, or unknown group reference must instead return `ReError(kind="invalid_group")`; absence and caller error must not be conflated.

`group_many` is the typed equivalent of CPython's variadic `Match.group`: it returns one list slot per requested key and preserves unmatched groups as `None`. `std.re.Match` does not overload indexing because an index hook cannot expose the checked `ReError` contract as clearly as `group`. `expand` must apply the same replacement-template grammar as `sub`, with unmatched groups rendered as empty strings to match CPython 3.14. `as_str` returns group `0` without a fallible lookup.

`pos`, `endpos`, `lastindex`, `lastgroup`, `pattern`, and `string` must preserve the corresponding successful-match metadata. Public values are owned rather than backend-borrowed, but match snapshots from one call must share immutable input and pattern storage internally so `finditer` does not copy the complete source string into every result.

Together, the required `Pattern` and `Match` methods cover CPython 3.14's documented string-pattern object surface. Incan intentionally expresses Python attributes as methods, names `Pattern.pattern` as `source`, names `Pattern.groups` as `group_count`, replaces variadic `Match.group` with `group_many`, and uses checked `group` instead of `Match.__getitem__`. These are typed spelling changes, not omissions of the underlying information or behavior. Python's always-true match-object truthiness and copy/deepcopy identity rules do not require separate Incan APIs because successful matches already arrive as `Some(Match)` and public values are owned.

Match offsets must use Unicode-scalar indices, the unit used by ordinary Incan string indexing and slicing. The runtime must convert backend byte offsets before constructing public `Match` values. This intentionally differs from the current `std.regex` byte-offset surface in favor of Python-compatible string indexing.

### Replacement templates

String replacements in `std.re` must use Python-style replacement syntax rather than Rust-style `$1` / `${name}` syntax. The module must support numbered references such as `\1`, named references such as `\g<name>`, escaped backslashes, and ordinary literal text. Invalid replacement templates must produce `ReError(kind="invalid_template")` rather than panic.

Callable replacements must receive a `Match` for the current match and return the replacement string. The callable replacement path must not interpret the returned string as a replacement template.

### `findall`

`findall` is part of the required public surface. It must return one payload-bearing enum value whose variant records the pattern's fixed capture shape:

```incan
pub enum FindAll:
    WholeMatches(list[str])
    SingleCapture(list[Option[str]])
    CaptureGroups(list[list[Option[str]]])
```

`WholeMatches` is used when the pattern has no capturing groups, `SingleCapture` when it has exactly one, and `CaptureGroups` when it has two or more. Each inner list in `CaptureGroups` must have the pattern's capture count and group order. Unlike CPython's empty-string substitution for an unmatched `findall` capture, Incan must preserve that absence as `None`.

### Limits and safety

`std.re` must not claim the same safety profile as `std.regex`. It must expose `MatchBudget(max_steps: int, max_results: int)` and `MatchBudget.default()`. The defaults must be `10_000_000` matcher steps and `100_000` materialized result slots. Both limits must be positive; invalid budgets must return `ReError(kind="invalid_budget")` whether supplied during compilation or as a call override.

A matcher step is one dispatch of the `std.re` matching engine's compiled instruction stream, including failed transitions, candidate-start advances, zero-width assertion work, and backtracked retries. The counter resets for each top-level API call and is shared across all candidate start positions, iterator results, splits, or substitutions produced by that call. Step totals are deterministic for the same Incan release, pattern, flags, and input; they are a safety ceiling rather than a cross-release performance metric, so compiler/runtime optimizations may change the count in a later release.

Every matching API must return `Result` from the first public release. Exceeding `max_steps` must return `ReError(kind="budget_exceeded")`, and exceeding `max_results` while producing `finditer`, `findall`, or captured split output must return `ReError(kind="result_limit_exceeded")`. One `finditer` match, one whole/single-capture `findall` item, each captured value inside a multi-capture `findall` item, and each returned split field or capture consume one result slot. `finditer` must eagerly snapshot its bounded results before returning the ordinary Incan iterator, matching the ownership pattern already used by `std.regex`; no partial iterator or collection may escape. Substitution remains subject to `max_steps`; replacement allocation follows the ordinary runtime memory contract because callable output size is application behavior rather than matcher result cardinality. Wall-clock timeouts and ambient runtime policy are not part of this contract because they are nondeterministic and hide cost from the call site.

### Relationship to `std.regex`

`std.re.Pattern` and `std.regex.Regex` must not be implicitly interchangeable. A library API that accepts `std.regex.Regex` is saying it accepts the safe regex contract. A library API that accepts `std.re.Pattern` is saying it accepts the Pythonic backtracking-capable contract. `std.re` must not provide a direct conversion API, even for patterns classified as safe-regular. Users who want the safe contract must explicitly compile the original pattern text and mapped flags through `std.regex.Regex`, making the contract change visible and allowing `std.regex` to validate it independently.

## Design details

### One shared engine family

The implementation must be structured as one engine family with two public stdlib contracts. This allows shared parser utilities, diagnostics, capture storage, replacement-template parsing, compatibility tests, and safe-subset classification while keeping user-facing APIs and execution guarantees separate. `std.regex` must accept only the safe subset and use its predictable safe backend. `std.re` must accept both subsets but execute every accepted pattern through one instrumented Python-compatible backtracking engine, including patterns classified as safe-regular. `std.re` must not silently dispatch some patterns to an uninstrumented DFA or another backend with different step accounting.

The phrase "same engine" must not mean "same public behavior." It means the project should avoid duplicating all regex infrastructure when common pieces are real. The public modules still document different capabilities and risks.

### Python compatibility baseline

The compatibility baseline is the documented string-pattern behavior of CPython 3.14's standard `re` module rather than the third-party `regex` package. It covers the full documented pattern grammar; the module helpers named in this RFC; the compiled-pattern operations and metadata mapped above; match expansion, groups, spans, and metadata; flags listed by this RFC; and replacement-template behavior. CPython `re` is the surface users mean when they ask for Pythonic regex, and naming 3.14 gives compatibility tests and documentation a bounded target.

The required implementation must pass an imported compatibility corpus for that pattern, flag, match, split, substitution, metadata, and replacement-template surface. Deliberate typed deviations defined by this RFC take precedence: recoverable failures use `Result[..., ReError]`, absent captures use `Option`, `findall` uses `FindAll`, Python attributes map to explicit methods, multi-group access uses `group_many`, flags remain an `int` bitmask rather than a dynamic `RegexFlag` object, `finditer` returns a bounded snapshot iterator, runtime matching is budgeted, and bytes/locale/diagnostic `DEBUG` behavior are excluded. Deprecated aliases beyond `error`, undocumented helpers, and CPython object-identity details are not compatibility requirements. Unsupported baseline features within the committed surface must return a clear `ReError`; the module must not accept syntax with subtly different semantics. A later compatibility-baseline update requires an RFC amendment because CPython may add syntax or change edge behavior.

### Cache behavior

Python's module-level helpers cache compiled patterns as an implementation detail. `std.re` may do the same. A cache key must include pattern source, flags, and the stored match budget so cached patterns cannot silently acquire another call's policy. If a cache exists, `re.purge()` must clear it. Code must not rely on object identity or cache retention for correctness.

### Bytes and locale behavior

This RFC standardizes only `str` patterns and `str` inputs. Python's bytes-pattern surface is deliberately deferred because preserving static type separation requires its own `BytesPattern`-style contract rather than a runtime-typed `Pattern`. A later RFC may add that surface, but bytes and string patterns must never be mixed in one call.

`LOCALE` is excluded from the string API rather than accepted as a no-op. Locale-sensitive matching depends on ambient process state and is meaningful in CPython only for bytes patterns; exposing it here would undermine deterministic matching. Any future locale support must arrive with the bytes-pattern design and define its locale source and reproducibility contract explicitly.

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

This section is non-normative except where it restates the execution guarantees above. A practical implementation should introduce an Incan-owned regex engine family with a parser, a feature classifier, a safe-regular backend for `std.regex`, and an instrumented Python-compatible backtracking engine for all `std.re` execution. The backtracking engine should use an SRE-like intermediate representation that can express lookaround, backreferences, captures, conditional groups, replacement templates, and deterministic instruction counting.

RustPython is useful prior art because it separates the Python-facing `re` surface from a low-level SRE engine and describes that engine as "A low-level implementation of Python's SRE regex engine." Incan should adapt the architectural idea rather than the Python object model: compile Pythonic pattern syntax into an internal representation, execute it through a controlled matcher, and surface owned Incan `Pattern`, `Match`, and error values.

The engine family must expose enough metadata for `std.regex` to reject backtracking-only constructs with targeted diagnostics and enough instrumentation for `std.re` to enforce the specified step budget.

## Layers affected

- **Typechecker / Symbol resolution**: stdlib module loading must expose the `std.re` module, its aliases, constants, functions, `Pattern`, `Match`, and error types with precise `Result` and `Option` types.
- **IR Lowering**: calls into `std.re` must lower like ordinary stdlib calls and must preserve callable replacement functions without treating them as string templates.
- **Emission**: generated Rust must link the selected engine-family runtime and must keep `std.regex` and `std.re` public types distinct.
- **Stdlib / Runtime (`incan_stdlib`)**: the runtime must provide the Pythonic engine backend, owned pattern and match values, replacement-template expansion, split/substitution helpers, and diagnostics.
- **LSP / Tooling**: completion and hover should distinguish `std.regex` from `std.re`, show pattern/error types, and surface docs that explain safe versus Pythonic matching.
- **Documentation**: stdlib reference docs must explain when to choose `std.regex` and when to choose `std.re`, including the backtracking risk and portability motivation.

## Design Decisions

1. **Compatibility has a named baseline.** The required string-pattern surface targets the documented CPython 3.14 `re` contract and an imported compatibility corpus. Incan's specified `Result[..., ReError]`, `Option`, `FindAll`, budget, and bytes/locale deviations take precedence over Python's dynamic or exception-based shapes.
2. **Every match has deterministic work and materialization budgets.** `MatchBudget` stores positive `max_steps` and `max_results` limits, defaults to `10_000_000` and `100_000`, and can be stored at compilation or overridden per call. Every `std.re` pattern uses the same instrumented Python-compatible backtracking engine, including safe-regular patterns, so step accounting does not switch with backend selection. The API does not use wall-clock timeouts or ambient policy.
3. **One error type covers the complete module.** Every recoverable failure uses `ReError`, and `PatternError` plus lowercase `error` are aliases rather than distinct types. This keeps `?` propagation valid in current Incan and avoids unsupported `Result[T, E1 | E2]` pseudotypes. An absent match remains `Ok(None)`; invalid patterns, templates, flags, groups, ranges, budgets, exhausted steps, and exhausted result limits use stable `ReError.kind()` categories. `finditer` eagerly snapshots at most `max_results` matches before its iterator escapes, so a failure cannot leave a partial result.
4. **`findall` is included through one typed tagged result.** `FindAll.WholeMatches`, `FindAll.SingleCapture`, and `FindAll.CaptureGroups` preserve the pattern's fixed capture shape, and unmatched captures stay `None`.
5. **Invalid group references are checked errors.** Existing but unmatched groups retain Python's `None` and `-1` sentinels inside `Ok`; negative, out-of-range, or unknown references return `ReError(kind="invalid_group")`.
6. **`ReError` is canonical and Python names are aliases.** `PatternError` and lowercase `error` exist for compatibility, while Incan documentation and new code use `ReError`.
7. **The RFC is string-only.** Bytes patterns and inputs require a separate typed surface and are deferred to a later RFC; they must not be smuggled through the string `Pattern` API.
8. **`LOCALE` is excluded rather than ignored.** The string surface does not expose `LOCALE` or `L`, and the corresponding numeric bit is rejected. A future bytes RFC must define locale sourcing and determinism before adding it.
9. **Changing to the safe regex contract requires explicit recompilation.** There is no direct `std.re.Pattern` to `std.regex.Regex` conversion. Users recompile original source and mapped flags through `std.regex`, which independently rejects unsupported syntax.
