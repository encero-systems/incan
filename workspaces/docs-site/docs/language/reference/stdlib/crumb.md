# std.primitives.crumb reference

This module provides bounded integer and byte framing for Crumb profiles.

```incan
from std.primitives.crumb import Budget, CrumbError, CrumbFailure, Reader, encode_signed, encode_unsigned, frame
```

## Request budget

| API | Return type | Contract |
| --- | --- | --- |
| `Budget.create(bytes: int, elements: int, depth: int)` | `Result[Budget, CrumbError]` | Create nonnegative caller-selected limits; there are no implicit defaults. |
| `budget.charge_bytes(count: int)` | `Result[None, CrumbError]` | Consume a byte allowance before reading or producing bytes. |
| `budget.charge_elements(count: int)` | `Result[None, CrumbError]` | Consume logical element or arithmetic-step work. |
| `budget.enter()` | `Result[None, CrumbError]` | Enter one nested value if its depth fits. |
| `budget.leave()` | `Result[None, CrumbError]` | Balance a successful entry; an unmatched leave is invalid input. |

Pass the same mutable budget through a request and its children. Failed operations do not refund work already charged. Depth entry and exit are explicit; callers must balance a successful entry on their error paths too. Byte allowances count protocol processing and buffers, not exact allocator overhead or total resident process memory.

## Integer and payload framing

| API | Return type | Contract |
| --- | --- | --- |
| `encode_unsigned(decimal: str, mut budget: Budget)` | `Result[bytes, CrumbError]` | Encode exact nonnegative decimal digits as shortest unsigned base-128, low group first. |
| `encode_signed(decimal: str, mut budget: Budget)` | `Result[bytes, CrumbError]` | Encode signed digits using zigzag: `2*n` for nonnegative values, `-2*n-1` otherwise. |
| `frame(data: bytes, mut budget: Budget)` | `Result[bytes, CrumbError]` | Return the shortest encoded byte length followed by copied payload bytes. |

Integer input is canonical ASCII decimal: no whitespace, plus sign or leading zeros; zero is `"0"`. Signed input may begin with `-`, but `"-0"` is refused. Values can exceed `u64` within the caller's limits. They are never converted through a floating-point or host-sized integer.

Base-128 uses seven payload bits per octet, with bit 7 set except in the final octet. Zero is `00`; 128 is `80 01`. Encoded payload lengths are separate from arbitrary-precision integer values.

## Owned reader

| API | Return type | Contract |
| --- | --- | --- |
| `Reader.from_bytes(data: bytes)` | `Reader` | Own the input and start at byte zero. |
| `reader.position()` | `int` | Return the next unread byte offset. |
| `reader.remaining()` | `int` | Return the unread byte count. |
| `reader.read_unsigned(mut budget: Budget)` | `Result[bytes, CrumbError]` | Return one validated shortest unsigned integer as its exact encoded bytes, with no numeric-width restriction. |
| `reader.read_length(mut budget: Budget)` | `Result[u64, CrumbError]` | Read the same spelling with the Crumb length limit of `u64`. |
| `reader.read_frame(mut budget: Budget)` | `Result[bytes, CrumbError]` | Read a length and copy exactly that payload; check host size, input bounds and budget before copying. |

Successful reads advance the cursor. Every failed read preserves its original cursor position, including a truncated payload after a valid length. The budget retains charges already spent. Empty payloads are valid and distinct from truncated input. Returned bytes own their storage; this reader does not expose borrowed or independently retained mapped views. Trailing bytes remain available for the next operation.

## Errors

`CrumbError` stores `kind: CrumbFailure` and one input-free `detail: str`. `message() -> str` joins the kind's stable diagnostic prefix with that detail.

| Kind | Detail | Meaning |
| --- | --- | --- |
| `CrumbFailure.InvalidInput` | Rejected operation or contract | Invalid caller argument, decimal spelling, count or unmatched depth exit. |
| `CrumbFailure.Truncated` | Decimal byte offset | Input ended at the reported byte offset before the requested operation completed. |
| `CrumbFailure.Nonminimal` | Decimal byte offset | A base-128 value beginning at the reported offset uses redundant groups. |
| `CrumbFailure.Overflow` | Decimal byte offset | A length beginning at the reported offset exceeds unsigned 64 bits. |
| `CrumbFailure.Limit` | Resource name | The named request allowance or host payload-length representation cannot accommodate the operation. |

## Type descriptions and spellings

The child modules `std.primitives.crumb.types`, `.spelling`, `.identity` and `.admission` expose explicit description inputs. They do not generate reflection hooks, authenticate a provider, load external descriptions or reconstruct a model. A structural view does not replace the complete description metadata or its digest.

`TypeNode` has these variants:

