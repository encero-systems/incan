//! Borrowed bootstrap views of the checker's declaration authority and current lexical visibility point.
//!
//! These views expose existing collected facts without resolving annotations again or cloning a symbol table. A view
//! cannot survive the mutable checker operation that advances visibility. They are not persisted record readers.

use incan_semantics_core::CanonicalSymbolId;
use incan_semantics_core::stable_identity::StableDeclarationId;

use crate::ast::{Declaration, Span, Spanned};
use crate::symbols::{Symbol, SymbolTable};
use std::collections::BTreeSet;

use super::{FunctionBindingInfo, TypeAliasTarget, TypeChecker};

/// Source-order boundary of the currently readable environment; coordinates are provenance, never semantic keys.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum VisibilityPoint {
    /// Collection has not completed; no final declaration inventory is promised.
    #[default]
    Collecting,
    /// Headers are final but no declaration body or user decorator has run.
    Collected,
    /// Declaring scope before this declaration's defaults, body and decorator rebinding.
    BeforeDeclaration(Span),
    /// Scope after this declaration's body and any decorator rebinding.
    AfterDeclaration(Span),
    /// Checking and deferred settlement have completed, possibly with diagnostics.
    Finished,
}

/// One existing name-slot selection, retaining the resolver's exact canonical target or explicit lack of proof.
#[derive(Clone, Copy)]
pub struct CheckedBindingView<'a> {
    /// Checked symbol, including its complete callable/overload/type/trait fields.
    pub symbol: &'a Symbol,
    /// Resolved target; aliases preserve their declaring target rather than minting an alias-site identity.
    pub canonical: Option<&'a CanonicalSymbolId>,
}

/// Read-only lexical resolution at one visibility point; borrowing prevents a later rebind becoming visible here.
#[derive(Clone, Copy)]
pub struct CheckEnvironmentView<'a> {
    symbols: &'a SymbolTable,
    point: VisibilityPoint,
}

impl<'a> CheckEnvironmentView<'a> {
    /// Return the boundary captured when this view was borrowed, without retaining a historical environment.
    pub fn visibility_point(self) -> VisibilityPoint {
        self.point
    }

    /// Read an active source import's accepted path; shadows are resolved by the existing symbol authority.
    pub fn import_path(self, name: &str) -> Option<&'a [String]> {
        self.symbols.import_binding_path(name)
    }

    /// Select through the existing lexical/import resolver, including shadowing and overload-set bindings.
    pub fn lookup(self, name: &str) -> Option<CheckedBindingView<'a>> {
        let id = self.symbols.lookup(name)?;
        Some(CheckedBindingView {
            symbol: self.symbols.get(id)?,
            canonical: self.symbols.identity_of(id),
        })
    }
}

/// Declaration-owned source facts, separate from executable carrier selection and mutable callable bindings.
#[derive(Clone, Copy)]
pub struct DeclarationView<'a> {
    checker: &'a TypeChecker,
}

impl<'a> DeclarationView<'a> {
    /// Read a collected source function candidate by its revision-local handle, preserving overload distinction.
    pub fn function(self, span: Span) -> Option<&'a FunctionBindingInfo> {
        self.checker.type_info.declarations.function_bindings_by_span.get(&span)
    }

    /// Read a collected method candidate without reconstructing its owner or checked annotation types.
    pub fn method(self, span: Span) -> Option<&'a FunctionBindingInfo> {
        self.checker.type_info.declarations.method_bindings_by_span.get(&span)
    }

    /// Read a source alias's checked arguments, target and nominal provenance from its declaring authority.
    pub fn alias(self, name: &str) -> Option<&'a TypeAliasTarget> {
        self.checker.type_aliases.get(name)
    }

    /// Read the checked inferred hash requirements of this exact callable, including imported requirements.
    pub fn hash_parameters(self, canonical: &CanonicalSymbolId) -> Option<&'a BTreeSet<String>> {
        self.checker.hash_key_inference.type_params.get(canonical)
    }

    /// Read the shared position-independent source-signature identity; unresolved signatures remain absent.
    pub fn source_identity(self, canonical: &CanonicalSymbolId) -> Option<&'a StableDeclarationId> {
        self.checker.type_info.declarations.source_identities.get(canonical)
    }
}

impl TypeChecker {
    /// Borrow declaration facts already published by collection; this performs no checking or annotation resolution.
    pub fn declaration_view(&self) -> DeclarationView<'_> {
        DeclarationView { checker: self }
    }

    /// Borrow the active lexical environment, including any prior source-order decorator transitions.
    pub fn check_environment(&self) -> CheckEnvironmentView<'_> {
        CheckEnvironmentView {
            symbols: &self.symbols,
            point: self.visibility_point,
        }
    }

    /// Advance one source-order boundary around the existing declaration checker, preserving default/body order.
    pub(super) fn check_environment_declaration(&mut self, declaration: &Spanned<Declaration>) {
        self.visibility_point = VisibilityPoint::BeforeDeclaration(declaration.span);
        self.check_declaration(declaration);
        self.visibility_point = VisibilityPoint::AfterDeclaration(declaration.span);
    }
}
