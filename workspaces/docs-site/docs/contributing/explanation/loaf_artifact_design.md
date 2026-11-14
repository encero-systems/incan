# Loaf artifacts and consumer indexing

This page holds the detailed design supporting [DD-0007](../../design_records/0007_loaf_artifacts.md), whose status is **Accepted**. It preserves the technical contracts relocated from that record; the decision was accepted on 2026-11-08, but acceptance does not establish implementation completion. The decision record retains the architectural choices and their rationale.

This is the artifact/index design supporting the accepted record. [DD-0013](../../design_records/0013_the_incan_build_engine.md) and its [build-engine architecture](build_engine_architecture.md) govern the later record-store migration; retaining the original artifact design here does not restore superseded engine behavior.

## Design contracts

### The artifact

`incan-rustc-driver --library` writes, for one Loaf and one profile:

| File | Contents |
|---|---|
| `<name>.incnlib` | the library manifest: public exports, vocabulary, soft keywords, contract metadata, the Rust ABI of its Rust facet |
| `<name>.incnbody` | the governed bodies: the executable representation of every body a caller instantiates (functions with type parameters) |
| `lib<name>.rlib` | the library unit: every other body, compiled once, with a crate-root export for each public name |
| `<name>.native.json` | the unit's native closure: the rlibs and search paths it links |
| `<name>.incngraph` | the Loaf's checked declarations, references, calls and imports, with span-free identities and Loaf-relative paths in a versioned indexed-postcard artifact |

### The index

One index per compilation records each reachable Loaf's key, artifact directory, manifest, and namespace: `pub::<key>` for ordinary dependencies and compiler-pinned `std.*` roots for standard packages. Every module path selects exactly one entry; types come from its manifest, governed bodies from `.incnbody`, and linkage from its rlib and native closure.

A checked source interface may publish declarations and governed bodies through this same index before native compilation, allowing the runtime and its standard trait declarations to be built from source without an earlier runtime. It grants no compiled callable or native layout and is never a substitute for a complete compiled Loaf in a release toolchain.

Validated manifest inventories and facts may be retained privately for an exact file generation and compiler; public artifact admission remains authoritative. Canonical lookups load complete manifests on first demand, including every global vocabulary, provider-operation, and native-union contributor; failed demands refuse analysis, and previous source footprints supply no negative lookup facts.

### Calling into a Loaf

A consumer names declarations without redeclaring them; every type and compiled function/method is exported at the crate root under its encoded canonical identity, with the crate name lowercased and non-ASCII-alphanumeric characters replaced by `_`, eliminating per-call identity records.

The checked, unambiguous nongeneric callable and nominal names at a package root also alias their canonical Loaf items for Rust consumers; these facade aliases add no layout or implementation. Explicit Rust item reexports retain their checked Rust targets, including target-owned type parameters.

Every public callable's `.incnbody` declaration supplies parameters, default computations, and type parameters for consumer call checking and omitted arguments; the driver verifies calls against rlib metadata. Compiled callables publish declarations only: their implementation dependencies stay in the rlib, while functions with type parameters and open trait defaults retain governed bodies.

Published unions retain checked semantic carriers, public native owners, canonical nominal payload identities, and physical variant order; consumers use that representation for construction, narrowing, and tests, refusing conflicting owners instead of inventing wrappers or import-dependent order.

Source enum carriers retain their checked owner across module and artifact boundaries; equal source names do not merge owners.

Each public model/class with type parameters has one native ADT preserving binders and authored field order; provider and consumer layouts alias its instantiations, with otherwise unused parameters retained by a private, validated zero-sized marker excluded from Debug output, which shows authored fields under the source name.

A callable union member owns its captured environment. Its checked signature and producer-owned callable object type cross the artifact boundary with the union representation; consumers preserve that ownership during injection, narrowing, invocation, and cloning.

A compiled callable retains its checked declaration frame even when its implementation cannot be published as a governed body. That frame authorizes calling the canonical rlib item, never compiling a consumer copy of its implementation.

Every package identity takes this one route. No Body IR record, lowering branch or driver path is specific to the standard packages.

### The standard library

The `std` namespace is a dedicated virtual namespace over ordinary Loaves, not a separate kind of library or compiler consumption path.

The standard packages are Loaves like any other. A project depends on the standard packages whose namespaces it imports, at the versions the compiler pins. Their artifacts come from the toolchain: built from `loaves/stdlib/*` in a checkout, and shipped in the local loaves.pub cache by an installation. No source module of the standard library is collected into a consumer.

### Runtime dependency selection

The runtime's public callable map identifies which declarations implement lowered operations. Its presence is not a request to load every runtime body. Planning selects the operations it actually needs, including deferred coroutine calls, and resolves their canonical declarations through the same Loaf index. If a required body is missing from the current context, planning returns those prerequisites separately from a completed plan or a semantic refusal. A retry must add a previously unrequested canonical declaration; an unchanged requirement is a compiler contract failure. Incomplete attempts publish neither native artifacts nor a final refusal report.

An empty library or a scalar identity function does not acquire the ordinary core Loaf merely because the runtime also exports fallible iterator helpers. Bytes iteration and hex encoding retain their selected runtime helpers. A selected helper whose checked declaration requires a core trait or iterator type still requires that provider. Native support artifacts remain a separate, explicit part of the compilation unit; this rule does not make those native foundations disappear. A missing executable provider diagnostic retains the requested canonical declaration and its requester chain, without attributing imported declaration offsets to the consumer's source file.
