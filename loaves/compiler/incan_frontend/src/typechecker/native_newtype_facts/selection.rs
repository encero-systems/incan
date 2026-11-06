//! Canonical local target classification and the bounded raw owner specialization profile.

use crate::ast::{self, Declaration, Program, Span, Spanned};
use crate::symbols::ResolvedType;
use incan_semantics_core::{CanonicalSymbolId, SemanticSourceTargetKind};

use super::project::BinderOrigin;
use super::{FunctionBindingInfo, NativeFactGuard, NewtypeConstructionInfo, TypeCheckInfo};

/// Exact checked local owner frame; source spelling alone never selects a constructor or method.
#[derive(Clone, Copy)]
pub(super) struct OwnerFrame<'source> {
    pub(super) span: Span,
    pub(super) source: &'source ast::NewtypeDecl,
    pub(super) identity: &'source CanonicalSymbolId,
    pub(super) construction: &'source NewtypeConstructionInfo,
    pub(super) binder_identity: Option<&'source CanonicalSymbolId>,
}

/// Source-local generic newtype coordinates remain targets even when admission metadata is missing.
pub(super) struct TargetDomain<'source> {
    owners: Vec<(&'source CanonicalSymbolId, &'source ast::NewtypeDecl)>,
    pub(super) declarations: Vec<(Span, CanonicalSymbolId)>,
}

impl<'source> TargetDomain<'source> {
    /// Preserve exact canonical declaration coordinates before applying the specialization profile.
    pub(super) fn collect(program: &'source Program, info: &'source TypeCheckInfo) -> Option<Self> {
        let mut owners = Vec::new();
        let mut declarations = Vec::new();
        for declaration in &program.declarations {
            if let Declaration::Newtype(source) = &declaration.node
                && !source.type_params.is_empty()
            {
                let identity = info
                    .declarations
                    .declaration_identities
                    .get(&(declaration.span.start, declaration.span.end))?;
                if identity.kind != SemanticSourceTargetKind::Newtype
                    || identity.declaration_span.start != declaration.span.start
                    || identity.declaration_span.end != declaration.span.end
                {
                    return None;
                }
                declarations.push((declaration.span, identity.clone()));
                owners.push((identity, source));
            }
        }
        Some(Self { owners, declarations })
    }

    /// A selected constructor belongs to the target domain independently of its eligibility.
    pub(super) fn constructor(&self, identity: &CanonicalSymbolId) -> bool {
        self.owners.iter().any(|(owner, _)| *owner == identity)
    }

    /// Member provenance uses the checked origin and exact declaration span, never reference spelling.
    pub(super) fn method(&self, identity: &CanonicalSymbolId) -> bool {
        identity.kind == SemanticSourceTargetKind::Method
            && self.owners.iter().any(|(owner, source)| {
                owner.origin == identity.origin
                    && source.methods.iter().any(|method| {
                        identity.declaration_span.start == method.span.start
                            && identity.declaration_span.end == method.span.end
                    })
            })
    }
}

impl OwnerFrame<'_> {
    /// Keep the first specialization rule to one raw unbounded binder, with no additional construction selection.
    fn has_identity_template(&self, info: &TypeCheckInfo) -> bool {
        let source = self.source;
        let [parameter] = source.type_params.as_slice() else {
            return false;
        };
        if !parameter.bounds.is_empty()
            || self.identity.kind != SemanticSourceTargetKind::Newtype
            || source.is_rusttype
            || !source.decorators.is_empty()
            || !source.traits.is_empty()
            || !source.rebindings.is_empty()
            || !source.method_aliases.is_empty()
            || !source.method_partials.is_empty()
            || !source.associated_types.is_empty()
            || !source.interop_edges.is_empty()
            || self.construction.type_params != [parameter.name.clone()]
            || self.construction.checked_constructor.is_some()
            || self.construction.checked_constructor_identity.is_some()
            || !self.construction.constraints.is_empty()
            || info.declarations.named_type_identities.get(&source.name) != Some(self.identity)
        {
            return false;
        }
        match &self.construction.underlying {
            ResolvedType::TypeVar(name) => *name == parameter.name,
            ResolvedType::Named(name) => {
                *name == parameter.name && !info.declarations.named_type_identities.contains_key(name)
            }
            _ => false,
        }
    }

    /// Capture the owner construction authority instead of rebuilding it at a later use site.
    pub(super) fn guard(&self) -> NativeFactGuard {
        NativeFactGuard::Owner(
            self.span,
            self.source.name.clone(),
            self.identity.clone(),
            Box::new(self.construction.clone()),
            self.construction.type_params[0].clone(),
            self.binder_identity.cloned(),
        )
    }

    /// One unique owner substitution uses the accepted payload type without invoking general inference.
    pub(super) fn instantiated_type(&self, payload: ResolvedType) -> ResolvedType {
        ResolvedType::Generic(self.source.name.clone(), vec![payload])
    }

    /// The symbolic owner argument is authorized only by this declaration's exact binder.
    pub(super) fn parameter(&self) -> ResolvedType {
        ResolvedType::TypeVar(self.construction.type_params[0].clone())
    }
}

