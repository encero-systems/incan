# RFC 003: Frontend and WebAssembly Support

- **Status:** Superseded
- **Created:** 2025-12-16
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 020 (offline locked reproducible builds)
    - RFC 031 (library system phase 1)
    - RFC 037 (native web and HTTP stdlib redesign)
    - RFC 092 (interactive runtime stdlib contracts)
- **Issue:** https://github.com/dannys-code-corner/incan/issues/312
- **RFC PR:** —
- **Written against:** v0.1
- **Shipped in:** —

> **Superseded:** This RFC began as *Frontend & WebAssembly Support* and later tried to absorb a broader interactive-runtime direction. That made the RFC carry too many responsibilities at once: browser UI, WASM, packaging, runtime target metadata, GPU/graphics, and compatibility with higher-level experience frameworks. The successor direction is to keep Incan RFCs focused on Incan-owned language, stdlib, compiler, and artifact contracts. RFC 092 extracts the stdlib/runtime contracts needed by future interactive runtime consumers. This file remains as provenance for the original frontend/WASM exploration and should not be treated as the active implementation plan.

## Summary

This RFC defines how Incan approaches **interactive runtime targets**. **Phase 1** is a **single mandatory browser-facing phase** that combines what was previously split as “server first” and “browser UI”: authors ship **real interactive UI in the browser** (components, surfaces, or equivalent), **explicit server vs client boundaries**, typed HTTP handlers, state-changing operations, packaging, and a documented **first browser emission target**. The server remains authoritative for policy and default mutations unless authors opt into client execution. **Phase 2 (optional)** is WASM-first client compilation, optional `html()`/`jsx()`-style syntax, routing/tooling, advanced graphics, and non-browser/native-class runtime expansion as detailed or superseded by follow-up runtime work; it does **not** gate Phase 1. Normative outcomes, dependency on RFC 037’s web/HTTP stdlib, and unblock gates apply primarily to Phase 1. Appendix crate choices and CLI flags are **not** normative for Phase 1 unless explicitly promoted from the appendix into Phase 1 text.

## Motivation

### Beyond “client-only WASM”

Developers still face **split stacks**: one language or framework on the server, another in the browser, duplicated types, and ad hoc integration. Incan’s opportunity is **single-language authoring** with **predictable lowering**. The browser remains a **JavaScript host** for most interactive UI; not every product should pay for large **WASM-first** client bundles when server-rendered, API-first, or progressively enhanced shapes are cheaper to operate.

A credible path includes:

- **Server-primary execution** for auth-aware mutations, business rules, typed HTTP APIs, and page assembly where security and latency demand it.
- **Browser-oriented output** for typical interactivity (hydration boundaries, component trees, progressive enhancement as appropriate).
- **Selective WASM** only where **client-side** performance or portability clearly wins—WASM is an **emission option**, not the platform posture.

That preserves the core promise: **one type system, one compiler, fewer hand-glued stacks**—without pretending the browser is always the right place for app logic.

### Runtime targets, not only browser widgets

Organizations need **apps, dashboards, operator consoles, assistants, and external APIs** that stay consistent with typed domain logic and policy. That requires more than a browser widget library: Incan needs a target model for rendering, state, host bindings, runtime diagnostics, packaging, and browser/native/server-assisted deployment. This RFC therefore treats the browser as the **first target**, not the whole architecture. Higher-level experience frameworks can then consume that runtime capability instead of absorbing primitive runtime mechanics.

## Goals

**Phase 1** must make the following **true** for at least one documented browser-product golden path (aligned with tutorials, RFC 037’s intended handler model, and an explicit higher-layer/runtime boundary):

