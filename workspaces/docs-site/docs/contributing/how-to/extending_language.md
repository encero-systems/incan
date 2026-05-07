# Extending Incan: Builtins vs New Syntax

This document is for **contributors** who want to add new language features.

Incan is implemented as a **multi-stage compiler**:

- Frontend: Lexer → Parser → AST → Typechecker (`typechecker/`)
- Backend: Lowering (AST → IR) → Emitter (IR → Rust)

That separation is intentional (clarity, correctness, debuggability), but it means that adding *new syntax* typically
touches multiple stages.

!!! warning "Required reading (contributors)"
    Before making language changes, read these end-to-end:

    - [Incan Compiler Architecture](../explanation/architecture.md) — internal pipeline + module layout
    - [How Incan works](../../language/explanation/how_incan_works.md) — conceptual pipeline schematic
    - [RFC index](../../RFCs/index.md) — required for *new* language features (syntax/semantics), not for bugs/chores

## Architecture schematic (high-level)

```mermaid
--8<-- "_snippets/diagrams/compiler_pipeline.mmd"
```

---

## Where things live (crates and modules)

Incan’s “language surface” spans a small number of key crates/modules:

|     Crate/Module      |                                        Purpose                                         |
| --------------------- | -------------------------------------------------------------------------------------- |
| `crates/incan_syntax` | Lexer/parser/AST/diagnostics (shared by compiler, formatter, and LSP to prevent drift) |
| `crates/incan_core`   | Semantic registries + pure helpers shared across the ecosystem (should not drift)      |
| `crates/incan_stdlib` | Runtime support for generated programs (preferred home for “just a function” behavior) |
| `crates/incan_derive` | Derives used by generated Rust programs (runtime-side)                                 |
| `src/frontend`        | Module resolution + typechecker (turns syntax into a typed program)                    |
| `src/backend`         | Lowering + IR + emission (turns typed program into Rust)                               |
| `src/format/`         | Source formatter (`incan fmt`)                                                         |
| `src/lsp/`            | Language server (reuses frontend to provide IDE diagnostics)                           |

When you’re unsure where to implement something, start by deciding which crate owns the responsibility.

## Rule of Thumb

**Prefer a library/builtin over a new keyword.**

Add a new **keyword / syntax form** only when the feature:

- **Introduces control-flow** that cannot be expressed as a call (e.g. `match`, `yield`, `await`, `?`)
- Requires **special typing rules** that would be awkward or misleading as a function
- Needs **non-standard evaluation** of its operands (short-circuiting, implicit returns, pattern binding, etc.)

If the feature is “some behavior” (logging, printing, tracing, helpers), it should usually be:

- A **stdlib function** (preferred), or
- A **compiler builtin** (when it must lower to special Rust code).

If the syntax belongs to a library and should only activate after import, do **not** add it to the core compiler first. Use a vocab companion crate instead; see [Author library DSLs with `incan_vocab`](authoring_vocab_crates.md).

---

<!-- markdownlint-disable MD033 -->
## <small>*Path A*:</small> Adding a Function (Stdlib or Compiler Builtin)

### <small>*A.1*: </small>  Stdlib function (no new syntax)
<!-- markdownlint-enable MD033 -->

Use this when the behavior can live in runtime support crates (e.g. `incan_stdlib`), without compiler special casing.

Typical work:

- Add runtime implementation in `crates/incan_stdlib/`
- Expose it via the prelude if appropriate
- Document it in the language guide

This avoids changing the lexer/parser/AST/IR.

<!-- markdownlint-disable MD033 -->
### <small>*A.2*: </small> Compiler builtin function (special lowering/emission)
<!-- markdownlint-enable MD033 -->

Use this when you want a function-call surface syntax, but it must emit a particular Rust pattern.

Incan already has enum-dispatched builtins in IR (`BuiltinFn`) and emission logic in `emit/expressions/builtins.rs`.

**End-to-end checklist:**

- **Frontend symbol table**: add the builtin name and signature so it typechecks
    - `src/frontend/symbols.rs` → `SymbolTable::add_builtins()`
- **IR builtin enum**: add a new variant and name mapping
    - `src/backend/ir/expr.rs` → `enum BuiltinFn` + `BuiltinFn::from_name()`
- **Lowering**: ensure calls to that name lower to `IrExprKind::BuiltinCall`
    - `src/backend/ir/lower/expr.rs` uses `BuiltinFn::from_name(name)` for identifiers
- **Emission**: emit the Rust code for the new builtin
    - `src/backend/ir/emit/expressions/builtins.rs` → `emit_builtin_call()`
- **Docs/tests**: add/adjust as needed

This path is often **much cheaper** than adding new syntax, while still letting you control the generated Rust.

<!-- markdownlint-disable MD033 -->
### <small>*A.2*: </small> Compiler builtin method (special method lowering/emission)
<!-- markdownlint-enable MD033 -->

