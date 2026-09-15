//! CodegenFacade: planned orchestration layer
//!
//! A future façade to coordinate scanners, lowering and emission.

use crate::IrCodegen;
use incan_frontend::ast::Program;

pub struct CodegenFacade;

impl CodegenFacade {
    pub fn generate(program: &Program) -> String {
        // Delegate to existing IrCodegen for now
        IrCodegen::new().generate(program)
    }
}
