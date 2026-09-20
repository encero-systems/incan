//! Identity across module boundaries: imported and dependency trait defaults, import aliases and re-exports, stdlib
//! facades, provider bootstrap resolution, dependency interfaces and predeclaration, and dependency aliases that keep
//! their target identity.

use super::*;

/// Imported trait defaults retain their declaration identity for lowering into a local adopter.
#[test]
fn imported_trait_default_method_identity_survives_for_lowering() -> Result<(), String> {
    let checker = check(
        r#"
from std.traits.error import Error

model LocalError with Error:
  detail: str

  def message(self) -> str:
    return self.detail
"#,
        "imported trait default identity",
    )?;

    let identity = checker
        .type_info()
        .traits
        .method_identities
        .get(&(traits::as_str(TraitId::Error).to_string(), "source".to_string()))
        .ok_or_else(|| {
            format!(
                "missing Error.source identity in lowering artifacts: {:?}",
                checker.type_info().traits.method_identities
            )
        })?;
    assert_eq!(identity.declaration_name, "source");
    assert_eq!(identity.kind, SemanticSourceTargetKind::Method);
    assert_eq!(
        identity.origin,
        SymbolOrigin::Module(vec!["std".to_string(), "traits".to_string(), "error".to_string(),])
    );
    Ok(())
}

/// Dependency-only trait metadata uses its canonical module-qualified key when no local symbol carries the trait.
#[test]
fn dependency_trait_default_method_identity_survives_private_module_checking() -> Result<(), String> {
    let provider = parse(
        r#"
pub trait Contract:
  def required(self) -> str: ...

  def fallback(self) -> str:
    return "fallback"
"#,
        "dependency trait provider",
    )?;
    let consumer = parse(
        r#"
from provider import Contract

model Implementation with Contract:
  def required(self) -> str:
    return "implemented"
"#,
        "dependency trait consumer",
    )?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["consumer".to_string()]));
    checker
        .check_with_imports_allow_private(&consumer, &[("provider", &provider)])
        .map_err(|errors| format!("dependency trait consumer should typecheck: {errors:?}"))?;

    let identity = checker
        .type_info()
        .traits
        .method_identities
        .get(&("provider.Contract".to_string(), "fallback".to_string()))
        .ok_or_else(|| {
            format!(
                "missing provider.Contract.fallback identity in lowering artifacts: {:?}",
                checker.type_info().traits.method_identities
            )
        })?;
    assert_eq!(identity.declaration_name, "fallback");
    assert_eq!(identity.kind, SemanticSourceTargetKind::Method);
    assert_eq!(identity.origin, SymbolOrigin::Module(vec!["provider".to_string()]));
    let adopted_identity = checker
        .type_info()
        .traits
        .method_identities
        .get(&("Contract".to_string(), "fallback".to_string()))
        .ok_or_else(|| "missing locally-adopted Contract.fallback identity in lowering artifacts".to_string())?;
    assert_eq!(adopted_identity, identity);
    Ok(())
}

/// An import, its alias, and a re-export are bindings to one declaration: every spelling records one identity.
#[test]
fn import_alias_and_reexport_share_the_declaration_identity() -> Result<(), String> {
    let lib = parse(
        r#"
pub def helper() -> int:
  return 1
"#,
        "identity lib",
    )?;
    let api = parse(
        r#"
pub from lib import helper as h
"#,
        "identity api facade",
    )?;
    let consumer_source = r#"
from lib import helper
from lib import helper as h
from api import h as run

def use_all() -> None:
  a = helper
  b = h
  c = run
"#;
    let consumer = parse(consumer_source, "identity consumer")?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["consumer".to_string()]));
    checker
        .check_with_imports(&consumer, &[("lib", &lib), ("api", &api)])
        .map_err(|errors| format!("identity consumer should typecheck: {errors:?}"))?;
    assert!(
        checker
            .dependency_member_reexports
            .get("api")
            .is_some_and(|exports| exports.contains_key("h")),
        "the facade must record `h` as an actual public re-export"
    );

    let direct = checker
        .type_info()
        .resolved_import_identity("helper")
        .ok_or("direct import must prove an identity")?
        .clone();
    let aliased = checker
        .type_info()
        .resolved_import_identity("h")
        .ok_or("aliased import must prove an identity")?
        .clone();
    let reexported = checker
        .type_info()
        .resolved_import_identity("run")
        .ok_or("re-exported import must prove an identity")?
        .clone();

    assert_eq!(direct, aliased, "an alias binds the same declaration");
    assert_eq!(
        direct, reexported,
        "a re-export resolves to the declaring module, never the facade"
    );
    assert_eq!(
        direct.declaration_name, "helper",
        "the declaration-site spelling survives every alias"
    );
    assert_eq!(direct.origin, SymbolOrigin::Module(vec!["lib".to_string()]));

    // Reference-side recording sees the same identity through every spelling.
    let helper_ref = identity_at(&checker, nth_span(consumer_source, "helper", 2)?, "direct reference")?;
    let h_ref = identity_at(
        &checker,
        nth_span(consumer_source, "b = h", 0).map(|span| Span::new(span.end - 1, span.end))?,
        "alias reference",
    )?;
    let run_ref = identity_at(&checker, nth_span(consumer_source, "run", 1)?, "re-export reference")?;
    assert_eq!(helper_ref, direct);
    assert_eq!(h_ref, direct);
    assert_eq!(run_ref, direct);
    Ok(())
}

