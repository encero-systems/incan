//! Direct execution of the deliberately narrow #988 Body-IR replacement profile.
//!
//! This module consumes [`BodyIrModule`] directly. It never reads generated Rust, never
//! delegates a requested replacement execution to [`crate::backend::ir`], and rejects every operation outside the
//! first free-function profile with the original Body-IR source span. The profile is intentionally limited to
//! scalar arithmetic, compiler-owned string concatenation, branches, normalized loops, assertions, and source-local
//! recursive tuple/list values. It admits one numeric tuple projection and one integer list projection or assignment;
//! builtin iteration remains limited to `list[tuple[scalar, scalar]]`. The selected entrypoint must produce a scalar
//! observable, although an admitted sibling may return a structural intermediate to its direct caller. The executor
//! also consumes the retained callable vocabulary directly: captured local closures, partial presets,
//! source-evaluable defaults, identity-selected local or same-module named calls, generator expressions and
//! generator functions, and their bounded lazy `map`/`filter` adapters. Packages, Rust interop, unsupported
//! callable/default forms, general destructuring, and other projections remain visible refusals. Its enclosing
//! declaration snapshot retains a deferred generator's shape, but the frame executes and adds execution-frame
//! evidence only when collection polls it; no path falls back to generated Rust.

use std::collections::{BTreeMap, BTreeSet};

use incan_core::{
    lang::surface::constructors::{ConstructorId, as_str as constructor_name},
    lang::types::collections::{self, CollectionTypeId},
    python_floor_div_i64, python_mod_i64,
};
use incan_semantics_core::body_ir::{
    AggregateKind, ArgumentBinding, BinOp, Body, BodyIrModule, CallableParam, CallableParamDefault, CallableTarget,
    Callee, ClosureBody, Constant, DefaultComputation, GeneratorBody, HelperOp, IterProtocol, LocalCallableTarget,
    LocalId, LocalOrigin, NamedCallableBuiltin, NamedCallableTarget, Operand, OwnershipFact, Place, PlaceElem, Rvalue,
    Statement, StatementKind, UnOp,
};
use incan_semantics_core::{AbiV0RuntimeRequirement, HirSourceSpan, IncanPrimitiveType, IncanType};

use crate::backend::selection::digest_output;

/// Bounded instruction count for one replacement execution.
///
/// The first profile deliberately executes normalized loops rather than translating them to native code. Keeping a
/// deterministic bound turns an accidental infinite loop into an explicit unavailable result instead of allowing a
/// test or CLI invocation to hang without a receipt.
const MAX_EXECUTION_STEPS: usize = 100_000;

/// One runtime value supported by the bounded replacement-execution profile.
#[derive(Debug, Clone, PartialEq)]
pub enum ReplacementValue {
    /// An Incan `int` value.
    Int(i64),
    /// An Incan `bool` value.
    Bool(bool),
    /// An owned Incan `str` value.
    Str(String),
    /// An Incan floating-point literal, retained so unsupported float operations refuse honestly.
    Float(String),
    /// An Incan `None`/unit value.
    Unit,
    /// A normalized runtime range iterator for the selected `range` source-spelling control-flow case.
    Range { next: i64, end: i64, step: i64 },
    /// A source-local structural list value with the cursor owned by its materialized builtin iterator local.
    List {
        elements: Vec<ReplacementValue>,
        next: usize,
    },
    /// A source-local structural tuple whose elements remain direct replacement values.
    Tuple(Vec<ReplacementValue>),
    /// A closure or partial application whose lexical captures were evaluated when the value was constructed.
    Callable(Box<ReplacementCallable>),
    /// A generator expression or generator function whose frame remains deferred until an admitted consumer polls
    /// it. The frame owns its locals and continuation, so resuming never replays preceding statements.
    Generator(Box<ReplacementGenerator>),
    /// A lazy map or filter adapter around another admitted iterator value.
    Adapter(Box<ReplacementAdapter>),
    /// Values materialized by an admitted lazy generator consumer such as `.collect()`.
    ///
    /// This is deliberately distinct from [`Self::List`]: the latter remains the existing scalar-pair collection
    /// profile, while this variant makes the narrow generator consumer explicit and lets its scalar results be
    /// indexed without admitting general list execution.
    CollectedGenerator {
        elements: Vec<ReplacementValue>,
        next: usize,
    },
}

/// A stored closure or partial-callable environment.
///
/// Parameters, captures, and the closure body come exclusively from Body IR. A call creates a fresh local frame
/// from this immutable environment; mutable execution state can therefore never leak between invocations.
#[derive(Debug, Clone, PartialEq)]
pub struct ReplacementCallable {
    params: Vec<CallableParam>,
    captures: Vec<(LocalId, ReplacementValue)>,
    body: ClosureBody,
}

/// Deferred state for a replacement generator expression or generator function.
///
/// The frame contains the local bindings and nested-block continuation needed to stop at one `yield` and resume at
/// the following statement. It intentionally owns cloned Body-IR statements rather than consulting source or
/// generated Rust after construction.
#[derive(Debug, Clone, PartialEq)]
pub struct ReplacementGenerator {
    frame: GeneratorFrame,
    /// A named generator declaration contributes its Body-IR snapshot and runtime requirements only after polling
    /// starts; expression generators are already represented by their enclosing body's rvalue snapshot.
    named_body: Option<Body>,
    /// Stable evidence that one retained generator frame actually began direct execution.
    frame_evidence: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
struct GeneratorFrame {
    locals: BTreeMap<LocalId, ReplacementValue>,
    cursors: Vec<GeneratorCursor>,
    exhausted: bool,
    steps: usize,
}

#[derive(Debug, Clone, PartialEq)]
struct GeneratorCursor {
    statements: Vec<Statement>,
    next: usize,
    is_loop: bool,
}

/// A lazy adapter whose callback remains a stored Body-IR callable value.
#[derive(Debug, Clone, PartialEq)]
pub struct ReplacementAdapter {
    source: ReplacementValue,
    callback: ReplacementCallable,
    kind: ReplacementAdapterKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReplacementAdapterKind {
    Map,
    Filter,
}

impl GeneratorFrame {
    /// Start a deferred generator at the first statement of its already-lowered Body-IR program.
    fn new(locals: BTreeMap<LocalId, ReplacementValue>, statements: Vec<Statement>) -> Self {
        Self {
            locals,
            cursors: vec![GeneratorCursor::block(statements)],
            exhausted: false,
            steps: 0,
        }
    }

    /// Return the cumulative execution budget a resumed frame must inherit from its caller.
    ///
    /// A generator retains the last count it observed between polls, while its parent may execute other statements
    /// before polling it again. Resumption must therefore start from whichever count is greater; choosing the
    /// frame count alone would let deferred work reset the direct execution budget.
    fn resume_step_budget(&self, caller_steps: usize) -> usize {
        caller_steps.max(self.steps)
    }
}

impl GeneratorCursor {
    /// A one-shot nested block entered from an `if` branch or the generator root.
    fn block(statements: Vec<Statement>) -> Self {
        Self {
            statements,
            next: 0,
            is_loop: false,
        }
    }

    /// A normalized loop body that restarts only after its stored cursor reaches the end.
    fn loop_body(statements: Vec<Statement>) -> Self {
        Self {
            statements,
            next: 0,
            is_loop: true,
        }
    }
}

impl ReplacementValue {
    /// Render a deterministic source-observable result spelling for replacement receipts and CLI output.
    pub fn observable_text(&self) -> String {
        match self {
            Self::Int(value) => value.to_string(),
            Self::Bool(value) => value.to_string(),
            Self::Str(value) => value.clone(),
            Self::Float(value) => value.clone(),
            Self::Unit => constructor_name(ConstructorId::None).to_string(),
            Self::Range { next, end, step } => format!("range({next}, {end}, {step})"),
            Self::List { elements, .. } => format!(
                "[{}]",
                elements
                    .iter()
                    .map(Self::observable_text)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::Tuple(elements) => format!(
                "({})",
                elements
                    .iter()
                    .map(Self::observable_text)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::Callable(_) => "<callable>".to_string(),
            Self::Generator(_) => "<generator>".to_string(),
            Self::Adapter(_) => "<generator-adapter>".to_string(),
            Self::CollectedGenerator { elements, .. } => format!(
                "[{}]",
                elements
                    .iter()
                    .map(Self::observable_text)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }
}

/// One Body-IR place read observed while executing the replacement profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnershipRead {
    /// The original Incan source span of the statement containing the read.
    pub span: HirSourceSpan,
    /// The compiler-owned ownership fact the executor honored.
    pub fact: OwnershipFact,
    /// Whether lowering identified this read as the local's last use.
    pub last_use: bool,
}

/// Canonical, machine-readable rendering of one ownership read used in a replacement receipt.
///
/// This projection deliberately uses stable source offsets and fact labels rather than Rust `Debug` output, so
/// receipt identities and CLI reports remain stable when implementation-only derives or field formatting change.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct OwnershipReadProjection {
    /// Original Incan source span start byte offset.
    pub span_start: usize,
    /// Original Incan source span end byte offset.
    pub span_end: usize,
    /// Stable compiler-owned ownership fact label.
    pub fact: &'static str,
    /// Whether lowering marked this source read as its local's last use.
    pub last_use: bool,
}

/// Canonical, machine-readable rendering of one Body-IR runtime requirement used in a replacement receipt.
///
/// `requirement` is a stable semantic label, not the Rust `Debug` representation of the internal enum.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RuntimeRequirementProjection {
    /// Stable semantic label for the runtime requirement.
    pub requirement: String,
}

/// Successful replacement execution evidence for one free function.
#[derive(Debug, Clone, PartialEq)]
pub struct ReplacementExecution {
    /// The function's source-level return value.
    pub value: ReplacementValue,
    /// Deterministic Body-IR snapshot retained as proof of the consumed input.
    pub body_snapshot: String,
    /// Every ownership decision observed during execution, in execution order.
    pub ownership_reads: Vec<OwnershipRead>,
    /// Runtime/helper requirements carried through from the consumed Body IR.
    pub runtime_requirements: Vec<AbiV0RuntimeRequirement>,
    /// Content identity of the actual Body-IR snapshot, ownership facts, requirements, and observed result.
    pub output_identity: String,
}

/// A free-function execution that has passed the bounded #988 profile validator.
///
/// The capability retains the exact typed Body IR, source-level function name, and concrete arguments that were
/// validated. It lets selection/receipt code decide whether direct execution may proceed without rerunning profile
/// validation or allowing an unvalidated Body IR body to reach the executor.
pub struct ValidatedFreeFunctionExecution<'module, 'args> {
    module: &'module BodyIrModule,
    name: String,
    args: &'args [ReplacementValue],
}

impl ReplacementExecution {
    /// Return the stable ownership evidence bound into this execution's output identity and CLI report.
    #[must_use]
    pub fn ownership_evidence(&self) -> Vec<OwnershipReadProjection> {
        ownership_read_projection(&self.ownership_reads)
    }

    /// Return the stable runtime-requirement evidence bound into this execution's output identity and CLI report.
    #[must_use]
    pub fn runtime_requirement_evidence(&self) -> Vec<RuntimeRequirementProjection> {
        runtime_requirement_projection(&self.runtime_requirements)
    }
}

/// A visible refusal or runtime outcome from the replacement executor.
#[derive(Debug, thiserror::Error)]
pub enum ReplacementExecutionError {
    /// The requested free function was absent from the typed Body-IR module.
    #[error("replacement backend has no free function named `{name}` to execute")]
    MissingFunction {
        /// The requested source-level function name.
        name: String,
    },
    /// The caller supplied a different number of arguments than the Body IR declares.
    #[error("replacement backend cannot execute `{name}`: expected {expected} arguments, got {actual}")]
    ArgumentCount {
        /// Function selected for execution.
        name: String,
        /// Parameter count from Body IR.
        expected: usize,
        /// Argument count supplied by the caller.
        actual: usize,
    },
    /// A Body-IR construct lies outside the declared first replacement profile.
    #[error(
        "replacement backend does not support {description} at original Incan source span {span_start}..{span_end}"
    )]
    Unsupported {
        /// Construct or semantic fact the first profile cannot execute.
        description: String,
        /// Original Incan source span carried by Body IR.
        span: HirSourceSpan,
        /// Start byte offset duplicated for typed error formatting.
        span_start: usize,
        /// End byte offset duplicated for typed error formatting.
        span_end: usize,
    },
    /// A selected operation reached a source-observable runtime failure.
    #[error("replacement backend runtime failure at original Incan source span {span_start}..{span_end}: {detail}")]
    RuntimeFailure {
        /// Source-observable runtime-failure description.
        detail: String,
        /// Original Incan source span carried by Body IR.
        span: HirSourceSpan,
        /// Start byte offset duplicated for typed error formatting.
        span_start: usize,
        /// End byte offset duplicated for typed error formatting.
        span_end: usize,
    },
}

impl ReplacementExecutionError {
    /// Construct a typed, source-span-preserving refusal for an unsupported source-profile boundary.
    #[must_use]
    pub fn unsupported_profile(description: impl Into<String>, span: HirSourceSpan) -> Self {
        unsupported(description, span)
    }

    /// Return the stable diagnostic code for this replacement outcome.
    pub const fn diagnostic_code(&self) -> &'static str {
        match self {
            Self::MissingFunction { .. } | Self::ArgumentCount { .. } => "INCAN-R988-ENTRYPOINT",
            Self::Unsupported { .. } => "INCAN-R988-UNSUPPORTED",
            Self::RuntimeFailure { .. } => "INCAN-R988-RUNTIME",
        }
    }

    /// Return the original Incan source location when this outcome arose from Body IR.
    pub const fn primary_span(&self) -> Option<HirSourceSpan> {
        match self {
            Self::Unsupported { span, .. } | Self::RuntimeFailure { span, .. } => Some(*span),
            Self::MissingFunction { .. } | Self::ArgumentCount { .. } => None,
        }
    }
}

