# RFC 118: Incan and Oven command-line surfaces

- **Status:** Planned
- **Created:** 2026-08-04
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 073 (environment matrices and toolchain constraints)
    - RFC 078 (tool execution and typed workflow actions)
    - RFC 090 (typed CLI framework)
    - RFC 102 (Incan semantic layer inspection surface)
    - RFC 105 (`incan architect` rule engine)
    - RFC 106 (compiler-backed agent context graph)
    - RFC 116 (typed C ABI interop)
    - RFC 117 (`loaf.toml` and Oven's language-neutral project model)
    - RFC 119 (Oven-native Rust build facets and Cargo interoperation)
- **Issue:** [#1010](https://github.com/encero-systems/incan/issues/1010)
- **RFC PR:** —
- **Written against:** v0.5
- **Target scope:** v0.7 design and implementation planning; this is not a shipment claim.
- **Shipped in:** —

## Summary

This RFC defines two command surfaces delivered in one Incan installation. `incan` is the language and semantic-tooling command: it is deliberately Python-shaped for direct source use, modules, REPL work, formatting, language-server work, semantic inspection, codegraph, and architect analysis. `oven` is the project, package, environment, target, registry, and artifact-lifecycle command: it is deliberately Cargo-shaped for `loaf.toml` workspaces, planning, baking, locking, testing, actions, and publication.

The surfaces are not peer orchestrators. Incan may delegate downward to the Oven API when a semantic command needs a project plan or build work. Oven must not invoke the Incan CLI, expose Incan semantic subcommands, or learn Cargo behavior through Incan. Oven consumes compiler service APIs and owns its own operational plan, policy, cache, receipt, and provider lifecycle.

## Core model

1. **One installation, two surfaces:** installing Incan provides the compiler/runtime, Oven core and providers, `incan`, and `oven`. Oven is not a separately installed product.
2. **`incan` is semantic and source-oriented:** outside a Loaf it owns direct-file and module execution, checking, formatting, REPL/LSP operation, compiler diagnostics, semantic inspection, `codegraph`, and `architect`. Inside a Loaf, its approved project-scale conveniences remain shallow calls to Oven.
3. **`oven` is project-oriented:** it owns `loaf.toml` discovery, workspaces, dependency resolution, registry trust, environments, targets, carriers, plans, baking, locks, artifacts, receipts, publication, and explicit Cargo compatibility.
4. **Cargo familiarity is deliberate but bounded:** Oven may use recognizable Cargo-shaped verbs such as `add`, `update`, `lock`, `test`, and `publish`; it does not promise Cargo's implicit workspace, build-script, proc-macro, or manifest semantics for a Loaf.
5. **Incan familiarity is deliberate but bounded:** Incan may use familiar direct-source affordances such as a file argument, `-m`, `-c`, and a REPL. It must not grow a second dependency resolver, environment manager, cache, or package lifecycle model.
6. **Delegation is downward only:** an Incan command may request work from the Oven API. Oven may call compiler service APIs, but it must never shell out to or route its behavior through the Incan CLI.
7. **One operational authority:** every project operation, whether invoked as `oven ...` or through an approved Incan convenience, produces the same Oven plan, policy decision, selected target, `*.loaf` asset, and receipt.
8. **Language-neutral package operation:** neither command surface makes Rust, Incan, C ABI, JNI, Python, or another implementation substrate second-class. A selected Loaf behaves according to its declared provider and target requirements.

## Motivation

The existing compiler CLI, historical project lifecycle commands, Cargo compatibility, and emerging Oven capabilities otherwise converge into one ambiguous command family. That ambiguity is costly: users cannot tell which operations are language semantics, which mutate project state, which own caches and lockfiles, or whether a command is silently adopting Cargo behavior.

The desired experience has two useful mental models without two competing systems. `incan` should feel immediate and source-oriented in the way Python tooling does: run a file, run a module, inspect a program, format code, open a REPL, and ask semantic tools useful questions. `oven` should feel deliberate and project-oriented in the way Cargo does: select a member, resolve it, plan it, bake it for a target, test it, update the lock, inspect the result, and publish it.

This separation also protects the architecture. Incan's semantic products—diagnostics, codegraph facts, architect findings, checked package interfaces, and compiler-owned source meaning—must not be reimplemented by a build tool. Conversely, Oven's operational products—provider selection, target policy, content-addressed storage, leases, crash-safe publication, effect receipts, and registry trust—must not be recreated by each Incan subcommand.

## Goals

- Deliver `incan` and `oven` together in one Incan distribution without creating two competing package systems.
- Give direct source and semantic work a Python-shaped `incan` surface while giving project, registry, target, artifact, and receipt work a Cargo-shaped `oven` surface.
- Make the owner of every command observable through its plan, diagnostics, selected manifest/target, and receipt.
- Require project-scale `incan run` and `incan test` conveniences to delegate shallowly to the same Oven plan, scheduler, bake, `*.loaf` asset, and receipt as the canonical Oven operation.
- Keep the Incan CLI native Incan while preserving Oven's Rust operational core/API as the explicit cache, store, lease, lifetime, concurrency, and crash-safety exception.
- Ensure that neither CLI makes Incan, Rust, C ABI, JNI, Python, or another supported implementation substrate second-class.

## Non-Goals

- Defining `loaf.toml`, `oven.lock`, dependency resolution, target/carrier semantics, artifact format, or interop safety rules; RFC 117 and the corresponding interop RFCs own those contracts.
- Creating a second resolver, environment manager, cache, registry/trust model, or package lifecycle in the Incan CLI.
- Requiring the Oven CLI itself to be implemented in Incan by v0.7, or allowing its implementation language to change the Oven API/receipt contract.
- Inferring or emulating Cargo workspace, build-script, proc-macro, or manifest semantics in a Loaf command.
- Making every Oven operation available through an Incan alias.

## Guide-level explanation

### Use Incan for direct language work outside a Loaf

```text
incan run app.incn
incan run -m tools.release_notes
incan check src/
incan fmt src/ tests/
incan repl
incan lsp
incan inspect symbols src/main.incn
incan codegraph export --format json
incan architect src/
```

These commands are about source, language behavior, and semantic facts. A direct-file invocation does not require a `loaf.toml` project. When a semantic command is given a Loaf project, it may ask Oven for the selected dependency/provider context, but the compiler remains the authority for semantic results.

### Use shallow Incan conveniences inside a Loaf

Within a discovered `loaf.toml` project, project-scale execution is not a second build system:

```text
incan run                 # shallow alias to the selected Oven run/bake plan
incan test                # shallow alias to the selected Oven test plan and scheduler
incan build                   # shallow alias to `oven build` (terminal-carrier bake, e.g. executable)
incan check                # in-Loaf: compiler diagnostics plus read-only Oven plan diagnostics
incan run --direct tool.incn  # explicit direct-source escape hatch
```

`incan run`, `incan test`, and `incan build` inside a Loaf forward project selection to Oven. Oven resolves the same dependencies, target, carrier, profile, policy, scheduler, artifacts, and receipt that it would for the corresponding `oven` operation. Incan contributes source collection and semantic diagnostics; it does not create build state, choose a separate target, or issue a second receipt. `incan check` extends its existing direct-source behavior with read-only Oven plan diagnostics (unmet dependencies, invalid target/carrier) without becoming a second resolver. `--direct` is explicit because a nearby manifest must not silently turn deliberate scratch execution into a project bake.

There is no `incan bake` alias. Baking a loaf-shaped carrier packages a `*.loaf` asset for another Oven consumer to depend on — a deliberate, project-level publishing action in the same register as `oven publish`, not the kind of instinctive direct-language action `incan` aliases exist to shorten. `oven bake` remains `oven`-only.

### Use Oven for project work

```text
oven init
oven add stdlib-web
oven add --crate serde
oven build --target aarch64-apple-ios
oven plan --target aarch64-apple-ios --carrier framework
oven bake --target aarch64-apple-ios --carrier framework
oven test --workspace
oven env run ci test
oven action run generate-client --dry-run
oven inspect receipt
oven update
oven publish
```

These commands operate on a selected Loaf or workspace closure. They discover `loaf.toml`, resolve the typed dependency graph, apply inherited workspace authority, and report a plan before effects where policy requires it.

### `oven build` and `oven bake` are not the same operation

RFC 117 gives every carrier a shape: a loaf-shaped carrier (static library, framework, JNI shared library, Python wheel, Rust-host caller projection) bakes to a `*.loaf` asset meant for another Oven consumer to depend on; the terminal carrier (`executable`) bakes to a plain build artifact meant to be run or deployed directly and never depended on as a package. `oven build` and `oven bake` name that distinction instead of hiding it behind a `--carrier` flag choice:

```text
oven build                  # terminal carriers only (e.g. executable); plain build artifact, no .loaf packaging
oven bake                   # loaf-shaped carriers only; produces a *.loaf asset for downstream reuse
```

Both commands resolve the same underlying plan, policy decision, and receipt; they differ only in which carrier shapes they accept and what they name the output. `oven build --carrier framework` and `oven bake --carrier executable` are each a carrier/command mismatch and must fail with a diagnostic naming the correct command, rather than silently producing the other command's output shape. A project whose declared targets have no terminal carrier has nothing for `oven build` to build, and must fail with a diagnostic explaining that rather than falling back to a loaf-shaped default.

### `oven add` is shorthand; every noun gets its own full command family

`oven add <pkg>` is the most common project-mutation operation, so it is a bare shorthand for `oven dependency add <pkg>`, the same ergonomic reasoning as `cargo add` or `npm install`. `dependency` is not special beyond earning that one shortcut: it still gets its own full, explicit, discoverable subcommand family, the same shape every other noun gets:

```text
oven add stdlib-web              # shorthand for `oven dependency add stdlib-web`
oven dependency add stdlib-web   # same operation, spelled explicitly
oven dependency add --crate serde
oven dependency add --provider vision-runtime --version ">=2.0"
oven dependency list
oven dependency remove stdlib-web

oven mix add cli
oven mix list
oven mix show cli
oven mix status
oven mix diff cli --to 1.6.0
oven mix update cli --to 1.6.0
```

Only `dependency` gets the bare-verb shortcut; `mix` and future nouns do not collapse into `oven add`/`oven update`, because a mix id and a package name could otherwise collide under one ambiguous bare verb. This mirrors Docker's `docker run`/`docker ps` shortcuts coexisting with the full `docker container run`/`docker container ls` form: one proven, real-world precedent for "simple shorthand plus full explicit form, both valid, no runtime ambiguity about what a bare verb operates on."

### Inspecting semantic and operational facts

The names intentionally overlap only where the underlying questions differ:

| Question                                                                                 | Command surface             | Example                                                           |
| ---------------------------------------------------------------------------------------- | --------------------------- | ----------------------------------------------------------------- |
| What declarations, types, imports, and source relationships exist?                       | Incan semantic inspection   | `incan inspect symbols`, `incan codegraph export`                 |
| What architecture or safety findings follow from checked source?                         | Incan analysis              | `incan architect`                                                 |
| Which members, providers, targets, artifacts, cache entries, and receipts were selected? | Oven operational inspection | `oven inspect plan`, `oven inspect receipt`, `oven inspect cache` |

`oven inspect` must not masquerade as a semantic codegraph API, and `incan inspect` must not infer provider/cache facts from generated Rust or filesystem conventions.

### Cargo-only projects remain explicit

```text
oven cargo test
oven cargo build --release
```

Cargo compatibility is a clearly selected mode for a directory with `Cargo.toml` and no `loaf.toml`. Cargo remains authoritative, including its own side effects. If both files exist, normal Oven commands use `loaf.toml`, warn that Cargo was ignored, and require explicit Cargo compatibility to run Cargo semantics.

## Reference-level explanation

### Command ownership

| Domain                                                                             | Canonical owner | Illustrative commands                                    |
| ---------------------------------------------------------------------------------- | --------------- | -------------------------------------------------------- |
| Direct source/module execution outside a Loaf, or explicitly requested direct work | `incan`         | `run --direct`, `-m`, `-c`, `repl`                       |
| Parsing, checking, formatting, compiler diagnostics                                | `incan`         | `check`, `fmt`                                           |
| Language services and semantic products                                            | `incan`         | `lsp`, `inspect`, `codegraph`, `architect`               |
| Manifest and workspace selection                                                   | `oven`          | `init`, `new`, member selection                          |
| Dependency, lock, and registry lifecycle                                           | `oven`          | `add` (shorthand for `dependency add`), `dependency`, `remove`, `update`, `lock`, `registry`, `publish` |
| Target, carrier, provider, and artifact lifecycle                                  | `oven`          | `plan`, `build`, `bake`, `run`, `test`, `inspect`        |
| Environments, typed actions, project mutations                                     | `oven`          | `env`, `action`, `starter`, `mix`                        |
| Cargo compatibility                                                                | `oven`          | `cargo`                                                  |

An operation that changes dependency resolution, registry trust, a lock, generated project state, selected toolchain/provider, target output, or execution receipt belongs to Oven even when the selected sources are entirely Incan.

### Delegation and API boundary

The supported call direction is:

```text
incan CLI (native Incan)
  -> compiler semantic service API
  -> Oven API when project work is needed
       -> providers, stores, targets, receipts
```

```text
oven CLI
  -> Oven API
  -> compiler semantic service API when compilation is required
```

The second path deliberately has no arrow to the Incan CLI. Oven must not depend on parsing Incan CLI output, invoking `incan build`, importing CLI command handlers, or acquiring Cargo knowledge through the Incan surface.

The Incan CLI must be authored in native Incan. Oven's operational API remains a narrow Rust exception: planning, dependency/provider resolution, policy evaluation, cache/store access, leases, concurrency, provider execution, and crash-safe publication require explicit ownership and lifetime control at one coherent boundary. The public Oven API is language-neutral: it returns owned plan and receipt snapshots plus opaque leased handles, never Rust borrows across the Incan boundary. Rust's in-process lifetimes are necessary but insufficient; durable leases, content identities, atomic publication, and recovery remain Oven requirements.

The Oven CLI may use the same Rust operational API directly. Its implementation language is not a user-facing contract and may change without changing command behavior or receipt semantics.

### Alias rules

RFC 118 does not require every canonical Oven operation to have an Incan alias. An approved alias must be shallow: it forwards the user's selection to Oven and returns Oven's diagnostics, plan, target choice, and receipt identity without reinterpretation. It must not:

- resolve dependencies or registries itself;
- rewrite a target, profile, environment, action, or policy decision;
- conceal an explicit Cargo compatibility operation; or
- turn an Oven effect into an unplanned Incan compiler side effect.

Inside a discovered Loaf, `incan run`, `incan test`, and `incan build` are required shallow aliases; `incan check` is a required read-only extension of its existing direct-source behavior. `run`, `test`, and `build` must produce the same Oven plan identity, scheduler decision, selected target/carrier/profile, build artifact where applicable, and receipt as the canonical Oven operation. `incan build` follows `oven build`'s own carrier restriction: it is a terminal-carrier-only alias and must fail with a diagnostic, not silently package a `*.loaf`, if the selected project has no terminal carrier. A direct `incan run` in that directory requires `--direct`; the explicit flag makes the absence of Oven planning visible. There is no `incan bake` alias (see "`oven build` and `oven bake` are not the same operation"): packaging a `*.loaf` for downstream reuse is a deliberate project-publishing action, not a direct-language convenience, so it stays `oven`-only.

The remaining alias set is intentionally deferred until the first native Incan CLI vertical slice proves which conveniences improve direct language use rather than making the ownership boundary less legible.

### Distribution and versioning

The Incan distribution is the installation and update unit. It includes a compatible compiler/runtime and Oven core/provider set, and exposes both command entry points. A Rust-only project may use Oven, but installs the Incan distribution; it does not need to author Incan sources or separately install a competing Oven product.

The distribution must report compatible component versions and reject an unsupported Oven/provider/compiler combination before it claims to have produced a plan or bake. The receipt records the selected component identities.

### Relationship to RFC 090

RFC 090 owns `std.cli`, the framework for applications authored in Incan. It does not own project lifecycle behavior. The native Incan CLI should dogfood `std.cli` when its surface is ready, but RFC 118 does not make the Incan/Oven command contract contingent on a particular parser implementation. Oven's command semantics must remain defined by the Oven API rather than by a second application-level command model.

### Release boundaries

- **v0.5:** retains the current bounded RFC 116 interop release work and its existing `incan.toml` contract; this RFC does not backport a Loaf transition into that release.
- **v0.6:** RFC 117 introduces `loaf.toml`, `oven.lock`, hierarchical sub-Loaves, and the language-neutral project model.
- **v0.7:** this RFC introduces the canonical command-surface split over the v0.6 Oven API and project model.

## Compatibility and migration

This RFC does not require a separate Oven installation or a second lockfile format. It is intentionally incompatible with a model in which every project operation remains an `incan` command and owns its own package behavior.

Direct Incan source commands remain usable without a project manifest. Inside a discovered Loaf, project-scale `incan run` and `incan test` use Oven as specified above; `incan run --direct` is the explicit direct-source alternative. Oven project commands require `loaf.toml`, except explicit Cargo compatibility. Existing 0.5 `incan.toml` behavior is governed by the RFC 117 cutover; it is not preserved as a CLI compatibility parser.

## Alternatives considered

### One universal `incan` CLI

Rejected. It collapses language semantics, project mutation, cache/store ownership, registry trust, and Cargo compatibility into one ambiguous command family. It would also pressure the native Incan CLI to know Cargo behavior and Oven operational internals.

### A wholly independent Oven product

Rejected. Oven is a core Incan capability and shares versioning, compiler service APIs, providers, receipts, and user support. Separate installation would make Rust users install two overlapping toolchains without gaining architectural independence.

### Oven delegates upward to the Incan CLI

Rejected. Shelling out to a CLI creates an unstable text protocol, makes planner behavior depend on presentation code, and reverses the semantic/operational authority boundary. Oven uses compiler service APIs instead.

### Make Cargo compatibility implicit whenever a `Cargo.toml` exists

Rejected. It would allow Cargo workspace discovery and side effects to leak into Loaf behavior. Cargo mode is an explicit command choice.

## Drawbacks

- Two command names introduce a learning cost, especially for users who expect a language toolchain to have one executable.
- The native Incan CLI and the Oven CLI need consistent help, diagnostics, exit behavior, and installation reporting without duplicating implementation.
- Some familiar words—`run`, `test`, and `inspect`—exist on both surfaces, so documentation and diagnostics must name the selected authority plainly.
- The split requires a stable compiler service API and a stable Oven API earlier than a monolithic CLI would.

## Layers affected

- **Incan CLI** — must be authored in native Incan, own direct-source and semantic command behavior, and forward approved project-scale conveniences without reinterpreting Oven decisions.
- **Oven API and operational core** — must own planning, resolver/provider choice, policy, target selection, scheduling, artifacts, receipts, caches, stores, leases, and crash-safe publication through language-neutral owned snapshots and opaque handles.
- **Oven CLI** — must present the canonical project lifecycle surface over the Oven API. Its implementation language is non-normative; it must not become a second semantic CLI.
- **Compiler service API** — must expose bounded compilation and semantic services to Oven without making Oven invoke or import the Incan CLI.
- **Project execution and testing** — must make in-Loaf `incan run` and `incan test` shallow aliases over the same Oven plan, scheduler, target/carrier/profile, `*.loaf` asset, and receipt as the canonical Oven operation.
- **Diagnostics and inspection** — must identify direct-source, Loaf/Oven, and Cargo-compatibility modes and preserve the semantic-versus-operational `inspect` boundary.
- **Distribution and documentation** — must install and document both entry points as one versioned Incan distribution, including the explicit direct-source escape hatch.

## Inspectability and tooling surface

- **Artifacts and metadata:** `loaf.toml`, `oven.lock`, selected plan, target-bound `*.loaf` assets, execution receipt, compiler diagnostics, semantic inspection facts, codegraph records, and architect findings expose the chosen authority.
- **Inspection commands:** `oven inspect` reports operational plan/store/receipt facts; `incan inspect`, `incan codegraph`, and `incan architect` report checked semantic facts.
- **Diagnostics:** commands must name whether failure occurred in direct-source mode, Loaf/Oven mode, or explicit Cargo-compatibility mode, and identify the selected manifest or target where relevant.
- **Not implicit:** neither command surface may infer Cargo behavior from an adjacent manifest, execute an action during resolution, or hide the delegation path that produced a receipt.

## Design decisions

- **`oven build` and `oven bake` are two commands, not one spelling choice:** RFC 117 gives every carrier a fixed shape (loaf-shaped or terminal); `build` and `bake` name that split instead of hiding it behind a `--carrier` flag. `oven build` is terminal-carrier-only and produces a plain build artifact; `oven bake` is loaf-shaped-carrier-only and produces a `*.loaf` asset for downstream reuse. Both resolve the same underlying plan/policy/receipt; a carrier/command mismatch (`oven build --carrier framework`, `oven bake --carrier executable`) fails with a diagnostic naming the correct command rather than silently producing the other command's output shape. There is no `oven build`-as-Cargo-compatibility-alias: it is the terminal-carrier command in its own right, not a synonym for `oven bake`.
- **The required in-Loaf alias set is `run`, `test`, `build`, and `check`:** `run`, `test`, and `build` are shallow aliases producing the same Oven plan identity, target/carrier/profile, artifact, and receipt as the canonical `oven` operation; `incan build` inherits `oven build`'s terminal-carrier restriction rather than silently packaging a `*.loaf`. `incan check` extends its existing direct-source behavior with read-only Oven plan diagnostics (unmet dependencies, invalid target/carrier) without becoming a second resolver. There is deliberately no `incan bake`: packaging a `*.loaf` for another consumer to depend on is a deliberate project-publishing action in the same register as `oven publish`, not the kind of instinctive direct-language convenience `incan` aliases exist to shorten — it stays `oven`-only, and the remaining alias set beyond this four stays deferred to the first native Incan CLI vertical slice, per this RFC's existing "Alias rules" language.
- **`oven registry` covers local registration and trust inspection only, no login/credential command:** `oven registry add/list/remove` manage local registration fields (kind, endpoint/index, trust policy) exactly as RFC 117 already defines them; `oven registry inspect <name>` reports trust policy, protocol, signature/integrity state, and allow-list usage. There is no `oven registry login` or other credential-acquisition command in this RFC. Credentials are referenced from secure user/organization storage, per RFC 117's already-settled model; how a credential gets into that storage is either RFC 034's job for the default `incan.pub` registry (which already owns its protocol and publication authority) or the registry operator's own concern for any other registry, per RFC 117's Q4 resolution. Inventing a generic login verb here would mean designing credential acquisition for registries this RFC has no visibility into.
- **Global flags are a shared naming/behavior convention, not shared code:** `incan` and `oven` use the same flag names and semantics for `--format json`, `--color`/`--no-color`, `-v`/`--verbose`, `--profile <name>`, `--target <triple>`, and project-selection flags, documented once and followed by both. This is ordinary consistency discipline for two entry points in one distribution, the same reasoning `git`/`git-lfs` or `docker`/`docker-compose` already apply — it does not require a shared flag-parsing dependency or any new mechanism, and does not interact with the delegation boundary this RFC defines elsewhere.
- **Evidence bar for an Incan-authored Oven CLI:** this RFC already leaves the Oven CLI's implementation language non-normative ("may change without changing command behavior or receipt semantics"), and RFC 117 already requires authored Rust in Incan's own tooling to have "a demonstrated Incan limitation and a tracked removal path." For the Oven CLI specifically, that evidence is: (1) RFC 090's `std.cli` has reached the same maturity bar this RFC already sets for the native Incan CLI; (2) a full command/receipt parity suite demonstrates identical plans, diagnostics, and receipts between the existing Rust CLI and the candidate Incan-authored one across real projects, proving the unchanged-contract requirement rather than assuming it; (3) the rewrite consumes only the same public, language-neutral Oven API any third-party Incan program could already call, with no special or internal access, proving the API boundary this RFC defines is real; and (4) no meaningful startup or latency regression, since a CLI is a high-frequency, latency-sensitive surface regardless of implementation language. None of these are met today; this is a future possibility this RFC leaves open, not a v0.7 commitment.
