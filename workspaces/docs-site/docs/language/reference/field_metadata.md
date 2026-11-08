# Field metadata (reference)

Field metadata appears after a model field name and before its type:

```text
FIELD [KEY=VALUE, ...] [as "WIRE_NAME"]: TYPE [= DEFAULT]
```

The `as "WIRE_NAME"` form is equivalent to `alias="WIRE_NAME"`. It can follow bracket metadata when the bracket does not also contain `alias`.

## Keys

| Key | Value | Default | Effect |
| --- | --- | --- | --- |
| `alias` | String literal | None | Sets the wire name and an alternate constructor, access and pattern key when the string is code-spellable. |
| `description` | String literal | None | Records field documentation. |
| `renamed_from` | String literal or list of string literals | Empty | Records earlier declared field names in chronological order, earliest first. |
| `canonical` | Boolean literal | `true` | Records whether the field participates in the canonical stored hash. |
| `made_private` | `true`, a message string, or a lifecycle block | None | Retains migration guidance after a field moves from public to private visibility. |

An explicit `canonical=true` is retained. A one-item `renamed_from=["old_name"]` list remains a list when formatted.

## Made-private lifecycle descriptor

`made_private` is valid only on a private field. `true` records the migration without additional prose, and a string is the migration message. The block form accepts these optional keys:

| Key | Value | Effect |
| --- | --- | --- |
| `since` | String literal | Version in which the declaration became private. |
| `retire_in` | String literal | Version in which the retained migration fact may be retired. |
| `message` | String literal | Guidance shown by the ordinary private-member refusal. |
| `by` | String literal | Date by which the migration should be complete. |
| `issue` | String literal | Tracking issue identifier. |
| `replacement` | Checked declaration reference | Replacement shown by the ordinary private-member refusal. |

```incan
model LegacyGraph:
    nodes [made_private { since="0.6", message="Use read_view", replacement=LegacyGraph.read_view }]: list[int]
```

Methods and nominal types use `@made_private(...)` with the same named lifecycle arguments. A `made_private` declaration is refused while it remains `pub`. It cannot also be `deprecated`: `deprecated` describes a callable declaration that still works, while `made_private` explains an access refusal. Compiled library metadata retains field, method and type migration facts for consumers.

## Rename rules

- Every earlier name is a non-empty Incan identifier.
- An earlier name cannot still be a field name on the same model. Aliases and method names do not count as fields for this rule.
- One earlier name can appear only once in a model's rename history, including within one ordered list.
- A rename changes neither the declared field name nor its wire name. Constructors, member access, patterns and decoders continue to use the declared name or `alias`/`as`; they do not accept an earlier name.
- The first earlier name determines the compiler-derived stored field identity. Without rename history, the declared name determines it.

```incan
model Customer:
    email_address [renamed_from="email"] as "email": str
    display_name [renamed_from=["name", "full_name"], canonical=false]: str
```

## Reflection

`__fields__()` keeps its existing field order and visibility rules. Each returned `FieldInfo` exposes these rename and storage facts:

| Field | Type | Value |
| --- | --- | --- |
| `identity` | `FrozenBytes` | The first six bytes of SHA-256 over the UTF-8 bytes of the earliest name, or the declared name when no earlier name exists. |
| `canonical` | `bool` | The authored `canonical` value, or `true` when the key is absent. |
| `renamed_from` | `FrozenList[FrozenStr]` | Every earlier declared name in authored order, earliest first; empty when no history is declared. |

`FieldInfo.name` remains the current declared field name. `alias` and `wire_name` remain controlled only by `alias` or `as`.

## Refusals

| Condition | Code |
| --- | --- |
| Unknown or duplicate metadata key | `INCAN-P0001` |
| Value does not have the key's required safe value kind | `INCAN-P0001` |
| Earlier name is empty or is not an identifier | `INCAN-T0001` |
| Earlier name is still a field on the same model | `INCAN-T0130` |
| Earlier name is claimed more than once | `INCAN-T0131` |
| `renamed_from` or `canonical` is used on a class field | `INCAN-T0001` |
| Construction, access, a pattern or decoding uses an earlier name | `INCAN-T0001` |
| `made_private` is attached to a public field, method or type | `INCAN-T0132` |
| A compiled consumer imports a type recorded as made private | `INCAN-T0133` |
