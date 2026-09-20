//! Workspace scope and fan-out, canonical lock publication, and locked/frozen build refusals.
//!
//! One of nine roots split out of the former `tests/cli_integration.rs`. The shared command surface lives in
//! `incan_test_support::cli_project`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use incan_test_support as support;

use incan_test_support::cli_project;

use cli_project::*;

#[test]
fn workspace_inspect_reports_deterministic_scope_and_stale_member_locks() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::write(
        root.path().join("loaf.toml"),
        r#"
[project]
name = "root"

[workspace]
members = ["packages/*"]
default-members = ["zebra", "alpha"]
"#,
    )?;
    for name in ["alpha", "zebra"] {
        let member_root = root.path().join("packages").join(name);
        fs::create_dir_all(member_root.join("src"))?;
        fs::write(member_root.join("loaf.toml"), format!("[project]\nname = \"{name}\"\n"))?;
    }
    fs::write(root.path().join("packages/zebra/oven.lock"), "obsolete member lock")?;

    let default_output = run_incan(root.path(), &["workspace", "inspect", "--format", "json"])?;
    assert_success(&default_output, "workspace inspect from root");
    let default_report = parse_json_stdout(&default_output)?;
    assert_eq!(default_report["schema_version"], 1);
    assert_eq!(default_report["selected_scope"]["origin"], "default_members");
    assert_eq!(default_report["selected_scope"]["members"][0]["name"], "alpha");
    assert_eq!(default_report["selected_scope"]["members"][1]["name"], "zebra");
    assert_eq!(
        default_report["lock"]["stale_member_local_locks"]
            .as_array()
            .map(Vec::len),
        Some(1)
    );

    let current_member_output = run_incan(
        &root.path().join("packages/zebra/src"),
        &["workspace", "inspect", "--format", "json"],
    )?;
    assert_success(&current_member_output, "workspace inspect from member");
    let current_member_report = parse_json_stdout(&current_member_output)?;
    assert_eq!(current_member_report["selected_scope"]["origin"], "current_member");
    assert_eq!(current_member_report["selected_scope"]["members"][0]["name"], "zebra");

    let all_output = run_incan(
        root.path(),
        &["workspace", "inspect", "--format", "json", "--workspace"],
    )?;
    assert_success(&all_output, "workspace inspect --workspace");
    let all_report = parse_json_stdout(&all_output)?;
    assert_eq!(all_report["selected_scope"]["origin"], "workspace");
    assert_eq!(
        all_report["selected_scope"]["members"].as_array().map(Vec::len),
        Some(3)
    );
    Ok(())
}

