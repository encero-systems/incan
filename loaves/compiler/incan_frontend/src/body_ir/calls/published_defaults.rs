//! Native record constructions that leave out fields whose defaults the record publishes.

use super::*;

impl BodyBuilder<'_, '_> {
    /// The public item path of the native record a checked construction at `span` builds, when its layout is retained.
    pub(super) fn native_construction_owner(&self, span: ast::Span) -> Option<String> {
        let owner = self.type_info.body_ir_native_constructor_identity(span)?;
        let path = incan_semantics_core::body_ir::native_nominal_item(owner)?;
        self.type_info.rust.checked_layouts.contains_key(&path).then_some(path)
    }

    /// Complete a native construction: after its written arguments, call the published default of each field it
    /// leaves out, in field order, and bind each result to its slot. The checker retains a defaulted slot only for a
    /// field whose record publishes its default, so the completed construction has no defaulted slot left.
    pub(super) fn fill_published_native_defaults(
        &mut self,
        owner: &str,
        lowered: (Vec<bir::Operand>, bir::ArgumentBinding),
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) -> Result<(Vec<bir::Operand>, bir::ArgumentBinding), String> {
        let (operands, binding) = lowered;
        let (arguments, defaulted_slots) = match binding {
            bir::ArgumentBinding::Resolved {
                arguments,
                defaulted_slots,
            } if !defaulted_slots.is_empty() => (arguments, defaulted_slots),
            complete => return Ok((operands, complete)),
        };
        let defaults = super::rust_calls::published_native_defaults(self.type_info, owner, &defaulted_slots)?;
        let written = arguments.len();
        let mut slots: Vec<(bir::BoundArgument, bir::Operand)> = arguments.into_iter().zip(operands).collect();
        for (index, (slot, path, ty)) in defaults.into_iter().enumerate() {
            let canonical = bir::rust_item_identity(&path).ok_or_else(|| format!("unnamed native default `{path}`"))?;
            let target = bir::NamedCallableTarget {
                name: path,
                direct_call_id: None,
                builtin: None,
                type_args: Vec::new(),
                binding: bir::ArgumentBinding::Resolved {
                    arguments: Vec::new(),
                    defaulted_slots: Vec::new(),
                },
                canonical: Some(canonical),
                receiver_type: None,
            };
            let callee = bir::Callee::Function(bir::CallableTarget::Named(target));
            let value = self.push_call_temp(callee, Vec::new(), ty, scope, span, false, out);
            let written_position = written + index;
            slots.push((bir::BoundArgument { slot, written_position }, value));
        }
        slots.sort_by_key(|(argument, _)| argument.slot);
        let (arguments, operands) = slots.into_iter().unzip();
        Ok((
            operands,
            bir::ArgumentBinding::Resolved {
                arguments,
                defaulted_slots: Vec::new(),
            },
        ))
    }
}
