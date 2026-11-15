//! Vocab payload, scoped symbol and scoped surface descriptor, and soft keyword validation.

use super::*;

/// Validate the optional vocab payload and its desugarer artifact metadata.
///
/// This keeps producer-facing vocab metadata internally consistent before the compiler tries to load any companion
/// artifact or resolve helper references against exported symbols.
pub(super) fn validate_vocab_payload(raw: &RawLibraryManifest) -> Result<(), LibraryManifestError> {
    let Some(vocab) = &raw.vocab else {
        return Ok(());
    };

    if vocab.metadata_version != incan_vocab::VOCAB_METADATA_VERSION {
        return Err(LibraryManifestError::Invalid(format!(
            "vocabulary schema version {}; supported version {}",
            vocab.metadata_version,
            incan_vocab::VOCAB_METADATA_VERSION,
        )));
    }

    if vocab.crate_path.trim().is_empty() {
        return Err(LibraryManifestError::Invalid(
            "vocab crate_path cannot be empty".to_string(),
        ));
    }
    if vocab.package_name.trim().is_empty() {
        return Err(LibraryManifestError::Invalid(
            "vocab package_name cannot be empty".to_string(),
        ));
    }

    validate_scoped_surface_descriptors(raw)?;
    validate_scoped_symbol_descriptors(raw)?;

    let Some(desugarer) = &vocab.desugarer_artifact else {
        return Ok(());
    };

    if matches!(desugarer.artifact_kind, incan_vocab::DesugarerArtifactKind::WasmModule) {
        return Err(LibraryManifestError::Invalid(
            "WebAssembly vocabulary desugarers are refused; rebuild the Loaf with a native desugarer".to_string(),
        ));
    }
    sha256_digest("native vocabulary unit identity", &desugarer.unit_identity, false)
        .map_err(LibraryManifestError::Invalid)?;

    if desugarer.abi_version != incan_vocab::VOCAB_SYNTAX_VERSION {
        return Err(LibraryManifestError::Invalid(format!(
            "vocabulary syntax version {}; supported version {}; rebuild the Loaf",
            desugarer.abi_version,
            incan_vocab::VOCAB_SYNTAX_VERSION
        )));
    }
    if desugarer.relative_path.trim().is_empty() {
        return Err(LibraryManifestError::Invalid(
            "vocab desugarer_artifact.relative_path cannot be empty".to_string(),
        ));
    }
    validate_relative_artifact_path(&desugarer.relative_path)?;
    if desugarer.sha256.trim().is_empty() {
        return Err(LibraryManifestError::Invalid(
            "vocab desugarer_artifact.sha256 cannot be empty".to_string(),
        ));
    }
    sha256_digest("vocab desugarer_artifact.sha256", &desugarer.sha256, false).map_err(LibraryManifestError::Invalid)
}

/// Validate RFC 045 scoped-symbol descriptors before they become compiler-facing manifest data.
pub(super) fn validate_scoped_symbol_descriptors(raw: &RawLibraryManifest) -> Result<(), LibraryManifestError> {
    let Some(vocab) = &raw.vocab else {
        return Ok(());
    };

    let mut seen_descriptor_keys = HashSet::new();
    let mut seen_positive_positions = HashSet::new();

    for surface in &vocab.dsl_surfaces {
        let activation_key = scoped_surface_activation_key(&surface.activation);
        let declarations: HashSet<&str> = surface
            .declarations
            .iter()
            .map(|declaration| declaration.keyword.as_str())
            .collect();
        let clauses: HashSet<(&str, &str)> = surface
            .declarations
            .iter()
            .flat_map(|declaration| {
                declaration
                    .clauses
                    .iter()
                    .map(|clause| (declaration.keyword.as_str(), clause.keyword.as_str()))
            })
            .collect();

        for descriptor in &surface.scoped_symbols {
            validate_scoped_symbol_descriptor_shape(descriptor)?;
            if !seen_descriptor_keys.insert(format!("{activation_key}:{}", descriptor.key)) {
                return Err(LibraryManifestError::Invalid(format!(
                    "duplicate scoped symbol descriptor key `{}` for activation `{activation_key}`",
                    descriptor.key
                )));
            }
            validate_scoped_symbol_role(descriptor)?;
            validate_scoped_symbol_diagnostics(descriptor)?;

            if descriptor.eligible_in.is_empty() {
                return Err(LibraryManifestError::Invalid(format!(
                    "scoped symbol descriptor `{}` must declare at least one eligible position",
                    descriptor.key
                )));
            }

            for eligibility in &descriptor.eligible_in {
                validate_scoped_symbol_eligibility(&descriptor.key, eligibility, &declarations, &clauses)?;
                let position_key = format!(
                    "{}:{}:{}:{}:{}:{:?}",
                    activation_key,
                    descriptor.symbol,
                    eligibility.declaration,
                    eligibility.clause.as_deref().unwrap_or(""),
                    eligibility.call.as_deref().unwrap_or(""),
                    eligibility.position
                );
                if !seen_positive_positions.insert(position_key) {
                    return Err(LibraryManifestError::Invalid(format!(
                        "ambiguous scoped symbol descriptor `{}` conflicts with another descriptor for the same activation, symbol, and eligible position",
                        descriptor.key
                    )));
                }
            }
        }
    }

    Ok(())
}

