//! EmitService: structured façade over IR emission
//!
//! This module provides typed emitters for expressions, statements, and declarations.
//! It currently delegates to `IrEmitter` to preserve behavior.

use crate::IrEmitter;
use crate::emit_service::builtins::BuiltinHandlers;
use incan_ir::IrProgram;

pub struct EmitService<'a> {
    inner: IrEmitter<'a>,
    builtins: BuiltinHandlers,
}

impl<'a> EmitService<'a> {
    pub fn new_from_program(ir: &'a IrProgram) -> Self {
        Self {
            inner: IrEmitter::new(&ir.function_registry),
            builtins: BuiltinHandlers::new(),
        }
    }
    pub fn inner_mut(&mut self) -> &mut IrEmitter<'a> {
        &mut self.inner
    }
    #[allow(dead_code)]
    pub fn builtins(&self) -> &BuiltinHandlers {
        &self.builtins
    }
}

pub mod builtins;
pub mod decl;
pub mod expr;
pub mod stmt;
