# Replacement compatibility inventory

!!! warning "Generated control-plane reference"

    Do not edit this page by hand. Regenerate it from the checked public-capability baseline and compiler-boundary registrations.

This is a validated migration control plane, not a permanent second language-feature catalogue and not a parity claim. Durable feature and private-mechanism records are registered beside the compiler boundary that owns them; the collector joins and validates them here. The explicitly marked migration bootstrap exists only while unlanded work lacks such a boundary. A feature row turns green only after direct execution and an independent, receipt-bound source-observable comparison for its full contract. A matched corpus case remains scoped evidence and cannot promote an incomplete feature. Generated Rust, Body IR representation, and legacy compilation are separate facts.

## Release-pinned public baseline

- Release: `v0.5.0` at `f6c17e0f8948c032f8b308236d57d9dee6ab1e9f`
- Baseline role: `MigrationCompatibilityTarget`
- Checked source blob: `42f718a9c35f816a68bb3ff13578eaf6725e3d0b`
- Capability descriptors: `67`
- Retirement: Retire this active baseline when the v0.5 replacement migration closes; retain the source only as an explicitly historical regression fixture if a later migration needs it.

The `v0.5.0` source is a frozen migration baseline, not the beginning of a version archive. It is retained only under the stated retirement condition.

## Collector assembly and bootstrap retirement

| Contributor | Lifecycle | Features | Private requirements | Location | Retirement condition |
|---|---|---|---|---|---|
| `backend.replacement.bounded-scalar-control` | LocalImplementation | 3 | 3 | `src/backend/replacement/mod.rs::fn replacement_compatibility_direct_execution_contribution` | - |
| `frontend.body-ir.callable-values` | LocalImplementation | 2 | 2 | `src/frontend/body_ir.rs::fn replacement_compatibility_body_ir_contribution` | - |
| `replacement-compatibility.migration-bootstrap` | MigrationBootstrap | 22 | 16 | `src/replacement_compatibility.rs::fn migration_bootstrap_compatibility_contribution` | Retire this contributor when every remaining feature and requirement has moved to the module that implements its coherent mechanism; then retain the v0.5 source only as an explicitly historical regression fixture if a later migration needs it. |

## Compatibility features

| Feature | Source contract | Legacy run | Body IR | Direct replacement | #987 | Feature comparison | Scoped case comparisons | Disposition | Owner |
|---|---|---|---|---|---|---|---|---|---|
| `async.tasks` | Checked | Unknown | Represented | Executable | registered replacement-body-v0-018, replacement-body-v0-019 | NonGreenShadowUnavailable | - | Preserved | #1155 |
| `call.named-and-variadic` | Checked | Unknown | Partial | BlockedByRequirements | planned parity-987-plan-call.named-and-variadic | NonGreenShadowUnavailable | - | Planned | #1152 |
| `call.partial-binding` | Checked | Unknown | Partial | BlockedByRequirements | planned parity-987-plan-call.partial-binding | NonGreenShadowUnavailable | - | Planned | #1152 |
| `call.stored-callables` | Checked | Unknown | Partial | BlockedByRequirements | planned parity-987-plan-call.stored-callables | NonGreenShadowUnavailable | - | Planned | #1152 |
| `decorators.dsl-surfaces` | Checked | Unknown | Partial | BlockedByRequirements | planned parity-987-plan-decorators.dsl-surfaces | NonGreenShadowUnavailable | - | Planned | #555 |
| `diagnostics.stable` | Checked | Unknown | Partial | BlockedByRequirements | planned parity-987-plan-diagnostics.stable | NonGreenShadowUnavailable | - | Planned | #655 |
| `error.result-and-try` | Checked | Unknown | Partial | BlockedByRequirements | planned parity-987-plan-error.result-and-try | NonGreenShadowUnavailable | - | Planned | #1154 |
| `generator.expressions` | Checked | Unknown | Partial | BlockedByRequirements | planned parity-987-plan-generator.expressions | NonGreenShadowUnavailable | - | Planned | #1152 |
| `generator.functions` | Checked | Unknown | Partial | BlockedByRequirements | planned parity-987-plan-generator.functions | NonGreenShadowUnavailable | - | Planned | #1152 |
| `interop.rust-and-c` | Checked | Unknown | Partial | BlockedByRequirements | planned parity-987-plan-interop.rust-and-c | NonGreenShadowUnavailable | - | Planned | #989 |
| `iteration.protocol-and-adapters` | Checked | Unknown | Partial | BlockedByRequirements | planned parity-987-plan-iteration.protocol-and-adapters | NonGreenShadowUnavailable | - | Planned | #1152 |
| `iteration.user-and-fallible` | Checked | Unknown | Partial | BlockedByRequirements | planned parity-987-plan-iteration.user-and-fallible | NonGreenShadowUnavailable | - | Planned | #1101 |
| `language.aggregates-and-projections` | Checked | Unknown | Partial | BlockedByRequirements | planned parity-987-plan-language.aggregates-and-projections | NonGreenShadowUnavailable | - | Planned | #1154 |
| `language.control-flow` | Checked | Unknown | Represented | Executable | registered replacement-body-v0-004, replacement-body-v0-005, replacement-body-v0-006 | NonGreenShadowUnavailable | - | Preserved | - |
| `language.control-flow-complete` | Checked | Unknown | Partial | BlockedByRequirements | planned parity-987-plan-language.control-flow-complete | NonGreenShadowUnavailable | - | Planned | #1154 |
| `language.match-and-patterns` | Checked | Unknown | Partial | BlockedByRequirements | planned parity-987-plan-language.match-and-patterns | NonGreenShadowUnavailable | - | Planned | #1154 |
| `language.numeric-and-scalar` | Checked | Unknown | Represented | Executable | registered replacement-body-v0-001, replacement-body-v0-002, replacement-body-v0-003, replacement-body-v0-005 | NonGreenShadowUnavailable | replacement-body-v0-001: ComparedMatch | Preserved | - |
| `language.numeric-complete` | Checked | Unknown | Partial | BlockedByRequirements | planned parity-987-plan-language.numeric-complete | NonGreenShadowUnavailable | - | Planned | #1154 |
| `language.strings-and-format` | Checked | Unknown | Partial | BlockedByRequirements | planned parity-987-plan-language.strings-and-format | NonGreenShadowUnavailable | - | Planned | #1101 |
| `module.identity-and-aliases` | Checked | Unknown | Partial | BlockedByRequirements | planned parity-987-plan-module.identity-and-aliases | NonGreenShadowUnavailable | - | Planned | #1042 |
| `nominal.models-unions-enums` | Checked | Unknown | Partial | BlockedByRequirements | planned parity-987-plan-nominal.models-unions-enums | NonGreenShadowUnavailable | - | Planned | #1154 |
| `package.public-boundaries` | Checked | Unknown | Partial | BlockedByRequirements | planned parity-987-plan-package.public-boundaries | NonGreenShadowUnavailable | - | Planned | #989 |
| `runtime.std-data-services` | Checked | Unknown | Partial | BlockedByRequirements | planned parity-987-plan-runtime.std-data-services | NonGreenShadowUnavailable | - | Planned | #1156 |
| `runtime.std-hosted-services` | Checked | Unknown | Partial | BlockedByRequirements | planned parity-987-plan-runtime.std-hosted-services | NonGreenShadowUnavailable | - | Planned | #1156 |
| `runtime.std-observability` | Checked | Unknown | Partial | BlockedByRequirements | planned parity-987-plan-runtime.std-observability | NonGreenShadowUnavailable | - | Planned | #1156 |
| `testing-and-tooling` | Checked | Unknown | Partial | BlockedByRequirements | planned parity-987-plan-testing-and-tooling | NonGreenShadowUnavailable | - | Planned | #1034 |
| `types.traits-generics-reflection` | Checked | Unknown | Partial | BlockedByRequirements | planned parity-987-plan-types.traits-generics-reflection | NonGreenShadowUnavailable | - | Planned | #1033 |

