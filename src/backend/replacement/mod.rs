//! Direct execution of the deliberately narrow #988 Body-IR replacement profile.
//!
//! This module consumes [`BodyIrModule`] directly. It never reads generated Rust, never
//! delegates a requested replacement execution to [`crate::backend::ir`], and rejects every operation outside the
//! first free-function profile with the original Body-IR source span. The profile is intentionally limited to
//! scalar arithmetic, compiler-owned string concatenation, branches, normalized loops, assertions, source-local
//! recursive tuple/list values, fully supplied source-local plain-model values, and exact source-local RFC 032
//! value-enum members followed by their generated scalar `.value()` extraction. It admits one numeric tuple or
//! canonical plain-model field projection and one integer list projection or assignment;
//! builtin iteration admits structural lists, canonical global list enumeration, and list-pair Zip. The selected
//! entrypoint must produce a scalar observable, although an admitted sibling may return a structural intermediate to
//! its direct caller. The executor also consumes the retained callable vocabulary directly: captured local closures,
//! partial presets, source-evaluable defaults, identity-selected local or same-module named calls, generator
//! expressions and generator functions, and their bounded lazy `map`/`filter` adapters. Packages, Rust interop,
//! unsupported callable/default forms, general destructuring, and other projections remain visible refusals. Its
//! enclosing declaration snapshot retains a deferred generator's shape, but the frame executes and adds execution-frame
//! evidence only when collection polls it; no path falls back to generated Rust.
//!
//! One checked provider-service operation also executes directly, from the already-lowered
//! [`ProviderOperationPlan`] rather than from source or generated Rust (#1156). That vertical is owned by
//! [`provider`], which consumes the RFC 104 authority and operation-receipt contracts instead of restating them;
//! this module supplies it with evaluated operands and refuses an unresolved, inactive, or unauthorized operation
//! at the original source span.

mod execution_preflight;
pub mod hashed;
mod list_iteration;
pub mod program_io;
pub mod provider;
pub mod source_profile;

pub use program_io::{ProgramIo, ProgramIoError, ProgramOutput};

use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
};

use incan_core::lang::builtins::{self, BuiltinFnId};
use incan_core::{
    errors::IncanError,
    lang::surface::constructors::{ConstructorId, as_str as constructor_name},
    lang::surface::iterator_methods::{self, IteratorMethodId},
    lang::types::collections::{self, CollectionTypeId},
    lang::types::numerics::{self, NumericTypeId},
    numeric_strings::{parse_float_string, parse_int_string},
    numeric_values::{IntegerBounds, decimal_value_fits, format_decimal_value, integer_bounds},
    python_floor_div_i64, python_mod_i64,
};
use incan_semantics_core::body_ir::{
    AggregateKind, ArgumentBinding, ArgumentElement, AssertionKind, BinOp, Body, BodyIrModule, CallableParam,
    CallableParamDefault, CallableTarget, Callee, ClosureBody, Constant, ConstructorTarget, DefaultComputation,
    DictEntry, FieldlessEnumDeclaration, FieldlessEnumVariantDeclaration, FieldlessEnumVariantTarget, FormatPart,
    FormatStyle, GeneratorBody, HelperOp, IterProtocol, LocalCallableTarget, LocalId, MatchArm, NamedCallableTarget,
    NominalDeclaration, NominalPatternTarget, Operand, OwnershipFact, Pattern, PatternBinding, Place, PlaceElem,
    ProviderActivationState, ProviderOperationPlan, ResultVariant, ResultVariantKind, Rvalue, Statement, StatementKind,
    TryErrorRouting, TypedNumericConstant, UnOp, ValueEnumBacking, ValueEnumDeclaration, ValueEnumVariantDeclaration,
    ValueEnumVariantTarget,
};
use incan_semantics_core::{
    AbiV0RuntimeRequirement, CanonicalSymbolId, CompilerNodeId, CompilerNodeKind, HirSourceSpan, IncanPrimitiveType,
    IncanType, SemanticSourceTargetKind, SymbolNamespace, SymbolOrigin, module_identity_for_path,
};

use crate::backend::selection::digest_output;
use hashed::{ReplacementDict, ReplacementSet};
use provider::{ProviderExecutionRecord, ProviderInputValue, ProviderRuntime, canonical_provider_execution_summary};

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
    /// An ordinary Incan binary float, normalized to the `f64` carrier the Rust-emission path uses.
    Float(f64),
    /// A compiler-typed exact numeric or decimal value whose source type is part of its runtime identity.
    Numeric(ReplacementNumericValue),
    /// An Incan `None`/unit value.
    Unit,
    /// A normalized runtime range iterator for the selected `range` source-spelling control-flow case.
    Range { next: i64, end: i64, step: i64 },
    /// A source-local structural list value with the cursor owned by its materialized builtin iterator local.
    List {
        elements: Vec<ReplacementValue>,
        next: usize,
    },
    /// A canonical global Zip over two checked structural lists, with private left-to-right polling state.
    Zip(Box<ReplacementZip>),
    /// A source-local structural tuple whose elements remain direct replacement values.
    Tuple(Vec<ReplacementValue>),
    /// An immutable hashed set; operand reads share its table rather than copying it before every probe.
    Set(Rc<ReplacementSet>),
    /// An immutable hashed dict; values are retained, while membership consults only scalar keys.
    Dict(Rc<ReplacementDict>),
    /// A source-local plain-model instance whose declaration identity and canonical field layout were verified.
    ///
    /// This is neither a generic object nor a name-based map. Construction resolves `direct_declaration_id` against
    /// `BodyIrModule::nominal_declarations`, and field reads repeat that verification before returning a stored
    /// canonical field. Nested nominal values, methods, field writes, aliases, and nominal entrypoint results stay
    /// outside this phase's profile.
    Nominal {
        /// Exact declaration identity retained from the source-local Body-IR nominal registry.
        direct_declaration_id: incan_semantics_core::CompilerNodeId,
        /// Canonical declared field names and values in declaration order.
        fields: Vec<(String, ReplacementValue)>,
    },
    /// An exact source-local fieldless normal-enum member verified against the Body-IR declaration registry.
    ///
    /// This carrier has no payload and exposes no methods, matching, collection behavior, or selected-entrypoint
    /// output. The direct runtime permits only equality or inequality after both operands revalidate their exact
    /// enum/member identities against the same source-local registry.
    FieldlessEnum {
        /// Exact retained source-local owner enum identity.
        enum_declaration_id: CompilerNodeId,
        /// Exact retained source-local unit-member identity.
        variant_declaration_id: CompilerNodeId,
    },
    /// An exact source-local RFC 032 value-enum member verified against the Body-IR declaration registry.
    ///
    /// The carrier deliberately stores no raw scalar and supports no general enum operations. The scalar literal is
    /// resolved again only by the admitted compiler-provided `.value()` call, after rechecking both identities and
    /// their membership in the same module registry.
    ValueEnum {
        /// Exact retained source-local owner enum identity.
        enum_declaration_id: incan_semantics_core::CompilerNodeId,
        /// Exact retained source-local member identity.
        variant_declaration_id: incan_semantics_core::CompilerNodeId,
    },
    /// One intrinsic `Result` carrier constructed directly from its retained Body-IR variant and payload.
    ///
    /// The source checker has already selected `Ok` or `Err`; direct execution retains that selection rather than
    /// reconstructing constructors from spelling. The payload is boxed solely because a Result may carry an admitted
    /// nominal or structural direct value.
    Result {
        kind: ResultVariantKind,
        payload: Box<ReplacementValue>,
        /// Checked `Result` success type retained by the constructing Body-IR rvalue.
        ok_type: IncanType,
        /// Checked `Result` error type retained by the constructing Body-IR rvalue.
        error_type: IncanType,
    },
    /// A closure or partial application whose lexical captures were evaluated when the value was constructed.
    Callable(Box<ReplacementCallable>),
    /// A generator expression or generator function whose frame remains deferred until an admitted consumer polls
    /// it. The frame owns its locals and continuation, so resuming never replays preceding statements.
    Generator(Box<ReplacementGenerator>),
    /// A source-local async function invocation retained as one direct Body-IR task frame.
    ///
    /// The shared cell is intentional: the source typechecker currently exposes a direct async call at its output
    /// type, so the executor must preserve task identity beneath a copy-shaped Body-IR read without duplicating its
    /// frame. Only an admitted `await` or `race for` may consume this value.
    Task(Rc<RefCell<ReplacementTask>>),
    /// A lazy map or filter adapter around another admitted iterator value.
    Adapter(Box<ReplacementAdapter>),
    /// Values materialized by an admitted lazy generator consumer such as `.collect()`.
    ///
    /// This is deliberately distinct from [`Self::List`]: the latter carries checked structural elements, while
    /// this variant preserves the generator consumer's separate admission and indexing contract.
    CollectedGenerator {
        elements: Vec<ReplacementValue>,
        next: usize,
    },
}

/// Exact numeric payloads admitted by replacement execution.
///
/// The signed/unsigned forms carry a canonical registry id so source aliases normalize once in the compiler. Direct
/// API callers may construct this enum, so entrypoint validation rechecks the id family and range before execution.
#[derive(Debug, Clone, PartialEq)]
pub enum ReplacementNumericValue {
    /// A signed integer with its canonical signed registry identity and a value inside that identity's range.
    Signed {
        /// Canonical signed integer kind; unsigned, float, and bool ids are malformed here.
        kind: NumericTypeId,
        /// Exact signed payload, constrained to `kind`'s inclusive bounds.
        value: i128,
    },
    /// An unsigned integer with its canonical unsigned registry identity and a value inside that identity's range.
    Unsigned {
        /// Canonical unsigned integer kind; signed, float, and bool ids are malformed here.
        kind: NumericTypeId,
        /// Exact unsigned payload, constrained to zero through `kind`'s inclusive maximum.
        value: u128,
    },
    /// A finite IEEE-754 binary32 value; non-finite direct carriers are malformed.
    F32(f32),
    /// A finite IEEE-754 binary64 value; non-finite direct carriers are malformed.
    F64(f64),
    /// A checked fixed-scale decimal retaining both its declared type and written fractional width.
    Decimal {
        /// Declared decimal precision in the supported range `1..=38`.
        precision: u8,
        /// Declared maximum fractional scale, no greater than `precision`.
        scale: u8,
        /// Signed literal digits with the decimal point removed.
        coefficient: i128,
        /// Fractional digits written in the source value, no greater than `scale`.
        literal_scale: u8,
    },
}

impl ReplacementNumericValue {
    /// Canonical checked type spelling retained for diagnostics and typed result transport.
    #[must_use]
    pub fn type_name(&self) -> String {
        match self {
            Self::Signed { kind, .. } | Self::Unsigned { kind, .. } => numerics::as_str(*kind).to_string(),
            Self::F32(_) => "f32".to_string(),
            Self::F64(_) => "f64".to_string(),
            Self::Decimal { precision, scale, .. } => format!("decimal[{precision}, {scale}]"),
        }
    }

    /// Native Display spelling for source output and scalar `str` conversion.
    #[must_use]
    pub fn observable_text(&self) -> String {
        match self {
            Self::Signed { value, .. } => value.to_string(),
            Self::Unsigned { value, .. } => value.to_string(),
            Self::F32(value) => value.to_string(),
            Self::F64(value) => value.to_string(),
            Self::Decimal {
                coefficient,
                literal_scale,
                ..
            } => format_decimal_value(*coefficient, *literal_scale),
        }
    }

    /// Typed payload used in receipt identity independently from source-observable Display.
    pub(crate) fn receipt_identity_text(&self) -> String {
        match self {
            Self::Signed { kind, value } => format!("{}:signed:{value}", numerics::as_str(*kind)),
            Self::Unsigned { kind, value } => format!("{}:unsigned:{value}", numerics::as_str(*kind)),
            Self::F32(value) => format!("f32:bits:{:08x}", value.to_bits()),
            Self::F64(value) => format!("f64:bits:{:016x}", value.to_bits()),
            Self::Decimal {
                precision,
                scale,
                coefficient,
                literal_scale,
            } => format!(
                "decimal[{precision},{scale}]:coefficient={}:literal_scale={}",
                coefficient, literal_scale
            ),
        }
    }

    /// Recheck the public carrier against its compiler-owned family and declared range.
    pub(crate) fn is_valid(&self) -> bool {
        match self {
            Self::Signed { kind, value } => {
                matches!(
                    integer_bounds(*kind),
                    Some(IntegerBounds::Signed { minimum, maximum }) if *value >= minimum && *value <= maximum
                )
            }
            Self::Unsigned { kind, value } => {
                matches!(integer_bounds(*kind), Some(IntegerBounds::Unsigned { maximum }) if *value <= maximum)
            }
            Self::F32(value) => value.is_finite(),
            Self::F64(value) => value.is_finite(),
            Self::Decimal {
                precision,
                scale,
                coefficient,
                literal_scale,
            } => decimal_value_fits(*precision, *scale, *coefficient, *literal_scale),
        }
    }
}

/// Validate a direct or lowered typed-numeric carrier against its canonical family and target range.
fn validate_numeric_value(
    value: &ReplacementNumericValue,
    span: HirSourceSpan,
) -> Result<(), ReplacementExecutionError> {
    if value.is_valid() {
        Ok(())
    } else {
        Err(unsupported(
            format!("malformed typed numeric carrier for `{}`", value.type_name()),
            span,
        ))
    }
}

/// Return whether one typed carrier exactly satisfies a checked semantic type.
fn numeric_value_matches_type(value: &ReplacementNumericValue, ty: &IncanType) -> bool {
    match (value, ty) {
        (
            ReplacementNumericValue::Signed { kind: actual, .. }
            | ReplacementNumericValue::Unsigned { kind: actual, .. },
            IncanType::Primitive(IncanPrimitiveType::Numeric(expected)),
        ) => actual == expected,
        (ReplacementNumericValue::F32(_), IncanType::Primitive(IncanPrimitiveType::Numeric(NumericTypeId::F32)))
        | (ReplacementNumericValue::F64(_), IncanType::Primitive(IncanPrimitiveType::Numeric(NumericTypeId::F64))) => {
            true
        }
        (
            ReplacementNumericValue::Decimal {
                precision: actual_precision,
                scale: actual_scale,
                ..
            },
            IncanType::Decimal {
                precision: expected_precision,
                scale: expected_scale,
            },
        ) => actual_precision == expected_precision && actual_scale == expected_scale,
        _ => false,
    }
}

/// Apply the typechecker's lossless numeric-widening contract at a Body-IR value boundary.
///
/// The target type decides the resulting carrier. In particular, widening `f32` to explicit `f64` produces an
/// `F64` carrier, while widening it to ordinary `float` produces the ordinary `Float` carrier. Keeping the source
/// tag after a checked widening would make later output and receipt evidence describe the wrong program type.
fn coerce_value_to_checked_type(
    value: ReplacementValue,
    target: &IncanType,
    span: HirSourceSpan,
) -> Result<ReplacementValue, ReplacementExecutionError> {
    let source_is_numeric = replacement_numeric_type_id(&value).is_some()
        || matches!(
            &value,
            ReplacementValue::Numeric(ReplacementNumericValue::Decimal { .. })
        );
    let target_is_numeric = checked_numeric_type_id(target).is_some() || matches!(target, IncanType::Decimal { .. });
    // Some normalized Body-IR temporaries intentionally carry task/generator frames in a slot whose source-level
    // type names the value eventually produced by that frame. Numeric coercion applies only at an actual
    // numeric-to-numeric boundary; nonnumeric internal carriers keep their established runtime representation.
    if !source_is_numeric || !target_is_numeric {
        return Ok(value);
    }

    if let (ReplacementValue::Numeric(decimal @ ReplacementNumericValue::Decimal { .. }), IncanType::Decimal { .. }) =
        (&value, target)
        && numeric_value_matches_type(decimal, target)
    {
        return Ok(value);
    }

    let Some(source) = replacement_numeric_type_id(&value) else {
        return Err(numeric_type_mismatch(&value, target, span));
    };
    let Some(target_id) = checked_numeric_type_id(target) else {
        return Err(numeric_type_mismatch(&value, target, span));
    };
    if !incan_core::numeric_values::numeric_type_losslessly_widens_to(source, target_id) {
        return Err(numeric_type_mismatch(&value, target, span));
    }

    let widened = widen_numeric_carrier(value, target_id).ok_or_else(|| {
        unsupported(
            format!(
                "typed numeric carrier could not realize checked lossless widening from `{}` to `{}`",
                numerics::as_str(source),
                target
            ),
            span,
        )
    })?;
    if matches!(target, IncanType::Primitive(IncanPrimitiveType::Numeric(_))) {
        match &widened {
            ReplacementNumericValue::F32(value) if !value.is_finite() => {
                return Err(runtime_failure(
                    IncanError::non_finite_exact_float("f32").to_string(),
                    span,
                ));
            }
            ReplacementNumericValue::F64(value) if !value.is_finite() => {
                return Err(runtime_failure(
                    IncanError::non_finite_exact_float("f64").to_string(),
                    span,
                ));
            }
            _ => validate_numeric_value(&widened, span)?,
        }
    }
    match target {
        IncanType::Primitive(IncanPrimitiveType::Int) => match widened {
            ReplacementNumericValue::Signed { value, .. } => i64::try_from(value)
                .map(ReplacementValue::Int)
                .map_err(|_| unsupported("lossless widening produced an out-of-range ordinary int", span)),
            _ => Err(unsupported(
                "lossless widening produced a non-integer ordinary int",
                span,
            )),
        },
        IncanType::Primitive(IncanPrimitiveType::Float) => match widened {
            ReplacementNumericValue::F64(value) => Ok(ReplacementValue::Float(value)),
            _ => Err(unsupported("lossless widening produced a non-f64 ordinary float", span)),
        },
        IncanType::Primitive(IncanPrimitiveType::Numeric(_)) => Ok(ReplacementValue::Numeric(widened)),
        _ => Err(numeric_type_mismatch(&ReplacementValue::Numeric(widened), target, span)),
    }
}

/// Build the canonical source-span refusal for a runtime carrier that contradicts its checked numeric destination.
fn numeric_type_mismatch(
    value: &ReplacementValue,
    target: &IncanType,
    span: HirSourceSpan,
) -> ReplacementExecutionError {
    unsupported(
        format!(
            "{} carrier contradicts checked numeric destination type `{target}`",
            value_kind(value)
        ),
        span,
    )
}

/// Project a runtime binary-numeric carrier to its canonical compiler-owned numeric identity.
fn replacement_numeric_type_id(value: &ReplacementValue) -> Option<NumericTypeId> {
    match value {
        ReplacementValue::Int(_) => Some(NumericTypeId::I64),
        ReplacementValue::Float(_) => Some(NumericTypeId::F64),
        ReplacementValue::Numeric(ReplacementNumericValue::Signed { kind, .. })
        | ReplacementValue::Numeric(ReplacementNumericValue::Unsigned { kind, .. }) => Some(*kind),
        ReplacementValue::Numeric(ReplacementNumericValue::F32(_)) => Some(NumericTypeId::F32),
        ReplacementValue::Numeric(ReplacementNumericValue::F64(_)) => Some(NumericTypeId::F64),
        _ => None,
    }
}

/// Project a checked ordinary or exact binary-numeric type to its canonical numeric identity.
fn checked_numeric_type_id(ty: &IncanType) -> Option<NumericTypeId> {
    match ty {
        IncanType::Primitive(IncanPrimitiveType::Int) => Some(NumericTypeId::I64),
        IncanType::Primitive(IncanPrimitiveType::Float) => Some(NumericTypeId::F64),
        IncanType::Primitive(IncanPrimitiveType::Numeric(kind)) if *kind != NumericTypeId::Bool => Some(*kind),
        _ => None,
    }
}

/// Materialize the target carrier after the typechecker has approved one lossless numeric widening.
fn widen_numeric_carrier(value: ReplacementValue, target: NumericTypeId) -> Option<ReplacementNumericValue> {
    match target {
        NumericTypeId::I8
        | NumericTypeId::I16
        | NumericTypeId::I32
        | NumericTypeId::I64
        | NumericTypeId::I128
        | NumericTypeId::ISize => {
            let value = match value {
                ReplacementValue::Int(value) => i128::from(value),
                ReplacementValue::Numeric(ReplacementNumericValue::Signed { value, .. }) => value,
                ReplacementValue::Numeric(ReplacementNumericValue::Unsigned { value, .. }) => {
                    i128::try_from(value).ok()?
                }
                _ => return None,
            };
            Some(ReplacementNumericValue::Signed { kind: target, value })
        }
        NumericTypeId::U8
        | NumericTypeId::U16
        | NumericTypeId::U32
        | NumericTypeId::U64
        | NumericTypeId::U128
        | NumericTypeId::USize => match value {
            ReplacementValue::Numeric(ReplacementNumericValue::Unsigned { value, .. }) => {
                Some(ReplacementNumericValue::Unsigned { kind: target, value })
            }
            _ => None,
        },
        NumericTypeId::F32 => match value {
            ReplacementValue::Numeric(ReplacementNumericValue::F32(value)) => Some(ReplacementNumericValue::F32(value)),
            _ => None,
        },
        NumericTypeId::F64 => match value {
            ReplacementValue::Float(value) => Some(ReplacementNumericValue::F64(value)),
            ReplacementValue::Numeric(ReplacementNumericValue::F32(value)) => {
                Some(ReplacementNumericValue::F64(f64::from(value)))
            }
            ReplacementValue::Numeric(ReplacementNumericValue::F64(value)) => Some(ReplacementNumericValue::F64(value)),
            _ => None,
        },
        NumericTypeId::Bool => None,
    }
}

