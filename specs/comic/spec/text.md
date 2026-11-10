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
| `Type << ( … )` | One value of `Type`. |
| `Type << [ … ]` | Several values of `Type`, one per row. |

A complete document:

~~~text
example.Address = Record { street: string, city: string, country: string }

example.Fulfillment = Enum { Shipped(carrier: string, tracking: string), Pending }

example.Order = Record {
    id: i64
    placed_at [canonical = false]: string
    address: example.Address
    fulfillment: example.Fulfillment
    note: string? = None
}

example.Order << (
    id=1003,
    placed_at="2026-11-04T09:13:00Z",
    address=(street="42 Halting Lane", city="Amsterdam", country="NL"),
    fulfillment=Shipped(carrier="DHL", tracking="JD144155"),
)

example.Order << [
    (1000, "2026-11-01T09:10:00Z", ("1 Compiler Court", "Zurich", "CH"), Pending),
    (1001, "2026-11-02T09:11:00Z", ("1 Compiler Court", "Boston", "US"), Pending, "Leave at the door"),
]
~~~

A value's type is described in the document before its first value. Builtin types need no description. A profile may register descriptions of its own standard types, which a document using that profile need not repeat.

Canonical form puts descriptions first, sorted by path, then values in the order written. A document has no hash and is never sealed; every value has its own hashes. A value or table cut off before its closing bracket is refused, so truncation inside an entry cannot pass. A document cut at an entry boundary reads as a shorter document. Values that cross a network boundary should therefore travel crumbled, as a sealed Crumb set (see [binary](binary.md)); text is for reading, diffs, tests and local files.

## Names and paths

