//! Project rustc definitions into the shared graph schema. Source coordinates never enter declaration identities.

use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

use incan_codegraph::{
    CodegraphDeclarationRecord, CodegraphFileRecord, CodegraphImportBinding, CodegraphImportRecord, CodegraphLanguage,
    CodegraphModuleRecord, CodegraphProvenance, CodegraphRecord, CodegraphSourceSpan, CodegraphStableDeclarationId,
    CodegraphStableDeclarationLocation, CodegraphSymbolOrigin,
};
use rustc_hir::{ItemKind, def::DefKind, def_id::DefId};
use rustc_middle::ty::TyCtxt;
use rustc_span::{FileName, Pos, Span};

/// A logical definition path, replacing rustc's traversal-numbered impl segments with their semantic owner.
pub fn path(tcx: TyCtxt<'_>, def: DefId) -> String {
    let def = declaration(tcx, def);
    if let DefKind::Impl { of_trait } = tcx.def_kind(def) {
        let self_ty = tcx.type_of(def).instantiate_identity().skip_normalization();
        let trait_name = of_trait.then(|| {
            let reference = tcx.impl_trait_ref(def).instantiate_identity().skip_normalization();
            let arguments = reference
                .args
                .iter()
                .skip(1)
                .map(|argument| argument.to_string())
                .collect::<Vec<_>>();
            let suffix = if arguments.is_empty() {
                String::new()
            } else {
                format!("<{}>", arguments.join(", "))
            };
            format!("{}{suffix}", path(tcx, reference.def_id))
        });
        return format!(
            "{}::<{}{}>",
            path(tcx, tcx.parent(def)),
            self_ty,
            trait_name.map(|name| format!(" as {name}")).unwrap_or_default()
        );
    }
    let Some(parent) = tcx.opt_parent(def) else {
        return tcx.crate_name(def.krate).to_string();
    };
    let name = tcx
        .opt_item_name(def)
        .map(|name| name.to_string())
        .unwrap_or_else(|| tcx.def_path_str(def));
    format!("{}::{name}", path(tcx, parent))
}

/// A span-free key; signatures separate impl blocks while methods retain their semantic self/trait owner.
pub fn identity(tcx: TyCtxt<'_>, def: DefId) -> CodegraphStableDeclarationId {
    let def = declaration(tcx, def);
    let logical = path(tcx, def);
    let signature = match tcx.def_kind(def) {
        DefKind::Fn | DefKind::AssocFn => Some(tcx.fn_sig(def).instantiate_identity().skip_normalization().to_string()),
        DefKind::Impl { .. } => Some(
            tcx.associated_item_def_ids(def)
                .iter()
                .map(|item| path(tcx, *item))
                .collect::<Vec<_>>()
                .join(","),
        ),
        _ => Some(logical.clone()),
    };
    CodegraphStableDeclarationId {
        namespace: "ordinary_lexical".to_owned(),
        origin: CodegraphSymbolOrigin::RustCrate {
            path: logical.split("::").map(str::to_owned).collect(),
        },
        declaration_name: tcx
            .opt_item_name(def)
            .map(|name| name.to_string())
            .unwrap_or_else(|| logical.clone()),
        declaration_kind: kind(tcx.def_kind(def)).to_owned(),
        location: CodegraphStableDeclarationLocation::ModuleLevel,
        signature,
    }
}

/// Tuple and unit constructors refer to the declared struct or enum variant rather than an unexported synthetic item.
fn declaration(tcx: TyCtxt<'_>, def: DefId) -> DefId {
    if matches!(tcx.def_kind(def), DefKind::Ctor(..)) {
        tcx.parent(def)
    } else {
        def
    }
}

/// Export-local linkage derived from the durable identity, never from rustc's local definition index.
pub fn id(tcx: TyCtxt<'_>, def: DefId) -> String {
    let stable = identity(tcx, def);
    let text = format!("{:?}", stable);
    format!(
        "rust:{}:{}",
        path(tcx, def),
        hex::encode(Sha256::digest(text.as_bytes()))
    )
}