/// Validate and prepare one Body-IR free function for later direct execution.
///
/// This side-effect-free boundary lets callers route availability through #986 selection and receipt logic before
/// committing to execution. Only [`execute_prevalidated_free_function`] can consume the returned capability.
pub fn prepare_free_function_execution<'module, 'args>(
    module: &'module BodyIrModule,
    name: &str,
    args: &'args [ReplacementValue],
) -> Result<ValidatedFreeFunctionExecution<'module, 'args>, ReplacementExecutionError> {
    let body = named_free_function(module, name)?;
    if body.is_generator() {
        return Err(unsupported("generator body", body.span));
    }
    if args.len() > body.params.len() {
        return Err(ReplacementExecutionError::ArgumentCount {
            name: name.to_string(),
            expected: body.params.len(),
            actual: args.len(),
        });
    }
    validate_scalar_arguments(args, body.span)?;
    validate_direct_body_profile(body)?;
    Ok(ValidatedFreeFunctionExecution {
        module,
        name: name.to_string(),
        args,
    })
}

/// Validate every direct-execution invariant of one body before it is executed or stored as a lazy frame.
///
/// The selected entrypoint and every same-module named callee use this one gate. Applying it only at the entrypoint
/// would let an otherwise admitted call dispatch an unvalidated sibling body and publish a receipt for a profile the
/// runtime promises to refuse.
fn validate_direct_body_profile(body: &Body) -> Result<(), ReplacementExecutionError> {
    // An `async def` produces an awaitable even when its body has no explicit `await`. Executing its statements as
    // an ordinary scalar body would erase task construction, suspension, wake, cancellation, and receipt semantics
    // that belong to #1155. The stored declaration fact is therefore a direct profile boundary, not something this
    // executor may infer by scanning the block for an await statement.
    if body.is_async {
        return Err(unsupported("async task body", body.span));
    }
    validate_binding_identity(body)?;
    let range_iterator_locals = range_iterator_locals(&body.block);
    validate_collection_local_types(body, &body.block.stmts, &range_iterator_locals)?;
    validate_nested_structural_aggregate_types(body)?;
    let tuple_iteration_locals = builtin_iteration_destinations(&body.block);
    let scalar_tuple_collection_locals = scalar_tuple_collection_elements(&body.block);
    validate_callable_params_profile(&body.params)?;
    if body.is_generator() {
        validate_generator_statements_profile(
            &body.block.stmts,
            &tuple_iteration_locals,
            &scalar_tuple_collection_locals,
        )
    } else {
        validate_block_profile(&body.block, &tuple_iteration_locals, &scalar_tuple_collection_locals)
    }
}

/// Execute one named, free-function Body IR body with concrete scalar arguments.
///
/// The caller must already have parsed and typechecked the source before constructing `module`; this boundary only
/// consumes Body IR and refuses unsupported operations rather than rerunning frontend or generated-Rust semantics.
pub fn execute_free_function(
    module: &BodyIrModule,
    name: &str,
    args: &[ReplacementValue],
) -> Result<ReplacementExecution, ReplacementExecutionError> {
    let execution = prepare_free_function_execution(module, name, args)?;
    execute_prevalidated_free_function(execution)
}

/// Execute one free function that has already passed [`prepare_free_function_execution`].
///
/// This consumes the validated capability, preserving the rule that callers select the source profile before the
/// direct Body-IR executor observes a result.
pub fn execute_prevalidated_free_function(
    execution: ValidatedFreeFunctionExecution<'_, '_>,
) -> Result<ReplacementExecution, ReplacementExecutionError> {
    let body = named_free_function(execution.module, &execution.name)?;

    let mut executor = BodyExecutor::new(execution.module, body, execution.args)?;
    let flow = executor.execute_block(&body.block)?;
    let value = match flow {
        Flow::Return(value) => match value {
            Some(value) => value,
            None => ReplacementValue::Unit,
        },
        Flow::Next => ReplacementValue::Unit,
        Flow::Break | Flow::Continue => {
            return Err(unsupported("loop control outside a normalized loop", body.span));
        }
    };
    ensure_scalar_result(&value, body.span)?;
    let body_snapshot = executor.body_snapshot();
    let ownership_summary = canonical_ownership_summary(&executor.ownership_reads);
    let requirements_summary = canonical_runtime_requirements_summary(&executor.runtime_requirements);
    let output_identity = digest_output(&[
        body_snapshot.as_str(),
        value.observable_text().as_str(),
        ownership_summary.as_str(),
        requirements_summary.as_str(),
    ]);
    Ok(ReplacementExecution {
        value,
        body_snapshot,
        ownership_reads: executor.ownership_reads,
        runtime_requirements: executor.runtime_requirements,
        output_identity,
    })
}

/// Locate the requested free-function body without inventing a fallback entrypoint.
fn named_free_function<'a>(module: &'a BodyIrModule, name: &str) -> Result<&'a Body, ReplacementExecutionError> {
    module
        .bodies
        .iter()
        .find(|body| body.name == name)
        .ok_or_else(|| ReplacementExecutionError::MissingFunction { name: name.to_string() })
}

/// Reject non-scalar direct API arguments before they can widen the first replacement profile.
fn validate_scalar_arguments(args: &[ReplacementValue], span: HirSourceSpan) -> Result<(), ReplacementExecutionError> {
    for argument in args {
        if !matches!(
            argument,
            ReplacementValue::Int(_) | ReplacementValue::Bool(_) | ReplacementValue::Str(_) | ReplacementValue::Unit
        ) {
            return Err(unsupported(
                format!("{} argument in the scalar replacement profile", value_kind(argument)),
                span,
            ));
        }
    }
    Ok(())
}

/// Refuse repeated user-binding spellings until Body IR carries an explicit binding-equivalence fact.
///
/// A local id is sufficient to address an already-selected value, but it is not enough to prove that every later
/// source read saw a reassignment rather than a fresh shadowing declaration. This runtime therefore keeps the
/// existing fail-closed boundary and does not repair that source-representation gap here.
fn validate_binding_identity(body: &Body) -> Result<(), ReplacementExecutionError> {
    let mut declared = BTreeMap::new();
    for local in &body.locals {
        if !matches!(local.origin, LocalOrigin::UserBinding) {
            continue;
        }
        let Some(name) = local.name.as_deref() else {
            continue;
        };
        if declared.insert(name, local.span).is_some() {
            return Err(unsupported(
                format!(
                    "repeated user binding `{name}` (lexical shadowing or reassignment); Body IR does not yet carry binding-equivalence facts for direct execution"
                ),
                local.span,
            ));
        }
    }
    Ok(())
}

/// Validate the stored call-time default contracts without consulting source or declaration structures.
fn validate_callable_params_profile(params: &[CallableParam]) -> Result<(), ReplacementExecutionError> {
    let mut locals = BTreeSet::new();
    for parameter in params {
        if !locals.insert(parameter.local) {
            return Err(unsupported("duplicate callable parameter local", parameter.span));
        }
        if let CallableParamDefault::Source(computation) = &parameter.default {
            for statement in &computation.stmts {
                validate_statement_profile(statement, &BTreeSet::new(), &BTreeSet::new())?;
            }
            validate_operand_profile(&computation.result, computation.span, &BTreeSet::new())?;
        }
    }
    Ok(())
}

/// Validate a stored closure/partial shape using its explicit capture and parameter contracts.
fn validate_closure_profile(
    params: &[CallableParam],
    captured_operands: &[Operand],
    body: &ClosureBody,
    span: HirSourceSpan,
    tuple_iteration_locals: &BTreeSet<LocalId>,
    scalar_tuple_collection_locals: &BTreeSet<LocalId>,
) -> Result<(), ReplacementExecutionError> {
    if captured_operands.len() != body.capture_locals.len() {
        return Err(unsupported("callable capture metadata mismatch", span));
    }
    validate_callable_params_profile(params)?;
    for operand in captured_operands {
        validate_operand_profile(operand, span, tuple_iteration_locals)?;
    }
    for statement in &body.stmts {
        validate_statement_profile(statement, tuple_iteration_locals, scalar_tuple_collection_locals)?;
    }
    validate_operand_profile(&body.result, span, tuple_iteration_locals)
}

/// Validate structural aggregate destinations and retain the narrower builtin-iteration type boundary.
///
/// Runtime operands alone cannot classify an empty aggregate. This pass therefore consumes the compiler-owned local
/// declaration type before execution: tuple and list aggregates may be recursively structural values, while the
/// existing builtin collection iteration profile remains restricted to scalar tuple pairs.
fn validate_collection_local_types(
    body: &Body,
    statements: &[Statement],
    range_iterator_locals: &BTreeSet<LocalId>,
) -> Result<(), ReplacementExecutionError> {
    for statement in statements {
        match &statement.kind {
            StatementKind::Assign {
                place,
                rvalue: Rvalue::Aggregate(AggregateKind::Tuple, _),
            }
            | StatementKind::Assign {
                place,
                rvalue: Rvalue::Aggregate(AggregateKind::List, _),
            } => validate_structural_aggregate_local_type(
                body,
                bare_local(place, statement.span)?,
                statement.span,
                "structural aggregate destination",
            )?,
            StatementKind::IterNext {
                destination,
                iterator: Operand::Place(iterator),
                protocol: IterProtocol::Builtin,
            } => {
                let iterator_local = bare_local(&iterator.place, statement.span)?;
                if range_iterator_locals.contains(&iterator_local) {
                    validate_range_iteration_local_types(body, destination, iterator_local, statement.span)?;
                } else {
                    validate_scalar_pair_tuple_local_type(
                        body,
                        bare_local(destination, statement.span)?,
                        statement.span,
                        "builtin collection iteration destination",
                    )?;
                    validate_scalar_pair_list_local_type(
                        body,
                        iterator_local,
                        statement.span,
                        "builtin collection iterator",
                    )?;
                }
            }
            StatementKind::If {
                then_block, else_block, ..
            } => {
                validate_collection_local_types(body, &then_block.stmts, range_iterator_locals)?;
                if let Some(else_block) = else_block {
                    validate_collection_local_types(body, &else_block.stmts, range_iterator_locals)?;
                }
            }
            StatementKind::Loop { body: loop_body } => {
                validate_collection_local_types(body, &loop_body.stmts, range_iterator_locals)?;
            }
            _ => {}
        }
    }
    Ok(())
}

/// Apply the compiler-owned aggregate type gate inside deferred callable and generator frames too.
///
/// Defaults, closures, and generators reuse their owning [`Body`] local-id space. Checking only the selected
/// body's ordinary block would let an empty `list[float]` pass the runtime's vacuous structural-value test before a
/// deferred frame executes it. This walk retains the declared local type as the authority without adding a second
/// runtime type model or looking back at source structures.
fn validate_nested_structural_aggregate_types(body: &Body) -> Result<(), ReplacementExecutionError> {
    validate_callable_param_aggregate_types(body, &body.params)?;
    validate_nested_aggregate_types_in_statements(body, &body.block.stmts)
}

/// Check source defaults on one callable surface and recurse into their deferred statements.
fn validate_callable_param_aggregate_types(
    body: &Body,
    params: &[CallableParam],
) -> Result<(), ReplacementExecutionError> {
    for parameter in params {
        let CallableParamDefault::Source(computation) = &parameter.default else {
            continue;
        };
        validate_structural_aggregate_types_in_statements(body, &computation.stmts)?;
        validate_nested_aggregate_types_in_statements(body, &computation.stmts)?;
    }
    Ok(())
}

/// Find deferred closure and generator frames nested below ordinary normalized statements.
fn validate_nested_aggregate_types_in_statements(
    body: &Body,
    statements: &[Statement],
) -> Result<(), ReplacementExecutionError> {
    for statement in statements {
        match &statement.kind {
            StatementKind::Assign { rvalue, .. } => validate_nested_aggregate_types_in_rvalue(body, rvalue)?,
            StatementKind::If {
                then_block, else_block, ..
            } => {
                validate_nested_aggregate_types_in_statements(body, &then_block.stmts)?;
                if let Some(else_block) = else_block {
                    validate_nested_aggregate_types_in_statements(body, &else_block.stmts)?;
                }
            }
            StatementKind::Loop { body: loop_body } => {
                validate_nested_aggregate_types_in_statements(body, &loop_body.stmts)?;
            }
            _ => {}
        }
    }
    Ok(())
}

/// Check each deferred rvalue that can contain normalized assignments under the owning body type environment.
fn validate_nested_aggregate_types_in_rvalue(body: &Body, rvalue: &Rvalue) -> Result<(), ReplacementExecutionError> {
    match rvalue {
        Rvalue::Closure {
            params, body: closure, ..
        } => {
            validate_callable_param_aggregate_types(body, params)?;
            validate_structural_aggregate_types_in_statements(body, &closure.stmts)?;
            validate_nested_aggregate_types_in_statements(body, &closure.stmts)
        }
        Rvalue::Generator { body: generator, .. } => {
            validate_structural_aggregate_types_in_statements(body, &generator.stmts)?;
            validate_nested_aggregate_types_in_statements(body, &generator.stmts)
        }
        _ => Ok(()),
    }
}

/// Validate only aggregate destinations in a deferred frame, preserving its own iteration profile.
///
/// Generator frames use a deliberately different `IterNext` type contract from ordinary bodies. Deferred aggregate
/// checks therefore share the same compiler-owned local type rule without accidentally applying the enclosing
/// body's scalar-pair collection-iteration restriction to generator-local iterator values.
fn validate_structural_aggregate_types_in_statements(
    body: &Body,
    statements: &[Statement],
) -> Result<(), ReplacementExecutionError> {
    for statement in statements {
        match &statement.kind {
            StatementKind::Assign {
                place,
                rvalue: Rvalue::Aggregate(AggregateKind::Tuple | AggregateKind::List, _),
            } => validate_structural_aggregate_local_type(
                body,
                bare_local(place, statement.span)?,
                statement.span,
                "structural aggregate destination",
            )?,
            StatementKind::If {
                then_block, else_block, ..
            } => {
                validate_structural_aggregate_types_in_statements(body, &then_block.stmts)?;
                if let Some(else_block) = else_block {
                    validate_structural_aggregate_types_in_statements(body, &else_block.stmts)?;
                }
            }
            StatementKind::Loop { body: loop_body } => {
                validate_structural_aggregate_types_in_statements(body, &loop_body.stmts)?;
            }
            _ => {}
        }
    }
    Ok(())
}

