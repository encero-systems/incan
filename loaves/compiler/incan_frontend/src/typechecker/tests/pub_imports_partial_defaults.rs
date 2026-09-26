//! A call through a public partial imported from a dependency may omit a leftover parameter exactly when the target's
//! default for it can be filled in, the rule a direct call to the target follows (#1760).

use super::*;

/// The dependency's library name, as consumers import it through `pub::`.
const LIBRARY: &str = "modulelib";

/// Check the dependency's root module and publish its manifest the way a library build does.
fn provider_index(source: &str) -> Result<LibraryManifestIndex, String> {
    let module_path = vec!["lib".to_string()];
    let tokens = lexer::lex(source).map_err(|errors| format!("provider lex failed: {errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("provider parse failed: {errors:?}"))?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(module_path.clone()));
    checker
        .check_program(&program)
        .map_err(|errors| format!("provider failed to check: {errors:?}"))?;
    let exports = collect_checked_public_exports(&program, &checker);
    let mut api_modules = vec![collect_checked_api_metadata(&program, &checker, module_path.clone())];
    materialize_api_alias_projections(&mut api_modules);
    let mut api = CheckedApiMetadataPackage {
        schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
        package: None,
        modules: api_modules,
        public_namespaces: Vec::new(),
    };
    materialize_checked_api_public_namespaces(&mut api).map_err(|error| format!("{error:?}"))?;
    let mut manifest = LibraryManifest::from_checked_exports(LIBRARY, "0.1.0", &exports);
    manifest
        .contract_metadata
        .identity_graph
        .extend_checked_api_exports(LIBRARY, &api, &[(module_path, exports)])
        .map_err(|error| format!("{error:?}"))?;
    manifest.contract_metadata.api = Some(api);
    Ok(LibraryManifestIndex::from_entries(HashMap::from([(
        LIBRARY.to_string(),
        LibraryManifestIndexEntry::Loaded {
            manifest: Box::new(manifest),
            metadata: LibraryArtifactMetadata::from_crate_root(LIBRARY, LIBRARY, synthetic_artifact_root(LIBRARY)),
        },
    )])))
}

/// #1760: `default_build()` over `pub default_build = partial build(size=3)` omits `label`. When `build` declares
/// `label: str = "flat"`, the call is accepted; when its default is `"fl" + "at"`, which a consumer cannot fill in,
/// the call is refused for the missing argument exactly as `build(3)` is, and naming `label` is accepted.
#[test]
fn imported_partial_leftover_follows_its_targets_default_issue1760() -> Result<(), String> {
    let carried = provider_index(
        r#"
pub def build(size: int, label: str = "flat") -> str:
    return label


pub default_build = partial build(size=3)
"#,
    )?;
    check_str_with_library_index(
        "from pub::modulelib import default_build\n\ndef use() -> str:\n  return default_build()\n",
        carried,
    )
    .map_err(|errors| format!("a carried target default makes the leftover optional: {errors:?}"))?;

    let uncarried = provider_index(
        r#"
pub def build(size: int, label: str = "fl" + "at") -> str:
    return label


pub default_build = partial build(size=3)
"#,
    )?;
    for (callee, call) in [("build", "build(3)"), ("default_build", "default_build()")] {
        let errors = check_str_with_library_index_err(
            &format!("from pub::modulelib import build, default_build\n\ndef use() -> str:\n  return {call}\n"),
            uncarried.clone(),
            "an uncarried target default leaves the parameter required",
        )?;
        assert!(
            errors
                .iter()
                .any(|error| error.message == format!("Missing required argument 'label' when calling '{callee}'")),
            "`{call}` is refused for the missing `label`: {errors:?}"
        );
    }
    check_str_with_library_index(
        "from pub::modulelib import default_build\n\ndef use() -> str:\n  return default_build(label=\"tall\")\n",
        uncarried,
    )
    .map_err(|errors| format!("naming the leftover satisfies the partial: {errors:?}"))
}

/// A dependency's default that constructs one of its public models is carried across the package boundary, and so is
/// one naming a variant of its private enum: a call may omit either. A default that constructs a private model is not
/// carried, because a consumer constructs another package's model only through its public path; a call omitting that
/// argument is refused for the missing argument rather than accepted and left to fail in the build.
#[test]
fn dependency_default_constructing_a_private_model_is_not_carried_issue1771() -> Result<(), String> {
    let index = provider_index(
        r#"
model _Default:
    pub size: int = 4


pub model Settings:
    pub size: int = 5


enum _Mode:
    Fast
    Slow


pub def make(settings: _Default = _Default()) -> int:
    return settings.size


pub def configure(settings: Settings = Settings(size=6)) -> int:
    return settings.size


pub def run(mode: _Mode = _Mode.Fast) -> int:
    return 1
"#,
    )?;
    let errors = check_str_with_library_index_err(
        "from pub::modulelib import make\n\ndef use() -> int:\n  return make()\n",
        index.clone(),
        "a construction of a private model is not carried across the package boundary",
    )?;
    assert!(
        errors
            .iter()
            .any(|error| error.message == "Missing required argument 'settings' when calling 'make'"),
        "`make()` is refused for the missing `settings`: {errors:?}"
    );
    for (callee, call) in [("configure", "configure()"), ("run", "run()")] {
        check_str_with_library_index(
            &format!("from pub::modulelib import {callee}\n\ndef use() -> int:\n  return {call}\n"),
            index.clone(),
        )
        .map_err(|errors| format!("`{call}` takes its carried default: {errors:?}"))?;
    }
    Ok(())
}

/// A dependency's default that calls a function or partial it declares, or constructs one of its newtypes, is carried
/// across the package boundary. A default that calls a builtin such as `abs` or `len`, or builds `Some(3)`, is not:
/// a consumer reaches only the package's own callables through the package's path, so a call omitting that argument
/// is refused for the missing argument rather than accepted and left to fail in the build.
#[test]
fn dependency_default_calling_a_builtin_is_not_carried_issue1771() -> Result<(), String> {
    let index = provider_index(
        r#"
pub type Meters = newtype int


def _base(n: int) -> int:
    return n * 2


pub def helper(n: int = _base(2)) -> int:
    return n


pub def measure(m: Meters = Meters(3)) -> int:
    return m.0


pub def absolute(n: int = abs(-3)) -> int:
    return n


pub def length(n: int = len("abc")) -> int:
    return n


pub def wrapped(o: Option[int] = Some(3)) -> int:
    match o:
        Some(v) => return v
        None => return 0
"#,
    )?;
    for (callee, param) in [("absolute", "n"), ("length", "n"), ("wrapped", "o")] {
        let errors = check_str_with_library_index_err(
            &format!("from pub::modulelib import {callee}\n\ndef use() -> int:\n  return {callee}()\n"),
            index.clone(),
            "a default calling something other than a package callable is not carried",
        )?;
        assert!(
            errors
                .iter()
                .any(|error| error.message == format!("Missing required argument '{param}' when calling '{callee}'")),
            "`{callee}()` is refused for the missing `{param}`: {errors:?}"
        );
    }
    for callee in ["helper", "measure"] {
        check_str_with_library_index(
            &format!("from pub::modulelib import {callee}\n\ndef use() -> int:\n  return {callee}()\n"),
            index.clone(),
        )
        .map_err(|errors| format!("`{callee}()` takes its carried default: {errors:?}"))?;
    }
    Ok(())
}
