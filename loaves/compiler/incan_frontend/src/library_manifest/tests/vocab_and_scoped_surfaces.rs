//! Vocabulary payloads: scoped surface and symbol descriptors round-trip, and the writer rejects empty, ambiguous or
//! malformed descriptors, helper bindings to unknown or uncallable exports, and non-normalized desugarer metadata.

use super::*;

#[test]
fn manifest_io_round_trip_preserves_vocab_payload() -> Result<(), Box<dyn std::error::Error>> {
    let mut manifest = LibraryManifest::new("mylib", "0.1.0");
    manifest.vocab = Some(VocabExports {
        crate_path: "crates/mylib_vocab".to_string(),
        package_name: "mylib_vocab".to_string(),
        keyword_registrations: vec![incan_vocab::KeywordRegistration {
            activation: incan_vocab::KeywordActivation::OnImport {
                namespace: "mylib.dsl".to_string(),
            },
            keywords: vec![incan_vocab::KeywordSpec::new(
                "await",
                incan_vocab::KeywordSurfaceKind::ControlFlow,
            )],
            valid_decorators: vec!["route".to_string()],
        }],
        dsl_surfaces: Vec::new(),
        provider_manifest: incan_vocab::LibraryManifest::default(),
        desugarer_artifact: None,
    });
    manifest.soft_keywords.activations = vec![SoftKeywordActivation {
        namespace: "mylib.dsl".to_string(),
        keyword: "await".to_string(),
    }];

    let tmp = tempfile::tempdir()?;
    let path = tmp.path().join("mylib.incnlib");
    manifest.write_to_path(&path)?;
    let loaded = LibraryManifest::read_from_path(&path)?;

    assert_eq!(loaded, manifest);
    Ok(())
}

#[test]
fn manifest_io_round_trip_preserves_scoped_surface_descriptors() -> Result<(), Box<dyn std::error::Error>> {
    let mut manifest = LibraryManifest::new("mylib", "0.1.0");
    manifest.vocab = Some(VocabExports {
        crate_path: "crates/mylib_vocab".to_string(),
        package_name: "mylib_vocab".to_string(),
        keyword_registrations: Vec::new(),
        dsl_surfaces: vec![
            incan_vocab::DslSurface::on_import("mylib.query")
                .with_declaration(
                    incan_vocab::DeclarationSurface::named("query")
                        .with_clause_body()
                        .desugars_to_expression()
                        .with_clauses([
                            incan_vocab::ClauseSurface::expr("FROM").required(),
                            incan_vocab::ClauseSurface::expr_list("SELECT").required().after("FROM"),
                        ]),
                )
                .with_scoped_surfaces([
                    incan_vocab::ScopedSurfaceDescriptor::operator("query.pipe", "|>")
                        .in_clause_body("query", "SELECT")
                        .with_misuse_scope(incan_vocab::ScopedSurfaceMisuseScope::ActivatingFile)
                        .with_diagnostic(incan_vocab::ScopedSurfaceDiagnosticTemplate::new(
                            "query-pipe-outside-scope",
                            incan_vocab::ScopedSurfaceDiagnosticKind::OutsideScope,
                            "`|>` is only valid inside query SELECT clauses",
                        ))
                        .pairwise_chain(),
                    incan_vocab::ScopedSurfaceDescriptor::leading_dot_path("query.field")
                        .in_clause_body("query", "SELECT")
                        .with_receiver(incan_vocab::ScopedSurfaceReceiver::clause("FROM")),
                    incan_vocab::ScopedSurfaceDescriptor::leading_dot_path("query.arg_field")
                        .with_eligibilities([
                            incan_vocab::ScopedSurfaceEligibility::call_argument("query", "filter"),
                            incan_vocab::ScopedSurfaceEligibility::call_argument("query", "select"),
                        ])
                        .with_receiver(incan_vocab::ScopedSurfaceReceiver::custom("method-receiver")),
                ]),
        ],
        provider_manifest: incan_vocab::LibraryManifest::default(),
        desugarer_artifact: None,
    });

    let tmp = tempfile::tempdir()?;
    let path = tmp.path().join("mylib.incnlib");
    manifest.write_to_path(&path)?;
    let loaded = LibraryManifest::read_from_path(&path)?;

    let Some(loaded_vocab) = loaded.vocab.as_ref() else {
        return Err("expected vocab payload to round-trip".into());
    };
    let scoped_surfaces = &loaded_vocab.dsl_surfaces[0].scoped_surfaces;
    assert_eq!(loaded, manifest);
    assert_eq!(scoped_surfaces.len(), 3);
    assert_eq!(
        scoped_surfaces[0].format_hint.chain_mode,
        incan_vocab::ScopedSurfaceChainMode::Pairwise
    );
    assert_eq!(
        scoped_surfaces[1].receiver,
        Some(incan_vocab::ScopedSurfaceReceiver::clause("FROM"))
    );
    assert_eq!(scoped_surfaces[2].eligible_in[0].call.as_deref(), Some("filter"));
    assert_eq!(
        scoped_surfaces[2].receiver,
        Some(incan_vocab::ScopedSurfaceReceiver::custom("method-receiver"))
    );
    Ok(())
}

