//! Declaration facts, stable and canonical identities, record languages, and record ids.

use super::*;

/// Read early checked source identities without constructing Body IR or waiting for executable representations.
pub(super) fn source_declaration_facts(
    modules: &[ParsedModule],
    analysis: &CompilationAnalysis,
) -> BTreeMap<CanonicalSymbolId, CheckedDeclarationFacts> {
    modules
        .iter()
        .filter_map(|module| analysis.type_info_for_module_path(&module.path_segments))
        .flat_map(|info| &info.declarations.source_identities)
        .map(|(canonical, identity)| {
            (
                canonical.clone(),
                CheckedDeclarationFacts {
                    source_identity: Some(identity.clone()),
                    ..CheckedDeclarationFacts::default()
                },
            )
        })
        .collect()
}

/// Retain early source identities even when executable digest construction is unavailable.
pub(super) fn checked_declaration_facts(
    modules: &[ParsedModule],
    analysis: &CompilationAnalysis,
    session: &CompilationSession,
) -> BTreeMap<CanonicalSymbolId, CheckedDeclarationFacts> {
    let mut facts = source_declaration_facts(modules, analysis);
    if let Ok(mut bodies) = crate::body_ir::checked_modules(modules, analysis, session) {
        append_body_declaration_facts(&mut facts, &mut bodies);
    }
    facts
}

/// Attach executable digests without deriving, replacing or refusing a source signature. A leading docstring is
/// temporarily detached by the shared digest boundary and restored before compilation consumes the body.
pub(super) fn append_body_declaration_facts(
    facts: &mut BTreeMap<CanonicalSymbolId, CheckedDeclarationFacts>,
    bodies: &mut [incan_semantics_core::body_ir::BodyIrModule],
) {
    use incan_semantics_core::semantic_digest::{body_docstring, body_semantic_digest, semantic_digest};

    for body in bodies.iter_mut().flat_map(|module| &mut module.bodies) {
        let Some(canonical) = &body.canonical else { continue };
        let fact = facts.entry(canonical.clone()).or_default();
        fact.semantic_digest = body_semantic_digest(body).ok();
        fact.doc_digest = body_docstring(body).and_then(|text| semantic_digest(&text).ok());
    }
}

/// Project a checked identity into the edit-stable form exported beside the span-carrying one.
///
/// Derived from [`StableDeclarationId`] rather than rebuilt here, so the exported identity and the compiler's own
/// cannot drift: an exported identity that keyed on spans would rebuild, one layer out, the defect that type exists
/// to remove.
pub(super) fn codegraph_stable_identity(
    identity: &CanonicalSymbolId,
    context: Option<StableDeclarationContext>,
) -> Option<CodegraphStableDeclarationId> {
    let stable = StableDeclarationId::from_canonical(identity, None, context)?;
    Some(codegraph_stable_identity_from_semantic(&stable))
}

/// Convert the compiler-owned stable identity into the storage-neutral codegraph shape.
pub(super) fn codegraph_stable_identity_from_semantic(stable: &StableDeclarationId) -> CodegraphStableDeclarationId {
    CodegraphStableDeclarationId {
        namespace: match stable.namespace {
            incan_semantics_core::SymbolNamespace::OrdinaryLexical => "ordinary_lexical",
            incan_semantics_core::SymbolNamespace::Member => "member",
            incan_semantics_core::SymbolNamespace::ModulePath => "module_path",
        }
        .to_string(),
        origin: match &stable.origin {
            SymbolOrigin::Module(path) => CodegraphSymbolOrigin::Module { path: path.clone() },
            SymbolOrigin::Package { library, module_path } => CodegraphSymbolOrigin::Package {
                library: library.clone(),
                module_path: module_path.clone(),
            },
            SymbolOrigin::RustCrate(path) => CodegraphSymbolOrigin::RustCrate { path: path.clone() },
            SymbolOrigin::Builtin => CodegraphSymbolOrigin::Builtin,
        },
        declaration_name: stable.declaration_name.clone(),
        declaration_kind: stable.kind.as_str().to_string(),
        location: match &stable.location {
            StableDeclarationLocation::ModuleLevel => CodegraphStableDeclarationLocation::ModuleLevel,
            StableDeclarationLocation::Nested { owner, binding_ordinal } => {
                CodegraphStableDeclarationLocation::Nested {
                    owner: Box::new(codegraph_stable_identity_from_semantic(owner)),
                    binding_ordinal: *binding_ordinal,
                }
            }
        },
        signature: stable
            .signature
            .as_ref()
            .map(|signature| signature.as_str().to_string()),
    }
}