## Public capability crosswalk

| Capability | Since | RFC | Landing provenance | Compatibility features |
|---|---:|---|---|---|
| `NamespacedStdlib` | 0.2 | RFC 022 | ReleaseRegistryDeclared | `module.identity-and-aliases` |
| `RustInteropBoundary` | 0.2 | RFC 041 | ReleaseRegistryDeclared | `interop.rust-and-c` |
| `CheckedCBindingFoundation` | 0.5 | RFC 116 | ReleaseRegistryDeclared | `interop.rust-and-c` |
| `OvenInteropRequirements` | 0.5 | RFC 116 | ReleaseRegistryDeclared | `package.public-boundaries` |
| `IncanLibraries` | 0.2 | RFC 031 | ReleaseRegistryDeclared | `package.public-boundaries` |
| `StaticStorage` | 0.2 | RFC 052 | ReleaseRegistryDeclared | `nominal.models-unions-enums` |
| `FirstClassFunctions` | 0.2 | RFC 035 | ReleaseRegistryDeclared | `call.named-and-variadic`, `call.stored-callables` |
| `CallSiteGenerics` | 0.2 | RFC 054 | ReleaseRegistryDeclared | `call.named-and-variadic` |
| `AbstractTraits` | 0.2 | RFC 042 | ReleaseRegistryDeclared | `types.traits-generics-reflection` |
| `SourceDefinedDerivesTraits` | 0.2 | RFC 024 | ReleaseRegistryDeclared | `types.traits-generics-reflection` |
| `ModelFieldMetadata` | 0.2 | RFC 021 | ReleaseRegistryDeclared | `nominal.models-unions-enums` |
| `TypeTokensReflection` | 0.3 | RFC 107 | HistoricalDiscrepancyUnresolved; owner #1153 | `types.traits-generics-reflection` |
| `ValueEnums` | 0.3 | RFC 032 | HistoricalDiscrepancyUnresolved; owner #1153 | `nominal.models-unions-enums` |
| `UnionTypes` | 0.3 | RFC 029 | ReleaseRegistryDeclared | `nominal.models-unions-enums` |
| `ValidatedNewtypes` | 0.3 | RFC 017 | ReleaseRegistryDeclared | `nominal.models-unions-enums` |
| `NumericTypeSystem` | 0.3 | RFC 009 | ReleaseRegistryDeclared | `language.numeric-and-scalar`, `language.numeric-complete`, `language.strings-and-format` |
| `LoopExpressions` | 0.3 | RFC 016 | ReleaseRegistryDeclared | `language.control-flow`, `language.control-flow-complete` |
| `IfWhileLet` | 0.3 | RFC 049 | ReleaseRegistryDeclared | `language.control-flow`, `language.control-flow-complete` |
| `PatternAlternation` | 0.3 | RFC 071 | ReleaseRegistryDeclared | `language.match-and-patterns` |
| `EnumMethodsTraits` | 0.3 | RFC 050 | ReleaseRegistryDeclared | `nominal.models-unions-enums` |
| `ComputedProperties` | 0.3 | RFC 046 | ReleaseRegistryDeclared | `nominal.models-unions-enums` |
| `SymbolAliases` | 0.3 | RFC 083 | ReleaseRegistryDeclared | `module.identity-and-aliases` |
| `CallablePresets` | 0.3 | RFC 084 | ReleaseRegistryDeclared | `call.partial-binding` |
| `VariadicAndSpreadCalls` | 0.3 | RFC 038 | ReleaseRegistryDeclared | `call.named-and-variadic` |
| `UserDefinedDecorators` | 0.3 | RFC 036 | ReleaseRegistryDeclared | `decorators.dsl-surfaces` |
| `Generators` | 0.3 | RFC 006 | ReleaseRegistryDeclared | `generator.expressions`, `generator.functions` |
| `IteratorAdapters` | 0.3 | RFC 088 | ReleaseRegistryDeclared | `iteration.protocol-and-adapters` |
| `FallibleIteration` | 0.5 | RFC 115 | ReleaseRegistryDeclared | `iteration.user-and-fallible` |
| `ResultCombinators` | 0.3 | RFC 070 | ReleaseRegistryDeclared | `error.result-and-try` |
| `ProtocolHooks` | 0.3 | RFC 068 | ReleaseRegistryDeclared | `types.traits-generics-reflection` |
| `RustTraitAdoption` | 0.3 | RFC 043 | ReleaseRegistryDeclared | `types.traits-generics-reflection` |
| `RustAllow` | 0.3 | RFC 057 | ReleaseRegistryDeclared | `interop.rust-and-c` |
| `ScopedDslSurfaces` | 0.3 | RFC 040 | ReleaseRegistryDeclared | `decorators.dsl-surfaces` |
| `StdWeb` | 0.2 | RFC 023 | HistoricalDiscrepancyUnresolved; owner #1153 | `runtime.std-hosted-services` |
| `StdMath` | 0.2 | RFC 022 | ReleaseRegistryDeclared | `runtime.std-data-services` |
| `StdCollections` | 0.3 | RFC 030 | ReleaseRegistryDeclared | `language.aggregates-and-projections` |
| `StdGraph` | 0.3 | RFC 047 | ReleaseRegistryDeclared | `runtime.std-data-services` |
| `StdFs` | 0.3 | RFC 055 | ReleaseRegistryDeclared | `runtime.std-hosted-services` |
| `StdIo` | 0.3 | RFC 056 | ReleaseRegistryDeclared | `runtime.std-hosted-services` |
| `StdRegex` | 0.3 | RFC 059 | ReleaseRegistryDeclared | `runtime.std-data-services` |
| `StdUuid` | 0.3 | RFC 060 | ReleaseRegistryDeclared | `runtime.std-data-services` |
| `StdCompression` | 0.3 | RFC 061 | ReleaseRegistryDeclared | `runtime.std-data-services` |
| `StdEncoding` | 0.3 | RFC 064 | ReleaseRegistryDeclared | `runtime.std-data-services` |
| `StdHash` | 0.3 | RFC 065 | ReleaseRegistryDeclared | `runtime.std-data-services` |
| `StdEnviron` | 0.5 | RFC 089 | ReleaseRegistryDeclared | `runtime.std-hosted-services` |
| `StdJson` | 0.3 | RFC 051 | ReleaseRegistryDeclared | `runtime.std-data-services` |
| `StdTempfile` | 0.3 | RFC 010 | ReleaseRegistryDeclared | `runtime.std-hosted-services` |
| `StdDatetime` | 0.3 | RFC 058 | ReleaseRegistryDeclared | `runtime.std-data-services` |
| `StdTelemetryCore` | 0.3 | RFC 072 | ReleaseRegistryDeclared | `runtime.std-observability` |
| `StdLogging` | 0.3 | RFC 072 | ReleaseRegistryDeclared | `runtime.std-observability` |
| `StdChecksum` | 0.5 | RFC 065 | ReleaseRegistryDeclared | `runtime.std-data-services` |
| `TestingAssertions` | 0.3 | RFC 018 | ReleaseRegistryDeclared | `testing-and-tooling` |
| `TestRunner` | 0.3 | RFC 019 | ReleaseRegistryDeclared | `testing-and-tooling` |
| `AsyncAwait` | 0.2 | RFC 023 | HistoricalDiscrepancyUnresolved; owner #1153 | `async.tasks` |
| `AsyncRace` | 0.3 | RFC 039 | ReleaseRegistryDeclared | `async.tasks` |
| `ProjectLifecycle` | 0.3 | RFC 015 | ReleaseRegistryDeclared | `testing-and-tooling` |
| `WorkspaceMultiPackageProjects` | 0.5 | RFC 077 | ReleaseRegistryDeclared | `package.public-boundaries` |
| `ToolchainInstallerManifest` | 0.4 | RFC 015 | ReleaseRegistryDeclared | `testing-and-tooling` |
| `ZeroCloneStarterFlow` | 0.4 | RFC 015 | ReleaseRegistryDeclared | `testing-and-tooling` |
| `StableDiagnostics` | 0.4 | RFC 015 | ReleaseRegistryDeclared | `diagnostics.stable` |
| `BuildReportsAndRustInspection` | 0.4 | RFC 015 | ReleaseRegistryDeclared | `testing-and-tooling` |
| `BuildTestOvenObservability` | 0.5 | RFC 015 | ReleaseRegistryDeclared | `testing-and-tooling` |
| `CodegraphInspection` | 0.4 | RFC 106 | HistoricalDiscrepancyUnresolved; owner #1153 | `testing-and-tooling` |
| `CheckedApiMetadata` | 0.3 | RFC 048 | ReleaseRegistryDeclared | `package.public-boundaries` |
| `CompiledProvidersSdkComponentsPackageFeatures` | 0.5 | RFC 114 | ReleaseRegistryDeclared | `package.public-boundaries` |
| `FormatterContract` | 0.3 | RFC 053 | ReleaseRegistryDeclared | `testing-and-tooling` |
| `StdRegistry` | 0.5 | RFC 113 | ReleaseRegistryDeclared | `runtime.std-observability` |

