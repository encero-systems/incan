//! Successful-check-only specialization facts for raw single-binder newtypes.
//!
//! These facts are private to the native bridge. They never alter checking, legacy inference, or dispatch.

use std::collections::{BTreeMap, HashMap};

use crate::ast::{Program, Span};
use crate::symbols::{CallableParam, ResolvedType};
use incan_semantics_core::CanonicalSymbolId;

use super::type_info::{ConstructorFieldBinding, FunctionBindingInfo, NewtypeConstructionInfo, TypeCheckInfo};

mod project;
mod selection;

use selection::unique_method_span;

/// Project successful checked source facts without changing any legacy-consumed table.
pub(super) fn project(program: &Program, info: &TypeCheckInfo) -> NativeNewtypeFacts {
    project::project(program, info)
}

/// An atomic completed-body transaction, revalidated against every original fact used to derive it.
#[derive(Debug, Default, Clone)]
pub(crate) struct NativeNewtypeFacts {
    bodies: Vec<NativeBodyFacts>,
    /// Source owners whose storage is controlled by this atomic projection, even when a body transaction is rejected.
    owners: Vec<CanonicalSymbolId>,
}

/// Body-local refinements cannot outlive a conflicting write or an altered canonical selection.
#[derive(Debug, Default, Clone)]
struct NativeBodyFacts {
    expressions: HashMap<(usize, usize), ResolvedType>,
    assignments: HashMap<(usize, usize), ResolvedType>,
    static_receivers: HashMap<(usize, usize), ResolvedType>,
    parameters: HashMap<(usize, usize), Vec<CallableParam>>,
    field_owners: HashMap<(usize, usize), CanonicalSymbolId>,
    guards: Vec<NativeFactGuard>,
}

/// Retained authority for a native refinement; absence is never permission to reconstruct a decision.
#[derive(Debug, Clone)]
enum NativeFactGuard {
    Reference(Span, CanonicalSymbolId),
    Write(Span, String, CanonicalSymbolId, ResolvedType),
    Expression(Span, ResolvedType, Option<BTreeMap<Vec<usize>, CanonicalSymbolId>>),
    Assignment(Span, ResolvedType),
    Constructor(Span, ConstructorFieldBinding),
    Method(Span, FunctionBindingInfo),
    MethodSelection(CanonicalSymbolId, Span),
    TargetDeclarations(Vec<(Span, CanonicalSymbolId)>),
    Function(Span, FunctionBindingInfo),
    Parameters(Span, Vec<CallableParam>),
    Owner(
        Span,
        String,
        CanonicalSymbolId,
        Box<NewtypeConstructionInfo>,
        String,
        Option<CanonicalSymbolId>,
    ),
}

impl NativeFactGuard {
    /// Require the completed checker snapshot to retain the same semantic choice and source coordinate.
    fn agrees(&self, info: &TypeCheckInfo) -> bool {
        match self {
            Self::Reference(span, identity) => info.resolved_identity(*span) == Some(identity),
            Self::Write(span, name, identity, ty) => {
                info.resolved_write_identity(*span, name) == Some(identity)
                    && info.resolved_write_type(*span, name) == Some(ty)
            }
            Self::Expression(span, ty, identities) => {
                info.expr_type(*span) == Some(ty)
                    && info.expressions.expression_type_identities.get(&(span.start, span.end)) == identities.as_ref()
            }
            Self::Assignment(span, ty) => info.assignment_binding_type(*span) == Some(ty),
            Self::Constructor(span, binding) => info.constructor_field_binding(*span) == Some(binding),
            Self::Method(span, binding) => {
                info.declarations.method_bindings_by_span.get(&(span.start, span.end)) == Some(binding)
            }
            Self::MethodSelection(identity, span) => unique_method_span(identity, info) == Some(*span),
            Self::TargetDeclarations(declarations) => declarations.iter().all(|(span, identity)| {
                info.declarations.declaration_identities.get(&(span.start, span.end)) == Some(identity)
            }),
            Self::Function(span, binding) => {
                info.declarations.function_bindings_by_span.get(&(span.start, span.end)) == Some(binding)
            }
            Self::Parameters(span, params) => info.call_site_callable_params(*span) == Some(params.as_slice()),
            Self::Owner(span, name, identity, construction, parameter, binder_identity) => {
                info.declarations.declaration_identities.get(&(span.start, span.end)) == Some(identity)
                    && info.declarations.named_type_identities.get(name) == Some(identity)
                    && info.declarations.newtype_construction.get(name) == Some(construction.as_ref())
                    && info.declarations.named_type_identities.get(parameter) == binder_identity.as_ref()
            }
        }
    }
}