/// Map rustc categories into language-independent declaration kinds.
pub fn kind(kind: DefKind) -> &'static str {
    match kind {
        DefKind::Fn => "function",
        DefKind::AssocFn => "method",
        DefKind::Struct => "struct",
        DefKind::Enum => "enum",
        DefKind::Trait => "trait",
        DefKind::Impl { .. } => "impl",
        DefKind::Mod => "module",
        DefKind::TyAlias | DefKind::AssocTy => "type_alias",
        DefKind::Const { .. } | DefKind::AssocConst { .. } => "const",
        DefKind::Static { .. } => "static",
        DefKind::Variant => "enum_variant",
        DefKind::Field => "field",
        _ => "declaration",
    }
}

/// Project a real source span, including byte offsets relative to its own source file; expansions use call sites.
pub fn span(tcx: TyCtxt<'_>, span: Span) -> Option<CodegraphSourceSpan> {
    let map = tcx.sess.source_map();
    let span = span.source_callsite();
    if span.is_dummy() {
        return None;
    }
    let start = map.lookup_char_pos(span.lo());
    let end = map.lookup_char_pos(span.hi());
    let FileName::Real(file) = &start.file.name else {
        return None;
    };
    Some(CodegraphSourceSpan {
        file: file.local_path()?.to_string_lossy().into_owned(),
        start: (span.lo() - start.file.start_pos).to_usize(),
        end: (span.hi() - start.file.start_pos).to_usize(),
        start_line: start.line,
        start_column: start.col.to_usize() + 1,
        end_line: end.line,
        end_column: end.col.to_usize() + 1,
    })
}

/// The module owning a definition, skipping impl and trait owners while keeping the source declaration identity.
pub fn module(tcx: TyCtxt<'_>, mut def: DefId) -> DefId {
    while tcx.def_kind(def) != DefKind::Mod {
        let Some(parent) = tcx.opt_parent(def) else {
            break;
        };
        def = parent;
    }
    def
}

/// Module nodes and their declaration nodes have distinct linkage ids, as they do in the Incan exporter.
pub fn module_id(tcx: TyCtxt<'_>, def: DefId) -> String {
    format!("module:{}", id(tcx, module(tcx, def)))
}

/// Return whether this rustc definition has a declaration record in the source graph.
fn represented_declaration(kind: DefKind) -> bool {
    matches!(
        kind,
        DefKind::Fn
            | DefKind::AssocFn
            | DefKind::Struct
            | DefKind::Enum
            | DefKind::Trait
            | DefKind::Impl { .. }
            | DefKind::Mod
            | DefKind::TyAlias
            | DefKind::AssocTy
            | DefKind::Const { .. }
            | DefKind::AssocConst { .. }
            | DefKind::Static { .. }
            | DefKind::Variant
            | DefKind::Field
    )
}

/// Link a body to its nearest represented declaration through rustc's parent relation. A closure expression keeps
/// its own type tables and fact IDs, but navigation follows the enclosing declaration rather than a missing node.
pub fn owner_id(tcx: TyCtxt<'_>, mut def: DefId) -> String {
    while !represented_declaration(tcx.def_kind(def)) {
        let Some(parent) = tcx.opt_parent(def) else {
            break;
        };
        def = parent;
    }
    id(tcx, def)
}

