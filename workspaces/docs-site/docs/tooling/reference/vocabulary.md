# Vocabulary entries (`[vocab]`)

The optional `[vocab]` table in `loaf.toml` declares a library's Incan vocabulary entry. A consumer reads the published registration of each dependency that declares one. Each registered surface activates in a consumer module as its [`DslSurface`](../../language/reference/stdlib/vocab.md#dslsurface) activation selects.

```toml
[vocab]
entry = "src/vocab.incn"
```

## Fields

| Key | Type | Default | Contract |
| --- | --- | --- | --- |
| `entry` | nonempty string | Required when `[vocab]` is present | An existing `.incn` module inside the Loaf after symlink resolution. Path components contain ASCII letters, digits or underscores and cannot start with a digit. |
| `crate` | — | — | Refused; `entry` is the replacement. |

Other keys in this table are refused. No module of the vocabulary unit may import the Loaf's own library (`pub::NAME`) or, for a standard package, its own `std` namespace; such an import is refused as a vocabulary activation cycle.

## Entry exports

The entry must export:

```incan
pub def library_vocab() -> VocabRegistration
```

`VocabRegistration` is imported from [`std.vocab`](../../language/reference/stdlib/vocab.md). When the registration calls `with_desugarer()`, the entry must also export:

```incan
pub def desugar(request: DesugarRequest) -> Result[DesugarResponse, DesugarError]
```

Each export is a public, synchronous function without type parameters, declared in the entry module itself; a re-export does not satisfy it. `desugar` takes one immutable parameter. Private, missing and differently shaped exports are refused.

A registration without `with_desugarer()` publishes descriptors only. Using one of its declarations is refused, except a declaration whose body is an embedded fragment: the fragment and its expression holes are checked, and executable compilation refuses the fragment.

## Publication and execution

Oven builds the entry and its imports as native host units. It runs registration at the producer and publishes the descriptors, schema version and vocabulary-unit identity with the library. Consumers read those descriptors. Registration runs again when any input of the vocabulary unit changes.

A custom expansion runs in a separate native worker. Requests contain structured syntax and the requesting module's path. Responses contain structured expressions, statements or compilation-unit declarations matching the registered output position. Source text is not an accepted response. Each output source range must lie inside its requesting vocabulary node; an omitted output range inherits that node's range. Distinct output nodes may share a range. A reused response produces the same output as the computation it replaces.

One module's requests to one provider may share a batch; replies retain request order. Each batch invocation, covering all of its requests, has a ten-second deadline and an 8 MiB response-frame limit. Invalid output, a wrong output position, a malformed frame, abnormal worker exit and a deadline violation produce a diagnostic (`INCAN-P0001`) at the requesting source node that names the providing Loaf. The rest of the module is still checked. Desugarers must reserve stdout for the worker protocol.

Published metadata version 1 and syntax version 3 are supported. A different version is refused with the supported version. WebAssembly desugarer metadata is refused and requires the library to be rebuilt.

## Admission, reuse and evidence

| Setting or artifact | Contract |
| --- | --- |
| `INCAN_VOCAB_POLICY` | Defaults to `locked-dependencies`, admitting the exact native units in the resolved provider lock. Other values, including `deny`, deny execution. |
| `target/incan/vocabulary/providers.lock.json` | Resolved provider, vocabulary-unit identity, manifest hash, worker hash and policy. |
| `target/incan/vocab-responses/evidence.jsonl` | Provider, vocabulary-unit identity, requesting module, policy, admission, granted capabilities and response decision (`computed`, `reused` or `denied`). |
| `OVEN_TRACE=-` | Writes vocabulary admission and response decisions to stderr. A nonempty path selects a trace file. |

Admission precedes response-cache reuse and worker execution. Worker artifact paths must remain inside the dependency artifact root, and their bytes must match the published hash. A worker invocation receives no capabilities; the worker starts with an empty environment, and its standard error is discarded. Admission does not provide an operating-system sandbox.

A desugarer's result must depend only on its vocabulary-unit identity and request. Ambient filesystem, environment, clock and network inputs are unsupported. Responses may be reused, and workers may be retained between invocations.

## See also

- [Author a vocabulary in Incan](../../contributing/how-to/authoring_vocabularies.md)
- [`std.vocab`](../../language/reference/stdlib/vocab.md)
