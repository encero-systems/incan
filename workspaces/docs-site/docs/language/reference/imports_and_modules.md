# Imports and modules (reference)

This page specifies import forms, module paths and files, what an import binds, the reserved name prefix, exports and re-exports, package namespaces, the `std` root, soft keywords, Rust crate imports, `import python`, and `import this`. Refusals of an import are reported with `INCAN-I0001`, syntax errors with `INCAN-P0001`, and refusals made when a project builds or locks with `INCAN-C0001`, unless a rule below names another code.

## Import forms

| Form | Binds |
| --- | --- |
| `from M import a, b as c` | `a` and `c` |
| `from M import (a, b as c,)` | The same; the list may span lines and end with a comma |
| `import M`, `import M as x` | The module `M`, as the last segment of its path, or as `x` |
| `import M::item`, `import M.item`, `import M::item as x` | The declaration `item` of `M` when `M::item` is not a module; otherwise the module `M::item` |

- `.` and `::` are interchangeable between path segments. After `pub`, the separator is `::`; `pub.` is refused (`INCAN-P0001`). After `rust`, `.` is accepted with a warning.
- `import a.b.c` binds `c`.
- Refused (`INCAN-P0001`): a wildcard, `from M import *`, and an empty list, `from M import ()`.

```incan
from models import User, Product as Item      # accepted
from utils import (
    format_currency as fmt,
    validate_email,
)                                             # accepted
import db.models                              # accepted
import db::models::User                       # accepted
from models import *                          # refused: wildcard (INCAN-P0001)
```

## Module paths