#[test]
fn manifest_io_round_trip_preserves_scoped_symbol_descriptors() -> Result<(), Box<dyn std::error::Error>> {
    let mut manifest = LibraryManifest::new("mylib", "0.1.0");
    manifest.vocab = Some(VocabExports {
        crate_path: "crates/mylib_vocab".to_string(),
        package_name: "mylib_vocab".to_string(),
        keyword_registrations: Vec::new(),
        dsl_surfaces: vec![
            incan_vocab::DslSurface::on_import("mylib.query")
                .with_declaration(
                    incan_vocab::DeclarationSurface::named("query")
                        .with_clause_body()
                        .desugars_to_expression()
                        .with_clauses([
                            incan_vocab::ClauseSurface::expr("FROM").required(),
                            incan_vocab::ClauseSurface::expr_list("SELECT").required().after("FROM"),
                        ]),
                )
                .with_scoped_symbols([
                    incan_vocab::ScopedSymbolDescriptor::aggregate("query.sum", "sum")
                        .in_clause_body("query", "SELECT")
                        .with_role(
                            incan_vocab::ScopedSymbolRoleMetadata::new("aggregate.sum")
                                .with_label("Sum")
                                .with_description("Sum aggregate"),
                        )
                        .with_misuse_scope(incan_vocab::ScopedSymbolMisuseScope::ActiveDsl)
                        .with_diagnostic(incan_vocab::ScopedSymbolDiagnosticTemplate::new(
                            "query-sum-outside-select",
                            incan_vocab::ScopedSymbolDiagnosticKind::OutsideEligiblePosition,
                            "`sum` is only a query aggregate inside SELECT clauses",
                        )),
                    incan_vocab::ScopedSymbolDescriptor::aggregate("query.count", "count").with_eligibilities([
                        incan_vocab::ScopedSymbolEligibility::clause_body("query", "SELECT"),
                        incan_vocab::ScopedSymbolEligibility::call_argument("query", "window"),
                    ]),
                ]),
        ],
        provider_manifest: incan_vocab::LibraryManifest::default(),
        desugarer_artifact: None,
    });

    let tmp = tempfile::tempdir()?;
    let path = tmp.path().join("mylib.incnlib");
    manifest.write_to_path(&path)?;
    let loaded = LibraryManifest::read_from_path(&path)?;

    let Some(loaded_vocab) = loaded.vocab.as_ref() else {
        return Err("expected vocab payload to round-trip".into());
    };
    let scoped_symbols = &loaded_vocab.dsl_surfaces[0].scoped_symbols;
    assert_eq!(loaded, manifest);
    assert_eq!(scoped_symbols.len(), 2);
    assert_eq!(scoped_symbols[0].symbol, "sum");
    assert_eq!(scoped_symbols[0].family, incan_vocab::ScopedSymbolFamily::AggregateLike);
    assert_eq!(
        scoped_symbols[0].role.as_ref().map(|role| role.key.as_str()),
        Some("aggregate.sum")
    );
    assert_eq!(scoped_symbols[1].eligible_in[1].call.as_deref(), Some("window"));
    assert_eq!(
        scoped_symbols[0].diagnostics[0].kind,
        incan_vocab::ScopedSymbolDiagnosticKind::OutsideEligiblePosition
    );
    Ok(())
}

