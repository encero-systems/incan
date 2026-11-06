# RFC 082: Checked API documentation generation

- **Status:** Planned
- **Created:** 2026-04-28
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 015 (project lifecycle and CLI tooling)
    - RFC 031 (library system phase 1)
    - RFC 125 (`loaves.pub` Loaf registry; supersedes RFC 034)
    - RFC 048 (checked contract metadata, Incan emit, and interrogation tooling)
    - RFC 079 (`loaves.pub` artifact graph)
- **Issue:** [#556](https://github.com/encero-systems/incan/issues/556)
- **RFC PR:** —
- **Written against:** v0.3
- **Shipped in:** —

## Summary

This RFC defines first-class Incan API documentation generation on top of RFC 048 checked API metadata. RFC 048 owns the compiler-checked metadata extraction contract; this RFC owns the user-facing documentation command, documentation bundle model, Markdown reference output, validation modes, deterministic anchors, and package-facing docs artifacts that can be consumed by local tooling, registries, package browsers, or downstream site tooling. Hosted documentation sites, MkDocs Material-style theming, publishing workflows, and product-specific navigation remain outside the compiler and belong to higher-level site tooling.

## Core model

1. **Checked metadata is the source of truth:** documentation generation consumes RFC 048 checked API metadata, either from a source package after parsing and typechecking or from a built artifact that carries the same metadata.
2. **Docs are derived artifacts:** generated reference docs are projections of checked source structure. They must not become a second source of truth for declarations, signatures, decorators, aliases, field metadata, or docstrings.
3. **The stable renderer is Markdown:** the required output is deterministic Markdown reference documentation with stable anchors and enough structure for downstream site tooling to ingest.
4. **HTML is a convenience renderer:** implementations may provide local HTML output or preview, but HTML is not the normative interchange format and must not be required by packages, registries, or downstream consumers.
5. **Validation is explicit:** docs generation includes a check mode that validates mechanically checkable docstring claims against RFC 048 metadata and reports actionable diagnostics.
6. **Package docs are artifact-aware:** packages may carry documentation bundles or enough RFC 048 metadata for registries and local tools to regenerate the same reference docs without source scraping.
7. **Site generation is downstream:** a site tool may turn generated Markdown and metadata into a hosted site, search UI, theme, or multi-package portal, but those presentation concerns are not part of this RFC.

## Motivation

Incan packages need trustworthy API reference documentation. The language already has enough structure to describe public functions, models, fields, decorators, aliases, constants, and docstrings, but handwritten docs drift unless the toolchain checks them against the compiled API surface. Source scraping is not good enough because it duplicates compiler behavior and breaks once packages are consumed as built artifacts rather than editable source trees.

RFC 048 solves the first half of the problem by defining checked metadata extraction. That is deliberately a metadata contract, not a docs product. The next layer is still needed: users need a command that turns checked metadata into readable reference docs, package tooling needs a deterministic artifact it can publish or inspect, and downstream systems need a stable input that does not require them to parse Incan or inspect compiler internals.

The end-state should look closer to a compiler-backed reference docs pipeline than to a static-site theme. Incan should be able to produce accurate package reference docs from checked APIs. Downstream site tooling can decide how those docs become a polished website, product portal, search experience, or registry page.

## Goals

- Define a first-class docs generation command family for source packages and metadata-bearing artifacts.
- Require documentation generation to consume RFC 048 checked API metadata rather than source scraping or compiler-internal representations.
- Define deterministic Markdown reference output as the stable documentation artifact.
- Define stable declaration anchors so generated docs can be linked by registries, package browsers, editors, and hosted sites.
- Define explicit validation modes for docstring drift, including parameters, returns, fields, aliases, and exposed decorator metadata where those are mechanically checkable.
- Allow local HTML preview as a convenience without making HTML the stable interchange contract.
- Define how package and registry flows may carry or regenerate documentation artifacts from checked metadata.
- Keep downstream site tooling responsible for hosted site generation, theming, navigation, search UI, publishing, and product-specific documentation composition.

## Non-Goals

- Defining the RFC 048 checked metadata schema. This RFC consumes that metadata and may add documentation-bundle requirements, but it does not replace the extraction contract.
- Owning hosted documentation-site generation in the compiler or CLI.
- Standardizing MkDocs Material-style themes, product portals, hosted search pages, or publishing workflows.
- Replacing handwritten guides, tutorials, conceptual docs, release notes, or whitepapers.
- Defining query-language-specific catalogs or any other product-specific reference view.
- Executing user code to discover documentation facts.
- Guaranteeing source-compatible behavior with rustdoc, Griffe, MkDocs, or any other external documentation tool.

## Guide-level explanation

A package author can generate API reference docs from checked source:

```shell
incan docs build --output docs/api
```

The command parses and typechecks the package, asks the RFC 048 metadata layer for public API structure, validates docstrings according to the selected mode, and writes deterministic Markdown files. The generated files are readable in a repository, can be packaged as derived artifacts, and can be consumed by a site tool without that site tool needing to understand Incan typechecking.

A stricter workflow can run documentation validation without producing a full reference tree:

```shell
incan docs check
```

If a public function documents a parameter that no longer exists, omits a required return section under the active policy, or describes a field that is not present in the checked model metadata, the command reports diagnostics against the source declaration or metadata-bearing artifact. During ordinary work such drift is a warning, so a stale parameter name in prose is visible without blocking unrelated editing; `incan docs check` and `incan docs build --strict` turn it into an error for CI and publishing. Source that is actually invalid, such as a wrong type, stays an ordinary compiler error.

`incan docs` on its own lists the family's commands, so authors find `build` and `check` without a separate documentation tool.

The input source remains ordinary Incan:

```incan
@aggregate("avg")
pub def avg(values: List[float]) -> float:
    """
    Return the arithmetic mean.

    Args:
        values: Input values.

    Returns:
        Mean value.
    """
    ...
```

The generated Markdown should be deterministic and linkable:

```markdown
## `avg(values: List[float]) -> float` {#fn-avg}

Return the arithmetic mean.

**Decorator:** `aggregate("avg")`

**Parameters**

- `values: List[float]` — Input values.

**Returns**

- `float` — Mean value.
```

That Markdown is not the authoritative API. It is a projection of checked metadata. If the function signature changes, the next docs run updates or rejects the projection according to the active validation mode.

For a built package that carries RFC 048 metadata, a registry or package browser can generate the same reference docs without a source checkout:

```shell
incan docs build --from-artifact dist/stats.incnlib --output docs/api
```

Docstrings link to other declarations the way MkDocs and Sphinx cross-references do, with a Markdown reference link whose target is a public symbol path:

```incan
pub def median(values: List[float]) -> float:
    """
    Return the middle value. For the arithmetic mean, see [avg][stats.avg].
    """
    ...
```

The generator resolves `stats.avg` against checked metadata, including through public aliases, and links it to the `#fn-avg` anchor; an unknown or ambiguous target is a drift diagnostic. Alongside the Markdown, the generator writes a versioned symbol inventory, like Sphinx's `objects.inv`, so other packages' docs can link into this one, and a versioned search index of public symbols, like MkDocs' search index, for site tools to build search on.

A downstream site generator can then treat the generated Markdown, inventory, index, and metadata as input to a richer site: navigation, search UI and ranking, cross-package landing pages, branding, themes, publishing, and hosted previews are downstream presentation choices.

## Reference-level explanation

### Inputs

Documentation generation must consume checked API metadata as defined by RFC 048. The metadata may come from a source root after parsing and typechecking, from an in-memory compiler result, or from a supported built artifact that carries RFC 048 metadata.

Documentation generation must not treat raw source text, compiler-internal representations, or untyped sidecar files as authoritative for public declarations, signatures, decorator arguments, aliases, model fields, or type information.

If the input artifact does not carry RFC 048 metadata, the command must report that checked documentation generation is unavailable for that artifact rather than inventing a best-effort API by scraping files.

### Documented surface

The default documented surface is the public package API exposed by RFC 048 metadata. That includes public functions and methods, public models/classes/traits/enums/newtypes/type aliases, exported constants or statics that RFC 048 exposes as safe metadata, public aliases, relevant decorators, checked signatures, model fields, field metadata, field documentation, docstrings, module paths, package identity, and stable anchors.

Private declarations are not included in generated reference docs by default. An explicit `--include-private` option may add private and internal declarations; it must not change the public API contract or the anchors of public declarations.

### Output formats

The required output format is Markdown. Markdown output must be deterministic for a fixed checked metadata document, documentation generator version, and configuration.

Markdown output must preserve enough structure for downstream site tooling to identify packages, modules, declarations, anchors, signatures, prose, parameter documentation, return documentation, field documentation, decorators, aliases, and cross-links where available.

Implementations may provide local HTML output, local preview, or other renderers. Those renderers must be treated as projections of the same checked documentation model and must not define additional API facts that are absent from RFC 048 metadata or the documentation bundle.

### Anchors and links

Generated declaration anchors must be stable across formatting-only source changes and should be derived from package identity, module path, declaration kind, public name, and signature disambiguators when needed.

Anchors must not rely solely on source line numbers. Line numbers may be included as diagnostic context, but they are not stable documentation identity.

If two public declarations would produce the same anchor, the generator must apply a documented deterministic disambiguation rule or emit a diagnostic if no stable disambiguation is possible. Formatting-only edits must not change an anchor; moving or renaming a declaration may change it deliberately.

A docstring cross-reference is a Markdown reference link whose target is a public symbol path, such as `[avg][stats.avg]` or `[stats.avg][]`. The generator must resolve each target against checked metadata, through public aliases to the canonical declaration, and must link it to that declaration's anchor. An unresolved or ambiguous target must be reported as drift.

Every generation must also produce, versioned with the documentation bundle:

- a **symbol inventory** mapping each public symbol path, and each public alias, to its canonical declaration, kind, signature summary, and anchor, so other packages' documentation can resolve references into this package, as Sphinx's `objects.inv` allows;
- a **search index** of public symbols with their kind, signature summary, first docstring sentence, and anchor, for site tools to build search on.

Search ranking, search UI, navigation, theming, and site layout are not part of this contract.

### Validation modes

Docs generation must support a validation-only mode. Validation compares mechanically checkable docstring sections against RFC 048 metadata.

The recognized sections are `Args:` (or `Parameters:`), `Returns:`, `Errors:`, `Fields:`, `Aliases:` and `Decorators:`. `Errors:` lists the error cases a `Result`-returning function can produce, by name.

Validation must detect documented parameters that do not exist, public parameters missing required documentation under strict policy, documented return sections that contradict the checked return type, documented errors the checked return type cannot produce, documented fields that do not exist, and exposed decorator or alias claims that contradict checked metadata.

Validation must not execute user code. It may rely on parsed docstrings, checked declarations, compiler-known safe constants, and safe metadata values exposed by RFC 048.

Mechanically established drift must be a warning by default. `incan docs check` and `incan docs build --strict` must report drift as an error and exit unsuccessfully, so CI and package publishing can reject stale docs. Diagnostics must name the exact stale parameter, field, section, or reference by comparing against checked metadata. Source that fails to typecheck remains an ordinary compiler error and is not reported as documentation drift.

### Package artifacts

A published package must carry the RFC 048 metadata needed to regenerate its reference docs. It may also carry a generated documentation bundle, which must record the digest of the metadata it was generated from and its generator version. Tooling must validate that digest before trusting a carried bundle, and must regenerate from the metadata when the bundle is missing, stale, or produced by an unsupported generator version. Rendered documentation never replaces checked facts.

Registry and package-browser tooling must use checked metadata or a validated documentation bundle, never source scraping. If a package contains neither, tooling must report that checked reference docs are unavailable.

Generated docs are derived artifacts. Publishing them must not change the package's checked API identity.

### Diagnostics

Diagnostics must point at the declaration, docstring section, artifact metadata entry, or generated documentation entry that caused the mismatch, depending on the available input.

Diagnostics should use checked names and public API concepts rather than backend implementation details.

## Design details

### Command model

Documentation is a semantic product, so the command family belongs to `incan` under RFC 118's command ownership. `incan docs` lists the family. `incan docs build` generates Markdown, the symbol inventory, and the search index from a source package or, with `--from-artifact`, from a metadata-bearing artifact. `incan docs check` validates without writing output. Both support RFC 118's `--format json` structured output for their diagnostics and results. Carrying the bundle in a package is part of Oven's bake and publish flow, which uses the same generator.

### Documentation bundle

Implementations should define a documentation bundle as the handoff between checked metadata and renderers. The bundle may be persisted or in-memory, but it must be versioned when persisted and must not contain unchecked API facts.

The bundle exists so Markdown generation, local HTML preview, package publishing, LSP previews, and downstream site tools can share one checked documentation model instead of each renderer reinterpreting RFC 048 metadata independently.

### Relationship to RFC 048

RFC 048 owns the checked metadata extraction contract. This RFC owns documentation-generation behavior that consumes that contract.

If RFC 048 metadata cannot represent a fact, this RFC cannot make that fact authoritative by rendering it. The renderer may include plain prose from docstrings, but structured API facts must come from checked metadata or from mechanically validated documentation sections.

### Relationship to hosted site tooling

Hosted site tooling is the right layer for MkDocs Material-style site generation: hosted pages, themes, search interfaces, multi-package portals, landing pages, product branding, publishing destinations, and cross-project documentation composition.

This RFC should give site tooling a stable, compiler-checked input rather than forcing it to scrape source or duplicate Incan's compiler semantics.

### Compatibility / migration

This RFC is additive. Existing packages without checked docs generation continue to build.

Packages adopting this RFC should treat generated Markdown as derived output. Hand-edited changes to generated reference files should either be overwritten on regeneration or moved into handwritten guide/tutorial docs.

Packaging flows that already preserve RFC 048 metadata can enable artifact-time docs generation without shipping source.

## Alternatives considered

1. **Keep all documentation generation inside RFC 048**
   - Rejected because metadata extraction and documentation rendering have different consumers, stability promises, and implementation slices. RFC 048 should stay focused on checked facts.

2. **Make MkDocs Material the Incan docs contract**
   - Rejected because MkDocs Material is a site renderer and theme ecosystem, not the compiler-checked API model. It belongs in downstream site tooling.

3. **Generate only HTML**
   - Rejected because HTML is useful for preview but awkward as the stable artifact for registries, package diffs, source control review, and downstream composition.

4. **Leave docs entirely to downstream tools**
   - Rejected because downstream tools should not have to parse Incan, duplicate typechecking, or inspect compiler internals to produce accurate API reference docs.

5. **Clone rustdoc or Griffe behavior wholesale**
   - Rejected because those tools are useful prior art, but Incan has its own package model, RFC 048 metadata layer, docstring validation needs, artifact story, and hosted-site boundary.

## Drawbacks

- Adds another user-facing tooling surface that must be maintained alongside compiler and package workflows.
- Creates a new derived artifact that can become stale if projects check generated Markdown into source control without validation.
- Requires careful versioning between RFC 048 metadata, documentation bundles, and renderers.
- Risks confusing users if local HTML preview is mistaken for the hosted-site story that belongs to downstream site tooling.
- Increases pressure on docstring parsing and diagnostics before the language has a large documentation corpus.

## Implementation architecture

*(Non-normative.)* The implementation should flow from checked package or artifact input to RFC 048 metadata, then to a documentation bundle, then to one or more renderers. Markdown is the required renderer. Local HTML can wrap the same bundle for preview. Package and registry integrations should consume the persisted bundle or regenerate it from RFC 048 metadata, while hosted site tooling consumes the generated Markdown and/or bundle for site generation.

## Layers affected

- **Parser / frontend**: must preserve docstrings and documentation-relevant source spans well enough for RFC 048 metadata extraction and diagnostics.
- **Typechecker / symbol resolution**: must provide checked public API facts through RFC 048 rather than requiring docs tooling to rediscover names, signatures, decorators, aliases, or fields.
- **Checked metadata extractor**: must provide the public API metadata needed by the documentation bundle and must report unsupported or unsafe metadata values explicitly.
- **CLI / tooling**: `incan docs build` and `incan docs check` expose documentation generation, validation, artifact input, output selection, structured output, and actionable diagnostics.
- **Formatter / docs renderer**: must produce deterministic Markdown, a symbol inventory, and a search index for a fixed checked metadata document and generator configuration, resolving docstring cross-references.
- **Build / packaging**: must preserve RFC 048 metadata in published packages and may carry a documentation bundle bound to that metadata by digest.
- **LSP / editor tooling**: should be able to preview generated declaration docs and validation diagnostics from the same documentation bundle model.
- **Registry / package browser**: should consume checked documentation artifacts or regenerate docs from carried RFC 048 metadata rather than scraping source.
- **Hosted site tooling**: should consume generated Markdown and documentation bundles as checked inputs for hosted site generation, navigation, search, and product presentation.

## Design Decisions

- Markdown is the required renderer. HTML and local preview remain optional projections of the same checked documentation model, as specified under Output formats; they must not introduce additional API facts.
- The command family is `incan docs`, with `incan docs build` and `incan docs check`, owned by `incan` as a semantic product under RFC 118 and sharing its `--format json` convention.
- Docstring drift is a warning by default; `incan docs check` and `incan docs build --strict` make it an error. Source that fails to typecheck stays an ordinary compiler error.
- Published packages must carry RFC 048 metadata. A generated documentation bundle is optional, is bound to that metadata by digest, and is regenerated when missing or stale; rendered docs never replace checked facts.
- The generator owns canonical declaration anchors, checked docstring cross-references written as Markdown reference links to symbol paths, a versioned symbol inventory for cross-package links, and a versioned search index. Search ranking and UI, navigation, theming, and publishing stay with downstream site tooling, in the way MkDocs and Sphinx separate their indexes from their themes.
