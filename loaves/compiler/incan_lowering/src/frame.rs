//! Shared native frame storage, ownership transfers, and unwinding calls.

use incan_mir_plan_native::{
    BasicBlock, Callee, CalleeKind, Constant, EnumDeclaration, ExternalFunction, FieldProjection, Function, Local,
    Operand, OperandKind, Place, PlanType, Projection, RvalueKind, SourceSpan, Statement, Terminator, TerminatorKind,
    Unwind, assign as plan_assign, block as plan_block, place as plan_place, tuple_element_type,
};
use incan_semantics_core::body_ir::{
    DecoratedBinding, EnumDeclaration as SourceEnumDeclaration, NominalDeclaration, TraitDefaultInstantiation,
    ValueEnumDeclaration,
};
use incan_semantics_core::{CanonicalSymbolId, IncanInputAccess, IncanType};

use crate::models::PlaceTables;
use crate::native_type_identity::ConstructorIndex;
use crate::program_context::ProgramContext;
use crate::type_parameters::Instance;

/// One reusable unwind path for an exact initialized-owner and partial-transfer state.
#[derive(Clone, Debug)]
pub struct CleanupChain {
    pub owners: Vec<i64>,
    pub moved_tuple_fields: Vec<(i64, i64)>,
    pub target: i64,
}

/// Construction state for one function's native frame.
///
/// `owned` lists the locals that currently hold an initialized value needing a drop, in initialization order.
/// `cleanup_chains` shares one unwind path among calls with that exact owner and partial-transfer state. `closed`
/// marks blocks whose terminator is final. `parameter_count` separates invocation-long argument storage from locals
/// with explicit storage lifetimes. The remaining fields carry the frame's checked bindings, iteration state and the
/// functions its closures synthesize, as the Incan lowering's `Builder` documents them.
pub struct Builder {
    pub locals: Vec<Local>,
    pub parameter_count: i64,
    pub blocks: Vec<BasicBlock>,
    pub current: usize,
    pub source: String,
    pub file: String,
    /// Each line's starting byte offset in `source`, read once per frame so each span maps by binary search.
    pub line_starts: Vec<i64>,
    pub module_identity: String,
    pub callable_identity: Option<CanonicalSymbolId>,
    pub loops: Vec<(i64, i64)>,
    pub loop_owners: Vec<Vec<i64>>,
    pub externals: Vec<ExternalFunction>,
    pub owned: Vec<i64>,
    pub cleanup_chains: Vec<CleanupChain>,
    pub closed: Vec<bool>,
    pub iterators: Vec<(i64, i64)>,
    pub borrowed_items: Vec<i64>,
    pub in_place_items: Vec<i64>,
    pub partial_callables: Vec<(i64, Vec<bool>)>,
    pub scalar_closures: Vec<i64>,
    pub unproven_closures: Vec<i64>,
    pub program: ProgramContext,
    pub unions: Vec<IncanType>,
    pub local_offset: i64,
    pub consuming_closure_frame: bool,
    pub intrinsics: Vec<EnumDeclaration>,
    pub generator_handle: i64,
    pub instances: Vec<Instance>,
    pub type_parameters: Vec<String>,
    pub type_arguments: Vec<IncanType>,
    pub callable_type_arguments: Vec<IncanType>,
    pub trait_environment: Option<TraitDefaultInstantiation>,
    pub function_pointer_locals: Vec<i64>,
    pub name: String,
    pub synthesized: Vec<Function>,
    pub materialized_closures: Vec<i64>,
    pub step_element: Option<PlanType>,
    pub anchor: Option<SourceSpan>,
    pub iteration_locals: Vec<i64>,
    pub step_bindings: Vec<i64>,
    pub step_clauses: Vec<(i64, i64, Vec<i64>)>,
    pub scoped_locals: Vec<(i64, i64)>,
    pub scope_parents: Vec<(i64, i64)>,
    pub loop_scopes: Vec<i64>,
    pub loop_exit_blocks: Vec<Vec<i64>>,
    pub loop_exit_owners: Vec<Vec<Vec<i64>>>,
    pub forbidden_reads: Vec<i64>,
    pub moved_tuple_fields: Vec<(i64, i64)>,
    pub body_identity: Option<CanonicalSymbolId>,
    pub checked_local_types: Vec<IncanType>,
    pub initializing_decorator: Option<CanonicalSymbolId>,
    pub cached_decorator: i64,
    pub callable_name_dynamic_profile: bool,
    pub reference_values: Vec<i64>,
    pub returned_view: IncanInputAccess,
    pub carries_view: bool,
    pub invocation_storage: Vec<(i64, i64)>,
    pub released: Vec<i64>,
}

