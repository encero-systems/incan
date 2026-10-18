# RFC 158: Incremental XML records over fragmented inputs

- **Status:** Draft
- **Created:** 2026-10-17
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 066 (bounded HTTP bodies)
    - RFC 094 (scoped resource lifetime)
    - RFC 103 (secret-safe diagnostics)
    - RFC 115 (fallible byte and item iteration)
    - RFC 131 (typed failures and aggregation)
    - RFC 142 (async consumers and failure policies)
    - RFC 146 (typed decoding boundary separation)
- **Issue:** —
- **RFC PR:** —
- **Written against:** v0.6
- **Shipped in:** —

## Summary

Propose std.xml incremental record parsing over one logical XML byte stream, including a document split across an ordered sequence of physical files. Parser and decoder state must survive arbitrary fragment boundaries. Consumers receive bounded complete records and may process bounded microbatches without retaining the document tree. Source/structure failures, record-validation failures, and callback failures remain separate policy boundaries, with honest progress and completion reporting.

## Core model

1. Physical fragments form one ordered logical byte stream, not a list of independently valid XML documents.
2. Incremental parsing retains syntax, namespace, encoding, and structural state across fragment boundaries.
3. A checked record selector identifies complete subtrees that can be yielded and released independently.
4. Memory limits constrain bytes and structural state, not just the number of records.
5. Completion means the logical document was structurally completed; stopping after a useful prefix is a distinct outcome.

## Motivation

A dataset may contain fifteen multi-gigabyte files whose first fragment has the document header and whose last fragment has the footer. Parsing every fragment separately is incorrect, and loading all fragments or a complete document tree is impractical. Searching for a closing-tag string is also insufficient when XML namespaces, comments, attributes, encodings, or nested same-name elements are involved.

## Goals

- Stream a single logical document across physical file boundaries.
- Yield complete selected records with bounded retained parser state.
- Integrate with existing fallible iteration and proposed async consumers.
- Preserve typed failures, safe locations, successful progress, and explicit interruption.
- Make record and batch byte limits configurable and inspectable.

## Non-Goals

- Treating each physical file as an independent XML document.
- Loading the entire document into memory or guaranteeing that a record is inherently small.
- Recovering arbitrarily after lost XML structural state.
- Designing typed XML model mapping or a new general task scheduler here.
- Redesigning std.json while specifying XML streaming.

## Guide-level explanation

Every std.xml name below is proposed. BinaryReader and explicit fallible for consumption are existing facilities. The application function is complete: its caller supplies an already opened reader and owns that reader's resource lifetime. The reader may represent one file, an ordered fragment stream, or another bounded byte source.

```incan
from std.io import BinaryReader
from std.xml import QName, XmlLimits, XmlStreamError, records


def count_people[R with BinaryReader](reader: R) -> Result[int, XmlStreamError]:
    limits = XmlLimits(
        chunk_bytes=64 * 1024,
        max_record_bytes=8 * 1024 * 1024,
        max_token_bytes=256 * 1024,
        max_context_bytes=1024 * 1024,
        max_depth=128,
    )
    source = records(
        reader,
        selector=QName(namespace="", local="person"),
        limits=limits,
    )
    mut count = 0
    for record in source?:
        # A complete bounded person subtree; no whole-document tree is retained.
        count += 1
    return Ok(count)
```

The proposed QName stores a namespace URI and local name, not a prefix-sensitive string. XmlLimits has the five shown integer fields, each required to be positive. Record bytes bound the retained record payload; token bytes bound a single tag, attribute, or text token; context bytes bound retained parser state such as namespace and structural context. Those limits do not authorize callers to retain unlimited yielded records. records returns a lazy fallible iterator of owned XmlRecord values with XmlStreamError failures; it does not start by parsing the entire input. XmlRecord's public navigation surface remains open because this test drive needs only complete-record delivery.

A proposed ordered fragment reader in std.io would implement BinaryReader and accept an explicit ordered list of paths. An empty read from one physical file advances to the next file; only the end of the final file is logical EOF. Its exact constructor and cleanup API are not selected here. The XML parser receives the resulting byte stream and does not reset at a file boundary. A multibyte encoded character, start tag, or record may cross that boundary.

