//! Closed result shapes that source-call projection can classify without retained dependencies.

use crate::ast::{CallArg, Expr, ParamKind, Receiver, Spanned};
use crate::symbols::ResolvedType;
use crate::typechecker::{ResolvedMethodDispatch, TypeCheckInfo};
use incan_lang::lang::builtins::BuiltinMethodId;
use incan_lang::lang::surface::clone_methods::CloneMethodId;
use incan_lang::lang::surface::{list_methods::ListMethodId, option_methods::OptionMethodId};
use incan_lang::lang::traits::{self as core_traits, TraitId};
use incan_lang::lang::types::collections::{self, CollectionTypeId};
use incan_lang::lang::types::ownership::OwnershipMode;
use incan_semantics_core::{CanonicalSymbolId, SemanticSourceTargetKind, SymbolNamespace, SymbolOrigin};

use super::super::{InputCallKind, flow};

/// Select the checked identity for one stored projection.
///
/// Ordinary fields carry their canonical `Field` identity. A newtype's physical `.0` slot deliberately carries the
/// canonical `Newtype` owner used by Body IR, so accept it only when the exact member spelling and the base
/// expression's checked root nominal identity agree. Computed properties never become stored projections.
pub(in crate::typechecker::ownership_contracts) fn stored_projection_identity<'a>(
    facts: &'a TypeCheckInfo,
    expression: &Spanned<Expr>,
) -> Option<&'a CanonicalSymbolId> {
    let Expr::Field(base, member) = &expression.node else {
        return None;
    };
    if facts.computed_property_access(expression.span).is_some() {
        return None;
    }
    let identity = facts.resolved_identity(expression.span)?;
    if identity.kind == SemanticSourceTargetKind::Field {
        return Some(identity);
    }
    if member != "0"
        || identity.namespace != SymbolNamespace::OrdinaryLexical
        || identity.kind != SemanticSourceTargetKind::Newtype
        || facts
            .expressions
            .expression_type_identities
            .get(&base.span)
            .and_then(|identities| identities.get(&Vec::new()))
            != Some(identity)
    {
        return None;
    }
    Some(identity)
}

/// Authenticate a direct method result before its checked newtype `.0` slot is treated as a fresh value. Ordinary
/// zero-parameter methods preserve the existing independent-source policy. The one parameterized exception is the
/// exact canonical standard-library `Path.joinpath` surface already admitted by source-call construction.
fn direct_method_newtype_result(facts: &TypeCheckInfo, base: &Spanned<Expr>, slot: &CanonicalSymbolId) -> bool {
    let Expr::MethodCall(receiver, _, _, arguments) = &base.node else {
        return false;
    };
    let Some(call) = facts.protocols.explicit_input_calls.get(&base.span) else {
        return false;
    };
    if call.kind != InputCallKind::Ordinary
        || call.owner.is_none()
        || call.receiver != Some(Receiver::Immutable)
        || facts.resolved_identity(base.span) != Some(&call.callee)
        || !facts
            .expressions
            .expression_type_identities
            .get(&receiver.span)
            .is_some_and(|identities| identities.values().any(|identity| identity == slot))
    {
        return false;
    }
    if arguments.is_empty() && call.parameters.is_empty() {
        return true;
    }
    let [CallArg::Positional(_)] = arguments.as_slice() else {
        return false;
    };
    let [parameter] = call.parameters.as_slice() else {
        return false;
    };
    call.callee.namespace == SymbolNamespace::Member
        && call.callee.origin
            == SymbolOrigin::Package {
                library: "incan_stdlib_system".to_string(),
                module_path: vec!["fs".to_string(), "path".to_string()],
            }
        && call.callee.declaration_name == "joinpath"
        && call.callee.kind == SemanticSourceTargetKind::Method
        && facts.resolved_builtin_method_call(base.span).is_none()
        && facts.call_site_callable_params(base.span) == Some(call.parameters.as_slice())
        && parameter.kind == ParamKind::Normal
        && !parameter.is_mut
        && parameter.ty == ResolvedType::Str
        && slot.declaration_name == "Path"
        && matches!(&slot.origin,
            SymbolOrigin::Package { library, module_path }
                if library == "incan_stdlib_system"
                    && module_path.iter().map(String::as_str).eq(["fs", "path"]))
}