#[test]
fn manifest_writer_rejects_empty_scoped_symbol_descriptor_key() -> Result<(), Box<dyn std::error::Error>> {
    let mut manifest = LibraryManifest::new("mylib", "0.1.0");
    manifest.vocab = Some(VocabExports {
        crate_path: "crates/mylib_vocab".to_string(),
        package_name: "mylib_vocab".to_string(),
        keyword_registrations: Vec::new(),
        dsl_surfaces: vec![
            incan_vocab::DslSurface::on_import("mylib.query")
                .with_declaration(
                    incan_vocab::DeclarationSurface::named("query")
                        .with_clause(incan_vocab::ClauseSurface::expr("SELECT")),
                )
                .with_scoped_symbol(
                    incan_vocab::ScopedSymbolDescriptor::aggregate("", "sum").in_clause_body("query", "SELECT"),
                ),
        ],
        provider_manifest: incan_vocab::LibraryManifest::default(),
        desugarer_artifact: None,
    });

    let tmp = tempfile::tempdir()?;
    let err = manifest.write_to_path(&tmp.path().join("mylib.incnlib"));
    assert!(matches!(
        err,
        Err(LibraryManifestError::Invalid(msg)) if msg.contains("vocab scoped symbol descriptor key cannot be empty")
    ));
    Ok(())
}

#[test]
fn manifest_writer_rejects_empty_scoped_symbol_spelling() -> Result<(), Box<dyn std::error::Error>> {
    let mut manifest = LibraryManifest::new("mylib", "0.1.0");
    manifest.vocab = Some(VocabExports {
        crate_path: "crates/mylib_vocab".to_string(),
        package_name: "mylib_vocab".to_string(),
        keyword_registrations: Vec::new(),
        dsl_surfaces: vec![
            incan_vocab::DslSurface::on_import("mylib.query")
                .with_declaration(
                    incan_vocab::DeclarationSurface::named("query")
                        .with_clause(incan_vocab::ClauseSurface::expr("SELECT")),
                )
                .with_scoped_symbol(
                    incan_vocab::ScopedSymbolDescriptor::aggregate("query.sum", "").in_clause_body("query", "SELECT"),
                ),
        ],
        provider_manifest: incan_vocab::LibraryManifest::default(),
        desugarer_artifact: None,
    });

    let tmp = tempfile::tempdir()?;
    let err = manifest.write_to_path(&tmp.path().join("mylib.incnlib"));
    assert!(matches!(
        err,
        Err(LibraryManifestError::Invalid(msg)) if msg.contains("symbol cannot be empty")
    ));
    Ok(())
}

#[test]
fn manifest_writer_rejects_hard_keyword_scoped_symbol_spelling() -> Result<(), Box<dyn std::error::Error>> {
    let mut manifest = LibraryManifest::new("mylib", "0.1.0");
    manifest.vocab = Some(VocabExports {
        crate_path: "crates/mylib_vocab".to_string(),
        package_name: "mylib_vocab".to_string(),
        keyword_registrations: Vec::new(),
        dsl_surfaces: vec![
            incan_vocab::DslSurface::on_import("mylib.query")
                .with_declaration(
                    incan_vocab::DeclarationSurface::named("query")
                        .with_clause(incan_vocab::ClauseSurface::expr("SELECT")),
                )
                .with_scoped_symbol(
                    incan_vocab::ScopedSymbolDescriptor::function("query.from", "from")
                        .in_clause_body("query", "SELECT"),
                ),
        ],
        provider_manifest: incan_vocab::LibraryManifest::default(),
        desugarer_artifact: None,
    });

    let tmp = tempfile::tempdir()?;
    let err = manifest.write_to_path(&tmp.path().join("mylib.incnlib"));
    assert!(matches!(
        err,
        Err(LibraryManifestError::Invalid(msg)) if msg.contains("cannot be a hard keyword")
    ));
    Ok(())
}

