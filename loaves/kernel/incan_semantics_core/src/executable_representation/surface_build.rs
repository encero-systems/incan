//! Surface index construction: checked candidates, reachability, coverage and declaration projection.

use super::*;

/// Build public coverage and its deterministic execution closure from one declaring compilation.
///
/// `public` comes from the finalized manifest, including public members, and remains the exact resolution surface.
/// Private declarations enter only when reachable from covered public roots. `unrepresentable` names the bodies Body IR
/// could not fully represent ([`crate::body_ir::Body::first_representation_gap`]), in addition to this layer's
/// portable-reference checks.
///
/// The work is a pipeline and the order is load-bearing, in a way worth stating exactly. Coverage starts
/// pessimistic, and `Covered` is written once, in the final phase, from whatever is still admitted by then. The
/// admitted set itself both grows and shrinks: projection fills it, and `retain_only_satisfiable` removes from it.
/// Publishing coverage last is what makes that safe, because a declaration dropped late can never already have been
/// reported covered. A declaration no phase reaches keeps the pessimistic answer it started with. Each phase is
/// separated out below; this function is the order they run in.
pub fn build_surface(
    modules: &[BodyIrModule],
    library: &str,
    package_version: &str,
    public: &BTreeSet<CanonicalSymbolId>,
    unrepresentable: &BTreeSet<CanonicalSymbolId>,
) -> Result<Vec<u8>, ExecutableRepresentationError> {
    let candidates = checked_candidates(modules, library, public);
    let mut declarations = uncovered_owned_declarations(library, &candidates);
    let mut admitted = project_declarations(modules, library, &candidates, unrepresentable, &mut declarations)?;
    record_trait_slots(&admitted, &mut declarations);
    record_type_context_members(&admitted, &mut declarations);
    retain_only_satisfiable(library, &mut admitted, &mut declarations);
    retain_reachable(library, public, &mut admitted, &mut declarations);
    for coverage in declarations.values_mut() {
        if matches!(coverage, DeclarationCoverage::TypeContext { owner } if !admitted.contains_key(owner)) {
            *coverage = DeclarationCoverage::Uncovered(CoverageReason::RequiredDeclarationUnavailable);
        }
    }
    encode_surface(library, package_version, public, admitted, declarations)
}

/// Collect actual checked declarations and their members; eligibility never adds an importable export.
pub(super) fn checked_candidates(
    modules: &[BodyIrModule],
    library: &str,
    public: &BTreeSet<CanonicalSymbolId>,
) -> BTreeSet<CanonicalSymbolId> {
    let mut identities = public.clone();
    for module in modules {
        // ---- Declarations: checked bodies, storage, aliases, and trait owners ----
        identities.extend(module.bodies.iter().filter_map(|body| body.canonical.clone()));
        identities.extend(module.static_declarations.iter().map(|value| value.canonical.clone()));
        identities.extend(module.constant_declarations.iter().map(|value| value.canonical.clone()));
        identities.extend(
            module
                .type_alias_declarations
                .iter()
                .map(|value| value.canonical.clone()),
        );
        identities.extend(module.trait_declarations.iter().cloned());

        // ---- Layouts: checked fields and variants remain part of their declaring context ----
        for value in &module.nominal_declarations {
            identities.insert(value.canonical.clone());
            identities.extend(value.field_identities.iter().cloned());
        }
        for value in &module.enum_declarations {
            identities.insert(value.canonical.clone());
            identities.extend(value.variants.iter().map(|variant| variant.canonical.clone()));
        }
        for value in &module.fieldless_enum_declarations {
            identities.insert(value.canonical.clone());
            identities.extend(value.variants.iter().map(|variant| variant.canonical.clone()));
        }
        for value in &module.value_enum_declarations {
            identities.insert(value.canonical.clone());
            identities.extend(value.variants.iter().map(|variant| variant.canonical.clone()));
        }

        // ---- Dispatch: retain actual slot identities, without completing an absent implementation body ----
        for implementation in &module.trait_implementations {
            identities.insert(implementation.method.clone());
            identities.insert(implementation.implementation.clone());
        }
    }
    identities.retain(|identity| identity.scope_discriminant.is_none() && owned_by(identity, library));
    identities
}

