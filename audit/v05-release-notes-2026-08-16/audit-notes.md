# Incan v0.5 release-notes and campaign DX audit

Date: 2026-08-16

## North star

A campaign visitor should understand within five seconds that Incan v0.5 is a readable language for building real native applications and libraries. Within one click, that visitor should reach a representative project; after installation, the documented commands should produce a visible native result without requiring repository knowledge or an undocumented recovery step.

## Executive finding

The release contains a compelling acquisition story, but the current release-notes page tells it from the compiler inward. Its opening gives equal visual weight to internal governance, Oven Alpha, migration, and the first-project CTA. The result is accurate and technically credible, but it asks a newcomer to understand how the implementation became coherent before showing what they can now build.

The main v0.5 story should be:

> **Readable source. Real native systems.**
>
> Incan v0.5 brings applications, libraries, workspaces, operational standard-library APIs, and checked native integration into one coherent toolchain.

Oven belongs later as an experimental proof of where native builds are going. It should not share the headline with the user-facing application platform that v0.5 makes possible.

## Current public-state warning

As observed on 2026-08-16:

- GitHub's latest published release is v0.4.0; no v0.5 release artifact or tag is public yet.
- The v0.5 milestone remains open with five items, including three user-facing defects: #1053, #1055, and #1057.
- The release page links to a Getting Started tutorial explicitly verified against `>=0.4.0,<0.5.0`.
- The install documentation says the canonical installer resolves v0.4 and requires a source checkout for v0.5.

Paid traffic should not be sent to the current page until the public v0.5 artifact, installer path, and canonical first-project journey agree with the release page.

## Journey audit

1. **Ad or shared link to release notes — needs work.** The page opens with implementation accountability and Oven rather than a concrete user outcome. There is no immediately visible project result, source example, or native artifact.
2. **Understand the value — needs work.** The most important user-facing changes are spread across six dense sections. Internal names such as `CompilationSession`, semantic snapshots, HIR, provider plans, and schema details arrive before a newcomer has accepted the product promise.
3. **Choose a test drive — at risk.** There is one weakly presented start card rather than a set of outcome routes. The strongest runnable proof, the typed data processor, is not promoted.
4. **Install v0.5 — blocked publicly.** The stable installer and Getting Started page still describe v0.4. The development path requires cloning and building the repository plus provisioning a complete release envelope.
5. **Create and run a starter — broken as documented.** A fresh `incan new hello --yes` project cannot immediately run, test, or release-build under the current v0.5 toolchain. The documented sequence omits the required `incan oven bake --project .` step.
6. **Build a representative project — healthy after setup.** The typed data processor tutorial passed end to end and produced a correct JSON report. It is a strong campaign flagship once installation and initial baking are made explicit or automatic.
7. **Understand limits — healthy.** Migration notes, generated-Rust boundaries, Oven's Alpha envelope, and the complete fix inventory are unusually honest. Keep them, but place them after the product story and test-drive invitation.

## Fresh-environment evidence

### Starter project

The documented sequence failed at `incan run`, `incan test`, and `incan build --release` because no receipt-compatible project Loaf existed.

After adding the missing explicit preparation step:

```console
incan oven bake --project .
incan run
incan test
incan build --release
```

the fresh project succeeded. Timings on this machine were approximately:

- project bake: 30.16 seconds;
- run: 0.26 seconds;
- tests: 0.62 seconds;
- warm release build: 0.05 seconds.

### Typed data processor

The representative tutorial succeeded unchanged after provisioning the isolated v0.5 toolchain:

- project bake: 44.37 seconds;
- two tests: 0.66 seconds;
- run: 0.37 seconds;
- result: two accepted orders, one rejected order, and the expected JSON report.

This is the most persuasive current proof that the product story is real.

### Existing development environment

An older prewarmed environment failed because its envelope manifest lacked the current `role` field. Re-running the documented prewarm target did not repair that stale state. Upgrade/recovery behavior needs a clean, supported answer before a campaign exposes more users to pre-release toolchain state.

## Recommended narrative

### Hero

**Incan 0.5**

**Readable source. Real native systems.**

Build applications, libraries, workspaces, typed data tools, and native integrations with Python-shaped source and Rust-backed artifacts—without writing Rust for the ordinary application path.

