---
title: "Ship the loaf, not the recipe: why Incan and Rust need incan.pub"
status: "Draft positioning paper"
snapshot_date: "2026-09-09"
authors:
  - "Danny Meijer"
audience:
  - "Rust developers evaluating Incan, including those who think crates.io already solved this"
  - "Registry, packaging, and supply-chain practitioners"
  - "Incan contributors and prospective collaborators"
scope: "Product positioning for incan.pub as a compiled, attested distribution tier for Oven-built projects, and the measured cost of Rust's source-only model that motivates it."
normative: false
related_rfc:
  - "RFC 034"
  - "RFC 079"
  - "RFC 117"
  - "RFC 118"
  - "RFC 119"
  - "RFC 123"
  - "RFC 124"
  - "RFC 125"
research_context:
  - "Cargo's sparse index, lockfile, and feature-unification model"
  - "uv's registry client, content-addressed cache, and universal lockfile"
  - "npm's content-addressed cache, lockfile history, scopes, and Sigstore provenance"
  - "Maven Central, PyPI wheels, Nix binary caches, and Homebrew bottles as compiled-distribution precedents"
  - "Cold-build measurements of tokio, axum, DataFusion, and the Incan compiler on two hosts"
  - "Cargo issues #1139, #1997, #4436, #5931 and the 2026 Cargo cross-workspace cache project goal"
  - "Rust internals threads on precompiled dependencies (2021) and precompiled artifact support (2025)"
  - "The serde_derive precompiled-binary episode (July to August 2023) and the watt project"
  - "Conan's package_id binary model, crate2nix, sccache, cargo-binstall, and the warg registry"
  - "crABI, the rlib-stabilisation pre-RFC, and the rustc reproducible-builds tracking issue"
related_whitepapers:
  - "A Cargo-free toolchain for Incan and Rust"
  - "Incan ecosystem north star"
review_after: "When a compiled-artifact tier ships in any Rust-ecosystem registry, or when Cargo's cross-workspace cache gains a remote tier"
---

# Ship the loaf, not the recipe: why Incan and Rust need `incan.pub`

--8<-- "_snippets/callouts/whitepaper_status.md"

> **Current boundary:** `incan.pub` does not exist as a service. This paper argues what such a registry should be measured against and why the contract is worth building. Every number below was collected on 2026-09-09 with the scripts and versions named in the measurements section, on two ordinary machines, and is reproducible from those scripts. Nothing here is a benchmark of `incan.pub` itself.

## Abstract

Rust's compiled artifacts are excellent. Producing them is the expensive part, and the Rust ecosystem makes every machine produce them from scratch, because it has no unit of exchange for a compiled artifact whose inputs are provably identical. crates.io distributes recipes; every laptop, CI runner, and container image bakes.

This paper measures what that costs on two hosts for three widely used libraries and for the Incan compiler itself, examines why Rust in particular never shipped compiled dependencies when Java, Python, Nix, and Homebrew all did, and derives the properties any answer would need to satisfy. What is missing is not a cache but a **verifiable identity model for compiled work**: a way to say precisely enough, about a stranger's artifact, that its inputs were the same as yours, that its bytes are the ones its builder attested, and whose claim you are accepting. A registry follows from that; it does not substitute for it.

One candidate is then measured against those properties — Oven's receipt model, with `incan.pub` as an exchange built on the identities it produces — without touching crates.io, and without asking any Rust project to adopt anything. The properties come first on purpose: they are a standard any answer can be judged against, including this one.

## The claim

One sentence carries the whole argument: **Cargo exchanges source; what Rust still lacks is an exchangeable identity for compiled work.**

Artifacts are exchanged. Identity determines whether they may be.

Everything below is evidence for that, and a proposal that follows from it.

For Rust, the compiled artifact is the product, and the ecosystem makes every consumer re-derive it from source on every machine. That re-derivation is expensive in time, memory, and disk, it is paid most heavily by the machines least able to afford it, and it is paid again for every checkout, every runner, and every image. The reason it has never been fixed is not that nobody wanted to; it is that two compilations of the same crate are only safely interchangeable when the inputs that affect compatibility match, and the Rust toolchain has never exposed an identity precise enough to establish that across machines.

The aim is to improve the foundation Rust already laid, not to displace it. The compiler, the crate model, the sparse index, and the lockfile stay. crates.io stays and is consumed as-is. What is added is the layer Rust never built: an exchangeable identity for a compiled artifact, and somewhere to exchange artifacts that carry it.

It is worth being exact about what that place is, because one word invites the wrong fear. For projects that publish a Loaf, `incan.pub` is a registry in the ordinary sense — a source of record, with versions and a scoped namespace. For crates.io dependencies it is not a second source registry at all; it is an artifact exchange layered over crates.io, holding compiled work whose source continues to live, unmirrored, where it always did. The fragmentation worry attaches to the second case, and in that case there is nothing to fragment. Java, Python, Nix, and Homebrew each added that layer on top of their source ecosystems, and none of them is remembered as a fork.

## What Cargo and crates.io got right