/// Collect eligible owner frames only from the checked program's exact local declaration coordinates.
pub(super) fn collect_owners<'source>(
    program: &'source Program,
    info: &'source TypeCheckInfo,
) -> Vec<OwnerFrame<'source>> {
    program
        .declarations
        .iter()
        .filter_map(|declaration| {
            let Declaration::Newtype(source) = &declaration.node else {
                return None;
            };
            let span = declaration.span;
            let identity = info.declarations.declaration_identities.get(&(span.start, span.end))?;
            let construction = info.declarations.newtype_construction.get(&source.name)?;
            let parameter = source.type_params.first()?;
            let frame = OwnerFrame {
                span,
                source,
                identity,
                construction,
                binder_identity: info.declarations.named_type_identities.get(&parameter.name),
            };
            frame.has_identity_template(info).then_some(frame)
        })
        .collect()
}

/// A source method keeps its owner and method binders separate from the call's concrete native instance.
#[derive(Clone, Copy)]
pub(super) struct MethodFrame<'source> {
    pub(super) owner: OwnerFrame<'source>,
    pub(super) source: &'source Spanned<ast::MethodDecl>,
    pub(super) binding: &'source FunctionBindingInfo,
}

/// Select a source method by retained canonical identity, never by a method or owner spelling.
pub(super) fn selected_method<'source>(
    identity: &CanonicalSymbolId,
    owners: &[OwnerFrame<'source>],
    info: &'source TypeCheckInfo,
) -> Option<MethodFrame<'source>> {
    let selected = unique_method_span(identity, info)?;
    for owner in owners {
        for method in &owner.source.methods {
            let span = method.span;
            let Some(binding) = info.declarations.method_bindings_by_span.get(&(span.start, span.end)) else {
                continue;
            };
            if span == selected
                && identity.kind == SemanticSourceTargetKind::Method
                && identity.origin == owner.identity.origin
                && identity.declaration_span.start == span.start
                && identity.declaration_span.end == span.end
                && binding.identity.as_ref() == Some(identity)
                && !binding.is_async
                && method.node.type_params.is_empty()
                && method.node.trait_target.is_none()
                && method.node.body.is_some()
                && method.node.receiver != Some(ast::Receiver::Mutable)
                && (method.node.receiver.is_some() || method.node.receiver_binding.is_none())
                && binding.params.iter().all(|parameter| {
                    parameter.kind == ast::ParamKind::Normal && !parameter.has_default && !parameter.is_mut
                })
            {
                return Some(MethodFrame {
                    owner: *owner,
                    source: method,
                    binding,
                });
            }
        }
    }
    None
}

/// Reject duplicate canonical candidates before any eligibility filter can hide one.
pub(super) fn unique_method_span(identity: &CanonicalSymbolId, info: &TypeCheckInfo) -> Option<Span> {
    let mut candidates = info
        .declarations
        .method_bindings_by_span
        .iter()
        .filter(|(_, binding)| binding.identity.as_ref() == Some(identity));
    let ((start, end), _) = candidates.next()?;
    if candidates.next().is_some() {
        return None;
    }
    Some(Span {
        start: *start,
        end: *end,
    })
}

impl OwnerFrame<'_> {
    /// Eligible frames have exactly one source binder whose coordinate remains tied to the canonical owner.
    pub(super) fn binder_origin(&self, argument: ResolvedType) -> BinderOrigin {
        BinderOrigin {
            owner: self.identity.clone(),
            declaration_span: self.source.type_params[0].span,
            name: self.construction.type_params[0].clone(),
            argument,
        }
    }
}
