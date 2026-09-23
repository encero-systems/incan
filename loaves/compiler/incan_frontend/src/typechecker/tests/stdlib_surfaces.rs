//! Stdlib module surfaces: SDK provider catalogs (#1435), the web wrapper, `std.testing` markers and fixtures, the
//! RFC 018 assert forms, `std.environ` (`args`, `get_as`), and `std.json` value indexing.

use super::*;

#[test]
fn test_web_wrapper_value_and_deref_access() {
    let source = r#"
from std.web import Json, Query
from std.serde import json

@derive(json)
model SearchParams:
  q: str

@derive(json)
model CreateUser:
  name: str

def use_query(params: Query[SearchParams]) -> str:
  let a = params.q
  let b = params.value.q
  return b

def use_body(body: Json[CreateUser]) -> str:
  let a = body.name
  let b = body.value.name
  return b
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_web_wrapper_invalid_constructor_args() {
    let source = r#"
from std.web import Json, Query
from std.serde import json
from std.serde.json import Serialize

@derive(Serialize)
model User:
  name: str

@derive(json)
model SearchParams:
  q: str

def bad_json() -> None:
  let a = Json(User(name="a"), User(name="b"))

def bad_query() -> None:
  let b = Query(value=SearchParams(q="x"), other=SearchParams(q="y"))
"#;
    let Err(errs) = check_str(source) else {
        panic!("expected type errors");
    };
    assert!(
        errs.iter()
            .any(|e| e.message.contains("Json() expects exactly one argument"))
    );
    assert!(
        errs.iter()
            .any(|e| e.message.contains("Query() expects exactly one argument"))
    );
}

// ---- #1721: a route handler returns a response type ----

/// Collect the messages of the diagnostics carrying `code`, in report order.
fn messages_with_code(errors: &[CompileError], code: &str) -> Vec<String> {
    errors
        .iter()
        .filter(|error| error.stable_code() == Some(code))
        .map(|error| error.message.clone())
        .collect()
}

#[test]
fn route_handler_returning_int_is_refused_with_the_response_types_named_issue1721() {
    // The program from #1721: both handlers compute a number and declare it as the response.
    let source = r#"
import std.async
from std.web import route, GET, POST

@route("/posts/{year}/{month}", methods=[GET])
async def get_posts(year: int, month: int) -> int:
    return year + month

@route("/users/{id}", methods=[POST])
async def create_user(id: int) -> int:
    return id

async def main() -> None:
    println(await create_user(7))
"#;
    let errors = check_str_err(source, "a route handler returning int must be refused");
    assert_eq!(
        messages_with_code(&errors, "INCAN-T0107"),
        vec![
            "Route handler 'get_posts' returns 'int', which is not a response type",
            "Route handler 'create_user' returns 'int', which is not a response type",
        ],
        "one report per handler, in source order; got {:?}",
        errors.iter().map(|error| &error.message).collect::<Vec<_>>()
    );
    let Some(refused) = errors.iter().find(|error| error.stable_code() == Some("INCAN-T0107")) else {
        panic!("the refusal must carry its stable code");
    };
    assert!(
        refused
            .hints
            .iter()
            .any(|hint| hint.contains("'str'") && hint.contains("'Json[...]'") && hint.contains("str(value)")),
        "the hint names the response types and the text form, got {:?}",
        refused.hints
    );
}

#[test]
fn route_handler_non_response_return_shapes_are_refused_issue1721() {
    // A tuple of numbers, a list, a plain model and a bool have no response form either; each is refused once,
    // through the canonical decorator path as through the prelude re-export, and the `Result` whose sides are
    // responses is not. A wrapper that derives `IntoResponse` counts as a response.
    let source = r#"
import std.async
from std.web import route, Json, IntoResponse
from std.serde import json

@derive(json)
model Reply:
  value: str

@derive(IntoResponse)
type Wrapped = newtype str

@route("/pair")
async def pair() -> tuple[int, int]:
    return (1, 2)

@route("/many")
async def many() -> list[str]:
    return ["a"]

@route("/plain")
async def plain() -> Reply:
    return Reply(value="a")

@std.web.routing.route("/flag")
async def flag() -> bool:
    return True

@route("/either")
async def either() -> Result[Json[Reply], str]:
    return Ok(Json(Reply(value="a")))

@route("/wrapped")
async def wrapped() -> Wrapped:
    return Wrapped("a")
"#;
    let errors = check_str_err(source, "non-response return shapes must be refused");
    assert_eq!(
        messages_with_code(&errors, "INCAN-T0107"),
        vec![
            "Route handler 'pair' returns '(int, int)', which is not a response type",
            "Route handler 'many' returns 'List[str]', which is not a response type",
            "Route handler 'plain' returns 'Reply', which is not a response type",
            "Route handler 'flag' returns 'bool', which is not a response type",
        ],
        "got {:?}",
        errors.iter().map(|error| &error.message).collect::<Vec<_>>()
    );
}

#[test]
fn route_handlers_returning_response_types_are_accepted_issue1721() {
    // The corrected program from #1721 beside every documented response type, through the prelude re-export and
    // the canonical decorator path alike.
    assert_check_ok(
        r#"
import std.async
from std.web import route, GET, POST, Json, Html, Response
from std.serde import json

@derive(json)
model Reply:
  value: str

@route("/posts/{year}/{month}", methods=[GET])
async def get_posts(year: int, month: int) -> str:
    total = year + month
    return str(total)

@route("/users/{id}", methods=[POST])
async def create_user(id: int) -> str:
    return str(id)

@std.web.routing.route("/reply/{id}")
async def reply(id: int) -> Json[Reply]:
    return Json(Reply(value=str(id)))

@route("/page")
async def page() -> Html:
    return Html("<p>hi</p>")

@route("/health")
async def health() -> Response:
    return Response.ok()

@route("/nothing")
async def nothing() -> None:
    pass

async def main() -> None:
    println(await create_user(7))
"#,
    );
}

// ---- #1722: a route handler parameter is bound by the path or read from the request ----

#[test]
fn route_handler_parameter_without_a_segment_is_refused_with_the_bound_path_issue1722() {
    // The program from #1722: `id` is neither a `{segment}` of `/things` nor an extractor.
    let source = r#"
import std.async
from std.web import route, POST

@route("/things", methods=[POST])
async def create(id: int) -> str:
    return str(id)

async def main() -> None:
    println(await create(7))
"#;
    let errors = check_str_err(source, "an unbound route parameter must be refused");
    assert_eq!(
        messages_with_code(&errors, "INCAN-T0108"),
        vec!["Route handler 'create' has a parameter 'id' that no segment of the path '/things' binds"],
        "got {:?}",
        errors.iter().map(|error| &error.message).collect::<Vec<_>>()
    );
    let Some(refused) = errors.iter().find(|error| error.stable_code() == Some("INCAN-T0108")) else {
        panic!("the refusal must carry its stable code");
    };
    assert!(
        refused.hints.iter().any(|hint| hint.contains("'/things/{id}'")),
        "the hint spells the path with the segment added, got {:?}",
        refused.hints
    );
    assert!(
        !refused.notes.iter().any(|note| note.starts_with("The path binds")),
        "a path with no captures lists none, got {:?}",
        refused.notes
    );
}

#[test]
fn route_handler_parameter_misspelled_against_the_captures_lists_them_issue1722() {
    // `month` is captured but the handler spells `mon`; the report names what the path does bind. The `Query`
    // parameter and the extractor-derived wrapper need no segment.
    let source = r#"
import std.async
from std.web import route, Query, FromRequestParts
from std.serde import json

@derive(json)
model Filter:
  q: str

@derive(FromRequestParts)
type Token = newtype str

@route("/posts/{year}/{month}")
async def get_posts(year: int, mon: int, query: Query[Filter], token: Token) -> str:
    return str(year)
"#;
    let errors = check_str_err(source, "a parameter the captures do not spell must be refused");
    assert_eq!(
        messages_with_code(&errors, "INCAN-T0108"),
        vec![
            "Route handler 'get_posts' has a parameter 'mon' that no segment of the path '/posts/{year}/{month}' binds"
        ],
        "got {:?}",
        errors.iter().map(|error| &error.message).collect::<Vec<_>>()
    );
    let Some(refused) = errors.iter().find(|error| error.stable_code() == Some("INCAN-T0108")) else {
        panic!("the refusal must carry its stable code");
    };
    assert!(
        refused
            .notes
            .iter()
            .any(|note| note == "The path binds 'year', 'month'"),
        "got {:?}",
        refused.notes
    );
}

#[test]
fn route_handler_parameters_bound_by_segments_or_extractors_are_accepted_issue1722() {
    // The corrected program from #1722, a wildcard capture, an unused typed `Path`, and the extractor wrappers.
    assert_check_ok(
        r#"
import std.async
from std.web import route, POST, Json, Query, Path
from std.serde import json

@derive(json)
model Search:
  q: str

@derive(json)
model Update:
  name: str

@route("/things/{id}", methods=[POST])
async def create(id: int) -> str:
    return str(id)

@route("/files/{*path}")
async def file(path: str) -> str:
    return path

@route("/typed/{id}")
async def typed(_: Path[int]) -> str:
    return "typed"

@route("/mixed/{id}", methods=[POST])
async def mixed(id: int, query: Query[Search], body: Json[Update]) -> Json[Update]:
    return Json(Update(name=f"{id} {query.q} {body.name}"))

async def main() -> None:
    println(await create(3))
"#,
    );
}

fn provider_plan_for_sdk_module(
    module: &[&str],
    enabled: bool,
    available: bool,
) -> Result<Arc<ProviderPlan>, ProviderPlanError> {
    provider_plan_for_sdk_modules(&[module], enabled, available)
}

fn provider_plan_for_sdk_modules(
    modules: &[&[&str]],
    enabled: bool,
    available: bool,
) -> Result<Arc<ProviderPlan>, ProviderPlanError> {
    let manifest = available.then(|| Arc::new(LibraryManifest::new("incan_stdlib_web", "0.5.0")));
    let namespace_claims = modules
        .iter()
        .map(|module| module.iter().map(|segment| (*segment).to_string()).collect())
        .collect::<BTreeSet<Vec<String>>>();
    ProviderPlan::new(
        LibraryManifestIndex::default(),
        vec![ProviderRecord {
            identity: ProviderIdentity {
                name: "incan_stdlib_web".to_string(),
                version: "0.5.0".to_string(),
                digest: "sha256:fixture".to_string(),
                feature_projection: BTreeSet::new(),
            },
            provenance: ProviderProvenance::Sdk {
                sdk_identity: "incan@0.5.0".to_string(),
                component_id: "stdlib-web".to_string(),
                inventory_path: None,
            },
            authority: NamespaceAuthority::SdkReserved,
            namespace_claims: namespace_claims.clone(),
            available,
            enabled,
            manifest,
            artifact: None,
            implementation_facets: Vec::new(),
        }],
        namespace_claims,
    )
    .map(Arc::new)
}

/// Build one in-memory SDK provider plan publishing `std.helpers` from a checked provider source.
///
/// The plan carries the provider's checked API and identity graph exactly as an installed artifact would, so a
/// consumer test proves what an import binds against the compiled provider rather than against provider source.
fn sdk_provider_plan_for_helpers_module(
    package_name: &str,
    provider_source: &str,
) -> Result<ProviderPlan, Box<dyn std::error::Error>> {
    let module_path = vec!["helpers".to_string()];
    let provider_ast = parse_program(provider_source, "checked SDK provider");
    let mut provider_checker = TypeChecker::new();
    provider_checker.set_current_package_identity(Some(package_name.to_string()));
    provider_checker.set_current_module_path(Some(module_path.clone()));
    provider_checker
        .check_program(&provider_ast)
        .map_err(|errors| format!("checked SDK provider should typecheck: {errors:?}"))?;
    let checked_exports = collect_checked_public_exports(&provider_ast, &provider_checker);
    let mut api = CheckedApiMetadataPackage {
        schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
        package: None,
        modules: vec![collect_checked_api_metadata(
            &provider_ast,
            &provider_checker,
            module_path.clone(),
        )],
        public_namespaces: Vec::new(),
    };
    materialize_checked_api_public_namespaces(&mut api)?;
    let mut identity_graph = LibraryIdentityGraph::from_checked_exports(package_name, &[]);
    identity_graph.extend_checked_api_exports(package_name, &api, &[(module_path, checked_exports)])?;
    let public_path = vec![package_name.to_string(), "helpers".to_string(), "helper".to_string()];
    if identity_graph.canonical_for_public_path(&public_path).is_none() {
        return Err(format!(
            "fixture identity graph did not retain {public_path:?}: {:?}",
            identity_graph.exports
        )
        .into());
    }
    let mut manifest = LibraryManifest::new(package_name, "0.5.0");
    manifest.contract_metadata.api = Some(api);
    manifest.contract_metadata.identity_graph = identity_graph;

    let namespace_claims = BTreeSet::from([vec!["std".to_string(), "helpers".to_string()]]);
    Ok(ProviderPlan::new(
        LibraryManifestIndex::default(),
        vec![ProviderRecord {
            identity: ProviderIdentity {
                name: package_name.to_string(),
                version: "0.5.0".to_string(),
                digest: "sha256:canonical-fixture".to_string(),
                feature_projection: BTreeSet::new(),
            },
            provenance: ProviderProvenance::Sdk {
                sdk_identity: "incan@0.5.0".to_string(),
                component_id: "stdlib-fixture".to_string(),
                inventory_path: None,
            },
            authority: NamespaceAuthority::SdkReserved,
            namespace_claims: namespace_claims.clone(),
            available: true,
            enabled: true,
            manifest: Some(Arc::new(manifest)),
            artifact: None,
            implementation_facets: Vec::new(),
        }],
        namespace_claims,
    )?)
}

#[test]
fn sdk_provider_import_retains_manifest_canonical_identity() -> Result<(), Box<dyn std::error::Error>> {
    let package_name = "incan_stdlib_fixture";
    let module_path = vec!["helpers".to_string()];
    let plan = sdk_provider_plan_for_helpers_module(package_name, "pub def helper() -> int:\n  return 42\n")?;
    let consumer_source = "from std.helpers import helper\n\ndef run() -> int:\n  return helper()\n";
    let consumer_ast = parse_program(consumer_source, "canonical SDK consumer");
    let mut consumer_checker = TypeChecker::new();
    consumer_checker.set_current_module_path(Some(vec!["consumer".to_string()]));
    consumer_checker.set_provider_plan(Arc::new(plan));
    let seeded = consumer_checker
        .dependency_direct_member_identities
        .get("std.helpers")
        .and_then(|identities| identities.get("helper"))
        .ok_or("SDK provider seeding must retain the manifest identity")?;
    assert_eq!(
        seeded.origin,
        SymbolOrigin::Package {
            library: package_name.to_string(),
            module_path: module_path.clone(),
        }
    );
    consumer_checker
        .check_program(&consumer_ast)
        .map_err(|errors| format!("canonical SDK consumer should typecheck: {errors:?}"))?;

    let imported = consumer_checker
        .type_info()
        .resolved_import_identity("helper")
        .ok_or("SDK import must retain its manifest canonical identity")?;
    assert_eq!(
        imported.origin,
        SymbolOrigin::Package {
            library: package_name.to_string(),
            module_path: module_path.clone(),
        }
    );
    let call_start = consumer_source.rfind("helper").ok_or("missing helper call")?;
    let called = consumer_checker
        .type_info()
        .resolved_identity(Span::new(call_start, call_start + "helper".len()))
        .ok_or("SDK call must retain its manifest canonical identity")?;
    assert_eq!(called, imported);
    Ok(())
}

/// A source facade republishing a compiled SDK function binds the provider's declaration, identity included.
///
/// Issue #1435: the facade hop used to resolve `std.*` through the source stdlib cache while a direct import went
/// through the provider registry, so the consumer's binding carried no identity and lowering could never reach
/// the compiled signature that owns the omitted default.
#[test]
fn sdk_provider_facade_reexport_retains_manifest_canonical_identity_issue1435() -> Result<(), Box<dyn std::error::Error>>
{
    let package_name = "incan_stdlib_fixture";
    let plan = sdk_provider_plan_for_helpers_module(
        package_name,
        "pub def helper(pretty: bool = false) -> int:\n  return 42\n",
    )?;
    let facade_ast = parse_program("pub from std.helpers import helper\n", "SDK facade");
    let consumer_source = "from codec import helper\n\ndef run() -> int:\n  return helper()\n";
    let consumer_ast = parse_program(consumer_source, "SDK facade consumer");
    let mut consumer_checker = TypeChecker::new();
    consumer_checker.set_current_module_path(Some(vec!["consumer".to_string()]));
    consumer_checker.register_dependency_module_path_segments("codec", vec!["codec".to_string()]);
    consumer_checker.set_provider_plan(Arc::new(plan));
    consumer_checker
        .check_with_imports(&consumer_ast, &[("codec", &facade_ast)])
        .map_err(|errors| format!("SDK facade consumer should typecheck: {errors:?}"))?;

    let expected_origin = SymbolOrigin::Package {
        library: package_name.to_string(),
        module_path: vec!["helpers".to_string()],
    };
    let imported = consumer_checker
        .type_info()
        .resolved_import_identity("helper")
        .ok_or("facade import of an SDK function must retain the provider's canonical identity")?;
    assert_eq!(imported.origin, expected_origin);
    assert_eq!(imported.declaration_name, "helper");
    let call_start = consumer_source.rfind("helper").ok_or("missing helper call")?;
    let called = consumer_checker
        .type_info()
        .resolved_identity(Span::new(call_start, call_start + "helper".len()))
        .ok_or("facade-bound SDK call must retain the provider's canonical identity")?;
    assert_eq!(called, imported);
    assert_eq!(
        consumer_checker.import_binding_path("helper"),
        Some(["codec".to_string(), "helper".to_string()].as_slice()),
        "the checked binding path keeps naming the facade the consumer imported from"
    );
    Ok(())
}

#[test]
fn disabled_sdk_component_import_has_a_component_selection_remedy() -> Result<(), Box<dyn std::error::Error>> {
    let ast = parse_program("import std.web\n", "disabled SDK provider import");
    let mut checker = TypeChecker::new();
    checker.set_provider_plan(provider_plan_for_sdk_module(&["std", "web"], false, true)?);
    let result = checker.check_program(&ast);
    assert!(result.is_err(), "a disabled SDK provider import must fail");
    let errors = result.err().unwrap_or_default();
    assert!(
        errors.iter().any(|error| {
            error.message.contains("component `stdlib-web`") && error.message.contains("disabled for this project")
        }),
        "expected disabled-component diagnostic, got: {errors:?}"
    );
    Ok(())
}

#[test]
fn unavailable_sdk_component_import_has_an_installation_remedy() -> Result<(), Box<dyn std::error::Error>> {
    let ast = parse_program("import std.web\n", "unavailable SDK provider import");
    let mut checker = TypeChecker::new();
    checker.set_provider_plan(provider_plan_for_sdk_module(&["std", "web"], true, false)?);
    let result = checker.check_program(&ast);
    assert!(result.is_err(), "an unavailable SDK provider import must fail");
    let errors = result.err().unwrap_or_default();
    assert!(
        errors
            .iter()
            .any(|error| { error.message.contains("component `stdlib-web`") && error.message.contains("unavailable") }),
        "expected unavailable-component diagnostic, got: {errors:?}"
    );
    Ok(())
}

#[test]
fn sdk_provider_catalog_accepts_modules_without_a_compiler_registry_entry() -> Result<(), Box<dyn std::error::Error>> {
    let ast = parse_program("import std.future\n", "provider-owned future SDK module");
    let mut checker = TypeChecker::new();
    checker.set_provider_plan(provider_plan_for_sdk_module(&["std", "future"], true, true)?);
    let result = checker.check_program(&ast);
    assert!(
        result.is_ok(),
        "provider-owned modules should not require a compiler registry entry: {result:?}"
    );
    Ok(())
}

#[test]
fn sdk_provider_catalog_materializes_submodule_imports_without_a_compiler_registry_entry()
-> Result<(), Box<dyn std::error::Error>> {
    let ast = parse_program("from std.future import tools\n", "provider-owned future SDK submodule");
    let mut checker = TypeChecker::new();
    checker.set_provider_plan(provider_plan_for_sdk_modules(
        &[&["std", "future"], &["std", "future", "tools"]],
        true,
        true,
    )?);
    let result = checker.check_program(&ast);
    assert!(
        result.is_ok(),
        "provider-owned submodules should not require a compiler registry entry: {result:?}"
    );
    Ok(())
}

#[test]
fn sdk_provider_bootstrap_preserves_source_stdlib_model_constructors() -> Result<(), String> {
    let ast = parse_program(
        r#"from std.io import IoError

def make_error() -> IoError:
    return IoError(kind="other", detail="failed", operation="test", position=-1, path=None)
"#,
        "source SDK provider constructor import",
    );
    let plan = ProviderPlan::for_in_memory_sdk_modules(
        LibraryManifestIndex::default(),
        [vec!["traits".to_string(), "error".to_string()]],
    )
    .with_bootstrap_sdk_namespace_roots(["io".to_string()]);
    let mut checker = TypeChecker::new();
    checker.set_provider_plan(Arc::new(plan));
    let result = checker.check_program(&ast);
    assert!(
        matches!(checker.lookup_type_info("IoError"), Some(TypeInfo::Model(_))),
        "source SDK provider imports must preserve concrete model metadata: {:?}",
        checker.lookup_symbol("IoError")
    );
    result.map_err(|errors| format!("source SDK provider constructors should typecheck: {errors:?}"))
}

#[test]
fn sdk_provider_catalog_does_not_fall_back_to_the_compiler_stdlib_inventory() -> Result<(), Box<dyn std::error::Error>>
{
    let ast = parse_program("import std.web\n", "module absent from active SDK catalog");
    let mut checker = TypeChecker::new();
    checker.set_provider_plan(provider_plan_for_sdk_module(&["std", "future"], true, true)?);
    let result = checker.check_program(&ast);
    assert!(result.is_err(), "a module absent from the active SDK catalog must fail");
    let errors = result.err().unwrap_or_default();
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("Unknown stdlib module `std.web`")),
        "expected provider-catalog unknown-module diagnostic, got: {errors:?}"
    );
    Ok(())
}

