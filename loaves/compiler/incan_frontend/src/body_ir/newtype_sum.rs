//! Direct-only numeric newtype summation selected by the checked iterator protocol.

use super::args::fixed_elements;
use super::{BodyBuilder, CanonicalSymbolId, HirSourceSpan, IncanPrimitiveType, IncanType, ast, bir, parameter_types};

impl BodyBuilder<'_, '_> {
    /// Retain the checked item projection and the single post-exhaustion reconstruction before native planning.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn lower_newtype_sum(
        &mut self,
        receiver: &ast::Spanned<ast::Expr>,
        name: &str,
        type_arguments: &[ast::Spanned<ast::Type>],
        arguments: &[ast::CallArg],
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> Option<bir::Operand> {
        // ---- Checked terminal and canonical owner ----
        if name != "sum" || !arguments.is_empty() || !type_arguments.is_empty() {
            return None;
        }
        let IncanType::Generic { base, args } = self.resolve_ty(receiver.span) else {
            return None;
        };
        if base != "Iterator" || args.len() != 1 {
            return None;
        }
        let IncanType::Named(owner) = &args[0] else {
            return None;
        };
        let source_owner = self.newtype_sum_source_owner(owner)?;
        let construction = &self.type_info.declarations.newtype_construction;
        let facts = construction.get(source_owner)?.clone();
        let source_span = super::hir_span(span);
        let target = args[0].clone();
        let identity = if self.retain_nominal_carriers {
            parameter_types::retained_nominal_identity(owner, self.type_info)
        } else {
            self.type_info.declarations.named_type_identities.get(owner)
        };
        let Some(identity) = identity else {
            return Some(self.unsupported_typed_operand(
                "numeric-newtype sum without canonical owner binding".into(),
                target,
                scope,
                source_span,
                out,
            ));
        };
        let Some(declaration) = self
            .local_nominal_declarations
            .values()
            .find(|declaration| &declaration.canonical == identity)
            .cloned()
        else {
            return Some(self.unsupported_typed_operand(
                "numeric-newtype sum without retained canonical layout".into(),
                target,
                scope,
                source_span,
                out,
            ));
        };
        if identity.kind != super::SemanticSourceTargetKind::Newtype {
            return None;
        }

        // ---- Complete local layout and hook selection ----
        let accepted = self.type_info.body_ir_iterator_sum_items.get(&span);
        let hook_matches = sum_hook_matches(
            identity,
            facts.checked_constructor.as_deref(),
            facts.checked_constructor_identity.as_ref(),
        );
        let source_iterator = self
            .type_info
            .declarations
            .named_type_identities
            .get(&base)
            .is_some_and(|identity| {
                matches!(
                    identity.kind,
                    super::SemanticSourceTargetKind::Class
                        | super::SemanticSourceTargetKind::Model
                        | super::SemanticSourceTargetKind::Newtype
                )
            });
        if source_iterator
            || !hook_matches
            || accepted.map(|item| self.checked_type(item)) != Some(target.clone())
            || self.method_call_result_type(span) != target
            || facts.underlying != crate::symbols::ResolvedType::Int
            || !facts.type_params.is_empty()
            || declaration.fields != ["0"]
            || declaration.field_types != [IncanType::Primitive(IncanPrimitiveType::Int)]
            || declaration.field_identities != [identity.clone()]
            || incan_semantics_core::canonical_module_identity(identity).as_deref() != Some(self.module_identity)
        {
            return Some(self.unsupported_typed_operand(
                "numeric-newtype sum without complete checked local selection".into(),
                target,
                scope,
                source_span,
                out,
            ));
        }

        // ---- Owned consumption and final reconstruction ----
        let (total, iterator) = self.newtype_sum_loop(receiver, &declaration, &target, scope, source_span, out);
        let result = self.finish_newtype_sum(
            owner,
            &declaration,
            facts.checked_constructor_identity,
            facts.checked_constructor.is_some(),
            total,
            target,
            scope,
            source_span,
            out,
        );
        out.push(bir::Statement {
            kind: bir::StatementKind::Drop { local: iterator },
            span: source_span,
        });
        Some(result)
    }

    /// Select the checked source binding for construction metadata without interpreting a projected carrier key.
    /// Project mode first authenticates the canonical owner and retained declaration, then requires the declaration's
    /// checked source binding to name that same owner. Local mode keeps its original metadata lookup. Missing or
    /// conflicting bindings cannot authorize a bridge from a matching field layout alone.
    fn newtype_sum_source_owner<'owner>(&'owner self, owner: &'owner str) -> Option<&'owner str> {
        let source_owner = if self.retain_nominal_carriers {
            match parameter_types::retained_nominal_identity(owner, self.type_info) {
                Some(identity) => {
                    let declaration = self
                        .local_nominal_declarations
                        .values()
                        .find(|declaration| &declaration.canonical == identity)?;
                    if self.type_info.declarations.named_type_identities.get(&declaration.name) != Some(identity) {
                        return None;
                    }
                    declaration.name.as_str()
                }
                None => owner,
            }
        } else {
            owner
        };
        Some(source_owner)
    }

