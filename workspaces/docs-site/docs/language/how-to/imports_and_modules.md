# Imports and modules (how-to)

This page shows how to structure multi-file projects and use imports in practice.

Prerequisite: follow [Install, build, and run](../../tooling/how-to/install_and_run.md) so you can run `incan`.

--8<-- "_snippets/callouts/no_install_fallback.md"

## Simple multi-file project

Recommended structure:

```text
myproject/
├── main.incn
├── models.incn
└── utils.incn
```

Example imports:

```incan
from models import User
import utils::format_currency
```

## Nested projects

Recommended structure:

```text
myproject/
└── src/
    ├── main.incn
    ├── db/
    │   └── models.incn
    └── shared/
        └── utils.incn
```

Example imports:

```incan
from db.models import User
import shared::utils::format_date
```

## How module discovery works (practical view)

When you import a local module, the compiler:

1. Resolves the path (handling `.`, `..`, `super`, `crate`)
2. Looks for the `.incn` file (or `mod.incn` for directories)
3. Parses and type-checks that file
4. Makes its types and functions available in your importing file

## Examples from the repo

- Multi-file example: `examples/advanced/multifile/`

Run:

```bash
incan run examples/advanced/multifile/main.incn
```

- Nested project example: `examples/advanced/nested_project/`

Run:

```bash
incan run examples/advanced/nested_project/src/main.incn
```

If you prefer browsing first, see the examples directory on GitHub:
`https://github.com/dannys-code-corner/incan/tree/main/examples`.

## Share module-owned state across files

If a module needs to export live runtime state, use `pub static`:

--8<-- "snippets/module_state.md"

For the full walkthrough, see: [Module state (how-to)](module_state.md).
