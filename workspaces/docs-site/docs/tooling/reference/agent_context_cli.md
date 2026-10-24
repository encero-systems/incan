# Agent context CLI and MCP reference

This page specifies the dev.8 graph retrieval, context packing, freshness and MCP interfaces. Paths select an Incan source file or an Oven project. Rust projects use their Oven Rust facet. Dependency facts come from published Loaf graphs or checking-only source exports. Inspection may populate local build and graph caches.

## CLI signatures

```text
incan inspect codegraph PATH [--format json|jsonl] [--strict] [--risk]
incan inspect codegraph PATH --query KIND TERM [--limit N] [--strict]
incan inspect codegraph PATH --task TASK [--budget N] [--format compact|json|jsonl] [--strict] [--risk]
incan inspect codegraph PATH --stale SNAPSHOT [--strict]
incan mcp PATH
```

| Argument | Type and default | Contract |
| --- | --- | --- |
| `PATH` | Required path string | Existing source file or project. The MCP server fixes this selection for its lifetime. |
| `--format` | `jsonl` by default | Raw export: JSON array or one JSON object per line. Task output: compact text, JSON array or JSONL header/nodes/edges. `compact` requires `--task`. |
| `--query KIND TERM` | Two strings | `KIND` is `defs`, `refs`, `callers`, `callees` or `imports`. `TERM` is an exact short name, qualified name or exported record ID. Output is text. |
| `--limit N` | Integer, default `20`, minimum `0` | Maximum query hits, followed by a remaining-hit count when truncated. `0` suppresses hit lines. It does not set the task budget. |
| `--task TASK` | Nonempty string | Declaration ID, name, qualified name or free task text. Cannot be combined with `--query` or `--stale`. |
| `--budget N` | Integer, default `8000`, range `128..1000000` | Conservative token upper bound: the complete UTF-8 response, including its final newline, must fit in this many bytes. The MCP JSON-RPC envelope is outside this bound. |
| `--stale SNAPSHOT` | String beginning `snapshot:` | Returns JSON with `expected`, current `snapshot_id` and Boolean `stale`. A stale snapshot is a successful status response. It takes precedence over a simultaneous query. |
| `--strict` | Boolean, default `false` | Refuses the export if any selected unit fails checking; reports its diagnostics. The default exports available facts and warns about unchecked units on stderr. |
| `--risk` | Boolean, default `false` | Adds optional local git observations. Missing git history contributes no risk records. |
| `--substitute NAME=DIR` | Repeatable dependency mapping | Selects a local dependency source directory for inspection. |

Unknown flags, absent values, invalid formats, unknown query kinds, invalid budgets, missing paths and conflicting task selectors fail with a diagnostic and a nonzero exit code. A task request fails if even its header cannot fit. Empty query results return no hits; alongside unchecked-unit warnings they do not prove absence of callers or references.

## Export identity and snapshots

The graph stream begins with its header. `schema_version` is `9`; the additional consumer identity schema is `identity_schema: "incan.graph.identity/1"`. Compiler `canonical_identity` remains compilation provenance. A signature-bearing compiler `stable_identity` produces a package-name-scoped `stable_key`. Package version, source spans, file layout and semantic digest do not enter that key. Ambiguous or unproved targets receive no inferred key. Module or declaration renames may change the logical identity.

`declaration_digests` maps each proved declaration key to its exported `semantic_digest`, or JSON `null` when unavailable. Identity and meaning remain separate. A missing digest authorizes no semantic reuse; this includes Rust declarations for which the producer has no checked digest.

`snapshot_id` is content-addressed over the selected graph artifacts, source-backed input trees, selection and mode. Source byte changes, including equal-size comment edits, invalidate source-backed snapshots. A published dependency graph is bound to its publication; editing unbuilt source behind that publication does not silently replace its facts. Snapshots do not assert that every selected unit checked.

## Context selection

The packer selects declaration, module, import, namespace, diagnostic and optional risk nodes from the export. Exact ID, identity or label matches seed a score of `4000`; matching ASCII task words add `100` each. Keywords search labels, paths, signatures, messages and documentation. Checked facts add `20` and public declarations add `10`. Two deterministic expansion waves follow graph links in both traversal directions; call/reference neighbors receive half the preceding score, other neighbors one quarter. Output edges retain their original direction. Ties sort by stable identity and then exported ID. Current explicit MCP feedback adds a bounded boost of `200` and an inclusion reason.

Nodes consume at most three quarters of the byte budget; edges use the remaining space. Selection is deterministic for the same snapshot, task, budget and active feedback. Records without a reason to match or connect are omitted. No embedding or external model is consulted.

### Header

| Field | Type | Contract |
| --- | --- | --- |
| `schema` | String | `incan.context/1`. |
| `context_id` | String | Content-addressed identity of the request and selected records. Formatting alone does not change it when the selected records remain identical. |
| `snapshot_id` | String | Snapshot of the underlying export. |
| `budget` | Integer | Requested conservative upper bound. |
| `token_accounting` | String | `utf8_bytes_upper_bound`. |
| `degraded` | Boolean | Whether the underlying graph is partial. |
| `unresolved_calls` | Integer | Count of unresolved call targets in the complete selected graph, including calls omitted by the budget. |

### Compact text

The first line has this shape:

```text
incan.context/1 SNAPSHOT CONTEXT degraded=BOOL unresolved_calls=N budget=N tokens<=utf8_bytes
```

