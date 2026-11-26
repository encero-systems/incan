//! Source import path resolution, logical candidates and source module import classification.

use super::*;

/// A caller-supplied project source root resolves imports outside a test directory.
#[test]
fn resolve_import_path_falls_back_to_supplied_source_root_for_tests_dir() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let root = tmp.path();
    std::fs::write(
        root.join("loaf.toml"),
        r#"[project]
name = "demo"
version = "0.1.0"
"#,
    )?;
    let tests_dir = root.join("tests");
    let src_dir = root.join("src");
    std::fs::create_dir_all(&tests_dir)?;
    std::fs::create_dir_all(&src_dir)?;
    let dataset = src_dir.join("dataset.incn");
    std::fs::write(&dataset, "pub trait DataSet[T]:\n    pass\n")?;

    let SourceModuleImportResolution::Local(resolved) =
        resolve_source_module_import(&tests_dir, Some(&src_dir), &relative_from_import("dataset"))
    else {
        return Err("source-root import was not resolved locally".into());
    };
    assert_eq!(resolved.file_path, dataset.canonicalize()?);
    Ok(())
}

/// A test-local module wins over a module under the caller-supplied source root.
#[test]
fn resolve_import_path_prefers_base_dir_before_source_root_fallback() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let root = tmp.path();
    std::fs::write(
        root.join("loaf.toml"),
        r#"[project]
name = "demo"
version = "0.1.0"
"#,
    )?;
    let tests_dir = root.join("tests");
    let src_dir = root.join("src");
    std::fs::create_dir_all(&tests_dir)?;
    std::fs::create_dir_all(&src_dir)?;
    let tests_dataset = tests_dir.join("dataset.incn");
    let src_dataset = src_dir.join("dataset.incn");
    std::fs::write(&tests_dataset, "pub trait LocalDataSet[T]:\n    pass\n")?;
    std::fs::write(&src_dataset, "pub trait SourceDataSet[T]:\n    pass\n")?;

    let SourceModuleImportResolution::Local(resolved) =
        resolve_source_module_import(&tests_dir, Some(&src_dir), &relative_from_import("dataset"))
    else {
        return Err("test-local import was not resolved locally".into());
    };
    assert_eq!(resolved.file_path, tests_dataset.canonicalize()?);
    Ok(())
}

/// Manifest orchestration supplies a nonstandard source root explicitly at the resolution boundary.
#[test]
fn resolve_import_path_uses_supplied_source_root_when_configured() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let root = tmp.path();
    std::fs::write(
        root.join("loaf.toml"),
        r#"[project]
name = "demo"
version = "0.1.0"

[build]
source-root = "library"
"#,
    )?;
    let tests_dir = root.join("tests");
    let library_dir = root.join("library");
    std::fs::create_dir_all(&tests_dir)?;
    std::fs::create_dir_all(&library_dir)?;
    let dataset = library_dir.join("dataset.incn");
    std::fs::write(&dataset, "pub trait DataSet[T]:\n    pass\n")?;

    let SourceModuleImportResolution::Local(resolved) =
        resolve_source_module_import(&tests_dir, Some(&library_dir), &relative_from_import("dataset"))
    else {
        return Err("configured source-root import was not resolved locally".into());
    };
    assert_eq!(resolved.file_path, dataset.canonicalize()?);
    let root_dataset = root.join("dataset.incn");
    std::fs::write(&root_dataset, "pub trait RootDataSet[T]:\n    pass\n")?;
    let mut root_import = relative_from_import("dataset");
    if let ImportKind::From { module, .. } = &mut root_import.kind {
        module.is_absolute = true;
    }
    assert_eq!(
        resolve_import_path(&tests_dir, &root_import),
        Some(root_dataset.canonicalize()?)
    );
    Ok(())
}

#[test]
fn logical_module_segments_strip_directory_entrypoint_suffixes() -> Result<(), Box<dyn std::error::Error>> {
    let base = PathBuf::from("src");

    let dataset = logical_module_segments_from_file(&base, &base.join("dataset").join("mod.incn"))
        .ok_or("dataset/mod.incn should resolve logical path")?;
    assert_eq!(dataset, vec!["dataset".to_string()]);

    let nested = logical_module_segments_from_file(&base, &base.join("dataset").join("ops").join("mod.incn"))
        .ok_or("dataset/ops/mod.incn should resolve logical path")?;
    assert_eq!(nested, vec!["dataset".to_string(), "ops".to_string()]);

    let leaf = logical_module_segments_from_file(&base, &base.join("dataset").join("ops.incn"))
        .ok_or("dataset/ops.incn should resolve logical path")?;
    assert_eq!(leaf, vec!["dataset".to_string(), "ops".to_string()]);

    let prelude = logical_module_segments_from_file(&base, &base.join("dataset").join("prelude.incn"))
        .ok_or("dataset/prelude.incn should resolve logical path")?;
    assert_eq!(prelude, vec!["dataset".to_string()]);

    let root_prelude = logical_module_segments_from_file(&base, &base.join("prelude.incn"))
        .ok_or("root prelude.incn should remain a distinct module")?;
    assert_eq!(root_prelude, vec!["prelude".to_string()]);

    Ok(())
}

