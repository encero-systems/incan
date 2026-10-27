//! Mutation generations and memoized declaration identities for one checker. Every mutable access invalidates the
//! relevant input, including changes through public checker fields and restoration of dependency lookup scopes.

use std::collections::BTreeMap;
use std::ops::{Deref, DerefMut};
use std::sync::Arc;

use incan_semantics_core::CanonicalSymbolId;

use super::TypeChecker;
use crate::symbols::next_identity_generation;

/// A checker input whose mutable access starts a new nominal-identity generation. Cloning a snapshot preserves its
/// generation; restoring that snapshot is safe because its contents are the same. Fresh values receive distinct
/// generations, so replacing an input cannot accidentally reuse a stale cache entry.
#[derive(Debug, Clone)]
pub struct IdentityInput<T> {
    value: T,
    generation: u64,
}

impl<T> From<T> for IdentityInput<T> {
    /// Start tracking an input before it is visible to the checker.
    fn from(value: T) -> Self {
        Self {
            value,
            generation: next_identity_generation(),
        }
    }
}

impl<T> Deref for IdentityInput<T> {
    type Target = T;

    /// Read the input without changing its generation.
    fn deref(&self) -> &T {
        &self.value
    }
}

impl<T: Default> Default for IdentityInput<T> {
    /// Initialize the default value with a fresh mutation generation, just like an explicitly supplied input.
    fn default() -> Self {
        Self::from(T::default())
    }
}

impl<T> DerefMut for IdentityInput<T> {
    /// Invalidate before handing out mutable access; this also covers nested map entries and symbol mutations.
    fn deref_mut(&mut self) -> &mut T {
        self.generation = next_identity_generation();
        &mut self.value
    }
}

impl<'a, T> IntoIterator for &'a IdentityInput<T>
where
    &'a T: IntoIterator,
{
    type Item = <&'a T as IntoIterator>::Item;
    type IntoIter = <&'a T as IntoIterator>::IntoIter;

    /// Iterate through an immutable input without invalidating the memo.
    fn into_iter(self) -> Self::IntoIter {
        (&self.value).into_iter()
    }
}

/// The current immutable publication, bounded to one input generation. The declaration store owns its append-only
/// carrier admission index; this memo retains no historical publication when an input generation changes.
#[derive(Debug, Default)]
pub(super) struct NominalIdentityCache {
    generation: Option<[u64; 4]>,
    identities: Arc<BTreeMap<String, CanonicalSymbolId>>,
    #[cfg(test)]
    recomputations: usize,
}

impl TypeChecker {
    /// Return the exact current nominal publication, reusing its immutable map until an input changes. Mutable input
    /// access is conservatively versioned even when it changes only unrelated metadata; no stale binding can survive.
    pub(super) fn checked_nominal_type_identities(&self) -> Arc<BTreeMap<String, CanonicalSymbolId>> {
        let generation = [
            self.symbols.nominal_identity_generation(),
            self.type_aliases.generation,
            self.public_library_type_identities.generation,
            self.declaration_nominal_scopes.generation,
        ];
        let mut cache = self.nominal_identity_cache.borrow_mut();
        if cache.generation == Some(generation) {
            return Arc::clone(&cache.identities);
        }
        // Release the previous generation before allocating its replacement. A caller that still needs its snapshot
        // owns an Arc, but the checker itself never keeps historical generations alive.
        cache.identities = Arc::default();
        let identities = self.compute_nominal_type_identities();
        cache.identities = Arc::new(identities);
        cache.generation = Some(generation);
        #[cfg(test)]
        {
            cache.recomputations += 1;
        }
        Arc::clone(&cache.identities)
    }

    /// Transfer the valid body-check snapshot or the current publication into checked facts, releasing checker-only
    /// references before taking ownership. Outstanding declaration readers keep their immutable snapshot; only that
    /// case needs an owned copy for the downstream artifact.
    pub(super) fn take_checked_nominal_type_identities(&mut self) -> BTreeMap<String, CanonicalSymbolId> {
        let reusable = self.checked_body_nominal_scope().is_some();
        let identities = match self.checking_nominal_scope.take() {
            Some(scope) if reusable => scope.identities,
            _ => self.checked_nominal_type_identities(),
        };
        self.clear_nominal_identity_cache();
        match Arc::try_unwrap(identities) {
            Ok(identities) => identities,
            Err(identities) => identities.as_ref().clone(),
        }
    }

    /// Release checker-only publications on both successful and refused checks before any later stage runs. Checked
    /// declaration facts and declaration-owned scopes are independent and remain available to their consumers.
    pub(super) fn clear_nominal_identity_cache(&self) {
        let mut cache = self.nominal_identity_cache.borrow_mut();
        cache.identities = Arc::default();
        cache.generation = None;
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
        assert!(checker.nominal_identity_cache.borrow().identities.is_empty());
        let published = checker.type_info.declarations.named_type_identities.clone();
        assert_fresh(&checker);
        assert_eq!(checker.take_checked_nominal_type_identities(), published);
        assert!(checker.nominal_identity_cache.borrow().identities.is_empty());
        assert!(checker.nominal_identity_cache.borrow().generation.is_none());
        let refused = program("def wrong() -> int:\n    return \"wrong\"\n")?;
        assert!(checker.check_program(&refused).is_err());
        assert!(checker.nominal_identity_cache.borrow().identities.is_empty());
        Ok(())
    }

    /// Each changed input recomputes once; repeated reads share one map, including after mutable entry access,
    /// dependency views, alias restoration, public identity changes and nested lexical lookup changes.
    #[test]
    fn nominal_cache_recomputes_once_per_input_generation() -> Result<(), String> {
        let mut checker = TypeChecker::new();
        let assert_generation = |checker: &TypeChecker| {
            let before = checker.nominal_identity_cache.borrow().recomputations;
            assert_fresh(checker);
            let first = checker.checked_nominal_type_identities();
            assert!(Arc::ptr_eq(&first, &checker.checked_nominal_type_identities()));
            assert_eq!(checker.nominal_identity_cache.borrow().recomputations, before + 1);
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
