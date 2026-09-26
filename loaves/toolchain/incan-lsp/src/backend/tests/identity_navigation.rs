//! Definition, references and hover through checked source identities: same-spelled locals keep distinct targets,
//! unreferenced member declarations still navigate, imported aliases and re-exports resolve to the provider identity,
//! and package definitions use real source or fail closed.

use std::collections::HashMap;

use incan_semantics_core::{CanonicalSymbolId, HirSourceSpan, SemanticSourceTargetKind, SymbolNamespace, SymbolOrigin};
use tower_lsp::lsp_types::Url;

use super::{
    CheckedIdentityDocument, CheckedIdentitySnapshot, RelatedDeclarationSource, RelatedDeclarationSources,
    checked_identity_at_offset, checked_identity_hover, checked_reference_locations, definition_location_for_identity,
    extend_lsp_package_declaration_sources,
};
use incan_frontend::api_metadata::{
    CHECKED_API_METADATA_SCHEMA_VERSION, CheckedApiMetadata, CheckedApiMetadataPackage, CheckedApiPackageIdentity,
};
use incan_frontend::ast::{Program, Span};
use incan_frontend::library_manifest::LibraryManifest;
use incan_frontend::library_manifest_index::{
    LibraryArtifactMetadata, LibraryManifestIndex, LibraryManifestIndexEntry,
};
use incan_frontend::typechecker::{TypeCheckInfo, TypeChecker};
use incan_frontend::{lexer, parser};

fn parse_module(source: &str, name: &str) -> Result<Program, String> {
    let tokens = lexer::lex(source).map_err(|errors| format!("{name} lex failed: {errors:?}"))?;
    let path = format!("/workspace/src/{name}.incn");
    parser::parse_with_module_path(&tokens, Some(&path)).map_err(|errors| format!("{name} parse failed: {errors:?}"))
}

fn check_module(
    source: &str,
    module_name: &str,
    dependencies: &[(&str, &Program)],
) -> Result<(Program, TypeCheckInfo), String> {
    let ast = parse_module(source, module_name)?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec![module_name.to_string()]));
    for (dependency_name, _) in dependencies {
        checker.register_dependency_module_path_segments(dependency_name, vec![(*dependency_name).to_string()]);
    }
    checker
        .check_with_imports(&ast, dependencies)
        .map_err(|errors| format!("{module_name} typecheck failed: {errors:?}"))?;
    Ok((ast, checker.type_info().clone()))
}

fn nth_span(source: &str, needle: &str, occurrence: usize) -> Result<Span, String> {
    source
        .match_indices(needle)
        .nth(occurrence)
        .map(|(start, matched)| Span::new(start, start + matched.len()))
        .ok_or_else(|| format!("occurrence {occurrence} of `{needle}` not found"))
}

fn source_map(entries: Vec<(SymbolOrigin, Url, &str)>) -> RelatedDeclarationSources {
    entries
        .into_iter()
        .map(|(origin, uri, source)| {
            (
                origin,
                RelatedDeclarationSource {
                    uri,
                    source: source.to_string(),
                },
            )
        })
        .collect()
}

#[test]
fn same_spelled_locals_keep_distinct_lsp_targets_and_references() -> Result<(), String> {
    let source = "def run() -> None:\n  if true:\n    value = 1\n    first = value\n  if true:\n    value = 2\n    second = value\n";
    let (ast, type_info) = check_module(source, "main", &[])?;
    let uri = Url::parse("file:///workspace/src/main.incn").map_err(|error| error.to_string())?;
    let declarations = source_map(vec![(
        SymbolOrigin::Module(vec!["main".to_string()]),
        uri.clone(),
        source,
    )]);
    let snapshot = CheckedIdentitySnapshot {
        type_info,
        declaration_sources: declarations.clone(),
    };
    let first_reference = nth_span(source, "value", 1)?;
    let second_reference = nth_span(source, "value", 3)?;
    let first = checked_identity_at_offset(&uri, &ast, source, &snapshot, first_reference.start)
        .ok_or_else(|| "first local reference has no checked identity".to_string())?;
    let second = checked_identity_at_offset(&uri, &ast, source, &snapshot, second_reference.start)
        .ok_or_else(|| "second local reference has no checked identity".to_string())?;
    assert_ne!(first.identity, second.identity);
    let first_hover = checked_identity_hover(source, &first);
    let second_hover = checked_identity_hover(source, &second);
    assert_ne!(first_hover.contents, second_hover.contents);

    let first_definition = definition_location_for_identity(&first.identity, &declarations)
        .ok_or_else(|| "first local definition has no source location".to_string())?;
    let second_definition = definition_location_for_identity(&second.identity, &declarations)
        .ok_or_else(|| "second local definition has no source location".to_string())?;
    assert_eq!(first_definition.range.start.line, 2);
    assert_eq!(second_definition.range.start.line, 5);

    let references = checked_reference_locations(
        &first.identity,
        false,
        [CheckedIdentityDocument {
            uri: &uri,
            source,
            ast: &ast,
            snapshot: &snapshot,
        }],
        &declarations,
    );
    assert!(references.iter().any(|location| location.range.start.line == 3));
    assert!(!references.iter().any(|location| location.range.start.line == 6));
    Ok(())
}

