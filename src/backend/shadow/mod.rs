//! One bounded, source-observable shadow comparison between the legacy and replacement backends (#1146).
//!
//! #986 gave the compiler a place to *record* a shadow comparison ([`ShadowComparisonState`]) but no way to run
//! one, so every requested comparison was `Unavailable`. This module supplies the first comparison that actually
//! runs, for one deliberately narrow profile, and keeps every other profile explicitly unavailable.
//!
//! ## What is compared
//!
//! The profile is [`SHADOW_COMPARISON_PROFILE_ID`]: one named free function in a source-only Incan module,
//! called with concrete scalar arguments. A module containing `main` is outside this harness profile. Both routes
//! observe the *same* source, independently:
//!
//! - **Replacement route.** The module is typechecked, lowered to Body IR, and executed directly by
//!   [`crate::backend::replacement`]. Nothing is generated, compiled, or spawned.
//! - **Legacy route.** The module plus a generated Incan entrypoint that calls the observed function with the profile's
//!   arguments and atomically stages a typed result report is run through Oven, the adopted build and execution
//!   authority: [`IrCodegen`] emits Rust, an Oven receipt authorizes those exact bytes, an immutable store-selected
//!   direct-`rustc` plan compiles them, and the produced program is executed as a process. See [`legacy_oven`] for that
//!   boundary.
//!
//! The observed function must not be named `main`, because the legacy route can only observe a scalar by calling
//! the function from a separate entrypoint and staging its result; a function that *is* the entrypoint has no
//! source-observable return value at all. This is why the CLI's `--backend replacement` build path, which
//! executes the module's `main`, is outside this profile — see [`PROGRAM_ENTRYPOINT_UNAVAILABLE_REASON`].
//!
//! ## The observable is transported losslessly
//!
//! Normal program output must remain program output. The legacy process therefore retains its stdout and stderr as
//! independent raw byte streams in [`LegacyProcessEvidence`], while a source-authored entrypoint writes its typed
//! function result to a staged file and atomically replaces the final report. [`decode_result_report`] reads that
//! report exactly: no result marker shares either program stream, no whitespace is trimmed, and no failed process
//! can contribute a partial report. The replacement route uses explicit capture writers. Comparison includes each
//! stream's exact bytes and order together with the typed result or classified failure; there is no cross-stream
//! ordering claim.
//!
//! ## What "matched" is allowed to mean
//!
//! [`RouteObservation`] combines exact per-stream bytes with the outcome in [`SourceObservable`]: either the observed
//! function returned normally (and the routes must agree on the typed scalar), or it stopped
//! on a runtime failure this profile can classify identically on both routes ([`RuntimeFailureClass`]). A failure
//! neither route can classify is [`ShadowUnavailable`], not agreement: "both stopped somehow" is not proof that
//! they stopped the same way, and an arithmetic overflow is not a division by zero.
//!
//! The generated wrapper uses two private exit statuses for result-file publication failures without writing a
//! control diagnostic into either program stream. Source imports of `rust::std::process` are outside this bounded
//! profile so user code cannot impersonate those private transport statuses.
//!
//! Three things are deliberately *not* comparators, because none of them observes program meaning:
//! generated Rust text, whether that Rust compiled, and the produced artifact's shape. A legacy program that
//! fails to build makes the comparison [`ShadowComparisonState::Unavailable`] with that build failure as the
//! reason — it never becomes a divergence claim, because a comparison that could not run has proven nothing.
//!
//! ## Receipts, and what survives an unavailable comparison
//!
//! Each route that executed is selected and finalized through the canonical #986 boundary ([`select_backend`],
//! [`resolve_execution`], [`finalize_receipt`]); this module never hashes a receipt itself. Both receipts carry
//! the same `source_identity` and the same `shadow_comparison` value, and both are declared
//! [`FallbackPolicy::Refuse`] so neither route can quietly become the other. They *differ*, correctly, in
//! `selected_backend`, `executed_backend`, and `output_identity`: each route's output identity covers what that
//! route actually produced, and pretending the two receipts were interchangeable would erase the very
//! independence the comparison depends on. The legacy route's output identity additionally covers its
//! [`LegacyExecutionAuthority`] — the Oven receipt, build unit, and direct-`rustc` plan that authorized the run —
//! so "which authority produced this legacy answer" is part of the record rather than an assumption.
//!
//! An unavailable comparison does **not** discard work that really happened. If the replacement route executed
//! and only the legacy route could not run, the replacement route keeps its own receipt and Body-IR evidence in
//! [`ShadowComparison::replacement`], carrying the unavailable state. Throwing that evidence away would lose a
//! real execution and make a staging gap look identical to a source the replacement backend cannot run at all.

pub mod legacy_oven;

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;
use std::sync::Arc;

use incan_core::lang::surface::constructors::{ConstructorId, as_str as constructor_name};

use crate::backend::IrCodegen;
use crate::backend::replacement::{
    ProgramIo, ProgramOutput, ReplacementExecution, ReplacementExecutionError, ReplacementNumericValue,
    ReplacementValue, execute_prevalidated_free_function_with_io, prepare_free_function_execution,
};
use crate::backend::selection::{
    BackendExecutionReceipt, BackendKind, BackendSelection, FallbackPolicy, ShadowComparisonState, digest_output,
    finalize_receipt, resolve_execution, select_backend,
};
use incan_semantics_core::body_ir::BodyIrModule;
use incan_semantics_core::{IncanPrimitiveType, IncanType};

use crate::frontend::body_ir::{apply_body_ir_input_contract, build_body_ir_module_v0};
use crate::frontend::diagnostics::DIAGNOSTIC_SCHEMA_VERSION;
use crate::frontend::typechecker::TypeChecker;
use crate::frontend::{lexer, parser};
use crate::provider::ProviderPlan;
use incan_core::lang::types::numerics::{self, NumericTypeId};
use incan_core::numeric_values::{decimal_value_fits, format_decimal_value, parse_decimal_literal_body};

/// Content-stable identity of the one comparison profile this module implements.
///
/// Recorded inside [`ShadowComparisonState::Matched`] and [`ShadowComparisonState::Diverged`] so a later reader
/// can tell *which* comparison a receipt is claiming, and so widening the profile forces a new identity rather
/// than silently reinterpreting old evidence.
pub const SHADOW_COMPARISON_PROFILE_ID: &str = "incan.shadow_comparison.direct_scalar_free_function.v2";

/// Reason a shadow request over a program entrypoint falls outside this profile.
///
/// The CLI's replacement build path executes the module's `main`. A `main` body's return value is not observable
/// from the produced legacy process, so there is nothing for the two routes to agree or disagree about; that is a
/// property of the profile, not a missing implementation, and it must not read as "no comparator exists".
pub const PROGRAM_ENTRYPOINT_UNAVAILABLE_REASON: &str = "the bounded source-observable comparison profile observes a named free function's returned scalar, and a \
     program entrypoint's return value is not observable through a separate source-authored report entrypoint; generated Rust is not \
     semantic proof";

/// The source-owned provider projection required to materialize one legacy comparison program.
///
/// The CLI adapter constructs this only from a compilation session that has already resolved the profile source's
/// Oven-compatible module graph. The legacy route receives the finished projection but never discovers providers,
/// invents a stdlib path, or infers provider identity from an Oven receipt or Rust extern list.
#[derive(Debug, Clone)]
pub(crate) struct ShadowLegacyMaterialization {
    provider_plan: Arc<ProviderPlan>,
    oven_build_unit_inputs: BTreeMap<String, String>,
    entry_source_identity: String,
}

impl ShadowLegacyMaterialization {
    /// Retain the final provider plan and native-closure inputs selected by the caller-owned compilation session.
    pub(crate) fn from_provider_plan(
        provider_plan: Arc<ProviderPlan>,
        oven_build_unit_inputs: BTreeMap<String, String>,
        entry_source_identity: String,
    ) -> Self {
        Self {
            provider_plan,
            oven_build_unit_inputs,
            entry_source_identity,
        }
    }

    /// Return the immutable provider plan shared by legacy codegen and project materialization.
    pub(crate) fn provider_plan(&self) -> &Arc<ProviderPlan> {
        &self.provider_plan
    }

    /// Refuse a legacy capability whose immutable native closure differs from this source session's projection.
    pub(crate) fn require_compatible_oven_build_unit_inputs(
        &self,
        adopted_build_unit_inputs: &BTreeMap<String, String>,
    ) -> Result<(), ShadowUnavailable> {
        if &self.oven_build_unit_inputs == adopted_build_unit_inputs {
            return Ok(());
        }
        Err(ShadowUnavailable::new(
            "the legacy comparison provider context does not match the adopted Oven build-unit inputs; stage a \
             receipt-backed native capability for this exact source-session provider closure",
        ))
    }

    /// Refuse a profile that differs from the exact source whose session selected this context.
    pub(crate) fn require_profile_source(&self, profile: &ShadowComparisonProfile) -> Result<(), ShadowUnavailable> {
        if self.entry_source_identity == profile.source_identity() {
            return Ok(());
        }
        Err(ShadowUnavailable::new(
            "the legacy comparison profile source does not match the source session that selected its provider \
             context",
        ))
    }
}

/// Exact first token in a source-authored typed result report.
const RESULT_REPORT_VERSION: &str = "incan-shadow-result-v2";

