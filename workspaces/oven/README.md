# Local Oven intake

This Incan prototype reads the local `loaf.toml` dependency closure. It retains authored aliases separately from canonical local roots and graph-local node handles. It preserves Rust version/feature requests without selecting a registry version or reconstructing compiler symbol facts.

`intake(Path(...))` returns a local topology or an `IntakeError` carrying manifest, literal field segments, optional TOML source location, category and detail. `LocalIntake` is a request/topology type, not a `BuildPlan`. Selected source digests, target/host units, compiler inputs and admission are not available from these manifests alone.

Manifest bytes and their SHA-256 are source evidence only. They are not an effective action key. Parsed requests exclude comments/formatting and are separate from location and raw evidence. No persisted unit identity or digest is invented here.

The bounded schema accepts project identity/scripts, local path dependencies, registry Rust requests and explicit build target/profile/edition intent. Other tables or local feature activation refuse explicitly. RFC119's unsettled Rust exception grammar is not guessed. A nearby Cargo manifest is never read.

The fixture uses the committed `workbench/app`, `pricing` and `catalog` projects. Run its tests from this directory with an explicitly selected authoring compiler/SDK. Validation using a pre-cut compiler is authoring-bootstrap evidence only; it cannot prove the repaired Oven pipeline works. The retained pre-cut compiler successfully baked this source and ran the ten-contract acceptance driver. This proves Incan authoring against that compiler and its explicit SDK, not the repaired Oven pipeline. A separate frontend `check` still refuses its inspection-source receipt; that is recorded independently from the successful native run.

The first filesystem intake accepts a conventional `src/lib.incn` or explicit script. A conventional `src/main.incn` without a declared script remains outside this prototype and refuses explicitly. Absent target/profile/edition remain `None`; the prototype does not select defaults.

The current source contains small adaptations tracked by #1455 (optional-value reassignment), #1461/#1464 (materializing dictionary keys before sorting), and #1462 (preserving constructor evaluation order with sequential bindings). Generated Rust is never edited.
