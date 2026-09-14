# Package executable representation

A built Incan library can publish checked executable content for its public declarations. The replacement backend reads that content to execute supported package calls without the producer's Incan source or native linking. The native backend continues to consume the compiled library.

For command steps, see [Execute a published package](../how-to/execute_published_package.md).

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
| `representation_version` | unsigned integer | Executable encoding version, independent of the package and manifest versions. The current compiler accepts version 4. |
| `content_digest` | string | Exactly 64 lowercase hexadecimal digits containing the binary file's SHA-256 digest. It selects `semantic/<content_digest>.incnsem` relative to the manifest directory. |

An omitted object denotes a package with no published executable representation. The package remains valid for native linking. The reader checks the binary's leading version before decoding version-specific data, verifies package and coverage identity against the manifest, and checks the complete file against its selected digest. This content check is not a package signature.

## Coverage and identity

Coverage is declared for the manifest's public canonical identities. A callable fragment carries its required public declarations and type context. Public fields and enum variants can refer to their declaring type's context. Aliases and facades resolve to the original declaration; they do not create another executable body.

Coverage can be partial. The published content excludes private declarations and private type layouts. A public body that needs either remains uncovered, as does a body containing an unsupported operation or an unresolved reference. An uncovered required public declaration also leaves its callers uncovered. An empty supported function is covered; it is distinct from an absent executable declaration.

Published fragments currently represent supported functions and methods, plain-model layout, fieldless enums and scalar value enums. Each selected body and value shape must also satisfy the replacement backend's [execution profile](../explanation/backend_selection_receipts.md). Publication does not grant general support for generics, methods, aggregate shapes, interop or other operations that the execution profile refuses.

## Refusals

A replacement command refuses an unusable required package declaration before program output or a new successful execution receipt. The diagnostic names the package, its version and the unmet requirement. An earlier successful receipt is not evidence that the refused attempt succeeded.

| Condition | Result |
| --- | --- |
| Representation absent or selected file unavailable | The required package declaration cannot be executed through this route. |
| Unsupported representation version | The compiler reports the incompatible version without decoding its payload. |
| Digest, package identity, public coverage or selected payload inconsistent | The compiler rejects the unusable artifact. |
| Required declaration uncovered | The compiler reports the unavailable public execution requirement. |
| Published body outside the consumer's execution profile | The compiler reports the package execution requirement that it cannot satisfy. |

The replacement command does not regenerate a dependency's representation from available source and does not fall back to native execution.

## Execution report

`incan build --backend replacement --report json --report-output <PATH>` writes the `incan.replacement_execution.v1` report. The following nonnegative integer fields occur inside `replacement_execution`:

| Field | Meaning |
| --- | --- |
| `package_declarations_decoded` | Number of selected package declaration fragments decoded, including required public type context. |
| `package_payload_bytes_read` | Sum of encoded bytes read for those selected fragments. |
| `package_content_bytes_verified` | Sum of complete semantic-file bytes streamed for digest verification. |

The reader decodes selected fragments and their required public closure. Digest verification still reads complete files, so selective decoding does not imply selective total I/O. These counters do not measure elapsed time or peak memory. See the [CLI reference](cli_reference.md#incan-build) for the remaining report fields.

## Publication and reuse

A successful library build publishes the semantic file selected by its new manifest. Unselected files are not searched for a substitute representation. Native outputs, checked metadata and selected semantic content are retained together during ordinary materialization and reuse.

A failed library rebuild restores the previous library output and its existing publication receipts. That output directory can be temporarily unavailable during a rebuild. This local workflow does not guarantee availability to concurrent readers or atomic recovery across a process crash, and it does not establish signed archive distribution.