To process microbatches, a consumer must constrain both record count and retained bytes. RFC 142 supplies the async consumption/failure-policy design; this RFC does not invent an unprovided save_batch callback. An application-defined callback must be shown with its own Result contract in the later batch test drive. Typed record validation is a separate mapping layer, not a reason to reopen structural parsing after an unrecoverable failure.

## Reference-level explanation

### Input, records, and limits

- Ordered fragment consumption must preserve every input byte and its order without synthesizing separators, headers, or footers.
- Encoding and parser state must persist across chunks and fragments. UTF-8 sequences must not be decoded independently per chunk.
- A physical boundary must not be interpreted as XML EOF. Final logical EOF must validate document completion.
- Record selection must use namespace-aware structural matching. The semantics of nested matching records and root/path selection must be explicit before acceptance.
- A yielded record must be structurally complete and independently owned or governed by an explicit checked borrow lifetime. The example proposes owned records.
- Retained parser state and record storage must obey explicit byte/depth limits. A giant attribute outside a selected record or growing namespace context must not evade those limits. Limit exhaustion must be a typed error, not uncontrolled allocation or silent truncation.
- A batch consumer must apply both count and retained-byte limits, including a policy for one record larger than its batch budget.
- The parser must not resolve external entities, access network resources, or expand unbounded entities implicitly. Any future supported external resolution needs explicit capability and resource limits.

### Failure and completion

- XmlStreamError must distinguish source I/O, malformed structure/encoding, incomplete document, and configured limit exhaustion, retaining original typed source errors.
- Record-domain validation errors must remain separate from failures that invalidate the stream's structural state.
- Ignoring a record-domain error must not imply permission to continue after an unrecoverable parser or source failure.
- Callback errors must follow an independently selected consumer policy, not a hard-coded always-stop rule.
- Cancellation, intentional early stop, source failure, and validated complete exhaustion must be distinguishable. Previously yielded successful records must not be reported as rolled back.
- Diagnostics must retain structured logical offsets and available physical fragment provenance. Rendered messages must not be the only location representation.
- Retained errors and diagnostic excerpts must obey RFC 103. Collect-all reporting must not require an unbounded in-memory list of all records or errors.
- Scoped reader finalization must preserve all failures under RFC 094; this RFC must not select a contradictory cleanup mechanism.

## Design details

The proposed synchronous foundation builds on BinaryReader's bounded byte contract and RFC 115 fallible iteration. An async source needs a corresponding bounded read contract and RFC 142 integration, without blocking the async executor. Shared fragment concatenation belongs to I/O because JSONL and other formats can also consume a logical byte stream; XML-specific structural completion belongs to std.xml.

The recommended safety boundary is bounded complete-record delivery with explicit selector and parser limits. Typed XML decoding can later reuse checked schemas and validation, but namespace/attribute/text mapping requires its own decisions. Resuming after a checkpoint is also not the same as restarting a parser at an arbitrary record byte offset: namespace and document context may be needed.

## Alternatives considered

- Parse each file separately: fails when headers, footers, tags, or encoded characters cross physical boundaries.
- Find closing tags line by line: does not implement XML structure or encoding correctly.
- Build a whole-document tree: defeats the bounded-memory requirement.
- Treat every failure as an ignorable item: can continue from invalid parser state and falsely report complete processing.

## Drawbacks

Parser state can grow independently of record count, so limits are essential. Owned records trade some copying for a simpler lifetime contract. Arbitrary error recovery and restart checkpoints need more information than a line number or filename. No throughput or multi-gigabyte memory claim is established by this design draft.

## Layers affected

- **Stdlib I/O:** ordered fragment streams and structured physical provenance.
- **Stdlib XML / interop:** incremental parsing, selectors, records, and limits.
- **Error / consumer integration:** typed source versus item failures and bounded reporting.
- **Documentation / diagnostics:** completion evidence and safe locations.

## Unresolved questions

- What ordered fragment-reader constructor, ownership, and finalization surface fits std.io and RFC 094?
- What selector expresses record path, namespace, and nested-match behavior without parsing a string query language?
- What navigation and retained representation does XmlRecord expose?
- What are the default byte, depth, entity, and batch limits, and which XML features are supported?
- How are header/footer metadata and validated completion exposed to consumers?
- How should error reporting stream to bounded external storage under CollectAll?
- Which async byte-read contract and backpressure rules complement the synchronous foundation?
- What checkpoint evidence is required for safe restart across fragments?

<!-- Rename this section to "Design Decisions" once all questions have been resolved.
     An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