/// Fresh root bindings for one generated legacy result-report wrapper.
///
/// These aliases never borrow an ordinary source spelling. A checked profile chooses one versioned stem that is
/// absent from all source root bindings, so the appended wrapper cannot shadow a transport primitive or local and
/// valid source using an older generated-looking spelling remains observable.
#[derive(Debug, Clone)]
struct GeneratedWrapperIdentifiers {
    fs_rename: String,
    fs_write: String,
    rust_path: String,
    process_exit: String,
    result_value: String,
}

impl GeneratedWrapperIdentifiers {
    /// Construct one candidate alias set without assuming any spelling is free in source.
    fn for_version(version: u64) -> Self {
        Self {
            fs_rename: format!("__incan_shadow_fs_rename_v{version}"),
            fs_write: format!("__incan_shadow_fs_write_v{version}"),
            rust_path: format!("__incan_shadow_rust_path_v{version}"),
            process_exit: format!("__incan_shadow_process_exit_v{version}"),
            result_value: format!("__incan_shadow_result_value_v{version}"),
        }
    }

    /// Enumerate every generated binding that must avoid a source root declaration.
    fn all(&self) -> [&str; 5] {
        [
            &self.fs_rename,
            &self.fs_write,
            &self.rust_path,
            &self.process_exit,
            &self.result_value,
        ]
    }

    /// Select the first complete alias set absent from the checked source root scope.
    fn fresh_from_checked_source(checker: &TypeChecker) -> Result<Self, ShadowUnavailable> {
        for version in 1..=u64::MAX {
            let identifiers = Self::for_version(version);
            if identifiers
                .all()
                .into_iter()
                .all(|identifier| checker.lookup_symbol(identifier).is_none())
            {
                return Ok(identifiers);
            }
        }
        Err(ShadowUnavailable::new(
            "the bounded source-observable comparison profile exhausted its generated wrapper identifier space",
        ))
    }
}

/// Private process status when source-authored result staging fails at the write step.
const RESULT_TRANSPORT_WRITE_EXIT_STATUS: i32 = 86;

/// Private process status when source-authored result staging fails at the atomic rename step.
const RESULT_TRANSPORT_RENAME_EXIT_STATUS: i32 = 87;

/// Domain tag folded into the legacy route's output identity.
///
/// Keeps a legacy process observation from ever colliding with a replacement execution's output identity, even
/// when both routes observed the same scalar.
const LEGACY_OUTPUT_IDENTITY_TAG: &str = "incan.shadow_comparison.legacy_process_observation.v0";

/// Domain tag folded into a failed direct execution's output identity.
const REPLACEMENT_FAILURE_IDENTITY_TAG: &str = "incan.shadow_comparison.replacement_failure.v0";

/// The checked scalar kind carried by the dedicated function-result channel.
///
/// The kind comes from the source function's type-check facts before either route executes. It is part of the
/// report grammar, so `str` containing `"true"` is never silently recast as `bool` merely because the rendered
/// payload looks similar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FunctionResultKind {
    /// An Incan `int` result.
    Int,
    /// An ordinary Incan `float` result.
    Float,
    /// An explicit canonical sized numeric result.
    Numeric(NumericTypeId),
    /// A checked fixed-scale decimal result.
    Decimal { precision: u8, scale: u8 },
    /// An Incan `bool` result.
    Bool,
    /// An owned Incan `str` result.
    Str,
    /// An Incan `None`/unit result.
    Unit,
}

impl FunctionResultKind {
    /// The stable report label for this checked scalar kind.
    fn report_label(self) -> String {
        match self {
            Self::Int => "int".to_string(),
            Self::Float => "float".to_string(),
            Self::Numeric(kind) => numerics::as_str(kind).to_string(),
            Self::Decimal { precision, scale } => format!("decimal[{precision},{scale}]"),
            Self::Bool => "bool".to_string(),
            Self::Str => "str".to_string(),
            Self::Unit => "none".to_string(),
        }
    }

    /// Return the exact source expression that appends this result's report payload.
    fn report_expression(self, result_local: &str) -> String {
        let header = result_report_header(self);
        match self {
            Self::Int | Self::Float | Self::Numeric(_) | Self::Decimal { .. } | Self::Bool => {
                format!("{header:?} + str({result_local})")
            }
            Self::Str => format!("{header:?} + {result_local}"),
            Self::Unit => format!("{header:?}"),
        }
    }

    /// Decode the report's payload as this scalar kind without relying on an untyped display spelling.
    fn decode_payload(self, payload: &[u8]) -> Result<TypedFunctionResult, ShadowUnavailable> {
        let value = match self {
            Self::Int => {
                let text = std::str::from_utf8(payload).map_err(|error| {
                    ShadowUnavailable::new(format!(
                        "the legacy `int` result payload is not UTF-8, so it cannot be compared: {error}"
                    ))
                })?;
                let parsed = text.parse::<i64>().map_err(|error| {
                    ShadowUnavailable::new(format!(
                        "the legacy `int` result payload is not a canonical i64 spelling: {error}"
                    ))
                })?;
                if parsed.to_string() != text {
                    return Err(ShadowUnavailable::new(
                        "the legacy `int` result payload is not its canonical i64 spelling",
                    ));
                }
                text.to_string()
            }
            Self::Float => canonical_f64_payload(payload, "float")?,
            Self::Numeric(kind) => canonical_numeric_payload(kind, payload)?,
            Self::Decimal { precision, scale } => canonical_decimal_payload(payload, precision, scale)?,
            Self::Bool => match payload {
                b"true" => "true".to_string(),
                b"false" => "false".to_string(),
                _ => {
                    return Err(ShadowUnavailable::new(
                        "the legacy `bool` result payload was not exactly `true` or `false`",
                    ));
                }
            },
            Self::Str => String::from_utf8(payload.to_vec()).map_err(|error| {
                ShadowUnavailable::new(format!(
                    "the legacy `str` result payload is not valid UTF-8, so it cannot be compared: {error}"
                ))
            })?,
            Self::Unit => {
                if !payload.is_empty() {
                    return Err(ShadowUnavailable::new(
                        "the legacy `None` result report carried an unexpected payload",
                    ));
                }
                constructor_name(ConstructorId::None).to_string()
            }
        };
        Ok(TypedFunctionResult { kind: self, value })
    }

    /// Build one typed observable from a successful direct replacement result.
    fn observe_replacement_value(self, value: &ReplacementValue) -> Result<TypedFunctionResult, ShadowUnavailable> {
        let rendered = match (self, value) {
            (Self::Int, ReplacementValue::Int(value)) => value.to_string(),
            (Self::Float, ReplacementValue::Float(value)) => value.to_string(),
            (Self::Numeric(expected), ReplacementValue::Numeric(value))
                if replacement_numeric_kind(value) == Some(expected) =>
            {
                value.observable_text()
            }
            (
                Self::Decimal {
                    precision: expected_precision,
                    scale: expected_scale,
                },
                ReplacementValue::Numeric(ReplacementNumericValue::Decimal { precision, scale, .. }),
            ) if *precision == expected_precision && *scale == expected_scale => value.observable_text(),
            (Self::Bool, ReplacementValue::Bool(value)) => value.to_string(),
            (Self::Str, ReplacementValue::Str(value)) => value.clone(),
            (Self::Unit, ReplacementValue::Unit) => constructor_name(ConstructorId::None).to_string(),
            (expected, actual) => {
                return Err(ShadowUnavailable::new(format!(
                    "the direct route returned {actual:?}, which contradicts its checked {} result kind",
                    expected.report_label()
                )));
            }
        };
        Ok(TypedFunctionResult {
            kind: self,
            value: rendered,
        })
    }
}

/// Return the exact fixed ASCII header for one typed result report.
///
/// The delimiter is a literal colon byte, not a line ending or source escape. The payload begins immediately after
/// this header and is otherwise preserved verbatim.
fn result_report_header(kind: FunctionResultKind) -> String {
    format!("{RESULT_REPORT_VERSION}:{}:", kind.report_label())
}

/// Project a direct-route binary-numeric result to its exact checked kind.
fn replacement_numeric_kind(value: &ReplacementNumericValue) -> Option<NumericTypeId> {
    match value {
        ReplacementNumericValue::Signed { kind, .. } | ReplacementNumericValue::Unsigned { kind, .. } => Some(*kind),
        ReplacementNumericValue::F32(_) => Some(NumericTypeId::F32),
        ReplacementNumericValue::F64(_) => Some(NumericTypeId::F64),
        ReplacementNumericValue::Decimal { .. } => None,
    }
}

/// Decode one typed result payload as strict UTF-8 while retaining its kind in diagnostics.
fn payload_text<'payload>(payload: &'payload [u8], kind: &str) -> Result<&'payload str, ShadowUnavailable> {
    std::str::from_utf8(payload).map_err(|error| {
        ShadowUnavailable::new(format!(
            "the legacy `{kind}` result payload is not UTF-8, so it cannot be compared: {error}"
        ))
    })
}

/// Parse a payload and require the original bytes to equal that type's canonical Display spelling.
fn canonical_display_payload<T>(payload: &[u8], kind: &str) -> Result<String, ShadowUnavailable>
where
    T: std::str::FromStr + ToString,
    T::Err: std::fmt::Display,
{
    let text = payload_text(payload, kind)?;
    let parsed = text.parse::<T>().map_err(|error| {
        ShadowUnavailable::new(format!("the legacy `{kind}` result payload is not canonical: {error}"))
    })?;
    if parsed.to_string() != text {
        return Err(ShadowUnavailable::new(format!(
            "the legacy `{kind}` result payload is not its canonical Display spelling"
        )));
    }
    Ok(text.to_string())
}

