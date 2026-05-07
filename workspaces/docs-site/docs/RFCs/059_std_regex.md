# RFC 059: `std.regex` — regular expressions, matches, captures, and replacement

- **Status:** Planned
- **Created:** 2026-04-14
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 022 (namespaced stdlib modules and compiler handoff)
    - RFC 023 (compilable stdlib and Rust module binding)
- **Issue:** https://github.com/dannys-code-corner/incan/issues/294
- **RFC PR:** —
- **Written against:** v0.2
- **Shipped in:** —

## Summary

This RFC proposes `std.regex` as Incan's standard regular-expression module. It provides a compiled `Regex` type together with explicit `Match` and `Captures` result types, plus the normal search, capture, split, and replacement operations that text-processing code expects. The core stdlib contract intentionally follows a predictable Rust-regex/RE2-style engine model rather than a fully backtracking Perl-like engine: the standard surface should be strong enough for real analytics, cleanup, and tooling work while preserving predictable performance and avoiding catastrophic-regex footguns in the default engine.

## Motivation

Regex support is a practical stdlib need for CLIs, data cleaning, text extraction, log processing, source transformation, and general systems glue. Without a standard regex module, users either hand-roll brittle string logic or fall through to Rust-specific APIs that leak substrate vocabulary and semantics into ordinary Incan code.

This matters because regex is not just a convenience:

- text-oriented tooling needs pattern search and replacement constantly;
- parsing-lite tasks often do not justify a full parser;
- migration and cleanup work depends on predictable capture and replacement behavior;
- analytics pipelines frequently need repeated scanning and extraction over large datasets, where pathological regex behavior is not acceptable as the default.

`std.regex` should therefore exist as a real module, not as an implicit "use Rust if you need it" footnote.

## Goals

- Provide a standard compiled `Regex` type.
- Support search, full-match, capture groups, splitting, and replacement.
- Define an Incan-owned API contract rather than exposing Rust engine types directly.
- Keep the core stdlib engine predictable enough that regex is safe as a default tool in analytics and text-processing code.
- Leave room for stronger, backtracking-style regex support in separate packages when users truly need advanced syntax such as lookaround or backreferences.

## Non-Goals

- Standardizing every possible regex engine feature in core stdlib.
- Committing to shell-glob syntax or wildcard matching in this RFC.
- Solving parser-level language grammars with regex alone.
- Guaranteeing cross-engine portability with non-stdlib regex implementations.
- Making advanced backtracking regex the default `std.regex` behavior.

## Guide-level explanation

Authors should be able to compile a regex once and reuse it for matching and extraction.

```incan
from std.regex import Regex

version_re = Regex(r"^v(?P<major>\d+)\.(?P<minor>\d+)$")?
caps = version_re.captures("v0.2")?

if caps.is_some():
    captured = caps.unwrap()
    println(captured.group("major")?)
```

Scanning through text should be straightforward too.

```incan
from std.regex import Regex

word_re = Regex(r"\w+")?

for m in word_re.find_iter(text):
    println(f"{m.start()}:{m.end()} => {m.as_str()}")
```

Replacement should support both simple strings and programmable replacements.

```incan
from std.regex import Regex

space_re = Regex(r"\s+")?
cleaned = space_re.replace_all(text, " ")
```

```incan
from std.regex import Regex

name_re = Regex(r"(?P<first>\w+)\s+(?P<last>\w+)")?
reversed = name_re.replace_all(text, fn(caps) -> str:
    f"{caps.group('last')?}, {caps.group('first')?}"
)
```

The mental model is:

- `Regex` is compiled and reusable;
- `Match` is for one match span;
- `Captures` is for one capture-bearing match;
- the core engine is the safe, predictable regex engine;
- stronger "fancy regex" can live in packages outside stdlib.

## Reference-level explanation

### Module scope

`std.regex` must provide:

- a compiled `Regex` type;
- a `Match` result type;
- a `Captures` result type;
- pattern compilation errors;
- search, capture, split, and replacement operations over `str`.

### Core engine contract

