//! The IR and its emission under their pre-ring path. The driver's own project tests and the language server's
//! conformance root still spell `backend::ir::…`; this module is that spelling and nothing else — the crates are
//! `incan_ir` and `incan_emit`.

#[cfg(test)]
pub use incan_emit::test_support;
pub use incan_emit::{
    CodegenFacade, EmitError, EmitService, GenerationError, IrCodegen, IrEmitter, codegen, conversions, emit,
    emit_service, facade, ownership, reference_shape, trait_bound_inference,
};
pub use incan_ir::*;