#[test]
fn unreferenced_member_declarations_have_hover_definition_and_references() -> Result<(), String> {
    let source = "model Entry:\n  value: int\n\n  property label -> str:\n    return \"entry\"\n\n  def render(self) -> int:\n    return 1\n\nenum Status:\n  READY\n";
    let (ast, type_info) = check_module(source, "main", &[])?;
    let uri = Url::parse("file:///workspace/src/main.incn").map_err(|error| error.to_string())?;
    let declarations = source_map(vec![(
        SymbolOrigin::Module(vec!["main".to_string()]),
        uri.clone(),
        source,
    )]);
    let snapshot = CheckedIdentitySnapshot {
        type_info,
        declaration_sources: declarations.clone(),
    };

    for (name, expected_kind, line) in [
        ("value", SemanticSourceTargetKind::Field, 1),
        ("label", SemanticSourceTargetKind::Property, 3),
        ("render", SemanticSourceTargetKind::Method, 6),
        ("READY", SemanticSourceTargetKind::Variant, 10),
    ] {
        let declaration_token = nth_span(source, name, 0)?;
        let occurrence = checked_identity_at_offset(&uri, &ast, source, &snapshot, declaration_token.start)
            .ok_or_else(|| format!("unreferenced `{name}` declaration has no checked identity"))?;
        assert_eq!(occurrence.identity.kind, expected_kind);
        assert_eq!(occurrence.span, declaration_token);

        let hover = checked_identity_hover(source, &occurrence);
        let hover_text = match hover.contents {
            tower_lsp::lsp_types::HoverContents::Markup(markup) => markup.value,
            _ => return Err("member identity hover must use markdown".to_string()),
        };
        assert!(hover_text.contains(&format!("resolved {}", expected_kind.as_str())));

        let definition = definition_location_for_identity(&occurrence.identity, &declarations)
            .ok_or_else(|| format!("unreferenced `{name}` declaration has no definition"))?;
        assert_eq!(definition.uri, uri);
        assert_eq!(definition.range.start.line, line);

        let references = checked_reference_locations(
            &occurrence.identity,
            true,
            [CheckedIdentityDocument {
                uri: &uri,
                source,
                ast: &ast,
                snapshot: &snapshot,
            }],
            &declarations,
        );
        assert_eq!(
            references.len(),
            1,
            "declaration-only `{name}` must be its sole reference"
        );
        assert_eq!(references[0].range.start.line, line);
    }
    Ok(())
}

#[test]
fn imported_alias_and_reexport_navigate_to_provider_identity() -> Result<(), String> {
    let provider_source = "pub def compute() -> int:\n  return 1\n";
    let facade_source = "pub from provider import compute as exposed\n";
    let consumer_source = "from provider import compute as renamed\nfrom facade import exposed as execute\n\ndef use_all() -> int:\n  first = renamed()\n  return execute()\n";
    let provider = parse_module(provider_source, "provider")?;
    let facade = parse_module(facade_source, "facade")?;
    let (_, provider_type_info) = check_module(provider_source, "provider", &[])?;
    let (facade_checked, facade_type_info) = check_module(facade_source, "facade", &[("provider", &provider)])?;
    let (consumer, type_info) = check_module(
        consumer_source,
        "consumer",
        &[("provider", &provider), ("facade", &facade)],
    )?;
    let consumer_uri = Url::parse("file:///workspace/src/consumer.incn").map_err(|error| error.to_string())?;
    let provider_uri = Url::parse("file:///workspace/src/provider.incn").map_err(|error| error.to_string())?;
    let facade_uri = Url::parse("file:///workspace/src/facade.incn").map_err(|error| error.to_string())?;
    let declarations = source_map(vec![
        (
            SymbolOrigin::Module(vec!["consumer".to_string()]),
            consumer_uri.clone(),
            consumer_source,
        ),
        (
            SymbolOrigin::Module(vec!["provider".to_string()]),
            provider_uri.clone(),
            provider_source,
        ),
        (
            SymbolOrigin::Module(vec!["facade".to_string()]),
            facade_uri.clone(),
            facade_source,
        ),
    ]);
    let consumer_snapshot = CheckedIdentitySnapshot {
        type_info,
        declaration_sources: declarations.clone(),
    };
    let provider_snapshot = CheckedIdentitySnapshot {
        type_info: provider_type_info,
        declaration_sources: declarations.clone(),
    };
    let facade_snapshot = CheckedIdentitySnapshot {
        type_info: facade_type_info,
        declaration_sources: declarations.clone(),
    };
    let renamed_span = nth_span(consumer_source, "renamed", 1)?;
    let execute_span = nth_span(consumer_source, "execute", 1)?;
    let renamed = checked_identity_at_offset(
        &consumer_uri,
        &consumer,
        consumer_source,
        &consumer_snapshot,
        renamed_span.start,
    )
    .ok_or_else(|| "aliased import reference has no identity".to_string())?;
    let execute = checked_identity_at_offset(
        &consumer_uri,
        &consumer,
        consumer_source,
        &consumer_snapshot,
        execute_span.start,
    )
    .ok_or_else(|| "re-exported import reference has no identity".to_string())?;
    assert_eq!(renamed.identity, execute.identity);

    let definition = definition_location_for_identity(&execute.identity, &declarations)
        .ok_or_else(|| "provider definition has no mapped source".to_string())?;
    assert_eq!(definition.uri, provider_uri);
    assert_eq!(definition.range.start.line, 0);

    let hover = checked_identity_hover(consumer_source, &execute);
    let hover_text = match hover.contents {
        tower_lsp::lsp_types::HoverContents::Markup(markup) => markup.value,
        _ => return Err("identity hover must use markdown".to_string()),
    };
    assert!(hover_text.contains("execute"));
    assert!(hover_text.contains("provider::compute"));

    let references = checked_reference_locations(
        &execute.identity,
        false,
        [
            CheckedIdentityDocument {
                uri: &provider_uri,
                source: provider_source,
                ast: &provider,
                snapshot: &provider_snapshot,
            },
            CheckedIdentityDocument {
                uri: &facade_uri,
                source: facade_source,
                ast: &facade_checked,
                snapshot: &facade_snapshot,
            },
            CheckedIdentityDocument {
                uri: &consumer_uri,
                source: consumer_source,
                ast: &consumer,
                snapshot: &consumer_snapshot,
            },
        ],
        &declarations,
    );
    for line in [0, 1, 4, 5] {
        assert!(
            references.iter().any(|location| location.range.start.line == line),
            "missing alias/re-export reference on line {line}: {references:?}"
        );
    }
    assert!(references.iter().any(|location| location.uri == facade_uri));
    assert!(!references.iter().any(|location| location.uri == provider_uri));
    Ok(())
}

