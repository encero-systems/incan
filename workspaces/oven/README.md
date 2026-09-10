# Local Oven intake

This Incan prototype reads the local `loaf.toml` dependency closure. It retains authored aliases separately from canonical local roots and graph-local node handles. It preserves Rust version/feature requests without selecting a registry version or reconstructing compiler symbol facts.

`intake(Path(...))` returns a local topology or an `IntakeError` carrying manifest, literal field segments, optional TOML source location, category and detail. `LocalIntake` is a request/topology type, not a `BuildPlan`. Selected source digests, target/host units, compiler inputs and admission are not available from these manifests alone.

Manifest bytes and their SHA-256 are source evidence only. They are not an effective action key. Parsed requests exclude comments/formatting and are separate from location and raw evidence. No persisted unit identity or digest is invented here.

The bounded schema accepts project identity/scripts, local path dependencies, registry Rust requests and explicit build target/profile/edition intent. Other tables or local feature activation refuse explicitly. RFC119's unsettled Rust exception grammar is not guessed. A nearby Cargo manifest is never read.

The fixture uses the committed `workbench/app`, `pricing` and `catalog` projects. Run its tests from this directory with an explicitly selected authoring compiler/SDK. Validation using a pre-cut compiler is authoring-bootstrap evidence only; it cannot prove the repaired Oven pipeline works. The retained pre-cut compiler successfully baked this source and ran the ten-contract acceptance driver. This proves Incan authoring against that compiler and its explicit SDK, not the repaired Oven pipeline. A separate frontend `check` still refuses its inspection-source receipt; that is recorded independently from the successful native run.

The first filesystem intake accepts a conventional `src/lib.incn` or explicit script. A conventional `src/main.incn` without a declared script remains outside this prototype and refuses explicitly. Absent target/profile/edition remain `None`; the prototype does not select defaults.

Generated Rust is never edited.

`plan.select_native` is a pure, partial intact-closure selector. It compares explicit runtime/target/toolchain/profile/features facts and checked provider semantic identities, accepting module/facet/direct-link supersets and choosing the least excess. It returns the request and candidate evidence references plus rank. Unknown input classes, malformed sets, incompatible candidates and equal best ranks refuse. A retained bootstrap compiler and genuine generation-7 SDK baked the assertion driver and ran all eight source assertions successfully. This is authoring evidence, not current hot-path compiler acceptance.

This component does not expose roots or publish a plan. The same-session host transport, authenticated candidate/receipt projection, typed owner-to-native-root association, leases and physical admission remain required. Source edition belongs to the root compilation and is deliberately absent from dependency compatibility. No native readiness or current catalog acceptance follows from this comparison alone.

Native feature lists are required on both request and candidate contexts and compared as validated sets; provider modules/facets retain their separate subset relation. `tests/fixtures/plan_missing_features/main.incn` is the validated negative frontend fixture: its check used the normal `src/plan.incn` module and required the missing required field diagnostic, not an import failure. With the normal plan module resolved, this fixture produced exactly the missing `features` field diagnostic. The positive driver called all eight assertion functions and printed `selector: 8 contracts passed` through a locked native run.

The assertions and the generated Rust are never edited by hand. The complete authoring gate passed, including input reconciliation, using the existing bootstrap compatibility baker. This does not restore Cargo authority to the current Oven implementation.

The retained native assertion entrypoint is `src/plan_acceptance.incn`. Its eight calls were executed from the bounded authoring project using the same selector/test modules. The repository copy preserves that driver, with formatting-only separation before `main`; current Oven request/response transport remains unimplemented. The existing intake acceptance driver and project manifest are unchanged.


## Selection JSON adapter (source implementation)

`src/plan_json.incn` strictly decodes `incan.oven.selection/1` and calls the existing typed selector. Every wire field
is required, including context `features`, runtime `unknown_input_keys`, and `unsupported_intent`. Unknown keys and
wrong types are refused at each nested object; array diagnostics use decimal path segments. Duplicate JSON object
keys retain `std.json` parsing behavior. No stricter duplicate-key guarantee is claimed.

