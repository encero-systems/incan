//! Result variants, callable parameters and defaults, closures and generators.

use super::*;

/// One direct compiler-owned `Result` construction with its checked payload and error facts retained.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResultVariant {
    /// Which intrinsic Result constructor source checking selected.
    pub kind: ResultVariantKind,
    /// The one source-order payload operand.
    pub payload: Operand,
    /// Checked `Result` success type.
    pub ok_type: IncanType,
    /// Checked `Result` error type.
    pub error_type: IncanType,
    /// Checked canonical identities of named leaves, with paths rooted at Result argument 0 (Ok) or 1 (Err).
    ///
    /// Nested tuple/list arguments append their zero-based child index. Names in `ok_type` and `error_type` remain
    /// diagnostic spelling; these identities authorize nominal payloads across aliases and package boundaries.
    pub canonical_types: std::collections::BTreeMap<Vec<usize>, CanonicalSymbolId>,
}

impl ResultVariant {
    /// Require exactly one declaration identity for every named payload-type leaf before execution. The exact
    /// builtin ValidationError and checked native item identities use the same admitted runtime carrier boundary
    /// as nominal declarations; arbitrary names and malformed identities confer no executable layout.
    pub fn has_complete_canonical_types(&self) -> bool {
        /// Collect named leaves using the same structural child positions retained in the type facts.
        fn paths(
            ty: &IncanType,
            path: &mut Vec<usize>,
            out: &mut std::collections::BTreeMap<Vec<usize>, Option<crate::RuntimeBuiltinType>>,
        ) {
            match ty {
                IncanType::Named(_) => {
                    out.insert(path.clone(), None);
                }
                IncanType::NativeBuiltin(builtin) => {
                    out.insert(path.clone(), Some(builtin.clone()));
                }
                IncanType::Tuple(items) | IncanType::Generic { args: items, .. } => {
                    for (index, item) in items.iter().enumerate() {
                        path.push(index);
                        paths(item, path, out);
                        path.pop();
                    }
                }
                IncanType::Ref(inner) | IncanType::RefMut(inner) | IncanType::NativeArray { element: inner, .. } => {
                    path.push(0);
                    paths(inner, path, out);
                    path.pop();
                }
                _ => {}
            }
        }
        let mut required = std::collections::BTreeMap::new();
        paths(&self.ok_type, &mut vec![0], &mut required);
        paths(&self.error_type, &mut vec![1], &mut required);
        required.len() == self.canonical_types.len()
            && required.iter().all(|(path, builtin)| {
                self.canonical_types.get(path).is_some_and(|identity| {
                    if let Some(builtin) = builtin {
                        let selected = crate::RuntimeBuiltinType::from_identity(identity).or_else(|| {
                            native_nominal_item(identity)
                                .and_then(|path| crate::RuntimeBuiltinType::from_carrier_path(&path))
                        });
                        return selected.as_ref() == Some(builtin);
                    }
                    identity.scope_discriminant.is_none()
                        && ((matches!(
                            identity.kind,
                            crate::SemanticSourceTargetKind::Model
                                | crate::SemanticSourceTargetKind::Class
                                | crate::SemanticSourceTargetKind::Enum
                                | crate::SemanticSourceTargetKind::Newtype
                        ) && matches!(
                            identity.origin,
                            crate::SymbolOrigin::Module(_) | crate::SymbolOrigin::Package { .. }
                        )) || (matches!(
                            identity.origin,
                            crate::SymbolOrigin::Builtin | crate::SymbolOrigin::RustCrate(_)
                        ) && native_nominal_item(identity).is_some()))
                })
            })
    }

    /// Render the construction and checked type facts without consulting any target-language Result spelling.
    pub(super) fn render_snapshot(&self) -> String {
        format!(
            "result_{}({}, ok_type={}, error_type={}, canonical_types=[{}])",
            self.kind.as_str(),
            self.payload.render_snapshot(),
            self.ok_type,
            self.error_type,
            self.canonical_types
                .iter()
                .map(|(path, identity)| format!("{path:?}:{}", identity.render_compact()))
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
}

/// Intrinsic source-level Result constructor selected by typechecking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResultVariantKind {
    /// `Ok(payload)`.
    Ok,
    /// `Err(payload)`.
    Err,
}

impl ResultVariantKind {
    /// Stable lowercase spelling for snapshots and receipts.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Err => "err",
        }
    }
}

