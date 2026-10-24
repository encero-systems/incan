# RFC 156: Typed MCP servers

- **Status:** Draft
- **Created:** 2026-10-17
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 036 (checked declaration decorators)
    - RFC 085 (field contracts)
    - RFC 086 (schema descriptors and adapters)
    - RFC 090 (model-first CLI boundaries)
    - RFC 103 (secret protection)
    - RFC 104 (capabilities and receipts)
    - RFC 106 (compiler agent surface)
    - RFC 113 (checked declaration registries)
    - RFC 131 (typed failures)
    - RFC 142 (async execution and cancellation)
    - RFC 146 (typed JSON boundary validation)
- **Issue:** [#1624](https://github.com/encero-systems/incan/issues/1624)
- **RFC PR:** —
- **Written against:** v0.6
- **Shipped in:** —

## Summary

Propose `std.mcp` as an Incan-authored, typed server surface for Model Context Protocol tools, resources, and prompts. Checked declarations supply registration, argument contracts, schemas, dispatch, and structured outcomes. The required baseline is MCP 2026-07-28 over stdio, with protocol-level JSON Schema support, secret-safe diagnostics, and per-invocation capability evidence. Transport simplicity must not defer schema correctness or authority checks. Declaration spelling and several boundary details remain draft candidates.

## Core model

1. A server receives an explicit checked catalog of declarations; imports do not register handlers by side effect.
2. A declaration has one canonical identity and one checked input/output contract, shared by schema generation and invocation.
3. Wire parsing, model construction, handler execution, and result encoding are separate observable boundaries.
4. Each invocation is governed independently. A description or tool annotation is information, not authority.
5. User-program MCP authoring and the compiler's own agent surface are separate consumers of shared conformance rules.

## Motivation

A tool advertised with handwritten JSON Schema can diverge from its actual handler. The result is especially harmful for an agent: a formally valid call may fail for an undocumented requirement, or an unsafe operation may appear harmless from a description. Incan should derive inspectable contracts from declarations and retain typed evidence when a boundary fails.

## Goals

- Author servers in Incan using ordinary typed models and functions.
- Reuse RFC 113 registries, RFC 086 schemas, and canonical construction checks.
- Supply conformant discovery, dispatch, structured outcomes, resources, and prompts.
- Keep per-request authorization and receipts explicit under RFC 104.
- Preserve RFC 103 protections across logs, protocol errors, schemas, and results.

## Non-Goals

- Making tool descriptions grant capabilities.
- Implicit module-import registration or arbitrary reflection-based exposure.
- Making std.mcp an authoring dependency of the compiler.
- Defining an HTTP server framework or an MCP client SDK here.
- Adding new authored Rust when existing crates can be consumed through Incan interop.

## Guide-level explanation

Every imported std.mcp name below is proposed. Both argument/result models and the application handler are defined here. The example deliberately performs no external I/O.

```incan
from std.mcp import McpError, Server, tool


model TitleArgs:
    title: str


model TitleSummary:
    text: str


@tool(description="Render a supplied title as a short summary")
def summarize_title(args: TitleArgs) -> TitleSummary:
    return TitleSummary(text=f"Title: {args.title}")


def main() -> Result[None, McpError]:
    server = Server(
        name="title-tools",
        version="0.1.0",
        tools=[summarize_title],
    )
    return server.serve_stdio()
```

The candidate Server constructor accepts a checked tool catalog; resources and prompts default to empty catalogs. `serve_stdio() -> Result[None, McpError]` owns the protocol loop. A malformed wire request does not call summarize_title. A valid call first constructs TitleArgs under the same field and validation rules as other typed JSON boundaries, then invokes the function and encodes TitleSummary.

The generated tool name is summarize_title unless explicit naming is selected later. Its input has a required string title; its output has a required string text. An advertised schema must not claim that arbitrary model validators are fully expressible in JSON Schema. Such checks still execute during canonical construction, with their limits described in the checked descriptor.

No ordinary println output belongs in this example's protocol loop. Protocol messages use stdout; application diagnostics need the server's safe diagnostic channel.

## Reference-level explanation

### Checked catalog and schemas

- Every exposed declaration must appear in an explicit catalog validated under RFC 113. Duplicate wire names must be rejected before serving.
- A tool must have a checked argument contract and a checked output contract. Unsupported schema shapes must be diagnosed rather than exported as an unconstrained placeholder.
- Input and output schemas must derive from RFC 086 metadata, preserving aliases, constraints, requiredness, and protected-field rules.
- Model validation must follow RFC 122 and RFC 146. Schema acceptance must not bypass canonical validation.
- Successful structured outputs must satisfy the advertised output contract. Domain failures need a separately specified safe error representation.
- Resources and prompts must use the same registration authority. Resource addresses, MIME contracts, prompt arguments, and message roles must be inspectable; they must not be discovered by executing arbitrary handler code.

### Protocol boundary

The baseline is [MCP 2026-07-28 discovery](https://modelcontextprotocol.io/specification/2026-07-28/server/discover), including server/discover and revision-appropriate self-contained request metadata. Required and recommended metadata fields must be distinguished; the framework must not infer requirements from abbreviated examples on another page.

The [tools contract](https://modelcontextprotocol.io/specification/2026-07-28/server/tools) requires tool input schemas and defines structured results, output schemas, and execution-error reporting. Incan must preserve the distinction between malformed protocol requests, argument/domain failures, and a successful domain result containing no usable answer. Discovery ordering must be deterministic for the same authorized catalog. Result discriminators and list/read cache metadata must follow the selected revision; authorization-sensitive results must not be cached as public.

The [stdio transport](https://modelcontextprotocol.io/specification/2026-07-28/basic/transports/stdio) uses newline-delimited JSON-RPC over stdin/stdout. Diagnostic output must not corrupt that stream. The framework must bound incoming message size and reject invalid framing safely. EOF, transport failure, and handler failure must remain distinguishable.

### Authority and diagnostics

- Tool metadata must not grant filesystem, network, process, or other effects. RFC 104 authority must be checked at invocation.
- Receipts must identify the selected declaration, authorized effect scope, outcome, and available provenance without retaining protected plaintext.
- RFC 131 failures must retain typed upstream identity locally. Safe protocol formatting must not flatten the internal diagnostic record into the only retained representation.
- Secret values must not leak through argument echoing, handler exceptions, schema defaults, cache keys, tracebacks, or protocol diagnostics.
- An explicit application state handle must not become an implicit protocol authorization grant.

## Design details

The candidate model-first handler uses one argument model, avoiding a second signature-to-object binding system. A multi-parameter convenience surface is possible only if it normalizes to the same checked contract. Resources and prompts are part of the intended complete authoring surface, although their exact decorators and return types need further test drives before acceptance.

Protocol mechanics may use an existing Rust SDK through rust:: interop. JSON Schema emission belongs to the schema adapter boundary even over stdio. Process standard-input support is tracked by issue [#1625](https://github.com/encero-systems/incan/issues/1625); an unsupported bridge must be tracked rather than replaced by an ungoverned host script. Async handlers must integrate with RFC 142 cancellation and RFC 094 cleanup without inventing a second scheduler.

## Alternatives considered

- Handwritten JSON-RPC and JSON Schema: duplicates checked declarations and invites drift.
- Expose only the compiler's MCP surface: serves compiler consumers, not user-authored servers.
- Fold all declarations into HTTP routes: confuses transport reuse with tool/resource/prompt identity.
- Dynamic registration by imports: makes the advertised capability depend on incidental execution order.

## Drawbacks

Protocol revisions change and need explicit compatibility handling. Full schema support is a substantial dependency, not a decoration around dispatch. Arbitrary validators cannot always be represented completely as JSON Schema. Authority-aware caching and safe diagnostic formatting need careful contracts.

## Layers affected

- **Stdlib / interop:** Incan server declarations and conformant transport integration.
- **Registry / schema adapters:** checked catalogs and JSON Schema emission.
- **Runtime authority / diagnostics:** per-invocation evidence and safe failures.
- **Tooling / documentation:** inspectable catalog, contract diagnostics, and conformance evidence.

## Unresolved questions

- What are the exact tool, resource, and prompt decorators and typed result envelopes?
- Should handlers take one argument model exclusively, or support checked signature sugar?
- How are request context, authentication identity, cancellation, and deadlines supplied to a handler?
- How are typed argument failures and domain failures represented on the wire while preserving internal error identity?
- Which schema shapes are supported, and how are non-schema-expressible validators described honestly?
- What are the message-size, concurrency, and cache defaults, and how are they configured?
- Which multi-round-trip features are required for conformance and which may be explicitly unadvertised?
- What protocol revision negotiation and conformance-suite evidence are required before release?

<!-- Rename this section to "Design Decisions" once all questions have been resolved.
     An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
