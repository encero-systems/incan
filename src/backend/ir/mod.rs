//! The IR and its emission, as one module for the callers that still spell `crate::backend::ir::…`: the IR crate and
//! the emission crate are re-exported here under their old paths until the driver becomes its own crate.

#[cfg(test)]
pub use incan_emit::test_support;
pub use incan_emit::{
    CodegenFacade, EmitError, EmitService, GenerationError, IrCodegen, IrEmitter, codegen, conversions, emit,
    emit_service, facade, ownership, prelude, reference_shape, trait_bound_inference,
};
pub use incan_ir::*;
