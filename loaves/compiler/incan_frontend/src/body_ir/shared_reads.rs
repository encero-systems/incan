//! Pass ordinary immutable parameters through shared storage, then publish that decision in the callable signature
//! before the executable ABI is consumed. Owned snapshots, mutable uses and escaping values materialize at the
//! operation that requires independent storage. A parameter its body consumes on any path keeps its by-value input, so
//! a caller's last use still moves it.

use crate::body_ir::ContextModules;
use std::collections::{HashMap, HashSet};

use super::{CanonicalSymbolId, IncanPrimitiveType, IncanType, bir};
use incan_lang::lang::{builtins::BuiltinFnId, types::collections::CollectionTypeId};

mod closure_reads;
mod consumed_reads;
mod cursor_reads;
mod derived_reads;
mod ended_loans;
mod loop_reads;
mod match_reads;
mod published_reads;
mod string_parameter_reads;

use derived_reads::{derived_temporaries_are_read_only, retain_shared_temporaries, temporary_is_read_only};
use match_reads::{match_bindings_are_read_only, pattern_is_read_only, pattern_selects_shared_carrier};
use string_parameter_reads::retain_shared_call_temporaries;

type Candidate = (usize, usize, usize);

/// One canonical callable's already checked closed storage signature.
struct Signature {
    identity: CanonicalSymbolId,
    generic: bool,
    arguments: Vec<IncanType>,
    parameters: Vec<SignatureParameter>,
}

/// Canonical callable signatures grouped once by identity so each fixed-point call check visits only matching
/// declarations. Multiple entries remain visible because an ambiguous exact or generic signature must still refuse.
struct Signatures {
    by_identity: HashMap<CanonicalSymbolId, Vec<Signature>>,
}

/// One canonical field's declared type template and any arguments that close a nongeneric runtime surface.
struct FieldType {
    parameters: Vec<String>,
    native_arguments: Vec<IncanType>,
    ty: IncanType,
}

/// Checked field types grouped by canonical member identity. Repeated inherited layouts remain visible so a
/// conflicting projection refuses instead of choosing whichever declaration was collected first.
struct FieldTypes {
    by_identity: HashMap<CanonicalSymbolId, Vec<FieldType>>,
}

impl FieldTypes {
    /// Collect the exact field contracts available to the bodies participating in this analysis.
    fn collect(modules: &[bir::BodyIrModule], context: ContextModules<'_>) -> Self {
        let mut by_identity: HashMap<CanonicalSymbolId, Vec<FieldType>> = HashMap::new();
        for declaration in context
            .iter()
            .chain(modules)
            .flat_map(|module| &module.nominal_declarations)
        {
            for (identity, ty) in declaration.field_identities.iter().zip(&declaration.field_types) {
                by_identity.entry(identity.clone()).or_default().push(FieldType {
                    parameters: declaration.type_parameters.clone(),
                    native_arguments: declaration.native_type_arguments.clone(),
                    ty: ty.clone(),
                });
            }
        }
        Self { by_identity }
    }

    /// Close one selected field against its checked owner type, declining missing or conflicting retained layouts.
    fn projected(&self, identity: &CanonicalSymbolId, owner: &IncanType) -> Option<IncanType> {
        let generic_arguments = match owner {
            IncanType::Generic { args, .. } => Some(args.as_slice()),
            IncanType::Named(_) => None,
            _ => return None,
        };
        let mut resolved = self.by_identity.get(identity)?.iter().filter_map(|field| {
            let arguments = generic_arguments.unwrap_or(&field.native_arguments);
            (field.parameters.len() == arguments.len())
                .then(|| super::enum_types::close_payload_type(&field.ty, &field.parameters, arguments))
        });
        let selected = resolved.next()?;
        resolved.all(|other| other == selected).then_some(selected)
    }
}

impl FromIterator<Signature> for Signatures {
    /// Build the read-only lookup without choosing between potentially conflicting declarations.
    fn from_iter<T: IntoIterator<Item = Signature>>(signatures: T) -> Self {
        let mut by_identity: HashMap<CanonicalSymbolId, Vec<Signature>> = HashMap::new();
        for signature in signatures {
            by_identity
                .entry(signature.identity.clone())
                .or_default()
                .push(signature);
        }
        Self { by_identity }
    }
}

/// One callable input's value type and its single checked access decision.
#[derive(Clone, PartialEq)]
struct SignatureParameter {
    ty: IncanType,
    access: incan_semantics_core::IncanInputAccess,
    candidate: Option<Candidate>,
    callback_does_not_escape: bool,
    /// Whether the callable's result may hold a loan of this input: a declared returned view or result dependency
    /// names it, the callable is deferred, or no checked frame says otherwise.
    result_holds_loan: bool,
}

impl SignatureParameter {
    /// Resolve this slot's current fixed-point hypothesis without rebuilding its containing signature.
    fn access(&self, candidates: &HashSet<Candidate>) -> incan_semantics_core::IncanInputAccess {
        if self.candidate.is_some_and(|candidate| candidates.contains(&candidate)) {
            incan_semantics_core::IncanInputAccess::InferredShared
        } else {
            self.access
        }
    }

    /// Return the underlying owner when this slot already receives shared access for fixed-point analysis.
    fn shared_owner(&self, candidates: &HashSet<Candidate>) -> Option<&IncanType> {
        match &self.ty {
            IncanType::Ref(owner) => Some(owner),
            owner if self.access(candidates).is_shared() => Some(owner),
            _ => None,
        }
    }
}

/// A read-only proof is local to one frame and never grants an escaping or exclusive loan.
#[derive(Clone, Copy)]
struct ReadContext<'a> {
    locals: &'a [bir::LocalDecl],
    signatures: &'a Signatures,
    field_types: &'a FieldTypes,
    candidates: &'a HashSet<Candidate>,
    local: bir::LocalId,
    cursor: bool,
    derived: bool,
}

/// Retain native cursors, pass ordinary immutable parameters through shared storage and rewrite every caller from the
/// resulting signatures. A function-item closure remains the declared by-value callable adapter: only its internal
/// call lends the wrapper's owned argument to the inferred-shared target.
pub(super) fn retain_modules(modules: &mut [bir::BodyIrModule], context: ContextModules<'_>) {
    retain_modules_with_selected(modules, context, &HashSet::new());
}