/// Validate scoped-symbol descriptor identity and identifier spelling.
pub(super) fn validate_scoped_symbol_descriptor_shape(
    descriptor: &incan_vocab::ScopedSymbolDescriptor,
) -> Result<(), LibraryManifestError> {
    if descriptor.key.trim().is_empty() {
        return Err(LibraryManifestError::Invalid(
            "vocab scoped symbol descriptor key cannot be empty".to_string(),
        ));
    }
    if descriptor.symbol.trim().is_empty() {
        return Err(LibraryManifestError::Invalid(format!(
            "scoped symbol descriptor `{}` symbol cannot be empty",
            descriptor.key
        )));
    }
    if !is_identifier_spelling(&descriptor.symbol) {
        return Err(LibraryManifestError::Invalid(format!(
            "scoped symbol descriptor `{}` symbol `{}` is not a valid identifier",
            descriptor.key, descriptor.symbol
        )));
    }
    if incan_lang::lang::keywords::from_str_hard_only(&descriptor.symbol).is_some() {
        return Err(LibraryManifestError::Invalid(format!(
            "scoped symbol descriptor `{}` symbol `{}` cannot be a hard keyword",
            descriptor.key, descriptor.symbol
        )));
    }
    Ok(())
}

/// Validate optional DSL-authored role metadata.
pub(super) fn validate_scoped_symbol_role(
    descriptor: &incan_vocab::ScopedSymbolDescriptor,
) -> Result<(), LibraryManifestError> {
    let Some(role) = &descriptor.role else {
        return Ok(());
    };

    if role.key.trim().is_empty() {
        return Err(LibraryManifestError::Invalid(format!(
            "scoped symbol descriptor `{}` role key cannot be empty",
            descriptor.key
        )));
    }
    if role.label.as_ref().is_some_and(|label| label.trim().is_empty()) {
        return Err(LibraryManifestError::Invalid(format!(
            "scoped symbol descriptor `{}` role label cannot be empty",
            descriptor.key
        )));
    }
    if role
        .description
        .as_ref()
        .is_some_and(|description| description.trim().is_empty())
    {
        return Err(LibraryManifestError::Invalid(format!(
            "scoped symbol descriptor `{}` role description cannot be empty",
            descriptor.key
        )));
    }
    Ok(())
}

/// Validate author-provided diagnostic templates for one scoped-symbol descriptor.
pub(super) fn validate_scoped_symbol_diagnostics(
    descriptor: &incan_vocab::ScopedSymbolDescriptor,
) -> Result<(), LibraryManifestError> {
    let mut seen_codes = HashSet::new();
    for diagnostic in &descriptor.diagnostics {
        if diagnostic.code.trim().is_empty() {
            return Err(LibraryManifestError::Invalid(format!(
                "scoped symbol descriptor `{}` diagnostic code cannot be empty",
                descriptor.key
            )));
        }
        if diagnostic.message.trim().is_empty() {
            return Err(LibraryManifestError::Invalid(format!(
                "scoped symbol descriptor `{}` diagnostic `{}` message cannot be empty",
                descriptor.key, diagnostic.code
            )));
        }
        if !seen_codes.insert(diagnostic.code.as_str()) {
            return Err(LibraryManifestError::Invalid(format!(
                "scoped symbol descriptor `{}` contains duplicate diagnostic code `{}`",
                descriptor.key, diagnostic.code
            )));
        }
    }
    Ok(())
}

