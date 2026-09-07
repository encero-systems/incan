//! Labels and shape checks behind every explicit lowering refusal.

use super::*;

/// Name one node that [`build_body_ir_module_v0`]'s input contract required the *caller* to resolve before
/// lowering, rather than one this stage has simply not modeled yet (#1166).
///
/// The distinction is worth carrying in the diagnostic text. An unmodeled construct is remaining work on Body IR
/// and tells a reader to wait for the owning sub-issue; a node the desugar pass should have removed is a broken
/// pipeline, and the repair belongs to whoever assembled the program. Wording every such refusal the same way
/// keeps the second from being read as the first.
///
/// This never becomes the primary diagnostic for a vocabulary whose library manifest is unavailable. The desugar
/// pass already refuses that, at that boundary, and a second message for one condition would be two answers to
/// the same question. This fires only when a caller skipped the pass altogether.
fn undesugared_label(node: &str) -> String {
    format!("undesugared {node} (Body IR input-contract violation: caller skipped the vocab desugar pass)")
}

/// Name a raw top-level declaration that the desugar pass had to remove before Body IR collection.
///
/// Top-level collection normally produces bodies only for executable declarations. A raw vocabulary declaration
/// therefore has no containing body to hold its refusal, so the module builder injects this label into every body
/// it did lower. That makes every direct execution fail at the declaration's original span rather than selecting an
/// otherwise-valid body from an incomplete module.
pub(super) fn unsupported_top_level_declaration_label(declaration: &ast::Declaration) -> Option<String> {
    match declaration {
        ast::Declaration::VocabBlock(_) => Some(undesugared_label("top-level vocab block")),
        _ => None,
    }
}

