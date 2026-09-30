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
    include!("support/rfc031_pub_import_integration_tests_rfc031_pub_import_integration_tests.rs");

    #[test]
    fn build_keeps_return_context_string_literal_union_arg_as_union_value() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path().join("return_context_union_arg");
        std::fs::create_dir_all(project_root.join("src"))?;
        std::fs::write(
            project_root.join("loaf.toml"),
            "[project]\nname = \"return_context_union_arg\"\nversion = \"0.1.0\"\n",
        )?;
        std::fs::write(
            project_root.join("src/projection_builders.incn"),
            r#"pub model ColumnRefExpr:
    pub column_name: str

pub model StringLiteralExpr:
    pub value: str

pub model FloatLiteralExpr:
    pub value: float

pub model EqExpr:
    pub arguments: list[ColumnExpr]

pub type ColumnExpr = Union[ColumnRefExpr, StringLiteralExpr, FloatLiteralExpr, EqExpr]

pub def col(name: str) -> ColumnExpr:
    return ColumnRefExpr(column_name=name)

pub def str_expr(value: str) -> ColumnExpr:
    return StringLiteralExpr(value=value)

pub def float_expr(value: float) -> ColumnExpr:
    return FloatLiteralExpr(value=value)

pub def lit(value: Union[int, float, str, bool]) -> ColumnExpr:
    match value:
        float(number) => return float_expr(number)
        str(text) => return str_expr(text)
        bool(flag) => return str_expr("bool")
        int(number) => return str_expr("int")

pub def eq(left: ColumnExpr, right: ColumnExpr) -> ColumnExpr:
    return EqExpr(arguments=[left, right])
"#,
        )?;
        std::fs::write(
            project_root.join("src/functions.incn"),
            "from projection_builders import col as col_builder, eq as eq_builder, lit as lit_builder\n\npub col = alias col_builder\npub lit = alias lit_builder\npub eq = alias eq_builder\n",
        )?;
        std::fs::write(
            project_root.join("src/dataset.incn"),
            r#"from projection_builders import ColumnExpr

pub class LazyFrame[T with Clone]:
    pub rows: list[T]

    def filter(self, predicate: ColumnExpr) -> Self:
        return self
"#,
        )?;
        let main_path = project_root.join("src/main.incn");
        std::fs::write(
            &main_path,
            r#"from dataset import LazyFrame
from functions import col, eq, lit

model OrderLine:
    status: str
    discount: float

def repro(lines: LazyFrame[OrderLine]) -> LazyFrame[OrderLine]:
    return lines.filter(eq(col("status"), lit("open"))).filter(eq(col("discount"), lit(0.9)))

def main() -> None:
    lines: LazyFrame[OrderLine] = LazyFrame[OrderLine](rows=[])
    _ = repro(lines)
    println("done")
"#,
        )?;

        let out_dir = project_root.join("out");
        let output = run_build(&main_path, &out_dir)?;
        assert!(
            output.status.success(),
            "expected union literal regression build to succeed.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let generated_main = std::fs::read_to_string(out_dir.join("src/main.rs"))?;
        let normalized: String = generated_main.chars().filter(|c| !c.is_whitespace()).collect();
        // Assert the wrapping, not the callee's spelling. RFC 120 projections emit a linker-visible
        // `__incan_v1_...` name for `lit`, so pinning the source spelling tested the projection rather than the union
        // arm this case exists for. The leading `(` still proves the literal reaches the call as the argument itself.
        assert!(
            normalized.contains("(crate::__IncanUnion43fbd19e99c1db05::V0(\"open\".to_string())"),
            "expected string literal to be wrapped directly as the union string arm, got:\n{generated_main}"
        );
        assert!(
            !normalized.contains("V0(\"open\".to_string()).to_string()"),
            "union wrapper must not receive a post-wrapper string coercion, got:\n{generated_main}"
        );
        Ok(())
    }

    #[test]
    fn std_json_and_generated_runtime_surfaces_share_one_generated_run() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"std_json_runtime_surface_batch\"\nversion = \"0.3.0-dev.1\"\n",
            r#"from std.serde import json
from std.serde.json import Deserialize, Serialize
from std.json import JsonValue