#[test]
fn manifest_writer_rejects_malformed_scoped_symbol_eligibility() -> Result<(), Box<dyn std::error::Error>> {
    let mut manifest = LibraryManifest::new("mylib", "0.1.0");
    manifest.vocab = Some(VocabExports {
        crate_path: "crates/mylib_vocab".to_string(),
        package_name: "mylib_vocab".to_string(),
        keyword_registrations: Vec::new(),
        dsl_surfaces: vec![
            incan_vocab::DslSurface::on_import("mylib.query")
                .with_declaration(
                    incan_vocab::DeclarationSurface::named("query")
                        .with_clause(incan_vocab::ClauseSurface::expr("SELECT")),
                )
                .with_scoped_symbol(
                    incan_vocab::ScopedSymbolDescriptor::aggregate("query.sum", "sum").with_eligibility(
                        incan_vocab::ScopedSymbolEligibility {
                            declaration: "query".to_string(),
                            clause: None,
                            call: None,
                            position: incan_vocab::ScopedSymbolPosition::ClauseBody,
                        },
                    ),
                ),
        ],
        provider_manifest: incan_vocab::LibraryManifest::default(),
        desugarer_artifact: None,
    });

    let tmp = tempfile::tempdir()?;
    let err = manifest.write_to_path(&tmp.path().join("mylib.incnlib"));
    assert!(matches!(
        err,
        Err(LibraryManifestError::Invalid(msg)) if msg.contains("clause-body eligibility must declare a clause")
    ));
    Ok(())
}

#[test]
fn manifest_writer_rejects_ambiguous_scoped_symbol_descriptors() -> Result<(), Box<dyn std::error::Error>> {
    let mut manifest = LibraryManifest::new("mylib", "0.1.0");
    let query_surface = incan_vocab::DslSurface::on_import("mylib.query")
        .with_declaration(
            incan_vocab::DeclarationSurface::named("query").with_clause(incan_vocab::ClauseSurface::expr("SELECT")),
        )
        .with_scoped_symbols([
            incan_vocab::ScopedSymbolDescriptor::aggregate("query.sum.primary", "sum")
                .in_clause_body("query", "SELECT"),
            incan_vocab::ScopedSymbolDescriptor::function("query.sum.secondary", "sum")
                .in_clause_body("query", "SELECT"),
        ]);
    manifest.vocab = Some(VocabExports {
        crate_path: "crates/mylib_vocab".to_string(),
        package_name: "mylib_vocab".to_string(),
        keyword_registrations: Vec::new(),
        dsl_surfaces: vec![query_surface],
        provider_manifest: incan_vocab::LibraryManifest::default(),
        desugarer_artifact: None,
    });

    let tmp = tempfile::tempdir()?;
    let err = manifest.write_to_path(&tmp.path().join("mylib.incnlib"));
    assert!(matches!(
        err,
        Err(LibraryManifestError::Invalid(msg)) if msg.contains("ambiguous scoped symbol descriptor")
    ));
    Ok(())
}