The legacy request is `{"schema":"incan.oven.selection/1","request":...}`. Its decoder explicitly constructs the typed legacy recipe mode; it does not accept exact-unit evidence.
The response repeats `schema`, `request_id`, and `request_evidence_id`, then has either `status: "selected"` and the
existing `NativeSelection` under `selection`, or `status: "refused"` and `{kind, fields}` under `error`. Decode
failures use empty binding strings and never contain `selection`; semantic refusals preserve the decoded binding.
The host must authenticate the original request and candidate evidence, not trust an echoed identifier as authority.

`src/plan_json_main.incn` accepts exactly `REQUEST RESPONSE` file paths and performs strict UTF-8 exchange. This is
source implementation only: scoped formatting and the test module frontend check passed with retained compiler
`c7d758d4`; the CLI frontend stopped at missing Rust inspection authority. The first native bake stopped during
dependency preparation before compiling these sources. The original eight adapter assertion functions and actual
file exchange remain unexecuted. It does not implement the governed host process boundary, catalog integration, root exposure or publication.
The future host must enforce regular explicit files, 1 MiB request/response limits, bounded diagnostics and deadline,
response binding, physical artifact validation and leases. The adapter does not itself enforce those host bounds.

### Source-unit batch selection (source implementation)

The same `plan_json_main REQUEST RESPONSE` adapter now recognizes `incan.oven.source-unit-batch/1` alongside the unchanged `incan.oven.selection/1` request. A batch embeds one selection/1 foundation request plus explicit library purpose, source units, checked provider edges, reserved needs and candidate-associated grants. Each unit retains the complete `native/source-unit.json` schema1 definition and all four existing `ProviderIdentity` fields.

Incan first computes whole-batch bindings for each offered foundation, then calls the existing selector once on the fully binding candidates. It returns the chosen foundation and original unit, definition, slot/alias, edge and grant handles atomically. An incomplete narrow candidate does not block a usable candidate. Malformed offered facts refuse before filtering; no fully binding candidate is an ordinary incompatibility refusal. Even empty authored dependency lists require an actual foundation and checked reserved support grants.

This first component supports checked public-provider and private SDK associations for one explicit profile. Registry, Git and path requests remain fully represented but active requests refuse; development requests remain inactive for library purpose, and test purpose is unsupported. It does not change ordinary debug/release behavior or claim a complete dual-profile command connection. The host must authenticate candidates and original handles, keep leases, enforce its 1 MiB/process limits, and validate all bindings before physical effects. No graph traversal, Cargo resolution, filesystem lookup or native readiness is granted by the response.

`test_source_unit.incn` contains synthetic batch contracts; the existing eight adapter assertions remain in `test_plan_json.incn`. The preserved version 1 extension passed semantic check and emission for its four modules and twenty-call driver; those assertions were not executed. Native authoring, same-session host transport, actual catalog/pricing execution and physical attachment remain pending; synthetic facts are not admitted artifacts.


### Explicit native evidence, version 2 (source implementation)

`incan.oven.selection/2` requires the current request's `build_unit_identity` alongside its existing runtime, provider,
context and binding fields. The host must derive that key and recipe from the **same verified current receipt**.
The decoder creates `RequestBuildUnit.VerifiedBuildUnit`; it cannot establish that authentication itself.
Version 1 explicitly creates `LegacyRecipe` and retains its original wire fields and recipe-only behavior.

Every version 2 candidate keeps its original `candidate_id`, `evidence_id`, `plan_identity`, `build_unit_identity`
and full context, and requires one exact `evidence` object:

- `{"kind":"exact_build_unit"}` represents an originally admitted Store unit. Only equal request/unit keys and
  equal target, toolchain, profile and feature sets make it compatible; its rank is zero.
