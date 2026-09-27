//! Checking a chained assignment, `x = y = value` (#1806).

use crate::ast::*;
use crate::diagnostics::{CompileError, errors};
use crate::symbols::ResolvedType;
use crate::typechecker::TypeChecker;
use crate::typechecker::type_info::TypeCheckInfo;

impl TypeChecker {
    /// Check `x = y = value` the way a single assignment checks each target.
    ///
    /// The value is checked against the type the already-bound targets share, as `x = value` checks against `x`'s
    /// type, so a literal adapts to it; the targets share the first bound target's type when every other bound
    /// target's type is compatible with it both ways (`list[int]` and `list[i64]` are). When the bound targets have
    /// different types:
    ///
    /// - a value built only from literals and builtin empty constructors (see [`Spanned::is_literal_construction`]) is
    ///   checked against each target's type in turn, and recorded for lowering, which gives each target its own
    ///   evaluation of it (`a = b = 5` over an `i8` and an `int`); an error that does not depend on the target is
    ///   reported once, as a single assignment reports it;
    /// - any other value is checked once and evaluated once, and refused when its type is not fully known, because
    ///   there is no one type for it to take (`a = b = empty()` returning `list[_]` over a `list[int]` and a
    ///   `list[str]`); a value whose check already reported an error gets no second report.
    pub(in crate::typechecker) fn check_chained_assignment(&mut self, chain: &ChainedAssignmentStmt, stmt_span: Span) {
        let target_types = chain
            .targets
            .iter()
            .map(|target| self.chained_target_type(chain.binding, target))
            .collect::<Vec<_>>();
        let mut bound = target_types.iter().flatten();
        let first_bound = bound.next().cloned();
        let different = first_bound.as_ref().and_then(|first| {
            bound
                .find(|other| !self.types_equivalent(first, other))
                .map(|other| (first.clone(), other.clone()))
        });

        let per_target_types: Vec<ResolvedType> = match different {
            None => {
                if first_bound.is_some() {
                    self.type_info.record_chained_targets_agree(chain.value.span);
                }
                let value_ty = self.check_expr_with_expected(&chain.value, first_bound.as_ref());
                vec![value_ty; chain.targets.len()]
            }
            Some((first, other)) => {
                // The check records which calls resolved to a builtin constructor, so it comes before the test.
                let (errors_before, warnings_before) = (self.errors.len(), self.warnings.len());
                let value_ty = self.check_expr(&chain.value);
                let literal = chain
                    .value
                    .is_literal_construction(&|span| self.type_info.resolved_collection_constructor(span).is_some());
                if literal {
                    // The untargeted check's reports go. Each target's check below reports again, so an error that
                    // does not depend on the target (`-"s"`) comes once per target until the repeats are dropped.
                    self.errors.truncate(errors_before);
                    self.warnings.truncate(warnings_before);
                    self.type_info.record_chained_value_written_per_target(chain.value.span);
                    let per_target = target_types
                        .iter()
                        .map(|target_ty| self.check_expr_with_expected(&chain.value, target_ty.as_ref()))
                        .collect();
                    Self::drop_repeated_reports(&mut self.errors, errors_before);
                    Self::drop_repeated_reports(&mut self.warnings, warnings_before);
                    per_target
                } else if Self::type_has_open_part(&value_ty) && self.errors.len() == errors_before {
                    self.errors.push(errors::chained_assignment_value_has_no_one_type(
                        &first.to_string(),
                        &other.to_string(),
                        &value_ty.to_string(),
                        chain.value.span,
                    ));
                    return;
                } else {
                    vec![value_ty; chain.targets.len()]
                }
            }
        };

        // Each target has the same declaration/reassignment distinction as a single assignment's target.
        for (index, (target, value_ty)) in chain.targets.iter().zip(per_target_types).enumerate() {
            let target_span = chain.target_spans.get(index).copied().unwrap_or(stmt_span);
            self.check_unannotated_assignment_target(target, chain.binding, value_ty, target_span, chain.value.span);
        }
    }

    /// Return the type an existing binding gives a chain target, or `None` for a target the chain declares.
    fn chained_target_type(&self, binding: BindingKind, name: &str) -> Option<ResolvedType> {
        if Self::binding_introduces_name(binding) {
            return None;
        }
        self.lookup_variable_info_in_scope_chain(name)
            .map(|info| info.ty.clone())
            .or_else(|| self.lookup_static_info(name).map(|info| info.ty.clone()))
    }

    /// Drop every report after `start` that repeats an earlier one after `start` in kind, message and span, keeping the
    /// first.
    fn drop_repeated_reports(reports: &mut Vec<CompileError>, start: usize) {
        let mut seen = Vec::new();
        let mut index = start;
        while index < reports.len() {
            let report = &reports[index];
            let key = (report.kind, report.message.clone(), report.span);
            if seen.contains(&key) {
                reports.remove(index);
            } else {
                seen.push(key);
                index += 1;
            }
        }
    }

    /// Return whether two types are the same type to the checker: each is compatible with the other.
    fn types_equivalent(&self, left: &ResolvedType, right: &ResolvedType) -> bool {
        self.types_compatible(left, right) && self.types_compatible(right, left)
    }

    /// Return whether a checked type still has a part only a destination could fix (`None`'s `Option[_]`, `list()`).
    fn type_has_open_part(ty: &ResolvedType) -> bool {
        match ty {
            ResolvedType::Unknown | ResolvedType::CallSiteInfer | ResolvedType::TypeVar(_) => true,
            ResolvedType::Generic(_, args) | ResolvedType::Tuple(args) => args.iter().any(Self::type_has_open_part),
            ResolvedType::FrozenList(inner)
            | ResolvedType::FrozenSet(inner)
            | ResolvedType::Ref(inner)
            | ResolvedType::RefMut(inner) => Self::type_has_open_part(inner),
            ResolvedType::FrozenDict(key, value) => Self::type_has_open_part(key) || Self::type_has_open_part(value),
            _ => false,
        }
    }
}

impl TypeCheckInfo {
    /// Record that each target of the chained assignment whose value is at `value_span` gets its own evaluation of it.
    pub fn record_chained_value_written_per_target(&mut self, value_span: Span) {
        self.expressions
            .chained_values_written_per_target
            .insert((value_span.start, value_span.end));
    }

    /// Return whether each target of the chained assignment whose value is at `value_span` gets its own evaluation of
    /// it: the targets have different types and the value is built only from literals and builtin empty
    /// constructors. Otherwise the value is evaluated once and shared.
    pub fn chained_value_is_written_per_target(&self, value_span: Span) -> bool {
        self.expressions
            .chained_values_written_per_target
            .contains(&(value_span.start, value_span.end))
    }

    /// Record that the chained assignment whose value is at `value_span` has bound targets that all have one type.
    pub fn record_chained_targets_agree(&mut self, value_span: Span) {
        self.expressions
            .chained_values_of_agreeing_targets
            .insert((value_span.start, value_span.end));
    }

    /// Return whether the chained assignment whose value is at `value_span` has at least one bound target and every
    /// other bound target's type is compatible both ways with the first bound target's type (`list[int]` and
    /// `list[i64]` are). The value was then checked against the first bound target's type.
    pub fn chained_targets_agree(&self, value_span: Span) -> bool {
        self.expressions
            .chained_values_of_agreeing_targets
            .contains(&(value_span.start, value_span.end))
    }
}
