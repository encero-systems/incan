//! Normalize checked structural hooks into ordinary calls and intrinsic option/result control flow.

use super::args::DeclaredSlot;
use super::primitives::{collection_member_types, result_error_type};
use super::{BodyBuilder, HirSourceSpan, IncanPrimitiveType, IncanType, SemanticSourceTargetKind, ast, bir};
use crate::symbols::canonical_builtin_constructor_identity;
use crate::typechecker::BodyIrProtocolCallInfo;
use incan_lang::lang::surface::constructors::ConstructorId;
use incan_lang::lang::types::collections::CollectionTypeId;

impl BodyBuilder<'_, '_> {
    /// Materialize an iterator once and retain the initializer's statement index after evaluating the source.
    /// Builtin in-place iteration can change that initializer's borrow without touching an earlier source call.
    /// Checked item-taking sources and whole temporary lists move into their iterator; custom hooks retain their
    /// declared receiver mode and report no builtin consumption.
    pub(super) fn initialize_general_iterator(
        &mut self,
        expression: &ast::Spanned<ast::Expr>,
        protocol_span: ast::Span,
        custom: bool,
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) -> Result<(bir::LocalId, usize, bool), String> {
        let iterable = self.lower_expr_to_place(expression, scope, out);
        let initializer = out.len();
        if custom {
            let hooks = self
                .type_info
                .protocols
                .body_ir_iterations
                .get(&(protocol_span.start, protocol_span.end))
                .ok_or("iteration without retained checked hooks")?
                .clone();
            let result = self.checked_protocol_call(&hooks.iter, iterable, scope, span, out)?;
            return match result {
                bir::Operand::Place(read) => read
                    .place
                    .local_id()
                    .map(|local| (local, initializer, false))
                    .ok_or_else(|| "iteration initializer without a local result".into()),
                _ => Err("iteration initializer without a place result".into()),
            };
        }
        let iterator_type = self.retained_expression_type(expression);
        let taking = self.type_info.for_loop_takes_items(protocol_span)
            || self.temporary_list_iterable(&iterable, &iterator_type);
        let iterator = self.new_temp(iterator_type, scope, span);
        out.push(bir::Statement {
            kind: bir::StatementKind::Assign {
                place: bir::Place::from_local(iterator),
                rvalue: bir::Rvalue::Use(bir::Operand::place(
                    iterable,
                    if taking {
                        bir::OwnershipFact::Move
                    } else {
                        bir::OwnershipFact::Borrow
                    },
                    taking,
                )),
            },
            span,
        });
        Ok((iterator, initializer, taking))
    }

    /// Emit one retained hook through the ordinary checked argument planner and canonical call contract.
    fn checked_protocol_call(
        &mut self,
        hook: &BodyIrProtocolCallInfo,
        receiver: bir::Place,
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) -> Result<bir::Operand, String> {
        // ---- Checked method and argument frame ----
        let canonical = hook
            .canonical
            .as_ref()
            .filter(|identity| identity.kind == SemanticSourceTargetKind::Method)
            .ok_or("iteration hook without a canonical method identity")?;
        let mutable = hook
            .receiver_is_mutable
            .ok_or("iteration hook without a checked instance receiver")?;
        let params = hook
            .params
            .as_ref()
            .ok_or("iteration hook without a checked signature")?;
        let declared = params.iter().map(DeclaredSlot::from_checked_param).collect();
        let (mut arguments, binding) = self.bind_declared_args("iteration hook", Some(declared), &[], scope, out)?;

        // ---- Declared receiver mode ----
        let fact = if mutable {
            bir::OwnershipFact::MutBorrow
        } else {
            bir::OwnershipFact::Borrow
        };
        arguments.insert(0, bir::ArgumentElement::One(bir::Operand::place(receiver, fact, false)));
        let returned = self.checked_type(&hook.return_type);

        // ---- Canonical call ----
        let target = bir::MethodTarget {
            native_trait_delegation: None,
            name: canonical.declaration_name.clone(),
            canonical: Some(canonical.clone()),
            intrinsic_carrier: None,
            reflection: None,
            sdk_trait: super::library_calls::trait_dispatch(
                canonical,
                hook.dispatch.as_ref(),
                None,
                &self.type_parameters,
                self.type_info,
            ),
            type_args: hook.type_args.iter().map(|ty| self.checked_type(ty)).collect(),
            binding,
        };
        Ok(self.push_call_temp(
            bir::Callee::Method(target),
            arguments,
            returned,
            scope,
            span,
            false,
            out,
        ))
    }

