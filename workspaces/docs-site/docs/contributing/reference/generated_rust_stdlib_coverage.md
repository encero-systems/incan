# Generated Rust stdlib coverage inventory

This inventory lists the generated-Rust test coverage of each `loaves/stdlib/**/*.incn` source module. It records test evidence; it does not claim that every exported API's runtime behavior is covered.

The inventory was built on 2026-05-20 from these searches, and a change that alters a module's coverage updates its row:

```sh
rg --files loaves/stdlib | rg '\.incn$'
rg -n 'std_|from std\.|import std' loaves/toolchain/incan-cli/tests loaves/compiler/incan_emit/tests/codegen_snapshots loaves/stdlib/core/rust/tests
```

## Coverage labels

| Label | Meaning |
| --- | --- |
| `snapshot-covered` | A codegen snapshot directly records generated Rust from the stdlib source module or from a focused user import that asserts generated Rust shape. |
| `compile-only-covered` | A test directly generates Rust from the module and asserts it compiles or emits an Incan-generated Rust artifact, but does not snapshot the generated Rust. |
| `import/user-facing-covered` | An `incan run`, fixture, or import-focused test exercises generated Rust through the public stdlib surface, without a snapshot of that module's generated Rust. |
| `indirect-only` | The module is pulled in by another covered module or prelude, but no test directly targets its generated Rust or user-facing surface. |
| `missing` | No test evidence of generated-Rust coverage was found. |

## Inventory

### Root modules

| Source file | Module | Coverage | Evidence |
| --- | --- | --- | --- |
| `stdlib/collections.incn` | `std.collections` | `import/user-facing-covered` | `loaves/compiler/incan_test_support/fixtures/rfc030_std_collections_behavior.incn`, `std_ordinal_map_surface`, `ordinal_key_builtin_impls`, `ordinal_map_str_fast_lookup`, and layering guards. |
| `stdlib/result.incn` | `std.result` | `snapshot-covered` | `stdlib_generated_rust_snapshot_tests::std_result_source_snapshot` snapshots direct source compilation; `test_std_result_helpers_compile_and_run` and Result method dogfood tests run helper calls through generated projects. |
| `stdlib/prelude.incn` | `std` prelude | `indirect-only` | The root binds its submodules only, so the prelude has no import surface of its own: `stdlib_generated_rust_snapshot_tests::std_root_prelude_trait_import_is_refused` asserts that importing its traits from `std` is refused, and each trait's generated Rust is covered through its declaring `std.derives.*` or `std.traits.*` module. |
| `stdlib/logging.incn` | `std.logging` | `import/user-facing-covered` | Multiple integration tests run `basic_config`, `get_logger`, ambient `log`, JSON rendering, invalid logger names, and structured fields. |
| `stdlib/testing.incn` | `std.testing` | `snapshot-covered` | `test_std_testing_compiled_codegen` snapshots direct module compilation; many CLI and integration tests exercise assertions, fixtures, parametrization, marks, resources, and skips. |
| `stdlib/math.incn` | `std.math` | `snapshot-covered` | `std_math` codegen snapshot plus `test_std_math_module_constants_and_functions_run` and numeric-like helper runtime tests. |
| `stdlib/graph.incn` | `std.graph` | `snapshot-covered` | `test_std_graph_compiled_codegen`, `std_graph_import`, and `std_graph_surface` cover declarations, import lowering, constructors, DAGs, and multigraph edge IDs. |
| `stdlib/reflection.incn` | `std.reflection` | `import/user-facing-covered` | `loaves/compiler/incan_test_support/fixtures/valid/std_reflection_import.incn`, `field_info_reflection.incn`, and missing-import diagnostics cover public import and compiler reflection behavior. |
| `stdlib/this.incn` | `std.this` | `missing` | No direct references found in tests or fixtures. |
| `stdlib/uuid.incn` | `std.uuid` | `snapshot-covered` | `test_std_uuid_compiled_codegen`, `std_uuid_import`, `std_uuid_surface`, and layering guards cover source-defined UUID generation/imports and absence of Rust-backed UUID type. |
| `stdlib/io.incn` | `std.io` | `snapshot-covered` | `stdlib_generated_rust_snapshot_tests::std_io_source_snapshot` snapshots direct source compilation; `test_std_io_compile_and_run_bytesio_core_and_numeric_helpers` exercises `BytesIO` and numeric helpers at runtime; fs, hash, compression, encoding, uuid, and tempfile tests also import it. |
| `stdlib/json.incn` | `std.json` | `import/user-facing-covered` | `test_std_json_value_indexing_emits_checked_helpers`, JSON deserialize/value runtime tests, and serde integration tests. |
| `stdlib/tempfile.incn` | `std.tempfile` | `snapshot-covered` | `std_tempfile_import` snapshot and `test_std_tempfile_compile_and_run_named_file_and_directory`. |

