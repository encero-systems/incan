# RFC 033: `ctx` — typed configuration context

- **Status:** Draft
- **Created:** 2026-03-06
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 017 (validated newtypes and from_underlying)
    - RFC 018 (language primitives for testing)
    - RFC 019 (test runner and fixtures)
    - RFC 032 (value enums)
    - RFC 086 (structured reflection)
    - RFC 089 (read-only process environment access)
    - RFC 090 (typed CLI and shared field metadata)
    - RFC 094 (context managers and scoped resource lifetimes)
    - RFC 103 (secret values)
    - RFC 104 (ambient runtime capabilities and receipts)
    - RFC 130 (implicit None return annotations)
    - RFC 131 (typed error declarations and diagnostics)
    - RFC 132 (frozen classes)
    - RFC 133 (Literal types and compile-time value sets)
    - RFC 135 (lazy Option defaults)
    - RFC 141 (type matching and named type operations)
    - RFC 143 (clone with field overrides; excludes ctx values)
    - RFC 163 (compile-time conditions; distinct from runtime ctx contributions)
- **Issue:** [#167](https://github.com/encero-systems/incan/issues/167)
- **RFC PR:** —
- **Written against:** v0.2
- **Shipped in:** —

## Summary

Introduce `ctx` as a core language keyword for typed, environment-aware configuration. A declaration describes typed fields, deferred model construction, and layered resolution from defaults, ordered recursive contributions from nestable `if` blocks, explicitly declared YAML sources, and environment variables. First use resolves the whole context at runtime and publishes a completed frozen value for subsequent reads. An explicit fallible `preflight()` performs the same resolution early for callers that want to handle failures; an unchecked first-use failure reports the typed diagnostic and terminates execution unsuccessfully. Resolved context values are non-cloneable; separate declarations provide separate retained sessions while reusing ordinary model contracts. Tests use ordinary sources and fixtures with runner-provided isolation. Resolution retains explainable, secret-safe provenance rather than reducing configuration failures to panic messages.

## Motivation

Every non-trivial application needs configuration: database paths, batch sizes, API endpoints, feature flags. Today's patterns are painful:

- **Python dataclass + env vars**: manual `os.environ.get("KEY", "default")` for every field. No compile-time validation. String-typed. Error-prone.
- **Settings libraries**: typed runtime loading is useful, but environment-dependent composition and its diagnostics still require application or library conventions.
- **YAML + merge**: `common.yaml` + `prod.yaml` merged at load time. Untyped, no IDE support, merge surprises.
- **Django settings**: Python module importable globally. Untyped, single-environment, no structured overrides.
- **Dynamic configuration contexts**: nested access and recursive composition are useful, but dynamically discovered keys cannot provide the same checked field references as declared Incan types.

The common pain points:

1. **No compile-time safety** — config field references are strings, typos are runtime errors
2. **No environment awareness** — dev vs prod logic scattered across conditionals and YAML overrides
3. **Parameter threading** — config objects passed through 5+ layers of function calls
4. **No single source of truth** — defaults in code, overrides in YAML, env vars in shell, CLI flags in scripts

`ctx` combines typed access with a declared resolution process and retained evidence of how configuration was selected.

Static checking validates authored field references, defaults, and overrides. It cannot prove the contents or availability of environment variables; those are checked during fallible resolution.

### Who benefits

- **Application developers** — typed config with IDE completion, no boilerplate
- **Data engineers** — coordinated typed pipeline configuration across environments
- **DevOps / platform teams** — env vars override defaults predictably; no code changes for per-environment tuning

## Goals

- Retain concise typed configuration declarations and convenient published-name reads.
- Compose common defaults and environment-dependent differences recursively across the configuration tree.
- Select coordinated settings once through ordinary Boolean conditions and nested `if` blocks.
- Assemble declared model fields lazily, without explicit constructor calls in the context.
- Resolve layered inputs through checked types and recoverable configuration errors.
- Compose explicit YAML sources, including common and environment-specific files selected from resolved fields, through the same checked configuration shape.
- Resolve contexts on first runtime use and provide an explicit fallible preflight for early checks.
- Support independently resolved session declarations with separate environment namespaces and reusable external model contracts.
- Keep resolved context values frozen and non-cloneable, independent of their fields' individual capabilities.
- Test ordinary configuration access through existing fixtures with isolated resolution state per test.
- Preserve explicit optional clearing separately from missing overrides.
- Explain source selection using structured, secret-safe provenance and governed source access.

## Non-Goals

- Runtime configuration reload or implicit replacement of a published application value.
- Cloning a resolved context, including zero-argument copying and clone-with-overrides reconstruction.
- Explicit whole-collection replacement, clearing, or entry-removal markers during recursive composition.
- Implicit deep freezing, deep cloning, or thread-safety guarantees from fixed field bindings alone.
- A configuration-only type system, Literal validator, secret wrapper, or capability authority.
- Dotenv files, automatic file discovery, or CLI override syntax in this proposal.
- Implicit transport of configuration between an orchestrator and worker processes.
- Defining managed resource entry, exit, ownership transfer, cleanup, or shutdown; that design is deferred to RFC 094.

Multiple selectors, compound Boolean conditions, nested `if` blocks, and dotted field contributions are included. Selected blocks compose in source order; recursive merging preserves unspecified fields and dictionary entries and appends list contributions. Environment-specific entries belong in their corresponding conditional blocks or selected YAML files. Explicit collection replacement and removal are out of scope; permitted expression effects and whole-collection environment decoding remain questions below.

## Guide-level explanation (how users think about it)

These are independent examples of proposed behavior, not claims that `ctx` or the related draft features are implemented. Repeated declaration names illustrate separate programs.

### Declaring a context

A `ctx` declaration defines typed configuration fields, common defaults, and conditional contributions:

```incan
ctx AppConfig(env_prefix="APP_"):
    """Application configuration with environment-dependent defaults."""
    env: Literal["dev", "prod"] = "dev"
    database_url: str = "sqlite:///dev.db"
    batch_size: int = 100
    debug: bool = true
    output_dir: str = "output"

    if env == "dev":
        batch_size = 10

    if env == "prod":
        database_url = "postgres://db.example.com/app"
        debug = false
        batch_size = 5000
```

`env` is an ordinary typed field, mapped to `APP_ENV`. There is no implicit enum selector or special interpretation of a type name. `if` uses a Boolean condition; `==` compares values and `=` contributes configuration values during resolution. The block does not mutate a published instance. No block needs to match when the defaults and external inputs already provide a complete configuration.

RFC 163 reserves `when` and compiler-owned `@when` for compile-time selection using compiler-provided target and feature facts. Runtime configuration selection uses `if` inside `ctx`: it reads ordinary typed fields during first-use resolution or preflight, contributes values to the declared configuration, and does not exclude compiled fields or acquire the compilation predicate namespace. The same binary can therefore resolve different settings in different runtime environments.

### Model-like access without eager construction

Data contracts are ordinary application-defined models outside the context:

```incan
model RetrySettings:
    attempts: int = 1
    delay_seconds: float = 0.0

model DeliverySettings:
    endpoint: str = "http://localhost:8080"
    retry: RetrySettings

ctx PipelineConfig(env_prefix="PIPE_"):
    env: Literal["dev", "prod"] = "dev"
    delivery: DeliverySettings

    if env == "prod":
        delivery.endpoint = "https://delivery.example.com"
        delivery.retry.attempts = 4
```

`delivery: DeliverySettings` needs no `DeliverySettings()` call. During whole-context resolution, ctx assembles pending inputs from the model's declared fields and defaults, recursively including `RetrySettings`, and applies contributions before constructing and validating the completed values. Reading `PipelineConfig.delivery.retry.attempts` yields 1 in development or 4 in production, unless an external override supplies another valid value.

This construction behavior belongs to ctx. Ordinary model fields outside ctx do not automatically construct nested values. Required scalar fields remain required: declaring `endpoint: str` without a default requires a selected contribution or external input to supply `delivery.endpoint`. No incomplete model is published while waiting for that input. Referencing an external type or reusable application value does not turn that declaration into a context field.

### Reading context fields

Context fields are accessed by type name — no variable, no parameter passing:

```incan
def configured_batch_size() -> int:
    return AppConfig.batch_size

def configured_database_url() -> str:
    return AppConfig.database_url
```

`AppConfig.batch_size` is a global read. The compiler knows the type (`int`) at compile time. IDE completion works. Rename-refactoring works.

The first declaration-name field read resolves and validates that whole context at runtime. On success it publishes the completed frozen value; subsequent reads use that value without reparsing environment variables. Ordinary field reads do not require `?`. Declaring or importing a context does not resolve it, and there is no automatic sweep of contexts before `main`.

```incan
ctx AppConfig(env_prefix="APP_"):
    timeout: int = 30

def main():
    println(AppConfig.timeout)
```

Here execution enters `main`, then the first read of `AppConfig.timeout` resolves `AppConfig`. If resolution fails, that unchecked read reports the full typed configuration diagnostic and terminates execution unsuccessfully. On success, it returns the selected integer. This example also uses RFC 130's proposed omission of the None return annotation. The fatal unchecked-read boundary is not Result propagation, a new exception mechanism, or a generated panic.

### Preflight before application work

The proposed `AppConfig.preflight()` operation returns `Result[None, ConfigError]`. `ConfigError` is the standard configuration umbrella described below. Preflight performs the same whole-context resolution as a first field read, retaining the successful value for subsequent reads rather than checking once and rereading sources later.

```incan
# AppConfig is declared in the preceding example.
# preflight() is the proposed ctx operation described above.
match AppConfig.preflight():
    Ok(_) => println(AppConfig.timeout)
    Err(failure) => println(failure.message())
```

This fragment explicitly handles a failed preflight; it does not perform a field read on the failure path. `message()` uses RFC 131's common error interface. Library authors can incorporate this operation into their own fallible initialization or health-check API to check required configuration before work starts. A handled preflight failure does not by itself terminate execution. Successful preflight prepares the same published value used by later field reads; it is not the API for obtaining an independent test or session value.

### Retry only when the application chooses recovery

A failed preflight leaves the context unresolved. A subsequent explicit preflight makes a fresh attempt using the declared sources; it does not permanently replay the first failure. Each call makes at most one resolution attempt, with no automatic retry loop. The application decides whether recovery is valid, how conditions can be corrected, and whether or when to try again.

For environment-only configuration, another attempt will normally encounter the same missing or invalid input. RFC 089's process environment API is read-only. Where a declared source or construction dependency can recover, another attempt can succeed. This rule does not introduce additional source providers or an environment-mutation API.

Failed attempts do not publish partial values. A fresh attempt may reevaluate defaults and validators and repeat their permitted effects; failure does not promise rollback. Previously returned errors retain the diagnostic facts of their own attempt. Once an attempt succeeds, the completed frozen value is retained permanently: later preflights and reads reuse it without resolving sources again.

### Coordinate concurrent first use

Concurrent callers using one unresolved published context share one in-progress resolution attempt. If one worker starts preflight while another reaches the same context, the second waits for that attempt's outcome instead of resolving a competing value. Success makes the same completed frozen context available to both callers and all subsequent reads. Callers joining a failed attempt observe that attempt's failure; waiting must not silently initiate another attempt. A later explicit preflight may retry under the application-controlled recovery rule.

Each caller retains its chosen failure boundary: preflight returns a Result, while an unchecked field access reports failure and terminates execution. Coordination applies while initialization is in progress. After successful publication, stored values are reused without repeated resolution or validation. This coordination does not introduce an async return type or implicit await for field access.

### Resolve dependencies and diagnose cycles

A context may use another context's values while resolving:

```incan
ctx BaseConfig(env_prefix="BASE_"):
    timeout: int = 30

ctx WorkerConfig(env_prefix="WORKER_"):
    timeout: int = BaseConfig.timeout
```

These are proposed ctx declarations. When WorkerConfig resolution needs that default, it resolves BaseConfig and uses its completed value. If BaseConfig instead needs WorkerConfig.timeout while WorkerConfig is still resolving, the attempt encounters a dependency cycle. Its typed failure must expose the dependency chain, for example `WorkerConfig.timeout → BaseConfig.timeout → WorkerConfig.timeout`, with available source locations. Cycle detection must also cover dependencies across concurrent attempts rather than allowing workers to wait indefinitely for one another.

A cycle encountered during preflight is returned as that resolution's failure. A cycle encountered during unchecked first use follows the fatal diagnostic rule. Neither path may publish partially resolved values. The failure uses `ConfigError.DependencyCycle` under the standard configuration umbrella.

### How env_prefix works

`ctx AppConfig(env_prefix="APP_")` maps each field to an environment variable using `{env_prefix}{UPPER_SNAKE_CASE_FIELD_NAME}`:

`env_prefix` is required. It declares an external configuration namespace independently of the context's source name, so renaming the declaration does not silently rename its environment variables. Per-field explicit `env` names retain their declared spelling.

| Field          | Env var            | Lookup order                  |
| -------------- | ------------------ | ----------------------------- |
| `database_url` | `APP_DATABASE_URL` | env → YAML → if → default   |
| `batch_size`   | `APP_BATCH_SIZE`   | env → YAML → if → default   |
| `debug`        | `APP_DEBUG`        | env → YAML → if → default   |

Selectors use the same field mapping as other settings. In the first example, `APP_ENV=prod` supplies the Literal value used by `if env == "prod"`; Literal string membership does not gain case-insensitive matching.

```bash
APP_ENV=prod APP_BATCH_SIZE=10000 incan run
```

This selects the production contributions and then gives the direct `APP_BATCH_SIZE` input precedence, producing batch size 10000.

### Resolution priority

```text
Field defaults                  ← lowest
    ↓
Matching if contributions     ← source order, recursively merged
    ↓
Declared YAML sources           ← listed order, recursively merged
    ↓
Direct environment inputs       ← highest
```

Selector inputs such as `APP_ENV` must be available when their conditions are evaluated. Giving direct field inputs highest priority must not mean that production selection observes a stale development default. Conditions must observe their inputs' resolved values. A reached circular dependency through defaults or conditions produces a typed configuration failure rather than iterative recalculation.

Within the conditional layer, later matching contributions win for the same scalar leaf. Declared YAML sources contribute above conditional settings and compose in their listed order. Nested fields and dictionary entries merge recursively; list contributions append in order. Direct environment inputs have higher priority. Whole-model environment inputs use JSON objects decoded into checked model-field contributions; whole-collection environment decoding and its source-composition details remain open below. Explicit descendant inputs override the corresponding parts of a whole-model input with an overlap warning.

An absent override must preserve the previously selected value. An explicitly supplied `None` for an optional field must replace it. Resolution must not flatten these two cases together, even though the final field has an ordinary `Option[T]` type. The external spelling for explicitly clearing an optional environment setting remains open.

### Explicit YAML sources and environment-specific files

The following introduces the proposed `std.config.yaml` adapter and ctx `sources` parameter. They are part of this proposal, not implemented APIs. Files are ordinary runtime paths relative to the application's working directory; declarations do not implicitly search for files or embed files from the compiler's source directory.

```text
config/
    common.yaml
    dev.yaml
    prod.yaml
```

`config/common.yaml`:

```yaml
storage:
  prefix: deliveries

gaps:
  by_contract:
    contract-a: 5
```

`config/dev.yaml`:

```yaml
storage:
  incoming_bucket: example-incoming-dev
  raw_bucket: example-raw-dev
```

`config/prod.yaml`:

```yaml
storage:
  incoming_bucket: example-incoming-prod
  raw_bucket: example-raw-prod

gaps:
  default_days: 3
  by_contract:
    contract-b: 2
```

The data contracts are application-defined models outside ctx:

```incan
from std.config import yaml

model StorageSettings:
    incoming_bucket: str
    raw_bucket: str
    prefix: str = "incoming"

model GapSettings:
    default_days: int = 0
    by_contract: dict[str, int] = {}

ctx PipelineConfig(
    env_prefix="PIPE_",
    sources=(
        yaml("config/common.yaml"),
        yaml(f"config/{env}.yaml"),
    ),
):
    env [env="ENV"]: Literal["dev", "prod"] = "dev"
    storage: StorageSettings
    gaps: GapSettings

    incoming_root [env=false]: str = (
        f"s3://{storage.incoming_bucket}/{storage.prefix}"
    )

    if env == "prod":
        gaps.default_days = 2
```

`ENV=dev` selects `common.yaml` followed by `dev.yaml`; `ENV=prod` selects `common.yaml` followed by `prod.yaml`. Source expressions may reference fields declared in the context, even though the expression appears in the declaration header. The filename selector resolves from `ENV` or its source-independent default before file selection. A selected source must not redefine the selector used to choose its own path; a source-selection dependency cycle receives a diagnostic rather than repeated file selection.

With `ENV=prod`, `PIPE_STORAGE__INCOMING_BUCKET=example-replay-input`, and `PIPE_GAPS__DEFAULT_DAYS=1`, the incoming root is `s3://example-replay-input/deliveries`. The raw bucket comes from `prod.yaml`. The tolerance mapping retains contract-a at 5 from the common file and adds contract-b at 2 from the production file. The default tolerance progresses from declaration default 0, to production condition 2, to production YAML 3, to environment input 1.

Each YAML source is a configuration fragment, not an eagerly constructed complete model. Missing fields retain lower-priority contributions; required fields are checked after assembly. Supplied paths must use the declared schema and types. An unknown key such as `gaps.default_dayz` receives a diagnostic with the file location and a suggested field name where available; malformed YAML and incompatible supplied values produce typed resolution failures. A list-valued fragment appends under the same recursive composition contract as conditional contributions.

A listed file is required by default. `yaml("config/local.yaml", required=false)` explicitly allows absence to contribute nothing; malformed or otherwise unreadable present files must still fail. Source loading occurs only during first-use resolution or preflight. A successful resolution retains its values and source evidence without rereading changed files on subsequent accesses. Missing files, failed reads, and failed decoding remain recoverable through preflight and use the ordinary fatal diagnostic boundary for unchecked first use.

Several contexts may explicitly bind their selector fields to the same external `ENV` name while retaining their own other environment prefixes. This shares an external input, not resolution state or resource instances. Applications needing a single retained selector may instead reference a separately declared context's resolved field.

Source files must be available to the process that resolves the context. YAML loading must not imply that a scheduler's files or resolved values appear automatically in a worker. CLI argument forwarding and orchestrator-specific adapters retain their own explicit transport contracts.

### Defaults that depend on other fields

```incan
ctx WorkerConfig(env_prefix="WORKER_"):
    env: Literal["dev", "prod"] = "dev"
    workers: int = 1
    batch_size: int = workers * 128

    if env == "prod":
        workers = 8
```

Defaults may reference other fields. They observe those fields' resolved values, including selected conditional contributions and direct environment inputs. Production selects workers 8, so the batch-size default is 1024. Adding `WORKER_WORKERS=10` makes it 1280. Supplying `WORKER_BATCH_SIZE=500` instead selects 500 without evaluating the replaced scalar default for that field.

This is dependency-based resolution, not a new category of calculated fields or a property recalculated on every read. Moving the `batch_size` declaration above `workers` must not change the result. Matching `if` contributions still compose in source order. A reached cycle must identify the participating fields and conditions in a typed diagnostic; resolution must not repeatedly recalculate until values appear to stabilize.

These rules apply during initial resolution. Context values cannot be cloned or reconstructed through clone overrides. Separate context declarations resolve their own dependent defaults. Recursive collection composition still needs any lower-priority fragments that contribute to the result; skipping a replaced scalar default must not discard a required collection fragment.

### Reuse declared value constraints

The following combines proposed `ctx` syntax with RFC 133's proposed Literal types:

```incan
type LogLevel = Literal["debug", "info", "warning", "error"]

ctx AppConfig(env_prefix="APP_"):
    log_level: LogLevel = "info"
    timeout: Option[int] = 30
```

A source default of `"verbose"` is rejected statically. `APP_LOG_LEVEL=verbose` fails during resolution through a typed configuration error retaining the validation failure. An absent `APP_TIMEOUT` leaves `Some(30)` selected. An explicit clear selects `None`; it is not treated as a missing override. Literal membership and conversion reuse RFC 133 rather than introducing a separate choices declaration for configuration.

### Plain values for Option fields

Ctx defaults and conditional contributions reuse ordinary expected-type assignment conversions. A known `Option[T]` destination accepts a compatible plain `T` value without an authored `Some(...)` wrapper:

```incan
ctx ClientConfig(env_prefix="CLIENT_"):
    mode: Literal["request", "subscription"] = "request"
    deadline_seconds: Option[float] = None

    if mode == "request":
        deadline_seconds = 30.0
```

Request mode selects the present deadline 30.0; subscription mode retains None. An explicit `Some(30.0)` remains valid, but is unnecessary here. This is reuse of ordinary Incan Option assignment semantics, not a ctx-specific conversion or an implicit default for an uninitialized field. Pattern matching still distinguishes `Some(value)` and `None`, and a present false, zero, or empty string must not become absence.

### Combine selectors and nest related conditions

This independent example defines its application models outside the context. Its nested models need no explicit constructor calls:

```incan
model RetrySettings:
    attempts: int = 1
    delay_seconds: float = 0.0

model WriterSettings:
    batch_size: int = 128
    retry: RetrySettings

ctx ImportConfig(env_prefix="IMPORT_"):
    env: Literal["dev", "prod"] = "dev"
    region: Literal["eu", "us"] = "eu"
    mode: Literal["incremental", "replay"] = "incremental"
    workers: int = 1
    writer: WriterSettings

    if env == "prod":
        workers = 8
        writer.retry.attempts = 4

        if region == "eu":
            workers = 12
            writer.retry.delay_seconds = 0.5

        if mode == "replay":
            writer.batch_size = 4096

    if mode == "replay":
        workers = 2
```

Every enclosing condition must hold for a nested block to contribute. The region block means `env == "prod" and region == "eu"`; a flat compound condition can express the same selection. Nesting groups conditions, not configuration objects, and does not change field paths.

Contributions apply in source order, descending into a matching nested block at its position. For production, Europe, and replay, workers progress from 1 to 8 to 12 to 2. The result has retry attempts 4, retry delay 0.5, and batch size 4096. `IMPORT_WRITER__RETRY__ATTEMPTS=6` overrides the selected attempts value. No specificity ranking or implicit Cartesian-product enum is involved.

### Recursive dictionary and list contributions

```incan
model RoutingSettings:
    destinations: dict[str, str] = {
        "events": "http://localhost:8001",
        "metrics": "http://localhost:8002",
    }
    labels: list[str] = ["audited"]

ctx RouterConfig(env_prefix="ROUTER_"):
    env: Literal["dev", "prod"] = "dev"
    region: Literal["eu", "us"] = "eu"
    routing: RoutingSettings

    if env == "prod":
        routing.destinations = {
            "events": "https://events.example.com",
        }
        routing.labels = ["production"]

    if region == "eu":
        routing.labels = ["eu"]
```

For production in Europe, `events` uses the production endpoint while the unspecified `metrics` entry retains its default. Labels become `["audited", "production", "eu"]`. Dictionary keys follow their declared key type; dotted paths select declared fields, not arbitrary dictionary keys. The `=` in a `if` block contributes to recursive composition rather than performing ordinary assignment on a completed collection.

Preserving a value does not establish that it is appropriate for every deployment. An application validation rule may reject a production destination that still points to localhost. Configuration assembly must retain ordinary validation before publication. Explicit collection replacement and removal are out of scope. Authors should place environment-specific entries in their matching blocks or files, and reserve unconditional defaults and common files for contributions intended to survive across environments.

### Environment-specific collection entries

```incan
ctx ContractConfig(env_prefix="CONTRACT_"):
    env [env="ENV"]: Literal["dev", "prod"] = "dev"
    allowed_contracts: list[str] = []

    if env == "dev":
        allowed_contracts = ["sandbox"]

    if env == "prod":
        allowed_contracts = ["live"]
```

Development receives `["sandbox"]`; production receives `["live"]`. Only one block contributes, so ordinary recursive composition is sufficient. The YAML equivalent puts environment-specific entries in `dev.yaml` and `prod.yaml`, with universal entries in `common.yaml`.

Common contributions survive subsequent composition. An empty list contributes no elements, and an empty dictionary contributes no entries; neither clears lower-priority contents. This RFC introduces no `replace(...)` marker, clearing operator, removal marker, or YAML replacement tag. A future requirement to subtract genuinely shared entries would need its own concrete design case.

### Per-field env var customization

All fields read from env vars by default via `env_prefix` plus `UPPER_SNAKE_CASE`. For fields that need a custom env var name, use field metadata:

```incan
ctx AppConfig(env_prefix="APP_"):
    database_url: str = "sqlite:///dev.db"           # reads from APP_DATABASE_URL
    auth_token [env="API_TOKEN"]: str                # reads from API_TOKEN (custom name)
    internal_state [env=false]: str = "computed"     # never read from env
```

This reuses the existing `[key=value]` field metadata syntax (same pattern as `[alias="type_"]` on models).

### Require a value without a source-code default

```incan
ctx DatabaseConfig(env_prefix="DB_"):
    url: str
    pool_size: int = 10
```

The declaration is valid without a default for `url`. Resolution requires `DB_URL`; if it is absent, resolution returns a typed missing-field error and does not publish an application value. An absent `DB_POOL_SIZE` retains 10. Invalid supplied input produces a typed decoding failure. Required fields need not have dummy defaults or exhaustive condition coverage: a selected if block or another declared source may supply them. A required field with provably no possible source under the declared resolution contract is rejected statically.

### Override nested fields

```incan
model DatabaseSettings:
    host: str = "localhost"
    port: int = 5432

ctx AppConfig(env_prefix="APP_"):
    env: Literal["dev", "prod"] = "dev"
    database: DatabaseSettings

    if env == "prod":
        database.host = "db.example.com"
```

The model is an ordinary data declaration; implicit ctx construction and nested environment mapping are proposed behavior. A deployment can override its fields separately:

```bash
APP_ENV=prod
APP_DATABASE__HOST=db.internal
APP_DATABASE__PORT=6432
```

Resolution selects host `"db.internal"` and port 6432. Supplying only the host preserves port 5432. Nested overrides participate before final validation and frozen construction; they do not mutate a published value. An invalid port produces a typed configuration failure identifying both `database.port` and `APP_DATABASE__PORT`. Ambiguous environment-name mappings must receive diagnostics.

### Explicit environment names on nested groups

```incan
model DatabaseSettings:
    host: str = "localhost"
    port: int = 5432

ctx AppConfig(env_prefix="APP_"):
    database [env="DATABASE"]: DatabaseSettings
```

The explicit parent name becomes the root for its descendants. This example reads `DATABASE__HOST` and `DATABASE__PORT` rather than `APP_DATABASE__HOST` and `APP_DATABASE__PORT`. The authored spelling of an explicit environment name is preserved. Source access remains `AppConfig.database.host`; this mapping does not rename the field or its YAML key.

A more specific explicit environment name on a child field remains authoritative for that child. Other descendants continue to inherit their nearest explicitly mapped ancestor's root, followed by `__` and their remaining checked field-path segments. Distinct paths mapping to the same external name receive a diagnostic rather than declaration-order precedence.

### Overlapping whole-object and descendant inputs

Using `AppConfig` from the preceding example, suppose both inputs are supplied:

```bash
DATABASE='{"host":"db.example.com","port":5432}'
DATABASE__PORT=6432
```

Whole-model environment inputs use JSON objects. The object contributes checked field inputs, so omitted fields retain their lower-priority contributions or declared defaults. Explicit descendant inputs patch the corresponding fields before final checked construction. The result has host `"db.example.com"` and port 6432, independent of environment-variable enumeration order.

This overlap must emit a warning identifying both input names and the affected checked field path, without exposing their values. For example:

```text
warning: overlapping configuration inputs
    DATABASE__PORT overrides database.port supplied by DATABASE
```

The selected and superseded inputs remain available as redacted provenance. This warning does not turn the valid explicit override into an error or bypass validation. Distinct fields accidentally mapped to the same external name remain an ambiguity error; they are not this intentional parent/descendant case.

### Multiple context types

A program can have multiple `ctx` declarations for different concerns:

```incan
ctx AppConfig(env_prefix="APP_"):
    database_url: str = "sqlite:///dev.db"
    debug: bool = true

ctx InfraConfig(env_prefix="INFRA_"):
    cluster_size: int = 2
    region: str = "us-east-1"
```

Each is independently initialized and globally accessible: `AppConfig.debug`, `InfraConfig.region`.

### Separate sessions with shared model contracts

The following application-defined model is reusable across independently declared contexts:

```incan
model RetrySettings:
    attempts: int = 3
    delay_seconds: float = 0.5

ctx PostgresSession(env_prefix="PG_"):
    host: str = "localhost"
    port: int = 5432
    database: str = "app"
    retry: RetrySettings

ctx SnowflakeSession(env_prefix="SF_"):
    account: str
    warehouse: str
    database: str
    retry: RetrySettings
```

These are session configuration declarations, not claims that the example opens either database. Each declaration has its own retained value, environment namespace, resolution attempt, and preflight. `PG_RETRY__ATTEMPTS` and `SF_RETRY__ATTEMPTS` can select different values while both contexts reuse the same model contract. Reusing a model definition does not implicitly share one mutable model instance or duplicate a live resource.

Multiple contexts can be active together. A shared declaration is resolved once; it is not a factory for arbitrarily many runtime sessions. Applications needing such factories may define them through ordinary application types and APIs. Resource initialization and cleanup remain subject to the checked ownership and effect contracts; separately naming a context does not grant authority or thread safety. Managed resource lifetimes and any ctx integration with them are deferred to RFC 094. These configuration examples establish no new entry, exit, ownership-transfer, cleanup, or shutdown protocol and must not imply public cleanup guarantees based solely on backend destruction.

### Frozen and non-cloneable contexts

```incan
# Uses PostgresSession declared above.
PostgresSession.host = "backup.example.com"  # rejected: frozen field
PostgresSession.clone()                     # rejected: ctx is non-cloneable
PostgresSession.clone(host="backup")        # rejected: ctx is non-cloneable
```

The restriction applies to resolved context values as well as declaration-name access. There is no ctx Clone implementation, implicit whole-value copy capability, or generated clone-with-overrides surface. Generic Clone APIs must not bypass this restriction, even if every field is individually cloneable.

Ordinary model values and resource handles stored in a context retain their own checked capabilities. A shared pool handle may support cloning under its own contract; that operation does not clone the context or reconnect using different settings. Frozen bindings remain shallow and do not freeze a database, guarantee deep independence, or make mutable contents thread-safe.

### Test the ordinary access path

Tests use ordinary configuration sources and existing fixtures. RFC 019 provides the `env` fixture and TestEnv helper for temporary environment values; RFC 018 supplies the imported testing marker. The proposed ctx integration is fresh resolution state for each test case:

```incan
ctx AppConfig(env_prefix="APP_"):
    timeout: int = 30

def request_timeout() -> int:
    return AppConfig.timeout

module tests:
    from std.testing import test
    from rust::incan_stdlib::testing import TestEnv

    @test
    def test_short_timeout(mut env: TestEnv):
        env.set("APP_TIMEOUT", "5")
        assert request_timeout() == 5

    @test
    def test_long_timeout(mut env: TestEnv):
        env.set("APP_TIMEOUT", "60")
        assert request_timeout() == 60
```

The helper is application-defined and reads configuration normally. The fixture installs the input before first use and restores the process environment afterward. Each test has fresh ctx resolution state, so a runner worker process must not reuse one case's ctx cache for another. This isolation does not refresh ordinary values retained by a shared fixture: fixture lifetimes and retained data follow RFC 019's normal rules, without a ctx-specific prohibition or automatic fixture rebuilding. Within each test, successful initialization remains fixed and reused. The function return-annotation omission follows RFC 130. This example does not require an independent resolution method or a production reset operation.

### Explain context state and resolved settings

`ContextName.explain()` is the proposed passive inspection API. It returns a structured report describing resolution state and available retained evidence. For example, using `AppConfig` from the preceding test example:

```incan
# Proposed ctx inspection API; does not initialize AppConfig.
report = AppConfig.explain()
```

Before any resolution attempt, the report must identify the context as not resolved. It may describe declarations and declared source mappings, but must not claim that runtime inputs have been observed. Calling explain must not read environment variables, load source files, evaluate initializers, initiate resolution, or retry a failed attempt. This permits interactive inspection without activating the context. First runtime field use and explicit preflight retain their normal initialization behavior.

After successful resolution, explain uses the retained successful resolution evidence. After a failed attempt, available failure evidence must be clearly identified as belonging to an unsuccessful attempt, not a published configuration. A report must distinguish unresolved or in-progress state from successful publication. Exact report types and accessors remain open.

Inspection must be able to explain which source supplied a field and which lower-priority candidates it superseded. For example, a timeout report can distinguish a declaration default of 30 from an environment override of 60, and identify the contributing declaration and environment-variable name. Missing or unavailable evidence must be visible rather than inferred after the fact by rereading the environment. The report retains the full observed, ordered contribution history, including superseded scalar contributions and surviving collection contributions, rather than only the winning source. Retention does not require copies of entire source files or raw secrets. Any unavailable or omitted evidence must be identified explicitly; the report must not silently present an incomplete history as complete. This contract does not require evaluating a discarded default or an unselected expression merely to populate history.

An explanation of `ImportConfig.workers` for the production, Europe, and replay example can retain the ordered selection below. This is an illustrative rendering of retained evidence, not the structured report schema:

```text
workers = 2
    declaration default:          1
    if env == "prod":           8
        if region == "eu":     12
    if mode == "replay":        2  <- selected
```

Secret fields and sensitive inputs must remain redacted in inspection, failures, and retained provenance. RFC 103 owns secret-value handling; RFC 104 owns capability checks and receipts at environment or secret-provider boundaries. The exact report schema and accessors within the report remain open.

## Reference-level explanation

### Syntax

```text
ctx_decl       ::= "ctx" IDENT "(" ctx_params ")" ":" ctx_body
ctx_params     ::= ctx_param ("," ctx_param)*
ctx_param      ::= IDENT "=" expr
ctx_body       ::= NEWLINE INDENT (field_decl | configuration_if_block | docstring)+ DEDENT
configuration_if_block     ::= "if" expr ":" NEWLINE INDENT configuration_if_item+ DEDENT
configuration_if_item      ::= field_override | configuration_if_block
field_override ::= field_path "=" expr NEWLINE
field_path     ::= IDENT ("." IDENT)*
```

`ctx` is a core reserved keyword. Within its contribution body, `if` introduces runtime conditional configuration contributions. Conditions are evaluated during resolution, not while importing or compiling the declaration. Nested blocks keep the same contribution semantics; this spelling does not turn their contents into general executable statement bodies. Ordinary `match` expressions retain their existing meaning; this RFC removes the earlier special `match EnumType: case Variant:` configuration-selection proposal. The grammar abbreviates ordinary expression and field syntax, including field metadata and multiline expressions.

### Declaration rules

1. The ctx name must be a valid identifier. `env_prefix` is required and declares its external namespace independently of that name.
2. The optional `sources` parameter declares an ordered tuple of source descriptors. Its expressions may reference the ctx's checked fields; constructing descriptors must not eagerly read files at import.
3. The body must contain typed field declarations and may contain a docstring and conditional `if` blocks. Data contracts and ordinary application data may be defined outside the ctx and referenced without becoming additional fields.
4. A `if` body may contain field contributions and nested `if` blocks. Every enclosing condition must hold for a nested contribution to apply. Nesting must not introduce new field namespaces.
5. Contribution paths must identify declared fields rooted in the same ctx. They must not introduce unknown fields, mutate external objects, or treat arbitrary dictionary keys as declared field names.
6. Required fields need not have source defaults or exhaustive condition coverage. Successful resolution must supply every required field from selected declared sources. Missing input must produce a typed failure without publication. A declaration must be rejected statically when a required field provably has no possible source; compilation must not require deployment environment variables to be present.

### Type checking and pending model construction

1. **Field types** remain authoritative, including primitives, Options, collections, nested models, and RFC 133 Literal types. A valid field type does not automatically supply an environment-string decoder; unsupported decoding must be diagnosed separately.
2. **Conditions** must type-check as Boolean expressions. Selectors are ordinary declared fields using ordinary decoding and value semantics, not inferred enum axes.
3. **Contributions** must be compatible with the destination's declared type. Dotted paths must resolve through checked field identities and respect construction and visibility rules. A scalar `int` field cannot receive a string contribution. Ordinary expected-type assignment conversions must apply to defaults and contributions, including compatible plain values written into `Option[T]` destinations. Authors must not be required to wrap those values in `Some(...)`; None and explicit Option values must retain their ordinary meanings.
4. **Implicit model construction** must recursively collect pending inputs for a model-typed field without an initializer, using declared field defaults and selected contributions. This rule applies during ctx construction, not to ordinary model construction elsewhere. Required scalar inputs must remain missing until supplied; the process must not invent zero or empty values. Recursive model expansion without a finite construction boundary must be diagnosed rather than recurse indefinitely.
5. **Final construction** must validate the complete inputs and preserve ordinary model construction contracts before publication. Resolution must not construct an incomplete model and mutate it to fill required fields. An explicitly supplied model value contains its stored field values; it must not be treated as a partial constructor merely because some values originated from constructor defaults.
6. **Typed access** through `AppConfig.field_name` must use the declared field type. The resolved value must remain suitable for checked model-like access; its exact naming and trait surface remain open.
7. **Freezing** must fix resolved field bindings under RFC 132's shallow contract. Assigning to a published field must be rejected. Composition must not mutate completed frozen instances or retained lower-priority values through aliases. Freezing alone must not authorize deep immutability or cross-thread sharing.

### Env var type coercion

Environment variables are strings. Resolution must distinguish a missing variable from a supplied value before applying the declared decoder:

| Field type          | Env var input     | Resolution behavior                                   |
| ------------------- | ----------------- | ----------------------------------------------------- |
| Any supported type  | Absent            | Preserve the previously selected value.               |
| `str`               | `"hello"`         | Use the supplied string.                              |
| `int`               | `"42"`            | Parse the integer through its checked decoder.        |
| `float`             | `"3.14"`          | Parse the float through its checked decoder.          |
| `bool`              | `"true"` / `"1"`  | Decode a recognized Boolean spelling.                 |
| `Option[T]`         | Supplied value    | Decode `T` and select `Some(value)`.                  |
| `Option[T]`         | Explicit clear    | Select `None`; the external spelling remains open.    |
| `Literal[...]`      | Supplied value    | Decode its underlying type, then validate membership. |

Resolution of invalid input must produce a typed configuration error retaining the original parsing or validation failure under RFC 131's error contract. Structured details must identify the field, contributing source, and expected type; sensitive actual values must remain redacted. Explicit preflight returns that failure as a recoverable Result. An unchecked first-use read must report the full retained diagnostic and terminate execution unsuccessfully. Neither path turns invalid configuration into a generated panic. The standard `ConfigError` hierarchy below classifies the failure without replacing the retained original error.

Whole-model environment inputs must decode JSON objects into checked field contributions rather than require a complete model to be constructed before composition. Omitted fields retain their lower-priority inputs or defaults; required fields still missing after composition must fail. Supplied fields must satisfy their declared decoding and validation contracts, and unknown fields must receive diagnostics. Malformed JSON or incompatible supplied values must fail rather than silently fall back to defaults. A field declared as `str` must preserve supplied text even when that text resembles JSON; resolution must not guess an encoding from its contents. This rule does not settle whole-collection environment decoding; custom scalar conversions and validated-newtype construction follow the contracts below.

Literal conversion must reuse RFC 133's checked membership rules. Parsing an environment string into an underlying integer is the environment decoder's explicit responsibility; Literal conversion itself must not acquire implicit parsing or widening behavior.

### Reuse custom conversions and validated newtypes

Custom scalar environment decoding must reuse checked conversion contracts rather than introduce a parallel ctx-only parser mechanism. `TryFrom[str]` supplies the ordinary fallible text-conversion surface. Validated newtypes also retain RFC 017's canonical `from_underlying` path: decode the environment text into the declared underlying type, then invoke the recoverable validation hook. A newtype must not require an additional handwritten `TryFrom[str]` merely to accept its ordinary underlying representation.

For example, these are application declarations using the existing newtype validation hook and proposed ctx integration:

```incan
type Attempts = newtype int:
    def from_underlying(value: int) -> Result[Self, ValidationError]:
        if value <= 0:
            return Err(ValidationError("attempts must be positive"))
        return Ok(Attempts(value))

ctx JobConfig(env_prefix="JOB_"):
    attempts: Attempts = 3
```

`JOB_ATTEMPTS=5` first decodes the text into the underlying integer 5, then calls `Attempts.from_underlying(5)`. `JOB_ATTEMPTS=0` fails validation, while `JOB_ATTEMPTS=invalid` fails integer decoding before the hook can run. A string-backed newtype receives the supplied string as its underlying value. Generated default hooks and constraints retain RFC 017's meanings. Parsing text into a primitive here is an explicit environment-boundary responsibility, not a new ambient language coercion.

An explicitly supplied compatible `TryFrom[str]` for the destination must take precedence over automatic underlying-type text decoding. Once selected, a rejecting converter must fail resolution; the resolver must not try the underlying decoder as a second interpretation. In the absence of that explicit converter, a validated newtype must use checked underlying-type decoding followed by its recoverable `from_underlying` hook. Neither path may bypass canonical validation or use the implicit panic-producing construction path.

The resolver must preserve the selected converter's actual failure contract. The current `TryFrom[T]` trait returns `Result[Self, str]`: that rejection must be classified as explicitly textual conversion failure under `ConfigError.InvalidInput`, without inventing an original domain Error identity or a foreign stack. Where an owning checked conversion or validation contract returns a typed error, the configuration umbrella must retain that original hierarchy and available diagnostics under RFC 131. Sensitive text must be redacted or omitted at capture, including before secret wrapping. General migration of TryFrom/TryInto error signatures is outside this RFC.

For example, an application may deliberately accept word-valued retry counts through its explicit `TryFrom[str]`: `once` maps to 1 and `twice` maps to 2. Supplying `JOB_RETRIES=2` must then fail that declared conversion rather than silently retry ordinary integer parsing. Without the explicit text converter, the same integer-backed newtype uses checked integer decoding and `from_underlying`. Resource-lifetime rules remain deferred to RFC 094.

### Nested environment mapping

Default nested mappings must use `__` between field-path segments, with the declared prefix and the same upper-snake-case conversion used for top-level fields. For example, `AppConfig.database.port` under prefix `APP_` maps to `APP_DATABASE__PORT`. Paths must be resolved against checked field identities and declared types rather than inferred from runtime dictionaries or printed type names.

A supplied leaf override must use that field's checked decoder and validation contract. Unspecified siblings must retain their selected lower-priority values. Required siblings without a value must still fail resolution; nested overrides must not manufacture missing required data. Supporting nested leaf mappings must not require a whole-object string decoder solely to override supported leaves.

Resolution must combine selected nested values before final validation and frozen construction. It must not write into a completed frozen value or mutate a retained lower-priority object through an alias. Failures and provenance must preserve the structured field path and contributing external name. Ambiguous external-name mappings to distinct checked paths must be diagnosed rather than resolved by declaration order. An explicit environment name on a parent must become the root for descendant mappings, preserving that explicit spelling and appending `__` plus the remaining upper-snake-case field-path segments. A more specific explicit child name must override the inherited mapping for that child. Mapping changes must not rename source fields or YAML keys. Where a whole-object input and explicit descendant inputs are supplied at the same source priority, descendant inputs must patch their corresponding fields in the structured base before final checked construction, independent of environment enumeration order. Overlapping inputs must emit a warning identifying both external source names and the affected checked field path without exposing input values. The overlap and selected result must remain represented in redacted provenance. This rule must not change source-order precedence between conditional contributions or turn ambiguous mappings of distinct fields to one external name into warnings.

### Conditional selection and recursive composition

Resolution must apply selected `if` contributions in source order. It must descend into each selected nested block where that block appears, then resume its enclosing block and subsequent siblings. A false enclosing condition must prevent its nested blocks from contributing. Implementations must not reorder contributions according to condition specificity.

For overlapping scalar leaves, the later selected contribution must replace the earlier value. For nested model inputs and dictionaries, composition must retain unspecified fields or entries and recursively combine supplied ones. Conflicting dictionary keys must retain their ordinary declared equality semantics, with later scalar values winning. List contributions must append in source order without implied deduplication. Empty lists or dictionaries must contribute no new contents rather than clear lower-priority contents. Resolution must not infer replacement or removal intent from an empty collection. Other leaves must retain checked value semantics; composition must not infer dictionary structure from strings or arbitrary object attributes.

Selector environment inputs must be available before their conditions are evaluated. Direct environment inputs must retain higher priority than conditional contributions. Defaults and conditions must observe their field dependencies' resolved values, independent of field declaration order. Resolution must diagnose reached dependency cycles, including a condition that needs a field whose selection depends on that condition. It must not use repeated evaluation or condition-specificity ranking to find a fixed point. Which expression effects are permitted remains open.

Provenance must retain the applicable condition path and ordered contributions, including superseded scalar values and contributing collection fragments, subject to secret redaction. Inspection must be able to explain why a nested block applied and which later contribution superseded it. The exact inspection schema remains open; collection replacement and removal are excluded from this RFC.

### Declared source contract

The proposed `std.config.yaml(path, required=true)` adapter must describe a lazy YAML source. Path expressions must type-check as strings and participate in ctx dependency resolution. They must resolve independently of the source they select. A source-selection cycle must produce a typed diagnostic identifying the field, source expression, and available locations; it must not cause implicit reloading until a selector stabilizes.

Paths must follow ordinary runtime filesystem semantics relative to the application's working directory unless explicitly made absolute by application code. A source must not acquire authority by being declared in ctx; file access and retained evidence must follow RFC 104. Source descriptor creation and importing a context must not read files.

Resolution must combine field defaults, ordered selected `if` contributions, listed YAML contributions, and direct environment inputs in increasing priority. YAML fragments must use the checked ctx field shape and preserve missing values separately from explicit values. Nested field and dictionary contributions must merge recursively, and lists must append in order. Later scalar contributions must win. Supplied fields must be decoded against their declared types, and unknown field paths must be diagnosed rather than silently accepted. Incomplete fragments must not require premature model construction; complete required-field and construction validation must precede publication.

A required missing file must fail resolution. With `required=false`, absence must contribute no values; syntax errors, denied access, and other failures of a present source must remain typed failures. Diagnostics and provenance must retain source filenames, field paths, and available line and column locations while preserving redaction. Successful context resolution must retain its observed source facts rather than reread files during inspection or subsequent access. This contract does not provide automatic source discovery, implicit cross-process forwarding, or runtime reload.

### Dependent defaults

A default expression that references another field must use that field's resolved value under the declared source priority and conditional composition rules. Reordering field declarations must not change these dependencies' results. This rule must not change the source-order precedence of `if` contributions.

When a selected higher-priority scalar replacement supplies a field, resolution must not evaluate its discarded default solely to construct that field. If another field or condition needs the default's dependencies for its own resolution, those dependencies must still resolve. Recursive dictionary and list composition must retain contributing lower-priority fragments; it must not treat every collection contribution as a replacement.

Within one resolution attempt, a successfully resolved field must supply a consistent value to its dependents. A reached cycle through field defaults, conditional selection, or context dependencies must produce a typed failure with the dependency chain and available declaration or condition locations. Resolution must not publish partial values or retry calculations until they stabilize. Preflight returns such a failure; unchecked first use follows the existing fatal diagnostic rule.

A dependent default is construction input, not a continuously computed property. After successful resolution, reads and preflight must reuse the completed value. Context cloning is prohibited; separate declarations have independent resolution lifecycles. Permitted expression effects remain an open question.

### Standard configuration errors

The proposed `std.config.ConfigError` umbrella must use RFC 131's native error contract and retain the following typed categories:

| Category | Required diagnostic facts |
| --- | --- |
| `ConfigError.MissingInput` | The checked field path and the missing or contributing source identity. |
| `ConfigError.InvalidInput` | The checked field path, supplied source identity and the actual retained decoding, conversion or validation failure. A text-only conversion rejection remains explicitly textual. |
| `ConfigError.SourceFailure` | The selected source identity and the actual retained read, decoding, denied-access or secret-ingress failure. Available source locations remain attached. |
| `ConfigError.DependencyCycle` | The reached dependency chain of checked fields, conditions or contexts, with available source locations. |
| `ConfigError.Multiple` | The independently discovered direct failures and the facts identifying dependents blocked by those failures. |

Field paths must retain structured checked segments, rather than become strings used for dispatch. Source identities must distinguish environment names, YAML file locations and declaration locations. Raw supplied values and raw secrets must not be mandatory diagnostic payloads. A single failure must use its category; multiple independent failures must retain the members under `Multiple`, through RFC 131's shared reporting contract. This RFC must not add a parallel cause-traversal mechanism or reduce typed originals to messages.

Both `ContextName.preflight()` and unchecked first use must retain the same resolution-attempt facts. Preflight must return `Result[None, ConfigError]`; unchecked access must use the existing unsuccessful termination boundary. Neither may publish a partial value.

For example, consider required `database.host`, an integer `database.port`, an integer `workers`, and a derived DSN that needs both database fields. If the host is missing and both integers fail parsing, the report must retain all three direct failures and identify the DSN as blocked. It must not evaluate DSN construction or invent a fourth DSN parsing error. Correcting input and making a later explicit attempt must not rewrite the earlier error's facts.

### Reporting independent resolution failures

A failed resolution attempt must report independently discoverable configuration failures together in one typed configuration failure, preserving each underlying typed error, checked field path, source information, and available provenance under RFC 131 and the normal redaction contract. Preflight and unchecked first use must report the same attempt's failure facts through their respective recoverable and fatal boundaries.

A field or condition whose required dependency failed must be identified as blocked by that failure. Resolution must not evaluate the blocked expression or manufacture a separate parsing or calculation error for it. Aggregation must preserve declared dependencies and permitted expression effects; it must not invent values or bypass validation to continue. Failure must not publish a partially resolved context.

### Lifecycle

1. **Resolution** must be fallible and produce a complete frozen value or a typed configuration failure. It must resolve pending defaults, selector inputs, ordered conditional contributions, and supplied environment values under the declared priority rules.
2. **First use** must resolve the whole context at runtime before returning its first declaration-name field value. Merely declaring, importing, or statically referencing a context must not activate it. Unused contexts must not require environment values or acquire source capabilities. Successful resolution must publish the completed value once; failure must not publish partial state. There must be no automatic pre-main sweep.
3. **Preflight** must be available as `ContextName.preflight()` and return `Result[None, ConfigError]`. It must perform the same resolution and prepare the same published value as first use. After successful publication, preflight and field reads must reuse that value without rereading sources or revalidating a different environment. A failed preflight must remain recoverable through ordinary Result handling.
    - A failed attempt must leave the context unresolved, without poisoning it permanently or publishing partial values. A subsequent explicit preflight must be allowed to make a fresh attempt against the declared sources. Each invocation must initiate at most one attempt; resolution must not automatically loop or retry. Application code owns recovery decisions and retry timing.
    - Fresh attempts may reevaluate defaults and validators and repeat their permitted effects. Failure must not imply rollback. A later attempt must not rewrite the retained diagnostic facts of an earlier returned error. After successful publication, no subsequent preflight may replace the value or rerun its resolution.
4. **Unchecked failure** during a direct field read must report the full typed configuration diagnostic and terminate the application unsuccessfully. The report must use RFC 131's common diagnostic contract, retaining available causes, origins, and stacks with secret redaction. This boundary must not pretend to return a Result through a plain field access, introduce catchable exceptions, or convert ordinary invalid configuration into a generated panic. Callers requiring recovery must preflight before the read.
5. **Separate sessions and non-cloneability** must be expressed through separate ctx declarations, each with its own environment prefix and retained resolution state. External model contracts may be reused without automatically sharing instances. Resolved ctx values must not expose Clone, Copy, or RFC 143 clone-with-overrides capabilities, including through aliases, imports, or generic APIs. This restriction applies even when every retained field is individually cloneable. Contained ordinary values must retain their own checked capabilities; their operations must not implicitly duplicate the whole context or restart its resolution.
6. **Concurrent resolution** must coordinate callers of the same unresolved published context around one in-progress attempt. Joining callers must wait for and observe that attempt's outcome without seeing partial values or silently initiating a retry. Success must publish one completed value reused by every caller. On failure, a joining preflight must return the failure from that attempt; a joining unchecked read must follow the fatal diagnostic rule. A later explicit preflight may start a new attempt. Coordination must preserve the synchronous Result contract of preflight and must not introduce implicit await into field reads. Waiting must remain subject to dependency-cycle detection.
7. **Sharing** must obey the checked ownership, borrowing, and thread-safety contracts of every retained field. Freezing alone must not authorize sharing mutable descendants across tasks or threads.
8. **Dependencies** between contexts must be permitted during resolution. A reached dependency must supply a completed value before its dependent resolution uses it. Cyclic dependencies, including cycles across concurrent attempts, must produce typed configuration failures rather than deadlock or expose partial state. The diagnostic must retain the observed dependency chain and available source locations. A preflight must return a cycle encountered by its resolution as a failure; unchecked first use must follow the fatal diagnostic rule. Dependency evidence must describe actual reached resolution relationships rather than treating an unused declaration as an active dependency.
9. **Test isolation** must give each test case fresh ctx resolution state through RFC 019's runner lifecycle, including before its function-scoped fixture setup. Ordinary application calls within the test must use that test's state. Within one test, first use, preflight, coordinated initialization, retained successful values, and failure behavior must follow the normal contracts. Reusing a worker process across test cases must not reuse a prior case's ctx cache. Tests must configure ordinary sources through existing fixtures rather than require a special resolution or reset API. Process-environment fixture concurrency and restoration remain governed by RFC 019; per-test ctx state must not be mistaken for isolation of the underlying process environment. Shared fixture lifetimes and ordinary retained values must follow RFC 019 rather than introduce a ctx-specific restriction, automatic refresh, or fixture rebuild.

### Lowering model

The compiler lowers each `ctx` declaration to a resolved-value representation, a publication mechanism, and resolution logic that:

1. collects pending field inputs and recursively declared model defaults without constructing incomplete values;
2. obtains the selector inputs required by conditions, including their direct environment inputs;
3. applies selected `if` contributions in source order, recursively visiting nested blocks and merging their inputs;
4. resolves declared source paths and combines their checked fragments in listed order, above conditional contributions;
5. gives direct environment inputs precedence over other contributions, without appending the same source twice;
6. validates the complete result and constructs a frozen value;
7. retains source-selection provenance and publishes the successful application value for ordinary read access.

These are logical resolution requirements, not rigid execution phases. Dependencies must resolve before their consumers use them, and discarded scalar defaults must not be evaluated solely for construction. Field declaration order must not affect dependency results; selected conditional contributions retain their source order. Permitted expression effects remain an explicit open question.

The backend implementation may use a once-only singleton primitive or an equivalent runtime mechanism for publication. The contract is fallible resolution, complete frozen construction, set-once application publication, typed reads, and explainable source selection; the runtime representation does not require configuration failures to panic.

### Structured type and provenance contracts

Decoding, validation, and inspection must consume checked field identities and structured type information consistent with RFC 086 and RFC 141. They must not infer semantics by parsing printed type names or define a separate configuration-only type system. Shared field metadata must retain the meanings defined by its owning RFC, including RFC 090's general-purpose `env` and `secret` metadata.

Retained provenance must preserve the full observed, ordered contribution history, including selected and superseded scalar contributions, surviving collection contributions, and source declaration locations or external source names when available. Retention must not silently reduce that history to only the winning source. Unavailable or omitted evidence must be identified explicitly so an incomplete history cannot be mistaken for a complete one. This requirement must not cause discarded defaults or unselected expressions to be evaluated solely for evidence, and it does not require whole-file copies or raw-secret retention. It must distinguish unavailable evidence from known absence. The generated `ContextName.explain()` API must return a structured report using retained resolution facts rather than repeat environment reads and present later values as historical evidence. Inspection must not initiate or retry resolution, read runtime sources, or evaluate initializers. Before resolution it must identify the context as unresolved; failed-attempt evidence must remain distinct from a successful published value. In-progress state must not be presented as completed resolution. Redaction must apply when evidence is captured as well as when it is displayed, including upstream failure payloads.

Environment and secret-provider access must remain subject to RFC 104's governed boundary checks and receipts. Configuration resolution must not grant implicit authority or bypass those checks. Full observed contribution-history retention is settled; exact provenance schemas, storage representation and lifetime, and accessors within the explain report remain open.

## Design details

### `ctx` vs `model` — how they relate

`ctx` and `model` share field declaration syntax but serve fundamentally different purposes:

|                         | `model`                          | `ctx`                                                 |
| ----------------------- | -------------------------------- | ----------------------------------------------------- |
| **What it is**          | A data shape — many instances    | A configuration schema with layered resolution        |
| **How many**            | Many instances, created freely   | One retained value per declaration                    |
| **How you access it**   | Via a variable: `user.name`      | Declaration-name reads or checked access to its value |
| **How it's created**    | `User(name="Alice", age=30)`     | First use or preflight                                |
| **Mutability**          | Per-instance (mutable via `mut`) | Frozen field bindings; shallow contents contract      |
| **Conditional blocks**  | No                               | Nestable `if` blocks and recursive composition      |
| **Env var integration** | No                               | Yes — fields auto-read from env vars                  |

**Mental model: `model` describes data. `ctx` describes the world the data lives in.**

### `ctx` contains models, not the other way around

```incan
model ClusterConfig:
    node_type: str = "small"
    min_workers: int = 2
    max_workers: int = 4

ctx InfraConfig(env_prefix="INFRA_"):
    env: Literal["dev", "prod"] = "dev"
    cluster: ClusterConfig

    if env == "prod":
        cluster.node_type = "large"
        cluster.min_workers = 8
        cluster.max_workers = 16
```

Nested access works naturally: `InfraConfig.cluster.node_type`. The external model defines the data contract; ctx assembles and validates its pending inputs lazily. Neither nested type declarations nor explicit constructor calls are required for this grouping. Reusable application data may also remain outside the ctx.

A ctx declaration is not itself an ordinary instance field value. Its resolved configuration can be passed to application code under the eventual checked value API. Ordinary data types need not acquire an implicit dependency on globally published configuration.

### Interaction with existing features

**Traits:** declaration-name configuration access does not make the declaration an ordinary instance. The resolved value's trait and construction surface must follow its checked representation; exact generated traits remain open.

**async/await:** published fields are synchronously available after successful initialization. Reading them does not require `await`. Cross-task or cross-thread access still requires the retained values to satisfy the ordinary sharing rules; shallow freezing does not establish those rules automatically.

**Imports/modules:** `ctx` declarations follow normal module visibility. A `pub ctx` in a library can be imported by consumers. Importing the library or declaration does not resolve it. Only runtime first use or an explicit preflight activates its published configuration; unused library contexts do not impose configuration requirements on the application.

**Error handling:** resolution produces typed configuration failures retaining original errors under RFC 131. Explicit preflight returns a recoverable Result. An unchecked first-use read reports the full typed diagnostic and terminates execution unsuccessfully at that point. Neither path silently continues with malformed configuration or panics for a decoding failure. A library can expose preflight through its own fallible API before starting work.

**Field metadata and secrets:** `ctx` reuses model field metadata and RFC 090's shared `env` and `secret` meanings. RFC 103's SecretStr and SecretBytes remain typed secret values, and their ingress and storage failures propagate through the configuration error boundary. A secret marker is not permission to retain raw sensitive input in provenance or failure reports.

**Context managers and resource lifetimes:** the design of managed resource entry, exit, ownership transfer, cleanup after partial initialization, retained lifetimes, and shutdown is deferred to RFC 094. RFC 033 does not introduce a parallel lifecycle protocol or promise managed cleanup solely from backend destruction. Its frozen configuration, source composition, preflight, and publication contracts do not settle context-manager integration. Resource-lifetime design is not an outstanding decision owned by this RFC.

**Clone and Copy:** resolved ctx values are excluded from whole-value copying and RFC 143 clone-with-overrides generation. The exclusion must hold through imports, aliases, and generic APIs. Ordinary models and resource handles keep their own capabilities; referencing their contracts from ctx must not make them non-cloneable elsewhere.

### Compatibility / migration

This is a new feature, so there is no migration path to manage. `ctx` does become a reserved keyword, which is a breaking change for anyone already using it as an identifier. Pre-1.0, that is acceptable.

## Alternatives considered

### Decorator on model (`@ctx`)

```incan
@ctx(env_prefix="APP_")
model AppConfig:
    database_url: str = "sqlite:///dev.db"
    ...
```

**Rejected.** Conditional configuration contributions, deferred recursive construction, retained application access, and environment integration define a resolution contract beyond ordinary model construction. A distinct declaration makes that contract visible.

### Runtime-only config (like Pydantic BaseSettings)

Use a regular model with runtime env var reading:

```incan
model AppConfig:
    database_url: str
    batch_size: int

config = AppConfig.from_env(prefix="APP_")
```

**Rejected as the entire configuration surface.** An ordinary typed model can preserve checked field references and is useful for independently passed values. A constructor alone does not provide this RFC's ordered conditional composition, published-name access, or retained source provenance. Resolved configuration values should nevertheless reuse ordinary typed data and construction contracts rather than invent an incompatible value model.

### Clone a resolved context for session variations

Rejected. Retained fields can include dependent defaults and live resources. Copying a resource may share its existing connection, while replacing its source settings leaves the stored DSN or resource unchanged. Automatically reconnecting would turn cloning into fresh resolution and change its failure and effect contract. Separate ctx declarations provide independent retained sessions with reusable external models, without requiring every field to support Clone.

### Special matching on enum type names

The earlier proposal used `match Env: case Prod:` to implicitly read an environment variable. This gave a type name a hidden runtime-selection meaning and used a separate matching grammar. Ordinary selector fields and `if env == "prod":` keep the tested value explicit, support compound conditions and nesting, and preserve ordinary match expressions.

### A separate match for each field

Field-local matches can express isolated defaults, but coordinated environment choices become duplicated across unrelated branches of the configuration tree. One `if` block may contribute to several depths without repeating its condition. Mandatory nested model declarations and implicit model-construction assignment arms are unnecessary for this surface.

### Soft keyword via library

Ship `ctx` as a library-provided soft keyword instead of a core keyword.

**Rejected.** `ctx` is universally useful rather than domain-specific. Every application needs configuration. Making it a library feature would mean the most basic use case, reading a setting from an env var, requires a dependency. The env var integration and singleton semantics also belong to the language contract, not an optional library add-on.

## Drawbacks

- **Language surface**: a reserved `ctx` declaration and conditional `if` blocks add syntax to learn and support.
- **Order-sensitive composition**: moving overlapping contributions can change results. Lists append rather than replace inside conditional composition, so ordinary-looking assignments need clear documentation and provenance.
- **Deferred construction**: pending model inputs and condition dependencies require diagnostics for missing values and cycles without eager incomplete instances.
- **Global dependency**: convenient published-name access still couples callers to the retained application configuration. Contexts cannot be cloned to obtain variations; another retained session requires a separate declaration or an ordinary application-owned factory.
- **Shallow freezing**: fixed field bindings do not guarantee recursively immutable contents or thread safety. Retained field types and access paths need checked sharing contracts.
- **Retained evidence**: source-selection provenance costs runtime storage and must preserve redaction, including when upstream validation fails.
- **Late failure**: a direct read may first resolve configuration during application work and terminate execution if it fails. This dependency is not represented as a Result in the reader's signature. Explicit preflight offers early, recoverable checks, but callers must choose to use it.

## Layers affected

- **Language surface** — `ctx` becomes a core reserved keyword with first-class declaration syntax, field declarations, dotted contributions, and nestable `if` blocks.
- **Type system** — implementations must validate field types, Boolean conditions, dotted path compatibility, deferred model construction, and typed declaration-name access.
- **Initialization model** — runtime first use, fallible preflight, complete frozen construction, independent session declarations, publication coordination, and declared source priority.
- **Errors / validation** — RFC 131 configuration failures retaining original causes, RFC 133 Literal checks, and shared construction validation.
- **Stdlib / sources** — proposed `std.config.yaml` descriptors, checked fragment loading, runtime source paths, and file-location diagnostics.
- **Runtime / governance** — secret-safe retained provenance, RFC 103 secret ingress, RFC 104 governed environment and file access, and ordinary checked sharing rules.
- **Formatter** — `ctx` declarations, dotted contributions, and nested `if` blocks should format predictably.
- **LSP / tooling** — completions and hover for `CtxName.field` access, structured field types, mapped sources, and resolution explanations.
- **Test support** — RFC 019 fixture integration, fresh ctx resolution state per case, and ordinary application access without a production reset API.

## Design Decisions

- Runtime conditional contributions in ctx use `if`, including nested blocks. `when` and compiler-owned `@when` remain compile-time selectors under RFC 163. This spelling change preserves dependency-based resolution, source-order recursive merging, lazy first use and retained successful preflight values.
- Retain concise ctx declarations and convenient declaration-name reads. First runtime use resolves the whole context and publishes the completed frozen value. Imports and static references do not activate contexts, and there is no automatic pre-main sweep.
- `ContextName.preflight()` performs the same whole-context resolution early, returning a recoverable Result with None on success and the configuration error umbrella on failure. Successful preflight prepares the retained value used by subsequent reads; it does not merely validate inputs for later rereading. The error is the standard `ConfigError` umbrella with retained original failures.
- Failed preflights leave the context unresolved and permit subsequent explicit attempts. Recovery and retry timing belong to the application; each call makes at most one attempt without automatic retries. A new attempt may repeat permitted construction effects, while previously returned errors retain their own diagnostic facts. Successful resolution is permanent and is reused by subsequent reads and preflights.
- Concurrent first uses share one in-progress resolution attempt and its outcome. Waiting callers do not silently retry after failure. Successful initialization publishes one completed frozen value reused thereafter; this coordination does not make preflight awaitable or introduce implicit await for fields.
- Context resolution may depend on other contexts. Reached cycles produce typed configuration errors containing the dependency chain and available source locations, including cycles across workers. Preflight returns the failure; unchecked first use reports it and terminates execution. No path may deadlock or expose partial values.
- Resolution preserves original parsing and validation failures under RFC 131. An unchecked first-use failure reports the full typed diagnostic and terminates execution unsuccessfully. It is not Result propagation through a plain field access, a new catchable exception, or a generated panic. Callers requiring recovery use preflight before reading fields.
- Independently discoverable resolution failures are reported together in one typed configuration failure, retaining each original error and source evidence. Dependent expressions blocked by failed inputs are identified without evaluating them or manufacturing extra errors. No partial context is published.
- Resolved ctx values have fixed field bindings under RFC 132's shallow freezing contract and are non-cloneable. Whole-context Clone, Copy, and RFC 143 clone-with-overrides are prohibited, including through generic APIs. Separate declarations provide independently resolved sessions and can reuse external model contracts. Contained model values and resource handles retain their own checked capabilities; freezing does not establish deep immutability or thread safety.
- Tests configure ordinary sources through existing fixtures and exercise normal ctx reads. RFC 019's runner supplies fresh ctx resolution state per case, including before function-scoped fixture setup, without reusing a prior case's ctx cache across worker-process reuse. Successful initialization remains fixed within each test. Ordinary data retained by shared fixtures follows normal fixture lifetimes; ctx introduces no special shared-fixture prohibition, automatic refresh, or fixture rebuilding. An independent-value or production reset API is not required for testing.
- Declared types remain authoritative, including RFC 133 Literal constraints. Shared field metadata and structured type information are reused rather than introducing a parallel configuration type system.
- Required fields may rely entirely on declared runtime sources, without source defaults or exhaustive condition coverage. Missing required input produces a typed resolution failure; no partial application value is published. A provably unsourceable required field is rejected statically, but compilation does not depend on deployment environment values.
- `env_prefix` is required and explicit. Renaming a context declaration must not implicitly change its environment-variable namespace; per-field explicit environment names remain authoritative.
- Whole-model environment inputs use JSON objects as checked field contributions, preserving omitted lower-priority fields and defaults before final construction. Explicit descendant inputs patch this structured base at the same source priority. The descendant input wins independently of environment enumeration order, with a warning identifying overlapping input names and affected field paths without printing their values. Original input facts and selected results remain in redacted provenance. Ordinary construction and validation still apply.
- An explicit compatible `TryFrom[str]` takes precedence over automatic underlying decoding. Its rejection fails resolution without a fallback parse. Without that converter, validated newtypes use checked underlying decoding and recoverable `from_underlying`, without duplicate handwritten text conversion or panic-producing coercion. The resolver retains actual typed failures when supplied and explicitly identifies the current TryFrom string rejection without fabricating typed identity or stacks. General conversion-trait signature migration is outside this RFC.
- Nested environment overrides use `__` between field-path segments. An explicit parent name rebases its descendant namespace, while a more specific explicit child mapping remains authoritative. Explicit names preserve their spelling without renaming source fields or YAML keys. Supplied leaves retain ordinary checked decoding and validation; unspecified siblings retain selected values. Nested resolution precedes final validation and frozen construction, and ambiguous external-name mappings receive diagnostics.
- Selectors are ordinary typed fields, including Literal fields, using the normal environment mapping. `if` accepts Boolean conditions and may nest; every enclosing condition must hold. A selected block may contribute to multiple depths through dotted declared-field paths.
- Selected contributions apply in source order, descending into matching nested blocks at their position. Later scalar contributions win without condition-specificity ranking. Nested model inputs and dictionaries merge recursively, preserving unspecified entries; list contributions append in order. Direct environment inputs retain higher priority.
- Model-typed fields without initializers are assembled recursively from pending field defaults and contributions during whole-context resolution. No explicit constructor call or nested type declaration is required. Ordinary model construction elsewhere remains unchanged; required scalar fields still need a source, and complete values must pass ordinary validation before publication.
- Dependent defaults and conditions observe resolved field values, including conditional contributions and direct environment inputs. Field declaration order does not affect dependency results. Replaced scalar defaults are not evaluated solely for construction; contributing collection fragments remain part of recursive composition. Reached cycles produce typed dependency diagnostics without fixed-point recalculation. Defaults are construction inputs, and successful context resolution is retained rather than recalculated on reads.
- Explicit YAML sources use the proposed `std.config.yaml` adapter and an ordered `sources` tuple. Their recursive contributions apply above conditional settings and below direct environment inputs. A common file may be followed by `yaml(f"config/{env}.yaml")`, with the filename selector resolved independently before that selected file is read. Header source expressions may refer to checked ctx fields. Files are required by default; `required=false` permits absence without hiding malformed or unreadable present inputs. Loading is lazy, successful values are retained, unknown field paths receive diagnostics, and source evidence retains file locations under redaction. No YAML-root selection API or automatic cross-process transport is required by this decision.
- Environment-specific collection entries belong in their respective `if` blocks or selected environment files, with unconditional defaults and common files reserved for universal contributions. Empty collections do not clear lower-priority contents. Explicit replacement, clearing, and removal markers are out of scope.
- Managed resource lifetimes and their integration with ctx are deferred to RFC 094, including cleanup after partial initialization and the lifetime of successfully retained resources. RFC 033 introduces no separate resource lifecycle or destructor-only public cleanup guarantee. This is a scope deferral, not an unresolved question for RFC 033.
- An absent override preserves the previously selected value; an explicit optional clear replaces it with None. Nested absence must not be flattened during layer resolution. External clearing syntax remains open.
- `ContextName.explain()` returns a structured report of resolution state and available retained evidence without initializing or retrying the context, reading runtime sources, or evaluating initializers. It supports passive interactive inspection before first use, clearly distinguishes unresolved, in-progress, failed, and successfully resolved state, and does not present failure evidence as a published value. Resolution retains the full observed, ordered contribution history, including superseded scalar and surviving collection contributions, with RFC 103 redaction and RFC 104 authority checks and receipts. Missing or omitted evidence is explicit rather than silently truncating the explanation to the winning source; evidence retention does not require whole-file copies, raw secrets, or evaluating otherwise skipped expressions. Inspection does not reread sources and misrepresent current values as historical evidence. Exact report schemas and accessors remain open.
- The standard configuration umbrella is `std.config.ConfigError`, with MissingInput, InvalidInput, SourceFailure, DependencyCycle and Multiple categories. Checked paths, source identities, retained originals and blocked dependencies remain structured and secret-safe. Single failures retain their category; independent failures aggregate without evaluating blocked expressions. Preflight returns `Result[None, ConfigError]`; unchecked reads report the same attempt facts and terminate unsuccessfully.

## Unresolved questions

1. **Expression effects during resolution.** Which function calls and effects may default expressions, contribution expressions, and conditions perform? How are their capabilities and evaluation guarantees communicated without making configuration depend on incidental field traversal order? Resolved-value dependencies, field declaration-order independence, skipped discarded scalar defaults, ordered conditional composition, and typed cycle failures are settled.

2. **Explicit optional clearing.** What external spelling distinguishes an intentional None from missing input, and how are empty strings handled for Option[str] without silently losing valid text?

3. **Resolved-value construction and sharing.** What checked representation exposes fields, validation, and eligible traits while keeping the context frozen and non-cloneable? Which retained field types and access paths permit global or cross-thread publication while preserving RFC 132's shallow contract?

4. **Provenance and inspection API.** What structured report types and accessors within `ContextName.explain()` expose selected and superseded sources, unavailable evidence, and ordered contribution history? The passive entry point, its non-activating behavior, and full observed ordered history with explicit evidence gaps are settled. How should storage representation, lifetime, and redaction preserve that contract without storing raw secrets or copying entire sources?

5. **Remaining environment decoding contracts.** Which checked decoders accept collection-valued environment inputs, and how do those contributions participate in recursive composition while preserving declared source priority? Reusing checked text conversions and canonical newtype validation without duplicate parsers is settled, as are whole-model JSON inputs and explicit descendant overrides with an overlap warning. Conditional and YAML dictionary merging and ordered list appends are settled. Explicit collection replacement, clearing, and removal markers are excluded rather than awaiting syntax.

<!-- Rename this section to "Design Decisions" once all questions have been resolved. An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
