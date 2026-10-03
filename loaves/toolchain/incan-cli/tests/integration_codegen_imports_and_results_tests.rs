#![allow(dead_code)] // Split test binaries intentionally use different parts of shared support fragments.

//! Integration tests for the Incan compiler frontend

include!("support/integration_tests_root.rs");

mod codegen_tests {
    include!("support/integration_tests_codegen_tests.rs");

    #[test]
    fn test_std_encoding_hex_compile_and_run_strict_surface() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .arg("run")
            .arg(incan_test_support::fixture("valid/std_encoding_hex_surface.incn"))
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "incan run std.encoding.hex smoke failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        let lines = stdout.lines().collect::<Vec<_>>();
        assert_eq!(
            lines,
            vec![
                "417a00",
                "3",
                "417a00",
                "417a00",
                "FF",
                "10",
                "00",
                "7f",
                "invalid_length",
                "invalid_character"
            ],
            "unexpected std.encoding.hex output:\n{stdout}"
        );
        Ok(())
    }

    #[test]
    fn test_std_fs_glob_string_api_compile_and_run() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .args([
                "run",
                "-c",
                r#"
from std.fs.glob import filter_matches, matches

def main() -> None:
    println(matches("routes/users.incn", "routes/*.incn"))
    println(matches("routes/users.incn", "routes/[a-z]*.incn"))
    println(matches("routes/users.incn", "routes/[!0-9]*.incn"))
    println(matches("routes/users.incn", "routes/?.incn"))
    hits = filter_matches(["api/users", "docs/readme", "api/orders"], "api/*")
    println(len(hits))
    println(hits[0])
    println(hits[1])
"#,
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;

        assert!(
            output.status.success(),
            "std.fs.glob string API failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = strip_ansi_escapes(&String::from_utf8_lossy(&output.stdout));
        let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        assert_eq!(
            lines,
            vec!["true", "true", "true", "false", "2", "api/users", "api/orders"],
            "unexpected std.fs.glob output:\n{stdout}"
        );
        Ok(())
    }

    #[test]
    fn test_imported_default_constructor_fields_compile_and_run() -> Result<(), Box<dyn std::error::Error>> {
        let root = make_temp_dir("incan_imported_defaults");
        fs::create_dir_all(root.join("pkg"))?;
        fs::write(
            root.join("pkg").join("config.incn"),
            r#"
pub model Config:
    pub enabled: bool = false
    pub retries: int = 3
"#,
        )?;
        let main_path = root.join("default_ctor.incn");
        fs::write(
            &main_path,
            r#"
from pkg.config import Config

def main() -> None:
    cfg = Config()
    println(cfg.enabled)
    println(cfg.retries)
"#,
        )?;
        let output = incan_command()
            .args(["run", main_path.to_string_lossy().as_ref()])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "imported default constructor regression failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "false\n3");
        Ok(())
    }

    #[test]
    fn test_imported_private_class_constructor_compile_and_run_issue886() -> Result<(), Box<dyn std::error::Error>> {
        let root = make_temp_dir("incan_imported_private_class_constructor");
        let package = root.join("src").join("pkg");
        fs::create_dir_all(&package)?;
        fs::write(
            root.join("loaf.toml"),
            "[project]\nname = \"imported_private_class_constructor\"\nversion = \"0.1.0\"\n",
        )?;
        fs::write(
            package.join("text_vaults.incn"),
            r#"
pub class Vault:
    secret: str = "sealed"
    pub label: str
    revision: int = 7

    def private_value(self) -> str:
        return self.secret

    def revision_value(self) -> int:
        return self.revision
"#,
        )?;
        fs::write(
            package.join("number_vaults.incn"),
            r#"
pub class Vault:
    secret: int = 41
    pub label: int
    revision: str = "r2"

    def private_value(self) -> int:
        return self.secret

    def revision_value(self) -> str:
        return self.revision
"#,
        )?;
        fs::write(
            package.join("vault_facade.incn"),
            "pub from text_vaults import Vault as FacadeVault\n",
        )?;
        fs::write(
            package.join("public_api.incn"),
            "pub from vault_facade import FacadeVault as ExportedVault\n",
        )?;
        let main_path = package.join("consumer.incn");
        // Direct, same-leaf alias, and multi-hop facade construction share one
        // source-resolution path, so one executable proves their coexistence.
        fs::write(
            &main_path,
            r#"
from text_vaults import Vault
from text_vaults import Vault as TextVault
from number_vaults import Vault as NumberVault
from public_api import ExportedVault as ConsumerVault

def main() -> None:
    direct = Vault(label="direct")
    text = TextVault(label="visible", revision=9)
    number = NumberVault(label=5)
    facade = ConsumerVault(label="facade", revision=11)
    println(direct.label)
    println(direct.private_value())
    println(direct.revision_value())
    println(text.label)
    println(text.private_value())
    println(text.revision_value())
    println(number.label)
    println(number.private_value())
    println(number.revision_value())
    println(facade.label)
    println(facade.private_value())
    println(facade.revision_value())
"#,
        )?;

        let direct_output = incan_command()
            .current_dir(&root)
            .args(["run", main_path.to_string_lossy().as_ref()])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            direct_output.status.success(),
            "direct imported private class constructor failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
            direct_output.status,
            String::from_utf8_lossy(&direct_output.stdout),
            String::from_utf8_lossy(&direct_output.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&direct_output.stdout).trim(),
            "direct\nsealed\n7\nvisible\nsealed\n9\n5\n41\nr2\nfacade\nsealed\n11"
        );

        let tests_dir = root.join("tests");
        fs::create_dir_all(&tests_dir)?;
        fs::write(
            tests_dir.join("test_private_class_facade.incn"),
            r#"
from pkg.public_api import ExportedVault as TestVault

def test_private_class_facade_constructor() -> None:
    value = TestVault(label="test-batch", revision=13)
    assert value.label == "test-batch"
    assert value.private_value() == "sealed"
    assert value.revision_value() == 13
"#,
        )?;
        let test_output = incan_command()
            .current_dir(&root)
            .args(["test", tests_dir.to_string_lossy().as_ref()])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            test_output.status.success(),
            "multi-hop source facade constructor failed in a generated test batch: status={:?}\nstdout:\n{}\nstderr:\n{}",
            test_output.status,
            String::from_utf8_lossy(&test_output.stdout),
            String::from_utf8_lossy(&test_output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&test_output.stdout).contains("test_private_class_facade_constructor"),
            "expected generated test batch to execute the facade constructor regression:\n{}",
            String::from_utf8_lossy(&test_output.stdout)
        );

        fs::write(
            &main_path,
            r#"
from text_vaults import Vault as PrivateVault

def main() -> None:
    value = PrivateVault(label="visible")
    println(value.secret)
"#,
        )?;
        let private_check = incan_command()
            .current_dir(&root)
            .args(["--check", main_path.to_string_lossy().as_ref()])
            .output()?;
        assert!(
            !private_check.status.success(),
            "expected imported private-field access to fail Incan typechecking"
        );
        let private_stderr = strip_ansi_escapes(&String::from_utf8_lossy(&private_check.stderr));
        assert!(
            private_stderr.contains("Field 'secret' on 'PrivateVault' is private"),
            "expected source-level private-field diagnostic, got:\n{private_stderr}"
        );
        Ok(())
    }

    #[test]
    fn test_imported_value_enum_ordinal_map_compile_and_run() -> Result<(), Box<dyn std::error::Error>> {
        let root = make_temp_dir("incan_imported_ordinal_enum");
        fs::create_dir_all(root.join("pkg"))?;
        fs::write(
            root.join("pkg").join("status.incn"),
            r#"
pub enum Status(str):
    Open = "open"
    Paid = "paid"
    Cancelled = "cancelled"
"#,
        )?;
        let main_path = root.join("ordinal_enum.incn");
        fs::write(
            &main_path,
            r#"
from std.collections import OrdinalMap
from pkg.status import Status

def main() -> None:
    statuses: list[Status] = [Status.Open, Status.Paid, Status.Cancelled]
    match OrdinalMap.from_keys(statuses):
        Ok(columns) => match columns.require(Status.Paid):
            Ok(value) => println(value)
            Err(err) => println(err.message())
        Err(err) => println(err.message())
"#,
        )?;
        let output = incan_command()
            .args(["run", main_path.to_string_lossy().as_ref()])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "imported value-enum OrdinalMap regression failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "1");
        Ok(())
    }

    #[test]
    fn test_imported_pascal_case_function_is_not_constructor() -> Result<(), Box<dyn std::error::Error>> {
        let root = make_temp_dir("incan_imported_pascal_case_function");
        fs::create_dir_all(root.join("pkg"))?;
        fs::write(
            root.join("pkg").join("factory.incn"),
            r#"
pub def BytesIO(initial: int = 7) -> int:
    return initial

pub class FactoryValue:
    secret: str = "sealed"
    pub label: str

pub def MakeValue(label: str) -> FactoryValue:
    return FactoryValue(label=label)
"#,
        )?;
        let main_path = root.join("factory_call.incn");
        fs::write(
            &main_path,
            r#"
from pkg.factory import BytesIO, MakeValue

def main() -> None:
    println(BytesIO())
    println(BytesIO(3))
    value = MakeValue(label="factory")
    println(value.label)
"#,
        )?;
        let output = incan_command()
            .args(["run", main_path.to_string_lossy().as_ref()])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "imported PascalCase function regression failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "7\n3\nfactory");
        Ok(())
    }

    #[test]
    fn test_imported_method_union_arg_compile_and_run() -> Result<(), Box<dyn std::error::Error>> {
        let root = make_temp_dir("incan_imported_method_union_arg");
        fs::create_dir_all(root.join("pkg"))?;
        fs::write(
            root.join("pkg").join("ops.incn"),
            r#"
pub model LocalPath:
    pub raw: str

pub class Opener:
    def accept(self, path: Union[LocalPath, str]) -> str:
        return "ok"
"#,
        )?;
        let main_path = root.join("union_arg.incn");
        fs::write(
            &main_path,
            r#"
from pkg.ops import LocalPath, Opener

def main() -> None:
    println(Opener().accept(LocalPath(raw="a")))
    println(Opener().accept("b"))
"#,
        )?;
        let output = incan_command()
            .args(["run", main_path.to_string_lossy().as_ref()])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "imported method union argument regression failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "ok\nok");
        Ok(())
    }

    #[test]
    fn test_std_fs_preserves_legacy_file_builtins() -> Result<(), Box<dyn std::error::Error>> {
        let path = std::env::temp_dir().join(format!("incan_std_fs_legacy_builtin_{}.txt", std::process::id()));
        let source = format!(
            r#"
def main() -> None:
    match write_file("{path}", "legacy"):
        Ok(_) => pass
        Err(err) => println(err.to_string())
    match read_file("{path}"):
        Ok(data) => println(data)
        Err(err) => println(err.to_string())
"#,
            path = path.display()
        );
        let output = incan_command()
            .args(["run", "-c", source.as_str()])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "legacy file builtins failed after std.fs registration: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(stdout.trim(), "legacy", "unexpected legacy builtin output:\n{stdout}");
        let _ = std::fs::remove_file(path);
        Ok(())
    }

    #[test]
    fn legacy_file_builtin_errors_honor_declared_string_type_issue874() -> Result<(), Box<dyn std::error::Error>> {
        let missing_root = std::env::temp_dir().join(format!(
            "incan_legacy_file_error_type_issue874_{}_missing",
            std::process::id()
        ));
        let read_path = missing_root.join("read.txt");
        let write_path = missing_root.join("write.txt");
        let source = format!(
            r#"
def normalize(error: str) -> str:
    return error.upper()

def main() -> None:
    match read_file("{read_path}").map_err((error) => normalize(error)):
        Ok(_) => println("unexpected read success")
        Err(error) => println(error)
    match write_file("{write_path}", "data").map_err((error) => normalize(error)):
        Ok(_) => println("unexpected write success")
        Err(error) => println(error)
    match read_file().map_err((error) => normalize(error)):
        Ok(_) => println("unexpected missing read argument success")
        Err(error) => println(error)
    match write_file().map_err((error) => normalize(error)):
        Ok(_) => println("unexpected missing write arguments success")
        Err(error) => println(error)
"#,
            read_path = read_path.display(),
            write_path = write_path.display(),
        );
        let output = incan_command()
            .args(["run", "-c", source.as_str()])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "legacy file builtin string error regression failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8(output.stdout)?;
        let lines = stdout.lines().collect::<Vec<_>>();
        assert_eq!(
            lines.len(),
            4,
            "expected valid- and missing-argument errors for read and write:\n{stdout}"
        );
        assert!(
            lines
                .iter()
                .all(|line| !line.is_empty() && *line == line.to_uppercase())
        );
        Ok(())
    }

    #[test]
    fn rust_result_non_clone_payload_routes_compile_and_run() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .args([
                "run",
                "-c",
                r#"
from rust::std::fs import read_dir
from rust::std::fs import ReadDir
from rust::std::path import Path as RustPath

def observe_entries(_entries: ReadDir) -> None:
    pass

def tap[T, E](result: Result[T, E], f: Callable[T, None]) -> Result[T, E]:
    match result:
        Ok(value) =>
            f(value)
            return Ok(value)
        Err(error) => return Err(error)

def main() -> None:
    match read_dir(RustPath.new(".")):
        Ok(entries) =>
            mut seen = False
            for entry_result in entries:
                match entry_result:
                    Ok(entry) =>
                        seen = seen or entry.path().to_string_lossy().into_owned() != ""
                    Err(err) => println(err.to_string())
            println(seen)
        Err(err) => println(err.to_string())
    inspected = read_dir(RustPath.new(".")).inspect(observe_entries)
    match inspected:
        Ok(entries) =>
            mut seen = False
            for entry_result in entries:
                match entry_result:
                    Ok(entry) =>
                        seen = seen or entry.path().to_string_lossy().into_owned() != ""
                    Err(err) => println(err.to_string())
            println(seen)
        Err(err) => println(err.to_string())
    tapped = tap(read_dir(RustPath.new(".")), observe_entries)
    match tapped:
        Ok(entries) =>
            mut seen = False
            for entry_result in entries:
                match entry_result:
                    Ok(entry) =>
                        seen = seen or entry.path().to_string_lossy().into_owned() != ""
                    Err(err) => println(err.to_string())
            println(seen)
        Err(err) => println(err.to_string())
"#,
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "non-Clone Rust Result route regression failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        let lines = stdout.lines().collect::<Vec<_>>();
        assert_eq!(
            lines,
            vec!["true", "true", "true"],
            "expected direct match, Result.inspect, and user-authored tap routes to consume the non-Clone payload:\n{stdout}"
        );
        Ok(())
    }

    #[test]
    fn test_result_execution_matrix() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .args([
                "run",
                "-c",
                r#"
from std.result import map as result_map, map_err as result_map_err
from std.result import and_then as result_and_then, or_else as result_or_else
from std.traits.callable import Callable1

def result_double(value: int) -> int:
    return value * 2

def result_prefix(error: str) -> str:
    return f"error: {error}"

def result_keep_even(value: int) -> Result[int, str]:
    if value % 2 == 0:
        return Ok(value)
    return Err("odd")

def result_recover(_error: str) -> Result[int, str]:
    return Ok(7)

model ResultPrefixer with Callable1[str, str]:
    prefix: str

    def __call__(self, error: str) -> str:
        return f"{self.prefix}: {error}"

def result_from_return() -> Result[str, str]:
    return Ok("from_return")

def std_result_free_helpers() -> None:
    ok_value: Result[int, str] = Ok(2)
    err_value: Result[int, str] = Err("bad")
    even_value: Result[int, str] = Ok(4)
    missing_value: Result[int, str] = Err("missing")
    match result_map(ok_value, result_double):
        Ok(value) => println(value)
        Err(error) => println(error)
    match result_map_err(err_value, result_prefix):
        Ok(value) => println(value)
        Err(error) => println(error)
    match result_and_then(even_value, result_keep_even):
        Ok(value) => println(value)
        Err(error) => println(error)
    match result_or_else(missing_value, result_recover):
        Ok(value) => println(value)
        Err(error) => println(error)

def result_methods() -> None:
    ok_value: Result[int, str] = Ok(2)
    err_value: Result[int, str] = Err("bad")
    missing_value: Result[int, str] = Err("missing")
    match ok_value.map(result_double).and_then(result_keep_even):
        Ok(value) => println(value)
        Err(error) => println(error)
    match err_value.map_err(result_prefix):
        Ok(value) => println(value)
        Err(error) => println(error)
    match missing_value.or_else(result_recover).map(result_double):
        Ok(value) => println(value)
        Err(error) => println(error)

def result_callable_object() -> None:
    value: Result[int, str] = Err("bad")
    match value.map_err(ResultPrefixer(prefix="error")):
        Ok(value) => println(value)
        Err(error) => println(error)

def result_capturing_closure() -> None:
    prefix = "uuid"
    value: Result[int, str] = Err("bad")
    mapped = value.map_err((err) => f"{prefix}: {err}")
    match mapped:
        Ok(number) => println(number)
        Err(error) => println(error)

def result_string_literals() -> None:
    direct: Result[str, str] = Ok("from_call")
    match direct:
        case Ok(msg):
            println(msg)
        case Err(err):
            println(err)
    match result_from_return():
        case Ok(msg):
            println(msg)
        case Err(err):
            println(err)

def main() -> None:
    std_result_free_helpers()
    result_methods()
    result_callable_object()
    result_capturing_closure()
    result_string_literals()
"#,
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "Result execution matrix failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = strip_ansi_escapes(&String::from_utf8_lossy(&output.stdout));
        let lines = stdout
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>();
        assert_eq!(
            lines,
            vec![
                "4",
                "error: bad",
                "4",
                "7",
                "4",
                "error: bad",
                "14",
                "error: bad",
                "uuid: bad",
                "from_call",
                "from_return",
            ],
            "unexpected Result execution matrix output:\n{stdout}"
        );
        Ok(())
    }

    #[test]
    fn test_question_mark_comprehensions_propagate_results_issue633() -> Result<(), Box<dyn std::error::Error>> {
        let output = run_incan_source(
            r#"
def parse_value(value: int) -> Result[int, str]:
    if value == 2:
        return Err("bad value")
    return Ok(value)


def parse_all(values: list[int]) -> Result[list[int], str]:
    return Ok([parse_value(value)? for value in values])


def parse_key(value: int) -> Result[str, str]:
    if value == 2:
        return Err("bad key")
    return Ok(str(value))


def parse_map(values: list[int]) -> Result[dict[str, int], str]:
    return Ok({parse_key(value)?: value for value in values})


def main() -> None:
    match parse_all([1, 2, 3]):
        Ok(values) => println(values[0])
        Err(err) => println(err)
    match parse_map([1, 2, 3]):
        Ok(values) => println(values["1"])
        Err(err) => println(err)
"#,
        );
        assert!(
            output.status.success(),
            "question-mark comprehension regression failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = strip_ansi_escapes(&String::from_utf8_lossy(&output.stdout));
        let lines = stdout
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>();
        assert_eq!(
            lines,
            vec!["bad value", "bad key"],
            "unexpected issue633 output:\n{stdout}"
        );
        Ok(())
    }

    #[test]
    fn test_static_str_index_and_slice_use_string_helpers() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .args([
                "run",
                "-c",
                r#"
const ALPHABET: str = "abcdef"

def main() -> None:
    println(ALPHABET[1])
    println(ALPHABET[2:5])
"#,
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "static str index/slice regression failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = strip_ansi_escapes(&String::from_utf8_lossy(&output.stdout));
        let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        assert_eq!(lines, vec!["b", "cde"], "unexpected static str output:\n{stdout}");
        Ok(())
    }

    #[test]
    fn test_collection_literal_spreads_compile_and_run() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .args([
                "run",
                "-c",
                r#"
def main() -> None:
    tail: tuple[int, int] = (4, 5)
    values = [1, *[2, 3], *tail]
    defaults = {"trace": "disabled", "accept": "json"}
    merged = {**defaults, "trace": "enabled"}
    println(values[0] + values[1] + values[2] + values[3] + values[4])
    println(merged["trace"])
"#,
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "collection literal spread run-path regression failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = strip_ansi_escapes(&String::from_utf8_lossy(&output.stdout));
        let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        assert_eq!(
            lines,
            vec!["15", "enabled"],
            "unexpected collection spread output:\n{stdout}"
        );
        Ok(())
    }

    #[test]
    fn test_enum_methods_and_trait_adoption_compile_and_run() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .args([
                "run",
                "-c",
                r#"
trait Labeled:
    def label(self) -> str: ...

enum Signal with Labeled:
    Start
    Stop

    def label(self) -> str:
        match self:
            Signal.Start => return "start"
            Signal.Stop => return "stop"

    def default() -> Self:
        return Signal.Start

def keep_labeled[T with Labeled](value: T) -> T:
    return value

def main() -> None:
    signal = keep_labeled(Signal.default())
    println(signal.label())
    println(Signal.Stop.label())
"#,
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "enum methods and trait adoption run-path regression failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = strip_ansi_escapes(&String::from_utf8_lossy(&output.stdout));
        let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        assert_eq!(lines, vec!["start", "stop"], "unexpected enum method output:\n{stdout}");
        Ok(())
    }

    #[test]
    fn test_union_types_compile_and_run() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .args([
                "run",
                "-c",
                r#"
@derive(Clone)
type LocalPath = newtype str

def normalize_path_like(value: LocalPath | str) -> LocalPath:
    if isinstance(value, str):
        return LocalPath(value)
    elif isinstance(value, LocalPath):
        return value

def parse_value(flag: bool) -> int | str:
    if flag:
        return 42
    return "fallback"

def normalize(value: int | str) -> str:
    if isinstance(value, int):
        return "number"
    else:
        return value.upper()

def describe(value: int | str) -> str:
    match value:
        int(n) =>
            return str(n)
        str(s) =>
            return s.upper()

def label(value: str | None) -> str:
    if value is not None:
        return value.upper()
    return "missing"

def describe_optional(value: int | str | None) -> str:
    match value:
        int(n) =>
            return str(n)
        str(s) =>
            return s.upper()
        None =>
            return "missing"

def describe_wide(value: int | str | bool) -> str:
    if isinstance(value, int):
        return "number"
    else:
        match value:
            bool(flag) =>
                if flag:
                    return "true"
                return "false"
            str(text) =>
                return text.upper()

def describe_chain(value: int | str | bool) -> str:
    if isinstance(value, int):
        return "number"
    elif isinstance(value, str):
        return value.upper()
    else:
        if value:
            return "true"
        return "false"

def describe_wide_chain(value: int | float | str | bool) -> str:
    if isinstance(value, bool):
        return "bool"
    elif isinstance(value, int):
        return "int"
    elif isinstance(value, float):
        return "float"
    elif isinstance(value, str):
        return value.upper()
    return "unknown"

def describe_wide_match(value: int | float | str | bool) -> str:
    match value:
        bool(flag) =>
            if flag:
                return "bool:true"
            return "bool:false"
        int(n) =>
            return str(n)
        float(f) =>
            return str(f)
        str(s) =>
            return s.upper()

def describe_optional_narrow(value: int | str | None) -> str:
    if isinstance(value, int):
        return "number"
    else:
        if value is None:
            return "missing"
        else:
            return value.upper()

def main() -> None:
    println(normalize(parse_value(False)))
    println(normalize(parse_value(True)))
    println(describe(parse_value(False)))
    println(label("present"))
    println(label(None))
    println(describe_optional(parse_value(True)))
    println(describe_optional(None))
    println(describe_wide("wide"))
    println(describe_wide(True))
    println(describe_chain("chain"))
    println(describe_chain(False))
    println(describe_wide_chain("wide-chain"))
    println(describe_wide_chain(1.25))
    println(describe_wide_match(True))
    println(describe_wide_match(7))
    println(describe_wide_match(2.5))
    println(describe_wide_match("match"))
    println(describe_optional_narrow("optional"))
    println(describe_optional_narrow(None))
    println(normalize_path_like("from-string").0)
    println(normalize_path_like(LocalPath("from-path")).0)
"#,
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "union type run-path regression failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = strip_ansi_escapes(&String::from_utf8_lossy(&output.stdout));
        let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        assert_eq!(
            lines,
            vec![
                "FALLBACK",
                "number",
                "FALLBACK",
                "PRESENT",
                "missing",
                "42",
                "missing",
                "WIDE",
                "true",
                "CHAIN",
                "false",
                "WIDE-CHAIN",
                "float",
                "bool:true",
                "7",
                "2.5",
                "MATCH",
                "OPTIONAL",
                "missing",
                "from-string",
                "from-path"
            ],
            "unexpected union output:\n{stdout}"
        );
        Ok(())
    }

    #[test]
    fn test_union_model_variants_compile_and_run() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .args([
                "run",
                "-c",
                r#"
@derive(Clone)
model Leaf:
    value: int

@derive(Clone)
model Pair:
    args: list[Expr]

type Expr = Union[Leaf, Pair]

def pair() -> Expr:
    return Pair(args=[Leaf(value=1), Leaf(value=2)])

def clone_expr(expr: Expr) -> Expr:
    return expr.clone()

def sum_expr(expr: Expr) -> int:
    match expr:
        Leaf(leaf) =>
            return leaf.value
        Pair(pair) =>
            return sum_expr(pair.args[0])

def main() -> None:
    println(sum_expr(clone_expr(pair())))
"#,
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "union model variant run-path regression failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = strip_ansi_escapes(&String::from_utf8_lossy(&output.stdout));
        let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        assert_eq!(lines, vec!["1"], "unexpected union model variant output:\n{stdout}");
        Ok(())
    }

    #[test]
    fn test_imported_union_alias_list_field_compiles_issue622() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path().join("union_list_cross_module_alias_repro");
        fs::create_dir_all(project_root.join("src"))?;
        fs::write(
            project_root.join("loaf.toml"),
            "[project]\nname = \"union_list_cross_module_alias_repro\"\nversion = \"0.1.0\"\n",
        )?;
        fs::write(
            project_root.join("src/exprs.incn"),
            r#"
@derive(Clone)
pub model Leaf:
    pub value: int

@derive(Clone)
pub model Pair:
    pub args: list[Expr]

pub type Expr = Union[Leaf, Pair]

pub def pair() -> Expr:
    return Pair(args=[Leaf(value=1), Leaf(value=2)])
"#,
        )?;
        fs::write(
            project_root.join("src/lib.incn"),
            r#"
from exprs import Expr, Leaf, Pair, pair

def sum_expr(expr: Expr) -> int:
    match expr:
        Leaf(leaf) => return leaf.value
        Pair(pair_expr) => return sum_expr(pair_expr.args[0])

pub def main_value() -> int:
    return sum_expr(pair())
"#,
        )?;

        let output = incan_command()
            .args(["build", "--lib"])
            .current_dir(&project_root)
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "expected imported union alias list-field project to build for #622.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(())
    }

    #[test]
    fn test_keyword_named_public_alias_compiles_issue669() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path().join("keyword_named_public_alias_repro");
        fs::create_dir_all(&project_root)?;
        fs::write(
            project_root.join("test_keyword_alias_probe.incn"),
            r#"
pub def modulo_value(value: int) -> int:
    return value

pub mod = alias modulo_value


def test_keyword_alias_probe__can_call_alias() -> None:
    assert mod(7) == 7, "keyword alias should call the implementation"
"#,
        )?;

        let output = incan_command()
            .args(["test", "test_keyword_alias_probe.incn"])
            .current_dir(&project_root)
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "expected keyword-named public alias test project to pass for #669.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(())
    }

    #[test]
    fn test_issue562_type_alias_dict_and_union_surfaces_compile_and_run() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .args([
                "run",
                "-c",
                r#"
type FieldValue = str | bool | int | float | None
type Fields = Dict[str, FieldValue]

model Logger:
    fields: Fields = {}

    def copy_fields(self, extra: Fields) -> Fields:
        mut merged: Fields = {}
        for key in self.fields.keys():
            merged[key] = self.fields[key]
        for key in extra.keys():
            merged[key] = extra[key]
        return merged

def to_text(value: FieldValue) -> str:
    match value:
        str(text) =>
            return text
        bool(flag) =>
            if flag:
                return "true"
            return "false"
        int(number) =>
            return str(number)
        float(number) =>
            return str(number)
        None =>
            return "none"

def main() -> None:
    logger = Logger(fields={"base": "one"})
    merged = logger.copy_fields({"count": 7, "flag": True, "ratio": 2.5, "none": None})
    println(to_text(merged["base"]))
    println(to_text(merged["count"]))
    println(to_text(merged["flag"]))
    println(to_text(merged["ratio"]))
    println(to_text(merged["none"]))
"#,
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "issue #562 alias transparency run-path regression failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = strip_ansi_escapes(&String::from_utf8_lossy(&output.stdout));
        let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        assert_eq!(
            lines,
            vec!["one", "7", "true", "2.5", "none"],
            "unexpected issue #562 alias transparency output:\n{stdout}"
        );
        Ok(())
    }

    #[test]
    fn test_issue502_independent_union_narrowing_branches_compile_and_run() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .args([
                "run",
                "-c",
                r#"
@derive(Clone)
type LocalPath = newtype str

def normalize_path_like(value: LocalPath | str) -> LocalPath:
    if isinstance(value, str):
        return LocalPath(value)
    if isinstance(value, LocalPath):
        return value

def main() -> None:
    println(normalize_path_like("from-string").0)
    println(normalize_path_like(LocalPath("from-path")).0)
"#,
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "independent union narrowing branch regression failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = strip_ansi_escapes(&String::from_utf8_lossy(&output.stdout));
        let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        assert_eq!(
            lines,
            vec!["from-string", "from-path"],
            "unexpected independent union narrowing output:\n{stdout}"
        );
        Ok(())
    }

    #[test]
    fn test_issue501_option_union_isinstance_narrowing_compile_and_run() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .args([
                "run",
                "-c",
                r#"
@derive(Clone)
type LocalPath = newtype str

def describe(value: Option[LocalPath | str]) -> str:
    if value is not None:
        if isinstance(value, str):
            return value.upper()
        elif isinstance(value, LocalPath):
            return value.0
    return "missing"

def main() -> None:
    println(describe("from-string"))
    println(describe(LocalPath("from-path")))
    println(describe(None))
"#,
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "Option[Union] isinstance narrowing regression failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = strip_ansi_escapes(&String::from_utf8_lossy(&output.stdout));
        let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        assert_eq!(
            lines,
            vec!["FROM-STRING", "from-path", "missing"],
            "unexpected Option[Union] narrowing output:\n{stdout}"
        );
        Ok(())
    }

    #[test]
    fn mixed_string_storage_isinstance_union_variants_compile_and_run() -> Result<(), Box<dyn std::error::Error>> {
        let stdout = compile_and_run_local_partial_project(
            r#"
const FROZEN_TEXT: FrozenStr = "frozen"

def is_string(value: FrozenStr | str | int) -> bool:
    return std.builtins.isinstance(value, str)

def optional_is_string(value: Option[FrozenStr | str | int]) -> bool:
    return std.builtins.isinstance(value, str)

def render_string(value: FrozenStr | str | int) -> str:
    if std.builtins.isinstance(value, str):
        return str(value)
    return "number"

def render_optional_string(value: Option[FrozenStr | str | int]) -> str:
    if std.builtins.isinstance(value, str):
        return str(value)
    return "other"

def main() -> None:
    println(is_string(FROZEN_TEXT))
    println(is_string("runtime"))
    println(is_string(7))
    println(optional_is_string(FROZEN_TEXT))
    println(optional_is_string("runtime"))
    println(optional_is_string(7))
    println(optional_is_string(None))
    println(render_string(FROZEN_TEXT))
    println(render_string("runtime"))
    println(render_string(7))
    println(render_optional_string(FROZEN_TEXT))
    println(render_optional_string("runtime"))
    println(render_optional_string(7))
    println(render_optional_string(None))
"#,
            "mixed_string_storage_isinstance_contract",
        )?;

        let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        assert_eq!(
            lines,
            vec![
                "true", "true", "false", "true", "true", "false", "false", "frozen", "runtime", "number", "frozen",
                "runtime", "other", "other",
            ],
            "unexpected mixed string-storage isinstance output:\n{stdout}"
        );
        Ok(())
    }
}
