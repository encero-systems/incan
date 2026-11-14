//! Preserve contextual collection carriers from exactly selected fields without changing checker state.

use std::collections::BTreeMap;

use super::{
    BodyBuilder, CanonicalSymbolId, IncanType, RetainedTypeLeaf, TypeCheckInfo, ast, named_leaf_paths,
    semantic_type_from_resolved,
};
use incan_semantics_core::body_ir::nominal_carrier_key;

/// Previous per-expression projections restored after the constructor's supplied operands have been lowered.
type PreviousTypes = BTreeMap<crate::ast::Span, Option<IncanType>>;

impl BodyBuilder<'_, '_> {
    /// Give an empty comparison literal the exact type of its checked field operand; nonempty values never gain an
    /// owner from this boundary.
    pub(super) fn retain_comparison_field_carriers(
        &mut self,
        lhs: &ast::Spanned<ast::Expr>,
        rhs: &ast::Spanned<ast::Expr>,
    ) -> PreviousTypes {
        let mut previous = PreviousTypes::new();
        if !self.retain_nominal_carriers {
            return previous;
        }
        for (source, target) in [(lhs, rhs), (rhs, lhs)] {
            if !empty_literal(target) {
                continue;
            }
            let ast::Expr::Field(base, name) = &source.node else {
                continue;
            };
            let super::bir::PlaceElem::Field {
                canonical: Some(field), ..
            } = self.lower_checked_field_projection(base, name, source.span)
            else {
                continue;
            };
            let Some((declaration, slot)) = self.local_nominal_declarations.values().find_map(|declaration| {
                declaration
                    .field_identities
                    .iter()
                    .position(|identity| identity == &field)
                    .map(|slot| (declaration, slot))
            }) else {
                continue;
            };
            let Some(field_type) = declaration.field_types.get(slot) else {
                continue;
            };
            let mut expected = field_type.clone();
            super::nominal_carriers::retain_declared_type(&mut expected, &declaration.named_type_identities);
            let identities = declaration
                .named_type_identities
                .values()
                .filter_map(|identity| nominal_carrier_key(identity).map(|key| (key, identity.clone())))
                .collect();
            let before = previous.len();
            retain_context(
                source,
                &expected,
                self.type_info,
                &identities,
                &mut self.type_overrides,
                &mut previous,
            );
            retain_context(
                target,
                &expected,
                self.type_info,
                &identities,
                &mut self.type_overrides,
                &mut previous,
            );
            if previous.len() != before {
                let mut leaves = Vec::new();
                named_leaf_paths(&expected, &mut Vec::new(), &mut leaves);
                for (_, leaf) in leaves {
                    if let RetainedTypeLeaf::Named(name) = leaf
                        && let Some(identity) = identities.get(&name)
                    {
                        self.contextual_type_identities
                            .entry(name)
                            .or_insert_with(|| identity.clone());
                    }
                }
            }
        }
        previous
    }

    /// Preserve only container-shaped arguments bound to a checked nongeneric model/class field; scalar ingress and
    /// evaluation order remain unchanged.
    pub(super) fn retain_constructor_argument_carriers(
        &mut self,
        canonical: Option<&CanonicalSymbolId>,
        field_count: usize,
        planned: &[(usize, &ast::Spanned<ast::Expr>)],
    ) -> PreviousTypes {
        let mut previous = PreviousTypes::new();
        if !self.retain_nominal_carriers {
            return previous;
        }
        let Some(declaration) = self.local_nominal_declarations.values().find(|declaration| {
            canonical == Some(&declaration.canonical)
                && declaration.type_parameter_count == 0
                && declaration.fields.len() == field_count
                && declaration.field_types.len() == field_count
                && matches!(
                    declaration.canonical.kind,
                    incan_semantics_core::SemanticSourceTargetKind::Model
                        | incan_semantics_core::SemanticSourceTargetKind::Class
                )
        }) else {
            return previous;
        };
        for (slot, expression) in planned {
            let Some(field_type) = declaration.field_types.get(*slot) else {
                continue;
            };
            let mut expected = field_type.clone();
            super::nominal_carriers::retain_declared_type(&mut expected, &declaration.named_type_identities);
            let mut identities = BTreeMap::new();
            for identity in declaration.named_type_identities.values() {
                if let Some(key) = nominal_carrier_key(identity) {
                    identities.insert(key, identity.clone());
                }
            }
            let before = previous.len();
            retain_context(
                expression,
                &expected,
                self.type_info,
                &identities,
                &mut self.type_overrides,
                &mut previous,
            );
            if previous.len() != before {
                let mut leaves = Vec::new();
                named_leaf_paths(&expected, &mut Vec::new(), &mut leaves);
                for (_, leaf) in leaves {
                    if let RetainedTypeLeaf::Named(name) = leaf
                        && let Some(identity) = identities.get(&name)
                    {
                        self.contextual_type_identities
                            .entry(name)
                            .or_insert_with(|| identity.clone());
                    }
                }
            }
        }
        previous
    }

