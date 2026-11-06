//! Codegraph collection entry points, checked analysis of a directory, and diagnostics helpers.

use super::*;

/// Project a compiled library's graph from the exact checking pass and Body IR used by compilation. This boundary
/// performs no collection, type checking or Body IR construction, and records only the owning unit's authored modules.
pub fn collect_checked_unit_codegraph(
    unit: &UnitDescription,
    session: &CompilationSession,
    modules: &[ParsedModule],
    analysis: &CompilationAnalysis,
    bodies: &mut [incan_semantics_core::body_ir::BodyIrModule],
) -> Vec<CodegraphRecord> {
    let _invocation = tracing::trace_span!(target: "incan::invocation", "incan_driver::checked_unit_codegraph", modules = modules.len()).entered();
    let mut builder = checked_unit_builder(unit, session, modules, analysis);
    let mut facts = source_declaration_facts(modules, analysis);
    append_body_declaration_facts(&mut facts, bodies);
    builder.set_checked_declaration_facts(facts);
    for module in modules {
        builder.collect_parsed_module(module, Vec::new());
    }
    builder.finish()
}

/// Project editor buffers from their existing checking pass into the same records as persisted inspection. A failed
/// pass contributes only marked parseable facts and diagnostics; it never retains checked call or reference edges.
pub fn collect_live_unit_codegraph(
    session: &CompilationSession,
    modules: &[ParsedModule],
    analysis: Result<&CompilationAnalysis, &CliDiagnosticFailure>,
) -> Vec<CodegraphRecord> {
    let unit = session.unit();
    let mut builder = match analysis {
        Ok(analysis) => {
            let mut builder = checked_unit_builder(unit, session, modules, analysis);
            builder.set_checked_declaration_facts(checked_declaration_facts(modules, analysis, session));
            builder
        }
        Err(failure) => {
            let mut builder = CodegraphBuilder::new(&unit.source, Some(unit_package(unit)), true);
            builder.semantic_contexts = vec![codegraph_semantic_context(&unit.project_root, session)];
            builder.collect_diagnostics(stable_diagnostics(failure));
            builder
        }
    };
    for module in modules {
        builder.collect_parsed_module_with_degraded(module, Vec::new(), analysis.is_err());
    }
    builder.finish()
}

/// Configure one graph projection from session-owned checked facts, shared by compilation and live navigation.
pub(super) fn checked_unit_builder<'analysis>(
    unit: &UnitDescription,
    session: &CompilationSession,
    modules: &[ParsedModule],
    analysis: &'analysis CompilationAnalysis,
) -> CodegraphBuilder<'analysis> {
    let mut builder = CodegraphBuilder::new(&unit.source, Some(unit_package(unit)), false);
    builder.semantic_contexts = vec![codegraph_semantic_context(&unit.project_root, session)];
    builder.set_semantic_snapshots(Cow::Borrowed(analysis.semantic_snapshots()));
    builder.set_registry_metadata(checked_registry_metadata_by_path(analysis, modules, &unit.package));
    builder.set_capabilities(checked_capabilities_by_path(analysis, modules));
    builder.set_c_abi_artifacts(checked_c_abi_by_path(analysis, modules));
    builder.seed_canonical_target_ids(modules);
    builder
}

/// Preserve the unit's declared package identity for every consumer of its graph.
pub(super) fn unit_package(unit: &UnitDescription) -> CodegraphPackage {
    CodegraphPackage {
        name: Some(unit.package.clone()),
        version: Some(unit.version.clone()),
        root_path: Some(path_string(&unit.project_root)),
    }
}