| Variant | Meaning |
| --- | --- |
| `Scalar(ScalarKind)` | One admitted scalar kind: `boolean`, signed/unsigned 8/16/32/64-bit integers, `fp32`, `fp64`, `string`, `binary`, `secret_string`, `secret_bytes` or `reference`. |
| `Decimal(int, int)` | Precision and scale, with positive precision and `0 <= scale <= precision`. |
| `Container(ContainerKind, list[TypeNode])` | `List`, `Set` and `Option` take one argument, `Map` two, `Tuple` any number and `Union` at least one. |
| `Nominal(NominalKind, bytes, list[TypeNode])` | `Record` or `Enum`, a six-byte declaration identity, and ordered closed arguments. This is a selector without structural fields. |
| `Bound(list[TypeNode])` | Exactly two nodes: a `Nominal` selector and its explicit matching-kind `Record` or `Enum` definition. |
| `Record(str, list[FieldShape])` | An explicit structural record; the string is a local recursion key, not a nominal identity. |
| `Enum(str, list[VariantShape])` | An explicit structural enum with the same local-key convention. |
| `Recursive(str)` | A reference to an active record/enum definition; an unbound reference is invalid. |
| `Underlying(list[TypeNode])` | Exactly one underlying node for an alias or constrained newtype. Admission of additional constraints remains with the binding. |
| `Parameter(str)` | An unresolved parameter, representable for diagnostics but refused by spelling and selector operations. |

`FieldShape(identity: bytes, storage_type: TypeNode, canonical: bool, absent: AbsentPolicy)` retains storage facts. `ScalarKind`, `ContainerKind`, `NominalKind` and `AbsentPolicy` are string-valued enums. Their values are respectively the scalar tokens above, the lowercase constructor names, `record` or `enum`, and `required`, `none` or `empty`; `from_value` parses those spellings. `ScalarKind.integer_facts()` returns signedness and bit width for integer members. `VariantShape(identity: bytes, fields: list[FieldShape])` describes one anonymous enum payload. Member identities are exactly six bytes. Display names, properties and declaration metadata remain in the owning complete description. Nominal models require `Bound` nodes; anonymous structural nodes carry no nominal authority.

| API | Return type | Contract |
| --- | --- | --- |
| `selector_spelling(node: TypeNode, mut budget: Budget)` | `Result[str, CrumbError]` | Produce the closed identity-bearing spelling N; supplied bound definitions are checked but their fields do not enter N. |
| `shape_spelling(node: TypeNode, mut budget: Budget)` | `Result[str, CrumbError]` | Produce structural spelling S from explicit definitions; a bare nominal selector cannot supply its missing shape. |

Both functions live in `.spelling`. S sorts members by identity, independently of declaration order, and retains canonical/absence settings. Recursive edges use active ancestor distance; anonymous enum payloads add no ancestor. Distinct admitted union alternatives retain their multiplicity in S even when their structural spellings match. N refuses alternatives that collapse to the same selector. Sorting for a spelling does not admit an unsorted stored field table.

Returned strings own their contents. Each operation shares the supplied work/byte/depth budget, balances its own depth entry on errors and invokes no user hash, comparison or constructor code.

## Member identities and closed selectors

These functions live in `.identity`:

| API | Return type | Contract |
| --- | --- | --- |
| `member_identity(earliest_name: str, mut budget: Budget)` | `Result[bytes, CrumbError]` | First six SHA-256 bytes of the exact UTF-8 name; no normalization or terminator. |
| `declaration_preimage(registry: str, package: str, earliest_path: str, mut budget: Budget)` | `Result[bytes, CrumbError]` | Three exact UTF-8 strings, each followed by a zero byte; embedded NUL bytes are invalid. |
| `declaration_identity(registry: str, package: str, earliest_path: str, mut budget: Budget)` | `Result[bytes, CrumbError]` | First six SHA-256 bytes of that independently assembled preimage. |
| `closed_selector(node: TypeNode, mut budget: Budget)` | `Result[bytes, CrumbError]` | For a nominal node, return K: identity, unsigned argument count and individually framed N arguments. An underlying wrapper is folded first. |
| `check_arity(node: TypeNode, expected: int)` | `Result[None, CrumbError]` | Check a nominal node's argument count against an independently supplied binding contract; negative expectations are invalid. |

These functions return owned bytes. A nongeneric K still includes a zero argument count. Parameter names and import aliases do not enter K. Neither a valid selector nor its digest proves registry authority, resolves a collision or authorizes instantiating a type. The binding supplies and checks declaration kind, arity, argument constraints and authority separately.

## Structural adaptation plans

These types and functions live in `.admission`:

| API | Return type | Contract |
| --- | --- | --- |
| `check_written(fields: list[FieldShape], mut budget: Budget)` | `Result[None, AdmissionError]` | Validate the whole written table, including nested tables, exact identity widths, strict stored order, uniqueness, supported types and absence policies. |
| `plan_adaptation(written: RecordShape, requested: RecordShape, mut budget: Budget)` | `Result[list[FieldPlan], AdmissionError]` | Check matching nominal bindings, validate both shapes, then produce requested fields in declaration order. |

`RecordShape(nominal: bytes, fields: list[FieldShape])` is an explicit structural view. Its nonempty nominal bytes come from the admitted binding; this helper neither parses nor authenticates that identity. Written fields are strictly identity-ordered; requested fields retain declaration order. All written fields are validated even when the request drops them. Nested nominal fields use `Bound` so unrelated types cannot match through structural similarity.

