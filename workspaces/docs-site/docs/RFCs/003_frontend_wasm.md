# RFC 003: frontend and WebAssembly support

- **Status:** Draft
- **Created:** 2025-12-11
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:** —
- **Issue:** https://github.com/dannys-code-corner/incan/issues/312
- **RFC PR:** —
- **Written against:** v0.1
- **Shipped in:** —

## Summary

This RFC proposes a browser-facing Incan story built around a WebAssembly compilation target plus first-class frontend and graphics APIs. The north-star is that Incan can compile to native server binaries and to browser-deployable WASM output while offering a reactive UI surface, browser tooling, and a scene-graph style graphics layer, so Python-oriented users do not need to split their application model across Python on the backend and JavaScript on the frontend. The document currently acts as an umbrella RFC for those surfaces; if later review splits it, that split should preserve this end-state rather than narrowing the ambition.

## Motivation

### The Python Full-Stack Problem

Python developers building full-stack applications face a common frustration:
**Python for backend, JavaScript for frontend**. The typical stack looks like:

```bash
Backend: Python (FastAPI, Django, Flask)
    ↕ API calls ↕
Frontend: JavaScript/TypeScript (React, Vue, Angular)
```

This requires:

- Learning a second language (JavaScript/TypeScript)
- Context-switching between paradigms
- Maintaining two codebases with different tooling
- Duplicating types/models between backend and frontend

### Existing Python → Frontend Solutions

| Solution      | Approach            | Limitations                              |
| ------------- | ------------------- | ---------------------------------------- |
| **Streamlit** | Python → widgets    | Limited UI, data apps only               |
| **Gradio**    | Python → components | Specialized for ML demos                 |
| **PyScript**  | CPython in WASM     | Slow startup, ~10MB bundle, GC overhead  |
| **Reflex**    | Python → React      | Generates JavaScript, server-round-trips |
| **NiceGUI**   | Python → Vue        | Server-side rendering, network latency   |
| **Anvil**     | Full Python web     | Proprietary, hosted platform             |

None of these provide:

- **Native WASM performance** (no Python interpreter overhead)
- **True compile-time type safety** (not runtime checks)
- **Rust's memory guarantees** (no garbage collector)
- **Offline-capable  Single Page Applications (SPAs)** (not server-dependent)

### Why Not Rust Directly?

Rust + WebAssembly solves the performance and safety issues, but presents barriers for Python developers:

- **Ownership model** — conceptually foreign to GC-language developers
- **Borrow checker** — rejects code that "looks correct"
- **Lifetime annotations** — complex syntax for memory management
- **Verbose syntax** — more ceremony than Python

TypeScript developers have a smaller gap to Rust (similar syntax, static types). But Python developers face a steeper learning curve.

### Incan's Opportunity

**Full-stack Python without JavaScript** — one language for backend APIs, frontend UIs, and 3D graphics,
all compiling to native performance:

```bash
Incan (Python-like syntax)
    ↓ compiles to
Rust (backend) + Rust/WASM (frontend)
    ↓ produces
Native binary (server) + WebAssembly (browser)
```

Benefits:

- **Familiar syntax** — Python developers feel at home
- **Native performance** — no interpreter, no GC pauses
- **True full-stack** — same language, same types, everywhere
- **Rust's safety** — memory safety without learning ownership
- **Modern tooling** — single build system, unified debugging

## Goals

- Give Incan an explicit WebAssembly/browser compilation target.
- Define a first-class frontend UI story rather than treating browser work as pure Rust interop.
- Define a graphics surface suitable for browser-based interactive and 3D applications.
- Standardize dev and production browser tooling as part of the end-state developer experience.
- Keep the user-facing surface Incan-first even when it lowers to Rust and WASM underneath.

## Non-Goals

- Promising that every surface in this RFC lands in one implementation phase.
- Replacing the entire JavaScript ecosystem or every frontend-specialized framework.
- Standardizing server-side rendering, static-site generation, desktop, mobile, VR, or AR in this RFC.
- Committing to one specific Rust crate stack as part of the public language contract.

## Guide-level explanation

### Part 1: WASM Compilation Target

Add a `--target wasm` flag to the Incan compiler:

```bash
incan build --target wasm app.incn
```

This generates:

- Rust code with `wasm-bindgen` annotations
- `Cargo.toml` configured for `wasm32-unknown-unknown`
- Build artifacts ready for browser deployment

#### Illustrative generated structure

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

### Part 2: UI Framework (React Alternative)

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

Both compile to the same output — choose based on preference.

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

Incan supports two wrapper modes: `html()` and `jsx()`. Both compile to the same output (Leptos view nodes), but they parse differently:
`html()` takes a string, while `jsx()` is native syntax the IDE can understand.

- `html()` takes a string:

    ```incan
    return html("""<div>{message}</div>""")
    #            ↑ This is a string literal
    ```

    The content inside `html()` is a **string** that gets parsed at compile time.
    Your IDE sees a string, not markup.