/// Validate one aggregate destination against the recursive tuple/list value vocabulary.
fn validate_structural_aggregate_local_type(
    body: &Body,
    local: LocalId,
    span: HirSourceSpan,
    role: &str,
) -> Result<(), ReplacementExecutionError> {
    let ty = declared_local_type(body, local, span)?;
    if is_direct_structural_type(ty) {
        Ok(())
    } else {
        Err(unsupported(format!("{role} has unsupported Body-IR type `{ty}`"), span))
    }
}

/// Collect builtin `range` iterator locals from explicit Body-IR builtin targets.
///
/// A same-module declaration named `range` now carries a [`NamedCallableTarget::direct_call_id`] and dispatches to
/// that declaration, while the compiler-recognized builtin carries [`NamedCallableBuiltin::Range`]. This keeps the
/// existing bounded builtin rule without guessing from a source spelling or treating an imported callable as range.
fn range_iterator_locals(block: &incan_semantics_core::body_ir::Block) -> BTreeSet<LocalId> {
    let mut locals = BTreeSet::new();
    while collect_range_iterator_locals(block, &mut locals) {}
    locals
}

/// Extend the admitted `range` source-spelling aliases until the enclosing body reaches a fixed point.
fn collect_range_iterator_locals(
    block: &incan_semantics_core::body_ir::Block,
    range_locals: &mut BTreeSet<LocalId>,
) -> bool {
    let mut changed = false;
    for statement in &block.stmts {
        match &statement.kind {
            StatementKind::Call {
                destination: Some(destination),
                callee: Callee::Function(CallableTarget::Named(target)),
                ..
            } if is_explicit_range_builtin(target) && destination.projection.is_empty() => {
                changed |= range_locals.insert(destination.local);
            }
            StatementKind::Assign {
                place,
                rvalue: Rvalue::Use(Operand::Place(source)),
            } if place.projection.is_empty()
                && source.place.projection.is_empty()
                && range_locals.contains(&source.place.local) =>
            {
                changed |= range_locals.insert(place.local);
            }
            StatementKind::If {
                then_block, else_block, ..
            } => {
                changed |= collect_range_iterator_locals(then_block, range_locals);
                if let Some(else_block) = else_block {
                    changed |= collect_range_iterator_locals(else_block, range_locals);
                }
            }
            StatementKind::Loop { body } => {
                changed |= collect_range_iterator_locals(body, range_locals);
            }
            _ => {}
        }
    }
    changed
}

/// Validate the compiler-owned local types of a preserved scalar `range` source-spelling iteration.
fn validate_range_iteration_local_types(
    body: &Body,
    destination: &Place,
    iterator: LocalId,
    span: HirSourceSpan,
) -> Result<(), ReplacementExecutionError> {
    let destination = bare_local(destination, span)?;
    let destination_ty = declared_local_type(body, destination, span)?;
    if !is_int_type(destination_ty) {
        return Err(unsupported(
            format!("range iteration destination has Body-IR type `{destination_ty}`, not int"),
            span,
        ));
    }
    let iterator_ty = declared_local_type(body, iterator, span)?;
    if is_range_iterator_type(iterator_ty) {
        Ok(())
    } else {
        Err(unsupported(
            format!("range iteration iterator has Body-IR type `{iterator_ty}`, not list[int]"),
            span,
        ))
    }
}

/// Validate that a Body-IR local has the only tuple type admitted as a replacement collection element.
fn validate_scalar_pair_tuple_local_type(
    body: &Body,
    local: LocalId,
    span: HirSourceSpan,
    role: &str,
) -> Result<(), ReplacementExecutionError> {
    let ty = declared_local_type(body, local, span)?;
    if is_scalar_pair_tuple_type(ty) {
        Ok(())
    } else {
        Err(unsupported(
            format!("{role} has Body-IR type `{ty}`, not tuple[scalar, scalar]"),
            span,
        ))
    }
}

/// Validate that a Body-IR local has the only list type admitted by the replacement collection profile.
fn validate_scalar_pair_list_local_type(
    body: &Body,
    local: LocalId,
    span: HirSourceSpan,
    role: &str,
) -> Result<(), ReplacementExecutionError> {
    let ty = declared_local_type(body, local, span)?;
    if is_scalar_pair_list_type(ty) {
        Ok(())
    } else {
        Err(unsupported(
            format!("{role} has Body-IR type `{ty}`, not list[tuple[scalar, scalar]]"),
            span,
        ))
    }
}

/// Return a local's compiler-owned type or refuse malformed Body IR at the owning source span.
fn declared_local_type(
    body: &Body,
    local: LocalId,
    span: HirSourceSpan,
) -> Result<&IncanType, ReplacementExecutionError> {
    body.locals
        .iter()
        .find(|declaration| declaration.id == local)
        .map(|declaration| &declaration.ty)
        .ok_or_else(|| unsupported("Body-IR local without a declared type", span))
}

/// Return whether a type is a two-element tuple of scalar shapes the replacement runtime already supports.
fn is_scalar_pair_tuple_type(ty: &IncanType) -> bool {
    match ty {
        IncanType::Tuple(elements) => {
            matches!(elements.as_slice(), [left, right] if is_collection_scalar_type(left) && is_collection_scalar_type(right))
        }
        IncanType::Generic { base, args } if collections::from_str(base) == Some(CollectionTypeId::Tuple) => {
            matches!(args.as_slice(), [left, right] if is_collection_scalar_type(left) && is_collection_scalar_type(right))
        }
        _ => false,
    }
}

/// Return whether a type is `list[tuple[scalar, scalar]]` according to the canonical collection registry.
fn is_scalar_pair_list_type(ty: &IncanType) -> bool {
    matches!(
        ty,
        IncanType::Generic { base, args }
            if collections::from_str(base) == Some(CollectionTypeId::List)
                && matches!(args.as_slice(), [element] if is_scalar_pair_tuple_type(element))
    )
}

/// Return whether a type is the compiler's list representation for the profile's `range` iterator.
fn is_range_iterator_type(ty: &IncanType) -> bool {
    matches!(
        ty,
        IncanType::Generic { base, args }
            if collections::from_str(base) == Some(CollectionTypeId::List)
                && matches!(args.as_slice(), [element] if is_int_type(element))
    )
}

/// Return whether a type is the integer scalar used by the selected `range` loop lowering.
const fn is_int_type(ty: &IncanType) -> bool {
    matches!(ty, IncanType::Primitive(IncanPrimitiveType::Int))
}

/// Return whether a type is a scalar shape the selected collection runtime can materialize and project.
fn is_collection_scalar_type(ty: &IncanType) -> bool {
    matches!(
        ty,
        IncanType::Primitive(
            IncanPrimitiveType::Int | IncanPrimitiveType::Bool | IncanPrimitiveType::Str | IncanPrimitiveType::Unit
        )
    )
}

/// Return whether `ty` has the source-local recursively structural tuple/list shape this runtime materializes.
fn is_direct_structural_type(ty: &IncanType) -> bool {
    if is_collection_scalar_type(ty) {
        return true;
    }
    match ty {
        IncanType::Tuple(elements) => elements.iter().all(is_direct_structural_type),
        IncanType::Generic { base, args }
            if matches!(
                collections::from_str(base),
                Some(CollectionTypeId::Tuple | CollectionTypeId::List)
            ) =>
        {
            args.iter().all(is_direct_structural_type)
        }
        _ => false,
    }
}

/// Return whether Body IR explicitly identified this target as the compiler-owned `range` builtin.
///
/// A source spelling or absent same-module declaration identity is not enough: imported and unresolved callables
/// retain the latter too, and must refuse rather than borrowing the builtin's execution rule.
fn is_explicit_range_builtin(target: &NamedCallableTarget) -> bool {
    target.direct_call_id.is_none() && target.builtin == Some(NamedCallableBuiltin::Range)
}

/// Collect the local identities written by builtin collection polling across one normalized body.
///
/// Only these compiler-created item locals may later be projected as a selected scalar tuple element. This keeps a
/// standalone tuple field access outside the profile even though it uses the same `PlaceElem::Field` representation.
fn builtin_iteration_destinations(block: &incan_semantics_core::body_ir::Block) -> BTreeSet<LocalId> {
    let mut destinations = BTreeSet::new();
    collect_builtin_iteration_destinations(block, &mut destinations);
    destinations
}

/// Recurse through normalized control flow to collect every builtin iteration destination local.
fn collect_builtin_iteration_destinations(
    block: &incan_semantics_core::body_ir::Block,
    destinations: &mut BTreeSet<LocalId>,
) {
    for statement in &block.stmts {
        match &statement.kind {
            StatementKind::If {
                then_block, else_block, ..
            } => {
                collect_builtin_iteration_destinations(then_block, destinations);
                if let Some(else_block) = else_block {
                    collect_builtin_iteration_destinations(else_block, destinations);
                }
            }
            StatementKind::Loop { body } => collect_builtin_iteration_destinations(body, destinations),
            StatementKind::IterNext {
                destination,
                protocol: IterProtocol::Builtin,
                ..
            } => {
                destinations.insert(destination.local);
            }
            _ => {}
        }
    }
}

/// Collect the tuple locals that are direct elements of a source-local list aggregate.
///
/// Body IR lowers each tuple literal before the surrounding list aggregate, so a replacement profile cannot infer
/// this relationship from a single rvalue. Intersecting tuple-assignment destinations with direct list operands
/// makes that lowering relationship explicit and prevents standalone tuples or scalar lists from slipping through
/// as unobserved runtime values.
fn scalar_tuple_collection_elements(block: &incan_semantics_core::body_ir::Block) -> BTreeSet<LocalId> {
    let mut tuple_destinations = BTreeSet::new();
    let mut list_operands = BTreeSet::new();
    collect_scalar_tuple_collection_locals(block, &mut tuple_destinations, &mut list_operands);
    tuple_destinations.intersection(&list_operands).copied().collect()
}

/// Recurse through control flow to collect tuple assignments and direct list-aggregate operands.
fn collect_scalar_tuple_collection_locals(
    block: &incan_semantics_core::body_ir::Block,
    tuple_destinations: &mut BTreeSet<LocalId>,
    list_operands: &mut BTreeSet<LocalId>,
) {
    for statement in &block.stmts {
        match &statement.kind {
            StatementKind::Assign {
                place,
                rvalue: Rvalue::Aggregate(AggregateKind::Tuple, _),
            } if place.projection.is_empty() => {
                tuple_destinations.insert(place.local);
            }
            StatementKind::Assign {
                rvalue: Rvalue::Aggregate(AggregateKind::List, operands),
                ..
            } => {
                for operand in operands {
                    if let Operand::Place(place_operand) = operand
                        && place_operand.place.projection.is_empty()
                    {
                        list_operands.insert(place_operand.place.local);
                    }
                }
            }
            StatementKind::If {
                then_block, else_block, ..
            } => {
                collect_scalar_tuple_collection_locals(then_block, tuple_destinations, list_operands);
                if let Some(else_block) = else_block {
                    collect_scalar_tuple_collection_locals(else_block, tuple_destinations, list_operands);
                }
            }
            StatementKind::Loop { body } => {
                collect_scalar_tuple_collection_locals(body, tuple_destinations, list_operands);
            }
            _ => {}
        }
    }
}

/// Validate every statement in one normalized Body-IR block before the direct executor starts.
fn validate_block_profile(
    block: &incan_semantics_core::body_ir::Block,
    tuple_iteration_locals: &BTreeSet<LocalId>,
    scalar_tuple_collection_locals: &BTreeSet<LocalId>,
) -> Result<(), ReplacementExecutionError> {
    for statement in &block.stmts {
        validate_statement_profile(statement, tuple_iteration_locals, scalar_tuple_collection_locals)?;
    }
    Ok(())
}

/// Validate one statement against the deliberately narrow #988 direct-execution profile.
fn validate_statement_profile(
    statement: &Statement,
    tuple_iteration_locals: &BTreeSet<LocalId>,
    scalar_tuple_collection_locals: &BTreeSet<LocalId>,
) -> Result<(), ReplacementExecutionError> {
    match &statement.kind {
        StatementKind::Assign { place, rvalue } => {
            validate_write_place(place, statement.span, tuple_iteration_locals)?;
            validate_rvalue_profile(
                rvalue,
                statement.span,
                tuple_iteration_locals,
                scalar_tuple_collection_locals,
                place.projection.is_empty().then_some(place.local),
            )
        }
        StatementKind::Call {
            destination,
            callee,
            args,
            may_panic: _,
        } => {
            let destination = destination
                .as_ref()
                .ok_or_else(|| unsupported("discarded call result", statement.span))?;
            validate_bare_local(destination, statement.span)?;
            validate_call_profile(callee, args, statement.span, tuple_iteration_locals)
        }
        StatementKind::Drop { .. } => Ok(()),
        StatementKind::If {
            cond,
            then_block,
            else_block,
        } => {
            validate_operand_profile(cond, statement.span, tuple_iteration_locals)?;
            validate_block_profile(then_block, tuple_iteration_locals, scalar_tuple_collection_locals)?;
            if let Some(else_block) = else_block {
                validate_block_profile(else_block, tuple_iteration_locals, scalar_tuple_collection_locals)?;
            }
            Ok(())
        }
        StatementKind::Loop { body } => {
            validate_block_profile(body, tuple_iteration_locals, scalar_tuple_collection_locals)
        }
        StatementKind::Break { value: Some(_) } => Err(unsupported("value-carrying loop break", statement.span)),
        StatementKind::Break { value: None } | StatementKind::Continue => Ok(()),
        StatementKind::Return { value } => value.as_ref().map_or(Ok(()), |value| {
            validate_operand_profile(value, statement.span, tuple_iteration_locals)
        }),
        StatementKind::Assert {
            cond,
            message,
            may_panic: _,
        } => {
            validate_operand_profile(cond, statement.span, tuple_iteration_locals)?;
            message.as_ref().map_or(Ok(()), |message| {
                validate_operand_profile(message, statement.span, tuple_iteration_locals)
            })
        }
        StatementKind::Expr { value } => validate_operand_profile(value, statement.span, tuple_iteration_locals),
        StatementKind::IterNext {
            destination,
            iterator,
            protocol: IterProtocol::Builtin,
        } => {
            validate_bare_local(destination, statement.span)?;
            validate_operand_profile(iterator, statement.span, tuple_iteration_locals)
        }
        StatementKind::Yield { .. } => Err(unsupported("generator yield", statement.span)),
        // #1164 gave Body IR an async vocabulary. Executing it -- task state, suspension, wake/resume, arm
        // selection, cancellation -- is #1155's. Until then these refuse by name at the original source span
        // rather than leaving the executor unable to compile against the representation.
        StatementKind::Await { .. } => Err(unsupported("async await suspension", statement.span)),
        StatementKind::Race { .. } => Err(unsupported("async race selection", statement.span)),
        StatementKind::TryPropagate { .. } => Err(unsupported("try propagation", statement.span)),
        StatementKind::IterNext { .. } => Err(unsupported("non-range iteration", statement.span)),
        StatementKind::Unsupported { description } => Err(unsupported(description, statement.span)),
    }
}