#[test]
fn test_std_testing_marker_runtime_call_is_rejected() {
    let source = r#"
from std.testing import skip

def main() -> None:
    skip("not as runtime call")
"#;
    let Err(errs) = check_str(source) else {
        panic!("runtime call to std.testing marker should fail");
    };
    assert!(
        errs.iter().any(|e| e.message.contains("cannot be called at runtime")),
        "Expected marker runtime-call diagnostic; got: {:?}",
        errs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_inline_module_resolves_module_local_testing_marker_imports() {
    let source = r#"
module tests:
  from std.testing import fixture, test

  @fixture(autouse=true)
  def seed() -> int:
    return 40

  @test
  def decorated(seed: int) -> None:
    assert seed == 40
"#;
    assert_check_ok(source);
}

#[test]
fn test_testing_decorator_alias_uses_checked_import_binding_in_either_source_order() -> Result<(), String> {
    let sources = [
        r#"
resource = alias fixture
from std.testing import fixture

@resource
def database() -> int:
  return 1
"#,
        r#"
from std.testing import fixture
resource = alias fixture

@resource
def database() -> int:
  return 1
"#,
    ];

    for source in sources {
        let tokens = lexer::lex(source).map_err(|errors| format!("lex failed: {errors:?}"))?;
        let ast = parser::parse(&tokens).map_err(|errors| format!("parse failed: {errors:?}"))?;
        let mut checker = TypeChecker::new();
        checker
            .check_program(&ast)
            .map_err(|errors| format!("typecheck failed: {errors:?}"))?;
        assert_eq!(
            checker.type_info().import_binding_path("resource"),
            Some(["std".to_string(), "testing".to_string(), "fixture".to_string()].as_slice())
        );
        if checker.type_info().testing_fixture("database").is_none() {
            return Err("decorator alias did not record database as a fixture".to_string());
        }
    }

    Ok(())
}

#[test]
fn test_async_fixture_records_frontend_metadata() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
import std.async
from std.testing import fixture

@fixture(scope="module", autouse=true)
async def resource() -> int:
  yield 1
"#;
    let tokens = lexer::lex(source).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let ast = parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let mut checker = TypeChecker::new();
    checker
        .check_program(&ast)
        .map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let info = checker
        .type_info()
        .testing_fixture("resource")
        .ok_or_else(|| std::io::Error::other("expected fixture metadata for resource"))?;
    assert_eq!(info.scope, TestingFixtureScope::Module);
    assert!(info.autouse);
    assert!(info.is_async);
    assert!(info.has_teardown);
    assert!(info.dependencies.is_empty());
    Ok(())
}

#[test]
fn test_async_fixture_records_mixed_fixture_dependencies() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
import std.async
from std.testing import fixture

@fixture
def config() -> int:
  return 1

@fixture(scope="session")
async def service(config: int) -> int:
  yield config
"#;
    let tokens = lexer::lex(source).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let ast = parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let mut checker = TypeChecker::new();
    checker
        .check_program(&ast)
        .map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let info = checker
        .type_info()
        .testing_fixture("service")
        .ok_or_else(|| std::io::Error::other("expected fixture metadata for service"))?;
    assert_eq!(info.scope, TestingFixtureScope::Session);
    assert_eq!(info.dependencies, vec!["config".to_string()]);
    assert!(info.is_async);
    Ok(())
}