A path is a dotted sequence of segments read as one token, such as `example.billing.Invoice`. A segment follows Unicode identifier rules (UAX #31: an `XID_Start` character or `_`, then `XID_Continue` characters). A profile may admit more in a segment, such as hyphens in registry names or a bracketed version, and must define where.

A path is absolute: it starts at a root its profile defines and never passes through an alias or link. Inside a value, a type or variant may be written by its short name when the slot admits exactly one type with that name. A short name that could mean two types is refused.

The printer writes a type's short name, its last path segment, whenever the slot admits no other type with that name, and the full path otherwise: `File << (…)` in an `example.File | example.Folder` slot, but `a.Id << 3` in an `a.Id | b.Id` slot. A variant prints as its name alone, as `Pending`. Only when the slot admits two enums with a variant of that name does it take its enum's name too, as `Status.Pending`.

A path holds at most one declaration of each kind. A profile may give one path several kinds, as a Rust trait and its derive macro share one name, but a value's type must name exactly one declaration that values can have: a `Record`, an `Enum`, or a profile kind that maps to one.

## Descriptions

A description is written in `{ }`. Inside it, `:` declares and `=` sets:

- `name: Type` declares a field, in declaration order. A row lists its cells in this order.
- `name: Type = None`, or `= []`, `= {}` for a collection, declares the field's absent value. A field without one is required.
- Field metadata goes in `[ ]` after the name, as in `placed_at [canonical = false]: string` and `price [renamed_from = ["unit_price"]]: decimal<10, 2>`. `canonical` defaults to `true`.
- `key = value` sets a property of the declaration itself, such as a profile's `visibility = Public`. A property may group further properties in `{ }`, as a profile's function signature `type = { … }` does.
- Properties come before the members and print in the order their kind declares them: the core for `Record` and `Enum`, a profile for its own kinds. `Incan.Function` declares `visibility`, `decorators`, `type` and `body`, so a function prints them in that order, whatever order a hand-written document used. Field metadata follows the same rule. A printer that has a profile's registration, which maps its kinds to `Record` or `Enum`, but not the kinds' own declarations reads the shape and keeps that profile's properties in the order it read them. A kind that no registration maps is refused.
- In an enum, each member is a variant: `Pending`, or `Shipped(carrier: string, tracking: string)` with named payload fields. A variant may instead list only its payload types, as `Moved(i64, i64)`. Its fields are then named by position, `0`, `1` and so on, and it prints the same way. One variant does not mix the two forms.

The core kinds are `Record` and `Enum`. A profile may define its own kinds, such as `Incan.Model` and `Incan.Enum`, which add that language's facts. The [profile registrations](../profiles.json) map each such kind to `Record` or `Enum`, so a reader that does not know the profile still reads the description.

A declaration's kind decides its type names. Inside a profile's kind, the profile's own types apply: an `Incan.Model` declares `int`, `str` and `Option[str]`, and a `Rust.Struct` declares `Vec<u8>`. Each profile maps its types onto the [value domain](values.md) itself. Inside the core kinds, types are [Apache Substrait](https://substrait.io/types/type_classes/)'s built-in types, with COMIC's additions for what Substrait doesn't have:

| Type | Text |
| --- | --- |
| Boolean | `boolean` |
| Integers | `i8`, `i16`, `i32`, `i64`, and COMIC's `i128` |
| Unsigned integers | `u8`, `u16`, `u32`, `u64`, from Substrait's unsigned-integer extension, and COMIC's `u128` |
| Floats | `fp32`, `fp64` |
| Decimal | `decimal<P, S>`, precision and scale |
| String, binary | `string`, `binary`, and `varchar<L>`, `fixedchar<L>`, `fixedbinary<L>` |
| Date and time | `date`, `precision_time<P>`, `precision_timestamp<P>`, `precision_timestamp_tz<P>`, `interval_year`, `interval_day<P>`, `interval_compound<P>`, `uuid` |
| Collections | `list<T>`, `map<K, V>`, and COMIC's `set<T>` |
| Optional | `T?`, one optional layer, and COMIC's `option<T>` for a nested one, as `option<string?>` |
| Union | COMIC's `A \| B` |
| Secret | COMIC's `secret_string`, `secret_bytes` |
| Reference | COMIC's `ref<T>`, a reference to a value of the declared type `T`, which reading never follows |
| Described type | its path, as `example.Address` |

A record, an enum with payloads and a reference are COMIC's own: they are declared by path, as above.

## Values

Data is written in `( )`. A root value names its type with `<<`; inside it, every value's type is known from its slot, so none is named.

Every record and every variant payload is one list of cells, read the same way wherever it appears:

- **Positions first.** Leading cells fill fields in the document's declaration order.
- **Then keys.** After the positions, a cell names its field, `city="Utrecht"`, or gives its declaration index counted from 0, `4="Utrecht"`. Keyed cells may come in any order and may mix names and indexes.
- **Everything else is absent.** A field without a cell takes its declared absent value. A required field without one is refused.
- A field given twice, whether by position, name or index, is refused, and so are a positional cell after a keyed one, a name the description does not declare and an index past its last field.

~~~text
example.Person << [
    (1001, "Jansen", city="Utrecht"),
    (1002, "de Vries"),
    (1003, "Bakker", "Anna", partner_id=1001),
    (id=1004, family_name="Visser", 4="Delft"),
]
~~~

The printer writes one form:

- **A value written alone** prints every present field by name: `example.Order << (id=1003, …)`.
- **A row** prints positions up to its first absent field, then each later present field by name.
- **A field at its declared absent value is never printed.** Every other field is, including one equal to a default, because the text must not depend on defaults.
- **Two or more records of one type together, in a list or a map, print as rows** wherever they appear, so a value's own collections read like tables, one row per line:

  ~~~text
  example.Order << (
      id=7,
      lines=[
          ("SKU-1", 2, 4.50d),
          ("SKU-2", 1, 9.95d),
      ],
  )
  ~~~
- **A single nested record and a variant payload follow the value they sit in:** by name inside a value written alone, and as a row inside a row. A variant is its name with its payload in `( )`: `Pending`, `Shipped(carrier="DHL", tracking="JD144155")`, and in a row `Shipped("DHL", "JD144155")`.
- Indexes are accepted on reading and never printed.

**Wrapped values.** Where a slot admits more than one type, a value names its type with `<<`: `Month << 4` for a newtype, `i64 << 3` in an `i64 | string` slot. Elsewhere a newtype or union member is written bare.

`Type << ( … )` lists stored fields. It is not a constructor call, and reading one never runs a constructor, field method or default. It rebuilds the stored state, and the requested type's final validation then checks that state, the same as after decoding a Crumb.

`<<` takes one value or a list of them, and every value in either is the same cell list. `Type << [(…)]` reads exactly as `Type << (…)`. Consecutive values of one type may be written one at a time or as one table; the printer writes a single value as `Type << ( … )` and two or more consecutive values as one table. Each row is one value with its own hashes. A table is layout only; no hash covers it.

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
| Multi-line string | `"""` … `"""` | Everything between the opening and closing `"""` is the value, exactly: every line break and every space of indentation. So it's written flush left. |
| Bytes | `b"abc\x00"` | Printable ASCII is literal except `\` and `"`; every other byte is `\x` and two lowercase hexadecimal digits. |
| List | `[1, None, 3]` | Keeps order and absent elements. |
| Tuple | `(10, 20)`, `(7,)` | A one-element tuple keeps its trailing comma. |
| Map | `{"a": 1}` | Sorted in the value profile's order, so one map prints one way. `{}` is the empty map or set its slot's type names. |
| Set | `{1, 2}` | Sorted in the value profile's order. |
| Optional | `note="mail"` | A present value is written bare, and a field at its absent value is left out. `None` is written only where absence is not a field left out: a list element (`[1, None, 3]`), a tuple or map member, a present inner optional (`Some(None)`), and an optional field with no declared absent value. |
| Reference | `example.billing.Cents` | A reference to a declaration is its unquoted path, and a string equal to that path is a string, not a reference. A reference to a stored record is the record's identity, written as the [store profile](../profiles/store.md) writes `___id`. |
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

In a table of stored records, each row writes the same five keys by name after its fields. Three underscores mark a key as the store's. The list is closed: any other `___` key is refused, and a description cannot declare a `___` field. No hash covers a `___` key. A hash names its value profile and algorithm, as `crumb1:sha256:`, and hashes under different profiles are never compared. A value that was never stored has no `___` keys. The [store profile](../profiles/store.md) owns the text form of `___id` and `___revision`.

## Layout

The printer produces exactly one text for a document:

- A value stays on one line up to 120 characters, counting its indentation, unless something inside it must break, such as a list of records. Otherwise each field goes on its own line, indented 4 spaces, each followed by a comma. Inside a value, `=` takes no spaces: `id=1003`.
- A description stays on one line up to 120 characters, with its properties first and `, ` between them and its members, as `shop.billing.Invoice = Incan.Model { visibility = Public, id: Int, customer: Str, total: shop.billing.Cents }`. Otherwise each member and property goes on its own line, indented 4 spaces, with no commas. Inside a description, `=` takes spaces: `canonical = false`.
- Lists, tuples and maps separate their elements with `, `. A list or map of more than one record, or of an enum with payloads, takes one element per line, with a trailing comma, wherever it appears, including in a description's property. A table takes one row per line.
- A line still past 120 characters breaks at its last open bracket.
- `#` starts a comment that runs to the end of the line. Comments are never data: they are not hashed, not stored in a Crumb, and not printed with a value. A formatter rewriting a hand-edited document keeps them.

Readers accept any whitespace between tokens. The printer's output must not depend on terminal width, locale, host map iteration or a language formatter.

## What a read produces

Text is a syntax for the [value domain](values.md). Reading produces the same descriptions and values a crumb decodes to, and printing writes them back:

- **Names and identities.** Each field and variant takes its identity from its earliest name, which is the first in its `renamed_from` when it has one and its own name otherwise, under [hashes](hashes.md). A value's fields and variant are matched to its type's description by name and held by identity. Printing finds the names again through the description.
- **Resolved types.** A path resolves to a description in the document, a type the reader's caller admits, or a type of the description's profile, before the read returns. A path that none of them resolves is refused, so no unresolved name reaches the value domain.
- **Nominal identities.** A described type's identity comes from its package's canonical identity. Its profile derives it where its paths name the package, as the Incan profile's `pub.` paths do, and the reader's caller supplies it otherwise. When neither does, the type has no identity: it still reads and prints as text, and encoding it as a crumb is refused, because a crumb stores identities.
- **References.** `ref<T>` keeps its target type `T`, and a reference value keeps both the target type's identity and the target's own.
- **Properties.** A property value is one [metadata value](hashes.md#complete-description-preimage-d):

| Text | Metadata |
| --- | --- |
| An unquoted path, as `Public` | Symbol |
| `"text"` | String |
| `true`, `false` | Boolean |
| `42` | Integer |
| `None` | Null |
| `b"…"` | Binary |
| `[ … ]` | List |
| `{ name = value, … }` | Map, keyed by property name |

A symbol stays a symbol and a string stays a string, as in values. A map holds its entries in the order read, so a printer without the kind's declared order keeps that order.

## Reading safely

Reading runs nothing from the document: a document holds no code, and a description grants none. The only code a read reaches is the final validation of the type the caller requested, which the caller's own implementation of that type supplies, as the [reconstruction boundary](values.md#reconstruction-boundary) states. A reader runs under caller-visible limits on bytes, nesting depth, the length of one string or number, and memory. It checks each table row and nested value against them as it reads, so a short document cannot make it allocate far more than its own size. A reference never expands into a copy of its target, so a document has no alias to multiply.

One text reads as one value or is refused. A key given twice, two declarations of one kind at one path, a value type that names no single declaration values can have, a value of a type with no description, an unknown `___` key and an ambiguous short name are refused, never settled by keeping the first or the last. Reading a value as a type means it fits that type's description, not that the type's owner wrote it.

## Open

- Date and time literals. Until decided, they are strings in a slot whose type validates them.
