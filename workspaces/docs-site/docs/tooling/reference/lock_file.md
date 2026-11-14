# Lock file (`oven.lock`)

`oven.lock` is generated TOML at a root project's directory. Path dependencies do not contribute another lock. `oven lock`, `oven add`, `oven dependency add` and `remove`, `oven update`, `oven interop stage` and the root project's `oven run`, `oven build` and `oven test` write the file atomically. `oven check`, `oven describe-unit` and `oven inspect dependencies` may read it and do not write it.

## Root fields

| Field | Type | Contract |
| --- | --- | --- |
| `format` | Integer | Required. Format 1 is accepted; every other value is refused. |
| `mode` | String | Required. `lowest`, `lowest-direct` and `newest` are accepted. |
| `graph` | String | Required. `sha256:` followed by the resolved root closure's digest. A mismatch is refused and names `oven lock`. |
| `toolchain` | Table | Required. The selected Incan version and compiler identity. |
| `loaf` | Array of tables | Required. Exactly one entry has `origin = "root"`; every other entry is reachable from it. |
| `interop` | Array of tables | Present when the root project or a local path dependency declares [`[[interop.c.targets]]`](project_configuration.md#interopc). One entry per declared target; see [C interop entries](#c-interop-entries). |

Unknown fields are refused.

## Toolchain table

`toolchain.version` is required. `toolchain.identity` records the content identity of the compiler, Rust fact caches, linked native units, target and selected SDK facts. `--locked` refuses a different version or identity. An ordinary lock refresh under a different toolchain keeps the recorded mode and retains every exact producer selection that still satisfies the graph.

## Loaf entries

Every `[[loaf]]` requires `name`, `version`, `origin` and `dependencies`. `origin` is one of `root`, `path`, `registry` or `toolchain`.

| Field | Type | Contract |
| --- | --- | --- |
| `path` | String | A path-origin Loaf's path relative to the lock directory. Generated paths are never absolute. |
| `registry` | String | Registry identity. Until a registry has a content-derived id, its configured name is recorded. |
| `protocol` | String | Registry provider protocol. Reserved in format 1. |
| `revision` | Integer | Exact registry record revision. Zero when absent. |
| `checksum` | String | Immutable registry source digest. A toolchain-origin Loaf records the source digest the toolchain's registry closure records for its unit, and none when the closure records none. |
| `source` | String | Immutable registry source reference. |
| `trust` | String | Trust outcome reached for the selected content. |
| `adopted` | Boolean | Whether the registry version was adopted locally. False when absent. |
| `entry` | String | Exact compiled producer identity for a toolchain-origin Loaf. |
| `dependencies` | Array of inline tables | Exact outgoing edges. Empty when the entry has no dependencies. |

An edge requires `key`, `loaf` and `version`. `domain` defaults to `target`; `target` records a target predicate or triple; `entry` disambiguates a compiled producer. Two entries may have the same Loaf name when their exact versions differ. A refresh keeps each recorded producer that still satisfies its requirement. A requirement that no recorded version of its Loaf satisfies is resolved as a fresh lock would resolve it, and its version is locked beside the recorded ones; another producer of a recorded version is refused. A `name` or edge `loaf` with a capital letter is refused, never folded. An edge must resolve to exactly one entry in its domain.

Each `[[loaf.features]]` table requires `target` and `domain`. `requested`, `enabled` and `predicates` are arrays of strings and default to empty. Host and target feature closures are separate entries.

## C interop entries

Each `[[interop]]` table records one declared C interop target contract, ordered by package and then target.

| Field | Type | Contract |
| --- | --- | --- |
| `package` | String | The local path dependency that declares the target. Omitted for the root project's targets. |
| `target` | String | Required. The declared target triple. |
| `requirements` | String | Required. The target's requirements as one line of `; `-separated facts, in this order: `toolchain=CAPABILITY VERSION`, `sdk=CAPABILITY VERSION`, `platform=KIND API-LEVEL DEPLOYMENT-TARGET`, one `definition=DEFINITION` per definition, one `needs=ARTIFACT:DEPENDENCY,...:MINIMUM-PLATFORM` per artifact and one `binding=MODULE.NAME:ARTIFACT,...` per binding relation. |
| `headers` | Array of strings | One `PATH sha256:DIGEST` per declared header, in ascending path order. |
| `artifact` | Array of tables | One `[[interop.artifact]]` table per declared artifact, in declaration order. |

Each `[[interop.artifact]]` table requires `name` and `kind`, which is `static`, `bundled` or `system`. A field without a value is omitted.

| Field | Type | Contract |
| --- | --- | --- |
| `path` | String | A `static` or `bundled` artifact's file, relative to the declaring project. |
| `digest` | String | `sha256:` followed by the SHA-256 digest of that file's bytes, or `missing` when the file is absent. |
| `runtime-name`, `placement` | Strings | A `bundled` artifact's declared runtime name and placement. |
| `capability` | String | A `system` artifact's toolchain or SDK capability. |
| `origin` | Inline table | The artifact's declared origin: `source`, `revision` and `license`, all required. |

A digest of a header or artifact file, as of every other interop fact, is part of `graph`. With `--locked`, a changed contract is refused, naming ``the C interop contract of `TARGET` `` or, for an artifact, ``KIND artifact `NAME` of `TARGET` ``, followed by `` in `PACKAGE` `` for a path dependency's target. Unknown fields in `[[interop]]`, `[[interop.artifact]]` and `origin` are refused.

## Graph identity

Each Loaf digest covers length-prefixed values for its origin, name, version, origin fields, feature closures and toolchain identity when applicable. Its dependency edges are ordered by key, domain, target, Loaf, version and compiled entry. Each edge is followed by the digest of the entry it reaches. Without `[[interop]]` entries, `graph` is the root entry's digest. With them, `graph` is the digest of the root entry's digest and the interop entries' facts, each length-prefixed.

Reordering `[[loaf]]` tables, dependency edges or feature names does not change `graph`. Changing a semantic field, an exact edge or any reached entry changes `graph`. Cycles, unresolved or ambiguous edges, multiple roots and entries outside the root closure are refused.

## Accepted example

```toml
# Generated by Oven. Do not edit.
format = 1
mode = "lowest"
graph = "sha256:0123456789abcdef"

[toolchain]
version = "0.6.0"
identity = "sha256:compiler"

[[loaf]]
name = "app"
version = "0.1.0"
origin = "root"
dependencies = [
  { key = "pricing", loaf = "pricing", version = "0.2.0" },
]

[[loaf.features]]
target = "aarch64-apple-darwin"
domain = "target"
requested = []
enabled = []
predicates = []

[[loaf]]
name = "pricing"
version = "0.2.0"
origin = "path"
path = "../pricing"
dependencies = []

[[loaf.features]]
target = "aarch64-apple-darwin"
domain = "target"
requested = []
enabled = []
predicates = []
```

Accepted only when `graph` equals the digest computed from the complete entries.

## Refused examples

```toml
format = 2
```

Refused: unsupported format.

```toml
format = 1
mode = "fastest"
```

Refused: unsupported version-selection mode.

```toml
format = 1
mode = "lowest"
graph = "sha256:edited"
```

Refused: the stored graph identity does not authenticate the entries.
