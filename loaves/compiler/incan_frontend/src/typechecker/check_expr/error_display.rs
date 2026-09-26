//! Display of `Error` adopters that define no `__str__` (#1778).
//!
//! A model or class has a textual form only through `__str__`. An `Error` adopter carries its human-readable text in
//! `message()` instead, so wherever such a value is displayed -- an f-string `{value}` part, `str(value)`, and each
//! `print`/`println` argument -- it renders that text. That holds for a concrete adopter, for one whose `message`
//! comes from a trait default, for a value of a type parameter bounded by `Error`, and for `self` inside a default
//! method of a trait that extends `Error`. The checker proves which operands take that route and resolves their
//! `message()` call exactly as a written `value.message()` is resolved, trait dispatch included; lowering rewrites each
//! recorded operand into that call. Nothing here refuses a program: every other operand keeps its existing display
//! path.

use incan_lang::lang::magic_methods::{self, MagicMethodId};
use incan_lang::lang::traits::{self as core_traits, TraitId};

use std::collections::BTreeSet;

use incan_semantics_core::CanonicalSymbolId;

use super::TypeChecker;
use crate::ast::Span;
use crate::symbols::{CallableParam, ResolvedType, TypeBoundInfo, TypeInfo};
use crate::typechecker::TypeCheckInfo;
use crate::typechecker::type_info::{ErrorMessageDisplay, ResolvedMethodCall};

/// The `Error` method whose text a displayed adopter renders.
const ERROR_MESSAGE_METHOD: &str = "message";

impl TypeChecker {
    /// Record that one display operand renders through `message()` when its checked type adopts `Error` and has no
    /// `Display` of its own.
    ///
    /// Callers pass each operand in a display position with the type `check_expr` just returned for it. An operand of
    /// any other type, or one whose `message()` does not resolve to text, records nothing.
    pub(in crate::typechecker) fn record_error_message_display(
        &mut self,
        operand_span: Span,
        operand_ty: &ResolvedType,
    ) {
        if matches!(operand_ty, ResolvedType::SelfType) {
            // A written `self.message()` in a trait default resolves no further than the trait's `Self`; the display
            // records the same, with the adopters that display themselves, and lowering decides per adopter.
            if let Some(trait_name) = self.current_trait_name.clone()
                && self.trait_default_self_displays_through_error_message(&trait_name)
            {
                let self_displaying_adopters = self.self_displaying_adopters_of(&trait_name);
                self.type_info.record_error_message_display(
                    operand_span,
                    ErrorMessageDisplay {
                        identity: None,
                        dispatch: None,
                        receiver_is_trait_self: true,
                        self_displaying_adopters,
                    },
                );
            }
            return;
        }
        if !self.displays_through_error_message(operand_ty) {
            return;
        }
        if let Some(display) = self.resolve_error_message_call(operand_span, operand_ty) {
            self.type_info.record_error_message_display(operand_span, display);
        }
    }

    /// Return whether `self` inside a default method of `trait_name` renders its `message()`.
    ///
    /// That holds in a default method of a trait that extends `Error` (or is `Error`) when neither the trait nor any
    /// supertrait gives `Self` a `Display`: a `Display` supertrait or a `__str__` declaration. Which adopters display
    /// themselves anyway is [`Self::self_displaying_adopters_of`].
    fn trait_default_self_displays_through_error_message(&self, trait_name: &str) -> bool {
        self.trait_reaches(trait_name, TraitId::Error)
            && !self.trait_reaches(trait_name, TraitId::Display)
            && !self.trait_supplies_str(trait_name)
    }

    /// Return the models and classes of this module that adopt `trait_name` and have a `Display` of their own.
    ///
    /// A trait default is expanded into each adopter, and `{self}` there follows the adopter: one that has a `Display`
    /// by the same rule [`Self::displays_through_error_message`] applies (a declared or inherited `__str__`, one
    /// supplied by any adopted trait, or a `Display` adoption or derive) keeps it, every other adopter renders
    /// `message()`. Lowering reads this set while expanding the default.
    fn self_displaying_adopters_of(&self, trait_name: &str) -> BTreeSet<String> {
        self.symbols
            .active_module_source_bindings()
            .map(|(_, name, _)| name.to_string())
            .filter(|name| self.type_implements_trait(name, trait_name))
            .filter(|name| self.nominal_has_own_display(name) == Some(true))
            .collect()
    }