    /// Poll inside the loop, propagating a checked failure before moving the intrinsic Option item or breaking.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn lower_checked_iteration_poll(
        &mut self,
        protocol_span: ast::Span,
        iterator: bir::LocalId,
        item: bir::LocalId,
        fallible: bool,
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) -> Result<(), String> {
        // ---- Checked result carrier ----
        let hook = self
            .type_info
            .protocols
            .body_ir_iterations
            .get(&(protocol_span.start, protocol_span.end))
            .ok_or("iteration poll without retained checked hooks")?
            .next
            .clone();
        let returned = self.checked_type(&hook.return_type);
        let option_type = if fallible {
            match &returned {
                IncanType::Generic { base, args } if base == "Result" && args.len() == 2 => &args[0],
                _ => return Err("fallible iteration poll without a checked Result".into()),
            }
        } else {
            &returned
        };
        if !collection_member_types(option_type, CollectionTypeId::Option).is_some_and(|members| members.len() == 1) {
            return Err("iteration poll without a checked Option item".into());
        }

        // ---- Poll and preserve the error channel ----
        let polled = self.checked_protocol_call(&hook, bir::Place::from_local(iterator), scope, span, out)?;
        let option = if fallible {
            self.propagate_iteration_poll(polled, &returned, scope, span, out)?
        } else {
            polled
        };

        // ---- Own the produced item or exit on exhaustion ----
        let some = bir::Pattern::Enum {
            canonical: Some(canonical_builtin_constructor_identity(ConstructorId::Some)),
            name: "Option".into(),
            variant: "Some".into(),
            fields: vec![bir::Pattern::Var(bir::PatternBinding {
                local: item,
                fact: bir::OwnershipFact::Move,
                last_use: true,
            })],
        };
        let arms = vec![
            bir::MatchArm {
                pattern: some,
                guard_stmts: Vec::new(),
                guard: None,
                body_stmts: Vec::new(),
                result: bir::Operand::Constant(bir::Constant::Unit),
            },
            bir::MatchArm {
                pattern: bir::Pattern::Wildcard,
                guard_stmts: Vec::new(),
                guard: None,
                body_stmts: vec![bir::Statement {
                    kind: bir::StatementKind::Break { value: None },
                    span,
                }],
                result: bir::Operand::Constant(bir::Constant::Unit),
            },
        ];
        self.push_assign_temp(
            bir::Rvalue::Match {
                scrutinee: option,
                arms,
            },
            IncanType::Primitive(IncanPrimitiveType::Unit),
            scope,
            span,
            out,
        );
        Ok(())
    }

    /// Preserve the checked Result success/error channels; unsupported conversions stay explicit routing facts.
    fn propagate_iteration_poll(
        &mut self,
        operand: bir::Operand,
        returned: &IncanType,
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) -> Result<bir::Operand, String> {
        // ---- Exact checked channels ----
        let IncanType::Generic { base, args } = returned else {
            return Err("fallible poll without a checked Result".into());
        };
        if base != "Result" || args.len() != 2 {
            return Err("fallible poll without a checked Result".into());
        }
        let error_routing = match result_error_type(&self.owner_return_type) {
            Some(error) if error == &args[1] => bir::TryErrorRouting::SameType {
                error_type: error.clone(),
            },
            Some(error) => bir::TryErrorRouting::ConversionRequired {
                source_error_type: args[1].clone(),
                destination_error_type: error.clone(),
            },
            None => bir::TryErrorRouting::Unresolved,
        };

        // ---- Move the successful poll into its own local ----
        let success = self.new_temp(args[0].clone(), scope, span);
        out.push(bir::Statement {
            kind: bir::StatementKind::TryPropagate {
                destination: bir::Place::from_local(success),
                operand,
                error_routing,
            },
            span,
        });
        Ok(self.temp_operand(success, &args[0]))
    }
}
