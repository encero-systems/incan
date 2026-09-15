# Replacement migration baselines

This directory holds a release-pinned source fixture only while an active compatibility migration needs an immutable public-contract ruler. It is not a historical stdlib archive and it is not a second authority for public capability definitions. The present-tense authority remains `crates/incan_stdlib/stdlib/capabilities.incn`.

Each baseline directory must contain a manifest that states its release identity, exact Git blob, checked descriptor count, migration role, and retirement condition. The compatibility collector decodes the source through the ordinary checked capability-metadata path, then validates that its feature and private-requirement registrations cover the released contract.

When the migration closes, remove the active baseline from the collector. Retain its source only when a later, explicitly named regression or migration requires it; do not add one directory per release by default.