/// Export checked facts and, for a library entry, its checked interface from one analysis pass. The interface has
/// no executable or native artifact claim and is intended only for checking downstream inspection units.
pub fn collect_codegraph_with_interface(
    path: &Path,
    units: &[UnitDescription],
    allow_errors: bool,
    rust: Option<&crate::session::RustFactsSetup>,
) -> CodegraphResult<(Vec<CodegraphRecord>, Option<LibraryManifest>)> {
    let mut sessions = UnitSessions::new(units);
    sessions.rust = rust;
    let package = package_identity(&sessions, path);
    let package_name = package.as_ref().and_then(|package| package.name.clone());
    let mut builder = CodegraphBuilder::new(path, package, allow_errors);
    let mut interface = None;
    builder.semantic_contexts = collect_codegraph_semantic_contexts(&mut sessions, allow_errors)?;

    if path.is_dir() {
        let files = discover_incan_files(path)?;
        let (modules, analysis) = directory_modules_diagnostics_and_info(&files, &mut sessions)?;
        if !analysis.diagnostics.is_empty() && !allow_errors {
            return Err(CodegraphError::failure(render_diagnostics(&analysis.diagnostics)));
        }
        if analysis.diagnostics.is_empty() {
            builder.set_semantic_snapshots(Cow::Owned(analysis.semantic_snapshots_by_path));
            builder.set_checked_declaration_facts(analysis.declaration_facts);
            builder.set_registry_metadata(analysis.registry_metadata_by_path);
            builder.set_capabilities(analysis.capabilities_by_path);
            builder.set_c_abi_artifacts(analysis.c_abi_by_path);
            builder.seed_canonical_target_ids(&modules);
            for module in &modules {
                builder.collect_parsed_module(module, Vec::new());
            }
        } else {
            let parsed_paths = modules
                .iter()
                .map(|module| module.file_path.clone())
                .collect::<BTreeSet<_>>();
            for module in &modules {
                // A directory scan collects each file as its own entrypoint, so every module arrives seeded as
                // `main`. A degraded scan used to discard these modules and re-read every file through the tolerant
                // path, which names a module by where it sits; keeping the parsed facts has to keep that naming too,
                // or every record in a degraded scan claims to be `main`.
                let module = ParsedModule {
                    path_segments: builder.fallback_module_segments(&module.file_path),
                    ..module.clone()
                };
                builder.collect_parsed_module_with_degraded(
                    &module,
                    diagnostics_for_file(&analysis.diagnostics, &module.file_path),
                    true,
                );
            }
            for file in files.iter().filter(|file| !parsed_paths.contains(*file)) {
                let session = sessions.for_file(file)?;
                builder.collect_tolerant_file_with_session(file, session)?;
            }
        }
        builder.collect_diagnostics(analysis.diagnostics);
        if !allow_errors && builder.has_diagnostics() {
            return Err(CodegraphError::failure(render_diagnostics(builder.diagnostics())));
        }
    } else {
        let session = sessions.for_file(path)?;
        let modules = if session.unit().kind == UnitKind::Library {
            collect_library_modules_detailed_with_session(path.to_path_buf(), session)
        } else {
            collect_modules_detailed_with_session(path.to_path_buf(), session)
        };
        match modules {
            Ok(modules) => {
                session.resolve_rust_facts(&modules)?;
                let analysis = typecheck_diagnostics_and_info(session, &modules, package_name.as_deref())?;
                if !analysis.diagnostics.is_empty() && !allow_errors {
                    return Err(CodegraphError::failure(render_diagnostics(&analysis.diagnostics)));
                }
                interface = analysis.interface;
                builder.set_semantic_snapshots(Cow::Owned(analysis.semantic_snapshots_by_path));
                builder.set_checked_declaration_facts(analysis.declaration_facts);
                builder.set_registry_metadata(analysis.registry_metadata_by_path);
                builder.set_capabilities(analysis.capabilities_by_path);
                builder.set_c_abi_artifacts(analysis.c_abi_by_path);
                builder.seed_canonical_target_ids(&modules);
                for module in &modules {
                    builder.collect_parsed_module_with_degraded(
                        module,
                        diagnostics_for_file(&analysis.diagnostics, &module.file_path),
                        !analysis.diagnostics.is_empty(),
                    );
                }
                builder.collect_diagnostics(analysis.diagnostics);
            }
            Err(failure) if allow_errors => {
                builder.collect_tolerant_failure(path, failure, session)?;
            }
            Err(failure) => return Err(CodegraphError::failure(failure.render_human())),
        }
    }

    Ok((builder.finish(), interface))
}

