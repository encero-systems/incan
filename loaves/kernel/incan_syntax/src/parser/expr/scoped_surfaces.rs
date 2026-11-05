/// Scoped DSL surfaces: leading-dot paths, scoped glyphs and symbols, and their eligibility and owners.
impl<'a> Parser<'a> {
    /// Parse a descriptor-enabled leading-dot path if the current DSL block accepts one.
    fn try_scoped_leading_dot_path(&mut self, start: usize) -> Result<Option<Spanned<Expr>>, CompileError> {
        let Some(active) = self.active_leading_dot_surface_descriptor() else {
            return Ok(None);
        };
        let dependency_key = active.dependency_key.clone();
        let descriptor_key = active.descriptor.key.clone();
        let receiver = active
            .descriptor
            .receiver
            .clone()
            .unwrap_or(incan_vocab::ScopedSurfaceReceiver::OwningDeclaration);
        let owner = self.scoped_surface_owner(active);
        let (min_segments, max_segments) = match &active.descriptor.syntax {
            incan_vocab::ScopedSurfaceSyntax::LeadingDotPath {
                min_segments,
                max_segments,
            } => (*min_segments as usize, max_segments.map(usize::from)),
            _ => return Ok(None),
        };

        let mut segments = Vec::new();
        loop {
            self.expect_punct(PunctuationId::Dot, "Expected '.' at start of scoped leading-dot path")?;
            segments.push(self.identifier_or_any_keyword()?);
            if max_segments.is_some_and(|max_segments| segments.len() >= max_segments) {
                break;
            }
            if !self.check(&TokenKind::Punctuation(PunctuationId::Dot)) {
                break;
            }
            if !matches!(self.peek_next().kind, TokenKind::Ident(_) | TokenKind::Keyword(_)) {
                break;
            }
        }

        if segments.len() < min_segments {
            return Err(errors::expected_expression(
                "scoped leading-dot path",
                Span::new(start, self.current_span().end),
            ));
        }

        let end = self.tokens[self.pos.saturating_sub(1)].span.end;
        Ok(Some(Spanned::new(
            Expr::Surface(Box::new(SurfaceExpr {
                key: SurfaceFeatureKey::ScopedDslSurface {
                    dependency_key,
                    descriptor_key,
                },
                payload: SurfaceExprPayload::LeadingDotPath {
                    segments,
                    receiver,
                    owner,
                },
            })),
            Span::new(start, end),
        )))
    }

    /// Return the first active leading-dot descriptor accepted by the current scoped context.
    fn active_leading_dot_surface_descriptor(&self) -> Option<&ActiveScopedSurfaceDescriptor> {
        self.active_scoped_surface_descriptors.iter().find(|active| {
            active.descriptor.family == incan_vocab::ScopedSurfaceFamily::ExpressionForm
                && matches!(
                    active.descriptor.syntax,
                    incan_vocab::ScopedSurfaceSyntax::LeadingDotPath { .. }
                )
                && active
                    .descriptor
                    .eligible_in
                    .iter()
                    .any(|eligibility| self.scoped_surface_eligibility_accepts_current_context(eligibility))
        })
    }

    /// Build an author-provided outside-scope diagnostic for an active leading-dot descriptor.
    fn scoped_leading_dot_outside_scope_error(&self, start: usize) -> Option<CompileError> {
        if !matches!(self.peek_next().kind, TokenKind::Ident(_) | TokenKind::Keyword(_)) {
            return None;
        }
        let active = self.active_scoped_surface_descriptors.iter().find(|active| {
            active.descriptor.family == incan_vocab::ScopedSurfaceFamily::ExpressionForm
                && !matches!(
                    active.descriptor.misuse_scope,
                    incan_vocab::ScopedSurfaceMisuseScope::None
                )
                && matches!(
                    active.descriptor.syntax,
                    incan_vocab::ScopedSurfaceSyntax::LeadingDotPath { .. }
                )
        })?;
        let diagnostic = active
            .descriptor
            .diagnostics
            .iter()
            .find(|diagnostic| diagnostic.kind == incan_vocab::ScopedSurfaceDiagnosticKind::OutsideScope);
        let message = diagnostic
            .map(|diagnostic| diagnostic.message.clone())
            .unwrap_or_else(|| {
                format!(
                    "Scoped surface `{}` is not valid in this position",
                    active.descriptor.key
                )
            });
        let mut error = CompileError::syntax(message, Span::new(start, self.peek_next().span.end));
        if let Some(code) = diagnostic
            .map(|diagnostic| diagnostic.code.as_str())
            .filter(|code| !code.is_empty())
        {
            error = error.with_note(format!("diagnostic code: {code}"));
        }
        if let Some(help) = diagnostic.and_then(|diagnostic| diagnostic.help.as_deref()) {
            error = error.with_hint(help);
        }
        Some(error)
    }

