# RFC 175: Bidirectional controls and confusable identifiers

- **Status:** Planned
- **Created:** 2026-11-10
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 027 (library vocabulary registration)
    - RFC 117 (`loaf.toml` and the Oven project model)
    - RFC 160 (vocabulary in Incan and native desugarers)
- **Issue:** [#2746](https://github.com/encero-systems/incan/issues/2746)
- **RFC PR:** —
- **Written against:** v0.6
- **Shipped in:** —

## Summary

Source is refused when it contains a Unicode bidirectional control character anywhere, in code, comments, strings and docstrings alike, because such a character makes source display differently from how it compiles. A string that needs one spells it with a new `\u{…}` escape. Two identifiers in one scope that look alike under Unicode's confusable mappings produce a warning. Each diagnostic has a code, points at the exact character, and gives a hint naming the fix. The formatter, the LSP and Oven apply the same rules as the compiler.

## Motivation

A bidirectional control reorders how text displays without changing what it is. In source, that lets a reviewer read one program while the compiler builds another: an override inside a comment or string can make the end of a line appear to close it, or move code into what looks like a comment. Editors and code review tools render the reordered text, so reading the source carefully doesn't help.

Look-alike names do the same with less effort. Incan identifiers are ASCII, but ASCII has its own look-alikes: `rn` beside `m`, `l` beside `I` and `1`, `O` beside `0`. A module that declares `admin` and imports `adrnin` gives a reader two names that read as one.

Neither belongs in Incan source, and the defense works only if every tool that shows or builds source enforces the same rule.

## Guide-level explanation

A bidirectional control is refused wherever it appears:

```incan
# ⟪RLO⟫ stands for U+202E RIGHT-TO-LEFT OVERRIDE, which this page does not show as the character itself.
is_admin = False  # ⟪RLO⟫ admins only
```

```text
error: bidirectional control character in source
 --> access.incn:2:21
  |
2 | is_admin = False  # \u{202e} admins only
  |                     ^^^^^^^^ U+202E RIGHT-TO-LEFT OVERRIDE
  |
help: delete it; a comment cannot hold a bidirectional control
```

A string that needs one writes it as an escape, so the source shows exactly what the value holds:

```incan
HEBREW_LABEL = "\u{200f}שלום"    # U+200F RIGHT-TO-LEFT MARK, then the text
```

Look-alike names in one scope warn:

```incan
from accounts import adrnin

def admin() -> None:    # warning: `admin` looks like `adrnin`, imported on line 1
    ...
```

## Reference-level explanation

### Bidirectional controls

- A bidirectional control is a code point with the Unicode property `Bidi_Control`: U+061C, U+200E, U+200F, U+202A to U+202E, and U+2066 to U+2069.
- Source that holds one is refused, wherever it appears: in code, in a comment, in any string literal or f-string, and in a docstring.
- A `bytes` literal already admits only ASCII, so it cannot hold one; its `\x` escapes write bytes, not characters.

### The `\u{…}` escape

- In `str` literals, f-strings and docstrings, `\u{` followed by one to six hexadecimal digits and `}` is the Unicode scalar value those digits name. Digits may be upper or lower case.
- `\u{` followed by anything else, or by digits naming a surrogate or a value above U+10FFFF, is refused.
- `\u` not followed by `{` is not an escape, as for any other backslash today.
- The escape is general: it spells any scalar value, not only the controls.

### Confusable identifiers

- Identifiers stay ASCII: a letter or `_`, then letters, digits and `_`.
- Two different identifiers in one scope are confusable when their skeletons under Unicode Technical Standard #39 are equal. A scope here is a module's names, including its imports; a function's parameters and locals; or a type's members.
- A confusable pair is a warning, reported at the later of the two, with the other labeled.
- The warning never stops a build. Code that must be free of it checks for its code in `incan check`'s machine-readable output.

### Diagnostics

- Each diagnostic has its own code: the refused control, the malformed escape and the confusable pair.
- A diagnostic points at the exact character and names its code point and Unicode name.
- The hint names the fix:
    - in a string, the escape that spells the character, such as `\u{202e}`;
    - in a comment or in code, deleting it;
    - for a confusable pair, renaming one of the two names.
- A diagnostic never prints a control itself. Wherever it quotes source, it shows the escape.

### Display

Every tool that shows source or a string value shows a bidirectional control as its escape, never as the character. That covers diagnostics, hover, signature help, checked API documentation and inspection output. A docstring whose value holds a control through an escape therefore displays the escape, not the reordered text.

### Tools

- **Formatter:** refuses a file that holds a bidirectional control, with the compiler's diagnostic, and never rewrites or removes the character itself.
- **LSP:** reports the same diagnostics as the compiler. The fix in each hint is offered as a code action.
- **Oven:** applies the refusal to every source file it compiles, in every facet of a Loaf and in its dependencies, including Rust and C sources. A Rust or C literal that needs a control uses that language's own escape. The diagnostic has the same code and names the file and the facet.

### Rust interop names

An Incan identifier is ASCII, so Incan source can name only a Rust item whose name is ASCII. Rust names that appear only in binding metadata are shown through the display rule, and a binding's ASCII names are checked for confusable pairs in the scope that imports them.

### Vocabulary blocks

- A vocabulary block's text is source, and the rules apply to it as written.
- Source that a desugarer produces is checked the same way before it compiles.
- Identifiers a desugarer introduces join the confusable check in the scope they enter.
- A diagnostic about produced source points at the vocabulary block and names the desugarer.

### Compatibility

- Source that holds a bidirectional control is refused. The diagnostic's hint gives the replacement for each occurrence.
- A `str` literal that held the text `\u{` followed by hexadecimal digits and `}` now holds the character those digits name, and one that held `\u{` followed by anything else is refused. Writing `\\u{` keeps the old text.

## Alternatives considered

- **Allowing the controls in comments.** The reordering attack works in comments as well as anywhere else, so a comment is no safer.
- **Allowing them in strings when they are balanced.** Balance doesn't decide what a reader sees, and tools disagree on how to render it. An escape shows exactly what the value holds.
- **Python's `\uXXXX` and `\UXXXXXXXX` escapes.** Two spellings with fixed lengths for one idea, and a backslash followed by `u` would stop being plain text in paths such as `"C:\users"`. Braces mark where the escape ends.
- **Refusing every invisible character.** Zero-width joiners and similar characters appear in legitimate emoji and in some scripts, and they don't reorder display. Only the reordering controls are refused.
- **Making confusable pairs an error.** Some ASCII pairs, such as `I` and `l` in mathematical code, are deliberate. A warning flags them without refusing working code.
- **Unicode identifiers.** They would bring UTS #39's mixed-script rules and restriction levels with them. Identifiers stay ASCII, so mixed scripts cannot occur.

## Drawbacks

- Right-to-left text in a string needs escapes for its marks, which makes such source harder to read.
- Oven scans every source file it compiles, including dependencies' Rust and C files. This costs build time and can refuse a dependency that compiled elsewhere.
- The confusable warning compares every pair of names in a scope by skeleton, which costs checking time in very large modules.

## Layers affected

- **Lexer:** the refusal, the `\u{…}` escape and the exact character position.
- **Typechecker:** the confusable check once names resolve, across imports.
- **Lowering / IR emission:** escapes lower to their scalar values.
- **Stdlib:** none.
- **CLI / tooling:** Oven's check of every compiled source file, and the display rule in diagnostics and API documentation.
- **LSP / formatter:** the same diagnostics, code actions for each hint, the formatter's refusal and the display rule in hover and signature help.

## Inspectability and tooling surface

- **Artifact or metadata:** none beyond the diagnostics.
- **Inspection command:** `incan check` reports each diagnostic with its code and exact position, and Oven reports the same code for Rust and C sources.
- **Diagnostics:** the refused control, the malformed escape and the confusable pair, each with a code, the exact character and a hint naming the fix.
- **Provenance:** the file, line and column of the character, and for produced source the vocabulary block and its desugarer.
- **Not implicit:** no tool removes or rewrites a control without the author, and none renders one.

## Design Decisions

- **Refuse everywhere.** One rule for code, comments and strings leaves no place where the attack works.
- **Escapes show what the value holds.** A control that a string needs is visible in the source and in every tool that displays it.
- **One rule in every tool.** A check that the formatter, the editor or the build skipped would be a place to hide the character.
- **Confusables warn.** Look-alike names are a hazard, not a mistake in every case, so they are flagged without refusing the build.