/// Retain shared-read representation for fresh owners while selected final bodies remain immutable context.
pub(super) fn retain_modules_with_selected(
    modules: &mut [bir::BodyIrModule],
    context: ContextModules<'_>,
    selected: &HashSet<CanonicalSymbolId>,
) {
    let no_candidates = HashSet::new();
    let field_types = FieldTypes::collect(modules, context);
    let cursor_signatures = collect_signatures(modules, context, &no_candidates);
    for module in modules.iter_mut() {
        for body in &mut module.bodies {
            if super::body_is_selected(body, selected) {
                continue;
            }
            cursor_reads::retain_cursors(body, &cursor_signatures, &field_types, &no_candidates);
        }
    }

    // Every callable follows the never-consumed rule; a published one also keeps each input the base's read-only rule
    // lends under the base's own hypothesis, so no published input that was shared before becomes owned.
    let base = candidate_parameters(modules, selected, InputRule::Base);
    let published = published_reads::read_only_parameters(modules, context, &field_types, base)
        .into_iter()
        .filter(|&(module_index, body_index, _)| published_body(modules, module_index, body_index))
        .collect::<HashSet<_>>();
    let mut candidates = candidate_parameters(modules, selected, InputRule::NeverConsumed);
    candidates.extend(published.iter().copied());
    consumed_reads::refuse_consumed_parameters(modules, context, &field_types, &published, &mut candidates);

    let signatures = collect_signatures(modules, context, &candidates);
    for (module_index, body_index, parameter_index) in &candidates {
        if let Some(body) = modules
            .get_mut(*module_index)
            .and_then(|module| module.bodies.get_mut(*body_index))
        {
            retain_shared_parameter(body, *parameter_index, &signatures, &field_types, &candidates);
        }
    }
    let final_signatures = collect_signatures(modules, context, &no_candidates);
    for module in modules {
        for body in &mut module.bodies {
            if super::body_is_selected(body, selected) {
                continue;
            }
            match_reads::retain_borrowed_payload_bindings(body, &final_signatures, &field_types, &no_candidates);
            loop_reads::retain_borrowed_loop_items(body, &final_signatures, &field_types, &no_candidates);
            retain_body_calls(body, &final_signatures, &field_types, &no_candidates);
        }
    }
}

/// Publish one inferred decision in both the executable slot and any already admitted checked-input profile. The
/// profile remains the authority for explicit inputs and returned views, so its complete slot inventory must not
/// drift merely because this later whole-body analysis refined an ordinary input.
fn retain_shared_parameter(
    body: &mut bir::Body,
    parameter_index: usize,
    signatures: &Signatures,
    field_types: &FieldTypes,
    candidates: &HashSet<Candidate>,
) {
    let Some(local) = body.params.get(parameter_index).map(|parameter| parameter.local) else {
        return;
    };
    retain_shared_temporaries(body, local, signatures, field_types, candidates);
    let Some(parameter) = body.params.get_mut(parameter_index) else {
        return;
    };
    parameter.input_access = incan_semantics_core::IncanInputAccess::InferredShared;
    if let Some(retained) = body
        .checked_input_profile
        .as_mut()
        .and_then(|profile| profile.parameters.get_mut(parameter_index))
        && retained.slot == parameter_index
    {
        retained.access = incan_semantics_core::IncanInputAccess::InferredShared;
    }
}

/// Select every ordinary non-Copy input whose source contract leaves access inferred, under `rule`. The base rule
/// selects every eligible callable's inputs for the read-only fixed point (`published_reads`); the never-consumed rule
/// selects them too, published or not, since an explicit ownership contract is the only override. Such a frame receives
/// shared storage because the binding is immutable; a move, store, return or owned call inside it materializes the
/// independent value at that operation, unless the body consumes the input on any path (`consumed_reads`). Open trait
/// slots and their checked implementations keep the trait's authored ABI because an implementation body cannot refine
/// that contract independently. A method targeted by a partial also keeps its authored ABI because the retained partial
/// frame has already copied that parameter surface.
fn candidate_parameters(
    modules: &[bir::BodyIrModule],
    selected: &HashSet<CanonicalSymbolId>,
    rule: InputRule,
) -> HashSet<Candidate> {
    let mut candidates = HashSet::new();
    let copy_nominals = copy_nominal_identities(modules);
    for (module_index, module) in modules.iter().enumerate() {
        let trait_implementations = module
            .trait_implementations
            .iter()
            .map(|implementation| implementation.implementation.clone())
            .collect::<HashSet<_>>();
        let method_partial_targets = module
            .nominal_declarations
            .iter()
            .flat_map(|declaration| &declaration.method_partial_defaults)
            .map(|partial| partial.target.clone())
            .collect::<HashSet<_>>();
        for (body_index, body) in module.bodies.iter().enumerate() {
            if super::body_is_selected(body, selected) {
                continue;
            }
            let eligible = body.type_parameters.is_empty()
                && !body.is_async
                && !body.is_generator()
                && body.native_attributes.is_empty()
                && body.canonical.as_ref().is_none_or(|identity| {
                    !trait_implementations.contains(identity) && !method_partial_targets.contains(identity)
                })
                && !has_open_trait_receiver(body);
            if !eligible {
                continue;
            }
            for (parameter_index, parameter) in body.params.iter().enumerate() {
                if parameter_is_candidate(body, parameter, rule, &copy_nominals) {
                    candidates.insert((module_index, body_index, parameter_index));
                }
            }
        }
    }
    candidates
}

/// The models, classes, newtypes and enums these modules declare whose checked derives include `Copy`.
fn copy_nominal_identities(modules: &[bir::BodyIrModule]) -> HashSet<CanonicalSymbolId> {
    modules
        .iter()
        .flat_map(|module| {
            let nominals = module
                .nominal_declarations
                .iter()
                .filter(|declaration| super::places_and_ownership::derives_copy(&declaration.derives))
                .map(|declaration| declaration.canonical.clone());
            let enums = module
                .enum_declarations
                .iter()
                .filter(|declaration| super::places_and_ownership::derives_copy(&declaration.derives))
                .map(|declaration| declaration.canonical.clone());
            nominals.chain(enums)
        })
        .collect()
}