impl Builder {
    /// The module's specialized nominal table, shared with the program context.
    pub fn nominals(&self) -> &[NominalDeclaration] {
        &self.program.nominals
    }

    /// The same ordered enum declarations used to assign this module's layout identities.
    pub fn enums(&self) -> &[SourceEnumDeclaration] {
        &self.program.enums
    }

    /// First-match constructor positions from the nominal table's program owner.
    pub fn constructors(&self) -> &ConstructorIndex {
        &self.program.constructors
    }

    pub fn value_enums(&self) -> &[ValueEnumDeclaration] {
        &self.program.value_enums
    }

    pub fn decorated_bindings(&self) -> &[DecoratedBinding] {
        &self.program.decorated_bindings
    }

    /// The declaration tables a checked place projects through.
    pub fn tables(&self) -> PlaceTables<'_> {
        PlaceTables {
            constructors: &self.program.constructors,
            declarations: &self.program.nominals,
            enums: &self.program.enums,
            unions: &self.unions,
            intrinsics: &self.intrinsics,
        }
    }

    /// Allocate a block before connecting edges, preserving index stability.
    pub fn new_block(&mut self, span: &SourceSpan) -> i64 {
        let index = self.blocks.len() as i64;
        self.blocks
            .push(plan_block(Vec::new(), TerminatorKind::Unreachable, span));
        self.closed.push(false);
        index
    }

    /// Close the active block; callers must select a new block before emitting.
    pub fn finish(&mut self, exit: TerminatorKind, span: &SourceSpan) {
        self.blocks[self.current].terminator = Terminator {
            kind: exit,
            span: span.clone(),
        };
        self.closed[self.current] = true;
    }

    /// Append one operation to the selected native block.
    pub fn emit(&mut self, statement: Statement) {
        self.blocks[self.current].statements.push(statement);
    }

    /// Select a block by its plan index.
    pub fn select(&mut self, block: i64) {
        self.current = block as usize;
    }
}

/// Reserve one typed temporary without assuming it is already initialized.
pub fn temporary(builder: &mut Builder, ty: PlanType, span: &SourceSpan) -> Place {
    let index = builder.locals.len() as i64;
    builder.locals.push(Local { ty, span: span.clone() });
    plan_place(index, span)
}

/// Remove an initialized owner after its value moves or drops.
pub fn forget_owned(builder: &mut Builder, index: i64) {
    let before = builder.owned.len();
    builder.owned.retain(|live| *live != index);
    if builder.owned.len() != before {
        builder.released.push(index);
    }
    builder.moved_tuple_fields.retain(|(local, _)| *local != index);
}

/// Describe one state's initialized remainder without borrowing the Builder that will receive its drop blocks.
fn remaining_owned_places(
    locals: &[Local],
    moved_tuple_fields: &[(i64, i64)],
    index: i64,
    span: &SourceSpan,
) -> Vec<Place> {
    if !moved_tuple_fields.iter().any(|(local, _)| *local == index) {
        return vec![plan_place(index, span)];
    }
    let mut places = Vec::new();
    if let PlanType::Tuple(elements) = &locals[index as usize].ty {
        for (slot, element) in elements.iter().enumerate() {
            if !moved_tuple_fields.contains(&(index, slot as i64)) {
                places.push(Place {
                    local: index,
                    projection: Projection::Field(slot as i64, tuple_element_type(element)),
                    span: span.clone(),
                });
            }
        }
    }
    places
}

