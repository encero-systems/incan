# Source security

Incan applies the same source-security rules in the compiler, formatter, language server and Oven.

## Bidirectional controls

The following Unicode `Bidi_Control` characters are refused when they occur literally anywhere in an Incan source file, including code, comments, strings, f-strings and docstrings (`INCAN-P0101`):

| Scalars | Names |
| --- | --- |
| U+061C | ARABIC LETTER MARK |
| U+200E, U+200F | LEFT-TO-RIGHT MARK, RIGHT-TO-LEFT MARK |
| U+202A–U+202E | LEFT-TO-RIGHT EMBEDDING, RIGHT-TO-LEFT EMBEDDING, POP DIRECTIONAL FORMATTING, LEFT-TO-RIGHT OVERRIDE, RIGHT-TO-LEFT OVERRIDE |
| U+2066–U+2069 | LEFT-TO-RIGHT ISOLATE, RIGHT-TO-LEFT ISOLATE, FIRST STRONG ISOLATE, POP DIRECTIONAL ISOLATE |

The diagnostic selects the exact control, names it and shows its visible lowercase scalar escape. In a string value, write that escape, such as `"\u{202e}"`. Outside a string, remove the control. The language server offers both edits when they apply.

Diagnostics, formatter output, hover and signature text, checked API documentation and inspection output render a control held by a decoded value as its visible scalar escape. They do not reproduce the raw control.

Oven applies the same refusal before compiling authored Rust and C-family source files in every selected local facet and dependency. Generated output directories and precompiled registry units are not authored native inputs.

## Confusable identifiers

Within one lexical scope, two different ASCII identifiers whose [Unicode security skeleton](https://www.unicode.org/reports/tr39/) is the same produce warning `INCAN-T0136`. The warning selects the later identifier and labels the earlier declaration. For example, `admin` and `adrnin` share a skeleton because the ASCII `m` maps to `rn`.

The check is scoped: the same pair in separate functions, types, closures or comprehensions does not warn. Exact repeated spellings continue to follow the ordinary duplicate-declaration rules rather than the confusable warning.
