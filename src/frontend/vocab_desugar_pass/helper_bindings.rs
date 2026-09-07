use std::collections::BTreeMap;

use crate::frontend::ast;
use crate::frontend::library_manifest_index::{LibraryManifestIndex, LibraryManifestIndexEntry};

/// One hidden import injected to back a symbolic helper reference.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct HelperImportSpec {
    dependency_key: String,
    exported_name: String,
    alias: String,
}

/// Deduplicates helper imports requested by desugared output before we splice them into the host program.
#[derive(Debug, Default)]
pub(super) struct HelperImportAccumulator {
    imports: BTreeMap<(String, String), HelperImportSpec>,
}

impl HelperImportAccumulator {
    /// Register one helper import and return the alias that desugared code should call.
    pub(super) fn register(&mut self, dependency_key: &str, exported_name: &str) -> String {
        let key = (dependency_key.to_string(), exported_name.to_string());
        let alias = helper_import_alias(dependency_key, exported_name);
        let spec = HelperImportSpec {
            dependency_key: dependency_key.to_string(),
            exported_name: exported_name.to_string(),
            alias: alias.clone(),
        };
        self.imports.entry(key).or_insert(spec);
        alias
    }

    /// Materialize deterministic hidden imports for all registered helper aliases.
    fn import_declarations(&self) -> Vec<ast::Spanned<ast::Declaration>> {
        let mut declarations = Vec::new();
        for spec in self.imports.values() {
            declarations.push(ast::Spanned::new(
                ast::Declaration::Import(ast::ImportDecl {
                    visibility: ast::Visibility::Private,
                    kind: ast::ImportKind::PubFrom {
                        library: spec.dependency_key.clone(),
                        path: Vec::new(),
                        items: vec![ast::ImportItem {
                            name: spec.exported_name.clone(),
                            alias: Some(spec.alias.clone()),
                        }],
                    },
                    alias: None,
                }),
                ast::Span::default(),
            ));
        }
        declarations
    }
}

/// Build the hidden import alias used when a desugarer references a provider helper symbol.
fn helper_import_alias(dependency_key: &str, exported_name: &str) -> String {
    let sanitize = |value: &str| {
        value
            .chars()
            .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
            .collect::<String>()
    };
    format!(
        "__incan_vocab_helper_{}_{}",
        sanitize(dependency_key),
        sanitize(exported_name)
    )
}

/// Inject hidden `pub::` imports for every helper symbol referenced by desugared output.
pub(super) fn inject_helper_imports(program: &mut ast::Program, helper_imports: &HelperImportAccumulator) {
    let imports = helper_imports.import_declarations();
    if imports.is_empty() {
        return;
    }

    let mut insert_at = 0usize;
    while let Some(declaration) = program.declarations.get(insert_at) {
        match declaration.node {
            ast::Declaration::Docstring(_) | ast::Declaration::Import(_) => insert_at += 1,
            _ => break,
        }
    }
    program.declarations.splice(insert_at..insert_at, imports);
}

/// Rewrite symbolic helper references inside desugared statements into hidden import aliases.
pub(super) fn resolve_helper_bindings_in_statements(
    statements: &mut [incan_vocab::IncanStatement],
    keyword_metadata: Option<&incan_vocab::VocabKeywordMetadata>,
    keyword: &str,
    library_manifest_index: &LibraryManifestIndex,
    helper_imports: &mut HelperImportAccumulator,
) -> Result<(), String> {
    for statement in statements {
        resolve_helper_bindings_in_statement(
            statement,
            keyword_metadata,
            keyword,
            library_manifest_index,
            helper_imports,
        )?;
    }
    Ok(())
}