    /// Remove the scoped contextual projection without replacing any preexisting expression override.
    pub(super) fn restore_constructor_argument_carriers(&mut self, previous: PreviousTypes) {
        for (span, ty) in previous {
            match ty {
                Some(ty) => {
                    self.type_overrides.insert(span, ty);
                }
                None => {
                    self.type_overrides.remove(&span);
                }
            }
        }
    }

    /// Emit the original checked bindings and the canonical leaves actually consumed from selected field contexts.
    pub(super) fn finish_type_identities(
        &self,
        mut identities: BTreeMap<String, CanonicalSymbolId>,
    ) -> BTreeMap<String, CanonicalSymbolId> {
        for (name, identity) in &self.contextual_type_identities {
            identities.entry(name.clone()).or_insert_with(|| identity.clone());
        }
        identities
    }
}

/// Recognize only data-free literal values, so comparison context cannot retag an existing value's storage.
fn empty_literal(expression: &ast::Spanned<ast::Expr>) -> bool {
    match &expression.node {
        ast::Expr::List(entries) => entries.is_empty(),
        ast::Expr::Dict(entries) => entries.is_empty(),
        ast::Expr::Set(items) | ast::Expr::Tuple(items) => items.is_empty(),
        ast::Expr::Paren(inner) => empty_literal(inner),
        _ => false,
    }
}

/// Project structurally unchanged checked containers, recursively preserving nested literal context rather than
/// choosing an owner from a bare source name.
fn retain_context(
    expression: &ast::Spanned<ast::Expr>,
    expected: &IncanType,
    facts: &TypeCheckInfo,
    identities: &BTreeMap<String, CanonicalSymbolId>,
    overrides: &mut std::collections::HashMap<crate::ast::Span, IncanType>,
    previous: &mut PreviousTypes,
) {
    let span = expression.span;
    let recorded = overrides
        .get(&span)
        .cloned()
        .or_else(|| facts.expr_type(expression.span).map(semantic_type_from_resolved));
    let Some(recorded) = recorded else { return };
    if !matches!(expected, IncanType::Generic { .. } | IncanType::Tuple(_))
        || !same_context_shape(&recorded, expected, facts, identities)
    {
        return;
    }
    previous.entry(span).or_insert_with(|| overrides.get(&span).cloned());
    overrides.insert(span, expected.clone());
    let arguments = match expected {
        IncanType::Generic { args, .. } | IncanType::Tuple(args) => args,
        _ => return,
    };
    match &expression.node {
        ast::Expr::List(entries) if arguments.len() == 1 => {
            for entry in entries {
                match entry {
                    ast::ListEntry::Element(item) => {
                        retain_context(item, &arguments[0], facts, identities, overrides, previous)
                    }
                    ast::ListEntry::Spread(source) => {
                        retain_context(source, expected, facts, identities, overrides, previous)
                    }
                }
            }
        }
        ast::Expr::Set(items) if arguments.len() == 1 => {
            for item in items {
                retain_context(item, &arguments[0], facts, identities, overrides, previous);
            }
        }
        ast::Expr::Dict(entries) if arguments.len() == 2 => {
            for entry in entries {
                match entry {
                    ast::DictEntry::Pair(key, value) => {
                        retain_context(key, &arguments[0], facts, identities, overrides, previous);
                        retain_context(value, &arguments[1], facts, identities, overrides, previous);
                    }
                    ast::DictEntry::Spread(source) => {
                        retain_context(source, expected, facts, identities, overrides, previous)
                    }
                }
            }
        }
        ast::Expr::Tuple(items) if items.len() == arguments.len() => {
            for (item, ty) in items.iter().zip(arguments) {
                retain_context(item, ty, facts, identities, overrides, previous);
            }
        }
        ast::Expr::Paren(inner) => retain_context(inner, expected, facts, identities, overrides, previous),
        _ => {}
    }
}

/// Require the same checked container shape and reject a caller binding that proves a different nominal owner.
fn same_context_shape(
    actual: &IncanType,
    expected: &IncanType,
    facts: &TypeCheckInfo,
    identities: &BTreeMap<String, CanonicalSymbolId>,
) -> bool {
    if actual == expected {
        return true;
    }
    match (actual, expected) {
        (IncanType::Named(actual), IncanType::Named(expected)) => {
            let Some(owner) = identities.get(expected) else {
                return false;
            };
            let checked = super::parameter_types::retained_nominal_identity(actual, facts)
                .or_else(|| facts.declarations.body_ir_native_type_identities.get(actual));
            checked.map_or_else(|| actual == &owner.declaration_name, |checked| checked == owner)
        }
        (
            IncanType::Generic {
                base: left,
                args: actual,
            },
            IncanType::Generic {
                base: right,
                args: expected,
            },
        ) if left == right && actual.len() == expected.len() => actual
            .iter()
            .zip(expected)
            .all(|(actual, expected)| same_context_shape(actual, expected, facts, identities)),
        (IncanType::Tuple(actual), IncanType::Tuple(expected)) if actual.len() == expected.len() => actual
            .iter()
            .zip(expected)
            .all(|(actual, expected)| same_context_shape(actual, expected, facts, identities)),
        _ => false,
    }
}
