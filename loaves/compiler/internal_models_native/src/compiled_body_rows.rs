//! Independent Crumb rows for compiler-produced checker records and Body IR.

use crate::checker_record_models::{CompiledBodyFallback, CompiledBodyValue, CompiledNamespaceValue};

/// One independently bounded compiler output row, never a whole-program transport envelope.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CompiledBodyOutput {
    Namespace(CompiledNamespaceValue),
    LegacyNamespace(String),
    Value(CompiledBodyValue),
    Fallback(CompiledBodyFallback),
}
