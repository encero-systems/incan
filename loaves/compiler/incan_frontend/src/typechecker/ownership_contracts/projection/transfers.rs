//! Identify cleanup subjects from authenticated explicit transfers, never from annotations or arbitrary moves.

use std::collections::BTreeSet;

use incan_semantics_core::{CanonicalSymbolId, IncanInputAccess, SemanticSourceTargetKind};

use super::{CheckedInputBody, flow};
use crate::ast::FunctionDecl;

impl CheckedInputBody<'_> {
    /// Retain every whole owner that an explicit call or Owned initializer actually transfers. Complete source
    /// access and CFG coverage are prerequisites, including on branches whose execution is still staged. The
    /// result selects cleanup subjects only; it does not authorize conditional execution or a native drop policy.
    pub(crate) fn whole_transfer_subjects(
        &self,
        function: &FunctionDecl,
    ) -> Result<BTreeSet<CanonicalSymbolId>, &'static str> {
        if self.canonical_owner().kind != SemanticSourceTargetKind::Function
            || function.name != self.canonical_owner().declaration_name
        {
            return Err("transfer inventory belongs to another source callable");
        }
        self.validate_operation_coverage(function)?;
        let mut subjects = BTreeSet::new();
        for invocation in self.graph.invocations.values() {
            for argument in &invocation.arguments {
                if argument.access == IncanInputAccess::Owned
                    && let Some(index) = argument.evaluation.source_node
                    && let Some(subject) = self.whole_transfer_subject(index)?
                {
                    subjects.insert(subject.clone());
                }
            }
        }
        for initializer in self.graph.owned_locals.values() {
            if let Some(index) = initializer.access_node
                && let Some(subject) = self.whole_transfer_subject(index)?
            {
                subjects.insert(subject.clone());
            }
        }
        Ok(subjects)
    }

    /// Only producer-linked Move events enter the inventory. The completed proof has already authenticated their
    /// exact source expression, selected input slot or Owned destination, and non-Copy decision. A borrowed alias,
    /// field projection, temporary or nonlocal source cannot borrow whole-owner cleanup authority.
    fn whole_transfer_subject(&self, index: usize) -> Result<Option<&CanonicalSymbolId>, &'static str> {
        let node = self
            .graph
            .nodes
            .get(index)
            .ok_or("transfer event is outside its owning body")?;
        match &node.event {
            flow::Event::Access {
                access: flow::Access::Read,
                ..
            } => Ok(None),
            flow::Event::Access {
                place,
                access: flow::Access::Move,
                through: None,
            } if place.projection.is_empty() => {
                let flow::Root::Binding(identity) = &place.root else {
                    return Err("whole transfer has no stored canonical owner");
                };
                if !matches!(
                    identity.kind,
                    SemanticSourceTargetKind::Local | SemanticSourceTargetKind::Parameter
                ) || !self
                    .graph
                    .permissions
                    .get(&place.root)
                    .is_some_and(|permission| permission.transferable)
                {
                    return Err("whole transfer source cannot own its cleanup obligation");
                }
                Ok(Some(identity))
            }
            _ => Err("transfer event does not describe a whole owned value"),
        }
    }
}
