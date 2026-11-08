# COMIC text

Status: draft rules. They record the syntax as designed and test-driven, so implementations and the grammar corpus have one target. A rule becomes conformance-tested when the corpus holds its input, expected typed value and exact canonical text. The corpus's JSON notation is not this text format.

COMIC describes typed values. It has no imports, statements, callable expressions or evaluation. Seeing a function description does not load or run that function, and reading a reference resolves nothing.

A document carries the descriptions of the types its values use, so it reads without the program that wrote it. A reader parses each value through the document's own description of its type, then admits it against the description the caller requests, matching fields by name and by the earlier names that description records, under the [compatibility](evolution.md) rules. Text is never typed by guessing.

## Documents

A document is UTF-8 with LF line endings and a final newline. Its extension is `.comic`, except that a file with a well-known name keeps that name. Unicode bidirectional controls are refused anywhere in a document, including comments and strings, where they must be written as escapes. Identifiers that mix scripts or are confusable under UTS #39 are reported.

A document is a sequence of root entries, separated by one blank line in canonical form. There are no imports, so a reader can stream a document and a writer can append to one. Three forms make up a document, each with one meaning:

| Form | Means |
| --- | --- |
| `path = Kind { … }` | What exists at `path`: a type's description, or another declaration a profile records. |
| `Type << ( … )` | One value of `Type`, with its fields by name. |
| `Type << [ … ]` | Several values of `Type`, each by position. |

A complete document:

~~~text
example.Address = Record { street: Str, city: Str, country: Str }

example.Fulfillment = Enum { Shipped(carrier: Str, tracking: Str), Pending }

example.Order = Record {
    id: Int
    placed_at [canonical = false]: Str
    address: example.Address
    fulfillment: example.Fulfillment
    note: Option[Str] = None
}

example.Order << (
    id=1003,
    placed_at="2026-11-04T09:13:00Z",
    address=(street="42 Halting Lane", city="Amsterdam", country="NL"),
    fulfillment=Shipped(carrier="DHL", tracking="JD144155"),
)

example.Order << [
    (1000, "2026-11-01T09:10:00Z", ("1 Compiler Court", "Zurich", "CH"), Pending, None),
    (1001, "2026-11-02T09:11:00Z", ("1 Compiler Court", "Boston", "US"), Pending, "Leave at the door"),
]
~~~

A value's type is described in the document before its first value. Builtin types need no description. A profile may register descriptions of its own standard types, which a document using that profile need not repeat.

Canonical form puts descriptions first, sorted by path, then values in the order written. A document has no hash and is never sealed; every value has its own hashes. A value or table cut off before its closing bracket is refused, so truncation inside an entry cannot pass. A document cut at an entry boundary reads as a shorter document. Values that cross a network boundary should therefore travel crumbled, as a sealed Crumb set (see [binary](binary.md)); text is for reading, diffs, tests and local files.

## Names and paths