    /// Return the first active operator-like or binding-like glyph descriptor accepted by the current context.
    fn active_scoped_glyph_surface_descriptor(&self, glyph: &str) -> Option<&ActiveScopedSurfaceDescriptor> {
        self.active_scoped_surface_descriptors.iter().find(|active| {
            matches!(
                active.descriptor.family,
                incan_vocab::ScopedSurfaceFamily::OperatorLike | incan_vocab::ScopedSurfaceFamily::BindingLike
            ) && matches!(
                &active.descriptor.syntax,
                incan_vocab::ScopedSurfaceSyntax::Glyph { spelling } if spelling == glyph
            ) && active
                .descriptor
                .eligible_in
                .iter()
                .any(|eligibility| self.scoped_surface_eligibility_accepts_current_context(eligibility))
        })
    }

    /// Return the innermost active scoped-symbol descriptor accepted by the current context.
    fn active_scoped_symbol_descriptor(
        &self,
        symbol: &str,
        span: Span,
    ) -> Result<Option<&ActiveScopedSymbolDescriptor>, CompileError> {
        let matches: Vec<_> = self
            .active_scoped_symbol_descriptors
            .iter()
            .filter(|active| {
                active.descriptor.symbol == symbol
                    && active
                        .descriptor
                        .eligible_in
                        .iter()
                        .any(|eligibility| self.scoped_symbol_eligibility_accepts_current_context(eligibility))
            })
            .collect();
        let Some(max_depth) = matches
            .iter()
            .map(|active| self.scoped_symbol_owner_depth(active))
            .max()
        else {
            return Ok(None);
        };
        let mut best = matches
            .into_iter()
            .filter(|active| self.scoped_symbol_owner_depth(active) == max_depth);
        let first = best.next();
        if best.next().is_some() {
            return Err(
                CompileError::syntax(format!("Ambiguous scoped symbol `{symbol}` in this DSL position"), span)
                    .with_hint("Use explicit qualification or adjust the active DSL symbol descriptors"),
            );
        }
        Ok(first)
    }

    /// Return the embedded-fragment descriptor (RFC 081) claiming the declaration-body position of `declaration`.
    ///
    /// This generalizes [`Parser::active_scoped_symbol_descriptor`]'s same-depth ambiguity pattern to embedded
    /// fragments: every activated descriptor whose `eligible_in` names `declaration`'s `DeclarationBody` position
    /// is at the *same* depth here (the check point is entering `declaration`'s body directly, not a nested
    /// name-resolution lookup), so two or more matches are unconditionally ambiguous rather than resolved by an
    /// innermost-wins tiebreak. `Ok(None)` means no descriptor claims this position, so the caller should fall
    /// back to ordinary RFC 040/045 statement-list parsing for the block body.
    pub(super) fn active_embedded_fragment_descriptor_for_declaration_body(
        &self,
        declaration: &str,
        span: Span,
    ) -> Result<Option<&incan_vocab::EmbeddedFragmentDescriptor>, CompileError> {
        let matches: Vec<&ActiveEmbeddedFragmentDescriptor> = self
            .active_embedded_fragment_descriptors
            .iter()
            .filter(|active| {
                active.descriptor.eligible_in.iter().any(|eligibility| {
                    eligibility.position == incan_vocab::ScopedSurfacePosition::DeclarationBody
                        && eligibility.declaration == declaration
                })
            })
            .collect();

        match matches.as_slice() {
            [] => Ok(None),
            [single] => Ok(Some(&single.descriptor)),
            [first, second, ..] => Err(CompileError::syntax(
                format!(
                    "Ambiguous embedded-fragment descriptors `{}` and `{}` both claim the `{declaration}` block body",
                    first.descriptor.key, second.descriptor.key
                ),
                span,
            )
            .with_hint(
                "Only one descriptor may claim a lexical submode for the same eligible position; remove or narrow \
                 one of the conflicting imports",
            )),
        }
    }

