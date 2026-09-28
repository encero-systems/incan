//! Integration tests for the Incan compiler frontend

include!("support/integration_tests_root.rs");

/// Regression (GitHub #247): `incan fmt` on disk must preserve body docstrings for all public block-like type
/// declarations, and [`exported_type_like_docs`] must still see them after the CLI round-trip.
///
/// `format_files` delegates to [`incan_format::format_source`]; this still covers subprocess + I/O if those paths
/// diverge from in-process formatting.
#[test]
fn test_cli_fmt_preserves_block_decl_docstrings_and_export_doc_surface() -> Result<(), Box<dyn std::error::Error>> {
    let dir = make_temp_test_dir();
    let path = dir.join("block_docstrings_cli.incn");
    fs::write(&path, block_docstring_public_type_like()?)?;
    let status = incan_command().arg("fmt").arg(&path).status()?;
    assert!(status.success(), "incan fmt failed");

    let formatted = fs::read_to_string(&path)?;
    let tokens = lexer::lex(&formatted)
        .map_err(|errs| std::io::Error::other(errs.iter().map(|e| e.message.clone()).collect::<Vec<_>>().join("\n")))?;
    let ast = parser::parse(&tokens)
        .map_err(|errs| std::io::Error::other(errs.iter().map(|e| e.message.clone()).collect::<Vec<_>>().join("\n")))?;

    fn assert_markers(doc: Option<&str>, ctx: &str) -> Result<(), Box<dyn std::error::Error>> {
        let Some(doc) = doc else {
            return Err(std::io::Error::other(format!("{ctx}: missing docstring after CLI fmt")).into());
        };
        let t = doc.trim();
        if !t.contains("Line A documents the class API.") {
            return Err(std::io::Error::other(format!("{ctx}: missing marker A in {t:?}")).into());
        }
        if !t.contains("Line B keeps interior newlines after trim().") {
            return Err(std::io::Error::other(format!("{ctx}: missing marker B in {t:?}")).into());
        }
        Ok(())
    }

    let docs = exported_type_like_docs(&ast);
    assert_eq!(docs.len(), 5, "expected five public type-like exports with docs");
    let mut by_name: std::collections::HashMap<String, ExportedTypeLikeDoc> = std::collections::HashMap::new();
    for d in docs {
        by_name.insert(d.name.clone(), d);
    }

    let m = by_name
        .get("CliModelProbe")
        .ok_or_else(|| std::io::Error::other("missing CliModelProbe"))?;
    assert_eq!(m.kind, ExportedTypeLikeKind::Model);
    assert_markers(m.docstring.as_deref(), "model")?;

    let c = by_name
        .get("CliClassProbe")
        .ok_or_else(|| std::io::Error::other("missing CliClassProbe"))?;
    assert_eq!(c.kind, ExportedTypeLikeKind::Class);
    assert_markers(c.docstring.as_deref(), "class")?;

    let e = by_name
        .get("CliEnumProbe")
        .ok_or_else(|| std::io::Error::other("missing CliEnumProbe"))?;
    assert_eq!(e.kind, ExportedTypeLikeKind::Enum);
    assert_markers(e.docstring.as_deref(), "enum")?;

    let t = by_name
        .get("CliTraitProbe")
        .ok_or_else(|| std::io::Error::other("missing CliTraitProbe"))?;
    assert_eq!(t.kind, ExportedTypeLikeKind::Trait);
    assert_markers(t.docstring.as_deref(), "trait")?;

    let n = by_name
        .get("CliNewtypeProbe")
        .ok_or_else(|| std::io::Error::other("missing CliNewtypeProbe"))?;
    assert_eq!(n.kind, ExportedTypeLikeKind::Newtype);
    assert_markers(n.docstring.as_deref(), "newtype")?;

    Ok(())
}