/// Select one source-less stored projection directly on an authenticated fresh call result. Classification proves
/// the call's independence separately; source evaluation and replay share this exact shape so neither can reinterpret
/// a nested, indexed, `?` or computed expression. Method results remain restricted to the existing zero-parameter
/// policy and exact canonical `Path.joinpath`.
pub(in crate::typechecker::ownership_contracts) fn direct_call_field_projection<'a>(
    facts: &TypeCheckInfo,
    expression: &'a Spanned<Expr>,
) -> Option<&'a Spanned<Expr>> {
    let Expr::Field(base, _) = &expression.node else {
        return None;
    };
    let projection = stored_projection_identity(facts, expression)?;
    match &base.node {
        Expr::Call(..) => Some(base),
        Expr::MethodCall(..) if direct_method_newtype_result(facts, base, projection) => Some(base),
        _ => None,
    }
}

/// This initial call profile admits only closed scalar/owned byte-string results and builtin `Result`/`Option`
/// carriers of them, with no possible retained source dependency. Other aggregates, references, native and generic
/// results await their checked dependency representation.
pub(super) fn plain_result(ty: &ResolvedType) -> bool {
    match ty {
        ResolvedType::Unit
        | ResolvedType::Int
        | ResolvedType::Float
        | ResolvedType::Numeric(_)
        | ResolvedType::Bool
        | ResolvedType::Str
        | ResolvedType::Bytes
        | ResolvedType::FrozenStr
        | ResolvedType::FrozenBytes => true,
        ResolvedType::Generic(_, arguments) if ty.is_result() || ty.is_option() => {
            !arguments.is_empty() && arguments.iter().all(plain_result)
        }
        _ => false,
    }
}

/// Retain an unresolved scalar-carrier obligation without declaring any named leaf independent. Legacy checking
/// can represent an enclosing binder as Named; Body IR classifies it using the exact declaration's binder inventory.
/// A real nominal leaf stays nominal and refuses closed admission until its dependency proof is supported.
pub(in crate::typechecker::ownership_contracts) fn conditional_result(ty: &ResolvedType) -> bool {
    /// Check carrier leaves while retaining whether any unresolved binder prevents a final independence proof.
    fn leaves(ty: &ResolvedType, generic: &mut bool) -> bool {
        match ty {
            ResolvedType::TypeVar(_) | ResolvedType::Named(_) => {
                *generic = true;
                true
            }
            ResolvedType::Generic(_, arguments) if ty.is_result() || ty.is_option() => {
                !arguments.is_empty() && arguments.iter().all(|item| leaves(item, generic))
            }
            ResolvedType::Generic(name, arguments)
                if collections::from_str(name) == Some(CollectionTypeId::List) && arguments.len() == 1 =>
            {
                *generic = true;
                arguments.iter().all(|item| leaves(item, generic))
            }
            _ => plain_result(ty),
        }
    }
    let mut generic = false;
    (ty.is_result() || ty.is_option()) && leaves(ty, &mut generic) && generic
}

/// The exact checked source surface shared by source admission and graph replay for one direct `Result.map_err`.
/// Live borrow, native-loan and capture facts deliberately remain source-only; a completed graph replays the exact
/// current-source marker that those checks authorized instead of publishing new `TypeCheckInfo` accessors.
#[derive(Debug, Clone, PartialEq)]
pub(in crate::typechecker::ownership_contracts) struct SelectedResultMapErr {
    pub callback: crate::ast::Span,
    pub receiver: crate::ast::Span,
    pub result: ResolvedType,
    pub inner_kind: InputCallKind,
    pub inner_callee: CanonicalSymbolId,
    pub inner_receiver: Option<crate::ast::Span>,
    pub outer_callee: CanonicalSymbolId,
}

