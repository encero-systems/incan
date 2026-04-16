# RFC 057: `@rust.allow(...)` — targeted Rust lint suppression for generated code

- **Status:** Draft
- **Created:** 2026-04-13
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 005 (Rust interop)
    - RFC 023 (compilable stdlib and Rust module binding)
    - RFC 024 (extensible derive protocol and `@rust.*` decorators)
    - RFC 033 (`ctx` keyword and generated initialization)
    - RFC 036 (user-defined decorators and compiler built-ins)
    - RFC 041 (first-class Rust interop authoring)
- **Issue:** https://github.com/dannys-code-corner/incan/issues/337
- **RFC PR:** —
- **Written against:** v0.2
- **Shipped in:** —

## Summary

This RFC introduces `@rust.allow(...)` as a narrowly scoped built-in decorator for suppressing specific Rust or Clippy lints on generated Rust items emitted from Incan declarations. The purpose is not to create a general Rust-attribute escape hatch; it is to let authors acknowledge unavoidable Rust-target warnings such as `deprecated` or `clippy::unwrap_used` at the smallest valid scope, instead of relying on blanket file-level `#![allow(...)]` attributes or forcing users to fall through to handwritten Rust.

## Core model

1. `@rust.allow(...)` is an item-level declaration that requests suppression of one or more named Rust lints for the Rust item generated from that Incan declaration.
2. The decorator is intentionally narrow: it may suppress specific lints, but it does not expose arbitrary Rust attributes, deny/forbid controls, or crate-wide warning policy.
3. The compiler must emit the suppression at the smallest valid Rust scope for the generated item, preserving ordinary Rust diagnostics everywhere else.
4. The decorator exists because some warnings in generated Rust are real but not avoidable from the Incan surface; the correct response in those cases is explicit, local acknowledgement, not global silence.

## Motivation

Generated Rust sometimes triggers warnings that are factually correct but not meaningfully fixable from Incan source. A common example is deprecation warnings caused by upstream crate APIs that Incan must still call for compatibility. Another is generated helper code that intentionally uses `unwrap()` or `expect()` to preserve fail-fast semantics in startup or host-boundary code. Today the project has two unsatisfying answers to that problem: emit coarse `#![allow(...)]` attributes over broad generated files, or tell authors to drop into handwritten Rust just to express a local lint policy. Both are wrong.

Incan already treats Rust interop as a first-class authoring surface rather than an implementation accident. If the language lets authors say `@rust.extern(...)` and `@rust.derive(...)`, it should also let them say "this specific generated Rust item needs this specific lint suppression" without opening the door to arbitrary Rust-attribute injection.

This is also a documentation and review problem. Blanket `allow(...)` at file scope hides too much and ages badly. A narrow, explicit `@rust.allow("deprecated")` on one declaration says exactly what is being suppressed and why the suppression exists.

## Goals

- Let Incan authors request specific Rust lint suppressions on declarations that lower to Rust items.
- Keep suppressions narrow and local instead of relying on blanket generated-file `#![allow(...)]` attributes.
- Reuse the existing `@rust.*` authoring model rather than inventing a new top-level keyword.
- Support both rustc lints such as `deprecated` and namespaced tool lints such as `clippy::unwrap_used`.
- Keep Rust diagnostics visible by default everywhere the author did not explicitly opt out.

## Non-Goals

- Exposing arbitrary Rust attributes or a general `@rust.attr(...)` escape hatch.
- Adding `@rust.deny(...)`, `@rust.forbid(...)`, or full Rust lint-policy management in this RFC.
- Providing crate-wide or package-wide lint configuration through `incan.toml` or CLI flags.
- Guaranteeing that every possible Rust warning can be suppressed from every possible generated artifact.
- Replacing ordinary compiler warnings or lints on Incan source itself.

## Guide-level explanation

Authors use `@rust.allow(...)` when generated Rust is expected to trigger a specific warning that is legitimate but unavoidable from the Incan side.

```incan
@rust.allow("deprecated")
fn transform_write_order_lines_csvincn(session: Session, input_uri: str, output_uri: str) -> Result[None, SessionError]:
    lines = session.read_csv(OrderLine)(uri=input_uri)?
    transformed = lines.filter(rule_order_by_lineid)
    return session.write_csv(transformed, output_uri)
```

