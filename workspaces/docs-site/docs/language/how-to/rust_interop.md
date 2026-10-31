# Call Rust APIs from Incan

Use `rust::` imports to call Rust crates through checked native signatures. Oven selects compiled dependencies, the checker resolves Rust items, and the pinned compiler compiles native calls. For import syntax and refusals, see [Imports and modules](../reference/imports_and_modules.md#rust-crate-imports).

## Start with Rust's standard library

Put this in `src/main.incn` of a project with a declared `main` script:

```incan
from rust::std::time import Duration


def main() -> None:
    seconds: f32 = 1.5
    duration = Duration.from_secs_f64(seconds)
    println(duration.as_secs_f64())
```

Check and run it with the matching installed toolchain:

```bash
incan check src/main.incn
incan run src/main.incn
```

The program prints `1.5`. `rust::std` needs no project dependency declaration. `std.*` names Incan's standard library; for example, `std.fs.Path` provides file operations without exposing Rust's path types. `rust::core` and `rust::alloc` imports are refused; use `rust::std`.

## Declare an external crate

In a project, declare the crate in `loaf.toml` before importing it. A registry Loaf can be declared under `[dependencies]`:

```toml
[project]
name = "json_app"
version = "0.1.0"

[project.scripts]
main = "src/main.incn"

[dependencies]
serde_json = { loaf = "crates-io/serde_json", version = "1" }
```

Alternatively, use `[rust-dependencies]` for a Rust crate requirement, including a package alias:

```toml
[rust-dependencies]
json = { package = "serde_json", version = "1" }
```

The second form is imported as `rust::json`. A sibling path Loaf linked as a Rust facet is also available through its dependency alias. See [Managing dependencies](../../tooling/how-to/dependencies.md) for project declarations.

The selected version and features must match an exact compiled registry unit in the installed toolchain. Declaring a crate does not download or build a missing registry unit. Use a toolchain that contains the required closure; changing only the requested version cannot supply a missing artifact.

For the first manifest above, parse JSON with a checked return type and propagate its error:

```incan
from rust::serde_json import from_str, Error as JsonError


def parse_number(text: str) -> Result[int, JsonError]:
    """Parse a JSON integer using the function's result annotation."""
    return from_str(text)


def main() -> Result[None, JsonError]:
    value = parse_number("7")?
    println(value)
    return Ok(None)
```

## Let a typed use fix a native result

An unannotated local can leave a native call's type parameters open until its first use that needs them. Use a typed return, parameter, or annotated assignment to fix a method's result, or insert a value to fix an empty collection's element types:

```incan
from rust::std::collections import HashMap
from rust::std::iter import Iterator as NativeIterator
from rust::std::vec import Vec


def total_of(values: Vec[int]) -> int:
    total = values.into_iter().sum()
    return total


def main() -> None:
    mut table = HashMap.new()
    assert len(table) == 0
    assert table.is_empty()
    table.insert("one", 1)
    println(len(table))
```

The length and emptiness reads need none of the open element types, so the insert can still fix them. A later conflicting insert is refused. If no use fixes the arguments, annotate the binding or supply the call's type arguments explicitly.

## Select standalone versions and features

A source file outside a project can state its Rust dependency requirement inline:

```incan
import rust::serde_json @ "1"
import rust::serde @ "1" with ["derive"]
```

Version strings use Cargo SemVer requirements, such as `"1"`, `"~1.2"`, `">=1, <2"` and `"=1.2.3"`. Repeated explicit requirements for a crate must match; requested features are unioned.

Without an inline version, Oven looks for that crate's declared dependency in the available standard packages' `loaf.toml` files. It takes the version and features from those declarations. There is no separate table of tested default versions. If no declaration supplies a default, provide an explicit requirement. The requested registry unit must still be installed.

For a project, keep dependencies in its manifest and use bare imports. Inline annotations on a manifest-configured crate are refused.

## Spell Rust names and generic calls

Use `::` between Rust path segments. Keyword-named Rust items can be imported with an ordinary local alias:

```incan
from rust::my_crate::proto import type as proto_type
```

For a Rust field named `r#type`, write `value.type` or `TypeName(type=value)` in Incan. A field actually named `type_` stays `value.type_`.

Place explicit type arguments after the associated function or method name. A constructor whose generic parameters belong only to its owner uses those brackets to specialize the owner. For example, for a crate exposing `Fft<T>::new`:

```incan
result: Result[Fft[f32], ResamplerConstructionError] = Fft.new[f32](
    48_000, 24_000, 480, 1, FixedSync.Input,
)
```

Do not write `Fft[f32].new(...)`. An expected result or parameter type can supply the receiver specialization without explicit brackets. When specifying method type arguments, supply the complete inspected type-parameter list; `_` leaves a slot inferred:

```incan
stream = device.build_output_stream[f32, _, _](config, on_data, on_error, None)
```

These examples assume the named crate dependencies, types and surrounding values are declared. Const-generic arguments cannot be supplied as Incan type arguments.

## Call methods through checked traits

Import the trait that supplies an extension method. The checker resolves the method against its Rust signature and bounds; a metadata-free import does not grant method-call authority.

```incan
from rust::std::borrow import Borrow
from rust::std::path import PathBuf


pub def borrowed(value: &PathBuf) -> &PathBuf:
    return value.borrow()
```

To disambiguate two imported traits with the same method name, call `Trait.method(receiver, arguments...)`. The named trait must declare the method, and its checked signature must be available. This is a native trait call; it does not depend on retaining a generated Rust `use` statement.

## Match a borrowed callback signature

When a Rust API requires a callback with a borrowed slice parameter, give a named Incan callback the matching borrowed list annotation:

```incan
def write_silence(data: &mut list[f32], info: &OutputCallbackInfo) -> None:
    pass
```

Here `OutputCallbackInfo` is imported from the callback's crate. The call's checked Rust signature supplies the slice ABI. A closure expression can receive the same contextual parameter types. This applies at that callback boundary; it does not change the representation of ordinary Incan lists.

A numeric or Boolean result from a native callback must be owned. A callback that returns its borrowed numeric or Boolean parameter has no retained output-lifetime contract and is refused while checking.

## Name a Rust-backed type

Use `rusttype` for an Incan-facing name backed by a Rust type:

```incan
from rust::std::string import String as RustString

type Name = rusttype RustString
```

A body may define methods and explicit conversion adapters. Adopt traits with `with TraitName`; when method names overlap, qualify the implementing declaration with `for TraitName` before its return arrow. A body-less adoption requires metadata proving that the backing type already satisfies the Rust trait. Rust coherence rules also apply to native implementations.

Inside a `rusttype` body, an `interop:` block can declare adapters:

```incan
interop:
    from str try Name.parse
    into str via Name.as_str
```

This fragment assumes `Name.parse` returns `Result[Name, E]` or `Option[Name]` and `Name.as_str` returns `str`. Use `from S via adapter` for an infallible inbound conversion. See [Rust types for Python developers](rust_types_for_python_devs.md) for the authoring model.

## Publish Rust-backed library APIs

A facade library can re-export imported Rust items:

```incan
pub from rust::serde_json import Value
```

Build the producer with `incan build src/lib.incn`. The producer publishes its checked API and Rust ABI in its library sidecars. A consumer declaring that path dependency imports `Value` through `pub::PACKAGE`. Its selected producer artifact supplies the Rust-facing contract; the consumer does not select an unrelated signature for the same path.

Public metadata and editor previews are specified in [Checked API metadata](../../tooling/reference/checked_api_metadata.md).

## Choose conversions explicitly

Exact numeric types and lossless widening are accepted at Rust call boundaries. The opening example passes `f32` to `Duration.from_secs_f64`; narrowing to `f32`, signed-to-unsigned conversions and other lossy changes need an explicit source conversion. Handle `try_resize()` with `Option` before making the call. See [Numeric semantics](../reference/numeric_semantics.md).

Incan `str` values can satisfy Rust `String` or shared `&str` parameters, and `bytes` can satisfy `Vec<u8>` or shared byte slices. Write `&T` or `&mut T` where an API explicitly borrows a value; Rust types with lifetime parameters can require a lifetime contract and may be refused.

For imported Rust free functions with checked parameter names, keyword arguments bind by those names. A name missing from the signature is refused rather than guessed.

## Rust lint annotations {#targeted-generated-rust-lint-suppression}

Oven's native build does not run Clippy or generate Rust source attributes. The checker validates `@rust.allow(...)` arguments, but that annotation does not admit an unsupported signature or satisfy a trait bound. Resolve a source error at its reported span rather than adding a lint allowance to the declaration.

## Diagnose a refused call

Run `incan check src/main.incn --format json` to identify the source span and stable diagnostic code. Check the dependency alias, installed artifact, receiver type, generic arity and trait bound before changing the call. A successful check establishes the source contract; `incan run` also proves that its native call compiles and executes.

For Rust capability bounds, see the [language reference](../reference/language.md#standard-library-namespaces). For checked C symbols and unsafe call blocks, see [Checked C bindings](checked_c_bindings.md).
