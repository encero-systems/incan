//! Close SDK tuple-result binders from consistent checked storage, preserving legacy's deferred inference.

use super::{BodyBuilder, IncanType, TypeCheckInfo, bir};
use crate::typechecker::stdlib_loader::StdlibAstCache;
use incan_lang::lang::types::collections::{self, CollectionTypeId};
use incan_semantics_core::{CanonicalSymbolId, SymbolOrigin};
use std::collections::BTreeMap;

/// Whole-value and checked structural tuple aliases in one lexical frame.
struct Edge {
    destination: usize,
    source: usize,
    fields: Vec<usize>,
}

/// A checked assignment's source must fit this retained destination type.
struct Constraint {
    source: usize,
    fields: Vec<usize>,
    expected: IncanType,
}

/// One SDK call whose source declaration supplies an open tuple template.
struct Root {
    local: usize,
    identity: CanonicalSymbolId,
}

impl BodyBuilder<'_, '_> {
    /// Retain one concrete SDK tuple instantiation only when every reachable checked constraint agrees.
    ///
    /// Reassigned aliases, other callable frames, unknown leaves and conflicting binders remain unresolved. SDK
    /// nominal constraints must name the same canonical declaration in the SDK and caller, not merely the same text.
    pub(super) fn retain_sdk_tuple_context(&mut self, statements: &mut [bir::Statement]) {
        let mut edges = Vec::new();
        let mut constraints = Vec::new();
        let mut roots = Vec::new();
        let mut writes = BTreeMap::new();
        intake(
            statements,
            &self.locals,
            &mut edges,
            &mut constraints,
            &mut roots,
            &mut writes,
        );
        let mut cache = StdlibAstCache::new();
        for root in roots {
            let Some(record) = super::library_calls::library_call(&root.identity, &mut cache) else {
                continue;
            };
            let IncanType::Tuple(_) = &record.return_type else {
                continue;
            };
            if record.type_parameters.is_empty() {
                continue;
            }
            let aliases = aliases(root.local, &edges, &writes);
            let mut bindings = BTreeMap::new();
            let mut valid = true;
            for constraint in &constraints {
                let Some(prefix) = aliases.get(&constraint.source) else {
                    continue;
                };
                let path: Vec<_> = prefix.iter().chain(&constraint.fields).copied().collect();
                let Some(template) = projected(&record.return_type, &path) else {
                    valid = false;
                    break;
                };
                if !closed(&constraint.expected, &self.type_parameters) {
                    continue;
                }
                if !unify(
                    template,
                    &constraint.expected,
                    &root.identity,
                    self.type_info,
                    &mut cache,
                    &mut bindings,
                ) {
                    valid = false;
                    break;
                }
            }
            if !valid || !record.type_parameters.iter().all(|name| bindings.contains_key(name)) {
                continue;
            }
            for (local, path) in aliases {
                if let Some(template) = projected(&record.return_type, &path)
                    && let Some(declaration) = self.locals.get_mut(local)
                {
                    declaration.ty = substitute(template, &bindings);
                }
            }
            let arguments: Vec<_> = record
                .type_parameters
                .iter()
                .filter_map(|name| bindings.get(name).cloned())
                .collect();
            retain_arguments(statements, root.local, &root.identity, &arguments);
        }
    }
}

/// Interpret only whole locals and compiler-proven numeric tuple fields; nominal and indexed places keep their owners.
fn source(operand: &bir::Operand) -> Option<(usize, Vec<usize>)> {
    let bir::Operand::Place(read) = operand else {
        return None;
    };
    let mut fields = Vec::new();
    for field in &read.place.projection {
        let bir::PlaceElem::Field {
            name, structural: true, ..
        } = field
        else {
            return None;
        };
        fields.push(name.parse().ok()?);
    }
    Some((read.place.local_id()?.index(), fields))
}

/// Gather constraints without crossing a closure or generator's independent local table.
fn intake(
    statements: &[bir::Statement],
    locals: &[bir::LocalDecl],
    edges: &mut Vec<Edge>,
    constraints: &mut Vec<Constraint>,
    roots: &mut Vec<Root>,
    writes: &mut BTreeMap<usize, usize>,
) {
    for statement in statements {
        match &statement.kind {
            bir::StatementKind::Assign { place, rvalue } if place.projection.is_empty() => {
                let Some(local) = place.local_id().map(|id| id.index()) else {
                    continue;
                };
                *writes.entry(local).or_default() += 1;
                if let bir::Rvalue::Use(value) = rvalue
                    && let Some((source, fields)) = source(value)
                {
                    edges.push(Edge {
                        destination: local,
                        source,
                        fields: fields.clone(),
                    });
                    constraints.push(Constraint {
                        source,
                        fields,
                        expected: locals[local].ty.clone(),
                    });
                }
                if let bir::Rvalue::Aggregate(bir::AggregateKind::List, values) = rvalue
                    && let IncanType::Generic { base, args } = &locals[local].ty
                    && collections::from_str(base) == Some(CollectionTypeId::List)
                    && args.len() == 1
                {
                    for value in values {
                        let bir::ArgumentElement::One(value) = value else {
                            continue;
                        };
                        if let Some((source, fields)) = source(value) {
                            constraints.push(Constraint {
                                source,
                                fields,
                                expected: args[0].clone(),
                            });
                        }
                    }
                }
            }
            bir::StatementKind::Call {
                destination: Some(place),
                callee: bir::Callee::Function(bir::CallableTarget::Named(target)),
                ..
            } if place.projection.is_empty() => {
                let Some(local) = place.local_id().map(|id| id.index()) else {
                    continue;
                };
                *writes.entry(local).or_default() += 1;
                if target.type_args.is_empty()
                    && let Some(identity) = &target.canonical
                    && super::library_nominals::sdk_identity(identity).is_some()
                {
                    roots.push(Root {
                        local,
                        identity: identity.clone(),
                    });
                }
            }
            bir::StatementKind::Loop { body } | bir::StatementKind::UnsafeRegion { body, .. } => {
                intake(&body.stmts, locals, edges, constraints, roots, writes)
            }
            bir::StatementKind::If {
                then_block, else_block, ..
            } => {
                intake(&then_block.stmts, locals, edges, constraints, roots, writes);
                if let Some(block) = else_block {
                    intake(&block.stmts, locals, edges, constraints, roots, writes);
                }
            }
            _ => {}
        }
    }
}

