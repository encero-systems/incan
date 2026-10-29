# Documentation and inspection surfaces

| Surface | Contract |
| --- | --- |
| `llms.txt` | Site entrypoint list. |
| `incan check PATH --format json` | Diagnostic report, schema version `2`. |
| `incan describe-unit PATH` | Unit description, schema `incan.unit/1`. |
| `incan inspect codegraph PATH --format jsonl` | Graph records, schema version `9`, with identities, provenance and degraded state. |
| `incan inspect codegraph PATH --task TASK` | Bounded context selection with snapshot and context identities. |
| `incan inspect codegraph PATH --stale SNAPSHOT` | Snapshot freshness response. |
| `incan mcp PATH` | Graph tools and resources over stdio JSON-RPC. |

[CLI reference](cli_reference.md) specifies command options. [Agent context CLI and MCP](agent_context_cli.md) specifies graph identities, context budgets, snapshot checks, MCP methods, tools and resources.
