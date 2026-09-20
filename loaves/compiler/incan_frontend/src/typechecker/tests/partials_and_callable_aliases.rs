//! Partial application presets (function, method and trait partials), top-level and method callable aliases, and the
//! alias metadata that public exports publish.

use super::*;

#[test]
fn test_partial_function_presets_project_as_defaults() {
    let source = r#"
def route(method: str, path: str, content_type: str = "text") -> str:
  return method

get = partial route(method="GET")

def use() -> str:
  a = get(path="/health")
  b = get(method="POST", path="/submit")
  return b
"#;
    let ast = parse_program(source, "partial function defaults");
    let mut checker = TypeChecker::new();
    checker
        .check_program(&ast)
        .unwrap_or_else(|errs| panic!("typecheck failed: {errs:?}"));
    let sym = checker
        .lookup_symbol("get")
        .unwrap_or_else(|| panic!("missing projected partial symbol"));
    let SymbolKind::Function(info) = &sym.kind else {
        panic!("expected function symbol for partial, got {:?}", sym.kind);
    };
    let method = info.params.iter().find(|param| param.name() == Some("method")).unwrap();
    let path = info.params.iter().find(|param| param.name() == Some("path")).unwrap();
    let content_type = info
        .params
        .iter()
        .find(|param| param.name() == Some("content_type"))
        .unwrap();
    assert!(method.has_default, "{info:?}");
    assert!(!path.has_default, "{info:?}");
    assert!(content_type.has_default, "{info:?}");
}

#[test]
fn test_local_partial_expression_preserves_overrideable_preset_defaults() -> Result<(), String> {
    let source = r#"
def add3(a: int, b: int, c: int = 3) -> int:
  return a + b + c

def use() -> int:
  p = partial add3(a=1)
  positional = p(9, 2)
  named = p(b=9, c=2)
  override = p(a=7, b=9, c=2)
  defaulted = p(9)
  return positional + named + override + defaulted
"#;

    check_str(source).map_err(|errors| format!("local partial preset defaults should typecheck: {errors:?}"))
}

#[test]
fn test_local_partial_expression_rejects_invalid_residual_arity() {
    let too_few = check_str_err(
        r#"
def add3(a: int, b: int, c: int) -> int:
  return a + b + c

def use() -> int:
  p = partial add3(a=1)
  return p(9)
"#,
        "a local partial call missing residual c must be rejected",
    );
    assert!(
        too_few
            .iter()
            .any(|error| error.message.contains("Missing required argument 'c'")),
        "missing residual c should be diagnosed: {too_few:?}"
    );
    assert!(
        !too_few
            .iter()
            .any(|error| error.message.contains("Missing required argument 'b'")),
        "the preset a must not leave a phantom positional slot: {too_few:?}"
    );

    let too_many = check_str_err(
        r#"
def add3(a: int, b: int, c: int) -> int:
  return a + b + c

def use() -> int:
  p = partial add3(a=1)
  return p(9, 2, 3)
"#,
        "a local partial call with a third residual argument must be rejected",
    );
    assert!(
        too_many
            .iter()
            .any(|error| error.message.contains("expects 2 argument(s), got 3")),
        "residual arity should be two after presetting a: {too_many:?}"
    );
}

#[test]
fn test_local_partial_expression_refuses_partial_of_stored_callable() {
    let errors = check_str_err(
        r#"
def add3(a: int, b: int, c: int) -> int:
  return a + b + c

def use() -> int:
  p = partial add3(a=1)
  q = partial p(b=2)
  return q(c=3)
"#,
        "partial application of a stored callable must fail before lowering",
    );
    assert!(
        errors.iter().any(|error| error
            .message
            .contains("Partial application of a locally stored callable is not supported")),
        "unsupported local partial composition should have an intentional source diagnostic: {errors:?}"
    );
}

