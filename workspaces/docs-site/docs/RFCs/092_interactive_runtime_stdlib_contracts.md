# RFC 092: Interactive Runtime Stdlib Contracts

- **Status:** Draft
- **Created:** 2026-05-07
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 003 (superseded frontend and WebAssembly support)
    - RFC 020 (offline locked reproducible builds)
    - RFC 031 (library system phase 1)
    - RFC 037 (native web and HTTP stdlib redesign)
- **Issue:** —
- **RFC PR:** —
- **Written against:** v0.2
- **Shipped in:** —

## Summary

This RFC defines the Incan-owned stdlib and artifact contracts needed by interactive runtime consumers: target descriptors, host capability boundaries, runtime manifests, server/client execution regions, event and state interop, diagnostics, input/accessibility hooks, asset packaging, and optional graphics/GPU capability surfaces. It does not define a product framework, UI framework, browser framework, or full runtime substrate; it defines the stdlib vocabulary that such layers need in order to consume Incan programs without reverse-engineering compiler output or generated framework glue.

## Core model

1. **Interactive runtime consumers are downstream of Incan.** They consume typed programs, stdlib declarations, emitted metadata, and packaged artifacts. They do not own Incan language semantics.
2. **Runtime targets are explicit.** A target describes where code may run, which host capabilities it may use, which artifacts are emitted, and which diagnostics are available.
3. **Host capabilities are declared.** Browser APIs, filesystem, network, GPU, timers, storage, input, and similar capabilities must be visible as typed boundaries instead of hidden imports.
4. **Artifacts are inspectable.** Routes, assets, target regions, generated bundles, GPU shader assets, and diagnostics must be discoverable by tooling and downstream runtime layers.
5. **Graphics and GPU support are capability families.** Incan may expose stdlib contracts for buffers, textures, pipelines, surfaces, shaders, and render passes without deciding that every interactive target is graphics-heavy or WASM-first.

## Motivation

RFC 003 tried to cover frontend support, WebAssembly, browser UI, packaging, and future graphics/runtime expansion in one document. That made the RFC too broad for an Incan-owned design record. Incan should not encode a whole downstream product framework into the compiler, but it does need stable stdlib contracts for interactive runtime consumers to build on.

The immediate need is practical. Browser product surfaces need typed routes, assets, server/client boundaries, and packaging metadata. Future richer targets may need GPU-backed rendering, native-class surfaces, remote UI protocols, or embedded interaction. If those needs are handled as framework-specific glue, Incan loses inspectability, reproducibility, and a clean boundary between language contracts and runtime consumers.

This RFC narrows the problem: Incan owns the contracts that make interactive runtime consumption possible. Downstream layers can decide product semantics, rendering strategy, deployment policy, and experience model.

## Goals

- Define a stdlib-level vocabulary for interactive runtime targets without defining a complete runtime framework.
- Define runtime target manifests that describe emitted artifacts, host capabilities, execution regions, assets, and diagnostics.
- Define how server-only, client-only, and shared execution regions are declared or represented.
- Define typed capability surfaces for host APIs needed by interactive runtimes, including browser, input, accessibility, timers, storage, and optional GPU/graphics capabilities.
- Keep RFC 037 responsible for HTTP route, handler, serialization, and request/response semantics while allowing runtime manifests to reference those surfaces.
- Preserve reproducible build and library-layout compatibility with RFC 020 and RFC 031.

## Non-Goals

- Defining a downstream product framework, governed experience layer, dashboard layer, assistant surface, or UI framework.
- Making WASM the default runtime posture. WASM may be one target capability, not the definition of interactive runtime support.
- Defining native JSX, `html()` parsing, a component DSL, or a browser router in this RFC.
- Defining GPU algorithms, shader language semantics, scene-graph APIs, physics engines, or rendering engines.
- Replacing RFC 037 handler semantics.
- Committing to a specific Rust web framework, JS framework, graphics crate, or bundler as the public contract.

