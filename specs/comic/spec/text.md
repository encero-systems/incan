# COMIC text

Status: draft rules. They record the syntax as designed and test-driven, so implementations and the grammar corpus have one target. A rule becomes conformance-tested when the corpus holds its input, expected typed value and exact canonical text. The corpus's JSON notation is not this text format.

COMIC describes typed values. It has no imports, statements, callable expressions or evaluation. Names label types, records and references; parentheses hold positional data. Seeing a function description does not load or run that function, and reading a reference resolves nothing.

Reading needs the admitted description of the value's type, as decoding a Crumb does. A key, a short name or an empty `{}` is interpreted through that description; text is never typed by guessing.

## Documents

A document is UTF-8 with LF line endings and a final newline. Its extension is `.comic`, except that a file with a well-known name keeps that name. Unicode bidirectional controls are refused anywhere in a document, including comments and strings, where they must be written as escapes. Identifiers that mix scripts or are confusable under UTS #39 are reported.

A document is a sequence of root entries, separated by one blank line in canonical form. There are no imports, so a reader can stream a document and a writer can append to one. An entry is one of:

- a **binding**, `path = Kind { … }`, which describes what exists at a path. A binding records data about a declaration; it declares nothing in a program that reads it. Kinds come from a profile.
- a **value**, `Type { … }`, which has no name of its own.
- a **table** of two or more values of one type, described below.

Canonical form sorts bindings by path. A document has no hash; every value has its own.

## Names and paths