/// Validate that a scoped-symbol positive eligibility rule references a known declaration or clause.
pub(super) fn validate_scoped_symbol_eligibility(
    descriptor_key: &str,
    eligibility: &incan_vocab::ScopedSymbolEligibility,
    declarations: &HashSet<&str>,
    clauses: &HashSet<(&str, &str)>,
) -> Result<(), LibraryManifestError> {
    if eligibility.declaration.trim().is_empty() {
        return Err(LibraryManifestError::Invalid(format!(
            "scoped symbol descriptor `{descriptor_key}` eligibility declaration cannot be empty"
        )));
    }
    if !declarations.contains(eligibility.declaration.as_str()) {
        return Err(LibraryManifestError::Invalid(format!(
            "scoped symbol descriptor `{descriptor_key}` references unknown declaration `{}`",
            eligibility.declaration
        )));
    }

    match eligibility.position {
        incan_vocab::ScopedSymbolPosition::ClauseBody => match &eligibility.clause {
            Some(clause) if !clause.trim().is_empty() => {
                if eligibility.call.is_some() {
                    return Err(LibraryManifestError::Invalid(format!(
                        "scoped symbol descriptor `{descriptor_key}` clause-body eligibility cannot declare a call"
                    )));
                }
                if !clauses.contains(&(eligibility.declaration.as_str(), clause.as_str())) {
                    return Err(LibraryManifestError::Invalid(format!(
                        "scoped symbol descriptor `{descriptor_key}` references unknown clause `{}` in declaration `{}`",
                        clause, eligibility.declaration
                    )));
                }
                Ok(())
            }
            _ => Err(LibraryManifestError::Invalid(format!(
                "scoped symbol descriptor `{descriptor_key}` clause-body eligibility must declare a clause"
            ))),
        },
        incan_vocab::ScopedSymbolPosition::DeclarationBody => {
            if eligibility.clause.is_some() || eligibility.call.is_some() {
                return Err(LibraryManifestError::Invalid(format!(
                    "scoped symbol descriptor `{descriptor_key}` declaration eligibility cannot declare a clause or call"
                )));
            }
            Ok(())
        }
        incan_vocab::ScopedSymbolPosition::CallArgument => {
            if eligibility.clause.is_some() {
                return Err(LibraryManifestError::Invalid(format!(
                    "scoped symbol descriptor `{descriptor_key}` call-argument eligibility cannot declare a clause"
                )));
            }
            match eligibility.call.as_deref() {
                Some(call) if !call.trim().is_empty() => Ok(()),
                _ => Err(LibraryManifestError::Invalid(format!(
                    "scoped symbol descriptor `{descriptor_key}` call-argument eligibility must declare a call"
                ))),
            }
        }
        _ => Err(LibraryManifestError::Invalid(format!(
            "scoped symbol descriptor `{descriptor_key}` uses an unsupported eligibility position"
        ))),
    }
}

/// Return whether a scoped symbol spelling is compatible with ordinary identifier call syntax.
pub(super) fn is_identifier_spelling(symbol: &str) -> bool {
    let mut chars = symbol.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first.is_ascii_alphabetic()) && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

/// Validate RFC 040 scoped-surface descriptors before they become compiler-facing manifest data.
pub(super) fn validate_scoped_surface_descriptors(raw: &RawLibraryManifest) -> Result<(), LibraryManifestError> {
    let Some(vocab) = &raw.vocab else {
        return Ok(());
    };

    let mut seen_descriptor_keys = HashSet::new();
    let mut seen_positive_positions = HashSet::new();

    for surface in &vocab.dsl_surfaces {
        let activation_key = scoped_surface_activation_key(&surface.activation);
        let declarations: HashSet<&str> = surface
            .declarations
            .iter()
            .map(|declaration| declaration.keyword.as_str())
            .collect();
        let clauses: HashSet<(&str, &str)> = surface
            .declarations
            .iter()
            .flat_map(|declaration| {
                declaration
                    .clauses
                    .iter()
                    .map(|clause| (declaration.keyword.as_str(), clause.keyword.as_str()))
            })
            .collect();

        for descriptor in &surface.scoped_surfaces {
            if descriptor.key.trim().is_empty() {
                return Err(LibraryManifestError::Invalid(
                    "vocab scoped surface descriptor key cannot be empty".to_string(),
                ));
            }
            if !seen_descriptor_keys.insert(format!("{activation_key}:{}", descriptor.key)) {
                return Err(LibraryManifestError::Invalid(format!(
                    "duplicate scoped surface descriptor key `{}` for activation `{activation_key}`",
                    descriptor.key
                )));
            }
            validate_scoped_surface_syntax(descriptor)?;
            validate_scoped_surface_receiver(descriptor)?;
            validate_scoped_surface_diagnostics(descriptor)?;

            if descriptor.eligible_in.is_empty() {
                return Err(LibraryManifestError::Invalid(format!(
                    "scoped surface descriptor `{}` must declare at least one eligible position",
                    descriptor.key
                )));
            }

            for eligibility in &descriptor.eligible_in {
                validate_scoped_surface_eligibility(&descriptor.key, eligibility, &declarations, &clauses)?;
                let position_key = format!(
                    "{}:{}:{}:{}:{}:{:?}",
                    activation_key,
                    scoped_surface_syntax_key(&descriptor.syntax),
                    eligibility.declaration,
                    eligibility.clause.as_deref().unwrap_or(""),
                    eligibility.call.as_deref().unwrap_or(""),
                    eligibility.position
                );
                if !seen_positive_positions.insert(position_key) {
                    return Err(LibraryManifestError::Invalid(format!(
                        "ambiguous scoped surface descriptor `{}` conflicts with another descriptor for the same activation, syntax, and eligible position",
                        descriptor.key
                    )));
                }
            }
        }
    }

    Ok(())
}