/// Whether `ty` names one of `copy_nominals` through the body's retained identity, never its spelling. Such a value is
/// copied in as a scalar is, so lending it would only turn each owned use into a clone of the referent.
fn is_copy_nominal(body: &bir::Body, ty: &IncanType, copy_nominals: &HashSet<CanonicalSymbolId>) -> bool {
    matches!(ty, IncanType::Named(name) if body
        .named_type_identities
        .get(name)
        .is_some_and(|identity| copy_nominals.contains(identity)))
}

/// Whether this is a trait-owned default body whose receiver is still the open `Self` contract. Abstract trait
/// methods have no body; concrete adopters are excluded separately through the checked implementation registry.
fn has_open_trait_receiver(body: &bir::Body) -> bool {
    body.params.first().is_some_and(|parameter| {
        parameter.ty == IncanType::SelfType
            && body
                .locals
                .get(parameter.local.index())
                .is_some_and(|local| matches!(local.origin, bir::LocalOrigin::Receiver { .. }))
    })
}

/// Which rule selects an input for shared access: the base's read-only rule, whose lent inputs a published callable
/// also keeps, or the never-consumed rule every callable follows.
#[derive(Clone, Copy, PartialEq)]
enum InputRule {
    Base,
    NeverConsumed,
}

/// Whether this body's declaration is published (`pub`): another module, package or Rust caller can name it, so its
/// parameter passing is part of its signature.
pub(super) fn published_body(modules: &[bir::BodyIrModule], module_index: usize, body_index: usize) -> bool {
    modules.get(module_index).is_some_and(|module| {
        module
            .bodies
            .get(body_index)
            .and_then(|body| body.canonical.as_ref())
            .is_some_and(|identity| module.published_callables.contains(identity))
    })
}

/// Keep receivers, rest packs, defaults, explicit modes, mutable inputs and Copy values (scalars, and the nominals in
/// `copy_nominals`) on their established passing rules. Under the base rule every other type except an explicit
/// reference carrier, callable storage and an open type parameter may borrow. Under the never-consumed rule only a
/// checked type that lowering gives a shared executable form (`IncanType::has_shared_executable_form`) may, because
/// lowering materializes its owned uses; every other type keeps its by-value input, including a generator the callee
/// advances. Function-item adapters may borrow inside their forwarding body, but callable-valued parameters keep the
/// by-value closure carrier that lowering supports. Defaults need a retained invocation owner before they can publish a
/// borrowed parameter contract.
fn parameter_is_candidate(
    body: &bir::Body,
    parameter: &bir::CallableParam,
    rule: InputRule,
    copy_nominals: &HashSet<CanonicalSymbolId>,
) -> bool {
    let shareable = match rule {
        InputRule::Base => !matches!(
            parameter.ty,
            IncanType::Ref(_)
                | IncanType::RefMut(_)
                | IncanType::Function { .. }
                | IncanType::TypeVar(_)
                | IncanType::SelfType
        ),
        InputRule::NeverConsumed => parameter.ty.has_shared_executable_form(),
    };
    parameter.input_access == incan_semantics_core::IncanInputAccess::Inferred
        && parameter.kind == incan_semantics_core::IncanCallableParamKind::Normal
        && body
            .params
            .iter()
            .all(|input| input.kind == incan_semantics_core::IncanCallableParamKind::Normal)
        && matches!(parameter.default, bir::CallableParamDefault::Required)
        && !parameter.mutable
        && !parameter.native_owned
        && !parameter.ty.abi_v0_facts().ownership.is_trivially_copy()
        && !is_copy_nominal(body, &parameter.ty, copy_nominals)
        && shareable
        && !body
            .locals
            .get(parameter.local.index())
            .is_some_and(|local| matches!(local.origin, bir::LocalOrigin::Receiver { .. }))
}

/// Finalize only this body's call-loan operands from selected final signatures, including its lexical closures and
/// generators. Callee signature production and shared-loan group settlement have already finished.
fn retain_body_calls(
    body: &mut bir::Body,
    signatures: &Signatures,
    field_types: &FieldTypes,
    candidates: &HashSet<Candidate>,
) {
    closure_reads::retain(body, signatures, field_types, candidates);
    retain_shared_call_temporaries(body, signatures, field_types, candidates);
    let locals = &body.locals;
    let mut visited = 0;
    let mut ended = Vec::new();
    walk_calls_mut(&mut body.block.stmts, &mut |statement| {
        retain_function_item_adapter(statement, signatures, candidates);
        for local in retain_call_loans(statement, locals, signatures, field_types, candidates) {
            ended.push((visited, local));
        }
        visited += 1;
    });
    ended_loans::drop_after_last_loans(body, signatures, candidates, &ended);
}

/// Keep a named function's declared by-value callable shape by lowering its forwarding closure as an adapter when
/// the selected body acquired inferred shared access. Direct function-item lowering is valid only when both input
/// contracts agree; the adapter owns its callable argument and lends it only for the internal call.
fn retain_function_item_adapter(
    statement: &mut bir::Statement,
    signatures: &Signatures,
    candidates: &HashSet<Candidate>,
) {
    let bir::StatementKind::Assign {
        rvalue: bir::Rvalue::Closure { params, body, .. },
        ..
    } = &mut statement.kind
    else {
        return;
    };
    if !body.function_item {
        return;
    }
    let [
        bir::Statement {
            kind: bir::StatementKind::Call { callee, args, .. },
            ..
        },
    ] = body.stmts.as_slice()
    else {
        return;
    };
    let adapts_inferred_shared = args.iter().enumerate().any(|(index, _)| {
        call_parameter(callee, index, signatures, candidates).is_some_and(|parameter| {
            parameter.access(candidates) == incan_semantics_core::IncanInputAccess::InferredShared
                && params
                    .get(index)
                    .is_some_and(|adapter| adapter.input_access != parameter.access)
        })
    });
    if adapts_inferred_shared {
        body.function_item = false;
        // The adapter takes the whole callable signature and forwards every input, so no input is left to a default.
        for parameter in params.iter_mut() {
            parameter.default = bir::CallableParamDefault::Required;
        }
    }
}