| Path | Resolves from |
| --- | --- |
| `models`, `db.models`, `db::models` | The importing file's directory, then the source root |
| `..common`, `super::common` | The parent of the importing file's directory |
| `...common`, `super::super::common` | Two directories above the importing file's directory |
| `crate.config`, `crate::config` | The source root |
| `pub::pkg`, `pub::pkg.sub` | The dependency package `pkg` (see [Package namespaces](#package-namespaces)) |
| `std.name` | The Incan standard library (see [The `std` root](#the-std-root)) |
| `rust::crate::item` | The Rust crate `crate`; `rust::std::...` is the Rust standard library |

- A run of `n` leading dots climbs `n - 1` directories. Each `super` climbs one. A single leading `.` is refused (`INCAN-P0001`).
- The source root is the `[build] source-root` of `loaf.toml`; else the project's `src/` directory, when present; else the project root, the directory that holds `loaf.toml`.
- Modules may import each other in a cycle. A module that imports itself is refused (`INCAN-T0001`).

A module path `a.b` names the first of these files that exists: `a/b.incn`, `a/b.incan`, `a/b/mod.incn`, `a/b/mod.incan`, `a/b/__init__.incn`, `a/b/__init__.incan`.

```incan
# store/relative.incn
from ..db.schema import Database          # accepted

# store/parent.incn
from super::db::schema import Database    # accepted

# app/store/nested.incn
from ...db.schema import Database         # accepted
```

## Bindings

### Module bindings

`import models` binds the module. Its public functions are called through the binding, `models.parse(path)`, and its public types and traits are named through it in any annotation: `models.Config` is the declaration `from models import Config` binds.

Refused (`INCAN-T0001`): a qualified annotation whose root is not a module binding, or whose member is not a type or trait the module declares. The name `c` bound by `from std.interop import c` is not a module binding; its members, such as `c.i32`, are specified in [`std.interop`](stdlib/interop.md).

```incan
import models

def load(path: str) -> models.Config:     # accepted
    return models.parse(path)
```

### Collisions

- An import binds its local name in the module scope, beside the module's declarations.
- A second binding of a name in the same scope is refused: a repeated import of the same declaration as a duplicate, and an import of a different declaration as ambiguous.
- `std` and `rust` cannot be bound as local names, by a declaration or an import (`INCAN-T0001` on a declaration). The exception is an import that binds the root of its own path, such as `import std.web as std`. `import rust::std` without `as` binds `std` and is refused.

```incan
import codecs.prelude as codecs_prelude
import compression.prelude as compression_prelude   # accepted
import models as std                                # refused: std is reserved
```

### Standard-library type names

- A standard-library type name, such as `Response` from `std.web`, names that type after an import that binds it.
- A module-level `model`, `class`, `enum`, `trait`, `newtype` or `type` declaration of the same name is the type that name refers to throughout the module: in annotations, including the method signatures of the declaration itself, and in calls that construct the type.
- Refused (`INCAN-T0001`): the name in an annotation of a module that neither declares nor imports it.

```incan
model Response:
    body: str

    @staticmethod
    def make(body: str) -> Response:          # accepted
        return Response(body=body)
```

### Builtin functions

- An import with the name of a builtin function, such as `sum`, is what an unqualified call of that name calls.
- `std.builtins.<name>` always calls the builtin function `name`.
- `print` and `println` cannot be bound by a declaration, a local, a parameter or a type parameter (`INCAN-T0001`), or by an import (`INCAN-I0001`). Fields and methods may use those names.

```incan
# report.incn
from aggregates import sum

def report() -> int:
    local_total = sum(41)                     # accepted
    builtin_total = std.builtins.sum([1, 2])  # accepted
    return local_total + builtin_total
```

The builtin functions and types are listed in the [language reference](language.md#builtin-functions).

### Reserved name prefix

Names that start with `__incan_` are reserved for the compiler. A name the source declares or binds with that prefix is refused (`INCAN-T0111`), public or private:

- a module-level function, constant, static, type, trait, alias or partial;
- a field, property, method, method alias, method partial, enum variant or variant alias;
- a parameter or type parameter;
- a local, pattern or closure binding;
- an import alias, or an imported name bound without an alias.

A method named `__incan_new` is exempt: it is a type's constructor hook.

```incan
from helpers import total as __incan_total   # refused: reserved prefix (INCAN-T0111)

pub def __incan_original_target() -> int:     # refused: reserved prefix (INCAN-T0111)
    return 2

class Counter:
    value: int = 0

    @staticmethod
    def __incan_new() -> Self:                # accepted
        return Counter(value=5)
```

## Exports

A module exports:

- each module-level declaration marked `pub`, and each variant and variant alias of a `pub` enum;
- each name that a `from M import ...` of the module binds, with or without `pub`, and each name a `from rust::... import ...` binds.

`from M import X`, and `import M::X` where `M::X` is not a module, bind `X` only when `M` exports it. Refused (`INCAN-I0001`): a name `M` does not export, such as a declaration without `pub`, or a module that `M` binds with `import`.

```incan
# a.incn
pub def f() -> int:
    return 1

def hidden() -> int:
    return 2
```

```incan
# b.incn
from a import f
```

```incan
# main.incn
from b import f          # accepted
import a::f              # accepted
from a import hidden     # refused: hidden is not pub (INCAN-I0001)
import a::hidden         # refused: hidden is not pub (INCAN-I0001)
```

## Re-exports

`pub from M import X [as Y]` publishes `X`, as `X` or `Y`, from the importing module.

- Accepted in a source file under a directory named `src`, for any module-level declaration kind: functions, models, classes, traits, enums, newtypes, type aliases, consts, statics, aliases and partial presets.
- A re-export of an alias, or of a dependency's re-export, publishes the declaration it names: a call through each re-exported name calls that declaration.

Refused (`INCAN-P0001`): `pub from` in a file outside a `src` directory, and `pub import`.

In a project that depends on the library `calc_lib`:

```incan
# calc_lib/src/lib.incn
pub from helpers import calculate as facade_calculate
```

```incan
# src/facade.incn
pub from pub::calc_lib import facade_calculate as b_calculate
```

```incan
# src/main.incn
from facade import b_calculate

def main() -> None:
    println(b_calculate(41))    # accepted
```

`incan build --lib` refuses (`INCAN-C0001`) a library entrypoint that re-exports from an unknown module, re-exports a name its module does not export, or exports one name twice.

## Package namespaces

`incan build --lib` publishes the library's modules under `pub::package`, following its source files below the source root.

- Each source module path, and each directory prefix of one, is a namespace: `src/hyperquant/index.incn` gives `pub::pkg.hyperquant` and `pub::pkg.hyperquant.index`.
- A namespace exposes the `pub` declarations of its own module and of its immediate child modules. Deeper modules stay in their own namespaces.
- Only `pub` declarations are exposed. Private declarations and imports are not.
- A name that two modules of one namespace declare is ambiguous there, and importing or accessing it through that namespace is refused; the exact module path selects one.
- A declared member takes precedence over a child namespace with the same name.
- At the package root, a re-export in `src/lib.incn` and a child namespace with the same name are ambiguous; the exact path selects one.

```incan
from pub::hees_ai import hyperquant                            # accepted
from pub::hees_ai.hyperquant.index import build_index          # accepted
import pub::hees_ai.hyperquant as hq                           # accepted
from pub::codecs.encoding.base64 import encode                 # accepted
from pub::codecs.encoding import encode                        # refused: base64 and hex both declare encode
```

## The `std` root

- `from std import name` binds the standard-library module `std.name`. A name that is not a standard-library module is refused.
- `import std.fs` and every other bare `std...` path name the Incan standard library; the Rust standard library is `rust::std`.
- `std.NAMESPACE.prelude` names the module `std.NAMESPACE`: `from std.async.prelude import spawn` imports what `from std.async import spawn` does. `std.prelude` is a module of its own.
- `from std.NAMESPACE import name` binds the module `std.NAMESPACE.name` when that is a standard-library module, and otherwise the member `name` of `std.NAMESPACE`. `std.derives` has no members: `from std.derives import comparison` imports the module `std.derives.comparison`, and any other name imported from `std.derives` is refused.
- An unknown `std.*` module is refused.

```incan
from std import toml                   # accepted
from std import math as arithmetic     # accepted
from std import Debug                  # refused: Debug is not a standard-library module
from std.derives.string import Debug   # accepted
from std.derives import comparison     # accepted: the module std.derives.comparison
from std.derives import Eq             # refused: std.derives has no members
```

The standard-library modules are listed in the [language reference](language.md#standard-library-namespaces).

## Soft keywords

- `async` and `await` are keywords only after an import whose path begins with `std.async`, such as `import std.async` or `from std.async.time import sleep`, from the next declaration on. `from std import async` does not activate them. `race for` requires the same import.
- A library's vocabulary keywords are keywords after an import of its provider namespace.

Refused (`INCAN-P0001`): `async` or `await` as a keyword before the import.

## Rust crate imports

```text
import rust::CRATE [@ "VERSION"] [with ["FEATURE", ...]]
from rust::CRATE [@ "VERSION"] [with ["FEATURE", ...]] import ITEMS
```

- `@ "VERSION"` is a Cargo SemVer requirement. `with [...]` selects crate features, with or without `@`.
- Without `@`, a known crate takes its default version; an unknown crate without `@` is refused.
- Annotations are refused on a crate that `loaf.toml` configures.
- Across files, one crate's versions match, and its features are unioned.
- Annotations apply to `rust::` imports only.
- A crate that `loaf.toml` declares under `[dependencies]`, the table of Incan library dependencies, and under neither `[rust-dependencies]` nor `[rust-dev-dependencies]`, is refused.
- A crate that `loaf.toml` declares only under `[rust-dev-dependencies]` is refused outside a test file (see [`[rust-dev-dependencies]`](../../tooling/reference/project_configuration.md#rust-dev-dependencies)).
- An empty `@ ""` version requirement, and one that is not a Cargo SemVer requirement, are refused.

These refusals are reported when the project builds or locks (`INCAN-C0001`). `rust::core` and `rust::alloc` are refused (`INCAN-I0001`).

```incan
import rust::my_crate @ "1.0"
import rust::tokio @ "1.0" with ["full"]
from rust::sqlx @ "0.7" with ["runtime-tokio", "postgres"] import Pool
```

## `import python`

`import python "PACKAGE" [as NAME]` parses and is always refused (`INCAN-I0001`).

```incan
import python "requests" as pyreq    # refused (INCAN-I0001)
```

## `import this`

`import this` prints the Zen of Incan when the program starts:

--8<-- "_snippets/language/zen_of_incan.md"

## See also

- [Imports and modules (explanation)](../explanation/imports_and_modules.md)
- [Imports and modules (how-to)](../how-to/imports_and_modules.md)
- [Rust interop (how-to)](../how-to/rust_interop.md)