#[test]
fn manifest_writer_rejects_malformed_scoped_symbol_diagnostics() -> Result<(), Box<dyn std::error::Error>> {
    let mut manifest = LibraryManifest::new("mylib", "0.1.0");
    manifest.vocab = Some(VocabExports {
        crate_path: "crates/mylib_vocab".to_string(),
        package_name: "mylib_vocab".to_string(),
        keyword_registrations: Vec::new(),
        dsl_surfaces: vec![
            incan_vocab::DslSurface::on_import("mylib.query")
                .with_declaration(
                    incan_vocab::DeclarationSurface::named("query")
                        .with_clause(incan_vocab::ClauseSurface::expr("SELECT")),
                )
                .with_scoped_symbol(
                    incan_vocab::ScopedSymbolDescriptor::aggregate("query.sum", "sum")
                        .in_clause_body("query", "SELECT")
                        .with_diagnostic(incan_vocab::ScopedSymbolDiagnosticTemplate::new(
                            "query-sum-outside-select",
                            incan_vocab::ScopedSymbolDiagnosticKind::OutsideEligiblePosition,
                            "`sum` is only valid inside SELECT",
                        ))
                        .with_diagnostic(incan_vocab::ScopedSymbolDiagnosticTemplate::new(
                            "query-sum-outside-select",
                            incan_vocab::ScopedSymbolDiagnosticKind::AmbiguousResolution,
                            "use an explicit qualifier to disambiguate `sum`",
                        )),
                ),
        ],
        provider_manifest: incan_vocab::LibraryManifest::default(),
        desugarer_artifact: None,
    });

    let tmp = tempfile::tempdir()?;
    let err = manifest.write_to_path(&tmp.path().join("mylib.incnlib"));
    assert!(matches!(
        err,
        Err(LibraryManifestError::Invalid(msg)) if msg.contains("contains duplicate diagnostic code")
    ));
    Ok(())
}

#[test]
fn manifest_writer_rejects_ambiguous_scoped_surface_descriptors() -> Result<(), Box<dyn std::error::Error>> {
    let mut manifest = LibraryManifest::new("mylib", "0.1.0");
    let query_surface = incan_vocab::DslSurface::on_import("mylib.query")
        .with_declaration(
            incan_vocab::DeclarationSurface::named("query").with_clause(incan_vocab::ClauseSurface::expr("SELECT")),
        )
        .with_scoped_surfaces([
            incan_vocab::ScopedSurfaceDescriptor::operator("query.pipe.primary", "|>")
                .in_clause_body("query", "SELECT"),
            incan_vocab::ScopedSurfaceDescriptor::operator("query.pipe.secondary", "|>")
                .in_clause_body("query", "SELECT"),
        ]);
    manifest.vocab = Some(VocabExports {
        crate_path: "crates/mylib_vocab".to_string(),
        package_name: "mylib_vocab".to_string(),
        keyword_registrations: Vec::new(),
        dsl_surfaces: vec![query_surface],
        provider_manifest: incan_vocab::LibraryManifest::default(),
        desugarer_artifact: None,
    });

    let tmp = tempfile::tempdir()?;
    let err = manifest.write_to_path(&tmp.path().join("mylib.incnlib"));
    assert!(matches!(
        err,
        Err(LibraryManifestError::Invalid(msg)) if msg.contains("ambiguous scoped surface descriptor")
    ));
    Ok(())
}

#[test]
fn manifest_writer_rejects_expression_form_without_receiver() -> Result<(), Box<dyn std::error::Error>> {
    let mut manifest = LibraryManifest::new("mylib", "0.1.0");
    manifest.vocab = Some(VocabExports {
        crate_path: "crates/mylib_vocab".to_string(),
        package_name: "mylib_vocab".to_string(),
        keyword_registrations: Vec::new(),
        dsl_surfaces: vec![
            incan_vocab::DslSurface::on_import("mylib.query")
                .with_declaration(
                    incan_vocab::DeclarationSurface::named("query")
                        .with_clause(incan_vocab::ClauseSurface::expr("SELECT")),
                )
                .with_scoped_surface(
                    incan_vocab::ScopedSurfaceDescriptor::leading_dot_path("query.field")
                        .in_clause_body("query", "SELECT"),
                ),
        ],
        provider_manifest: incan_vocab::LibraryManifest::default(),
        desugarer_artifact: None,
    });

    let tmp = tempfile::tempdir()?;
    let err = manifest.write_to_path(&tmp.path().join("mylib.incnlib"));
    assert!(matches!(
        err,
        Err(LibraryManifestError::Invalid(msg)) if msg.contains("must declare receiver derivation")
    ));
    Ok(())
}

