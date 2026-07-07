# 1.0 domain-native demo target

This page describes the public 1.0 demo shape Incan should earn. It is a target for positioning and evaluation, not a promise that every construct below exists in the current beta.

The demo should prove that Incan is not merely "Python but compiled." It should show language and package surfaces that Python cannot provide as ordinary syntax: typed models as contracts, domain query blocks, pipeline steps, quality gates, typed config, diagnostics, build reports, and artifact inspection.

## Target story

A developer trying the 1.0 demo should be able to clone a small project, run one command, and see a complete typed data workflow:

1. Load raw records.
2. Validate rows against typed models.
3. Transform data through typed query blocks.
4. Compose work through steps and a pipeline.
5. Apply quality gates at batch boundaries.
6. Emit diagnostics and build reports.
7. Inspect produced artifacts and compiler facts.

The important result is not only that the program runs, but that the code shape explains why Incan exists.

## Target source shape

This is target syntax, not a claim that every construct belongs in the base language. `model` is part of Incan's core language surface, while `DataFrame`, `query {}`, `step`, `pipeline`, and `quality:` may be owned by domain packages such as InQL or workflow/quality packages rather than the base vocabulary.

```incan
# Row contract: a typed shape that downstream query and quality code can reference.
model RawRepo:
    id: i64
    name: str
    language: Option[str]
    stars: i64

# Output contract for the analytics layer.
model LanguageStats:
    language: str
    repo_count: i64
    total_stars: i64

# Typed configuration replaces loose environment/string plumbing.
ctx PipelineConfig:
    input_path: str
    output_path: str
    min_stars: i64 = 10

# Step boundary: typed input/output, and a natural place for observability/retries.
step load_repos(config: PipelineConfig) -> DataFrame[RawRepo]:
    return read_json(config.input_path)

step aggregate_by_language(repos: DataFrame[RawRepo], config: PipelineConfig) -> DataFrame[LanguageStats]:
    # InQL-style query syntax should be checked against RawRepo fields.
    return query {
        FROM repos
        WHERE .stars >= config.min_stars
        GROUP BY .language
        SELECT
            language,
            count() as repo_count,
            sum(.stars) as total_stars,
        ORDER BY total_stars DESC
    }

pipeline repo_analytics(config: PipelineConfig):
    raw = load_repos(config)?

    # Batch-level quality belongs at the collection/pipeline boundary.
    quality raw:
        completeness("id") == 1.0
        uniqueness("id") == 1.0

    stats = aggregate_by_language(raw, config)?

    quality stats:
        row_count() > 0

    write_parquet(stats, config.output_path)?
```

Exact syntax may change, but the contract should not: the demo must make typechecked domain workflow code visible instead of burying the interesting behavior in a framework object or a README.

## Current versus planned surfaces

| Surface                  | Current posture                                                                 | 1.0 demo role                                                                           |
| ------------------------ | ------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------- |
| `model`                  | Existing language surface.                                                      | Row-level contracts and schema-shaped code.                                             |
| `ctx` / typed config     | Directional surface.                                                            | Environment-aware configuration without loose YAML/env glue.                            |
| `query {}`               | InQL/domain package direction.                                                  | Typechecked relational logic over model-shaped data.                                    |
| `step`                   | Planned/domain package direction.                                               | Operational unit with typed inputs, outputs, retries, observability, and failure paths. |
| `pipeline`               | Planned/domain package direction.                                               | Typed DAG/workflow composition rather than ad hoc function calls.                       |
| `quality:`               | Planned/domain package direction.                                               | Batch-level expectations tied to typed fields.                                          |
| Build report             | Existing CLI direction through `incan build --report json`.                     | Machine-readable artifact and dependency summary.                                       |
| Diagnostics              | Existing CLI direction through `incan check --format json` and `incan explain`. | Stable failure contract for humans, CI, editors, and agents.                            |
| Artifact/code inspection | Existing and expanding inspection direction.                                    | Show what was built, which facts are known, and what is intentionally not stable.       |

## What the demo should avoid

- A generic benchmark loop as the main story.
- A Python-like script whose only difference is native output.
- A framework-heavy example where the compiler cannot inspect the workflow shape.
- A Rust-interop example that requires users to understand Rust before they understand Incan.
- A data demo that relies on hidden runtime validation instead of typed contracts and diagnostics.

## Related docs

- [What Incan is for](what_incan_is_for.md)
- [Incan vs Python](../comparisons/python.md)
- [CLI reference](../tooling/reference/cli_reference.md)
- [Codegraph inspection](../tooling/reference/codegraph_inspection.md)