/// Collect and typecheck every discovered directory source root, keeping semantic artifacts only when the whole
/// directory graph is clean.
pub(super) fn directory_modules_diagnostics_and_info(
    files: &[PathBuf],
    sessions: &mut UnitSessions<'_>,
) -> CodegraphResult<DirectoryTypecheckArtifacts> {
    let file_set = files.iter().cloned().collect::<BTreeSet<_>>();
    let mut modules_by_path = BTreeMap::new();
    let mut diagnostics = Vec::new();
    let mut semantic_snapshots_by_path = BTreeMap::new();
    let mut declaration_facts = BTreeMap::new();
    let mut registry_metadata_by_path = BTreeMap::new();
    let mut capabilities_by_path = BTreeMap::new();
    let mut c_abi_by_path = BTreeMap::new();

    for file in files {
        let session = sessions.for_file(file)?;
        let collected = logical_module_segments_from_file(&session.source_root, file).map_or_else(
            || collect_modules_detailed_with_session(file.clone(), session),
            |root_module_path| collect_modules_detailed_with_session_at_path(file.clone(), session, root_module_path),
        );
        match collected {
            Ok(modules) => {
                session.resolve_rust_facts(&modules)?;
                for module in &modules {
                    if file_set.contains(&module.file_path) {
                        modules_by_path
                            .entry(module.file_path.clone())
                            .or_insert_with(|| module.clone());
                    }
                }
                match session.analyze_modules(&modules) {
                    Ok(analysis) => {
                        for (path, snapshot) in analysis.semantic_snapshots() {
                            retain_root_analysis(&mut semantic_snapshots_by_path, file, path, snapshot.clone());
                        }
                        let package_name = session.unit().package.clone();
                        for (path, metadata) in checked_registry_metadata_by_path(&analysis, &modules, &package_name) {
                            retain_root_analysis(&mut registry_metadata_by_path, file, &path, metadata);
                        }
                        for (path, declarations) in checked_capabilities_by_path(&analysis, &modules) {
                            retain_root_analysis(&mut capabilities_by_path, file, &path, declarations);
                        }
                        for (path, c_abi) in checked_c_abi_by_path(&analysis, &modules) {
                            retain_root_analysis(&mut c_abi_by_path, file, &path, c_abi);
                        }
                        // Header identities stay independent of optional executable digests. Only graph inspection
                        // pays for Body IR here; unavailable checked facts never acquire a portable signature.
                        for (identity, signature) in checked_declaration_facts(&modules, &analysis, session) {
                            declaration_facts.entry(identity).or_insert(signature);
                        }
                    }
                    Err(failure) => {
                        dedup_diagnostics(&mut diagnostics, stable_diagnostics(&failure));
                    }
                }
            }
            Err(failure) => {
                dedup_diagnostics(&mut diagnostics, stable_diagnostics(&failure));
            }
        }
    }

    let has_diagnostics = !diagnostics.is_empty();
    Ok((
        modules_by_path.into_values().collect(),
        CheckedCodegraphAnalysis {
            interface: None,
            diagnostics,
            semantic_snapshots_by_path: if has_diagnostics {
                BTreeMap::new()
            } else {
                semantic_snapshots_by_path
            },
            declaration_facts: if has_diagnostics {
                BTreeMap::new()
            } else {
                declaration_facts
            },
            registry_metadata_by_path: if has_diagnostics {
                BTreeMap::new()
            } else {
                registry_metadata_by_path
            },
            capabilities_by_path: if has_diagnostics {
                BTreeMap::new()
            } else {
                capabilities_by_path
            },
            c_abi_by_path: if has_diagnostics {
                BTreeMap::new()
            } else {
                c_abi_by_path
            },
        },
    ))
}

