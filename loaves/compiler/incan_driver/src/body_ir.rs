//! Shared checked source-to-Body-IR construction for compilation, inspection and editor analysis.

use crate::session::{CompilationAnalysis, CompilationSession};
use crate::unit::UnitKind;
use incan_frontend::loaf_index::LoafIndex;
use incan_frontend::parsed_module::ParsedModule;
use incan_semantics_core::body_ir::BodyIrModule;
use incan_semantics_core::{CanonicalSymbolId, SymbolOrigin};
use std::collections::BTreeSet;

/// Build authored modules in collection order against one provider catalog and the preceding dependency context.
/// The unit kind supplies publication identity; no caller may rebuild a module with a different library contract.
///
/// A member of an import cycle is also lowered against the declarations of the cycle's members that follow it, so it
/// sees every other member's models whatever order the cycle's bodies are lowered in. Those declarations leave the
/// context once the member is lowered; each member's complete module takes their place in turn.
pub fn lower_authored_modules(
    modules: &[ParsedModule],
    analysis: &CompilationAnalysis,
    session: &CompilationSession,
    bodies: &mut Vec<BodyIrModule>,
) -> Result<(), String> {
    let authored_start = bodies.len();
    let provider_operations = incan_frontend::body_ir::ProviderOperationCatalog::from_loaves(session.loaves())?;
    let unit = session.unit();
    let library_package = (unit.kind == UnitKind::Library).then_some(unit.package.as_str());
    let cycle_peers = crate::modules::import_cycle_peers(modules, session.own_standard_package());
    for (index, module) in modules.iter().enumerate() {
        let type_info = analysis
            .type_info_for_path(&module.file_path)
            .ok_or("module analysis is missing")?;
        let completed = bodies.len();
        for &peer in cycle_peers[index].iter().filter(|&&peer| peer > index) {
            let peer_module = &modules[peer];
            let peer_info = analysis
                .type_info_for_path(&peer_module.file_path)
                .ok_or("module analysis is missing")?;
            bodies.push(incan_frontend::body_ir::build_cycle_declarations(
                &peer_module.ast,
                &peer_module.path_segments,
                peer_info,
                library_package,
            ));
        }
        let context = incan_frontend::body_ir::BodyIrContext {
            modules: bodies,
            provider_operations: &provider_operations,
            registry_package: Some(&unit.package),
            library_package,
            include_inline_tests: unit.kind == UnitKind::Test,
        };
        if let Some(directory) = std::env::var_os("INCAN_BODY_IR_FREE_BASELINE") {
            incan_frontend::body_ir::verify_free_function_inputs(
                &module.ast,
                &module.path_segments,
                type_info,
                &context,
                std::path::Path::new(&directory),
            )
            .map_err(|error| format!("Body IR free-function input baseline: {error}"))?;
        }
        let lowered = incan_frontend::body_ir::build_body_ir_module_unsettled(
            &module.ast,
            &module.path_segments,
            type_info,
            &context,
        );
        bodies.truncate(completed);
        bodies.push(lowered);
    }
    let (context, authored) = bodies.split_at_mut(authored_start);
    incan_frontend::body_ir::settle_shared_reads(authored, context);
    Ok(())
}

/// Construct authored source bodies through compilation's sequential module boundary.
/// Checking-only dependency interfaces have no executable publication to load; their checked identities remain
/// available through the analysis and provider catalog rather than requiring a native build for inspection.
pub fn checked_modules(
    modules: &[ParsedModule],
    analysis: &CompilationAnalysis,
    session: &CompilationSession,
) -> Result<Vec<BodyIrModule>, String> {
    let mut bodies = Vec::new();
    lower_authored_modules(modules, analysis, session, &mut bodies)?;
    Ok(bodies)
}

