//! The `LanguageServer` protocol implementation.

use super::*;

#[tower_lsp::async_trait]
impl LanguageServer for IncanLanguageServer {
    /// Advertise the LSP capabilities implemented by the Incan language server.
    async fn initialize(&self, _: InitializeParams) -> Result<InitializeResult> {
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                // Real-time diagnostics via text sync
                text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),
                // Hover support
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                // Signature help for parsed local function calls
                signature_help_provider: Some(SignatureHelpOptions {
                    trigger_characters: Some(vec!["(".to_string(), ",".to_string()]),
                    retrigger_characters: Some(vec![",".to_string()]),
                    ..Default::default()
                }),
                // Go-to-definition
                definition_provider: Some(OneOf::Left(true)),
                // Canonical-identity references across open checked documents
                references_provider: Some(OneOf::Left(true)),
                call_hierarchy_provider: Some(CallHierarchyServerCapability::Simple(true)),
                // Document symbols (outline)
                document_symbol_provider: Some(OneOf::Left(true)),
                // Completions (basic)
                completion_provider: Some(CompletionOptions {
                    trigger_characters: Some(vec![".".to_string(), ":".to_string(), "[".to_string()]),
                    ..Default::default()
                }),
                execute_command_provider: Some(ExecuteCommandOptions {
                    commands: vec![EMIT_CONTRACT_MODEL_COMMAND.to_string()],
                    ..Default::default()
                }),
                // Semantic highlighting for the whole language, including RFC 081 embedded-fragment ownership.
                semantic_tokens_provider: Some(SemanticTokensServerCapabilities::SemanticTokensOptions(
                    SemanticTokensOptions {
                        legend: semantic_tokens::legend(),
                        full: Some(SemanticTokensFullOptions::Bool(true)),
                        range: Some(false),
                        ..Default::default()
                    },
                )),
                ..Default::default()
            },
            server_info: Some(ServerInfo {
                name: "incan-lsp".to_string(),
                version: Some(incan_lang::version::INCAN_VERSION.to_string()),
            }),
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.client
            .log_message(MessageType::INFO, "Incan LSP initialized")
            .await;
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    /// Handle editor commands that need checked metadata not otherwise exposed by standard LSP requests.
    async fn execute_command(&self, params: ExecuteCommandParams) -> Result<Option<serde_json::Value>> {
        if params.command != EMIT_CONTRACT_MODEL_COMMAND {
            self.client
                .log_message(
                    MessageType::WARNING,
                    format!("unsupported Incan LSP command `{}`", params.command),
                )
                .await;
            return Ok(Some(json!({
                "error": format!("unsupported command `{}`", params.command)
            })));
        }

        let parsed = match parse_emit_contract_model_command_args(params.arguments) {
            Ok(parsed) => parsed,
            Err(error) => {
                return Ok(Some(json!({ "error": error })));
            }
        };
        let Some(uri_or_path) = parsed.uri.as_deref().or(parsed.path.as_deref()) else {
            return Ok(Some(json!({
                "error": format!("{EMIT_CONTRACT_MODEL_COMMAND} requires `uri` or `path`")
            })));
        };
        let path = match path_from_lsp_uri_or_path(uri_or_path) {
            Ok(path) => path,
            Err(error) => return Ok(Some(json!({ "error": error }))),
        };
        let format = match ContractModelCommandFormat::parse(parsed.format.as_deref()) {
            Ok(format) => format,
            Err(error) => return Ok(Some(json!({ "error": error }))),
        };
        let profile = Arc::clone(&self.profile);
        let emitted = tokio::task::spawn_blocking(move || {
            emit_contract_model_command_payload(&path, &parsed.model, format, &profile)
        })
        .await;
        match emitted {
            Ok(Ok(payload)) => Ok(Some(payload)),
            Ok(Err(error)) => Ok(Some(json!({ "error": error }))),
            Err(error) => Ok(Some(json!({ "error": format!("model emission failed: {error}") }))),
        }
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri;
        let source = params.text_document.text;
        let version = params.text_document.version;

        self.analyze_document(&uri, &source, version, false).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri;
        let version = params.text_document.version;

        // We use FULL sync, so there's only one change with the full content
        if let Some(change) = params.content_changes.into_iter().next() {
            self.analyze_document(&uri, &change.text, version, true).await;
        }
    }

    /// Discard pending analysis and cached facts before clearing the closed document's diagnostics.
    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;
        let mut tickets = self.analysis_tickets.write().await;
        tickets.remove(&uri);
        self.pending_sources.write().await.remove(&uri);

        // Remove document from cache
        let mut docs = self.documents.write().await;
        graph_navigation::invalidate(&uri, &mut docs);
        docs.remove(&uri);

        // Clear diagnostics
        self.client.publish_diagnostics(uri, vec![], None).await;
    }

    /// Provide hover text for symbols, call-site type arguments, Rust imports, and contextual receiver bindings.
    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;

        let docs = self.documents.read().await;
        let doc = match docs.get(uri) {
            Some(doc) => doc,
            None => return Ok(None),
        };

        let ast = match &doc.ast {
            Some(ast) => ast,
            None => return Ok(None),
        };

        if let Some(offset) = position_to_offset(&doc.source, position) {
            let aliases = collect_import_aliases(ast);

            // Ownership first (RFC 081, #1022). Every path below this resolves names against ordinary Incan, and
            // most of them start from a raw source-text identifier scan that cannot tell a tag name or a selector
            // from an Incan binding that happens to share its spelling. Inside a fragment's DSL-owned syntax the
            // only correct answer is who owns this text; an expression hole is ordinary Incan and falls through.
            if let Some(hover) = source_surface_hover(doc, ast, offset, &aliases) {
                return Ok(Some(hover));
            }

            if let Some(hover) = resolved_value_hover(uri, doc, ast, offset, &aliases) {
                return Ok(Some(hover));
            }

            // Call-site explicit type arguments: `f[T](...)`, `_.method[U](...)`
            if let Some(ty_spanned) = call_site_type_args::call_site_innermost_type_at_offset(ast, offset) {
                let display = format_type(&ty_spanned.node);
                let markdown = match &ty_spanned.node {
                    Type::Infer => "```incan\n_\n```\n\n*Call-site inference placeholder* — this type parameter is filled from the value arguments (RFC 054)."
                        .to_string(),
                    Type::Simple(_) => {
                        format!("```incan\n{display}\n```\n\n*Unresolved builtin or unqualified type argument.*")
                    }
                    _ => format!("```incan\n{display}\n```\n\n*Type argument* at call site (RFC 054)."),
                };
                return Ok(Some(Hover {
                    contents: HoverContents::Markup(MarkupContent {
                        kind: MarkupKind::Markdown,
                        value: markdown,
                    }),
                    range: Some(span_to_range(&doc.source, ty_spanned.span.start, ty_spanned.span.end)),
                }));
            }

            if let Some(preview) = api_metadata_preview_at_offset(&doc.api_metadata_previews, offset) {
                return Ok(Some(Hover {
                    contents: HoverContents::Markup(MarkupContent {
                        kind: MarkupKind::Markdown,
                        value: preview.markdown.clone(),
                    }),
                    range: Some(span_to_range(&doc.source, preview.span.start, preview.span.end)),
                }));
            }

            if let Some(scoped_symbol) =
                scoped_symbol_at_offset(ast, &doc.source, &doc.library_imported_dsl_surfaces, offset)
            {
                return Ok(Some(Hover {
                    contents: HoverContents::Markup(MarkupContent {
                        kind: MarkupKind::Markdown,
                        value: scoped_symbol_hover_markdown(scoped_symbol.dependency_key, scoped_symbol.descriptor),
                    }),
                    range: Some(span_to_range(
                        &doc.source,
                        scoped_symbol.symbol_span.start,
                        scoped_symbol.symbol_span.end,
                    )),
                }));
            }
        }

        if position_to_offset(&doc.source, position)
            .and_then(|offset| identifier_at_offset(&doc.source, offset))
            .is_some()
        {
            return Ok(None);
        }

        if let Some(info) = self.find_symbol_at_position(ast, &doc.source, position) {
            let detail = if info.kind == "const" {
                if let Some(resolved) = doc.const_types.get(&info.name) {
                    format!("const {}: {}", info.name, resolved)
                } else {
                    info.detail.clone()
                }
            } else {
                info.detail.clone()
            };

            let mut markdown = format!("```incan\n{}\n```\n\n*{}*", detail, info.kind);

            if info.kind == "rusttype"
                && let Some(rust_path) = doc.rusttype_info.get(&info.name)
            {
                markdown.push_str(&format!("\n\nunderlying Rust type: `rust::{}`", rust_path));
            }
            if let Some(note) = inactive_declaration_note_at_offset(ast, info.span.start, &doc.active_features) {
                markdown.push_str(&format!("\n\n**Package feature state:** {note}"));
            }

            return Ok(Some(Hover {
                contents: HoverContents::Markup(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: markdown,
                }),
                range: Some(span_to_range(&doc.source, info.span.start, info.span.end)),
            }));
        }

        Ok(None)
    }

    /// Provide signature help for parsed local function calls.
    async fn signature_help(&self, params: SignatureHelpParams) -> Result<Option<SignatureHelp>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;

        let docs = self.documents.read().await;
        let doc = match docs.get(uri) {
            Some(doc) => doc,
            None => return Ok(None),
        };
        let ast = match &doc.ast {
            Some(ast) => ast,
            None => return Ok(None),
        };
        let Some(offset) = position_to_offset(&doc.source, position) else {
            return Ok(None);
        };
        Ok(local_signature_help_at_offset(ast, &doc.source, offset))
    }

    /// Return document symbols for the current parsed document, if available. Answer
    /// `textDocument/semanticTokens/full` for an open document.
    ///
    /// The reply covers every token in the file, as the protocol requires: there is no way to classify one region
    /// and leave the rest to the editor's grammar. A document with no stored state yields `None` so the client keeps
    /// its previous result rather than briefly losing all color; a document that failed to parse still answers,
    /// from the token stream alone, because losing highlighting on every keystroke is worse than never having it.
    async fn semantic_tokens_full(&self, params: SemanticTokensParams) -> Result<Option<SemanticTokensResult>> {
        let docs = self.documents.read().await;
        let Some(doc) = docs.get(&params.text_document.uri) else {
            return Ok(None);
        };
        let data = semantic_tokens::semantic_tokens(&doc.source, doc.ast.as_ref());
        Ok(Some(SemanticTokensResult::Tokens(SemanticTokens {
            result_id: None,
            data,
        })))
    }

    async fn document_symbol(&self, params: DocumentSymbolParams) -> Result<Option<DocumentSymbolResponse>> {
        let uri = &params.text_document.uri;
        let docs = self.documents.read().await;
        let doc = match docs.get(uri) {
            Some(doc) => doc,
            None => return Ok(None),
        };
        let ast = match &doc.ast {
            Some(ast) => ast,
            None => return Ok(None),
        };

        let mut symbols = Vec::new();
        for decl in &ast.declarations {
            let Some(kind) = lsp_symbol_kind_for_decl(&decl.node) else {
                continue;
            };
            let Some((name, mut detail)) = lsp_document_symbol_name_and_detail(&decl.node, &doc.source) else {
                continue;
            };
            if !declaration_active_for_lsp(decl, &doc.active_features) {
                detail.push_str(&format!(
                    " — inactive; requires {}",
                    decl.required_features
                        .iter()
                        .map(|feature| format!("`{feature}`"))
                        .collect::<Vec<_>>()
                        .join(" and ")
                ));
            }
            let range = span_to_range(&doc.source, decl.span.start, decl.span.end);
            #[allow(deprecated)]
            let symbol = DocumentSymbol {
                name,
                detail: Some(detail),
                kind,
                tags: None,
                deprecated: None,
                range,
                selection_range: range,
                children: None,
            };
            symbols.push(symbol);
        }

        Ok(Some(DocumentSymbolResponse::Nested(symbols)))
    }

    /// Resolve go-to-definition for local symbols, stdlib imports, decorators, and DSL-scoped symbols.
    async fn goto_definition(&self, params: GotoDefinitionParams) -> Result<Option<GotoDefinitionResponse>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;

        let docs = self.documents.read().await;
        let doc = match docs.get(uri) {
            Some(doc) => doc,
            None => return Ok(None),
        };

        let ast = match &doc.ast {
            Some(ast) => ast,
            None => return Ok(None),
        };

        let Some(offset) = position_to_offset(&doc.source, position) else {
            return Ok(None);
        };
        // Same ownership rule as `hover` (RFC 081, #1022): a tag name or selector inside a fragment has no
        // definition in ordinary Incan, and jumping to an unrelated binding that shares its spelling is worse than
        // answering nothing. Expression holes are ordinary Incan and fall through to every path below.
        if embedded_dsl_ownership_at_offset(ast, offset).is_some() {
            return Ok(None);
        }
        if let Some(preview) = registry_preview_at_offset(&doc.registry_previews, offset) {
            return Ok(Some(GotoDefinitionResponse::Scalar(Location {
                uri: uri.clone(),
                range: span_to_range(&doc.source, preview.subject_span.start, preview.subject_span.end),
            })));
        }
        if let Some(preview) = c_binding_preview_at_offset(&doc.c_binding_previews, offset)
            && preview.span != preview.declaration_span
        {
            return Ok(Some(GotoDefinitionResponse::Scalar(Location {
                uri: uri.clone(),
                range: span_to_range(
                    &doc.source,
                    preview.declaration_span.start,
                    preview.declaration_span.end,
                ),
            })));
        }

        if let Some(scoped_symbol) =
            scoped_symbol_at_offset(ast, &doc.source, &doc.library_imported_dsl_surfaces, offset)
            && let Some(import_span) =
                find_pub_library_import_span(ast, scoped_symbol.dependency_key, scoped_symbol.symbol_span.start)
        {
            return Ok(Some(GotoDefinitionResponse::Scalar(Location {
                uri: uri.clone(),
                range: span_to_range(&doc.source, import_span.start, import_span.end),
            })));
        }

        if let Some(occurrence) = checked_identity_at_offset(uri, ast, &doc.source, &doc.checked_identities, offset) {
            if doc
                .graph_navigation
                .as_ref()
                .is_some_and(|graph| graph.is_current(&docs))
                && let Some(location) = graph_navigation::definition(doc, &docs, &occurrence.identity)
            {
                return Ok(Some(GotoDefinitionResponse::Scalar(location)));
            }
            // Published dependencies without a live declaration record retain their checked manifest locations.
            if let Some(location) =
                definition_location_for_identity(&occurrence.identity, &doc.checked_identities.declaration_sources)
            {
                return Ok(Some(GotoDefinitionResponse::Scalar(location)));
            }
        }

        Ok(None)
    }

    /// Return references that share the exact compiler-owned canonical identity across open checked documents.
    async fn references(&self, params: ReferenceParams) -> Result<Option<Vec<Location>>> {
        let uri = &params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;
        let docs = self.documents.read().await;
        let Some(doc) = docs.get(uri) else {
            return Ok(None);
        };
        let Some(ast) = doc.ast.as_ref() else {
            return Ok(None);
        };
        let Some(offset) = position_to_offset(&doc.source, position) else {
            return Ok(None);
        };
        let Some(occurrence) = checked_identity_at_offset(uri, ast, &doc.source, &doc.checked_identities, offset)
        else {
            return Ok(None);
        };
        Ok(graph_navigation::references(
            doc,
            &docs,
            &occurrence.identity,
            params.context.include_declaration,
        ))
    }

    /// Prepare a checked declaration for graph-backed incoming and outgoing call navigation.
    async fn prepare_call_hierarchy(
        &self,
        params: CallHierarchyPrepareParams,
    ) -> Result<Option<Vec<CallHierarchyItem>>> {
        let docs = self.documents.read().await;
        Ok(graph_navigation::prepare(
            &params.text_document_position_params.text_document.uri,
            params.text_document_position_params.position,
            &docs,
        ))
    }

    /// Traverse only incoming calls recorded by the current shared graph export.
    async fn incoming_calls(
        &self,
        params: CallHierarchyIncomingCallsParams,
    ) -> Result<Option<Vec<CallHierarchyIncomingCall>>> {
        let docs = self.documents.read().await;
        Ok(graph_navigation::incoming(&params.item, &docs))
    }

    /// Traverse only outgoing calls recorded by the current shared graph export.
    async fn outgoing_calls(
        &self,
        params: CallHierarchyOutgoingCallsParams,
    ) -> Result<Option<Vec<CallHierarchyOutgoingCall>>> {
        let docs = self.documents.read().await;
        Ok(graph_navigation::outgoing(&params.item, &docs))
    }

    /// Provide context-aware completions for decorators, imports, type arguments, symbols, and receiver bindings.
    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let uri = &params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;

        let docs = self.documents.read().await;
        let doc = match docs.get(uri) {
            Some(doc) => doc,
            None => return Ok(None),
        };

        let mut items: Vec<CompletionItem> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();

        // Extract the current line text before the cursor for context-aware completions.
        let line_prefix = line_text_before_cursor(&doc.source, position);

        // ---- Context-specific completions ----
        if let Some(items) = specific_completion_items(doc, &line_prefix, position) {
            return Ok(Some(CompletionResponse::Array(items)));
        }

        // ---- General completions (not in a specific context) ----

        push_contextual_completions(doc, position, &mut items, &mut seen);
        push_builtin_completions(&mut items, &mut seen);

        push_document_completions(doc, &mut items, &mut seen);

        push_rust_binding_completions(doc, &mut items, &mut seen);

        // Add `std` as a module name so import completions can start from it.
        push_completion(
            &mut items,
            &mut seen,
            "std",
            CompletionItemKind::MODULE,
            Some("Incan standard library namespace".to_string()),
            None,
        );

        Ok(Some(CompletionResponse::Array(items)))
    }
}
