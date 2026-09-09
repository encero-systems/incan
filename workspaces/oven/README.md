# Local Oven intake

This Incan prototype reads the local `loaf.toml` dependency closure. It retains authored aliases separately from canonical local roots and graph-local node handles. It preserves Rust version/feature requests without selecting a registry version or reconstructing compiler symbol facts.

`intake(Path(...))` returns a local topology or an `IntakeError` carrying manifest, literal field segments, optional TOML source location, category and detail. `LocalIntake` is a request/topology type, not a `BuildPlan`. Selected source digests, target/host units, compiler inputs and admission are not available from these manifests alone.

Manifest bytes and their SHA-256 are source evidence only. They are not an effective action key. Parsed requests exclude comments/formatting and are separate from location and raw evidence. No persisted unit identity or digest is invented here.

The bounded schema accepts project identity/scripts, local path dependencies, registry Rust requests and explicit build target/profile/edition intent. Other tables or local feature activation refuse explicitly. RFC119's unsettled Rust exception grammar is not guessed. A nearby Cargo manifest is never read.

The fixture uses the committed `workbench/app`, `pricing` and `catalog` projects. Run its tests from this directory with an explicitly selected authoring compiler/SDK. Validation using a pre-cut compiler is authoring-bootstrap evidence only; it cannot prove the repaired Oven pipeline works. The retained pre-cut compiler successfully baked this source and ran the ten-contract acceptance driver. This proves Incan authoring against that compiler and its explicit SDK, not the repaired Oven pipeline. A separate frontend `check` still refuses its inspection-source receipt; that is recorded independently from the successful native run.

The first filesystem intake accepts a conventional `src/lib.incn` or explicit script. A conventional `src/main.incn` without a declared script remains outside this prototype and refuses explicitly. Absent target/profile/edition remain `None`; the prototype does not select defaults.

The current source contains small adaptations tracked by #1455 (optional-value reassignment), #1461/#1464 (materializing dictionary keys before sorting), and #1462 (preserving constructor evaluation order with sequential bindings). Generated Rust is never edited.

`plan.select_native` is a pure, partial intact-closure selector. It compares explicit runtime/target/toolchain/profile/features facts and checked provider semantic identities, accepting module/facet/direct-link supersets and choosing the least excess. It returns the request and candidate evidence references plus rank. Unknown input classes, malformed sets, incompatible candidates and equal best ranks refuse. A retained bootstrap compiler and genuine generation-7 SDK baked the assertion driver and ran all eight source assertions successfully. This is authoring evidence, not current hot-path compiler acceptance.

This component does not expose roots or publish a plan. The same-session host transport, authenticated candidate/receipt projection, typed owner-to-native-root association, leases and physical admission remain required. Source edition belongs to the root compilation and is deliberately absent from dependency compatibility. No native readiness or current catalog acceptance follows from this comparison alone.

Native feature lists are required on both request and candidate contexts and compared as validated sets; provider modules/facets retain their separate subset relation. `tests/fixtures/plan_missing_features/main.incn` is the validated negative frontend fixture: its check used the normal `src/plan.incn` module and required the missing required field diagnostic, not an import failure. With the normal plan module resolved, this fixture produced exactly the missing `features` field diagnostic. The positive driver called all eight assertion functions and printed `selector: 8 contracts passed` through a locked native run.

The selector assertions retain two tracked source adaptations: #1470 binds an expected model value before equality, and #1471 explicitly types nested string-list test cases. Neither changes the assertions or generated Rust by hand. The final complete authoring gate passed in 140.88 seconds, including input reconciliation; the existing bootstrap compatibility baker was used. This does not restore Cargo authority to the current Oven implementation.

The retained native assertion entrypoint is `src/plan_acceptance.incn`. Its eight calls were executed from the bounded authoring project using the same selector/test modules. The repository copy preserves that driver, with formatting-only separation before `main`; current Oven request/response transport remains unimplemented. The existing intake acceptance driver and project manifest are unchanged.


## Selection JSON adapter (source implementation)

`src/plan_json.incn` strictly decodes `incan.oven.selection/1` and calls the existing typed selector. Every wire field
is required, including context `features`, runtime `unknown_input_keys`, and `unsupported_intent`. Unknown keys and
wrong types are refused at each nested object; array diagnostics use decimal path segments. Duplicate JSON object
keys retain `std.json` parsing behavior. No stricter duplicate-key guarantee is claimed.

The request is `{"schema":"incan.oven.selection/1","request":...}` with the exact `SelectionRequest` field names.
The response repeats `schema`, `request_id`, and `request_evidence_id`, then has either `status: "selected"` and the
existing `NativeSelection` under `selection`, or `status: "refused"` and `{kind, fields}` under `error`. Decode
failures use empty binding strings and never contain `selection`; semantic refusals preserve the decoded binding.
The host must authenticate the original request and candidate evidence, not trust an echoed identifier as authority.

`src/plan_json_main.incn` accepts exactly `REQUEST RESPONSE` file paths and performs strict UTF-8 exchange. This is
source implementation only: scoped formatting and the test module frontend check passed with retained compiler
`c7d758d4`; the CLI frontend stopped at missing Rust inspection authority. The first native bake stopped during
dependency preparation before compiling these sources. All eight functions in `test_plan_json.incn` and actual
file exchange remain unexecuted. It does not implement the governed host process boundary, catalog integration, root exposure or publication.
The future host must enforce regular explicit files, 1 MiB request/response limits, bounded diagnostics and deadline,
response binding, physical artifact validation and leases. The adapter does not itself enforce those host bounds.

### Source-unit batch selection (source implementation)

The same `plan_json_main REQUEST RESPONSE` adapter now recognizes `incan.oven.source-unit-batch/1` alongside the unchanged `incan.oven.selection/1` request. A batch embeds one selection/1 foundation request plus explicit library purpose, source units, checked provider edges, reserved needs and candidate-associated grants. Each unit retains the complete `native/source-unit.json` schema1 definition and all four existing `ProviderIdentity` fields.

Incan first computes whole-batch bindings for each offered foundation, then calls the existing selector once on the fully binding candidates. It returns the chosen foundation and original unit, definition, slot/alias, edge and grant handles atomically. An incomplete narrow candidate does not block a usable candidate. Malformed offered facts refuse before filtering; no fully binding candidate is an ordinary incompatibility refusal. Even empty authored dependency lists require an actual foundation and checked reserved support grants.

This first component supports checked public-provider and private SDK associations for one explicit profile. Registry, Git and path requests remain fully represented but active requests refuse; development requests remain inactive for library purpose, and test purpose is unsupported. It does not change ordinary debug/release behavior or claim a complete dual-profile command connection. The host must authenticate candidates and original handles, keep leases, enforce its 1 MiB/process limits, and validate all bindings before physical effects. No graph traversal, Cargo resolution, filesystem lookup or native readiness is granted by the response.

`test_source_unit.incn` contains twelve synthetic batch contracts; eight adapter assertions remain in `test_plan_json.incn`. The selector, batch implementation, JSON adapter and a driver calling all twenty assertions pass typechecking and Rust emission with the retained authoring compiler. Those assertions have not executed natively. Same-session host transport, actual catalog/pricing execution and physical attachment remain pending; synthetic facts are not admitted artifacts.