/// Project exact public package requirements reachable from accepted module facts, including private re-export calls.
/// The current package's declarations compile from this unit and never resolve through a dependency artifact.
/// Trait-wide default inventories are not execution roots: unused adaptor bodies must not instantiate for every
/// adopter. Native comparison bridges add their selected slots from checked Body IR in `complete_dependency_context`.
pub fn required_loaf_identities(
    modules: &[ParsedModule],
    analysis: &CompilationAnalysis,
    package_identity: &str,
) -> Result<BTreeSet<CanonicalSymbolId>, String> {
    let infos = modules
        .iter()
        .map(|module| {
            analysis
                .type_info_for_path(&module.file_path)
                .ok_or("module analysis is missing")
        })
        .collect::<Result<Vec<_>, _>>()?;
    let facts = modules
        .iter()
        .zip(&infos)
        .map(|(module, info)| info.semantic_fact_store(&module.path_segments))
        .collect::<Vec<_>>();
    let roots = infos.iter().copied().flat_map(|info| {
        info.declarations
            .declaration_identities
            .values()
            .chain(info.declarations.member_declaration_identities.values())
            .cloned()
    });
    let requested = incan_semantics_core::dependencies::CheckedDependencyGraph::from_fact_stores(&facts)
        .reachable_from(roots)
        .into_iter()
        .chain(
            infos
                .iter()
                .copied()
                .flat_map(|info| info.calls.body_ir_reexport_calls.values())
                .map(|call| call.canonical.clone()),
        )
        .chain(
            infos
                .iter()
                .copied()
                .flat_map(|info| info.calls.body_ir_async_call_targets.values().cloned()),
        )
        .chain(
            infos
                .iter()
                .copied()
                .flat_map(|info| info.calls.source_display_methods.values().cloned()),
        )
        .chain(
            infos
                .iter()
                .copied()
                .flat_map(|info| info.declarations.body_ir_trait_adoptions.values())
                .flatten()
                .flat_map(|adoption| adoption.methods.iter().flatten().cloned()),
        )
        .chain(
            infos
                .iter()
                .copied()
                .flat_map(|info| info.expressions.validated_newtype_coercions.values())
                .flat_map(|coercion| &coercion.steps)
                .flat_map(|step| step.owner_identity.iter().chain(step.ctor_identity.iter()).cloned()),
        )
        .chain(
            infos
                .iter()
                .copied()
                .flat_map(|info| info.declarations.body_ir_native_type_identities.values().cloned()),
        )
        .chain(
            infos
                .iter()
                .copied()
                .flat_map(incan_frontend::body_ir::registry_helper_identities),
        )
        .chain(infos.iter().copied().flat_map(retained_result_type_identities))
        .filter(
            |identity| matches!(&identity.origin, SymbolOrigin::Package { library, .. } if library != package_identity),
        )
        .collect();
    Ok(requested)
}

/// Retain dependency nominal declarations that checked expression and call results place in executable frame locals.
///
/// A selected dependency method can return a generic carrier that the caller never names in source. The selected
/// method identity reaches its body, but native layout also needs the carrier declaration named only by the checked
/// result type. Walk only executable result facts, preserving the checker's canonical binding for every nested leaf;
/// the dependency graph then closes the carrier's own fields and methods normally.
fn retained_result_type_identities(
    info: &incan_frontend::typechecker::TypeCheckInfo,
) -> impl Iterator<Item = CanonicalSymbolId> {
    let mut retained = Vec::new();
    for identity in info
        .expressions
        .expression_type_identities
        .values()
        .flat_map(|identities| identities.values())
    {
        if !retained.contains(identity) {
            retained.push(identity.clone());
        }
    }
    for ty in info
        .expressions
        .body_ir_expr_types
        .values()
        .chain(info.calls.body_ir_call_result_types.values())
        .chain(info.calls.inferred_call_result_types.values())
    {
        retain_type_identities(ty, info, &mut retained);
    }
    retained.into_iter()
}

/// Project canonical nominal leaves from one already checked type without resolving source spelling again.
fn retain_type_identities(
    ty: &incan_frontend::symbols::ResolvedType,
    info: &incan_frontend::typechecker::TypeCheckInfo,
    identities: &mut Vec<CanonicalSymbolId>,
) {
    use incan_frontend::symbols::ResolvedType;

    match ty {
        ResolvedType::Named(name) => retain_type_identity(name, info, identities),
        ResolvedType::Generic(base, arguments) => {
            retain_type_identity(base, info, identities);
            for argument in arguments {
                retain_type_identities(argument, info, identities);
            }
        }
        ResolvedType::Function(parameters, result) => {
            for parameter in parameters {
                retain_type_identities(&parameter.ty, info, identities);
            }
            retain_type_identities(result, info, identities);
        }
        ResolvedType::TypeToken(inner)
        | ResolvedType::Ref(inner)
        | ResolvedType::RefMut(inner)
        | ResolvedType::FrozenList(inner)
        | ResolvedType::FrozenSet(inner) => retain_type_identities(inner, info, identities),
        ResolvedType::FrozenDict(key, value) => {
            retain_type_identities(key, info, identities);
            retain_type_identities(value, info, identities);
        }
        ResolvedType::Tuple(items) => {
            for item in items {
                retain_type_identities(item, info, identities);
            }
        }
        _ => {}
    }
}

