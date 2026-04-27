# RFC 072: Ambient `log` Surface with Tracing-Backed Runtime Policy

- **Status:** Draft
- **Created:** 2026-04-23
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 022 (namespaced stdlib modules and compiler handoff)
    - RFC 023 (compilable stdlib and Rust module binding)
    - RFC 033 (`ctx` typed configuration context)
    - RFC 041 (first-class Rust interop authoring)
    - RFC 063 (`std.process` process spawning and command execution)
    - RFC 066 (`std.http` HTTP client surface)
- **Issue:** https://github.com/dannys-code-corner/incan/issues/392
- **RFC PR:** —
- **Written against:** v0.2
- **Shipped in:** —

## Summary

This RFC proposes a built-in ambient `log` surface for Incan. Users should write `log.info(...)`, `log.warning(...)`, `log.error(...)`, and optionally scoped operations through `with log.span(...)` without importing or constructing logger objects. The emitted diagnostics model is language-level and backend-agnostic, while the reference Rust runtime maps it onto `tracing` and `tracing-subscriber`. Project defaults live in `incan.toml`, runtime overrides come from CLI flags and environment variables, and built-in human renderers prioritize terminal readability over prefix-heavy verbosity. The canonical event/span model defined here must remain rich enough for a follow-up RFC to add OpenTelemetry export without changing the source-level API.

## Core model

Read this RFC as one foundation plus five mechanisms:

1. **Foundation:** Logging is an ambient language surface, not an imported stdlib logger object and not a Rust interop recipe.
2. **Mechanism A:** Ordinary code emits events through `log.info(...)`, `log.debug(...)`, `log.warning(...)`, `log.error(...)`, `log.critical(...)`, and `log.trace(...)`.
3. **Mechanism B:** Scoped units of work may be expressed through `with log.span(...)`, which groups nested events, carries inherited metadata, and enables duration-aware tracing.
4. **Mechanism C:** Runtime policy is separate from emission. CLI flags and environment variables control what is rendered or exported for a given run, while `incan.toml` provides project defaults.
5. **Mechanism D:** The runtime records structured events and spans even when human output is compact. Human renderers, JSON output, and future external exporters are presentation and delivery concerns over one canonical payload.
6. **Mechanism E:** The reference Rust runtime should use `tracing` and `tracing-subscriber` internally, but those crates must not define the Incan source-language contract.

## Motivation

Logging is one of the first places where a language either feels credible for real applications or falls back to awkward glue. CLIs, services, automation scripts, libraries, test helpers, and future query-language surfaces all need one coherent diagnostics story. Today, the escape hatch is Rust interop. That proves capability, but not language design. It leaks Rust crate names, backend assumptions, and macro-shaped habits into code that should stay ordinary Incan.

The ordinary Python answer is also not sufficient. Python's logging module gets the basics right for many users, but it assumes a runtime world that Incan is not obligated to copy. Incan is not constrained to threadlocals, ad hoc adapters, or logger-object plumbing. It can deliberately choose a simpler source surface because the compiler and runtime can cooperate.

The real use-cases also go beyond plain text logging. Purpose-built libraries, pipeline frameworks, query systems, and future application surfaces will need correlation identifiers, operation boundaries, and nested work visibility. That means the design has to separate the easy source surface from the richer runtime model. Plain log messages are not enough if the end-state includes trace-capable spans and future external observability integration.

Prior art points in two directions. Rust's `log` crate gives a facade, while `tracing` models events and spans explicitly. Frameworks such as Koheesio show that users care as much about how logs are consumed as how they are emitted: rich default context, automatic operation grouping, and low boilerplate at call sites matter more than whether a module is named `logging`. Incan should therefore optimize for an ambient, language-native event surface with a tracing-capable runtime behind it.

## Goals

- Provide an ambient `log` surface that exists by default with no `get_logger(...)` or import ceremony in ordinary code.
- Keep the call-site model simple and unsurprising: `log.info(...)`, `log.warning(...)`, and similar method calls.
- Support scoped operation grouping through `with log.span(...)` so nested work, duration, and correlation can be represented cleanly.
- Distinguish emission semantics from runtime policy, project defaults, and future external export.
- Provide built-in human renderers that are readable in terminals, with `short` as the default.
- Provide a machine-oriented JSON mode without forcing JSON as the default terminal experience.
- Allow custom renderers and sinks over the canonical structured event/span payload.
- Keep the source-language contract backend-agnostic while making a `tracing`-backed Rust runtime the recommended implementation architecture.
- Leave room for typed ambient context, including possible `ctx` integration, without requiring `ctx` just to log.

