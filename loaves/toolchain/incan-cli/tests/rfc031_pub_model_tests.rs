#![allow(dead_code)] // Split test binaries intentionally use different parts of shared support fragments.

//! RFC 031 pub-import integration tests.
//!
//! These regressions each drive a full `incan build` of a library plus one or more consumers, so they are the most
//! expensive cases in the suite. They live in their own libtest root because the Oven replay partitioner packs whole
//! roots: kept alongside the rest of the frontend integration tests they made one indivisible root that no split
//! could shorten.

include!("support/rfc031_pub_import_integration_tests_root.rs");

use support::strip_ansi_escapes;

mod rfc031_pub_import_integration_tests {
    use incan_frontend::library_manifest::FieldVisibilityExport;

    include!("support/rfc031_pub_import_integration_tests_rfc031_pub_import_integration_tests.rs");

    #[test]
    fn compiled_library_preserves_public_computed_property_contract_issue952() -> Result<(), Box<dyn std::error::Error>>
    {
        let tmp = tempfile::tempdir()?;
        let source_root = support::repo_root();
        let stdlib = source_root.join("loaves/stdlib");
        let producer_root = tmp.path().join("computed_property_provider");
        std::fs::create_dir_all(producer_root.join("src"))?;
        std::fs::write(
            producer_root.join("loaf.toml"),
            "[project]\nname = \"computed_property_provider\"\nversion = \"0.1.0\"\n",
        )?;
        std::fs::write(
            producer_root.join("src/lib.incn"),
            "pub class Index:\n  value: int\n\n  pub property dimensions -> int:\n    return self.value\n\npub def make_index() -> Index:\n  return Index(value=2)\n",
        )?;

        let producer_build = super::incan_command()
            .args(["build", "--lib"])
            .current_dir(&producer_root)
            .env("CARGO_NET_OFFLINE", "true")
            .env("INCAN_SOURCE_ROOT", &source_root)
            .env("INCAN_STDLIB", &stdlib)
            .env_remove("INCAN_STDLIB_DIR")
            .output()?;
        assert!(
            producer_build.status.success(),
            "expected producer library build to succeed.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&producer_build.stdout),
            String::from_utf8_lossy(&producer_build.stderr)
        );

        let consumer_root = tmp.path().join("consumer_app");
        std::fs::create_dir_all(consumer_root.join("src"))?;
        std::fs::write(
            consumer_root.join("loaf.toml"),
            "[project]\nname = \"consumer\"\n\n[dependencies]\ncomputed_property_provider = { path = \"../computed_property_provider\" }\n",
        )?;
        let consumer_main = consumer_root.join("src/main.incn");
        std::fs::write(
            &consumer_main,
            "from pub::computed_property_provider import make_index\n\ndef main() -> None:\n  index = make_index()\n  println(index.dimensions)\n",
        )?;

        let consumer_check = super::incan_command()
            .arg("--check")
            .arg(&consumer_main)
            .env("CARGO_NET_OFFLINE", "true")
            .env("INCAN_SOURCE_ROOT", &source_root)
            .env("INCAN_STDLIB", &stdlib)
            .env_remove("INCAN_STDLIB_DIR")
            .output()?;
        assert!(
            consumer_check.status.success(),
            "expected a compiled-library consumer to access a public computed property.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&consumer_check.stdout),
            String::from_utf8_lossy(&consumer_check.stderr)
        );
        Ok(())
    }

    #[test]
    fn build_lib_publishes_checked_modules_and_split_aliases_issues948_892() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let producer_root = tmp.path().join("modulelib");
        std::fs::create_dir_all(producer_root.join("src/hyperquant"))?;
        std::fs::write(
            producer_root.join("loaf.toml"),
            "[project]\nname = \"modulelib\"\nversion = \"0.1.0\"\n",
        )?;
        std::fs::write(
            producer_root.join("src/lib.incn"),
            "\"\"\"A module-oriented library.\"\"\"\n\npub from proposals import ConsoleProposal, make_console_proposal\n",
        )?;
        std::fs::write(
            producer_root.join("src/proposals.incn"),
            r#"pub model ConsoleProposal:
  pub title: str

pub def make_console_proposal(title: str) -> ConsoleProposal:
  return ConsoleProposal(title=title)
"#,
        )?;
        std::fs::write(
            producer_root.join("src/hyperquant/mod.incn"),
            "pub def namespace_version() -> int:\n  return 1\n",
        )?;
        std::fs::write(
            producer_root.join("src/hyperquant/index.incn"),
            r#"pub model HyperquantIndex:
  pub size: int

  def doubled(self) -> int:
    return self.size * 2

pub class IndexBuilder:
  pub size: int = 3

  def build(self) -> HyperquantIndex:
    return HyperquantIndex(size=self.size)

pub enum IndexMode:
  Dense
  Sparse

pub type IndexSize = newtype int
pub type IndexList = list[HyperquantIndex]

pub const DEFAULT_SIZE: int = 4

pub def build_index(size: int = DEFAULT_SIZE) -> HyperquantIndex:
  return HyperquantIndex(size=size)

pub default_index = partial build_index(size=DEFAULT_SIZE)

def pack_bits(size: int) -> int:
  return size
"#,
        )?;
        std::fs::write(
            producer_root.join("src/hyperquant/search.incn"),
            r#"from hyperquant.index import HyperquantIndex

pub def search(index: HyperquantIndex) -> int:
  return index.size * 2
"#,
        )?;
        std::fs::write(
            producer_root.join("src/hyperquant/facade.incn"),
            "pub from hyperquant.index import HyperquantIndex as PublicIndex, build_index as make_index\n",
        )?;

