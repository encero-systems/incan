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