- `jsx()` enables native syntax (inspired by React's JSX syntax):

    ```incan
    return jsx(<div>{message}</div>)
    #          ↑ This is NOT a string — it's native Incan syntax
    ```

    The content inside `jsx()` is **parsed directly by Incan** as first-class syntax.
    Your IDE sees markup, not a string.

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

### Part 3: 3D Graphics (Three.js Alternative)

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

#### Illustrative configuration (`Cargo.toml`)

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

Illustrative dependency mapping examples:

| Incan usage                           | Added to Cargo.toml |
| ------------------------------------- | ------------------- |
| `from incan.ui import component, jsx` | `leptos`            |
| `target = "wasm"`                     | `wasm-bindgen`      |
| `from incan.graphics import Scene`    | `wgpu`, `glam`      |
| `from incan.physics import RigidBody` | `rapier3d`          |

## Reference-level explanation

The RFC proposes four coupled surfaces: an explicit WASM target, a reactive UI layer, a browser-oriented graphics layer, and browser-focused tooling.

- The language and tooling must support an explicit browser or WASM target rather than treating browser emission as ad hoc Rust interop.
- The browser target should produce deployable artifacts and preserve a stable mapping between Incan values and browser-facing JavaScript boundaries.
- The UI surface should provide components, local reactive state, effects, templating, event binding, and routing.
- The graphics surface should provide a scene-graph style API with cameras, meshes, materials, asset loading, and animation hooks.
- Browser tooling should provide distinct development and production flows rather than requiring users to hand-assemble Rust or WASM build commands.
- The RFC currently describes both `html(...)` string templates and `jsx(...)` wrapper syntax; it does not yet settle whether both belong in the final public contract.
- The RFC currently bundles browser UI and 3D graphics into one umbrella proposal; that coupling remains open to revision if later review concludes they should become separate RFCs.

## Design details

### Surface decomposition

- **Target layer**: `incan build --target wasm ...` and `incan dev --target wasm` represent the intended user-facing entry points for browser work.
- **UI layer**: the proposal includes components, signals, effects, routing, and template-based rendering.
- **Graphics layer**: the proposal includes a browser-friendly scene graph, materials, asset loading, and animation support.
- **Tooling layer**: the proposal includes a dev server, browser refresh or HMR-style feedback loops, source maps, and optimized production output.

### Example Rust ecosystem backing (non-normative)

| Feature       | Crate                     | Purpose             |
| ------------- | ------------------------- | ------------------- |
| WASM interop  | wasm-bindgen              | JS↔Rust FFI         |
| DOM access    | web-sys                   | Browser APIs        |
| Reactivity    | Custom or Leptos-inspired | Signal system       |
| 3D graphics   | wgpu                      | WebGPU abstraction  |
| Math          | glam                      | Vectors, matrices   |
| Asset loading | gltf, image               | 3D models, textures |
| Physics       | rapier                    | Optional physics    |

### Demonstration targets

The intended surface should be strong enough to cover:

- a "hello world" WASM app;
- a reactive counter-style component;
- a TodoMVC-scale UI application;
- a rotating-cube style graphics demo; and
- a combined UI plus graphics demo such as a 3D product viewer.

## Alternatives considered

1. **Keep the status quo: Python backend plus JavaScript or TypeScript frontend**
   - Rejected because it preserves the split-language experience the RFC is trying to eliminate.

2. **Tell users to write Rust directly for browser work**
   - Rejected because it defeats the Python-first positioning of Incan and pushes users into a different language and mental model at exactly the point where a full-stack story is most valuable.

3. **Treat frontend UI and graphics strictly as third-party ecosystem work**
   - Rejected as the north-star because it leaves one of the largest developer-experience gaps in the language outside the standard design story.

### References

- [wasm-bindgen](https://rustwasm.github.io/wasm-bindgen/)
- [Leptos](https://leptos.dev/) - Rust reactive framework
- [wgpu](https://wgpu.rs/) - WebGPU for Rust
- [Bevy](https://bevyengine.org/) - Rust game engine
- [Three.js](https://threejs.org/) - JS 3D library (inspiration)
- [SolidJS](https://www.solidjs.com/) - JS signals (inspiration)


## Drawbacks

- This RFC is very broad: it couples a compiler target, UI framework ideas, graphics abstractions, and tooling expectations in one document.
- Browser runtimes and frontend ergonomics evolve quickly, so locking down the wrong abstraction too early would be costly.
- The document currently mixes mature target-shape ideas with much more speculative UI and graphics surface design.
- Supporting browser and WASM workflows well would add substantial compiler, runtime, and tooling scope.

## Layers affected

- **CLI / tooling**: must expose explicit browser-target build and dev commands rather than forcing manual Rust or WASM workflows.
- **Execution handoff**: implementations must support browser-oriented output that preserves stable browser-boundary semantics.
- **Stdlib / runtime**: browser-facing UI, graphics, and support libraries would be required if this RFC remains bundled as one proposal.
- **Language surface**: the proposed browser-facing module surfaces must be recognized and validated coherently.
- **Docs / examples**: the browser build model, UI reactivity model, and graphics model must be explained coherently.

## Unresolved questions

- Should the WebAssembly target, UI framework, and graphics framework remain in one RFC, or should this be split into smaller but still complete north-star RFCs?
- Should both `html(...)` and `jsx(...)` remain in scope, or should the long-term contract standardize one primary templating surface?
- Should browser tooling such as dev-server and bundling behavior be part of this RFC's public contract, or should they move to a separate tooling RFC?
- How much of the proposed graphics surface belongs in core stdlib versus a purpose-built library layer?

<!-- Rename this section to "Design Decisions" once all questions have been resolved. An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