/// Resolve helper references recursively inside one desugared public statement.
fn resolve_helper_bindings_in_statement(
    statement: &mut incan_vocab::IncanStatement,
    keyword_metadata: Option<&incan_vocab::VocabKeywordMetadata>,
    keyword: &str,
    library_manifest_index: &LibraryManifestIndex,
    helper_imports: &mut HelperImportAccumulator,
) -> Result<(), String> {
    match statement {
        incan_vocab::IncanStatement::Expr(expr) => {
            resolve_helper_bindings_in_expr(expr, keyword_metadata, keyword, library_manifest_index, helper_imports)
        }
        incan_vocab::IncanStatement::Return(Some(expr))
        | incan_vocab::IncanStatement::Assign { value: expr, .. }
        | incan_vocab::IncanStatement::Let { value: expr, .. } => {
            resolve_helper_bindings_in_expr(expr, keyword_metadata, keyword, library_manifest_index, helper_imports)
        }
        incan_vocab::IncanStatement::If {
            condition,
            then_body,
            else_body,
        } => {
            resolve_helper_bindings_in_expr(
                condition,
                keyword_metadata,
                keyword,
                library_manifest_index,
                helper_imports,
            )?;
            resolve_helper_bindings_in_statements(
                then_body,
                keyword_metadata,
                keyword,
                library_manifest_index,
                helper_imports,
            )?;
            resolve_helper_bindings_in_statements(
                else_body,
                keyword_metadata,
                keyword,
                library_manifest_index,
                helper_imports,
            )
        }
        incan_vocab::IncanStatement::While { condition, body } => {
            resolve_helper_bindings_in_expr(
                condition,
                keyword_metadata,
                keyword,
                library_manifest_index,
                helper_imports,
            )?;
            resolve_helper_bindings_in_statements(
                body,
                keyword_metadata,
                keyword,
                library_manifest_index,
                helper_imports,
            )
        }
        incan_vocab::IncanStatement::For { iter, body, .. } => {
            resolve_helper_bindings_in_expr(iter, keyword_metadata, keyword, library_manifest_index, helper_imports)?;
            resolve_helper_bindings_in_statements(
                body,
                keyword_metadata,
                keyword,
                library_manifest_index,
                helper_imports,
            )
        }
        incan_vocab::IncanStatement::Pass | incan_vocab::IncanStatement::Return(None) => Ok(()),
        _ => Ok(()),
    }
}

/// Resolve helper references recursively inside one desugared public expression.
pub(super) fn resolve_helper_bindings_in_expr(
    expr: &mut incan_vocab::IncanExpr,
    keyword_metadata: Option<&incan_vocab::VocabKeywordMetadata>,
    keyword: &str,
    library_manifest_index: &LibraryManifestIndex,
    helper_imports: &mut HelperImportAccumulator,
) -> Result<(), String> {
    match expr {
        incan_vocab::IncanExpr::Helper(helper_key) => {
            let keyword_metadata = keyword_metadata.ok_or_else(|| {
                format!(
                    "keyword `{keyword}` does not carry provider metadata, so helper `{helper_key}` cannot be resolved"
                )
            })?;
            let exported_name =
                resolve_helper_export_name(library_manifest_index, &keyword_metadata.dependency_key, helper_key)?;
            let alias = helper_imports.register(&keyword_metadata.dependency_key, &exported_name);
            *expr = incan_vocab::IncanExpr::Name(alias);
            Ok(())
        }
        incan_vocab::IncanExpr::List(items) | incan_vocab::IncanExpr::Tuple(items) => {
            for item in items {
                resolve_helper_bindings_in_expr(
                    item,
                    keyword_metadata,
                    keyword,
                    library_manifest_index,
                    helper_imports,
                )?;
            }
            Ok(())
        }
        incan_vocab::IncanExpr::Dict(entries) => {
            for (key_expr, value_expr) in entries {
                resolve_helper_bindings_in_expr(
                    key_expr,
                    keyword_metadata,
                    keyword,
                    library_manifest_index,
                    helper_imports,
                )?;
                resolve_helper_bindings_in_expr(
                    value_expr,
                    keyword_metadata,
                    keyword,
                    library_manifest_index,
                    helper_imports,
                )?;
            }
            Ok(())
        }
        incan_vocab::IncanExpr::Binary(left, _, right) => {
            resolve_helper_bindings_in_expr(left, keyword_metadata, keyword, library_manifest_index, helper_imports)?;
            resolve_helper_bindings_in_expr(right, keyword_metadata, keyword, library_manifest_index, helper_imports)
        }
        incan_vocab::IncanExpr::Unary(_, value) => {
            resolve_helper_bindings_in_expr(value, keyword_metadata, keyword, library_manifest_index, helper_imports)
        }
        incan_vocab::IncanExpr::Call { callee, args } => {
            resolve_helper_bindings_in_expr(
                callee,
                keyword_metadata,
                keyword,
                library_manifest_index,
                helper_imports,
            )?;
            for arg in args {
                resolve_helper_bindings_in_expr(
                    arg,
                    keyword_metadata,
                    keyword,
                    library_manifest_index,
                    helper_imports,
                )?;
            }
            Ok(())
        }
        incan_vocab::IncanExpr::Field { object, .. } => resolve_helper_bindings_in_expr(
            object,
            keyword_metadata,
            keyword,
            library_manifest_index,
            helper_imports,
        ),
        _ => Ok(()),
    }
}