model SerializePayload with Serialize:
  value: int

model HelperPayload with Serialize:
  value: int

@derive(json)
model JsonPayload:
  value: int
  label: str

@derive(Deserialize)
model DirectPayload:
  value: int

@derive(json)
model Envelope:
  status: int
  data: JsonValue

@derive(json)
model Probe:
  name: Option[JsonValue]
  first: Option[JsonValue]
  missing: Option[JsonValue]

const NUMBERS: FrozenList[float] = [3.0, 1.5, 4.25]

def run_explicit_serialize_trait() -> None:
  println(SerializePayload(value=1).to_json())

def run_generated_runtime_helpers() -> None:
  mut xs = [3, 1, 4]
  println(xs.pop())
  println(min(xs))
  println(max(xs))
  println(HelperPayload(value=2).to_json())

def run_std_json_deserialize() -> None:
  match JsonPayload.from_json('{"value":7,"label":"dogfood"}'):
    case Ok(payload):
      println(payload.to_json())
    case Err(err):
      println(err)

def run_direct_deserialize_derive() -> None:
  match DirectPayload.from_json('{"value":7}'):
    case Ok(payload):
      println(f"{payload.value}")
    case Err(err):
      println(err)

def run_json_value_model_field_roundtrip() -> None:
  match Envelope.from_json('{"status":200,"data":{"name":"Ada","items":[1,2]}}'):
    case Ok(envelope):
      match envelope.data["items"]:
        case Some(items):
          let probe = Probe(name=envelope.data["name"], first=items[0], missing=items[9])
          println(probe.to_json())
        case None:
          println("missing items")
    case Err(err):
      println(err)

def run_std_json_value_broad_surface() -> None:
  match JsonValue.parse('{"items":[1,2],"name":"Ada","n":null}'):
    case Ok(data):
      assert data.kind().as_str() == "object"
      assert JsonValue.str("Ada").as_str() == Some("Ada")
      match data.get("n"):
        case Some(value):
          assert value.is_null()
        case None:
          assert false
      match data.get("missing"):
        case Some(_):
          assert false
        case None:
          pass
      match data["items"]:
        case Some(items):
          match items[0]:
            case Some(first):
              match first.expect_int():
                case Ok(n):
                  assert n == 1
                case Err(_):
                  assert false
            case None:
              assert false
          match items[-1]:
            case Some(_):
              assert false
            case None:
              pass
        case None:
          assert false
      mut target = JsonValue.object({"a": JsonValue.int(1)})
      match target.merge(JsonValue.object({"a": JsonValue.int(2), "b": JsonValue.str("bee")})):
        case Ok(_):
          assert target.contains_key("b")
          match target.require("a"):
            case Ok(value):
              assert value.as_int() == Some(2)
            case Err(_):
              assert false
        case Err(_):
          assert false
    case Err(err):
      println(err.message())
      assert false

def run_frozen_float_helpers() -> None:
  println(min(NUMBERS))
  println(max(NUMBERS))

def main() -> None:
  run_explicit_serialize_trait()
  run_generated_runtime_helpers()
  run_std_json_deserialize()
  run_direct_deserialize_derive()
  run_json_value_model_field_roundtrip()
  run_std_json_value_broad_surface()
  run_frozen_float_helpers()
