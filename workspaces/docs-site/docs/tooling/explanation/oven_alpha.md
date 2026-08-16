---
title: Oven Alpha
hide:
  - toc
  - navigation
---

<!-- markdownlint-disable MD033 MD060 -->

<section class="inc-oven-hero" aria-labelledby="oven-hero-title" markdown="1">

<div class="inc-oven-hero__copy" markdown="1">

<p class="inc-oven-hero__kicker">A better build system</p>

<h1 id="oven-hero-title"><span>Build once.</span><span>Keep the proof.</span></h1>

<p class="inc-oven-hero__lead">Oven turns a verified Rust compatibility closure into immutable Loafs, then lets normal Incan build, run, and test commands reuse direct-<code>rustc</code> plans without Cargo on the consumer path.</p>

<div class="inc-oven-hero__actions" markdown="1">
[Why Oven exists](#why-oven-exists){ .md-button .md-button--primary }
[See the Alpha flow](#how-oven-builds-and-proves){ .md-button }
[Read the architecture](../../whitepapers/incan_oven_positioning.md){ .md-button }
[Run the compiler suite](../../contributing/how-to/oven_alpha_benchmark.md){ .md-button }
</div>

<p class="inc-oven-hero__truth"><strong>Alpha means explicit boundaries.</strong> A compatible Loaf is reproducible by the same toolchain and inputs.</p>

</div>

<div class="inc-oven-hero__image" role="img" aria-label="A cybernetic alpaca baker tending a glowing oven in a mountain workshop"></div>

</section>

<nav class="inc-oven-local-nav" aria-label="Oven page sections">
<span class="inc-oven-local-nav__label">Explore Oven</span>
<span class="inc-oven-local-nav__links">
<a href="#oven-hero-title">Overview</a>
<a href="#how-oven-builds-and-proves">Architecture</a>
<a href="#normal-commands">Commands</a>
<a href="#evidence-you-can-inspect">Evidence</a>
<a href="#current-alpha-boundary">Alpha boundary</a>
<a href="../../reference/cli_reference/">CLI reference ↗</a>
</span>
</nav>

<section class="inc-oven-route" aria-labelledby="oven-route-title" markdown="1">

<div class="inc-oven-section-heading" markdown="1">
<h2 id="oven-route-title">One system, three ways in.</h2>
<p>Use Oven through the surface that matches the question you are asking.</p>
</div>

<div class="inc-oven-route__grid">

<a class="inc-oven-route__card inc-oven-route__card--developer" href="#normal-commands">
<span class="inc-oven-route__icon"><img src="../../../shared/incapunk/icons/terminal.svg" alt="" aria-hidden="true"></span>
<span class="inc-oven-route__number">CLI</span>
<strong>Command line</strong>
<span>Build, run, and test through the normal Incan workflow.</span>
<em><code>incan build</code> →</em>
</a>

<a class="inc-oven-route__card inc-oven-route__card--maintainer" href="#how-oven-builds-and-proves">
<span class="inc-oven-route__icon"><img src="../../../shared/incapunk/icons/code.svg" alt="" aria-hidden="true"></span>
<span class="inc-oven-route__number">MODEL</span>
<strong>Programmatic surface</strong>
<span>See the typed compiler-facing contracts behind every plan.</span>
<em>Read the architecture →</em>
</a>

<a class="inc-oven-route__card inc-oven-route__card--operator" href="#evidence-you-can-inspect">
<span class="inc-oven-route__icon"><img src="../../../shared/incapunk/icons/eye.svg" alt="" aria-hidden="true"></span>
<span class="inc-oven-route__number">PROOF</span>
<strong>Inspect the proof</strong>
<span>Explore stored evidence and the reason a plan was reused or rejected.</span>
<em><code>incan oven store inspect</code> →</em>
</a>

</div>

</section>

<section id="why-oven-exists" class="inc-oven-thesis" aria-labelledby="oven-thesis-title">
<header>
<span>Why Oven exists</span>
<h2 id="oven-thesis-title">The build result should explain itself.</h2>
<p>Cargo can compile Incan's generated Rust. The limitation is the contract around that result: a mutable target tree and local fingerprints do not give Incan one durable answer for what was built, why it is compatible, or when it may be reused.</p>
</header>
<div class="inc-oven-thesis__grid">
<article><span>01 · Artifact</span><strong>A Loaf is not a cache guess.</strong><p>Its identity binds source, dependency lock, compiler, SDK, target, profile, features, artifacts, and integrity evidence.</p></article>
<article><span>02 · Boundary</span><strong>Prepare once. Consume deliberately.</strong><p>A publisher may pay the compatibility cost once. Normal commands only select and consume a verified result; a miss stays explicit.</p></article>
<article><span>03 · Lifecycle</span><strong>Retention is part of correctness.</strong><p>Bounded admission, atomic publication, and active leases keep reuse predictable under concurrency, crashes, and cleanup.</p></article>
</div>
<p class="inc-oven-thesis__conclusion"><strong>The innovation is not merely calling <code>rustc</code> directly.</strong> Oven makes a prepared compatibility closure a first-class Incan artifact—with an identity, a receipt, and an owned lifecycle.</p>
</section>

<section id="normal-commands" class="inc-oven-mechanic inc-oven-mechanic--commands" aria-labelledby="oven-commands-title">
<header>
<span>Use · the normal workflow</span>
<h2 id="oven-commands-title">Oven sits behind the commands you already know.</h2>
<p>For a supported project, <code>build</code>, <code>run</code>, and <code>test</code> select a receipt-compatible Loaf and its stored direct-<code>rustc</code> plan.</p>
</header>
<div class="inc-oven-command-stack" aria-label="Normal Incan commands">
<code><span>$</span> incan build</code>
<code><span>$</span> incan run</code>
<code><span>$</span> incan test</code>
</div>
<div class="inc-oven-mechanic__explain">
<strong>Compatibility is selected, not guessed.</strong>
<p>Compiler, target, SDK, feature selection, dependency lock, and relevant source all contribute to identity. Change one and Oven prepares or refuses; match them and Oven can reuse the sealed result without Cargo on the consumer path.</p>
<p class="inc-oven-mechanic__prep"><b>Make first materialization explicit</b><code>incan oven bake --project . --format json</code><code>incan build --lib</code></p>
<details class="inc-oven-detail">
<summary>Maintainer and low-level commands</summary>
<p><code>oven bake</code> can materialize a compatible Loaf already shipped by the active toolchain. It does not compile a final <code>rlib</code>, resolve a Rust graph, or launch Cargo. If no compatible Loaf exists, the normal command stops explicitly.</p>
<p><code>incan oven store inspect --format json</code> reads the store; <code>incan oven store prune --dry-run --format json</code> previews bounded reclamation. <code>bake-loafs</code> remains the maintainer-only publisher path, never a developer fallback.</p>
</details>
</div>
</section>

<aside class="inc-oven-manifesto" aria-label="Oven operating principle">
<span aria-hidden="true">“</span>
<div><strong>Reuse the proof—or explain the miss.</strong><p>Deterministic inputs. Immutable Loafs. Verifiable outcomes.</p></div>
<img src="../../../shared/incapunk/incus-library/hint/incus_hint_magnifying_glass.webp" alt="" aria-hidden="true">
</aside>

## How Oven builds and proves

The publisher pays the compatibility cost once. Oven fingerprints the inputs that made the result valid, seals the result and its receipt into one immutable Loaf, then lets a compatible consumer reuse it without Cargo. If those facts no longer match, Oven refuses reuse and tells you why. Alpha ships two typed compatibility envelopes: the release foundations distributed with the toolchain and the compiler-suite foundations used to test Incan itself.

<section class="inc-oven-architecture" aria-label="Oven publisher, Loaf, and consumer architecture">

<ol class="inc-oven-proof-story" aria-label="How Oven turns publisher inputs into reusable proof">
<li><span>01–02 · Publisher</span><strong>Identify and prepare once.</strong><p>Fingerprint source, lock, compiler, SDK, target, profile, and build intent before paying the compatibility cost.</p></li>
<li><span>03 · Seal</span><strong>Keep the result and its explanation.</strong><p>Package the immutable artifact, direct-<code>rustc</code> plan, compatibility identity, and build receipt as one Loaf.</p></li>
<li><span>04–05 · Consumer</span><strong>Reuse—or get a precise refusal.</strong><p>Match the requested environment, reuse locally without Cargo, and inspect the evidence behind the decision.</p></li>
</ol>

<div class="inc-oven-architecture__board">
<img class="inc-oven-architecture__connectors inc-oven-architecture__connectors--publisher" src="../../../shared/incapunk/oven_architecture_connectors_publisher_v1.png" alt="" aria-hidden="true">
<img class="inc-oven-architecture__connectors inc-oven-architecture__connectors--consumer" src="../../../shared/incapunk/oven_architecture_connectors_consumer_v1.png" alt="" aria-hidden="true">
<div class="inc-oven-architecture__inputs">
<p>Publisher inputs</p>
<div><img src="../../../shared/incapunk/icons/file-code.svg" alt="" aria-hidden="true"><span><strong>Source</strong><small><code>main.incn</code></small></span></div>
<div><img src="../../../shared/incapunk/icons/shield-check.svg" alt="" aria-hidden="true"><span><strong>Dependency lock</strong><small><code>lock.json</code></small></span></div>
<div><img src="../../../shared/incapunk/icons/workflow.svg" alt="" aria-hidden="true"><span><strong>SDK + toolchain</strong><small>compiler · target · profile</small></span></div>
</div>

<figure class="inc-oven-loaf">
<img src="../../../shared/incapunk/oven_loaf_evidence_002.png" alt="A sealed bread Loaf carrying the Oven stamp">
<figcaption><strong>LOAF</strong><span><b>L</b>ocked, <b>O</b>bservable <b>A</b>rtifact <b>F</b>ormat</span><small>Immutable · inspectable · reusable</small></figcaption>
</figure>

<div class="inc-oven-architecture__outcomes">
<p>Consumer outcomes</p>
<div><img src="../../../shared/incapunk/icons/shield-check.svg" alt="" aria-hidden="true"><span><strong>Compatibility check</strong><small>Inputs match the environment</small></span></div>
<div><img src="../../../shared/incapunk/icons/package.svg" alt="" aria-hidden="true"><span><strong>Local reuse</strong><small>Run and test without Cargo</small></span></div>
<div><img src="../../../shared/incapunk/icons/rocket.svg" alt="" aria-hidden="true"><span><strong>Artifacts</strong><small>Native binary + files</small></span></div>
<div><img src="../../../shared/incapunk/icons/file-code.svg" alt="" aria-hidden="true"><span><strong>Receipt</strong><small>Why this result exists</small></span></div>
<div><img src="../../../shared/incapunk/icons/eye.svg" alt="" aria-hidden="true"><span><strong>Inspect</strong><small>Explore the evidence</small></span></div>
</div>

<aside class="inc-oven-inspector" aria-label="Example fields in an Oven evidence receipt">
<p><span></span>Live inspector</p>
<dl>
<div><dt>Compiler</dt><dd>incan 0.5</dd></div>
<div><dt>Source</dt><dd>SHA256: 3f2a…</dd></div>
<div><dt>Lock</dt><dd>SHA256: 8b12…</dd></div>
<div><dt>Artifact</dt><dd>SHA256: 7cfb…</dd></div>
<div><dt>Target</dt><dd>x86_64-linux</dd></div>
<div><dt>Receipt</dt><dd>rec_0f9c…</dd></div>
</dl>
<a href="#evidence-you-can-inspect">Open the evidence fields →</a>
</aside>
</div>

<div class="inc-oven-flow__boundary">
<div><span>What the proof remembers</span><strong>One receipt answers: “Why does this artifact exist?”</strong><p>Every Loaf is bound to its source, dependency lock, compiler, SDK, target, profile, and build intent. The proof travels with the result instead of disappearing into a build cache.</p></div>
<div><span>Why Oven refuses reuse</span><strong>A miss protects the guarantee.</strong><p>Oven rebuilds or stops when the requested environment cannot honestly reuse the sealed result.</p><ul><li>the toolchain or target differs;</li><li>dependency or source evidence changed; or</li><li>the request sits outside the Alpha envelope.</li></ul></div>
</div>

<section id="evidence-you-can-inspect" class="inc-oven-evidence" aria-labelledby="oven-evidence-title">
<header>
<span>Inspect · the reason</span>
<h2 id="oven-evidence-title">The receipt separates what happened from why reuse was allowed.</h2>
<p>Oven keeps preparation, execution, compatibility, and storage evidence distinct. The result is a proof you can inspect instead of a cache entry you have to reverse-engineer.</p>
</header>
<div class="inc-oven-evidence__grid">
<article><img src="../../../shared/incapunk/icons/workflow.svg" alt="" aria-hidden="true"><span><b>Selection</b><strong>Which Loaf matched?</strong><small>Source, lock, compiler, SDK, target, profile, features, and build intent.</small></span></article>
<article><img src="../../../shared/incapunk/icons/rocket.svg" alt="" aria-hidden="true"><span><b>Execution</b><strong>What did the replay prove?</strong><small>Prepared roots, passed and failed tests, artifacts, and phase timings remain separate.</small></span></article>
<article><img src="../../../shared/incapunk/icons/database-arrow-right.svg" alt="" aria-hidden="true"><span><b>Storage</b><strong>What can be reclaimed?</strong><small>Logical, physical, owned, reclaimable, and active-lease bytes are reported independently.</small></span></article>
</div>
<details class="inc-oven-detail inc-oven-detail--evidence">
<summary>Evidence fields, limits, and benchmark rules</summary>
<p>Do not combine cold publication and prepared replay into one headline number. The approximately-five-minute acceptance target applies to the prepared full-suite replay and is meaningful only with its commit, machine, toolchain, cache state, workload, and storage junctions.</p>
<div class="inc-oven-evidence__fields" role="table" aria-label="Oven storage evidence fields">
<div role="row"><strong role="rowheader">Logical artifact bytes</strong><span role="cell">Declared immutable payload lengths.</span></div>
<div role="row"><strong role="rowheader">Policy physical bytes</strong><span role="cell">Filesystem allocation charged by Oven policy.</span></div>
<div role="row"><strong role="rowheader">Owned bytes</strong><span role="cell">Allocation owned by the current envelope.</span></div>
<div role="row"><strong role="rowheader">Reclaimable bytes</strong><span role="cell">Inactive allocation policy may safely remove.</span></div>
<div role="row"><strong role="rowheader">Active-lease bytes</strong><span role="cell">Allocation protected by running consumers.</span></div>
</div>
<p>The developer store defaults to <code>$INCAN_HOME/oven/store/v1</code> (or <code>~/.incan/oven/store/v1</code>) with bounded aggregate and per-domain allowances. Oven may reclaim least-recently-used inactive entries, never an active lease, and never silently expands an operator-supplied limit.</p>
<a href="../../contributing/how-to/oven_alpha_benchmark.md">Run the reproducible benchmark sequence →</a>
</details>
</section>

</section>

<section class="inc-oven-cargo" aria-labelledby="oven-cargo-title">
<div class="inc-oven-cargo__compare">
<p>Why Oven beats Cargo-only workflows</p>
<h2 id="oven-cargo-title">One build. A reusable result with receipts.</h2>
<div class="inc-oven-cargo__intro">Cargo is a compatibility publisher inside today's Alpha boundary, not Oven's consumer build path. Oven owns the artifact contract: one reusable result, one compatibility identity, and one inspectable explanation.</div>
<div class="inc-oven-cargo__table" role="table" aria-label="Cargo-only and Oven Alpha comparison">
<div role="row"><span role="columnheader">Cargo-centered Incan</span><b role="columnheader">Concern</b><span role="columnheader">Oven Alpha</span></div>
<div role="row"><span role="cell">Each environment resolves and compiles its own generated graph.</span><b role="cell">Process</b><span role="cell">A publisher prepares the supported closure once.</span></div>
<div role="row"><span role="cell">Target artifacts and mutable build state.</span><b role="cell">Output</b><span role="cell">Immutable Loaf + build receipt.</span></div>
<div role="row"><span role="cell">Local target cache and Cargo fingerprints.</span><b role="cell">Reuse</b><span role="cell">Explicit compatibility identity.</span></div>
<div role="row"><span role="cell">Cargo explains build activity, not one Incan artifact contract.</span><b role="cell">Evidence</b><span role="cell">One inspectable proof spans selection, execution, and storage.</span></div>
</div>
</div>

<div class="inc-oven-capabilities" aria-label="Oven Alpha capabilities available now">
<p>Alpha · available now</p>
<strong>Useful proof today</strong>
<span>The current surface is deliberately narrow, but the core reuse contract is real.</span>
<ul><li>Deterministic builds</li><li>Immutable Loaf artifacts</li><li>Human-readable inspection</li><li>No Cargo on the consumer path</li></ul>
<a href="#normal-commands">Get started with Oven →</a>
</div>

<div class="inc-oven-north-star" aria-label="Planned Oven capabilities">
<p>North star · planned</p>
<strong>Proof that can travel</strong>
<span>These layers explain where Oven is going; they are not presented as shipped Alpha capabilities.</span>
<ul><li>Trust and signatures</li><li>SBOM and broader provenance</li><li>Portable publication</li><li>Verification chains</li></ul>
<small>Planned capabilities. Not available in Alpha.</small>
</div>
</section>

<section class="inc-oven-decision" aria-labelledby="oven-decision-title">
<header>
<span>Decide · the compatibility boundary</span>
<h2 id="oven-decision-title">A matching receipt permits reuse. A mismatch stops it.</h2>
<p>Oven does not infer that yesterday's artifact is “probably fine.” It compares the requested environment with the identity sealed into the Loaf, then returns an explicit reuse decision.</p>
</header>
<div class="inc-oven-decision__cards">
<article><img src="../../../shared/incapunk/icons/shield-check.svg" alt="" aria-hidden="true"><span><b>Match</b><strong>Reuse the sealed Loaf.</strong><small>The same source, lock, compiler, SDK, target, profile, features, and build intent select the stored direct-<code>rustc</code> plan.</small></span></article>
<article><img src="../../../shared/incapunk/icons/eye.svg" alt="" aria-hidden="true"><span><b>Miss</b><strong>Rebuild or stop with a reason.</strong><small>A changed compatibility fact—or a request outside the Alpha envelope—cannot silently fall back to hidden Cargo work.</small></span></article>
</div>
<a href="../../whitepapers/incan_oven_positioning.md">Read the full architecture and ownership model →</a>
</section>

<section id="current-alpha-boundary" class="inc-oven-closing" aria-labelledby="oven-boundary-title">
<div class="inc-oven-closing__commands">
<p>Inspect the surface</p>
<code><span>$</span> incan oven store inspect</code>
<code><span>$</span> incan oven store prune --dry-run</code>
<code><span>$</span> incan oven bake --project .</code>
<code><span>$</span> incan inspect oven --receipt receipt.json</code>
<a href="../reference/cli_reference.md">Open the CLI reference →</a>
</div>
<div class="inc-oven-closing__boundary">
<p>Alpha boundary</p>
<h2 id="oven-boundary-title">Early by design. Explicit by default.</h2>
<span>Oven Alpha proves the maintained Incan workflow and the repository's own compiler suite. It does not yet claim:</span>
<ul><li>general Cargo compatibility for arbitrary Rust workspaces;</li><li>every build script, procedural macro, target, or platform dependency shape;</li><li>compressed or remotely distributed <code>.loaf</code> bundles;</li><li>the authored <code>Loaf.toml</code>, resolved <code>Oven.lock</code>, workspace, or registry model proposed for later work; or</li><li>broad ecosystem readiness from external-library bake-offs.</li></ul>
<small>Those belong to 0.6-and-later releases and RFC work. If the Alpha envelope cannot authorize a normal command, Oven explains the miss and stops.</small>
</div>
<div class="inc-oven-closing__visual" role="img" aria-label="Incus keeping watch beside the glowing Oven">
<span>Incus is observing.<br>Always.</span>
</div>
</section>

<p class="inc-oven-page-end">For the complete command surface, see the <a href="../reference/cli_reference.md">CLI reference</a>.</p>