- `{"kind":"runtime_provider_recipe","runtime":...,"providers":[...]}` represents original Loaf recipe facts.
  Existing runtime equality, provider subset compatibility and excess ranking still apply.

A nonmatching exact candidate cannot fall back to a guessed recipe. Equal exact and recipe ranks remain ambiguous.
All candidates are validated before filtering, including losing candidates. Unknown runtime or unsupported request
intent still refuses. Version 1 rejects version 2 evidence rather than inferring a mode from omitted fields.

`incan.oven.source-unit-batch/2` requires a selection/2 foundation; persisted source-unit definitions remain schema1.
One selection still serves the entire batch for one explicit profile. Required capabilities must be included in the
verified request recipe. Exact equality provides logical capability evidence only: original candidate-bound member
grants, owner/provider identities, features, roles, externs and checked edge handles remain independently required.
Recipe candidates retain their own capability checks. No receipt key creates a grant or public output.

The version 2 modules and private 37-call driver passed semantic check and Rust emission with retained compiler
`3d0bc8be` and the unchanged generation 7 SDK (complete guarded window: 22.758 seconds). The driver retains the
original twenty adapter/batch calls, adds the eight existing typed-selector controls and nine new evidence controls.
The assertions have **not executed natively**. Two earlier failures exposed authored syntax/test errors; their receipts
remain preserved. Earlier version 1 proof remains historical. Current host transport,
physical admission, native execution and complete dual-profile operation remain pending. The public file entrypoint
is unchanged and dispatches both explicit versions through the same decoder; host file/process bounds still apply.

The compiler-side Engine descriptor publisher and borrowed reader now bind an explicitly declared module contract to its original completed output, source authority, receipt, native file and actual compiling Incan executable. Legacy outputs without that compiler observation cannot acquire it during reuse; optional compiler checkout provenance remains unavailable. The descriptor adds no executable copy and retains both original owners during admission. This source checkpoint has not been compiled or tested. It does not select or execute the adapter, grant host operations, or complete the compiler/source/ABI handshake; native file exchange itself provides no process sandbox. Non-Unix executable-mode admission remains unavailable under the current store metadata contract.

A private compiler bridge now implements the host side of file exchange in source. It consumes a separately issued, single-use caller permit and the two original admitted owners, checks the declared ABI and exact request binding, and supervises the original executable with bounded files, diagnostic capture, cancellation and process-group cleanup. Its report distinguishes attempted phases, failures and cleanup from successful exact response bytes. Response decoding and selection remain in Incan; returned handles still need the caller's physical authority checks. The installer/caller increment below connects bounded core permission issuance in source. This bridge and its store-backed process tests have not been executed as current native acceptance. File scopes and process groups do not supply an OS sandbox.

An explicit version 2 Engine publisher declaration records support for the current version 2 selection and batch protocols alongside version 1. Existing version 1 descriptors retain their original version and protocol binding; changing only their role is refused. The private bridge does not infer this declaration from an executable path, environment value, source filename or child response, and it does not turn a descriptor or command receipt into permission to launch.

The bridge's phase report remains distinct from a source-language OperationReceipt. The ordinary caller now has source implementation for a version 1 kernel observation record containing actual identities, bounded response/diagnostic bytes, phases and terminal outcome. Complete receipt sealing, denial-before-exchange records and restart replay remain unfinished. The exchange currently contains no permit issuer or construction path. The permit has no JSON decoder or deserialization implementation. Rust child-module access to its parent-owned private fields does not itself prohibit future permit construction code.


## Compiler support source capture (source implementation)

New native source definitions use schema 2 to retain compiler support at the original emission sites: the runtime
version check, and `incan_derive` only where the compiler emits its derives. Required runtime features come from the
existing collected project requirements. Schema 1 remains readable with support evidence explicitly absent.

