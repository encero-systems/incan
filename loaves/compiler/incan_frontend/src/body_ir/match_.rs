//! Lowering for `match` expressions and their patterns.

use super::control_flow::intersect_local_states;
use super::primitives::*;
use super::reads::*;
use super::refusals::*;
use super::*;

mod match_lowering;
mod pattern_lowering;

/// Where the names a pattern binds can still be read once that pattern has matched.
///
/// [`BodyBuilder::lower_match_pattern`] seeds each fresh binding's last-use countdown from this, and its two
/// callers disagree about what "afterwards" means. A `match` arm's bindings are visible only inside that arm, so
/// its guard and body are the whole live range. An `assert value is P` binding instead stays live for the rest of
/// the enclosing block, so its live range is the statement suffix following the assertion. Passing the counting
/// context in rather than hard-coding one keeps a single pattern lowerer serving both.
pub(super) enum PatternReadScope<'ast> {
    /// A `match` arm: its guard plus its body.
    MatchArm(&'ast ast::MatchArm),
    /// The statement suffix following an `assert value is P` in its enclosing block.
    FollowingStatements(&'ast [ast::Spanned<ast::Statement>]),
}

impl PatternReadScope<'_> {
    /// Count reads of `name` reachable from this scope, using the same source-order over-approximation
    /// [`count_reads_in_stmts`] documents.
    fn count_reads(&self, name: &str) -> usize {
        match self {
            Self::MatchArm(arm) => count_reads_in_match_arm(name, arm),
            Self::FollowingStatements(stmts) => count_reads_in_stmts(name, stmts),
        }
    }
}

/// The same constructor pattern with its one payload binding replaced by `_`, for an arm whose name is bound elsewhere.
fn without_payload_binding(pattern: &ast::Spanned<ast::Pattern>) -> ast::Spanned<ast::Pattern> {
    let mut pattern = pattern.clone();
    if let ast::Pattern::Constructor(_, arguments) = &mut pattern.node
        && let [ast::PatternArg::Positional(payload)] = arguments.as_mut_slice()
    {
        payload.node = ast::Pattern::Wildcard;
    }
    pattern
}

/// Whether storage of type `ty` can hold a reference at any depth; a type still open counts, since it may become one.
fn storage_names_reference(ty: &IncanType) -> bool {
    match ty {
        IncanType::Ref(_)
        | IncanType::RefMut(_)
        | IncanType::Function { .. }
        | IncanType::TypeVar(_)
        | IncanType::SelfType
        | IncanType::Infer
        | IncanType::Unknown
        | IncanType::Unsupported(_) => true,
        IncanType::Generic { args, .. } | IncanType::Tuple(args) => args.iter().any(storage_names_reference),
        IncanType::NativeArray { element, .. } => storage_names_reference(element),
        _ => false,
    }
}

/// Name a model pattern's field by its declared canonical name rather than the spelling the pattern wrote.
///
/// An RFC 021 alias (`Plan(type="premium")` for a field declared `type_`) resolves through the checker to the declared
/// field's identity, so that identity selects the canonical name a consumer finds in the declaration's layout. A field
/// whose identity the checker did not record keeps its written spelling, which a consumer then refuses as unproven.
fn canonical_pattern_field(
    declaration: &bir::NominalDeclaration,
    identity: Option<&CanonicalSymbolId>,
    written: &str,
) -> String {
    identity
        .and_then(|identity| {
            declaration
                .field_identities
                .iter()
                .position(|candidate| candidate == identity)
        })
        .and_then(|slot| declaration.fields.get(slot))
        .map_or_else(|| written.to_string(), Clone::clone)
}
