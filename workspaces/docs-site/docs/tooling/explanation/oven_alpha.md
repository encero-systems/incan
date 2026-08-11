---
title: Oven Alpha
hide:
  - toc
---

<!-- markdownlint-disable MD033 -->

<section class="inc-oven-hero" aria-labelledby="oven-hero-title" markdown="1">

<div class="inc-oven-hero__copy" markdown="1">

<p class="inc-oven-hero__kicker">Experimental · Oven Alpha</p>

<h1 id="oven-hero-title"><span>Build once.</span><span>Keep the proof.</span></h1>

<p class="inc-oven-hero__lead">Oven turns a verified Rust compatibility closure into immutable Loafs, then lets normal Incan build, run, and test commands reuse direct <code>rustc</code> plans without Cargo on the consumer path.</p>

<div class="inc-oven-hero__actions" markdown="1">
[Why Oven exists](#why-oven-exists){ .md-button .md-button--primary }
[See the Alpha flow](#from-source-to-loaf-to-result){ .md-button }
[Read the architecture](../../whitepapers/incan_oven_positioning.md){ .md-button }
[Run the compiler suite](../../contributing/how-to/oven_alpha_benchmark.md){ .md-button }
</div>

<p class="inc-oven-hero__truth"><strong>Alpha means explicit boundaries.</strong> A compatible Loaf is reused; a miss explains whether to install an Oven-enabled toolchain or remove unsupported caller-owned Rust dependencies. Normal <code>incan build</code>, <code>incan run</code>, and <code>incan test</code> never quietly launch Cargo or switch backends.</p>

</div>

<div class="inc-oven-hero__image" role="img" aria-label="A cybernetic alpaca baker tending a glowing oven in a mountain workshop"></div>

<div class="inc-oven-hero__proofs" aria-label="Oven Alpha guarantees">
<div><strong>Receipt-bound</strong><span>Exact source, target, compiler, SDK, and dependency evidence.</span></div>
<div><strong>Policy-bounded</strong><span>Logical artifacts and physical disk use are measured separately.</span></div>
<div><strong>Lease-safe</strong><span>Active work cannot be pruned underneath a consumer.</span></div>
</div>

</section>

Oven Alpha is the first production-shaped slice of Incan's native build system. It is designed around a simple developer expectation: unchanged work should be ready to use, and the toolchain should be able to explain why.

For supported compatibility domains, Oven provides:

- content-addressed `<identity>.loaf/` bundles with a checked `loaf.json`, direct-`rustc` plan, artifacts, compatibility identity, provenance, digests, and byte accounting;
- exact reuse from compiler, SDK/provider, toolchain, target, profile, feature, lock, and source evidence;
- normal build, run, and test consumers that clear inherited Cargo state and never use Cargo as a fallback;
- a bounded Oven-owned store with dry-run pruning and active leases; and
- a complete repository test runner that reports both test cases and green or red roots.

## Why Oven exists

Cargo is a capable Rust package manager and build tool. The problem is not that Cargo cannot compile Incan's generated Rust; it can. The problem is that making Cargo the normal backend also makes Cargo's project graph, mutable target state, fingerprints, and command lifecycle the authority behind every Incan build. Incan can wrap that process, but a wrapper cannot independently promise that work was prepared once, prove why an artifact is compatible, bound all owned storage, or give the CLI, CI, IDE, and future interactive sessions one shared account of project state.

Oven changes who owns that contract. Rust source, crates, target triples, and `rustc` remain part of the system. Cargo remains a narrow compatibility publisher during Alpha. But Oven owns selection, identity, execution, evidence, and lifecycle for the supported consumer envelope.

| Concern | Cargo-centered Incan path | Oven model |
| --- | --- | --- |
| Project authority | Incan emits a generated Cargo project, then Cargo decides the build graph and manages target state. | Incan checks the project; Oven selects an explicit compatibility identity and verified build plan. |
| Unit of reuse | A mutable local target tree whose fingerprints are Cargo implementation state, not a portable Incan artifact contract. | An immutable Loaf with declared inputs, artifacts, direct-`rustc` plan, digests, provenance, and accounting. |
| When expensive work happens | The first consumer command pays unless a compatible local Cargo target happens to be warm. | The hidden publisher performs compatibility compilation once. `incan oven bake` explicitly materializes a shipped sealed Loaf into a local bounded store; an ordinary command may perform that same bounded copy, but never re-resolves or invokes Cargo. |
| Why reuse is valid | Cargo can explain compilation activity, but Incan has no single receipt spanning source, SDK, compiler, dependency closure, selection, and storage. | The same evidence selects the Loaf, drives execution, and records why work was reused or invalidated. |
| Storage and liveness | Cargo target directories are useful build state, but their lifecycle is not an Incan-owned, lease-protected storage policy. | Oven admits against aggregate and domain bounds, separates logical and physical use, and will not prune active work. |
| Local and CI semantics | Each surrounding tool can arrange Cargo differently and inherit different cache and process state. | Normal commands and the repository suite use the same stored plans, Cargo guard, compatibility rules, and reports. |

The Alpha implements the Oven side of this table for a deliberately bounded compatibility envelope. It does **not** yet replace Cargo's resolver for arbitrary Rust workspaces, publish Loafs through `incan.pub`, or provide the persistent interactive session model. Those are the north star, documented separately in [A Cargo-free toolchain for Incan and Rust](../../whitepapers/incan_oven_positioning.md).

## Where the innovation lies

Directly invoking `rustc` is not the innovation by itself, and putting a new command in front of Cargo would not be either. The architectural shift is treating a prepared compatibility closure as a first-class product rather than incidental build-cache state:

1. **A Loaf is an artifact, not a cache guess.** Its identity binds the source and lock evidence, compiler, SDK/provider, target, profile, features, artifacts, and integrity metadata needed to consume it safely.
2. **Preparation and consumption are separate contracts.** A publisher may do expensive compatibility work once; a normal command only selects and consumes a verified result. A miss is explicit instead of silently turning a small command into a large build.
3. **Receipts connect correctness, performance, and provenance.** The evidence that authorizes reuse is also what explains invalidation, elapsed phases, artifact origin, and storage ownership.
4. **Lifecycle is part of correctness.** Bounded admission, atomic publication, deterministic oversized-domain refusal, and active leases make artifact retention predictable under concurrency and crashes—not a cleanup convention left to users.

That foundation is what can later support published Loafs, mixed Incan/Rust workspaces, and persistent notebook or IDE sessions without changing the meaning of a normal build. Oven Alpha is the first narrow proof of that architecture, not the completed ecosystem.

## From source to Loaf to result

```text
checked source + lock + SDK + rustc
                 │
                 ▼
      hidden legacy_cargo Loaf baker
                 │
                 ▼
       <content-identity>.loaf/
        loaf.json + artifacts
                 │
                 ▼
      normal incan build/run/test
          direct rustc, no Cargo
```

Within Oven Alpha compatibility publication, Cargo has one deliberately named role: maintainers and CI may use the hidden `legacy-cargo` baker to resolve and compile a missing Rust dependency closure. Oven verifies the result, seals it into Loafs, and owns subsequent selection and execution. That boundary is visible and auditable; it is not a normal-command backend. Cargo may still be used to build the Incan compiler itself or by repository lint and security tooling.

An exact, complete envelope match returns `reused` without launching Cargo or rerunning behavioural probes. A missing, mutated, incomplete, or incompatible Loaf fails closed. Publication happens in isolated staging and an atomic manifest switch, so an interrupted replacement leaves the previous valid generation authoritative.

The built-in Alpha envelopes are typed in Oven:

- `release` contains feature-unified direct-`rustc` closures whose checked registry-source authority is selected independently for the standard-library Rust facades they support;
- `compiler-suite` contains the debug and release foundations used to run Incan's repository tests through Oven.

Their source programs are checked Incan fixtures. Make and CI compose the CLI; they do not define identity, bundle contents, admission policy, or fixture source. For the compiler-suite envelope, the same baker call also prepares or reuses the bounded receipt-compatible suite store selected by the Cargo-guarded replay.

## Normal commands

For a supported project, use the commands developers already know:

```bash
incan build
incan run
incan test
```

Each command selects a receipt-compatible Loaf and stored direct-`rustc` plan. A changed compiler, target, SDK, feature selection, lock, or relevant source changes the identity. An unchanged clean checkout can select the same Loaf because Git status and shell-script revisions are not compatibility inputs.

For a supported Incan library, an explicit preparation step makes first-use store materialization visible before a build:

```bash
incan oven bake --project . --format json
incan build --lib
```

`oven bake` records debug and release receipts, then reports `materialized` or `reused` for each selected plan. It only copies a receipt-compatible Loaf already shipped by the active toolchain through Oven's bounded, atomic, lease-aware store; it does not compile a final `rlib`, resolve a Rust graph, or launch Cargo. Registry-source authority for supported standard-library facades is selected independently from linkable closure selection, so source inspection never joins artifacts from different feature graphs. Generated Rust remains project-owned. Normal `build`, `run`, and `test` may also perform this controlled first materialization for a shipped compatible Loaf, but never an uncontrolled cold compatibility bake.

If no compatible Loaf exists, the command stops without invoking Cargo. For a library, first try `incan oven bake --project <library-root>`; if the active toolchain has no compatible shipped Loaf, install or reinstall an Oven-enabled toolchain for the target, or remove caller-owned Rust dependencies outside the documented Alpha envelope. `bake-loafs` remains only the maintainer path for preparing a toolchain; neither normal commands nor `oven bake` run that baker automatically.

Low-level `incan oven` commands remain available for inspection and verification. For example:

```bash
incan oven store inspect --format json
incan oven store prune --dry-run --format json
incan oven bake --project . --format json
```

The hidden baker is a maintainer interface, not a developer fallback:

```bash
incan oven legacy-cargo bake-loafs \
  --compiler-root . \
  --output target/share/incan/oven/loafs \
  --suite-store target/oven-compiler-suite-store \
  --envelope compiler-suite \
  --sdk-inventory path/to/sdk-inventory.json \
  --cargo "$(rustup which --toolchain nightly-2026-03-24 cargo)" \
  --rustc "$(rustup which --toolchain stable rustc)" \
  --format json
```

The publisher Cargo and consumer compiler are separate on purpose. The pinned Cargo supplies publisher-only graph data; the selected `rustc` defines the Loaf identity and performs direct compilation and replay. This lets the stable and MSRV lanes prove their actual consumer toolchains.

## Evidence you can inspect

Oven reports preparation and replay as separate phases. Repository-suite reports include total, passed, failed, ignored, and filtered test cases plus total, green, red, and unreported roots. The benchmark procedure records cold preparation, exact warm reuse, prepared replay, representative normal builds, Cargo-guard results, and phase wall-clock timings. Do not combine cold publication and prepared replay into one headline number: the approximately-five-minute acceptance target applies to the prepared full-suite replay.

Storage reports keep these quantities distinct:

| Field | Meaning |
| --- | --- |
| Logical artifact bytes | Sum of declared immutable payload lengths. |
| Policy physical bytes | Filesystem allocation charged by Oven policy. |
| Raw disk use | Host measurement of the relevant store or output tree. |
| Owned bytes | Physical allocation owned by the current envelope. |
| Reclaimable bytes | Inactive allocation policy may safely remove. |
| Active-lease bytes | Allocation protected by running consumers. |

See the [Oven Alpha benchmark guide](../../contributing/how-to/oven_alpha_benchmark.md) for the reproducible local sequence and the evidence files it produces. Performance numbers are meaningful only with their exact commit, machine, toolchain, cache state, workload, and storage junctions.

## Bounded storage, not cache archaeology

The default developer store is `$INCAN_HOME/oven/store/v2`, or `~/.incan/oven/store/v2` when `INCAN_HOME` is unset. Its filesystem encoding is safe to embed in a Unix native-runtime search path. Its defaults are 3 GiB aggregate physical allocation, 1 GiB physical allocation per compatibility domain, and 768 MiB logical artifact bytes per domain. Compiler-suite and release baking may use explicit allowances calibrated from their measured valid closures.

Oven enforces those bounds during admission. It can reclaim least-recently-used inactive entries, but never an active lease. A single domain that cannot fit its allowance is refused deterministically; an operator-supplied limit is never silently expanded. Production defaults are practical policy, not aspirational guesses: a healthy measured closure receives sensible headroom, while duplication, leaked intermediates, and unbounded growth remain defects.

Publication and replacement account for both the existing authoritative generation and private staging at the high-water point. Logical size, policy physical allocation, and raw disk use are reported independently because filesystem clones, sparse files, compression, and allocation units can make them differ.

## Current Alpha boundary

Oven Alpha proves the maintained Incan workflow and the repository's own compiler suite. It does not yet claim:

- general Cargo compatibility for arbitrary Rust workspaces;
- native resolution of every build script, procedural macro, target, or platform dependency shape;
- compressed or remotely distributed `.loaf` bundles;
- the authored `Loaf.toml`, resolved `Oven.lock`, workspace settings, or registry model proposed for later work; or
- broad ecosystem readiness based on Axum, Tokio, DataFusion, or other external-library bake-offs.

Those are 0.6-and-later releases and RFC work. For the v0.5 Alpha, the hidden baker is the only Cargo-backed compatibility publisher for supported Oven closures. If the Alpha envelope cannot authorize a normal command, Oven explains the miss and stops.

For the complete command surface, see the [CLI reference](../reference/cli_reference.md).
