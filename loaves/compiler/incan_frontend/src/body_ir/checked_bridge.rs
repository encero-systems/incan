//! Checked private C storage and bridge operations, projected before any source nominal reaches native lowering.

use super::{BodyBuilder, IncanType, TypeCheckInfo, ast, bir, checked_foreign, hir_span};
use crate::typechecker::COutputMode;
use incan_lang::lang::c_abi;
use incan_semantics_core::{CanonicalSymbolId, IncanPrimitiveType, SemanticSourceTargetKind, SymbolOrigin};

/// Preserve the actual standard namespace declaration rather than reconstructing authority from a source alias.
fn namespace(facts: &TypeCheckInfo) -> Option<CanonicalSymbolId> {
    facts.declarations.named_type_identities.values()
        .chain(facts.declarations.qualified_type_references.values().map(|reference| &reference.identity))
        .find(|identity| identity.kind == SemanticSourceTargetKind::Class && identity.declaration_name == "c"
            && matches!(&identity.origin, SymbolOrigin::Package { library, .. } if library == "incan_stdlib_interop"))
        .cloned()
}

/// The scalar allocation is private; its pointer handle preserves aliasing without moving its storage.
pub(super) fn constructor_storage(span: ast::Span, facts: &TypeCheckInfo) -> Option<IncanType> {
    let slot = facts
        .c_abi
        .output_slots
        .iter()
        .find(|slot| slot.constructor_span == span)?;
    if !facts.c_abi.verified_bindings.contains(&slot.binding) {
        return Some(IncanType::Unsupported("C output slot has no verified binding".into()));
    }
    let value = checked_foreign::carrier(&slot.value)
        .unwrap_or_else(|| IncanType::Unsupported("checked C output slot value carrier".into()));
    Some(IncanType::NativePointer {
        mutable: true,
        pointee: Box::new(value),
    })
}

/// Project only compiler-selected private carriers; a same-spelled source declaration gains no bridge authority.
pub(super) fn private_storage(name: &str, facts: &TypeCheckInfo) -> Option<IncanType> {
    if facts.c_abi.bridge_calls.is_empty() && facts.c_abi.output_slots.is_empty() && facts.c_abi.raw_calls.is_empty() {
        return None;
    }
    let selected = name == c_abi::C_STRING_TYPE_ID
        || name == c_abi::SCOPED_C_STRING_VIEW_TYPE_ID
        || name == c_abi::pointer_type_identity(false, "c.c_char")
        || facts.c_abi.output_slots.iter().any(|slot| slot.identity == name);
    if !selected {
        return None;
    }
    if facts.declarations.named_type_identities.contains_key(name) {
        return Some(IncanType::Unsupported(
            "source declaration shadows a private checked C carrier".into(),
        ));
    }
    if name == c_abi::C_STRING_TYPE_ID {
        return Some(IncanType::Generic {
            base: "List".into(),
            args: vec![IncanType::Primitive(IncanPrimitiveType::Numeric(
                incan_lang::lang::types::numerics::NumericTypeId::U8,
            ))],
        });
    }
    if let Some(slot) = facts.c_abi.output_slots.iter().find(|slot| slot.identity == name) {
        return constructor_storage(slot.constructor_span, facts);
    }
    Some(IncanType::NativePointer {
        mutable: false,
        pointee: Box::new(IncanType::NativeCChar),
    })
}

impl BodyBuilder<'_, '_> {
    /// Consume only span-specific checker operations or a verified output constructor's exact descriptor.
    /// Method spelling has no dispatch role here, and every refusal retains its checked result storage.
    pub(super) fn lower_checked_bridge_call(
        &mut self,
        receiver: &ast::Spanned<ast::Expr>,
        args: &[ast::CallArg],
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> Option<bir::Operand> {
        let slot = self
            .type_info
            .c_abi
            .output_slots
            .iter()
            .find(|slot| slot.constructor_span == span)
            .cloned();
        let selected = self
            .type_info
            .c_abi
            .bridge_calls
            .iter()
            .find(|call| call.span == span)
            .cloned();
        if slot.is_none() && selected.is_none() {
            return None;
        }
        let result_type = self.resolve_ty(span);
        let operation = slot
            .as_ref()
            .map(|slot| match slot.mode {
                COutputMode::Out => bir::CheckedBridgeOperation::NewOutputSlot,
                COutputMode::InOut => bir::CheckedBridgeOperation::NewInOutSlot,
            })
            .or_else(|| selected.as_ref().map(|selected| selected.operation))?;
        let selected_slot = slot.as_ref().or_else(|| {
            selected
                .as_ref()
                .and_then(|selected| selected.slot_identity.as_ref())
                .and_then(|identity| {
                    self.type_info
                        .c_abi
                        .output_slots
                        .iter()
                        .find(|slot| &slot.identity == identity)
                })
        });
        let authority = selected_slot
            .and_then(|slot| super::parameter_types::retained_nominal_identity(&slot.binding, self.type_info).cloned())
            .or_else(|| namespace(self.type_info));
        let Some(authority) = authority else {
            return Some(self.unsupported_typed_operand(
                "checked C bridge lost its declaration authority".into(),
                result_type,
                scope,
                hir_span(span),
                out,
            ));
        };
        let mut values = Vec::new();
        if matches!(
            operation,
            bir::CheckedBridgeOperation::CStringPointer
                | bir::CheckedBridgeOperation::CopyUtf8
                | bir::CheckedBridgeOperation::TakeOutputSlot
        ) {
            let mut value = self.lower_expr_to_operand(receiver, scope, out);
            if operation == bir::CheckedBridgeOperation::CStringPointer
                && let bir::Operand::Place(read) = &mut value
            {
                self.retain_argument_borrow(read, false);
            }
            if matches!(
                operation,
                bir::CheckedBridgeOperation::CopyUtf8 | bir::CheckedBridgeOperation::TakeOutputSlot
            ) && let bir::Operand::Place(read) = &mut value
            {
                read.fact = bir::OwnershipFact::Copy;
            }
            values.push(bir::ArgumentElement::One(value));
        }
        for argument in args {
            let (ast::CallArg::Positional(value) | ast::CallArg::Named(_, value)) = argument else {
                return Some(self.unsupported_typed_operand(
                    "checked C bridge cannot spread arguments".into(),
                    result_type,
                    scope,
                    hir_span(span),
                    out,
                ));
            };
            let mut value = self.lower_expr_to_operand(value, scope, out);
            if operation == bir::CheckedBridgeOperation::MakeCString
                && let bir::Operand::Place(read) = &mut value
            {
                self.retain_argument_borrow(read, false);
            }
            values.push(bir::ArgumentElement::One(value));
        }
        Some(self.push_call_temp(
            bir::Callee::CheckedBridge(Box::new(bir::CheckedBridgeCall {
                authority,
                operation,
                return_type: result_type.clone(),
            })),
            values,
            result_type,
            scope,
            hir_span(span),
            false,
            out,
        ))
    }
}