#[test]
fn manifest_writer_rejects_declaration_head_scoped_surface_position() -> Result<(), Box<dyn std::error::Error>> {
    let mut manifest = LibraryManifest::new("mylib", "0.1.0");
    manifest.vocab = Some(VocabExports {
        crate_path: "crates/mylib_vocab".to_string(),
        package_name: "mylib_vocab".to_string(),
        keyword_registrations: Vec::new(),
        dsl_surfaces: vec![
            incan_vocab::DslSurface::on_import("mylib.query")
                .with_declaration(incan_vocab::DeclarationSurface::named("query"))
                .with_scoped_surface(
                    incan_vocab::ScopedSurfaceDescriptor::operator("query.pipe", "|>")
                        .with_eligibility(incan_vocab::ScopedSurfaceEligibility::declaration_head("query")),
                ),
        ],
        provider_manifest: incan_vocab::LibraryManifest::default(),
        desugarer_artifact: None,
    });

    let tmp = tempfile::tempdir()?;
    let err = manifest.write_to_path(&tmp.path().join("mylib.incnlib"));
    assert!(matches!(
        err,
        Err(LibraryManifestError::Invalid(msg)) if msg.contains("declaration-head eligibility is not supported yet")
    ));
    Ok(())
}

#[test]
fn manifest_writer_rejects_helper_binding_to_unknown_export() -> Result<(), Box<dyn std::error::Error>> {
    let mut manifest = LibraryManifest::new("mylib", "0.1.0");
    manifest.vocab = Some(VocabExports {
        crate_path: "crates/mylib_vocab".to_string(),
        package_name: "mylib_vocab".to_string(),
        keyword_registrations: Vec::new(),
        dsl_surfaces: Vec::new(),
        provider_manifest: incan_vocab::LibraryManifest {
            helper_bindings: vec![incan_vocab::HelperBinding {
                key: "filter".to_string(),
                exported_name: "filter".to_string(),
            }],
            ..incan_vocab::LibraryManifest::default()
        },
        desugarer_artifact: None,
    });

    let tmp = tempfile::tempdir()?;
    let err = manifest.write_to_path(&tmp.path().join("mylib.incnlib"));
    assert!(matches!(err, Err(LibraryManifestError::Invalid(msg)) if msg.contains("unknown exported symbol `filter`")));
    Ok(())
}

#[test]
fn manifest_writer_rejects_helper_binding_to_an_uncallable_export() -> Result<(), Box<dyn std::error::Error>> {
    // The name is exported, so the unknown-symbol check passes; a helper reference is spliced into call position
    // though, and a trait cannot go there. Failing the provider's own build puts the error where its author can act
    // on it, rather than surfacing in every consumer as broken generated Rust.
    let mut manifest = legacy_manifest_fixture("mylib", "0.1.0");
    manifest.exports.traits.push(TraitExport {
        name: "Filterable".to_string(),
        source_name: None,
        type_params: Vec::new(),
        supertraits: Vec::new(),
        requires: Vec::new(),
        methods: Vec::new(),
    });
    manifest.vocab = Some(VocabExports {
        crate_path: "crates/mylib_vocab".to_string(),
        package_name: "mylib_vocab".to_string(),
        keyword_registrations: Vec::new(),
        dsl_surfaces: Vec::new(),
        provider_manifest: incan_vocab::LibraryManifest {
            helper_bindings: vec![incan_vocab::HelperBinding {
                key: "filter".to_string(),
                exported_name: "Filterable".to_string(),
            }],
            ..incan_vocab::LibraryManifest::default()
        },
        desugarer_artifact: None,
    });

    let tmp = tempfile::tempdir()?;
    let err = manifest.write_to_path(&tmp.path().join("mylib.incnlib"));
    assert!(
        matches!(&err, Err(LibraryManifestError::Invalid(msg)) if msg.contains("trait `Filterable`")),
        "error should name the kind: {err:?}"
    );
    assert!(
        matches!(&err, Err(LibraryManifestError::Invalid(msg)) if msg.contains("cannot be called")),
        "unexpected error: {err:?}"
    );
    Ok(())
}