/// A facade republishing a stdlib function no provider serves binds the declaration a direct import binds.
///
/// Issue #1435: the re-export hop resolved `std.*` members from source metadata without carrying their identity,
/// so a consumer importing through the facade held an identity-less binding while the direct spelling was proven.
#[test]
fn stdlib_facade_reexport_shares_the_direct_import_identity_issue1435() -> Result<(), String> {
    let facade = parse("pub from std.regex import compile\n", "stdlib facade")?;
    let consumer_source = r#"
from std.regex import compile
from codec import compile as facade_compile

def use_all() -> None:
  a = compile
  b = facade_compile
"#;
    let consumer = parse(consumer_source, "stdlib facade consumer")?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["consumer".to_string()]));
    checker.register_dependency_module_path_segments("codec", vec!["codec".to_string()]);
    checker
        .check_with_imports(&consumer, &[("codec", &facade)])
        .map_err(|errors| format!("stdlib facade consumer should typecheck: {errors:?}"))?;

    // Reference-side recording is the proof lowering consumes: both spellings must name one declaration.
    let direct = identity_at(
        &checker,
        nth_span(consumer_source, "a = compile", 0).map(|span| Span::new(span.end - "compile".len(), span.end))?,
        "direct stdlib reference",
    )?;
    let reexported = identity_at(
        &checker,
        nth_span(consumer_source, "facade_compile", 1)?,
        "facade re-export reference",
    )?;
    assert_eq!(
        direct, reexported,
        "the facade binds the declaration, never a facade-minted identity"
    );
    assert_eq!(direct.declaration_name, "compile");
    assert_eq!(direct.kind, SemanticSourceTargetKind::Function);
    assert!(
        matches!(&direct.origin, SymbolOrigin::Module(path) if path.first().map(String::as_str) == Some("std")),
        "a source-served stdlib declaration is owned by its std module: {:?}",
        direct.origin
    );
    Ok(())
}

/// Static imports keep the provider declaration identity even though statics are not codegraph source targets.
#[test]
fn static_import_alias_and_reexport_share_the_declaration_identity() -> Result<(), String> {
    let provider = parse("pub static counter: int = 1\n", "static identity provider")?;
    let facade = parse("pub from provider import counter\n", "static identity facade")?;
    let consumer_source = r#"
from facade import counter as shared

def read() -> int:
  return shared
"#;
    let consumer = parse(consumer_source, "static identity consumer")?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["consumer".to_string()]));
    checker
        .check_with_imports(&consumer, &[("provider", &provider), ("facade", &facade)])
        .map_err(|errors| format!("static identity consumer should typecheck: {errors:?}"))?;

    let imported = checker
        .type_info()
        .resolved_import_identity("shared")
        .ok_or("re-exported static import must prove an identity")?
        .clone();
    assert_eq!(imported.kind, SemanticSourceTargetKind::Static);
    assert_eq!(imported.declaration_name, "counter");
    assert_eq!(imported.origin, SymbolOrigin::Module(vec!["provider".to_string()]));
    assert_eq!(
        identity_at(
            &checker,
            nth_span(consumer_source, "shared", 1)?,
            "re-exported static read",
        )?,
        imported
    );
    Ok(())
}

