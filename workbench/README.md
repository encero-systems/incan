# Oven working project

A small multi-package project for exercising the Cargo-free Oven path by hand. It sits deliberately outside the test suite: the point is a fast edit/build/run loop on a graph big enough to expose real integration failures, not coverage.

The project prints an order quotation from a small catalog. The application and the pricing package both depend on the catalog, so the graph contains a shared dependency reached by two package paths. Public models, an enum, defaults, local facades and a Rust regex dependency give it enough structure to be worth measuring.

- `catalog/` — product data and line-price calculation.
- `pricing/` — a package consumer that produces a typed quotation.
- `app/` — the executable, with typed TOML settings and Rust regex for SKU validation.

## Running it

From `app/`:

```sh
incan oven bake --project <package>
incan run --locked src/main.incn
```

Expected output for the unmodified project:

```text
SKU-100: 2 units, 700 cents
SKU-200: 3 units, 375 cents
total: 1075 cents
```

Use a compiler built from the Oven working branch, and record its exact revision, toolchain, and SDK/provider identity alongside any timing you report — a number without that identity cannot be compared to another number. Do not modify an installed toolchain or a Cargo cache to make this project run; needing to would itself be the finding.

## What to measure

The target is a normal edit/build/run iteration finishing within **300 seconds**, without rebuilding unchanged compiler, SDK, or dependency outputs. That budget covers the whole iteration, including rebuild work the edit genuinely requires.

For each run, record preparation, build, run and cleanup wall times, plus the inputs and output digests of every unit that executed or was reused. Keep a cold preparation lane as well: it is the evidence that a warm cache is not quietly standing in for Cargo.

## Edits worth trying

Each of these probes a different invalidation boundary. Keep the previous output and store state so the actual invalidation is observable rather than inferred.

| Edit | Expectation |
| --- | --- |
| `app/quote.toml` | rebuilds nothing — the application reads it from the working directory at run time, and rejects negative quantities |
| `catalog/src/sample.incn` | an implementation change behind a stable interface |
| `pricing/src/quotation.incn` | a public API change, so consumers rebuild too |
| a Rust input, then removing producer source after publication | whether execution survives without the producer's source |

Confirm the result changes as expected, then inspect exactly which units rebuilt. When an iteration exposes a concrete defect, add a focused regression to the compiler suite once the cause is understood — this project is for finding defects, not for holding them.

This file deliberately does not wrap a full-suite or prewarm command around every edit; doing so would hide the behavior being measured.
