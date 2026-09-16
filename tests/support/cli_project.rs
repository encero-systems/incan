//! Shared helpers for the `incan` command-line acceptance roots.
//!
//! `tests/cli_integration.rs` used to be one 12,473-line root holding every CLI acceptance test in the repository.
//! The Oven replay partitioner packs whole roots, so a root that large sets the floor for the entire lane no matter
//! how the shards are balanced -- and a file that large stops being readable long before that. The tests now live in
//! nine topic roots, and the surface they all drive lives here so it is written once.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use crate::support;

#[path = "canonical_projection.rs"]
mod canonical_projection;

/// Read generated Rust with RFC 120 projections decoded back to the spellings the source used.
///
/// Every linker-visible Incan-origin declaration reaches generated Rust as an encoded projection, so an assertion
/// written against a source spelling can only be evaluated after decoding. Decoding preserves the generated header
/// comment; the caller compares against this text rather than the raw file.
#[allow(dead_code)]
pub(crate) fn read_generated_rust(path: &std::path::Path) -> Result<String, Box<dyn std::error::Error>> {
    let decoded = canonical_projection::decoded_source_spellings(&fs::read_to_string(path)?);
    Ok(canonical_projection::reformatted_after_decode(&decoded).unwrap_or(decoded))
}

/// The compiler under test, resolved once in `support::incan_debug_binary` for every root.
#[allow(dead_code)]
pub(crate) fn incan_binary() -> PathBuf {
    support::incan_debug_binary()
}

#[allow(dead_code)]
pub(crate) fn run_incan(current_dir: &Path, args: &[&str]) -> Result<Output, Box<dyn std::error::Error>> {
    run_incan_with_env(current_dir, args, &[])
}

/// Publish a public-library provider before a separate consumer selects its package Loaf. This is intentionally
/// distinct from normal `build --lib`: only the explicit Oven command may create the provider handoff.
#[allow(dead_code)]
pub(crate) fn run_explicit_oven_bake(current_dir: &Path) -> Result<Output, Box<dyn std::error::Error>> {
    run_explicit_oven_bake_with_home(current_dir, None)
}

/// Bake one project while sharing a caller-selected standalone Oven home with its later workspace replay.
#[allow(dead_code)]
pub(crate) fn run_explicit_oven_bake_with_home(
    current_dir: &Path,
    standalone_incan_home: Option<&Path>,
) -> Result<Output, Box<dyn std::error::Error>> {
    let mut command = configured_incan_command(current_dir, &["oven", "bake", "--project", "."]);
    if !support::oven_compiler_suite_is_active()
        && let Some(incan_home) = standalone_incan_home
    {
        command.env("INCAN_HOME", incan_home);
    }
    support::configure_explicit_oven_bake_command(&mut command)?;
    let timing = support::command_timing_started();
    let output = command.output()?;
    support::report_command_timing("incan oven bake --project .", timing);
    Ok(output)
}

/// Copy one checked package handoff without following a symlink outside its fixture. The relocation test needs both the
/// public library output and its immutable package Loaf collection.
#[allow(dead_code)]
pub(crate) fn copy_fixture_directory(source: &Path, destination: &Path) -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let target = destination.join(entry.file_name());
        if file_type.is_symlink() {
            return Err(format!("fixture handoff contains symlink {}", entry.path().display()).into());
        }
        if file_type.is_dir() {
            copy_fixture_directory(&entry.path(), &target)?;
        } else if file_type.is_file() {
            fs::copy(entry.path(), target)?;
        } else {
            return Err(format!("fixture handoff contains unsupported path {}", entry.path().display()).into());
        }
    }
    Ok(())
}

/// Run a CLI command with a Cargo executable that records and rejects any launch.
#[cfg(unix)]
#[allow(dead_code)]
pub(crate) fn run_incan_with_failing_cargo_guard(
    current_dir: &Path,
    args: &[&str],
    guard_dir: &Path,
    marker: &Path,
) -> Result<Output, Box<dyn std::error::Error>> {
    run_incan_with_failing_cargo_guard_and_env(current_dir, args, guard_dir, marker, &[])
}

/// Install one Cargo executable that records and rejects any launch.
#[cfg(unix)]
#[allow(dead_code)]
pub(crate) fn install_failing_cargo_guard(
    guard_dir: &Path,
    marker: &Path,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    use std::os::unix::fs::PermissionsExt;

    fs::create_dir_all(guard_dir)?;
    let guard = guard_dir.join("cargo");
    fs::write(
        &guard,
        format!("#!/bin/sh\nprintf cargo > \"{}\"\nexit 97\n", marker.display()),
    )?;
    let mut permissions = fs::metadata(&guard)?.permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&guard, permissions)?;
    Ok(guard)
}