#[test]
fn test_public_partial_exports_projected_defaults() {
    let source = r#"
pub def route(method: str, path: str, content_type: str = "text") -> str:
  return method

pub get = partial route(method="GET")
"#;
    let ast = parse_program(source, "partial public export");
    let mut checker = TypeChecker::new();
    checker
        .check_program(&ast)
        .unwrap_or_else(|errs| panic!("typecheck failed: {errs:?}"));

    let exports = collect_checked_public_exports(&ast, &checker);
    let get = exports
        .iter()
        .find_map(|export| match &export.kind {
            CheckedExportKind::Partial(partial) if partial.name == "get" => Some(partial),
            _ => None,
        })
        .unwrap_or_else(|| panic!("missing public partial export: {exports:?}"));
    assert_eq!(get.target_path, vec!["route"]);
    assert_eq!(get.target_kind, CheckedPartialTargetKind::Function);
    assert_eq!(get.presets[0].name, "method");
    assert_eq!(get.presets[0].value, CheckedPresetValue::String("GET".to_string()));
    let method = get.params.iter().find(|param| param.name() == Some("method")).unwrap();
    let path = get.params.iter().find(|param| param.name() == Some("path")).unwrap();
    let content_type = get
        .params
        .iter()
        .find(|param| param.name() == Some("content_type"))
        .unwrap();
    assert!(method.has_default, "{get:?}");
    assert!(!path.has_default, "{get:?}");
    assert!(content_type.has_default, "{get:?}");

    let manifest = LibraryManifest::from_checked_exports("routes".to_string(), "0.1.0".to_string(), &exports);
    assert_eq!(manifest.exports.partials.len(), 1);
    assert_eq!(
        manifest.exports.partials[0].target_kind,
        PartialTargetKindExport::Function
    );
    assert_eq!(
        manifest.exports.partials[0].presets[0].value,
        PresetValueExport::String("GET".to_string())
    );
}

#[test]
fn test_public_partial_exports_declaration_safe_preset_values() {
    let source = r#"
pub model Profile:
  pub name: str

pub def configure(headers: dict[str, str], codes: list[int], profile: Profile) -> str:
  return profile.name

pub default_config = partial configure(headers={"accept": "json"}, codes=[200], profile=Profile(name="ops"))
"#;
    let ast = parse_program(source, "partial public preset metadata");
    let mut checker = TypeChecker::new();
    checker
        .check_program(&ast)
        .unwrap_or_else(|errs| panic!("typecheck failed: {errs:?}"));

    let exports = collect_checked_public_exports(&ast, &checker);
    let default_config = exports
        .iter()
        .find_map(|export| match &export.kind {
            CheckedExportKind::Partial(partial) if partial.name == "default_config" => Some(partial),
            _ => None,
        })
        .unwrap_or_else(|| panic!("missing partial export: {exports:?}"));

    assert!(
        default_config
            .presets
            .iter()
            .any(|preset| matches!(preset.value, CheckedPresetValue::Dict(_))),
        "{default_config:?}"
    );
    assert!(
        default_config
            .presets
            .iter()
            .any(|preset| matches!(preset.value, CheckedPresetValue::List(_))),
        "{default_config:?}"
    );
    assert!(
        default_config
            .presets
            .iter()
            .any(|preset| matches!(preset.value, CheckedPresetValue::ModelLiteral { .. })),
        "{default_config:?}"
    );
}

