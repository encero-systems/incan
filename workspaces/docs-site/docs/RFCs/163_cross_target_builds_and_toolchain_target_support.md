# RFC 163: Cross-target builds and toolchain target support

- **Status:** Draft
- **Created:** 2026-10-25
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 020 (offline, locked, and reproducible builds)
    - RFC 116 (typed C ABI interop)
    - RFC 117 (`loaf.toml` and Oven's language-neutral project model)
    - RFC 118 (Incan and Oven command-line surfaces)
    - RFC 119 (Oven-native Rust build facets and Cargo interoperation)
    - RFC 124 (Oven store unit identity and cross-plan artifact sharing)
    - RFC 125 (`incan.pub` Loaf registry and baked asset distribution)
    - RFC 159 (Cloves and the managed C toolchain)
    - RFC 160 (vocabulary in Incan and native desugarers)
- **Issue:** —
- **RFC PR:** —
- **Written against:** v0.6 (in development)
- **Shipped in:** —

## Summary

Incan compiles for the machine it runs on. RFC 117 already lets a Loaf declare the targets it supports, RFC 118 gives `incan` and `oven` a `--target <triple>` flag, and RFC 119 requires a plan to report its build host and its target separately, but no contract says what an installed toolchain must hold to compile for a target other than its host, or how compilation, conditional compilation, linking, artifacts and execution behave across that split. This RFC supplies that contract. A toolchain release supports a declared set of targets, and support for one target is an installable component. Every unit is compiled either for the host, when it runs during the build, or for the target, when it ships. Conditional compilation, including a new target predicate for Incan's `when` blocks, evaluates against the domain a unit is compiled for. A link that needs platform files the host does not hold is refused before any work starts, and a cross-built program is built but never run unless the target executes on the host.

## Core model

1. **The target is a plan input.** Every plan has a build host and a platform target, selected as RFC 117 and RFC 118 define. When nothing selects one, the target is the host.
2. **Target support is a toolchain component.** For one toolchain release and one target, it consists of the Rust standard library for that target, the Rust ABI facts derived from it, the standard Loaves' compiled units for that target, and the identity of the platform files a final link for that target needs. A toolchain release declares the targets it supports; an installation holds support for its host and for any targets added to it.
3. **Every unit has a domain.** A unit that runs during the build (a procedural macro, a build-time tool, a vocabulary desugarer under RFC 160) is compiled for the host. A unit linked into the deliverable is compiled for the target. When the target is the host, the two domains coincide but stay distinct in identity, as RFC 124 already requires.
4. **Conditional compilation follows the domain.** Rust `cfg` predicates and Incan `when` conditions evaluate against the configuration of the domain the unit is compiled for, never against the build host when the unit is target-domain.
5. **Linking needs platform files.** A final link for a target uses that target's platform files under RFC 159: a published sysroot for Linux targets, a licensed SDK for Apple and Windows targets. A plan whose link needs platform files the build host does not hold is refused before execution. Units that do not link, such as libraries and Loaf units, may still be compiled.
6. **Artifacts follow the target.** Executable and library file names, the form of debug information, and the output location are those of the target, not of the build host.
7. **Execution needs a compatible host.** `run` and `test` execute a program only when the build host can run the target's executables. Otherwise the program is built and the plan reports that it was not run.
8. **The compiler is a host artifact.** The toolchain's own compiler, Oven and language server are built for and installed on a host; they are not outputs of a user's cross build.

## Motivation

A project that ships to Linux servers from a macOS laptop, or to Windows desktops from a Linux CI runner, has to build on each target machine today. The project model already anticipates more: RFC 117 has `[targets.<name>]` blocks with literal Rust target triples, RFC 118 lists `oven build --target aarch64-apple-ios`, RFC 119 says "cross compilation selects a build host and target separately", and RFC 124 and RFC 125 put the target triple and the domain into every unit identity and every baked asset. None of them says where a target's standard library, ABI facts and standard Loaf units come from, which units compile for which domain, how Incan source can tell a Windows target from a Unix one, or what happens when a link needs an SDK the machine is not licensed for. An implementation has to guess, and today it simply takes the host from the Rust compiler and never passes a target.

Incan's own toolchain shows the cost. Producing a Linux toolchain from a macOS machine, or a Windows toolchain at all, currently requires building on that platform from an earlier toolchain of the same platform, even though almost everything in a toolchain is an ordinary target artifact that a cross build could produce.

Incan source also has no way to express platform differences. `when feature("name"):` conditions declarations on package features only, so a standard-library module that must use one file-locking or symbolic-link primitive on Unix and another on Windows cannot be written in Incan, and has to move into a Rust facet or stay Unix-only.

## Goals

- Define what an installed toolchain holds to compile for a target other than its host, and how that support is installed and identified.
- Define which units compile for the host and which for the target, so build-time code runs on the build host and shipped code runs on the target.
- Make Rust `cfg` predicates and Incan conditional compilation evaluate against the right domain, and add a target predicate to Incan's `when` conditions.
- Refuse, before execution, a link whose platform files are not available on the build host, and name what is missing.
- Name and place artifacts by the target's conventions.
- Keep a target-domain unit interchangeable across build hosts, so a unit cross-built on one host and the same unit built on the target are one RFC 124 unit when their identity inputs match.

## Non-Goals

- Running cross-built programs through emulators, simulators, remote devices or user-declared runners. Execution on an incompatible host is refused; a runner contract is future work.
- Cross-building the toolchain's compiler. The compiler links the Rust compiler's own libraries, which Rust publishes per host; it is built on the host it runs on.
- Defining new carriers or delivery policies. RFC 117 owns targets, carriers and delivery policy; this RFC uses them.
- Publishing Apple or Windows SDKs. RFC 159's rule stands: those platform files are used where licensed and recorded by identity, never distributed.
- Choosing which targets a toolchain release supports. That is release policy.

## Guide-level explanation

### Adding a target

A toolchain installation compiles for its host out of the box. To compile for another target, add support for it:

```text
incan toolchain target add x86_64-unknown-linux-gnu
incan toolchain target list
```

`target add` installs the target's component of the active toolchain release: its Rust standard library, its ABI facts and the standard Loaves compiled for it. `target list` shows the installed targets, the host, and which installed targets can link on this host.

### Building for a target

Select the target on the command line, as RFC 118 defines:

```text
incan build --target x86_64-unknown-linux-gnu
oven build --target x86_64-unknown-linux-gnu
```

Output for a target other than the host goes under a directory named by the target triple, so host and cross builds of one project never overwrite each other. The executable is named the way the target names executables: `server` for Linux, `server.exe` for Windows.

A Loaf that declares targets under RFC 117 limits what may be selected:

```toml
[targets.linux]
platforms = ["x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu"]
carrier = "executable"
```

Selecting a triple the Loaf does not list is refused with a diagnostic naming the Loaf and its supported triples.

### Code that differs by platform

Incan's `when` blocks gain a target predicate:

```incan
when target(family="unix"):
    from rust::std::os::unix::fs import symlink

    def link_file(source: Path, link: Path) -> Result[None, str]:
        return symlink(source, link).map_err((error) => error.to_string())

when target(os="windows"):
    from rust::std::os::windows::fs import symlink_file

    def link_file(source: Path, link: Path) -> Result[None, str]:
        return symlink_file(source, link).map_err((error) => error.to_string())
```

The condition is evaluated when the module is compiled, for the target it is compiled for. Only the matching block's declarations and imports exist for that compilation. A build for a target that matches neither block refuses any use of `link_file` with an ordinary unresolved-name diagnostic.

### What runs where

A procedural macro used by a dependency, or a vocabulary desugarer, runs during the build, so Oven compiles it for the build host even in a cross build. Everything that ends up in the program is compiled for the target. `oven plan` shows both domains and every unit in each.

### Running and testing

`incan run --target <triple>` and `incan test --target <triple>` build for the target. They execute the result only when this machine can run that target's programs, such as an `x86_64` Linux target on an `x86_64` Linux host. Otherwise they build, report that execution was skipped and why, and exit with a distinct status so a script cannot mistake a build for a passing test.

### When a link is not possible here

Linking a Windows executable needs the Windows SDK and the MSVC runtime libraries, which may not be redistributed. On a macOS host without them:

```text
error: cannot link for x86_64-pc-windows-msvc on this host
  the link needs the Windows SDK and MSVC runtime libraries (RFC 159 platform files)
  libraries and Loaf units can still be built: oven build --target x86_64-pc-windows-msvc --no-link
```

## Reference-level explanation

### Target selection

- A plan's target must be selected as RFC 117 defines: an explicit invocation first, then workspace delivery policy, then a deterministic host default. When nothing selects a target, the target must be the build host.
- A selected target must be a Rust target triple that the active toolchain release declares as supported. Any other triple must be refused before planning, with a diagnostic naming the triple and the release's supported targets.
- A selected target whose support is not installed must be refused before planning, with a diagnostic naming the command that installs it.
- A selected target that a selected Loaf's `[targets.*]` declarations exclude must be refused before planning, with a diagnostic naming the Loaf and the triples it supports.

### Target support

- A toolchain release must declare the targets it supports. Each supported target's support component must contain:
    - the Rust standard library for the target, matching the release's pinned Rust compiler;
    - the Rust ABI facts the compiler derives from that standard library;
    - the standard Loaves' compiled units for the target, in every profile the release ships;
    - the identity of the platform files a final link for the target needs, under RFC 159.
- A support component must be identified by the toolchain release identity and the target triple. Installing it must verify every part against that identity.
- An installation's host support must be present whenever the toolchain is installed. Other targets' support must be installed explicitly; a build must not download a support component implicitly.
- A support component must not contain licensed platform files that RFC 159 forbids redistributing.

### Domains

- Every unit in a plan must be assigned exactly one domain, host or target.
- These units must be compiled for the host domain: Rust procedural macros, declared build-time tools under RFC 119, vocabulary desugarer units under RFC 160, and any unit only such units depend on.
- Every unit linked into a selected carrier must be compiled for the target domain.
- A unit needed in both domains must be compiled once per domain; the two compilations are distinct RFC 124 units.
- A host-domain unit's identity must name the build host's triple as its target and the host domain. A target-domain unit's identity must name the selected target and the target domain, and must not depend on which host compiled it, beyond the toolchain identity RFC 124 already includes.

### Conditional compilation

- Rust `cfg` predicates in Rust facets and in RFC 119 predicates must be evaluated against the configuration of the unit's domain: the target's configuration for target-domain units, the build host's for host-domain units.
- Incan `when` conditions must accept a target predicate, `target(key="value", ...)`, alongside `feature("name")`. The keys are `os`, `family`, `arch`, `env`, `vendor`, `endian` and `pointer_width`, with the values Rust's target configuration reports for those keys. A predicate holds when every named key has the named value. Unknown keys must be refused.
- A target predicate must be evaluated against the domain the module is compiled for, before semantic facts become visible, exactly as feature predicates are. It combines with `and` and nests like a feature predicate.
- A module compiled in both domains must be checked once per domain; each compilation sees only the declarations its conditions admit.
- The formatter must give equivalent target predicates one canonical spelling, as it does for feature predicates.

### Linking and platform files

- A final link for a target must use that target's platform files under RFC 159 and the linker the toolchain release designates for the target.
- When a plan's links need platform files the build host does not hold, Oven must refuse the plan before execution, naming the target and the missing platform files, unless the invocation selects compile-only output. A compile-only plan must produce every unit that does not link.
- `--no-link` on `incan build` and `oven build` must select compile-only output. It must be accepted for any target, including the host.
- Platform files must be recorded by identity in the receipt of every unit and artifact they reach.

### Artifacts

- Executable, static library and dynamic library names, and the debug-information artifacts beside them, must follow the target's conventions, not the build host's.
- Outputs for a target other than the build host must be placed under a directory named by the target triple. Outputs for the host target keep their present location.
- Receipts must record the build host, the target, and the domain of every unit, as RFC 119 requires.

### Execution

- `run` and `test` must execute a built program only when the build host can execute the target's programs. The toolchain must decide this from the two triples, not by trying to execute.
- When execution is not possible, the command must build, report that execution was skipped and why, and exit with a status distinct from both success and test failure.

### Toolchain hosts

- A toolchain release must declare its hosts separately from its targets. A host's installation includes the compiler, Oven and the language server built for that host, and the host's target support.
- The compiler must be built on the host it runs on. Every other part of a host installation may be produced by a cross build.

## Design details

### Syntax

`target(...)` is a new predicate inside the existing `when` form; no new statement is introduced. Keyword arguments name configuration keys rather than taking one Rust `cfg` string, so the formatter can sort and deduplicate them and diagnostics can name the key that did not match. `family="unix"` and `family="windows"` cover the common split; `os` distinguishes `macos`, `linux` and `windows` when the family is not enough.

### Semantics

A `when target(...)` block behaves like a `when feature(...)` block whose truth comes from the domain's target configuration instead of the package's feature set. Declarations in a block whose condition is false do not exist for that compilation: they are not checked, not visible to other modules, and not part of the unit's public surface. A module's public surface can therefore differ by target; RFC 124's external identity already includes the target, so consumers compiled for one target never see another target's surface.

### Interaction with RFC 117 and RFC 118

This RFC adds no manifest field. `[targets.<name>]` keeps its meaning as the supported-target declaration, and `--target` keeps its RFC 118 spelling and precedence. `incan toolchain target add` and `incan toolchain target list` extend RFC 118's toolchain command family.

### Interaction with RFC 119 and RFC 124

RFC 119 already separates host and target and requires the plan to report both; this RFC fixes which units go to which domain. RFC 124 already puts the target triple and the domain into unit identity; this RFC adds that the build host, beyond the toolchain identity, must not enter a target-domain unit's identity, so a unit cross-built on one host and built natively on another can be the same unit.

### Interaction with RFC 125

Baked assets already record the target triple and domains. Target support components are the toolchain's counterpart: `incan.pub` can publish them per toolchain release and target, so `target add` is a download and a verification, not a build.

### Interaction with RFC 159

RFC 159 defines the platform files and their identities; this RFC defines when a link needs them and what happens when the host lacks them. A Linux target links anywhere the published sysroot is installed. Apple and Windows targets link only on hosts holding the licensed SDK, which matches RFC 159's registry-builder rule.

### Interaction with RFC 160

Vocabulary desugarers run during the build, so they are host-domain units. A cross build for Windows on a macOS host runs macOS desugarer workers; the desugared output is target-independent source and is checked for the target.

### Compatibility / migration

Builds that select no target are unchanged: the target is the host, and outputs keep their location. Existing `when feature(...)` blocks are unchanged. Code that today assumes it runs on the build host, such as reading the host's operating system at compile time, keeps working for host builds and must use a target predicate to be correct in a cross build.

## Alternatives considered

### Build on every target instead of cross-building

This is the current state. It needs a machine of every target to build anything for it, and for the toolchain it needs an earlier toolchain of the same platform. Cross builds remove both requirements for everything except the compiler itself.

### Accept Rust `cfg` strings in `when`

`when cfg('target_os = "windows"'):` would reuse Rust's predicate language directly. It puts a second expression language inside a string, gives the formatter nothing to normalize, and lets unrelated Rust configuration keys into Incan source. Keyword arguments over a closed key set are checkable and formattable; whether to also accept `any`/`not` combinators is left open below.

### Download target support on demand

Installing a target's support at first use would make `--target` work immediately, but it turns a build into a network operation and makes the installed state depend on what was built last. Explicit installation keeps builds offline and reproducible, as RFC 020 requires.

### Ship every target in every installation

Every installation would hold every supported target's standard library and standard Loaves. That multiplies the installation size by the number of targets for the many users who build only for their host.

## Drawbacks

- A toolchain release must build and publish one support component per target, multiplying release work and storage.
- Modules with target conditions are checked once per domain they are compiled in, which costs time in cross builds that need a module in both domains.
- A public surface that differs by target can surprise consumers who only build for one target; diagnostics must make the target of a missing declaration visible.
- Licensed platform files keep Apple and Windows links on licensed hosts, so a full cross build from Linux to Windows still needs a Windows-licensed machine for the final link.

## Layers affected

- **Parser / AST**: the `target(...)` predicate in `when` conditions.
- **Typechecker / Symbol resolution**: target predicates evaluated per domain before semantic facts become visible; a module checked once per domain it is compiled in.
- **IR Lowering**: none beyond compiling a unit for its domain's configuration.
- **Emission**: the selected target reaches the Rust compiler for target-domain units; host-domain units compile for the host.
- **Stdlib / Runtime (the `incan_std_<component>` facets)**: standard Loaves compiled and published per supported target; platform-specific standard modules written with target predicates.
- **Formatter**: canonical spelling of target predicates.
- **LSP / Tooling**: analysis for a selected target; diagnostics that name the target of a missing declaration; `incan toolchain target add` and `incan toolchain target list`; plans and receipts that show both domains.

## Unresolved questions

- Which target should the language server analyze a module for when its Loaf declares several targets: the host, the first declared target, or each declared target in turn?
- Should target predicates accept `any(...)` and `not(...)` combinators, or stay positive-only like feature predicates?
- Does a Loaf that declares no `[targets.*]` block support every target its toolchain supports, or only its build host?
- Should `--target` also accept the name of a `[targets.<name>]` block, building every triple it lists?
- What exit status should `run` and `test` use when execution is skipped on an incompatible host?

<!-- Rename this section to "Design Decisions" once all questions have been resolved.
     An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