#[test]
fn workspace_lock_is_published_once_at_the_root_from_any_member() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::write(
        root.path().join("loaf.toml"),
        r#"
[workspace]
members = ["packages/*"]

[workspace.rust-dependencies]
itoa = "1"
"#,
    )?;
    for (name, version) in [("alpha", "1.2.3"), ("zebra", "4.5.6")] {
        let member_root = root.path().join("packages").join(name);
        fs::create_dir_all(member_root.join("src"))?;
        fs::write(
            member_root.join("loaf.toml"),
            format!(
                "[project]\nname = \"{name}\"\nversion = \"{version}\"\n\n[project.scripts]\nmain = \"src/main.incn\"\n\n[project.features]\ndefault = [\"{name}\"]\n{name} = []\n{}",
                if name == "alpha" {
                    "\n[rust-dependencies]\nitoa = { workspace = true }\n"
                } else {
                    ""
                },
            ),
        )?;
        fs::write(
            member_root.join("src/main.incn"),
            if name == "alpha" {
                "from rust::itoa import Buffer\n\ndef main() -> None:\n  println(\"workspace lock\")\n"
            } else {
                "def main() -> None:\n  println(\"workspace lock\")\n"
            },
        )?;
        fs::create_dir_all(member_root.join("tests"))?;
        fs::write(
            member_root.join("tests/test_member.incn"),
            format!("from std.testing import test\n\n@test\ndef test_{name}() -> None:\n  assert True\n"),
        )?;
    }

    let output = run_incan(&root.path().join("packages/alpha"), &["lock"])?;
    assert_success(&output, "incan lock from workspace member");
    let root_lock = root.path().join("oven.lock");
    assert!(root_lock.is_file(), "workspace root lock was not written");
    let lock = oven_model::lock::IncanLock::load(&root_lock)?;
    assert_eq!(
        lock.cargo_lock_payload, "version = 4\n",
        "normal Oven lock publication must retain only the inert legacy Cargo payload"
    );
    let member_roots = lock
        .semantic
        .workspace_members
        .iter()
        .map(|member| member.member_root.as_str())
        .collect::<Vec<_>>();
    assert_eq!(member_roots, vec!["packages/alpha", "packages/zebra"]);
    let member_features = lock
        .semantic
        .workspace_members
        .iter()
        .map(|member| {
            member
                .packages
                .first()
                .map(|package| package.active_features.iter().cloned().collect::<Vec<_>>())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>();
    assert_eq!(
        member_features,
        vec![
            vec!["alpha".to_string(), "default".to_string()],
            vec!["default".to_string(), "zebra".to_string()]
        ]
    );
    let inspect_output = run_incan(
        &root.path().join("packages/alpha"),
        &["workspace", "inspect", "--format", "json", "--workspace"],
    )?;
    assert_success(&inspect_output, "workspace inspect after semantic lock publication");
    let inspect_report = parse_json_stdout(&inspect_output)?;
    assert_eq!(
        inspect_report["lock"]["state"]["semantic"]["workspace_members"]
            .as_array()
            .map(Vec::len),
        Some(2)
    );
    assert!(
        !root.path().join("packages/alpha/oven.lock").exists()
            && !root.path().join("packages/zebra/oven.lock").exists(),
        "workspace members must not receive authoritative lockfiles"
    );
    for (name, _) in [("alpha", "1.2.3"), ("zebra", "4.5.6")] {
        let member_root = root.path().join("packages").join(name);
        let bake_output = run_explicit_oven_bake(&member_root)?;
        assert_success(&bake_output, &format!("explicit Oven bake for workspace member {name}"));
        let build_output = run_incan(&member_root, &["build", "--locked"])?;
        assert_success(
            &build_output,
            &format!("incan build --locked from workspace member {name}"),
        );
        assert!(
            root.path()
                .join("packages")
                .join(name)
                .join("target/incan")
                .join(name)
                .join("oven/release")
                .join(name)
                .is_file(),
            "workspace member {name} did not emit its caller-owned Oven binary"
        );
    }

    // The member lock, explicit member builds, and aggregate workspace build
    // all operate on this one canonical topology. Retain each command mode,
    // but do not recreate the same members solely to inspect the JSON fan-out.
    let aggregate_output = run_incan(root.path(), &["build", "--workspace", "--report", "json"])?;
    assert_success(&aggregate_output, "workspace build --workspace --report json");
    let aggregate_report = parse_json_stdout(&aggregate_output)?;
    assert_eq!(aggregate_report["schema_version"], "incan.workspace.build.v1");
    assert_eq!(aggregate_report["ok"], true);
    assert_eq!(aggregate_report["workspace"]["selected_scope"]["origin"], "workspace");
    assert_eq!(aggregate_report["results"][0]["member"]["name"], "alpha");
    assert_eq!(aggregate_report["results"][1]["member"]["name"], "zebra");
    assert_eq!(
        aggregate_report["results"][0]["report"]["workspace"]["member_name"],
        "alpha"
    );
    assert_eq!(
        aggregate_report["results"][1]["report"]["workspace"]["member_name"],
        "zebra"
    );
    assert!(
        aggregate_report["results"][0]["report"]["dependencies"]["rust"]
            .as_array()
            .is_some_and(|dependencies| dependencies.iter().any(|dependency| dependency["crate_name"] == "itoa"))
    );

    let test_output = run_incan(root.path(), &["test", "--workspace", "--format", "json"])?;
    assert_success(&test_output, "workspace test --workspace --format json");
    let test_records = parse_jsonl_stdout(&test_output)?;
    assert_eq!(test_records[0]["event"], "workspace_scope");
    assert_eq!(test_records[0]["workspace"]["selected_scope"]["origin"], "workspace");
    let tested_members = test_records
        .iter()
        .filter(|record| record.get("test_id").is_some())
        .map(|record| record["workspace"]["member"]["name"].as_str().unwrap_or_default())
        .collect::<Vec<_>>();
    assert_eq!(tested_members, vec!["alpha", "zebra"]);
    assert!(
        test_records
            .iter()
            .filter(|record| record.get("summary").is_some())
            .all(|record| record["workspace"]["root"].is_string())
    );
    Ok(())
}

/// A command's `--features` selection selects what that command builds; it never rewrites the shared workspace lock.
/// Every member is recorded with its declared activation, so a bake or lock run with `--features json` inside a leaf
/// neither fails because a sibling without that feature was asked to resolve it, nor leaves the leaf's entry reading
/// differently from what the next command in another member would write (#1414).
#[test]
fn workspace_lock_scopes_command_feature_flags_to_the_invoking_member_issue1414()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::write(
        root.path().join("loaf.toml"),
        "[workspace]\nmembers = [\"leaf\", \"other\"]\n",
    )?;
    let leaf = root.path().join("leaf");
    fs::create_dir_all(leaf.join("src"))?;
    fs::write(
        leaf.join("loaf.toml"),
        "[project]\nname = \"leaf\"\nversion = \"0.1.0\"\n\n[project.features]\ndefault = []\njson = []\n",
    )?;
    fs::write(leaf.join("src/lib.incn"), "pub def value() -> int:\n    return 1\n")?;
    let other = root.path().join("other");
    fs::create_dir_all(other.join("src"))?;
    fs::write(
        other.join("loaf.toml"),
        "[project]\nname = \"other\"\nversion = \"0.1.0\"\n\n[project.scripts]\nmain = \"src/main.incn\"\n\n\
         [project.features]\ndefault = [\"other\"]\nother = []\n",
    )?;
    fs::write(
        other.join("src/main.incn"),
        "def main() -> None:\n    println(\"other\")\n",
    )?;

    let output = run_incan(&leaf, &["lock", "--no-default-features", "--features", "json"])?;
    assert_success(&output, "incan lock --features json from the leaf member");

    let lock = oven_model::lock::IncanLock::load(&root.path().join("oven.lock"))?;
    let member_features = lock
        .semantic
        .workspace_members
        .iter()
        .map(|member| {
            (
                member.member_root.clone(),
                member
                    .packages
                    .first()
                    .map(|package| package.active_features.iter().cloned().collect::<Vec<_>>())
                    .unwrap_or_default(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        member_features,
        vec![
            ("leaf".to_string(), vec!["default".to_string()]),
            ("other".to_string(), vec!["default".to_string(), "other".to_string()]),
        ],
        "every member is locked at its declared activation; the command's flags reach neither entry"
    );
    Ok(())
}

#[test]
fn workspace_root_library_without_a_script_publishes_the_canonical_lock_issue997()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir_all(root.path().join("src"))?;
    fs::write(
        root.path().join("loaf.toml"),
        r#"[project]
name = "root-library"
version = "0.1.0"

[workspace]
members = ["packages/member"]
"#,
    )?;
    fs::write(
        root.path().join("src/lib.incn"),
        "pub def answer() -> int:\n  return 42\n",
    )?;

    let member = root.path().join("packages/member");
    fs::create_dir_all(member.join("src"))?;
    fs::write(
        member.join("loaf.toml"),
        "[project]\nname = \"member-library\"\nversion = \"0.1.0\"\n",
    )?;
    fs::write(
        member.join("src/lib.incn"),
        "pub def member_answer() -> int:\n  return 7\n",
    )?;

    let output = run_incan(root.path(), &["lock"])?;
    assert_success(&output, "rooted workspace lock without scripts");

    let root_lock = root.path().join("oven.lock");
    assert!(root_lock.is_file(), "rooted workspace lock was not written");
    let lock = oven_model::lock::IncanLock::load(&root_lock)?;
    let roots = lock
        .semantic
        .workspace_members
        .iter()
        .map(|member| member.member_root.as_str())
        .collect::<Vec<_>>();
    assert_eq!(roots, vec!["", "packages/member"]);
    assert!(
        !member.join("oven.lock").exists(),
        "a workspace member must not receive a second authoritative lock"
    );
    Ok(())
}

#[test]
fn rooted_workspace_semantic_lock_is_relocation_stable_issue906() -> Result<(), Box<dyn std::error::Error>> {
    fn create_locked_workspace(
        root: &Path,
        prebuilt_artifact: &Path,
    ) -> Result<oven_model::lock::IncanLock, Box<dyn std::error::Error>> {
        fs::create_dir_all(root.join("src"))?;
        fs::write(
            root.join("loaf.toml"),
            r#"[project]
name = "root_lib"
version = "0.1.0"

[workspace]
members = ["consumer"]
default-members = ["root_lib", "consumer"]

[workspace.dependencies]
root_lib = { path = "." }
"#,
        )?;
        fs::write(root.join("src/lib.incn"), "pub def answer() -> int:\n  return 42\n")?;
        let artifact = root.join("target/lib");
        fs::create_dir_all(artifact.join("src"))?;
        for relative in ["Cargo.toml", "root_lib.incnlib", "src/lib.rs"] {
            fs::copy(prebuilt_artifact.join(relative), artifact.join(relative))?;
        }
        copy_fixture_directory(&prebuilt_artifact.join("oven"), &artifact.join("oven"))?;
        let consumer = root.join("consumer");
        fs::create_dir_all(consumer.join("src"))?;
        fs::write(
            consumer.join("loaf.toml"),
            r#"[project]
name = "consumer"
version = "0.1.0"

[project.scripts]
main = "src/main.incn"

[dependencies]
root_lib = { workspace = true }
"#,
        )?;
        fs::write(
            consumer.join("src/main.incn"),
            "from pub::root_lib import answer\n\n\ndef main() -> None:\n  println(answer())\n",
        )?;

        let lock_output = run_incan(root, &["lock"])?;
        assert_success(&lock_output, "rooted workspace lock generation");
        Ok(oven_model::lock::IncanLock::load(&root.join("oven.lock"))?)
    }

    let temp = tempfile::tempdir()?;
    let producer = temp.path().join("prebuilt/root_lib");
    fs::create_dir_all(producer.join("src"))?;
    fs::write(
        producer.join("loaf.toml"),
        r#"[project]
name = "root_lib"
version = "0.1.0"

"#,
    )?;
    fs::write(producer.join("src/lib.incn"), "pub def answer() -> int:\n  return 42\n")?;
    let library_output = run_explicit_oven_bake(&producer)?;
    assert_success(
        &library_output,
        "standalone root library explicit Oven bake before workspace activation",
    );

    let first = create_locked_workspace(&temp.path().join("first/root_lib"), &producer.join("target/lib"))?;
    let second = create_locked_workspace(&temp.path().join("relocated/root_lib"), &producer.join("target/lib"))?;

    assert_eq!(first.semantic, second.semantic);
    assert_eq!(first.deps_fingerprint, second.deps_fingerprint);
    let consumer = first
        .semantic
        .workspace_members
        .iter()
        .find(|member| member.member_root == "consumer")
        .ok_or("consumer semantic graph missing")?;
    assert!(
        consumer
            .packages
            .iter()
            .any(|package| package.package == "root_lib" && package.project_root.is_empty()),
        "the root package should use the workspace-root coordinate"
    );
    assert!(
        consumer
            .packages
            .iter()
            .any(|package| package.package == "consumer" && package.project_root == "consumer"),
        "the selected member package should use its workspace-relative coordinate"
    );
    assert!(
        consumer
            .feature_edges
            .iter()
            .any(|edge| edge.from == "consumer" && edge.to.is_empty()),
        "the member-to-root dependency edge should be workspace-relative"
    );
    Ok(())
}

#[test]
fn rooted_workspace_member_build_uses_direct_rust_dependencies_issue907() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir_all(root.path().join("src"))?;
    fs::write(
        root.path().join("loaf.toml"),
        r#"[project]
name = "root_lib"
version = "0.1.0"

[workspace]
members = ["consumer"]
default-members = ["consumer"]
"#,
    )?;
    fs::write(
        root.path().join("src/lib.incn"),
        "pub def root_marker() -> None:\n  pass\n",
    )?;

    let consumer = root.path().join("consumer");
    fs::create_dir_all(consumer.join("src"))?;
    fs::write(
        consumer.join("loaf.toml"),
        r#"[project]
name = "consumer"
version = "0.1.0"

[project.scripts]
main = "src/main.incn"

[rust-dependencies]
itoa = "1"
"#,
    )?;
    fs::write(
        consumer.join("src/main.incn"),
        "from rust::itoa import Buffer\n\n\ndef main() -> None:\n  println(\"direct dependency\")\n",
    )?;

    let bake_output = run_explicit_oven_bake(&consumer)?;
    assert_success(
        &bake_output,
        "explicit Oven bake for rooted workspace member with a direct Rust dependency",
    );
    let output = run_incan(&consumer, &["build", "--no-locked"])?;
    assert_success(&output, "rooted workspace member build with a direct Rust dependency");
    Ok(())
}

#[cfg(unix)]
#[test]
fn rooted_workspace_cold_lock_and_selected_member_preserve_identity_issues908_909_931()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir_all(root.path().join("src"))?;
    fs::write(
        root.path().join("src/lib.incn"),
        "from std.json import JsonValue\n\n\npub def answer() -> int:\n  return 42\n",
    )?;
    fs::write(
        root.path().join("src/main.incn"),
        "def main() -> None:\n  println(\"root executable\")\n",
    )?;

    fs::write(
        root.path().join("loaf.toml"),
        r#"[project]
name = "root_lib"
version = "0.1.0"

[project.scripts]
main = "src/main.incn"

[workspace]
members = ["consumer"]
default-members = ["root_lib", "consumer"]

[workspace.dependencies]
root_lib = { path = "." }

[workspace.rust-dependencies]
itoa = "1"
"#,
    )?;
    let consumer = root.path().join("consumer");
    fs::create_dir_all(consumer.join("src"))?;
    fs::write(
        consumer.join("loaf.toml"),
        r#"[project]
name = "consumer"
version = "0.1.0"

[project.scripts]
main = "src/main.incn"

[dependencies]
root_lib = { workspace = true }

[rust-dependencies]
regex = "1"
itoa = { workspace = true }
"#,
    )?;
    fs::write(
        consumer.join("src/main.incn"),
        "from pub::root_lib import answer\nfrom rust::regex import Regex\n\n\ndef main() -> None:\n  println(answer())\n",
    )?;
    fs::create_dir_all(consumer.join("tests"))?;
    fs::write(
        consumer.join("tests/test_workspace_rust_dependency.incn"),
        r#"from rust::itoa import Buffer
from std.testing import test


@test
def test_workspace_rust_dependency_is_available() -> None:
    assert True
"#,
    )?;

    let source_root = support::repo_root();
    let stdlib = source_root.join("loaves/stdlib");
    let incan_home = root.path().join(".incan-home");
    let provider_store = support::cold_sdk_provider_store_or(&incan_home.join("cache/providers/sdk-v2"));
    let generated_target = support::generated_cargo_target_dir_or(&incan_home.join("generated-target"));
    let configure = |cwd: &Path, args: &[&str]| -> Result<Command, Box<dyn std::error::Error>> {
        let mut command = support::repo_command();
        command
            .args(args)
            .current_dir(cwd)
            .env("CARGO_NET_OFFLINE", "true")
            .env("INCAN_NO_BANNER", "1")
            .env("INCAN_LOCK_PREHEAT", "1")
            .env("INCAN_SOURCE_ROOT", &source_root)
            .env("INCAN_STDLIB", &stdlib)
            .env("INCAN_STDLIB_DIR", &stdlib)
            .env("INCAN_HOME", &incan_home)
            .env("INCAN_INTERNAL_SDK_PROVIDER_STORE", &provider_store)
            .env("INCAN_GENERATED_CARGO_TARGET_DIR", &generated_target);
        if !support::oven_compiler_suite_is_active() {
            command
                .env_remove("INCAN_SDK_INVENTORY")
                .env_remove("INCAN_INTERNAL_SDK_PROVIDER_PATH_FILE");
        }
        if args == ["oven", "bake", "--project", "."] {
            support::configure_explicit_oven_bake_command(&mut command)?;
        }
        Ok(command)
    };
    let run = |cwd: &Path, args: &[&str]| -> Result<Output, Box<dyn std::error::Error>> {
        Ok(configure(cwd, args)?.output()?)
    };

    assert!(!root.path().join("target").exists());
    assert!(!incan_home.exists());
    assert!(!root.path().join("oven.lock").exists());

    // The same cold fixture covers both #908/#909's selected-root artifact and #931's bounded Oven
    // admission/fixed-point contract. The explicit package bake is the only permitted provider publication step.
    // The one cold publication now covers both library and executable profiles. Keep a bounded watchdog, but give
    // sealed or low-core runners enough headroom that the deliberately consolidated journey is not killed between
    // its library and executable halves.
    let artifact_preparation_timeout = std::thread::available_parallelism()
        .map(|parallelism| {
            if support::oven_compiler_suite_is_active() || parallelism.get() <= 4 {
                std::time::Duration::from_secs(6 * 60)
            } else {
                std::time::Duration::from_secs(3 * 60)
            }
        })
        .unwrap_or_else(|_| std::time::Duration::from_secs(6 * 60));
    let (provider_bake_output, timed_out) = run_command_with_timeout(
        configure(root.path(), &["oven", "bake", "--project", "."])?,
        "rooted workspace explicit provider bake",
        artifact_preparation_timeout,
    )?;
    assert!(
        !timed_out,
        "rooted workspace provider bake exceeded its bounded preparation window\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&provider_bake_output.stdout),
        String::from_utf8_lossy(&provider_bake_output.stderr)
    );
    assert_success(&provider_bake_output, "cold rooted workspace provider publication");

    let lock_path = root.path().join("oven.lock");
    assert!(
        lock_path.is_file(),
        "the explicit project bake must publish the canonical workspace lock before sealing its completed Loaf"
    );
    let first_lock = fs::read(&lock_path)?;
    let parsed = oven_model::lock::IncanLock::load(&lock_path)?;
    assert!(!parsed.deps_fingerprint.is_empty());

    let (first_lock_output, timed_out) = run_command_with_timeout(
        configure(root.path(), &["lock"])?,
        "cold rooted workspace lock",
        artifact_preparation_timeout,
    )?;
    assert!(
        !timed_out,
        "rooted workspace lock exceeded its bounded artifact-preparation window\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&first_lock_output.stdout),
        String::from_utf8_lossy(&first_lock_output.stderr)
    );
    assert_success(
        &first_lock_output,
        "rooted workspace lock fixed point after explicit bake",
    );
    assert_eq!(first_lock, fs::read(&lock_path)?);
    assert!(
        root.path().join("target/lib/root_lib.incnlib").is_file(),
        "the explicit provider bake must materialize the selected root library artifact"
    );
    if support::oven_compiler_suite_is_active() {
        assert!(
            !provider_store.exists(),
            "sealed compiler-suite execution must not publish a mutable per-fixture provider store: {}",
            provider_store.display()
        );
        let inventory_path = std::env::var_os("INCAN_SDK_INVENTORY")
            .map(PathBuf::from)
            .ok_or("compiler-suite workspace lock has no sealed SDK inventory")?;
        assert!(
            inventory_path.is_file(),
            "compiler-suite SDK inventory is not a regular file: {}",
            inventory_path.display()
        );
    } else {
        assert!(
            !provider_store.exists(),
            "normal Oven lock must not publish a mutable SDK provider store: {}",
            provider_store.display()
        );
        assert!(
            incan_home.join("oven/store/v2").is_dir(),
            "cold normal Oven lock did not materialize its selected Loaf into the bounded Oven store"
        );
    }

    let second_lock_output = run(root.path(), &["lock"])?;
    assert_success(&second_lock_output, "second rooted workspace lock fixed point");
    assert_eq!(first_lock, fs::read(&lock_path)?);

    let cargo_guard_dir = root.path().join("cargo-reuse-guard");
    let cargo_marker = root.path().join("cargo-reuse-invoked");
    let cargo_guard = install_failing_cargo_guard(&cargo_guard_dir, &cargo_marker)?;
    for projection in [
        root.path().join("target/lib/oven/package-loafs.json"),
        root.path().join("target/lib/oven/debug/libroot_lib.rlib"),
        root.path().join("target/lib/oven/release/libroot_lib.rlib"),
        root.path().join("target/lib/src/lib.rs"),
        root.path().join("target/incan/root_lib/oven/debug/root_lib"),
        root.path().join("target/incan/root_lib/oven/release/root_lib"),
        root.path().join("target/incan/root_lib/src/main.rs"),
    ] {
        fs::remove_file(&projection)?;
    }
    let mut second_bake = configure(root.path(), &["oven", "bake", "--project", "."])?;
    let mut guarded_path = vec![cargo_guard_dir];
    if let Some(inherited) = std::env::var_os("PATH") {
        guarded_path.extend(std::env::split_paths(&inherited));
    }
    second_bake
        .env("CARGO", &cargo_guard)
        .env("PATH", std::env::join_paths(&guarded_path)?)
        .env("INCAN_SDK_INVENTORY", root.path().join("missing-sdk-inventory.json"))
        .env_remove("INCAN_INTERNAL_SDK_PROVIDER_PATH_FILE");
    let (second_bake_output, timed_out) = run_command_with_timeout(
        second_bake,
        "rooted workspace unchanged explicit bake reuse",
        artifact_preparation_timeout,
    )?;
    assert!(
        !timed_out,
        "unchanged rooted workspace bake exceeded its reuse window\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&second_bake_output.stdout),
        String::from_utf8_lossy(&second_bake_output.stderr)
    );
    assert_success(&second_bake_output, "unchanged rooted workspace project-bake reuse");
    let second_bake_stdout = String::from_utf8_lossy(&second_bake_output.stdout);
    assert_eq!(
        second_bake_stdout.matches("Reused Oven library").count(),
        2,
        "unchanged debug and release profiles must both reuse their completed Loafs:\n{second_bake_stdout}"
    );
    assert_eq!(
        second_bake_stdout.matches("Reused Oven executable").count(),
        2,
        "unchanged mixed projects must reuse both executable profiles without frontend work:\n{second_bake_stdout}"
    );
    for restored in [
        root.path().join("target/lib/oven/package-loafs.json"),
        root.path().join("target/lib/oven/debug/libroot_lib.rlib"),
        root.path().join("target/lib/oven/release/libroot_lib.rlib"),
        root.path().join("target/lib/src/lib.rs"),
        root.path().join("target/incan/root_lib/oven/debug/root_lib"),
        root.path().join("target/incan/root_lib/oven/release/root_lib"),
        root.path().join("target/incan/root_lib/src/main.rs"),
    ] {
        assert!(
            restored.is_file(),
            "unchanged bake did not restore {}",
            restored.display()
        );
    }
    assert!(
        !cargo_marker.exists(),
        "unchanged explicit project bake launched Cargo instead of reusing its sealed Loafs"
    );
    assert_eq!(first_lock, fs::read(&lock_path)?);

    let library_output = run(root.path(), &["build", "--lib", "--member", "root_lib"])?;
    assert_success(&library_output, "rooted workspace library build after lock publication");
    assert_eq!(first_lock, fs::read(&lock_path)?);

    let strict_output = run(root.path(), &["build", "--lib", "--member", "root_lib", "--locked"])?;
    assert_success(&strict_output, "strict rooted workspace library build");
    assert_eq!(first_lock, fs::read(&lock_path)?);
    assert!(
        root.path().join("target/lib/oven/release/libroot_lib.rlib").is_file(),
        "the selected root must materialize a caller-owned direct-rustc library"
    );
    assert!(root.path().join("target/lib/root_lib.incnlib").is_file());

    let consumer_bake_output = run(&consumer, &["oven", "bake", "--project", "."])?;
    assert_success(
        &consumer_bake_output,
        "explicit Oven bake for rooted workspace consumer",
    );
    let consumer_output = run(&consumer, &["run", "src/main.incn", "--locked"])?;
    assert_success(&consumer_output, "consumer of the freshly rebuilt root library");
    assert_eq!(first_lock, fs::read(&lock_path)?);

    // #907 uses the same rooted workspace and canonical lock as #908/#909/#931.
    // Keep its distinct selected-member test command, but do not build a second
    // identical workspace merely to prove inherited Rust dependency selection.
    let inherited_dependency_test = run(
        root.path(),
        &[
            "test",
            "--member",
            "consumer",
            "--locked",
            "--fail-on-empty",
            "tests/test_workspace_rust_dependency.incn",
        ],
    )?;
    assert_success(
        &inherited_dependency_test,
        "rooted workspace selected-member test with an inherited Rust dependency",
    );
    assert_eq!(first_lock, fs::read(&lock_path)?);

    let mut rejected_features = configure(
        root.path(),
        &["build", "--lib", "--member", "root_lib", "--cargo-features", "sentinel"],
    )?;
    rejected_features
        .env("CARGO", &cargo_guard)
        .env("PATH", std::env::join_paths(&guarded_path)?);
    let rejected_features = rejected_features.output()?;
    assert_failure(
        &rejected_features,
        "completed library output with unsupported Cargo feature controls",
    );
    assert!(
        String::from_utf8_lossy(&rejected_features.stderr).contains("do not accept Cargo feature controls"),
        "completed-output selection bypassed the normal Cargo-feature rejection:\n{}",
        String::from_utf8_lossy(&rejected_features.stderr)
    );
    assert!(!cargo_marker.exists());

    let fresh_lock = fs::read_to_string(&lock_path)?;
    let stale_lock = fresh_lock.replace("deps-fingerprint = \"sha256:", "deps-fingerprint = \"sha256:stale");
    assert_ne!(
        fresh_lock, stale_lock,
        "the regression must corrupt canonical lock authority"
    );
    fs::write(&lock_path, &stale_lock)?;
    for (description, args) in [
        (
            "strict completed library build",
            vec!["build", "--lib", "--member", "root_lib", "--locked"],
        ),
        (
            "strict completed library JSON report",
            vec!["build", "--lib", "--member", "root_lib", "--locked", "--report", "json"],
        ),
        (
            "strict completed executable run",
            vec!["run", "src/main.incn", "--member", "root_lib", "--locked"],
        ),
    ] {
        let mut command = configure(root.path(), &args)?;
        command
            .env("CARGO", &cargo_guard)
            .env("PATH", std::env::join_paths(&guarded_path)?);
        let output = command.output()?;
        assert_failure(&output, description);
        let diagnostic = format!(
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            diagnostic.contains("workspace oven.lock is out of date"),
            "{description} did not preserve strict-lock diagnostic precedence:\n{diagnostic}"
        );
        assert!(
            !diagnostic.contains("no receipt-compatible Loaf"),
            "{description} inspected stale completed outputs before strict lock authority:\n{diagnostic}"
        );
        assert_eq!(fs::read_to_string(&lock_path)?, stale_lock);
        assert!(!cargo_marker.exists(), "{description} launched Cargo");
    }

    let mut non_strict = configure(root.path(), &["build", "--lib", "--member", "root_lib"])?;
    non_strict
        .env("CARGO", &cargo_guard)
        .env("PATH", std::env::join_paths(&guarded_path)?);
    let non_strict = non_strict.output()?;
    assert_success(
        &non_strict,
        "non-strict completed library build with stale canonical lock",
    );
    let non_strict_diagnostic = format!(
        "{}\n{}",
        String::from_utf8_lossy(&non_strict.stdout),
        String::from_utf8_lossy(&non_strict.stderr)
    );
    assert!(
        non_strict_diagnostic.contains("workspace oven.lock is out of date; continuing without using it"),
        "non-strict stale-lock build did not expose its tolerated-stale authority decision:\n{non_strict_diagnostic}"
    );
    assert_eq!(fs::read_to_string(&lock_path)?, stale_lock);
    assert!(!cargo_marker.exists(), "non-strict stale-lock build launched Cargo");
    Ok(())
}

