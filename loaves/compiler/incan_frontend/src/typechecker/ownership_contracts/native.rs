//! Native calls in an explicit body. The selected Rust signature gives every slot its access, as an explicit
//! contract would: a `&T` input is borrowed shared, a `&mut T` input exclusively, and a value input is owned. The
//! receiver is borrowed as `&self` or `&mut self` selects. Each call becomes an input call of the body, keyed by a
//! callee identity naming its Rust item at the call site, with the contract its signature gives it.

use incan_lang::lang::types::ownership::OwnershipMode;
use incan_semantics_core::{CanonicalSymbolId, SemanticSourceTargetKind, SymbolNamespace, SymbolOrigin};

use super::{InputCall, InputCallKind};
use crate::ast::{Receiver, Span};
use crate::ownership_metadata::{AccessMode, CheckedOwnershipContract, TypeComponent};
use crate::symbols::ResolvedType;
use crate::typechecker::RustCheckedCall;
use crate::typechecker::type_info::{ProtocolArtifacts, RustInteropArtifacts};

/// Retain each native call inside the body at `body`, or every native call when the facts are one body's own, as an
/// input call of `owner`, unless the call already has one.
/// A call whose signature gives a slot no access the source flow can check is left without one, and the source flow
/// refuses it. The body proof runs this before building its flow, and record restoration runs it again, since native
/// input calls are not recorded.
pub(in crate::typechecker) fn retain_native_input_calls(
    protocols: &mut ProtocolArtifacts,
    rust: &RustInteropArtifacts,
    owner: &CanonicalSymbolId,
    body: Option<Span>,
) {
    let calls: Vec<_> = rust
        .checked_calls
        .iter()
        .filter(|(span, _)| {
            body.is_none_or(|body| span.start >= body.start && span.end <= body.end)
                && !protocols.explicit_input_calls.contains_key(*span)
        })
        .filter_map(|(span, checked)| native_input_call(owner, *span, checked).map(|call| (*span, call)))
        .collect();
    for (span, (call, contract)) in calls {
        protocols.native_input_contracts.insert(call.callee.clone(), contract);
        protocols.explicit_input_calls.insert(span, call);
    }
}

/// The input call and contract a native call's checked signature gives it.
fn native_input_call(
    owner: &CanonicalSymbolId,
    span: Span,
    checked: &RustCheckedCall,
) -> Option<(InputCall, CheckedOwnershipContract)> {
    // The checked parameters keep each native slot's reference mode, the receiver's first.
    let first = usize::from(checked.has_receiver);
    let receiver = match checked.parameters.first().map(|parameter| &parameter.ty) {
        _ if !checked.has_receiver => None,
        Some(ResolvedType::Ref(_)) => Some(Receiver::Immutable),
        Some(ResolvedType::RefMut(_)) => Some(Receiver::Mutable),
        // A receiver taken by value consumes the caller's storage, which this profile does not move.
        _ => return None,
    };
    let mut parameters = checked.parameters.get(first..)?.to_vec();
    let mut modes = Vec::with_capacity(parameters.len());
    for (slot, parameter) in parameters.iter_mut().enumerate() {
        let (mode, underlying) = match &parameter.ty {
            ResolvedType::Ref(inner) => (OwnershipMode::Borrowed, inner.as_ref().clone()),
            ResolvedType::RefMut(inner) => (OwnershipMode::BorrowedMut, inner.as_ref().clone()),
            other => (OwnershipMode::Owned, other.clone()),
        };
        // An input call's parameters carry value types; the contract carries their access.
        parameter.ty = underlying.clone();
        modes.push(AccessMode {
            mode,
            component: TypeComponent {
                parameter: Some(slot),
                children: Vec::new(),
            },
            underlying,
        });
    }
    let segments: Vec<String> = checked.path.split("::").map(str::to_owned).collect();
    let callee = CanonicalSymbolId {
        namespace: SymbolNamespace::OrdinaryLexical,
        declaration_name: segments.last()?.clone(),
        origin: SymbolOrigin::RustCrate(segments),
        kind: SemanticSourceTargetKind::Function,
        scope_discriminant: None,
        declaration_span: span,
    };
    let call = InputCall {
        kind: InputCallKind::Native,
        owner: Some(owner.clone()),
        callee,
        parameters,
        receiver,
    };
    let contract = CheckedOwnershipContract {
        schema_version: 1,
        modes,
        result_source: None,
    };
    Some((call, contract))
}

impl crate::typechecker::TypeCheckInfo {
    /// Whether the call at `span` is a native associated function written on its type, as in `HashMap.new()`: its
    /// method syntax names the type, not a receiver value, so it is called like a free function.
    pub(in crate::typechecker) fn native_associated_call(&self, span: Span) -> bool {
        self.protocols
            .explicit_input_calls
            .get(&span)
            .is_some_and(|call| call.kind == InputCallKind::Native && call.receiver.is_none())
    }
}
