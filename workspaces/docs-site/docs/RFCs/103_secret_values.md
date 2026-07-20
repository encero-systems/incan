# RFC 103: `std.secrets` — Secret strings, secret bytes, and redaction-safe values

- **Status:** Planned
- **Created:** 2026-05-24
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 017 (validated newtypes with implicit coercion)
    - RFC 033 (`ctx` typed configuration context)
    - RFC 066 (`std.http` HTTP client surface)
    - RFC 070 (`Result` combinators and borrowed callback adaptation)
    - RFC 072 (`std.logging` structured logging)
    - RFC 078 (tool execution and typed workflow actions)
    - RFC 085 (field metadata and type constraints)
    - RFC 089 (`std.environ` runtime environment access)
    - RFC 090 (typed CLI framework)
    - RFC 093 (`std.telemetry` observability)
    - RFC 102 (semantic layer inspection surface)
    - RFC 104 (ambient runtime capabilities and receipts)
- **Issue:** https://github.com/encero-systems/incan/issues/661
- **RFC PR:** -
- **Written against:** v0.3
- **Shipped in:** —

## Summary

This RFC proposes `std.secrets` as Incan's standard library home for secret value wrappers, beginning with `SecretStr` and `SecretBytes`. Secret values are ordinary typed values that can flow through config, CLI, environment, HTTP, logging, telemetry, workflow actions, and generated reports without revealing their plaintext through unauthorized display, debug, structured logs, diagnostics, default serialization, or inspection surfaces. The goal is not to pretend secrets become impossible to copy or exfiltrate inside a compromised process; the goal is to make plaintext exposure deny-by-default, keep raw access scoped and intentional, and allow stronger protected storage such as encrypted idle memory where the backend can provide it.

## Core model

1. **Secrets are values, not logging conventions:** secrecy must travel with the value's type so redaction is not rebuilt separately by every caller.
2. **Plaintext exposure is deny-by-default:** Incan-owned display, debug output, logs, telemetry attributes, diagnostics, semantic inspection, reports, and default serialization must not reveal secret contents.
3. **Reveal is scoped and intentional:** APIs that need raw bytes or strings should consume `SecretStr` or `SecretBytes` directly, or require an intentionally named scoped reveal operation that tooling can recognize.
4. **Protected idle storage is capability-reported:** every target provides redaction, scoped reveal, and zeroizable owned storage; targets may claim protected idle storage only when they satisfy the stronger key and authenticated-encryption contract.
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
- Define honest memory-handling expectations, including scoped plaintext lifetimes and required non-elidable zeroization attempts for wrapper-owned storage and reveal buffers.
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
from std.environ import secret_str
from std.secrets import SecretStr

token: SecretStr = secret_str("SERVICE_TOKEN")?
println(token)
```

The printed value is exactly `<secret>`. It confirms that a secret is present without exposing its type, payload, length, source identifier, or other derived material.

HTTP clients and other stdlib APIs should accept secret values directly:

```incan
from std.environ import secret_str
from std.http import Client, bearer
from std.secrets import SecretStr

token: SecretStr = secret_str("SERVICE_TOKEN")?
client = Client(default_headers={"Authorization": bearer(token)})
response = client.get("https://api.example.com/items")?
```

The caller does not reveal the token manually. The HTTP boundary may perform a scoped internal reveal when constructing the wire request, but diagnostics, debug output, retries, telemetry, and action reports must preserve sensitivity.

When a raw value is genuinely needed, the operation should read as intentional and scoped:

```incan
from std.environ import secret_bytes
from std.secrets import SecretBytes

def sign_with_key(raw_key: bytes) -> Signature:
    return hmac.sign(raw_key, payload)


key: SecretBytes = secret_bytes("SIGNING_KEY")?
signature = key.with_exposed_bytes(sign_with_key)?
```

`with_exposed_bytes` is the only public raw-bytes reveal shape in this RFC. Its callback receives a read-only, non-escaping view that is valid only for the callback invocation. `SecretStr` provides the corresponding `with_exposed_str` operation. Code review, search, LSP, and future policy or audit tooling can therefore identify every user-authored raw-secret exposure site by name without normalizing an unscoped getter as ordinary value access. Because RFC 089 exposes only Unicode environment values, this example's `SecretBytes` contains the UTF-8 encoding of `SIGNING_KEY`, not raw non-Unicode host bytes.

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

`std.secrets` must expose `SecretStr`, `SecretBytes`, `SecretError`, `SecretErrorKind`, `SecretSourceKind`, `SecretStorageProtection`, `RedactedSecret`, and `redacted`.

```incan
from std.serde.json import Serialize
from std.traits.error import Error