`FieldPlan(requested: FieldShape, source: Option[int], conversion: ValuePlan)` selects a written field index or an explicitly admitted absent value. `None` means outer optional absence or an empty list/set/map, only when the requested field permits it. A nonempty constructor default is not used. A present incompatible value is never replaced by absence.

`ValuePlan` retains these steps: `Identity`, `Widen(str)`, `Wrap(list[ValuePlan])`, `Optional(list[ValuePlan])`, `Container(ContainerKind, list[ValuePlan])`, `Record(list[FieldPlan])`, `Recur(int)`, `Enum(list[VariantPlan])` and `Union(list[MemberPlan])`. `Wrap` and `Optional` carry one child; other container children follow their type arguments. `Recur` selects an already active record/enum conversion by ancestor distance, keeping the plan finite. `VariantPlan(identity: bytes, fields: list[FieldPlan])` retains variant identity. `MemberPlan(written_index: int, requested_index: int, conversion: ValuePlan)` retains a union branch's original and requested positions.

Plans permit declared integer range inclusion, `fp32` to `fp64`, optional wrapping and recursively compatible fields/containers. They refuse narrowing, optional unwrapping, unknown written variants and missing union discriminators. They do not inspect a small row value to justify narrowing. No model is constructed or partially exposed by planning: a consumer must still validate actual values, interpret these steps and run the original model's final validation. Plans own their retained descriptor data; this API is not a borrowed cell view.

`AdmissionError` stores `kind: AdmissionFailure`, `path: str` and `cause: Option[CrumbError]`. `AdmissionFailure` is a string-valued enum whose values are `IdentityWidth`, `DuplicateIdentity`, `IdentityOrder`, `InvalidType`, `NominalMismatch`, `MissingField`, `IncompatibleType`, `MissingDescription` and `ResourceLimit`. Only `ResourceLimit` sets `cause`; `.message() -> str` renders `<kind> at <path>` without stored contents. Paths use identities, such as `/fields/82244417f956`, rather than treating a display-name match as identity evidence.

Description-digest verification, full metadata admission, display-name rename hints, logical hash projections, codecs and generated-model reconstruction are separate work. Structural success alone must not be advertised as complete-description or record admission.

## Logical hashes

`std.primitives.crumb.hashes` hashes explicit logical values. Its input types live in `.values`. These APIs do not parse COMIC text or Crumb row/column bytes, generate model hooks, resolve reference targets or authorize reconstruction. A binding supplies already admitted model values, including its range, constraint and nominal checks.

| API | Return type | Contract |
| --- | --- | --- |
| `value_hash(value: Value, projection: Projection, mut budget: Budget, capture: bool = false)` | `Result[HashResult, CrumbError]` | Validate original logical values, then hash the object or canonical contribution. |
| `model_hash(node: TypeNode, mut budget: Budget, capture: bool = false)` | `Result[HashResult, CrumbError]` | Hash the framed structural S spelling from explicit definitions. |
| `description_hash(description: Metadata, mut budget: Budget, capture: bool = false)` | `Result[HashResult, CrumbError]` | Hash every entry in the complete metadata tree. |
| `verify_description_digest(description: Metadata, expected: bytes, mut budget: Budget)` | `Result[None, CrumbError]` | Derive the complete metadata digest and compare it with an exactly 32-byte expectation; mismatch is invalid input. |
| `seal_hash(values: list[Value], mut budget: Budget, capture: bool = false)` | `Result[HashResult, CrumbError]` | Hash the ordered, individually framed object contributions. |

`HashResult` contains `digest: bytes` and `preimage: Option[bytes]`. Digests are SHA-256 bytes. Ordinary hashing updates incremental state without retaining the whole preimage. Explicit `capture=true` retains and returns an independent preimage within the supplied budget. No error returns a partial digest. A seal counts each repeatable contribution before streaming its frame. It is not an incremental RecordStream writer.

Domains start with the exact bytes `crumb1`, zero, then purpose byte: object `01`, canonical `02`, model `03`, seal `04`, description `05`. A description digest checks integrity of the supplied complete tree; it is not authentication, binding authority, property-semantic admission or a model's validation.

### Logical inputs

`Projection` is a string-valued enum: `Object = "object"` and `Canonical = "canonical"`; `from_value` parses those corpus spellings. `Value` has these variants:

| Variant | Input contract |
| --- | --- |
| `NoneValue`, `Some(list[Value])` | Some has exactly one child. Nested absent states remain distinct; present optional wrappers do not change their child's contribution. |
| `Boolean(bool)`, `Integer(str)` | Integers use canonical exact decimal strings and may exceed host width. |
| `Float(int, bytes)` | Width is 32 or 64; bytes are the corresponding IEEE bits in big-endian order. Binary32 widens exactly. All NaNs normalize; signed zeros remain distinct. |
| `Decimal(str, str)` | Exact coefficient and scale. Trailing coefficient zeros are removed and scale adjusted without host-width limits; zero normalizes to coefficient and scale zero. |
| `Text(str)`, `Binary(bytes)`, `Withheld` | Text is exact UTF-8 with no Unicode normalization. Withheld is a distinct contribution, not an empty value. |
| `List(list[Value])`, `Tuple(list[Value])`, `Set(list[Value])` | Lists and tuples retain order. Sets are ordered by stored-value comparison; duplicate original values are invalid. |
| `Map(list[MapEntry])` | `MapEntry(key: Value, value: Value)` retains original entries; duplicate original keys are invalid. |
| `Record(list[ValueField])` | Fields contribute in identity order. Duplicate or non-six-byte identities are invalid. |
| `Enum(bytes, list[ValueField])` | An exactly six-byte variant identity and its anonymous record payload. |
| `Union(TypeNode, list[Value])` | An explicit selected closed member and exactly one payload; the selector remains significant even for equal payloads. |
| `Reference(bytes, bytes)` | Opaque target-type identity and reference identity; target contents are not read. |