### `std.async`

| Source file | Module | Coverage | Evidence |
| --- | --- | --- | --- |
| `stdlib/async/prelude.incn` | `std.async` | `snapshot-covered` | `stdlib_generated_rust_snapshot_tests::std_async_prelude_import_snapshot` snapshots representative public async imports; task/time/channel/sync/race modules are covered individually. |
| `stdlib/async/task.incn` | `std.async.task` | `snapshot-covered` | `test_std_async_task_compiled_codegen`, async task/time wrapper fixture, and spawn runtime tests. |
| `stdlib/async/time.incn` | `std.async.time` | `snapshot-covered` | `test_std_async_time_compiled_codegen`, timeout/sleep wrapper fixture, race helper tests, and runtime timeout tests. |
| `stdlib/async/channel.incn` | `std.async.channel` | `snapshot-covered` | `test_std_async_channel_compiled_codegen` plus channel runtime tests using `channel`, `unbounded_channel`, and `oneshot`. |
| `stdlib/async/sync.incn` | `std.async.sync` | `snapshot-covered` | `test_std_async_sync_compiled_codegen` plus runtime tests for `Mutex`, `RwLock`, `Semaphore`, and `Barrier`. |
| `stdlib/async/race.incn` | `std.async.race` | `snapshot-covered` | `test_std_async_race_compiled_codegen`, `race_for_expression_codegen`, and helper runtime tests. |

### `std.compression`

| Source file | Module | Coverage | Evidence |
| --- | --- | --- | --- |
| `stdlib/compression/prelude.incn` | `std.compression` | `snapshot-covered` | `stdlib_generated_rust_snapshot_tests::std_compression_prelude_source_snapshot` snapshots direct prelude source compilation; `test_std_compression_modules_compile_codegen` includes this file; `std_compression_surface` runs the public surface. |
| `stdlib/compression/_core.incn` | `std.compression._core` | `snapshot-covered` | `stdlib_generated_rust_snapshot_tests::std_compression_core_source_snapshot` snapshots direct core source compilation; direct compile loop and public compression surface tests cover all codecs. |
| `stdlib/compression/_auto.incn` | `std.compression._auto` | `compile-only-covered` | Direct compile loop plus `decompress_auto` and `decompress_auto_stream` in the surface fixture. |
| `stdlib/compression/gzip.incn` | `std.compression.gzip` | `compile-only-covered` | Direct compile loop and surface fixture. |
| `stdlib/compression/zlib.incn` | `std.compression.zlib` | `compile-only-covered` | Direct compile loop and surface fixture. |
| `stdlib/compression/deflate.incn` | `std.compression.deflate` | `compile-only-covered` | Direct compile loop and surface fixture. |
| `stdlib/compression/zstd.incn` | `std.compression.zstd` | `compile-only-covered` | Direct compile loop and surface fixture. |
| `stdlib/compression/bz2.incn` | `std.compression.bz2` | `compile-only-covered` | Direct compile loop and surface fixture. |
| `stdlib/compression/lzma.incn` | `std.compression.lzma` | `compile-only-covered` | Direct compile loop and surface fixture. |
| `stdlib/compression/snappy.incn` | `std.compression.snappy` | `compile-only-covered` | Direct compile loop and surface fixture. |
| `stdlib/compression/snappy/raw.incn` | `std.compression.snappy.raw` | `compile-only-covered` | Direct compile loop and surface fixture imports raw snappy compress/decompress. |

