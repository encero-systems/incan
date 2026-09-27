# Derives: Copying and Default (reference)

This page specifies `Clone`, `Copy` and `Default`, and field defaults at construction. The derive catalog and the automatic derives are in [Derives and traits](../derives_and_traits.md).

## Clone

- **Provides**: `.clone()`, which returns a deep copy, and `T with Clone` bounds.
- **Provided by**: every `model`, `class` and `enum`; a `newtype` as listed under [Automatic derives](../derives_and_traits.md#automatic-derives); `@derive(Clone)`.
- **Behavior**: every field is cloned.
- **Dunder**: none.
- **Requires**: every field type implements `Clone` (`INCAN-T0113` for a field of a `model`, `class` or `enum`).

## Copy

- **Provides**: an assignment or an argument copies the value, and the original stays usable.
- **Provided by**: `@derive(Copy)`; a `newtype` whose underlying type is `Copy`.
- **Behavior**: every field is copied.
- **Dunder**: none.
- **Requires**: every field type is `Copy`: `int`, `float`, `bool`, an exact-width numeric type, or a type that derives `Copy`.

## Default

- **Provides**: `Type.default()`, and `T with Default` bounds, under which `T.default()` constructs a value.
- **Provided by**: `@derive(Default)`.
- **Behavior**: each field takes its declared default, or its type's default when it declares none.
- **Dunder**: none.
- **Requires**: each field that declares no default has a type that implements `Default`.

| Type | Default |
| --- | --- |
| `int` and the exact-width integers | `0` |
| `float`, `f32`, `f64` | `0.0` |
| `bool` | `false` |
| `str` | `""` |
| `list[T]` | `[]` |
| `dict[K, V]` | `{}` |
| `set[T]` | an empty set |
| `Option[T]` | `None` |
| A type that derives `Default` | its `default()` |

```incan
@derive(Default)
model Settings:
    theme: str = "dark"
    font_size: int = 14
    retries: int

def make[T with Default]() -> T:
    return T.default()

def main() -> None:
    a = Settings.default()      # accepted: theme is "dark", font_size 14, retries 0
    b: Settings = make()        # accepted
```

## Field defaults

- A field declares a default with `name: Type = value`.
- A construction may omit a field that declares a default; the field takes that default.
- A construction that omits a field without a default is refused (`INCAN-T0001`).

```incan
model Settings:
    theme: str = "dark"
    font_size: int

def main() -> None:
    a = Settings(font_size=14)  # accepted: theme is "dark"
    b = Settings()              # refused: font_size declares no default
```
