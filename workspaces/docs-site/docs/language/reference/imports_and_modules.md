# Imports and modules (reference)

This page specifies import forms, module paths and files, what an import binds, re-exports, package namespaces, the `std` root, soft keywords, and Rust crate imports. Refusals of an import are reported with `INCAN-I0001`, and syntax errors with `INCAN-P0001`, unless a rule below names another code.

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
import db.models                              # accepted: binds models
import db::models::User                       # accepted: binds User
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
from ..db.schema import Database          # accepted: db/schema.incn, beside store/
from super::db::schema import Database    # accepted: the same module

# app/store/nested.incn
from ...db.schema import Database         # accepted: db/schema.incn, two directories above app/store/
```

## Bindings

### Module bindings

`import models` binds the module. Its public functions are called through the binding, `models.parse(path)`, and its public types and traits are named through it in any annotation: `models.Config` is the declaration `from models import Config` binds.

Refused (`INCAN-T0001`): a qualified annotation whose root is not a module binding, or whose member is not a type or trait the module declares. The C namespace bound by `from std.interop import c` is not a module binding; its spellings, such as `c.i32`, follow [Checked C bindings](../how-to/checked_c_bindings.md).

```incan
import models

def load(path: str) -> models.Config:     # accepted
    return models.parse(path)
```

### Collisions

- An import binds its local name in the module scope, beside the module's declarations.
- A second binding of a name in the same scope is refused: a repeated import of the same declaration as a duplicate, and an import of a different declaration as ambiguous. The first binding stays in effect.
- `std` and `rust` cannot be bound as local names, by a declaration or an import (`INCAN-T0001` on a declaration). `import rust::std` without `as` binds `std` and is refused.

```incan
import codecs.prelude as codecs_prelude
import compression.prelude as compression_prelude   # accepted: distinct local names
import models as std                                # refused: std is reserved
```

### Builtin functions

- An import with the name of a builtin function, such as `sum`, is what an unqualified call of that name calls.
- `std.builtins.<name>` always calls the builtin function `name`.
- `print` and `println` cannot be bound by a declaration, an import, a local, a parameter or a type parameter. Fields and methods may use those names.

```incan
# report.incn
from aggregates import sum

def report() -> int:
    local_total = sum(41)                     # accepted: calls aggregates.sum
    builtin_total = std.builtins.sum([1, 2])  # accepted: calls the builtin
    return local_total + builtin_total
```

The builtin functions and types are listed in the [language reference](language.md#builtin-functions).

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
    println(b_calculate(41))    # accepted: calls calc_lib's helpers.calculate
```

`incan build --lib` refuses a library entrypoint that re-exports from an unknown module, re-exports a name its module does not export, or exports one name twice.

## Package namespaces

`incan build --lib` publishes the library's modules under `pub::package`, following its source files below the source root.

- Each source module path, and each directory prefix of one, is a namespace: `src/hyperquant/index.incn` gives `pub::pkg.hyperquant` and `pub::pkg.hyperquant.index`.
- A namespace exposes the `pub` declarations of its own module and of its immediate child modules. Deeper modules stay in their own namespaces.
- Only `pub` declarations are exposed. Private declarations and imports are not.
- A name that two modules of one namespace declare is ambiguous there, and importing or accessing it through that namespace is refused; the exact module path selects one.
- A declared member takes precedence over a child namespace with the same name.
- At the package root, a re-export in `src/lib.incn` and a child namespace with the same name are ambiguous; the exact path selects one.

```incan
from pub::hees_ai import hyperquant                            # accepted: the namespace
from pub::hees_ai.hyperquant.index import build_index          # accepted
import pub::hees_ai.hyperquant as hq                           # accepted
from pub::codecs.encoding.base64 import encode                 # accepted
from pub::codecs.encoding import encode                        # refused: base64 and hex both declare encode
```

## The `std` root

- `from std import name` binds the standard-library module `std.name`. A name that is not a standard-library module is refused, naming the module that declares it when that is known.
- `import std.fs` and every other bare `std...` path name the Incan standard library; the Rust standard library is `rust::std`.
- An unknown `std.*` module is refused.

```incan
from std import toml                   # accepted: the module std.toml
from std import math as arithmetic     # accepted: the module std.math, as arithmetic
from std import Debug                  # refused: Debug is declared in std.derives.string
from std.derives.string import Debug   # accepted
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

These refusals are reported when the project builds or locks.

```incan
import rust::my_crate @ "1.0"
import rust::tokio @ "1.0" with ["full"]
from rust::sqlx @ "0.7" with ["runtime-tokio", "postgres"] import Pool
```

## `import this`

`import this` prints the Zen of Incan when the program starts:

--8<-- "_snippets/language/zen_of_incan.md"