## Non-Goals

- Requiring users to create or acquire logger objects through `get_logger(...)`.
- Exposing Rust crate names such as `tracing`, `log`, or OpenTelemetry packages in ordinary Incan source code.
- Defining a full distributed tracing surface area or exporter configuration matrix in the first version.
- Standardizing every knob of `tracing-subscriber` or every backend-specific formatter and transport.
- Making JSON the default human-facing terminal output.
- Making the logging surface dependent on `ctx`; `ctx` integration is additive, not foundational.
- Treating the first version as a text-only `print` replacement with no structured event model underneath.

## Guide-level explanation

### Basic use

Logging should exist by default. Ordinary code should not need imports or logger construction:

```incan
def main() -> None:
    log.info("Starting session")
```

The common path is intentionally boring:

- `log.trace(...)` for very low-priority diagnostic events
- `log.debug(...)` for development-oriented detail
- `log.info(...)` for ordinary operational messages
- `log.warning(...)` for degraded-but-continuing behavior
- `log.error(...)` for failures in the current operation
- `log.critical(...)` for severe failures

### Scoped operations

Some work is more than one message. Query execution, pipeline steps, request handling, and export jobs all have a start, nested work, and an end. Those should be expressible as a scoped operation:

```incan
def run_query(query: str) -> ResultFrame:
    with log.span("run_query"):
        log.info("Parsing query")
        parsed = parse_query(query)

        log.info("Planning query")
        plan = build_plan(parsed)

        log.info("Executing query")
        return execute_plan(plan)
```

The value of the span is not syntax cleverness. The value is that the runtime can group those events under one operation, measure duration, and preserve a trace-capable structure for human renderers, JSON output, and future export integrations.

### Nested work

Scoped operations may nest:

```incan
def compile_query(plan: Plan) -> CompiledQuery:
    with log.span("compile_query"):
        with log.span("lower_plan"):
            lowered = lower_plan(plan)

        with log.span("emit_sql"):
            sql = emit_sql(lowered)

        return CompiledQuery(sql=sql)
```

The human renderer should make that hierarchy visible, while structured output should preserve the full parent-child relationship.

### Runtime overrides vs project defaults

Project defaults belong in `incan.toml`:

```toml
[logging]
default_level = "warning"
default_style = "short"
default_format = "human"
timestamp_format = "%H:%M:%S"
color = "auto"
```

Runtime overrides belong to the run command or the environment:

```text
incan run --log-level=debug
incan run --log-style=complete
INCAN_LOG_LEVEL=debug incan run
INCAN_LOG_STYLE=minimal incan run
```

The runtime values override the project defaults. A project can declare a sensible baseline without taking control away from operators or developers running the program.

### Color policy

Color belongs to the human renderer layer, not to the canonical event/span payload. Human renderers should support color policies such as `auto`, `always`, and `never`.

Illustrative shape:

```toml
[logging]
default_level = "warning"
default_style = "short"
default_format = "human"
color = "auto"
```

`auto` should enable color in interactive terminal contexts and disable it when the output is redirected or consumed as plain text. JSON output should remain colorless regardless of the human renderer color policy.

### Renderer templates

Human renderers should be configured through templates in the runtime policy layer rather than through language syntax. Built-in human styles and custom house styles should use the same renderer template system.

Illustrative shape:

```toml
[logging]
default_level = "warning"
default_style = "short"
default_format = "human"

[logging.renderers.minimal]
template = "[{level}] {message}"

[logging.renderers.short]
template = "{timestamp} {tree_prefix}[{level}] {message}"
timestamp_format = "%H:%M:%S"

[logging.renderers.complete]
template = "{timestamp} {tree_prefix}[{level}] {message}"
timestamp_format = "%Y-%m-%dT%H:%M:%S.%fZ"

[logging.renderers.verbose]
template = "{timestamp} {tree_prefix}[{level}] {message}\n  target={target} trace={trace_id} span={span_id} run={run_id}"
timestamp_format = "%Y-%m-%dT%H:%M:%S.%fZ"

[logging.renderers.house_style]
template = "[{run_id}] [{timestamp}] [{level}] [{target}] {{{file}:{function}:{line}}} - {message}"
timestamp_format = "%Y-%m-%d %H:%M:%S"
```

