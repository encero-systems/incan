# Compiler record contracts

`internal_models` holds the compiler's shared record contracts: the values a compiler producer publishes and the scoped reads it makes. A consumer that only names a compiler fact depends on this Loaf, not on the lowering, Oven or the worker lifecycle. It depends on no native compiler object; the frontend's native adapters translate to and from these values outside it.

**Versioning:** Release-train cadence, with the driver that links it.

| Module | Holds |
| --- | --- |
| `record_scope` | Revision-bound reads over one pinned snapshot: full record keys, selections, read evidence, witnesses, faults and the one complete receipt. Sources own bytes; the scope owns read evidence. |
| `records` | The public read boundary over `record_scope`. |
| `record_service` | The DD-0013 compiler producer interface over those reads: stages, ranks, preparation and evaluation. |
| `checker_records` | The canonical checker record schema, its bounded field codec and its semantic digest, for the initial free-function profile. |

## Contract

- **Reads.** `record_service` reuses `records` for keyed reads. Preparation distinguishes final inputs, earlier prerequisites, semantic diagnostics and compiler faults; evaluation distinguishes staged results, semantic diagnostics and compiler faults. An unfinished supplier during evaluation is a typed contract fault.
- **Contracts only.** This Loaf does not implement record-scoped checking or plan production. The current reader's table positions and boolean witnesses cannot certify revision-bound read roots. Producers require the existing checker and Body IR operations to consume final keyed environments before they can implement this interface; whole-program checking or lowering followed by record slicing does not satisfy it.
- **Canonical values.** A checker record is a profile, a checked stable owner, a position-free semantic root and ordered fields. Native objects never enter the codec, and an unsupported capability is refused, never silently omitted.
- **Compatibility.** `incan_mir_lowering` re-exports the `records` and `record_service` names from its facade.

The cases are in `tests/test_record_scope.incn` and `tests/canonical_record_controls.incn`. The canonical controls compare against the frontend's native adapter, which stays out of this Loaf's manifest, so `scripts/test-checker-records.sh OVEN EVIDENCE_DIRECTORY` runs them in an isolated project that adds it.