/// Select the exact facts-only source surface for a direct `Result` producer accepted by builtin `Result.map_err`:
/// the established closed Ordinary route or the new strictly retained Explicit route.
pub(in crate::typechecker::ownership_contracts) fn selected_result_map_err_surface(
    facts: &TypeCheckInfo,
    owner: &CanonicalSymbolId,
    receiver: &Spanned<Expr>,
    invocation: crate::ast::Span,
    arguments: &[crate::ast::CallArg],
) -> Option<SelectedResultMapErr> {
    let outer = facts.protocols.explicit_input_calls.get(&invocation)?;
    if outer.kind != InputCallKind::Ordinary
        || outer.owner.as_ref() != Some(owner)
        || outer.callee != crate::symbols::canonical_builtin_member_identity("Result", "map_err")
        || outer.receiver != Some(crate::ast::Receiver::Immutable)
        || facts.selected_input_contract(outer).ok() != Some(None)
        || facts.resolved_identity(invocation) != Some(&outer.callee)
        || facts.resolved_builtin_method_call(invocation)
            != Some(BuiltinMethodId::Result(
                incan_lang::lang::surface::result_methods::ResultMethodId::MapErr,
            ))
    {
        return None;
    }
    let [crate::ast::CallArg::Positional(callback) | crate::ast::CallArg::Named(_, callback)] = arguments else {
        return None;
    };
    let [parameter] = outer.parameters.as_slice() else {
        return None;
    };
    if !matches!(&parameter.ty, ResolvedType::Function(parameters, _) if parameters.len() == 1)
        || !matches!(callback.node, Expr::Closure(..))
        || facts.expr_type(callback.span) != Some(&parameter.ty)
    {
        return None;
    }
    let inner = facts.protocols.explicit_input_calls.get(&receiver.span)?;
    let contract = facts.selected_input_contract(inner).ok()?;
    let inner_receiver = match &receiver.node {
        Expr::Call(..) if inner.receiver.is_none() => None,
        Expr::MethodCall(method_receiver, ..)
            if inner.kind == InputCallKind::Ordinary
                && inner.receiver == Some(crate::ast::Receiver::Immutable)
                && facts.resolved_identity(receiver.span) == Some(&inner.callee) =>
        {
            Some(method_receiver.span)
        }
        _ => return None,
    };
    let selected = match inner.kind {
        InputCallKind::Ordinary => contract.is_none(),
        InputCallKind::Explicit => contract.is_some_and(|contract| {
            contract.result_source.is_none()
                && contract.modes.iter().all(|mode| {
                    mode.component.parameter.is_some()
                        && mode.component.children.is_empty()
                        && mode.mode != OwnershipMode::Reference
                })
                && inner
                    .parameters
                    .iter()
                    .all(|parameter| parameter.kind == crate::ast::ParamKind::Normal && !parameter.is_partial_preset)
                && crate::ownership_metadata::returned_view(Some(contract)).is_ok_and(|view| view.is_none())
                && crate::ownership_metadata::owned_result(Some(contract)).is_ok_and(|owned| owned.is_none())
        }),
        _ => false,
    };
    if !selected
        || inner.owner.as_ref() != Some(owner)
        || !matches!(
            inner.callee.origin,
            incan_semantics_core::SymbolOrigin::Module(_) | incan_semantics_core::SymbolOrigin::Package { .. }
        )
    {
        return None;
    }
    let result = facts.expr_type(receiver.span)?;
    if !matches!(result, ResolvedType::Generic(name, arguments)
        if collections::from_str(name) == Some(CollectionTypeId::Result) && arguments.len() == 2)
        || (inner.kind == InputCallKind::Explicit && !conditional_result(result))
        || checked_clone_call(facts, receiver.span)
    {
        return None;
    }
    Some(SelectedResultMapErr {
        callback: callback.span,
        receiver: receiver.span,
        result: result.clone(),
        inner_kind: inner.kind,
        inner_callee: inner.callee.clone(),
        inner_receiver,
        outer_callee: outer.callee.clone(),
    })
}

