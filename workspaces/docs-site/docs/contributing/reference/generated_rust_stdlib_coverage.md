# Generated Rust stdlib coverage inventory

This inventory tracks generated Rust coverage for every `crates/incan_stdlib/stdlib/**/*.incn` source module. It is a maintenance aid for deciding where generated stdlib Rust needs stronger tests; it is not a claim that the runtime behavior of every exported API is exhaustively covered.

Generated from repo inspection on 2026-05-20 with:

```sh
rg --files crates/incan_stdlib/stdlib | rg '\.incn$'
rg -n 'std_|from std\.|import std' tests tests/codegen_snapshots crates/incan_stdlib/tests
```

## Coverage labels

| Label | Meaning |
| --- | --- |
| `snapshot-covered` | A codegen snapshot directly records generated Rust from the stdlib source module or from a focused user import that asserts generated Rust shape. |
| `compile-only-covered` | A test directly generates Rust from the module and asserts it compiles or emits an Incan-generated Rust artifact, but does not snapshot the generated Rust. |
| `import/user-facing-covered` | An `incan run`, fixture, or import-focused test exercises generated Rust through the public stdlib surface, but does not snapshot that module's generated Rust. |
| `indirect-only` | The module is pulled in by another covered module or prelude, but no test directly targets its generated Rust or user-facing surface. |
| `missing` | No current test evidence was found for generated Rust coverage. |

## Inventory

### Root modules

| Source file | Module | Current coverage | Evidence | Recommended next test |
| --- | --- | --- | --- | --- |
| `stdlib/collections.incn` | `std.collections` | `import/user-facing-covered` | `tests/fixtures/rfc030_std_collections_behavior.incn`, `std_ordinal_map_surface`, `ordinal_key_builtin_impls`, `ordinal_map_str_fast_lookup`, and layering guards. | Add a direct generated-Rust snapshot for the module source or a focused snapshot for the most important generated helpers. |
| `stdlib/result.incn` | `std.result` | `snapshot-covered` | `stdlib_generated_rust_snapshot_tests::std_result_source_snapshot` snapshots direct source compilation; `test_std_result_helpers_compile_and_run` and Result method dogfood tests run helper calls through generated projects. | Keep direct source snapshot aligned when Result helper lowering changes. |
| `stdlib/prelude.incn` | `std` prelude | `snapshot-covered` | `stdlib_generated_rust_snapshot_tests::std_root_prelude_import_snapshot` snapshots representative root prelude re-exports for derive, conversion, ops, error, indexing, and callable traits. | Add more imported trait families only when the prelude surface expands. |
| `stdlib/logging.incn` | `std.logging` | `import/user-facing-covered` | Multiple integration tests run `basic_config`, `get_logger`, ambient `log`, JSON rendering, invalid logger names, and structured fields. | Add one generated-Rust snapshot for a minimal `std.logging` import to guard emitted module wiring. |
| `stdlib/testing.incn` | `std.testing` | `snapshot-covered` | `test_std_testing_compiled_codegen` snapshots direct module compilation; many CLI and integration tests exercise assertions, fixtures, parametrization, marks, resources, and skips. | Keep as-is unless new decorators/helpers are added. |
| `stdlib/math.incn` | `std.math` | `snapshot-covered` | `std_math` codegen snapshot plus `test_std_math_module_constants_and_functions_run` and numeric-like helper runtime tests. | Add missing function cases only when public math surface expands. |
| `stdlib/graph.incn` | `std.graph` | `snapshot-covered` | `test_std_graph_compiled_codegen`, `std_graph_import`, and `std_graph_surface` cover declarations, import lowering, constructors, DAGs, and multigraph edge IDs. | Keep import fixture aligned with any new graph types or methods. |
| `stdlib/reflection.incn` | `std.reflection` | `import/user-facing-covered` | `tests/fixtures/valid/std_reflection_import.incn`, `field_info_reflection.incn`, and missing-import diagnostics cover public import and compiler reflection behavior. | Add a generated-Rust snapshot for `FieldInfo` import/use. |
| `stdlib/this.incn` | `std.this` | `missing` | No direct references found in tests or fixtures. | Add either a compile-only source test if this is internal glue, or delete/deprecate if unused. |
| `stdlib/uuid.incn` | `std.uuid` | `snapshot-covered` | `test_std_uuid_compiled_codegen`, `std_uuid_import`, `std_uuid_surface`, and layering guards cover source-defined UUID generation/imports and absence of Rust-backed UUID type. | Keep as-is unless new UUID versions or formatting helpers are added. |
| `stdlib/io.incn` | `std.io` | `snapshot-covered` | `stdlib_generated_rust_snapshot_tests::std_io_source_snapshot` snapshots direct source compilation; `test_std_io_compile_and_run_bytesio_core_and_numeric_helpers` exercises `BytesIO` and numeric helpers at runtime; fs, hash, compression, encoding, uuid, and tempfile tests also import it. | Keep direct source snapshot aligned when `BytesIO` or binary reader/writer lowering changes. |
| `stdlib/json.incn` | `std.json` | `import/user-facing-covered` | `test_std_json_value_indexing_emits_checked_helpers`, JSON deserialize/value runtime tests, and serde integration tests. | Add a snapshot covering `JsonValue` constructors plus object/array indexing. |
| `stdlib/tempfile.incn` | `std.tempfile` | `snapshot-covered` | `std_tempfile_import` snapshot and `test_std_tempfile_compile_and_run_named_file_and_directory`. | Keep as-is; add new runtime cases when persistence or cleanup semantics change. |

