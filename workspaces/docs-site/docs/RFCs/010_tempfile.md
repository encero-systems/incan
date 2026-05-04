# RFC 010: Python-style `tempfile` standard library

- **Status:** In Progress
- **Created:** 2024-12-11
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:** RFC 019 (runner testing), RFC 023 (stdlib namespacing and compiler handoff), RFC 055 (`std.fs` path-centric filesystem APIs), RFC 056 (`std.io` in-memory byte streams)
- **Issue:** [#79](https://github.com/dannys-code-corner/incan/issues/79)
- **RFC PR:** —
- **Written against:** v0.1
- **Shipped in:** —

## Summary

This RFC adds a Python-style `std.tempfile` module to the Incan standard library so programs can create scratch files, staging directories, and short-lived test fixtures that are cleaned up automatically unless explicitly persisted. The naming direction is settled: the public surface follows Python's `tempfile` family rather than abbreviated Rust-style type names.

## Motivation

Temporary filesystem objects are a basic systems-programming need:

- tests need isolated scratch space;
- safe file updates often write to a temporary path and rename into place;
- data-processing pipelines frequently need short-lived intermediate files;
- cleanup should happen reliably even on early returns or errors.

Python solves this with `tempfile`, while Rust commonly uses the `tempfile` crate. Incan should provide an equally explicit story rather than forcing users into manual `create -> remember path -> remember cleanup` patterns.

## Goals

- Provide first-class temporary files and directories with automatic cleanup.
- Use Python-style `tempfile` naming for the public surface.
- Make persistence explicit so authors can keep a temp artifact intentionally.
- Support both default system temp locations and caller-provided parent directories.
- Keep the feature ergonomic for tests and ordinary application code.

## Non-Goals

- Requiring an exact one-to-one clone of every Python `tempfile` behavior.
- Context-manager syntax just for temporary resources.
- Defining every possible OS-specific temporary-file flag or security knob in the initial RFC.
- Replacing ordinary `Path` and filesystem APIs.
- Defining generic `Reader` / `Writer` protocols or a pathless open-file handle model; those belong with `std.fs` / stream abstractions.

## Guide-level explanation (how users think about it)

### Named temporary files

```incan
from std.tempfile import NamedTemporaryFile

temp = NamedTemporaryFile.try_new()?
temp.path().write_text("some data", "utf-8", "strict", None)?

process_file(temp.path())
# file is deleted when `temp` goes out of scope unless it is persisted
```

### Temporary directories

```incan
from std.tempfile import TemporaryDirectory

temp_dir = TemporaryDirectory.try_new()?

config = temp_dir.path() / "config.toml"
config.write_text(default_config, "utf-8", "strict", None)?

data_dir = temp_dir.path() / "data"
data_dir.mkdir(True, True)?
```

When the `TemporaryDirectory` value is dropped, the temporary directory tree is removed.

### Keeping a result

```incan
from std.tempfile import NamedTemporaryFile

temp = NamedTemporaryFile.try_new_with("", ".json", None)?
temp.path().write_text(data, "utf-8", "strict", None)?

final_path = temp.persist()?
println(f"saved to {final_path}")
```

`persist()` converts a temporary resource into an ordinary path that will no longer be auto-deleted by the temp handle.

## Reference-level explanation (precise rules)

### Surface

The stdlib provides temporary filesystem types through `std.tempfile`. The implementable initial surface is:

- `NamedTemporaryFile`
- `TemporaryDirectory`

The module name and public type names are part of the contract. The RFC deliberately uses `std.tempfile.NamedTemporaryFile` and `std.tempfile.TemporaryDirectory`, not top-level compiler builtins and not abbreviated `TempFile` / `TempDir` names.

`TemporaryFile` and `SpooledTemporaryFile` are reserved Python-aligned follow-up names, not part of this RFC's required implementation surface. `TemporaryFile` needs a clearer open-file-handle contract than Incan currently has outside `std.fs.File`, and `SpooledTemporaryFile` is a storage-policy type that should align with `std.io.BytesIO` and `std.fs.File` once both surfaces exist.

### Required capabilities

- Create a named temporary file in the system temp directory.
- Create a temporary directory in the system temp directory.
- Create either one under a caller-provided parent directory.
- Provide configured `prefix`, `suffix`, and `dir` construction parameters for both initial types where the parameter makes sense.
- Expose the realized `std.fs.Path`.
- Persist the resource so automatic cleanup no longer runs.
- Provide text/binary file operations through `std.fs.Path` and `std.fs.File`, not by duplicating the filesystem API on the temporary handle.

### Factory shape

The user-visible fallible factories are:

- `NamedTemporaryFile.try_new() -> Result[NamedTemporaryFile, E]`.
- `NamedTemporaryFile.try_new_with(prefix: str, suffix: str, dir: Option[Path]) -> Result[NamedTemporaryFile, E]`.
- `TemporaryDirectory.try_new() -> Result[TemporaryDirectory, E]`.
- `TemporaryDirectory.try_new_with(prefix: str, suffix: str, dir: Option[Path]) -> Result[TemporaryDirectory, E]`.

The exact error payload type may follow the stdlib's filesystem error model, but construction failures must be ordinary `Result` failures.

Implementation note: the initial implementation splits default and configured factories because imported stdlib static-method default expansion is tracked in [#500](https://github.com/dannys-code-corner/incan/issues/500). The configured `dir` parameter is `Option[Path]` rather than `Option[Path | str]` because nested option-union lowering in authored stdlib code is tracked in [#501](https://github.com/dannys-code-corner/incan/issues/501).

### Handle methods

Both initial handle types must expose:

- `path() -> Path`.
- `persist() -> Result[Path, E]`.

`NamedTemporaryFile` may additionally expose `open(...) -> Result[File, E]` if `std.fs.File` is available in the same implementation slice, but file reading, writing, seeking, flushing, and durability remain the `std.fs.File` contract rather than a separate `std.tempfile` contract.

### Cleanup semantics

- A non-persisted temporary file must be removed when its owning temp handle is dropped.
- A non-persisted temporary directory must remove its directory tree when its owning handle is dropped.
- Cleanup failures during explicit operations must surface as ordinary `Result` failures.
- Cleanup failures during drop must not panic or abort ordinary control flow. The stdlib docs must describe how such failures are reported, logged, or intentionally ignored on each supported target.
- If the host OS refuses to delete or rename a temporary file because another handle still has it open, the operation must fail with an actionable filesystem error. Incan must not promise cross-platform deletion of open files.

### Filesystem interaction

- Temporary resources are ordinary filesystem entries while they exist.
- Existing path-based APIs can consume `temp.path()` without any special cases.
- Persisting a resource yields a normal path that remains after the temp handle is gone.
- `std.tempfile` depends on `std.fs` for path vocabulary. It must not become a second home for ordinary path operations.
- Spill-to-disk buffering belongs in a later `SpooledTemporaryFile` design under `std.tempfile`, not in `std.io.BytesIO`.

### Documentation contract

The implementation must ship authored user documentation with the feature. RFC text and release notes are not enough.

Required docs:

- A dedicated `std.tempfile` reference page under the standard-library reference section.
- Stdlib reference index and docs navigation entries for `std.tempfile`.
- Task-oriented guidance showing ordinary workflows: scratch test directories, temporary staging files, explicit `persist()`, caller-provided parent directories, and cleanup behavior on early returns.
- Cross-links from the filesystem docs once `std.fs` exists, so users understand that `std.tempfile` owns lifecycle while `std.fs.Path` / `std.fs.File` own ordinary file operations.
- Release notes for the release that ships the module.

The reference page must document constructor parameters, return types, `path()`, `persist()`, cleanup semantics, failure behavior, platform caveats around open handles, and the intentionally deferred `TemporaryFile` / `SpooledTemporaryFile` names.

## Design details

### Why Python-style naming

The naming question is settled in favor of Python's `tempfile` family. The public stdlib should optimize for familiarity at the Incan layer even if the backing implementation uses shorter or differently named runtime types underneath.

### Why types instead of bare helper functions

Using dedicated temp-handle types keeps lifetime and cleanup tied together. A raw helper like `create_temp_file() -> Path` would push the burden back onto callers, who would then need to remember cleanup manually.

### Why `std.tempfile` is separate from `std.fs`

`std.fs` owns ordinary path and file operations. `std.tempfile` owns the lifecycle policy for scratch filesystem objects: safe creation, automatic cleanup, and explicit persistence. Keeping those concerns separate matches RFC 055's path-centric filesystem model without turning `Path` into a policy bucket for every resource lifecycle pattern.

### Why `TemporaryFile` is deferred

Python's `TemporaryFile` can be nameless or not durably addressable depending on platform behavior. That is useful, but it is a poorer first target for Incan than path-addressable temporary files and directories because the current stdlib direction centers ordinary filesystem work on `std.fs.Path`. A follow-up RFC may add `TemporaryFile` once the open-file and stream contracts are mature enough to make pathless temporary storage portable and teachable.

### Why `SpooledTemporaryFile` is deferred

`SpooledTemporaryFile` crosses the boundary between in-memory buffering and temporary filesystem storage. RFC 056 deliberately keeps spill-to-disk behavior out of `std.io.BytesIO`; this RFC keeps the matching boundary by reserving `SpooledTemporaryFile` for a later design that can align with both `BytesIO` and `std.fs.File`.

### Interaction with existing features

- Testing benefits immediately because scratch files and directories are a common fixture pattern.
- Error handling composes naturally because cleanup should still happen when functions return early with `?`.
- `std.fs.Path` and `std.fs.File` remain the ordinary filesystem vocabulary; temp handles provide lifecycle ownership around those values.
- The backend can map the feature to a Rust temp-resource implementation, but the language contract is about lifecycle and behavior, not about a specific Rust crate.

### Compatibility / migration

This feature is additive. Existing `Path` and filesystem APIs keep their meaning.

## Alternatives considered

1. **Manual create-and-delete helpers**
   - Too easy to misuse, especially on error paths.

2. **Context-manager-only surface**
   - Incan does not need a new control-flow surface just to make temporary resources safe.

3. **Abbreviated names such as `TempFile` / `TempDir`**
   - Shorter, but they give up the Python-aligned naming that this RFC explicitly wants for the Incan stdlib surface.

4. **Fold temporary-resource helpers into `std.fs.Path`**
   - Rejected because temporary resources are lifecycle-managed values, not ordinary path operations.

5. **Include `TemporaryFile` and `SpooledTemporaryFile` immediately**
   - Rejected because both require adjacent stream/file contracts that should not be smuggled into the initial `std.tempfile` implementation.

## Drawbacks

- Temporary-resource cleanup semantics vary subtly across operating systems, especially around open handles.
- The Python-style surface may not map one-to-one onto the backing runtime's naming or exact semantics, so the docs must be explicit about where Incan intentionally differs.
- Users may overuse temp files where in-memory buffers would be simpler or faster.
- Deferring `TemporaryFile` and `SpooledTemporaryFile` leaves some Python `tempfile` workflows for follow-up work.

## Layers affected

- **Stdlib / runtime**: must define `std.tempfile`, implement safe temporary-resource creation, and document cleanup semantics.
- **Stdlib registry / typechecker**: must expose the module and its typed constructors/methods through the normal stdlib namespace machinery.
- **Lowering / emission**: must preserve cleanup and persistence behavior across success and error paths, including code paths that return early with `?`.
- **Docs / examples**: must add a dedicated `std.tempfile` reference page plus task-oriented user guidance; release notes and RFC edits alone do not satisfy this layer.
- **Tests / tooling**: should cover creation, persistence, cleanup, and discoverability through stdlib imports.

## Implementation Plan

### Phase 1: Stdlib surface and registry

- Add `std.tempfile` to the standard-library registry so imports, hints, LSP, and stub loading follow the same path as other stdlib modules.
- Define authored Incan declarations for `NamedTemporaryFile` and `TemporaryDirectory`, returning `std.fs.Path` and `std.fs.IoError` through the normal stdlib loader.
- Keep `TemporaryFile` and `SpooledTemporaryFile` out of the exported implementation surface.

### Phase 2: Runtime behavior

- Implement safe creation for named temporary files and temporary directories in the system temp location or a caller-provided parent directory.
- Implement `path()` and `persist()` for both handle types.
- Ensure non-persisted handles clean up on drop without panicking ordinary control flow.
- Preserve host-sensitive errors for explicit creation, persistence, and filesystem operations.

### Phase 3: Verification

- Add registry, typechecker, codegen snapshot, and end-to-end integration tests for `std.tempfile`.
- Cover cleanup, persistence, prefix/suffix, caller-provided parent directories, and the fact that temporary handles expose `std.fs.Path` rather than raw strings or `std.web.Path`.
- Preserve existing `std.fs` behavior from RFC 055.

### Phase 4: Docs and release

- Add a dedicated `std.tempfile` standard-library reference page.
- Add task-oriented file I/O guidance for scratch directories, staging files, persistence, caller-provided parents, and cleanup behavior.
- Cross-link `std.fs` and `std.tempfile` so lifecycle and ordinary path/file operations are clearly separated.
- Add release notes for the active development release.

## Progress Checklist

### Spec / lifecycle

- [x] Settle initial surface as `NamedTemporaryFile` and `TemporaryDirectory`.
- [x] Defer `TemporaryFile` and `SpooledTemporaryFile`.
- [x] Record dependency on RFC 055 `std.fs`.
- [ ] Move RFC 010 to Implemented once all implementation and docs work is complete.

### Stdlib / registry

- [x] Register `std.tempfile` in the standard-library namespace registry.
- [x] Add authored `stdlib/tempfile.incn` declarations.
- [x] Ensure imported `std.tempfile` types resolve through stdlib AST loading.
- [x] Ensure `path()` returns `std.fs.Path`.

### Runtime behavior

- [x] Create `NamedTemporaryFile` with default and configured `prefix`, `suffix`, and `dir` factories.
- [x] Create `TemporaryDirectory` with default and configured `prefix`, `suffix`, and `dir` factories.
- [x] Remove non-persisted named temporary files on drop.
- [x] Remove non-persisted temporary directories recursively on drop.
- [x] Persist named temporary files and temporary directories as ordinary `std.fs.Path` values.
- [x] Surface explicit creation and persistence failures as filesystem errors.

### Tests

- [x] Registry/unit tests cover `std.tempfile` import discovery and hints.
- [x] Codegen snapshot verifies `std.tempfile` imports and `std.fs.Path` usage.
- [x] Integration test covers create, path usage, cleanup, and persistence for named temporary files.
- [x] Integration test covers create, path usage, cleanup, and persistence for temporary directories.
- [ ] Regression coverage confirms `TemporaryFile` and `SpooledTemporaryFile` are not exported in the initial surface; currently blocked by import-validation bug [#499](https://github.com/dannys-code-corner/incan/issues/499).

### Docs

- [x] Add `std.tempfile` reference page.
- [x] Add stdlib index and MkDocs navigation entries.
- [x] Add task-oriented tempfile guidance to file I/O docs.
- [x] Cross-link `std.fs` and `std.tempfile`.
- [x] Add release notes entry.

## Design Decisions

1. Direct class construction and `.new()` remain infallible constructor conventions; temporary resource acquisition uses explicit `try_new(...)` factories.
2. Python-style type names `NamedTemporaryFile` and `TemporaryDirectory` are part of the required public surface.
3. The implementation lives in `std.tempfile`; temporary-resource APIs must not be compiler builtins or hidden test-runner utilities.
4. Temporary resources remain path-usable filesystem entries while they exist; this RFC does not invent a separate non-`Path` interaction model for them.
5. The initial implementation is path-addressable only. `TemporaryFile` is deferred until Incan has a settled pathless open-file model.
6. `SpooledTemporaryFile` is deferred until it can align with `std.io.BytesIO` and `std.fs.File`.
7. Open-handle deletion and rename behavior is host-sensitive. Incan must surface explicit filesystem failures instead of promising impossible cross-platform cleanup guarantees.
8. RFC 010 depends on RFC 055's filesystem path surface for its exact `Path` return contract. It should not be implemented by reusing the unrelated `std.web.Path` route extractor or by quietly substituting raw strings for filesystem paths.
9. `std.tempfile` must not ship as a docs-light stdlib. The implementation is incomplete until it includes a dedicated stdlib reference page, navigation/index updates, task-oriented usage guidance, and release notes.
