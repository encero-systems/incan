//! Refresh edit-unstable dependency call locators in retained whole-module Body IR.

use std::collections::{HashMap, HashSet};

use super::bir;
use incan_semantics_core::CanonicalSymbolId;
use incan_semantics_core::stable_identity::StableDeclarationId;

/// Rebind retained imported calls against the dependency modules selected for this compilation.
///
/// Canonical identities deliberately contain source spans and therefore survive only one compilation. A retained
/// namespace may keep its Body IR when an imported callable's signature is unchanged, while that callable's edited
/// body changes its current canonical span. This boundary replaces only the retained call locator; declaration
/// selection remains keyed by the compiler's span-free stable identity and ambiguous candidates remain untouched.
pub fn refresh_dependency_call_targets(module: &mut bir::BodyIrModule, dependencies: &[bir::BodyIrModule]) {
    let mut exact = HashSet::new();
    let mut current: HashMap<StableDeclarationId, Option<CanonicalSymbolId>> = HashMap::new();
    for identity in dependencies.iter().flat_map(dependency_callable_identities) {
        exact.insert(identity.clone());
        let Some(stable) = StableDeclarationId::from_canonical(identity, None, None) else {
            continue;
        };
        match current.entry(stable) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(Some(identity.clone()));
            }
            std::collections::hash_map::Entry::Occupied(mut entry) => {
                if entry.get().as_ref() != Some(identity) {
                    entry.insert(None);
                }
            }
        }
    }
    for body in &mut module.bodies {
        refresh_statements(&mut body.block.stmts, &exact, &current);
        for parameter in &mut body.params {
            if let bir::CallableParamDefault::Source(default) = &mut parameter.default {
                refresh_statements(&mut default.stmts, &exact, &current);
            }
        }
    }
}

/// Enumerate declarations a dependency module can supply to an imported call without using call-site spellings.
fn dependency_callable_identities(module: &bir::BodyIrModule) -> impl Iterator<Item = &CanonicalSymbolId> {
    module
        .bodies
        .iter()
        .filter_map(|body| body.canonical.as_ref())
        .chain(module.loaf_callables.iter().map(|callable| &callable.canonical))
}

