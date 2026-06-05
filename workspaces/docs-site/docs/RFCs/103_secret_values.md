# RFC 103: `std.secrets` — Secret strings, secret bytes, and redaction-safe values

- **Status:** Draft
- **Created:** 2026-05-24
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 017 (validated newtypes with implicit coercion)
    - RFC 033 (`ctx` typed configuration context)
    - RFC 066 (`std.http` HTTP client surface)
    - RFC 072 (`std.logging` structured logging)
    - RFC 078 (tool execution and typed workflow actions)
    - RFC 089 (`std.environ` runtime environment access)
    - RFC 090 (typed CLI framework)
    - RFC 093 (`std.telemetry` observability)
    - RFC 102 (semantic layer inspection surface)
- **Issue:** https://github.com/dannys-code-corner/incan/issues/661
- **RFC PR:** -
- **Written against:** v0.3
- **Shipped in:** —

## Summary

This RFC proposes `std.secrets` as Incan's standard library home for secret value wrappers, beginning with `SecretStr` and `SecretBytes`. Secret values are ordinary typed values that can flow through config, CLI, environment, HTTP, logging, telemetry, workflow actions, and generated reports without revealing their plaintext through unauthorized display, debug, structured logs, diagnostics, default serialization, or inspection surfaces. The goal is not to pretend secrets become impossible to copy or exfiltrate inside a compromised process; the goal is to make plaintext exposure deny-by-default, keep raw access scoped and intentional, and allow stronger protected storage such as encrypted idle memory where the backend can provide it.

## Core model

1. **Secrets are values, not logging conventions:** secrecy must travel with the value's type so redaction is not rebuilt separately by every caller.
2. **Plaintext exposure is deny-by-default:** Incan-owned display, debug output, logs, telemetry attributes, diagnostics, semantic inspection, reports, and default serialization must not reveal secret contents.
3. **Reveal is scoped and intentional:** APIs that need raw bytes or strings should consume `SecretStr` or `SecretBytes` directly, or require an intentionally named scoped reveal operation that tooling can recognize.
4. **Protected idle storage is preferred:** implementations should keep secret contents encrypted or otherwise protected while idle when a backend can do so meaningfully, and decrypt only inside a scoped reveal operation.
5. **Memory guarantees are honest:** protected idle storage and zeroization reduce exposure, but the public contract must not promise that every intermediate copy made by encoders, transport backends, operating systems, foreign APIs, crash handlers, or the process itself is erased.
6. **Specific types come first:** `SecretStr` and `SecretBytes` are the initial stable surface. A generic `Secret[T]` may come later if it does not weaken the concrete-string and concrete-bytes contracts.
7. **Tooling preserves sensitivity metadata:** CLI, LSP, semantic inspection, workflow action output, generated docs, and reports should know that a value exists and what type it has without seeing the raw payload.

## Motivation

Python ecosystems often represent secrets with wrapper classes, Pydantic field flags, logging filters, and framework-specific conventions. Those mechanisms help, but they remain easy to bypass because Python string interpolation, `repr`, dictionaries, serializers, exception traces, and third-party clients can all treat the wrapped value as just another object unless every boundary cooperates perfectly.

Incan has a better opportunity because its stdlib, typechecker, generated Rust, structured logging, HTTP surface, CLI framework, environment access, action metadata, and semantic inspection model can agree on one value-level contract. A `SecretStr` used as a CLI option, loaded from an environment variable, passed to an HTTP authorization helper, logged as a structured field, or surfaced in an action report should remain recognizably present but redacted all the way through those boundaries. The core promise should be stronger than "nice `repr`": plaintext must not leave a secret wrapper through an Incan-owned surface unless the code has made an explicit reveal decision or passed the value to a trusted API that owns a scoped reveal internally.

This RFC also closes a design gap left deliberately open by RFC 017. Validated newtypes can model domain-specific string and byte constraints, but secret handling is more than a validation constraint: it changes display, debug, logging, diagnostic serialization, wire-boundary APIs, equality, cloning, and drop behavior expectations.

