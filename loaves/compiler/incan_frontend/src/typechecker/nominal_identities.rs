//! One current nominal publication, governed by the mutation owners' revisions and released after checking.

use std::collections::BTreeMap;
use std::sync::Arc;

use incan_semantics_core::CanonicalSymbolId;

use super::{CheckingNominalScope, TypeChecker};

impl TypeChecker {
    /// Share the current nominal map only while its mutation-owned inputs match; release a stale snapshot before
    /// collecting its replacement so the checker retains no historical publication during allocation.
    pub(super) fn checked_nominal_type_identities(&self) -> Arc<BTreeMap<String, CanonicalSymbolId>> {
        if let Some(identities) = self.checked_body_nominal_scope() {
            return identities;
        }
        self.clear_nominal_identity_cache();
        let identities = Arc::new(self.collect_checked_nominal_scope());
        self.checking_nominal_scope.replace(Some(CheckingNominalScope {
            identities: Arc::clone(&identities),
            inputs: self.nominal_scope_inputs(),
        }));
        identities
    }

    /// Transfer the valid body-check snapshot or the current publication into checked facts, releasing checker-only
    /// references before taking ownership. Outstanding declaration readers keep their immutable snapshot; only that
    /// case needs an owned copy for the downstream artifact.
    pub(super) fn take_checked_nominal_type_identities(&mut self) -> BTreeMap<String, CanonicalSymbolId> {
        let identities = self.checked_nominal_type_identities();
        self.clear_nominal_identity_cache();
        match Arc::try_unwrap(identities) {
            Ok(identities) => identities,
            Err(identities) => identities.as_ref().clone(),
        }
    }

    /// Release checker-only publications on both successful and refused checks before any later stage runs. Checked
    /// declaration facts and declaration-owned scopes are independent and remain available to their consumers.
    pub(super) fn clear_nominal_identity_cache(&self) {
        self.checking_nominal_scope.replace(None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{Program, Span};
    use crate::symbols::{ResolvedType, ScopeKind, Symbol, SymbolKind, TypeInfo};
    use crate::typechecker::{PublicLibraryTypeIdentity, TypeAliasTarget};
    use incan_semantics_core::{HirSourceSpan, SemanticSourceTargetKind};

    /// Parse the same source form accepted by an ordinary multi-module check.
    fn program(source: &str) -> Result<Program, String> {
        let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
        crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))
    }

    /// Compare the memo with a computation that neither reads nor populates either cache.
    fn assert_fresh(checker: &TypeChecker) {
        let fresh = crate::typechecker::nominal_scope_tests::fresh_scope(checker);
        assert_eq!(checker.checked_nominal_type_identities().as_ref(), &fresh);
    }

    /// Dependency interface restoration, same-spelled declarations and alias imports retain fresh-computation maps.
    #[test]
    fn nominal_cache_matches_fresh_across_modules() -> Result<(), String> {
        let left = program("pub model Left:\n    pub value: int\n\npub type Alias = Left\n")?;
        let right = program("from left import Alias\n\npub model Right:\n    pub value: Alias\n")?;
        let main =
            program("from right import Right\n\ndef read(value: Right) -> int:\n    return value.value.value\n")?;
        let mut checker = TypeChecker::new();
        checker.import_module(&left, "left");
        assert_fresh(&checker);
        let owner = checker
            .declaration_nominal_scopes
            .iter()
            .find(|(identity, _)| identity.declaration_name == "Left")
            .ok_or("missing declaring scope")?;
        assert!(
            checker
                .declaration_nominal_scopes
                .iter()
                .any(|(identity, scope)| { identity != owner.0 && Arc::ptr_eq(scope, owner.1) })
        );
        checker.import_module(&right, "right");
        assert_fresh(&checker);
        checker.check_program(&main).map_err(|errors| format!("{errors:?}"))?;
        assert!(checker.checking_nominal_scope.borrow().is_none());
        let published = checker.type_info.declarations.named_type_identities.clone();
        assert_fresh(&checker);
        assert_eq!(checker.take_checked_nominal_type_identities(), published);
        assert!(checker.checking_nominal_scope.borrow().is_none());
        let refused = program("def wrong() -> int:\n    return \"wrong\"\n")?;
        assert!(checker.check_program(&refused).is_err());
        assert!(checker.checking_nominal_scope.borrow().is_none());
        Ok(())
    }

    /// Repeated reads share one fresh map across mutable aliases, dependency views, restored owners and lexical
    /// lookup changes; each nominal revision also releases the checker's earlier snapshot.
    #[test]
    fn nominal_cache_recomputes_once_per_input_generation() -> Result<(), String> {
        let mut checker = TypeChecker::new();
        let assert_generation = |checker: &TypeChecker| {
            assert_fresh(checker);
            let first = checker.checked_nominal_type_identities();
            assert!(Arc::ptr_eq(&first, &checker.checked_nominal_type_identities()));
        };
        assert_generation(&checker);
        let previous = Arc::downgrade(&checker.checked_nominal_type_identities());
        let id = checker.symbols.define(Symbol {
            name: "Alias".into(),
            kind: SymbolKind::Type(TypeInfo::TypeAlias),
            span: Span::new(0, 1),
            scope: 0,
        });
        assert_generation(&checker);
        assert!(previous.upgrade().is_none(), "the previous generation must be released");
        let identity = CanonicalSymbolId::module_declaration(
            vec!["provider".into()],
            "Item",
            SemanticSourceTargetKind::Model,
            HirSourceSpan::new(0, 4),
        );
        checker.type_aliases.insert(
            "Alias".into(),
            TypeAliasTarget {
                type_params: vec![],
                target: ResolvedType::Named("Item".into()),
                nominal_identities: BTreeMap::from([("Item".into(), identity.clone())]),
            },
        );
        assert_generation(&checker);
        let saved = checker.type_aliases.clone();
        checker
            .type_aliases
            .get_mut("Alias")
            .ok_or("missing alias")?
            .nominal_identities
            .clear();
        assert_generation(&checker);
        checker.type_aliases = saved;
        assert_generation(&checker);
        checker.public_library_type_identities.insert(
            "Export".into(),
            PublicLibraryTypeIdentity {
                dependency_key: "provider".into(),
                source_path: vec!["Item".into()],
                canonical: Some(identity.clone()),
                selected_provider: None,
            },
        );
        assert_generation(&checker);
        let scope = Arc::new(BTreeMap::from([("Item".into(), identity.clone())]));
        checker.declaration_nominal_scopes.insert(identity, Arc::clone(&scope));
        checker.retain_nominal_scope_carriers(&scope);
        assert_generation(&checker);
        checker.symbols.get_mut(id).ok_or("missing symbol")?.kind = SymbolKind::Type(TypeInfo::TypeAlias);
        assert_generation(&checker);
        checker.symbols.begin_dependency_interface_bindings();
        assert_generation(&checker);
        checker.symbols.finish_dependency_interface_bindings();
        assert_generation(&checker);
        checker.symbols.enter_scope(ScopeKind::Block);
        assert_generation(&checker);
        checker.symbols.exit_scope();
        assert_generation(&checker);
        checker.symbols.restore_current_scope_binding("Alias", None);
        assert_generation(&checker);
        checker.public_library_type_identities.clear();
        assert_generation(&checker);
        Ok(())
    }
}