/// Retain each file's own analysis over any copy first encountered as another root's dependency.
pub(super) fn retain_root_analysis<T>(analyses: &mut BTreeMap<PathBuf, T>, root: &Path, path: &Path, analysis: T) {
    if path == root {
        analyses.insert(path.to_path_buf(), analysis);
    } else {
        analyses.entry(path.to_path_buf()).or_insert(analysis);
    }
}

/// Run typechecking and keep reusable semantic artifacts when the checked graph succeeds.
pub(super) fn typecheck_diagnostics_and_info(
    session: &CompilationSession,
    modules: &[ParsedModule],
    package_name: Option<&str>,
) -> CodegraphResult<CheckedCodegraphAnalysis> {
    let library = session.unit().kind == UnitKind::Library;
    let mut publication = library.then(|| PublicationFacts::new(&session.unit().package));
    let analysis = session.analyze_unit(
        modules,
        library.then_some(session.unit().package.as_str()),
        &mut |module, checker| {
            if let Some(publication) = publication.as_mut() {
                publication.visit(module, checker, module.file_path == session.unit().source);
            }
        },
    );
    let interface = if analysis.is_ok() {
        publication
            .map(|facts| {
                let entry = modules
                    .iter()
                    .find(|module| module.file_path == session.unit().source)
                    .ok_or_else(|| CodegraphError::failure("library entry module is missing"))?;
                facts
                    .library_manifest(modules, entry, session.unit(), &session.contract_model_bundles)
                    .map_err(CodegraphError::from)
            })
            .transpose()?
    } else {
        None
    };
    match analysis {
        Ok(analysis) => Ok(CheckedCodegraphAnalysis {
            interface,
            diagnostics: Vec::new(),
            semantic_snapshots_by_path: analysis.semantic_snapshots().clone(),
            declaration_facts: checked_declaration_facts(modules, &analysis, session),
            registry_metadata_by_path: checked_registry_metadata_by_path(
                &analysis,
                modules,
                package_name.unwrap_or("<unpackaged>"),
            ),
            capabilities_by_path: checked_capabilities_by_path(&analysis, modules),
            c_abi_by_path: checked_c_abi_by_path(&analysis, modules),
        }),
        Err(failure) => Ok(CheckedCodegraphAnalysis {
            interface: None,
            diagnostics: stable_diagnostics(&failure),
            semantic_snapshots_by_path: BTreeMap::new(),
            declaration_facts: BTreeMap::new(),
            registry_metadata_by_path: BTreeMap::new(),
            capabilities_by_path: BTreeMap::new(),
            c_abi_by_path: BTreeMap::new(),
        }),
    }
}

/// Build the portable registry projection from the same session analysis used by codegraph body facts.
pub(super) fn checked_registry_metadata_by_path(
    analysis: &CompilationAnalysis,
    modules: &[ParsedModule],
    package_name: &str,
) -> BTreeMap<PathBuf, CheckedRegistryMetadataModule> {
    modules
        .iter()
        .filter_map(|module| {
            analysis
                .type_info_for_module_path(&module.path_segments)
                .map(|type_info| {
                    (
                        module.file_path.clone(),
                        collect_checked_registry_metadata(type_info, module.path_segments.clone(), package_name),
                    )
                })
        })
        .collect()
}

/// Retain the compilation's checked RFC 104 capability declarations for the codegraph projection.
///
/// Ordered by canonical identity so an export is byte-stable across runs: the typechecker keys these by identity in a
/// `BTreeMap`, and preserving that order avoids a second sort with a different tiebreak.
pub(super) fn checked_capabilities_by_path(
    analysis: &CompilationAnalysis,
    modules: &[ParsedModule],
) -> BTreeMap<PathBuf, Vec<CapabilityDeclarationInfo>> {
    modules
        .iter()
        .filter_map(|module| {
            analysis
                .type_info_for_module_path(&module.path_segments)
                .map(|type_info| {
                    (
                        module.file_path.clone(),
                        type_info.declarations.capabilities.values().cloned().collect(),
                    )
                })
        })
        .collect()
}