`ValueField(identity: bytes, canonical: bool, absent: AbsentPolicy, value: Value)` describes a stored field value. Outer `NoneValue` is omitted only for `NoneValue` absence policy; an empty list/set/map is omitted only for `Empty`. Canonical projection also omits fields with `canonical=false`, recursively. Every original field value is checked before omission. These values do not carry declared scalar widths or complete model constraints; their owning binding must check those before calling this layer.

Stored comparison is numeric for integers and decimals, preserves signed-zero order and places normalized NaN after positive infinity. It is not lexicographic comparison of encoded contribution bytes. Map/set projection happens before ordering. Distinct original objects that become equal canonically retain their multiplicity; projected map-key ties compare projected values. Unrelated value kinds require an explicit union discriminator instead of implicit cross-type ordering.

### Complete metadata inputs

`Metadata` supports `Null`, `Boolean(bool)`, `Integer(str)`, `Text(str)`, `Binary(bytes)`, `List(list[Metadata])` and `Map(list[MetadataEntry])`. `MetadataEntry(key: str, value: Metadata)` uses exact string keys. Duplicate keys are refused; map entries encode in UTF-8 key order, while lists retain order. Every supplied property participates in the digest, including properties this layer does not interpret. A consumer must separately admit any property interpretation that changes value decoding or reconstruction. Uninterpreted metadata remains data and is retained in full.

## Profile kind registrations

`std.primitives.crumb.registrations` supplies local shape and printing-order facts. It does not load a profile, resolve its type names or authorize code execution.

| Type | Fields |
| --- | --- |
| `PropertyGroupOrder` | `name: str`, `order: list[str]` |
| `KindRegistration` | `kind: str`, `shape: NominalKind`, `property_order: Option[list[str]]`, `property_groups: list[PropertyGroupOrder]` |

`NominalKind` comes from `.types` and is `Record` or `Enum`. `property_order=None` means the profile's property ordering is unknown; `Some([])` supplies a known empty order. Group orders apply independently to the named nested property. These lists are ordering facts, not lists of allowed properties. Codecs retain unknown properties as data in their original read order, including unknown nested groups.

| API | Return type | Contract |
| --- | --- | --- |
| `KindRegistry.create(registrations: list[KindRegistration], mut budget: Budget)` | `Result[KindRegistry, CrumbError]` | Retain a checked local registry. Reject empty names, duplicate kinds, duplicate group or ordering names, and attempts to redefine core kinds. |
| `registry.shape(kind: str, mut budget: Budget)` | `Result[NominalKind, CrumbError]` | Resolve core `Record`/`Enum` or an explicitly registered profile kind. Unknown profile kinds are invalid input. |
| `registry.registration(kind: str, mut budget: Budget)` | `Result[Option[KindRegistration], CrumbError]` | Return retained profile facts, or `None` for a core kind. Unknown profile kinds fail. The codec supplies the core kinds' property orders. |

All calls share the request budget. Construction and returned registration copies charge name storage and list work before allocation; name accounting reserves a UTF-8 upper bound. Registration does not check a description's field table, absence rules, identities or value-changing property interpretations. Those checks remain mandatory at the codec's admission boundary. A registered shape with unknown profile semantics can retain metadata without treating it as executable behavior.

## Finite row writer

`std.primitives.crumb.row_writer` writes one complete experimental `crumb-row/1` envelope. It accepts explicit final stored values and complete descriptions. It does not invoke constructors, normalize virtual construction inputs, fetch descriptions or resolve reference targets.

| Type | Fields |
| --- | --- |
| `.descriptions.Description` | `metadata: Metadata`, `mode: DescriptionMode` |
| `.descriptions.DescriptionMode` | `Inline` or `Reference` |
| `.records.RowRecord` | `key: bytes`, `value: Value` |

`key` is the complete closed nominal selector K, including its argument framing. A record's `Value.Record` fields retain their exact stored identities, canonical flags and absent policies. Enum roots use `Value.Enum` with their stored variant identity. A reference-mode description still supplies its complete metadata to the writer; the envelope carries K and the derived complete-description digest instead of inline metadata.

```incan
from std.primitives.crumb.records import RowRecord
from std.primitives.crumb.row_writer import write_rows
from std.primitives.crumb.descriptions import Description, DescriptionMode
```