/// An SDK component's own `std.*` spelling resolves to the exact physical source declaration covered by its
/// bootstrap grant. The public spelling remains a binding path; it must not mint a second provider identity.
#[test]
fn bootstrap_std_import_uses_physical_provider_source_identity() -> Result<(), String> {
    let registry = parse(
        r#"
pub model RegistrySubject:
  label: str

  @staticmethod
  def current_unit() -> Self:
    return RegistrySubject(label="unit")

pub class Registry:
  def entry(self) -> int:
    return 1
"#,
        "bootstrap registry provider",
    )?;
    let source = r#"
from std.registry import Registry, RegistrySubject

def read(registry: Registry) -> int:
  return registry.entry()

def subject() -> RegistrySubject:
  return RegistrySubject.current_unit()
"#;
    let consumer = parse(source, "bootstrap registry consumer")?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["features".to_string()]));
    checker.register_dependency_module_path_segments("registry", vec!["registry".to_string()]);
    checker.set_provider_plan(Arc::new(
        ProviderPlan::default().with_bootstrap_sdk_namespace_roots(["registry".to_string()]),
    ));
    checker
        .check_with_imports_allow_private(&consumer, &[("registry", &registry)])
        .map_err(|errors| format!("bootstrap registry consumer should typecheck: {errors:?}"))?;

    for imported_name in ["Registry", "RegistrySubject"] {
        let identity = checker
            .type_info()
            .resolved_import_identity(imported_name)
            .ok_or_else(|| format!("missing resolved import identity for {imported_name}"))?;
        assert_eq!(identity.origin, SymbolOrigin::Module(vec!["registry".to_string()]));
    }
    let entry = identity_at(
        &checker,
        nth_span(source, "registry.entry()", 0)?,
        "Registry.entry call",
    )?;
    assert_eq!(entry.declaration_name, "entry");
    assert_eq!(entry.kind, SemanticSourceTargetKind::Method);
    assert_eq!(entry.origin, SymbolOrigin::Module(vec!["registry".to_string()]));
    let current_unit = identity_at(
        &checker,
        nth_span(source, "RegistrySubject.current_unit()", 0)?,
        "RegistrySubject.current_unit call",
    )?;
    assert_eq!(current_unit.declaration_name, "current_unit");
    assert_eq!(current_unit.kind, SemanticSourceTargetKind::Method);
    assert_eq!(current_unit.origin, SymbolOrigin::Module(vec!["registry".to_string()]));
    Ok(())
}

/// Nested field method lookup during provider bootstrap must use the physical source dependency metadata too. A
/// direct import of `Date` would hide the stale-stub precedence bug this test guards against.
#[test]
fn bootstrap_nested_member_uses_physical_provider_source_identity() -> Result<(), String> {
    let naive = parse(
        r#"
pub model Date:
  day: int

  def add_months(self, months: int) -> Self:
    return self

pub model DateTime:
  pub date: Date
"#,
        "bootstrap datetime provider",
    )?;
    let source = r#"
from std.datetime.civil.naive import DateTime

def shift(value: DateTime) -> None:
  _ = value.date.add_months(1)
"#;
    let consumer = parse(source, "bootstrap datetime consumer")?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec![
        "datetime".to_string(),
        "civil".to_string(),
        "offset".to_string(),
    ]));
    checker.set_current_package_identity(Some("incan_stdlib_data".to_string()));
    checker.register_dependency_module_path_segments(
        "datetime_civil_naive",
        vec!["datetime".to_string(), "civil".to_string(), "naive".to_string()],
    );
    checker.set_provider_plan(Arc::new(
        ProviderPlan::default().with_bootstrap_sdk_namespace_roots(["datetime".to_string()]),
    ));
    checker
        .check_with_imports_allow_private(&consumer, &[("datetime_civil_naive", &naive)])
        .map_err(|errors| format!("bootstrap nested datetime consumer should typecheck: {errors:?}"))?;

    let add_months = identity_at(
        &checker,
        nth_span(source, "value.date.add_months(1)", 0)?,
        "nested Date.add_months call",
    )?;
    assert_eq!(add_months.declaration_name, "add_months");
    assert_eq!(add_months.kind, SemanticSourceTargetKind::Method);
    assert_eq!(
        add_months.origin,
        SymbolOrigin::Package {
            library: "incan_stdlib_data".to_string(),
            module_path: vec!["datetime".to_string(), "civil".to_string(), "naive".to_string()],
        }
    );
    Ok(())
}

