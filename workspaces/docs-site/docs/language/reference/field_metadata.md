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

An explicit `canonical=true` is retained. A one-item `renamed_from=["old_name"]` list remains a list when formatted.

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
