//! BuiltinHandlers registry (scaffold)
//!
//! This module prepares a table-driven approach for builtin function handling
//! without changing current behavior. The actual emission remains in IrEmitter
//! until parity is proven.

#[derive(Default)]
pub struct BuiltinHandlers;

impl BuiltinHandlers {
    pub fn new() -> Self {
        Self
    }
    pub fn is_builtin(&self, _name: &str) -> bool {
        false
    }
    // Future: emit builtin call fragments via syn/quote
}