/// Drop a known initialized owner and continue in a fresh normal block.
pub fn drop_owned(builder: &mut Builder, index: i64, span: &SourceSpan) {
    if !builder.owned.contains(&index) {
        return;
    }
    let places = remaining_owned_places(&builder.locals, &builder.moved_tuple_fields, index, span);
    forget_owned(builder, index);
    for place in places {
        let successor = builder.new_block(span);
        builder.finish(TerminatorKind::Drop(place, successor, Unwind::Continue), span);
        builder.select(successor);
    }
}

/// Release every remaining owner, newest first, before a function returns.
pub fn drop_all_owned(builder: &mut Builder, span: &SourceSpan) {
    while let Some(newest) = builder.owned.last().copied() {
        drop_owned(builder, newest, span);
    }
}

/// Leave the innermost loop once its iteration is exhausted, as a `break` leaves it.
pub fn exhaust_loop(builder: &mut Builder, span: &SourceSpan) {
    let current = builder.current as i64;
    let owned = builder.owned.clone();
    if let Some(exits) = builder.loop_exit_blocks.last_mut() {
        exits.push(current);
    }
    if let Some(owners) = builder.loop_exit_owners.last_mut() {
        owners.push(owned);
    }
    let after = builder.loops.last().map(|(_, after)| *after).unwrap_or_default();
    builder.finish(TerminatorKind::Goto(after), span);
}

/// Initialize a pattern binding with the value its read produced, and give the binding exactly what that read owns.
pub fn bind_read_value(builder: &mut Builder, local: i64, value: Operand, span: &SourceSpan) {
    track_rvalue_moves(builder, &RvalueKind::Use(value.clone()));
    let moved = matches!(value.kind, OperandKind::Move(_));
    builder.emit(plan_assign(plan_place(local, span), RvalueKind::Use(value), span));
    if moved {
        builder.owned.push(local);
    }
}

/// Remove transferred whole owners before building cleanup edges.
pub fn track_rvalue_moves(builder: &mut Builder, value: &RvalueKind) {
    match value {
        RvalueKind::Use(operand) => forget_moved_arguments(builder, std::slice::from_ref(operand)),
        RvalueKind::Tuple(operands) | RvalueKind::Model(_, operands) | RvalueKind::Enum(_, _, operands) => {
            forget_moved_arguments(builder, operands)
        }
        _ => {}
    }
}

/// Forget transferred whole owners and retain the initialized remainder of a consumed tuple.
pub fn forget_moved_arguments(builder: &mut Builder, arguments: &[Operand]) {
    for argument in arguments {
        let OperandKind::Move(value) = &argument.kind else {
            continue;
        };
        if value.projection == Projection::Whole {
            forget_owned(builder, value.local);
        } else if let Projection::Field(slot, _) = &value.projection
            && matches!(builder.locals[value.local as usize].ty, PlanType::Tuple(_))
        {
            builder.moved_tuple_fields.push((value.local, *slot));
        }
    }
}

/// Reuse or build the unwind path for the current owners and return its first block.
///
/// The chain drops owners newest first and ends in `UnwindResume`; calls with the same initialized locals and
/// transferred tuple fields target that same path, and the first call's span names its drop blocks.
pub fn cleanup_exit(builder: &mut Builder, span: &SourceSpan) -> i64 {
    let owners = builder.owned.clone();
    let moved = builder.moved_tuple_fields.clone();
    cleanup_chain(builder, owners, moved, span)
}