The core stdlib contract follows the predictable Rust-regex/RE2-style family rather than a backtracking Perl-like engine.

That means the core contract should support:

- ordinary literals, character classes, quantifiers, alternation, grouping, and anchors;
- indexed capture groups;
- named capture groups;
- standard regex flags such as multiline, dotall, ignore-case, and verbose modes where the chosen syntax supports them;
- Unicode-aware matching by default.

And the core contract should explicitly exclude:

- lookaround;
- backreferences inside patterns;
- other features that would force the default engine into full backtracking semantics.

This boundary is deliberate. The stdlib default should be predictable and safe for large-scale analytics and tooling work. More expressive backtracking regex can exist later in separate packages.

### Search and capture surface

The module must make the difference explicit between:

- finding a match anywhere in the input;
- checking whether the entire input matches;
- retrieving capture groups from a match.

The committed `Regex` surface includes:

- `is_match(text: str) -> bool`
- `find(text: str) -> Option[Match]`
- `find_iter(text: str) -> Iterator[Match]`
- `captures(text: str) -> Option[Captures]`
- `captures_iter(text: str) -> Iterator[Captures]`
- `full_match(text: str) -> Option[Captures]`
- `split(text: str) -> Iterator[str]`
- `splitn(text: str, limit: int) -> Iterator[str]`

Iteration order is normative:

- `find_iter` and `captures_iter` iterate left-to-right
- matches are non-overlapping

### `Match` and `Captures`

`Match` and `Captures` are separate public types because a plain match span and a capture-bearing result are different abstractions.

#### `Match`

`Match` represents one match span and must expose at least:

- `as_str() -> str`
- `start() -> int`
- `end() -> int`
- `span() -> Tuple[int, int]` or equivalent

#### `Captures`

`Captures` represents one successful match together with capture groups and must expose at least:

- access to the full match
- indexed capture lookup
- named capture lookup
- span information for captures where meaningful

The group contract is:

- group `0` is the full match;
- indexed groups are available by number;
- named groups are available by name;
- missing groups must surface absence explicitly rather than silently returning empty strings.

The convenience surface should go beyond one-at-a-time lookup:

- `group(...)` returns one capture by index or name;
- `span(...)` returns the span for one capture by index or name;
- `groups()` returns the indexed capture values as an iterable collection;
- `groupdict()` returns the named capture values as a mapping.

Those bulk views must keep unmatched optional groups explicit. They must not silently coerce absence into empty strings.

### Replacement surface

The replacement surface is complete, not minimal:

- `replace(text: str, repl: Replacement) -> str`
- `replace_all(text: str, repl: Replacement) -> str`
- `replacen(text: str, limit: int, repl: Replacement) -> str`

`Replacement` is a conceptual union of:

- a literal replacement string;
- a capture-aware replacement string;
- a callable that receives `Captures` and returns a replacement string.

The replacement-string syntax should be standardized as part of the Incan contract. A coherent default is Rust-style replacement references such as `$1` and `${name}`, but the key requirement is that the syntax must be language-owned and documented rather than delegated to backend accidents.

### Flags and modifiers

The standard library should support both:

- inline pattern flags, for patterns that travel as self-contained literals;
- constructor or builder options, for callers that want configuration separate from the pattern text.

That dual surface is justified because both are normal regex usage patterns. Inline flags make patterns portable and self-describing; constructor options make it easier to keep the regex text itself data-like when configuration is controlled by the surrounding program.

The supported flag set should align with the chosen safe engine model and include the normal modifiers such as:

- ignore case
- multiline
- dot matches newline
- verbose mode

But the exact supported set is part of the Incan contract, not "whatever the backend happens to allow."

The public construction surface should stay direct and simple:

- `Regex(pattern)?` for the ordinary case;
- `Regex(pattern, ignore_case=True, multiline=True, dotall=True, verbose=True)?` or equivalent keyword-driven options for configured construction.

A separate public builder object is not necessary in the Incan-facing contract. Direct construction plus keyword options is the better DX default here.

### Long-run stronger regex support