## Goals

- Add a `std.secrets` module with `SecretStr` and `SecretBytes`.
- Make redaction a property of the value type rather than a per-logger or per-HTTP-client convention.
- Prevent plaintext secret emission through Incan-owned display, debug, diagnostic, logging, telemetry, semantic inspection, generated-report, and default serialization paths.
- Require safe default behavior for display, debug, structured logs, telemetry, diagnostics, semantic inspection, and generated reports.
- Provide intentionally named, tooling-visible APIs for scoped exposure of raw secret material at trusted boundaries.
- Prefer encrypted or otherwise protected idle memory for secret storage where the target backend can provide it meaningfully.
- Let stdlib consumers such as `std.http`, `std.environ`, typed CLI surfaces, `ctx`, workflow actions, logging, and telemetry accept or preserve secret values without converting them to plain `str` or `bytes`.
- Define a conservative serialization contract that prevents accidental JSON, TOML, YAML, CLI, or report emission of raw secret contents.
- Define honest memory-handling expectations, including scoped plaintext lifetimes and best-effort zeroization for plaintext buffers where the backend can support it.
- Leave room for future secret providers, vault integrations, redaction policies, and generic secret wrappers without blocking the concrete `SecretStr` and `SecretBytes` surface.

## Non-Goals

- This RFC does not define a password manager, vault, keyring, or secrets backend.
- This RFC does not define encryption at rest for source files, manifests, lockfiles, logs, reports, or generated artifacts.
- This RFC does not provide full information-flow control, taint tracking, or a data-loss-prevention system.
- This RFC does not guarantee that all process memory, operating-system buffers, network buffers, allocator copies, panic payloads, crash dumps, foreign library copies, or compiler temporaries are erased.
- This RFC does not claim that encrypted idle storage protects against arbitrary code execution inside the same process; any implementation must still hold or derive decryption material somewhere.
- This RFC does not make secrets safe to expose to untrusted code.
- This RFC does not define random secret generation; a future `std.random` or expanded `std.secrets` surface may do that separately.
- This RFC does not define identity protocols such as SAML, OAuth, OIDC, JWT validation, service-account exchange, or single sign-on workflows.
- This RFC does not standardize every sensitive-data class such as PII, payment data, access tokens, API keys, passwords, and private keys as distinct semantic categories in the initial surface.
- This RFC does not replace access control, capability checks, sandboxing, policy approval, or runtime permission boundaries.

## Guide-level explanation

Users should be able to load a secret value and pass it through normal code without turning it into a plain string just to keep working.

```incan
from std.environ import env
from std.secrets import SecretStr

token: SecretStr = env.secret_str("SERVICE_TOKEN")?
println(token)
```

The printed value is redacted. The exact placeholder is a design detail, but it must not include the token.

HTTP clients and other stdlib APIs should accept secret values directly:

```incan
from std.environ import env
from std.http import Client, bearer
from std.secrets import SecretStr

token: SecretStr = env.secret_str("SERVICE_TOKEN")?
client = Client(default_headers={"Authorization": bearer(token)})
response = client.get("https://api.example.com/items")?
```

The caller does not reveal the token manually. The HTTP boundary may perform a scoped internal reveal when constructing the wire request, but diagnostics, debug output, retries, telemetry, and action reports must preserve sensitivity.

When a raw value is genuinely needed, the operation should read as intentional and scoped:

```incan
from std.secrets import SecretBytes

def sign_with_key(raw_key: bytes) -> Signature:
    return hmac.sign(raw_key, payload)


key: SecretBytes = SecretBytes.from_hex(env.secret_str("SIGNING_KEY_HEX")?)?
signature = key.with_exposed_bytes(sign_with_key)
```

The exact reveal method names remain open in this Draft. The important property is that code review, search, LSP, and policy tooling can recognize raw-secret exposure sites, and that the preferred shape does not hand an ordinary string or byte buffer back to the caller for uncontrolled storage.

