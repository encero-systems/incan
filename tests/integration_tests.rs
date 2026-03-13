//! Integration tests for the Incan compiler frontend

use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use incan::frontend::{lexer, parser, typechecker};

/// Helper to run full pipeline on a source file
fn compile_file(path: &Path) -> Result<(), Vec<String>> {
    let source = fs::read_to_string(path).map_err(|e| vec![e.to_string()])?;
    compile_source(&source)
}

fn compile_source(source: &str) -> Result<(), Vec<String>> {
    let tokens = lexer::lex(source).map_err(|errs| errs.iter().map(|e| e.message.clone()).collect::<Vec<_>>())?;

    let ast = parser::parse(&tokens).map_err(|errs| errs.iter().map(|e| e.message.clone()).collect::<Vec<_>>())?;

    typechecker::check(&ast).map_err(|errs| errs.iter().map(|e| e.message.clone()).collect::<Vec<_>>())?;

    Ok(())
}

fn strip_ansi_escapes(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' && chars.peek() == Some(&'[') {
            let _ = chars.next();
            for c in chars.by_ref() {
                if c == 'm' {
                    break;
                }
            }
            continue;
        }
        out.push(ch);
    }
    out
}

fn is_incan_fixture(path: &Path) -> bool {
    matches!(path.extension().and_then(|e| e.to_str()), Some("incn") | Some("incan"))
}

/// Make a temporary test directory to be able to run the CLI tests.
fn make_temp_test_dir() -> std::path::PathBuf {
    let mut dir = std::env::temp_dir();
    let uniq = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    dir.push(format!("incan_cli_test_{}", uniq));
    let Ok(()) = std::fs::create_dir_all(&dir) else {
        panic!("failed to create temp test dir");
    };
    dir
}

/// Regression: float compound-assign with int RHS should typecheck (Python-like / promotion).
#[test]
fn test_compound_assign_float_with_int_rhs() {
    let program = r#"
def main() -> None:
    mut y: float = 100.0
    y /= 3
    y %= 7
    println(y)
"#;

    let result = compile_source(program);
    assert!(result.is_ok(), "Expected program to typecheck, got {:?}", result.err());
}

/// Test that all valid fixtures compile successfully
#[test]
fn test_valid_fixtures() {
    let fixtures_dir = Path::new("tests/fixtures/valid");
    if !fixtures_dir.exists() {
        return; // Skip if fixtures not present
    }

    let mut matched = 0usize;
    let Ok(entries) = fs::read_dir(fixtures_dir) else {
        panic!("failed to read directory {}", fixtures_dir.display());
    };
    for entry in entries {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        if is_incan_fixture(&path) {
            matched += 1;
            let result = compile_file(&path);
            if let Err(errs) = result {
                panic!(
                    "Expected {} to compile successfully, got errors: {:?}",
                    path.display(),
                    errs
                );
            }
        }
    }
    assert!(matched > 0, "No .incn fixtures found in {}", fixtures_dir.display());
}

/// Test that invalid fixtures produce errors
#[test]
fn test_invalid_fixtures() {
    let fixtures_dir = Path::new("tests/fixtures/invalid");
    if !fixtures_dir.exists() {
        return; // Skip if fixtures not present
    }

    let mut matched = 0usize;
    let Ok(entries) = fs::read_dir(fixtures_dir) else {
        panic!("failed to read directory {}", fixtures_dir.display());
    };
    for entry in entries {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        if is_incan_fixture(&path) {
            matched += 1;
            let result = compile_file(&path);
            assert!(
                result.is_err(),
                "Expected {} to fail compilation, but it succeeded",
                path.display()
            );
        }
    }
    assert!(matched > 0, "No .incn fixtures found in {}", fixtures_dir.display());
}

#[test]
fn test_help_is_banner_free() {
    let Ok(output) = Command::new("target/debug/incan").arg("--help").output() else {
        panic!("failed to run incan --help");
    };
    assert!(
        output.status.success(),
        "incan --help failed: status={:?} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stdout.contains("░░███") && !stderr.contains("░░███"),
        "logo leaked into help output"
    );
}

#[test]
fn test_version_is_single_line_and_banner_free() {
    let Ok(output) = Command::new("target/debug/incan").arg("--version").output() else {
        panic!("failed to run incan --version");
    };
    assert!(
        output.status.success(),
        "incan --version failed: status={:?} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stdout.contains("░░███") && !stderr.contains("░░███"),
        "logo leaked into version output"
    );
    assert_eq!(stdout.lines().count(), 1, "expected single-line version output");
}

#[test]
fn test_parse_error_is_banner_free() {
    let Ok(output) = Command::new("target/debug/incan")
        .arg("--definitely-not-a-flag")
        .output()
    else {
        panic!("failed to run incan with invalid args");
    };
    assert!(
        !output.status.success(),
        "expected invalid args to fail, status={:?}",
        output.status
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stdout.contains("░░███") && !stderr.contains("░░███"),
        "logo leaked into parse error output"
    );
}

