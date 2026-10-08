## Summary

Issue #1337, stage 4 night lane **stdlib**: standard-library builtins, `Option` carriers of non-scalars, and standard-library nominal types on the direct native route. Each step adds the construct to the Incan lowering (`loaves/compiler/incan_mir_lowering`), the native runtime Loaf where a legacy-identical entry point is needed, and the Rust adapter in the driver Loaf only where rustc needs it. Anything outside a construct stays refused by name. Every step is proven by a focused test in a sibling module of the native driver root (`native_driver_project_tests/stdlib.rs`) that compiles a small program natively and through legacy and requires identical exit status, stdout and stderr, then by the census.

Base: `feature/1337-night-staging` at `4ecd6b3a`.

## Status

- **Done and proven:** steps 1–2 (`a84f6d42`). The focused test `stdlib::direct_route_stdlib_matches_legacy` passes, the census reports 0 wrong, and `make test-inventory` and every `make pre-commit-fast` gate pass.
- **Next: merge current staging.** Staging has moved on (collections 9, bakefast2 4, deps 6, tailmerge 1, models 11, typeparams 4). The merge conflicts in `decls.incn`, `defaults.incn`, `lowering.incn`, `models.incn`, `options.incn` and the test inventory. I'll resolve it with a merge commit, then re-run the focused test and the census on the merged base.
- **In progress: step 3, provider-crate nominals and calls (`IoError`, `EnvironError`, …).** Body IR gains `sdk_nominal_declarations`/`sdk_callables` (native path, checked field layout, checked signature). The lowering reaches them by crate path, and the adapter verifies each provider nominal's layout against dependency metadata before building MIR. This step is implemented and passes `cargo check`, clippy on the touched crates, and the rustdoc gate. It is not yet proven by its focused tests (environ calls, io fields), so it is not in this PR yet.

## Steps and census

Census = `direct_route_fixture_census` (1101 fixtures; 131 check-only and 13 pending are unchanged throughout).

| Step | Commit | pass | refused | wrong | Fixtures that moved |
| --- | --- | ---: | ---: | ---: | --- |
| baseline | `4ecd6b3a` | 410 | 546 | 0 | |
| 1–2. builtins and Option model fields | `a84f6d42` | 412 | 545 | 0 | `emit_calls_and_builtins/file_builtins_report_errors_as_strings`, `execution_strings_and_output/refuses_json_stringify_outside_the_scalar_domain_at_the_call_span` (refused → pass); see below for fixtures that moved to a later refusal |

The baseline's one driver-error (`emit_dependency_unions_and_surfaces/dependency_constructor_surfaces`, a local-library preparation timeout) measured as an ordinary refusal on the rerun; it is load-dependent, not a change of this lane.

### 1. `min`/`max`, `read_file`/`write_file`, JSON lists

