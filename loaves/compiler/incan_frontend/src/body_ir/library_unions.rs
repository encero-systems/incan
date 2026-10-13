//! Retain SDK-owned anonymous union definitions from admitted provider metadata, never from generated-name guesses.

use super::{IncanType, ProviderPlan, bir};
use crate::library_manifest::TypeRef;
use crate::library_manifest_index::{LibraryArtifactKind, LibraryManifestIndex, LibraryManifestIndexEntry};
use incan_semantics_core::IncanPrimitiveType;
use std::collections::HashMap;

/// Collect only SDK signature unions whose exact owner and producer payload order the selected artifact admits.
pub(super) fn collect(
    plan: &ProviderPlan,
    modules: &[bir::BodyIrModule],
) -> Result<Vec<bir::NativeUnionDeclaration>, String> {
    let plan = sdk_artifact_view(plan)?;
    let mut declarations: Vec<bir::NativeUnionDeclaration> = Vec::new();
    for artifact in plan.public_artifacts() {
        let root = artifact.identity.name.replace('-', "_");
        if !root.starts_with("incan_stdlib_") {
            continue;
        }
        let mut requested = Vec::new();
        for call in modules.iter().flat_map(|module| &module.stdlib_delegations) {
            if call
                .rust_path()
                .and_then(|path| path.split("::").next().map(str::to_owned))
                .as_deref()
                != Some(root.as_str())
                || !call.type_parameters.is_empty()
            {
                continue;
            }
            for ty in call.parameters.iter().chain(std::iter::once(&call.return_type)) {
                unions(ty, &mut requested);
            }
        }
        if requested.is_empty() {
            continue;
        }
        for native in &artifact.manifest.contract_metadata.native_unions {
            let (bound, route) = plan
                .public_native_union_projection(&artifact.identity.name, native)
                .map_err(|error| format!("unsupported Body IR admitted native union: {error}"))?;
            let owner = crate::rust_type_display::provider_bridge_route(&root, route).join("::");
            let Some(members) = bound
                .members
                .iter()
                .map(|member| member_type(member, modules))
                .collect::<Option<Vec<_>>>()
            else {
                continue;
            };
            for ty in &requested {
                let IncanType::Generic { base, args } = ty else {
                    continue;
                };
                if base != "Union" || args.len() != members.len() || !args.iter().all(|member| members.contains(member))
                {
                    continue;
                }
                let declaration = bir::NativeUnionDeclaration {
                    ty: ty.clone(),
                    rust_item: format!("{owner}::{}", bound.rust_name),
                    members: members.clone(),
                };
                if let Some(existing) = declarations.iter().find(|existing| existing.ty == declaration.ty) {
                    if existing != &declaration {
                        return Err("unsupported Body IR conflicting admitted native union owners".to_owned());
                    }
                } else {
                    declarations.push(declaration);
                }
            }
        }
    }
    Ok(declarations)
}

/// Admit a command-local physical view of already selected SDK records without altering the checker's provider plan.
///
/// The public artifact graph intentionally omits reserved SDK records. Body IR needs their emitted-definition tables,
/// so this private view indexes only their retained materialized artifacts and checks that admission preserves every
/// selected identity. It grants no source namespace and is never returned to legacy compilation.
fn sdk_artifact_view(selected: &ProviderPlan) -> Result<ProviderPlan, String> {
    let mut entries = HashMap::new();
    for record in selected.active_sdk_records() {
        let (Some(manifest), Some(artifact)) = (&record.manifest, &record.artifact) else {
            continue;
        };
        if artifact.kind != LibraryArtifactKind::Materialized {
            continue;
        }
        entries.insert(
            record.identity.name.clone(),
            LibraryManifestIndexEntry::Loaded {
                manifest: Box::new(manifest.as_ref().clone()),
                metadata: artifact.clone(),
            },
        );
    }
    let view = ProviderPlan::from_admitted_libraries(LibraryManifestIndex::from_entries(entries), &[], [])
        .map_err(|error| format!("unsupported Body IR selected SDK artifact view: {error}"))?;
    for artifact in view.public_artifacts() {
        if !selected
            .active_sdk_records()
            .any(|record| record.identity == artifact.identity)
        {
            return Err("unsupported Body IR selected SDK artifact identity mismatch".to_owned());
        }
    }
    Ok(view)
}

/// Select nominal names only after the projected member's hydrated identity agrees with a retained SDK layout.
fn member_type(member: &TypeRef, modules: &[bir::BodyIrModule]) -> Option<IncanType> {
    match member {
        TypeRef::Named {
            origin: Some(origin), ..
        } => {
            let identity = origin.canonical.hydrate()?;
            modules
                .iter()
                .flat_map(|module| &module.nominal_declarations)
                .find(|declaration| declaration.canonical == identity)
                .map(|declaration| IncanType::Named(declaration.name.clone()))
        }
        TypeRef::Named { name, origin: None } => {
            let primitive = match name.as_str() {
                "int" => IncanPrimitiveType::Int,
                "str" => IncanPrimitiveType::Str,
                "bool" => IncanPrimitiveType::Bool,
                "float" => IncanPrimitiveType::Float,
                "bytes" => IncanPrimitiveType::Bytes,
                _ => return None,
            };
            Some(IncanType::Primitive(primitive))
        }
        TypeRef::Applied {
            name,
            args,
            origin: None,
        } if name == "List" => Some(IncanType::Generic {
            base: name.clone(),
            args: args
                .iter()
                .map(|arg| member_type(arg, modules))
                .collect::<Option<Vec<_>>>()?,
        }),
        _ => None,
    }
}

/// Retain semantic union requests through compound signatures without assigning any physical owner.
fn unions(ty: &IncanType, requested: &mut Vec<IncanType>) {
    match ty {
        IncanType::Generic { base, args } => {
            for arg in args {
                unions(arg, requested);
            }
            if base == "Union" && !requested.contains(ty) {
                requested.push(ty.clone());
            }
        }
        IncanType::Tuple(elements) => {
            for element in elements {
                unions(element, requested);
            }
        }
        IncanType::Ref(inner) | IncanType::RefMut(inner) => unions(inner, requested),
        _ => {}
    }
}
