//! Frozen mutation settlement for the fresh-check oracle; never used as a production fallback.

use super::{
    ArgumentPlace, CanonicalSymbolId, HashSet, MutArgumentPlace, MutParameterChange, MutParameterLabel, Span,
    TypeChecker, errors,
};

impl TypeChecker {
    /// Decide every recorded argument once the module's bodies have been checked.
    ///
    /// A callee changes a caller-visible parameter when its body writes through it or through a view into it, calls a
    /// method that may change it, uses it when it is a `Generator`, or passes it on to a parameter another callee
    /// changes; a callee whose body this check did not read (an imported declaration, a trait method without a default,
    /// a method reached by trait dispatch, a callable known only by its type) is taken to change it. An immutable
    /// binding or field, an element or a static passed to a changed parameter is refused with `INCAN-T0117`, and so is
    /// a `for` loop variable. An immutable binding or field, or a loop variable, passed to an unchanged one is
    /// published for lowering as a copy, unless its type cannot be copied (a `Generator`, a type holding one),
    /// which is refused with `INCAN-T0117` too.
    pub(in crate::typechecker) fn legacy_resolve_mut_arguments(&mut self) {
        self.check_work.mutation_settlements += 1;
        let changed = self.legacy_mut_param_change_closure();
        let pending = std::mem::take(&mut self.mut_params.pending);
        for argument in pending {
            let known_callee = argument.callee.as_ref().filter(|callee| {
                self.mut_params.bodies_checked.contains(*callee)
                    && !self.call_goes_through_a_reassigned_local(argument.through_local)
            });
            let change = match known_callee {
                Some(_) => MutParameterChange::Changes,
                None => MutParameterChange::MayChange,
            };
            let changes = known_callee.is_none_or(|callee| changed.contains(&(callee.clone(), argument.param.clone())));
            let immutable_place = match &argument.place {
                ArgumentPlace::Immutable { binding } => Some(match binding {
                    Some(name) => MutArgumentPlace::Binding(name.clone()),
                    None => MutArgumentPlace::Field,
                }),
                ArgumentPlace::Detached { place } => Some(place.clone()),
                ArgumentPlace::Temporary | ArgumentPlace::Mutable { .. } => None,
            };
            let Some(place) = immutable_place else {
                continue;
            };
            let parameter = if argument.param.is_empty() {
                MutParameterLabel::Position(argument.position)
            } else {
                MutParameterLabel::Named(&argument.param)
            };
            let error = if changes {
                errors::immutable_argument_to_mut_parameter(
                    parameter,
                    &argument.callee_name,
                    change,
                    place,
                    argument.span,
                )
            } else if let Some(value_type) = &argument.uncopyable_type {
                errors::uncopyable_argument_to_mut_parameter(
                    parameter,
                    &argument.callee_name,
                    value_type,
                    place,
                    argument.span,
                )
            } else {
                // An element or a static already reaches the call as a copy of the stored value; a binding the caller
                // cannot change, or a loop variable, handed to a parameter the callee never changes is accepted too:
                // the call passes it as its parameter's own marker says.
                continue;
            };
            let already_reported = self
                .errors
                .iter()
                .any(|existing| existing.span == error.span && existing.message == error.message);
            if !already_reported {
                self.errors.push(error);
            }
        }
    }

    /// Return whether a call went through a local that is reassigned somewhere in the module, so the callable it runs
    /// is not known at the call.
    fn call_goes_through_a_reassigned_local(&self, through_local: Option<Span>) -> bool {
        through_local.is_some_and(|local| self.mut_params.reassigned_locals.contains(&local))
    }

    /// Return every (callable, parameter) pair whose caller-visible parameter the callable changes, transitively.
    pub(in crate::typechecker) fn legacy_mut_param_change_closure(&self) -> HashSet<(CanonicalSymbolId, String)> {
        let mut changed = self
            .mut_params
            .changed
            .iter()
            .flat_map(|(identity, params)| params.iter().map(move |param| (identity.clone(), param.clone())))
            .collect::<HashSet<_>>();
        loop {
            let mut grew = false;
            for forward in &self.mut_params.forwards {
                let callee_changes = !self.mut_params.bodies_checked.contains(&forward.callee)
                    || self.call_goes_through_a_reassigned_local(forward.through_local)
                    || changed.contains(&(forward.callee.clone(), forward.callee_param.clone()));
                if callee_changes && changed.insert((forward.owner.clone(), forward.param.clone())) {
                    grew = true;
                }
            }
            if !grew {
                return changed;
            }
        }
    }
}