#[test]
fn locked_build_synthesizes_unreferenced_selected_workspace_member_cargo_root() -> Result<(), Box<dyn std::error::Error>>
{
    let root = tempfile::tempdir()?;
    fs::create_dir_all(root.path().join("src"))?;
    fs::write(
        root.path().join("loaf.toml"),
        r#"[project]
name = "root_lib"
version = "0.1.0"

[workspace]
members = ["leaf", "sibling"]
default-members = ["root_lib", "leaf", "sibling"]
"#,
    )?;
    fs::write(
        root.path().join("src/lib.incn"),
        "pub def root_value() -> int:\n  return 1\n",
    )?;

    let vendor = root.path().join("vendor");
    fs::create_dir_all(&vendor)?;
    fs::write(
        vendor.join("Cargo.toml"),
        "[workspace]\nmembers = [\"foo-v1\", \"foo-v2\"]\n\n[workspace.package]\nversion = \"1.0.0\"\n",
    )?;
    for (directory, version) in [("foo-v1", "1.0.0"), ("foo-v2", "2.0.0")] {
        let package = vendor.join(directory);
        fs::create_dir_all(package.join("src"))?;
        let version = if directory == "foo-v1" {
            "version.workspace = true".to_string()
        } else {
            format!("version = \"{version}\"")
        };
        fs::write(
            package.join("Cargo.toml"),
            format!("[package]\nname = \"foo\"\n{version}\nedition = \"2021\"\n"),
        )?;
        fs::write(package.join("src/lib.rs"), "pub fn value() -> i64 { 1 }\n")?;
    }

    let leaf = root.path().join("leaf");
    fs::create_dir_all(leaf.join("src"))?;
    fs::write(
        leaf.join("loaf.toml"),
        r#"[project]
name = "leaf"
version = "0.2.0"

[rust-dependencies.json_alias]
package = "serde_json"
version = "1"

[rust-dependencies.old_flags]
package = "bitflags"
version = "=1.3.2"

[rust-dependencies.foo_old]
package = "foo"
path = "../vendor/foo-v1"
"#,
    )?;
    fs::write(
        leaf.join("src/lib.incn"),
        "from std.json import JsonValue\nfrom rust::json_alias import Value\nfrom rust::old_flags import bitflags\nfrom rust::foo_old import value\n\n\npub def leaf_value() -> int:\n  return 2\n",
    )?;

    let sibling = root.path().join("sibling");
    fs::create_dir_all(sibling.join("src"))?;
    fs::write(
        sibling.join("loaf.toml"),
        r#"[project]
name = "sibling"
version = "0.3.0"

[rust-dependencies.new_flags]
package = "bitflags"
version = "=2.11.0"

[rust-dependencies.foo_new]
package = "foo"
path = "../vendor/foo-v2"
"#,
    )?;
    fs::write(
        sibling.join("src/lib.incn"),
        "from std.regex import Regex as StdRegex\nfrom rust::new_flags import bitflags\nfrom rust::foo_new import value\n\n\npub def sibling_value() -> int:\n  return 3\n",
    )?;

    let bake_output = run_explicit_oven_bake_with_home(&leaf, Some(&root.path().join(".incan-test")))?;
    assert_success(
        &bake_output,
        "explicit Oven bake for the selected unreferenced workspace member",
    );
    let canonical = oven_model::lock::IncanLock::load(&root.path().join("oven.lock"))?;
    let member_roots = canonical
        .semantic
        .workspace_members
        .iter()
        .map(|member| member.member_root.as_str())
        .collect::<Vec<_>>();
    assert!(member_roots.contains(&"leaf"));
    assert!(member_roots.contains(&"sibling"));
    let _ = fs::remove_dir_all(root.path().join("target"));
    let _ = fs::remove_dir_all(leaf.join("target"));
    let locked_build = run_incan(root.path(), &["build", "--lib", "--member", "leaf", "--locked"])?;
    assert_success(
        &locked_build,
        "target-free locked build of an unreferenced selected workspace member",
    );

    let inspect = run_incan(&leaf, &["workspace", "inspect", "--format", "json"])?;
    assert_success(&inspect, "inspect selected member dependency authority");
    let inspect = parse_json_stdout(&inspect)?;
    let leaf_member = inspect["members"]
        .as_array()
        .and_then(|members| members.iter().find(|member| member["name"] == "leaf"))
        .ok_or("workspace inspection omitted the selected leaf member")?;
    let rust_dependencies = leaf_member["effective_dependencies"]["rust"]
        .as_object()
        .ok_or("selected member inspection had no effective Rust dependency map")?;
    assert!(rust_dependencies.contains_key("json_alias"));
    assert!(rust_dependencies.contains_key("old_flags"));
    assert!(rust_dependencies.contains_key("foo_old"));
    assert!(
        !rust_dependencies.contains_key("new_flags"),
        "the sibling's direct Rust dependency leaked into selected-member authority"
    );
    assert!(leaf.join("target/lib/oven/release/libleaf.rlib").is_file());
    Ok(())
}

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

