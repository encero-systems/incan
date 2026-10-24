# Author a vocabulary in Incan

Use an Incan vocabulary entry to publish a library's import-activated block syntax. This guide adds a query expression that returns its `FROM` value. The manifest and execution contract is in [Vocabulary entries](../../tooling/reference/vocabulary.md); type and method signatures are in [`std.vocab`](../../language/reference/stdlib/vocab.md).

## Register the surface

Declare the entry in the producer's `loaf.toml`:

```toml
[project]
name = "queries"
version = "0.1.0"

[vocab]
entry = "src/vocab.incn"
```

Export the registration from that module. This example registers an expression named `query` with a required `FROM` clause, activated by importing the `queries` library:

```incan
from std.vocab import ClauseSurface, DeclarationSurface, DslSurface, VocabRegistration
from std.vocab import DesugarError, DesugarOutput, DesugarRequest, DesugarResponse, VocabSyntaxNode


pub def library_vocab() -> VocabRegistration:
    """Register the library's import-activated query expression."""
    mut declaration = DeclarationSurface.named("query").with_clause_body().desugars_to_expression()
    declaration = declaration.with_clause(ClauseSurface.expr("FROM").required())
    surface = DslSurface.on_import("queries").with_declaration(declaration)
    return VocabRegistration.new().with_surface(surface).with_desugarer()
```

The `.with_desugarer()` registration requires the public typed export below. Compiler-owned surfaces that need no custom expansion omit it.

## Define the expansion

Add this function below registration in the same entry module:

```incan
pub def desugar(request: DesugarRequest) -> Result[DesugarResponse, DesugarError]:
    """Return the FROM expression with its original source location."""
    match request.node:
        VocabSyntaxNode.Declaration(declaration) =>
            clause = declaration.clause("FROM")?
            expression = clause.expression()?
            return Ok(DesugarResponse(output=DesugarOutput.Expression(expression)))
        _ => return Err(DesugarError.at(request.node.span(), "query requires a declaration"))
```

Return structured public syntax, never generated source. Each returned node carries the original-source span it came from. Preserve a clause expression's own span when returning it so a later type error points at that expression. A newly constructed node may use any span inside its input node; an omitted output span inherits the whole input node's range.

Keep the entry separate from your ordinary library module. In `src/lib.incn`, expose the symbol the consumer imports to activate the vocabulary:

```incan
pub def marker() -> int:
    """Return a value usable inside an expanded query."""
    return 7
```

Build the producer with `incan build path/to/queries`. Oven generates the worker's main, builds the entry and its imports for the host, and publishes its registration. A consumer importing the library reads those descriptors.

## Use the library

Add the producer as a path dependency in the consumer's `loaf.toml`, with an explicit executable entry:

```toml
[project]
name = "query_consumer"
version = "0.1.0"

[project.scripts]
main = "src/main.incn"

[dependencies]
queries = { path = "../queries" }
```

In the consumer's `src/main.incn`, import the ordinary public symbol and use the registered syntax:

```incan
from pub::queries import marker


def main() -> None:
    """Execute the query's expanded expression as ordinary Incan."""
    value = query:
        FROM marker() + 1
    println(value)
```

Run `incan run path/to/query_consumer`. This prints `8`. Provider diagnostics point at the requesting source node. For error conditions and execution bounds, see the [entry reference](../../tooling/reference/vocabulary.md#publication-and-execution).

## Choose an output position

Set `desugars_to_expression()`, `desugars_to_statements()` or `desugars_to_declarations()` on the declaration descriptor, and return the matching `DesugarOutput` variant. Use declaration output at compilation-unit scope to introduce functions, models or constants through `IncanDeclaration`. Keep each generated declaration's genuine source range; its fields, annotations and body values retain their own ranges. See [declaration output](../../language/reference/stdlib/vocab.md#compilation-unit-output) for the accepted shapes.

For scoped glyphs or leading-dot paths, register `ScopedSurfaceDescriptor` values with `DslSurface.with_scoped_surfaces(...)`. Match the request's `IncanExprKind.Scoped` values by their descriptor identity and return ordinary Incan syntax. The querykit, routekit and studiokit entries under `examples/pro/` contain complete implementations.

## Inspect admission and reuse

Run `OVEN_TRACE=- incan check src/main.incn` to see vocabulary decisions. The default policy admits exact units in the build's provider lock. Set `INCAN_VOCAB_POLICY=deny` to refuse provider execution even for a resolved dependency. The build records the provider, vocabulary unit identity, module, admission decision and computed/reused response under `target/incan/vocab-responses/evidence.jsonl`.

Providers must be pure functions of their unit identity and request. Responses can be reused, and the compilation owner can retain workers. No capabilities are granted to a worker invocation. This is an admission boundary rather than an operating-system sandbox; ambient filesystem, environment, clock or network access is unsupported.

Keep registration deterministic and reserve a desugarer's stdout for its protocol. Use `DesugarError.at(...)` to return a diagnostic at a genuine input range. Rebuild a provider when its published syntax version is unsupported.

## The submode catalog

RFC 081's parser catalog contains markup, style, raw text, regex/template, selector/declaration-value and type-position submodes. Their lexical grammar, expression holes, ownership and formatter behavior are unchanged. Register an `EmbeddedFragmentDescriptor` with `DslSurface.with_embedded_fragment(...)`, selecting its `EmbeddedFragmentSubmode` and eligible declaration or clause body. Embedded block bodies currently use statement position.

A registered native desugarer receives the structured artifact through `VocabDeclaration.fragment()`. Its `VocabFragmentNode` tree retains descriptor identity, source ranges, attributes and ordinary Incan expression holes. Return ordinary Incan syntax from the hook, retaining any original hole expressions that contribute to that output. A fragment with no hook follows its existing checked-body path and explicit emission refusal.

When constructing a wrapper call around an original hole, assign the wrapper the enclosing fragment's range and keep the hole's finer range when available. Nested generated calls may share exactly one original-source range. Keep genuine coordinates even when several output nodes came from the same input; no artificial offsets are needed to distinguish them.