/// Retain the successful typechecker's C ABI artifacts for the codegraph projection without a second analysis pass.
pub(super) fn checked_c_abi_by_path(
    analysis: &CompilationAnalysis,
    modules: &[ParsedModule],
) -> BTreeMap<PathBuf, CAbiInteropArtifacts> {
    modules
        .iter()
        .filter_map(|module| {
            analysis
                .type_info_for_module_path(&module.path_segments)
                .map(|type_info| (module.file_path.clone(), type_info.c_abi.clone()))
        })
        .collect()
}

/// Convert shared CLI diagnostic failures into the public diagnostic projection used by both `incan check` and
/// codegraph records.
pub(super) fn stable_diagnostics(failure: &CliDiagnosticFailure) -> Vec<StableDiagnostic> {
    crate::check::stable_diagnostics_from(&failure.diagnostics)
}

/// Append diagnostics while suppressing duplicate records produced by overlapping directory entrypoint checks.
pub(super) fn dedup_diagnostics(target: &mut Vec<StableDiagnostic>, diagnostics: Vec<StableDiagnostic>) {
    for diagnostic in diagnostics {
        if !target.contains(&diagnostic) {
            target.push(diagnostic);
        }
    }
}

/// Return the diagnostics whose primary span belongs to one parsed module file.
pub(super) fn diagnostics_for_file(diagnostics: &[StableDiagnostic], file_path: &Path) -> Vec<StableDiagnostic> {
    let file_path = file_path.to_string_lossy();
    diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.primary_span.file == file_path)
        .cloned()
        .collect()
}

/// Render a compact strict-mode failure summary without duplicating the full human diagnostic renderer in
/// JSONL-specific code.
pub(super) fn render_diagnostics(diagnostics: &[StableDiagnostic]) -> String {
    let mut output = String::from("codegraph export failed because the checked graph has diagnostics");
    for diagnostic in diagnostics {
        output.push_str("\n- ");
        output.push_str(diagnostic.code);
        output.push_str(": ");
        output.push_str(&diagnostic.message);
    }
    output
}

/// Discover `.incn` files below a directory in deterministic path order.
pub(super) fn discover_incan_files(root: &Path) -> CodegraphResult<Vec<PathBuf>> {
    let mut files = Vec::new();
    crate::sources::collect_incan_source_files(root, &mut files)
        .map_err(|error| CodegraphError::failure(format!("cannot discover {}: {error}", root.display())))?;
    files.sort();
    Ok(files)
}

/// Project the owning unit's package so graph headers agree with compilation.
pub(super) fn package_identity(sessions: &UnitSessions<'_>, path: &Path) -> Option<CodegraphPackage> {
    sessions.unit_for(path).ok().map(unit_package)
}

/// Build the Loaf header projection from the same sessions used by codegraph checking: one per described unit.
pub(super) fn collect_codegraph_semantic_contexts(
    sessions: &mut UnitSessions<'_>,
    allow_errors: bool,
) -> CodegraphResult<Vec<CodegraphSemanticContext>> {
    let mut contexts = Vec::new();
    for unit in sessions.units {
        let session = match sessions.for_file(&unit.source) {
            Ok(session) => session,
            Err(_) if allow_errors => continue,
            Err(error) => return Err(error),
        };
        contexts.push(codegraph_semantic_context(&unit.project_root, session));
    }
    Ok(contexts)
}

/// Project the Loaves a session indexed into the storage-agnostic codegraph wire contract.
pub(super) fn codegraph_semantic_context(
    project_root: &Path,
    session: &CompilationSession,
) -> CodegraphSemanticContext {
    CodegraphSemanticContext {
        project_root: path_string(project_root),
        loaves: session
            .loaves()
            .entries()
            .map(|entry| CodegraphLoafProjection {
                key: entry.key.clone(),
                name: entry.artifact.identity.name.clone(),
                version: entry.artifact.identity.version.clone(),
                digest: entry.artifact.identity.digest.clone(),
                standard: matches!(
                    entry.namespace,
                    incan_frontend::loaf_index::LoafNamespace::Standard { .. }
                ),
            })
            .collect(),
    }
}
