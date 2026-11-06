//! Retain the selected builtin codec's ordered arguments and registry-owned constant defaults.

use super::args::{DeclaredSlot, plan_declared_args};
use super::{BodyBuilder, IncanCallableParamKind, IncanPrimitiveType, IncanType, ast, bir, hir_span};
use incan_lang::lang::builtins::BuiltinMethodId;
use incan_lang::lang::surface::bytes_methods::BytesMethodId;
use incan_lang::lang::surface::frozen_bytes_methods::FrozenBytesMethodId;
use incan_lang::lang::surface::string_methods::StringMethodId;
use incan_lang::lang::text_codecs::{DecodeErrorsPolicy, UTF8_ENCODING_LABEL};
use incan_semantics_core::{CanonicalSymbolId, SymbolOrigin};

/// Identify the existing checked builtin codec surface; lexical or source-owned lookalikes grant no codec facts.
fn builtin_codec(identity: &CanonicalSymbolId, operation: BuiltinMethodId) -> bool {
    let admitted = matches!(
        operation,
        BuiltinMethodId::Str(StringMethodId::Encode)
            | BuiltinMethodId::FrozenStr(StringMethodId::Encode)
            | BuiltinMethodId::Bytes(BytesMethodId::Decode)
            | BuiltinMethodId::FrozenBytes(FrozenBytesMethodId::Decode)
    );
    admitted
        && identity.origin == SymbolOrigin::Builtin
        && identity == &crate::symbols::canonical_builtin_member_identity(operation.owner(), operation.member())
}

