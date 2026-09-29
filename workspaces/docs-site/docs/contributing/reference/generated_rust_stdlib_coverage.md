# Generated Rust stdlib coverage inventory

This inventory lists the generated-Rust test evidence for every `std.*` module: each `.incn` file under `loaves/stdlib/<component>/src/` other than the component's `lib.incn` provider entrypoint. A row records test evidence; it does not state that every exported API's runtime behavior is covered. Source file paths are relative to `loaves/stdlib/`, and the sections follow the components in `loaves/stdlib/sdk-components.toml`.

Maintaining the inventory is described in [Auditing generated Rust](../how-to/auditing_generated_rust.md#update-the-stdlib-coverage-inventory).

## Coverage labels

| Label | Meaning |
| --- | --- |
| `snapshot-covered` | A codegen snapshot directly records generated Rust from the stdlib source module or from a focused user import that asserts generated Rust shape. |
| `compile-only-covered` | A test directly generates Rust from the module and asserts it compiles or emits an Incan-generated Rust artifact, but does not snapshot the generated Rust. |
| `import/user-facing-covered` | An `incan run`, fixture, or import-focused test exercises generated Rust through the public stdlib surface, without a snapshot of that module's generated Rust. |
| `indirect-only` | The module is pulled in by another covered module or prelude, but no test directly targets its generated Rust or user-facing surface. |
| `missing` | No test evidence of generated-Rust coverage was found. |

## Inventory

### `core`

| Source file | Module | Coverage | Evidence |
| --- | --- | --- | --- |
| `core/src/features.incn` | `std.features` | `missing` | No test imports it. `cli_catalog_forms_tests.rs` reads its `std_datetime_forms` list as text and builds and runs those forms; the compiler-suite foundation closure (`loaves/oven/oven_rustc/src/fixtures/compiler_suite_foundation.incn`) imports it. |
| `core/src/prelude.incn` | `std` prelude | `indirect-only` | The root binds its submodules only, so the prelude has no import surface of its own: `stdlib_generated_rust_snapshot_tests::std_root_prelude_trait_import_is_refused` asserts that importing its traits from `std` is refused, and each trait's generated Rust is covered through its declaring `std.derives.*` or `std.traits.*` module. |
| `core/src/registry.incn` | `std.registry` | `snapshot-covered` | `test_std_registry_import_codegen`, `test_std_registry_methods_codegen`, `test_std_registry_subjects_codegen` and `test_std_registry_type_token_codegen` snapshot user imports; the `snapshots_stdlib` behavior fixtures `std_registry_functions`, `std_registry_methods` and `std_registry_subjects` run the public surface. |
| `core/src/result.incn` | `std.result` | `snapshot-covered` | `stdlib_generated_rust_snapshot_tests::std_result_source_snapshot` snapshots direct source compilation; `test_result_execution_matrix` runs the `std.result` helpers through a generated project. |
| `core/src/reflection.incn` | `std.reflection` | `import/user-facing-covered` | `loaves/compiler/incan_test_support/fixtures/valid/std_reflection_import.incn`, `field_info_reflection.incn`, and missing-import diagnostics cover public import and compiler reflection behavior. |
| `core/src/this.incn` | `std.this` | `missing` | No test targets it. The compiler-suite foundation closure imports it. |
| `core/src/runtime/prelude.incn` | `std.runtime` | `missing` | The `capabilities.rs` typechecker tests and the codegraph unit test `codegraph_projects_a_checked_capability_and_its_requirement` import `host` from it without generating Rust. |
| `core/src/runtime/host.incn` | `std.runtime.host` | `missing` | The `capabilities.rs` typechecker tests import it as `host` without generating Rust; the compiler-suite foundation closure imports it. |
| `core/src/runtime/host/clock.incn` | `std.runtime.host.clock` | `missing` | No test references it. |
| `core/src/runtime/host/env.incn` | `std.runtime.host.env` | `missing` | No test references it. |
| `core/src/runtime/host/fs.incn` | `std.runtime.host.fs` | `missing` | The `capabilities.rs` typechecker tests name `host.fs.read` without generating Rust. |
| `core/src/runtime/host/http.incn` | `std.runtime.host.http` | `missing` | No test references it. |
| `core/src/runtime/host/model.incn` | `std.runtime.host.model` | `missing` | No test references it. |
| `core/src/runtime/host/process.incn` | `std.runtime.host.process` | `missing` | No test references it. |
| `core/src/runtime/host/tool.incn` | `std.runtime.host.tool` | `missing` | No test references it. |
| `core/src/derives/comparison.incn` | `std.derives.comparison` | `snapshot-covered` | `test_std_derives_comparison_compiled_codegen`. |
| `core/src/derives/copying.incn` | `std.derives.copying` | `snapshot-covered` | `test_std_derives_copying_compiled_codegen`. |
| `core/src/derives/string.incn` | `std.derives.string` | `snapshot-covered` | `test_std_derives_string_compiled_codegen`. |
| `core/src/derives/collection.incn` | `std.derives.collection` | `snapshot-covered` | `test_std_derives_collection_compiled_codegen`. |
| `core/src/traits/prelude.incn` | `std.traits` | `snapshot-covered` | `test_std_traits_prelude_compiled_codegen`. |
| `core/src/traits/ops.incn` | `std.traits.ops` | `snapshot-covered` | `test_std_traits_ops_compiled_codegen`. |
| `core/src/traits/error.incn` | `std.traits.error` | `snapshot-covered` | `test_std_traits_error_compiled_codegen`; also used by error-bearing stdlib modules. |
| `core/src/traits/indexing.incn` | `std.traits.indexing` | `snapshot-covered` | `test_std_traits_indexing_compiled_codegen` and `JsonValue` indexing generated-Rust assertions. |
| `core/src/traits/callable.incn` | `std.traits.callable` | `snapshot-covered` | `test_std_traits_callable_compiled_codegen` and callable object Result tests. |
| `core/src/traits/convert.incn` | `std.traits.convert` | `snapshot-covered` | `test_std_traits_convert_compiled_codegen`, `std_traits_convert_usage` snapshot, and runtime usage test. |

### `interop`

| Source file | Module | Coverage | Evidence |
| --- | --- | --- | --- |
| `interop/src/interop.incn` | `std.interop` | `import/user-facing-covered` | `oven_interop_bake_bootstraps_direct_c_then_locked_run_uses_the_sealed_plan` (`cli_interop_target_tests.rs`) bakes and runs a program that imports `c` from it; `consumer_check_activates_standard_checked_c_vocab` checks the vocabulary it activates. |

### `system`

| Source file | Module | Coverage | Evidence |
| --- | --- | --- | --- |
| `system/src/environ.incn` | `std.environ` | `snapshot-covered` | `stdlib_generated_rust_snapshot_tests::std_environ_source_snapshot` snapshots direct source compilation; `test_issue1668_std_environ_args_codegen` snapshots a user import; `cli_std_environ_tests.rs` runs the public surface. |
| `system/src/io.incn` | `std.io` | `snapshot-covered` | `stdlib_generated_rust_snapshot_tests::std_io_source_snapshot` snapshots direct source compilation; `test_std_io_compile_and_run_bytesio_core_and_numeric_helpers` exercises `BytesIO` and numeric helpers at runtime; fs, hash, compression, encoding, uuid, and tempfile tests also import it. |
| `system/src/tempfile.incn` | `std.tempfile` | `snapshot-covered` | `std_tempfile_import` snapshot (`test_std_tempfile_import_codegen`); `test_std_fs_compile_and_run_path_file_and_tree_operations` runs `NamedTemporaryFile`, `TemporaryDirectory` and `SpooledTemporaryFile`. |
| `system/src/fs/prelude.incn` | `std.fs` | `snapshot-covered` | `std_fs_import` snapshot and `test_std_fs_compile_and_run_path_file_and_tree_operations`. |
| `system/src/fs/path.incn` | `std.fs.path` | `import/user-facing-covered` | `std.fs` integration test exercises paths, globbing, reads/writes, copy/move/touch/stat, and tree removal. |
| `system/src/fs/file.incn` | `std.fs.file` | `import/user-facing-covered` | `std.fs` integration test exercises open modes, readers/writers, encodings, `OpenOptions`, and byte/text operations. |
| `system/src/fs/metadata.incn` | `std.fs.metadata` | `import/user-facing-covered` | `std.fs` integration test exercises `stat`, `modified_unix`, `disk_usage`, and directory entries. |
| `system/src/fs/glob.incn` | `std.fs.glob` | `import/user-facing-covered` | `test_std_fs_glob_string_api_compile_and_run` and `Path.glob`/`Path.rglob` integration coverage. |
| `system/src/fs/locking.incn` | `std.fs.locking` | `import/user-facing-covered` | `compiled_sdk_providers_replace_consumer_fs_source_closure` (`cli_provider_boundary_tests.rs`) builds a consumer that calls `try_exclusive` and asserts that its generated Rust calls through the compiled provider. |

### `codecs`

| Source file | Module | Coverage | Evidence |
| --- | --- | --- | --- |
| `codecs/src/checksum.incn` | `std.checksum` | `import/user-facing-covered` | `test_std_checksum_compile_and_run_crc32_vectors` runs `crc32` against known vectors. |
| `codecs/src/encoding/prelude.incn` | `std.encoding` | `snapshot-covered` | `stdlib_generated_rust_snapshot_tests::std_encoding_prelude_import_snapshot` snapshots representative public prelude imports for family modules and `EncodingError`; `rfc064_std_encoding_behavior` imports the public prelude; algorithm modules are covered individually. |
| `codecs/src/encoding/_shared.incn` | `std.encoding._shared` | `indirect-only` | Imported by all algorithm modules and covered through their tests; no direct source/import target found. |
| `codecs/src/encoding/hex.incn` | `std.encoding.hex` | `import/user-facing-covered` | `std_encoding_hex_surface` fixture and the `rfc064_std_encoding_behavior` fixture. |
| `codecs/src/encoding/base32.incn` | `std.encoding.base32` | `import/user-facing-covered` | `loaves/toolchain/incan-cli/tests/std_encoding_algorithm_modules.rs` runs module source with vector and lenient decode assertions; the `rfc064_std_encoding_behavior` fixture also imports it. |
| `codecs/src/encoding/base58.incn` | `std.encoding.base58` | `import/user-facing-covered` | `loaves/toolchain/incan-cli/tests/std_encoding_algorithm_modules.rs` and the `rfc064_std_encoding_behavior` fixture. |
| `codecs/src/encoding/base64.incn` | `std.encoding.base64` | `import/user-facing-covered` | `loaves/toolchain/incan-cli/tests/std_encoding_algorithm_modules.rs` and the `rfc064_std_encoding_behavior` fixture. |
| `codecs/src/encoding/base85.incn` | `std.encoding.base85` | `import/user-facing-covered` | `loaves/toolchain/incan-cli/tests/std_encoding_algorithm_modules.rs` and the `rfc064_std_encoding_behavior` fixture. |
| `codecs/src/encoding/bech32.incn` | `std.encoding.bech32` | `import/user-facing-covered` | `loaves/toolchain/incan-cli/tests/std_encoding_algorithm_modules.rs` and the `rfc064_std_encoding_behavior` fixture. |

### `compression`

| Source file | Module | Coverage | Evidence |
| --- | --- | --- | --- |
| `compression/src/compression/prelude.incn` | `std.compression` | `snapshot-covered` | `stdlib_generated_rust_snapshot_tests::std_compression_prelude_source_snapshot` snapshots direct prelude source compilation; `test_std_compression_modules_compile_codegen` includes this file; `std_compression_surface` runs the public surface. |
| `compression/src/compression/_core.incn` | `std.compression._core` | `snapshot-covered` | `stdlib_generated_rust_snapshot_tests::std_compression_core_source_snapshot` snapshots direct core source compilation; direct compile loop and public compression surface tests cover all codecs. |
| `compression/src/compression/_auto.incn` | `std.compression._auto` | `compile-only-covered` | Direct compile loop plus `decompress_auto` and `decompress_auto_stream` in the surface fixture. |
| `compression/src/compression/gzip.incn` | `std.compression.gzip` | `compile-only-covered` | Direct compile loop and surface fixture. |
| `compression/src/compression/zlib.incn` | `std.compression.zlib` | `compile-only-covered` | Direct compile loop and surface fixture. |
| `compression/src/compression/deflate.incn` | `std.compression.deflate` | `compile-only-covered` | Direct compile loop and surface fixture. |
| `compression/src/compression/zstd.incn` | `std.compression.zstd` | `compile-only-covered` | Direct compile loop and surface fixture. |
| `compression/src/compression/bz2.incn` | `std.compression.bz2` | `compile-only-covered` | Direct compile loop and surface fixture. |
| `compression/src/compression/lzma.incn` | `std.compression.lzma` | `compile-only-covered` | Direct compile loop and surface fixture. |
| `compression/src/compression/snappy.incn` | `std.compression.snappy` | `compile-only-covered` | Direct compile loop and surface fixture. |
| `compression/src/compression/snappy/raw.incn` | `std.compression.snappy.raw` | `compile-only-covered` | Direct compile loop and surface fixture imports raw snappy compress/decompress. |

### `data`

| Source file | Module | Coverage | Evidence |
| --- | --- | --- | --- |
| `data/src/collections.incn` | `std.collections` | `import/user-facing-covered` | `loaves/compiler/incan_test_support/fixtures/rfc030_std_collections_behavior.incn`, `std_ordinal_map_surface`, `ordinal_key_builtin_impls`, `ordinal_map_str_fast_lookup`, and layering guards. |
| `data/src/graph.incn` | `std.graph` | `snapshot-covered` | `test_std_graph_compiled_codegen`, `std_graph_import`, and `std_graph_surface` cover declarations, import lowering, constructors, DAGs, and multigraph edge IDs. |
| `data/src/json.incn` | `std.json` | `import/user-facing-covered` | `test_std_json_value_indexing_emits_checked_helpers`, JSON deserialize/value runtime tests, and serde integration tests. |
| `data/src/toml.incn` | `std.toml` | `import/user-facing-covered` | `std_toml_manifest_and_lock_roundtrip_through_compiled_sdk` bakes and runs the `std_toml_surface`, `std_toml_typed_lookup` and `std_toml_module_import` fixtures. |
| `data/src/math.incn` | `std.math` | `snapshot-covered` | `std_math` codegen snapshot (`test_std_math_codegen`) plus `test_std_math_surface_runs`, which runs constants, functions and the numeric-like helpers. |
| `data/src/uuid.incn` | `std.uuid` | `snapshot-covered` | `test_std_uuid_compiled_codegen`, `std_uuid_import`, the `test_std_uuid_surface` fixture, and layering guards cover source-defined UUID generation/imports and absence of Rust-backed UUID type. |
| `data/src/datetime/prelude.incn` | `std.datetime` | `snapshot-covered` | `stdlib_generated_rust_snapshot_tests::std_datetime_prelude_import_snapshot` snapshots representative public datetime re-exports; `std_datetime_surface` imports and runs `std.datetime` names. |
| `data/src/datetime/runtime.incn` | `std.datetime.runtime` | `import/user-facing-covered` | `test_std_datetime_surface_runs_with_std_time_runtime_boundary` reads this file and asserts the Rust `std::time` boundary before running the surface fixture. |
| `data/src/datetime/error.incn` | `std.datetime.error` | `indirect-only` | Used by runtime/civil modules and exercised through error cases in `std_datetime_surface`; no direct generated-Rust target found. |
| `data/src/datetime/civil.incn` | `std.datetime.civil` | `import/user-facing-covered` | `std_datetime_surface` reads and runs the civil aggregate with calendar, parsing, formatting, and offset cases. |
| `data/src/datetime/civil/intervals.incn` | `std.datetime.civil.intervals` | `import/user-facing-covered` | Included by the datetime surface test's civil directory read and used by `TimeDelta`, `YearMonthInterval`, and `DateTimeInterval` fixture cases. |
| `data/src/datetime/civil/naive.incn` | `std.datetime.civil.naive` | `snapshot-covered` | `imported_stdlib_value_fragment` snapshots an import from this module; `std_datetime_surface` covers dates, times, parsing, formatting, and ordinal helpers. |
| `data/src/datetime/civil/offset.incn` | `std.datetime.civil.offset` | `import/user-facing-covered` | Included by datetime surface fixture through `DateTimeOffset` cases. |
| `data/src/hash/prelude.incn` | `std.hash` | `import/user-facing-covered` | `test_std_hash_compile_and_run_digest_file_and_error_paths` imports digest and streaming helpers. |
| `data/src/hash/_core.incn` | `std.hash._core` | `import/user-facing-covered` | Covered through digest calls for MD5, SHA, SHA3, Blake, Shake, and xxhash in the hash integration test. |
| `data/src/hash/_hmac.incn` | `std.hash._hmac` | `import/user-facing-covered` | `test_std_hash_hmac_compile_and_run_rfc4231_vectors` runs `hmac_sha256` from `std.hash` against known vectors. |
| `data/src/hash/_streaming.incn` | `std.hash._streaming` | `import/user-facing-covered` | Covered through `file_digest`, `reader_digest`, and typed file/reader hash helpers in the hash integration test. |
| `data/src/regex/prelude.incn` | `std.regex` | `import/user-facing-covered` | `std_regex_surface`, constructor-hook codegen test, and layering guard cover public imports and source-owned behavior. |
| `data/src/regex/_core.incn` | `std.regex._core` | `import/user-facing-covered` | `std_regex_surface` exercises constructors and matching; layering guard checks regex engine construction remains in source. |
| `data/src/regex/types.incn` | `std.regex.types` | `import/user-facing-covered` | `std_regex_surface` exercises `Captures`, `Match`, and iterators; layering guard includes this file. |
| `data/src/regex/_replacement.incn` | `std.regex._replacement` | `import/user-facing-covered` | `std_regex_surface` and layering guard cover replacement helpers staying in Incan source. |
| `data/src/serde/prelude.incn` | `std.serde` | `snapshot-covered` | `stdlib_generated_rust_snapshot_tests::std_serde_prelude_import_snapshot` snapshots `from std.serde import json` derive resolution; existing user fixtures also import `from std.serde import json`. |
| `data/src/serde/json.incn` | `std.serde.json` | `snapshot-covered` | `test_std_serde_json_compiled_codegen`, `std_serde_json_import`, `std_serde_with_serialize_trait`, module derive snapshots, and JSON runtime tests. |

### `async`

| Source file | Module | Coverage | Evidence |
| --- | --- | --- | --- |
| `async/src/async/prelude.incn` | `std.async` | `snapshot-covered` | `stdlib_generated_rust_snapshot_tests::std_async_prelude_import_snapshot` snapshots representative public async imports; task/time/channel/sync/race modules are covered individually. |
| `async/src/async/task.incn` | `std.async.task` | `snapshot-covered` | `test_std_async_task_compiled_codegen`, async task/time wrapper fixture, and spawn runtime tests. |
| `async/src/async/time.incn` | `std.async.time` | `snapshot-covered` | `test_std_async_time_compiled_codegen`, timeout/sleep wrapper fixture, race helper tests, and runtime timeout tests. |
| `async/src/async/channel.incn` | `std.async.channel` | `snapshot-covered` | `test_std_async_channel_compiled_codegen` plus channel runtime tests using `channel`, `unbounded_channel`, and `oneshot`. |
| `async/src/async/sync.incn` | `std.async.sync` | `snapshot-covered` | `test_std_async_sync_compiled_codegen` plus runtime tests for `Mutex`, `RwLock`, `Semaphore`, and `Barrier`. |
| `async/src/async/race.incn` | `std.async.race` | `snapshot-covered` | `test_std_async_race_compiled_codegen`, `race_for_expression_codegen`, and helper runtime tests. |

### `observability`

| Source file | Module | Coverage | Evidence |
| --- | --- | --- | --- |
| `observability/src/logging.incn` | `std.logging` | `import/user-facing-covered` | Multiple integration tests run `basic_config`, `get_logger`, ambient `log`, JSON rendering, invalid logger names, and structured fields. |
| `observability/src/telemetry/prelude.incn` | `std.telemetry` | `snapshot-covered` | `stdlib_generated_rust_snapshot_tests::std_telemetry_prelude_import_snapshot` snapshots representative public telemetry re-exports; logging tests import `std.telemetry.core`. |
| `observability/src/telemetry/core.incn` | `std.telemetry.core` | `snapshot-covered` | `stdlib_generated_rust_snapshot_tests::std_telemetry_core_source_snapshot` snapshots direct core source compilation; logging JSON structured-field tests import `TelemetryValue`; logging source imports telemetry core types. |

### `web`

| Source file | Module | Coverage | Evidence |
| --- | --- | --- | --- |
| `web/src/web/prelude.incn` | `std.web` | `snapshot-covered` | `stdlib_generated_rust_snapshot_tests::std_web_prelude_import_snapshot` snapshots representative public web prelude imports including route macro wiring; existing public web import snapshots exercise re-exported names. |
| `web/src/web/app.incn` | `std.web.app` | `import/user-facing-covered` | `std_web_routing_compiled`, web route extractor snapshots, and app route codegen tests. |
| `web/src/web/request.incn` | `std.web.request` | `import/user-facing-covered` | `web_route_extractors`, nested route extractor snapshots, and `newtype_from_request`. |
| `web/src/web/response.incn` | `std.web.response` | `import/user-facing-covered` | `newtype_web_response`, web route extractor snapshots, and response wrapper codegen. |
| `web/src/web/routing.incn` | `std.web.routing` | `snapshot-covered` | `std_web_routing_compiled`, route extractor snapshots, and route invalid-usage tests. |
| `web/src/web/macros.incn` | `std.web.macros` | `import/user-facing-covered` | Used by response/request extractor and route snapshots through `IntoResponse` and `FromRequestParts`. |

### `testing`

| Source file | Module | Coverage | Evidence |
| --- | --- | --- | --- |
| `testing/src/testing.incn` | `std.testing` | `snapshot-covered` | `test_std_testing_compiled_codegen` snapshots direct module compilation; many CLI and integration tests exercise assertions, fixtures, parametrization, marks, resources, and skips. |