/// Reauthenticate one source-authorized `Result.map_err` route against its exact completed current-source graph.
/// `ResultMapErrUse` is intentionally not serialized; its presence proves the live TypeChecker admission ran.
pub(in crate::typechecker::ownership_contracts) fn retained_result_map_err(
    facts: &TypeCheckInfo,
    owner: &CanonicalSymbolId,
    receiver: &Spanned<Expr>,
    invocation: crate::ast::Span,
    arguments: &[crate::ast::CallArg],
    graph: &flow::Graph,
) -> Option<crate::ast::Span> {
    let selected = selected_result_map_err_surface(facts, owner, receiver, invocation, arguments)?;
    let value_use = graph.value_uses.get(&selected.receiver)?;
    if value_use.kind != flow::ValueUseKind::CallResult
        || !value_use.accesses.is_empty()
        || value_use.value_type != selected.result
        || value_use.result_map_err
            != Some(flow::ResultMapErrUse {
                invocation,
                receiver: selected.receiver,
                callback: selected.callback,
            })
    {
        return None;
    }
    let inner = graph.invocations.get(&selected.receiver)?;
    let expected = (selected.inner_kind == InputCallKind::Explicit).then_some(&selected.result);
    if inner.callee != selected.inner_callee || inner.result_requirement.as_ref() != expected {
        return None;
    }
    match (selected.inner_receiver, inner.receiver) {
        (None, None) => {}
        (Some(receiver), Some(loan)) => {
            let loan = graph.loans.get(loan.0)?;
            if loan.kind != flow::LoanKind::Shared || loan.writable || loan.span != receiver {
                return None;
            }
        }
        _ => return None,
    }
    let outer = graph.invocations.get(&invocation)?;
    let [callback] = outer.arguments.as_slice() else {
        return None;
    };
    if outer.callee != selected.outer_callee
        || outer.receiver_owned
        || outer.receiver_source.is_some()
        || !outer.receiver_dependencies.is_empty()
        || callback.span != selected.callback
        || callback.slot != 0
        || callback.written_position != 0
        || callback.access != incan_semantics_core::IncanInputAccess::Inferred
        || callback.source.is_some()
        || callback.loan.is_some()
    {
        return None;
    }
    let loan = graph.loans.get(outer.receiver?.0)?;
    if loan.kind != flow::LoanKind::Shared
        || loan.writable
        || loan.parent.is_some()
        || loan.span != selected.receiver
        || !loan.source.projection.is_empty()
        || !matches!(&loan.source.root,
            flow::Root::Temporary { owner: root_owner, expression, invocation: outer_invocation }
                if root_owner == owner && *expression == selected.receiver && *outer_invocation == invocation)
    {
        return None;
    }
    Some(selected.callback)
}

/// Match the semantic `Clone.clone` selection without trusting the method spelling.
fn checked_clone_call(facts: &TypeCheckInfo, span: crate::ast::Span) -> bool {
    let builtin = crate::symbols::canonical_builtin_member_identity(
        core_traits::as_str(TraitId::Clone),
        incan_lang::lang::trait_bounds::rust::CLONE_METHOD,
    );
    if matches!(
        facts.resolved_builtin_method_call(span),
        Some(
            BuiltinMethodId::Clone(CloneMethodId::Clone)
                | BuiltinMethodId::List(ListMethodId::Clone)
                | BuiltinMethodId::Option(OptionMethodId::Clone)
        )
    ) || facts.traits.inferred_method_identities.get(&span) == Some(&builtin)
    {
        return true;
    }
    facts.resolved_method_call(span).is_some_and(|call| {
        call.method == incan_lang::lang::trait_bounds::rust::CLONE_METHOD
            && matches!(
                &call.dispatch,
                ResolvedMethodDispatch::Trait { trait_name, .. }
                    if core_traits::from_str(trait_name) == Some(TraitId::Clone)
            )
    })
}

/// Printing reaches no nominal display hook for primitive values and for lists whose items print the same way.
pub(super) fn primitive_display(ty: &ResolvedType) -> bool {
    match ty {
        ResolvedType::Str
        | ResolvedType::FrozenStr
        | ResolvedType::Int
        | ResolvedType::Float
        | ResolvedType::Bool
        | ResolvedType::Unit => true,
        ResolvedType::Generic(name, arguments) => {
            collections::from_str(name) == Some(CollectionTypeId::List)
                && matches!(arguments.as_slice(), [item] if primitive_display(item))
        }
        _ => false,
    }
}
