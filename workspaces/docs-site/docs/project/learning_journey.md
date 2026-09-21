# Learning journey QA

This page records the acceptance contract for Incan's public learning journey. It defines what to test; it does not claim that reader research or analytics have already produced the target results.

## North star

A newcomer should be able to decide whether Incan fits, choose a route that matches their goal or background, run a first program, build one representative real thing, and identify the next step without reading the complete reference taxonomy.

## Route contract

The [Learn hub](../start_here/index.md) exposes six intents:

1. **Evaluate** — determine fit and maturity.
2. **Try** — complete the canonical first-contact loop.
3. **Learn** — study language fundamentals.
4. **Build** — choose a representative project.
5. **Bridge** — translate an existing mental model.
6. **Reference** — look up an established contract.

The target is one click from the Learn hub to the correct intent route and no more than two clicks from the hub to a representative project.

## Audience lenses

| Lens | Correct first route | Representative project | Boundary that must remain explicit |
| --- | --- | --- | --- |
| Python application developer | Python bridge | Typed data processor or typed API | No CPython or PyPI compatibility |
| Rust evaluator | Rust bridge | Build and consume an Incan library | Incan is not a low-level Rust replacement; crate interop remains explicitly scoped |
| TypeScript/JavaScript developer | TS/JS bridge | Typed API or library package | No JavaScript runtime or npm package compatibility |
| Automation/data-tool developer | Pipelines and automation | Pipeline mini-project or failed-build diagnosis | Relational query work is IncQL-shaped; hosted delivery is a preview |
| Technical evaluator | What Incan is for | Any representative project | Beta, release line, and deployment facts stay visible |
| Contributor | Contributor documentation | Contributor Book | Not part of the minimum user onboarding route |

## Structured route walkthrough (2026-08-10)

This implementation walkthrough tests the published navigation as each audience lens; it is not a substitute for observing independent readers. Click counts start on the Learn hub.

| Lens | Route exercised | Clicks to route | Representative outcome | Total clicks | Result |
| --- | --- | ---: | --- | ---: | --- |
| Python application developer | Coming from Python | 1 | Your first project or pipeline mini-project | 2 | Pass |
| Rust evaluator | Coming from Rust | 1 | Build and consume an Incan library | 2 | Pass |
| TypeScript/JavaScript developer | Coming from TypeScript or JavaScript | 1 | Your first project or library package | 2 | Pass |
| Automation/data-tool developer | Pipelines and automation | 1 | Pipeline mini-project or failed-build diagnosis | 2 | Pass |
| Technical evaluator | What Incan is for | 1 | Representative executable project or an explicitly labeled packaging preview | 2 | Pass |
| Contributor | Contributor documentation | Outside minimum onboarding | Contributor Book | Outside minimum onboarding | Intentionally separate |

The walkthrough found no dead route or extra taxonomy decision before a recommended executable project. Web, typed-serde, async, library, and scoped crate-interoperability work is executable in the completed 0.5 envelope; hosted-delivery packaging remains explicitly labeled as a preview. The walkthrough did not test comprehension, terminology, or confidence with external readers; record those observations before adding further tutorial categories.

## Manual test

For each lens:

1. Start at the homepage without using the sidebar.
2. State what Incan is for and one case where it does not fit.
3. Reach the lens-specific bridge or evaluation page in one click through Learn.
4. Reach the representative project in at most one additional click.
5. Identify prerequisites, verified release line, expected outcome, artifacts, completion condition, and next step before reading the full tutorial.
6. Confirm that installation and the first project loop lead back to Getting Started rather than a copied command block.
7. Confirm that every remaining preview names the missing packaging boundary before presenting commands.

Record the chosen route, clicks, wrong turns, unclear terms, and missing prerequisites. Do not add tutorials based on page count alone; use repeated search or route confusion as the evidence for new material.

## Automated guard

`make docs-check-learning` verifies:

- the six route markers and representative project links;
- the self-contained Incan/IncQL boundary with no dead external destination;
- normalized Python, Rust, TS/JS, and automation bridge structure;
- single-source installer and first-project snippets;
- metadata, completion, and next-step contracts on project tutorials;
- executable-versus-preview status on every project tutorial and Learn card;
- the prepared 0.5 release-envelope command in the development-toolchain source of truth;
- the Learn navigation label and this QA contract.

The automated check protects structure. It cannot replace observation of real readers.
