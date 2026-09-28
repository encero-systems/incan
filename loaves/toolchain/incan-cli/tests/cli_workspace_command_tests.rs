//! Workspace scope and fan-out, canonical lock publication, and locked/frozen build refusals.
//!
//! One of nine roots split out of the former `tests/cli_integration.rs`. The shared command surface lives in
//! `incan_test_support::cli_project`.

include!("support/cli_workspace_and_lock_tests_root.rs");

#[cfg(unix)]
#[test]
fn workspace_lock_concurrent_publishers_leave_one_parseable_root_lock() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::write(root.path().join(".gitignore"), "target/\n.incan-home/\n")?;
    fs::write(
        root.path().join("loaf.toml"),
        "[workspace]\nmembers = [\"packages/*\"]\n",
    )?;
    for name in ["alpha", "zebra"] {
        let member_root = root.path().join("packages").join(name);
        fs::create_dir_all(member_root.join("src"))?;
        fs::write(
            member_root.join("loaf.toml"),
            format!(
                "[project]\nname = \"{name}\"\nversion = \"0.1.0\"\n\n[project.scripts]\nmain = \"src/main.incn\"\n"
            ),
        )?;
        fs::write(member_root.join("src/main.incn"), "def main() -> None:\n  pass\n")?;
    }

    let stdlib = support::repo_root().join("loaves/stdlib");
    let generated_target = support::generated_cargo_target_dir();
    let spawn_lock = |member: &str| -> Result<std::process::Child, Box<dyn std::error::Error>> {
        Ok(support::repo_command()
            .arg("lock")
            .current_dir(root.path().join("packages").join(member))
            .env("CARGO_NET_OFFLINE", "true")
            .env("INCAN_NO_BANNER", "1")
            .env("INCAN_STDLIB", &stdlib)
            .env("INCAN_STDLIB_DIR", &stdlib)
            .env("INCAN_HOME", root.path().join(".incan-home"))
            .env("INCAN_GENERATED_CARGO_TARGET_DIR", &generated_target)
            .spawn()?)
    };

    let baseline_output = spawn_lock("alpha")?.wait_with_output()?;
    assert_success(&baseline_output, "baseline workspace lock publisher");
    let run_git = |args: &[&str]| -> Result<Output, Box<dyn std::error::Error>> {
        Ok(Command::new("git").args(args).current_dir(root.path()).output()?)
    };
    let init_output = run_git(&["init", "--quiet"])?;
    assert_success(&init_output, "initialize workspace hygiene fixture");
    let add_output = run_git(&["add", "."])?;
    assert_success(&add_output, "stage workspace hygiene fixture");
    let commit_output = run_git(&[
        "-c",
        "user.name=Incan Tests",
        "-c",
        "user.email=tests@incan.invalid",
        "commit",
        "--quiet",
        "-m",
        "baseline",
    ])?;
    assert_success(&commit_output, "commit workspace hygiene fixture");
    let status_before = run_git(&["status", "--short"])?;
    assert_success(&status_before, "inspect clean workspace fixture before publication");
    assert!(
        status_before.stdout.is_empty(),
        "workspace fixture was not clean before lock publication: {}",
        String::from_utf8_lossy(&status_before.stdout)
    );

    let left = spawn_lock("alpha")?;
    let right = spawn_lock("zebra")?;
    let left_output = left.wait_with_output()?;
    let right_output = right.wait_with_output()?;
    assert_success(&left_output, "first concurrent workspace lock publisher");
    assert_success(&right_output, "second concurrent workspace lock publisher");

    let lock_path = root.path().join("oven.lock");
    let lock = oven_model::lock::IncanLock::load(&lock_path)?;
    assert!(!lock.deps_fingerprint.is_empty());
    assert!(
        root.path()
            .join("target/incan_lock/.oven.lock.publication.lock")
            .is_file(),
        "concurrent publishers must share one stable compiler-owned publication lock"
    );
    assert!(
        !root.path().join(".oven.lock.incan.lock").exists(),
        "concurrent lock publication must not leave a persistent project-root sidecar"
    );
    assert!(
        fs::read_dir(root.path())?
            .filter_map(Result::ok)
            .all(|entry| !entry.file_name().to_string_lossy().contains(".incan-stage-")),
        "failed workspace lock publication left a private staging file behind"
    );
    let status_after = run_git(&["status", "--short"])?;
    assert_success(&status_after, "inspect workspace fixture after publication");
    assert!(
        status_after.stdout.is_empty(),
        "incan lock dirtied a clean Git checkout:\n{}",
        String::from_utf8_lossy(&status_after.stdout)
    );
    Ok(())
}