### `std.async`

| Source file | Module | Current coverage | Evidence | Recommended next test |
| --- | --- | --- | --- | --- |
| `stdlib/async/prelude.incn` | `std.async` | `snapshot-covered` | `stdlib_generated_rust_snapshot_tests::std_async_prelude_import_snapshot` snapshots representative public async imports; task/time/channel/sync/race modules are covered individually. | Add more re-export names only when the public async prelude expands. |
| `stdlib/async/task.incn` | `std.async.task` | `snapshot-covered` | `test_std_async_task_compiled_codegen`, async task/time wrapper fixture, and spawn runtime tests. | Keep as-is. |
| `stdlib/async/time.incn` | `std.async.time` | `snapshot-covered` | `test_std_async_time_compiled_codegen`, timeout/sleep wrapper fixture, race helper tests, and runtime timeout tests. | Keep as-is. |
| `stdlib/async/channel.incn` | `std.async.channel` | `snapshot-covered` | `test_std_async_channel_compiled_codegen` plus channel runtime tests using `channel`, `unbounded_channel`, and `oneshot`. | Keep as-is. |
| `stdlib/async/sync.incn` | `std.async.sync` | `snapshot-covered` | `test_std_async_sync_compiled_codegen` plus runtime tests for `Mutex`, `RwLock`, `Semaphore`, and `Barrier`. | Keep as-is. |
| `stdlib/async/race.incn` | `std.async.race` | `snapshot-covered` | `test_std_async_race_compiled_codegen`, `race_for_expression_codegen`, and helper runtime tests. | Keep as-is. |

### `std.compression`