/// Validate one Body-IR call before the executor dispatches it.
fn validate_call_profile(
    callee: &Callee,
    args: &[Operand],
    span: HirSourceSpan,
    tuple_iteration_locals: &BTreeSet<LocalId>,
) -> Result<(), ReplacementExecutionError> {
    let supported = match callee {
        Callee::Helper(HelperOp::StrConcat) => true,
        // Named calls remain direct module dispatches. Their target/binding facts are Body-IR values, not a source
        // lookup reconstructed by this executor.
        Callee::Function(CallableTarget::Named(target)) if is_explicit_range_builtin(target) => true,
        Callee::Function(CallableTarget::Named(target)) => {
            target.direct_call_id.is_some()
                && target.builtin.is_none()
                && validate_argument_binding_profile(&target.binding)
        }
        Callee::Function(CallableTarget::Local(target)) => {
            validate_operand_profile(&Operand::Place(target.operand.clone()), span, tuple_iteration_locals)?;
            validate_argument_binding_profile(&target.binding)
        }
        Callee::Method(target) if target.name == "collect" => args.len() == 1,
        // The compiler currently records the iterator-adapter receiver and callback in source order but leaves
        // their stdlib method signature as `UnresolvedPositional`. That is sufficient for this deliberately
        // positional two-argument profile: neither adapter has named arguments or callable defaults to bind.
        Callee::Method(target) if matches!(target.name.as_str(), "map" | "filter") => {
            args.len() == 2
                && match &target.binding {
                    ArgumentBinding::UnresolvedPositional => true,
                    binding @ ArgumentBinding::Resolved { .. } => validate_argument_binding_profile(binding),
                }
        }
        Callee::Helper(_) => false,
        _ => false,
    };
    if !supported {
        return Err(unsupported(format!("call to {}", callee_label(callee)), span));
    }
    for arg in args {
        validate_operand_profile(arg, span, tuple_iteration_locals)?;
    }
    Ok(())
}

/// Validate only the structural facts every direct callable dispatcher can enforce before execution.
fn validate_argument_binding_profile(binding: &ArgumentBinding) -> bool {
    let ArgumentBinding::Resolved { arguments, .. } = binding else {
        return false;
    };
    let mut slots = BTreeSet::new();
    let mut written_positions = BTreeSet::new();
    arguments
        .iter()
        .all(|argument| slots.insert(argument.slot) && written_positions.insert(argument.written_position))
}

/// Validate one rvalue before it can be evaluated by the bounded executor.
fn validate_rvalue_profile(
    rvalue: &Rvalue,
    span: HirSourceSpan,
    tuple_iteration_locals: &BTreeSet<LocalId>,
    scalar_tuple_collection_locals: &BTreeSet<LocalId>,
    destination: Option<LocalId>,
) -> Result<(), ReplacementExecutionError> {
    match rvalue {
        Rvalue::Use(operand) | Rvalue::UnaryOp(_, operand) => {
            validate_operand_profile(operand, span, tuple_iteration_locals)
        }
        Rvalue::BinaryOp(_, left, right) => {
            validate_operand_profile(left, span, tuple_iteration_locals)?;
            validate_operand_profile(right, span, tuple_iteration_locals)
        }
        Rvalue::Aggregate(kind, operands) => validate_aggregate_profile(
            kind,
            operands,
            span,
            tuple_iteration_locals,
            scalar_tuple_collection_locals,
            destination,
        ),
        Rvalue::Format(_) => Err(unsupported("f-string", span)),
        Rvalue::Closure {
            params,
            captured_operands,
            body,
        } => validate_closure_profile(
            params,
            captured_operands,
            body,
            span,
            tuple_iteration_locals,
            scalar_tuple_collection_locals,
        ),
        Rvalue::Generator {
            source,
            captured_operands,
            body,
        } => {
            validate_operand_profile(source, span, tuple_iteration_locals)?;
            for operand in captured_operands {
                validate_operand_profile(operand, span, tuple_iteration_locals)?;
            }
            validate_generator_body_profile(body, tuple_iteration_locals, scalar_tuple_collection_locals)
        }
        Rvalue::Match { .. } => Err(unsupported("match expression", span)),
    }
}

/// Validate the deferred body carried by an admitted generator-expression rvalue.
///
/// A normal function body still refuses [`StatementKind::Yield`]. Within this explicitly stored generator body,
/// however, `yield` is exactly the deferred result boundary and is interpreted only by `.collect()` below.
fn validate_generator_body_profile(
    body: &GeneratorBody,
    tuple_iteration_locals: &BTreeSet<LocalId>,
    scalar_tuple_collection_locals: &BTreeSet<LocalId>,
) -> Result<(), ReplacementExecutionError> {
    validate_generator_statements_profile(&body.stmts, tuple_iteration_locals, scalar_tuple_collection_locals)
}

/// Recurse through the generator's normalized control flow while admitting only its terminal yields.
fn validate_generator_statements_profile(
    statements: &[Statement],
    tuple_iteration_locals: &BTreeSet<LocalId>,
    scalar_tuple_collection_locals: &BTreeSet<LocalId>,
) -> Result<(), ReplacementExecutionError> {
    for statement in statements {
        match &statement.kind {
            StatementKind::Yield { value } => {
                validate_operand_profile(value, statement.span, tuple_iteration_locals)?;
            }
            StatementKind::If {
                cond,
                then_block,
                else_block,
            } => {
                validate_operand_profile(cond, statement.span, tuple_iteration_locals)?;
                validate_generator_statements_profile(
                    &then_block.stmts,
                    tuple_iteration_locals,
                    scalar_tuple_collection_locals,
                )?;
                if let Some(else_block) = else_block {
                    validate_generator_statements_profile(
                        &else_block.stmts,
                        tuple_iteration_locals,
                        scalar_tuple_collection_locals,
                    )?;
                }
            }
            StatementKind::Loop { body } => validate_generator_statements_profile(
                &body.stmts,
                tuple_iteration_locals,
                scalar_tuple_collection_locals,
            )?,
            StatementKind::Return { .. } => {
                return Err(unsupported("return from a generator expression", statement.span));
            }
            _ => validate_statement_profile(statement, tuple_iteration_locals, scalar_tuple_collection_locals)?,
        }
    }
    Ok(())
}

/// Validate the source-local tuple/list aggregate vocabulary without admitting dict or set semantics.
fn validate_aggregate_profile(
    kind: &AggregateKind,
    operands: &[Operand],
    span: HirSourceSpan,
    tuple_iteration_locals: &BTreeSet<LocalId>,
    _scalar_tuple_collection_locals: &BTreeSet<LocalId>,
    _destination: Option<LocalId>,
) -> Result<(), ReplacementExecutionError> {
    match kind {
        AggregateKind::Tuple | AggregateKind::List => {
            for operand in operands {
                validate_operand_profile(operand, span, tuple_iteration_locals)?;
            }
            Ok(())
        }
        _ => Err(unsupported(format!("{} aggregate", aggregate_label(kind)), span)),
    }
}

/// Validate a single operand's place shape and compiler-owned ownership decision.
fn validate_operand_profile(
    operand: &Operand,
    span: HirSourceSpan,
    tuple_iteration_locals: &BTreeSet<LocalId>,
) -> Result<(), ReplacementExecutionError> {
    let Operand::Place(place_operand) = operand else {
        return Ok(());
    };
    validate_read_place(&place_operand.place, span, tuple_iteration_locals)?;
    if matches!(place_operand.fact, OwnershipFact::Unknown) {
        return Err(unsupported("unknown ownership fact", span));
    }
    Ok(())
}

/// Reject non-local assignments at the profile boundary while preserving the statement's source authority.
fn validate_bare_local(place: &Place, span: HirSourceSpan) -> Result<(), ReplacementExecutionError> {
    bare_local(place, span).map(|_| ())
}

/// Admit one-level numeric tuple fields and one-level indexes over source-local structural values.
fn validate_read_place(
    place: &Place,
    span: HirSourceSpan,
    tuple_iteration_locals: &BTreeSet<LocalId>,
) -> Result<(), ReplacementExecutionError> {
    match place.projection.as_slice() {
        [] => Ok(()),
        [PlaceElem::Field(field)] if field.parse::<usize>().is_ok() => Ok(()),
        [PlaceElem::Field(_)] => Err(unsupported("non-numeric tuple field projection", span)),
        [PlaceElem::Index(index)] => validate_operand_profile(index, span, tuple_iteration_locals),
        [PlaceElem::Slice { .. }] => Err(unsupported("slice projection", span)),
        _ => Err(unsupported("nested place projection", span)),
    }
}

/// Admit a bare assignment or a one-level mutable list-index assignment.
fn validate_write_place(
    place: &Place,
    span: HirSourceSpan,
    tuple_iteration_locals: &BTreeSet<LocalId>,
) -> Result<(), ReplacementExecutionError> {
    match place.projection.as_slice() {
        [] => Ok(()),
        [PlaceElem::Index(index)] => validate_operand_profile(index, span, tuple_iteration_locals),
        [PlaceElem::Field(_)] => Err(unsupported("tuple field assignment", span)),
        [PlaceElem::Slice { .. }] => Err(unsupported("slice assignment", span)),
        _ => Err(unsupported("nested place assignment", span)),
    }
}

/// Mutable interpreter state for one Body-IR execution.
struct BodyExecutor {
    module: BodyIrModule,
    locals: BTreeMap<LocalId, ReplacementValue>,
    ownership_reads: Vec<OwnershipRead>,
    runtime_requirements: Vec<AbiV0RuntimeRequirement>,
    body_snapshots: Vec<String>,
    steps: usize,
}

impl BodyExecutor {
    /// Bind the already-typechecked call arguments to their Body-IR parameter locals.
    fn new(module: &BodyIrModule, body: &Body, args: &[ReplacementValue]) -> Result<Self, ReplacementExecutionError> {
        let mut executor = Self {
            module: module.clone(),
            locals: BTreeMap::new(),
            ownership_reads: Vec::new(),
            runtime_requirements: Vec::new(),
            body_snapshots: Vec::new(),
            steps: 0,
        };
        executor.record_body(body);
        executor.locals = executor.bind_direct_arguments(body, args)?;
        Ok(executor)
    }

    /// Build an isolated executor for a nested callable, default computation, or suspended generator frame.
    fn with_locals(module: &BodyIrModule, locals: BTreeMap<LocalId, ReplacementValue>, steps: usize) -> Self {
        Self {
            module: module.clone(),
            locals,
            ownership_reads: Vec::new(),
            runtime_requirements: Vec::new(),
            body_snapshots: Vec::new(),
            steps,
        }
    }

    /// Record a directly consumed declaration body as evidence and preserve its runtime requirements in first-seen
    /// order.
    fn record_body(&mut self, body: &Body) {
        self.body_snapshots.push(body.render_snapshot());
        for requirement in &body.runtime_requirements {
            if !self.runtime_requirements.contains(requirement) {
                self.runtime_requirements.push(requirement.clone());
            }
        }
    }

    /// Record a stable marker for a non-declaration frame whose precise Body IR is nested in an already-recorded
    /// declaration snapshot.
    fn record_frame_evidence(&mut self, evidence: String) {
        self.body_snapshots.push(evidence);
    }

    /// Render every directly consumed Body-IR declaration and nested execution frame for the receipt-bound identity.
    fn body_snapshot(&self) -> String {
        self.body_snapshots.join("\n-- direct execution frame --\n")
    }

    /// Merge an isolated nested frame's runtime evidence into its caller after that frame actually executed.
    fn merge_child(&mut self, child: Self) {
        self.ownership_reads.extend(child.ownership_reads);
        for requirement in child.runtime_requirements {
            if !self.runtime_requirements.contains(&requirement) {
                self.runtime_requirements.push(requirement);
            }
        }
        self.body_snapshots.extend(child.body_snapshots);
        self.steps = child.steps;
    }

    /// Bind direct API arguments in declaration order, applying stored defaults only to omitted trailing slots.
    fn bind_direct_arguments(
        &mut self,
        body: &Body,
        args: &[ReplacementValue],
    ) -> Result<BTreeMap<LocalId, ReplacementValue>, ReplacementExecutionError> {
        let mut supplied = args.iter().cloned().map(Some).collect::<Vec<_>>();
        supplied.resize_with(body.params.len(), || None);
        self.bind_parameter_values(&body.params, supplied, &BTreeMap::new(), body.span)
    }