/// Decode one canonical finite f32 result payload.
fn canonical_f32_payload(payload: &[u8], kind: &str) -> Result<String, ShadowUnavailable> {
    let text = canonical_display_payload::<f32>(payload, kind)?;
    if text.parse::<f32>().is_ok_and(f32::is_finite) {
        Ok(text)
    } else {
        Err(ShadowUnavailable::new(format!(
            "the legacy `{kind}` result payload is not a finite binary float"
        )))
    }
}

/// Decode one canonical finite f64 result payload.
fn canonical_f64_payload(payload: &[u8], kind: &str) -> Result<String, ShadowUnavailable> {
    let text = canonical_display_payload::<f64>(payload, kind)?;
    if text.parse::<f64>().is_ok_and(f64::is_finite) {
        Ok(text)
    } else {
        Err(ShadowUnavailable::new(format!(
            "the legacy `{kind}` result payload is not a finite binary float"
        )))
    }
}

/// Decode an exact binary-numeric payload with the parser for its retained numeric identity.
fn canonical_numeric_payload(kind: NumericTypeId, payload: &[u8]) -> Result<String, ShadowUnavailable> {
    let label = numerics::as_str(kind);
    match kind {
        NumericTypeId::I8 => canonical_integer_payload::<i8>(payload, label),
        NumericTypeId::I16 => canonical_integer_payload::<i16>(payload, label),
        NumericTypeId::I32 => canonical_integer_payload::<i32>(payload, label),
        NumericTypeId::I64 => canonical_integer_payload::<i64>(payload, label),
        NumericTypeId::I128 => canonical_integer_payload::<i128>(payload, label),
        NumericTypeId::ISize => canonical_integer_payload::<isize>(payload, label),
        NumericTypeId::U8 => canonical_integer_payload::<u8>(payload, label),
        NumericTypeId::U16 => canonical_integer_payload::<u16>(payload, label),
        NumericTypeId::U32 => canonical_integer_payload::<u32>(payload, label),
        NumericTypeId::U64 => canonical_integer_payload::<u64>(payload, label),
        NumericTypeId::U128 => canonical_integer_payload::<u128>(payload, label),
        NumericTypeId::USize => canonical_integer_payload::<usize>(payload, label),
        NumericTypeId::F32 => canonical_f32_payload(payload, label),
        NumericTypeId::F64 => canonical_f64_payload(payload, label),
        NumericTypeId::Bool => Err(ShadowUnavailable::new(
            "bool reached the typed numeric result-report channel",
        )),
    }
}

/// Decode one exact integer payload and enforce its target type's range and canonical spelling.
fn canonical_integer_payload<T>(payload: &[u8], kind: &str) -> Result<String, ShadowUnavailable>
where
    T: std::str::FromStr + ToString,
    T::Err: std::fmt::Display,
{
    canonical_display_payload::<T>(payload, kind)
}

/// Decode a decimal payload while enforcing checked precision, scale, range, and Display spelling.
fn canonical_decimal_payload(payload: &[u8], precision: u8, scale: u8) -> Result<String, ShadowUnavailable> {
    let kind = format!("decimal[{precision},{scale}]");
    let text = payload_text(payload, &kind)?;
    let Some(parsed) = parse_decimal_literal_body(text) else {
        return Err(ShadowUnavailable::new(format!(
            "the legacy `{kind}` result payload is not a canonical in-range decimal spelling"
        )));
    };
    if !decimal_value_fits(precision, scale, parsed.coefficient, parsed.literal_scale) {
        return Err(ShadowUnavailable::new(format!(
            "the legacy `{kind}` result payload is not a canonical in-range decimal spelling"
        )));
    }
    let canonical = format_decimal_value(parsed.coefficient, parsed.literal_scale);
    if canonical != text {
        return Err(ShadowUnavailable::new(format!(
            "the legacy `{kind}` result payload is not its canonical Display spelling"
        )));
    }
    Ok(text.to_string())
}

/// One typed result recovered through the separate source-authored report channel.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct TypedFunctionResult {
    /// Checked source-level kind carried by the report grammar.
    pub kind: FunctionResultKind,
    /// Exact canonical scalar payload, preserving all `str` bytes after UTF-8 validation.
    pub value: String,
}

// ============================================================================
// Profile
// ============================================================================

/// One instance of the bounded comparison profile: a source module, an observed function, and its arguments.
///
/// The module must hold exactly one free function (the #988 replacement executor refuses anything else), so the
/// legacy entrypoint that calls it is generated for the legacy route's derived program only and never enters the
/// source the replacement route consumes.
///
/// Deliberately `PartialEq` but not `Eq`: `ReplacementValue` now carries generator values, which have no total
/// equality. Two profiles are compared by content identity anyway ([`ShadowComparisonProfile::profile_identity`]),
/// and this profile only ever admits scalar arguments, so nothing here needs a stronger bound than the values it
/// holds can honestly provide.
#[derive(Debug, Clone, PartialEq)]
pub struct ShadowComparisonProfile {
    source: String,
    function: String,
    arguments: Vec<ReplacementValue>,
}

impl ShadowComparisonProfile {
    /// Declare a comparison profile over one source module, observed free function, and argument list.
    ///
    /// Nothing is validated here: whether the source, function name, and arguments are inside the profile is
    /// decided when a route is actually observed, so an out-of-profile request produces a recorded
    /// [`ShadowComparisonState::Unavailable`] with a concrete reason instead of a constructor error a caller
    /// could discard.
    #[must_use]
    pub fn new(source: impl Into<String>, function: impl Into<String>, arguments: Vec<ReplacementValue>) -> Self {
        Self {
            source: source.into(),
            function: function.into(),
            arguments,
        }
    }

    /// The module source both routes observe.
    #[must_use]
    pub fn source(&self) -> &str {
        &self.source
    }

    /// The observed free function's source-level name.
    #[must_use]
    pub fn function(&self) -> &str {
        &self.function
    }

    /// Stable kind of comparison this profile performs.
    ///
    /// Recorded alongside the instance identity in every receipt so a consumer keyed on comparison capability —
    /// #1153's compatibility-feature registry, for one — can link to "this kind of comparison" without decoding
    /// a content hash, while evidence still names the exact instance.
    #[must_use]
    pub fn profile_kind(&self) -> &'static str {
        SHADOW_COMPARISON_PROFILE_ID
    }

    /// Content identity of the observed source, used as both routes' selection `source_identity`.
    ///
    /// Deliberately covers the module source alone, so the two routes are provably selected for the same source
    /// even though the legacy route additionally generates an entrypoint to call into it.
    #[must_use]
    pub fn source_identity(&self) -> String {
        digest_output(&[self.source.as_str()])
    }

    /// Content identity of this source input: the profile kind, the source, the observed function, and the
    /// exact arguments.
    ///
    /// Two comparisons over the same module but different arguments are different claims, so they must not share an
    /// identity, and [`classify_observations`] refuses to pair observations whose identities disagree. An argument
    /// that has no source spelling makes the profile unrepresentable, and the identity falls back to a stable marker
    /// rather than silently colliding with a representable argument list.
    #[must_use]
    pub fn profile_identity(&self) -> String {
        let arguments = match self.argument_literals() {
            Ok(_) => {
                let identities = self
                    .arguments
                    .iter()
                    .map(ReplacementValue::receipt_identity_text)
                    .collect::<Vec<_>>();
                let identity_parts = identities.iter().map(String::as_str).collect::<Vec<_>>();
                digest_output(&identity_parts)
            }
            Err(_) => "<unrepresentable arguments>".to_string(),
        };
        digest_output(&[
            SHADOW_COMPARISON_PROFILE_ID,
            self.source.as_str(),
            self.function.as_str(),
            arguments.as_str(),
        ])
    }

    /// Build the Incan program the legacy route runs: this profile's module plus a generated entrypoint that
    /// calls the observed function and atomically publishes a typed result report.
    ///
    /// The generated entrypoint is Incan source, not hand-written Rust. It imports existing `rust::std::fs`
    /// `write` and `rename` primitives, leaving program stdout and stderr untouched by the result transport.
    fn legacy_program_source(
        &self,
        result_kind: FunctionResultKind,
        report_path: &Path,
        identifiers: &GeneratedWrapperIdentifiers,
    ) -> Result<String, ShadowUnavailable> {
        let report_path = report_path.to_str().ok_or_else(|| {
            ShadowUnavailable::new(format!(
                "the source-authored result report path {} is not valid UTF-8",
                report_path.display()
            ))
        })?;
        let staged_path = format!("{report_path}.next");
        let report_path = incan_string_literal(report_path)?;
        let staged_path = incan_string_literal(&staged_path)?;
        let arguments = self.argument_literals()?.join(", ");
        let result_expression = result_kind.report_expression(&identifiers.result_value);
        let mut program = format!(
            "from rust::std::fs import rename as {fs_rename}, write as {fs_write}\n\
             from rust::std::path import Path as {rust_path}\n\
             from rust::std::process import exit as {process_exit}\n",
            fs_rename = identifiers.fs_rename,
            fs_write = identifiers.fs_write,
            rust_path = identifiers.rust_path,
            process_exit = identifiers.process_exit,
        );
        program.push_str(&self.source);
        if !program.ends_with('\n') {
            program.push('\n');
        }
        let _ = write!(
            program,
            "\ndef main() -> None:\n    \
             \"\"\"Publish this harness call's typed result without sharing program streams.\"\"\"\n    \
             {result_value} = {}({arguments})\n    \
             match {fs_write}({rust_path}.new({staged_path}), {result_expression}):\n        \
                 Ok(_) => pass\n        \
                 Err(_) => {process_exit}({RESULT_TRANSPORT_WRITE_EXIT_STATUS})\n    \
             match {fs_rename}({rust_path}.new({staged_path}), {rust_path}.new({report_path})):\n        \
                 Ok(_) => pass\n        \
                 Err(_) => {process_exit}({RESULT_TRANSPORT_RENAME_EXIT_STATUS})\n",
            self.function,
            result_value = identifiers.result_value,
            fs_write = identifiers.fs_write,
            fs_rename = identifiers.fs_rename,
            rust_path = identifiers.rust_path,
            process_exit = identifiers.process_exit,
        );
        Ok(program)
    }

    /// Parse and prepare the profile source through the same manifest-free Body-IR input contract as both routes.
    ///
    /// The profile intentionally owns no package manifest or feature selection, so its only valid feature
    /// projection is empty. Preparing before either route observes the selected function prevents a declaration
    /// behind `when feature(...)` from becoming comparison evidence for a compilation that does not contain it.
    fn program_after_input_contract(&self) -> Result<crate::frontend::ast::Program, ShadowUnavailable> {
        let tokens = lexer::lex(self.source())
            .map_err(|errors| ShadowUnavailable::new(format!("comparison source did not lex: {errors:?}")))?;
        let program = parser::parse(&tokens)
            .map_err(|errors| ShadowUnavailable::new(format!("comparison source did not parse: {errors:?}")))?;
        apply_body_ir_input_contract(program, Path::new("incan_shadow_comparison.incn")).map_err(|errors| {
            ShadowUnavailable::new(format!(
                "comparison source did not satisfy the Body IR input contract: {errors:?}"
            ))
        })
    }

    /// Render every argument as the Incan source literal the legacy entrypoint passes.
    fn argument_literals(&self) -> Result<Vec<String>, ShadowUnavailable> {
        self.arguments.iter().map(argument_literal).collect()
    }
}

