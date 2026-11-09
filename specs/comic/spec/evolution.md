# Shape compatibility and adaptation

Status: foundation contract. The corpus uses explicit written and requested descriptions, not a hand-written version number as evidence of compatibility.

## Admission order

1. Admit the codec/hash/binding profiles and the requested nominal identity.
2. Validate the written description: field identity width, strict ordering and uniqueness, known kinds and finite description references.
3. Match written and requested fields by identity. Decide structural compatibility once per description pair; do not hash names on every record.
4. Decode the selected values under shared limits. A present incompatible field is a failure naming the field, never absence.
5. Construct the requested final stored state and run its validation. No partial model escapes on failure.

## Compatible changes

A rename that retains the earliest identity and a declaration reorder preserve existing contents. Widening a signed integer to a wider signed type, an unsigned integer to a wider unsigned type, an unsigned integer to a signed type that represents its entire range, and binary32 to binary64 preserve values. Optional wrapping can preserve the same value. Compatibility is based on the declared written type, not a lucky small value in one row. The binding must state its integer widths.

An added field may supply outer absence for an optional or an empty collection only when the requested description admits that absent value. A nonempty default is not a migration. Dropping a field permits adaptation but leaves the original record unchanged. Nested records follow the same rules. An incompatible present field is not silently dropped merely because the requested field is optional.

A narrowed integer, integer-to-float, string-to-integer, optional-to-required or unrelated nominal record is refused. Unknown variants are refused unless a separately admitted binding contract explicitly represents open values. Field identity reuse for different meaning is a schema change, not a rename.

## Worked example

A stored customer has email and a legacy code. A later description calls email contact, retaining email's identity; adds an optional nickname; and removes legacy code. Reading produces contact with the original value and an absent nickname. The old crumb still holds the legacy code. Its object hash cannot be attached to the adapted customer as though the field were still present.

If a later description requires age with no absent policy, that same record fails at age. An empty or optional model still cannot read another model's record: nominal identity is checked before missing fields can be supplied.

## A forgotten rename

A rename without its earlier name looks like one field dropped and another added. The refusal must say so when the written shape points at it. If a required field is missing, and exactly one written field matches no requested field and has a compatible type, the refusal names that field and the fix:

~~~text
Line[0].price: required, and the written shape has no such field
  the written shape has unit_price: Decimal { scale = 2 }, which matches no requested field
  if price is its new name, declare price [renamed_from = ["unit_price"]]
~~~

The hint never changes the outcome. The reader still refuses, because matching by type alone could pair two unrelated fields.

## Migration is separate

An explicit application migration can transform incompatible data. Decoding does not invoke a migration selected by untrusted text, import code or overwrite the stored record. Store publication, snapshots, atomic replacement and retention are external contracts. Original-shape re-encoding and adapted-value re-encoding are different conformance checks.