/// Validate that descriptor syntax is well-formed and matches the declared family.
pub(super) fn validate_scoped_surface_syntax(
    descriptor: &incan_vocab::ScopedSurfaceDescriptor,
) -> Result<(), LibraryManifestError> {
    match (&descriptor.family, &descriptor.syntax) {
        (
            incan_vocab::ScopedSurfaceFamily::OperatorLike | incan_vocab::ScopedSurfaceFamily::BindingLike,
            incan_vocab::ScopedSurfaceSyntax::Glyph { spelling },
        ) => {
            if spelling.trim().is_empty() {
                return Err(LibraryManifestError::Invalid(format!(
                    "scoped surface descriptor `{}` glyph spelling cannot be empty",
                    descriptor.key
                )));
            }
        }
        (
            incan_vocab::ScopedSurfaceFamily::ExpressionForm,
            incan_vocab::ScopedSurfaceSyntax::LeadingDotPath {
                min_segments,
                max_segments,
            },
        ) => {
            if *min_segments == 0 {
                return Err(LibraryManifestError::Invalid(format!(
                    "scoped surface descriptor `{}` leading-dot path must accept at least one segment",
                    descriptor.key
                )));
            }
            if max_segments.is_some_and(|max_segments| max_segments < *min_segments) {
                return Err(LibraryManifestError::Invalid(format!(
                    "scoped surface descriptor `{}` leading-dot max_segments cannot be less than min_segments",
                    descriptor.key
                )));
            }
        }
        _ => {
            return Err(LibraryManifestError::Invalid(format!(
                "scoped surface descriptor `{}` uses a syntax shape that does not match its family",
                descriptor.key
            )));
        }
    }

    Ok(())
}

/// Validate receiver metadata for expression-form descriptors.
pub(super) fn validate_scoped_surface_receiver(
    descriptor: &incan_vocab::ScopedSurfaceDescriptor,
) -> Result<(), LibraryManifestError> {
    if descriptor.family == incan_vocab::ScopedSurfaceFamily::ExpressionForm && descriptor.receiver.is_none() {
        return Err(LibraryManifestError::Invalid(format!(
            "expression-form scoped surface descriptor `{}` must declare receiver derivation",
            descriptor.key
        )));
    }
    if descriptor.family != incan_vocab::ScopedSurfaceFamily::ExpressionForm && descriptor.receiver.is_some() {
        return Err(LibraryManifestError::Invalid(format!(
            "non-expression scoped surface descriptor `{}` cannot declare receiver derivation",
            descriptor.key
        )));
    }

    match &descriptor.receiver {
        Some(incan_vocab::ScopedSurfaceReceiver::Clause { clause }) if clause.trim().is_empty() => {
            Err(LibraryManifestError::Invalid(format!(
                "scoped surface descriptor `{}` receiver clause cannot be empty",
                descriptor.key
            )))
        }
        Some(incan_vocab::ScopedSurfaceReceiver::Custom { key }) if key.trim().is_empty() => {
            Err(LibraryManifestError::Invalid(format!(
                "scoped surface descriptor `{}` receiver custom key cannot be empty",
                descriptor.key
            )))
        }
        _ => Ok(()),
    }
}

/// Validate author-provided diagnostic templates for one scoped-surface descriptor.
pub(super) fn validate_scoped_surface_diagnostics(
    descriptor: &incan_vocab::ScopedSurfaceDescriptor,
) -> Result<(), LibraryManifestError> {
    let mut seen_codes = HashSet::new();
    for diagnostic in &descriptor.diagnostics {
        if diagnostic.code.trim().is_empty() {
            return Err(LibraryManifestError::Invalid(format!(
                "scoped surface descriptor `{}` diagnostic code cannot be empty",
                descriptor.key
            )));
        }
        if diagnostic.message.trim().is_empty() {
            return Err(LibraryManifestError::Invalid(format!(
                "scoped surface descriptor `{}` diagnostic `{}` message cannot be empty",
                descriptor.key, diagnostic.code
            )));
        }
        if !seen_codes.insert(diagnostic.code.as_str()) {
            return Err(LibraryManifestError::Invalid(format!(
                "scoped surface descriptor `{}` contains duplicate diagnostic code `{}`",
                descriptor.key, diagnostic.code
            )));
        }
    }
    Ok(())
}