/// Render one scalar argument as an Incan source literal.
///
/// Only the scalars the profile can spell in source are accepted. A collection, tuple, range, or unit
/// argument has no literal form this profile is willing to synthesize, so it becomes an explicit unavailable
/// reason rather than a guessed spelling that would silently change what was executed.
fn argument_literal(value: &ReplacementValue) -> Result<String, ShadowUnavailable> {
    match value {
        ReplacementValue::Int(value) => Ok(value.to_string()),
        ReplacementValue::Float(value) => finite_float_literal(*value, "float"),
        ReplacementValue::Numeric(value) => numeric_argument_literal(value),
        ReplacementValue::Bool(value) => Ok(value.to_string()),
        ReplacementValue::Str(value) => incan_string_literal(value),
        other => Err(ShadowUnavailable::new(format!(
            "the bounded source-observable comparison profile can only pass checked scalar arguments as \
             source literals; got {other:?}"
        ))),
    }
}

/// Render one finite binary float as a source token that cannot be reclassified as an integer literal.
fn finite_float_literal<T>(value: T, kind: &str) -> Result<String, ShadowUnavailable>
where
    T: Copy + std::fmt::Display + Into<f64>,
{
    if !value.into().is_finite() {
        return Err(ShadowUnavailable::new(format!(
            "the bounded source-observable comparison profile refuses a non-finite `{kind}` argument"
        )));
    }
    let mut literal = value.to_string();
    if !literal.contains(['.', 'e', 'E']) {
        literal.push_str(".0");
    }
    Ok(literal)
}

/// Render one checked exact numeric through the source spelling its declared parameter type will contextualize.
fn numeric_argument_literal(value: &ReplacementNumericValue) -> Result<String, ShadowUnavailable> {
    if !value.is_valid() {
        return Err(malformed_numeric_argument(value));
    }
    match value {
        ReplacementNumericValue::Signed { value, .. } => Ok(value.to_string()),
        ReplacementNumericValue::Unsigned { value, .. } => Ok(value.to_string()),
        ReplacementNumericValue::F32(value) => finite_float_literal(*value, "f32"),
        ReplacementNumericValue::F64(value) => finite_float_literal(*value, "f64"),
        ReplacementNumericValue::Decimal { .. } => Ok(format!("{}d", value.observable_text())),
    }
}

/// Build the comparison-profile refusal for a malformed direct typed-numeric argument.
fn malformed_numeric_argument(value: &ReplacementNumericValue) -> ShadowUnavailable {
    ShadowUnavailable::new(format!(
        "the bounded source-observable comparison profile refuses a malformed `{}` argument",
        value.type_name()
    ))
}

/// Escape one string argument into an Incan double-quoted literal.
///
/// Only the escapes Incan itself spells are emitted. Any other control character is refused instead of being
/// approximated, because a legacy entrypoint that passes a *different* string than the replacement route received
/// would produce a comparison over two different inputs.
fn incan_string_literal(value: &str) -> Result<String, ShadowUnavailable> {
    let mut literal = String::with_capacity(value.len() + 2);
    literal.push('"');
    for character in value.chars() {
        match character {
            '"' => literal.push_str("\\\""),
            '\\' => literal.push_str("\\\\"),
            '\n' => literal.push_str("\\n"),
            '\r' => literal.push_str("\\r"),
            '\t' => literal.push_str("\\t"),
            other if other.is_control() => {
                return Err(ShadowUnavailable::new(format!(
                    "the bounded source-observable comparison profile refuses a `str` argument containing the \
                     control character U+{:04X}, which it cannot spell as an Incan literal",
                    u32::from(other)
                )));
            }
            other => literal.push(other),
        }
    }
    literal.push('"');
    Ok(literal)
}

/// Recover the exact typed function result from its dedicated source-authored report.
///
/// The report starts with fixed ASCII version and checked-kind tokens separated by literal colon bytes. The
/// remaining bytes are the payload verbatim. In particular, a `str` payload may contain leading, trailing, or
/// embedded newlines without being confused with normal stdout. A malformed report is unavailable rather than
/// guessed or lossily decoded.
pub fn decode_result_report(
    report: &[u8],
    expected_kind: FunctionResultKind,
) -> Result<TypedFunctionResult, ShadowUnavailable> {
    let header = result_report_header(expected_kind);
    let Some(payload) = report.strip_prefix(header.as_bytes()) else {
        return Err(ShadowUnavailable::new(format!(
            "the legacy result report did not start with the expected `{RESULT_REPORT_VERSION}` {} header",
            expected_kind.report_label()
        )));
    };
    expected_kind.decode_payload(payload)
}

// ============================================================================
// The compared observable
// ============================================================================

/// A runtime failure this profile can recognize identically on both routes.
///
/// Narrow on purpose: a failure class exists here only when the legacy process and the direct Body-IR executor
/// both report it unambiguously. Everything else is [`ShadowUnavailable`], because agreeing that "something went
/// wrong" is not evidence that the same thing went wrong. The classes stay distinct for the same reason — an
/// arithmetic overflow and a division by zero are different program behaviors, and collapsing them would let a
/// real divergence report as agreement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeFailureClass {
    /// A source-level `assert` failed.
    Assertion,
    /// An integer division or modulo by zero was attempted.
    DivisionByZero,
    /// An integer operation produced a result the type cannot represent.
    ArithmeticOverflow,
    /// The canonical `int(str)` parser rejected its input.
    IntConversion,
    /// The canonical `float(str)` parser rejected its input.
    FloatConversion,
    /// A non-finite value attempted to cross into exact `f32`.
    NonFiniteExactF32,
    /// A non-finite value attempted to cross into exact `f64`.
    NonFiniteExactF64,
}

impl RuntimeFailureClass {
    /// Stable label used in comparison detail text and receipts.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Assertion => "assertion-failed",
            Self::DivisionByZero => "division-by-zero",
            Self::ArithmeticOverflow => "arithmetic-overflow",
            Self::IntConversion => "conversion-int",
            Self::FloatConversion => "conversion-float",
            Self::NonFiniteExactF32 => "non-finite-exact-f32",
            Self::NonFiniteExactF64 => "non-finite-exact-f64",
        }
    }
}

/// The termination and typed function-result portion of one source-level observation.
///
/// The route's exact stdout/stderr bytes are compared alongside this value. Timing, artifact layout and generated
/// code are not part of the comparison. Platform exit codes remain in raw legacy evidence; source-level failures
/// compare through the explicit classified failure vocabulary.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum SourceObservable {
    /// The observed function returned normally through the typed result channel.
    ///
    /// For the replacement route this is validated against the pre-execution checked return kind. For the legacy
    /// route it is recovered from the atomically published report by [`decode_result_report`].
    Completed { result: TypedFunctionResult },
    /// The observed function stopped on a classified runtime failure.
    Failed { failure: RuntimeFailureClass },
}