## Private implementation requirements

| Requirement | Owning boundary | Enabled features | Verification anchor |
|---|---|---|---|
| `async.runtime` | source-local Body IR plus replacement task runtime | `async.tasks` | replacement-body-v0-018 and replacement-body-v0-019 corpus probes |
| `call.argument-binder` | typechecker partial projection and replacement call runtime | `call.named-and-variadic`, `call.partial-binding` | partial/default typechecker and Body-IR tests |
| `call.frames` | replacement runtime call dispatcher | `call.named-and-variadic`, `call.stored-callables`, `generator.functions`, `iteration.protocol-and-adapters` | stored-callable Body-IR tests and #1152 execution probes |
| `captures.lexical-environments` | Body IR closure lowering and replacement runtime | `call.partial-binding`, `call.stored-callables` | closure/partial capture timing regressions |
| `control.normalized-flow` | Body IR lowering and replacement evaluator | `language.control-flow`, `language.control-flow-complete` | replacement-body-v0 corpus |
| `diagnostics.source-authority` | frontend diagnostics and backend selection receipts | `diagnostics.stable`, `error.result-and-try` | diagnostic fixtures and #987 classifier |
| `error.result-routing` | Body IR try lowering and replacement runtime | `error.result-and-try`, `iteration.user-and-fallible` | try/result Body-IR and runtime probes |
| `iteration.protocol-dispatch` | typechecker protocol facts, Body IR, replacement runtime | `generator.expressions`, `iteration.protocol-and-adapters`, `iteration.user-and-fallible` | iteration protocol Body-IR snapshots |
| `modules.canonical-identity` | module resolver and semantic facts | `module.identity-and-aliases` | canonical identity and package-boundary tests |
| `nominal.value-model` | Body IR value lowering and replacement runtime | `language.match-and-patterns`, `nominal.models-unions-enums` | model/union/value-enum Body-IR probes |
| `packages.public-contract` | package/ABI boundary | `interop.rust-and-c`, `package.public-boundaries` | #989 boundary corpus |
| `patterns.dispatch` | typechecker match facts and Body IR | `language.match-and-patterns` | match diagnostics and Body-IR tests |
| `providers.runtime-services` | stdlib/provider plan and replacement runtime | `runtime.std-data-services`, `runtime.std-hosted-services`, `runtime.std-observability` | stdlib/provider and authority tests |
| `receipts.comparison` | backend selection, #1146, and #987 | `async.tasks`, `diagnostics.stable`, `runtime.std-hosted-services`, `testing-and-tooling` | backend selection and parity corpus tests |
| `runtime.aggregate-store` | Body IR aggregates/places and replacement runtime | `language.aggregates-and-projections`, `language.control-flow-complete`, `language.numeric-complete`, `runtime.std-data-services` | aggregate and assignment Body-IR tests |
| `runtime.scalar-values` | Body IR operands/rvalues and replacement evaluator | `language.numeric-and-scalar`, `language.numeric-complete`, `language.strings-and-format` | replacement-body-v0 scalar corpus |
| `surface.decorator-dispatch` | semantic registry and surface semantics packs | `decorators.dsl-surfaces` | surface semantics and vocab tests |
| `suspension.continuations` | Body IR generator model and replacement runtime | `generator.expressions`, `generator.functions` | generator laziness and resume probes |
| `testing.tooling-control-plane` | CLI, Oven, and test-runner boundaries | `testing-and-tooling` | CLI and Oven integration tests |
| `types.resolved-dispatch` | typechecker facts and Body IR call model | `call.named-and-variadic`, `types.traits-generics-reflection` | typechecker resolution and type-token tests |
| `unsafe.interop-boundary` | interop metadata and package boundary | `interop.rust-and-c` | Rust/C interop and package consumer tests |

## Remaining-work issue map

Every planned feature below has a currently open mechanism owner. #1146 is completed comparison infrastructure: it supplies reusable provenance, never ownership of missing comparison evidence. Scheduled evidence belongs to its feature/runtime owner; direct profiles without one carry explicit unscheduled evidence debt. `replacement-body-v0-001` has one paired match, while all incomplete features and uncovered cases remain non-green.

- #555: `decorators.dsl-surfaces`
- #655: `diagnostics.stable`
- #989: `interop.rust-and-c`, `package.public-boundaries`
- #1033: `types.traits-generics-reflection`
- #1034: `testing-and-tooling`
- #1042: `module.identity-and-aliases`
- #1101: `iteration.user-and-fallible`, `language.strings-and-format`
- #1152: `call.named-and-variadic`, `call.partial-binding`, `call.stored-callables`, `generator.expressions`, `generator.functions`, `iteration.protocol-and-adapters`
- #1154: `error.result-and-try`, `language.aggregates-and-projections`, `language.control-flow-complete`, `language.match-and-patterns`, `language.numeric-complete`, `nominal.models-unions-enums`
- #1155: `async.tasks`
- #1156: `runtime.std-data-services`, `runtime.std-hosted-services`, `runtime.std-observability`

## Probe and ownership obligations

### `async.tasks`

One exact source-local `std.async` activation executes same-module async calls, direct await, and source-order ready-tie races through receipt-bound task frames.

- `probe:async.tasks:bounded-direct-profile` — positive AcceptedBehavior at Observed `src/replacement_compatibility/migration_baselines/v0.5.0/capabilities.incn::AsyncAwait`; negative IntentionalRefusal at Observed `src/frontend/typechecker/check_expr/control_flow.rs::fn check_await`
  - Positive contract: One exact source-local `std.async` activation executes same-module async calls, direct await, and source-order ready-tie races through receipt-bound task frames.
  - Negative contract: Inputs outside the bounded direct profile refuse visibly with their source span.
- Source/AST: Observed `src/replacement_compatibility/migration_baselines/v0.5.0/capabilities.incn::AsyncAwait`
- Typechecker: Observed `src/frontend/typechecker/check_expr/control_flow.rs::fn check_await`
- Body IR: Observed `src/frontend/body_ir.rs::fn lower_race_for`
- Replacement executor: Observed `src/backend/replacement/mod.rs::fn execute_race`
- Aggregate comparison: unavailable; completed comparison infrastructure #1146 at Observed `tests/support/parity_corpus.rs::NonGreenShadowUnavailable`; outstanding evidence owner #1155: #1155 owns exact paired source-observable evidence through #1146's completed route; direct task execution alone remains non-green.
- Blocker/migration: #1155 owns the bounded source-local task profile and its remaining paired source-observable comparison evidence.

### `call.named-and-variadic`

