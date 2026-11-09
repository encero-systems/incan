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
| `Scalar(str)` | One admitted scalar token: `boolean`, signed/unsigned 8/16/32/64-bit integers, `fp32`, `fp64`, `string`, `binary`, `secret_string`, `secret_bytes` or `reference`. |
| `Decimal(int, int)` | Precision and scale, with positive precision and `0 <= scale <= precision`. |
| `Container(ContainerKind, list[TypeNode])` | `List`, `Set` and `Option` take one argument, `Map` two, `Tuple` any number and `Union` at least one. |
| `Nominal(NominalKind, bytes, list[TypeNode])` | `Record` or `Enum`, a six-byte declaration identity, and ordered closed arguments. This is a selector without structural fields. |
| `Bound(list[TypeNode])` | Exactly two nodes: a `Nominal` selector and its explicit matching-kind `Record` or `Enum` definition. |
| `Record(str, list[FieldShape])` | An explicit structural record; the string is a local recursion key, not a nominal identity. |
| `Enum(str, list[VariantShape])` | An explicit structural enum with the same local-key convention. |
| `Recursive(str)` | A reference to an active record/enum definition; an unbound reference is invalid. |
| `Underlying(list[TypeNode])` | Exactly one underlying node for an alias or constrained newtype. Admission of additional constraints remains with the binding. |
| `Parameter(str)` | An unresolved parameter, representable for diagnostics but refused by spelling and selector operations. |

`FieldShape(identity: bytes, storage_type: TypeNode, canonical: bool, absent: AbsentPolicy)` retains storage facts. `ContainerKind`, `NominalKind` and `AbsentPolicy` are string-valued enums. Their values are respectively the lowercase constructor names, `record` or `enum`, and `required`, `none` or `empty`; `from_value` parses those spellings. `VariantShape(identity: bytes, fields: list[FieldShape])` describes one anonymous enum payload. Member identities are exactly six bytes. Display names, properties and declaration metadata remain in the owning complete description. Nominal models require `Bound` nodes; anonymous structural nodes carry no nominal authority.

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
| `.row_descriptions.Description` | `metadata: Metadata`, `mode: DescriptionMode` |
| `.row_descriptions.DescriptionMode` | `Inline` or `Reference` |
| `.row_writer.RowRecord` | `key: bytes`, `value: Value` |

`key` is the complete closed nominal selector K, including its argument framing. A record's `Value.Record` fields retain their exact stored identities, canonical flags and absent policies. Enum roots use `Value.Enum` with their stored variant identity. A reference-mode description still supplies its complete metadata to the writer; the envelope carries K and the derived complete-description digest instead of inline metadata.

```incan
from std.primitives.crumb.row_writer import RowRecord, write_rows
from std.primitives.crumb.row_descriptions import Description, DescriptionMode
```

| API | Return type | Contract |
| --- | --- | --- |
| `write_rows(descriptions: list[Description], records: list[RowRecord], registry: KindRegistry, sealed: bool, mut budget: Budget)` | `Result[bytes, CrumbError]` | Admit the entire dictionary, check every typed stored value, then return one independent owned envelope. All arguments are explicit; there is no default sealing or budget policy. |

The complete metadata has the core `kind`, `nominal`, `path` and `properties` fields plus `fields` or `variants`. Stored fields have `id`, `name`, `type`, `canonical`, `absent` and `properties`. Names are display information; exact identities determine dictionary and member lookup. Unknown metadata is retained, including in inline headers. Unknown kinds need an explicit registry entry; unsupported value-changing interpretations still need an admitted binding.

Dictionary entries and stored members use identity order while their complete metadata retains declaration order. Every nominal dependency must exist in this local dictionary before any rows are written. Missing descriptions, duplicate identities, wrong scalar ranges, wrong field types or mismatched canonical/absence flags return `CrumbError.InvalidInput`. Invalid shapes cannot be hidden by an empty record set or by an omitted absent field. Resource failures return `CrumbError.Limit`; no error returns a successful output prefix.

Presence bits remove only the declared outer absence. Nested `Some(None)` remains distinct from absence. Maps and sets use the primitive's stored-value comparison and reject duplicate original entries. Numeric conversion is exact: integer widths are range checked, decimal rescaling refuses rounding, and binary64 values must be exactly representable when stored as binary32. NaNs normalize and signed zero remains distinct.

`sealed=true` adds the count and ordered logical-value seal. It does not hash physical row bytes or authenticate a sender. This API builds a finite owned result under the shared request budget. It does not promise bounded-memory streaming, spooling, a column codec or a decoder.