#[test]
fn workspace_fmt_fans_out_in_member_order_without_changing_single_project_semantics()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::write(
        root.path().join("loaf.toml"),
        "[workspace]\nmembers = [\"packages/*\"]\n",
    )?;
    for name in ["zebra", "alpha"] {
        let member_root = root.path().join("packages").join(name);
        fs::create_dir_all(member_root.join("src"))?;
        fs::write(member_root.join("loaf.toml"), format!("[project]\nname = \"{name}\"\n"))?;
        fs::write(
            member_root.join("src/main.incn"),
            "def main() -> None:\n  println(\"formatted\")\n",
        )?;
    }

    let output = run_incan(root.path(), &["fmt", "--workspace"])?;
    assert_success(&output, "workspace fmt --workspace");
    let stdout = String::from_utf8(output.stdout)?;
    let alpha = stdout
        .find("workspace member alpha")
        .ok_or("alpha formatting output missing")?;
    let zebra = stdout
        .find("workspace member zebra")
        .ok_or("zebra formatting output missing")?;
    assert!(
        alpha < zebra,
        "workspace formatter did not use deterministic member order:\n{stdout}"
    );
    Ok(())
}

#[test]
fn workspace_check_fans_out_with_one_member_scoped_json_report() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::write(
        root.path().join("loaf.toml"),
        "[workspace]\nmembers = [\"packages/*\"]\n",
    )?;
    for (name, source) in [
        ("zebra", "def main() -> None:\n  println(\"zebra\")\n"),
        ("alpha", "def main() -> None:\n  println(\"alpha\")\n"),
    ] {
        let member_root = root.path().join("packages").join(name);
        fs::create_dir_all(member_root.join("src"))?;
        fs::write(
            member_root.join("loaf.toml"),
            format!("[project]\nname = \"{name}\"\n\n[project.scripts]\nmain = \"src/main.incn\"\n"),
        )?;
        fs::write(member_root.join("src/main.incn"), source)?;
    }

    let output = run_incan(root.path(), &["check", "--workspace", "--format", "json"])?;
    assert_success(&output, "workspace check --workspace");
    let report = parse_json_stdout(&output)?;
    assert_eq!(report["schema_version"], "incan.workspace.check.v1");
    assert_eq!(report["ok"], true);
    assert_eq!(report["workspace"]["selected_scope"]["origin"], "workspace");
    assert_eq!(report["results"][0]["member"]["name"], "alpha");
    assert_eq!(report["results"][1]["member"]["name"], "zebra");
    assert_eq!(report["results"][0]["report"]["ok"], true);
    assert_eq!(report["results"][1]["report"]["ok"], true);

    let member_output = run_incan(root.path(), &["check", "--member", "zebra", "--format", "json"])?;
    assert_success(&member_output, "workspace check --member");
    let member_report = parse_json_stdout(&member_output)?;
    assert_eq!(
        member_report["workspace"]["selected_scope"]["origin"],
        "explicit_members"
    );
    assert_eq!(member_report["results"].as_array().map(Vec::len), Some(1));
    assert_eq!(member_report["results"][0]["member"]["name"], "zebra");
    Ok(())
}

#[test]
fn workspace_run_and_version_require_one_explicit_member() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::write(
        root.path().join("loaf.toml"),
        "[workspace]\nmembers = [\"packages/*\"]\n",
    )?;
    for name in ["zebra", "alpha"] {
        let member_root = root.path().join("packages").join(name);
        fs::create_dir_all(member_root.join("src"))?;
        fs::write(
            member_root.join("loaf.toml"),
            format!(
                "[project]\nname = \"{name}\"\nversion = \"0.1.0\"\n\n[project.scripts]\nmain = \"src/main.incn\"\n"
            ),
        )?;
        fs::write(
            member_root.join("src/main.incn"),
            format!("def main() -> None:\n  println(\"{name}\")\n"),
        )?;
    }

    let run_output = run_incan(root.path(), &["run", "--member", "alpha"])?;
    assert_success(&run_output, "workspace run --member alpha");
    assert_eq!(String::from_utf8(run_output.stdout)?, "alpha\n");

    let multi_run = run_incan(root.path(), &["run", "--workspace"])?;
    assert!(
        !multi_run.status.success(),
        "workspace run unexpectedly accepted multiple members"
    );
    assert!(
        String::from_utf8(multi_run.stderr)?.contains("incan run requires exactly one workspace member"),
        "workspace run did not explain the one-member requirement"
    );

    let version_output = run_incan(root.path(), &["version", "patch", "--member", "alpha"])?;
    assert_success(&version_output, "workspace version --member alpha");
    let alpha_manifest = fs::read_to_string(root.path().join("packages/alpha/loaf.toml"))?;
    let zebra_manifest = fs::read_to_string(root.path().join("packages/zebra/loaf.toml"))?;
    assert!(alpha_manifest.contains("version = \"0.1.1\""));
    assert!(zebra_manifest.contains("version = \"0.1.0\""));
    Ok(())
}