| API | Return type | Contract |
| --- | --- | --- |
| `write_rows(descriptions: list[Description], records: list[RowRecord], registry: KindRegistry, sealed: bool, mut budget: Budget)` | `Result[bytes, CrumbError]` | Admit the entire dictionary, check every typed stored value, then return one independent owned envelope. All arguments are explicit; there is no default sealing or budget policy. |

The complete metadata has the core `kind`, `nominal`, `path` and `properties` fields plus `fields` or `variants`. Stored fields have `id`, `name`, `type`, `canonical`, `absent` and `properties`. Names are display information; exact identities determine dictionary and member lookup. Unknown metadata is retained, including in inline headers. Unknown kinds need an explicit registry entry; unsupported value-changing interpretations still need an admitted binding.

Dictionary entries and stored members use identity order while their complete metadata retains declaration order. Every nominal dependency must exist in this local dictionary before any rows are written. Missing descriptions, duplicate identities, wrong scalar ranges, wrong field types or mismatched canonical/absence flags return `CrumbError.InvalidInput`. Invalid shapes cannot be hidden by an empty record set or by an omitted absent field. Resource failures return `CrumbError.Limit`; no error returns a successful output prefix.

Presence bits remove only the declared outer absence. Nested `Some(None)` remains distinct from absence. Maps and sets use the primitive's stored-value comparison and reject duplicate original entries. Numeric conversion is exact: integer widths are range checked, decimal rescaling refuses rounding, and binary64 values must be exactly representable when stored as binary32. NaNs normalize and signed zero remains distinct.

`sealed=true` adds the count and ordered logical-value seal. It does not hash physical row bytes or authenticate a sender. This API builds a finite owned result under the shared request budget. It does not promise bounded-memory streaming, spooling, a column codec or a decoder.

## Finite row reader and field access

`std.primitives.crumb.row_reader` reads the same experimental row profile. It admits a complete finite envelope before returning a `RowSet`, including every reachable typed cell and any logical seal. Dictionary references resolve only from the supplied complete descriptions. There is no implicit fetch, approximate shape match or application constructor call.

```incan
from std.primitives.crumb.row_reader import RowSet, read_rows, read_prefix
from std.primitives.crumb.encoding_errors import BinaryError, BinaryFailure
from std.primitives.views import MappedView
```

| API | Return type | Contract |
| --- | --- | --- |
| `read_rows(view: MappedView, supplied: list[Description], registry: KindRegistry, require_sealed: bool, mut budget: Budget)` | `Result[RowSet, BinaryError]` | Admit exactly one complete envelope. Any bytes after its framed boundary are trailing data. Every argument is explicit. |
| `read_prefix(view: MappedView, supplied: list[Description], registry: KindRegistry, require_sealed: bool, mut budget: Budget)` | `Result[tuple[RowSet, int], BinaryError]` | Admit one complete leading envelope and return its consumed byte length. Remaining bytes belong to the caller; this does not report transport completion. |
| `rows.len()` | `int` | Return the admitted record count. |
| `rows.is_sealed()` | `bool` | Report whether the complete logical seal was verified, not merely whether a flag was present. |
| `rows.descriptions(mut budget: Budget)` | `Result[list[Description], BinaryError]` | Copy complete written descriptions and original inline/reference modes, retaining unknown metadata and declaration order. |
| `rows.read_record(index: int, mut budget: Budget)` | `Result[RowRecord, BinaryError]` | Explicitly materialize one complete format value with its original K. No application constructor, normalization or final model validation runs. |
| `rows.field_view(index: int, identity: bytes, mut budget: Budget)` | `Result[Option[MappedView], BinaryError]` | Return an independently retained encoded cell range. `None` means declared stored absence. An unknown field is an error. |
| `rows.read_field(index: int, identity: bytes, mut budget: Budget)` | `Result[Value, BinaryError]` | Materialize only the selected stored field after complete admission; declared absence becomes its typed `Value`. |

Record indexes are zero-based and nonnegative. A field identity is exactly six bytes. Enum records select fields from their admitted variant payload. Selection binary-searches identity order, counts preceding present eligible bits and reads two directly addressed offsets. The offset lookup does not scan preceding offsets; bitmap rank still scans preceding bitmap bytes. It is not a constant-time lookup claim for arbitrarily wide records.

A `RowSet` retains its backing and row indexes, not all decoded records. Admission may temporarily materialize one row's format-value tree for validation, ordering and sealing. Selection does not reconstruct neighboring fields. The shared byte/work/depth budget governs both admission and subsequent explicit operations; callers may supply a new budget for later access. These allowances are not an exact allocator or process-memory bound.

A returned `MappedView` independently retains the complete backing, even when the original owner or `RowSet` leaves scope. It is not a lifetime-bound borrowed model reference, and it does not copy the selected bytes. `view.to_bytes()` explicitly copies. Keeping a small view can therefore keep a much larger backing alive. A mapped file's stable contents remain the caller/store's responsibility; retaining a handle does not prevent another process from changing a file. Use immutable owned backing when that guarantee cannot be made.

An absent-policy optional cell omits its outer option tag. `field_view` returns that encoded inner cell; `read_field` restores the outer `Some`. Neither operation applies constructor defaults or virtual construction inputs. Binding-specific final validation and reconstruction remain separate from the byte codec.

### Row failures

