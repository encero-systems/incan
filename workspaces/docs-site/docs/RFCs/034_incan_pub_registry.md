# RFC 034: `incan.pub` — The Incan Package Registry

- **Status:** Draft
- **Created:** 2026-03-06
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:** RFC 031 (library system phase 1), RFC 027 (incan-vocab)
- **Target version:** TBD

## Summary

Define the `incan.pub` package registry: the infrastructure, protocols, and CLI commands that allow Incan library authors to publish packages and consumers to resolve them. The registry is an Incan web service backed by S3-compatible European object storage, with SHA256 integrity verification and Sigstore-based package signing. It costs €0 at launch and scales predictably with hard spending caps.

## Constraints

Two non-negotiable requirements drive every decision in this RFC:

1. **EU infrastructure.** The Incan project is partially funded by an EU grant. All data storage and compute must be hosted by EU-based providers. This rules out US-headquartered cloud providers (AWS, GCP, Azure, Cloudflare) as primary infrastructure. EU-based CDN is acceptable for edge caching.

2. **Cost ceiling.** The registry must not produce surprise bills. At every scale tier the monthly cost must be predictable and capped. The project cannot absorb a €1,000+/month bill from sudden popularity — cost must grow slowly and linearly, with hard limits enforced via provider spending caps and architectural choices (CDN offloading, bandwidth caps, immutable caching).

## Motivation

RFC 031 introduces Incan libraries with local `path` dependencies. That's enough for monorepos and co-located projects, but the ecosystem needs a way to **publish and discover** shared packages. Without a registry:

- Library authors have no distribution channel beyond "clone my repo."
- Consumers cannot express versioned dependencies (`mylib = "0.1.0"`).
- There is no central discovery point for the Incan ecosystem.
- Supply chain security has no foundation (no checksums, no signatures, no audit trail).

The `incan.pub` registry closes this gap. It is the canonical source for published Incan packages, parallel to crates.io for Rust and PyPI for Python.

## Guide-level explanation

### Publishing a package

```bash
# One-time setup: create an account and save credentials
$ incan login
  Opening https://incan.pub/tokens in your browser...
  Paste your API token: ****
  Saved to ~/.incan/credentials

# Publish from a library project
$ incan publish
  Building library...
  Packaging mylib 0.1.0...
  Signing with Sigstore (GitHub: @dannymeijer)...
  Uploading to incan.pub...
  Published mylib 0.1.0
```

### Consuming a published package

```toml
# my-app/incan.toml
[project]
name = "my-app"
version = "0.1.0"

[dependencies]
mylib = "0.1.0"
```

```incan
# src/main.incn
from pub::mylib import Widget

def main():
    w = Widget(title="hello")
    print(w)
```

```bash
$ incan build
  Resolving dependencies...
    mylib 0.1.0 (incan.pub)
  Downloading mylib 0.1.0...
  Verifying checksum... ok
  Verifying signature... ok (signed by @dannymeijer via Sigstore)
  Compiling my-app...
  Done.
```

### Yanking a version

```bash
$ incan yank mylib 0.1.0
  Yanked mylib 0.1.0 — existing lockfiles still resolve, new resolves skip it.
```

### What the user sees on incan.pub

`incan.pub` serves a static web page (like `lib.rs` or `pypi.org`) showing:

- Package name, description, version history
- Author, license, repository link
- Signature status and signer identity
- Download counts
- Dependency tree

This is a static site generated from the index — no dynamic server needed for the web UI.

## Reference-level explanation

### Architecture overview

```text
             ┌──────────────────────────────────┐
             │         incan.pub                │
             │         (DNS)                    │
             └────────┬──────────┬──────────────┘
                      │          │
             GET /crates/*       │  POST /api/v1/*
             GET /index/*        │  (publish, yank, login)
                      │          │
                      ▼          ▼
             ┌────────────────────────────────────┐
             │  CDN Layer (Bunny.net — Slovenia)  │
             │  Caches all GET requests at edge   │
             │  Hard bandwidth cap configurable   │
             │  EU PoPs only (or worldwide later) │
             └────────┬───────────────────────────┘
                      │ cache miss
                      ▼
             ┌────────────────────────────────────┐
             │  Registry Service                  │
             │  (Incan binary on Scaleway         │
             │   Serverless Containers — Paris)   │
             │                                    │
             │  Handles:                          │
             │  - POST /api/v1/publish            │
             │  - POST /api/v1/yank               │
             │  - POST /api/v1/login              │
             │  - GET fallthrough (cache miss)    │
             │  Scales to zero when idle          │
             └────────┬───────────────────────────┘
                      │
                      ▼
             ┌────────────────────────────────────┐
             │  Scaleway Object Storage (Paris)   │
             │  S3-compatible                     │
             │  75 GB free tier                   │
             │                                    │
             │  Bucket layout:                    │
             │    crates/<name>/<ver>.crate       │
             │    crates/<name>/<ver>.crate.sig   │
             │    crates/<name>/<ver>.crate.cert  │
             │    index/<prefix>/<name>           │
             └────────────────────────────────────┘
```