/// Prune implementation fragments outside the satisfiable closure of finalized public exports.
pub(super) fn retain_reachable(
    library: &str,
    public: &BTreeSet<CanonicalSymbolId>,
    admitted: &mut BTreeMap<CanonicalSymbolId, AdmittedDeclaration>,
    declarations: &mut BTreeMap<CanonicalSymbolId, DeclarationCoverage>,
) {
    let mut pending = public.clone();
    let mut reached = BTreeSet::new();
    while let Some(identity) = pending.pop_first() {
        if !reached.insert(identity.clone()) {
            continue;
        }
        if let Some(DeclarationCoverage::TypeContext { owner }) = declarations.get(&identity) {
            pending.insert(owner.clone());
        }
        if let Some(entry) = admitted.get(&identity) {
            pending.extend(
                entry
                    .requirements
                    .iter()
                    .filter(|required| owned_by(required, library))
                    .cloned(),
            );
        }
    }
    admitted.retain(|identity, _| reached.contains(identity));
    declarations.retain(|identity, _| public.contains(identity) || reached.contains(identity));
}

/// Start every eligible identity this package declares at uncovered before dependency and reachability checks.
///
/// An identity no later phase reaches keeps this answer, which is the safe direction: a consumer refuses a
/// declaration this package could not publish, rather than attempting one whose payload was never written. Later
/// phases may replace this reason with a more specific one, or supersede it entirely once the declaration is
/// encoded, but nothing reaches `Covered` without surviving every phase in between.
pub(super) fn uncovered_owned_declarations(
    library: &str,
    public: &BTreeSet<CanonicalSymbolId>,
) -> BTreeMap<CanonicalSymbolId, DeclarationCoverage> {
    public
        .iter()
        .filter(|identity| owned_by(identity, library))
        .map(|identity| {
            (
                identity.clone(),
                DeclarationCoverage::Uncovered(CoverageReason::NoExecutableDeclaration),
            )
        })
        .collect()
}

/// Return whether a canonical identity is declared by this package rather than reached through a dependency.
pub(super) fn owned_by(identity: &CanonicalSymbolId, library: &str) -> bool {
    matches!(&identity.origin, crate::SymbolOrigin::Package { library: owner, .. } if owner == library)
}

/// Project every declaration this package owns, admitting what survives and recording why the rest did not.
///
/// Each declaration kind reaches the same two outcomes through its own projection: admitted with its requirements,
/// or uncovered with the reason a consumer is told. Nothing here decides whether a requirement can actually be met
/// — that is a whole-package question, answered next.
pub(super) fn project_declarations(
    modules: &[BodyIrModule],
    library: &str,
    public: &BTreeSet<CanonicalSymbolId>,
    unrepresentable: &BTreeSet<CanonicalSymbolId>,
    declarations: &mut BTreeMap<CanonicalSymbolId, DeclarationCoverage>,
) -> Result<BTreeMap<CanonicalSymbolId, AdmittedDeclaration>, ExecutableRepresentationError> {
    let mut admitted = BTreeMap::new();
    let native_callables = modules
        .iter()
        .flat_map(|module| &module.loaf_callables)
        .collect::<Vec<_>>();
    for module in modules {
        for value in &module.static_declarations {
            if !declarations.contains_key(&value.canonical) {
                continue;
            }
            match publication::project_static(value, module, library, public) {
                Ok((value, requirements)) => {
                    admitted.insert(
                        value.canonical.clone(),
                        AdmittedDeclaration {
                            declaration: ExecutableDeclaration::Static(value),
                            requirements,
                        },
                    );
                }
                Err(reason) => {
                    declarations.insert(value.canonical.clone(), DeclarationCoverage::Uncovered(reason));
                }
            }
        }
        for value in &module.type_alias_declarations {
            if !declarations.contains_key(&value.canonical) {
                continue;
            }
            match publication::project_type_alias(value, module, library, public) {
                Ok((value, requirements)) => {
                    admitted.insert(
                        value.canonical.clone(),
                        AdmittedDeclaration {
                            declaration: ExecutableDeclaration::TypeAlias(value),
                            requirements,
                        },
                    );
                }
                Err(reason) => {
                    declarations.insert(value.canonical.clone(), DeclarationCoverage::Uncovered(reason));
                }
            }
        }
        for value in &module.constant_declarations {
            if declarations.contains_key(&value.canonical) {
                match publication::project_constant(value, module, library, public) {
                    Ok((value, requirements)) => {
                        admitted.insert(
                            value.canonical.clone(),
                            AdmittedDeclaration {
                                declaration: ExecutableDeclaration::Constant(value),
                                requirements,
                            },
                        );
                    }
                    Err(reason) => {
                        declarations.insert(value.canonical.clone(), DeclarationCoverage::Uncovered(reason));
                    }
                }
            }
        }
        // ---- Bodies: the executable declarations a consumer calls ----
        for body in &module.bodies {
            let Some(identity) = body.canonical.as_ref() else {
                continue;
            };
            if !declarations.contains_key(identity) {
                continue;
            }
            // A compiled implementation publishes its declaration independently; a consumer-instantiated body must
            // retain the full executable closure and its ordinary refusal vocabulary.
            let projected = if publishes_compiled_frame(module, body) {
                publication::project_compiled_frame(body, module, library, public)
            } else if unrepresentable.contains(identity) {
                Err(CoverageReason::UnsupportedConstruct)
            } else {
                publication::project_callable(body, module, library, public, &native_callables)
            };
            match projected {
                Ok((declaration, requirements)) => {
                    admitted.insert(
                        identity.clone(),
                        AdmittedDeclaration {
                            declaration,
                            requirements,
                        },
                    );
                }
                Err(reason) => {
                    declarations.insert(identity.clone(), DeclarationCoverage::Uncovered(reason));
                }
            }
        }

        project_nominals(module, library, public, &mut admitted, declarations);
        project_enums(module, library, public, &mut admitted, declarations)?;
    }
    Ok(admitted)
}

