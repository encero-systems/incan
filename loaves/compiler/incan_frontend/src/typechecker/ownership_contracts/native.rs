//! Native calls in an explicit body. The selected Rust signature gives every slot its access, as an explicit
//! contract would: a `&T` input is borrowed shared, a `&mut T` input exclusively, and a value input is owned. The
//! receiver is borrowed as `&self` or `&mut self` selects. Each call becomes an input call of the body, keyed by a
//! callee identity naming its Rust item at the call site, with the contract its signature gives it.

use incan_lang::lang::types::ownership::OwnershipMode;
use incan_semantics_core::{CanonicalSymbolId, SemanticSourceTargetKind, SymbolNamespace, SymbolOrigin};

use super::{InputCall, InputCallKind};
use crate::ast::{Receiver, Span};
use crate::ownership_metadata::{AccessMode, CheckedOwnershipContract, SourceSlot, TypeComponent};
use crate::symbols::ResolvedType;
use crate::typechecker::RustCheckedCall;
use crate::typechecker::type_info::{ProtocolArtifacts, RustInteropArtifacts};

/// The admitted owners retained by one native result and the carrier payload that exposes its borrowed value. An
/// empty source set is a rustc-proven static result; `None` from selection means the result holds no native loan.
#[derive(Debug, Clone)]
pub(super) struct NativeResultDependencies {
    pub(super) call: String,
    pub(super) sources: Vec<SourceSlot>,
    pub(super) carried: Option<usize>,
}

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
        // A value receiver is represented as present here; source flow reads or moves it according to its checked
        // Copy capability instead of manufacturing a receiver loan.
        Some(_) => Some(Receiver::Immutable),
        None => return None,
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
    /// Whether a selected native method takes its receiver by value rather than by shared or mutable reference.
    pub(in crate::typechecker) fn native_value_receiver(&self, span: Span) -> bool {
        self.protocols
            .explicit_input_calls
            .get(&span)
            .is_some_and(|call| call.kind == InputCallKind::Native && call.receiver.is_some())
            && self
                .rust
                .checked_calls
                .get(&span)
                .and_then(|call| call.parameters.first())
                .is_some_and(|parameter| !matches!(parameter.ty, ResolvedType::Ref(_) | ResolvedType::RefMut(_)))
    }

    /// Whether the call at `span` is a native associated function written on its type, as in `HashMap.new()`: its
    /// method syntax names the type, not a receiver value, so it is called like a free function.
    pub(in crate::typechecker) fn native_associated_call(&self, span: Span) -> bool {
        self.protocols
            .explicit_input_calls
            .get(&span)
            .is_some_and(|call| call.kind == InputCallKind::Native && call.receiver.is_none())
    }
}

impl crate::typechecker::TypeChecker {
    /// Whether a checked native result has complete signature facts and cannot retain a native loan. This lets an
    /// aggregate such as `Result[usize, Error]` finish a call without borrowing merely because its receiver was lent.
    pub(super) fn native_result_is_independent(&self, span: Span) -> bool {
        self.type_info.rust.checked_calls.get(&span).is_some_and(|call| {
            call.signature_loans.len() == call.parameters.len() + 1
                && !self.type_holds_native_signature_loan(&call.return_type)
        })
    }

    /// Select every caller-owned input whose Rust loan identity occurs in this native call's result. Missing facts
    /// retain the established refusal, while an exact empty result set represents a static lifetime.
    pub(super) fn native_result_dependencies(&self, span: Span) -> Result<Option<NativeResultDependencies>, String> {
        let Some(call) = self.type_info.rust.checked_calls.get(&span) else {
            return Ok(None);
        };
        if !self.type_holds_native_signature_loan(&call.return_type) {
            return Ok(None);
        }
        let expected = call.parameters.len() + 1;
        if call.signature_loans.len() != expected {
            return Err(format!(
                "native call `{}` has no retained result loan contract",
                call.path
            ));
        }
        let result = call
            .signature_loans
            .last()
            .ok_or_else(|| format!("native call `{}` has no retained result loan contract", call.path))?;
        if result.is_empty() {
            return Ok(Some(NativeResultDependencies {
                call: call.path.clone(),
                sources: Vec::new(),
                carried: native_result_payload(self, &call.return_type)?,
            }));
        }
        let first_argument = usize::from(call.has_receiver);
        let mut sources = Vec::new();
        for (index, loans) in call.signature_loans[..call.parameters.len()].iter().enumerate() {
            if !loans.iter().any(|loan| result.contains(loan)) {
                continue;
            }
            let source = if call.has_receiver && index == 0 {
                SourceSlot::Receiver
            } else {
                SourceSlot::Parameter(index - first_argument)
            };
            if !sources.contains(&source) {
                sources.push(source);
            }
        }
        // A method on a native owner with lifetime parameters may return the owner's carried loan rather than the
        // receiver reference's call-bounded loan. Rust's signature establishes that result lifetime, and the source
        // receiver is the Incan value that physically carries it.
        if sources.is_empty()
            && call.has_receiver
            && call
                .parameters
                .first()
                .map(|parameter| match &parameter.ty {
                    ResolvedType::Ref(inner) | ResolvedType::RefMut(inner) => inner.as_ref(),
                    value => value,
                })
                .is_some_and(|receiver| self.type_holds_native_signature_loan(receiver))
        {
            sources.push(SourceSlot::Receiver);
        }
        if sources.is_empty() {
            return Err(format!(
                "native call `{}` returns a borrowed value without an admitted Incan owner",
                call.path
            ));
        }
        Ok(Some(NativeResultDependencies {
            call: call.path.clone(),
            sources,
            carried: native_result_payload(self, &call.return_type)?,
        }))
    }
}

/// Identify the one builtin carrier payload holding a native loan. Direct references and lifetime-bearing native
/// owners use no payload; competing borrowed payloads require a richer variant-specific contract and stay refused.
fn native_result_payload(
    checker: &crate::typechecker::TypeChecker,
    result: &ResolvedType,
) -> Result<Option<usize>, String> {
    let ResolvedType::Generic(_, arguments) = result else {
        return Ok(None);
    };
    if result.is_option() {
        return Ok(arguments
            .first()
            .is_some_and(|value| checker.type_holds_native_signature_loan(value))
            .then_some(0));
    }
    if result.is_result() {
        let borrowed = arguments
            .iter()
            .enumerate()
            .filter_map(|(index, value)| checker.type_holds_native_signature_loan(value).then_some(index))
            .collect::<Vec<_>>();
        return match borrowed.as_slice() {
            [] => Ok(None),
            [payload] => Ok(Some(*payload)),
            _ => Err("native Result carries loans in more than one payload".into()),
        };
    }
    Ok(None)
}