/// Resolve one helper key to the public export name a desugared reference should import.
///
/// An explicit `HelperBinding` is the override, for a key whose export spelling deliberately differs from the key
/// the desugarer emits. With no binding, the key resolves directly against the provider's own checked public
/// surface, which is the same surface an ordinary consumer imports through.
///
/// That default is the point of #1032. A companion that re-states its package's export names is maintaining a
/// second copy of that surface, kept honest only by a test; the copy is what drifts. Resolving the key against the
/// package removes the copy rather than validating it.
fn resolve_helper_export_name(
    library_manifest_index: &LibraryManifestIndex,
    dependency_key: &str,
    helper_key: &str,
) -> Result<String, String> {
    let Some(entry) = library_manifest_index.get(dependency_key) else {
        return Err(format!("provider `pub::{dependency_key}` is not loaded"));
    };
    let LibraryManifestIndexEntry::Loaded { manifest, .. } = entry else {
        return Err(format!("provider `pub::{dependency_key}` failed to load"));
    };
    let Some(vocab) = manifest.vocab.as_ref() else {
        return Err(format!(
            "provider `pub::{dependency_key}` does not expose vocab metadata"
        ));
    };

    let mut matching = vocab
        .provider_manifest
        .helper_bindings
        .iter()
        .filter(|binding| binding.key == helper_key);
    let bound = matching.next();
    if let (Some(binding), Some(duplicate)) = (bound, matching.next()) {
        return Err(format!(
            "provider `pub::{dependency_key}` binds helper `{helper_key}` more than once, to `{}` and `{}`; \
             remove the duplicate so the helper has one spelling",
            binding.exported_name, duplicate.exported_name
        ));
    }

    // With no explicit binding the key names the export directly, so the diagnostics below have to say which of the
    // two the author actually wrote; "binds helper X to missing export X" would read as a broken binding that does
    // not exist.
    let exported_name = match bound {
        Some(binding) => binding.exported_name.as_str(),
        None => helper_key,
    };

    let Some(kind) = manifest.exports.view().helper_export_kind(exported_name) else {
        return Err(match bound {
            Some(_) => format!(
                "provider `pub::{dependency_key}` binds helper `{helper_key}` to missing export `{exported_name}`"
            ),
            None => format!(
                "provider `pub::{dependency_key}` does not export `{helper_key}`; export it, or bind the helper to \
                 the name it should resolve to"
            ),
        });
    };
    if !kind.is_callable() {
        return Err(format!(
            "provider `pub::{dependency_key}` resolves helper `{helper_key}` to {} `{exported_name}`, which cannot \
             be called; a helper must resolve to a function, class, model, newtype, enum variant, partial, or alias",
            kind.label()
        ));
    }
    Ok(exported_name.to_string())
}

#[cfg(test)]
mod tests {
    use incan_core::lang::surface::constructors;
    use std::collections::HashMap;
    use std::path::PathBuf;

    use super::*;

    /// Build a minimal exported function record for helper-resolution fixtures.
    fn function_export(name: &str) -> crate::library_manifest::FunctionExport {
        crate::library_manifest::FunctionExport {
            name: name.to_string(),
            emitted_name: None,
            type_params: Vec::new(),
            params: Vec::new(),
            return_type: crate::library_manifest::TypeRef::Named {
                name: constructors::as_str(constructors::ConstructorId::None).to_string(),
            },
            is_async: false,
        }
    }