/// Private list cursors for one compiler-selected Zip invocation.
///
/// Construction evaluates both source operands once in written order. Polling then advances the left list before
/// the right and returns no pair as soon as either is exhausted; this carrier grants no general iterator admission.
#[derive(Debug, Clone, PartialEq)]
pub struct ReplacementZip {
    left: ReplacementValue,
    right: ReplacementValue,
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

/// One unpolled or terminal source-local async function frame.
///
/// This is deliberately separate from [`GeneratorFrame`]. A generator yields values through iterator polling;
/// this frame carries one awaited return value and can be cancelled by a selected `race for` boundary.
#[derive(Debug, Clone, PartialEq)]
pub struct ReplacementTask {
    id: usize,
    body: Body,
    locals: BTreeMap<LocalId, ReplacementValue>,
    state: ReplacementTaskState,
}

/// Lifecycle state for one direct replacement task frame.
#[derive(Debug, Clone, PartialEq)]
enum ReplacementTaskState {
    Constructed,
    Running,
    Completed(ReplacementValue),
    /// The frame stopped at a source-observable failure and may never be polled or cancelled again.
    ///
    /// This stays distinct from [`Self::Cancelled`]: a failed source-order race winner did not lose the race. The
    /// original execution error is returned immediately, so retaining a second error payload in the task is neither
    /// needed nor a receipt authority.
    Failed,
    Cancelled,
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

/// Convert one registry-owned value-enum literal to its admitted scalar only after backing-category validation.
///
/// This deliberately accepts neither arbitrary `Constant` shapes nor a raw scalar carried by an execution target:
/// the declaration registry remains the single source of truth for a direct value enum's value representation.
fn value_enum_scalar_value(
    declaration: &ValueEnumDeclaration,
    variant: &ValueEnumVariantDeclaration,
    span: HirSourceSpan,
) -> Result<ReplacementValue, ReplacementExecutionError> {
    match (&declaration.backing, &variant.raw_value) {
        (ValueEnumBacking::Int, Constant::Int(value)) => Ok(ReplacementValue::Int(*value)),
        (ValueEnumBacking::Str, Constant::Str(value)) => Ok(ReplacementValue::Str(value.clone())),
        _ => Err(unsupported(
            format!(
                "value enum `{}` member `{}` has a raw literal incompatible with its retained scalar backing",
                declaration.name, variant.name
            ),
            span,
        )),
    }
}

/// Return whether `id` is one canonical span-derived declaration identity owned by `module`.
///
/// The registry is supplied alongside executable Body IR, so membership alone cannot establish source locality: a
/// malformed module could otherwise carry a coherent-looking foreign record and target. Direct value-enum execution
/// accepts only the exact `CompilerNodeId::declaration_span` shape emitted for this module by lowering.
fn is_module_span_declaration_id(module: &BodyIrModule, id: &CompilerNodeId) -> bool {
    if module.module_id.kind() != CompilerNodeKind::Module || id.kind() != CompilerNodeKind::Declaration {
        return false;
    }
    let prefix = format!("{}#decl.", module.module_id.path());
    let Some(span) = id.path().strip_prefix(&prefix) else {
        return false;
    };
    let Some((start, end)) = span.split_once("..") else {
        return false;
    };
    matches!(
        (start.parse::<usize>(), end.parse::<usize>()),
        (Ok(start), Ok(end)) if start <= end
    )
}

/// Return whether a body retains precisely the source-span identity lowering derives for its own declaration.
///
/// A caller's direct identity is not enough to establish a dispatch target: malformed Body IR could copy a valid
/// same-module identity onto an unrelated body. Direct execution therefore requires both a unique target match and
/// this body-local canonicality check before it enters the child frame.
fn has_canonical_direct_call_id(module: &BodyIrModule, body: &Body) -> bool {
    body.direct_call_id == CompilerNodeId::declaration_span(module.module_id.path(), body.span.start, body.span.end)
        && body.canonical.as_ref().is_some_and(|canonical| {
            canonical.declaration_name == body.name
                && direct_declaration_id_for_canonical(
                    module,
                    canonical,
                    SymbolNamespace::OrdinaryLexical,
                    SemanticSourceTargetKind::Function,
                ) == Some(body.direct_call_id.clone())
        })
}

/// Project one retained source-module identity onto the physical declaration id used by this Body-IR module.
///
/// This never mints or completes a semantic identity. It only checks that the already-retained identity belongs to
/// this module, then derives the local span key needed to address the module's physical declaration records.
fn direct_declaration_id_for_canonical(
    module: &BodyIrModule,
    identity: &CanonicalSymbolId,
    expected_namespace: SymbolNamespace,
    expected_kind: SemanticSourceTargetKind,
) -> Option<CompilerNodeId> {
    let SymbolOrigin::Module(module_path) = &identity.origin else {
        return None;
    };
    (identity.namespace == expected_namespace
        && identity.kind == expected_kind
        && identity.scope_discriminant.is_none()
        && module_identity_for_path(module_path) == module.module_id.path())
    .then(|| {
        CompilerNodeId::declaration_span(
            module.module_id.path(),
            identity.declaration_span.start,
            identity.declaration_span.end,
        )
    })
}

/// Validate that a retained plain-model layout is internally consistent with its checked canonical identities.
fn valid_local_nominal_declaration(module: &BodyIrModule, declaration: &NominalDeclaration) -> bool {
    direct_declaration_id_for_canonical(
        module,
        &declaration.canonical,
        SymbolNamespace::OrdinaryLexical,
        SemanticSourceTargetKind::Model,
    ) == Some(declaration.direct_declaration_id.clone())
        && declaration.canonical.declaration_name == declaration.name
        && declaration.fields.len() == declaration.field_identities.len()
        && declaration.fields.iter().collect::<BTreeSet<_>>().len() == declaration.fields.len()
        && declaration.field_identities.iter().collect::<BTreeSet<_>>().len() == declaration.field_identities.len()
        && declaration
            .fields
            .iter()
            .zip(&declaration.field_identities)
            .all(|(name, identity)| {
                identity.namespace == SymbolNamespace::Member
                    && identity.kind == SemanticSourceTargetKind::Field
                    && identity.scope_discriminant.is_none()
                    && identity.origin == declaration.canonical.origin
                    && identity.declaration_name == *name
            })
}

/// Validate one retained enum owner/member registry without recovering any identity from its source spelling.
fn valid_local_enum_declaration<T>(
    module: &BodyIrModule,
    direct_declaration_id: &CompilerNodeId,
    canonical: &CanonicalSymbolId,
    name: &str,
    variants: &[T],
    variant_facts: impl Fn(&T) -> (&CompilerNodeId, &CanonicalSymbolId, &str),
) -> bool {
    direct_declaration_id_for_canonical(
        module,
        canonical,
        SymbolNamespace::OrdinaryLexical,
        SemanticSourceTargetKind::Enum,
    ) == Some(direct_declaration_id.clone())
        && canonical.declaration_name == name
        && variants
            .iter()
            .map(|variant| variant_facts(variant).0)
            .collect::<BTreeSet<_>>()
            .len()
            == variants.len()
        && variants
            .iter()
            .map(|variant| variant_facts(variant).2)
            .collect::<BTreeSet<_>>()
            .len()
            == variants.len()
        && variants.iter().all(|variant| {
            let (direct_variant_id, variant_canonical, variant_name) = variant_facts(variant);
            direct_declaration_id_for_canonical(
                module,
                variant_canonical,
                SymbolNamespace::Member,
                SemanticSourceTargetKind::Variant,
            ) == Some(direct_variant_id.clone())
                && variant_canonical.origin == canonical.origin
                && variant_canonical.declaration_name == variant_name
        })
}

/// Return whether a retained fieldless enum is internally bound to this Body-IR module.
fn valid_local_fieldless_enum_declaration(module: &BodyIrModule, declaration: &FieldlessEnumDeclaration) -> bool {
    valid_local_enum_declaration(
        module,
        &declaration.direct_declaration_id,
        &declaration.canonical,
        &declaration.name,
        &declaration.variants,
        |variant| {
            (
                &variant.direct_declaration_id,
                &variant.canonical,
                variant.name.as_str(),
            )
        },
    )
}

/// Return whether a retained value enum is internally bound to this Body-IR module.
fn valid_local_value_enum_declaration(module: &BodyIrModule, declaration: &ValueEnumDeclaration) -> bool {
    valid_local_enum_declaration(
        module,
        &declaration.direct_declaration_id,
        &declaration.canonical,
        &declaration.name,
        &declaration.variants,
        |variant| {
            (
                &variant.direct_declaration_id,
                &variant.canonical,
                variant.name.as_str(),
            )
        },
    )
}

impl ReplacementValue {
    /// Render a deterministic source-observable result spelling for replacement receipts and CLI output.
    pub fn observable_text(&self) -> String {
        match self {
            Self::Int(value) => value.to_string(),
            Self::Bool(value) => value.to_string(),
            Self::Str(value) => value.clone(),
            Self::Float(value) => value.to_string(),
            Self::Numeric(value) => value.observable_text(),
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
            Self::Set(values) => values.observable_text(),
            Self::Dict(values) => values.observable_text(),
            Self::Nominal {
                direct_declaration_id,
                fields,
            } => format!(
                "nominal({direct_declaration_id}){{{}}}",
                fields
                    .iter()
                    .map(|(field, value)| format!("{field}={}", value.observable_text()))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::FieldlessEnum {
                enum_declaration_id,
                variant_declaration_id,
            } => format!("fieldless_enum({enum_declaration_id}::{variant_declaration_id})"),
            Self::ValueEnum {
                enum_declaration_id,
                variant_declaration_id,
            } => format!("value_enum({enum_declaration_id}::{variant_declaration_id})"),
            Self::Result { kind, payload, .. } => format!("{}({})", kind.as_str(), payload.observable_text()),
            Self::Callable(_) => "<callable>".to_string(),
            Self::Generator(_) => "<generator>".to_string(),
            Self::Task(_) => "<task>".to_string(),
            Self::Adapter(_) => "<generator-adapter>".to_string(),
            Self::Zip(_) => "<zip-iterator>".to_string(),
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

    /// Return the exact checked scalar type carried across a direct entrypoint/report boundary.
    #[must_use]
    pub fn scalar_type_name(&self) -> Option<String> {
        match self {
            Self::Int(_) => Some("int".to_string()),
            Self::Bool(_) => Some("bool".to_string()),
            Self::Str(_) => Some("str".to_string()),
            Self::Float(_) => Some("float".to_string()),
            Self::Numeric(value) => Some(value.type_name()),
            Self::Unit => Some(constructor_name(ConstructorId::None).to_string()),
            _ => None,
        }
    }

    /// Stable typed value projection used by receipt identity.
    pub(crate) fn receipt_identity_text(&self) -> String {
        match self {
            Self::Numeric(value) => value.receipt_identity_text(),
            _ => self.observable_text(),
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

/// Canonical lifecycle evidence for one direct replacement async task transition.
///
/// This is report-only execution evidence, bound into [`ReplacementExecution::output_identity`]. The generic
/// backend receipt remains deliberately unaware of runtime-specific task implementation details.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct TaskLifecycleProjection {
    /// Stable per-execution task-frame identity in construction order.
    pub task_id: usize,
    /// Stable lifecycle transition label.
    pub event: &'static str,
    /// Original source span that caused the transition.
    pub span_start: usize,
    /// Original source span that caused the transition.
    pub span_end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TaskLifecycleEvent {
    task_id: usize,
    event: &'static str,
    span: HirSourceSpan,
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
    /// Every direct task-frame transition observed during this successful execution, in source execution order.
    task_lifecycle: Vec<TaskLifecycleEvent>,
    /// Accepted program-stream bytes and completed print calls, independent of receipt publication.
    pub output: ProgramOutput,
    /// Every provider operation this execution ran, each referencing its own RFC 104 operation receipt.
    ///
    /// Empty for a run that executed no provider operation. A run that was *refused* by a governed denial or a
    /// provider failure never produces a [`ReplacementExecution`] at all, so its receipts are read from the
    /// [`provider::ProviderRuntime`] the caller supplied rather than from here.
    provider_executions: Vec<ProviderExecutionRecord>,
    /// Content identity of the actual Body-IR snapshot, ownership facts, requirements, provider executions,
    /// observed result, and emitted program output.
    pub output_identity: String,
}

/// A free-function execution that has passed the bounded #988 profile validator.
///
/// The capability retains the exact typed Body IR, source-level function name, and concrete arguments that were
/// validated. It lets selection/receipt code decide whether direct execution may proceed without rerunning profile
/// validation or allowing an unvalidated Body IR body to reach the executor.
/// The set of Body-IR modules one replacement execution may resolve a call into.
///
/// The executor previously held a single `&BodyIrModule`, which is exactly right for the bounded #988 profile where
/// every reachable call is same-module: an imported callable deliberately carries no same-module `direct_call_id`,
/// so there was nothing to resolve elsewhere. #1260 widens that to a call graph crossing one source-local module
/// edge or one direct public path-package edge, which needs somewhere to look the callee's owning module up.
///
/// Resolution is by compiler-owned module identity. A module is never selected from a source spelling, an import
/// spelling, a source path, a generated Rust name, or declaration order, which is the property that lets the same
/// graph serve a local module edge and a package edge without the executor knowing which it crossed.
///
/// The single-module case remains a one-node graph, so the existing #988 behaviour is unchanged and its tests keep
/// proving the same thing.
#[derive(Debug, Clone)]
pub struct ReplacementExecutionGraph<'module> {
    primary: &'module BodyIrModule,
    reachable: Vec<&'module BodyIrModule>,
}

impl<'module> ReplacementExecutionGraph<'module> {
    /// Build the one-node graph that represents today's single-module execution.
    #[must_use]
    pub fn single_module(primary: &'module BodyIrModule) -> Self {
        Self {
            primary,
            reachable: Vec::new(),
        }
    }

    /// Build a graph whose entrypoint is `primary` and whose reachable callees may live in `reachable`.
    ///
    /// Duplicate module identities are rejected rather than silently de-duplicated: two modules claiming one identity
    /// means the caller assembled the graph from disagreeing analyses, and picking either one would make dispatch
    /// depend on assembly order.
    pub fn new(
        primary: &'module BodyIrModule,
        reachable: impl IntoIterator<Item = &'module BodyIrModule>,
    ) -> Result<Self, ReplacementExecutionError> {
        let mut modules: Vec<&'module BodyIrModule> = Vec::new();
        for module in reachable {
            if module.module_id == primary.module_id || modules.iter().any(|seen| seen.module_id == module.module_id) {
                // Anchor the refusal at the entrypoint module's first body, which is the only span this graph owns.
                // A graph-assembly fault has no single source construct to point at, and inventing a synthetic span
                // would put a location in a diagnostic that no source position backs.
                let span = primary
                    .bodies
                    .first()
                    .map(|body| body.span)
                    .unwrap_or(incan_semantics_core::HirSourceSpan { start: 0, end: 0 });
                return Err(unsupported(
                    "duplicate module identity in the replacement execution graph",
                    span,
                ));
            }
            modules.push(module);
        }
        Ok(Self {
            primary,
            reachable: modules,
        })
    }

    /// Return the module owning the entrypoint this execution was prepared for.
    #[must_use]
    pub fn primary(&self) -> &'module BodyIrModule {
        self.primary
    }

    /// Return every module this execution may resolve into, entrypoint first.
    pub fn modules(&self) -> impl Iterator<Item = &'module BodyIrModule> + '_ {
        std::iter::once(self.primary).chain(self.reachable.iter().copied())
    }

    /// Resolve the declaration a canonical target selects, together with the module that owns it.
    ///
    /// This is the cross-module counterpart to [`BodyIrModule::body_for_canonical_target`], which answers only for
    /// the module it is asked. Each module in the graph is asked in turn, and that method already refuses a target
    /// whose module path is not its own, so a match means the owning module was found rather than a same-named
    /// declaration in the wrong place.
    ///
    /// Two modules answering for one canonical identity is a contradiction rather than an ambiguity to break by
    /// order: an identity names exactly one declaration site. `None` is returned instead, because dispatching to
    /// either would make the choice depend on graph assembly order.
    #[must_use]
    pub fn body_for_canonical_target(
        &self,
        target: &incan_semantics_core::CanonicalSymbolId,
    ) -> Option<(&'module BodyIrModule, &'module Body)> {
        let mut found = self
            .modules()
            .filter_map(|module| module.body_for_canonical_target(target).map(|body| (module, body)));
        let first = found.next()?;
        found.next().is_none().then_some(first)
    }

    /// Resolve the module owning `module_id`, or `None` when the graph does not contain it.
    ///
    /// A `None` here is a refusal, not a fallback: an unresolvable callee must fail before program effects rather
    /// than resolve against the caller's own module, which is the invariant
    /// `a_cross_module_call_is_refused_by_the_single_module_executor` pins.
    #[must_use]
    pub fn module_for(&self, module_id: &CompilerNodeId) -> Option<&'module BodyIrModule> {
        self.modules().find(|module| module.module_id == *module_id)
    }
}

pub struct ValidatedFreeFunctionExecution<'module, 'args> {
    /// Every module this execution may resolve a call into, entrypoint first.
    ///
    /// Held as a graph rather than a single module so #1260 can widen dispatch without changing this type again. The
    /// single-module profile is a one-node graph, so today's resolution is unchanged: `graph.primary()` is the same
    /// module the executor used before.
    graph: ReplacementExecutionGraph<'module>,
    name: String,
    args: &'args [ReplacementValue],
    /// The provider runtime this execution's admitted provider operations were validated against.
    ///
    /// Retained on the capability rather than passed again at execution time so the runtime that answered
    /// "does any host execute this operation" is necessarily the one that later invokes it.
    providers: Option<Rc<ProviderRuntime>>,
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

    /// The lines this execution emitted through `print`/`println`, in emission order.
    ///
    /// These lines have already been delivered to the supplied program writer; callers must not replay them.
    /// They remain a compatibility projection for reports. Compare exact bytes in [`Self::output`] for stream parity.
    #[must_use]
    pub fn emitted_output(&self) -> &[String] {
        &self.output.printed_lines
    }

    /// Return the stable direct-task lifecycle evidence bound into this execution's output identity and CLI report.
    #[must_use]
    pub fn task_lifecycle_evidence(&self) -> Vec<TaskLifecycleProjection> {
        task_lifecycle_projection(&self.task_lifecycle)
    }

    /// Return the backend provider-execution receipts bound into this execution's output identity.
    ///
    /// Each entry references its RFC 104 operation receipt by sequence id rather than restating it, so a consumer
    /// that needs the authority decision or the recorded attributes reads the operation receipt itself.
    #[must_use]
    pub fn provider_execution_evidence(&self) -> Vec<provider::ProviderExecutionProjection> {
        self.provider_executions
            .iter()
            .map(ProviderExecutionRecord::projection)
            .collect()
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
        /// Module identity the span was measured in, when it is not the executed entrypoint's.
        ///
        /// A span is a byte range and means nothing without the file it was measured in. While only the entrypoint
        /// could raise a refusal this was implicit; once a call can leave that module, a refusal raised in another
        /// one has to say so or the diagnostic points at the wrong file.
        module_id: Option<String>,
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
    /// A program-stream write or flush failed after any earlier accepted bytes were already delivered.
    #[error("replacement backend {error} at original Incan source span {span_start}..{span_end}")]
    ProgramIo {
        /// Typed host failure; partial output stays in the caller-owned [`ProgramIo`].
        #[source]
        error: ProgramIoError,
        /// Original print or stream-operation span carried by Body IR.
        span: HirSourceSpan,
        /// Start byte offset duplicated for typed diagnostic formatting.
        span_start: usize,
        /// End byte offset duplicated for typed diagnostic formatting.
        span_end: usize,
    },
    /// An RFC 104 authority decision refused an admitted provider operation, so it never ran.
    ///
    /// Deliberately distinct from [`Self::RuntimeFailure`]: nothing executed, and the remedy is a grant rather than
    /// a change to the program. The referenced receipt is the denial's own record — a denied operation still emits
    /// one, which is why this error names it rather than reporting a bare refusal.
    #[error(
        "replacement backend refused provider operation `{operation}` at original Incan source span \
         {span_start}..{span_end}: {reason}"
    )]
    ProviderAuthorityDenied {
        /// Declaration name of the refused operation, from its canonical identity.
        operation: String,
        /// Why authority was refused, and which grant would permit it.
        reason: String,
        /// Sequence id of the denied RFC 104 operation receipt this refusal produced.
        receipt_sequence_id: u64,
        /// Original Incan source span carried by the operation's plan.
        span: HirSourceSpan,
        /// Start byte offset duplicated for typed error formatting.
        span_start: usize,
        /// End byte offset duplicated for typed error formatting.
        span_end: usize,
    },
    /// Authority was granted and the provider operation itself failed.
    ///
    /// Separate from a denial because the two have nothing in common but their visibility: here the operation ran,
    /// any resource it acquired was released, and governed or observed runs retain a failed receipt. Permissive
    /// execution deliberately reports no receipt.
    #[error(
        "replacement backend provider operation `{operation}` failed at original Incan source span \
         {span_start}..{span_end}: {detail}"
    )]
    ProviderOperationFailed {
        /// Declaration name of the failed operation, from its canonical identity.
        operation: String,
        /// Source-observable description of the provider's own failure.
        detail: String,
        /// Sequence id of the failed RFC 104 operation receipt, when reporting was enabled.
        receipt_sequence_id: Option<u64>,
        /// Original Incan source span carried by the operation's plan.
        span: HirSourceSpan,
        /// Start byte offset duplicated for typed error formatting.
        span_start: usize,
        /// End byte offset duplicated for typed error formatting.
        span_end: usize,
    },
}

impl ReplacementExecutionError {
    /// Record the module a refusal's span was measured in, when it is not the executed entrypoint's.
    ///
    /// Applied where a walk crosses into another module, so every refusal raised beyond that point carries its own
    /// source rather than inheriting the entrypoint's. An error that already names a module keeps it: the innermost
    /// module that refused is the one that owns the span.
    pub(crate) fn measured_in_module(self, module: &str) -> Self {
        match self {
            Self::Unsupported {
                description,
                span,
                span_start,
                span_end,
                module_id,
            } => Self::Unsupported {
                description,
                span,
                span_start,
                span_end,
                module_id: module_id.or_else(|| Some(module.to_string())),
            },
            other => other,
        }
    }

    /// Return the module identity a refusal's span was measured in, when it is not the entrypoint's.
    pub(crate) fn measured_module(&self) -> Option<&str> {
        match self {
            Self::Unsupported { module_id, .. } => module_id.as_deref(),
            _ => None,
        }
    }

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
            Self::RuntimeFailure { .. } | Self::ProgramIo { .. } => "INCAN-R988-RUNTIME",
            Self::ProviderAuthorityDenied { .. } => "INCAN-R1156-DENIED",
            Self::ProviderOperationFailed { .. } => "INCAN-R1156-PROVIDER",
        }
    }

    /// Return the original Incan source location when this outcome arose from Body IR.
    pub const fn primary_span(&self) -> Option<HirSourceSpan> {
        match self {
            Self::Unsupported { span, .. }
            | Self::RuntimeFailure { span, .. }
            | Self::ProgramIo { span, .. }
            | Self::ProviderAuthorityDenied { span, .. }
            | Self::ProviderOperationFailed { span, .. } => Some(*span),
            Self::MissingFunction { .. } | Self::ArgumentCount { .. } => None,
        }
    }

    /// Return the RFC 104 operation receipt this outcome emitted, when it emitted one.
    ///
    /// Governed denials always do; provider failures do when reporting was enabled. A refusal that happened before
    /// the operation was invoked — an unsupported construct, an inactive provider, an unresolved operation — and a
    /// reporting-disabled permissive invocation deliberately have no receipt to name.
    pub const fn operation_receipt(&self) -> Option<super::replacement::provider::ProviderReceiptLink> {
        match self {
            Self::ProviderAuthorityDenied {
                receipt_sequence_id, ..
            } => Some(super::replacement::provider::ProviderReceiptLink {
                sequence_id: *receipt_sequence_id,
            }),
            Self::ProviderOperationFailed {
                receipt_sequence_id, ..
            } => match receipt_sequence_id {
                Some(sequence_id) => Some(super::replacement::provider::ProviderReceiptLink {
                    sequence_id: *sequence_id,
                }),
                None => None,
            },
            Self::MissingFunction { .. }
            | Self::ArgumentCount { .. }
            | Self::Unsupported { .. }
            | Self::RuntimeFailure { .. }
            | Self::ProgramIo { .. } => None,
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
    prepare_free_function_execution_with_providers(module, name, args, None)
}

/// Validate and prepare one Body-IR free function that may invoke admitted provider operations.
///
/// A `None` runtime is not a permissive default: it means no host executes any provider operation in this run, so
/// every admitted provider call refuses at its own source span during this pre-execution gate rather than part-way
/// through the body. That is what keeps [`prepare_free_function_execution`]'s existing contract — refuse before
/// executing, and emit no receipt for a refusal — true for the provider vocabulary too.
pub fn prepare_free_function_execution_with_providers<'module, 'args>(
    module: &'module BodyIrModule,
    name: &str,
    args: &'args [ReplacementValue],
    providers: Option<&Rc<ProviderRuntime>>,
) -> Result<ValidatedFreeFunctionExecution<'module, 'args>, ReplacementExecutionError> {
    prepare_free_function_execution_in_graph(ReplacementExecutionGraph::single_module(module), name, args, providers)
}

/// Validate and prepare one Body-IR free function whose reachable calls may leave its own module.
///
/// The graph names every module a call may resolve into, entrypoint first. Validation still runs against the
/// entrypoint's module, because that is where the selected body and its arguments live; what the graph adds is the
/// ability for a frame to execute against the module its callee was resolved to rather than the module the call was
/// written in.
///
/// A one-node graph is exactly the previous behaviour, which is why
/// [`prepare_free_function_execution_with_providers`] delegates here rather than duplicating the validation order.
pub fn prepare_free_function_execution_in_graph<'module, 'args>(
    graph: ReplacementExecutionGraph<'module>,
    name: &str,
    args: &'args [ReplacementValue],
    providers: Option<&Rc<ProviderRuntime>>,
) -> Result<ValidatedFreeFunctionExecution<'module, 'args>, ReplacementExecutionError> {
    let module = graph.primary();
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
    validate_selected_parameter_arguments(&body.params, args)?;
    validate_reachable_typed_numeric_profile(&graph, module, body)?;
    let preflight_reachable: Vec<BodyIrModule> = graph
        .modules()
        .filter(|candidate| candidate.module_id != module.module_id)
        .cloned()
        .collect();
    execution_preflight::validate(module, &preflight_reachable, body, providers.map(Rc::as_ref))?;
    Ok(ValidatedFreeFunctionExecution {
        graph,
        name: name.to_string(),
        args,
        providers: providers.cloned(),
    })
}

/// Validate the structural direct-execution invariants of one body before it is executed or stored as a lazy frame.
///
/// The selected entrypoint and every same-module named callee use this one gate. Applying it only at the entrypoint
/// would let an otherwise admitted call dispatch an unvalidated sibling body and publish a receipt for a profile the
/// runtime promises to refuse. Provider-host availability is checked separately across the reachable computation
/// once during preparation; runtime invocation still rechecks the host and authority.
fn validate_direct_body_profile(body: &Body) -> Result<(), ReplacementExecutionError> {
    // An `async def` produces an awaitable even when its body has no explicit `await`. Executing its statements as
    // an ordinary scalar body would erase task construction, suspension, wake, cancellation, and receipt semantics
    // that belong to #1155. The stored declaration fact is therefore a direct profile boundary, not something this
    // executor may infer by scanning the block for an await statement.
    if body.is_async {
        return validate_direct_async_body_profile(body);
    }
    let range_iterator_locals = range_iterator_locals(&body.block);
    let zip_iterator_locals = list_iteration::validate_body(body)?;
    validate_collection_local_types(body, &body.block.stmts, &range_iterator_locals, &zip_iterator_locals)?;
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

/// Resolve one named call by its retained same-module identity for both preflight and runtime dispatch.
///
/// The source name is diagnostic spelling only after unique, module-scoped identity selection. Neither caller may
/// validate or dispatch by that spelling, fall back to a name lookup, or admit an imported or malformed body
/// identity.
fn named_callable_body<'module>(
    module: &'module BodyIrModule,
    target: &NamedCallableTarget,
    span: HirSourceSpan,
) -> Result<&'module Body, ReplacementExecutionError> {
    let direct_call_id = target.direct_call_id.as_ref().ok_or_else(|| {
        unsupported(
            format!(
                "named callable `{}` without a same-module declaration identity",
                target.name
            ),
            span,
        )
    })?;
    if !is_module_span_declaration_id(module, direct_call_id) {
        return Err(unsupported(
            "named callable declaration identity is not scoped to this Body-IR module",
            span,
        ));
    }
    let canonical = target.canonical.as_ref().ok_or_else(|| {
        unsupported(
            format!(
                "named callable `{}` without a canonical declaration target",
                target.name
            ),
            span,
        )
    })?;
    let canonical_call_id = direct_declaration_id_for_canonical(
        module,
        canonical,
        SymbolNamespace::OrdinaryLexical,
        SemanticSourceTargetKind::Function,
    )
    .ok_or_else(|| {
        unsupported(
            "named callable canonical target is not owned by this Body-IR module",
            span,
        )
    })?;
    if canonical_call_id != *direct_call_id {
        return Err(unsupported(
            "named callable canonical target disagrees with its physical Body-IR declaration",
            span,
        ));
    }
    let mut matching_bodies = module
        .bodies
        .iter()
        .filter(|body| body.direct_call_id == *direct_call_id);
    let body = matching_bodies.next().ok_or_else(|| {
        unsupported(
            format!(
                "named callable `{}` targets a declaration outside this Body-IR module",
                target.name
            ),
            span,
        )
    })?;
    if matching_bodies.next().is_some() {
        return Err(unsupported(
            format!(
                "named callable `{}` declaration identity selects multiple Body-IR bodies",
                target.name
            ),
            span,
        ));
    }
    if !has_canonical_direct_call_id(module, body) {
        return Err(unsupported(
            format!(
                "named callable `{}` body does not retain its canonical declaration identity",
                target.name
            ),
            span,
        ));
    }
    if body.canonical.as_ref() != Some(canonical) {
        return Err(unsupported(
            format!(
                "named callable `{}` target disagrees with the selected body's canonical identity",
                target.name
            ),
            span,
        ));
    }
    Ok(body)
}

/// Validate the deliberately source-local async subset without treating a task frame as a synchronous body.
///
/// Race-arm scopes are explicit in Body IR, so this may admit the same binding spelling independently in each arm
/// while retaining the normal fail-closed rejection for shadowing outside an arm. Every ordinary statement still
/// passes through the existing direct profile validator.
fn validate_direct_async_body_profile(body: &Body) -> Result<(), ReplacementExecutionError> {
    if body.is_generator() {
        return Err(unsupported("async generator body", body.span));
    }
    let range_iterator_locals = range_iterator_locals(&body.block);
    let zip_iterator_locals = list_iteration::validate_body(body)?;
    validate_collection_local_types(body, &body.block.stmts, &range_iterator_locals, &zip_iterator_locals)?;
    validate_nested_structural_aggregate_types(body)?;
    let tuple_iteration_locals = builtin_iteration_destinations(&body.block);
    let scalar_tuple_collection_locals = scalar_tuple_collection_elements(&body.block);
    validate_callable_params_profile(&body.params)?;
    validate_async_block_profile(&body.block, &tuple_iteration_locals, &scalar_tuple_collection_locals)
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

/// Execute checked Body IR using caller-supplied program writers and retain observations on failure as well as success.
///
/// Profile validation runs before any program write. The caller owns `io` after return, including accepted prefixes
/// when a runtime error, broken pipe, or flush failure prevented a successful execution result.
pub fn execute_free_function_with_io(
    module: &BodyIrModule,
    name: &str,
    args: &[ReplacementValue],
    io: &mut ProgramIo<'_>,
) -> Result<ReplacementExecution, ReplacementExecutionError> {
    let execution = prepare_free_function_execution(module, name, args)?;
    execute_prevalidated_free_function_with_io(execution, io)
}

/// Execute one named free function that may invoke admitted provider operations against `providers`.
///
/// The runtime is the caller's, and stays the caller's: a governed denial and a provider failure both stop this
/// execution with an error, and their RFC 104 receipts are read from `providers` afterwards. Returning receipts
/// only on success would make the two outcomes RFC 104 most wants recorded the two it could not report.
pub fn execute_free_function_with_providers(
    module: &BodyIrModule,
    name: &str,
    args: &[ReplacementValue],
    providers: &Rc<ProviderRuntime>,
) -> Result<ReplacementExecution, ReplacementExecutionError> {
    let execution = prepare_free_function_execution_with_providers(module, name, args, Some(providers))?;
    execute_prevalidated_free_function(execution)
}

/// Execute one free function that has already passed [`prepare_free_function_execution`].
///
/// This consumes the validated capability, preserving the rule that callers select the source profile before the
/// direct Body-IR executor observes a result.
pub fn execute_prevalidated_free_function(
    execution: ValidatedFreeFunctionExecution<'_, '_>,
) -> Result<ReplacementExecution, ReplacementExecutionError> {
    let mut stdout = std::io::stdout().lock();
    let mut stderr = std::io::stderr().lock();
    let mut io = ProgramIo::new(&mut stdout, &mut stderr);
    execute_prevalidated_free_function_with_io(execution, &mut io)
}

/// Execute a validated capability with ordinary program delivery and independently caller-owned observation.
///
/// Nested frames reborrow the same writers. Printing writes and flushes during execution, never after a receipt
/// succeeds; a failed execution still leaves accepted bytes available through `io.output()`.
pub fn execute_prevalidated_free_function_with_io(
    execution: ValidatedFreeFunctionExecution<'_, '_>,
    io: &mut ProgramIo<'_>,
) -> Result<ReplacementExecution, ReplacementExecutionError> {
    let body = named_free_function(execution.graph.primary(), &execution.name)?;
    let checkpoint = io.checkpoint();
    let mut executor = BodyExecutor::new(&execution.graph, body, execution.args, execution.providers.clone(), io)?;
    let (value, result_span) = if body.is_async {
        let task = executor.construct_task(body.clone(), executor.locals.clone(), body.span)?;
        (executor.drive_task(&task, body.span)?, body.span)
    } else {
        let flow = executor.execute_block(&body.block)?;
        match flow {
            Flow::Return(value, span) => (
                match value {
                    Some(value) => value,
                    None => ReplacementValue::Unit,
                },
                span,
            ),
            Flow::Next => (ReplacementValue::Unit, body.span),
            Flow::Break | Flow::Continue => {
                return Err(unsupported("loop control outside a normalized loop", body.span));
            }
        }
    };
    let value = coerce_value_to_checked_type(value, &body.return_type, result_span)?;
    ensure_scalar_result(&value, &body.return_type, result_span)?;
    let body_snapshot = executor.body_snapshot();
    let ownership_summary = canonical_ownership_summary(&executor.ownership_reads);
    let requirements_summary = canonical_runtime_requirements_summary(&executor.runtime_requirements);
    let task_summary = canonical_task_lifecycle_summary(&executor.task_lifecycle);
    let provider_executions = execution
        .providers
        .as_ref()
        .map(|runtime| runtime.provider_executions())
        .unwrap_or_default();
    let provider_summary = canonical_provider_execution_summary(&provider_executions);
    let output = executor.io.output_since(checkpoint);
    let emitted_output_summary = canonical_emitted_output_summary(&output.printed_lines);
    let stream_summary = format!(
        "program-streams-v1;stdout={};stderr={}",
        hex::encode(output.stdout()),
        hex::encode(output.stderr())
    );
    let output_identity = digest_output(&[
        body_snapshot.as_str(),
        value.receipt_identity_text().as_str(),
        ownership_summary.as_str(),
        requirements_summary.as_str(),
        task_summary.as_str(),
        provider_summary.as_str(),
        emitted_output_summary.as_str(),
        stream_summary.as_str(),
    ]);
    Ok(ReplacementExecution {
        value,
        body_snapshot,
        ownership_reads: executor.ownership_reads,
        runtime_requirements: executor.runtime_requirements,
        task_lifecycle: executor.task_lifecycle,
        output,
        provider_executions,
        output_identity,
    })
}

/// Locate the requested free-function body without inventing a fallback entrypoint.
fn named_free_function<'a>(module: &'a BodyIrModule, name: &str) -> Result<&'a Body, ReplacementExecutionError> {
    let mut candidates = module.bodies.iter().filter(|body| {
        body.canonical
            .as_ref()
            .is_some_and(|canonical| canonical.declaration_name == name)
            && has_canonical_direct_call_id(module, body)
    });
    let Some(body) = candidates.next() else {
        if let Some(named_body) = module.bodies.iter().find(|body| body.name == name) {
            return Err(unsupported(
                format!("entrypoint `{name}` without an exact canonical free-function identity"),
                named_body.span,
            ));
        }
        return Err(ReplacementExecutionError::MissingFunction { name: name.to_string() });
    };
    if candidates.next().is_some() {
        return Err(unsupported(
            format!("ambiguous overloaded free-function entrypoint `{name}`"),
            body.span,
        ));
    }
    Ok(body)
}

/// Reject non-scalar direct API arguments before they can widen the first replacement profile.
fn validate_scalar_arguments(args: &[ReplacementValue], span: HirSourceSpan) -> Result<(), ReplacementExecutionError> {
    for argument in args {
        if !matches!(
            argument,
            ReplacementValue::Int(_)
                | ReplacementValue::Bool(_)
                | ReplacementValue::Str(_)
                | ReplacementValue::Float(_)
                | ReplacementValue::Numeric(_)
                | ReplacementValue::Unit
        ) {
            return Err(unsupported(
                format!("{} argument in the scalar replacement profile", value_kind(argument)),
                span,
            ));
        }
        if let ReplacementValue::Numeric(value) = argument {
            validate_numeric_value(value, span)?;
        }
    }
    Ok(())
}

/// Keep a direct API caller from supplying a carrier that contradicts a checked parameter type.
///
/// The selected replacement entrypoint accepts only direct profile carriers, not a source typechecker context. A
/// supplied `Int` or `Str` therefore cannot stand in for a checked `float` parameter merely because the executor
/// can materialize that carrier. This is deliberately an entrypoint-only boundary: source-resolved sibling calls
/// keep their own checked call contract and must not be rejected by a whole-body parameter scan.
fn validate_selected_parameter_arguments(
    params: &[CallableParam],
    args: &[ReplacementValue],
) -> Result<(), ReplacementExecutionError> {
    for (parameter, argument) in params.iter().zip(args) {
        let matches = replacement_value_matches_checked_type(argument, &parameter.ty);
        if !matches {
            return Err(unsupported(
                format!(
                    "direct {} argument does not satisfy checked parameter `{}` of type `{}`",
                    value_kind(argument),
                    parameter.name,
                    parameter.ty
                ),
                parameter.span,
            ));
        }
    }
    Ok(())
}

