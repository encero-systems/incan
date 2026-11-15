# 5. Your first syntax change

New syntax is the most invasive type of change: it touches multiple stages and increases long-term maintenance cost.

This chapter shows how to do it safely and predictably.

!!! warning "RFC required for new language features"
    If you are proposing a **new** user-visible language feature (syntax or semantics), write an RFC first:

    - [RFC index](../../../RFCs/index.md)
    - [RFC template](../../../RFCs/TEMPLATE.md)

    Bugfixes and chores do not require an RFC.

## Before you start

Read:

- [Extending the language](../../how-to/extending_language.md) (the builtin-vs-syntax decision and checklists)

## Your mental checklist

--8<-- "_snippets/contributing/compiler_pipeline_checklist.md"

You should expect Rust exhaustiveness errors to guide your work when you add enum variants.

## Pick a “first syntax change” that stays small

A good first syntax change:

- is local (one new statement or expression form),
- has a simple typing rule,
- has a direct checked Body IR and native lowering representation,
- can be covered by 1–2 regression tests.

Avoid starting with a feature that requires new runtime types, new module system rules, or complex ownership behavior.

## Next

Next chapter: [06. Tooling loop: formatter + tests](06_tooling_loop_formatter_and_tests.md).