Secret values should also compose with typed configuration and CLIs:

```incan
from std.secrets import SecretStr

ctx Deploy:
    api_token: SecretStr = env("API_TOKEN")
    endpoint: str = "https://api.example.com"
```

An inspection view can show that `api_token` exists, is required, and has type `SecretStr`, without showing the token itself.

## Reference-level explanation

### Module surface

`std.secrets` must expose `SecretStr` and `SecretBytes`.

`SecretStr` must represent owned UTF-8 secret text. `SecretBytes` must represent owned binary secret material.

The module may expose helper types such as redaction placeholders, reveal guards, redacted serialization adapters, or sensitivity metadata, but `SecretStr` and `SecretBytes` are the required initial surface.

### Construction

`SecretStr` must be constructible from a `str` through an explicit constructor or conversion path. `SecretBytes` must be constructible from `bytes` through an explicit constructor or conversion path.

Construction APIs should make plain-to-secret conversion visible in source. Implicit conversion from `str` to `SecretStr` or from `bytes` to `SecretBytes` should be avoided unless a surrounding API already declares that an input position is secret, such as a typed CLI option, an environment accessor, or a `ctx` field.

`SecretStr` should support conversion to `SecretBytes` using an explicit encoding operation. `SecretBytes` should support UTF-8 decoding into `SecretStr` through a fallible operation.

`std.environ` should provide secret-returning helpers, such as a `secret_str` shape, so callers do not need to load an environment variable as plain text and then wrap it manually.

### Display and debug behavior

`SecretStr` and `SecretBytes` must redact their contents in display, debug, assertion failure, panic, diagnostic, and structured-inspection contexts owned by the Incan standard library and toolchain.

The redacted representation must communicate that the value is secret and present. It must not include the secret contents, prefix, suffix, length, checksum, entropy estimate, or other derived value unless a later RFC defines an explicit policy for such metadata.

String interpolation and formatting protocols must use the redacted representation by default. Formatting a secret must not implicitly call the reveal operation.

### Plaintext leakage boundary

The normative security boundary for this RFC is Incan-owned plaintext emission. `SecretStr` and `SecretBytes` must not reveal raw contents through Incan-owned display, debug, panic formatting, assertion messages, diagnostics, structured logs, telemetry attributes, semantic inspection, generated reports, CLI help, CLI echo, default serialization, or action metadata.

This boundary also applies to nested structures. A model, list, dict, result, error, request, response, action input, or telemetry event containing a secret value must preserve redaction when formatted or serialized through Incan-owned mechanisms.

Trusted stdlib APIs may reveal plaintext internally only for the duration of the operation that requires it, such as computing an HMAC or sending an HTTP authorization header. That internal reveal must not become observable through error values, debug payloads, telemetry attributes, retry reports, or generated artifacts.

### Reveal operations

`SecretStr` must provide an intentionally named operation for exposing the raw `str` value. `SecretBytes` must provide an intentionally named operation for exposing the raw bytes value.

Reveal operations must be easy for tooling to identify. They should use names that communicate risk, such as `expose_secret`, `expose_secret_str`, or `expose_secret_bytes`, rather than neutral names like `value`, `get`, or `as_str`.

The preferred reveal shape is scoped: a callback, guard, or equivalent API that makes plaintext available only for a bounded lexical or dynamic lifetime. Owned plaintext copies should either be unavailable by default or exposed through a more explicit and noisier escape hatch than scoped reveal.

The reveal operation may return a borrowed view, a scoped guard, a backend-specific safe-access wrapper, or an owned copy only when the API name makes the copying behavior explicit. The accepted design must document the lifetime, copying behavior, and zeroization behavior of every reveal path.

APIs that genuinely need raw material should prefer accepting `SecretStr` or `SecretBytes` directly instead of forcing user code to reveal the secret first.

### Serialization

