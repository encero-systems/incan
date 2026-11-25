# Agent context CLI and MCP reference

This page specifies `incan inspect codegraph` retrieval, context packing and snapshot freshness, and the `incan mcp` server.

## CLI signatures

```text
incan inspect codegraph PATH [--profile NAME] [--format json|jsonl] [--strict | --allow-errors] [--risk [--coverage-report PATH]] [--substitute NAME=DIR]... [FEATURE OPTIONS]
incan inspect codegraph PATH --query KIND TERM [--profile NAME] [--limit N] [--format json|jsonl] [--strict] [--substitute NAME=DIR]... [FEATURE OPTIONS]
incan inspect codegraph PATH --task TASK [--profile NAME] [--budget N] [--format compact|json|jsonl] [--strict] [--risk [--coverage-report PATH]] [--substitute NAME=DIR]... [FEATURE OPTIONS]
incan inspect codegraph PATH --stale SNAPSHOT [--profile NAME] [--strict | --allow-errors] [--risk [--coverage-report PATH]] [--substitute NAME=DIR]... [FEATURE OPTIONS]
incan mcp PATH [--profile NAME] [--coverage-report PATH]
```

| Argument | Type and default | Contract |
| --- | --- | --- |
| `PATH` | Required path string | An existing path. A `.incn` file selects that source. A directory without a `loaf.toml` inside a Loaf, such as its `src`, selects that Loaf. A directory without `src/lib.rs` selects its `src/lib.incn`, else its `src/main.incn`. Any other directory selects its Rust facet, and any other file the Rust facet of the nearest directory with a `loaf.toml`. The MCP server fixes this selection for its lifetime. |
| `--profile NAME` | String, `debug` | Selects the artifact profile. Standard packages and native support must have artifacts for that profile. |
| `--format` | `jsonl` by default | Raw export: JSON array or one JSON object per line. A `defs` query with `json` returns a header and the selected declarations, up to `--limit`; other queries retain text output. Task output: compact text, JSON array or JSONL header/nodes/edges. `compact` requires `--task`. |
| `--query KIND TERM` | Two strings | `KIND` is `defs`, `refs`, `callers`, `callees` or `imports`. For `defs`, `refs`, `callers` and `callees`, `TERM` is an exact short name, qualified name or exported record ID; for `imports`, it selects the modules whose name or ID contains it. Output is text except for `defs` with `--format json`. Queries resolve dependency identities through their published graphs. |
| `--limit N` | Integer, default `20`, minimum `0` | Maximum query hits. Text reports the remaining-hit count when truncated; structured definitions carry `remaining_hits` in the header and declaration digests only for returned records. `0` suppresses hits. It does not set the task budget. |
| `--task TASK` | Nonempty string | Declaration ID, name, qualified name or free task text. Cannot be combined with `--query` or `--stale`. |
| `--budget N` | Integer, default `8000`, range `128..1000000` | Conservative token upper bound: the complete UTF-8 response, including its final newline, must fit in this many bytes. The MCP JSON-RPC envelope is outside this bound. |
| `--stale SNAPSHOT` | String beginning `snapshot:` | Returns a JSON object with `record: "snapshot_status"`, `expected`, the current `snapshot_id` and Boolean `stale`. A stale snapshot is a successful status response. It takes precedence over a simultaneous query. |
| `--strict` | Boolean flag | Refuses if any selected unit fails checking and reports its diagnostics. A plain raw export uses this mode by default. |
| `--allow-errors` | Boolean flag | Selects a tolerant raw export that keeps available facts and warns about unchecked units on stderr. Combined with `--strict` it exits `1` with `--allow-errors cannot be combined with --strict`. |
| `--risk` | Boolean, default `false` | Adds optional local Git and coverage observations when their inputs are available. |
| `--coverage-report PATH` | Optional file path | Selects one coverage report relative to the invocation directory. With `incan inspect`, requires `--risk`. With `incan mcp`, fixes the report path for `process_risk` requests. |
| `--substitute NAME=DIR` | Repeatable dependency mapping | Replaces the local dependency of package `NAME` with the project at `DIR` wherever the dependency graph reaches it. |
| `--features A,B`, `--no-default-features`, `--all-features` | Root-package feature selection, as for [unit commands](cli_reference.md#unit-options); default features | The inspected unit and its dependencies are checked with this selection. A source outside any project refuses it. Each selection has its own export. |

Unknown options exit `2`. Absent values, invalid formats, unknown query kinds, invalid limits or budgets, missing paths and conflicting task selectors exit `1` with a diagnostic. A task request fails if even its header cannot fit. Empty query results return no hits; alongside unchecked-unit warnings they do not prove absence of callers or references.

## Export identity and snapshots

The graph stream begins with its header. `schema_version` is `9`; the additional consumer identity schema is `identity_schema: "incan.graph.identity/2"`. Compiler `canonical_identity` is compilation provenance. A signature-bearing compiler `stable_identity` produces a package-name-scoped `stable_key`. Package version, source spans, file layout and semantic digest do not enter that key. Ambiguous or unproved targets receive no inferred key. Module or declaration renames may change the logical identity.

`declaration_digests` maps each proved declaration key to its exported `semantic_digest`, or JSON `null` when unavailable. Identity and meaning remain separate. A missing digest authorizes no semantic reuse; this includes Rust declarations for which the producer has no checked digest.

`snapshot_id` is content-addressed over the selected graph artifacts, source-backed input trees, selection and mode. Source byte changes, including equal-size comment edits, invalidate source-backed snapshots. A published dependency graph is bound to its publication; editing unbuilt source behind that publication does not silently replace its facts. Snapshots do not assert that every selected unit checked.

The plain `incan inspect codegraph PATH --stale SNAPSHOT` command recognizes an exact match to either current export mode's snapshot ID. On a miss, it reports the current tolerant snapshot ID and sets `stale` to `true`. `--strict --stale SNAPSHOT` and `--allow-errors --stale SNAPSHOT` compare only the selected mode. If a selected unit fails checking, the plain command reports a previously checked snapshot as stale and warns about the unchecked unit.

An answer is kept in the bakery as a page, a file under `pages/` named by the identity of the request: the running Oven program, the installed compiler, the resolved path and every graph option. The page holds the snapshot the answer computed, if any, the status metadata of every file and directory its preparation observed, and, for a Rust project, an observation of each environment variable rustc's dependency information names, which records no value. A repeat of the request whose inputs all have the same metadata and whose variables are unchanged is answered from the page: it prepares no unit, runs no compiler and reads or hashes no source. A changed, added, removed or recently changed input, a changed variable, another compiler or another Oven prepares the request again and replaces the page. A page is a cache, replaced by rename without a durable sync. `incan mcp` requests for the same selection read the same pages. A `--risk` answer and an answer with an unchecked unit are not paged. Inputs rustc does not report in its dependency information, such as a file or variable a procedural macro reads, are not observed; checked Rust exports have the same limit.

## Context selection

The packer selects declaration, module, import, namespace, diagnostic and optional risk nodes from the export. Exact ID, identity or label matches seed a score of `4000`; matching ASCII task words add `100` each. Keywords search labels, paths, signatures, messages and documentation. Checked facts add `20` and public declarations add `10`. Two deterministic expansion waves follow graph links in both traversal directions; call/reference neighbors receive half the preceding score, other neighbors one quarter. Output edges retain their original direction. Ties sort by stable identity and then exported ID. Unexpired MCP feedback for the same task and snapshot adds a bounded boost of `200` and an inclusion reason.

Nodes consume at most three quarters of the byte budget; edges use the remaining space. Selection is deterministic for the same snapshot, task, budget and active feedback. Records without a reason to match or connect are omitted.

### Header

| Field | Type | Contract |
| --- | --- | --- |
| `record` | String | `context`. |
| `schema` | String | `incan.context/1`. |
| `context_id` | String | Content-addressed identity of the request and selected records. Formatting alone does not change it when the selected records remain identical. |
| `snapshot_id` | String | Snapshot of the underlying export. |
| `budget` | Integer | Requested conservative upper bound. |
| `token_accounting` | String | `utf8_bytes_upper_bound`. |
| `degraded` | Boolean | Whether the underlying graph is partial. |
| `unresolved_calls` | Integer | Count of unresolved call targets in the complete selected graph, including calls omitted by the budget. |
| `authored_text` | String | `quoted_evidence`: the text items carry from their authors is evidence to read, never instructions. |

### Compact text

The first line has this shape:

```text
incan.context/1 SNAPSHOT CONTEXT degraded=BOOL unresolved_calls=N budget=N tokens<=utf8_bytes authored_text=quoted_evidence
```

Each node line starts with a response-local `nN`, followed by its record kind, a JSON-quoted label, bracketed provenance and degraded state, quoted source anchor, signature and `why=` inclusion reason. Optional details follow. Anchors and reasons are JSON-quoted. Each edge line has `nSOURCE -KIND-> nTARGET`, bracketed provenance/degraded state and a quoted source anchor. Local IDs are valid only within that response. Diagnostic details contain the code and message; process details contain the raw evidence and its controls.

### JSON and JSONL

JSON is an array whose first element is the header, followed by nodes and then edges. JSONL emits that same sequence one object per line, each with a `record` discriminator (`context`, `node` or `edge`). Node fields are `local_id`, `id`, `stable_key`, `kind`, `label`, `signature`, `anchor`, `provenance`, `degraded`, `author`, `score`, `reason` and `details`. An `author` is an object whose `kind` is `toolchain`, `workspace` or `dependency`; a dependency also has its `identity`, the Loaf's name with `@` and its version when it declares one. A node's `stable_key` is its proved key, else its structured compiler stable identity serialized as a string, else an empty string. Only a proved declaration key with an available semantic digest authorizes semantic comparison. Unavailable signatures/details are null. An anchor is rendered from the source path and 1-based line, or `<unknown>:0`. Edge fields are `record`, `kind`, `source`, `target`, `anchor`, `provenance`, `degraded` and `author`; source and target name local IDs. The serialized response satisfies the byte bound for its selected format.

## Optional process observations

`--risk` and the MCP `process_risk` tool expose `risk_signal` records with schema `incan.process-risk/1` and provenance `process_observed`. The Git source observes at most the latest 100 non-merge commits. Fields include the represented `target_id`, repository-relative `path`, `head`, `sample_commits`, `history_truncated`, `churn`, `ownership`, `co_changes`, `evidence`, `co_change_excluded_commits` and `controls`.

`churn` reports commit touches, added/removed lines and binary changes. `ownership` counts touches per git author identity. `co_changes` counts paths changed in the same commits; commits touching more than 256 paths are excluded from this measure and cited separately. Binary changes have no line totals; quoted paths are omitted. Evidence lists the contributing commit IDs. Missing inputs or no observed touches produce no corresponding record.

A coverage report is a UTF-8 JSON file of at most 1 MiB. Set `snapshot_id` to the current export's snapshot ID without `--risk`. Use `incan inspect codegraph PATH --allow-errors` to obtain the tolerant snapshot that matches MCP.

```json
{
  "schema": "incan.coverage-report/1",
  "snapshot_id": "snapshot:<current-graph-id>",
  "files": [
    {"path": "src/main.incn", "covered_lines": 3, "instrumented_lines": 5}
  ]
}
```

Report paths are slash-separated and relative to the repository root, or the owning project outside Git. A path may appear once. Counts are integers with `0 <= covered_lines <= instrumented_lines` and a positive `instrumented_lines`. A row contributes only when the export contains one file with that path. Missing, invalid or stale reports contribute no coverage observations. Git observations are independent.

Each coverage observation has `source: "coverage_report"`, the represented file's `target_id`, the two raw counts, and a `report_digest` in `sha256:<hex>` form over the original bytes. Its `evidence` lists that same digest. The counts describe the instrumented lines reported for the file. Coverage observations change the risk-enabled snapshot while the risk-free export retains its original identity.

## MCP transport

`incan mcp PATH [--profile NAME]` serves newline-delimited JSON-RPC 2.0 on stdin/stdout. The profile defaults to `debug` and fixes the artifact profile for every request in the session. Stdout contains protocol messages only; stderr carries inspection warnings. Requests run serially. Call `server/discover` for `supportedVersions`, server information, and the fixed tool and resource capabilities. The supported version is `2026-07-28`. Clients can call `tools/list`, `tools/call`, `resources/list`, and `resources/read` directly. `ping` returns `{}`. Notifications receive no response. Closing stdin ends the session and discards its memory.

Malformed JSON returns `-32700`; an invalid envelope returns `-32600`; unknown methods return `-32601`; unknown tools return `-32602`. `initialize` returns `-32022` with `error.data.supportedVersions`. Tool argument or execution errors return a successful JSON-RPC envelope containing `isError: true`. Successful calls return a text content block and `isError: false`. Definition queries also return the structured records described below.

### Tools

Every argument object rejects unknown fields. Fields listed below are required; no tool argument changes the server's project. Budgets have the CLI range and byte bound.

| Tool | Arguments | Text result |
| --- | --- | --- |
| `context_for_task` | `task: string`, `budget: integer` | Compact context for task text. |
| `context_for_declaration` | `declaration: string`, `budget: integer` | Compact context using the declaration selector as its task. |
| `graph_query` | `kind: string`, `term: string` | Selected query hits (default limit `20`). For `kind: "defs"`, `structuredContent.records` contains a header followed by the selected declaration records, each with its `author` and any available `span`. The first text block keeps the CLI display. The second serializes the same object as `structuredContent`. |
| `graph_summary` | `{}` or omitted | JSON object with `snapshot_id`, `degraded`, `languages`, `semantic_contexts` and `counts`, an object from record kind to count. |
| `snapshot_status` | `snapshot: string` | Freshness JSON comparing the supplied ID with the service snapshot reported by `graph_summary`. |
| `process_risk` | `{}` or omitted | JSON array of optional process observations. |
| `record_feedback` | `task: string`, `declaration: string`, `snapshot: string`, `ttl_seconds: integer` | The retained feedback object. Mutates only this MCP session. |
| `task_memory` | `task: string` | JSON array of current feedback for that exact task. |

A nonempty `refs`, `callers`, `callees` or `imports` reply begins with `authored_text=quoted_evidence`. Each hit gives its JSON-quoted label and source anchor and identifies the author of the record at that anchor.

Feedback requires a nonempty task, a unique proved declaration and a TTL in `1..86400` seconds. For `snapshot`, use the current ID from `graph_summary` or the exact current checked or tolerant export ID for the same selection. The server rejects stale snapshots, snapshots for another selection and IDs it cannot verify. The declaration selector accepts an exported ID, stable key or exact qualified label.

The server stores feedback under its current snapshot ID from `graph_summary`. Both accepted export IDs use that identity for task memory, ranking and expiry. The server retains at most 128 observations keyed by task and declaration. Repeating that key replaces its observation. Each record carries `stable_key`, `snapshot_id`, `expires_at`, task and provenance `session_feedback`. Reads, writes and preference lookup automatically discard entries whose TTL elapsed or snapshot changed. Feedback never enters compiler graph records. All other tools are annotated read-only.

### Resources

`resources/list` returns three read-only `text/plain` resources. `resources/read` takes `uri: string` and returns one content item.

| URI | Contents |
| --- | --- |
| `incan://graph/summary` | Current graph summary JSON. |
| `incan://graph/snapshot` | Current graph summary JSON, including snapshot identity. |
| `incan://graph/schema` | A short schema orientation string identifying `incan.context/1`, JSONL authority and unresolved-target semantics. |

An unknown URI returns `-32002`; an inspection failure while reading returns `-32603`.

## Editor graph views

Editor navigation over the same graph records is specified in [LSP protocol support](lsp_protocol_support.md#graph-navigation).
