# Measurement scope and unresolved mechanisms

The accompanying [execution](results.md), [build](build_times.md), and [development-loop](dev_loop.md) tables are controls on a packed and freshly reinstalled dev.8 seed; the development-loop page's first two tables instead compare the integration lane's source-built toolchain before and after the resident compiler served checks, formatting and compilation and units named the toolchain's search index. They do not certify a source rebuild of the integrated compiler. The seed archive SHA-256 is `727e961d5769f0f874f72af332f75f3e254ee8ec7939bbb062a2b5b07c3cd900`; the driver is `8c6c71701034a646c43c58042a022bbc07d8c6be2603e8e72c1c142d1b484e52`, and Oven is `532169638e946130a1f7fc2d2841b828783f2d717dfd037bdba32ec384685351`. The installed Rust compiler is 1.98.0. Source-built runtime, matching standard packages, and compiler fixed-point proof remain unestablished.

The final controls run after native rebuild attempts stopped. Every workload retains `uptime`, and the development-loop subjects rotate within each round. Historical August times were rounded to whole milliseconds and were collected on a different compiler and load regime. Contemporaneous Rust measurements are the stronger comparison. No performance improvement is claimed.

An ABI-file interruption affected the last two development-loop rounds. Both active homes lost the configured file; the archived copy remained intact. A fresh extraction with the selected driver is byte-identical to that copy, with SHA-256 `967c1162700d00313efbcd027b9d1b96c318c4f64f422ed64016d957e2db05d8`. The file was restored and whole affected rounds were recollected. Original interrupted samples remain available. No removal cause is attributed.

## Source-build refusals

Native source rebuilding stopped at the following refusals. These are distinct, reproduced failures:

| Component | Refusal |
| --- | --- |
| Native lowering | `unsupported Body IR tuple element type usize with carrier USize`; an earlier occurrence named `native_callable_storage`. Scalar copies and direct error propagation advanced preceding refusals, but the complete producer still failed. |
| Runtime | `RustStr.split_whitespace` has no admitted callable signature, and the panic-hook closure does not match the higher-ranked boxed native callback signature. |
| Core standard package | `incan_native_runtime::freeze_list`: external metadata differs from the admitted safe scalar signature. |
| Data standard package | Dependency preparation reaches the same core `freeze_list` refusal. |
| Oven with the packed seed | No retained canonical callable for the installed `Dag` constructor required by current source. |
| Oven with an earlier installation | Unsupported `VersionReq` nominal carrier in a `Result` payload. |

The full seed census was run twice over 1,171 identical keys: 1,096 PASS, 75 FAIL, zero pending. Every verdict is identical and no passing key was lost. This is seed-route evidence, not execution of the unbuilt modified compiler.

## Execution mechanisms

All six executable workloads match their Rust and Python stdout exactly. N-body is refused at `f64.sqrt`; mergesort is refused at `List.reserve`. The additional standalone OrdinalMap build is refused at its existing `08d` format specifier. No workload expectation or algorithm was weakened.

Emitted MIR and LLVM IR were compared with the pinned Rust baseline for every flagged executable workload. On this host `sample` could not inspect the recorded owned process; the IR and linked disassembly provide the profiling evidence instead.

| Workload | Observed mechanism | Remaining work |
| --- | --- | --- |
| Fibonacci | Signed remainder is followed by negative-result adjustment and a select; Rust's positive-only benchmark uses truncating remainder. | Preserve the required modulo behavior while proving nonnegative loop values before eliminating normalization. |
| Collatz | LLVM retains `smul.with.overflow` and `sadd.with.overflow` branches in the recurrence and accumulator. | Carry the selected build policy into lowering. RFC 009 and the numeric reference require ordinary release arithmetic to wrap; explicit checked operations must remain checked. |
| GCD | The linked library function enters its module initializer on every call; that initializer invokes the runtime dictionary-based initialization guard. | Provide a cheap completed-state path with the same initialization order and reentrancy behavior. A diagnostic direct-Rust-primitive probe produced the same checksum at about 155 ms, but was not a paired speedup experiment. |
| Mandelbrot | The floating recurrence remains native; checked integer accumulation remains in the surrounding loops. | The historical threshold is flagged, while contemporaneous Rust is close. Separate the checked bookkeeping cost from host/load differences before attributing the entire historical delta. |
| Primes | The constant comprehension grows a vector element by element (`RawVec<bool>::grow_one`), while Rust uses `llvm.memset` and vector reduction. | Retain exact range cardinality for allocation/fill optimization and preserve indexing and sum semantics. |
| Quicksort | Index normalization is inlined, with signed/negative-index handling and checked integer bookkeeping retained. | Prove the generated indices' range and honor release arithmetic policy without removing checks from arbitrary user indices. |

These mechanisms remain unresolved. Changes to lowering or runtime cannot be certified through the required native source build while the producer refusals above remain. No alternate producer was substituted.

## Development-loop interpretation

The three shapes use the same positive generation function; run samples verify its exact value, and tests execute after edits to that function. Fresh and unchanged debug/release builds, checks, tests, formatting, inspection, and direct first/repeated executable launches are retained separately. Each row has 15 samples, minimum-to-maximum spread, successful command count, and applicable target count.

The final dataset has 675 successful commands across 45 rows. Unchanged single-file and two-module commands can meet their run/test targets; library preparation and edited/new-project commands remain above their targets. Checking alone consumes roughly a second in this seed, before the additional native compilation cost of an edit. The selected driver rejects `--resident`, while the integrated source contains that worker interface. Repeated fresh compiler startup and metadata preparation therefore remain a measured boundary, rather than proof of current resident-worker performance. Warm library commands retain a separate dependency-preparation cost even when binaries are reused.

First direct launches and repeated launches expose the host's first-execution cost separately. No fixed 140 ms allowance is subtracted. Edited commands still include fresh compilation costs. Unchanged library commands also repeat dependency preparation in this seed. The integrated resident-compiler implementation is not certified by these controls because its source-built toolchain could not be produced.

Reproduce the tables using the benchmark README and `dev_loop/README.md`, selecting a coherent installed toolchain. The raw evidence directory retains exact command logs, census keys and the archive receipt.