The intent is precise: this one declaration may emit Rust that uses a deprecated upstream field or API, and that warning should not leak into every build of the generated project.

Multiple targeted suppressions may be attached to the same declaration:

```incan
@rust.allow("deprecated", "clippy::unwrap_used")
fn boot_runtime() -> Runtime:
    return Runtime.from_env().unwrap()
```

The mental model is simple:

- `@rust.allow(...)` is for unavoidable Rust-target warnings in generated code;
- it is declaration-local, not project-wide;
- it should be rare and justified;
- it does not give users arbitrary Rust attribute power.

## Reference-level explanation

### Surface syntax

`@rust.allow(...)` is a compiler built-in decorator.

- The decorator must take one or more string literal arguments.
- Each argument names one Rust lint to suppress.
- A bare rustc lint such as `"deprecated"` is valid.
- A tool-prefixed lint such as `"clippy::unwrap_used"` is valid.
- Empty argument lists must be rejected.
- Non-string arguments must be rejected.

Examples:

```incan
@rust.allow("deprecated")
fn f() -> None:
    pass
```

```incan
@rust.allow("clippy::unwrap_used", "clippy::expect_used")
fn g() -> None:
    pass
```

### Valid attachment points

`@rust.allow(...)` may appear only on declarations that lower to one or more concrete Rust items owned by that declaration.

This RFC commits to support at least:

- top-level functions;
- methods;
- type-like declarations that lower to Rust items such as models, enums, classes, and newtypes.

The decorator must not be accepted on:

- local variable bindings;
- expressions;
- imports;
- module-level statements that do not own a Rust item;
- declarations whose lowering does not produce a stable Rust item boundary.

### Emission semantics

The compiler must emit the suppression on the smallest valid Rust scope that covers the generated item for the annotated declaration.

Normative consequences:

- If an annotated function lowers to a Rust function, the Rust function must carry the emitted `#[allow(...)]` attribute.
- If an annotated type declaration lowers to a Rust struct or enum, the emitted type item must carry the attribute.
- If the compiler generates helper Rust items that exist solely to implement the annotated declaration, the compiler may propagate the same suppression to those helper items when that is necessary to make the annotation truthful.
- The compiler must not silently widen an item-level `@rust.allow(...)` into a crate-wide `#![allow(...)]`.

### Lint-name handling

The compiler should validate lint names conservatively.

- It must reject empty strings.
- It must reject duplicate names within the same decorator invocation after normalization.
- It must reject obvious broad catch-all forms such as `"warnings"` and `"clippy::all"`.
- It may otherwise treat lint names as opaque Rust-style paths and leave final unknown-lint validation to the Rust toolchain.

Normalization rules:

- surrounding whitespace inside the string literal is not permitted;
- lint names are case-sensitive and must be preserved as written;
- duplicate names after exact-string comparison are redundant and must be rejected or deduplicated consistently.

### Interaction with other `@rust.*` decorators

`@rust.allow(...)` composes with existing built-in Rust decorators such as `@rust.extern(...)` and `@rust.derive(...)`.

- Ordering must not change semantics: multiple built-in `@rust.*` decorators on the same declaration describe emission metadata for that declaration and may be accumulated together.
- `@rust.allow(...)` does not change call semantics, trait semantics, or Rust binding targets; it only affects Rust lint emission.
- The presence of `@rust.allow(...)` must not disable ordinary Incan-side diagnostics on the declaration.

### Documentation and diagnostics

When a declaration uses `@rust.allow(...)`, tooling should surface that fact in hover or generated docs where Rust-emission metadata is already shown.

Compiler diagnostics for invalid use should be explicit. Examples:

- `@rust.allow requires at least one lint name`
- `@rust.allow expects string literal lint names`
- `@rust.allow("warnings") is too broad; suppress a specific lint instead`
- `@rust.allow cannot be used on local bindings`

## Design details

### Why this should be `@rust.allow(...)`

The `@rust.*` namespace already carries Rust-emission intent. `@rust.extern(...)` declares a Rust implementation boundary, and `@rust.derive(...)` declares Rust derive metadata. Lint suppression belongs in that same family because it is also Rust-emission metadata, not a pure Incan semantic feature.

### Why this should stay narrow