/// Provider bootstrap resolution remains physical when a public `std.*` facade re-exports another source module from
/// the same component. Following the facade through its written public path must not mint a second member identity.
#[test]
fn bootstrap_std_facade_reexport_uses_physical_member_identity() -> Result<(), String> {
    let file = parse(
        r#"
pub class File:
  def flush(self) -> None:
    pass
"#,
        "bootstrap fs.file provider",
    )?;
    let facade = parse("from std.fs.file import File\n", "bootstrap fs facade")?;
    let source = r#"
from std.fs import File

def flush(file: File) -> None:
  file.flush()
"#;
    let consumer = parse(source, "bootstrap tempfile consumer")?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["tempfile".to_string()]));
    checker.register_dependency_module_path_segments("fs_file", vec!["fs".to_string(), "file".to_string()]);
    checker.register_dependency_module_path_segments("fs", vec!["fs".to_string()]);
    checker.set_provider_plan(Arc::new(
        ProviderPlan::default().with_bootstrap_sdk_namespace_roots(["fs".to_string()]),
    ));
    checker
        .check_with_imports_allow_private(&consumer, &[("fs_file", &file), ("fs", &facade)])
        .map_err(|errors| format!("bootstrap tempfile consumer should typecheck: {errors:?}"))?;

    let imported = checker
        .type_info()
        .resolved_import_identity("File")
        .ok_or("facade-imported File must prove its physical declaration identity")?;
    assert_eq!(
        imported.origin,
        SymbolOrigin::Module(vec!["fs".to_string(), "file".to_string()])
    );
    let flush = identity_at(&checker, nth_span(source, "file.flush()", 0)?, "File.flush call")?;
    assert_eq!(flush.declaration_name, "flush");
    assert_eq!(flush.kind, SemanticSourceTargetKind::Method);
    assert_eq!(
        flush.origin,
        SymbolOrigin::Module(vec!["fs".to_string(), "file".to_string()])
    );
    Ok(())
}

/// A dependency symbol without its compiler-retained canonical identity is unproven; lookup must fail closed rather
/// than reconstructing an identity from the dependency key, source spelling, kind, and span.
#[test]
fn dependency_member_identity_does_not_reconstruct_missing_canonical_data() -> Result<(), String> {
    let provider = parse(
        "pub def helper() -> int:\n  return 1\n",
        "fail-closed identity provider",
    )?;
    let consumer = parse(
        "from provider import helper\n\ndef run() -> int:\n  return helper()\n",
        "fail-closed identity consumer",
    )?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["consumer".to_string()]));
    checker.register_dependency_module_path_segments("provider", vec!["provider".to_string()]);
    checker
        .check_with_imports(&consumer, &[("provider", &provider)])
        .map_err(|errors| format!("fail-closed identity consumer should typecheck: {errors:?}"))?;

    let path = crate::ast::ImportPath::simple(vec!["provider".to_string()]);
    assert!(
        checker.dependency_member_identity(&path, "helper").is_some(),
        "the intact compiler-owned dependency cache must prove the declaration"
    );
    checker
        .dependency_direct_member_identities
        .get_mut("provider")
        .ok_or("provider identity cache must exist")?
        .remove("helper");
    assert_eq!(
        checker.dependency_member_identity(&path, "helper"),
        None,
        "missing canonical data must remain absent rather than being reconstructed"
    );
    Ok(())
}

/// Diagnostics keep the consumer spelling primary while carrying the imported declaration's own source identity.
#[test]
fn imported_alias_call_error_retains_original_declaration_location() -> Result<(), String> {
    let provider_source = r#"
pub def parse(value: int) -> int:
  return value
"#;
    let provider = parse(provider_source, "diagnostic provider")?;
    let consumer_source = r#"
from provider import parse as alias

def run() -> int:
  return alias("bad")
"#;
    let consumer = parse(consumer_source, "diagnostic consumer")?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["consumer".to_string()]));
    checker.register_dependency_module_path_segments("provider", vec!["provider".to_string()]);
    let errors = match checker.check_with_imports(&consumer, &[("provider", &provider)]) {
        Ok(()) => return Err("alias call with a str argument unexpectedly typechecked".to_string()),
        Err(errors) => errors,
    };
    let mismatch = errors
        .iter()
        .find(|error| error.message.contains("alias") && error.message.contains("Argument"))
        .ok_or_else(|| format!("missing alias argument diagnostic: {errors:?}"))?;
    assert_eq!(mismatch.span, nth_span(consumer_source, "\"bad\"", 0)?);
    let related = mismatch
        .related_declarations()
        .first()
        .ok_or("alias diagnostic must carry the provider declaration")?;
    assert_eq!(
        related.identity.origin,
        SymbolOrigin::Module(vec!["provider".to_string()])
    );
    assert_eq!(related.identity.declaration_name, "parse");
    assert_eq!(
        related.identity.declaration_span.start,
        provider_source
            .find("pub def parse")
            .ok_or("provider declaration missing")?
    );
    assert!(mismatch.related_spans().is_empty());
    Ok(())
}

