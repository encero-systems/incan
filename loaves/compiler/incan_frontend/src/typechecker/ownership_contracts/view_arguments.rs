//! RFC 164 one type: a native `&T` value is a `Borrowed[T]` value and a native `&mut T` value is a `BorrowedMut[T]`
//! value, so either passes to an explicit view parameter as the view it already is.

use super::TypeChecker;
use crate::ast::{CallArg, ParamKind};
use crate::symbols::{CallableParam, ResolvedType};
use incan_lang::lang::types::ownership::OwnershipMode;
use incan_semantics_core::CanonicalSymbolId;

impl TypeChecker {
    /// Present each argument bound to a direct `Borrowed[T]` or `BorrowedMut[T]` slot of the selected declaration by
    /// its value type when it is a native reference whose access that mode admits: `&T` or `&mut T` for a shared
    /// view, `&mut T` for an exclusive one. The argument's checked expression keeps its native type, so lowering
    /// passes the reference itself with no adjustment or copy; only its compatibility with the parameter's value type
    /// is decided here. A shared `&T` stays refused for an exclusive view. Slots after an unpacked argument keep the
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
            let value = match (mode, actual) {
                (Some(OwnershipMode::Borrowed), ResolvedType::Ref(value) | ResolvedType::RefMut(value))
                | (Some(OwnershipMode::BorrowedMut), ResolvedType::RefMut(value)) => value.as_ref(),
                _ => continue,
            };
            presented.get_or_insert_with(|| arg_types.to_vec())[position] = value.clone();
        }
        presented
    }
}