Named calls preserve resolved targets, generic arguments, positional/named binding, and spread diagnostics.

- `probe:call.named-and-variadic:binding-and-refusal` — positive AcceptedBehavior at Observed `src/replacement_compatibility/migration_baselines/v0.5.0/capabilities.incn::CallSiteGenerics`; negative IntentionalRefusal at Observed `src/frontend/typechecker/check_expr/calls.rs::fn check_call`
  - Positive contract: Named calls preserve resolved targets, generic arguments, positional/named binding, and spread diagnostics.
  - Negative contract: Reject unsupported variants with an intentional source-owned diagnostic and no silent legacy fallback.
- Source/AST: Observed `src/replacement_compatibility/migration_baselines/v0.5.0/capabilities.incn::CallSiteGenerics`
- Typechecker: Observed `src/frontend/typechecker/check_expr/calls.rs::fn check_call`
- Body IR: Planned `src/frontend/body_ir.rs::fn lower_call`; owner #1152
- Replacement executor: Planned `src/backend/replacement/mod.rs::fn execute_call`; owner #1152
- Aggregate comparison: unavailable; completed comparison infrastructure #1146 at Observed `tests/support/parity_corpus.rs::NonGreenShadowUnavailable`; outstanding evidence owner #1152: The feature/runtime owner must add receipt-bound comparison evidence after its direct profile is materialized.
- Blocker/migration: Call-frame and binder execution beyond the bounded scalar profile is owned by the callable runtime slice.

### `call.partial-binding`

Partial presets capture at construction, remain overrideable defaults, and preserve named/positional binding rules.

- `probe:call.partial-binding:binding-and-refusal` — positive AcceptedBehavior at Observed `src/replacement_compatibility/migration_baselines/v0.5.0/capabilities.incn::CallablePresets`; negative IntentionalRefusal at Observed `src/frontend/typechecker/check_expr/calls.rs::fn check_call`
  - Positive contract: Partial presets capture at construction, remain overrideable defaults, and preserve named/positional binding rules.
  - Negative contract: Reject unsupported variants with an intentional source-owned diagnostic and no silent legacy fallback.
- Source/AST: Observed `src/replacement_compatibility/migration_baselines/v0.5.0/capabilities.incn::CallablePresets`
- Typechecker: Observed `src/frontend/typechecker/check_expr/calls.rs::fn check_call`
- Body IR: Planned `src/frontend/body_ir.rs::fn lower_call`; owner #1152
- Replacement executor: Planned `src/backend/replacement/mod.rs::fn execute_call`; owner #1152
- Aggregate comparison: unavailable; completed comparison infrastructure #1146 at Observed `tests/support/parity_corpus.rs::NonGreenShadowUnavailable`; outstanding evidence owner #1152: The feature/runtime owner must add receipt-bound comparison evidence after its direct profile is materialized.
- Blocker/migration: Body IR carries the source contract; direct local callable targets remain visibly refused until the callable runtime slice executes them.

### `call.stored-callables`

Stored closures and partials retain lexical capture timing, ownership, and isolated local call frames.

- `probe:call.stored-callables:binding-and-refusal` — positive AcceptedBehavior at Observed `src/replacement_compatibility/migration_baselines/v0.5.0/capabilities.incn::FirstClassFunctions`; negative IntentionalRefusal at Observed `src/frontend/typechecker/check_expr/calls.rs::fn check_call`
  - Positive contract: Stored closures and partials retain lexical capture timing, ownership, and isolated local call frames.
  - Negative contract: Reject unsupported variants with an intentional source-owned diagnostic and no silent legacy fallback.
- Source/AST: Observed `src/replacement_compatibility/migration_baselines/v0.5.0/capabilities.incn::FirstClassFunctions`
- Typechecker: Observed `src/frontend/typechecker/check_expr/calls.rs::fn check_call`
- Body IR: Planned `src/frontend/body_ir.rs::fn lower_call`; owner #1152
- Replacement executor: Planned `src/backend/replacement/mod.rs::fn execute_call`; owner #1152
- Aggregate comparison: unavailable; completed comparison infrastructure #1146 at Observed `tests/support/parity_corpus.rs::NonGreenShadowUnavailable`; outstanding evidence owner #1152: The feature/runtime owner must add receipt-bound comparison evidence after its direct profile is materialized.
- Blocker/migration: Direct execution deliberately refuses local callable targets; this is the coherent callable-frame profile.

### `decorators.dsl-surfaces`

Decorators and scoped DSL surfaces preserve activation, dispatch, and source-owned diagnostics.

- `probe:decorators.dsl-surfaces:binding-and-refusal` — positive AcceptedBehavior at Observed `src/replacement_compatibility/migration_baselines/v0.5.0/capabilities.incn::ScopedDslSurfaces`; negative IntentionalRefusal at Observed `src/frontend/typechecker/collect/decorators.rs::fn validate_decorators_allowing_user_defined`
  - Positive contract: Decorators and scoped DSL surfaces preserve activation, dispatch, and source-owned diagnostics.
  - Negative contract: Reject unsupported variants with an intentional source-owned diagnostic and no silent legacy fallback.
- Source/AST: Observed `src/replacement_compatibility/migration_baselines/v0.5.0/capabilities.incn::ScopedDslSurfaces`
- Typechecker: Observed `src/frontend/typechecker/collect/decorators.rs::fn validate_decorators_allowing_user_defined`
- Body IR: Planned `src/frontend/body_ir.rs::fn lower_function_body`; owner #555
- Replacement executor: Planned `src/backend/replacement/mod.rs::fn execute_call`; owner #555
- Aggregate comparison: unavailable; completed comparison infrastructure #1146 at Observed `tests/support/parity_corpus.rs::NonGreenShadowUnavailable`; outstanding evidence owner #555: The feature/runtime owner must add receipt-bound comparison evidence after its direct profile is materialized.
- Blocker/migration: Surface packs and decorators require a source-to-runtime dispatch boundary before direct execution can classify them.

### `diagnostics.stable`

Source diagnostics retain intentional acceptance/refusal boundaries, spans, and machine-readable identity.

- `probe:diagnostics.stable:binding-and-refusal` — positive AcceptedBehavior at Observed `src/replacement_compatibility/migration_baselines/v0.5.0/capabilities.incn::StableDiagnostics`; negative IntentionalRefusal at Observed `src/frontend/typechecker/check_stmt.rs::fn check_statement`
  - Positive contract: Source diagnostics retain intentional acceptance/refusal boundaries, spans, and machine-readable identity.
  - Negative contract: Reject unsupported variants with an intentional source-owned diagnostic and no silent legacy fallback.
- Source/AST: Observed `src/replacement_compatibility/migration_baselines/v0.5.0/capabilities.incn::StableDiagnostics`
- Typechecker: Observed `src/frontend/typechecker/check_stmt.rs::fn check_statement`
- Body IR: Planned `src/frontend/body_ir.rs::fn lower_function_body`; owner #655
- Replacement executor: Planned `src/backend/replacement/mod.rs::fn execute_free_function`; owner #655
- Aggregate comparison: unavailable; completed comparison infrastructure #1146 at Observed `tests/support/parity_corpus.rs::NonGreenShadowUnavailable`; outstanding evidence owner #655: The feature/runtime owner must add receipt-bound comparison evidence after its direct profile is materialized.
- Blocker/migration: The compatibility report and corpus need receipt-bound diagnostic evidence; generated Rust diagnostics are not a substitute.

### `error.result-and-try`

Result combinators and explicit propagation retain success, error, ordering, and diagnostic behavior.

- `probe:error.result-and-try:binding-and-refusal` — positive AcceptedBehavior at Observed `src/replacement_compatibility/migration_baselines/v0.5.0/capabilities.incn::ResultCombinators`; negative IntentionalRefusal at Observed `src/frontend/typechecker/check_expr/control_flow.rs::fn check_try`
  - Positive contract: Result combinators and explicit propagation retain success, error, ordering, and diagnostic behavior.
  - Negative contract: Reject unsupported variants with an intentional source-owned diagnostic and no silent legacy fallback.