#[test]
fn test_fstring_unknown_symbol_cli_caret_points_to_interpolation() {
    let source = "def main() -> str:\n  return f\"value: {unknown_var}\"\n";
    let Ok(output) = Command::new("target/debug/incan").args(["run", "-c", source]).output() else {
        panic!("failed to run incan with f-string source");
    };

    assert!(
        !output.status.success(),
        "expected unknown symbol compilation failure, status={:?}",
        output.status
    );

    let stderr_colored = String::from_utf8_lossy(&output.stderr);
    let stderr = strip_ansi_escapes(&stderr_colored);
    assert!(
        stderr.contains("Unknown symbol 'unknown_var'"),
        "expected unknown symbol diagnostic in stderr, got:\n{}",
        stderr
    );
    assert!(
        stderr.contains("return f\"value: {unknown_var}\""),
        "expected source line in diagnostic, got:\n{}",
        stderr
    );

    let caret_line = match stderr.lines().find(|line| line.contains('^')) {
        Some(line) => line,
        None => panic!("expected caret line in diagnostic, got:\n{}", stderr),
    };

    let mut max_caret_run = 0usize;
    let mut current_run = 0usize;
    for c in caret_line.chars() {
        if c == '^' {
            current_run += 1;
            if current_run > max_caret_run {
                max_caret_run = current_run;
            }
        } else {
            current_run = 0;
        }
    }

    assert_eq!(
        max_caret_run,
        "{unknown_var}".len(),
        "expected caret width to match interpolation span; stderr:\n{}",
        stderr
    );
}

