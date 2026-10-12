//! Actual lowering/emission handoffs preserve source authority and refuse ambient or stale cache replacements.

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::Arc;

use incan_frontend::ast::{Declaration, Program, Visibility};
use incan_frontend::library_manifest_index::LibraryManifestIndex;
use incan_frontend::provider::ProviderPlan;
use incan_frontend::provider::source_policy::TrustedStandardSourcePublication;
use incan_frontend::typechecker::stdlib_loader::StdlibAstCache;
use incan_frontend::typechecker::{TypeCheckInfo, TypeChecker};
use incan_ir::AstLowering;
use incan_lang::lang::standard_packages::standard_package_namespace_policy;
use incan_semantics_core::encode_incan_symbol_identity;

use crate::backend::IrCodegen;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const OWN_IO: &str = "pub def owned() -> int:\n    return 7\n";
const MAIN: &str = "def main() -> None:\n    pass\n";
const ROUTES: &str = "from std.web.routing import route\n@route(\"/\")\ndef hidden() -> str:\n    return \"handler\"\n";
// The public signature in loaves/stdlib/web/src/web/routing.incn, including its actual defaulted methods parameter.
const WEB_ROUTING: &str = "rust.module(\"incan_web_macros\")\n@rust.extern\npub def route(path: str, methods: list[str] = [\"GET\"]) -> None:\n    ...\n";

/// Parse fixture source through the real frontend without introducing test-only semantic shortcuts.
fn parse(source: &str) -> Result<Program, String> {
    let tokens = incan_frontend::lexer::lex(source).map_err(|error| format!("lex: {error:?}"))?;
    incan_frontend::parser::parse(&tokens).map_err(|error| format!("parse: {error:?}"))
}

/// Obtain genuine checked callable identities for the unchanged fixture AST before testing stage cache authority.
fn checked(program: &Program, path: &str, plan: Option<&Arc<ProviderPlan>>) -> Result<TypeCheckInfo, String> {
    let mut checker = TypeChecker::new();
    if let Some(plan) = plan {
        checker.set_provider_plan(Arc::clone(plan));
    }
    checker.set_current_module_path(Some(module(path)));
    checker
        .check_program(program)
        .map_err(|error| format!("check: {error:?}"))?;
    Ok(checker.type_info().clone())
}

/// Spell a canonical module path for the public cache API.
fn module(path: &str) -> Vec<String> {
    path.split('.').map(str::to_string).collect()
}

/// Write a real source member with a confined, ordinary directory ancestry.
fn write(path: &Path, text: &str) -> TestResult {
    fs::create_dir_all(path.parent().ok_or("missing source parent")?)?;
    fs::write(path, text)?;
    Ok(())
}

