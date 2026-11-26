//! Source-local C ABI call state and open native generic call bindings.

use super::*;

/// Source-local slot state before a checked C call establishes the exact native storage contract.
///
/// This state never reaches lowering. Once a raw symbol accepts the slot, a typed [`CAbiOutputSlot`] fact replaces it
/// in [`TypeCheckInfo`].
#[derive(Debug, Clone)]
pub struct PendingCAbiOutputSlot {
    /// Closure frame that owns the slot's storage; captures require an unimplemented lifetime contract.
    pub closure_depth: usize,
    pub constructor_span: Span,
    pub mode: COutputMode,
    pub declared_type: Option<String>,
    pub initial_type: Option<ResolvedType>,
    pub bound: bool,
}

/// Source-local ownership state for one opaque checked C typed span carrier.
///
/// This state is deliberately frontend-only. The accepted constructor and bridge operations are retained as
/// [`CAbiInteropArtifacts`] facts; this record only prevents the source local that carries them from escaping or
/// being consumed twice while the body is typechecked.
#[derive(Debug, Clone, Copy)]
pub struct CAbiSpanLocal {
    /// Exact element representation and mutability selected by its direct constructor.
    pub kind: CAbiSpanKind,
    /// Ordinary local declaration span, retained so lexical shadowing cannot reuse a stale name-state entry.
    pub binding_span: Span,
}

/// Result-local relationship between one raw C call and the slots it may initialize or invalidate.
#[derive(Debug, Clone)]
pub struct CAbiRawCallResult {
    pub binding: String,
    pub symbol: String,
    /// Ordinary local name that received this raw call result.
    pub local_name: Option<String>,
    /// Source span of the ordinary local that received this raw-call result.
    ///
    /// Keeping the declaration span makes outcome facts follow regular lexical shadowing rather than a bare local
    /// spelling.
    pub local_symbol_span: Option<Span>,
    pub slots_by_parameter: HashMap<String, String>,
}

/// A native call whose result carries type arguments the call itself left open (#1720, #2481).
///
/// `HashMap.new()` on `rust::std::collections::HashMap` names neither `K` nor `V`, and nothing at the call fixes
/// them: no explicit `[str, int]`, no expected type, no argument that mentions them. A local bound to the value can
/// still have them fixed by its first use (#2481); the checker keeps this record until it knows whether such a use
/// exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenRustGenericCall {
    /// The receiver as the source spells it (`HashMap`), for the diagnostic.
    pub owner: String,
    /// The associated function or method called (`new`, `sum`).
    pub method: String,
    /// The owner or method parameters the call left open, in declaration order (`K`, `V`, or `S`).
    pub type_params: Vec<String>,
    /// Whether these are the associated constructor's owner parameters; method-result parameters use T0122.
    pub owner_parameters: bool,
    /// The call's native result with those parameters open, once callee retention has computed it. A local bound to
    /// the call holds it as its native carrier until a use fixes the parameters.
    pub native: Option<ResolvedType>,
    /// The native result's nominal item (`std::collections::HashMap`), which selects the local's native methods.
    pub native_item: Option<String>,
}

/// A local bound to an [`OpenRustGenericCall`], watched until its first use fixes the call's open parameters (#1720).
///
/// The first read whose checking needs the parameters must determine them, and the binding takes that type at once.
/// Independent reads leave it watched. A type-dependent read that fixes nothing refuses the call at the statement's
/// end; no fixing use refuses it at the declaring block's end (#2481).
#[derive(Debug, Clone, PartialEq)]
pub struct OpenRustGenericBinding {
    /// The local's name.
    pub name: String,
    /// Identity of the statement block that declared the binding, so the block's end reports only its own.
    pub block: usize,
    /// Span of the call expression the binding received.
    pub span: Span,
    /// The call whose open parameters the binding carries.
    pub call: OpenRustGenericCall,
    /// The declaring assignment, checked again against the type the first use fixes.
    pub assignment: crate::ast::AssignmentStmt,
    /// The declaring statement's span, which keys the binding's checked type.
    pub statement: Span,
    /// The binding's canonical identity: only a read of this binding can fix it.
    pub identity: Option<CanonicalSymbolId>,
    /// The statement block of the first read whose checking needs the open parameters.
    pub read_in: Option<usize>,
    /// Reads in the current fixing statement that still require the open parameters.
    pub active_reads: Vec<Span>,
    /// Earlier operations whose own checking needs no open parameter, rechecked once the initializer is fixed.
    pub passive_uses: Vec<OpenRustPassiveUse>,
}

/// An operation whose checking needs no open owner argument, retained until its native layout can be fixed.
#[derive(Debug, Clone, PartialEq)]
pub enum OpenRustPassiveUse {
    /// Recheck an ordinary native method expression with its original source argument shape.
    Expression(crate::ast::Spanned<crate::ast::Expr>),
    /// Keep builtin length's native call shape, whose receiver is an ordinary argument rather than a method slot.
    Length {
        receiver: crate::ast::Spanned<crate::ast::Expr>,
        span: Span,
    },
}
