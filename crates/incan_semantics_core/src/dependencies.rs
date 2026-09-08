//! Checked declaration relationships shared by execution requirements and inspection graph projections.

use std::collections::{BTreeMap, BTreeSet};

use crate::{CanonicalSymbolId, CompilerNodeId, HirSourceSpan, SemanticFactKind, SemanticFactStore, SemanticFactValue};

/// One checked reference site with its unique nearest declaring owner, when the site is declaration-owned.
#[derive(Debug, Clone, Copy)]
pub struct CheckedReference<'a> {
    /// Identity selected by the checker, independent of source aliases and inspection record labels.
    pub target: &'a CanonicalSymbolId,
    /// Closest enclosing checked declaration, excluding broader ancestors and ambiguous ownership.
    pub owner: Option<&'a CanonicalSymbolId>,
}

/// Canonical relationships projected from the existing semantic snapshots, without parsing or resolving source.
#[derive(Debug, Clone, Default)]
pub struct CheckedDependencyGraph {
    dependencies: BTreeMap<CanonicalSymbolId, BTreeSet<CanonicalSymbolId>>,
}

impl CheckedDependencyGraph {
    /// Join checked declaration and reference-owner facts from one analysis's module snapshots.
    pub fn from_fact_stores<'a>(stores: impl IntoIterator<Item = &'a SemanticFactStore>) -> Self {
        let mut graph = Self::default();
        for facts in stores {
            for fact in facts.facts_by_kind(SemanticFactKind::DeclarationIdentity) {
                if let SemanticFactValue::CanonicalIdentity(identity) = &fact.value {
                    graph.dependencies.entry(identity.clone()).or_default();
                    if let Some(owner) = unique_identity(facts, &fact.subject, SemanticFactKind::RequiredMemberOwner) {
                        graph
                            .dependencies
                            .entry(owner.clone())
                            .or_default()
                            .insert(identity.clone());
                    }
                }
            }
            for fact in facts.facts_by_kind(SemanticFactKind::SymbolIdentity) {
                if let Some(reference) = facts.checked_reference(&fact.subject)
                    && let Some(owner) = reference.owner
                {
                    graph
                        .dependencies
                        .entry(owner.clone())
                        .or_default()
                        .insert(reference.target.clone());
                }
            }
        }
        graph
    }

    /// Return direct checked references owned by exactly this declaration.
    pub fn dependencies(&self, owner: &CanonicalSymbolId) -> impl Iterator<Item = &CanonicalSymbolId> {
        self.dependencies
            .get(owner)
            .into_iter()
            .flat_map(|targets| targets.iter())
    }

    /// Follow declared relationships from selected roots; unknown external targets remain explicit leaves.
    pub fn reachable_from(&self, roots: impl IntoIterator<Item = CanonicalSymbolId>) -> BTreeSet<CanonicalSymbolId> {
        let mut pending = roots.into_iter().collect::<Vec<_>>();
        let mut visited = BTreeSet::new();
        while let Some(identity) = pending.pop() {
            if visited.insert(identity.clone()) {
                pending.extend(self.dependencies(&identity).cloned());
            }
        }
        visited
    }
}

impl SemanticFactStore {
    /// Return checked source sites, including type/default references omitted by syntax-only expression walkers.
    pub fn checked_reference_sites(&self) -> impl Iterator<Item = (HirSourceSpan, CheckedReference<'_>)> {
        self.facts_by_kind(SemanticFactKind::ReferenceSpan).filter_map(|fact| {
            let SemanticFactValue::SourceSpan(span) = fact.value else {
                return None;
            };
            Some((span, self.checked_reference(&fact.subject)?))
        })
    }

    /// Return a checked target and its projected ownership through one shared query for graph consumers.
    pub fn checked_reference(&self, subject: &CompilerNodeId) -> Option<CheckedReference<'_>> {
        let target = unique_identity(self, subject, SemanticFactKind::SymbolIdentity)?;
        Some(CheckedReference {
            target,
            owner: unique_identity(self, subject, SemanticFactKind::ReferenceOwner),
        })
    }
}

/// Refuse disagreeing identity facts rather than selecting an arbitrary target or owner.
fn unique_identity<'a>(
    facts: &'a SemanticFactStore,
    subject: &CompilerNodeId,
    kind: SemanticFactKind,
) -> Option<&'a CanonicalSymbolId> {
    let mut identities = facts
        .facts_for_kind(subject, kind)
        .filter_map(|fact| match &fact.value {
            SemanticFactValue::CanonicalIdentity(identity) => Some(identity),
            _ => None,
        });
    let first = identities.next()?;
    identities.all(|identity| identity == first).then_some(first)
}