pub enum SecretSourceKind(str):
    Environment = "environment"
    Cli = "cli"
    Config = "config"
    Provider = "provider"
    Generated = "generated"
    Derived = "derived"
    Unknown = "unknown"


pub enum SecretStorageProtection(str):
    Portable = "portable"
    Protected = "protected"


pub model RedactedSecret with Serialize:
    pub sensitive: bool
    pub redacted: bool
    pub type_ [alias="type"]: str


pub enum SecretErrorKind(str):
    InvalidUtf8 = "invalid_utf8"
    IntegrityFailure = "integrity_failure"
    Poisoned = "poisoned"
    StorageUnavailable = "storage_unavailable"


pub model SecretError with Error:
    pub kind: SecretErrorKind

    def message(self) -> str: ...


pub def redacted(secret: SecretStr | SecretBytes) -> RedactedSecret: ...
```

This is a normative source-signature sketch for prospective APIs, not a claim that the pre-implementation compiler can check the block as a standalone module. It uses current Incan union, value-enum, public-model, field-alias, trait-adoption, and function-signature spelling; `...` omits implementation bodies, while `SecretStr` and `SecretBytes` are specified below.

`RedactedSecret` is an intentionally public, forgeable, non-secret serialization marker. Direct construction carries no security or trust authority: constructing a marker never makes a plain value secret, and consumers must derive sensitivity from the checked secret type or another authoritative policy source rather than trusting marker claims. `redacted` produces the canonical marker by setting `sensitive=True`, `redacted=True`, and `type_` to exactly `"SecretStr"` or `"SecretBytes"`. Ordinary serialization uses the field alias and therefore emits exactly `{"sensitive": true, "redacted": true, "type": "SecretStr"}` or the corresponding `SecretBytes` object. The adapter contains no provenance, storage mode, payload slot, or payload-derived value.

`SecretError` is the only public operation error for secret storage and payload access. `message()` must return a stable redacted explanation selected only from `kind`; it must not expose payload, length, invalid UTF-8 position, cryptographic algorithm detail, nonce, key epoch, source identifier, or another payload-derived fact. `InvalidUtf8` reports failed UTF-8 validation, `IntegrityFailure` reports the first authenticated-storage verification failure, `Poisoned` reports later access to a value whose integrity has already failed, and `StorageUnavailable` reports inability to establish or maintain the selected storage contract.

`SecretStr` must represent owned UTF-8 secret text. `SecretBytes` must represent owned binary secret material.

`SecretStr` and `SecretBytes` are the only secret-bearing public value types. `SecretError`, its category enum, the provenance and protection enums, and `RedactedSecret` carry no payload. Reveal guards, plaintext-view types, and process key material are runtime/compiler mechanisms, not public values that user code can name or retain.

### Construction

`SecretStr.from_str(value)` and `SecretBytes.from_bytes(value)` are the explicit plain-to-secret constructors. Both are fallible and assign `SecretSourceKind.Unknown` because caller-provided plaintext has no trusted source classification. They return `SecretError` with kind `SecretErrorKind.StorageUnavailable` rather than accepting a value when the runtime cannot establish the storage mode it selected before construction.

These constructors secure the wrapper-owned allocation, not the ordinary value supplied by the caller. The runtime may adopt a uniquely owned mutable input allocation when it can immediately place that allocation under the secret-storage contract; otherwise it must copy into wrapper-owned storage. In either case, the RFC does not promise to erase an earlier caller-owned `str` or `bytes` allocation. Secret-returning ingress APIs remain the preferred way to avoid plaintext staging.

Construction APIs must make plain-to-secret conversion visible in source. There is no general implicit conversion from `str` to `SecretStr` or from `bytes` to `SecretBytes`. A typed CLI option, environment accessor, `ctx` field, or other declared secret ingress may construct the wrapper internally, but that boundary must propagate `SecretError` through its own fallible result rather than hide a storage failure.

This restriction applies only to conversion from a plain value; it does not restrict where an existing secret value can be stored. `SecretStr` and `SecretBytes` are ordinary declared field types, so a model may declare `password: SecretStr`, be constructed with an existing `SecretStr`, and then be passed or nested according to the normal model rules. The field keeps its secret type and sensitivity metadata throughout that flow.

```incan
from std.environ import secret_str
from std.secrets import SecretStr