`BinaryError` contains `kind: BinaryFailure`, `offset: int`, `field: Option[bytes] = None`, `variant: Option[bytes] = None` and `cause: Option[CrumbError] = None`. Offsets are relative to the supplied view. Field and variant identities identify the deepest available context. `message() -> str` reports the category and position without including input payloads.

| `BinaryFailure` categories | Meaning |
| --- | --- |
| `BadMagic`, `UnsupportedProfile`, `ReservedFlags` | Invalid header or unsupported codec/value profile/dictionary mode. |
| `NoncanonicalInteger`, `IntegerOverflow`, `Truncated`, `TrailingBytes` | Nonminimal number, length beyond u64, incomplete frame or bytes beyond an exact boundary. A valid u64 length beyond the host range is `LimitExceeded`. |
| `MissingDescription`, `DescriptionMismatch`, `DuplicateSelector`, `DescriptionOrder`, `DescriptionIndex` | Unavailable exact description, mismatched digest, duplicate/unsorted K or an out-of-range dictionary selection. |
| `InvalidDescription`, `InvalidSelector`, `DuplicateMember`, `MemberPosition`, `MemberOrder` | Invalid complete metadata/type facts, duplicate member identity, invalid declaration-position permutation or unsorted wire members. |
| `InvalidOffsets`, `InvalidOffsetWidth`, `NonminimalOffsetWidth`, `BitmapPadding` | Invalid cell boundaries, unsupported/nonminimal table width or set unused presence bits. |
| `InvalidCell`, `InvalidUtf8`, `RedundantPresence`, `NoncanonicalNan` | Typed bytes violate their written contract. Required empty collections remain valid; redundant present empty absent-policy collections do not. |
| `ArityMismatch`, `DuplicateKey`, `CollectionOrder`, `UnionIndex`, `VariantIndex` | Wrong tuple size, duplicate/descending map or set entries, or an invalid union/enum selection. |
| `SealRequired`, `SealCount`, `SealMismatch` | Missing required seal, wrong trailer count or a mismatched logical seal. No set is returned. |
| `LimitExceeded` | Shared resource allowance or host representation exhausted. |
| `MissingField`, `RecordIndex` | Invalid field identity/unknown field or a negative/out-of-range record lookup. |

No failed finite admission exposes a partial set or unchecked view. The prefix API still requires a complete envelope; a paused transport is not successful completion. This row surface does not provide an incremental record-yielding stream or generated model reconstruction. The finite column reader is described below.

## COMIC text

`std.primitives.crumb` exports the bounded COMIC reader, canonical printer and their inert typed carriers. The reader accepts UTF-8 data and never loads code, follows references or evaluates declaration properties.

| API | Return type | Contract |
| --- | --- | --- |
| `TextContext.create(registrations: list[KindRegistration], admitted_type_names: list[str], mut budget: Budget)` | `Result[TextContext, CrumbError]` | Build a text context through the checked shared kind registry. Registration names and ordering facts consume the supplied budget. |
| `TextContext.core(mut budget: Budget)` | `Result[TextContext, CrumbError]` | Build a context containing only the core `Record` and `Enum` kinds. |
| `read_comic(data: bytes, context: TextContext, mut budget: Budget)` | `Result[TextDocument, TextError]` | Decode one UTF-8 COMIC snapshot. Input bytes, retained token bytes, token elements and nesting consume the shared budget. Invalid UTF-8, malformed grammar, unknown or duplicate declarations, type mismatches, ambiguous selections and limit exhaustion return `TextError`. |
| `read_comic_text(source: str, context: TextContext, mut budget: Budget)` | `Result[TextDocument, TextError]` | Read an already decoded string under the same token, element and nesting limits. Callers that begin with bytes use `read_comic` so UTF-8 validation and the input-byte charge remain part of the operation. |
| `print_comic(document: TextDocument, context: TextContext, mut budget: Budget)` | `Result[bytes, TextError]` | Return the unique canonical UTF-8 form with LF separators and a final newline. Output bytes and traversed document elements consume the shared budget. Invalid typed carriers, unknown kinds and limit exhaustion return `TextError`. |

`TextError(kind: str, position: TextPosition)` reports a stable refusal category. `TextPosition(line: int, column: int)` is one-based and counts Unicode scalar values. `TextError.message() -> str` returns `"<kind> at <line>:<column>"`. Printer errors use position `1:1`.

`TextDocument(descriptions: list[TextDescription], values: list[TextValue])` owns descriptions and root values. Descriptions are printed by path and kind; values retain their document order, and consecutive root values of one type print as one table.

| Carrier | Fields or variants |
| --- | --- |
| `TextDescription` | `path: str`, `kind: str`, `shape: str`, `properties: list[TextProperty]`, `fields: list[TextField]`, `variants: list[TextVariant]` |
| `TextField` | `name: str`, `storage_type: TextType`, `absent: AbsentPolicy`, `canonical: bool`, `properties: list[TextProperty]` |
| `TextVariant` | `name: str`, `fields: list[TextField]` |
| `TextProperty` | `name: str`, `value: TextPropertyValue` |
| `TextNamedValue` | `name: str`, `value: TextValue` |
| `TextMapEntry` | `key: TextValue`, `value: TextValue` |