    /// Build a minimal exported const record for helper-resolution fixtures.
    fn const_export(name: &str) -> crate::library_manifest::ConstExport {
        crate::library_manifest::ConstExport {
            name: name.to_string(),
            ty: crate::library_manifest::TypeRef::Named {
                name: "int".to_string(),
            },
        }
    }

    /// Build a one-provider index whose manifest carries the given helper bindings and exports.
    fn index_with(
        bindings: Vec<incan_vocab::HelperBinding>,
        customize: impl FnOnce(&mut crate::library_manifest::LibraryManifest),
    ) -> LibraryManifestIndex {
        let mut manifest = crate::library_manifest::LibraryManifest::new("demo", "0.1.0");
        customize(&mut manifest);
        manifest.vocab = Some(crate::library_manifest::VocabExports {
            crate_path: "vocab_companion".to_string(),
            package_name: "vocab_companion".to_string(),
            keyword_registrations: Vec::new(),
            dsl_surfaces: Vec::new(),
            provider_manifest: incan_vocab::LibraryManifest {
                helper_bindings: bindings,
                ..incan_vocab::LibraryManifest::default()
            },
            desugarer_artifact: None,
        });
        LibraryManifestIndex::from_entries(HashMap::from([(
            "demo".to_string(),
            LibraryManifestIndexEntry::Loaded {
                manifest: Box::new(manifest),
                metadata: crate::frontend::library_manifest_index::LibraryArtifactMetadata::from_crate_root(
                    "demo",
                    "demo",
                    PathBuf::from("/tmp/demo"),
                ),
            },
        )]))
    }

    /// Build a minimal exported public partial for helper-resolution fixtures.
    fn partial_export(name: &str) -> crate::library_manifest::PartialExport {
        crate::library_manifest::PartialExport {
            name: name.to_string(),
            target_path: vec!["demo".to_string(), "filter_rows".to_string()],
            target_kind: crate::library_manifest::PartialTargetKindExport::Function,
            presets: Vec::new(),
            type_params: Vec::new(),
            params: Vec::new(),
            return_type: crate::library_manifest::TypeRef::Named {
                name: constructors::as_str(constructors::ConstructorId::None).to_string(),
            },
            is_async: false,
        }
    }

    #[test]
    fn helper_resolution_derives_an_unbound_key_from_the_package_surface() -> Result<(), Box<dyn std::error::Error>> {
        // With no explicit binding the helper key names the export directly. A companion that re-stated its own
        // package's export names was keeping a second copy of the public surface, and the copy is what drifts
        // (#1032).
        let index = index_with(Vec::new(), |manifest| {
            manifest.exports.functions.push(function_export("col"));
        });

        let exported_name = resolve_helper_export_name(&index, "demo", "col")?;
        assert_eq!(exported_name, "col");
        Ok(())
    }

    #[test]
    fn an_explicit_binding_still_overrides_the_package_surface() -> Result<(), Box<dyn std::error::Error>> {
        // Deriving from the surface is the default, not the only path: a key whose export spelling deliberately
        // differs still resolves through its declared binding.
        let index = index_with(
            vec![incan_vocab::HelperBinding {
                key: "filter".to_string(),
                exported_name: "filter_rows".to_string(),
            }],
            |manifest| {
                manifest.exports.functions.push(function_export("filter_rows"));
                manifest.exports.functions.push(function_export("filter"));
            },
        );

        let exported_name = resolve_helper_export_name(&index, "demo", "filter")?;
        assert_eq!(
            exported_name, "filter_rows",
            "the declared binding should win over the same-spelled export"
        );
        Ok(())
    }

    #[test]
    fn a_derived_key_that_names_nothing_reports_the_key_the_author_wrote() -> Result<(), Box<dyn std::error::Error>> {
        // The bound and derived paths fail differently, and the message has to name what the author actually
        // wrote. "binds helper `col` to missing export `col`" would describe a binding that does not exist.
        let index = index_with(Vec::new(), |manifest| {
            manifest.exports.functions.push(function_export("lit"));
        });

        let err = match resolve_helper_export_name(&index, "demo", "col") {
            Err(err) => err,
            Ok(exported_name) => panic!("expected an unknown-key rejection, resolved to `{exported_name}`"),
        };
        assert!(err.contains("does not export `col`"), "unexpected error: {err}");
        Ok(())
    }

