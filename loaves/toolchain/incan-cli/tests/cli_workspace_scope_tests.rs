#![allow(dead_code)] // Split test binaries intentionally use different parts of shared support fragments.

//! Workspace scope and fan-out, canonical lock publication, and locked/frozen build refusals.
//!
//! One of nine roots split out of the former `tests/cli_integration.rs`. The shared command surface lives in
//! `incan_test_support::cli_project`.

include!("support/cli_workspace_and_lock_tests_root.rs");

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use incan_test_support as support;

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