    /// Return whether a model or class has a `Display` of its own, or `None` when `type_name` is neither.
    ///
    /// A `Display` comes from a `__str__` the type declares or inherits, a `__str__` an adopted trait (or one of its
    /// supertraits) declares, or a `Display` adoption or derive.
    fn nominal_has_own_display(&self, type_name: &str) -> Option<bool> {
        let (methods, trait_adoptions) = match self.lookup_semantic_type_info(type_name)? {
            TypeInfo::Model(model) => (&model.methods, &model.trait_adoptions),
            TypeInfo::Class(class) => (&class.methods, &class.trait_adoptions),
            _ => return None,
        };
        Some(
            self.type_implements_trait(type_name, core_traits::as_str(TraitId::Display))
                || methods.contains_key(magic_methods::as_str(MagicMethodId::Str))
                || trait_adoptions.iter().any(|adoption| self.bound_supplies_str(adoption)),
        )
    }

    /// Return whether a displayed value of this type renders its `message()`.
    ///
    /// The route applies to a model or class that adopts `Error` (directly or through a subtrait) and to a type
    /// parameter bounded by `Error`, as long as nothing gives the value a `Display` of its own: a `__str__` that is
    /// declared, inherited or supplied by an adopted trait, or a `Display` adoption, derive or bound.
    fn displays_through_error_message(&self, ty: &ResolvedType) -> bool {
        if let Some(placeholder) = self.generic_placeholder_name(ty) {
            let bounds = self.placeholder_bounds(placeholder);
            return bounds
                .iter()
                .any(|bound| self.bound_reaches_trait(bound, TraitId::Error))
                && !bounds
                    .iter()
                    .any(|bound| self.bound_reaches_trait(bound, TraitId::Display) || self.bound_supplies_str(bound));
        }
        let (ResolvedType::Named(type_name) | ResolvedType::Generic(type_name, _)) = ty else {
            return false;
        };
        self.nominal_has_own_display(type_name) == Some(false)
            && self.type_implements_trait(type_name, core_traits::as_str(TraitId::Error))
    }

    /// Resolve the `message()` call one display operand renders through, the way a written `value.message()` is.
    ///
    /// The ordinary method resolution records its facts at the call's span, in exactly four maps:
    /// `calls.resolved_method_calls` (the trait dispatch), `references.resolved_identities` (the selected
    /// declaration), `calls.call_site_callable_params` and `calls.call_site_monomorph_type_args`. The display has no
    /// call of its own, so resolution runs under a key no expression can have (the operand's span with its bounds
    /// reversed); [`ProbeFacts`] takes whatever those four maps hold under the key before the probe, takes what the
    /// probe wrote there afterwards, and puts the earlier entries back, so the probe leaves the facts as it found them
    /// at the cost of four map operations. Diagnostics raised while probing are discarded: the probe is not something
    /// the program wrote.
    fn resolve_error_message_call(
        &mut self,
        operand_span: Span,
        operand_ty: &ResolvedType,
    ) -> Option<ErrorMessageDisplay> {
        if operand_span.start >= operand_span.end {
            return None;
        }
        let probe = Span::new(operand_span.end, operand_span.start);
        let before = ProbeFacts::take(&mut self.type_info, probe);
        let first_error = self.errors.len();
        // The two resolutions a written `value.message()` goes through, in the same order: the owner's sole direct or
        // adopted method first, then the general resolution that also covers a type parameter's bounds.
        let message_ty = self
            .resolve_synthesized_source_method_call(
                ERROR_MESSAGE_METHOD,
                operand_ty,
                operand_span,
                probe,
                Some(&ResolvedType::Str),
            )
            .or_else(|| {
                self.resolve_operator_dunder(
                    operand_ty,
                    ERROR_MESSAGE_METHOD,
                    &[],
                    &[],
                    probe,
                    Some(&ResolvedType::Str),
                )
            });
        self.errors.truncate(first_error);
        let probed = ProbeFacts::take(&mut self.type_info, probe);
        before.restore(&mut self.type_info, probe);
        let renders_text = message_ty.is_some_and(|ty| self.types_compatible(&ty, &ResolvedType::Str));
        renders_text.then(|| ErrorMessageDisplay {
            identity: probed.identity,
            dispatch: probed.method_call.map(|call| call.dispatch),
            receiver_is_trait_self: false,
            self_displaying_adopters: BTreeSet::new(),
        })
    }