#[test]
fn logical_source_import_candidates_follow_sibling_then_root_precedence() {
    let path = ImportPath::simple(vec!["vaults".to_string()]);
    assert_eq!(
        logical_source_import_candidates(&["pkg".to_string(), "consumer".to_string()], &path),
        vec![
            vec!["pkg".to_string(), "vaults".to_string()],
            vec!["vaults".to_string()]
        ]
    );
    assert_eq!(
        logical_source_import_candidates(&[], &path),
        vec![vec!["vaults".to_string()]]
    );
}

#[test]
fn logical_source_import_candidates_preserve_absolute_and_parent_paths() {
    assert_eq!(
        logical_source_import_candidates(
            &["pkg".to_string(), "nested".to_string(), "consumer".to_string()],
            &ImportPath::absolute(vec!["shared".to_string()]),
        ),
        vec![vec!["shared".to_string()]]
    );
    assert_eq!(
        logical_source_import_candidates(
            &["pkg".to_string(), "nested".to_string(), "consumer".to_string()],
            &ImportPath::relative(1, vec!["shared".to_string()]),
        ),
        vec![vec!["pkg".to_string(), "shared".to_string()]]
    );
}

#[test]
fn resolve_source_module_import_classifies_stdlib_imports() {
    let import = relative_module_import(&["std", "testing"]);
    let resolved = resolve_source_module_import(Path::new("src"), None, &import);

    assert_eq!(
        resolved,
        SourceModuleImportResolution::Stdlib {
            module_path: vec!["std".to_string(), "testing".to_string()]
        }
    );
}

/// A `from std.<namespace> import ...` of submodules alone depends on those submodules and not on the namespace's
/// own module, which `std.derives` does not have; an import that also names a member depends on the namespace's
/// module too (#1561).
#[test]
fn stdlib_namespace_import_of_submodules_depends_on_the_submodules_only_issue1561()
-> Result<(), Box<dyn std::error::Error>> {
    let path = |segments: &[&str]| {
        segments
            .iter()
            .map(|segment| (*segment).to_string())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        stdlib_dependencies_of("from std.derives import comparison\n")?,
        vec![path(&["std", "derives", "comparison"])]
    );
    assert_eq!(
        stdlib_dependencies_of("from std.derives import comparison, copying\n")?,
        vec![
            path(&["std", "derives", "comparison"]),
            path(&["std", "derives", "copying"])
        ]
    );
    assert_eq!(
        stdlib_dependencies_of("from std.async import time, spawn\n")?,
        vec![path(&["std", "async"]), path(&["std", "async", "time"])]
    );
    assert_eq!(
        stdlib_dependencies_of("from std.derives.comparison import Ord\n")?,
        vec![path(&["std", "derives", "comparison"])]
    );
    Ok(())
}

/// A namespace's `prelude` path names the namespace's own module in the source graph, as it does for the checker
/// and the provider module-use set: `import std.async.prelude`, `from std.async.prelude import sleep` and
/// `from std.async import prelude` depend on `std.async`, which the compiled SDK claims, and not on a
/// `std.async.prelude` module it does not (#1561). The root `std.prelude` stays a module of its own.
#[test]
fn stdlib_namespace_prelude_imports_depend_on_the_namespace_module_issue1561() -> Result<(), Box<dyn std::error::Error>>
{
    let path = |segments: &[&str]| {
        segments
            .iter()
            .map(|segment| (*segment).to_string())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        stdlib_dependencies_of("import std.async.prelude\nfrom std.async.prelude import sleep\n")?,
        vec![path(&["std", "async"]), path(&["std", "async"])]
    );
    assert_eq!(
        stdlib_dependencies_of("from std.async import prelude, time\n")?,
        vec![path(&["std", "async"]), path(&["std", "async", "time"])]
    );
    assert_eq!(
        stdlib_dependencies_of("import std.prelude\n")?,
        vec![path(&["std", "prelude"])]
    );
    Ok(())
}

#[test]
fn resolve_source_module_import_prefers_full_module_import_path() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let src = tmp.path().join("src");
    std::fs::create_dir_all(src.join("dataset"))?;
    let parent = src.join("dataset.incn");
    let nested = src.join("dataset").join("ops.incn");
    std::fs::write(&parent, "pub const PARENT: int = 1\n")?;
    std::fs::write(&nested, "pub const NESTED: int = 2\n")?;

    let import = relative_module_import(&["dataset", "ops"]);
    let resolved = resolve_source_module_import(&src, Some(&src), &import);

    let SourceModuleImportResolution::Local(module_ref) = resolved else {
        return Err("expected local source module resolution".into());
    };
    assert_eq!(module_ref.file_path, nested.canonicalize()?);
    assert_eq!(module_ref.path_segments, vec!["dataset".to_string(), "ops".to_string()]);
    Ok(())
}