#[test]
fn test_cli_fmt_accepts_assert_identity_bool_literals() -> Result<(), Box<dyn std::error::Error>> {
    let dir = make_temp_test_dir();
    let path = dir.join("assert_identity_bool_literals.incn");
    fs::write(
        &path,
        r#"
def check_flags(ready: bool, done: bool) -> None:
    assert ready is true, "ready should be true"
    assert done is false
"#,
    )?;

    let output = incan_command().arg("fmt").arg(&path).output()?;
    assert!(
        output.status.success(),
        "expected `incan fmt` to accept assert identity checks against bool literals.\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

/// Regression (GitHub #484): parenthesized logical chains should wrap at obvious boolean breakpoints.
#[test]
fn test_cli_fmt_wraps_long_parenthesized_logical_expression_chain() -> Result<(), Box<dyn std::error::Error>> {
    let dir = make_temp_test_dir();
    let path = dir.join("long_logical_chain.incn");
    fs::write(
        &path,
        r#"model Item:
    kind_name: str
    predicate_kind_name: str
    source_name: str


def matches(item: Item) -> bool:
    return (item.kind_name == "filter" and item.predicate_kind_name == "bool_literal" and item.source_name == "rewritten_prism_node")
"#,
    )?;

    let status = incan_command().arg("fmt").arg(&path).status()?;
    assert!(status.success(), "incan fmt failed");

    let formatted = fs::read_to_string(&path)?;
    let expected = r#"model Item:
    kind_name: str
    predicate_kind_name: str
    source_name: str


def matches(item: Item) -> bool:
    return (
        item.kind_name == "filter"
        and item.predicate_kind_name == "bool_literal"
        and item.source_name == "rewritten_prism_node"
    )
"#;
    assert_eq!(formatted, expected);
    assert!(
        formatted.lines().all(|line| line.len() <= 120),
        "expected formatted output to stay within 120 columns:\n{formatted}"
    );

    let output = incan_command().arg("--check").arg(&path).output()?;
    assert!(
        output.status.success(),
        "expected wrapped expression to parse/typecheck after CLI fmt; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    Ok(())
}

/// Regression (GitHub #289): `incan fmt` must preserve escaped newlines in f-strings as textual `\\n`.
#[test]
fn test_cli_fmt_preserves_fstring_escaped_newline_roundtrip() -> Result<(), Box<dyn std::error::Error>> {
    let dir = make_temp_test_dir();
    let path = dir.join("fstring_escaped_newline.incn");
    fs::write(
        &path,
        r#"def main() -> str:
    return f"a\n{1}"
"#,
    )?;

    let status = incan_command().arg("fmt").arg(&path).status()?;
    assert!(status.success(), "incan fmt failed");

    let formatted = fs::read_to_string(&path)?;
    assert!(
        formatted.contains(r#"f"a\n{1}""#),
        "expected formatted output to preserve escaped newline text, got:\n{}",
        formatted
    );

    let output = incan_command().arg("--check").arg(&path).output()?;
    assert!(
        output.status.success(),
        "expected formatted file to parse/typecheck after CLI fmt; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    Ok(())
}

/// Regression (GitHub #336 / RFC 053): the CLI formatter must apply the vertical-spacing contract on disk.
#[test]
fn test_cli_fmt_applies_rfc053_vertical_spacing_contract() -> Result<(), Box<dyn std::error::Error>> {
    let dir = make_temp_test_dir();
    let path = dir.join("rfc053_vertical_spacing.incn");
    fs::write(
        &path,
        r#"type UserId = str
# comment about the alias

model User:
  """
  First paragraph.


  Second paragraph.
  """
  id: UserId

trait Service:
  def connect(self) -> None: ...
  def reset(self) -> None:
    pass
"#,
    )?;

    let status = incan_command().arg("fmt").arg(&path).status()?;
    assert!(status.success(), "incan fmt failed");

    let formatted = fs::read_to_string(&path)?;
    let expected = r#"type UserId = str
# comment about the alias


model User:
    """
    First paragraph.

    Second paragraph.
    """

    id: UserId


trait Service:
    def connect(self) -> None

    def reset(self) -> None:
        pass
"#;
    assert_eq!(formatted, expected);

    let tokens = lexer::lex(&formatted)
        .map_err(|errs| std::io::Error::other(errs.iter().map(|e| e.message.clone()).collect::<Vec<_>>().join("\n")))?;
    parser::parse(&tokens)
        .map_err(|errs| std::io::Error::other(errs.iter().map(|e| e.message.clone()).collect::<Vec<_>>().join("\n")))?;

    Ok(())
}

/// Regression (GitHub #336 / RFC 053): top-level type/function-shaped declarations keep two blank lines even when
/// adjacent to module statics.
#[test]
fn test_cli_fmt_keeps_two_blank_lines_between_static_and_function() -> Result<(), Box<dyn std::error::Error>> {
    let dir = make_temp_test_dir();
    let path = dir.join("rfc053_static_function_spacing.incn");
    fs::write(
        &path,
        r#"static prism_store_node_counts: list[int] = []
pub def allocate_prism_store_id() -> int:
  return len(prism_store_node_counts)
"#,
    )?;

    let status = incan_command().arg("fmt").arg(&path).status()?;
    assert!(status.success(), "incan fmt failed");

    let formatted = fs::read_to_string(&path)?;
    let expected = r#"static prism_store_node_counts: list[int] = []


pub def allocate_prism_store_id() -> int:
    return len(prism_store_node_counts)
"#;
    assert_eq!(formatted, expected);

    let tokens = lexer::lex(&formatted)
        .map_err(|errs| std::io::Error::other(errs.iter().map(|e| e.message.clone()).collect::<Vec<_>>().join("\n")))?;
    parser::parse(&tokens)
        .map_err(|errs| std::io::Error::other(errs.iter().map(|e| e.message.clone()).collect::<Vec<_>>().join("\n")))?;

    Ok(())
}

/// Regression (GitHub #336 / RFC 053): a trailing own-line comment after a multi-line construct must stay after the
/// full suite, not get reinserted after the construct header.
#[test]
fn test_cli_fmt_keeps_trailing_comment_after_multiline_function() -> Result<(), Box<dyn std::error::Error>> {
    let dir = make_temp_test_dir();
    let path = dir.join("rfc053_trailing_comment_after_function.incn");
    fs::write(
        &path,
        r#"def load_user(id: str) -> str:
    return id

# TODO: split retries
"#,
    )?;

    let status = incan_command().arg("fmt").arg(&path).status()?;
    assert!(status.success(), "incan fmt failed");

    let formatted = fs::read_to_string(&path)?;
    let expected = r#"def load_user(id: str) -> str:
    return id
# TODO: split retries
"#;
    assert_eq!(formatted, expected);

    let tokens = lexer::lex(&formatted)
        .map_err(|errs| std::io::Error::other(errs.iter().map(|e| e.message.clone()).collect::<Vec<_>>().join("\n")))?;
    parser::parse(&tokens)
        .map_err(|errs| std::io::Error::other(errs.iter().map(|e| e.message.clone()).collect::<Vec<_>>().join("\n")))?;

    Ok(())
}

/// Regression (GitHub #394): multiline function parameter lists must accept a trailing comma.
#[test]
fn test_cli_check_accepts_trailing_comma_in_multiline_function_params() -> Result<(), Box<dyn std::error::Error>> {
    let dir = make_temp_test_dir();
    let path = dir.join("trailing_param_comma.incn");
    fs::write(
        &path,
        r#"def identity(
    value: int,
) -> int:
    return value


def main() -> None:
    println(identity(1))
"#,
    )?;

    let output = incan_command().arg("--check").arg(&path).output()?;
    assert!(
        output.status.success(),
        "expected multiline trailing parameter comma to parse/typecheck; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    Ok(())
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
    let fixtures_dir = incan_test_support::fixture("valid");
    if !fixtures_dir.exists() {
        return; // Skip if fixtures not present
    }

    let mut matched = 0usize;
    let Ok(entries) = fs::read_dir(&fixtures_dir) else {
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
    let fixtures_dir = incan_test_support::fixture("invalid");
    if !fixtures_dir.exists() {
        return; // Skip if fixtures not present
    }

    let mut matched = 0usize;
    let Ok(entries) = fs::read_dir(&fixtures_dir) else {
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
fn test_help_is_banner_free() -> Result<(), Box<dyn std::error::Error>> {
    let output = incan_command().arg("--help").output()?;
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
    Ok(())
}

#[test]
fn test_version_is_single_line_and_banner_free() -> Result<(), Box<dyn std::error::Error>> {
    let output = incan_command().arg("--version").output()?;
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
    Ok(())
}

#[test]
fn lifecycle_new_version_and_env_commands_work() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let project_dir = tmp.path().join("greeter");

    let new_output = incan_command()
        .args(["new", "greeter", "--yes", "--dir"])
        .arg(&project_dir)
        .args([
            "--description",
            "A generated greeting app",
            "--author",
            "Danny <danny@example.com>",
            "--license",
            "MIT",
        ])
        .output()?;
    assert!(
        new_output.status.success(),
        "incan new failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&new_output.stdout),
        String::from_utf8_lossy(&new_output.stderr)
    );

    let manifest_path = project_dir.join("loaf.toml");
    let initial_manifest = fs::read_to_string(&manifest_path)?;
    assert!(initial_manifest.contains(r#"name = "greeter""#));
    assert!(initial_manifest.contains(r#"description = "A generated greeting app""#));
    assert!(initial_manifest.contains(r#"authors = ["Danny <danny@example.com>"]"#));
    assert!(initial_manifest.contains(r#"license = "MIT""#));
    // Derived rather than written out: a prerelease compiler emits a `-0` lower bound so rc builds accept rc
    // projects, and a final release omits it so a released project does not silently accept prereleases. Pinning
    // one spelling makes this test fail on every transition between the two, which it did on the 0.5.0 bump.
    let expected_requires_incan = {
        let version = semver::Version::parse(incan_lang::version::INCAN_VERSION)?;
        let lower = if version.pre.is_empty() {
            format!(">={}.{}.0", version.major, version.minor)
        } else {
            format!(">={}.{}.0-0", version.major, version.minor)
        };
        format!("{lower},<{}.{}.0", version.major, version.minor + 1)
    };
    assert!(
        initial_manifest.contains(&format!(r#"requires-incan = "{expected_requires_incan}""#)),
        "generated manifest should require {expected_requires_incan}, got:\n{initial_manifest}"
    );
    assert!(project_dir.join("src/main.incn").exists());
    assert!(project_dir.join("tests/test_main.incn").exists());

    let default_show_output = incan_command()
        .args(["env", "show", "default"])
        .current_dir(&project_dir)
        .output()?;
    assert!(
        default_show_output.status.success(),
        "env show default on fresh project failed: {}",
        String::from_utf8_lossy(&default_show_output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&default_show_output.stdout).contains("overlay chain: project -> default"),
        "unexpected env show default output:\n{}",
        String::from_utf8_lossy(&default_show_output.stdout)
    );

    let dry_run = incan_command()
        .args(["version", "patch", "--dry-run"])
        .current_dir(&project_dir)
        .output()?;
    assert!(
        dry_run.status.success(),
        "dry-run failed: {}",
        String::from_utf8_lossy(&dry_run.stderr)
    );
    assert!(
        String::from_utf8_lossy(&dry_run.stdout).contains("new version: 0.1.1"),
        "unexpected dry-run output:\n{}",
        String::from_utf8_lossy(&dry_run.stdout)
    );
    assert_eq!(
        fs::read_to_string(&manifest_path)?,
        initial_manifest,
        "dry-run must not modify loaf.toml"
    );

    let version_output = incan_command()
        .args(["version", "patch"])
        .current_dir(&project_dir)
        .output()?;
    assert!(
        version_output.status.success(),
        "version bump failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&version_output.stdout),
        String::from_utf8_lossy(&version_output.stderr)
    );
    assert!(fs::read_to_string(&manifest_path)?.contains(r#"version = "0.1.1""#));

    let set_output = incan_command()
        .args([
            "version",
            "--set",
            "2.0.0-rc.1",
            "--project",
            manifest_path.to_str().ok_or("manifest path is not valid UTF-8")?,
        ])
        .current_dir(tmp.path())
        .output()?;
    assert!(
        set_output.status.success(),
        "version set failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&set_output.stdout),
        String::from_utf8_lossy(&set_output.stderr)
    );
    assert!(fs::read_to_string(&manifest_path)?.contains(r#"version = "2.0.0-rc.1""#));

    let keep_prerelease_output = incan_command()
        .args([
            "version",
            "patch",
            "--keep-prerelease",
            "--project",
            project_dir.to_str().ok_or("project path is not valid UTF-8")?,
        ])
        .current_dir(tmp.path())
        .output()?;
    assert!(
        keep_prerelease_output.status.success(),
        "version keep-prerelease failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&keep_prerelease_output.stdout),
        String::from_utf8_lossy(&keep_prerelease_output.stderr)
    );
    assert!(fs::read_to_string(&manifest_path)?.contains(r#"version = "2.0.1-rc.1""#));

    let missing_request_output = incan_command()
        .args([
            "version",
            "--project",
            project_dir.to_str().ok_or("project path is not valid UTF-8")?,
        ])
        .current_dir(tmp.path())
        .output()?;
    assert!(!missing_request_output.status.success());
    assert!(
        String::from_utf8_lossy(&missing_request_output.stderr).contains("requires a bump name or `--set <version>`"),
        "unexpected missing-request stderr:\n{}",
        String::from_utf8_lossy(&missing_request_output.stderr)
    );

    let conflicting_request_output = incan_command()
        .args([
            "version",
            "patch",
            "--set",
            "3.0.0",
            "--project",
            project_dir.to_str().ok_or("project path is not valid UTF-8")?,
        ])
        .current_dir(tmp.path())
        .output()?;
    assert!(!conflicting_request_output.status.success());
    assert!(
        String::from_utf8_lossy(&conflicting_request_output.stderr)
            .contains("accepts either a bump name or `--set <version>`, not both"),
        "unexpected conflicting-request stderr:\n{}",
        String::from_utf8_lossy(&conflicting_request_output.stderr)
    );

    fs::write(
        &manifest_path,
        format!(
            "{}\n[rust-dependencies.serde]\nversion = \"1.0\"\nfeatures = [\"derive\"]\n\n[tool.incan.envs.default]\nenv-vars = {{ INCAN_NO_BANNER = \"1\" }}\n\n[tool.incan.envs.unit]\ncwd = \".\"\n\n[tool.incan.envs.unit.rust-dependencies.serde]\nversion = \"1.0\"\nfeatures = [\"alloc\"]\n\n[tool.incan.envs.unit.scripts]\nprobe = [\"{}\", \"--version\"]\n",
            fs::read_to_string(&manifest_path)?,
            incan_debug_binary().display()
        ),
    )?;

    let list_output = incan_command()
        .args(["env", "list"])
        .current_dir(project_dir.join("src"))
        .output()?;
    assert!(
        list_output.status.success(),
        "env list failed: {}",
        String::from_utf8_lossy(&list_output.stderr)
    );
    let list_stdout = String::from_utf8_lossy(&list_output.stdout);
    assert!(list_stdout.contains("default"));
    assert!(list_stdout.contains("unit"));

    let list_json_output = incan_command()
        .args([
            "env",
            "list",
            "--format",
            "json",
            "--project",
            project_dir.to_str().ok_or("project path is not valid UTF-8")?,
        ])
        .current_dir(tmp.path())
        .output()?;
    assert!(
        list_json_output.status.success(),
        "env list json failed: {}",
        String::from_utf8_lossy(&list_json_output.stderr)
    );
    let list_json: serde_json::Value = serde_json::from_slice(&list_json_output.stdout)?;
    assert_eq!(list_json, serde_json::json!(["default", "unit"]));

    let show_output = incan_command()
        .args(["env", "show", "unit"])
        .current_dir(&project_dir)
        .output()?;
    assert!(
        show_output.status.success(),
        "env show failed: {}",
        String::from_utf8_lossy(&show_output.stderr)
    );
    let show_stdout = String::from_utf8_lossy(&show_output.stdout);
    assert!(show_stdout.contains("overlay chain: project -> default -> unit"));
    assert!(show_stdout.contains("INCAN_NO_BANNER=1"));
    assert!(show_stdout.contains("Dependencies"));
    assert!(show_stdout.contains("serde"));
    assert!(show_stdout.contains("alloc"));
    assert!(show_stdout.contains("derive"));

    let show_overview_output = incan_command()
        .args(["env", "show"])
        .current_dir(&project_dir)
        .output()?;
    assert!(
        show_overview_output.status.success(),
        "env show overview failed: {}",
        String::from_utf8_lossy(&show_overview_output.stderr)
    );
    let show_overview_stdout = String::from_utf8_lossy(&show_overview_output.stdout);
    assert!(show_overview_stdout.contains("default"));
    assert!(show_overview_stdout.contains("unit"));
    assert!(show_overview_stdout.contains("Scripts"));

    let show_overview_json_output = incan_command()
        .args([
            "env",
            "show",
            "--format",
            "json",
            "--project",
            manifest_path.to_str().ok_or("manifest path is not valid UTF-8")?,
        ])
        .current_dir(tmp.path())
        .output()?;
    assert!(
        show_overview_json_output.status.success(),
        "env show overview json failed: {}",
        String::from_utf8_lossy(&show_overview_json_output.stderr)
    );
    let show_overview_json: serde_json::Value = serde_json::from_slice(&show_overview_json_output.stdout)?;
    let show_overview_array = show_overview_json.as_array().ok_or("expected array json output")?;
    assert_eq!(show_overview_array.len(), 2);
    assert!(show_overview_array.iter().any(|entry| entry["name"] == "default"));
    assert!(show_overview_array.iter().any(|entry| entry["name"] == "unit"));

    let show_json_output = incan_command()
        .args(["env", "show", "unit", "--format", "json"])
        .current_dir(&project_dir)
        .output()?;
    assert!(
        show_json_output.status.success(),
        "env show json failed: {}",
        String::from_utf8_lossy(&show_json_output.stderr)
    );
    let show_json: serde_json::Value = serde_json::from_slice(&show_json_output.stdout)?;
    assert_eq!(show_json["env"], "unit");
    assert_eq!(show_json["dependencies"]["serde"]["version"], "1.0");

    let dry_run_env = incan_command()
        .args(["env", "run", "unit", "probe", "--dry-run"])
        .current_dir(&project_dir)
        .output()?;
    assert!(
        dry_run_env.status.success(),
        "env dry-run failed: {}",
        String::from_utf8_lossy(&dry_run_env.stderr)
    );
    assert!(
        String::from_utf8_lossy(&dry_run_env.stdout).contains("--version"),
        "unexpected env dry-run output:\n{}",
        String::from_utf8_lossy(&dry_run_env.stdout)
    );

    let run_env = incan_command()
        .args(["env", "run", "unit", "probe"])
        .current_dir(&project_dir)
        .output()?;
    assert!(
        run_env.status.success(),
        "env run failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&run_env.stdout),
        String::from_utf8_lossy(&run_env.stderr)
    );
    assert!(String::from_utf8_lossy(&run_env.stdout).starts_with("incan "));
    Ok(())
}

#[test]
fn zero_clone_starter_project_runs_tests_and_release_builds() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let project_name = unique_test_project_name("starter");
    let project_dir = tmp.path().join(&project_name);

    let new_output = incan_command()
        .args(["new", &project_name, "--yes", "--dir"])
        .arg(&project_dir)
        .output()?;
    assert!(
        new_output.status.success(),
        "incan new failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&new_output.stdout),
        String::from_utf8_lossy(&new_output.stderr)
    );
    let new_stdout = String::from_utf8_lossy(&new_output.stdout);
    assert!(new_stdout.contains("Run it:     incan run"));
    assert!(new_stdout.contains("Test it:    incan test"));
    assert!(new_stdout.contains("Release it: incan build --release"));

    let main_source = fs::read_to_string(project_dir.join("src/main.incn"))?;
    assert!(
        main_source.contains("pub def greeting() -> str:"),
        "starter source should expose a small testable function, got:\n{main_source}"
    );
    let test_source = fs::read_to_string(project_dir.join("tests/test_main.incn"))?;
    assert!(
        test_source.contains("assert_eq(greeting()"),
        "starter test should assert generated behavior, got:\n{test_source}"
    );

    let run_output = incan_command()
        .arg("run")
        .current_dir(&project_dir)
        .env("CARGO_NET_OFFLINE", "true")
        .output()?;
    assert!(
        run_output.status.success(),
        "starter incan run failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&run_output.stdout),
        String::from_utf8_lossy(&run_output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&run_output.stdout).contains(&format!("Hello from {project_name}!")),
        "unexpected starter run output:\n{}",
        String::from_utf8_lossy(&run_output.stdout)
    );

    let test_output = incan_command()
        .arg("test")
        .current_dir(&project_dir)
        .env("CARGO_NET_OFFLINE", "true")
        .output()?;
    assert!(
        test_output.status.success(),
        "starter incan test failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&test_output.stdout),
        String::from_utf8_lossy(&test_output.stderr)
    );

    let build_output = incan_command()
        .args(["build", "--release"])
        .current_dir(&project_dir)
        .env("CARGO_NET_OFFLINE", "true")
        .output()?;
    assert!(
        build_output.status.success(),
        "starter incan build --release failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&build_output.stdout),
        String::from_utf8_lossy(&build_output.stderr)
    );

    Ok(())
}

#[test]
fn env_run_nested_incan_run_uses_dependency_overlay_override() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let project_root = tmp.path();
    fs::create_dir_all(project_root.join("src"))?;
    fs::write(
        project_root.join("loaf.toml"),
        format!(
            r#"[project]
name = "env_overlay_exec"
version = "0.1.0"

[rust-dependencies.serde_json]
version = "999.0.0"

[tool.incan.envs.unit.scripts]
run = ["{}", "run", "src/main.incn"]

[tool.incan.envs.unit.rust-dependencies.serde_json]
version = "1.0"
"#,
            incan_debug_binary().display()
        ),
    )?;
    fs::write(
        project_root.join("src/main.incn"),
        r#"import rust::serde_json as json

def main() -> None:
  pass
"#,
    )?;

    let bare_run = incan_command()
        .args(["run", "src/main.incn"])
        .current_dir(project_root)
        .env("CARGO_NET_OFFLINE", "true")
        .output()?;
    assert!(
        !bare_run.status.success(),
        "plain run unexpectedly succeeded\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&bare_run.stdout),
        String::from_utf8_lossy(&bare_run.stderr)
    );
    let bare_stderr = strip_ansi_escapes(&String::from_utf8_lossy(&bare_run.stderr));
    assert!(
        bare_stderr.contains("serde_json") && bare_stderr.contains("999.0.0"),
        "expected invalid pinned dependency diagnostic, got:\n{}",
        bare_stderr
    );

    let env_run = incan_command()
        .args(["env", "run", "unit", "run"])
        .current_dir(project_root)
        .env("CARGO_NET_OFFLINE", "true")
        .output()?;
    assert!(
        env_run.status.success(),
        "env-backed nested run failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&env_run.stdout),
        String::from_utf8_lossy(&env_run.stderr)
    );
    let env_stderr = strip_ansi_escapes(&String::from_utf8_lossy(&env_run.stderr));
    assert!(
        !env_stderr.contains("999.0.0"),
        "nested env-backed run should use the overlay manifest instead of the broken base pin, got:\n{}",
        env_stderr
    );
    Ok(())
}

#[test]
fn env_run_nested_incan_env_show_prefers_parent_project_override() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let project_root = tmp.path();
    fs::create_dir_all(project_root.join("child"))?;
    fs::write(
        project_root.join("loaf.toml"),
        format!(
            r#"[project]
name = "parent_project"
version = "0.1.0"

[tool.incan.envs.unit]
cwd = "child"
env-vars = {{ PARENT = "1" }}

[tool.incan.envs.unit.scripts]
inspect = ["{}", "env", "show", "unit", "--format", "json"]
"#,
            incan_debug_binary().display()
        ),
    )?;
    fs::write(
        project_root.join("child/loaf.toml"),
        r#"[project]
name = "child_project"
version = "0.1.0"

[tool.incan.envs.unit]
env-vars = { CHILD = "1" }
"#,
    )?;

    let bare_show = incan_command()
        .args(["env", "show", "unit", "--format", "json"])
        .current_dir(project_root.join("child"))
        .output()?;
    assert!(
        bare_show.status.success(),
        "bare child env show failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&bare_show.stdout),
        String::from_utf8_lossy(&bare_show.stderr)
    );
    let bare_json: serde_json::Value = serde_json::from_slice(&bare_show.stdout)?;
    assert_eq!(bare_json["env_vars"]["CHILD"], "1");
    assert!(bare_json["env_vars"].get("PARENT").is_none());

    let env_show = incan_command()
        .args(["env", "run", "unit", "inspect"])
        .current_dir(project_root)
        .output()?;
    assert!(
        env_show.status.success(),
        "env-backed nested env show failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&env_show.stdout),
        String::from_utf8_lossy(&env_show.stderr)
    );
    let nested_json: serde_json::Value = serde_json::from_slice(&env_show.stdout)?;
    assert_eq!(nested_json["env_vars"]["PARENT"], "1");
    assert!(nested_json["env_vars"].get("CHILD").is_none());
    Ok(())
}