## Guide-level explanation

Authors should be able to write Incan programs that expose typed HTTP handlers, state-changing operations, assets, and optional interactive regions. The project should then emit enough metadata for a runtime consumer to understand what exists: which routes are present, which assets must ship, which code is server-only or client-capable, which host capabilities are required, which diagnostics are available, and which optional graphics/GPU assets belong to a target.

For example, a browser product surface might need server-authoritative mutations, browser-enhanced UI, static assets, client-side event hooks, and accessibility/input metadata. A richer graphics target might additionally need GPU buffers, shader module assets, textures, render surfaces, and frame diagnostics. In both cases, Incan should expose typed stdlib contracts and manifests. The downstream runtime decides how to render and orchestrate the experience.

A future author-facing API might look like this, with exact names left to this RFC's unresolved questions:

```incan
from std.runtime.host import capability
from std.runtime.region import client_region, server_region
from std.runtime.target import runtime_target
from std.web import Request, Response, route

@runtime_target("browser-product-surface")
@capability("browser.dom")
@capability("browser.fetch")
def app_target() -> None:
    pass

@server_region
@route("POST", "/submit")
def submit(req: Request) -> Response:
    return Response.ok()

@client_region
def increment(count: int) -> int:
    return count + 1
```

The important contract is not the spelling of `runtime_target`, `capability`, or `client_region`. The important contract is that the compiler and stdlib can preserve the target, host capability, route, and execution-region facts in an emitted manifest that downstream runtime consumers can inspect.

The author-facing model is:

```text
Incan source
  -> typed HTTP/std capability surfaces
  -> declared runtime target requirements
  -> emitted manifest and artifacts
  -> downstream runtime consumer
```

## Reference-level explanation

An Incan project that opts into an interactive runtime target **must** emit or provide a machine-readable runtime manifest for that target.

The runtime manifest **must** identify:

- the target name and target kind
- source entrypoints and generated artifact entrypoints
- referenced HTTP routes, handlers, or actions where applicable
- static assets and generated bundles required by the target
- declared server-only, client-only, and shared execution regions where applicable
- host capabilities required by the target
- diagnostics available for generated boundaries
- optional graphics/GPU artifacts required by the target

The stdlib **must** provide typed representations for runtime target identity, host capability declarations, artifact references, execution region identity, runtime diagnostics, input/accessibility hooks, and optional graphics/GPU resources.

Runtime target declarations **must not** imply permission to use a host capability. Capability use **must** be explicit enough for tooling to inspect and reject unsupported or undeclared host access.

Runtime target manifests **must not** redefine HTTP semantics. When a manifest references routes, handlers, actions, or response contracts, those semantics remain governed by RFC 037 and the web/HTTP stdlib.

Graphics/GPU stdlib contracts **may** expose typed handles for adapters, devices, queues, surfaces, buffers, textures, samplers, shader modules, bind groups, pipelines, render passes, and command submission. They **must** be capability-checked and target-aware. They **must not** require every interactive runtime target to support GPU access.

## Design details

The exact module names are provisional, but the public shape should remain split by responsibility.

| Capability family               | Responsibility                                                                                      |
| ------------------------------- | --------------------------------------------------------------------------------------------------- |
| `std.runtime.target`            | target identity, target kind, manifest declarations                                                 |
| `std.runtime.artifact`          | static assets, generated bundles, generated metadata, artifact digests                              |
| `std.runtime.region`            | server-only, client-only, shared, and host-bound execution regions                                  |
| `std.runtime.host`              | host capability declarations and capability checks                                                  |
| `std.runtime.input`             | input event boundaries, focus, pointer, keyboard, composition, and accessibility hooks              |
| `std.runtime.diagnostics`       | generated-boundary spans, runtime events, host crossings, asset provenance                          |
| `std.gpu` or `std.graphics.gpu` | optional GPU adapters, devices, queues, surfaces, buffers, textures, shaders, pipelines, and passes |