#[test]
fn resolve_source_module_import_falls_back_to_parent_for_item_style_module_import()
-> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let src = tmp.path().join("src");
    std::fs::create_dir_all(src.join("db"))?;
    let schema = src.join("db").join("schema.incn");
    std::fs::write(&schema, "pub model Database:\n    pub id: int\n")?;

    let import = relative_module_import(&["db", "schema", "Database"]);
    let resolved = resolve_source_module_import(&src, Some(&src), &import);

    let SourceModuleImportResolution::Local(module_ref) = resolved else {
        return Err("expected parent source module resolution".into());
    };
    assert_eq!(module_ref.file_path, schema.canonicalize()?);
    assert_eq!(module_ref.path_segments, vec!["db".to_string(), "schema".to_string()]);
    Ok(())
}

#[test]
fn resolve_source_module_import_uses_source_root_for_nested_module_identity() -> Result<(), Box<dyn std::error::Error>>
{
    let tmp = tempfile::tempdir()?;
    let src = tmp.path().join("src");
    let package = src.join("pkg");
    std::fs::create_dir_all(&package)?;
    let sibling = package.join("bar.incn");
    std::fs::write(&sibling, "pub const VALUE: int = 1\n")?;

    let import = relative_module_import(&["bar"]);
    let resolved = resolve_source_module_import(&package, Some(&src), &import);

    let SourceModuleImportResolution::Local(module_ref) = resolved else {
        return Err("expected local source module resolution".into());
    };
    assert_eq!(module_ref.file_path, sibling.canonicalize()?);
    assert_eq!(module_ref.path_segments, vec!["pkg".to_string(), "bar".to_string()]);
    assert_eq!(module_ref.module_name, "pkg_bar");
    Ok(())
}

#[test]
fn resolve_program_source_imports_does_not_admit_same_leaf_self_import() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let src = tmp.path().join("src");
    let nested = src.join("substrait");
    std::fs::create_dir_all(&nested)?;
    let nested_registry = nested.join("schema_registry.incn");
    std::fs::write(&nested_registry, "from schema_registry import registered_columns\n")?;

    let source = std::fs::read_to_string(&nested_registry)?;
    let tokens =
        lexer::lex(&source).map_err(|errors| std::io::Error::other(format!("fixture should lex: {errors:?}")))?;
    let source_path = nested_registry.to_string_lossy();
    let program = parser::parse_with_context_and_surfaces(&tokens, Some(source_path.as_ref()), None, None)
        .map_err(|errors| std::io::Error::other(format!("fixture should parse: {errors:?}")))?;

    let resolved = resolve_program_source_imports(&program, &nested, Some(&src));
    let canonical_nested_registry = nested_registry.canonicalize()?;
    assert!(matches!(
        &resolved[0].resolution,
        SourceModuleImportResolution::SelfImport { module_ref, import_path, can_use_root_import }
            if module_ref.file_path == canonical_nested_registry
                && import_path == &["schema_registry"]
                && *can_use_root_import
    ));

    Ok(())
}

#[test]
fn resolve_source_module_import_keeps_explicit_root_and_distinct_sibling_precedence()
-> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let src = tmp.path().join("src");
    let nested = src.join("substrait");
    std::fs::create_dir_all(&nested)?;
    let root_registry = src.join("schema_registry.incn");
    let nested_registry = nested.join("schema_registry.incn");
    let consumer = nested.join("consumer.incn");
    std::fs::write(&root_registry, "pub const ROOT: int = 1\n")?;
    std::fs::write(&nested_registry, "pub const NESTED: int = 2\n")?;
    std::fs::write(&consumer, "pub const CONSUMER: int = 3\n")?;

    let bare_import = relative_module_import(&["schema_registry"]);
    let SourceModuleImportResolution::Local(sibling) =
        resolve_source_module_import_from_source_file(&nested, Some(&src), Some(&consumer), &bare_import)
    else {
        return Err("expected a distinct same-directory module to remain preferred".into());
    };
    assert_eq!(sibling.file_path, nested_registry.canonicalize()?);

    let root_import = ImportDecl {
        visibility: Visibility::Private,
        kind: ImportKind::Module(ImportPath::absolute(vec!["schema_registry".to_string()])),
        alias: None,
    };
    let SourceModuleImportResolution::Local(root) =
        resolve_source_module_import_from_source_file(&nested, Some(&src), Some(&nested_registry), &root_import)
    else {
        return Err("expected an explicit root import to select the root module".into());
    };
    assert_eq!(root.file_path, root_registry.canonicalize()?);

    Ok(())
}