/// Combine linked, previously settled module and current-body signatures without deriving a callable from its name.
fn collect_signatures(
    modules: &[bir::BodyIrModule],
    context: ContextModules<'_>,
    candidates: &HashSet<Candidate>,
) -> Signatures {
    context
        .iter()
        .flat_map(module_signatures)
        .chain(modules.iter().enumerate().flat_map(|(module_index, module)| {
            module.loaf_callables.iter().map(loaf_signature).chain(
                module
                    .bodies
                    .iter()
                    .enumerate()
                    .filter_map(move |(body_index, body)| body_signature(body, module_index, body_index, candidates)),
            )
        }))
        .collect()
}

/// Read already settled signatures from a dependency module.
fn module_signatures(module: &bir::BodyIrModule) -> impl Iterator<Item = Signature> + '_ {
    module.loaf_callables.iter().map(loaf_signature).chain(
        module
            .bodies
            .iter()
            .enumerate()
            .filter_map(|(body_index, body)| body_signature(body, usize::MAX, body_index, &HashSet::new())),
    )
}

/// Prefer an authenticated declaration frame's access facts; a native callable without one retains its reference
/// spelling as the shared/exclusive authority it already had.
fn loaf_signature(callable: &bir::LoafCallable) -> Signature {
    let parameters = callable
        .declaration_frame()
        .filter(|frame| frame.params.len() == callable.parameters.len())
        .map(|frame| {
            frame
                .params
                .iter()
                .enumerate()
                .map(|(index, parameter)| SignatureParameter {
                    ty: callable.parameters[index].clone(),
                    access: parameter.input_access,
                    candidate: None,
                    callback_does_not_escape: only_called_parameter(frame, parameter),
                    result_holds_loan: result_holds_loan(frame, index),
                })
                .collect()
        })
        .unwrap_or_else(|| {
            callable
                .parameters
                .iter()
                .map(|ty| SignatureParameter {
                    ty: ty.clone(),
                    access: incan_semantics_core::IncanInputAccess::Inferred,
                    candidate: None,
                    callback_does_not_escape: false,
                    result_holds_loan: true,
                })
                .collect()
        });
    Signature {
        identity: callable.canonical.clone(),
        generic: !callable.type_parameters.is_empty(),
        arguments: callable.native_type_arguments.clone(),
        parameters,
    }
}

/// Project one body's parameter access independently from its executable statements. Receivers retain the same
/// shared/exclusive representation as `LoafCallable::from_body`; anonymous frames supply no named signature.
/// Candidate inputs are shared only for this fixed-point hypothesis.
fn body_signature(
    body: &bir::Body,
    module_index: usize,
    body_index: usize,
    candidates: &HashSet<Candidate>,
) -> Option<Signature> {
    Some(Signature {
        identity: body.canonical.clone()?,
        generic: !body.type_parameters.is_empty(),
        arguments: Vec::new(),
        parameters: body
            .params
            .iter()
            .enumerate()
            .map(|(parameter_index, parameter)| SignatureParameter {
                ty: match body.locals.get(parameter.local.index()).map(|local| &local.origin) {
                    Some(bir::LocalOrigin::Receiver { mutable: false }) => {
                        IncanType::Ref(Box::new(parameter.ty.clone()))
                    }
                    Some(bir::LocalOrigin::Receiver { mutable: true }) => {
                        IncanType::RefMut(Box::new(parameter.ty.clone()))
                    }
                    _ => parameter.ty.clone(),
                },
                access: parameter.input_access,
                candidate: candidates
                    .contains(&(module_index, body_index, parameter_index))
                    .then_some((module_index, body_index, parameter_index)),
                callback_does_not_escape: only_called_parameter(body, parameter),
                result_holds_loan: result_holds_loan(body, parameter_index),
            })
            .collect(),
    })
}

/// Whether a callable's result may hold a loan of its input `index`. A returned view and an owned result's dependency
/// are declared, and each names its one source input (a method's receiver is input 0), so every other input of a
/// plain callable is free again when the call returns. A deferred callable's result holds its inputs until it runs,
/// and a generic one's result obligations are only settled per instantiation.
fn result_holds_loan(body: &bir::Body, index: usize) -> bool {
    body.is_async
        || body.is_generator()
        || !body.type_parameters.is_empty()
        || body.returned_view.as_ref().is_some_and(|view| view.slot == index)
        || body
            .checked_input_profile
            .as_ref()
            .is_some_and(|profile| !profile.result_requirements.is_empty())
}

/// Concrete function-typed slots retain the checker's `body_only_calls` proof. Generic closure-storage slots do not
/// establish escape behavior, so instantiations and asynchronous/deferred bodies supply no additional authority.
fn only_called_parameter(body: &bir::Body, parameter: &bir::CallableParam) -> bool {
    body.type_parameters.is_empty()
        && body.nominal_instantiation.is_empty()
        && body.trait_default_instantiation.is_none()
        && !body.is_async
        && !body.is_generator()
        && matches!(parameter.ty, IncanType::Function { .. })
        && body
            .callable_representation
            .as_ref()
            .is_some_and(|representation| representation.closure_holding_parameters.contains(&parameter.local))
}

/// Read the checked type of a whole reference binding; projected initialization keeps its checked destination type.
fn whole_type<'a>(place: &bir::Place, locals: &'a [bir::LocalDecl]) -> Option<&'a IncanType> {
    if !place.projection.is_empty() {
        return None;
    }
    locals.get(place.local_id()?.index()).map(|local| &local.ty)
}