        let producer_build = bake_library_provider(&producer_root)?;
        assert!(
            producer_build.status.success(),
            "expected an unplumbed source directory to publish as a checked namespace.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&producer_build.stdout),
            String::from_utf8_lossy(&producer_build.stderr)
        );
        assert!(
            producer_root.join("target/lib/modulelib.incnlib").is_file(),
            "expected the combined provider's compiled .incnlib manifest"
        );
        let generated_namespace = std::fs::read_to_string(producer_root.join("target/lib/src/hyperquant/mod.rs"))?;
        assert!(
            generated_namespace.contains("pub use facade::*;")
                && generated_namespace.contains("pub use index::*;")
                && generated_namespace.contains("pub use search::*;"),
            "expected the generated package namespace to re-export its immediate checked source modules.\n\
             generated hyperquant/mod.rs:\n{generated_namespace}"
        );

        let consumer_root = tmp.path().join("consumer");
        std::fs::create_dir_all(consumer_root.join("src"))?;
        std::fs::write(
            consumer_root.join("loaf.toml"),
            "[project]\nname = \"modulelib_consumer\"\n\n[dependencies]\nmodulelib = { path = \"../modulelib\" }\n",
        )?;
        let main_path = consumer_root.join("src/main.incn");
        std::fs::write(
            &main_path,
            r#"from pub::modulelib import hyperquant
from pub::modulelib import ConsoleProposal as ProviderConsoleProposal, make_console_proposal
from pub::modulelib.hyperquant.facade import PublicIndex as FacadeIndex, make_index
from pub::modulelib.hyperquant import HyperquantIndex as PublicIndex, IndexBuilder, IndexList, IndexMode, IndexSize, search as nested_search

def proposal() -> ProviderConsoleProposal:
  return make_console_proposal("ready")

def main() -> None:
  index: PublicIndex = PublicIndex(size=hyperquant.DEFAULT_SIZE)
  facade_index: FacadeIndex = make_index()
  default_index: PublicIndex = hyperquant.default_index()
  builder = IndexBuilder()
  built: PublicIndex = builder.build()
  indexes: IndexList = [index, facade_index, default_index, built]
  size = IndexSize(index.doubled())
  mode = IndexMode.Dense
  println(nested_search(indexes[0]) + size.0 + hyperquant.namespace_version())
  println(nested_search(facade_index))
  println(nested_search(default_index))
  println(proposal().title)
  match mode:
    IndexMode.Dense => println("dense")
    IndexMode.Sparse => println("sparse")
"#,
        )?;