### `std.datetime`

| Source file | Module | Coverage | Evidence |
| --- | --- | --- | --- |
| `stdlib/datetime/prelude.incn` | `std.datetime` | `snapshot-covered` | `stdlib_generated_rust_snapshot_tests::std_datetime_prelude_import_snapshot` snapshots representative public datetime re-exports; `std_datetime_surface` imports and runs `std.datetime` names. |
| `stdlib/datetime/runtime.incn` | `std.datetime.runtime` | `import/user-facing-covered` | `test_std_datetime_surface_runs_with_std_time_runtime_boundary` reads this file and asserts the Rust `std::time` boundary before running the surface fixture. |
| `stdlib/datetime/error.incn` | `std.datetime.error` | `indirect-only` | Used by runtime/civil modules and exercised through error cases in `std_datetime_surface`; no direct generated-Rust target found. |
| `stdlib/datetime/civil.incn` | `std.datetime.civil` | `import/user-facing-covered` | `std_datetime_surface` reads and runs the civil aggregate with calendar, parsing, formatting, and offset cases. |
| `stdlib/datetime/civil/intervals.incn` | `std.datetime.civil.intervals` | `import/user-facing-covered` | Included by the datetime surface test's civil directory read and used by `TimeDelta`, `YearMonthInterval`, and `DateTimeInterval` fixture cases. |
| `stdlib/datetime/civil/naive.incn` | `std.datetime.civil.naive` | `snapshot-covered` | `imported_stdlib_value_fragment` snapshots an import from this module; `std_datetime_surface` covers dates, times, parsing, formatting, and ordinal helpers. |
| `stdlib/datetime/civil/offset.incn` | `std.datetime.civil.offset` | `import/user-facing-covered` | Included by datetime surface fixture through `DateTimeOffset` cases. |

### `std.derives`

| Source file | Module | Coverage | Evidence |
| --- | --- | --- | --- |
| `stdlib/derives/comparison.incn` | `std.derives.comparison` | `snapshot-covered` | `test_std_derives_comparison_compiled_codegen`. |
| `stdlib/derives/copying.incn` | `std.derives.copying` | `snapshot-covered` | `test_std_derives_copying_compiled_codegen`. |
| `stdlib/derives/string.incn` | `std.derives.string` | `snapshot-covered` | `test_std_derives_string_compiled_codegen`. |
| `stdlib/derives/collection.incn` | `std.derives.collection` | `snapshot-covered` | `test_std_derives_collection_compiled_codegen`. |

### `std.encoding`