/// Project the compiler identity into the storage-neutral codegraph wire shape.
///
/// This keeps the span, deliberately. It is the record's provenance — where the declaration sat in the compilation
/// that produced it — and a consumer reporting a location needs it. What a consumer must not do is key on it across
/// compilations; [`codegraph_stable_identity`] is what it keys on instead.
pub fn codegraph_canonical_identity(identity: &CanonicalSymbolId) -> CodegraphCanonicalSymbolId {
    let origin = match &identity.origin {
        SymbolOrigin::Module(path) => CodegraphSymbolOrigin::Module { path: path.clone() },
        SymbolOrigin::Package { library, module_path } => CodegraphSymbolOrigin::Package {
            library: library.clone(),
            module_path: module_path.clone(),
        },
        SymbolOrigin::RustCrate(path) => CodegraphSymbolOrigin::RustCrate { path: path.clone() },
        SymbolOrigin::Builtin => CodegraphSymbolOrigin::Builtin,
    };
    CodegraphCanonicalSymbolId {
        namespace: match identity.namespace {
            incan_semantics_core::SymbolNamespace::OrdinaryLexical => "ordinary_lexical",
            incan_semantics_core::SymbolNamespace::Member => "member",
            incan_semantics_core::SymbolNamespace::ModulePath => "module_path",
        }
        .to_string(),
        origin,
        declaration_name: identity.declaration_name.clone(),
        declaration_kind: identity.kind.as_str().to_string(),
        scope_discriminant: identity.scope_discriminant.map(|scope| scope.0),
        declaration_span: CodegraphIdentitySpan {
            start: identity.declaration_span.start,
            end: identity.declaration_span.end,
        },
    }
}

/// Return the languages the export's facts are written in, `incan` first (RFC 106: the header lists the languages
/// represented by the export, and `language` is an attribute of each fact).
///
/// Incan is always represented: every export carries the Incan file and module facts of its sources. Rust is
/// represented when any fact is a Rust one, such as the import of a `rust::` item or a reference resolved into a Rust
/// crate (#1561).
pub(super) fn represented_languages(records: &[CodegraphRecord]) -> Vec<CodegraphLanguage> {
    let rust = records
        .iter()
        .any(|record| record_language(record) == Some(CodegraphLanguage::Rust));
    let mut languages = vec![CodegraphLanguage::Incan];
    if rust {
        languages.push(CodegraphLanguage::Rust);
    }
    languages
}

/// Return the language one fact is written in, or `None` for the header, which describes the export.
pub(super) fn record_language(record: &CodegraphRecord) -> Option<CodegraphLanguage> {
    match record {
        CodegraphRecord::Header(_) => None,
        CodegraphRecord::File(record) => Some(record.language),
        CodegraphRecord::Namespace(record) => Some(record.language),
        CodegraphRecord::Module(record) => Some(record.language),
        CodegraphRecord::Declaration(record) => Some(record.language),
        CodegraphRecord::Import(record) => Some(record.language),
        CodegraphRecord::Export(record) => Some(record.language),
        CodegraphRecord::Reference(record) => Some(record.language),
        CodegraphRecord::Call(record) => Some(record.language),
        CodegraphRecord::Containment(record) => Some(record.language),
        CodegraphRecord::Diagnostic(record) => Some(record.language),
        CodegraphRecord::Registry(record) => Some(record.language),
        CodegraphRecord::CBinding(record) => Some(record.language),
        CodegraphRecord::CBindingCall(record) => Some(record.language),
        CodegraphRecord::CBindingFacade(record) => Some(record.language),
        CodegraphRecord::Capability(record) => Some(record.language),
    }
}

/// Read the degraded flag from any codegraph record variant.
pub(super) fn record_degraded(record: &CodegraphRecord) -> bool {
    match record {
        CodegraphRecord::Header(record) => record.degraded,
        CodegraphRecord::File(record) => record.degraded,
        CodegraphRecord::Namespace(record) => record.degraded,
        CodegraphRecord::Module(record) => record.degraded,
        CodegraphRecord::Declaration(record) => record.degraded,
        CodegraphRecord::Import(record) => record.degraded,
        CodegraphRecord::Export(record) => record.degraded,
        CodegraphRecord::Reference(record) => record.degraded,
        CodegraphRecord::Call(record) => record.degraded,
        CodegraphRecord::Containment(record) => record.degraded,
        CodegraphRecord::Diagnostic(record) => record.degraded,
        CodegraphRecord::Registry(record) => record.degraded,
        CodegraphRecord::CBinding(record) => record.degraded,
        CodegraphRecord::CBindingCall(record) => record.degraded,
        CodegraphRecord::CBindingFacade(record) => record.degraded,
        CodegraphRecord::Capability(record) => record.degraded,
    }
}

/// Construct a deterministic id for one capability declaration.
///
/// Module path, declaration name, and span start together stay unique within an export without depending on the
/// order modules were analyzed in, which is what keeps a JSONL export byte-stable between runs.
pub(super) fn capability_record_id(identity: &CanonicalSymbolId, anchor_start: usize) -> String {
    let module = identity.module_path().map(|path| path.join(".")).unwrap_or_default();
    format!(
        "capability:{}:{}:{anchor_start}",
        sanitize_record_label(&module),
        sanitize_record_label(&identity.declaration_name)
    )
}

/// Construct a deterministic id for one registry entry without depending on runtime loading order.
pub(super) fn registry_record_id(registry_identity: &str, subject_identity: &str, anchor_start: usize) -> String {
    format!(
        "registry:{}:{}:{anchor_start}",
        sanitize_record_label(registry_identity),
        sanitize_record_label(subject_identity)
    )
}