/// Project the unique nearest checked declaration containing a reference, preserving nested ownership.
///
/// Equal-width disagreeing owners are deliberately ambiguous. No consumer may turn a broad containing source span
/// into additional dependency edges merely because a nested declaration occupies part of that span.
pub fn closest_declaring_owner<'a>(
    declarations: impl IntoIterator<Item = &'a CanonicalSymbolId>,
    span: HirSourceSpan,
) -> Option<&'a CanonicalSymbolId> {
    let mut closest: Option<&CanonicalSymbolId> = None;
    let mut ambiguous = false;
    for declaration in declarations {
        let owner_span = declaration.declaration_span;
        if owner_span.start > span.start || owner_span.end < span.end {
            continue;
        }
        let width = owner_span.end.saturating_sub(owner_span.start);
        match closest {
            None => {
                closest = Some(declaration);
                ambiguous = false;
            }
            Some(previous) => {
                let previous_width = previous
                    .declaration_span
                    .end
                    .saturating_sub(previous.declaration_span.start);
                if width < previous_width {
                    closest = Some(declaration);
                    ambiguous = false;
                } else if width == previous_width && declaration != previous {
                    ambiguous = true;
                }
            }
        }
    }
    if ambiguous { None } else { closest }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SemanticFact, SemanticSourceTargetKind};

    /// Give each fixture a real declaring span; reference spellings do not participate in identity.
    fn declaration(name: &str, kind: SemanticSourceTargetKind, start: usize, end: usize) -> CanonicalSymbolId {
        CanonicalSymbolId::module_declaration(vec!["main".into()], name, kind, HirSourceSpan::new(start, end))
    }

    /// Insert one checked relationship using the same fact shape the checker publishes.
    fn reference(facts: &mut SemanticFactStore, offset: usize, owner: &CanonicalSymbolId, target: &CanonicalSymbolId) {
        let subject = CompilerNodeId::expression_span("main", offset, offset + 1);
        facts.insert(SemanticFact::new(
            subject.clone(),
            SemanticFactKind::SymbolIdentity,
            SemanticFactValue::canonical_identity(target.clone()),
        ));
        facts.insert(SemanticFact::new(
            subject,
            SemanticFactKind::ReferenceOwner,
            SemanticFactValue::canonical_identity(owner.clone()),
        ));
    }

    #[test]
    fn closest_owner_preserves_nested_method_field_and_default_contexts() {
        let model = declaration("Container", SemanticSourceTargetKind::Model, 0, 100);
        let field = declaration("value", SemanticSourceTargetKind::Field, 10, 20);
        let method = declaration("compute", SemanticSourceTargetKind::Method, 30, 90);
        let nested = declaration("inner", SemanticSourceTargetKind::Function, 50, 80);
        let declarations = [&model, &field, &method, &nested];
        assert_eq!(
            closest_declaring_owner(declarations, HirSourceSpan::new(15, 16)),
            Some(&field)
        );
        assert_eq!(
            closest_declaring_owner(declarations, HirSourceSpan::new(35, 36)),
            Some(&method)
        );
        assert_eq!(
            closest_declaring_owner(declarations, HirSourceSpan::new(60, 61)),
            Some(&nested)
        );
        assert_eq!(
            closest_declaring_owner(declarations, HirSourceSpan::new(95, 96)),
            Some(&model)
        );
        let collision = declaration("other", SemanticSourceTargetKind::Method, 30, 90);
        assert_eq!(
            closest_declaring_owner([&model, &method, &collision], HirSourceSpan::new(40, 41)),
            None
        );
    }

    #[test]
    fn execution_closure_and_reference_query_share_exact_edges_without_broad_ancestors()
    -> Result<(), Box<dyn std::error::Error>> {
        let main = declaration("main", SemanticSourceTargetKind::Function, 100, 150);
        let model = declaration("Container", SemanticSourceTargetKind::Model, 0, 90);
        let field = declaration("value", SemanticSourceTargetKind::Field, 10, 20);
        let unused_method = declaration("unused", SemanticSourceTargetKind::Method, 30, 80);
        let default = declaration("default_value", SemanticSourceTargetKind::Function, 200, 220);
        let unrelated = declaration("unrelated", SemanticSourceTargetKind::Function, 230, 250);
        let mut facts = SemanticFactStore::new();
        let subject = CompilerNodeId::declaration_span("main", 10, 20);
        facts.insert(SemanticFact::new(
            subject.clone(),
            SemanticFactKind::DeclarationIdentity,
            SemanticFactValue::canonical_identity(field.clone()),
        ));
        facts.insert(SemanticFact::new(
            subject,
            SemanticFactKind::RequiredMemberOwner,
            SemanticFactValue::canonical_identity(model.clone()),
        ));
        reference(&mut facts, 110, &main, &model);
        reference(&mut facts, 15, &field, &default);
        reference(&mut facts, 40, &unused_method, &unrelated);
        let graph = CheckedDependencyGraph::from_fact_stores([&facts]);
        let checked = facts
            .checked_reference(&CompilerNodeId::expression_span("main", 15, 16))
            .ok_or("reference absent")?;
        assert_eq!(checked.owner, Some(&field));
        assert_eq!(graph.dependencies(&field).collect::<Vec<_>>(), vec![checked.target]);
        assert_eq!(
            graph.reachable_from([main.clone()]),
            BTreeSet::from([main, model, field, default])
        );
        Ok(())
    }

    #[test]
    fn disagreeing_checked_targets_cannot_authorize_a_dependency() {
        let owner = declaration("main", SemanticSourceTargetKind::Function, 0, 90);
        let first = declaration("first", SemanticSourceTargetKind::Function, 100, 120);
        let second = declaration("second", SemanticSourceTargetKind::Function, 130, 150);
        let mut facts = SemanticFactStore::new();
        reference(&mut facts, 10, &owner, &first);
        reference(&mut facts, 10, &owner, &second);
        assert!(
            facts
                .checked_reference(&CompilerNodeId::expression_span("main", 10, 11))
                .is_none()
        );
        assert!(
            CheckedDependencyGraph::from_fact_stores([&facts])
                .dependencies(&owner)
                .next()
                .is_none()
        );
    }
}