| Source file | Module | Coverage | Evidence |
| --- | --- | --- | --- |
| `stdlib/encoding/prelude.incn` | `std.encoding` | `snapshot-covered` | `stdlib_generated_rust_snapshot_tests::std_encoding_prelude_import_snapshot` snapshots representative public prelude imports for family modules and `EncodingError`; `rfc064_std_encoding_behavior` imports the public prelude; algorithm modules are covered individually. |
| `stdlib/encoding/_shared.incn` | `std.encoding._shared` | `indirect-only` | Imported by all algorithm modules and covered through their tests; no direct source/import target found. |
| `stdlib/encoding/hex.incn` | `std.encoding.hex` | `import/user-facing-covered` | `std_encoding_hex_surface` fixture and the `rfc064_std_encoding_behavior` fixture. |
| `stdlib/encoding/base32.incn` | `std.encoding.base32` | `import/user-facing-covered` | `loaves/toolchain/incan-cli/tests/std_encoding_algorithm_modules.rs` runs module source with vector and lenient decode assertions; the `rfc064_std_encoding_behavior` fixture also imports it. |
| `stdlib/encoding/base58.incn` | `std.encoding.base58` | `import/user-facing-covered` | `loaves/toolchain/incan-cli/tests/std_encoding_algorithm_modules.rs` and the `rfc064_std_encoding_behavior` fixture. |
| `stdlib/encoding/base64.incn` | `std.encoding.base64` | `import/user-facing-covered` | `loaves/toolchain/incan-cli/tests/std_encoding_algorithm_modules.rs` and the `rfc064_std_encoding_behavior` fixture. |
| `stdlib/encoding/base85.incn` | `std.encoding.base85` | `import/user-facing-covered` | `loaves/toolchain/incan-cli/tests/std_encoding_algorithm_modules.rs` and the `rfc064_std_encoding_behavior` fixture. |
| `stdlib/encoding/bech32.incn` | `std.encoding.bech32` | `import/user-facing-covered` | `loaves/toolchain/incan-cli/tests/std_encoding_algorithm_modules.rs` and the `rfc064_std_encoding_behavior` fixture. |

### `std.fs`

| Source file | Module | Coverage | Evidence |
| --- | --- | --- | --- |
| `stdlib/fs/prelude.incn` | `std.fs` | `snapshot-covered` | `std_fs_import` snapshot and `test_std_fs_compile_and_run_path_file_and_tree_operations`. |
| `stdlib/fs/path.incn` | `std.fs.path` | `import/user-facing-covered` | `std.fs` integration test exercises paths, globbing, reads/writes, copy/move/touch/stat, and tree removal. |
| `stdlib/fs/file.incn` | `std.fs.file` | `import/user-facing-covered` | `std.fs` integration test exercises open modes, readers/writers, encodings, `OpenOptions`, and byte/text operations. |
| `stdlib/fs/metadata.incn` | `std.fs.metadata` | `import/user-facing-covered` | `std.fs` integration test exercises `stat`, `modified_unix`, `disk_usage`, and directory entries. |
| `stdlib/fs/glob.incn` | `std.fs.glob` | `import/user-facing-covered` | `test_std_fs_glob_string_api_compile_and_run` and `Path.glob`/`Path.rglob` integration coverage. |

### `std.hash`

| Source file | Module | Coverage | Evidence |
| --- | --- | --- | --- |
| `stdlib/hash/prelude.incn` | `std.hash` | `import/user-facing-covered` | `test_std_hash_compile_and_run_digest_file_and_error_paths` imports digest and streaming helpers. |
| `stdlib/hash/_core.incn` | `std.hash._core` | `import/user-facing-covered` | Covered through digest calls for MD5, SHA, SHA3, Blake, Shake, and xxhash in the hash integration test. |
| `stdlib/hash/_streaming.incn` | `std.hash._streaming` | `import/user-facing-covered` | Covered through `file_digest`, `reader_digest`, and typed file/reader hash helpers in the hash integration test. |

### `std.regex`

| Source file | Module | Coverage | Evidence |
| --- | --- | --- | --- |
| `stdlib/regex/prelude.incn` | `std.regex` | `import/user-facing-covered` | `std_regex_surface`, constructor-hook codegen test, and layering guard cover public imports and source-owned behavior. |
| `stdlib/regex/_core.incn` | `std.regex._core` | `import/user-facing-covered` | `std_regex_surface` exercises constructors and matching; layering guard checks regex engine construction remains in source. |
| `stdlib/regex/types.incn` | `std.regex.types` | `import/user-facing-covered` | `std_regex_surface` exercises `Captures`, `Match`, and iterators; layering guard includes this file. |
| `stdlib/regex/_replacement.incn` | `std.regex._replacement` | `import/user-facing-covered` | `std_regex_surface` and layering guard cover replacement helpers staying in Incan source. |