#[test]
fn test_fail_on_empty_collection() {
    let dir = make_temp_test_dir();
    let test_file = dir.join("test_empty.incn");
    let Ok(()) = std::fs::write(
        &test_file,
        r#"
def helper() -> Unit:
  pass
"#,
    ) else {
        panic!("failed to write test file");
    };

    let Ok(output) = Command::new("target/debug/incan")
        .args(["test", dir.to_string_lossy().as_ref()])
        .output()
    else {
        panic!("failed to run incan test");
    };
    assert!(
        output.status.success(),
        "expected empty collection to succeed by default: status={:?} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );

    let Ok(output) = Command::new("target/debug/incan")
        .args(["test", "--fail-on-empty", dir.to_string_lossy().as_ref()])
        .output()
    else {
        panic!("failed to run incan test --fail-on-empty");
    };
    assert!(
        !output.status.success(),
        "expected empty collection to fail with --fail-on-empty: status={:?} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Test specific lexer behavior
mod lexer_tests {
    use incan::frontend::lexer::{TokenKind, lex};
    use incan_core::lang::keywords::KeywordId;
    use incan_core::lang::operators::OperatorId;
    use incan_core::lang::punctuation::PunctuationId;

    #[test]
    fn test_floor_div_tokens() {
        let Ok(tokens) = lex("a //= b\nc // d") else {
            panic!("lex failed");
        };
        let has_floor_div_eq = tokens.iter().any(|t| t.kind.is_operator(OperatorId::SlashSlashEq));
        let has_floor_div = tokens.iter().any(|t| t.kind.is_operator(OperatorId::SlashSlash));
        assert!(has_floor_div_eq, "expected to see //= token");
        assert!(has_floor_div, "expected to see // token");
    }

    #[test]
    fn test_rust_style_imports() {
        let Ok(tokens) = lex("import foo::bar::baz as fb") else {
            panic!("lex failed");
        };
        assert!(tokens[0].kind.is_keyword(KeywordId::Import));
        assert!(matches!(&tokens[1].kind, TokenKind::Ident(s) if s == "foo"));
        assert!(tokens[2].kind.is_punctuation(PunctuationId::ColonColon));
        assert!(matches!(&tokens[3].kind, TokenKind::Ident(s) if s == "bar"));
        assert!(tokens[4].kind.is_punctuation(PunctuationId::ColonColon));
        assert!(matches!(&tokens[5].kind, TokenKind::Ident(s) if s == "baz"));
        assert!(tokens[6].kind.is_keyword(KeywordId::As));
        assert!(matches!(&tokens[7].kind, TokenKind::Ident(s) if s == "fb"));
    }

    #[test]
    fn test_try_operator() {
        let Ok(tokens) = lex("result?") else {
            panic!("lex failed");
        };
        assert!(matches!(&tokens[0].kind, TokenKind::Ident(s) if s == "result"));
        assert!(tokens[1].kind.is_punctuation(PunctuationId::Question));
    }

    #[test]
    fn test_fat_arrow() {
        let Ok(tokens) = lex("x => y") else {
            panic!("lex failed");
        };
        assert!(tokens[1].kind.is_punctuation(PunctuationId::FatArrow));
    }

    #[test]
    fn test_case_keyword() {
        let Ok(tokens) = lex("case Some(x):") else {
            panic!("lex failed");
        };
        assert!(tokens[0].kind.is_keyword(KeywordId::Case));
    }

    #[test]
    fn test_pass_keyword() {
        let Ok(tokens) = lex("pass") else {
            panic!("lex failed");
        };
        assert!(tokens[0].kind.is_keyword(KeywordId::Pass));
    }

    #[test]
    fn test_mut_self() {
        let Ok(tokens) = lex("mut self") else {
            panic!("lex failed");
        };
        assert!(tokens[0].kind.is_keyword(KeywordId::Mut));
        assert!(tokens[1].kind.is_keyword(KeywordId::SelfKw));
    }

    #[test]
    fn test_fstring() {
        let Ok(tokens) = lex(r#"f"Hello {name}""#) else {
            panic!("lex failed");
        };
        assert!(matches!(&tokens[0].kind, TokenKind::FString(_)));
    }

    #[test]
    fn test_yield_keyword() {
        let Ok(tokens) = lex("yield value") else {
            panic!("lex failed");
        };
        assert!(tokens[0].kind.is_keyword(KeywordId::Yield));
        assert!(matches!(&tokens[1].kind, TokenKind::Ident(s) if s == "value"));
    }

    #[test]
    fn test_rust_keyword() {
        let Ok(tokens) = lex("import rust::serde_json") else {
            panic!("lex failed");
        };
        assert!(tokens[0].kind.is_keyword(KeywordId::Import));
        assert!(tokens[1].kind.is_keyword(KeywordId::Rust));
        assert!(tokens[2].kind.is_punctuation(PunctuationId::ColonColon));
        assert!(matches!(&tokens[3].kind, TokenKind::Ident(s) if s == "serde_json"));
    }
}

mod numeric_semantics_tests {
    use incan::frontend::{lexer, parser, typechecker};

    #[test]
    fn test_python_like_numeric_ops_compile() {
        let source = r#"
def main() -> None:
  a: int = 7
  b: int = -3
  x = a / b       # float
  y = a // b      # floor div
  z = a % b       # python remainder
  f: float = 7.0
  g = f % 2.0
  h = f // 2.0
"#;
        let Ok(tokens) = lexer::lex(source) else {
            panic!("lexing failed");
        };
        let Ok(ast) = parser::parse(&tokens) else {
            panic!("parse failed");
        };
        let Ok(()) = typechecker::check(&ast) else {
            panic!("typecheck failed");
        };
    }
}

/// End-to-end codegen tests
mod codegen_tests {
    use incan::backend::IrCodegen;
    use incan::frontend::{lexer, parser, typechecker};
    use std::fs;
    use std::path::Path;
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn rustc_compile_ok(source: &str) -> Result<(), String> {
        let mut dir = std::env::temp_dir();
        let Ok(duration) = SystemTime::now().duration_since(UNIX_EPOCH) else {
            panic!("system time before UNIX epoch");
        };
        let uniq = duration.as_nanos();
        dir.push(format!("incan_bench_smoke_{}", uniq));
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

        let rs_path = dir.join("main.rs");
        let bin_path = dir.join("bin");
        std::fs::write(&rs_path, source).map_err(|e| e.to_string())?;

        let out = Command::new("rustc")
            .arg("--edition=2021")
            .arg(&rs_path)
            .arg("-o")
            .arg(&bin_path)
            .output()
            .map_err(|e| e.to_string())?;

        if out.status.success() {
            Ok(())
        } else {
            Err(String::from_utf8_lossy(&out.stderr).to_string())
        }
    }

    fn make_temp_dir(prefix: &str) -> std::path::PathBuf {
        let mut dir = std::env::temp_dir();
        let Ok(duration) = SystemTime::now().duration_since(UNIX_EPOCH) else {
            panic!("system time before UNIX epoch");
        };
        let uniq = duration.as_nanos();
        dir.push(format!("{}_{}", prefix, uniq));
        let Ok(()) = std::fs::create_dir_all(&dir) else {
            panic!("failed to create temp dir");
        };
        dir
    }

    #[test]
    fn test_hello_world_codegen() {
        let path = Path::new("examples/hello.incn");
        if !path.exists() {
            return; // Skip if example not present
        }

        let Ok(source) = fs::read_to_string(path) else {
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
    fn test_run_c_import_this() {
        let Ok(output) = Command::new("target/debug/incan")
            .args(["run", "-c", "import this"])
            // This test should not require network access. We expect the workspace dependencies to already be available
            // (the test suite built them)
            .env("CARGO_NET_OFFLINE", "true")
            .output()
        else {
            panic!("failed to run incan");
        };
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
    }

    #[test]
    fn test_build_web_route_uses_proc_macro_passthrough() {
        let project_dir = make_temp_dir("incan_web_proc_macro_test");
        let source_path = project_dir.join("main.incn");
        let out_dir = project_dir.join("out");
        let source = r#"
import std.async
from std.web import route

@route("/health")
async def health() -> str:
    return "ok"

def main() -> None:
    pass
"#;
        let Ok(()) = std::fs::write(&source_path, source) else {
            panic!("failed to write source file");
        };

        let Ok(output) = Command::new("target/debug/incan")
            .args([
                "build",
                source_path.to_string_lossy().as_ref(),
                out_dir.to_string_lossy().as_ref(),
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()
        else {
            panic!("failed to run incan build");
        };

        assert!(
            output.status.success(),
            "incan build web route failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );

        let generated_main = out_dir.join("src/main.rs");
        let Ok(main_rs) = std::fs::read_to_string(&generated_main) else {
            panic!("failed to read generated Rust source");
        };
        assert!(
            main_rs.contains("#[incan_web_macros::route("),
            "expected generated web route to use proc macro passthrough:\n{}",
            main_rs
        );
        assert!(
            !main_rs.contains("__incan_router!"),
            "legacy __incan_router! macro should not be emitted:\n{}",
            main_rs
        );
        assert!(
            !main_rs.contains("set_router"),
            "legacy set_router() call should not be emitted:\n{}",
            main_rs
        );
    }

    #[test]
    fn test_run_async_channel_facade() {
        let project_dir = make_temp_dir("incan_async_channel_facade_test");
        let source_path = project_dir.join("main.incn");
        let source = r#"
import std.async
from std.async.channel import channel, unbounded_channel, oneshot

async def main() -> None:
    tx, rx = channel(4)
    cloned = tx.clone()

    match await cloned.send(1):
        Ok(_) => println("sent")
        Err(err) => println(err.message())

    match await rx.recv():
        Some(value) => println(value)
        None => println("closed")

    tx2, rx2 = unbounded_channel()
    match await tx2.send(2):
        Ok(_) => println("sent")
        Err(err) => println(err.message())

    match rx2.try_recv():
        Some(value) => println(value)
        None => println("empty")

    rx2.close()
    println(tx2.is_closed())

    otx, orx = oneshot()
    match otx.send(3):
        Ok(_) => println("delivered")
        Err(value) => println(value)

    match await orx.recv():
        Ok(value) => println(value)
        Err(err) => println(err.message())
"#;
        let Ok(()) = std::fs::write(&source_path, source) else {
            panic!("failed to write source file");
        };

        let Ok(output) = Command::new("target/debug/incan")
            .args(["run", source_path.to_string_lossy().as_ref()])
            .env("CARGO_NET_OFFLINE", "true")
            .output()
        else {
            panic!("failed to run incan");
        };

        assert!(
            output.status.success(),
            "incan run async channel facade failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("sent"), "expected send output; got:\n{}", stdout);
        assert!(
            stdout.contains("1"),
            "expected bounded receive output; got:\n{}",
            stdout
        );
        assert!(
            stdout.contains("2"),
            "expected unbounded receive output; got:\n{}",
            stdout
        );
        assert!(
            stdout.contains("true"),
            "expected closed-state output; got:\n{}",
            stdout
        );
        assert!(
            stdout.contains("delivered"),
            "expected oneshot send output; got:\n{}",
            stdout
        );
        assert!(
            stdout.contains("3"),
            "expected oneshot receive output; got:\n{}",
            stdout
        );
    }

    #[test]
    fn test_run_repro_model_traits() {
        let Ok(output) = Command::new("target/debug/incan")
            .args(["run", "tests/fixtures/repro_model_traits.incn"])
            // This should not require network access (workspace deps should already be available).
            .env("CARGO_NET_OFFLINE", "true")
            .output()
        else {
            panic!("failed to run incan");
        };

        assert!(
            output.status.success(),
            "incan run repro_model_traits failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("[Ada] hello"),
            "expected repro output; got:\n{}",
            stdout
        );
    }

    /// RFC 021: Runtime verification that __fields__() returns correct FieldInfo values
    #[test]
    fn test_run_field_info_reflection() {
        let Ok(output) = Command::new("target/debug/incan")
            .args(["run", "tests/fixtures/field_info_reflection.incn"])
            .env("CARGO_NET_OFFLINE", "true")
            .output()
        else {
            panic!("failed to run incan");
        };

        assert!(
            output.status.success(),
            "incan run field_info_reflection failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = String::from_utf8_lossy(&output.stdout);

        // Verify __class_name__
        assert!(
            stdout.contains("Account"),
            "expected __class_name__ to return 'Account'; got:\n{}",
            stdout
        );

        // Verify field info for type_ (has alias)
        assert!(
            stdout.contains("field:type_|wire:type|type:str|default:false"),
            "expected type_ field info with alias='type'; got:\n{}",
            stdout
        );

        // Verify field info for balance (has default)
        assert!(
            stdout.contains("field:balance|wire:balance|type:int|default:true"),
            "expected balance field info with default=true; got:\n{}",
            stdout
        );

        // Verify field info for name (no alias, no default)
        assert!(
            stdout.contains("field:name|wire:name|type:str|default:false"),
            "expected name field info; got:\n{}",
            stdout
        );

        // Empty models should produce no FieldInfo entries
        assert!(
            stdout.contains("empty_fields:0"),
            "expected empty model to return 0 fields; got:\n{}",
            stdout
        );

        // Nested generics should use Incan type formatting
        assert!(
            stdout.contains("settings_field:complex|type:list[dict[str, int]]"),
            "expected nested generic type name; got:\n{}",
            stdout
        );

        // User-defined field types should use their Incan type name
        assert!(
            stdout.contains("user_field:address|type:Address"),
            "expected user-defined field type name; got:\n{}",
            stdout
        );

        // Inherited class fields should appear in __fields__()
        assert!(
            stdout.contains("child_field:base_id|type:int"),
            "expected inherited base field in __fields__; got:\n{}",
            stdout
        );
        assert!(
            stdout.contains("child_field:name|type:str"),
            "expected child field in __fields__; got:\n{}",
            stdout
        );
    }

    #[test]
    fn test_benchmark_quicksort_codegen_compiles() {
        let path = Path::new("benchmarks/sorting/quicksort/quicksort.incn");
        if !path.exists() {
            return;
        }

        let Ok(source) = fs::read_to_string(path) else {
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

        // Regression: Vec::swap indices must be cast to usize.
        let mut ok = true;
        let mut search_from = 0usize;
        while let Some(pos) = rust_code[search_from..].find(".swap(") {
            let abs = search_from + pos;
            let window_end = (abs + 120).min(rust_code.len());
            let window = &rust_code[abs..window_end];
            if !window.contains("as usize") {
                ok = false;
                break;
            }
            search_from = abs + 5;
        }
        assert!(
            ok,
            "expected quicksort to cast swap indices to usize; generated:\n{}",
            rust_code
        );

        // Note: This test uses standalone rustc compilation, which can't access incan_stdlib/incan_derive.
        // Skip the compilation check if stdlib imports are present (models/classes with derives).
        if rust_code.contains("use incan_stdlib::prelude") || rust_code.contains("use incan_derive") {
            // Skip rustc compilation test for code that requires stdlib crates
            return;
        }

        let Ok(()) = rustc_compile_ok(&rust_code) else {
            panic!("generated quicksort Rust failed to compile");
        };
    }

    #[test]
    fn test_const_declarations_compile_and_run() {
        let Ok(output) = Command::new("target/debug/incan")
            .args([
                "run",
                "-c",
                r#"
const PI: float = 3.14159
const APP_NAME: str = "Incan"
const MAGIC: int = 42
const ENABLED: bool = true
const RAW_DATA: bytes = b"\x00\x01\x02\x03"
const FROZEN_TEXT: FrozenStr = "frozen"
const NUMBERS: FrozenList[int] = [1, 2, 3, 4, 5]
const GREETING: str = "Hello World"

def main() -> None:
    print(PI)
    print(APP_NAME)
    print(MAGIC)
    print(ENABLED)
    print(RAW_DATA.len())
    print(FROZEN_TEXT.len())
    print(NUMBERS.len())
    print(GREETING)
"#,
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()
        else {
            panic!("failed to run incan");
        };

        assert!(
            output.status.success(),
            "const declarations test failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("3.14159"), "PI const not emitted correctly");
        assert!(stdout.contains("Incan"), "APP_NAME const not emitted correctly");
        assert!(stdout.contains("42"), "MAGIC const not emitted correctly");
        assert!(stdout.contains("true"), "ENABLED const not emitted correctly");
        assert!(stdout.contains("4"), "RAW_DATA length incorrect");
        assert!(stdout.contains("6"), "FROZEN_TEXT length incorrect");
        assert!(stdout.contains("5"), "NUMBERS length incorrect");
        assert!(stdout.contains("Hello World"), "GREETING concat not working");
    }

    #[test]
    fn test_mixed_numeric_codegen_runs() {
        let Ok(output) = Command::new("target/debug/incan")
            .args([
                "run",
                "-c",
                r#"
def main() -> None:
    size: int = 2
    x: float = 3.0
    result = 2.0 * x / size
    println(result)
"#,
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()
        else {
            panic!("failed to run incan");
        };

        assert!(
            output.status.success(),
            "mixed numeric run failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains('3'),
            "mixed numeric output missing expected result; stdout={}",
            stdout
        );
    }
}

/// End-to-end integration tests for `incan test`.
///
/// These tests exercise the full pipeline: write an Incan test file → run `incan test` via the CLI → verify
/// stdout/stderr/exit code.  They catch integration bugs like missing `fn main()` or broken parametrize expansion that
/// unit tests cannot detect.
mod test_runner_e2e {
    use std::process::Command;

    /// Create a temp directory with a single test file and return the directory path.
    fn write_test_project(filename: &str, source: &str) -> std::path::PathBuf {
        use std::time::{SystemTime, UNIX_EPOCH};

        let mut dir = std::env::temp_dir();
        let Ok(duration) = SystemTime::now().duration_since(UNIX_EPOCH) else {
            panic!("system time before UNIX epoch");
        };
        let uniq = duration.as_nanos();
        dir.push(format!("incan_e2e_test_{}", uniq));
        let Ok(()) = std::fs::create_dir_all(&dir) else {
            panic!("failed to create temp dir");
        };
        let Ok(()) = std::fs::write(dir.join(filename), source) else {
            panic!("failed to write test file");
        };
        dir
    }

    /// Run `incan test` on a directory and return the combined output.
    fn run_incan_test(dir: &std::path::Path) -> std::process::Output {
        Command::new("target/debug/incan")
            .args(["test", dir.to_string_lossy().as_ref()])
            .env("CARGO_NET_OFFLINE", "true")
            .output()
            .unwrap_or_else(|e| panic!("failed to run `incan test`: {}", e))
    }

    /// Run `incan test` with extra flags.
    fn run_incan_test_with_args(dir: &std::path::Path, extra: &[&str]) -> std::process::Output {
        let mut cmd = Command::new("target/debug/incan");
        cmd.arg("test");
        for arg in extra {
            cmd.arg(arg);
        }
        cmd.arg(dir.to_string_lossy().as_ref());
        cmd.env("CARGO_NET_OFFLINE", "true");
        cmd.output()
            .unwrap_or_else(|e| panic!("failed to run `incan test`: {}", e))
    }

    // ---- Passing test ----

    #[test]
    fn e2e_passing_test_succeeds() {
        let dir = write_test_project(
            "test_math.incn",
            r#"
from std.testing import assert_eq

def test_addition() -> None:
    assert_eq(1 + 1, 2)
"#,
        );

        let output = run_incan_test(&dir);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        assert!(
            output.status.success(),
            "expected passing test to succeed.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        assert!(
            stdout.contains("PASSED") || stdout.contains("passed"),
            "expected PASSED in output.\nstdout:\n{}",
            stdout,
        );
    }

    #[test]
    fn e2e_assert_statement_with_module_import_succeeds() {
        let dir = write_test_project(
            "test_assert_stmt.incn",
            r#"
import std.testing

def test_assert_statement_sugar() -> None:
    assert 1 + 1 == 2
    assert 3 != 4
    assert not False
    assert True
"#,
        );

        let output = run_incan_test(&dir);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        assert!(
            output.status.success(),
            "expected assert-statement test to succeed.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        assert!(
            stdout.contains("PASSED") || stdout.contains("passed"),
            "expected PASSED in output.\nstdout:\n{}",
            stdout,
        );
    }

    // ---- Failing test ----

    #[test]
    fn e2e_failing_test_reports_failure() {
        let dir = write_test_project(
            "test_bad.incn",
            r#"
from std.testing import assert_eq

def test_wrong() -> None:
    assert_eq(1 + 1, 99)
"#,
        );

        let output = run_incan_test(&dir);
        let stdout = String::from_utf8_lossy(&output.stdout);

        assert!(
            !output.status.success(),
            "expected failing test to exit non-zero.\nstdout:\n{}",
            stdout,
        );
        assert!(
            stdout.contains("FAILED") || stdout.contains("failed"),
            "expected FAILED in output.\nstdout:\n{}",
            stdout,
        );
    }

    // ---- Skip marker ----

    #[test]
    fn e2e_skip_marker_skips_test() {
        let dir = write_test_project(
            "test_skip.incn",
            r#"
from std.testing import skip

@skip("not implemented yet")
def test_todo() -> None:
    pass
"#,
        );

        let output = run_incan_test(&dir);
        let stdout = String::from_utf8_lossy(&output.stdout);

        assert!(
            output.status.success(),
            "expected skipped test to succeed overall.\nstdout:\n{}",
            stdout,
        );
        assert!(
            stdout.contains("SKIPPED") || stdout.contains("skipped"),
            "expected SKIPPED in output.\nstdout:\n{}",
            stdout,
        );
    }

    // ---- Parametrize expansion ----

    #[test]
    fn e2e_parametrize_expands_and_runs_all_cases() {
        let dir = write_test_project(
            "test_param.incn",
            r#"
from std.testing import parametrize, assert_eq

@parametrize("a, b, expected", [(1, 2, 3), (10, 20, 30), (0, 0, 0)])
def test_add(a: int, b: int, expected: int) -> None:
    assert_eq(a + b, expected)
"#,
        );

        let output = run_incan_test_with_args(&dir, &["--verbose"]);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        assert!(
            output.status.success(),
            "expected parametrized test to succeed.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );

        // All three parametrized variants should appear in the output.
        assert!(
            stdout.contains("test_add[1-2-3]"),
            "expected test_add[1-2-3] in output.\nstdout:\n{}",
            stdout,
        );
        assert!(
            stdout.contains("test_add[10-20-30]"),
            "expected test_add[10-20-30] in output.\nstdout:\n{}",
            stdout,
        );
        assert!(
            stdout.contains("test_add[0-0-0]"),
            "expected test_add[0-0-0] in output.\nstdout:\n{}",
            stdout,
        );

        // Should report 3 passed
        assert!(
            stdout.contains("3 passed"),
            "expected '3 passed' in output.\nstdout:\n{}",
            stdout,
        );
    }

    // ---- Parametrize with a failing case ----

    #[test]
    fn e2e_parametrize_reports_failing_case() {
        let dir = write_test_project(
            "test_param_fail.incn",
            r#"
from std.testing import parametrize, assert_eq

@parametrize("x, expected", [(2, 4), (3, 7)])
def test_double(x: int, expected: int) -> None:
    assert_eq(x * 2, expected)
"#,
        );

        let output = run_incan_test_with_args(&dir, &["--verbose"]);
        let stdout = String::from_utf8_lossy(&output.stdout);

        // 2*2==4 passes, 3*2==6!=7 fails
        assert!(
            !output.status.success(),
            "expected one failing case to make the run fail.\nstdout:\n{}",
            stdout,
        );
        assert!(
            stdout.contains("1 passed") && stdout.contains("1 failed"),
            "expected '1 passed' and '1 failed'.\nstdout:\n{}",
            stdout,
        );
    }
}

/// Test specific parser behavior
mod parser_tests {
    use incan::frontend::ast::*;
    use incan::frontend::{lexer, parser};

    fn parse_str(source: &str) -> Result<Program, ()> {
        let tokens = lexer::lex(source).map_err(|_| ())?;
        parser::parse(&tokens).map_err(|_| ())
    }

    #[test]
    fn test_model_with_decorator() {
        let source = r#"
@derive(Debug, Eq)
model User:
  name: str
"#;
        let Ok(program) = parse_str(source) else {
            panic!("parse failed");
        };
        match &program.declarations[0].node {
            Declaration::Model(m) => {
                assert_eq!(m.decorators.len(), 1);
                assert_eq!(m.decorators[0].node.name, "derive");
            }
            _ => panic!("Expected model"),
        }
    }

    #[test]
    fn test_class_with_traits() {
        let source = r#"
class Service with Loggable, Serializable:
  name: str
"#;
        let Ok(program) = parse_str(source) else {
            panic!("parse failed");
        };
        match &program.declarations[0].node {
            Declaration::Class(c) => {
                assert_eq!(c.traits.len(), 2);
                assert_eq!(c.traits[0].node, "Loggable");
                assert_eq!(c.traits[1].node, "Serializable");
            }
            _ => panic!("Expected class"),
        }
    }

    #[test]
    fn test_method_with_mut_self() {
        let source = r#"
class Counter:
  value: int = 0
  
  def inc(mut self) -> Unit:
    pass
"#;
        let Ok(program) = parse_str(source) else {
            panic!("parse failed");
        };
        match &program.declarations[0].node {
            Declaration::Class(c) => {
                assert_eq!(c.methods[0].node.receiver, Some(Receiver::Mutable));
            }
            _ => panic!("Expected class"),
        }
    }

    #[test]
    fn test_match_with_case() {
        let source = r#"
def foo(x: Option[int]) -> int:
  match x:
    case Some(n):
      return n
    case None:
      return 0
"#;
        let Ok(program) = parse_str(source) else {
            panic!("parse failed");
        };
        match &program.declarations[0].node {
            Declaration::Function(f) => {
                assert_eq!(f.body.len(), 1);
            }
            _ => panic!("Expected function"),
        }
    }

    #[test]
    fn test_list_comprehension() {
        let source = r#"
def squares(nums: List[int]) -> List[int]:
  return [x * x for x in nums if x > 0]
"#;
        let Ok(program) = parse_str(source) else {
            panic!("parse failed");
        };
        assert_eq!(program.declarations.len(), 1);
    }

    #[test]
    fn test_generic_type() {
        let source = r#"
def foo() -> Result[int, str]:
  return Ok(42)
"#;
        let Ok(program) = parse_str(source) else {
            panic!("parse failed");
        };
        match &program.declarations[0].node {
            Declaration::Function(f) => match &f.return_type.node {
                Type::Generic(name, args) => {
                    assert_eq!(name, "Result");
                    assert_eq!(args.len(), 2);
                }
                _ => panic!("Expected generic type"),
            },
            _ => panic!("Expected function"),
        }
    }

    #[test]
    fn test_yield_expression() {
        let source = r#"
def fixture() -> str:
  value = "test"
  yield value
"#;
        let Ok(program) = parse_str(source) else {
            panic!("parse failed");
        };
        match &program.declarations[0].node {
            Declaration::Function(f) => {
                assert_eq!(f.body.len(), 2);
                // Second statement should be the yield
                match &f.body[1].node {
                    Statement::Expr(expr) => {
                        match &expr.node {
                            Expr::Yield(Some(_)) => {} // Success
                            _ => panic!("Expected yield expression with value"),
                        }
                    }
                    _ => panic!("Expected expression statement"),
                }
            }
            _ => panic!("Expected function"),
        }
    }

    #[test]
    fn test_fixture_decorator() {
        let source = r#"
from std.testing import fixture

@fixture(scope="module")
def database() -> Database:
  db = connect()
  yield db
"#;
        let Ok(program) = parse_str(source) else {
            panic!("parse failed");
        };
        // declarations[0] is the import, declarations[1] is the function
        match &program.declarations[1].node {
            Declaration::Function(f) => {
                assert_eq!(f.decorators.len(), 1);
                assert_eq!(f.decorators[0].node.name, "fixture");
                assert!(!f.decorators[0].node.args.is_empty());
            }
            _ => panic!("Expected function"),
        }
    }

    #[test]
    fn test_rust_crate_import() {
        let source = r#"import rust::serde_json as json"#;
        let Ok(program) = parse_str(source) else {
            panic!("parse failed");
        };
        match &program.declarations[0].node {
            Declaration::Import(i) => {
                match &i.kind {
                    ImportKind::RustCrate {
                        crate_name,
                        path,
                        version,
                        features,
                    } => {
                        assert_eq!(crate_name, "serde_json");
                        assert!(path.is_empty());
                        assert!(version.is_none());
                        assert!(features.is_empty());
                    }
                    _ => panic!("Expected RustCrate import kind"),
                }
                assert_eq!(i.alias.as_deref(), Some("json"));
            }
            _ => panic!("Expected import"),
        }
    }

    #[test]
    fn test_rust_from_import() {
        let source = r#"from rust::time import Instant, Duration"#;
        let Ok(program) = parse_str(source) else {
            panic!("parse failed");
        };
        match &program.declarations[0].node {
            Declaration::Import(i) => match &i.kind {
                ImportKind::RustFrom {
                    crate_name,
                    path,
                    version,
                    features,
                    items,
                } => {
                    assert_eq!(crate_name, "time");
                    assert!(path.is_empty());
                    assert!(version.is_none());
                    assert!(features.is_empty());
                    assert_eq!(items.len(), 2);
                    assert_eq!(items[0].name, "Instant");
                    assert_eq!(items[1].name, "Duration");
                }
                _ => panic!("Expected RustFrom import kind"),
            },
            _ => panic!("Expected import"),
        }
    }
}

mod rfc031_pub_import_integration_tests {
    use super::*;
    use incan::library_manifest::{LibraryManifest, ModelExport};

    fn incan_bin_path() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("target").join("debug").join("incan")
    }

    fn write_project_files(
        root: &Path,
        manifest_content: &str,
        main_source: &str,
    ) -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
        std::fs::create_dir_all(root.join("src"))?;
        std::fs::write(root.join("incan.toml"), manifest_content)?;
        let main_path = root.join("src").join("main.incn");
        std::fs::write(&main_path, main_source)?;
        Ok(main_path)
    }

    fn run_check(main_path: &Path) -> Result<std::process::Output, Box<dyn std::error::Error>> {
        Ok(Command::new(incan_bin_path())
            .arg("--check")
            .arg(main_path)
            .output()?)
    }

    fn run_build(main_path: &Path, out_dir: &Path) -> Result<std::process::Output, Box<dyn std::error::Error>> {
        Ok(Command::new(incan_bin_path())
            .args([
                "build",
                main_path.to_string_lossy().as_ref(),
                out_dir.to_string_lossy().as_ref(),
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?)
    }

    fn run_build_lib(project_root: &Path) -> Result<std::process::Output, Box<dyn std::error::Error>> {
        Ok(Command::new(incan_bin_path())
            .args(["build", "--lib"])
            .current_dir(project_root)
            .env("CARGO_NET_OFFLINE", "true")
            .output()?)
    }

    fn write_minimal_library_crate(artifact_root: &Path, package_name: &str) -> Result<(), Box<dyn std::error::Error>> {
        std::fs::create_dir_all(artifact_root.join("src"))?;
        std::fs::write(
            artifact_root.join("Cargo.toml"),
            format!(
                "[package]\nname = \"{package_name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[lib]\npath = \"src/lib.rs\"\n"
            ),
        )?;
        std::fs::write(artifact_root.join("src/lib.rs"), "pub fn linked() {}\n")?;
        Ok(())
    }

    fn mylib_manifest_with_widget() -> LibraryManifest {
        let mut manifest = LibraryManifest::new("mylib", "0.1.0");
        manifest.exports.models.push(ModelExport {
            name: "Widget".to_string(),
            type_params: Vec::new(),
            traits: Vec::new(),
            fields: Vec::new(),
            methods: Vec::new(),
        });
        manifest
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
            fields: Vec::new(),
            methods: Vec::new(),
        });
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
    fn build_lib_artifacts_and_consumer_alias_linkage() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let producer_root = tmp.path().join("widgets_core_project");
        std::fs::create_dir_all(producer_root.join("src"))?;
        std::fs::write(
            producer_root.join("incan.toml"),
            "[project]\nname = \"widgets_core\"\nversion = \"0.1.0\"\n",
        )?;
        std::fs::write(
            producer_root.join("src/widgets.incn"),
            "pub model Widget:\n  name: str\n\npub def make_widget(name: str) -> Widget:\n  return Widget(name=name)\n",
        )?;
        std::fs::write(
            producer_root.join("src/lib.incn"),
            "pub from widgets import Widget, make_widget\n",
        )?;

        let producer_build = run_build_lib(&producer_root)?;
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
            consumer_root.join("incan.toml"),
            "[project]\nname = \"consumer\"\n\n[dependencies]\nwidgets = { path = \"../widgets_core_project\" }\n",
        )?;
        let consumer_main = consumer_root.join("src/main.incn");
        std::fs::write(
            &consumer_main,
            "from pub::widgets import Widget as PublicWidget, make_widget\n\ndef main() -> None:\n  w: PublicWidget = make_widget(\"ok\")\n  print(w.name)\n",
        )?;

        let out_dir = consumer_root.join("out");
        let consumer_build = run_build(&consumer_main, &out_dir)?;
        assert!(
            consumer_build.status.success(),
            "expected consumer build to succeed.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&consumer_build.stdout),
            String::from_utf8_lossy(&consumer_build.stderr)
        );

        let generated_toml = std::fs::read_to_string(out_dir.join("Cargo.toml"))?;
        assert!(
            generated_toml.contains("[dependencies.widgets]"),
            "expected library alias dependency entry, got:\n{generated_toml}"
        );
        assert!(
            generated_toml.contains("package = \"widgets_core\""),
            "expected package alias mapping in Cargo.toml, got:\n{generated_toml}"
        );
        assert!(
            generated_toml.contains("path = "),
            "expected path dependency in Cargo.toml, got:\n{generated_toml}"
        );

        let generated_main_rs = std::fs::read_to_string(out_dir.join("src/main.rs"))?;
        assert!(
            generated_main_rs.contains("pub use widgets::Widget as PublicWidget;"),
            "expected pub:: item alias import emission, got:\n{generated_main_rs}"
        );
        assert!(
            generated_main_rs.contains("pub use widgets::make_widget;"),
            "expected pub:: item import emission, got:\n{generated_main_rs}"
        );

        Ok(())
    }
}
