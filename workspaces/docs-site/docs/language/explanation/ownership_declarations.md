# Why ownership declarations are introduced in stages

Explicit ownership declarations are being introduced in stages. The [annotation reference](../reference/explicit_ownership.md) lists recognized syntax and diagnostics; recognition does not enable executable ownership behavior. Existing unannotated functions retain their current semantics.

The parser preserves each result-source token for diagnostics and formatting. Static field paths are resolved through checked model and class declarations, including generic field substitution. Naming a valid path does not prove the returned value originates there: execution remains refused until ownership behavior is checked. An owned wrapper may retain a borrowed dependency, but recording that request does not prove the wrapper preserves its source.

## Preparing ordinary bindings for borrowing

A mutable local initialized directly from a stored string field currently receives an independent value. The migration warning identifies bindings whose meaning will change: a mutable declaration will become a write-through borrow; bare and `let` declarations will borrow read-only. Compatible annotations do not request a copy. Use an explicit clone when an independent snapshot is intended. The warning release must be available before a subsequent release changes ordinary binding semantics; shipping both changes together does not provide a migration interval.

## Metadata and tooling

Checked contracts retain parameter slots, access modes, underlying types and canonical stored-field identities. Parameter renaming does not change which input a result depends on. Provider metadata, aliases and editor signatures preserve this relationship; imported signatures render `from` using the currently exported parameter name. Incomplete or unsupported ownership metadata is refused. Providers carrying these contracts use manifest format 7 so older compilers cannot silently ignore a source-only relationship; ordinary providers retain format 6. A provider containing explicit ownership contracts cannot be admitted for execution by this stage.

The free-function checker-record profile is version 2. Its Incan-owned codec records access modes and source relationships in structured fields; old profiles cannot silently omit them. The profile remains limited to its supported scalar, list and callable type grammar with explicit ownership wrappers. Nominal, generic-owner and other unsupported record shapes are refused; provider metadata preserves those richer declaration contracts separately.