/// Type annotations reached through an import alias and a re-export retain the original type declaration identity.
#[test]
fn imported_alias_and_reexport_type_annotations_share_the_declaration_identity() -> Result<(), String> {
    let provider = parse("pub model Item:\n  value: int\n", "type identity provider")?;
    let facade = parse("pub from provider import Item as PublicItem\n", "type identity facade")?;
    let consumer_source = r#"
from facade import PublicItem as LocalItem

def keep(value: LocalItem) -> LocalItem:
  return value
"#;
    let consumer = parse(consumer_source, "type identity consumer")?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["consumer".to_string()]));
    checker
        .check_with_imports(&consumer, &[("provider", &provider), ("facade", &facade)])
        .map_err(|errors| format!("type identity consumer should typecheck: {errors:?}"))?;

    let imported = checker
        .type_info()
        .resolved_import_identity("LocalItem")
        .ok_or("LocalItem import must prove its target identity")?
        .clone();
    let parameter = identity_at(
        &checker,
        nth_span(consumer_source, "LocalItem", 1)?,
        "imported parameter annotation",
    )?;
    let returned = identity_at(
        &checker,
        nth_span(consumer_source, "LocalItem", 2)?,
        "imported return annotation",
    )?;
    assert_eq!(parameter, imported);
    assert_eq!(returned, imported);
    assert_eq!(imported.declaration_name, "Item");
    assert_eq!(imported.origin, SymbolOrigin::Module(vec!["provider".to_string()]));
    Ok(())
}

/// Preparing a dependency records its interface for explicit import resolution without placing its declarations in
/// the consumer's lexical scope.
#[test]
fn dependency_interfaces_are_not_ambient_consumer_bindings() -> Result<(), String> {
    let provider = parse(
        "pub model Hidden:\n  value: int\n\npub trait Contract:\n  def read(self) -> int\n\npub def helper() -> int:\n  return 1\n",
        "dependency interface provider",
    )?;
    let consumer = parse(
        "def read(value: Hidden) -> int:\n  observed = helper\n  return 1\n",
        "dependency interface consumer",
    )?;
    let mut checker = TypeChecker::new();
    let errors = match checker.check_with_imports(&consumer, &[("provider", &provider)]) {
        Ok(()) => return Err("unimported dependency declarations leaked into consumer scope".to_string()),
        Err(errors) => errors,
    };
    assert!(
        errors.iter().any(|error| error.message == "Unknown symbol 'Hidden'"),
        "expected unimported type diagnostic, got: {errors:?}"
    );
    assert!(
        errors.iter().any(|error| error.message == "Unknown symbol 'helper'"),
        "expected unimported value diagnostic, got: {errors:?}"
    );
    for name in ["Hidden", "Contract", "helper"] {
        assert_eq!(
            checker.symbols.lookup(name),
            None,
            "dependency interface binding `{name}` survived into consumer lookup"
        );
    }
    Ok(())
}