    #[test]
    fn a_derived_key_that_names_an_uncallable_export_is_rejected() -> Result<(), Box<dyn std::error::Error>> {
        // A helper reference is spliced into call position, so deriving from the surface must not admit a name the
        // surface exports but nothing can call.
        let index = index_with(Vec::new(), |manifest| {
            manifest.exports.consts.push(const_export("col"));
        });

        let err = match resolve_helper_export_name(&index, "demo", "col") {
            Err(err) => err,
            Ok(exported_name) => panic!("expected an uncallable rejection, resolved to `{exported_name}`"),
        };
        assert!(err.contains("const `col`"), "unexpected error: {err}");
        assert!(err.contains("cannot"), "unexpected error: {err}");
        Ok(())
    }

    #[test]
    fn helper_resolution_accepts_a_public_partial() -> Result<(), Box<dyn std::error::Error>> {
        // A public partial is a preset over a callable target, so it is callable itself and belongs on the surface a
        // companion may bind to. Leaving it out rejected a legitimate public export as missing.
        let index = index_with(
            vec![incan_vocab::HelperBinding {
                key: "filter".to_string(),
                exported_name: "filter_active".to_string(),
            }],
            |manifest| {
                manifest.exports.functions.push(function_export("filter_rows"));
                manifest.exports.partials.push(partial_export("filter_active"));
            },
        );

        let exported_name = resolve_helper_export_name(&index, "demo", "filter")?;
        assert_eq!(exported_name, "filter_active");
        Ok(())
    }

    #[test]
    fn helper_resolution_follows_a_reexport_alias_to_its_target() -> Result<(), Box<dyn std::error::Error>> {
        // A library may publish a helper under an alias rather than its declaration name. Binding the alias must
        // resolve exactly as binding the original would, instead of failing as an unknown export.
        let index = index_with(
            vec![incan_vocab::HelperBinding {
                key: "filter".to_string(),
                exported_name: "where_".to_string(),
            }],
            |manifest| {
                manifest.exports.functions.push(function_export("filter_rows"));
                manifest.exports.aliases.push(crate::library_manifest::AliasExport {
                    name: "where_".to_string(),
                    target_path: vec!["demo".to_string(), "filter_rows".to_string()],
                    projected_function: None,
                });
            },
        );

        let exported_name = resolve_helper_export_name(&index, "demo", "filter")?;
        assert_eq!(exported_name, "where_");
        Ok(())
    }

    #[test]
    fn helper_resolution_rejects_an_alias_that_lands_on_an_uncallable_target() -> Result<(), Box<dyn std::error::Error>>
    {
        // Following the alias must not launder an ineligible target into an eligible one.
        let index = index_with(
            vec![incan_vocab::HelperBinding {
                key: "filter".to_string(),
                exported_name: "Filterish".to_string(),
            }],
            |manifest| {
                manifest.exports.consts.push(const_export("FILTER_LIMIT"));
                manifest.exports.aliases.push(crate::library_manifest::AliasExport {
                    name: "Filterish".to_string(),
                    target_path: vec!["demo".to_string(), "FILTER_LIMIT".to_string()],
                    projected_function: None,
                });
            },
        );

        let err = match resolve_helper_export_name(&index, "demo", "filter") {
            Err(err) => err,
            Ok(_) => panic!("expected the alias target to be rejected"),
        };
        assert!(
            err.contains("const `Filterish`"),
            "error should name the resolved kind: {err}"
        );
        Ok(())
    }