impl SourceObservable {
    /// Render the observable as one comparable line for receipts and divergence detail.
    ///
    /// The completed payload is debug-quoted so whitespace differences stay visible in a divergence report; a
    /// bare rendering would make `"x"` and `"x\n"` look identical in the very message meant to explain them.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::Completed { result } => format!("completed({:?}, {:?})", result.kind, result.value),
            Self::Failed { failure } => format!("failed({})", failure.label()),
        }
    }
}

/// One route's observation plus the raw evidence behind it.
///
/// `detail` is never compared; it exists so a divergence or an unavailable result can name what actually
/// happened without widening the comparison surface itself. `profile_identity` binds the observation to the exact
/// profile instance that produced it, so [`classify_observations`] can refuse to compare observations of two
/// different profiles instead of reporting a meaningless verdict.
#[derive(Debug, Clone)]
pub struct RouteObservation {
    /// Stable kind of comparison profile this observation was produced under.
    pub profile_kind: String,
    /// Content identity of the exact profile instance this observation was produced under.
    pub profile_identity: String,
    /// The compared termination/result outcome, in addition to the two program streams below.
    pub observable: SourceObservable,
    /// Exact program stdout bytes, with order preserved within this stream.
    pub stdout: Vec<u8>,
    /// Exact program stderr bytes, independent of stdout and without lossily decoding diagnostics.
    pub stderr: Vec<u8>,
    /// Route-specific evidence retained for reporting only.
    pub detail: String,
    /// Content identity of what this route produced, bound into its receipt.
    pub output_identity: String,
}

impl RouteObservation {
    /// Describe the complete comparison surface without expanding arbitrarily large program streams.
    ///
    /// Exact bytes remain available in the observation; receipts and diagnostic summaries bind their lengths and
    /// digests alongside the source-level outcome. Stream equality itself always compares bytes, not these labels.
    fn describe(&self) -> String {
        format!(
            "{}; stdout={} bytes ({}); stderr={} bytes ({})",
            self.observable.describe(),
            self.stdout.len(),
            crate::oven::digest_bytes(&self.stdout),
            self.stderr.len(),
            crate::oven::digest_bytes(&self.stderr),
        )
    }
}

/// A comparison that could not run, with the concrete boundary that stopped it.
///
/// Folded into [`ShadowComparisonState::Unavailable`], which is always non-green.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{reason}")]
pub struct ShadowUnavailable {
    /// Concrete, actionable explanation of why no comparison ran.
    pub reason: String,
}

impl ShadowUnavailable {
    /// Record one concrete reason a comparison could not run.
    #[must_use]
    pub fn new(reason: impl Into<String>) -> Self {
        Self { reason: reason.into() }
    }
}

// ============================================================================
// Route evidence and the comparison result
// ============================================================================

/// The Oven authority that permitted one legacy-route execution.
///
/// Bound into the legacy receipt's output identity so a reader can tell which project receipt, reusable build
/// unit, and immutable direct-`rustc` plan produced the observed answer. Without it a legacy observation would be
/// an unattributed process result rather than an Oven-owned execution.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct LegacyExecutionAuthority {
    /// Identity of the verified Oven receipt that authorized the emitted source.
    pub oven_receipt_identity: String,
    /// Identity of the reusable Oven build unit the plan was selected against.
    pub oven_build_unit_identity: String,
    /// Store identity of the immutable direct-`rustc` plan that compiled the program.
    pub direct_rustc_plan_identity: String,
    /// Digest of the caller-owned native output Oven produced.
    pub output_digest: String,
    /// Whether any Cargo process participated; Oven-owned execution requires `false`.
    pub cargo_process_started: bool,
}

/// Raw evidence from one Oven-produced legacy process.
///
/// Stdout and stderr remain independent, unmodified byte streams. The optional report is separate because it is a
/// source-authored result transport, read only after a successful process exit and only after the staged file was
/// atomically replaced. A process that exits unsuccessfully still retains its streams but has no report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyProcessEvidence {
    /// Native process exit code, or `None` when the platform did not provide one.
    pub exit_code: Option<i32>,
    /// Raw bytes the legacy program wrote to stdout.
    pub stdout: Vec<u8>,
    /// Raw bytes the legacy program wrote to stderr.
    pub stderr: Vec<u8>,
    /// Raw typed-result report bytes, present only after a successful exit and successful host read.
    pub result_report: Option<Vec<u8>>,
}

/// Everything one route contributed to a comparison.
///
/// Present whenever the route actually executed. `receipt` is optional only because receipt finalization can
/// itself fail; when it does, the observation is still reported and the comparison state names the failure, so an
/// execution that really happened is never silently discarded.
#[derive(Debug, Clone)]
pub struct RouteEvidence {
    /// The #986 receipt binding this route's execution to its declared selection, when one could be finalized.
    pub receipt: Option<BackendExecutionReceipt>,
    /// The observation the comparison was made over.
    pub observation: RouteObservation,
}

impl RouteEvidence {
    /// The finalized receipt, or the reason this executed route has none.
    ///
    /// Callers that need a receipt should use this rather than unwrapping the option, so the missing-receipt case
    /// reads as the explicit failure it is.
    pub fn receipt(&self) -> Result<&BackendExecutionReceipt, String> {
        self.receipt.as_ref().ok_or_else(|| {
            format!(
                "the {} route executed but its receipt could not be finalized",
                self.observation.profile_kind
            )
        })
    }
}

/// The result of one bounded shadow comparison.
///
/// A route is present exactly when it actually executed. That is deliberate: an unavailable comparison whose
/// replacement route ran still carries that route's receipt and Body-IR evidence, so a staging gap is
/// distinguishable from a source the replacement backend cannot execute at all.
#[derive(Debug, Clone)]
pub struct ShadowComparison {
    /// Stable kind of comparison profile this comparison was declared for.
    pub profile_kind: String,
    /// Content identity of the exact profile instance this comparison was declared for.
    pub profile_identity: String,
    /// Source identity both routes were selected for.
    pub source_identity: String,
    /// The recorded outcome, also carried inside every produced receipt.
    pub state: ShadowComparisonState,
    /// Legacy-route evidence, present only when the legacy program actually ran.
    pub legacy: Option<RouteEvidence>,
    /// Replacement-route evidence, present only when direct execution actually ran.
    pub replacement: Option<RouteEvidence>,
    /// The direct Body-IR execution behind [`ShadowComparison::replacement`], when it completed normally.
    pub replacement_execution: Option<ReplacementExecution>,
    /// Direct-route stream observation after any execution attempt, including an unclassifiable failure.
    ///
    /// This is independent of a successful execution product or finalized receipt. Pre-execution refusals have no
    /// observation; bytes accepted before a runtime failure remain available even when comparison is unavailable.
    pub replacement_output: Option<ProgramOutput>,
    /// The Oven authority behind [`ShadowComparison::legacy`], when the legacy route ran.
    pub legacy_authority: Option<LegacyExecutionAuthority>,
    /// Raw legacy process streams and its separate typed-result report, when that process ran.
    pub legacy_process: Option<LegacyProcessEvidence>,
}

impl ShadowComparison {
    /// Whether both routes ran independently and agreed on the compared observable.
    ///
    /// This is the only state that may promote anything to green; `Unavailable` and `Diverged` must not.
    #[must_use]
    pub fn matched(&self) -> bool {
        matches!(self.state, ShadowComparisonState::Matched { .. })
    }

    /// The recorded reason when no comparison was made, if this comparison is unavailable.
    #[must_use]
    pub fn unavailable_reason(&self) -> Option<&str> {
        match &self.state {
            ShadowComparisonState::Unavailable { reason } => Some(reason.as_str()),
            _ => None,
        }
    }
}

/// Compare a profile using a source session that has already selected its provider projection.
///
/// This remains crate-private so the public comparison API cannot accept provider or receipt data detached from the
/// source it observes. Unit tests use it only to exercise refusal edges with deliberately forged materialization
/// facts; production callers must supply materialization selected from the exact source session they observe.
#[must_use]
pub(crate) fn compare_source_observable_with_materialization(
    profile: &ShadowComparisonProfile,
    materialization: &ShadowLegacyMaterialization,
    capability: &legacy_oven::LegacyOvenCapability,
    workspace: &Path,
) -> ShadowComparison {
    // The source, its Body IR, and its scalar return kind are all checked before either route executes. The
    // source-authored report wrapper is preflighted through the same legacy materialization path before direct
    // execution too, so a wrapper capability gap is an honest profile refusal rather than an asymmetrical attempt.
    if let Err(unavailable) = materialization.require_profile_source(profile) {
        return assemble_comparison(
            profile,
            profile.profile_identity(),
            Err(unavailable.clone()),
            Err(unavailable),
        );
    }
    let prepared = match PreparedShadowProfile::new(profile) {
        Ok(prepared) => prepared,
        Err(unavailable) => {
            return assemble_comparison(
                profile,
                profile.profile_identity(),
                Err(unavailable.clone()),
                Err(unavailable),
            );
        }
    };
    if let Err(unavailable) = capability.require_materialization_compatibility(materialization) {
        return assemble_comparison(
            profile,
            profile.profile_identity(),
            Err(unavailable.clone()),
            Err(unavailable),
        );
    }
    if let Err(unavailable) = preflight_result_transport(profile, &prepared, materialization, workspace) {
        return assemble_comparison(
            profile,
            profile.profile_identity(),
            Err(unavailable.clone()),
            Err(unavailable),
        );
    }

    // ---- Route 1: direct Body IR, no generation and no subprocess ----
    let replacement = observe_replacement_route(profile, &prepared);

    // ---- Route 2: emitted Rust, Oven-authorized native build, executed as a process ----
    let legacy = legacy_oven::observe_legacy_route(profile, &prepared, materialization, capability, workspace);

    assemble_comparison(profile, profile.profile_identity(), replacement, legacy)
}