Use this when you want to add a method on existing types (e.g. `list.some_method()`) that needs special Rust emission.

Incan has enum-dispatched methods in IR (`MethodKind`) and emission logic in `emit/expressions/methods.rs`.

**End-to-end checklist:**

- **IR method enum**: add a variant to the appropriate method family and classify it for supported receiver types
    - `src/backend/ir/expr.rs` → `enum MethodKind` + `MethodKind::for_receiver()`
- **Lowering**: receiver-aware classification for builtin-family receivers
    - `src/backend/ir/lower/expr.rs` calls `MethodKind::for_receiver(&receiver.ty, name)` for method calls
- **Emission**: emit the Rust code for the new method
    - `src/backend/ir/emit/expressions/methods.rs` → `emit_known_method_call()`
- **Docs/tests**: add/adjust as needed

Unknown methods pass through as regular Rust method calls, so you don't break Rust interop by adding known methods.

---

<!-- markdownlint-disable MD033 -->
## <small>*Path B*:</small> Add a New Keyword / Syntax Form
<!-- markdownlint-enable MD033 -->

Use this only when the feature is genuinely syntactic/control-flow.

**End-to-end checklist (typical):**

**Lexer**: `crates/incan_syntax/src/lexer/*`

- Add a `KeywordId` **and a `KEYWORDS` entry** (canonical spelling/metadata) in `crates/incan_core/src/lang/keywords.rs`
- Ensure tokenization emits `TokenKind::Keyword(KeywordId::YourKeyword)`
- Update lexer parity tests (keyword/operator/punctuation registry parity)

!!! note "Word-operators (special case)"
    If the new “keyword” is meant to behave like an operator (it participates in expression precedence like `and`, `or`,
    `not`, `in`, `is`), treat it as a **word-operator**:

    - Add it to `crates/incan_core/src/lang/operators.rs` (precedence/fixity source of truth)
    - Add a corresponding `KeywordId` + `KEYWORDS` entry in `crates/incan_core/src/lang/keywords.rs` (so the lexer will
      still lex it as a keyword)
    - Update expression parsing in `crates/incan_syntax/src/parser/expr.rs` to place it at the right precedence level

**Parser**: `crates/incan_syntax/src/parser/*`

- Parse the syntax and build a new AST node (usually an `Expr` or `Statement` variant)

**AST**: `crates/incan_syntax/src/ast.rs`

- Add the new `Expr::<YourNode>` or `Statement::<YourNode>` variant

**Formatter**: `src/format/formatter.rs`

- Teach the formatter how to print the new node

**Typechecker**:`src/frontend/typechecker/`

- `check_decl.rs` – add type-level rules (models, classes, traits)
- `check_stmt.rs` – add statement-level rules (assignments, control flow)
- `check_expr/*.rs` – add expression-level rules (calls, operators, match)

**(Optional) Scanners**: `src/backend/ir/scanners.rs`

- Ensure feature detection traverses the new node if relevant

**Lowering (AST → IR)**: `src/backend/ir/lower/*`

- Lower the new AST node into an IR representation

**IR (if needed)**: `src/backend/ir/expr.rs` / `stmt.rs` / `decl.rs`

- Add a new `IrExprKind`/`IrStmtKind` variant if the feature is not expressible via existing IR

**Emitter (IR → Rust)**: `src/backend/ir/emit/**/*.rs`

- Emit correct Rust for the new IR node

**Editor tooling (optional but recommended)**: `editors/*`

- `editors/vscode/*`: keyword highlighting / indentation patterns

<!-- markdownlint-disable MD036 -->
**Docs + tests**
<!-- markdownlint-enable MD036 -->

- Add a guide snippet and at least one parse/typecheck/codegen regression test

---

<!-- markdownlint-disable MD033 -->
## <small>*Path C*:</small> Add a Soft Keyword via the Surface Semantics Engine
<!-- markdownlint-enable MD033 -->

[Surface Semantics Engine]:../explanation/architecture.md#surface-semantics-engine

Use this when the feature is an **import-activated keyword** whose behavior desugars to stdlib calls — not a
permanently reserved hard keyword. Current examples: `assert` (activated by `import std.testing`), `async`/`await`
(activated by `import std.async`).

This path avoids creating new IR variants. Instead, the parser emits a generic `Surface` AST node keyed by
`SurfaceFeatureKey`, and downstream stages (typechecker, lowering, scanning) consult the semantics registry for
**action descriptors** that tell them what to do. See the [Surface Semantics Engine] section of the architecture
docs for the full design.

### How the registry drives every compiler stage

The `SurfaceSemanticsPack` trait covers the full compiler:

<!-- markdownlint-disable MD013 -->

