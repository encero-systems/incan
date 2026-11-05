//! Places, globals, reads, terminal paths, ownership facts, scope drops, and temporaries.

use super::*;

impl<'type_info, 'source> BodyBuilder<'type_info, 'source> {
    /// Resolve one source reference from the canonical identity recorded by typechecking.
    ///
    /// A proven local identity must select a frame local with that same identity. Proven `const`/`static` references
    /// become canonical global places. Any other proven identity that has no Body IR value representation returns
    /// `None`, so the caller emits an explicit unsupported node instead of silently changing meaning through a
    /// spelling lookup. Checked union member reads carry an explicit payload projection while the local retains its
    /// storage type. Only a genuinely unproven reference may use the legacy `External` recovery local.
    /// A synthetic truthiness call shares this span with its receiver, whose separate Body IR identity selects the
    /// value here while the unchanged legacy reference still selects the method.
    /// Checked static decorator factories likewise retain their receiver's canonical identity and carrier separately
    /// from their selected method at a shared synthetic span.
    pub(super) fn place_for_name(&mut self, name: &str, span: ast::Span, ty: &IncanType) -> Option<bir::Place> {
        if let Some(receiver) = self
            .type_info
            .declarations
            .body_ir_decorator_static_receivers
            .get(&span)
            .filter(|receiver| {
                receiver.binding_name == name && receiver.identity.kind != SemanticSourceTargetKind::Module
            })
        {
            return self
                .global_place(receiver.identity.clone(), self.checked_type(&receiver.ty))
                .map(bir::Place::from_global);
        }
        if let Some(identity) = self
            .type_info
            .protocols
            .body_ir_truthiness_receivers
            .get(&span)
            .or_else(|| self.type_info.resolved_identity(span))
            .or_else(|| self.type_info.calls.body_ir_native_argument_identities.get(&span))
            .cloned()
        {
            if let Some(alias) = self.input_alias(&identity) {
                return Some(alias.place.clone());
            }
            if let Some(&id) = self.identity_bindings.get(&identity) {
                if let Some(global) = self.static_aliases.get(&id) {
                    return Some(bir::Place::from_global(global.clone()));
                }
                let mut place = bir::Place::from_local(id);
                // The local keeps its storage type; a read keeps the checker's narrower member as an explicit
                // projection.
                if let Some(local) = self.locals.get(id.index()) {
                    carrier_projection::retain_checked_narrowing(&mut place, &local.ty, ty);
                }
                return Some(place);
            }
            return self.global_place(identity, ty.clone()).map(bir::Place::from_global);
        }

        Some(bir::Place::from_local(
            self.external_local_for_name(name, hir_span(span)),
        ))
    }

    /// The storage a `module.NAME` member access reads, when its base is a module binding and the checker resolved the
    /// member to a `const` or `static` (`math.PI`, `helpers.DEFAULT_LABEL`).
    ///
    /// A module binding is a namespace, not a value, so the access is not a field of anything: it is the member's own
    /// canonical global place, the same one a bare imported name would read.
    pub(super) fn module_member_place(
        &self,
        base: &ast::Spanned<ast::Expr>,
        access_span: ast::Span,
    ) -> Option<bir::Place> {
        if !self.is_module_namespace_span(base.span) {
            return None;
        }
        let identity = self.type_info.resolved_identity(access_span)?.clone();
        self.global_place(identity, self.resolve_ty(access_span))
            .map(bir::Place::from_global)
    }

    /// Return whether one checked expression span denotes a module namespace, including a qualified decorator's
    /// synthetic receiver span whose ordinary identity slot belongs to the selected member.
    pub(super) fn is_module_namespace_span(&self, span: ast::Span) -> bool {
        self.type_info
            .resolved_identity(span)
            .is_some_and(|identity| identity.kind == SemanticSourceTargetKind::Module)
            || self
                .type_info
                .declarations
                .body_ir_decorator_static_receivers
                .get(&span)
                .is_some_and(|receiver| receiver.identity.kind == SemanticSourceTargetKind::Module)
            || (self
                .type_info
                .declarations
                .body_ir_decorator_applications
                .contains_key(&span)
                && self
                    .type_info
                    .resolved_identity(span)
                    .is_some_and(|identity| identity.kind == SemanticSourceTargetKind::Function))
    }

