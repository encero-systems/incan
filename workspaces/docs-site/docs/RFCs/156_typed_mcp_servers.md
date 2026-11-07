# RFC 156: Typed MCP servers

- **Status:** Planned
- **Created:** 2026-10-17
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 036 (checked declaration decorators)
    - RFC 037 (native `std.web`)
    - RFC 042 (trait names as abstract types)
    - RFC 066 (HTTP client and test fixtures)
    - RFC 082 (checked API documentation)
    - RFC 085 (field contracts)
    - RFC 094 (scoped cleanup)
    - RFC 095 (telemetry spans)
    - RFC 096 (declaration metadata)
    - RFC 103 (secret protection)
    - RFC 104 (capabilities and receipts)
    - RFC 106 (compiler agent surface)
    - RFC 113 (checked declaration registries)
    - RFC 118 (Incan and Oven command surfaces)
    - RFC 122 (model validation policies)
    - RFC 128 (callables and bound receivers)
    - RFC 131 (typed failures)
    - RFC 141 (`ArgumentsOf` and named type operations)
    - RFC 142 (async execution and cancellation)
    - RFC 146 (typed JSON boundary validation)
    - RFC 152 (depth limits for parsers of external data)
    - RFC 153 (async closures and callables)
    - RFC 167 (debugging over MCP)