model SomeHttpHeader:
    user: str
    password: SecretStr

credentials = SomeHttpHeader(user="service", password=secret_str("HTTP_PASSWORD")?)
```

This model is valid. An ordinary serializer rejects `credentials` because it would return reusable plaintext, while a trusted HTTP wire encoder may accept it and serialize the nested password only inside the outbound send operation as defined below.

`SecretStr.encode_utf8()` must return `Result[SecretBytes, SecretError]` while preserving provenance and without staging an ordinary public `bytes` value. `SecretBytes.decode_utf8()` must return `Result[SecretStr, SecretError]` while preserving provenance and without staging an ordinary public `str`. Besides storage-integrity errors, decoding may return `SecretError` with kind `SecretErrorKind.InvalidUtf8`; the error exposes no invalid byte, offset, length, or derived payload material.

`std.environ` must provide `secret_str(name) -> Result[SecretStr, EnvironError | SecretError]` and `secret_bytes(name) -> Result[SecretBytes, EnvironError | SecretError]` so callers do not need to load an environment variable as a plain value and then wrap it manually. Both assign `SecretSourceKind.Environment`; their error values must not echo the environment value. Both follow RFC 089's Unicode environment boundary: `secret_str` reads the same Unicode value as `get`, and `secret_bytes` UTF-8-encodes that Unicode value directly into secret storage. `secret_bytes` does not expose raw non-Unicode platform environment bytes and must still return `EnvironError` with kind `EnvironErrorKind.NotUnicode` when RFC 089 cannot represent the host value as Unicode. Host lookup and encoding may require transient runtime or operating-system buffers; implementations must keep wrapper-owned scratch within the ingress scope and zeroize it on supported cleanup paths, but this RFC does not claim control over host-owned copies.

Every secret value must carry a non-payload provenance category. `SecretSourceKind` must distinguish `Environment`, `Cli`, `Config`, `Provider`, `Generated`, `Derived`, and `Unknown`. Trusted constructors such as environment and CLI accessors must assign their category; explicit wrapping uses `SecretSourceKind.Unknown`; conversions preserve provenance; and an operation that combines or transforms secret material uses `SecretSourceKind.Derived`. The initial contract stores no source name, environment-variable name, provider path, or other source identifier because those identifiers can themselves be sensitive.

### Display and debug behavior

`SecretStr` and `SecretBytes` must redact their contents in display, debug, assertion failure, panic, diagnostic, and structured-inspection contexts owned by the Incan standard library and toolchain.

Display and string formatting must render both types exactly as `<secret>`. Debug output must render `SecretStr(<secret>)` or `SecretBytes(<secret>)`. Assertion, panic, and human-readable diagnostic rendering must use the display or debug form appropriate to that surface. None of these representations may include the secret contents, prefix, suffix, length, checksum, entropy estimate, provenance, storage-protection mode, or another derived value.

String interpolation and formatting protocols must use the redacted representation by default. Formatting a secret must not implicitly call the reveal operation.

### Plaintext leakage boundary

The normative security boundary for this RFC is Incan-owned plaintext emission. `SecretStr` and `SecretBytes` must not reveal raw contents through Incan-owned display, debug, panic formatting, assertion messages, diagnostics, structured logs, telemetry attributes, semantic inspection, generated reports, CLI help, CLI echo, default serialization, or action metadata.

This boundary also applies to nested structures. A model, list, dict, result, error, request, response, action input, or telemetry event containing a secret value must preserve redaction when formatted or serialized through Incan-owned mechanisms.

Trusted stdlib APIs may reveal plaintext internally only for the duration of the operation that requires it, such as computing an HMAC or sending an HTTP authorization header. That internal reveal must not become observable through error values, debug payloads, telemetry attributes, retry reports, or generated artifacts.

### Reveal operations

`SecretStr` must provide `with_exposed_str`; `SecretBytes` must provide `with_exposed_bytes`.

```incan
pub class SecretStr:
    @staticmethod
    def from_str(value: str) -> Result[Self, SecretError]: ...

    def encode_utf8(self) -> Result[SecretBytes, SecretError]: ...
    def with_exposed_str[R](self, use: Callable[str, R]) -> Result[R, SecretError]: ...
    def constant_time_eq(self, other: Self) -> Result[bool, SecretError]: ...
    def clone_secret(self) -> Result[Self, SecretError]: ...
    def source_kind(self) -> SecretSourceKind: ...
    def storage_protection(self) -> SecretStorageProtection: ...


