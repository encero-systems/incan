# RFC 010: Python-style `tempfile` standard library

- **Status:** Draft
- **Created:** 2024-12-11
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:** RFC 019 (runner testing), RFC 023 (stdlib namespacing and compiler handoff)
- **Issue:** https://github.com/dannys-code-corner/incan/issues/79
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

- Requiring an exact one-to-one clone of every Python `tempfile` behavior in the first version.
- Context-manager syntax just for temporary resources.
- Defining every possible OS-specific temporary-file flag or security knob in the initial RFC.
- Replacing ordinary `Path` and filesystem APIs.

## Guide-level explanation (how users think about it)

### Named temporary files

```incan
from std.tempfile import NamedTemporaryFile

temp = NamedTemporaryFile()?
temp.write_text("some data")?

process_file(temp.path())
# file is deleted when `temp` goes out of scope unless it is persisted
```

### Temporary directories

```incan
from std.tempfile import TemporaryDirectory

temp_dir = TemporaryDirectory()?

config = temp_dir.path() / "config.toml"
config.write_text(default_config)?

data_dir = temp_dir.path() / "data"
data_dir.mkdir()?
```

When the `TemporaryDirectory` value is dropped, the temporary directory tree is removed.

### Keeping a result

```incan
from std.tempfile import NamedTemporaryFile

temp = NamedTemporaryFile(suffix=".json")?
temp.write_text(data)?

final_path = temp.persist()?
println(f"saved to {final_path}")
```

`persist()` converts a temporary resource into an ordinary path that will no longer be auto-deleted by the temp handle.

## Reference-level explanation (precise rules)

### Surface

The stdlib provides temporary filesystem types through `std.tempfile`. The naming direction is part of the contract:

- `NamedTemporaryFile`
- `TemporaryDirectory`
- `TemporaryFile`
- `SpooledTemporaryFile`

Not every member of that family must land with identical maturity on day one, but the public naming should align with that Python-style vocabulary.

### Required capabilities

- Create a named temporary file in the system temp directory.
- Create a temporary directory in the system temp directory.
- Create either one under a caller-provided parent directory.
- Expose the realized `Path` where applicable.
- Persist the resource so automatic cleanup no longer runs.

### Cleanup semantics

- A non-persisted temporary file must be removed when its owning temp handle is dropped.
- A non-persisted temporary directory must remove its directory tree when its owning handle is dropped.
- Cleanup failures must surface as diagnostics or documented runtime errors; the language must not silently claim successful cleanup if the underlying filesystem rejected it.

### Filesystem interaction

- Temporary resources are ordinary filesystem entries while they exist.
- Existing path-based APIs can consume `temp.path()` without any special cases.
- Persisting a resource yields a normal path that remains after the temp handle is gone.

## Design details

### Why Python-style naming

The naming question is settled in favor of Python's `tempfile` family. The public stdlib should optimize for familiarity at the Incan layer even if the backing implementation uses shorter or differently named runtime types underneath.

### Why types instead of bare helper functions

Using dedicated temp-handle types keeps lifetime and cleanup tied together. A raw helper like `create_temp_file() -> Path` would push the burden back onto callers, who would then need to remember cleanup manually.

### Interaction with existing features

- Testing benefits immediately because scratch files and directories are a common fixture pattern.
- Error handling composes naturally because cleanup should still happen when functions return early with `?`.
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

## Drawbacks

- Temporary-resource cleanup semantics vary subtly across operating systems, especially around open handles.
- The Python-style surface may not map one-to-one onto the backing runtime's naming or exact semantics, so the docs must be explicit about where Incan intentionally differs.
- Users may overuse temp files where in-memory buffers would be simpler or faster.

## Layers affected

- **Stdlib**: must define the temporary-resource surface and document cleanup semantics.
- **Typechecker / docs**: must treat temp resources as ordinary typed values with path-returning methods.
- **Lowering / runtime**: must preserve cleanup and persistence behavior across success and error paths.
- **Testing / tooling**: should make examples and diagnostics around temporary resources easy to discover.

## Design Decisions

1. Construction follows ordinary direct Incan construction rather than `.new()`-style factory calls.
2. Python-style type names such as `NamedTemporaryFile` and `TemporaryDirectory` are part of the intended public surface.
3. Temporary resources remain path-usable filesystem entries while they exist; this RFC does not invent a separate non-`Path` interaction model for them.

## Unresolved questions

1. Is `TemporaryFile` without a durable path in scope for the initial version, or is path-addressable temporary storage the only guaranteed surface?
2. Should `SpooledTemporaryFile` be part of the initial RFC, or explicitly deferred?
3. What are the exact cross-platform guarantees when a temp file is still open elsewhere at drop time?

<!-- Rename this section to "Design Decisions" once all questions have been resolved. An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