| Source file | Module | Current coverage | Evidence | Recommended next test |
| --- | --- | --- | --- | --- |
| `stdlib/compression/prelude.incn` | `std.compression` | `snapshot-covered` | `stdlib_generated_rust_snapshot_tests::std_compression_prelude_source_snapshot` snapshots direct prelude source compilation; `test_std_compression_modules_compile_codegen` includes this file; `std_compression_surface` runs the public surface. | Keep as-is unless public compression re-exports change. |
| `stdlib/compression/_core.incn` | `std.compression._core` | `snapshot-covered` | `stdlib_generated_rust_snapshot_tests::std_compression_core_source_snapshot` snapshots direct core source compilation; direct compile loop and public compression surface tests cover all codecs. | Add codec-specific snapshots only if per-codec lowering diverges from shared core helpers. |
| `stdlib/compression/_auto.incn` | `std.compression._auto` | `compile-only-covered` | Direct compile loop plus `decompress_auto` and `decompress_auto_stream` in the surface fixture. | Add snapshot or runtime cases for ambiguous/unsupported header paths. |
| `stdlib/compression/gzip.incn` | `std.compression.gzip` | `compile-only-covered` | Direct compile loop and surface fixture. | Add per-codec generated-Rust snapshot only if codec lowering diverges. |
| `stdlib/compression/zlib.incn` | `std.compression.zlib` | `compile-only-covered` | Direct compile loop and surface fixture. | Same as gzip. |
| `stdlib/compression/deflate.incn` | `std.compression.deflate` | `compile-only-covered` | Direct compile loop and surface fixture. | Same as gzip. |
| `stdlib/compression/zstd.incn` | `std.compression.zstd` | `compile-only-covered` | Direct compile loop and surface fixture. | Same as gzip. |
| `stdlib/compression/bz2.incn` | `std.compression.bz2` | `compile-only-covered` | Direct compile loop and surface fixture. | Same as gzip. |
| `stdlib/compression/lzma.incn` | `std.compression.lzma` | `compile-only-covered` | Direct compile loop and surface fixture. | Same as gzip. |
| `stdlib/compression/snappy.incn` | `std.compression.snappy` | `compile-only-covered` | Direct compile loop and surface fixture. | Same as gzip. |
| `stdlib/compression/snappy/raw.incn` | `std.compression.snappy.raw` | `compile-only-covered` | Direct compile loop and surface fixture imports raw snappy compress/decompress. | Add a small generated-Rust assertion for raw module import path if this remains a nested module. |

### `std.datetime`

| Source file | Module | Current coverage | Evidence | Recommended next test |
| --- | --- | --- | --- | --- |
| `stdlib/datetime/prelude.incn` | `std.datetime` | `snapshot-covered` | `stdlib_generated_rust_snapshot_tests::std_datetime_prelude_import_snapshot` snapshots representative public datetime re-exports; `std_datetime_surface` imports and runs `std.datetime` names. | Add more re-export names only when the public datetime prelude expands. |
| `stdlib/datetime/runtime.incn` | `std.datetime.runtime` | `import/user-facing-covered` | `test_std_datetime_surface_runs_with_std_time_runtime_boundary` reads this file and asserts the Rust `std::time` boundary before running the surface fixture. | Add a snapshot for `Instant`, `Duration`, and `SystemTime` generated Rust if boundary churn continues. |
| `stdlib/datetime/error.incn` | `std.datetime.error` | `indirect-only` | Used by runtime/civil modules and exercised through error cases in `std_datetime_surface`; no direct generated-Rust target found. | Add direct compile or import snapshot for `DateTimeError`. |
| `stdlib/datetime/civil.incn` | `std.datetime.civil` | `import/user-facing-covered` | `std_datetime_surface` reads and runs the civil aggregate with calendar, parsing, formatting, and offset cases. | Add snapshot for aggregate re-exports. |
| `stdlib/datetime/civil/intervals.incn` | `std.datetime.civil.intervals` | `import/user-facing-covered` | Included by the datetime surface test's civil directory read and used by `TimeDelta`, `YearMonthInterval`, and `DateTimeInterval` fixture cases. | Add direct source snapshot if interval generated code changes often. |
| `stdlib/datetime/civil/naive.incn` | `std.datetime.civil.naive` | `snapshot-covered` | `imported_stdlib_value_fragment` snapshots an import from this module; `std_datetime_surface` covers dates, times, parsing, formatting, and ordinal helpers. | Broaden snapshot coverage beyond one imported value if needed. |
| `stdlib/datetime/civil/offset.incn` | `std.datetime.civil.offset` | `import/user-facing-covered` | Included by datetime surface fixture through `DateTimeOffset` cases. | Add direct import snapshot for offset parsing/formatting names. |

### `std.derives`

