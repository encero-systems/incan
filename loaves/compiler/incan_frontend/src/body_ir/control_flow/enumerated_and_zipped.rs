//! Lowering of `enumerate` and `zip` iteration sources.

use super::*;

impl<'type_info, 'source> BodyBuilder<'type_info, 'source> {
    /// Select a canonical direct enumeration of a source list or an iterable with checked native hooks; a stored
    /// enumeration remains its own value.
    pub(super) fn enumerated_loop_source<'expr>(
        &self,
        expr: &'expr ast::Spanned<ast::Expr>,
    ) -> Option<&'expr ast::Spanned<ast::Expr>> {
        if let ast::Expr::Paren(inner) = &expr.node {
            return self.enumerated_loop_source(inner);
        }
        let ast::Expr::Call(callee, type_args, args) = &expr.node else {
            return None;
        };
        if self.type_info.resolved_builtin_call(expr.span) != Some(bir::BuiltinFnId::Enumerate) || !type_args.is_empty()
        {
            return None;
        }
        let ast::Expr::Ident(name) = &callee.node else {
            return None;
        };
        if self
            .declared_slots_for_direct_call(name, callee.span, expr.span)
            .ok()?
            .builtin
            != Some(bir::BuiltinFnId::Enumerate)
        {
            return None;
        }
        let [ast::CallArg::Positional(source)] = args.as_slice() else {
            return None;
        };
        if !self.type_info.rust.iteration_hooks.contains_key(&source.span) {
            self.resolve_ty(source.span)
                .collection_arguments(CollectionTypeId::List)?;
        }
        Some(source)
    }

    /// Normalize destructured enumeration only; a whole pair needs its own mixed-reference layout contract.
    pub(super) fn enumerated_pattern_is_normalizable(&self, item: bir::LocalId) -> bool {
        let Some(local) = self.locals.get(item.index()) else {
            return false;
        };
        let elements = tuple_element_types(&local.ty, 2);
        local.origin == bir::LocalOrigin::Temporary
            && elements[0] == IncanType::Primitive(IncanPrimitiveType::Int)
            && !matches!(elements[1], IncanType::Unknown)
    }

    /// Materialize a native enumeration source through its checked hooks; source lists retain their shared initializer.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn initialize_enumeration_source(
        &mut self,
        source: &ast::Spanned<ast::Expr>,
        native: bool,
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) -> Result<(bir::LocalId, usize), String> {
        if native {
            return self
                .initialize_general_iterator(source, source.span, true, scope, span, out)
                .map(|(iterator, initializer, _taking)| (iterator, initializer));
        }
        let source_ty = self.resolve_ty(source.span);
        let source_place = self.lower_expr_to_place(source, scope, out);
        let iterator = self.new_temp(source_ty, scope, span);
        let initializer = out.len();
        out.push(bir::Statement {
            kind: bir::StatementKind::Assign {
                place: bir::Place::from_local(iterator),
                rvalue: bir::Rvalue::Use(bir::Operand::place(source_place, bir::OwnershipFact::Borrow, false)),
            },
            span,
        });
        Ok((iterator, initializer))
    }

    /// Poll a direct enumeration's original list or checked native iterable and retain its index as explicit scalar
    /// state. Pattern components bind the produced element itself when changed, rather than a materialized tuple's
    /// snapshot of it.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn lower_enumerated_iteration(
        &mut self,
        source: &ast::Spanned<ast::Expr>,
        pattern_local: bir::LocalId,
        outer_scope: bir::ScopeId,
        loop_scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
        body_fn: impl FnOnce(&mut Self, bir::ScopeId, &mut Vec<bir::Statement>),
    ) {
        // ---- Source storage and enumeration state ----
        let native = self.type_info.rust.iteration_hooks.contains_key(&source.span);
        let (iterator, initializer) = match self.initialize_enumeration_source(source, native, outer_scope, span, out) {
            Ok(initialized) => initialized,
            Err(reason) => {
                self.push_unsupported_stmt(reason, span, out);
                return;
            }
        };
        let index_ty = IncanType::Primitive(IncanPrimitiveType::Int);
        let index = self.new_temp(index_ty.clone(), outer_scope, span);
        out.push(bir::Statement {
            kind: bir::StatementKind::Assign {
                place: bir::Place::from_local(index),
                rvalue: bir::Rvalue::Use(bir::Operand::Constant(bir::Constant::Int(0))),
            },
            span,
        });
        let pattern_span = self.locals[pattern_local.index()].span;
        let element_ty = tuple_element_types(&self.locals[pattern_local.index()].ty, 2)[1].clone();
        let element = self.new_temp(element_ty, loop_scope, pattern_span);
        let current_index = self.new_temp(index_ty, loop_scope, pattern_span);

        // ---- One produced element and its pre-advance index ----
        self.loop_break_targets.push(None);
        self.loop_scopes.push(loop_scope);
        self.loop_continue_actions.push(Vec::new());
        let mut stmts = Vec::new();
        if native {
            if let Err(reason) =
                self.lower_checked_iteration_poll(source.span, iterator, element, false, loop_scope, span, &mut stmts)
            {
                self.push_unsupported_stmt(reason, span, &mut stmts);
            }
        } else {
            stmts.push(bir::Statement {
                kind: bir::StatementKind::IterNext {
                    destination: bir::Place::from_local(element),
                    iterator: bir::Operand::place(
                        bir::Place::from_local(iterator),
                        bir::OwnershipFact::MutBorrow,
                        false,
                    ),
                    protocol: bir::IterProtocol::Builtin,
                },
                span,
            });
        }
        stmts.extend([
            bir::Statement {
                kind: bir::StatementKind::Assign {
                    place: bir::Place::from_local(current_index),
                    rvalue: bir::Rvalue::Use(index_read(index)),
                },
                span,
            },
            bir::Statement {
                kind: bir::StatementKind::Assign {
                    place: bir::Place::from_local(index),
                    rvalue: bir::Rvalue::BinaryOp(
                        bir::BinOp::Add,
                        index_read(index),
                        bir::Operand::Constant(bir::Constant::Int(1)),
                    ),
                },
                span,
            },
        ]);

        // ---- Checked pattern bindings and source ownership ----
        body_fn(self, loop_scope, &mut stmts);
        self.remap_enumerated_bindings(&mut stmts, pattern_local, current_index, element);
        self.retain_in_place_iteration_bindings(&mut stmts, element);
        if !native && self.body_changes_item_in_place(&stmts, element) {
            borrow_iterable_exclusively(&mut out[initializer]);
        }
        self.loop_break_targets.pop();
        self.loop_scopes.pop();
        self.loop_continue_actions.pop();
        out.push(bir::Statement {
            kind: bir::StatementKind::Loop {
                body: bir::Block {
                    scope: loop_scope,
                    stmts,
                },
            },
            span,
        });
    }

    /// Replace the destructured tuple's checked component reads with the normalized index and element places.
    pub(super) fn remap_enumerated_bindings(
        &self,
        stmts: &mut [bir::Statement],
        pattern: bir::LocalId,
        index: bir::LocalId,
        element: bir::LocalId,
    ) {
        for statement in stmts {
            let bir::StatementKind::Assign {
                place: destination,
                rvalue: bir::Rvalue::Use(bir::Operand::Place(read)),
            } = &mut statement.kind
            else {
                continue;
            };
            if read.place.local_id() != Some(pattern) {
                continue;
            }
            let Some(bir::PlaceElem::Field { name, .. }) = read.place.projection.first() else {
                continue;
            };
            let replacement = match name.as_str() {
                "0" => index,
                "1" => element,
                _ => continue,
            };
            let mut place = bir::Place::from_local(replacement);
            place.projection.extend(read.place.projection.iter().skip(1).cloned());
            read.place = place;
            if read.place.projection.is_empty()
                && destination.local_id().is_some_and(|local| {
                    !self.locals[local.index()]
                        .ty
                        .abi_v0_facts()
                        .ownership
                        .is_trivially_copy()
                })
            {
                read.fact = bir::OwnershipFact::Move;
                read.last_use = true;
            }
        }
    }

    /// Require a destructured checked pair before replacing its component reads with independent source items.
    pub(super) fn pair_pattern_is_normalizable(&self, item: bir::LocalId) -> bool {
        let local = &self.locals[item.index()];
        local.origin == bir::LocalOrigin::Temporary
            && tuple_element_types(&local.ty, 2)
                .iter()
                .all(|ty| !matches!(ty, IncanType::Unknown))
    }

    /// Select canonical direct zip over lists or builtin ranges; a same-spelled source call keeps its ordinary path.
    pub(super) fn zipped_loop_sources<'expr>(
        &self,
        expr: &'expr ast::Spanned<ast::Expr>,
    ) -> Option<[&'expr ast::Spanned<ast::Expr>; 2]> {
        if let ast::Expr::Paren(inner) = &expr.node {
            return self.zipped_loop_sources(inner);
        }
        let ast::Expr::Call(callee, type_args, args) = &expr.node else {
            return None;
        };
        if self.type_info.resolved_builtin_call(expr.span) != Some(bir::BuiltinFnId::Zip) || !type_args.is_empty() {
            return None;
        }
        let ast::Expr::Ident(name) = &callee.node else {
            return None;
        };
        if self
            .declared_slots_for_direct_call(name, callee.span, expr.span)
            .ok()?
            .builtin
            != Some(bir::BuiltinFnId::Zip)
        {
            return None;
        }
        let [ast::CallArg::Positional(left), ast::CallArg::Positional(right)] = args.as_slice() else {
            return None;
        };
        // Frozen legacy's two-range zip requires list storage it does not have; keep its existing named refusal.
        if self.checked_range_source(left) && self.checked_range_source(right) {
            return None;
        }
        for source in [left, right] {
            if self.checked_range_source(source) {
                continue;
            }
            self.resolve_ty(source.span)
                .collection_arguments(CollectionTypeId::List)?;
        }
        Some([left, right])
    }

    /// Recognize a checked builtin range through parentheses before choosing scalar zip state instead of list storage.
    pub(super) fn checked_range_source(&self, source: &ast::Spanned<ast::Expr>) -> bool {
        if let ast::Expr::Paren(inner) = &source.node {
            return self.checked_range_source(inner);
        }
        self.type_info.resolved_builtin_call(source.span) == Some(bir::BuiltinFnId::Range)
    }

    /// Evaluate a zip operand in source order, retaining builtin range validation before the first poll.
    pub(super) fn initialize_zip_source(
        &mut self,
        source: &ast::Spanned<ast::Expr>,
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) -> Result<ZipLoopSource, String> {
        if self.checked_range_source(source) {
            let Some(RangeLoopSource::Builtin(bounds)) = self.builtin_range_loop_source(source, scope, out)? else {
                return Err("unsupported checked zip range source".into());
            };
            let index = self.snapshot_local(
                bounds.start.clone(),
                IncanType::Primitive(IncanPrimitiveType::Int),
                scope,
                span,
                out,
            );
            return Ok(ZipLoopSource::Range { bounds, index });
        }
        let (iterator, initializer) = self.initialize_iteration_source(source, scope, span, out);
        Ok(ZipLoopSource::List { iterator, initializer })
    }

    /// Produce one zip component, stopping before the source body; scalar advancement belongs to body exit edges.
    pub(super) fn poll_zip_source(
        &mut self,
        source: &ZipLoopSource,
        item: bir::LocalId,
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) {
        match source {
            ZipLoopSource::List { iterator, .. } => out.push(self.iteration_poll(*iterator, item, span)),
            ZipLoopSource::Range { bounds, index } => {
                let cond = self.builtin_range_stop_condition(&bounds.end, &bounds.step, *index, scope, span, out);
                let exit_scope = self.new_scope(Some(scope), span);
                out.push(bir::Statement {
                    kind: bir::StatementKind::If {
                        cond,
                        then_block: bir::Block {
                            scope: exit_scope,
                            stmts: vec![bir::Statement {
                                kind: bir::StatementKind::Break { value: None },
                                span,
                            }],
                        },
                        else_block: None,
                    },
                    span,
                });
                out.push(bir::Statement {
                    kind: bir::StatementKind::Assign {
                        place: bir::Place::from_local(item),
                        rvalue: bir::Rvalue::Use(index_read(*index)),
                    },
                    span,
                });
            }
        }
    }

    /// Retain scalar zip advances for fallthrough and continue, after body effects as in an ordinary range loop.
    pub(super) fn zip_range_advances(
        &mut self,
        sources: [&ZipLoopSource; 2],
        scope: bir::ScopeId,
        span: HirSourceSpan,
    ) -> Vec<bir::Statement> {
        let mut out = Vec::new();
        for source in sources {
            let ZipLoopSource::Range { bounds, index } = source else {
                continue;
            };
            let next = self.push_assign_temp(
                bir::Rvalue::BinaryOp(bir::BinOp::Add, index_read(*index), bounds.step.clone()),
                IncanType::Primitive(IncanPrimitiveType::Int),
                scope,
                span,
                &mut out,
            );
            out.push(bir::Statement {
                kind: bir::StatementKind::Assign {
                    place: bir::Place::from_local(*index),
                    rvalue: bir::Rvalue::Use(next),
                },
                span,
            });
        }
        out
    }

    /// Evaluate one original iterable once and retain the initializer whose checked borrow mode the body settles.
    pub(super) fn initialize_iteration_source(
        &mut self,
        source: &ast::Spanned<ast::Expr>,
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) -> (bir::LocalId, usize) {
        let ty = self.resolve_ty(source.span);
        let place = self.lower_expr_to_place(source, scope, out);
        let iterator = self.new_temp(ty, scope, span);
        let initializer = out.len();
        out.push(bir::Statement {
            kind: bir::StatementKind::Assign {
                place: bir::Place::from_local(iterator),
                rvalue: bir::Rvalue::Use(bir::Operand::place(place, bir::OwnershipFact::Borrow, false)),
            },
            span,
        });
        (iterator, initializer)
    }

    /// Poll a retained builtin source, with exhaustion leaving the normalized loop before any body effect.
    pub(super) fn iteration_poll(
        &self,
        iterator: bir::LocalId,
        item: bir::LocalId,
        span: HirSourceSpan,
    ) -> bir::Statement {
        bir::Statement {
            kind: bir::StatementKind::IterNext {
                destination: bir::Place::from_local(item),
                iterator: bir::Operand::place(bir::Place::from_local(iterator), bir::OwnershipFact::MutBorrow, false),
                protocol: bir::IterProtocol::Builtin,
            },
            span,
        }
    }

    /// Poll each direct zip operand independently and borrow only the source whose non-Copy component changes.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn lower_zipped_iteration(
        &mut self,
        sources: [&ast::Spanned<ast::Expr>; 2],
        pattern: bir::LocalId,
        outer_scope: bir::ScopeId,
        loop_scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
        body_fn: impl FnOnce(&mut Self, bir::ScopeId, &mut Vec<bir::Statement>),
    ) -> Result<(), String> {
        // ---- Original source storage and independent polls ----
        let left = self.initialize_zip_source(sources[0], outer_scope, span, out)?;
        let right = self.initialize_zip_source(sources[1], outer_scope, span, out)?;
        let types = tuple_element_types(&self.locals[pattern.index()].ty, 2);
        let pattern_span = self.locals[pattern.index()].span;
        let first = self.new_temp(types[0].clone(), loop_scope, pattern_span);
        let second = self.new_temp(types[1].clone(), loop_scope, pattern_span);
        let mut stmts = Vec::new();
        self.poll_zip_source(&left, first, loop_scope, span, &mut stmts);
        self.poll_zip_source(&right, second, loop_scope, span, &mut stmts);
        self.loop_break_targets.push(None);
        self.loop_scopes.push(loop_scope);
        let advances = self.zip_range_advances([&left, &right], loop_scope, span);
        self.loop_continue_actions.push(advances);

        // ---- Checked component bindings and operand ownership ----
        body_fn(self, loop_scope, &mut stmts);
        self.remap_enumerated_bindings(&mut stmts, pattern, first, second);
        for (item, source) in [(first, left), (second, right)] {
            self.retain_in_place_iteration_bindings(&mut stmts, item);
            if let ZipLoopSource::List { initializer, .. } = source
                && self.body_changes_item_in_place(&stmts, item)
            {
                borrow_iterable_exclusively(&mut out[initializer]);
            }
        }
        if let Some(advances) = self.loop_continue_actions.last() {
            stmts.extend(advances.iter().cloned());
        }
        self.loop_break_targets.pop();
        self.loop_scopes.pop();
        self.loop_continue_actions.pop();
        out.push(bir::Statement {
            kind: bir::StatementKind::Loop {
                body: bir::Block {
                    scope: loop_scope,
                    stmts,
                },
            },
            span,
        });
        Ok(())
    }
}
