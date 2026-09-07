# Build and consume an Incan library

This tutorial creates a reusable Incan library, exports a deliberately small public API, and consumes it from a second project through a path dependency. Both projects live in one workspace so they share portable lock state.

<aside class="inc-tutorial-meta" aria-label="Tutorial details">
  <dl>
    <div><dt>Reader</dt><dd>Package or application developer</dd></div>
    <div><dt>Prerequisites</dt><dd>Modules, manifests, and Getting Started</dd></div>
    <div><dt>Time</dt><dd>30–45 minutes</dd></div>
    <div><dt>Verified</dt><dd>Native library build and locked consumer execution verified with Incan <code>&gt;=0.5.0-0,&lt;0.6.0</code></dd></div>
    <div><dt>Status</dt><dd>Release-envelope executable</dd></div>
    <div><dt>Outcome</dt><dd>A built local library and running locked consumer</dd></div>
    <div><dt>Artifacts</dt><dd>Workspace manifest, library artifact, consumer, and lockfile</dd></div>
  </dl>
</aside>

<ol class="inc-step-rail" style="--inc-step-count: 5" aria-label="Library tutorial steps">
  <li><strong>Produce</strong>Create the library project</li>
  <li><strong>Export</strong>Choose the public surface</li>
  <li><strong>Build</strong>Emit the library artifact</li>
  <li><strong>Consume</strong>Import through <code>pub::</code></li>
  <li><strong>Lock</strong>Make resolution repeatable</li>
</ol>

The complete producer and consumer live at `examples/advanced/library_package`.

## Step 1: Create the producer

Create this workspace layout:

```text
incan.toml
producer/
├── incan.toml
└── src/
    ├── lib.incn
    └── pricing.incn
```

The root `incan.toml` defines both projects as workspace members:

```toml
[workspace]
members = ["producer", "consumer"]
default-members = ["pricing_app"]
```

`producer/incan.toml` identifies the package:

```toml
[project]
name = "pricing_core"
version = "0.1.0"
```

Put reusable implementation in `src/pricing.incn`:

```incan
pub model LineItem:  # (1)
    """One priced item whose quantity participates in a subtotal."""

    pub name: str
    pub quantity: int
    pub unit_price: float

pub def subtotal(item: LineItem) -> float:  # (2)
    """Return `quantity * unit_price` for one line item."""

    return float(item.quantity) * item.unit_price
```

1. The type and the fields used by consumers are public at the producer module boundary.
2. The function accepts the domain type rather than exposing storage or backend details.

## Step 2: Define the exported API

`src/lib.incn` is the library root:

```incan
pub from pricing import LineItem, subtotal  # (1)
```

1. `pub from` is the library's deliberate export list; declarations omitted here stay internal even if another producer module can see them.

The export is explicit. A declaration being `pub` inside `pricing.incn` makes the model visible to the library root, while its `pub` fields can be read across module boundaries. The `pub from ...` line decides which declarations downstream packages receive.

## Step 3: Build the library

From `producer/`:

```bash
incan lock
incan oven bake --project . --format json
incan build --lib
```

The explicit bake seals the producer's project extension and checked library sidecars. The build emits a `.incnlib` artifact under `target/lib/` together with generated library output. Consumers use the checked public manifest rather than importing the producer's private source paths.

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
from pub::pricing import LineItem, subtotal  # (1)

def main() -> None:
    item = LineItem(name="keyboard", quantity=2, unit_price=79.5)
    println(f"{item.name}: {subtotal(item)}")
```

1. `pub::pricing` selects the dependency's checked public namespace rather than a path into its source tree.

`pub::pricing` is a package boundary. It is not a path into `producer/src/`.

## Step 5: Lock and run

From the workspace root:

```bash
incan lock
incan oven bake --project . --format json
incan run --member pricing_app --locked
```

The workspace bake discovers the producer and consumer, publishes the required project extensions and completed outputs, and names their exact full-standard-library base. The normal run then reuses those checked artifacts without invoking Cargo. Commit the generated root `oven.lock`; member-local lockfiles are not workspace authorities. When the producer changes its public API, bake and build it again rather than copying source files between projects.

<section class="inc-learning-panel inc-learning-panel--complete inc-incus-slot" data-label="Complete" data-incus-category="success" markdown="1">

You selected a public API, built its checked library artifact, locked the workspace, and ran a consumer through the public package boundary.

</section>

## Continue

- [Managing dependencies](../how-to/dependencies.md)
- [Project configuration](../reference/project_configuration.md)
- [Imports and modules](../../language/reference/imports_and_modules.md)