#[test]
fn lock_generates_lockfile_for_manifest_project() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "cli_lock_project", "")?;

    let output = run_incan(
        tmp.path(),
        &["lock", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;

    assert_success(&output, "incan lock");
    let lock = fs::read_to_string(tmp.path().join("oven.lock"))?;
    assert!(lock.contains("# Auto-generated by Incan - do not edit manually"));
    assert!(lock.contains("[incan]"));
    assert!(
        !lock.contains("generated ="),
        "oven.lock must not include volatile generation timestamps"
    );
    assert!(lock.contains("deps-fingerprint = \"sha256:"));
    assert!(lock.contains("[cargo]"));
    let parsed = oven_model::lock::IncanLock::load(&tmp.path().join("oven.lock"))?;
    assert_eq!(
        parsed.cargo_lock_payload, "version = 4\n",
        "normal `incan lock` records semantic Incan state, not a generated Cargo resolution"
    );

    let second_output = run_incan(
        tmp.path(),
        &["lock", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(&second_output, "second incan lock");
    let second_lock = fs::read_to_string(tmp.path().join("oven.lock"))?;
    assert_eq!(lock, second_lock, "relocking unchanged inputs must be deterministic");
    Ok(())
}

#[cfg(unix)]
#[test]
fn lock_generates_semantic_state_without_starting_cargo() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "cli_guarded_lock_project", "")?;
    let marker = tmp.path().join("cargo-was-started");

    let output = run_incan_with_failing_cargo_guard(
        tmp.path(),
        &["lock", main_path.to_str().ok_or("main path was not valid UTF-8")?],
        &tmp.path().join("cargo-guard"),
        &marker,
    )?;

    assert_success(&output, "Cargo-guarded incan lock");
    assert!(
        !marker.exists(),
        "normal incan lock must not launch Cargo; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let lock = oven_model::lock::IncanLock::load(&tmp.path().join("oven.lock"))?;
    assert_eq!(lock.cargo_lock_payload, "version = 4\n");
    Ok(())
}

#[test]
fn semantic_lock_records_registry_dependency_input_changes() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(
        tmp.path(),
        "registry_resolution_lock",
        r#"
[rust-dependencies]
bitflags = "=1.3.2"
"#,
    )?;
    fs::write(
        &main_path,
        "rust.module(\"bitflags\")\n\n\ndef main() -> None:\n  pass\n",
    )?;

    let first_output = run_incan_with_env(tmp.path(), &["lock"], &[("INCAN_LOCK_PREHEAT", "0")])?;
    assert_success(&first_output, "canonical lock with bitflags 1.3.2");
    let first_bytes = fs::read(tmp.path().join("oven.lock"))?;
    let first = oven_model::lock::IncanLock::load(&tmp.path().join("oven.lock"))?;
    assert_eq!(
        first.cargo_lock_payload, "version = 4\n",
        "normal lock generation must not resolve a Cargo package graph"
    );

    let manifest_path = tmp.path().join("loaf.toml");
    let first_manifest = fs::read_to_string(&manifest_path)?;
    fs::write(&manifest_path, first_manifest.replace("=1.3.2", "=2.11.0"))?;
    let second_output = run_incan_with_env(tmp.path(), &["lock"], &[("INCAN_LOCK_PREHEAT", "0")])?;
    assert_success(&second_output, "canonical lock with bitflags 2.11.0");
    let second_bytes = fs::read(tmp.path().join("oven.lock"))?;
    let second = oven_model::lock::IncanLock::load(&tmp.path().join("oven.lock"))?;
    assert_eq!(second.cargo_lock_payload, "version = 4\n");
    assert_ne!(
        first.deps_fingerprint, second.deps_fingerprint,
        "the semantic dependency fingerprint must change with the declared registry input"
    );
    assert_ne!(
        first_bytes, second_bytes,
        "the published canonical lock must change byte-for-byte"
    );
    Ok(())
}

#[test]
fn build_lib_reuses_canonical_lock_when_manifest_dependency_is_unused() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let _main_path = write_minimal_project(
        tmp.path(),
        "cli_library_unused_manifest_dependency",
        r#"
[rust-dependencies]
serde_json = "1"
"#,
    )?;
    fs::write(
        tmp.path().join("src").join("lib.incn"),
        "pub def exported_value() -> int:\n  return 7\n",
    )?;

    let build = run_incan(tmp.path(), &["build", "--lib"])?;
    assert_success(&build, "incan build --lib with an unused manifest Rust dependency");
    let generated_manifest = fs::read_to_string(tmp.path().join("target/lib/Cargo.toml"))?;
    assert!(
        !generated_manifest.contains("serde_json"),
        "unused manifest dependencies must not expand the generated library beyond the canonical reachable graph:\n\
         {generated_manifest}"
    );
    Ok(())
}

