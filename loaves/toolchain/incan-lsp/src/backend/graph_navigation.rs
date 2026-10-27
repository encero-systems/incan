//! Editor navigation is a projection of the exporter's records from the current checking pass, including unsaved
//! buffers. Captured source text maps byte spans to UTF-16; it never supplies an independently resolved target.

use std::collections::{BTreeMap, HashMap};

use incan_codegraph::{CodegraphCanonicalSymbolId, CodegraphDeclarationRecord, CodegraphRecord, CodegraphSourceSpan};
use incan_driver::diagnostics::CliDiagnosticFailure;
use incan_driver::inspect::codegraph::{codegraph_canonical_identity, collect_live_unit_codegraph};
use incan_driver::session::{CompilationAnalysis, CompilationSession};
use incan_frontend::parsed_module::ParsedModule;
use incan_semantics_core::CanonicalSymbolId;
use tower_lsp::lsp_types::{
    CallHierarchyIncomingCall, CallHierarchyItem, CallHierarchyOutgoingCall, Location, Position, SymbolKind, Url,
};

use super::DocumentState;

/// One live export with the exact buffers its ranges describe. A document version binds retained hierarchy items
/// to this analysis; dependent open-buffer changes invalidate the view before another navigation answer is served.
#[derive(Debug, Clone)]
pub(super) struct GraphSnapshot {
    records: Vec<CodegraphRecord>,
    declarations: HashMap<String, usize>,
    canonical_declarations: BTreeMap<CodegraphCanonicalSymbolId, usize>,
    sources: BTreeMap<String, String>,
    version: i32,
}

impl GraphSnapshot {
    /// Retain the compiler export directly, preserving its source paths and ids without an artifact round trip.
    /// Failed checking supplies degraded records with no checked navigation relationships.
    pub(super) fn from_analysis(
        session: &CompilationSession,
        modules: &[ParsedModule],
        analysis: std::result::Result<&CompilationAnalysis, &CliDiagnosticFailure>,
        version: i32,
    ) -> std::result::Result<Self, Box<dyn std::error::Error>> {
        let records = collect_live_unit_codegraph(session, modules, analysis);
        let mut declarations = HashMap::new();
        let mut canonical_declarations = BTreeMap::new();
        for (index, record) in records.iter().enumerate() {
            let CodegraphRecord::Declaration(declaration) = record else {
                continue;
            };
            if declaration.degraded {
                continue;
            }
            declarations.entry(declaration.id.clone()).or_insert(index);
            if let Some(identity) = &declaration.canonical_identity {
                canonical_declarations.entry(identity.clone()).or_insert(index);
            }
        }
        let sources = modules
            .iter()
            .map(|module| (module.file_path.to_string_lossy().into_owned(), module.source.clone()))
            .collect();
        Ok(Self {
            records,
            declarations,
            canonical_declarations,
            sources,
            version,
        })
    }

    /// A dependency view is current only while every captured open buffer still has the captured text.
    pub(super) fn is_current(&self, documents: &HashMap<Url, DocumentState>) -> bool {
        documents.iter().all(|(uri, document)| {
            let Ok(path) = uri.to_file_path() else {
                return true;
            };
            self.sources
                .get(path.to_string_lossy().as_ref())
                .is_none_or(|source| source == &document.source)
        })
    }

    /// Find a declaration by its compiler-proven identity, never by a matching spelling or source line.
    fn declaration_for(&self, identity: &CanonicalSymbolId) -> Option<&CodegraphDeclarationRecord> {
        let canonical = codegraph_canonical_identity(identity);
        self.declaration_at(*self.canonical_declarations.get(&canonical)?)
    }

    /// Resolve an artifact-local relationship endpoint to the declaration the exporter linked it to.
    fn declaration(&self, id: &str) -> Option<&CodegraphDeclarationRecord> {
        self.declaration_at(*self.declarations.get(id)?)
    }

    /// Read a declaration at an index retained from this snapshot's immutable record vector.
    fn declaration_at(&self, index: usize) -> Option<&CodegraphDeclarationRecord> {
        match self.records.get(index)? {
            CodegraphRecord::Declaration(declaration) => Some(declaration),
            _ => None,
        }
    }

