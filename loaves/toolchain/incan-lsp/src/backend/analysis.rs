//! One document's analysis on the compiler's route: the session `incan check` opens, its module collector with the
//! editor's unsaved buffers in place of their files, and one checking pass whose document checker the editor reads.

use super::*;

use incan_driver::diagnostics::CliDiagnostic;
use incan_driver::features::FeatureSelection;
use incan_driver::modules::collect_document_modules;
use incan_driver::session::DEFAULT_PROFILE;
use incan_frontend::module::logical_module_segments_from_file;

impl IncanLanguageServer {
    /// Analyze a document and publish its diagnostics.
    pub(super) async fn analyze_document(&self, uri: &Url, source: &str, version: i32) {
        let open_sources = self.open_sources(uri, source).await;
        let (diagnostics, state) = match uri.to_file_path() {
            Ok(path) => analyze(
                uri,
                &path.canonicalize().unwrap_or(path),
                source,
                version,
                &open_sources,
            ),
            Err(()) => (
                vec![lsp_root_error_diagnostic(
                    "Incan analyzes documents that are files".to_string(),
                )],
                None,
            ),
        };
        if let Some(state) = state {
            self.documents.write().await.insert(uri.clone(), state);
        }
        self.client
            .publish_diagnostics(uri.clone(), diagnostics, Some(version))
            .await;
    }

    /// Every open document's buffer by canonical path, with `source` as `uri`'s.
    async fn open_sources(&self, uri: &Url, source: &str) -> HashMap<PathBuf, String> {
        let documents = self.documents.read().await;
        let mut sources = documents
            .iter()
            .filter_map(|(open, document)| {
                let path = open.to_file_path().ok()?;
                Some((path.canonicalize().unwrap_or(path), document.source.clone()))
            })
            .collect::<HashMap<_, _>>();
        if let Ok(path) = uri.to_file_path() {
            sources.insert(path.canonicalize().unwrap_or(path), source.to_string());
        }
        sources
    }
}

/// What the editor reads from the document module's checker.
struct DocumentFacts {
    type_info: TypeCheckInfo,
    api_metadata: CheckedApiMetadata,
    registry_previews: Vec<RegistryPreview>,
    c_binding_previews: Vec<CBindingPreview>,
    rust_origin_symbols: Vec<RustOriginSymbol>,
    const_types: HashMap<String, String>,
    value_types: Vec<ValueTypeFact>,
    rusttype_info: HashMap<String, String>,
    warnings: Vec<CompileError>,
}

impl DocumentFacts {
    /// Read the document's facts from its checker, whether or not it checked cleanly.
    fn read(module: &ParsedModule, checker: &typechecker::TypeChecker) -> Self {
        let mut const_types = HashMap::new();
        let mut value_types = Vec::new();
        for symbol in checker.symbols.all_symbols() {
            let ty = match &symbol.kind {
                FrontendSymbolKind::Variable(info) => &info.ty,
                FrontendSymbolKind::Static(info) => &info.ty,
                _ => continue,
            };
            value_types.push(ValueTypeFact {
                name: symbol.name.clone(),
                span: symbol.span,
                kind: value_type_kind(ty),
            });
        }
        let mut rusttype_info = HashMap::new();
        for declaration in &module.ast.declarations {
            let symbol = |name: &str| checker.symbols.lookup(name).and_then(|id| checker.symbols.get(id));
            match &declaration.node {
                Declaration::Const(konst) => {
                    if let Some(symbol) = symbol(&konst.name)
                        && let FrontendSymbolKind::Variable(variable) = &symbol.kind
                    {
                        const_types.insert(konst.name.clone(), variable.ty.to_string());
                    }
                }
                Declaration::Static(static_decl) => {
                    if let Some(symbol) = symbol(&static_decl.name)
                        && let FrontendSymbolKind::Static(info) = &symbol.kind
                    {
                        const_types.insert(static_decl.name.clone(), info.ty.to_string());
                    }
                }
                Declaration::Newtype(newtype) if newtype.is_rusttype => {
                    if let Some(symbol) = symbol(&newtype.name)
                        && let FrontendSymbolKind::Type(TypeInfo::Newtype(info)) = &symbol.kind
                        && let ResolvedType::RustPath(path) = &info.underlying
                    {
                        rusttype_info.insert(newtype.name.clone(), path.clone());
                    }
                }
                _ => {}
            }
        }
        Self {
            type_info: checker.type_info().clone(),
            api_metadata: collect_checked_api_metadata(&module.ast, checker, module.path_segments.clone()),
            registry_previews: registry_previews(&checker.type_info().registry),
            c_binding_previews: c_binding_previews(&checker.type_info().c_abi, &module.path_segments),
            rust_origin_symbols: collect_rust_origin_symbols(checker),
            const_types,
            value_types,
            rusttype_info,
            warnings: checker.warnings().to_vec(),
        }
    }
}

