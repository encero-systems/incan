# Package executable representation

A built Incan library can publish checked executable content for its public declarations, beside its compiled library. `incan inspect representation` reports its coverage.

## Artifact layout

The `.incnlib` manifest selects one immutable binary file beside it:

```text
target/lib/
├── <package>.incnlib
└── semantic/
    └── <content_digest>.incnsem
```

This is the semantic portion of the library artifact; native outputs and other generated files also belong to that artifact. Normal Oven materialization retains the selected semantic file. Copying only the `.incnlib` manifest is insufficient.

The optional `contract_metadata.executable_representation` object has these fields:

| Field | Type | Contract |
| --- | --- | --- |
| `representation_version` | unsigned integer | Executable encoding version, independent of the package and manifest versions. The current compiler writes and reads version 9. |
| `content_digest` | string | Exactly 64 lowercase hexadecimal digits containing the binary file's SHA-256 digest. It selects `semantic/<content_digest>.incnsem` relative to the manifest directory. |

An omitted object denotes a package with no published executable representation. The package remains valid for native linking. The reader checks the binary's leading version before decoding version-specific data, verifies package and coverage identity against the manifest, and checks the complete file against its selected digest. This content check is not a package signature.

## Coverage and identity

Coverage is declared for the manifest's public canonical identities. A callable fragment carries its required public declarations and type context. Public fields, enum variants, and abstract trait methods can refer to their declaring type's context. Aliases and facades resolve to the original declaration; they do not create another executable body.

Coverage can be partial. The published content excludes private declarations and private type layouts. A public body that needs either remains uncovered, as does a body containing an unsupported operation or an unresolved reference. An uncovered required public declaration also leaves its callers uncovered. An empty supported function is covered; it is distinct from an absent executable declaration.

Published fragments represent functions and methods, checker-evaluated scalar and text constants, nongeneric erased type aliases including unions, admitted model and class layouts, concrete trait implementation records, trait identities, normal enum payload layouts, fieldless enums, and scalar value enums. A body is covered when Body IR represents all of it and its types and references satisfy the public executable closure checks. A trait default retains its abstract receiver until concrete implementation specialization. Published function bodies have no original source text; native execution uses source-less diagnostic spans for their package identities.

## Resolution refusals

Resolving a required public declaration from a dependency's representation refuses, naming the package, its version and the unmet requirement, when:

| Condition | Result |
| --- | --- |
| Representation absent or selected file unavailable | The required declaration has no executable content. |
| Unsupported representation version | The incompatible version is reported without decoding its payload. |
| Digest, package identity, public coverage or selected payload inconsistent | The artifact is rejected as unusable. |
| Required declaration uncovered | The unavailable public requirement is reported. |

A dependency's representation is never regenerated from available source.

## Publication and reuse

A successful library build publishes the semantic file selected by its new manifest. Unselected files are not searched for a substitute representation. Native outputs, checked metadata and selected semantic content are retained together during ordinary materialization and reuse.

A failed library rebuild restores the previous library output and its existing publication receipts. That output directory can be temporarily unavailable during a rebuild. This local workflow does not guarantee availability to concurrent readers or atomic recovery across a process crash, and it does not establish signed archive distribution.
