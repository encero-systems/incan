# Build and consume an Incan library

Create two ordinary Loaves: a producer exporting one function and a consumer calling it.

<aside class="inc-tutorial-meta" aria-label="Tutorial details"><dl>
<div><dt>Reader</dt><dd>Package author</dd></div>
<div><dt>Prerequisites</dt><dd>Getting started and modules</dd></div>
<div><dt>Time</dt><dd>15 minutes</dd></div>
<div><dt>Verified</dt><dd>Incan <code>0.6.0-dev.8</code></dd></div>
<div><dt>Status</dt><dd>Development-toolchain executable</dd></div>
<div><dt>Outcome</dt><dd>A native library consumed by another project</dd></div>
<div><dt>Artifacts</dt><dd>Two manifests, sources and compiled library sidecars</dd></div>
</dl></aside>

## Create the producer

```bash
mkdir -p producer/src consumer/src
```

Write `producer/loaf.toml`:

```toml
[project]
name = "pricing_core"
version = "0.1.0"
```

Write `producer/src/lib.incn`:

```incan
"""Price calculations exported to consumers."""

pub def subtotal(quantity: int, unit_price: int) -> int:
    """Return the total for a quantity at one unit price."""
    return quantity * unit_price
```

Build the library by its source:

```bash
incan build producer/src/lib.incn
```

A source named `lib.incn` selects a library. Its artifact set includes the rlib, `.incnlib`, `.incnbody` and native metadata. Keep these artifacts together.

## Create the consumer

Write `consumer/loaf.toml`:

```toml
[project]
name = "pricing_app"
version = "0.1.0"

[project.scripts]
main = "src/main.incn"

[dependencies]
pricing = { path = "../producer" }
```

Write `consumer/src/main.incn`:

```incan
"""Use the pricing library."""

from pub::pricing import subtotal

def main() -> None:
    """Print the price of two items."""
    println(subtotal(2, 79))
```

## Run the consumer

```bash
incan run consumer/src/main.incn
```

The program prints `158`. Oven prepares the producer from the declared path before compiling the consumer. The dependency alias `pricing` determines the import namespace. No workspace root, lock command or explicit bake is required.

<section class="inc-learning-panel inc-learning-panel--complete" data-label="Complete" markdown="1">

You exported a public function, compiled a library and called it from a separate project through its declared dependency.

</section>

## Continue

- [Managing dependencies](../how-to/dependencies.md)
- [Project configuration](../reference/project_configuration.md)
