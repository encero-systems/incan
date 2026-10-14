//! Source-verified SDK rusttype enum contracts; the adapter verifies payloads against linked native metadata.

use incan_semantics_core::{
    CanonicalSymbolId, CompilerNodeId, SemanticSourceTargetKind, SymbolNamespace, SymbolOrigin,
};

use super::{IncanType, TypeCheckInfo, bir};
use crate::symbols::{ResolvedType, TypeInfo};
use crate::typechecker::stdlib_loader::StdlibAstCache;

/// Retain the native timeout outcome only when the checked stdlib alias names its exact runtime backing type.
/// Source or foreign declarations, even with the same spelling, cannot acquire this native payload layout.
pub(super) fn collect(facts: &TypeCheckInfo) -> Vec<bir::EnumDeclaration> {
    let mut cache = StdlibAstCache::new();
    super::library_nominals::type_identities(facts, &mut cache)
        .iter()
        .filter_map(|identity| {
            if let Some(bound) = facts
                .declarations
                .named_type_identities
                .get(&identity.declaration_name)
                .or_else(|| {
                    facts
                        .declarations
                        .body_ir_native_type_identities
                        .get(&identity.declaration_name)
                })
            {
                let bound = super::library_nominals::sdk_identity(bound)?;
                if cache.callable_source_identity(&bound)? != cache.callable_source_identity(identity)? {
                    return None;
                }
            }
            outcome(identity, &mut cache)
        })
        .collect()
}

/// Mirror the public native enum's ordered contract after checking the source alias and its underlying Rust path.
/// This declaration grants pattern payload access only; physical storage always aliases the SDK's linked ADT.
/// Native members use stable synthetic addresses contained in the checked alias's span; native metadata must prove
/// their names, order and payload types before execution.
fn outcome(identity: &CanonicalSymbolId, cache: &mut StdlibAstCache) -> Option<bir::EnumDeclaration> {
    if identity.declaration_name != "TimeoutJoinOutcome" {
        return None;
    }
    let source = cache.callable_source_identity(identity)?;
    let SymbolOrigin::Module(path) = &source.origin else {
        return None;
    };
    if path != &["std", "async", "time"] {
        return None;
    }
    let TypeInfo::Newtype(alias) = cache.lookup_type(path, &identity.declaration_name)? else {
        return None;
    };
    if !alias.is_rusttype
        || alias.type_params != ["T"]
        || alias.underlying != ResolvedType::RustPath("incan_std_async::time::TimeoutJoinOutcome<T>".into())
    {
        return None;
    }
    let mut canonical = source;
    canonical.origin = SymbolOrigin::Package {
        library: "incan_std_async".into(),
        module_path: vec!["time".into()],
    };
    canonical.kind = SemanticSourceTargetKind::Enum;
    let module = incan_semantics_core::canonical_module_identity(&canonical)?;
    let start = canonical.declaration_span.start;
    let end = canonical.declaration_span.end;
    let payloads = [
        ("Completed", IncanType::TypeVar("T".into())),
        ("JoinFailed", IncanType::Named("TaskJoinError".into())),
        (
            "TimedOut",
            IncanType::Generic {
                base: "JoinHandle".into(),
                args: vec![IncanType::TypeVar("T".into())],
            },
        ),
    ];
    let variants = payloads
        .into_iter()
        .enumerate()
        .map(|(index, (name, field))| {
            let mut variant = canonical.clone();
            variant.namespace = SymbolNamespace::Member;
            variant.kind = SemanticSourceTargetKind::Variant;
            variant.declaration_name = name.into();
            variant.declaration_span = incan_semantics_core::HirSourceSpan::new(start + index, start + index + 1);
            bir::EnumVariantDeclaration {
                direct_declaration_id: CompilerNodeId::declaration_span(&module, start + index, start + index + 1),
                canonical: variant,
                name: name.into(),
                fields: vec![field],
            }
        })
        .collect();
    Some(bir::EnumDeclaration {
        direct_declaration_id: CompilerNodeId::declaration_span(&module, start, end),
        canonical,
        name: "TimeoutJoinOutcome".into(),
        public: true,
        type_parameters: vec!["T".into()],
        native_type_arguments: Vec::new(),
        variants,
        derives: Vec::new(),
        // This source-verified native alias grants only linked-runtime pattern access. It does not publish a
        // source-authored payload context or invent identities for its runtime-owned fields.
        named_type_identities: std::collections::BTreeMap::new(),
    })
}

/// Resolve a source-checked SDK outcome pattern against the retained scrutinee; unresolved patterns remain refused.
pub(super) fn pattern_identity(
    declarations: &std::collections::HashMap<CanonicalSymbolId, bir::EnumDeclaration>,
    written: &str,
    destination: &IncanType,
) -> Option<CanonicalSymbolId> {
    let IncanType::Generic { base, args } = destination else {
        return None;
    };
    let (owner, member) = written.rsplit_once("::").or_else(|| written.rsplit_once('.'))?;
    if owner != base {
        return None;
    }
    let declaration = declarations.values().find(|item| {
        item.name == *base
            && item.type_parameters.len() == args.len()
            && matches!(&item.canonical.origin, SymbolOrigin::Package { library, module_path }
            if library == "incan_std_async" && module_path == &["time"])
    })?;
    declaration
        .variants
        .iter()
        .find(|variant| variant.name == member)
        .map(|variant| variant.canonical.clone())
}
