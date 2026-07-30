# Native deployment plan reference

`incan inspect native-plan` projects one declared and locked native target into a deterministic handoff for Gradle, Xcode, or another platform adapter. The report is binding-kind-neutral: it describes physical target inputs and actions without treating C headers, C ownership, or one platform command protocol as universal package metadata.

## Command

```text
incan inspect native-plan [PATH] --target <TRIPLE> [--format text|json]
```

`PATH` defaults to the current directory. It must select a standalone package or one project member of an Incan workspace. The selected package must contain an `incan.toml`, a `[native]` declaration with the exact selected target, and a current canonical `incan.lock`. For a workspace member, the command reads that member's projection from the single workspace-root lock rather than accepting a member-local lock. It re-hashes every declared native file and refuses to emit a plan when the selected native projection is missing or stale.

Use text output to audit the selected actions:

```console
incan inspect native-plan --target aarch64-linux-android
```

Use JSON when an Oven, Gradle, or Xcode adapter needs the versioned handoff:

```console
incan inspect native-plan --target aarch64-apple-ios --format json
```

The JSON report uses `schema_version: 1`. It contains only logical identities and package-relative paths, so moving the locked package does not change the report.

## Top-level fields

| Field | Meaning |
| --- | --- |
| `schema_version` | Compatibility version of the native deployment-plan shape. |
| `target` | Exact compilation and deployment target triple. |
| `toolchain` | Logical managed toolchain identity retained by the lock. |
| `sdk` | Logical SDK identity when the target requires one. |
| `platform` | Android API level or iOS deployment target selected for this target. |
| `headers` | Package-relative header paths and content digests. |
| `include_roots` | Deterministic package-relative include roots derived from locked public and shim headers. |
| `definitions` | Explicit preprocessor definitions shared by verification and future shim compilation. |
| `artifacts` | Dependency-ordered static, bundled, and system actions. |
| `shims` | Locked authored shim sources, headers, language, and logical output. |
| `provenance` | Package-supplied provenance evidence retained without making a publication-policy decision. |

## Artifact actions

Every artifact has a stable `name`, a sorted `dependencies` list, and one `deployment` action. Dependencies appear before their consumers in the report; missing siblings, duplicate edges, self-dependencies, and cycles are rejected before locking or plan emission. This is deterministic planning order, not a raw linker argument list. A platform adapter must use the explicit edges when deriving the order or grouping required by its selected linker.

`static_link` carries one package-relative archive path and its digest. It tells a later native builder that the archive participates in the final link without spelling a platform command line.

`bundle` carries one package-relative dynamic artifact and digest plus its runtime loader name, logical packager placement, and minimum platform constraint. An Android adapter can map the selected target and placement into its ABI-specific application inputs; an Apple adapter can map the same structured facts into its bundle or framework phase.

`system` carries one explicit toolchain or SDK capability such as `android.library.log` or `apple.framework.Accelerate`. It never searches the host for a similarly named library.

## Shim actions

A shim entry identifies locked C or C++ source files, the headers for its bounded C contract, and its logical output. The report does not claim that the shim has already been compiled. A future Oven bake must add a compiler- and target-keyed build receipt before the resulting Loaf can present that output as ready for final platform assembly.

## Boundary and exclusions

The report is a directionally useful native deployment handoff, not proof of a completed mobile build. This command does not provision a toolchain, download or build artifacts, compile shims, cross-compile generated Rust, stage files into an application, resolve link symbols, invoke Gradle or Xcode, sign an application, accept a license, or decide whether an artifact may be published.

Gradle and Xcode adapters should consume the structured target, link, bundle, capability, and placement facts rather than asking users to duplicate include roots, dependency order, ABI directories, framework capabilities, or runtime names manually. Their exact task and command protocols remain adapter concerns.
