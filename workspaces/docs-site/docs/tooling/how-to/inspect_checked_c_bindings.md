# Inspect checked C bindings

Use `incan inspect bindings` when you need to review the raw C contract accepted by the compiler without reading generated Rust or reconstructing facts from the original vocabulary syntax.

## Review a project from the terminal

Run the command from the project root:

```bash
incan inspect bindings
```

The default text output lists each checked binding with its source module, header, exact native link declaration (`c.system_library(...)` or `c.framework(...)`), source location, resources, symbols, descriptor-owned bounded span associations, output outcomes, enums, and plain structures. This is the quickest form for reviewing whether a private binding says what its public façade expects.

Pass a source file when the binding is not reachable from the project's normal entrypoint:

```bash
incan inspect bindings src/sqlite.incn
```

When `PATH` is a directory, inspection selects `src/main.incn` when it exists and otherwise `src/lib.incn`. Local imports reachable from that entrypoint are included through the ordinary checked module graph.

## Produce JSON for another tool

Use the JSON format for an editor integration, audit tool, or reproducible review artifact:

```bash
incan inspect bindings . --format json
```

The report includes `schema_version: 2`, deterministic binding ordering, a compiler-owned descriptor identity, structural C types, and source anchors. Consumers should read the documented fields rather than parsing text output. The identity retains the declared header spelling, so use a portable spelling when it must survive relocation. See the [binding inspection JSON schema](../reference/binding_inspection_schema.md) for the exact contract.

## Emit a redacted binding-use receipt

Use the distinct receipt format when a build or audit system needs to retain only checked binding and direct raw-call identities:

```bash
incan inspect bindings . --format receipt
```

The receipt is JSON with `schema_version: 2`. It contains logical module names, compiler-owned binding identities, called native symbol names, and—when the compiler proves one—a direct public-function-to-private-bridge relation with that bridge's raw calls. It deliberately excludes source spans, package paths, declared headers, argument values, native bytes, and process-local addresses.

Join the receipt to one current locked Oven target when target provenance matters:

```bash
incan inspect bindings . --format receipt --target aarch64-apple-darwin
```

This validates the target's `oven.lock` projection and emits its portable `locked_target_identity`. When an already-selected Oven interop execution receipt exists for that exact target, the receipt validates it and includes its execution identity. If the target explicitly maps a checked binding's module/name pair to declared artifacts, that binding includes `target_artifacts`; a mapping with no compiler-produced binding is rejected rather than guessed. The receipt's `compatibility` object makes the v0.5 policy explicit: descriptor/ownership/ABI changes require an identical descriptor identity, and a selected target/artifact closure requires an identical locked-target identity. `--target` is accepted only with `--format receipt`; it is never silently ignored by the declaration report formats.

## Match the project's selected source view

Pass the same package features and SDK profile that the build or review uses when declarations are conditional:

```bash
incan inspect bindings . --format json --features sqlite --sdk-profile minimal
```

Inspection applies the normal package-feature and SDK-profile selection rules before typechecking. The result therefore describes the selected source graph rather than every declaration that could exist under a different configuration.

## Diagnose a failed inspection

Inspection is strict. If parsing, imports, typechecking, or the host-target C probe fails, the command emits the ordinary compiler diagnostic and no partial report.

Use the diagnostic to correct the binding declaration or selected environment, then run the inspection again. Do not weaken an exact C type, ownership mode, output contract, enum carrier, or plain layout merely to obtain JSON: a partial or guessed report would defeat the purpose of the command.

## Know where the report stops

The declaration report describes the checked language declaration. The receipt can join that checked use to one locked target and an already-selected execution receipt, and can report an explicitly authored binding-to-artifact-name correspondence. Neither format resolves or downloads artifacts, compiles C/C++ shims, selects a concrete compiler or SDK, emits a generated bridge implementation or linker invocation, classifies a whole package or re-export graph, or provides editor navigation. Its facade relation is limited to a direct same-module public-function-to-private-bridge call proved by the typechecker. `incan inspect codegraph` provides the corresponding source graph edges. Use `[interop.c]` and `incan lock` for declared physical requirements and locked package inputs; later Oven resolution and editor projections have separate lifecycles.

For the language-side recipes, see [Work with checked C bindings](../../language/how-to/checked_c_bindings.md). For the reason these facts stay separate, see [How checked C interop is structured](../../language/explanation/checked_c_interop.md).
