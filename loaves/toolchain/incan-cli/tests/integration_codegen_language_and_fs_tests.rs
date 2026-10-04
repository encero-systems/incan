#![allow(dead_code)] // Split test binaries intentionally use different parts of shared support fragments.

//! Integration tests for the Incan compiler frontend

include!("support/integration_tests_root.rs");

mod codegen_tests {
    use incan_driver::backend::IrCodegen;
    use incan_frontend::{lexer, parser, typechecker};
    use incan_semantics_core::{SemanticSourceTargetKind, decode_incan_symbol_identity, encode_incan_symbol_identity};
    use std::process::Stdio;
    use std::thread;
    use std::time::Duration;

    include!("support/integration_tests_codegen_tests.rs");

    #[test]
    fn test_hello_world_codegen() {
        let path = repo_root().join("examples/hello.incn");
        if !path.exists() {
            return; // Skip if example not present
        }

        let Ok(source) = fs::read_to_string(&path) else {
            panic!("failed to read {}", path.display());
        };
        let Ok(tokens) = lexer::lex(&source) else {
            panic!("lexing failed");
        };
        let Ok(ast) = parser::parse(&tokens) else {
            panic!("parse failed");
        };
        let Ok(()) = typechecker::check(&ast) else {
            panic!("typecheck failed");
        };
        let Ok(rust_code) = IrCodegen::new().try_generate(&ast) else {
            panic!("codegen failed");
        };

        // Verify the generated code contains expected elements
        assert!(rust_code.contains("fn main()"), "Should have main function");
        assert!(rust_code.contains("println!"), "Should have println macro");
        assert!(rust_code.contains("Hello from Incan!"), "Should have the message");
    }