/// Rewrite named calls in every nested executable block, including closure, generator and match-owned statements.
fn refresh_statements(
    statements: &mut [bir::Statement],
    exact: &HashSet<CanonicalSymbolId>,
    current: &HashMap<StableDeclarationId, Option<CanonicalSymbolId>>,
) {
    for statement in statements {
        if let bir::StatementKind::Call {
            callee: bir::Callee::Function(bir::CallableTarget::Named(target)),
            ..
        } = &mut statement.kind
            && target.direct_call_id.is_none()
            && target.builtin.is_none()
            && let Some(previous) = &mut target.canonical
            && !exact.contains(previous)
            && let Some(stable) = StableDeclarationId::from_canonical(previous, None, None)
            && let Some(Some(selected)) = current.get(&stable)
        {
            *previous = selected.clone();
        }
        match &mut statement.kind {
            bir::StatementKind::Assign {
                rvalue: bir::Rvalue::Match { arms, .. },
                ..
            } => {
                for arm in arms {
                    refresh_statements(&mut arm.guard_stmts, exact, current);
                    refresh_statements(&mut arm.body_stmts, exact, current);
                }
            }
            bir::StatementKind::Assign {
                rvalue: bir::Rvalue::Closure { body, .. },
                ..
            } => refresh_statements(&mut body.stmts, exact, current),
            bir::StatementKind::Assign {
                rvalue: bir::Rvalue::Generator { body, .. },
                ..
            } => refresh_statements(&mut body.stmts, exact, current),
            bir::StatementKind::If {
                then_block, else_block, ..
            } => {
                refresh_statements(&mut then_block.stmts, exact, current);
                if let Some(block) = else_block {
                    refresh_statements(&mut block.stmts, exact, current);
                }
            }
            bir::StatementKind::Loop { body } | bir::StatementKind::UnsafeRegion { body, .. } => {
                refresh_statements(&mut body.stmts, exact, current);
            }
            bir::StatementKind::Race { arms, .. } => {
                for arm in arms {
                    refresh_statements(&mut arm.body.stmts, exact, current);
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use incan_semantics_core::{
        CanonicalSymbolId, CompilerNodeId, HirSourceSpan, IncanPrimitiveType, IncanType, SemanticSourceTargetKind,
    };

    /// A retained importer follows the current dependency declaration after a signature-preserving body edit.
    #[test]
    fn retained_import_call_rebinds_after_dependency_body_edit() -> Result<(), String> {
        let previous = identity("dependency", "parse_version", 10, 80);
        let current = identity("dependency", "parse_version", 10, 96);
        let mut dependency = bir::BodyIrModule::declarations_only("dependency.incn".to_owned());
        dependency.bodies.push(body(current.clone(), None));
        let mut importer = bir::BodyIrModule::declarations_only("main.incn".to_owned());
        importer
            .bodies
            .push(body(identity("main", "main", 0, 40), Some(previous)));

        refresh_dependency_call_targets(&mut importer, &[dependency]);

        let Some(statement) = importer.bodies.first().and_then(|body| body.block.stmts.first()) else {
            return Err("retained importer has no call statement".to_owned());
        };
        let bir::StatementKind::Call {
            callee: bir::Callee::Function(bir::CallableTarget::Named(target)),
            ..
        } = &statement.kind
        else {
            return Err("retained importer call changed shape".to_owned());
        };
        if target.canonical.as_ref() != Some(&current) {
            return Err("retained importer kept the dependency's previous canonical locator".to_owned());
        }
        Ok(())
    }

    /// Build one module-level function identity whose span can change independently of its stable identity.
    fn identity(module: &str, name: &str, start: usize, end: usize) -> CanonicalSymbolId {
        CanonicalSymbolId::module_declaration(
            vec![module.to_owned()],
            name,
            SemanticSourceTargetKind::Function,
            HirSourceSpan::new(start, end),
        )
    }

    /// Build the minimal complete body needed to represent a dependency declaration or one imported call.
    fn body(canonical: CanonicalSymbolId, call: Option<CanonicalSymbolId>) -> bir::Body {
        let span = canonical.declaration_span;
        let statements = call
            .map(|target| {
                vec![bir::Statement {
                    kind: bir::StatementKind::Call {
                        destination: None,
                        callee: bir::Callee::Function(bir::CallableTarget::Named(bir::NamedCallableTarget {
                            name: target.declaration_name.clone(),
                            direct_call_id: None,
                            builtin: None,
                            type_args: Vec::new(),
                            binding: bir::ArgumentBinding::resolved_positional(0),
                            canonical: Some(target),
                            receiver_type: None,
                        })),
                        args: Vec::new(),
                        may_panic: false,
                    },
                    span,
                }]
            })
            .unwrap_or_default();
        bir::Body {
            nominal_instantiation: Vec::new(),
            trait_default_instantiation: None,
            type_parameters: Vec::new(),
            decl_id: CompilerNodeId::declaration_span("fixture", span.start, span.end),
            direct_call_id: CompilerNodeId::declaration_span("fixture", span.start, span.end),
            canonical: Some(canonical.clone()),
            name: canonical.declaration_name,
            span,
            return_type: IncanType::Primitive(IncanPrimitiveType::Unit),
            callable_representation: None,
            checked_input_profile: None,
            explicit_input_bindings: Vec::new(),
            returned_view: None,
            named_type_identities: Default::default(),
            locals: Vec::new(),
            params: Vec::new(),
            param_locals: Vec::new(),
            scopes: vec![bir::ScopeInfo {
                id: bir::ScopeId(0),
                parent: None,
                span,
            }],
            block: bir::Block {
                scope: bir::ScopeId(0),
                stmts: statements,
            },
            runtime_requirements: Vec::new(),
            panic_facts: Vec::new(),
            is_async: false,
            retired_rust_facade_wire: None,
            native_attributes: Vec::new(),
        }
    }
}