A paper that reads as anti-Cargo would lose the readers it is written for, and it would also be wrong. Cargo solved the problems that have to be solved before compiled distribution is even thinkable.

The **sparse index** is the right shape for package metadata: one small static file per package, listing every version with its dependencies and features, cacheable by any CDN, with no request that returns more than a resolver needs. npm spent years bolting an abbreviated-metadata content type onto a design that returned every version's README in one document; Cargo never had that problem. A registry serving compiled artifacts should copy the sparse index almost verbatim.

The **lockfile** is flat, keyed by package identity rather than filesystem path, records a checksum per artifact, and merges cleanly. That is the format uv adopted for Python and the format npm arrived at after three incompatible versions. `oven.lock` inherits it.

**Immutability and yank** were correct from the start: a published version is never overwritten, and withdrawing one leaves existing locks working while steering new resolutions away. The left-pad incident taught npm the same lesson in public.

**Feature unification** is the mechanism that guarantees one compiled instance of a crate per build, which Node's per-package `node_modules` nesting never had and which peer dependencies were invented to paper over. It is also, as the next section shows, the mechanism that makes compiled artifacts hard to share across builds. Both facts are true at once.

Most importantly, crates.io made Rust's source ecosystem **trustworthy enough** that compiled distribution is now possible at all. You can only ship an artifact derived from a source if the source's identity is settled. Cargo settled it. Compiled distribution is what becomes possible once that trust exists.

## What producing the artifact costs today

The argument needs measurements, not adjectives, and it needs measurements on hardware that developers actually own. Two hosts were used. One is a high-end laptop: Apple M5 Max, 18 threads, 128 GB of memory. The other is what most developers and most CI runners resemble: an AMD Ryzen 5 7640HS with 12 threads and 16 GB of memory. Both ran stable Rust 1.98 and the same script, which creates a fresh project whose `main` does the minimum to exercise one library, fetches sources, and builds once in release mode with `--timings` into an empty target directory.

Three libraries were chosen for their reach rather than their difficulty. tokio is one of Rust's most widely used async runtimes. axum is a common web entry point. DataFusion is a large, real, widely used dependency: a query engine built on Arrow, representative of a substantial analytical dependency graph, and the proving-ground workload named in the Oven positioning paper.

### The high-end laptop

| Probe | Packages | Source download | Wall | Unit-seconds | Build scripts / proc-macros | Binary | Target directory | Compiled closure |
|---|---|---|---|---|---|---|---|---|
| tokio 1.53 `full`, async hello | 24 | 5.3 MB | 4.9 s | 9 s | 4 / 1 | 0.8 MB | 54 MB | 18 rlibs, 8 MB compressed |
| axum 0.8.9, one route | 52 | 7.5 MB | 5.8 s | 23 s | 8 / 2 | 0.9 MB | 114 MB | 47 rlibs, 19 MB compressed |
| DataFusion 55, open a session | 282 | 35.8 MB | 2 min 32 s | 903 s | 40 / 21 | 121 MB | 1.25 GB | 237 rlibs, 172 MB compressed |

### The consumer machine

| Probe | Packages | Source download | Wall | Unit-seconds | Build scripts / proc-macros | Binary | Target directory | Compiled closure |
|---|---|---|---|---|---|---|---|---|
| tokio 1.53 `full`, async hello | 24 | 5.1 MB | 8.5 s | 17 s | 4 / 1 | 0.4 MB | 83 MB | 17 rlibs, 11 MB compressed |
| axum 0.8.9, one route | 52 | 7.1 MB | 11.6 s | 49 s | 8 / 2 | 0.5 MB | 151 MB | 48 rlibs, 22 MB compressed |
| DataFusion 55, open a session | 282 | 34.1 MB | 6 min 57 s | 3,454 s | 40 / 21 | 106 MB | 1.64 GB | 237 rlibs, 195 MB compressed |

Unit-seconds is the sum of the durations cargo attributes to each compilation unit. It is not a clean per-core measure: under memory pressure it includes contention, which is exactly what happened on the smaller machine.

### What the tables say

**Small libraries are not the problem.** tokio and axum build in seconds on both machines. Anyone who leads a conversation with a Rust developer by calling tokio expensive will lose them, correctly. crates.io's source model is entirely adequate for the long tail of small crates, and nothing in this paper proposes changing how they are consumed.

**Heavy stacks are the problem, and they are common.** A program that opens a DataFusion session and prints its id costs twenty-seven CPU-minutes on the laptop and, on the consumer machine, seven minutes during which the desktop was unresponsive. The task manager during that build read 100% CPU, 69% memory, 1% disk, and 0% network, with a single `rustc` process at 95% of a core and 1.16 GB resident. The last two numbers are the argument in miniature: nothing was being downloaded and almost nothing was being read or written. The machine was purely re-deriving compiled work already performed elsewhere, many times over, for this same dependency closure, toolchain, target, and configuration.

**Consumer hardware pays most.** Wall time grew 2.7 times between the two hosts; unit-seconds grew 3.8 times. The extra growth is contention. Cargo runs one `rustc` per logical core by default, each DataFusion crate wants over a gigabyte while it compiles, and a 16 GB machine has nowhere to put twelve of them. The cost of source-only distribution lands hardest on exactly the machines that can least absorb it, and many hosted CI runners are comparably constrained.