/// Recover the exact checked type read from a local or global place through retained nominal, collection and payload
/// projections. An index is admitted only where the checked container itself determines one element type.
fn place_type(place: &bir::Place, locals: &[bir::LocalDecl], field_types: &FieldTypes) -> Option<IncanType> {
    let mut ty = match &place.root {
        bir::PlaceRoot::Local(local) => locals.get(local.index())?.ty.clone(),
        bir::PlaceRoot::Global(global) => global.ty.clone(),
    };
    for projection in &place.projection {
        while let IncanType::Ref(inner) | IncanType::RefMut(inner) = ty {
            ty = *inner;
        }
        ty = match projection {
            bir::PlaceElem::Field {
                canonical: Some(identity),
                structural: false,
                ..
            } => field_types.projected(identity, &ty)?,
            bir::PlaceElem::Index(index) => indexed_type(&ty, index)?,
            bir::PlaceElem::Slice { .. } => sliced_type(&ty)?,
            bir::PlaceElem::UnionMember { ty } => ty.clone(),
            bir::PlaceElem::OptionPayload { payload_type, .. } => payload_type.clone(),
            _ => return None,
        };
    }
    Some(ty)
}

/// Resolve an indexed place without guessing a user-defined indexing contract or a heterogeneous dynamic tuple slot.
fn indexed_type(ty: &IncanType, index: &bir::Operand) -> Option<IncanType> {
    match ty {
        IncanType::NativeArray { element, .. } => return Some(element.as_ref().clone()),
        IncanType::Tuple(elements) => {
            let bir::Operand::Constant(bir::Constant::Int(index)) = index else {
                return None;
            };
            return usize::try_from(*index)
                .ok()
                .and_then(|index| elements.get(index))
                .cloned();
        }
        _ => {}
    }
    let (collection, arguments) = ty.collection()?;
    match collection {
        CollectionTypeId::List | CollectionTypeId::FrozenList => arguments.first().cloned(),
        CollectionTypeId::Dict | CollectionTypeId::FrozenDict => arguments.get(1).cloned(),
        _ => None,
    }
}

/// Preserve the checked carrier of a builtin slice. Element selection is separate from slicing and remains covered by
/// [`indexed_type`].
fn sliced_type(ty: &IncanType) -> Option<IncanType> {
    match ty {
        IncanType::Primitive(IncanPrimitiveType::Str | IncanPrimitiveType::Bytes) | IncanType::NativeArray { .. } => {
            Some(ty.clone())
        }
        _ if matches!(
            ty.collection(),
            Some((CollectionTypeId::List | CollectionTypeId::FrozenList, _))
        ) =>
        {
            Some(ty.clone())
        }
        _ => None,
    }
}

/// Return the checked value type of a place operand without treating literals as storage.
fn operand_type(value: &bir::Operand, locals: &[bir::LocalDecl], field_types: &FieldTypes) -> Option<IncanType> {
    let bir::Operand::Place(read) = value else {
        return None;
    };
    place_type(&read.place, locals, field_types)
}

/// Prove all uses in this frame while conservatively declining unsupported control flow and escaping values.
fn readonly(statements: &[bir::Statement], context: &ReadContext<'_>) -> bool {
    if !derived_temporaries_are_read_only(statements, context) {
        return false;
    }
    let mut accepted = true;
    walk_neighbors(statements, &mut |statement, next| {
        accepted &= closure_reads::shared_statement(statement, next, context);
    });
    accepted
}

/// Admit observations, shared calls and local reference bindings; consuming, exclusive and escaping operations keep
/// owned storage. Work unrelated to this parameter never weakens its proof.
fn shared_statement(statement: &bir::Statement, context: &ReadContext<'_>) -> bool {
    use bir::StatementKind as S;
    match &statement.kind {
        S::Assign { place, rvalue } => {
            if place.local_id() == Some(context.local) {
                return context.cursor && place.projection.is_empty() && matches!(rvalue, bir::Rvalue::Use(_));
            }
            if place_uses(place, context.local) {
                return false;
            }
            if let bir::Rvalue::Use(value) = rvalue {
                if projection_uses(value, context.local) {
                    return false;
                }
                if !uses(value, context.local) {
                    return true;
                }
                let source = operand_type(value, context.locals, context.field_types);
                let destination = whole_type(place, context.locals);
                return scalar_projection(value, context.local)
                    || source.as_ref().is_some_and(|source| {
                        destination == Some(&IncanType::Ref(Box::new(source.clone())))
                            || borrowed_temporary(value, place, source, context.locals)
                    });
            }
            shared_rvalue(rvalue, context)
        }
        S::Call {
            destination,
            callee,
            args,
            ..
        } => {
            if destination
                .as_ref()
                .is_some_and(|place| place.local_id() == Some(context.local))
            {
                return false;
            }
            args.iter().enumerate().all(|(index, argument)| {
                if !argument_uses(argument, context.local) {
                    return true;
                }
                let Some(value) = argument.as_one() else { return false };
                if projection_uses(value, context.local) {
                    return false;
                }
                if collection_argument_requires_owned(callee, index) {
                    return false;
                }
                scalar_projection(value, context.local)
                    || (read_only_callee(callee) && !through_payload(value))
                    || call_parameter(callee, index, context.signatures, context.candidates)
                        .and_then(|parameter| parameter.shared_owner(context.candidates))
                        .is_some_and(|owner| {
                            operand_type(value, context.locals, context.field_types).as_ref() == Some(owner)
                        })
            })
        }
        S::If { .. } => true,
        S::Loop { .. } | S::UnsafeRegion { .. } | S::Drop { .. } | S::Continue => true,
        S::Return { value } | S::Break { value } => value.as_ref().is_none_or(|value| {
            !projection_uses(value, context.local)
                && (!uses(value, context.local) || scalar_projection(value, context.local))
        }),
        S::Expr { value } => {
            !projection_uses(value, context.local)
                && (!uses(value, context.local)
                    || !matches!(value, bir::Operand::Place(read) if read.fact == bir::OwnershipFact::Move))
        }
        S::IterNext {
            destination, iterator, ..
        } => destination.local_id() != Some(context.local) && !uses(iterator, context.local),
        S::Assert { kind, .. } => match kind {
            bir::AssertionKind::Pattern { scrutinee, pattern } if uses(scrutinee, context.local) => {
                borrowed(scrutinee, context.local)
                    && pattern_selects_shared_carrier(pattern)
                    && pattern_is_read_only(pattern)
            }
            bir::AssertionKind::Raises { call, .. } => !uses(call, context.local),
            _ => true,
        },
        S::TryPropagate {
            destination, operand, ..
        } => destination.local_id() != Some(context.local) && !uses(operand, context.local),
        S::Await { destination, awaited } => {
            destination
                .as_ref()
                .is_none_or(|place| place.local_id() != Some(context.local))
                && !uses(awaited, context.local)
        }
        S::Race { destination, .. } => destination
            .as_ref()
            .is_none_or(|place| place.local_id() != Some(context.local)),
        S::Yield { value } => !uses(value, context.local),
        S::Unsupported { .. } => false,
    }
}

