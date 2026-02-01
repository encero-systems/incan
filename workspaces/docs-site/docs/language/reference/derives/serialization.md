# Derives: Serialization (Reference)

This page documents `Serialize` and `Deserialize` for JSON.

See also:

- [Derives & traits](../derives_and_traits.md)
- [Error handling](../../explanation/error_handling.md)

---

## Serialize

- **Derive**: `@derive(Serialize)`
- **API**: `json_stringify(value) -> str`

```incan
@derive(Serialize)
model User:
    name: str
    age: int

def main() -> None:
    u = User(name="Alice", age=30)
    println(json_stringify(u))
```

---

## Deserialize

- **Derive**: `@derive(Deserialize)`
- **API**: `T.from_json(input: str) -> Result[T, str]`

```incan
@derive(Deserialize)
model User:
    name: str
    age: int

def main() -> None:
    result: Result[User, str] = User.from_json("{\"name\":\"Alice\",\"age\":30}")
```

---

## Schema-safe field names (models only)

If your JSON schema uses keys that are not valid Incan identifiers (or are keywords like `type`), represent them using a
`model` field alias and choose a schema-safe canonical field name (e.g. `type_`).

```incan
@derive(Serialize, Deserialize)
model Account:
    type_ as "type": str
```

When `Serialize`/`Deserialize` is derived, the alias is used as the JSON key (`"type"`). The canonical identifier
(`type_`) remains the stable field name in code. See [Models: Using aliases in code](../../explanation/models_and_classes/models.md#using-aliases-in-code).

`class` does not support field metadata/aliases, so class JSON keys always match the canonical field names.

## Type mappings (Incan → JSON)

| Incan             | JSON             |
| ----------------- | ---------------- |
| `str`             | string           |
| `int`             | number           |
| `float`           | number           |
| `bool`            | boolean          |
| `List[T]`         | array            |
| `Dict[str, T]`    | object           |
| `Option[T]`       | value or `null`  |
| `model` / `class` | object           |