**The trade is favourable.** The compiled closure of the DataFusion probe compresses to 172 to 195 MB with gzip, the figure in the tables, and to about 120 MB with zstd at a high level, roughly three to six times the 35 MB of source. Two thirds of the compressed bytes are compiler metadata rather than machine code, which compresses only 3.5 times against 8 times for the object files; that ratio is a property of the compiler, not of the registry. A CDN serves that in seconds on an ordinary connection, in exchange for seven minutes of a saturated machine and 1.6 GB of scratch space, on every machine that ever wants this dependency at this toolchain and target.

**Repetition is the multiplier.** None of the numbers above is paid once. They are paid per checkout, per clean CI run, per container layer, per developer on the team, and per toolchain bump.

### The compiler is the same shape

Incan is, at its core, a very large Rust project, and it feels every one of these costs first. A cold release build of the `incan` command at the current development head compiles 340 crates in 1 minute 47 seconds on the laptop, 722 unit-seconds, for a 73 MB binary; the rust-analyzer and Wasmtime families it depends on are half of that time. Toggling one Cargo feature afterwards recompiled 53 units because tokio's feature set changed underneath them. A freshly generated Incan hello-world project compiles fourteen dependency rlibs into a 51 MB local Loaf per profile before it prints a line, and two such projects on one machine each did it separately. Importing two functions from `std.hash` raised that to thirty-nine rlibs. These are the compiler's own findings about itself, and they are the reason Incan's toolchain has to be shaped differently from an ordinary Cargo workspace.

### Disk is the second symptom

On the laptop, in its resting state after a cleanup, one development root held twenty `target/` directories totalling 52 GB, two of them the Incan compiler itself at 10.8 GB and 10.4 GB a few commits apart. The registry cache held 6.5 GB of `.crate` files. The maintainer's periodic cleanups run to 500 GB or more. This is not an argument for a registry, but it is the same design fact from another angle: a compiled artifact with no identity anyone else can trust cannot be shared and cannot be safely deleted, so every worktree keeps its own copy of the same wasmtime, and nothing can tell that two 10 GB directories are the same thing.

## Compilation is infrastructure, not developer experience

Compile time is the tangible symptom, and it is the one every developer feels, which makes it the easiest thing to argue about and the easiest to dismiss as impatience. The more durable argument is that repeated derivation is an infrastructure cost the whole ecosystem pays, continuously, for work that has already been done somewhere else.

The same seven minutes reappears in places that have nothing to do with anyone's patience. It is billed as CI compute on every push, on every branch, for every project in an organisation, and CI runners are rented by the minute. It is drawn as energy in a datacentre and as battery on a laptop, for a computation whose result was deterministic before it started. It is onboarding latency, where a new contributor's first impression of a project is a progress bar. It is the reason ephemeral development environments — devcontainers, cloud workspaces, throwaway CI images — are quietly expensive, because ephemerality means every environment starts from nothing and re-derives everything. It is the reason a classroom of thirty students compiles the same dependency graph thirty times on hardware chosen for price. And it is a trust problem in deployment, because a build that takes seven minutes on one machine and is rebuilt on another is accepted on the assumption that it produced the same thing, rather than on evidence.

None of these is a complaint about Rust being slow. They are all the same structural fact: the ecosystem treats a deterministic, expensive computation as something each consumer must perform privately, and has no way to share the result safely. Every mature ecosystem eventually stops paying that cost — not because compilation got faster, but because artifacts became exchangeable.

## Why Rust never shipped compiled dependencies

Every other mainstream ecosystem did. Maven Central has distributed compiled JARs since its beginning; nobody compiles Guava from source. PyPI's wheels ended the era of building numpy on every laptop, and uv's design assumes wheels as the normal case. Nix binary caches serve build outputs keyed by the hash of every input that produced them. Homebrew bottles turned a source package manager into one that installs in seconds. Debian, Fedora, and every Linux distribution are compiled distribution by definition.

Rust's own toolchain does it too, up to a point: `rustup` ships a compiled standard library for every target, and nobody thinks that is a fork of Rust. It stops at `std`.

The reason it stops is real. Rust has no stable ABI across compiler versions, and a compiled crate encodes its exact dependency versions, its enabled features, its target, its profile, and its compiler build into a strict version hash. Two compilations of the same crate at the same version are interchangeable only when all of those match, and mixing two that do not is not a warning but a broken build or a subtly wrong program. Cargo's answer was to make the local target directory the unit of consistency and never to share compiled output beyond it.

### The triangle that made this look impossible

Underneath the specific objections is one shape, and naming it explains why the stall lasted a decade rather than a release cycle. Rust's ecosystem insists on three things at once:

- **Derivation fidelity** — the artifact is attributable to the declared source and the compilation inputs actually in effect, not merely adjacent to them. This is weaker than reproducible builds and is deliberately not the same property; the distinction is taken up later.
- **Source transparency** — every artifact has a source of record that anyone can read, audit, rebuild, and redistribute.
- **Configurable builds** — features, profiles, targets, and dependency versions are the consumer's to choose, and choosing differently produces genuinely different output.