- **Issue:** [#1624](https://github.com/encero-systems/incan/issues/1624)
- **RFC PR:** —
- **Written against:** v0.6
- **Shipped in:** —

## Summary

`std.mcp` lets an Incan program serve Model Context Protocol tools, resources and prompts by writing ordinary functions. `@tool` is ordinary library code: from a function's parameters, return type and docstring it builds the JSON Schema an AI client reads, the decoding of its arguments and the encoding of its result, so the schema cannot drift from the handler. The compiler knows nothing about MCP. A server takes an explicit list of what it serves, over stdio or Streamable HTTP on `std.web`, against MCP 2026-07-28. Every call is checked for authority and leaves a receipt, a description grants nothing, and secrets stay out of everything a client sees.

## Core model

1. **A tool is a function.** Its parameters are the inputs, its return type is the output, and its docstring is what the AI reads.
2. **`@tool` is library code.** `std.mcp` is written in Incan over general language facilities, and the compiler has no knowledge of tools, resources or prompts.
3. **One declaration, one contract.** The schema and the decoding come from the same checked declaration, so they cannot disagree.
4. **The server lists what it serves.** A server receives explicit lists, and importing a module never registers anything.
5. **Boundaries stay separate.** Reading a message, decoding arguments, running a handler and encoding a result are distinct, and a malformed request never reaches a handler.
6. **Each call is governed on its own.** A description or hint is information, not authority.
7. **There is one MCP implementation.** The toolchain's MCP commands are `std.mcp` servers. The compiler's analysis libraries do not import `std.mcp`; the commands that serve them do.

## Motivation

The Model Context Protocol is how AI clients call into programs. A server exposes tools, which are functions the model may call; resources, which are readable data addressed by URI; and prompts, which are message templates a user can pick. Each tool advertises a JSON Schema for its inputs.

Today an author writes that schema by hand beside the handler, and the two drift apart. For an agent this is worse than for a person. A call the schema allows can fail on a rule nobody documented, and a harmful tool can look harmless, because descriptions are only text. Incan has the checked declarations to derive all of this, and to keep typed evidence when a call fails.

## Goals

- Serve tools, resources and prompts from ordinary typed functions and methods.
- Generate input and output schemas, decoding and encoding from one declaration.
- Serve over stdio and Streamable HTTP, against MCP 2026-07-28.
- Report bad arguments completely, so a model can correct every field in one retry.
- Check authority and emit a receipt on every call under RFC 104.
- Keep secrets out of schemas, results, errors, logs and cache keys under RFC 103.
- Serve the toolchain's own MCP commands with the same implementation.

## Non-Goals

- Making descriptions or hints grant capabilities.
- Registering handlers as a side effect of importing a module.
- An MCP client. A client would build on `std.http`, and `std.mcp.testing` provides a test client.
- Older protocol revisions, which require the session state that 2026-07-28 removed.

## Guide-level explanation

Every `std.mcp` name below is proposed by this RFC.

### Write a server

This server lets an agent search and close tickets in a company's support desk. `TicketStore` and `DeskError` are the application's own, and the capability it requires is declared in its own module:

```incan
# support_desk/capabilities.incn
capability desk_write:
    description = "Change tickets in the support desk"
    requires = [host.fs.write]
```

```incan
from std.fs import Path
from std.mcp import McpError, Server, ToolContext, ToolHints, tool
from std.serde import json
from support_desk.capabilities import desk_write
from support_desk.store import DeskError, TicketStore

@derive(json)
model TicketSummary:
    id: str
    title: str

@derive(json)
model ClosedTicket:
    id: str
    closed_by: Option[str]

class SupportDesk:
    tickets: TicketStore

    @tool(hints=ToolHints(read_only=true))
    def search_tickets(self, query: str, limit: int = 20) -> Result[list[TicketSummary], DeskError]:
        """
        Search open support tickets.

        Args:
            query: Words to look for.
            limit: Most results to return.
        """
        return self.tickets.search(query, limit)

    @tool(caps=[desk_write])
    async def close_ticket(self, ticket_id: str, note: str) -> Result[ClosedTicket, DeskError]:
        """
        Close a ticket with a resolution note.

        Args:
            ticket_id: The ticket to close.
            note: Why it is being closed.
        """
        return await self.tickets.close(ticket_id, note, by=ToolContext.current().principal)

async def main() -> Result[None, McpError]:
    desk = SupportDesk(tickets=TicketStore.open(Path("desk.db"))?)
    server = Server(
        name="support-desk",
        version="1.4.0",
        tools=[desk.search_tickets, desk.close_ticket],
    )
    return await server.serve_stdio()
```

That is the whole server. The AI client sees two tools, `search_tickets` and `close_ticket`. Each has its docstring's first paragraph as its description and one described input per parameter, and `limit` is optional because it has a default. The model depends on those descriptions, so a server with a missing one refuses to start, and `incan docs check` reports it earlier.

`tools=[...]` is the list of what this server serves, so a read-only server is `tools=[desk.search_tickets]`. Plain functions, methods of different classes and async functions can all appear in one list. `desk.search_tickets` carries `desk` with it, so handlers reach application state without a container or a global. A decorated function still takes its own arguments, so a test calls `desk.search_tickets("refund", limit=5)` directly.

Some things a handler needs do not come from the AI. `ToolContext.current()` gives the running call's cancel signal, deadline, caller identity and a way to report progress, as `telemetry.current_span()` gives the current span. Outside a call it returns an empty context, so a direct test still works.

### What the return type means

The return type decides the reply, so most tools need no wrapper:

| Handler returns | The client receives |
| --- | --- |
| `R` | `structuredContent`, with a JSON text block for older client code |
| `Result[R, E]` | The same for `Ok`; `isError: true` with the error's message for `Err` |
| `Option[R]` | A structured `null` when there is no usable answer, which is not an error |
| `Content` or `list[Content]` | Text, images or resource links, with no output schema |
| `Answer[R]` | `Answer.NeedsInput(...)` asks the client for more input and the client calls again |

### When a call fails

A request that breaks the protocol, such as one naming an unknown tool, gets a protocol error. Bad arguments and handler errors come back as an ordinary `isError` reply, because that is what a model can read and correct. RFC 146's complete reporting means one reply lists every bad input:

```text
isError: true
- limit: expected an integer
- query: this field is required
```

The typed error keeps its identity on the server, in the receipt and the telemetry span, and is never reconstructed from text. Secrets are redacted from everything a client sees. A failing handler never ends the server.

### Authority

`hints=ToolHints(read_only=true)` tells the client what a tool claims about itself. It enforces nothing. `caps=[desk_write]` is RFC 104's requirement: the host grants the capability, for example with `--allow`, and a call without the grant is refused with `isError` and a receipt. Every call leaves a receipt naming the declaration, the scope and the outcome.

### Resources and prompts

Resources and prompts follow the same shape, and each has its own list:

```incan
from std.mcp import PromptMessage, prompt, resource

class SupportDesk:
    tickets: TicketStore

    @resource(uri="ticket://{ticket_id}", mime_type="application/json")
    def ticket(self, ticket_id: str) -> Result[TicketSummary, DeskError]:
        """
        One support ticket.

        Args:
            ticket_id: The ticket to read.
        """
        return self.tickets.get(ticket_id)

    @prompt
    def triage(self, ticket_id: str) -> list[PromptMessage]:
        """
        Triage one ticket.

        Args:
            ticket_id: The ticket to triage.
        """
        return [PromptMessage.user(f"Read ticket {ticket_id} and propose a priority.")]
```

```incan
def build_server(desk: SupportDesk) -> Server:
    return Server(name="support-desk", version="1.4.0", tools=[desk.search_tickets], resources=[desk.ticket], prompts=[desk.triage])
```

A variable in the URI template must be a parameter of the handler. MCP prompt arguments are strings, so a prompt's parameters must be `str` or `Option[str]`. A server that breaks either rule refuses to start.

### Serve over HTTP

`mount` turns the same server into a group for a `std.web` application, which owns serving, authentication and guards:

```incan
from std.web import App

def build_app(server: Server) -> Result[App, McpError]:
    return Ok(App(groups=[server.mount(path="/mcp")?]))
```

Over HTTP, `ToolContext.current().principal` comes from the application's RFC 037 `Authenticated` type. Over stdio it is `None`, because the protocol says stdio servers take credentials from the environment.

### Print statements and stdio

In stdio mode the AI client starts your program and reads its messages from standard output, so a stray `print("debug")` would land in the middle of them and break the connection. While `serve_stdio` runs, everything the program writes to standard output goes to stderr instead, and only the server writes to the protocol stream. `serve_stdio` returns `Ok(None)` when the client closes stdin, which is the protocol's way of shutting down.

### Test a server

`std.mcp.testing` provides an `mcp_client` fixture, as `std.http.testing` provides `http_mock`. It drives a `Server` in-process through real protocol messages, without a transport.

## Reference-level explanation

### Declarations

1. `std.mcp` must be written in Incan over general language facilities. The compiler must have no knowledge of tools, resources or prompts.
2. `tool`, `resource` and `prompt` must be ordinary RFC 036 decorators that work on functions, async functions and, under RFC 128, `self` methods. `tool` must be an overloaded function, so that both `@tool` and `@tool(name=..., hints=..., caps=..., timeout=...)` work, and must take its description only from the docstring. A wire name must default to the function's name.
3. A decorated declaration must keep accepting its own arguments and must remain directly callable.
4. A tool's inputs must be RFC 141's `ArgumentsOf[F]`, so names, types, defaults and order come from the signature and a bound receiver is absent. A parameter whose type is a model must be a nested JSON object. Decoding must use `ArgumentsOf[F]` under RFC 146 and construct it under RFC 122's active policy, so a schema that accepts an input never bypasses validation.
5. An input or output type with no JSON form must fail at compile time as an ordinary bound on `tool`.
6. The docstring's first paragraph must be the description, and its `Args:` entries must be the input descriptions, read through RFC 128's `__signature__()`. RFC 082's `incan docs check` reports stale entries.
7. A server must validate its lists before reading any message, and must fail with `McpError.InvalidDeclaration` naming the declaration on a missing description, an undocumented input, a `bytes` input, a resource template variable with no matching parameter, or a prompt parameter other than `str` or `Option[str]`.

### Catalog

8. `Server` must take `name`, `version`, and `tools: list[Tool]`, `resources: list[Resource]` and `prompts: list[Prompt]`, each defaulting to empty. Under RFC 042 these are trait annotations, so one list must hold any adopters together, including bound methods of different classes.
9. `tool` must return a value that adopts `Tool`. `Tool` must expose `spec(self) -> ToolSpec` and one uniform call, `async def dispatch(self, request: ToolCall) -> ToolReply`, and `Resource` and `Prompt` must do the same for their requests.
10. The server must serve exactly what it was given, and `server.catalog()` must return its specs. A duplicate wire name in one server must fail with `McpError.DuplicateName` before any message is read.
11. Discovery order must be deterministic: the order of the lists.

### Schemas

12. `std.json.json_schema[T]()` must project a type to JSON Schema 2020-12 from the JSON type mapping, for `std.mcp` and for `std.web`'s OpenAPI output alike:

    | Incan type | JSON Schema |
    | --- | --- |
    | `str` | `string` |
    | `int` and exact-width integers | `integer`, with the width's bounds |
    | `float` | `number` |
    | `bool` | `boolean` |
    | `list[T]` | `array` |
    | `dict[str, T]` | `object` with `additionalProperties` |
    | `Option[T]` | not required, and may be `null` |
    | value enum | `enum` |
    | payload enum | `oneOf`, in the mapping's `{"Name": payload}` form |
    | newtype | its underlying type |
    | model | `object` with `required` and `additionalProperties: false` |

13. `additionalProperties: false` must hold because RFC 146 rejects unknown fields by default. RFC 085 constraints must project to schema keywords, such as `pattern` and `minimum`, and RFC 096 `json_schema.*` keys must pass through.
14. A named validator cannot be expressed in JSON Schema. Its declared message must be appended to the property or object description, and it must still run at construction. An unsupported shape must fail at compile time, never become an unconstrained placeholder.
15. A protected field must not expose its value through a schema default or example under RFC 103.

### Handlers and context

16. A handler may be synchronous or async, and a synchronous handler must not prevent other requests from running.
17. `ToolContext.current()` must return the running call's context, and an empty context outside a call. It must provide `cancel`, an RFC 142 `CancellationToken` that fires on `notifications/cancelled` or a closed stream; `deadline: Option[Instant]`, from `timeout=`; `principal: Option[Principal]`; `client`, for display only; `client_capabilities`; `progress(done, total)`, which reports when the request carries a progress token; and `log`, which honors the requested log level.
18. Cancellation must drop the pending await under RFC 142 and RFC 094, with no retry and no rollback.

### Results and failures

19. Replies must follow the table in the guide. Successful structured output must satisfy the advertised output schema.
20. Protocol failures must be JSON-RPC errors: an unknown tool, missing required `_meta` or a malformed call shape is `-32602`; malformed JSON or JSON nested beyond `max_depth` is `-32700`; an oversized message is `-32600`; an unsupported protocol version is `-32022`; a handler panic is `-32603`.
21. Argument decoding failures and handler `Err` values must be tool execution errors with `isError: true`. `content` must hold the RFC 131 message chain, redacted under RFC 103. Structured detail, with paths and case identities, must go under an Incan-owned reverse-DNS `_meta` key, not in `structuredContent`, which must match the output schema.
22. A typed failure must keep its identity locally in the receipt and the span. Secrets must not leak through argument echoes, defaults, cache keys, tracebacks or protocol errors.
23. `serve_stdio` must return `Ok(None)` on stdin's end and `Err(McpError.Transport(cause))` on a transport failure. A handler failure must never end the server.

### Authority and evidence

24. Serving `tools/call` is not `host.tool.invoke`. It must emit a receipt under a `std.mcp` capability, identifying the declaration, the authorized scope, the outcome and the available provenance, without protected plaintext.
25. `caps=` must declare what a call requires under RFC 104, and is also a ceiling on what the call may use while it runs. It must never grant anything, and the runtime must check it on each call. A refused call must be a tool execution error with a receipt.
26. An application state handle must not become an authorization grant.
27. Each request must run in one RFC 095 span that continues the `traceparent` in `_meta`.

### Protocol

28. A server must implement MCP 2026-07-28 and list it in `supportedVersions`. It must answer `initialize` with the error that names its supported versions, and must implement `server/discover`.
29. The server must implement the protocol's message patterns: multi round-trip requests, `subscriptions/listen` acknowledgment, cancellation, progress and logging. Elicitation, sampling and resource-update subscriptions must be advertised only when the catalog uses them.
30. The server must never send an input request of a kind the client did not declare. It must integrity-protect `requestState`, which the protocol treats as attacker-controlled, binding it to the principal, a time limit and a digest of the request with a key the server holds.
31. Resource updates must flow through an RFC 142 `broadcast`. A lagged subscriber must receive `resources/updated` again rather than invented values.
32. `listChanged` must be `false`, because a server's lists cannot change within a process.
33. `tools/list`, `prompts/list` and template lists must carry `cacheScope: "public"` and `ttlMs: 3_600_000`, or `"private"` when any declaration has an authorization guard. `resources/read` must default to `"private"` with `ttlMs: 0` unless the resource declares `ttl=`. Authorization-sensitive results must never be cached as public.

### Transports and limits

34. `serve_stdio(max_message_bytes=4_194_304, max_depth=128, max_concurrent=16)` must read newline-delimited JSON through RFC 146's reader over `std.io.stdin()`. An oversized line must be skipped and answered with `-32600`, after which reading must continue. `max_depth` must follow RFC 152.
35. In-flight requests must run through RFC 142's `for_each_concurrent` with `limit=max_concurrent`, with no scheduler of its own.
36. While `serve_stdio` runs, everything the process and its children write to standard output, including through native libraries, must reach stderr, and only the server must write to the protocol stream.
37. `std.io` must provide `stdin()` as a `BinaryReader` with an async form, and `stdout()`.
38. `server.mount(path, ...)` must return a group that serves Streamable HTTP on an RFC 037 `App`, taking the same limits. Authentication must come from the application's RFC 037 `Authenticated` types and guards into `ToolContext.current().principal`, and must not be reimplemented by `std.mcp`.
39. `mount` and its `std.web` dependency must be separately selectable, so a server that only uses stdio, such as `incan mcp`, does not carry the web stack.

### Conformance workloads

| Workload | Required evidence |
| --- | --- |
| Official suite | The official `modelcontextprotocol/conformance` server suite passes against a mounted server in CI. |
| One contract | Every declared input and output has a schema matching what decoding accepts, across every type in the mapping; unsupported shapes fail at compile time. |
| Descriptions | A server with a missing or misnamed `Args:` entry refuses to start, and `incan docs check` reports the same entry. |
| Library code | `std.mcp` builds using only documented language facilities, with no compiler knowledge of MCP. |
| Mixed lists | One list holds plain functions, async functions and bound methods of different classes, and a decorated function remains directly callable. |
| Failure mapping | Each protocol failure has its code; bad arguments list every bad input; a panic does not end the server. |
| Stream safety | A handler that prints, a native library that writes to standard output and a child process do not corrupt the stdio stream. |
| Limits | An oversized line, deep JSON and more than `max_concurrent` requests behave as specified, and the next message is read. |
| Authority | A call without its capability is refused with a receipt; hints grant nothing; tampered `requestState` is refused. |
| Secrets | A secret never appears in a schema, a result, an error, a log or a cache key. |
| Toolchain | The toolchain's MCP commands are `std.mcp` servers passing the same suite. |

## Design details

### Why parameters

Ordinary parameters are what an author writes first, and they read like every other function. Tests call them directly, and the docstring that documents the function is the description the model reads, so there is nothing to keep in step. Incan's standard library already writes `Args:` sections, and RFC 082 already checks them. The Python SDK takes the same approach, with a type-hinted function and its docstring.

Decoding needs one type, and RFC 141's `ArgumentsOf[F]` is that type: a model of `F`'s parameters that a call can be made from. It is a general type operation beside `ReturnOf`, which any library can use. `std.web` needs the same thing, because RFC 037's `async def get_user(id: int)` has to turn a request into arguments.

### Why a list

The list is explicit, so what a process serves is visible at the call that serves it. A server can offer a subset, and it can mix functions and methods from several places. RFC 042 already says a trait name may appear anywhere a type annotation is allowed, so `list[Tool]` is the ordinary spelling. The registry from RFC 113 answers a different question, what is declared in a package, and it takes only constant values, so it cannot hold a running server's tools.
### One implementation

RFC 167 requires its debugging operations over MCP, and RFC 106 and RFC 118 define the toolchain's MCP commands. Two implementations would mean two conformance targets. The commands that serve those surfaces are `std.mcp` servers, so the toolchain's serving commands depend on `std.mcp`, while the compiler's analysis libraries stay free of it. RFC 167 uses bounded, versioned structured results, continuation handles, cancellation and progress from this surface.

### Alignment with other RFCs

Application resources are scoped with RFC 094's `with` around serving. `std.mcp` composes `std.json`, `std.web`, `std.async` and `std.runtime`. The word "tool" here means an MCP tool, not RFC 078's Oven tools.

### Prior art

The Python SDK derives schemas from type hints and takes descriptions from docstrings, and binds resource template variables to parameters. The TypeScript SDK registers tools with Standard Schema validators, which an author writes beside the handler. The Go SDK generates schemas from one typed input struct. Rust's `rmcp` generates them through `schemars` and targets the same protocol revision. The official conformance suite runs server scenarios against a URL.

## Alternatives considered

- **Handwritten JSON-RPC and JSON Schema:** duplicates checked declarations and invites drift.
- **Compiler support for tools:** a compiler that knows `@tool` could report more at compile time, but only MCP would benefit, and every other library would still have to write this by hand.
- **One argument model per tool, as the Go SDK does:** needs no new language, but every tool needs a model even for one input, and `std.web` needs `ArgumentsOf` either way. A model remains available as a nested parameter.
- **A `ToolContext` parameter recognized by its type:** the decorator would have to treat one parameter type specially. `ToolContext.current()` is ordinary code, as `telemetry.current_span()` is.
- **Recording tools in `std.registry`:** `@describe` takes only constant values, and it would add a second catalog beside the list.
- **A class as the catalog:** the server would read marked methods from one object. A list is explicit, can serve a subset, and mixes sources; bound methods already give handlers their state.
- **Wrapping each entry, `tools=[tool(a), tool(b)]`:** repeats what the decorator already says.
- **Registering handlers on import:** makes the advertised surface depend on what happened to load.
- **Serving whatever the registry holds:** a registry describes what is declared, not what one server chooses to serve.
- **Schemas through a general adapter layer:** the JSON type mapping already exists, and `std.web` needs the same projection.
- **Refusing `print` at compile time:** cannot see native libraries or child processes, so it would still let the stream be corrupted.
- **Supporting legacy `initialize` with sessions:** brings back the session state the 2026-07-28 revision removed.
- **Separate MCP implementations for programs and the toolchain:** two conformance targets for one protocol.

## Drawbacks

Protocol revisions change, so `supportedVersions` needs attention with each one. A library cannot raise a compile error, so a missing description or a bad resource template surfaces when the server starts, or earlier through `incan docs check`. Requiring a description for every input is stricter than the protocol, and a quick internal tool has to write them. Redirecting standard output means a program's own output moves while it serves. A named validator is only described in a schema, so a client can pass the schema and still fail it.

## Implementation architecture (non-normative)

`tool` is a few dozen lines of ordinary Incan in `std.mcp`, with one overload for bare `@tool` and one for `@tool(...)`:

```incan
# std/mcp/tools.incn
from std.json import json_schema
from std.reflection import ArgumentsOf, ReturnOf, SignatureOf
from std.serde.json import Deserialize

pub trait Tool:
    def spec(self) -> ToolSpec: ...
    async def dispatch(self, request: ToolCall) -> ToolReply: ...

pub class ToolFunction[F where ArgumentsOf[F] with Deserialize] with Tool, Callable[SignatureOf[F]]:
    handler: F
    options: ToolOptions

    def __call__(self, **args: ArgumentsOf[F]) -> ReturnOf[F]:
        return self.handler(**args)

    def spec(self) -> ToolSpec:
        return ToolSpec.of(
            self.handler.__signature__(),
            json_schema[ArgumentsOf[F]](),
            json_schema[ReturnOf[F]](),
            self.options,
        )

    async def dispatch(self, request: ToolCall) -> ToolReply:
        match ArgumentsOf[F].from_json(request.arguments):
            Ok(args) => return to_reply(await invoke(self.handler, args))
            Err(error) => return ToolReply.invalid(error)

pub def tool[F where ArgumentsOf[F] with Deserialize](handler: F) -> ToolFunction[F]:
    return ToolFunction(handler=handler, options=ToolOptions())

pub def tool[F where ArgumentsOf[F] with Deserialize](
    *,
    name: Option[str] = None,
    hints: ToolHints = ToolHints(),
    caps: list[Capability] = [],
    timeout: Option[Duration] = None,
) -> Callable[(F) -> ToolFunction[F]]:
    options = ToolOptions(name=name, hints=hints, caps=caps, timeout=timeout)
    return (handler) => ToolFunction(handler=handler, options=options)
```

`invoke` awaits an async handler and calls a synchronous one directly, chosen by an RFC 141 type match on `F`. `to_reply` is a small trait adopted for each result shape in the guide's table. The server checks `caps`, enters the `ToolContext` and opens the span around `dispatch`.

Three general language facilities make this possible, and none of them knows about MCP:

- `ArgumentsOf[F]`, `handler(**args)` and `**args: ArgumentsOf[F]`, from RFC 141.
- A function's name and docstring through `__signature__()`, from RFC 128.
- Decorators that return a wrapper on `self` methods, from RFC 128.

## Layers affected

- **Language:** the facilities above in RFCs 128 and 141, and lists holding different adopters of one trait under RFC 042.
- **Stdlib MCP:** declarations, server, context, replies, receipts and the test client.
- **Stdlib JSON:** `json_schema[T]()`, shared with `std.web`.
- **Stdlib I/O:** `stdin()` and `stdout()`.
- **Stdlib web:** mounting, and explicit registration in place of link-time route registration, so every standard-library server follows one rule.
- **Toolchain:** the MCP commands of RFC 106, RFC 118 and RFC 167 as `std.mcp` servers.
- **Documentation:** references, and the conformance workloads above.

## Design Decisions

- **No compiler magic:** `tool`, `resource` and `prompt` are ordinary decorators in `std.mcp`, built on general language facilities. The compiler knows nothing about MCP.
- **Inputs:** ordinary parameters, with descriptions from the docstring. RFC 141's `ArgumentsOf[F]` is the one type decoding uses, so bad arguments are reported completely. A model is available as a nested parameter.
- **Catalog:** `tools=[...]`, `resources=[...]` and `prompts=[...]` are `list[Tool]`, `list[Resource]` and `list[Prompt]`, explicit and able to mix functions and bound methods. Imports register nothing, and `server.catalog()` reads the lists.
- **Context:** `ToolContext.current()` gives the running call's cancellation, deadline, identity and progress, and an empty context outside a call.
- **Declaration errors:** types with no JSON form fail at compile time as ordinary bounds. A missing description or bad resource template stops the server before it reads a message, and `incan docs check` reports stale docstrings earlier.
- **Replies:** the return type decides, with `R`, `Result[R, E]`, `Option[R]`, `Content` and `Answer[R]`.
- **Failures:** protocol errors for broken requests; `isError` for bad arguments and handler errors. Identity stays on the server.
- **Schemas:** a projection of the JSON type mapping from `std.json`, shared with `std.web`. Validators are described, not faked, and unsupported shapes are compile errors.
- **Standard output:** while `serve_stdio` runs, all of the process's standard output goes to stderr.
- **Protocol:** MCP 2026-07-28 only, with `initialize` answered by the error naming supported versions.
- **Transports:** stdio, and Streamable HTTP mounted on `std.web`, which owns authentication.
- **Limits:** `max_message_bytes` of 4 MiB, `max_depth` of 128 and `max_concurrent` of 16, as keyword arguments.
- **Caching:** public for a fixed catalog unless authorization applies; private and uncached for resource reads unless declared.
- **Multi round-trip:** the framework implements the message patterns and protects `requestState`. Handler-facing features are advertised only when used.
- **One implementation:** the toolchain's MCP commands are `std.mcp` servers, and the compiler's analysis libraries do not import it.
- **Evidence:** the official conformance suite and an `mcp_client` test fixture.