#[test]
fn test_source_imported_partial_records_projection_metadata() {
    let provider = parse_program(
        r#"
pub model Policy:
  pub family: FrozenStr
  pub role: FrozenStr
  pub enabled: bool

pub policy = partial Policy(family="core", enabled=true)
"#,
        "imported partial projection provider",
    );
    let consumer = parse_program(
        r#"
from provider import Policy, policy

const DEFAULT_POLICY: Policy = policy(role="admin")

def runtime_policy_enabled() -> bool:
  return policy(role="runtime").enabled
"#,
        "imported partial projection consumer",
    );
    let mut checker = TypeChecker::new();
    checker
        .check_with_imports(&consumer, &[("provider", &provider)])
        .unwrap_or_else(|errs| panic!("imported partial projection should typecheck: {errs:?}"));
    assert!(
        checker.type_info.partial_projection("policy").is_some(),
        "imported partial should preserve projection metadata"
    );
    let policy_symbol = checker
        .lookup_symbol("policy")
        .unwrap_or_else(|| panic!("imported partial symbol should exist"));
    let SymbolKind::Function(policy_info) = &policy_symbol.kind else {
        panic!("imported partial should be a function, got {:?}", policy_symbol.kind);
    };
    let param_names = policy_info
        .params
        .iter()
        .map(|param| param.name.as_deref().unwrap_or("<anonymous>"))
        .collect::<Vec<_>>();
    assert_eq!(param_names, ["family", "role", "enabled"], "{:?}", policy_info.params);
    assert!(
        matches!(
            policy_info.params.as_slice(),
            [
                CallableParam {
                    ty: ResolvedType::FrozenStr,
                    ..
                },
                CallableParam {
                    ty: ResolvedType::FrozenStr,
                    ..
                },
                CallableParam {
                    ty: ResolvedType::Bool,
                    ..
                }
            ]
        ),
        "expected imported partial params to preserve FrozenStr, got {:?}",
        policy_info.params
    );
    let call_params = checker
        .type_info
        .calls
        .call_site_callable_params
        .values()
        .find(|params| {
            params
                .iter()
                .filter_map(|param| param.name.as_deref())
                .collect::<Vec<_>>()
                == ["family", "role", "enabled"]
        })
        .unwrap_or_else(|| {
            panic!(
                "runtime partial call should record source-ordered call-site params, got {:?}",
                checker.type_info.calls.call_site_callable_params
            )
        });
    assert!(
        matches!(
            call_params.as_slice(),
            [
                CallableParam {
                    ty: ResolvedType::FrozenStr,
                    ..
                },
                CallableParam {
                    ty: ResolvedType::FrozenStr,
                    ..
                },
                CallableParam {
                    ty: ResolvedType::Bool,
                    ..
                }
            ]
        ),
        "expected runtime partial call params to preserve FrozenStr, got {call_params:?}"
    );
}

#[test]
fn test_top_level_partial_rejects_runtime_preset_values() {
    let source = r#"
def default_method() -> str:
  return "GET"

def route(method: str, path: str) -> str:
  return method

get = partial route(method=default_method())
"#;
    let errors = check_str_err(source, "top-level partial runtime preset should fail");
    assert!(
        errors.iter().any(|error| error
            .message
            .contains("Top-level partial preset 'method' must be declaration-safe")),
        "expected declaration-safe preset diagnostic, got {errors:?}"
    );
}

#[test]
fn test_top_level_partial_invalid_diagnostics_are_complete() {
    for (source, expected, context) in [
        (
            r#"
def route(method: str) -> str:
  return method

noop = partial route()
"#,
            "must preset at least one keyword",
            "empty partial should be rejected",
        ),
        (
            r#"
def route(method: str) -> str:
  return method

get = partial route(method="GET", method="POST")
"#,
            "repeats preset keyword 'method'",
            "duplicate partial preset should be rejected",
        ),
        (
            r#"
def route(method: str) -> str:
  return method

get = partial route(verb="GET")
"#,
            "presets unknown parameter 'verb'",
            "unknown partial preset should be rejected",
        ),
        (
            r#"
const method = "GET"
get = partial method(value="GET")
"#,
            "targets unsupported symbol 'method'",
            "unsupported partial target should be rejected",
        ),
        (
            r#"
static method: str = "GET"
get = partial method(value="GET")
"#,
            "targets unsupported symbol 'method'",
            "unsupported static partial target should be rejected",
        ),
        (
            r#"
trait Labelled:
  def label(self) -> str: ...

get = partial Labelled(value="GET")
"#,
            "targets unsupported symbol 'Labelled'",
            "unsupported trait partial target should be rejected",
        ),
        (
            r#"
enum Method:
  Get

get = partial Get(value="GET")
"#,
            "targets unsupported symbol 'Get'",
            "unsupported enum variant partial target should be rejected",
        ),
        (
            r#"
get = partial route(method="GET")
"#,
            "targets unknown callable 'route'",
            "unknown partial target should be rejected",
        ),
        (
            r#"
def route(method: str, **labels: str) -> str:
  return method

get = partial route(labels={"accept": "json"})
"#,
            "cannot target callable 'route' because parameter 'labels' is a rest parameter",
            "rest keyword partial target should be rejected",
        ),
        (
            r#"
def route(method: str, *segments: str) -> str:
  return method

get = partial route(method="GET")
"#,
            "cannot target callable 'route' because parameter 'segments' is a rest parameter",
            "rest positional partial target should be rejected even when preset fills a normal parameter",
        ),
    ] {
        let errors = check_str_err(source, context);
        assert!(
            errors.iter().any(|error| error.message.contains(expected)),
            "expected diagnostic containing `{expected}` for {context}, got {errors:?}"
        );
    }
}

