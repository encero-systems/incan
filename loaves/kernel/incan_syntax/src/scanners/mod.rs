//! AST scanners that detect language-feature usage without depending on compiler internals.
//!
//! These scanners walk the AST and consult the semantics registry to determine runtime requirements and feature
//! activation. They live in `incan_syntax` (alongside the AST they inspect) rather than the main compiler crate because
//! they only need AST types + registry types — no IR, no lowering.

pub mod runtime;
