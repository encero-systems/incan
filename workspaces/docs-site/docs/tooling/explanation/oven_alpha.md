# Oven

Oven is the Incan-authored `incan` command line in dev.8. It reads projects, describes units and invokes the installed native compiler.

<div class="inc-oven-route__grid">
<a href="../tutorials/getting_started/">Run a project</a>
<a href="../reference/cli_reference/">Look up commands</a>
</div>

<section class="inc-oven-capabilities" markdown="1">

## Source, packages and compilation

An authored project has Incan source and a `loaf.toml`. Oven resolves active dependencies and features into a unit description. The compiler checks that unit, lowers Body IR and produces native machine code through the pinned Rust compiler. No generated Rust project or Cargo invocation lies on this path.

</section>

<ol class="inc-oven-proof-story">
<li><span>Producer</span> A library publishes its public contract, retained bodies and native artifacts.</li>
<li><span>Artifact set</span> Checked sidecars stay with their native implementation.</li>
<li><span>Consumer</span> A declared dependency supplies the public namespace and implementation.</li>
</ol>

<section class="inc-oven-mechanic inc-oven-mechanic--commands" markdown="1">

## Normal commands

Run, check, test and build take explicit paths. Format takes files or trees. Standard packages are ordinary Loaves supplied by the installed toolchain. Path dependencies are prepared before their consumers.

Oven reuses unchanged executables and retains project preparation in resident owners. Test execution remains fresh. `OVEN_TRACE` explains reuse and rebuild decisions.

</section>

<section id="current-alpha-boundary" class="inc-oven-evidence__grid" markdown="1">

## Installed-toolchain boundary

A build requires the matching driver, local Rust sysroot and compiled dependency artifacts for its profile. A missing package is an error, not an automatic download. Toolchain packing and installation transfer the native closure between compatible hosts.

The 0.5 Oven Alpha CLI and SDK-component model are historical interfaces. Dev.8's accepted commands and options are specified in the [CLI reference](../reference/cli_reference.md).

</section>

For practical steps, see [Getting started](../tutorials/getting_started.md), [dependencies](../how-to/dependencies.md) and [testing](../how-to/testing.md).
