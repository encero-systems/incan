//! Checked enum payload facts at construction and pattern boundaries; legacy expression types stay unchanged.

use super::{BodyBuilder, CanonicalSymbolId, IncanType, SemanticSourceTargetKind, ast};

impl BodyBuilder<'_, '_> {
    /// Pin enum arguments from an exact nongeneric declaration when checking omitted a call-site snapshot.
    pub(super) fn settle_declared_enum_arguments(&mut self, identity: &CanonicalSymbolId, arguments: &[ast::CallArg]) {
        let Some(binding) = self
            .type_info
            .declarations
            .function_bindings_by_span
            .values()
            .chain(self.type_info.declarations.function_bindings.values())
            .find(|binding| binding.identity.as_ref() == Some(identity))
        else {
            return;
        };
        let types: Vec<_> = binding
            .params
            .iter()
            .map(|parameter| self.checked_type(&parameter.ty))
            .collect();
        if types.iter().any(|ty| !closed_signature_type(ty)) {
            return;
        }
        let slots: Vec<_> = binding
            .params
            .iter()
            .map(super::args::DeclaredSlot::from_checked_param)
            .collect();
        let Ok(planned) = super::args::plan_declared_args("enum argument", &slots, arguments) else {
            return;
        };
        for (slot, argument) in planned {
            if let Some(destination) = types.get(slot) {
                self.settle_enum_result_value(argument, destination);
            }
        }
    }

    /// Instantiate the selected variant's checked fields in the destination's ordered declaration arguments.
    pub(super) fn enum_payload_types(
        &self,
        identity: &CanonicalSymbolId,
        destination: &IncanType,
    ) -> Option<Vec<IncanType>> {
        let (owner, variant) = self.local_enum_declarations.values().find_map(|owner| {
            owner
                .variants
                .iter()
                .find(|variant| variant.canonical == *identity)
                .map(|variant| (owner, variant))
        })?;
        let arguments = match destination {
            IncanType::Generic { base, args }
                if self.type_info.declarations.named_type_identities.get(base) == Some(&owner.canonical)
                    && args.len() == owner.type_parameters.len() =>
            {
                args.as_slice()
            }
            IncanType::Named(name) if owner.type_parameters.is_empty() && *name == owner.name => &[],
            _ => return None,
        };
        Some(
            variant
                .fields
                .iter()
                .map(|field| close_payload_type(field, &owner.type_parameters, arguments))
                .collect(),
        )
    }

    /// Retain a checked generic destination for an exact selected variant and recursively pin its literal payloads.
    pub(super) fn settle_enum_result_value(
        &mut self,
        value: &ast::Spanned<ast::Expr>,
        destination: &IncanType,
    ) -> bool {
        let (span, arguments): (_, &[ast::CallArg]) = match &value.node {
            ast::Expr::Call(callee, _, arguments) => (callee.span, arguments),
            ast::Expr::MethodCall(_, _, _, arguments) => (value.span, arguments),
            ast::Expr::Field(_, _) => (value.span, &[]),
            _ => return false,
        };
        let Some(identity) = self
            .type_info
            .resolved_identity(span)
            .filter(|identity| identity.kind == SemanticSourceTargetKind::Variant)
        else {
            return false;
        };
        let Some(fields) = self.enum_payload_types(identity, destination) else {
            return false;
        };
        if fields.len() != arguments.len() {
            return false;
        }
        self.settled_types
            .insert((value.span.start, value.span.end), destination.clone());
        for (argument, field) in arguments.iter().zip(fields) {
            if let ast::CallArg::Positional(argument) = argument {
                self.settle_open_result_value(argument, &field);
            }
        }
        true
    }
}

/// Reject every unresolved declaration placeholder before using a signature without call-site substitution.
fn closed_signature_type(ty: &IncanType) -> bool {
    match ty {
        IncanType::TypeVar(_) | IncanType::SelfType | IncanType::Unknown | IncanType::Infer => false,
        IncanType::Generic { args, .. } | IncanType::Tuple(args) => args.iter().all(closed_signature_type),
        IncanType::Ref(inner) | IncanType::RefMut(inner) | IncanType::TypeToken(inner) => closed_signature_type(inner),
        IncanType::Function { params, return_type } => {
            params.iter().all(|parameter| closed_signature_type(&parameter.ty)) && closed_signature_type(return_type)
        }
        _ => true,
    }
}

/// Replace only the declaration's retained placeholders; nominal leaves and unrelated binders remain untouched.
fn close_payload_type(ty: &IncanType, parameters: &[String], arguments: &[IncanType]) -> IncanType {
    match ty {
        IncanType::TypeVar(name) => parameters
            .iter()
            .position(|parameter| parameter == name)
            .and_then(|slot| arguments.get(slot))
            .cloned()
            .unwrap_or_else(|| ty.clone()),
        IncanType::Generic { base, args } => IncanType::Generic {
            base: base.clone(),
            args: args
                .iter()
                .map(|ty| close_payload_type(ty, parameters, arguments))
                .collect(),
        },
        IncanType::Tuple(items) => IncanType::Tuple(
            items
                .iter()
                .map(|ty| close_payload_type(ty, parameters, arguments))
                .collect(),
        ),
        IncanType::Ref(inner) => IncanType::Ref(Box::new(close_payload_type(inner, parameters, arguments))),
        IncanType::RefMut(inner) => IncanType::RefMut(Box::new(close_payload_type(inner, parameters, arguments))),
        _ => ty.clone(),
    }
}