    /// Reconstruct once after exhaustion, using the selected hook or legacy's raw constrained/plain tuple form.
    #[allow(clippy::too_many_arguments)]
    fn finish_newtype_sum(
        &mut self,
        owner: &str,
        declaration: &bir::NominalDeclaration,
        hook: Option<CanonicalSymbolId>,
        checked_constructor: bool,
        total: bir::Operand,
        target: IncanType,
        scope: bir::ScopeId,
        source_span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        if let Some(hook) = hook {
            let failure = format!(
                "validated newtype construction failed: {}::{}",
                declaration.name, hook.declaration_name
            );
            let checked =
                self.checked_newtype_hook_call(&declaration.name, hook, total, &target, scope, source_span, out);
            return self.push_call_temp(
                bir::Callee::Helper(bir::HelperOp::NewtypeSumExtract),
                fixed_elements(vec![checked, bir::Operand::Constant(bir::Constant::Str(failure))]),
                target,
                scope,
                source_span,
                true,
                out,
            );
        }
        if checked_constructor {
            return self.unsupported_typed_operand(
                "numeric-newtype sum without checked hook identity".into(),
                target,
                scope,
                source_span,
                out,
            );
        }
        self.push_assign_temp(
            bir::Rvalue::Aggregate(
                bir::AggregateKind::Constructor(Box::new(bir::ConstructorTarget {
                    name: owner.to_owned(),
                    canonical: Some(declaration.canonical.clone()),
                    direct_declaration_id: Some(declaration.direct_declaration_id.clone()),
                    canonical_field_layout: Some(declaration.fields.clone()),
                    binding: bir::ArgumentBinding::resolved_positional(1),
                })),
                fixed_elements(vec![total]),
            ),
            target,
            scope,
            source_span,
            out,
        )
    }

    /// Poll an owned builtin iterator in order, projecting only the canonical payload and retaining explicit cleanup.
    fn newtype_sum_loop(
        &mut self,
        receiver: &ast::Spanned<ast::Expr>,
        declaration: &bir::NominalDeclaration,
        target: &IncanType,
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) -> (bir::Operand, bir::LocalId) {
        // ---- Owned iterator and primitive accumulator ----
        let source = self.lower_expr_to_operand(receiver, scope, out);
        let iterator = self.new_temp(self.resolve_ty(receiver.span), scope, span);
        out.push(bir::Statement {
            kind: bir::StatementKind::Assign {
                place: bir::Place::from_local(iterator),
                rvalue: bir::Rvalue::Use(source),
            },
            span,
        });
        let integer = IncanType::Primitive(IncanPrimitiveType::Int);
        let accumulator = self.new_temp(integer.clone(), scope, span);
        out.push(bir::Statement {
            kind: bir::StatementKind::Assign {
                place: bir::Place::from_local(accumulator),
                rvalue: bir::Rvalue::Use(bir::Operand::Constant(bir::Constant::Int(0))),
            },
            span,
        });

        // ---- Canonical payload polling ----
        let loop_scope = self.new_scope(Some(scope), span);
        let item = self.new_temp(target.clone(), loop_scope, span);
        let mut place = bir::Place::from_local(item);
        place.projection.push(bir::PlaceElem::Field {
            name: "0".into(),
            canonical: Some(declaration.canonical.clone()),
            structural: false,
        });
        let statements = vec![
            bir::Statement {
                kind: bir::StatementKind::IterNext {
                    destination: bir::Place::from_local(item),
                    iterator: bir::Operand::place(
                        bir::Place::from_local(iterator),
                        bir::OwnershipFact::MutBorrow,
                        false,
                    ),
                    protocol: bir::IterProtocol::Builtin,
                },
                span,
            },
            bir::Statement {
                kind: bir::StatementKind::Call {
                    destination: Some(bir::Place::from_local(accumulator)),
                    callee: bir::Callee::Helper(bir::HelperOp::NewtypeSumAdd),
                    args: fixed_elements(vec![
                        bir::Operand::place(bir::Place::from_local(accumulator), bir::OwnershipFact::Copy, false),
                        bir::Operand::place(place, bir::OwnershipFact::Copy, false),
                    ]),
                    may_panic: false,
                },
                span,
            },
        ];

        // ---- Retained consumer loop ----
        out.push(bir::Statement {
            kind: bir::StatementKind::Loop {
                body: bir::Block {
                    scope: loop_scope,
                    stmts: statements,
                },
            },
            span,
        });
        (self.temp_operand(accumulator, &integer), iterator)
    }
}

/// Require the selected hook's complete local owner provenance; a partially retained hook cannot authorize
/// reconstruction.
fn sum_hook_matches(owner: &CanonicalSymbolId, name: Option<&str>, hook: Option<&CanonicalSymbolId>) -> bool {
    match (name, hook) {
        (None, None) => true,
        (Some(name), Some(hook)) => {
            hook.declaration_name == name
                && hook.kind == super::SemanticSourceTargetKind::Method
                && hook.origin == owner.origin
                && owner.declaration_span.start <= hook.declaration_span.start
                && hook.declaration_span.end <= owner.declaration_span.end
        }
        _ => false,
    }
}
