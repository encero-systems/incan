---
title: What's New
search:
  boost: 3
hide:
  - navigation
  - toc
tags:
  - release notes
  - changelog
  - releases
---

<!-- markdownlint-disable MD033 -->

<article class="inc-news" data-inc-news markdown="1">
<nav class="inc-news__releases" aria-label="Browse releases" id="releases">
<p class="inc-news__eyebrow">Releases</p>
<a class="inc-news__version inc-news__version--development" href="#release-0-6" data-news-release="0.6">0.6 <span>In development</span></a>
<a class="inc-news__version" href="#release-0-5" data-news-release="0.5">0.5<span>Released</span></a>
<a class="inc-news__version" href="#release-0-4" data-news-release="0.4">0.4<span>Released</span></a>
<a class="inc-news__version" href="#release-0-3" data-news-release="0.3">0.3<span>Released</span></a>
<a class="inc-news__version" href="#release-0-2" data-news-release="0.2">0.2<span>Released</span></a>
<a class="inc-news__version" href="#release-0-1" data-news-release="0.1">0.1<span>Released</span></a>
<a class="inc-news__archive" href="#full-release-notes">All release notes</a>
</nav>
<div class="inc-news__main" markdown="1">
<header class="inc-news__intro">
<h1 id="release-notes">What's new in Incan</h1>
<p>Explore the language and the tools behind each release.</p>
<a class="inc-news__full-link" href="#full-release-notes">Full release notes <img src="../shared/incapunk/icons/arrow-right.svg" alt=""></a>
</header>
<div class="inc-news__controls" data-news-controls hidden>
<div class="inc-news__views" role="group" aria-label="Focus of changes">
<button type="button" data-news-view="language" aria-pressed="true">Language</button>
<button type="button" data-news-view="toolchain" aria-pressed="false">Toolchain</button>
</div>
<p class="inc-news__view-description" data-news-description></p>
<div class="inc-news__filters" role="group" aria-label="Filter by change type">
<button type="button" data-news-type="all" aria-pressed="true">All</button>
<button type="button" data-news-type="new" aria-pressed="false">New</button>
<button type="button" data-news-type="improved" aria-pressed="false">Improved</button>
<button type="button" data-news-type="fixed" aria-pressed="false">Fixed</button>
<button type="button" data-news-type="breaking" aria-pressed="false">Breaking</button>
</div>
<p class="inc-news__status" data-news-status role="status" aria-live="polite" aria-atomic="true"></p>
</div>
<div class="inc-news__feed" id="news-feed" markdown="1">
<section class="inc-news__release" id="release-0-5" data-news-version="0.5" data-news-default markdown="1" aria-labelledby="release-0-5-title">
<header class="inc-news__release-heading" markdown="1">

## Incan 0.5 <span class="inc-news__badge">Released</span> {#release-0-5-title}

<a href="0_5/">Full 0.5 notes <img src="../shared/incapunk/icons/arrow-right.svg" alt=""></a>
</header>
<p class="inc-news__summary">Readable source. Real native systems.</p>
<div class="inc-news__timeline" markdown="1">
<section class="inc-news__entry" data-news-area="language" data-news-kind="new" markdown="1">

<p class="inc-news__meta"><span class="inc-news__kind inc-news__kind--new">New</span><span>Standard library</span></p>

### Build a typed data processor

Model JSON records, handle file failures, test transformations, and produce a native report tool.

```incan
batch = OrderBatch.from_json(source)?
```

<p class="inc-news__code-caption">Excerpt: OrderBatch is defined by the tutorial application.</p>

[Try the tutorial](../language/tutorials/typed_data_processor.md){ .inc-news__entry-link }

</section>
<section class="inc-news__entry" data-news-area="language" data-news-kind="improved" markdown="1">

<p class="inc-news__meta"><span class="inc-news__kind inc-news__kind--improved">Improved</span><span>Native interop</span></p>

### Use established Rust crates from Incan

Keep application logic in readable Incan and use existing crates through an explicit native boundary.