/// Follow uniquely assigned aliases; any competing definition prevents borrowing its context for this root.
fn aliases(root: usize, edges: &[Edge], writes: &BTreeMap<usize, usize>) -> BTreeMap<usize, Vec<usize>> {
    let mut aliases = BTreeMap::from([(root, Vec::new())]);
    for _ in 0..edges.len() {
        let mut progress = false;
        for edge in edges {
            if writes.get(&edge.destination) != Some(&1) || aliases.contains_key(&edge.destination) {
                continue;
            }
            if let Some(prefix) = aliases.get(&edge.source) {
                let path = prefix.iter().chain(&edge.fields).copied().collect();
                aliases.insert(edge.destination, path);
                progress = true;
            }
        }
        if !progress {
            break;
        }
    }
    aliases
}

/// Select only an actual tuple component, never infer a nominal storage field.
fn projected<'a>(mut ty: &'a IncanType, fields: &[usize]) -> Option<&'a IncanType> {
    for field in fields {
        let IncanType::Tuple(items) = ty else { return None };
        ty = items.get(*field)?;
    }
    Some(ty)
}

/// Bind placeholders consistently and compare SDK nominal owners using their retained canonical identities.
fn unify(
    template: &IncanType,
    actual: &IncanType,
    identity: &CanonicalSymbolId,
    info: &TypeCheckInfo,
    cache: &mut StdlibAstCache,
    bindings: &mut BTreeMap<String, IncanType>,
) -> bool {
    if let IncanType::TypeVar(name) = template {
        if let Some(previous) = bindings.get(name) {
            return previous == actual;
        }
        bindings.insert(name.clone(), actual.clone());
        return true;
    }
    if let (
        IncanType::Generic { base, args },
        IncanType::Generic {
            base: other,
            args: actuals,
        },
    ) = (template, actual)
    {
        let SymbolOrigin::Module(path) = &identity.origin else {
            return false;
        };
        if let Some(expected) = cache.lookup_identity(path, base) {
            let found = info
                .declarations
                .named_type_identities
                .get(other)
                .or_else(|| info.body_ir_runtime_types.get(other));
            if found.and_then(super::library_nominals::sdk_identity) != super::library_nominals::sdk_identity(&expected)
            {
                return false;
            }
        } else if base != other {
            return false;
        }
        return args.len() == actuals.len()
            && args
                .iter()
                .zip(actuals)
                .all(|(a, b)| unify(a, b, identity, info, cache, bindings));
    }
    template == actual
}

/// Replace only declaration-owned placeholders after the complete constraint set has proved one substitution.
fn substitute(ty: &IncanType, bindings: &BTreeMap<String, IncanType>) -> IncanType {
    match ty {
        IncanType::TypeVar(name) => bindings.get(name).cloned().unwrap_or_else(|| ty.clone()),
        IncanType::Generic { base, args } => IncanType::Generic {
            base: base.clone(),
            args: args.iter().map(|ty| substitute(ty, bindings)).collect(),
        },
        IncanType::Tuple(items) => IncanType::Tuple(items.iter().map(|ty| substitute(ty, bindings)).collect()),
        _ => ty.clone(),
    }
}

/// A usable constraint contains no unknown leaves or SDK placeholders from another declaration.
fn closed(ty: &IncanType, parameters: &[String]) -> bool {
    match ty {
        IncanType::Unknown | IncanType::Infer | IncanType::Unsupported(_) => false,
        IncanType::TypeVar(name) => parameters.contains(name),
        IncanType::Tuple(items) | IncanType::Generic { args: items, .. } => {
            items.iter().all(|ty| closed(ty, parameters))
        }
        _ => true,
    }
}

/// Preserve the concrete call selection beside its result storage, including calls inside source control flow.
fn retain_arguments(
    statements: &mut [bir::Statement],
    root: usize,
    identity: &CanonicalSymbolId,
    arguments: &[IncanType],
) {
    for statement in statements {
        match &mut statement.kind {
            bir::StatementKind::Call {
                destination: Some(place),
                callee: bir::Callee::Function(bir::CallableTarget::Named(target)),
                ..
            } if place.local_id().is_some_and(|id| id.index() == root)
                && target.canonical.as_ref() == Some(identity) =>
            {
                target.type_args = arguments.to_vec()
            }
            bir::StatementKind::Loop { body } | bir::StatementKind::UnsafeRegion { body, .. } => {
                retain_arguments(&mut body.stmts, root, identity, arguments)
            }
            bir::StatementKind::If {
                then_block, else_block, ..
            } => {
                retain_arguments(&mut then_block.stmts, root, identity, arguments);
                if let Some(block) = else_block {
                    retain_arguments(&mut block.stmts, root, identity, arguments);
                }
            }
            _ => {}
        }
    }
}