/// Validate the source-observable profile before a caller prepares external execution authority.
///
/// Keeping this preflight in the backend ensures invalid source refuses before the CLI adapter materializes files or
/// resolves providers, while the backend remains independent of CLI session construction.
pub(crate) fn validate_source_observable_profile(profile: &ShadowComparisonProfile) -> Result<(), ShadowUnavailable> {
    PreparedShadowProfile::new(profile).map(|_| ())
}

/// Record a pre-execution refusal for both routes without manufacturing execution evidence.
#[must_use]
pub(crate) fn unavailable_source_observable_comparison(
    profile: &ShadowComparisonProfile,
    unavailable: ShadowUnavailable,
) -> ShadowComparison {
    assemble_comparison(
        profile,
        profile.profile_identity(),
        Err(unavailable.clone()),
        Err(unavailable),
    )
}

/// Fold both routes' results into one comparison, retaining every route that actually executed.
///
/// Separated from the source-session adapter so the assembly rules — classification, receipt binding, and
/// partial-evidence retention — stay testable without staging an Oven capability.
fn assemble_comparison(
    profile: &ShadowComparisonProfile,
    profile_identity: String,
    replacement: Result<ReplacementRouteResult, ShadowUnavailable>,
    legacy: Result<LegacyRouteResult, ShadowUnavailable>,
) -> ShadowComparison {
    let profile_kind = profile.profile_kind().to_string();
    let source_identity = profile.source_identity();

    let (replacement_observation, replacement_execution, replacement_output, replacement_unavailable) =
        match replacement {
            Ok(observed) => (
                observed.observation,
                observed.execution,
                Some(observed.output),
                observed.unavailable_reason,
            ),
            Err(unavailable) => (None, None, None, Some(unavailable.reason)),
        };
    let (legacy_observation, legacy_authority, legacy_process, legacy_unavailable) = match legacy {
        Ok(observed) => (
            observed.observation,
            Some(observed.authority),
            Some(observed.process),
            observed.unavailable_reason,
        ),
        Err(unavailable) => (None, None, None, Some(unavailable.reason)),
    };

    let state = match (&legacy_observation, &replacement_observation) {
        (Some(legacy), Some(replacement)) => classify_observations(legacy, replacement),
        _ => ShadowComparisonState::Unavailable {
            reason: unavailable_reason(legacy_unavailable, replacement_unavailable),
        },
    };

    // Receipts embed the comparison state, so the state must be settled before any receipt is finalized. A
    // receipt that cannot be finalized makes the comparison unverifiable and withdraws the verdict, so
    // finalization is probed first and only then performed against the settled state. Probing is pure hashing;
    // doing it twice is cheaper than publishing a receipt that claims a verdict the comparison no longer holds.
    let mut receipt_failures = Vec::new();
    for (backend, label, observation) in [
        (BackendKind::Legacy, "legacy", legacy_observation.as_ref()),
        (
            BackendKind::Replacement,
            "replacement",
            replacement_observation.as_ref(),
        ),
    ] {
        let Some(observation) = observation else {
            continue;
        };
        if let Err(error) = route_receipt(&source_identity, backend, observation, &state) {
            receipt_failures.push(format!(
                "the {label} route executed and observed {} but its receipt could not be finalized: {error}",
                observation.observable.describe()
            ));
        }
    }

    let state = if receipt_failures.is_empty() {
        state
    } else {
        ShadowComparisonState::Unavailable {
            reason: format!(
                "both routes' observations were produced, but a receipt could not be finalized, so the comparison \
                 is not verifiable: {}",
                receipt_failures.join("; ")
            ),
        }
    };

    // Every route that executed keeps its evidence, including under an unavailable state: discarding a real
    // execution would make a staging gap indistinguishable from a backend that could not run the source at all.
    let legacy_evidence = route_evidence(&source_identity, BackendKind::Legacy, legacy_observation, &state);
    let replacement_evidence = route_evidence(
        &source_identity,
        BackendKind::Replacement,
        replacement_observation,
        &state,
    );

    ShadowComparison {
        profile_kind,
        profile_identity,
        source_identity,
        state,
        legacy: legacy_evidence,
        replacement: replacement_evidence,
        replacement_execution,
        replacement_output,
        legacy_authority,
        legacy_process,
    }
}

/// Bind one executed route's observation to its own receipt, keeping the observation either way.
///
/// A route that did not run contributes nothing. A route that ran always contributes its observation, even when
/// its receipt cannot be finalized — the caller has already withdrawn the verdict and recorded why, and dropping
/// the evidence here would hide that the route executed at all.
fn route_evidence(
    source_identity: &str,
    backend: BackendKind,
    observation: Option<RouteObservation>,
    state: &ShadowComparisonState,
) -> Option<RouteEvidence> {
    let observation = observation?;
    let receipt = route_receipt(source_identity, backend, &observation, state).ok();
    Some(RouteEvidence { receipt, observation })
}

/// Compose the recorded reason for a comparison that could not run, naming every route that failed to execute.
fn unavailable_reason(legacy: Option<String>, replacement: Option<String>) -> String {
    match (legacy, replacement) {
        (Some(legacy), Some(replacement)) => {
            format!(
                "neither route produced a comparable observation; legacy route: {legacy}; replacement route: {replacement}"
            )
        }
        (Some(legacy), None) => format!("the legacy route observation is unavailable: {legacy}"),
        (None, Some(replacement)) => format!("the replacement route observation is unavailable: {replacement}"),
        (None, None) => "no comparison was made and neither route reported a reason".to_string(),
    }
}

/// Decide whether two independent observations of the same profile agree.
///
/// The source-level outcome and both raw byte streams must agree, after confirming the same profile instance.
/// Comparing different profiles would manufacture a verdict about a comparison nobody ran. No total ordering
/// between the independent streams is inferred.
#[must_use]
pub fn classify_observations(legacy: &RouteObservation, replacement: &RouteObservation) -> ShadowComparisonState {
    if legacy.profile_kind != replacement.profile_kind || legacy.profile_identity != replacement.profile_identity {
        return ShadowComparisonState::Unavailable {
            reason: format!(
                "the two observations describe different comparison profiles ({}/{} and {}/{}), so no comparison \
                 over one source was made",
                legacy.profile_kind, legacy.profile_identity, replacement.profile_kind, replacement.profile_identity
            ),
        };
    }
    if legacy.observable == replacement.observable
        && legacy.stdout == replacement.stdout
        && legacy.stderr == replacement.stderr
    {
        return ShadowComparisonState::Matched {
            profile_kind: legacy.profile_kind.clone(),
            profile_identity: legacy.profile_identity.clone(),
            observable: legacy.describe(),
        };
    }
    ShadowComparisonState::Diverged {
        profile_kind: legacy.profile_kind.clone(),
        profile_identity: legacy.profile_identity.clone(),
        detail: format!(
            "legacy route observed {} and replacement route observed {}; legacy detail: {}; replacement detail: {}",
            legacy.describe(),
            replacement.describe(),
            legacy.detail,
            replacement.detail
        ),
    }
}

/// A completed direct execution plus the observation derived from it.
pub(crate) struct ReplacementRouteResult {
    pub(crate) observation: Option<RouteObservation>,
    pub(crate) execution: Option<ReplacementExecution>,
    pub(crate) output: ProgramOutput,
    pub(crate) unavailable_reason: Option<String>,
}

/// A completed Oven-owned legacy process plus its authority and retained raw evidence.
pub(crate) struct LegacyRouteResult {
    pub(crate) observation: Option<RouteObservation>,
    pub(crate) authority: LegacyExecutionAuthority,
    pub(crate) process: LegacyProcessEvidence,
    /// A post-process report or classification refusal that must not discard the process's raw evidence.
    pub(crate) unavailable_reason: Option<String>,
}

/// Prepared direct-execution facts shared by both routes before either begins execution.
pub(crate) struct PreparedShadowProfile {
    body_ir: BodyIrModule,
    result_kind: FunctionResultKind,
    wrapper_identifiers: GeneratedWrapperIdentifiers,
}