    #[test]
    fn test_string_literal_match_patterns_compile_and_run() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .args([
                "run",
                "-c",
                r#"
def describe(value: str) -> str:
    match value:
        case "star":
            return "literal"
        case other:
            return other.upper()

def describe_alt(value: str) -> str:
    mut out = ""
    match value:
        "star" | "sun" => out += "literal"
        other => out += other.upper()
    return out

def main() -> None:
    println(describe("star"))
    println(describe("fallback"))
    println(describe_alt("sun"))
    println(describe_alt("fallback"))
"#,
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;

        assert!(
            output.status.success(),
            "string literal match pattern regression failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = strip_ansi_escapes(&String::from_utf8_lossy(&output.stdout));
        let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        assert_eq!(
            lines,
            vec!["literal", "FALLBACK", "literal", "FALLBACK"],
            "unexpected string match output:\n{stdout}"
        );
        Ok(())
    }

    #[test]
    fn test_payload_enum_without_equality_payload_compiles() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .args([
                "run",
                "-c",
                r#"
model Payload:
    value: str

enum Token:
    Item(Payload)
    Empty

enum Mode:
    Fast
    Slow

def describe(token: Token) -> str:
    match token:
        case Token.Item(payload):
            return payload.value
        case Token.Empty:
            return "empty"

def main() -> None:
    if Mode.Fast == Mode.Fast:
        println(describe(Token.Item(Payload(value="ok"))))
"#,
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;

        assert!(
            output.status.success(),
            "payload enum derive regression failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = strip_ansi_escapes(&String::from_utf8_lossy(&output.stdout));
        let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        assert_eq!(lines, vec!["ok"], "unexpected payload enum output:\n{stdout}");
        Ok(())
    }

    #[test]
    fn test_method_alias_codegen_rewrites_to_target_method() -> Result<(), Box<dyn std::error::Error>> {
        let source = r#"
model Stats:
  value: int
  mean = avg

  def avg(self) -> int:
    return self.value

def main() -> None:
  let stats = Stats(value=10)
  println(stats.mean())
"#;
        let tokens = lexer::lex(source).map_err(|errors| std::io::Error::other(format!("lex failed: {errors:?}")))?;
        let ast =
            parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("parse failed: {errors:?}")))?;
        let rust_code = IrCodegen::new()
            .try_generate(&ast)
            .map_err(|error| std::io::Error::other(format!("codegen failed: {error:?}")))?;
        let avg_identity = rust_code
            .split(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
            .filter_map(|token| decode_incan_symbol_identity(token).ok().flatten())
            .find(|identity| identity.kind == SemanticSourceTargetKind::Method && identity.declaration_name == "avg")
            .ok_or_else(|| std::io::Error::other("generated Rust did not carry the target method identity"))?;
        let projection = encode_incan_symbol_identity(&avg_identity);
        assert!(
            rust_code.contains(&format!(".{projection}(")),
            "expected method alias call to lower to the target declaration's canonical projection, got:\n{rust_code}"
        );
        assert!(
            !rust_code.contains(".mean("),
            "method alias must not emit an independent wrapper call, got:\n{rust_code}"
        );
        Ok(())
    }

    #[test]
    fn test_run_c_import_this() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .args(["run", "-c", "import this"])
            // This test should not require network access. We expect the workspace dependencies to already be available
            // (the test suite built them)
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "incan run -c import this failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("The Zen of Incan") && stdout.contains("Readability counts"),
            "stdout missing zen line; got:\n{}",
            stdout
        );
        Ok(())
    }

    #[test]
    fn test_run_c_import_this_release_flag() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .args(["run", "--release", "-c", "import this"])
            // This test should not require network access. We expect the workspace dependencies to already be available
            // (the test suite built them)
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "incan run --release -c import this failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("The Zen of Incan") && stdout.contains("Readability counts"),
            "stdout missing zen line; got:\n{}",
            stdout
        );
        Ok(())
    }

    #[test]
    fn test_variadic_rest_calls_compile_and_run() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .args([
                "run",
                "-c",
                r#"
def collect(prefix: str, *items: int, **labels: str) -> int:
    mut total: int = 0
    for item in items:
        total = total + item
    if labels["name"] == "direct":
        return total
    if labels["name"] == "callable":
        return total
    return total

class Collector:
    def collect(self, *items: int, **labels: str) -> int:
        mut total: int = 0
        for item in items:
            total = total + item
        if labels["name"] == "method":
            return total
        return -100

def main() -> None:
    f = collect
    collector = Collector()
    println(collect("x", 1, 2, name="direct") + f("x", 4, 5, name="callable") + collector.collect(6, 7, name="method"))
"#,
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "variadic rest run-path regression failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = strip_ansi_escapes(&String::from_utf8_lossy(&output.stdout));
        let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        assert_eq!(lines, vec!["25"], "unexpected variadic rest output:\n{stdout}");
        Ok(())
    }

    #[test]
    fn local_partial_default_and_override_compile_and_run_issue1124() -> Result<(), Box<dyn std::error::Error>> {
        let stdout = compile_and_run_local_partial_project(
            r#"
def route(method: str, path: str, content_type: str = "text") -> str:
  return method + path + content_type

def main() -> None:
  get = partial route(method="GET")
  alias = get
  println(alias("/health") + "|" + alias(method="HEAD", path="/head"))
"#,
            "local_partial_default_contract",
        )?;
        let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        assert_eq!(
            lines,
            vec!["GET/healthtext|HEAD/headtext"],
            "unexpected local partial output:\n{stdout}"
        );
        Ok(())
    }

    #[test]
    fn generic_caller_inherits_selected_implementation_bounds_issue1280() -> Result<(), Box<dyn std::error::Error>> {
        let stdout = compile_and_run_local_partial_project(
            r#"
from std.derives.collection import FallibleIterator

model Stream[R] with FallibleIterator[int, str]:
    value: R

    def __next__(mut self) -> Result[Option[int], str]:
        return Ok(None)

pub def drain[R](value: R) -> Result[int, str]:
    mut count = 0
    for _item in Stream[R](value=value)?:
        count += 1
    return Ok(count)

pub def relay[R](value: R) -> Result[int, str]:
    return drain(value)

def main() -> Result[None, str]:
    println(relay("ready")?)
    return Ok(None)
"#,
            "generic_implementation_bound_contract",
        )?;
        let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        assert_eq!(
            lines,
            vec!["0"],
            "unexpected generic implementation-bound output:\n{stdout}"
        );
        Ok(())
    }

    #[test]
    fn local_partial_runtime_preset_captures_at_construction_issue1124() -> Result<(), Box<dyn std::error::Error>> {
        let stdout = compile_and_run_local_partial_project(
            r#"
def route(method: int, path: int, content_type: int = 3) -> int:
  return method + path + content_type

def main() -> None:
  mut method = 1
  get = partial route(method=method)
  method = 2
  println(get(4))
  println(get(method=7, path=4))
"#,
            "local_partial_capture_contract",
        )?;
        let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        assert_eq!(
            lines,
            vec!["8", "14"],
            "the omitted preset must retain its construction-time value while a named call overrides it:\n{stdout}"
        );
        Ok(())
    }

    #[test]
    fn test_decorated_variadic_callables_compile_and_run() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .args([
                "run",
                "-c",
                r#"
def preserve[F]() -> ((F) -> F):
    return (func) => func

@preserve()
pub def decorated_total(first: int, second: int, *rest: int, **labels: str) -> int:
    mut total: int = first + second
    for value in rest:
        total = total + value
    if labels["mode"] == "sum":
        return total
    return -1

class Box:
    base: int

    @preserve()
    def total(self, first: int, *rest: int, **labels: str) -> int:
        mut total: int = self.base + first
        for value in rest:
            total = total + value
        if labels["mode"] == "sum":
            return total
        return -1

def main() -> None:
    box = Box(base=5)
    println(decorated_total(1, 2, 3, 4, mode="sum") + box.total(6, 7, 8, mode="sum"))
"#,
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "decorated variadic callable regression failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = strip_ansi_escapes(&String::from_utf8_lossy(&output.stdout));
        let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        assert_eq!(lines, vec!["36"], "unexpected decorated variadic output:\n{stdout}");
        Ok(())
    }

    #[test]
    fn test_decorated_variadic_library_builds() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let root = tmp.path();
        fs::create_dir_all(root.join("src"))?;
        fs::write(
            root.join("loaf.toml"),
            "[project]\nname = \"decorated_rest_lib\"\nversion = \"0.1.0\"\n",
        )?;
        fs::write(
            root.join("src/lib.incn"),
            r#"
def preserve[F]() -> ((F) -> F):
    return (func) => func

@preserve()
pub def decorated_total(first: int, second: int, *rest: int, **labels: str) -> int:
    mut total: int = first + second
    for value in rest:
        total = total + value
    if labels["mode"] == "sum":
        return total
    return -1

pub class Box:
    base: int

    @preserve()
    def total(self, first: int, *rest: int, **labels: str) -> int:
        mut total: int = self.base + first
        for value in rest:
            total = total + value
        if labels["mode"] == "sum":
            return total
        return -1
"#,
        )?;

        let mut command = incan_command();
        let output = command
            .args(["build", "--lib"])
            .current_dir(root)
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "decorated variadic library build failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(())
    }

    #[test]
    fn test_string_and_bytes_iteration_compile_and_run() -> Result<(), Box<dyn std::error::Error>> {
        let output = run_incan_source(
            "def main() -> None:\n  mut out = \"\"\n  for ch in \"Az\":\n    out += ch\n  for index, ch in enumerate(\"xy\"):\n    out += f\"{index}{ch}\"\n  mut total = 0\n  for byte in b\"Az\":\n    total += byte\n  for index, byte in enumerate(b\"\\x01\\x02\"):\n    total += index + byte\n  println(out)\n  println(total)\n",
        );

        assert!(
            output.status.success(),
            "incan run string/bytes iteration regression failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        let lines = stdout.lines().collect::<Vec<_>>();
        assert_eq!(lines, vec!["Az0x1y", "191"]);

        Ok(())
    }

    #[test]
    fn test_std_fs_compile_and_run_path_file_and_tree_operations() -> Result<(), Box<dyn std::error::Error>> {
        let base = std::env::temp_dir().join(format!("incan_std_fs_integration_{}", std::process::id()));
        let root = base.join("root");
        let copied = base.join("copy");
        let moved = base.join("moved");
        let source = format!(
            r#"
from std.fs import IoError, OpenOptions, Path
from std.tempfile import NamedTemporaryFile, SpooledTemporaryFile, TemporaryDirectory
from rust::std::thread import sleep
from rust::std::time import Duration

def run() -> Result[None, IoError]:
    root = Path("{root}")
    copied = Path("{copied}")
    moved = Path("{moved}")
    if moved.exists():
        moved.remove_tree()?
    if copied.exists():
        copied.remove_tree()?
    if root.exists():
        root.remove_tree()?
    root.mkdir(true, true)?
    root.joinpath("a.txt").write_text("alpha", "utf-8", "strict", None)?
    root.joinpath("c.md").write_text("charlie", "utf-8", "strict", None)?
    root.joinpath("sub").mkdir(true, true)?
    root.joinpath("sub").joinpath("b.txt").write_text("bravo", "utf-8", "strict", None)?
    println(len(root.glob("*.txt")?))
    println(len(root.rglob("*.txt")?))
    println(len(root.rglob("sub/[ab].txt")?))
    match root.joinpath("a.txt").open("r", -1, Some("definitely-not-an-encoding"), None, None):
        Ok(_) => println("bad")
        Err(err) => println(err.kind)
    match root.joinpath("a.txt").open("rbb+", -1, None, None, None):
        Ok(_) => println("bad")
        Err(err) => println(err.kind)
    default_reader = root.joinpath("a.txt").open()?
    println(default_reader.read(-1)?)
    default_out = root.joinpath("default-open.txt")
    default_writer = default_out.open("w")?
    default_writer.write("delta")?
    default_writer.flush()?
    println(default_out.read_text("utf-8", "strict")?)
    latin = root.joinpath("latin.txt")
    latin.write_bytes(b"\xff")?
    println(len(latin.read_text("windows-1252", "strict")?) > 0)
    match latin.read_text("utf-8", "strict"):
        Ok(_) => println("bad")
        Err(err) => println(err.kind)
    println(latin.read_text("utf-8", "replace")? != "")
    latin_out = root.joinpath("latin-out.txt")
    latin_out.write_text("€", "windows-1252", "strict", None)?
    println(latin_out.read_text("windows-1252", "strict")? == "€")
    latin_handle_out = root.joinpath("latin-handle-out.txt")
    latin_handle = latin_handle_out.open("w", -1, Some("windows-1252"), Some("strict"), None)?
    latin_handle.write("€")?
    latin_handle.flush()?
    println(latin_handle_out.read_text("windows-1252", "strict")? == "€")
    text_handle = latin.open("r", -1, Some("windows-1252"), Some("strict"), None)?
    println(len(text_handle.read(-1)?) > 0)
    options_file = OpenOptions().write(true).create(true).truncate(true).open(root.joinpath("options.txt"))?
    options_file.write_bytes(b"opts")?
    options_file.flush()?
    println(root.joinpath("options.txt").read_text("utf-8", "strict")?)
    handle = root.joinpath("a.txt").open("rb", 0, None, None, None)?
    chunk = handle.read_exact(2)?
    println(len(chunk))
    source_modified = root.joinpath("a.txt").stat()?.modified_unix()?
    root.copy(copied, true, true)?
    copied_text = copied.joinpath("sub").joinpath("b.txt").read_text("utf-8", "strict")?
    println(copied_text)
    copied_modified = copied.joinpath("a.txt").stat()?.modified_unix()?
    println(copied_modified == source_modified)
    sleep(Duration.from_secs(1))
    copied.joinpath("a.txt").touch(true)?
    touched_modified = copied.joinpath("a.txt").stat()?.modified_unix()?
    println(touched_modified > copied_modified)
    copied.move(moved)?
    println(moved.joinpath("a.txt").exists())
    stat = moved.joinpath("a.txt").stat()?
    println(stat.modified_unix()? > 0)
    usage = moved.disk_usage()?
    println(usage.total > 0 and usage.free > 0)

    staged = root.joinpath("published.next")
    published = root.joinpath("published.txt")
    staged.write_text("published", "utf-8", "strict", None)?
    staged.replace(published)?
    root.sync_directory()?
    println(published.read_text("utf-8", "strict")?)
    match published.sync_directory():
        Ok(_) => println("bad")
        Err(err) => println(err.kind)
    rejected_staged = root.joinpath("rejected.next")
    rejected_target = root.joinpath("rejected-directory")
    rejected_staged.write_text("new", "utf-8", "strict", None)?
    rejected_target.mkdir(false, false)?
    match rejected_staged.replace(rejected_target):
        Ok(_) => println("bad")
        Err(err) => println(err.kind)
    println(rejected_target.is_dir())
    println(rejected_staged.exists())
    held_lock = published.lock_exclusive()?
    match published.try_lock_exclusive()?:
        Some(_) => println("bad")
        None => println("contended")

    file = NamedTemporaryFile.try_new_with("incan-", ".txt", None)?
    path = file.path()
    path.write_text("hello", "utf-8", "strict", None)?
    println(path.read_text("utf-8", "strict")?)

    directory = TemporaryDirectory.try_new_with("incan-dir-", "", None)?
    child = directory.path() / "child.txt"
    child.write_text("world", "utf-8", "strict", None)?
    println(child.read_text("utf-8", "strict")?)

    mut memory = SpooledTemporaryFile(max_size=64)
    memory.write(b"memory")?
    println(memory.rolled_to_disk())
    memory.seek(0, 0)?
    println(len(memory.read(-1)?))

    mut spool = SpooledTemporaryFile(max_size=4)
    spool.write(b"rolled")?
    println(spool.rolled_to_disk())
    println(spool.path()?.exists())
    spool.seek(0, 0)?
    println(len(spool.read(-1)?))
    kept_spool = spool.persist()?
    println(kept_spool.exists())
    kept_spool.unlink()?

    kept_file = file.persist()?
    println(kept_file.exists())
    kept_file.unlink()?

    kept_directory = directory.persist()?
    println(kept_directory.exists())
    kept_directory.remove_tree()?

    moved.remove_tree()?
    root.remove_tree()?
    return Ok(None)

def main() -> None:
    match run():
        Ok(_) => pass
        Err(err) => println(err.message())
"#,
            root = root.display(),
            copied = copied.display(),
            moved = moved.display()
        );
        let output = incan_command()
            .args(["run", "-c", source.as_str()])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "incan run std.fs smoke failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        let lines = stdout.lines().collect::<Vec<_>>();
        assert_eq!(
            lines,
            vec![
                "1",
                "2",
                "1",
                "invalid_input",
                "invalid_input",
                "alpha",
                "delta",
                "true",
                "invalid_data",
                "true",
                "true",
                "true",
                "true",
                "opts",
                "2",
                "bravo",
                "true",
                "true",
                "true",
                "true",
                "true",
                "published",
                "invalid_input",
                "invalid_input",
                "true",
                "true",
                "contended",
                "hello",
                "world",
                "false",
                "6",
                "true",
                "true",
                "6",
                "true",
                "true",
                "true"
            ],
            "unexpected std.fs output:\n{stdout}"
        );
        Ok(())
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_std_fs_replace_rejects_cross_device_without_losing_target() -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::MetadataExt;

        let source_root = tempfile::tempdir()?;
        let shared_memory = Path::new("/dev/shm");
        if !shared_memory.is_dir() || fs::metadata(source_root.path())?.dev() == fs::metadata(shared_memory)?.dev() {
            return Ok(());
        }
        let target_root = shared_memory.join(format!(
            "incan_std_fs_cross_device_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos()
        ));
        fs::create_dir_all(&target_root)?;
        let staged = source_root.path().join("staged.txt");
        let target = target_root.join("published.txt");
        let source = format!(
            r#"
from std.fs import IoError, Path

def run() -> Result[None, IoError]:
    staged = Path("{staged}")
    target = Path("{target}")
    staged.write_text("new", "utf-8", "strict", None)?
    target.write_text("old", "utf-8", "strict", None)?
    match staged.replace(target):
        Ok(_) => println("bad")
        Err(err) => println(err.kind)
    println(target.read_text("utf-8", "strict")?)
    println(staged.exists())
    return Ok(None)

def main() -> Result[None, IoError]:
    run()?
    return Ok(None)
"#,
            staged = staged.display(),
            target = target.display(),
        );
        let output = incan_command()
            .args(["run", "-c", source.as_str()])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        let _ = fs::remove_dir_all(&target_root);
        assert!(
            output.status.success(),
            "cross-device replace smoke failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).lines().collect::<Vec<_>>(),
            vec!["cross_device", "old", "true"],
            "cross-device replacement must preserve the target and staged source"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn test_std_fs_locks_coordinate_between_incan_processes() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let target = root.path().join("published.lock");
        let ready = root.path().join("holder-ready");
        let holder_path = root.path().join("holder.incn");
        let holder_source = format!(
            r#"
from std.fs import IoError, Path
from rust::std::thread import sleep
from rust::std::time import Duration

def hold() -> Result[None, IoError]:
    guard = Path("{target}").lock_shared()?
    Path("{ready}").write_text("ready", "utf-8", "strict", None)?
    # The Rust harness terminates this generated program after the direct probe finishes, so this
    # is a liveness ceiling rather than test latency.
    sleep(Duration.from_secs(300))
    return Ok(None)

def main() -> None:
    match hold():
        Ok(_) => pass
        Err(err) => println(err.message())
"#,
            target = target.display(),
            ready = ready.display(),
        );
        fs::write(&holder_path, holder_source)?;
        let holder_binary = build_incan_source_binary(&holder_path, &root.path().join("holder-build"))?;

        let probe_path = root.path().join("probe.incn");
        let probe_source = format!(
            r#"
from std.fs import IoError, Path

def main() -> None:
    match Path("{target}").try_lock_shared():
        Ok(Some(_)) => println("shared")
        Ok(None) => println("blocked")
        Err(err) => println(err.message())
    match Path("{target}").try_lock_exclusive():
        Ok(Some(_)) => println("acquired")
        Ok(None) => println("contended")
        Err(err) => println(err.message())
"#,
            target = target.display(),
        );
        fs::write(&probe_path, probe_source)?;
        let probe_binary = build_incan_source_binary(&probe_path, &root.path().join("probe-build"))?;

        let mut holder = Command::new(&holder_binary)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        // A cold CI runner must compile the generated holder project before it can create the readiness file. Keep
        // this deadline comfortably above observed cold pinned-toolchain compilation time while retaining a finite
        // failure bound.
        let holder_ready_started = std::time::Instant::now();
        let holder_ready_timeout = Duration::from_secs(120);
        let mut holder_ready = false;
        while holder_ready_started.elapsed() < holder_ready_timeout {
            if ready.exists() {
                holder_ready = true;
                break;
            }
            thread::sleep(Duration::from_millis(25));
        }
        if !holder_ready {
            let _ = holder.kill();
            let output = holder.wait_with_output()?;
            return Err(format!(
                "Incan lock holder did not become ready. stdout:\n{}\nstderr:\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }

        let probes = Command::new(&probe_binary).output();
        let _ = holder.kill();
        let holder_output = holder.wait_with_output()?;
        let probes = probes?;

        assert!(
            probes.status.success(),
            "lock probes failed. stdout:\n{}\nstderr:\n{}\nholder stdout:\n{}\nholder stderr:\n{}",
            String::from_utf8_lossy(&probes.stdout),
            String::from_utf8_lossy(&probes.stderr),
            String::from_utf8_lossy(&holder_output.stdout),
            String::from_utf8_lossy(&holder_output.stderr),
        );
        assert_eq!(String::from_utf8_lossy(&probes.stdout).trim(), "shared\ncontended");
        Ok(())
    }
}
