# Build and consume an Incan library

This tutorial creates a reusable Incan library, exports a deliberately small public API, and consumes it from a second project through a path dependency.

<ol class="inc-step-rail" style="--inc-step-count: 5" aria-label="Library tutorial steps">
  <li><strong>Produce</strong>Create the library project</li>
  <li><strong>Export</strong>Choose the public surface</li>
  <li><strong>Build</strong>Emit the library artifact</li>
  <li><strong>Consume</strong>Import through <code>pub::</code></li>
  <li><strong>Lock</strong>Make resolution repeatable</li>
</ol>

The complete producer and consumer live at `examples/advanced/library_package`.

## Step 1: Create the producer

Create this layout:

```text
producer/
├── incan.toml
└── src/
    ├── lib.incn
    └── pricing.incn
```

`producer/incan.toml` identifies the package:

```toml
[project]
name = "pricing_core"
version = "0.1.0"
```

Put reusable implementation in `src/pricing.incn`:

```incan
pub model LineItem:
    name: str
    quantity: int
    unit_price: float

pub def subtotal(item: LineItem) -> float:
    return float(item.quantity) * item.unit_price
```

## Step 2: Define the exported API

`src/lib.incn` is the library root:

```incan
pub from pricing import LineItem, subtotal
```

The export is explicit. A declaration being `pub` inside `pricing.incn` makes it visible to the library root; the `pub from ...` line decides what downstream packages receive.

## Step 3: Build the library

From `producer/`:

```bash
incan build --lib
```

The build emits a `.incnlib` artifact under `target/lib/` together with generated library output. Consumers use the checked public manifest rather than importing the producer's private source paths.

## Step 4: Create the consumer

Create a sibling project:

```text
consumer/
├── incan.toml
└── src/
    └── main.incn
```

Declare the local dependency in `consumer/incan.toml`:

```toml
[project]
name = "pricing_app"
version = "0.1.0"

[project.scripts]
main = "src/main.incn"

[dependencies]
pricing = { path = "../producer" }
```

The dependency key becomes the public import namespace:

```incan title="consumer/src/main.incn"
from pub::pricing import LineItem, subtotal

def main() -> None:
    item = LineItem(name="keyboard", quantity=2, unit_price=79.5)
    println(f"{item.name}: {subtotal(item)}")
```

`pub::pricing` is a package boundary. It is not a path into `producer/src/`.

## Step 5: Lock and run

From `consumer/`:

```bash
incan lock
incan run --locked
```

Commit the generated `incan.lock`. When the producer changes its public API, rebuild it and re-run the consumer checks rather than copying source files between projects.

<section class="inc-learning-panel inc-learning-panel--complete inc-incus-slot" data-label="Complete" data-incus-category="success" markdown="1">

You created a checked library artifact, selected a public API, consumed it through `pub::`, and locked the dependency graph.

</section>

## Continue

- [Managing dependencies](../how-to/dependencies.md)
- [Project configuration](../reference/project_configuration.md)
- [Imports and modules](../../language/reference/imports_and_modules.md)
