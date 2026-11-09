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