/// Export all local declarations, including private and associated items; macro definitions are outside this slice.
pub fn declarations(tcx: TyCtxt<'_>, records: &mut Vec<CodegraphRecord>) {
    let mut files = BTreeSet::new();
    for local in tcx.iter_local_def_id() {
        let def = local.to_def_id();
        if !represented_declaration(tcx.def_kind(def)) {
            continue;
        }
        let source = span(tcx, tcx.def_span(def));
        if let Some(source) = &source {
            if files.insert(source.file.clone()) {
                records.push(CodegraphRecord::File(CodegraphFileRecord {
                    id: format!("file:{}", source.file),
                    language: CodegraphLanguage::Rust,
                    path: source.file.clone(),
                    size_bytes: std::fs::metadata(&source.file)
                        .map(|file| usize::try_from(file.len()).unwrap_or(0))
                        .unwrap_or(0),
                    provenance: CodegraphProvenance::Source,
                    degraded: false,
                }));
            }
        }
        if tcx.def_kind(def) == DefKind::Mod {
            let module_path: Vec<_> = path(tcx, def).split("::").map(str::to_owned).collect();
            records.push(CodegraphRecord::Module(CodegraphModuleRecord {
                id: module_id(tcx, def),
                language: CodegraphLanguage::Rust,
                file_id: source
                    .as_ref()
                    .map(|span| format!("file:{}", span.file))
                    .unwrap_or_default(),
                module_path: module_path.clone(),
                namespace_path: module_path,
                internal: false,
                name: path(tcx, def),
                span: source.clone(),
                provenance: CodegraphProvenance::Checked,
                degraded: false,
            }));
        }
        let stable = identity(tcx, def);
        records.push(CodegraphRecord::Declaration(CodegraphDeclarationRecord {
            id: id(tcx, def),
            language: CodegraphLanguage::Rust,
            module_id: module_id(tcx, def),
            kind: kind(tcx.def_kind(def)).to_owned(),
            name: stable.declaration_name.clone(),
            visibility: if tcx.visibility(def).is_public() {
                "public"
            } else {
                "private"
            }
            .to_owned(),
            type_params: Vec::new(),
            signature: stable.signature.clone(),
            canonical_identity: None,
            stable_identity: Some(stable),
            semantic_digest: None,
            doc_digest: None,
            span: source,
            provenance: CodegraphProvenance::Checked,
            degraded: false,
        }));
    }
    imports(tcx, records);
}

/// Retain resolved source imports and their aliases, excluding rustc's synthetic prelude import.
fn imports(tcx: TyCtxt<'_>, records: &mut Vec<CodegraphRecord>) {
    for item in tcx.hir_crate_items(()).free_items() {
        let item = tcx.hir_item(item);
        if let ItemKind::Use(path, _) = &item.kind {
            let Some(source) = span(tcx, item.span) else {
                continue;
            };
            let mut targets: Vec<_> = path.res.iter().flatten().filter_map(|res| res.opt_def_id()).collect();
            targets.sort_by_cached_key(|def| crate::rust_graph_records::path(tcx, *def));
            targets.dedup();
            let alias = item
                .kind
                .ident()
                .map(|ident| ident.name.to_string())
                .unwrap_or_default();
            let owner = module(tcx, tcx.parent(item.owner_id.to_def_id()));
            records.push(CodegraphRecord::Import(CodegraphImportRecord {
                id: format!("import:{}", tcx.def_path_str(item.owner_id)),
                language: CodegraphLanguage::Rust,
                module_id: module_id(tcx, owner),
                kind: "use".to_owned(),
                path: path
                    .segments
                    .iter()
                    .map(|segment| segment.ident.name.to_string())
                    .collect::<Vec<_>>()
                    .join("::"),
                items: targets
                    .iter()
                    .map(|def| crate::rust_graph_records::path(tcx, *def))
                    .collect(),
                bindings: targets
                    .iter()
                    .map(|def| CodegraphImportBinding {
                        local_name: alias.clone(),
                        canonical_identity: None,
                        stable_identity: Some(identity(tcx, *def)),
                    })
                    .collect(),
                alias: Some(alias),
                visibility: if tcx.visibility(item.owner_id).is_public() {
                    "public"
                } else {
                    "private"
                }
                .to_owned(),
                span: Some(source),
                provenance: CodegraphProvenance::Checked,
                degraded: false,
            }));
        }
    }
}
