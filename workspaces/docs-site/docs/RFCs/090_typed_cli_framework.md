# RFC 090: typed CLI framework

- **Status:** Planned
- **Created:** 2026-05-06
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 015 (hatch-like tooling and project lifecycle CLI)
    - RFC 019 (test runner, CLI, and ecosystem)
    - RFC 021 (model field metadata and schema-safe aliases)
    - RFC 024 (extensible derive protocol)
    - RFC 033 (`ctx` typed configuration context)
    - RFC 063 (`std.process` process spawning and command execution)
    - RFC 073 (environment matrices and toolchain constraints)
    - RFC 083 (symbol and method aliases)
    - RFC 089 (`std.environ` runtime environment access)
    - RFC 117 (`loaf.toml` and Oven's language-neutral project model)
    - RFC 118 (Incan and Oven command-line surfaces)
- **Issue:** https://github.com/encero-systems/incan/issues/87
- **RFC PR:** —
- **Written against:** ~~v0.3~~ v0.5
- **Shipped in:** —

## Summary

This RFC introduces `std.cli`, a typed model-first framework for authoring command-line applications in Incan. A CLI specification is derived from command enums, argument models, reusable field contracts, and a deliberately small metadata surface, then used to parse an argument vector into a typed command value, generate help and usage text, validate user input, render consistent diagnostics, and map outcomes to process exit codes. The framework owns current-program argument parsing and CLI user experience; it does not replace `std.process` for spawning child commands, `std.environ` for direct environment reads, project lifecycle env execution, or the existing `incan` compiler CLI.

## Core model

1. **A CLI spec is typed data:** command structure is declared with ordinary Incan enums, models, reusable fields, descriptions, and defaults rather than a stringly parser builder.
2. **Parsing produces a command value:** `std.cli.parse[T](argv)` validates an argument vector and returns `Result[T, CliError]`, where `T` is a command model or enum that has derived `CliArgs` or `CliCommand` (see "CLI eligibility is a derived trait, not an ambient marker").
3. **One spec drives every surface:** parse behavior, help text, usage text, completions metadata, validation errors, command descriptions, and optional dispatch helpers must all lower to the same underlying CLI spec.
4. **Metadata stays small:** the CLI framework should reuse ordinary `description` text and define only the extra keys required for command-line shape, while the field type, default, optionality, and collection shape remain the source of truth for parsing.
5. **Command execution is separate:** this RFC parses current-program arguments and renders CLI outcomes; child process spawning remains owned by `std.process`.
6. **Environment fallback is delegated:** CLI environment fallback behavior composes with the current-process environment semantics of `std.environ` instead of defining a second environment API.
7. **Dogfooding is optional:** rewriting the `incan` compiler CLI is a useful validation path, but it is not required by this RFC's user-facing contract.

## Motivation

Incan needs to be credible for real tooling, not only libraries and compiler tests. Command-line applications are one of the smallest useful units of software a user can write, and they expose a language's ergonomics quickly: parse a flag, validate a value, show useful help, return a clear exit status, and keep the business logic testable. Without a standard CLI framework, Incan users either hand-roll fragile `argv` parsing, bind directly to Rust or shell helpers, or keep practical tools in another language.

The right Incan-shaped answer should use the language's type system. A command is naturally an enum. Subcommand options are naturally models. Common argument declarations may be reusable fields. Required values, optional values, repeated values, enum choices, defaults, descriptions, and validated newtypes already have type-level or metadata-level homes. The framework should make those declarations executable as a CLI contract instead of asking users to maintain a second parser description.

This also helps library and application tests. A CLI program should be testable as `argv -> typed command -> handler result`, with rendering and process exit behavior kept at the boundary. That keeps command parsing deterministic, lets tests avoid spawning a process for every case, and gives users consistent behavior across independently authored Incan CLIs.

## Goals

- Provide a standard `std.cli` module for typed CLI specifications, parsing, help generation, diagnostics, and exit-code helpers.
- Make enum-plus-model command declarations the primary authoring style.
- Parse `argv` into typed Incan values through `parse[T](argv) -> Result[T, CliError]`.
- Use field types, defaults, optionality, collection shapes, enum variants, reusable field contracts, descriptions, and a small CLI metadata set to derive CLI behavior.
- Support required and optional positionals, named options, boolean flags, repeated options, enum choices, defaults, environment fallbacks, short options, value names, and subcommands.
- Generate deterministic help and usage text from the same spec used for parsing.
- Provide consistent error rendering that names the failing argument, expected shape, and corrective hint where possible.
- Standardize exit code mapping for success, usage errors, and application failures.
- Keep command handlers independent from parsing so CLI logic is easy to unit test.
- Allow command presentation metadata to live in a typed sidecar spec without requiring new enum syntax.
- Generate shell completion scripts for bash, zsh, fish, and PowerShell from the same spec used for parsing and help.

## Non-Goals

- Requiring the `incan` compiler CLI to adopt this framework.
- Replacing `std.process.Command`, `Pipeline`, or shell DSL execution from RFC 063.
- Replacing Oven project lifecycle env execution, matrix execution, or `oven env run` behavior from RFCs 073 and 117.
- Replacing direct runtime environment access from RFC 089.
- Defining terminal UI widgets, progress bars, prompts, interactive menus, curses-style interfaces, or rich text styling.
- Defining dynamic, context-aware, or custom completion callbacks for open-ended values (for example completing a `Path` field against the real filesystem, or completing a value from a live remote lookup). Static completions derived from the spec itself — command names, subcommand names, option/flag spellings, and enum-typed value choices — are in scope; runtime-dependent completion logic is not.
- Defining a general application dependency-injection framework.
- Making decorator CLI authoring a separate parser system.
- Adding enum-variant metadata syntax as a special case for CLI only.
- Changing core language syntax.

## Guide-level explanation

A command-line program starts with a typed command shape. A single-command program can use one model:

```incan
from std.cli import parse

@derive(CliArgs)
model ServeArgs:
    host [description="Address to bind"]: str = "127.0.0.1"
    port [description="TCP port to listen on", short="p", value_name="PORT"]: int = 8000
    reload [description="Restart when source files change"]: bool = false

def main(argv: list[str]) -> int:
    args = parse[ServeArgs](argv)?
    return run_server(args.host, args.port, reload=args.reload)
```

For a multi-command program, an enum defines the command set and each variant carries the model for that command's arguments:

```incan
from std.cli import parse

@derive(CliArgs)
model ServeArgs:
    host [description="Address to bind"]: str = "127.0.0.1"
    port [description="TCP port to listen on", short="p", value_name="PORT"]: int = 8000
    reload [description="Restart when source files change"]: bool = false

@derive(CliArgs)
model BuildArgs:
    release [description="Build optimized artifacts"]: bool = false
    output [description="Write artifacts to this path", short="o", value_name="PATH"]: Path | None = None

@derive(CliCommand)
enum ToolCommand:
    Serve(ServeArgs)
    Build(BuildArgs)

def main(argv: list[str]) -> int:
    command = parse[ToolCommand](argv)?

    match command:
        Serve(args) => run_server_command(args)
        Build(args) => run_build_command(args)
```

The mapping is direct:

```text
tool serve --host 0.0.0.0 -p 9000 --reload
```

parses to:

```incan
ToolCommand.Serve(ServeArgs(
    host="0.0.0.0",
    port=9000,
    reload=true,
))
```

The declaration controls the mapping:

| Declaration | CLI behavior |
| ----------- | ------------ |
| `ToolCommand.Serve(ServeArgs)` | Adds a `serve` subcommand whose arguments are described by `ServeArgs`. |
| `ToolCommand.Build(BuildArgs)` | Adds a `build` subcommand whose arguments are described by `BuildArgs`. |
| `host: str = "127.0.0.1"` | Adds optional `--host HOST`; omitted input uses the typed default. |
| `port [short="p", value_name="PORT"]: int = 8000` | Adds `--port PORT` and `-p PORT`; the string token is parsed as `int`. |
| `reload: bool = false` | Adds a `--reload` flag that sets `reload` to `true` when present. |
| `output: Path | None = None` | Adds optional `--output PATH`; omitted input produces `None`. |

Command descriptions do not require new enum syntax. They can live in a typed sidecar spec:

```incan
const TOOL_CLI = cli.spec[ToolCommand](
    name="tool",
    commands={
        ToolCommand.Serve: cli.command(description="Run the development server"),
        ToolCommand.Build: cli.command(description="Build project artifacts"),
    },
)
```

The sidecar spec augments the parseable enum/model shape; it does not replace it. If the sidecar omits a command, the command still exists and uses the derived spelling without extra description text. If the sidecar names a command that is not a variant of `ToolCommand`, the spec is invalid.

Manual `match` dispatch is the transparent low-level form. It makes the parse boundary explicit and keeps handlers testable as ordinary functions:

```incan
def main(argv: list[str]) -> int:
    command = TOOL_CLI.parse(argv)?

    match command:
        Serve(args) => run_server_command(args)
        Build(args) => run_build_command(args)
```

An optional binding helper can reduce dispatch boilerplate without becoming a second parser contract:

```incan
app = cli.app(TOOL_CLI)
    .handler(ToolCommand.Serve, run_server_command)
    .handler(ToolCommand.Build, run_build_command)

def run_build_command(args: BuildArgs) -> int:
    return build_project(release=args.release, output=args.output)

def main(argv: list[str]) -> int:
    return app.run(argv)
```

This RFC treats that binding form as a dispatch facade, not as a second source of truth. The command enum, argument models, and sidecar spec remain the parseable contract.

A dedicated CLI DSL is also a viable design direction if the sidecar syntax proves too indirect. The DSL would still need to lower to the same `CliSpec`:

```incan
cli ToolCli for ToolCommand:
    name = "tool"

    command Serve:
        description = "Run the development server"

    command Build:
        description = "Build project artifacts"
```

This is more local and readable than a const sidecar, but it is new syntax or vocab surface. This RFC keeps it as an open design option rather than silently choosing syntax in a stdlib RFC.

Reusable field contracts from RFC 087 compose with CLI argument models. A reusable field can carry the ordinary type, default, and description, while a command-specific model can add CLI presentation metadata at the use site:

```incan
field port [description="TCP port to listen on"]: int = 8000

@derive(CliArgs)
model ServeArgs:
    host [description="Address to bind"]: str = "127.0.0.1"
    field port [short="p", value_name="PORT"]
    reload [description="Restart when source files change"]: bool = false
```

The imported `port` field supplies the canonical field name, type, default, and description. The local use adds `short` and `value_name` for this command without changing the underlying field contract.

Environment fallbacks use the same string-boundary parsing rules as command-line arguments:

```incan
@derive(CliArgs)
model DeployArgs:
    token [description="API token for deployment", env="API_TOKEN", secret=true]: SecretStr
    region [description="Deployment region", env="APP_REGION"]: str = "eu-west-1"
```

If `--token` is omitted, parsing may consult `API_TOKEN`. If the token is still absent or fails validation, the parser returns a `CliError` rather than letting the handler discover the problem later. Handlers do not have to know whether a value came from an option, a positional argument, a default, or an environment fallback unless the program explicitly asks for provenance metadata. The ordinary result is just a typed command value.

### CLI eligibility is a derived trait, not an ambient marker

`parse[T]` and `spec_for[T]` only accept a type that has explicitly derived `CliArgs` (for a single-command model) or `CliCommand` (for a multi-command enum):

```incan
trait CliArgs:
    ...

trait CliCommand:
    ...

@derive(CliArgs)
model ServeArgs:
    ...

@derive(CliCommand)
enum ToolCommand:
    ...
```

`@derive(CliArgs)`/`@derive(CliCommand)` are not new compiler-internal derives in the shape of `Debug`/`Eq`/`Clone` (which are Rust proc macros with no Incan-expressible body). They are ordinary RFC 024 `__derives__` modules: `CliArgs`/`CliCommand` trait bodies authored in plain Incan, using RFC 021's `Model.__fields__() -> List[FieldInfo]` reflection to inspect a model's fields or an enum's variants at derive time — the same pattern RFC 024's own `SqlSchema` example already demonstrates for a different derivable trait. No new compiler-internal trait-generation logic is needed; `std.cli` is a library consumer of two mechanisms this project has already shipped, not a third compiler-magic derive path.

This does require `FieldInfo` to carry more than it does today. RFC 021 already reserves an `extra: dict[str, str]` slot on `FieldInfo` for exactly this kind of consumer-specific metadata, but nothing populates it yet. `std.cli`'s `__derives__` body reads its CLI-specific bracket keys (`option`, `short`, `positional`, `value_name`) out of `extra`, so populating that reserved slot for the compiler's own field reflection is part of this RFC's own scope — not an amendment to RFC 021 (which stays untouched and Implemented), but the first real use of an extension point RFC 021 already built for this. `FieldInfo` does not need to expose a field's actual default *value*: `parse[T]`'s generated command value only passes fields it actually resolved (from CLI arguments or an environment fallback) to the model constructor, and omits everything else, letting the model's own ordinary default application handle the rest — exactly like any other Incan constructor call. `has_default: bool`, already part of `FieldInfo`, is all `CliArgs`'s derive body needs to know whether omission is legal.

This split matters for the metadata keys too. `option`, `short`, `positional`, and `value_name` are meaningful only on a `CliArgs`/`CliCommand`-derived type and are diagnosed if present on a field of a type that hasn't derived one; `env` and `secret` are general-purpose model field metadata usable on any model regardless of CLI derivation (see "Relationship to field metadata and reusable fields").

## Reference-level explanation

### Module surface

`std.cli` must provide a core parsing and rendering surface. Every function taking a command type `T` is declared twice, bound on `CliArgs` and on `CliCommand` respectively, since Incan's generic bounds are intersection-only and there is no union-bound syntax to express "T implements either trait" in one declaration. The concrete `T` at a call site (a `@derive(CliArgs)` model or a `@derive(CliCommand)` enum) resolves which declaration applies, the same multi-instantiation dispatch RFC 025 already establishes for this shape of problem:

```incan
def parse[T with CliArgs](argv: list[str]) -> Result[T, CliError]
def parse[T with CliCommand](argv: list[str]) -> Result[T, CliError]

def parse_from[T with CliArgs](argv: list[str], config: CliParseConfig = CliParseConfig()) -> Result[T, CliError]
def parse_from[T with CliCommand](argv: list[str], config: CliParseConfig = CliParseConfig()) -> Result[T, CliError]

def spec_for[T with CliArgs]() -> Result[CliSpec[T], CliSpecError]
def spec_for[T with CliCommand]() -> Result[CliSpec[T], CliSpecError]

def render_help[T with CliArgs](program_name: str | None = None) -> Result[str, CliSpecError]
def render_help[T with CliCommand](program_name: str | None = None) -> Result[str, CliSpecError]

def render_completions[T with CliArgs](shell: Shell) -> Result[str, CliSpecError]
def render_completions[T with CliCommand](shell: Shell) -> Result[str, CliSpecError]

def render_error(error: CliError, style: CliRenderStyle = CliRenderStyle.default()) -> str
def exit_code_for(result: CliOutcome) -> int

def run_cli[T with CliArgs](argv: list[str], handler: Callable[[T], Result[int, E] | int]) -> int
def run_cli[T with CliCommand](argv: list[str], handler: Callable[[T], Result[int, E] | int]) -> int
```

`Shell` is a plain enum (`Bash`, `Zsh`, `Fish`, `PowerShell`). `render_completions[T]` draws entirely from the same `CliSpec` every other renderer already uses — no second spec, no completion-specific derive.

The exact helper names may change before this RFC moves to Planned, but the committed surface must include typed parse, spec derivation, help rendering, completion rendering, error rendering, and exit-code mapping.

### CLI spec derivation

`spec_for[T]` must derive a CLI spec from a supported command type `T`. `T` must have derived `CliArgs` or `CliCommand`; a type that has not is rejected at the `spec_for[T]`/`parse[T]` call site as a compile-time bound-not-satisfied error, not a runtime diagnostic.

A model type that has derived `CliArgs` may describe a single-command CLI. Its fields become options, flags, or positional arguments according to the rules in this RFC.

An enum type that has derived `CliCommand` may describe a multi-command CLI. Each variant becomes a subcommand. A variant with a payload model uses that model as the subcommand argument shape; that payload model must itself have derived `CliArgs`. A payload-free variant is a subcommand with no additional arguments. A variant with unsupported payload shape, or a payload model that has not derived `CliArgs`, must be rejected at spec derivation time.

Field metadata may customize CLI presentation and input sources, but type information remains authoritative. Metadata must not cause a field typed as `int` to parse as `str`, a required field to silently become optional, or a non-collection field to accept repeated values unless a future RFC defines that behavior.

### Relationship to field metadata and reusable fields

`std.cli` must reuse `description` from RFC 021 as the default help text for model fields. A separate `help` key is not part of this RFC's committed surface.

This RFC must not add bracket metadata to enum variants. Current enum syntax already supports payload-carrying variants such as `Serve(ServeArgs)`, but this RFC does not extend variant declarations with `[description="..."]` or similar syntax. Command descriptions may be supplied by a typed sidecar spec, a dedicated CLI DSL if this RFC accepts one, or by a later RFC that defines enum-variant metadata generally.

`std.cli` must not reuse RFC 021 `alias` as the spelling for command-line option aliases. Field `alias` is a wire/schema name and participates in model construction, field access, destructuring, and descriptor metadata. Command-line spellings live in the CLI parser namespace and must not change ordinary model member resolution.

RFC 087 reusable field contracts may appear in CLI argument models. When a model imports a reusable field, the imported field's canonical name, type, default, description, and validation contract are available to the CLI spec. A local model use may add CLI presentation metadata such as `short`, `option`, `positional`, or `value_name` without changing the reusable field contract itself.

If a reusable field and a local model use both provide the same CLI-specific metadata key, the local model use must win. This allows one domain field such as `port` to be reused across several commands while each command chooses its own short option or positional behavior.

The metadata set splits into two groups, matching whether it needs `CliArgs`/`CliCommand` to mean anything at all:

CLI-specific, meaningful only on a field of a `CliArgs`/`CliCommand`-derived type and diagnosed otherwise:

- `option`: override the long option spelling without leading dashes;
- `short`: provide one short option character without a leading dash;
- `positional`: mark a field as positional;
- `value_name`: display name for usage and help.

General-purpose model field metadata, usable on any field regardless of CLI derivation, in the same way `description` already is:

- `env`: name an environment fallback, useful beyond CLI parsing for any typed configuration value (for example an RFC 033 `ctx` model) that wants "fall back to this environment variable" independent of ever being parsed as CLI args;
- `secret`: suppress value display in diagnostics and rendered defaults, useful for any sensitive field (a config model's password, a stored token) regardless of whether the containing type is ever used as CLI args.

`std.cli` reuses `env` and `secret` for its own environment-fallback and diagnostic-redaction behavior rather than owning them, the same relationship it already has with `description`. A field using an `option`, `short`, `positional`, or `value_name` key on a type that has not derived `CliArgs` or `CliCommand` must be diagnosed: these four keys are only meaningful on a derived type, and the diagnostic should suggest adding the missing `@derive`.

Aliases, deprecations, hidden commands, grouping, shell completion annotations, prompts, and rich terminal formatting are outside this RFC unless Draft discussion proves they are required for the core contract.

If a type cannot be represented as a CLI spec, `spec_for[T]` and `parse[T]` must fail with a diagnostic that names the unsupported type or field and explains the required shape.

### Names and command spelling

By default, enum variant names and model field names should be converted to kebab-case for command-line spelling. For example, `ServeStatic` becomes `serve-static`, and `output_dir` becomes `--output-dir`.

Metadata may override command-line spellings:

- `option` may set a long option spelling such as `config` for `--config`.
- `short` may set a one-character short option spelling such as `c` for `-c`.
- `value_name` may set a display name for a positional or option value in usage text.

The derived spec must reject duplicate canonical names, duplicate long option spellings, and ambiguous short options within the same command scope.

Command and option matching should be case-sensitive.

### Field classification

A model field must be classified as one of:

- a named option;
- a boolean flag;
- a positional argument;
- a repeated positional argument;
- a skipped field supplied only by default or programmatic construction.

By default, scalar fields become named options. A `bool` field with a default of `false` becomes a flag that is set to `true` when present. A field marked with `positional=true` becomes a positional argument. A collection field marked as positional becomes a repeated positional argument.

The spec must reject a required field that has no command-line source, no default, and no environment fallback.

The spec must reject positional fields after a repeated positional field unless a future RFC defines disambiguation.

### Requiredness, defaults, and optionality

A field without a default and without `Option[...]` type is required unless metadata provides an environment fallback or another explicit source.

A field with a default is optional from the command line. If the user omits it and no higher-priority source provides a value, the default is used.

A field of type `Option[T]` may be omitted and produces `None` unless a value is supplied by the command line or an environment fallback.

A collection field may use an empty collection as its default when omitted. A required non-empty collection must be explicitly expressible in metadata or rejected until non-empty collection constraints exist.

Defaults must be ordinary typed Incan values. CLI metadata must not store string defaults that bypass type checking.

### String-boundary parsing and validation

Command-line arguments and environment variables are string boundaries. `std.cli` must parse those strings into target field types at the boundary; it must not introduce general-purpose implicit coercion inside ordinary Incan expressions.

For each field, parsing must use the target field type:

- `str` accepts the token unchanged;
- `int` accepts the committed integer literal grammar for base-10 runtime text and must reject malformed values;
- sized integer types must also reject overflow and underflow for the target width;
- `float` accepts the committed floating-point runtime text grammar and must reject malformed values;
- `Path` accepts a lexical path value without requiring that the path exists unless a separate validation contract says otherwise;
- `bool` is normally handled by flag presence rather than by consuming a string value;
- enum values parse from the accepted command-line spellings for the enum variants;
- validated newtypes parse the underlying type and then run the checked construction path;
- `Option[T]` treats absence as `None`, while a present malformed value is an error;
- collection fields parse each occurrence as the element type and preserve input order.

The same string-boundary parser should be shared by command-line input and `std.environ` typed reads where the source type is environment text. This keeps `"12" -> 12` behavior straightforward: it is a boundary parse from external text to `int`, not a language-wide string-to-int coercion.

Enum fields must parse from the accepted variant spelling set. Invalid enum values must report the accepted values.

Validated newtypes must be constructed through their checked construction path. If construction fails, the CLI error must identify the argument and surface the validation message without discarding the argument context.

If a target type has no CLI parse path, spec derivation should fail before runtime parsing when possible.

### Boolean flags

A `bool` field with default `false` must accept the positive flag spelling, such as `--reload`, and set the field to `true`.

A `bool` field with default `true` must either require explicit metadata for negation or be rejected until the design settles the spelling. This RFC does not want silent or inconsistent `--no-*` behavior.

Boolean flags must not require a separate value by default. A future RFC may define explicit `--flag=true` compatibility if needed.

### Repeated values

A field whose type is a collection may accept repeated occurrences when it is a named option, such as `--include a --include b`.

A repeated positional field consumes the remaining positional arguments for its command.

Repeated values must preserve argument order.

If the collection element type cannot be parsed, the collection field must be rejected as a CLI field.

### Environment fallback

Metadata may declare an environment fallback with `env`.

Command-line values must take precedence over environment fallbacks. Environment fallbacks must take precedence over field defaults.

Environment fallback reads must use current-process environment semantics compatible with `std.environ`. Missing values, malformed values, invalid Unicode, and validation failures must be distinguishable where the environment module can distinguish them.

CLI diagnostics must not print secret environment values by default.

### Help and usage rendering

Help and usage text must be generated from the same spec used by parsing.

Help output must include command names, option spellings, value names, requiredness where appropriate, defaults where safe to display, environment fallback names where safe to display, field descriptions, and command descriptions when supplied by a binding facade.

Help output must not display secret default values or secret environment values. Metadata should allow a field to mark its value as secret.

Hidden fields and hidden commands are deferred until a follow-up extension defines compatibility rules for undocumented accepted input.

Help rendering must be deterministic for a given spec.

The parser must reserve `--help` for help by default. A command may not define its own conflicting `--help` option unless an explicit parser configuration disables automatic help.

### Shell completion

`render_completions[T](shell: Shell)` must generate a completion script from the same spec used by parsing and help, for each of `Shell.Bash`, `Shell.Zsh`, `Shell.Fish`, and `Shell.PowerShell`.

Completions must cover: command and subcommand names, long and short option spellings, and enum-typed value choices. These are static facts already present in the derived `CliSpec`; generation must not require any metadata beyond what `spec_for[T]` already produces.

Completion generation must not invoke dynamic or context-aware logic (filesystem lookups, remote calls, running the program itself) to produce candidates. A field whose value cannot be statically enumerated from the spec (for example a `Path` or a plain `str`) completes with no candidates rather than a runtime-dependent guess; per-field custom completion callbacks are out of scope for this RFC (see Non-Goals).

Completion output must be deterministic for a given spec, matching help rendering's own determinism requirement.

Secret fields must not appear as completable values; a secret option's own name may still complete normally.

### Errors and diagnostics

`CliError` must distinguish at least:

- unknown command;
- unknown option;
- ambiguous option spelling;
- missing required argument;
- missing required option;
- invalid value;
- invalid enum choice;
- too many positional arguments;
- repeated option conflict where repetition is not allowed;
- help requested;
- invalid CLI spec.

Errors must include enough structured context for alternate renderers: argument spelling, command path, expected type or accepted values where relevant, source kind, and span-like argv index information where possible.

The default renderer should produce concise command-line diagnostics suitable for stderr. It should include a usage hint for parse errors and avoid dumping full help unless configured.

### Exit behavior

The framework must define standard exit code categories:

- successful handler completion maps to `0` unless the handler returns another explicit code;
- help requested maps to `0`;
- CLI usage errors map to a non-zero usage code;
- application errors map to a non-zero application code unless the handler returns a more specific code.

This RFC should prefer conventional CLI behavior but must document exact numeric values before moving to Planned.

`run_cli` should catch parse and help outcomes, render the appropriate output, and return the standard exit code. Programs that need custom rendering may call `parse` and rendering helpers directly.

### Testing model

CLI parsing must be testable without spawning a process.

Programs should be able to test `parse[T](argv)` directly with explicit argument vectors.

Programs should be able to test handlers using typed command values directly, bypassing parsing.

Rendering helpers should produce deterministic strings so snapshot-style CLI tests remain stable.

## Design details

### Why model-first instead of parser-builder-first

A parser builder makes every CLI a second schema. Incan already has the ingredients for a stronger declaration: models describe named fields, enums describe alternatives, defaults describe omission behavior, and metadata describes user-facing labels. A model-first CLI framework lets those declarations become executable without duplicating them in a parser DSL.

### Why enums own subcommands

Subcommands are sum types. A program invocation chooses one path from a finite set, and each path has its own argument model. Encoding that as an enum is direct, statically inspectable, and testable with ordinary pattern matching.

### Why a sidecar spec owns command presentation

Command descriptions are useful, but adding enum-variant metadata just for CLI help would be the wrong owner. The command enum should stay a pure command shape, and a typed sidecar spec should own presentation details that are specific to the CLI surface.

This also keeps constant spec data inspectable. A tool can read the command enum and the `TOOL_CLI` sidecar without inspecting function decorators or handler bodies.

### Why one underlying spec matters

Dispatch helpers can be ergonomic for small programs, but they should not create a second parser system. If users bind handlers through a helper, that surface must consume the same `CliSpec` contract used by model-first declarations. Otherwise help rendering, completion metadata, diagnostics, defaults, and test behavior will drift.

### Why dispatch binding is a facade

Binding helpers are useful for attaching handlers to command variants. They are not the primary spec because the CLI contract should remain inspectable as typed data even when the program chooses a different dispatch style. In the helper form, `.handler(ToolCommand.Serve, run_server_command)` attaches a handler to an existing enum variant; it does not define a new command outside the enum.

This keeps the less-is-more contract: one command enum, one set of argument models, one derived spec, and optional dispatch ergonomics on top.

### Prior art and research

Python [`argparse`](https://docs.python.org/3/library/argparse.html) establishes the baseline standard-library pattern: a program defines argument specifications, then the parser derives `sys.argv` parsing, help, and errors. The Python docs say `argparse` “automatically generates help and usage messages” and reports invalid input.

Go's standard [`flag`](https://pkg.go.dev/flag) package is a useful lower-bound design: it says to “Define flags using flag.String, Bool, Int” and then call `flag.Parse()`. The lesson for Incan is that typed defaults and parse functions are enough for small tools, but subcommands, help structure, and reusable typed schemas need more than a global flag set.

[`Click`](https://click.palletsprojects.com/en/stable/parameters/)'s distinction between options and arguments is the strongest pressure toward keeping positional arguments narrow. Its current docs say options are “recommended to use for everything except subcommands, urls, or files.” That supports this RFC's default that scalar fields become named options and positionals require an explicit marker.

[`Typer`](https://typer.tiangolo.com/tutorial/parameter-types/) is direct evidence for type-shaped CLI authoring: its docs say a typed CLI parameter will “convert the data received in the command line” to that type. Incan should adopt the typed-boundary idea, but keep the parsing contract explicit so this is not confused with general implicit coercion.

Rust [`clap`](https://docs.rs/clap/latest/clap/_derive/_tutorial/) derive is the closest prior art for model-first static derivation. Its derive tutorial says an “appropriate default parser/validator” is selected from the field type. Incan should borrow that type-driven derivation and enum-subcommand shape, while avoiding Rust macro attributes as the user-facing model.

GNU and POSIX-style conventions matter for defaults. The [GNU standards](https://www.gnu.org/prep/standards/html_node/Command_002dLine-Interfaces.html) recommend long options corresponding to short ones and say programs should support `--help`; the [CLI Guidelines](https://clig.dev/) project similarly advises using a parsing library and returning zero on success, non-zero on failure. This RFC should stay compatible with those conventions unless a command explicitly opts out.

### Boundary with `std.process`

`std.cli` parses the current program's argument vector. It does not spawn other programs. When a CLI handler needs to invoke a child command, it should use `std.process.Command` or `Pipeline` from RFC 063.

This separation is important because process execution has different safety rules. `std.process` is argument-vector-first and owns shell-mode execution, pipelines, timeouts, and child lifecycle. `std.cli` should not smuggle shell strings or subprocess semantics into argument parsing.

### Boundary with `std.environ`

`std.cli` may read environment variables only as declared fallbacks for arguments. Direct environment access remains owned by `std.environ`.

When a CLI argument has `env`, parsing should use the same missing, malformed, parse, validation, and secrecy principles as `std.environ`. That gives users one environment story instead of two subtly different ones.

### Boundary with project lifecycle tooling

This RFC is for applications written in Incan. It does not change `oven env run`, matrix expansion, project scripts, or Oven lifecycle commands. Those remain owned by RFCs 019, 073, 117, and 118.

The long-term compiler CLI may eventually dogfood `std.cli`, but this RFC deliberately does not make compiler CLI migration part of the contract. A forced migration would make the RFC larger and tie framework design to current compiler implementation details.

### Compatibility and migration

This RFC is additive. Existing Incan programs, stdlib modules, project manifests, and compiler CLI behavior continue to work.

Users with hand-rolled parsing can migrate command by command by introducing typed models, calling `parse[T](argv)`, and moving parser validation into field types and metadata.

Libraries should avoid exposing `std.cli` types in non-CLI APIs unless their purpose is specifically CLI integration.

## Alternatives considered

### Keep CLIs in Rust, TypeScript, Python, or shell

Rejected because it leaves Incan without a practical tooling authoring story. Users should be able to write small professional tools in Incan itself.

### Provide only a thin `argv` helper

Rejected because it solves token access but not CLI quality. Users would still hand-roll help, aliases, validation, defaults, repeated values, errors, and exit behavior in every program.

### Parser-builder-first API

Rejected as the primary surface because it duplicates information already present in typed Incan declarations. A builder may still be useful as a lower-level escape hatch, but it should not be the user-facing center of gravity.

### CLI eligibility as a plain decorator marker

Rejected. An earlier shape used a bare `@cli.args`/`@cli.command` decorator purely as a checked marker with no generated behavior — functionally adequate for gating `parse[T]`, but an inert annotation that carries no substance and forecloses ever attaching real methods to a CLI-shaped type without a later redesign. `@derive(CliArgs)`/`@derive(CliCommand)` generating a real trait implementation, using the same derive convention `@derive(Debug, Eq, Clone)` already establishes, costs nothing extra today and leaves room to attach real trait methods later without revisiting the eligibility mechanism.

### Ambient eligibility via import

Rejected. Making any model automatically CLI-eligible merely by importing `std.cli` somewhere in the file would be ambient, implicit behavior with no local signal at the declaration site — exactly the kind of inference this RFC's model-first philosophy and the rest of this project's design principles avoid. Eligibility must be visible at the type's own declaration.

### A dedicated `cli.args` declaration form

Open, deferred past this RFC. A form such as `cli.args ServeArgs:` (declaring the model with CLI-specific vocabulary directly, rather than an ordinary `model` plus `@derive`) would read more locally but is new syntax or vocab surface that must justify itself beyond a nicer example, the same bar this RFC already applies to the dedicated CLI DSL alternative below. `@derive(CliArgs)` is the answer for this RFC's committed contract; a future RFC may reconsider dedicated declaration syntax once real usage shows the derive form is too indirect.

### Attach command descriptions via RFC 032 value-enum assignment instead of a sidecar

Considered and rejected for this RFC, worth revisiting when a general enum-variant metadata RFC exists. The proposed shape was `Serve(ServeArgs) = cli.command(description="...")`, reusing RFC 032's `enum Name(str|int): Variant = literal` value-assignment syntax so a command's description could live directly on its variant instead of in a separate `cli.spec[...]` sidecar. This does not work today on two independent grounds: RFC 032 makes payload-carrying variants and value-assignment mutually exclusive (an explicit Non-Goal, enforced by the parser), and even without that conflict, RFC 032 only accepts a `str`/`int` literal as the assigned value, not a function call — Incan has no `const fn` concept (a function provably pure/deterministic enough to fold into a compile-time constant), so `cli.command(...)` cannot be treated as a literal today regardless of the payload restriction. Separately from both syntactic blockers, RFC 032's value slot is conceptually the enum's wire/backing representation (its `str`/`int` identity for serialization, comparison, FFI), not a general per-variant metadata slot — even a hypothetical relaxed literal rule would be reusing the wrong feature for this. The right future vehicle is the general enum-variant metadata RFC this RFC's own "Relationship to field metadata and reusable fields" section already points at ("a later RFC that defines enum-variant metadata generally"), not an expansion of RFC 032's narrower, already-shipped scope. RFC 032 is `Implemented` and is not retroactively amended.

### Dedicated CLI DSL

Open. A DSL such as `cli ToolCli for ToolCommand:` would make command descriptions and presentation metadata more local than a const sidecar while still avoiding enum-variant metadata. The cost is a new syntax or vocab surface that must justify itself beyond one nicer example. If accepted, the DSL must lower to the same `CliSpec` as the typed sidecar and must not create a second parser model.

### Handler-binding-first API

Accepted only as a facade over typed specs. A binding helper may attach a handler to an existing command variant, but it must not create a separate parser contract outside the enum-plus-model spec and sidecar presentation spec.

### Require `incan` compiler CLI migration

Rejected. Dogfooding is valuable, but forcing compiler CLI migration into this RFC would couple the framework contract to current compiler implementation details.

## Drawbacks

- The framework adds a meaningful stdlib and tooling surface that must be documented, tested, and kept stable.
- Metadata-driven behavior can become opaque if the accepted metadata keys are too broad or poorly named, so this RFC keeps the metadata set intentionally small.
- Help rendering and diagnostic quality require polish; a technically correct parser with weak messages would not meet this RFC's goal.
- Deriving behavior from types creates pressure to define parse paths for more types than this RFC should commit to.
- Shell completion adds four separate script-generation targets (bash, zsh, fish, PowerShell) to implement and keep correct as shells evolve their own completion conventions; rich terminal output stays out of scope to bound this RFC's surface.

## Implementation architecture

*(Non-normative.)* A practical implementation should derive an intermediate `CliSpec` from type metadata and use that single spec for parsing, validation, help rendering, and error rendering. The parser should operate on explicit `list[str]` values so tests and embedding scenarios can bypass process globals. Runtime helpers can provide a thin boundary for obtaining the current process `argv` and returning exit codes, but the core parser should remain deterministic and side-effect-light.

## Layers affected

- **Stdlib / runtime (`incan_stdlib`)**: new `std.cli` module, parser entry points, error types, help rendering, shell completion rendering for bash/zsh/fish/PowerShell, and exit-code helpers.
- **Typechecker / symbol resolution**: CLI spec derivation must validate supported command enum and model shapes, metadata keys, option spelling collisions, and parse paths; must reject `parse[T]`/`spec_for[T]` calls where `T` has derived neither `CliArgs` nor `CliCommand` (ordinary RFC 025 multi-instantiation bound checking, no CLI-specific typechecker logic required beyond that); must diagnose CLI-specific metadata keys (`option`, `short`, `positional`, `value_name`) used on a field of a type that has derived neither trait. `@derive(CliArgs)`/`@derive(CliCommand)` themselves are RFC 024 `__derives__` modules with Incan-authored bodies, not new typechecker-level derive-generation code.
- **Metadata / descriptors**: RFC 021's `FieldInfo.extra: dict[str, str]` reserved slot must actually be populated with a field's bracket-metadata keys during reflection, so `std.cli`'s Incan-authored derive body can read `option`/`short`/`positional`/`value_name`/`env`/`secret` back out through `__fields__()`. This is the first real consumer of that reserved slot, not a new descriptor mechanism.
- **Emission / runtime handoff**: generated programs need a stable way to pass the current argument vector into Incan `main` or equivalent CLI entry points.
- **Formatter**: no new syntax is required, but examples and metadata-heavy model declarations should format predictably.
- **LSP / tooling**: completions and hovers should understand `std.cli` metadata keys, command specs, and parse errors where practical.
- **Docs / examples**: tutorials should show parse-test-handler structure and clarify the boundaries with `std.process`, `std.environ`, and project lifecycle commands.

## Design decisions

- **CLI eligibility is the `CliArgs`/`CliCommand` derived traits, not a bare marker or ambient inference:** `parse[T]`/`spec_for[T]`/`render_help[T]`/`run_cli[T]` each declare two bound overloads, `T with CliArgs` and `T with CliCommand`, dispatched per RFC 025's existing multi-instantiation mechanism (Incan's generic bounds are intersection-only; there is no union-bound syntax, so one declaration cannot accept "either trait"). This also gives the LSP an unambiguous, local signal at the type's own declaration site, without needing to trace `parse[T]` call sites across module or package boundaries to know a model is CLI-shaped. An ambient-by-import mechanism was considered and rejected (see "Alternatives considered").
- **`@derive(CliArgs)`/`@derive(CliCommand)` are RFC 024 `__derives__` modules, not new compiler-internal derive logic:** unlike `Debug`/`Eq`/`Clone` (Rust proc macros with no Incan-expressible body), `CliArgs`/`CliCommand` are ordinary trait bodies authored in plain Incan, using RFC 021's `Model.__fields__() -> List[FieldInfo]` reflection — the same pattern RFC 024's own `SqlSchema` example already demonstrates. `std.cli` is a library consumer of two already-shipped mechanisms, not a third compiler-magic derive path; this was a real correction caught during this pass, not the original framing. This requires actually populating RFC 021's reserved-but-empty `FieldInfo.extra: dict[str, str]` slot with a field's bracket-metadata keys during reflection — the first real consumer of an extension point RFC 021 already built, not an amendment to RFC 021 (which stays untouched and `Implemented`). `FieldInfo` does not need to expose a field's actual default value: `parse[T]`'s generated command value omits any field it didn't resolve from CLI/env and lets the model's own ordinary default application handle it, so `has_default: bool` (already in `FieldInfo`) is sufficient.
- **The metadata key set splits into CLI-specific and general-purpose groups, resolving the original naming question without renaming anything:** `option`, `short`, `positional`, and `value_name` remain CLI-specific — meaningful, and diagnosed as a probable missing-`@derive` mistake when absent, only on a field of a `CliArgs`/`CliCommand`-derived type. `env` and `secret` become general-purpose model field metadata, usable on any model regardless of CLI derivation, exactly as `description` already is — `std.cli` reuses them rather than owning them. This split was motivated by genuinely broader utility beyond CLI parsing: an RFC 033 `ctx` typed configuration model has the same "fall back to this environment variable" and "don't display this value" needs independent of ever being parsed as CLI args. No key names changed; the six original names all stay.
- **The const sidecar (`cli.spec[ToolCommand](...)`) is the only committed command-presentation mechanism:** a dedicated CLI DSL and a dedicated `cli.args`/reused-value-enum declaration form were all considered (see "Alternatives considered") and stay explicitly open/deferred, not spec'd by this RFC. Each would need new syntax or vocab surface to justify itself beyond a nicer-looking example; the sidecar already does the job without that cost. Command descriptions living directly on enum variants — via RFC 032's value-enum assignment or a future dedicated syntax — remains a real, worth-revisiting direction once a general enum-variant metadata RFC exists to own it properly, rather than reusing or loosening RFC 032's narrower, already-shipped wire-representation feature.
- **Positional fields stay strictly opt-in:** `positional=true` is the only way a field becomes positional; a required scalar field with no default never becomes positional by convention. Matches this RFC's existing default (scalar fields become named options) and the explicit-over-inference instinct applied throughout this pass — two structurally identical fields could legitimately need different treatment, which convention-based inference can't express.
- **Exit codes: `0` success, `2` usage error, `1` application error**, matching the widely-recognized Unix convention (bash, Python's `argparse`, GNU tools) this RFC's own Prior Art section already gestures at.
- **A `negate` metadata key, not automatic `--no-*` generation, for default-`true` bool fields:** a `bool` field defaulting to `true` needs an explicit `negate=true` to generate its `--no-<name>` spelling; there is no implicit negation and no rejection of default-`true` bool fields outright. This is a fifth CLI-specific metadata key (joining `option`/`short`/`positional`/`value_name`), chosen over forcing users to invert a field's polarity (e.g. renaming `color: bool = true` to `no_color: bool = false`) just to satisfy the parser.
- **The guaranteed CLI parse-path type list is already fully specified** by the "String-boundary parsing and validation" section: `str`, `int` (plus sized integers with overflow/underflow rejection), `float`, `Path`, `bool`, enum values, validated newtypes (covering types like `SecretStr`), `Option[T]`, and collection fields, with the rules composing for nested cases rather than needing per-combination entries. This closes the question; no new type support was added.
- **No `CliSpec` builder API, now or later:** manual `CliSpec` construction without deriving it from a model is not a compiler-provided fluent/chained builder (`.builder().option(...).build()`) — that shape is borrowed Rust/Java idiom, not native Incan. If manual construction is ever needed, `CliSpec` is an ordinary constructible value using normal Incan value-construction syntax, the same as any other model. This is not "defer the builder until a need appears" — there is no builder pattern to defer.
- **Long aliases beyond `short` stay deferred**, per this RFC's own existing "outside this RFC unless Draft discussion proves required" language for aliases. No guide-level example has needed a second long spelling for one field; nothing demonstrated the need.
- **Shell completion is designed and committed now, not deferred to a follow-up RFC.** Initially framed as out of scope purely for contract-size reasons — corrected mid-pass as a "north star first, don't pre-assign versions" mistake, the same principle already applied to RFC 117/118/119: settle the design now, decide implementation phasing (which shells ship first, which release) separately. `render_completions[T](shell: Shell)` generates static completions (command/subcommand names, option spellings, enum-typed value choices) entirely from the existing `CliSpec`, for `Bash`, `Zsh`, `Fish`, and `PowerShell`. Dynamic/context-aware completion callbacks for open-ended values (filesystem paths, remote lookups) remain explicitly out of scope — a genuinely different, runtime-dependent feature, not a scope cut for size alone.