    /// Build a descriptor-owned diagnostic for a scoped symbol used inside its active DSL but outside an eligible
    /// position.
    fn scoped_symbol_misuse_error(&self, symbol: &str, span: Span) -> Result<Option<CompileError>, CompileError> {
        let matches: Vec<_> = self
            .active_scoped_symbol_descriptors
            .iter()
            .filter(|active| {
                active.descriptor.symbol == symbol
                    && active.descriptor.misuse_scope == incan_vocab::ScopedSymbolMisuseScope::ActiveDsl
                    && active
                        .descriptor
                        .eligible_in
                        .iter()
                        .any(|eligibility| self.scoped_symbol_eligibility_is_inside_owning_dsl(eligibility))
            })
            .collect();
        let Some(max_depth) = matches
            .iter()
            .filter_map(|active| self.scoped_symbol_owner_declaration_depth(active))
            .max()
        else {
            return Ok(None);
        };
        let mut best = matches
            .into_iter()
            .filter(|active| self.scoped_symbol_owner_declaration_depth(active) == Some(max_depth));
        let Some(active) = best.next() else {
            return Ok(None);
        };
        if best.next().is_some() {
            return Err(
                CompileError::syntax(format!("Ambiguous scoped symbol `{symbol}` in this DSL position"), span)
                    .with_hint("Use explicit qualification or adjust the active DSL symbol descriptors"),
            );
        }

        let diagnostic = active
            .descriptor
            .diagnostics
            .iter()
            .find(|diagnostic| diagnostic.kind == incan_vocab::ScopedSymbolDiagnosticKind::OutsideEligiblePosition);
        let message = diagnostic
            .map(|diagnostic| diagnostic.message.clone())
            .unwrap_or_else(|| {
                format!(
                    "Scoped symbol `{}` is not valid in this DSL position",
                    active.descriptor.symbol
                )
            });
        let mut error = CompileError::syntax(message, span);
        if let Some(code) = diagnostic
            .map(|diagnostic| diagnostic.code.as_str())
            .filter(|code| !code.is_empty())
        {
            error = error.with_note(format!("diagnostic code: {code}"));
        }
        if let Some(help) = diagnostic.and_then(|diagnostic| diagnostic.help.as_deref()) {
            error = error.with_hint(help);
        }
        Ok(Some(error))
    }

    /// Consume the longest active scoped glyph at the current token position.
    fn consume_active_scoped_glyph(&mut self) -> Option<(ActiveScopedSurfaceDescriptor, String)> {
        let mut best: Option<(ActiveScopedSurfaceDescriptor, String, usize)> = None;

        for active in &self.active_scoped_surface_descriptors {
            if !matches!(
                active.descriptor.family,
                incan_vocab::ScopedSurfaceFamily::OperatorLike | incan_vocab::ScopedSurfaceFamily::BindingLike
            ) {
                continue;
            }
            if !active
                .descriptor
                .eligible_in
                .iter()
                .any(|eligibility| self.scoped_surface_eligibility_accepts_current_context(eligibility))
            {
                continue;
            }
            let incan_vocab::ScopedSurfaceSyntax::Glyph { spelling } = &active.descriptor.syntax else {
                continue;
            };
            let Some(token_count) = self.scoped_glyph_token_count_at(spelling, self.pos) else {
                continue;
            };

            let should_replace = best
                .as_ref()
                .map(|(_, current_spelling, current_count)| {
                    token_count > *current_count
                        || (token_count == *current_count && spelling.len() > current_spelling.len())
                })
                .unwrap_or(true);
            if should_replace {
                best = Some((active.clone(), spelling.clone(), token_count));
            }
        }

        let (active, spelling, token_count) = best?;
        for _ in 0..token_count {
            self.advance();
        }
        Some((active, spelling))
    }