impl PreparedShadowProfile {
    /// Check the selected function and reserve collision-free transport names before either route executes.
    fn new(profile: &ShadowComparisonProfile) -> Result<Self, ShadowUnavailable> {
        if profile.function == "main" {
            return Err(ShadowUnavailable::new(PROGRAM_ENTRYPOINT_UNAVAILABLE_REASON));
        }
        let program = profile.program_after_input_contract()?;
        refuse_source_process_import(&program)?;
        let function_declaration = program
            .declarations
            .iter()
            .find(|declaration| {
                matches!(
                    &declaration.node,
                    crate::frontend::ast::Declaration::Function(function) if function.name == profile.function
                )
            })
            .ok_or_else(|| {
                ShadowUnavailable::new(format!(
                    "the comparison profile's function `{}` is absent from the manifest-free feature projection, \
                     so neither route may observe it",
                    profile.function
                ))
            })?;
        let module_path = vec!["incan_shadow_comparison".to_string()];
        let mut checker = TypeChecker::new();
        checker.set_current_module_path(Some(module_path.clone()));
        checker
            .check_program(&program)
            .map_err(|errors| ShadowUnavailable::new(format!("comparison source did not typecheck: {errors:?}")))?;
        refuse_source_main(&checker)?;
        let wrapper_identifiers = GeneratedWrapperIdentifiers::fresh_from_checked_source(&checker)?;
        let body_ir = build_body_ir_module_v0(&program, &module_path, checker.type_info());
        prepare_free_function_execution(&body_ir, profile.function(), &profile.arguments)
            .map_err(replacement_profile_unavailable)?;
        let selected_body = body_ir
            .bodies
            .iter()
            .find(|body| {
                body.span.start == function_declaration.span.start && body.span.end == function_declaration.span.end
            })
            .ok_or_else(|| {
                ShadowUnavailable::new(format!(
                    "Body IR did not retain the checked declaration for comparison function `{}`",
                    profile.function
                ))
            })?;
        let result_kind = function_result_kind(&selected_body.return_type, &profile.function)?;
        Ok(Self {
            body_ir,
            result_kind,
            wrapper_identifiers,
        })
    }
}

/// Project the selected Body-IR declaration's checked return fact into the result-report grammar.
fn function_result_kind(ty: &IncanType, function: &str) -> Result<FunctionResultKind, ShadowUnavailable> {
    match ty {
        IncanType::Primitive(IncanPrimitiveType::Int) => Ok(FunctionResultKind::Int),
        IncanType::Primitive(IncanPrimitiveType::Float) => Ok(FunctionResultKind::Float),
        IncanType::Primitive(IncanPrimitiveType::Numeric(kind)) if *kind != NumericTypeId::Bool => {
            Ok(FunctionResultKind::Numeric(*kind))
        }
        IncanType::Decimal { precision, scale } => Ok(FunctionResultKind::Decimal {
            precision: *precision,
            scale: *scale,
        }),
        IncanType::Primitive(IncanPrimitiveType::Bool) => Ok(FunctionResultKind::Bool),
        IncanType::Primitive(IncanPrimitiveType::Str) => Ok(FunctionResultKind::Str),
        IncanType::Primitive(IncanPrimitiveType::Unit) => Ok(FunctionResultKind::Unit),
        other => Err(ShadowUnavailable::new(format!(
            "the bounded source-observable comparison profile requires a checked scalar or `None` return type; \
             `{function}` returns {other:?}"
        ))),
    }
}

/// Refuse a source entrypoint that would be rebound when the legacy wrapper is appended.
///
/// The source is checked first, so this consults the compiler's root binding table rather than guessing from source
/// spelling. It covers root declarations and imports after the manifest-free input contract; local bindings remain
/// valid because the generated `main` owns its own scope.
fn refuse_source_main(checker: &TypeChecker) -> Result<(), ShadowUnavailable> {
    if checker.lookup_symbol("main").is_some() {
        return Err(ShadowUnavailable::new(PROGRAM_ENTRYPOINT_UNAVAILABLE_REASON));
    }
    Ok(())
}

/// Keep private result-transport statuses distinct from user-program failure behavior.
///
/// This bounded source-only profile has no process-entrypoint comparison contract. Direct Rust process imports would
/// let source intentionally produce the wrapper's statuses, so reject them from parsed source before either route
/// executes. The generated wrapper adds its own fresh process alias only after this check.
fn refuse_source_process_import(program: &crate::frontend::ast::Program) -> Result<(), ShadowUnavailable> {
    let imports_process = program.declarations.iter().any(|declaration| {
        let crate::frontend::ast::Declaration::Import(import) = &declaration.node else {
            return false;
        };
        match &import.kind {
            crate::frontend::ast::ImportKind::RustCrate { crate_name, path, .. } => {
                crate_name == "std" && (path.is_empty() || path.first().is_some_and(|segment| segment == "process"))
            }
            crate::frontend::ast::ImportKind::RustFrom {
                crate_name,
                path,
                items,
                ..
            } => {
                crate_name == "std"
                    && (path.first().is_some_and(|segment| segment == "process")
                        || (path.is_empty() && items.iter().any(|item| item.name == "process")))
            }
            _ => false,
        }
    });
    if imports_process {
        return Err(ShadowUnavailable::new(
            "the bounded source-observable comparison profile reserves direct `rust::std::process` imports for its private result-transport failure statuses",
        ));
    }
    Ok(())
}

/// Typecheck and emit one non-executed result-report wrapper before either route begins execution.
fn preflight_result_transport(
    profile: &ShadowComparisonProfile,
    prepared: &PreparedShadowProfile,
    materialization: &ShadowLegacyMaterialization,
    workspace: &Path,
) -> Result<(), ShadowUnavailable> {
    // The wrapper only compiles here; nevertheless, give it a caller-managed path so this generated source never
    // advertises a broad fixed report destination.
    let report_path = workspace.join("incan-shadow-result-preflight");
    let program = profile.legacy_program_source(prepared.result_kind, &report_path, &prepared.wrapper_identifiers)?;
    let project_root = workspace.join("incan-shadow-result-preflight-project");
    legacy_oven::materialize_legacy_program(&program, materialization, &project_root)
        .map(|_| ())
        .map_err(|unavailable| {
            ShadowUnavailable::new(format!(
                "the bounded source-authored result-report wrapper could not be preflighted through the legacy \
                 materialization path: {}",
                unavailable.reason
            ))
        })
}

/// Observe the replacement route by executing the already checked Body IR.
///
/// Refusals from the bounded #988 profile (unsupported construct, wrong arity, missing function) mean this
/// profile instance is outside the comparison, not that the routes disagreed, so they surface as
/// [`ShadowUnavailable`].
fn observe_replacement_route(
    profile: &ShadowComparisonProfile,
    prepared: &PreparedShadowProfile,
) -> Result<ReplacementRouteResult, ShadowUnavailable> {
    let profile_kind = profile.profile_kind().to_string();
    let profile_identity = profile.profile_identity();
    let plan = prepare_free_function_execution(&prepared.body_ir, profile.function(), &profile.arguments)
        .map_err(replacement_profile_unavailable)?;
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut io = ProgramIo::new(&mut stdout, &mut stderr);
    let executed = execute_prevalidated_free_function_with_io(plan, &mut io);
    let output = io.output().clone();
    let (execution, observed) = match executed {
        Ok(execution) => {
            let observation = prepared
                .result_kind
                .observe_replacement_value(&execution.value)
                .map(|result| RouteObservation {
                    profile_kind,
                    profile_identity,
                    observable: SourceObservable::Completed { result },
                    stdout: output.stdout().to_vec(),
                    stderr: output.stderr().to_vec(),
                    detail: format!("direct Body-IR execution of `{}`", profile.function()),
                    output_identity: execution.output_identity.clone(),
                });
            (Some(execution), observation)
        }
        Err(ReplacementExecutionError::RuntimeFailure { detail, .. }) => {
            let observation = classify_replacement_failure(&detail).map(|failure| RouteObservation {
                profile_kind,
                profile_identity,
                observable: SourceObservable::Failed { failure },
                stdout: output.stdout().to_vec(),
                stderr: output.stderr().to_vec(),
                detail: format!("direct Body-IR execution failed: {detail}"),
                output_identity: digest_output(&[
                    REPLACEMENT_FAILURE_IDENTITY_TAG,
                    &detail,
                    &crate::oven::digest_bytes(output.stdout()),
                    &crate::oven::digest_bytes(output.stderr()),
                ]),
            });
            (None, observation)
        }
        Err(error) => (None, Err(replacement_profile_unavailable(error))),
    };
    let (observation, unavailable_reason) = match observed {
        Ok(observation) => (Some(observation), None),
        Err(unavailable) => (None, Some(unavailable.reason)),
    };
    Ok(ReplacementRouteResult {
        observation,
        execution,
        output,
        unavailable_reason,
    })
}

/// Fold a replacement-profile refusal into an unavailable reason that keeps the original source span.
fn replacement_profile_unavailable(error: ReplacementExecutionError) -> ShadowUnavailable {
    ShadowUnavailable::new(format!(
        "the replacement route cannot execute this profile instance: {error}"
    ))
}

/// Emit Rust for the legacy route's derived program through the real legacy backend and caller-owned provider plan.
pub(crate) fn emit_legacy_rust_with_materialization(
    program: &str,
    materialization: &ShadowLegacyMaterialization,
) -> Result<String, ShadowUnavailable> {
    emit_legacy_rust_with_provider_plan(program, Some(materialization.provider_plan()))
}

/// Emit legacy Rust while optionally configuring a source-backed test fixture provider plan.
fn emit_legacy_rust_with_provider_plan(
    program: &str,
    provider_plan: Option<&Arc<ProviderPlan>>,
) -> Result<String, ShadowUnavailable> {
    let tokens = lexer::lex(program)
        .map_err(|errors| ShadowUnavailable::new(format!("the legacy program did not lex: {errors:?}")))?;
    let ast = parser::parse(&tokens)
        .map_err(|errors| ShadowUnavailable::new(format!("the legacy program did not parse: {errors:?}")))?;
    crate::frontend::typechecker::check(&ast)
        .map_err(|errors| ShadowUnavailable::new(format!("the legacy program did not typecheck: {errors:?}")))?;
    let mut codegen = IrCodegen::new();
    if let Some(provider_plan) = provider_plan {
        codegen.set_provider_plan(Arc::clone(provider_plan));
    }
    codegen
        .try_generate(&ast)
        .map_err(|error| ShadowUnavailable::new(format!("the legacy backend could not emit Rust: {error}")))
}