/// An explicit import creates exactly one consumer binding carrying the provider declaration's identity; sibling
/// exports remain absent.
#[test]
fn explicit_dependency_import_materializes_only_its_aliased_binding() -> Result<(), String> {
    let provider = parse(
        "pub model Imported:\n  value: int\n\npub model Unimported:\n  value: int\n",
        "explicit import provider",
    )?;
    let consumer_source =
        "from provider import Imported as Local\n\ndef read(value: Local) -> Local:\n  return value\n";
    let consumer = parse(consumer_source, "explicit import consumer")?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["consumer".to_string()]));
    checker
        .check_with_imports(&consumer, &[("provider", &provider)])
        .map_err(|errors| format!("explicit import should typecheck: {errors:?}"))?;
    assert!(
        checker.symbols.lookup("Local").is_some(),
        "the explicit alias must be active"
    );
    assert_eq!(
        checker.symbols.lookup("Unimported"),
        None,
        "an unimported sibling declaration must remain absent"
    );
    let identity = checker
        .type_info()
        .declarations
        .resolved_import_identities
        .get("Local")
        .ok_or("the explicit alias must carry a resolved identity")?;
    assert_eq!(identity.origin, SymbolOrigin::Module(vec!["provider".to_string()]));
    assert_eq!(identity.declaration_name, "Imported");
    Ok(())
}

/// A public source alias remains a binding to its target declaration after crossing a dependency boundary.
#[test]
fn dependency_public_alias_preserves_its_target_identity() -> Result<(), String> {
    let provider_source = r#"
pub def helper() -> int:
  return 1

pub run = alias helper
"#;
    let provider = parse(provider_source, "public alias provider")?;
    let consumer = parse(
        "from provider import run\n\ndef read() -> int:\n  return run()\n",
        "public alias consumer",
    )?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["consumer".to_string()]));
    checker
        .check_with_imports(&consumer, &[("provider", &provider)])
        .map_err(|errors| format!("public alias import should typecheck: {errors:?}"))?;
    let identity = checker
        .type_info()
        .declarations
        .resolved_import_identities
        .get("run")
        .ok_or("the imported public alias must retain its target identity")?;
    assert_eq!(identity.origin, SymbolOrigin::Module(vec!["provider".to_string()]));
    assert_eq!(identity.declaration_name, "helper");
    let helper_span = provider.declarations[0].span;
    assert_eq!(
        identity.declaration_span,
        incan_semantics_core::HirSourceSpan::new(helper_span.start, helper_span.end),
        "the alias import must remain anchored at the target declaration"
    );
    Ok(())
}

/// Dependency-owned transparent aliases do not rewrite a same-spelled local nominal declaration, while an explicit
/// aliased import receives the exact provider-owned target.
#[test]
fn dependency_type_alias_targets_are_isolated_until_explicitly_imported() -> Result<(), String> {
    let provider = parse("pub type Payload = str\n", "type alias provider")?;
    let local = parse(
        "model Payload:\n  value: int\n\ndef keep(value: Payload) -> Payload:\n  return value\n",
        "local nominal consumer",
    )?;
    let mut checker = TypeChecker::new();
    checker
        .check_with_imports(&local, &[("provider", &provider)])
        .map_err(|errors| format!("local nominal type should remain independent: {errors:?}"))?;
    assert!(
        !checker.type_aliases.contains_key("Payload"),
        "an unimported dependency alias target leaked into the consumer"
    );

    let imported = parse(
        "from provider import Payload as TextPayload\n\ndef read(value: TextPayload) -> str:\n  return value\n",
        "explicit type alias consumer",
    )?;
    let mut checker = TypeChecker::new();
    checker
        .check_with_imports(&imported, &[("provider", &provider)])
        .map_err(|errors| format!("explicit type alias import should typecheck: {errors:?}"))?;
    let target = checker
        .type_aliases
        .get("TextPayload")
        .ok_or("the explicit type alias import must retain its target")?;
    assert_eq!(target.target, crate::symbols::ResolvedType::Str);
    Ok(())
}

/// A rejected duplicate type alias cannot overwrite the active alias-expansion target.
#[test]
fn duplicate_type_alias_side_table_keeps_the_first_target() -> Result<(), String> {
    let source = "type Payload = int\ntype Payload = str\n";
    let program = parse(source, "duplicate type alias target")?;
    let mut checker = TypeChecker::new();
    let errors = match checker.check_program(&program) {
        Ok(()) => return Err("duplicate type aliases were accepted".to_string()),
        Err(errors) => errors,
    };
    assert!(
        errors
            .iter()
            .any(|error| error.message == "Duplicate definition of 'Payload'"),
        "expected duplicate type-alias diagnostic, got: {errors:?}"
    );
    let target = checker
        .type_aliases
        .get("Payload")
        .ok_or("the first type alias target must remain active")?;
    assert_eq!(target.target, crate::symbols::ResolvedType::Int);
    Ok(())
}