/// Add one checker-authenticated nominal binding, including a module-qualified route when that is the checked name.
fn retain_type_identity(
    name: &str,
    info: &incan_frontend::typechecker::TypeCheckInfo,
    identities: &mut Vec<CanonicalSymbolId>,
) {
    let identity = info
        .declarations
        .named_type_identities
        .get(name)
        .or_else(|| info.declarations.body_ir_native_type_identities.get(name))
        .or_else(|| {
            info.declarations
                .named_type_identities
                .values()
                .chain(info.declarations.body_ir_native_type_identities.values())
                .find(|identity| incan_semantics_core::body_ir::nominal_carrier_key(identity).as_deref() == Some(name))
        })
        .or_else(|| {
            info.declarations
                .qualified_type_references
                .values()
                .find(|reference| match &reference.resolved {
                    incan_frontend::symbols::ResolvedType::Named(route)
                    | incan_frontend::symbols::ResolvedType::Generic(route, _) => route == name,
                    _ => false,
                })
                .map(|reference| &reference.identity)
        });
    if let Some(identity) = identity
        && !identities.contains(identity)
    {
        identities.push(identity.clone());
    }
}

/// Read the authenticated dependency closure and its reached package identities, allowing only exact checked
/// source identities to satisfy local edges. Native linking consumes the same package set rather than treating
/// every available ambient standard package as reached.
pub fn load_loaf_modules(
    loaves: &LoafIndex,
    requested: &BTreeSet<CanonicalSymbolId>,
    checked_local: &BTreeSet<CanonicalSymbolId>,
) -> Result<incan_frontend::executable_resolution::ResolvedExecutableModules, String> {
    incan_frontend::executable_resolution::resolve_loaf_declarations(loaves, requested, checked_local)
        .map_err(|error| format!("unsupported Body IR Loaf declaration: {error}"))
}

#[cfg(test)]
mod tests {
    use super::retained_result_type_identities;
    use incan_frontend::symbols::ResolvedType;
    use incan_frontend::typechecker::TypeCheckInfo;
    use incan_semantics_core::{CanonicalSymbolId, HirSourceSpan, SemanticSourceTargetKind, SymbolOrigin};

    /// A dependency carrier named only by a nested checked call result remains an executable dependency root.
    #[test]
    fn checked_call_results_retain_dependency_nominal_identities() -> Result<(), Box<dyn std::error::Error>> {
        let mut info = TypeCheckInfo::default();
        let mut carrier = CanonicalSymbolId::module_declaration(
            vec!["typed_source".into()],
            "Source",
            SemanticSourceTargetKind::Class,
            HirSourceSpan::new(4, 10),
        );
        carrier.origin = SymbolOrigin::Package {
            library: "standard".into(),
            module_path: vec!["typed_source".into()],
        };
        let key = incan_semantics_core::body_ir::nominal_carrier_key(&carrier)
            .ok_or("dependency carrier has no canonical key")?;
        info.declarations
            .named_type_identities
            .insert("Source".into(), carrier.clone());
        info.calls.inferred_call_result_types.insert(
            incan_frontend::ast::Span::new(20, 30),
            ResolvedType::Generic(
                "Result".into(),
                vec![ResolvedType::Generic(key, vec![ResolvedType::Int]), ResolvedType::Str],
            ),
        );
        info.expressions.expression_type_identities.insert(
            incan_frontend::ast::Span::new(40, 50),
            std::collections::BTreeMap::from([(Vec::new(), carrier.clone())]),
        );

        let retained = retained_result_type_identities(&info).collect::<Vec<_>>();
        assert_eq!(retained, [carrier]);
        Ok(())
    }
}
