# Extend the language

Start from the observable source contract and trace a nearby implementation before editing a compiler stage.

## Choose the owning boundary

Use an ordinary Incan library function when the behavior can be expressed in the language. A builtin needs a language-owned contract; new syntax needs parser, formatter and checker support as well as native behavior. If the syntax belongs to a library and should only activate after import, use an Incan vocabulary entry; see [Author a vocabulary in Incan](authoring_vocabularies.md).

## Read a local precedent

Locate the relevant declaration with codegraph queries. Read two or three same-stage examples. Identify the registry or checked representation that owns the fact.

The pipeline is Oven unit description → parser → checker and Body IR → Incan lowering → native plan → Rust MIR adapter. [Architecture](../explanation/architecture.md) lists the source directories.

## State and prove behavior

Extend an existing contract case with accepted and refused forms. Use a focused selector to establish the initial failure. Keep expectations about program output or diagnostics, rather than private compiler mechanics.

## Implement the checked fact once

Have checking record the resolved behavior in Body IR. Have Incan lowering read that fact. Keep rustc-specific API translation in the Rust adapter, and runtime behavior in Incan runtime or standard packages. Do not re-resolve types or names in lowering.

## Complete the tooling boundary

Update formatting, source diagnostics and graph facts when the language surface changes. Verify aliases, imported modules and public library consumers retain the same declaration identities.

## Validate and document

Run the focused case, then affected whole families. Audit changed Rust functions for rustdoc and changed Incan for readable structure. Run `make pre-commit-fast`, `make reference-contract-check` for references and `make docs-build`.

Public references state syntax, types, defaults and refusals. Task procedures belong in how-tos; rationale belongs in explanations.
