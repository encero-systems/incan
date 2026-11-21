# RFC 179: Library facts as Crumb records

- **Status:** Draft
- **Created:** 2026-11-11
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 031 (library system, phase 1: its type manifest is what this RFC replaces)
    - RFC 082 (checked API documentation generation)
    - RFC 106 (declaration identity)
    - RFC 124 (the bakery: unit identity and shared artifacts)
    - RFC 125 (the Loaf registry and baked assets)
    - RFC 165 (typed embedded storage: Crumb's value rules)
    - RFC 168 (`std.primitives`)
- **Issue:** —
- **RFC PR:** —
- **Written against:** v0.6
- **Shipped in:** —

## Summary

A compiled Incan library carries its library facts as a set of Crumb records: one library record, one record for each declaration that consumers can reach, and separate documentation records. These records are exactly what a consumer's compiler reads. They replace RFC 031's JSON type manifest. A record names the other records it depends on by identity rather than embedding them, so a reader decodes the declaration it asked for and the records that declaration depends on, and nothing unrelated. Separate digests keep separate questions apart. RFC 124's payload digest verifies the stored bytes. A **record-content digest** verifies the records, their schemas and their full values. A **semantic facts digest** says whether compilation meaning changed, and it alone enters unit identity. Republishing unchanged code under a new version, or editing a docstring, therefore changes no consumer's build. Admitting a library to the store copies its records and converts nothing. COMIC, Crumb's text form, is how tools print library facts for people; a printed form is a view and never an input.

## Motivation

RFC 031 gave a library a JSON type manifest: one document holding every exported declaration a consumer's checker needs. That served one library in one build, and it no longer fits how Oven builds:

- **Whole-document reads.** A consumer that needs one function reads and parses the entire manifest. The standard library's manifests are megabytes, and every compile read all of them.
- **A second encoding at admission.** To read declarations one at a time, the toolchain converts each installed manifest into per-declaration records. So every fact exists twice, as JSON and as records, and conversion is a step that can disagree with its input.
- **No typed contract.** The manifest's shape is whatever the compiler's serializer writes. Nothing versions a record kind, gives two equal declarations equal hashes, or lets a reader refuse a record of another schema with a precise diagnostic.
- **One digest for everything.** A change to one declaration, a docstring, or only the version number changes the whole manifest's digest, so everything keyed on it invalidates together.

Crumb, the typed encoding in `std.primitives.crumb` (RFC 165 and RFC 168), already provides typed records with field identity, defined same-value rules and canonical and object hashes. Library facts should be Crumb records from the start.

## Goals

- A library record, one declaration record per reachable declaration, and separate documentation records, as Crumb records under RFC 165's value rules.
- Records are what the consumer's compiler reads: no conversion at admission and no second copy of the facts.
- Dependencies between records are references by identity, so a reader decodes what a declaration needs and nothing unrelated.
- Byte verification (RFC 124's payload digest), record-content verification, and a semantic facts digest for compilation identity, each with its own job.
- Deterministic bytes and digests: the same checked library produces the same records and the same digests.
- A closed, versioned set of record kinds, and refusal of an unknown kind or schema with a diagnostic.
- The standard library's Loaves ship the same records with the toolchain.
- People read library facts as COMIC through Oven.

## Non-Goals

- Changing `loaf.toml`, which stays TOML because people write it.
- Changing `oven.lock` or the registry's index lines (RFC 125).
- Defining Crumb's byte format or COMIC's syntax, which their specification owns.
- A JSON export of library facts for other tools; a later RFC can add one as a projection.

## Guide-level explanation

Building a library writes its facts with the compiled unit. Nothing about writing or importing a library changes:

```incan
# mylib/src/money.incn
pub model Money:
    cents: int

pub def price() -> Money:
    """The list price."""
    return Money(cents=1999)
```

A consumer that calls `price` reads the library record, `price`'s declaration record and the `Money` record that `price` refers to. It decodes nothing else from `mylib`, and it never reads `price`'s documentation record.

Some changes don't change what consumers compile against, and they don't rebuild them:

- Publishing the same code as version 1.0.1 instead of 1.0.0.
- Editing the docstring of `price`: that updates its documentation record only.

Whether a body change rebuilds consumers depends on whether they can see the body while compiling. If consumers only call `price` through its signature, changing its body changes no declaration record and rebuilds no consumer. If they can see the body, because the compiler inlines `price` into them or evaluates it while compiling them, its body is part of its facts. Then changing `1999` to `2499` rebuilds those consumers. Generic bodies are always seen, because consumers instantiate them. Changing a signature always rebuilds the consumers that read it.

To see what a library holds, ask Oven:

```text
oven inspect facts mylib
oven inspect facts mylib price
```

The first prints the library record and every declaration record in COMIC. The second prints `price`'s record and lists the identities of the records it refers to. The printed text is for reading. Editing it changes nothing, and Oven never reads it back.

## Reference-level explanation

### The record set

- A compiled Incan library unit must carry its library facts as a Crumb record set: exactly one **library record**, one **declaration record** per reachable declaration, and zero or more **documentation records**.
- A declaration is **reachable** when it is exported, or when an exported declaration's checking, lowering, layout or consumer-observable body depends on it. This includes private helpers that exported generic or inlinable bodies call, and private types in exported layouts. A reachable declaration that is not exported is marked so; consumers' name resolution never sees it.
- Every record is keyed by the pair **(kind, identity)**: its record kind and the identity of the declaration it describes (RFC 106), or the library's identity for the library record. A declaration's declaration record and its documentation record share the identity and differ by kind, so their keys never collide.
- Key order, used by both digests, sorts by the kind's registered name and then by the identity's bytes.
- Each declaration record must carry everything a consumer's checker and lowering need for that declaration that isn't another record's.
- A declaration record must include the body facts of every body a consumer can observe during compilation, following RFC 124's boundary. That covers bodies consumers instantiate, bodies the compiler inlines into consumers, including inlining it selects across libraries, and bodies consumers evaluate while compiling. The bodies those bodies reach are reachable declarations in their turn. A body no consumer can observe is excluded, so changing it changes no declaration record.
- Record kinds are a closed set: library, alias, partial, model, class, function, trait, enum, type alias, newtype, const, static, made-private type, soft-keyword activation, vocabulary export, contract metadata, Rust ABI, and documentation. Adding a kind is a change to this contract.
- The record models are the compiler's, versioned with the compiler. `std.primitives.crumb` supplies their encoding.

### References between records

- A record that depends on another declaration must refer to it by that declaration's identity, not embed a copy of its facts.
- A record's references must be complete: they name every record a consumer needs to check or lower against that declaration.
- A reader must be able to read one record by key, and then the records its references name, without decoding any record outside that closure.

### Documentation

- Docstrings and other documentation live in documentation records, keyed by `(documentation, identity)` with the identity of the declaration they document.
- Declaration records must not contain documentation. Editing documentation changes only documentation records.

### Values and schemas

- Records must follow RFC 165's value rules: the same value encodes to the same bytes, and each record has its canonical and object hash.
- The library record's version, and its record of the toolchain that checked it, are publication fields: they are declared `[canonical=false]` (RFC 165), so they don't enter the record's canonical hash.
- Each record must carry its model description hash. A reader must refuse a record whose kind or schema it does not know, with a diagnostic that names the record, its kind and both schema hashes.

### Digests

Three digests answer three questions. Two are defined here, and the third is RFC 124's.

- **Bytes: RFC 124's payload digest.** Stored and transferred bytes are verified by the payload digest RFC 124 already defines for unit payloads. RFC 165's object hash covers a typed value across encoding profiles, so a digest built from object hashes can't prove two encodings are the same bytes, and this RFC doesn't use it for that.
- **Record content: the record-content digest.** It covers every record of the set, documentation included: each record's key, model description hash and object hash, in key order. It verifies that a record set holds the intended records with their schemas and full values, whatever encoding profile carried them. Admission, the registry and repair check it beside the payload digest.
- **Compilation meaning: the semantic facts digest.** It decides compilation reuse, and only it enters unit identity (RFC 124). It covers each library and declaration record's key, model description hash and canonical hash, in key order. Documentation records and publication fields don't enter it.
- Because RFC 165's canonical hash excludes the model description hash, the semantic facts digest must bind each record's model description hash itself. Two records with equal values under different schemas therefore never share a semantic digest.
- Two bakes of the same checked library must produce the same semantic facts digest. Republishing it under another version or with changed documentation must produce the same semantic facts digest and a different record-content digest.

### Reading and admission

- A compiler must read library facts only through these records. A JSON type manifest is neither produced nor read.
- Admitting a library to the store must copy its record set as it is, verified by its payload digest and its record-content digest. Admission must not re-encode, convert or merge records.
- The standard library's Loaves must ship their record sets with the toolchain, admitted like any other library's.

### Printing

- `oven inspect facts LIBRARY` must print the library record and every declaration record, in COMIC.
- `oven inspect facts LIBRARY DECLARATION` must print that declaration's record and list the identities of the records it refers to.
- `--docs` must add the documentation records to either form.
- Printed COMIC is a view: no Oven or compiler operation may read it as library facts.

## Design details

### Interaction with existing features

- **RFC 031.** This replaces its type manifest. The generated native unit it describes is unchanged.
- **RFC 124.** Unit identity covers the semantic facts digest, so a consumer is keyed on the meaning it compiled against, not on versions or documentation.
- **RFC 125.** A baked asset carries its unit's record set, its payload digest and its record-content digest. The registry's index lines don't change.
- **RFC 082.** Documentation generation reads declaration records and documentation records together.

### Compatibility

Libraries are rebuilt for each toolchain, because unit identity covers the toolchain, so no library baked by an earlier toolchain is read through this contract. No reader of the JSON type manifest is kept.

## Alternatives considered

- **Keep the JSON manifest and convert it at admission.** That is today's arrangement: two encodings of every fact, a conversion step, and whole-document reads wherever conversion hasn't run.
- **Per-declaration JSON values.** These give per-record reads but keep an untyped schema, no defined same-value rule and no hashes a reader can check.
- **Embed each dependency's facts in the records that use it.** One read would answer more, but every shared type would be copied into every user, and a change to it would have to find every copy.
- **One digest for content and meaning.** Simpler, but a version bump or a docstring edit would then rebuild every consumer.
- **Object hashes as byte verification.** RFC 165 hashes typed values across encoding profiles, so equal object hashes don't prove equal bytes. RFC 124's payload digest already proves bytes.
- **A schema format other than Crumb.** Protocol Buffers or FlatBuffers would give typed records, but as a second typed encoding beside Crumb, with two value rules to keep in step.
- **Ship COMIC files.** Text is easier to read, but parsing it at every compile costs time, and a text copy beside binary records would be a second authority.

## Drawbacks

- Generic JSON tools such as `jq` no longer read library facts; `oven inspect facts` replaces them.
- The compiler's readers need Crumb decoding at the native boundary until they are written in Incan.
- The record kinds are a contract: changing a declaration kind's fields means a schema change and a refusal path, where the JSON manifest silently absorbed new fields.
- Reachable private declarations become visible in a library's facts to anyone who inspects them, though never to name resolution.

## Layers affected

- **Compiler:** the record models and their versions; writing records at bake time, with reachability, consumer-observable bodies, references, documentation records and the record-content and semantic facts digests; reading library facts by key and by reference; removing the type-manifest reader.
- **Oven:** admission by copy, verified by the payload and record-content digests; the standard Loaves' record sets in the toolchain; unit identity using the semantic facts digest; `oven inspect facts`.
- **`std.primitives.crumb`:** encoding and the value rules the records use.
- **Docs:** reference pages for library facts and `oven inspect facts`, and release notes.

## Design Decisions

- **Documentation records are separate**, linked by declaration identity. Editing a docstring refreshes documentation without changing compiled meaning, so it rebuilds no consumer.
- **Dependencies are references, not embedded facts.** A record names the records it depends on, and a reader follows those references.
- **Three digests, three jobs.** RFC 124's payload digest verifies bytes. The record-content digest verifies records, schemas and full values across encoding profiles. The semantic facts digest alone enters unit identity, and excludes publication fields and documentation.
- **Consumer-visible bodies follow RFC 124.** A body is part of the facts when consumers can observe it while compiling, whether they instantiate, inline or evaluate it.
- **Records are keyed by (kind, identity)**, so a declaration and its documentation never share a key.