#[test]
fn default_build_and_test_leave_stale_lockfile_unchanged() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "cli_default_stale_lock_project", "")?;
    let tests_dir = tmp.path().join("tests");
    fs::create_dir_all(&tests_dir)?;
    fs::write(
        tests_dir.join("test_main.incn"),
        r#"from std.testing import assert_eq

def test_smoke() -> None:
  assert_eq(1, 1)
"#,
    )?;

    let lock_output = run_incan(
        tmp.path(),
        &["lock", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(&lock_output, "incan lock before default build");
    let stale_lock = stale_lockfile_without_changing_cargo_payload(tmp.path())?;

    let build_output = run_incan(
        tmp.path(),
        &["build", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;

    assert_success(&build_output, "incan build with stale lockfile by default");
    assert_eq!(
        fs::read_to_string(tmp.path().join("oven.lock"))?,
        stale_lock,
        "default build must not rewrite an existing stale oven.lock"
    );

    let test_output = run_incan(tmp.path(), &["test"])?;
    assert_success(&test_output, "incan test with stale lockfile by default");
    assert_eq!(
        fs::read_to_string(tmp.path().join("oven.lock"))?,
        stale_lock,
        "default test must not rewrite an existing stale oven.lock"
    );
    Ok(())
}

#[test]
fn multi_entrypoint_lock_covers_project_scripts_and_tests_issue505() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(
        tmp.path(),
        "cli_multi_entry_lock_freshness_project",
        r#"
extra = "src/extra.incn"

[rust-dependencies]
tiny_helper = { path = "rust/tiny_helper" }
"#,
    )?;
    fs::write(
        &main_path,
        r#"pub def value() -> int:
  return 1

def main() -> None:
  println(value())
"#,
    )?;
    let extra_path = tmp.path().join("src").join("extra.incn");
    fs::write(
        &extra_path,
        r#"from rust::tiny_helper import plus_one

def main() -> None:
  println(plus_one(1))
"#,
    )?;

    let tests_dir = tmp.path().join("tests");
    fs::create_dir_all(&tests_dir)?;
    fs::write(
        tests_dir.join("test_main.incn"),
        r#"from std.serde.json import Serialize
from std.testing import assert_eq
from crate.main import value

model Event with Serialize:
  id: int

def test_value() -> None:
  event = Event(id=1)
  assert_eq(event.to_json(), "{\"id\":1}")
  assert_eq(value(), 1)
"#,
    )?;

    let helper_src = tmp.path().join("rust").join("tiny_helper").join("src");
    fs::create_dir_all(&helper_src)?;
    fs::write(
        helper_src
            .parent()
            .ok_or("helper src has no parent")?
            .join("Cargo.toml"),
        r#"[package]
name = "tiny_helper"
version = "0.1.0"
edition = "2021"
"#,
    )?;
    fs::write(
        helper_src.join("lib.rs"),
        "pub fn plus_one(value: i64) -> i64 { value + 1 }\n",
    )?;

    let assert_no_stale_warning = |output: &Output, context: &str| {
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !stderr.contains("oven.lock is out of date"),
            "{context} should not warn that oven.lock is stale, got:\n{stderr}"
        );
    };

    let default_lock_output = run_incan(tmp.path(), &["lock"])?;
    assert_success(&default_lock_output, "default incan lock");
    let bake_output = run_explicit_oven_bake(tmp.path())?;
    assert_success(&bake_output, "explicit Oven bake for all declared project entrypoints");

    let main_after_default_lock = run_incan(
        tmp.path(),
        &[
            "run",
            "--locked",
            main_path.to_str().ok_or("main path was not valid UTF-8")?,
        ],
    )?;
    assert_success(&main_after_default_lock, "incan run --locked main after default lock");
    assert_no_stale_warning(&main_after_default_lock, "incan run --locked main after default lock");
    assert_eq!(
        String::from_utf8_lossy(&main_after_default_lock.stdout).trim(),
        "1",
        "the conventional main target must replay its own sealed executable"
    );

    let locked_test_after_default_lock = run_incan(tmp.path(), &["test", "--locked"])?;
    assert_success(
        &locked_test_after_default_lock,
        "incan test --locked after default lock",
    );
    assert_no_stale_warning(
        &locked_test_after_default_lock,
        "incan test --locked after default lock",
    );

    let extra_after_default_lock = run_incan(
        tmp.path(),
        &[
            "run",
            "--locked",
            extra_path.to_str().ok_or("extra path was not valid UTF-8")?,
        ],
    )?;
    assert_success(&extra_after_default_lock, "incan run --locked extra after default lock");
    assert_no_stale_warning(&extra_after_default_lock, "incan run --locked extra after default lock");
    assert_eq!(
        String::from_utf8_lossy(&extra_after_default_lock.stdout).trim(),
        "2",
        "the declared extra target must replay its own sealed executable"
    );

    let main_report_output = run_incan(
        tmp.path(),
        &[
            "build",
            "--locked",
            "--report",
            "json",
            main_path.to_str().ok_or("main path was not valid UTF-8")?,
        ],
    )?;
    assert_success(&main_report_output, "sealed main build report after one explicit bake");
    let main_report = parse_json_stdout(&main_report_output)?;
    let extra_report_output = run_incan(
        tmp.path(),
        &[
            "build",
            "--locked",
            "--report",
            "json",
            extra_path.to_str().ok_or("extra path was not valid UTF-8")?,
        ],
    )?;
    assert_success(
        &extra_report_output,
        "sealed extra build report after one explicit bake",
    );
    let extra_report = parse_json_stdout(&extra_report_output)?;
    assert_eq!(
        main_report["entrypoint"],
        serde_json::json!(main_path.to_string_lossy()),
        "main replay report selected the wrong entrypoint"
    );
    assert_eq!(
        extra_report["entrypoint"],
        serde_json::json!(extra_path.to_string_lossy()),
        "extra replay report selected the wrong entrypoint"
    );
    let binary_path = |report: &serde_json::Value| {
        report["artifacts"]
            .as_array()
            .and_then(|artifacts| artifacts.iter().find(|artifact| artifact["kind"] == "binary"))
            .and_then(|artifact| artifact["path"].as_str())
            .map(str::to_string)
    };
    let main_binary = binary_path(&main_report).ok_or("main report had no binary artifact")?;
    let extra_binary = binary_path(&extra_report).ok_or("extra report had no binary artifact")?;
    assert_ne!(
        main_binary, extra_binary,
        "distinct declared scripts must retain distinct caller-visible native outputs"
    );

    let extra_lock_output = run_incan(
        tmp.path(),
        &["lock", extra_path.to_str().ok_or("extra path was not valid UTF-8")?],
    )?;
    assert_success(&extra_lock_output, "incan lock extra");

    let test_after_extra_lock = run_incan(tmp.path(), &["test", "--locked"])?;
    assert_success(&test_after_extra_lock, "incan test --locked after extra lock");
    assert_no_stale_warning(&test_after_extra_lock, "incan test --locked after extra lock");

    Ok(())
}

#[test]
fn build_locked_rejects_stale_lockfile() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "cli_locked_project", "")?;

    let lock_output = run_incan(
        tmp.path(),
        &["lock", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(&lock_output, "incan lock before locked build");

    fs::write(
        tmp.path().join("loaf.toml"),
        r#"[project]
name = "cli_locked_project"
version = "0.1.0"

[project.scripts]
main = "src/main.incn"

[rust-dependencies.serde]
version = "1.0"
"#,
    )?;
    fs::write(
        &main_path,
        r#"from rust::serde import Serialize

def main() -> None:
  println("cli lifecycle ok")
"#,
    )?;

    let build_output = run_incan(
        tmp.path(),
        &[
            "build",
            "--locked",
            main_path.to_str().ok_or("main path was not valid UTF-8")?,
        ],
    )?;

    assert_failure(&build_output, "incan build --locked with stale lockfile");
    let stderr = String::from_utf8_lossy(&build_output.stderr);
    assert!(
        stderr.contains("oven.lock is out of date"),
        "locked build should report stale lockfile, got:\n{stderr}"
    );
    assert!(
        stderr.contains("incan lock"),
        "locked build should tell users how to refresh the lockfile"
    );
    Ok(())
}

#[test]
fn build_frozen_rejects_missing_lockfile() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "cli_frozen_project", "")?;

    let build_output = run_incan(
        tmp.path(),
        &[
            "build",
            "--frozen",
            main_path.to_str().ok_or("main path was not valid UTF-8")?,
        ],
    )?;

    assert_failure(&build_output, "incan build --frozen without lockfile");
    let stderr = String::from_utf8_lossy(&build_output.stderr);
    assert!(
        stderr.contains("oven.lock is missing; run `incan lock`"),
        "frozen build should report missing lockfile, got:\n{stderr}"
    );
    assert!(
        !tmp.path().join("oven.lock").exists(),
        "frozen build must not create oven.lock after rejecting a missing lockfile"
    );
    Ok(())
}

