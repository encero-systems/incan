//! Whether a method call changes the value it is called on.
//!
//! Two checks ask this question of the same call. A write through `self` inside a plain-`self` method is refused when
//! the method changes its receiver (#1723), and an argument for a caller-visible `mut` parameter is refused when the
//! callee may change it (#1773, `INCAN-T0117`). Both read the one answer [`TypeChecker::method_receiver_change`]
//! gives:
//!
//! - a builtin `list`, `dict` or `set` method answers from its registry entry (`changes_receiver`);
//! - a method of a `Generator` is an iterator method, and each one advances or consumes the generator;
//! - the methods of a tuple, `Option`, `Result`, a frozen collection, a string, bytes or a number only read it;
//! - a method of a declared `model`, `class`, `enum` or newtype answers from its declarations: its own methods and
//!   overloads, an alias resolved to its target, and when it declares none by that name, the methods of the traits it
//!   adopts;
//! - inside a trait's default method, a method called on `Self` answers from the trait's own declaration.
//!
//! Anything else is [`ReceiverChange::Unknown`], and each check applies its own documented policy to it: the refusal
//! through `self` refuses only a call it can read from a declaration, while the argument check counts an unknown call
//! as a change, so an immutable argument is never copied when the change would matter.

use crate::ast::{Receiver, Span};
use crate::symbols::{ResolvedType, TypeInfo};
use crate::typechecker::helpers::collection_type_id;
use incan_lang::lang::surface::{dict_methods, iterator_methods, list_methods, set_methods};
use incan_lang::lang::types::collections::CollectionTypeId;

use super::TypeChecker;

/// What a method call does to the value it is called on, as the declarations it may run say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::typechecker) enum ReceiverChange {
    /// Every declaration the call may run only reads its receiver.
    Reads,
    /// Every declaration the call may run changes its receiver.
    Changes,
    /// The checker cannot tell: no declaration by that name is known, the candidate declarations disagree, or the
    /// receiver's type names no declaration the checker reads (a type parameter, a Rust type, a function).
    Unknown,
}

impl ReceiverChange {
    /// Answer from a registry fact, when the method is registered at all.
    fn from_registry(changes: Option<bool>) -> Self {
        match changes {
            Some(true) => Self::Changes,
            Some(false) => Self::Reads,
            None => Self::Unknown,
        }
    }

    /// Answer from the receivers of the declarations a call may run: `None` when there are none.
    fn from_receivers(receivers: &[Option<Receiver>]) -> Option<Self> {
        if receivers.is_empty() {
            return None;
        }
        let changing = receivers
            .iter()
            .filter(|receiver| **receiver == Some(Receiver::Mutable))
            .count();
        Some(if changing == receivers.len() {
            Self::Changes
        } else if changing == 0 {
            Self::Reads
        } else {
            Self::Unknown
        })
    }
}

impl TypeChecker {
    /// Return what calling `method` on a value of `receiver_ty` does to that value; `span` locates the call.
    ///
    /// The answer never reports a diagnostic: resolving a method an adopted trait provides can find an ambiguity, and
    /// the call's own resolution reports that.
    pub(in crate::typechecker) fn method_receiver_change(
        &mut self,
        receiver_ty: &ResolvedType,
        method: &str,
        span: Span,
    ) -> ReceiverChange {
        match receiver_ty {
            ResolvedType::Generic(name, _) => match collection_type_id(name.as_str()) {
                Some(CollectionTypeId::List) => {
                    ReceiverChange::from_registry(list_methods::from_str(method).map(list_methods::changes_receiver))
                }
                Some(CollectionTypeId::Dict) => {
                    ReceiverChange::from_registry(dict_methods::from_str(method).map(dict_methods::changes_receiver))
                }
                Some(CollectionTypeId::Set) => {
                    ReceiverChange::from_registry(set_methods::from_str(method).map(set_methods::changes_receiver))
                }
                Some(CollectionTypeId::Generator) => {
                    ReceiverChange::from_registry(iterator_methods::from_str(method).map(|_| true))
                }
                Some(
                    CollectionTypeId::Tuple
                    | CollectionTypeId::Option
                    | CollectionTypeId::Result
                    | CollectionTypeId::FrozenList
                    | CollectionTypeId::FrozenDict
                    | CollectionTypeId::FrozenSet,
                ) => ReceiverChange::Reads,
                None => self.nominal_receiver_change(name, method, span),
            },
            ResolvedType::Named(name) => self.nominal_receiver_change(name, method, span),
            ResolvedType::SelfType => self
                .current_trait_name
                .as_deref()
                .and_then(|trait_name| self.lookup_semantic_trait_info(trait_name))
                .and_then(|info| info.methods.get(method))
                .map_or(ReceiverChange::Unknown, |info| {
                    if info.receiver == Some(Receiver::Mutable) {
                        ReceiverChange::Changes
                    } else {
                        ReceiverChange::Reads
                    }
                }),
            ResolvedType::Int
            | ResolvedType::Float
            | ResolvedType::Numeric(_)
            | ResolvedType::Bool
            | ResolvedType::Str
            | ResolvedType::Bytes
            | ResolvedType::FrozenStr
            | ResolvedType::FrozenBytes
            | ResolvedType::FrozenList(_)
            | ResolvedType::FrozenDict(_, _)
            | ResolvedType::FrozenSet(_)
            | ResolvedType::Tuple(_)
            | ResolvedType::Unit => ReceiverChange::Reads,
            _ => ReceiverChange::Unknown,
        }
    }