    /// Return the trait bounds active for one type parameter, innermost scope first.
    fn placeholder_bounds(&self, placeholder: &str) -> Vec<TypeBoundInfo> {
        self.current_type_param_bound_details
            .iter()
            .rev()
            .find_map(|frame| frame.get(placeholder).cloned())
            .unwrap_or_default()
    }

    /// Return whether one bound or adoption names `target` or a trait whose supertraits include it.
    fn bound_reaches_trait(&self, bound: &TypeBoundInfo, target: TraitId) -> bool {
        self.trait_reaches(&bound.name, target)
            || bound
                .source_name
                .as_deref()
                .is_some_and(|source_name| self.trait_name_matches(source_name, core_traits::as_str(target)))
    }

    /// Return whether the trait `trait_name` is `target` or has it among its supertraits.
    fn trait_reaches(&self, trait_name: &str, target: TraitId) -> bool {
        let target = core_traits::as_str(target);
        self.trait_name_matches(trait_name, target)
            || self
                .semantic_supertrait_closure(trait_name)
                .iter()
                .any(|(name, _)| self.trait_name_matches(name, target))
    }

    /// Return whether one bound or adoption, or any of its supertraits, declares `__str__`.
    fn bound_supplies_str(&self, bound: &TypeBoundInfo) -> bool {
        self.trait_supplies_str(&bound.name)
    }

    /// Return whether the trait `trait_name`, or any of its supertraits, declares `__str__`.
    fn trait_supplies_str(&self, trait_name: &str) -> bool {
        let str_method = magic_methods::as_str(MagicMethodId::Str);
        std::iter::once(trait_name.to_string())
            .chain(
                self.semantic_supertrait_closure(trait_name)
                    .into_iter()
                    .map(|(name, _)| name),
            )
            .any(|trait_name| {
                self.lookup_semantic_trait_info(&trait_name)
                    .is_some_and(|info| info.methods.contains_key(str_method))
            })
    }
}

/// The facts one method resolution records under its call span, taken out of the checker's facts by span key.
///
/// These four maps are everything the resolutions `resolve_error_message_call` runs write under the call span; see
/// that function for why the display probe takes and restores them.
struct ProbeFacts {
    /// Entry of `calls.resolved_method_calls`: the selected trait dispatch.
    method_call: Option<ResolvedMethodCall>,
    /// Entry of `references.resolved_identities`: the selected declaration.
    identity: Option<CanonicalSymbolId>,
    /// Entry of `calls.call_site_callable_params`.
    callable_params: Option<Vec<CallableParam>>,
    /// Entry of `calls.call_site_monomorph_type_args`.
    monomorph_type_args: Option<Vec<ResolvedType>>,
}

impl ProbeFacts {
    /// Remove and return the entries the four maps hold under `span`.
    fn take(info: &mut TypeCheckInfo, span: Span) -> Self {
        let key = (span.start, span.end);
        Self {
            method_call: info.calls.resolved_method_calls.remove(&key),
            identity: info.references.resolved_identities.remove(&key),
            callable_params: info.calls.call_site_callable_params.remove(&key),
            monomorph_type_args: info.calls.call_site_monomorph_type_args.remove(&key),
        }
    }

    /// Put these entries back under `span`, leaving absent ones absent.
    fn restore(self, info: &mut TypeCheckInfo, span: Span) {
        let key = (span.start, span.end);
        if let Some(method_call) = self.method_call {
            info.calls.resolved_method_calls.insert(key, method_call);
        }
        if let Some(identity) = self.identity {
            info.references.resolved_identities.insert(key, identity);
        }
        if let Some(params) = self.callable_params {
            info.calls.call_site_callable_params.insert(key, params);
        }
        if let Some(type_args) = self.monomorph_type_args {
            info.calls.call_site_monomorph_type_args.insert(key, type_args);
        }
    }
}