    /// Return whether an active scoped glyph starts `offset` tokens after the current parser position.
    fn active_scoped_glyph_starts_at_offset(&self, offset: usize) -> bool {
        let pos = self.pos.saturating_add(offset);
        self.active_scoped_surface_descriptors.iter().any(|active| {
            matches!(
                active.descriptor.family,
                incan_vocab::ScopedSurfaceFamily::OperatorLike | incan_vocab::ScopedSurfaceFamily::BindingLike
            ) && active
                .descriptor
                .eligible_in
                .iter()
                .any(|eligibility| self.scoped_surface_eligibility_accepts_current_context(eligibility))
                && matches!(
                    &active.descriptor.syntax,
                    incan_vocab::ScopedSurfaceSyntax::Glyph { spelling }
                        if self.scoped_glyph_token_count_at(spelling, pos).is_some()
                )
        })
    }

    /// Return the number of tokens that compose `spelling` at `pos`, if the token spellings match exactly.
    fn scoped_glyph_token_count_at(&self, spelling: &str, pos: usize) -> Option<usize> {
        let mut matched = String::new();
        let mut idx = pos;

        while matched.len() < spelling.len() {
            let piece = self.token_symbol_spelling(idx)?;
            matched.push_str(piece);
            if !spelling.starts_with(&matched) {
                return None;
            }
            idx += 1;
        }

        if matched == spelling { Some(idx - pos) } else { None }
    }