- Source/AST: Observed `src/replacement_compatibility/migration_baselines/v0.5.0/capabilities.incn::ResultCombinators`
- Typechecker: Observed `src/frontend/typechecker/check_expr/control_flow.rs::fn check_try`
- Body IR: Planned `src/frontend/body_ir.rs::fn lower_try`; owner #1154
- Replacement executor: Planned `src/backend/replacement/mod.rs::fn execute_call`; owner #1154
- Aggregate comparison: unavailable; completed comparison infrastructure #1146 at Observed `tests/support/parity_corpus.rs::NonGreenShadowUnavailable`; outstanding evidence owner #1154: The feature/runtime owner must add receipt-bound comparison evidence after its direct profile is materialized.
- Blocker/migration: Depends on #1101 for complete Body IR vocabulary; #1154 owns Result/error value routing.

### `generator.expressions`

Generator expressions preserve construction-versus-consumption timing and lazy collection in the admitted profile.

- `probe:generator.expressions:binding-and-refusal` — positive AcceptedBehavior at Observed `src/replacement_compatibility/migration_baselines/v0.5.0/capabilities.incn::Generators`; negative IntentionalRefusal at Observed `src/frontend/typechecker/check_expr/calls.rs::fn check_call`
  - Positive contract: Generator expressions preserve construction-versus-consumption timing and lazy collection in the admitted profile.
  - Negative contract: Reject unsupported variants with an intentional source-owned diagnostic and no silent legacy fallback.
- Source/AST: Observed `src/replacement_compatibility/migration_baselines/v0.5.0/capabilities.incn::Generators`
- Typechecker: Observed `src/frontend/typechecker/check_expr/calls.rs::fn check_call`
- Body IR: Planned `src/frontend/body_ir.rs::fn lower_generator_expr`; owner #1152
- Replacement executor: Planned `src/backend/replacement/mod.rs::ReplacementGenerator`; owner #1152
- Aggregate comparison: unavailable; completed comparison infrastructure #1146 at Observed `tests/support/parity_corpus.rs::NonGreenShadowUnavailable`; outstanding evidence owner #1152: The feature/runtime owner must add receipt-bound comparison evidence after its direct profile is materialized.
- Blocker/migration: The bounded generator-expression collect path exists, but broader consumption and comparison remain non-green.

### `generator.functions`

Generator functions suspend and resume without replaying prior effects or losing local state.

- `probe:generator.functions:binding-and-refusal` — positive AcceptedBehavior at Observed `src/replacement_compatibility/migration_baselines/v0.5.0/capabilities.incn::Generators`; negative IntentionalRefusal at Observed `src/frontend/typechecker/check_expr/calls.rs::fn check_call`
  - Positive contract: Generator functions suspend and resume without replaying prior effects or losing local state.
  - Negative contract: Reject unsupported variants with an intentional source-owned diagnostic and no silent legacy fallback.
- Source/AST: Observed `src/replacement_compatibility/migration_baselines/v0.5.0/capabilities.incn::Generators`
- Typechecker: Observed `src/frontend/typechecker/check_expr/calls.rs::fn check_call`
- Body IR: Planned `src/frontend/body_ir.rs::fn lower_generator_expr`; owner #1152
- Replacement executor: Planned `src/backend/replacement/mod.rs::ReplacementGenerator`; owner #1152
- Aggregate comparison: unavailable; completed comparison infrastructure #1146 at Observed `tests/support/parity_corpus.rs::NonGreenShadowUnavailable`; outstanding evidence owner #1152: The feature/runtime owner must add receipt-bound comparison evidence after its direct profile is materialized.
- Blocker/migration: Generator-function frames and resumption remain explicit replacement refusals.

### `interop.rust-and-c`

Rust and C boundaries preserve checked signatures, coercions, explicit unsafe acknowledgements, and source-map diagnostics.

- `probe:interop.rust-and-c:binding-and-refusal` — positive AcceptedBehavior at Observed `src/replacement_compatibility/migration_baselines/v0.5.0/capabilities.incn::CheckedCBindingFoundation`; negative IntentionalRefusal at Observed `src/frontend/typechecker/check_expr/calls/rust_boundary.rs::fn validate_rust_boundary_value`
  - Positive contract: Rust and C boundaries preserve checked signatures, coercions, explicit unsafe acknowledgements, and source-map diagnostics.
  - Negative contract: Reject unsupported variants with an intentional source-owned diagnostic and no silent legacy fallback.
- Source/AST: Observed `src/replacement_compatibility/migration_baselines/v0.5.0/capabilities.incn::CheckedCBindingFoundation`
- Typechecker: Observed `src/frontend/typechecker/check_expr/calls/rust_boundary.rs::fn validate_rust_boundary_value`
- Body IR: Planned `src/frontend/body_ir.rs::fn lower_call`; owner #989
- Replacement executor: Planned `src/backend/replacement/mod.rs::fn execute_call`; owner #989
- Aggregate comparison: unavailable; completed comparison infrastructure #1146 at Observed `tests/support/parity_corpus.rs::NonGreenShadowUnavailable`; outstanding evidence owner #989: The feature/runtime owner must add receipt-bound comparison evidence after its direct profile is materialized.
- Blocker/migration: Public ABI and interop parity is an explicit replacement-boundary slice, not a direct scalar-executor extension.

### `iteration.protocol-and-adapters`

Iterator protocols, adapters, and consumers preserve lazy dispatch, callback timing, exhaustion, and errors.

- `probe:iteration.protocol-and-adapters:binding-and-refusal` — positive AcceptedBehavior at Observed `src/replacement_compatibility/migration_baselines/v0.5.0/capabilities.incn::IteratorAdapters`; negative IntentionalRefusal at Observed `src/frontend/typechecker/check_expr/ops.rs::fn resolve_iteration_protocol`
  - Positive contract: Iterator protocols, adapters, and consumers preserve lazy dispatch, callback timing, exhaustion, and errors.
  - Negative contract: Reject unsupported variants with an intentional source-owned diagnostic and no silent legacy fallback.
- Source/AST: Observed `src/replacement_compatibility/migration_baselines/v0.5.0/capabilities.incn::IteratorAdapters`
- Typechecker: Observed `src/frontend/typechecker/check_expr/ops.rs::fn resolve_iteration_protocol`
- Body IR: Planned `src/frontend/body_ir.rs::fn lower_general_iteration`; owner #1152
- Replacement executor: Planned `src/backend/replacement/mod.rs::fn execute_loop`; owner #1152
- Aggregate comparison: unavailable; completed comparison infrastructure #1146 at Observed `tests/support/parity_corpus.rs::NonGreenShadowUnavailable`; outstanding evidence owner #1152: The feature/runtime owner must add receipt-bound comparison evidence after its direct profile is materialized.
- Blocker/migration: The callable/lazy-generator runtime slice owns the first admitted adapter profile; general protocol dispatch remains blocked.

### `iteration.user-and-fallible`

User-defined and fallible iteration preserve protocol calls, terminal behavior, and error routing.

- `probe:iteration.user-and-fallible:binding-and-refusal` — positive AcceptedBehavior at Observed `src/replacement_compatibility/migration_baselines/v0.5.0/capabilities.incn::FallibleIteration`; negative IntentionalRefusal at Observed `src/frontend/typechecker/check_expr/ops.rs::fn resolve_iteration_protocol`
  - Positive contract: User-defined and fallible iteration preserve protocol calls, terminal behavior, and error routing.
  - Negative contract: Reject unsupported variants with an intentional source-owned diagnostic and no silent legacy fallback.