    /// Select a canonical module-storage root when `identity` denotes a `const` or `static`. Only a declaration in
    /// the checker's exact source-owned inventory grants rebinding permission, including a package compilation;
    /// imported storage keeps projection-only writes regardless of its spelling or package name.
    /// Project profiles retain an imported storage type from its exact owning context declaration; conflicting copies
    /// cannot authorize a global place.
    pub(super) fn global_place(&self, identity: CanonicalSymbolId, ty: IncanType) -> Option<bir::GlobalPlace> {
        let ty = if self.retain_nominal_carriers {
            match self.published_storage_types.get(&identity) {
                Some(Some(retained)) => retained.clone(),
                Some(None) => return None,
                None => ty,
            }
        } else {
            ty
        };
        let write_policy = match identity.kind {
            SemanticSourceTargetKind::Const => bir::GlobalWritePolicy::ReadOnly,
            SemanticSourceTargetKind::Static => {
                let declared_here = self
                    .type_info
                    .declarations
                    .declaration_identities
                    .values()
                    .any(|declared| declared == &identity);
                if declared_here {
                    bir::GlobalWritePolicy::Rebindable
                } else {
                    bir::GlobalWritePolicy::ProjectionOnly
                }
            }
            _ => return None,
        };
        Some(bir::GlobalPlace {
            identity,
            ty,
            write_policy,
        })
    }

    /// Allocate the explicit recovery local for a reference whose resolver supplied no identity.
    pub(super) fn external_local_for_name(&mut self, name: &str, span: HirSourceSpan) -> bir::LocalId {
        if let Some(&id) = self.external_locals.get(name) {
            return id;
        }
        let id = bir::LocalId(self.next_local);
        self.next_local += 1;
        self.locals.push(bir::LocalDecl {
            id,
            name: Some(name.to_string()),
            identity: None,
            ty: IncanType::Unknown,
            origin: bir::LocalOrigin::External,
            scope: bir::ScopeId(0),
            span,
        });
        self.external_locals.insert(name.to_string(), id);
        id
    }

    // ---- Ownership facts ----

    /// Read a checked source place, consuming its root's textual occurrence exactly once even for nested projections.
    ///
    /// Place construction does not read the base separately. Generated pattern/range projections instead use
    /// [`Self::ownership_fact_for_place`] directly: several generated reads may implement one source occurrence,
    /// so they must not consume the source countdown. Canonical local identity selects the owner, not field spelling.
    pub(super) fn source_place_operand(&mut self, place: bir::Place, ty: &IncanType) -> bir::Operand {
        if !place.projection.is_empty() {
            let _ = self.consume_place_read(&place);
        }
        let (fact, last_use) = self.ownership_fact_for_place(&place, ty);
        bir::Operand::place(place, fact, last_use)
    }

    /// Consume one seeded read through its canonical owner, returning the remaining source occurrences.
    /// Source projections and shared comparisons count once; generated projections must not consume this budget.
    pub(super) fn consume_place_read(&mut self, place: &bir::Place) -> Option<usize> {
        let local = place.local_id()?;
        for path in &mut self.terminal_paths {
            if let Some(reads) = path.reads.get_mut(&local) {
                *reads = reads.saturating_sub(1);
            }
        }
        let remaining = self.remaining_reads.get_mut(&local)?;
        remaining.remaining = remaining.remaining.saturating_sub(1);
        remaining.projected |= !place.projection.is_empty();
        Some(remaining.remaining)
    }

    /// Enter a block that ends by returning, counting the reads it holds of each non-Copy owner declared before it.
    /// The count is textual over the block, as the ordinary countdown is over a suffix, so it is never too low.
    pub(crate) fn enter_terminal_path(&mut self, stmts: &[ast::Spanned<ast::Statement>], scope: bir::ScopeId) {
        let owners = self
            .remaining_reads
            .iter()
            .filter(|(_, countdown)| countdown.remaining > 0)
            .map(|(local, _)| *local)
            .filter(|local| self.local_is_outside_scope(*local, scope))
            .collect::<Vec<_>>();
        let reads = owners
            .into_iter()
            .filter_map(|local| {
                let declaration = self.locals.get(local.index())?;
                if declaration.ty.abi_v0_facts().ownership.is_trivially_copy() {
                    return None;
                }
                Some((local, count_reads_in_stmts(declaration.name.as_deref()?, stmts)))
            })
            .collect();
        self.terminal_paths.push(TerminalPath {
            scope,
            conditionals: self.conditional_scopes.len(),
            loops: self.loop_scopes.len(),
            reads,
        });
    }