The stdlib contract deliberately leaves room for stronger regex syntax outside core stdlib. If Incan later wants lookaround, backreferences, or other backtracking features, that should arrive as a separate package track rather than being silently folded into `std.regex`.

That future story might look like:

```incan
from pub::fancy_regex @ 0.1 import Regex
```

The standard library default and the stronger package are different tools for different constraints. The existence of the latter must not weaken the predictability contract of the former.

## Design details

### Why regex deserves its own module

Regex is a substantial text-processing abstraction, not just a handful of string helpers. A dedicated module keeps the surface discoverable, teaches compilation and reuse explicitly, and gives the language one place to define pattern, match, capture, and replacement semantics.

### Why the default engine should be the predictable one

Python-style or Perl-style regex engines are more expressive, but they also bring backtracking semantics and the risk of pathological runtime behavior. That trade is acceptable sometimes, but it is not the right default for a language stdlib that wants regex to be normal in analytics pipelines, tooling, and large-scale text processing.

The safe-default / fancy-opt-in split is a better long-run design:

- `std.regex` is the predictable default;
- stronger regex semantics can live in a separate package with clearly different guarantees.

### Rust-backed but Incan-owned

Rust provides an excellent predictable regex substrate, but the Incan stdlib should not simply inherit Rust type names and method names wholesale. The RFC defines the user-facing contract first and then uses Rust's implementation strength underneath where that remains compatible with Incan's needs.

### Interaction with existing features

- String operations remain useful for literal and structural text work; regex is the pattern-matching companion, not a replacement.
- Future JSON, CSV, and tooling RFCs may rely on `std.regex` for validation or extraction helpers in examples and docs.
- Rust interop remains available for advanced engine-specific features the stdlib does not standardize.
- Package imports make it straightforward to add stronger regex engines later without weakening the stdlib contract.

### Compatibility / migration

This feature is additive. Existing string-manipulation code keeps working, but regex-heavy code should no longer require Rust escape hatches once the stdlib surface exists.

## Alternatives considered

1. **Only string helpers**
   - Too weak for real extraction and replacement tasks.

2. **Rust interop only**
   - Too implementation-shaped and poor for beginner-facing docs.

3. **Make backtracking/fancy regex the stdlib default**
   - More expressive, but worse as a safe default for analytics, tooling, and large text-processing workloads.

4. **Expose only one combined result type instead of `Match` and `Captures`**
   - Too muddy. Plain span matches and capture-bearing matches are not the same abstraction.

## Drawbacks

- The safe default engine will reject some advanced patterns users may expect from Python or PCRE-like regex flavors.
- Supporting a stronger package later may create a two-tier regex story users need to understand.
- Match and capture APIs still need careful design to avoid awkward optionality or indexing traps.

## Layers affected

- **Stdlib / runtime**: must provide the regex types, errors, and documented matching and replacement behavior.
- **Language surface**: the module and result types must be available as specified.
- **Execution handoff**: implementations must preserve the chosen matching and replacement semantics without leaking backend-specific quirks.
- **Docs / tooling**: should standardize regex examples, error reporting expectations, and the distinction between stdlib-safe regex and any later fancy-regex package.

## Design Decisions

- `std.regex` uses the predictable Rust-regex/RE2-style engine model as its core contract.
- The core stdlib regex surface explicitly excludes lookaround and backreferences.
- Stronger backtracking regex support belongs in separate packages, not the core stdlib.
- The core contract includes both named and indexed capture groups.
- The core contract includes both `find_iter(...)` and `captures_iter(...)`.
- Replacement supports literal strings, capture-aware replacement strings, and callable replacements.
- The replacement-string interpolation syntax follows the Rust-style `$1` / `${name}` family.
- The stdlib supports both inline flags and constructor-configured flags.
- Constructor-configured flags use direct `Regex(pattern, ...)` construction with keyword options rather than a separate public builder object.
- `Match` and `Captures` are separate public types.
- `Captures` exposes `group(...)`, `span(...)`, `groups()`, and `groupdict()`, and the bulk views preserve unmatched groups as explicit absence.
