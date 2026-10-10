//! Real executable-selected source cache controls with hostile ambient catalog contents.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::sync::Arc;

use crate::library_manifest_index::LibraryManifestIndex;
use crate::provider::ProviderPlan;
use crate::provider::source_policy::TrustedStandardSourcePublication;
use crate::symbols::{ResolvedType, SymbolKind};
use crate::typechecker::TypeChecker;
use incan_lang::lang::standard_packages::standard_package_namespace_policy;
use incan_semantics_core::SymbolOrigin;

use super::StdlibAstCache;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const OWN_IO: &str = "pub const LIMIT: int = 7\npub def owned(value: int = LIMIT) -> int:\n    return value\n";

/// Spell exact canonical module segments for cache queries.
fn path(dotted: &str) -> Vec<String> {
    dotted.split('.').map(str::to_string).collect()
}

/// Write one source file, creating its real confined directory ancestry.
fn write(path: &Path, text: &str) -> TestResult {
    fs::create_dir_all(path.parent().ok_or("missing source parent")?)?;
    fs::write(path, text)?;
    Ok(())
}

/// Exercise actual public discovery in a child; environment overrides are hostile inputs, never source authority.
#[cfg(unix)]
fn child(mode: &str) -> TestResult {
    let root = tempfile::tempdir()?;
    let package = root.path().join("stdlib/system");
    let policy = standard_package_namespace_policy("incan_stdlib_system").ok_or("missing policy")?;
    write(&package.join("loaf.toml"), policy.declaration)?;
    write(&package.join("src/io.incn"), OWN_IO)?;
    write(&package.join("src/fs/prelude.incn"), "pub from std.io import owned\n")?;
    let executable = root.path().join("bin/incan");
    fs::create_dir_all(executable.parent().ok_or("missing executable parent")?)?;
    fs::copy(std::env::current_exe()?, &executable)?;
    let decoy = tempfile::tempdir()?;
    write(
        &decoy.path().join("sdk-components.toml"),
        r#"
[sdk]
id = "incan"
version = "0.6.0-dev.6"
compiler-requirement = ">=0.6.0-dev.6,<0.7.0"
[profiles]
minimal = ["stdlib-system", "stdlib-core"]
default = ["stdlib-system", "stdlib-core"]
[components.stdlib-system]
project = "system"
namespace-roots = ["io", "fs", "environ", "tempfile"]
[components.stdlib-core]
project = "core"
namespace-roots = ["result"]
"#,
    )?;
    write(&decoy.path().join("system/loaf.toml"), policy.declaration)?;
    write(
        &decoy.path().join("system/src/io.incn"),
        "pub def owned() -> str:\n    return \"ambient\"\n",
    )?;
    write(
        &decoy.path().join("system/src/fs/prelude.incn"),
        "pub from std.io import owned\n",
    )?;
    write(
        &decoy.path().join("core/src/result.incn"),
        "pub def foreign() -> str:\n    return \"ambient\"\n",
    )?;
    let output = std::process::Command::new(executable)
        .args([
            "--exact",
            "typechecker::stdlib_loader::source_context_tests::dev7_standard_source_cache_actual_executable_child",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("INCAN_DEV7_STDLIB_CACHE_CHILD", mode)
        .env("INCAN_STDLIB", decoy.path())
        .env("INCAN_SOURCE_ROOT", decoy.path())
        .env("INCAN_INTERNAL_TOOLCHAIN_DATA_ROOT", decoy.path())
        .current_dir(decoy.path())
        .output()?;
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
    Ok(())
}

/// Own signatures/reexports/defaults and callable identities come from retained bytes; repeated lookups do not parse.
#[cfg(unix)]
#[test]
fn dev7_standard_source_cache_uses_owned_bytes_and_recursive_metadata() -> TestResult {
    child("valid")
}

/// An ordinary admitted empty consumer refuses productive ambient source instead of inheriting legacy permission.
#[cfg(unix)]
#[test]
fn dev7_standard_source_cache_ordinary_empty_refuses_ambient() -> TestResult {
    child("ordinary-empty")
}

/// A missing owned module refuses even though a hostile ambient implementation is available.
#[cfg(unix)]
#[test]
fn dev7_standard_source_cache_refuses_missing_own_file() -> TestResult {
    child("missing")
}

/// A symlinked owned module refuses even if it resolves to identical bytes inside the same package.
#[cfg(unix)]
#[test]
fn dev7_standard_source_cache_refuses_symlinked_own_file() -> TestResult {
    child("symlink")
}

/// Changed member bytes and equal-byte replacement refuse reuse, while restoration and fresh authority remain valid.
#[cfg(unix)]
#[test]
fn dev7_standard_source_cache_revalidates_original_members_and_context() -> TestResult {
    child("freshness")
}

/// A member first read through a clone remains an original owner verified by its untouched parent cache.
#[cfg(unix)]
#[test]
fn dev7_standard_source_cache_clone_new_member_is_retained_by_parent() -> TestResult {
    child("clone-new-member")
}

/// The real copied executable enters the public factory, then binds that exact Arc through the real checker adapter.
#[cfg(unix)]
#[test]
fn dev7_standard_source_cache_actual_executable_child() -> TestResult {
    let Some(mode) = std::env::var_os("INCAN_DEV7_STDLIB_CACHE_CHILD") else {
        return Ok(());
    };
    let io = path("std.io");
    let fs_module = path("std.fs");
    let mut legacy = StdlibAstCache::new();
    assert!(
        matches!(legacy.lookup_function_symbol(&io, "owned"), Some(SymbolKind::Function(info)) if info.return_type == ResolvedType::Str)
    );
    assert!(legacy.lookup_function_symbol(&path("std.result"), "foreign").is_some());
    let ordinary = Arc::new(ProviderPlan::from_admitted_libraries(
        LibraryManifestIndex::default(),
        &[],
        std::iter::empty(),
    )?);
    let mut checked_only = TypeChecker::new();
    checked_only.stdlib_cache = legacy.clone();
    checked_only.set_provider_plan(Arc::clone(&ordinary));
    assert!(checked_only.stdlib_cache.lookup_function_symbol(&io, "owned").is_none());
    assert!(
        checked_only
            .stdlib_cache
            .lookup_function_symbol(&path("std.result"), "foreign")
            .is_none()
    );
    checked_only.stdlib_cache.verify_retained_sources()?;
    if mode == "ordinary-empty" {
        checked_only.set_library_manifest_index(LibraryManifestIndex::default());
        assert!(checked_only.stdlib_cache.lookup_function_symbol(&io, "owned").is_some());
        checked_only.set_provider_plan(Arc::clone(&ordinary));
        assert!(checked_only.stdlib_cache.lookup_function_symbol(&io, "owned").is_none());
        checked_only.set_library_manifest_index_shared(Arc::new(LibraryManifestIndex::default()));
        assert!(checked_only.stdlib_cache.lookup_function_symbol(&io, "owned").is_some());
        return Ok(());
    }
    let source =
        Arc::new(TrustedStandardSourcePublication::discover("incan_stdlib_system")?.ok_or("missing genuine source")?);
    let package = source.verified_package_root()?.to_path_buf();
    let plan = Arc::new(ordinary.as_ref().clone().with_standard_source_publication(
        Arc::clone(&source),
        &package,
        "incan_stdlib_system",
        "0.5.0",
    )?);
    let mut checker = TypeChecker::new();
    checker.stdlib_cache = legacy;
    checker.set_provider_plan(Arc::clone(&plan));
    if mode == "clone-new-member" {
        let mut clone = checker.stdlib_cache.clone();
        assert!(clone.lookup_function_meta(&io, "owned").is_some());
        assert_eq!(checker.stdlib_cache.source_inputs.parses(), 0);
        assert_eq!(clone.source_inputs.parses(), 1);
        let file = package.join("src/io.incn");
        let modified = fs::metadata(&file)?.modified()?;
        fs::write(&file, OWN_IO.replace("7", "9"))?;
        fs::File::open(&file)?.set_modified(modified)?;
        assert!(checker.stdlib_cache.verify_retained_sources().is_err());
        return Ok(());
    }
    if mode == "missing" {
        fs::remove_file(package.join("src/io.incn"))?;
        let mut clone = checker.stdlib_cache.clone();
        assert!(clone.lookup_function_symbol(&io, "owned").is_none());
        checker.stdlib_cache.bind_provider_plan(&plan);
        assert!(checker.stdlib_cache.verify_retained_sources().is_err());
        return Ok(());
    }
    if mode == "symlink" {
        let file = package.join("src/io.incn");
        fs::rename(&file, package.join("src/original.incn"))?;
        std::os::unix::fs::symlink(package.join("src/original.incn"), file)?;
        assert!(checker.stdlib_cache.lookup_function_symbol(&io, "owned").is_none());
        assert!(checker.stdlib_cache.verify_retained_sources().is_err());
        return Ok(());
    }
    let function = checker
        .stdlib_cache
        .lookup_function_source(&fs_module, "owned")
        .ok_or("missing own reexport")?;
    assert_eq!(function.default_const_paths.get("LIMIT"), Some(&path("std.io.LIMIT")));
    assert!(
        matches!(checker.stdlib_cache.lookup_function_symbol(&fs_module, "owned"), Some(SymbolKind::Function(info)) if info.return_type == ResolvedType::Int)
    );
    assert!(
        checker
            .stdlib_cache
            .lookup_function_symbol(&path("std.result"), "foreign")
            .is_none()
    );
    assert_eq!(checker.stdlib_cache.source_inputs.parses(), 2);
    let mut repeat = checker.stdlib_cache.clone();
    repeat.bind_provider_plan(&plan);
    assert!(repeat.lookup_function_symbol(&fs_module, "owned").is_some());
    repeat.verify_retained_sources()?;
    assert_eq!(repeat.source_inputs.parses(), 2);
    let source_identity = repeat.lookup_identity(&io, "owned").ok_or("missing own identity")?;
    let mut package_identity = source_identity.clone();
    package_identity.origin = SymbolOrigin::Package {
        library: "incan_stdlib_system".to_string(),
        module_path: path("io"),
    };
    assert_eq!(
        repeat.callable_source_identity(&package_identity),
        Some(source_identity)
    );
    assert!(repeat.callable_program(&package_identity).is_some());
    package_identity.origin = SymbolOrigin::Package {
        library: "incan_stdlib_core".to_string(),
        module_path: path("io"),
    };
    assert!(repeat.callable_program(&package_identity).is_none());
    let tokens = crate::lexer::lex("from std.io import owned\ndef consume() -> int:\n    return owned()\n")
        .map_err(|error| format!("lex: {error:?}"))?;
    let program = crate::parser::parse(&tokens).map_err(|error| format!("parse: {error:?}"))?;
    checker
        .check_program(&program)
        .map_err(|error| format!("own import check: {error:?}"))?;
    if mode == "freshness" {
        let file = package.join("src/io.incn");
        let modified = fs::metadata(&file)?.modified()?;
        fs::write(&file, OWN_IO.replace("7", "9"))?;
        fs::File::open(&file)?.set_modified(modified)?;
        assert!(repeat.verify_retained_sources().is_err());
        checker.stdlib_cache = repeat.clone();
        checker.set_provider_plan(Arc::clone(&plan));
        assert!(checker.check_program(&program).is_err());
        fs::write(&file, OWN_IO)?;
        fs::File::open(&file)?.set_modified(modified)?;
        repeat.bind_provider_plan(&plan);
        assert!(repeat.lookup_function_symbol(&io, "owned").is_some());
        repeat.verify_retained_sources()?;
        fs::rename(&file, package.join("src/held.incn"))?;
        fs::write(&file, OWN_IO)?;
        assert!(repeat.verify_retained_sources().is_err());
        let next =
            Arc::new(TrustedStandardSourcePublication::discover("incan_stdlib_system")?.ok_or("missing fresh source")?);
        let fresh = ordinary.as_ref().clone().with_standard_source_publication(
            next,
            &package,
            "incan_stdlib_system",
            "0.5.0",
        )?;
        repeat.bind_provider_plan(&fresh);
        assert!(repeat.lookup_function_symbol(&io, "owned").is_some());
        repeat.verify_retained_sources()?;
        assert_eq!(repeat.source_inputs.parses(), 1);
    }
    // No installed namespace grant or materialized/native artifact was introduced by own source cache admission.
    assert!(plan.public_artifacts().next().is_none());
    assert_eq!(
        source.namespace_roots().iter().copied().collect::<BTreeSet<_>>(),
        BTreeSet::from(["environ", "io", "tempfile", "fs"])
    );
    Ok(())
}
