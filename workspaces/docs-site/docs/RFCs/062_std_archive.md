# RFC 062: `std.archive` — archive container creation and extraction

- **Status:** Draft
- **Created:** 2026-04-14
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 022 (namespaced stdlib modules and compiler handoff)
    - RFC 023 (compilable stdlib and Rust module binding)
    - RFC 055 (`std.fs` path-centric filesystem APIs)
    - RFC 056 (`std.io` in-memory byte streams and binary parsing helpers)
    - RFC 061 (`std.compression` codec-based compression and decompression)
- **Issue:** https://github.com/dannys-code-corner/incan/issues/340
- **RFC PR:** —
- **Written against:** v0.2
- **Shipped in:** —

## Summary

This RFC proposes `std.archive` as Incan's standard library module for archive-container workflows. It standardizes creation, listing, inspection, selective entry access, and extraction for `tar` and `zip`, keeps compression explicit and layered via `std.compression`, and enforces safe extraction defaults as part of the core contract.

## Motivation

Archive containers are routine systems and data infrastructure: build artifacts, releases, model bundles, dataset drops, and backup payloads. Compression alone is not enough because archive formats encode multi-file structure, path metadata, and entry-level behavior.

Without a standard archive module, users either write unsafe extraction glue code or fall directly into backend-specific interop for basic workflows.

## Goals

- Provide a standard archive-container surface in `std.archive`.
- Support core lifecycle operations: create, list, inspect, extract, and read single entries.
- Keep archive and compression concerns separate but composable.
- Make safe extraction behavior a default contract requirement.
- Include deterministic archive creation options in this RFC.

## Non-Goals

- Replacing specialized packaging toolchains.
- Covering every archive format in the first contract.
- Embedding package manager semantics into archive APIs.
- Implicitly trusting archive paths, links, or metadata from untrusted archives.

## Guide-level explanation

```incan
from std.archive import tar

tar.create(
    source_dir="build/out",
    destination="build/out.tar",
    compression="none",
    deterministic=True,
)?
```

```incan
from std.archive import tar

tar.create(
    source_dir="release",
    destination="release.tar.zst",
    compression="zstd",
    deterministic=True,
)?
```

```incan
from std.archive import zip

entries = zip.list("artifacts.zip")?
for entry in entries:
    println(entry.path)
```

```incan
from std.archive import tar

tar.extract(
    source="release.tar.gz",
    destination_dir="release",
    policy="safe",
    overwrite="never",
)?
```

## Reference-level explanation

### Module scope

`std.archive` provides two format submodules in this RFC:

- `std.archive.tar`
- `std.archive.zip`

Each submodule exposes:

- `create(...)`
- `list(...)`
- `extract(...)`
- `read_entry(...)`

### Format set

Initial format set is fixed to `tar` and `zip`.

Other formats may be added in later RFCs, but this RFC does not leave the initial set open.

### Compression layering

Compression remains explicit:

- `tar` supports an explicit `compression` option (`none`, `gzip`, `zstd`, `bz2`, `lzma`, where available through `std.compression`).
- `zip` exposes method-specific options as part of `zip.create(...)` and does not route through implicit top-level autodetection.

### Core types

The module should define a stable entry metadata type:

- `ArchiveEntry` with fields such as `path`, `kind`, `size`, `compressed_size`, `modified_at`, and format-specific optional metadata.

### Extraction policy

The extraction contract should be explicit:

- `policy="safe"` is the default and required baseline behavior.
- `policy="unsafe"` is explicit opt-in.
- overwrite policy is explicit (`never`, `if_newer`, `always`).

## Design details

### Rust-grounded safety baseline

The safety baseline is grounded in Rust archive behavior, not Python defaults:

- Rust `tar` `unpack_in` states it “avoids writing outside of `dst`” and skips `..` traversal paths.
- Rust `zip` extraction uses `enclosed_name` path sanitization and documents non-atomic extraction.

`std.archive` should normalize those lessons into a language-level contract:

- reject absolute-path escapes;
- reject traversal escapes (`..`);
- block link targets that escape extraction root;
- avoid implicit unsafe behavior.

### Safe extraction defaults

`policy="safe"` must enforce:

- destination confinement for all extracted entries;
- path sanitization for archive names;
- symlink/hardlink escape checks;
- explicit overwrite behavior (default `never`);
- surfaced partial-extraction errors rather than silent best effort.

Unsafe extraction behavior, when enabled, must be explicit and clearly documented.

### Deterministic archive creation

Deterministic mode is in scope for this RFC and should include:

- stable entry ordering;
- normalized timestamps;
- normalized owner/group metadata where relevant;
- deterministic permission normalization policy.

This supports reproducible pipelines and artifact signing workflows.

### Archive/compression boundary

`std.archive` owns container semantics. `std.compression` owns codec semantics.

This RFC keeps that boundary strict:

- explicit compression selection for `tar`;
- explicit method/options for `zip`;
- no hidden codec guessing in archive creation/extraction APIs.

### Interoperability

The module should prioritize cross-tool compatibility with mainstream tar/zip tooling and should document unsupported edge features explicitly.

## Alternatives considered

1. **Treat archives as just compression plus concatenation**
   - Incorrect model; archives carry entry structure and metadata.

2. **Push archive handling entirely to Rust interop**
   - Too low-level for common Incan workflows.

3. **Fold archive APIs into `std.fs`**
   - Wrong boundary; archive container semantics deserve dedicated scope.

## Drawbacks

- Archive semantics are more complex than plain compression.
- Safe extraction defaults require strict behavior that may surprise users migrating from permissive tools.
- Tar and zip have different metadata models, increasing API design pressure.

## Layers affected

- **Stdlib / runtime**: format handling, entry metadata model, extraction safety, and deterministic creation.
- **Language surface**: the module and types must be available as specified.
- **Execution handoff**: implementations must preserve archive behavior without backend leakage.
- **Docs / examples**: safe extraction and deterministic archive patterns.

## Design Decisions

- The initial format set is `tar` and `zip`.
- `std.archive` is container-focused and is not folded into `std.fs` or `std.compression`.
- `tar` compression is represented via explicit `compression=...` options.
- Safe extraction is the default (`policy="safe"`), and unsafe behavior is explicit opt-in.
- Safe extraction must enforce destination confinement, traversal rejection, and link-escape blocking.
- Overwrite behavior is explicit, with default `overwrite="never"`.
- Deterministic archive creation is in scope for this RFC.
- The baseline public surface includes `create`, `list`, `extract`, and `read_entry` per format.