/// One parameter of a top-level/method [`Body`] or [`Rvalue::Closure`].
///
/// Every callable surface uses this one representation so a direct consumer can bind top-level functions, methods,
/// and local closures by the same contract. `default` is intentionally tagged instead of using an availability bit
/// plus an optional capture: ordinary defaults are evaluated at the omitted call site, while partial presets were
/// already evaluated when the callable was constructed. A consumer must visibly refuse
/// [`CallableParamDefault::Unsupported`] at its stored source span instead of guessing from source structures or
/// silently falling back.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CallableParam {
    /// The local that receives this argument in the callable's execution frame.
    pub local: LocalId,
    /// Source-level parameter name.
    pub name: String,
    /// Fully resolved parameter type.
    pub ty: IncanType,
    /// Checked source role. Rest parameters own their packed list or dictionary, while ordinary collection
    /// parameters retain their normal borrowed passing convention. `ty` is the packed frame type for a rest role.
    #[serde(default = "ordinary_callable_parameter_kind")]
    pub kind: IncanCallableParamKind,
    /// Original parameter span, retained for argument-binding diagnostics.
    ///
    /// The syntax AST currently retains no receiver-token range, so a synthetic method `self` parameter uses the
    /// enclosing method declaration span. All source-spelled parameters retain their own parameter span.
    pub span: HirSourceSpan,
    /// The value to use when this parameter is omitted, or the explicit reason direct evaluation is unavailable.
    pub default: CallableParamDefault,
    /// The parameter is declared `mut` (#2022). For a non-`Copy` type the callee borrows the caller's value mutably
    /// and its changes are visible to the caller: callers pass the argument as
    /// [`OwnershipFact::MutBorrow`], and the callee neither owns nor drops the parameter, as the emitted route's
    /// `&mut T` parameter does. For a `Copy` type it is a mutable local copy, so callers still pass a copy.
    /// An owned native collection keeps its by-value boundary through `native_owned`; mutability then applies to the
    /// owned local.
    pub mutable: bool,
    /// The checker selected an owned native collection boundary rather than implicit source collection borrowing.
    /// The local may still be mutable; that does not turn this by-value ABI into an exclusive caller loan.
    #[serde(default)]
    pub native_owned: bool,
    /// Checked explicit source input access, separate from binding mutation and native collection ownership.
    #[serde(default)]
    pub input_access: crate::IncanInputAccess,
}

/// Preserve the ordinary passing convention when reading a Body IR parameter published before source roles.
pub(super) fn ordinary_callable_parameter_kind() -> IncanCallableParamKind {
    IncanCallableParamKind::Normal
}

impl CallableParam {
    /// Render a deterministic maintainer-facing spelling for this parameter.
    pub(super) fn render_snapshot(&self) -> String {
        format!(
            "{}{}: {} local=_{} span={}..{}{}",
            if self.mutable { "mut " } else { "" },
            self.name,
            self.input_access
                .source_form()
                .map_or_else(|| self.ty.to_string(), |form| format!("{form}[{}]", self.ty)),
            self.local.0,
            self.span.start,
            self.span.end,
            self.default.render_snapshot()
        )
    }
}

/// The direct-consumer binding behavior for one [`CallableParam`].
///
/// A source default owns a closed, type-fact-backed Body-IR computation and is evaluated only when the corresponding
/// argument is omitted. Lowering must use [`CallableParamDefault::Unsupported`] instead when it cannot establish
/// that contract. A partial preset instead names a closure-frame local populated from the partial's construction-time
/// capture. These variants must remain distinct: treating a preset as a source computation would evaluate it again,
/// while treating a source computation as a capture would evaluate it too early.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CallableParamDefault {
    /// The caller must supply this argument.
    Required,
    /// A source-declared default with usable canonical type facts whose Body-IR statements and result run at the
    /// omitted call site.
    ///
    /// The computation runs in its declaration-owned default-evaluation context, before the callable execution
    /// frame receives an argument for this parameter. Its lowering deliberately refuses reads that would require a
    /// callable-local binding, so a direct consumer must execute the stored computation as-is and then bind its
    /// result to [`CallableParam::local`].
    Source(Box<DefaultComputation>),
    /// A partial-callable value captured when the partial was constructed and still overrideable by the caller.
    PartialPreset {
        /// The closure-frame local populated from the matching captured operand.
        capture: LocalId,
    },
    /// A source-declared default Body IR cannot evaluate without recreating another compiler layer.
    ///
    /// Direct consumers must refuse at `span`; they must not use a legacy fallback or report the enclosing
    /// declaration/call span instead.
    Unsupported {
        /// Original source span of the unrepresentable default expression.
        span: HirSourceSpan,
        /// Maintainer-facing reason the expression cannot be executed directly.
        description: String,
    },
}

