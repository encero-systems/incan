//! RFC 164 one type: a native `&T` value is a `Borrowed[T]` value and a native `&mut T` value is a `BorrowedMut[T]`
//! value, so either is the view an explicit view position holds: a parameter, a local or a returned view.

use super::TypeChecker;
use crate::ast::{CallArg, ParamKind};
use crate::symbols::{CallableParam, ResolvedType};
use incan_lang::lang::types::ownership::OwnershipMode;
use incan_semantics_core::{CanonicalSymbolId, IncanInputAccess};

/// Return the value type a native reference presents in an explicit view position of `mode`, when that mode admits
/// its access: `&T` or `&mut T` for a shared view, `&mut T` for an exclusive one. A shared `&T` grants no exclusive
/// access, and an owned position takes no reference.
pub(in crate::typechecker) fn native_view_value(mode: OwnershipMode, actual: &ResolvedType) -> Option<&ResolvedType> {
    match (mode, actual) {
        (OwnershipMode::Borrowed, ResolvedType::Ref(value) | ResolvedType::RefMut(value))
        | (OwnershipMode::BorrowedMut | OwnershipMode::Reference, ResolvedType::RefMut(value)) => Some(value.as_ref()),
        _ => None,
    }
}

impl TypeChecker {
    /// Present each argument bound to a direct `Borrowed[T]` or `BorrowedMut[T]` slot of the selected declaration by
    /// its value type when it is a native reference whose access that mode admits. The argument's checked expression
    /// keeps its native type, so lowering passes the reference itself with no adjustment or copy; only its
    /// compatibility with the parameter's value type is decided here. Slots after an unpacked argument keep the
    /// written argument types.
    pub(in crate::typechecker) fn view_argument_types(
        &self,
        identity: Option<&CanonicalSymbolId>,
        params: &[CallableParam],
        args: &[CallArg],
        arg_types: &[ResolvedType],
    ) -> Option<Vec<ResolvedType>> {
        let contract = self.type_info.declarations.ownership_contracts.get(identity?)?;
        let positional_slots: Vec<usize> = params
            .iter()
            .enumerate()
            .filter(|(_, param)| param.kind == ParamKind::Normal && !param.is_partial_preset)
            .map(|(slot, _)| slot)
            .collect();
        let mut positional = positional_slots.iter();
        let mut presented: Option<Vec<ResolvedType>> = None;
        for (position, (arg, actual)) in args.iter().zip(arg_types).enumerate() {
            let slot = match arg {
                CallArg::Positional(_) => positional.next().copied(),
                CallArg::Named(name, _) => params
                    .iter()
                    .position(|param| param.kind == ParamKind::Normal && param.name() == Some(name.node.as_str())),
                CallArg::PositionalUnpack(_) | CallArg::KeywordUnpack(_) => break,
            };
            let Some(slot) = slot else { continue };
            let mode = contract
                .modes
                .iter()
                .find(|mode| mode.component.parameter == Some(slot) && mode.component.children.is_empty())
                .map(|mode| mode.mode);
            let Some(value) = mode.and_then(|mode| native_view_value(mode, actual)) else {
                continue;
            };
            presented.get_or_insert_with(|| arg_types.to_vec())[position] = value.clone();
        }
        presented
    }

    /// Present a returned native reference by its value type when the body being checked returns a direct view
    /// (`Borrowed[T]`, or `Reference[T]` for a native `&mut T`). The returned expression keeps its native type, and
    /// the body's source proof still has to show the reference comes from the declared source.
    pub(in crate::typechecker) fn returned_view_value(&self, actual: &ResolvedType) -> Option<ResolvedType> {
        let identity = self.current_body_identity()?;
        let contract = self.type_info.declarations.ownership_contracts.get(identity);
        let view = crate::ownership_metadata::returned_view(contract).ok()??;
        if view.carried.is_some() || !view.component.is_empty() {
            return None;
        }
        let mode = match view.access {
            IncanInputAccess::Exclusive => OwnershipMode::Reference,
            _ => OwnershipMode::Borrowed,
        };
        native_view_value(mode, actual).cloned()
    }
}
