# Glossary

This page defines the terms the docs use.

## Type

A type describes what a value is (for example: `int`, `str`, `bool`) and what operations are valid on it.

See also: [Language reference (generated)](language.md).

## Function

A function is a named block of code you can call. In Incan:

```incan
def add(a: int, b: int) -> int:
    return a + b
```

## Module

A module is one `.incn` or `.incan` source file and the declarations it contains (functions, models, constants, and so on).

## Import

An import binds a declaration or module of another module in the current module.

See:

- Explanation: [Imports and modules](../explanation/imports_and_modules.md)
- How-to: [Imports and modules (how-to)](../how-to/imports_and_modules.md)
- Reference: [Imports and modules (reference)](imports_and_modules.md)

## Keyword

A keyword is a reserved word with special meaning in the language syntax (for example `def`, `return`, `class`). The keywords are listed in the [Language reference (generated)](language.md).

## Identifier

An identifier is a name that is not a keyword. Any identifier can name a model or class field, a method, a function, a parameter or keyword argument, an enum variant, a type parameter or a module.

## Soft keyword

A soft keyword is a keyword only after a particular import; before it, the word is an identifier.

`async` and `await` are soft keywords: they are keywords after an import whose path begins with `std.async`.

See: [Imports and modules (reference)](imports_and_modules.md#soft-keywords).

## Result

`Result[T, E]` represents either success (`Ok(T)`) or failure (`Err(E)`).

See: [Error Handling](../explanation/error_handling.md).

## Option

`Option[T]` represents either “some value” (`Some(T)`) or “no value” (`None`).

See: [Error Handling](../explanation/error_handling.md).

## Async

Async code lets a program do other work while waiting on I/O (network, disk, timers).

`async def` and `await` require an import whose path begins with `std.async` (see [Soft keyword](#soft-keyword)).

See:

- [Async Programming](../how-to/async_programming.md)
- [Imports and modules (reference)](imports_and_modules.md#soft-keywords)

## rustup

`rustup` is the Rust toolchain installer and version manager. It installs `rustc` and `cargo`.

## cargo

`cargo` is Rust’s build tool and package manager.

## PATH

`PATH` is an environment variable that controls which directories your shell searches for executable commands (like `incan`).

## make

`make` runs the targets of a `Makefile`.

## crate

A crate is a Rust compilation unit: a library or a binary. In an Incan import path, `crate` names the project's source root (see [Module paths](imports_and_modules.md#module-paths)).
