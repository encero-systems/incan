# RFC 158: Incremental XML records over split and compressed inputs

- **Status:** Planned
- **Created:** 2026-10-17
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 021 (model field metadata and aliases)
    - RFC 061 (`std.compression` codecs)
    - RFC 066 (bounded HTTP bodies)
    - RFC 085 (field metadata and namespaced keys)
    - RFC 094 (scoped resource lifetime)
    - RFC 103 (secret-safe diagnostics)
    - RFC 115 (fallible byte and item iteration)
    - RFC 122 (model validation policies)
    - RFC 131 (typed failures and aggregation)
    - RFC 142 (async consumers and failure policies)
    - RFC 146 (typed decoding and JSONL readers)
    - RFC 152 (depth limits for parsers of external data)
    - RFC 169 (typed record sets and record streams)
- **Issue:** [#2644](https://github.com/encero-systems/incan/issues/2644)
- **RFC PR:** —
- **Written against:** v0.6
- **Shipped in:** —

## Summary

`std.xml` reads a large XML document one record at a time, as typed models or as dynamic elements, in memory that does not grow with the document. The document is one logical byte stream, however it is delivered: `std.fs.open_parts` joins a file split into parts, and new `std.compression` readers decode a compressed stream on the way through, so a 45 GB catalog arriving as 1.5 GB of gzip parts reads like any other file. Parser state survives every boundary. A bad record is reported and reading continues; broken structure stops the stream; and only a document that really ended counts as complete. An import that crashes resumes after a saved position. Parsing must be at least as fast as Python's standard library on the same file.

## Core model

1. **A document is one byte stream.** Parts and compression are ordinary byte readers that compose, and the parser sees only the bytes they deliver. A part boundary is not the end of a document.
2. **Parser state survives every boundary.** Decoding, open elements and namespace bindings carry across chunks, parts and compressed members, so a cut may fall inside a tag or a multibyte character.
3. **Records are children of a named parent.** The caller names the path to the parent, and the record type names the elements. There is no query language, and an element nested inside a record belongs to that record.
4. **Memory follows the limits, not the document.** Each record, token and piece of parser state has a bound, and exceeding one is a typed failure, never truncation.
5. **A bad record is not broken structure.** The parser can step over a record it cannot use. It cannot continue after losing track of the document's structure.
6. **Parsing is not the slow part.** An import spends its time in its own per-record work, so the parser must not be what limits it.

## Motivation

A supplier sends a product catalog as fifteen files cut by byte count, `catalog.xml.gz.001` to `.015`: 1.5 GB of gzip that decodes to 45 GB of XML. The first part holds the opening `<catalog>` and the last part holds the closing tag. Any cut can land inside a compressed block, a tag or a character. Three common approaches fail:

- Parsing each part separately fails, because no part is a valid document or even a valid compressed stream on its own.
- Loading the document or building its whole tree needs memory proportional to 45 GB.
- Searching for `</product>` breaks on namespaces, comments, CDATA, attribute values and nested elements of the same name.

Real feeds have this shape. PubMed publishes `PubmedArticleSet/PubmedArticle` as `.xml.gz` files, OpenStreetMap publishes `osm/{node,way,relation}` as `.osm.bz2`, and banks and insurers exchange long documents that end in control totals.

Python's standard library scans such a file in about a minute through its C parser, and gzip and zstd decode faster than that. Parsing and decompression are not where a large import spends its time; its own per-record work is. A standard library for a language that compiles natively must not make them the bottleneck, and it must not make the caller choose between speed and typed records.

`std.json` already has this family for JSONL under RFC 146: lazy typed records, one `Result` per record, bounded memory and resumption. XML needs the same family, and both need split and compressed input.

## Goals

- Read one logical document across part, chunk and compressed-member boundaries.
- Read compressed byte streams through every `std.compression` codec, for XML, JSONL and any other `std.io.BinaryReader` consumer.
- Yield typed models through `@derive(xml)` and dynamic elements, using the same shapes as RFC 146's JSONL readers.
- Select records by a namespace-aware parent path and the record type, without a query language.
- Keep memory bounded by explicit limits, independent of document size.
- Keep record failures, structural failures, source failures and early stops distinct, with honest completion.
- Resume an interrupted import after a saved position, through any byte source.
- Refuse external entities and entity expansion while still reading real documents that carry a DOCTYPE.
- Parse at least as fast as Python's standard library on the same file.

## Non-Goals

- Treating each part as an independent document. A set of complete documents, such as PubMed's numbered files, is one `records` call per file.
- Recovering after lost structural state. XML 1.0 requires a processor to stop normal processing after a fatal error.
- XPath, XQuery or any other string query language, or XML Schema and DTD validation.
- Writing XML from models or a streaming XML writer. `XmlElement.to_xml()` serves quarantine and inspection.
- XML 1.1, which is reported as a typed error.
- Waiting for more bytes at the end of a file that is still being written. A source ends at its reader's end, as RFC 146's does.

## Guide-level explanation

Every `std.xml` name below, `std.fs.open_parts` and the `std.compression` readers are proposed by this RFC.

### Read a product catalog

A catalog looks like this:

```xml
<catalog xmlns="urn:example:catalog" generated="2026-11-04" supplier="acme">
  <product sku="A-1">
    <name>Kettle</name>
    <price currency="EUR">39.00</price>
    <tag>kitchen</tag>
    <tag>sale</tag>
  </product>
  <product sku="A-2">
    <name>Toaster</name>
    <price currency="EUR">24.50</price>
    <description>Two slots.</description>
  </product>
</catalog>
```

The models describe one record, and `records[T]` reads them one at a time:

```incan
from std.fs import Path
from std.io import IoError
from std.serde import xml
from std.xml import QName, XmlStreamError, records, xml_element

const CATALOG: str = "urn:example:catalog"

@derive(xml)
model Price:
    currency [xml.attribute=true]: str
    amount [xml.text=true]: float

@derive(xml)
@xml_element("product", namespace=CATALOG)
model Product:
    sku [xml.attribute=true]: str
    name: str
    price: Price
    tags as "tag": list[str]
    description: Option[str]

error ImportError = IoError | XmlStreamError

def import_catalog(path: Path, mut store: ProductStore) -> Result[None, ImportError]:
    with path.open("rb")? as file:
        for item in records[Product](file, at=[QName(CATALOG, "catalog")], max_record_bytes=8_000_000)?:
            match item:
                Ok(product) => store.add(product)
                Err(error) => store.reject(error)
    return Ok(None)
```

`ProductStore` is the application's own. `at=` names the path from the root to the parent of the records, and `@xml_element` names the record's element, so this reads every `product` directly under `catalog`. A name is a namespace plus a local name: a document that writes `<c:product>` with `c` bound to the same namespace is the same element. Fields map to child elements of the same name, `xml.attribute` and `xml.text` mark attributes and text, `as` gives a different name, a `list` field collects repeated children and an `Option` field may be absent. A child field uses its parent's namespace unless it declares `xml.namespace`.

Each `product` is one item, and it is released as soon as the program lets go of it. Other elements under `catalog` are checked for well-formedness and skipped without being kept. A `<product>` nested inside a `<product>` belongs to the outer record and is never yielded on its own.

### Read split and compressed files

Bytes reach the parser through ordinary readers, so split and compressed input compose with the same call:

```incan
from std.compression import gzip
from std.fs import Path, open_parts

def import_parts(paths: list[Path], mut store: ProductStore) -> Result[None, ImportError]:
    with open_parts(paths)? as parts:
        for item in records[Product](gzip.reader(parts), at=[QName(CATALOG, "catalog")], max_record_bytes=8_000_000)?:
            match item:
                Ok(product) => store.add(product)
                Err(error) => store.reject(error)
    return Ok(None)
```

`open_parts` checks that every part exists and records its size before reading anything, so a missing part 14 fails at once, not hours into an import. It opens one part at a time. The caller supplies the order; part names that do not sort correctly as text must be ordered by the caller.

Every codec module has a `reader`: `gzip.reader`, `zlib.reader`, `deflate.reader`, `zstd.reader`, `bz2.reader`, `lzma.reader` and `snappy.reader`. `compression.reader_auto(source)` detects the codec from the stream's first bytes, under the same rules as `decompress_auto`. A reader holds one chunk and the codec's own window, however large the stream is. A file compressed before it was split and a document whose parts were each compressed are read by the same call, because concatenated gzip members decode as one stream.

A cut-off download is a typed failure, not a clean end, and a checksum is checked where the format has one. These readers are for any consumer, so a compressed JSONL log reads the same way:

```incan
for record in jsonl[Event](zstd.reader(file), max_record_bytes=1_000_000)?:
    handle(record)
```

A service reading untrusted uploads can stop a decompression bomb with `gzip.reader(file, max_decoded_bytes=10_000_000_000)`. There is no default ceiling, because a 30:1 ratio on 45 GB is an ordinary file.

An HTTP feed works the same way. `gzip.reader` also takes an RFC 142 async chunk stream, as does `records`, so `records[Product](gzip.reader(response.body()?), at=..., max_record_bytes=...)` is an async source that reads from the connection only as the consumer advances.

### Read several record kinds

An enum of models reads several kinds in document order, each variant contributing its element name. Selecting one model skips the others without building them:

```incan
for item in records[Way](file, at=[QName("", "osm")], max_record_bytes=4_000_000)?:
    handle(item)
```

A trailer is one more record kind, so an insurer's import reconciles it with ordinary code:

```incan
enum Feed:
    Policy(Policy)
    Totals(ControlTotals)
```

Attributes of the root element, such as `generated`, come from `root()`, which reads through the root's start tag:

```incan
source = records[Product](file, at=[QName(CATALOG, "catalog")], max_record_bytes=8_000_000)
generated = source.root()?.attribute(QName("", "generated"))
```

`records` adapts to RFC 169 like any other reader, and RFC 142's consumers apply unchanged: `RecordSet[Product].from_iter(records[Product](...))` gives field selection and byte-bounded batches, and `collect(item_errors=ErrorPolicy.CollectAll)` or a `report_item_error` reporter collects or quarantines failures.

### Read records without a model

`parse_records` yields a dynamic `XmlElement`, the counterpart of `JsonValue`, for inspection tools and for documents with no fixed model:

```incan
from std.xml import QName, parse_records

for item in parse_records(file, at=[QName(CATALOG, "catalog")], names=[QName(CATALOG, "product")], max_record_bytes=8_000_000)?:
    match item:
        Ok(element) => inspect(element)
        Err(error) => println(error.message())
```

An `XmlElement` owns its contents. `to_xml()` writes a standalone document that declares the namespaces it inherited, so a rejected record can be quarantined and still parse.

### Failures

A record the parser can step over is an item failure, and reading continues:

```incan
error XmlRecordError[E with Error]:
    RecordTooLarge(position: SourcePosition, max_record_bytes: int) = f"The record at line {position.line} exceeds {max_record_bytes} bytes."

    Invalid(position: SourcePosition, cause: E):
        message = f"Could not decode the record at line {position.line}."
        caused_by = cause
```

Losing the document's structure stops the stream through the loop header's `?`, as an `XmlStreamError`: a failed read, malformed XML, an input that ends with elements still open, or a limit that protects the parser itself. Only a source that ended with its root element closed finishes normally, so a loop that completes read a complete document. Records already delivered before a failure stay delivered.

### Hostile input

A DOCTYPE is accepted, because real feeds carry one, and its external subset is never fetched. Entity declarations are refused. Together these stop XXE and billion-laughs attacks while still reading PubMed's files. The five predefined entities, character references, CDATA, comments and processing instructions work as XML 1.0 specifies.

### Resume an interrupted import

An import of this catalog may take hours, nearly all of them in `store.add`. When it crashes after 40 GB, the work worth keeping is the records already stored. The import saves each record's position with the record and continues after the saved one:

```incan
from std.xml import XmlPosition, XmlResumeError

error ResumeError = ImportError | XmlResumeError

def resume_import(paths: list[Path], saved: XmlPosition, mut store: ProductStore) -> Result[None, ResumeError]:
    with open_parts(paths)? as parts:
        source = records[Product](gzip.reader(parts), at=[QName(CATALOG, "catalog")], max_record_bytes=8_000_000, after=saved)?
        for item, position in source.with_positions()?:
            match item:
                Ok(product) => store.add(product, position)
                Err(error) => store.reject(error, position)
    return Ok(None)
```

`with_positions()` pairs every item, success or failure, with the `XmlPosition` after it. `after=saved` scans the bytes before that position to rebuild the open elements and namespace bindings. It builds no records and calls no application code, so resuming costs about one parse of the input already read, far less than the work it saves, and it works through any reader, compressed or not.

```incan
error XmlResumeError:
    Stream(cause: XmlStreamError):
        message = "The input before the saved position could not be read."
        caused_by = cause

    SourceChanged(line: int) = f"The input no longer matches the saved position at line {line}."
```

## Reference-level explanation

### Byte sources

1. `records`, `parse_records` and every decompressing reader must accept a `std.io.BinaryReader`, and must accept an RFC 142 `AsyncFallibleIterator[bytes, E]`, which is what RFC 066's response bodies are. Over an async source they must be async sources and must read only as the consumer advances. In-memory input uses `std.io.BytesIO`.
2. `std.io.BinaryReader` must provide `skip_bytes(size: int) -> Result[int, IoError]`, which advances by `size` bytes and returns the number skipped, fewer only at the end of input. The trait's provided implementation reads and discards. `std.fs.File`, `std.io.BytesIO` and `PartsReader` must seek instead.
3. Acquiring, owning and closing a reader belong to the caller, under RFC 094.

### Parts

4. `std.fs.open_parts(paths: list[Path]) -> Result[PartsReader, IoError]` must check that the list is nonempty and that every path is a regular file, and must record each part's size, before any read. It must not sort the list.
5. A `PartsReader` must implement `BinaryReader` and RFC 094's `ContextManager`, must deliver every byte of every part in order with nothing added, and must hold at most one part open, closing each at its end. Only the end of the last part is the end of input. Cleanup failures must be preserved under RFC 094.
6. A part that does not end at its recorded size must fail the read with an `IoError` naming the part.
7. `parts.layout()` must return an owned, immutable `PartsLayout` whose `locate(byte_position)` returns the part's path, its index and the one-based offset within it. It takes a position in the bytes the reader delivers, so it does not apply to positions read through a decompressing reader.

### Decompression

8. Every `std.compression` codec module except `snappy.raw` must expose `reader(source, chunk_size=65536, max_window_bytes=134_217_728, max_decoded_bytes=None)`. `std.compression.reader_auto(source, allowed=Codec.all(), ...)` must take the same options and detect the codec under the rules of `decompress_auto`, using only the stream's first bytes. The result must expose its `codec`.
9. Over a `BinaryReader`, a decompressing reader must be a `BinaryReader`. Over an `AsyncFallibleIterator[bytes, E]`, it must be an `AsyncFallibleIterator[bytes, DecompressError[E]]`, whose cases are `Source(cause: E)` and `Codec(cause: CompressionError)`.
10. Working memory must be bounded by `chunk_size`, the codec's state and `max_window_bytes`, independent of the stream's length. A stream that requires a larger window must fail with `CompressionError(kind="window_too_large")` before allocating it. The default is the zstd reference decoder's own 128 MiB limit.
11. Gzip members, zstd frames, bzip2 streams, xz streams and snappy frames that follow one another must decode as their concatenation. A zlib or raw deflate stream ends at its end, and bytes after it must fail with `invalid_data`. The existing `decompress` and `decompress_stream` functions must follow the same rule, so one input never decodes to different bytes through different functions.
12. Input that ends inside a member must fail with `truncated_input`, never a clean end. A checksum or length the format carries must be verified, and a mismatch must fail with `invalid_data`.
13. Decoding past `max_decoded_bytes` must fail with `CompressionError(kind="decoded_limit")`. `None` means no ceiling.
14. Over a `BinaryReader`, a codec failure must surface as an `IoError` retaining its `CompressionError` as the cause under RFC 131, with kind `unexpected_eof` for truncation and `invalid_data` otherwise.

### Selecting records

15. `at` must be a nonempty path of `QName` values from the root element to the parent of the records. A `QName` must hold a namespace URI and a local name; prefixes must not affect matching, and `""` means no namespace.
16. A root element that differs from `at[0]` must fail with `XmlStreamError.UnexpectedRoot`, so a wrong document never reads as an empty one. A deeper path that never occurs yields no records.
17. A record must be a direct child of an element at that path whose name is selected. For `records[T]`, a model selects its `@xml_element` name and an enum of models selects each variant's; a record type with no declared element, or two variants with one name, must be a compile-time diagnostic. For `parse_records`, `names` selects, and `None` selects every child.
18. Elements inside a record belong to it. Elements that are not selected must be checked for well-formedness and skipped without retaining their contents or being charged to `max_record_bytes`.
19. A record must be structurally complete, independently owned, and released when its last owner drops it.
20. `source.root()` must read through the root's start tag and return an owned `XmlStart` with its name and attributes. It must return the same value when called again.

### Typed records

21. `std.serde.xml` must provide `@derive(xml)`, which generates `from_xml(element: XmlElement) -> Result[Self, XmlDecodeError]`. A handwritten implementation with another return type is an ordinary type error. `records[T]` must yield `Result[T, XmlRecordError[XmlDecodeError]]` and may decode without building an `XmlElement`, with identical results.
22. A field must map to the child element of its own name or RFC 021 alias. `xml.attribute=true` must map it to an attribute and `xml.text=true` to the element's character data, which includes CDATA and resolved references and excludes comments and processing instructions. `xml.namespace` must override the namespace: a child field otherwise uses its parent element's namespace, and an attribute field otherwise has none. These are RFC 085 namespaced keys that the derive must check. An unknown `xml.` key, two text fields on one model, or an unsupported field type must be a compile-time diagnostic.
23. `list[T]` must collect repeated children in document order, and `Option[T]` must accept absence. A nested model must decode from its child element and needs no `@xml_element`. Text must convert to scalars by XML Schema's lexical rules for that type.
24. `XmlDecodeError` must mirror RFC 146's `JsonDecodeError`: `MissingField`, `UnknownField`, `DuplicateField` for a repeated child mapped to a non-list field, `WrongType`, `InvalidValue` retaining the original validation failure, `UnexpectedText` for non-whitespace text the model does not map, and `Multiple`, with `coverage()`. Paths must be structured `XmlPath` segments: element with sibling index, attribute and text. Unknown children and attributes must be rejected by default, and `unknown_fields=UnknownFields.Ignore` must tolerate them. Complete reporting and RFC 122's FailFast scope must apply as they do to JSON.

### Dynamic records

25. `parse_records` must yield `Result[XmlElement, XmlRecordError[XmlDecodeError]]`, and the only item failure it can produce is `RecordTooLarge`. `XmlElement` must expose `name`, `attribute(QName)`, `child(QName)`, `children()`, `children_named(QName)`, `text()`, `position` and `to_xml()`, and must own its contents.
26. `to_xml()` must produce a standalone document that declares every namespace binding the element inherited.

### Limits and XML features

27. `max_record_bytes` must be required and positive. A longer record must yield `XmlRecordError.RecordTooLarge`, after which the parser must skip the record while still checking its structure and reading must continue.
28. `max_depth: Option[int]` must default to 128 under RFC 152, and only an explicit `None` lifts it. Exceeding it must fail with `XmlStreamError.TooDeep(path, max_depth)`. It stops the stream, because the parser needs the element stack to match end tags.
29. `max_token_bytes` bounds one tag or attribute value the parser holds outside a retained record, and `max_context_bytes` bounds the open-element and namespace state. Each must default to 1 MiB, and exceeding either must stop the stream with a typed error. `read_bytes=65536` sets the read size; it is buffering, not a limit.
30. The parser must implement XML 1.0 and Namespaces in XML 1.0, and must report a `version="1.1"` declaration as a typed error. It must read UTF-8 and UTF-16 and must decode other declared encodings; an unknown encoding must be a typed error. UTF-8 sequences must not be decoded independently per chunk.
31. The parser must accept a DOCTYPE, must never fetch its external subset or any other resource, and must fail with `XmlStreamError.EntityRefused` on an entity declaration.

### Failure and completion

32. `XmlStreamError` must distinguish `Source(cause: Error)`, `Malformed(position, detail)`, `Incomplete(open_path)`, `UnexpectedRoot(expected, found)`, `TooDeep(path, max_depth)`, `TokenTooLarge(position, max_token_bytes)`, `ContextTooLarge(position, max_context_bytes)`, `EntityRefused(position)` and `UnsupportedDocument(position, detail)`, retaining typed source errors under RFC 131.
33. Advancement must report exhaustion only after the root element has closed and the input has ended, allowing only what XML 1.0 permits after the root. Input ending earlier must fail with `Incomplete`. Under RFC 142, that is `Completed` versus `Stopped` with a source failure; an intentional early stop and cancellation stay distinct from both, and delivered records are never reported as rolled back.
34. An item failure must not be used to continue after a failure that loses structural state, under any error policy.
35. Positions must use the shared `std.io.SourcePosition`, with one-based lines, Unicode-character columns and byte positions, counting the bytes the reader delivers. Through a decompressing reader they count decompressed bytes.
36. Retained failures, excerpts and paths must uphold RFC 103. Reporting all failures must use RFC 142's reporters and must not require an unbounded in-memory list.

### Positions and resumption

37. A reader-backed source must provide `with_positions()`, pairing every item, success or failure, with the `XmlPosition` after it. An `XmlPosition` must carry the one-based `line` and `byte_position` immediately after the record and a check of that record, and must have a stable JSON form. It must not carry parser state.
38. `records` and `parse_records` must accept `after=position` over a `BinaryReader`, and that call must return a `Result` with `XmlResumeError`. It must scan the input before the position with the same parser and the same limits, building no records and running no application code, and must continue line numbering from it.
39. A scan failure must be `XmlResumeError.Stream`. The call must fail with `XmlResumeError.SourceChanged` when the input is shorter than the position, the position is not the end of a record under the selected parent, or that record's check differs. It must never continue from a different document.
40. RFC 146's `after=` must accept any `BinaryReader` through `skip_bytes`, so a compressed JSONL log resumes too.

### Throughput and memory

41. On one thread, on the same machine and file, for both a text-heavy and a markup-dense document, `std.xml` must be at least as fast as CPython's standard library:

    | `std.xml` operation | Must be at least as fast as |
    | --- | --- |
    | Skipping elements that are not selected, and the resume scan | `xml.parsers.expat` with no handlers |
    | `parse_records` building each record | `xml.etree.ElementTree.iterparse`, clearing each record |
    | `records[T]` decoding each record | `xml.etree.ElementTree.iterparse`, clearing each record |

42. Reading through a decompressing reader must add no more than the codec's reference decoder takes for the same bytes.
43. Peak memory must follow the configured limits and must not differ between a small document and a 45 GB document with the same records.

### Conformance workloads

| Workload | Required evidence |
| --- | --- |
| Boundaries | A small document cut at every byte offset, including inside a multibyte character, tag, comment and CDATA section, yields identical records through parts, chunks and compressed members. |
| Composition | Compressed then split, each part compressed, split without compression, and compressed without splitting all read the same, for XML and JSONL. |
| Corrupt streams | Truncation, a bad checksum, trailing bytes after a zlib stream, a window above the limit and a decompression bomb each produce their typed failure. Concatenated members decode fully through `reader`, `decompress` and `decompress_stream`. |
| Selection | Rebinding prefixes, default namespaces, nested elements of the same name and unselected siblings larger than `max_record_bytes` behave as specified; a wrong root fails rather than reads empty. |
| Typed mapping | Attributes, text, repeated and absent children, aliases and namespaces decode as declared; each `XmlDecodeError` case carries its path, and compile-time diagnostics cover unsupported declarations. |
| Failure classes | A bad record continues; a deep, malformed or incomplete document stops; a loop that completes read a closed root. |
| Hostile input | XXE and billion-laughs documents are refused without network or file access; a document with a DOCTYPE and no entity declarations reads. |
| Resumption | Resuming at every record boundary through parts and gzip yields the remaining records exactly once; a changed, shortened or reordered input reports `SourceChanged`. |
| Throughput and memory | The table above holds on both document shapes, with peak memory measured on a document larger than the machine's memory. |

## Design details

### Ownership of the contracts

RFC 142 owns consumers, batching by count, error policies and reporters. RFC 169 owns record sets, field selection and byte-bounded batches. RFC 146 owns JSON and JSONL. RFC 061 owns the existing one-shot and push-style compression functions, and this RFC adds pull-style readers beside them, in the same modules and under the same error model. `std.fs` owns `open_parts` because it opens paths, and it works for every `BinaryReader` consumer. `std.xml` owns XML structure, selection and completion, and imports neither `std.json` nor `std.data`.

`SourcePosition` moves from `std.json` to `std.io`, and `std.json` keeps exporting it with the same identity, so both formats share one location type.

### One family with JSONL

| Concern | JSONL (RFC 146) | XML |
| --- | --- | --- |
| Dynamic records | `parse_jsonl(source)` gives `JsonValue` | `parse_records(source, at=, names=)` gives `XmlElement` |
| Typed records | `jsonl[T](source)` | `records[T](source, at=)` |
| Per-record item | `Result[T, JsonlRecordError[E]]` | `Result[T, XmlRecordError[E]]` |
| Advancement failure | `IoError` | `XmlStreamError` |
| Oversized record | `RecordTooLarge`, then continue | the same |
| Depth | `max_depth=128`, `JsonError.TooDeep` | `max_depth=128`, `XmlStreamError.TooDeep` |
| Positions | `SourcePosition` | the same type |
| Split and compressed input | `open_parts`, `gzip.reader` | the same |
| Resume | `with_positions()`, `after=`, `SourceChanged` | the same |

### Why resumption scans

JSONL can skip to a position because a line boundary carries no state. An XML position does: the open elements and namespace bindings in force there. There are two ways to restore them: save them in the position and seek, or scan the input before the position.

Saving state makes each position larger and ties a stored format to parser internals, and seeking through a compressed stream means decoding it anyway, unless the position also carries the codec's window. Scanning needs none of that. It costs one parse of the input already read, which the measurements below put at about one to two minutes for 45 GB on one core. An import that resumes has lost far more than that in its own work, and the scan also proves that the input still parses to the same point. This is the policy of storing as little as possible and recovering cheaply.

### Measured baseline

Measured on 2026-11-04 on one core of an Apple M5 Max with CPython 3.14.4, over 1 GB of generated XML held in the page cache:

| Operation | Throughput | 45 GB takes |
| --- | --- | --- |
| `expat`, no handlers, text-heavy document | 793 MB/s | 57 s |
| `expat`, no handlers, markup-dense document | 365 MB/s | 123 s |
| `expat` with namespaces, counting records in a handler, markup-dense | 232 MB/s | 194 s |
| `iterparse`, clearing each record, markup-dense | 141 MB/s | 319 s |
| `gzip -d` | 2,371 MB/s | 19 s |
| `zstd -d` | 1,502 MB/s | 30 s |
| `bzip2 -d` | 84 MB/s | 534 s |

Gzip and zstd cost less than parsing. Bzip2 costs more, so a resumed `.bz2` import spends minutes decoding; that is the format's cost, and it remains small beside hours of application work.

### Prior art

Python's `XMLPullParser` and `iterparse` keep parsed elements until the caller clears them, a leak that owned records avoid by construction. Go's `encoding/xml` streams tokens and decodes one subtree into a struct, the same "stream, then decode one record" shape, and its `,attr` and `,chardata` tags inspired the field keys. Spark's XML source selects by a bare `rowTag` and leaves nested elements of the same name unspecified, which an anchored parent path settles. .NET disables DTD processing by default; refusing only entity declarations gives the same protection and still reads real feeds. Rust's `flate2` separates `GzDecoder` from `MultiGzDecoder`, a distinction that silently truncates concatenated files for callers who pick the wrong one, so these readers have one behavior. Zstd's reference decoder supplies the default window limit.

## Alternatives considered

- **Parse each part separately:** fails when a declaration, tag or encoded character crosses a boundary.
- **Search for closing tags:** does not implement XML structure, namespaces or encodings.
- **Build a whole-document tree:** defeats the memory bound.
- **A string path language such as XPath:** adds a language and a parser to answer what a parent path and a record type already answer.
- **Select by bare element name, as Spark does:** matches by accident at other depths and leaves nested matches unspecified.
- **A limits model, `XmlLimits`:** differs from RFC 146's keyword arguments and mixes buffering with limits.
- **A special header and footer API:** root attributes come from `root()`, and a trailer is another record kind.
- **Saving parser state in the position:** see "Why resumption scans".
- **A `SeekableReader` trait for resumption:** a decompressing reader can only emulate seeking by decoding, and a trait bound would have made compressed input unable to resume. `skip_bytes` gives one route, with seeking as an optimization inside it.
- **Saving the codec's window in the position:** ties stored positions to codec internals, with a window of up to the limit per position, to save seconds of decoding.
- **Choosing the codec from the file name:** names are not evidence. The module's existing autodetection reads signatures, and only explicitly.
- **A default ceiling on decoded bytes:** any value small enough to stop a bomb rejects ordinary files. Memory stays bounded either way, and an application that takes untrusted input sets its own ceiling.
- **A throughput statement with no floor:** would let a slower parser pass, and a parser slower than the caller's previous tool will not be adopted.

## Drawbacks

Typed mapping adds a derive, metadata keys and an error family to what could have been a dynamic reader. Owned records copy bytes that a borrowed view would not, which the floor must still absorb. Resuming costs a scan of everything already read, and a `.bz2` input makes that scan slow. The throughput floor binds the implementation to a moving external baseline and may decide how the tokenizer is built. A position's check covers only the record before it, as JSONL's does, so a change earlier in the input that leaves the document parseable to the same point goes undetected.

## Layers affected

- **Stdlib I/O:** `BinaryReader.skip_bytes`, and `SourcePosition` moved from `std.json`.
- **Stdlib filesystem:** `open_parts`, `PartsReader` and `PartsLayout`.
- **Stdlib compression:** `reader` in every codec module, `reader_auto`, `DecompressError`, new error kinds, and concatenated input through the existing functions.
- **Stdlib XML:** the incremental parser, selection, `XmlElement`, limits, positions and failures.
- **Stdlib serde and derive:** `@derive(xml)`, `@xml_element` and the `xml.` field keys, with their compile-time diagnostics.
- **Stdlib JSON:** `after=` over any `BinaryReader`, and async chunk sources, under RFC 146.
- **Documentation:** references for each module, and the conformance workloads above.

## Design Decisions

- **One byte stream:** parts and compression are ordinary readers that compose, and the parser sees only bytes. A part boundary is not the end of a document.
- **Parts:** `std.fs.open_parts(paths)` checks every part up front, holds one open at a time and works for every `BinaryReader` consumer. The caller supplies the order.
- **Compressed input:** every codec has a pull-style `reader` over sync and async sources, plus `reader_auto`, in bounded memory. Concatenated members decode as one stream, truncation and bad checksums fail, and the window has a limit. Decoded size has no default ceiling.
- **Selection:** `at=` names the parent path, and the record type names the elements. Nested elements belong to their record, unselected siblings are skipped, and a wrong root fails.
- **Typed records:** this RFC owns `@derive(xml)`, `records[T]` and `XmlDecodeError`, mirroring RFC 146. `parse_records` and `XmlElement` are the dynamic counterparts.
- **Failures:** a bad or oversized record is an item failure and reading continues; structural, depth and source failures stop the stream. Exhaustion means the root closed and the input ended.
- **Limits:** keyword arguments as in RFC 146. `max_record_bytes` is required, and `max_depth` follows RFC 152.
- **Hostile input:** a DOCTYPE is accepted, nothing external is ever fetched, and entity declarations are refused. UTF-8 and UTF-16 are required.
- **Header and trailer:** `root()` gives the root's start tag, and a trailer is another record kind.
- **Resumption:** `with_positions()` and `after=` as in RFC 146. A position holds no parser state; resuming scans the input before it, through any reader, and fails with `SourceChanged` when that input differs.
- **Async:** any `AsyncFallibleIterator[bytes, E]` is a source, so HTTP bodies work unchanged and backpressure follows from pull-based consumption. `after=` requires a `BinaryReader`.
- **Throughput:** Python's standard library on the same file is the floor, on both document shapes, and peak memory does not grow with document size. How the implementation reaches that floor belongs in a design record.
- **Positions:** `SourcePosition` lives in `std.io` and is shared with JSON.
