# Write your first checked C binding

This tutorial calls C's `abs` function through a small Incan façade. It shows the complete current path: describe an exact scalar C signature, let the compiler verify it against a header, and keep the raw call inside a narrow `unsafe:` block. The same binding surface also supports declared opaque resources and compiler-managed output positions; a later section shows how those contracts work.

The checked C surface is experimental and intentionally small. Use this tutorial on a Linux x86-64 or Apple arm64 host with a Clang-compatible C toolchain and the standard C headers installed. It does not yet provision cross-target toolchains or native artifacts for you.

## Declare the raw contract

Create `src/main.incn`:

```incan title="src/main.incn"
from std.interop import c

binding LibC:
    header = "stdlib.h"
    link = c.system_library("c")

    symbol absolute(value: c.i32) -> c.i32:
        native = "abs"
```

`from std.interop import c` activates `binding` in this module. The `binding` form is vocabulary for an ordinary, unexported declaration class; it is not a new global class syntax. `header` names the header Clang must check, while `link` records the logical system library used by generated Rust. The `symbol` member has no implementation body: it only maps an Incan name and exact C types to one native symbol spelling.

## Add an ordinary Incan façade

The binding is not the application API. Add a function below it:

```incan title="src/main.incn"
def absolute(value: int) -> int:
    unsafe:
        return LibC.absolute(value)

def main() -> None:
    assert absolute(-7) == 7
```

`unsafe:` is where the source acknowledges the raw foreign call. The compiler range-checks the conversion between Incan `int` and `c.i32`; it does not silently narrow a value. The surrounding `absolute` function is ordinary Incan and is the right place for validation, domain errors, and a public API.

Run it:

```console
incan run src/main.incn
```

Before Rust is generated, Incan asks Clang to check the declared C signature against `stdlib.h` for the host target. If the header says that `abs` has a different type, the compiler reports the binding error before it can become a link-time or runtime surprise.

## Expose a C constant as an Incan value

Add an enum declaration to the binding:

```incan title="src/main.incn"
binding LibC:
    header = "stdlib.h"
    link = c.system_library("c")

    symbol absolute(value: c.i32) -> c.i32:
        native = "abs"

    enum ExitStatus:
        Success: c.i32 = EXIT_SUCCESS
```

The right-hand side is the native C macro or identifier, not an Incan expression to evaluate. Incan asks Clang to fold it for the selected target and makes the checked value available as an ordinary Incan integer:

```incan title="src/main.incn"
def main() -> None:
    assert LibC.ExitStatus.Success == 0
    assert absolute(-7) == 7
```

## Model an owned handle and an output position

Many native APIs create a handle through an output pointer and require one matching release function. Keep both facts in the binding instead of modelling the pointer as an `int` or exposing it from a public façade:

```incan
binding Fixture:
    header = "fixture.h"
    link = c.system_library("fixture")

    resource Handle:
        native = "fixture_handle"
        release = close

    symbol close(handle: c.Owned[Handle]) -> None:
        native = "fixture_close"

    enum Status:
        OK: c.i32 = FIXTURE_OK

    symbol open(output: c.Out[c.Owned[Handle]]) -> c.i32:
        native = "fixture_open"

        outcome Status.OK:
            initializes = [output]
```

`c.Owned[Handle]` is a non-copyable private bridge value. Passing it to `close` transfers the resource to the declared release operation. If ordinary control flow leaves a still-owned handle unconsumed, the generated release guard performs the same release once. `c.Out[...]` is compiler-managed storage, not a general-purpose container: call it with `c.out[...]()`, and call `take()` only on the outcome path that declares the slot initialized.

```incan
def open_handle() -> int:
    unsafe:
        output = c.out[c.Owned[Handle]]()
        status = Fixture.open(output)
        if status == Fixture.Status.OK:
            handle = output.take()
            Fixture.close(handle)
        return status
```

Use `c.InOut[c.i32]` for a scalar pointer whose initial value is supplied by the caller and may be updated by the native call. Create it with `c.inout(value)` and consume its post-call value with `take()`. A binding outcome can state that an `Out` slot is initialized and that an `InOut` slot is updated; the compiler will reject an unguarded `Out.take()`.

## What this slice supports

You can use checked bindings today for scalar free functions, native integer constants, plain-structure layout declarations, opaque owned resources, inferred call-scoped resource borrows, scalar or owned-resource `Out`/`InOut` positions, and NUL-terminated text input through `c.cstr(value)?` to an exact `c.ConstPtr[c.c_char]` parameter. General pointer and by-value plain-structure calls remain unavailable. Returned C strings, spans, caller-owned buffers, and scoped foreign views also remain separate work, as do callbacks, variadics, shims, bundled artifacts, and mobile packaging.

Next, use the [checked C binding how-to](../how-to/checked_c_bindings.md) when you need to model a real header, and read [how checked C interop is structured](../explanation/checked_c_interop.md) before choosing C over Rust interop. The [`std.interop` reference](../reference/stdlib/interop.md) lists every accepted declaration form and boundary.