    /// Evaluate a resolved call site's operands in written source order and bind them to declared parameter slots.
    fn bind_call_arguments(
        &mut self,
        params: &[CallableParam],
        args: &[Operand],
        binding: &ArgumentBinding,
        captures: &BTreeMap<LocalId, ReplacementValue>,
        span: HirSourceSpan,
    ) -> Result<BTreeMap<LocalId, ReplacementValue>, ReplacementExecutionError> {
        let ArgumentBinding::Resolved {
            arguments,
            defaulted_slots,
        } = binding
        else {
            return Err(unsupported(
                "call with unresolved parameter binding outside the callable replacement profile",
                span,
            ));
        };
        if arguments.len() != args.len() {
            return Err(unsupported("call argument-binding metadata mismatch", span));
        }
        let mut supplied = vec![None; params.len()];
        let mut argument_indices: Vec<usize> = (0..arguments.len()).collect();
        argument_indices.sort_by_key(|index| arguments[*index].written_position);
        for index in argument_indices {
            let argument = arguments[index];
            if argument.slot >= params.len()
                || supplied[argument.slot].is_some()
                || arguments
                    .iter()
                    .filter(|other| other.written_position == argument.written_position)
                    .count()
                    != 1
            {
                return Err(unsupported("invalid resolved callable argument binding", span));
            }
            supplied[argument.slot] = Some(self.evaluate_operand(&args[index], span)?);
        }
        let defaulted = defaulted_slots.iter().copied().collect::<BTreeSet<_>>();
        for (slot, value) in supplied.iter().enumerate() {
            if value.is_none() && !defaulted.contains(&slot) {
                return Err(unsupported(
                    format!(
                        "call omitted parameter `{}` without a default-binding fact",
                        params[slot].name
                    ),
                    span,
                ));
            }
        }
        if defaulted
            .iter()
            .any(|slot| *slot >= params.len() || supplied[*slot].is_some())
        {
            return Err(unsupported("invalid defaulted callable parameter binding", span));
        }
        self.bind_parameter_values(params, supplied, captures, span)
    }

    /// Materialize supplied values, source defaults, and construction-time partial presets into one isolated frame.
    fn bind_parameter_values(
        &mut self,
        params: &[CallableParam],
        supplied: Vec<Option<ReplacementValue>>,
        captures: &BTreeMap<LocalId, ReplacementValue>,
        call_span: HirSourceSpan,
    ) -> Result<BTreeMap<LocalId, ReplacementValue>, ReplacementExecutionError> {
        if supplied.len() != params.len() {
            return Err(unsupported("callable parameter binding arity mismatch", call_span));
        }
        let mut locals = captures.clone();
        for (parameter, supplied) in params.iter().zip(supplied) {
            let value = match supplied {
                Some(value) => value,
                None => match &parameter.default {
                    CallableParamDefault::Required => {
                        return Err(unsupported(
                            format!("missing required callable parameter `{}`", parameter.name),
                            call_span,
                        ));
                    }
                    CallableParamDefault::Source(computation) => self.evaluate_default(computation)?,
                    CallableParamDefault::PartialPreset { capture } => {
                        captures.get(capture).cloned().ok_or_else(|| {
                            unsupported(
                                format!("missing construction-time preset for parameter `{}`", parameter.name),
                                parameter.span,
                            )
                        })?
                    }
                    CallableParamDefault::Unsupported { span, description } => {
                        return Err(unsupported(
                            format!("unsupported default for parameter `{}`: {description}", parameter.name),
                            *span,
                        ));
                    }
                },
            };
            if locals.contains_key(&parameter.local) {
                return Err(unsupported(
                    "callable parameter aliases a captured local",
                    parameter.span,
                ));
            }
            locals.insert(parameter.local, value);
        }
        Ok(locals)
    }

    /// Run a declaration-owned source-default computation before its callable frame receives that parameter.
    fn evaluate_default(
        &mut self,
        computation: &DefaultComputation,
    ) -> Result<ReplacementValue, ReplacementExecutionError> {
        let mut default_executor = Self::with_locals(&self.module, BTreeMap::new(), self.steps);
        for statement in &computation.stmts {
            match default_executor.execute_statement(statement)? {
                Flow::Next => {}
                Flow::Return(_) | Flow::Break | Flow::Continue => {
                    return Err(unsupported(
                        "control flow in a callable default computation",
                        statement.span,
                    ));
                }
            }
        }
        let result = default_executor.evaluate_operand(&computation.result, computation.span)?;
        self.merge_child(default_executor);
        self.record_frame_evidence(format!(
            "executed source default frame span={}..{} statements={}",
            computation.span.start,
            computation.span.end,
            computation.stmts.len()
        ));
        Ok(result)
    }

    /// Execute one normalized Body-IR block until it falls through or produces control flow.
    fn execute_block(
        &mut self,
        block: &incan_semantics_core::body_ir::Block,
    ) -> Result<Flow, ReplacementExecutionError> {
        for statement in &block.stmts {
            self.record_step(statement.span)?;
            match self.execute_statement(statement)? {
                Flow::Next => {}
                flow => return Ok(flow),
            }
        }
        Ok(Flow::Next)
    }

    /// Execute one Body-IR statement without consulting generated Rust or the legacy backend.
    fn execute_statement(&mut self, statement: &Statement) -> Result<Flow, ReplacementExecutionError> {
        match &statement.kind {
            StatementKind::Assign { place, rvalue } => {
                let value = self.evaluate_rvalue(rvalue, statement.span)?;
                self.assign_place(place, value, statement.span)?;
                Ok(Flow::Next)
            }
            StatementKind::Call {
                destination,
                callee,
                args,
                may_panic: _,
            } => self.execute_call(destination.as_ref(), callee, args, statement.span),
            StatementKind::Drop { local } => {
                let _ = self.locals.remove(local);
                Ok(Flow::Next)
            }
            StatementKind::If {
                cond,
                then_block,
                else_block,
            } => {
                if self.evaluate_operand(cond, statement.span)?.into_bool(statement.span)? {
                    self.execute_block(then_block)
                } else if let Some(else_block) = else_block {
                    self.execute_block(else_block)
                } else {
                    Ok(Flow::Next)
                }
            }
            StatementKind::Loop { body } => self.execute_loop(body, statement.span),
            StatementKind::Break { value: Some(_) } => Err(unsupported("value-carrying loop break", statement.span)),
            StatementKind::Break { value: None } => Ok(Flow::Break),
            StatementKind::Continue => Ok(Flow::Continue),
            StatementKind::Return { value } => Ok(Flow::Return(
                value
                    .as_ref()
                    .map(|value| self.evaluate_operand(value, statement.span))
                    .transpose()?,
            )),
            StatementKind::Assert {
                cond,
                message,
                may_panic: _,
            } => {
                if self.evaluate_operand(cond, statement.span)?.into_bool(statement.span)? {
                    Ok(Flow::Next)
                } else {
                    let detail = match message {
                        Some(message) => format!(
                            "assertion failed: {}",
                            self.evaluate_operand(message, statement.span)?.observable_text()
                        ),
                        None => "assertion failed".to_string(),
                    };
                    Err(runtime_failure(detail, statement.span))
                }
            }
            StatementKind::Expr { value } => {
                let _ = self.evaluate_operand(value, statement.span)?;
                Ok(Flow::Next)
            }
            StatementKind::IterNext {
                destination,
                iterator,
                protocol: IterProtocol::Builtin,
            } => self.execute_builtin_next(destination, iterator, statement.span),
            StatementKind::Yield { .. } => Err(unsupported(
                "generator yield outside a generator expression",
                statement.span,
            )),
            StatementKind::TryPropagate { .. } => Err(unsupported("try propagation", statement.span)),
            // See the matching arms in `validate_statement_profile`: representation is #1164's, execution is #1155's.
            StatementKind::Await { .. } => Err(unsupported("async await suspension", statement.span)),
            StatementKind::Race { .. } => Err(unsupported("async race selection", statement.span)),
            StatementKind::IterNext { .. } => Err(unsupported("non-range iteration", statement.span)),
            StatementKind::Unsupported { description } => Err(unsupported(description, statement.span)),
        }
    }

    /// Evaluate a direct Body-IR call without invoking generated Rust or a legacy backend.
    ///
    /// Local callable values own their capture environment; named bodies are looked up only in this typed module;
    /// and generator adapters retain an unpolled source value until a consumer asks for the next element.
    fn execute_call(
        &mut self,
        destination: Option<&Place>,
        callee: &Callee,
        args: &[Operand],
        span: HirSourceSpan,
    ) -> Result<Flow, ReplacementExecutionError> {
        let destination = destination.ok_or_else(|| unsupported("discarded string-concatenation result", span))?;
        let local = bare_local(destination, span)?;
        let value = match callee {
            Callee::Helper(HelperOp::StrConcat) => {
                let [left, right] = args else {
                    return Err(unsupported("string-concatenation call arity", span));
                };
                let left = self.evaluate_operand(left, span)?.into_string(span)?;
                let right = self.evaluate_operand(right, span)?.into_string(span)?;
                ReplacementValue::Str(format!("{left}{right}"))
            }
            Callee::Function(CallableTarget::Named(target)) if is_explicit_range_builtin(target) => {
                self.evaluate_range(args, span)?
            }
            Callee::Function(CallableTarget::Named(target)) => self.execute_named_callable(target, args, span)?,
            Callee::Function(CallableTarget::Local(target)) => self.execute_local_callable(target, args, span)?,
            Callee::Method(target) if target.name == "collect" => {
                let [receiver] = args else {
                    return Err(unsupported("generator collect call arity", span));
                };
                let iterator = self.take_generator_receiver(receiver, span)?;
                self.collect_generator(iterator, span)?
            }
            Callee::Method(target) if matches!(target.name.as_str(), "map" | "filter") => {
                self.construct_generator_adapter(target.name.as_str(), args, span)?
            }
            _ => return Err(unsupported(format!("call to {}", callee_label(callee)), span)),
        };
        self.assign_local(local, value);
        Ok(Flow::Next)
    }

    /// Invoke a stored closure or partial through its resolved call-site binding in a fresh frame.
    fn execute_local_callable(
        &mut self,
        target: &LocalCallableTarget,
        args: &[Operand],
        span: HirSourceSpan,
    ) -> Result<ReplacementValue, ReplacementExecutionError> {
        let callable = self.take_callable_receiver(&target.operand, span)?;
        let captures = callable.captures.iter().cloned().collect::<BTreeMap<_, _>>();
        if captures.len() != callable.captures.len() {
            return Err(unsupported("duplicate callable capture local", span));
        }
        let locals = self.bind_call_arguments(&callable.params, args, &target.binding, &captures, span)?;
        self.execute_callable_frame(&callable, locals, span, "stored callable")
    }

    /// Execute one callable expression body in a fresh local frame and retain evidence only after it completed.
    fn execute_callable_frame(
        &mut self,
        callable: &ReplacementCallable,
        locals: BTreeMap<LocalId, ReplacementValue>,
        span: HirSourceSpan,
        frame_kind: &str,
    ) -> Result<ReplacementValue, ReplacementExecutionError> {
        let mut child = Self::with_locals(&self.module, locals, self.steps);
        for statement in &callable.body.stmts {
            match child.execute_statement(statement)? {
                Flow::Next => {}
                Flow::Return(_) | Flow::Break | Flow::Continue => {
                    return Err(unsupported(
                        "control flow in a callable expression body",
                        statement.span,
                    ));
                }
            }
        }
        let result = child.evaluate_operand(&callable.body.result, span)?;
        self.merge_child(child);
        self.record_frame_evidence(format!(
            "executed {frame_kind} frame call_span={}..{} params={} captures={} statements={}",
            span.start,
            span.end,
            callable.params.len(),
            callable.captures.len(),
            callable.body.stmts.len()
        ));
        Ok(result)
    }

    /// Invoke one identity-selected in-module named function, creating a lazy frame when it yields.
    fn execute_named_callable(
        &mut self,
        target: &NamedCallableTarget,
        args: &[Operand],
        span: HirSourceSpan,
    ) -> Result<ReplacementValue, ReplacementExecutionError> {
        let direct_call_id = target.direct_call_id.as_ref().ok_or_else(|| {
            unsupported(
                format!(
                    "named callable `{}` without a same-module declaration identity",
                    target.name
                ),
                span,
            )
        })?;
        let body = self
            .module
            .bodies
            .iter()
            .find(|body| body.direct_call_id == *direct_call_id)
            .cloned()
            .ok_or_else(|| {
                unsupported(
                    format!(
                        "named callable `{}` targets a declaration outside this Body-IR module",
                        target.name
                    ),
                    span,
                )
            })?;
        if body.name != target.name {
            return Err(unsupported(
                format!(
                    "named callable `{}` disagrees with its same-module declaration identity",
                    target.name
                ),
                span,
            ));
        }
        validate_direct_body_profile(&body)?;
        let locals = self.bind_call_arguments(&body.params, args, &target.binding, &BTreeMap::new(), span)?;
        if body.is_generator() {
            let named_body = body.clone();
            let name = named_body.name.clone();
            let statement_count = named_body.block.stmts.len();
            return Ok(ReplacementValue::Generator(Box::new(ReplacementGenerator {
                frame: GeneratorFrame::new(locals, body.block.stmts),
                named_body: Some(named_body),
                frame_evidence: Some(format!(
                    "executed generator-function frame name={} call_span={}..{} statements={}",
                    name, span.start, span.end, statement_count
                )),
            })));
        }
        let mut child = Self::with_locals(&self.module, locals, self.steps);
        child.record_body(&body);
        let flow = child.execute_block(&body.block)?;
        let value = match flow {
            Flow::Return(Some(value)) => value,
            Flow::Return(None) | Flow::Next => ReplacementValue::Unit,
            Flow::Break | Flow::Continue => {
                return Err(unsupported("loop control outside a nested callable loop", body.span));
            }
        };
        self.merge_child(child);
        Ok(value)
    }

    /// Capture one admitted map or filter adapter without polling its source or callback.
    fn construct_generator_adapter(
        &mut self,
        name: &str,
        args: &[Operand],
        span: HirSourceSpan,
    ) -> Result<ReplacementValue, ReplacementExecutionError> {
        let [receiver, callback] = args else {
            return Err(unsupported(format!("generator {name} adapter arity"), span));
        };
        let source = self.take_iterable_receiver(receiver, span)?;
        let callback = self.evaluate_operand(callback, span)?;
        let ReplacementValue::Callable(callback) = callback else {
            return Err(unsupported(
                format!("generator {name} adapter callback is not a stored callable"),
                span,
            ));
        };
        let kind = match name {
            "map" => ReplacementAdapterKind::Map,
            "filter" => ReplacementAdapterKind::Filter,
            _ => return Err(unsupported(format!("generator {name} adapter"), span)),
        };
        Ok(ReplacementValue::Adapter(Box::new(ReplacementAdapter {
            source,
            callback: *callback,
            kind,
        })))
    }