        let consumer_bake = bake_project(&consumer_root)?;
        assert!(
            consumer_bake.status.success(),
            "expected an explicit Oven bake to import the modulelib package closure.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&consumer_bake.stdout),
            String::from_utf8_lossy(&consumer_bake.stderr)
        );
        let out_dir = consumer_root.join("out");
        let consumer_build = run_build(&main_path, &out_dir)?;
        assert!(
            consumer_build.status.success(),
            "expected generated Rust for public module imports to compile.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&consumer_build.stdout),
            String::from_utf8_lossy(&consumer_build.stderr)
        );
        let generated_main = std::fs::read_to_string(out_dir.join("src/main.rs"))?;
        assert!(
            generated_main.contains("modulelib::ConsoleProposal"),
            "expected generated Rust to retain the combined provider-qualified proposal path.\ngenerated main.rs:\n{generated_main}"
        );
        std::fs::create_dir_all(consumer_root.join("tests"))?;
        std::fs::write(
            consumer_root.join("tests/test_public_module.incn"),
            r#"import pub::modulelib.hyperquant as h

def test_public_module_namespace() -> None:
  index = h.HyperquantIndex(size=h.DEFAULT_SIZE)
  builder = h.IndexBuilder()
  mode = h.IndexMode.Dense
  size = h.IndexSize(index.doubled())
  assert h.search(index) == 8
  assert builder.build().size == 3
  assert size.0 == 8
  assert mode == h.IndexMode.Dense
"#,
        )?;
        std::fs::write(
            consumer_root.join("tests/test_split_alias.incn"),
            r#"from pub::modulelib import ConsoleProposal as SplitConsoleProposal
from pub::modulelib import make_console_proposal

def make_split() -> SplitConsoleProposal:
  return make_console_proposal("split")

def test_split_pub_import_alias() -> None:
  proposal: SplitConsoleProposal = make_split()
  assert proposal.title == "split"
"#,
        )?;
        std::fs::write(
            consumer_root.join("tests/test_same_statement_alias.incn"),
            r#"from pub::modulelib import ConsoleProposal as SameStatementConsoleProposal, make_console_proposal

def make_same_statement() -> SameStatementConsoleProposal:
  return make_console_proposal("same")

def test_same_statement_pub_import_alias() -> None:
  proposal: SameStatementConsoleProposal = make_same_statement()
  assert proposal.title == "same"
"#,
        )?;
        let consumer_tests = run_test(&consumer_root.join("tests"))?;
        assert!(
            consumer_tests.status.success(),
            "expected direct public-module imports to compile and run in a package test batch.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&consumer_tests.stdout),
            String::from_utf8_lossy(&consumer_tests.stderr)
        );
        let test_stdout = String::from_utf8_lossy(&consumer_tests.stdout);
        assert!(
            test_stdout.contains("test_public_module.incn::test_public_module_namespace")
                && test_stdout.contains("test_split_alias.incn::test_split_pub_import_alias")
                && test_stdout.contains("test_same_statement_alias.incn::test_same_statement_pub_import_alias"),
            "expected all #948/#892 regressions in the shared test batch.\nstdout:\n{test_stdout}"
        );