    #[test]
    fn helper_resolution_survives_a_cyclic_alias_chain() -> Result<(), Box<dyn std::error::Error>> {
        // A manifest that aliases in a loop must terminate rather than recurse forever.
        let index = index_with(
            vec![incan_vocab::HelperBinding {
                key: "filter".to_string(),
                exported_name: "a".to_string(),
            }],
            |manifest| {
                for (name, target) in [("a", "b"), ("b", "a")] {
                    manifest.exports.aliases.push(crate::library_manifest::AliasExport {
                        name: name.to_string(),
                        target_path: vec!["demo".to_string(), target.to_string()],
                        projected_function: None,
                    });
                }
            },
        );

        // Terminating at all is the assertion; an unresolvable chain stays callable rather than being rejected on
        // incomplete information.
        let exported_name = resolve_helper_export_name(&index, "demo", "filter")?;
        assert_eq!(exported_name, "a");
        Ok(())
    }

    #[test]
    fn helper_resolution_rejects_a_duplicated_helper_key() -> Result<(), Box<dyn std::error::Error>> {
        // Two bindings for one key used to resolve silently to whichever was declared first, so a provider could
        // ship an ambiguous surface and the choice would depend on declaration order.
        let index = index_with(
            vec![
                incan_vocab::HelperBinding {
                    key: "filter".to_string(),
                    exported_name: "filter_rows".to_string(),
                },
                incan_vocab::HelperBinding {
                    key: "filter".to_string(),
                    exported_name: "filter_cols".to_string(),
                },
            ],
            |_| {},
        );

        let err = match resolve_helper_export_name(&index, "demo", "filter") {
            Err(err) => err,
            Ok(exported_name) => panic!("expected duplicate rejection, resolved to `{exported_name}`"),
        };
        assert!(err.contains("more than once"), "unexpected error: {err}");
        assert!(
            err.contains("filter_rows") && err.contains("filter_cols"),
            "error should name both: {err}"
        );
        Ok(())
    }

    #[test]
    fn helper_resolution_rejects_a_binding_to_an_uncallable_export() -> Result<(), Box<dyn std::error::Error>> {
        // A trait is exported and therefore passed the old name-only check, but a desugared helper reference is
        // spliced into call position, where a trait cannot go.
        let index = index_with(
            vec![incan_vocab::HelperBinding {
                key: "filter".to_string(),
                exported_name: "Filterable".to_string(),
            }],
            |manifest| {
                manifest.exports.traits.push(crate::library_manifest::TraitExport {
                    name: "Filterable".to_string(),
                    source_name: None,
                    type_params: Vec::new(),
                    supertraits: Vec::new(),
                    requires: Vec::new(),
                    methods: Vec::new(),
                });
            },
        );

        let err = match resolve_helper_export_name(&index, "demo", "filter") {
            Err(err) => err,
            Ok(_) => panic!("expected an ineligible-kind rejection"),
        };
        assert!(err.contains("trait `Filterable`"), "error should name the kind: {err}");
        assert!(err.contains("cannot be called"), "unexpected error: {err}");
        Ok(())
    }

    #[test]
    fn helper_resolution_rejects_bindings_to_missing_exports() -> Result<(), Box<dyn std::error::Error>> {
        let mut manifest = crate::library_manifest::LibraryManifest::new("demo", "0.1.0");
        manifest.vocab = Some(crate::library_manifest::VocabExports {
            crate_path: "vocab_companion".to_string(),
            package_name: "vocab_companion".to_string(),
            keyword_registrations: Vec::new(),
            dsl_surfaces: Vec::new(),
            provider_manifest: incan_vocab::LibraryManifest {
                helper_bindings: vec![incan_vocab::HelperBinding {
                    key: "filter".to_string(),
                    exported_name: "filter".to_string(),
                }],
                ..incan_vocab::LibraryManifest::default()
            },
            desugarer_artifact: None,
        });
        let index = LibraryManifestIndex::from_entries(HashMap::from([(
            "demo".to_string(),
            LibraryManifestIndexEntry::Loaded {
                manifest: Box::new(manifest),
                metadata: crate::frontend::library_manifest_index::LibraryArtifactMetadata::from_crate_root(
                    "demo",
                    "demo",
                    PathBuf::from("/tmp/demo"),
                ),
            },
        )]));

        let err = match resolve_helper_export_name(&index, "demo", "filter") {
            Err(err) => err,
            Ok(_) => panic!("expected missing export rejection"),
        };
        assert!(err.contains("missing export `filter`"), "unexpected error: {err}");
        Ok(())
    }
}
