//! Trait bounds and derived capabilities: `Clone` / `Eq` / ordinal-key bounds, `@derive` on models, enums and newtypes,
//! module and serde derives, and which derives satisfy which generic bounds.

use super::*;

#[test]
fn test_explicit_clone_bound_accepts_builtin_clone_types() {
    let source = r#"
def identity[T with Clone](value: T) -> T:
  return value

def main() -> int:
  return identity(1)
"#;
    assert_check_ok(source);
}

#[test]
fn test_ordinal_key_bound_accepts_builtin_deterministic_keys() {
    let source = r#"
from std.collections import OrdinalKey

def accept_key[T with OrdinalKey](value: T) -> T:
  return value

def accept_str() -> str:
  return accept_key("abc")

def accept_bytes() -> bytes:
  return accept_key(b"abc")

def accept_bool() -> bool:
  return accept_key(true)

def accept_int() -> int:
  return accept_key(1)

def accept_i32(value: i32) -> i32:
  return accept_key(value)

def accept_u8(value: u8) -> u8:
  return accept_key(value)

def accept_decimal(value: decimal[5, 2]) -> decimal[5, 2]:
  return accept_key(value)
"#;
    assert_check_ok(source);
}

#[test]
fn test_ordinal_key_bound_accepts_import_alias() {
    let source = r#"
from std.collections import OrdinalKey as Key

def accept_key[T with Key](value: T) -> T:
  return value

def accept_str() -> str:
  return accept_key("abc")

def accept_i32(value: i32) -> i32:
  return accept_key(value)
"#;
    assert_check_ok(source);
}

#[test]
fn test_ordinal_key_bound_accepts_value_enums() {
    let source = r#"
from std.collections import OrdinalKey

enum Env(str):
  Dev = "development"
  Prod = "production"

enum HttpStatus(int):
  Ok = 200
  NotFound = 404

def accept_key[T with OrdinalKey](value: T) -> T:
  return value

def accept_env(value: Env) -> Env:
  return accept_key(value)

def accept_status(value: HttpStatus) -> HttpStatus:
  return accept_key(value)
"#;
    assert_check_ok(source);
}

