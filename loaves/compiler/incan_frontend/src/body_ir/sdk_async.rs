//! Retain SDK async operations by accepted signatures and exact catalog declaration identities.

use super::{IncanPrimitiveType, IncanType, TypeCheckInfo, ast, bir, semantic_type_from_resolved};
use crate::symbols::SymbolKind;
use crate::typechecker::stdlib_loader::StdlibAstCache;
use incan_semantics_core::SymbolOrigin;

/// Project accepted SDK bindings once, deduplicating aliases while preserving complete owner and span identities.
pub(super) fn collect(type_info: &TypeCheckInfo) -> Vec<bir::SdkAsyncPrimitive> {
    let mut cache = StdlibAstCache::new();
    let mut retained = std::collections::BTreeMap::new();
    let identities = type_info
        .references
        .resolved_identities
        .values()
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    for identity in identities {
        let Some(source) = cache.callable_source_identity(&identity) else {
            continue;
        };
        let SymbolOrigin::Module(path) = &source.origin else {
            continue;
        };
        let Some((kind, expected)) = operation(path, &source.declaration_name) else {
            continue;
        };
        let Some(SymbolKind::Function(binding)) = cache.lookup_function_symbol(path, &source.declaration_name) else {
            continue;
        };
        let parameters: Vec<_> = binding
            .params
            .iter()
            .map(|parameter| semantic_type_from_resolved(&parameter.ty))
            .collect();
        if !binding.is_async
            || parameters != expected
            || semantic_type_from_resolved(&binding.return_type) != IncanType::Primitive(IncanPrimitiveType::Unit)
            || binding
                .params
                .iter()
                .any(|parameter| parameter.has_default || parameter.is_mut || parameter.kind != ast::ParamKind::Normal)
        {
            continue;
        }
        retained.insert(
            identity.clone(),
            bir::SdkAsyncPrimitive {
                canonical: identity.clone(),
                kind,
                parameters,
            },
        );
    }
    retained.into_values().collect()
}

/// Select only the declaring SDK modules; a facade or a same-spelled source function cannot mint this fact.
fn operation(path: &[String], name: &str) -> Option<(bir::SdkAsyncPrimitiveKind, Vec<IncanType>)> {
    use bir::SdkAsyncPrimitiveKind::{SleepMillis, SleepSeconds, YieldNow};
    let [root, family, module] = path else {
        return None;
    };
    if root != "std" || family != "async" {
        return None;
    }
    match (module.as_str(), name) {
        ("time", "sleep") => Some((SleepSeconds, vec![IncanType::Primitive(IncanPrimitiveType::Float)])),
        ("time", "sleep_ms") => Some((SleepMillis, vec![IncanType::Primitive(IncanPrimitiveType::Int)])),
        ("task", "yield_now") => Some((YieldNow, Vec::new())),
        _ => None,
    }
}