- Source/AST: Observed `src/replacement_compatibility/migration_baselines/v0.5.0/capabilities.incn::FallibleIteration`
- Typechecker: Observed `src/frontend/typechecker/check_expr/ops.rs::fn resolve_iteration_protocol`
- Body IR: Planned `src/frontend/body_ir.rs::fn lower_general_iteration`; owner #1101
- Replacement executor: Planned `src/backend/replacement/mod.rs::fn execute_loop`; owner #1101
- Aggregate comparison: unavailable; completed comparison infrastructure #1146 at Observed `tests/support/parity_corpus.rs::NonGreenShadowUnavailable`; outstanding evidence owner #1101: The feature/runtime owner must add receipt-bound comparison evidence after its direct profile is materialized.
- Blocker/migration: Body IR protocol facts need a runtime dispatch and error-routing owner before the direct profile can admit these forms.

### `language.aggregates-and-projections`

Tuple, list, dict, set, slice, projection, mutation, equality, and ordering retain source semantics.

- `probe:language.aggregates-and-projections:binding-and-refusal` — positive AcceptedBehavior at Observed `src/replacement_compatibility/migration_baselines/v0.5.0/capabilities.incn::StdCollections`; negative IntentionalRefusal at Observed `src/frontend/typechecker/check_expr/collections.rs::fn check_list`
  - Positive contract: Tuple, list, dict, set, slice, projection, mutation, equality, and ordering retain source semantics.
  - Negative contract: Reject unsupported variants with an intentional source-owned diagnostic and no silent legacy fallback.
- Source/AST: Observed `src/replacement_compatibility/migration_baselines/v0.5.0/capabilities.incn::StdCollections`
- Typechecker: Observed `src/frontend/typechecker/check_expr/collections.rs::fn check_list`
- Body IR: Planned `src/frontend/body_ir.rs::fn lower_aggregate`; owner #1154
- Replacement executor: Planned `src/backend/replacement/mod.rs::fn evaluate_aggregate`; owner #1154
- Aggregate comparison: unavailable; completed comparison infrastructure #1146 at Observed `tests/support/parity_corpus.rs::NonGreenShadowUnavailable`; outstanding evidence owner #1154: The feature/runtime owner must add receipt-bound comparison evidence after its direct profile is materialized.
- Blocker/migration: Depends on #1101 for complete aggregate/place vocabulary; #1154 owns direct value storage and mutation.

### `language.control-flow`

Bounded scalar conditionals, loops, returns, assertions, and range iteration execute directly with explicit receipts.

- `probe:language.control-flow:bounded-direct-profile` — positive AcceptedBehavior at Observed `src/replacement_compatibility/migration_baselines/v0.5.0/capabilities.incn::IfWhileLet`; negative IntentionalRefusal at Observed `src/frontend/typechecker/check_expr/control_flow.rs::fn check_if_expr`
  - Positive contract: Bounded scalar conditionals, loops, returns, assertions, and range iteration execute directly with explicit receipts.
  - Negative contract: Inputs outside the bounded direct profile refuse visibly with their source span.
- Source/AST: Observed `src/replacement_compatibility/migration_baselines/v0.5.0/capabilities.incn::IfWhileLet`
- Typechecker: Observed `src/frontend/typechecker/check_expr/control_flow.rs::fn check_if_expr`
- Body IR: Observed `src/frontend/body_ir.rs::fn lower_if`
- Replacement executor: Observed `src/backend/replacement/mod.rs::fn execute_loop`
- Aggregate comparison: unavailable; completed comparison infrastructure #1146 at Observed `tests/support/parity_corpus.rs::NonGreenShadowUnavailable`; unscheduled evidence debt: The bounded direct profile has no scheduled owner for its remaining aggregate and corpus-case comparison evidence.

### `language.control-flow-complete`

Control flow beyond the bounded scalar profile preserves value-carrying branches, pattern binding, loop results, and diagnostics.

- `probe:language.control-flow-complete:binding-and-refusal` — positive AcceptedBehavior at Observed `src/replacement_compatibility/migration_baselines/v0.5.0/capabilities.incn::IfWhileLet`; negative IntentionalRefusal at Observed `src/frontend/typechecker/check_expr/control_flow.rs::fn check_if_expr`
  - Positive contract: Control flow beyond the bounded scalar profile preserves value-carrying branches, pattern binding, loop results, and diagnostics.
  - Negative contract: Reject unsupported variants with an intentional source-owned diagnostic and no silent legacy fallback.
- Source/AST: Observed `src/replacement_compatibility/migration_baselines/v0.5.0/capabilities.incn::IfWhileLet`
- Typechecker: Observed `src/frontend/typechecker/check_expr/control_flow.rs::fn check_if_expr`
- Body IR: Planned `src/frontend/body_ir.rs::fn lower_if_expr`; owner #1154
- Replacement executor: Planned `src/backend/replacement/mod.rs::fn execute_loop`; owner #1154
- Aggregate comparison: unavailable; completed comparison infrastructure #1146 at Observed `tests/support/parity_corpus.rs::NonGreenShadowUnavailable`; outstanding evidence owner #1154: The feature/runtime owner must add receipt-bound comparison evidence after its direct profile is materialized.
- Blocker/migration: The current direct profile covers only the bounded scalar subset; #1154 owns the required value and pattern runtime.

### `language.match-and-patterns`

Match, destructuring, alternation, guards, and exhaustiveness preserve branch selection and diagnostics.

- `probe:language.match-and-patterns:binding-and-refusal` — positive AcceptedBehavior at Observed `src/replacement_compatibility/migration_baselines/v0.5.0/capabilities.incn::PatternAlternation`; negative IntentionalRefusal at Observed `src/frontend/typechecker/check_expr/match_.rs::fn check_match`
  - Positive contract: Match, destructuring, alternation, guards, and exhaustiveness preserve branch selection and diagnostics.
  - Negative contract: Reject unsupported variants with an intentional source-owned diagnostic and no silent legacy fallback.
- Source/AST: Observed `src/replacement_compatibility/migration_baselines/v0.5.0/capabilities.incn::PatternAlternation`
- Typechecker: Observed `src/frontend/typechecker/check_expr/match_.rs::fn check_match`
- Body IR: Planned `src/frontend/body_ir.rs::fn lower_match`; owner #1154
- Replacement executor: Planned `src/backend/replacement/mod.rs::fn execute_call`; owner #1154
- Aggregate comparison: unavailable; completed comparison infrastructure #1146 at Observed `tests/support/parity_corpus.rs::NonGreenShadowUnavailable`; outstanding evidence owner #1154: The feature/runtime owner must add receipt-bound comparison evidence after its direct profile is materialized.
- Blocker/migration: Depends on #1101 for complete Body IR vocabulary; #1154 owns pattern dispatch over direct values.

### `language.numeric-and-scalar`

Bounded scalar arithmetic, comparisons, boolean operators, and strings execute directly from Body IR.

- `probe:language.numeric-and-scalar:bounded-direct-profile` — positive AcceptedBehavior at Observed `src/replacement_compatibility/migration_baselines/v0.5.0/capabilities.incn::NumericTypeSystem`; negative IntentionalRefusal at Observed `src/frontend/typechecker/check_expr/ops.rs::fn check_binary`
  - Positive contract: Bounded scalar arithmetic, comparisons, boolean operators, and strings execute directly from Body IR.
  - Negative contract: Inputs outside the bounded direct profile refuse visibly with their source span.
- Source/AST: Observed `src/replacement_compatibility/migration_baselines/v0.5.0/capabilities.incn::NumericTypeSystem`
- Typechecker: Observed `src/frontend/typechecker/check_expr/ops.rs::fn check_binary`
- Body IR: Observed `src/frontend/body_ir.rs::fn lower_binary`
- Replacement executor: Observed `src/backend/replacement/mod.rs::fn evaluate_binary`
- Aggregate comparison: unavailable; completed comparison infrastructure #1146 at Observed `tests/support/parity_corpus.rs::NonGreenShadowUnavailable`; unscheduled evidence debt: The bounded direct profile has no scheduled owner for its remaining aggregate and corpus-case comparison evidence.
- Completed #1146 case `replacement-body-v0-001` (ComparedMatch) comparison: paired Observed `tests/parity_corpus_tests.rs::legacy_receipt_identity`; Observed `tests/parity_corpus_tests.rs::replacement_receipt_identity`; Observed `tests/parity_corpus_tests.rs::fn the_compared_row_carries_two_route_receipts_and_its_oven_authority`