#[test]
fn test_async_fixture_requires_exactly_one_yield() {
    let missing = r#"
import std.async
from std.testing import fixture

@fixture
async def resource() -> int:
  return 1
"#;
    let missing_errs = check_str_err(missing, "async fixture without yield should fail");
    assert!(
        missing_errs
            .iter()
            .any(|e| e.message.contains("must contain exactly one top-level `yield value`")),
        "Expected missing-yield async fixture diagnostic; got: {:?}",
        missing_errs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );

    let repeated = r#"
import std.async
from std.testing import fixture

@fixture
async def resource() -> int:
  yield 1
  yield 2
"#;
    let repeated_errs = check_str_err(repeated, "async fixture with repeated yield should fail");
    assert!(
        repeated_errs
            .iter()
            .any(|e| e.message.contains("must use exactly one top-level `yield value`")),
        "Expected repeated-yield async fixture diagnostic; got: {:?}",
        repeated_errs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_async_fixture_rejects_nested_or_empty_yield() {
    let nested = r#"
import std.async
from std.testing import fixture

@fixture
async def resource(flag: bool) -> int:
  if flag:
    yield 1
  return 2
"#;
    let nested_errs = check_str_err(nested, "async fixture with nested yield should fail");
    assert!(
        nested_errs
            .iter()
            .any(|e| e.message.contains("must use exactly one top-level `yield value`")),
        "Expected nested-yield async fixture diagnostic; got: {:?}",
        nested_errs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );

    let empty = r#"
import std.async
from std.testing import fixture

@fixture
async def resource() -> int:
  yield
"#;
    let empty_errs = check_str_err(empty, "async fixture with empty yield should fail");
    assert!(
        empty_errs
            .iter()
            .any(|e| e.message.contains("must yield the fixture value")),
        "Expected empty-yield async fixture diagnostic; got: {:?}",
        empty_errs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_fixture_rejects_per_fixture_timeout_config() {
    let source = r#"
from std.testing import fixture

@fixture(timeout="1s")
def resource() -> int:
  return 1
"#;
    let errs = check_str_err(source, "fixture timeout config should fail");
    assert!(
        errs.iter()
            .any(|e| e.message.contains("cannot declare per-fixture timeout configuration")),
        "Expected fixture-timeout diagnostic; got: {:?}",
        errs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_rfc018_assert_is_some_binding_is_visible_after_assert() {
    let source = r#"
import std.testing

def unwrap_name(value: Option[str]) -> str:
  assert value is Some(name)
  return name
"#;
    assert_check_ok(source);
}

#[test]
fn test_rfc018_assert_is_result_bindings_are_visible_after_assert() {
    let source = r#"
import std.testing

def unwrap_ok(value: Result[int, str]) -> int:
  assert value is Ok(number)
  return number

def unwrap_err(value: Result[int, str]) -> str:
  assert value is Err(message)
  return message
"#;
    assert_check_ok(source);
}

#[test]
fn test_rfc018_assert_is_binding_uses_shared_duplicate_registration() {
    let source = r#"
import std.testing

def unwrap_name(value: Option[str]) -> str:
  let name = "fallback"
  assert value is Some(name)
  return name
"#;
    let errors = check_str_err(source, "an assert pattern cannot silently replace an active local");
    assert!(
        errors
            .iter()
            .any(|error| error.message == "Duplicate definition of 'name'"),
        "expected shared duplicate-binding diagnostic, got: {errors:?}"
    );
}

#[test]
fn test_rfc018_assert_is_none_and_wildcard_patterns_typecheck() {
    let source = r#"
import std.testing

def check_option(value: Option[int], other: Option[str]) -> None:
  assert value is None
  assert other is Some(_)
"#;
    assert_check_ok(source);
}

#[test]
fn test_rfc018_assert_raises_accepts_builtin_error_vocabulary() {
    let source = r#"
def explode() -> None:
  pass

def check() -> None:
  assert explode() raises ValueError, "expected failure"
  assert explode() raises AssertionError
"#;
    assert_check_ok(source);
}

#[test]
fn test_rfc018_assert_raises_rejects_unknown_error_type() {
    let source = r#"
def explode() -> None:
  pass

def check() -> None:
  assert explode() raises MadeUpError
"#;
    let errs = check_str_err(source, "unknown assert raises error type should fail");
    assert!(
        errs.iter().any(|e| e.message.contains("MadeUpError")),
        "Expected unknown error type diagnostic; got: {:?}",
        errs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_rfc018_assert_is_pattern_rejects_wrong_scrutinee_type() {
    let source = r#"
import std.testing

def broken(value: int) -> None:
  assert value is Some(inner)
"#;
    let errs = check_str_err(source, "assert is Some on non-Option should fail");
    assert!(
        errs.iter()
            .any(|e| e.message.contains("Option[_]") && e.message.contains("int")),
        "Expected Option mismatch diagnostic; got: {:?}",
        errs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_rfc018_assert_is_pattern_rejects_nested_and_multiple_bindings() {
    let nested = r#"
import std.testing

def broken(value: Option[Result[int, str]]) -> None:
  assert value is Some(Ok(inner))
"#;
    let nested_errs = check_str_err(nested, "nested assert pattern should fail");
    assert!(
        nested_errs
            .iter()
            .any(|e| e.message.contains("Expected assert `is` pattern")
                || e.message.contains("patterns only support a single identifier or `_`")
                || e.message.contains("patterns require exactly one binding or `_`")),
        "Expected nested-pattern diagnostic; got: {:?}",
        nested_errs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );

    let multiple = r#"
import std.testing

def broken(value: Option[int]) -> None:
  assert value is Some(left, right)
"#;
    let multiple_errs = check_str_err(multiple, "multiple assert bindings should fail");
    assert!(
        multiple_errs
            .iter()
            .any(|e| e.message.contains("Expected assert `is` pattern")
                || e.message.contains("patterns only support a single identifier or `_`")
                || e.message.contains("patterns require exactly one binding or `_`")),
        "Expected multiple-binding diagnostic; got: {:?}",
        multiple_errs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_std_environ_args_returns_result_of_list_of_str_issue1668() {
    let source = r#"
from std.environ import EnvironError, args

def subcommand() -> Result[str, EnvironError]:
    arguments: list[str] = args()?
    if len(arguments) < 2:
        return Ok("help")
    return Ok(arguments[1])

def main() -> Result[None, EnvironError]:
    for argument in args()?[1:]:
        println(argument)
    match args():
        Ok(arguments) => println(len(arguments))
        Err(error) => println(f"{error.kind_name()}:{error.key}")
    return Ok(None)
"#;
    assert_check_ok(source);
}

#[test]
fn test_std_environ_args_rejects_arguments_and_non_list_bindings_issue1668() {
    let arity_errors = check_str_err(
        r#"
from std.environ import args

def main() -> None:
    arguments = args("extra")
"#,
        "args() takes no arguments",
    );
    assert!(
        !arity_errors.is_empty(),
        "expected an arity diagnostic for args(\"extra\"), got none"
    );

    let type_errors = check_str_err(
        r#"
from std.environ import args

def main() -> None:
    arguments: list[str] = args()
"#,
        "args() returns Result[list[str], EnvironError], not list[str]",
    );
    assert!(
        type_errors.iter().any(|error| error
            .message
            .contains("expected 'List[str]', found 'Result[List[str], EnvironError]'")),
        "expected a Result mismatch diagnostic, got: {:?}",
        type_errors.iter().map(|error| &error.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_std_environ_get_as_accepts_required_primitive_targets() {
    let source = r#"
from std.environ import EnvironError, get_as

def read_values() -> None:
  text: Result[Option[str], EnvironError] = get_as[str]("TEXT")
  flag: Result[Option[bool], EnvironError] = get_as[bool]("FLAG")
  integer: Result[Option[int], EnvironError] = get_as[int]("INTEGER")
  floating: Result[Option[float], EnvironError] = get_as[float]("FLOATING")
  i8_value: Result[Option[i8], EnvironError] = get_as[i8]("I8")
  i16_value: Result[Option[i16], EnvironError] = get_as[i16]("I16")
  i32_value: Result[Option[i32], EnvironError] = get_as[i32]("I32")
  i64_value: Result[Option[i64], EnvironError] = get_as[i64]("I64")
  i128_value: Result[Option[i128], EnvironError] = get_as[i128]("I128")
  isize_value: Result[Option[isize], EnvironError] = get_as[isize]("ISIZE")
  u8_value: Result[Option[u8], EnvironError] = get_as[u8]("U8")
  u16_value: Result[Option[u16], EnvironError] = get_as[u16]("U16")
  u32_value: Result[Option[u32], EnvironError] = get_as[u32]("U32")
  u64_value: Result[Option[u64], EnvironError] = get_as[u64]("U64")
  u128_value: Result[Option[u128], EnvironError] = get_as[u128]("U128")
  usize_value: Result[Option[usize], EnvironError] = get_as[usize]("USIZE")
  f32_value: Result[Option[f32], EnvironError] = get_as[f32]("F32")
  f64_value: Result[Option[f64], EnvironError] = get_as[f64]("F64")
"#;
    assert_check_ok(source);
}

#[test]
fn test_std_environ_get_as_accepts_validated_newtype_target() {
    let source = r#"
from std.environ import EnvironError, get_as

type Port = newtype int:
  def from_underlying(value: int) -> Result[Self, ValidationError]:
    if value < 1 or value > 65535:
      return Err(ValidationError("port out of range"))
    return Ok(Port(value))

def read_port() -> Result[Option[Port], EnvironError]:
  return get_as[Port]("PORT")
"#;
    assert_check_ok(source);
}

#[test]
fn test_std_environ_get_as_accepts_generic_newtype_when_concrete_underlying_is_supported() {
    let source = r#"
from std.environ import EnvironError, get_as

type Boxed[T] = newtype T

def read_boxed() -> Result[Option[Boxed[int]], EnvironError]:
  return get_as[Boxed[int]]("BOXED")
"#;
    assert_check_ok(source);
}

#[test]
fn test_std_environ_get_as_rejects_generic_newtype_when_concrete_underlying_is_unsupported() {
    let source = r#"
from std.environ import get_as

type Boxed[T] = newtype T

def read_boxed() -> None:
  get_as[Boxed[bytes]]("BOXED")
"#;
    let errors = check_str_err(source, "unsupported generic newtype target should fail typechecking");
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("Boxed[bytes]") && error.message.contains("TryFrom[str]")),
        "expected a concrete generic target diagnostic, got: {:?}",
        errors.iter().map(|error| &error.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_std_environ_get_as_rejects_newtype_over_rust_backed_value() {
    let source = r#"
from rust::std::path import PathBuf
from std.environ import get_as

type WrappedPath = newtype PathBuf

def read_path() -> None:
  get_as[WrappedPath]("PATH")
"#;
    let errors = check_str_err(source, "rust-backed newtype target should fail typechecking");
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("WrappedPath") && error.message.contains("TryFrom[str]")),
        "expected a Rust-backed target diagnostic, got: {:?}",
        errors.iter().map(|error| &error.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_std_environ_get_as_accepts_positional_and_keyword_defaults() {
    let source = r#"
from std.environ import EnvironError, get_as

type Port = newtype int:
  def from_underlying(value: int) -> Result[Self, ValidationError]:
    if value < 1 or value > 65535:
      return Err(ValidationError("port out of range"))
    return Ok(Port(value))

def read_values() -> None:
  positional: Result[int, EnvironError] = get_as[int]("PORT", 8080)
  keyword: Result[int, EnvironError] = get_as[int]("PORT", default=8080)
  newtype_positional: Result[Port, EnvironError] = get_as[Port]("PORT", 8080)
  newtype_keyword: Result[Port, EnvironError] = get_as[Port]("PORT", default=8080)
"#;
    assert_check_ok(source);
}

#[test]
fn test_std_environ_module_qualified_get_as_preserves_overloads() {
    let source = r#"
import std.environ as environ
from std.environ import EnvironError

def read_values() -> None:
  optional: Result[Option[int], EnvironError] = environ.get_as[int]("PORT")
  positional: Result[int, EnvironError] = environ.get_as[int]("PORT", 8080)
  keyword: Result[int, EnvironError] = environ.get_as[int]("PORT", default=8080)
"#;
    assert_check_ok(source);
}

#[test]
fn test_std_environ_get_as_rejects_unsupported_target_during_typechecking() {
    let source = r#"
from std.environ import get_as

model Config:
  name: str

def read_config() -> None:
  get_as[Config]("CONFIG")
"#;
    let errors = check_str_err(source, "unsupported std.environ target should fail typechecking");
    assert!(
        errors.iter().any(|error| {
            error.message.contains("Config")
                && error.message.contains("generic bound")
                && error.message.contains("TryFrom[str]")
        }),
        "expected a target-specific TryFrom[str] diagnostic, got: {:?}",
        errors.iter().map(|error| &error.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_std_environ_get_as_does_not_accept_try_from_int_adoption() {
    let source = r#"
from std.environ import get_as
from std.traits.convert import TryFrom

model NumericToken with TryFrom[int]:
  value: int

  @classmethod
  def try_from(cls, value: int) -> Result[Self, str]:
    return Ok(NumericToken(value=value))

def read_token() -> None:
  get_as[NumericToken]("TOKEN")
"#;
    let errors = check_str_err(source, "TryFrom[int] must not satisfy TryFrom[str]");
    assert!(
        errors.iter().any(|error| {
            error.message.contains("NumericToken")
                && error.message.contains("generic bound")
                && error.message.contains("TryFrom[str]")
        }),
        "expected exact TryFrom[str] bound rejection, got: {:?}",
        errors.iter().map(|error| &error.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_std_environ_get_as_rejects_unrelated_local_try_from_trait() {
    let source = r#"
from std.environ import get_as

trait TryFrom[T]:
  @classmethod
  def try_from(cls, value: T) -> Result[Self, str]: ...

model LocalToken with TryFrom[str]:
  value: str

  @classmethod
  def try_from(cls, value: str) -> Result[Self, str]:
    return Ok(LocalToken(value=value))

def read_token() -> None:
  get_as[LocalToken]("TOKEN")
"#;
    let errors = check_str_err(
        source,
        "an unrelated local TryFrom trait must not satisfy std conversion",
    );
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("LocalToken") && error.message.contains("TryFrom[str]")),
        "expected canonical trait identity rejection, got: {:?}",
        errors.iter().map(|error| &error.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_std_environ_get_as_rejects_unrelated_local_try_from_on_active_generic() {
    let source = r#"
from std.environ import EnvironError, get_as

trait TryFrom[T]:
  @classmethod
  def try_from(cls, value: T) -> Result[Self, str]: ...

def read_token[T with TryFrom[str]]() -> Result[Option[T], EnvironError]:
  return get_as[T]("TOKEN")
"#;
    let errors = check_str_err(
        source,
        "an unrelated active generic TryFrom bound must not satisfy std conversion",
    );
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("T") && error.message.contains("TryFrom[str]")),
        "expected canonical active-bound rejection, got: {:?}",
        errors.iter().map(|error| &error.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_std_environ_get_as_accepts_generic_subtrait_adopter() {
    let source = r#"
from std.environ import EnvironError, get_as
from std.traits.convert import TryFrom

trait EnvReadable[T] with TryFrom[T]:
  def source_name(self) -> str: ...

model Token with EnvReadable[str]:
  value: str

  @classmethod
  def try_from(cls, value: str) -> Result[Self, str]:
    return Ok(Token(value=value))

  def source_name(self) -> str:
    return "environment"

def read_token() -> Result[Option[Token], EnvironError]:
  return get_as[Token]("TOKEN")
"#;
    assert_check_ok(source);
}

#[test]
fn test_std_environ_calls_are_rejected_in_const_initializers() {
    let source = r#"
from std.environ import get

const TOKEN = get("API_TOKEN")
"#;
    let errors = check_str_err(source, "std.environ reads must remain runtime-only");
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("not allowed") || error.message.contains("const initializers")),
        "expected const-context rejection, got: {:?}",
        errors.iter().map(|error| &error.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_std_json_value_indexing_typechecks_as_optional_json_value() {
    let source = r#"
from std.json import JsonValue

def read_object(data: JsonValue) -> Option[JsonValue]:
  return data["name"]

def read_array(data: JsonValue) -> Option[JsonValue]:
  return data[0]
"#;
    assert_check_ok(source);
}

#[test]
fn test_std_json_value_indexing_records_trait_dispatch() -> Result<(), Vec<CompileError>> {
    let source = r#"
from std.json import JsonValue

def read_object(data: JsonValue) -> Option[JsonValue]:
  return data["name"]
"#;
    let tokens = lexer::lex(source)?;
    let ast = parser::parse(&tokens)?;
    let mut checker = TypeChecker::new();
    checker.check_program(&ast)?;

    assert!(
        checker
            .type_info()
            .calls
            .resolved_method_calls
            .values()
            .any(|call| match &call.dispatch {
                ResolvedMethodDispatch::Trait {
                    trait_name,
                    module_path,
                    ..
                } =>
                    trait_name == "Index"
                        && module_path.as_deref()
                            == Some(&["std".to_string(), "traits".to_string(), "indexing".to_string()]),
            }),
        "expected JsonValue indexing to preserve std.traits.indexing.Index dispatch, got {:?}",
        checker.type_info().calls.resolved_method_calls
    );
    assert!(
        checker
            .type_info()
            .calls
            .call_site_callable_params
            .values()
            .any(|params| params.len() == 1 && params[0].ty == ResolvedType::Str),
        "expected JsonValue indexing to preserve the selected string parameter, got {:?}",
        checker.type_info().calls.call_site_callable_params
    );
    Ok(())
}

#[test]
fn test_std_json_value_indexing_rejects_unsupported_key_type() {
    let source = r#"
from std.json import JsonValue

def read_bad(data: JsonValue) -> Option[JsonValue]:
  return data[True]
"#;
    let errs = check_str_err(source, "JsonValue indexing should reject bool keys");
    assert!(
        errs.iter()
            .any(|err| err.message.contains("JsonValue indices must be int or str")),
        "Expected JsonValue index-key diagnostic; got: {errs:?}"
    );
}