    /// Whether the read just consumed was `local`'s last on the innermost block that ends by returning, read at that
    /// block's own depth, so no statement on this path reads it again.
    pub(super) fn last_read_on_terminal_path(&self, local: bir::LocalId) -> bool {
        self.terminal_paths.last().is_some_and(|path| {
            path.reads.get(&local) == Some(&0)
                && self.loop_scopes.len() <= path.loops
                && path.conditionals == self.conditional_scopes.len()
                && self.local_is_outside_scope(local, path.scope)
        })
    }

    /// Select the Duckborrower fact and last-use marker for reading `place`.
    ///
    /// Explicit shared-reference values copy their pointer, including projected references; exclusive references
    /// remain transfers or reborrows. This differs from a borrowed ABI slot whose logical source type is an owner.
    /// Projected reads (`.field`, `[index]`) never move: v0 does not track partial-move state, so a non-Copy
    /// projected read borrows rather than risking an unsound move out of a place the surrounding code still owns.
    /// A checked source enum field instead records an explicit clone when read as a value; reference consumers retain
    /// their ordinary borrow adjustment. A bare read of a [`bir::LocalOrigin::Receiver`] local (`self`/`mut self`)
    /// never moves either, for a stronger reason than the projected case: a receiver is always a Rust-level
    /// reference in the native frame, so moving a non-Copy value out of it is invalid. Producing an owned value
    /// therefore clones the referent. A bare read of caller-owned parameter storage, identified by
    /// [`Self::is_borrowed_parameter`], never moves for the same reason: a collection parameter is a shared
    /// binding, or a mutable borrow when declared `mut` (RFC 129). An owned value read from it clones; passing it
    /// on to a `mut` parameter instead retains a reborrow fact. Every other bare local read decrements its
    /// remaining-reads countdown; reaching zero selects `Move` (and records the local as moved for
    /// [`Self::insert_scope_drops`]), otherwise `Clone`. A pending earlier call argument keeps its owner alive
    /// across later nested argument evaluation, so even its final textual read clones until the outer call consumes
    /// that operand. An owner declared outside the active loop also clones at its last textual read, unless the read
    /// consumes the value replaced by its enclosing assignment. Loop-local owners can transfer at their last use. A
    /// local with no tracked countdown (an [`bir::LocalOrigin::External`] reference) gets the explicit
    /// [`bir::OwnershipFact::Unknown`]. Source projections consume their counted occurrence through
    /// [`Self::source_place_operand`]; synthetic projections do not. A newly eligible final read in a conditional body
    /// retains an enclosing owner for the other paths' scope-exit drops. Bare-only native transfers retain their
    /// existing contract, including owners without Clone. After the join, a final whole-owner read may transfer
    /// normally. An owner's last read in a block that ends by returning transfers it on that path alone, since no
    /// later statement is on that path (see [`TerminalPath`]). This remains conservative across shadowed names and
    /// unvisited source-read forms.
    pub(super) fn ownership_fact_for_place(
        &mut self,
        place: &bir::Place,
        ty: &IncanType,
    ) -> (bir::OwnershipFact, bool) {
        let is_copy = matches!(ty, IncanType::Ref(_)) || ty.abi_v0_facts().ownership.is_trivially_copy();
        if !place.projection.is_empty() {
            let fact = if is_copy {
                bir::OwnershipFact::Copy
            } else if self.is_checked_enum_value(ty) {
                bir::OwnershipFact::Clone
            } else {
                bir::OwnershipFact::Borrow
            };
            return (fact, false);
        }
        let Some(local) = place.local_id() else {
            return (
                if is_copy {
                    bir::OwnershipFact::Copy
                } else {
                    bir::OwnershipFact::Clone
                },
                false,
            );
        };
        if self.is_receiver_local(local) || self.is_borrowed_parameter(local) {
            let fact = if is_copy {
                bir::OwnershipFact::Copy
            } else {
                bir::OwnershipFact::Clone
            };
            return (fact, false);
        }
        if is_copy {
            let _ = self.consume_place_read(place);
            return (bir::OwnershipFact::Copy, false);
        }
        let reused_by_loop = self.loop_reuses_local(local);
        let shortened_by_projection = self.remaining_reads.get(&local).is_some_and(|reads| reads.projected);
        let conditional_owner = shortened_by_projection
            && self
                .conditional_scopes
                .last()
                .is_some_and(|scope| self.local_is_outside_scope(local, *scope));
        let Some(remaining) = self.consume_place_read(place) else {
            return (bir::OwnershipFact::Unknown, false);
        };
        if remaining == 0 && !self.pending_argument_reads.contains(&local) && !reused_by_loop && !conditional_owner {
            self.moved_out.insert(local);
            (bir::OwnershipFact::Move, true)
        } else if !self.pending_argument_reads.contains(&local) && self.last_read_on_terminal_path(local) {
            // The block returns after this read, so the owner transfers on this path alone. Other paths still own
            // it, and its scope-exit drop stays for them.
            (bir::OwnershipFact::Move, true)
        } else {
            (bir::OwnershipFact::Clone, false)
        }
    }