### `language.numeric-complete`

The full numeric contract preserves widths, literals, conversions, overflow, decimal behavior, and diagnostics beyond the scalar profile.

- `probe:language.numeric-complete:binding-and-refusal` — positive AcceptedBehavior at Observed `src/replacement_compatibility/migration_baselines/v0.5.0/capabilities.incn::NumericTypeSystem`; negative IntentionalRefusal at Observed `src/frontend/typechecker/check_expr/ops.rs::fn check_binary`
  - Positive contract: The full numeric contract preserves widths, literals, conversions, overflow, decimal behavior, and diagnostics beyond the scalar profile.
  - Negative contract: Reject unsupported variants with an intentional source-owned diagnostic and no silent legacy fallback.
- Source/AST: Observed `src/replacement_compatibility/migration_baselines/v0.5.0/capabilities.incn::NumericTypeSystem`
- Typechecker: Observed `src/frontend/typechecker/check_expr/ops.rs::fn check_binary`
- Body IR: Planned `src/frontend/body_ir.rs::fn lower_binary`; owner #1154
- Replacement executor: Planned `src/backend/replacement/mod.rs::fn evaluate_binary`; owner #1154
- Aggregate comparison: unavailable; completed comparison infrastructure #1146 at Observed `tests/support/parity_corpus.rs::NonGreenShadowUnavailable`; outstanding evidence owner #1154: The feature/runtime owner must add receipt-bound comparison evidence after its direct profile is materialized.
- Blocker/migration: The current direct profile is intentionally scalar-bounded; #1154 owns the shared direct value model required for remaining numeric forms.

### `language.strings-and-format`

String operators and formatting preserve interpolation order, conversions, and runtime failures.

- `probe:language.strings-and-format:binding-and-refusal` — positive AcceptedBehavior at Observed `src/replacement_compatibility/migration_baselines/v0.5.0/capabilities.incn::NumericTypeSystem`; negative IntentionalRefusal at Observed `src/frontend/typechecker/check_expr/ops.rs::fn check_binary`
  - Positive contract: String operators and formatting preserve interpolation order, conversions, and runtime failures.
  - Negative contract: Reject unsupported variants with an intentional source-owned diagnostic and no silent legacy fallback.
- Source/AST: Observed `src/replacement_compatibility/migration_baselines/v0.5.0/capabilities.incn::NumericTypeSystem`
- Typechecker: Observed `src/frontend/typechecker/check_expr/ops.rs::fn check_binary`
- Body IR: Planned `src/frontend/body_ir.rs::fn lower_binary`; owner #1101
- Replacement executor: Planned `src/backend/replacement/mod.rs::fn evaluate_binary`; owner #1101
- Aggregate comparison: unavailable; completed comparison infrastructure #1146 at Observed `tests/support/parity_corpus.rs::NonGreenShadowUnavailable`; outstanding evidence owner #1101: The feature/runtime owner must add receipt-bound comparison evidence after its direct profile is materialized.
- Blocker/migration: String concatenation, bounded scalar interpolation and selected canonical string helpers execute directly. The separate replacement-body-v0-021 corpus case proves the selected helpers, not this full formatting contract; broad feature parity remains non-green.

### `module.identity-and-aliases`

Modules, imports, aliases, namespaces, and reexports resolve to one source-observable identity.

- `probe:module.identity-and-aliases:binding-and-refusal` — positive AcceptedBehavior at Observed `src/replacement_compatibility/migration_baselines/v0.5.0/capabilities.incn::NamespacedStdlib`; negative IntentionalRefusal at Observed `src/frontend/typechecker/collect/stdlib_imports.rs::fn collect_import`
  - Positive contract: Modules, imports, aliases, namespaces, and reexports resolve to one source-observable identity.
  - Negative contract: Reject unsupported variants with an intentional source-owned diagnostic and no silent legacy fallback.
- Source/AST: Observed `src/replacement_compatibility/migration_baselines/v0.5.0/capabilities.incn::NamespacedStdlib`
- Typechecker: Observed `src/frontend/typechecker/collect/stdlib_imports.rs::fn collect_import`
- Body IR: Planned `src/frontend/body_ir.rs::fn lower_call`; owner #1042
- Replacement executor: Planned `src/backend/replacement/mod.rs::fn execute_call`; owner #1042
- Aggregate comparison: unavailable; completed comparison infrastructure #1146 at Observed `tests/support/parity_corpus.rs::NonGreenShadowUnavailable`; outstanding evidence owner #1042: The feature/runtime owner must add receipt-bound comparison evidence after its direct profile is materialized.
- Blocker/migration: Canonical source identity is a prerequisite for a replacement profile that crosses module boundaries.

### `nominal.models-unions-enums`

Models, unions, value enums, newtypes, computed properties, and static storage preserve construction and dispatch semantics.

- `probe:nominal.models-unions-enums:binding-and-refusal` — positive AcceptedBehavior at Observed `src/replacement_compatibility/migration_baselines/v0.5.0/capabilities.incn::ComputedProperties`; negative IntentionalRefusal at Observed `src/frontend/typechecker/check_decl.rs::fn check_model`
  - Positive contract: Models, unions, value enums, newtypes, computed properties, and static storage preserve construction and dispatch semantics.
  - Negative contract: Reject unsupported variants with an intentional source-owned diagnostic and no silent legacy fallback.
- Source/AST: Observed `src/replacement_compatibility/migration_baselines/v0.5.0/capabilities.incn::ComputedProperties`
- Typechecker: Observed `src/frontend/typechecker/check_decl.rs::fn check_model`
- Body IR: Planned `src/frontend/body_ir.rs::fn lower_constructor`; owner #1154
- Replacement executor: Planned `src/backend/replacement/mod.rs::fn evaluate_aggregate`; owner #1154
- Aggregate comparison: unavailable; completed comparison infrastructure #1146 at Observed `tests/support/parity_corpus.rs::NonGreenShadowUnavailable`; outstanding evidence owner #1154: The feature/runtime owner must add receipt-bound comparison evidence after its direct profile is materialized.
- Blocker/migration: Depends on #1101 for complete nominal Body IR; #1154 owns the direct nominal value representation.

### `package.public-boundaries`

Libraries, checked API metadata, providers, workspaces, and consumer imports preserve public identity and defaults.

- `probe:package.public-boundaries:binding-and-refusal` — positive AcceptedBehavior at Observed `src/replacement_compatibility/migration_baselines/v0.5.0/capabilities.incn::CheckedApiMetadata`; negative IntentionalRefusal at Observed `src/frontend/typechecker/collect/stdlib_imports.rs::fn collect_pub_imports`
  - Positive contract: Libraries, checked API metadata, providers, workspaces, and consumer imports preserve public identity and defaults.
  - Negative contract: Reject unsupported variants with an intentional source-owned diagnostic and no silent legacy fallback.
- Source/AST: Observed `src/replacement_compatibility/migration_baselines/v0.5.0/capabilities.incn::CheckedApiMetadata`
- Typechecker: Observed `src/frontend/typechecker/collect/stdlib_imports.rs::fn collect_pub_imports`
- Body IR: Planned `src/frontend/body_ir.rs::fn lower_call`; owner #989
- Replacement executor: Planned `src/backend/replacement/mod.rs::fn execute_call`; owner #989
- Aggregate comparison: unavailable; completed comparison infrastructure #1146 at Observed `tests/support/parity_corpus.rs::NonGreenShadowUnavailable`; outstanding evidence owner #989: The feature/runtime owner must add receipt-bound comparison evidence after its direct profile is materialized.
- Blocker/migration: Package and ABI boundaries deliberately remain outside the direct source-only profile until #656/#989 evidence exists.