impl BodyBuilder<'_, '_> {
    /// Retain the builtin codec's guarded evaluation: encoding, then decode policy, then receiver. A rejected label
    /// skips subsequent expressions. Only the checker's builtin identity authorizes this order and constant defaults.
    pub(super) fn lower_text_codec_call(
        &mut self,
        receiver: &ast::Spanned<ast::Expr>,
        args: &[ast::CallArg],
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> Option<bir::Operand> {
        let identity = self.selected_codec_identity(span)?.clone();
        let operation = self.type_info.resolved_builtin_method_call(span)?;
        if !builtin_codec(&identity, operation) {
            return None;
        }
        let defaults = match operation {
            BuiltinMethodId::Str(StringMethodId::Encode) | BuiltinMethodId::FrozenStr(StringMethodId::Encode) => {
                vec![("encoding", UTF8_ENCODING_LABEL)]
            }
            BuiltinMethodId::Bytes(BytesMethodId::Decode)
            | BuiltinMethodId::FrozenBytes(FrozenBytesMethodId::Decode) => vec![
                ("encoding", UTF8_ENCODING_LABEL),
                ("errors", DecodeErrorsPolicy::DEFAULT.as_str()),
            ],
            _ => return None,
        };
        let slots = defaults
            .iter()
            .map(|(name, _)| DeclaredSlot {
                name: Some((*name).to_owned()),
                has_default: true,
                is_partial_preset: false,
                kind: IncanCallableParamKind::Normal,
                ty: IncanType::Primitive(IncanPrimitiveType::Str),
                is_mut: false,
            })
            .collect::<Vec<_>>();
        let planned = match plan_declared_args(&identity.declaration_name, &slots, args) {
            Ok(planned) => planned,
            Err(reason) => return Some(self.unsupported_operand(reason, scope, hir_span(span), out)),
        };
        let expressions = defaults
            .iter()
            .enumerate()
            .map(|(slot, (_, default))| {
                (
                    planned
                        .iter()
                        .find(|(index, _)| *index == slot)
                        .map(|(_, value)| *value),
                    *default,
                )
            })
            .collect::<Vec<_>>();
        let output = self.new_temp(self.method_call_result_type(span), scope, hir_span(span));
        self.lower_codec_guards(
            receiver,
            &identity,
            operation,
            &expressions,
            Vec::new(),
            output,
            span,
            scope,
            out,
        );
        Some(self.temp_operand(output, &self.method_call_result_type(span)))
    }

    /// Evaluate one label and retain both successors before evaluating the next expression. Failure uses an empty
    /// receiver with the same builtin codec contract, so the runtime owns error messages and no source receiver runs.
    #[allow(clippy::too_many_arguments)]
    fn lower_codec_guards(
        &mut self,
        receiver: &ast::Spanned<ast::Expr>,
        identity: &CanonicalSymbolId,
        operation: BuiltinMethodId,
        expressions: &[(Option<&ast::Spanned<ast::Expr>>, &str)],
        mut supplied: Vec<bir::Operand>,
        output: bir::LocalId,
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) {
        let slot = supplied.len();
        let Some((expression, default)) = expressions.get(slot) else {
            let place = self.lower_expr_to_place(receiver, scope, out);
            let receiver = bir::Operand::place(place, bir::OwnershipFact::Borrow, false);
            self.emit_codec_call(
                identity.clone(),
                operation,
                receiver,
                supplied,
                output,
                span,
                scope,
                out,
            );
            return;
        };
        let operand = match expression {
            Some(expression) => self.lower_expr_to_operand(expression, scope, out),
            None => bir::Operand::Constant(bir::Constant::Str((*default).to_owned())),
        };
        let operand = match operand {
            bir::Operand::Place(mut read) => {
                read.fact = bir::OwnershipFact::Borrow;
                read.last_use = false;
                bir::Operand::Place(read)
            }
            constant => constant,
        };
        supplied.push(operand.clone());
        if expression.is_none() || matches!(expression.map(|value| &value.node), Some(ast::Expr::Literal(_))) {
            self.lower_codec_guards(
                receiver,
                identity,
                operation,
                expressions,
                supplied,
                output,
                span,
                scope,
                out,
            );
            return;
        }
        let helper = if slot == 0 {
            bir::HelperOp::TextCodecEncodingValid
        } else {
            bir::HelperOp::TextCodecErrorsValid
        };
        let cond = self.push_call_temp(
            bir::Callee::Helper(helper),
            vec![bir::ArgumentElement::One(operand)],
            IncanType::Primitive(IncanPrimitiveType::Bool),
            scope,
            hir_span(span),
            false,
            out,
        );
        let mut accepted = Vec::new();
        let mut rejected = Vec::new();
        let mut failure_arguments = supplied.clone();
        for (_, default) in expressions.iter().skip(failure_arguments.len()) {
            failure_arguments.push(bir::Operand::Constant(bir::Constant::Str((*default).to_owned())));
        }
        let (failure_operation, empty) = match operation {
            BuiltinMethodId::Str(StringMethodId::Encode) | BuiltinMethodId::FrozenStr(StringMethodId::Encode) => (
                BuiltinMethodId::Str(StringMethodId::Encode),
                bir::Constant::Str(String::new()),
            ),
            BuiltinMethodId::Bytes(BytesMethodId::Decode)
            | BuiltinMethodId::FrozenBytes(FrozenBytesMethodId::Decode) => (
                BuiltinMethodId::Bytes(BytesMethodId::Decode),
                bir::Constant::Bytes(Vec::new()),
            ),
            _ => return,
        };
        self.emit_codec_call(
            crate::symbols::canonical_builtin_member_identity(failure_operation.owner(), failure_operation.member()),
            failure_operation,
            bir::Operand::Constant(empty),
            failure_arguments,
            output,
            span,
            scope,
            &mut rejected,
        );
        self.lower_codec_guards(
            receiver,
            identity,
            operation,
            expressions,
            supplied,
            output,
            span,
            scope,
            &mut accepted,
        );
        out.push(bir::Statement {
            kind: bir::StatementKind::If {
                cond,
                then_block: bir::Block { scope, stmts: accepted },
                else_block: Some(bir::Block { scope, stmts: rejected }),
            },
            span: hir_span(span),
        });
    }

    /// Emit the authenticated codec with evaluated labels into a shared branch result. Label reads borrow their
    /// saved storage across guard and codec calls; lowering must not reevaluate or reorder their source expressions.
    #[allow(clippy::too_many_arguments)]
    fn emit_codec_call(
        &mut self,
        identity: CanonicalSymbolId,
        operation: BuiltinMethodId,
        receiver: bir::Operand,
        supplied: Vec<bir::Operand>,
        output: bir::LocalId,
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) {
        let count = supplied.len();
        let operands = std::iter::once(receiver)
            .chain(supplied)
            .map(bir::ArgumentElement::One)
            .collect();
        let value = self.push_call_temp(
            bir::Callee::Method(bir::MethodTarget {
                native_trait_delegation: None,
                name: identity.declaration_name.clone(),
                intrinsic_carrier: None,
                operation: Some(bir::ReflectionOperation::Builtin(operation)),
                loaf_trait: None,
                trait_arguments: None,
                canonical: Some(identity),
                type_args: Vec::new(),
                binding: bir::ArgumentBinding::resolved_positional(count),
            }),
            operands,
            self.method_call_result_type(span),
            scope,
            hir_span(span),
            false,
            out,
        );
        out.push(bir::Statement {
            kind: bir::StatementKind::Assign {
                place: bir::Place::from_local(output),
                rvalue: bir::Rvalue::Use(value),
            },
            span: hir_span(span),
        });
    }

    /// Reuse the ordinary checked target first, then the independent frozen-codec selection legacy does not read.
    fn selected_codec_identity(&self, span: ast::Span) -> Option<&CanonicalSymbolId> {
        self.type_info
            .resolved_identity(span)
            .or_else(|| self.type_info.calls.body_ir_native_codec_members.get(&span))
    }
}