    /// Materialize the admitted `range` source-spelling call before its normalized loop.
    fn evaluate_range(
        &mut self,
        args: &[Operand],
        span: HirSourceSpan,
    ) -> Result<ReplacementValue, ReplacementExecutionError> {
        let values = args
            .iter()
            .map(|argument| self.evaluate_operand(argument, span))
            .collect::<Result<Vec<_>, _>>()?;
        let ints = values
            .iter()
            .map(|value| match value {
                ReplacementValue::Int(value) => Ok(*value),
                value => Err(unsupported(format!("range argument using {}", value_kind(value)), span)),
            })
            .collect::<Result<Vec<_>, _>>()?;
        let (next, end, step) = match ints.as_slice() {
            [end] => (0, *end, 1),
            [start, end] => (*start, *end, 1),
            [start, end, step] if *step != 0 => (*start, *end, *step),
            [_, _, 0] => return Err(runtime_failure("range step cannot be zero".to_string(), span)),
            _ => return Err(unsupported("range call arity", span)),
        };
        Ok(ReplacementValue::Range { next, end, step })
    }

    /// Poll one admitted range or scalar-tuple-list iterator and express exhaustion as the Body-IR loop break it
    /// represents.
    fn execute_builtin_next(
        &mut self,
        destination: &Place,
        iterator: &Operand,
        span: HirSourceSpan,
    ) -> Result<Flow, ReplacementExecutionError> {
        let Operand::Place(iterator) = iterator else {
            return Err(unsupported("non-place builtin iterator", span));
        };
        let iterator_local = bare_local(&iterator.place, span)?;
        self.ownership_reads.push(OwnershipRead {
            span,
            fact: iterator.fact,
            last_use: iterator.last_use,
        });
        let mut iterator_value = self
            .locals
            .remove(&iterator_local)
            .ok_or_else(|| runtime_failure("read of an unavailable builtin iterator".to_string(), span))?;
        let next = self.poll_iterator(&mut iterator_value, span)?;
        self.locals.insert(iterator_local, iterator_value);
        let Some(value) = next else {
            return Ok(Flow::Break);
        };
        self.assign_local(bare_local(destination, span)?, value);
        Ok(Flow::Next)
    }

    /// Assign exactly the authoritative Body-IR `LocalId` selected by lowering.
    ///
    /// The profile preflight rejects repeated user-binding names rather than reconstructing scope equivalence from
    /// spelling. Reassignment is therefore refused until Body IR carries binding-equivalence facts; no name-based
    /// aliasing is permitted here.
    fn assign_local(&mut self, local: LocalId, value: ReplacementValue) {
        self.locals.insert(local, value);
    }

    /// Execute one normalized Body-IR loop and propagate only non-local control flow outward.
    fn execute_loop(
        &mut self,
        body: &incan_semantics_core::body_ir::Block,
        span: HirSourceSpan,
    ) -> Result<Flow, ReplacementExecutionError> {
        loop {
            match self.execute_block(body)? {
                Flow::Next | Flow::Continue => {}
                Flow::Break => return Ok(Flow::Next),
                Flow::Return(value) => return Ok(Flow::Return(value)),
            }
            if self.steps >= MAX_EXECUTION_STEPS {
                return Err(runtime_failure(
                    format!("normalized loop exceeded the {MAX_EXECUTION_STEPS}-step replacement profile limit"),
                    span,
                ));
            }
        }
    }

    /// Evaluate an assignment rvalue supported by the initial profile.
    fn evaluate_rvalue(
        &mut self,
        rvalue: &Rvalue,
        span: HirSourceSpan,
    ) -> Result<ReplacementValue, ReplacementExecutionError> {
        match rvalue {
            Rvalue::Use(operand) => self.evaluate_operand(operand, span),
            Rvalue::UnaryOp(operator, operand) => self.evaluate_unary(*operator, operand, span),
            Rvalue::BinaryOp(operator, left, right) => self.evaluate_binary(*operator, left, right, span),
            Rvalue::Aggregate(kind, operands) => self.evaluate_aggregate(kind, operands, span),
            Rvalue::Format(_) => Err(unsupported("f-string", span)),
            Rvalue::Closure {
                params,
                captured_operands,
                body,
            } => self.construct_callable(params, captured_operands, body, span),
            Rvalue::Generator {
                source,
                captured_operands,
                body,
            } => self.construct_generator(source, captured_operands, body, span),
            Rvalue::Match { .. } => Err(unsupported("match expression", span)),
        }
    }

