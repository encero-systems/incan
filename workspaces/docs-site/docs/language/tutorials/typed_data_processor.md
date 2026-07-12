# Build a typed data processor

This tutorial builds a small JSON-in/JSON-out order processor. It combines models, JSON derives, file I/O, error handling, modules, collection transforms, and tests in one runnable project.

<ol class="inc-step-rail" style="--inc-step-count: 5" aria-label="Data processor tutorial steps">
  <li><strong>Model</strong>Define input and output contracts</li>
  <li><strong>Transform</strong>Keep valid orders and calculate totals</li>
  <li><strong>Connect</strong>Read and write JSON at the boundary</li>
  <li><strong>Test</strong>Exercise the pure transformation</li>
  <li><strong>Run</strong>Produce a report</li>
</ol>

The complete example lives at `examples/advanced/typed_data_processor` in the Incan repository.

## Step 1: Create the project

Use this layout:

```text
typed_data_processor/
├── incan.toml
├── orders.json
├── src/
│   ├── domain.incn
│   ├── transform.incn
│   └── main.incn
└── tests/
    └── test_transform.incn
```

The separation is intentional: `domain.incn` owns contracts, `transform.incn` stays pure, and `main.incn` owns filesystem effects.

## Step 2: Define typed contracts

Create `src/domain.incn`:

```incan
from std.serde import json

@derive(Debug, Clone, json)
pub model Order:
    id: str
    product: str
    quantity: int
    unit_price: float

@derive(Debug, Clone, json)
pub model OrderBatch:
    orders: list[Order]

@derive(Debug, Clone, json)
pub model OrderSummary:
    id: str
    product: str
    total: float

@derive(Debug, Clone, json)
pub model OrderReport:
    accepted: list[OrderSummary]
    rejected_count: int
```

`@derive(json)` supplies typed serialization and deserialization. The JSON boundary is checked against these model fields instead of being passed through as an unstructured dictionary.

## Step 3: Write the transformation

Create `src/transform.incn`:

```incan
from domain import OrderBatch, OrderReport, OrderSummary

pub def build_report(batch: OrderBatch) -> OrderReport:
    mut accepted: list[OrderSummary] = []
    mut rejected_count = 0

    for order in batch.orders:
        if order.quantity > 0 and order.unit_price >= 0.0:
            accepted.append(OrderSummary(
                id=order.id,
                product=order.product,
                total=float(order.quantity) * order.unit_price,
            ))
        else:
            rejected_count += 1

    return OrderReport(accepted=accepted, rejected_count=rejected_count)
```

This function knows nothing about files. Keeping the domain transformation pure makes it straightforward to test and reuse.

## Step 4: Connect the file boundary

In `src/main.incn`, read the source through `std.fs.Path`, parse it through the model, and write the derived report:

```incan
from domain import OrderBatch
from transform import build_report
from std.fs import Path
from std.serde.json import Deserialize, Serialize

def main() -> None:
    input_path = Path("orders.json")
    output_path = Path("order-report.json")

    match input_path.read_text("utf-8", "strict"):
        case Err(error):
            println(f"Could not read input: {error.message()}")
        case Ok(source):
            match OrderBatch.from_json(source):
                case Err(error):
                    println(f"Invalid order data: {error}")
                case Ok(batch):
                    report = build_report(batch)
                    match output_path.write_text(report.to_json(), "utf-8", "strict", None):
                        case Err(error): println(f"Could not write report: {error.message()}")
                        case Ok(_): println(f"Wrote {len(report.accepted)} accepted orders")
```

There are two failure domains, so they remain explicit: filesystem operations return `IoError`, while typed JSON parsing returns a JSON error.

## Step 5: Test the transformation

Create `tests/test_transform.incn`:

```incan
from domain import Order, OrderBatch
from transform import build_report
from std.testing import assert_eq

def test_build_report_keeps_valid_orders() -> None:
    batch = OrderBatch(orders=[
        Order(id="A-1", product="keyboard", quantity=2, unit_price=50.0),
        Order(id="A-2", product="invalid", quantity=0, unit_price=12.0),
    ])

    report = build_report(batch)

    assert_eq(len(report.accepted), 1)
    assert_eq(report.accepted[0].total, 100.0)
    assert_eq(report.rejected_count, 1)
```

Run the test and the complete example from the repository root:

```bash
incan test examples/advanced/typed_data_processor/tests
incan run examples/advanced/typed_data_processor/src/main.incn
cat examples/advanced/typed_data_processor/target/tutorial-output/order-report.json
```

<section class="inc-learning-panel inc-learning-panel--complete inc-incus-slot" data-label="Complete" data-incus-category="success" markdown="1">

You built a multi-module program whose filesystem boundary is fallible, whose JSON boundary is typed, and whose transformation is independently testable.

</section>

## Continue

- [File I/O](../how-to/file_io.md)
- [Dynamic JSON](../how-to/dynamic_json.md)
- [Serialization derives](../reference/derives/serialization.md)
- [Fallible and infallible paths](fallible_and_infallible_paths.md)