/// Run a guarded CLI command with explicit child-only environment handoffs.
#[cfg(unix)]
#[allow(dead_code)]
pub(crate) fn run_incan_with_failing_cargo_guard_and_env(
    current_dir: &Path,
    args: &[&str],
    guard_dir: &Path,
    marker: &Path,
    envs: &[(&str, &Path)],
) -> Result<Output, Box<dyn std::error::Error>> {
    let _guard = install_failing_cargo_guard(guard_dir, marker)?;
    let mut paths = vec![guard_dir.to_path_buf()];
    if let Some(inherited) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&inherited));
    }
    let mut command = configured_incan_command(current_dir, args);
    command.env("PATH", std::env::join_paths(paths)?);
    for (key, value) in envs {
        command.env(*key, *value);
    }
    let timing = support::command_timing_started();
    let output = command.output()?;
    support::report_command_timing(&format!("incan {} (Cargo guard)", args.join(" ")), timing);
    Ok(output)
}

#[allow(dead_code)]
pub(crate) fn run_incan_with_env(
    current_dir: &Path,
    args: &[&str],
    envs: &[(&str, &str)],
) -> Result<Output, Box<dyn std::error::Error>> {
    run_incan_with_env_and_removed(current_dir, args, envs, &[])
}

#[allow(dead_code)]
pub(crate) fn run_incan_with_env_and_removed(
    current_dir: &Path,
    args: &[&str],
    envs: &[(&str, &str)],
    removed_envs: &[&str],
) -> Result<Output, Box<dyn std::error::Error>> {
    let mut command = configured_incan_command(current_dir, args);
    for key in removed_envs {
        command.env_remove(key);
    }
    let timing = support::command_timing_started();
    let output = command.envs(envs.iter().copied()).output()?;
    support::report_command_timing(&format!("incan {}", args.join(" ")), timing);
    Ok(output)
}

#[allow(dead_code)]
pub(crate) fn configured_incan_command(current_dir: &Path, args: &[&str]) -> Command {
    let mut command = support::repo_command();
    command
        .args(args)
        .current_dir(current_dir)
        .env("CARGO_NET_OFFLINE", "true")
        .env("INCAN_NO_BANNER", "1")
        .env("INCAN_STDLIB", crate::support::repo_root().join("loaves/stdlib"))
        .env("INCAN_STDLIB_DIR", crate::support::repo_root().join("loaves/stdlib"));
    if !support::oven_compiler_suite_is_active() {
        command
            .env(
                "INCAN_GENERATED_CARGO_TARGET_DIR",
                support::generated_cargo_target_dir(),
            )
            .env("INCAN_INTERNAL_SDK_PROVIDER_STORE", support::sdk_provider_store())
            // Explicit provider bakes must not contend with a developer's ambient store when this test binary runs
            // outside the suite.
            .env("INCAN_HOME", current_dir.join(".incan-test"));
    }
    command
}

/// Return a Clang executable suitable for a header-only C ABI verifier fixture, when this host has one.
#[allow(dead_code)]
pub(crate) fn c_abi_test_clang() -> Option<String> {
    if let Some(executable) = std::env::var_os("INCAN_C_ABI_CLANG").filter(|value| !value.is_empty()) {
        return Some(executable.to_string_lossy().into_owned());
    }
    #[cfg(target_os = "macos")]
    {
        let output = Command::new("xcrun").args(["--find", "clang"]).output().ok()?;
        if !output.status.success() {
            return None;
        }
        let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        (!path.is_empty()).then_some(path)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let status = Command::new("clang").arg("--version").status().ok()?;
        status.success().then_some("clang".to_string())
    }
}