    /// Identify storage declared outside the innermost loop, whose textual final read can execute repeatedly.
    /// Descendants of the loop scope are initialized per iteration and retain ordinary last-use transfers.
    pub(super) fn loop_reuses_local(&self, local: bir::LocalId) -> bool {
        if self.replacing_local == Some((local, self.loop_scopes.len())) {
            return false;
        }
        let Some(loop_scope) = self.loop_scopes.last() else {
            return false;
        };
        self.local_is_outside_scope(local, *loop_scope)
    }

    /// Identify storage that predates a control-flow body; only descendants are initialized on that body's path.
    pub(super) fn local_is_outside_scope(&self, local: bir::LocalId, boundary: bir::ScopeId) -> bool {
        let Some(declaration) = self.locals.get(local.index()) else {
            return false;
        };
        let mut scope = Some(declaration.scope);
        while let Some(current) = scope {
            if current == boundary {
                return false;
            }
            scope = self
                .scopes
                .iter()
                .find(|entry| entry.id == current)
                .and_then(|entry| entry.parent);
        }
        true
    }

    /// Whether `local` is a method's `self`/`mut self` receiver, per its recorded [`bir::LocalOrigin`].
    pub(super) fn is_receiver_local(&self, local: bir::LocalId) -> bool {
        self.locals
            .get(local.index())
            .is_some_and(|decl| matches!(decl.origin, bir::LocalOrigin::Receiver { .. }))
    }

    /// Identify caller-owned parameter storage, including shared collection bindings (RFC 129).
    ///
    /// An ordinary source list, set, or dictionary parameter borrows caller storage. Rest packs and checked native
    /// collection value parameters own their storage, so last-use reads may move it and scope exit drops it.
    pub(super) fn is_borrowed_parameter(&self, local: bir::LocalId) -> bool {
        if self.owned_parameters.contains(&local) {
            return false;
        }
        self.borrowed_parameters.contains(&local)
            || self.locals.get(local.index()).is_some_and(|decl| {
                matches!(decl.origin, bir::LocalOrigin::Parameter)
                    && matches!(
                        decl.ty.collection(),
                        Some((
                            CollectionTypeId::List | CollectionTypeId::Set | CollectionTypeId::Dict,
                            _
                        ))
                    )
            })
    }

    /// Build the operand for a freshly created temporary's single, immediate use.
    pub(super) fn temp_operand(&self, local: bir::LocalId, ty: &IncanType) -> bir::Operand {
        let fact = if ty.abi_v0_facts().ownership.is_trivially_copy() {
            bir::OwnershipFact::Copy
        } else {
            bir::OwnershipFact::Move
        };
        bir::Operand::place(bir::Place::from_local(local), fact, true)
    }

    /// Record a runtime/helper requirement for this body, deduplicated and kept in first-seen order (see
    /// [`bir::Body::runtime_requirements`] for why lowering relies on traversal order rather than sorting).
    pub(super) fn record_runtime_requirement(&mut self, requirement: AbiV0RuntimeRequirement) {
        if !self.runtime_requirements.contains(&requirement) {
            self.runtime_requirements.push(requirement);
        }
    }