        std::fs::write(
            producer_root.join("src/hyperquant.incn"),
            "pub def conflicting_module_file() -> int:\n  return 2\n",
        )?;
        let collision_build = bake_library_provider(&producer_root)?;
        assert!(
            !collision_build.status.success(),
            "a module file and directory entrypoint with the same logical identity must not publish nondeterministically"
        );
        let collision_error = String::from_utf8_lossy(&collision_build.stderr);
        assert!(
            collision_error.contains("both resolve to library module `hyperquant`"),
            "expected the producer collision diagnostic, got:\n{collision_error}"
        );
        Ok(())
    }

    #[test]
    fn build_lib_consumer_preserves_private_class_field_visibility_issue883() -> Result<(), Box<dyn std::error::Error>>
    {
        let tmp = tempfile::tempdir()?;
        let producer_root = tmp.path().join("sealed_class_lib");
        std::fs::create_dir_all(producer_root.join("src"))?;
        std::fs::write(
            producer_root.join("loaf.toml"),
            "[project]\nname = \"sealed_class_lib\"\nversion = \"0.1.0\"\n",
        )?;
        std::fs::write(
            producer_root.join("src/vaults.incn"),
            r#"const DEFAULT_PRIVATE_TEXT: str = "authority"

def default_label() -> str:
  return "sealed"

pub class VaultBase:
  private_text: str = DEFAULT_PRIVATE_TEXT
  computed_secret: int = 1 + 2
  pub base_count: int

pub class Vault extends VaultBase:
  secret: str
  pub label: str = default_label()

  def reveal(self) -> str:
    return self.secret

pub def make_vault(secret: str) -> Vault:
  return Vault(base_count=7, secret=secret)
"#,
        )?;
        std::fs::write(
            producer_root.join("src/lib.incn"),
            "pub from crate.vaults import Vault as PublicVault, VaultBase, make_vault\n",
        )?;

        let producer_build = bake_library_provider(&producer_root)?;
        assert!(
            producer_build.status.success(),
            "expected private-field provider library build to succeed.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&producer_build.stdout),
            String::from_utf8_lossy(&producer_build.stderr)
        );
        let provider_manifest =
            LibraryManifest::read_from_path(&producer_root.join("target/lib/sealed_class_lib.incnlib"))?;
        let vault = provider_manifest
            .contract_metadata
            .api
            .as_ref()
            .and_then(|api| api.modules.iter().find(|module| module.module_path == ["vaults"]))
            .and_then(|module| {
                module.declarations.iter().find_map(|declaration| match declaration {
                    incan_frontend::api_metadata::ApiDeclaration::Class(class) if class.name == "Vault" => Some(class),
                    _ => None,
                })
            })
            .ok_or("expected facade-backed Vault in checked API metadata")?;
        assert_eq!(
            vault.fields.iter().map(|field| field.name.as_str()).collect::<Vec<_>>(),
            vec!["private_text", "computed_secret", "base_count", "secret", "label"],
            "manifest constructor fields must retain the provider's parent-first ABI order"
        );
        let private_text = vault
            .fields
            .iter()
            .find(|field| field.name == "private_text")
            .ok_or("expected inherited private_text field in provider manifest")?;
        let secret = vault
            .fields
            .iter()
            .find(|field| field.name == "secret")
            .ok_or("expected secret field in provider manifest")?;
        let computed_secret = vault
            .fields
            .iter()
            .find(|field| field.name == "computed_secret")
            .ok_or("expected computed_secret field in provider manifest")?;
        let label = vault
            .fields
            .iter()
            .find(|field| field.name == "label")
            .ok_or("expected label field in provider manifest")?;
        assert_eq!(secret.visibility, FieldVisibilityExport::Private);
        assert_eq!(label.visibility, FieldVisibilityExport::Public);
        assert_eq!(private_text.visibility, FieldVisibilityExport::Private);
        assert_eq!(computed_secret.visibility, FieldVisibilityExport::Private);
        assert!(computed_secret.has_default);
        assert_eq!(computed_secret.default, None);
        assert!(private_text.has_default);
        assert_eq!(
            private_text.default,
            Some(incan_frontend::library_manifest::ParamDefaultExport::ConstRef(vec![
                "vaults".to_string(),
                "DEFAULT_PRIVATE_TEXT".to_string(),
            ]))
        );
        assert!(label.has_default);
        assert!(matches!(
            label.default,
            Some(incan_frontend::library_manifest::ParamDefaultExport::Call {
                ref path,
                ref signature,
                ..
            }) if path == &["vaults".to_string(), "default_label".to_string()] && signature.is_some()
        ));

        let decoy_root = tmp.path().join("decoy_class_lib");
        std::fs::create_dir_all(decoy_root.join("src"))?;
        std::fs::write(
            decoy_root.join("loaf.toml"),
            "[project]\nname = \"decoy_class_lib\"\nversion = \"0.1.0\"\n",
        )?;
        std::fs::write(
            decoy_root.join("src/lib.incn"),
            r#"pub class PublicVault:
  private_text: int = 11
  computed_secret: str = "wrong" + "provider"
  pub base_count: str
  secret: int
  pub label: int = 99
"#,
        )?;
        let decoy_build = bake_library_provider(&decoy_root)?;
        assert!(
            decoy_build.status.success(),
            "expected duplicate-short-name decoy library build to succeed.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&decoy_build.stdout),
            String::from_utf8_lossy(&decoy_build.stderr)
        );

        let consumer_root = tmp.path().join("consumer");
        // Both spellings resolve the same published class. Keep them in one
        // consumer build while preserving the separate negative access check.
        let consumer_main = write_project_files(
            &consumer_root,
            "[project]\nname = \"sealed_class_consumer\"\n\n[dependencies]\nsealed_class_lib = { path = \"../sealed_class_lib\" }\ndecoy_class_lib = { path = \"../decoy_class_lib\" }\n",
            r#"from pub::sealed_class_lib import PublicVault, PublicVault as ConsumerVault

def main() -> None:
  value: PublicVault = PublicVault(base_count=9, secret="authority")
  overridden: PublicVault = PublicVault(computed_secret=4, base_count=10, secret="override")
  aliased: ConsumerVault = ConsumerVault(base_count=11, secret="alias")
  println(value.label)
  println(value.base_count)
  println(overridden.base_count)
  println(aliased.base_count)
"#,
        )?;

        let consumer_bake = bake_project(&consumer_root)?;
        assert!(
            consumer_bake.status.success(),
            "expected an explicit Oven bake to import the sealed-class package closure.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&consumer_bake.stdout),
            String::from_utf8_lossy(&consumer_bake.stderr)
        );
        let public_build = run_build(&consumer_main, &consumer_root.join("out"))?;
        assert!(
            public_build.status.success(),
            "expected public and aliased named construction to generate valid Rust.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&public_build.stdout),
            String::from_utf8_lossy(&public_build.stderr)
        );
        std::fs::write(
            &consumer_main,
            r#"from pub::sealed_class_lib import PublicVault as ConsumerVault

def main() -> None:
  value: ConsumerVault = ConsumerVault(base_count=7, secret="authority")
  println(value.secret)
"#,
        )?;
        let private_check = run_check(&consumer_main)?;
        assert!(
            !private_check.status.success(),
            "expected compiled-library private field access to fail Incan typechecking"
        );
        let stderr = strip_ansi_escapes(&String::from_utf8_lossy(&private_check.stderr));
        assert!(
            stderr.contains("Field 'secret' on 'ConsumerVault' is private"),
            "expected source-level private field diagnostic, got:\n{stderr}"
        );

        Ok(())
    }

    #[test]
    fn private_pub_model_survives_facade_library_and_test_batch_boundaries_issue884()
    -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let producer_root = tmp.path().join("sealed_model_lib");
        std::fs::create_dir_all(producer_root.join("src"))?;
        std::fs::write(
            producer_root.join("loaf.toml"),
            "[project]\nname = \"sealed_model_lib\"\nversion = \"0.1.0\"\n",
        )?;
        std::fs::write(
            producer_root.join("src/vaults.incn"),
            r#"from std.serde.json import Serialize

pub model Vault with Serialize:
  secret [alias="wire_secret"]: str = "sealed"
  pub label: str

  def reveal(self) -> str:
    return self.secret

  def reflected_field_count(self) -> int:
    return len(self.__field_items__())
"#,
        )?;
        std::fs::write(
            producer_root.join("src/public_api.incn"),
            "pub from crate.vaults import Vault as PublicVault\n",
        )?;
        std::fs::write(
            producer_root.join("src/source_consumer.incn"),
            r#"from crate.vaults import Vault

pub def make_vault(label: str) -> Vault:
  return Vault(label=label)
"#,
        )?;
        std::fs::write(
            producer_root.join("src/lib.incn"),
            concat!(
                "pub from crate.public_api import PublicVault as ExportedVault\n",
                "pub from crate.source_consumer import make_vault\n",
            ),
        )?;
        let sibling_leak = producer_root.join("src/sibling_leak.incn");
        std::fs::write(
            &sibling_leak,
            r#"from crate.vaults import Vault

def leak(value: Vault) -> str:
  return value.secret
"#,
        )?;
        let sibling_check = run_check(&sibling_leak)?;
        assert!(
            !sibling_check.status.success(),
            "expected a sibling source module to be outside the private model boundary"
        );
        let sibling_stderr = strip_ansi_escapes(&String::from_utf8_lossy(&sibling_check.stderr));
        assert!(
            sibling_stderr.contains("Field 'secret' on 'Vault' is private"),
            "expected sibling private-field diagnostic, got:\n{sibling_stderr}"
        );
        std::fs::remove_file(&sibling_leak)?;

        let producer_build = bake_library_provider(&producer_root)?;
        assert!(
            producer_build.status.success(),
            "expected private-model provider library build to succeed.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&producer_build.stdout),
            String::from_utf8_lossy(&producer_build.stderr)
        );
        let provider_manifest =
            LibraryManifest::read_from_path(&producer_root.join("target/lib/sealed_model_lib.incnlib"))?;
        let vault = provider_manifest
            .contract_metadata
            .api
            .as_ref()
            .and_then(|api| api.modules.iter().find(|module| module.module_path == ["vaults"]))
            .and_then(|module| {
                module.declarations.iter().find_map(|declaration| match declaration {
                    incan_frontend::api_metadata::ApiDeclaration::Model(model) if model.name == "Vault" => Some(model),
                    _ => None,
                })
            })
            .ok_or("expected facade-backed Vault in checked API metadata")?;
        let secret = vault
            .fields
            .iter()
            .find(|field| field.name == "secret")
            .ok_or("expected private secret field in provider manifest")?;
        let label = vault
            .fields
            .iter()
            .find(|field| field.name == "label")
            .ok_or("expected public label field in provider manifest")?;
        assert_eq!(secret.visibility, FieldVisibilityExport::Private);
        assert_eq!(label.visibility, FieldVisibilityExport::Public);

        let consumer_root = tmp.path().join("consumer");
        let consumer_main = write_project_files(
            &consumer_root,
            "[project]\nname = \"sealed_model_consumer\"\nversion = \"0.1.0\"\n\n[dependencies]\nsealed_model_lib = { path = \"../sealed_model_lib\" }\n",
            r#"from pub::sealed_model_lib import ExportedVault as ConsumerVault, make_vault

def main() -> None:
  value = ConsumerVault(label="visible")
  make_vault("sibling")
  println(value.label)
  println(value.reveal())
  fields = value.__fields__()
  println(len(fields))
  println(fields[0].name)
  println(value.reflected_field_count())
"#,
        )?;
        let tests_dir = consumer_root.join("tests");
        std::fs::create_dir_all(&tests_dir)?;
        std::fs::write(
            tests_dir.join("test_private_model.incn"),
            r#"from pub::sealed_model_lib import ExportedVault as TestVault

def test_private_model_provider_bridge_and_reflection() -> None:
  value = TestVault(label="test-batch")
  assert value.label == "test-batch"
  assert value.reveal() == "sealed"
  assert len(value.__fields__()) == 1
  assert value.reflected_field_count() == 1
"#,
        )?;
        let consumer_bake = bake_project(&consumer_root)?;
        assert!(
            consumer_bake.status.success(),
            "expected an explicit Oven bake to import the sealed-model package closure.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&consumer_bake.stdout),
            String::from_utf8_lossy(&consumer_bake.stderr)
        );
        let consumer_run = incan_command()
            .current_dir(&consumer_root)
            .args(["run", consumer_main.to_string_lossy().as_ref()])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            consumer_run.status.success(),
            "expected compiled private-model consumer to run.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&consumer_run.stdout),
            String::from_utf8_lossy(&consumer_run.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&consumer_run.stdout).trim(),
            "visible\nsealed\n1\nlabel\n1"
        );
        let test_output = run_test(&tests_dir)?;
        assert!(
            test_output.status.success(),
            "expected compiled private-model test batch to pass.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&test_output.stdout),
            String::from_utf8_lossy(&test_output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&test_output.stdout).contains("test_private_model_provider_bridge_and_reflection"),
            "expected private-model regression test to execute:\n{}",
            String::from_utf8_lossy(&test_output.stdout)
        );

        std::fs::write(
            &consumer_main,
            r#"from pub::sealed_model_lib import ExportedVault as ConsumerVault

def leak(value: ConsumerVault) -> str:
  return value.secret

def construct() -> ConsumerVault:
  return ConsumerVault(wire_secret="leaked", label="outside")

def unpack(value: ConsumerVault) -> str:
  match value:
    ConsumerVault(secret=secret) =>
      return secret
"#,
        )?;
        let private_check = run_check(&consumer_main)?;
        assert!(
            !private_check.status.success(),
            "expected compiled consumer access and construction through private model fields to fail"
        );
        let private_stderr = strip_ansi_escapes(&String::from_utf8_lossy(&private_check.stderr));
        assert!(
            private_stderr.contains("Field 'secret' on 'ConsumerVault' is private")
                && private_stderr.contains("Field 'wire_secret' on 'ConsumerVault' is private"),
            "expected canonical and aliased private-field diagnostics, got:\n{private_stderr}"
        );

        Ok(())
    }

    #[test]
    fn compiled_parent_fields_lower_into_consumer_subclasses_issue885() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let provider_root = tmp.path().join("compiled_parent");
        std::fs::create_dir_all(provider_root.join("src"))?;
        std::fs::write(
            provider_root.join("loaf.toml"),
            "[project]\nname = \"compiled_parent\"\nversion = \"0.1.0\"\n",
        )?;
        std::fs::write(
            provider_root.join("src/lib.incn"),
            r#"
pub class Base:
  private_text: str = "authority"
  pub base_count: int

pub class Child extends Base:
  pub own_flag: bool
"#,
        )?;
        let provider_build = bake_library_provider(&provider_root)?;
        assert!(
            provider_build.status.success(),
            "expected compiled parent library build to succeed.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&provider_build.stdout),
            String::from_utf8_lossy(&provider_build.stderr)
        );
        assert!(
            provider_root.join("target/lib/compiled_parent.incnlib").is_file(),
            "expected the provider build to publish its compiled library artifact"
        );

        let consumer_root = tmp.path().join("consumer");
        let consumer_main = write_project_files(
            &consumer_root,
            "[project]\nname = \"compiled_parent_consumer\"\nversion = \"0.1.0\"\n\n[dependencies]\ncompiled_parent = { path = \"../compiled_parent\" }\n",
            r#"from pub::compiled_parent import Child

class GrandChild extends Child:
  pub extra: float

def main() -> None:
  value: GrandChild = GrandChild(
    base_count=7,
    own_flag=true,
    extra=1.5,
  )
  println(value.base_count)
"#,
        )?;

        let out_dir = consumer_root.join("out");
        let consumer_bake = bake_project(&consumer_root)?;
        assert!(
            consumer_bake.status.success(),
            "expected an explicit Oven bake to import the compiled-parent package closure.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&consumer_bake.stdout),
            String::from_utf8_lossy(&consumer_bake.stderr)
        );
        // One normal consumer build proves the completed result is reusable for the original lowering regression.
        // Lock, package-test batch, transitive provider, and Rust bridge paths
        // each have focused coverage; repeating them here adds cost without
        // extending #885's compiled-parent field-materialization contract.
        let build_output = run_build(&consumer_main, &out_dir)?;
        assert!(
            build_output.status.success(),
            "expected inherited compiled-parent fields to survive generated Rust lowering.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&build_output.stdout),
            String::from_utf8_lossy(&build_output.stderr)
        );

        let generated_main = std::fs::read_to_string(out_dir.join("src/main.rs"))?;
        let grandchild_start = generated_main
            .find("struct GrandChild")
            .ok_or("expected generated GrandChild struct")?;
        let grandchild_end = generated_main[grandchild_start..]
            .find("\n}")
            .map(|offset| grandchild_start + offset)
            .ok_or("expected generated GrandChild struct body")?;
        let grandchild = &generated_main[grandchild_start..grandchild_end];
        let private_index = grandchild
            .find("private_text: String")
            .ok_or("expected inherited private_text field in generated GrandChild")?;
        assert!(
            !grandchild.contains("pub private_text: String"),
            "expected inherited private_text to preserve private visibility.\ngenerated GrandChild:\n{grandchild}"
        );
        let base_index = grandchild
            .find("pub base_count: i64")
            .ok_or("expected inherited base_count field in generated GrandChild")?;
        let child_index = grandchild
            .find("pub own_flag: bool")
            .ok_or("expected inherited own_flag field in generated GrandChild")?;
        let local_index = grandchild
            .find("pub extra: f64")
            .ok_or("expected local extra field in generated GrandChild")?;
        assert!(
            private_index < base_index && base_index < child_index && child_index < local_index,
            "expected compiled and consumer inheritance fields in parent-first order.\ngenerated GrandChild:\n{grandchild}"
        );

        Ok(())
    }

    #[test]
    fn build_succeeds_for_pub_import_regression_batch() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path().join("pub_import_regression_batch_project");
        std::fs::create_dir_all(project_root.join("src"))?;
        std::fs::write(
            project_root.join("loaf.toml"),
            "[project]\nname = \"pub_import_regression_batch\"\nversion = \"0.1.0\"\n",
        )?;

        let files = [
            (
                "src/session/types.incn",
                r#"pub class Session:
  pub id: int
"#,
            ),
            ("src/session/mod.incn", "pub from crate.session.types import Session\n"),
            (
                "src/session_facade_case.incn",
                r#"from session import Session

pub def run_session_facade() -> None:
  s = Session(id=1)
  print(s.id)
"#,
            ),
            (
                "src/imported_enum_loop_rels.incn",
                r#"@derive(Clone)
pub enum ConformanceRel:
  Read
  Filter
"#,
            ),
            (
                "src/imported_enum_loop_case.incn",
                r#"from imported_enum_loop_rels import ConformanceRel

def relation_kind_name_from_conformance(rel: ConformanceRel) -> str:
  match rel:
    ConformanceRel.Read =>
      return "ReadRel"
    _ =>
      return "Other"

def scenario_matches(required: list[ConformanceRel]) -> bool:
  for expected in required:
    if expected == ConformanceRel.Read:
      if relation_kind_name_from_conformance(expected) == "ReadRel":
        return true
  return false

pub def run_imported_enum_loop() -> None:
  println(scenario_matches([ConformanceRel.Read]))
"#,
            ),
            (
                "src/len_comparison_recursive_case.incn",
                r#"@derive(Clone)
pub enum ExprKind:
  Column
  Add

@derive(Clone)
pub model Expr:
  pub kind: ExprKind
  pub column_name: str
  pub arguments: list[Expr]

pub def lower(expr: Expr) -> int:
  if expr.kind == ExprKind.Column:
    return 0
  if len(expr.arguments) < 2:
    return -1
  return 1

pub def run_len_comparison_recursive() -> None:
  println(lower(Expr(kind=ExprKind.Add, column_name="root", arguments=[])))
"#,
            ),
            (
                "src/loop_helper_shared_string_list_case.incn",
                r#"def match_index(xs: list[str], y: int) -> int:
  mut idx = 0
  while idx < len(xs):
    if len(xs[idx]) == y:
      return idx
    idx = idx + 1
  return -1

def helper_loop(xs: list[str], ys: list[int]) -> list[int]:
  mut out: list[int] = []
  for y in ys:
    out.append(match_index(xs, y))
  return out

pub def run_loop_helper_shared_string_list() -> None:
  helper_loop(["a", "bb", "ccc"], [1, 2])
"#,
            ),
            (
                "src/dict_comp_reuses_noncopy_key_case.incn",
                r#"def lengths(names: list[str]) -> dict[str, int]:
  return {name: len(name) for name in names}

pub def run_dict_comp_reuses_noncopy_key() -> None:
  values = lengths(["alice", "bob"])
  println(values["alice"])
"#,
            ),
            (
                "src/tuple_unpack_enumerate_cases.incn",
                r#"model Binding:
  name: str
  output_index: int
  expr_index: int

def field_ref(index: int) -> int:
  return index

def bind_loop(xs: list[str]) -> list[Binding]:
  mut out: list[Binding] = []
  for idx, name in enumerate(xs):
    out.append(Binding(name=name, output_index=idx, expr_index=field_ref(idx)))
  return out

def bind_comp(xs: list[str]) -> list[Binding]:
  return [Binding(name=name, output_index=idx, expr_index=field_ref(idx)) for idx, name in enumerate(xs)]

pub def run_tuple_unpack_enumerate_cases() -> None:
  bind_loop(["a", "bb"])
  bind_comp(["a", "bb"])
"#,
            ),
            (
                "src/list_str_append_literal_case.incn",
                r#"pub def columns(input_columns: list[str]) -> list[str]:
  mut columns: list[str] = []
  columns.append(input_columns[0])
  columns.append("count")
  return columns

pub def run_list_str_append_literal() -> None:
  columns(["orders_total"])
"#,
            ),
            (
                "src/imported_sum_functions.incn",
                r#"pub model ColumnRef:
  pub name: str

pub model AggregateMeasure:
  pub column_name: str

pub def col(name: str) -> ColumnRef:
  return ColumnRef(name=name)

pub def sum(expr: ColumnRef) -> AggregateMeasure:
  return AggregateMeasure(column_name=expr.name)
"#,
            ),
            (
                "src/imported_sum_shadow_case.incn",
                r#"from imported_sum_functions import col, sum

def selected_column_name() -> str:
  amount = col("amount")
  result = sum(amount)
  return result.column_name

pub def run_imported_sum_shadow() -> None:
  println(selected_column_name())
"#,
            ),
            (
                "src/cross_module_union_producers.incn",
                r#"pub def parse_value(flag: bool) -> int | str:
  if flag:
    return 1
  return "fallback"
"#,
            ),
            (
                "src/cross_module_union_consumers.incn",
                r#"pub def describe(value: int | str) -> str:
  if isinstance(value, int):
    return "number"
  else:
    return value.upper()
"#,
            ),
            (
                "src/cross_module_union_case.incn",
                r#"from cross_module_union_producers import parse_value
from cross_module_union_consumers import describe

pub def run_cross_module_union() -> None:
  println(describe(parse_value(False)))
  println(describe("literal"))
"#,
            ),
            (
                "src/qualified_enum_constructor_match_case.incn",
                r#"pub enum QualifiedConformanceRel:
  Read
  Filter
  Project

pub def relation_kind_name_from_conformance(rel: QualifiedConformanceRel) -> str:
  match rel:
    QualifiedConformanceRel.Read =>
      return "ReadRel"
    QualifiedConformanceRel.Filter =>
      return "FilterRel"
    QualifiedConformanceRel.Project =>
      return "ProjectRel"
    _ =>
      return "UnknownRel"

pub def run_qualified_enum_constructor_match() -> None:
  println(relation_kind_name_from_conformance(QualifiedConformanceRel.Filter))
"#,
            ),
            (
                "src/main.incn",
                r#"from cross_module_union_case import run_cross_module_union
from dict_comp_reuses_noncopy_key_case import run_dict_comp_reuses_noncopy_key
from imported_enum_loop_case import run_imported_enum_loop
from imported_sum_shadow_case import run_imported_sum_shadow
from len_comparison_recursive_case import run_len_comparison_recursive
from list_str_append_literal_case import run_list_str_append_literal
from loop_helper_shared_string_list_case import run_loop_helper_shared_string_list
from qualified_enum_constructor_match_case import run_qualified_enum_constructor_match
from session_facade_case import run_session_facade
from tuple_unpack_enumerate_cases import run_tuple_unpack_enumerate_cases

def main() -> None:
  run_session_facade()
  run_imported_enum_loop()
  run_len_comparison_recursive()
  run_loop_helper_shared_string_list()
  run_dict_comp_reuses_noncopy_key()
  run_tuple_unpack_enumerate_cases()
  run_list_str_append_literal()
  run_imported_sum_shadow()
  run_cross_module_union()
  run_qualified_enum_constructor_match()
"#,
            ),
        ];

        for (relative, source) in files {
            let path = project_root.join(relative);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(path, source)?;
        }

        let main_path = project_root.join("src/main.incn");
        let build_output = run_build(&main_path, &project_root.join("out"))?;
        assert!(
            build_output.status.success(),
            "expected pub import regression batch project to build successfully.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&build_output.stdout),
            String::from_utf8_lossy(&build_output.stderr)
        );

        Ok(())
    }
}