/// Validate that a positive eligibility rule references a known declaration or clause.
pub(super) fn validate_scoped_surface_eligibility(
    descriptor_key: &str,
    eligibility: &incan_vocab::ScopedSurfaceEligibility,
    declarations: &HashSet<&str>,
    clauses: &HashSet<(&str, &str)>,
) -> Result<(), LibraryManifestError> {
    if eligibility.declaration.trim().is_empty() {
        return Err(LibraryManifestError::Invalid(format!(
            "scoped surface descriptor `{descriptor_key}` eligibility declaration cannot be empty"
        )));
    }
    if !declarations.contains(eligibility.declaration.as_str()) {
        return Err(LibraryManifestError::Invalid(format!(
            "scoped surface descriptor `{descriptor_key}` references unknown declaration `{}`",
            eligibility.declaration
        )));
    }

    match eligibility.position {
        incan_vocab::ScopedSurfacePosition::ClauseBody => match &eligibility.clause {
            Some(clause) if !clause.trim().is_empty() => {
                if eligibility.call.is_some() {
                    return Err(LibraryManifestError::Invalid(format!(
                        "scoped surface descriptor `{descriptor_key}` clause-body eligibility cannot declare a call"
                    )));
                }
                if !clauses.contains(&(eligibility.declaration.as_str(), clause.as_str())) {
                    return Err(LibraryManifestError::Invalid(format!(
                        "scoped surface descriptor `{descriptor_key}` references unknown clause `{}` in declaration `{}`",
                        clause, eligibility.declaration
                    )));
                }
                Ok(())
            }
            _ => Err(LibraryManifestError::Invalid(format!(
                "scoped surface descriptor `{descriptor_key}` clause-body eligibility must declare a clause"
            ))),
        },
        incan_vocab::ScopedSurfacePosition::DeclarationHead => Err(LibraryManifestError::Invalid(format!(
            "scoped surface descriptor `{descriptor_key}` cannot use declaration-head eligibility; select a declaration body, clause body or call argument"
        ))),
        incan_vocab::ScopedSurfacePosition::DeclarationBody => {
            if eligibility.clause.is_some() || eligibility.call.is_some() {
                return Err(LibraryManifestError::Invalid(format!(
                    "scoped surface descriptor `{descriptor_key}` declaration eligibility cannot declare a clause or call"
                )));
            }
            Ok(())
        }
        incan_vocab::ScopedSurfacePosition::CallArgument => {
            if eligibility.clause.is_some() {
                return Err(LibraryManifestError::Invalid(format!(
                    "scoped surface descriptor `{descriptor_key}` call-argument eligibility cannot declare a clause"
                )));
            }
            match eligibility.call.as_deref() {
                Some(call) if !call.trim().is_empty() => Ok(()),
                _ => Err(LibraryManifestError::Invalid(format!(
                    "scoped surface descriptor `{descriptor_key}` call-argument eligibility must declare a call"
                ))),
            }
        }
        _ => Err(LibraryManifestError::Invalid(format!(
            "scoped surface descriptor `{descriptor_key}` uses an unsupported eligibility position"
        ))),
    }
}

/// Build a stable validation key for a descriptor activation rule.
pub(super) fn scoped_surface_activation_key(activation: &incan_vocab::KeywordActivation) -> String {
    match activation {
        incan_vocab::KeywordActivation::Always => "always".to_string(),
        incan_vocab::KeywordActivation::OnImport { namespace } => format!("import:{namespace}"),
        _ => "unknown".to_string(),
    }
}

/// Build a stable validation key for a descriptor syntax shape.
pub(super) fn scoped_surface_syntax_key(syntax: &incan_vocab::ScopedSurfaceSyntax) -> String {
    match syntax {
        incan_vocab::ScopedSurfaceSyntax::Glyph { spelling } => format!("glyph:{spelling}"),
        incan_vocab::ScopedSurfaceSyntax::LeadingDotPath {
            min_segments,
            max_segments,
        } => format!("leading-dot:{min_segments}:{max_segments:?}"),
        _ => "unsupported".to_string(),
    }
}