#[test]
fn manifest_writer_rejects_a_helper_binding_through_an_alias_to_an_uncallable_target()
-> Result<(), Box<dyn std::error::Error>> {
    // The alias is exported and looks callable on its face, so a check that stops at the alias admits it. Following
    // the reexport to its target is what the consumer's frontend does, and the provider's own build has to reach the
    // same answer; otherwise this publishes cleanly and then fails in every consumer as broken generated Rust.
    let mut manifest = legacy_manifest_fixture("mylib", "0.1.0");
    manifest.exports.traits.push(TraitExport {
        name: "Filterable".to_string(),
        source_name: None,
        type_params: Vec::new(),
        supertraits: Vec::new(),
        requires: Vec::new(),
        methods: Vec::new(),
    });
    manifest.exports.aliases.push(AliasExport {
        name: "Filter".to_string(),
        target_path: vec!["mylib".to_string(), "Filterable".to_string()],
        projected_type: None,
        projected_function: None,
    });
    manifest.vocab = Some(VocabExports {
        crate_path: "crates/mylib_vocab".to_string(),
        package_name: "mylib_vocab".to_string(),
        keyword_registrations: Vec::new(),
        dsl_surfaces: Vec::new(),
        provider_manifest: incan_vocab::LibraryManifest {
            helper_bindings: vec![incan_vocab::HelperBinding {
                key: "filter".to_string(),
                exported_name: "Filter".to_string(),
            }],
            ..incan_vocab::LibraryManifest::default()
        },
        desugarer_artifact: None,
    });

    let tmp = tempfile::tempdir()?;
    let err = manifest.write_to_path(&tmp.path().join("mylib.incnlib"));
    assert!(
        matches!(&err, Err(LibraryManifestError::Invalid(msg)) if msg.contains("trait `Filter`")),
        "error should name what the alias resolves to: {err:?}"
    );
    Ok(())
}

#[test]
fn manifest_writer_accepts_a_helper_binding_to_a_public_partial() -> Result<(), Box<dyn std::error::Error>> {
    // A public partial is a preset over a callable target, so it is callable in its own right and belongs on the
    // surface a companion may bind to. Omitting it from the export authority rejected a legitimate public export as
    // an unknown symbol, and the provider had no way to publish the helper at all.
    let mut manifest = legacy_manifest_fixture("mylib", "0.1.0");
    manifest.exports.partials.push(PartialExport {
        name: "filter_active".to_string(),
        target_path: vec!["mylib".to_string(), "filter_rows".to_string()],
        target_kind: PartialTargetKindExport::Function,
        presets: vec![PartialPresetExport {
            name: "status".to_string(),
            ty: TypeRef::Named {
                origin: None,
                name: "str".to_string(),
            },
            value: PresetValueExport::String("active".to_string()),
        }],
        type_params: Vec::new(),
        params: vec![ParamExport {
            is_mut: false,
            name: "status".to_string(),
            ty: TypeRef::Named {
                origin: None,
                name: "str".to_string(),
            },
            kind: ParamKindExport::Normal,
            has_default: true,
            default: None,
        }],
        return_type: TypeRef::Named {
            origin: None,
            name: "None".to_string(),
        },
        is_async: false,
    });
    manifest.vocab = Some(VocabExports {
        crate_path: "crates/mylib_vocab".to_string(),
        package_name: "mylib_vocab".to_string(),
        keyword_registrations: Vec::new(),
        dsl_surfaces: Vec::new(),
        provider_manifest: incan_vocab::LibraryManifest {
            helper_bindings: vec![incan_vocab::HelperBinding {
                key: "filter".to_string(),
                exported_name: "filter_active".to_string(),
            }],
            ..incan_vocab::LibraryManifest::default()
        },
        desugarer_artifact: None,
    });

    let tmp = tempfile::tempdir()?;
    manifest.write_to_path(&tmp.path().join("mylib.incnlib"))?;
    Ok(())
}