/// Run the copied native test executable in fixed installed geometry, isolating all hostile ambient inputs.
#[cfg(unix)]
fn child(mode: &str) -> TestResult {
    let root = tempfile::tempdir()?;
    let policy = standard_package_namespace_policy("incan_stdlib_system").ok_or("missing pinned policy")?;
    write(&root.path().join("stdlib/system/loaf.toml"), policy.declaration)?;
    write(&root.path().join("stdlib/system/src/io.incn"), OWN_IO)?;
    let web_policy = standard_package_namespace_policy("incan_stdlib_web").ok_or("missing pinned web policy")?;
    write(&root.path().join("stdlib/web/loaf.toml"), web_policy.declaration)?;
    write(&root.path().join("stdlib/web/src/web/routing.incn"), WEB_ROUTING)?;
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
minimal = ["stdlib-system", "stdlib-web"]
default = ["stdlib-system", "stdlib-web"]
[components.stdlib-system]
project = "system"
namespace-roots = ["io", "fs", "environ", "tempfile"]
[components.stdlib-web]
project = "web"
namespace-roots = ["web"]
"#,
    )?;
    write(&decoy.path().join("system/loaf.toml"), policy.declaration)?;
    write(
        &decoy.path().join("system/src/io.incn"),
        "pub def owned() -> str:\n    return \"ambient\"\n",
    )?;
    write(&decoy.path().join("web/src/web/routing.incn"), WEB_ROUTING)?;
    let output = std::process::Command::new(executable)
        .args([
            "--exact",
            "source_authority_stage_tests::dev7_standard_source_stages_actual_executable_child",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("INCAN_DEV7_SOURCE_STAGES_CHILD", mode)
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

/// An ordinary empty plan rejects productive ambient web reachability under both stage setter orders.
#[cfg(unix)]
#[test]
fn dev7_standard_source_stages_ordinary_empty_and_hostile_web() -> TestResult {
    child("ordinary")
}

/// Original source survives both setter orders and explicit legacy replacement invalidates trusted cache entries.
#[cfg(unix)]
#[test]
fn dev7_standard_source_stages_setter_orders_and_legacy_replacement() -> TestResult {
    child("orders")
}

/// Preserved modification time cannot hide changed bytes from either stage's prechecked handoff.
#[cfg(unix)]
#[test]
fn dev7_standard_source_stages_refuse_preserved_time_mutation() -> TestResult {
    child("mutation")
}

/// An equal-byte replacement cannot substitute the originally retained member at a stage handoff.
#[cfg(unix)]
#[test]
fn dev7_standard_source_stages_refuse_equal_byte_replacement() -> TestResult {
    child("replacement")
}

/// Supply genuine source authority through public factories inside the actual copied executable.
#[cfg(unix)]
#[test]
fn dev7_standard_source_stages_actual_executable_child() -> TestResult {
    let Some(mode) = std::env::var_os("INCAN_DEV7_SOURCE_STAGES_CHILD") else {
        return Ok(());
    };
    let mode = mode.to_str().ok_or("invalid child mode")?;
    let main = parse(MAIN)?;
    let routes = parse(ROUTES)?;
    let main_info = checked(&main, "main", None)?;
    // Check the handler through genuine executable-selected web authority. The separate ambient cache below is
    // deliberately hostile input to the stages, and must not be responsible for manufacturing callable identities.
    let web_source =
        Arc::new(TrustedStandardSourcePublication::discover("incan_stdlib_web")?.ok_or("missing genuine web source")?);
    let web_package = web_source.verified_package_root()?.to_path_buf();
    let web_plan = Arc::new(
        ProviderPlan::from_admitted_libraries(LibraryManifestIndex::default(), &[], std::iter::empty())?
            .with_standard_source_publication(web_source, &web_package, "incan_stdlib_web", "0.5.0")?,
    );
    // Cache readers intentionally return no metadata after a parser refusal. Assert the real exported declaration
    // and productive retained-source metadata before using the checker, so malformed fixture syntax cannot masquerade
    // as an import-authority failure.
    let parsed_web = parse(WEB_ROUTING)?;
    assert!(parsed_web.declarations.iter().any(|declaration| {
        matches!(&declaration.node, Declaration::Function(function)
            if function.name == "route" && function.visibility == Visibility::Public)
    }));
    let mut own_web_cache = StdlibAstCache::new();
    own_web_cache.bind_provider_plan(&web_plan);
    let web = module("std.web.routing");
    assert!(own_web_cache.lookup_function_symbol(&web, "route").is_some());
    let own_metadata = own_web_cache
        .lookup_function_meta(&web, "route")
        .ok_or("genuine retained web metadata is not productive")?;
    assert!(own_metadata.is_rust_extern);
    assert_eq!(own_metadata.rust_module_path.as_deref(), Some("incan_web_macros"));
    own_web_cache.verify_retained_sources()?;
    let routes_info = checked(&routes, "routes", Some(&web_plan))?;
    let hidden_identity = routes_info
        .declarations
        .function_bindings_by_span
        .values()
        .filter_map(|binding| binding.identity.as_ref())
        .find(|identity| identity.declaration_name == "hidden")
        .ok_or("missing checked handler identity")?;
    let hidden_definition = format!("fn {}", encode_incan_symbol_identity(hidden_identity));
    let mut legacy = StdlibAstCache::new();
    let metadata = legacy
        .lookup_function_meta(&web, "route")
        .ok_or("hostile web metadata is not productive")?;
    assert!(metadata.is_rust_extern);
    assert_eq!(metadata.rust_module_path.as_deref(), Some("incan_web_macros"));
    let ordinary = Arc::new(ProviderPlan::from_admitted_libraries(
        LibraryManifestIndex::default(),
        &[],
        std::iter::empty(),
    )?);
    if mode == "ordinary" {
        for plan_first in [false, true] {
            let mut lowering = AstLowering::new_with_type_info(main_info.clone());
            bind_lowering(&mut lowering, &ordinary, legacy.clone(), plan_first);
            assert!(lowering.stdlib_cache.lookup_function_meta(&web, "route").is_none());
            lowering.lower_program(&main)?;
            let mut codegen = IrCodegen::new();
            bind_codegen(&mut codegen, &ordinary, legacy.clone(), plan_first);
            assert!(!generate_routes(codegen, &main, &routes, &routes_info)?.contains(&hidden_definition));
        }
        // Explicit legacy generation proves the hostile metadata would otherwise keep this unimported handler.
        assert!(generate_routes(IrCodegen::new(), &main, &routes, &routes_info)?.contains(&hidden_definition));
        return Ok(());
    }
    let source =
        Arc::new(TrustedStandardSourcePublication::discover("incan_stdlib_system")?.ok_or("missing genuine source")?);
    let package = source.verified_package_root()?.to_path_buf();
    let plan = Arc::new(ordinary.as_ref().clone().with_standard_source_publication(
        source,
        &package,
        "incan_stdlib_system",
        "0.5.0",
    )?);
    let io = module("std.io");
    let mut retained = StdlibAstCache::new();
    retained.bind_provider_plan(&plan);
    assert!(retained.lookup_function_meta(&io, "owned").is_some());
    retained.verify_retained_sources()?;
    if mode == "orders" {
        for plan_first in [false, true] {
            let mut lowering = AstLowering::new_with_type_info(main_info.clone());
            bind_lowering(&mut lowering, &plan, retained.clone(), plan_first);
            assert!(lowering.stdlib_cache.lookup_function_meta(&io, "owned").is_some());
            assert!(lowering.stdlib_cache.lookup_function_meta(&web, "route").is_none());
            lowering.lower_program(&main)?;
            lowering.set_provider_plan(None);
            assert!(lowering.stdlib_cache.lookup_function_meta(&web, "route").is_some());
            let mut codegen = IrCodegen::new();
            bind_codegen(&mut codegen, &plan, retained.clone(), plan_first);
            assert!(!generate_routes(codegen, &main, &routes, &routes_info)?.contains(&hidden_definition));
            let mut replaced = IrCodegen::new();
            bind_codegen(&mut replaced, &plan, retained.clone(), plan_first);
            replaced.set_library_manifest_index(LibraryManifestIndex::default());
            assert!(generate_routes(replaced, &main, &routes, &routes_info)?.contains(&hidden_definition));
        }
        return Ok(());
    }
    let mut lowering = AstLowering::new_with_type_info(main_info.clone());
    bind_lowering(&mut lowering, &plan, retained.clone(), false);
    let mut generators = Vec::new();
    for plan_first in [false, true] {
        let mut codegen = IrCodegen::new();
        bind_codegen(&mut codegen, &plan, retained.clone(), plan_first);
        codegen.set_prechecked_type_info(main_info.clone(), HashMap::new());
        generators.push(codegen);
    }
    let file = package.join("src/io.incn");
    let modified = fs::metadata(&file)?.modified()?;
    match mode {
        "mutation" => fs::write(&file, OWN_IO.replace('7', "9"))?,
        "replacement" => {
            fs::rename(&file, package.join("src/original.incn"))?;
            fs::write(&file, OWN_IO)?;
        }
        _ => return Err("unknown child mode".into()),
    }
    fs::File::open(&file)?.set_modified(modified)?;
    let error = lowering.lower_program(&main).err().ok_or("stale lowering succeeded")?;
    assert!(error.to_string().contains("retained standard source metadata refused"));
    for codegen in generators {
        let error = codegen
            .try_generate(&main)
            .err()
            .ok_or("stale prechecked emission succeeded")?;
        assert!(error.to_string().contains("retained standard source metadata refused"));
    }
    Ok(())
}

/// Exercise both public setter orders without replacing the retained source capability with a reconstructed one.
fn bind_lowering(lowering: &mut AstLowering, plan: &Arc<ProviderPlan>, cache: StdlibAstCache, plan_first: bool) {
    if plan_first {
        lowering.set_provider_plan(Some(Arc::clone(plan)));
        lowering.set_stdlib_cache(cache);
    } else {
        lowering.set_stdlib_cache(cache);
        lowering.set_provider_plan(Some(Arc::clone(plan)));
    }
}

/// Exercise the emission setters in the same order-independent way as standalone embedding callers.
fn bind_codegen(codegen: &mut IrCodegen<'_>, plan: &Arc<ProviderPlan>, cache: StdlibAstCache, plan_first: bool) {
    if plan_first {
        codegen.set_provider_plan(Arc::clone(plan));
        codegen.set_stdlib_cache(cache);
    } else {
        codegen.set_stdlib_cache(cache);
        codegen.set_provider_plan(Arc::clone(plan));
    }
}

/// Use the actual nested generation route, including production reachability collection and source lowering.
fn generate_routes<'a>(
    mut codegen: IrCodegen<'a>,
    main: &'a Program,
    routes: &'a Program,
    routes_info: &TypeCheckInfo,
) -> Result<String, String> {
    let path = module("routes");
    codegen.add_module_with_path_segments("routes", routes, path.clone());
    codegen.set_preserve_dependency_public_items(false);
    codegen.set_prechecked_type_info(
        checked(main, "main", None)?,
        HashMap::from([(path.clone(), routes_info.clone())]),
    );
    let (_, modules) = codegen
        .try_generate_multi_file_nested(main, &[path.clone()])
        .map_err(|error| error.to_string())?;
    modules
        .get(&path)
        .cloned()
        .ok_or("missing emitted routes module".to_string())
}