### `runtime.std-data-services`

Data-oriented stdlib services preserve their documented input, output, and error contracts.

- `probe:runtime.std-data-services:binding-and-refusal` — positive AcceptedBehavior at Observed `src/replacement_compatibility/migration_baselines/v0.5.0/capabilities.incn::StdChecksum`; negative IntentionalRefusal at Observed `src/frontend/typechecker/stdlib_loader.rs::fn lookup_function_symbol`
  - Positive contract: Data-oriented stdlib services preserve their documented input, output, and error contracts.
  - Negative contract: Reject unsupported variants with an intentional source-owned diagnostic and no silent legacy fallback.
- Source/AST: Observed `src/replacement_compatibility/migration_baselines/v0.5.0/capabilities.incn::StdChecksum`
- Typechecker: Observed `src/frontend/typechecker/stdlib_loader.rs::fn lookup_function_symbol`
- Body IR: Planned `src/frontend/body_ir.rs::fn lower_call`; owner #1156
- Replacement executor: Planned `src/backend/replacement/mod.rs::fn execute_call`; owner #1156
- Aggregate comparison: unavailable; completed comparison infrastructure #1146 at Observed `tests/support/parity_corpus.rs::NonGreenShadowUnavailable`; outstanding evidence owner #1156: The feature/runtime owner must add receipt-bound comparison evidence after its direct profile is materialized.
- Blocker/migration: #1156 owns checked provider-service dispatch; collection and nominal values remain a #1154 prerequisite.

### `runtime.std-hosted-services`

Hosted filesystem, environment, I/O, web, temporary-resource, and process-adjacent services retain authority and lifecycle semantics.

- `probe:runtime.std-hosted-services:binding-and-refusal` — positive AcceptedBehavior at Observed `src/replacement_compatibility/migration_baselines/v0.5.0/capabilities.incn::StdEnviron`; negative IntentionalRefusal at Observed `src/frontend/typechecker/stdlib_loader.rs::fn lookup_function_symbol`
  - Positive contract: Hosted filesystem, environment, I/O, web, temporary-resource, and process-adjacent services retain authority and lifecycle semantics.
  - Negative contract: Reject unsupported variants with an intentional source-owned diagnostic and no silent legacy fallback.
- Source/AST: Observed `src/replacement_compatibility/migration_baselines/v0.5.0/capabilities.incn::StdEnviron`
- Typechecker: Observed `src/frontend/typechecker/stdlib_loader.rs::fn lookup_function_symbol`
- Body IR: Planned `src/frontend/body_ir.rs::fn lower_call`; owner #1156
- Replacement executor: Planned `src/backend/replacement/mod.rs::fn execute_call`; owner #1156
- Aggregate comparison: unavailable; completed comparison infrastructure #1146 at Observed `tests/support/parity_corpus.rs::NonGreenShadowUnavailable`; outstanding evidence owner #1156: The feature/runtime owner must add receipt-bound comparison evidence after its direct profile is materialized.
- Blocker/migration: #1156 owns checked provider-service dispatch and depends on authority/receipt facts from #662.

### `runtime.std-observability`

Logging, telemetry, registries, and metadata services preserve structured values and provider behavior.

- `probe:runtime.std-observability:binding-and-refusal` — positive AcceptedBehavior at Observed `src/replacement_compatibility/migration_baselines/v0.5.0/capabilities.incn::StdLogging`; negative IntentionalRefusal at Observed `src/frontend/typechecker/stdlib_loader.rs::fn lookup_function_symbol`
  - Positive contract: Logging, telemetry, registries, and metadata services preserve structured values and provider behavior.
  - Negative contract: Reject unsupported variants with an intentional source-owned diagnostic and no silent legacy fallback.
- Source/AST: Observed `src/replacement_compatibility/migration_baselines/v0.5.0/capabilities.incn::StdLogging`
- Typechecker: Observed `src/frontend/typechecker/stdlib_loader.rs::fn lookup_function_symbol`
- Body IR: Planned `src/frontend/body_ir.rs::fn lower_call`; owner #1156
- Replacement executor: Planned `src/backend/replacement/mod.rs::fn execute_call`; owner #1156
- Aggregate comparison: unavailable; completed comparison infrastructure #1146 at Observed `tests/support/parity_corpus.rs::NonGreenShadowUnavailable`; outstanding evidence owner #1156: The feature/runtime owner must add receipt-bound comparison evidence after its direct profile is materialized.
- Blocker/migration: #1156 owns checked provider-service dispatch; provider authority and receipts remain explicit prerequisites.

### `testing-and-tooling`

Test discovery, assertions, formatter, build reports, inspection, lifecycle, installer, and Oven observability preserve documented contracts.

- `probe:testing-and-tooling:binding-and-refusal` — positive AcceptedBehavior at Observed `src/replacement_compatibility/migration_baselines/v0.5.0/capabilities.incn::BuildReportsAndRustInspection`; negative IntentionalRefusal at Observed `src/frontend/typechecker/check_decl.rs::fn check_test_module`
  - Positive contract: Test discovery, assertions, formatter, build reports, inspection, lifecycle, installer, and Oven observability preserve documented contracts.
  - Negative contract: Reject unsupported variants with an intentional source-owned diagnostic and no silent legacy fallback.
- Source/AST: Observed `src/replacement_compatibility/migration_baselines/v0.5.0/capabilities.incn::BuildReportsAndRustInspection`
- Typechecker: Observed `src/frontend/typechecker/check_decl.rs::fn check_test_module`
- Body IR: Planned `src/frontend/body_ir.rs::fn lower_function_body`; owner #1034
- Replacement executor: Planned `src/backend/replacement/mod.rs::fn execute_free_function`; owner #1034
- Aggregate comparison: unavailable; completed comparison infrastructure #1146 at Observed `tests/support/parity_corpus.rs::NonGreenShadowUnavailable`; outstanding evidence owner #1034: The feature/runtime owner must add receipt-bound comparison evidence after its direct profile is materialized.
- Blocker/migration: These are control-plane contracts with source and receipt evidence, not direct Body-IR execution rows.

### `types.traits-generics-reflection`

Traits, generics, type tokens, protocol hooks, derives, and resolved method signatures preserve checked dispatch decisions.

- `probe:types.traits-generics-reflection:binding-and-refusal` — positive AcceptedBehavior at Observed `src/replacement_compatibility/migration_baselines/v0.5.0/capabilities.incn::AbstractTraits`; negative IntentionalRefusal at Observed `src/frontend/typechecker/trait_bound_relations.rs::fn type_satisfies_explicit_bound`
  - Positive contract: Traits, generics, type tokens, protocol hooks, derives, and resolved method signatures preserve checked dispatch decisions.
  - Negative contract: Reject unsupported variants with an intentional source-owned diagnostic and no silent legacy fallback.
- Source/AST: Observed `src/replacement_compatibility/migration_baselines/v0.5.0/capabilities.incn::AbstractTraits`
- Typechecker: Observed `src/frontend/typechecker/trait_bound_relations.rs::fn type_satisfies_explicit_bound`
- Body IR: Planned `src/frontend/body_ir.rs::fn lower_call`; owner #1033
- Replacement executor: Planned `src/backend/replacement/mod.rs::fn execute_call`; owner #1033
- Aggregate comparison: unavailable; completed comparison infrastructure #1146 at Observed `tests/support/parity_corpus.rs::NonGreenShadowUnavailable`; outstanding evidence owner #1033: The feature/runtime owner must add receipt-bound comparison evidence after its direct profile is materialized.
- Blocker/migration: Type-directed runtime calls and reflection need canonical source facts and value representation beyond the current profile.
