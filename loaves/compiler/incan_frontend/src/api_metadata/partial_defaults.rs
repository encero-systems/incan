//! The defaults a public partial's leftover parameters take from the function it targets.
//!
//! A partial projects its target's signature: a parameter the partial does not preset keeps the default its target
//! declares. The checked API records the partial's surface with such a parameter marked as defaulted but without the
//! default itself, which lives on the target's declaration. A consumer across the package boundary reads both through
//! this module, so the checker accepts a call that omits the parameter exactly when lowering can fill it in, the same
//! rule a direct call to the target follows (#1760).

use super::{ApiDeclaration, ApiPartial, CheckedApiMetadataPackage, partial_export_from_api};
use crate::library_manifest::{ParamExport, PartialExport};

/// How many partial-over-partial hops are followed before the search gives up.
///
/// The checker refuses partial cycles, so this bounds only a malformed provider manifest.
const MAX_PARTIAL_TARGET_HOPS: usize = 16;

/// The root of a crate-relative checked API target path.
const API_CRATE_ROOT_SEGMENT: &str = "crate";

/// Return a checked API partial's export with each leftover defaulted parameter completed from its target.
///
/// A leftover parameter takes the default its target declares, whether or not that default can be carried to a
/// consumer; one whose target declares it without a carried default stops counting as defaulted, since no caller can
/// have it filled in. A partial whose declaring module or target the checked API does not describe keeps its surface
/// as recorded.
pub fn partial_export_with_target_defaults(api: &CheckedApiMetadataPackage, partial: &ApiPartial) -> PartialExport {
    complete_partial(api, partial, 0)
}

/// Return a manifest partial export completed from the checked API partial it was published from.
///
/// The compact export list and the checked API record the same partial surface; the checked API is the one that also
/// knows the module the partial is declared in, which its target is resolved against.
pub fn manifest_partial_with_target_defaults(
    api: Option<&CheckedApiMetadataPackage>,
    export: &PartialExport,
) -> PartialExport {
    let Some(api) = api else {
        return export.clone();
    };
    let mut candidates = api.modules.iter().flat_map(|module| {
        module.declarations.iter().filter_map(|declaration| match declaration {
            ApiDeclaration::Partial(partial)
                if partial.name == export.name && partial.target_path == export.target_path =>
            {
                Some(partial)
            }
            _ => None,
        })
    });
    match (candidates.next(), candidates.next()) {
        (Some(partial), None) => complete_partial(api, partial, 0),
        _ => export.clone(),
    }
}

/// Complete one partial, `hops` partial targets below the partial a consumer named.
fn complete_partial(api: &CheckedApiMetadataPackage, partial: &ApiPartial, hops: usize) -> PartialExport {
    let mut export = partial_export_from_api(partial);
    let is_leftover = |param: &ParamExport| {
        param.has_default && param.default.is_none() && !partial.presets.iter().any(|preset| preset.name == param.name)
    };
    if hops >= MAX_PARTIAL_TARGET_HOPS || !export.params.iter().any(is_leftover) {
        return export;
    }
    let Some(target_params) = declaring_module(api, partial)
        .and_then(|module_path| target_params(api, &module_path, &partial.target_path, hops + 1))
    else {
        return export;
    };
    for param in export.params.iter_mut() {
        if !is_leftover(param) {
            continue;
        }
        let Some(target) = target_params.iter().find(|target| target.name == param.name) else {
            continue;
        };
        if target.default.is_some() {
            param.default = target.default.clone();
        } else if !target.has_default {
            param.has_default = false;
        }
    }
    export
}

/// Return the module that declares one checked API partial.
fn declaring_module(api: &CheckedApiMetadataPackage, partial: &ApiPartial) -> Option<Vec<String>> {
    api.modules
        .iter()
        .find(|module| {
            module.declarations.iter().any(|declaration| {
                matches!(declaration, ApiDeclaration::Partial(candidate)
                    if candidate.name == partial.name && candidate.anchor == partial.anchor)
            })
        })
        .map(|module| module.module_path.clone())
}

/// Return the parameters of the callable a partial targets: first within the partial's module, then as written.
fn target_params(
    api: &CheckedApiMetadataPackage,
    module_path: &[String],
    target_path: &[String],
    hops: usize,
) -> Option<Vec<ParamExport>> {
    let module_local = (target_path.len() == 1).then(|| {
        let mut path = module_path.to_vec();
        path.extend(target_path.iter().cloned());
        path
    });
    module_local
        .and_then(|path| callable_params_at(api, &path, hops))
        .or_else(|| callable_params_at(api, target_path, hops))
}

/// Return the parameters of the callable a module-qualified checked API path names.
///
/// Aliases are followed to their target, and a partial is itself completed from its own target.
fn callable_params_at(
    api: &CheckedApiMetadataPackage,
    target_path: &[String],
    hops: usize,
) -> Option<Vec<ParamExport>> {
    if hops >= MAX_PARTIAL_TARGET_HOPS {
        return None;
    }
    let path = match target_path.split_first() {
        Some((root, rest)) if root == API_CRATE_ROOT_SEGMENT => rest,
        _ => target_path,
    };
    let (name, module_path) = path.split_last()?;
    let module = api.modules.iter().find(|module| module.module_path == module_path)?;
    module.declarations.iter().find_map(|declaration| match declaration {
        ApiDeclaration::Function(function) if function.name == *name => Some(function.params.clone()),
        ApiDeclaration::Alias(alias) if alias.name == *name => match &alias.projected_function {
            Some(projected) => Some(projected.callable.params.clone()),
            None => callable_params_at(api, &alias.target_path, hops + 1),
        },
        ApiDeclaration::Partial(partial) if partial.name == *name => Some(complete_partial(api, partial, hops).params),
        _ => None,
    })
}