/// Compiled callables retain declaration frames independently of their implementation's portable closure.
/// Open owners, trait defaults and generator producers still require their executable governed representation.
pub(super) fn publishes_compiled_frame(module: &BodyIrModule, body: &Body) -> bool {
    let Some(identity) = &body.canonical else {
        return false;
    };
    crate::body_ir::LoafCallable::from_body(body).is_some()
        && !module.body_requires_instantiation(body)
        && !body.is_generator()
        && !module
            .decorated_bindings
            .iter()
            .any(|binding| binding.original_body.canonical.as_ref() == Some(identity))
        && !module
            .static_declarations
            .iter()
            .any(|storage| &storage.canonical == identity)
        && matches!(
            identity.kind,
            crate::SemanticSourceTargetKind::Function
                | crate::SemanticSourceTargetKind::Method
                | crate::SemanticSourceTargetKind::Property
        )
        && !module.trait_declarations.iter().any(|owner| {
            owner.origin == identity.origin
                && owner.declaration_span.start <= identity.declaration_span.start
                && owner.declaration_span.end >= identity.declaration_span.end
        })
}

/// Project public nominal layouts and retain checked trait slots without synthesizing consumer dispatch.
pub(super) fn project_nominals(
    module: &BodyIrModule,
    library: &str,
    public: &BTreeSet<CanonicalSymbolId>,
    admitted: &mut BTreeMap<CanonicalSymbolId, AdmittedDeclaration>,
    declarations: &mut BTreeMap<CanonicalSymbolId, DeclarationCoverage>,
) {
    // ---- Nominals: models and classes, whose members are addressed through them ----
    for nominal in &module.nominal_declarations {
        if !declarations.contains_key(&nominal.canonical) {
            continue;
        }
        match publication::project_nominal(nominal, module, library, public) {
            Ok((nominal, mut requirements)) => {
                let implementations = module
                    .trait_implementations
                    .iter()
                    .filter(|implementation| implementation.owner == nominal.canonical)
                    .cloned()
                    .collect::<Vec<_>>();
                for implementation in &implementations {
                    // A checked lazy default records dispatch eligibility, not a call. Its body is required only
                    // when a consumer selects that exact slot; unrelated defaults cannot invalidate this layout.
                    if implementation.implementation == implementation.method {
                        continue;
                    }
                    requirements.insert(implementation.method.clone());
                    requirements.insert(implementation.implementation.clone());
                }
                let identity = nominal.canonical.clone();
                let declaration = if implementations.is_empty() {
                    ExecutableDeclaration::Nominal(nominal)
                } else {
                    ExecutableDeclaration::NominalWithTraits(nominal, implementations)
                };
                admitted.insert(
                    identity,
                    AdmittedDeclaration {
                        declaration,
                        requirements,
                    },
                );
            }
            Err(reason) => {
                declarations.insert(nominal.canonical.clone(), DeclarationCoverage::Uncovered(reason));
            }
        }
    }

    for identity in &module.trait_declarations {
        if declarations.contains_key(identity) {
            admitted.insert(
                identity.clone(),
                AdmittedDeclaration {
                    declaration: ExecutableDeclaration::Trait(identity.clone()),
                    requirements: BTreeSet::new(),
                },
            );
        }
    }
}

