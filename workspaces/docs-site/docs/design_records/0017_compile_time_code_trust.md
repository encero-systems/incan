---
id: DD-0017
title: Compile-time code trust
status: Accepted
type: design-decision
date: 2026-11-09
review_target: v0.6
sources:
  - RFC 117
  - RFC 119
  - RFC 160
  - Maintainer decision of 2026-11-07 on compile-time trust
  - https://github.com/encero-systems/incan/issues/2739
---

<!-- markdownlint-configure-file {"MD025": {"front_matter_title": ""}} -->

# DD-0017: Compile-time code trust

## Context

Procedural macros and vocabulary registration or desugaring execute code while preparing or checking another project. An explicit build, run, or test already authorizes the requested compilation route, but opening a project in an editor or eager preparation must not silently grant the same authority. The trust decision must also remain distinct from execution modes and from the capability and operating-system containment boundaries that govern what admitted code may do.

## Decision

1. **Unasked execution requires trust.** Outside an explicit build, run, or test request, Oven may execute compile-time code only when trust covers the selected Loaf. An explicit request keeps its existing behavior; trust is not another execution mode.
2. **Trust comes from outside the project.** User or organization configuration may trust a publisher, an exact Loaf, or an exact project path. A project manifest may neither grant trust nor weaken an existing policy. Publisher trust covers the publisher's Loaves; exact-Loaf trust is used where adoption policy requires it; project trust is the local-development fallback.
3. **Toolchain content is exempt by identity.** A Loaf shipped by the installed toolchain needs no separate entry only when its artifact is covered by that installation's manifest digest. A matching name or directory is insufficient.
4. **A refusal is actionable and code-free.** When trust is missing, preparation stops before the relevant compile-time code runs, names every uncovered publisher or Loaf, and offers the exact entry that can cover it. Existing checked interfaces and vocabulary descriptors may still support code-free analysis. Evidence records the authority that covered each admitted provider, or records that execution was not trusted.
5. **The release default is ordinary policy.** The release configuration starts with publisher `incan` trusted. That entry is listed, auditable, and removable through the same trust command as any other publisher entry; it is not a hidden bypass.
6. **Publisher names are provisional identity.** Until RFC 125 supplies signed registry identity claims, publisher coverage matches the selected publisher scope name. Trust does not convert that name into cryptographic identity.
7. **The v0.7 execution boundary remains separate.** Each admitted compile-time execution will receive one RFC 104 capability grant, and governed execution and operating-system containment will be derived from that grant. Identity trust never widens those controls.

## Consequences

Cloning and opening an untrusted project does not run its procedural macros, vocabulary registration, or desugarer workers. Editors can report unavailable expansions as informational state and offer a confirmation-gated user-policy change without editing the project. Toolchain exemptions survive relocation but not artifact substitution. Publisher entries reduce repeated prompts, while exact-Loaf and project entries preserve narrower choices.

Trust answers whose compile-time code may run unasked; capability grants, execution mode and containment answer what an admitted execution may do. Audits and receipts must keep those answers distinct.

## Non-goals

This record does not implement RFC 104 capability enforcement or RFC 119 operating-system containment, define signed publisher identity before RFC 125, change explicit build/run/test behavior, or make an unavailable custom grammar parseable before a trusted or previously prepared descriptor exists.

## Revisit condition

Revisit when RFC 125 signed publisher claims replace scope-name matching, or when v0.7 lands the per-execution grant and containment contract.

## Provenance

This record applies RFC 117's compile-time trust boundary to RFC 119 procedural macros and RFC 160 vocabulary code for the v0.6 work tracked by issue #2739. It records the maintainer's 2026-11-07 decisions and supersedes no RFC.

## References

- RFC 117: Loaf project model and compile-time trust
- RFC 119: Oven-native Rust build facets and procedural macros
- RFC 160: native vocabulary registration and desugaring
- Issue #2739: compile-time trust delivery