|    Stage     |                             Pack method                             |              Returns              |
| ------------ | ------------------------------------------------------------------- | --------------------------------- |
| Parser       | `statement_payload_*`, `expression_payload_*`, `modifier_payload_*` | Payload kind (how to parse)       |
| Typechecker  | `typecheck_surface_stmt_action`, `typecheck_surface_expr_action`    | Action descriptor (what to check) |
| Lowering     | `lower_surface_stmt_action`, `lower_surface_expr_action`            | Action descriptor (how to lower)  |
| Scanning     | `modifier_runtime_requirement`, `import_runtime_requirement`        | `RuntimeRequirement`              |
| Call targets | `assert_call_target`                                                | `SurfaceCallTarget`               |

<!-- markdownlint-enable MD013 -->

Action descriptors are small enums (e.g., `SurfaceStmtLoweringAction::AssertCall`,
`SurfaceExprTypeCheck::AwaitCheck`) that describe *what* the compiler should do, not *which keyword*
triggered it. Multiple keywords can share the same action pattern. If your keyword fits an existing
action pattern, the compiler already knows how to execute it — **no main-crate changes needed**.

### End-to-end checklist

**1. Keyword descriptor** (`crates/incan_core/src/lang/keywords.rs`):

- Add a `KeywordId` variant.
- Register it with `info_soft()`, specifying the activating stdlib namespace and the `KeywordSurfaceKind`
  (`StatementKeywordArgs`, `PrefixExpression`, or `DeclarationModifier`).

**2. Semantics pack** (`crates/incan_semantics_stdlib/src/lib.rs`):

- Implement the **parser routing** method (`statement_payload_for_soft_keyword`,
  `expression_payload_for_soft_keyword`, or `modifier_payload_for_soft_keyword`).
- Implement the **typechecker action** method (`typecheck_surface_stmt_action` or
  `typecheck_surface_expr_action`), returning an existing action descriptor if one fits.
- Implement the **lowering action** method (`lower_surface_stmt_action` or `lower_surface_expr_action`),
  returning an existing action descriptor if one fits.
- If the keyword implies a runtime requirement (e.g., async runtime), implement
  `modifier_runtime_requirement` and/or `import_runtime_requirement`.
- If the keyword desugars to a stdlib call, implement `assert_call_target()` (or the equivalent for your
  feature) returning a `SurfaceCallTarget` with the canonical callee path.
- Gate the handler behind the appropriate Cargo feature (`std_testing`, `std_async`, etc.).

**3. Parser**: No per-keyword code needed. The generic helpers — `current_surface_keyword()` and
`match_surface_keyword()` in `crates/incan_syntax/src/parser/helpers.rs`, and
`try_surface_keyword_statement()` in `crates/incan_syntax/src/parser/stmts.rs` — automatically
pick up any soft keyword whose descriptor has a matching `KeywordSurfaceKind`.

**4. Typechecker**: No per-keyword code needed if you returned an existing action descriptor in step 2.
`check_surface_stmt()` and `check_surface_expr()` query the registry and dispatch on the action.

**5. Lowering**: No per-keyword code needed if you returned an existing action descriptor in step 2.
`lower_surface_statement()` and the `Expr::Surface` arm of `lower_expr()` query the registry and dispatch
on the action.

**6. Formatter** (`src/format/formatter/`):

- Handle the new `Statement::Surface` or `Expr::Surface` variant (usually a one-liner using
  `keywords::as_str()`).

**7. Tests**:

- Add a codegen snapshot test (`.incn` file + snapshot) exercising the full surface path.
- Verify the snapshot shows canonical-path-resolved calls (e.g. `crate::__incan_std::*`).

!!! note "When you need a new action pattern"
    If no existing action descriptor fits your keyword's behavior, you'll need to:

    1. Add a new variant to the relevant action enum in `crates/incan_semantics_core/src/lib.rs`
       (e.g., `SurfaceStmtLoweringAction::YourPattern`).
    2. Add a handler arm in the corresponding compiler module (`lower_surface_statement()`,
       `check_surface_stmt()`, etc.).

    This is deliberately rare — action descriptors represent *compiler behavior patterns*, not individual keywords.
    You might add many keywords before needing a new pattern.

!!! tip "Key difference from Path B"
    Path B creates new AST variants and new IR variants for each keyword. Path C reuses the generic `Surface` AST nodes
    and the existing `IrExprKind::Call` with `canonical_path` metadata — so you don't need to touch the IR definition or
    the emitter's call-emission logic at all.

---

## Practical guidance

- If you find yourself adding a keyword to achieve *“a function with a special implementation”*, pause and consider making
  it a **builtin function** (or a decorator) instead.
- If you add a new AST/IR enum variant, rely on Rust’s exhaustiveness errors as your checklist: the compiler will tell you
  which match arms you need to update.
- Keywords may only be introduced as the result of a language change (RFC).