pub class SecretBytes:
    @staticmethod
    def from_bytes(value: bytes) -> Result[Self, SecretError]: ...

    def decode_utf8(self) -> Result[SecretStr, SecretError]: ...
    def with_exposed_bytes[R](self, use: Callable[bytes, R]) -> Result[R, SecretError]: ...
    def constant_time_eq(self, other: Self) -> Result[bool, SecretError]: ...
    def clone_secret(self) -> Result[Self, SecretError]: ...
    def source_kind(self) -> SecretSourceKind: ...
    def storage_protection(self) -> SecretStorageProtection: ...
```

This is likewise a normative method-signature sketch. The generic `Callable[...]` spelling exists today, but the secret-specific scoped-borrow and non-escape behavior is new compiler work required by this RFC; the block is not presented as an already-compilable implementation.

Both operations must accept a callback and make a read-only plaintext view available only while that callback is executing. The view must not be storable, returnable, capturable by a longer-lived closure, or otherwise allowed to outlive the reveal scope. The callback result may escape only when it does not contain or borrow from the plaintext view. Normal return, a callback result or error, and an unwind-capable failure must all close the reveal scope and run cleanup. Panic-abort, process kill, power loss, non-unwinding cancellation, and other abrupt termination are outside this guarantee.

The callback parameter is written as ordinary `str` or `bytes` in Incan source, as in the guide example. Consistent with Incan's implicit ownership model and RFC 070 callback adaptation, the compiler treats that parameter as a borrowed scoped view at this call boundary without exposing lifetime syntax or a public view type. RFC 103 implementation must add a secret-reveal-specific scoped-borrow rule: it rejects returning the borrowed parameter, storing it in a model or container, assigning it to longer-lived storage, or capturing it in an escaping closure. This is required compiler behavior, not a claim about the current pre-implementation stdlib. The callback may still make an explicit owned copy; that copy is caller-owned plaintext outside the wrapper's guarantees.

Reveal callbacks must be synchronous. They must not return an awaitable or generator, suspend, or transfer execution to another task while the borrowed plaintext view is live. Async, streaming, and background stdlib operations must accept the secret wrapper directly and own any shorter internal reveal at their final trusted boundary.

This RFC defines no public owned-plaintext extraction operation and no public reveal guard. Adding either is a security-surface expansion that requires a follow-up RFC. A caller that intentionally needs an owned plaintext copy must create it inside the callback; that allocation is ordinary plaintext outside the wrapper's zeroization guarantee and must remain conspicuous at the reveal site.

Tooling must treat direct calls to both methods as explicit reveal sites. This requirement is independent of any capability framework: the source operation remains named, searchable, and policy-visible even in an ungoverned runtime. Future governance integration is specified conditionally below; this RFC neither reserves a capability identifier nor defines a receipt schema.

APIs that genuinely need raw material should prefer accepting `SecretStr` or `SecretBytes` directly instead of forcing user code to reveal the secret first.

### Serialization

`SecretStr` and `SecretBytes` must not implement the ordinary data-serialization protocol. A statically typed serializer must reject either type, including a nested occurrence, during checking. A dynamic or type-erased serializer must reject the value through its existing error channel before reading the payload; an infallible serializer cannot accept a secret-typed value. No rejection path may panic with, emit, or retain plaintext or a placeholder.

This rule prohibits a generic serializer from returning reusable plaintext JSON, TOML, YAML, or another ordinary serialized value; it does not prohibit a trusted typed wire boundary from accepting a model that contains secret fields. An HTTP client may, for example, accept a header or request-body model containing `SecretStr` and encode its JSON representation directly into the outbound request. That operation must be explicitly wire-bound and fallible, keep any serialized plaintext buffer inside the send scope, and expose no API that returns the encoded plaintext to user code. Request diagnostics, debug output, retries, errors, telemetry, and captured artifacts must continue to redact the nested field.

Diagnostic serialization, generated reports, semantic inspection, logs, telemetry, and CLI output are type-aware observation surfaces rather than ordinary user-data serialization. They must project a structured marker with `sensitive=true`, `redacted=true`, and the declared type. They may include `source_kind` and `storage_protection` only when the active inspection policy allows those non-payload facts. They must never include a payload slot, payload-derived material, or a source identifier.

`std.secrets.redacted(secret)` must return the `RedactedSecret` adapter defined in the module surface. Choosing that adapter is an explicit decision to emit a stable marker, not a raw-secret serialization path. Sending or persisting plaintext requires a trusted typed boundary that accepts the secret wrapper, or an explicit scoped reveal followed by caller-owned serialization.

### Equality, ordering, and hashing

`SecretStr` and `SecretBytes` must not implement ordinary equality, ordering, or hashing protocols.

Both types must provide `constant_time_eq(other: Self) -> Result[bool, SecretError]`. The helper must compare the complete operands without early exit, and its control flow and memory access must not depend on matching prefixes or payload-byte values. Any plaintext scratch storage used for comparison follows the same scoped zeroization contract as explicit reveal. Execution time may depend on the longer operand length, so the API does not promise to hide length or eliminate operating-system, hardware, compiler, or same-process side channels. Cross-type comparison is unavailable; callers must perform an explicit checked conversion first. An integrity or poison error returns no comparison result.

Adding ordinary equality, ordering, hashing, or a differently scoped comparison helper requires a follow-up RFC.

### Cloning and copying

`SecretStr` and `SecretBytes` must not be trivially copyable value types.

Both types must support an explicit `clone_secret() -> Result[Self, SecretError]` operation. Because cloning is fallible, the compiler must not insert a hidden secret clone during ordinary ownership planning; code that needs another owned value must call `clone_secret()` explicitly and handle or propagate its result. No bitwise-copy path is allowed. Each successful clone must receive independent owned storage protected under the current key epoch, preserve provenance, report its actual storage-protection mode, and zeroize independently. Tooling and docs must state that cloning creates another secret-bearing allocation. An integrity, poison, or storage failure returns no partial clone.

### Protected storage and memory handling

Every conforming target must provide the portable storage floor: mutable owned secret storage, redacting observation behavior, scoped reveal, and non-elidable zeroization attempts for wrapper-owned storage and reveal buffers. A target may additionally advertise `SecretStorageProtection.Protected` only when it keeps idle payloads under authenticated encryption, obtains root key material from an operating-system cryptographic random source, gives each stored value cryptographically unique encryption context, and keeps key material separate from the ordinary payload allocation. Protected mode must use a reviewed cryptographic library and an authenticated-encryption construction with at least 128-bit security; it must preserve nonce/context uniqueness for every value under a key epoch and record the algorithm version and key epoch only in internal non-payload metadata. Custom cryptographic constructions are not conforming. A target that cannot uphold that contract must report `SecretStorageProtection.Portable`; it must not silently describe redaction or an encrypted buffer next to an unprotected key as protected storage.

A target with a conforming protected backend must use `SecretStorageProtection.Protected` by default. Runtime or deployment policy may require protected storage; when it does, runtime initialization or secret construction must fail closed with `SecretErrorKind.StorageUnavailable` instead of falling back to portable storage. A target without a conforming protected backend may use `Portable` only when policy permits it, and it must select and report that mode before accepting secret values. An integrity failure, memory-locking failure required by policy, or key-rotation failure must never trigger a silent downgrade of an existing secret.

A protected runtime must generate a fresh non-exportable root key for each runtime instance, never persist or serialize it, use host memory-locking or non-pageable facilities when those facilities are available, and zeroize the key on orderly shutdown. Here, non-exportable means that the runtime exposes no public or administrative API that returns or serializes root key material; it does not claim that arbitrary same-process code, a debugger, or a memory compromise cannot read that material. Failure to establish an advertised host memory protection must either fail protected-mode initialization or report `SecretStorageProtection.Portable` before accepting secret values. New values and clones use the current key epoch. Rotation is a runtime-administration operation rather than an ordinary application reveal API; host policy may request it, and the runtime must perform it before exhausting any per-key uniqueness bound. Rotation must install a fresh root key for new values, re-protect every live value before retiring its old epoch, and retain an old key only until no live value or reveal scope depends on it. Values created during rotation use the new epoch; reveal and clone operations must observe one complete payload under one epoch and never an intermediate representation. Failure to re-protect a live value must keep the old epoch active and report rotation failure; it must never drop data or downgrade that value to plaintext. Abrupt process termination remains outside the guaranteed destruction boundary.

Authenticated-ciphertext verification is mandatory before protected payload plaintext becomes observable. On the first verification failure, the operation must not invoke a reveal callback, return a comparison result, conversion, or clone, or expose partial plaintext. It must zeroize any scratch material, atomically mark the affected value poisoned, and return `SecretError` with kind `SecretErrorKind.IntegrityFailure`. Every later payload-reading operation on that value, including reveal, conversion, comparison, cloning, and trusted stdlib use, must return `SecretError` with kind `SecretErrorKind.Poisoned` without another decryption attempt. `source_kind`, `storage_protection`, redacted display/debug, and `redacted(secret)` remain available because they do not inspect the payload. A trusted stdlib boundary must propagate the same redacted failure through its own error channel rather than retry, downgrade storage, or substitute a value.

Plaintext scratch buffers created by `with_exposed_str`, `with_exposed_bytes`, or trusted stdlib internals must be mutable, must not be shared with an ordinary immutable string/bytes allocation, and must be zeroized when the scope ends through normal return, a callback result or error, or an unwind-capable failure. Both `SecretBytes` and `SecretStr` must zeroize their owned mutable storage on drop; `SecretStr` must validate UTF-8 while using the same zeroizable byte-storage contract rather than keeping an unerasable ordinary string as its authority. Ciphertext, per-value key material, and other secret-bearing runtime metadata must also be zeroized when released.

These guarantees do not extend to caller-created copies, allocator remnants beyond the owned allocation, foreign APIs, transport buffers, operating-system buffers, crash dumps, debuggers, compiler temporaries, panic-abort, process kill, power loss, non-unwinding cancellation, or other abrupt termination. The storage-protection mode must be inspectable so callers and policy can distinguish the portable floor from protected idle storage without examining payloads.

Both types must document that redaction is an exposure-control guarantee for standard display, debug, logging, telemetry, diagnostics, and serialization paths. Protected idle storage and zeroization strengthen that guarantee, but they are not full memory-forensics or same-process-compromise guarantees.

The implementation should avoid unnecessary copies in stdlib APIs that consume or forward secret values, especially HTTP authorization helpers, cryptographic helpers, and secret-provider integrations.

### Logging, telemetry, diagnostics, and inspection

`std.logging`, `std.telemetry`, diagnostics, and semantic inspection must treat `SecretStr` and `SecretBytes` as sensitive fields by type.

Structured outputs must preserve the fact that a field exists, its declared type, and the type-derived sensitivity marker. They may include the provenance category and storage-protection mode when policy allows. They must not include raw values, source identifiers, payload-derived values, or protected-storage key metadata.

Tooling must mark explicit reveal operations as searchable and inspectable sites. LSP hover, semantic inspection, and policy checks may use those sites to explain where secret material leaves the protected wrapper.

### HTTP and wire-boundary APIs

`std.http` authorization helpers, header builders, request diagnostics, retry reporting, and telemetry should preserve secret sensitivity. Header values constructed from `SecretStr` or `SecretBytes` must be redacted in debug-facing output even if the header name is not in a built-in sensitive-header list.

`std.http` may expose raw secret material internally when sending a request. That internal exposure must not change the public `Request`, `Response`, `HttpError`, log, telemetry, or action-output redaction contract.

Trusted HTTP header and body encoders may accept typed models containing nested secret fields and serialize those fields directly to the wire. They must not obtain that behavior by making secret types implement the ordinary serialization protocol. The encoder owns the scoped reveal and any transient encoded buffer, erases that buffer on supported cleanup paths, and never exposes it through a reusable JSON value, request inspection API, retry record, or error.

This trusted path must use a sealed compiler- and stdlib-owned wire-encoding contract rather than a public trait that arbitrary code can implement or invoke to obtain plaintext. The typechecker may admit a nested secret field only when the destination parameter is such a trusted wire sink; ordinary `Serialize` bounds and ordinary JSON helpers continue to reject the same model.

### Typed actions, CLIs, and configuration

Typed action inputs, CLI options, and `ctx` fields must be able to declare `SecretStr` and `SecretBytes` directly.

Machine-readable action metadata must distinguish a required secret input from a plain string input. Action output must not include raw secret values. A policy approval can authorize a reveal operation at a trusted boundary, but it does not turn plaintext into safe action metadata or disable downstream redaction.

CLI help may show that an option expects a secret. It must not echo secret defaults or environment-derived values.

Checked model and parameter metadata must infer `sensitive=true` plus `secret_type="str"` or `secret_type="bytes"` from `SecretStr` or `SecretBytes`; an explicit `secret=true` marker is not required to obtain this contract. If a surface also accepts an explicit marker for input handling or external-schema projection, tooling must check it for consistency with the declared type. A marker on plain `str` or `bytes` may request masked input or schema annotation, but it must not claim the runtime redaction, reveal, storage, comparison, or zeroization guarantees of a secret type.

### Generic secret wrappers

This RFC does not expose `Secret[T]`. A follow-up RFC may add a generic wrapper only around a sealed secret-material protocol whose implementation supplies a mutable zeroizable owned representation, a read-only non-escaping reveal view, type-owned redaction and sensitivity metadata, an explicit content-independent comparison contract where comparison is meaningful, and no ordinary data serialization. User-defined implementations must not be able to claim that protocol without proving those invariants through compiler- or runtime-owned machinery. `SecretStr` and `SecretBytes` remain stable concrete types even if a generic wrapper is added later.

### Capability, sandbox, and policy behavior

User-authored `with_exposed_str` and `with_exposed_bytes` calls are explicit reveal operations regardless of whether the runtime is governed. RFC 103 does not depend on Draft RFC 104, reserve a reveal capability identifier, or define a receipt schema. A future governed-runtime RFC may gate these stable reveal sites. If it does, authorization must complete before decryption or plaintext buffer creation, and any audit record must remain redacted: it may describe the operation, source span when available, secret type, policy-permitted provenance category, storage-protection mode, and outcome, but never payload, length, source identifier, or payload-derived material. The future governance RFC owns the capability identity, receipt schema, and decision whether trusted internal reveal receives separate audit representation.

Trusted stdlib APIs that accept a secret directly do not grant general reveal authority. They may expose plaintext only inside their narrow boundary operation, and they must preserve the scoped cleanup and error behavior defined here whether or not governance exists. A future sandbox may deny explicit reveal while still allowing a narrower trusted operation such as an approved HTTP request. Policy approval never changes the redaction or serialization behavior of the secret value itself.

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
- **RFC 070 (`Result` combinators)**: secret reveal reuses the source-level `Callable[...]` and compiler-owned borrowed callback adaptation model, then adds a reveal-specific non-escape rule and synchronous-scope restriction.
- **RFC 072 (`std.logging`)**: structured logging should redact secret-typed fields by default.
- **RFC 078 (typed workflow actions)**: action inputs and outputs should preserve sensitivity metadata so reports can describe secret use without exposing values.
- **RFC 085 (field metadata)**: checked field and parameter metadata derives sensitivity from the secret type; descriptive markers do not become an alternate runtime secrecy authority.
- **RFC 089 (`std.environ`)**: environment access provides only Unicode host values. Secret-returning helpers avoid public Incan plaintext staging, preserve `NotUnicode`, and define `secret_bytes` as direct UTF-8 encoding into secret storage rather than raw platform bytes.
- **RFC 090 (typed CLI framework)**: CLI options can use `SecretStr` and `SecretBytes` as declared types.
- **RFC 093 (`std.telemetry`)**: telemetry attributes and events must redact secret-typed values.
- **RFC 102 (semantic layer inspection surface)**: semantic inspection should represent secret facts as redacted facts with stable type and source metadata.
- **RFC 104 (ambient capabilities and receipts)**: RFC 104 remains Draft, so RFC 103 has no normative capability-ID or receipt-schema dependency. A future governance RFC may gate the stable reveal sites before plaintext creation while preserving this RFC's redaction, cleanup, and trusted-boundary rules.

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
- Scoped reveal requires callback and tooling support that prevents plaintext views from escaping, and callers that deliberately copy plaintext inside a callback leave the wrapper's zeroization contract.

## Implementation architecture

*(Non-normative.)* A practical runtime can represent both concrete types over one zeroizable byte-storage mechanism, validate UTF-8 at `SecretStr` construction and reveal boundaries, and layer protected idle storage over the same interface when the target advertises it. Protected implementations should keep process root keys outside payload allocations, derive distinct per-value encryption context, authenticate ciphertext, and re-protect live values across key epochs. Scoped callbacks should receive views backed by zeroizing scratch storage rather than ordinary owned strings or byte arrays. Stdlib consumers should pass secret wrappers through typed APIs and reveal internally only at the final trusted boundary.

## Layers affected

- **Stdlib / Runtime (`incan_stdlib`)**: must provide `std.secrets`, `SecretStr`, `SecretBytes`, redaction behavior, construction helpers, scoped reveal operations, protected-storage behavior where supported, and integration hooks for stdlib consumers.
- **Typechecker / Symbol resolution / checked metadata**: must preserve the distinct types, reject implicit conversion from secret wrappers to plain `str` or `bytes`, enforce non-escape of the implicitly borrowed callback view, and derive the sensitivity and secret-type facts without runtime payload inspection.
- **Emission**: generated Rust must preserve redacting display/debug behavior, scoped cleanup, and non-elidable zeroization attempts for every wrapper-owned or reveal-scratch allocation.
- **Formatter**: no syntax changes are required, but examples and generated code should preserve readable secret-type annotations.
- **LSP / Tooling**: hover, completion, diagnostics, semantic inspection, action metadata, generated docs, and policy checks should preserve sensitivity metadata and make reveal operations discoverable.
- **Docs / Examples**: environment, CLI, HTTP, logging, telemetry, and workflow examples should demonstrate secret values instead of plain string tokens.

## Design Decisions

- Name the only public reveal methods `with_exposed_str` and `with_exposed_bytes`. They use non-escaping callback views; no public guard or owned extraction method is part of this RFC.
- Keep reveal callbacks synchronous and enforce their implicit borrowed view with a secret-reveal-specific scoped-borrow check based on RFC 070 adaptation; no plaintext-view type enters the public Incan surface.
- Make redaction and zeroization the portable conformance floor. Protected idle storage is a truthful, inspectable target capability, not an assumption or silent fallback.
- Require protected runtimes to use fresh process-local key material, authenticated per-value protection with unique context, non-persistence, best available host memory protection, explicit epoch rotation, and zeroization on orderly release.
- Treat authenticated-storage failure as a fallible payload-access event: the first failure returns an integrity error and atomically poisons the value; later payload reads return a poison error without retry or partial output.
- Omit ordinary equality, ordering, and hashing. Provide only explicit fallible same-type `constant_time_eq`, with a content-independent comparison contract, integrity/poison propagation, and an honest length/host-side-channel caveat.
- Make cloning fallible and explicit through `clone_secret()`; compiler ownership planning must not insert hidden secret clones. Every successful clone receives independent storage and cleanup, and no failed operation yields a partial clone.
- Give `SecretStr` and `SecretBytes` the same mutable zeroizable storage and scoped-plaintext contract. UTF-8 validation does not justify retaining an ordinary unerasable string as `SecretStr`'s storage authority.
- Standardize display as `<secret>` and debug as `SecretStr(<secret>)` or `SecretBytes(<secret>)`. Type-aware structured observation uses a non-payload sensitivity marker rather than a general serializer.
- Make `RedactedSecret` a public, forgeable marker with no trust authority; checked secret types and policy metadata, not marker claims, remain the sensitivity authority.
- Make ordinary data serialization fail for either secret type, including when nested. `std.secrets.redacted(...)` is the explicit stable marker adapter; trusted wire boundaries may accept secret types directly or nested in typed models and own the scoped serialization internally, while user-authored persistence or reusable serialization requires a visible scoped reveal.
- Keep `SecretStr` and `SecretBytes` as the complete concrete surface in this RFC. Any future `Secret[T]` requires a sealed secret-material protocol that preserves zeroization, scoped reveal, redaction, comparison, and serialization invariants.
- Carry an initial non-payload provenance category distinguishing environment, CLI, config, provider, generated, derived, and unknown sources. Do not retain source identifiers by default.
- Align environment ingress with RFC 089: `secret_bytes` is UTF-8 encoding of the Unicode environment value, not a raw platform-byte escape hatch, and ingress remains fallible for both environment and secret-storage errors.
- Guarantee reveal-buffer cleanup on normal, error, and unwind-capable exits only; panic-abort, process kill, power loss, non-unwinding cancellation, and other abrupt termination remain outside the destruction boundary.
- Keep governance integration future-coordinated. Reveal sites are stable tooling facts, but RFC 103 reserves no capability identifier or receipt schema; any future authorization occurs before plaintext creation and never disables redaction.
- Derive checked field and parameter sensitivity metadata from the secret type. An explicit marker can influence ingress or external schema projection but cannot substitute for the runtime guarantees of `SecretStr` or `SecretBytes`.