#[test]
fn build_lock_policy_env_defaults_refuse_a_missing_lock() -> Result<(), Box<dyn std::error::Error>> {
    // `INCAN_LOCKED=1` is the `--locked` default and `INCAN_FROZEN=1` implies locked (and offline), so either one
    // refuses a project without `oven.lock` exactly as the flag would, and creates none. The environment defaults
    // are lock-policy inputs, not a Cargo surface; they survive the cutover (#1561).
    for (variable, context) in [
        ("INCAN_LOCKED", "INCAN_LOCKED=1 build"),
        ("INCAN_FROZEN", "INCAN_FROZEN=1 build"),
    ] {
        let tmp = tempfile::tempdir()?;
        let main_path = write_minimal_project(tmp.path(), "cli_lock_env_default_project", "")?;
        let main_arg = main_path.to_str().ok_or("main path was not valid UTF-8")?;

        let build_output = run_incan_with_env(tmp.path(), &["build", main_arg], &[(variable, "1")])?;

        assert_failure(&build_output, context);
        let stderr = String::from_utf8_lossy(&build_output.stderr);
        assert!(
            stderr.contains("oven.lock is missing; run `incan lock`"),
            "{context} should report the missing lockfile, got:\n{stderr}"
        );
        assert!(
            !tmp.path().join("oven.lock").exists(),
            "{context} must not create oven.lock after refusing"
        );
    }
    Ok(())
}