| Source file | Module | Current coverage | Evidence | Recommended next test |
| --- | --- | --- | --- | --- |
| `stdlib/derives/comparison.incn` | `std.derives.comparison` | `snapshot-covered` | `test_std_derives_comparison_compiled_codegen`. | Keep as-is. |
| `stdlib/derives/copying.incn` | `std.derives.copying` | `snapshot-covered` | `test_std_derives_copying_compiled_codegen`. | Keep as-is. |
| `stdlib/derives/string.incn` | `std.derives.string` | `snapshot-covered` | `test_std_derives_string_compiled_codegen`. | Keep as-is. |
| `stdlib/derives/collection.incn` | `std.derives.collection` | `snapshot-covered` | `test_std_derives_collection_compiled_codegen`. | Keep as-is. |

### `std.encoding`

| Source file | Module | Current coverage | Evidence | Recommended next test |
| --- | --- | --- | --- | --- |
| `stdlib/encoding/prelude.incn` | `std.encoding` | `snapshot-covered` | `stdlib_generated_rust_snapshot_tests::std_encoding_prelude_import_snapshot` snapshots representative public prelude imports for family modules and `EncodingError`; `rfc064_std_encoding_behavior` imports the public prelude; algorithm modules are covered individually. | Add more family imports only when the public encoding prelude expands. |
| `stdlib/encoding/_shared.incn` | `std.encoding._shared` | `indirect-only` | Imported by all algorithm modules and covered through their tests; no direct source/import target found. | Add a compact compile test for `EncodingError` and shared helpers. |
| `stdlib/encoding/hex.incn` | `std.encoding.hex` | `import/user-facing-covered` | `std_encoding_hex_surface` fixture and RFC 064 encoding behavior fixture. | Add direct module-source runtime coverage like the other algorithms, or a generated-Rust snapshot. |
| `stdlib/encoding/base32.incn` | `std.encoding.base32` | `import/user-facing-covered` | `tests/std_encoding_algorithm_modules.rs` runs module source with vector and lenient decode assertions; RFC 064 fixture also imports it. | Add snapshot only if generated helper shape needs review. |
| `stdlib/encoding/base58.incn` | `std.encoding.base58` | `import/user-facing-covered` | `tests/std_encoding_algorithm_modules.rs` and RFC 064 fixture. | Same as base32. |
| `stdlib/encoding/base64.incn` | `std.encoding.base64` | `import/user-facing-covered` | `tests/std_encoding_algorithm_modules.rs` and RFC 064 fixture. | Same as base32. |
| `stdlib/encoding/base85.incn` | `std.encoding.base85` | `import/user-facing-covered` | `tests/std_encoding_algorithm_modules.rs` and RFC 064 fixture. | Same as base32. |
| `stdlib/encoding/bech32.incn` | `std.encoding.bech32` | `import/user-facing-covered` | `tests/std_encoding_algorithm_modules.rs` and RFC 064 fixture. | Same as base32. |

### `std.fs`

| Source file | Module | Current coverage | Evidence | Recommended next test |
| --- | --- | --- | --- | --- |
| `stdlib/fs/prelude.incn` | `std.fs` | `snapshot-covered` | `std_fs_import` snapshot and `test_std_fs_compile_and_run_path_file_and_tree_operations`. | Keep as-is. |
| `stdlib/fs/path.incn` | `std.fs.path` | `import/user-facing-covered` | `std.fs` integration test exercises paths, globbing, reads/writes, copy/move/touch/stat, and tree removal. | Add direct source snapshot for `Path` methods if generated method shape is important. |
| `stdlib/fs/file.incn` | `std.fs.file` | `import/user-facing-covered` | `std.fs` integration test exercises open modes, readers/writers, encodings, `OpenOptions`, and byte/text operations. | Add direct source snapshot for file/open option definitions. |
| `stdlib/fs/metadata.incn` | `std.fs.metadata` | `import/user-facing-covered` | `std.fs` integration test exercises `stat`, `modified_unix`, `disk_usage`, and directory entries. | Add snapshot if metadata models change. |
| `stdlib/fs/glob.incn` | `std.fs.glob` | `import/user-facing-covered` | `test_std_fs_glob_string_api_compile_and_run` and `Path.glob`/`Path.rglob` integration coverage. | Add direct generated-Rust assertion for glob helpers only if matching semantics move. |

