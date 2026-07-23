//! Parameter mutation analysis for function emission.
//!
//! Uses the same recursive statement mutation query as loop emission so newly supported expressions cannot make the
//! two mutability decisions drift.

use std::collections::HashSet;

use super::super::IrEmitter;
use crate::backend::ir::emit::statements::stmt_mutates_var;

impl<'a> IrEmitter<'a> {
    /// Collect the function parameters that are actually mutated by the emitted body.
    ///
    /// Rust signatures receive `mut` or `&mut` only for parameters whose assignments, borrows, or selected method
    /// calls require it. Sharing the statement mutation query with loop emission keeps that decision consistent.
    pub(in crate::backend::ir::emit) fn collect_mutated_params(
        &self,
        func: &super::super::super::decl::IrFunction,
    ) -> HashSet<String> {
        func.params
            .iter()
            .filter(|param| func.body.iter().any(|stmt| stmt_mutates_var(stmt, &param.name)))
            .map(|param| param.name.clone())
            .collect()
    }
}