/// Reuse one exact cleanup state, or prepend its newest owner to the shared predecessor tail.
fn cleanup_chain(
    builder: &mut Builder,
    owners: Vec<i64>,
    moved_tuple_fields: Vec<(i64, i64)>,
    span: &SourceSpan,
) -> i64 {
    for chain in &builder.cleanup_chains {
        if chain.owners == owners && chain.moved_tuple_fields == moved_tuple_fields {
            return chain.target;
        }
    }
    let Some(newest) = owners.last().copied() else {
        let target = builder.new_block(span);
        builder.blocks[target as usize].cleanup = true;
        builder.blocks[target as usize].terminator = Terminator {
            kind: TerminatorKind::UnwindResume,
            span: span.clone(),
        };
        builder.cleanup_chains.push(CleanupChain {
            owners,
            moved_tuple_fields,
            target,
        });
        return target;
    };
    let predecessor_owners = owners[..owners.len() - 1].to_vec();
    let predecessor_transfers: Vec<(i64, i64)> = moved_tuple_fields
        .iter()
        .filter(|(local, _)| *local != newest)
        .copied()
        .collect();
    let places = remaining_owned_places(&builder.locals, &moved_tuple_fields, newest, span);
    let mut target = cleanup_chain(builder, predecessor_owners, predecessor_transfers, span);
    for place in places {
        let current = builder.new_block(span);
        builder.blocks[current as usize].cleanup = true;
        builder.blocks[current as usize].terminator = Terminator {
            kind: TerminatorKind::Drop(place, target, Unwind::Terminate),
            span: span.clone(),
        };
        target = current;
    }
    builder.cleanup_chains.push(CleanupChain {
        owners,
        moved_tuple_fields,
        target,
    });
    target
}

/// Emit a call with the current owners' unwind cleanup; a Never destination closes its continuation.
pub fn terminate_call(
    builder: &mut Builder,
    callee: CalleeKind,
    arguments: Vec<Operand>,
    destination: Place,
    span: &SourceSpan,
) {
    // A moved argument is the callee's to drop now, so it leaves the owners before the cleanup chain is built.
    forget_moved_arguments(builder, &arguments);
    let continuation = builder.new_block(span);
    let cleanup = cleanup_exit(builder, span);
    let never = place_type(&builder.locals, &destination) == PlanType::Never;
    let lent = lent_locals(&arguments);
    let call = TerminatorKind::Call(
        Callee {
            kind: callee,
            span: span.clone(),
        },
        arguments,
        destination,
        continuation,
        Unwind::Cleanup(cleanup),
    );
    builder.finish(call, span);
    builder.select(continuation);
    if never {
        builder.finish(TerminatorKind::Unreachable, span);
        return;
    }
    release_invocation_storage(builder, &lent, span);
}

/// Own storage materialized for one call, and pair it with the reference that call receives.
pub fn lend_invocation_storage(builder: &mut Builder, storage: &Place, lent: &Operand) {
    builder.owned.push(storage.local);
    match &lent.kind {
        OperandKind::Copy(reference) | OperandKind::Move(reference) => {
            builder.invocation_storage.push((reference.local, storage.local))
        }
        _ => {}
    }
}

fn lent_locals(arguments: &[Operand]) -> Vec<i64> {
    arguments
        .iter()
        .filter_map(|argument| match &argument.kind {
            OperandKind::Copy(reference) | OperandKind::Move(reference) => Some(reference.local),
            _ => None,
        })
        .collect()
}

/// Drop the storage materialized for the call that just returned, once its loan has ended.
fn release_invocation_storage(builder: &mut Builder, lent: &[i64], span: &SourceSpan) {
    if builder.invocation_storage.is_empty() {
        return;
    }
    let mut kept = Vec::new();
    let mut released = Vec::new();
    for (reference, storage) in builder.invocation_storage.iter().copied() {
        if lent.contains(&reference) {
            released.push(storage);
        } else {
            kept.push((reference, storage));
        }
    }
    builder.invocation_storage = kept;
    for storage in released.into_iter().rev() {
        drop_owned(builder, storage, span);
    }
}

/// An explicit native move or copy from a known typed local.
pub fn read_place(place: Place, moved: bool, span: &SourceSpan) -> Operand {
    let kind = if moved {
        OperandKind::Move(place)
    } else {
        OperandKind::Copy(place)
    };
    Operand {
        kind,
        span: span.clone(),
    }
}

/// A materialized value's plan type, without consulting source syntax or frontend policy.
pub fn value_type(builder: &Builder, operand: &Operand) -> Result<PlanType, String> {
    Ok(match &operand.kind {
        OperandKind::Copy(place) | OperandKind::Move(place) => place_type(&builder.locals, place),
        OperandKind::Literal(constant) => match constant {
            Constant::Numeric(_, ty) | Constant::RustItem(_, ty) => ty.clone(),
            Constant::Int(_) => PlanType::Int,
            Constant::Float(_) => PlanType::Float,
            Constant::Bool(_) => PlanType::Bool,
            Constant::Unit => PlanType::Unit,
            Constant::TypeToken(represented) => PlanType::TypeToken(represented.clone()),
            Constant::Text(_) => PlanType::StrRef,
        },
    })
}