    /// Convert a recorded byte span against the captured buffer rather than whatever bytes happen to be on disk.
    fn location(&self, span: &CodegraphSourceSpan) -> Option<Location> {
        let source = self.sources.get(&span.file)?;
        Some(Location {
            uri: Url::from_file_path(&span.file).ok()?,
            range: super::span_to_range(source, span.start, span.end),
        })
    }

    /// Present a declaration and attach the live document version so a delayed hierarchy request cannot use old facts.
    fn item(&self, declaration: &CodegraphDeclarationRecord) -> Option<CallHierarchyItem> {
        let location = self.location(declaration.span.as_ref()?)?;
        let kind = match declaration.kind.as_str() {
            "function" => SymbolKind::FUNCTION,
            "method" => SymbolKind::METHOD,
            "model" | "struct" | "class" => SymbolKind::STRUCT,
            "enum" => SymbolKind::ENUM,
            "const" => SymbolKind::CONSTANT,
            _ => SymbolKind::VARIABLE,
        };
        Some(CallHierarchyItem {
            name: declaration.name.clone(),
            kind,
            tags: None,
            detail: declaration.signature.clone(),
            uri: location.uri,
            range: location.range,
            selection_range: location.range,
            data: Some(serde_json::json!({"id": declaration.id, "documentVersion": self.version})),
        })
    }

    /// Validate a retained item against the current snapshot before traversing its proved call edges.
    fn selected(&self, item: &CallHierarchyItem) -> Option<&str> {
        let data = item.data.as_ref()?;
        if data.get("documentVersion")?.as_i64()? != i64::from(self.version) {
            return None;
        }
        let id = data.get("id")?.as_str()?;
        Some(&self.declaration(id)?.id)
    }
}

/// Invalidate every view that captured a buffer before its new analysis or close notification is published.
pub(super) fn invalidate(uri: &Url, documents: &mut HashMap<Url, DocumentState>) {
    let Ok(path) = uri.to_file_path() else {
        return;
    };
    for document in documents.values_mut() {
        if document
            .graph_navigation
            .as_ref()
            .is_some_and(|graph| graph.sources.contains_key(path.to_string_lossy().as_ref()))
        {
            document.graph_navigation = None;
        }
    }
}

/// A graph-backed definition shares the exporter's identity and source range; stale views return no target.
pub(super) fn definition(
    document: &DocumentState,
    documents: &HashMap<Url, DocumentState>,
    identity: &CanonicalSymbolId,
) -> Option<Location> {
    let graph = document.graph_navigation.as_ref()?;
    if !graph.is_current(documents) {
        return None;
    }
    graph.location(graph.declaration_for(identity)?.span.as_ref()?)
}

/// Read checked references from current live graph records across open documents, deduplicating captured modules.
pub(super) fn references(
    document: &DocumentState,
    documents: &HashMap<Url, DocumentState>,
    identity: &CanonicalSymbolId,
    include_declaration: bool,
) -> Option<Vec<Location>> {
    if !document.graph_navigation.as_ref()?.is_current(documents) {
        return None;
    }
    let canonical = codegraph_canonical_identity(identity);
    let mut locations = Vec::new();
    for graph in documents
        .values()
        .filter_map(|document| document.graph_navigation.as_ref())
        .filter(|graph| graph.is_current(documents))
    {
        for record in &graph.records {
            let span = match record {
                CodegraphRecord::Reference(reference)
                    if !reference.degraded && reference.canonical_identity.as_ref() == Some(&canonical) =>
                {
                    reference.span.as_ref()
                }
                CodegraphRecord::Declaration(declaration)
                    if include_declaration
                        && !declaration.degraded
                        && declaration.canonical_identity.as_ref() == Some(&canonical) =>
                {
                    declaration.span.as_ref()
                }
                _ => None,
            };
            if let Some(location) = span.and_then(|span| graph.location(span))
                && !locations.contains(&location)
            {
                locations.push(location);
            }
        }
    }
    locations.sort_by(|left, right| {
        (left.uri.as_str(), left.range.start.line, left.range.start.character).cmp(&(
            right.uri.as_str(),
            right.range.start.line,
            right.range.start.character,
        ))
    });
    Some(locations)
}