/// Project normal, fieldless, and value enums using the same checked-candidate boundary.
pub(super) fn project_enums(
    module: &BodyIrModule,
    library: &str,
    public: &BTreeSet<CanonicalSymbolId>,
    admitted: &mut BTreeMap<CanonicalSymbolId, AdmittedDeclaration>,
    declarations: &mut BTreeMap<CanonicalSymbolId, DeclarationCoverage>,
) -> Result<(), ExecutableRepresentationError> {
    // ---- Enums: retain every checked variant required by the complete layout ----
    for value in &module.enum_declarations {
        if !declarations.contains_key(&value.canonical) {
            continue;
        }
        match publication::project_enum(value, module, library, public) {
            Ok((value, requirements)) => {
                admitted.insert(
                    value.canonical.clone(),
                    AdmittedDeclaration {
                        declaration: ExecutableDeclaration::Enum(value),
                        requirements,
                    },
                );
            }
            Err(reason) => {
                declarations.insert(value.canonical.clone(), DeclarationCoverage::Uncovered(reason));
            }
        }
    }
    //
    // Every variant must have a retained checked identity. Publication of internal context does not make its
    // variants importable. Neither kind carries requirements: a variant has no payload to depend on.
    //
    // Value enums additionally retain their checked native layout: the scalar registry cannot substitute for
    // the carrier's derive selections or canonical member addresses.
    for value in &module.fieldless_enum_declarations {
        if admitted.contains_key(&value.canonical)
            || !declarations.contains_key(&value.canonical)
            || !value.variants.iter().all(|variant| public.contains(&variant.canonical))
        {
            continue;
        }
        let mut value = value.clone();
        stamp_declaration_id(&mut value.direct_declaration_id, &value.canonical, "enum")?;
        for variant in &mut value.variants {
            stamp_declaration_id(&mut variant.direct_declaration_id, &variant.canonical, "variant")?;
        }
        admitted.insert(
            value.canonical.clone(),
            AdmittedDeclaration {
                declaration: ExecutableDeclaration::FieldlessEnum(value),
                requirements: BTreeSet::new(),
            },
        );
    }

    for value in &module.value_enum_declarations {
        if !declarations.contains_key(&value.canonical)
            || !value.variants.iter().all(|variant| public.contains(&variant.canonical))
        {
            continue;
        }
        let Some(AdmittedDeclaration {
            declaration: ExecutableDeclaration::Enum(layout),
            requirements,
        }) = admitted.remove(&value.canonical)
        else {
            declarations.insert(
                value.canonical.clone(),
                DeclarationCoverage::Uncovered(CoverageReason::NoExecutableDeclaration),
            );
            continue;
        };
        let mut value = value.clone();
        stamp_declaration_id(&mut value.direct_declaration_id, &value.canonical, "value enum")?;
        for variant in &mut value.variants {
            stamp_declaration_id(&mut variant.direct_declaration_id, &variant.canonical, "variant")?;
        }
        admitted.insert(
            value.canonical.clone(),
            AdmittedDeclaration {
                declaration: ExecutableDeclaration::ValueEnumWithLayout(value, layout),
                requirements,
            },
        );
    }
    Ok(())
}