/// Hashed collection operations keep their key and inserted-value carriers owned. Their native runtime methods do
/// not accept an inferred `StringRef`, even though the collection receiver itself is shared.
fn collection_argument_requires_owned(callee: &bir::Callee, index: usize) -> bool {
    let bir::Callee::Method(target) = callee else {
        return false;
    };
    index > 0
        && (target.builtin_dict().is_some()
            || target.builtin_set().is_some()
            || target.builtin_frozen_dict().is_some()
            || target.builtin_frozen_set().is_some())
}

/// Admit rvalues that either do not mention this parameter or have read-only semantics. Aggregate construction,
/// closure capture and generator capture remain ownership boundaries when they contain the parameter. Result and
/// Option payloads may copy scalar projections, just like a direct scalar return.
fn shared_rvalue(value: &bir::Rvalue, context: &ReadContext<'_>) -> bool {
    let local = context.local;
    match value {
        bir::Rvalue::Use(value) | bir::Rvalue::UnaryOp(_, value) => {
            !uses(value, local) || scalar_projection(value, local)
        }
        bir::Rvalue::IsInstance(payload) => !uses(&payload.value, local) || scalar_projection(&payload.value, local),
        bir::Rvalue::BinaryOp(operator, left, right) => {
            let comparison = matches!(
                operator,
                bir::BinOp::Eq
                    | bir::BinOp::Ne
                    | bir::BinOp::Lt
                    | bir::BinOp::Le
                    | bir::BinOp::Gt
                    | bir::BinOp::Ge
                    | bir::BinOp::Is
                    | bir::BinOp::IsNot
            );
            (!uses(left, local) || comparison || scalar_projection(left, local))
                && (!uses(right, local) || comparison || scalar_projection(right, local))
        }
        bir::Rvalue::Format(parts) => parts.iter().all(|part| match part {
            bir::FormatPart::Literal(_) => true,
            bir::FormatPart::Expr { .. } => true,
        }),
        bir::Rvalue::Match { scrutinee, arms } => {
            !projection_uses(scrutinee, local)
                && (!uses(scrutinee, local)
                || ((borrowed(scrutinee, local) || context.derived)
                    && arms.iter().any(|arm| pattern_selects_shared_carrier(&arm.pattern))
                    && arms.iter().all(|arm| pattern_is_read_only(&arm.pattern))))
                && arms.iter().all(|arm| {
                    arm.guard
                        .as_ref()
                        .is_none_or(|guard| !uses(guard, local) || scalar_projection(guard, local))
                        && (!uses(&arm.result, local) || scalar_projection(&arm.result, local))
                })
                // Payload ownership constrains the matched owner, not an unrelated parameter. The normal statement
                // walk still checks that parameter's uses inside every arm and guard.
                && (!uses(scrutinee, local) || match_bindings_are_read_only(arms, context))
        }
        bir::Rvalue::Aggregate(bir::AggregateKind::OptionSome, elements) => {
            elements.iter().all(|element| match element {
                bir::ArgumentElement::One(payload) => !uses(payload, local) || scalar_projection(payload, local),
                _ => !argument_uses(element, local),
            })
        }
        bir::Rvalue::Aggregate(_, elements) => !elements.iter().any(|element| argument_uses(element, local)),
        bir::Rvalue::Dict(entries) => !entries.iter().any(|entry| dict_entry_uses(entry, local)),
        bir::Rvalue::ResultVariant(variant) => {
            !uses(&variant.payload, local) || scalar_projection(&variant.payload, local)
        }
        bir::Rvalue::Closure { captured_operands, .. } => !captured_operands.iter().any(|operand| uses(operand, local)),
        bir::Rvalue::Generator {
            source,
            captured_operands,
            ..
        } => !uses(source, local) && !captured_operands.iter().any(|operand| uses(operand, local)),
        bir::Rvalue::ValueEnumVariant(_) | bir::Rvalue::FieldlessEnumVariant(_) => true,
    }
}

/// A compiler temporary may carry a checked shared observation without turning an ordinary user binding into an
/// alias implicitly. A last-use move is provisional until whole-body inference proves that the temporary never owns
/// or escapes the value; [`retain_shared_temporaries`] then publishes the reference representation.
fn borrowed_temporary(
    value: &bir::Operand,
    destination: &bir::Place,
    source: &IncanType,
    locals: &[bir::LocalDecl],
) -> bool {
    matches!(value, bir::Operand::Place(read)
        if matches!(read.fact, bir::OwnershipFact::Borrow | bir::OwnershipFact::Clone)
            || (read.fact == bir::OwnershipFact::Move
                && *source == IncanType::Primitive(IncanPrimitiveType::Str)))
        && whole_type(destination, locals) == Some(source)
        && destination
            .local_id()
            .and_then(|local| locals.get(local.index()))
            .is_some_and(|local| matches!(local.origin, bir::LocalOrigin::Temporary))
}

/// Whether a call element reads this local. Named and spread elements stay distinct from a resolved fixed slot.
fn argument_uses(argument: &bir::ArgumentElement, local: bir::LocalId) -> bool {
    match argument {
        bir::ArgumentElement::One(operand) | bir::ArgumentElement::Named { operand, .. } => {
            uses(operand, local) || projection_uses(operand, local)
        }
        bir::ArgumentElement::Spread(spread) => uses(&spread.source, local) || projection_uses(&spread.source, local),
    }
}