/// Short diagnostic label for a statement kind v0 does not lower.
///
/// Only reached from [`BodyBuilder::lower_stmt_into`]'s fallback arm, so every statement kind that arm dispatches
/// by name is deliberately absent here -- statement-position `loop:` and `unsafe:` regions included, since #1162
/// gave the first a lowering and the second a stated permanent refusal that carries its own reason.
///
/// The raw vocabulary forms take the [`undesugared_label`] wording instead, because neither is a lowering gap:
/// both are syntax the desugar pass owns and resolves before this module ever runs. An [`ast::Statement::Surface`]
/// could be either, so it defers to [`surface_stmt_label`] to decide from its key.
pub(super) fn unsupported_stmt_label(stmt: &ast::Statement) -> String {
    match stmt {
        ast::Statement::VocabExpressionItem(_) => undesugared_label("vocab expression item"),
        ast::Statement::Surface(surface) => surface_stmt_label(&surface.key),
        ast::Statement::VocabBlock(_) => undesugared_label("vocab block"),
        _ => "statement".to_string(),
    }
}
/// Name an [`ast::Statement::Surface`] refusal by the surface *key* that produced it.
///
/// The payload shape cannot make this call. `ast::SurfaceStmtPayload::KeywordArgs` is the only payload there is,
/// and it carries both a library's scoped-DSL statement and the stdlib-registered soft keywords (`assert` is the
/// one registered today -- see `incan_semantics_stdlib`'s `lower_surface_stmt_action`). Only a scoped-DSL key is a
/// contract violation. A soft-keyword surface statement is real language surface with no Body IR lowering yet, so
/// it keeps an unmodeled-construct label and names the keyword a program actually hit rather than reading as a
/// pipeline fault its author cannot act on.
fn surface_stmt_label(key: &SurfaceFeatureKey) -> String {
    match key {
        SurfaceFeatureKey::ScopedDslSurface { dependency_key, .. } => {
            undesugared_label(&format!("scoped DSL statement from `{dependency_key}`"))
        }
        SurfaceFeatureKey::SoftKeyword(keyword) => format!(
            "soft-keyword surface statement `{}`",
            incan_core::lang::keywords::as_str(*keyword)
        ),
        SurfaceFeatureKey::Decorator(_) => "decorator surface statement".to_string(),
    }
}
/// Why an admitted provider operation cannot become a checked execution plan, or `None` when it can (#1213).
///
/// Consulted once, before any argument of the call is lowered, so a refusal never leaves the operands of a call that
/// never happens behind -- the same "check before partially lowering" precedent as [`match_pattern_is_supported`].
/// Refusing here rather than emitting a plan is what makes the "no execution receipt for a lowering refusal"
/// guarantee structural: with no [`bir::Callee::ProviderOperation`] statement there is nothing for an executor to
/// run, and nothing for it to report having run.
///
/// Two independent things make an operation unexecutable, and both are checked.
///
/// **Activation.** Only an active provider may be planned against. A disabled or unavailable provider is a real
/// entry in the catalog, so the call did resolve; what it did not do is reach something this compilation can
/// execute, and the two states are named separately because they have different remedies.
///
/// **Capability identity.** The plan promises that [`bir::ProviderOperationPlan::required_capability`] names an RFC
/// 104 `capability` declaration, which is what makes an authority request answerable. An identity of any other kind
/// -- a function, a model, a module -- would produce a request no authority source could decide, so it is refused
/// here rather than carried into a plan that quietly cannot be authorized.
///
/// The message names the *declaration* the identity selected, never the call site's spelling: which operation this
/// is, is a question only the canonical identity answers.
pub(super) fn unsupported_provider_operation(
    operation: &CanonicalSymbolId,
    record: &ProviderOperationRecord,
) -> Option<String> {
    let declared = &operation.declaration_name;
    match record.provider.state {
        bir::ProviderActivationState::Active => {}
        bir::ProviderActivationState::Disabled => {
            return Some(format!(
                "provider operation `{declared}` whose provider is not enabled in this compilation"
            ));
        }
        bir::ProviderActivationState::Unavailable => {
            return Some(format!(
                "provider operation `{declared}` whose provider has no locally available artifact"
            ));
        }
    }
    if record.required_capability.kind != SemanticSourceTargetKind::Capability {
        return Some(format!(
            "provider operation `{declared}` whose required authority does not name a capability declaration"
        ));
    }
    None
}
/// Short diagnostic label for an expression kind v0 does not lower.
///
/// Only reached from [`BodyBuilder::lower_expr_to_operand`]'s fallback arm, so every expression kind that arm
/// dispatches by name -- closures and partial callables since #1124, range values since #1165 -- is
/// deliberately absent here. Async surface (`await`, `race for`) and vocab/scoped-DSL surface are named rather than
/// left to the generic label, because a diagnostic reading only "expression" hides which one a program actually
/// hit. The two are not the same kind of finding: async surface is remaining Body IR work under #1164, while a
/// vocab node is a violation of [`build_body_ir_module_v0`]'s input contract (#1166).
pub(super) fn unsupported_expr_label(expr: &ast::Expr) -> String {
    match expr {
        ast::Expr::Yield(_) => "yield expression".to_string(),
        ast::Expr::Surface(surface) => surface_expr_label(&surface.payload),
        ast::Expr::VocabBlock(_) => undesugared_label("vocab block expression"),
        // RFC 081 (#1023): unlike `VocabBlock`/`Surface`, an embedded fragment is *not* a `build_body_ir_module_v0`
        // input-contract violation -- it is meant to reach lowering as itself (see `IrExprKind::EmbeddedFragment`'s
        // rustdoc in `src/backend/ir/expr.rs`, the pipeline this replacement-backend Body IR does not yet share).
        // This still-maturing pipeline simply does not cover it yet, so it falls through this fallback arm like
        // any other not-yet-supported expression kind; only the label is specific enough to say which one.
        ast::Expr::Embedded(_) => "embedded DSL fragment expression".to_string(),
        _ => "expression".to_string(),
    }
}
/// Name the specific surface-expression payload behind an [`ast::Expr::Surface`] refusal.
///
/// The payloads split into two very different buckets, and the wording follows that split rather than flattening
/// it. `await`/`race for` are the async surface #1164 represents, which #1155 needs before it can execute task
/// state: real language surface, no desugarer involved, so they keep an unmodeled-construct label. The remaining
/// payloads are scoped-DSL nodes the desugar pass resolves before lowering, so one arriving here means a caller
/// skipped that pass and takes the [`undesugared_label`] wording (#1166).
pub(super) fn surface_expr_label(payload: &ast::SurfaceExprPayload) -> String {
    match payload {
        ast::SurfaceExprPayload::PrefixUnary(_) => {
            "prefix-keyword surface expression (for example `await`)".to_string()
        }
        ast::SurfaceExprPayload::RaceFor(_) => "`race for` expression".to_string(),
        ast::SurfaceExprPayload::LeadingDotPath { .. } => undesugared_label("scoped DSL leading-dot path"),
        ast::SurfaceExprPayload::ScopedGlyph { .. } => undesugared_label("scoped DSL glyph operator"),
        ast::SurfaceExprPayload::ScopedSymbolCall { .. } => undesugared_label("scoped DSL symbol call"),
    }
}
/// Resolve the per-element types for a tuple-typed value being destructured into `count` targets, falling back to
/// [`IncanType::Unknown`] per element when the resolved type is not (or not yet) known to be a tuple of the right
/// arity -- mirrors how the existing Rust-emission backend falls back to `IrType::Unknown` per slot in the same
/// situation (`src/backend/ir/lower/stmt.rs`'s `TupleUnpack` lowering). Used by
/// [`BodyBuilder::lower_tuple_unpack`], [`BodyBuilder::lower_tuple_assign`], and
/// [`BodyBuilder::bind_for_pattern_fields`].
///
/// A tuple type reaches lowering in two spellings and both must be understood here. A tuple *literal* resolves to
/// [`IncanType::Tuple`], while a written `tuple[A, B]` *annotation* resolves through the collection-type registry
/// and therefore arrives as an [`IncanType::Generic`] whose base is that registry's canonical name. Matching only
/// the first spelling silently degraded every element of an annotated tuple to `Unknown`, which in turn made each
/// element read `Borrow` rather than its real Copy/non-Copy fact. The generic base is classified through
/// [`collections::from_str`] rather than compared against a literal name, so the registry stays the single source
/// of truth for that vocabulary.
/// Why a statement-level destructure of `value_ty` into `arity` names cannot be lowered, or `None` when it can.
///
/// The statement sibling of [`unsupported_for_pattern`], and it exempts the same two types for the same reason:
/// `Unknown` and `Never` mean the typechecker either already reported a failure or is looking at unreachable code,
/// so lowering has nothing to refuse. Everything else — including Rust interop, which is checked against the same
/// [`rust_tuple_arity`] rule the typechecker uses rather than waved through — must be a tuple of exactly matching
/// arity before lowering may emit a `.0`/`.1` field projection. Without this, a non-tuple value produced
/// `__incan_tuple_unpack_*.0` against a fieldless value and surfaced as a raw `rustc` E0610 (#1132).
pub(super) fn unsupported_tuple_destructure(value_ty: &IncanType, arity: usize) -> Option<String> {
    if matches!(value_ty, IncanType::Unknown | IncanType::Never) {
        return None;
    }
    // Interop values go through the same accepted-shape rule the typechecker uses, not an exemption: a readable
    // tuple spelling lowers, and anything opaque refuses. Waving every `RustInteropPath` through would have let a
    // genuine non-tuple Rust value reach a `.0`/`.1` projection, which is the leakage #1132 closes.
    if let IncanType::RustInteropPath(path) = value_ty {
        return match rust_tuple_arity(path) {
            Some(rust_arity) if rust_arity == arity => None,
            Some(rust_arity) => Some(format!(
                "tuple destructure binds {arity} names but Rust value type `{path}` has {rust_arity} elements"
            )),
            None => Some(format!(
                "tuple destructure of Rust value type `{path}` whose tuple shape cannot be verified"
            )),
        };
    }
    let Some(element_types) = tuple_type_elements(value_ty) else {
        return Some(format!("tuple destructure of non-tuple value type `{value_ty}`"));
    };
    if element_types.len() != arity {
        return Some(format!(
            "tuple destructure binds {arity} names but value type `{value_ty}` has {} elements",
            element_types.len()
        ));
    }
    None
}
pub(super) fn tuple_element_types(ty: &IncanType, count: usize) -> Vec<IncanType> {
    match tuple_type_elements(ty) {
        Some(items) if items.len() == count => items.to_vec(),
        _ => vec![IncanType::Unknown; count],
    }
}
/// The element types of a tuple-shaped [`IncanType`], in either spelling, or `None` when `ty` is not a tuple at
/// all. Backs both [`tuple_element_types`] and [`unsupported_for_pattern`], so the "is this a tuple, and of what
/// arity" question is answered in exactly one place rather than once per caller.
pub(super) fn tuple_type_elements(ty: &IncanType) -> Option<&[IncanType]> {
    match ty {
        IncanType::Tuple(items) => Some(items),
        IncanType::Generic { base, args } if collections::from_str(base) == Some(CollectionTypeId::Tuple) => Some(args),
        _ => None,
    }
}
/// Whether `pattern` is representable by [`bir::Pattern`]'s closed vocabulary. The only unrepresentable shape is a
/// byte-string literal pattern: [`bir::Constant::Bytes`] represents byte *values*, but Body IR does not yet model
/// byte-pattern matching semantics. Every other pattern shape lowers structurally, with [`IncanType::Unknown`]
/// field-type fallbacks where needed rather than an outright failure (see
/// [`BodyBuilder::lower_match_pattern`]'s own docs). Checked for every arm before [`BodyBuilder::lower_match`]
/// lowers any of them, mirroring [`BodyBuilder::binary_op_is_supported`]'s "check before partially lowering"
/// precedent.
pub(super) fn match_pattern_is_supported(pattern: &ast::Pattern) -> bool {
    match pattern {
        ast::Pattern::Literal(ast::Literal::Bytes(_)) => false,
        ast::Pattern::Literal(_) | ast::Pattern::Wildcard | ast::Pattern::Binding(_) => true,
        ast::Pattern::Tuple(items) => items.iter().all(|item| match_pattern_is_supported(&item.node)),
        ast::Pattern::Constructor(_, args) => args.iter().all(|arg| match arg {
            ast::PatternArg::Positional(pat) | ast::PatternArg::Named(_, pat) => match_pattern_is_supported(&pat.node),
        }),
        ast::Pattern::Group(inner) => match_pattern_is_supported(&inner.node),
        ast::Pattern::Or(items) => items.iter().all(|item| match_pattern_is_supported(&item.node)),
    }
}
/// Name the reason Body IR cannot bind `pattern` against a produced item of type `item_ty`, or `None` when it
/// can. Consulted once, up front, so a refusal never leaves half-emitted bindings behind -- the same precedent as
/// [`match_pattern_is_supported`].
///
/// Two independent things can make a loop pattern unbindable, and both are checked here.
///
/// **Shape.** The accepted subset is deliberately the same one `TypeChecker::define_for_pattern_bindings`
/// (`src/frontend/typechecker/check_stmt.rs`) accepts -- a plain binding, `_`, and recursively a tuple of those
/// (#1125). Naming the offending shape keeps a hand-built AST that bypassed the typechecker diagnosable.
///
/// **Type agreement.** A tuple pattern can only take elements from a tuple. Without this check, `for a, b in
/// items` over a `list[int]` would lower `.0`/`.1` projections out of an `int` -- structurally valid Body IR
/// describing something that does not exist. The typechecker rejects that program first, so this is defence in
/// depth for hand-built ASTs and for lowering that runs despite type errors, not the primary diagnostic.
///
/// Two item types are exempt from the tuple requirement, mirroring `TypeChecker::define_for_pattern_bindings`
/// exactly so the two stages cannot disagree about which programs are bindable.
/// [`IncanType::Unknown`] is recovery-only: it means the type is unresolved, not proven non-tuple, so each element
/// binds as `Unknown` just as [`tuple_element_types`] already falls back to. [`IncanType::Never`] is the bottom
/// type, which the typechecker's own `types_compatible` treats as compatible with every type including a tuple.
///
/// A bare [`IncanType::TypeVar`] is deliberately **not** exempt. An unconstrained `T` is known to be
/// underdetermined rather than merely unknown, and can be instantiated as `int`; Incan has no tuple-shaped bound
/// that could promise otherwise. This does not affect the common `list[Tuple[K, V]]` shape, whose item type is a
/// tuple whose *elements* are type variables.
pub(super) fn unsupported_for_pattern(pattern: &ast::Pattern, item_ty: &IncanType) -> Option<String> {
    match pattern {
        ast::Pattern::Binding(_) | ast::Pattern::Wildcard => None,
        ast::Pattern::Tuple(items) => {
            if matches!(item_ty, IncanType::Unknown | IncanType::Never) {
                return items
                    .iter()
                    .find_map(|item| unsupported_for_pattern(&item.node, &IncanType::Unknown));
            }
            let Some(element_types) = tuple_type_elements(item_ty) else {
                return Some(format!("for-loop tuple pattern over non-tuple item type `{item_ty}`"));
            };
            if element_types.len() != items.len() {
                return Some(format!(
                    "for-loop tuple pattern binds {} names but item type `{item_ty}` has {} elements",
                    items.len(),
                    element_types.len()
                ));
            }
            items
                .iter()
                .zip(element_types)
                .find_map(|(item, element_ty)| unsupported_for_pattern(&item.node, element_ty))
        }
        ast::Pattern::Literal(_) => Some("for-loop pattern shape: literal".to_string()),
        ast::Pattern::Constructor(..) => Some("for-loop pattern shape: constructor".to_string()),
        ast::Pattern::Group(_) => Some("for-loop pattern shape: parenthesized group".to_string()),
        ast::Pattern::Or(_) => Some("for-loop pattern shape: alternation".to_string()),
    }
}
