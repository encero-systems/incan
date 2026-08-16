# Incan or IncQL?

Incan and IncQL address different problem shapes. Choose the boundary first; do not translate a relational problem into application code merely because both systems work with data.

## Choose Incan-shaped work

Use Incan when the thing you are building is primarily:

- an application, command-line tool, service, worker, or operational workflow;
- typed control flow around files, APIs, messages, models, errors, and external libraries;
- a native executable whose behavior should be tested, built, and deployed as a program;
- orchestration that must make side effects and failure boundaries explicit.

A command-line application, explicit workflow step, checked library boundary, or service controller is Incan-shaped. The prepared 0.5 release envelope executes command-line applications and workflow steps; library packages, web, typed-serde, async, and arbitrary-crate examples are separately labelled source-verified previews where the finite envelope lacks their native Loaf.

## Choose IncQL-shaped work

Treat work as IncQL-shaped when it is primarily:

- relational querying or transformation;
- logical and physical query plans;
- lineage, dependency, or semantic relationships between datasets;
- SQL, database, dataframe, or Spark-style analysis whose core abstraction remains a query plan.

IncQL's public learning site is not available yet. This page records the boundary without sending you to a dead destination or pretending that Incan is a SQL, dataframe, or Spark compatibility layer.

## Use both when the boundary is real

An application may be Incan-shaped at its service, workflow, error, and deployment boundaries while delegating relational planning to IncQL. That does not collapse the two languages into one: Incan owns program behavior; IncQL owns query-shaped work.

## Continue

- If your work is Incan-shaped, [run your first Incan project](../tooling/tutorials/getting_started.md).
- If you are still evaluating the fit, read [What Incan is for](what_incan_is_for.md).
- For the broader product context, see [The Encero stack](encero_stack.md).