    /// Capture a closure or partial environment exactly once at its construction point.
    fn construct_callable(
        &mut self,
        params: &[CallableParam],
        captured_operands: &[Operand],
        body: &ClosureBody,
        span: HirSourceSpan,
    ) -> Result<ReplacementValue, ReplacementExecutionError> {
        if captured_operands.len() != body.capture_locals.len() {
            return Err(unsupported("callable capture metadata mismatch", span));
        }
        let captures = captured_operands
            .iter()
            .zip(&body.capture_locals)
            .map(|(operand, local)| self.evaluate_operand(operand, span).map(|value| (*local, value)))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ReplacementValue::Callable(Box::new(ReplacementCallable {
            params: params.to_vec(),
            captures,
            body: body.clone(),
        })))
    }

    /// Capture a generator expression's construction-time source and free values without executing its body.
    fn construct_generator(
        &mut self,
        source: &Operand,
        captured_operands: &[Operand],
        body: &GeneratorBody,
        span: HirSourceSpan,
    ) -> Result<ReplacementValue, ReplacementExecutionError> {
        if captured_operands.len() != body.capture_locals.len() {
            return Err(unsupported("generator capture metadata mismatch", span));
        }
        let source = self.evaluate_operand(source, span)?;
        let captures = captured_operands
            .iter()
            .zip(&body.capture_locals)
            .map(|(operand, local)| self.evaluate_operand(operand, span).map(|value| (*local, value)))
            .collect::<Result<Vec<_>, _>>()?;
        let mut locals = BTreeMap::new();
        locals.insert(body.source_local, source);
        for (local, value) in captures {
            if locals.insert(local, value).is_some() {
                return Err(unsupported("generator capture aliases its source local", span));
            }
        }
        Ok(ReplacementValue::Generator(Box::new(ReplacementGenerator {
            frame: GeneratorFrame::new(locals, body.stmts.clone()),
            named_body: None,
            frame_evidence: Some(format!(
                "executed generator-expression frame span={}..{} source_local=_{} captures={} statements={}",
                span.start,
                span.end,
                body.source_local.0,
                body.capture_locals.len(),
                body.stmts.len()
            )),
        })))
    }

    /// Consume an admitted generator by resuming its retained frame until exhaustion.
    fn collect_generator(
        &mut self,
        mut iterator: ReplacementValue,
        span: HirSourceSpan,
    ) -> Result<ReplacementValue, ReplacementExecutionError> {
        let mut elements = Vec::new();
        while let Some(value) = self.poll_iterator(&mut iterator, span)? {
            elements.push(value);
        }
        Ok(ReplacementValue::CollectedGenerator { elements, next: 0 })
    }

    /// Take the generator receiver consumed by `.collect()` while retaining Body IR's recorded receiver read.
    ///
    /// Body-IR's generic method-lowering convention records receivers as borrows, but `Generator.collect` consumes
    /// its iterator in the runtime contract. The bounded executor therefore removes precisely this bare local after
    /// recording that compiler-owned read, so a second collection cannot manufacture a fresh deferred iterator.
    fn take_generator_receiver(
        &mut self,
        receiver: &Operand,
        span: HirSourceSpan,
    ) -> Result<ReplacementValue, ReplacementExecutionError> {
        let Operand::Place(place_operand) = receiver else {
            return Err(unsupported("non-place generator collect receiver", span));
        };
        let local = bare_local(&place_operand.place, span)?;
        self.ownership_reads.push(OwnershipRead {
            span,
            fact: place_operand.fact,
            last_use: place_operand.last_use,
        });
        let value = self
            .locals
            .remove(&local)
            .ok_or_else(|| runtime_failure("read of an unavailable generator receiver".to_string(), span))?;
        if !matches!(value, ReplacementValue::Generator(_) | ReplacementValue::Adapter(_)) {
            return Err(unsupported(
                format!("collecting {} outside the generator profile", value_kind(&value)),
                span,
            ));
        }
        Ok(value)
    }

    /// Consume a stored callable target while honoring the compiler-recorded ownership read on the target itself.
    fn take_callable_receiver(
        &mut self,
        operand: &incan_semantics_core::body_ir::PlaceOperand,
        span: HirSourceSpan,
    ) -> Result<ReplacementCallable, ReplacementExecutionError> {
        let local = bare_local(&operand.place, span)?;
        self.ownership_reads.push(OwnershipRead {
            span,
            fact: operand.fact,
            last_use: operand.last_use,
        });
        let value = match operand.fact {
            OwnershipFact::Move => self.locals.remove(&local),
            OwnershipFact::Clone | OwnershipFact::Copy | OwnershipFact::Borrow | OwnershipFact::MutBorrow => {
                self.locals.get(&local).cloned()
            }
            OwnershipFact::Unknown => None,
        }
        .ok_or_else(|| runtime_failure("read of an unavailable callable receiver".to_string(), span))?;
        let ReplacementValue::Callable(callable) = value else {
            return Err(unsupported(
                format!("calling {} outside the stored-callable profile", value_kind(&value)),
                span,
            ));
        };
        Ok(*callable)
    }

    /// Consume an iterator receiver for a lazy adapter, preserving its source-owned read fact.
    fn take_iterable_receiver(
        &mut self,
        receiver: &Operand,
        span: HirSourceSpan,
    ) -> Result<ReplacementValue, ReplacementExecutionError> {
        let Operand::Place(place_operand) = receiver else {
            return Err(unsupported("non-place generator adapter receiver", span));
        };
        let local = bare_local(&place_operand.place, span)?;
        self.ownership_reads.push(OwnershipRead {
            span,
            fact: place_operand.fact,
            last_use: place_operand.last_use,
        });
        let value = self
            .locals
            .remove(&local)
            .ok_or_else(|| runtime_failure("read of an unavailable generator adapter receiver".to_string(), span))?;
        if matches!(value, ReplacementValue::Generator(_) | ReplacementValue::Adapter(_)) {
            Ok(value)
        } else {
            Err(unsupported(
                format!("{} adapter outside the generator profile", value_kind(&value)),
                span,
            ))
        }
    }

    /// Resume a generator frame until one yield or exhaustion, merging its actual execution evidence into the caller.
    fn resume_generator(
        &mut self,
        generator: &mut ReplacementGenerator,
        span: HirSourceSpan,
    ) -> Result<Option<ReplacementValue>, ReplacementExecutionError> {
        let named_body = generator.named_body.take();
        let frame_evidence = generator.frame_evidence.take();
        let locals = std::mem::take(&mut generator.frame.locals);
        let resume_steps = generator.frame.resume_step_budget(self.steps);
        let mut deferred = Self::with_locals(&self.module, locals, resume_steps);
        if let Some(body) = &named_body {
            deferred.record_body(body);
        }
        if let Some(evidence) = frame_evidence {
            deferred.record_frame_evidence(evidence);
        }
        let value = deferred.resume_generator_frame(&mut generator.frame, span)?;
        generator.frame.locals = std::mem::take(&mut deferred.locals);
        generator.frame.steps = deferred.steps;
        self.merge_child(deferred);
        Ok(value)
    }

    /// Poll an iterator value once. This single surface is shared by normalized `for` lowering and lazy adapters.
    fn poll_iterator(
        &mut self,
        value: &mut ReplacementValue,
        span: HirSourceSpan,
    ) -> Result<Option<ReplacementValue>, ReplacementExecutionError> {
        match value {
            ReplacementValue::Range { next, end, step }
                if (*step > 0 && *next < *end) || (*step < 0 && *next > *end) =>
            {
                let value = ReplacementValue::Int(*next);
                *next += *step;
                Ok(Some(value))
            }
            ReplacementValue::Range { .. } => Ok(None),
            ReplacementValue::List { elements, next } | ReplacementValue::CollectedGenerator { elements, next }
                if *next < elements.len() =>
            {
                let value = elements[*next].clone();
                *next += 1;
                Ok(Some(value))
            }
            ReplacementValue::List { .. } | ReplacementValue::CollectedGenerator { .. } => Ok(None),
            ReplacementValue::Generator(generator) => self.resume_generator(generator, span),
            ReplacementValue::Adapter(adapter) => self.poll_adapter(adapter, span),
            value => Err(unsupported(format!("iteration over {}", value_kind(value)), span)),
        }
    }

    /// Poll one map/filter adapter without materializing its upstream source.
    fn poll_adapter(
        &mut self,
        adapter: &mut ReplacementAdapter,
        span: HirSourceSpan,
    ) -> Result<Option<ReplacementValue>, ReplacementExecutionError> {
        loop {
            let Some(candidate) = self.poll_iterator(&mut adapter.source, span)? else {
                return Ok(None);
            };
            let callback_result = self.invoke_callable_value(&adapter.callback, vec![Some(candidate.clone())], span)?;
            match adapter.kind {
                ReplacementAdapterKind::Map => return Ok(Some(callback_result)),
                ReplacementAdapterKind::Filter => {
                    if callback_result.into_bool(span)? {
                        return Ok(Some(candidate));
                    }
                }
            }
        }
    }

    /// Invoke a captured callable with already-evaluated values, used by lazy adapters while polling.
    fn invoke_callable_value(
        &mut self,
        callable: &ReplacementCallable,
        supplied: Vec<Option<ReplacementValue>>,
        span: HirSourceSpan,
    ) -> Result<ReplacementValue, ReplacementExecutionError> {
        let captures = callable.captures.iter().cloned().collect::<BTreeMap<_, _>>();
        if captures.len() != callable.captures.len() {
            return Err(unsupported("duplicate callable capture local", span));
        }
        let locals = self.bind_parameter_values(&callable.params, supplied, &captures, span)?;
        self.execute_callable_frame(callable, locals, span, "generator-adapter callback")
    }

    /// Interpret a persisted nested-block cursor until the first yield or final exhaustion.
    fn resume_generator_frame(
        &mut self,
        frame: &mut GeneratorFrame,
        span: HirSourceSpan,
    ) -> Result<Option<ReplacementValue>, ReplacementExecutionError> {
        if frame.exhausted {
            return Ok(None);
        }
        loop {
            let Some(cursor) = frame.cursors.last_mut() else {
                frame.exhausted = true;
                return Ok(None);
            };
            if cursor.next == cursor.statements.len() {
                if cursor.is_loop {
                    cursor.next = 0;
                    continue;
                }
                frame.cursors.pop();
                continue;
            }
            let statement = cursor.statements[cursor.next].clone();
            cursor.next += 1;
            self.record_step(statement.span)?;
            match &statement.kind {
                StatementKind::Yield { value } => return self.evaluate_operand(value, statement.span).map(Some),
                StatementKind::If {
                    cond,
                    then_block,
                    else_block,
                } => {
                    let selected = if self.evaluate_operand(cond, statement.span)?.into_bool(statement.span)? {
                        Some(then_block)
                    } else {
                        else_block.as_ref()
                    };
                    if let Some(block) = selected {
                        frame.cursors.push(GeneratorCursor::block(block.stmts.clone()));
                    }
                }
                StatementKind::Loop { body } => frame.cursors.push(GeneratorCursor::loop_body(body.stmts.clone())),
                StatementKind::Break { value: Some(_) } => {
                    return Err(unsupported("value-carrying loop break in generator", statement.span));
                }
                StatementKind::Break { value: None } => self.break_generator_loop(frame, statement.span)?,
                StatementKind::Continue => self.continue_generator_loop(frame, statement.span)?,
                StatementKind::Return { value: Some(_) } => {
                    return Err(unsupported("value-carrying return from generator", statement.span));
                }
                StatementKind::Return { value: None } => {
                    frame.cursors.clear();
                    frame.exhausted = true;
                    return Ok(None);
                }
                _ => match self.execute_statement(&statement)? {
                    Flow::Next => {}
                    Flow::Break => self.break_generator_loop(frame, statement.span)?,
                    Flow::Continue => self.continue_generator_loop(frame, statement.span)?,
                    Flow::Return(_) => {
                        return Err(unsupported("unsupported generator return flow", statement.span));
                    }
                },
            }
            frame.steps = self.steps;
            if frame.steps >= MAX_EXECUTION_STEPS {
                return Err(runtime_failure(
                    format!("generator exceeded the {MAX_EXECUTION_STEPS}-step replacement profile limit"),
                    span,
                ));
            }
        }
    }

    /// Leave the innermost persisted loop after a generator `break` without replaying its parent cursor.
    fn break_generator_loop(
        &mut self,
        frame: &mut GeneratorFrame,
        span: HirSourceSpan,
    ) -> Result<(), ReplacementExecutionError> {
        let Some(index) = frame.cursors.iter().rposition(|cursor| cursor.is_loop) else {
            return Err(unsupported("generator break outside a loop", span));
        };
        frame.cursors.truncate(index);
        Ok(())
    }

    /// Restart the innermost persisted loop after a generator `continue`, retaining its locals and parent cursor.
    fn continue_generator_loop(
        &mut self,
        frame: &mut GeneratorFrame,
        span: HirSourceSpan,
    ) -> Result<(), ReplacementExecutionError> {
        let Some(index) = frame.cursors.iter().rposition(|cursor| cursor.is_loop) else {
            return Err(unsupported("generator continue outside a loop", span));
        };
        frame.cursors.truncate(index + 1);
        frame.cursors[index].next = 0;
        Ok(())
    }

    /// Materialize source-local recursive tuples and lists without inventing dict or set behavior.
    fn evaluate_aggregate(
        &mut self,
        kind: &AggregateKind,
        operands: &[Operand],
        span: HirSourceSpan,
    ) -> Result<ReplacementValue, ReplacementExecutionError> {
        let values = operands
            .iter()
            .map(|operand| self.evaluate_operand(operand, span))
            .collect::<Result<Vec<_>, _>>()?;
        match kind {
            AggregateKind::Tuple if values.iter().all(ReplacementValue::is_direct_structural) => {
                Ok(ReplacementValue::Tuple(values))
            }
            AggregateKind::Tuple => Err(unsupported("tuple aggregate with a non-structural element", span)),
            AggregateKind::List if values.iter().all(ReplacementValue::is_direct_structural) => {
                Ok(ReplacementValue::List {
                    elements: values,
                    next: 0,
                })
            }
            AggregateKind::List => Err(unsupported("list aggregate with a non-structural element", span)),
            _ => Err(unsupported(format!("{} aggregate", aggregate_label(kind)), span)),
        }
    }

    /// Evaluate a scalar unary operation.
    fn evaluate_unary(
        &mut self,
        operator: UnOp,
        operand: &Operand,
        span: HirSourceSpan,
    ) -> Result<ReplacementValue, ReplacementExecutionError> {
        let value = self.evaluate_operand(operand, span)?;
        match (operator, value) {
            (UnOp::Neg, ReplacementValue::Int(value)) => Ok(ReplacementValue::Int(-value)),
            (UnOp::Not, ReplacementValue::Bool(value)) => Ok(ReplacementValue::Bool(!value)),
            (UnOp::Invert, ReplacementValue::Int(value)) => Ok(ReplacementValue::Int(!value)),
            (operator, value) => Err(unsupported(
                format!("{} applied to {}", unary_label(operator), value_kind(&value)),
                span,
            )),
        }
    }

    /// Evaluate a scalar arithmetic, comparison, or boolean operation.
    fn evaluate_binary(
        &mut self,
        operator: BinOp,
        left: &Operand,
        right: &Operand,
        span: HirSourceSpan,
    ) -> Result<ReplacementValue, ReplacementExecutionError> {
        let left = self.evaluate_operand(left, span)?;
        let right = self.evaluate_operand(right, span)?;
        match (operator, left, right) {
            (BinOp::Add, ReplacementValue::Int(left), ReplacementValue::Int(right)) => {
                Ok(ReplacementValue::Int(left + right))
            }
            (BinOp::Sub, ReplacementValue::Int(left), ReplacementValue::Int(right)) => {
                Ok(ReplacementValue::Int(left - right))
            }
            (BinOp::Mul, ReplacementValue::Int(left), ReplacementValue::Int(right)) => {
                Ok(ReplacementValue::Int(left * right))
            }
            (BinOp::FloorDiv, ReplacementValue::Int(_), ReplacementValue::Int(0))
            | (BinOp::Mod, ReplacementValue::Int(_), ReplacementValue::Int(0))
            | (BinOp::Div, ReplacementValue::Int(_), ReplacementValue::Int(0)) => {
                Err(runtime_failure("division or modulo by zero".to_string(), span))
            }
            (BinOp::FloorDiv, ReplacementValue::Int(left), ReplacementValue::Int(right)) => {
                checked_python_floor_division(left, right, span)
            }
            (BinOp::Mod, ReplacementValue::Int(left), ReplacementValue::Int(right)) => {
                Ok(ReplacementValue::Int(python_mod_i64(left, right)))
            }
            (BinOp::Eq, left, right) if left.is_collection_scalar() && right.is_collection_scalar() => {
                Ok(ReplacementValue::Bool(left == right))
            }
            (BinOp::Ne, left, right) if left.is_collection_scalar() && right.is_collection_scalar() => {
                Ok(ReplacementValue::Bool(left != right))
            }
            (BinOp::Lt, ReplacementValue::Int(left), ReplacementValue::Int(right)) => {
                Ok(ReplacementValue::Bool(left < right))
            }
            (BinOp::Le, ReplacementValue::Int(left), ReplacementValue::Int(right)) => {
                Ok(ReplacementValue::Bool(left <= right))
            }
            (BinOp::Gt, ReplacementValue::Int(left), ReplacementValue::Int(right)) => {
                Ok(ReplacementValue::Bool(left > right))
            }
            (BinOp::Ge, ReplacementValue::Int(left), ReplacementValue::Int(right)) => {
                Ok(ReplacementValue::Bool(left >= right))
            }
            (BinOp::And, ReplacementValue::Bool(left), ReplacementValue::Bool(right)) => {
                Ok(ReplacementValue::Bool(left && right))
            }
            (BinOp::Or, ReplacementValue::Bool(left), ReplacementValue::Bool(right)) => {
                Ok(ReplacementValue::Bool(left || right))
            }
            (operator, left, right) => Err(unsupported(
                format!(
                    "{} between {} and {}",
                    binary_label(operator),
                    value_kind(&left),
                    value_kind(&right)
                ),
                span,
            )),
        }
    }

    /// Read one constant or local place while applying its recorded ownership decision.
    fn evaluate_operand(
        &mut self,
        operand: &Operand,
        span: HirSourceSpan,
    ) -> Result<ReplacementValue, ReplacementExecutionError> {
        match operand {
            Operand::Constant(constant) => Ok(constant_value(constant)),
            Operand::Place(place_operand) => {
                let local = place_operand.place.local;
                self.ownership_reads.push(OwnershipRead {
                    span,
                    fact: place_operand.fact,
                    last_use: place_operand.last_use,
                });
                let value = match place_operand.fact {
                    OwnershipFact::Copy => self
                        .locals
                        .get(&local)
                        .cloned()
                        .ok_or_else(|| runtime_failure("read of an unavailable local".to_string(), span))?,
                    OwnershipFact::Move => self.read_moved_place(&place_operand.place, span)?,
                    OwnershipFact::Clone | OwnershipFact::Borrow | OwnershipFact::MutBorrow => self
                        .locals
                        .get(&local)
                        .cloned()
                        .ok_or_else(|| runtime_failure("read of an unavailable local".to_string(), span))?,
                    OwnershipFact::Unknown => return Err(unsupported("unknown ownership fact", span)),
                };
                let value = self.project_place(value, &place_operand.place, span)?;
                if matches!(place_operand.fact, OwnershipFact::Copy) && !value.is_copy_shaped() {
                    return Err(unsupported("copy of a non-copy or unavailable local", span));
                }
                Ok(value)
            }
        }
    }

    /// Move a complete local while refusing the partial moves Body IR v0 does not model for collection tuples.
    fn read_moved_place(
        &mut self,
        place: &Place,
        span: HirSourceSpan,
    ) -> Result<ReplacementValue, ReplacementExecutionError> {
        if !place.projection.is_empty() {
            return Err(unsupported(
                "move through a tuple field projection outside the scalar tuple collection profile",
                span,
            ));
        }
        self.locals
            .remove(&place.local)
            .ok_or_else(|| runtime_failure("read of a moved or dropped local".to_string(), span))
    }

    /// Apply one source-local tuple field or list index projection while retaining the original source span.
    fn project_place(
        &mut self,
        value: ReplacementValue,
        place: &Place,
        span: HirSourceSpan,
    ) -> Result<ReplacementValue, ReplacementExecutionError> {
        match place.projection.as_slice() {
            [] | [PlaceElem::Field(_)] => project_tuple_field(value, place, span),
            [PlaceElem::Index(index)] => {
                let index = self.evaluate_operand(index, span)?;
                let ReplacementValue::Int(index) = index else {
                    return Err(unsupported("list index using a non-int value", span));
                };
                let index =
                    usize::try_from(index).map_err(|_| runtime_failure("list index is negative".to_string(), span))?;
                match value {
                    ReplacementValue::List { elements, .. } | ReplacementValue::CollectedGenerator { elements, .. } => {
                        elements
                            .get(index)
                            .cloned()
                            .ok_or_else(|| runtime_failure("list index is out of range".to_string(), span))
                    }
                    value => Err(unsupported(
                        format!("indexing {} outside the source-local list profile", value_kind(&value)),
                        span,
                    )),
                }
            }
            _ => Err(unsupported(
                "place projection outside the source-local structural profile",
                span,
            )),
        }
    }

    /// Assign a complete local or one source-local list element without permitting nested writes.
    fn assign_place(
        &mut self,
        place: &Place,
        value: ReplacementValue,
        span: HirSourceSpan,
    ) -> Result<(), ReplacementExecutionError> {
        match place.projection.as_slice() {
            [] => {
                self.assign_local(place.local, value);
                Ok(())
            }
            [PlaceElem::Index(index)] => {
                if !value.is_direct_structural() {
                    return Err(unsupported("list assignment with a non-structural value", span));
                }
                let index = self.evaluate_operand(index, span)?;
                let ReplacementValue::Int(index) = index else {
                    return Err(unsupported("list assignment using a non-int index", span));
                };
                let index = usize::try_from(index)
                    .map_err(|_| runtime_failure("list assignment index is negative".to_string(), span))?;
                let target = self
                    .locals
                    .get_mut(&place.local)
                    .ok_or_else(|| runtime_failure("assignment to an unavailable local".to_string(), span))?;
                let ReplacementValue::List { elements, .. } = target else {
                    return Err(unsupported(
                        "index assignment outside the source-local list profile",
                        span,
                    ));
                };
                let Some(element) = elements.get_mut(index) else {
                    return Err(runtime_failure(
                        "list assignment index is out of range".to_string(),
                        span,
                    ));
                };
                *element = value;
                Ok(())
            }
            _ => Err(unsupported(
                "place assignment outside the source-local list profile",
                span,
            )),
        }
    }

    /// Record one executed statement and enforce the bounded-profile step limit.
    fn record_step(&mut self, span: HirSourceSpan) -> Result<(), ReplacementExecutionError> {
        self.steps = self.steps.saturating_add(1);
        if self.steps > MAX_EXECUTION_STEPS {
            return Err(runtime_failure(
                format!("replacement profile exceeded the {MAX_EXECUTION_STEPS}-step execution limit"),
                span,
            ));
        }
        Ok(())
    }
}

impl ReplacementValue {
    /// Return whether a value can honor a Body-IR `Copy` read without duplicating owned state.
    const fn is_copy_shaped(&self) -> bool {
        matches!(self, Self::Int(_) | Self::Bool(_) | Self::Unit)
    }

    /// Return whether this value is one scalar leaf of the source-local structural vocabulary.
    const fn is_collection_scalar(&self) -> bool {
        matches!(self, Self::Int(_) | Self::Bool(_) | Self::Str(_) | Self::Unit)
    }

    /// Return whether this value is recursively materializable by the tuple/list profile.
    fn is_direct_structural(&self) -> bool {
        self.is_collection_scalar()
            || matches!(self, Self::Tuple(elements) | Self::List { elements, .. }
                if elements.iter().all(Self::is_direct_structural))
    }