/// Per-module predeclaration keeps equal spellings from sibling dependencies independent even when the importing
/// bridge is collected before either provider.
#[test]
fn dependency_predeclaration_is_module_exact_and_order_independent() -> Result<(), String> {
    let left = parse("pub model Contract[T]:\n  value: T\n", "left contract")?;
    let right = parse("pub model Contract[A, B]:\n  first: A\n  second: B\n", "right contract")?;
    let bridge = parse(
        "from right import Contract\n\npub Selected = alias Contract\n\npub def pair(value: Contract[int, str]) -> Contract[int, str]:\n  return value\n",
        "contract bridge",
    )?;
    let consumer = parse("def noop() -> None:\n  pass\n", "order consumer")?;
    for dependencies in [
        [("bridge", &bridge), ("left", &left), ("right", &right)],
        [("bridge", &bridge), ("right", &right), ("left", &left)],
    ] {
        let mut checker = TypeChecker::new();
        checker
            .check_with_imports(&consumer, &dependencies)
            .map_err(|errors| format!("module-exact dependency collection failed: {errors:?}"))?;
        let selected = checker
            .dependency_member_symbols
            .get("bridge")
            .and_then(|members| members.get("Selected"))
            .ok_or("bridge.Selected was not cached")?;
        let crate::symbols::SymbolKind::Type(crate::symbols::TypeInfo::Model(info)) = selected else {
            return Err(format!("bridge.Selected is not a model alias: {selected:?}"));
        };
        assert_eq!(
            info.type_params,
            vec!["A".to_string(), "B".to_string()],
            "bridge.Selected must carry right.Contract's two-parameter interface regardless of collection order"
        );
    }
    Ok(())
}

/// The non-canonical same-leaf fallback fails closed when more than one dependency module can answer it.
#[test]
fn ambiguous_dependency_leaf_fallback_does_not_select_by_order() -> Result<(), String> {
    let first = parse("pub model Contract:\n  first: int\n", "first helpers")?;
    let second = parse("pub model Contract:\n  second: str\n", "second helpers")?;
    let consumer = parse("def noop() -> None:\n  pass\n", "ambiguous leaf consumer")?;
    let mut checker = TypeChecker::new();
    checker.register_dependency_module_path_segments("pkg_helpers", vec!["pkg".to_string(), "helpers".to_string()]);
    checker.register_dependency_module_path_segments("other_helpers", vec!["other".to_string(), "helpers".to_string()]);
    checker
        .check_with_imports(&consumer, &[("pkg_helpers", &first), ("other_helpers", &second)])
        .map_err(|errors| format!("dependency cache setup should typecheck: {errors:?}"))?;
    let ambiguous_path = crate::ast::ImportPath::simple(vec!["helpers".to_string()]);
    assert!(
        checker
            .dependency_member_symbol_for_path(&ambiguous_path, "Contract")
            .is_none(),
        "same-leaf fallback must not select whichever dependency was collected first"
    );
    Ok(())
}

/// Module imports participate in the same ambiguity mechanism as item imports.
#[test]
fn same_alias_for_different_module_imports_is_ambiguous() -> Result<(), String> {
    let left = parse("pub def left_value() -> int:\n  return 1\n", "left module")?;
    let right = parse("pub def right_value() -> int:\n  return 2\n", "right module")?;
    let consumer = parse(
        "import left as shared\nimport right as shared\n",
        "module alias consumer",
    )?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["consumer".to_string()]));
    let errors = match checker.check_with_imports(&consumer, &[("left", &left), ("right", &right)]) {
        Ok(()) => return Err("different modules shared one local import alias".to_string()),
        Err(errors) => errors,
    };
    assert!(
        errors
            .iter()
            .any(|error| error.message == "Ambiguous import binding 'shared'"),
        "expected shared module-import ambiguity diagnostic, got: {errors:?}"
    );
    assert_eq!(
        checker.type_info().import_binding_path("shared"),
        Some(["left".to_string()].as_slice()),
        "decorator/module routing must retain the first accepted import"
    );
    let active = checker
        .symbols
        .lookup("shared")
        .ok_or("shared module binding must remain active")?;
    let Some(crate::symbols::SymbolKind::Module(module)) = checker.symbols.get(active).map(|symbol| &symbol.kind)
    else {
        return Err("shared did not remain a module binding".to_string());
    };
    assert_eq!(module.path, vec!["left".to_string()]);
    Ok(())
}