### `std.serde`

| Source file | Module | Coverage | Evidence |
| --- | --- | --- | --- |
| `stdlib/serde/prelude.incn` | `std.serde` | `snapshot-covered` | `stdlib_generated_rust_snapshot_tests::std_serde_prelude_import_snapshot` snapshots `from std.serde import json` derive resolution; existing user fixtures also import `from std.serde import json`. |
| `stdlib/serde/json.incn` | `std.serde.json` | `snapshot-covered` | `test_std_serde_json_compiled_codegen`, `std_serde_json_import`, `std_serde_with_serialize_trait`, module derive snapshots, and JSON runtime tests. |

### `std.telemetry`

| Source file | Module | Coverage | Evidence |
| --- | --- | --- | --- |
| `stdlib/telemetry/prelude.incn` | `std.telemetry` | `snapshot-covered` | `stdlib_generated_rust_snapshot_tests::std_telemetry_prelude_import_snapshot` snapshots representative public telemetry re-exports; logging tests import `std.telemetry.core`. |
| `stdlib/telemetry/core.incn` | `std.telemetry.core` | `snapshot-covered` | `stdlib_generated_rust_snapshot_tests::std_telemetry_core_source_snapshot` snapshots direct core source compilation; logging JSON structured-field tests import `TelemetryValue`; logging source imports telemetry core types. |

### `std.traits`

| Source file | Module | Coverage | Evidence |
| --- | --- | --- | --- |
| `stdlib/traits/prelude.incn` | `std.traits` | `snapshot-covered` | `test_std_traits_prelude_compiled_codegen`. |
| `stdlib/traits/ops.incn` | `std.traits.ops` | `snapshot-covered` | `test_std_traits_ops_compiled_codegen`. |
| `stdlib/traits/error.incn` | `std.traits.error` | `snapshot-covered` | `test_std_traits_error_compiled_codegen`; also used by error-bearing stdlib modules. |
| `stdlib/traits/indexing.incn` | `std.traits.indexing` | `snapshot-covered` | `test_std_traits_indexing_compiled_codegen` and `JsonValue` indexing generated-Rust assertions. |
| `stdlib/traits/callable.incn` | `std.traits.callable` | `snapshot-covered` | `test_std_traits_callable_compiled_codegen` and callable object Result tests. |
| `stdlib/traits/convert.incn` | `std.traits.convert` | `snapshot-covered` | `test_std_traits_convert_compiled_codegen`, `std_traits_convert_usage` snapshot, and runtime usage test. |

### `std.web`

| Source file | Module | Coverage | Evidence |
| --- | --- | --- | --- |
| `stdlib/web/prelude.incn` | `std.web` | `snapshot-covered` | `stdlib_generated_rust_snapshot_tests::std_web_prelude_import_snapshot` snapshots representative public web prelude imports including route macro wiring; existing public web import snapshots exercise re-exported names. |
| `stdlib/web/app.incn` | `std.web.app` | `import/user-facing-covered` | `std_web_routing_compiled`, web route extractor snapshots, and app route codegen tests. |
| `stdlib/web/request.incn` | `std.web.request` | `import/user-facing-covered` | `web_route_extractors`, nested route extractor snapshots, and `newtype_from_request`. |
| `stdlib/web/response.incn` | `std.web.response` | `import/user-facing-covered` | `newtype_web_response`, web route extractor snapshots, and response wrapper codegen. |
| `stdlib/web/routing.incn` | `std.web.routing` | `snapshot-covered` | `std_web_routing_compiled`, route extractor snapshots, and route invalid-usage tests. |
| `stdlib/web/macros.incn` | `std.web.macros` | `import/user-facing-covered` | Used by response/request extractor and route snapshots through `IntoResponse` and `FromRequestParts`. |