/// Retained scalar and owner projections, including supported collection reborrows.
pub fn place_type(locals: &[Local], value: &Place) -> PlanType {
    match &value.projection {
        Projection::VariantField(_, _, ty)
        | Projection::Field(_, ty)
        | Projection::DerefField(_, ty)
        | Projection::NumericValue(ty)
        | Projection::Deref(ty) => ty.clone(),
        Projection::Fields(fields) | Projection::DerefFields(fields) => {
            fields.last().map(|field| field.ty.clone()).unwrap_or(PlanType::Unit)
        }
        Projection::Value => PlanType::Int,
        Projection::Overflow => PlanType::Bool,
        Projection::Whole => locals[value.local as usize].ty.clone(),
    }
}

/// Append one exact payload or field step without discarding the preceding checked projection path.
pub fn appended_field(source: &Place, field: FieldProjection, span: &SourceSpan) -> Result<Place, String> {
    let mut fields = Vec::new();
    let mut dereference = false;
    match &source.projection {
        Projection::Whole => {}
        Projection::Field(slot, ty) => fields.push(FieldProjection::new(*slot, ty.clone())),
        Projection::VariantField(variant, slot, ty) => {
            let mut projection = FieldProjection::new(*slot, ty.clone());
            projection.variant = *variant;
            fields.push(projection);
        }
        Projection::Fields(path) => fields = path.clone(),
        Projection::Deref(_) => dereference = true,
        Projection::DerefField(slot, ty) => {
            dereference = true;
            fields.push(FieldProjection::new(*slot, ty.clone()));
        }
        Projection::DerefFields(path) => {
            dereference = true;
            fields = path.clone();
        }
        _ => return Err("invalid Body IR carrier projection prefix".into()),
    }
    fields.push(field);
    let projection = if dereference {
        Projection::DerefFields(fields)
    } else {
        Projection::Fields(fields)
    };
    Ok(Place {
        local: source.local,
        projection,
        span: span.clone(),
    })
}

/// Append a typed dereference while preserving the complete path to the stored reference.
pub fn dereferenced_place(source: &Place, referent: PlanType, span: &SourceSpan) -> Result<Place, String> {
    if source.projection == Projection::Whole {
        return Ok(Place {
            local: source.local,
            projection: Projection::Deref(referent),
            span: span.clone(),
        });
    }
    let mut field = FieldProjection::new(-1, referent);
    field.dereferences = 1;
    appended_field(source, field, span)
}

/// Declare a monomorphic runtime signature and emit its unwinding call.
pub fn external_call(
    builder: &mut Builder,
    name: &str,
    parameters: Vec<PlanType>,
    result: PlanType,
    arguments: Vec<Operand>,
    destination: Place,
    span: &SourceSpan,
) {
    builder.externals.push(ExternalFunction {
        path: name.to_owned(),
        type_arguments: Vec::new(),
        parameters,
        return_type: result.clone(),
        span: span.clone(),
    });
    let local = destination.local;
    terminate_call(
        builder,
        CalleeKind::External(name.to_owned()),
        arguments,
        destination,
        span,
    );
    // A returned String is a new owner: it is dropped at scope exit or on unwind like any other.
    if result == PlanType::String {
        builder.owned.push(local);
    }
}

/// Wrap a plan constant as an operand.
pub fn literal(constant: Constant, span: &SourceSpan) -> Operand {
    Operand {
        kind: OperandKind::Literal(constant),
        span: span.clone(),
    }
}

/// Retain static text without an implicit allocation.
pub fn text_literal(text: &str, span: &SourceSpan) -> Operand {
    literal(Constant::Text(text.to_owned()), span)
}

/// Connect an open block while retaining explicit return and loop exits.
pub fn finish_fallthrough(builder: &mut Builder, target: i64, span: &SourceSpan) {
    if !builder.closed[builder.current] {
        builder.finish(TerminatorKind::Goto(target), span);
    }
}