#[test]
fn test_top_level_partial_cycles_are_rejected() {
    for (source, expected) in [
        (
            r#"
get = partial get(method="GET")
"#,
            "Partial cycle detected: get -> get",
        ),
        (
            r#"
get = partial alias_get(method="GET")
alias_get = get
"#,
            "Partial cycle detected: get -> alias_get -> get",
        ),
        (
            r#"
left = partial right(method="GET")
right = partial left(method="POST")
"#,
            "Partial cycle detected",
        ),
    ] {
        let errors = check_str_err(source, "partial cycle should fail");
        assert!(
            errors.iter().any(|error| error.message.contains(expected)),
            "expected partial cycle diagnostic containing `{expected}`, got {errors:?}"
        );
    }
}

#[test]
fn test_public_partial_rejects_private_target_and_private_preset_values() {
    for (source, expected) in [
        (
            r#"
def route(method: str) -> str:
  return method

pub get = partial route(method="GET")
"#,
            "Public partial 'get' targets private symbol 'route'",
        ),
        (
            r#"
const DEFAULT_METHOD = "GET"

pub def route(method: str) -> str:
  return method

pub get = partial route(method=DEFAULT_METHOD)
"#,
            "Public partial 'get' preset 'method' references private symbol 'DEFAULT_METHOD'",
        ),
        (
            r#"
model Profile:
  name: str

pub def configure(profile: Profile) -> str:
  return profile.name

pub default_config = partial configure(profile=Profile(name="ops"))
"#,
            "Public partial 'default_config' preset 'profile' references private symbol 'Profile'",
        ),
    ] {
        let errors = check_str_err(source, "public partial visibility leak should fail");
        assert!(
            errors.iter().any(|error| error.message.contains(expected)),
            "expected visibility diagnostic containing `{expected}`, got {errors:?}"
        );
    }
}

#[test]
fn test_check_with_imports_collects_explicit_public_partial_as_callable() -> Result<(), String> {
    let library = parse_program(
        r#"
pub def route(method: str, path: str) -> str:
  return path

pub get = partial route(method="GET")
"#,
        "partial import library",
    );
    let consumer = parse_program(
        r#"
from routes import get

def use() -> str:
  return get(path="/health")
"#,
        "partial import consumer",
    );

    let mut checker = TypeChecker::new();
    checker
        .check_with_imports(&consumer, &[("routes", &library)])
        .map_err(|errs| format!("consumer should import public partial callable: {errs:?}"))?;
    Ok(())
}

#[test]
fn test_from_import_accepts_public_partial_export() {
    let library = parse_program(
        r#"
pub model Spec:
  pub namespace: str
  pub policy: str
  pub klass: str
  pub lifecycle: str

pub core_spec = partial Spec(namespace="core", policy="portable")
"#,
        "partial import library",
    );
    let consumer = parse_program(
        r#"
from presets import core_spec

def use() -> str:
  spec = core_spec(klass="scalar", lifecycle="v1")
  return spec.namespace
"#,
        "partial from-import consumer",
    );

    let mut checker = TypeChecker::new();
    checker
        .check_with_imports(&consumer, &[("presets", &library)])
        .unwrap_or_else(|errs| panic!("consumer should import public partial callable by name: {errs:?}"));
}