impl CallableParamDefault {
    /// Render a deterministic maintainer-facing spelling of the omission behavior.
    pub(super) fn render_snapshot(&self) -> String {
        match self {
            Self::Required => String::new(),
            Self::Source(computation) => {
                format!(" = source_default({})", computation.render_snapshot())
            }
            Self::PartialPreset { capture } => format!(" = captured(_{})", capture.0),
            Self::Unsupported { span, description } => {
                format!(
                    " = unsupported_default({description} span={}..{})",
                    span.start, span.end
                )
            }
        }
    }
}

/// A deferred Body-IR computation for one source-declared parameter default.
///
/// `stmts` are intentionally not appended to the owning callable's normal block: evaluating them there would run a
/// default even when a caller supplied an argument. A direct consumer executes this closed computation at the
/// omitted call site, before binding the callable frame's [`CallableParam::local`] argument. Any source read that
/// would require a callable-local or otherwise unrepresented lexical binding is recorded instead as
/// [`CallableParamDefault::Unsupported`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DefaultComputation {
    /// Full source span of the default expression.
    pub span: HirSourceSpan,
    /// Statements that must run before producing `result`.
    pub stmts: Vec<Statement>,
    /// The default expression's computed value.
    pub result: Operand,
}

impl DefaultComputation {
    /// Render a deterministic maintainer-facing spelling of the deferred computation.
    pub(super) fn render_snapshot(&self) -> String {
        let flattened = render_flattened_stmts(&self.stmts);
        if flattened.is_empty() {
            format!(
                "span={}..{} result: {}",
                self.span.start,
                self.span.end,
                self.result.render_snapshot()
            )
        } else {
            format!(
                "span={}..{} {}; result: {}",
                self.span.start,
                self.span.end,
                flattened.join("; "),
                self.result.render_snapshot()
            )
        }
    }
}

/// A closure literal's or synthesized partial-callable closure's own self-contained body computation.
///
/// Deliberately lighter than [`Body`]: it carries no `decl_id`/`name`/`scopes`/`runtime_requirements`/`panic_facts` of
/// its own -- a closure is not a top-level declaration, and any runtime/panic facts its body introduces are folded
/// directly into the owning [`Body`]'s own accumulated facts by lowering rather than tracked separately per closure. It
/// also reuses the *same* [`LocalId`] numbering as its owning [`Body`] rather than starting a fresh local space at
/// zero: the frontend lowering that builds this model (`loaves/compiler/incan_frontend/src/body_ir.rs`) already keeps
/// one flat, function-wide local-ID namespace. The frontend snapshots and restores its active name-to-local map at
/// lexical boundaries (including closures), so giving each closure a separate zero-based local space would mean
/// inventing a parallel indexing scheme just for this one construct. Reusing the owning body's monotonic counter keeps
/// every [`LocalId`] in a function globally unique and lets [`Rvalue::Closure`]'s [`CallableParam::local`] values and
/// [`Self::capture_locals`] simply index into the same [`Body::locals`] the rest of the function uses, so a closure's
/// own parameters and captures show up in the ordinary `locals:` listing like any other local.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClosureBody {
    /// Native callback storage checked against the callee's retained bounds. Absent storage grants no auto traits.
    #[serde(default)]
    pub native_storage: Option<NativeClosureStorage>,
    /// Compiler-retained original callable read during RFC 036 initialization. This must resolve to exactly one
    /// decorated binding, and is never inferred from a forwarding closure's source spelling.
    #[serde(default)]
    pub original_callable: Option<CanonicalSymbolId>,
    /// Whether this computation came from a named function read. Its canonical item identity lives once in the
    /// forwarding call. A source closure expression or partial remains distinct even when it forwards the same
    /// arguments without captures.
    #[serde(default)]
    pub function_item: bool,
    /// Invocation consumes the owned environment because a proven captured iterator moves into deferred work.
    /// Consumers must construct an FnOnce object and invoke it through a moved last-use operand; reusable Fn
    /// borrowing or cloning the iterator for another invocation would change this contract.
    #[serde(default)]
    pub consumes_environment: bool,
    /// The closure's own captured-binding locals, in the same order as [`Rvalue::Closure::captured_operands`] --
    /// `capture_locals[i]` is where a read of the `i`-th captured operand's value is durably bound inside the
    /// closure body, so subsequent reads inside the body see it as an ordinary local rather than re-reading the
    /// enclosing body's place directly.
    pub capture_locals: Vec<LocalId>,
    /// Statements needed to compute `result`.
    pub stmts: Vec<Statement>,
    /// The closure body expression's value.
    pub result: Operand,
}