The template language should borrow the familiar named-placeholder model used by Python formatters, but it may extend that model with logging-specific fields such as `{tree_prefix}`, `{trace_id}`, `{span_id}`, and other structured diagnostics metadata. Projects that need to match an existing ecosystem's house style, such as Koheesio, should do so through a project-defined renderer profile rather than by making that external name part of the built-in contract.

### Human output styles

The runtime must provide built-in human styles. `short` should be the default because it keeps timestamps and hierarchy without overwhelming the terminal. Those built-in styles are named defaults over the same renderer-template mechanism available to custom project renderers.

Illustrative shapes:

`minimal`

```text
[INFO] starting query
└─ [INFO] lowering plan
└─ [INFO] emitting sql
└─ [INFO] query complete
```

`short`

```text
21:18:04 [INFO] starting query
21:18:04 └─ [INFO] lowering plan
21:18:04 └─ [INFO] emitting sql
21:18:04 └─ [INFO] query complete
```

`complete`

```text
2026-04-23T21:18:04.221Z [INFO] starting query
2026-04-23T21:18:04.228Z └─ [INFO] lowering plan
2026-04-23T21:18:04.240Z └─ [INFO] emitting sql
2026-04-23T21:18:04.261Z └─ [INFO] query complete
```

`verbose`

```text
2026-04-23T21:18:04.221Z [INFO] starting query
  target=sample_query.query trace=7f9c span=01 run=abc123
2026-04-23T21:18:04.228Z └─ [INFO] lowering plan
  target=sample_query.query.lowering trace=7f9c span=02 parent=01 run=abc123
```

The exact spacing and glyphs are renderer choices, but the first line must stay message-first and terminal-readable. Human styles should not sacrifice readability for metadata density.

### Machine-oriented output

JSON output is a format choice, not a style. It is appropriate for machine consumers, collectors, and ingestion pipelines, not as the default human terminal experience.

```text
incan run --log-format=json
```

### Custom presentation and delivery

Teams that want a house style should be able to define custom renderers and sinks. That extensibility belongs at the runtime presentation and delivery layer, not in the source-level emission API. Ordinary Incan code should keep using the standard `log` surface regardless of how a project renders or exports its diagnostics.

## Reference-level explanation

### Ambient surface

The language must provide an ambient `log` surface in every ordinary execution context. Users must not need to import a logging module or call `get_logger(...)` before emitting events.

The ambient surface must provide, at minimum:

- `log.trace(message, ...)`
- `log.debug(message, ...)`
- `log.info(message, ...)`
- `log.warning(message, ...)`
- `log.error(message, ...)`
- `log.critical(message, ...)`
- `with log.span(name, ...)`

The first argument to each event method must be a message expression. Additional named arguments on `log.info(...)`, `log.debug(...)`, and the other event methods must be treated as structured event fields. Additional named arguments on `with log.span(...)` must be treated as structured span fields. The runtime payload must preserve those fields as structured data rather than flattening them into one interpolated string.

### Levels

The standard level set must include:

- `TRACE`
- `DEBUG`
- `INFO`
- `WARNING`
- `ERROR`
- `CRITICAL`

User-facing APIs must expose `warning`, not only `warn`.

### Event records

Every emitted event record must preserve, at minimum:

- timestamp
- level
- message
- source identity sufficient for human rendering and debugging
- any structured fields provided at the call site
- active operation context, if the event is emitted inside a span

The runtime may record more metadata than a human renderer shows. Human rendering must be a projection of the structured event, not the event itself.

### Structured field syntax

Structured fields must use keyword-style arguments at the call site.

Illustrative shapes:

```incan
log.info("Loaded dataset rows", dataset=dataset, rows=row_count)

with log.span("run_query", query_id=query_id, dataset=dataset_name):
    ...
```