A path is a dotted sequence of segments read as one token, such as `example.billing.Invoice`. A segment follows Unicode identifier rules (UAX #31: an `XID_Start` character or `_`, then `XID_Continue` characters). A profile may admit more in a segment, such as hyphens in registry names or a bracketed version, and must define where.

A path in a record is absolute: it starts at a root its profile defines and never passes through an alias or link. Inside a value, a type may be written by its short name when the slot's description admits exactly one type with that name. A short name that could mean two types is refused.

## Blocks and fields

Named fields go in `{ }`, as `key = value`. Positional data goes in `( )`, and only after a variant name or as a tuple, so nothing can be called.

- A reader matches a key to the description's current field name or any earlier name the description records. A key given twice, or a key the description does not declare, is refused.
- The printer writes fields in declaration order. A reader accepts any order. Hashes use member identity order, never presentation order.
- A field at its declared absent value is left out: an optional that is absent, or a collection that is empty where the description declares empty as its absent value. Every other field is written, including a field equal to a default, because the text must not depend on defaults.
- A block's type name is left out when its slot admits only one type. Root entries and slots admitting several types keep it.
- A record with exactly one field, in a slot whose type is that record, prints as that field's value: `body = Block { statements = […] }` prints as `body = […]`.

## Literals

| Value | Text | Rule |
| --- | --- | --- |
| Boolean | `true`, `false` | |
| Integer | `1000000`, `-42` | Decimal digits only: no `+`, no leading zeros, no `_`. |
| Float | `20.5`, `100.0`, `1e16`, `2.5e-7` | The shortest digits that round-trip at the declared width, always with a point or an exponent. Positional when the decimal exponent is from -4 to 15, otherwise scientific with no `+` and no leading zeros in the exponent. |
| Signed zero | `0.0`, `-0.0` | Distinct values. |
| Non-finite | `inf`, `-inf`, `nan` | Only in a float slot. |
| Decimal | `19.90d`, `-0.50d` | Exactly the type's scale, then `d`. |
| String | `"text"` | Double quotes, no normalization. Escapes: `\\`, `\"`, `\n`, `\t`, `\r`, and `\u{…}` in lowercase hexadecimal without leading zeros for every other control character and every bidirectional control. All other characters are literal UTF-8. |
| Multi-line string | `"""` … `"""` | Written flush left, because indentation would be part of the value. |
| Bytes | `b"abc\x00"` | Printable ASCII is literal except `\` and `"`; every other byte is `\x` and two lowercase hexadecimal digits. |
| List | `[1, None, 3]` | Keeps order and absent elements. |
| Tuple | `(10, 20)`, `(7,)` | A one-element tuple keeps its trailing comma. |
| Map | `{"a": 1}` | Sorted in the value profile's order, so one map prints one way. `{}` is the empty map or set its slot's type names. |
| Set | `{1, 2}` | Sorted in the value profile's order. |
| Optional | `alias = "mail"` | A present value is written bare. `Some(…)` is written only for a present inner optional, as `Some(None)`. |
| Enum | `Status.Active` | A value enum prints its variant, not its backing value. |
| Payload | `Shape.Circle(1.0)` | Positional payloads in `( )`. |
| Newtype | `month = 4` | Bare in a slot of that newtype. Written as `Month(4)` where the slot admits more than one type. Reading validates the newtype's constraints. |
| Union | `3`, `Int(3)` | Bare when only one member can read the text; otherwise wrapped in its member's type name. |
| Generic value | `example.Box[Int] { item = 1 }` | Type arguments in `[ ]`. |
| Reference | `example.billing.Cents` | An unquoted path. A string equal to that path is a string, not a reference. |
| Secret | `<secret>` | A secret-typed value is withheld (see [values](values.md)). Reading yields a withheld value. |

A marker other than `<secret>`, such as `<Duration: 1.5s>` for opaque state, is diagnostic text from a profile. Reading one is refused.

## Tables

Two or more consecutive values of one type print as a table, and so does a list of two or more records in a slot:

~~~text
example.Order { id, buyer { id, name }, items { sku, quantity } } [
    (1, (7, "ada"), [("SKU-1", 3), ("SKU-2", 1)]),
    (2, (9, "bob"), []),
]
~~~

- The header names every field a row holds, including nested records and lists of records, so rows are matched to fields by name and reordering a description cannot silently change what old text means.
- A cell is never skipped. A row writes `None` for an absent optional and `[]` for an empty collection. A column is left out of the header when every row could leave that field out.
- Each row is one value with its own hashes. A table is layout only; no hash covers it.
- A reader accepts a lone value or a second table where a table could continue, and the printer merges them, so a writer can append and each value keeps one text.

## Store keys

A record exported from a store carries the store's keys after its fields, in this order:

~~~text
example.readings.Reading {
    sensor = "a1"
    value = 20.5
    ___id = "…"
    ___revision = "…"
    ___model_hash = "crumb1:sha256:…"
    ___canonical_hash = "crumb1:sha256:…"
    ___object_hash = "crumb1:sha256:…"
}
~~~

Three underscores mark a key as the store's. The list is closed: any other `___` key is refused, and a description cannot declare a `___` field. No hash covers a `___` key. A hash names its value profile and algorithm, as `crumb1:sha256:`, and hashes under different profiles are never compared. A value that was never stored has no `___` keys. The [store profile](../profiles/store.md) owns the text form of `___id` and `___revision`.

## Layout

The printer produces exactly one text for a value:

- A block stays on one line up to 120 characters, counting its indentation. Otherwise each field goes on its own line, indented 4 spaces, with no comma between fields.
- Lists, tuples and one-line blocks separate their elements with `, `. A list of more than one block or variant takes one element per line, with a trailing comma. A table takes one row per line.
- A line still past 120 characters breaks at its last open bracket.
- `#` starts a comment that runs to the end of the line. Comments are never data: they are not hashed, not stored in a Crumb, and not printed with a record. A formatter rewriting a hand-edited document keeps them.

Readers accept any whitespace between tokens. The printer's output must not depend on terminal width, locale, host map iteration or a language formatter.

## Reading safely

Reading builds data and runs nothing. A reader runs under caller-visible limits on bytes, nesting depth, the length of one string or number, and memory. It checks each table row and nested block against them as it reads, so a short document cannot make it allocate far more than its own size. A reference never expands into a copy of its target, so a document has no alias to multiply.

One text reads as one value or is refused. A key given twice, two kinds at one path, an unknown `___` key and an ambiguous short name are refused, never settled by keeping the first or the last. Reading a record as a type means it fits that type's description, not that the type's owner wrote it.

## Open

- The exact framing of a multi-line string: which line breaks after the opening and before the closing `"""` belong to the value.
- Named enum payloads: a payload with named fields printing as `Shape.Circle { radius = 1.0 }`, so reordering it cannot misread a loose document.
- The table form of a map of records.
- Whether a canonical document carries row counts and a covering hash, so truncation or splicing fails before any value is used. This conflicts with "a document has no hash" above and needs one answer.
- Date and time literals. Until decided, they are strings in a slot whose type validates them.