    /// Return this value as a boolean, refusing a type-shape mismatch at the original source location.
    fn into_bool(self, span: HirSourceSpan) -> Result<bool, ReplacementExecutionError> {
        match self {
            Self::Bool(value) => Ok(value),
            value => Err(unsupported(
                format!("boolean condition using {}", value_kind(&value)),
                span,
            )),
        }
    }

    /// Return this value as an owned string, refusing an incompatible helper call at the original source location.
    fn into_string(self, span: HirSourceSpan) -> Result<String, ReplacementExecutionError> {
        match self {
            Self::Str(value) => Ok(value),
            value => Err(unsupported(
                format!("string operation using {}", value_kind(&value)),
                span,
            )),
        }
    }
}

/// Control flow propagated between normalized blocks.
enum Flow {
    /// Ordinary fallthrough.
    Next,
    /// Break from the innermost normalized loop.
    Break,
    /// Continue the innermost normalized loop.
    Continue,
    /// Return from the selected free function.
    Return(Option<ReplacementValue>),
}

/// Return one bare local id, refusing fields, indexes, and slices in the first profile.
fn bare_local(place: &Place, span: HirSourceSpan) -> Result<LocalId, ReplacementExecutionError> {
    if place.projection.is_empty() {
        Ok(place.local)
    } else {
        Err(unsupported("place projection", span))
    }
}

/// Project one numeric source-local tuple field while retaining the statement's original source authority on refusal.
fn project_tuple_field(
    value: ReplacementValue,
    place: &Place,
    span: HirSourceSpan,
) -> Result<ReplacementValue, ReplacementExecutionError> {
    let [PlaceElem::Field(field)] = place.projection.as_slice() else {
        return if place.projection.is_empty() {
            Ok(value)
        } else {
            Err(unsupported(
                "place projection outside the source-local structural profile",
                span,
            ))
        };
    };
    let index = field
        .parse::<usize>()
        .map_err(|_| unsupported("non-numeric tuple field projection", span))?;
    match value {
        ReplacementValue::Tuple(elements) => elements
            .into_iter()
            .nth(index)
            .ok_or_else(|| runtime_failure("tuple field index is out of range".to_string(), span)),
        value => Err(unsupported(
            format!(
                "tuple field projection `.{}` using {} outside the source-local structural profile",
                field,
                value_kind(&value)
            ),
            span,
        )),
    }
}

/// Construct a source-span-preserving unsupported-profile error.
fn unsupported(description: impl Into<String>, span: HirSourceSpan) -> ReplacementExecutionError {
    ReplacementExecutionError::Unsupported {
        description: description.into(),
        span,
        span_start: span.start,
        span_end: span.end,
    }
}

/// Construct a source-span-preserving runtime-failure error.
fn runtime_failure(detail: String, span: HirSourceSpan) -> ReplacementExecutionError {
    ReplacementExecutionError::RuntimeFailure {
        detail,
        span,
        span_start: span.start,
        span_end: span.end,
    }
}

/// Apply Python integer floor division while keeping an unrepresentable direct-execution quotient visible.
fn checked_python_floor_division(
    left: i64,
    right: i64,
    span: HirSourceSpan,
) -> Result<ReplacementValue, ReplacementExecutionError> {
    if left == i64::MIN && right == -1 {
        return Err(runtime_failure("integer division overflow".to_string(), span));
    }
    Ok(ReplacementValue::Int(python_floor_div_i64(left, right)))
}

/// Reject a return value that would widen the first replacement profile beyond scalar source observables.
fn ensure_scalar_result(value: &ReplacementValue, span: HirSourceSpan) -> Result<(), ReplacementExecutionError> {
    match value {
        ReplacementValue::Int(_) | ReplacementValue::Bool(_) | ReplacementValue::Str(_) | ReplacementValue::Unit => {
            Ok(())
        }
        _ => Err(unsupported(
            format!("returning {} from the scalar replacement profile", value_kind(value)),
            span,
        )),
    }
}

/// Project ownership reads into the stable evidence shape shared by identities and CLI reports.
fn ownership_read_projection(reads: &[OwnershipRead]) -> Vec<OwnershipReadProjection> {
    reads
        .iter()
        .map(|read| OwnershipReadProjection {
            span_start: read.span.start,
            span_end: read.span.end,
            fact: ownership_label(read.fact),
            last_use: read.last_use,
        })
        .collect()
}

/// Project Body-IR runtime requirements into stable semantic labels shared by identities and CLI reports.
fn runtime_requirement_projection(requirements: &[AbiV0RuntimeRequirement]) -> Vec<RuntimeRequirementProjection> {
    requirements
        .iter()
        .map(|requirement| RuntimeRequirementProjection {
            requirement: runtime_requirement_label(requirement),
        })
        .collect()
}

/// Render ownership evidence as one deterministic digest component without relying on `Debug` formatting.
fn canonical_ownership_summary(reads: &[OwnershipRead]) -> String {
    ownership_read_projection(reads)
        .into_iter()
        .map(|read| {
            format!(
                "span={}..{};fact={};last_use={}",
                read.span_start, read.span_end, read.fact, read.last_use
            )
        })
        .collect::<Vec<_>>()
        .join("|")
}

/// Render runtime requirements as one deterministic digest component without relying on `Debug` formatting.
fn canonical_runtime_requirements_summary(requirements: &[AbiV0RuntimeRequirement]) -> String {
    runtime_requirement_projection(requirements)
        .into_iter()
        .map(|requirement| requirement.requirement)
        .collect::<Vec<_>>()
        .join("|")
}

/// Render one runtime requirement with the semantic labels used in replacement evidence.
fn runtime_requirement_label(requirement: &AbiV0RuntimeRequirement) -> String {
    match requirement {
        AbiV0RuntimeRequirement::RuntimeHelper(name) => format!("runtime_helper({name})"),
        AbiV0RuntimeRequirement::HostedStd => "hosted_std".to_string(),
        AbiV0RuntimeRequirement::Allocator => "allocator".to_string(),
        AbiV0RuntimeRequirement::PanicStrategy => "panic_strategy".to_string(),
        AbiV0RuntimeRequirement::AsyncRuntime => "async_runtime".to_string(),
    }
}

/// Render one ownership fact without relying on generated-Rust implementation details.
const fn ownership_label(fact: OwnershipFact) -> &'static str {
    match fact {
        OwnershipFact::Copy => "copy",
        OwnershipFact::Move => "move",
        OwnershipFact::Clone => "clone",
        OwnershipFact::Borrow => "borrow",
        OwnershipFact::MutBorrow => "mut_borrow",
        OwnershipFact::Unknown => "unknown",
    }
}

/// Render a narrow unsupported-call label for diagnostics.
fn callee_label(callee: &Callee) -> String {
    match callee {
        Callee::Function(CallableTarget::Named(target)) => format!("function `{}`", target.name),
        Callee::Function(CallableTarget::Local(_)) => "stored callable".to_string(),
        Callee::Method(target) => format!("method `{}`", target.name),
        Callee::Helper(helper) => format!("runtime helper `{}`", helper_label(*helper)),
    }
}

/// Render a compiler-owned helper name without depending on generated-Rust spellings.
const fn helper_label(helper: HelperOp) -> &'static str {
    match helper {
        HelperOp::StrConcat => "str_concat",
        HelperOp::StrEq => "str_eq",
        HelperOp::StrNe => "str_ne",
        HelperOp::StrLt => "str_lt",
        HelperOp::StrLe => "str_le",
        HelperOp::StrGt => "str_gt",
        HelperOp::StrGe => "str_ge",
        HelperOp::ListConcat => "list_concat",
    }
}

/// Render an aggregate kind as a compact source-level diagnostic label.
fn aggregate_label(kind: &incan_semantics_core::body_ir::AggregateKind) -> &'static str {
    match kind {
        incan_semantics_core::body_ir::AggregateKind::Tuple => "tuple",
        incan_semantics_core::body_ir::AggregateKind::List => "list",
        incan_semantics_core::body_ir::AggregateKind::Dict => "dict",
        incan_semantics_core::body_ir::AggregateKind::Set => "set",
        incan_semantics_core::body_ir::AggregateKind::Constructor(_) => "constructor",
    }
}

/// Register the bounded scalar/control profile beside the replacement executor that implements it.
///
/// The compatibility collector reports this contribution but does not own its feature definitions. In particular,
/// successful direct execution remains non-green until aggregate source contracts have their own paired comparison
/// evidence; the single `replacement-body-v0-001` match stays case-scoped.
pub(crate) fn replacement_compatibility_direct_execution_contribution()
-> crate::replacement_compatibility::ReplacementCompatibilityContribution {
    use crate::replacement_compatibility::{
        feature_requirement_link, implementation_requirement, local_implementation_contribution,
        preserved_feature_at_boundary,
    };

    local_implementation_contribution(
        "backend.replacement.bounded-scalar-control",
        "src/backend/replacement/mod.rs",
        "fn replacement_compatibility_direct_execution_contribution",
        vec![
            preserved_feature_at_boundary(
                "language.control-flow",
                "Bounded scalar conditionals, loops, returns, assertions, and range iteration execute directly with explicit receipts.",
                "src/frontend/typechecker/check_expr/control_flow.rs",
                "fn check_if_expr",
                "fn lower_if",
                "fn execute_loop",
            ),
            preserved_feature_at_boundary(
                "language.numeric-and-scalar",
                "Bounded scalar arithmetic, comparisons, boolean operators, and strings execute directly from Body IR.",
                "src/frontend/typechecker/check_expr/ops.rs",
                "fn check_binary",
                "fn lower_binary",
                "fn evaluate_binary",
            ),
        ],
        vec![
            implementation_requirement(
                "control.normalized-flow",
                "Branches, loops, returns, assertions, and breaks execute from normalized Body IR.",
                "Body IR lowering and replacement evaluator",
                "replacement-body-v0 corpus",
                "Normalized control nodes are implementation vocabulary.",
            ),
            implementation_requirement(
                "runtime.scalar-values",
                "Scalars, strings, operators, and conversions preserve checked type and failure behavior.",
                "Body IR operands/rvalues and replacement evaluator",
                "replacement-body-v0 scalar corpus",
                "Scalar representation is an internal evaluator mechanism.",
            ),
        ],
        Vec::new(),
        vec![
            feature_requirement_link("language.control-flow", "control.normalized-flow"),
            feature_requirement_link("language.numeric-and-scalar", "runtime.scalar-values"),
        ],
    )
}

/// Render a unary operator as a compact source-level diagnostic label.
const fn unary_label(operator: UnOp) -> &'static str {
    match operator {
        UnOp::Neg => "negation",
        UnOp::Not => "boolean negation",
        UnOp::Invert => "bitwise inversion",
    }
}

/// Render a binary operator as a compact source-level diagnostic label.
const fn binary_label(operator: BinOp) -> &'static str {
    match operator {
        BinOp::Add => "addition",
        BinOp::Sub => "subtraction",
        BinOp::Mul => "multiplication",
        BinOp::Div => "division",
        BinOp::FloorDiv => "floor division",
        BinOp::Mod => "modulo",
        BinOp::Eq => "equality comparison",
        BinOp::Ne => "inequality comparison",
        BinOp::Lt => "less-than comparison",
        BinOp::Le => "less-or-equal comparison",
        BinOp::Gt => "greater-than comparison",
        BinOp::Ge => "greater-or-equal comparison",
        BinOp::And => "boolean conjunction",
        BinOp::Or => "boolean disjunction",
    }
}

/// Render one replacement value's dynamic shape for an unsupported-operation diagnostic.
const fn value_kind(value: &ReplacementValue) -> &'static str {
    match value {
        ReplacementValue::Int(_) => "int",
        ReplacementValue::Bool(_) => "bool",
        ReplacementValue::Str(_) => "str",
        ReplacementValue::Float(_) => "float",
        ReplacementValue::Unit => "unit",
        ReplacementValue::Range { .. } => "range",
        ReplacementValue::List { .. } => "list",
        ReplacementValue::Tuple(_) => "tuple",
        ReplacementValue::Callable(_) => "callable",
        ReplacementValue::Generator(_) => "generator",
        ReplacementValue::Adapter(_) => "generator adapter",
        ReplacementValue::CollectedGenerator { .. } => "collected generator list",
    }
}

/// Convert one Body-IR literal into a first-profile replacement value.
fn constant_value(constant: &Constant) -> ReplacementValue {
    match constant {
        Constant::Int(value) => ReplacementValue::Int(*value),
        Constant::Bool(value) => ReplacementValue::Bool(*value),
        Constant::Str(value) => ReplacementValue::Str(value.clone()),
        Constant::Unit | Constant::None => ReplacementValue::Unit,
        Constant::Float(value) => ReplacementValue::Float(value.clone()),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use incan_semantics_core::CompilerNodeId;

    use super::{
        BodyExecutor, BodyIrModule, Constant, GeneratorFrame, HirSourceSpan, MAX_EXECUTION_STEPS, Operand,
        ReplacementExecutionError, ReplacementGenerator, Statement, StatementKind,
    };

    /// A resumed generator must retain the steps its parent spent before the first poll.
    #[test]
    fn generator_resume_counts_the_parent_budget_before_polling_its_frame() {
        let span = HirSourceSpan::new(0, 1);
        let module = BodyIrModule {
            module_id: CompilerNodeId::module("replacement.generator_budget_test"),
            bodies: Vec::new(),
        };
        let mut executor = BodyExecutor::with_locals(&module, BTreeMap::new(), MAX_EXECUTION_STEPS);
        let mut generator = ReplacementGenerator {
            frame: GeneratorFrame::new(
                BTreeMap::new(),
                vec![Statement {
                    kind: StatementKind::Yield {
                        value: Operand::Constant(Constant::Int(1)),
                    },
                    span,
                }],
            ),
            named_body: None,
            frame_evidence: None,
        };

        let result = executor.resume_generator(&mut generator, span);
        assert!(
            matches!(result, Err(ReplacementExecutionError::RuntimeFailure { .. })),
            "the first generator poll must consume the caller's already-exhausted execution budget"
        );
    }
}