/// Convert one checked registry value into the codegraph's schema-stable JSON representation.
pub(super) fn checked_registry_value_json(value: &CheckedRegistryValue) -> Value {
    match value {
        CheckedRegistryValue::Int(value) => json!({ "kind": "int", "value": value }),
        CheckedRegistryValue::Float(value) => json!({ "kind": "float", "value": value }),
        CheckedRegistryValue::Bool(value) => json!({ "kind": "bool", "value": value }),
        CheckedRegistryValue::String(value) => {
            let visible = incan_frontend::source_security::escape_bidirectional_controls(value);
            json!({ "kind": "string", "value": visible.as_ref() })
        }
        CheckedRegistryValue::Bytes(value) => json!({ "kind": "bytes", "value": value }),
        CheckedRegistryValue::None => json!({ "kind": "none", "value": null }),
        CheckedRegistryValue::Type(value) => json!({ "kind": "type", "value": value }),
        CheckedRegistryValue::Option(value) => json!({
            "kind": "option",
            "value": checked_registry_value_json(value),
        }),
        CheckedRegistryValue::List(values) => json!({
            "kind": "list",
            "value": values.iter().map(checked_registry_value_json).collect::<Vec<_>>(),
        }),
        CheckedRegistryValue::Dict(entries) => json!({
            "kind": "dict",
            "value": entries.iter().map(|entry| json!({
                "key": checked_registry_value_json(&entry.key),
                "value": checked_registry_value_json(&entry.value),
            })).collect::<Vec<_>>(),
        }),
        CheckedRegistryValue::ConstRef(path) => json!({ "kind": "const_ref", "value": path }),
        CheckedRegistryValue::Newtype { name, value } => json!({
            "kind": "newtype",
            "value": { "name": name, "value": checked_registry_value_json(value) },
        }),
        CheckedRegistryValue::Model { name, fields } => json!({
            "kind": "model",
            "value": {
                "name": name,
                "fields": fields.iter().map(|field| json!({
                    "name": field.name,
                    "value": checked_registry_value_json(&field.value),
                })).collect::<Vec<_>>(),
            },
        }),
    }
}

/// Replace source controls before prose enters an inspection record or terminal-facing serialization.
pub(super) fn visible_source_text(value: &str) -> String {
    incan_frontend::source_security::escape_bidirectional_controls(value).into_owned()
}

#[cfg(test)]
mod source_security_tests {
    use super::{checked_registry_value_json, visible_source_text};
    use incan_frontend::registry_metadata::CheckedRegistryValue;

    /// Inspection values and prose expose visible escapes rather than raw bidirectional controls.
    #[test]
    fn inspection_text_escapes_bidirectional_controls() {
        let rendered = checked_registry_value_json(&CheckedRegistryValue::String("left\u{202e}right".to_string()));
        assert_eq!(rendered["value"], "left\\u{202e}right");
        assert_eq!(visible_source_text("left\u{2067}right"), "left\\u{2067}right");
    }
}

/// Return the codegraph spelling for one checked registry subject kind.
pub(super) const fn checked_registry_subject_kind(kind: CheckedRegistrySubjectKind) -> &'static str {
    match kind {
        CheckedRegistrySubjectKind::Function => "function",
        CheckedRegistrySubjectKind::Method => "method",
        CheckedRegistrySubjectKind::CompilationUnit => "compilation_unit",
        CheckedRegistrySubjectKind::Package => "package",
    }
}

/// Return the language a body fact belongs to, taken from the identity the compiler gave what it resolved to.
///
/// RFC 106 keeps one graph across both languages and makes `language` an attribute of a fact rather than a separate
/// node kind, so a reference the compiler resolved to a Rust crate item is a Rust fact even though the source that
/// wrote it is Incan. Only a checked identity decides this. Where nothing was proven the fact stays Incan, the
/// language of the module that wrote it: matching a name against the module's Rust imports would infer origin from
/// a spelling, which is the mistake RFC 120 exists to prevent and which an alias defeats immediately.
pub(super) fn reference_language(identity: Option<&CanonicalSymbolId>) -> CodegraphLanguage {
    match identity.map(|identity| &identity.origin) {
        Some(SymbolOrigin::RustCrate(_)) => CodegraphLanguage::Rust,
        _ => CodegraphLanguage::Incan,
    }
}

/// Return provenance for body facts that may carry compiler-checked canonical identity.
pub(super) fn provenance_for_identity(identity: Option<&CanonicalSymbolId>) -> CodegraphProvenance {
    if identity.is_some() {
        CodegraphProvenance::Checked
    } else {
        CodegraphProvenance::Syntax
    }
}

/// Return the source node on which expression checking records a direct callee identity.
pub(super) fn codegraph_callee_identity_span(callee: &Spanned<Expr>) -> Span {
    match &callee.node {
        Expr::Paren(inner) => codegraph_callee_identity_span(inner),
        _ => callee.span,
    }
}