Each node line starts with a response-local `nN`, followed by its record kind, a JSON-quoted label, bracketed provenance and degraded state, quoted source anchor, signature and `why=` inclusion reason. Optional details follow. Anchors and reasons are quoted so embedded newlines remain unambiguous. Each edge line has `nSOURCE -KIND-> nTARGET`, bracketed provenance/degraded state and a quoted source anchor. Local IDs are valid only within that response. Diagnostic details contain the code and message; process details contain the raw git evidence and its controls.

### JSON and JSONL

JSON is an array whose first element is the header, followed by nodes and then edges. JSONL emits that same sequence one object per line, each with a `record` discriminator (`context`, `node` or `edge`). Node fields are `local_id`, `id`, `stable_key`, `kind`, `label`, `signature`, `anchor`, `provenance`, `degraded`, `score`, `reason` and `details`. The context node's `stable_key` is the preferred consumer identity: its proved key, a structured compiler stable identity serialized as a string, or an empty string. Only a proved declaration key with an available semantic digest authorizes semantic comparison. Unavailable signatures/details are null. An anchor is rendered from the source path and 1-based line, or `<unknown>:0`. Edge fields are `record`, `kind`, `source`, `target`, `anchor`, `provenance` and `degraded`; source and target name local IDs. The serialized response satisfies the byte bound for its selected format.

## Optional process observations

`--risk` and the MCP `process_risk` tool expose `risk_signal` records with schema `incan.process-risk/1` and provenance `process_observed`. They observe at most the latest 100 non-merge commits. Fields include the represented `target_id`, repository-relative `path`, `head`, `sample_commits`, `history_truncated`, `churn`, `ownership`, `co_changes`, `evidence`, `co_change_excluded_commits` and `controls`.

`churn` reports commit touches, added/removed lines and binary changes. `ownership` counts touches per git author identity; it makes no claim about organizational ownership. `co_changes` counts paths changed in the same commits; commits touching more than 256 paths are excluded from this measure and cited separately. Binary changes have no line totals; quoted paths are omitted. Evidence lists the contributing commit IDs. No predictive defect score or coverage value is synthesized. Missing inputs or no observed touches produce no corresponding record.

## MCP transport

`incan mcp PATH` serves newline-delimited JSON-RPC 2.0 on stdin/stdout. Stdout contains protocol messages only; stderr carries inspection warnings. Requests run serially. The server advertises protocol version `2025-11-25`, tools and resources. Clients send `initialize`, then `notifications/initialized`, before using tools/resources. `ping` returns `{}`. Notifications receive no response. Closing stdin ends the session and discards its memory.

Malformed JSON returns `-32700`; an invalid envelope returns `-32600`; unknown methods return `-32601`; unknown tools return `-32602`; use before initialization returns `-32002`. Tool argument or execution errors return a successful JSON-RPC envelope containing `isError: true`. Successful calls contain one text content block and `isError: false`.

### Tools

Every argument object rejects unknown fields. Fields listed below are required; no tool argument changes the server's project. Budgets have the CLI range and byte bound.

| Tool | Arguments | Text result |
| --- | --- | --- |
| `context_for_task` | `task: string`, `budget: integer` | Compact context for task text. |
| `context_for_declaration` | `declaration: string`, `budget: integer` | Compact context using the declaration selector as its task. |
| `graph_query` | `kind: string`, `term: string` | CLI query hits; the default limit is `20`. |
| `graph_summary` | `{}` or omitted | JSON object with `snapshot_id`, `degraded`, `languages`, `semantic_contexts` and counts per record kind. |
| `snapshot_status` | `snapshot: string` | Freshness JSON as described above. |
| `process_risk` | `{}` or omitted | JSON array of optional process observations. |
| `record_feedback` | `task: string`, `declaration: string`, `snapshot: string`, `ttl_seconds: integer` | The retained feedback object. Mutates only this MCP session. |
| `task_memory` | `task: string` | JSON array of current feedback for that exact task. |

Feedback requires a nonempty task, the current snapshot, a unique proved declaration and a TTL in `1..86400` seconds. The declaration selector accepts an exported ID, stable key or exact qualified label. The server retains at most 128 observations keyed by task and declaration; repeating that key replaces its observation. Each record carries `stable_key`, `snapshot_id`, `expires_at`, task and provenance `session_feedback`. Reads, writes and preference lookup automatically discard entries whose TTL elapsed or snapshot changed. Feedback never enters compiler graph records. All other tools are annotated read-only.

### Resources

`resources/list` returns three read-only `text/plain` resources. `resources/read` takes `uri: string` and returns one content item.

| URI | Contents |
| --- | --- |
| `incan://graph/summary` | Current graph summary JSON. |
| `incan://graph/snapshot` | Current graph summary JSON, including snapshot identity. |
| `incan://graph/schema` | A short schema orientation string identifying `incan.context/1`, JSONL authority and unresolved-target semantics. |

An unknown URI returns `-32002`; an inspection failure while reading returns `-32603`. The server exposes no arbitrary file reader, remote index or program execution tool.

## Editor graph views

The dev.8 LSP projects the same portable graph records from its existing captured-buffer analysis for definition, references and incoming/outgoing call hierarchy. Source ranges use the captured text's UTF-16 positions. The consumer's declared Loaves must have prepared compiled interfaces. A changed or closed buffer invalidates dependent views. Call hierarchy items carry a document version; using an item from an earlier version returns null. An unavailable or stale view yields no navigation result until analysis supplies a current view.