### The package format

A `.crate` file is a gzipped tarball containing the Rust crate output from `incan build --lib` plus the `.incnlib` type manifest:

```text
mylib-0.1.0.crate (tar.gz):
└── mylib-0.1.0/
    ├── Cargo.toml              # Generated Rust crate metadata
    ├── src/
    │   ├── lib.rs              # Generated Rust source
    │   └── widgets.rs
    └── .incnlib                # Type manifest (JSON, from RFC 031)
```

The `.incnlib` file is invisible to Cargo (which ignores unknown files in the tarball). The `incan` CLI extracts it for typechecking; `cargo build` only sees the Rust source.

This is a single artifact — the type manifest and compiled Rust source are never stored or transferred separately. This simplifies every part of the pipeline: publish uploads one file, download retrieves one file, cache stores one file.

### Index format

The registry uses a **sparse index** format inspired by Cargo's sparse registry protocol. Each package has one file in the index, containing one JSON line per published version:

```text
index/my/li/mylib
```

```json
{"name":"mylib","vers":"0.1.0","cksum":"sha256:e3b0c442...","deps":[],"incan_version":">=0.2.0","yanked":false,"publisher":"dannymeijer","signatures":{"keyid":"sigstore-oidc","sig":"MEUC...","cert":"MIIB..."}}
{"name":"mylib","vers":"0.2.0","cksum":"sha256:a1b2c3d4...","deps":[{"name":"widgets","req":"^0.1"}],"incan_version":">=0.2.0","yanked":false,"publisher":"dannymeijer","signatures":{"keyid":"sigstore-oidc","sig":"MEYCIQDx...","cert":"MIIB..."}}
```

**Index entry fields:**