Primary action: **Build the typed data processor**  
Secondary action: **Install Incan 0.5**  
Tertiary action: **See every change**

The hero should show a small source-to-result proof: readable Incan source, passing tests, and the produced native or JSON artifact. Incus can support the composition, but Oven should not be the hero image for this release.

### Story order

1. **Start with what you can build now.** Four outcome cards: typed data processor, native API/service, library and workspace, native ecosystem bridge.
2. **What changed from 0.4.** Explain the transition from an installable evaluator toolchain to a coherent native application platform.
3. **Build real programs.** Lead with explicit failure, fallible iteration, environment access, crash-safe file publication, hashing, collections, web extractors, and other operational language/stdlib surfaces.
4. **Compose real systems.** Explain libraries, packages, workspaces, nested public namespaces, private fields, features, components, registries, and portable locks in user terms.
5. **Cross native boundaries safely.** Present the typed C ABI and strengthened Rust interop as checked integration capabilities, with precise experimental boundaries.
6. **One compiler view.** Use compiler coherence, semantic identity, inspection, and HIR work as credibility underneath the product outcomes rather than as the opening premise.
7. **Oven Alpha: an early look at accountable builds.** Keep one restrained card linking to the full Oven page and explicitly reserve the stronger product narrative for v0.6.
8. **Reliability ledger.** Summarize selected user-visible correctness improvements and optionally state the verified count of tracked bug issues closed in the milestone. Do not turn the long fix list into the main story.
9. **Upgrade and current boundaries.** Preserve the current migration warning, MSRV, generated-Rust boundary, C ABI exclusions, and Alpha limits.
10. **Complete inventory.** Retain the collapsed issue-by-issue record for traceability.
11. **Choose your test drive.** End with the same four outcome routes and a direct trouble-reporting link.

## Campaign readiness gates

### P0 — before paid traffic

- Publish the actual v0.5 release artifacts and make the installer/version selector visibly resolve them.
- Make one canonical v0.5 installation source authoritative across release notes, Getting Started, and audience routes.
- Make the scaffolded first-contact loop executable as written. Either prepare the project Loaf automatically where the product contract permits it, or teach the explicit bake before `run`, `test`, and `build`.
- Update Getting Started metadata and prose from v0.4 to v0.5 and verify it on clean supported hosts.
- Resolve or explicitly gate the ordinary-language correctness defect #1057 before launch.
- Resolve #1053 before promoting the producer/consumer library route, and avoid leaning on the method-alias trait story while #1055 remains open.
- Verify install, scaffold, bake, run, test, and release build from clean macOS and Linux environments using only public artifacts.

### P1 — release-page rewrite

- Replace the five equal summary cards with one value proposition and concrete test-drive routes.
- Move migration and compiler internals below the outcome story.
- Promote the typed data processor with its actual output.
- Reduce the first narrative layer to three user-facing pillars.
- Keep Oven to a late, clearly Alpha supporting section.

### P2 — campaign funnel

- Give campaign links a dedicated route or anchor that lands on the v0.5 value proposition and verified test drive.
- Track at minimum: release-page to install click, install to tutorial click, tutorial completion proxy, and trouble-report clicks.
- Add an obvious `Ran into trouble?` path that captures host, toolchain version, failed command, and diagnostic output.

## Acceptance criteria

- A new visitor can say what v0.5 enables without knowing what Oven, HIR, a provider plan, or a Loaf is.
- A visitor can reach a representative project in one click and the canonical install path in one click.
- A fresh user can follow one command sequence from public installation to successful native output without an undocumented step.
- Every campaign-promoted tutorial declares and passes against the published v0.5 toolchain.
- Oven is clearly useful and clearly Alpha, but it does not eclipse the packages, workspaces, language, standard-library, and native-boundary story.
- Migration and known-boundary claims remain visible before a user upgrades, without interrupting the acquisition narrative.

## Evidence files

- `01-release-top.png` — current first viewport and summary hierarchy.
- `02-release-story.png` — current dense compiler/package narrative.
- `03-release-boundary.png` — boundaries and complete-inventory handoff.
- `04-getting-started.png` — release CTA landing on the v0.4 verified tutorial.