[Explore native integrations](0_5.md#native-boundaries-become-checked){ .inc-news__entry-link }

</section>
<section class="inc-news__entry" data-news-area="language" data-news-kind="breaking" markdown="1">

<p class="inc-news__meta"><span class="inc-news__kind inc-news__kind--breaking">Breaking</span><span>Models</span></p>

### Choose which model fields are public

Fields on a public model now need their own `pub` marker to belong to its external API. Check your models before upgrading.

[Read the migration guidance](0_5.md#before-you-upgrade){ .inc-news__entry-link }

</section>
<section class="inc-news__entry" data-news-area="language" data-news-kind="fixed" markdown="1">

<p class="inc-news__meta"><span class="inc-news__kind inc-news__kind--fixed">Fixed</span><span>0.5.1 · Expressions</span></p>

### Keep compound powers grouped

Compound bases of `**` retain their grouping during emission. The patch fixes valid programs that could previously return an incorrect numeric result.

[Read the patch notes](0_5.md#v051-rust-interop-and-installed-toolchain-repairs){ .inc-news__entry-link }

</section>
<section class="inc-news__entry" data-news-area="toolchain" data-news-kind="new" markdown="1">

<p class="inc-news__meta"><span class="inc-news__kind inc-news__kind--new">New</span><span>Packages</span></p>

### Publish a checked library

Build a library and consume its public contract through one portable dependency identity.

[Build and consume a library](../tooling/tutorials/build_and_consume_library.md){ .inc-news__entry-link }

</section>
<section class="inc-news__entry" data-news-area="toolchain" data-news-kind="new" markdown="1">

<p class="inc-news__meta"><span class="inc-news__kind inc-news__kind--new">New</span><span>Oven Alpha</span></p>

### Bake once, reuse compatible artifacts

Within the documented Alpha envelope, an explicit bake seals native build inputs for compatible build, run, and test commands. Unsupported shapes stop with guidance.

[Explore Oven Alpha](0_5.md#oven-turns-builds-into-evidence){ .inc-news__entry-link }

</section>
<section class="inc-news__entry" data-news-area="toolchain" data-news-kind="improved" markdown="1">

<p class="inc-news__meta"><span class="inc-news__kind inc-news__kind--improved">Improved</span><span>Workspaces</span></p>

### Keep projects on one checked view

Rooted and virtual workspaces share portable locks, provider identities, and inspectable feature selection.

[Read the package changes](0_5.md#packages-become-checked-inputs){ .inc-news__entry-link }

</section>
<section class="inc-news__entry" data-news-area="toolchain" data-news-kind="fixed" markdown="1">

<p class="inc-news__meta"><span class="inc-news__kind inc-news__kind--fixed">Fixed</span><span>0.5.1 · Distribution</span></p>

### Repair installed native toolchains

Release archives retain the standard-library sources and split-metadata sidecars needed by installed native consumers.

[Read the patch notes](0_5.md#v051-rust-interop-and-installed-toolchain-repairs){ .inc-news__entry-link }

</section>
</div>
</section>
<section class="inc-news__release" id="release-0-4" data-news-version="0.4" markdown="1" aria-labelledby="release-0-4-title">
<header class="inc-news__release-heading" markdown="1">

## Incan 0.4 <span class="inc-news__badge">Released</span> {#release-0-4-title}

<a href="0_4/">Full 0.4 notes <img src="../shared/incapunk/icons/arrow-right.svg" alt=""></a>
</header>
<p class="inc-news__summary">Install, try, inspect, and automate.</p>
<div class="inc-news__timeline" markdown="1">
<section class="inc-news__entry" data-news-area="language" data-news-kind="improved" markdown="1">

<p class="inc-news__meta"><span class="inc-news__kind inc-news__kind--improved">Improved</span><span>Compatibility</span></p>

### Keep ordinary 0.3 programs working

The release focuses on tooling rather than a language rewrite. Review its migration notes when adopting the new inspection and reporting commands.

[Read the migration notes](0_4.md#migrating-from-03){ .inc-news__entry-link }

</section>
<section class="inc-news__entry" data-news-area="toolchain" data-news-kind="new" markdown="1">

<p class="inc-news__meta"><span class="inc-news__kind inc-news__kind--new">New</span><span>Installation</span></p>

### Start without cloning the compiler

The checksum-verified installer and zero-clone starter flow take you from a toolchain install to a runnable project.

[Explore installation](0_4.md#install-and-first-contact){ .inc-news__entry-link }

</section>
<section class="inc-news__entry" data-news-area="toolchain" data-news-kind="new" markdown="1">

<p class="inc-news__meta"><span class="inc-news__kind inc-news__kind--new">New</span><span>Diagnostics</span></p>

### Give tools structured compiler output

Use incan check, incan explain, and JSON build reports instead of scraping terminal prose.

[Explore diagnostics and reports](0_4.md#diagnostics-and-reports){ .inc-news__entry-link }

</section>
<section class="inc-news__entry" data-news-area="toolchain" data-news-kind="new" markdown="1">

<p class="inc-news__meta"><span class="inc-news__kind inc-news__kind--new">New</span><span>Inspection</span></p>

### Inspect compiler-backed source structure

incan inspect codegraph exports source, declaration, import, diagnostic, and provenance facts as JSONL.

[Explore codegraph inspection](0_4.md#semantic-inspection-and-codegraph-export){ .inc-news__entry-link }

</section>
</div>
</section>
<section class="inc-news__release" id="release-0-3" data-news-version="0.3" markdown="1" aria-labelledby="release-0-3-title">
<header class="inc-news__release-heading" markdown="1">

## Incan 0.3 <span class="inc-news__badge">Released</span> {#release-0-3-title}

<a href="0_3/">Full 0.3 notes <img src="../shared/incapunk/icons/arrow-right.svg" alt=""></a>
</header>
<p class="inc-news__summary">More expressive programs, stronger library boundaries.</p>
<div class="inc-news__timeline" markdown="1">
<section class="inc-news__entry" data-news-area="language" data-news-kind="new" markdown="1">

<p class="inc-news__meta"><span class="inc-news__kind inc-news__kind--new">New</span><span>Control flow</span></p>

### Let a loop produce a value

Use loop: with break <value> for searches, retries, and other loops whose result belongs in an expression.

[Explore the language additions](0_3.md#language-features){ .inc-news__entry-link }

</section>
<section class="inc-news__entry" data-news-area="language" data-news-kind="new" markdown="1">

<p class="inc-news__meta"><span class="inc-news__kind inc-news__kind--new">New</span><span>Iteration</span></p>

### Compose lazy iteration pipelines

Generators and source-owned iterator adapters support pipelines with explicit terminal operations such as collect, find, and fold.

[Work with generators](../language/how-to/generators.md){ .inc-news__entry-link }

</section>
<section class="inc-news__entry" data-news-area="language" data-news-kind="breaking" markdown="1">

<p class="inc-news__meta"><span class="inc-news__kind inc-news__kind--breaking">Breaking</span><span>Numeric names</span></p>

### Check local numeric type aliases

Names such as decimal, bigint, and numeric can collide with the canonical numeric vocabulary. Rename local aliases when migrating.

[Read the migration guidance](0_3.md#migrating-from-02){ .inc-news__entry-link }

</section>
<section class="inc-news__entry" data-news-area="toolchain" data-news-kind="improved" markdown="1">

<p class="inc-news__meta"><span class="inc-news__kind inc-news__kind--improved">Improved</span><span>Formatter</span></p>

### Format calls and fluent chains consistently

The formatter follows the vertical-spacing contract, wraps more long calls and signatures, and preserves scoped DSL expressions.

[Read the tooling changes](0_3.md#tooling){ .inc-news__entry-link }

</section>
<section class="inc-news__entry" data-news-area="toolchain" data-news-kind="new" markdown="1">

<p class="inc-news__meta"><span class="inc-news__kind inc-news__kind--new">New</span><span>Testing</span></p>

### Run tests with structured results

The test runner supports fixtures, parametrization, markers, JSON Lines, and JUnit XML for local and automated workflows.

[Explore the test workflow](0_3.md#tooling){ .inc-news__entry-link }

</section>
</div>
</section>
<section class="inc-news__release" id="release-0-2" data-news-version="0.2" markdown="1" aria-labelledby="release-0-2-title">
<header class="inc-news__release-heading" markdown="1">

## Incan 0.2 <span class="inc-news__badge">Released</span> {#release-0-2-title}

<a href="0_2/">Full 0.2 notes <img src="../shared/incapunk/icons/arrow-right.svg" alt=""></a>
</header>
<p class="inc-news__summary">Explicit libraries and a clearer native boundary.</p>
<div class="inc-news__timeline" markdown="1">
<section class="inc-news__entry" data-news-area="language" data-news-kind="breaking" markdown="1">

<p class="inc-news__meta"><span class="inc-news__kind inc-news__kind--breaking">Breaking</span><span>Imports</span></p>

### Move standard-library imports to std.*

Import web, testing, and async capabilities from their named standard-library modules. Existing 0.1 projects need an import migration.

```incan
from std.testing import assert_eq
```

<p class="inc-news__code-caption">Existing standard-library import.</p>

[Migrate imports and decorators](0_2.md#migrating-from-01){ .inc-news__entry-link }

</section>
<section class="inc-news__entry" data-news-area="language" data-news-kind="new" markdown="1">

<p class="inc-news__meta"><span class="inc-news__kind inc-news__kind--new">New</span><span>Modules</span></p>

### Keep runtime state in a module

Module static storage gives source modules an explicit place for runtime state.

[Explore static storage](0_2.md#module-static-storage){ .inc-news__entry-link }

</section>
<section class="inc-news__entry" data-news-area="toolchain" data-news-kind="new" markdown="1">

<p class="inc-news__meta"><span class="inc-news__kind inc-news__kind--new">New</span><span>Libraries · Experimental</span></p>

### Share an Incan library between projects

Experimental library builds publish public APIs for other Incan projects to consume.

[Explore the library workflow](0_2.md#incan-libraries-experimental){ .inc-news__entry-link }

</section>
<section class="inc-news__entry" data-news-area="toolchain" data-news-kind="breaking" markdown="1">

<p class="inc-news__meta"><span class="inc-news__kind inc-news__kind--breaking">Breaking</span><span>Dependencies</span></p>

### Separate Incan and Rust dependencies

Use distinct manifest tables for Incan packages and Rust crates when migrating a project from 0.1.

[Update the project manifest](0_2.md#3-split-incan-dependencies-from-rust-dependencies){ .inc-news__entry-link }

</section>
</div>
</section>
<section class="inc-news__release" id="release-0-1" data-news-version="0.1" markdown="1" aria-labelledby="release-0-1-title">
<header class="inc-news__release-heading" markdown="1">

## Incan 0.1 <span class="inc-news__badge">Released</span> {#release-0-1-title}

<a href="0_1/">Full 0.1 notes <img src="../shared/incapunk/icons/arrow-right.svg" alt=""></a>
</header>
<p class="inc-news__summary">The first published Incan release.</p>
<div class="inc-news__timeline" markdown="1">
<section class="inc-news__entry" data-news-area="language" data-news-kind="new" markdown="1">

<p class="inc-news__meta"><span class="inc-news__kind inc-news__kind--new">New</span><span>Language</span></p>

### Write typed programs with explicit failure

Typed functions, models, enums, pattern matching, and Result/Option error handling establish the first language surface.

[Explore the first release](0_1.md#features-and-enhancements){ .inc-news__entry-link }

</section>
<section class="inc-news__entry" data-news-area="toolchain" data-news-kind="new" markdown="1">

<p class="inc-news__meta"><span class="inc-news__kind inc-news__kind--new">New</span><span>CLI</span></p>

### Build, run, format, and test

The first published release introduces the core CLI workflow and a generated language reference.

[Read the release highlights](0_1.md#highlights){ .inc-news__entry-link }

</section>
</div>
</section>
<section class="inc-news__release" id="release-0-6" data-news-version="0.6" markdown="1" aria-labelledby="release-0-6-title">
<header class="inc-news__release-heading" markdown="1">

## Incan 0.6 <span class="inc-news__badge inc-news__badge--development">In development</span> {#release-0-6-title}

<a href="0_6/">Full 0.6 notes <img src="../shared/incapunk/icons/arrow-right.svg" alt=""></a>
</header>
<p class="inc-news__summary">Compiler-owned meaning and Oven-native project authority.</p>
<p class="inc-news__development-note"><strong>Incan 0.6 has not shipped.</strong> These highlights describe the development line and its release contract. Consult the full notes for delivered behavior, remaining acceptance work, and current limitations.</p>
<div class="inc-news__timeline" markdown="1">
<section class="inc-news__entry" data-news-area="language" data-news-kind="new" markdown="1">

<p class="inc-news__meta"><span class="inc-news__kind inc-news__kind--new">New</span><span>Standard library · Development</span></p>

### Read typed TOML values

The development notes describe typed manifest values and declared models in std.toml, with source locations for parse and structural errors. Comment-preserving editing remains deferred.

[Read the development notes](0_6.md#the-compiler-owns-source-meaning-end-to-end){ .inc-news__entry-link }

</section>
<section class="inc-news__entry" data-news-area="language" data-news-kind="improved" markdown="1">

<p class="inc-news__meta"><span class="inc-news__kind inc-news__kind--improved">Improved</span><span>Packages · Development</span></p>

### Carry union types across package contracts

The development line records anonymous unions in checked package signatures and an explicit manifest-format boundary for older readers.

[Read the package development notes](0_6.md#the-compiler-owns-source-meaning-end-to-end){ .inc-news__entry-link }

</section>
<section class="inc-news__entry" data-news-area="toolchain" data-news-kind="improved" markdown="1">

<p class="inc-news__meta"><span class="inc-news__kind inc-news__kind--improved">Improved</span><span>Compiler · Development</span></p>

### Make compiler facts the compilation authority

The release contract moves lowering, backend selection, and parity evidence onto compiler-owned facts. The notes distinguish delivered slices from acceptance work still owed.

[Explore the compiler direction](0_6.md#the-compiler-owns-source-meaning-end-to-end){ .inc-news__entry-link }

</section>
<section class="inc-news__entry" data-news-area="toolchain" data-news-kind="improved" markdown="1">

<p class="inc-news__meta"><span class="inc-news__kind inc-news__kind--improved">Improved</span><span>Oven · Development</span></p>

### Make Oven the normal project authority

The development contract covers selected Loafs, sealed dependencies, and explicit build evidence. Read the current boundaries before trying the development line.

[Explore the Oven direction](0_6.md#loaf-and-oven-become-the-opted-in-project-authority){ .inc-news__entry-link }

</section>
</div>
</section>
</div>
<div class="inc-news__empty" data-news-empty hidden>
<h3>No matching highlights</h3>
<p>This is a curated selection. The full release notes include the complete change inventory.</p>
<button type="button" data-news-reset>Show all change types</button>
</div>
<footer class="inc-news__footer" markdown="1">

## Full release notes

Highlights link to the detailed release notes and guides. For complete inventories, patch notes, and migration requirements, choose a release:

[0.6 — in development](0_6.md) · [0.5](0_5.md) · [0.4](0_4.md) · [0.3](0_3.md) · [0.2](0_2.md) · [0.1](0_1.md)

### Docs versioning model

**dev** documentation is built from `main` and includes work that has not shipped. Released documentation is built from tags and published by minor version. A release badge on this page describes the release; it does not detect your installed compiler. See the [stability policy](../stability.md) before upgrading.

</footer>
</div>
</article>

<!-- markdownlint-enable MD033 -->