| Field | Type | Description |
|---|---|---|
| `name` | string | Package name |
| `vers` | string | SemVer version |
| `cksum` | string | SHA256 of the `.crate` tarball (prefixed with `sha256:`) |
| `deps` | array | Incan library dependencies (`name` + `req` version range) |
| `rust_deps` | array | Rust crate dependencies (merged into consumer's Cargo.toml) |
| `incan_version` | string | Minimum compiler version required |
| `yanked` | bool | If true, existing lockfiles still resolve but new resolves skip |
| `publisher` | string | Publisher identity (username) |
| `signatures` | object \| null | Sigstore signature + certificate, or null if unsigned |

**Index path convention** (matches crates.io):

| Package name length | Path |
|---|---|
| 1 character | `index/1/<name>` |
| 2 characters | `index/2/<name>` |
| 3 characters | `index/3/<first char>/<name>` |
| 4+ characters | `index/<first two>/<next two>/<name>` |

### Registry API

The registry service exposes a small HTTP API. All mutating endpoints require authentication.

#### `POST /api/v1/publish`

```text
Headers:
  Authorization: Bearer <token>
  Content-Type: application/octet-stream
  X-Package-Name: mylib
  X-Package-Version: 0.1.0
  X-Checksum: sha256:e3b0c442...
  X-Signature: MEUC... (base64, optional in Phase 1)
  X-Certificate: MIIB... (base64, optional in Phase 1)

Body: .crate tarball (binary)
```

**Server-side validation:**

1. Verify token → resolve publisher identity
2. Verify publisher owns this package name, or name is unclaimed
3. Verify `(name, version)` does not already exist → 409 Conflict
4. Verify `X-Checksum` matches SHA256 of request body
5. If signature provided: verify Sigstore signature is valid, signer matches publisher
6. Extract `.incnlib` from tarball → verify it parses (basic structural validation)
7. Store `.crate` in object storage: `crates/<name>/<version>.crate`
8. Store signature artifacts: `crates/<name>/<version>.crate.sig`, `.cert`
9. Update index: append version line to `index/<prefix>/<name>`
10. Invalidate CDN cache for the index entry
11. Return 200

**Response:** `{ "published": "mylib", "version": "0.1.0" }`

#### `POST /api/v1/yank`

```text
Headers:
  Authorization: Bearer <token>
Body: { "name": "mylib", "version": "0.1.0" }
```

Sets `yanked: true` in the index entry. Does not delete the `.crate` file (existing lockfiles and builds that reference this exact version still work).

#### `GET /index/<prefix>/<name>`

Returns the JSON-lines index file for the named package. Served from object storage, cached at CDN edge.

#### `GET /crates/<name>/<version>.crate`

Returns the `.crate` tarball. Served from object storage, cached at CDN edge. Immutable forever — cache headers set to maximum TTL.

### Authentication

#### Token-based auth

Publishers authenticate with API tokens. Tokens are generated via the `incan.pub` web UI (or a future `incan token create` CLI command).

Token storage:

- **Server side:** Scaleway Serverless Database (PostgreSQL, free tier: 1 GB) or a simple JSON file in object storage (sufficient for early scale). Maps token hash → publisher identity + package ownership list.
- **Client side:** `~/.incan/credentials` (file permissions 0600). Format: `[registry]\ntoken = "incan_tok_..."`.

```bash
$ incan login
  Opening https://incan.pub/tokens in your browser...
  Paste your API token: ****
  Saved to ~/.incan/credentials
```

#### Package ownership

- First publish of a name → publisher becomes owner
- Owners can add co-owners via `incan owner add <username> <package>`
- Only owners can publish new versions or yank existing ones
- Reserved names: `std`, `incan`, `core`, `test` — cannot be claimed by users

### Package signing with Sigstore

Every `incan publish` signs the `.crate` tarball using [Sigstore](https://sigstore.dev) keyless signing:

**Publish side:**

1. `incan publish` initiates an OIDC flow (opens browser → GitHub/GitLab/Google login)
2. Sigstore's Fulcio CA issues a short-lived signing certificate tied to the OIDC identity
3. The `.crate` file's SHA256 digest is signed with the ephemeral private key
4. The signature + certificate + checksum are recorded in Sigstore's Rekor transparency log
5. The signature and certificate are sent to the registry alongside the `.crate`

**Verification side (`incan build`):**

1. Download `.crate` + `.sig` + `.cert` from registry
2. Verify SHA256 of `.crate` matches the index checksum
3. Verify the certificate was issued by Sigstore Fulcio CA
4. Verify the signature matches the `.crate` digest
5. Verify the signer identity in the certificate matches the `publisher` field in the index
6. Verify the signature is recorded in Sigstore Rekor (transparency log lookup)

If verification fails, `incan build` refuses to use the package and emits a clear diagnostic.

**Sigstore is optional in Phase 1** — the `signatures` field in the index is nullable. Unsigned packages are accepted but display a warning during `incan build`. The goal is to make signing the default from early on, then make it mandatory once the tooling is proven.

**Rust integration:** the `sigstore-rs` crate provides the Sigstore client libraries. The registry service itself verifies signatures on publish (to reject invalid signatures early).

### Security properties

| Threat | Mitigation |
|---|---|
| Unauthorized publish | API token required; validated server-side |
| Package tampering (in transit) | SHA256 checksum verified client-side after download |
| Package tampering (at rest / registry compromise) | Sigstore signature is independently verifiable; transparency log is external |
| Token theft | Sigstore OIDC ties signature to real identity, not just a token; stolen token can publish but signature won't match |
| Version overwrite | Server rejects duplicate `(name, version)` — immutable once published |
| Name squatting | Reserved names enforced; future: similarity checks |
| Dependency confusion | `pub::` import prefix makes origin unambiguous (RFC 031) |
| Supply chain audit | Every publish recorded in Sigstore Rekor transparency log — public, append-only, external |

### Consumer resolution flow

When `incan build` encounters a registry dependency:

```text
[dependencies]
mylib = "0.1.0"          # exact version
mylib = "^0.1"           # SemVer-compatible range
mylib = { version = "0.1.0", registry = "incan.pub" }  # explicit (default)
```

Resolution:

1. Read `[dependencies]` from `incan.toml`
2. For each registry dep: `GET https://incan.pub/index/<prefix>/<name>`
3. Parse JSON lines, filter by version requirement, select newest matching non-yanked version
4. Check local cache `~/.incan/libs/<name>-<version>/` — if cached and checksum matches, skip download
5. `GET https://incan.pub/crates/<name>/<version>.crate`
6. Verify SHA256 checksum matches index entry
7. Verify Sigstore signature (if present; warn if absent)
8. Extract to `~/.incan/libs/<name>-<version>/`
9. Load `.incnlib` into typechecker symbol table
10. Wire Rust crate as path dependency in generated `Cargo.toml`

**Lockfile (`incan.lock`):** on first resolution, write resolved versions + checksums to `incan.lock`. Subsequent builds use the lockfile for reproducibility. `incan update` re-resolves.

### CLI commands

| Command | Description |
|---|---|
| `incan add <pkg>` | Add a dependency to `incan.toml` (fetch latest version from registry) |
| `incan remove <pkg>` | Remove a dependency from `incan.toml` |
| `incan update` | Re-resolve all dependencies and update `incan.lock` |
| `incan login` | Authenticate with `incan.pub`, save token to `~/.incan/credentials` |
| `incan publish` | Build library, package `.crate`, sign, upload to registry |
| `incan yank <pkg> <ver>` | Mark a version as yanked (still downloadable but skipped in new resolves) |
| `incan search <query>` | Search the registry index (client-side text search over cached index) |
| `incan owner add <user> <pkg>` | Add a co-owner for a package |
| `incan owner list <pkg>` | List owners of a package |

#### `incan add` in detail

Like `cargo add`, this is the primary way users add dependencies. It edits `incan.toml` for you:

```bash
# Add latest version from incan.pub
$ incan add widgets
  Added widgets = "^0.2.1" to [dependencies]

# Add a specific version
$ incan add widgets@0.1.0
  Added widgets = "0.1.0" to [dependencies]

# Add a Rust crate (to [rust-dependencies])
$ incan add --rust serde
  Added serde = "1.0" to [rust-dependencies]

# Add a path dependency (local library)
$ incan add widgets --path ../widgets
  Added widgets = { path = "../widgets" } to [dependencies]

# Add a git dependency (Phase 2)
$ incan add widgets --git https://github.com/example/widgets --tag v0.2.0
  Added widgets = { git = "https://...", tag = "v0.2.0" } to [dependencies]
```

**Behavior:**

1. If no `incan.toml` exists, error with "run `incan init` first"
2. Query the registry index for the latest non-yanked version (unless `@version` or `--path`/`--git` specified)
3. Default to `^major.minor.patch` range (SemVer-compatible, like Cargo)
4. Write the entry to `[dependencies]` (or `[rust-dependencies]` with `--rust`)
5. If the package is already in `incan.toml`, update the version (with a confirmation prompt unless `--force`)
6. Run `incan lock` to update `incan.lock` with the resolved version

`incan remove` does the inverse: removes the entry from `incan.toml` and re-locks.

## Infrastructure: provider selection

### Requirements

| Requirement | Rationale |
|---|---|
| EU-headquartered provider | EU grant compliance; GDPR-native |
| Predictable cost with hard caps | Project cannot absorb surprise bills from scaling |
| S3-compatible object storage | Standard tooling; provider-portable |
| Scale-to-zero compute | No cost when idle; only pay for publishes |
| CDN with configurable bandwidth cap | Read traffic offloaded to edge; cap prevents bill shock |

### Selected providers

| Component | Provider | Country | Why |
|---|---|---|---|
| **Object storage** | Scaleway Object Storage | France | S3-compatible, 75 GB free, €0.01/GB beyond, EU data residency |
| **Compute** | Scaleway Serverless Containers | France | Scales to zero, 400K vCPU-s/month free, deploy Incan binary as container |
| **CDN** | Bunny.net | Slovenia | EU-based, €0.005-0.01/GB, configurable monthly bandwidth cap, EU-only PoPs option |
| **DNS** | Scaleway or registrar | EU | Point `incan.pub` at CDN |

### Cost projections with hard caps

| Stage | Packages | Downloads/month | Storage | CDN bandwidth | Compute | **Total** | **Cap** |
|---|---|---|---|---|---|---|---|
| Launch | <100 | <1K | €0 (free tier) | €0 (<1 GB) | €0 (free tier) | **€0** | €5 cap on Scaleway |
| Growing | ~1K | ~50K | ~€1 | ~€5 | €0 | **~€6** | €20 cap |
| Traction | ~5K | ~500K | ~€5 | ~€25 | ~€5 | **~€35** | €75 cap |
| Large | ~50K | ~5M | ~€50 | ~€250 | ~€15 | **~€315** | €500 cap |

**How caps work:**

- **Scaleway:** billing alerts + hard spending limits per project. Set a monthly budget; services are suspended (not billed) when exceeded.
- **Bunny.net:** configurable monthly bandwidth limit per pull zone. When reached, traffic returns 503 (or falls through to origin with a smaller rate limit). Set to the cap column value; increase manually as growth justifies it.

**The worst case** of "sudden PyPI-scale fame" is: CDN hits its bandwidth cap, new downloads get 503, existing cached packages keep working. You notice, evaluate whether to raise the cap, and decide. You never wake up to a €10K bill.

### Provider portability

The registry service talks to object storage via the S3 API (`rust::aws_sdk_s3`). Configuration is environment variables:

```text
S3_ENDPOINT=https://s3.fr-par.scw.cloud
S3_BUCKET=incan-pub
S3_REGION=fr-par
S3_ACCESS_KEY=...
S3_SECRET_KEY=...
```

Switching providers means changing these variables and running an `rclone sync` to migrate existing objects. Zero code changes.

### Why not self-hosted Kellnr?

Kellnr is a self-hosted Rust crate registry that implements the Cargo registry protocol. It was considered and rejected because:

- It only speaks the Cargo registry protocol — no awareness of `.incnlib` manifests
- Requires a persistent server (no scale-to-zero)
- Written in Rust, not Incan (misses the dogfooding opportunity)
- The `.incnlib`-in-`.crate` trick makes Cargo protocol compatibility free anyway — any tool that can download a `.crate` gets both the Rust source and the type manifest

## The registry service: written in Incan

The `incan.pub` registry API is itself an Incan project. This is deliberate:

1. **Dogfooding.** The registry is the first production Incan web service. If `std.web` can't handle it, that's a bug we need to find.
2. **Marketing.** "Our package registry is written in Incan" demonstrates the language's production readiness.
3. **Simplicity.** Incan compiles to a native Rust binary — no runtime dependencies, no container base image beyond `scratch`.

```incan
# registry/src/main.incn
import std.web
from std.web import App, Request, Response, route, status
from handlers import publish, yank, get_index, download_crate
from middleware import require_auth, log_request

app = App()
app.use(log_request)

@route("POST", "/api/v1/publish")
@require_auth
async def handle_publish(req: Request) -> Response:
    return await publish(req)

@route("POST", "/api/v1/yank")
@require_auth
async def handle_yank(req: Request) -> Response:
    return await yank(req)

@route("GET", "/index/{prefix}/{name}")
async def handle_index(req: Request) -> Response:
    return await get_index(req)

@route("GET", "/crates/{name}/{version}.crate")
async def handle_download(req: Request) -> Response:
    return await download_crate(req)

app.run(host="0.0.0.0", port=8080)
```

Deployed as a Docker container on Scaleway Serverless Containers. The container image is minimal (~10 MB): the compiled binary plus TLS root certificates.

## Interaction with existing features

- **RFC 031 (library system):** This RFC builds directly on RFC 031. The `.incnlib` manifest format, `pub::` import syntax, and `incan build --lib` command are defined there. This RFC adds the distribution layer on top.
- **RFC 027 (incan-vocab):** Library soft keyword declarations are serialized into the `.incnlib` manifest during `incan build --lib` and included in the `.crate` tarball. The registry is unaware of soft keywords — it just stores and serves packages.
- **`rust::` imports (RFC 005):** `pub::` registry imports and `rust::` Rust crate imports coexist. A package's Rust dependencies (from its generated `Cargo.toml`) are listed in the index entry's `rust_deps` field.

## Alternatives considered

### Cloudflare Workers + R2

US-headquartered, disqualified by the EU infrastructure requirement. Cloudflare's EU-only deployment options exist but the company remains US-based.

### GitHub Releases as package storage

Free but ties the ecosystem to a US platform. Also lacks integrity guarantees — release assets can be silently re-uploaded.

### Self-hosted Kellnr

See "Why not self-hosted Kellnr?" above.

### Static git repository (no compute)

A git repo deployed as a static site (like `incan.io`). Works for reads but cannot validate publishes server-side — anyone with push access can publish anything. No auth, no ownership, no validation. Acceptable for first-party-only use but not for a community registry.

### Hetzner VPS + Object Storage

German provider, good pricing. However: no scale-to-zero compute (minimum €4.51/month even when idle), no serverless containers. Viable as a fallback but Scaleway's free tier and serverless model are a better fit for the early stage.

## Drawbacks

- **Complexity.** A package registry is a significant piece of infrastructure to build and maintain, even a simple one.
- **Dependency on Scaleway/Bunny.net.** The architecture is provider-portable (S3 API + HTTP CDN), but the initial deployment is tied to these specific providers.
- **Sigstore learning curve.** Keyless signing via OIDC is unfamiliar to many developers. Clear documentation and good CLI UX can mitigate this.
- **`std.web` dependency.** Writing the registry in Incan means `std.web` (RFC 023) must ship first. If `std.web` is delayed, the registry could be written in Rust as a temporary measure and rewritten in Incan later.

## Implementation plan

### Phase 1: MVP registry (alongside RFC 031 implementation)

| Component | What | Estimated size |
|---|---|---|
| Registry service (Incan) | HTTP API: publish, yank, index, download | ~300 lines |
| S3 storage layer | Object storage client for crate + index read/write | ~150 lines |
| `incan login` CLI command | Token prompt, save to `~/.incan/credentials` | ~50 lines |
| `incan publish` CLI command | Build lib, package `.crate`, upload with checksum | ~150 lines |
| Index reader (in compiler) | Fetch + parse sparse index, resolve versions | ~200 lines |
| Cache manager | `~/.incan/libs/` cache with checksum verification | ~100 lines |
| Lockfile (`incan.lock`) | Write/read resolved dependency versions | ~150 lines |
| Scaleway + Bunny.net setup | Infra provisioning (one-time) | ~2 hours |

### Phase 2: Sigstore signing

| Component | What | Estimated size |
|---|---|---|
| Signing in `incan publish` | OIDC flow + Sigstore signing via `sigstore-rs` | ~80 lines |
| Verification in `incan build` | Signature + certificate + Rekor verification | ~100 lines |
| Registry-side validation | Verify signature on publish before storing | ~50 lines |

### Phase 3: Web UI + search

| Component | What |
|---|---|
| Static site generator | Generate HTML from index data (package pages, search) |
| `incan search` CLI | Client-side search over cached index |
| Download counters | Increment counter in object storage metadata on each download |

## Unresolved questions

1. **Token management UX.** Should tokens be scoped (per-package, read-only, full-access)? Scoped tokens reduce blast radius of token theft but add complexity. Start with full-access tokens and add scoping later?

2. **Version resolution strategy.** Cargo uses SemVer with maximal version resolution. Should Incan do the same, or default to minimal version resolution (Cargo's `-Z minimal-versions`) for reproducibility? Maximal is more familiar; minimal is safer.

3. **Namespace / scope model.** Should packages be flat (`mylib`) or scoped (`@danny/mylib`)? Flat is simpler; scoped prevents name conflicts as the ecosystem grows. PyPI is flat; npm is scoped. Cargo is flat with a `foo` / `foo-bar` convention.

4. **CDN cache invalidation on publish.** When a new version is published, the index entry must be updated at the CDN edge. Bunny.net supports API-based purge — but should the registry service call it synchronously (slower publish, instant availability) or asynchronously (fast publish, ~60s propagation delay)?

5. **Fallback when CDN cap is reached.** When Bunny.net hits its bandwidth cap, should the client fall back to direct origin requests (slower but functional) or fail with a clear "registry bandwidth limit reached" error? Fallback is better UX but could overload the origin.

## Future extensions (out of scope)

- **Private registries.** Organizations running their own `incan.pub`-compatible registry for internal packages. Uses the same protocol and CLI commands with a different registry URL in `incan.toml`.
- **Trusted Publishers (GitHub Actions OIDC).** Like PyPI's Trusted Publishers — CI/CD can publish without long-lived tokens by using GitHub Actions' OIDC identity directly with Sigstore.
- **Package auditing tools.** `incan audit` to check dependencies against a vulnerability database.
- **Mirror support.** Read-only mirrors of `incan.pub` for network-restricted environments or regional performance.