Default data serialization of `SecretStr` and `SecretBytes` must not emit raw secret contents.

For diagnostic serialization, generated reports, semantic inspection, logs, telemetry, and CLI output, the value must serialize as a redacted secret marker or an equivalent structured redaction object.

For data formats that are intended to leave the process as user data, such as JSON request bodies, TOML files, YAML files, or generated artifacts, default serialization should fail unless the caller chooses an explicit redacted adapter or an explicit reveal operation. This avoids accidentally sending placeholder text where a real secret was expected, and avoids accidentally persisting the raw value.

### Equality, ordering, and hashing

`SecretStr` and `SecretBytes` should not expose ordering operations by default.

Equality is an open design question. If equality is exposed, it should avoid timing behavior that is obviously inappropriate for token, password, or key comparison, and the docs must state whether the comparison is constant-time. If the implementation cannot provide a meaningful constant-time guarantee for a given storage representation, it should prefer an explicit comparison helper over ordinary equality.

Hashing secret values should be avoided by default because hash maps and debug tooling often make key material harder to reason about. If hash support is needed later, it should be introduced deliberately with documented semantics.

### Cloning and copying

`SecretStr` and `SecretBytes` must not be trivially copyable value types.

Cloning may be supported when the language's ownership model requires it for ordinary value flow, but clone operations must preserve secrecy metadata and must not reveal raw contents. The docs must state that cloning creates another copy of the secret material.

### Protected storage and memory handling

Implementations should keep secret contents encrypted or otherwise protected while idle when the target backend can provide a meaningful protected-storage implementation. Plaintext should be produced only inside scoped reveal operations or trusted stdlib internals that need raw bytes or text for a bounded operation.

Any protected-storage implementation must document its threat model. Encrypting a buffer while idle can reduce accidental plaintext retention and may help with some memory disclosure scenarios, but it does not protect against arbitrary code execution in the same process, a compromised runtime, a debugger with full process access, or backend APIs that must receive plaintext.

Plaintext buffers created during reveal should be zeroized as soon as their scoped use ends when the backend can support that. `SecretBytes` should zeroize owned plaintext memory on drop when generated code can do so without weakening correctness. `SecretStr` may also zeroize owned storage when implemented over a mutable owned buffer, but the public contract must not imply that all UTF-8 string copies are erased.

Both types must document that redaction is an exposure-control guarantee for standard display, debug, logging, telemetry, diagnostics, and serialization paths. Protected idle storage and zeroization strengthen that guarantee, but they are not full memory-forensics or same-process-compromise guarantees.

The implementation should avoid unnecessary copies in stdlib APIs that consume or forward secret values, especially HTTP authorization helpers, cryptographic helpers, and secret-provider integrations.

### Logging, telemetry, diagnostics, and inspection

`std.logging`, `std.telemetry`, diagnostics, and semantic inspection must treat `SecretStr` and `SecretBytes` as sensitive fields by type.

Structured outputs should preserve the fact that a field exists, its declared type, and relevant non-sensitive metadata such as source kind when appropriate. They must not include the raw value.

Tooling should mark explicit reveal operations as searchable and inspectable sites. LSP hover, semantic inspection, and policy checks may use those sites to explain where secret material leaves the protected wrapper.

### HTTP and wire-boundary APIs

`std.http` authorization helpers, header builders, request diagnostics, retry reporting, and telemetry should preserve secret sensitivity. Header values constructed from `SecretStr` or `SecretBytes` must be redacted in debug-facing output even if the header name is not in a built-in sensitive-header list.

`std.http` may expose raw secret material internally when sending a request. That internal exposure must not change the public `Request`, `Response`, `HttpError`, log, telemetry, or action-output redaction contract.

### Typed actions, CLIs, and configuration

Typed action inputs, CLI options, and `ctx` fields should be able to declare `SecretStr` and `SecretBytes` directly.