/// Replace a declaration's source-local address with the package-scoped projection of its canonical identity.
///
/// `what` names the thing in the refusal, so an enum and its variants are distinguishable in a diagnostic that
/// otherwise reports the same failure twice.
pub(super) fn stamp_declaration_id(
    slot: &mut CompilerNodeId,
    canonical: &CanonicalSymbolId,
    what: &str,
) -> Result<(), ExecutableRepresentationError> {
    *slot = publication::declaration_id(canonical).map_err(|_| malformed(format!("{what} has no canonical owner")))?;
    Ok(())
}

/// Drop admitted declarations whose own-package requirements are not themselves admitted, to a fixed point.
///
/// One pass is not enough: dropping a declaration can strand a second that required it. Repeating until nothing
/// more is rejected makes the result independent of iteration order, which is what lets mutual public recursion
/// resolve the same way every build.
pub(super) fn retain_only_satisfiable(
    library: &str,
    admitted: &mut BTreeMap<CanonicalSymbolId, AdmittedDeclaration>,
    declarations: &mut BTreeMap<CanonicalSymbolId, DeclarationCoverage>,
) {
    loop {
        let rejected = admitted
            .iter()
            .filter(|(_, entry)| {
                entry
                    .requirements
                    .iter()
                    .any(|required| owned_by(required, library) && !admitted.contains_key(required)
                        && !matches!(declarations.get(required), Some(DeclarationCoverage::TypeContext { owner }) if admitted.contains_key(owner)))
            })
            .map(|(identity, _)| identity.clone())
            .collect::<Vec<_>>();
        if rejected.is_empty() {
            break;
        }
        for identity in rejected {
            admitted.remove(&identity);
            declarations.insert(
                identity,
                DeclarationCoverage::Uncovered(CoverageReason::RequiredDeclarationUnavailable),
            );
        }
    }
}

/// Point each admitted type's members at their owner.
///
/// A field or variant is not separately addressable: it is reached through the declaration that owns it. Recording
/// the owner lets a consumer resolve a member identity without the package publishing a fragment per member.
pub(super) fn record_type_context_members(
    admitted: &BTreeMap<CanonicalSymbolId, AdmittedDeclaration>,
    declarations: &mut BTreeMap<CanonicalSymbolId, DeclarationCoverage>,
) {
    let required_members: BTreeSet<_> = admitted.values().flat_map(|entry| entry.requirements.iter()).collect();
    for entry in admitted.values() {
        let value = match &entry.declaration {
            ExecutableDeclaration::ValueEnum(value) | ExecutableDeclaration::ValueEnumWithLayout(value, _) => value,
            _ => continue,
        };
        let crate::SymbolOrigin::Package { library, .. } = &value.canonical.origin else {
            continue;
        };
        for required in &required_members {
            if let Some(owner) =
                publication::generated_value_enum_member_owner(required, std::slice::from_ref(value), library)
            {
                declarations.insert((*required).clone(), DeclarationCoverage::TypeContext { owner });
            }
        }
    }
    for (identity, entry) in admitted {
        // Layouts own addressable members; bodies, traits, constants, aliases and storage are addressed directly.
        let members: Vec<&CanonicalSymbolId> = match &entry.declaration {
            ExecutableDeclaration::Nominal(value) | ExecutableDeclaration::NominalWithTraits(value, _) => {
                value.field_identities.iter().collect()
            }
            ExecutableDeclaration::FieldlessEnum(value) => {
                value.variants.iter().map(|variant| &variant.canonical).collect()
            }
            ExecutableDeclaration::ValueEnum(value) | ExecutableDeclaration::ValueEnumWithLayout(value, _) => {
                value.variants.iter().map(|variant| &variant.canonical).collect()
            }
            ExecutableDeclaration::Enum(value) => value.variants.iter().map(|variant| &variant.canonical).collect(),
            ExecutableDeclaration::Body(_)
            | ExecutableDeclaration::NativeBody { .. }
            | ExecutableDeclaration::CompiledCallableFrame { .. }
            | ExecutableDeclaration::DecoratedBody { .. }
            | ExecutableDeclaration::InvocationDecoratedBody { .. }
            | ExecutableDeclaration::GenericDecoratedBody { .. }
            | ExecutableDeclaration::Trait(_)
            | ExecutableDeclaration::Constant(_) => Vec::new(),
            ExecutableDeclaration::TypeAlias(_) | ExecutableDeclaration::Static(_) => Vec::new(),
        };
        for member in members {
            declarations.insert(
                member.clone(),
                DeclarationCoverage::TypeContext {
                    owner: identity.clone(),
                },
            );
        }
    }
}

