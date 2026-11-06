# Capability baselines

This directory holds a release-pinned copy of the capability catalog, read by `loaves/toolchain/incan-cli/tests/example_capability_coverage.rs` to measure which documented capabilities the example corpus demonstrates. It is not a historical stdlib archive and not a second authority for capability definitions. The present-tense authority remains `loaves/stdlib/core/src/features.incn`.

Each baseline directory contains a manifest that states its release identity, exact Git blob, checked descriptor count and retirement condition. Do not add one directory per release by default: a baseline stays only while a named test reads it.