The runtime must preserve these named arguments as structured event or span fields. It must not treat them as mere string interpolation aids.

### Spans

`with log.span(...)` must represent a scoped unit of work with a beginning and an end. Events emitted inside a span must be associated with that span. Nested spans must preserve parent-child structure.

The runtime must preserve, at minimum:

- span name
- start time
- end time or duration
- parent span identity, when nested
- active contextual metadata inherited by nested events

The runtime must not flatten spans into ordinary standalone log lines that lose structural information.

### Human renderers

The runtime must provide built-in human renderers with at least the following style names:

- `minimal`
- `short`
- `complete`
- `verbose`

`short` must be the default human style.

Normative rendering expectations:

- `minimal` must omit timestamps.
- `short` must render a compact time-of-day prefix.
- `complete` must render a full datetime prefix.
- `verbose` may render one additional metadata continuation line per event or span, but it must remain human-readable and message-first.

The runtime must allow built-in human styles to be expressed through the same renderer-template mechanism available to custom renderers. Timestamp rendering must be configurable separately from the surrounding message template through a standard timestamp-format setting.

Hierarchy in human renderers should be visible through lightweight layout cues such as glyphs or guides rather than long repeated target prefixes. The renderer does not need to reproduce the exact finalized tree glyph topology in a live stream as long as nesting remains visually obvious.

### Machine format

The runtime must support a machine-oriented JSON output format that preserves the canonical structured event/span payload. JSON is a format choice and must not replace the built-in human styles as the default terminal experience.

### Project defaults vs runtime overrides

Project defaults may be declared in `incan.toml`. Runtime overrides may be provided through CLI flags and environment variables.

The precedence order must be:

1. CLI flags
2. environment variables
3. project defaults from `incan.toml`
4. built-in runtime default

At minimum, the runtime must support policy control over:

- log level
- human style
- output format
- renderer template selection
- timestamp format
- color policy

The exact option names may vary, but the distinction between project defaults and runtime overrides must remain explicit.

### Future export compatibility

The canonical structured event/span payload defined by this RFC must preserve enough information that a follow-up RFC can add structured export and OpenTelemetry-compatible tracing without changing the source-level `log` API. The language-level contract must remain backend-agnostic.

### Custom renderers and sinks

The runtime must allow custom renderers and sinks to consume the canonical structured event/span payload. Projects may define custom presentation and delivery behavior, including renderer templates that match surrounding house styles such as Koheesio, but they must not change the meaning of the source-level `log` API.

### Color handling

The canonical structured event/span payload must be colorless. Color is a presentation concern of human renderers only.

The runtime must support, at minimum:

- `auto`
- `always`
- `never`

When color is enabled, the renderer may colorize level markers, hierarchy glyphs, timestamps, or other human-facing fragments, but it must not encode color into JSON output.

## Design details

### Why there is no `get_logger(...)`

Logger-object acquisition is ceremony that ordinary Incan code does not need. It is inherited from library-shaped ecosystems, not a necessary property of logging itself. Since Incan controls both the language surface and the runtime boundary, it can provide an ambient `log` surface directly and avoid forcing users to thread logger values or repeat boilerplate in every module.

### Why the source surface stays method-shaped

The design deliberately keeps `log.info(...)` and similar method calls rather than introducing a floating statement form. The dot-method shape is familiar, readable, and easy to extend without creating a special DSL that feels isolated from the rest of the language.

### Why structured fields use keyword arguments

Keyword-style arguments are the most direct and readable way to attach named metadata to events and spans. They preserve Python familiarity, avoid an extra `fields={...}` wrapper, and keep the call-site model simple while still producing a structured payload underneath.

### Why spans exist separately from trace-level logs

`trace` as a severity level and spans as scoped operations are different concepts. Reusing one word for both would be confusing. `log.trace(...)` should remain the very low-severity event level, while `with log.span(...)` should represent scoped work and hierarchy.

### Why human output is message-first

Rich metadata is valuable, but not if it obscures the actual log stream in a terminal. The built-in human renderers should therefore keep the message as the primary unit the eye reads first. Timestamps, hierarchy markers, and optional metadata lines support the message rather than competing with it.

### Why JSON is not a style