`TextType` has `Scalar(str)`, `Decimal(int, int)`, `List(list[TextType])`, `Set(list[TextType])`, `Map(list[TextType])`, `Option(list[TextType])`, `Union(list[TextType])`, `Reference(list[TextType])` and `Named(str)`. The list-backed constructor variants contain their declared fixed arity: one item for list, set, option and reference; two for map; one or more for union.

`TextPropertyValue` has `Symbol(str)`, `Text(str)`, `Boolean(bool)`, `Integer(str)`, `List(list[TextPropertyValue])` and `Group(list[TextProperty])`. Symbols and groups remain inert. A known registration supplies canonical outer and nested property orders; an unknown order retains read order.

`TextValue` has `NoneValue`, `Some(list[TextValue])`, `Boolean(bool)`, `Integer(str, str)`, `Float(int, bytes)`, `Decimal(str, int, int)`, `Text(str)`, `Binary(bytes)`, `Withheld(str)`, `List(list[TextValue])`, `Set(list[TextValue])`, `Map(list[TextMapEntry])`, `Record(str, list[TextNamedValue])`, `Enum(str, str, list[TextNamedValue])`, `Union(str, list[TextValue])` and `Reference(str, str)`. Integer values retain their exact storage type and decimal digits. Float values retain width and network-order IEEE bits; printing derives the shortest canonical spelling from those bits. Decimal values retain coefficient, precision and scale. Withheld secret values print only as `<secret>`.

Comments are discarded. A cut between complete entries yields the shorter document; a cut inside a value or table returns `unexpected_end`. Raw Unicode bidirectional controls are refused everywhere and must use the canonical escaped spelling in string data. A reference retains its target type and identity text without resolution.

## Finite column writer

`std.primitives.crumb.column_writer` writes one complete experimental `crumb-columns/1` envelope from the same `Description` and `RowRecord` inputs as the row writer. It admits complete descriptions and all final stored values before returning owned bytes. It does not construct application models, resolve references, normalize constructor inputs or change logical record order.

```incan
from std.primitives.crumb.column_writer import write_columns
from std.primitives.crumb.column_types import BlockEncoding, ColumnChoice, GroupChoice
```

| API | Return type | Contract |
| --- | --- | --- |
| `write_columns(descriptions: list[Description], records: list[RowRecord], registry: KindRegistry, sealed: bool, choices: list[GroupChoice], mut budget: Budget)` | `Result[bytes, CrumbError]` | Return one complete codec-2 envelope or an error. All arguments are explicit. An empty `choices` list selects indexed typed cells throughout; a nonempty list must provide every maximal consecutive nominal group's complete column/block choices. |

`GroupChoice(columns: list[ColumnChoice])` follows field-identity order for record fields, or has one column containing complete root-enum cells. `ColumnChoice(blocks: list[BlockEncoding])` has one choice per 256 logical row positions, including absent positions. An empty record type has zero field columns but still counts as a logical record. Adjacent records with the same K form one group; nonadjacent runs remain separate.

| `BlockEncoding` | Physical form | Stored types |
| --- | --- | --- |
| `Indexed` | Minimum-width offsets plus typed row cells | All row cell types |
| `FrameOfReference` | Minimum exact value plus packed unsigned differences | Fixed integers and decimal coefficients |
| `ZigzagDelta` | First value plus packed exact signed differences | Fixed integers and decimal coefficients |
| `Runs` | Positive lengths and canonical typed cells | All row cell types |
| `DictionaryPacked` | Packed codes into the shared column dictionary | String, binary, fixed integers and decimal coefficients |
| `DictionaryRuns` | Positive lengths and dictionary codes | The same dictionary types |

A group column shares one sorted distinct-key dictionary whenever any block chooses a code encoding. It includes values occurring in all of that column's blocks, including blocks using other encodings. String/binary keys use longest byte prefixes; numeric keys use exact positive differences. Both restart every 16 keys. Blocks do not create independent dictionaries. Counts and widths can exceed 255 where the profile permits; caller budgets still apply.

Only the declared outer absence is removed. `Some(None)` remains present, a required empty collection retains its typed cell, and a block containing only absent values requires `Indexed`. Invalid choice cardinalities, incompatible encodings, wrong field identities/contracts, invalid stored values and missing descriptions return `CrumbError.InvalidInput`. Resource failures return `CrumbError.Limit`. No failure returns a successful byte prefix.

Every selected encoding has exact canonical bytes, but column bytes are not canonical across choices or writers. Two choices can encode the same records differently. The optional seal hashes ordered logical values exactly as the row codec does; it is independent of group packing, dictionaries and encoding IDs. It does not authenticate the sender.

This finite writer transposes groups in memory under the shared request budget. It does not promise minimum-size encoding, streaming output, spooling or native-array borrowing. Byte/work allowances are explicit protocol accounting rather than an exact allocator or resident-memory bound.

## Finite column reader

`std.primitives.crumb.column_reader` admits a complete experimental `crumb-columns/1` envelope before exposing a `ColumnSet`. Admission checks every block and dictionary, including unselected data, and verifies a supplied logical seal. Descriptions, typed cells and logical hashes use the same contracts as the row codec; no application model constructors or normalization hooks run.