#[test]
fn manifest_writer_rejects_duplicate_helper_binding_keys() -> Result<(), Box<dyn std::error::Error>> {
    let mut manifest = legacy_manifest_fixture("mylib", "0.1.0");
    manifest.exports.functions.push(FunctionExport {
        name: "filter".to_string(),
        emitted_name: None,
        type_params: Vec::new(),
        params: Vec::new(),
        return_type: TypeRef::Unknown,
        is_async: false,
    });
    manifest.exports.functions.push(FunctionExport {
        name: "where_impl".to_string(),
        emitted_name: None,
        type_params: Vec::new(),
        params: Vec::new(),
        return_type: TypeRef::Unknown,
        is_async: false,
    });
    manifest.vocab = Some(VocabExports {
        crate_path: "crates/mylib_vocab".to_string(),
        package_name: "mylib_vocab".to_string(),
        keyword_registrations: Vec::new(),
        dsl_surfaces: Vec::new(),
        provider_manifest: incan_vocab::LibraryManifest {
            helper_bindings: vec![
                incan_vocab::HelperBinding {
                    key: "filter".to_string(),
                    exported_name: "filter".to_string(),
                },
                incan_vocab::HelperBinding {
                    key: "filter".to_string(),
                    exported_name: "where_impl".to_string(),
                },
            ],
            ..incan_vocab::LibraryManifest::default()
        },
        desugarer_artifact: None,
    });

    let tmp = tempfile::tempdir()?;
    let err = manifest.write_to_path(&tmp.path().join("mylib.incnlib"));
    assert!(matches!(err, Err(LibraryManifestError::Invalid(msg)) if msg.contains("duplicate key `filter`")));
    Ok(())
}

#[test]
fn manifest_writer_rejects_non_normalized_desugarer_relative_path() -> Result<(), Box<dyn std::error::Error>> {
    let mut manifest = LibraryManifest::new("mylib", "0.1.0");
    manifest.vocab = Some(VocabExports {
        crate_path: "crates/mylib_vocab".to_string(),
        package_name: "mylib_vocab".to_string(),
        keyword_registrations: Vec::new(),
        dsl_surfaces: Vec::new(),
        provider_manifest: incan_vocab::LibraryManifest::default(),
        desugarer_artifact: Some(VocabDesugarerArtifact {
            artifact_kind: incan_vocab::DesugarerArtifactKind::WasmModule,
            abi_version: incan_vocab::WASM_DESUGAR_ABI_VERSION,
            relative_path: "../escape.wasm".to_string(),
            target: "wasm32-wasip1".to_string(),
            profile: "release".to_string(),
            entrypoint: incan_vocab::WASM_DESUGAR_ENTRYPOINT.to_string(),
            sha256: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_string(),
        }),
    });

    let tmp = tempfile::tempdir()?;
    let err = manifest.write_to_path(&tmp.path().join("mylib.incnlib"));
    assert!(
        matches!(err, Err(LibraryManifestError::Invalid(msg)) if msg.contains("must be a normalized relative path"))
    );
    Ok(())
}

#[test]
fn manifest_writer_rejects_non_hex_desugarer_sha256() -> Result<(), Box<dyn std::error::Error>> {
    let mut manifest = LibraryManifest::new("mylib", "0.1.0");
    manifest.vocab = Some(VocabExports {
        crate_path: "crates/mylib_vocab".to_string(),
        package_name: "mylib_vocab".to_string(),
        keyword_registrations: Vec::new(),
        dsl_surfaces: Vec::new(),
        provider_manifest: incan_vocab::LibraryManifest::default(),
        desugarer_artifact: Some(VocabDesugarerArtifact {
            artifact_kind: incan_vocab::DesugarerArtifactKind::WasmModule,
            abi_version: incan_vocab::WASM_DESUGAR_ABI_VERSION,
            relative_path: "desugarers/mylib.wasm".to_string(),
            target: "wasm32-wasip1".to_string(),
            profile: "release".to_string(),
            entrypoint: incan_vocab::WASM_DESUGAR_ENTRYPOINT.to_string(),
            sha256: "not-a-valid-sha256".to_string(),
        }),
    });

    let tmp = tempfile::tempdir()?;
    let err = manifest.write_to_path(&tmp.path().join("mylib.incnlib"));
    assert!(matches!(err, Err(LibraryManifestError::Invalid(msg)) if msg.contains("must be 64 hex characters")));
    Ok(())
}