#[test]
fn test_top_level_alias_preserves_overloaded_type_token_function_set() -> Result<(), String> {
    let source = r#"
model ColumnExpr:
  name: str

model IntColumnExpr:
  source: str

model FloatColumnExpr:
  source: str

def col(name: str) -> ColumnExpr:
  return ColumnExpr(name=name)

def cast(expr: ColumnExpr, target: Type[int]) -> IntColumnExpr:
  return IntColumnExpr(source=expr.name)

def cast(expr: ColumnExpr, target: Type[float]) -> FloatColumnExpr:
  return FloatColumnExpr(source=expr.name)

def cast(expr: ColumnExpr, target: str) -> ColumnExpr:
  return ColumnExpr(name=target)

safe_cast = alias cast

def use() -> None:
  typed: FloatColumnExpr = safe_cast(col("amount"), float)
  fallback: ColumnExpr = safe_cast(col("amount"), "float64")
  return
"#;
    let tokens = lexer::lex(source).map_err(|errs| format!("{errs:?}"))?;
    let ast = parser::parse(&tokens).map_err(|errs| format!("{errs:?}"))?;
    let mut checker = TypeChecker::new();
    checker
        .check_program(&ast)
        .map_err(|errs| format!("overloaded alias should typecheck: {errs:?}"))?;

    let alias = checker
        .lookup_symbol("safe_cast")
        .ok_or_else(|| "expected overloaded alias symbol".to_string())?;
    let SymbolKind::FunctionOverloads(overloads) = &alias.kind else {
        return Err(format!("expected safe_cast overload set, got {:?}", alias.kind));
    };
    assert_eq!(overloads.len(), 3);
    assert_eq!(
        checker
            .type_info()
            .function_overloads("safe_cast")
            .map(|overloads| overloads.len()),
        Some(3)
    );
    Ok(())
}

#[test]
fn test_method_partial_presets_project_as_defaults_for_trait_and_model() {
    let source = r#"
trait Named:
  def label(self, prefix: str, suffix: str = "!") -> str:
    return prefix
  short = partial label(prefix="name")

model User with Named:
  name: str
  def label(self, prefix: str, suffix: str = "!") -> str:
    return prefix
  loud = partial label(prefix="user")

def use(user: User) -> str:
  a = user.loud()
  b = user.loud(prefix="admin")
  c = user.short()
  return b
"#;
    check_str(source).unwrap_or_else(|errs| panic!("typecheck failed: {errs:?}"));
}

#[test]
fn test_method_partial_preset_values_are_typechecked() {
    let source = r#"
trait Named:
  def label(self, prefix: str) -> str:
    return prefix
  short = partial label(prefix=1)
"#;
    let errors = check_str_err(source, "method partial preset should be typechecked");
    let messages: Vec<_> = errors.iter().map(|err| err.message.as_str()).collect();
    assert!(
        messages
            .iter()
            .any(|message| message.contains("Type mismatch") || message.contains("expected str")),
        "expected type mismatch, got {messages:?}"
    );
}

#[test]
fn test_method_partial_name_collisions_are_rejected() {
    for (source, expected) in [
        (
            r#"
trait Named:
  def label(self, prefix: str) -> str:
    return prefix
  label = partial label(prefix="name")
"#,
            "Duplicate method partial 'Named.label'",
        ),
        (
            r#"
trait Named:
  def label(self, prefix: str) -> str:
    return prefix
  short = partial label(prefix="name")
  short = partial label(prefix="user")
"#,
            "Duplicate method partial 'Named.short'",
        ),
        (
            r#"
trait Named:
  def label(self, prefix: str) -> str:
    return prefix
  short = label
  short = partial label(prefix="name")
"#,
            "Duplicate method partial 'Named.short'",
        ),
    ] {
        let errors = check_str_err(source, "method partial collision should fail");
        assert!(
            errors.iter().any(|error| error.message.contains(expected)),
            "expected method partial collision diagnostic containing `{expected}`, got {errors:?}"
        );
    }
}