"#,
        )?;

        let output = super::incan_command()
            .arg("run")
            .arg(&main_path)
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;

        assert!(
            output.status.success(),
            "expected std/json and generated runtime surface batch to run successfully.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(
            stdout.lines().collect::<Vec<_>>(),
            vec![
                "{\"value\":1}",
                "4",
                "1",
                "3",
                "{\"value\":2}",
                "{\"value\":7,\"label\":\"dogfood\"}",
                "7",
                "{\"name\":\"Ada\",\"first\":1,\"missing\":null}",
                "1.5",
                "4.25",
            ],
            "expected std/json and generated runtime surface transcript, got:\n{stdout}"
        );
        Ok(())
    }

    #[test]
    fn check_reports_unknown_pub_library() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"app\"\n",
            "from pub::missinglib import Widget\n",
        )?;

        let output = run_check(&main_path)?;
        assert!(
            !output.status.success(),
            "expected check to fail for unknown library, stderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stderr = strip_ansi_escapes(&String::from_utf8_lossy(&output.stderr));
        assert!(
            stderr.contains("Unknown `pub::` library `missinglib`"),
            "expected unknown-library diagnostic, got:\n{stderr}"
        );
        Ok(())
    }

    #[test]
    fn check_reports_missing_pub_export() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let dep_manifest_path = tmp
            .path()
            .join("deps")
            .join("mylib")
            .join("target")
            .join("lib")
            .join("mylib.incnlib");
        std::fs::create_dir_all(dep_manifest_path.parent().ok_or("missing dependency manifest parent")?)?;
        mylib_manifest_with_widget().write_to_path(&dep_manifest_path)?;
        write_minimal_library_crate(
            dep_manifest_path.parent().ok_or("missing dependency artifact root")?,
            "mylib",
        )?;

        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"app\"\n\n[dependencies]\nmylib = { path = \"deps/mylib\" }\n",
            "from pub::mylib import MissingSymbol\n",
        )?;

        let output = run_check(&main_path)?;
        assert!(
            !output.status.success(),
            "expected check to fail for missing export, stderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stderr = strip_ansi_escapes(&String::from_utf8_lossy(&output.stderr));
        assert!(
            stderr.contains("is not exported by `pub::mylib`"),
            "expected missing-export diagnostic, got:\n{stderr}"
        );
        Ok(())
    }

    #[test]
    fn check_reports_pub_manifest_load_failure() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let dep_manifest_path = tmp
            .path()
            .join("deps")
            .join("mylib")
            .join("target")
            .join("lib")
            .join("mylib.incnlib");
        std::fs::create_dir_all(dep_manifest_path.parent().ok_or("missing dependency manifest parent")?)?;
        std::fs::write(&dep_manifest_path, "{ not-json }\n")?;

        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"app\"\n\n[dependencies]\nmylib = { path = \"deps/mylib\" }\n",
            "from pub::mylib import Widget\n",
        )?;

        let output = run_check(&main_path)?;
        assert!(
            !output.status.success(),
            "expected check to fail for manifest load failure, stderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stderr = strip_ansi_escapes(&String::from_utf8_lossy(&output.stderr));
        assert!(
            stderr.contains("Failed to load manifest for `pub::mylib`"),
            "expected manifest-load diagnostic, got:\n{stderr}"
        );
        Ok(())
    }

    #[test]
    fn check_passes_for_pub_imported_manifest_type() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let dep_manifest_path = tmp
            .path()
            .join("deps")
            .join("mylib")
            .join("target")
            .join("lib")
            .join("mylib.incnlib");
        std::fs::create_dir_all(dep_manifest_path.parent().ok_or("missing dependency manifest parent")?)?;
        mylib_manifest_with_widget().write_to_path(&dep_manifest_path)?;
        write_minimal_library_crate(
            dep_manifest_path.parent().ok_or("missing dependency artifact root")?,
            "mylib",
        )?;

        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"app\"\n\n[dependencies]\nmylib = { path = \"deps/mylib\" }\n",
            "from pub::mylib import Widget\n\ndef build(x: Widget) -> Widget:\n  return x\n",
        )?;

        let output = run_check(&main_path)?;
        assert!(
            output.status.success(),
            "expected check to pass for valid pub import, stderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(())
    }

    #[test]
    fn check_reports_missing_pub_library_artifacts() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let dep_manifest_path = tmp
            .path()
            .join("deps")
            .join("mylib")
            .join("target")
            .join("lib")
            .join("mylib.incnlib");
        std::fs::create_dir_all(dep_manifest_path.parent().ok_or("missing dependency manifest parent")?)?;
        mylib_manifest_with_widget().write_to_path(&dep_manifest_path)?;
        // Intentionally do not write Cargo.toml / src/lib.rs to exercise artifact-contract diagnostics.

        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"app\"\n\n[dependencies]\nmylib = { path = \"deps/mylib\" }\n",
            "from pub::mylib import Widget\n",
        )?;

        let output = run_check(&main_path)?;
        assert!(
            !output.status.success(),
            "expected check to fail for missing crate artifacts, stderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stderr = strip_ansi_escapes(&String::from_utf8_lossy(&output.stderr));
        assert!(
            stderr.contains("Missing generated crate artifacts for `pub::mylib`"),
            "expected missing-artifact diagnostic, got:\n{stderr}"
        );
        Ok(())
    }

    #[test]
    fn check_reports_pub_library_artifact_mismatch() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let dep_artifact_root = tmp.path().join("deps").join("widgets-lib").join("target").join("lib");
        std::fs::create_dir_all(&dep_artifact_root)?;
        let mut manifest = LibraryManifest::new("widgets_core", "0.1.0");
        manifest.exports.models.push(ModelExport {
            name: "Widget".to_string(),
            type_params: Vec::new(),
            traits: Vec::new(),
            trait_adoptions: Vec::new(),
            derives: Vec::new(),
            fields: Vec::new(),
            properties: Vec::new(),
            methods: Vec::new(),
        });
        push_root_model_identity(&mut manifest, "widgets_core", "Widget");
        manifest.write_to_path(&dep_artifact_root.join("widgets_core.incnlib"))?;
        write_minimal_library_crate(&dep_artifact_root, "different_package_name")?;

        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"app\"\n\n[dependencies]\nwidgets = { path = \"deps/widgets-lib\" }\n",
            "from pub::widgets import Widget\n",
        )?;

        let output = run_check(&main_path)?;
        assert!(
            !output.status.success(),
            "expected check to fail for artifact mismatch, stderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stderr = strip_ansi_escapes(&String::from_utf8_lossy(&output.stderr));
        assert!(
            stderr.contains("Generated crate metadata mismatch for `pub::widgets`"),
            "expected artifact mismatch diagnostic, got:\n{stderr}"
        );
        Ok(())
    }

    #[test]
    fn build_lib_artifacts_and_consumer_alias_typecheck() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let producer_root = tmp.path().join("widgets_core_project");
        std::fs::create_dir_all(producer_root.join("src"))?;
        std::fs::write(
            producer_root.join("loaf.toml"),
            "[project]\nname = \"widgets_core\"\nversion = \"0.1.0\"\n",
        )?;
        std::fs::write(
            producer_root.join("src/widgets.incn"),
            "pub model Widget:\n  pub name: str\n\npub def make_widget(name: str) -> Widget:\n  return Widget(name=name)\n",
        )?;
        std::fs::write(
            producer_root.join("src/boxmod.incn"),
            "pub class Box:\n  def get[T with Clone](self, value: T) -> T:\n    return value\n",
        )?;
        std::fs::write(
            producer_root.join("src/lib.incn"),
            "pub from boxmod import Box\npub from widgets import Widget, make_widget\n",
        )?;

        let producer_build = bake_library_provider(&producer_root)?;
        assert!(
            producer_build.status.success(),
            "expected `build --lib` to succeed.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&producer_build.stdout),
            String::from_utf8_lossy(&producer_build.stderr)
        );
        let producer_artifact_root = producer_root.join("target").join("lib");
        assert!(producer_artifact_root.join("Cargo.toml").is_file());
        assert!(producer_artifact_root.join("src/lib.rs").is_file());
        assert!(producer_artifact_root.join("widgets_core.incnlib").is_file());

        let consumer_root = tmp.path().join("consumer_app");
        std::fs::create_dir_all(consumer_root.join("src"))?;
        std::fs::write(
            consumer_root.join("loaf.toml"),
            "[project]\nname = \"consumer\"\n\n[dependencies]\nwidgets = { path = \"../widgets_core_project\" }\n",
        )?;
        let consumer_main = consumer_root.join("src/main.incn");
        std::fs::write(
            &consumer_main,
            "from pub::widgets import Box, Widget as PublicWidget, make_widget\n\ndef main() -> None:\n  w: PublicWidget = make_widget(\"ok\")\n  box: Box = Box()\n  value: int = box.get(1)\n  print(w.name)\n  print(value)\n",
        )?;

        let consumer_check = run_check(&consumer_main)?;
        assert!(
            consumer_check.status.success(),
            "expected consumer check to accept pub:: alias and generic carrier imports.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&consumer_check.stdout),
            String::from_utf8_lossy(&consumer_check.stderr)
        );

        Ok(())
    }
}