#[test]
fn a_plain_build_of_a_toolchain_loaf_selects_its_rust_binaries_and_names_the_interim_receipt_inputs_issue1698()
-> Result<(), Box<dyn std::error::Error>> {
    // A Loaf that declares `[[rust.bin]]` roles and no Incan `main` script builds those binaries from the stored
    // compiler-suite plans. Those plans are still receipted by the workspace's Cargo manifest and lock, so a Loaf
    // without them is refused by name rather than with a missing-Cargo-input error; the refusal proves the
    // dispatch took the toolchain route, both alone and as the selected member of a rooted workspace.
    let root = tempfile::tempdir()?;
    let member_root = root.path().join("loaves/toolchain/incan-cli");
    fs::create_dir_all(member_root.join("src"))?;
    fs::write(
        root.path().join("loaf.toml"),
        "[project]\nname = \"incan\"\n\n[workspace]\nmembers = [\"loaves/toolchain/incan-cli\"]\ndefault-members = [\"loaves/toolchain/incan-cli\"]\n",
    )?;
    fs::write(
        member_root.join("loaf.toml"),
        "[project]\nname = \"incan-cli\"\n\n[[rust.bin]]\nname = \"incan\"\npath = \"src/main.rs\"\n",
    )?;
    fs::write(member_root.join("src/main.rs"), "fn main() {}\n")?;

    let member_build = run_incan(&member_root, &["build"])?;
    assert_failure(&member_build, "toolchain member build without workspace Cargo inputs");
    let member_stderr = String::from_utf8(member_build.stderr)?;
    assert!(
        member_stderr.contains("baked from the stored compiler-suite plans")
            && member_stderr.contains("keyed by the workspace Cargo.toml"),
        "member build did not name the interim receipt input: {member_stderr}"
    );
    assert!(
        !member_stderr.contains("[project.scripts].main"),
        "member build fell through to the Incan entrypoint path: {member_stderr}"
    );

    let root_build = run_incan(root.path(), &["build"])?;
    assert_failure(&root_build, "toolchain root build without workspace Cargo inputs");
    let root_stderr = String::from_utf8(root_build.stderr)?;
    assert!(
        root_stderr.contains("baked from the stored compiler-suite plans"),
        "root build did not select the default member's binaries: {root_stderr}"
    );

    let report_build = run_incan(&member_root, &["build", "--report", "json"])?;
    assert_failure(&report_build, "toolchain build with --report");
    assert!(
        String::from_utf8(report_build.stderr)?.contains("--report is not available for a toolchain binary build"),
        "the report surface was not refused by name"
    );

    // Declaring an Incan entrypoint beside the Rust role keeps the Incan meaning of a plain build.
    fs::write(
        member_root.join("loaf.toml"),
        "[project]\nname = \"incan-cli\"\n\n[project.scripts]\nmain = \"src/main.incn\"\n\n[[rust.bin]]\nname = \"incan\"\npath = \"src/main.rs\"\n",
    )?;
    let incan_build = run_incan(&member_root, &["build"])?;
    let incan_stderr = String::from_utf8(incan_build.stderr)?;
    assert!(
        !incan_stderr.contains("stored compiler-suite plans"),
        "a Loaf with a main script took the toolchain route: {incan_stderr}"
    );
    Ok(())
}

#[test]
fn workspace_env_fragments_are_inherited_only_through_explicit_member_extends() -> Result<(), Box<dyn std::error::Error>>
{
    let root = tempfile::tempdir()?;
    fs::write(
        root.path().join("loaf.toml"),
        r#"
[workspace]
members = ["packages/member"]

[workspace.envs.ci]
env-vars = { ROOT = "1", SHARED = "workspace" }

[workspace.envs.ci.scripts]
test = ["incan", "test"]
"#,
    )?;
    let member_root = root.path().join("packages/member");
    fs::create_dir_all(member_root.join("src"))?;
    fs::write(
        member_root.join("loaf.toml"),
        r#"
[project]
name = "member"

[tool.incan.envs.ci]
extends = ["workspace:ci"]
env-vars = { SHARED = "member", MEMBER = "1" }
"#,
    )?;

    let output = run_incan(&member_root, &["env", "show", "ci", "--format", "json"])?;
    assert_success(&output, "workspace env inheritance");
    let report = parse_json_stdout(&output)?;
    assert_eq!(
        report["overlay_chain"],
        serde_json::json!(["project", "default", "workspace:ci", "ci"])
    );
    assert_eq!(report["env_vars"]["ROOT"], "1");
    assert_eq!(report["env_vars"]["SHARED"], "member");
    assert_eq!(report["env_vars"]["MEMBER"], "1");
    assert_eq!(report["scripts"]["test"], serde_json::json!(["incan", "test"]));
    Ok(())
}
