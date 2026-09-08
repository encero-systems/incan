# Native source-unit definition

`native/source-unit.json` is physical metadata inside a generated library artifact. Both source-only and native library publication write it. Its presence describes emitted Rust source and retained dependency requests; it does not establish a selected Rust dependency closure or native readiness.

The file is covered by the containing artifact's physical digest and, when a package Loaf handoff exists, its sealed metadata-file list. It contains no digest of its own containing artifact. It is separate from the [package executable representation](package_executable_representation.md).

## Schema 1

Fields below are required except `package` and `version_requirement` inside requirement objects, which may be omitted and decode as absent. Unknown fields and unsupported variants are rejected. Writers encode absent optional values as JSON `null`. Readers reject documents larger than 4 MiB and inspect `schema_version` before interpreting version-specific fields.

| Field | Type | Meaning |
| --- | --- | --- |
| `schema_version` | Integer | Exactly `1`. |
| `package` | Object with string `name` and `version` | Identity of the containing checked manifest. |
| `crate_name` | String | Actual emitted Rust target name. |
| `crate_kind` | `"rlib"` or `"proc_macro"` | Library operation recorded by its producer. Incan's library publisher records `"rlib"`. |
| `edition` | `"2015"`, `"2018"`, `"2021"` or `"2024"` | Producer-selected Rust edition. Incan library publication uses `build.rust_edition`, defaulting to `"2024"`. |
| `authored_source_digest` | SHA-256 string | Existing checked authored Incan/manifest identity. It does not cover unresolved Rust source dependencies. |
| `entrypoint` | Object with `path` and `digest` | Exactly `src/lib.rs` and its file digest. |
| `source_tree` | Object with `path` and `digest` | Exactly `src` and its generated-tree digest. |
| `requirements` | Array of requirement objects | Normal requirements first, then development requirements; aliases are sorted within each role, with unique `(role, alias)` keys. Normal and development requirements remain distinct. |

Digests use `sha256:` followed by 64 lowercase hexadecimal digits. File digests hash the file bytes. The tree digest hashes the compact JSON object mapping sorted portable relative file paths to file digests. Every regular file is included; no generated-tree exclusions apply. Empty trees, symlinks and special files are rejected. Directory names affect paths, but empty directories have no tree-digest entry. The complete artifact digest separately covers physical directories.

## Requirement object

| Field | Type | Meaning |
| --- | --- | --- |
| `role` | `"normal"` or `"dev"` | Original requirement role. |
| `alias` | String | Rust dependency alias. |
| `package` | String or `null` | Explicit package rename. |
| `version_requirement` | String or `null` | Retained version requirement, not an invented selected version. |
| `features` | Array of strings | Sorted, unique requested Rust features. |
| `default_features` | Boolean | Whether the request enables default Rust features. |
| `optional` | Boolean | Whether the request is optional. |
| `source` | Tagged object below | Existing provider-edge reference or unresolved source request. |

### Source variants

| `source.kind` | Additional fields | Contract |
| --- | --- | --- |
| `provider_edge` | `edge_kind`: `"public_package"` or `"private_implementation"`; `dependency_key`: string | References one exact edge in the containing checked manifest. The key equals `alias`; the effective requested package equals that edge's provider name. Selected artifact identity and public feature projection remain in the checked edge. Public provider features and requested Rust features are distinct. |
| `registry_request` | None | Registry request with the requirement's version/features; no selected version, checksum or source digest. |
| `git_request` | `url`: string; `reference`: object with `kind` (`"branch"`, `"tag"`, `"rev"`) and string `value` | Declared Git request; a reference string does not admit a checkout. |
| `authored_path_request` | `anchor`: `"authoring_project"`; `path`: relative string | Request relative to the original authoring project. It is not artifact-relative and grants no permission to reopen the project. Parent components are permitted only in this declarative request. |
| `unbound_path_request` | `request_key`: `"<role>:<alias>"`; `reason`: `"portable_source_binding_unavailable"` | Preserves a requirement whose effective path lacks a portable authored coordinate or checked provider edge. Absolute publisher paths are not serialized. A later operation must obtain the missing selected-source evidence. |

The current Incan producer has effective path coordinates but no retained distinction between an originally relative and absolute path spelling. It therefore emits `unbound_path_request` for path requirements without a checked provider edge, including paths inside the authoring project. `authored_path_request` is reserved for a producer that retains that original coordinate evidence.

## Validation and absence

Package name, package version and authored-source digest must match the containing checked manifest. Every provider reference must identify one matching edge. Generated source validation checks the explicitly named file and tree under the artifact root; it never reads unresolved dependency sources. An equal-byte relocated artifact can satisfy these checks without retaining the original publisher's checkout path.

A missing sidecar remains distinguishable from a malformed present sidecar. Existing native-only use of older artifacts may omit it; operations that require a source definition must establish that requirement separately. Unsupported versions, malformed JSON, inconsistent bindings, source mutation and indirect source paths are errors. Successful decoding or source validation is not native admission.

Publication participates in the library's existing ordinary-error rollback: a failed rebuild restores the preceding artifact and publication receipts. The output can be temporarily unavailable during a rebuild; this is not a concurrent-reader or process-crash atomicity guarantee.