- Authors can define **multiple HTTP routes** on one process (JSON and/or HTML-style responses) with **types** carried through handler signatures and framework-owned conversion where the stdlib provides it.
- Authors can perform **state-changing server operations** (forms or RPC-shaped handlers) with **stable error mapping** to HTTP (status + body shape documented, not ad hoc per sample).
- The toolchain produces a **named deployable unit**: declared entrypoint(s), declared static asset roots or manifests, and **machine-readable metadata** for at least one environment dimension (e.g. dev vs prod flags or equivalent), without requiring authors to hand-assemble Cargo-only layouts for every app.
- **Session, identity, and authorization** compose via **documented extension points** (middleware, context, or decorator-shaped hooks—exact spelling follows RFC 037 and stdlib), not by copying unrelated logic into each handler.
- Authors ship **interactive browser UI** as part of the same golden path: a **component or page runtime** with a **documented first browser target** (framework-shaped output, minimal runtime, or hybrid—see [Unresolved questions](#unresolved-questions)), **explicit server-only vs client-capable regions** so secrets and policy do not leak by default, and **data loading and forms** wired to Phase 1 handlers. Business rules and default mutations remain **server-authoritative** unless authors opt into client execution explicitly.
- The boundary between **runtime mechanics** and **higher-level surface semantics** is explicit enough that downstream frameworks can build on the runtime target without owning rendering, state, host bindings, or packaging themselves.

**Phase 2** remains **optional**: WASM-first bundles, compiler-native `html()`/`jsx()`, client-side routing as in the appendix, dev-server/HMR focused on WASM, WebGPU-style 3D, and related tooling—**spikes** until promoted by a future status change or a narrowed RFC or supplement.

## Non-goals

- **WASM everywhere** or “all logic in the browser bundle” as a design constraint.
- Redefining **Incan core** semantics here (traits, modules, general syntax). HTTP and handler ergonomics are owned by RFC 037 and the stdlib; this RFC **depends on** that direction rather than duplicating it.
- A **second semantic or orchestration engine** inside the UI layer—surfaces must **call into** typed domain and platform capabilities instead of re-embedding them.
- Making higher-level experience frameworks responsible for primitive rendering, state/event mechanics, host bindings, target packaging, or emitted artifact shape. Those belong below such frameworks in the runtime substrate.
- Committing to every **Phase 2** appendix detail (specific crates, `--target wasm`, native `jsx()` parsing) as part of Phase 1; Phase 1 requires **a** browser UI story, not necessarily the appendix implementation.

## Guide-level explanation

Authors think in **four layers**: (1) **domain and policy**—ordinary Incan types and functions; (2) **HTTP surface**—routes, handlers, and serialization shaped by the stdlib web platform (see RFC 037); (3) **interactive runtime target**—rendering abstraction, state/event model, host bindings, target packaging, and emitted browser/native/server-assisted artifacts; (4) **product surface semantics**—how downstream experience frameworks expose capability, evidence, policy, and context to people or systems. **Phase 1** delivers a browser slice of (2)+(3) that downstream frameworks can build on: **shippable web apps with UI**, not “API-only until later.” **Phase 2** adds **optional** WASM-heavy, appendix-style client stacks, and broader target work. Stable Phase 1 **must** include whatever surface is chosen for the **first browser target**; advanced appendix features remain optional until promoted.

## Normative contracts by phase

**Phase 1 — must**

- The compiler and project metadata **must** support building a **single deployable server binary** (or equivalent documented entry) from Incan sources and **shipping browser-oriented artifacts** required by the chosen first browser target (bundles, islands, surface artifacts, or framework output—exact shape documented in the golden path), **without** requiring Phase 2-only features such as `--target wasm` unless the project explicitly opts into them.
- Handler and routing behavior **must** remain consistent with **RFC 037** as that RFC matures; this RFC **must not** specify competing handler semantics—only product-surface packaging, UI boundaries, and phasing around it.
- Error and response contracts for handlers **must** be **documented** for the golden path (what callers receive on success and on typed failures).
- Any browser-emitted code **must** declare **which** code runs only on server, only on client, or both, so secrets and policy do not leak by default.
- Runtime target artifacts **must** be inspectable enough for downstream surface layers to understand routes, interactions, browser assets, and server/client boundaries without reverse-engineering framework glue.

**Phase 1 — should**

- Static assets **should** be referenced in a way reproducible builds (RFC 020) and library layout (RFC 031) can validate or copy.
- Examples **should** share one **composition pattern** for auth/session, not divergent one-off globals.

**Phase 2 — may**

- The appendix **may** inform spikes; nothing in the appendix **must** hold for Phase 1 compliance.

## Layering

- **Incan** — syntax, types, modules, lowering to Rust, shared stdlib patterns.
- **HTTP/web stdlib (RFC 037)** — typed routes, handlers, serialization, server-side mutations/actions, and response/error contracts.
- **Interactive runtime substrate** — rendering abstraction, state/event model, host bindings, target packaging, emitted artifacts, runtime diagnostics, accessibility hooks, and input model.
- **Experience surfaces** — apps, dashboards, assistants, review/approval surfaces, APIs, context-preserving interaction, evidence-preserving interaction, and policy-aware presentation.
- **Managed hosting and org-scale operation** — orthogonal: preview envs, rollout, centralized observability, tenancy are **additive** around the open model, not prerequisites for authoring.

## Delivery strategy

- Prefer **server-side Incan or generated Rust** for mutations, typed HTTP APIs, SSR or server-assembled pages, and policy-sensitive logic.
- Emit **browser-oriented assets** for the first target; treat hydration, cookies, CSRF, CORS, accessibility hooks, and input boundaries as **platform-level** concerns where possible.
- Treat **WASM client compilation** (`--target wasm`, `wasm-bindgen`, etc.) as a **targeted** option in the emission matrix—not the default product shape.
- Allow multiple **delivery modes** over time: server-rendered apps, API-first products, progressively enhanced tools, hybrid UI + API surfaces, richer client-heavy experiences where justified, and non-browser/native-class targets once the runtime model is ready.

## Phased roadmap

### Phase 1 — Browser product surface as first runtime target

Single mandatory phase: **server-authoritative** HTTP and mutations **plus** **interactive UI in the browser** on one documented golden path—no separate “server milestone” before UI, and no claim that browser is the only eventual runtime target.

- Typed HTTP handlers and external API surfaces. Until RFC 037 stabilizes, **hand-wired** routing is acceptable only if the **golden path** in docs uses one **documented** router integration; the RFC **must** be updated when the stable shape is chosen.
- Server-side **mutations or actions** (exact surface follows RFC 037 / stdlib) with **predictable errors** and **hooks** for auth and policy. Tutorials today may map to a concrete Rust web stack; that mapping is **illustrative**, not the permanent ABI.
- **App packaging**: entrypoints, static asset pointers, environment/preview **metadata**—a deployable **unit**, not only a loose crate.
- **Session, identity, and access** described as **composition** targets shared across pages, handlers, and APIs—not reimplemented per sample.
- **Browser UI/runtime target**: component, page, or surface **rendering** story, **first browser target** locked for the golden path (see [Unresolved questions](#unresolved-questions)), **server vs client** boundaries enforced by convention or language/tooling, and **data loading + forms** integrated with the handlers above.
- **Experience-framework compatibility**: runtime artifacts and boundaries are explicit enough that a downstream surface layer can project capability and interaction evidence on top without owning the primitive runtime.

#### Default Phase 1 vertical slice (proposal — refine before unblock) {#default-phase-1-vertical-slice}

This slice is the **minimum** that should count as “product with UI, not toy API or static HTML only”:

1. One runnable server **binary** built from Incan (plus generated Rust as today).
2. At least **three routes**: one `GET` returning HTML or template-backed content, one `GET` returning JSON (or typed serializable body), one `POST` (or mutation-shaped handler) accepting a body and returning success or typed error mapped to HTTP.
3. **Static assets** (e.g. CSS, client JS, or framework bundles) referenced from the app manifest or metadata such that a deploy step can gather them without manual path lists per route.
4. One **cross-cutting** concern (session or auth stub) applied through the **same** mechanism across all three routes in the golden-path example.
5. At least one **interactive browser-driven** flow (e.g. in-page interactivity, client navigation, hydrated island, or governed interaction surface) implemented with the **chosen Phase 1 browser target**, with documentation of **what executes on the server vs in the browser**.
6. An inspectable artifact or manifest describing routes, browser assets, runtime target, and interaction boundaries well enough for downstream experience-surface projection.

*Phase 1 success:* the golden-path app above is **usable as a real small web app with UI**, satisfies the [Phase 1 checklist](#checklist) and the [vertical slice](#default-phase-1-vertical-slice) (or successor), and **does not** depend on Phase 2 (`--target wasm`, appendix-native `html()`/`jsx()`, WebGPU, or non-browser native targets) unless the golden path explicitly adopts them.

### Phase 2 (optional) — broader runtime targets, WASM-first client, advanced templates, and graphics

The **Reference design** below (WASM target, signals, string or `jsx()` templates, client-centric routing, dev server, WebGPU-style 3D) belongs here. Broader runtime-target work may supersede pieces of it. It supports **selective** high-performance or ergonomics-driven client work; it **does not** gate Phase 1.

## Alternatives considered

- **WASM-first as the only UI story** — Rejected as the **default** for Phase 1 because it couples UI delivery to the heaviest client stack; WASM-first remains **Phase 2** for teams that need it.
- **SPA-only** (no first-class server HTML or authoritative mutations) — Rejected because it conflicts with server-authoritative policy and excludes API-first backends and operator tools.
- **A higher-level experience framework owns the full runtime** — Rejected because experience frameworks should not own primitive rendering, state/event mechanics, host bindings, or target packaging.
- **Deferring packaging to Cargo-only** — Rejected for Phase 1 **success**: authors need a **named app unit** and asset story to deploy confidently; raw crates alone are insufficient as the long-term contract.

## Drawbacks

- **Phasing adds coordination cost**: RFC 037, this RFC, and tooling must move together so handler semantics and packaging do not diverge.
- **Two audiences**: teams shipping only Phase 1 may ignore the Phase 2 appendix entirely; maintainers must keep the **normative** sections short and the **exploratory** appendix clearly labeled.
- **Blocking status** delays **Phase 2** appendix work; Phase 1 still requires **a** browser runtime target and golden path before thrashing compiler, stdlib, downstream framework, and runtime-substrate work.

## Unresolved questions {#unresolved-questions}

**Blocking (must answer to unblock this RFC or move to Planned/In Progress)**

- Final **Phase 1 golden path** alignment with RFC 037: single router story, decorator shapes, and error mapping once RFC 037 advances.
- **HTTP API (endpoint)** vs **browser/form mutation (action)**: distinct constructs indefinitely, or two **policy profiles** over one handler model?
- **First browser target** for Phase 1 (defines how interactive UI ships): framework-shaped output, minimal custom runtime, surface-artifact projection, or hybrid?
- **Runtime-substrate boundary**: which mechanics are owned below downstream experience frameworks (rendering, state/event model, host bindings, packaging, diagnostics, input/accessibility hooks), and which are exposed as framework-facing hooks?
- **Experience-framework compatibility contract**: what route, interaction, evidence, and server/client boundary metadata must the first browser target emit so downstream frameworks can build on it without becoming the runtime?

**Non-blocking (may track in RFC 037 or follow-up RFCs)**

- How much **page-level data loading** is first-class syntax vs idiomatic Incan + libraries.
- Which **preview, rollback, and dependency-inspection** behaviors live in open tooling vs optional hosted layers.

## Relationship to other RFCs and the roadmap

- RFC 037 ([Native Web and HTTP Stdlib Redesign](../../037_native_web_stdlib_redesign.md)) — complementary; Phase 1 handler semantics and stdlib shape **must** evolve together with this RFC.
- RFC 020 ([Offline Locked Reproducible Builds](../implemented/020_offline_locked_reproducible_builds.md)) and RFC 031 ([Library System Phase 1](../implemented/031_library_system_phase1.md)) — multi-target and reproducible builds **must** stay coherent as server and optional client emission land.
- Downstream experience frameworks — directional input for the upper surface boundary; these frameworks should consume inspectable runtime artifacts and own product interaction semantics, not primitive runtime mechanics.
- Interactive runtime substrates — directional input for the lower runtime boundary; they should own rendering/state/host/packaging mechanics that RFC 003 must not accidentally assign to higher-level surface frameworks.
- [Roadmap — deferred / later](../../../roadmap.md) — SSR/SSG emphasis, desktop/mobile wgpu, CRDT/collab remain explicitly **later**; Phase 1 subsume items previously listed only under WASM for **shippable UI**.

## Layers affected

- **Parser** — Phase 2 (appendix) for native `html()`/`jsx()` and related syntax; Phase 1 may introduce route or manifest syntax as needed; boundary annotations may require new surface forms.
- **Typechecker** — handler typing; server/client boundary annotations for Phase 1 UI.
- **Lowering / emission** — server binary layout, **first browser target** emission, optional Phase 2 second target for WASM; packaging metadata emission.
- **Stdlib / runtime** — HTTP, sessions, serialization (per RFC 037); **client UI runtime** for Phase 1; Phase 2 extras as needed.
- **Tooling** — build, dev server, asset and bundle pipeline; LSP may need route and boundary awareness.

## Why this RFC is blocked

Blocked until **Phase 1** is defined end-to-end: **HTTP + mutations + packaging + browser UI/runtime target** on one golden path, with **first browser target** chosen at a **product-engineering** level (not only a compiler spike) and with downstream-framework/runtime-substrate ownership separated. Concrete gates:

1. A **minimal web product surface with UI** matching the [**Default Phase 1 vertical slice**](#default-phase-1-vertical-slice) (or an explicitly amended successor) is agreed and reflected in docs for implementers.
2. **Browser emission** for that golden path is chosen and documented (framework, minimal runtime, surface-artifact projection, or hybrid).
3. Runtime-substrate mechanics and downstream experience-surface semantics are separated enough that higher-level frameworks can build on the target without owning primitive runtime concerns.
4. RFC 020 / generated-project contracts and RFC 031 remain satisfied for multi-target builds.

Unblocking can proceed **incrementally**: Phase 1 may ship **without** Phase 2 (`--target wasm`, appendix-native `jsx()`, WebGPU).

---

## Reference design: Phase 2 (exploratory — WASM-heavy client compilation)

The following sections preserve the **earlier** RFC 003 sketch: WASM as a compilation target, reactive UI (`html()`, `jsx()`), routing, build tooling, and optional WebGPU-style 3D. Use it for **spikes** and Phase 2 planning; Phase 1 **may** adopt pieces of it, but the appendix is **not** the sole definition of Incan’s web UI.

### Part 1: WASM Compilation Target

Add a `--target wasm` flag to the Incan compiler:

```bash
incan build --target wasm app.incn
```

This generates:

- Rust code with `wasm-bindgen` annotations
- `Cargo.toml` configured for `wasm32-unknown-unknown`
- Build artifacts ready for browser deployment

#### Generated Structure

```bash
target/wasm/my_app/
├── Cargo.toml
├── src/
│   └── lib.rs          # Generated Rust + wasm-bindgen
├── pkg/                # wasm-pack output
│   ├── my_app.js
│   ├── my_app_bg.wasm
│   └── my_app.d.ts
└── index.html          # Dev server entry
```

#### Type Mapping for WASM

| Incan         | Rust (WASM)    | JS                    |
| ------------- | -------------- | --------------------- |
| `str`         | `String`       | `string`              |
| `int`         | `i64` / `i32`  | `number` / `BigInt`   |
| `float`       | `f64`          | `number`              |
| `bool`        | `bool`         | `boolean`             |
| `list[T]`     | `Vec<T>`       | `Array`               |
| `dict[K,V]`   | `HashMap<K,V>` | `Object` / `Map`      |
| `Option[T]`   | `Option<T>`    | `T` or `null`         |
| `Result[T,E]` | `Result<T,E>`  | `T` (throws on `Err`) |

---

### Part 2: UI Framework (React Alternative — exploratory)

A reactive component model for building web UIs.

#### Component Syntax

```incan
from incan.ui import component, signal, html, Element

@component
def counter(initial: int = 0) -> Element:
    """A simple counter component."""
    count, set_count = signal(initial)
    
    def increment() -> None:
        set_count(count + 1)
    
    def decrement() -> None:
        set_count(count - 1)
    
    return html("""
        <div class="counter">
            <span>Count: {count}</span>
            <button on:click={increment}>+</button>
            <button on:click={decrement}>-</button>
        </div>
    """)
```

> **Note**: For simple inline handlers, arrow syntax is also supported:
> `<button on:click={() => set_count(count + 1)}>+</button>`

#### Reactive State: Signals

Signals provide fine-grained reactivity (like SolidJS/Leptos):

```incan
from incan.ui import signal, computed, effect

# Create reactive state
name, set_name = signal("World")

# Derived state (auto-updates when dependencies change)
greeting = computed(() => f"Hello, {name}!")

# Side effects
effect(() => println(f"Name changed to: {name}"))

# Update triggers recomputation
set_name("Incan")  # Logs: "Name changed to: Incan"
```

#### HTML Templating

Embedded HTML with Incan expressions:

```incan
return html("""
    <div class={active ? "active" : ""}>
        <!-- Conditionals -->
        {
        if logged_in:
            <UserProfile user={user} />
        else:
            <LoginForm />
        }
        
        <!-- Loops -->
        <ul>
            {
            for item in items:
                <li key={item.id}>{item.name}</li>
            }
        </ul>
        
        <!-- Event handlers -->
        <button on:click={handle_click}>Click me</button>
        <input on:input={(e) => set_value(e.target.value)} />
    </div>
""")
```

#### Component Props and Children

```incan
from incan.ui import component, html, Element, Children

@component
def Card(title: str, children: Children) -> Element:
    return html("""
        <div class="card">
            <h2>{title}</h2>
            <div class="content">
                {children}
            </div>
        </div>
    """)

# Usage
html("""
    <Card title="My Card">
        <p>This is the card content.</p>
    </Card>
""")
```

#### Lifecycle and Effects

```incan
from incan.ui import component, signal, effect, html, Element
import std.async

@component
def dataFetcher(url: str) -> Element:
    data, set_data = signal(None)
    loading, set_loading = signal(True)
    
    # Runs on mount and when url changes
    effect(async () => (
        set_loading(True)
        result = await fetch(url)
        set_data(result)
        set_loading(False)
    ), deps=[url])
    
    return html("""
        {
        if loading:
            <Spinner />
        else:
            <DataView data={data} />
        }
    """)
```

#### Routing

```incan
from incan.ui import component, Router, Route, Link, Element

@component
def app() -> Element:
    return html("""
        <Router>
            <nav>
                <Link to="/">Home</Link>
                <Link to="/about">About</Link>
                <Link to="/users/{id}">User</Link>
            </nav>
            
            <Route path="/" component={Home} />
            <Route path="/about" component={About} />
            <Route path="/users/{id}" component={UserProfile} />
        </Router>
    """)
```

---

### Part 2b: JSX Template Syntax (Alternative)

As an alternative to `html()` string templates, Incan supports JSX (JavaScript XML) syntax via the `jsx()` wrapper. This provides a more familiar experience for developers coming from React/TypeScript, with full IDE support.

#### Why a `jsx()` Wrapper?

Raw JSX in Incan would create parser ambiguity:

```incan
result = <div>content</div>    # JSX? Or...
result = x < y                  # Less-than comparison?
```

The `jsx()` wrapper solves this by explicitly marking JSX regions:

```incan
return jsx(
    <div>content</div>
)
```

This approach:

- **No parser ambiguity** — content inside `jsx()` is parsed as JSX
- **IDE support** — editors know to provide JSX highlighting/completion
- **Explicit** — follows Incan's "explicit is better than implicit" philosophy
- **Similar to Rust** — mirrors Leptos's `view!` macro approach

#### Comparison

| Approach          | Syntax          | IDE Support | Parser Complexity |
| ----------------- | --------------- | ----------- | ----------------- |
| `html("""...""")` | String template | Limited     | Simple            |
| `jsx(...)`        | Native JSX      | Full        | Moderate (scoped) |

Both compile to the same UI intermediate representation — choose based on preference.

#### JSX Syntax in Incan

```incan
from incan.ui import component, signal, jsx, Element

@component
def counter(initial: int = 0) -> Element:
    count, set_count = signal(initial)
    
    def increment() -> None:
        set_count(count + 1)
    
    def decrement() -> None:
        set_count(count - 1)
    
    return jsx(
        <div class="counter">
            <span>Count: {count}</span>
            <button onClick={increment}>+</button>
            <button onClick={decrement}>-</button>
        </div>
    )
```

#### Expressions in JSX

> **Note**: The following examples show content *inside* a `jsx()` wrapper for brevity.

```incan
# Variables
<span>{user.name}</span>

# Expressions
<div class={is_active ? "active" : "inactive"}>
    {items.len()} items
</div>

# Function calls
<span>{format_date(created_at)}</span>
```

#### Conditionals

```incan
# If expression
<div>
    {
    if logged_in:
        <UserProfile user={user} />
    else:
        <LoginForm />
    }
</div>

# Match expression
<div>
    {
    match status:
        case Status.Loading: <Spinner />
        case Status.Error(msg): <ErrorMessage message={msg} />
        case Status.Success(data): <DataView data={data} />
    }
</div>
```

#### Loops

```incan
<ul>
    {
    for item in items:
        <li key={item.id}>
            {item.name} - ${item.price}
        </li>
    }
</ul>

# With index
<ol>
    {
    for i, item in enumerate(items):
        <li>{i + 1}. {item.name}</li>
    }
</ol>
```

#### Event Handlers

**Preferred**: Named handler functions

```incan
def handle_click() -> None:
    println("Button clicked!")

def handle_input(e: Event) -> None:
    set_text(e.target.value)

def handle_key(e: KeyboardEvent) -> None:
    if e.key == "Enter":
        submit()

# Reference handlers by name
<button onClick={handle_click}>Click me</button>
<input value={text} onInput={handle_input} onKeyDown={handle_key} />
<form onSubmit={handle_submit}>...</form>
```

**Alternative**: Arrow syntax for simple inline cases

```incan
<button onClick={() => set_count(count + 1)}>+</button>
<input onInput={(e) => set_text(e.target.value)} />
```

#### Components in JSX

```incan
# Using components
<Card title="Welcome">
    <p>Hello, {user.name}!</p>
</Card>

# With spread props
<Button {...button_props} />

# Conditional rendering
<div>
    <Header />
    {if show_sidebar: <Sidebar />}
    <Main />
    <Footer />
</div>
```

#### Fragments

```incan
from incan.ui import component, jsx, Element

# Return multiple elements without wrapper
@component
def list_items() -> Element:
    return jsx(
        <>
            <li>Item 1</li>
            <li>Item 2</li>
            <li>Item 3</li>
        </>
    )
```

#### Style Handling

```incan
# Inline styles (dict)
<div style={{"color": "red", "fontSize": "16px"}}>
    Styled text
</div>

# Dynamic classes
<div class={["base", active ? "active" : "", error ? "error" : ""]}>
    Content
</div>

# CSS modules (future)
<div class={styles.container}>
    ...
</div>
```

#### Parser Implementation Notes

Incan supports two wrapper modes: `html()` and `jsx()`. Both compile to the same UI intermediate representation, but they parse differently: `html()` takes a string, while `jsx()` is native syntax the IDE can understand.

- `html()` takes a string:

    ```incan
    return html("""<div>{message}</div>""")
    #            ↑ This is a string literal
    ```

    The content inside `html()` is a **string** that gets parsed at compile time. Your IDE sees a string, not markup.

- `jsx()` enables native syntax (inspired by React's JSX syntax):

    ```incan
    return jsx(<div>{message}</div>)
    #          ↑ This is NOT a string — it's native Incan syntax
    ```

    The content inside `jsx()` is **parsed directly by Incan** as first-class syntax. Your IDE sees markup, not a string.

#### Parser Behavior

When the parser encounters `jsx(`, it switches to JSX mode:

1. `<` is always a tag open, never less-than
2. `{...}` switches back to Incan expression parsing
3. Self-closing tags: `<Component />`
4. Attributes: `prop={expr}` and `prop="string"`

#### Why This Matters

| Aspect              | `html()` (string)   | `jsx()` (native syntax)   |
| ------------------- | ------------------- | ------------------------- |
| What IDE sees       | A string            | Markup syntax             |
| Syntax highlighting | Requires plugin     | Automatic                 |
| Autocomplete        | None                | Full                      |
| Error messages      | "Invalid string"    | "Unknown component `Foo`" |
| Escaping `"""`      | Requires workaround | Not an issue              |

---

### Part 3: 3D Graphics (Three.js Alternative — exploratory)

A scene-graph API for 3D graphics, built on WebGPU via wgpu.

#### Basic Scene

```incan
from incan.graphics import Scene, Camera, Renderer
from incan.graphics import Mesh, BoxGeometry, StandardMaterial
from incan.graphics import AmbientLight, DirectionalLight

# Create scene
scene = Scene()

# Add camera
camera = Camera.perspective(
    fov=75,
    aspect=16/9,
    near=0.1,
    far=1000
)
camera.position = Vec3(0, 5, 10)
camera.look_at(Vec3.zero())

# Add lighting
scene.add(AmbientLight(color=0x404040))
scene.add(DirectionalLight(
    color=0xffffff,
    intensity=1.0,
    position=Vec3(10, 10, 10)
))

# Add a cube
cube = Mesh(
    geometry=BoxGeometry(2, 2, 2),
    material=StandardMaterial(
        color=0x00ff00,
        metalness=0.5,
        roughness=0.5
    )
)
scene.add(cube)

# Create renderer
renderer = Renderer(canvas="#canvas")

# Animation loop
def animate(delta: float) -> None:
    cube.rotation.x += delta
    cube.rotation.y += delta * 0.5
    renderer.render(scene, camera)

renderer.start(animate)
```

#### Geometries

```incan
from incan.graphics.geometry import (
    BoxGeometry,
    SphereGeometry,
    PlaneGeometry,
    CylinderGeometry,
    TorusGeometry,
    BufferGeometry,  # Custom geometry
)

# Parametric geometries
sphere = SphereGeometry(radius=1, segments=32)
plane = PlaneGeometry(width=10, height=10)
torus = TorusGeometry(radius=1, tube=0.4, segments=16)

# Custom geometry from vertices
custom = BufferGeometry()
custom.set_attribute("position", positions)
custom.set_attribute("normal", normals)
custom.set_attribute("uv", uvs)
custom.set_index(indices)
```

#### Materials

```incan
from incan.graphics.material import (
    StandardMaterial,   # PBR material
    BasicMaterial,      # Unlit
    PhongMaterial,      # Classic lighting
    ShaderMaterial,     # Custom shaders
)

# PBR material
pbr = StandardMaterial(
    color=0xff0000,
    metalness=0.8,
    roughness=0.2,
    normal_map=load_texture("normal.png"),
    ao_map=load_texture("ao.png"),
)

# Custom shader
custom = ShaderMaterial(
    vertex_shader="""
        @vertex
        fn main(@location(0) position: vec3<f32>) -> @builtin(position) vec4<f32> {
            return uniforms.mvp * vec4(position, 1.0);
        }
    """,
    fragment_shader="""
        @fragment
        fn main() -> @location(0) vec4<f32> {
            return vec4(1.0, 0.0, 0.0, 1.0);
        }
    """,
    uniforms={"time": 0.0}
)
```

#### Asset Loading

```incan
from incan.graphics import load_gltf, load_texture, load_cubemap
import std.async

# Load 3D model
model = await load_gltf("model.glb")
scene.add(model)

# Load texture
texture = await load_texture("diffuse.png")

# Load environment map
skybox = await load_cubemap([
    "px.jpg", "nx.jpg",
    "py.jpg", "ny.jpg", 
    "pz.jpg", "nz.jpg"
])
scene.environment = skybox
```

#### Animation

```incan
from incan.graphics import AnimationMixer, AnimationClip

# Load animated model
model = await load_gltf("character.glb")
mixer = AnimationMixer(model)

# Play animation
walk = model.animations["walk"]
action = mixer.clip_action(walk)
action.play()

# In render loop
def animate(delta: float) -> None:
    mixer.update(delta)
    renderer.render(scene, camera)
```

#### Physics Integration (Optional)

```incan
from incan.physics import World, RigidBody, Collider

# Create physics world
world = World(gravity=Vec3(0, -9.81, 0))

# Add physics to mesh
body = RigidBody(
    position=cube.position,
    collider=Collider.box(2, 2, 2),
    mass=1.0
)
world.add(body)

# Sync physics → graphics
def animate(delta: float) -> None:
    world.step(delta)
    cube.position = body.position
    cube.rotation = body.rotation
    renderer.render(scene, camera)
```

---

### Part 4: Build Tooling

#### Development Server

```bash
incan dev --target wasm
```

Features:

- Hot module replacement (HMR)
- Source maps for debugging
- Automatic browser refresh
- Error overlay

#### Production Build

```bash
incan build --target wasm --release
```

Outputs:

- Optimized WASM binary (wasm-opt)
- Minified JS glue code
- Tree-shaken bundle
- Asset hashing for caching

#### Project Structure

```bash
my_app/
├── src/
│   ├── main.incn          # Entry point
│   ├── components/
│   │   ├── App.incn
│   │   └── Counter.incn
│   └── scenes/
│       └── MainScene.incn
├── assets/
│   ├── models/
│   ├── textures/
│   └── fonts/
├── public/
│   └── index.html
└── Cargo.toml             # Project config (with Incan metadata)
```

#### Configuration (Cargo.toml)

Incan uses Cargo.toml with `[package.metadata.incan]` for Incan-specific settings. This follows Rust ecosystem conventions (used by wasm-pack, cargo-deb, etc.). Rust dependencies are auto-injected by the Incan toolchain based on what your Incan code imports and the build target.

```toml
[package]
name = "my_app"
version = "0.1.0"
edition = "2021"

# Incan-specific configuration
[package.metadata.incan]
entry = "src/main.incn"
target = "wasm"

[package.metadata.incan.wasm]
optimize = true
debug_symbols = false

[package.metadata.incan.dev]
port = 3000
open_browser = true
```

Auto-added dependencies (examples):

| Incan usage                           | Added to Cargo.toml |
| ------------------------------------- | ------------------- |
| `from incan.ui import component, jsx` | `leptos`            |
| `target = "wasm"`                     | `wasm-bindgen`      |
| `from incan.graphics import Scene`    | `wgpu`, `glam`      |
| `from incan.physics import RigidBody` | `rapier3d`          |

---

## Rust Crate Dependencies

| Feature       | Crate                     | Purpose             |
| ------------- | ------------------------- | ------------------- |
| WASM interop  | wasm-bindgen              | JS↔Rust FFI         |
| DOM access    | web-sys                   | Browser APIs        |
| Reactivity    | Custom or Leptos-inspired | Signal system       |
| 3D graphics   | wgpu                      | WebGPU abstraction  |
| Math          | glam                      | Vectors, matrices   |
| Asset loading | gltf, image               | 3D models, textures |
| Physics       | rapier                    | Optional physics    |

---

## Success Criteria

### Phase 1

1. The **documented golden-path app** satisfies the [**Default Phase 1 vertical slice**](#default-phase-1-vertical-slice) (or a successor recorded in this RFC or docs): server-authoritative handlers, **interactive UI** in the browser, deployable **without** depending on Phase 2-only features unless explicitly chosen.
2. **Typed HTTP/API surface** with predictable errors and documented integration with auth/policy hooks.
3. **Packaging story**: deployable unit with entrypoints, browser artifacts required by the first target, and environment/preview metadata.
4. **Explicit server vs client boundaries**; representative flows (data loading, forms) work end-to-end.
5. **First browser target** chosen and documented for the golden path.

### Phase 2 (Reference design — optional)

1. **"Hello World" WASM app** compiles and runs in browser.
2. **Counter component** demonstrates reactive state.
3. **TodoMVC** proves component model completeness.
4. **Rotating cube** demonstrates basic 3D.
5. **Animated character** demonstrates asset loading + animation.
6. **Full demo app** combines UI + 3D (e.g., 3D product viewer).

---

## Future Extensions

- **Later / roadmap-aligned:** server-side rendering (SSR) with hydration; static site generation (SSG); native desktop via wgpu (non-WASM); mobile via wgpu + platform bindings; collaborative editing (CRDTs). See [Roadmap — deferred / later](../../../roadmap.md).
- VR/AR support via WebXR

---

## References

- [wasm-bindgen](https://rustwasm.github.io/wasm-bindgen/)
- [Leptos](https://leptos.dev/) - Rust reactive framework
- [wgpu](https://wgpu.rs/) - WebGPU for Rust
- [Bevy](https://bevyengine.org/) - Rust game engine
- [Three.js](https://threejs.org/) - JS 3D library (inspiration)
- [SolidJS](https://www.solidjs.com/) - JS signals (inspiration)

---

## Checklist {#checklist}

### Phase 1

- [ ] **Golden path** matches the [**Default Phase 1 vertical slice**](#default-phase-1-vertical-slice) (or an explicitly documented successor)
- [ ] Typed HTTP handlers / external API surface (as required by that slice)
- [ ] Server-side mutations or actions with predictable errors and auth/policy hooks
- [ ] App packaging: entrypoints, static assets, environment/preview metadata
- [ ] Session/identity/access composition targets documented and usable across samples
- [ ] Component or page **rendering** runtime with explicit server vs client boundaries
- [ ] First browser target decision + representative data-loading, forms, and **interactive UI** flows

### Phase 2 (Reference design — optional)

- [ ] CLI: `incan build --target wasm` plumbing
- [ ] Codegen: wasm-bindgen output for `wasm32-unknown-unknown`
- [ ] Auto-deps: inject wasm-bindgen/web-sys/leptos/etc. from usage
- [ ] UI: signals/effect/component runtime surface
- [ ] Templates: `html()` strings
- [ ] Templates: `jsx()` wrapper parsing/emission
- [ ] Event handlers: named + arrow inline support
- [ ] Routing: Router/Route/Link mapping
- [ ] Dev server: `incan dev --target wasm` with HMR/overlay
- [ ] Prod build: wasm-opt/minify/tree-shake/assets
- [ ] 3D: wgpu bindings + scene graph + loaders
- [ ] Examples: counter, TodoMVC, rotating cube, 3D demo

<!-- Rename "Unresolved questions" to "Design Decisions" once all blocking questions have accepted answers and this RFC moves toward Planned or In Progress. An RFC should not unblock while blocking items remain open. -->
