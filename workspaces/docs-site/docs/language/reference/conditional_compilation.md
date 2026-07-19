# Conditional compilation

Incan package features can condition declarations and other compilation-unit facts with `when feature("name"):`. The predicate is evaluated from the current package's resolved public feature set before semantic facts become visible to compilation. It is not a runtime branch.

```incan
when feature("json"):
    from json_support import JsonEncoder

    pub def encode(value: Report) -> str:
        return JsonEncoder.new().encode(value)
```

## Grammar

The v0.5 form is:

```text
feature-condition  ::= "when" feature-predicate ("and" feature-predicate)* ":" NEWLINE INDENT declaration+ DEDENT
feature-predicate  ::= "feature" "(" STRING_LITERAL ")"
```

Every predicate is positive and names a feature owned by the current package. A conjunction requires every named feature:

```incan
when feature("json") and feature("pretty"):
    pub def render_pretty(value: Report) -> str:
        ...
```

Nested feature blocks are equivalent to a conjunction. The formatter sorts and deduplicates requirements so equivalent conditions have one canonical spelling.

## What may be conditioned

A feature block may contain compilation-unit declarations, including imports, public reexports, functions, models, classes, traits, enums, constants, static storage, aliases, registry declarations, and nested feature blocks. A condition applies to every checked fact derived from declarations in its body, including documentation and provider implementation requirements.

Inactive declarations remain parseable syntax for formatting and editor navigation, but they do not enter the active typechecking, lowering, generated-Rust, documentation, or codegraph projection. Tooling that needs another projection must select the corresponding package features rather than interpreting the source independently.

## Restrictions

- `feature(...)` accepts one string literal containing a local package-feature name.
- Dependency-qualified names such as `dependency/feature` are invalid in source conditions; dependency features are selected on dependency edges in `incan.toml`.
- `not`, `or`, target predicates, values, and arbitrary expressions are not supported.
- A feature condition is valid only at compilation-unit scope. It is not an `if` expression and cannot inspect runtime state.
- Features are additive. Enabling a feature may contribute API or dependencies but must not subtract or reinterpret an unconditional API.

Declare package features and dependency edges in [`incan.toml`](../../tooling/reference/project_configuration.md#projectfeatures), select them through manifest dependencies or Incan CLI feature flags, and inspect the resolved projection with `incan inspect features --format json`. See [SDK components and package features](../../tooling/reference/sdk_components_and_package_features.md) for the full resolution model.