    /// Answer for a method of the source type `type_name` from its declarations.
    ///
    /// The type's own method table and overload set answer first, after resolving a method alias to its target; when
    /// the type declares nothing by that name, the methods its adopted traits provide answer instead. A name the
    /// checker does not know as a source type with a method table (a builtin, a type alias) is unknown.
    fn nominal_receiver_change(&mut self, type_name: &str, method: &str, span: Span) -> ReceiverChange {
        let Some((declared, adoptions)) = self.declared_method_receivers_and_adoptions(type_name, method) else {
            return ReceiverChange::Unknown;
        };
        if let Some(answer) = ReceiverChange::from_receivers(&declared) {
            return answer;
        }
        let reported = self.errors.len();
        let adopted = adoptions
            .iter()
            .filter_map(|adoption| {
                self.trait_method_entry_resolved_for_adoption(adoption, method, span)
                    .map(|entry| entry.info.receiver)
            })
            .collect::<Vec<_>>();
        self.errors.truncate(reported);
        ReceiverChange::from_receivers(&adopted).unwrap_or(ReceiverChange::Unknown)
    }

    /// Collect the receivers of every declaration of `method` that the source type `type_name` itself carries, an
    /// alias resolved to its target, together with the traits the type adopts.
    ///
    /// `None` when the name is not a source-declared type with a method table (a builtin or a type alias). The
    /// receivers are the type's own answer; the adoptions let the caller ask the traits when that answer is empty,
    /// and are returned owned because that question needs the checker mutably.
    pub(in crate::typechecker) fn declared_method_receivers_and_adoptions(
        &self,
        type_name: &str,
        method: &str,
    ) -> Option<(Vec<Option<Receiver>>, Vec<crate::symbols::TypeBoundInfo>)> {
        let (aliases, methods, overloads, adoptions) = match self.lookup_semantic_type_info(type_name)? {
            TypeInfo::Class(class) => (
                Some(&class.method_aliases),
                &class.methods,
                &class.method_overloads,
                &class.trait_adoptions,
            ),
            TypeInfo::Model(model) => (
                Some(&model.method_aliases),
                &model.methods,
                &model.method_overloads,
                &model.trait_adoptions,
            ),
            TypeInfo::Newtype(newtype) => (
                Some(&newtype.method_aliases),
                &newtype.methods,
                &newtype.method_overloads,
                &newtype.trait_adoptions,
            ),
            TypeInfo::Enum(enum_info) => (
                None,
                &enum_info.methods,
                &enum_info.method_overloads,
                &enum_info.trait_adoptions,
            ),
            TypeInfo::Builtin | TypeInfo::TypeAlias => return None,
        };
        let target = aliases
            .and_then(|aliases| aliases.get(method))
            .map(String::as_str)
            .unwrap_or(method);
        let mut receivers = methods
            .get(target)
            .map(|info| info.receiver)
            .into_iter()
            .collect::<Vec<_>>();
        if let Some(candidates) = overloads.get(target) {
            receivers.extend(candidates.iter().map(|info| info.receiver));
        }
        Some((receivers, adoptions.clone()))
    }
}