    /// Return the source spelling for an operator or punctuation token at `idx`.
    fn token_symbol_spelling(&self, idx: usize) -> Option<&'static str> {
        match self.tokens.get(idx).map(|token| &token.kind)? {
            TokenKind::Operator(id) => Some(incan_lang::lang::operators::info_for(*id).spellings[0]),
            TokenKind::Punctuation(id) => Some(incan_lang::lang::punctuation::as_str(*id)),
            _ => None,
        }
    }

    /// Return whether a scoped-surface eligibility matches the parser's current scoped context.
    fn scoped_surface_eligibility_accepts_current_context(
        &self,
        eligibility: &incan_vocab::ScopedSurfaceEligibility,
    ) -> bool {
        match eligibility.position {
            incan_vocab::ScopedSurfacePosition::DeclarationBody => {
                self.vocab_block_stack.last() == Some(&eligibility.declaration)
            }
            incan_vocab::ScopedSurfacePosition::ClauseBody => {
                self.current_vocab_clause_matches(eligibility.declaration.as_str(), eligibility.clause.as_deref())
            }
            incan_vocab::ScopedSurfacePosition::CallArgument => self
                .scoped_call_argument_stack
                .last()
                .is_some_and(|context| eligibility.call.as_deref() == Some(context.call.as_str())),
            incan_vocab::ScopedSurfacePosition::DeclarationHead => false,
            _ => false,
        }
    }

    /// Return whether a scoped-symbol eligibility matches the parser's current scoped context.
    fn scoped_symbol_eligibility_accepts_current_context(
        &self,
        eligibility: &incan_vocab::ScopedSymbolEligibility,
    ) -> bool {
        match eligibility.position {
            incan_vocab::ScopedSymbolPosition::DeclarationBody => {
                self.vocab_block_stack.last() == Some(&eligibility.declaration)
            }
            incan_vocab::ScopedSymbolPosition::ClauseBody => {
                self.current_vocab_clause_matches(eligibility.declaration.as_str(), eligibility.clause.as_deref())
            }
            incan_vocab::ScopedSymbolPosition::CallArgument => {
                self.scoped_call_argument_stack
                    .last()
                    .is_some_and(|context| eligibility.call.as_deref() == Some(context.call.as_str()))
                    && self
                        .vocab_block_stack
                        .iter()
                        .any(|declaration| declaration == &eligibility.declaration)
            }
            _ => false,
        }
    }

    /// Return whether the current parser position is inside the owning DSL declaration for an eligibility rule.
    fn scoped_symbol_eligibility_is_inside_owning_dsl(
        &self,
        eligibility: &incan_vocab::ScopedSymbolEligibility,
    ) -> bool {
        self.vocab_block_stack
            .iter()
            .any(|declaration| declaration == &eligibility.declaration)
    }

    /// Return whether the current vocab stack is positioned inside a clause-like nested vocab block.
    fn current_vocab_clause_matches(&self, declaration: &str, clause: Option<&str>) -> bool {
        let Some(clause) = clause else {
            return false;
        };
        let Some(clause_depth) = self.vocab_block_stack.iter().rposition(|active| active == clause) else {
            return false;
        };
        self.vocab_block_stack
            .iter()
            .take(clause_depth)
            .any(|active| active == declaration)
    }

    /// Build the owner metadata attached to a scoped-surface AST payload.
    fn scoped_surface_owner(&self, active: &ActiveScopedSurfaceDescriptor) -> ScopedSurfaceOwner {
        active
            .descriptor
            .eligible_in
            .iter()
            .find(|eligibility| self.scoped_surface_eligibility_accepts_current_context(eligibility))
            .map(|eligibility| ScopedSurfaceOwner {
                declaration: eligibility.declaration.clone(),
                clause: eligibility.clause.clone(),
                call: eligibility.call.clone(),
            })
            .unwrap_or_else(|| ScopedSurfaceOwner {
                declaration: self.vocab_block_stack.last().cloned().unwrap_or_default(),
                clause: None,
                call: self
                    .scoped_call_argument_stack
                    .last()
                    .map(|context| context.call.clone()),
            })
    }

    /// Build the owner metadata attached to a scoped-symbol AST payload.
    fn scoped_symbol_owner(&self, active: &ActiveScopedSymbolDescriptor) -> ScopedSurfaceOwner {
        active
            .descriptor
            .eligible_in
            .iter()
            .find(|eligibility| self.scoped_symbol_eligibility_accepts_current_context(eligibility))
            .map(|eligibility| ScopedSurfaceOwner {
                declaration: eligibility.declaration.clone(),
                clause: eligibility.clause.clone(),
                call: eligibility.call.clone(),
            })
            .unwrap_or_else(|| ScopedSurfaceOwner {
                declaration: self.vocab_block_stack.last().cloned().unwrap_or_default(),
                clause: None,
                call: self
                    .scoped_call_argument_stack
                    .last()
                    .map(|context| context.call.clone()),
            })
    }

    /// Return a lexical-depth proxy for nested scoped-symbol ownership.
    fn scoped_symbol_owner_depth(&self, active: &ActiveScopedSymbolDescriptor) -> usize {
        self.scoped_symbol_owner_declaration_depth(active).unwrap_or(0)
    }

    /// Return the innermost active declaration depth that can own this scoped symbol.
    fn scoped_symbol_owner_declaration_depth(&self, active: &ActiveScopedSymbolDescriptor) -> Option<usize> {
        active
            .descriptor
            .eligible_in
            .iter()
            .filter_map(|eligibility| {
                self.vocab_block_stack
                    .iter()
                    .rposition(|declaration| declaration == &eligibility.declaration)
            })
            .max()
    }

    /// Return the function or method name whose argument list is about to be parsed.
    fn call_argument_target(&self, expr: &Spanned<Expr>) -> Option<String> {
        match &expr.node {
            Expr::Ident(name) | Expr::Field(_, name) => Some(name.clone()),
            _ => None,
        }
    }
}
