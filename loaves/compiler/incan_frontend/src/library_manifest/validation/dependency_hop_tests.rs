//! A Loaf republishing what its dependency publishes: the identity graph binds each hop that the dependency's own
//! manifest proves, a renamed function and an overload set alike.

use std::path::Path;
use std::sync::Arc;

use crate::api_metadata::{
    CHECKED_API_METADATA_SCHEMA_VERSION, CheckedApiMetadataPackage, collect_checked_api_metadata,
    materialize_api_alias_projections, materialize_checked_api_public_namespaces,
};
use crate::library_exports::collect_checked_public_exports;
use crate::library_manifest::LibraryManifest;
use crate::loaf_index::{LoafEntry, LoafIndex};
use crate::typechecker::TypeChecker;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// Check `source` as module `module` of `package` (`lib` publishes at the package root) against `loaves`, and
/// publish it as that package's manifest.
fn published(package: &str, module: &str, source: &str, loaves: LoafIndex) -> TestResult<LibraryManifest> {
    let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let path = vec![module.to_owned()];
    let mut checker = TypeChecker::new();
    checker.set_current_package_identity(Some(package.into()));
    checker.set_current_module_path(Some(path.clone()));
    checker.set_loaves(Arc::new(loaves));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{package}.{module}: {errors:?}"))?;
    let exports = collect_checked_public_exports(&program, &checker);
    let mut api = CheckedApiMetadataPackage {
        schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
        package: None,
        modules: vec![collect_checked_api_metadata(&program, &checker, path.clone())],
        public_namespaces: Vec::new(),
    };
    materialize_api_alias_projections(&mut api.modules);
    materialize_checked_api_public_namespaces(&mut api)?;
    let mut manifest = if module == "lib" {
        LibraryManifest::from_checked_exports(package, "0.1.0", &exports)
    } else {
        LibraryManifest::new(package, "0.1.0")
    };
    manifest
        .contract_metadata
        .identity_graph
        .extend_checked_api_exports(package, &api, &[(path, exports)])?;
    manifest.contract_metadata.api = Some(api);
    Ok(manifest)
}

/// Write `manifest` where the index reads a dependency's compiled artifact and admit it under its own name.
fn installed(root: &Path, manifest: &LibraryManifest) -> TestResult<LoafEntry> {
    let directory = root.join(&manifest.name);
    std::fs::create_dir(&directory)?;
    std::fs::write(
        directory.join(format!("{}.incnlib", manifest.name)),
        manifest.to_json_string()?,
    )?;
    Ok(LoafIndex::dependency(&manifest.name, &manifest.name, &directory)?)
}

/// `calc.ops` declares `calculate`; `mid` publishes it at its root as `facade_calculate`. `relay` publishes `mid`'s
/// name again under a third one, at its root and in its module `facade`, and both manifests are valid: the hop
/// `pub.mid.facade_calculate` is `mid`'s, and `mid`'s manifest binds it to `calculate`.
#[test]
fn a_loaf_republishes_a_dependency_rename_under_a_new_name_at_its_root_and_in_a_module() -> TestResult {
    let root = tempfile::tempdir()?;
    let calc = published(
        "calc",
        "ops",
        "pub def calculate(value: int) -> int:\n    return value * 2\n",
        LoafIndex::default(),
    )?;
    let calc = installed(root.path(), &calc)?;
    let mid = published(
        "mid",
        "lib",
        "pub from pub::calc.ops import calculate as facade_calculate\n",
        LoafIndex::new([calc.clone()])?,
    )?;
    let mid = installed(root.path(), &mid)?;
    let republished = [
        ("lib", "pub from pub::mid import facade_calculate as relay_calculate\n"),
        ("facade", "pub from pub::mid import facade_calculate as b_calculate\n"),
    ]
    .iter()
    .map(|(module, source)| {
        published("relay", module, source, LoafIndex::new([calc.clone(), mid.clone()])?)
            .and_then(|manifest| Ok(manifest.to_json_string()?))
    })
    .collect::<Vec<_>>();
    let refusals = republished
        .iter()
        .filter_map(|result| result.as_ref().err().map(ToString::to_string))
        .collect::<Vec<_>>();
    assert!(refusals.is_empty(), "{refusals:?}");
    assert_eq!(republished.len(), 2);
    Ok(())
}

/// `converters` declares two overloads of `choose`; the module `relayed` of `bridge` re-exports the set, and its
/// manifest is valid with one graph entry per overload.
#[test]
fn a_loaf_module_republishes_a_dependency_overload_set_with_one_entry_per_overload() -> TestResult {
    let root = tempfile::tempdir()?;
    let converters = published(
        "converters",
        "lib",
        "pub def choose(value: int) -> int:\n    return 1\n\n\npub def choose(value: float) -> int:\n    return 2\n",
        LoafIndex::default(),
    )?;
    let converters = installed(root.path(), &converters)?;
    let bridge = published(
        "bridge",
        "relayed",
        "pub from pub::converters import choose\n",
        LoafIndex::new([converters])?,
    )?;
    let entries = bridge
        .contract_metadata
        .identity_graph
        .exports
        .iter()
        .filter(|entry| entry.public_path == ["bridge", "relayed", "choose"])
        .count();
    assert_eq!(entries, 2);
    bridge.to_json_string()?;
    Ok(())
}
