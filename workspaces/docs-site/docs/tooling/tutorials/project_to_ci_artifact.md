# Take a project to a CI artifact

Turn the project from [Getting started](getting_started.md) into a repeatable check-and-build job.

<aside class="inc-tutorial-meta" aria-label="Tutorial details"><dl>
<div><dt>Reader</dt><dd>Project author configuring CI</dd></div>
<div><dt>Prerequisites</dt><dd>A compatible runner with the matching dev.8 toolchain installed</dd></div>
<div><dt>Time</dt><dd>15 minutes</dd></div>
<div><dt>Verified</dt><dd>Incan <code>0.6.0-dev.8</code></dd></div>
<div><dt>Status</dt><dd>Development-toolchain executable</dd></div>
<div><dt>Outcome</dt><dd>A checked project and retained native executable</dd></div>
<div><dt>Artifacts</dt><dd>Diagnostic JSON and debug binary</dd></div>
</dl></aside>

## Check the source

From the starter project:

```bash
incan --version
incan fmt --check src tests
incan check src/main.incn --format json
incan test tests/test_main.incn
```

Keep the status of each command. Tests execute on every invocation.

## Build and retain the executable

```bash
incan build src/main.incn
find target/incan/debug/bin -type f
```

Retain the executable under `target/incan/debug/bin/` with your CI system's artifact upload facility. A release-profile build requires release-profile standard packages; use `--profile release` only with a matching installation.

## Use the same commands on a runner

Install the archive for the runner's host with the matching Oven executable, and set `INCAN_HOME` and `INCAN_STDLIB` as in [Install and run](../how-to/install_and_run.md). Run the commands above from the checked-out project. The source checkout alone does not contain a prepared compiler or standard-package artifacts.

<section class="inc-learning-panel inc-learning-panel--complete" data-label="Complete" markdown="1">

You have a local CI gate and a native artifact path. The runner uses the same installed compiler and explicit source commands.

</section>

## Continue

- [CI and automation](../how-to/ci_and_automation.md)
- [Diagnose a failed build](diagnose_failed_build.md)
