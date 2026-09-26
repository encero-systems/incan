# Behavior-fixture candidates

A candidate is a behavior fixture that cannot be admitted to an area under `../behavior/` yet. It is written in the format of `../behavior/README.md`, and it lives in the subdirectory named after the area family it belongs to (`snapshots/`, `cli/`, …).

Nothing runs a candidate. The runner and the test inventory read only the areas declared in `scripts/test_inventory/dispositions.json` (`fixture_roots`), so a file here is not discovered, checked, or counted.

## What a candidate waits for

Every candidate is named by at least one `open` row in `scripts/test_inventory/dispositions.json`: the retire-class test the candidate twins. The row's note states what the candidate waits for, either an issue number or the Cargo-free bake that `rust::` programs need under the compiler suite (see *Provider bakes and Cargo* in `../behavior/README.md`). A program that twins no retire-class test does not belong here; a compiler bug found while writing a fixture is filed as an issue with the program as its reproduction.

## Admitting a candidate

1. Run the program standalone and correct its header from what it actually shows.
2. Move it into its area under `../behavior/`.
3. Point each row that names it at the new path, as a `twin`.
4. Run that area's root.

The change that fixes what a candidate waits for admits it or deletes it.