/// Abstract public trait slots execute through their declaring trait and its concrete implementation records.
pub(super) fn record_trait_slots(
    admitted: &BTreeMap<CanonicalSymbolId, AdmittedDeclaration>,
    declarations: &mut BTreeMap<CanonicalSymbolId, DeclarationCoverage>,
) {
    for (identity, entry) in admitted {
        if !matches!(entry.declaration, ExecutableDeclaration::Trait(_)) {
            continue;
        }
        for (member, coverage) in declarations.iter_mut() {
            if member.kind == crate::SemanticSourceTargetKind::Method
                && member.origin == identity.origin
                && member.declaration_span.start >= identity.declaration_span.start
                && member.declaration_span.end <= identity.declaration_span.end
                && !admitted.contains_key(member)
                && matches!(
                    coverage,
                    DeclarationCoverage::Uncovered(CoverageReason::NoExecutableDeclaration)
                )
            {
                *coverage = DeclarationCoverage::TypeContext {
                    owner: identity.clone(),
                };
            }
        }
    }
}

/// Encode the admitted fragments and the index that addresses them into the framed wire form.
///
/// The frame is version, then index length, then index, then payload. A consumer reads the version before anything
/// else, so it can refuse an interpretation it does not support without having to parse the rest.
pub(super) fn encode_surface(
    library: &str,
    package_version: &str,
    public: &BTreeSet<CanonicalSymbolId>,
    admitted: BTreeMap<CanonicalSymbolId, AdmittedDeclaration>,
    mut declarations: BTreeMap<CanonicalSymbolId, DeclarationCoverage>,
) -> Result<Vec<u8>, ExecutableRepresentationError> {
    // ---- Payloads: encode in canonical order across public and implementation declarations ----
    let mut payload = Vec::new();
    for (identity, entry) in admitted {
        let mut encoded = postcard::to_allocvec(&entry.declaration).map_err(|error| malformed(error.to_string()))?;
        match &entry.declaration {
            ExecutableDeclaration::Nominal(nominal) | ExecutableDeclaration::NominalWithTraits(nominal, _) => {
                append_trait_fields(&mut encoded, std::slice::from_ref(nominal))?;
            }
            _ => {}
        }
        declarations.insert(
            identity,
            DeclarationCoverage::Covered {
                offset: u64::try_from(payload.len()).map_err(|_| malformed("payload offset exceeds wire range"))?,
                length: u64::try_from(encoded.len()).map_err(|_| malformed("fragment length exceeds wire range"))?,
                requirements: entry.requirements.into_iter().collect(),
            },
        );
        payload.extend(encoded);
    }

    // ---- Indexes: public membership stays identical to the finalized manifest ----
    let implementation = declarations
        .extract_if(.., |identity, _| !public.contains(identity))
        .collect();
    let index = SurfaceIndex {
        library: library.to_owned(),
        package_version: package_version.to_owned(),
        declarations,
        implementation,
    };
    // ---- Envelope: stable version prefix precedes all version-specific content ----
    let encoded_index = postcard::to_allocvec(&index).map_err(|error| malformed(error.to_string()))?;
    let mut bytes =
        postcard::to_allocvec(&EXECUTABLE_REPRESENTATION_VERSION).map_err(|error| malformed(error.to_string()))?;
    bytes.extend(
        u64::try_from(encoded_index.len())
            .map_err(|_| malformed("index length exceeds wire range"))?
            .to_le_bytes(),
    );
    bytes.extend(encoded_index);
    bytes.extend(payload);
    Ok(bytes)
}

/// Normalize codec and contract errors without coupling this layer to package filesystem diagnostics.
pub(super) fn malformed(reason: impl Into<String>) -> ExecutableRepresentationError {
    ExecutableRepresentationError::Malformed { reason: reason.into() }
}