- `min`/`max` over `list[int|float|bool|str]` call native-runtime wrappers whose legacy-compiled bodies are `min(values)`/`max(values)`, so they reach the same `incan_std_core::collections::__private::list_{min,max}_{copy,f64,clone}` helpers, empty-sequence `ValueError` text included. Legacy's checker admits `min`/`max` only over owned lists, so the lowering passes the wrapper a `clone_list` copy; the observable result is identical.
- `read_file`/`write_file` call wrappers whose bodies are the legacy builtins, returning the checked `Result[str, str]`/`Result[None, str]` carrier (the plan's carrier alias is the standard `Result`). Wrong arity, non-text operands and discarded results stay refused by name.
- `json_stringify` of a scalar-leaf list (`int`, `float`, `bool`, `str`, any depth) borrows the list into the runtime's generic `serialize_model[T with Serialize]`, the same `stringify_or_raise(value, type_name_of_val(value))` boundary legacy emits.
- Moved fixtures: `file_builtins_report_errors_as_strings` and `refuses_json_stringify_outside_the_scalar_domain_at_the_call_span` pass; `snapshots_collections_and_strings/builtins` now stops at `Method without canonical identity` (a later construct in the same file).

### 2. `Option` model fields

- Model field types resolve through the retained standard carriers (`intrinsic_declarations` now also scans model field types; `lower_model_declarations` and field projection receive the carriers), so `Option[str]`, `Option[int]` and `Option[list[int]]` fields declare, construct (including a defaulted `None`), and read into locals.
- Body IR records every non-Copy projected read as a borrow (`ownership_fact_for_place`), so a carrier or Clone-deriving source enum copied out of a model field into a local is lowered as its derived clone, as string and list field reads already are. Legacy copies or moves the field; the clone is observably identical. Unions stay refused (they share the enum slots after the source enums, so a carrier must also match by name).
- A defaulted `None` is admitted for model fields and function parameters: `default_type` now accepts a retained standard carrier as well as scalars and text, and the existing deferred-default frame evaluates it.
- Guards that keep this from lowering wrongly: a downcast payload binding requires a whole enum local (a carrier read through a model field must be copied out first), and moving an enum out of a model field is refused like a `String` field move.
- Moved fixtures: `snapshots_models_and_classes/models` → `collection leaf List[int]`; `snapshots_models_and_classes/model_with_alias` → `type Unknown`; `lowering_expressions/value_written_to_option_place_is_wrapped` → `nonunion enum isinstance`; `snapshots_values_and_control_flow/exact_float_ingress_and_aggregates` → `ExternDelegation`; `lowering_expressions/native_clone_on_union_and_option_fields` → `Union[...]` type; `codegen_modules_and_imports/qualified_type_annotations` → `nominal type FieldInfo`.

## Buckets left refused, and why

| Bucket (baseline count) | Owner / reason |
| --- | --- |
| `Option[List[int]]` (2): `mut_local_changed_through_a_pattern_binding`, `mut_parameter_changed_through_a_pattern_binding` | collections lane: `list[Option[list[int]]]` needs carrier list leaves (and ownership lane: mutation through pattern bindings) |
| `Option[int]` (1 of 2): `match_arms_and_loop_values_share_one_type` | collections/tuples lanes: `list[Option[int]]` leaves and `Option` tuple elements |
| `FrozenList[str]` (2) | statics/collections lanes: const `FrozenList`/`FrozenDict`/`FrozenBytes` values; Body IR constants carry only scalars and text |
| `Html` (1) | async lane: the fixture's `@route` async handler and web vocabulary |
| `ValidationError` (2) | `FrozenStr`/`FrozenBytes` consts and bytes `encode`/`decode` methods |
| `JsonValue` (2) | newtypes lane: a standard-library `newtype` over a Rust type with `Index` trait implementations, not a provider-crate model |
| `IoError` (7), `EnvironError` (3), `CompressionError` (3), `OrdinalMapError` (2) | this lane, step 3 (in progress): provider-crate nominals and calls |

## Legacy gaps and findings

- Legacy's checker rejects `min`/`max` over a borrowed list (`&list[T]`), although `sum` accepts one; the runtime wrappers therefore take owned lists.
- Legacy emission moves an owned `str` local into `read_file(path)`, so reusing `path` afterwards fails rustc with E0382 (`use of moved value: path`) although the checker accepts the program. The focused test avoids the reuse; the native route borrows the text and would accept it, so this gap is recorded, not matched.
- Pending fixture `contextual_negative_literals_survive_direct_returns_and_same_module_ca_3` observes `-1` vs `-1.0` exactly as at baseline (legacy prints a whole-number `f32` without `.0`, already recorded as its pending reason).
- Environment (not code): this container's offline cargo needed `make fetch-locked-cargo-sources` and `make fetch-oven-loaf-sources`; the duplicate `librustc_driver` copy in `rustc-dev` (also recorded on #2092) needed the same symlink; each driver bake publishes about 2.6 GB of closure units into the bake workspace's Oven store and filled the disk once.

## Type of change

- [ ] Bug fix
- [x] New feature
- [ ] Refactor / maintenance
- [ ] Documentation
- [ ] CI / tooling
- [ ] RFC (adds/updates `docs/RFCs/*`)

## Area(s)

- [ ] Incan Language (syntax/semantics)
- [x] Compiler (frontend/backend/codegen)
- [ ] Tooling (CLI/formatter/test runner)
- [ ] Editor integration (LSP/VS Code extension)
- [x] Runtime / Core crates (stdlib/core/derive)
- [ ] Documentation

## Key details

- **User-facing behavior**: none on the legacy route; more programs compile on the direct native route with legacy-identical output.
- **Internals**: Incan lowering, the native runtime Loaf, and small adapter additions in `incan-rustc-driver`.
- **Risks**: a wrong native lowering; guarded by the focused byte-for-byte legacy comparisons and census `wrong = 0`.

## Testing / verification

- `make test-one TEST_ROOT=loaves/compiler/incan_driver/tests/native_driver_project_tests.rs TEST_EXACT=stdlib::direct_route_stdlib_matches_legacy`: one test process runs every stdlib case, so the driver graph is baked once; the census runs in its own invocation (`TEST_EXACT=direct_route_fixture_census`) because the two together exceed the Oven's 60-minute root timeout.
- The census alone now also exceeds that timeout here (driver bake plus 1101 fixtures: 3600 s, cut off with no evidence). From step 2 on, the census ran the same Oven-built test binary with the environment the Oven passes it, captured from a running root, with no wrapper deadline. The binary, sources and fixture bake are identical; only the wrapper's deadline is absent.
- `make test-inventory` and every `make pre-commit-fast` gate before each push. This container has the pinned `nightly-2026-03-24` toolchain but no toolchain named `nightly`, so the format gate ran as `cargo +nightly-2026-03-24 fmt --all -- --check`.

## Docs impact

- [x] No docs changes needed (the test corpus inventory page is regenerated for the new test module)

## v0.6 ledger update (#1074)

**Active** — #1337 stage 4 night lane *stdlib*: standard-library builtins, `Option` carriers of non-scalars and standard-library nominal types on the direct native route. Branch `feature/1337-night-stdlib-cloud` → `feature/1337-night-staging`. Evidence per step is the focused `stdlib::*` legacy comparison and the census table above; wrong stays 0.
