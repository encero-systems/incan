# Collection protocols (Reference)

This page documents stdlib traits that model Python-like collection behavior.

Dunder hooks are the implementation methods. Explicit trait adoption names the capability when a bound, diagnostic, or reference page needs stable vocabulary.

## Contains (membership)

- **Syntax**: `item in collection` / `item not in collection`
- **Hook**: `__contains__(self, item: T) -> bool`
- **Trait**: `Contains[T]`

## Len (length)

- **Syntax**: `len(x)`
- **Hook**: `__len__(self) -> int`
- **Trait**: `Len`

## Iterable / Iterator (iteration)

- **Syntax**: `for x in y:`
- **Hooks**:
    - `__iter__(self) -> Iterator[T]`
    - `__next__(self) -> Option[T]`
- **Traits**:
    - `Iterable[T]`
    - `Iterator[T]`

`Generator[T]` implements this iteration surface. Generator functions and generator expressions can be used directly in `for` loops or passed to APIs that accept `Iterable[T]` / `Iterator[T]`.

## Bool (truthiness)

- **Syntax**: `if x:` / `while x:`
- **Hook**: `__bool__(self) -> bool`
- **Trait**: `Bool`

`Bool` is available for types whose domain has a clear truth value. It should not replace explicit checks for optionality, errors, emptiness, or named state. Prefer patterns such as `value is Some(x)`, `result is Ok(x)`, `len(items) > 0`, `name != ""`, or `connection.is_open` when those are what the code actually means.