#[test]
fn test_method_partial_can_target_same_type_method_alias() {
    let source = r#"
trait Named:
  def label(self, prefix: str) -> str:
    return prefix
  labelled = label
  short = partial labelled(prefix="name")

model User with Named:
  def label(self, prefix: str) -> str:
    return prefix

def use(user: User) -> str:
  return user.short()
"#;
    check_str(source).unwrap_or_else(|errs| panic!("method partial targeting alias should typecheck: {errs:?}"));
}

#[test]
fn test_trait_partial_override_conflict_is_rejected() {
    let source = r#"
trait Named:
  def label(self, prefix: str) -> str:
    return prefix
  short = partial label(prefix="name")

model User with Named:
  def label(self, prefix: str) -> str:
    return prefix
  def short(self, prefix: int) -> str:
    return "bad"
"#;
    let errors = check_str_err(source, "trait partial override conflict should fail");
    assert!(
        errors.iter().any(|error| error
            .message
            .contains("Trait 'Named' requires 'User'::short to match its signature")),
        "expected trait partial override conflict, got {errors:?}"
    );
}

#[test]
fn test_inherited_trait_partial_ambiguity_is_rejected() {
    let source = r#"
trait Left:
  def label(self, prefix: str) -> str:
    return prefix
  short = partial label(prefix="left")

trait Right:
  def label(self, prefix: str) -> str:
    return prefix
  short = partial label(prefix="right")

trait Both with Left, Right:
  def both(self) -> str: ...

model User with Both:
  def label(self, prefix: str) -> str:
    return prefix
  def both(self) -> str:
    return "both"
"#;
    let errors = check_str_err(source, "inherited trait partial ambiguity should fail");
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("Ambiguous trait method 'short'")),
        "expected inherited trait partial ambiguity diagnostic, got {errors:?}"
    );
}

#[test]
fn test_subtrait_partial_override_must_match_inherited_partial_signature() {
    let source = r#"
trait Base:
  def label(self, prefix: str) -> str:
    return prefix
  short = partial label(prefix="base")

trait Child with Base:
  def labelled(self, prefix: str, count: int) -> str:
    return prefix
  short = partial labelled(prefix="child")
"#;
    let errors = check_str_err(source, "incompatible subtrait partial override should fail");
    assert!(
        errors.iter().any(|error| error
            .message
            .contains("Trait 'Base' requires 'Child'::short to match its signature")),
        "expected inherited partial override conflict, got {errors:?}"
    );
}

#[test]
fn test_subtrait_partial_can_override_inherited_partial_with_compatible_signature() {
    let source = r#"
trait Base:
  def label(self, prefix: str) -> str:
    return prefix
  short = partial label(prefix="base")

trait Child with Base:
  def label_child(self, prefix: str) -> str:
    return prefix
  short = partial label_child(prefix="child")

model User with Child:
  def label(self, prefix: str) -> str:
    return prefix
  def label_child(self, prefix: str) -> str:
    return prefix
"#;
    check_str(source).unwrap_or_else(|errs| panic!("compatible inherited partial override should typecheck: {errs:?}"));
}

#[test]
fn test_generic_trait_bound_partial_ambiguity_is_rejected() {
    let source = r#"
trait Left:
  def label(self, prefix: str) -> str:
    return prefix
  short = partial label(prefix="left")

trait Right:
  def label(self, prefix: str) -> str:
    return prefix
  short = partial label(prefix="right")

trait Both with Left, Right:
  def both(self) -> str: ...

def use[T with Both](value: T) -> str:
  return value.short()
"#;
    let errors = check_str_err(source, "generic trait-bound partial ambiguity should fail");
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("Ambiguous trait method 'short'")),
        "expected generic trait-bound partial ambiguity diagnostic, got {errors:?}"
    );
}