/// Whether a dictionary construction entry consumes this local.
fn dict_entry_uses(entry: &bir::DictEntry, local: bir::LocalId) -> bool {
    match entry {
        bir::DictEntry::Pair(key, value) => uses(key, local) || uses(value, local),
        bir::DictEntry::Spread(spread) => uses(&spread.source, local),
    }
}

/// Whether a read reaches a narrowed union or option payload, which native lowering can't project through a lent enum.
fn through_payload(value: &bir::Operand) -> bool {
    matches!(value, bir::Operand::Place(read) if read.place.projection.iter().any(|step| matches!(
        step,
        bir::PlaceElem::UnionMember { .. } | bir::PlaceElem::OptionPayload { .. }
    )))
}

/// Compiler-owned helpers whose inputs are observations or explicit value-producing operations over observations.
/// Extraction and validation helpers stay owned because they may consume carrier payloads.
fn read_only_callee(callee: &bir::Callee) -> bool {
    if let bir::Callee::Function(bir::CallableTarget::Named(target)) = callee {
        return target.builtin == Some(BuiltinFnId::Len);
    }
    let bir::Callee::Helper(helper) = callee else {
        return false;
    };
    matches!(
        helper,
        bir::HelperOp::StrConcat
            | bir::HelperOp::StrEq
            | bir::HelperOp::StrNe
            | bir::HelperOp::StrLt
            | bir::HelperOp::StrLe
            | bir::HelperOp::StrGt
            | bir::HelperOp::StrGe
            | bir::HelperOp::StrUpper
            | bir::HelperOp::StrLower
            | bir::HelperOp::StrStrip
            | bir::HelperOp::StrLen
            | bir::HelperOp::StrReplace
            | bir::HelperOp::StrJoin
            | bir::HelperOp::StrSplit
            | bir::HelperOp::StrContains
            | bir::HelperOp::StrNotContains
            | bir::HelperOp::ListConcat
            | bir::HelperOp::ListRepeat
            | bir::HelperOp::ListContains
            | bir::HelperOp::ListNotContains
    )
}

/// Whether this exact operand is already represented as a shared read of the candidate.
fn borrowed(value: &bir::Operand, local: bir::LocalId) -> bool {
    matches!(value, bir::Operand::Place(read)
        if read.place.local_id() == Some(local) && read.fact == bir::OwnershipFact::Borrow)
}

/// A checked Copy read through a field observes that field rather than transferring its native owner.
fn scalar_projection(value: &bir::Operand, local: bir::LocalId) -> bool {
    matches!(value, bir::Operand::Place(read) if read.place.local_id() == Some(local)
        && !read.place.projection.is_empty() && read.fact == bir::OwnershipFact::Copy)
}

/// Detect a read rooted in this lexical local, independently of its diagnostic spelling.
fn uses(value: &bir::Operand, local: bir::LocalId) -> bool {
    matches!(value, bir::Operand::Place(read) if read.place.local_id() == Some(local))
}

/// Detect a local used by an index or slice bound embedded in an assignment target. Such an operand becomes part of
/// the mutation rather than an ordinary observation, so it cannot justify a shared parameter contract.
fn place_uses(place: &bir::Place, local: bir::LocalId) -> bool {
    place.projection.iter().any(|projection| match projection {
        bir::PlaceElem::Index(index) => uses(index, local),
        bir::PlaceElem::Slice { start, end, step } => {
            [start, end, step].into_iter().flatten().any(|bound| uses(bound, local))
        }
        _ => false,
    })
}

/// Detect a local embedded as an index or slice bound in another operand's place.
fn projection_uses(value: &bir::Operand, local: bir::LocalId) -> bool {
    matches!(value, bir::Operand::Place(read) if place_uses(&read.place, local))
}

/// Compare the effective contracts of duplicate signature records without treating analysis-local candidate
/// coordinates as published ABI facts.
fn same_parameters(left: &[SignatureParameter], right: &[SignatureParameter], candidates: &HashSet<Candidate>) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|(left, right)| {
            left.ty == right.ty
                && left.access(candidates) == right.access(candidates)
                && left.callback_does_not_escape == right.callback_does_not_escape
                && left.result_holds_loan == right.result_holds_loan
        })
}

/// Resolve a function or method slot only from the retained canonical target, instantiation and checked argument
/// binding. A method binding excludes its receiver, while its body signature keeps that receiver in slot zero.
fn call_parameter<'a>(
    callee: &bir::Callee,
    index: usize,
    signatures: &'a Signatures,
    candidates: &HashSet<Candidate>,
) -> Option<&'a SignatureParameter> {
    let (identity, arguments, slot) = match callee {
        bir::Callee::Function(bir::CallableTarget::Named(target)) => (
            target.canonical.as_ref()?,
            &target.type_args,
            target.binding.slot_arguments()?.get(index)?.slot,
        ),
        bir::Callee::Method(target) => {
            let slot = if index == 0 {
                0
            } else {
                target.binding.slot_arguments()?.get(index - 1)?.slot + 1
            };
            (target.canonical.as_ref()?, &target.type_args, slot)
        }
        _ => return None,
    };
    let matching = signatures.by_identity.get(identity)?;
    let mut exact = matching.iter().filter(|signature| signature.arguments == *arguments);
    if let Some(signature) = exact.next() {
        if exact.any(|other| !same_parameters(&other.parameters, &signature.parameters, candidates)) {
            return None;
        }
        return signature.parameters.get(slot);
    }
    let mut generic = matching.iter().filter(|signature| signature.generic);
    let signature = generic.next()?;
    if generic.any(|other| !same_parameters(&other.parameters, &signature.parameters, candidates)) {
        return None;
    }
    signature.parameters.get(slot)
}