A path is a dotted sequence of segments read as one token, such as `example.billing.Invoice`. A segment follows Unicode identifier rules (UAX #31: an `XID_Start` character or `_`, then `XID_Continue` characters). A profile may admit more in a segment, such as hyphens in registry names or a bracketed version, and must define where.

A path is absolute: it starts at a root its profile defines and never passes through an alias or link. Inside a value, a type or variant may be written by its short name when the slot admits exactly one type with that name. A short name that could mean two types is refused.

## Descriptions

A description is written in `{ }`. Inside it, `:` declares and `=` sets:

- `name: Type` declares a field, in declaration order. A row lists its cells in this order.
- `name: Type = None`, or `= []`, `= {}` for a collection, declares the field's absent value. A field without one is required.
- Field metadata goes in `[ ]` after the name, as in `placed_at [canonical = false]: Str` and `price [renamed_from = ["unit_price"]]: Decimal { scale = 2 }`. `canonical` defaults to `true`.
- `key = value` sets a property of the declaration itself, such as a profile's `visibility = Public`. A property may group further properties in `{ }`, as a profile's function signature `type = { … }` does.
- In an enum, each member is a variant: `Pending`, or `Shipped(carrier: Str, tracking: Str)` with named payload fields.

The core kinds are `Record` and `Enum`. A profile may define its own kinds, such as `Incan.Model` and `Incan.Enum`, which add that language's facts. The [profile registrations](../profiles.json) map each such kind to `Record` or `Enum`, so a reader that does not know the profile still reads the description.

Types are written as follows:

| Type | Text |
| --- | --- |
| Boolean | `Bool` |
| Integer | `Int`, which is 64-bit signed. Another width or a bound: `Int { storage = u32, ge = 0 }`. |
| Float | `Float`, which is binary64. Binary32: `Float { storage = f32 }`. |
| Decimal | `Decimal { scale = 2 }` |
| String, bytes | `Str`, `Bytes` |
| Secret | `SecretStr`, `SecretBytes` |
| Collections | `List[T]`, `Tuple[A, B]`, `Dict[K, V]`, `Set[T]` |
| Optional | `Option[T]` |
| Union | `A \| B` |
| Described type | its path, with type arguments in `[ ]`, as `example.Box[Int]` |

## Values

Data is written in `( )`. A root value names its type with `<<`; inside it, every value's type is known from its slot, so none is named.

- **By name.** `Type << (name=value, …)` lists fields by name. A reader accepts them in any order; the printer writes declaration order. A key given twice, or a key the description does not declare, is refused.
- **By position.** In a table, `Type << [(…), (…)]`, each row lists every field in the document's declaration order. Nested records and lists of records inside a row are positional too. A cell is never skipped: a row writes `None` for an absent optional and `[]` or `{}` for an empty collection.
- **Absent fields.** A value written by name leaves out every field at its declared absent value. Every other field is written, including one equal to a default, because the text must not depend on defaults.
- **Nested records.** A nested record is `( … )`, by name inside a value written by name and by position inside a row.
- **Variants.** A variant is its name, with its payload in `( )`: `Pending`, `Shipped(carrier="DHL", tracking="JD144155")` inside a value written by name, and `Shipped("DHL", "JD144155")` inside a row.
- **Wrapped values.** Where a slot admits more than one type, a value names its type with `<<`: `Month << 4` for a newtype, `Int << 3` in a union slot. Elsewhere a newtype or union member is written bare.

`Type << ( … )` lists stored fields. It is not a constructor call, and reading one never runs a constructor, field method or default. It rebuilds the stored state and runs the type's final validation, the same as decoding a Crumb.

Consecutive values of one type may be written one at a time or as one table; the printer writes two or more consecutive values as a table. Each row is one value with its own hashes. A table is layout only; no hash covers it.

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
| Optional | `note="mail"` | A present value is written bare. `Some(…)` is written only for a present inner optional, as `Some(None)`. |
| Reference | `example.billing.Cents` | An unquoted path. A string equal to that path is a string, not a reference. |
| Secret | `<secret>` | A secret-typed value is withheld (see [values](values.md)). Reading yields a withheld value. |

A marker other than `<secret>`, such as `<Duration: 1.5s>` for opaque state, is diagnostic text from a profile. Reading one is refused.

## Store keys

A record exported from a store carries the store's keys after its fields, in this order:

~~~text
example.readings.Reading << (
    sensor="a1",
    value=20.5,
    ___id="…",
    ___revision="…",
    ___model_hash="crumb1:sha256:…",
    ___canonical_hash="crumb1:sha256:…",
    ___object_hash="crumb1:sha256:…",
)
~~~

In a table of stored records, each row adds the same five cells after its fields. Three underscores mark a key as the store's. The list is closed: any other `___` key is refused, and a description cannot declare a `___` field. No hash covers a `___` key. A hash names its value profile and algorithm, as `crumb1:sha256:`, and hashes under different profiles are never compared. A value that was never stored has no `___` keys. The [store profile](../profiles/store.md) owns the text form of `___id` and `___revision`.

## Layout

The printer produces exactly one text for a document:

- A value stays on one line up to 120 characters, counting its indentation. Otherwise each field goes on its own line, indented 4 spaces, each followed by a comma. Inside a value, `=` takes no spaces: `id=1003`.
- A description stays on one line up to 120 characters, with `, ` between members. Otherwise each member and property goes on its own line, indented 4 spaces, with no commas. Inside a description, `=` takes spaces: `canonical = false`.
- Lists, tuples and maps separate their elements with `, `. A list of more than one record or variant takes one element per line, with a trailing comma. A table takes one row per line.
- A line still past 120 characters breaks at its last open bracket.
- `#` starts a comment that runs to the end of the line. Comments are never data: they are not hashed, not stored in a Crumb, and not printed with a value. A formatter rewriting a hand-edited document keeps them.

Readers accept any whitespace between tokens. The printer's output must not depend on terminal width, locale, host map iteration or a language formatter.

## Reading safely

Reading builds data and runs nothing. A reader runs under caller-visible limits on bytes, nesting depth, the length of one string or number, and memory. It checks each table row and nested value against them as it reads, so a short document cannot make it allocate far more than its own size. A reference never expands into a copy of its target, so a document has no alias to multiply.

One text reads as one value or is refused. A key given twice, two kinds at one path, two descriptions of one type, a value of a type with no description, an unknown `___` key and an ambiguous short name are refused, never settled by keeping the first or the last. Reading a value as a type means it fits that type's description, not that the type's owner wrote it.

## Open

- The exact framing of a multi-line string: which line breaks after the opening and before the closing `"""` belong to the value.
- The table form of a map of records.
- Date and time literals. Until decided, they are strings in a slot whose type validates them.
- An optional closing `___` entry holding a document hash, for text that must cross a boundary anyway. It would be checked when present and never required, so it seals nothing by default.