#[test]
fn top_level_function_alias_typechecks_as_callable() -> Result<(), String> {
    let source = r#"
def avg(x: int) -> int:
  return x

mean = avg

def main() -> int:
  return mean(10)
"#;
    let tokens = lexer::lex(source).map_err(|errs| format!("{errs:?}"))?;
    let ast = parser::parse(&tokens).map_err(|errs| format!("{errs:?}"))?;
    let mut checker = TypeChecker::new();
    checker.check_program(&ast).map_err(|errs| format!("{errs:?}"))?;
    let alias = checker
        .lookup_symbol("mean")
        .ok_or_else(|| "expected alias symbol to be collected".to_string())?;
    assert!(matches!(alias.kind, crate::symbols::SymbolKind::Function(_)));
    Ok(())
}

#[test]
fn qualified_top_level_alias_resolves_imported_module_target() -> Result<(), String> {
    let source = r#"
import std.math as math

def sqrt(value: str) -> str:
  return value

root = math.sqrt

def main() -> float:
  return root(4.0)
"#;
    let tokens = lexer::lex(source).map_err(|errs| format!("{errs:?}"))?;
    let ast = parser::parse(&tokens).map_err(|errs| format!("{errs:?}"))?;
    let mut checker = TypeChecker::new();
    checker.check_program(&ast).map_err(|errs| format!("{errs:?}"))?;
    let alias = checker
        .lookup_symbol("root")
        .ok_or_else(|| "expected qualified alias symbol to be collected".to_string())?;
    let crate::symbols::SymbolKind::Function(info) = &alias.kind else {
        return Err(format!("expected root to resolve as a function, got {:?}", alias.kind));
    };
    assert_eq!(info.return_type, ResolvedType::Float);
    Ok(())
}

#[test]
fn top_level_alias_rejects_non_callable_value_target() {
    let errors = check_str_err(
        r#"
const count = 1
total = count
"#,
        "const alias target should be rejected",
    );
    assert!(
        errors
            .iter()
            .any(|err| err.message.contains("targets unsupported symbol 'count'")),
        "expected unsupported alias target diagnostic, got {errors:?}"
    );
}

#[test]
fn top_level_alias_cycle_is_rejected() {
    let errors = check_str_err(
        r#"
left = right
right = left
"#,
        "alias cycle should be rejected",
    );
    assert!(
        errors.iter().any(|err| err.message.contains("Alias cycle detected")),
        "expected alias cycle diagnostic, got {errors:?}"
    );
}

#[test]
fn public_top_level_alias_rejects_private_target() {
    let errors = check_str_err(
        r#"
pub mean = avg

def avg(x: int) -> int:
  return x
"#,
        "public alias to private target should be rejected",
    );
    assert!(
        errors
            .iter()
            .any(|err| err.message.contains("Public alias 'mean' targets private symbol 'avg'")),
        "expected public/private alias diagnostic, got {errors:?}"
    );
}

#[test]
fn same_type_method_alias_typechecks_as_method_call() -> Result<(), String> {
    let source = r#"
model Stats:
  value: int
  mean = avg

  def avg(self) -> int:
    return self.value

def main() -> int:
  let stats = Stats(value=10)
  return stats.mean()
"#;
    let tokens = lexer::lex(source).map_err(|errs| format!("{errs:?}"))?;
    let ast = parser::parse(&tokens).map_err(|errs| format!("{errs:?}"))?;
    let mut checker = TypeChecker::new();
    checker.check_program(&ast).map_err(|errs| format!("{errs:?}"))?;
    let Some(TypeInfo::Model(model)) = checker.lookup_type_info("Stats") else {
        return Err("expected Stats model metadata".to_string());
    };
    assert_eq!(model.method_aliases.get("mean").map(String::as_str), Some("avg"));
    assert!(model.methods.contains_key("mean"));
    assert_eq!(model.methods["mean"].alias_of.as_deref(), Some("avg"));
    Ok(())
}

