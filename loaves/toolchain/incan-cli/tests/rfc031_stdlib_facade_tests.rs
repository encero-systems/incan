#![allow(dead_code)] // Split test binaries intentionally use different parts of shared support fragments.

//! RFC 031 pub-import integration tests.
//!
//! These regressions each drive a full `incan build` of a library plus one or more consumers, so they are the most
//! expensive cases in the suite. They live in their own libtest root because the Oven replay partitioner packs whole
//! roots: kept alongside the rest of the frontend integration tests they made one indivisible root that no split
//! could shorten.

include!("support/rfc031_pub_import_integration_tests_root.rs");

use std::process::Command;

use support::unique_test_project_name;

mod rfc031_pub_import_integration_tests {
    use incan_frontend::library_manifest::TypeRef;

    include!("support/rfc031_pub_import_integration_tests_rfc031_pub_import_integration_tests.rs");

    #[test]
    fn build_and_run_iterator_comprehension_and_if_let_scenarios() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"iterator_comprehension_if_let_batch\"\nversion = \"0.1.0\"\n",
            "def is_even(n: int) -> bool:\n  return n % 2 == 0\n\n\
def double(n: int) -> int:\n  return n * 2\n\n\
def maybe_double(opt: Option[int]) -> int:\n  if let Some(value) = opt:\n    return value * 2\n  return 0\n\n\
def next_value(values: list[Option[int]], idx: int) -> Option[int]:\n  if idx < len(values):\n    return values[idx]\n  return None\n\n\
def sum_values(values: list[Option[int]]) -> int:\n  mut idx = 0\n  mut total = 0\n  while let Some(value) = next_value(values, idx):\n    total = total + value\n    idx = idx + 1\n  return total\n\n\
def main() -> None:\n  xs = [1, 2, 3, 4, 5]\n  ys = xs.iter().filter(is_even).map(double).take(2).collect()\n  batches = xs.iter().batch(2).collect()\n  println(len(ys))\n  println(ys[0])\n  println(len(batches))\n  comp_source = [1, 2, 3]\n  comp = [n * 2 for n in comp_source if n > 1]\n  println(len(comp))\n  println(comp[0])\n  println(len(comp_source))\n  println(maybe_double(Some(21)))\n  println(maybe_double(None))\n  println(sum_values([Some(1), Some(2), None, Some(99)]))\n",
        )?;

        let out_dir = tmp.path().join("out");
        let build_output = run_build(&main_path, &out_dir)?;
        assert!(
            build_output.status.success(),
            "expected iterator/comprehension/if-let batch to build successfully.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&build_output.stdout),
            String::from_utf8_lossy(&build_output.stderr)
        );

        // The normal build above already proves the compiler route. Execute that exact Oven artifact for the
        // language-runtime assertions instead of recompiling the same source through `incan run`.
        let binary = out_dir.join("oven/release/iterator_comprehension_if_let_batch");
        assert!(
            binary.is_file(),
            "expected Oven to produce the iterator/comprehension executable at {}",
            binary.display()
        );
        let run_output = Command::new(&binary).output()?;
        assert!(
            run_output.status.success(),
            "expected built iterator/comprehension/if-let batch to run successfully.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&run_output.stdout),
            String::from_utf8_lossy(&run_output.stderr)
        );

        let stdout = String::from_utf8_lossy(&run_output.stdout);
        assert_eq!(
            stdout.lines().collect::<Vec<_>>(),
            vec!["2", "4", "3", "2", "4", "3", "42", "0", "3"]
        );

        Ok(())
    }

    #[test]
    fn build_lib_with_vocab_companion_embeds_vocab_payload() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let producer_root = tmp.path().join("widgets_vocab_project");
        std::fs::create_dir_all(producer_root.join("src"))?;
        std::fs::write(
            producer_root.join("loaf.toml"),
            "[project]\nname = \"widgets_vocab_core\"\nversion = \"0.1.0\"\n\n[vocab]\ncrate = \"vocab_companion\"\n",
        )?;
        std::fs::write(
            producer_root.join("src/lib.incn"),
            "pub def make_widget(name: str) -> str:\n  return name\n",
        )?;
        write_vocab_companion_crate(&producer_root, "vocab_companion", "widgets_vocab_companion")?;

        let producer_build = run_profiled_build_lib("incan build --lib vocabulary companion", &producer_root)?;
        assert!(
            producer_build.status.success(),
            "expected `build --lib` with vocab companion to succeed.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&producer_build.stdout),
            String::from_utf8_lossy(&producer_build.stderr)
        );
        assert_library_build_phase_keys(
            &producer_build,
            &[
                "library_codegen_emit_rust",
                "library_codegen_write_project",
                "library_codegen_sync_provider_dependencies",
                "library_oven_prepare_profiles",
                "library_oven_prepare_vocab_context",
                "library_generate_rust",
            ],
        )?;

        let manifest_path = producer_root
            .join("target")
            .join("lib")
            .join("widgets_vocab_core.incnlib");
        let manifest = LibraryManifest::read_from_path(&manifest_path)?;
        let vocab = manifest.vocab.as_ref().ok_or("expected vocab payload in .incnlib")?;
        assert_eq!(vocab.crate_path, "vocab_companion");
        assert_eq!(vocab.package_name, "widgets_vocab_companion");
        assert_eq!(vocab.keyword_registrations.len(), 1);
        assert_eq!(
            manifest.soft_keywords.activations,
            vec![incan_frontend::library_manifest::SoftKeywordActivation {
                namespace: "widgets.dsl".to_string(),
                keyword: "await".to_string(),
            }]
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn normal_oven_build_lib_with_vocab_companion_never_launches_cargo() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let producer_root = tmp.path().join("guarded_widgets_vocab_project");
        std::fs::create_dir_all(producer_root.join("src"))?;
        std::fs::write(
            producer_root.join("loaf.toml"),
            "[project]\nname = \"guarded_widgets_core\"\nversion = \"0.1.0\"\n\n[vocab]\ncrate = \"vocab_companion\"\n",
        )?;
        std::fs::write(
            producer_root.join("src/lib.incn"),
            "pub def make_widget(name: str) -> str:\n  return name\n",
        )?;
        write_vocab_companion_crate(&producer_root, "vocab_companion", "guarded_widgets_vocab_companion")?;
        let cargo_marker = tmp.path().join("cargo-was-started");

        let producer_build = run_incan_with_failing_cargo_guard(
            &producer_root,
            &tmp.path().join("cargo-guard"),
            &cargo_marker,
            &["build", "--lib"],
        )?;
        assert!(
            producer_build.status.success(),
            "expected normal Oven `build --lib` with vocab companion to succeed without Cargo.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&producer_build.stdout),
            String::from_utf8_lossy(&producer_build.stderr)
        );
        assert!(
            !cargo_marker.exists(),
            "normal Oven vocabulary extraction launched the guarded Cargo binary"
        );

        let manifest = LibraryManifest::read_from_path(&producer_root.join("target/lib/guarded_widgets_core.incnlib"))?;
        let vocab = manifest.vocab.as_ref().ok_or("expected vocab payload in .incnlib")?;
        assert_eq!(vocab.package_name, "guarded_widgets_vocab_companion");
        assert_eq!(vocab.keyword_registrations.len(), 1);
        Ok(())
    }

    #[test]
    fn build_lib_preserves_ordinal_map_metadata_for_consumer_check() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let producer_root = tmp.path().join("ordinal_keys_lib");
        std::fs::create_dir_all(producer_root.join("src"))?;
        std::fs::write(
            producer_root.join("loaf.toml"),
            "[project]\nname = \"ordinal_keys_core\"\nversion = \"0.1.0\"\n",
        )?;
        std::fs::write(
            producer_root.join("src/status.incn"),
            r#"import std.collections as collections
from std.collections import OrdinalKey as Key, OrdinalMap, OrdinalMapError

pub enum Status(str):
    Open = "open"
    Paid = "paid"
    Cancelled = "cancelled"


@derive(Clone, Eq)
pub trait StableKey with Key:
    def stable_marker(self) -> int: ...


@derive(Clone, Eq)
pub model SmallKey with StableKey:
    pub value: int

    @staticmethod
    def ordinal_encoding() -> str:
        return "ordinal-keys-core:small-key-v1"

    @staticmethod
    def from_ordinal_bytes(data: bytes) -> Result[Self, OrdinalMapError]:
        if len(data) != 1:
            return Err(OrdinalMapError.invalid_key_record("SmallKey requires one byte"))
        return Ok(SmallKey(value=int(data[0])))

    def ordinal_bytes(self) -> bytes:
        value: u8 = self.value.wrapping_resize()
        return [value]

    def ordinal_hash(self) -> int:
        return 10_000 + self.value

    def stable_marker(self) -> int:
        return self.value


pub def echo_key[T with Key](value: T) -> T:
    return value


pub def status_map_bytes() -> bytes:
    statuses: list[Status] = [Status.Open, Status.Paid, Status.Cancelled]
    match OrdinalMap.from_keys(statuses):
        Ok(columns) => return columns.to_bytes()
        Err(_) => return b""


pub def small_key_map_bytes() -> bytes:
    alpha = SmallKey(value=1)
    beta = SmallKey(value=2)
    gamma = SmallKey(value=3)
    match OrdinalMap.from_pairs([(alpha, 10), (beta, 20), (gamma, 30)]):
        Ok(columns) => return columns.to_bytes()
        Err(_) => return b""
"#,
        )?;
        std::fs::write(
            producer_root.join("src/lib.incn"),
            "pub from status import SmallKey, StableKey as PublicStableKey, Status, echo_key, small_key_map_bytes, status_map_bytes\n",
        )?;

        let producer_build = bake_library_provider(&producer_root)?;
        assert!(
            producer_build.status.success(),
            "expected `build --lib` to succeed.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&producer_build.stdout),
            String::from_utf8_lossy(&producer_build.stderr)
        );
        assert!(
            producer_root
                .join("target")
                .join("lib")
                .join("ordinal_keys_core.incnlib")
                .is_file()
        );
        let manifest = LibraryManifest::read_from_path(
            &producer_root
                .join("target")
                .join("lib")
                .join("ordinal_keys_core.incnlib"),
        )?;
        let stable_key = manifest
            .exports
            .traits
            .iter()
            .find(|trait_export| trait_export.name == "PublicStableKey")
            .ok_or("expected aliased StableKey export")?;
        assert_eq!(stable_key.source_name.as_deref(), Some("StableKey"));
        assert_eq!(stable_key.supertraits[0].name, "Key");
        assert_eq!(stable_key.supertraits[0].source_name.as_deref(), Some("OrdinalKey"));
        let status = manifest
            .exports
            .enums
            .iter()
            .find(|enum_export| enum_export.name == "Status")
            .ok_or("expected Status value enum export")?;
        assert_eq!(
            status.ordinal_type_identity.as_deref(),
            Some("ordinal_keys_core.Status")
        );

        let consumer_root = tmp.path().join("ordinal_keys_consumer");
        let consumer_name = unique_test_project_name("ordinal_keys_consumer");
        std::fs::create_dir_all(consumer_root.join("src"))?;
        std::fs::write(
            consumer_root.join("loaf.toml"),
            format!(
                "[project]\nname = \"{consumer_name}\"\n\n[dependencies]\nordinal_keys = {{ path = \"../ordinal_keys_lib\" }}\n"
            ),
        )?;
        let consumer_main = consumer_root.join("src/main.incn");
        std::fs::write(
            &consumer_main,
            "from std.collections import OrdinalMap, OrdinalMapError\nfrom pub::ordinal_keys import SmallKey, Status, echo_key, small_key_map_bytes, status_map_bytes\n\ndef run() -> Result[None, OrdinalMapError]:\n  probe = echo_key(\"probe\")\n  if len(probe) == 0:\n    print(probe)\n  status_map: OrdinalMap[Status] = OrdinalMap.from_bytes(status_map_bytes())?\n  small_key_map: OrdinalMap[SmallKey] = OrdinalMap.from_bytes(small_key_map_bytes())?\n  print(status_map.require(Status.Paid)?)\n  print(small_key_map.require(SmallKey(value=2))?)\n  return Ok(None)\n\ndef main() -> None:\n  match run():\n    Ok(_) => pass\n    Err(err) => print(err.message())\n",
        )?;

        let consumer_check = run_check(&consumer_main)?;
        assert!(
            consumer_check.status.success(),
            "expected consumer check to accept imported OrdinalMap metadata.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&consumer_check.stdout),
            String::from_utf8_lossy(&consumer_check.stderr)
        );
        Ok(())
    }

    #[test]
    fn build_lib_preserves_std_environ_typed_reads_for_consumers() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let producer_root = tmp.path().join("environ_types_provider");
        std::fs::create_dir_all(producer_root.join("src"))?;
        std::fs::write(
            producer_root.join("loaf.toml"),
            "[project]\nname = \"environ_types_core\"\nversion = \"0.1.0\"\n",
        )?;
        std::fs::write(
            producer_root.join("src/types.incn"),
            r#"from std.traits.convert import TryFrom

pub model EnvToken with TryFrom[str]:
  pub value: str

  @classmethod
  def try_from(cls, value: str) -> Result[Self, str]:
    if len(value) == 0:
      return Err("token must not be empty")
    return Ok(EnvToken(value=value))

pub model MultiToken with TryFrom[str], TryFrom[int]:
  pub value: str

  @classmethod
  def try_from(cls, value: str) for TryFrom[str] -> Result[Self, str]:
    if len(value) == 0:
      return Err("token must not be empty")
    return Ok(MultiToken(value=value))

  @classmethod
  def try_from(cls, value: int) for TryFrom[int] -> Result[Self, str]:
    return Ok(MultiToken(value=f"{value}"))

pub trait EnvReadable[T] with TryFrom[T]:
  def source_name(self) -> str: ...

pub model TraitToken with EnvReadable[str]:
  pub value: str

  @classmethod
  def try_from(cls, value: str) -> Result[Self, str]:
    return Ok(TraitToken(value=value))

  def source_name(self) -> str:
    return "environment"

pub class EnvClass with TryFrom[str]:
  pub value: str

  @classmethod
  def try_from(cls, value: str) -> Result[Self, str]:
    return Ok(EnvClass(value=value))

pub enum EnvMode with TryFrom[str]:
  Dev
  Prod

  @classmethod
  def try_from(cls, value: str) -> Result[Self, str]:
    if value == "prod":
      return Ok(EnvMode.Prod)
    return Ok(EnvMode.Dev)

pub type ExplicitLabel = newtype str with TryFrom[str]:
  @classmethod
  def try_from(cls, value: str) -> Result[Self, str]:
    return Ok(ExplicitLabel(value))

pub type Port = newtype int:
  def from_underlying(value: int) -> Result[Self, ValidationError]:
    if value < 1 or value > 65535:
      return Err(ValidationError("port out of range"))
    return Ok(Port(value))

pub type WrappedPort = newtype Port
pub type Positive = newtype int[gt=0]
pub type PositiveBox = newtype Positive
pub type Ratio = newtype float[ge=0, le=1]
pub type Boxed[T] = newtype T
pub type ClonedBox[T with Clone] = newtype T

@no_implicit_coercion
pub type StrictPort = newtype int
"#,
        )?;
        std::fs::write(
            producer_root.join("src/lib.incn"),
            "pub from types import Boxed as PublicBoxed, ClonedBox, EnvClass, EnvMode, EnvReadable, EnvToken, ExplicitLabel, MultiToken, Port, Port as PublicPort, Positive as PublicPositive, PositiveBox as PublicPositiveBox, Ratio, StrictPort, TraitToken, WrappedPort\n",
        )?;

        let producer_build = bake_library_provider(&producer_root)?;
        assert!(
            producer_build.status.success(),
            "expected std.environ type provider to build.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&producer_build.stdout),
            String::from_utf8_lossy(&producer_build.stderr)
        );

        let provider_manifest = LibraryManifest::read_from_path(
            &producer_root
                .join("target")
                .join("lib")
                .join("environ_types_core.incnlib"),
        )?;
        let positive = provider_manifest
            .exports
            .newtypes
            .iter()
            .find(|newtype| newtype.name == "PublicPositive")
            .ok_or("missing aliased constrained newtype export")?;
        assert_eq!(positive.constraints.len(), 1);
        assert_eq!(positive.constraints[0].value, 0);
        let positive_box = provider_manifest
            .exports
            .newtypes
            .iter()
            .find(|newtype| newtype.name == "PublicPositiveBox")
            .ok_or("missing aliased composed newtype export")?;
        assert_eq!(
            positive_box.underlying,
            TypeRef::Named {
                origin: None,
                name: "PublicPositive".to_string()
            }
        );
        let boxed = provider_manifest
            .exports
            .newtypes
            .iter()
            .find(|newtype| newtype.name == "PublicBoxed")
            .ok_or("missing aliased generic newtype export")?;
        assert_eq!(boxed.type_params.len(), 1);
        let strict = provider_manifest
            .exports
            .newtypes
            .iter()
            .find(|newtype| newtype.name == "StrictPort")
            .ok_or("missing no-implicit-coercion newtype export")?;
        assert!(!strict.implicit_coercion_enabled);

        let consumer_root = tmp.path().join("environ_types_consumer");
        std::fs::create_dir_all(consumer_root.join("src"))?;
        std::fs::write(
            consumer_root.join("loaf.toml"),
            "[project]\nname = \"environ_types_consumer\"\n\n[dependencies]\nenviron_types = { path = \"../environ_types_provider\" }\n",
        )?;
        let consumer_main = consumer_root.join("src/main.incn");
        std::fs::write(
            &consumer_main,
            r#"from std.environ import EnvironError, get_as
from std.traits.convert import TryFrom
from pub::environ_types import ClonedBox, EnvClass, EnvMode, EnvToken, ExplicitLabel, MultiToken, Port, PublicBoxed, PublicPort, PublicPositive, PublicPositiveBox, Ratio, StrictPort, TraitToken, WrappedPort

def require[T with TryFrom[str]](key: str) -> Result[T, EnvironError]:
  match get_as[T](key)?:
    Some(value) => return Ok(value)
    None => return Err(EnvironError.missing(key))

def mode_name(mode: EnvMode) -> str:
  match mode:
    EnvMode.Dev => return "Dev"
    EnvMode.Prod => return "Prod"

def read_values() -> Result[None, EnvironError]:
  token = require[EnvToken]("INCAN_PACKAGE_ENV_TOKEN")?
  trait_token = require[TraitToken]("INCAN_PACKAGE_ENV_TRAIT_TOKEN")?
  env_class = require[EnvClass]("INCAN_PACKAGE_ENV_CLASS")?
  env_mode = require[EnvMode]("INCAN_PACKAGE_ENV_MODE")?
  explicit_label = require[ExplicitLabel]("INCAN_PACKAGE_ENV_EXPLICIT_LABEL")?
  port = require[Port]("INCAN_PACKAGE_ENV_PORT")?
  public_port = require[PublicPort]("INCAN_PACKAGE_ENV_PUBLIC_PORT")?
  wrapped = require[WrappedPort]("INCAN_PACKAGE_ENV_WRAPPED")?
  multi = require[MultiToken]("INCAN_PACKAGE_ENV_MULTI")?
  positive = require[PublicPositive]("INCAN_PACKAGE_ENV_POSITIVE")?
  positive_box = require[PublicPositiveBox]("INCAN_PACKAGE_ENV_POSITIVE_BOX")?
  boxed = require[PublicBoxed[int]]("INCAN_PACKAGE_ENV_BOXED")?
  cloned = require[ClonedBox[str]]("INCAN_PACKAGE_ENV_CLONED")?
  ratio = require[Ratio]("INCAN_PACKAGE_ENV_RATIO")?
  strict = require[StrictPort]("INCAN_PACKAGE_ENV_STRICT")?
  unwrapped = wrapped.0
  positive_inner = positive_box.0
  defaulted = get_as[Port]("INCAN_PACKAGE_ENV_MISSING", default=8080)?
  println(f"{token.value}:{trait_token.value}:{env_class.value}:{mode_name(env_mode)}:{explicit_label.0}:{multi.value}:{port.0}:{public_port.0}:{unwrapped.0}:{positive.0}:{positive_inner.0}:{boxed.0}:{cloned.0}:{ratio.0}:{strict.0}:{defaulted.0}")
  return Ok(None)

def main() -> None:
  match read_values():
    Ok(_) => pass
    Err(error) => println(f"unexpected:{error.kind_name()}")
  match get_as[Port]("INCAN_PACKAGE_ENV_BAD_PORT"):
    Ok(_) => println("invalid:unexpected")
    Err(error) => println(f"invalid:{error.kind_name()}")
  match get_as[PublicPositive]("INCAN_PACKAGE_ENV_NON_POSITIVE"):
    Ok(_) => println("constrained:unexpected")
    Err(error) => println(f"constrained:{error.kind_name()}")
  match get_as[Ratio]("INCAN_PACKAGE_ENV_RATIO_HIGH"):
    Ok(_) => println("ratio-high:unexpected")
    Err(error) => println(f"ratio-high:{error.kind_name()}")
"#,
        )?;

        let consumer_bake = bake_project(&consumer_root)?;
        assert!(
            consumer_bake.status.success(),
            "expected an explicit Oven bake to import the typed-environment package closure.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&consumer_bake.stdout),
            String::from_utf8_lossy(&consumer_bake.stderr)
        );
        let consumer_run = super::incan_command()
            .args(["run", consumer_main.to_string_lossy().as_ref()])
            .env("CARGO_NET_OFFLINE", "true")
            .env("INCAN_PACKAGE_ENV_TOKEN", "secret-token")
            .env("INCAN_PACKAGE_ENV_TRAIT_TOKEN", "trait-token")
            .env("INCAN_PACKAGE_ENV_CLASS", "class-token")
            .env("INCAN_PACKAGE_ENV_MODE", "prod")
            .env("INCAN_PACKAGE_ENV_EXPLICIT_LABEL", "explicit")
            .env("INCAN_PACKAGE_ENV_PORT", "5432")
            .env("INCAN_PACKAGE_ENV_PUBLIC_PORT", "5433")
            .env("INCAN_PACKAGE_ENV_WRAPPED", "6543")
            .env("INCAN_PACKAGE_ENV_MULTI", "multi-token")
            .env("INCAN_PACKAGE_ENV_POSITIVE", "12")
            .env("INCAN_PACKAGE_ENV_POSITIVE_BOX", "13")
            .env("INCAN_PACKAGE_ENV_BOXED", "88")
            .env("INCAN_PACKAGE_ENV_CLONED", "cloned")
            .env("INCAN_PACKAGE_ENV_RATIO", "0.25")
            .env("INCAN_PACKAGE_ENV_STRICT", "9090")
            .env("INCAN_PACKAGE_ENV_BAD_PORT", "70000")
            .env("INCAN_PACKAGE_ENV_NON_POSITIVE", "0")
            .env("INCAN_PACKAGE_ENV_RATIO_HIGH", "1.5")
            .env_remove("INCAN_PACKAGE_ENV_MISSING")
            .output()?;
        assert!(
            consumer_run.status.success(),
            "expected package typed environment reads to run.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&consumer_run.stdout),
            String::from_utf8_lossy(&consumer_run.stderr)
        );
        assert_eq!(
            String::from_utf8(consumer_run.stdout)?,
            concat!(
                "secret-token:trait-token:class-token:Prod:explicit:multi-token:5432:5433:6543:12:13:88:cloned:0.25:9090:8080\n",
                "invalid:invalid_value\n",
                "constrained:invalid_value\n",
                "ratio-high:invalid_value\n",
            )
        );

        let strict_default = consumer_root.join("src/strict_default.incn");
        std::fs::write(
            &strict_default,
            r#"from std.environ import get_as
from pub::environ_types import StrictPort

def main() -> None:
  get_as[StrictPort]("INCAN_PACKAGE_ENV_STRICT_DEFAULT", 8080)
"#,
        )?;
        let strict_check = run_check(&strict_default)?;
        assert!(
            !strict_check.status.success(),
            "expected package no-implicit-coercion policy to reject underlying default"
        );
        assert!(
            String::from_utf8_lossy(&strict_check.stderr)
                .contains("Implicit coercion into newtype 'StrictPort' is disabled"),
            "expected package coercion diagnostic, got:\n{}",
            String::from_utf8_lossy(&strict_check.stderr)
        );
        Ok(())
    }

    #[test]
    fn std_environ_typed_reads_survive_facades_and_test_batches() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path();
        let project_name = unique_test_project_name("environ_facade_test_batch");
        std::fs::create_dir_all(project_root.join("src"))?;
        std::fs::create_dir_all(project_root.join("tests"))?;
        std::fs::write(
            project_root.join("loaf.toml"),
            format!("[project]\nname = \"{project_name}\"\nversion = \"0.1.0\"\n"),
        )?;
        std::fs::write(
            project_root.join("src/env_types.incn"),
            r#"pub type Port = newtype int:
  def from_underlying(value: int) -> Result[Self, ValidationError]:
    if value < 1 or value > 65535:
      return Err(ValidationError("port out of range"))
    return Ok(Port(value))
"#,
        )?;
        std::fs::write(
            project_root.join("src/environ_facade.incn"),
            "pub from env_types import Port\n",
        )?;
        let main_path = project_root.join("src/main.incn");
        std::fs::write(
            &main_path,
            r#"from environ_facade import Port
from std.environ import get_as

def main() -> None:
  match get_as[Port]("INCAN_ENVIRON_FACADE_MISSING", default=8080):
    Ok(port) => println(port.0)
    Err(error) => println(error.kind_name())
"#,
        )?;
        std::fs::write(
            project_root.join("tests/test_environ.incn"),
            r#"from environ_facade import Port
from std.environ import get_as
from std.testing import assert_eq, fail

def test_defaulted_port_through_facade() -> None:
  match get_as[Port]("INCAN_ENVIRON_TEST_BATCH_MISSING", 9090):
    Ok(port) => assert_eq(port.0, 9090)
    Err(error) => fail(f"expected defaulted Port, got {error.kind_name()}")
  match get_as[Port]("INCAN_ENVIRON_TEST_BATCH_INVALID"):
    Ok(_) => fail("expected invalid facade Port to fail validation")
    Err(error) => assert_eq(error.kind_name(), "invalid_value")
"#,
        )?;

        let run_output = super::incan_command()
            .args(["run", main_path.to_string_lossy().as_ref()])
            .env("CARGO_NET_OFFLINE", "true")
            .env_remove("INCAN_ENVIRON_FACADE_MISSING")
            .output()?;
        assert!(
            run_output.status.success(),
            "expected facade typed environment read to run.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&run_output.stdout),
            String::from_utf8_lossy(&run_output.stderr)
        );
        assert_eq!(String::from_utf8(run_output.stdout)?, "8080\n");

        let test_output = super::incan_command()
            .args(["test", project_root.join("tests").to_string_lossy().as_ref()])
            .env("CARGO_NET_OFFLINE", "true")
            .env("INCAN_TEST_SHARED_TARGET_DIR", shared_test_runner_target_dir())
            .env("INCAN_ENVIRON_TEST_BATCH_INVALID", "70000")
            .env_remove("INCAN_ENVIRON_TEST_BATCH_MISSING")
            .output()?;
        assert!(
            test_output.status.success(),
            "expected test batch typed environment read to pass.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&test_output.stdout),
            String::from_utf8_lossy(&test_output.stderr)
        );
        Ok(())
    }

    #[test]
    fn std_environ_module_qualified_overloads_run() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path();
        let project_name = unique_test_project_name("environ_module_qualified");
        std::fs::create_dir_all(project_root.join("src"))?;
        std::fs::write(
            project_root.join("loaf.toml"),
            format!("[project]\nname = \"{project_name}\"\nversion = \"0.1.0\"\n"),
        )?;
        let main_path = project_root.join("src/main.incn");
        std::fs::write(
            &main_path,
            r#"import std.environ as environ
from std.environ import EnvironError


def read_values() -> Result[None, EnvironError]:
  present = environ.get_as[int]("INCAN_ENVIRON_QUALIFIED_PRESENT")?.unwrap_or(0)
  fallback = environ.get_as[int]("INCAN_ENVIRON_QUALIFIED_MISSING", default=8080)?
  println(f"{present}:{fallback}")
  return Ok(None)


def main() -> None:
  match read_values():
    Ok(_) => pass
    Err(error) => println(error.kind_name())
"#,
        )?;

        let output = super::incan_command()
            .args(["run", main_path.to_string_lossy().as_ref()])
            .env("CARGO_NET_OFFLINE", "true")
            .env("INCAN_ENVIRON_QUALIFIED_PRESENT", "42")
            .env_remove("INCAN_ENVIRON_QUALIFIED_MISSING")
            .output()?;
        assert!(
            output.status.success(),
            "expected module-qualified std.environ overloads to run.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(String::from_utf8(output.stdout)?, "42:8080\n");
        Ok(())
    }

    #[test]
    fn std_environ_overloads_run_through_source_facade() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path();
        let project_name = unique_test_project_name("environ_function_facade");
        std::fs::create_dir_all(project_root.join("src"))?;
        std::fs::write(
            project_root.join("loaf.toml"),
            format!("[project]\nname = \"{project_name}\"\nversion = \"0.1.0\"\n"),
        )?;
        std::fs::write(
            project_root.join("src/environ_facade.incn"),
            "pub from std.environ import EnvironError, get_as\n",
        )?;
        let main_path = project_root.join("src/main.incn");
        std::fs::write(
            &main_path,
            r#"from environ_facade import EnvironError, get_as


def read_values() -> Result[None, EnvironError]:
  present = get_as[int]("INCAN_ENVIRON_FACADE_PRESENT")?.unwrap_or(0)
  fallback = get_as[int]("INCAN_ENVIRON_FACADE_MISSING", 9090)?
  println(f"{present}:{fallback}")
  return Ok(None)


def main() -> None:
  match read_values():
    Ok(_) => pass
    Err(error) => println(error.kind_name())
"#,
        )?;

        let output = super::incan_command()
            .args(["run", main_path.to_string_lossy().as_ref()])
            .env("CARGO_NET_OFFLINE", "true")
            .env("INCAN_ENVIRON_FACADE_PRESENT", "73")
            .env_remove("INCAN_ENVIRON_FACADE_MISSING")
            .output()?;
        assert!(
            output.status.success(),
            "expected std.environ overloads re-exported by a source facade to run.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(String::from_utf8(output.stdout)?, "73:9090\n");
        Ok(())
    }

    /// A compiled SDK function re-exported by a source facade keeps its omitted defaults through native emission.
    ///
    /// Issue #1435: `std.regex.compile` declares four defaulted flags. Checking accepted `compile(pattern)` through
    /// the facade while emission passed only the pattern, so rustc rejected the generated call with E0061. The
    /// direct `from std.regex import compile` spelling filled the defaults all along; the facade must bind the same
    /// provider declaration.
    #[test]
    fn std_function_defaults_survive_source_facade_issue1435() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path();
        let project_name = unique_test_project_name("regex_default_facade");
        std::fs::create_dir_all(project_root.join("src"))?;
        std::fs::write(
            project_root.join("loaf.toml"),
            format!("[project]\nname = \"{project_name}\"\nversion = \"0.1.0\"\n"),
        )?;
        std::fs::write(
            project_root.join("src/codec.incn"),
            "pub from std.regex import compile\n",
        )?;
        let main_path = project_root.join("src/main.incn");
        std::fs::write(
            &main_path,
            r#"from codec import compile
from std.regex import RegexError


def run() -> Result[None, RegexError]:
  strict = compile("ab+c")?
  relaxed = compile("AB+C", ignore_case=true)?
  exact = strict.is_match("xabbcx")
  upper = strict.is_match("xABBCx")
  folded = relaxed.is_match("xabbcx")
  println(f"{exact}:{upper}:{folded}")
  return Ok(None)


def main() -> None:
  match run():
    Ok(_) => pass
    Err(error) => println(error.message())
"#,
        )?;

        let output = super::incan_command()
            .args(["run", main_path.to_string_lossy().as_ref()])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "expected a compiled SDK function with omitted defaults to run through a source facade.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(String::from_utf8(output.stdout)?, "true:false:true\n");
        Ok(())
    }

    /// A `std.serde.json` trait re-exported by a source facade keeps its JSON protocol under a compiled SDK provider.
    ///
    /// Issue #1431 keyed the protocol on the trait's canonical identity, and issue #1435 made a facade of a compiled
    /// `std.*` member bind the provider's declaration, whose identity is package-owned rather than module-owned.
    /// Lowering must read that identity back to the `std.serde.json` declaration; otherwise the facade-bound
    /// `Serialize` loses its backend `to_json` and the adopter's `from_json` is dropped from the `Deserialize` impl.
    #[test]
    fn std_serde_json_traits_keep_their_protocol_through_source_facade_issue1431()
    -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path();
        let project_name = unique_test_project_name("serde_trait_facade");
        std::fs::create_dir_all(project_root.join("src"))?;
        std::fs::write(
            project_root.join("loaf.toml"),
            format!("[project]\nname = \"{project_name}\"\nversion = \"0.1.0\"\n"),
        )?;
        std::fs::write(
            project_root.join("src/codec.incn"),
            "pub from std.serde.json import Serialize, Deserialize\n",
        )?;
        let main_path = project_root.join("src/main.incn");
        std::fs::write(
            &main_path,
            r#"from codec import Serialize, Deserialize


model Payload with Serialize, Deserialize:
  value: int

  def from_json(json_str: str) -> Result[Payload, str]:
    return Ok(Payload(value=len(json_str)))


def main() -> None:
  println(Payload(value=1).to_json())
  match Payload.from_json("{}"):
    case Ok(restored):
      println(restored.value)
    case Err(message):
      println(message)
"#,
        )?;

        let output = super::incan_command()
            .args(["run", main_path.to_string_lossy().as_ref()])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "expected the std.serde.json traits re-exported by a source facade to keep their JSON protocol.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(String::from_utf8(output.stdout)?, "{\"value\":1}\n2\n");
        Ok(())
    }

    #[test]
    fn check_pub_boundary_preserves_consumer_type_fidelity_cases() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        write_pub_boundary_type_fidelity_library(tmp.path())?;

        let cases = [
            (
                "question_mark_result",
                "`lazy.collect()?` across pub boundary",
                r#"from pub::pubdemo import LazyFrame, SessionError

model Row:
  value: int

def main() -> Result[None, SessionError]:
  lazy = LazyFrame[Row](_type_witness=[])
  df = lazy.collect()?
  print(df.to_substrait_plan())
  return Ok(None)
"#,
            ),
            (
                "derived_method_chain",
                "`lazy.clone().collect()?` across pub boundary",
                r#"from pub::pubdemo import LazyFrame, SessionError

model Row:
  value: int

def main() -> Result[None, SessionError]:
  lazy = LazyFrame[Row](_type_witness=[])
  df = lazy.clone().collect()?
  print(df.to_substrait_plan())
  return Ok(None)
"#,
            ),
            (
                "trait_supertype",
                "`DataFrame[T]` satisfying `DataSet[T]` across pub boundary",
                r#"from pub::pubdemo import DataFrame, SessionError, display

model Row:
  value: int

def main() -> Result[None, SessionError]:
  df = DataFrame[Row](_type_witness=[])
  display(df)
  return Ok(None)
"#,
            ),
        ];

        for (name, description, source) in cases {
            let case_root = tmp.path().join(name);
            let main_path = write_project_files(
                &case_root,
                "[project]\nname = \"consumer\"\n\n[dependencies]\npubdemo = { path = \"../pub_boundary_library\" }\n",
                source,
            )?;

            let output = run_check(&main_path)?;
            assert!(
                output.status.success(),
                "expected {description} to typecheck.\nstdout:\n{}\nstderr:\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        Ok(())
    }
}