#[test]
fn package_definition_uses_real_source_and_absence_never_fabricates_a_target() -> Result<(), Box<dyn std::error::Error>>
{
    let provider = tempfile::tempdir()?;
    let source_root = provider.path().join("src");
    std::fs::create_dir_all(&source_root)?;
    let package_source = "pub def calculate() -> int:\n  return 1\n";
    let package_path = source_root.join("math.incn");
    std::fs::write(&package_path, package_source)?;
    let crate_root = provider.path().join("target/lib");
    std::fs::create_dir_all(&crate_root)?;

    let mut manifest = LibraryManifest::new("arithmetic", "0.1.0");
    manifest.contract_metadata.api = Some(CheckedApiMetadataPackage {
        schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
        package: Some(CheckedApiPackageIdentity {
            name: "arithmetic".to_string(),
            version: Some("0.1.0".to_string()),
        }),
        modules: vec![CheckedApiMetadata {
            schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
            derivable_traits: Vec::new(),
            module_path: vec!["math".to_string()],
            declarations: Vec::new(),
        }],
        public_namespaces: Vec::new(),
    });
    let index = LibraryManifestIndex::from_entries(HashMap::from([(
        "math".to_string(),
        LibraryManifestIndexEntry::Loaded {
            manifest: Box::new(manifest),
            metadata: LibraryArtifactMetadata::from_crate_root("math", "arithmetic", &crate_root),
        },
    )]));
    let mut sources = RelatedDeclarationSources::new();
    extend_lsp_package_declaration_sources(&mut sources, &index);
    let identity = CanonicalSymbolId {
        namespace: SymbolNamespace::OrdinaryLexical,
        origin: SymbolOrigin::Package {
            library: "arithmetic".to_string(),
            module_path: vec!["math".to_string()],
        },
        declaration_name: "calculate".to_string(),
        kind: SemanticSourceTargetKind::Function,
        scope_discriminant: None,
        declaration_span: HirSourceSpan::new(0, package_source.len()),
    };
    let definition = definition_location_for_identity(&identity, &sources)
        .ok_or("locally available package source was not mapped")?;
    let canonical_package_path = package_path.canonicalize()?;
    assert_eq!(
        definition.uri,
        Url::from_file_path(canonical_package_path).map_err(|_| "invalid package source path")?
    );

    sources.clear();
    assert!(definition_location_for_identity(&identity, &sources).is_none());

    let unresolved_source = "def missing() -> None:\n  pass\n\ndef run() -> None:\n  missing()\n";
    let unresolved_ast = parse_module(unresolved_source, "main")?;
    let unresolved_uri = Url::parse("file:///workspace/src/main.incn")?;
    let unresolved_offset = nth_span(unresolved_source, "missing", 1)?.start;
    assert!(
        checked_identity_at_offset(
            &unresolved_uri,
            &unresolved_ast,
            unresolved_source,
            &CheckedIdentitySnapshot::default(),
            unresolved_offset,
        )
        .is_none()
    );
    Ok(())
}
