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