The project should not smuggle arbitrary Rust attributes into the language under the banner of pragmatism. That would turn a focused fix for unavoidable warnings into a general backdoor for backend-specific policy, making code less portable, less reviewable, and harder to teach. `@rust.allow(...)` is justified because the problem is real and recurring. A generic `@rust.attr(...)` would be unjustified because it collapses design judgment into "just pass raw Rust through."

### Why blanket file-level `allow(...)` is the wrong default

Generated file-level `#![allow(...)]` is easy to add and hard to defend. It suppresses warnings outside the actual local problem, makes reviews less precise, and encourages laziness in generated-code hygiene. This RFC pushes the design in the other direction: if a suppression is needed, the declaration that needs it should say so.

### Why this RFC does not include `deny` / `forbid`

Suppression is the immediate hole because generated Rust sometimes needs a narrower escape hatch than the compiler currently provides. Escalating warnings to errors is a different policy problem with different ergonomics and failure modes. It can be proposed later if there is a clear Incan-side use case, but bundling it here would over-broaden the RFC.

### Relationship to unavoidable deprecations

Deprecation warnings are the motivating case for this RFC. Generated Rust may need to call deprecated upstream fields or functions because the newer path is not yet available, or because the host library has no non-deprecated equivalent for the required behavior. When that happens, the right contract is explicit acknowledgement on the affected declaration, not silence for the whole generated module.

### Relationship to compiler-generated helpers

Some Incan declarations expand into more than one Rust item. If a helper exists solely because a declaration requires it, then the author's `@rust.allow(...)` annotation should remain truthful for that lowered shape. This RFC therefore permits propagation to helper items that are semantically owned by the annotated declaration, while still forbidding escalation into blanket crate-level allows.

## Alternatives considered

1. **Do nothing**
   - Rejected because the problem is real and already pushes users toward broad generated-file suppressions or handwritten Rust.

2. **Keep blanket file-level `#![allow(...)]` attributes**
   - Rejected because they are too broad, hide unrelated warnings, and work against reviewability.

3. **Expose arbitrary Rust attributes**
   - Rejected because it is far too broad. The language needs a narrow lint-suppression tool, not a general backend escape hatch.

4. **Put lint policy in `incan.toml`**
   - Rejected for this RFC because the motivating need is local and declaration-specific, not package-global.

5. **Require users to write Rust wrappers manually**
   - Rejected because it defeats the point of first-class Rust interop authoring from Incan.

## Drawbacks

- This introduces one more backend-facing decorator into the language surface.
- The exact boundary of "owned helper items" requires careful documentation so the feature does not feel magical.
- Conservative validation means some invalid lint names will still only be caught by the Rust toolchain rather than by Incan itself.
- Once the feature exists, there will be pressure to broaden it into general Rust-attribute passthrough. That pressure should be resisted unless a future RFC makes a much stronger case.

## Implementation architecture

*(Non-normative.)* A practical implementation stores `@rust.allow(...)` metadata on supported declarations alongside other Rust-emission metadata and threads it through lowering to the Rust emitter. Emission should prefer outer attributes on concrete Rust items and only propagate to generated helper items when those helpers are semantically owned by the annotated declaration.

## Layers affected

- **Parser / AST**: parse `@rust.allow(...)` as a built-in decorator carrying one or more string literal lint names.
- **Typechecker / symbol resolution**: validate legal attachment points and report misuse clearly.
- **Emission**: lower the recorded lint names into targeted Rust `#[allow(...)]` attributes on emitted items.
- **Formatter**: preserve and format `@rust.allow(...)` consistently with other decorators.
- **LSP / Tooling**: surface the decorator in hover, completion, and diagnostics for invalid use.
- **Docs / examples**: document when targeted suppression is appropriate and when it is papering over a fixable problem.

## Unresolved questions

- Should this RFC include a module-level `rust.allow(...)` directive for the few cases where the narrowest valid Rust scope is genuinely not an item owned by one declaration, or should v1 stay item-only?
- Should the compiler reject additional broad lint-group names such as `"unused"` or `"clippy::pedantic"` up front, or is rejecting only obvious catch-alls enough for v1?
- Should `@rust.allow(...)` be accepted on every type-like declaration from day one, or should the first implementation be restricted to functions and methods until multi-item lowering behavior is better characterized?

<!-- Rename this section to "Design Decisions" once all questions have been resolved. An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
