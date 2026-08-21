# RFC 092: Interactive Runtime Stdlib Contracts

- **Status:** Rejected
- **Created:** 2026-05-07
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 003 (superseded frontend and WebAssembly support)
    - RFC 020 (offline locked reproducible builds)
    - RFC 031 (library system phase 1)
    - RFC 037 (native web and HTTP stdlib redesign)
- **Issue:** [#558](https://github.com/encero-systems/incan/issues/558)
- **RFC PR:** —
- **Written against:** v0.2
- **Shipped in:** —

## Summary

This RFC defines the Incan-owned stdlib and artifact contracts needed by interactive runtime consumers: target descriptors, host capability boundaries, runtime manifests, server/client execution regions, event and state interop, diagnostics, input/accessibility hooks, and asset packaging. It does not define a product framework, UI framework, browser framework, GPU contract, or full runtime substrate; it defines the stdlib vocabulary that such layers need in order to consume Incan programs without reverse-engineering compiler output or generated framework glue. Optional GPU work is deferred to [#1041](https://github.com/encero-systems/incan/issues/1041).

## Core model

1. **Interactive runtime consumers are downstream of Incan.** They consume typed programs, stdlib declarations, emitted metadata, and packaged artifacts. They do not own Incan language semantics.
2. **Runtime targets are explicit.** A target describes where code may run, which host capabilities it may use, which artifacts are emitted, and which diagnostics are available.
3. **Host capabilities are declared.** Browser APIs, filesystem, network, timers, storage, input, and similar capabilities must be visible as typed boundaries instead of hidden imports.
4. **Artifacts are inspectable.** Routes, assets, target regions, generated bundles, and diagnostics must be discoverable by tooling and downstream runtime layers.

## Motivation

RFC 003 tried to cover frontend support, WebAssembly, browser UI, packaging, and future runtime expansion in one document. That made the RFC too broad for an Incan-owned design record. Incan should not encode a whole downstream product framework into the compiler, but it does need stable stdlib contracts for interactive runtime consumers to build on.

The immediate need is practical. Browser product surfaces need typed routes, assets, server/client boundaries, and packaging metadata. Future runtime consumers may need native-class surfaces, remote UI protocols, or embedded interaction. If those needs are handled as framework-specific glue, Incan loses inspectability, reproducibility, and a clean boundary between language contracts and runtime consumers.

This RFC narrows the problem: Incan owns the contracts that make interactive runtime consumption possible. Downstream layers can decide product semantics, rendering strategy, deployment policy, and experience model.

## Goals

- Define a stdlib-level vocabulary for interactive runtime targets without defining a complete runtime framework.
- Define runtime target manifests that describe emitted artifacts, host capabilities, execution regions, assets, and diagnostics.
- Define how server-only, client-only, and shared execution regions are declared or represented.
- Define typed capability surfaces for host APIs needed by interactive runtimes, including browser, input, accessibility, timers, and storage.
- Keep RFC 037 responsible for HTTP route, handler, serialization, and request/response semantics while allowing runtime manifests to reference those surfaces.
- Preserve reproducible build and library-layout compatibility with RFC 020 and RFC 031.

## Non-Goals

- Defining a downstream product framework, governed experience layer, dashboard layer, assistant surface, or UI framework.
- Making WASM the default runtime posture. WASM may be one target capability, not the definition of interactive runtime support.
- Defining native JSX, `html()` parsing, a component DSL, or a browser router in this RFC.
- Defining GPU target, capability, resource, artifact, diagnostic, or rendering contracts. That work is deferred to #1041.
- Defining no-std/freestanding targets, kernel support, unsafe/layout controls, panic strategy, or allocator strategy. Runtime target manifests may inform that later work, but this RFC is not the freestanding/kernel RFC.
- Replacing RFC 037 handler semantics.
- Committing to a specific Rust web framework, JS framework, or bundler as the public contract.

## Guide-level explanation

Authors should be able to write Incan programs that expose typed HTTP handlers, state-changing operations, assets, and optional interactive regions. The project should then emit enough metadata for a runtime consumer to understand what exists: which routes are present, which assets must ship, which code is server-only or client-capable, which host capabilities are required, and which diagnostics are available.

For example, a browser product surface might need server-authoritative mutations, browser-enhanced UI, static assets, client-side event hooks, and accessibility/input metadata. Incan should expose typed stdlib contracts and manifests. The downstream runtime decides how to render and orchestrate the experience.

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

The stdlib **must** provide typed representations for runtime target identity, host capability declarations, artifact references, execution region identity, runtime diagnostics, and input/accessibility hooks.

Runtime target declarations **must not** imply permission to use a host capability. Capability use **must** be explicit enough for tooling to inspect and reject unsupported or undeclared host access.

Runtime target manifests **must not** redefine HTTP semantics. When a manifest references routes, handlers, actions, or response contracts, those semantics remain governed by RFC 037 and the web/HTTP stdlib.

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

The examples above are illustrative, not normative syntax. The normative requirement is that the target data exists in an inspectable machine-readable form and that stdlib contracts expose the same concepts to authored Incan code where appropriate.

## Alternatives considered

- **Keep RFC 003 active and broaden it further** — Rejected because a single RFC would continue mixing browser UI, WASM, graphics, packaging, and downstream framework direction.
- **Define the downstream runtime framework inside Incan** — Rejected because Incan should own typed contracts and emitted artifacts, not product-framework semantics.
- **Include GPU support in this RFC** — Rejected. GPU work is not required for the v0.6 interactive-runtime foundation and needs a concrete consumer before its public contract is designed; it is tracked by #1041.
- **Treat manifests as build-tool internals** — Rejected because downstream runtime consumers and docs need stable inspectability.

## Drawbacks

- The RFC creates another abstraction boundary before a full interactive runtime exists.
- The stdlib contracts may feel abstract until at least one browser target and one richer capability target exercise them.
- Capability declarations add ceremony to small examples.

## Implementation architecture

The recommended internal architecture is to keep the runtime manifest separate from generated Rust implementation details. Compiler output, build tooling, docs, and downstream runtime consumers should all read the same manifest concepts rather than each inferring routes, assets, regions, and capabilities independently.

## Layers affected

- **Parser / AST**: may need future syntax for execution regions or target declarations if library-only declarations are insufficient.
- **Typechecker / Symbol resolution**: validates capability declarations, target references, and server/client/host-boundary constraints.
- **IR Lowering**: preserves target, region, capability, and artifact metadata through lowering.
- **Emission**: emits runtime manifests, artifact metadata, and diagnostics metadata.
- **Stdlib / Runtime (`incan_stdlib`)**: provides runtime target, artifact, host, input/accessibility, and diagnostics contracts.
- **LSP / Tooling**: surfaces target/capability diagnostics, generated-boundary spans, asset references, and unsupported-host warnings.

## Rejection rationale

RFC 092 set out to define Incan-owned stdlib contracts for interactive runtime consumers -- target manifests, host capability boundaries, execution regions, and packaging metadata. On review, its actual normative surface duplicated or overreached into ground already covered elsewhere:

- Capability authority (browser or otherwise) is already fully covered by RFC 104's general, package-namespaced capability mechanism. No `host.browser.*` or similar toolchain-reserved namespace is needed.
- Marking functions for different compilation targets (server-side, client-side, or any future split) is already supported by RFC 036's user-defined decorators, combined with existing compiler introspection (`incan inspect codegraph`). No new Incan-core "region" concept is needed.
- Producing multiple artifacts from one project is already the shape of RFC 117-119's target/carrier/profile model -- a completed bake already emits one or more target-bound `*.loaf` assets according to the selected roles and carriers. Whether a carrier can someday be selected by a declaration-level criterion rather than only by role is a narrow, generic question for that model to answer if and when it becomes concrete, not a reason for a separate interactive-runtime RFC to exist.

This RFC's actual driving motivation -- a native, Omerus-backed interactive runtime target -- remains real, but Omerus does not yet have a concrete API surface to design against. Rejecting this RFC rather than leaving it open avoids Incan's core language and stdlib pre-committing to browser- or native-specific vocabulary before a real downstream consumer's design exists to validate it. This note is the only carried-forward record of that intent; if and when Omerus's own design solidifies, its actual needs should be designed there, not resurrected from this RFC's speculative shape.