/// Analyze `path` with `source` as its text: the document's diagnostics, and its state once it parses.
fn analyze(
    uri: &Url,
    path: &Path,
    source: &str,
    version: i32,
    open_sources: &HashMap<PathBuf, String>,
) -> (Vec<Diagnostic>, Option<DocumentState>) {
    let mut diagnostics = Vec::new();
    let session = match CompilationSession::discover(path, &FeatureSelection::default(), DEFAULT_PROFILE) {
        Ok(session) => session,
        Err(error) => return (vec![lsp_root_error_diagnostic(error.to_string())], None),
    };

    // ---- The authored program, inactive feature declarations included: what navigation walks ----
    let source_ast =
        match session.parse_source_unprojected(path, source, false) {
            Ok(ast) => {
                diagnostics.extend(ast.warnings.iter().map(|warning| {
                    compile_error_to_diagnostic_with_phase(warning, source, uri, DiagnosticPhase::Parse)
                }));
                ast
            }
            Err(errors) => {
                diagnostics.extend(
                    errors.iter().map(|error| {
                        compile_error_to_diagnostic_with_phase(error, source, uri, DiagnosticPhase::Parse)
                    }),
                );
                return (diagnostics, None);
            }
        };

    // ---- One collection and one checking pass, as `incan check` runs them ----
    let segments =
        logical_module_segments_from_file(&session.source_root, path).unwrap_or_else(|| vec!["main".to_string()]);
    let modules = match collect_document_modules(path.to_path_buf(), &session, segments.clone(), open_sources) {
        Ok(modules) => modules,
        Err(failure) => {
            push_failures(
                &mut diagnostics,
                uri,
                path,
                source,
                &failure.diagnostics,
                &[],
                &RelatedDeclarationSources::new(),
            );
            return (diagnostics, None);
        }
    };
    let mut facts = None;
    let analysis = session.analyze_unit(&modules, None, &mut |module, checker| {
        if module.file_path == path {
            facts = Some(DocumentFacts::read(module, checker));
        }
    });
    let (Some(facts), Some(document)) = (facts, modules.iter().find(|module| module.file_path == path)) else {
        return (diagnostics, None);
    };
    let mut related = related_declaration_sources(uri, source, &segments, &modules, path);
    extend_package_declaration_sources(&mut related, session.loaves());

    // ---- Diagnostics: the document's own, and a note per imported module with errors ----
    let failures = analysis
        .as_ref()
        .err()
        .map(|failure| failure.diagnostics.as_slice())
        .unwrap_or_default();
    push_failures(
        &mut diagnostics,
        uri,
        path,
        source,
        failures,
        &facts.rust_origin_symbols,
        &related,
    );
    let checked = !failures.iter().any(|failure| Path::new(&failure.file_path) == path);
    for warning in &facts.warnings {
        diagnostics.push(compile_error_to_diagnostic_with_rust_context(
            warning,
            source,
            uri,
            phase_for_typecheck_span(&document.ast, warning.span),
            &facts.rust_origin_symbols,
            &related,
        ));
    }
    let api_metadata_previews = if checked {
        for diagnostic in validate_checked_api_docstrings(std::slice::from_ref(&facts.api_metadata)) {
            diagnostics.push(compile_error_to_diagnostic_with_phase_and_sources(
                &diagnostic.error,
                source,
                uri,
                DiagnosticPhase::Typecheck,
                &related,
            ));
        }
        api_metadata_previews(&document.ast, &facts.api_metadata)
    } else {
        Vec::new()
    };

    let state = DocumentState {
        source: source.to_string(),
        ast: Some(source_ast),
        version,
        checked_identities: CheckedIdentitySnapshot {
            type_info: facts.type_info,
            declaration_sources: related,
        },
        const_types: facts.const_types,
        value_types: facts.value_types,
        rust_origin_symbols: facts.rust_origin_symbols,
        rusttype_info: facts.rusttype_info,
        api_metadata_previews,
        registry_previews: if checked { facts.registry_previews } else { Vec::new() },
        c_binding_previews: if checked { facts.c_binding_previews } else { Vec::new() },
        library_imported_dsl_surfaces: session.library_imported_dsl_surfaces.clone(),
        loaves: Arc::clone(session.loaves()),
        active_features: session.active_features.clone(),
    };
    (diagnostics, Some(state))
}

