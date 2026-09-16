//! Emission: from the IR the frontend's program lowered to, to Rust source, the conversions and ownership decisions
//! that make the source compile, the direct-execution replacement backend, and the backend selection every build
//! records.

/// The stdlib ring's version line this compiler generates code for.
///
/// Every generated crate carries `incan_std_core::__incan_stdlib_version_check!` with this literal, and the
/// `incan_std_core` it links must be compatible with it — exactly equal while the line is a prerelease, the same
/// major.minor with a patch no older than this once it is a release. The compiler ring does not link the runtime it
/// generates for (`tests/layering_guard.rs`), so this is a declared fact rather than a read from the crate;
/// `scripts/check_ring_versions.py` keeps it equal to the stdlib ring line in the manifests, and a stdlib bump
/// updates it in the same change.
pub const GENERATED_FOR_STDLIB_VERSION: &str = "0.6.0-dev.4";

#[cfg(test)]
mod checked_program;
pub mod codegen;
pub mod conversions;
pub mod emit;
pub mod emit_service;
pub mod facade;
pub mod ownership;
pub mod prelude;
pub mod reference_shape;
pub mod replacement;
pub mod selection;
#[cfg(any(test, feature = "test_support"))]
pub mod test_support;
#[cfg(test)]
mod tests;
pub mod trait_bound_inference;

pub use codegen::{GenerationError, IrCodegen};
pub use emit::{EmitError, IrEmitter};
pub use emit_service::EmitService;
pub use facade::CodegenFacade;