### `std.hash`

| Source file | Module | Current coverage | Evidence | Recommended next test |
| --- | --- | --- | --- | --- |
| `stdlib/hash/prelude.incn` | `std.hash` | `import/user-facing-covered` | `test_std_hash_compile_and_run_digest_file_and_error_paths` imports digest and streaming helpers. | Add generated-Rust snapshot for public hash prelude re-exports. |
| `stdlib/hash/_core.incn` | `std.hash._core` | `import/user-facing-covered` | Covered through digest calls for MD5, SHA, SHA3, Blake, Shake, and xxhash in the hash integration test. | Add direct compile/snapshot for core digest wrappers. |
| `stdlib/hash/_streaming.incn` | `std.hash._streaming` | `import/user-facing-covered` | Covered through `file_digest`, `reader_digest`, and typed file/reader hash helpers in the hash integration test. | Add direct compile/snapshot for streaming helper lowering. |

### `std.regex`

| Source file | Module | Current coverage | Evidence | Recommended next test |
| --- | --- | --- | --- | --- |
| `stdlib/regex/prelude.incn` | `std.regex` | `import/user-facing-covered` | `std_regex_surface`, constructor-hook codegen test, and layering guard cover public imports and source-owned behavior. | Add snapshot for public prelude import lowering. |
| `stdlib/regex/_core.incn` | `std.regex._core` | `import/user-facing-covered` | `std_regex_surface` exercises constructors and matching; layering guard checks regex engine construction remains in source. | Add direct source snapshot for core generated Rust. |
| `stdlib/regex/types.incn` | `std.regex.types` | `import/user-facing-covered` | `std_regex_surface` exercises `Captures`, `Match`, and iterators; layering guard includes this file. | Add direct snapshot if iterator generated code changes. |
| `stdlib/regex/_replacement.incn` | `std.regex._replacement` | `import/user-facing-covered` | `std_regex_surface` and layering guard cover replacement helpers staying in Incan source. | Add explicit runtime replacement edge-case test if replacement syntax expands. |

### `std.serde`

| Source file | Module | Current coverage | Evidence | Recommended next test |
| --- | --- | --- | --- | --- |
| `stdlib/serde/prelude.incn` | `std.serde` | `snapshot-covered` | `stdlib_generated_rust_snapshot_tests::std_serde_prelude_import_snapshot` snapshots `from std.serde import json` derive resolution; existing user fixtures also import `from std.serde import json`. | Keep as-is unless additional serde prelude re-exports are added. |
| `stdlib/serde/json.incn` | `std.serde.json` | `snapshot-covered` | `test_std_serde_json_compiled_codegen`, `std_serde_json_import`, `std_serde_with_serialize_trait`, module derive snapshots, and JSON runtime tests. | Keep as-is. |

### `std.telemetry`

| Source file | Module | Current coverage | Evidence | Recommended next test |
| --- | --- | --- | --- | --- |
| `stdlib/telemetry/prelude.incn` | `std.telemetry` | `snapshot-covered` | `stdlib_generated_rust_snapshot_tests::std_telemetry_prelude_import_snapshot` snapshots representative public telemetry re-exports; logging tests import `std.telemetry.core`. | Add runtime smoke once telemetry provider APIs become user-facing. |
| `stdlib/telemetry/core.incn` | `std.telemetry.core` | `snapshot-covered` | `stdlib_generated_rust_snapshot_tests::std_telemetry_core_source_snapshot` snapshots direct core source compilation; logging JSON structured-field tests import `TelemetryValue`; logging source imports telemetry core types. | Keep direct source snapshot aligned when telemetry data models expand. |

### `std.traits`

