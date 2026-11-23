# Typed values and shapes

Status: experimental foundation. The terms below are independent of any source language or native ABI.

## Value domain

A value has an admitted type. The domain includes booleans, exact integers, binary floating-point values, strings, bytes, secret strings and secret bytes, decimal values with declared constraints, ordered sequences, fixed positional tuples, maps, sets, optional layers, discriminated unions, nominal records, nominal enum variants, constrained newtypes and typed references. An enum payload can contain named or positional fields. A reader does not infer a missing type from a string or a field name.

Strings preserve their Unicode scalar sequence. Bytes are not strings. Integers are mathematical values constrained by a declared storage type; no host JSON-number conversion is permitted. A sequence retains order and repeated elements. A map refuses duplicate keys under the selected stored comparison profile; it must not resolve duplicates by taking the last value. An implementation may stream values: this domain does not require allocating a universal value tree.

An optional distinguishes absence from a present inner optional that is absent. The test notation writes these as None, Some(None) and Some(Some(value)); these are logical constructors, not executable calls. A union records which member holds the value. A reference records a typed identity; reading it never traverses its target.

## Foundation type vocabulary

The specification and corpus share core type names: `boolean`; signed `i8`, `i16`, `i32`, `i64`, `i128`; unsigned `u8`, `u16`, `u32`, `u64`, `u128`; IEEE `fp32`, `fp64`; `string`; `binary`; `secret_string`; and `secret_bytes`. Decimal types carry precision and scale. Collections, optional layers, unions and described types follow [COMIC text](text.md). An implementation must not substitute its host-sized integer for a declared width.

The core kinds, `Record` and `Enum`, use the type spellings listed in the text specification. A language profile maps its own type vocabulary into this domain. Test notation's tagged kinds such as `int` and `float` classify values; their declared widths remain explicit. They are not additional type spellings.

## Written shape

A record description provides its nominal identity and its fields. Each stored field has a six-byte identity, a declared type, a canonical participation flag and an explicit absent-value policy. Display names and declaration positions are available for diagnostics but do not select stored slots. Written field identities occur once, in strictly increasing unsigned byte order. Equal identities are an error, including when different names collide. Variants have identities within their owning enum and payload descriptions.

Recursive descriptions use explicit description references rather than infinitely expanded trees. A finite recursive description is allowed. A cycle in encoded value containment is not. Following an inert typed reference is a separate application operation, so two records referencing each other do not require recursive decoding.

Descriptions cannot grant execution authority. A reader needs an admitted description and an implementation for its requested type. It refuses an unknown profile or unsupported type. It does not download a package because a document names one.

## Reconstruction boundary

A language binding may use model-owned traversal and reconstruction to access private stored fields. That does not expose private assignment to ordinary application code. All stored fields participate; a virtual construction input is not a stored field. Reconstruct final stored state and validate that state without replaying construction-only transformations.

For example, a constructor may accept hours and store seconds. The shape has seconds. Restoring seconds=7200 must not run the hours-to-seconds transformation again. A reader cannot invent an absent required value to make construction succeed.

A value of a secret type, `secret_string` or `secret_bytes`, is never written. COMIC text and Crumb bytes both carry a withheld marker in its place. Reading yields a withheld value, which the language binding fills from its own source before the record is complete, or refuses. A withheld value left unfilled is an error, never read as absence. Keeping secrets out of other types is the binding's job; the format cannot tell that a `string` holds a password. Process resources, such as handles and pointers, have no portable encoding. A diagnostic rendering may redact other data but is not a round-trip document. Application-specific validation and capability checks remain distinct from structural decoding.

## Record boundaries

A RecordSet is a finite collection of selected records. A RecordStream may yield more records later. End of the selected encoded portion, end of one record and end of the stream are separate observations. A waiting stream is not complete. Neither abstraction requires all records or all decoded model instances to be in memory at once.

Each decode applies one caller-visible byte, depth and allocation budget through nested reads. Check lengths and counts against available input and budget before allocation. Retain backing storage while borrowed values use it. Refuse an exceeded budget without returning a partial model. Unconsumed bytes inside a selected record are an error; the next framed record is not trailing garbage.