A minimal manifest could look like this conceptually:

```text
target: browser-product-surface
entrypoint: app.main
routes:
  - GET /
  - POST /submit
regions:
  - server: app.policy
  - client: app.counter
assets:
  - public/app.css
  - generated/app.js
capabilities:
  - browser.dom
  - browser.fetch
diagnostics:
  - generated_boundary_spans
  - route_asset_links
```

GPU-capable targets extend the same model instead of creating a separate build world:

```text
target: gpu-surface
capabilities:
  - gpu.adapter
  - gpu.device
  - gpu.surface
gpu_artifacts:
  - shader: shaders/main.wgsl
  - texture: assets/albedo.png
diagnostics:
  - gpu_pipeline_creation
  - frame_timing
```

The examples above are illustrative, not normative syntax. The normative requirement is that the target data exists in an inspectable machine-readable form and that stdlib contracts expose the same concepts to authored Incan code where appropriate.

## Alternatives considered

- **Keep RFC 003 active and broaden it further** — Rejected because a single RFC would continue mixing browser UI, WASM, graphics, packaging, and downstream framework direction.
- **Define the downstream runtime framework inside Incan** — Rejected because Incan should own typed contracts and emitted artifacts, not product-framework semantics.
- **Make GPU support a separate unrelated RFC immediately** — Rejected for now because graphics/GPU support needs the same capability, artifact, target, and diagnostics model as other interactive runtime targets. A future GPU-specific RFC may refine the stdlib surface once this foundation is accepted.
- **Treat manifests as build-tool internals** — Rejected because downstream runtime consumers and docs need stable inspectability.

## Drawbacks

- The RFC creates another abstraction boundary before a full interactive runtime exists.
- The stdlib contracts may feel abstract until at least one browser target and one richer capability target exercise them.
- Capability declarations add ceremony to small examples.
- GPU contracts can prematurely constrain future graphics work if accepted before enough spikes validate the shape.

## Implementation architecture

The recommended internal architecture is to keep the runtime manifest separate from generated Rust implementation details. Compiler output, build tooling, docs, and downstream runtime consumers should all read the same manifest concepts rather than each inferring routes, assets, regions, and capabilities independently.

Graphics/GPU support should start as capability and artifact metadata before exposing broad author-facing APIs. Early spikes should prove that shader assets, surface/device requirements, generated diagnostics, and target compatibility can be represented without forcing all interactive runtimes into a graphics-first model.

## Layers affected

- **Parser / AST**: may need future syntax for execution regions or target declarations if library-only declarations are insufficient.
- **Typechecker / Symbol resolution**: validates capability declarations, target references, and server/client/host-boundary constraints.
- **IR Lowering**: preserves target, region, capability, and artifact metadata through lowering.
- **Emission**: emits runtime manifests, artifact metadata, diagnostics metadata, and optional GPU asset references.
- **Stdlib / Runtime (`incan_stdlib`)**: provides runtime target, artifact, host, input/accessibility, diagnostics, and optional graphics/GPU contracts.
- **LSP / Tooling**: surfaces target/capability diagnostics, generated-boundary spans, asset references, and unsupported-host warnings.

## Unresolved questions

- Are runtime target declarations library calls, project metadata, decorators, or a dedicated syntax form?
- What is the minimum manifest format that is stable enough for downstream consumers while still allowing compiler internals to evolve?
- Which host capabilities belong in the first browser target, and which remain target-specific extensions?
- Should GPU stdlib contracts begin as low-level resource handles, higher-level drawing primitives, or metadata-only capability declarations?
- How should capability denial be reported: compile-time error, build-time target mismatch, runtime diagnostic, or a mix based on target?
- How much server/client region information must be explicit in source versus derived from imports, handlers, and target configuration?

<!-- Rename this section to "Design Decisions" once all questions have been resolved.
     An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
