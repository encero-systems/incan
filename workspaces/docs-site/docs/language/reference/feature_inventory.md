# Incan feature inventory

!!! warning "Generated file"
    Do not edit this page by hand. If it looks wrong/outdated, update `crates/incan_core/src/lang/features.rs` and regenerate it.

    Regenerate with: `cargo run -p incan_core --bin generate_lang_reference`

This page is a generated, present-tense atlas of user-facing Incan capabilities. It is intentionally higher-level than the generated language vocabulary tables: one feature can span syntax, type checking, stdlib source, manifests, tooling, and examples.

Use it when deciding whether code should use an existing Incan surface before adding wrappers, Rust fallbacks, or project-local conventions.

## Contents

- [All features](#all-features)
- [Feature details](#feature-details)

## All features

| Feature | Category | Since | Activation | Canonical forms | Summary | Prefer over | References |
|---|---|---:|---|---|---|---|---|
| Namespaced stdlib imports and decorators | Stdlib | 0.2 | Import the relevant `std.*` module. | `from std.testing import assert_eq`<br>`from std.web.routing import route`<br>`@route("/hello")` | Standard-library APIs and compiler-owned decorators resolve through explicit `std.*` module paths. | Bare pre-0.2 stdlib names and ambient decorator magic. | [Imports and modules](imports_and_modules.md), [Standard library](stdlib/index.md), [Release 0.2](../../release_notes/0_2.md) |
| Rust interop boundary | Interop | 0.2 | Declare Rust dependencies in `incan.toml`; import through `rust` / `rust::` paths. | `from rust import uuid`<br>`from rust::std::time import Instant`<br>`type UserId = rusttype i64` | Incan can import Rust crates, bind Rust paths, declare `rusttype` wrappers, and model explicit interop edges. | Custom Rust backend modules used when ordinary Rust imports or wrappers are sufficient. | [Rust interop](../how-to/rust_interop.md), [Rust types for Python developers](../how-to/rust_types_for_python_devs.md), [Release 0.2](../../release_notes/0_2.md) |
| Incan libraries and `pub::` imports | Libraries | 0.2 | Build libraries with `incan build --lib`; consume dependencies through `pub::` imports. | `from pub::mylib import my_function`<br>`pub from session import Session` | Projects can publish public API manifests and downstream projects can import those public symbols. | Copying source files between projects or relying on private module paths. | [Imports and modules](imports_and_modules.md), [Release 0.2](../../release_notes/0_2.md) |
| Module static storage | Syntax | 0.2 | None. | `static hits: int = 0`<br>`pub static registry: dict[str, int] = {}` | `static` declares live module-owned runtime storage, distinct from deeply immutable `const` values. | Module-level mutable state hidden behind ad hoc helper functions. | [Static storage](static_storage.md), [Module state](../how-to/module_state.md), [Release 0.2](../../release_notes/0_2.md) |
| First-class function references | TypeSystem | 0.2 | None. | `handler: Callable[int, str] = label`<br>`callbacks = [on_success, on_error]` | Named functions can be passed, stored, and typed with `Callable[...]` or function type syntax. | Closures or wrappers whose only job is to pass through an existing named function. | [Functions and calls](functions.md), [Derives and traits](derives_and_traits.md), [Release 0.2](../../release_notes/0_2.md) |
| Explicit call-site generics | TypeSystem | 0.2 | None. | `decode_rows[Order, _](path)`<br>`session.read_csv[Order](path)` | Direct function and method calls can spell type arguments when inference needs help. | Adding throwaway typed locals or duplicate helper functions only to steer inference. | [Derives and traits](derives_and_traits.md#call-site-type-arguments), [Why call-site type arguments exist](../explanation/call_site_type_arguments.md), [Release 0.2](../../release_notes/0_2.md) |
| Abstract traits and supertraits | TypeSystem | 0.2 | None. | `trait OrderedCollection[T] with Collection[T]:`<br>`def first(values: Collection[int]) -> int:` | Trait names are abstract annotation types, and traits can adopt supertraits with `with`. | Hidden generic bounds or duplicated method requirements when a trait annotation names the concept. | [Derives and traits](derives_and_traits.md#traits-authoring), [Derives and traits explained](../explanation/derives_and_traits.md) |
| Source-defined derives and trait contracts | TypeSystem | 0.2 | Import the relevant `std.derives.*`, `std.traits.*`, or derivable module. | `@derive(json)`<br>`model Row with Serialize:`<br>`T with Clone` | Derive and trait surfaces are authored as named stdlib capability contracts rather than compiler folklore. | Backend-only helper shims or comments that claim derive behavior without source-visible contracts. | [Derives and traits](derives_and_traits.md), [std.derives](stdlib/derives.md), [std.traits](stdlib/traits.md) |
| Model field metadata and reflection | TypeSystem | 0.2 | None for field metadata; import `std.reflection` helpers when needed. | `name as "wire_name": str`<br>`user.__fields__()` | Model fields can carry aliases/descriptions and reflection exposes typed `FieldInfo` metadata. | Stringly schema maps that duplicate model field names and wire aliases. | [Reflection](reflection.md), [std.reflection](stdlib/reflection.md), [Release 0.2](../../release_notes/0_2.md) |
| Type tokens and type-argument reflection | TypeSystem | 0.3 | Use explicit type arguments for compile-time reflection, or call an overload that expects `Type[T]`. | `T.__class_name__()`<br>`def cast(expr: ColumnExpr, target: Type[int]) -> IntColumnExpr:`<br>`def accepts_schema(value: Type[MySchema]) -> str:`<br>`cast(col("amount"), int)` | Primitive type arguments expose stable source names, model type tokens carry checked source type evidence, and expected `Type[T]` parameters let visible type names select precise overloads without making types general runtime values. | String target names, dummy schema values, or helper families used only to recover type-specific return types. | [Reflection](reflection.md), [std.reflection](stdlib/reflection.md), [RFC 107 north star](../../RFCs/107_type_directed_library_apis.md), [Release 0.3](../../release_notes/0_3.md) |
| Value enums | TypeSystem | 0.3 | None. | `enum Level(str):`<br>`WARN = "WARN"`<br>`Level.from_value(raw)` | Enums can use `str` or `int` backing values while preserving enum type safety. | Loose string/int constants or duplicate parsing helpers around enum-like values. | [Enums explained](../explanation/enums.md), [Modeling with enums](../how-to/modeling_with_enums.md), [Release 0.3](../../release_notes/0_3.md) |
| Union types and narrowing | TypeSystem | 0.3 | None. | `value: int \| str`<br>`if isinstance(value, int):`<br>`match value:` | Closed anonymous unions support `Union[A, B]`, `A \| B`, narrowing, and exhaustive match type patterns. | Untyped `Any`-like values, parallel option fields, or manual tag/payload models for closed alternatives. | [Union types](union_types.md), [Release 0.3](../../release_notes/0_3.md) |
| Validated newtypes and checked coercion | TypeSystem | 0.3 | None. | `type UserId = newtype int[ge=0]:`<br>`Email.new(value)`<br>`@no_implicit_coercion` | Newtypes can validate primitive constraints and participate in checked construction/coercion. | Raw primitives passed across APIs with comments describing expected invariants. | [Newtypes](newtypes.md), [Book: newtypes](../tutorials/book/12_newtypes.md), [Release 0.3](../../release_notes/0_3.md) |
| Exact numeric types and conversions | TypeSystem | 0.3 | None. | `count: u32 = 1`<br>`price: decimal[12, 2] = 19.99d`<br>`small = value.try_resize()` | Exact integer widths, float widths, schema aliases, decimal precision/scale, and explicit resize policies are typed language surface. | Using broad `int`/`float` at wire, Rust interop, binary, or schema boundaries where representation matters. | [Numeric semantics](numeric_semantics.md), [Choosing numeric types](../how-to/choosing_numeric_types.md), [Release 0.3](../../release_notes/0_3.md) |
| `loop:` expressions and break values | Syntax | 0.3 | None. | `result = loop:`<br>`break value` | Intentional infinite loops can produce values directly through `break <value>`. | Mutable sentinel initialization followed by later branch assignment. | [Control flow](../explanation/control_flow.md), [Book: control flow](../tutorials/book/04_control_flow.md), [Release 0.3](../../release_notes/0_3.md) |
| `if let` and `while let` | Syntax | 0.3 | None. | `if let Some(value) = maybe:`<br>`while let Some(item) = iterator.next():` | Single-pattern control flow handles the common success-path case without full `match` scaffolding. | Verbose one-arm `match` blocks where the non-match path intentionally does nothing. | [Control flow](../explanation/control_flow.md), [Release 0.3](../../release_notes/0_3.md) |
| Pattern alternation | Syntax | 0.3 | None. | `Status.Pending \| Status.Retrying => handle_waiting()` | `match` and `if let` patterns can share a branch across alternatives with compatible bindings. | Duplicated branch bodies for variants that have the same behavior. | [Control flow](../explanation/control_flow.md), [Release 0.3](../../release_notes/0_3.md) |
| Enum methods and trait adoption | TypeSystem | 0.3 | None. | `enum Direction with Display:`<br>`def opposite(self) -> Direction:` | Enums can own methods, associated functions, and trait implementations directly in the enum body. | Detached helper functions for behavior that belongs to a closed enum. | [Enums explained](../explanation/enums.md), [Derives and traits](derives_and_traits.md), [Release 0.3](../../release_notes/0_3.md) |
| Computed properties | Syntax | 0.3 | None. | `property display_name -> str:`<br>`return self.first + " " + self.last` | Models, classes, and traits can expose field-like computed readers with `property`. | Zero-argument methods when callers should read a value-like member. | [Computed properties](computed_properties.md), [Models](../explanation/models_and_classes/models.md), [Classes](../explanation/models_and_classes/classes.md) |
| Symbol, method, and variant aliases | Syntax | 0.3 | None. | `pub average = alias avg`<br>`mean = avg`<br>`WARNING = alias WARN` | Aliases expose another resolved name for the same declaration, method, or enum variant without duplicating behavior. | Wrapper functions or duplicated enum variants used only for compatibility names. | [Symbol aliases](symbol_aliases.md), [Imports and modules](imports_and_modules.md), [Release 0.3](../../release_notes/0_3.md) |
| Callable presets with `partial` | Syntax | 0.3 | None. | `pub get = partial route(method="GET")`<br>`set_alive = partial set_state(state=true)` | `partial` creates a callable surface from an existing callable by supplying named preset values. | Hand-written wrappers whose only job is to pass the same keyword defaults. | [Callable presets](callable_presets.md), [Callable presets explained](../explanation/callable_presets.md), [Release 0.3](../../release_notes/0_3.md) |
| Rest parameters, unpacking, and spreads | Syntax | 0.3 | None. | `def log(*items: str, **fields: str) -> None:`<br>`f(*xs, **kw)`<br>`[*prefix, item]`<br>`{**base, "x": 1}` | Functions can capture `*args` / `**kwargs`; calls and literals support typed unpack/spread forms. | Manually spelling every forwarding arity or merging collections one element at a time. | [Functions and calls](functions.md), [Release 0.3](../../release_notes/0_3.md) |
| User-defined decorators | Syntax | 0.3 | None for user-defined decorators; compiler-owned decorators keep their documented imports. | `@logged`<br>`@registered("catalog.ref")`<br>`func.__name__`<br>`@registered[(str) -> ColumnExpr]("catalog.ref")` | Decorators are ordinary callable values applied to functions and methods, including generic decorator factories that infer or accept the decorated function type and decorator helpers that expose `func.__name__`. | Boilerplate wrapper declarations around every function that needs the same callable transform. | [Language reference](language.md#decorators), [Derives and traits](derives_and_traits.md), [Release 0.3](../../release_notes/0_3.md) |
| Generators | Syntax | 0.3 | None. | `def numbers() -> Generator[int]:`<br>`yield value`<br>`(x * 2 for x in values)` | `yield`-based functions and generator expressions produce lazy `Generator[T]` values. | Eager list construction when callers only need lazy iteration. | [Generators](generators.md), [Generators how-to](../how-to/generators.md), [Release 0.3](../../release_notes/0_3.md) |
| Iterator adapters and terminal consumers | Stdlib | 0.3 | Use iterator values. | `values.iter().map(parse).filter(valid).collect()`<br>`items.enumerate().take(10)`<br>`numbers.fold(0, add)` | Iterator pipelines expose lazy adapters and explicit terminal consumers. | Manual loop accumulators for ordinary map/filter/fold pipeline shapes. | [Collection protocols](stdlib_traits/collection_protocols.md), [Release 0.3](../../release_notes/0_3.md) |
| `Result[T, E]` combinators | Stdlib | 0.3 | Use `Result[T, E]` values. | `result.map(transform)`<br>`result.and_then(validate)`<br>`result.inspect(log_success)` | `Result` values support branch-local transforms, fallible chaining, recovery, and inspection taps. | Nested matches that only rewrap `Ok` / `Err` around one transformed branch. | [std.result](stdlib/result.md), [Fallible and infallible paths](../tutorials/fallible_and_infallible_paths.md), [Release 0.3](../../release_notes/0_3.md) |
| Protocol hooks for core syntax | TypeSystem | 0.3 | Define compatible dunder hooks and adopt/document the corresponding trait vocabulary where useful. | `def __len__(self) -> int:`<br>`def __iter__(self) -> Iterator[T]:`<br>`def __call__(self, value: T) -> U:` | User-defined types can participate in truthiness, length, membership, iteration, indexing, assignment, and calls. | Special-casing custom types in caller code instead of giving the type the expected protocol. | [Traits as language hooks](../explanation/traits_as_language_hooks.md), [Collection protocols](stdlib_traits/collection_protocols.md), [Operators](stdlib_traits/operators.md) |
| Rust trait adoption from Incan | Interop | 0.3 | Import the Rust trait metadata and adopt with `with TraitName`. | `type UserId = rusttype i64 with Display:`<br>`def fmt(self, f: Formatter) for Display -> Result[None, FmtError]:`<br>`type Output for Add[int] = UserId` | Newtype and rusttype declarations can author Rust trait impls with Incan adoption syntax. | Writing Rust-shaped `impl Trait for Type` concepts in comments or custom backend code. | [Rust interop](../how-to/rust_interop.md), [Derives and traits](derives_and_traits.md), [Release 0.3](../../release_notes/0_3.md) |
| Targeted generated-Rust lint suppression | Interop | 0.3 | Use `@rust.allow(...)` on supported declarations. | `@rust.allow("dead_code")`<br>`def helper() -> None:` | Generated Rust can receive narrow lint suppressions on individual items when source semantics require them. | Project-wide lint disables or broad generated-Rust allowance groups. | [Rust interop](../how-to/rust_interop.md#targeted-generated-rust-lint-suppression), [Release 0.3](../../release_notes/0_3.md) |
| Scoped DSL surfaces | Syntax | 0.3 | Import a vocab package that publishes scoped surface descriptors. | `query:`<br>`.field`<br>`\|>`<br>`sum(value)` | Library vocab crates can activate declaration, clause, glyph, leading-dot, and scoped symbol syntax inside their own DSL blocks. | Global parser changes for syntax that only belongs to one imported DSL. | [Authoring vocab crates](../../contributing/how-to/authoring_vocab_crates.md), [Release 0.3](../../release_notes/0_3.md) |
| `std.collections` specialized containers | Stdlib | 0.3 | Import from `std.collections`. | `from std.collections import Deque, Counter, PriorityQueue`<br>`queue = Deque[int]()` | Specialized containers cover deque, counter, default dict, ordered/sorted maps and sets, chain maps, and priority queues. | Encoding specialized container behavior in plain `list`, `dict`, or `set` plus ad hoc helpers. | [std.collections](stdlib/collections.md), [Choosing collections](../how-to/choosing_collections.md), [Release 0.3](../../release_notes/0_3.md) |
| `std.graph` directed graph types | Stdlib | 0.3 | Import from `std.graph`. | `from std.graph import DiGraph, Dag`<br>`graph = DiGraph[Task]()` | Graph types provide stable node/edge ids, DAG invariants, adjacency queries, traversal, and topological ordering. | Hand-rolled adjacency maps for ordinary dependency, plan, or workflow graphs. | [std.graph](stdlib/graph.md), [Release 0.3](../../release_notes/0_3.md) |
| `std.fs` filesystem APIs | Stdlib | 0.3 | Import from `std.fs` or submodules such as `std.fs.path`. | `from std.fs import Path`<br>`Path("data").join("orders.csv")` | Path-centric filesystem APIs cover paths, files, metadata, traversal, globbing, copy/move/delete, and durability syncs. | One-off Rust filesystem wrappers for ordinary path and file work. | [std.fs](stdlib/fs.md), [File IO](../how-to/file_io.md), [Release 0.3](../../release_notes/0_3.md) |
| `std.io` in-memory binary streams | Stdlib | 0.3 | Import from `std.io`. | `from std.io import BytesIO, Endian`<br>`stream.write(value, Endian.Little)` | Binary stream APIs cover `BytesIO`, endian-aware reads/writes, cursor helpers, delimiter operations, and buffer extraction. | Byte-twiddling helpers with unclear endian or cursor semantics. | [std.io](stdlib/io.md), [Release 0.3](../../release_notes/0_3.md) |
| `std.environ` runtime environment access | Stdlib | 0.5 | Import from `std.environ`. | `from std.environ import get, get_optional, get_or`<br>`token = get("API_TOKEN")?`<br>`mode = get_or("APP_MODE", "dev")` | `std.environ` provides read-only current-process Unicode environment reads through required, optional, and defaulted string helpers. | Direct `rust::std::env` imports or shell glue for ordinary runtime environment reads. | [std.environ](stdlib/environ.md), [RFC 089](../../RFCs/089_std_environ.md) |
| `std.json` dynamic JSON values | Stdlib | 0.3 | Import from `std.json`. | `from std.json import JsonValue`<br>`JsonValue.parse(source)`<br>`value["key"]`<br>`value[0]` | `JsonValue` provides dynamic parse-inspect-transform JSON workflows with checked optional indexing, explicit shape inspection, mutation helpers, traversal, and typed-model interop. | Ad hoc dictionaries or over-modeled schemas for payloads whose shape is intentionally open. | [std.json](stdlib/json.md), [Derives: Serialization](derives/serialization.md), [Release 0.3](../../release_notes/0_3.md) |
| `std.tempfile` temporary resources | Stdlib | 0.3 | Import from `std.tempfile`. | `NamedTemporaryFile.try_new()`<br>`TemporaryDirectory.try_new()`<br>`tmp.persist()` | Temporary files and directories are explicit resources with cleanup and persist semantics. | Manual random path generation or unchecked cleanup around temporary files. | [std.tempfile](stdlib/tempfile.md), [Release 0.3](../../release_notes/0_3.md) |
| `std.datetime` temporal values | Stdlib | 0.3 | Import from `std.datetime` modules or prelude. | `Date.utc_today()`<br>`DateTime.utc_now()`<br>`TimeDelta(days=1)` | Temporal APIs cover runtime timing, civil dates/times, fixed offsets, parsing/formatting, intervals, and calendar arithmetic. | Raw strings or integer timestamps inside code that has date/time semantics. | [std.datetime](stdlib/datetime.md), [Dates and times](../tutorials/dates_and_times.md), [Dates and times how-to](../how-to/dates_and_times.md) |
| `std.telemetry.core` data model | Stdlib | 0.3 | Import from `std.telemetry.core` or the `std.telemetry` prelude. | `from std.telemetry.core import TelemetryValue, Attributes`<br>`TelemetryValue.string("ready")`<br>`Attributes.from_string_fields(fields)` | Telemetry core provides structured values, attributes, resources, scopes, and trace context identifiers without configuring providers or exporters. | Stringifying structured observability fields before they reach logging or telemetry boundaries. | [std.logging](stdlib/logging.md), [Release 0.3](../../release_notes/0_3.md) |
| `std.logging` structured logging | Stdlib | 0.3 | Import from `std.logging`; ambient `log` is available for the current module logger. | `from std.logging import Level, basic_config`<br>`log.info("started", fields={"component": "worker"})` | Structured logging includes levels, named loggers, bound fields, formatting, JSON rendering, and telemetry values. | Printing diagnostic strings or routing ordinary application logging through custom Rust shims. | [std.logging](stdlib/logging.md), [Release 0.3](../../release_notes/0_3.md) |
| Testing assertions and markers | Testing | 0.3 | Use `assert` directly; import marker/helper APIs from `std.testing`. | `assert value == expected`<br>`assert call() raises ValueError`<br>`@parametrize("case", cases)` | Tests can use language assertions, raises checks, helper assertions, fixtures, parametrization, and marker decorators. | Ad hoc panic helpers or external test metadata formats for ordinary Incan tests. | [std.testing](stdlib/testing.md), [Testing how-to](../how-to/testing_stdlib.md), [Release 0.3](../../release_notes/0_3.md) |
| `incan test` runner | Testing | 0.3 | Run `incan test`. | `module tests:`<br>`incan test --list`<br>`incan test --format json --junit report.xml` | The runner owns discovery, inline test modules, stable ids, selection, fixtures, parametrization, reporting, shuffling, and scheduling. | Project-local scripts that duplicate core test discovery and reporting behavior. | [Tooling: testing](../../tooling/how-to/testing.md), [std.testing](stdlib/testing.md), [Release 0.3](../../release_notes/0_3.md) |
| Async and await | Async | 0.2 | Import `std.async` or one of its submodules. | `from std.async.time import sleep`<br>`async def main() -> None:`<br>`await sleep(1)` | `async` and `await` are import-activated soft-keyword surfaces backed by `std.async` modules. | Threading async behavior through synchronous wrappers or relying on pre-0.2 ambient async syntax. | [Async programming](../how-to/async_programming.md), [std.async](stdlib/async.md), [Release 0.2](../../release_notes/0_2.md) |
| Async race and awaitability | Async | 0.3 | Import `std.async.race` or the relevant async prelude helpers. | `race for value:`<br>`arm(task)`<br>`race(arms)` | `Awaitable[T]`, `race for`, and helper-style race composition support first-ready async workflows. | Legacy `std.async.select` or hand-rolled polling loops. | [Awaitable trait](stdlib_traits/awaitable.md), [Async programming](../how-to/async_programming.md), [Release 0.3](../../release_notes/0_3.md) |
| Project lifecycle tooling | Tooling | 0.3 | Use `incan init`, `incan new`, `incan version`, or `incan env`. | `incan new greeter --yes`<br>`incan version patch`<br>`incan env run dev test` | Project commands create scaffolds, manage versions, and run configured environments from `incan.toml`. | One-off project scaffolding scripts or manual version-file edits. | [Project lifecycle](project_lifecycle.md), [Project lifecycle how-to](../how-to/project_lifecycle.md), [Release 0.3](../../release_notes/0_3.md) |
| Toolchain installer and release manifest | Tooling | 0.4 | Use the GitHub Release installer, Homebrew formula, npm package, pipx package, or versioned toolchain manifest. | `curl -fsSL https://github.com/encero-systems/incan/releases/latest/download/install.sh \| bash`<br>`brew tap encero-systems/tap && brew install incan`<br>`npm install -g @incan/toolchain`<br>`pipx install incan` | The toolchain install path is manifest-driven, checksum-verified, and installs `incan` plus `incan-lsp` from the same release archives across direct and package-manager channels. Direct, npm, and pipx installation can provision stable Rust and `wasm32-wasip1`; Homebrew installs the prebuilt command binaries and leaves Rust management to the user. | Treating Cargo, repository checkouts, and package-manager adapters as separate sources of install truth. | [Install and run](../../tooling/how-to/install_and_run.md), [Release 0.4](../../release_notes/0_4.md) |
| Zero-clone starter project flow | Tooling | 0.4 | Use `incan new`, then run project commands from the generated directory. | `incan new hello --yes`<br>`cd hello`<br>`incan run`<br>`incan test`<br>`incan build --release` | `incan new` creates a runnable, testable project with a manifest, entrypoint, starter test, README, `.gitignore`, and release-line toolchain constraint. | Cloning the compiler repository or copying examples manually before a first run. | [Getting started](../../tooling/tutorials/getting_started.md), [Project lifecycle](project_lifecycle.md), [Release 0.4](../../release_notes/0_4.md) |
| Stable diagnostics commands | Tooling | 0.4 | Use `incan check` or `incan explain`. | `incan check src/main.incn --format json`<br>`incan explain INCAN-T0001` | Type-check diagnostics can be emitted as versioned JSON with stable codes, source spans, and catalog-backed explanations. | Scraping terminal diagnostics or building tool-specific error-code maps. | [CLI reference](../../tooling/reference/cli_reference.md), [Release 0.4](../../release_notes/0_4.md) |
| Build reports and generated Rust inspection | Tooling | 0.4 | Use `incan build --report json` or `incan inspect rust`. | `incan build src/main.incn --report json`<br>`incan build --lib --report json --report-output build.json`<br>`incan inspect rust src/main.incn --format json` | Builds can emit versioned machine-readable reports, generated Rust can be inspected intentionally as current backend output, and generated public Rust items preserve checked source docstrings as Rust doc comments when available. | Scraping terminal progress output or treating `--emit-rust` debug output as a stable artifact contract. | [CLI reference](../../tooling/reference/cli_reference.md), [Release 0.4](../../release_notes/0_4.md) |
| Build and test preheat observability | Tooling | 0.4 | Use verbose test output or `INCAN_RUST_INSPECT_TIMING=1` when build/test preheat timing needs to be inspected. | `incan test -v tests`<br>`INCAN_RUST_INSPECT_TIMING=1 incan build --lib`<br>`incan build --lib` | Build and test preheat paths report phase timing, Cargo target reuse, Rust metadata warmed/reused/skipped counts, vocab companion cache reuse, and cache split reasons. | Silent long-running preheat phases or separate probes that do not share the real build/test cache policy. | [CLI reference](../../tooling/reference/cli_reference.md), [Release 0.4](../../release_notes/0_4.md) |
| Compiler-backed codegraph inspection | Tooling | 0.4 | Use `incan inspect codegraph`. | `incan inspect codegraph src/main.incn --format jsonl`<br>`incan inspect codegraph src --format jsonl --allow-errors` | Incan-language source files, modules, declarations, imports, exports, body-level reference and call syntax, conservative resolved reference and call targets, containment, spans, provenance, language tags, degraded state, and diagnostics can be exported as deterministic JSONL records. | Repeated grep/read loops or tool-specific source scrapers when agents and tooling need basic Incan structure. | [Codegraph inspection](../../tooling/reference/codegraph_inspection.md), [CLI reference](../../tooling/reference/cli_reference.md), [Release 0.4](../../release_notes/0_4.md) |
| Checked API metadata | Tooling | 0.3 | Use `incan tools metadata api` or LSP metadata commands. | `incan tools metadata api src/lib.incn`<br>`incan tools metadata model emit` | Typechecked public APIs can emit structured metadata for docs, manifests, hovers, and model bundle tooling. | Scraping source text or generated Rust when tooling needs API contracts. | [Release 0.3](../../release_notes/0_3.md), [Project lifecycle](project_lifecycle.md) |
| Formatter spacing and wrapping contract | Tooling | 0.3 | Run `incan fmt`. | `incan fmt src/main.incn`<br>`incan fmt --check` | Formatter output has explicit vertical-spacing buckets, docstring normalization, comment attachment, and common wrapping rules. | Hand-maintained whitespace conventions that drift from the formatter. | [Code style](code_style.md), [Formatting how-to](../../tooling/how-to/formatting.md), [Release 0.3](../../release_notes/0_3.md) |

## Feature details

### Namespaced stdlib imports and decorators

- **Id:** `NamespacedStdlib`
- **Category:** `Stdlib`
- **Since:** `0.2`
- **RFC:** `RFC 022`
- **Stability:** `Stable`
- **Activation:** Import the relevant `std.*` module.
- **Use instead of:** Bare pre-0.2 stdlib names and ambient decorator magic.
- **References:** [Imports and modules](imports_and_modules.md), [Standard library](stdlib/index.md), [Release 0.2](../../release_notes/0_2.md)

Standard-library APIs and compiler-owned decorators resolve through explicit `std.*` module paths.

Canonical forms:

- `from std.testing import assert_eq`
- `from std.web.routing import route`
- `@route("/hello")`

### Rust interop boundary

- **Id:** `RustInteropBoundary`
- **Category:** `Interop`
- **Since:** `0.2`
- **RFC:** `RFC 041`
- **Stability:** `Stable`
- **Activation:** Declare Rust dependencies in `incan.toml`; import through `rust` / `rust::` paths.
- **Use instead of:** Custom Rust backend modules used when ordinary Rust imports or wrappers are sufficient.
- **References:** [Rust interop](../how-to/rust_interop.md), [Rust types for Python developers](../how-to/rust_types_for_python_devs.md), [Release 0.2](../../release_notes/0_2.md)

Incan can import Rust crates, bind Rust paths, declare `rusttype` wrappers, and model explicit interop edges.

Canonical forms:

- `from rust import uuid`
- `from rust::std::time import Instant`
- `type UserId = rusttype i64`

### Incan libraries and `pub::` imports

- **Id:** `IncanLibraries`
- **Category:** `Libraries`
- **Since:** `0.2`
- **RFC:** `RFC 031`
- **Stability:** `Stable`
- **Activation:** Build libraries with `incan build --lib`; consume dependencies through `pub::` imports.
- **Use instead of:** Copying source files between projects or relying on private module paths.
- **References:** [Imports and modules](imports_and_modules.md), [Release 0.2](../../release_notes/0_2.md)

Projects can publish public API manifests and downstream projects can import those public symbols.

Canonical forms:

- `from pub::mylib import my_function`
- `pub from session import Session`

### Module static storage

- **Id:** `StaticStorage`
- **Category:** `Syntax`
- **Since:** `0.2`
- **RFC:** `RFC 052`
- **Stability:** `Stable`
- **Activation:** None.
- **Use instead of:** Module-level mutable state hidden behind ad hoc helper functions.
- **References:** [Static storage](static_storage.md), [Module state](../how-to/module_state.md), [Release 0.2](../../release_notes/0_2.md)

`static` declares live module-owned runtime storage, distinct from deeply immutable `const` values.

Canonical forms:

- `static hits: int = 0`
- `pub static registry: dict[str, int] = {}`

### First-class function references

- **Id:** `FirstClassFunctions`
- **Category:** `TypeSystem`
- **Since:** `0.2`
- **RFC:** `RFC 035`
- **Stability:** `Stable`
- **Activation:** None.
- **Use instead of:** Closures or wrappers whose only job is to pass through an existing named function.
- **References:** [Functions and calls](functions.md), [Derives and traits](derives_and_traits.md), [Release 0.2](../../release_notes/0_2.md)

Named functions can be passed, stored, and typed with `Callable[...]` or function type syntax.

Canonical forms:

- `handler: Callable[int, str] = label`
- `callbacks = [on_success, on_error]`

### Explicit call-site generics

- **Id:** `CallSiteGenerics`
- **Category:** `TypeSystem`
- **Since:** `0.2`
- **RFC:** `RFC 054`
- **Stability:** `Stable`
- **Activation:** None.
- **Use instead of:** Adding throwaway typed locals or duplicate helper functions only to steer inference.
- **References:** [Derives and traits](derives_and_traits.md#call-site-type-arguments), [Why call-site type arguments exist](../explanation/call_site_type_arguments.md), [Release 0.2](../../release_notes/0_2.md)

Direct function and method calls can spell type arguments when inference needs help.

Canonical forms:

- `decode_rows[Order, _](path)`
- `session.read_csv[Order](path)`

### Abstract traits and supertraits

- **Id:** `AbstractTraits`
- **Category:** `TypeSystem`
- **Since:** `0.2`
- **RFC:** `RFC 042`
- **Stability:** `Stable`
- **Activation:** None.
- **Use instead of:** Hidden generic bounds or duplicated method requirements when a trait annotation names the concept.
- **References:** [Derives and traits](derives_and_traits.md#traits-authoring), [Derives and traits explained](../explanation/derives_and_traits.md)

Trait names are abstract annotation types, and traits can adopt supertraits with `with`.

Canonical forms:

- `trait OrderedCollection[T] with Collection[T]:`
- `def first(values: Collection[int]) -> int:`

### Source-defined derives and trait contracts

- **Id:** `SourceDefinedDerivesTraits`
- **Category:** `TypeSystem`
- **Since:** `0.2`
- **RFC:** `RFC 024`
- **Stability:** `Stable`
- **Activation:** Import the relevant `std.derives.*`, `std.traits.*`, or derivable module.
- **Use instead of:** Backend-only helper shims or comments that claim derive behavior without source-visible contracts.
- **References:** [Derives and traits](derives_and_traits.md), [std.derives](stdlib/derives.md), [std.traits](stdlib/traits.md)

Derive and trait surfaces are authored as named stdlib capability contracts rather than compiler folklore.

Canonical forms:

- `@derive(json)`
- `model Row with Serialize:`
- `T with Clone`

### Model field metadata and reflection

- **Id:** `ModelFieldMetadata`
- **Category:** `TypeSystem`
- **Since:** `0.2`
- **RFC:** `RFC 021`
- **Stability:** `Stable`
- **Activation:** None for field metadata; import `std.reflection` helpers when needed.
- **Use instead of:** Stringly schema maps that duplicate model field names and wire aliases.
- **References:** [Reflection](reflection.md), [std.reflection](stdlib/reflection.md), [Release 0.2](../../release_notes/0_2.md)

Model fields can carry aliases/descriptions and reflection exposes typed `FieldInfo` metadata.

Canonical forms:

- `name as "wire_name": str`
- `user.__fields__()`

### Type tokens and type-argument reflection

- **Id:** `TypeTokensReflection`
- **Category:** `TypeSystem`
- **Since:** `0.3`
- **RFC:** `RFC 107`
- **Stability:** `Stable`
- **Activation:** Use explicit type arguments for compile-time reflection, or call an overload that expects `Type[T]`.
- **Use instead of:** String target names, dummy schema values, or helper families used only to recover type-specific return types.
- **References:** [Reflection](reflection.md), [std.reflection](stdlib/reflection.md), [RFC 107 north star](../../RFCs/107_type_directed_library_apis.md), [Release 0.3](../../release_notes/0_3.md)

Primitive type arguments expose stable source names, model type tokens carry checked source type evidence, and expected `Type[T]` parameters let visible type names select precise overloads without making types general runtime values.

Canonical forms:

- `T.__class_name__()`
- `def cast(expr: ColumnExpr, target: Type[int]) -> IntColumnExpr:`
- `def accepts_schema(value: Type[MySchema]) -> str:`
- `cast(col("amount"), int)`

### Value enums

- **Id:** `ValueEnums`
- **Category:** `TypeSystem`
- **Since:** `0.3`
- **RFC:** `RFC 032`
- **Stability:** `Stable`
- **Activation:** None.
- **Use instead of:** Loose string/int constants or duplicate parsing helpers around enum-like values.
- **References:** [Enums explained](../explanation/enums.md), [Modeling with enums](../how-to/modeling_with_enums.md), [Release 0.3](../../release_notes/0_3.md)

Enums can use `str` or `int` backing values while preserving enum type safety.

Canonical forms:

- `enum Level(str):`
- `WARN = "WARN"`
- `Level.from_value(raw)`

### Union types and narrowing

- **Id:** `UnionTypes`
- **Category:** `TypeSystem`
- **Since:** `0.3`
- **RFC:** `RFC 029`
- **Stability:** `Stable`
- **Activation:** None.
- **Use instead of:** Untyped `Any`-like values, parallel option fields, or manual tag/payload models for closed alternatives.
- **References:** [Union types](union_types.md), [Release 0.3](../../release_notes/0_3.md)

Closed anonymous unions support `Union[A, B]`, `A | B`, narrowing, and exhaustive match type patterns.

Canonical forms:

- `value: int | str`
- `if isinstance(value, int):`
- `match value:`

### Validated newtypes and checked coercion

- **Id:** `ValidatedNewtypes`
- **Category:** `TypeSystem`
- **Since:** `0.3`
- **RFC:** `RFC 017`
- **Stability:** `Stable`
- **Activation:** None.
- **Use instead of:** Raw primitives passed across APIs with comments describing expected invariants.
- **References:** [Newtypes](newtypes.md), [Book: newtypes](../tutorials/book/12_newtypes.md), [Release 0.3](../../release_notes/0_3.md)

Newtypes can validate primitive constraints and participate in checked construction/coercion.

Canonical forms:

- `type UserId = newtype int[ge=0]:`
- `Email.new(value)`
- `@no_implicit_coercion`

### Exact numeric types and conversions

- **Id:** `NumericTypeSystem`
- **Category:** `TypeSystem`
- **Since:** `0.3`
- **RFC:** `RFC 009`
- **Stability:** `Stable`
- **Activation:** None.
- **Use instead of:** Using broad `int`/`float` at wire, Rust interop, binary, or schema boundaries where representation matters.
- **References:** [Numeric semantics](numeric_semantics.md), [Choosing numeric types](../how-to/choosing_numeric_types.md), [Release 0.3](../../release_notes/0_3.md)

Exact integer widths, float widths, schema aliases, decimal precision/scale, and explicit resize policies are typed language surface.

Canonical forms:

- `count: u32 = 1`
- `price: decimal[12, 2] = 19.99d`
- `small = value.try_resize()`

### `loop:` expressions and break values

- **Id:** `LoopExpressions`
- **Category:** `Syntax`
- **Since:** `0.3`
- **RFC:** `RFC 016`
- **Stability:** `Stable`
- **Activation:** None.
- **Use instead of:** Mutable sentinel initialization followed by later branch assignment.
- **References:** [Control flow](../explanation/control_flow.md), [Book: control flow](../tutorials/book/04_control_flow.md), [Release 0.3](../../release_notes/0_3.md)

Intentional infinite loops can produce values directly through `break <value>`.

Canonical forms:

- `result = loop:`
- `break value`

### `if let` and `while let`

- **Id:** `IfWhileLet`
- **Category:** `Syntax`
- **Since:** `0.3`
- **RFC:** `RFC 049`
- **Stability:** `Stable`
- **Activation:** None.
- **Use instead of:** Verbose one-arm `match` blocks where the non-match path intentionally does nothing.
- **References:** [Control flow](../explanation/control_flow.md), [Release 0.3](../../release_notes/0_3.md)

Single-pattern control flow handles the common success-path case without full `match` scaffolding.

Canonical forms:

- `if let Some(value) = maybe:`
- `while let Some(item) = iterator.next():`

### Pattern alternation

- **Id:** `PatternAlternation`
- **Category:** `Syntax`
- **Since:** `0.3`
- **RFC:** `RFC 071`
- **Stability:** `Stable`
- **Activation:** None.
- **Use instead of:** Duplicated branch bodies for variants that have the same behavior.
- **References:** [Control flow](../explanation/control_flow.md), [Release 0.3](../../release_notes/0_3.md)

`match` and `if let` patterns can share a branch across alternatives with compatible bindings.

Canonical forms:

- `Status.Pending | Status.Retrying => handle_waiting()`

### Enum methods and trait adoption

- **Id:** `EnumMethodsTraits`
- **Category:** `TypeSystem`
- **Since:** `0.3`
- **RFC:** `RFC 050`
- **Stability:** `Stable`
- **Activation:** None.
- **Use instead of:** Detached helper functions for behavior that belongs to a closed enum.
- **References:** [Enums explained](../explanation/enums.md), [Derives and traits](derives_and_traits.md), [Release 0.3](../../release_notes/0_3.md)

Enums can own methods, associated functions, and trait implementations directly in the enum body.

Canonical forms:

- `enum Direction with Display:`
- `def opposite(self) -> Direction:`

### Computed properties

- **Id:** `ComputedProperties`
- **Category:** `Syntax`
- **Since:** `0.3`
- **RFC:** `RFC 046`
- **Stability:** `Stable`
- **Activation:** None.
- **Use instead of:** Zero-argument methods when callers should read a value-like member.
- **References:** [Computed properties](computed_properties.md), [Models](../explanation/models_and_classes/models.md), [Classes](../explanation/models_and_classes/classes.md)

Models, classes, and traits can expose field-like computed readers with `property`.

Canonical forms:

- `property display_name -> str:`
- `return self.first + " " + self.last`

### Symbol, method, and variant aliases

- **Id:** `SymbolAliases`
- **Category:** `Syntax`
- **Since:** `0.3`
- **RFC:** `RFC 083`
- **Stability:** `Stable`
- **Activation:** None.
- **Use instead of:** Wrapper functions or duplicated enum variants used only for compatibility names.
- **References:** [Symbol aliases](symbol_aliases.md), [Imports and modules](imports_and_modules.md), [Release 0.3](../../release_notes/0_3.md)

Aliases expose another resolved name for the same declaration, method, or enum variant without duplicating behavior.

Canonical forms:

- `pub average = alias avg`
- `mean = avg`
- `WARNING = alias WARN`

### Callable presets with `partial`

- **Id:** `CallablePresets`
- **Category:** `Syntax`
- **Since:** `0.3`
- **RFC:** `RFC 084`
- **Stability:** `Stable`
- **Activation:** None.
- **Use instead of:** Hand-written wrappers whose only job is to pass the same keyword defaults.
- **References:** [Callable presets](callable_presets.md), [Callable presets explained](../explanation/callable_presets.md), [Release 0.3](../../release_notes/0_3.md)

`partial` creates a callable surface from an existing callable by supplying named preset values.

Canonical forms:

- `pub get = partial route(method="GET")`
- `set_alive = partial set_state(state=true)`

### Rest parameters, unpacking, and spreads

- **Id:** `VariadicAndSpreadCalls`
- **Category:** `Syntax`
- **Since:** `0.3`
- **RFC:** `RFC 038`
- **Stability:** `Stable`
- **Activation:** None.
- **Use instead of:** Manually spelling every forwarding arity or merging collections one element at a time.
- **References:** [Functions and calls](functions.md), [Release 0.3](../../release_notes/0_3.md)

Functions can capture `*args` / `**kwargs`; calls and literals support typed unpack/spread forms.

Canonical forms:

- `def log(*items: str, **fields: str) -> None:`
- `f(*xs, **kw)`
- `[*prefix, item]`
- `{**base, "x": 1}`

### User-defined decorators

- **Id:** `UserDefinedDecorators`
- **Category:** `Syntax`
- **Since:** `0.3`
- **RFC:** `RFC 036`
- **Stability:** `Stable`
- **Activation:** None for user-defined decorators; compiler-owned decorators keep their documented imports.
- **Use instead of:** Boilerplate wrapper declarations around every function that needs the same callable transform.
- **References:** [Language reference](language.md#decorators), [Derives and traits](derives_and_traits.md), [Release 0.3](../../release_notes/0_3.md)

Decorators are ordinary callable values applied to functions and methods, including generic decorator factories that infer or accept the decorated function type and decorator helpers that expose `func.__name__`.

Canonical forms:

- `@logged`
- `@registered("catalog.ref")`
- `func.__name__`
- `@registered[(str) -> ColumnExpr]("catalog.ref")`

### Generators

- **Id:** `Generators`
- **Category:** `Syntax`
- **Since:** `0.3`
- **RFC:** `RFC 006`
- **Stability:** `Stable`
- **Activation:** None.
- **Use instead of:** Eager list construction when callers only need lazy iteration.
- **References:** [Generators](generators.md), [Generators how-to](../how-to/generators.md), [Release 0.3](../../release_notes/0_3.md)

`yield`-based functions and generator expressions produce lazy `Generator[T]` values.

Canonical forms:

- `def numbers() -> Generator[int]:`
- `yield value`
- `(x * 2 for x in values)`

### Iterator adapters and terminal consumers

- **Id:** `IteratorAdapters`
- **Category:** `Stdlib`
- **Since:** `0.3`
- **RFC:** `RFC 088`
- **Stability:** `Stable`
- **Activation:** Use iterator values.
- **Use instead of:** Manual loop accumulators for ordinary map/filter/fold pipeline shapes.
- **References:** [Collection protocols](stdlib_traits/collection_protocols.md), [Release 0.3](../../release_notes/0_3.md)

Iterator pipelines expose lazy adapters and explicit terminal consumers.

Canonical forms:

- `values.iter().map(parse).filter(valid).collect()`
- `items.enumerate().take(10)`
- `numbers.fold(0, add)`

### `Result[T, E]` combinators

- **Id:** `ResultCombinators`
- **Category:** `Stdlib`
- **Since:** `0.3`
- **RFC:** `RFC 070`
- **Stability:** `Stable`
- **Activation:** Use `Result[T, E]` values.
- **Use instead of:** Nested matches that only rewrap `Ok` / `Err` around one transformed branch.
- **References:** [std.result](stdlib/result.md), [Fallible and infallible paths](../tutorials/fallible_and_infallible_paths.md), [Release 0.3](../../release_notes/0_3.md)

`Result` values support branch-local transforms, fallible chaining, recovery, and inspection taps.

Canonical forms:

- `result.map(transform)`
- `result.and_then(validate)`
- `result.inspect(log_success)`

### Protocol hooks for core syntax

- **Id:** `ProtocolHooks`
- **Category:** `TypeSystem`
- **Since:** `0.3`
- **RFC:** `RFC 068`
- **Stability:** `Stable`
- **Activation:** Define compatible dunder hooks and adopt/document the corresponding trait vocabulary where useful.
- **Use instead of:** Special-casing custom types in caller code instead of giving the type the expected protocol.
- **References:** [Traits as language hooks](../explanation/traits_as_language_hooks.md), [Collection protocols](stdlib_traits/collection_protocols.md), [Operators](stdlib_traits/operators.md)

User-defined types can participate in truthiness, length, membership, iteration, indexing, assignment, and calls.

Canonical forms:

- `def __len__(self) -> int:`
- `def __iter__(self) -> Iterator[T]:`
- `def __call__(self, value: T) -> U:`

### Rust trait adoption from Incan

- **Id:** `RustTraitAdoption`
- **Category:** `Interop`
- **Since:** `0.3`
- **RFC:** `RFC 043`
- **Stability:** `Stable`
- **Activation:** Import the Rust trait metadata and adopt with `with TraitName`.
- **Use instead of:** Writing Rust-shaped `impl Trait for Type` concepts in comments or custom backend code.
- **References:** [Rust interop](../how-to/rust_interop.md), [Derives and traits](derives_and_traits.md), [Release 0.3](../../release_notes/0_3.md)

Newtype and rusttype declarations can author Rust trait impls with Incan adoption syntax.

Canonical forms:

- `type UserId = rusttype i64 with Display:`
- `def fmt(self, f: Formatter) for Display -> Result[None, FmtError]:`
- `type Output for Add[int] = UserId`

### Targeted generated-Rust lint suppression

- **Id:** `RustAllow`
- **Category:** `Interop`
- **Since:** `0.3`
- **RFC:** `RFC 057`
- **Stability:** `Stable`
- **Activation:** Use `@rust.allow(...)` on supported declarations.
- **Use instead of:** Project-wide lint disables or broad generated-Rust allowance groups.
- **References:** [Rust interop](../how-to/rust_interop.md#targeted-generated-rust-lint-suppression), [Release 0.3](../../release_notes/0_3.md)

Generated Rust can receive narrow lint suppressions on individual items when source semantics require them.

Canonical forms:

- `@rust.allow("dead_code")`
- `def helper() -> None:`

### Scoped DSL surfaces

- **Id:** `ScopedDslSurfaces`
- **Category:** `Syntax`
- **Since:** `0.3`
- **RFC:** `RFC 040`
- **Stability:** `Stable`
- **Activation:** Import a vocab package that publishes scoped surface descriptors.
- **Use instead of:** Global parser changes for syntax that only belongs to one imported DSL.
- **References:** [Authoring vocab crates](../../contributing/how-to/authoring_vocab_crates.md), [Release 0.3](../../release_notes/0_3.md)

Library vocab crates can activate declaration, clause, glyph, leading-dot, and scoped symbol syntax inside their own DSL blocks.

Canonical forms:

- `query:`
- `.field`
- `|>`
- `sum(value)`

### `std.collections` specialized containers

- **Id:** `StdCollections`
- **Category:** `Stdlib`
- **Since:** `0.3`
- **RFC:** `RFC 030`
- **Stability:** `Stable`
- **Activation:** Import from `std.collections`.
- **Use instead of:** Encoding specialized container behavior in plain `list`, `dict`, or `set` plus ad hoc helpers.
- **References:** [std.collections](stdlib/collections.md), [Choosing collections](../how-to/choosing_collections.md), [Release 0.3](../../release_notes/0_3.md)

Specialized containers cover deque, counter, default dict, ordered/sorted maps and sets, chain maps, and priority queues.

Canonical forms:

- `from std.collections import Deque, Counter, PriorityQueue`
- `queue = Deque[int]()`

### `std.graph` directed graph types

- **Id:** `StdGraph`
- **Category:** `Stdlib`
- **Since:** `0.3`
- **RFC:** `RFC 047`
- **Stability:** `Stable`
- **Activation:** Import from `std.graph`.
- **Use instead of:** Hand-rolled adjacency maps for ordinary dependency, plan, or workflow graphs.
- **References:** [std.graph](stdlib/graph.md), [Release 0.3](../../release_notes/0_3.md)

Graph types provide stable node/edge ids, DAG invariants, adjacency queries, traversal, and topological ordering.

Canonical forms:

- `from std.graph import DiGraph, Dag`
- `graph = DiGraph[Task]()`

### `std.fs` filesystem APIs

- **Id:** `StdFs`
- **Category:** `Stdlib`
- **Since:** `0.3`
- **RFC:** `RFC 055`
- **Stability:** `Stable`
- **Activation:** Import from `std.fs` or submodules such as `std.fs.path`.
- **Use instead of:** One-off Rust filesystem wrappers for ordinary path and file work.
- **References:** [std.fs](stdlib/fs.md), [File IO](../how-to/file_io.md), [Release 0.3](../../release_notes/0_3.md)

Path-centric filesystem APIs cover paths, files, metadata, traversal, globbing, copy/move/delete, and durability syncs.

Canonical forms:

- `from std.fs import Path`
- `Path("data").join("orders.csv")`

### `std.io` in-memory binary streams

- **Id:** `StdIo`
- **Category:** `Stdlib`
- **Since:** `0.3`
- **RFC:** `RFC 056`
- **Stability:** `Stable`
- **Activation:** Import from `std.io`.
- **Use instead of:** Byte-twiddling helpers with unclear endian or cursor semantics.
- **References:** [std.io](stdlib/io.md), [Release 0.3](../../release_notes/0_3.md)

Binary stream APIs cover `BytesIO`, endian-aware reads/writes, cursor helpers, delimiter operations, and buffer extraction.

Canonical forms:

- `from std.io import BytesIO, Endian`
- `stream.write(value, Endian.Little)`

### `std.environ` runtime environment access

- **Id:** `StdEnviron`
- **Category:** `Stdlib`
- **Since:** `0.5`
- **RFC:** `RFC 089`
- **Stability:** `Stable`
- **Activation:** Import from `std.environ`.
- **Use instead of:** Direct `rust::std::env` imports or shell glue for ordinary runtime environment reads.
- **References:** [std.environ](stdlib/environ.md), [RFC 089](../../RFCs/089_std_environ.md)

`std.environ` provides read-only current-process Unicode environment reads through required, optional, and defaulted string helpers.

Canonical forms:

- `from std.environ import get, get_optional, get_or`
- `token = get("API_TOKEN")?`
- `mode = get_or("APP_MODE", "dev")`

### `std.json` dynamic JSON values

- **Id:** `StdJson`
- **Category:** `Stdlib`
- **Since:** `0.3`
- **RFC:** `RFC 051`
- **Stability:** `Stable`
- **Activation:** Import from `std.json`.
- **Use instead of:** Ad hoc dictionaries or over-modeled schemas for payloads whose shape is intentionally open.
- **References:** [std.json](stdlib/json.md), [Derives: Serialization](derives/serialization.md), [Release 0.3](../../release_notes/0_3.md)

`JsonValue` provides dynamic parse-inspect-transform JSON workflows with checked optional indexing, explicit shape inspection, mutation helpers, traversal, and typed-model interop.

Canonical forms:

- `from std.json import JsonValue`
- `JsonValue.parse(source)`
- `value["key"]`
- `value[0]`

### `std.tempfile` temporary resources

- **Id:** `StdTempfile`
- **Category:** `Stdlib`
- **Since:** `0.3`
- **RFC:** `RFC 010`
- **Stability:** `Stable`
- **Activation:** Import from `std.tempfile`.
- **Use instead of:** Manual random path generation or unchecked cleanup around temporary files.
- **References:** [std.tempfile](stdlib/tempfile.md), [Release 0.3](../../release_notes/0_3.md)

Temporary files and directories are explicit resources with cleanup and persist semantics.

Canonical forms:

- `NamedTemporaryFile.try_new()`
- `TemporaryDirectory.try_new()`
- `tmp.persist()`

### `std.datetime` temporal values

- **Id:** `StdDatetime`
- **Category:** `Stdlib`
- **Since:** `0.3`
- **RFC:** `RFC 058`
- **Stability:** `Stable`
- **Activation:** Import from `std.datetime` modules or prelude.
- **Use instead of:** Raw strings or integer timestamps inside code that has date/time semantics.
- **References:** [std.datetime](stdlib/datetime.md), [Dates and times](../tutorials/dates_and_times.md), [Dates and times how-to](../how-to/dates_and_times.md)

Temporal APIs cover runtime timing, civil dates/times, fixed offsets, parsing/formatting, intervals, and calendar arithmetic.

Canonical forms:

- `Date.utc_today()`
- `DateTime.utc_now()`
- `TimeDelta(days=1)`

### `std.telemetry.core` data model

- **Id:** `StdTelemetryCore`
- **Category:** `Stdlib`
- **Since:** `0.3`
- **RFC:** `RFC 072`
- **Stability:** `Stable`
- **Activation:** Import from `std.telemetry.core` or the `std.telemetry` prelude.
- **Use instead of:** Stringifying structured observability fields before they reach logging or telemetry boundaries.
- **References:** [std.logging](stdlib/logging.md), [Release 0.3](../../release_notes/0_3.md)

Telemetry core provides structured values, attributes, resources, scopes, and trace context identifiers without configuring providers or exporters.

Canonical forms:

- `from std.telemetry.core import TelemetryValue, Attributes`
- `TelemetryValue.string("ready")`
- `Attributes.from_string_fields(fields)`

### `std.logging` structured logging

- **Id:** `StdLogging`
- **Category:** `Stdlib`
- **Since:** `0.3`
- **RFC:** `RFC 072`
- **Stability:** `Stable`
- **Activation:** Import from `std.logging`; ambient `log` is available for the current module logger.
- **Use instead of:** Printing diagnostic strings or routing ordinary application logging through custom Rust shims.
- **References:** [std.logging](stdlib/logging.md), [Release 0.3](../../release_notes/0_3.md)

Structured logging includes levels, named loggers, bound fields, formatting, JSON rendering, and telemetry values.

Canonical forms:

- `from std.logging import Level, basic_config`
- `log.info("started", fields={"component": "worker"})`

### Testing assertions and markers

- **Id:** `TestingAssertions`
- **Category:** `Testing`
- **Since:** `0.3`
- **RFC:** `RFC 018`
- **Stability:** `Stable`
- **Activation:** Use `assert` directly; import marker/helper APIs from `std.testing`.
- **Use instead of:** Ad hoc panic helpers or external test metadata formats for ordinary Incan tests.
- **References:** [std.testing](stdlib/testing.md), [Testing how-to](../how-to/testing_stdlib.md), [Release 0.3](../../release_notes/0_3.md)

Tests can use language assertions, raises checks, helper assertions, fixtures, parametrization, and marker decorators.

Canonical forms:

- `assert value == expected`
- `assert call() raises ValueError`
- `@parametrize("case", cases)`

### `incan test` runner

- **Id:** `TestRunner`
- **Category:** `Testing`
- **Since:** `0.3`
- **RFC:** `RFC 019`
- **Stability:** `Stable`
- **Activation:** Run `incan test`.
- **Use instead of:** Project-local scripts that duplicate core test discovery and reporting behavior.
- **References:** [Tooling: testing](../../tooling/how-to/testing.md), [std.testing](stdlib/testing.md), [Release 0.3](../../release_notes/0_3.md)

The runner owns discovery, inline test modules, stable ids, selection, fixtures, parametrization, reporting, shuffling, and scheduling.

Canonical forms:

- `module tests:`
- `incan test --list`
- `incan test --format json --junit report.xml`

### Async and await

- **Id:** `AsyncAwait`
- **Category:** `Async`
- **Since:** `0.2`
- **RFC:** `RFC 023`
- **Stability:** `Stable`
- **Activation:** Import `std.async` or one of its submodules.
- **Use instead of:** Threading async behavior through synchronous wrappers or relying on pre-0.2 ambient async syntax.
- **References:** [Async programming](../how-to/async_programming.md), [std.async](stdlib/async.md), [Release 0.2](../../release_notes/0_2.md)

`async` and `await` are import-activated soft-keyword surfaces backed by `std.async` modules.

Canonical forms:

- `from std.async.time import sleep`
- `async def main() -> None:`
- `await sleep(1)`

### Async race and awaitability

- **Id:** `AsyncRace`
- **Category:** `Async`
- **Since:** `0.3`
- **RFC:** `RFC 039`
- **Stability:** `Stable`
- **Activation:** Import `std.async.race` or the relevant async prelude helpers.
- **Use instead of:** Legacy `std.async.select` or hand-rolled polling loops.
- **References:** [Awaitable trait](stdlib_traits/awaitable.md), [Async programming](../how-to/async_programming.md), [Release 0.3](../../release_notes/0_3.md)

`Awaitable[T]`, `race for`, and helper-style race composition support first-ready async workflows.

Canonical forms:

- `race for value:`
- `arm(task)`
- `race(arms)`

### Project lifecycle tooling

- **Id:** `ProjectLifecycle`
- **Category:** `Tooling`
- **Since:** `0.3`
- **RFC:** `RFC 015`
- **Stability:** `Stable`
- **Activation:** Use `incan init`, `incan new`, `incan version`, or `incan env`.
- **Use instead of:** One-off project scaffolding scripts or manual version-file edits.
- **References:** [Project lifecycle](project_lifecycle.md), [Project lifecycle how-to](../how-to/project_lifecycle.md), [Release 0.3](../../release_notes/0_3.md)

Project commands create scaffolds, manage versions, and run configured environments from `incan.toml`.

Canonical forms:

- `incan new greeter --yes`
- `incan version patch`
- `incan env run dev test`

### Toolchain installer and release manifest

- **Id:** `ToolchainInstallerManifest`
- **Category:** `Tooling`
- **Since:** `0.4`
- **RFC:** `RFC 015`
- **Stability:** `Stable`
- **Activation:** Use the GitHub Release installer, Homebrew formula, npm package, pipx package, or versioned toolchain manifest.
- **Use instead of:** Treating Cargo, repository checkouts, and package-manager adapters as separate sources of install truth.
- **References:** [Install and run](../../tooling/how-to/install_and_run.md), [Release 0.4](../../release_notes/0_4.md)

The toolchain install path is manifest-driven, checksum-verified, and installs `incan` plus `incan-lsp` from the same release archives across direct and package-manager channels. Direct, npm, and pipx installation can provision stable Rust and `wasm32-wasip1`; Homebrew installs the prebuilt command binaries and leaves Rust management to the user.

Canonical forms:

- `curl -fsSL https://github.com/encero-systems/incan/releases/latest/download/install.sh | bash`
- `brew tap encero-systems/tap && brew install incan`
- `npm install -g @incan/toolchain`
- `pipx install incan`

### Zero-clone starter project flow

- **Id:** `ZeroCloneStarterFlow`
- **Category:** `Tooling`
- **Since:** `0.4`
- **RFC:** `RFC 015`
- **Stability:** `Stable`
- **Activation:** Use `incan new`, then run project commands from the generated directory.
- **Use instead of:** Cloning the compiler repository or copying examples manually before a first run.
- **References:** [Getting started](../../tooling/tutorials/getting_started.md), [Project lifecycle](project_lifecycle.md), [Release 0.4](../../release_notes/0_4.md)

`incan new` creates a runnable, testable project with a manifest, entrypoint, starter test, README, `.gitignore`, and release-line toolchain constraint.

Canonical forms:

- `incan new hello --yes`
- `cd hello`
- `incan run`
- `incan test`
- `incan build --release`

### Stable diagnostics commands

- **Id:** `StableDiagnostics`
- **Category:** `Tooling`
- **Since:** `0.4`
- **RFC:** `RFC 015`
- **Stability:** `Stable`
- **Activation:** Use `incan check` or `incan explain`.
- **Use instead of:** Scraping terminal diagnostics or building tool-specific error-code maps.
- **References:** [CLI reference](../../tooling/reference/cli_reference.md), [Release 0.4](../../release_notes/0_4.md)

Type-check diagnostics can be emitted as versioned JSON with stable codes, source spans, and catalog-backed explanations.

Canonical forms:

- `incan check src/main.incn --format json`
- `incan explain INCAN-T0001`

### Build reports and generated Rust inspection

- **Id:** `BuildReportsAndRustInspection`
- **Category:** `Tooling`
- **Since:** `0.4`
- **RFC:** `RFC 015`
- **Stability:** `Stable`
- **Activation:** Use `incan build --report json` or `incan inspect rust`.
- **Use instead of:** Scraping terminal progress output or treating `--emit-rust` debug output as a stable artifact contract.
- **References:** [CLI reference](../../tooling/reference/cli_reference.md), [Release 0.4](../../release_notes/0_4.md)

Builds can emit versioned machine-readable reports, generated Rust can be inspected intentionally as current backend output, and generated public Rust items preserve checked source docstrings as Rust doc comments when available.

Canonical forms:

- `incan build src/main.incn --report json`
- `incan build --lib --report json --report-output build.json`
- `incan inspect rust src/main.incn --format json`

### Build and test preheat observability

- **Id:** `BuildTestPreheatObservability`
- **Category:** `Tooling`
- **Since:** `0.4`
- **RFC:** `RFC 015`
- **Stability:** `Stable`
- **Activation:** Use verbose test output or `INCAN_RUST_INSPECT_TIMING=1` when build/test preheat timing needs to be inspected.
- **Use instead of:** Silent long-running preheat phases or separate probes that do not share the real build/test cache policy.
- **References:** [CLI reference](../../tooling/reference/cli_reference.md), [Release 0.4](../../release_notes/0_4.md)

Build and test preheat paths report phase timing, Cargo target reuse, Rust metadata warmed/reused/skipped counts, vocab companion cache reuse, and cache split reasons.

Canonical forms:

- `incan test -v tests`
- `INCAN_RUST_INSPECT_TIMING=1 incan build --lib`
- `incan build --lib`

### Compiler-backed codegraph inspection

- **Id:** `CodegraphInspection`
- **Category:** `Tooling`
- **Since:** `0.4`
- **RFC:** `RFC 106`
- **Stability:** `Stable`
- **Activation:** Use `incan inspect codegraph`.
- **Use instead of:** Repeated grep/read loops or tool-specific source scrapers when agents and tooling need basic Incan structure.
- **References:** [Codegraph inspection](../../tooling/reference/codegraph_inspection.md), [CLI reference](../../tooling/reference/cli_reference.md), [Release 0.4](../../release_notes/0_4.md)

Incan-language source files, modules, declarations, imports, exports, body-level reference and call syntax, conservative resolved reference and call targets, containment, spans, provenance, language tags, degraded state, and diagnostics can be exported as deterministic JSONL records.

Canonical forms:

- `incan inspect codegraph src/main.incn --format jsonl`
- `incan inspect codegraph src --format jsonl --allow-errors`

### Checked API metadata

- **Id:** `CheckedApiMetadata`
- **Category:** `Tooling`
- **Since:** `0.3`
- **RFC:** `RFC 048`
- **Stability:** `Stable`
- **Activation:** Use `incan tools metadata api` or LSP metadata commands.
- **Use instead of:** Scraping source text or generated Rust when tooling needs API contracts.
- **References:** [Release 0.3](../../release_notes/0_3.md), [Project lifecycle](project_lifecycle.md)

Typechecked public APIs can emit structured metadata for docs, manifests, hovers, and model bundle tooling.

Canonical forms:

- `incan tools metadata api src/lib.incn`
- `incan tools metadata model emit`

### Formatter spacing and wrapping contract

- **Id:** `FormatterContract`
- **Category:** `Tooling`
- **Since:** `0.3`
- **RFC:** `RFC 053`
- **Stability:** `Stable`
- **Activation:** Run `incan fmt`.
- **Use instead of:** Hand-maintained whitespace conventions that drift from the formatter.
- **References:** [Code style](code_style.md), [Formatting how-to](../../tooling/how-to/formatting.md), [Release 0.3](../../release_notes/0_3.md)

Formatter output has explicit vertical-spacing buckets, docstring normalization, comment attachment, and common wrapping rules.

Canonical forms:

- `incan fmt src/main.incn`
- `incan fmt --check`
