# Take a local project to a CI artifact

This tutorial prepares a working Incan project for reproducible artifact delivery. The local gate is executable with the prepared 0.5 release envelope; the hosted runner stops at source verification until public 0.5 packaging installs that same envelope.

<aside class="inc-tutorial-meta" aria-label="Tutorial details">
  <dl>
    <div><dt>Reader</dt><dd>Project author preparing CI</dd></div>
    <div><dt>Prerequisites</dt><dd>A passing manifest-backed Incan project</dd></div>
    <div><dt>Time</dt><dd>20–30 minutes, excluding a cold CI build</dd></div>
    <div><dt>Verified</dt><dd>Commands verified with Incan <code>&gt;=0.5.0-0,&lt;0.6.0</code>; hosted CI installation remains a 0.5 packaging preview</dd></div>
    <div><dt>Status</dt><dd>Source-verified delivery preview</dd></div>
    <div><dt>Outcome</dt><dd>A locked local artifact gate and an honest hosted source-check lane</dd></div>
    <div><dt>Artifacts</dt><dd><code>incan.lock</code>, local build report, and native binary</dd></div>
  </dl>
</aside>

<ol class="inc-step-rail" style="--inc-step-count: 4" aria-label="CI artifact tutorial steps">
  <li><strong>Lock</strong>Freeze dependency resolution</li>
  <li><strong>Gate</strong>Match the local CI commands</li>
  <li><strong>Report</strong>Discover the artifact</li>
  <li><strong>Handoff</strong>Separate shipped CI from packaging preview</li>
</ol>

## Step 1: create the lock authority

From the project root, resolve dependencies and commit the result with the manifest:

```bash
incan lock
git add incan.toml incan.lock
```

`incan.lock` is the reproducibility authority for `--locked` commands. Regenerate it intentionally after dependency changes; do not let CI silently resolve a different graph.

## Step 2: run the local gate

Run the same checks CI will enforce:

```bash
incan fmt --check .
incan test --locked
mkdir -p target
incan build src/main.incn --locked --report json --report-output target/build-report.json
```

Use `--frozen` only when the runner already has every required dependency cached. `--locked` prevents resolution drift while still allowing a clean runner to fetch the committed dependency graph.

## Step 3: read the compiler-owned artifact path

The build report records emitted artifacts as structured rows. Select the existing binary instead of assuming a generated Cargo directory:

```bash
jq -r '.artifacts[] | select(.kind == "binary" and .exists == true) | .path' \
  target/build-report.json
```

The report schema is the tooling contract. Generated Rust paths remain inspectable implementation output rather than a stable ABI.

## Step 4: add the hosted source-check lane

The repository composite action currently installs a compiler binary from the selected Incan source ref. It does not install the prepared 0.5 release Loaf envelope used by the local gate above. Keep the hosted lane within that smaller contract instead of publishing a workflow that is expected to fail at native execution.

Create `.github/workflows/incan.yml` and pin the action to the accepted Incan commit SHA:

```yaml
name: Incan

on:
  push:
    branches: [main]
  pull_request:

env:
  INCAN_NO_BANNER: 1

jobs:
  build:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v5
      - name: Install Incan
        uses: encero-systems/incan/.github/actions/install-incan@<accepted-commit-sha>
      - name: Verify source contracts
        run: |
          incan --version
          incan fmt --check .
          incan check src/main.incn --format json
```

Do not replace the placeholder with `@main`: a moving compiler ref is not a reproducible delivery authority. When public 0.5 packaging installs the finite release envelope, extend this lane with the same locked test/build/report commands already verified locally and retain the report-selected artifact. Until then, the local build report proves the artifact path while hosted CI proves source and formatting contracts.

<section class="inc-learning-panel inc-learning-panel--complete inc-incus-slot" data-label="Complete" data-incus-category="success" markdown="1">

You now have a release-envelope executable local gate, a compiler-owned artifact path, and a hosted lane that stays within the installer contract actually available today. The missing hosted envelope is visible instead of hidden behind a workflow that cannot succeed.

</section>

## Continue

- [CI and automation reference](../how-to/ci_and_automation.md)
- [Diagnose a failed build](diagnose_failed_build.md)
- [Build report contract](../reference/cli_reference.md#incan-build)