/// Run one Unix CLI probe in its own process group so a timed-out recursive subprocess tree can be terminated
/// together. Callers configure the command first, which keeps the watchdog independent of a fixture's environment.
#[cfg(unix)]
#[allow(dead_code)]
pub(crate) fn run_command_with_timeout(
    mut command: Command,
    label: &str,
    timeout: std::time::Duration,
) -> Result<(Output, bool), Box<dyn std::error::Error>> {
    use std::os::unix::process::CommandExt;

    command.process_group(0).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn()?;
    let started = std::time::Instant::now();
    let timing = support::command_timing_started();
    loop {
        if child.try_wait()?.is_some() {
            let output = child.wait_with_output()?;
            support::report_command_timing(&format!("{label} (timeout supervised)"), timing);
            return Ok((output, false));
        }
        if started.elapsed() >= timeout {
            // TERM is best-effort because the group can disappear between the timeout check and this signal. The
            // group-wide KILL below is the authoritative cleanup before any output pipe is reaped.
            let _ = signal_process_group(child.id(), libc::SIGTERM);
            let grace_started = std::time::Instant::now();
            while grace_started.elapsed() < std::time::Duration::from_secs(2) {
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            // Always address the full group with SIGKILL after the grace window. The group may contain a descendant
            // that retained the output pipes after the leader exited or ignored SIGTERM.
            if let Err(error) = signal_process_group(child.id(), libc::SIGKILL) {
                let leader_kill = child.kill();
                let leader_wait = child.wait();
                if let Err(kill_error) = leader_kill {
                    return Err(std::io::Error::other(format!(
                        "process-group SIGKILL failed ({error}); leader kill also failed ({kill_error})"
                    ))
                    .into());
                }
                leader_wait?;
                return Err(error.into());
            }
            let kill_started = std::time::Instant::now();
            while kill_started.elapsed() < std::time::Duration::from_secs(2) {
                if child.try_wait()?.is_some() {
                    let output = child.wait_with_output()?;
                    support::report_command_timing(&format!("{label} (timeout supervised)"), timing);
                    return Ok((output, true));
                }
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            return Err("timed-out Incan process group did not exit after SIGKILL".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
}

/// Send one signal to the complete Unix process group owned by a bounded CLI probe.
#[cfg(unix)]
#[allow(dead_code)]
pub(crate) fn signal_process_group(child_id: u32, signal: libc::c_int) -> std::io::Result<()> {
    let process_group = i32::try_from(child_id).map_err(|error| std::io::Error::other(error.to_string()))?;
    // SAFETY: The child was spawned with its PID as its process-group ID, and negating that validated positive ID
    // targets only the task-owned group. `signal` is one of libc's SIGTERM/SIGKILL constants supplied above.
    let result = unsafe { libc::kill(-process_group, signal) };
    if result == 0 {
        return Ok(());
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        Ok(())
    } else {
        Err(error)
    }
}

#[cfg(unix)]
#[allow(dead_code)]
pub(crate) fn run_incan_with_os_env(
    current_dir: &Path,
    args: &[&str],
    key: &str,
    value: std::ffi::OsString,
) -> Result<Output, Box<dyn std::error::Error>> {
    Ok(configured_incan_command(current_dir, args).env(key, value).output()?)
}

#[allow(dead_code)]
pub(crate) fn assert_success(output: &Output, context: &str) {
    assert!(
        output.status.success(),
        "{context} failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[allow(dead_code)]
pub(crate) fn assert_failure(output: &Output, context: &str) {
    assert!(
        !output.status.success(),
        "{context} unexpectedly succeeded\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[allow(dead_code)]
pub(crate) fn write_minimal_project(
    root: &Path,
    name: &str,
    extra_manifest: &str,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let src_dir = root.join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(
        root.join("loaf.toml"),
        format!(
            r#"[project]
name = "{name}"
version = "0.1.0"

[project.scripts]
main = "src/main.incn"
{extra_manifest}"#
        ),
    )?;

    let main_path = src_dir.join("main.incn");
    fs::write(
        &main_path,
        r#"def main() -> None:
  println("cli lifecycle ok")
"#,
    )?;
    Ok(main_path)
}

/// Write the canonical interop-only lock projection needed by `inspect interop-plan` without compiling SDK providers.
///
/// Interop-plan inspection re-hashes declared package inputs itself. The command tests below therefore need a valid
/// canonical semantic projection, not the unrelated provider-install work performed by `incan lock`.
#[allow(dead_code)]
pub(crate) fn write_locked_oven_interop_plan(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let manifest = incan::manifest::ProjectManifest::discover(root)?.ok_or("interop fixture manifest was missing")?;
    let interop = incan::oven_interop::locked_oven_interop_targets(&manifest)?;
    let lock = incan::lockfile::IncanLock::new_with_semantic(
        incan::version::INCAN_VERSION,
        "fixture".to_string(),
        incan::lockfile::CargoFeatureSelection::default(),
        incan::lockfile::SemanticLockState {
            oven: Some(incan::lockfile::LockedOvenState { interop }),
            ..Default::default()
        },
        String::new(),
    );
    lock.write(&root.join("oven.lock"))?;
    Ok(())
}

/// Write one workspace-root interop projection for a selected member without materializing SDK providers.
#[allow(dead_code)]
pub(crate) fn write_locked_workspace_oven_interop_plan(
    workspace_root: &Path,
    member_root: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let manifest = incan::manifest::ProjectManifest::discover(member_root)?
        .ok_or("workspace interop fixture member manifest was missing")?;
    let interop = incan::oven_interop::locked_oven_interop_targets(&manifest)?;
    let member_root = member_root
        .strip_prefix(workspace_root)?
        .to_string_lossy()
        .replace('\\', "/");
    let lock = incan::lockfile::IncanLock::new_with_semantic(
        incan::version::INCAN_VERSION,
        "fixture".to_string(),
        incan::lockfile::CargoFeatureSelection::default(),
        incan::lockfile::SemanticLockState {
            workspace_members: vec![incan::lockfile::LockedWorkspaceMember {
                member_root,
                sdk: None,
                packages: Vec::new(),
                feature_edges: Vec::new(),
                providers: Vec::new(),
                oven: Some(incan::lockfile::LockedOvenState { interop }),
            }],
            ..Default::default()
        },
        String::new(),
    );
    lock.write(&workspace_root.join("oven.lock"))?;
    Ok(())
}

#[allow(dead_code)]
pub(crate) fn parse_json_stdout(output: &Output) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    Ok(serde_json::from_slice(&output.stdout)?)
}

#[allow(dead_code)]
pub(crate) fn parse_jsonl_stdout(output: &Output) -> Result<Vec<serde_json::Value>, Box<dyn std::error::Error>> {
    let stdout = String::from_utf8(output.stdout.clone())?;
    stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| Ok(serde_json::from_str(line)?))
        .collect()
}

/// Assert the record contract every codegraph export owes a consumer, over an Incan-only fixture.
///
/// The language assertion is a property of *these fixtures*, which import nothing from Rust, not of the export
/// format: RFC 106 keeps one graph across both languages and makes `language` an attribute of each fact, so a
/// fixture that reaches a Rust item legitimately yields `"rust"` records and must not be checked with this helper.
#[allow(dead_code)]
pub(crate) fn assert_codegraph_record_contract(records: &[serde_json::Value]) {
    assert!(!records.is_empty(), "codegraph export should include a header record");
    assert_eq!(records[0]["record"], serde_json::json!("header"));
    assert_eq!(records[0]["schema_version"], serde_json::json!(7));
    assert_eq!(records[0]["languages"], serde_json::json!(["incan"]));
    assert!(
        records[0]["degraded"].is_boolean(),
        "codegraph snapshot metadata should carry degraded state: {}",
        records[0]
    );

    for record in records.iter().skip(1) {
        assert_eq!(
            record["language"],
            serde_json::json!("incan"),
            "every fact from an Incan-only fixture should be an Incan-language fact: {record}"
        );
        assert!(
            record["provenance"].is_string(),
            "codegraph fact records should carry provenance: {record}"
        );
        assert!(
            record["degraded"].is_boolean(),
            "codegraph fact records should carry degraded state: {record}"
        );

        if let Some(span) = record.get("span").filter(|span| span.is_object()) {
            assert_source_span_shape(span, record);
        }
        if let Some(span) = record.get("primary_span").filter(|span| span.is_object()) {
            assert_source_span_shape(span, record);
        }
    }
}

#[allow(dead_code)]
pub(crate) fn assert_source_span_shape(span: &serde_json::Value, record: &serde_json::Value) {
    assert!(
        span["file"].is_string()
            && span["start"].is_number()
            && span["end"].is_number()
            && span["start_line"].is_number()
            && span["start_column"].is_number()
            && span["end_line"].is_number()
            && span["end_column"].is_number(),
        "source-backed codegraph records should carry stable file and span identity: {record}"
    );
}

#[allow(dead_code)]
pub(crate) fn assert_source_files_include(
    report: &serde_json::Value,
    suffixes: &[&str],
) -> Result<(), Box<dyn std::error::Error>> {
    let files = report["source_files"]
        .as_array()
        .ok_or_else(|| format!("report source_files should be an array: {report}"))?;
    for suffix in suffixes {
        if !files
            .iter()
            .any(|file| file["path"].as_str().is_some_and(|path| path.ends_with(suffix)))
        {
            return Err(format!("expected report to include source file ending with {suffix}: {report}").into());
        }
    }
    Ok(())
}

#[allow(dead_code)]
pub(crate) fn stale_lockfile_without_changing_cargo_payload(root: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let lock_path = root.join("oven.lock");
    let original = fs::read_to_string(&lock_path)?;
    let stale = original.replace("deps-fingerprint = \"sha256:", "deps-fingerprint = \"sha256:stale");
    fs::write(lock_path, &stale)?;
    Ok(stale)
}

#[allow(dead_code)]
pub(crate) fn write_order_summary_bundle(project_dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let contract_dir = project_dir.join("contracts");
    fs::create_dir_all(&contract_dir)?;
    fs::write(
        contract_dir.join("order_summary.json"),
        r#"{
  "schema_version": 1,
  "stable_model_id": "orders.summary",
  "logical_type_name": "OrderSummary",
  "publishable": true,
  "fields": [
    {
      "name": "order_id",
      "type": "str",
      "alias": "orderId",
      "description": "Stable order identifier"
    },
    {
      "name": "total_cents",
      "type": "int"
    },
    {
      "name": "coupon_code",
      "type": "str",
      "nullable": true
    }
  ]
}
"#,
    )?;
    Ok(())
}
