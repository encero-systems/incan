---
search:
  boost: 3
tags:
  - roadmap
  - release 0.6
  - v0.6
  - in development
---

# Incan 0.6 roadmap

0.6 is the architectural cutover that makes compiler-owned facts and the Oven project model the normal authority for compilation, packages, build artifacts, and inspection. Generated Rust remains available for inspection and debugging; it is no longer the semantic handoff the release is working toward.

This page is the public delivery map for that work. It complements the [development release note](../release_notes/0_6.md): the release note explains the contract, while this page shows how the work is organized and what each slice must prove.

> **How to read this map.** The rows are the recommended delivery sequence, not a claim that all work is serial. Select a row to expand that slice’s issue graph directly beneath it. A solid line is GitHub’s native parent/sub-issue relationship. A dashed line is a GitHub `blocked by` prerequisite. Every issue node opens the corresponding GitHub issue.

## The release plan

--8<-- "_snippets/project/v0_6_roadmap.md"

## Keeping the map current

The rendered map is a checked-in GitHub snapshot, not a browser-side query. That keeps site builds reproducible and preserves the exact program state readers are seeing. To refresh it after a material milestone, hierarchy, status, or dependency change, run:

```console
make -C workspaces/docs-site docs-refresh
```

The umbrella command currently delegates to the v0.6 roadmap refresh and is the stable entry point for future checked-in documentation snapshots. It uses the public GitHub REST API: no `gh` session or browser-side request is involved. A small POSIX tool using `curl` and `jq` validates the response, reconstructs GitHub’s native hierarchy, counts issue state, and renders the Markdown/Mermaid snapshot. It works anonymously for occasional refreshes; set `GITHUB_TOKEN` with read access for routine use, avoiding GitHub's small anonymous request budget. Review and commit the resulting snapshot with any corresponding change to the release note or [main roadmap](../roadmap.md).