    /// Emit explicit `Drop` statements in reverse declaration order for every non-Copy owned binding, parameter or
    /// temporary declared directly in `scope` that was never moved out. A native borrow guard must be released before
    /// an earlier owner binding, rather than waiting for the final temporary cleanup after that owner's explicit drop.
    /// This is scoped to locals declared *directly* in
    /// this block — it does not attempt cross-branch or early-return/break drop-obligation dataflow, which needs
    /// full control-flow analysis out of scope for v0 (see [`incan_semantics_core::body_ir`] module docs).
    pub(super) fn insert_scope_drops(&mut self, stmts: &mut Vec<bir::Statement>, scope: bir::ScopeId) {
        let span = self.scope_span(scope);
        let candidates: Vec<bir::LocalId> = self
            .locals
            .iter()
            .rev()
            .filter(|local| local.scope == scope)
            .filter(|local| {
                matches!(
                    local.origin,
                    bir::LocalOrigin::UserBinding | bir::LocalOrigin::Parameter | bir::LocalOrigin::Temporary
                )
            })
            .filter(|local| !local.ty.abi_v0_facts().ownership.is_trivially_copy())
            .map(|local| local.id)
            .collect();
        for id in candidates {
            if self.moved_out.contains(&id) || self.is_borrowed_parameter(id) {
                continue;
            }
            stmts.push(bir::Statement {
                kind: bir::StatementKind::Drop { local: id },
                span,
            });
        }
    }

    /// Push a [`bir::StatementKind::Unsupported`] statement carrying a short diagnostic `description`, so an
    /// unmodeled source construct still leaves a total, structurally valid statement rather than being dropped.
    pub(super) fn push_unsupported_stmt(
        &self,
        description: String,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) {
        out.push(bir::Statement {
            kind: bir::StatementKind::Unsupported { description },
            span,
        });
    }

    /// Emit an `Unsupported` marker statement and return a handle operand for it, so callers evaluating an
    /// unsupported expression in value position still get a structurally valid [`bir::Operand`] to thread onward.
    pub(super) fn unsupported_operand(
        &mut self,
        description: String,
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        self.unsupported_typed_operand(description, IncanType::Unknown, scope, span, out)
    }

    /// Preserve a known expression carrier on a refusal placeholder, so frame admission cannot mask the named marker.
    /// The marker must abort execution before this uninitialized operand is consumed; its ownership remains unknown.
    pub(super) fn unsupported_typed_operand(
        &mut self,
        description: String,
        ty: IncanType,
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let temp = self.new_temp(ty, scope, span);
        self.push_unsupported_stmt(description, span, out);
        bir::Operand::place(bir::Place::from_local(temp), bir::OwnershipFact::Unknown, true)
    }

    // ---- Rvalue / call helpers ----

    /// Allocate a fresh temporary, push an `Assign` statement giving it `rvalue`'s value, and return an operand for
    /// that temporary's single, immediate use (see [`Self::temp_operand`]). The common tail shared by every
    /// expression-lowering path that needs to flatten a computed value into a place before it can be read again.
    pub(super) fn push_assign_temp(
        &mut self,
        rvalue: bir::Rvalue,
        ty: IncanType,
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let temp = self.new_temp(ty.clone(), scope, span);
        out.push(bir::Statement {
            kind: bir::StatementKind::Assign {
                place: bir::Place::from_local(temp),
                rvalue,
            },
            span,
        });
        self.temp_operand(temp, &ty)
    }

    /// Allocate a fresh temporary, push a `Call` statement storing its result there, and return an operand for that
    /// temporary's single, immediate use — the call-lowering counterpart to [`Self::push_assign_temp`].
    #[allow(clippy::too_many_arguments)]
    pub(super) fn push_call_temp(
        &mut self,
        callee: bir::Callee,
        args: Vec<bir::ArgumentElement>,
        ty: IncanType,
        scope: bir::ScopeId,
        span: HirSourceSpan,
        may_panic: bool,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let temp = self.new_temp(ty.clone(), scope, span);
        out.push(bir::Statement {
            kind: bir::StatementKind::Call {
                destination: Some(bir::Place::from_local(temp)),
                callee,
                args,
                may_panic,
            },
            span,
        });
        self.temp_operand(temp, &ty)
    }

    /// Build the boolean negation of `operand` as a fresh temporary (`not operand`), used to turn a loop's
    /// continuation condition into its complementary exit condition for the leading conditional `Break`.
    pub(super) fn negate_operand(
        &mut self,
        operand: bir::Operand,
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        self.push_assign_temp(
            bir::Rvalue::UnaryOp(bir::UnOp::Not, operand),
            IncanType::Primitive(IncanPrimitiveType::Bool),
            scope,
            span,
            out,
        )
    }

    // ---- Statements ----
}
