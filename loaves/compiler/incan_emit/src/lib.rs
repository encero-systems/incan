//! Emission: from the IR the frontend's program lowered to, to Rust source, the conversions and ownership decisions
//! that make the source compile, the direct-execution replacement backend, and the backend selection every build
//! records.

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