#[test]
fn build_no_flags_override_lock_policy_env_defaults() -> Result<(), Box<dyn std::error::Error>> {
    // `--no-locked` and `--no-frozen` negate the environment defaults on the command line, so the same project
    // without `oven.lock` builds; the negation is the last word.
    for (variable, flag, context) in [
        ("INCAN_LOCKED", "--no-locked", "INCAN_LOCKED=1 build --no-locked"),
        ("INCAN_FROZEN", "--no-frozen", "INCAN_FROZEN=1 build --no-frozen"),
    ] {
        let tmp = tempfile::tempdir()?;
        let main_path = write_minimal_project(tmp.path(), "cli_lock_env_negation_project", "")?;
        let main_arg = main_path.to_str().ok_or("main path was not valid UTF-8")?;

        let build_output = run_incan_with_env(tmp.path(), &["build", flag, main_arg], &[(variable, "1")])?;

        assert_success(&build_output, context);
    }
    Ok(())
}

#[test]
fn build_frozen_does_not_read_a_pre_rename_lock() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "cli_pre_rename_lock_project", "")?;
    let main_arg = main_path.to_str().ok_or("main path was not valid UTF-8")?;

    // Generate a genuinely current lock, then give it the pre-rename name. RFC 117 makes `oven.lock` the sole
    // generated resolution state and says `incan.lock` is not read afterwards, so the planted file must be invisible
    // rather than read and rejected. Planting a hand-written file could not tell those two outcomes apart: a frozen
    // build would fail either way, and for the wrong reason.
    let lock_output = run_incan(tmp.path(), &["lock", main_arg])?;
    assert_success(&lock_output, "incan lock before renaming the generated lock");
    fs::rename(tmp.path().join("oven.lock"), tmp.path().join("incan.lock"))?;

    let build_output = run_incan(tmp.path(), &["build", "--frozen", main_arg])?;

    assert_failure(&build_output, "incan build --frozen with only a pre-rename lock");
    let stderr = String::from_utf8_lossy(&build_output.stderr);
    assert!(
        stderr.contains("oven.lock is missing; run `incan lock`"),
        "a project holding only the pre-rename lock must report the canonical lock missing, got:\n{stderr}"
    );
    assert!(
        tmp.path().join("incan.lock").is_file(),
        "the pre-rename lock is inert state, not something the compiler consumes or removes"
    );
    Ok(())
}

#[test]
fn build_frozen_uses_existing_lockfile_without_network() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "cli_frozen_existing_lock_project", "")?;

    let lock_output = run_incan(
        tmp.path(),
        &["lock", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(&lock_output, "incan lock before frozen build");

    let build_output = run_incan(
        tmp.path(),
        &[
            "build",
            "--frozen",
            main_path.to_str().ok_or("main path was not valid UTF-8")?,
        ],
    )?;

    assert_success(&build_output, "incan build --frozen with existing lockfile");
    let stdout = String::from_utf8_lossy(&build_output.stdout);
    assert!(
        stdout.contains("Oven build successful"),
        "frozen build should complete with the existing lockfile, got:\n{stdout}"
    );
    Ok(())
}