| Source file | Module | Current coverage | Evidence | Recommended next test |
| --- | --- | --- | --- | --- |
| `stdlib/traits/prelude.incn` | `std.traits` | `snapshot-covered` | `test_std_traits_prelude_compiled_codegen`. | Keep as-is. |
| `stdlib/traits/ops.incn` | `std.traits.ops` | `snapshot-covered` | `test_std_traits_ops_compiled_codegen`. | Keep as-is. |
| `stdlib/traits/error.incn` | `std.traits.error` | `snapshot-covered` | `test_std_traits_error_compiled_codegen`; also used by error-bearing stdlib modules. | Keep as-is. |
| `stdlib/traits/indexing.incn` | `std.traits.indexing` | `snapshot-covered` | `test_std_traits_indexing_compiled_codegen` and `JsonValue` indexing generated-Rust assertions. | Keep as-is. |
| `stdlib/traits/callable.incn` | `std.traits.callable` | `snapshot-covered` | `test_std_traits_callable_compiled_codegen` and callable object Result tests. | Keep as-is. |
| `stdlib/traits/convert.incn` | `std.traits.convert` | `snapshot-covered` | `test_std_traits_convert_compiled_codegen`, `std_traits_convert_usage` snapshot, and runtime usage test. | Keep as-is. |

### `std.web`

| Source file | Module | Current coverage | Evidence | Recommended next test |
| --- | --- | --- | --- | --- |
| `stdlib/web/prelude.incn` | `std.web` | `snapshot-covered` | `stdlib_generated_rust_snapshot_tests::std_web_prelude_import_snapshot` snapshots representative public web prelude imports including route macro wiring; existing public web import snapshots exercise re-exported names. | Add more web prelude imports only when the public route/request/response surface expands. |
| `stdlib/web/app.incn` | `std.web.app` | `import/user-facing-covered` | `std_web_routing_compiled`, web route extractor snapshots, and app route codegen tests. | Add direct source snapshot if `App` shape changes. |
| `stdlib/web/request.incn` | `std.web.request` | `import/user-facing-covered` | `web_route_extractors`, nested route extractor snapshots, and `newtype_from_request`. | Add source snapshot for `Query`/`Path` extractor models. |
| `stdlib/web/response.incn` | `std.web.response` | `import/user-facing-covered` | `newtype_web_response`, web route extractor snapshots, and response wrapper codegen. | Add source snapshot for `Json`, `Html`, and `Response` if response semantics change. |
| `stdlib/web/routing.incn` | `std.web.routing` | `snapshot-covered` | `std_web_routing_compiled`, route extractor snapshots, and route invalid-usage tests. | Keep as-is. |
| `stdlib/web/macros.incn` | `std.web.macros` | `import/user-facing-covered` | Used by response/request extractor and route snapshots through `IntoResponse` and `FromRequestParts`. | Add direct compile/snapshot for macro marker traits. |

## Representative gaps

- Prelude and re-export modules now have representative generated-Rust snapshots for `std`, `std.async`, `std.datetime`, `std.encoding`, `std.serde`, `std.telemetry`, and `std.web`. Future gaps should focus on newly added re-exports rather than duplicating every imported name.
- `stdlib/this.incn` has no test evidence in current repo searches. It needs an owner decision: add direct coverage if it is intentionally shipped, or remove/deprecate it if it is stale.
- Runtime/user-facing smoke tests are strong for `std.fs`, `std.hash`, `std.logging`, and `std.datetime`, but most of those still do not preserve generated Rust shape. `std.io` and `std.result` now have direct source snapshots; for additional generated Rust regressions, add compact snapshots instead of broad runtime-only tests.
- `std.compression` has direct compile checks for every module plus representative snapshots for the public prelude and shared core. Add codec-specific snapshots only if codec lowering starts to diverge.

## Recommended next tests

1. Add one direct generated-Rust snapshot each for `std.hash`, `std.json`, and `std.logging`, because their runtime coverage is useful but does not review emitted Rust shape.
2. Consider direct snapshots for `std.fs.path`, `std.fs.file`, `std.regex._core`, and selected datetime civil modules only if those generated shapes begin changing frequently.
3. Decide whether `stdlib/this.incn` is intentional. If yes, add a compile-only or snapshot test that names its expected purpose.
4. For `std.compression`, keep the current representative snapshots scoped to public prelude and shared core unless per-codec lowering needs direct review.