/// Prepare hierarchy for the checked declaration at the cursor, using the same occurrence ownership as definitions.
pub(super) fn prepare(
    uri: &Url,
    position: Position,
    documents: &HashMap<Url, DocumentState>,
) -> Option<Vec<CallHierarchyItem>> {
    let document = documents.get(uri)?;
    let graph = document.graph_navigation.as_ref()?;
    if !graph.is_current(documents) {
        return None;
    }
    let ast = document.ast.as_ref()?;
    let offset = super::position_to_offset(&document.source, position)?;
    if super::embedded_dsl_ownership_at_offset(ast, offset).is_some() {
        return None;
    }
    let occurrence =
        super::checked_identity_at_offset(uri, ast, &document.source, &document.checked_identities, offset)?;
    Some(vec![graph.item(graph.declaration_for(&occurrence.identity)?)?])
}

/// Incoming calls follow only recorded target IDs, retaining real call-site ranges and grouping calls by owner.
pub(super) fn incoming(
    item: &CallHierarchyItem,
    documents: &HashMap<Url, DocumentState>,
) -> Option<Vec<CallHierarchyIncomingCall>> {
    let graph = selected_graph(item, documents)?;
    let target = graph.selected(item)?;
    let mut calls: BTreeMap<String, CallHierarchyIncomingCall> = BTreeMap::new();
    for record in &graph.records {
        let CodegraphRecord::Call(call) = record else {
            continue;
        };
        if call.degraded || call.target_id.as_deref() != Some(target) {
            continue;
        }
        let Some(owner) = call.owner_id.as_deref().and_then(|id| graph.declaration(id)) else {
            continue;
        };
        let Some(location) = call.span.as_ref().and_then(|span| graph.location(span)) else {
            continue;
        };
        let Some(from) = graph.item(owner) else {
            continue;
        };
        calls
            .entry(owner.id.clone())
            .or_insert_with(|| CallHierarchyIncomingCall {
                from,
                from_ranges: Vec::new(),
            })
            .from_ranges
            .push(location.range);
    }
    Some(calls.into_values().collect())
}

/// Outgoing calls retain unresolved targets as unavailable rather than guessing a declaration from the callee name.
pub(super) fn outgoing(
    item: &CallHierarchyItem,
    documents: &HashMap<Url, DocumentState>,
) -> Option<Vec<CallHierarchyOutgoingCall>> {
    let graph = selected_graph(item, documents)?;
    let owner = graph.selected(item)?;
    let mut calls: BTreeMap<String, CallHierarchyOutgoingCall> = BTreeMap::new();
    for record in &graph.records {
        let CodegraphRecord::Call(call) = record else {
            continue;
        };
        if call.degraded || call.owner_id.as_deref() != Some(owner) {
            continue;
        }
        let Some(target) = call.target_id.as_deref().and_then(|id| graph.declaration(id)) else {
            continue;
        };
        let Some(location) = call.span.as_ref().and_then(|span| graph.location(span)) else {
            continue;
        };
        let Some(to) = graph.item(target) else {
            continue;
        };
        calls
            .entry(target.id.clone())
            .or_insert_with(|| CallHierarchyOutgoingCall {
                to,
                from_ranges: Vec::new(),
            })
            .from_ranges
            .push(location.range);
    }
    Some(calls.into_values().collect())
}

/// The item's URI identifies its captured source, which may be an imported module rather than the entry document.
fn selected_graph<'documents>(
    item: &CallHierarchyItem,
    documents: &'documents HashMap<Url, DocumentState>,
) -> Option<&'documents GraphSnapshot> {
    let path = item.uri.to_file_path().ok()?;
    documents
        .values()
        .filter_map(|document| document.graph_navigation.as_ref())
        .find(|graph| {
            graph.is_current(documents)
                && graph.selected(item).is_some()
                && graph.sources.contains_key(path.to_string_lossy().as_ref())
        })
}