impl NativeNewtypeFacts {
    /// Keep a cached source owner's generic storage under atomic authority. This is a denial boundary: other additive
    /// call facts cannot repair a rejected transaction or a damaged supporting fact by supplying the same closed type.
    pub(crate) fn controls_type(&self, ty: &ResolvedType) -> bool {
        match ty {
            ResolvedType::Generic(owner, arguments) => {
                self.owners.iter().any(|identity| identity.declaration_name == *owner)
                    || arguments.iter().any(|ty| self.controls_type(ty))
            }
            ResolvedType::Ref(inner) | ResolvedType::RefMut(inner) => self.controls_type(inner),
            ResolvedType::Tuple(elements) => elements.iter().any(|ty| self.controls_type(ty)),
            _ => false,
        }
    }

    /// Read an expression refinement only while its whole body transaction retains its checked authority.
    pub(crate) fn expression_type(&self, span: Span, info: &TypeCheckInfo) -> Option<&ResolvedType> {
        self.valid_body(span, info, |body, key| body.expressions.contains_key(&key))?
            .expressions
            .get(&(span.start, span.end))
    }

    /// Keep native storage closure separate from the shared assignment types read by legacy emission.
    pub(crate) fn assignment_type(&self, span: Span, info: &TypeCheckInfo) -> Option<&ResolvedType> {
        self.valid_body(span, info, |body, key| body.assignments.contains_key(&key))?
            .assignments
            .get(&(span.start, span.end))
    }

    /// Retain distinct closed owner arguments for separate calls sharing one written static receiver.
    pub(crate) fn static_receiver_type(&self, span: Span, info: &TypeCheckInfo) -> Option<&ResolvedType> {
        self.valid_body(span, info, |body, key| body.static_receivers.contains_key(&key))?
            .static_receivers
            .get(&(span.start, span.end))
    }

    /// Supply the original selected method signature closed by its already checked native owner arguments.
    pub(crate) fn callable_params(&self, span: Span, info: &TypeCheckInfo) -> Option<&[CallableParam]> {
        self.valid_body(span, info, |body, key| body.parameters.contains_key(&key))?
            .parameters
            .get(&(span.start, span.end))
            .map(Vec::as_slice)
    }

    /// Authorize only tuple slot zero of the canonical raw newtype whose payload type was retained.
    pub(crate) fn field_owner(&self, span: Span, info: &TypeCheckInfo) -> Option<&CanonicalSymbolId> {
        self.valid_body(span, info, |body, key| body.field_owners.contains_key(&key))?
            .field_owners
            .get(&(span.start, span.end))
    }

    /// A damaged supporting fact invalidates every refinement for the body, including its earlier reads.
    fn valid_body(
        &self,
        span: Span,
        info: &TypeCheckInfo,
        contains: impl Fn(&NativeBodyFacts, (usize, usize)) -> bool,
    ) -> Option<&NativeBodyFacts> {
        let key = (span.start, span.end);
        let mut matches = self.bodies.iter().filter(|body| contains(body, key));
        let body = matches.next()?;
        if matches.next().is_some() || !body.guards.iter().all(|guard| guard.agrees(info)) {
            return None;
        }
        Some(body)
    }
}
