# RFC 067: `std.ci` — deterministic CI and automation scripting primitives

- **Status:** Rejected
- **Created:** 2026-04-16
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 015 (hatch-like tooling and project lifecycle CLI)
    - RFC 051 (`JsonValue` for `std.json`)
    - RFC 055 (`std.fs` path-centric filesystem APIs)
    - RFC 063 (`std.process` process spawning and command execution)
    - RFC 066 (`std.http` HTTP client surface)
    - RFC 103 (secret values and non-disclosure)
- **Issue:** <https://github.com/encero-systems/incan/issues/85>
- **RFC PR:** —
- **Written against:** v0.2
- **Shipped in:** —

## Summary

The proposal for a standard `std.ci` namespace was retired after a test drive against Incan's release qualification and Oven test scripts. General process and file operations belong in their standard modules. CI provider integrations belong in optional Loaves. The original proposal is retained in the [reviewed draft](https://github.com/encero-systems/incan/blob/51734ca37df6dd30eed528fe11b9288248aaac03/workspaces/docs-site/docs/RFCs/067_std_ci.md).

## Design Decisions

- RFC 063 owns the process contracts demonstrated by the two scripts: shared stdout/stderr file redirection, null stdin, launching a child for later polling, and deadline cleanup with typed failure reporting.
- Release qualification owns its step name, release evidence, and status policy. Oven owns admission by job count and memory reservation, and decides which tests must run alone.
- CI scripts can use ordinary `std.environ`, filesystem, JSON, HTTP, and process APIs. Runner-specific output encoding, annotations, authentication, and publishing conventions belong in external libraries or workflow configuration.
- A library for check findings could be tested across two project checks. Its inclusion in the standard library would require separate evidence and review.

## Evidence

The [release helper](https://github.com/encero-systems/incan/blob/ee29e866b8c76d76eabfb3b0eebc6c0aa5cb98aa/scripts/release_support.incn#L259-L293) runs a child to completion, redirects both streams to one log, and handles its deadline. The [Oven launcher](https://github.com/encero-systems/incan/blob/ee29e866b8c76d76eabfb3b0eebc6c0aa5cb98aa/scripts/oven_tests.incn#L312-L370) returns a live child while the runner schedules other tests. The [workflow](https://github.com/encero-systems/incan/blob/ee29e866b8c76d76eabfb3b0eebc6c0aa5cb98aa/.github/workflows/release.yml#L35-L57) supplies explicit arguments and handles provider-specific artifact upload. These sources motivated the RFC 063 requirements. They do not establish that the proposed `std.process` ports already run.