```incan
from std.primitives.crumb.column_reader import ColumnSet, read_columns, read_prefix
from std.primitives.crumb.column_projection import ColumnField
from std.primitives.crumb.column_errors import ColumnError, ColumnFailure
```

| API | Return type | Contract |
| --- | --- | --- |
| `read_columns(view: MappedView, supplied: list[Description], registry: KindRegistry, require_sealed: bool, mut budget: Budget)` | `Result[ColumnSet, ColumnError]` | Admit exactly one complete envelope, rejecting trailing bytes. Every argument is explicit. |
| `read_prefix(view: MappedView, supplied: list[Description], registry: KindRegistry, require_sealed: bool, mut budget: Budget)` | `Result[tuple[ColumnSet, int], ColumnError]` | Admit a complete leading envelope and return its consumed byte length. This is an explicit framed boundary, not transport completion or partial admission. |
| `columns.len()` | `int` | Return the logical record count, including zero-field records. |
| `columns.is_sealed()` | `bool` | Report successful complete logical-seal verification. |
| `columns.descriptions(mut budget: Budget)` | `Result[list[Description], ColumnError]` | Copy complete descriptions and their original inline/reference modes. |
| `columns.choices(mut budget: Budget)` | `Result[list[GroupChoice], ColumnError]` | Return admitted physical encoding choices for explicit original-shape re-encoding. |
| `columns.read_record(index: int, mut budget: Budget)` | `Result[RowRecord, ColumnError]` | Materialize one format record and its nominal key. |
| `columns.field(index: int, identity: bytes, mut budget: Budget)` | `Result[ColumnField, ColumnError]` | Retain the selected block, backing and type/dictionary facts independently of the set's scope. |
| `columns.read_field(index: int, identity: bytes, mut budget: Budget)` | `Result[Value, ColumnError]` | Select and materialize one field. Unknown identities differ from valid declared absence. |
| `field.read(mut budget: Budget)` | `Result[Value, ColumnError]` | Decode an independently retained selection under an explicit access budget. |

Record positions are zero-based and field identities are six bytes. Group lookup binary-searches logical prefixes; a column selects its 256-position block directly. Packed values use addressed bits. Delta values replay at most the selected block's preceding deltas; runs scan only the selected block. Dictionary lookup begins at the selected 16-key restart. These are bounded accesses, not a constant-time claim for all representations. Root-enum columns contain complete enum cells, so selecting one payload field can decode that one enum value.

Packed values do not necessarily occupy an independent byte slice. `ColumnField` therefore provides explicit decoding rather than pretending to be a native-array borrow or an encoded row-cell view. It retains the complete underlying allocation, and can keep a large envelope alive. Mapped-file contents must remain stable through the caller/store's own protocol; a retained mapping cannot prevent external writes. The set retains indexes and encoded backing rather than all decoded records. Admission temporarily materializes bounded blocks, and seal checking may revisit selected values within a block.

Both admission and later access consume shared byte/work/depth allowances. Reconstructed dictionary prefixes and repeated run payloads are charged, not merely their compact physical bytes. Callers can supply fresh access budgets after admission. Allowances are conservative protocol accounting, not an exact allocator or resident-memory bound. Original-shape re-encoding uses both descriptions and `choices()`; equal logical records alone do not imply equal column bytes.

### Column failures

`ColumnError` contains `kind: ColumnFailure`, `offset: int`, `field: Option[bytes] = None`, `variant: Option[bytes] = None` and `cause: Option[RowError] = None`. Offsets are relative to the supplied view. Shared framing, description, cell, seal and resource refusals retain their typed row cause; field and variant context identifies the deepest available member. `message() -> str` reports categories and positions without input payloads.

| `ColumnFailure` categories | Meaning |
| --- | --- |
| `Shared` | A preserved shared row failure, including resource exhaustion. |
| `UnsupportedEncoding`, `EncodingType`, `EmptyEncoding` | Unknown encoding, incompatible stored type or non-indexed encoding for an all-absent block. |
| `NonzeroPadding`, `NonminimalBits`, `NonminimalBase`, `NonminimalDelta`, `ValueOverflow` | Invalid packed padding, nonminimal numeric representation or reconstructed value outside its declared type. |
| `RunLength`, `AdjacentRuns` | Invalid positive run counts/lengths, wrong total or redundant adjacent runs. |
| `DictionaryCode`, `MissingDictionary`, `DictionaryMembership`, `UnusedDictionary` | Invalid code, unavailable keys, keys not exactly matching all column values or a dictionary without a code-encoded block. |
| `NonmaximalPrefix`, `DictionaryOrder`, `InvalidPrefix`, `RestartOffset`, `RestartPrefix` | Nonminimal key encoding, unordered keys, invalid prefix or restart. |
| `NonmaximalGroup`, `BlockCount` | Adjacent groups with the same nominal identity or an inconsistent block count. |

A failed finite admission returns no partial set or unchecked selection. This surface does not provide incremental record yields, application model reconstruction or automatic typed-array borrowing.
