//! Visit typed Body IR source spans in the stable traversal order of native plan input witnesses.

use incan_mir_plan_native::SourceSpan;
use incan_semantics_core::HirSourceSpan;
use incan_semantics_core::body_ir::{
    Body, CallableParam, CallableParamDefault, Callee, Rvalue, Statement, StatementKind,
};

use crate::checked_values::{indexed_location, line_starts};

/// The line index is shared by every span in one body, including nested defaults and closure frames.
struct BodyPositions<'a> {
    starts: &'a [i64],
    file: &'a str,
    owner_line: i64,
    nodes: Vec<(i64, i64)>,
}

impl BodyPositions<'_> {
    fn add(&mut self, span: &HirSourceSpan) {
        let current = indexed_location(self.starts, span, self.file);
        self.nodes.push((current.line - self.owner_line, current.column));
    }

    fn parameters(&mut self, values: &[CallableParam]) {
        for parameter in values {
            self.default(&parameter.default);
            self.add(&parameter.span);
        }
    }

    fn default(&mut self, value: &CallableParamDefault) {
        match value {
            CallableParamDefault::Source(computation) => {
                self.add(&computation.span);
                self.statements(&computation.stmts);
            }
            CallableParamDefault::Unsupported { span, .. } => self.add(span),
            _ => {}
        }
    }

    fn calls(&mut self, callee: &Callee) {
        match callee {
            Callee::Method(target) => {
                if let Some(delegation) = &target.native_trait_delegation {
                    for default in &delegation.parameter_defaults {
                        self.default(default);
                    }
                }
            }
            Callee::ProviderOperation(plan) => {
                for input in &plan.inputs {
                    self.add(&input.span);
                }
            }
            _ => {}
        }
    }

    fn assignment(&mut self, value: &Rvalue) {
        match value {
            Rvalue::IsInstance(payload) => self.add(&payload.target.span),
            Rvalue::Closure { params, body, .. } => {
                self.statements(&body.stmts);
                self.parameters(params);
            }
            Rvalue::Generator { body, .. } => self.statements(&body.stmts),
            Rvalue::Match { arms, .. } => {
                for arm in arms {
                    self.statements(&arm.body_stmts);
                    self.statements(&arm.guard_stmts);
                }
            }
            _ => {}
        }
    }

    fn statements(&mut self, values: &[Statement]) {
        for statement in values {
            match &statement.kind {
                StatementKind::UnsafeRegion { acknowledgment, body } => {
                    self.add(&acknowledgment.span);
                    self.statements(&body.stmts);
                }
                StatementKind::Assign { rvalue, .. } => self.assignment(rvalue),
                StatementKind::Call { callee, .. } => self.calls(callee),
                StatementKind::If {
                    then_block, else_block, ..
                } => {
                    if let Some(other) = else_block {
                        self.statements(&other.stmts);
                    }
                    self.statements(&then_block.stmts);
                }
                StatementKind::Loop { body } => self.statements(&body.stmts),
                StatementKind::Race { arms, .. } => {
                    for arm in arms {
                        self.statements(&arm.body.stmts);
                    }
                }
                _ => {}
            }
            self.add(&statement.span);
        }
    }

    /// Collect only native span-bearing fields; declarations and identity tables never supply diagnostic positions.
    fn collect(body: &Body, starts: &[i64], file: &str, owner: &SourceSpan) -> Vec<(i64, i64)> {
        let mut map = BodyPositions {
            starts,
            file,
            owner_line: owner.line,
            nodes: Vec::new(),
        };
        map.statements(&body.block.stmts);
        for local in &body.locals {
            map.add(&local.span);
        }
        for fact in &body.panic_facts {
            map.add(&fact.span);
        }
        map.parameters(&body.params);
        for scope in &body.scopes {
            map.add(&scope.span);
        }
        map.add(&body.span);
        map.nodes
    }
}

/// A body's node positions, each line relative to its owner's line, in the traversal order products key by.
pub fn body_position_map(body: &Body, source: &str, file: &str, owner: &SourceSpan) -> Vec<(i64, i64)> {
    BodyPositions::collect(body, &line_starts(source), file, owner)
}

/// Use a module worker's one UTF-8 line index to place each admitted body in the edited file.
pub fn indexed_body_positions(body: &Body, starts: &[i64], file: &str, owner: &SourceSpan) -> Vec<(i64, i64)> {
    BodyPositions::collect(body, starts, file, owner)
}
