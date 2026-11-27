# Engine head-to-head

Compare the Legacy and Records build engines on one installed toolchain, under the rules of [the build engine architecture's acceptance section](../../docs-site/docs/contributing/explanation/build_engine_architecture.md). Both engines use the same language-core revision, archive, installation, fixtures, target and profile. Only `INCAN_ENGINE` and each arm's own Bakery differ.

## Arms

| Arm | Workload | States |
| --- | --- | --- |
| `hello` | `incan run hello.incn`, then the produced executable run directly as the native and launch floor | Prefilled cold, warm unchanged, private-body edit (an equal-width literal), signature edit (string to integer with the caller using the integer). Pair 1 recomputes both final edits on fresh clones with no prior target, untimed, so timed edit series hold only warm samples. |
| `two-homes` | Two homes per engine, installed from the archive, bound to that engine's shared Bakery through `[storage] root` | First home, second home on the shared store, then an unchanged request in each. Both `bakery.view` descriptors must name one store under two distinct views. After each pair, both homes' workers stop and their descriptors and storage roots are withdrawn, so no home stays bound to that pair's Bakery. |
| `graph-lsp` | The Contracts `graph/incan/relationships` fixture: `incan inspect codegraph . --format jsonl`, a `--query defs helper` request, and one `incan-lsp --profile release` session (initialize, open, diagnostics, definition, references, shutdown) | Warm unchanged after an untimed release build. Every normalized reply must equal the first one either engine gave. The export header's `snapshot_id` names the arm's own path and store, so engines compare with it masked, and each engine must keep its own value across pairs. Graph replies stay in `streams/`, and each LSP session's raw reply bodies stay in `graph-lsp/ENGINE/lsp-logs/`. With `--contracts-runner`, setup also runs the stock case under each engine. |
| `census` | The stock Contracts runner with the installed verifier's arguments and serial family order, at each `--census-jobs` value: 2 (two-job CI) and 12 (the runner's machine-capacity default) by default | Prefilled cold, then warm. Per engine, the warm run reuses the cold run's Bakery and retained scratch, so the same realized projects, targets and paths are reused. Each state keeps its own log and temporary directory, and a `retained-state` sample records the scratch, targets, Bakery generation and live workers before and after. The complete verdict set must equal the frozen oracle and keep every committed PASS. |

Pair N runs Legacy first when N is odd and Records first when N is even. Within a pair, each engine runs its whole stage sequence before the other one starts, so warm states keep that engine's own service generation. Before each cold sample, the home's owner, engine and resident workers are stopped and reported.

## Run

Run every command under the host's resource lease, with `INCAN_HOME` set to the installed home and `INCAN` to its launcher.

1. `run.rb seal-prefill BAKERY RECEIPT` records every file of a quiescent Bakery that holds the admitted bundle roots and no project results. Each arm clones it with APFS copies. A clone shares storage blocks with the prefill, so the operating system's page cache is not cold. State the cache condition with `--cache-note`.
2. `run.rb build-runner OUTPUT --source CHECKOUT --prefill BAKERY` builds the stock Contracts runner from the checkout, as the installed verifier does.
3. `run.rb compare --untimed ...` runs one pair of every arm with all output, parity and oracle checks and no clock readings. Run it before scheduling timed arms.
4. `run.rb compare ...` repeats the same work with monotonic clocks and host samples: 20 pairs per edit-loop arm and 5 census pairs per state and jobs setting by default.

`--retire-trees` removes each passed pair's Bakeries, targets and census scratch, and the secondary homes once the two-home arm passes. Samples, logs, streams and receipts stay. A refused pair keeps its trees for diagnosis. `--keep-going` lets the census continue past a refused run, so every engine, state and jobs setting is on record. The first refused census pair keeps its trees and later ones keep their logs.

`compare` refuses a run when the installed launcher, LSP, manifest, archive, fixture tree, PASS baseline, oracle or prefill differ from the pin file. The checkout may differ from the pinned revision only inside this directory and at the pin's `harness_paths`, the Contracts sources both engines share, whose trees the pin also fixes. `pins/c79-r1.json` pins the C79-r1 qualification with the Contracts harness that gives three cases a fresh project per run.

## Output

| File | Content |
| --- | --- |
| `inputs.json` | Frozen inputs: installation, host, checkout, prefill, census oracle and runner digests, settings and inherited environment |
| `samples.jsonl` | Every observation in order: arm, pair, position, engine, stage, state, role, argv, environment overrides, exit, signal, stdout and stderr digests, and in timed runs wall time, first output and host samples |
| `failures.jsonl` | The first refused command of each failed arm, with its evidence |
| `result.json` | Each arm's terminal result and whether the home's own Bakery stayed unchanged |
| `summary.json`, `summary.md` | Medians, quartiles, minimum and maximum per series (arm, stage, state, role, engine and jobs), and p95 from 20 samples. `run.rb summarize OUTPUT` writes them again from the samples. |