/// Return whether one direct API carrier inhabits the checked parameter type selected by the frontend.
fn replacement_value_matches_checked_type(value: &ReplacementValue, ty: &IncanType) -> bool {
    match (ty, value) {
        (IncanType::Primitive(IncanPrimitiveType::Int), ReplacementValue::Int(_))
        | (IncanType::Primitive(IncanPrimitiveType::Float), ReplacementValue::Float(_))
        | (IncanType::Primitive(IncanPrimitiveType::Bool), ReplacementValue::Bool(_))
        | (IncanType::Primitive(IncanPrimitiveType::Str), ReplacementValue::Str(_))
        | (IncanType::Primitive(IncanPrimitiveType::Unit), ReplacementValue::Unit) => true,
        (ty, ReplacementValue::Numeric(value)) => numeric_value_matches_type(value, ty),
        (IncanType::Generic { base, args }, value)
            if base == incan_core::lang::types::UNION_TYPE_NAME && !args.is_empty() =>
        {
            args.iter()
                .any(|member| replacement_value_matches_checked_type(value, member))
        }
        _ => false,
    }
}

/// Validate the closed typed-numeric type vocabulary before execution.
fn validate_typed_numeric_types(body: &Body) -> Result<(), ReplacementExecutionError> {
    for local in &body.locals {
        if matches!(
            local.ty,
            IncanType::Primitive(IncanPrimitiveType::Numeric(NumericTypeId::Bool))
        ) {
            return Err(unsupported(
                "bool reached Body IR through the exact numeric type channel",
                local.span,
            ));
        }
        if let IncanType::Decimal { precision, scale } = local.ty
            && (precision == 0 || precision > 38 || scale > precision)
        {
            return Err(unsupported(
                format!("invalid checked decimal type `{}`", local.ty),
                local.span,
            ));
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy)]
enum TypedNumericProfileKind {
    Binary(NumericTypeId),
    Decimal,
}

impl TypedNumericProfileKind {
    /// Render the source-facing numeric kind named by a profile refusal.
    fn label(self) -> String {
        match self {
            Self::Binary(kind) => numerics::as_str(kind).to_string(),
            Self::Decimal => "decimal".to_string(),
        }
    }

    /// Return whether this profile kind is a fixed-scale decimal rather than a binary numeric.
    const fn is_decimal(self) -> bool {
        matches!(self, Self::Decimal)
    }
}

/// Validate every typed-numeric operation reachable from the selected entrypoint before program output begins.
///
/// The carrier itself is admitted. Operations are a separate contract: this walk permits checked movement,
/// lossless widening, direct calls, scalar conversions and Display output, while keeping arithmetic, Debug,
/// aggregates, methods and other unproved behavior explicitly non-green under #988.
fn validate_reachable_typed_numeric_profile(
    graph: &ReplacementExecutionGraph<'_>,
    module: &BodyIrModule,
    root: &Body,
) -> Result<(), ReplacementExecutionError> {
    let mut visited = BTreeSet::new();
    validate_typed_numeric_body_profile(graph, module, root, &mut visited)
}

/// Validate one reachable body once, following its source defaults and identity-selected sibling calls.
fn validate_typed_numeric_body_profile(
    graph: &ReplacementExecutionGraph<'_>,
    module: &BodyIrModule,
    body: &Body,
    visited: &mut BTreeSet<CompilerNodeId>,
) -> Result<(), ReplacementExecutionError> {
    if !visited.insert(body.direct_call_id.clone()) {
        return Ok(());
    }
    validate_typed_numeric_types(body).map_err(|error| error.measured_in_module(module.module_id.path()))?;
    for parameter in &body.params {
        if let CallableParamDefault::Source(computation) = &parameter.default {
            validate_typed_numeric_statements(graph, module, body, &computation.stmts, visited)?;
            let _ = typed_numeric_operand_kind(body, &computation.result, computation.span)?;
        }
    }
    validate_typed_numeric_statements(graph, module, body, &body.block.stmts, visited)
        .map_err(|error| error.measured_in_module(module.module_id.path()))
}

/// Validate a statement sequence for typed-numeric carrier movement and explicit operation refusals.
fn validate_typed_numeric_statements(
    graph: &ReplacementExecutionGraph<'_>,
    module: &BodyIrModule,
    body: &Body,
    statements: &[Statement],
    visited: &mut BTreeSet<CompilerNodeId>,
) -> Result<(), ReplacementExecutionError> {
    for statement in statements {
        validate_typed_numeric_statement(graph, module, body, statement, visited)?;
    }
    Ok(())
}

/// Validate one normalized statement without executing its effects.
fn validate_typed_numeric_statement(
    graph: &ReplacementExecutionGraph<'_>,
    module: &BodyIrModule,
    body: &Body,
    statement: &Statement,
    visited: &mut BTreeSet<CompilerNodeId>,
) -> Result<(), ReplacementExecutionError> {
    match &statement.kind {
        StatementKind::Assign { rvalue, .. } => {
            validate_typed_numeric_rvalue(graph, module, body, rvalue, statement.span, visited)
        }
        StatementKind::Call { callee, args, .. } => {
            validate_typed_numeric_call(graph, module, body, callee, args, statement.span, visited)
        }
        StatementKind::Drop { .. } | StatementKind::Continue | StatementKind::Unsupported { .. } => Ok(()),
        StatementKind::If {
            cond,
            then_block,
            else_block,
        } => {
            refuse_typed_numeric_operand(body, cond, statement.span, "condition")?;
            validate_typed_numeric_statements(graph, module, body, &then_block.stmts, visited)?;
            if let Some(else_block) = else_block {
                validate_typed_numeric_statements(graph, module, body, &else_block.stmts, visited)?;
            }
            Ok(())
        }
        StatementKind::Loop { body: loop_body } => {
            validate_typed_numeric_statements(graph, module, body, &loop_body.stmts, visited)
        }
        StatementKind::Break { value } => {
            if let Some(value) = value {
                let _ = typed_numeric_operand_kind(body, value, statement.span)?;
            }
            Ok(())
        }
        StatementKind::Return { value } => {
            if let Some(value) = value {
                let _ = typed_numeric_operand_kind(body, value, statement.span)?;
            }
            Ok(())
        }
        StatementKind::Assert { kind, message, .. } => {
            match kind {
                AssertionKind::Condition { cond } => {
                    refuse_typed_numeric_operand(body, cond, statement.span, "assertion condition")?;
                }
                AssertionKind::Pattern { scrutinee, pattern } => {
                    refuse_typed_numeric_operand(body, scrutinee, statement.span, "pattern assertion")?;
                    validate_typed_numeric_pattern(pattern, statement.span)?;
                }
                AssertionKind::Raises { call, .. } => {
                    refuse_typed_numeric_operand(body, call, statement.span, "raises assertion")?;
                }
            }
            if let Some(message) = message {
                let _ = typed_numeric_operand_kind(body, message, statement.span)?;
            }
            Ok(())
        }
        StatementKind::Expr { value } => {
            let _ = typed_numeric_operand_kind(body, value, statement.span)?;
            Ok(())
        }
        StatementKind::IterNext { iterator, .. } => {
            refuse_typed_numeric_operand(body, iterator, statement.span, "iteration")
        }
        StatementKind::Yield { value } => refuse_typed_numeric_operand(body, value, statement.span, "generator yield"),
        StatementKind::TryPropagate { operand, .. } => {
            refuse_typed_numeric_operand(body, operand, statement.span, "try propagation")
        }
        StatementKind::Await { awaited, .. } => refuse_typed_numeric_operand(body, awaited, statement.span, "await"),
        StatementKind::Race { arms, .. } => {
            for arm in arms {
                refuse_typed_numeric_operand(body, &arm.awaitable, statement.span, "race awaitable")?;
                validate_typed_numeric_statements(graph, module, body, &arm.body.stmts, visited)?;
                let _ = typed_numeric_operand_kind(body, &arm.result, statement.span)?;
            }
            Ok(())
        }
    }
}

/// Validate one rvalue, admitting carrier movement and refusing unproved typed-numeric operations.
fn validate_typed_numeric_rvalue(
    graph: &ReplacementExecutionGraph<'_>,
    module: &BodyIrModule,
    body: &Body,
    rvalue: &Rvalue,
    span: HirSourceSpan,
    visited: &mut BTreeSet<CompilerNodeId>,
) -> Result<(), ReplacementExecutionError> {
    match rvalue {
        Rvalue::Use(operand) => {
            let _ = typed_numeric_operand_kind(body, operand, span)?;
            Ok(())
        }
        Rvalue::UnaryOp(operator, operand) => {
            if let Some(kind) = typed_numeric_operand_kind(body, operand, span)? {
                return Err(typed_numeric_operation_refusal(
                    kind,
                    unary_label(*operator).to_string(),
                    span,
                ));
            }
            Ok(())
        }
        Rvalue::BinaryOp(operator, left, right) => {
            let left_kind = typed_numeric_operand_kind(body, left, span)?;
            let right_kind = typed_numeric_operand_kind(body, right, span)?;
            if let Some(kind) = left_kind.or(right_kind) {
                return Err(typed_numeric_operation_refusal(
                    kind,
                    binary_label(*operator).to_string(),
                    span,
                ));
            }
            Ok(())
        }
        Rvalue::IsInstance { value, .. } => refuse_typed_numeric_operand(body, value, span, "isinstance type test"),
        Rvalue::Format(parts) => {
            for part in parts {
                let FormatPart::Expr { operand, style } = part else {
                    continue;
                };
                if let Some(kind) = typed_numeric_operand_kind(body, operand, span)?
                    && matches!(style, FormatStyle::Debug)
                {
                    return Err(typed_numeric_operation_refusal(kind, "Debug formatting", span));
                }
            }
            Ok(())
        }
        Rvalue::Aggregate(_, operands) => {
            for operand in argument_operands(operands) {
                refuse_typed_numeric_operand(body, operand, span, "aggregate construction")?;
            }
            Ok(())
        }
        Rvalue::Dict(entries) => {
            for entry in entries {
                match entry {
                    DictEntry::Pair(key, value) => {
                        refuse_typed_numeric_operand(body, key, span, "dict construction")?;
                        refuse_typed_numeric_operand(body, value, span, "dict construction")?;
                    }
                    DictEntry::Spread(spread) => {
                        refuse_typed_numeric_operand(body, &spread.source, span, "dict spread")?;
                    }
                }
            }
            Ok(())
        }
        Rvalue::ResultVariant(variant) => {
            refuse_typed_numeric_operand(body, &variant.payload, span, "Result construction")
        }
        Rvalue::Closure {
            params,
            captured_operands,
            body: closure,
        } => {
            for operand in captured_operands {
                let _ = typed_numeric_operand_kind(body, operand, span)?;
            }
            for parameter in params {
                if let CallableParamDefault::Source(computation) = &parameter.default {
                    validate_typed_numeric_statements(graph, module, body, &computation.stmts, visited)?;
                    let _ = typed_numeric_operand_kind(body, &computation.result, computation.span)?;
                }
            }
            validate_typed_numeric_statements(graph, module, body, &closure.stmts, visited)?;
            let _ = typed_numeric_operand_kind(body, &closure.result, span)?;
            Ok(())
        }
        Rvalue::Generator {
            source,
            captured_operands,
            body: generator,
        } => {
            refuse_typed_numeric_operand(body, source, span, "generator source")?;
            for operand in captured_operands {
                let _ = typed_numeric_operand_kind(body, operand, span)?;
            }
            validate_typed_numeric_statements(graph, module, body, &generator.stmts, visited)
        }
        Rvalue::Match { scrutinee, arms } => {
            refuse_typed_numeric_operand(body, scrutinee, span, "match")?;
            for arm in arms {
                validate_typed_numeric_pattern(&arm.pattern, span)?;
                validate_typed_numeric_statements(graph, module, body, &arm.guard_stmts, visited)?;
                if let Some(guard) = &arm.guard {
                    refuse_typed_numeric_operand(body, guard, span, "match guard")?;
                }
                validate_typed_numeric_statements(graph, module, body, &arm.body_stmts, visited)?;
                let _ = typed_numeric_operand_kind(body, &arm.result, span)?;
            }
            Ok(())
        }
        Rvalue::FieldlessEnumVariant(_) | Rvalue::ValueEnumVariant(_) => Ok(()),
    }
}

/// Validate one call that receives a typed-numeric operand and follow any retained same-module target.
fn validate_typed_numeric_call(
    graph: &ReplacementExecutionGraph<'_>,
    module: &BodyIrModule,
    body: &Body,
    callee: &Callee,
    args: &[ArgumentElement],
    span: HirSourceSpan,
    visited: &mut BTreeSet<CompilerNodeId>,
) -> Result<(), ReplacementExecutionError> {
    let kinds = argument_operands(args)
        .map(|operand| typed_numeric_operand_kind(body, operand, span))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();

    // Follow the callee into whichever module declares it. A same-module target is reached by its span identity; one
    // that left the module carries a canonical identity instead, and the graph owns that resolution. Following both is
    // what makes admitting a cross-module call below sound: the callee's own operations are validated under the same
    // contract, in its own module, rather than being trusted because the call site could not see them.
    if let Callee::Function(CallableTarget::Named(target)) = callee
        && target.builtin.is_none()
    {
        if target.direct_call_id.is_some() {
            let called = named_callable_body(module, target, span)?;
            validate_typed_numeric_body_profile(graph, module, called, visited)?;
        } else if let Some(canonical) = target.canonical.as_ref()
            && let Some((owner, called)) = graph.body_for_canonical_target(canonical)
        {
            validate_typed_numeric_body_profile(graph, owner, called, visited)?;
        }
    }
    let Some(kind) = kinds.first().copied() else {
        return Ok(());
    };

    match callee {
        Callee::Function(CallableTarget::Named(target)) => match explicit_builtin(target) {
            Some(BuiltinFnId::Print | BuiltinFnId::Str) => Ok(()),
            Some(BuiltinFnId::Int | BuiltinFnId::Float) if !kinds.iter().any(|kind| kind.is_decimal()) => Ok(()),
            Some(BuiltinFnId::Int | BuiltinFnId::Float) => Err(typed_numeric_operation_refusal(
                kind,
                format!("`{}` conversion", target.name),
                span,
            )),
            // A direct call is admitted whether the declaration is in this module or another one the graph resolved.
            // `direct_call_id` alone made locality the criterion; the callee's operations are validated above either
            // way, so the distinction the profile actually cares about is that the frontend resolved the call at all.
            None if target.direct_call_id.is_some() || target.canonical.is_some() => Ok(()),
            other => Err(typed_numeric_operation_refusal(
                kind,
                format!(
                    "call to {}",
                    other.map_or_else(|| target.name.clone(), |builtin| builtins::as_str(builtin).to_string())
                ),
                span,
            )),
        },
        Callee::Function(CallableTarget::Local(_)) => Ok(()),
        Callee::Method(target) => Err(typed_numeric_operation_refusal(
            kind,
            format!("method `{}`", target.name),
            span,
        )),
        Callee::Helper(helper) => Err(typed_numeric_operation_refusal(
            kind,
            format!("helper `{}`", helper.as_str()),
            span,
        )),
        Callee::ProviderOperation(plan) => Err(typed_numeric_operation_refusal(
            kind,
            format!("provider operation `{}`", plan.operation.declaration_name),
            span,
        )),
    }
}

/// Iterate every operand represented by positional, named, or spread argument elements.
fn argument_operands(elements: &[ArgumentElement]) -> impl Iterator<Item = &Operand> {
    elements.iter().map(|element| match element {
        ArgumentElement::One(operand) | ArgumentElement::Named { operand, .. } => operand,
        ArgumentElement::Spread(spread) => &spread.source,
    })
}

/// Resolve an operand's exact numeric profile kind from its constant or declared local type.
fn typed_numeric_operand_kind(
    body: &Body,
    operand: &Operand,
    span: HirSourceSpan,
) -> Result<Option<TypedNumericProfileKind>, ReplacementExecutionError> {
    match operand {
        Operand::Constant(Constant::TypedNumeric(constant)) => {
            let value = typed_numeric_constant_value(constant, span)?;
            Ok(Some(match value {
                ReplacementNumericValue::Signed { kind, .. } | ReplacementNumericValue::Unsigned { kind, .. } => {
                    TypedNumericProfileKind::Binary(kind)
                }
                ReplacementNumericValue::F32(_) => TypedNumericProfileKind::Binary(NumericTypeId::F32),
                ReplacementNumericValue::F64(_) => TypedNumericProfileKind::Binary(NumericTypeId::F64),
                ReplacementNumericValue::Decimal { .. } => TypedNumericProfileKind::Decimal,
            }))
        }
        Operand::Constant(_) => Ok(None),
        Operand::Place(place) => Ok(typed_numeric_kind_from_type(declared_local_type(
            body,
            local_root(&place.place, span)?,
            span,
        )?)),
    }
}

/// Classify a checked exact binary-numeric or decimal type for the bounded execution profile.
fn typed_numeric_kind_from_type(ty: &IncanType) -> Option<TypedNumericProfileKind> {
    match ty {
        IncanType::Primitive(IncanPrimitiveType::Numeric(kind)) if *kind != NumericTypeId::Bool => {
            Some(TypedNumericProfileKind::Binary(*kind))
        }
        IncanType::Decimal { .. } => Some(TypedNumericProfileKind::Decimal),
        _ => None,
    }
}

/// Refuse an operand when it carries a typed numeric through an operation outside the admitted profile.
fn refuse_typed_numeric_operand(
    body: &Body,
    operand: &Operand,
    span: HirSourceSpan,
    operation: &str,
) -> Result<(), ReplacementExecutionError> {
    if let Some(kind) = typed_numeric_operand_kind(body, operand, span)? {
        Err(typed_numeric_operation_refusal(kind, operation, span))
    } else {
        Ok(())
    }
}

/// Build the canonical #988-owned refusal for one unproved typed-numeric operation.
fn typed_numeric_operation_refusal(
    kind: TypedNumericProfileKind,
    operation: impl std::fmt::Display,
    span: HirSourceSpan,
) -> ReplacementExecutionError {
    unsupported(
        format!(
            "typed numeric `{}` {operation} is outside the admitted carrier profile (owned by #988)",
            kind.label()
        ),
        span,
    )
}

/// Walk a pattern and refuse any typed-numeric literal before match execution can produce effects.
fn validate_typed_numeric_pattern(pattern: &Pattern, span: HirSourceSpan) -> Result<(), ReplacementExecutionError> {
    match pattern {
        Pattern::Literal(Constant::TypedNumeric(constant)) => {
            let value = typed_numeric_constant_value(constant, span)?;
            let kind = match value {
                ReplacementNumericValue::Signed { kind, .. } | ReplacementNumericValue::Unsigned { kind, .. } => {
                    TypedNumericProfileKind::Binary(kind)
                }
                ReplacementNumericValue::F32(_) => TypedNumericProfileKind::Binary(NumericTypeId::F32),
                ReplacementNumericValue::F64(_) => TypedNumericProfileKind::Binary(NumericTypeId::F64),
                ReplacementNumericValue::Decimal { .. } => TypedNumericProfileKind::Decimal,
            };
            Err(typed_numeric_operation_refusal(kind, "pattern matching", span))
        }
        Pattern::Tuple(items) | Pattern::Or(items) => {
            for item in items {
                validate_typed_numeric_pattern(item, span)?;
            }
            Ok(())
        }
        Pattern::Struct { fields, .. } | Pattern::Nominal { fields, .. } => {
            for (_, field) in fields {
                validate_typed_numeric_pattern(field, span)?;
            }
            Ok(())
        }
        Pattern::Result { fields, .. } | Pattern::Enum { fields, .. } => {
            for field in fields {
                validate_typed_numeric_pattern(field, span)?;
            }
            Ok(())
        }
        Pattern::Wildcard | Pattern::Var(_) | Pattern::Literal(_) | Pattern::FieldlessEnumVariant(_) => Ok(()),
    }
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
/// declaration type before execution: tuple and list aggregates and their loop items may be recursively structural
/// values. Range and canonical Zip iterators have their own checked item contracts.
fn validate_collection_local_types(
    body: &Body,
    statements: &[Statement],
    range_iterator_locals: &BTreeSet<LocalId>,
    zip_iterator_locals: &BTreeSet<LocalId>,
) -> Result<(), ReplacementExecutionError> {
    for statement in statements {
        match &statement.kind {
            StatementKind::Assign {
                place,
                rvalue: Rvalue::Aggregate(AggregateKind::Set, _),
            } => validate_hashed_aggregate_local_type(body, place, statement.span, CollectionTypeId::Set)?,
            StatementKind::Assign {
                place,
                rvalue: Rvalue::Dict(_),
            } => {
                validate_hashed_aggregate_local_type(body, place, statement.span, CollectionTypeId::Dict)?;
            }
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
                } else if !zip_iterator_locals.contains(&iterator_local) {
                    validate_structural_iteration_local_type(
                        body,
                        bare_local(destination, statement.span)?,
                        statement.span,
                        "builtin collection iteration destination",
                    )?;
                    validate_structural_list_local_type(
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
                validate_collection_local_types(body, &then_block.stmts, range_iterator_locals, zip_iterator_locals)?;
                if let Some(else_block) = else_block {
                    validate_collection_local_types(
                        body,
                        &else_block.stmts,
                        range_iterator_locals,
                        zip_iterator_locals,
                    )?;
                }
            }
            StatementKind::Loop { body: loop_body } => {
                validate_collection_local_types(body, &loop_body.stmts, range_iterator_locals, zip_iterator_locals)?;
            }
            StatementKind::Assign {
                rvalue: Rvalue::Match { arms, .. },
                ..
            } => {
                for arm in arms {
                    validate_collection_local_types(
                        body,
                        &arm.guard_stmts,
                        range_iterator_locals,
                        zip_iterator_locals,
                    )?;
                    validate_collection_local_types(body, &arm.body_stmts, range_iterator_locals, zip_iterator_locals)?;
                }
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
        Rvalue::Match { arms, .. } => {
            for arm in arms {
                validate_structural_aggregate_types_in_statements(body, &arm.guard_stmts)?;
                validate_nested_aggregate_types_in_statements(body, &arm.guard_stmts)?;
                validate_structural_aggregate_types_in_statements(body, &arm.body_stmts)?;
                validate_nested_aggregate_types_in_statements(body, &arm.body_stmts)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// Validate only aggregate destinations in a deferred frame, preserving its own iteration profile.
///
/// Generator frames use a deliberately different `IterNext` type contract from ordinary bodies. Deferred aggregate
/// checks therefore share the same compiler-owned local type rule without accidentally applying the enclosing
/// body's structural-list iteration rule to generator-local iterator values.
fn validate_structural_aggregate_types_in_statements(
    body: &Body,
    statements: &[Statement],
) -> Result<(), ReplacementExecutionError> {
    for statement in statements {
        match &statement.kind {
            StatementKind::Assign {
                place,
                rvalue: Rvalue::Aggregate(AggregateKind::Set, _),
            } => validate_hashed_aggregate_local_type(body, place, statement.span, CollectionTypeId::Set)?,
            StatementKind::Assign {
                place,
                rvalue: Rvalue::Dict(_),
            } => {
                validate_hashed_aggregate_local_type(body, place, statement.span, CollectionTypeId::Dict)?;
            }
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
            StatementKind::Assign {
                rvalue: Rvalue::Match { arms, .. },
                ..
            } => {
                for arm in arms {
                    validate_structural_aggregate_types_in_statements(body, &arm.guard_stmts)?;
                    validate_structural_aggregate_types_in_statements(body, &arm.body_stmts)?;
                }
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

/// Validate the retained key type even when an empty hashed aggregate provides no runtime element to inspect.
fn validate_hashed_aggregate_local_type(
    body: &Body,
    place: &Place,
    span: HirSourceSpan,
    collection: CollectionTypeId,
) -> Result<(), ReplacementExecutionError> {
    let ty = declared_local_type(body, bare_local(place, span)?, span)?;
    let valid = match ty {
        IncanType::Generic { base, args } if collections::from_str(base) == Some(collection) => {
            match (collection, args.as_slice()) {
                (CollectionTypeId::Set, [element]) | (CollectionTypeId::Dict, [element, _]) => {
                    is_collection_scalar_type(element)
                }
                _ => false,
            }
        }
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(unsupported(
            format!("hashed aggregate has unsupported key type in `{ty}`"),
            span,
        ))
    }
}

/// Collect builtin `range` iterator locals from explicit Body-IR builtin targets.
///
/// A same-module declaration named `range` now carries a [`NamedCallableTarget::direct_call_id`] and dispatches to
/// that declaration, while the compiler-recognized builtin carries [`BuiltinFnId::Range`]. This keeps the
/// existing bounded builtin rule without guessing from a source spelling or treating an imported callable as range.
fn range_iterator_locals(block: &incan_semantics_core::body_ir::Block) -> BTreeSet<LocalId> {
    let mut locals = BTreeSet::new();
    while collect_range_iterator_locals(&block.stmts, &mut locals) {}
    locals
}

/// Extend the admitted `range` source-spelling aliases until the enclosing body reaches a fixed point.
fn collect_range_iterator_locals(statements: &[Statement], range_locals: &mut BTreeSet<LocalId>) -> bool {
    let mut changed = false;
    for statement in statements {
        match &statement.kind {
            StatementKind::Call {
                destination: Some(destination),
                callee: Callee::Function(CallableTarget::Named(target)),
                ..
            } if is_explicit_range_builtin(target) && destination.projection.is_empty() => {
                if let Some(local) = destination.local_id() {
                    changed |= range_locals.insert(local);
                }
            }
            StatementKind::Assign {
                place,
                rvalue: Rvalue::Use(Operand::Place(source)),
            } if place.projection.is_empty()
                && source.place.projection.is_empty()
                && source
                    .place
                    .local_id()
                    .is_some_and(|local| range_locals.contains(&local)) =>
            {
                if let Some(local) = place.local_id() {
                    changed |= range_locals.insert(local);
                }
            }
            StatementKind::If {
                then_block, else_block, ..
            } => {
                changed |= collect_range_iterator_locals(&then_block.stmts, range_locals);
                if let Some(else_block) = else_block {
                    changed |= collect_range_iterator_locals(&else_block.stmts, range_locals);
                }
            }
            StatementKind::Loop { body } => {
                changed |= collect_range_iterator_locals(&body.stmts, range_locals);
            }
            StatementKind::Assign {
                rvalue: Rvalue::Match { arms, .. },
                ..
            } => {
                for arm in arms {
                    changed |= collect_range_iterator_locals(&arm.guard_stmts, range_locals);
                    changed |= collect_range_iterator_locals(&arm.body_stmts, range_locals);
                }
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

/// Validate that a builtin collection loop binds an item this runtime can hold.
///
/// The bound item is whatever [`BodyExecutor::poll_iterator`] hands back — an element cloned out of the list — so
/// the question is only whether that value is representable, which is the same question
/// [`validate_structural_aggregate_local_type`] asks of a list literal. Iterating `list[int]` was previously refused
/// here for want of a two-element tuple, even though execution never depended on the shape: the poll clones the
/// element at the cursor and assigns it, whatever it is. Ranges already iterated scalars through their own gate, so
/// the narrower rule made the most ordinary loop in the language the one shape that could not run.
fn validate_structural_iteration_local_type(
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

/// Validate that a Body-IR local has the only list type admitted by the replacement collection profile.
fn validate_structural_list_local_type(
    body: &Body,
    local: LocalId,
    span: HirSourceSpan,
    role: &str,
) -> Result<(), ReplacementExecutionError> {
    let ty = declared_local_type(body, local, span)?;
    if is_structural_list_type(ty) {
        Ok(())
    } else {
        Err(unsupported(
            format!("{role} has Body-IR type `{ty}`, not a list of representable elements"),
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

/// Render one interpolated value the way the Rust-emission backend's `{}` / `{:?}` would.
///
/// Restricted to the scalars whose two renderings provably agree. `Debug` on a string quotes it, matching Rust's
/// `{:?}`; every other supported scalar renders identically under both styles. Anything outside this set refuses,
/// because an interpolation that renders differently per backend is exactly the silent divergence the parity
/// corpus exists to catch, and it would be invisible in the value itself.
fn format_interpolation(
    value: &ReplacementValue,
    style: FormatStyle,
    span: HirSourceSpan,
) -> Result<String, ReplacementExecutionError> {
    match (value, style) {
        (ReplacementValue::Int(value), _) => Ok(value.to_string()),
        (ReplacementValue::Bool(value), _) => Ok(value.to_string()),
        (ReplacementValue::Str(text), FormatStyle::Display) => Ok(text.clone()),
        (ReplacementValue::Str(text), FormatStyle::Debug) => Ok(format!("{text:?}")),
        (ReplacementValue::Float(value), FormatStyle::Display) => Ok(value.to_string()),
        (ReplacementValue::Numeric(value), FormatStyle::Display) => Ok(value.observable_text()),
        (other, _) => Err(unsupported(
            format!("f-string interpolation of {}", value_kind(other)),
            span,
        )),
    }
}

/// Serialize one admitted scalar with the same `serde_json` implementation as generated native code.
///
/// The direct profile intentionally stops at `int`, `bool`, `str`, and `None`/unit. Structural values may be
/// serializable on the native route, but admitting them here requires separate type, ordering, and failure-parity
/// evidence; they remain an original-call-span refusal instead of acquiring a second serializer policy.
fn stringify_json_scalar(
    value: ReplacementValue,
    span: HirSourceSpan,
) -> Result<ReplacementValue, ReplacementExecutionError> {
    let serialized = match value {
        ReplacementValue::Int(value) => serde_json::to_string(&value),
        ReplacementValue::Bool(value) => serde_json::to_string(&value),
        ReplacementValue::Str(value) => serde_json::to_string(&value),
        ReplacementValue::Unit => serde_json::to_string(&()),
        other => return Err(unsupported(format!("`json_stringify` of {}", value_kind(&other)), span)),
    }
    .map_err(|error| runtime_failure(format!("`json_stringify` serialization failed: {error}"), span))?;
    Ok(ReplacementValue::Str(serialized))
}

/// The integer elements of a list-shaped value, with booleans counted as 1/0.
///
/// `sum`, `min` and `max` all emit `iter()`-based Rust over a list, and the emitted `sum` maps `bool` to `1i64`/
/// `0i64` explicitly. Mirroring that here keeps the two backends' answers identical for the same source rather than
/// leaving this runtime to invent a numeric interpretation of its own.
fn integer_elements(
    value: &ReplacementValue,
    builtin: &str,
    span: HirSourceSpan,
) -> Result<Vec<i64>, ReplacementExecutionError> {
    let elements = match value {
        ReplacementValue::List { elements, .. } | ReplacementValue::CollectedGenerator { elements, .. } => elements,
        other => {
            return Err(unsupported(format!("`{builtin}` of {}", value_kind(other)), span));
        }
    };
    elements
        .iter()
        .map(|element| match element {
            ReplacementValue::Int(value) => Ok(*value),
            ReplacementValue::Bool(value) => Ok(i64::from(*value)),
            other => Err(unsupported(format!("`{builtin}` over {}", value_kind(other)), span)),
        })
        .collect()
}

/// Return whether a type is a list whose elements this runtime can materialize, per the collection registry.
fn is_structural_list_type(ty: &IncanType) -> bool {
    matches!(
        ty,
        IncanType::Generic { base, args }
            if collections::from_str(base) == Some(CollectionTypeId::List)
                && matches!(args.as_slice(), [element] if is_direct_structural_type(element))
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
    explicit_builtin(target) == Some(BuiltinFnId::Range)
}

/// The compiler-owned builtin this target names, if it names one and source did not take the spelling.
///
/// A same-module declaration carries a [`NamedCallableTarget::direct_call_id`] and dispatches to itself, so a
/// module defining its own `range` or `len` keeps meaning its own. One accessor rather than a predicate per
/// builtin, so admission and execution read the same answer instead of drifting apart as the set grows.
fn explicit_builtin(target: &NamedCallableTarget) -> Option<BuiltinFnId> {
    let builtin = target.direct_call_id.is_none().then_some(target.builtin).flatten()?;
    let canonical = target.canonical.as_ref()?;
    (canonical.namespace == SymbolNamespace::OrdinaryLexical
        && canonical.origin == SymbolOrigin::Builtin
        && canonical.kind == SemanticSourceTargetKind::Builtin
        && canonical.scope_discriminant.is_none()
        && canonical.declaration_span == HirSourceSpan::new(0, 0)
        && canonical.declaration_name == builtins::as_str(builtin))
    .then_some(builtin)
}

/// Closed set of source method operations admitted by this replacement profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReplacementMethodOperation {
    GeneratorCollect,
    GeneratorMap,
    GeneratorFilter,
    ValueEnumValue,
}

/// Resolve an admitted method solely from the canonical target retained by typechecking.
///
/// `MethodTarget::name` is intentionally not consulted. It is a source/display spelling and may be an alias; only
/// the canonical identity can authorize a runtime operation.
fn replacement_method_operation(
    target: &incan_semantics_core::body_ir::MethodTarget,
) -> Option<ReplacementMethodOperation> {
    let identity = target.canonical.as_ref()?;
    if identity.namespace != SymbolNamespace::Member
        || identity.kind != SemanticSourceTargetKind::Method
        || identity.scope_discriminant.is_some()
    {
        return None;
    }
    if identity.origin == SymbolOrigin::Builtin && identity.declaration_span == HirSourceSpan::new(0, 0) {
        let member = identity.declaration_name.strip_prefix("Generator.")?;
        return match iterator_methods::from_str(member)? {
            IteratorMethodId::Collect => Some(ReplacementMethodOperation::GeneratorCollect),
            IteratorMethodId::Map => Some(ReplacementMethodOperation::GeneratorMap),
            IteratorMethodId::Filter => Some(ReplacementMethodOperation::GeneratorFilter),
            _ => None,
        };
    }
    (matches!(identity.origin, SymbolOrigin::Module(_)) && identity.declaration_name == "value")
        .then_some(ReplacementMethodOperation::ValueEnumValue)
}

/// The builtins this runtime executes, as opposed to those it merely recognizes.
///
/// Deliberately a subset. A builtin belongs here only when this runtime's answer provably matches the one the
/// Rust-emission backend generates for the same call; anything else refuses by name rather than producing a second
/// opinion. String `len` is admitted because both routes share the canonical Unicode-scalar helper.
const EXECUTABLE_BUILTINS: &[BuiltinFnId] = &[
    BuiltinFnId::Print,
    BuiltinFnId::Bool,
    BuiltinFnId::Str,
    BuiltinFnId::Int,
    BuiltinFnId::Float,
    BuiltinFnId::Len,
    BuiltinFnId::Abs,
    BuiltinFnId::Sum,
    BuiltinFnId::Min,
    BuiltinFnId::Max,
    BuiltinFnId::Sorted,
    BuiltinFnId::Enumerate,
    BuiltinFnId::Zip,
    BuiltinFnId::JsonStringify,
];

/// Collect the local identities written by builtin collection polling across one normalized body.
///
/// Only these compiler-created item locals may later be projected as a selected scalar tuple element. This keeps a
/// standalone tuple field access outside the profile even though it uses the same `PlaceElem::Field` representation.
fn builtin_iteration_destinations(block: &incan_semantics_core::body_ir::Block) -> BTreeSet<LocalId> {
    let mut destinations = BTreeSet::new();
    collect_builtin_iteration_destinations(&block.stmts, &mut destinations);
    destinations
}

/// Recurse through normalized control flow to collect every builtin iteration destination local.
fn collect_builtin_iteration_destinations(statements: &[Statement], destinations: &mut BTreeSet<LocalId>) {
    for statement in statements {
        match &statement.kind {
            StatementKind::If {
                then_block, else_block, ..
            } => {
                collect_builtin_iteration_destinations(&then_block.stmts, destinations);
                if let Some(else_block) = else_block {
                    collect_builtin_iteration_destinations(&else_block.stmts, destinations);
                }
            }
            StatementKind::Loop { body } => collect_builtin_iteration_destinations(&body.stmts, destinations),
            StatementKind::Assign {
                rvalue: Rvalue::Match { arms, .. },
                ..
            } => {
                for arm in arms {
                    collect_builtin_iteration_destinations(&arm.guard_stmts, destinations);
                    collect_builtin_iteration_destinations(&arm.body_stmts, destinations);
                }
            }
            StatementKind::IterNext {
                destination,
                protocol: IterProtocol::Builtin,
                ..
            } => {
                if let Some(local) = destination.local_id() {
                    destinations.insert(local);
                }
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
    collect_scalar_tuple_collection_locals(&block.stmts, &mut tuple_destinations, &mut list_operands);
    tuple_destinations.intersection(&list_operands).copied().collect()
}

/// Recurse through control flow to collect tuple assignments and direct list-aggregate operands.
fn collect_scalar_tuple_collection_locals(
    statements: &[Statement],
    tuple_destinations: &mut BTreeSet<LocalId>,
    list_operands: &mut BTreeSet<LocalId>,
) {
    for statement in statements {
        match &statement.kind {
            StatementKind::Assign {
                place,
                rvalue: Rvalue::Aggregate(AggregateKind::Tuple, _),
            } if place.projection.is_empty() => {
                if let Some(local) = place.local_id() {
                    tuple_destinations.insert(local);
                }
            }
            StatementKind::Assign {
                rvalue: Rvalue::Aggregate(AggregateKind::List, operands),
                ..
            } => {
                // A spread element is skipped rather than refused here: this only collects candidate locals, and
                // `validate_collection_local_types` has already refused any spread-bearing list aggregate.
                for operand in operands.iter().filter_map(ArgumentElement::as_one) {
                    if let Operand::Place(place_operand) = operand
                        && place_operand.place.projection.is_empty()
                        && let Some(local) = place_operand.place.local_id()
                    {
                        list_operands.insert(local);
                    }
                }
            }
            StatementKind::If {
                then_block, else_block, ..
            } => {
                collect_scalar_tuple_collection_locals(&then_block.stmts, tuple_destinations, list_operands);
                if let Some(else_block) = else_block {
                    collect_scalar_tuple_collection_locals(&else_block.stmts, tuple_destinations, list_operands);
                }
            }
            StatementKind::Loop { body } => {
                collect_scalar_tuple_collection_locals(&body.stmts, tuple_destinations, list_operands);
            }
            StatementKind::Assign {
                rvalue: Rvalue::Match { arms, .. },
                ..
            } => {
                for arm in arms {
                    collect_scalar_tuple_collection_locals(&arm.guard_stmts, tuple_destinations, list_operands);
                    collect_scalar_tuple_collection_locals(&arm.body_stmts, tuple_destinations, list_operands);
                }
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

/// Validate one async block, delegating every non-suspension statement to the existing direct profile.
fn validate_async_block_profile(
    block: &incan_semantics_core::body_ir::Block,
    tuple_iteration_locals: &BTreeSet<LocalId>,
    scalar_tuple_collection_locals: &BTreeSet<LocalId>,
) -> Result<(), ReplacementExecutionError> {
    for statement in &block.stmts {
        match &statement.kind {
            StatementKind::Await { destination, awaited } => {
                let destination = destination
                    .as_ref()
                    .ok_or_else(|| unsupported("await without a destination", statement.span))?;
                validate_bare_local(destination, statement.span)?;
                validate_operand_profile(awaited, statement.span, tuple_iteration_locals)?;
            }
            StatementKind::Race { destination, arms } => {
                let destination = destination
                    .as_ref()
                    .ok_or_else(|| unsupported("race without a destination", statement.span))?;
                validate_bare_local(destination, statement.span)?;
                if arms.is_empty() {
                    return Err(unsupported("race without an arm", statement.span));
                }
                for arm in arms {
                    validate_operand_profile(&arm.awaitable, statement.span, tuple_iteration_locals)?;
                    validate_async_block_profile(&arm.body, tuple_iteration_locals, scalar_tuple_collection_locals)?;
                    validate_operand_profile(&arm.result, statement.span, tuple_iteration_locals)?;
                }
            }
            StatementKind::If {
                cond,
                then_block,
                else_block,
            } => {
                validate_operand_profile(cond, statement.span, tuple_iteration_locals)?;
                validate_async_block_profile(then_block, tuple_iteration_locals, scalar_tuple_collection_locals)?;
                if let Some(else_block) = else_block {
                    validate_async_block_profile(else_block, tuple_iteration_locals, scalar_tuple_collection_locals)?;
                }
            }
            StatementKind::Loop { body } => {
                validate_async_block_profile(body, tuple_iteration_locals, scalar_tuple_collection_locals)?
            }
            _ => validate_statement_profile(statement, tuple_iteration_locals, scalar_tuple_collection_locals)?,
        }
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
                place.projection.is_empty().then(|| place.local_id()).flatten(),
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
        // #1167 gave Body IR the pattern and `raises` assertion forms. Executing them -- match-and-bind on the
        // panicking path, and catching a raised runtime error -- stays bounded by #1154's value-state work, so
        // they refuse by name at the original source span rather than leaving the executor unable to compile
        // against the representation, the same treatment `Await`/`Race` get above.
        StatementKind::Assert {
            kind: AssertionKind::Condition { cond },
            message,
            may_panic: _,
        } => {
            validate_operand_profile(cond, statement.span, tuple_iteration_locals)?;
            message.as_ref().map_or(Ok(()), |message| {
                validate_operand_profile(message, statement.span, tuple_iteration_locals)
            })
        }
        StatementKind::Assert {
            kind: AssertionKind::Pattern { .. },
            ..
        } => Err(unsupported("pattern assertion", statement.span)),
        StatementKind::Assert {
            kind: AssertionKind::Raises { .. },
            ..
        } => Err(unsupported("raises assertion", statement.span)),
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
        StatementKind::TryPropagate {
            destination,
            operand,
            error_routing,
        } => {
            validate_bare_local(destination, statement.span)?;
            validate_operand_profile(operand, statement.span, tuple_iteration_locals)?;
            match error_routing {
                TryErrorRouting::SameType { error_type } if is_direct_result_payload_type(error_type) => Ok(()),
                TryErrorRouting::SameType { .. } => Err(unsupported(
                    "try propagation with an unsupported Result error payload",
                    statement.span,
                )),
                TryErrorRouting::ConversionRequired { .. } => {
                    Err(unsupported("cross-error-type try propagation", statement.span))
                }
                TryErrorRouting::Unresolved => Err(unsupported(
                    "try propagation without a resolved Result error route",
                    statement.span,
                )),
            }
        }
        StatementKind::IterNext { .. } => Err(unsupported("non-range iteration", statement.span)),
        StatementKind::Unsupported { description } => Err(unsupported(description, statement.span)),
    }
}

/// Validate one Body-IR call before the executor dispatches it.
fn validate_call_profile(
    callee: &Callee,
    args: &[ArgumentElement],
    span: HirSourceSpan,
    tuple_iteration_locals: &BTreeSet<LocalId>,
) -> Result<(), ReplacementExecutionError> {
    // Representation is #1159's; executing a spliced argument belongs to the execution owner. Refuse by name at
    // the original source span rather than counting the spread as one value.
    let Some(args) = fixed_operands(args) else {
        return Err(unsupported(
            format!("call to {} with a spread argument", callee_label(callee)),
            span,
        ));
    };
    let supported = match callee {
        Callee::Helper(HelperOp::StrUpper | HelperOp::StrLower | HelperOp::StrStrip | HelperOp::StrLen) => {
            args.len() == 1
        }
        Callee::Helper(HelperOp::StrReplace) => args.len() == 3,
        Callee::Helper(
            HelperOp::StrJoin
            | HelperOp::StrContains
            | HelperOp::StrEq
            | HelperOp::StrNe
            | HelperOp::StrLt
            | HelperOp::StrLe
            | HelperOp::StrGt
            | HelperOp::StrGe,
        ) => args.len() == 2,
        Callee::Helper(HelperOp::StrSplit) => matches!(args.len(), 1 | 2),
        Callee::Helper(
            HelperOp::StrConcat
            | HelperOp::ListConcat
            | HelperOp::ListContains
            | HelperOp::ListNotContains
            | HelperOp::SetContains
            | HelperOp::SetNotContains
            | HelperOp::DictContainsKey
            | HelperOp::DictNotContainsKey,
        ) => true,
        // Named calls remain direct module dispatches. Their target/binding facts are Body-IR values, not a source
        // lookup reconstructed by this executor.
        Callee::Function(CallableTarget::Named(target)) if is_explicit_range_builtin(target) => true,
        Callee::Function(CallableTarget::Named(target))
            if explicit_builtin(target).is_some_and(|id| EXECUTABLE_BUILTINS.contains(&id)) =>
        {
            true
        }
        Callee::Function(CallableTarget::Named(target)) => {
            // `direct_call_id` is a span identity that only exists for a same-module declaration, so requiring it made
            // locality the admission criterion. #1260 executes calls that leave the entry module, where the frontend
            // resolves the callee to a canonical identity instead. Resolution is the property this gate actually
            // needs: an unresolved call carries neither. Whether the execution graph holds that identity's body is
            // decided at dispatch, which reports its own error rather than being silently admitted here.
            (target.direct_call_id.is_some() || target.canonical.is_some())
                && target.builtin.is_none()
                && validate_argument_binding_profile(&target.binding)
        }
        Callee::Function(CallableTarget::Local(target)) => {
            validate_operand_profile(&Operand::Place(target.operand.clone()), span, tuple_iteration_locals)?;
            validate_argument_binding_profile(&target.binding)
        }
        Callee::Method(target)
            if replacement_method_operation(target) == Some(ReplacementMethodOperation::GeneratorCollect) =>
        {
            args.len() == 1
        }
        // The generated RFC 032 `.value()` surface is admitted only when its receiver becomes an identity-validated
        // value-enum runtime carrier. Explicit type arguments and ordinary arguments have no retained source fact.
        Callee::Method(target)
            if replacement_method_operation(target) == Some(ReplacementMethodOperation::ValueEnumValue) =>
        {
            target.type_args.is_empty() && args.len() == 1
        }
        // The compiler currently records the iterator-adapter receiver and callback in source order but leaves
        // their stdlib method signature as `UnresolvedPositional`. That is sufficient for this deliberately
        // positional two-argument profile: neither adapter has named arguments or callable defaults to bind.
        Callee::Method(target)
            if matches!(
                replacement_method_operation(target),
                Some(ReplacementMethodOperation::GeneratorMap | ReplacementMethodOperation::GeneratorFilter)
            ) =>
        {
            args.len() == 2
                && match &target.binding {
                    ArgumentBinding::UnresolvedPositional => true,
                    binding @ ArgumentBinding::Resolved { .. } => validate_argument_binding_profile(binding),
                }
        }
        // An admitted provider operation is executable when its plan is: an active provider, an authority that
        // really names a capability, and one described input per evaluated argument. Whether *this run* has a host
        // for it is a different question, answered by `execution_preflight` before execution starts.
        Callee::ProviderOperation(plan) => {
            if let Some(description) = unexecutable_provider_plan(plan, args.len()) {
                return Err(unsupported(description, span));
            }
            true
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

/// Why one provider-operation plan cannot be executed at all, or `None` when it can.
///
/// Every rule here is checked against the plan's own facts, never against a provider name, a call-site spelling, or
/// an emitted Rust name, so the same answer holds for a local call, an import, an alias, and a re-export of one
/// operation. Lowering already refuses an inactive provider and a non-capability authority, so reaching either here
/// means a plan arrived from somewhere that skipped that gate — which is exactly when a runtime must be
/// fail-closed rather than trusting its input.
fn unexecutable_provider_plan(plan: &ProviderOperationPlan, argument_count: usize) -> Option<String> {
    let declared = &plan.operation.declaration_name;
    if plan.provider.state != ProviderActivationState::Active {
        return Some(format!(
            "provider operation `{declared}` whose provider is {} in this compilation",
            plan.provider.state.as_str()
        ));
    }
    if plan.required_capability.kind != SemanticSourceTargetKind::Capability {
        return Some(format!(
            "provider operation `{declared}` whose required authority does not name a capability declaration"
        ));
    }
    // The plan claims to describe the values execution will actually see. One input per evaluated argument, each at
    // a distinct written position, is what makes that claim checkable before a host is handed anything.
    if plan.inputs.len() != argument_count {
        return Some(format!(
            "provider operation `{declared}` whose plan describes {} inputs for {argument_count} evaluated arguments",
            plan.inputs.len()
        ));
    }
    let mut written_positions = BTreeSet::new();
    if !plan
        .inputs
        .iter()
        .all(|input| input.written_position < argument_count && written_positions.insert(input.written_position))
    {
        return Some(format!(
            "provider operation `{declared}` whose plan does not describe each evaluated argument exactly once"
        ));
    }
    None
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
        Rvalue::Format(parts) => parts.iter().try_for_each(|part| match part {
            FormatPart::Literal(_) => Ok(()),
            FormatPart::Expr { operand, .. } => validate_operand_profile(operand, span, tuple_iteration_locals),
        }),
        Rvalue::BinaryOp(_, left, right) => {
            validate_operand_profile(left, span, tuple_iteration_locals)?;
            validate_operand_profile(right, span, tuple_iteration_locals)
        }
        Rvalue::IsInstance {
            value,
            value_ty,
            target,
        } => {
            validate_operand_profile(value, span, tuple_iteration_locals)?;
            validate_isinstance_target_profile(target, span)?;
            validate_isinstance_value_type_profile(value_ty, span)
        }
        Rvalue::Dict(entries) => {
            for entry in entries {
                let DictEntry::Pair(key, value) = entry else {
                    return Err(unsupported("dict aggregate with a spread entry", span));
                };
                validate_operand_profile(key, span, tuple_iteration_locals)?;
                validate_operand_profile(value, span, tuple_iteration_locals)?;
            }
            Ok(())
        }
        Rvalue::Aggregate(kind, operands) => validate_aggregate_profile(
            kind,
            operands,
            span,
            tuple_iteration_locals,
            scalar_tuple_collection_locals,
            destination,
        ),
        Rvalue::FieldlessEnumVariant(target) => validate_fieldless_enum_variant_target(target, span),
        Rvalue::ValueEnumVariant(target) => validate_value_enum_variant_target(target, span),
        Rvalue::ResultVariant(variant) => validate_result_variant_profile(variant, span, tuple_iteration_locals),
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
        Rvalue::Match { scrutinee, arms } => {
            validate_operand_profile(scrutinee, span, tuple_iteration_locals)?;
            validate_match_arms_profile(arms, span, tuple_iteration_locals, scalar_tuple_collection_locals)
        }
    }
}

/// Validate that a checked `isinstance` value can only carry the four scalar tags proven by this bounded profile.
fn validate_isinstance_value_type_profile(
    value_ty: &IncanType,
    call_span: HirSourceSpan,
) -> Result<(), ReplacementExecutionError> {
    let admitted_scalar = |ty: &IncanType| {
        matches!(
            ty,
            IncanType::Primitive(
                IncanPrimitiveType::Int
                    | IncanPrimitiveType::Bool
                    | IncanPrimitiveType::Str
                    | IncanPrimitiveType::Float
            )
        )
    };
    let admitted = admitted_scalar(value_ty)
        || matches!(
            value_ty,
            IncanType::Generic { base, args }
                if base == incan_core::lang::types::UNION_TYPE_NAME
                    && !args.is_empty()
                    && args.iter().all(admitted_scalar)
        );
    if admitted {
        Ok(())
    } else {
        Err(unsupported(
            format!("isinstance value type `{value_ty}` outside the primitive replacement profile"),
            call_span,
        ))
    }
}

/// Validate the bounded primitive `isinstance` target set before any program effect can run.
fn validate_isinstance_target_profile(
    target: &incan_semantics_core::body_ir::IsInstanceTarget,
    call_span: HirSourceSpan,
) -> Result<(), ReplacementExecutionError> {
    if target.span.start >= target.span.end || target.span.start < call_span.start || target.span.end > call_span.end {
        return Err(unsupported(
            "isinstance target with an invalid retained source span",
            call_span,
        ));
    }
    if target.canonical.is_some() {
        return Err(unsupported(
            "isinstance target with a declaration identity outside the primitive replacement profile",
            target.span,
        ));
    }
    match &target.ty {
        IncanType::Primitive(
            IncanPrimitiveType::Int | IncanPrimitiveType::Bool | IncanPrimitiveType::Str | IncanPrimitiveType::Float,
        ) => Ok(()),
        unsupported_target => Err(unsupported(
            format!("isinstance target type `{unsupported_target}` outside the primitive replacement profile"),
            target.span,
        )),
    }
}

/// Validate the intrinsic Result constructor facts retained by Body IR.
fn validate_result_variant_profile(
    variant: &ResultVariant,
    span: HirSourceSpan,
    tuple_iteration_locals: &BTreeSet<LocalId>,
) -> Result<(), ReplacementExecutionError> {
    if !is_direct_result_payload_type(&variant.ok_type) || !is_direct_result_payload_type(&variant.error_type) {
        return Err(unsupported(
            "Result construction with an unsupported payload type",
            span,
        ));
    }
    validate_operand_profile(&variant.payload, span, tuple_iteration_locals)
}

/// Accept only data-only Result payload types that this executor can carry without recovering source behavior.
fn is_direct_result_payload_type(ty: &IncanType) -> bool {
    match ty {
        IncanType::Primitive(
            IncanPrimitiveType::Int | IncanPrimitiveType::Bool | IncanPrimitiveType::Str | IncanPrimitiveType::Unit,
        )
        | IncanType::Named(_) => true,
        IncanType::Tuple(elements) => elements.iter().all(is_direct_result_payload_type),
        IncanType::Generic { base, args } => match collections::from_str(base) {
            Some(CollectionTypeId::Tuple) => args.iter().all(is_direct_result_payload_type),
            Some(CollectionTypeId::List) => args.len() == 1 && is_direct_result_payload_type(&args[0]),
            _ => false,
        },
        _ => false,
    }
}

/// Validate all selected arm facts before a direct match executes any source statement.
fn validate_match_arms_profile(
    arms: &[MatchArm],
    span: HirSourceSpan,
    tuple_iteration_locals: &BTreeSet<LocalId>,
    scalar_tuple_collection_locals: &BTreeSet<LocalId>,
) -> Result<(), ReplacementExecutionError> {
    if arms.is_empty() {
        return Err(unsupported("match expression without arms", span));
    }
    for arm in arms {
        validate_pattern_profile(&arm.pattern, span)?;
        for statement in &arm.guard_stmts {
            validate_statement_profile(statement, tuple_iteration_locals, scalar_tuple_collection_locals)?;
        }
        if let Some(guard) = &arm.guard {
            validate_operand_profile(guard, span, tuple_iteration_locals)?;
        }
        for statement in &arm.body_stmts {
            validate_statement_profile(statement, tuple_iteration_locals, scalar_tuple_collection_locals)?;
        }
        validate_operand_profile(&arm.result, span, tuple_iteration_locals)?;
    }
    Ok(())
}

/// Validate only the exact retained pattern vocabulary this direct runtime implements.
fn validate_pattern_profile(pattern: &Pattern, span: HirSourceSpan) -> Result<(), ReplacementExecutionError> {
    match pattern {
        Pattern::Wildcard | Pattern::Literal(_) => Ok(()),
        Pattern::Var(binding) => (!matches!(binding.fact, OwnershipFact::Move | OwnershipFact::Unknown))
            .then_some(())
            .ok_or_else(|| unsupported("match binding with unsupported move or unknown ownership", span)),
        Pattern::Tuple(items) | Pattern::Or(items) => {
            for item in items {
                validate_pattern_profile(item, span)?;
            }
            Ok(())
        }
        Pattern::Nominal { target, fields } => {
            validate_nominal_pattern_target(target, span)?;
            for (_, field_pattern) in fields {
                validate_pattern_profile(field_pattern, span)?;
            }
            Ok(())
        }
        Pattern::FieldlessEnumVariant(target) => validate_fieldless_enum_variant_target(target, span),
        Pattern::Result { fields, .. } => {
            if fields.len() != 1 {
                return Err(unsupported("Result pattern without one payload", span));
            }
            validate_pattern_profile(&fields[0], span)
        }
        Pattern::Struct { canonical: None, .. } | Pattern::Enum { canonical: None, .. } => Err(unsupported(
            "match pattern without an exact direct target identity",
            span,
        )),
        Pattern::Struct { canonical: Some(_), .. } | Pattern::Enum { canonical: Some(_), .. } => Err(unsupported(
            "match pattern without an admitted direct target layout",
            span,
        )),
    }
}

/// Reject a nominal pattern whose retained identity is not a canonical module-owned model target.
fn validate_nominal_pattern_target(
    target: &NominalPatternTarget,
    span: HirSourceSpan,
) -> Result<(), ReplacementExecutionError> {
    if target.canonical.namespace != SymbolNamespace::OrdinaryLexical
        || target.canonical.kind != SemanticSourceTargetKind::Model
        || !matches!(&target.canonical.origin, SymbolOrigin::Module(_))
        || target.canonical.scope_discriminant.is_some()
    {
        return Err(unsupported(
            "nominal match pattern without a canonical source-local model target",
            span,
        ));
    }
    Ok(())
}

/// Reject a malformed fieldless-enum rvalue before execution can attempt declaration-name recovery.
///
/// Runtime resolution remains responsible for module-local identity and membership checks because only the retained
/// Body-IR registry owns those facts. This preflight rejects an incomplete target at the original source span.
fn validate_fieldless_enum_variant_target(
    target: &FieldlessEnumVariantTarget,
    span: HirSourceSpan,
) -> Result<(), ReplacementExecutionError> {
    if target.enum_name.is_empty()
        || target.variant_name.is_empty()
        || target.enum_canonical.namespace != SymbolNamespace::OrdinaryLexical
        || target.enum_canonical.kind != SemanticSourceTargetKind::Enum
        || target.enum_canonical.scope_discriminant.is_some()
        || !matches!(target.enum_canonical.origin, SymbolOrigin::Module(_))
        || target.enum_canonical.declaration_name != target.enum_name
        || target.variant_canonical.namespace != SymbolNamespace::Member
        || target.variant_canonical.kind != SemanticSourceTargetKind::Variant
        || target.variant_canonical.scope_discriminant.is_some()
        || target.variant_canonical.origin != target.enum_canonical.origin
        || target.variant_canonical.declaration_name != target.variant_name
    {
        return Err(unsupported(
            "fieldless-enum member without exact canonical source-local owner/member identities",
            span,
        ));
    }
    Ok(())
}

/// Reject a malformed value-enum rvalue before execution can attempt declaration-name recovery.
///
/// Runtime resolution remains responsible for membership and raw-scalar checks because only the Body-IR module
/// registry contains those source-local facts. This preflight ensures that malformed targets cannot be mistaken for
/// ordinary unresolved field projections.
fn validate_value_enum_variant_target(
    target: &ValueEnumVariantTarget,
    span: HirSourceSpan,
) -> Result<(), ReplacementExecutionError> {
    if target.enum_name.is_empty()
        || target.variant_name.is_empty()
        || target.enum_canonical.namespace != SymbolNamespace::OrdinaryLexical
        || target.enum_canonical.kind != SemanticSourceTargetKind::Enum
        || target.enum_canonical.scope_discriminant.is_some()
        || !matches!(target.enum_canonical.origin, SymbolOrigin::Module(_))
        || target.enum_canonical.declaration_name != target.enum_name
        || target.variant_canonical.namespace != SymbolNamespace::Member
        || target.variant_canonical.kind != SemanticSourceTargetKind::Variant
        || target.variant_canonical.scope_discriminant.is_some()
        || target.variant_canonical.origin != target.enum_canonical.origin
        || target.variant_canonical.declaration_name != target.variant_name
    {
        return Err(unsupported(
            "value-enum member without exact canonical source-local owner/member identities",
            span,
        ));
    }
    Ok(())
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

/// Validate source-local tuple/list/set aggregates plus the constrained plain-model constructor vocabulary.
///
/// Set elements must satisfy the hashed scalar-key profile during materialization. Constructor admission requires
/// declaration identity, complete checked bindings, and structural field values before the executor materializes a
/// model value.
fn validate_aggregate_profile(
    kind: &AggregateKind,
    operands: &[ArgumentElement],
    span: HirSourceSpan,
    tuple_iteration_locals: &BTreeSet<LocalId>,
    _scalar_tuple_collection_locals: &BTreeSet<LocalId>,
    _destination: Option<LocalId>,
) -> Result<(), ReplacementExecutionError> {
    // A spread makes the element count a runtime fact, so every arity guard below would be counting the wrong
    // thing. Refuse before any of them run.
    let Some(operands) = fixed_operands(operands) else {
        return Err(unsupported(
            format!("{} aggregate with a spread element", aggregate_label(kind)),
            span,
        ));
    };
    let operands = operands.as_slice();
    match kind {
        AggregateKind::Tuple | AggregateKind::List | AggregateKind::Set => {
            for operand in operands {
                validate_operand_profile(operand, span, tuple_iteration_locals)?;
            }
            Ok(())
        }
        AggregateKind::Constructor(target) => validate_nominal_constructor_target(target, operands.len(), span),
        _ => Err(unsupported(format!("{} aggregate", aggregate_label(kind)), span)),
    }
}

/// Check that a constructor shape carries the minimal checked facts required before direct materialization.
///
/// The module registry and canonical field layout are verified at execution time, where the executor has the exact
/// `BodyIrModule`; this preflight rejects missing identity/default/binding claims before any constructor operand can
/// produce a successful replacement receipt.
fn validate_nominal_constructor_target(
    target: &ConstructorTarget,
    operand_count: usize,
    span: HirSourceSpan,
) -> Result<(), ReplacementExecutionError> {
    if target.direct_declaration_id.is_none() {
        return Err(unsupported(
            format!(
                "constructor `{}` without a source-local declaration identity",
                target.name
            ),
            span,
        ));
    }
    if target.canonical_field_layout.is_none() {
        return Err(unsupported(
            format!("constructor `{}` without a checked canonical field layout", target.name),
            span,
        ));
    }
    let Some(canonical) = target.canonical.as_ref() else {
        return Err(unsupported(
            format!("constructor `{}` without a canonical declaration target", target.name),
            span,
        ));
    };
    if canonical.namespace != SymbolNamespace::OrdinaryLexical
        || canonical.kind != SemanticSourceTargetKind::Model
        || !matches!(&canonical.origin, SymbolOrigin::Module(_))
        || canonical.scope_discriminant.is_some()
    {
        return Err(unsupported(
            "constructor canonical target is not a source-local model",
            span,
        ));
    }
    let ArgumentBinding::Resolved {
        arguments,
        defaulted_slots,
    } = &target.binding
    else {
        return Err(unsupported(
            format!("constructor `{}` with unresolved field binding", target.name),
            span,
        ));
    };
    if !defaulted_slots.is_empty() {
        return Err(unsupported(
            format!("constructor `{}` with an omitted field default", target.name),
            span,
        ));
    }
    if arguments.len() != operand_count || !validate_argument_binding_profile(&target.binding) {
        return Err(unsupported(
            format!("constructor `{}` with invalid field binding", target.name),
            span,
        ));
    }
    Ok(())
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

/// Refuse canonical module storage at profile validation before execution could mistake it for frame state.
fn validate_local_place_root(place: &Place, span: HirSourceSpan) -> Result<(), ReplacementExecutionError> {
    if place.global().is_some() {
        return bare_local(place, span).map(|_| ());
    }
    Ok(())
}

/// Admit one-level tuple/model fields and one-level indexes over source-local structural values.
fn validate_read_place(
    place: &Place,
    span: HirSourceSpan,
    tuple_iteration_locals: &BTreeSet<LocalId>,
) -> Result<(), ReplacementExecutionError> {
    validate_local_place_root(place, span)?;
    match place.projection.as_slice() {
        [] => Ok(()),
        [PlaceElem::Field { .. }] => Ok(()),
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
    validate_local_place_root(place, span)?;
    match place.projection.as_slice() {
        [] => Ok(()),
        [PlaceElem::Index(index)] => validate_operand_profile(index, span, tuple_iteration_locals),
        [PlaceElem::Field { .. }] => Err(unsupported("field assignment", span)),
        [PlaceElem::Slice { .. }] => Err(unsupported("slice assignment", span)),
        _ => Err(unsupported("nested place assignment", span)),
    }
}

/// Mutable interpreter state for one Body-IR execution.
struct BodyExecutor<'run, 'writer> {
    module: BodyIrModule,
    /// Modules other than this frame's own that a resolved call may execute against.
    ///
    /// Shared rather than cloned per frame: a nested frame inherits the same set, and the graph is immutable for the
    /// life of one execution. `module` stays owned because a frame executes against exactly one module and every
    /// existing lookup reads it directly.
    reachable: Rc<Vec<BodyIrModule>>,
    locals: BTreeMap<LocalId, ReplacementValue>,
    /// Checked local types for the currently selected declaration or its nested source-local frame.
    local_types: BTreeMap<LocalId, IncanType>,
    ownership_reads: Vec<OwnershipRead>,
    runtime_requirements: Vec<AbiV0RuntimeRequirement>,
    body_snapshots: Vec<String>,
    steps: usize,
    /// The next task-frame identity. Child executors inherit and return this counter so direct task construction
    /// stays globally ordered for one receipt-bound execution.
    next_task_id: usize,
    /// Direct task transitions observed in execution order and bound into the output identity.
    task_lifecycle: Vec<TaskLifecycleEvent>,
    /// Caller-owned streams reborrowed by nested frames; delivery and observation outlive a failed frame.
    io: &'run mut ProgramIo<'writer>,
    /// The task whose Body IR this executor is currently polling, if any.
    active_task: Option<usize>,
    /// The authority source, provider host, and receipt log admitted provider operations run against.
    ///
    /// Shared with every nested frame rather than cloned per frame: RFC 104 receipts are sequenced across one run,
    /// so two frames each holding their own log would each believe it emitted receipt `#0`. `None` means this run
    /// executes no provider operation, which the pre-execution gate has already turned into a refusal.
    providers: Option<Rc<ProviderRuntime>>,
    /// A structured match expression may execute an arm whose body returns, breaks, or continues before the
    /// enclosing assignment has a value to store. Keep that flow explicit rather than assigning a placeholder and
    /// accidentally continuing execution after source-level control flow.
    pending_flow: Option<Flow>,
}

impl<'run, 'writer> BodyExecutor<'run, 'writer> {
    /// Bind the already-typechecked call arguments to their Body-IR parameter locals.
    fn new(
        graph: &ReplacementExecutionGraph<'_>,
        body: &Body,
        args: &[ReplacementValue],
        providers: Option<Rc<ProviderRuntime>>,
        io: &'run mut ProgramIo<'writer>,
    ) -> Result<Self, ReplacementExecutionError> {
        let module = graph.primary();
        let reachable = Rc::new(
            graph
                .modules()
                .filter(|candidate| candidate.module_id != module.module_id)
                .cloned()
                .collect::<Vec<_>>(),
        );
        let mut executor = Self {
            module: module.clone(),
            reachable,
            locals: BTreeMap::new(),
            local_types: BTreeMap::new(),
            ownership_reads: Vec::new(),
            runtime_requirements: Vec::new(),
            body_snapshots: Vec::new(),
            steps: 0,
            next_task_id: 0,
            task_lifecycle: Vec::new(),
            io,
            active_task: None,
            providers,
            pending_flow: None,
        };
        executor.record_body(body);
        executor.locals = executor.bind_direct_arguments(body, args)?;
        Ok(executor)
    }

    /// Build an isolated executor for a nested callable, default computation, or suspended generator frame.
    fn with_locals(
        module: &BodyIrModule,
        reachable: Rc<Vec<BodyIrModule>>,
        locals: BTreeMap<LocalId, ReplacementValue>,
        local_types: BTreeMap<LocalId, IncanType>,
        steps: usize,
        io: &'run mut ProgramIo<'writer>,
    ) -> Self {
        Self {
            module: module.clone(),
            reachable,
            locals,
            local_types,
            ownership_reads: Vec::new(),
            runtime_requirements: Vec::new(),
            body_snapshots: Vec::new(),
            steps,
            next_task_id: 0,
            task_lifecycle: Vec::new(),
            io,
            active_task: None,
            providers: None,
            pending_flow: None,
        }
    }

    /// Record a directly consumed declaration body as evidence and preserve its runtime requirements in first-seen
    /// order.
    fn record_body(&mut self, body: &Body) {
        self.local_types = body.locals.iter().map(|local| (local.id, local.ty.clone())).collect();
        self.body_snapshots.push(body.render_snapshot());
        for requirement in &body.runtime_requirements {
            if !self.runtime_requirements.contains(requirement) {
                self.runtime_requirements.push(requirement.clone());
            }
        }
        if body.is_async
            && !self
                .runtime_requirements
                .contains(&AbiV0RuntimeRequirement::AsyncRuntime)
        {
            self.runtime_requirements.push(AbiV0RuntimeRequirement::AsyncRuntime);
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

    /// Execute an isolated frame while reborrowing the caller's streams, then merge its execution evidence.
    ///
    /// The closure cannot outlive the reborrow. This keeps one mutable stream owner without shared interior
    /// mutability, and preserves accepted output even if the child exits with an error before returning a value.
    fn execute_child<T>(
        &mut self,
        locals: BTreeMap<LocalId, ReplacementValue>,
        steps: usize,
        execute: impl FnOnce(&mut BodyExecutor<'_, 'writer>) -> Result<T, ReplacementExecutionError>,
    ) -> Result<T, ReplacementExecutionError> {
        self.execute_child_with_local_types(locals, self.local_types.clone(), steps, execute)
    }

    /// Execute an isolated frame against the local-type table owned by that frame's declaration.
    ///
    /// The frame runs in this executor's own module, which is correct for every same-module call in the #988 profile.
    /// A frame whose callee was resolved to another module goes through [`Self::execute_child_in_module`] instead.
    fn execute_child_with_local_types<T>(
        &mut self,
        locals: BTreeMap<LocalId, ReplacementValue>,
        local_types: BTreeMap<LocalId, IncanType>,
        steps: usize,
        execute: impl FnOnce(&mut BodyExecutor<'_, 'writer>) -> Result<T, ReplacementExecutionError>,
    ) -> Result<T, ReplacementExecutionError> {
        let module = self.module.clone();
        self.execute_child_in_module(&module, locals, local_types, steps, execute)
    }

    /// Execute an isolated frame that is owned by `module` rather than by this executor's own module.
    ///
    /// Every nested frame -- callable, generator, task, default computation, and adapter -- reaches its executor
    /// through here, so this is the one place a cross-module call changes what a child frame executes against. The
    /// caller passes the module the callee was *resolved* to, never the module the call was written in: a frame that
    /// kept the caller's module would resolve an imported body against the wrong declaration table, which is the
    /// failure `a_cross_module_call_is_refused_by_the_single_module_executor` exists to prevent.
    ///
    /// Evidence merging is unchanged and deliberately module-agnostic. Ownership reads, runtime requirements, body
    /// snapshots, step count, task identities, and lifecycle events belong to the one receipt-bound execution rather
    /// than to whichever module a frame happened to run in, so they merge upward the same way across a module edge.
    fn execute_child_in_module<T>(
        &mut self,
        module: &BodyIrModule,
        locals: BTreeMap<LocalId, ReplacementValue>,
        local_types: BTreeMap<LocalId, IncanType>,
        steps: usize,
        execute: impl FnOnce(&mut BodyExecutor<'_, 'writer>) -> Result<T, ReplacementExecutionError>,
    ) -> Result<T, ReplacementExecutionError> {
        let mut child =
            BodyExecutor::with_locals(module, Rc::clone(&self.reachable), locals, local_types, steps, self.io);
        child.next_task_id = self.next_task_id;
        child.providers = self.providers.clone();
        // A frame running in another module raises refusals measured in that module's source, so record it here
        // rather than letting the failure inherit the entrypoint's file on its way out.
        let result = execute(&mut child).map_err(|error| error.measured_in_module(module.module_id.path()));
        let BodyExecutor {
            ownership_reads,
            runtime_requirements,
            body_snapshots,
            steps,
            next_task_id,
            task_lifecycle,
            ..
        } = child;
        self.ownership_reads.extend(ownership_reads);
        for requirement in runtime_requirements {
            if !self.runtime_requirements.contains(&requirement) {
                self.runtime_requirements.push(requirement);
            }
        }
        self.body_snapshots.extend(body_snapshots);
        self.steps = steps;
        self.next_task_id = self.next_task_id.max(next_task_id);
        self.task_lifecycle.extend(task_lifecycle);
        result
    }

    /// Construct one unpolled task directly from an identity-selected async Body-IR body.
    fn construct_task(
        &mut self,
        body: Body,
        locals: BTreeMap<LocalId, ReplacementValue>,
        span: HirSourceSpan,
    ) -> Result<Rc<RefCell<ReplacementTask>>, ReplacementExecutionError> {
        if !body.is_async || body.is_generator() {
            return Err(unsupported("non-task callable construction", span));
        }
        let id = self.next_task_id;
        self.next_task_id = self.next_task_id.saturating_add(1);
        self.record_task_event(id, "constructed", span);
        Ok(Rc::new(RefCell::new(ReplacementTask {
            id,
            body,
            locals,
            state: ReplacementTaskState::Constructed,
        })))
    }

    /// Record one source-span-preserving task transition for receipt-bound execution evidence.
    fn record_task_event(&mut self, task_id: usize, event: &'static str, span: HirSourceSpan) {
        self.task_lifecycle.push(TaskLifecycleEvent { task_id, event, span });
    }

    /// Poll an admitted task to its direct Body-IR completion.
    ///
    /// The initially admitted source-local profile has no external wake source: a task can only suspend on another
    /// direct task. Its frame is nevertheless explicit and shared, so its construction, poll, await/resume, result,
    /// and cancellation never collapse into a synchronous named-call result or a generator frame.
    fn drive_task(
        &mut self,
        task: &Rc<RefCell<ReplacementTask>>,
        span: HirSourceSpan,
    ) -> Result<ReplacementValue, ReplacementExecutionError> {
        let (id, body, locals) = {
            let mut task = task.borrow_mut();
            match &task.state {
                ReplacementTaskState::Completed(value) => return Ok(value.clone()),
                ReplacementTaskState::Cancelled => {
                    return Err(unsupported("await of a race-cancelled task", span));
                }
                ReplacementTaskState::Running => {
                    return Err(unsupported("recursive task poll", span));
                }
                ReplacementTaskState::Failed => {
                    return Err(unsupported("await of a failed direct task", span));
                }
                ReplacementTaskState::Constructed => {}
            }
            task.state = ReplacementTaskState::Running;
            (task.id, task.body.clone(), task.locals.clone())
        };
        self.record_task_event(id, "polled", span);
        let result = self.execute_child(locals, self.steps, |child| {
            child.active_task = Some(id);
            child.record_body(&body);
            child.execute_block(&body.block).and_then(|flow| match flow {
                Flow::Return(Some(value), return_span) => {
                    coerce_value_to_checked_type(value, &body.return_type, return_span)
                }
                Flow::Return(None, _) | Flow::Next => Ok(ReplacementValue::Unit),
                Flow::Break | Flow::Continue => Err(unsupported("loop control outside a direct task loop", body.span)),
            })
        });
        let value = match result {
            Ok(value) => value,
            Err(error) => {
                task.borrow_mut().state = ReplacementTaskState::Failed;
                return Err(error);
            }
        };
        task.borrow_mut().state = ReplacementTaskState::Completed(value.clone());
        self.record_task_event(id, "completed", span);
        Ok(value)
    }

    /// Cancel one losing `race for` task at the selection boundary.
    fn cancel_task(
        &mut self,
        task: &Rc<RefCell<ReplacementTask>>,
        span: HirSourceSpan,
    ) -> Result<(), ReplacementExecutionError> {
        let id = {
            let mut task = task.borrow_mut();
            match &task.state {
                ReplacementTaskState::Cancelled => return Ok(()),
                ReplacementTaskState::Running => {
                    return Err(unsupported("cancellation of a running direct task", span));
                }
                ReplacementTaskState::Failed => {
                    return Err(unsupported("cancellation of a failed direct task", span));
                }
                ReplacementTaskState::Constructed | ReplacementTaskState::Completed(_) => {
                    task.state = ReplacementTaskState::Cancelled;
                    task.id
                }
            }
        };
        self.record_task_event(id, "cancelled", span);
        Ok(())
    }

    /// Cancel every still-constructed losing race frame after the selected frame has failed.
    ///
    /// The source-local profile constructs all race arms before it polls the first source-order arm. A winner error
    /// must therefore close those unpolled losers before it escapes. This helper is intentionally total and only
    /// changes `Constructed` frames: a malformed repeated/running/terminal handle cannot replace the selected
    /// frame's original diagnostic or be relabelled as cancellation.
    fn cancel_constructed_race_losers_after_failure(
        &mut self,
        tasks: &[Rc<RefCell<ReplacementTask>>],
        winner_index: usize,
        span: HirSourceSpan,
    ) {
        for (index, task) in tasks.iter().enumerate() {
            if index == winner_index {
                continue;
            }
            let id = {
                let mut task = task.borrow_mut();
                if matches!(task.state, ReplacementTaskState::Constructed) {
                    task.state = ReplacementTaskState::Cancelled;
                    Some(task.id)
                } else {
                    None
                }
            };
            if let Some(id) = id {
                self.record_task_event(id, "cancelled", span);
            }
        }
    }

    /// Bind direct API arguments in declaration order, applying stored defaults only to omitted trailing slots.
    fn bind_direct_arguments(
        &mut self,
        body: &Body,
        args: &[ReplacementValue],
    ) -> Result<BTreeMap<LocalId, ReplacementValue>, ReplacementExecutionError> {
        let mut supplied = args.iter().cloned().map(Some).collect::<Vec<_>>();
        supplied.resize_with(body.params.len(), || None);
        let local_types = self.local_types.clone();
        self.bind_parameter_values(&body.params, supplied, &BTreeMap::new(), &local_types, body.span)
    }

    /// Evaluate a resolved call site's operands in written source order and bind them to declared parameter slots.
    fn bind_call_arguments(
        &mut self,
        params: &[CallableParam],
        args: &[&Operand],
        binding: &ArgumentBinding,
        captures: &BTreeMap<LocalId, ReplacementValue>,
        local_types: &BTreeMap<LocalId, IncanType>,
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
            supplied[argument.slot] = Some(self.evaluate_operand(args[index], span)?);
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
        self.bind_parameter_values(params, supplied, captures, local_types, span)
    }

    /// Materialize supplied values, source defaults, and construction-time partial presets into one isolated frame.
    fn bind_parameter_values(
        &mut self,
        params: &[CallableParam],
        supplied: Vec<Option<ReplacementValue>>,
        captures: &BTreeMap<LocalId, ReplacementValue>,
        local_types: &BTreeMap<LocalId, IncanType>,
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
                    CallableParamDefault::Source(computation) => self.evaluate_default(computation, local_types)?,
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
            let value = coerce_value_to_checked_type(value, &parameter.ty, call_span)?;
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
        local_types: &BTreeMap<LocalId, IncanType>,
    ) -> Result<ReplacementValue, ReplacementExecutionError> {
        let result = self.execute_child_with_local_types(
            BTreeMap::new(),
            local_types.clone(),
            self.steps,
            |default_executor| {
                for statement in &computation.stmts {
                    match default_executor.execute_statement(statement)? {
                        Flow::Next => {}
                        Flow::Return(..) | Flow::Break | Flow::Continue => {
                            return Err(unsupported(
                                "control flow in a callable default computation",
                                statement.span,
                            ));
                        }
                    }
                }
                default_executor.evaluate_operand(&computation.result, computation.span)
            },
        )?;
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
                if let Some(flow) = self.pending_flow.take() {
                    return Ok(flow);
                }
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
                statement.span,
            )),
            StatementKind::Assert {
                kind: AssertionKind::Condition { cond },
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
            // Refused by `validate_statement_profile` before execution starts -- see its own `Assert` arms.
            StatementKind::Assert {
                kind: AssertionKind::Pattern { .. },
                ..
            } => Err(unsupported("pattern assertion", statement.span)),
            StatementKind::Assert {
                kind: AssertionKind::Raises { .. },
                ..
            } => Err(unsupported("raises assertion", statement.span)),
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
            StatementKind::TryPropagate {
                destination,
                operand,
                error_routing,
            } => self.execute_try_propagate(destination, operand, error_routing, statement.span),
            StatementKind::Await { destination, awaited } => {
                self.execute_await(destination.as_ref(), awaited, statement.span)
            }
            StatementKind::Race { destination, arms } => self.execute_race(destination.as_ref(), arms, statement.span),
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
        args: &[ArgumentElement],
        span: HirSourceSpan,
    ) -> Result<Flow, ReplacementExecutionError> {
        // Mirrors `validate_call_profile`: validation already refused a spread-bearing call, and refusing again
        // here keeps the executor fail-closed rather than relying on that ordering.
        let Some(args) = fixed_operands(args) else {
            return Err(unsupported(
                format!("call to {} with a spread argument", callee_label(callee)),
                span,
            ));
        };
        let args = args.as_slice();
        let destination = destination.ok_or_else(|| unsupported("discarded string-concatenation result", span))?;
        let local = bare_local(destination, span)?;
        let value = match callee {
            Callee::Helper(
                helper @ (HelperOp::StrUpper
                | HelperOp::StrLower
                | HelperOp::StrStrip
                | HelperOp::StrLen
                | HelperOp::StrReplace
                | HelperOp::StrJoin
                | HelperOp::StrSplit
                | HelperOp::StrContains
                | HelperOp::StrEq
                | HelperOp::StrNe
                | HelperOp::StrLt
                | HelperOp::StrLe
                | HelperOp::StrGt
                | HelperOp::StrGe),
            ) => self.execute_string_helper(*helper, args, span)?,
            Callee::Helper(HelperOp::StrConcat) => {
                let [left, right] = args else {
                    return Err(unsupported("string-concatenation call arity", span));
                };
                let left = self.evaluate_operand(left, span)?.into_string(span)?;
                let right = self.evaluate_operand(right, span)?.into_string(span)?;
                ReplacementValue::Str(format!("{left}{right}"))
            }
            Callee::Helper(HelperOp::ListConcat) => {
                let [left, right] = args else {
                    return Err(unsupported("list-concatenation call arity", span));
                };
                let mut elements = self.evaluate_list_elements(left, span)?;
                elements.extend(self.evaluate_list_elements(right, span)?);
                // A fresh cursor, not either operand's: concatenation produces a new list, and inheriting a
                // partially-advanced iterator position would hand the result someone else's traversal state.
                ReplacementValue::List { elements, next: 0 }
            }
            Callee::Helper(helper @ (HelperOp::ListContains | HelperOp::ListNotContains)) => {
                let [haystack, needle] = args else {
                    return Err(unsupported("list-membership call arity", span));
                };
                // Operands arrive haystack-first, matching the helper's own signature rather than the source
                // spelling -- see `HelperOp::ListContains`.
                let elements = self.evaluate_list_elements(haystack, span)?;
                let needle = self.evaluate_operand(needle, span)?;
                // Every element must be comparable, not just the ones that happen to be. Skipping an element this
                // profile cannot compare would let a `false` mean "not present" and "could not tell" at once, which
                // is the silent-wrong-answer shape this operator's representation exists to avoid.
                if !needle.is_collection_scalar() || !elements.iter().all(ReplacementValue::is_collection_scalar) {
                    return Err(unsupported("list membership over a non-scalar element", span));
                }
                let found = elements.contains(&needle);
                ReplacementValue::Bool(matches!(helper, HelperOp::ListContains) == found)
            }
            Callee::Helper(helper @ (HelperOp::SetContains | HelperOp::SetNotContains)) => {
                let [haystack, needle] = args else {
                    return Err(unsupported("set-membership call arity", span));
                };
                let haystack = self.evaluate_operand(haystack, span)?;
                let needle = self.evaluate_operand(needle, span)?;
                let ReplacementValue::Set(values) = haystack else {
                    return Err(unsupported("set membership using a non-set carrier", span));
                };
                let found = values.contains(needle).map_err(|error| {
                    unsupported(format!("set membership with a non-scalar {} needle", error.kind), span)
                })?;
                ReplacementValue::Bool(matches!(helper, HelperOp::SetContains) == found)
            }
            Callee::Helper(helper @ (HelperOp::DictContainsKey | HelperOp::DictNotContainsKey)) => {
                let [haystack, needle] = args else {
                    return Err(unsupported("dict-membership call arity", span));
                };
                let haystack = self.evaluate_operand(haystack, span)?;
                let needle = self.evaluate_operand(needle, span)?;
                let ReplacementValue::Dict(values) = haystack else {
                    return Err(unsupported("dict membership using a non-dict carrier", span));
                };
                let found = values.contains_key(needle).map_err(|error| {
                    unsupported(format!("dict membership with a non-scalar {} needle", error.kind), span)
                })?;
                ReplacementValue::Bool(matches!(helper, HelperOp::DictContainsKey) == found)
            }
            Callee::Function(CallableTarget::Named(target)) if is_explicit_range_builtin(target) => {
                self.evaluate_range(args, span)?
            }
            Callee::Function(CallableTarget::Named(target))
                if explicit_builtin(target).is_some_and(|id| EXECUTABLE_BUILTINS.contains(&id)) =>
            {
                let Some(builtin) = explicit_builtin(target) else {
                    return Err(unsupported("builtin call without a resolved identity", span));
                };
                self.execute_builtin(builtin, args, span)?
            }
            Callee::Function(CallableTarget::Named(target)) => self.execute_named_callable(target, args, span)?,
            Callee::Function(CallableTarget::Local(target)) => self.execute_local_callable(target, args, span)?,
            Callee::Method(target)
                if replacement_method_operation(target) == Some(ReplacementMethodOperation::GeneratorCollect) =>
            {
                let [receiver] = args else {
                    return Err(unsupported("generator collect call arity", span));
                };
                let iterator = self.take_generator_receiver(receiver, span)?;
                self.collect_generator(iterator, span)?
            }
            Callee::Method(target)
                if replacement_method_operation(target) == Some(ReplacementMethodOperation::ValueEnumValue) =>
            {
                self.extract_value_enum_scalar(target, args, span)?
            }
            Callee::Method(target)
                if matches!(
                    replacement_method_operation(target),
                    Some(ReplacementMethodOperation::GeneratorMap | ReplacementMethodOperation::GeneratorFilter)
                ) =>
            {
                self.construct_generator_adapter(replacement_method_operation(target), args, span)?
            }
            Callee::ProviderOperation(plan) => self.execute_provider_operation(plan, args, span)?,
            _ => return Err(unsupported(format!("call to {}", callee_label(callee)), span)),
        };
        self.assign_local(local, value, span)?;
        Ok(Flow::Next)
    }

    /// Execute one admitted provider operation from its already-lowered plan.
    ///
    /// This executor's whole part in the vertical is here: it evaluates the call's operands into the inputs the plan
    /// describes and hands both to the provider runtime. It decides no authority, publishes no receipt, and applies
    /// no redaction; those belong to the RFC 104 contracts the runtime consumes. Whether the plan is executable at
    /// all is answered once by [`unexecutable_provider_plan`] — at the pre-execution profile gate, and again by the
    /// runtime immediately before it reaches a host — rather than a third time here.
    ///
    /// Notably, this never dispatches to the operation's own declaration body even when this module happens to
    /// contain one. The plan names a *provider* operation, and executing the local declaration instead would
    /// silently substitute source-local behavior for the service's.
    fn execute_provider_operation(
        &mut self,
        plan: &ProviderOperationPlan,
        args: &[&Operand],
        span: HirSourceSpan,
    ) -> Result<ReplacementValue, ReplacementExecutionError> {
        let Some(runtime) = self.providers.clone() else {
            return Err(unsupported(
                format!(
                    "provider operation `{}` without a provider runtime for this execution",
                    plan.operation.declaration_name
                ),
                span,
            ));
        };
        // The plan's runtime requirements are the executing body's too, and recording them here keeps a provider
        // invocation's demands visible in the same evidence every other direct execution reports.
        for requirement in &plan.runtime_requirements {
            if !self.runtime_requirements.contains(requirement) {
                self.runtime_requirements.push(requirement.clone());
            }
        }
        let mut inputs = Vec::with_capacity(plan.inputs.len());
        for input in &plan.inputs {
            let operand = args
                .get(input.written_position)
                .ok_or_else(|| unsupported("provider operation input outside the call's arguments", span))?;
            let value = self.evaluate_operand(operand, span)?;
            inputs.push(ProviderInputValue {
                slot: input.slot,
                written_position: input.written_position,
                ty: input.ty.clone(),
                span: input.span,
                value,
            });
        }
        runtime.execute(plan, inputs)
    }

    /// Suspend the active direct task on one source-local task value and resume it with that task's result.
    fn execute_await(
        &mut self,
        destination: Option<&Place>,
        awaited: &Operand,
        span: HirSourceSpan,
    ) -> Result<Flow, ReplacementExecutionError> {
        let destination = destination.ok_or_else(|| unsupported("await without a destination", span))?;
        let destination = bare_local(destination, span)?;
        let awaited = self.evaluate_operand(awaited, span)?;
        let ReplacementValue::Task(task) = awaited else {
            return Err(unsupported(
                format!("await of {} outside the direct task profile", value_kind(&awaited)),
                span,
            ));
        };
        if let Some(task_id) = self.active_task {
            self.record_task_event(task_id, "await_suspended", span);
        }
        let value = self.drive_task(&task, span)?;
        if let Some(task_id) = self.active_task {
            self.record_task_event(task_id, "await_resumed", span);
        }
        self.assign_local(destination, value, span)?;
        Ok(Flow::Next)
    }

    /// Poll source-order race arms until one is ready, then select it and cancel every loser.
    fn execute_race(
        &mut self,
        destination: Option<&Place>,
        arms: &[incan_semantics_core::body_ir::RaceArm],
        span: HirSourceSpan,
    ) -> Result<Flow, ReplacementExecutionError> {
        let destination = destination.ok_or_else(|| unsupported("race without a destination", span))?;
        let destination = bare_local(destination, span)?;
        if arms.is_empty() {
            return Err(unsupported("race without an arm", span));
        }

        // The lowering emits construction calls before this Race statement. Reading the retained operands here
        // therefore captures each already-constructed frame before a poll can select a winner.
        let tasks = arms
            .iter()
            .map(|arm| {
                let awaitable = self.evaluate_operand(&arm.awaitable, span)?;
                let ReplacementValue::Task(task) = awaitable else {
                    return Err(unsupported(
                        format!("race over {} outside the direct task profile", value_kind(&awaitable)),
                        span,
                    ));
                };
                Ok(task)
            })
            .collect::<Result<Vec<_>, ReplacementExecutionError>>()?;

        if let Some(task_id) = self.active_task {
            self.record_task_event(task_id, "race_suspended", span);
        }
        // This follows `std.async::race::scoped_race`: arms are polled in source order and the first ready arm wins
        // immediately. All awaitables were already constructed above; every later arm remains unpolled and is
        // cancelled below, so a loser cannot execute just because it appears in this direct scheduler round.
        let winner_index = 0;
        let winner_task = tasks
            .first()
            .ok_or_else(|| unsupported("race without a ready arm", span))?;
        let winner_value = match self.drive_task(winner_task, span) {
            Ok(value) => value,
            Err(error) => {
                self.cancel_constructed_race_losers_after_failure(&tasks, winner_index, span);
                return Err(error);
            }
        };
        let winner_id = tasks[winner_index].borrow().id;
        self.record_task_event(winner_id, "race_winner", span);
        for (index, task) in tasks.iter().enumerate() {
            if index != winner_index {
                self.cancel_task(task, span)?;
            }
        }
        if let Some(task_id) = self.active_task {
            self.record_task_event(task_id, "race_resumed", span);
        }

        let arm = &arms[winner_index];
        let prior_locals = self.locals.clone();
        self.assign_local(arm.binding, winner_value, span)?;
        let arm_outcome = (|| -> Result<Result<ReplacementValue, Flow>, ReplacementExecutionError> {
            match self.execute_block(&arm.body)? {
                Flow::Next => Ok(Ok(self.evaluate_operand(&arm.result, span)?)),
                flow => Ok(Err(flow)),
            }
        })();
        self.locals = prior_locals;
        match arm_outcome? {
            Ok(value) => {
                self.assign_local(destination, value, span)?;
                Ok(Flow::Next)
            }
            Err(flow) => Ok(flow),
        }
    }

    /// Invoke a stored closure or partial through its resolved call-site binding in a fresh frame.
    fn execute_local_callable(
        &mut self,
        target: &LocalCallableTarget,
        args: &[&Operand],
        span: HirSourceSpan,
    ) -> Result<ReplacementValue, ReplacementExecutionError> {
        let callable = self.take_callable_receiver(&target.operand, span)?;
        let captures = callable.captures.iter().cloned().collect::<BTreeMap<_, _>>();
        if captures.len() != callable.captures.len() {
            return Err(unsupported("duplicate callable capture local", span));
        }
        let local_types = self.local_types.clone();
        let locals =
            self.bind_call_arguments(&callable.params, args, &target.binding, &captures, &local_types, span)?;
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
        let result = self.execute_child(locals, self.steps, |child| {
            for statement in &callable.body.stmts {
                match child.execute_statement(statement)? {
                    Flow::Next => {}
                    Flow::Return(..) | Flow::Break | Flow::Continue => {
                        return Err(unsupported(
                            "control flow in a callable expression body",
                            statement.span,
                        ));
                    }
                }
            }
            child.evaluate_operand(&callable.body.result, span)
        })?;
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
        args: &[&Operand],
        span: HirSourceSpan,
    ) -> Result<ReplacementValue, ReplacementExecutionError> {
        let (callee_module, body) = self.resolve_named_callable(target, span)?;
        validate_direct_body_profile(&body)?;
        if body.is_async && !target.type_args.is_empty() {
            return Err(unsupported("generic async callable target", span));
        }
        let local_types = body.locals.iter().map(|local| (local.id, local.ty.clone())).collect();
        let locals = self.bind_call_arguments(
            &body.params,
            args,
            &target.binding,
            &BTreeMap::new(),
            &local_types,
            span,
        )?;
        if body.is_async {
            return self.construct_task(body, locals, span).map(ReplacementValue::Task);
        }
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
        let frame_module = callee_module.unwrap_or_else(|| self.module.clone());
        let local_types_for_frame = self.local_types.clone();
        self.execute_child_in_module(&frame_module, locals, local_types_for_frame, self.steps, |child| {
            child.record_body(&body);
            match child.execute_block(&body.block)? {
                Flow::Return(Some(value), return_span) => {
                    coerce_value_to_checked_type(value, &body.return_type, return_span)
                }
                Flow::Return(None, _) | Flow::Next => Ok(ReplacementValue::Unit),
                Flow::Break | Flow::Continue => {
                    Err(unsupported("loop control outside a nested callable loop", body.span))
                }
            }
        })
    }

    /// Resolve a named call to its declaration, and to the module that owns it when that is not this frame's own.
    ///
    /// A same-module call keeps its existing path exactly: `direct_call_id` is a span identity that only exists for a
    /// declaration physically present here, so its presence already proves the callee is local and
    /// `named_callable_body` performs the same checks it always did. `None` for the owning module means "this
    /// frame's module", so nothing about same-module execution changes.
    ///
    /// A call whose callee is imported has no such span identity, and this is the only case that consults the wider
    /// graph. It resolves on the canonical identity the typechecker selected, never on the callee's spelling: an
    /// import spelling, alias, or source path is not a dispatch key, and a same-named declaration in a reachable
    /// module must not answer for a different one.
    fn resolve_named_callable(
        &self,
        target: &NamedCallableTarget,
        span: HirSourceSpan,
    ) -> Result<(Option<BodyIrModule>, Body), ReplacementExecutionError> {
        if target.direct_call_id.is_some() {
            return Ok((None, named_callable_body(&self.module, target, span)?.clone()));
        }
        let canonical = target.canonical.as_ref().ok_or_else(|| {
            unsupported(
                format!(
                    "named callable `{}` without a same-module declaration identity or a canonical target",
                    target.name
                ),
                span,
            )
        })?;
        let mut resolved = self
            .reachable
            .iter()
            .filter_map(|module| module.body_for_canonical_target(canonical).map(|body| (module, body)));
        let (module, body) = resolved.next().ok_or_else(|| {
            unsupported(
                format!(
                    "named callable `{}` resolves to a declaration outside this execution graph",
                    target.name
                ),
                span,
            )
        })?;
        if resolved.next().is_some() {
            return Err(unsupported(
                format!(
                    "named callable `{}` resolves to more than one module in this execution graph",
                    target.name
                ),
                span,
            ));
        }
        Ok((Some(module.clone()), body.clone()))
    }

    /// Capture one admitted map or filter adapter without polling its source or callback.
    fn construct_generator_adapter(
        &mut self,
        operation: Option<ReplacementMethodOperation>,
        args: &[&Operand],
        span: HirSourceSpan,
    ) -> Result<ReplacementValue, ReplacementExecutionError> {
        let [receiver, callback] = args else {
            return Err(unsupported("generator adapter arity", span));
        };
        let source = self.take_iterable_receiver(receiver, span)?;
        let callback = self.evaluate_operand(callback, span)?;
        let ReplacementValue::Callable(callback) = callback else {
            return Err(unsupported("generator adapter callback is not a stored callable", span));
        };
        let kind = match operation {
            Some(ReplacementMethodOperation::GeneratorMap) => ReplacementAdapterKind::Map,
            Some(ReplacementMethodOperation::GeneratorFilter) => ReplacementAdapterKind::Filter,
            _ => return Err(unsupported("generator adapter without a canonical method target", span)),
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
        args: &[&Operand],
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

    /// Poll one admitted range, structural list or canonical Zip and express exhaustion as the Body-IR loop break it
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
        self.assign_local(bare_local(destination, span)?, value, span)?;
        Ok(Flow::Next)
    }

    /// Assign exactly the authoritative Body-IR `LocalId` selected by lowering.
    ///
    /// Body IR has already resolved repeated source spellings to the canonical local selected by lexical scope. The
    /// executor writes that exact `LocalId` and never reconstructs binding equivalence or aliases locals by name.
    fn assign_local(
        &mut self,
        local: LocalId,
        value: ReplacementValue,
        span: HirSourceSpan,
    ) -> Result<(), ReplacementExecutionError> {
        let target = self
            .local_types
            .get(&local)
            .ok_or_else(|| unsupported("Body-IR assignment destination without a declared type", span))?;
        let value = coerce_value_to_checked_type(value, target, span)?;
        self.locals.insert(local, value);
        Ok(())
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
                Flow::Return(value, return_span) => return Ok(Flow::Return(value, return_span)),
            }
            if self.steps >= MAX_EXECUTION_STEPS {
                return Err(runtime_failure(
                    format!("normalized loop exceeded the {MAX_EXECUTION_STEPS}-step replacement profile limit"),
                    span,
                ));
            }
        }
    }

    /// Execute an admitted compiler-owned builtin from its retained target identity.
    ///
    /// Each arm consumes the checked operand profile; the separate shadow route measures agreement with native
    /// execution rather than deriving an answer from this evaluator.
    ///
    /// String `len` follows the canonical Unicode-scalar helper shared with generated Rust. Collection length keeps
    /// counting materialized elements, so the executor does not infer a string policy from a source name.
    fn execute_builtin(
        &mut self,
        builtin: BuiltinFnId,
        args: &[&Operand],
        span: HirSourceSpan,
    ) -> Result<ReplacementValue, ReplacementExecutionError> {
        if matches!(builtin, BuiltinFnId::Print) {
            return self.execute_print(args, span);
        }
        if matches!(builtin, BuiltinFnId::Enumerate | BuiltinFnId::Zip) {
            return self.execute_list_iteration_builtin(builtin, args, span);
        }

        let [argument] = args else {
            return Err(unsupported(format!("`{}` call arity", builtins::as_str(builtin)), span));
        };
        let value = self.evaluate_operand(argument, span)?;

        match builtin {
            // Canonical `bool` follows the native emitter only for values this replacement profile represents with
            // the same checked carrier. Float, bytes, frozen collections, and higher-level wrappers remain visible
            // refusals rather than acquiring truthiness from a lossy runtime guess.
            BuiltinFnId::Bool => match value {
                ReplacementValue::Bool(value) => Ok(ReplacementValue::Bool(value)),
                ReplacementValue::Int(value) => Ok(ReplacementValue::Bool(value != 0)),
                ReplacementValue::Str(value) => Ok(ReplacementValue::Bool(!value.is_empty())),
                ReplacementValue::List { elements, .. } => Ok(ReplacementValue::Bool(!elements.is_empty())),
                ReplacementValue::Set(values) => Ok(ReplacementValue::Bool(!values.is_empty())),
                ReplacementValue::Dict(values) => Ok(ReplacementValue::Bool(!values.is_empty())),
                other => Err(unsupported(format!("`bool` of {}", value_kind(&other)), span)),
            },
            // Collection length counts elements. String length follows the canonical Unicode-scalar contract, and a
            // value with no length at all remains outside the profile.
            BuiltinFnId::Len => match value {
                ReplacementValue::List { elements, .. }
                | ReplacementValue::CollectedGenerator { elements, .. }
                | ReplacementValue::Tuple(elements) => Ok(ReplacementValue::Int(elements.len() as i64)),
                ReplacementValue::Set(values) => Ok(ReplacementValue::Int(values.len() as i64)),
                ReplacementValue::Dict(values) => Ok(ReplacementValue::Int(values.len() as i64)),
                ReplacementValue::Str(value) => Ok(ReplacementValue::Int(incan_core::strings::str_len(&value))),
                other => Err(unsupported(format!("`len` of {}", value_kind(&other)), span)),
            },
            BuiltinFnId::Abs => match value {
                ReplacementValue::Int(value) => value
                    .checked_abs()
                    .map(ReplacementValue::Int)
                    .ok_or_else(|| runtime_failure("integer overflow in builtin `abs`".to_string(), span)),
                other => Err(unsupported(format!("`abs` of {}", value_kind(&other)), span)),
            },
            // These pairs mirror the existing Rust emitter's scalar conversions. Typed source carriers retain
            // their identity until this compiler-selected conversion explicitly produces an ordinary result.
            BuiltinFnId::Str => match value {
                ReplacementValue::Int(value) => Ok(ReplacementValue::Str(value.to_string())),
                ReplacementValue::Bool(value) => Ok(ReplacementValue::Str(value.to_string())),
                ReplacementValue::Str(value) => Ok(ReplacementValue::Str(value)),
                ReplacementValue::Float(value) => Ok(ReplacementValue::Str(value.to_string())),
                ReplacementValue::Numeric(value) => Ok(ReplacementValue::Str(value.observable_text())),
                other => Err(unsupported(format!("`str` of {}", value_kind(&other)), span)),
            },
            BuiltinFnId::Int => match value {
                ReplacementValue::Int(value) => Ok(ReplacementValue::Int(value)),
                ReplacementValue::Bool(value) => Ok(ReplacementValue::Int(i64::from(value))),
                ReplacementValue::Str(value) => parse_int_conversion(&value, span),
                ReplacementValue::Float(value) => Ok(ReplacementValue::Int(value as i64)),
                ReplacementValue::Numeric(value) => numeric_to_int(value, span).map(ReplacementValue::Int),
                other => Err(unsupported(format!("`int` of {}", value_kind(&other)), span)),
            },
            BuiltinFnId::Float => match value {
                ReplacementValue::Int(value) => Ok(ReplacementValue::Float(value as f64)),
                ReplacementValue::Str(value) => parse_float_conversion(&value, span),
                ReplacementValue::Float(value) => Ok(ReplacementValue::Float(value)),
                ReplacementValue::Numeric(value) => numeric_to_float(value, span).map(ReplacementValue::Float),
                other => Err(unsupported(format!("`float` of {}", value_kind(&other)), span)),
            },
            // Checked integer accumulation, with bools counted as 1/0 exactly as the emitted Rust does.
            BuiltinFnId::Sum => {
                let elements = integer_elements(&value, "sum", span)?;
                let sum = elements
                    .iter()
                    .try_fold(0_i64, |total, value| total.checked_add(*value))
                    .ok_or_else(|| runtime_failure("integer overflow in builtin `sum`".to_string(), span))?;
                Ok(ReplacementValue::Int(sum))
            }
            BuiltinFnId::Min => {
                let elements = integer_elements(&value, "min", span)?;
                elements
                    .iter()
                    .min()
                    .copied()
                    .map(ReplacementValue::Int)
                    .ok_or_else(|| unsupported("`min` of an empty collection", span))
            }
            BuiltinFnId::Max => {
                let elements = integer_elements(&value, "max", span)?;
                elements
                    .iter()
                    .max()
                    .copied()
                    .map(ReplacementValue::Int)
                    .ok_or_else(|| unsupported("`max` of an empty collection", span))
            }
            // This first sorting profile has no checked element-type fact at runtime for an empty list, so it
            // admits only a nonempty list whose represented elements prove the integer carrier. Sorting consumes
            // the evaluated clone and returns a fresh cursor, leaving the source local unchanged.
            BuiltinFnId::Sorted => match value {
                ReplacementValue::List { elements, .. } if elements.is_empty() => Err(unsupported(
                    "`sorted` of an empty list outside the integer-only profile",
                    span,
                )),
                ReplacementValue::List { elements, .. } => {
                    let mut values = elements
                        .into_iter()
                        .map(|element| match element {
                            ReplacementValue::Int(value) => Ok(value),
                            other => Err(unsupported(
                                format!(
                                    "`sorted` list element {} outside the integer-only profile",
                                    value_kind(&other)
                                ),
                                span,
                            )),
                        })
                        .collect::<Result<Vec<_>, ReplacementExecutionError>>()?;
                    values.sort();
                    Ok(ReplacementValue::List {
                        elements: values.into_iter().map(ReplacementValue::Int).collect(),
                        next: 0,
                    })
                }
                other => Err(unsupported(format!("`sorted` of {}", value_kind(&other)), span)),
            },
            BuiltinFnId::JsonStringify => stringify_json_scalar(value, span),
            other => Err(unsupported(format!("builtin `{}`", builtins::as_str(other)), span)),
        }
    }

    /// Construct canonical global enumeration or Zip after the owning Body's checked-type preflight.
    ///
    /// Enumeration has a checked list result and therefore materializes its zero-based pairs. Zip retains two
    /// list cursors for polling; evaluating its operands here preserves written argument order without inventing
    /// general user-iterator dispatch. Both start fresh traversals rather than inheriting another local's cursor.
    fn execute_list_iteration_builtin(
        &mut self,
        builtin: BuiltinFnId,
        args: &[&Operand],
        span: HirSourceSpan,
    ) -> Result<ReplacementValue, ReplacementExecutionError> {
        match (builtin, args) {
            (BuiltinFnId::Enumerate, [source]) => {
                let values = self.evaluate_list_elements(source, span)?;
                let elements = values
                    .into_iter()
                    .enumerate()
                    .map(|(index, value)| {
                        let index = i64::try_from(index)
                            .map_err(|_| unsupported("enumerate index outside the Incan int range", span))?;
                        Ok(ReplacementValue::Tuple(vec![ReplacementValue::Int(index), value]))
                    })
                    .collect::<Result<Vec<_>, ReplacementExecutionError>>()?;
                Ok(ReplacementValue::List { elements, next: 0 })
            }
            (BuiltinFnId::Zip, [left, right]) => {
                let left = self.evaluate_list_elements(left, span)?;
                let right = self.evaluate_list_elements(right, span)?;
                Ok(ReplacementValue::Zip(Box::new(ReplacementZip {
                    left: ReplacementValue::List {
                        elements: left,
                        next: 0,
                    },
                    right: ReplacementValue::List {
                        elements: right,
                        next: 0,
                    },
                })))
            }
            _ => Err(unsupported("enumerate/Zip call arity", span)),
        }
    }

    /// Deliver a `print`/`println` line through the caller's stdout writer and flush before continuing.
    ///
    /// Every argument renders, space-separated, matching Python's `print` and the Rust-emission backend's
    /// `emit_print_call`. That agreement is recent: both backends previously emitted only the first argument and
    /// discarded the rest, so `println("count", 3)` printed `count` with nothing reporting the loss.
    ///
    /// Accepted bytes are observed independently of delivery. A later runtime or receipt failure cannot hide the
    /// line, and a partial write or flush failure is reported at this original call span.
    fn execute_print(
        &mut self,
        args: &[&Operand],
        span: HirSourceSpan,
    ) -> Result<ReplacementValue, ReplacementExecutionError> {
        let mut parts = Vec::with_capacity(args.len());
        for argument in args {
            let value = self.evaluate_operand(argument, span)?;
            parts.push(format_interpolation(&value, FormatStyle::Display, span)?);
        }
        let rendered = parts.join(" ");
        self.io
            .print_line(rendered)
            .map_err(|error| ReplacementExecutionError::ProgramIo {
                error,
                span,
                span_start: span.start,
                span_end: span.end,
            })?;
        Ok(ReplacementValue::Unit)
    }

    /// Evaluate an f-string into a single string value.
    ///
    /// Body IR represents an f-string as its own structured node rather than a desugared concatenation, so this
    /// walks the parts directly. Only the scalar kinds whose rendering provably matches the Rust-emission backend
    /// are interpolated; anything else refuses by name rather than inventing a spelling the two backends would
    /// disagree on. Ordinary Float Display uses the same normalized `f64` rendering as the Rust-emission backend;
    /// Float Debug remains a refusal until that distinct formatting contract has direct parity evidence.
    fn evaluate_format(
        &mut self,
        parts: &[FormatPart],
        span: HirSourceSpan,
    ) -> Result<ReplacementValue, ReplacementExecutionError> {
        let mut rendered = String::new();
        for part in parts {
            match part {
                FormatPart::Literal(text) => rendered.push_str(text),
                FormatPart::Expr { operand, style } => {
                    let value = self.evaluate_operand(operand, span)?;
                    rendered.push_str(&format_interpolation(&value, *style, span)?);
                }
            }
        }
        Ok(ReplacementValue::Str(rendered))
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
            Rvalue::IsInstance { value, target, .. } => self.evaluate_isinstance(value, target, span),
            Rvalue::Format(parts) => self.evaluate_format(parts, span),
            Rvalue::Dict(entries) => self.evaluate_dict(entries, span),
            Rvalue::Aggregate(kind, operands) => self.evaluate_aggregate(kind, operands, span),
            Rvalue::FieldlessEnumVariant(target) => self.evaluate_fieldless_enum_variant(target, span),
            Rvalue::ValueEnumVariant(target) => self.evaluate_value_enum_variant(target, span),
            Rvalue::ResultVariant(variant) => self.evaluate_result_variant(variant, span),
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
            Rvalue::Match { scrutinee, arms } => self.evaluate_match(scrutinee, arms, span),
        }
    }

    /// Evaluate one prevalidated compiler-owned primitive type test.
    fn evaluate_isinstance(
        &mut self,
        value: &Operand,
        target: &incan_semantics_core::body_ir::IsInstanceTarget,
        span: HirSourceSpan,
    ) -> Result<ReplacementValue, ReplacementExecutionError> {
        let value = self.evaluate_operand(value, span)?;
        let matches = matches!(
            (&value, &target.ty),
            (ReplacementValue::Int(_), IncanType::Primitive(IncanPrimitiveType::Int))
                | (
                    ReplacementValue::Bool(_),
                    IncanType::Primitive(IncanPrimitiveType::Bool)
                )
                | (ReplacementValue::Str(_), IncanType::Primitive(IncanPrimitiveType::Str))
                | (
                    ReplacementValue::Float(_),
                    IncanType::Primitive(IncanPrimitiveType::Float)
                )
        );
        Ok(ReplacementValue::Bool(matches))
    }

    /// Materialize a dict in written key-then-value order, preserving the later-entry-wins construction rule.
    fn evaluate_dict(
        &mut self,
        entries: &[DictEntry],
        span: HirSourceSpan,
    ) -> Result<ReplacementValue, ReplacementExecutionError> {
        let mut values = Vec::with_capacity(entries.len());
        for entry in entries {
            let DictEntry::Pair(key, value) = entry else {
                return Err(unsupported("dict aggregate with a spread entry", span));
            };
            let key = self.evaluate_operand(key, span)?;
            let value = self.evaluate_operand(value, span)?;
            values.push((key, value));
        }
        let dict = ReplacementDict::from_entries(values)
            .map_err(|error| unsupported(format!("dict aggregate with a non-scalar {} key", error.kind), span))?;
        Ok(ReplacementValue::Dict(Rc::new(dict)))
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
        self.execute_child(locals, resume_steps, |deferred| {
            if let Some(body) = &named_body {
                deferred.record_body(body);
            }
            if let Some(evidence) = frame_evidence {
                deferred.record_frame_evidence(evidence);
            }
            let result = deferred.resume_generator_frame(&mut generator.frame, span);
            generator.frame.locals = std::mem::take(&mut deferred.locals);
            generator.frame.steps = deferred.steps;
            result
        })
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
            ReplacementValue::Zip(zip) => {
                let Some(left) = self.poll_iterator(&mut zip.left, span)? else {
                    return Ok(None);
                };
                let Some(right) = self.poll_iterator(&mut zip.right, span)? else {
                    return Ok(None);
                };
                Ok(Some(ReplacementValue::Tuple(vec![left, right])))
            }
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
        let local_types = self.local_types.clone();
        let locals = self.bind_parameter_values(&callable.params, supplied, &captures, &local_types, span)?;
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
                    Flow::Return(..) => {
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

    /// Materialize source-local tuples/lists or a retained plain-model constructor without inventing dict or set
    /// behavior.
    fn evaluate_aggregate(
        &mut self,
        kind: &AggregateKind,
        operands: &[ArgumentElement],
        span: HirSourceSpan,
    ) -> Result<ReplacementValue, ReplacementExecutionError> {
        // Validation already refused a spread-bearing aggregate; refuse again rather than trusting that ordering.
        let Some(operands) = fixed_operands(operands) else {
            return Err(unsupported(
                format!("{} aggregate with a spread element", aggregate_label(kind)),
                span,
            ));
        };
        let operands = operands.as_slice();
        if let AggregateKind::Constructor(target) = kind {
            return self.evaluate_nominal_constructor(target, operands, span);
        }
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
            AggregateKind::Set => {
                let set = ReplacementSet::from_elements(values).map_err(|error| {
                    unsupported(format!("set aggregate with a non-scalar {} element", error.kind), span)
                })?;
                Ok(ReplacementValue::Set(Rc::new(set)))
            }
            _ => Err(unsupported(format!("{} aggregate", aggregate_label(kind)), span)),
        }
    }

    /// Construct the checked intrinsic Result carrier without falling back to constructor-name interpretation.
    fn evaluate_result_variant(
        &mut self,
        variant: &ResultVariant,
        span: HirSourceSpan,
    ) -> Result<ReplacementValue, ReplacementExecutionError> {
        if !is_direct_result_payload_type(&variant.ok_type) || !is_direct_result_payload_type(&variant.error_type) {
            return Err(unsupported(
                "Result construction with an unsupported payload type",
                span,
            ));
        }
        let payload = self.evaluate_operand(&variant.payload, span)?;
        let payload_type = match variant.kind {
            ResultVariantKind::Ok => &variant.ok_type,
            ResultVariantKind::Err => &variant.error_type,
        };
        if !payload.is_direct_result_payload() || !self.value_matches_direct_result_type(&payload, payload_type) {
            return Err(unsupported(
                format!(
                    "Result construction with {} payload incompatible with retained type `{payload_type}`",
                    value_kind(&payload)
                ),
                span,
            ));
        }
        self.record_frame_evidence(format!(
            "executed Result::{} construction span={}..{}",
            variant.kind.as_str(),
            span.start,
            span.end
        ));
        Ok(ReplacementValue::Result {
            kind: variant.kind,
            payload: Box::new(payload),
            ok_type: variant.ok_type.clone(),
            error_type: variant.error_type.clone(),
        })
    }

    /// Execute the first selected structured match arm while restoring the complete local environment after every
    /// failed guard or completed arm. Pattern locals are arm-scoped source facts; leaking them would let an arm
    /// shadow an enclosing local in later code.
    fn evaluate_match(
        &mut self,
        scrutinee: &Operand,
        arms: &[MatchArm],
        span: HirSourceSpan,
    ) -> Result<ReplacementValue, ReplacementExecutionError> {
        let value = self.evaluate_operand(scrutinee, span)?;
        for arm in arms {
            let saved_locals = self.locals.clone();
            let Some(bindings) = self.match_pattern(&arm.pattern, &value, span)? else {
                self.locals = saved_locals;
                continue;
            };
            self.bind_pattern_values(bindings, span)?;

            for statement in &arm.guard_stmts {
                self.record_step(statement.span)?;
                match self.execute_statement(statement)? {
                    Flow::Next => {}
                    flow => {
                        self.locals = saved_locals;
                        self.record_frame_evidence(format!(
                            "executed direct match arm span={}..{}",
                            span.start, span.end
                        ));
                        self.pending_flow = Some(flow);
                        return Ok(ReplacementValue::Unit);
                    }
                }
            }
            if let Some(guard) = &arm.guard
                && !self.evaluate_operand(guard, span)?.into_bool(span)?
            {
                self.locals = saved_locals;
                continue;
            }

            for statement in &arm.body_stmts {
                self.record_step(statement.span)?;
                match self.execute_statement(statement)? {
                    Flow::Next => {}
                    flow => {
                        self.locals = saved_locals;
                        self.record_frame_evidence(format!(
                            "executed direct match arm span={}..{}",
                            span.start, span.end
                        ));
                        self.pending_flow = Some(flow);
                        return Ok(ReplacementValue::Unit);
                    }
                }
            }
            let result = self.evaluate_operand(&arm.result, span)?;
            self.locals = saved_locals;
            self.record_frame_evidence(format!("executed direct match arm span={}..{}", span.start, span.end));
            return Ok(result);
        }
        Err(runtime_failure(
            "exhaustive Body-IR match had no matching arm".to_string(),
            span,
        ))
    }

    /// Materialize arm-local bindings using their lowered ownership facts.
    fn bind_pattern_values(
        &mut self,
        bindings: Vec<(PatternBinding, ReplacementValue)>,
        span: HirSourceSpan,
    ) -> Result<(), ReplacementExecutionError> {
        for (binding, value) in bindings {
            if matches!(binding.fact, OwnershipFact::Move | OwnershipFact::Unknown) {
                return Err(unsupported(
                    "match binding with an unsupported move or unknown ownership fact",
                    span,
                ));
            }
            if matches!(binding.fact, OwnershipFact::Copy) && !value.is_copy_shaped() {
                return Err(unsupported("copy match binding of a non-copy value", span));
            }
            self.ownership_reads.push(OwnershipRead {
                span,
                fact: binding.fact,
                last_use: binding.last_use,
            });
            self.locals.insert(binding.local, value);
        }
        Ok(())
    }

    /// Check a Result payload against the checked type retained by its construction rvalue.
    ///
    /// Named values are accepted only after their runtime identity re-resolves to a declaration of the same source
    /// name in this module. This prevents malformed Body IR from placing an arbitrary enum/model carrier in a
    /// Result solely because both happen to be `Named` types.
    fn value_matches_direct_result_type(&self, value: &ReplacementValue, ty: &IncanType) -> bool {
        match (value, ty) {
            (ReplacementValue::Int(_), IncanType::Primitive(IncanPrimitiveType::Int))
            | (ReplacementValue::Bool(_), IncanType::Primitive(IncanPrimitiveType::Bool))
            | (ReplacementValue::Str(_), IncanType::Primitive(IncanPrimitiveType::Str))
            | (ReplacementValue::Unit, IncanType::Primitive(IncanPrimitiveType::Unit)) => true,
            (ReplacementValue::Tuple(values), IncanType::Tuple(types)) => {
                if values.len() != types.len() {
                    return false;
                }
                values
                    .iter()
                    .zip(types)
                    .all(|(value, ty)| self.value_matches_direct_result_type(value, ty))
            }
            (ReplacementValue::Tuple(values), IncanType::Generic { base, args })
                if collections::from_str(base) == Some(CollectionTypeId::Tuple) =>
            {
                if values.len() != args.len() {
                    return false;
                }
                values
                    .iter()
                    .zip(args)
                    .all(|(value, ty)| self.value_matches_direct_result_type(value, ty))
            }
            (ReplacementValue::List { elements, .. }, IncanType::Generic { base, args })
                if collections::from_str(base) == Some(CollectionTypeId::List) =>
            {
                let [element_type] = args.as_slice() else {
                    return false;
                };
                elements
                    .iter()
                    .all(|element| self.value_matches_direct_result_type(element, element_type))
            }
            (
                ReplacementValue::Nominal {
                    direct_declaration_id, ..
                },
                IncanType::Named(expected),
            ) => self.module.nominal_declarations.iter().any(|declaration| {
                declaration.direct_declaration_id == *direct_declaration_id && declaration.name == *expected
            }),
            (
                ReplacementValue::FieldlessEnum {
                    enum_declaration_id, ..
                },
                IncanType::Named(expected),
            ) => self.module.fieldless_enum_declarations.iter().any(|declaration| {
                declaration.direct_declaration_id == *enum_declaration_id && declaration.name == *expected
            }),
            (
                ReplacementValue::ValueEnum {
                    enum_declaration_id, ..
                },
                IncanType::Named(expected),
            ) => self.module.value_enum_declarations.iter().any(|declaration| {
                declaration.direct_declaration_id == *enum_declaration_id && declaration.name == *expected
            }),
            _ => false,
        }
    }

    /// Match only the identity-retaining direct pattern vocabulary, returning selected arm bindings without mutating
    /// the executor until an arm is known to match.
    fn match_pattern(
        &self,
        pattern: &Pattern,
        value: &ReplacementValue,
        span: HirSourceSpan,
    ) -> Result<Option<Vec<(PatternBinding, ReplacementValue)>>, ReplacementExecutionError> {
        match pattern {
            Pattern::Wildcard => Ok(Some(Vec::new())),
            Pattern::Var(binding) => Ok(Some(vec![(binding.clone(), value.clone())])),
            Pattern::Literal(constant) => {
                let constant = direct_pattern_constant(constant, span)?;
                Ok((constant == *value).then_some(Vec::new()))
            }
            Pattern::Tuple(patterns) => {
                let ReplacementValue::Tuple(values) = value else {
                    return Ok(None);
                };
                if patterns.len() != values.len() {
                    return Ok(None);
                }
                let mut bindings = Vec::new();
                for (pattern, value) in patterns.iter().zip(values) {
                    let Some(mut nested) = self.match_pattern(pattern, value, span)? else {
                        return Ok(None);
                    };
                    bindings.append(&mut nested);
                }
                Ok(Some(bindings))
            }
            Pattern::Nominal { target, fields } => self.match_nominal_pattern(target, fields, value, span),
            Pattern::FieldlessEnumVariant(target) => {
                let (declaration, variant) = self.local_fieldless_enum_variant_by_ids(
                    &target.enum_declaration_id,
                    &target.variant_declaration_id,
                    span,
                )?;
                if declaration.name != target.enum_name
                    || declaration.canonical != target.enum_canonical
                    || variant.name != target.variant_name
                    || variant.canonical != target.variant_canonical
                {
                    return Err(unsupported(
                        "fieldless-enum pattern disagrees with its source-local declaration identity",
                        span,
                    ));
                }
                let ReplacementValue::FieldlessEnum {
                    enum_declaration_id,
                    variant_declaration_id,
                } = value
                else {
                    return Ok(None);
                };
                Ok((enum_declaration_id == &declaration.direct_declaration_id
                    && variant_declaration_id == &variant.direct_declaration_id)
                    .then_some(Vec::new()))
            }
            Pattern::Result { variant, fields } => {
                let [payload_pattern] = fields.as_slice() else {
                    return Err(unsupported("Result pattern without one payload", span));
                };
                let ReplacementValue::Result { kind, payload, .. } = value else {
                    return Ok(None);
                };
                if kind != variant {
                    return Ok(None);
                }
                self.match_pattern(payload_pattern, payload, span)
            }
            Pattern::Or(alternatives) => {
                for alternative in alternatives {
                    if let Some(bindings) = self.match_pattern(alternative, value, span)? {
                        return Ok(Some(bindings));
                    }
                }
                Ok(None)
            }
            Pattern::Struct { canonical: None, .. } | Pattern::Enum { canonical: None, .. } => Err(unsupported(
                "match pattern without an exact direct target identity",
                span,
            )),
            Pattern::Struct { canonical: Some(_), .. } | Pattern::Enum { canonical: Some(_), .. } => Err(unsupported(
                "match pattern without an admitted direct target layout",
                span,
            )),
        }
    }

    /// Match an identity-selected source-local plain model and its canonical named fields.
    fn match_nominal_pattern(
        &self,
        target: &NominalPatternTarget,
        patterns: &[(String, Pattern)],
        value: &ReplacementValue,
        span: HirSourceSpan,
    ) -> Result<Option<Vec<(PatternBinding, ReplacementValue)>>, ReplacementExecutionError> {
        let declaration = self.local_nominal_pattern_declaration(target, span)?;
        let ReplacementValue::Nominal {
            direct_declaration_id,
            fields,
        } = value
        else {
            return Ok(None);
        };
        if *direct_declaration_id != declaration.direct_declaration_id {
            return Ok(None);
        }
        if fields.len() != declaration.fields.len()
            || declaration
                .fields
                .iter()
                .zip(fields)
                .any(|(declared, (stored, _))| declared != stored)
        {
            return Err(unsupported(
                "nominal match with a mismatched canonical field layout",
                span,
            ));
        }
        let mut pattern_fields = BTreeSet::new();
        let mut bindings = Vec::new();
        for (field, pattern) in patterns {
            if !pattern_fields.insert(field) {
                return Err(unsupported("nominal match pattern with duplicate field", span));
            }
            let Some((_, stored)) = fields.iter().find(|(stored, _)| stored == field) else {
                return Err(unsupported(
                    format!("nominal match pattern for unknown field `{field}`"),
                    span,
                ));
            };
            let Some(mut nested) = self.match_pattern(pattern, stored, span)? else {
                return Ok(None);
            };
            bindings.append(&mut nested);
        }
        Ok(Some(bindings))
    }

    /// Materialize a fully supplied source-local plain model from checked constructor binding facts.
    ///
    /// `ArgumentBinding` orders operand storage by declaration slot but records source written positions separately.
    /// Evaluating through that written order is essential: evaluating the surrounding operand vector directly would
    /// reverse source effects for an out-of-order named constructor call.
    fn evaluate_nominal_constructor(
        &mut self,
        target: &ConstructorTarget,
        operands: &[&Operand],
        span: HirSourceSpan,
    ) -> Result<ReplacementValue, ReplacementExecutionError> {
        let declaration = self.local_nominal_declaration(target, span)?;
        let ArgumentBinding::Resolved {
            arguments,
            defaulted_slots,
        } = &target.binding
        else {
            return Err(unsupported(
                format!("constructor `{}` with unresolved field binding", target.name),
                span,
            ));
        };
        if !defaulted_slots.is_empty() {
            return Err(unsupported(
                format!("constructor `{}` with an omitted field default", target.name),
                span,
            ));
        }
        if operands.len() != declaration.fields.len() || arguments.len() != operands.len() {
            return Err(unsupported(
                format!("constructor `{}` with incomplete field binding", target.name),
                span,
            ));
        }

        let mut field_values = vec![None; declaration.fields.len()];
        let mut argument_indices = (0..arguments.len()).collect::<Vec<_>>();
        argument_indices.sort_by_key(|index| arguments[*index].written_position);
        for index in argument_indices {
            let argument = arguments[index];
            if argument.slot >= field_values.len()
                || field_values[argument.slot].is_some()
                || arguments
                    .iter()
                    .filter(|other| other.written_position == argument.written_position)
                    .count()
                    != 1
            {
                return Err(unsupported(
                    format!("constructor `{}` with invalid field binding", target.name),
                    span,
                ));
            }
            let value = self.evaluate_operand(operands[index], span)?;
            if !value.is_direct_structural() {
                return Err(unsupported(
                    format!("constructor `{}` with a non-structural field value", target.name),
                    span,
                ));
            }
            field_values[argument.slot] = Some(value);
        }
        let fields = declaration
            .fields
            .iter()
            .cloned()
            .zip(field_values)
            .map(|(field, value)| match value {
                Some(value) => Ok((field, value)),
                None => Err(unsupported(
                    format!("constructor `{}` omitted field `{field}`", target.name),
                    span,
                )),
            })
            .collect::<Result<Vec<_>, _>>()?;
        self.record_frame_evidence(format!(
            "executed nominal constructor name={} id={} fields=[{}] call_span={}..{}",
            declaration.name,
            declaration.direct_declaration_id,
            declaration.fields.join(", "),
            span.start,
            span.end
        ));
        Ok(ReplacementValue::Nominal {
            direct_declaration_id: declaration.direct_declaration_id,
            fields,
        })
    }

    /// Materialize one exact source-local fieldless normal-enum member without reducing it to a source spelling.
    ///
    /// The resulting carrier stores only validated declaration identities. It has no payload and can reach a scalar
    /// result solely through same-enum equality or inequality, preserving a narrow direct-execution boundary.
    fn evaluate_fieldless_enum_variant(
        &mut self,
        target: &FieldlessEnumVariantTarget,
        span: HirSourceSpan,
    ) -> Result<ReplacementValue, ReplacementExecutionError> {
        let (declaration, variant) = self.local_fieldless_enum_variant_by_ids(
            &target.enum_declaration_id,
            &target.variant_declaration_id,
            span,
        )?;
        if declaration.name != target.enum_name
            || declaration.canonical != target.enum_canonical
            || variant.name != target.variant_name
            || variant.canonical != target.variant_canonical
        {
            return Err(unsupported(
                format!(
                    "fieldless-enum member `{}::{}` disagrees with its source-local declaration identity",
                    target.enum_name, target.variant_name
                ),
                span,
            ));
        }
        self.record_frame_evidence(format!(
            "executed fieldless-enum variant name={}::{} enum_id={} variant_id={}",
            declaration.name, variant.name, declaration.direct_declaration_id, variant.direct_declaration_id
        ));
        Ok(ReplacementValue::FieldlessEnum {
            enum_declaration_id: declaration.direct_declaration_id,
            variant_declaration_id: variant.direct_declaration_id,
        })
    }

    /// Resolve one normal-enum/member pair solely through this module's retained fieldless-enum registry.
    fn local_fieldless_enum_variant_by_ids(
        &self,
        enum_declaration_id: &CompilerNodeId,
        variant_declaration_id: &CompilerNodeId,
        span: HirSourceSpan,
    ) -> Result<(FieldlessEnumDeclaration, FieldlessEnumVariantDeclaration), ReplacementExecutionError> {
        if !is_module_span_declaration_id(&self.module, enum_declaration_id)
            || !is_module_span_declaration_id(&self.module, variant_declaration_id)
        {
            return Err(unsupported(
                "fieldless-enum member declaration identity is not scoped to this Body-IR module",
                span,
            ));
        }
        let declarations = self
            .module
            .fieldless_enum_declarations
            .iter()
            .filter(|declaration| declaration.direct_declaration_id == *enum_declaration_id)
            .collect::<Vec<_>>();
        let [declaration] = declarations.as_slice() else {
            return Err(unsupported(
                "fieldless-enum member targets a declaration outside this Body-IR module",
                span,
            ));
        };
        if !valid_local_fieldless_enum_declaration(&self.module, declaration) {
            return Err(unsupported(
                "fieldless-enum registry lacks exact canonical owner/member identities",
                span,
            ));
        }
        let canonical_names = declaration
            .variants
            .iter()
            .map(|variant| variant.name.as_str())
            .collect::<BTreeSet<_>>();
        if canonical_names.len() != declaration.variants.len() {
            return Err(unsupported(
                format!(
                    "fieldless enum `{}` has a duplicate canonical member layout",
                    declaration.name
                ),
                span,
            ));
        }
        let variants = declaration
            .variants
            .iter()
            .filter(|variant| variant.direct_declaration_id == *variant_declaration_id)
            .collect::<Vec<_>>();
        let [variant] = variants.as_slice() else {
            return Err(unsupported(
                format!(
                    "fieldless enum `{}` has no retained selected member identity",
                    declaration.name
                ),
                span,
            ));
        };
        Ok(((*declaration).clone(), (*variant).clone()))
    }

    /// Compare two identity-validated fieldless normal-enum carriers without admitting a general enum operation.
    fn fieldless_enum_values_equal(
        &self,
        left_enum_declaration_id: &CompilerNodeId,
        left_variant_declaration_id: &CompilerNodeId,
        right_enum_declaration_id: &CompilerNodeId,
        right_variant_declaration_id: &CompilerNodeId,
        span: HirSourceSpan,
    ) -> Result<bool, ReplacementExecutionError> {
        let (left_declaration, left_variant) =
            self.local_fieldless_enum_variant_by_ids(left_enum_declaration_id, left_variant_declaration_id, span)?;
        let (right_declaration, right_variant) =
            self.local_fieldless_enum_variant_by_ids(right_enum_declaration_id, right_variant_declaration_id, span)?;
        if left_declaration.direct_declaration_id != right_declaration.direct_declaration_id {
            return Err(unsupported(
                "fieldless-enum comparison across distinct source-local enum declarations",
                span,
            ));
        }
        Ok(left_variant.direct_declaration_id == right_variant.direct_declaration_id)
    }

    /// Materialize one exact source-local RFC 032 value-enum member without reducing it to a name or raw scalar.
    ///
    /// The scalar stays in the verified declaration registry until the separately admitted generated `.value()`
    /// call asks for it. Keeping the carrier identity-bearing prevents ordinary enum/member spellings, imported
    /// lookalikes, and malformed Body IR from acquiring direct execution semantics.
    fn evaluate_value_enum_variant(
        &mut self,
        target: &ValueEnumVariantTarget,
        span: HirSourceSpan,
    ) -> Result<ReplacementValue, ReplacementExecutionError> {
        let (declaration, variant) =
            self.local_value_enum_variant_by_ids(&target.enum_declaration_id, &target.variant_declaration_id, span)?;
        if declaration.name != target.enum_name
            || declaration.canonical != target.enum_canonical
            || variant.name != target.variant_name
            || variant.canonical != target.variant_canonical
        {
            return Err(unsupported(
                format!(
                    "value-enum member `{}::{}` disagrees with its source-local declaration identity",
                    target.enum_name, target.variant_name
                ),
                span,
            ));
        }
        let raw_value = value_enum_scalar_value(&declaration, &variant, span)?;
        self.record_frame_evidence(format!(
            "executed value-enum variant name={}::{} enum_id={} variant_id={} raw={}",
            declaration.name,
            variant.name,
            declaration.direct_declaration_id,
            variant.direct_declaration_id,
            raw_value.observable_text()
        ));
        Ok(ReplacementValue::ValueEnum {
            enum_declaration_id: declaration.direct_declaration_id,
            variant_declaration_id: variant.direct_declaration_id,
        })
    }

    /// Extract the backing scalar through the only admitted compiler-provided value-enum method.
    ///
    /// No ordinary method dispatch occurs here: the call is valid only for a runtime carrier already materialized
    /// by [`Self::evaluate_value_enum_variant`], and the raw literal is resolved solely from the same Body-IR
    /// declaration registry after membership verification.
    fn extract_value_enum_scalar(
        &mut self,
        target: &incan_semantics_core::body_ir::MethodTarget,
        args: &[&Operand],
        span: HirSourceSpan,
    ) -> Result<ReplacementValue, ReplacementExecutionError> {
        if !target.type_args.is_empty() {
            return Err(unsupported("value-enum `.value()` with explicit type arguments", span));
        }
        let [receiver] = args else {
            return Err(unsupported("value-enum `.value()` call arity", span));
        };
        let value = self.evaluate_operand(receiver, span)?;
        let ReplacementValue::ValueEnum {
            enum_declaration_id,
            variant_declaration_id,
        } = value
        else {
            return Err(unsupported("`.value()` on a non-value-enum receiver", span));
        };
        let canonical = target
            .canonical
            .as_ref()
            .ok_or_else(|| unsupported("value-enum `.value()` without a canonical method target", span))?;
        let canonical_owner = direct_declaration_id_for_canonical(
            &self.module,
            canonical,
            SymbolNamespace::Member,
            SemanticSourceTargetKind::Method,
        )
        .ok_or_else(|| unsupported("value-enum `.value()` with a foreign method target", span))?;
        if canonical_owner != enum_declaration_id {
            return Err(unsupported(
                "value-enum `.value()` target does not belong to its runtime receiver",
                span,
            ));
        }
        let (declaration, variant) =
            self.local_value_enum_variant_by_ids(&enum_declaration_id, &variant_declaration_id, span)?;
        let raw_value = value_enum_scalar_value(&declaration, &variant, span)?;
        self.record_frame_evidence(format!(
            "extracted value-enum scalar name={}::{} enum_id={} variant_id={}",
            declaration.name, variant.name, declaration.direct_declaration_id, variant.direct_declaration_id
        ));
        Ok(raw_value)
    }

    /// Resolve one enum/member pair through this module's retained source-local value-enum registry.
    fn local_value_enum_variant_by_ids(
        &self,
        enum_declaration_id: &CompilerNodeId,
        variant_declaration_id: &CompilerNodeId,
        span: HirSourceSpan,
    ) -> Result<(ValueEnumDeclaration, ValueEnumVariantDeclaration), ReplacementExecutionError> {
        if !is_module_span_declaration_id(&self.module, enum_declaration_id)
            || !is_module_span_declaration_id(&self.module, variant_declaration_id)
        {
            return Err(unsupported(
                "value-enum member declaration identity is not scoped to this Body-IR module",
                span,
            ));
        }
        let declarations = self
            .module
            .value_enum_declarations
            .iter()
            .filter(|declaration| declaration.direct_declaration_id == *enum_declaration_id)
            .collect::<Vec<_>>();
        let [declaration] = declarations.as_slice() else {
            return Err(unsupported(
                "value-enum member targets a declaration outside this Body-IR module",
                span,
            ));
        };
        if !valid_local_value_enum_declaration(&self.module, declaration) {
            return Err(unsupported(
                "value-enum registry lacks exact canonical owner/member identities",
                span,
            ));
        }
        let canonical_names = declaration
            .variants
            .iter()
            .map(|variant| variant.name.as_str())
            .collect::<BTreeSet<_>>();
        if canonical_names.len() != declaration.variants.len() {
            return Err(unsupported(
                format!(
                    "value enum `{}` has a duplicate canonical member layout",
                    declaration.name
                ),
                span,
            ));
        }
        let variants = declaration
            .variants
            .iter()
            .filter(|variant| variant.direct_declaration_id == *variant_declaration_id)
            .collect::<Vec<_>>();
        let [variant] = variants.as_slice() else {
            return Err(unsupported(
                format!(
                    "value enum `{}` has no retained selected member identity",
                    declaration.name
                ),
                span,
            ));
        };
        Ok(((*declaration).clone(), (*variant).clone()))
    }

    /// Resolve one constructor identity solely through this module's retained plain-model declaration registry.
    fn local_nominal_declaration(
        &self,
        target: &ConstructorTarget,
        span: HirSourceSpan,
    ) -> Result<NominalDeclaration, ReplacementExecutionError> {
        let direct_declaration_id = target.direct_declaration_id.as_ref().ok_or_else(|| {
            unsupported(
                format!(
                    "constructor `{}` without a source-local declaration identity",
                    target.name
                ),
                span,
            )
        })?;
        let canonical = target.canonical.as_ref().ok_or_else(|| {
            unsupported(
                format!("constructor `{}` without a canonical declaration target", target.name),
                span,
            )
        })?;
        let canonical_declaration_id = direct_declaration_id_for_canonical(
            &self.module,
            canonical,
            SymbolNamespace::OrdinaryLexical,
            SemanticSourceTargetKind::Model,
        )
        .ok_or_else(|| unsupported("constructor canonical target is not a source-local model", span))?;
        if canonical_declaration_id != *direct_declaration_id {
            return Err(unsupported(
                "constructor canonical target disagrees with its physical Body-IR declaration",
                span,
            ));
        }
        let declaration = self
            .module
            .nominal_declarations
            .iter()
            .find(|declaration| declaration.direct_declaration_id == *direct_declaration_id)
            .cloned()
            .ok_or_else(|| {
                unsupported(
                    format!(
                        "constructor `{}` targets a declaration outside this Body-IR module",
                        target.name
                    ),
                    span,
                )
            })?;
        if declaration.canonical != *canonical || !valid_local_nominal_declaration(&self.module, &declaration) {
            return Err(unsupported(
                "constructor canonical target disagrees with the retained declaration identity",
                span,
            ));
        }
        let canonical_field_layout = target.canonical_field_layout.as_deref().ok_or_else(|| {
            unsupported(
                format!("constructor `{}` without a checked canonical field layout", target.name),
                span,
            )
        })?;
        if declaration.fields != canonical_field_layout {
            return Err(unsupported(
                "canonical field layout disagrees with checked constructor facts",
                span,
            ));
        }
        if declaration.type_parameter_count != 0 {
            return Err(unsupported(
                format!("generic model constructor `{}`", target.name),
                span,
            ));
        }
        let fields = declaration.fields.iter().collect::<BTreeSet<_>>();
        if fields.len() != declaration.fields.len()
            || declaration.field_identities.len() != declaration.fields.len()
            || declaration.field_identities.iter().collect::<BTreeSet<_>>().len() != declaration.field_identities.len()
        {
            return Err(unsupported(
                format!("constructor `{}` has a duplicate canonical field layout", target.name),
                span,
            ));
        }
        Ok(declaration)
    }

    /// Resolve a nominal pattern exclusively through its retained source-local declaration identity.
    fn local_nominal_pattern_declaration(
        &self,
        target: &NominalPatternTarget,
        span: HirSourceSpan,
    ) -> Result<NominalDeclaration, ReplacementExecutionError> {
        if !is_module_span_declaration_id(&self.module, &target.direct_declaration_id) {
            return Err(unsupported(
                "nominal match pattern declaration identity is not scoped to this Body-IR module",
                span,
            ));
        }
        let declarations = self
            .module
            .nominal_declarations
            .iter()
            .filter(|declaration| declaration.direct_declaration_id == target.direct_declaration_id)
            .collect::<Vec<_>>();
        let [declaration] = declarations.as_slice() else {
            return Err(unsupported(
                "nominal match pattern targets a declaration outside this Body-IR module",
                span,
            ));
        };
        if declaration.canonical != target.canonical
            || !valid_local_nominal_declaration(&self.module, declaration)
            || declaration.type_parameter_count != 0
        {
            return Err(unsupported(
                "nominal match pattern disagrees with its source-local declaration identity",
                span,
            ));
        }
        if declaration.fields.iter().collect::<BTreeSet<_>>().len() != declaration.fields.len() {
            return Err(unsupported(
                "nominal match pattern has a duplicate canonical field layout",
                span,
            ));
        }
        Ok((*declaration).clone())
    }

    /// Execute the explicit Result propagation primitive only when lowering retained an exact same-error route.
    fn execute_try_propagate(
        &mut self,
        destination: &Place,
        operand: &Operand,
        error_routing: &TryErrorRouting,
        span: HirSourceSpan,
    ) -> Result<Flow, ReplacementExecutionError> {
        let TryErrorRouting::SameType { error_type } = error_routing else {
            return Err(unsupported(
                match error_routing {
                    TryErrorRouting::ConversionRequired { .. } => "cross-error-type try propagation",
                    TryErrorRouting::Unresolved => "try propagation without a resolved Result error route",
                    TryErrorRouting::SameType { .. } => unreachable!(),
                },
                span,
            ));
        };
        if !is_direct_result_payload_type(error_type) {
            return Err(unsupported(
                "try propagation with an unsupported Result error payload",
                span,
            ));
        }
        let destination = bare_local(destination, span)?;
        let value = self.evaluate_operand(operand, span)?;
        let ReplacementValue::Result {
            kind,
            payload,
            ok_type,
            error_type: carrier_error_type,
        } = value
        else {
            return Err(unsupported("try propagation using a non-Result carrier", span));
        };
        if carrier_error_type != *error_type {
            return Err(unsupported(
                "try propagation whose Result carrier disagrees with the retained same-error route",
                span,
            ));
        }
        match kind {
            ResultVariantKind::Ok => {
                self.assign_local(destination, *payload, span)?;
                self.record_frame_evidence(format!(
                    "executed Result try route=ok span={}..{}",
                    span.start, span.end
                ));
                Ok(Flow::Next)
            }
            ResultVariantKind::Err => {
                self.record_frame_evidence(format!(
                    "executed Result try route=err span={}..{}",
                    span.start, span.end
                ));
                Ok(Flow::Return(
                    Some(ReplacementValue::Result {
                        kind: ResultVariantKind::Err,
                        payload,
                        ok_type,
                        error_type: carrier_error_type,
                    }),
                    span,
                ))
            }
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
            (
                BinOp::Eq,
                ReplacementValue::FieldlessEnum {
                    enum_declaration_id: left_enum_declaration_id,
                    variant_declaration_id: left_variant_declaration_id,
                },
                ReplacementValue::FieldlessEnum {
                    enum_declaration_id: right_enum_declaration_id,
                    variant_declaration_id: right_variant_declaration_id,
                },
            ) => Ok(ReplacementValue::Bool(self.fieldless_enum_values_equal(
                &left_enum_declaration_id,
                &left_variant_declaration_id,
                &right_enum_declaration_id,
                &right_variant_declaration_id,
                span,
            )?)),
            (
                BinOp::Ne,
                ReplacementValue::FieldlessEnum {
                    enum_declaration_id: left_enum_declaration_id,
                    variant_declaration_id: left_variant_declaration_id,
                },
                ReplacementValue::FieldlessEnum {
                    enum_declaration_id: right_enum_declaration_id,
                    variant_declaration_id: right_variant_declaration_id,
                },
            ) => Ok(ReplacementValue::Bool(!self.fieldless_enum_values_equal(
                &left_enum_declaration_id,
                &left_variant_declaration_id,
                &right_enum_declaration_id,
                &right_variant_declaration_id,
                span,
            )?)),
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

    /// Execute one compiler-selected string helper through the existing shared string semantics.
    ///
    /// Operands are receiver-first and evaluated in their retained order. Split creates a fresh structural list;
    /// its absent separator and empty separator deliberately retain the semantic core's distinct behavior.
    fn execute_string_helper(
        &mut self,
        helper: HelperOp,
        args: &[&Operand],
        span: HirSourceSpan,
    ) -> Result<ReplacementValue, ReplacementExecutionError> {
        let value = match (helper, args) {
            (
                comparison @ (HelperOp::StrEq
                | HelperOp::StrNe
                | HelperOp::StrLt
                | HelperOp::StrLe
                | HelperOp::StrGt
                | HelperOp::StrGe),
                [left, right],
            ) => {
                let left = self.evaluate_operand(left, span)?.into_string(span)?;
                let right = self.evaluate_operand(right, span)?.into_string(span)?;
                let ordering = incan_core::strings::str_cmp(&left, &right);
                let matches = match comparison {
                    HelperOp::StrEq => ordering.is_eq(),
                    HelperOp::StrNe => ordering.is_ne(),
                    HelperOp::StrLt => ordering.is_lt(),
                    HelperOp::StrLe => ordering.is_le(),
                    HelperOp::StrGt => ordering.is_gt(),
                    HelperOp::StrGe => ordering.is_ge(),
                    _ => return Err(unsupported("non-comparison string helper", span)),
                };
                ReplacementValue::Bool(matches)
            }
            (HelperOp::StrUpper, [receiver]) => {
                let receiver = self.evaluate_operand(receiver, span)?.into_string(span)?;
                ReplacementValue::Str(incan_core::strings::str_upper(&receiver))
            }
            (HelperOp::StrLower, [receiver]) => {
                let receiver = self.evaluate_operand(receiver, span)?.into_string(span)?;
                ReplacementValue::Str(incan_core::strings::str_lower(&receiver))
            }
            (HelperOp::StrStrip, [receiver]) => {
                let receiver = self.evaluate_operand(receiver, span)?.into_string(span)?;
                ReplacementValue::Str(incan_core::strings::str_strip(&receiver))
            }
            (HelperOp::StrLen, [receiver]) => {
                let receiver = self.evaluate_operand(receiver, span)?.into_string(span)?;
                ReplacementValue::Int(incan_core::strings::str_len(&receiver))
            }
            (HelperOp::StrReplace, [receiver, from, to]) => {
                let receiver = self.evaluate_operand(receiver, span)?.into_string(span)?;
                let from = self.evaluate_operand(from, span)?.into_string(span)?;
                let to = self.evaluate_operand(to, span)?.into_string(span)?;
                ReplacementValue::Str(incan_core::strings::str_replace(&receiver, &from, &to))
            }
            (HelperOp::StrJoin, [separator, items]) => {
                let separator = self.evaluate_operand(separator, span)?.into_string(span)?;
                let items = self.evaluate_list_elements(items, span)?;
                let items = items
                    .into_iter()
                    .map(|item| item.into_string(span))
                    .collect::<Result<Vec<_>, _>>()?;
                ReplacementValue::Str(incan_core::strings::str_join(&separator, &items))
            }
            (HelperOp::StrSplit, [receiver, rest @ ..]) if rest.len() <= 1 => {
                let receiver = self.evaluate_operand(receiver, span)?.into_string(span)?;
                let separator = rest
                    .first()
                    .map(|separator| self.evaluate_operand(separator, span)?.into_string(span))
                    .transpose()?;
                ReplacementValue::List {
                    elements: incan_core::strings::str_split(&receiver, separator.as_deref())
                        .into_iter()
                        .map(ReplacementValue::Str)
                        .collect(),
                    next: 0,
                }
            }
            (HelperOp::StrContains, [haystack, needle]) => {
                let haystack = self.evaluate_operand(haystack, span)?.into_string(span)?;
                let needle = self.evaluate_operand(needle, span)?.into_string(span)?;
                ReplacementValue::Bool(incan_core::strings::str_contains(&haystack, &needle))
            }
            _ => {
                return Err(unsupported(
                    format!("string helper {} call arity", helper.as_str()),
                    span,
                ));
            }
        };
        Ok(value)
    }

    /// Evaluate an operand that must be a list, returning its elements.
    ///
    /// Accepts a collected generator alongside a list because both carry the same materialized element vector, and
    /// the cursor each holds is traversal state that a concatenation or membership test has no business reading.
    fn evaluate_list_elements(
        &mut self,
        operand: &Operand,
        span: HirSourceSpan,
    ) -> Result<Vec<ReplacementValue>, ReplacementExecutionError> {
        match self.evaluate_operand(operand, span)? {
            ReplacementValue::List { elements, .. } | ReplacementValue::CollectedGenerator { elements, .. } => {
                Ok(elements)
            }
            other => Err(unsupported(format!("list operation over {}", value_kind(&other)), span)),
        }
    }

    /// Read one constant or local place while applying its recorded ownership decision.
    fn evaluate_operand(
        &mut self,
        operand: &Operand,
        span: HirSourceSpan,
    ) -> Result<ReplacementValue, ReplacementExecutionError> {
        match operand {
            Operand::Constant(constant) => constant_value(constant, span),
            Operand::Place(place_operand) => {
                let local = local_root(&place_operand.place, span)?;
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

    /// Move a complete local while refusing unrepresented partial moves through a projected place.
    fn read_moved_place(
        &mut self,
        place: &Place,
        span: HirSourceSpan,
    ) -> Result<ReplacementValue, ReplacementExecutionError> {
        if !place.projection.is_empty() {
            return Err(unsupported(
                "move through a place projection outside the direct replacement profile",
                span,
            ));
        }
        self.locals
            .remove(&bare_local(place, span)?)
            .ok_or_else(|| runtime_failure("read of a moved or dropped local".to_string(), span))
    }

    /// Apply one source-local tuple/model field or list index projection while retaining the original source span.
    fn project_place(
        &mut self,
        value: ReplacementValue,
        place: &Place,
        span: HirSourceSpan,
    ) -> Result<ReplacementValue, ReplacementExecutionError> {
        match place.projection.as_slice() {
            [] => Ok(value),
            [PlaceElem::Field { name, canonical: None }] if name.parse::<usize>().is_ok() => {
                project_tuple_field(value, place, span)
            }
            [
                PlaceElem::Field {
                    name,
                    canonical: Some(canonical),
                },
            ] => self.project_nominal_field(value, name, canonical, span),
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

    /// Read a canonical field from a nominal value after revalidating its retained declaration layout.
    fn project_nominal_field(
        &self,
        value: ReplacementValue,
        field: &str,
        canonical: &incan_semantics_core::CanonicalSymbolId,
        span: HirSourceSpan,
    ) -> Result<ReplacementValue, ReplacementExecutionError> {
        let ReplacementValue::Nominal {
            direct_declaration_id,
            fields,
        } = value
        else {
            return Err(unsupported(
                format!("named field projection `.{field}` using a non-nominal value"),
                span,
            ));
        };
        let declaration = self
            .module
            .nominal_declarations
            .iter()
            .find(|declaration| declaration.direct_declaration_id == direct_declaration_id)
            .ok_or_else(|| {
                unsupported(
                    "nominal field projection with an unavailable declaration identity",
                    span,
                )
            })?;
        if !valid_local_nominal_declaration(&self.module, declaration)
            || declaration.fields.len() != fields.len()
            || declaration
                .fields
                .iter()
                .zip(&fields)
                .any(|(declared, (stored, _))| declared != stored)
        {
            return Err(unsupported(
                "nominal field projection with a mismatched canonical field layout",
                span,
            ));
        }
        let Some(slot) = declaration
            .field_identities
            .iter()
            .position(|identity| identity == canonical)
        else {
            return Err(unsupported(
                "nominal field projection without the retained canonical member identity",
                span,
            ));
        };
        fields.into_iter().nth(slot).map(|(_, value)| value).ok_or_else(|| {
            unsupported(
                format!("named field projection `.{field}` outside the source-local nominal layout"),
                span,
            )
        })
    }

    /// Assign a complete local or one source-local list element without permitting nested writes.
    fn assign_place(
        &mut self,
        place: &Place,
        value: ReplacementValue,
        span: HirSourceSpan,
    ) -> Result<(), ReplacementExecutionError> {
        match place.projection.as_slice() {
            [] => self.assign_local(bare_local(place, span)?, value, span),
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
                    .get_mut(&local_root(place, span)?)
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
        matches!(
            self,
            Self::Int(_)
                | Self::Bool(_)
                | Self::Float(_)
                | Self::Numeric(_)
                | Self::Unit
                | Self::FieldlessEnum { .. }
                | Self::Task(_)
        )
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

    /// Return whether this value can be carried in the intentionally data-only direct Result profile.
    fn is_direct_result_payload(&self) -> bool {
        self.is_direct_structural()
            || matches!(
                self,
                Self::Nominal { .. } | Self::FieldlessEnum { .. } | Self::ValueEnum { .. }
            )
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
    Return(Option<ReplacementValue>, HirSourceSpan),
}

/// Return one bare local id, refusing fields, indexes, and slices in the first profile.
fn bare_local(place: &Place, span: HirSourceSpan) -> Result<LocalId, ReplacementExecutionError> {
    let local = local_root(place, span)?;
    if place.projection.is_empty() {
        Ok(local)
    } else {
        Err(unsupported("place projection", span))
    }
}

/// Return the local storage root of a place while leaving its already-admitted projection for the caller to apply.
fn local_root(place: &Place, span: HirSourceSpan) -> Result<LocalId, ReplacementExecutionError> {
    let local = match &place.root {
        incan_semantics_core::body_ir::PlaceRoot::Local(local) => *local,
        incan_semantics_core::body_ir::PlaceRoot::Global(global) => Err(unsupported(
            format!(
                "canonical global `{}` is outside the direct replacement value-state profile",
                global.identity.render_compact()
            ),
            span,
        ))?,
    };
    Ok(local)
}

/// Project one numeric source-local tuple field while retaining the statement's original source authority on refusal.
fn project_tuple_field(
    value: ReplacementValue,
    place: &Place,
    span: HirSourceSpan,
) -> Result<ReplacementValue, ReplacementExecutionError> {
    let [
        PlaceElem::Field {
            name: field,
            canonical: None,
        },
    ] = place.projection.as_slice()
    else {
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

/// Return the fixed operands of an element list, or `None` when any element splices.
///
/// #1159 made Body IR's aggregate and call element lists variable-arity: an element may splice a source whose
/// length is only known at runtime. Every profile check and every executor path here assumes a fixed, countable
/// arity -- slice patterns, `len() == 2` guards, positional `range` arguments -- so each one calls this first and
/// refuses by name rather than counting a spread as a single value, which would validate and then execute the
/// wrong arity. Executing a spliced element is the execution owner's work, not this boundary's.
fn fixed_operands(elements: &[ArgumentElement]) -> Option<Vec<&Operand>> {
    elements.iter().map(ArgumentElement::as_one).collect()
}

/// Construct a source-span-preserving unsupported-profile error.
fn unsupported(description: impl Into<String>, span: HirSourceSpan) -> ReplacementExecutionError {
    ReplacementExecutionError::Unsupported {
        description: description.into(),
        span,
        span_start: span.start,
        span_end: span.end,
        module_id: None,
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
fn ensure_scalar_result(
    value: &ReplacementValue,
    return_type: &IncanType,
    span: HirSourceSpan,
) -> Result<(), ReplacementExecutionError> {
    let scalar = matches!(
        value,
        ReplacementValue::Int(_)
            | ReplacementValue::Bool(_)
            | ReplacementValue::Str(_)
            | ReplacementValue::Float(_)
            | ReplacementValue::Numeric(_)
            | ReplacementValue::Unit
    );
    if !scalar {
        return Err(unsupported(
            format!("returning {} from the scalar replacement profile", value_kind(value)),
            span,
        ));
    }
    let matches = match (return_type, value) {
        (IncanType::Primitive(IncanPrimitiveType::Int), ReplacementValue::Int(_))
        | (IncanType::Primitive(IncanPrimitiveType::Float), ReplacementValue::Float(_))
        | (IncanType::Primitive(IncanPrimitiveType::Bool), ReplacementValue::Bool(_))
        | (IncanType::Primitive(IncanPrimitiveType::Str), ReplacementValue::Str(_))
        | (IncanType::Primitive(IncanPrimitiveType::Unit), ReplacementValue::Unit) => true,
        (ty, ReplacementValue::Numeric(value)) => numeric_value_matches_type(value, ty),
        (IncanType::Unknown | IncanType::Infer, _) => true,
        _ => false,
    };
    if matches {
        if let ReplacementValue::Numeric(value) = value {
            validate_numeric_value(value, span)?;
        }
        Ok(())
    } else {
        Err(unsupported(
            format!(
                "returned {} contradicts checked result type `{return_type}`",
                value_kind(value)
            ),
            span,
        ))
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

/// Project direct task lifecycle events into the stable receipt/report vocabulary.
fn task_lifecycle_projection(events: &[TaskLifecycleEvent]) -> Vec<TaskLifecycleProjection> {
    events
        .iter()
        .map(|event| TaskLifecycleProjection {
            task_id: event.task_id,
            event: event.event,
            span_start: event.span.start,
            span_end: event.span.end,
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

/// Render task transitions as a deterministic output-identity component without relying on debug formatting.
fn canonical_task_lifecycle_summary(events: &[TaskLifecycleEvent]) -> String {
    task_lifecycle_projection(events)
        .into_iter()
        .map(|event| {
            format!(
                "task={};event={};span={}..{}",
                event.task_id, event.event, event.span_start, event.span_end
            )
        })
        .collect::<Vec<_>>()
        .join("|")
}

/// Render emitted source output as an unambiguous output-identity component.
///
/// Each line is length-prefixed so embedded separators or newlines cannot make two distinct output streams share a
/// digest input. This deliberately records the observable source effect rather than treating a successful return
/// value as the whole program result.
fn canonical_emitted_output_summary(lines: &[String]) -> String {
    lines
        .iter()
        .map(|line| format!("{}:{line}", line.len()))
        .collect::<String>()
}

#[cfg(test)]
mod output_identity_tests {
    use super::canonical_emitted_output_summary;

    #[test]
    fn emitted_output_summary_is_unambiguous() {
        let split_after_two = vec!["ab".to_string(), "c".to_string()];
        let split_after_one = vec!["a".to_string(), "bc".to_string()];
        let one_empty_line = vec![String::new()];

        assert_ne!(
            canonical_emitted_output_summary(&split_after_two),
            canonical_emitted_output_summary(&split_after_one),
            "different source-output boundaries must not share an output-identity component"
        );
        assert_ne!(
            canonical_emitted_output_summary(&[]),
            canonical_emitted_output_summary(&one_empty_line),
            "no output and one blank output line are different source-observable outcomes"
        );
    }
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
        // The label names the operation's *declaration* rather than the call site's spelling, because the plan's
        // canonical identity is the only thing that says which operation this is.
        Callee::ProviderOperation(plan) => {
            format!("provider operation `{}`", plan.operation.declaration_name)
        }
    }
}

/// Render a compiler-owned helper name without depending on generated-Rust spellings.
const fn helper_label(helper: HelperOp) -> &'static str {
    helper.as_str()
}

/// Render an aggregate kind as a compact source-level diagnostic label.
fn aggregate_label(kind: &incan_semantics_core::body_ir::AggregateKind) -> &'static str {
    match kind {
        incan_semantics_core::body_ir::AggregateKind::Tuple => "tuple",
        incan_semantics_core::body_ir::AggregateKind::List => "list",
        incan_semantics_core::body_ir::AggregateKind::Set => "set",
        incan_semantics_core::body_ir::AggregateKind::Range => "range",
        incan_semantics_core::body_ir::AggregateKind::Constructor(_) => "constructor",
    }
}

/// Register the bounded scalar/control profile beside the replacement executor that implements it.
///
/// The compatibility collector reports this contribution but does not own its feature definitions. In particular,
/// successful direct execution remains non-green until each admitted source contract has paired comparison evidence;
/// every individual corpus match stays case-scoped.
pub(crate) fn replacement_compatibility_direct_execution_contribution()
-> crate::replacement_compatibility::ReplacementCompatibilityContribution {
    use crate::replacement_compatibility::{
        ComparisonEvidence, OutstandingComparisonEvidence, feature_requirement_link, implementation_requirement,
        local_implementation_contribution, partially_materialized_feature_at_boundary, preserved_feature_at_boundary,
    };

    let mut async_tasks = preserved_feature_at_boundary(
        "async.tasks",
        "One exact source-local `std.async` activation executes same-module async calls, direct await, and source-order ready-tie races through receipt-bound task frames.",
        "src/frontend/typechecker/check_expr/control_flow.rs",
        "fn check_await",
        "fn lower_race_for",
        "fn execute_race",
    );
    async_tasks.owner_issue = Some(988);
    async_tasks.migration_or_blocker = Some(
        "Closed #1155 delivered the bounded source-local task profile; open #988 owns its remaining paired source-observable comparison evidence."
            .to_string(),
    );
    if let ComparisonEvidence::Unavailable {
        outstanding_evidence, ..
    } = &mut async_tasks.evidence.surfaces.independent_comparison
    {
        *outstanding_evidence = OutstandingComparisonEvidence::Scheduled {
            owner_issue: 988,
            note: "Closed #1155 delivered direct task execution; open #988 owns exact paired source-observable evidence through #1146's completed route, so the broader async feature remains non-green."
                .to_string(),
        };
    }

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
                "Bounded scalar arithmetic, comparisons, boolean operators, strings, and int/bool/str/None JSON stringification execute directly from Body IR.",
                "src/frontend/typechecker/check_expr/ops.rs",
                "fn check_binary",
                "fn lower_binary",
                "fn evaluate_binary",
            ),
            partially_materialized_feature_at_boundary(
                "language.numeric-complete",
                "Exact signed and unsigned widths, finite f32/f64, and decimal values retain their checked carrier through literals, constants, locals, lossless widening, source-local calls, entry arguments and results, Display output, receipts, reports, and bounded source-observable comparison. Public direct and shadow exact-float carriers reject NaN and infinities; ordinary float parsing remains separately compared. Arithmetic, unary operations, resize methods, Debug formatting, aggregates, matching, and decimal scalar casts remain explicit pre-effect refusals owned by #988.",
                988,
                "#1279 materializes the typed carrier and bounded movement/output contract. #988 owns the explicitly refused numeric operations, overflow behavior, aggregate integration, Debug formatting, resize methods, and decimal scalar conversions required before the wider feature can become green.",
                "src/frontend/typechecker/check_stmt.rs",
                "fn check_assignment",
                "src/frontend/body_ir/primitives.rs",
                "fn lower_checked_literal",
                "fn validate_reachable_typed_numeric_profile",
            ),
            async_tasks,
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
                "Scalars, strings, operators, conversions, and scalar JSON stringification preserve checked type, exact bytes, and failure behavior.",
                "Body IR operands/rvalues and replacement evaluator",
                "replacement-body-v0 scalar corpus, including replacement-body-v0-025",
                "Scalar representation is an internal evaluator mechanism.",
            ),
            implementation_requirement(
                "async.runtime",
                "Source-local direct tasks preserve construction, polling, source-order race ties, cancellation, and receipt-bound lifecycle evidence.",
                "source-local Body IR plus replacement task runtime",
                "replacement-body-v0-018 and replacement-body-v0-019 corpus probes",
                "Task frames are private direct-execution machinery, not a general scheduler claim.",
            ),
        ],
        Vec::new(),
        vec![
            feature_requirement_link("language.control-flow", "control.normalized-flow"),
            feature_requirement_link("language.numeric-and-scalar", "runtime.scalar-values"),
            feature_requirement_link("language.numeric-complete", "runtime.scalar-values"),
            feature_requirement_link("async.tasks", "async.runtime"),
            feature_requirement_link("async.tasks", "receipts.comparison"),
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
        BinOp::Pow => "exponentiation",
        BinOp::BitAnd => "bitwise and",
        BinOp::BitOr => "bitwise or",
        BinOp::BitXor => "bitwise exclusive or",
        BinOp::Shl => "left shift",
        BinOp::Shr => "right shift",
        BinOp::Eq => "equality comparison",
        BinOp::Ne => "inequality comparison",
        BinOp::Lt => "less-than comparison",
        BinOp::Le => "less-or-equal comparison",
        BinOp::Gt => "greater-than comparison",
        BinOp::Ge => "greater-or-equal comparison",
        BinOp::Is => "identity comparison",
        BinOp::IsNot => "negated identity comparison",
        BinOp::And => "boolean conjunction",
        BinOp::Or => "boolean disjunction",
    }
}

/// Render one replacement value's dynamic shape for an unsupported-operation diagnostic.
fn value_kind(value: &ReplacementValue) -> String {
    match value {
        ReplacementValue::Int(_) => "int".to_string(),
        ReplacementValue::Bool(_) => "bool".to_string(),
        ReplacementValue::Str(_) => "str".to_string(),
        ReplacementValue::Float(_) => "float".to_string(),
        ReplacementValue::Numeric(value) => value.type_name(),
        ReplacementValue::Unit => "unit".to_string(),
        ReplacementValue::Range { .. } => "range".to_string(),
        ReplacementValue::List { .. } => "list".to_string(),
        ReplacementValue::Tuple(_) => "tuple".to_string(),
        ReplacementValue::Set(_) => "set".to_string(),
        ReplacementValue::Dict(_) => "dict".to_string(),
        ReplacementValue::Nominal { .. } => "nominal".to_string(),
        ReplacementValue::FieldlessEnum { .. } => "fieldless enum".to_string(),
        ReplacementValue::ValueEnum { .. } => "value enum".to_string(),
        ReplacementValue::Result { .. } => "Result".to_string(),
        ReplacementValue::Callable(_) => "callable".to_string(),
        ReplacementValue::Generator(_) => "generator".to_string(),
        ReplacementValue::Task(_) => "direct task".to_string(),
        ReplacementValue::Adapter(_) => "generator adapter".to_string(),
        ReplacementValue::Zip(_) => "Zip iterator".to_string(),
        ReplacementValue::CollectedGenerator { .. } => "collected generator list".to_string(),
    }
}

/// Convert one Body-IR literal into a first-profile replacement value.
///
/// This is fallible because not every constant the frontend can now represent has a direct runtime value. A
/// byte-string literal is representable in Body IR (#1165) but carries no `bytes` value in this profile, so it
/// refuses at the read rather than being coerced into the `str` it is not.
fn constant_value(constant: &Constant, span: HirSourceSpan) -> Result<ReplacementValue, ReplacementExecutionError> {
    match constant {
        Constant::Int(value) => Ok(ReplacementValue::Int(*value)),
        Constant::Bool(value) => Ok(ReplacementValue::Bool(*value)),
        Constant::Str(value) => Ok(ReplacementValue::Str(value.clone())),
        Constant::Unit | Constant::None => Ok(ReplacementValue::Unit),
        Constant::Float(value) => binary_float_literal_value(value, span),
        Constant::TypedNumeric(value) => typed_numeric_constant_value(value, span).map(ReplacementValue::Numeric),
        Constant::Bytes(_) => Err(unsupported("byte-string literal", span)),
    }
}

/// Materialize and validate one exact typed-numeric Body-IR constant.
fn typed_numeric_constant_value(
    constant: &TypedNumericConstant,
    span: HirSourceSpan,
) -> Result<ReplacementNumericValue, ReplacementExecutionError> {
    let value = match constant {
        TypedNumericConstant::Signed { kind, value } => ReplacementNumericValue::Signed {
            kind: *kind,
            value: *value,
        },
        TypedNumericConstant::Unsigned { kind, value } => ReplacementNumericValue::Unsigned {
            kind: *kind,
            value: *value,
        },
        TypedNumericConstant::F32 { bits } => ReplacementNumericValue::F32(f32::from_bits(*bits)),
        TypedNumericConstant::F64 { bits } => ReplacementNumericValue::F64(f64::from_bits(*bits)),
        TypedNumericConstant::Decimal {
            precision,
            scale,
            coefficient,
            literal_scale,
        } => ReplacementNumericValue::Decimal {
            precision: *precision,
            scale: *scale,
            coefficient: *coefficient,
            literal_scale: *literal_scale,
        },
    };
    validate_numeric_value(&value, span)?;
    Ok(value)
}

/// Materialize an ordinary binary-float Body-IR literal with the same value normalization as Rust emission.
///
/// Body IR deliberately retains the lexical representation for diagnostics and snapshots. The lexer has already
/// accepted that spelling as a binary float, but direct execution needs the value rather than its source text so
/// `str(1_000.50)` and `str(1.0)` use ordinary `f64` Display. A checked decimal uses `TypedNumeric`; a `d` suffix
/// reaching this legacy variant is malformed Body IR and remains a visible refusal.
fn binary_float_literal_value(repr: &str, span: HirSourceSpan) -> Result<ReplacementValue, ReplacementExecutionError> {
    if repr.ends_with('d') {
        return Err(unsupported("decimal literal", span));
    }
    let normalized = repr.replace('_', "");
    normalized
        .parse::<f64>()
        .map(ReplacementValue::Float)
        .map_err(|_| unsupported("binary float literal outside the direct f64 carrier", span))
}

/// Apply the native emitter's explicit `as i64` scalar conversion to one typed binary numeric.
fn numeric_to_int(value: ReplacementNumericValue, span: HirSourceSpan) -> Result<i64, ReplacementExecutionError> {
    match value {
        ReplacementNumericValue::Signed { value, .. } => Ok(value as i64),
        ReplacementNumericValue::Unsigned { value, .. } => Ok(value as i64),
        ReplacementNumericValue::F32(value) => Ok(value as i64),
        ReplacementNumericValue::F64(value) => Ok(value as i64),
        ReplacementNumericValue::Decimal { .. } => Err(unsupported(
            "`int` of decimal has no admitted native scalar-cast contract",
            span,
        )),
    }
}

/// Apply the native emitter's explicit `as f64` scalar conversion to one typed binary numeric.
fn numeric_to_float(value: ReplacementNumericValue, span: HirSourceSpan) -> Result<f64, ReplacementExecutionError> {
    match value {
        ReplacementNumericValue::Signed { value, .. } => Ok(value as f64),
        ReplacementNumericValue::Unsigned { value, .. } => Ok(value as f64),
        ReplacementNumericValue::F32(value) => Ok(f64::from(value)),
        ReplacementNumericValue::F64(value) => Ok(value),
        ReplacementNumericValue::Decimal { .. } => Err(unsupported(
            "`float` of decimal has no admitted native scalar-cast contract",
            span,
        )),
    }
}

/// Execute the shared `int(str)` policy without exposing the native route's panic-based API to direct execution.
fn parse_int_conversion(value: &str, span: HirSourceSpan) -> Result<ReplacementValue, ReplacementExecutionError> {
    parse_int_string(value)
        .map(ReplacementValue::Int)
        .ok_or_else(|| runtime_failure(IncanError::cannot_convert_to_int(value).to_string(), span))
}

/// Execute the shared `float(str)` policy while retaining the original input spelling in any diagnostic.
fn parse_float_conversion(value: &str, span: HirSourceSpan) -> Result<ReplacementValue, ReplacementExecutionError> {
    parse_float_string(value)
        .map(ReplacementValue::Float)
        .ok_or_else(|| runtime_failure(IncanError::cannot_convert_to_float(value).to_string(), span))
}

/// Convert only scalar/unit Body-IR constants to a direct pattern comparison value.
fn direct_pattern_constant(
    constant: &Constant,
    span: HirSourceSpan,
) -> Result<ReplacementValue, ReplacementExecutionError> {
    match constant {
        Constant::Int(_) | Constant::Bool(_) | Constant::Str(_) | Constant::Unit | Constant::None => {
            constant_value(constant, span)
        }
        Constant::Float(_) => Err(unsupported("floating-point match literal", span)),
        Constant::TypedNumeric(value) => Err(unsupported(
            format!("typed numeric `{}` match literal (owned by #988)", value.type_name()),
            span,
        )),
        Constant::Bytes(_) => Err(unsupported("byte-string match literal", span)),
    }
}

#[cfg(test)]
mod tests {
    use std::{
        cell::RefCell,
        collections::{BTreeMap, BTreeSet},
        rc::Rc,
    };

    use incan_semantics_core::{
        CanonicalSymbolId, CompilerNodeId, IncanPrimitiveType, IncanType, SemanticSourceTargetKind,
        body_ir::{Block, GlobalPlace, GlobalWritePolicy, RaceArm, ScopeId},
    };

    use super::{
        Body, BodyExecutor, BodyIrModule, Constant, GeneratorFrame, HirSourceSpan, LocalId, MAX_EXECUTION_STEPS,
        Operand, OwnershipFact, Place, ProgramIo, ReplacementExecutionError, ReplacementGenerator, ReplacementTask,
        ReplacementTaskState, ReplacementValue, Statement, StatementKind, bare_local, validate_read_place,
    };

    #[test]
    fn canonical_globals_are_refused_without_becoming_missing_frame_locals() {
        let span = HirSourceSpan::new(10, 20);
        let identity = CanonicalSymbolId::module_declaration(
            vec!["provider".to_string()],
            "LIMIT",
            SemanticSourceTargetKind::Const,
            HirSourceSpan::new(0, 5),
        );
        let place = Place::from_global(GlobalPlace {
            identity: identity.clone(),
            ty: IncanType::Primitive(IncanPrimitiveType::Int),
            write_policy: GlobalWritePolicy::ReadOnly,
        });

        for result in [
            bare_local(&place, span).map(|_| ()),
            validate_read_place(&place, span, &BTreeSet::new()),
        ] {
            assert!(matches!(
                result,
                Err(ReplacementExecutionError::Unsupported { description, span: error_span, .. })
                    if description.contains(&identity.render_compact()) && error_span == span
            ));
        }
    }

    /// A resumed generator must retain the steps its parent spent before the first poll.
    #[test]
    fn generator_resume_counts_the_parent_budget_before_polling_its_frame() {
        let span = HirSourceSpan::new(0, 1);
        let module = BodyIrModule {
            module_id: CompilerNodeId::module("replacement.generator_budget_test"),
            nominal_declarations: Vec::new(),
            fieldless_enum_declarations: Vec::new(),
            value_enum_declarations: Vec::new(),
            bodies: Vec::new(),
        };
        let mut stdout = std::io::sink();
        let mut stderr = std::io::sink();
        let mut io = ProgramIo::new(&mut stdout, &mut stderr);
        let mut executor = BodyExecutor::with_locals(
            &module,
            Rc::new(Vec::new()),
            BTreeMap::new(),
            BTreeMap::new(),
            MAX_EXECUTION_STEPS,
            &mut io,
        );
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

    /// A failed selected race frame becomes terminal while every still-constructed loser is cancelled before the
    /// original child failure can escape the direct executor.
    #[test]
    fn failed_race_winner_terminalizes_before_constructed_losers_are_cancelled() {
        let span = HirSourceSpan::new(4, 10);
        let module = BodyIrModule {
            module_id: CompilerNodeId::module("replacement.race_failure_test"),
            nominal_declarations: Vec::new(),
            fieldless_enum_declarations: Vec::new(),
            value_enum_declarations: Vec::new(),
            bodies: Vec::new(),
        };
        let body = Body {
            decl_id: CompilerNodeId::declaration("replacement.race_failure_test", "child"),
            direct_call_id: CompilerNodeId::declaration_span("replacement.race_failure_test", span.start, span.end),
            canonical: None,
            name: "child".to_string(),
            span,
            return_type: IncanType::Primitive(IncanPrimitiveType::Unit),
            locals: Vec::new(),
            params: Vec::new(),
            param_locals: Vec::new(),
            scopes: Vec::new(),
            block: Block {
                scope: ScopeId(0),
                stmts: vec![Statement {
                    kind: StatementKind::Break { value: None },
                    span,
                }],
            },
            runtime_requirements: Vec::new(),
            panic_facts: Vec::new(),
            is_async: true,
        };
        let winner = Rc::new(RefCell::new(ReplacementTask {
            id: 0,
            body: body.clone(),
            locals: BTreeMap::new(),
            state: ReplacementTaskState::Constructed,
        }));
        let loser = Rc::new(RefCell::new(ReplacementTask {
            id: 1,
            body: body.clone(),
            locals: BTreeMap::new(),
            state: ReplacementTaskState::Constructed,
        }));
        let later_loser = Rc::new(RefCell::new(ReplacementTask {
            id: 2,
            body,
            locals: BTreeMap::new(),
            state: ReplacementTaskState::Constructed,
        }));
        let winner_local = LocalId(0);
        let loser_local = LocalId(1);
        let later_loser_local = LocalId(2);
        let destination = LocalId(3);
        let arm_binding = LocalId(4);
        let mut locals = BTreeMap::new();
        locals.insert(winner_local, ReplacementValue::Task(winner.clone()));
        locals.insert(loser_local, ReplacementValue::Task(loser.clone()));
        locals.insert(later_loser_local, ReplacementValue::Task(later_loser.clone()));
        let mut stdout = std::io::sink();
        let mut stderr = std::io::sink();
        let mut io = ProgramIo::new(&mut stdout, &mut stderr);
        let mut executor = BodyExecutor::with_locals(&module, Rc::new(Vec::new()), locals, BTreeMap::new(), 0, &mut io);
        let arms = [
            RaceArm {
                awaitable: Operand::place(Place::from_local(winner_local), OwnershipFact::Borrow, false),
                binding: arm_binding,
                body: Block {
                    scope: ScopeId(0),
                    stmts: Vec::new(),
                },
                result: Operand::Constant(Constant::Unit),
            },
            RaceArm {
                awaitable: Operand::place(Place::from_local(loser_local), OwnershipFact::Borrow, false),
                binding: LocalId(5),
                body: Block {
                    scope: ScopeId(0),
                    stmts: Vec::new(),
                },
                result: Operand::Constant(Constant::Unit),
            },
            RaceArm {
                awaitable: Operand::place(Place::from_local(later_loser_local), OwnershipFact::Borrow, false),
                binding: LocalId(6),
                body: Block {
                    scope: ScopeId(0),
                    stmts: Vec::new(),
                },
                result: Operand::Constant(Constant::Unit),
            },
        ];

        let result = executor.execute_race(Some(&Place::from_local(destination)), &arms, span);
        assert_eq!(
            result.as_ref().err().and_then(ReplacementExecutionError::primary_span),
            Some(span)
        );
        assert!(matches!(&winner.borrow().state, ReplacementTaskState::Failed));
        assert!(matches!(&loser.borrow().state, ReplacementTaskState::Cancelled));
        assert!(matches!(&later_loser.borrow().state, ReplacementTaskState::Cancelled));
        let winner_id = winner.borrow().id;
        let loser_ids = [loser.borrow().id, later_loser.borrow().id];
        let winner_poll = executor
            .task_lifecycle
            .iter()
            .position(|event| event.task_id == winner_id && event.event == "polled");
        assert!(
            matches!(
                (
                    winner_poll,
                    executor
                        .task_lifecycle
                        .iter()
                        .position(|event| event.task_id == loser_ids[0] && event.event == "cancelled")
                ),
                (Some(winner_poll), Some(loser_cancellation)) if winner_poll < loser_cancellation
            ),
            "the selected winner must be polled before source-order loser cancellation: {:?}",
            executor.task_lifecycle
        );
        for loser_id in loser_ids {
            assert!(
                executor
                    .task_lifecycle
                    .iter()
                    .any(|event| event.task_id == loser_id && event.event == "cancelled"),
                "every constructed loser must record cancellation before the winner failure escapes: {:?}",
                executor.task_lifecycle
            );
            assert!(
                !executor
                    .task_lifecycle
                    .iter()
                    .any(|event| event.task_id == loser_id && matches!(event.event, "polled" | "completed")),
                "a cancelled race loser must never be polled or completed: {:?}",
                executor.task_lifecycle
            );
        }
    }
}
