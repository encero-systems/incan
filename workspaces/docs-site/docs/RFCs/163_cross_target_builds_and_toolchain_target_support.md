# RFC 163: Cross-target builds and toolchain target support

- **Status:** Planned
- **Created:** 2026-10-25
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 020 (offline, locked, and reproducible builds)
    - RFC 033 (runtime configuration contexts)
    - RFC 036 (declaration decorators)
    - RFC 114 (package features and compilation-unit feature conditions)
    - RFC 116 (typed C ABI interop)
    - RFC 117 (`loaf.toml` and Oven's language-neutral project model)
    - RFC 118 (Incan and Oven command-line surfaces)
    - RFC 119 (Oven-native Rust build facets and Cargo interoperation)
    - RFC 124 (Oven store unit identity and cross-plan artifact sharing)
    - RFC 125 (`incan.pub` Loaf registry and baked asset distribution)
    - RFC 148 (must-use values and deliberate discard)
    - RFC 159 (Cloves and the managed C toolchain)
    - RFC 160 (vocabulary in Incan and native desugarers)
- **Issue:** [#2480](https://github.com/encero-systems/incan/issues/2480)
- **RFC PR:** —
- **Written against:** v0.6 (in development)
- **Shipped in:** —

## Summary

Incan compiles for the machine it runs on. RFC 117 already lets a Loaf declare the targets it supports, RFC 118 gives `incan` and `oven` a `--target <triple>` flag, and RFC 119 requires a plan to report its build host and its target separately, but no contract says what an installed toolchain must hold to compile for a target other than its host, or how compilation, conditional compilation, linking, artifacts and execution behave across that split. This RFC supplies that contract. A toolchain release supports a declared set of targets, and support for one target is an installable component. Every unit is compiled either for the host, when it runs during the build, or for the target, when it ships. Conditional compilation uses `when condition:` for declaration or statement groups and `@when(condition)` for individual declarations, with a dedicated compile-time predicate namespace evaluated against the domain a unit is compiled for. A link that needs platform files the host does not hold is refused before any work starts, and a cross-built program is built but never run unless the target executes on the host.

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
- Make Rust `cfg` predicates and Incan conditional compilation evaluate against the right domain, and expose platform, architecture and feature conditions through Incan's compile-time `when` forms.
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

Selecting a triple the Loaf does not list is refused with a diagnostic naming the Loaf and its supported triples. If a Loaf declares no `[targets.*]` blocks, it adds no project-specific restriction: any target supported by the active toolchain may be selected, subject to installed support, dependency declarations, carrier compatibility and ordinary compile and link checks. Omission is not a claim that every such target has been tested.

`--target` accepts one literal triple, not a target-block name. With the manifest above, `oven build --target x86_64-unknown-linux-gnu` selects that triple; `oven build --target linux` is refused with a diagnostic showing the block's literal triples. Building a named group as a matrix requires a separate command contract.

### Code that differs by platform

The proposed `when` form is a compile-time conditional with a dedicated predicate namespace. The block form groups imports and declarations:

```incan
when unix:
    from rust::std::os::unix::fs import symlink

    def platform_label() -> str:
        return "Unix"

when windows:
    from rust::std::os::windows::fs import symlink_file

    def platform_label() -> str:
        return "Windows"
```

The Rust imports name the platform APIs directly; they exist only in the selected block. Each selected `platform_label` is a complete function. A use of `platform_label` on a target matching neither block receives an ordinary unresolved-name diagnostic naming the active target.

For one declaration, `@when(condition)` expresses the same selection:

```incan
@when(macos and aarch64)
def platform_label() -> str:
    return "macOS on ARM64"

@when(linux and x86_64)
def platform_label() -> str:
    return "Linux on x86-64"
```

These examples are separate modules illustrating alternatives, not declarations to append to the previous example. The compiler selects declarations before ordinary name resolution and type checking. Excluded code must still be syntactically valid.

Common contextual shorthands have explicit equivalents:

| Shorthand | Equivalent condition |
| --- | --- |
| `unix` | `target_family == "unix"` |
| `windows` | `target_family == "windows"` |
| `macos` | `target_os == "macos"` |
| `linux` | `target_os == "linux"` |
| `x86_64` | `target_arch == "x86_64"` |
| `aarch64` | `target_arch == "aarch64"` |
| `wasm32` | `target_arch == "wasm32"` |

A macOS target satisfies both `unix` and `macos`. Family and operating system are distinct facts. Architecture names identify exact architectures; `aarch64` does not mean 32-bit ARM.

Several package features can be enabled together, so feature conditions use membership:

```incan
@when(unix and "telemetry" in features)
def report_startup() -> None:
    println("Application started")
```

`features` is the enabled package feature set supplied by the build. `unix`, `target_family`, `target_arch` and `features` are contextual predicate names here, not application variables or imported functions. Conditions support `and`, `or`, `not` and parentheses; for example, `when unix and (x86_64 or aarch64):` selects either listed architecture in the Unix family.

### Inside models and classes

Compilation conditions can select fields and methods inside a model or class. This complete proposed model has a telemetry field only when that package feature is enabled, and a Unix-only method:

```incan
model WorkerSettings:
    workers: int = 4

    when "telemetry" in features:
        trace_enabled: bool = false

    @when(unix)
    def backend_name(self) -> str:
        return "unix"
```

Without telemetry, `trace_enabled` is absent; supplying it to the constructor or accessing it is a checked error. On other target families, `backend_name` is absent. Constructor parameters, reflection, schema, validation and serialization are derived from the selected members. They must not expose placeholder fields or methods for excluded source. Retained members still have to satisfy the type's ordinary composition and trait obligations.

### Inside functions and methods

In an executable body, `when` selects statements in the surrounding lexical scope. This independent example has one retained return for every target:

```incan
model WorkerSettings:
    workers: int = 4

    def describe(self) -> str:
        when unix:
            return f"Unix worker pool: {self.workers}"

        when not unix:
            return f"Other worker pool: {self.workers}"
```

For a Unix-family target, the method is checked as if its body contained only the first return. The selected statements remain at the position of their block, without a new runtime branch or an extra lexical scope. Ordinary binding, ownership, control-flow and return checks apply after selection. For example, omitting the second block would leave a missing-return diagnostic on other targets. Nesting compilation conditions requires all enclosing conditions to hold.

A method's application data does not become a compilation predicate:

```incan
# Rejected condition inside a method:
# when self.workers > 4:
#     ...
# Diagnostic: compilation conditions cannot read self or application values.
# Use if self.workers > 4: for this runtime decision.
```

RFC 033 uses `if env == "prod":` inside `ctx` for runtime configuration contributions. That condition reads ordinary typed fields during resolution; it does not select compiled declarations or acquire this RFC's predicate namespace. `when` and compiler-owned `@when` retain their compile-time meaning.

### Selection and declaration annotations

`@when` and RFC 148's `@must_use` are compiler-defined annotations written using decorator syntax. They are not callable wrappers under RFC 036 and do not add runtime wrapper calls. Their distinct compiler semantics compose as follows:

```incan
@when(unix)
@must_use(message="use the selected backend name")
def backend_name() -> str:
    return "unix"

when unix:
    def show_backend() -> None:
        backend_name()           # RFC 148 diagnostic: discarded expression.
        println(backend_name())  # Value passed to another function.
        name = backend_name()    # Binding is outside RFC 148's discard check.
```

On other targets, neither function exists. Selection happens before annotation semantics, derives or user-defined decorator application, regardless of annotation order. The predicate namespace belongs only to compilation `when` conditions; it does not change ordinary argument lookup for other annotations. RFC 148 continues to own must-use defaults, severity and deliberate-discard spelling.

### What runs where

A procedural macro used by a dependency, or a vocabulary desugarer, runs during the build, so Oven compiles it for the build host even in a cross build. Everything that ends up in the program is compiled for the target. `oven plan` shows both domains and every unit in each.

### Running and testing

`incan run --target <triple>` and `incan test --target <triple>` build for the target. They execute the result only when this machine can run that target's programs, such as an `x86_64` Linux target on an `x86_64` Linux host. Otherwise they build, report that execution was skipped and why, and exit with status `3` so a script cannot mistake a build for a passing test. A failed build keeps its ordinary failure result and takes precedence over an execution skip. A successful cross build with skipped execution has no program exit status and no passing-test result.

### Selecting an editor target

The language server analyzes one active target for the selected project: an explicit editor target first, then existing workspace target policy, then the host only if that project permits it. For example, on a macOS host a project that declares only the two Linux triples above cannot silently use macOS or pick the first Linux entry. It must request an active target selection. Selecting `aarch64-unknown-linux-gnu` gives diagnostics and completion for that target, including declarations retained by `when linux and aarch64:`. Checking other targets as a matrix is separate from this active editing context.

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
- A selected target that a selected Loaf's `[targets.*]` declarations exclude must be refused before planning, with a diagnostic naming the Loaf and the triples it supports. If no such blocks are declared, that Loaf must impose no additional target restriction; toolchain support, installed components, dependency target restrictions, carrier compatibility and ordinary compile and link checks must still apply. Absence must not be reported as proof of tested portability.
- `--target <triple>` must select exactly one literal triple. A `[targets.<name>]` group name must not be expanded into one or more triples; it must receive a diagnostic naming the literal alternatives. Declaration order must not choose an architecture or introduce implicit matrix execution.

### Editor analysis target

- The language server must select one active analysis target using an explicit editor selection, then existing workspace target policy, then the build host only when the selected project's target declarations permit it. Each selected target must satisfy ordinary target-support and project-selection checks.
- If no valid active target follows from those rules, tooling must diagnose the missing or incompatible selection and offer supported targets. It must not silently choose the first declaration, merge mutually exclusive target surfaces, or mark all targets checked.
- Diagnostics, completion, selected-member metadata and semantic inspection must identify the active target. An additional matrix check must retain separate target-specific results rather than changing the active editor context.

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
- A host-domain unit's identity must name the build host's triple as its target and the host domain. A target-domain unit's identity must name the selected target and the target domain. The build host alone must not distinguish otherwise identical portable target-domain units, but all actual compiler, provider, platform-file and generated-input identities required by RFC 124 must remain. Machine-local or host-sensitive inputs must retain their declared restrictions; selecting a target must not make them portable.

### Conditional compilation

- Rust `cfg` predicates in Rust facets and in RFC 119 predicates must be evaluated against the configuration of the unit's domain: the target's configuration for target-domain units, the build host's for host-domain units.
- Incan must accept `when condition:` at module declaration scope, in model and class member bodies, and within ordinary synchronous or asynchronous function and method statement bodies. The content must be legal in the surrounding context: member bodies may select fields and methods; executable bodies may select statements. `when` must not admit otherwise invalid forms. Compiler-owned `@when(condition)` must select an individual declaration, including a model, class or method declaration; it must not decorate arbitrary expressions or statements. Both forms must use the same compile-time condition grammar and evaluation rules. This does not introduce a runtime conditional or an ordinary decorator function.
- The condition grammar must accept target-property comparisons using `==`, membership of a feature name in `features`, the contextual shorthands listed above, Boolean composition with `and`, `or` and `not`, and parentheses. It must not evaluate arbitrary application expressions, attribute lookup or function calls. The existing `feature("name")` compatibility spelling described below is recognized grammar, not a callable expression.
- Explicit target properties are `target_os`, `target_family`, `target_arch`, `target_env`, `target_vendor`, `target_endian` and `target_pointer_width`. Their comparison values must follow the corresponding Rust target configuration. Family matching must preserve Rust's reported family memberships when a target belongs to more than one family. `features` must represent the unit's enabled package features.
- Predicate names must be resolved within the compiler's dedicated condition namespace. They require no import and must not be shadowed by application bindings. Outside compilation conditions these contextual spellings must not acquire special meaning from this RFC. Unknown predicate names and unsupported condition expressions must produce source-level diagnostics.
- Conditions must be evaluated against the domain the module is compiled for before ordinary name resolution, type checking, derives, declaration-metadata checks or user-defined decorator application. Excluded source must remain syntactically valid, but its declarations and imports must not contribute semantic facts or annotation obligations. This ordering must not depend on the written order of annotations.
- A retained block's contents must occupy its source position in the surrounding lexical scope; compilation selection must not introduce a runtime branch or an extra scope. Nested compilation conditions must require every enclosing condition to hold. Ordinary name binding, ownership, control-flow and return analysis must check the resulting body. A selection that leaves a required return path uncovered or a referenced name absent must produce the ordinary diagnostic for that selected configuration.
- Model and class construction, member lookup, reflection, schema, validation, serialization and derives must use only retained members. Excluded members must have no constructor argument, stored slot or published member metadata. Retained types must still satisfy ordinary trait, composition and validation contracts; selection must not waive an obligation.
- Compilation conditions inside a method must retain the dedicated predicate namespace. They must not read `self`, parameters, local values or runtime environment settings; a diagnostic for such use must explain that an ordinary `if` is needed. RFC 033 configuration-contribution `if` blocks retain their runtime-resolution semantics and ordinary typed field lookup.
- A retained declaration's annotations must keep their own contracts. In particular, RFC 148 must check discarded expressions against retained must-use declarations; selection must not change its severity, defaults, exemption rules or deliberate-discard contract.
- A module compiled in both domains must be checked once per domain; each compilation sees only the declarations its conditions admit. If retained definitions collide, ordinary duplicate-declaration diagnostics must apply; `when` must not choose a winner by source order.
- The formatter must preserve condition meaning and give both forms a consistent spelling. Explicit property comparisons and their documented shorthands must have identical selection behavior.

### Linking and platform files

- A final link for a target must use that target's platform files under RFC 159 and the linker the toolchain release designates for the target.
- When a selected carrier's final link needs platform files the build host does not hold, Oven must refuse that link before execution, naming the target and the missing platform files, unless the invocation selects compile-only output.
- `--no-link` on `incan build` and `oven build` must omit the selected carrier's final link and must be accepted for any target, including the host. It must not disable links required to produce executable host tools, procedural macros or vocabulary desugarers used by the build, unless a usable admitted prebuilt artifact already supplies them.
- A compile-only plan must retain every input needed by the units it actually compiles, including C headers and SDK compile inputs. Missing such inputs or required host-tool links must remain diagnosed before execution. An incomplete output must not be reported as a completed executable.
- Platform files must be recorded by identity in the receipt of every unit and artifact they reach.

### Artifacts

- Executable, static library and dynamic library names, and the debug-information artifacts beside them, must follow the target's conventions, not the build host's.
- Outputs for a target other than the build host must be placed under a directory named by the target triple. Outputs for the host target keep their present location.
- Receipts must record the build host, the target, and the domain of every unit, as RFC 119 requires.

### Execution

- `run` and `test` must execute a built program only when the build host can execute the target's programs. The toolchain must use its declared host/target execution-compatibility facts together with the required loader, ABI and runtime availability. Matching architecture or comparing triples alone must not be treated as proof that the executable can run. It must not try an arbitrary target binary to discover compatibility.
- If a build succeeds but execution is unavailable, the command must report execution as skipped, retain the host, target and reason, and exit with the dedicated non-success status `3`. This command-generated status is distinct from success (`0`) and the ordinary build or test failure status (`1`). It must not report a program exit status or count unexecuted tests as passing. Actual program exits retain their ordinary `run` behavior; structured reporting must distinguish them from a command-generated skip.
- A compile, link or other build failure must retain its ordinary failure result and diagnostics, taking precedence over the execution-skip result. Successful execution must report the actual program or test outcome. The build and execution outcomes must remain separately inspectable through the same Oven result and receipt that RFC 118's Incan aliases forward.

### Toolchain hosts

- A toolchain release must declare its hosts separately from its targets. A host's installation includes the compiler, Oven and the language server built for that host, and the host's target support.
- The compiler must be built on the host it runs on. Every other part of a host installation may be produced by a cross build.

## Design details

### Syntax

`when condition:` and `@when(condition)` share a bounded compile-time condition language. Compiler-provided target properties, enabled package features and documented shorthands live in its dedicated namespace. The syntax reads like a Boolean expression without introducing globally visible `TARGET` or `features` bindings or ordinary `target(...)` calls. The block selects declarations, members or executable statements according to its surrounding context; the annotation selects one declaration under RFC 036's compiler-owned annotation boundary.

### Semantics

The truth of a target condition comes from the unit's domain configuration, and feature membership comes from that unit's enabled package features. Declarations in an excluded group or under a false `@when` do not exist for that compilation: they are not checked, not visible to other modules, and not part of the unit's public surface. A module's public surface can therefore differ by target; RFC 124's external identity already includes the target, so consumers compiled for one target never see another target's surface.

### Interaction with RFC 033 and RFC 114

RFC 033 uses `if` within `ctx` declarations to contribute runtime configuration defaults, preserving ordinary typed field lookup. `when` and compiler-owned `@when` remain compile-time-only selectors with the dedicated predicate namespace. Runtime input must not be interpreted as a compilation predicate.

RFC 114 defines additive package-feature resolution and its existing positive, compilation-unit declaration conditions. This RFC extends the source condition grammar and the permitted locations; it does not add feature disabling, precedence or a separate feature resolver. Explicitly conditional model or class members make that type's member set configuration-dependent, a scoped extension to RFC 114's restriction on feature-dependent changes to an unconditional type. This must be visible in the selected constructor and published type metadata, with consumers checked against that same selected configuration. It must not silently rewrite a retained field's declared type, waive an unconditional trait obligation or relax provider verification of supported feature projections.

### Interaction with RFC 036 and RFC 148

Compilation selection precedes declaration annotation semantics. `@when` is a compiler-owned selector, not a callable wrapper under RFC 036. RFC 148's `@must_use` records an obligation only on a retained declaration or type; an excluded declaration has no must-use obligation. The predicate namespace is local to selection conditions, and does not turn other annotations' arguments into compilation predicates. This RFC settles composition and ordering, not RFC 148's remaining diagnostic-policy decisions.

### Interaction with RFC 117 and RFC 118

This RFC adds no manifest field. `[targets.<name>]` keeps its meaning as the supported-target declaration, and `--target` keeps its RFC 118 spelling and precedence. `incan toolchain target add` and `incan toolchain target list` extend RFC 118's toolchain command family.

### Interaction with RFC 119 and RFC 124

RFC 119 already separates host and target and requires the plan to report both; this RFC fixes which units go to which domain. RFC 124 already puts the target triple and the domain into unit identity. A portable target-domain unit cross-built on one host and built natively on another can be the same unit only when its actual identity inputs match. Host-sensitive inputs and machine-local restrictions remain part of that contract; cross-target selection must not erase them.

### Interaction with RFC 125

Baked assets already record the target triple and domains. Target support components are the toolchain's counterpart: `incan.pub` can publish them per toolchain release and target, so `target add` is a download and a verification, not a build.

### Interaction with RFC 159

RFC 159 defines the platform files and their identities; this RFC defines when a link needs them and what happens when the host lacks them. A Linux target links anywhere the published sysroot is installed. Apple and Windows targets link only on hosts holding the licensed SDK, which matches RFC 159's registry-builder rule.

### Interaction with RFC 160

Vocabulary desugarers run during the build, so they are host-domain units. A cross build for Windows on a macOS host runs macOS desugarer workers; the desugared output is target-independent source and is checked for the target.

### Compatibility / migration

Builds that select no target are unchanged: the target is the host, and outputs keep their location. Existing `when feature("name"):` blocks retain their feature-selection behavior as a compatibility spelling within the condition grammar; `"name" in features` is the preferred new spelling. The compatibility form is a compiler predicate, not ordinary function lookup, and must not permit arbitrary calls. Code that today assumes it runs on the build host, such as reading the host's operating system at compile time, keeps working for host builds and must use domain-aware target conditions to be correct in a cross build.

## Alternatives considered

### Build on every target instead of cross-building

This is the current state. It needs a machine of every target to build anything for it, and for the toolchain it needs an earlier toolchain of the same platform. Cross builds remove both requirements for everything except the compiler itself.

### Accept Rust `cfg` strings in `when`

`when cfg('target_os = "windows"'):` would reuse Rust's predicate language directly. It puts a second expression language inside a string, gives the formatter nothing to normalize, and lets unrelated Rust configuration keys into Incan source. A dedicated condition namespace with explicit properties and Boolean operators is checkable and formattable without embedding Rust grammar in a string.

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

- **Parser / AST**: the shared compilation condition grammar at module, type-member and executable-body positions, with contextual predicate names and compiler-owned declaration annotations; RFC 033 contribution bodies retain their separate grammar.
- **Typechecker / Symbol resolution**: conditions evaluated per domain before semantic facts or annotation obligations become visible; selected members determine construction and metadata; selected statement bodies receive ordinary scope, ownership and control-flow checks.
- **IR Lowering**: only selected declarations, members and statements may reach ordinary lowering; compilation `when` must not become a runtime branch.
- **Emission**: the selected target reaches the Rust compiler for target-domain units; host-domain units compile for the host.
- **Stdlib / Runtime (the `incan_std_<component>` facets)**: standard Loaves compiled and published per supported target; platform-specific standard modules written with target predicates.
- **Formatter**: canonical spelling of target predicates.
- **LSP / Tooling**: analysis for a selected target; diagnostics that name the target of a missing declaration; `incan toolchain target add` and `incan toolchain target list`; plans and receipts that show both domains.

## Design Decisions

- **Compile-time conditional with a dedicated namespace:** `when condition:` selects declaration, member or statement groups according to its surrounding context; `@when(condition)` selects one declaration. Both read compiler-provided domain facts and package features rather than application variables.
- **Readable conditions:** contextual platform and architecture shorthands have explicit `target_family`, `target_os` and `target_arch` equivalents. Feature selection uses membership in `features`. Boolean `and`, `or`, `not` and parentheses are supported; the condition language does not execute arbitrary application code.
- **Selection precedes annotations:** excluded source must parse but contributes no ordinary semantic facts. Selection runs before type checking, must-use checks, derives and user-defined decorator application, independent of annotation order. RFC 148 keeps ownership of must-use diagnostic policy.

- **Member and statement contexts:** compilation selection is valid within models, classes, functions and methods. Selected statements share their enclosing scope and receive ordinary control-flow checks; selected members determine the actual type contract and generated metadata. Runtime `ctx` contributions remain governed by RFC 033.

- **One active editor target:** explicit editor selection, then workspace target policy, then a permitted host fallback. Missing valid selection is diagnosed; declaration order and an implicit all-target analysis do not choose the editor surface.
- **Omitted target tables add no restriction:** an undeclared project target matrix permits selection from the active toolchain's supported targets, while installed support, dependency restrictions and ordinary compile/link checks remain required. This is not a tested-portability claim.
- **One literal triple per invocation:** `--target` does not expand a named target block or introduce implicit fan-out. A separately specified matrix operation may add that behavior later.
- **Skipped execution is unsuccessful:** a successful build with unavailable execution exits with status `3`, retains separate build/execution facts and contributes no passing-test result. Build failure takes precedence. Compatibility requires checked host/target and runtime prerequisites, not trial execution.
- **Compile-only preserves real prerequisites:** `--no-link` omits the selected carrier's final link; required host tools and target compile inputs remain necessary. RFC 124's portable and machine-local identity restrictions remain intact.