Machine-readable action metadata should distinguish a required secret input from a plain string input. Action output must not include raw secret values unless a future policy system defines an explicit, user-approved reveal path.

CLI help may show that an option expects a secret. It must not echo secret defaults or environment-derived values.

### Higher-level identity protocols

Identity and federation protocols such as SAML, OAuth, OIDC, JWT validation, service-account exchange, and single sign-on workflows should be built above `std.secrets`, not inside it. Those protocols have their own security models: XML or JSON token formats, signatures, certificates, issuer and audience validation, replay windows, metadata discovery, clock skew, session state, and provider-specific policy.

`std.secrets` should provide the primitive secret value contract those packages consume. A future identity or platform library may store private keys, bearer tokens, client secrets, SAML assertions, or signed credentials in `SecretStr` or `SecretBytes`, and may use scoped reveal internally when validating or transmitting them. That does not make `std.secrets` responsible for the protocol semantics.

## Design details

### Syntax

This RFC does not introduce new parser syntax. `SecretStr` and `SecretBytes` are stdlib types.

### Semantics

Secret values have ordinary type identity and can be passed, returned, stored in models, and used in containers according to the language's normal value rules. Their special behavior is attached to display, debug, formatting, serialization, logging, telemetry, diagnostics, inspection, equality, hashing, cloning, reveal, protected storage, and drop semantics.

Implicit downcast from `SecretStr` to `str` and from `SecretBytes` to `bytes` must not be allowed. Raw exposure must require either an explicit scoped reveal operation or a trusted stdlib API that accepts a secret type directly and owns the scoped reveal internally.

### Interaction with existing features

- **RFC 017 (validated newtypes)**: secret values may use newtype-like machinery internally, but their display, debug, serialization, and memory expectations are a separate contract.
- **RFC 033 (`ctx`)**: typed configuration can declare secret fields and source them from environment or future secret providers without exposing raw values in inspection.
- **RFC 066 (`std.http`)**: HTTP auth helpers and headers should accept secret values and preserve redaction through request diagnostics, retries, telemetry, and workflow output.
- **RFC 072 (`std.logging`)**: structured logging should redact secret-typed fields by default.
- **RFC 078 (typed workflow actions)**: action inputs and outputs should preserve sensitivity metadata so reports can describe secret use without exposing values.
- **RFC 089 (`std.environ`)**: environment access should provide secret-returning helpers that avoid plain-string staging.
- **RFC 090 (typed CLI framework)**: CLI options can use `SecretStr` and `SecretBytes` as declared types.
- **RFC 093 (`std.telemetry`)**: telemetry attributes and events must redact secret-typed values.
- **RFC 102 (semantic layer inspection surface)**: semantic inspection should represent secret facts as redacted facts with stable type and source metadata.

### Compatibility / migration

This feature is additive. Existing code that stores tokens in plain strings remains valid, but docs and examples should prefer `SecretStr` and `SecretBytes` at configuration, CLI, environment, HTTP, and action boundaries once the types exist.

Migration helpers may wrap existing `str` or `bytes` values explicitly. Such helpers should not hide the fact that code still created a plain value before wrapping it.

## Alternatives considered

- **Plain `newtype str` and `newtype bytes` only**
  - Rejected because newtypes alone do not define formatting, debug, serialization, logging, telemetry, equality, cloning, and memory behavior.
- **Logging-only redaction**
  - Rejected because secrets leak through more than logs: debug strings, exception messages, assertions, generated reports, telemetry, HTTP diagnostics, CLI echo, and semantic inspection all matter.
- **HTTP-only secret headers**
  - Rejected because the same token often starts in environment or CLI config, flows through `ctx`, enters an HTTP client, appears in telemetry, and may be referenced by typed actions.
- **One generic `Secret[T]` as the first surface**
  - Rejected for the initial version because strings and bytes have distinct encoding, display, comparison, and memory concerns. A generic wrapper may still be useful later.