#[test]
fn method_alias_rejects_unknown_target() {
    let errors = check_str_err(
        r#"
model Stats:
  value: int
  mean = avg
"#,
        "method alias to missing target should be rejected",
    );
    assert!(
        errors.iter().any(|err| err
            .message
            .contains("Method alias 'Stats.mean' targets unknown method 'avg'")),
        "expected missing method alias target diagnostic, got {errors:?}"
    );
}

#[test]
fn method_alias_cycle_is_rejected() {
    let errors = check_str_err(
        r#"
model Stats:
  value: int
  mean = average
  average = mean
"#,
        "method alias cycle should be rejected",
    );
    assert!(
        errors
            .iter()
            .any(|err| err.message.contains("Method alias cycle detected on 'Stats'")),
        "expected method alias cycle diagnostic, got {errors:?}"
    );
}

#[test]
fn public_top_level_alias_exports_as_alias_metadata() -> Result<(), String> {
    let source = r#"
pub def avg(x: int) -> int:
  return x

pub mean = alias avg
"#;
    let tokens = lexer::lex(source).map_err(|errs| format!("{errs:?}"))?;
    let ast = parser::parse(&tokens).map_err(|errs| format!("{errs:?}"))?;
    let mut checker = TypeChecker::new();
    checker.check_program(&ast).map_err(|errs| format!("{errs:?}"))?;
    let exports = collect_checked_public_exports(&ast, &checker);
    let manifest = LibraryManifest::from_checked_exports("stats".to_string(), "0.1.0".to_string(), &exports);
    assert_eq!(manifest.exports.aliases.len(), 1);
    assert_eq!(manifest.exports.aliases[0].name, "mean");
    assert_eq!(manifest.exports.aliases[0].target_path, vec!["avg"]);
    assert!(
        manifest.exports.aliases[0].projected_function.is_some(),
        "function aliases should carry callable projection metadata for pub:: consumers"
    );
    assert!(
        manifest
            .exports
            .functions
            .iter()
            .all(|function| function.name != "mean"),
        "alias must not be flattened into a duplicate function export"
    );
    Ok(())
}

/// A public alias targeting a function in its own module publishes that function's callable metadata.
///
/// The projection pass keys candidates by resolved declaration path, `["provider", "helper"]`, while an alias whose
/// target lives in its own module records the target as the source writes it, `["helper"]`. The two never met, so
/// every same-module alias published `projected_function: None` and any consumer requiring callable metadata for a
/// canonical callable target refused it.
///
/// The existing coverage did not catch this because it aliases across modules, where the recorded target is already
/// qualified and the lookup happens to line up.
#[test]
fn a_same_module_public_alias_publishes_its_target_callable_metadata() -> Result<(), String> {
    let source = "pub def helper(value: int) -> int:\n  return value + 1\n\npub run = alias helper\n";
    let ast = parse_program(source, "same-module alias provider");
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["provider".to_string()]));
    checker
        .check_program(&ast)
        .map_err(|errors| format!("same-module alias provider should typecheck: {errors:?}"))?;

    let mut api_modules = vec![collect_checked_api_metadata(
        &ast,
        &checker,
        vec!["provider".to_string()],
    )];
    materialize_api_alias_projections(&mut api_modules);

    let alias = api_modules
        .iter()
        .flat_map(|module| module.declarations.iter())
        .find_map(|declaration| match declaration {
            ApiDeclaration::Alias(alias) if alias.name == "run" => Some(alias),
            _ => None,
        })
        .ok_or("the checked API must publish the `run` alias")?;
    let projected = alias
        .projected_function
        .as_ref()
        .ok_or("a same-module alias must publish its target's callable metadata")?;
    assert_eq!(
        projected.callable.name, "run",
        "a projection is published under the alias's own name"
    );
    assert_eq!(
        projected.source_path,
        vec!["provider".to_string(), "helper".to_string()],
        "the projection must point at the declaration the alias targets"
    );
    Ok(())
}