/// Checked ownership and auto-trait obligations of a callback retained by Rust.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct NativeClosureStorage {
    /// The native environment owns its captures rather than borrowing its creating frame.
    pub move_capture: bool,
    /// Every captured value has retained Send evidence.
    pub send: bool,
    /// Every captured value has retained Sync evidence.
    pub sync: bool,
}

impl ClosureBody {
    /// Render a deterministic maintainer-facing spelling for this closure body, flattening its (possibly
    /// multi-line) statement rendering onto one `; `-joined line so it nests inside the single-line
    /// [`Rvalue::render_snapshot`] output the same way every other `Rvalue` variant does.
    pub(super) fn render_snapshot(&self) -> String {
        let flattened = render_flattened_stmts(&self.stmts);
        if flattened.is_empty() {
            format!("result: {}", self.result.render_snapshot())
        } else {
            format!("{}; result: {}", flattened.join("; "), self.result.render_snapshot())
        }
    }
}

/// The deferred, suspendable body of one [`Rvalue::Generator`].
///
/// Like [`ClosureBody`], this reuses its owning [`Body`]'s flat local-id namespace. `source_local` receives the
/// rvalue's construction-time [`Rvalue::Generator::source`] value. `capture_locals[i]` receives the matching
/// `captured_operands[i]` value, so no statement in `stmts` reaches back into the enclosing body's places when the
/// generator is later polled. Clause bindings and iterator temporaries are ordinary locals in `stmts`; they become
/// live only while the corresponding deferred loops execute.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GeneratorBody {
    /// Local holding the eagerly evaluated first-clause source value.
    pub source_local: LocalId,
    /// Locals holding the rvalue's construction-time lexical captures, in operand order.
    pub capture_locals: Vec<LocalId>,
    /// Deferred iteration, filtering, and [`StatementKind::Yield`] operations.
    pub stmts: Vec<Statement>,
}

impl GeneratorBody {
    /// Render a deterministic maintainer-facing spelling of the deferred body for nested rvalue snapshots.
    pub(super) fn render_snapshot(&self) -> String {
        let flattened = render_flattened_stmts(&self.stmts);
        if flattened.is_empty() {
            "deferred: <empty>".to_string()
        } else {
            format!("deferred: {}", flattened.join("; "))
        }
    }
}

/// Render `stmts` at zero indentation and split into trimmed, non-empty lines, for embedding a (possibly
/// multi-statement) nested block into a single-line snapshot alongside a trailing `result: ...`/`if ...` segment.
/// Shared by [`ClosureBody::render_snapshot`], [`GeneratorBody::render_snapshot`], and
/// [`MatchArm::render_snapshot`], which all need to flatten a nested [`Block`]-shaped computation the same way.
pub(super) fn render_flattened_stmts(stmts: &[Statement]) -> Vec<String> {
    let mut body = String::new();
    for stmt in stmts {
        render_statement(&mut body, stmt, "", 0);
    }
    body.lines().map(str::trim).map(str::to_string).collect()
}
