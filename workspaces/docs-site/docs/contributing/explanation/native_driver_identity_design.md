# Native driver identity verification

This page holds the detailed design supporting [DD-0004](../../design_records/0004_native_route_through_the_pinned_rustc.md), whose status remains **Accepted**. It preserves the technical contracts relocated from that record; relocation does not approve a draft or establish implementation completion. The decision record retains the architectural choices and their rationale.

[DD-0013](../../design_records/0013_the_incan_build_engine.md) supersedes DD-0004's restriction on which Loaves may use rustc internals. It does not remove the identity-verification boundary described here.

## Design contracts

### Driver identity

Every compilation rejects inherited `RUSTC_BOOTSTRAP` and verifies the complete compiler identity (`rustc -vV`) and the loaded `rustc_driver` library, which must be the one inside the selected sysroot, against their build-bound values before passing the proven identity to the adapter. Identity is content, never location: the driver accepts any sysroot path whose compiler output and library bytes match the build evidence, so a toolchain built on one machine runs against the same rustc installed anywhere else.

Library bytes must match the build-bound SHA-256 digest; installed drivers may retain a complete private proof atomically after verification and reuse it only while canonical path, device, inode, size, nanosecond modification/change times, and expected digest match, with stable descriptor/path state during verification. Changed, missing, malformed, or unreadable proofs require hashing; unwritable proof storage permits compilation after successful hashing, and uninstalled drivers retain no proof.

Proof reuse trusts toolchain-home ownership, offers no protection against forged proofs or filesystem metadata, and never bypasses the other identity checks.

The complete compiler identity may also be reused for an unchanged canonical compiler executable under the same file-generation guards; a missing or invalid proof requires the full identity query again. Installed drivers retain the proof after successful verification, while uninstalled drivers keep full verification.