JSON is a serialization format for machine consumers, not a human terminal style. Treating it as just another style would blur the line between terminal UX and structured export and would encourage noisy defaults in interactive environments.

### Why project defaults and runtime policy are separate

Projects need a baseline policy, but operators and developers still need per-run control. That makes `incan.toml` an appropriate home for defaults and CLI/env an appropriate home for runtime overrides. The runtime must not collapse those into one opaque setting source.

### Why export is split into a follow-up RFC

OpenTelemetry and similar systems are about transporting and correlating diagnostics across processes and systems. They are real requirements, but they add enough propagation and backend-contract detail to deserve a follow-up RFC. This RFC therefore defines the source-level event/span model and runtime policy boundaries that the follow-up export RFC will build on.

### Why custom renderers are allowed

Different applications and teams will want different human output surfaces. Built-in styles are necessary, but not sufficient. Allowing custom renderers over the canonical structured payload gives flexibility without fragmenting the source-language API. Using one template system for both built-in and custom human renderers keeps that flexibility coherent instead of creating two different formatting worlds.

### Why color is runtime policy

Color improves terminal readability, but it is not part of the meaning of an event. The same event may be rendered with color in an interactive terminal, without color in redirected output, and as structured JSON for machines. That makes color a renderer/theme concern rather than part of the language or payload contract.

## Alternatives considered

- **Python-style `get_logger(...)` plus `basic_config(...)`**
  - Rejected because it keeps logger-object ceremony that Incan does not need and over-anchors the design to Python's runtime assumptions.
- **Rust-style logging crates as the public surface**
  - Rejected because ordinary Incan code should not need to know about `log`, `tracing`, subscriber layers, or exporter crates.
- **A floating statement syntax such as `log info "message"`**
  - Rejected for now because it reads like a narrow DSL and does not buy enough over `log.info(...)`.
- **Plain text logging with no span model**
  - Rejected because the end-state includes nested work and trace-capable diagnostics, and must remain compatible with future export.
- **A stdlib-only formatting API with no custom renderer hook**
  - Rejected because real applications will need house styles and custom delivery behavior.

## Drawbacks

- Making logging ambient is a stronger language commitment than shipping another stdlib module.
- `with log.span(...)` introduces a scoped operation concept that will need careful syntax and formatter support.
- The runtime architecture is richer than plain console logging, which raises implementation complexity.
- The style system can sprawl if the built-in options are not kept disciplined and clearly differentiated.
- Keeping the runtime model export-compatible without specifying the export layer here leaves some future integration details intentionally deferred.

## Implementation architecture

*(Non-normative.)* A sensible Rust runtime implementation lowers ambient Incan events and spans into `tracing` events and spans and configures rendering through `tracing-subscriber`. Human renderers should operate over one canonical internal event/span model rather than each sink inventing its own interpretation. Project defaults should be loaded from `incan.toml` before CLI and environment overrides are applied. A follow-up RFC can then define how that canonical model maps onto OpenTelemetry export and propagation.

## Layers affected

- **Language surface**: ambient `log` availability and scoped `with log.span(...)` behavior.
- **Parser / AST**: support for the chosen `with log.span(...)` scoped syntax and any structured field-passing syntax that the language standardizes.
- **Typechecker / Symbol resolution**: resolving the ambient `log` surface, validating event/span call shapes, and enforcing any structured field contracts.
- **IR Lowering**: preserving event/span structure, nesting, and metadata through lowering.
- **Emission**: mapping the canonical diagnostics model onto the target runtime without losing hierarchy or structured data.
- **Stdlib / Runtime (`incan_stdlib`)**: human renderers, JSON output, runtime policy loading, custom sink/renderer extension points, and a canonical internal event/span model that remains export-compatible.
- **Formatter**: predictable formatting for scoped span blocks and any structured log field syntax.
- **LSP / Tooling**: completions, hover docs, and diagnostics for the ambient `log` surface and runtime configuration.
- **Documentation**: guides and references for emission, styles, policy, and runtime/export behavior.

## Unresolved questions

- How much theme/color customization should built-in human renderers expose beyond `auto`, `always`, and `never`?

<!-- Rename this section to "Design Decisions" once all questions have been resolved.
     An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