fn assert_ordinal_key_bound_rejects_builtin(type_name: &str) {
    let source = format!(
        r#"
from std.collections import OrdinalKey

def accept_key[T with OrdinalKey](value: T) -> T:
  return value

def accept_value(value: {type_name}) -> {type_name}:
  return accept_key(value)
"#
    );
    let errs = check_str_err(&source, &format!("{type_name} should fail explicit OrdinalKey bound"));
    assert!(
        errs.iter()
            .any(|e| e.message.contains("violates generic bound") && e.message.contains(type_name)),
        "Expected explicit OrdinalKey bound error mentioning {type_name}; got: {:?}",
        errs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_ordinal_key_bound_rejects_float_builtin() {
    for type_name in ["float", "f32", "f64"] {
        assert_ordinal_key_bound_rejects_builtin(type_name);
    }
}

#[test]
fn test_ordinal_key_bound_rejects_pointer_sized_integer_builtin() {
    for type_name in ["usize", "isize"] {
        assert_ordinal_key_bound_rejects_builtin(type_name);
    }
}

#[test]
fn test_local_ordinal_key_shape_does_not_grant_builtin_support() {
    let source = r#"
trait OrdinalKey:
  def ordinal_bytes(self) -> bytes: ...
  def ordinal_encoding() -> str: ...
  def from_ordinal_bytes(data: bytes) -> Result[Self, str]: ...

def accept_key[T with OrdinalKey](value: T) -> T:
  return value

def accept_str() -> str:
  return accept_key("abc")
	"#;
    let errs = check_str_err(source, "local OrdinalKey-shaped trait should not grant builtin support");
    assert!(
        errs.iter().any(|e| e.message.contains("violates generic bound")),
        "Expected explicit generic bound error; got: {:?}",
        errs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_ordinal_map_backing_fields_are_private() {
    let source = r#"
from std.collections import OrdinalMap

def leak(columns: OrdinalMap[str]) -> int:
  return len(columns.key_values)
"#;
    let errors = check_str_err(source, "OrdinalMap backing field access should fail typechecking");
    assert!(
        has_private_field_error(&errors, "OrdinalMap", "key_values"),
        "expected private field error, got: {:?}",
        errors.iter().map(|error| &error.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_ordinal_key_bound_preserves_nominal_adoption() {
    let source = r#"
trait OrdinalKey:
  def ordinal_bytes(self) -> bytes: ...
  def ordinal_encoding() -> str: ...
  def from_ordinal_bytes(data: bytes) -> Result[Self, str]: ...

model UserId with OrdinalKey:
  value: int

  def ordinal_bytes(self) -> bytes:
    return b"user-id"

  @staticmethod
  def ordinal_encoding() -> str:
    return "user-id:v1"

  @staticmethod
  def from_ordinal_bytes(data: bytes) -> Result[Self, str]:
    return Ok(UserId(value=len(data)))

def accept_key[T with OrdinalKey](value: T) -> T:
  return value

def accept_user_id(value: UserId) -> UserId:
  return accept_key(value)
"#;
    assert_check_ok(source);
}

/// A model always derives `Clone` (the reference's automatic derives), so it satisfies an explicit `Clone` bound
/// without `@derive(Clone)`; the derive relation answers the bound as lowering realizes it (#1754).
#[test]
fn test_explicit_clone_bound_accepts_a_model_through_its_automatic_derive() {
    let source = r#"
model Token:
  value: int

def identity[T with Clone](value: T) -> T:
  return value

def main() -> Token:
  return identity(Token(value=1))
"#;
    assert_check_ok(source);
}

/// GitHub #193: `@derive(Clone)` must allow `.clone()` on the concrete type (not only through unconstrained `T`).
#[test]
fn test_derive_clone_allows_direct_clone_on_model() {
    let source = r#"
@derive(Clone)
model Issue193Foo:
  id: int

def direct(f: Issue193Foo) -> Issue193Foo:
  return f.clone()

def via_generic[T](x: T) -> T:
  return x.clone()

def main() -> Issue193Foo:
  x = Issue193Foo(id=1)
  _ = via_generic(x)
  return direct(x)
"#;
    assert_check_ok(source);
}

#[test]
fn test_derive_clone_allows_direct_clone_on_enum() {
    let source = r#"
@derive(Clone)
enum Issue193Bar:
  A
  B

def dup_bar(e: Issue193Bar) -> Issue193Bar:
  return e.clone()

def main() -> None:
  pass
"#;
    assert_check_ok(source);
}

#[test]
fn test_explicit_serialize_trait_adoption_allows_default_to_json() {
    let source = r#"
from std.serde.json import Serialize

model Payload with Serialize:
  value: int

def encode(payload: Payload) -> str:
  return payload.to_json()
"#;
    assert_check_ok(source);
}

#[test]
fn test_bare_serde_derive_without_import_is_rejected() {
    let source = r#"
@derive(Serialize)
model Payload:
  value: int
"#;
    let Err(errs) = check_str(source) else {
        panic!("bare Serialize derive should require an imported std.serde.json trait or Rust derive");
    };
    assert!(
        errs.iter()
            .any(|err| err.message.contains("Unknown derive 'Serialize'")),
        "Expected unknown derive diagnostic; got: {errs:?}"
    );
}

#[test]
fn test_rust_imported_serde_derive_still_typechecks() {
    let source = r#"
from rust::serde @ "1.0" import Deserialize

@derive(Deserialize)
model Payload:
  value: int
"#;
    assert_check_ok(source);
}

#[test]
fn test_module_derive_json_adopts_traits_for_methods_and_bounds() {
    let source = r#"
from std.serde import json

@derive(json)
model Payload:
  value: int

def encode[T with json.Serialize](value: T) -> str:
  return value.to_json()

def main() -> str:
  return encode(Payload(value=1))
"#;
    assert_check_ok(source);
}

#[test]
fn test_user_module_derive_adopts_imported_module_traits_for_methods_and_bounds() {
    let yaml_source = r#"
__derives__ = [Serialize]

@rust.derive("serde::Serialize")
pub trait Serialize:
  def to_yaml(self) -> str:
    return str("yaml")
"#;
    let source = r#"
import yaml

@derive(yaml)
model Payload:
  value: int

def encode[T with yaml.Serialize](value: T) -> str:
  return value.to_yaml()

def main() -> str:
  return encode(Payload(value=1))
"#;

    let yaml_ast = parse_program(yaml_source, "yaml module");
    let ast = parse_program(source, "consumer");
    let mut checker = TypeChecker::new();
    checker
        .check_with_imports(&ast, &[("yaml", &yaml_ast)])
        .unwrap_or_else(|errs| panic!("user derivable module should typecheck: {errs:?}"));
}

#[test]
fn test_user_module_derive_bound_survives_exported_enum_variant_spelled_like_the_trait() -> Result<(), String> {
    // #1429: a dependency's exported enum variant with the same spelling as its derivable trait must not steal the
    // trait's lookup binding while the dependency interface is collected. The variant is a member convenience binding
    // in the enum's namespace; the trait bound and the module derive resolve the trait declaration whichever of the
    // two is declared first.
    for codec_source in [
        r#"
__derives__ = [Ready]

@rust.derive("Debug")
pub trait Ready:
  pass

pub enum Kind(str):
  Ready = "ready"

pub def encode[T with Ready](value: T) -> None:
  pass
"#,
        r#"
__derives__ = [Ready]

pub enum Kind(str):
  Ready = "ready"

@rust.derive("Debug")
pub trait Ready:
  pass

pub def encode[T with Ready](value: T) -> None:
  pass
"#,
    ] {
        // The module derive and the directly imported trait derive both resolve the trait declaration; the second
        // form reported the derive as unknown when the variant had taken the binding.
        for source in [
            r#"
import codec

@derive(codec)
model Item:
  value: int

def main() -> None:
  codec.encode(Item(value=1))
"#,
            r#"
from codec import Ready, encode

@derive(Ready)
model Item:
  value: int

def main() -> None:
  encode(Item(value=1))
"#,
        ] {
            let codec_ast = parse_program(codec_source, "codec module");
            let ast = parse_program(source, "consumer");
            let mut checker = TypeChecker::new();
            checker
                .check_with_imports(&ast, &[("codec", &codec_ast)])
                .map_err(|errs| {
                    format!("the derive should satisfy the bound despite the same-spelled variant: {errs:?}\n{source}")
                })?;
        }
    }
    Ok(())
}

/// #1431: a stdlib trait reached through a facade re-export binds the stdlib trait itself, and the recorded import
/// identity names the declaring `std.serde.json` module rather than the facade. Lowering keys the trait's protocol on
/// that identity, so the written import path (which names only the facade) must not be the only fact available.
#[test]
fn test_facade_reexported_stdlib_trait_records_declaring_identity() -> Result<(), String> {
    let facade_source = "from std.serde.json import Serialize, Deserialize\n";
    let source = r#"
from facade import Serialize, Deserialize

model Payload with Serialize, Deserialize:
  value: int

  def from_json(json_str: str) -> Result[Payload, str]:
    return Ok(Payload(value=len(json_str)))

def main() -> None:
  println(Payload(value=1).to_json())
"#;
    let facade_ast = parse_program(facade_source, "facade");
    let ast = parse_program(source, "consumer");
    let mut checker = TypeChecker::new();
    checker
        .check_with_imports(&ast, &[("facade", &facade_ast)])
        .map_err(|errs| format!("facade re-export should typecheck: {errs:?}"))?;

    let json_module = vec!["std".to_string(), "serde".to_string(), "json".to_string()];
    for trait_name in ["Serialize", "Deserialize"] {
        let identity = checker
            .type_info()
            .resolved_import_identity(trait_name)
            .ok_or_else(|| format!("no resolved import identity recorded for facade-re-exported {trait_name}"))?;
        if identity.origin != SymbolOrigin::Module(json_module.clone())
            || identity.declaration_name != trait_name
            || identity.kind != SemanticSourceTargetKind::Trait
        {
            return Err(format!(
                "{trait_name} identity must name the declaring stdlib module: {identity:?}"
            ));
        }
        if checker.lookup_trait_info(trait_name).is_none() {
            return Err(format!(
                "{trait_name} must bind the stdlib trait, not an import placeholder"
            ));
        }
        // The written import path keeps its meaning: it names the facade the consumer actually imported from.
        if checker.import_binding_path(trait_name) != Some(["facade".to_string(), trait_name.to_string()].as_slice()) {
            return Err(format!(
                "{trait_name} import binding path must stay the written facade path"
            ));
        }
    }

    // A direct stdlib import records the same identity through the loading lookup.
    let direct = parse_program(
        "from std.serde.json import Serialize\n\nmodel Payload with Serialize:\n  value: int\n",
        "direct",
    );
    let mut direct_checker = TypeChecker::new();
    direct_checker
        .check_program(&direct)
        .map_err(|errs| format!("direct stdlib import should typecheck: {errs:?}"))?;
    let direct_identity = direct_checker
        .type_info()
        .resolved_import_identity("Serialize")
        .ok_or("no resolved import identity recorded for a direct stdlib trait import")?;
    if direct_identity.origin != SymbolOrigin::Module(json_module) {
        return Err(format!(
            "direct identity must name the declaring stdlib module: {direct_identity:?}"
        ));
    }
    Ok(())
}

/// An aliased `std.serde.json` import derives, bounds and dispatches like the direct one. The checker records the
/// alias's declaring identity (#1431), and lowering builds the dispatch path from that declaration name, never from
/// the alias spelling (#1712).
#[test]
fn test_aliased_partial_serde_derive_adopts_trait_for_methods_and_bounds() -> Result<(), String> {
    let source = r#"
from std.serde.json import Serialize as JsonSerialize

@derive(JsonSerialize)
model Payload:
  value: int

def encode[T with JsonSerialize](value: T) -> str:
  return value.to_json()

def main() -> str:
  return encode(Payload(value=1))
"#;
    let ast = parse_program(source, "aliased serde derive");
    let mut checker = TypeChecker::new();
    checker
        .check_program(&ast)
        .map_err(|errs| format!("aliased serde derive should typecheck: {errs:?}"))?;

    let json_module = vec!["std".to_string(), "serde".to_string(), "json".to_string()];
    let identity = checker
        .type_info()
        .resolved_import_identity("JsonSerialize")
        .ok_or("no resolved import identity recorded for the aliased stdlib trait import")?;
    if identity.origin != SymbolOrigin::Module(json_module.clone())
        || identity.declaration_name != "Serialize"
        || identity.kind != SemanticSourceTargetKind::Trait
    {
        return Err(format!(
            "the alias identity must name the declaring stdlib module and declaration: {identity:?}"
        ));
    }
    let dispatch = checker
        .type_info()
        .calls
        .resolved_method_calls
        .values()
        .find(|call| call.method == "to_json")
        .map(|call| call.dispatch.clone())
        .ok_or("the bounded `value.to_json()` call must resolve to a trait dispatch")?;
    let ResolvedMethodDispatch::Trait {
        trait_name,
        module_path,
        ..
    } = dispatch;
    if trait_name != "JsonSerialize" || module_path != Some(json_module) {
        return Err(format!(
            "the dispatch keeps the call site's spelling beside the declaring module: {trait_name} in {module_path:?}"
        ));
    }
    Ok(())
}

#[test]
fn test_module_derive_rejects_user_module_without_derives_metadata() {
    let yaml_source = r#"
pub trait Serialize:
  def to_yaml(self) -> str:
    return str("yaml")
"#;
    let source = r#"
import yaml

@derive(yaml)
model Payload:
  value: int
"#;

    let yaml_ast = parse_program(yaml_source, "yaml module");
    let ast = parse_program(source, "consumer");
    let mut checker = TypeChecker::new();
    let errs = checker
        .check_with_imports(&ast, &[("yaml", &yaml_ast)])
        .expect_err("module derive should require __derives__ metadata");
    assert!(
        errs.iter()
            .any(|err| err.message.contains("does not declare `__derives__`")),
        "Expected missing __derives__ diagnostic; got: {errs:?}"
    );
}

#[test]
fn test_user_module_derive_reports_method_collision_between_derived_traits() {
    let left_source = r#"
__derives__ = [Readable]

pub trait Readable:
  def label(self) -> str:
    return str("left")
"#;
    let right_source = r#"
__derives__ = [Displayable]

pub trait Displayable:
  def label(self) -> str:
    return str("right")
"#;
    let source = r#"
import left
import right

@derive(left, right)
model Item:
  value: int
"#;

    let left_ast = parse_program(left_source, "left module");
    let right_ast = parse_program(right_source, "right module");
    let ast = parse_program(source, "consumer");
    let mut checker = TypeChecker::new();
    let errs = checker
        .check_with_imports(&ast, &[("left", &left_ast), ("right", &right_ast)])
        .expect_err("derived traits with the same default method should be ambiguous");
    assert!(
        errs.iter()
            .any(|err| err.message.contains("Ambiguous trait method 'label'")),
        "Expected derived trait method collision diagnostic; got: {errs:?}"
    );
}

#[test]
fn test_derives_metadata_rejects_non_trait_entries() {
    let source = r#"
trait Good:
  def ok(self) -> None: ...

const Bad = 1
__derives__ = [Good, Bad]
"#;
    let errs = check_str_err(source, "__derives__ metadata should reject non-trait entries");
    assert!(
        errs.iter()
            .any(|err| err.message.contains("entry 'Bad' is not a trait")),
        "Expected non-trait __derives__ diagnostic; got: {:?}",
        errs
    );
}

#[test]
fn test_explicit_eq_bound_rejects_float_arguments() {
    let source = r#"
def show_eq[T with Eq](value: T) -> T:
  return value

def main() -> float:
  return show_eq(1.5)
"#;
    let Err(errs) = check_str(source) else {
        panic!("float should fail explicit Eq bound");
    };
    assert!(
        errs.iter().any(|e| e.message.contains("violates generic bound")),
        "Expected explicit Eq bound error; got: {:?}",
        errs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_derived_traits_on_direct_newtype_satisfy_generic_bounds() {
    let source = r#"
@derive(Clone, Eq)
type Identifier = newtype str

def require_traits[T with (Clone, Eq)](value: T) -> T:
  return value

def main() -> Identifier:
  return require_traits(Identifier("one"))
"#;
    assert_check_ok(source);
}

#[test]
fn test_derived_trait_on_newtype_satisfies_imported_alias_bound() {
    let source = r#"
from std.derives.comparison import Eq as Equality

@derive(Eq)
type Identifier = newtype str

def require_equality[T with Equality](value: T) -> T:
  return value

def main() -> Identifier:
  return require_equality(Identifier("one"))
"#;
    assert_check_ok(source);
}

#[test]
fn test_derived_trait_on_newtype_does_not_satisfy_unrelated_imported_trait_alias()
-> Result<(), Box<dyn std::error::Error>> {
    let unrelated_source = r#"
pub trait Eq:
  def unrelated(self) -> int: ...
"#;
    let source = r#"
from unrelated import Eq as Equality

@derive(Eq)
type Identifier = newtype str

def require_equality[T with Equality](value: T) -> T:
  return value

def main() -> Identifier:
  return require_equality(Identifier("one"))
"#;
    let unrelated_ast = parse_program(unrelated_source, "unrelated Eq provider");
    let ast = parse_program(source, "unrelated Eq alias consumer");
    let mut checker = TypeChecker::new();
    let errors = match checker.check_with_imports(&ast, &[("unrelated", &unrelated_ast)]) {
        Ok(()) => return Err("builtin Eq derive unexpectedly satisfied an unrelated imported Eq trait".into()),
        Err(errors) => errors,
    };
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("violates generic bound") && error.message.contains("Equality")),
        "expected unrelated Equality bound rejection, got: {errors:?}"
    );
    Ok(())
}

#[test]
fn test_derived_trait_on_newtype_does_not_satisfy_unrelated_unaliased_import() -> Result<(), Box<dyn std::error::Error>>
{
    let unrelated_source = r#"
pub trait Eq:
  def unrelated(self) -> int: ...
"#;
    let source = r#"
from unrelated import Eq

@derive(Eq)
type Identifier = newtype str

def require_equality[T with Eq](value: T) -> T:
  return value

def main() -> Identifier:
  return require_equality(Identifier("one"))
"#;
    let unrelated_ast = parse_program(unrelated_source, "unrelated unaliased Eq provider");
    let ast = parse_program(source, "unrelated unaliased Eq consumer");
    let mut checker = TypeChecker::new();
    let errors = match checker.check_with_imports(&ast, &[("unrelated", &unrelated_ast)]) {
        Ok(()) => return Err("builtin Eq derive unexpectedly satisfied an unrelated unaliased Eq trait".into()),
        Err(errors) => errors,
    };
    let eq_name = builtin_traits::as_str(TraitId::Eq);
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("violates generic bound") && error.message.contains(eq_name)),
        "expected unrelated {eq_name} bound rejection, got: {errors:?}"
    );
    Ok(())
}

#[test]
fn test_imported_stdlib_newtype_derives_satisfy_generic_bounds() {
    let source = r#"
from std.graph import NodeId

def require_traits[T with (Clone, Eq)](value: T) -> T:
  return value

def main() -> NodeId:
  return require_traits(NodeId(1))
"#;
    assert_check_ok(source);
}

#[test]
fn test_user_module_derives_on_newtypes_use_owner_bearing_trait_adoptions() -> Result<(), Box<dyn std::error::Error>> {
    let yaml_source = r#"
__derives__ = [Serialize]

@rust.derive("serde::Serialize")
pub trait Serialize:
  def to_yaml(self) -> str:
    return str("yaml")
"#;
    let source = r#"
import yaml
from yaml import Serialize as YamlSerialize

@derive(yaml)
pub type Token = newtype str

@derive(YamlSerialize)
pub type AliasToken = newtype str

def encode[T with yaml.Serialize](value: T) -> str:
  return value.to_yaml()

def main() -> str:
  return encode(Token("one"))
"#;

    let yaml_ast = parse_program(yaml_source, "yaml module");
    let ast = parse_program(source, "custom derived newtype consumer");
    let mut checker = TypeChecker::new();
    checker
        .check_with_imports(&ast, &[("yaml", &yaml_ast)])
        .map_err(|errors| std::io::Error::other(format!("custom newtype derive failed: {errors:?}")))?;
    let exports = collect_checked_public_exports(&ast, &checker);
    let manifest = LibraryManifest::from_checked_exports("custom_newtype", "0.1.0", &exports);
    let token = manifest
        .exports
        .newtypes
        .iter()
        .find(|newtype| newtype.name == "Token")
        .ok_or("missing Token export")?;
    assert!(
        token.trait_adoptions.iter().any(|adoption| {
            adoption.name == "yaml.Serialize"
                && adoption.source_name.as_deref() == Some("Serialize")
                && adoption.module_path.as_deref() == Some(&["yaml".to_string()])
        }),
        "expected identity- and owner-bearing yaml.Serialize adoption, got {:?}",
        token.trait_adoptions
    );
    let alias_token = manifest
        .exports
        .newtypes
        .iter()
        .find(|newtype| newtype.name == "AliasToken")
        .ok_or("missing AliasToken export")?;
    assert!(
        alias_token.trait_adoptions.iter().any(|adoption| {
            adoption.name == "YamlSerialize"
                && adoption.source_name.as_deref() == Some("Serialize")
                && adoption.module_path.as_deref() == Some(&["yaml".to_string()])
        }),
        "expected directly imported YamlSerialize adoption to retain its canonical owner, got {:?}",
        alias_token.trait_adoptions
    );
    Ok(())
}

#[test]
fn test_imported_newtype_custom_derive_spelling_does_not_satisfy_unrelated_consumer_trait()
-> Result<(), Box<dyn std::error::Error>> {
    let producer = r#"
pub type Token = newtype str
"#;
    let ast = parse_program(producer, "custom derive spelling provider");
    let mut checker = TypeChecker::new();
    checker
        .check_program(&ast)
        .map_err(|errors| std::io::Error::other(format!("provider typecheck failed: {errors:?}")))?;
    let exports = collect_checked_public_exports(&ast, &checker);
    let mut manifest = LibraryManifest::from_checked_exports("custom_derive_provider", "0.1.0", &exports);
    let token = manifest.exports.newtypes.first_mut().ok_or("missing Token export")?;
    token.derives = vec!["Capability".to_string()];
    token.trait_adoptions.clear();

    let index = LibraryManifestIndex::from_entries(HashMap::from([(
        "custom_derive_provider".to_string(),
        LibraryManifestIndexEntry::Loaded {
            manifest: Box::new(manifest),
            metadata: LibraryArtifactMetadata::from_crate_root(
                "custom_derive_provider",
                "custom_derive_provider",
                synthetic_artifact_root("custom_derive_provider"),
            ),
        },
    )]));
    let consumer = r#"
from pub::custom_derive_provider import Token

trait Capability:
  def marker(self) -> int: ...

def require_capability[T with Capability](value: T) -> T:
  return value

def main() -> Token:
  return require_capability(Token("one"))
"#;
    let errors = check_str_with_library_index_err(
        consumer,
        index,
        "a provider-local custom derive spelling must not satisfy an unrelated consumer trait",
    )?;
    assert!(
        errors
            .iter()
            .any(|error| { error.message.contains("violates generic bound") && error.message.contains("Capability") }),
        "expected unrelated Capability bound rejection, got: {errors:?}"
    );
    Ok(())
}

#[test]
fn test_public_nested_newtype_derives_satisfy_consumer_generic_bounds() -> Result<(), Box<dyn std::error::Error>> {
    let producer = r#"
@derive(Clone, Eq)
pub type BaseId = newtype str

@derive(Clone, Eq)
pub type ChildId = newtype BaseId
"#;
    let ast = parse_program(producer, "public derived nested newtype provider");
    let mut checker = TypeChecker::new();
    checker
        .check_program(&ast)
        .map_err(|errors| std::io::Error::other(format!("provider typecheck failed: {errors:?}")))?;
    let exports = collect_checked_public_exports(&ast, &checker);
    let manifest = LibraryManifest::from_checked_exports("derived_ids", "0.1.0", &exports);
    let temp = tempfile::tempdir()?;
    let manifest_path = temp.path().join("derived_ids.incnlib");
    manifest.write_to_path(&manifest_path)?;
    let manifest = LibraryManifest::read_from_path(&manifest_path)?;
    let child = manifest
        .exports
        .newtypes
        .iter()
        .find(|newtype| newtype.name == "ChildId")
        .ok_or("missing ChildId export")?;
    for trait_id in [TraitId::Clone, TraitId::Eq] {
        let trait_name = builtin_traits::as_str(trait_id);
        assert!(
            child.derives.iter().any(|derive| derive == trait_name),
            "expected exported ChildId derives to contain {trait_name}, got {:?}",
            child.derives
        );
    }

    let index = LibraryManifestIndex::from_entries(HashMap::from([(
        "derived_ids".to_string(),
        LibraryManifestIndexEntry::Loaded {
            manifest: Box::new(manifest),
            metadata: LibraryArtifactMetadata::from_crate_root(
                "derived_ids",
                "derived_ids",
                synthetic_artifact_root("derived_ids"),
            ),
        },
    )]));
    let consumer = r#"
from pub::derived_ids import BaseId, ChildId

def require_traits[T with (Clone, Eq)](value: T) -> T:
  return value

def same(left: ChildId, right: ChildId) -> bool:
  return left == right

def main() -> bool:
  identifier = require_traits(ChildId(BaseId("one")))
  return same(identifier, identifier)
"#;
    check_str_with_library_index(consumer, index)
        .map_err(|errors| std::io::Error::other(format!("consumer typecheck failed: {errors:?}")))?;
    Ok(())
}

#[test]
fn test_derived_traits_on_nested_newtype_satisfy_generic_bounds_and_equality() {
    let source = r#"
@derive(Clone, Eq)
type BaseId = newtype str

@derive(Clone, Eq)
type ChildId = newtype BaseId

def has_duplicates[T with (Clone, Eq)](values: list[T]) -> bool:
  for left in range(len(values)):
    for right in range(len(values)):
      if right > left and values[left] == values[right]:
        return true
  return false

def main() -> bool:
  first = ChildId(BaseId("one"))
  second = ChildId(BaseId("one"))
  identifiers = [first, second]
  return first == second and has_duplicates(identifiers)
"#;
    assert_check_ok(source);
}

/// A newtype carries the automatic `Clone` of its underlying type, as lowering derives it (#1754), but no other derive:
/// an undecorated newtype over an `Eq` type is not `Eq`.
#[test]
fn test_nested_newtype_inherits_only_the_automatic_derives_for_generic_bounds() {
    let source = r#"
@derive(Clone, Eq)
type BaseId = newtype str

type ChildId = newtype BaseId

def require_traits[T with (Clone, Eq)](value: T) -> T:
  return value

def main() -> ChildId:
  return require_traits(ChildId(BaseId("one")))
"#;
    let errors = check_str_err(
        source,
        "an undecorated nested newtype must not inherit its underlying type's Eq derive",
    );
    let violation = |trait_id: TraitId| {
        let trait_name = builtin_traits::as_str(trait_id);
        errors.iter().any(|error| {
            error.message.contains("violates generic bound")
                && error.message.contains(&format!("requires '{trait_name}'"))
        })
    };
    assert!(
        violation(TraitId::Eq),
        "expected missing Eq bound diagnostic, got: {errors:?}"
    );
    assert!(
        !violation(TraitId::Clone),
        "the automatic Clone must satisfy the Clone bound, got: {errors:?}"
    );
}

#[test]
fn test_nested_newtype_missing_one_derive_still_rejects_generic_bound() {
    let source = r#"
@derive(Clone, Eq)
type BaseId = newtype str

@derive(Clone)
type ChildId = newtype BaseId

def require_traits[T with (Clone, Eq)](value: T) -> T:
  return value

def main() -> ChildId:
  return require_traits(ChildId(BaseId("one")))
"#;
    let errors = check_str_err(source, "a nested newtype missing Eq must fail the Eq bound");
    let eq_name = builtin_traits::as_str(TraitId::Eq);
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("violates generic bound") && error.message.contains(eq_name)),
        "expected missing {eq_name} bound diagnostic, got: {errors:?}"
    );
    let clone_name = builtin_traits::as_str(TraitId::Clone);
    assert!(
        !errors
            .iter()
            .any(|error| error.message.contains("violates generic bound") && error.message.contains(clone_name)),
        "the explicitly derived {clone_name} capability must remain visible, got: {errors:?}"
    );
}

#[test]
fn test_explicit_trait_bound_accepts_transitive_supertrait_adopter() {
    let source = r#"
trait Capability:
  def capability(self) -> int: ...

trait Ordered with Capability:
  def ordered(self) -> int: ...

model Carrier with Ordered:
  value: int

  def capability(self) -> int:
    return self.value

  def ordered(self) -> int:
    return self.value

def require_capability[T with Capability](value: T) -> T:
  return value

def main() -> Carrier:
  return require_capability(Carrier(value=1))
"#;
    assert_check_ok(source);
}

#[test]
fn test_explicit_trait_bound_accepts_trait_typed_arguments() {
    let source = r#"
trait Capability:
  def capability(self) -> int: ...

trait Ordered with Capability:
  def ordered(self) -> int: ...

model Carrier with Ordered:
  value: int

  def capability(self) -> int:
    return self.value

  def ordered(self) -> int:
    return self.value

def as_ordered(value: Carrier) -> Ordered:
  return value

def require_capability[T with Capability](value: T) -> T:
  return value

def main() -> Ordered:
  ordered = as_ordered(Carrier(value=1))
  return require_capability(ordered)
"#;
    assert_check_ok(source);
}

#[test]
fn test_method_generic_bound_accepts_transitive_capability_adopter() {
    let source = r#"
trait Capability:
  def capability(self) -> int: ...

trait Ordered with Capability:
  def ordered(self) -> int: ...

model Carrier with Ordered:
  value: int

  def capability(self) -> int:
    return self.value

  def ordered(self) -> int:
    return self.value

class Helpers:
  @staticmethod
  def require_capability[T with Capability](value: T) -> T:
    return value

def main() -> Carrier:
  return Helpers.require_capability(Carrier(value=1))
"#;
    assert_check_ok(source);
}

#[test]
fn test_explicit_trait_bound_rejects_missing_capability() {
    let source = r#"
trait Capability:
  def capability(self) -> int: ...

trait Other:
  def other(self) -> int: ...

model Plain with Other:
  value: int

  def other(self) -> int:
    return self.value

def require_capability[T with Capability](value: T) -> T:
  return value

def main() -> Plain:
  return require_capability(Plain(value=1))
"#;
    let Err(errs) = check_str(source) else {
        panic!("missing capability should fail explicit trait bound");
    };
    assert!(
        errs.iter().any(|e| e.message.contains("violates generic bound")),
        "Expected explicit generic bound error; got: {:?}",
        errs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}
