//! Checked private C storage and bridge operations, projected before any source nominal reaches native lowering.

use super::{BodyBuilder, IncanType, TypeCheckInfo, ast, bir, checked_foreign, hir_span};
use crate::typechecker::COutputMode;
use incan_lang::lang::c_abi;
use incan_lang::lang::types::numerics::NumericTypeId;
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
    let span_carrier = span_storage(name).or_else(|| span_pointer(name));
    let selected = name == c_abi::C_STRING_TYPE_ID
        || name == c_abi::SCOPED_C_STRING_VIEW_TYPE_ID
        || name == c_abi::pointer_type_identity(false, "c.c_char")
        || span_carrier.is_some()
        || facts.c_abi.output_slots.iter().any(|slot| slot.identity == name);
    if !selected {
        return None;
    }
    if facts.declarations.named_type_identities.contains_key(name) {
        return Some(IncanType::Unsupported(
            "source declaration shadows a private checked C carrier".into(),
        ));
    }
    if span_carrier.is_some() {
        return span_carrier;
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

/// The owned buffer a typed span carrier holds: the `bytes` or `list[f32]` storage its constructor moved in.
fn span_storage(name: &str) -> Option<IncanType> {
    let element = match name {
        c_abi::C_BYTES_SPAN_TYPE_ID | c_abi::C_MUTABLE_BYTES_SPAN_TYPE_ID => NumericTypeId::U8,
        c_abi::C_F32_SPAN_TYPE_ID | c_abi::C_MUTABLE_F32_SPAN_TYPE_ID => NumericTypeId::F32,
        _ => return None,
    };
    Some(IncanType::Generic {
        base: "List".into(),
        args: vec![IncanType::Primitive(IncanPrimitiveType::Numeric(element))],
    })
}

/// The raw element pointer a span's pointer method returns, selected by its checked pointer identity.
fn span_pointer(name: &str) -> Option<IncanType> {
    let carriers = [
        (false, "c.u8", NumericTypeId::U8),
        (true, "c.u8", NumericTypeId::U8),
        (false, "c.f32", NumericTypeId::F32),
        (true, "c.f32", NumericTypeId::F32),
    ];
    let (mutable, _, element) = carriers
        .into_iter()
        .find(|(mutable, scalar, _)| name == c_abi::pointer_type_identity(*mutable, scalar))?;
    Some(IncanType::NativePointer {
        mutable,
        pointee: Box::new(IncanType::Primitive(IncanPrimitiveType::Numeric(element))),
    })
}

/// How a bridge operation reads its receiver value, when it has one.
#[derive(Clone, Copy)]
enum ReceiverRead {
    /// A shared loan of the receiver's storage for the operation.
    Borrow,
    /// An exclusive loan of the receiver's storage for the operation.
    MutBorrow,
    /// A copy of a scalar or view receiver.
    Copy,
}

/// The receiver read of each operation; constructors have no receiver value.
fn receiver_read(operation: bir::CheckedBridgeOperation) -> Option<ReceiverRead> {
    match operation {
        bir::CheckedBridgeOperation::CStringPointer
        | bir::CheckedBridgeOperation::SpanConstPointer
        | bir::CheckedBridgeOperation::SpanCount
        | bir::CheckedBridgeOperation::FinishSpan => Some(ReceiverRead::Borrow),
        bir::CheckedBridgeOperation::SpanMutPointer => Some(ReceiverRead::MutBorrow),
        bir::CheckedBridgeOperation::CopyUtf8 | bir::CheckedBridgeOperation::TakeOutputSlot => Some(ReceiverRead::Copy),
        bir::CheckedBridgeOperation::MakeCString
        | bir::CheckedBridgeOperation::MakeSpan
        | bir::CheckedBridgeOperation::NewOutputSlot
        | bir::CheckedBridgeOperation::NewInOutSlot => None,
    }
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
        if let Some(read_kind) = receiver_read(operation) {
            let mut value = self.lower_expr_to_operand(receiver, scope, out);
            if let bir::Operand::Place(read) = &mut value {
                match read_kind {
                    ReceiverRead::Borrow => self.retain_argument_borrow(read, false),
                    ReceiverRead::MutBorrow => self.retain_argument_borrow(read, true),
                    ReceiverRead::Copy => read.fact = bir::OwnershipFact::Copy,
                }
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
