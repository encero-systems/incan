# Vocabulary entries (`[vocab]`)

The optional `[vocab]` table in `loaf.toml` declares a library's Incan vocabulary entry. Importing that library activates the surfaces in its published registration.

```toml
[vocab]
entry = "src/vocab.incn"
```

## Fields

| Key | Type | Default | Contract |
| --- | --- | --- | --- |
| `entry` | string | Required when `[vocab]` is present | An existing `.incn` module inside the Loaf after symlink resolution. Path components contain ASCII letters, digits or underscores and cannot start with a digit. |
| `crate` | — | — | Refused; `entry` is the replacement. |

Other keys in this table are refused. The entry and its imports must not activate their own vocabulary, including through a dependency cycle.

## Entry exports

The entry must export:

```incan
pub def library_vocab() -> VocabRegistration
```

`VocabRegistration` is imported from [`std.vocab`](../../language/reference/stdlib/vocab.md). When the registration calls `with_desugarer()`, the entry must also export:

```incan
pub def desugar(request: DesugarRequest) -> Result[DesugarResponse, DesugarError]
```

Private exports and missing desugarer exports are refused. A registration without `with_desugarer()` provides descriptors without a custom expansion worker.

## Publication and execution

Oven builds the entry and its imports as native host units. It runs registration at the producer and publishes the descriptors, schema version and vocabulary-unit identity with the library. Consumers read those descriptors. Unchanged registration artifacts are reusable only while their complete unit inputs and retained artifacts match.

A custom expansion runs in a separate native worker. Requests contain structured syntax and the requesting module's path. Responses contain structured expressions, statements or compilation-unit declarations matching the registered output position. Source text is not an accepted response. Each output source range must lie inside its requesting vocabulary node; an omitted output range inherits that node's range. Distinct output nodes may share a range. Reusing a response preserves the checked identities of its output nodes.

One module's requests may share a batch; replies retain request order. Each invocation has a ten-second deadline and an 8 MiB response-frame limit. Invalid output, a wrong output position, a malformed frame, abnormal worker exit and a deadline violation produce provider diagnostics at the requesting source node. Independent module checking continues. Desugarers must reserve stdout for the worker protocol.

Published metadata version 1 and syntax version 3 are supported. A different version is refused with the supported version. WebAssembly desugarer metadata is refused and requires the library to be rebuilt.

## Admission, reuse and evidence

| Setting or artifact | Contract |
| --- | --- |
| `INCAN_VOCAB_POLICY` | Defaults to `locked-dependencies`, admitting the exact native units in the resolved provider lock. Other values, including `deny`, deny execution. |
| `target/incan/vocabulary/providers.lock.json` | Resolved provider, vocabulary-unit identity, manifest hash, worker hash and policy. |
| `target/incan/vocab-responses/evidence.jsonl` | Provider, vocabulary-unit identity, requesting module, policy, admission, granted capabilities and computed/reused response decision. |
| `OVEN_TRACE=-` | Writes vocabulary admission and response decisions to stderr. A nonempty path selects a trace file. |

Admission precedes response-cache reuse and worker execution. Worker artifact paths must remain inside the dependency artifact root, and their bytes must match the published hash. A worker invocation receives no capabilities. Admission does not provide an operating-system sandbox.

A desugarer's result must depend only on its vocabulary-unit identity and request. Ambient filesystem, environment, clock and network inputs are unsupported. Responses may be reused, and workers may be retained between invocations.

## See also

- [Author a vocabulary in Incan](../../contributing/how-to/authoring_vocabularies.md)
- [`std.vocab`](../../language/reference/stdlib/vocab.md)