#[cfg(test)]
/// Emit a provider-free legacy source fixture for unit tests that do not materialize or execute a native program.
pub(crate) fn emit_legacy_rust(program: &str) -> Result<String, ShadowUnavailable> {
    emit_legacy_rust_with_provider_plan(program, None)
}

/// Turn one completed legacy process result into a comparable observation.
///
/// A successful exit must carry the separate exact typed report; a failing exit must carry a classifiable stderr
/// diagnostic. Neither an exit status nor any program stream is read as a return value.
pub(crate) fn observe_legacy_process(
    profile_kind: &str,
    profile_identity: &str,
    authority: &LegacyExecutionAuthority,
    process: &LegacyProcessEvidence,
    result_kind: FunctionResultKind,
) -> Result<RouteObservation, ShadowUnavailable> {
    if let Some(reason) = result_transport_failure_reason(process.exit_code) {
        return Err(ShadowUnavailable::new(reason));
    }
    let observable = if process.exit_code == Some(0) {
        let report = process.result_report.as_deref().ok_or_else(|| {
            ShadowUnavailable::new(
                "the legacy process exited successfully but did not publish its source-authored result report",
            )
        })?;
        SourceObservable::Completed {
            result: decode_result_report(report, result_kind)?,
        }
    } else {
        let stderr = std::str::from_utf8(&process.stderr).map_err(|error| {
            ShadowUnavailable::new(format!(
                "the failed legacy process wrote non-UTF-8 stderr, so this profile cannot classify its diagnostic: \
                 {error}"
            ))
        })?;
        SourceObservable::Failed {
            failure: classify_legacy_failure(stderr)?,
        }
    };
    let stdout_digest = crate::oven::digest_bytes(&process.stdout);
    let stderr_digest = crate::oven::digest_bytes(&process.stderr);
    let report_digest = process
        .result_report
        .as_deref()
        .map(crate::oven::digest_bytes)
        .unwrap_or_else(|| "none".to_string());
    let detail = format!(
        "Oven-owned legacy process exited with {:?} under direct-rustc plan {}; raw stdout {}, stderr {}, and \
         result report {}",
        process.exit_code, authority.direct_rustc_plan_identity, stdout_digest, stderr_digest, report_digest
    );
    let output_identity = digest_output(&[
        LEGACY_OUTPUT_IDENTITY_TAG,
        authority.oven_receipt_identity.as_str(),
        authority.oven_build_unit_identity.as_str(),
        authority.direct_rustc_plan_identity.as_str(),
        authority.output_digest.as_str(),
        &format!("{:?}", process.exit_code),
        stdout_digest.as_str(),
        stderr_digest.as_str(),
        report_digest.as_str(),
        observable.describe().as_str(),
    ]);
    Ok(RouteObservation {
        profile_kind: profile_kind.to_string(),
        profile_identity: profile_identity.to_string(),
        observable,
        stdout: process.stdout.clone(),
        stderr: process.stderr.clone(),
        detail,
        output_identity,
    })
}

/// Recognize the generated wrapper's private report-publication statuses before a non-zero process is interpreted
/// as a source failure from stderr.
fn result_transport_failure_reason(exit_code: Option<i32>) -> Option<&'static str> {
    match exit_code {
        Some(RESULT_TRANSPORT_WRITE_EXIT_STATUS) => Some(
            "the legacy process stopped because its source-authored result transport could not write the staged report",
        ),
        Some(RESULT_TRANSPORT_RENAME_EXIT_STATUS) => Some(
            "the legacy process stopped because its source-authored result transport could not atomically rename the staged report",
        ),
        _ => None,
    }
}

/// Classify a legacy process failure into a class the replacement route can also report.
///
/// Generated programs install a panic hook that prints the panic payload, so the panic message is the source-level
/// evidence here. Canonical conversion payloads are recognized first because their rejected input may contain any
/// diagnostic words. Overflow is tested before division because a division-overflow panic also names division.
/// An unrecognized failure is unavailable, not a shared failure observation.
fn classify_legacy_failure(stderr: &str) -> Result<RuntimeFailureClass, ShadowUnavailable> {
    if let Some(failure) = classify_canonical_exact_float_failure(stderr) {
        return Ok(failure);
    }
    if let Some(failure) = classify_canonical_conversion_failure(stderr) {
        return Ok(failure);
    }
    let lowered = stderr.to_lowercase();
    if lowered.contains("assertionerror") || lowered.contains("assertion failed") {
        return Ok(RuntimeFailureClass::Assertion);
    }
    if lowered.contains("overflow") {
        return Ok(RuntimeFailureClass::ArithmeticOverflow);
    }
    if lowered.contains("zerodivisionerror")
        || lowered.contains("divide by zero")
        || lowered.contains("division by zero")
        || lowered.contains("remainder with a divisor of zero")
    {
        return Ok(RuntimeFailureClass::DivisionByZero);
    }
    Err(ShadowUnavailable::new(format!(
        "the legacy program failed in a way this profile cannot classify, so agreement cannot be claimed: {stderr}"
    )))
}

/// Classify a direct-execution runtime failure into the same class vocabulary as the legacy route.
///
/// Canonical conversion payloads are recognized before inspecting their rejected inputs for diagnostic words.
/// Other failures retain the same ordering as the legacy route: an "integer division overflow" is an overflow,
/// not a division by zero.
fn classify_replacement_failure(detail: &str) -> Result<RuntimeFailureClass, ShadowUnavailable> {
    if let Some(failure) = classify_canonical_exact_float_failure(detail) {
        return Ok(failure);
    }
    if let Some(failure) = classify_canonical_conversion_failure(detail) {
        return Ok(failure);
    }
    let lowered = detail.to_lowercase();
    if lowered.contains("assertion") {
        return Ok(RuntimeFailureClass::Assertion);
    }
    if lowered.contains("overflow") {
        return Ok(RuntimeFailureClass::ArithmeticOverflow);
    }
    if lowered.contains("division or modulo by zero") {
        return Ok(RuntimeFailureClass::DivisionByZero);
    }
    Err(ShadowUnavailable::new(format!(
        "the replacement route failed in a way this profile cannot classify, so agreement cannot be claimed: \
         {detail}"
    )))
}

/// Recognize the finite-only boundary shared by generated Rust and direct execution.
fn classify_canonical_exact_float_failure(detail: &str) -> Option<RuntimeFailureClass> {
    match detail.strip_suffix('\n').unwrap_or(detail) {
        "ValueError: non-finite float cannot initialize exact f32" => Some(RuntimeFailureClass::NonFiniteExactF32),
        "ValueError: non-finite float cannot initialize exact f64" => Some(RuntimeFailureClass::NonFiniteExactF64),
        _ => None,
    }
}

/// Recognize one complete canonical scalar-conversion payload before inspecting its rejected input for heuristic words.
///
/// The native panic hook appends one line feed to the payload while the direct executor retains the payload itself.
/// Removing at most that one framing byte lets both routes use the same exact grammar without trimming source data.
/// The input between the fixed prefix and suffix remains opaque: words such as `AssertionError`, `overflow`, and
/// `division by zero` describe the rejected string, not the failure class.
fn classify_canonical_conversion_failure(detail: &str) -> Option<RuntimeFailureClass> {
    let message = detail.strip_suffix('\n').unwrap_or(detail);
    let rejected_input = message.strip_prefix("ValueError: cannot convert '")?;
    if rejected_input.strip_suffix("' to int").is_some() {
        return Some(RuntimeFailureClass::IntConversion);
    }
    if rejected_input.strip_suffix("' to float").is_some() {
        return Some(RuntimeFailureClass::FloatConversion);
    }
    None
}

// ============================================================================
// Receipt binding
// ============================================================================

/// Declare, resolve, and finalize one route's #986 receipt for a comparison.
///
/// Every route is declared explicitly with [`FallbackPolicy::Refuse`], so a receipt from this module can never
/// record one backend standing in for the other — which is exactly the substitution a shadow comparison exists to
/// rule out.
fn route_receipt(
    source_identity: &str,
    backend: BackendKind,
    observation: &RouteObservation,
    state: &ShadowComparisonState,
) -> Result<BackendExecutionReceipt, crate::backend::selection::BackendSelectionError> {
    let selection: BackendSelection = select_backend(backend, true, true, source_identity, FallbackPolicy::Refuse);
    let executed = resolve_execution(&selection, backend.is_implemented())?;
    finalize_receipt(
        &selection,
        executed,
        observation.output_identity.clone(),
        state.clone(),
        DIAGNOSTIC_SCHEMA_VERSION,
    )
}

#[cfg(test)]
mod len_string_tests;
#[cfg(test)]
mod tests;

#[cfg(test)]
mod abs_sum_profile_tests;

#[cfg(all(test, feature = "cli"))]
mod enumerate_zip_tests;

#[cfg(test)]
mod json_stringify_tests;
