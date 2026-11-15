//! Exported symbols and exported type-like documentation of a module.

use super::*;

/// Extract symbols exported by a module.
///
/// Visibility is enforced for declarations (`pub` only), and module-level `from ... import ...` statements are treated
/// as re-exports by name (including aliases).
///
/// This follows Python semantics: `from foo import bar` at module level makes `bar` part of the module's public
/// surface. The primary use case is stdlib prelude files (e.g., `loaves/stdlib/web/src/web/prelude.incn`) that
/// re-export items from submodules, but it applies uniformly to all modules. The consumer of this list
/// (`validate_import_visibility` in the typechecker) uses it to check whether `from some_module import X` is valid.
pub fn exported_symbols(ast: &Program) -> Vec<ExportedSymbol> {
    let mut exports = Vec::new();

    for decl in &ast.declarations {
        match &decl.node {
            Declaration::Const(c) => {
                if matches!(c.visibility, Visibility::Public) {
                    exports.push(ExportedSymbol::Const(c.name.clone()));
                }
            }
            Declaration::Static(s) => {
                if matches!(s.visibility, Visibility::Public) {
                    exports.push(ExportedSymbol::Static(s.name.clone()));
                }
            }
            Declaration::Model(m) => {
                if matches!(m.visibility, Visibility::Public) {
                    exports.push(ExportedSymbol::Type(m.name.clone()));
                }
            }
            Declaration::Class(c) => {
                if matches!(c.visibility, Visibility::Public) {
                    exports.push(ExportedSymbol::Type(c.name.clone()));
                }
            }
            Declaration::Enum(e) => {
                if matches!(e.visibility, Visibility::Public) {
                    exports.push(ExportedSymbol::Type(e.name.clone()));
                    // Also export variants
                    for variant in &e.variants {
                        exports.push(ExportedSymbol::Variant {
                            enum_name: e.name.clone(),
                            variant_name: variant.node.name.clone(),
                        });
                    }
                    for alias in &e.variant_aliases {
                        exports.push(ExportedSymbol::Variant {
                            enum_name: e.name.clone(),
                            variant_name: alias.node.name.clone(),
                        });
                    }
                }
            }
            Declaration::TypeAlias(a) => {
                if matches!(a.visibility, Visibility::Public) {
                    exports.push(ExportedSymbol::Type(a.name.clone()));
                }
            }
            Declaration::Alias(a) => {
                if matches!(a.visibility, Visibility::Public) {
                    exports.push(ExportedSymbol::Reexported(a.name.clone()));
                }
            }
            Declaration::Newtype(n) => {
                if matches!(n.visibility, Visibility::Public) {
                    exports.push(ExportedSymbol::Type(n.name.clone()));
                }
            }
            Declaration::Trait(t) => {
                if matches!(t.visibility, Visibility::Public) {
                    exports.push(ExportedSymbol::Trait(t.name.clone()));
                }
            }
            Declaration::Function(f) => {
                if matches!(f.visibility, Visibility::Public) {
                    exports.push(ExportedSymbol::Function(f.name.clone()));
                }
            }
            Declaration::Partial(p) => {
                if matches!(p.visibility, Visibility::Public) {
                    exports.push(ExportedSymbol::Function(p.name.clone()));
                }
            }
            Declaration::Import(import) => {
                // Both `from module import X` and `from rust::crate import X` are treated as re-exports. This lets
                // stdlib files like `response.incn` expose axum types (`from rust::axum import Json`) to importers
                // without needing a newtype wrapper.
                let items = match &import.kind {
                    ImportKind::From { items, .. } => Some(items.as_slice()),
                    ImportKind::RustFrom { items, .. } => Some(items.as_slice()),
                    ImportKind::PubFrom { items, .. } => Some(items.as_slice()),
                    _ => None,
                };
                if let Some(items) = items {
                    for item in items {
                        let exported_name = item.alias.as_ref().unwrap_or(&item.name);
                        exports.push(ExportedSymbol::Reexported(exported_name.clone()));
                    }
                }
            }
            Declaration::Capability(cap) => {
                if matches!(cap.visibility, Visibility::Public) {
                    exports.push(ExportedSymbol::Capability(cap.name.clone()));
                }
            }
            Declaration::Docstring(_) | Declaration::TestModule(_) | Declaration::VocabBlock(_) => {}
        }
    }

    exports
}

/// Collect body docstrings for public model/class/enum/trait/newtype declarations.
///
/// Order follows declaration order in `ast`. Entries with `docstring: None` are still included so callers can tell a
/// public type exists without attached body docs.
pub fn exported_type_like_docs(ast: &Program) -> Vec<ExportedTypeLikeDoc> {
    let mut out = Vec::new();
    for decl in &ast.declarations {
        match &decl.node {
            Declaration::Model(m) => {
                if matches!(m.visibility, Visibility::Public) {
                    out.push(ExportedTypeLikeDoc {
                        name: m.name.clone(),
                        kind: ExportedTypeLikeKind::Model,
                        docstring: m.docstring.clone(),
                    });
                }
            }
            Declaration::Class(c) => {
                if matches!(c.visibility, Visibility::Public) {
                    out.push(ExportedTypeLikeDoc {
                        name: c.name.clone(),
                        kind: ExportedTypeLikeKind::Class,
                        docstring: c.docstring.clone(),
                    });
                }
            }
            Declaration::Enum(e) => {
                if matches!(e.visibility, Visibility::Public) {
                    out.push(ExportedTypeLikeDoc {
                        name: e.name.clone(),
                        kind: ExportedTypeLikeKind::Enum,
                        docstring: e.docstring.clone(),
                    });
                }
            }
            Declaration::Trait(t) => {
                if matches!(t.visibility, Visibility::Public) {
                    out.push(ExportedTypeLikeDoc {
                        name: t.name.clone(),
                        kind: ExportedTypeLikeKind::Trait,
                        docstring: t.docstring.clone(),
                    });
                }
            }
            Declaration::Newtype(n) => {
                if matches!(n.visibility, Visibility::Public) {
                    out.push(ExportedTypeLikeDoc {
                        name: n.name.clone(),
                        kind: ExportedTypeLikeKind::Newtype,
                        docstring: n.docstring.clone(),
                    });
                }
            }
            _ => {}
        }
    }
    out
}