/// Record the callee's shared passing decision at each source call. A read of the selected owner, or of a checked
/// reference to it, becomes a loan. Lowering lends by its plan types, so three reads keep their facts: a conversion
/// and a temporary, which may hold a range declared as the list it converts to, because each holds a value this call
/// consumes or lends once; and a read whose type is only exact after instantiation.
///
/// A read moved to another input of the same call would leave that loan reading moved storage, so the move becomes the
/// clone the lent read no longer needs.
fn retain_call_loans(
    statement: &mut bir::Statement,
    locals: &[bir::LocalDecl],
    signatures: &Signatures,
    field_types: &FieldTypes,
    candidates: &HashSet<Candidate>,
) -> Vec<bir::LocalId> {
    let bir::StatementKind::Call { callee, args, .. } = &mut statement.kind else {
        return Vec::new();
    };
    let mut lent = Vec::new();
    let mut ended = Vec::new();
    for (index, argument) in args.iter_mut().enumerate() {
        let bir::ArgumentElement::One(bir::Operand::Place(read)) = argument else {
            continue;
        };
        let Some(parameter) = call_parameter(callee, index, signatures, candidates) else {
            continue;
        };
        let Some(owner) = parameter.shared_owner(candidates) else {
            continue;
        };
        lent.push((index, read.place.clone()));
        let temporary = read
            .place
            .local_id()
            .and_then(|local| locals.get(local.index()))
            .is_some_and(|local| matches!(local.origin, bir::LocalOrigin::Temporary));
        let actual = place_type(&read.place, locals, field_types);
        let retained_temporary = matches!(&actual,
            Some(IncanType::Ref(referent)) if referent.as_ref() == owner);
        if (!temporary || retained_temporary)
            && actual.is_some_and(|actual| {
                actual == *owner || matches!(actual, IncanType::Ref(referent) if referent.as_ref() == owner)
            })
        {
            if !parameter.result_holds_loan && ended_loans::replaces_last_transfer(read, locals, owner) {
                ended.extend(read.place.local_id());
            }
            read.fact = bir::OwnershipFact::Borrow;
            read.last_use = false;
        }
    }
    for (index, argument) in args.iter_mut().enumerate() {
        if let bir::ArgumentElement::One(bir::Operand::Place(read)) = argument
            && read.fact == bir::OwnershipFact::Move
            && lent
                .iter()
                .any(|(slot, place)| *slot != index && overlapping(place, &read.place))
        {
            read.fact = bir::OwnershipFact::Clone;
            read.last_use = false;
        }
    }
    ended
}

/// Whether two reads reach shared storage: the same root, with one projection path a prefix of the other.
fn overlapping(left: &bir::Place, right: &bir::Place) -> bool {
    left.root == right.root
        && left
            .projection
            .iter()
            .zip(&right.projection)
            .all(|(left, right)| left == right)
}

/// Traverse lexical control flow while leaving closure and generator frames independent.
fn walk(statements: &[bir::Statement], visit: &mut impl FnMut(&bir::Statement)) {
    walk_neighbors(statements, &mut |statement, _next| visit(statement));
}

/// Visit each lexical statement with its next sibling, never treating the end of one branch as adjacent to another.
fn walk_neighbors(statements: &[bir::Statement], visit: &mut impl FnMut(&bir::Statement, Option<&bir::Statement>)) {
    for (index, statement) in statements.iter().enumerate() {
        visit(statement, statements.get(index + 1));
        match &statement.kind {
            bir::StatementKind::Assign {
                rvalue: bir::Rvalue::Match { arms, .. },
                ..
            } => {
                for arm in arms {
                    walk_neighbors(&arm.guard_stmts, visit);
                    walk_neighbors(&arm.body_stmts, visit);
                }
            }
            bir::StatementKind::If {
                then_block, else_block, ..
            } => {
                walk_neighbors(&then_block.stmts, visit);
                if let Some(block) = else_block {
                    walk_neighbors(&block.stmts, visit);
                }
            }
            bir::StatementKind::Loop { body } | bir::StatementKind::UnsafeRegion { body, .. } => {
                walk_neighbors(&body.stmts, visit)
            }
            bir::StatementKind::Race { arms, .. } => {
                for arm in arms {
                    walk_neighbors(&arm.body.stmts, visit);
                }
            }
            _ => {}
        }
    }
}

/// Rewrite the same lexical statement surface without allowing a nested callable to share frame-local identities.
fn walk_mut(statements: &mut [bir::Statement], visit: &mut impl FnMut(&mut bir::Statement)) {
    walk_statements_mut(statements, false, visit);
}

/// Rewrite every call site of one body, including the calls inside its closure and generator bodies.
fn walk_calls_mut(statements: &mut [bir::Statement], visit: &mut impl FnMut(&mut bir::Statement)) {
    walk_statements_mut(statements, true, visit);
}

/// The one mutable statement walk: lexical control flow always, closure and generator bodies when `enter_callables`.
fn walk_statements_mut(
    statements: &mut [bir::Statement],
    enter_callables: bool,
    visit: &mut impl FnMut(&mut bir::Statement),
) {
    for statement in statements {
        visit(statement);
        match &mut statement.kind {
            bir::StatementKind::Assign {
                rvalue: bir::Rvalue::Match { arms, .. },
                ..
            } => {
                for arm in arms {
                    walk_statements_mut(&mut arm.guard_stmts, enter_callables, visit);
                    walk_statements_mut(&mut arm.body_stmts, enter_callables, visit);
                }
            }
            bir::StatementKind::Assign {
                rvalue: bir::Rvalue::Closure { body, .. },
                ..
            } if enter_callables => walk_statements_mut(&mut body.stmts, enter_callables, visit),
            bir::StatementKind::Assign {
                rvalue: bir::Rvalue::Generator { body, .. },
                ..
            } if enter_callables => walk_statements_mut(&mut body.stmts, enter_callables, visit),
            bir::StatementKind::If {
                then_block, else_block, ..
            } => {
                walk_statements_mut(&mut then_block.stmts, enter_callables, visit);
                if let Some(block) = else_block {
                    walk_statements_mut(&mut block.stmts, enter_callables, visit);
                }
            }
            bir::StatementKind::Loop { body } | bir::StatementKind::UnsafeRegion { body, .. } => {
                walk_statements_mut(&mut body.stmts, enter_callables, visit)
            }
            bir::StatementKind::Race { arms, .. } => {
                for arm in arms {
                    walk_statements_mut(&mut arm.body.stmts, enter_callables, visit);
                }
            }
            _ => {}
        }
    }
}