/// Report collection and checking failures: the document's own with their Rust context, and one note per other
/// module that has errors, which its own analysis reports once it is opened.
fn push_failures(
    diagnostics: &mut Vec<Diagnostic>,
    uri: &Url,
    path: &Path,
    source: &str,
    failures: &[CliDiagnostic],
    rust_origin_symbols: &[RustOriginSymbol],
    related: &RelatedDeclarationSources,
) {
    let mut others = BTreeSet::new();
    for failure in failures {
        if Path::new(&failure.file_path) == path {
            diagnostics.push(compile_error_to_diagnostic_with_rust_context(
                &failure.error,
                source,
                uri,
                failure.phase,
                rust_origin_symbols,
                related,
            ));
        } else {
            others.insert(failure.file_path.clone());
        }
    }
    for other in others {
        diagnostics.push(lsp_root_error_diagnostic(format!(
            "imported module {other} has errors; open it for details"
        )));
    }
}

/// The sources canonical declaration identities project into: the document's buffer and every collected module.
fn related_declaration_sources(
    uri: &Url,
    source: &str,
    segments: &[String],
    modules: &[ParsedModule],
    path: &Path,
) -> RelatedDeclarationSources {
    let mut sources = RelatedDeclarationSources::new();
    sources.insert(
        SymbolOrigin::Module(segments.to_vec()),
        RelatedDeclarationSource {
            uri: uri.clone(),
            source: source.to_string(),
        },
    );
    for module in modules.iter().filter(|module| module.file_path != path) {
        let Ok(uri) = Url::from_file_path(&module.file_path) else {
            continue;
        };
        sources.insert(
            SymbolOrigin::Module(module.path_segments.clone()),
            RelatedDeclarationSource {
                uri,
                source: module.source.clone(),
            },
        );
    }
    sources
}

/// Add the authored sources of the Loaves whose producer checkout is present beside their artifacts.
///
/// A Loaf's artifact directory is `<root>/target/incan/loaf/<profile>` (DD-0007). A Loaf without its sources adds
/// nothing: a declaration span from its manifest is no offset into any file the editor could open.
fn extend_package_declaration_sources(sources: &mut RelatedDeclarationSources, loaves: &LoafIndex) {
    for entry in loaves.entries() {
        let Some(root) = entry.artifact.dir.ancestors().nth(4) else {
            continue;
        };
        let manifest = &entry.artifact.manifest;
        let Some(api) = manifest.contract_metadata.api.as_ref() else {
            continue;
        };
        for module in &api.modules {
            let Some(path) = package_source_path(root, &module.module_path) else {
                continue;
            };
            let (Ok(source), Ok(uri)) = (fs::read_to_string(&path), Url::from_file_path(&path)) else {
                continue;
            };
            sources.insert(
                SymbolOrigin::Package {
                    library: manifest.name.clone(),
                    module_path: module.module_path.clone(),
                },
                RelatedDeclarationSource { uri, source },
            );
        }
    }
}

/// The source file of one Loaf module below its project root, when it is present.
fn package_source_path(root: &Path, module_path: &[String]) -> Option<PathBuf> {
    let joined = module_path
        .iter()
        .fold(root.join("src"), |path, segment| path.join(segment));
    [
        joined.with_extension("incn"),
        joined.join("mod.incn"),
        joined.join("__init__.incn"),
    ]
    .into_iter()
    .find(|candidate| candidate.is_file())
    .map(|path| path.canonicalize().unwrap_or(path))
}
