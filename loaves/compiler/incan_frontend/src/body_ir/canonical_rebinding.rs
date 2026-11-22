//! Refresh edit-unstable dependency call locators in retained whole-module Body IR.

use std::cell::{Cell, OnceCell};
use std::collections::HashMap;
use std::sync::OnceLock;

use super::bir;
use incan_semantics_core::CanonicalSymbolId;
use incan_semantics_core::stable_identity::StableDeclarationId;

type CallTargets<'a> = HashMap<&'a str, Vec<&'a CanonicalSymbolId>>;

/// The immutable Loaf portion of every authored module's call-target view, indexed at first use.
pub struct DependencyCallTargets<'a> {
    modules: &'a [bir::BodyIrModule],
    callables: OnceLock<CallTargets<'a>>,
}

impl<'a> DependencyCallTargets<'a> {
    /// Borrow the fixed dependencies for one compilation and share their index among its workers.
    pub fn new(modules: &'a [bir::BodyIrModule]) -> Self {
        Self {
            modules,
            callables: OnceLock::new(),
        }
    }

    /// The modules whose exact callable identities back the shared index.
    pub fn modules(&self) -> &'a [bir::BodyIrModule] {
        self.modules
    }

    /// Rebind retained imported calls to their current locations. The known Loaves have a shared index.
    /// The authored portion is specific to a component and includes only the peers this body can see.
    /// Displaced locations may change only after exact and ambiguous matches have been ruled out.
    pub fn refresh(&self, module: &mut bir::BodyIrModule, authored: &[&bir::BodyIrModule]) -> bool {
        let changed = Cell::new(false);
        let authored_targets = OnceCell::new();
        let rebind = |previous: &mut CanonicalSymbolId| {
            let fixed = self.callables.get_or_init(|| index_call_targets(self.modules));
            let local = authored_targets.get_or_init(|| index_call_targets(authored.iter().copied()));
            let name = previous.declaration_name.as_str();
            let fixed = fixed.get(name).map(Vec::as_slice).unwrap_or_default();
            let local = local.get(name).map(Vec::as_slice).unwrap_or_default();
            if let Some(selected) = displaced_call_target(previous, fixed, local) {
                *previous = selected.clone();
                changed.set(true);
            }
        };
        for body in &mut module.bodies {
            refresh_statements(&mut body.block.stmts, &rebind);
            for parameter in &mut body.params {
                if let bir::CallableParamDefault::Source(default) = &mut parameter.default {
                    refresh_statements(&mut default.stmts, &rebind);
                }
            }
        }
        changed.get()
    }
}

/// Group borrowed canonical declarations by the spelling at the retained call target.
fn index_call_targets<'a>(modules: impl IntoIterator<Item = &'a bir::BodyIrModule>) -> CallTargets<'a> {
    let mut targets = CallTargets::new();
    for identity in modules.into_iter().flat_map(dependency_callable_identities) {
        targets.entry(&identity.declaration_name).or_default().push(identity);
    }
    targets
}

/// An exact match needs no rewrite. A displaced call can move only to one distinct matching stable declaration.
fn displaced_call_target<'a>(
    previous: &CanonicalSymbolId,
    fixed: &[&'a CanonicalSymbolId],
    authored: &[&'a CanonicalSymbolId],
) -> Option<&'a CanonicalSymbolId> {
    if (fixed.is_empty() && authored.is_empty()) || fixed.contains(&previous) || authored.contains(&previous) {
        return None;
    }
    let stable = StableDeclarationId::from_canonical(previous, None, None)?;
    let mut matching = fixed.iter().chain(authored).copied().filter(|candidate| {
        candidate.origin == previous.origin
            && StableDeclarationId::from_canonical(candidate, None, None).as_ref() == Some(&stable)
    });
    let first = matching.next()?;
    matching.all(|candidate| candidate == first).then_some(first)
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
fn refresh_statements(statements: &mut [bir::Statement], rebind: &impl Fn(&mut CanonicalSymbolId)) {
    for statement in statements {
        if let bir::StatementKind::Call {
            callee: bir::Callee::Function(bir::CallableTarget::Named(target)),
            ..
        } = &mut statement.kind
            && target.direct_call_id.is_none()
            && target.builtin.is_none()
            && let Some(previous) = &mut target.canonical
        {
            rebind(previous);
        }
        match &mut statement.kind {
            bir::StatementKind::Assign {
                rvalue: bir::Rvalue::Match { arms, .. },
                ..
            } => {
                for arm in arms {
                    refresh_statements(&mut arm.guard_stmts, rebind);
                    refresh_statements(&mut arm.body_stmts, rebind);
                }
            }
            bir::StatementKind::Assign {
                rvalue: bir::Rvalue::Closure { body, .. },
                ..
            } => refresh_statements(&mut body.stmts, rebind),
            bir::StatementKind::Assign {
                rvalue: bir::Rvalue::Generator { body, .. },
                ..
            } => refresh_statements(&mut body.stmts, rebind),
            bir::StatementKind::If {
                then_block, else_block, ..
            } => {
                refresh_statements(&mut then_block.stmts, rebind);
                if let Some(block) = else_block {
                    refresh_statements(&mut block.stmts, rebind);
                }
            }
            bir::StatementKind::Loop { body } | bir::StatementKind::UnsafeRegion { body, .. } => {
                refresh_statements(&mut body.stmts, rebind);
            }
            bir::StatementKind::Race { arms, .. } => {
                for arm in arms {
                    refresh_statements(&mut arm.body.stmts, rebind);
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

        let loaded = [dependency];
        let targets = DependencyCallTargets::new(&loaded);
        assert!(targets.refresh(&mut importer, &[]));
        assert!(!targets.refresh(&mut importer, &[]));

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

    /// The shared Loaf index and authored peer index contribute equally to exact matches and ambiguity.
    #[test]
    fn exact_foreign_duplicate_and_ambiguous_candidates_keep_their_authority() {
        let old = identity("first", "read", 1, 10);
        let new = identity("first", "read", 2, 11);
        let other = identity("first", "read", 3, 12);
        let foreign = identity("second", "read", 1, 10);
        assert_eq!(displaced_call_target(&old, &[], &[&foreign]), None);
        assert_eq!(displaced_call_target(&old, &[&new], &[&old]), None);
        assert_eq!(displaced_call_target(&old, &[&new], &[&other]), None);
        assert_eq!(displaced_call_target(&old, &[&foreign, &new], &[&new]), Some(&new));
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
