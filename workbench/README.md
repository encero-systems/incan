# Oven working project

This is a working project for developing the Cargo-free Oven path. It is deliberately outside the test suite. Edit the application, its packages, and Oven itself here; the normal edit/build/run iteration must finish within **300 seconds** without rebuilding unchanged compiler, SDK or dependency outputs.

The project prints an order quotation from a small catalog. The application and pricing package both depend on the catalog, creating a shared dependency reached through two package paths. Public models, an enum, defaults, local facades and a Rust regex dependency give the graph enough structure to expose real integration failures.

- `catalog/`: product data and line-price calculation.
- `pricing/`: a package consumer that produces a typed quotation.
- `app/`: the executable, with typed TOML settings and Rust regex used for SKU validation.

Use the compiler built from the owning hot-path worktree. Keep its exact path, revision, toolchain and SDK/provider identity in the run record. No installed/global toolchain or Cargo cache should be modified to run this project.

Initial source is prepared; native baseline and the Cargo removal are still pending RFC 123 acceptance. The TOML settings use the accepted #1438 API. There is no claim that the complete project currently builds or meets the time budget.

## Working loop

1. Establish the native baseline and record preparation, build, run and cleanup wall times.
2. Remove Cargo-dependent Oven paths together on this branch and keep that broken checkpoint.
3. Repair the real project path, rerunning only the selected project operation. Keep a 300-second deadline over the entire normal iteration, including required rebuild work.
4. Change catalog prices, the public quotation shape, a Rust input, and then remove producer source after publication. Confirm the result changes correctly and inspect exactly which units rebuild.
5. Preserve a cold preparation lane as evidence that warm caches are not hiding Cargo. Add focused regressions to the compiler suite after each concrete defect is understood.

Current production commands are `incan oven bake --project <package>` and `incan run --locked src/main.incn` from `app/`. The first verified transcript will pin the exact commands and environment. This file intentionally does not wrap a full-suite/prewarm command around every edit.

Expected unmodified output after SKU validation:

```text
SKU-100: 2 units, 700 cents
SKU-200: 3 units, 375 cents
total: 1075 cents
```

Change `app/quote.toml` for a runtime-only edit that should rebuild nothing. The application reads this file from its working directory and rejects negative quantities. Change `catalog/src/sample.incn` for the first implementation-edit experiment. Change `pricing/src/quotation.incn` for a public API edit. Keep previous output/store state so we can observe actual invalidation.
