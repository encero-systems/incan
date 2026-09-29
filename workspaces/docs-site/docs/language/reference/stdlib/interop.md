# `std.interop`: checked C bindings

`std.interop` exports the `c` namespace and the `BindingDeclaration` class. The `c` namespace provides the `binding` declaration, the C types that a binding's declarations use, and the operations that pass values to and from a binding symbol.

## Import

```incan
from std.interop import c
```

The import makes `binding` a declaration keyword in the importing module. In a module without it, a `binding` declaration is refused (`INCAN-P0001`).

## Binding declarations

```text
binding Name:
    header = "header.h"
    link = c.system_library("name")
    <members>
```

A `binding` declares `Name` as a private declaration of its module. Its body holds one `header`, one `link`, and any number of members.

| Field | Form | Meaning |
| --- | --- | --- |
| `header` | A non-empty string literal. | The C header that declares the binding's functions, constants and types. |
| `link` | `c.system_library("name")` or `c.framework("name")`, with a non-empty string literal. | The native library the program links: a system library, or an Apple framework. |

| Member | Declares |
| --- | --- |
| [`symbol`](#symbols) | A C function. |
| [`resource`](#resources) | An opaque C type and the function that releases it. |
| [`enum`](#enums) | Named C integer constants. |
| [`struct`](#structs) | A C structure layout. |

- Refused (`INCAN-P0001`): a missing or non-literal `header`, a missing `link`, a repeated field, a field other than `header` and `link`, and header arguments on `binding`.
- Refused (`INCAN-T0001`): an empty `header`, a `link` in another form, two members of one kind with the same name, and a member with decorators or header arguments.

## C types

| Spelling | C type | Incan type at a call |
| --- | --- | --- |
| `c.i8`, `c.i16`, `c.i32`, `c.i64` | `int8_t`, `int16_t`, `int32_t`, `int64_t` | `i8`, `i16`, `i32`, `i64` |
| `c.u8`, `c.u16`, `c.u32`, `c.u64` | `uint8_t`, `uint16_t`, `uint32_t`, `uint64_t` | `u8`, `u16`, `u32`, `u64` |
| `c.i128`, `c.u128` | `__int128`, `unsigned __int128` | `i128`, `u128` |
| `c.f32`, `c.f64` | `float`, `double` | `f32`, `f64` |
| `c.Size` | `size_t` | `usize` |
| `c.c_char`, `c.c_int` | `char`, `int` | `int` |
| `None` | `void`, as a result type only | `None` |
| `c.ConstPtr[T]`, `c.MutPtr[T]` | `const T *`, `T *` | See [Text](#text) and [Spans](#spans). |
| `c.Owned[R]`, `c.Borrowed[R]`, `c.BorrowedMut[R]` | A pointer to `R`'s native type | See [Resources](#resources). |
| `Option[c.Owned[R]]` | A pointer to `R`'s native type that may be null | See [Resources](#resources). |
| `c.Out[T]`, `c.InOut[T]` | `T *`, as a parameter type only | See [Output positions](#output-positions). |
| The name of a `struct` member | The structure, by value | See [Structs](#structs). |

- `R` is a `resource` member of the same binding. `T` in `c.ConstPtr[T]` and `c.MutPtr[T]` is a C type from this table; `T` in `c.Out[T]` is a scalar or `c.Owned[R]`; `T` in `c.InOut[T]` is a scalar. The scalars are the `c.i*`, `c.u*`, `c.f*`, `c.Size`, `c.c_char` and `c.c_int` types.
- `c.i128` and `c.u128` require a verification target whose Clang accepts `__int128`.
- `Option[T]` admits only `c.Owned[R]`. A nullable pointer, `Option[c.ConstPtr[T]]` or `Option[c.MutPtr[T]]`, is refused (`INCAN-T0001`).
- A type outside this table in a signature or a struct field, and `c.Out` or `c.InOut` outside a parameter, is refused (`INCAN-T0001`). `None` as a parameter, field, pointer or slot type is refused at [verification](#verification) (`INCAN-T0001`).

## Symbols

```text
symbol name(parameter: Type, ...) -> Type:
    native = "c_function"
    bounds = { pointer_parameter: count_parameter, ... }
    outcome Enum.Variant:
        initializes = [parameter, ...]
        updates = [parameter, ...]
        invalidates = [parameter, ...]
```

- `native` is required: one non-empty string literal, the C function's name.
- `bounds` is optional and appears at most once. See [Spans](#spans).
- `outcome` declarations are optional. See [Output positions](#output-positions).
- Refused (`INCAN-T0001`): a missing return type; a parameter without a type or with a default value; a missing, empty or repeated `native`; a field other than `native` and `bounds`; and a body statement that is neither a field nor an `outcome`.

### Calls

`Binding.name(arguments)` calls a symbol. Arguments bind to parameters as in a function call, by position or by parameter name.

- A call is valid only inside an `unsafe:` block.
- An `int` argument is accepted for an integer parameter. A value outside the parameter's C type range panics at the call.
- A call is valid only when the symbol's signature uses these types. Parameters: scalars; `c.Owned[R]`, `c.Borrowed[R]` and `c.BorrowedMut[R]`; `c.ConstPtr[c.c_char]`; `c.ConstPtr` or `c.MutPtr` of `c.u8` or `c.f32`, named in `bounds`; `c.Out` of a scalar or `c.Owned[R]`; `c.InOut` of a scalar. Results: scalars, `None`, `c.Owned[R]`, `Option[c.Owned[R]]` and `c.ConstPtr[c.c_char]`. A symbol with another signature is declared and verified, and a call to it is refused.
- Refused (`INCAN-T0001`): a call outside `unsafe:`, a call to a symbol whose signature uses other types, a symbol the binding does not declare, and explicit type arguments.
- A `pub` function whose body contains a call receives a warning (`INCAN-T0001`).

## Resources

```text
resource Name:
    native = "c_type"
    release = symbol_name
```

- `native` is the C type's name, as an identifier or as `struct tag`. `release` names a symbol of the same binding whose only parameter is `c.Owned[Name]`.
- Refused (`INCAN-T0001`): a signature on the resource, a missing or repeated field, a field other than `native` and `release`, and a `release` symbol with another parameter list.

| Parameter or result | At a call |
| --- | --- |
| `c.Owned[R]` parameter | The argument's binding is consumed. A later use of it is refused (`INCAN-T0001`). |
| `c.Borrowed[R]` parameter | The resource is shared for the duration of the call. |
| `c.BorrowedMut[R]` parameter | The resource is exclusive for the duration of the call. An argument that is not a `mut` binding is refused (`INCAN-T0001`). |
| `c.Owned[R]` result | An owned resource. A null pointer panics. |
| `Option[c.Owned[R]]` result | `None` for a null pointer, otherwise `Some` of an owned resource. |

An owned resource that no `c.Owned[R]` parameter consumes is released through its `release` symbol once, when the value goes out of scope.

## Enums

```text
enum Name:
    Variant: c.i32 = NATIVE_CONSTANT
```

- Every variant has the same scalar carrier type and names a C constant, a C identifier that the header defines.
- `Binding.Name.Variant` is an `int` expression whose value is the constant's value on the verification target.
- Refused (`INCAN-T0001`): an enum without variants, variants with different carrier types, a carrier outside the scalars, a value that is not a name or a dotted name, and a signature on the enum.
- `Binding.Name.Other`, for a variant the enum does not declare, is refused (`INCAN-T0001`).

## Structs

```text
struct Name:
    native = "c_type"
    field: c.Type = field
```

- `native` is the C type's name, as an identifier or as `struct tag`.
- Each field line names a C field on both sides of `=` and gives its type.
- A struct is valid in a symbol signature; a call to that symbol is refused (see [Calls](#calls)).
- Refused (`INCAN-T0001`): a missing, empty or repeated `native`; a struct without fields; a field line in another form; and a field type outside the C types.

## Output positions

A `c.Out[T]` or `c.InOut[T]` parameter takes a slot created in the calling function.

| Form | Result |
| --- | --- |
| `c.out[T]()` | An uninitialized slot for a `c.Out[T]` parameter. |
| `c.inout(value)` | A slot holding `value`, for a `c.InOut[T]` parameter; `value` has `T`'s Incan type. |
| `slot.take()` | The slot's value, with `T`'s Incan type. |

- A slot is assigned directly to a local, and that local is passed to one output parameter of one call.
- `outcome Enum.Variant:` names a variant of an enum of the same binding whose carrier is the symbol's result type. `initializes` lists `c.Out` parameters; `updates` and `invalidates` list `c.InOut` parameters. Every `c.Out` parameter is in some outcome's `initializes`.
- `take()` reads a slot once. A `c.Out` slot is readable inside `if result == Binding.Enum.Variant:`, where `result` is the local holding the call's result and that outcome initializes the slot. A `c.InOut` slot is readable after the call when no outcome invalidates it; otherwise it is readable inside the `if` of an outcome that does not invalidate it.
- Refused (`INCAN-T0001`): an outcome without exactly one `Enum.Variant` value or with a signature; an outcome that names an unknown enum, variant or parameter; a variant whose carrier is not the result type; an outcome field that is not a list of distinct parameter names, or that lists a parameter of the other kind or one listed in another field of the outcome; a repeated outcome or outcome field; a field other than `initializes`, `updates` and `invalidates`; and a `c.Out` parameter that no outcome initializes.
- Refused (`INCAN-T0001`): `c.out` without exactly one type argument or with value arguments; `c.inout` without exactly one positional value or with type arguments; an output argument that is not a slot local; a slot of the other kind; a slot whose type is not the parameter's; a slot passed to an earlier call; one slot passed twice in a call; and `take()` with arguments, before the call, a second time, or where the slot is not readable.

```incan
from std.interop import c

binding Fixture:
    header = "fixture.h"
    link = c.system_library("fixture")

    resource Handle:
        native = "fixture_handle"
        release = close

    symbol close(handle: c.Owned[Handle]) -> None:
        native = "fixture_close"

    symbol touch(handle: c.BorrowedMut[Handle]) -> c.i32:
        native = "fixture_touch"

    enum Status:
        OK: c.i32 = FIXTURE_OK

    symbol open(output: c.Out[c.Owned[Handle]], attempts: c.InOut[c.i32]) -> c.i32:
        native = "fixture_open"

        outcome Status.OK:
            initializes = [output]
            updates = [attempts]

def open_handle() -> int:
    unsafe:
        output = c.out[c.Owned[Handle]]()
        initial: i32 = 0
        attempts = c.inout(initial)
        status = Fixture.open(output, attempts)
        if status == Fixture.Status.OK:
            mut handle = output.take()
            Fixture.touch(handle)
            Fixture.close(handle)
        return status
```

## Text

| Form | Meaning |
| --- | --- |
| `c.cstr(value)` | `value: str`. Returns `Result`: `Ok` with a NUL-terminated copy of `value`, or `Err(str)` when `value` contains a NUL character. |
| `text.as_const_ptr()` | Inside `unsafe:`, on an `Ok` value of `c.cstr`. The argument for a `c.ConstPtr[c.c_char]` parameter. |
| `view.copy_utf8(max_bytes=n)` | Inside `unsafe:`, on the result of a symbol whose result type is `c.ConstPtr[c.c_char]`; `n: int`. Returns `Result[str, str]`: `Ok` with the text before the first NUL in the first `n` bytes, or `Err` when the result is a null pointer, `n` is not positive, no NUL is in the first `n` bytes, or the text is not UTF-8. |

- The result of a `c.ConstPtr[c.c_char]` symbol has one operation, `copy_utf8`. Its type has no spelling, so a parameter or return annotation cannot name it.
- The result is the receiver of `copy_utf8`, or the value of an assignment to a local; that local is read only as the receiver of `copy_utf8`, outside any closure the assignment is not in.
- Refused (`INCAN-T0001`): `c.cstr` without exactly one positional `str` argument or with type arguments; `as_const_ptr()` outside `unsafe:` or with arguments; `copy_utf8` outside `unsafe:`, without exactly one argument named `max_bytes`, or with a bound that is not an `int`; and a result, or a local holding one, that is returned, stored in a value or collection, passed as an argument, assigned to another name, or read in a closure.

```incan
from std.interop import c

binding LibC:
    header = "string.h"
    link = c.system_library("c")

    symbol string_length(value: c.ConstPtr[c.c_char]) -> c.Size:
        native = "strlen"

def string_length(value: str) -> Result[usize, str]:
    text = c.cstr(value)?
    unsafe:
        return Ok(LibC.string_length(text.as_const_ptr()))
```

```incan
from std.interop import c

binding Env:
    header = "stdlib.h"
    link = c.system_library("c")

    symbol lookup(name: c.ConstPtr[c.c_char]) -> c.ConstPtr[c.c_char]:
        native = "getenv"

def home() -> Result[str, str]:
    name = c.cstr("HOME")?
    unsafe:
        view = Env.lookup(name.as_const_ptr())
        return view.copy_utf8(max_bytes=4096)                    # accepted

def leak() -> Result[None, str]:
    name = c.cstr("HOME")?
    unsafe:
        views = [Env.lookup(name.as_const_ptr())]                # refused: the result is stored (INCAN-T0001)
    return Ok(None)
```

## Spans

A span passes a buffer to a symbol as a pointer and a count.

| Constructor | Argument | Pointer | Count | Finish |
| --- | --- | --- | --- | --- |
| `c.bytes_span(value)` | `bytes` | `as_const_ptr()`, for `c.ConstPtr[c.u8]` | `byte_length() -> usize` | |
| `c.mutable_bytes_span(value)` | `bytes` | `as_mut_ptr()`, for `c.MutPtr[c.u8]` | `byte_capacity() -> usize` | `into_bytes(written) -> Result[bytes, str]` |
| `c.f32_span(values)` | `list[f32]` | `as_const_ptr()`, for `c.ConstPtr[c.f32]` | `element_count() -> usize` | |
| `c.mutable_f32_span(values)` | `list[f32]` | `as_mut_ptr()`, for `c.MutPtr[c.f32]` | `element_capacity() -> usize` | `into_f32s(written) -> Result[list[f32], str]` |

- A span is assigned directly to a local, and the local is used only as the receiver of its methods. Each method call is inside `unsafe:`. `as_mut_ptr()` requires a `mut` local.
- `bounds = { pointer: count }` pairs each `c.ConstPtr` or `c.MutPtr` parameter of `c.u8` or `c.f32` with a `c.Size` parameter. Every such pointer parameter is named in `bounds` exactly once.
- In a call, a paired pointer argument is the pointer method of a span local, and its count argument is the count method of the same local.
- `into_bytes(written)` and `into_f32s(written)` consume the span. They return `Ok` with the first `written` elements, or `Err` when `written` is negative or exceeds the capacity.
- Refused (`INCAN-T0001`): a `bounds` value that is not a non-empty dict of parameter names; a repeated `bounds`; a pointer parameter of `c.u8` or `c.f32` that `bounds` does not name exactly once; a `bounds` name that is not such a pointer parameter; a count parameter that is unknown or not `c.Size`; and `bounds` on a symbol without such a pointer parameter.
- Refused (`INCAN-T0001`): a constructor without exactly one positional argument of its type, or with type arguments; a span not assigned directly to a local; a span local used other than as a method receiver; a method call outside `unsafe:`; arguments to a pointer or count method; `as_mut_ptr()` on a local that is not `mut`; a finish method without one positional integer; a use of a span after its finish method; and a call whose paired pointer and count arguments are not the matching methods of one span local.

```incan
from std.interop import c

binding Buffers:
    header = "buffers.h"
    link = c.system_library("buffers")

    symbol copy(source: c.ConstPtr[c.u8], source_length: c.Size, destination: c.MutPtr[c.u8], destination_capacity: c.Size) -> c.Size:
        native = "buffers_copy"
        bounds = { source: source_length, destination: destination_capacity }

def copy_bounded(data: bytes) -> Result[bytes, str]:
    source = c.bytes_span(data)
    mut destination = c.mutable_bytes_span(b"\0\0\0\0")
    unsafe:
        written = Buffers.copy(source.as_const_ptr(), source.byte_length(), destination.as_mut_ptr(), destination.byte_capacity())
        return destination.into_bytes(written)
```

## Verification

Before a module with a binding is checked, built or run, each binding is verified with Clang against one target:

- each symbol's C function type, from its parameter and result types;
- each enum constant's C type, against the enum's carrier;
- each struct's size and alignment, and the offset of each listed field.

| Target | Selected by | Clang target |
| --- | --- | --- |
| Linux x86-64 host | Default on that host | `x86_64-unknown-linux-gnu` |
| macOS arm64 host | Default on that host | `arm64-apple-macos11` |
| `x86_64-unknown-linux-gnu` | `incan check --interop-target x86_64-unknown-linux-gnu` | `x86_64-unknown-linux-gnu` |
| `aarch64-apple-darwin` | `incan check --interop-target aarch64-apple-darwin` | `arm64-apple-macos11` |
| `aarch64-linux-android` with Android platform facts | `incan check --interop-target aarch64-linux-android` | `aarch64-linux-android<api-level>` |
| `aarch64-apple-ios` with iOS platform facts | `incan check --interop-target aarch64-apple-ios` | `arm64-apple-ios<deployment-target>` |
| `aarch64-apple-ios-sim` with iOS platform facts | `incan check --interop-target aarch64-apple-ios-sim` | `arm64-apple-ios<deployment-target>-simulator` |

- `--interop-target` selects a target declared under [`[interop.c]`](../../../tooling/reference/project_configuration.md#interopc) in `loaf.toml`; the target's `definitions` are passed to Clang. A triple the package does not declare fails the command.
- `INCAN_C_ABI_CLANG`, when set, is the Clang executable. Otherwise the executable is Xcode's Clang, with the macOS, iPhoneOS or iPhoneSimulator SDK as sysroot, on macOS, and `clang` on Linux. An Android target requires `INCAN_C_ABI_CLANG`.
- A `native` name, enum constant or struct field name is a C identifier; a struct or resource `native` may also be `struct tag`.
- Refused (`INCAN-T0001`): a binding that fails verification; a name that is not a C identifier; a binding that Clang cannot verify; and, without `--interop-target`, a binding on a host other than Linux x86-64 and macOS arm64.

## See also

- [Write your first checked C binding](../../tutorials/checked_c_binding.md)
- [Work with checked C bindings](../../how-to/checked_c_bindings.md)
- [How checked C interop is structured](../../explanation/checked_c_interop.md)
- [Inspect checked C bindings](../../../tooling/how-to/inspect_checked_c_bindings.md)
- [Binding inspection JSON schema](../../../tooling/reference/binding_inspection_schema.md)
- [Oven interop deployment plans](../../../tooling/reference/interop_deployment_plans.md)
