# Stateful compiler callbacks authored in Incan

Both callback milestones now execute with the isolated dev.8 compiler patch: rustc's existing `TimePassesCallbacks`, and a source-authored `Callbacks` implementation that mutates its own state and rustc's configuration. The consumer is wholly Incan. It imports rustc libraries directly, not the current Incan frontend, Body IR, Plan, lowering or adapter. No Rust consumer shim or generated Rust source was added.

## Complete working source

```incan
from rust::rustc_driver import Callbacks, run_compiler
from rust::rustc_interface import Config


# The Rust callback interface supplies Config; the behavior and state live here.
class Hooks with Callbacks:
    pub calls: int = 0
    flag: str

    def config(mut self, mut config: &mut Config) -> None:
        self.calls += 1
        config.crate_cfg.append(self.flag)


def main() -> None:
    mut first = Hooks(flag="incan_callback_first")
    mut second = Hooks(flag="incan_callback_second")
    assert first.calls == 0
    assert second.calls == 0
    run_compiler(["surface-probe", "--print", "cfg"], first)
    assert first.calls == 1
    assert second.calls == 0
    run_compiler(["surface-probe", "--print", "cfg"], second)
    assert first.calls == 1
    assert second.calls == 1
```

`Callbacks` and `run_compiler` come from the pinned rustc driver; `Config` is its public `rustc_interface` re-export. The callback increments the selected instance and appends that instance's cfg flag. The program performs two compiler-library invocations in the same process. Assertions prove both initial counters are zero, the first invocation changes only the first instance, and the second leaves both counters at one. Rustc prints each instance's flag exactly once and prints its target architecture twice.

The observed output includes:

```text
incan_callback_first
target_arch="aarch64"
incan_callback_second
target_arch="aarch64"
```

The executable exits zero. This is configuration injection into real compiler sessions, not typed target-body injection and not a compiler subprocess wrapper.

## Refusal controls

| Probe | Observed result |
| --- | --- |
| Native TimePassesCallbacks | Builds and executes target cfg |
| Source class using native default methods | Builds and executes target cfg |
| Stateful source callback | Both invocations and all independent-state assertions pass |
| Immutable callback local | Checking refuses the required mutable borrow |
| Wrong native callback type | Checking reports a type mismatch |
| Wrong source method receiver | Native E0053 rejects the ABI |
| Missing required foreign method | Native E0046 reports missing fmt |
| Rc-backed callback | The native solver refuses conversion to Callbacks + Send |
| Access to an unsupported native field | Plan validation refuses projection of an opaque native field |

The native solver proves the complete reference-to-trait-object conversion before unsizing. The principal trait and auto traits are retained. Actual rustc impl headers forward to the Incan-authored method; rustc checks signatures, required methods, defaults and coherence. No callback name is hardcoded and no vtable is fabricated.

## Minimum surface revealed

The tested source needs ordinary classes, fields, instance methods, mutable references, strings, lists, assertions, native imports, concrete foreign trait adoption and concrete-reference-to-trait-object conversion. It does not yet need a new compiler DSL. Native structs can remain opaque except for supported selected fields. Lowering retains slot identity and visibility; the native adapter checks actual metadata and refuses operations on unsupported fields. It does not reinterpret unknown native layouts.

The callback build reuses the native runtime and core Loaves, plus their exact native artifacts, including derive support. List helpers need executable runtime templates; a runtime rlib alone was insufficient. The entry-only probe still needs no standard-library Loaves. The SDK remains physically intact: this is a reduced selected dependency closure, not a minimal distribution or Rust no_std claim. No compiler implementation Loaves are consumer dependencies.

## Experimental compiler changes

The isolated seed compiler gained general foreign-trait intent and canonical method mappings, explicit borrowed trait-object conversion, solver validation, foreign impl forwarding declarations, and native-field opacity with revalidation. Existing persisted Body IR records are reused; their schema was not expanded. Native storage discovery no longer recursively imports unused foreign fields' carrier graph. The existing runtime and standard-library providers were reused rather than rebuilt.

These changes include Rust in the seed compiler's existing boundary and Incan in its lowering library. This proves an all-Incan consumer, not that the seed compiler is already self-hosted. The local issue draft CA-2 tracks the limitation and experimental remedy. There is no public API adoption, public PR or release qualification.

## Remaining limits

- This slice covers concrete classes and nongeneric method-only foreign traits. Generic traits/classes, associated types/constants and higher-ranked callable authoring are not established.
- Use the public `rustc_interface.Config` re-export in this seed. Importing the same type through `rustc_interface.interface` exposed a field-identity mismatch, recorded for follow-up rather than concealed by a name-based lookup.
- Unsupported native field carriers remain inaccessible. In particular, this slice does not grant write access to lifetime-erased representations of static borrowed fields.
- Native errors in synthesized impl declarations can still point at line 0. Improving source-attributed diagnostics is future work.
- Compiler-private admission still uses the explicit local experimental switch and diagnostic unit preparation. A supported capability/provider surface is not defined here.
- No target function/body has been injected. The next spike should author the smallest target declaration and body from Incan, then compare valid execution and invalid-input diagnostics, without importing the current Incan Plan or adapter.

## Verification and reproduction

Run `sh scripts/verify-callbacks.sh` and `sh scripts/verify-reduced.sh` from `/Volumes/Builds/incan-compiler-surface-spike`. They capture raw logs under `evidence/reduced/` and only run a newly successful build. Callback assertions and expected refusal messages are checked, not merely exit codes. `evidence/callback-verification.txt` and `evidence/callback-regression.txt` retain the acceptance runs.

The experimental driver is built through installed dev.8 Oven with the copied matching SDK. Rust formatting, Incan formatting and the fast quality/rustdoc gates are recorded separately. No full repository census, full facet suite or release qualification is claimed.