- **Always serialize redacted placeholders**
  - Rejected for data serialization because silently writing `<redacted>` into JSON payloads, config files, or generated artifacts can create corrupt data and hide bugs.
- **Unscoped raw getters**
  - Rejected because a method that returns an ordinary `str` or `bytes` as the primary reveal path makes it too easy to store, log, serialize, or return plaintext accidentally.
- **Always require manual reveal before wire use**
  - Rejected because it pushes raw exposure into user code and makes the safe path noisier than the risky path.

## Drawbacks

- Secret wrappers add friction when code genuinely needs raw strings or bytes.
- Redaction can create a false sense of security if users interpret it as encryption, access control, or memory-forensics protection.
- Encrypted idle storage has key-management and performance costs, and it cannot protect against every same-process threat.
- Equality, hashing, and serialization need conservative choices that may surprise users expecting string-like behavior.
- Stdlib modules and tooling must consistently honor the secret contract or the abstraction becomes unreliable.
- The exact reveal API needs careful design because it becomes the standard searchable marker for sensitive exposure.

## Implementation architecture

*(Non-normative.)* The Rust-backed implementation should use owned storage with redacting display and debug implementations. Where practical, secret payloads should be stored encrypted while idle with process-local key material and decrypted only inside scoped reveal guards. Plaintext buffers created by reveal guards should be zeroized when the guard closes. `SecretBytes` should use a zeroizing buffer where available. `SecretStr` may store UTF-8 in a protected byte buffer with fallible UTF-8 views, or use another representation that preserves the public contract. Stdlib consumers should pass secret wrappers through typed APIs and reveal internally only at the final trusted boundary.

## Layers affected

- **Stdlib / Runtime (`incan_stdlib`)**: must provide `std.secrets`, `SecretStr`, `SecretBytes`, redaction behavior, construction helpers, scoped reveal operations, protected-storage behavior where supported, and integration hooks for stdlib consumers.
- **Typechecker / Symbol resolution**: must preserve the distinct types and reject implicit conversion from secret wrappers to plain `str` or `bytes`.
- **Emission**: generated Rust must preserve redacting display/debug behavior and best-effort zeroization where promised.
- **Formatter**: no syntax changes are required, but examples and generated code should preserve readable secret-type annotations.
- **LSP / Tooling**: hover, completion, diagnostics, semantic inspection, action metadata, generated docs, and policy checks should preserve sensitivity metadata and make reveal operations discoverable.
- **Docs / Examples**: environment, CLI, HTTP, logging, telemetry, and workflow examples should demonstrate secret values instead of plain string tokens.

## Unresolved questions

- What are the exact reveal method names for `SecretStr` and `SecretBytes`?
- Should reveal operations return borrowed views, owned copies, scoped guards, or multiple variants?
- Should scoped reveal be the only stable v1 reveal surface, with owned plaintext extraction left for a later explicit escape hatch?
- Should encrypted idle storage be mandatory for all v1 targets, or a documented target capability with redaction and zeroization as the portable floor?
- How should process-local encryption keys be generated, stored, rotated, and destroyed?
- Should ordinary equality be available, or should secret comparison require explicit constant-time helper functions?
- Should `SecretStr` attempt to provide the same zeroization behavior as `SecretBytes`, or should the docs make `SecretStr` strictly a redaction-first wrapper?
- What exact redaction placeholder should display, debug, and diagnostic serialization use?
- Should default data serialization of secrets fail everywhere, or should some stdlib-owned formats serialize structured redaction objects by default?
- Should `std.secrets` eventually expose a generic `Secret[T]`, and if so, what protocol must `T` satisfy?
- Should secret provenance metadata distinguish environment variables, CLI input, config files, secret providers, and generated values in the initial surface?
- How should reveal sites interact with future policy approval, sandboxing, and capability checks?
- Should secret values participate in model field metadata automatically, or should fields still require an explicit `secret=true` marker for generated schema and docs?

<!-- Rename this section to "Design Decisions" once all questions have been resolved. An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