`incan.oven.source-unit-batch/3` adds compiler-runtime needs and grants alongside checked SDK provider needs.
Matching stays in Incan and uses the existing runtime or exact build-unit evidence. The v3 decoder requires explicit
support slots; older wire versions cannot silently consume a v2 definition while dropping its support obligations.
Runtime grants refer to original named members and source roles. Their digests are host-supplied evidence, not proof
conferred by JSON. Host integration must retain the original owner and resolve returned IDs to its handles; the existing attachment
API rechecks original roles and bytes. The new published-store control exercises that boundary.

The new emitter, real artifact-only publisher, selector and published-store controls are source-only and unexecuted.
The ordinary command grant table and Engine exchange invocation remain to be connected. This slice does not restore
Cargo preparation or claim complete source authority for external build-script, environment or generated-output inputs.


## Trusted core installation and ordinary caller (uncompiled source)

`incan oven install-core-engine --toolchain-root PATH` is an explicit toolchain administration operation. It requires
that toolchain's actual `bin/incan` executable, reads the fixed authored source project at
`share/incan/oven/core-source`, and publishes only its declared `src/plan_json_main.incn` target with the explicit
version 3 Engine role. The release packager stages those tracked sources and invokes this operation after its existing
foundation publication. The core source has no project-provider dependencies; its SDK/inspection prerequisites remain
required, while its empty provider graph avoids calling the Engine during its own publication. The existing release
foundation publisher is not replaced by this increment, and full Cargo-free archive production remains unverified.

The installation index records the installing compiler binary separately from the original compiling binary. It names
the original Engine and ProjectOutput identities and the original module source/receipt binding. Installation copies
those exact admitted owners through the existing Store publisher. Replacement holds the previous pair and each
newly copied destination owner under leases, serializes index updates, and commits only after destination admission. Normal lookup uses the canonical running toolchain, with no executable environment override or project
fallback. The archive/installer is the trust boundary; directory names and digests alone do not confer host permission.

Ordinary project and library source rematerialization now have a caller connection for an original Store selection.
They retain the existing producer's semantic map and receipt, borrow the selected original owner once, and invoke the
Incan batch selector. Warm same-unit Store reuse still lacks an original private-SDK recipe witness when its receipt
identity differs from the command receipt; this is an open carrier defect, not accepted warm reuse. The host explicitly permits this bounded core operation, then validates returned original IDs
before native materialization. SDK-only and empty project-provider paths do not require an installed Engine. Already
packaged provider closures keep their existing conflict checks. Composed extension/bare-Loaf batch owner ingress,
source-free inspection transport, and active registry/Git/path source slots still require their existing checked owner
connections. The current workbench application requires registry Rust `regex`; path/Git coverage remains part of the broader source contract.

This command uses a 30-second exchange deadline, 1 MiB request/response ceilings and 64 KiB diagnostic ceilings.
Command-wide cancellation integration remains unfinished. Every attempted exchange is retained as a versioned kernel
observation under the generated project's `.incan/engine-exchange` mutable-output directory, even if the child or
returned selection fails. The record preserves exact response bytes and diagnostic-prefix meanings; it is neither an
execution grant nor an RFC104 OperationReceipt. File scopes and process groups provide no OS sandbox.

The private tests exercise the real descriptor/Store installer, original-owner relocation and substitution refusals,
explicit issuer denials, exact bounded process observations, the empty-provider command path, and target filtering.
Their executable fixtures are process-test data, not native Incan proof. This increment has only source/static checks;
compilation, real core publication and the plain-provider/facade native acceptance remain pending. #991 stays open.

New native Store publications retain the publisher's full receipt as bounded, versioned metadata. Ordinary provider selection borrows that original recipe under the selected owner lease, including when the current source receipt differs but the reusable build unit matches. Legacy entries without the metadata keep their original bytes and native admission; the provider recipe path reports the missing witness instead of substituting the current receipt. This source connection has focused controls but has not yet been compiled or executed.