Every historical attempt to ship compiled artifacts sacrificed one of the three. Distributing a binary without its source gives up transparency, which is what the ecosystem rejected in 2023. Pinning one blessed configuration gives up configurability, which is why a precompiled artifact for one target helped almost nobody. Accepting a near match gives up derivation fidelity, and in a language with no stable ABI the result can be a defect that survives a clean build.

The triangle is only binding while artifacts cannot be told apart. Given an identity that captures every input a configuration can vary, all three survive: the source stays the publication of record, any configuration may be published or built locally, and an artifact is offered only where its inputs match exactly. That is the actual claim of this paper. The registry is a consequence of it.

### A decade of asking

This is not a gap nobody noticed. The request is older than most of Rust's stable features, and the Rust project is now building the local half of the answer.

- [Cargo issue #1139](https://github.com/rust-lang/cargo/issues/1139), "Support for pre-built dependencies", was opened in January 2015. It is still open, labelled as needing design, and was still receiving comments in April 2026 from people asking how to ship a Rust library without shipping its source.
- [Cargo issue #5931](https://github.com/rust-lang/cargo/issues/5931), "Per-user compiled artifact cache", was opened in 2018 and is still open with sixty-seven comments. The newest, from September 2026, reports an experiment backing the cache with a content-addressed store that saved half a gigabyte of thirty, because the cache is deliberately conservative about what it dares to share. [Issue #4436](https://github.com/rust-lang/cargo/issues/4436) from 2017 was closed into it; [issue #1997](https://github.com/rust-lang/cargo/issues/1997) from 2015 was closed when sccache gained distributed compilation.
- Brian Anderson's [The Rust Compilation Model Calamity](https://www.pingcap.com/blog/rust-compilation-model-calamity/) (2020) documented fifteen-minute debug and thirty-minute release rebuilds of TiKV and traced them to design choices that favour run-time over compile-time. The series diagnosed the cost; it did not propose distribution.
- In 2026 the Cargo team made a **cross-workspace build cache** an official [project goal](https://goals.rust-lang.org/2026/cargo-cross-workspace-cache.html), following the 2025 split of the target directory into artifact and build directories. The ecosystem is now building the local half of this; what it does with the remote half is examined below.

### The objections, and what answers them

The May 2025 internals thread [Add some form of precompiled artifact support to cargo](https://internals.rust-lang.org/t/add-some-form-of-precompiled-artifact-support-to-cargo/22871) is the clearest statement of why the Rust project has not done this through crates.io, and every objection in it is a design constraint this paper accepts.

The Cargo maintainer's objection was that precompiled packages lock in profile settings, compiler flags, and dependency versions, taking control away from the top-level package that Cargo's model puts in charge. An exact-match asset does the opposite: the consumer's plan is computed first, and an asset is offered only when its receipt equals that plan. The top-level project never loses a decision; it gains a shortcut when someone already made the same one.

A second objection was that Cargo would need a real model of the relationship between build environment and ABI before it could trust an artifact. That model is the requirement this paper derives below, and the thing any candidate has to supply. A third was that `-sys` crates with system dependencies cannot be shared; they will often be inapplicable, and inapplicable means a bake, not a failure. A fourth, from a participant who wanted crates.io to stay source-only, was that any shared cache needs "100% deterministic and reproducible derivations from source packages", in the manner of a Nix binary cache. That is the standard registry-built assets have to meet, and the rustc [reproducible-builds tracking issue](https://github.com/rust-lang/rust/issues/129080) with its known Windows nondeterminism is an honest constraint on how soon they can meet it. The thread's own landing point was the request for "a shared, blessed compilation cache with graceful fallback to local compilation". That sentence is the asset tier.

### The serde lesson

The one time a major Rust crate shipped a compiled artifact, the ecosystem rejected it, and the way it was rejected is instructive. In July 2023 `serde_derive` 1.0.172 began shipping a precompiled macro binary for one target with no way to build from source. [Issue #2538](https://github.com/serde-rs/serde/issues/2538) ran to ninety-nine comments. Fedora could not redistribute a binary it had not built. Others could not audit it. The supply-chain objection was that a compromised maintainer account would ship a binary nobody could inspect. On 21 August 2023, 1.0.184 restored the source build with a release note that reads: "eventually we'd like to use a first-class precompiled macro if such a thing becomes supported by cargo / crates.io". The maintainer's closing comment asked for a Cargo or crates.io RFC on first-class precompiled artifacts.

The lesson is not that Rust developers refuse compiled artifacts. It is that they refuse artifacts that arrive without a source of record, without a verifiable link from artifact to source, without a builder identity, and without a way to say no. The same maintainer's [watt](https://github.com/dtolnay/watt) project supplied the sandboxing half of a proper answer, compiling proc-macros to WebAssembly so a macro can only consume and produce tokens. The rest is what a registry must supply: the source Loaf is always the publication of record, every asset is attested to its source digest and toolchain and names its builder, trust policy is per builder kind and belongs to the consuming project, and declining an asset costs nothing but the bake everyone does today.

### The closest precedent is C++, not Java

Java, Python, and Nix are the familiar examples, but they each had something Rust lacks: a stable bytecode, a stable C ABI at the boundary, or a sandbox that makes every input explicit. The ecosystem that shares Rust's actual problem is C++, and it solved it anyway. [Conan](https://docs.conan.io/2/reference/binary_model/settings_and_options.html) computes a `package_id` from compiler, version, architecture, options, and dependency versions; one recipe has many binaries; a consumer's profile selects a matching one, and `--build=missing` falls back to source. That is the receipt-and-plan match this paper proposes, in production for a decade in an ecosystem with no stable ABI. Within Rust, [crate2nix](https://github.com/nix-community/crate2nix) already does per-crate derivations that are "shared across projects and further can be passed to the binary cache", which is the same idea with Nix as the receipt. The Bytecode Alliance's [warg](https://github.com/bytecodealliance/registry) brought content addressing and a transparency log to a registry of compiled WebAssembly components.

What none of the partial answers has is an identity for a compiled unit precise enough to trust from a stranger, produced by the build system itself rather than inferred afterwards. That is the missing piece, and it is worth stating as a requirement before proposing anything that claims to meet it.

## Why the existing answers fall short

Rust is not short of attempts. Each one works, and each one stops at the same place, which is the most useful thing about them: the boundary they share is the shape of what is missing.

**[sccache](https://github.com/mozilla/sccache)** shares compilation results across a team's machines and is genuinely effective there. It reconstructs a cache key from compiler invocation inputs rather than receiving one from the build system, so it is conservative by necessity, and it is deployed within an organisation that already trusts itself. It distributes nothing publicly, because a key you reconstruct locally is not a claim a stranger can verify.

**[cargo-chef](https://github.com/LukeMathWalker/cargo-chef)** makes Docker layer caching work for Rust by separating dependency builds from application builds. It is a workaround for a container-layer model, and its unit is the layer: change one dependency and the layer rebuilds entirely. Nothing crosses between images, projects, or organisations.

**Cargo's [cross-workspace cache](https://goals.rust-lang.org/2026/cargo-cross-workspace-cache.html)**, an official 2026 project goal, is the most direct answer and the one this paper agrees with most. The target directory was split into artifact and build directories in 2025, and a cache lands on nightly in 2026 — conservative at first, excluding build scripts and proc-macros. The goal text notes it "could be extended in the future to be pre-populated from a remote cache for CI usecases". That is the local half of the problem, funded and staffed. The remote half needs something the local half does not: an identity a stranger's machine can check.

**[Nix](https://nixos.org) and [crate2nix](https://github.com/nix-community/crate2nix)** solve it properly and have for years. Per-crate derivations are shared through a binary cache, which is exactly the mechanism this paper argues for. The cost is that Nix requires the whole ecosystem to adopt Nix: its guarantee comes from controlling every input, including ones Cargo deliberately leaves to the consumer. It is a correct answer that most Rust projects will not adopt, which makes it evidence that the approach works rather than a path the ecosystem can take.

**[cargo-binstall](https://github.com/cargo-bins/cargo-binstall)** and cargo-quickinstall distribute finished binaries and are widely used. They serve end-user tools, not dependencies. A binary has no consumer-chosen configuration to match — nobody links against `ripgrep` with different features — so the identity problem never arises, which is precisely why they could ship and a dependency tier could not.

**[crABI](https://github.com/rust-lang/rust/pull/105586)** and the [rlib-stabilisation pre-RFC](https://internals.rust-lang.org/t/pre-rfc-stabilize-a-version-of-the-rlib-format/17558)** attack the problem from the other end, by making artifacts compatible across more compilations. That reduces how often identities differ; it does not remove the need to know when they do.

Reading them against the requirements below makes the pattern exact:

| Approach | Identity | Applicability | Public exchange | What stops it |
| --- | --- | --- | --- | --- |
| sccache | reconstructed locally | conservative | no | a key you rebuild yourself is not a claim a stranger can check |
| cargo-chef | layer-shaped | coarse | no | the unit is the image layer, not the artifact |
| Cargo cross-workspace cache | local | yes | not yet | the remote half needs an identity a stranger's machine can verify |
| Nix, crate2nix | complete | yes | yes | requires the whole ecosystem to adopt Nix |
| cargo-binstall | not required | not required | yes | end-user binaries have no consumer-chosen configuration to match |
| crABI, rlib stabilisation | — | — | — | widens compatibility; does not decide when artifacts differ |

Every approach either restricts the configuration space until matching becomes trivial, or restricts the trust boundary until verification becomes unnecessary. Nix is the exception that proves it: it satisfies every requirement by controlling every input, which is precisely the price most Rust projects decline to pay. None of them makes a compiled artifact something a stranger can offer and an ordinary Cargo-shaped consumer can check.

## What any solution must provide

Before naming any implementation, it is worth stating what the ecosystem's own objections imply. These are not requirements chosen to suit a particular design; they are what the internals threads, the serde episode, and the Cargo roadmap have collectively been asking for since 2015. A reader should be able to evaluate any proposal against them, including this one.

1. **Unit identity.** A digest over every input that can change the output, including the identities of dependencies, so that two artifacts are comparable at all. The Cargo maintainer's second objection in the 2025 internals thread was precisely this: Cargo would need a real model of the relationship between build environment and ABI before it could trust an artifact. Nothing else on this list is reachable without it.

2. **Deterministic applicability.** A consumer must decide locally, and without ambiguity, whether an artifact matches what it would otherwise have built. A near match must be a miss. This is not fastidiousness. Rust has no stable ABI across compiler versions, and two almost-identical instances of one library reaching a single link can produce a link incompatibility, a type mismatch, duplicated global state, or — worst — a binary that builds cleanly and fails only at runtime. The last case is why "close enough" is the one answer the ecosystem cannot accept: the failure outlives the build that caused it.

3. **Source transparency.** Every artifact has a source of record that anyone can read, audit, rebuild and redistribute. The 2023 serde episode settled this and settled it emphatically: Fedora could not redistribute a binary it had not built, others could not audit it, and the objection that carried was that a compromised account could ship something nobody could inspect. Ninety-nine comments later the source build was restored. Any proposal that reopens this question has already failed.

4. **Provenance.** An artifact names who produced it, bound to the source it came from, so a consumer reasons about trust instead of assuming it. This is the half of the serde objection that survived after transparency: not only *can I read the source*, but *who turned it into this*.

5. **Trust policy belonging to the consumer.** The consuming project decides which producers it accepts. The same episode showed why: the ecosystem's objection was never to compiled artifacts as such, it was to having one imposed with no way to decline.

6. **Graceful fallback.** A miss, a refusal, or an unavailable artifact degrades to compiling from source. No workflow may depend on an artifact existing — which is also what keeps requirement 5 honest, since declining is only free if the fallback is ordinary.

7. **Configuration sovereignty.** The consumer keeps control of features, profiles, targets, and dependency versions. This was the Cargo maintainer's first objection: precompiled packages lock in profile settings, compiler flags, and dependency versions, taking control away from the top-level package that Cargo's model deliberately puts in charge. An artifact tier must not quietly narrow what a project may choose.

8. **No ecosystem fork.** crates.io remains the source of record, unmirrored and unreplaced, and a Rust developer who never adopts any of this is unaffected. An answer that requires the ecosystem to move is not an answer to this problem; it is a different ecosystem, which is the honest reading of why Nix has not absorbed Rust packaging despite solving the technical problem years ago.

Requirements 1 and 2 are the ones nothing in the Rust ecosystem currently supplies, and they are the reason the rest have never been reachable. Requirements 3 through 8 are, in effect, the conditions the ecosystem has already attached to any answer.

## What a receipt records, and when two are interchangeable

Requirements 1 and 2 are the hard ones, and the rest are unreachable without them. So the question for any candidate is narrow: is its identity precise enough to decide interchangeability?

Oven is one answer, and what follows is only the part that bears on that question. How Oven plans, bakes, seals and reuses work is the subject of its own paper, [A Cargo-free toolchain for Incan and Rust](incan_oven_positioning.md); nothing here depends on reading it first. What matters for this argument is the receipt, stated once, in full.

A receipt identifies a compiled unit by a digest over its *effective compilation inputs*:

- the semantic content of the source it compiles;
- the manifest facts the compilation actually observes, such as a version string reaching code through an environment macro;
- the toolchain identity;
- the target triple, and whether the unit was built for the host or the target;
- the profile facts that change codegen — optimisation level, debug-info level, codegen units, panic strategy, LTO mode, target features;
- the resolved feature set;
- the artifact kind and edition;
- the identities of every unit it links against;
- the receipt identity of any build script or procedural macro whose output it consumed;
- the identities of the native toolchain inputs and external artifacts the unit observes, including linked libraries and included headers.

That last item is the one most easily underestimated. A build script driving a C or C++ compile observes far more than the headers it ends up including: the compiler and its version, the flags, the sysroot and SDK, the search paths, and any generated output all reach the result. Defining that closure precisely is RFC 124's work, and this paper names it rather than pretending it is easy.

Equally important is what a receipt must *exclude*, because each of these breaks sharing without changing output: absolute paths, timestamps, hostnames, user names, publication signatures, and which plan requested the unit. Source paths are remapped so the same source at two locations yields the same identity.

Every `*.loaf` Oven seals carries one, and Oven already refuses to reuse an artifact whose receipt does not match the plan. The project's own engineering notes record the failure that rule prevents: two independently resolved closures of identical source produced ABI-incompatible rlibs, and linking them left tokio unable to find its own runtime. That is requirement 2 failing in the small, caught locally. The claim this paper makes is only that the same identity can then be exchanged across machines — a registry does not need to invent it, it needs to distribute artifacts that already carry it.

Three different questions are hiding in the word "same", and an exchange between strangers only works if they stay apart.

**Unit identity answers "was this built for my plan?"** This is the named concept the rest of the paper leans on: a *unit identity* is the digest over effective compilation inputs described above, and it is what RFC 124 calls it too, so the paper and the specification cannot drift apart. Two units are applicable to one another when their unit identities are equal. Same package and version with a different identity is a miss, and a miss means baking from source. Nothing is substituted on a near match, and no ownership declaration waives it.

**Payload digest answers "are these the exact bytes that were attested?"** A consumer verifies what it downloaded against the digest in the attestation covering it. This is integrity, and it is independent of the first question.

**Publication provenance answers "whose claim that these bytes follow from that source am I accepting?"** A matching unit identity proves the inputs were right, not that the producer was honest: a malicious builder can take correct inputs and attest malicious output. Trust is a separate judgement, and it belongs to the consuming project.

Keeping them apart matters because collapsing the first two would make reproducible builds a precondition for exchange, and `rustc` output is not bit-for-bit reproducible in every configuration. A consumer does not need a stranger's artifact to equal the bytes it would have produced. It needs the artifact's unit identity to match its own plan, and the bytes it received to match the payload digest the builder attested. Where reproducible rebuilds *are* available they supply a stronger check on top, by letting a third party confirm the transformation rather than trust it.

The whole architecture follows from that separation, and it compresses to five sentences. Unit identity determines applicability. Payload digest determines integrity. Provenance determines trust. Source transparency preserves auditability. Fallback preserves sovereignty.

What the matching rule prevents is not a slow build but a wrong one. Two ABI-incompatible instances of one library reaching a single link can yield a link error, a type incompatibility, duplicated global state, or a binary whose failure appears only at runtime — and the last of those is the one that makes "close enough" unusable as a policy.

Because the identities of dependencies are themselves inputs, an identity is a Merkle root over the whole closure: nothing can change beneath a unit without changing the unit.

The source input is a semantic digest rather than a hash of file bytes, so where equivalence can be established, changes that the compilation cannot observe — a comment, a docstring, a relocation under path remapping — need not invalidate a unit or anything downstream of it. The boundary is what compilation can observe, and it is deliberately conservative: anything a macro, a build script, an `include!`, an environment lookup, or a source-location-sensitive expansion can see remains an identity input. Establishing that boundary for a given language surface is work, not a property that falls out of hashing; RFC 124 and RFC 106 define where it currently holds.

This is a design, not a proof. It encodes the inputs Cargo lacks when it has to decide whether compiled artifacts are interchangeable; it does not demonstrate that no input has been overlooked. That distinction matters, and the honest claim is the narrower one.

## What changes with Oven and `incan.pub`

The shape, in the terms a user sees:

**The unit of publication is the Loaf.** A project with a `loaf.toml` publishes as one thing, whether its facets are Incan, Rust, or both. A Rust-only project that adopted Loaf publishes exactly like an Incan library; the registry does not care which. This is what makes the registry serve Rust and not only Incan.

**Two tiers, one identity chain.** The publisher signs and uploads a source Loaf, the publication of record. Alongside it travel baked assets: the target-bound outputs Oven produced, each with its receipt and an attestation binding the asset digest to the source digest and the toolchain, and naming who built it. An asset is applicable only when its unit identity equals the unit identity the consumer's plan requires. A near match is a miss, and a miss means baking from source, exactly as today. Nothing is ever substituted.

**The consumer does not choose.** `oven bake` computes the plan, asks the index which assets exist for exactly that plan, downloads and verifies the ones that match, and bakes the rest. For the DataFusion probe on the consumer machine, that is a 195 MB verified download standing in for seven minutes of a frozen desktop and 1.6 GB of scratch, on the first use and on every subsequent clean checkout, runner, and image.

**Who bakes is a policy, not a mystery.** Assets can be built in the publisher's trusted CI, by a registry bakery over the signed source, or on the publishing machine, and each carries a distinct attestation so a project's trust policy can accept some kinds and not others. Publisher-built assets with verifiable CI provenance offer a strong publisher-authority path; registry-built assets can provide broader and more uniform coverage; local assets exist so that publishing never requires infrastructure. Which of those a project accepts is its own call — a transparency log records what happened and who asserted it, which is not the same as making the producer trustworthy.

**The store is the local end of the same exchange.** Verified reuse of sealed work is already demonstrable: a second build of the same project reuses a completed Loaf whose source, dependency, semantic, and interop facts were verified and sealed at bake time. That is the identity chain working, and it is precisely what every attempt to add compiled artifacts to Cargo since 2015 lacked.

What that does not yet do is share *below* the plan. Two projects with identical dependency closures each sealed their own 51 MB Loaf holding the same fourteen rlibs, and twenty build directories under one development root totalled 52 GB. Both are the same defect: the plan, not the compiled unit, is the unit of reuse. Making the compiled unit the store's addressable object — keyed by an identity over its effective compilation inputs, kept distinct from where its source was published — shares units across projects and worktrees, lets a version-only republication of unchanged code still hit, and makes the store collectable by reachability. A registry then exchanges those same identities across machines rather than inventing a second notion of sameness.

**crates.io is consumed, not replaced.** `crate` dependencies resolve against the crates.io sparse index through the same client, verification, and store as `incan.pub` Loaves. The registry never mirrors crates.io source and never publishes to it. Where it can help Rust users directly is by publishing attested, registry-built assets for popular crates at the closures a toolchain pins, so that consuming DataFusion through Oven is a download even though DataFusion itself lives on crates.io.

**The Incan toolchain is the first customer.** The standard library's components and the base Loaf families the compiler needs become ordinary assets in the index, keyed by toolchain and target. A hello-world project stops compiling tokio and serde_json because the toolchain already published them; the web stack stops being sealed into every user's base Loaf because it is a separate asset that only web projects match. Standard library fixes ship as registry publications without a compiler release.

The client and trust design borrows deliberately. From uv: one HTTPS client with pure-Rust TLS and an opt-in for the platform certificate store, a content-addressed cache that installs by linking rather than copying, one universal lockfile valid on every host, and the discipline that resolution never executes package code. From npm's history: content addressing with integrity on every entry, keyless signing with a public transparency log and trusted publishing from CI, scoped names so that a Rust crate and an `incan.pub` Loaf can share a short name without colliding, root-only overrides, and a prompt before running anything not already local. From Cargo: the index and the lockfile themselves.

## The fragmentation objection

The reasonable objection from a Rust reader is that a second registry splits the ecosystem. The answer has to be direct.

`incan.pub` publishes nothing to crates.io and presents nothing as a crates.io package; RFC 119 rules that out. It does not mirror crates.io source. It does not change how a Cargo project resolves anything, because a Cargo project never talks to it; only a project that has adopted `loaf.toml` does, and adoption is explicit. Its community namespace is scoped, so `acme/regex` on `incan.pub` and `regex` on crates.io are different names in different registries selected by different dependency keys. A Rust library author who never hears of Incan is unaffected, and one who publishes a Loaf keeps publishing the crate.

What the tier adds is the same thing a Nix binary cache adds to nixpkgs or a wheel adds to an sdist: a compiled, attested form of something whose source of truth stays where it was. The ecosystem is not split when an artifact tier is layered over a source tier; it is split when two source tiers compete, and this design has exactly one.

## What this proposal does not claim

- It does not claim a measured speedup from the asset path. The measurements above are the cost of the source path; the asset path does not exist yet to be measured.
- It does not claim that every dependency can be precompiled for every target, toolchain, and feature closure. The index never promises completeness; a miss is a bake, not a failure, and small crates may never have assets at all.
- It does not turn `incan.pub` into a binary-only channel. The source Loaf is the publication of record, is always retained, and is what every asset is attested against.
- It does not give the registry authority over a project. Trust policy per builder kind, registered sources, and overrides belong to the consuming project, as RFC 117 already requires.
- It does not decide who bakes. Whether a registry bakery arrives with the first release or after publisher-built assets is deliberately left open.
- It does not claim the numbers are stable. They are a snapshot of two machines, three libraries, and one toolchain on one day, reproducible from the scripts that produced them, and they will drift.

## Where the contract lives

RFC 124 defines unit identity, cross-plan sharing in the store, and collection; the registry depends on it. RFC 125 defines the registry: package identity and scoping, the two artifact tiers, the index and asset manifest, signing and trusted publishing, registry events, fetching and verification, crates.io consumption, the web surface, and mirrors. RFC 117 defines `loaf.toml`, `oven.lock`, and registry configuration. RFC 118 defines `oven publish` and the other command surfaces. RFC 119 defines the Rust facet that lets Rust-only projects take part and the host and target domains that make assets comparable. RFC 123 defines what an Incan package publishes so that its exports are executable by any consumer. RFC 079 describes the wider artifact graph the registry grows into. The Oven positioning paper describes the toolchain that produces the artifacts this paper wants to distribute. [The Oven ecosystem](../tooling/explanation/oven_ecosystem.md) maps these components, the Loaf they produce, and the RFC that owns each.

## Closing

Rust made compiled distribution possible by making its source ecosystem trustworthy, and then never built it. The cost of that omission is measurable: seven minutes of a saturated consumer machine to open a query engine session, half a terabyte of indistinguishable duplicates on a maintainer's laptop, and the same work repeated on every runner and in every image. The Rust project has been asked for this since 2015 and is now building the local half of it; the fix was never only a faster cache. It was a unit identity precise enough to trust from a stranger, and a place to exchange artifacts that carry it. Rust already knows how to describe source, and describes it better than most. What it still lacks is a way to describe compiled work precisely enough that strangers can safely exchange it. That is the missing layer, and it is a smaller thing than a package manager and a larger thing than a cache.

Oven's receipt model is built to supply that description. An exchange built on those identities distributes the result.

crates.io goes on exchanging recipes, because the recipe remains the source of truth and should. What changes is that the recipe stops being the only thing that can travel. Once one kitchen can state exactly which recipe, ingredients, and oven it used, and another can confirm those are exactly the ones its own order requires, there is no reason for both to bake. What is left is not a technical question but the old one: whose word you take.
