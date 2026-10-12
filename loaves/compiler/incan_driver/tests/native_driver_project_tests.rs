//! Oven-built pinned driver conformance at the Incan-plan-to-MIR boundary.

#[path = "native_driver_project_tests/corpus.rs"]
mod corpus;

#[path = "native_driver_project_tests/enum_alias_admission.rs"]
mod enum_alias_admission;

#[path = "native_driver_project_tests/dev7_carrier_admission.rs"]
mod dev7_carrier_admission;

#[path = "native_driver_project_tests/ordinary_rust_dependencies.rs"]
mod ordinary_rust_dependencies;

use incan_test_support as support;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::OnceLock;

/// Prove narrowed union tests and matches retain their physical tag layouts through chains and optional payloads.
#[test]
fn narrowed_union_layout_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("narrowed-union-layout")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"def chain(value: bool | float | int | str) -> str:
    if isinstance(value, bool):
        return "bool"
    elif isinstance(value, int):
        return "int"
    elif isinstance(value, float):
        return "float"
    elif isinstance(value, str):
        return value.upper()
    return "unknown"

def subset(value: int | bool | str) -> str:
    if isinstance(value, int):
        return "int"
    else:
        match value:
            bool(flag) => return f"bool:{flag}"
            str(text) => return text.upper()

def optional(value: Option[bool | float | int | str]) -> str:
    if isinstance(value, bool):
        return "bool"
    elif isinstance(value, int):
        return "int"
    elif isinstance(value, float):
        return "float"
    elif isinstance(value, str):
        return value.upper()
    return "none"

def main() -> None:
    println(chain(true))
    println(chain(4))
    println(chain(2.5))
    println(chain("text"))
    println(subset(3))
    println(subset(false))
    println(subset("word"))
    println(optional(Some(true)))
    println(optional(Some(7)))
    println(optional(Some(2.5)))
    println(optional(Some("value")))
    println(optional(None))
"#,
    )?;
    census::require_named_refusal(
        "emit_dependency_unions_and_surfaces",
        "dependency_unions_across_the_package_boundary",
        "unsupported Body IR unresolved ArgumentBinding",
    )
}

/// Prove positional and keyword rest packing, bound function values, method calls and empty rest collections against
/// legacy.
#[test]
fn rest_arguments_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("rest-arguments")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"model Thing:
    name: str

def collect(prefix: str, *items: int, **labels: str) -> int:
    println(f"{prefix} {len(items)} {labels["name"]}")
    return sum(items)

def takes_rest(value: Thing, *rest: int) -> str:
    return f"{value.name} {len(rest)} {sum(rest)}"

class Collector:
    def collect(self, *items: int, **labels: str) -> int:
        println(labels["name"])
        return sum(items)

def main() -> None:
    xs_direct = [2, 3]
    kw_direct = {"name": "spread"}
    xs_bound = [2, 3]
    kw_bound = {"name": "spread"}
    xs_method = [2, 3]
    kw_method = {"name": "spread"}
    println(collect("direct", 1, *xs_direct, name="inline", **kw_direct))
    f = collect
    println(f("bound", 4, *xs_bound, name="inline", **kw_bound))
    collector = Collector()
    println(collector.collect(5, *xs_method, name="inline", **kw_method))
    thing = Thing(name="thing")
    println(takes_rest(thing, 1, 2, 3))
    println(takes_rest(thing))
"#,
    )
}

/// Replace `destination` with a copy of the source tree, without admitting a checkout's generated targets.
///
/// The destination is removed first: in a kept workspace, a source file deleted from the checkout must not survive
/// from an earlier run. Callers name source directories, never a project root whose `target/` they want to keep.
fn copy_tree(source: &Path, destination: &Path) -> Result<(), Box<dyn std::error::Error>> {
    if destination.exists() {
        fs::remove_dir_all(destination)?;
    }
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let target = destination.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

/// Select the installed pinned compiler including rustc-dev, which ordinary suite compiler closures omit.
fn pinned_driver_rustc() -> Result<PathBuf, Box<dyn std::error::Error>> {
    let output = Command::new("rustup")
        .args(["which", "--toolchain", "1.98.0", "rustc"])
        .output()?;
    success(&output, "pinned compiler selection");
    Ok(PathBuf::from(String::from_utf8(output.stdout)?.trim()))
}

/// Bake one debug fixture in the persistent Oven home and preserve full failure diagnostics.
fn bake(project: &Path, home: &Path) -> Result<(), Box<dyn std::error::Error>> {
    bake_with_reuse_requirement(project, home, false)
}

/// A manifest-backed Body IR executable skips frontend/lowering/native work, restores outputs and rejects edits.
#[test]
fn native_source_completed_output_reuses_before_frontend() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let root = fixture.scratch("native-source-completed-output")?.canonicalize()?;
    fs::create_dir_all(root.join("src"))?;
    fs::create_dir_all(root.join("target/native"))?;
    fs::write(
        root.join("loaf.toml"),
        "[project]\nname='native_source_completed'\nversion='0.1.0'\n",
    )?;
    let source = root.join("src/main.incn");
    let original = "def main() -> None:\n    println(42)\n";
    fs::write(&source, original)?;
    let native = root.join("target/native/program");
    let closure = corpus::runtime_closure(&fixture.formatting)?;
    let driver = fixture.driver_binary("debug");
    let compile = |required: bool, trace: bool| {
        let mut command = corpus::source_command(&driver, &source, &native, &fixture.sysroot, &closure);
        command.env_remove("INCAN_TEST_REQUIRE_COMPLETED_PROJECT_REUSE");
        command.env_remove("INCAN_TRACE_DIR").env_remove("INCAN_TRACE_PARENT");
        #[cfg(target_os = "macos")]
        command
            .env("MACOSX_DEPLOYMENT_TARGET", "12.0")
            .env("SDKROOT", "/nonexistent-incan-native-reuse-sdk");
        if required {
            command.env("INCAN_TEST_REQUIRE_COMPLETED_PROJECT_REUSE", "1");
        }
        if trace {
            command.env("INCAN_TRACE_DIR", root.join("target/reuse-trace"));
        }
        command.output()
    };
    success(&compile(false, false)?, "first native source compilation");
    let before = fs::metadata(&native)?;
    let bytes = fs::read(&native)?;
    #[cfg(target_os = "macos")]
    {
        let headers = Command::new("otool").arg("-l").arg(&native).output()?;
        success(&headers, "native source pinned platform metadata");
        let headers = String::from_utf8(headers.stdout)?;
        assert!(headers.contains("minos 11.0"));
        assert!(headers.contains("sdk 26.5"));
    }
    success(&compile(true, true)?, "unchanged native source reuse before frontend");
    let after = fs::metadata(&native)?;
    assert_eq!(bytes, fs::read(&native)?);
    assert_eq!(before.modified()?, after.modified()?);
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        assert_eq!(before.ino(), after.ino());
    }
    let mut hits = 0;
    for entry in fs::read_dir(root.join("target/reuse-trace"))? {
        for line in fs::read_to_string(entry?.path())?.lines() {
            let event: serde_json::Value = serde_json::from_str(line)?;
            if event["kind"] != "begin" {
                continue;
            }
            let name = event["function"].as_str().ok_or("trace function missing")?;
            hits += usize::from(name == "incan_driver::reuse_native_source_output");
            assert!(
                ![
                    "incan_rustc_driver::checked_modules",
                    "incan_rustc_driver::analyze_modules",
                    "incan_rustc_driver::lower_program",
                    "incan_rustc_driver::native_compile",
                ]
                .contains(&name),
                "unchanged native source entered {name}"
            );
        }
    }
    assert_eq!(hits, 1);
    fs::remove_file(&native)?;
    success(&compile(true, false)?, "native source projection recovery");
    let run = Command::new(&native).output()?;
    success(&run, "recovered native source execution");
    assert_eq!(run.stdout, b"42\n");
    fs::write(&source, "def main() -> None:\n    println(47)\n")?;
    let refused = compile(true, false)?;
    if refused.status.success() {
        // A previous suite run may already have compiled this exact generation; JEC should reuse it.
        let run = Command::new(&native).output()?;
        success(&run, "cached changed native source execution");
        assert_eq!(run.stdout, b"47\n");
        assert_ne!(bytes, fs::read(&native)?);
    } else {
        assert!(
            String::from_utf8_lossy(&refused.stderr)
                .contains("completed project reuse missed before frontend preparation")
        );
        assert_eq!(bytes, fs::read(&native)?);
    }
    success(&compile(false, false)?, "changed native source compilation");
    let run = Command::new(&native).output()?;
    success(&run, "changed native source execution");
    assert_eq!(run.stdout, b"47\n");
    fs::write(&source, original)?;
    success(&compile(true, false)?, "exact source restoration native reuse");
    assert_eq!(bytes, fs::read(native)?);
    Ok(())
}

/// Shared Rust caller generations replace read-only projections without modifying aliases or invoking compilation.
#[cfg(unix)]
#[test]
fn completed_rust_caller_reuse_replaces_read_only_generation() -> Result<(), Box<dyn std::error::Error>> {
    use std::os::unix::fs::PermissionsExt;

    let project = tempfile::tempdir()?;
    let root = fs::canonicalize(project.path())?;
    let producer = root.join("producer");
    let consumer = root.join("consumer");
    let provider = root.join("provider");
    let home = support::oven_fixture_home()?;
    fs::create_dir_all(provider.join("src"))?;
    fs::write(
        provider.join("loaf.toml"),
        "[project]\nname = \"recovery_provider\"\nversion = \"0.1.0\"\n",
    )?;
    fs::write(
        provider.join("src/lib.incn"),
        "pub def value() -> int:\n    return 42\n",
    )?;
    bake(&provider, &home)?;
    let original = "use recovery_provider::caller::incan::value;\nfn main() { println!(\"{}\", value()); }\n";
    let changed = "use recovery_provider::caller::incan::value;\nfn main() { println!(\"{}\", value() + 1); }\n";
    for owner in [&producer, &consumer] {
        fs::create_dir_all(owner.join("src"))?;
        fs::write(
            owner.join("loaf.toml"),
            "[project]\nname = \"rust_generation_recovery\"\nversion = \"0.1.0\"\n[dependencies]\nrecovery_provider = { loaf = \"recovery_provider\", path = \"../provider\" }\n[[rust.bin]]\nname = \"rust_generation_recovery\"\npath = \"src/main.rs\"\n",
        )?;
        fs::write(owner.join("src/main.rs"), original)?;
    }
    bake(&producer, &home)?;
    bake_with_reuse_requirement(&consumer, &home, true)?;
    let relative = "target/rust/debug/rust_generation_recovery";
    let native = consumer.join(relative);
    let receipt = consumer.join("target/rust/receipts/rust_generation_recovery-debug.json");
    let original_native = fs::read(&native)?;
    let original_receipt = fs::read(&receipt)?;
    fs::set_permissions(&native, fs::Permissions::from_mode(0o555))?;
    let alias = root.join("prior-generation-alias");
    fs::hard_link(&native, &alias)?;
    for owner in [&producer, &consumer] {
        fs::write(owner.join("src/main.rs"), changed)?;
    }
    bake(&producer, &home)?;
    bake_with_reuse_requirement(&consumer, &home, true)?;
    assert_eq!(fs::read(&native)?, fs::read(producer.join(relative))?);
    let output = Command::new(&native).output()?;
    success(&output, "reused Rust caller generation");
    assert_eq!(output.stdout, b"43\n");
    assert_eq!(fs::read(&alias)?, original_native);
    assert_eq!(fs::metadata(&alias)?.permissions().mode() & 0o777, 0o555);
    assert_eq!(fs::metadata(&native)?.permissions().mode() & 0o777, 0o755);
    let modified = fs::metadata(&native)?.modified()?;
    let receipt_modified = fs::metadata(&receipt)?.modified()?;
    bake_with_reuse_requirement(&consumer, &home, true)?;
    assert_eq!(fs::metadata(&native)?.modified()?, modified);
    assert_eq!(fs::metadata(&receipt)?.modified()?, receipt_modified);
    fs::write(consumer.join("src/main.rs"), original)?;
    bake_with_reuse_requirement(&consumer, &home, true)?;
    assert_eq!(fs::read(&native)?, original_native);
    assert_eq!(fs::read(&receipt)?, original_receipt);
    Ok(())
}

/// Standalone builds retain native results in the bounded store even when their caller projection is removed.
#[test]
fn standalone_file_build_reuses_stored_native_output_after_projection_loss() -> Result<(), Box<dyn std::error::Error>> {
    let project = tempfile::tempdir()?;
    let root = fs::canonicalize(project.path())?;
    let home = root.join("home");
    let output = root.join("output");
    let source = root.join("program.incn");
    let original = "def main() -> None:\n    print(42)\n";
    fs::write(&source, original)?;
    let rustc = pinned_driver_rustc()?;
    let compile = |require_reuse: bool| -> Result<Output, Box<dyn std::error::Error>> {
        let mut command = support::repo_command();
        command
            .env("INCAN_HOME", &home)
            .env("RUSTC", &rustc)
            .arg("build")
            .arg(&source)
            .arg(&output);
        if require_reuse {
            command.env("INCAN_TEST_REQUIRE_STORED_NATIVE_REUSE", "1");
        }
        Ok(command.output()?)
    };
    success(&compile(false)?, "initial standalone build");
    let native = output.join("oven/release/program");
    let original_native = fs::read(&native)?;
    let store = oven_store::store::OvenStore::new(
        oven_store::store::store_root_for_home(&home),
        oven_store::store::OvenStoreLimits::new(
            oven_store::DEFAULT_OVEN_MAX_PHYSICAL_BYTES,
            oven_store::DEFAULT_OVEN_MAX_DOMAIN_PHYSICAL_BYTES,
            oven_store::DEFAULT_OVEN_MAX_DOMAIN_LOGICAL_BYTES,
        ),
    );
    assert_eq!(
        store
            .manifests_for_selection()?
            .iter()
            .filter(|entry| entry.domain == "generated-rust-binary-v2")
            .count(),
        1,
        "standalone native output must be admitted in the bounded store",
    );
    fs::remove_dir_all(&output)?;
    success(&compile(true)?, "standalone replay after projection loss");
    assert_eq!(fs::read(&native)?, original_native);
    let restored = Command::new(&native).output()?;
    success(&restored, "replayed standalone execution");
    assert_eq!(restored.stdout, b"42\n");
    let modified = fs::metadata(&native)?.modified()?;
    success(&compile(true)?, "warm standalone replay");
    assert_eq!(fs::metadata(&native)?.modified()?, modified);
    fs::write(&source, "def main() -> None:\n    print(43)\n")?;
    assert!(
        !compile(true)?.status.success(),
        "changed source must not reuse the previous unit"
    );
    assert_eq!(fs::read(&native)?, original_native);
    success(&compile(false)?, "changed standalone build");
    let changed = Command::new(&native).output()?;
    success(&changed, "changed standalone execution");
    assert_eq!(changed.stdout, b"43\n");
    fs::write(&source, original)?;
    success(&compile(true)?, "original standalone generation recovery");
    assert_eq!(fs::read(&native)?, original_native);
    Ok(())
}

/// Restoring exact library inputs selects the original native generation and its immutable producer receipt.
#[test]
fn completed_library_source_roundtrip_reuses_original_admitted_generation() -> Result<(), Box<dyn std::error::Error>> {
    let project = tempfile::tempdir()?;
    let root = fs::canonicalize(project.path())?;
    let home = support::oven_fixture_home()?;
    fs::create_dir_all(root.join("src"))?;
    fs::write(
        root.join("loaf.toml"),
        "[project]\nname = \"recovery_roundtrip\"\nversion = \"0.1.0\"\n",
    )?;
    let source = root.join("src/lib.incn");
    let original = "pub def value() -> int:\n    return 42\n";
    fs::write(&source, original)?;
    bake(&root, &home)?;
    let native = root.join("target/lib/oven/debug/librecovery_roundtrip.rlib");
    let receipt = root.join(".incan/oven/library-debug-receipt.json");
    let metadata = root.join("target/lib/recovery_roundtrip.incnlib");
    let native_before = fs::read(&native)?;
    let receipt_before = fs::read(&receipt)?;
    let metadata_before = fs::read(&metadata)?;
    fs::write(&source, "pub def value() -> int:\n    return 43\n")?;
    bake(&root, &home)?;
    assert_ne!(fs::read(&native)?, native_before);
    assert_ne!(fs::read(&receipt)?, receipt_before);
    fs::write(&source, original)?;
    bake_with_reuse_requirement(&root, &home, true)?;
    assert_eq!(fs::read(&native)?, native_before);
    assert_eq!(fs::read(&receipt)?, receipt_before);
    assert_eq!(fs::read(&metadata)?, metadata_before);
    let receipt_modified = fs::metadata(&receipt)?.modified()?;
    bake_with_reuse_requirement(&root, &home, true)?;
    assert_eq!(fs::metadata(&receipt)?.modified()?, receipt_modified);
    Ok(())
}

/// Exact executable source restoration must recover its original admitted native bytes and producer receipt.
#[test]
fn completed_executable_source_roundtrip_reuses_original_admitted_generation() -> Result<(), Box<dyn std::error::Error>>
{
    let project = tempfile::tempdir()?;
    let root = fs::canonicalize(project.path())?;
    let home = support::oven_fixture_home()?;
    fs::create_dir_all(root.join("src"))?;
    fs::write(
        root.join("loaf.toml"),
        "[project]\nname = \"executable_generation_recovery\"\nversion = \"0.1.0\"\n",
    )?;
    let source = root.join("src/main.incn");
    let original = "def main() -> None:\n    print(42)\n";
    fs::write(&source, original)?;
    bake(&root, &home)?;
    let native = root.join("target/incan/executable_generation_recovery/oven/debug/executable_generation_recovery");
    let receipt = root.join(".incan/oven/executable-debug-receipt.json");
    let native_before = fs::read(&native)?;
    let receipt_before = fs::read(&receipt)?;
    let first = Command::new(&native).output()?;
    success(&first, "original executable generation");
    assert_eq!(String::from_utf8(first.stdout)?, "42\n");
    fs::write(&source, "def main() -> None:\n    print(43)\n")?;
    bake(&root, &home)?;
    assert_ne!(fs::read(&native)?, native_before);
    assert_ne!(fs::read(&receipt)?, receipt_before);
    let changed = Command::new(&native).output()?;
    success(&changed, "changed executable generation");
    assert_eq!(String::from_utf8(changed.stdout)?, "43\n");
    fs::write(&source, original)?;
    bake_with_reuse_requirement(&root, &home, true)?;
    assert_eq!(fs::read(&native)?, native_before);
    assert_eq!(fs::read(&receipt)?, receipt_before);
    let restored = Command::new(&native).output()?;
    success(&restored, "restored executable generation");
    assert_eq!(String::from_utf8(restored.stdout)?, "42\n");
    let modified = fs::metadata(&receipt)?.modified()?;
    bake_with_reuse_requirement(&root, &home, true)?;
    assert_eq!(fs::metadata(receipt)?.modified()?, modified);
    Ok(())
}

/// Restoring a provider refreshes its consumer's resolved lock before recovering the original executable generation.
#[test]
fn completed_consumer_reuse_refreshes_restored_provider_identity() -> Result<(), Box<dyn std::error::Error>> {
    let project = tempfile::tempdir()?;
    let root = fs::canonicalize(project.path())?;
    let provider = root.join("provider");
    let consumer = root.join("consumer");
    let home = support::oven_fixture_home()?;
    fs::create_dir_all(provider.join("src"))?;
    fs::create_dir_all(consumer.join("src"))?;
    fs::write(
        provider.join("loaf.toml"),
        "[project]\nname = \"recovery_provider\"\nversion = \"0.1.0\"\n",
    )?;
    fs::write(
        consumer.join("loaf.toml"),
        "[project]\nname = \"recovery_consumer\"\nversion = \"0.1.0\"\n[dependencies]\nrecovery_provider = { path = \"../provider\" }\n",
    )?;
    let consumer_source = consumer.join("src/main.incn");
    let consumer_original = "from pub::recovery_provider import value\n\ndef main() -> None:\n    print(value())\n";
    fs::write(&consumer_source, consumer_original)?;
    let source = provider.join("src/lib.incn");
    let original = "pub def value() -> int:\n    return 42\n";
    fs::write(&source, original)?;
    bake(&provider, &home)?;
    bake(&consumer, &home)?;
    let native = consumer.join("target/incan/recovery_consumer/oven/debug/recovery_consumer");
    let receipt = consumer.join(".incan/oven/executable-debug-receipt.json");
    let lock_path = consumer.join("oven.lock");
    let native_before = fs::read(&native)?;
    let receipt_before = fs::read(&receipt)?;
    let lock_before = oven_model::lock::IncanLock::load(&lock_path)?;
    let first = Command::new(&native).output()?;
    success(&first, "original provider consumer execution");
    assert_eq!(first.stdout, b"42\n");
    fs::write(&source, "pub def value() -> int:\n    return 43\n")?;
    bake(&provider, &home)?;
    bake(&consumer, &home)?;
    assert_ne!(fs::read(&native)?, native_before);
    let changed_lock = oven_model::lock::IncanLock::load(&lock_path)?;
    assert_ne!(changed_lock.semantic.providers, lock_before.semantic.providers);
    let changed = Command::new(&native).output()?;
    success(&changed, "changed provider consumer execution");
    assert_eq!(changed.stdout, b"43\n");

    fs::write(&source, original)?;
    bake_with_reuse_requirement(&provider, &home, true)?;
    let stale_lock = fs::read(&lock_path)?;
    let changed_native = fs::read(&native)?;
    let changed_receipt = fs::read(&receipt)?;
    fs::write(
        &consumer_source,
        "from pub::recovery_provider import value\n\ndef main() -> None:\n    print(value() + 1)\n",
    )?;
    refuse_completed_bake(&consumer, &home)?;
    assert_eq!(
        fs::read(&lock_path)?,
        stale_lock,
        "a late reuse miss must roll back the resolved lock"
    );
    assert_eq!(fs::read(&native)?, changed_native);
    assert_eq!(fs::read(&receipt)?, changed_receipt);
    fs::write(&consumer_source, consumer_original)?;
    fs::write(&source, "pub def value() -> int:\n    return 99\n")?;
    refuse_completed_bake(&consumer, &home)?;
    assert_eq!(
        fs::read(&lock_path)?,
        stale_lock,
        "unbaked provider sources cannot authorize a lock refresh"
    );
    assert_eq!(fs::read(&native)?, changed_native);
    fs::write(&source, original)?;
    bake_with_reuse_requirement(&consumer, &home, true)?;
    assert_eq!(fs::read(&native)?, native_before);
    assert_eq!(fs::read(&receipt)?, receipt_before);
    assert_eq!(
        oven_model::lock::IncanLock::load(&lock_path)?.semantic.providers,
        lock_before.semantic.providers
    );
    let restored = Command::new(&native).output()?;
    success(&restored, "restored provider consumer execution");
    assert_eq!(restored.stdout, b"42\n");
    let receipt_modified = fs::metadata(&receipt)?.modified()?;
    let lock_modified = fs::metadata(&lock_path)?.modified()?;
    bake_with_reuse_requirement(&consumer, &home, true)?;
    assert_eq!(fs::metadata(&receipt)?.modified()?, receipt_modified);
    assert_eq!(fs::metadata(&lock_path)?.modified()?, lock_modified);
    let restored_lock = fs::read(&lock_path)?;
    for mutation in [
        "cargo-features",
        "coordinate",
        "features",
        "participation",
        "cargo-payload",
    ] {
        let mut lock = oven_model::lock::IncanLock::load(&lock_path)?;
        let provider = lock
            .semantic
            .providers
            .iter_mut()
            .find(|provider| provider.identity.starts_with("recovery_provider@"))
            .ok_or("consumer lock lost its custom provider")?;
        match mutation {
            "cargo-features" => lock.cargo_features.cargo_features.push("changed-selection".to_string()),
            "coordinate" => provider.identity = provider.identity.replace("@0.1.0#", "@0.2.0#"),
            "features" => provider.identity = provider.identity.replace("[]", "[changed-selection]"),
            "participation" => provider.participation = "disabled".to_string(),
            "cargo-payload" => lock.cargo_lock_payload.push_str("\n# changed resolution authority\n"),
            _ => return Err("unknown lock mutation".into()),
        }
        lock.write(&lock_path)?;
        let changed = fs::read(&lock_path)?;
        refuse_completed_bake(&consumer, &home)?;
        assert_eq!(fs::read(&lock_path)?, changed, "refusal must retain {mutation}");
        assert_eq!(fs::read(&native)?, native_before);
        assert_eq!(fs::read(&receipt)?, receipt_before);
        fs::write(&lock_path, &restored_lock)?;
    }
    bake_with_reuse_requirement(&consumer, &home, true)?;
    Ok(())
}

/// Refuse a completed-output miss under the explicit guard without compiling a replacement generation.
fn refuse_completed_bake(project: &Path, home: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let mut command = support::repo_command();
    support::configure_explicit_oven_bake_command(&mut command)?;
    let output = command
        .env("INCAN_TEST_REQUIRE_COMPLETED_BAKE_REUSE", "1")
        .env("RUSTC", pinned_driver_rustc()?)
        .env("INCAN_HOME", home)
        .args(["oven", "bake", "--project"])
        .arg(project)
        .output()?;
    assert!(
        !output.status.success(),
        "changed authority must refuse completed reuse"
    );
    if std::env::var_os("INCAN_OVEN_TRACE_REUSE").is_some() {
        eprintln!("completed reuse refusal:\n{}", String::from_utf8_lossy(&output.stderr));
    }
    Ok(())
}

/// Derived SDK fingerprints cannot veto exact executable reuse; authored lock changes still refuse it.
#[test]
fn completed_project_reuse_binds_canonical_lock_instead_of_derived_fingerprint()
-> Result<(), Box<dyn std::error::Error>> {
    let project = tempfile::tempdir()?;
    let root = fs::canonicalize(project.path())?;
    let home = support::oven_fixture_home()?;
    fs::create_dir_all(root.join("src"))?;
    fs::write(
        root.join("loaf.toml"),
        "[project]\nname = \"canonical_lock_recovery\"\nversion = \"0.1.0\"\n",
    )?;
    fs::write(root.join("src/main.incn"), "def main() -> None:\n    print(42)\n")?;
    bake(&root, &home)?;
    let receipt = root.join(".incan/oven/executable-debug-receipt.json");
    let original_receipt = fs::read(&receipt)?;
    let lock_path = root.join("oven.lock");
    let original_lock = fs::read(&lock_path)?;
    let original_source = incan_driver::build::source_authority::digest_baked_project_source_authority(&root)?;
    let mut lock = oven_model::lock::IncanLock::load(&lock_path)?;
    lock.deps_fingerprint = "sha256:derived-cohort-refresh".to_string();
    lock.write(&lock_path)?;
    assert_eq!(
        original_source,
        incan_driver::build::source_authority::digest_baked_project_source_authority(&root)?
    );
    bake_with_reuse_requirement(&root, &home, true)?;
    assert_eq!(fs::read(&receipt)?, original_receipt);

    lock.cargo_features.cargo_features.push("changed-selection".to_string());
    lock.write(&lock_path)?;
    assert_ne!(
        original_source,
        incan_driver::build::source_authority::digest_baked_project_source_authority(&root)?
    );
    let mut command = support::repo_command();
    support::configure_explicit_oven_bake_command(&mut command)?;
    let output = command
        .env("INCAN_TEST_REQUIRE_COMPLETED_BAKE_REUSE", "1")
        .env("RUSTC", pinned_driver_rustc()?)
        .env("INCAN_HOME", &home)
        .args(["oven", "bake", "--project"])
        .arg(&root)
        .output()?;
    assert!(
        !output.status.success(),
        "changed canonical lock must refuse completed reuse"
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("completed project reuse missed before frontend preparation")
    );
    assert_eq!(fs::read(&receipt)?, original_receipt);
    fs::write(lock_path, original_lock)?;
    bake_with_reuse_requirement(&root, &home, true)?;
    assert_eq!(fs::read(&receipt)?, original_receipt);
    Ok(())
}

/// A warm diagnostic permits checked lock resolution but refuses fresh compilation after a completed-output miss.
fn bake_with_reuse_requirement(
    project: &Path,
    home: &Path,
    require_completed: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut command = support::repo_command();
    support::configure_explicit_oven_bake_command(&mut command)?;
    if require_completed {
        command.env("INCAN_TEST_REQUIRE_COMPLETED_BAKE_REUSE", "1");
    }
    let rustc = pinned_driver_rustc()?;
    let timing = support::command_timing_started();
    let output = command
        .env("RUSTC", rustc)
        .args(["oven", "bake", "--project"])
        .arg(project)
        .env("INCAN_HOME", home)
        .env("RUSTC_BOOTSTRAP", "ambient-unit")
        .output()?;
    support::report_command_timing(
        &format!(
            "fixture bake {}",
            project.file_name().ok_or("project has no name")?.to_string_lossy()
        ),
        timing,
    );
    if std::env::var_os("INCAN_OVEN_TRACE_REUSE").is_some() {
        eprintln!(
            "fixture {}:\n{}\n{}",
            project.display(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    success(&output, &format!("Oven bake {}", project.display()));
    Ok(())
}

/// Start the pinned driver without the caller's dynamic-loader search paths.
///
/// The driver must load the `rustc_driver` its runpath names, and its startup check refuses any other copy. On Linux an
/// inherited `LD_LIBRARY_PATH` is searched before that runpath, and the compiler-suite runner exports one naming the
/// toolchain's `lib` directory to every libtest child, so an inheriting launch loads rustup's second, byte-identical
/// copy of the library from the rustc component and the check refuses it (#1755, #1785).
fn driver_command(driver: &Path) -> Command {
    let mut command = Command::new(driver);
    for name in ["LD_LIBRARY_PATH", "DYLD_LIBRARY_PATH", "DYLD_FALLBACK_LIBRARY_PATH"] {
        command.env_remove(name);
    }
    command
}

/// Assert native success with both output streams available on failure.
fn success(output: &Output, operation: &str) {
    assert!(
        output.status.success(),
        "{operation}\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Retain a debug runtime's receipt-selected native plan, including its immutable output leases.
///
/// Native SDK publication supplies a stored direct-Rustc plan. Older installed toolchains supply a compiler-owned
/// Loaf instead; both paths validate the same receipt and keep their selected closure alive through execution.
fn runtime_plan(runtime: &Path) -> Result<oven_rustc::plan::OvenDirectRustcPlanSelection, Box<dyn std::error::Error>> {
    let path = oven_store::default_receipt_path(runtime).with_file_name("library-debug-receipt.json");
    let receipt: oven_store::OvenReceipt = serde_json::from_slice(&fs::read(path)?)?;
    let store = oven_store::store::OvenStore::new(
        oven_store::store::store_root_for_home(&support::oven_fixture_home()?),
        oven_store::store::OvenStoreLimits::new(
            oven_store::DEFAULT_OVEN_MAX_PHYSICAL_BYTES,
            oven_store::DEFAULT_OVEN_MAX_DOMAIN_PHYSICAL_BYTES,
            oven_store::DEFAULT_OVEN_MAX_DOMAIN_LOGICAL_BYTES,
        ),
    );
    if incan_provider::inventory::discover_or_reuse_published_sdk_inventory()?
        .is_some_and(|inventory| inventory.root.join(".sealed-native-units.json").is_file())
    {
        let manifest = oven_model::manifest::ProjectManifest::load(&runtime.join("loaf.toml"))?;
        let dependencies = manifest.rust_dependencies().values().cloned().collect::<Vec<_>>();
        return Ok(
            incan_driver::build::native_sdk::select_prepared_native_sdk_plan(&store, &receipt, &dependencies)?.1,
        );
    }
    if let Some(plan) = oven_rustc::plan::selection::select_receipt_direct_rustc_execution_plan(&store, &receipt)? {
        return Ok(oven_rustc::plan::OvenDirectRustcPlanSelection::Stored(Box::new(plan)));
    }
    let loaf = oven_rustc::loaf::resolve_compiler_owned_loaf_for_registry_dependencies(&receipt, &[])?
        .ok_or("runtime fixture has no receipt-selected native closure")?;
    Ok(oven_rustc::plan::OvenDirectRustcPlanSelection::ToolchainLoaf(Box::new(
        loaf,
    )))
}

/// Startup refusals precede output creation, including a substituted loaded driver library.
fn check_startup_refusals(
    binary: &Path,
    source: &Path,
    root: &Path,
    sysroot: &Path,
    runtime_rlib: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    // ---- Ambient permission and detached sysroot ----
    let ambient = driver_command(binary)
        .env("RUSTC_BOOTSTRAP", "1")
        .arg(source)
        .arg("scalar")
        .arg(root.join("ambient-output"))
        .arg(sysroot)
        .arg(runtime_rlib)
        .output()?;
    assert!(!ambient.status.success());
    assert!(String::from_utf8_lossy(&ambient.stderr).contains("Bootstrap"));
    assert!(!root.join("ambient-output").exists());
    let mismatch = driver_command(binary)
        .env_remove("RUSTC_BOOTSTRAP")
        .arg(source)
        .arg("scalar")
        .arg(root.join("refused-output"))
        .arg(root)
        .arg(runtime_rlib)
        .output()?;
    assert!(!mismatch.status.success());
    assert!(String::from_utf8_lossy(&mismatch.stderr).contains("Mismatch"));
    assert!(!root.join("refused-output").exists());

    // ---- Loaded-library substitution ----
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        let substitute = root.join("substituted-driver");
        fs::create_dir_all(&substitute)?;
        let directory = sysroot
            .join("lib/rustlib")
            .join(oven_rustc::rustc::rustc_host_target(&pinned_driver_rustc()?)?)
            .join("lib");
        let mut library = None;
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with("librustc_driver-") && (name.ends_with(".dylib") || name.ends_with(".so")) {
                if library.is_some() {
                    return Err("pinned driver library is ambiguous".into());
                }
                library = Some(entry.path());
            }
        }
        let library = library.ok_or("pinned driver library is missing")?;
        fs::copy(
            &library,
            substitute.join(library.file_name().ok_or("driver library has no name")?),
        )?;
        let loader_variable = if cfg!(target_os = "macos") {
            "DYLD_LIBRARY_PATH"
        } else {
            "LD_LIBRARY_PATH"
        };
        let substituted = Command::new(binary)
            .env_remove("RUSTC_BOOTSTRAP")
            .env(loader_variable, &substitute)
            .arg(source)
            .arg("scalar")
            .arg(root.join("substituted-output"))
            .arg(sysroot)
            .arg(runtime_rlib)
            .output()?;
        assert!(!substituted.status.success());
        assert!(
            String::from_utf8_lossy(&substituted.stderr).contains("loaded a different rustc_driver"),
            "{}",
            String::from_utf8_lossy(&substituted.stderr)
        );
        assert!(!root.join("substituted-output").exists());
    }
    Ok(())
}

/// Preserve dependency coordinates across compatible checkout layouts without changing their selected source roots.
fn fixture_dependency_path(owner: &Path, dependency: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let owner = owner.canonicalize()?;
    let dependency = dependency.canonicalize()?;
    let common = owner
        .ancestors()
        .find(|ancestor| dependency.starts_with(ancestor))
        .ok_or("fixture dependency and owner have no common filesystem root")?;
    let mut relative = PathBuf::new();
    for _ in owner.strip_prefix(common)?.components() {
        relative.push("..");
    }
    relative.push(dependency.strip_prefix(common)?);
    Ok(relative
        .to_str()
        .ok_or("fixture dependency path is not UTF-8")?
        .replace('\\', "/"))
}

/// Prepare one source-only driver graph using portable paths to the selected checkout's own compiler sources.
fn prepare_source_driver(root: &Path, repo: &Path) -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
    let driver_source = repo.join("loaves/toolchain/incan-rustc-driver");
    let driver = root.join("driver");
    let library = root.join("library");
    let lowering = root.join("lowering");
    copy_tree(&driver_source.join("src"), &driver.join("src"))?;
    fs::create_dir_all(&library)?;
    fs::create_dir_all(&lowering)?;
    fs::copy(
        repo.join("loaves/compiler/incan_mir_plan/loaf.toml"),
        library.join("loaf.toml"),
    )?;
    copy_tree(&repo.join("loaves/compiler/incan_mir_plan/src"), &library.join("src"))?;
    let core = fixture_dependency_path(&lowering, &repo.join("loaves/kernel/incan_semantics_core"))?;
    let frontend = fixture_dependency_path(&driver, &repo.join("loaves/compiler/incan_frontend"))?;
    let language = fixture_dependency_path(&lowering, &repo.join("loaves/kernel/incan_lang"))?;
    let compiler = fixture_dependency_path(&driver, &repo.join("loaves/compiler/incan_driver"))?;
    let oven_cli = fixture_dependency_path(&driver, &repo.join("loaves/toolchain/oven-cli"))?;
    let oven_rustc = fixture_dependency_path(&driver, &repo.join("loaves/oven/oven_rustc"))?;
    let manifest = fs::read_to_string(repo.join("loaves/compiler/incan_mir_lowering/loaf.toml"))?
        .replace("../incan_mir_plan", "../library")
        .replace("../../kernel/incan_semantics_core", &core)
        .replace("../../kernel/incan_lang", &language);
    fs::write(lowering.join("loaf.toml"), manifest)?;
    copy_tree(
        &repo.join("loaves/compiler/incan_mir_lowering/src"),
        &lowering.join("src"),
    )?;
    let manifest = fs::read_to_string(driver_source.join("loaf.toml"))?
        .replace("../../compiler/incan_mir_plan", "../library")
        .replace("../../compiler/incan_mir_lowering", "../lowering")
        .replace("../../kernel/incan_semantics_core", &core)
        .replace("../../compiler/incan_frontend", &frontend)
        .replace("../../compiler/incan_driver", &compiler)
        .replace("../../oven/oven_rustc", &oven_rustc)
        .replace("../oven-cli", &oven_cli);
    fs::write(driver.join("loaf.toml"), manifest)?;
    Ok(driver)
}

/// Fresh fixture roots share the plan library's immutable debug output and retain its native SDK closure.
#[test]
fn oven_plan_fixture_shares_debug_native_output() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let source = support::repo_root().join("loaves/compiler/incan_mir_plan");
    let home = support::oven_fixture_home()?;
    let mut outputs = Vec::new();
    for name in ["first", "second"] {
        let project = root.path().join(name);
        copy_tree(&source.join("src"), &project.join("src"))?;
        fs::copy(source.join("loaf.toml"), project.join("loaf.toml"))?;
        bake(&project, &home)?;
        let plan = runtime_plan(&project)?;
        assert!(matches!(
            plan,
            oven_rustc::plan::OvenDirectRustcPlanSelection::Stored(_)
        ));
        assert!(!project.join("target/lib/oven/release").exists());
        assert!(!oven_store::default_receipt_path(&project).is_file());
        outputs.push(oven_store::digest_bytes(&fs::read(
            project.join("target/lib/oven/debug/libincan_mir_plan.rlib"),
        )?));
    }
    assert_eq!(
        outputs[0], outputs[1],
        "identical authored sources must select the same debug output bytes"
    );
    Ok(())
}

/// Actual native children retain loaded-library identity checks while avoiding unchanged library byte reads.
#[cfg(unix)]
#[test]
fn native_driver_reuses_loaded_library_digest_without_skipping_identity_checks()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let root = fixture.scratch("library-observation")?;
    let source = root.join("main.incn");
    fs::write(&source, "def main() -> None:\n    println(42)\n")?;
    let closure = corpus::runtime_closure(&fixture.formatting)?;
    let cache = root.join("observed-library-home");
    let mut library = None;
    for (index, name) in ["cold", "warm"].into_iter().enumerate() {
        let native = root.join(name);
        let compile = corpus::source_command(
            &fixture.driver_binary("debug"),
            &source,
            &native,
            &fixture.sysroot,
            &closure,
        )
        .env("INCAN_HOME", &cache)
        .env("INCAN_OVEN_TRACE_FILE_DIGESTS", "1")
        .output()?;
        success(&compile, "observed native driver compilation");
        let observations = String::from_utf8_lossy(&compile.stderr)
            .lines()
            .filter_map(|line| line.strip_prefix("Oven file digest: "))
            .map(serde_json::from_str::<serde_json::Value>)
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .filter(|record| {
                record["path"].as_str().is_some_and(|path| {
                    Path::new(path)
                        .file_name()
                        .is_some_and(|name| name.to_string_lossy().starts_with("librustc_driver-"))
                })
            })
            .collect::<Vec<_>>();
        assert_eq!(observations.len(), 1, "the actual loaded library must be checked once");
        let path = PathBuf::from(
            observations[0]["path"]
                .as_str()
                .ok_or("missing observed library path")?,
        );
        let bytes = observations[0]["input_bytes_read"]
            .as_u64()
            .ok_or("missing observed library byte count")?;
        if index == 0 {
            let length = fs::metadata(&path)?.len();
            assert!(length > 0);
            assert_eq!(bytes, length);
            library = Some(path);
            eprintln!("native loaded library digest: cold={length}, unchanged=0");
        } else {
            assert_eq!(library.as_ref(), Some(&path));
            assert_eq!(bytes, 0);
        }
        let executed = Command::new(&native).output()?;
        success(&executed, "observed native executable");
        assert_eq!(executed.stdout, b"42\n");
    }
    Ok(())
}

/// Publish the compiler Loafs, then reuse one driver bake for native output, spans, and typed refusals.
#[test]
fn oven_driver_compiles_scalar_plan_and_refuses_invalid_inputs() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let root = fixture.scratch("scalar")?;
    let root = root.as_path();
    let driver_source = support::repo_root().join("loaves/toolchain/incan-rustc-driver");
    let runtime = root.join("runtime");
    copy_tree(&driver_source.join("tests/fixtures/native_output"), &runtime)?;
    let source = root.join("scalar.incn");
    fs::copy(driver_source.join("tests/fixtures/scalar.incn"), &source)?;
    let home = &fixture.home;
    let formatting = &fixture.formatting;
    // The scalar driver consumes this producer's own receipt; the Rust caller owns a separate dependency projection.
    bake(&runtime, home)?;
    let runtime_caller = root.join("runtime-caller");
    fs::create_dir_all(runtime_caller.join("src"))?;
    fs::write(
        runtime_caller.join("loaf.toml"),
        "[project]\nname = \"runtime-caller\"\n[dependencies]\nnative_output = { loaf = \"native_output\", path = \"../runtime\" }\n[[rust.bin]]\nname = \"runtime-caller\"\npath = \"src/main.rs\"\n",
    )?;
    fs::write(
        runtime_caller.join("src/main.rs"),
        "use native_output::caller::incan::print_int;\nfn main() { print_int(42); }\n",
    )?;
    bake(&runtime_caller, home)?;
    let sysroot = &fixture.sysroot;
    {
        let profile = "debug";
        let closure = runtime_plan(&runtime)?;
        let directories = &closure.artifact_plan().dependency_search_paths;
        let binary = fixture.driver_binary(profile);
        let runtime_rlib = runtime
            .join("target/lib/oven")
            .join(profile)
            .join("libnative_output.rlib");
        for mode in ["normal", "overflow", "dangling"] {
            let output_binary = root.join(format!("scalar-{profile}-{mode}"));
            let compile = driver_command(&binary)
                .env_remove("RUSTC_BOOTSTRAP")
                .arg(&source)
                .arg("scalar")
                .arg(&output_binary)
                .arg(sysroot)
                .arg(&runtime_rlib)
                .arg(mode)
                .args(directories)
                .output()?;
            if mode == "dangling" {
                assert!(!compile.status.success());
                assert!(
                    String::from_utf8_lossy(&compile.stderr).contains("UnknownBlock"),
                    "{}",
                    String::from_utf8_lossy(&compile.stderr)
                );
                assert!(!output_binary.exists());
                continue;
            }
            success(&compile, "native plan compilation");
            let run = Command::new(&output_binary).output()?;
            if mode == "normal" {
                success(&run, "scalar binary");
                assert_eq!(String::from_utf8(run.stdout)?, "42\n");
            } else {
                assert!(!run.status.success());
                assert!(
                    String::from_utf8_lossy(&run.stderr).contains("scalar.incn:3:12"),
                    "{}",
                    String::from_utf8_lossy(&run.stderr)
                );
            }
        }
        check_startup_refusals(&binary, &source, root, sysroot, &runtime_rlib)?;
        check_source_pipeline(&binary, root, &fixture.sysroot, formatting, profile)?;
    }
    let debug = fixture.driver_binary("debug");
    corpus::check_strings(&debug, root, &fixture.sysroot, formatting)?;
    for name in ["fib", "collatz", "mandelbrot"] {
        corpus::check_benchmark(&debug, root, &fixture.sysroot, formatting, name)?;
    }
    Ok(())
}

/// Choose where the shared fixture graph lives; each bake still validates the current copied sources.
///
/// In order: a replayed graph, a retained evidence directory, the suite's kept workspace (so Oven reuses the previous
/// run's bakes), and otherwise a fresh temporary directory.
fn fixture_root() -> Result<(PathBuf, Option<tempfile::TempDir>), Box<dyn std::error::Error>> {
    if let Some(path) = std::env::var_os("INCAN_NATIVE_DRIVER_REPLAY") {
        let path = PathBuf::from(path);
        for project in ["library", "lowering", "driver"] {
            if !path.join(project).join("loaf.toml").is_file() {
                return Err(format!("native driver replay requires a retained {project} fixture").into());
            }
        }
        eprintln!("replaying native driver fixture: {}", path.display());
        return Ok((path, None));
    }
    if let Some(parent) = std::env::var_os("INCAN_NATIVE_DRIVER_EVIDENCE") {
        fs::create_dir_all(&parent)?;
        let path = tempfile::tempdir_in(parent)?.keep();
        eprintln!("retained native driver fixture: {}", path.display());
        Ok((path, None))
    } else if let Some(workspace) = support::explicit_bake_workspace() {
        fs::create_dir_all(&workspace)?;
        Ok((workspace, None))
    } else {
        let directory = tempfile::tempdir()?;
        Ok((directory.path().to_path_buf(), Some(directory)))
    }
}

/// The baked fixture graph every test in this root shares: the plan, lowering and driver Loaves and the formatting
/// runtime, baked once per test process into one Oven home.
struct DriverFixture {
    /// Root of the graph: the copied Loaves, their home, and each test's scratch directory under `cases/`.
    root: PathBuf,
    /// The driver Loaf project; its binaries live under `target/rust/<profile>/`.
    driver: PathBuf,
    /// The Oven home every fixture bake shares.
    home: PathBuf,
    /// The authored Incan runtime Loaf, baked through a caller.
    formatting: PathBuf,
    /// The pinned driver compiler's sysroot.
    sysroot: PathBuf,
    /// Keeps a fresh temporary root alive for the whole process when no workspace was granted.
    _temporary: Option<tempfile::TempDir>,
}

impl DriverFixture {
    /// The driver binary for one profile.
    fn driver_binary(&self, profile: &str) -> PathBuf {
        self.driver.join("target/rust").join(profile).join("incan-rustc-driver")
    }

    /// Return an empty scratch directory owned by one test, so concurrent tests and earlier runs never collide.
    fn scratch(&self, name: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
        let path = self.root.join("cases").join(name);
        if path.exists() {
            fs::remove_dir_all(&path)?;
        }
        fs::create_dir_all(&path)?;
        Ok(path)
    }
}

/// The process-wide fixture; a bake failure is kept as text so every test reports it.
static DRIVER_FIXTURE: OnceLock<Result<DriverFixture, String>> = OnceLock::new();

/// Return the shared fixture, baking it on first use. Tests running in parallel wait for that single bake.
fn driver_fixture() -> Result<&'static DriverFixture, Box<dyn std::error::Error>> {
    DRIVER_FIXTURE
        .get_or_init(|| bake_driver_fixture().map_err(|error| error.to_string()))
        .as_ref()
        .map_err(|error| error.clone().into())
}

/// Copy the current Loaf sources into the fixture root and bake the graph the driver tests compile through.
fn bake_driver_fixture() -> Result<DriverFixture, Box<dyn std::error::Error>> {
    let (root, temporary) = fixture_root()?;
    let repo = support::repo_root();
    let driver = prepare_source_driver(&root, &repo)?;
    let home = support::oven_fixture_home()?;
    bake(&root.join("library"), &home)?;
    bake(&root.join("lowering"), &home)?;
    bake(&driver, &home)?;
    let formatting = prepare_formatting_runtime(&root, &repo, &home)?;
    let sysroot = oven_rustc::rustc::rustc_sysroot(&pinned_driver_rustc()?)?;
    Ok(DriverFixture {
        root,
        driver,
        home,
        formatting,
        sysroot,
        _temporary: temporary,
    })
}

/// A library naming one public item of each stdlib SDK component the runtime does not already link.
///
/// Legacy links each stdlib component a program uses. Direct-route project tests and the census link every component
/// crate a bake of this library selects from the fixture Store, so a program reaches the same compiled stdlib items
/// legacy calls.
const SDK_CLOSURE_MANIFEST: &str = "[project]\nname = \"sdk_closure\"\nversion = \"0.1.0\"\n";
const SDK_CLOSURE_SOURCE: &str =
    "\"\"\"Name one public item of each stdlib component, so its bake links them all.\"\"\"

from std.uuid import UuidError
from std.io import IoError
from std.encoding.bech32 import Decoded
from std.compression import CompressionError
from std.interop import BindingDeclaration
from std.logging import Logger
from std.testing import assert_true


pub def data_item(value: UuidError) -> UuidError:
    return value


pub def system_item(value: IoError) -> IoError:
    return value


pub def codecs_item(value: Decoded) -> Decoded:
    return value


pub def compression_item(value: CompressionError) -> CompressionError:
    return value


pub def interop_item(value: BindingDeclaration) -> BindingDeclaration:
    return value


pub def observability_item(value: Logger) -> Logger:
    return value


pub def testing_item(value: bool) -> None:
    assert_true(value)
";

/// The process-wide SDK closure library; tests running in parallel wait for its single bake instead of baking the
/// same project at once.
static SDK_CLOSURE: OnceLock<Result<PathBuf, String>> = OnceLock::new();

/// Return the SDK closure library, baking it on first use.
fn sdk_closure(fixture: &DriverFixture) -> Result<PathBuf, Box<dyn std::error::Error>> {
    SDK_CLOSURE
        .get_or_init(|| bake_sdk_closure(fixture).map_err(|error| error.to_string()))
        .clone()
        .map_err(Into::into)
}

/// Bake the SDK closure library beside the runtime, reusing an unchanged earlier bake.
fn bake_sdk_closure(fixture: &DriverFixture) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let project = fixture.root.join("sdk-closure");
    fs::create_dir_all(project.join("src"))?;
    fs::write(project.join("loaf.toml"), SDK_CLOSURE_MANIFEST)?;
    fs::write(project.join("src/lib.incn"), SDK_CLOSURE_SOURCE)?;
    bake(&project, &fixture.home)?;
    Ok(project)
}

/// Transport runtime preparation to Incan until the native test kernel has an Incan fixture hook (#1698).
///
/// The returned root owns the library and receipt; preparation no longer depends on a generated Rust caller.
fn prepare_formatting_runtime(root: &Path, repo: &Path, home: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let mut command = support::repo_command();
    support::configure_explicit_oven_bake_command(&mut command)?;
    let compiler = PathBuf::from(command.get_program());
    let timing = support::command_timing_started();
    let output = command
        .env("RUSTC", pinned_driver_rustc()?)
        .env("INCAN_NO_BANNER", "1")
        .arg("run")
        .arg(repo.join("workspaces/compiler-fixture-preparation/src/main.incn"))
        .arg("--")
        .arg(&compiler)
        .arg(root)
        .arg(repo)
        .arg(home)
        .output()?;
    support::report_command_timing("fixture prepare formatting-runtime", timing);
    fs::write(root.join("formatting-helper.stdout"), &output.stdout)?;
    fs::write(root.join("formatting-helper.stderr"), &output.stderr)?;
    if !output.status.success() {
        return Err(format!(
            "Incan runtime preparation failed:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    let response: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    if response.get("schema").and_then(serde_json::Value::as_str) != Some("incan.compiler-fixture-preparation/1") {
        return Err("Incan runtime preparation returned an unsupported response".into());
    }
    let runtime = PathBuf::from(
        response
            .get("runtime_root")
            .and_then(serde_json::Value::as_str)
            .ok_or("Incan runtime preparation returned no root")?,
    );
    if runtime.canonicalize()? != root.join("formatting-runtime").canonicalize()? {
        return Err("Incan runtime preparation returned a different fixture root".into());
    }
    Ok(runtime)
}

/// Exercise real checking and Incan lowering, including branches, calls, ranges, conversions, and formatting.
fn check_source_pipeline(
    driver: &Path,
    root: &Path,
    sysroot: &Path,
    runtime: &Path,
    profile: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let source = root.join("checked-source.incn");
    fs::write(
        &source,
        "def twice(mut n: int) -> int:\n  n += n\n  return n\n\ndef main() -> None:\n  mut total = 0\n  for i in range(5, -1, -2):\n    if i == 3:\n      continue\n    total += twice(i)\n  println(f\"total={total} float={float(total)} div={-7 // 3} mod={-7 % 3}\")\n",
    )?;
    let closure = runtime_plan(runtime)?;
    let binary = root.join(format!("checked-source-{profile}"));
    let library = runtime
        .join("target/lib/oven")
        .join(profile)
        .join("libincan_native_runtime.rlib");
    let mut command = driver_command(driver);
    command
        .env_remove("RUSTC_BOOTSTRAP")
        .args(["--source"])
        .arg(&source)
        .arg("checked_source")
        .arg(&binary)
        .arg(sysroot)
        .arg("--extern")
        .arg(format!("incan_native_runtime={}", library.display()));
    for (name, artifact) in &closure.artifact_plan().externs {
        command.arg("--extern").arg(format!("{name}={}", artifact.display()));
    }
    for directory in &closure.artifact_plan().dependency_search_paths {
        command.arg("--search").arg(directory);
    }
    success(&command.output()?, "checked source native compilation");
    let run = Command::new(binary).output()?;
    success(&run, "checked source execution");
    assert_eq!(String::from_utf8(run.stdout)?, "total=12 float=12.0 div=-3 mod=2\n");
    Ok(())
}

#[path = "native_driver_project_tests/census.rs"]
mod census;

#[path = "native_driver_project_tests/tail.rs"]
mod tail;

/// Compile one source through both routes and compare successful execution bytes against a fixed oracle.
fn assert_native_legacy_bytes(name: &str, program: &str, stdout: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let root = fixture.scratch(name)?;
    let source = root.join(format!("{name}.incn"));
    fs::write(&source, program)?;
    let closure = corpus::runtime_closure(&fixture.formatting)?;
    let native = root.join("native");
    success(
        &corpus::source_command(
            &fixture.driver_binary("debug"),
            &source,
            &native,
            &fixture.sysroot,
            &closure,
        )
        .output()?,
        "native parity compilation",
    );
    let legacy = root.join("legacy");
    success(
        &support::repo_command()
            .current_dir(&root)
            .arg("build")
            .arg(&source)
            .arg(&legacy)
            .output()?,
        "legacy parity compilation",
    );
    let expected = Command::new(legacy.join("oven/release").join(name)).output()?;
    let actual = Command::new(native).output()?;
    success(&expected, "legacy parity execution");
    success(&actual, "native parity execution");
    assert_eq!(actual.stdout, stdout);
    assert_eq!(actual.stdout, expected.stdout);
    assert_eq!(actual.stderr, expected.stderr);
    Ok(())
}

/// An explicitly typed function pointer borrows its list input without consuming the caller's list.
#[test]
fn named_list_function_pointer_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    assert_native_legacy_bytes(
        "named_list_function_pointer",
        r#"def total(items: list[int]) -> int:
    mut sum = 0
    for item in items:
        sum += item
    return sum

def main() -> None:
    values = [1, 2, 3]
    f: (list[int]) -> int = total
    println(f(values))
    println(len(values))
"#,
        b"6\n3\n",
    )
}

/// A callable parameter receives a shared view and preserves its caller's list after invocation.
#[test]
fn list_callable_parameter_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    assert_native_legacy_bytes(
        "list_callable_parameter",
        r#"def total(items: list[int]) -> int:
    mut sum = 0
    for item in items:
        sum += item
    return sum

def apply(f: (list[int]) -> int, items: list[int]) -> int:
    return f(items)

def main() -> None:
    values = [1, 2, 3]
    println(apply(total, values))
    println(len(values))
"#,
        b"6\n3\n",
    )
}

/// Local and returned closures borrow list inputs on each call while retaining their scalar captures.
#[test]
fn list_closure_inputs_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    assert_native_legacy_bytes(
        "list_closure_inputs",
        r#"def apply(f: (list[int]) -> int, items: list[int]) -> int:
    return f(items)

def counter(offset: int) -> (list[int]) -> int:
    return (xs) => len(xs) + offset

def main() -> None:
    values = [1, 2, 3]
    size: (list[int]) -> int = (xs) => len(xs)
    println(size(values))
    println(apply(size, [4, 5]))
    count = counter(4)
    println(count([6]))
    println(count(values))
    println(len(values))
"#,
        b"3\n2\n5\n7\n3\n",
    )
}

/// Nested empty list literals retain checker-proven element types and match legacy bytes.
#[test]
fn direct_route_nested_empty_lists_match_legacy() -> Result<(), Box<dyn std::error::Error>> {
    assert_native_legacy_bytes(
        "nested_empty_lists",
        "def main() -> None:\n    rows = [[], [1]]\n    println(len(rows))\n    println(rows[1][0])\n    deep = [[[]], [[2]]]\n    println(len(deep))\n    println(deep[1][0][0])\n",
        b"2\n1\n2\n2\n",
    )
}

/// Reassigning collection literals injects the checked Option or union payload and preserves legacy output.
#[test]
fn direct_route_optional_collection_assignment_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    assert_native_legacy_bytes(
        "optional_collection_assignment",
        include_str!(
            "../../incan_test_support/fixtures/behavior/cli_values_and_calls/empty_list_assigned_to_optional_binding_is_wrapped.incn"
        ),
        b"1\nsome 0\nsome 1\nlist 0\n0\n",
    )
}

/// Tuple annotations and tuple literals share one Result carrier through construction, propagation and matching.
#[test]
fn direct_route_tuple_result_identity_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    assert_native_legacy_bytes(
        "tuple_result_identity",
        include_str!(
            "../../incan_test_support/fixtures/behavior/execution_calls_types_and_modules/result_routing_through_try_and_match.incn"
        ),
        b"2\n0\n42\n",
    )
}

/// Contextual None payloads construct nested intrinsic carriers with their proven types.
#[test]
fn direct_route_contextual_none_payloads_match_legacy() -> Result<(), Box<dyn std::error::Error>> {
    assert_native_legacy_bytes(
        "contextual_none_payloads",
        "def number(value: Result[Option[int], str]) -> int:\n    match value:\n        Ok(optional) => return optional.unwrap_or(0)\n        Err(_) => return -1\n\ndef main() -> None:\n    println(number(Ok(None)))\n    println(number(Ok(Some(7))))\n    println(number(Err(\"failure\")))\n",
        b"0\n7\n-1\n",
    )
}

/// Inferred bindings and in-place matches construct Results with the checker-settled open sides.
#[test]
fn direct_route_settled_result_sides_match_legacy() -> Result<(), Box<dyn std::error::Error>> {
    assert_native_legacy_bytes(
        "settled_result_sides",
        "def value() -> Result[int, str]:\n    inferred = Ok(9)\n    return inferred\n\ndef main() -> None:\n    inferred = Ok(7)\n    match inferred:\n        Ok(number) => println(number)\n        Err(_) => pass\n    match Ok(8):\n        Ok(number) => println(number)\n        Err(_) => pass\n    println(value().unwrap_or(0))\n",
        b"7\n8\n9\n",
    )
}

/// Compiler-generated collection writes in filtered comprehensions preserve canonical dispatch and legacy bytes.
#[test]
fn direct_route_comprehension_writes_match_legacy() -> Result<(), Box<dyn std::error::Error>> {
    assert_native_legacy_bytes(
        "comprehension_writes",
        "def main() -> None:\n    xs = [1, 2, 3, 4]\n    values = [x * x for x in xs if x > 2]\n    indexed = {x: x * x for x in xs if x % 2 == 0}\n    println(values[0])\n    println(values[1])\n    println(indexed[2])\n    println(indexed[4])\n    println(len(xs))\n",
        b"9\n16\n4\n16\n4\n",
    )
}

/// Short-circuit branches release borrowed text literals without changing incoming owners or legacy behavior.
#[test]
fn direct_route_borrowed_text_branches_match_legacy() -> Result<(), Box<dyn std::error::Error>> {
    assert_native_legacy_bytes(
        "borrowed_text_branches",
        "def compare(a: str, b: str) -> bool:\n    return not (a == \"x\" and b == \"y\")\n\ndef main() -> None:\n    println(compare(\"x\", \"y\"))\n    println(compare(\"p\", \"y\"))\n    println(compare(\"x\", \"q\"))\n    text = \"retained\"\n    println(bool(text) and bool(\"value\") and not bool(\"\"))\n    println(false and bool(\"skipped\"))\n    println(text)\n",
        b"false\ntrue\ntrue\ntrue\nfalse\nretained\n",
    )
}

/// Imported aliases and module-qualified scalar calls preserve canonical binding and legacy output; async vocabulary
/// reaches lowering.
#[test]
fn direct_route_stdlib_imports_match_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let root = fixture.scratch("imports")?;
    let runtime = &fixture.formatting;
    let sysroot = &fixture.sysroot;
    let source = root.join("imports.incn");
    fs::write(
        &source,
        "from std.math import gcd as common, sqrt as root\nimport std.math\nfrom std.derives.comparison import Eq\n\ndef gcd(a: int, b: int) -> int:\n  return a + b\n\ndef main() -> None:\n  println(common(b=18, a=48))\n  println(math.lcm(4, 6))\n  println(gcd(4, 6))\n  println(root(16.0))\n  println(math.sqrt(25.0))\n",
    )?;
    let closure = corpus::runtime_closure(runtime)?;
    let native = root.join("imports-native");
    let binary = fixture.driver_binary("debug");
    check_async_frontend_refusal(&binary, &root, sysroot, &closure)?;
    success(
        &corpus::source_command(&binary, &source, &native, sysroot, &closure).output()?,
        "native imported scalar compilation",
    );
    let legacy_root = root.join("imports-legacy");
    success(
        &support::repo_command()
            .current_dir(&root)
            .arg("build")
            .arg(&source)
            .arg(&legacy_root)
            .output()?,
        "legacy imported scalar compilation",
    );
    let legacy = Command::new(legacy_root.join("oven/release/imports")).output()?;
    let actual = Command::new(native).output()?;
    success(&legacy, "legacy imported scalar execution");
    success(&actual, "native imported scalar execution");
    assert_eq!(actual.stdout, b"6\n12\n10\n4.0\n5.0\n");
    assert_eq!(actual.stdout, legacy.stdout);
    Ok(())
}

/// Compare concrete carrier construction, matching, propagation, defaults, and printing against legacy.
#[test]
fn option_result_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let root = fixture.scratch("option-result")?;
    let source = root.join("option_result.incn");
    fs::write(
        &source,
        r#"enum Failure:
    Bad

def optional_text() -> Option[str]:
    return Some("text")

def optional_list() -> Option[List[int]]:
    return Some([2, 3])

def show_result(value: Result[str, str]) -> None:
    match value:
        Ok(text) => println(text)
        Err(error) => println(error)

def unit_result() -> Result[None, str]:
    return Ok(None)

def failure_result() -> Result[int, Failure]:
    return Err(Failure.Bad)

def value(good: bool) -> Result[int, str]:
    if good:
        return Ok(7)
    return Err("failure")

def doubled(good: bool) -> Result[int, str]:
    number = value(good)?
    return Ok(number * 2)

def selected(option: Option[int]) -> int:
    match option:
        Some(number) => return number
        None => return -1

def main() -> None:
    show_result(Ok("parameter"))
    println(selected(Some(5)))
    println(selected(None))
    match optional_text():
        Some(text) => println(text)
        None => println("missing")
    match optional_list():
        Some(values) => println(values[1])
        None => println(0)
    match unit_result():
        Ok(_) => println("unit")
        Err(error) => println(error)
    match failure_result():
        Ok(number) => println(number)
        Err(_) => println("bad")
    option: Option[int] = Some(9)
    missing: Option[int] = None
    println(option.unwrap_or(0))
    println(missing.unwrap_or(4))
    println(option)
    for_good = doubled(true)
    for_bad = doubled(false)
    println(for_good.unwrap_or(0))
    println(for_bad.unwrap_or(3))
    match for_bad:
        Ok(number) => println(number)
        Err(error) => println(error)
"#,
    )?;
    let closure = corpus::runtime_closure(&fixture.formatting)?;
    let native = root.join("native");
    success(
        &corpus::source_command(
            &fixture.driver_binary("debug"),
            &source,
            &native,
            &fixture.sysroot,
            &closure,
        )
        .output()?,
        "Option/Result native compilation",
    );
    let legacy = root.join("legacy");
    success(
        &support::repo_command()
            .current_dir(&root)
            .arg("build")
            .arg(&source)
            .arg(&legacy)
            .output()?,
        "Option/Result legacy compilation",
    );
    let expected = Command::new(legacy.join("oven/release/option_result")).output()?;
    let actual = Command::new(native).output()?;
    assert_eq!(actual.status.code(), expected.status.code());
    assert_eq!(actual.stderr, expected.stderr);
    assert_eq!(actual.stdout, expected.stdout);
    assert_eq!(
        actual.stdout,
        b"parameter\n5\n-1\ntext\n3\nunit\nbad\n9\n4\nSome(9)\n14\n3\nfailure\n"
    );
    Ok(())
}

/// Compare stored function values, callable parameters, and capturing closure expressions with legacy execution.
#[test]
fn direct_route_function_values_and_closures_match_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let root = fixture.scratch("closures")?;
    check_function_item_values(fixture, &root)?;
    let source = root.join("closures.incn");
    fs::write(
        &source,
        concat!(
            "def double(value: int) -> int:\n    \"\"\"Double the argument.\"\"\"\n    return value * 2\n\n",
            "def apply(f: (int) -> int, value: int) -> int:\n    \"\"\"Invoke a function-typed argument.\"\"\"\n    return f(value)\n\n",
            "def apply_callable(f: Callable[int, int], value: int) -> int:\n    \"\"\"Invoke a Callable-typed argument.\"\"\"\n    return f(value)\n\n",
            "def make_adder(offset: int) -> (int) -> int:\n    \"\"\"Return a closure owning its offset.\"\"\"\n    return (value) => value + offset\n\n",
            "def scaled(value: int) -> int:\n    \"\"\"Box the same item as main does, from a second function.\"\"\"\n    return apply_callable(double, value)\n\n",
            "def main() -> None:\n    \"\"\"Exercise stored, borrowed, returned, and snapshot closures.\"\"\"\n",
            "    stored = double\n",
            "    println(stored(4))\n",
            "    println(apply(double, 5))\n",
            "    println(apply((value) => value + 1, 4))\n",
            "    println(apply_callable(double, 6))\n",
            "    println(scaled(7))\n",
            "    offset = 2\n",
            "    add: (int) -> int = (value) => value + offset\n",
            "    println(add(40))\n",
            "    println(apply(add, 39))\n",
            "    println(add(41))\n",
            "    returned = make_adder(3)\n",
            "    println(returned(40))\n",
            "    mut bias = 2\n",
            "    snapshot: (int) -> int = (value) => value + bias\n",
            "    bias = 5\n",
            "    println(snapshot(40))\n",
            "    println(bias)\n",
        ),
    )?;
    let legacy_root = root.join("legacy");
    success(
        &support::repo_command()
            .current_dir(&root)
            .arg("build")
            .arg(&source)
            .arg(&legacy_root)
            .output()?,
        "legacy function values and closure expressions compilation",
    );
    let legacy = Command::new(legacy_root.join("oven/release/closures")).output()?;
    success(&legacy, "legacy function values and closure expressions execution");
    assert_eq!(legacy.stdout, b"8\n10\n5\n12\n14\n42\n41\n43\n43\n42\n5\n");
    let closure = corpus::runtime_closure(&fixture.formatting)?;
    let native = root.join("native");
    success(
        &corpus::source_command(
            &fixture.driver_binary("debug"),
            &source,
            &native,
            &fixture.sysroot,
            &closure,
        )
        .output()?,
        "native function values and closure expressions compilation",
    );
    let actual = Command::new(native).output()?;
    success(&actual, "native function values and closure expressions execution");
    assert_eq!(actual.stdout, legacy.stdout);
    Ok(())
}

/// A closure expression over a sized carrier keeps that carrier's native arithmetic through its lifted function,
/// pointer reification, and indirect calls: `100000 * 100000` wraps in `i32` exactly as legacy's release build does,
/// where an `i64` slip would print `10000000000`.
#[test]
fn direct_route_sized_numeric_closures_match_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let root = fixture.scratch("sized-closures")?;
    let source = root.join("sized_closures.incn");
    fs::write(
        &source,
        concat!(
            "def main() -> None:\n",
            "    square: (i32) -> i32 = (value) => value * value\n",
            "    println(square(100000))\n",
            "    cube: (i32) -> i32 = (value) => value * value * value\n",
            "    println(cube(2000))\n",
            "    println(cube(-7))\n",
        ),
    )?;
    let legacy_root = root.join("legacy");
    success(
        &support::repo_command()
            .current_dir(&root)
            .arg("build")
            .arg(&source)
            .arg(&legacy_root)
            .output()?,
        "legacy sized-numeric closure compilation",
    );
    let legacy = Command::new(legacy_root.join("oven/release/sized_closures")).output()?;
    success(&legacy, "legacy sized-numeric closure execution");
    assert_eq!(legacy.stdout, b"1410065408\n-589934592\n-343\n");
    let closure = corpus::runtime_closure(&fixture.formatting)?;
    let native = root.join("native");
    success(
        &corpus::source_command(
            &fixture.driver_binary("debug"),
            &source,
            &native,
            &fixture.sysroot,
            &closure,
        )
        .output()?,
        "native sized-numeric closure compilation",
    );
    let actual = Command::new(native).output()?;
    success(&actual, "native sized-numeric closure execution");
    assert_eq!(actual.stdout, legacy.stdout);
    Ok(())
}

/// Prove stored items, aliases, explicit pointer coercions, returned pointers, and indirect invocation separately
/// before the same focused test exercises closure-holding contracts.
fn check_function_item_values(fixture: &DriverFixture, root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let source = root.join("function_items.incn");
    fs::write(
        &source,
        concat!(
            "def double(value: int) -> int:\n    return value * 2\n\n",
            "def pointer() -> (int) -> int:\n    return double\n\n",
            "def echo(value: str) -> str:\n    return value\n\n",
            "def main() -> None:\n",
            "    stored = double\n    alias = stored\n    typed: (int) -> int = double\n",
            "    println(alias(4))\n    println(typed(5))\n",
            "    returned = pointer()\n    println(returned(6))\n",
            "    stored_echo = echo\n    println(stored_echo(\"hello\"))\n",
        ),
    )?;
    let legacy_root = root.join("function_items_legacy");
    success(
        &support::repo_command()
            .current_dir(root)
            .arg("build")
            .arg(&source)
            .arg(&legacy_root)
            .output()?,
        "legacy function-item compilation",
    );
    let legacy = Command::new(legacy_root.join("oven/release/function_items")).output()?;
    success(&legacy, "legacy function-item execution");
    assert_eq!(legacy.stdout, b"8\n10\n12\nhello\n");
    let closure = corpus::runtime_closure(&fixture.formatting)?;
    let native = root.join("function_items_native");
    success(
        &corpus::source_command(
            &fixture.driver_binary("debug"),
            &source,
            &native,
            &fixture.sysroot,
            &closure,
        )
        .output()?,
        "native function-item compilation",
    );
    let actual = Command::new(native).output()?;
    success(&actual, "native function-item execution");
    assert_eq!(actual.stdout, legacy.stdout);
    eprintln!("stored function items and pointers matched legacy byte for byte");
    Ok(())
}

/// Measure every behavior fixture only when explicitly requested.
#[test]
#[ignore = "explicit full direct-route census"]
fn direct_route_fixture_census() -> Result<(), Box<dyn std::error::Error>> {
    census::run()
}

/// Import-activated async vocabulary checks before pending runtime operations receive a named direct-route refusal.
fn check_async_frontend_refusal(
    driver: &Path,
    root: &Path,
    sysroot: &Path,
    closure: &corpus::NativeClosure,
) -> Result<(), Box<dyn std::error::Error>> {
    let source = root.join("async_frontend.incn");
    fs::write(
        &source,
        "import std.async\nfrom std.async.time import sleep_ms\n\nasync def main() -> None:\n    await sleep_ms(1)\n",
    )?;
    success(
        &support::repo_command().arg("check").arg(&source).output()?,
        "legacy async checking",
    );
    let output = corpus::source_command(driver, &source, &root.join("async-native"), sysroot, closure).output()?;
    assert!(!output.status.success());
    let diagnostic = String::from_utf8(output.stderr)?;
    assert!(diagnostic.contains("unsupported Body IR"), "{diagnostic}");
    Ok(())
}

/// Prove named construction, field mutation, argument passing, returned models, and final drops against legacy.
#[test]
fn plain_model_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_plain_model(
        &fixture.driver_binary("debug"),
        &fixture.scratch("plain-model")?,
        &fixture.sysroot,
        &fixture.formatting,
    )
}

/// Prove two checked instantiations of a function with type parameters against legacy output.
#[test]
fn type_parameter_function_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("type-parameter-function")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"def identity[T](value: T) -> T:
    return value

def forward[T](value: T) -> T:
    copied = value
    return identity[T](value)

def main() -> None:
    println(identity[int](42))
    println(identity[bool](true))
    println(identity(7))
    println(forward[int](9))
    println(identity[str]("text"))
"#,
    )
}

/// Prove closed class layouts and methods with type parameters against legacy output.
#[test]
fn type_parameter_class_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("type-parameter-class")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"def identity[T](value: T) -> T:
    return value

class Box[T]:
    pub value: T

    def get(self) -> T:
        return self.value

    def forward(self) -> T:
        return identity[T](self.value)

    def pick[U](self, value: U) -> U:
        return value

class Picker:
    def pick[T](self, value: T) -> T:
        return value

def read_box(value: Box[int]) -> int:
    return value.get()

def main() -> None:
    whole = Box[int](value=42)
    flag = Box[bool](value=true)
    text = Box[str](value="stored")
    println(whole.get())
    println(read_box(whole))
    println(text.forward())
    println(flag.get())
    println(text.get())
    println(whole.pick[int](7))
    println(flag.pick[int](8))
    picker = Picker()
    println(picker.pick[int](9))
    println(picker.pick(true))
    println(picker.pick[str]("picked"))
"#,
    )
}

/// Prove model owner layouts, independent method arguments, defaults, and source Debug names against legacy.
#[test]
fn type_parameter_model_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("type-parameter-model")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"def identity[T](value: T) -> T:
    return value

@derive(Debug)
model Envelope[T]:
    pub value: T
    pub label: str = "label"

    def get(self) -> T:
        return self.value

    def forward(self) -> T:
        return identity[T](self.value)

    def pick[U](self, value: U) -> U:
        return value

@derive(Debug)
model Marker[T]:
    pub label: str

def read[T](value: Envelope[T]) -> T:
    return value.value

def main() -> None:
    whole = Envelope[int](value=42)
    text = Envelope[str](value="stored")
    marker = Marker[int](label="marker")
    println(whole.get())
    println(text.forward())
    println(read(text))
    println(whole.pick[str]("picked"))
    println(text.pick(true))
    println(whole.label)
    println(f"{whole:?}")
    println(f"{text:?}")
    println(f"{marker:?}")
"#,
    )
}

/// Prove inferred constructor carriers close generic function and method arguments without written type arguments.
#[test]
fn type_parameter_inferred_model_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("release"),
        &fixture.scratch("type-parameter-inferred-model")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"model Boxed[T]:
    pub value: T

def get_value[T](boxed: Boxed[T]) -> T:
    return boxed.value

class Reader:
    def read[T](self, boxed: Boxed[T]) -> T:
        return boxed.value

def main() -> None:
    reader = Reader()
    println(get_value(Boxed(value=41)))
    println(get_value(Boxed(value="forty-one")))
    println(reader.read(Boxed(value=42)))
    println(reader.read(Boxed(value="forty-two")))
"#,
    )
}

/// Prove carriers retained only inside closed function instances, including forwarded calls, against legacy.
#[test]
fn type_parameter_carrier_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("type-parameter-carrier")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"def show[T with Display](value: T) -> None:
    stored: Option[T] = Some(value)
    println(stored.unwrap_or(value))

def forward[T with Display](value: T) -> None:
    show[T](value)

def identity[T](value: Option[T]) -> Option[T]:
    return value

def main() -> None:
    forward[int](7)
    forward[str]("text")
    println(identity[int](Some(9)).unwrap_or(0))
"#,
    )
}

/// Prove list indexing, mutation, shared parameters, owned returns and iteration against legacy.
#[test]
fn direct_route_lists_match_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_lists(
        &fixture.driver_binary("debug"),
        &fixture.scratch("lists")?,
        &fixture.sysroot,
        &fixture.formatting,
    )
}

/// Compare scalar tuple list construction, indexed reads, and copies byte-for-byte with legacy.
#[test]
fn direct_route_tuple_lists_match_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let root = fixture.scratch("tuple-lists")?;
    let source = root.join("tuple_lists.incn");
    fs::write(
        &source,
        r#"
def main() -> None:
    """Exercise copied tuple lists, iteration, and owned string fields."""
    mut pairs: list[tuple[int, int]] = [(1, 2), (3, 4)]
    pairs.append((5, 6))
    copied = pairs
    pair = copied[1]
    println(pair[0])
    println(pair[1])
    println(len(pairs))
    empty: list[tuple[int, int]] = []
    println(len(empty))
    for item in pairs:
        println(item[0] + item[1])
    texts: list[tuple[str, int]] = [("one", 1), ("two", 2)]
    text = texts[0]
    println(text[0])
    for item in texts:
        println(item[0])
"#,
    )?;
    let closure = corpus::runtime_closure(&fixture.formatting)?;
    let native = root.join("tuple-lists-native");
    success(
        &corpus::source_command(
            &fixture.driver_binary("debug"),
            &source,
            &native,
            &fixture.sysroot,
            &closure,
        )
        .output()?,
        "native tuple lists compilation",
    );
    let legacy_root = root.join("tuple-lists-legacy");
    success(
        &support::repo_command()
            .current_dir(&root)
            .arg("build")
            .arg(&source)
            .arg(&legacy_root)
            .output()?,
        "legacy tuple lists compilation",
    );
    let expected = Command::new(legacy_root.join("oven/release/tuple_lists")).output()?;
    let actual = Command::new(native).output()?;
    success(&expected, "legacy tuple lists execution");
    success(&actual, "native tuple lists execution");
    assert_eq!(actual.stdout, expected.stdout);
    assert_eq!(actual.stdout, b"3\n4\n3\n0\n3\n7\n11\none\none\ntwo\n");
    Ok(())
}

/// Compare model list construction, copied reads, and iteration byte-for-byte with legacy.
#[test]
fn direct_route_model_lists_match_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let root = fixture.scratch("model-lists")?;
    let source = root.join("model_lists.incn");
    fs::write(
        &source,
        r#"
model Entry:
    value: int
    text: str

def main() -> None:
    """Exercise source-local model leaves and owned text through list copies."""
    mut entries = [Entry(value=1, text="one"), Entry(value=2, text="two")]
    entries.append(Entry(value=3, text="three"))
    copied = entries
    entry = copied[1]
    println(entry.value)
    println(entry.text)
    println(len(entries))
    for item in entries:
        println(item.value)
        println(item.text)
"#,
    )?;
    let closure = corpus::runtime_closure(&fixture.formatting)?;
    let native = root.join("model-lists-native");
    success(
        &corpus::source_command(
            &fixture.driver_binary("debug"),
            &source,
            &native,
            &fixture.sysroot,
            &closure,
        )
        .output()?,
        "native model lists compilation",
    );
    let legacy_root = root.join("model-lists-legacy");
    success(
        &support::repo_command()
            .current_dir(&root)
            .arg("build")
            .arg(&source)
            .arg(&legacy_root)
            .output()?,
        "legacy model lists compilation",
    );
    let expected = Command::new(legacy_root.join("oven/release/model_lists")).output()?;
    let actual = Command::new(native).output()?;
    success(&expected, "legacy model lists execution");
    success(&actual, "native model lists execution");
    assert_eq!(actual.stdout, expected.stdout);
    assert_eq!(actual.stdout, b"2\ntwo\n3\n1\none\n2\ntwo\n3\nthree\n");
    Ok(())
}

/// Compare exact byte literals, copied byte vectors, and borrowed parameters with legacy.
#[test]
fn direct_route_bytes_match_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let root = fixture.scratch("bytes")?;
    let source = root.join("bytes.incn");
    fs::write(
        &source,
        r#"
def count(value: bytes) -> int:
    """Observe borrowed byte storage without consuming the caller's vector."""
    return len(value)

def main() -> None:
    """Exercise empty and unsigned byte literals and independent copied storage."""
    values = b"\x00\x7f\x80\xff"
    copied = values
    println(count(values))
    println(count(b""))
    println(values[0])
    println(copied[1])
    println(values[2])
    println(copied[3])
"#,
    )?;
    let closure = corpus::runtime_closure(&fixture.formatting)?;
    let native = root.join("bytes-native");
    success(
        &corpus::source_command(
            &fixture.driver_binary("debug"),
            &source,
            &native,
            &fixture.sysroot,
            &closure,
        )
        .output()?,
        "native bytes compilation",
    );
    let legacy_root = root.join("bytes-legacy");
    success(
        &support::repo_command()
            .current_dir(&root)
            .arg("build")
            .arg(&source)
            .arg(&legacy_root)
            .output()?,
        "legacy bytes compilation",
    );
    let expected = Command::new(legacy_root.join("oven/release/bytes")).output()?;
    let actual = Command::new(native).output()?;
    success(&expected, "legacy bytes execution");
    success(&actual, "native bytes execution");
    assert_eq!(actual.stdout, expected.stdout);
    assert_eq!(actual.stdout, b"4\n0\n0\n127\n128\n255\n");
    Ok(())
}

/// Compare unit and scalar tuple hash-key membership and insertion against legacy, including string ownership.
#[test]
fn direct_route_hash_leaves_match_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let root = fixture.scratch("hash-leaves")?;
    let source = root.join("hash_leaves.incn");
    fs::write(
        &source,
        r#"
def unit_value() -> None:
    """Return a concrete unit hash key."""
    pass

def main() -> None:
    """Exercise unit keys, tuple keys, duplicate elimination, and owned text."""
    mut units = {unit_value()}
    units.add(unit_value())
    println(len(units))
    println(unit_value() in units)
    mut rows: dict[tuple[str, int], int] = {("key", 1): 7}
    println(("key", 1) in rows)
    rows.insert(("other", 2), 9)
    println(len(rows))
    keys = {("left", 1), ("right", 2)}
    println(("left", 1) in keys)
"#,
    )?;
    let closure = corpus::runtime_closure(&fixture.formatting)?;
    let native = root.join("hash-leaves-native");
    success(
        &corpus::source_command(
            &fixture.driver_binary("debug"),
            &source,
            &native,
            &fixture.sysroot,
            &closure,
        )
        .output()?,
        "native hash leaves compilation",
    );
    let legacy_root = root.join("hash-leaves-legacy");
    success(
        &support::repo_command()
            .current_dir(&root)
            .arg("build")
            .arg(&source)
            .arg(&legacy_root)
            .output()?,
        "legacy hash leaves compilation",
    );
    let expected = Command::new(legacy_root.join("oven/release/hash_leaves")).output()?;
    let actual = Command::new(native).output()?;
    success(&expected, "legacy hash leaves execution");
    success(&actual, "native hash leaves execution");
    assert_eq!(actual.stdout, expected.stdout);
    assert_eq!(actual.stdout, b"1\ntrue\ntrue\n2\ntrue\n");
    Ok(())
}

/// Compare exact decimal scale, function boundaries, numeric comparisons, and hashed duplicate elimination.
#[test]
fn direct_route_decimals_match_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let root = fixture.scratch("decimals")?;
    let source = root.join("decimals.incn");
    fs::write(
        &source,
        r#"
def identity(value: decimal[38, 2]) -> decimal[38, 2]:
    """Retain the written scale across a function boundary."""
    println(value)
    return value

def main() -> None:
    """Observe exact digits, scale, numeric ordering, and collection hashing."""
    a: decimal[4, 2] = 1.50d
    b: decimal[3, 1] = 1.5d
    c: decimal[4, 2] = 1.49d
    println(a == b)
    println(a != b)
    println(c < b)
    println(c <= b)
    println(b > c)
    println(b >= c)
    println(len({a, b}))
    println(identity(19.90d))
    large: decimal[38, 2] = 123456789012345678901234567890123456.78d
    println(identity(large))
    values = [a, b]
    println(values[0])
    println(values[1])
"#,
    )?;
    let closure = corpus::runtime_closure(&fixture.formatting)?;
    let native = root.join("decimals-native");
    success(
        &corpus::source_command(
            &fixture.driver_binary("debug"),
            &source,
            &native,
            &fixture.sysroot,
            &closure,
        )
        .output()?,
        "native decimal compilation",
    );
    let legacy_root = root.join("decimals-legacy");
    success(
        &support::repo_command()
            .current_dir(&root)
            .arg("build")
            .arg(&source)
            .arg(&legacy_root)
            .output()?,
        "legacy decimal compilation",
    );
    let expected = Command::new(legacy_root.join("oven/release/decimals")).output()?;
    let actual = Command::new(native).output()?;
    success(&expected, "legacy decimal execution");
    success(&actual, "native decimal execution");
    assert_eq!(actual.stdout, expected.stdout);
    assert_eq!(actual.stdout, b"true\nfalse\ntrue\ntrue\ntrue\ntrue\n1\n19.90\n19.90\n123456789012345678901234567890123456.78\n123456789012345678901234567890123456.78\n1.50\n1.5\n");
    Ok(())
}

/// Compare frozen static text, UTF-8 length, copied calls, string conversion, fields, and collection leaves.
#[test]
fn direct_route_frozen_strings_match_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let root = fixture.scratch("frozen-strings")?;
    let source = root.join("frozen_strings.incn");
    fs::write(
        &source,
        r#"
model Holder:
    label: FrozenStr

def keep(value: FrozenStr) -> FrozenStr:
    """Keep the static carrier across copied calls."""
    return value

def policy() -> FrozenStr:
    """Return a frozen Unicode literal."""
    return "é😀"

def label(text: str) -> str:
    """Accept owned text at an explicit string boundary."""
    return f"[{text}]"

def main() -> None:
    """Observe frozen carriers without replacing their storage with owned strings."""
    println(policy())
    println(len(policy()))
    println(keep("strict"))
    held = Holder(label="held")
    println(held.label)
    println(label(held.label))
    values: list[FrozenStr] = ["left", "right"]
    copied = values
    println(copied[0])
    println(copied[1])
    words: dict[str, FrozenStr] = {"k": "value"}
    println(words["k"])
"#,
    )?;
    let closure = corpus::runtime_closure(&fixture.formatting)?;
    let native = root.join("frozen-strings-native");
    success(
        &corpus::source_command(
            &fixture.driver_binary("debug"),
            &source,
            &native,
            &fixture.sysroot,
            &closure,
        )
        .output()?,
        "native frozen strings compilation",
    );
    let legacy_root = root.join("frozen-strings-legacy");
    success(
        &support::repo_command()
            .current_dir(&root)
            .arg("build")
            .arg(&source)
            .arg(&legacy_root)
            .output()?,
        "legacy frozen strings compilation",
    );
    let expected = Command::new(legacy_root.join("oven/release/frozen_strings")).output()?;
    let actual = Command::new(native).output()?;
    success(&expected, "legacy frozen strings execution");
    success(&actual, "native frozen strings execution");
    assert_eq!(actual.stdout, expected.stdout);
    assert_eq!(
        actual.stdout,
        "é😀\n2\nstrict\nheld\n[held]\nleft\nright\nvalue\n".as_bytes()
    );
    Ok(())
}

/// Prove class construction, shared and mutable receivers, and passing classes against legacy.
#[test]
fn source_class_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_source_class(
        &fixture.driver_binary("debug"),
        &fixture.scratch("source-class")?,
        &fixture.sysroot,
        &fixture.formatting,
    )
}

/// Explicit class derives use the checked selection: repeated automatic derives, and Display through the value's
/// Debug structure.
#[test]
fn class_derives_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("class-derives")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"@derive(Clone, Debug)
class Counter:
    value: int

    def get(self) -> int:
        return self.value

    def bump(mut self) -> None:
        self.value += 1


@derive(Display)
class Account:
    pub owner: str
    pub balance: int


def describe(account: Account) -> str:
    return f"<{account}>"


def main() -> None:
    mut counter = Counter(value=41)
    println(counter.get())
    counter.bump()
    println(counter.get())
    account = Account(owner="Ada", balance=3)
    println(account)
    println(f"{account}")
    println(str(account))
    println(describe(account))
"#,
    )
}

/// Omitted class fields evaluate their defaults once per construction; supplied fields suppress those effects.
#[test]
fn class_field_defaults_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("class-field-defaults")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"def secret() -> int:
    println(99)
    return 7


class Vault:
    pub label: int
    hidden: int = secret()
    tag: str = "sealed"

    def reveal(self) -> str:
        return f"{self.label} {self.hidden} {self.tag}"


def main() -> None:
    first = Vault(label=1)
    second = Vault(label=2, hidden=9, tag="open")
    println(first.reveal())
    println(second.reveal())
"#,
    )
}

/// Builtin staticmethod and classmethod markers on models and classes, including a classmethod constructing an
/// explicitly instantiated generic owner, match legacy.
#[test]
fn builtin_method_markers_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("builtin-method-markers")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"class Factory:
    @classmethod
    def answer(cls) -> int:
        return 42

    @staticmethod
    def twice(value: int) -> int:
        return value * 2


model Page:
    n: int

    @staticmethod
    def make(n: int) -> Page:
        return Page(n=n)

    @classmethod
    def build(cls, n: int) -> Self:
        return cls(n=n)


@derive(Clone)
class FactoryBox[T with Clone]:
    pub value: T

    @classmethod
    def make(cls, value: T) -> Self:
        return cls(value=value)


def main() -> None:
    println(Factory.answer())
    println(Factory.twice(21))
    println(Page.make(3).n)
    println(Page.build(5).n)
    println(FactoryBox[int].make(1).value)
    println(FactoryBox[str].make("two").value)
"#,
    )
}

/// Binary, comparison, unary, index and index-assignment operators on models and classes call their dunder methods
/// exactly as legacy does.
#[test]
fn user_defined_operators_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("user-defined-operators")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"model Money:
    cents: int

    def __add__(self, other: Money) -> Money:
        return Money(cents=self.cents + other.cents)

    def __lt__(self, other: Money) -> bool:
        return self.cents < other.cents


model Value:
    value: int
    label: str

    def __eq__(self, other: Value) -> bool:
        return self.value == other.value


model Row:
    value: int

    def __getitem__(self, index: int) -> int:
        return self.value + index

    def __setitem__(self, index: int, value: int) -> None:
        println(f"set {index} to {value}")


class OpBox:
    pub value: int

    def __matmul__(self, other: OpBox) -> OpBox:
        return OpBox(value=self.value + other.value)

    def __and__(self, other: OpBox) -> OpBox:
        return OpBox(value=self.value * other.value)

    def __invert__(self) -> OpBox:
        return OpBox(value=0 - self.value)


def same(left: Value, right: Value) -> bool:
    return left == right


def main() -> None:
    a = Money(cents=100)
    b = Money(cents=25)
    println((a + b).cents)
    println(b < a)
    println(same(Value(value=1, label="a"), Value(value=1, label="b")))
    println(Value(value=3, label="x") == Value(value=4, label="x"))
    row = Row(value=4)
    println(row[3])
    row[3] = 9
    left = OpBox(value=2)
    right = OpBox(value=3)
    println(f"{(left @ right).value} {(left & right).value} {(~left).value}")
"#,
    )
}

/// Class method aliases, including one that fills a trait's required method, and a newtype's short rebinding call the
/// methods they name, as legacy does.
#[test]
fn class_and_newtype_method_aliases_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("class-and-newtype-method-aliases")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"trait Renamable:
    def where(self, value: int) -> int: ...


class Example with Renamable:
    where = alias filter

    def filter(self, value: int) -> int:
        return value


class Counter:
    pub n: int
    grow = add

    def add(mut self) -> None:
        self.n += 1

    def read(self) -> int:
        return self.n

    current = read


type Meters = newtype int:
    def value(self) -> int:
        return self.0
    get = value


def main() -> None:
    println(Example().where(7))
    println(Example().filter(8))
    mut counter = Counter(n=1)
    counter.grow()
    counter.grow()
    println(counter.current())
    println(Meters(3).get())
"#,
    )
}

/// Lint suppressions on models, classes, enums, newtypes, methods and functions change nothing legacy prints.
#[test]
fn lint_suppressions_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("lint-suppressions")?,
        &fixture.sysroot,
        &fixture.formatting,
        include_str!("../../incan_test_support/fixtures/behavior/snapshots_stdlib/rust_allow_lint_suppressions.incn"),
    )
}

/// Model and class fields read by value, as arguments, receivers' arguments and returned values, clone from their
/// owners and leave them intact, as legacy does (#241).
#[test]
fn nominal_field_reads_by_value_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("nominal-field-reads-by-value")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"@derive(Clone)
class Cursor:
    pub name: str

    def join(self, other: Self) -> Self:
        return Cursor(name=f"{self.name}+{other.name}")


model Pair:
    left: Cursor
    right: Cursor

    def first(self) -> Cursor:
        return self.left


@derive(Clone)
class Wrapper:
    _cursor: Cursor

    def merge(self, other: Self) -> Self:
        return Wrapper(_cursor=self._cursor.join(other._cursor))

    def label(self) -> str:
        return self._cursor.name


def take(cursor: Cursor) -> str:
    return cursor.name


def main() -> None:
    pair = Pair(left=Cursor(name="a"), right=Cursor(name="b"))
    println(take(pair.left))
    println(pair.left.join(pair.right).name)
    println(pair.first().name)
    println(pair.left.name)
    left = Wrapper(_cursor=Cursor(name="left"))
    right = Wrapper(_cursor=Cursor(name="right"))
    println(left.merge(right).label())
    println(left.label())
    println(right.label())
"#,
    )
}

/// Partials over model and class constructors build their values with presets as overridable defaults and every
/// other field left to the declaration's own default or the call's argument, as legacy does (#701).
#[test]
fn constructor_partials_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("constructor-partials")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"const DEFAULT_NAMESPACE: str = "registry"


class Counter:
    pub start: int
    pub step: int = 1


counter = partial Counter(step=2)
plain = partial Counter(start=5)


model Spec:
    name: str
    namespace: str = "unset"
    replicas: int = 1

    def describe(self) -> str:
        return f"{self.namespace}/{self.name}:{self.replicas}"


spec = partial Spec(namespace=DEFAULT_NAMESPACE)


pub model Reader:
    pub layer: str
    pub format: str


pub BronzeReader = partial Reader(layer="bronze", format="delta")


def main() -> None:
    c = counter(start=1)
    println(f"{c.start} {c.step}")
    p = plain()
    println(f"{p.start} {p.step}")
    q = plain(start=4, step=3)
    println(f"{q.start} {q.step}")
    println(spec(name="alpha", replicas=3).describe())
    println(spec(name="beta").describe())
    println(spec(name="gamma", namespace="custom").describe())
    reader = BronzeReader()
    println(f"{reader.layer} {reader.format}")
"#,
    )
}

/// `Ok(...)` and `Err(...)` values passed to generic functions and methods, alone, by name or in a list literal,
/// returned by closures and built by a list comprehension take the `Result` side the checker settles for them, as
/// legacy builds them (#1561).
#[test]
fn settled_result_constructor_sides_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("settled-result-constructor-sides")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"model Picker:
    n: int

    def pick[T](self, r: Result[T, int], d: T) -> T:
        return r.unwrap_or(d)


def pick[T](r: Result[T, int], d: T) -> T:
    return r.unwrap_or(d)


def pick_list[T](rs: list[Result[T, int]], d: T) -> T:
    for r in rs:
        return r.unwrap_or(d)
    return d


def run() -> Result[str, int]:
    f = () => Ok(1)
    println(f().unwrap_or(0))
    g = () => Err(5)
    match g():
        Ok(_) => println("ok")
        Err(e) => println(e)
    println(pick(Err(2), 6))
    return Ok("done")


def main() -> None:
    println(pick(Err(2), 5))
    println(pick(Ok(7), 5))
    println(pick(d=8, r=Err(2)))
    println(pick_list([Err(3)], 9))
    println(Picker(n=1).pick(Err(2), 10))
    e = () => Err("no")
    match e():
        Ok(_) => println("ok")
        Err(m) => println(m)
    ys = [Ok(x * 2) for x in [1, 2]]
    for y in ys:
        println(y.unwrap_or(0))
    println(run().unwrap_or("none"))
"#,
    )
}

/// A `Result` or `Option` payload a generic function binds by matching its type-parameter argument is copied out when
/// the instance makes it a scalar, and cloned when it makes it a string, as legacy's instances do.
#[test]
fn scalar_payload_bindings_in_type_parameter_instances_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>>
{
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("scalar-payload-bindings")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"def first[T](r: Result[T, int]) -> Option[T]:
    match r:
        Ok(v) => return Some(v)
        Err(_) => return None


def describe[T with Display](r: Result[T, int]) -> str:
    match r:
        Ok(v) => return f"{v}"
        Err(e) => return f"err {e}"


def flag[T](o: Option[T], fallback: T) -> T:
    match o:
        Some(v) => return v
        None => return fallback


def main() -> None:
    match first(Ok(3)):
        Some(v) => println(v)
        None => println("none")
    match first(Err(4)):
        Some(_) => println("some")
        None => println("none")
    println(describe(Ok(7)))
    println(describe(Ok(2.5)))
    println(describe(Ok(true)))
    println(describe(Ok("text")))
    println(describe[str](Err(4)))
    println(flag(Some(false), true))
"#,
    )
}

/// Method partials on models and classes, directly or through a method alias and over `mut self` methods, call their
/// target with the presets the call omits and the arguments it supplies, and an ordinary method call takes its
/// method's own defaults for the slots it omits, as legacy does.
#[test]
fn method_partials_and_method_defaults_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("method-partials")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"model User:
    name: str

    def label(self, prefix: str, suffix: str = "!") -> str:
        return f"{prefix} {self.name}{suffix}"

    display = label
    short = partial display(prefix="name")
    brief = partial label(prefix="brief", suffix="?")


model Cell:
    alive: bool
    set_alive = partial set_state(state=true)
    set_dead = partial set_state(state=false)

    def set_state(mut self, state: bool) -> None:
        self.alive = state


class Counter:
    pub count: int

    def bump(mut self, by: int, times: int = 1) -> None:
        self.count += by * times

    bump_one = partial bump(by=1)

    def total(self, extra: int = 10) -> int:
        return self.count + extra


def main() -> None:
    user = User(name="Ada")
    println(user.short())
    println(user.brief())
    println(user.short(prefix="other"))
    println(user.short(suffix="."))
    println(user.display("direct"))
    mut cell = Cell(alive=false)
    println(cell.alive)
    cell.set_alive()
    println(cell.alive)
    cell.set_dead()
    println(cell.alive)
    mut counter = Counter(count=0)
    counter.bump_one()
    counter.bump_one(times=3)
    counter.bump(2)
    println(counter.count)
    println(counter.total())
    println(counter.total(1))
"#,
    )
}

/// Computed properties on a model and a class run their bodies on each read, in expressions, f-strings, as a method
/// receiver and from another member, as legacy does.
#[test]
fn computed_properties_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("computed-properties")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"model Account:
    cents: int

    property dollars -> int:
        return self.cents // 100

    property label -> str:
        return f"{self.dollars} dollars"

    def doubled(self) -> int:
        return self.dollars * 2


class Wallet:
    pub owner: str
    pub coins: list[int]

    pub property total -> int:
        mut sum = 0
        for coin in self.coins:
            sum += coin
        return sum

    pub property name -> str:
        return self.owner.upper()


def main() -> None:
    account = Account(cents=1250)
    println(account.dollars)
    println(account.label)
    println(account.doubled())
    println(account.label.upper())
    wallet = Wallet(owner="ada", coins=[1, 2, 3])
    println(wallet.total + 1)
    println(f"{wallet.name}: {wallet.total}")
"#,
    )
}

/// A value whose checked type is a type parameter displays, in an f-string and when printed, through its type
/// argument's own display in every closed instance, an integral float without its fractional part, including inside an
/// operator hook of a generic owner whose type parameter no field mentions (#1370), as legacy's generic code does.
#[test]
fn type_parameter_display_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("type-parameter-display")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"@derive(Debug)
model Column[T with Display]:
    sql: str

    def __mul__(self, other: T) -> Column[T]:
        return Column[T](sql=f"({self.sql} * {other})")


def show[T with Display](value: T) -> str:
    return f"<{value}>"


def announce[T with Display](value: T) -> None:
    println(value)


def main() -> None:
    amount = Column[float](sql="amount")
    println((amount * 2.0).sql)
    println((amount * 2.5).sql)
    label = Column[str](sql="label")
    println((label * "x").sql)
    println(show(2.0))
    println(show(1.5))
    println(show(3))
    println(show("s"))
    announce(4.0)
    announce(0.25)
    announce(7)
    println(f"{amount:?}")
"#,
    )
}

/// Generic functions whose parameters are callables over their type parameters (`transform: (T) -> U`,
/// `f: () -> Result[T, E]`) run in each closed instance with a named function or a closure as the argument, as legacy
/// does.
#[test]
fn generic_callable_parameters_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("generic-callable-parameters")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"def call_both[T, U with Display](transform: (T) -> U, first: T, second: T) -> str:
    return f"{transform(first)} {transform(second)}"


def call[T, E](f: () -> Result[T, E]) -> Result[T, E]:
    return f()


def apply[T](f: (T) -> T, value: T) -> T:
    return f(value)


def increment(value: int) -> int:
    return value + 1


def shout(text: str) -> str:
    return text.upper()


def main() -> None:
    println(call_both(increment, 1, 2))
    println(call_both(shout, "a", "b"))
    println(call(() => Ok(2)).unwrap_or(0))
    println(apply(increment, 41))
    println(apply((x) => x * 2, 21))
"#,
    )
}

/// A generic call whose result a constructor argument closes is matched with its payloads at the closed type, and the
/// callee's instance reads its type-parameter field of a generic model by copy for a scalar and by clone for a string,
/// as legacy does.
#[test]
fn matched_generic_results_closed_by_constructor_arguments_output_matches_legacy()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("closed-generic-match")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"model Decoded[T]:
    value: T


def unwrap[T](decoded: Decoded[T]) -> Result[T, str]:
    return Ok(decoded.value)


def main() -> None:
    match unwrap(Decoded(value=9)):
        Ok(number) => println(number)
        Err(message) => println(message)
    match unwrap(Decoded(value=true)):
        Ok(flag) => println(flag)
        Err(message) => println(message)
    match unwrap(Decoded(value="text")):
        Ok(text) => println(text)
        Err(message) => println(message)
    match unwrap(Decoded(value=2.0)):
        Ok(amount) => println(f"{amount} {amount}")
        Err(message) => println(message)
"#,
    )
}

/// A value coerced into a plain newtype where it is used, as an argument of a static factory sharing its spelling
/// with a field, and a construction whose supplied call arguments follow fields defaulted from literals, build their
/// values as legacy does.
#[test]
fn plain_newtype_coercions_and_literal_defaults_beside_calls_output_matches_legacy()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("plain-newtype-coercions")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"type Days = newtype int


model TimeDelta:
    pub days: Days

    @staticmethod
    def days(value: Days) -> TimeDelta:
        return TimeDelta(days=value)


def total(days: Days, extra: int) -> int:
    return days.0 + extra


def label(text: str) -> str:
    println(f"label {text}")
    return text.upper()


model Account:
    balance: int = 0
    ratio: float = 1.5
    name: str
    status: str = "active"
    note: str


def main() -> None:
    delta = TimeDelta.days(-7)
    println(delta.days.0)
    println(total(5, 2))
    account = Account(name=label("ada"), note=label("vip"))
    println(f"{account.balance} {account.ratio} {account.name} {account.status} {account.note}")
"#,
    )
}

/// Prove canonical scalar casts and default values byte-identical to legacy.
#[test]
fn numeric_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_numerics(
        &fixture.driver_binary("debug"),
        &fixture.scratch("numerics")?,
        &fixture.sysroot,
        &fixture.formatting,
    )
}

/// Prove mixed arithmetic whose checked result is a float carrier byte-identical to legacy: `int / int`, `int` with
/// `float` on either side and in `//` and `%`, an `f32` operand widened by `int` or `float`, and a compound `/=`,
/// while `f32` with `f32`, integer results and `**` keep their own carriers.
#[test]
fn mixed_float_arithmetic_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("mixed-float-arithmetic")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"def widen(narrow: f32, whole: int, wide: float) -> float:
    return narrow * whole + narrow - wide

def main() -> None:
    whole: int = 10
    ratio: float = 2.5
    println(whole / 3)
    println(10 / 4)
    println(whole + ratio)
    println(ratio - whole)
    println(whole * ratio)
    println(whole // 4.0)
    println(whole % 4.0)
    narrow: f32 = 1.5
    println(widen(narrow, 3, 0.25))
    println(narrow + narrow)
    mut total: float = 1.0
    total /= 4
    total += whole
    println(total)
    println(whole // 3)
    println(whole % 3)
    println(2 ** 3)
"#,
    )
}

/// Prove list-literal spreads byte-identical to legacy: a local, a nested literal, a tuple of scalars or text, a
/// field and a `mut` parameter each splice at their written positions, and every source that is read again keeps
/// its items.
#[test]
fn list_literal_spread_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("list-literal-spread")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"class Bag:
    items: list[int]

    def doubled(self) -> int:
        more = [*self.items, *self.items]
        return len(more) + len(self.items)

def joined(items: list[int], mut extra: list[int]) -> int:
    both = [*items, *extra, 9]
    extra.append(5)
    return len(both) + len(extra) + both[2]

def main() -> None:
    xs = [2, 3]
    values = [1, *xs, 4]
    println(len(values))
    println(values[1] + values[2])
    println(len(xs))
    tail: tuple[int, int] = (5, 6)
    spliced = [*[7, 8], *tail]
    println(spliced[0] + spliced[3])
    names: tuple[str, str] = ("a", "b")
    letters = ["z", *names]
    println(letters[2])
    println(names[0])
    mut extra = [7]
    println(joined([1, 2], extra))
    println(len(extra))
    println(Bag(items=[1, 2]).doubled())
"#,
    )
}

/// Prove callables whose signatures carry a list byte-identical to legacy: a function item passed as a function
/// pointer, an annotated callable local holding a closure, a returned closure, and a list of callables all take the
/// list by the same shared reference, and the caller's list keeps its items.
#[test]
fn list_callable_signature_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("list-callable-signature")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"def total(items: list[int]) -> int:
    mut sum = 0
    for item in items:
        sum += item
    return sum

def apply(f: (list[int]) -> int, items: list[int]) -> int:
    return f(items)

def counter() -> (list[int]) -> int:
    return (xs) => len(xs)

def main() -> None:
    values = [1, 2, 3]
    println(apply(total, values))
    println(len(values))
    size: (list[int]) -> int = (xs) => len(xs)
    println(size(values))
    println(apply(size, [4, 5]))
    count = counter()
    println(count([6]))
    ops: list[(list[int]) -> int] = [total, counter()]
    first = ops[0]
    println(first(values))
"#,
    )
}

/// Prove mixed numeric carriers in `**` and comparisons byte-identical to legacy: an exact-width integer base keeps
/// its carrier under a literal exponent, an `f32` base widens under a float exponent, and mixed comparisons read both
/// operands at the common carrier.
#[test]
fn numeric_operator_carrier_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("numeric-operator-carrier")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"def signed(a: i8, b: i8) -> str:
    return f"{a ** 2} {a * b}"

def compare(narrow: f32, wide: f64, whole: int, small: i8, byte: u8) -> str:
    return f"{narrow < wide} {narrow == whole} {small < byte} {whole > small}"

def main() -> None:
    println(signed(-7, 3))
    small: u8 = 15
    println(small ** 2)
    deep: i128 = -7
    println(deep ** 3)
    println(2 ** 3)
    narrow: f32 = 2.5
    exact: f64 = 2.0
    println(narrow ** exact)
    println(compare(narrow, 3.0, 2, -1, 200))
"#,
    )
}

/// Prove builtin value methods byte-identical to legacy: `unwrap` on a direct and a nested Option, on a Result and on
/// owned text leaves each receiver usable, `powf` runs on a `float`, and `is_finite` and `is_nan` run on both widths.
/// A receiver projected out of a list element is the separate model index projection family.
#[test]
fn builtin_value_method_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("builtin-value-method")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"def parse(text: str) -> Result[int, str]:
    if text == "7":
        return Ok(7)
    return Err(f"bad {text}")

def main() -> None:
    maybe: Option[int] = Some(4)
    println(maybe.unwrap())
    println(maybe.unwrap_or(0))
    nested: Option[Option[int]] = Some(Some(5))
    println(nested.unwrap().unwrap_or(0))
    println(parse("7").unwrap())
    label: Option[str] = Some("text")
    println(label.unwrap())
    println(label.unwrap())
    x: float = 1.5
    println(x.powf(2.0))
    narrow: f32 = 2.5
    println(narrow.is_finite())
    println(x.is_nan())
    println(float("nan").is_nan())
"#,
    )
}

/// Prove structural matching byte-identical to legacy: model patterns test and bind named fields, including a field
/// named by its RFC 021 alias, a nested alternation
/// that binds nothing tests each alternative, and a guard runs after its arm's pattern bound, falling through to the
/// next arm when it rejects, whether the scrutinee is an `int`, a `bool`, a tuple or an `Option`.
#[test]
fn structural_match_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("structural-match")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"model Account:
    tier: int
    name: str

model Pair:
    left: int
    right: int

model Plan:
    type_ [alias="type"]: str
    name: str

enum Signal:
    Ready
    Stop

def describe(account: Account) -> str:
    match account:
        Account(tier=1) => return "premium"
        Account(tier=2, name=label) => return f"basic {label}"
        _ => return "other"

def describe_plan(plan: Plan) -> str:
    match plan:
        Plan(type="premium") => return "premium plan"
        _ => return "other plan"

def classify(pair: Pair, signal: Signal) -> int:
    match pair:
        case Pair(left=40, right=2):
            match signal:
                case Signal.Ready:
                    return 42
                case Signal.Stop:
                    return 0
        case Pair(left=value):
            return value
    return 0

def size(value: int) -> str:
    match value:
        case n if n < 10:
            return "small"
        case n if n < 20:
            return "medium"
        case _:
            return "large"

def answered(pair: tuple[int, str]) -> str:
    match pair:
        (n, "yes" | "no") if n > 0 => return "answered"
        _ => return "open"

def admits(value: list[str]) -> bool:
    return len(value) > 0

def keep(value: list[str], choose: bool) -> list[str]:
    match choose:
        case true if admits(value): return value
        case _: return value

def greet(name: Option[str]) -> str:
    match name:
        Some(word) if len(word) > 3 => return word
        Some(word) => return f"short {word}"
        None => return "nobody"

def main() -> None:
    println(describe(Account(tier=1, name="a")))
    println(describe(Account(tier=2, name="b")))
    println(describe(Account(tier=3, name="c")))
    println(describe_plan(Plan(type="premium", name="a")))
    println(describe_plan(Plan(type="basic", name="b")))
    println(classify(Pair(left=40, right=2), Signal.Ready))
    println(classify(Pair(left=40, right=2), Signal.Stop))
    println(classify(Pair(left=7, right=2), Signal.Ready))
    println(size(5))
    println(size(15))
    println(size(25))
    yes: tuple[int, str] = (1, "yes")
    println(answered(yes))
    zero: tuple[int, str] = (0, "no")
    println(answered(zero))
    maybe: tuple[int, str] = (1, "maybe")
    println(answered(maybe))
    println(len(keep(["a", "b"], true)))
    println(len(keep([], true)))
    println(greet(Some("alice")))
    println(greet(Some("bob")))
    println(greet(None))
"#,
    )
}

/// Prove branches that leave different owners live byte-identical to legacy: a fieldless enum compared inside `and`,
/// a model constructed in an `elif` condition, a `str` parameter moved into a dictionary on one branch and read on the
/// other, a moved loop item, and a model or tuple kept by one `match` arm or `if` branch while the other leaves it.
#[test]
fn owned_branch_merge_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("owned-branch-merge")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"enum Signal:
    Ready
    Stop

model Reading:
    value: int

    def is_positive(self) -> bool:
        return self.value > 0

model Outer:
    inner: Reading
    values: list[int]

def score(left: Signal, right: Signal) -> int:
    if left == Signal.Ready and right != Signal.Ready:
        return 42
    return 0

def bump(mut counts: dict[str, int], key: str) -> None:
    if key in counts:
        counts[key] += 1
    else:
        counts[key] = 1

def keep_or_drop(flag: bool, text: str) -> list[str]:
    mut kept: list[str] = []
    if flag:
        kept.append(text)
    return kept

def first_long(words: list[str]) -> str:
    for word in words:
        if len(word) > 2:
            return word
    return "none"

def choose(signal: Signal, reading: Reading) -> int:
    mut kept: list[Reading] = []
    match signal:
        Signal.Ready => kept.append(reading)
        Signal.Stop => pass
    return len(kept)

def pair_or_zero(flag: bool, pair: tuple[str, int]) -> int:
    mut total = 0
    if flag:
        mut held: list[tuple[str, int]] = []
        held.append(pair)
        total = len(held) + 2
    return total

def main() -> None:
    println(score(Signal.Ready, Signal.Stop))
    println(score(Signal.Stop, Signal.Stop))
    mut counts: dict[str, int] = {}
    bump(counts, "a")
    bump(counts, "a")
    bump(counts, "b")
    a = counts["a"]
    b = counts["b"]
    println(f"{a} {b}")
    if false:
        println("wrong")
    elif Outer(inner=Reading(value=3), values=[3]).inner.is_positive():
        println("elif")
    println(len(keep_or_drop(true, "x")))
    println(len(keep_or_drop(false, "y")))
    println(first_long(["a", "abc"]))
    println(first_long(["a"]))
    println(choose(Signal.Ready, Reading(value=1)))
    println(choose(Signal.Stop, Reading(value=2)))
    println(pair_or_zero(true, ("a", 1)))
    println(pair_or_zero(false, ("b", 2)))
"#,
    )
}

/// Prove method arguments byte-identical to legacy: a trait default method's `mut` list parameter changes the caller's
/// list, takes a list literal and an unchanged immutable list, and its `mut` int parameter is the method's own copy; a
/// method on a class or model with type parameters fills its default `str` parameter at positional and named call
/// sites and displays a field of its scalar type parameter; a loop variable reaches a `mut` list parameter; and a list
/// literal reaches a function's `mut` list parameter.
#[test]
fn method_mutable_and_default_arguments_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("method-mutable-and-default-arguments")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"trait Replacer:
    def replace(self, mut items: list[int]) -> int:
        items.append(9)
        return len(items)

    def measured(self, mut items: list[int]) -> int:
        return len(items)

    def bumped(self, mut n: int) -> int:
        n += 1
        return n

model Widget with Replacer:
    id: int

class Box[T with Clone]:
    pub value: T

    def join(self, other: Box[T], suffix: str = "") -> Box[T]:
        println(f"join{suffix}")
        return other

model Shelf[T with Display]:
    item: T
    label: str = ""

    def relabel(self, suffix: str = "") -> Shelf[T]:
        return Shelf(item=self.item, label=f"{self.item}-{suffix}")

def width(mut row: list[int]) -> int:
    return len(row)

def doubled(mut items: list[int]) -> list[int]:
    items.append(1)
    return [x * 2 for x in items]

def main() -> None:
    mut items: list[int] = [1, 2]
    println(Widget(id=1).replace(items))
    println(len(items))
    println(Widget(id=1).replace([5]))
    fixed: list[int] = [1, 2, 3]
    println(Widget(id=1).measured(fixed))
    println(Widget(id=1).bumped(5))
    println(len(items))
    left = Box(value=1)
    right = Box(value=2)
    joined: Box[int] = left.join(right)
    named: Box[int] = left.join(other=right, suffix="!")
    println(joined.value + named.value)
    shelf: Shelf[int] = Shelf(item=1)
    println(shelf.relabel().label)
    println(shelf.relabel(suffix="again").label)
    rows: list[list[int]] = [[1], [2, 3]]
    mut total = 0
    for row in rows:
        total += width(row)
        total += len(row)
    println(total)
    println(rows)
    println(doubled([2]))
"#,
    )
}

/// Prove patterns matched in place byte-identical to legacy: a `match`, `if let` or `while let` arm that changes a
/// list or model its pattern binds from a `mut` local, a field of one or a field of `mut self` changes that place,
/// an `int` bound beside a changed list stays a copy, and a match that only reads leaves its scrutinee alone.
#[test]
fn in_place_pattern_binding_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("in-place-pattern-binding")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"model Holder:
    pub inner: Option[list[int]]

model Counter:
    pub count: int

    def bump(mut self) -> None:
        self.count += 1

enum Slot:
    Pair(list[int], int)
    Empty

enum Wrapper:
    Held(Counter)
    Nothing

class Grid:
    pub row: Option[list[int]]

    def fill(mut self) -> None:
        if let Some(xs) = self.row:
            xs.append(7)

def main() -> None:
    mut box = Some([1])
    match box:
        Some(xs) => xs.append(2)
        None => pass
    if let Some(xs) = box:
        xs.append(3)
    println(box)
    mut h = Holder(inner=Some([1]))
    if let Some(xs) = h.inner:
        xs.append(4)
    println(h.inner)
    mut s = Slot.Pair([1], 5)
    match s:
        Slot.Pair(items, n) =>
            items.append(n)
            println(len(items))
        Slot.Empty => pass
    match s:
        Slot.Pair(items, n) => println(f"{items} {n}")
        Slot.Empty => pass
    mut w = Wrapper.Held(Counter(count=1))
    match w:
        Wrapper.Held(c) => c.bump()
        Wrapper.Nothing => pass
    match w:
        Wrapper.Held(c) => println(c.count)
        Wrapper.Nothing => pass
    mut g = Grid(row=Some([1]))
    g.fill()
    println(g.row)
    mut count = 0
    mut queue = Some([1, 2, 3])
    while let Some(items) = queue:
        items.pop()
        count += 1
        if len(items) == 0 or count == 5:
            break
    println(f"{count} {queue}")
    kept = Some([1])
    match kept:
        Some(xs) => println(len(xs))
        None => pass
"#,
    )
}

/// Prove loops that change their items in place byte-identical to legacy: a loop over a `mut` list parameter or a
/// `mut` local that appends to each list item, also when the loop variable reuses the parameter's name, or calls a
/// `mut self` method on each model item, changes the iterated list, while the same item passed to a function's `mut`
/// parameter is passed as a copy.
#[test]
fn in_place_loop_item_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("in-place-loop-item")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"model Cell:
    pub tags: list[int]

    def tag(mut self, value: int) -> None:
        self.tags.append(value)

def show(mut row: list[int]) -> int:
    return len(row)

def grow(mut items: list[list[int]]) -> int:
    for row in items:
        row.append(3)
    return len(items)

def reuse(mut items: list[list[int]]) -> int:
    for items in items:
        items.append(4)
    return 1

def measure(mut items: list[list[int]]) -> int:
    mut total = 0
    for row in items:
        row.append(0)
        total += show(row)
    return total

def main() -> None:
    mut live: list[list[int]] = [[1]]
    println(grow(live))
    println(live)
    println(reuse(live))
    println(live)
    println(measure(live))
    println(live)
    mut cells = [Cell(tags=[]), Cell(tags=[1])]
    for cell in cells:
        cell.tag(9)
    for cell in cells:
        println(len(cell.tags))
    mut rows: list[list[int]] = [[1], [2]]
    for row in rows:
        row.append(len(row))
    println(rows)
"#,
    )
}

/// Prove `mut` text, model and enum parameters byte-identical to legacy: a `mut str` parameter takes a literal, the
/// caller's variable and a copy of a loop variable; a `mut` `Option`, model or enum parameter changed through a field
/// or a pattern binding changes the caller's value, including a nested `Option`, while one only read leaves the
/// caller's value alone; and a loop over a `mut` list of `Option`s or a match on a list element changes the element
/// in place.
#[test]
fn exclusive_parameter_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("exclusive-parameter")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"model Holder:
    pub inner: Option[list[int]]
    pub count: int

enum Slot:
    Pair(list[int], int)
    Empty

def bump(mut text: str) -> None:
    println(text + "!")

def width(mut s: str) -> int:
    return len(s)

def by_match(mut box: Option[list[int]]) -> None:
    match box:
        Some(xs) => xs.append(1)
        None => pass

def total(mut box: Option[list[int]]) -> int:
    match box:
        Some(xs) => return len(xs)
        None => return 0

def by_field(mut h: Holder) -> None:
    h.count += 1
    match h.inner:
        Some(xs) => xs.append(4)
        None => pass

def by_nested(mut box: Option[Option[list[int]]]) -> None:
    match box:
        Some(inner) =>
            match inner:
                Some(xs) => xs.append(6)
                None => pass
        None => pass

def paired(mut s: Slot) -> int:
    match s:
        Slot.Pair(items, n) =>
            items.append(n)
            return n + len(items)
        Slot.Empty => return 0

def by_loop_view(mut rows: list[Option[list[int]]]) -> None:
    for row in rows:
        if let Some(xs) = row:
            xs.append(5)

def main() -> None:
    bump("hello")
    mut name = "name"
    bump(name)
    println(name)
    names: list[str] = ["a", "bc"]
    mut w = 0
    for item in names:
        w += width(item)
    println(w)
    mut box = Some([0])
    by_match(box)
    println(box)
    println(total(box))
    kept = Some([1, 2])
    println(total(kept))
    println(kept)
    mut h = Holder(inner=Some([0]), count=0)
    by_field(h)
    println(f"{h.inner} {h.count}")
    mut nested = Some(Some([0]))
    by_nested(nested)
    println(nested)
    mut s = Slot.Pair([1], 5)
    println(paired(s))
    println(paired(s))
    mut rows = [Some([0]), None]
    by_loop_view(rows)
    println(rows)
    mut slots: list[Option[list[int]]] = [Some([1])]
    match slots[0]:
        Some(xs) => xs.append(2)
        None => pass
    println(slots)
"#,
    )
}

/// Prove carrier patterns and injections byte-identical to legacy: payload patterns that test a literal, a nested
/// `Option` or `None`; alternations over carriers and fieldless enums, with and without shared bindings, also over a
/// tuple; a value written into a nested `Option` or a narrower integer written into an `Option`, wrapped in each layer;
/// `Option` defaults on model fields and parameters; and `unwrap_or` on a model field and on a list element.
#[test]
fn carrier_pattern_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("carrier-pattern")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"enum Status:
    Pending
    Retrying
    Done

model Counter:
    count: Option[int] = None
    name: Option[str] = None

def describe(result: Result[Option[int], str]) -> str:
    match result:
        Ok(Some(0)) => return "zero"
        Ok(Some(value)) => return f"{value}"
        Ok(None) => return "empty"
        Err(message) => return f"failed:{message}"

def switch(flag: Option[bool]) -> str:
    match flag:
        Some(true) => return "on"
        Some(false) => return "off"
        None => return "unset"

def nested(value: Option[Option[int]]) -> str:
    match value:
        Some(Some(inner)) => return f"inner {inner}"
        Some(None) => return "inner none"
        None => return "none"

def greet(name: Option[str]) -> str:
    match name:
        Some("alice") | Some("bob") => return "friend"
        Some(other) => return f"stranger {other}"
        None => return "nobody"

def label(status: Status) -> str:
    match status:
        Status.Pending | Status.Retrying => return "waiting"
        Status.Done => return "done"

def first(result: Result[int, int]) -> int:
    if let Ok(value) | Err(value) = result:
        return value
    return 0

def either_side(pair: tuple[str, str]) -> str:
    match pair:
        (x, "a") | ("b", x) => return x
        _ => return "none"

def unwrap_twice(value: Option[Option[int]]) -> int:
    return value.unwrap().unwrap_or(0)

def label_or(name: Option[str] = None) -> str:
    return name.unwrap_or("anonymous")

def main() -> None:
    println(describe(Ok(Some(0))))
    println(describe(Ok(Some(4))))
    println(describe(Ok(None)))
    println(describe(Err("disk")))
    println(f"{switch(Some(true))} {switch(Some(false))} {switch(None)}")
    println(f"{nested(Some(Some(3)))} {nested(Some(None))} {nested(None)}")
    println(f"{greet(Some("bob"))} {greet(Some("eve"))} {greet(None)}")
    println(f"{label(Status.Retrying)} {label(Status.Done)}")
    println(f"{first(Ok(1))} {first(Err(2))}")
    println(f"{either_side(("q", "a"))} {either_side(("b", "r"))} {either_side(("c", "d"))}")
    a: Option[Option[int]] = 5
    println(a.unwrap().unwrap())
    mut b: Option[Option[list[int]]] = None
    b = []
    println(len(b.unwrap().unwrap()))
    println(unwrap_twice(6))
    small: i8 = -5
    maybe: Option[int] = small
    println(maybe.unwrap_or(0))
    counter = Counter(count=1)
    println(f"{counter.count.unwrap_or(0)} {counter.name.unwrap_or("none")}")
    slots: list[Option[int]] = [Some(7)]
    println(slots[0].unwrap_or(0))
    println(f"{label_or()} {label_or(Some("kim"))}")
"#,
    )
}

/// Prove closure captures beyond `int`, `float` and `bool` byte-identical to legacy: an `f32` and an `i32` capture
/// keep their carrier, and a captured `str` stays owned by the closure across repeated calls while the captured
/// local is still read after the closure is built.
#[test]
fn closure_capture_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("closure-capture")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"def tagged(prefix: str) -> str:
    render: () -> str = () => prefix + "-item"
    first = render()
    second = render()
    return first + " " + second + " " + prefix

def main() -> None:
    value: f32 = 1.23456789
    show: () -> str = () => str(value)
    println(show())
    step: i32 = 100000
    scale: (i32) -> i32 = (factor) => step * factor
    println(scale(3))
    println(scale(-2))
    println(tagged("a"))
    name = "carol"
    greet: (int) -> str = (times) => f"{name} {times}"
    println(greet(1))
    println(greet(2))
    println(name)
"#,
    )
}

/// Prove collection elements byte-identical to legacy when the checker typed them against a wider element: narrower
/// integers and `f32` values widen in list and dict literals and in indexed list and dict writes, and a frozen `str`
/// constant becomes owned text in an indexed list or dict write.
#[test]
fn collection_element_coercion_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("collection-element-coercion")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"const GREETING: str = "hello"

def main() -> None:
    small: i8 = -5
    byte: u8 = 250
    mut values: list[int] = [small, 0]
    values[1] = byte
    println(values)
    narrow: f32 = 1.5
    floats: list[float] = [narrow, 2.0]
    println(floats)
    mut items: list[str] = ["initial"]
    items[0] = GREETING
    println(items[0])
    mut table: dict[str, str] = {"message": "initial"}
    table["message"] = GREETING
    println(table["message"])
    mut counts: dict[int, int] = {small: byte}
    counts[byte] = small
    println(counts[250])
    println(counts[-5])
    widths: dict[str, float] = {"narrow": narrow}
    println(widths["narrow"])
"#,
    )
}

/// Prove chained assignments of literals over targets of different types byte-identical to legacy: each target gets
/// its own evaluation of the value at its own type, whichever target comes last, under a negation, inside list and
/// dict literals, and for a name the chain declares.
#[test]
fn chained_literal_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("chained-literal")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"def main() -> None:
    mut n: int = 0
    mut f: float = 0.5
    n = f = 1
    println(n)
    println(f)
    f = n = -2
    println(n)
    println(f)
    mut g: f32 = 0.5
    g = n = 3
    println(n)
    println(g)
    mut ints: list[int] = []
    mut floats: list[float] = []
    ints = floats = [4, -4]
    println(ints)
    println(floats)
    mut counts: dict[str, int] = {}
    mut weights: dict[str, float] = {}
    counts = weights = {"a": 5}
    println(counts["a"])
    println(weights["a"])
    x = n = f = 6
    println(x + n)
    println(f)
"#,
    )
}

/// Prove tuples holding a list, an `Option` or another tuple byte-identical to legacy: a list element changes in place
/// through a constant index, including inside a nested tuple, a class field and an indexed write, and is read,
/// measured and passed with its tuple; an `Option` element is matched; a copied tuple stays independent; a returned
/// tuple keeps its `None`; and each tuple displays as legacy spells it.
#[test]
fn tuple_carrier_element_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("tuple-carrier-element")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"class Holder:
    pub pair: tuple[list[int], int]

    def add(mut self, value: int) -> None:
        self.pair[0].append(value)

def total(pair: tuple[list[int], int]) -> int:
    return len(pair[0]) + pair[1]

def label(entry: tuple[Option[str], int]) -> str:
    match entry[0]:
        Some(text) => return f"{text} {entry[1]}"
        None => return f"missing {entry[1]}"

def make(count: int) -> tuple[list[int], Option[str]]:
    mut items: list[int] = []
    for index in range(count):
        items.append(index + 1)
    return (items, None)

def main() -> None:
    mut pair: tuple[list[int], int] = ([], 3)
    pair[0].append(1)
    pair[0].append(2)
    println(len(pair[0]))
    println(pair[0][1])
    println(total(pair))
    println(pair)
    named: tuple[Option[str], int] = (Some("a"), 1)
    println(label(named))
    missing: tuple[Option[str], int] = (None, 2)
    println(label(missing))
    println(named)
    counted: tuple[Option[int], bool] = (Some(4), true)
    println(counted)
    mut first: tuple[list[int], int] = ([1], 0)
    second = first
    first[0].append(2)
    println(f"{first} {second}")
    made = make(3)
    match made[1]:
        Some(text) => println(text)
        None => println(f"{len(made[0])} none")
    println(made)
    mut outer: tuple[int, tuple[list[int], int]] = (0, ([], 0))
    outer[1][0].append(3)
    println(outer[1][0][0])
    println(outer)
    nested: tuple[tuple[Option[int], int], int] = ((None, 1), 2)
    match nested[0][0]:
        Some(value) => println(value)
        None => println(f"inner missing {nested[1]}")
    println(nested)
    mut items: tuple[list[int], int] = ([0, 2], 0)
    items[0][0] = 5
    items[0][0] += 1
    println(items)
    mut holder = Holder(pair=([], 0))
    holder.add(7)
    holder.pair[0].append(8)
    println(f"{len(holder.pair[0])} {holder.pair[0][1]}")
"#,
    )
}

/// Prove trait adoption arguments close nested collection frames and calls between default and required methods, and
/// that a value-returning body ending in a `while true:` loop it leaves only by `return` has no fallthrough.
#[test]
fn trait_default_collection_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("trait-default-collection")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"trait Holder[T]:
    def get(self) -> T: ...

    def pair(self) -> list[T]:
        return [self.get(), self.get()]

model Number with Holder[int]:
    value: int

    def get(self) -> int:
        return self.value

trait Source[T with Clone]:
    def pull(mut self) -> Option[T]: ...

    def drain(mut self) -> list[T]:
        mut items: list[T] = []
        while true:
            match self.pull():
                Some(item) => items.append(item)
                None => return items

model Countdown with Source[list[int]]:
    n: int

    def pull(mut self) -> Option[list[int]]:
        if self.n <= 0:
            return None
        self.n -= 1
        return Some([self.n])

def first_even(start: int) -> int:
    mut i = start
    while true:
        if i % 2 == 0:
            return i
        i += 1

def main() -> None:
    println(Number(value=4).pair())
    mut count = Countdown(n=3)
    items = count.drain()
    println(len(items))
    println(items[0][0] * 10 + items[2][0])
    println(first_even(7))
"#,
    )
}

/// Measure the actual unchanged graph with expensive fallback forbidden and retain numeric parity evidence.
#[test]
#[ignore = "machine-local diagnostic of the 50 ms completed fixture graph reuse budget"]
fn unchanged_fixture_graph_reuse_meets_budget() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let started = std::time::Instant::now();
    for name in ["library", "lowering", "driver", "formatting-caller"] {
        bake_with_reuse_requirement(&fixture.root.join(name), &fixture.home, true)?;
    }
    let elapsed = started.elapsed();
    eprintln!(
        "completed fixture graph reuse: {:.3} ms",
        elapsed.as_secs_f64() * 1000.0
    );
    corpus::check_numerics(
        &fixture.driver_binary("release"),
        &fixture.scratch("warm-reuse-numerics")?,
        &fixture.sysroot,
        &fixture.formatting,
    )?;
    assert!(
        elapsed < std::time::Duration::from_millis(50),
        "completed graph reuse took {elapsed:?}, budget is 50 ms"
    );
    Ok(())
}

/// Verify the profile-specific base, portable package entries, and local native bytes used by completed reuse.
fn diagnose_fixture_package(
    project: &Path,
    packaged: &incan_driver::build::OvenPackagedLibraryLoafProfile,
    output: &incan_driver::build::OvenStoredProjectOutput,
    store: &oven_store::store::OvenStore,
) -> Result<(), Box<dyn std::error::Error>> {
    packaged.receipt.verify_identity()?;
    assert_eq!(packaged.receipt.identity, output.payload.receipt_identity);
    assert_eq!(packaged.receipt.build_unit_identity, output.payload.build_unit_identity);
    assert_eq!(packaged.receipt.intent, output.intent);
    assert_eq!(
        output.payload.package_loaf_store_relative_path.as_deref(),
        Some("target/lib/oven/loafs")
    );
    assert_eq!(packaged.entries, output.payload.required_project_loafs);
    let native = output
        .payload
        .files
        .iter()
        .find(|file| file.output_relative_path == "output/native")
        .ok_or("no native output")?;
    assert_eq!(packaged.library_digest, native.digest);
    assert_eq!(
        Path::new(&native.caller_relative_path).strip_prefix("target/lib")?,
        Path::new(&packaged.library_relative_path)
    );
    assert_eq!(
        oven_store::digest_bytes(&fs::read(
            project.join("target/lib").join(&packaged.library_relative_path)
        )?),
        native.digest
    );
    let package_store = oven_store::store::OvenStore::new(project.join("target/lib/oven/loafs"), *store.limits());
    for entry in &packaged.entries {
        entry.receipt.verify_identity()?;
        assert_eq!(entry.receipt.intent, packaged.receipt.intent);
        if let Some(base) = &entry.base_loaf_identity {
            let available = oven_rustc::loaf::resolve_compiler_owned_loaf_by_identity(&packaged.receipt, base)?;
            eprintln!(
                "lowering/{}: package base {base}, available {}",
                output.profile,
                available.is_some()
            );
            assert!(available.is_some(), "profile-specific package base is unavailable");
        }
        let selected = package_store.select_payloads_matching_for_execution(
            |header| {
                header.identity == entry.identity
                    && header.kind == entry.kind
                    && header.receipt_identity == entry.receipt.identity
                    && header.build_unit_identity == entry.receipt.build_unit_identity
                    && header.intent == entry.receipt.intent
            },
            |_, _| true,
        )?;
        assert_eq!(selected.len(), 1, "exact package entry must remain available");
    }
    Ok(())
}

/// Inspect the persisted lowering's complete reuse boundary without baking or changing its fixture inputs.
#[test]
#[ignore = "machine-local diagnostic of persisted fixture reuse authority"]
fn completed_lowering_reuse_authority_diagnostic() -> Result<(), Box<dyn std::error::Error>> {
    use incan_driver::build::output_selection::{
        baked_project_owner_identity, matching_baked_project_outputs_with_source_authority,
    };
    use incan_driver::build::source_authority::{
        baked_project_lock_dependencies_fingerprint, digest_baked_project_source_authority, project_bake_receipt_path,
    };
    use incan_driver::build::{OvenBakeProjectTarget, OvenPackagedLibraryLoafManifest};
    use oven_rustc::rustc::{OvenProjectInspectionConstituent, load_project_inspection_authority};
    use oven_store::store::{OvenStore, OvenStoreLimits};

    let root = support::explicit_bake_workspace().ok_or("diagnostic requires the persisted fixture workspace")?;
    let project = root.join("lowering");
    let entrypoint = project.join("src/lib.incn");
    let source = digest_baked_project_source_authority(&project)?;
    let lock = baked_project_lock_dependencies_fingerprint(&project)?;
    let compiler = oven_store::digest_bytes(&fs::read(support::incan_debug_binary())?);
    let store = OvenStore::new(
        root.join("home/oven/store/v2"),
        OvenStoreLimits::new(
            oven_store::DEFAULT_OVEN_MAX_PHYSICAL_BYTES,
            oven_store::DEFAULT_OVEN_MAX_DOMAIN_PHYSICAL_BYTES,
            oven_store::DEFAULT_OVEN_MAX_DOMAIN_LOGICAL_BYTES,
        ),
    );
    let package: OvenPackagedLibraryLoafManifest =
        serde_json::from_slice(&fs::read(project.join("target/lib/oven/package-loafs.json"))?)?;
    assert_eq!(package.profiles.len(), 2);
    assert_eq!(package.compiler_version, incan_lang::version::INCAN_VERSION);
    let mut inspection_ref = None;
    for profile in ["debug", "release"] {
        let receipt: oven_store::OvenReceipt = serde_json::from_slice(&fs::read(project_bake_receipt_path(
            &project,
            OvenBakeProjectTarget::Library,
            &entrypoint,
            profile,
        )?)?)?;
        receipt.verify_identity()?;
        let candidates = matching_baked_project_outputs_with_source_authority(
            &store,
            &project,
            &entrypoint,
            OvenBakeProjectTarget::Library,
            profile,
            &source,
            Some((&receipt.intent.target, &receipt.intent.toolchain)),
        )?;
        eprintln!(
            "lowering/{profile}: source {source}; lock {lock:?}; {} verified candidates",
            candidates.len()
        );
        let output = candidates
            .into_iter()
            .find(|output| {
                output.payload.compiler_identity_digest.as_deref() == Some(&compiler)
                    && output.payload.lock_dependencies_fingerprint == lock
            })
            .ok_or("no exact source/compiler/lock candidate")?;
        assert_eq!(receipt.identity, output.payload.receipt_identity);
        assert_eq!(receipt.build_unit_identity, output.payload.build_unit_identity);
        let reference = output
            .payload
            .inspection_authority
            .as_ref()
            .ok_or("no inspection authority")?;
        if let Some(previous) = &inspection_ref {
            assert_eq!(previous, reference, "both profiles must share one inspection authority");
        } else {
            inspection_ref = Some(reference.clone());
        }
        let authority = load_project_inspection_authority(
            &store,
            reference,
            &baked_project_owner_identity(&project)?,
            &source,
            incan_lang::version::INCAN_VERSION,
        )?;
        for constituent in &authority.payload.constituents {
            if let OvenProjectInspectionConstituent::ReleaseLoaf {
                loaf_identity, receipt, ..
            } = constituent
            {
                let available = oven_rustc::loaf::resolve_compiler_owned_loaf_by_identity(receipt, loaf_identity)?;
                eprintln!(
                    "lowering/{profile}: release base {loaf_identity}, available {}",
                    available.is_some()
                );
                assert!(available.is_some(), "recorded release base is unavailable");
            }
        }
        incan_driver::lock::registry_sources::prepare_project_registry_source_authorities(authority)?;
        assert_eq!(package.source_authority_digest, source);
        let packaged = package.profiles.get(profile).ok_or("missing packaged profile")?;
        diagnose_fixture_package(&project, packaged, &output, &store)?;
        eprintln!("lowering/{profile}: inspection and package boundaries validated");
    }
    bake_with_reuse_requirement(&project, &root.join("home"), true)?;
    Ok(())
}

/// Prove concrete class and model trait methods and a static trait default against legacy.
#[test]
fn source_trait_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_source_trait(
        &fixture.driver_binary("debug"),
        &fixture.scratch("source-trait")?,
        &fixture.sysroot,
        &fixture.formatting,
    )
}

/// Compare hashed collection literals, mutation, membership, and indexed reads with legacy.
#[test]
fn direct_route_collections_match_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_collections(
        &fixture.driver_binary("debug"),
        &fixture.scratch("collections")?,
        &fixture.sysroot,
        &fixture.formatting,
    )?;
    Ok(())
}

/// Compare deferred creation, ordered yields, collection, mutable polling, and exhaustion with legacy.
#[test]
fn direct_route_generators_match_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let root = fixture.scratch("generators")?;
    let source = root.join("generators.incn");
    fs::write(
        &source,
        r#"def mark(value: int) -> int:
    println(value)
    return value

def parameterized(start: int, end: int) -> Generator[int]:
    println(start)
    for value in range(start, end):
        yield value

def labels(prefix: str) -> Generator[str]:
    println(prefix)
    yield prefix

def counter() -> Generator[int]:
    println(10)
    for value in range(1, 3):
        yield value
    yield 3

def counter__producer() -> int:
    return 40

def first(mut values: Generator[int]) -> int:
    for value in values:
        return value
    return -1

def outer(mut values: Generator[int]) -> int:
    return first(values)

def main() -> None:
    unused = counter()
    values = counter()
    println(20)
    collected = values.collect()
    println(collected[0])
    println(collected[1])
    println(collected[2])
    mut remaining = counter()
    println(first(remaining))
    println(outer(remaining))
    println(first(remaining))
    println(first(remaining))
    println(counter__producer())
    unused_parameters = parameterized(90, 92)
    pending = parameterized(mark(4), mark(6))
    println(50)
    for value in pending:
        println(value)
    mut prefix = "captured"
    texts = labels(prefix)
    prefix = "changed"
    println(prefix)
    println(60)
    for text in texts:
        println(text)
"#,
    )?;
    let native = root.join("native");
    success(
        &corpus::source_command(
            &fixture.driver_binary("debug"),
            &source,
            &native,
            &fixture.sysroot,
            &corpus::runtime_closure(&fixture.formatting)?,
        )
        .output()?,
        "generator native compilation",
    );
    let legacy = root.join("legacy");
    success(
        &support::repo_command()
            .current_dir(&root)
            .arg("build")
            .arg(&source)
            .arg(&legacy)
            .output()?,
        "generator legacy compilation",
    );
    let expected = Command::new(legacy.join("oven/release/generators")).output()?;
    let actual = Command::new(native).output()?;
    success(&expected, "generator legacy execution");
    success(&actual, "generator native execution");
    assert_eq!(
        actual.stdout, expected.stdout,
        "generator output must be byte-identical"
    );
    assert_eq!(
        actual.stderr, expected.stderr,
        "generator stderr must be byte-identical"
    );
    assert_eq!(
        actual.stdout,
        b"20\n10\n1\n2\n3\n10\n1\n2\n3\n-1\n40\n4\n6\n50\n4\n4\n5\nchanged\n60\ncaptured\ncaptured\n"
    );
    let named = root.join("named-generator.incn");
    fs::write(
        &named,
        "def mark(value: int) -> int:\n    println(value)\n    return value\n\ndef values(first: int, second: int) -> Generator[int]:\n    yield first\n    yield second\n\ndef main() -> None:\n    pending = values(second=mark(2), first=mark(1))\n    for value in pending:\n        println(value)\n",
    )?;
    let rejected = corpus::source_command(
        &fixture.driver_binary("debug"),
        &named,
        &root.join("named-native"),
        &fixture.sysroot,
        &corpus::runtime_closure(&fixture.formatting)?,
    )
    .output()?;
    assert!(!rejected.status.success());
    assert!(
        String::from_utf8_lossy(&rejected.stderr).contains("unsupported Body IR reordered generator ArgumentBinding")
    );
    Ok(())
}

/// Compare checked integer and float widening at the generator yield boundary with legacy.
#[test]
fn direct_route_generator_numeric_yields_match_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("generator-numeric-yields")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"def integers(value: i8) -> Generator[int]:
    """Yield a narrow integer through the checked int destination."""
    yield value

def floats(value: f32) -> Generator[float]:
    """Yield a narrow float through the checked float destination."""
    yield value

def main() -> None:
    """Observe both numeric generator yield conversions."""
    small: i8 = 7
    single: f32 = 1.5
    for value in integers(small):
        println(value)
    for value in floats(single):
        println(value)
"#,
    )?;
    Ok(())
}

/// Prove range values byte-identical to legacy: a range evaluates its bounds once, in order, even when unused;
/// exclusive, inclusive and empty ranges bound to a local and copied to another are iterated; an inclusive range ending
/// at the maximum integer stops there as a value, as a loop header and on a `continue` edge; and a range reassigned on
/// one branch is iterated as assigned.
#[test]
fn range_value_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("range-value")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"def bound(value: int) -> int:
    println(value)
    return value

def main() -> None:
    unused = bound(1)..bound(4)
    r = 0..10
    alias = r
    mut total = 0
    for i in alias:
        total += i
    println(total)
    inclusive = 2..=bound(4)
    for j in inclusive:
        println(j)
    edge = (bound(9223372036854775806))..=bound(9223372036854775807)
    for m in edge:
        println(m)
    for n in (bound(9223372036854775806))..=bound(9223372036854775807):
        println(n)
    continued = bound(9223372036854775807)..=bound(9223372036854775807)
    for c in continued:
        println(c)
        continue
    empty = 5..2
    for e in empty:
        println(e)
    mut chosen = 0..2
    if total > 3:
        chosen = 5..7
    for k in chosen:
        println(k)
    println("done")
"#,
    )
}

/// Prove generator expressions byte-identical to legacy: a filtered expression over a builtin range collects its
/// accepted elements; an unconsumed expression never evaluates its element; an expression over a list draws each
/// element only when a loop polls it; and float, boolean, text and list-snapshot expressions collect in order.
#[test]
fn generator_expression_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("generator-expression")?,
        &fixture.sysroot,
        &fixture.formatting,
        r##"def note(value: int) -> int:
    println(f"note {value}")
    return value

def positives(values: list[int]) -> Generator[int]:
    return (note(x) for x in values if x > 0)

def scaled(limit: int) -> list[int]:
    return (value * 10 for value in range(1, limit) if value > 2).collect()

def main() -> None:
    collected = (value * 10 for value in range(1, 5) if value > 2).collect()
    println(collected[0] + collected[1])
    unused = (value // 0 for value in range(1, 2))
    lazy = positives([-1, 2, 3])
    println("constructed")
    for item in lazy:
        println(item)
    println(len(scaled(6)))
    halves = (x * 0.5 for x in [1.0, 3.0]).collect()
    println(halves[1])
    flipped = (not flag for flag in [true, false]).collect()
    println(flipped[0])
    stepped = (v for v in range(10, 0, -3)).collect()
    println(len(stepped))
    labels = (f"#{v}" for v in range(2)).collect()
    println(labels[1])
    values = [4, 5]
    doubled = (v * 2 for v in values).collect()
    println(doubled[0] + values[1])
"##,
    )
}

/// Prove the lazy generator adapters byte-identical to legacy: `map` and `filter` through annotated function values
/// and through function names, over a generator expression and a yield-based generator; `take` drawing only the items
/// it keeps, including a negative count; and `map` running its function only when the result is consumed.
#[test]
fn generator_adapter_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("generator-adapter")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"def double(value: int) -> int:
    return value * 2

def is_even(value: int) -> bool:
    return value % 2 == 0

def halve(value: float) -> float:
    return value / 2.0

def numbers(limit: int) -> Generator[int]:
    for value in range(limit):
        yield value

def note(value: int) -> int:
    println(f"note {value}")
    return value

def main() -> None:
    increment: (int) -> int = (value) => value + 1
    accepted: (int) -> bool = (value) => value > 2
    values = (value for value in range(1, 5)).map(increment).filter(accepted).collect()
    println(values[0] * 10 + values[1])
    println(len(numbers(6).map(double).filter(is_even).collect()))
    println(len(numbers(6).take(2).collect()))
    lazy = (note(v) for v in [1, 2, 3, 4]).take(2)
    println("built")
    for item in lazy:
        println(item)
    halves = (x for x in [1.0, 3.0]).map(halve).collect()
    println(halves[1])
    println(len(numbers(3).take(-1).collect()))
    mapped = (v for v in [1, 2]).map(note)
    println("mapped")
    println(mapped.collect()[1])
"#,
    )
}

/// Prove the generator consumers byte-identical to legacy: `count`, `sum`, `fold`, `any`, `all`, `find` and `for_each`
/// draining yield-based generators, generator expressions and generators chained through `map` and `filter`, including
/// an empty generator and a `find` with no match; and `any` and `all` drawing only the items they need.
#[test]
fn generator_consumer_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("generator-consumer")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"def numbers(limit: int) -> Generator[int]:
    for value in range(limit):
        yield value

def is_even(n: int) -> bool:
    return n % 2 == 0

def is_odd(n: int) -> bool:
    return n % 2 == 1

def add(acc: int, n: int) -> int:
    return acc + n

def double(n: int) -> int:
    return n * 2

def show(n: int) -> None:
    println(n)

def note(value: int) -> int:
    println(f"note {value}")
    return value

def main() -> None:
    println(numbers(4).count())
    println(numbers(4).sum())
    println(numbers(4).fold(10, add))
    println(numbers(4).any(is_even))
    println(numbers(4).all(is_even))
    match numbers(4).find(is_odd):
        Some(value) => println(value)
        None => println("none")
    match numbers(1).find(is_odd):
        Some(value) => println(value)
        None => println("none")
    numbers(2).for_each(show)
    println((n * 2 for n in [1, 2, 3]).sum())
    println((note(v) for v in [1, 2, 3, 4]).any(is_even))
    println((note(v) for v in [1, 3, 4, 5]).all(is_odd))
    println(numbers(3).map(double).count())
    println(numbers(5).filter(is_even).sum())
    println(numbers(0).count())
    println((v for v in range(3)).fold(1, add))
"#,
    )
}

/// Prove the iterator surface byte-identical to legacy: a list's `iter()` leaves the list readable, and the `map`,
/// `filter`, `take`, `skip`, `take_while`, `skip_while`, `chain`, `flat_map` and `batch` adapters and the `count`,
/// `sum`, `fold`, `reduce`, `any`, `all`, `find` and `for_each` consumers called on the `Iterator` values it and a
/// generator's adapters return draw their items lazily; an `Iterator[int]` parameter takes either, a loop draws one,
/// and an indexed batch has a length.
#[test]
fn iterator_surface_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("iterator-surface")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"def numbers(limit: int) -> Generator[int]:
    for value in range(limit):
        yield value

def double(n: int) -> int:
    return n * 2

def is_even(n: int) -> bool:
    return n % 2 == 0

def small(n: int) -> bool:
    return n < 3

def twice(n: int) -> list[int]:
    return [n, n]

def add(acc: int, n: int) -> int:
    return acc + n

def show(n: int) -> None:
    println(n)

def note(n: int) -> int:
    println(f"note {n}")
    return n

def total(items: Iterator[int]) -> int:
    mut sum = 0
    for item in items:
        sum += item
    return sum

def render(items: list[int]) -> str:
    return " ".join([str(item) for item in items])

def main() -> None:
    xs: list[int] = [1, 2, 3, 4, 5]
    println(render(xs.iter().map(double).collect()))
    println(render(xs.iter().filter(is_even).collect()))
    println(render(xs.iter().skip(2).take(2).collect()))
    println(render(xs.iter().take_while(small).collect()))
    println(render(xs.iter().skip_while(small).collect()))
    println(render(xs.iter().chain([9, 8].iter()).collect()))
    println(render(xs.iter().take(2).flat_map(twice).collect()))
    batches: list[list[int]] = xs.iter().batch(2).collect()
    println(f"{len(batches)} {len(batches[2])}")
    println(xs.iter().count())
    println(xs.iter().sum())
    println(xs.iter().fold(10, add))
    println(xs.iter().reduce(0, add))
    println(xs.iter().any(is_even))
    println(xs.iter().all(is_even))
    match xs.iter().find(is_even):
        Some(found) => println(found)
        None => println("none")
    xs.iter().take(2).for_each(show)
    println(total(xs.iter()))
    println(total(numbers(4).skip(1)))
    println(render(numbers(6).skip(1).take_while(small).collect()))
    println(render(numbers(3).chain(numbers(2)).collect()))
    println(xs[0])
    println(render(xs.iter().map(note).take(2).collect()))
    for value in xs.iter().skip(3):
        println(value)
"#,
    )
}

/// Prove pair iterators byte-identical to legacy: `enumerate` and `zip` called on iterators and generators, and builtin
/// `zip` over two `Iterator` parameters, pair their items lazily for loops, list and dictionary comprehensions and
/// `collect`, stopping at the shorter source.
#[test]
fn iterator_pair_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("iterator-pairs")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"def dot(left: Iterator[float], right: Iterator[float]) -> float:
    mut score = 0.0
    for pair in zip(left, right):
        score += pair[0] * pair[1]
    return score

def numbers(limit: int) -> Generator[int]:
    for value in range(limit):
        yield value

def main() -> None:
    words = ["a", "b", "c"]
    mut seen: list[str] = []
    for index, word in words.iter().take(2).enumerate():
        seen.append(f"{index}:{word}")
    println(" ".join(seen))
    mut zipped: list[str] = []
    for left, right in [1, 2, 3].iter().zip(numbers(2)):
        zipped.append(f"{left}-{right}")
    println(" ".join(zipped))
    pairs: list[tuple[int, int]] = numbers(3).enumerate().collect()
    println(len(pairs))
    println(" ".join([f"{i}{w}" for i, w in words.iter().enumerate()]))
    indexed = {i: w for i, w in words.iter().enumerate()}
    println(indexed[1])
    println(dot([1.0, 2.0].iter(), [3.0, 4.0].iter()))
    println(len(words.iter().zip(numbers(5)).collect()))
"#,
    )
}

/// Prove capturing generator expressions byte-identical to legacy: an element and a filter read captured integers,
/// text and lists, which keep the value they had when the expression was built; text items, whether from a list or
/// from an iterator, are expanded through the same lazy chain; and `flat_map` over a function returning such a
/// generator draws each expansion one item at a time.
#[test]
fn capturing_generator_expression_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("capturing-generator-expression")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"static produced: list[int] = []

def note(n: int) -> int:
    produced.append(n)
    println(f"note {n}")
    return n

def scaled(xs: list[int], factor: int) -> Generator[int]:
    return (x * factor for x in xs if x > 1)

def labeled(words: list[str], prefix: str) -> Generator[str]:
    return (f"{prefix}{w}" for w in words if w != prefix)

def offsets(xs: list[int], extra: list[int]) -> Generator[int]:
    return (x + len(extra) + extra[0] for x in xs)

def tracked(n: int) -> Generator[int]:
    return (note(n * 10 + step) for step in [0, 1, 2])

def main() -> None:
    a: list[int] = scaled([1, 2, 3], 3).collect()
    println(f"{a[0]} {a[1]} {len(a)}")
    b: list[str] = labeled(["a", ">", "b"], ">").collect()
    println(" ".join(b))
    extra = [7, 8, 9]
    c: list[int] = offsets([1, 2, 3], extra).collect()
    println(" ".join([str(v) for v in c]))
    println(len(extra))
    words = ["a", "b", "c"]
    d: list[str] = (w for w in words.iter().skip(1)).collect()
    println(" ".join(d))
    upper: list[str] = (w.upper() for w in words if w != "b").collect()
    println(" ".join(upper))
    mut limit = 1
    e = (note(v) for v in [1, 2, 3] if v > limit)
    limit = 5
    println(limit)
    first: list[int] = e.take(1).collect()
    println(first[0])
    taken: list[int] = [1, 2, 3].iter().flat_map(tracked).take(1).collect()
    println(f"{taken[0]} {len(produced)}")
    crossing: list[int] = [4, 5].iter().flat_map(tracked).take(4).collect()
    println(f"{crossing[3]} {len(produced)}")
"#,
    )
}

/// Prove generator expressions with later `for` clauses byte-identical to legacy: a later clause reads the earlier
/// clauses' items, captured lists and its own filters; clauses nest three deep, draw from a list or a builtin range the
/// earlier item bounds, and yield text; and taking a prefix runs each element only as it is drawn.
#[test]
fn later_clause_generator_expression_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("later-clause-generator-expression")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"def note(n: int) -> int:
    println(f"note {n}")
    return n

def positives(xs: list[int], ys: list[int]) -> Generator[int]:
    return (x * y for x in xs if x > 0 for y in ys if y > x)

def main() -> None:
    values: list[int] = positives([1, 2, 3], [2, 3, 4]).take(3).collect()
    println(f"{values[0]} {values[1]} {values[2]}")
    println(len(positives([1, 2, 3], [2, 3, 4]).collect()))
    sums: list[int] = (x + y + z for x in [100] for y in [10, 20] for z in [1, 2]).collect()
    println(" ".join([str(v) for v in sums]))
    ranged: list[int] = (y for x in [1, 2, 3] for y in range(x)).collect()
    println(" ".join([str(v) for v in ranged]))
    drawn: list[int] = (note(x * 10 + y) for x in [1, 2] for y in [0, 1]).take(3).collect()
    println(len(drawn))
    words = ["a", "b"]
    suffixes = ["x", "y"]
    joined: list[str] = (w + s for w in words for s in suffixes if s != w).collect()
    println(" ".join(joined))
"#,
    )
}

/// Generator expressions over pair items: the first source is builtin `enumerate` over a list of numbers or text, or
/// builtin `zip` over two lists, read through tuple fields or destructured, filtered on either half and drawn lazily by
/// a prefix. Legacy's emitted Rust does not build for these (its iterator borrows a temporary list for `'static`), as
/// the pending `refuses_corrupted_enumerate_*` fixtures record, so the output is pinned to the values they record.
#[test]
fn pair_item_generator_expression_output() -> Result<(), Box<dyn std::error::Error>> {
    check_native_case(
        "pair_item_generator_expression",
        r#"def main() -> None:
    sums: list[int] = (pair.0 + pair.1 for pair in enumerate([5, 6, 7])).collect()
    println(sums)
    labels: list[str] = (f"{i}:{s}" for i, s in enumerate(["a", "b", "c"])).collect()
    println(labels)
    later: list[str] = (s.upper() for i, s in enumerate(["a", "b", "c"]) if i > 0).collect()
    println(later)
    products: list[int] = (l * r for l, r in zip([1, 2, 3], [4, 5])).collect()
    println(products)
    named: list[str] = (f"{n}={v}" for n, v in zip(["x", "y"], [1, 2]) if v > 1).collect()
    println(named)
    first: list[int] = (i * 10 for i, _ in enumerate([9, 9, 9])).take(2).collect()
    println(first)
"#,
        b"[5, 7, 9]\n[\"0:a\", \"1:b\", \"2:c\"]\n[\"B\", \"C\"]\n[4, 10]\n[\"y=2\"]\n[0, 10]\n",
    )
}

/// A generator expression over pair items also runs inside a closure and inside a parameter's deferred default, as the
/// pending `refuses_corrupted_enumerate_*` and `executes_deferred_enumerate_and_zip_frames_with_distinct_bindings`
/// fixtures record; legacy's emitted Rust does not build for them either.
#[test]
fn generator_expressions_in_closures_and_defaults_output() -> Result<(), Box<dyn std::error::Error>> {
    check_native_case(
        "generator_expressions_in_closures_and_defaults",
        r#"def defaulted(values: list[int] = (pair.1 for pair in enumerate([7, 8])).collect()) -> int:
    return values[0] + values[1]


def zipped(values: list[int] = (l + r for l, r in zip([1], [2])).collect()) -> int:
    return values[0]


def main() -> None:
    firsts: () -> list[int] = () => (pair.0 for pair in enumerate([8, 9])).collect()
    println(firsts())
    sums: () -> list[int] = () => (l + r for l, r in zip([2, 4], [3, 5])).collect()
    println(sums())
    println(defaulted())
    println(defaulted([1, 2]))
    println(zipped())
"#,
        b"[0, 1]\n[5, 9]\n15\n3\n3\n",
    )
}

/// A generator expression step over pair items that captures a value would cross a boxed environment's call boundary
/// with a tuple input, which that boundary does not carry, so the direct route refuses it by name.
#[test]
fn capturing_pair_item_generator_step_is_refused() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let root = fixture.scratch("capturing-pair-item-generator-step")?;
    let source = root.join("pairs.incn");
    fs::write(
        &source,
        "def main() -> None:\n    n = 5\n    picked = (i for i, v in enumerate([5, 6, 7]) if v > n).collect()\n    println(len(picked))\n",
    )?;
    let closure = corpus::runtime_closure(&fixture.formatting)?;
    let binary = root.join("native");
    let refused = corpus::source_command(
        &fixture.driver_binary("debug"),
        &source,
        &binary,
        &fixture.sysroot,
        &closure,
    )
    .output()?;
    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stderr)
            .contains("unsupported Body IR capturing generator expression step over tuple items"),
        "{}",
        String::from_utf8_lossy(&refused.stderr)
    );
    assert!(!binary.exists());
    Ok(())
}

/// Prove collections held by other collections byte-identical to legacy: dictionaries whose values are lists or
/// dictionaries, a list of dictionaries and a model field holding a dictionary of lists are built, passed and read by
/// key and index, through nested lookups, without consuming their sources.
#[test]
fn nested_collection_value_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("nested-collection-values")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"model Bag:
    tags: dict[str, list[int]]


def count(groups: dict[str, list[int]], key: str) -> int:
    return len(groups[key])


def main() -> None:
    groups: dict[str, list[int]] = {"a": [1, 2], "b": [3]}
    println(count(groups, "a"))
    println(groups["b"][0])
    first: list[int] = groups["a"]
    println(first[1])
    nested: dict[str, dict[str, int]] = {"x": {"y": 5}, "z": {"w": 6}}
    println(nested["x"]["y"])
    inner: dict[str, int] = nested["z"]
    println(inner["w"])
    rows: list[dict[str, int]] = [{"a": 1}, {"a": 2}]
    println(rows[1]["a"])
    bag = Bag(tags={"k": [7, 8, 9]})
    println(len(bag.tags["k"]))
    println(groups["a"])
    println(len(groups))
    println(len(nested["x"]))
"#,
    )
}

/// Prove set, dictionary and value enum display byte-identical to legacy: `print`, `str` and an f-string spell a set,
/// a dictionary, a dictionary of lists and a list of dictionaries through their structure, and a text or integer value
/// enum as its raw value.
#[test]
fn collection_and_value_enum_display_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("collection-and-value-enum-display")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"pub enum Env(str):
    Dev = "development"
    Prod = "production"


pub enum Code(int):
    Ok = 200
    Missing = 404


def main() -> None:
    counts: dict[str, int] = {"a": 1}
    println(counts)
    println(str(counts))
    println(f"{counts}")
    tags: set[int] = {3}
    println(tags)
    println({1: 2})
    println({1})
    groups: dict[str, list[int]] = {"k": [1, 2]}
    println(groups)
    rows: list[dict[str, int]] = [{"a": 1}]
    println(rows)
    println(str(Env.Dev))
    println(f"{Env.Prod}")
    println(str(Code.Missing))
    println(f"{Code.Ok}!")
    env = Env.Prod
    println(env)
    println(f"x {env}")
"#,
    )
}

/// Prove the generated enum helpers byte-identical to legacy: `message()` spells the variant name of a value enum, a
/// unit variant and a payload variant, and `from_value()` finds a text or integer value enum's variant by its raw value
/// from a literal or a local, or reports `None`.
#[test]
fn enum_message_and_from_value_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("enum-message-and-from-value")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"pub enum Env(str):
    Dev = "development"
    Prod = "production"


pub enum Code(int):
    Ok = 200
    NotFound = 404


enum State:
    Ready
    Busy
    Pair(int, str)


def main() -> None:
    println(Env.Dev.message())
    state = State.Busy
    println(state.message())
    println(State.Pair(1, "x").message())
    match Env.from_value("production"):
        Some(env) => println(f"parsed {env.message()}")
        None => println("unknown")
    match Env.from_value("staging"):
        Some(env) => println(f"parsed {env.message()}")
        None => println("unknown")
    match Code.from_value(404):
        Some(code) => println(f"parsed {code.message()}")
        None => println("unknown")
    match Code.from_value(500):
        Some(code) => println(f"parsed {code.message()}")
        None => println("unknown")
    key = "development"
    match Env.from_value(key):
        Some(env) => println(env.value())
        None => println("unknown")
"#,
    )
}

/// Prove unit and payload construction, enum passing/returning, and variant-bound match output against legacy.
#[test]
fn source_enum_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let root = fixture.scratch("source-enum")?;
    let source = root.join("source_enum.incn");
    fs::write(
        &source,
        r#"enum Signal:
    Ready
    Waiting

enum Shape:
    Empty
    Circle(int)
    Rectangle(int, int)
    Label(str)

def make_shape(size: int) -> Shape:
    return Shape.Circle(size)

def area(shape: Shape) -> int:
    match shape:
        Shape.Empty => return 0
        Shape.Circle(radius) => return radius * radius
        Shape.Rectangle(width, height) => return width * height
        Shape.Label(_) => return -1

def label(shape: Shape) -> str:
    match shape:
        Shape.Label(text) => return text
        _ => return "unlabeled"

def signal_value(signal: Signal) -> int:
    match signal:
        Signal.Ready => return 1
        Signal.Waiting => return 2

def main() -> None:
    shape = make_shape(7)
    println(area(shape))
    println(area(shape))
    println(area(Shape.Empty))
    println(area(Shape.Rectangle(3, 5)))
    println(signal_value(Signal.Ready))
    println(signal_value(Signal.Waiting))
    text = Shape.Label("payload")
    println(label(text))
    println(label(text))
"#,
    )?;
    let closure = corpus::runtime_closure(&fixture.formatting)?;
    let native = root.join("native");
    success(
        &corpus::source_command(
            &fixture.driver_binary("debug"),
            &source,
            &native,
            &fixture.sysroot,
            &closure,
        )
        .output()?,
        "source enum native compilation",
    );
    let legacy = root.join("legacy");
    success(
        &support::repo_command()
            .current_dir(&root)
            .arg("build")
            .arg(&source)
            .arg(&legacy)
            .output()?,
        "source enum legacy compilation",
    );
    let expected = Command::new(legacy.join("oven/release/source_enum")).output()?;
    let actual = Command::new(native).output()?;
    success(&expected, "source enum legacy execution");
    success(&actual, "source enum native execution");
    assert_eq!(actual.stdout, expected.stdout);
    assert_eq!(actual.stdout, b"49\n49\n0\n15\n1\n2\npayload\npayload\n");
    Ok(())
}

/// Ordinary newtype builders and receiver methods preserve their wrapped values and legacy output.
#[test]
fn newtype_methods_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let root = fixture.scratch("newtype-methods")?;
    let source = root.join("newtype_methods.incn");
    fs::write(
        &source,
        r#"trait Doubled:
    """Expose a shared read of a wrapped count."""

    def doubled(self) -> int

type Count = newtype int with Doubled:
    """Wrap a count without introducing checked construction."""

    def create(value: int) -> Count:
        """Construct through an ordinary associated builder."""
        return Count(value)

    def doubled(self) -> int:
        """Read tuple slot zero through a nominal receiver."""
        return self.0 * 2

type Message = newtype str:
    """Keep owned text behind a nominal wrapper."""

    def create(value: str) -> Message:
        """Wrap the supplied text through an associated method."""
        return Message(value)

    def text(self) -> str:
        """Read the wrapped text without consuming the receiver."""
        return self.0

def main() -> None:
    """Exercise builders, scalar projection, and repeated text reads."""
    count = Count.create(21)
    println(count.doubled())
    println(count.0)
    message = Message.create("wrapped")
    println(message.text())
    println(message.text())
"#,
    )?;
    let closure = corpus::runtime_closure(&fixture.formatting)?;
    let native = root.join("native");
    success(
        &corpus::source_command(
            &fixture.driver_binary("debug"),
            &source,
            &native,
            &fixture.sysroot,
            &closure,
        )
        .output()?,
        "newtype methods native compilation",
    );
    let legacy = root.join("legacy");
    success(
        &support::repo_command()
            .current_dir(&root)
            .arg("build")
            .arg(&source)
            .arg(&legacy)
            .output()?,
        "newtype methods legacy compilation",
    );
    let expected = Command::new(legacy.join("oven/release/newtype_methods")).output()?;
    let actual = Command::new(native).output()?;
    success(&expected, "newtype methods legacy execution");
    success(&actual, "newtype methods native execution");
    assert_eq!(actual.stdout, expected.stdout);
    assert_eq!(actual.stdout, b"42\n21\nwrapped\nwrapped\n");
    let checked_source = root.join("checked_newtype.incn");
    fs::write(
        &checked_source,
        "type Positive = newtype int:\n    def from_underlying(value: int) -> Result[Self, ValidationError]:\n        if value <= 0:\n            return Err(ValidationError(\"must be positive\"))\n        return Ok(Positive(value))\n\ndef main() -> None:\n    value = Positive(1)\n",
    )?;
    let checked_output = root.join("checked-native");
    let refused = corpus::source_command(
        &fixture.driver_binary("debug"),
        &checked_source,
        &checked_output,
        &fixture.sysroot,
        &closure,
    )
    .output()?;
    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("unsupported source checked Newtype construction"),
        "{}",
        String::from_utf8_lossy(&refused.stderr)
    );
    assert!(!checked_output.exists());
    Ok(())
}

/// Fieldless enum equality and inequality borrow their values and compare exactly like the legacy route.
#[test]
fn fieldless_enum_comparisons_match_legacy() -> Result<(), Box<dyn std::error::Error>> {
    check_declaration_case(
        "fieldless_enum_comparisons",
        r#"enum Signal:
    Ready
    Waiting

enum HttpStatus(int):
    Ok = 200
    NotFound = 404

def same(left: Signal, right: Signal) -> bool:
    return left == right

def main() -> None:
    signal = Signal.Ready
    println(same(signal, Signal.Ready))
    println(signal == Signal.Waiting)
    println(signal != Signal.Waiting)
    println(signal != Signal.Ready)
    println(same(signal, signal))
    println(HttpStatus.Ok == HttpStatus.NotFound)
    println(HttpStatus.Ok != HttpStatus.NotFound)
    println(HttpStatus.Ok == HttpStatus.Ok)
"#,
        None,
    )?;
    let fixture = driver_fixture()?;
    let root = fixture.scratch("payload-enum-comparison-refusal")?;
    let source = root.join("payload.incn");
    fs::write(
        &source,
        "enum Shape:\n    Square(int)\n    Empty\n\ndef main() -> None:\n    println(Shape.Square(2) == Shape.Square(3))\n",
    )?;
    let closure = corpus::runtime_closure(&fixture.formatting)?;
    let binary = root.join("native");
    let refused = corpus::source_command(
        &fixture.driver_binary("debug"),
        &source,
        &binary,
        &fixture.sysroot,
        &closure,
    )
    .output()?;
    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("unsupported Body IR payload enum comparison"),
        "{}",
        String::from_utf8_lossy(&refused.stderr)
    );
    assert!(!binary.exists());
    Ok(())
}

/// Integer and string value enums retain canonical construction, parameter passing, and repeated scalar extraction.
#[test]
fn value_enum_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    check_declaration_case(
        "value_enum_getters",
        r#"enum HttpStatus(int):
    Ok = 200
    NotFound = 404

enum Env(str):
    Dev = "development"
    Prod = "production"

def status_code(status: HttpStatus) -> int:
    return status.value()

def environment_name(environment: Env) -> str:
    return environment.value()

def main() -> None:
    status = HttpStatus.NotFound
    println(status_code(status))
    println(status.value())
    println(HttpStatus.Ok.value())
    environment = Env.Prod
    println(environment_name(environment))
    println(environment.value())
    println(Env.Dev.value())
"#,
        None,
    )
}

/// String static reads preserve live aliases, detached bindings, assignments, and returned snapshots.
#[test]
fn string_statics_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    check_declaration_case(
        "string_statics",
        r#"static TEXT: str = "initial"

def read() -> str:
    return TEXT

def replace(value: str) -> None:
    TEXT = value

def main() -> None:
    first = read()
    live = TEXT
    mut changing = TEXT
    println(TEXT)
    println(read())
    replace("changed")
    println(live)
    println(changing)
    changing += "!"
    replace("final")
    println(live)
    println(changing)
    println(first)
    println(TEXT)
    println(read())
"#,
        None,
    )
}

/// Primitive list storage preserves live aliases, detached snapshots, and argument effects before mutations.
#[test]
fn list_statics_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    check_declaration_case("list_statics", corpus::LIST_STATICS_SOURCE, None)
}

/// Primitive and tuple patterns preserve literal tests, source arm order, and owned binding snapshots.
#[test]
fn structural_matches_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    check_declaration_case("structural_matches", corpus::STRUCTURAL_MATCHES_SOURCE, None)
}

/// Newtypes, erased aliases, scalar constants, and persistent scalar statics retain exactly the legacy output.
#[test]
fn declarations_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_declarations(
        &fixture.driver_binary("debug"),
        &fixture.scratch("declarations")?,
        &fixture.sysroot,
        &fixture.formatting,
    )
}
/// Builtin model derives retain the legacy declaration and shared-argument cloning behavior.
#[test]
fn model_derives_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    check_declaration_case(
        "model_derives",
        r#"@derive(Debug, Clone, Eq, Hash, Ord, Default)
model Point:
    x: int
    label: str

def copy(point: Point) -> Point:
    return point

def main() -> None:
    point = Point(x=3, label="derived\"\n")
    other = copy(point)
    later = Point(x=3, label="z")
    higher = Point(x=4, label="a")
    println(point.x)
    println(other.label)
    println(f"{point:?}")
    println(f"debug={point:?}")
    println(point == other)
    println(point != later)
    println(point < later)
    println(point <= other)
    println(higher > later)
    println(higher >= later)
    println(point.x)
"#,
        None,
    )
}

/// Field aliases share canonical storage across construction, reads, receiver methods, and writes.
#[test]
fn model_field_aliases_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    check_declaration_case(
        "model_field_aliases",
        r#"model AliasRecord:
    value [alias="wire_value"]: int
    label [alias="wire_label"]: str

    def read(self) -> int:
        return self.wire_value

def main() -> None:
    mut value = AliasRecord(wire_label="aliased", wire_value=7)
    println(value.value)
    println(value.wire_value)
    println(value.read())
    value.value = 9
    println(value.value)
    println(value.label)
    println(value.wire_label)
"#,
        None,
    )
}

/// Model field defaults execute for each omitted slot and preserve the legacy default effects and owned text.
#[test]
fn model_field_defaults_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    check_declaration_case(
        "model_field_defaults",
        r#"def default_number() -> int:
    println(100)
    return 7

def default_text() -> str:
    println(200)
    prefix = "default"
    return prefix + "!"

model Defaults:
    required: int
    number: int = default_number()
    label: str = default_text()
    flag: bool = true
    fraction: float = 2.5

def main() -> None:
    first = Defaults(required=1)
    second = Defaults(label="supplied", required=2, number=9)
    third = Defaults(required=3)
    println(first.required)
    println(first.number)
    println(first.label)
    println(first.flag)
    println(first.fraction)
    println(second.number)
    println(second.label)
    println(third.number)
    println(third.label)
"#,
        None,
    )
}

/// Scalar and string defaults execute at omitted calls, while supplied arguments bypass them.
#[test]
fn direct_route_defaults_match_legacy() -> Result<(), Box<dyn std::error::Error>> {
    check_declaration_case(
        "defaults",
        "def compute() -> int:\n    println(100)\n    return 7\n\ndef choose(value: int = compute()) -> int:\n    return value\n\ndef flag(value: bool = true) -> bool:\n    return value\n\ndef fraction(value: float = 2.5) -> float:\n    return value\n\ndef number(value: int = 2 + 3) -> int:\n    return value\n\ndef greeting() -> str:\n    prefix = \"hello\"\n    return prefix + \"!\"\n\ndef text(value: str = greeting()) -> str:\n    return value\n\ndef literal_text(value: str = \"literal\") -> str:\n    return value\n\ndef main() -> None:\n    println(choose(9))\n    println(choose())\n    println(flag())\n    println(fraction())\n    println(number())\n    println(number(9))\n    println(text())\n    println(text(\"supplied\"))\n    println(literal_text())\n",
        None,
    )?;
    check_declaration_case(
        "defaults_with_caller_collections",
        "class DefaultBox:\n    value: int\n    def get(self) -> int:\n        return self.value\n\ndef compute() -> int:\n    return 7\n\ndef method_number(value: int = DefaultBox(value=5).get()) -> int:\n    return value\n\ndef number(value: int = compute() + 2) -> int:\n    return value\n\ndef main() -> None:\n    first = {1}\n    second = {2}\n    println(number())\n    println(method_number())\n    println(len(first) + len(second))\n",
        None,
    )
}

/// Condition assertions preserve successful output, failure payloads, and exit codes.
#[test]
fn direct_route_assertions_match_legacy() -> Result<(), Box<dyn std::error::Error>> {
    for (name, source, message) in [
        (
            "assert_pass",
            "def message() -> str:\n    println(99)\n    return \"message\"\n\ndef main() -> None:\n    assert true, message()\n    assert 3 > 2, \"unused\"\n    println(42)\n",
            None,
        ),
        (
            "assert_fail",
            "def main() -> None:\n    assert false\n",
            Some("AssertionError"),
        ),
        (
            "assert_message",
            "def main() -> None:\n    assert false, \"failed check\"\n",
            Some("AssertionError: failed check"),
        ),
        (
            "assert_empty",
            "def main() -> None:\n    assert false, \"\"\n",
            Some("AssertionError"),
        ),
    ] {
        check_declaration_case(name, source, message)?;
    }
    Ok(())
}

/// Compare complete output streams and exit codes, including the canonical panic payload without Rust's wrapper.
fn check_declaration_case(
    name: &str,
    text: &str,
    panic_message: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    let (actual, expected) = declaration_case_outputs(name, text)?;
    if let Some(message) = panic_message {
        let expected_error = String::from_utf8_lossy(&expected.stderr);
        let actual_error = String::from_utf8_lossy(&actual.stderr);
        assert_eq!(
            expected_error.lines().find(|line| line.starts_with("AssertionError")),
            Some(message),
            "{expected_error}"
        );
        assert_eq!(
            actual_error.lines().find(|line| line.starts_with("AssertionError")),
            Some(message),
            "{actual_error}"
        );
    } else {
        success(&actual, "declaration execution");
    }
    Ok(())
}

/// Build one declaration on both routes, run both binaries, and require identical output streams and exit codes.
///
/// Returns the direct route's output first and legacy's second, for checks on what the shared streams contain.
fn declaration_case_outputs(
    name: &str,
    text: &str,
) -> Result<(std::process::Output, std::process::Output), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let root = fixture.scratch(name)?;
    let source = root.join(format!("{name}.incn"));
    fs::write(&source, text)?;
    let native = root.join("native");
    let closure = corpus::runtime_closure(&fixture.formatting)?;
    success(
        &corpus::source_command(
            &fixture.driver_binary("debug"),
            &source,
            &native,
            &fixture.sysroot,
            &closure,
        )
        .output()?,
        "native declaration compilation",
    );
    let legacy_root = root.join("legacy");
    success(
        &support::repo_command()
            .current_dir(&root)
            .arg("build")
            .arg(&source)
            .arg(&legacy_root)
            .output()?,
        "legacy declaration compilation",
    );
    let expected = Command::new(legacy_root.join("oven/release").join(name)).output()?;
    let actual = Command::new(native).output()?;
    assert_eq!(actual.stdout, expected.stdout);
    assert_eq!(actual.stderr, expected.stderr);
    assert_eq!(actual.status.code(), expected.status.code());
    Ok((actual, expected))
}

/// Source async calls defer discarded body effects and evaluate arguments before their immediately ready awaits.
#[test]
fn direct_route_async_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    check_declaration_case(
        "async_ready",
        r#"
import std.async

def argument() -> int:
    """Observe argument construction before polling."""
    println("argument")
    return 40

async def answer(value: int) -> int:
    """Observe deferred execution and return a ready value."""
    println(f"body={value}")
    return value + 1

async def nested(value: int) -> int:
    """Await one source future inside another."""
    return await answer(value)

async def main() -> None:
    """Discard a future and then evaluate nested ready awaits."""
    answer(999)
    println("constructed")
    println(await nested(argument()))
    println(await answer(7))
"#,
        None,
    )
}

/// Ready races construct every arm, poll in source order, run only the winner, and preserve first-poll failures.
#[test]
fn direct_route_ready_races_match_legacy() -> Result<(), Box<dyn std::error::Error>> {
    check_declaration_case(
        "ready_race",
        r#"
import std.async

def argument(value: int) -> int:
    """Observe construction of each race arm."""
    println(f"construct={value}")
    return value

async def answer(value: int) -> int:
    """Observe polling of the selected source future."""
    println(f"body={value}")
    return value + 1

async def select() -> int:
    """Run one block callback and leave the losing body and callback unexecuted."""
    winner = race for value:
        await answer(argument(2)) =>
            println(f"winner={value}")
            value * 10
        await answer(argument(1)) =>
            println("losing callback")
            value * 100
    return winner

async def main() -> None:
    """Await a ready source race."""
    println(await select())
"#,
        None,
    )?;
    check_declaration_case(
        "ready_race_failure",
        r#"
import std.async

def argument() -> int:
    """Show that the losing future is constructed before the first arm fails."""
    println("loser constructed")
    return 2

async def fail() -> int:
    """Fail on the first poll of the winning source future."""
    assert false, "first"
    return 1

async def loser(value: int) -> int:
    """Observe an incorrect poll of the losing future."""
    println("loser polled")
    return value

async def main() -> None:
    """Keep the first failure ahead of both callbacks and all later effects."""
    winner = race for value:
        await fail() => value
        await loser(argument()) => value
    println(winner)
"#,
        Some("AssertionError: first"),
    )
}

/// Prove union injection at assignment, argument and return boundaries against legacy output.
#[test]
fn source_union_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let root = fixture.scratch("source-union")?;
    let source = root.join("source_union.incn");
    fs::write(
        &source,
        r#"def choose(flag: bool) -> int | str:
    if flag:
        return 42
    return "payload"

def classify(value: int | str) -> int:
    if isinstance(value, int):
        return 1
    return 2

def narrowed(value: int | str) -> int:
    if isinstance(value, int):
        return value + 1
    return -1

def captured(value: int | str) -> str:
    match value:
        int(_) => return "number"
        str(text) => return text

def defaulted(value: int | str, step: int = 4) -> int:
    return classify(value) + step

def main() -> None:
    value: int | str = 7
    println(classify(value))
    println(classify(8))
    println(classify("text"))
    println(classify(choose(true)))
    println(classify(choose(false)))
    println(narrowed(41))
    println(narrowed("text"))
    println(captured(7))
    println(captured("payload"))
    println(defaulted(7))
    println(defaulted("text", 8))
"#,
    )?;
    let closure = corpus::runtime_closure(&fixture.formatting)?;
    let native = root.join("native");
    success(
        &corpus::source_command(
            &fixture.driver_binary("debug"),
            &source,
            &native,
            &fixture.sysroot,
            &closure,
        )
        .output()?,
        "source union native compilation",
    );
    let legacy = root.join("legacy");
    success(
        &support::repo_command()
            .current_dir(&root)
            .arg("build")
            .arg(&source)
            .arg(&legacy)
            .output()?,
        "source union legacy compilation",
    );
    let expected = Command::new(legacy.join("oven/release/source_union")).output()?;
    let actual = Command::new(native).output()?;
    success(&expected, "source union legacy execution");
    success(&actual, "source union native execution");
    assert_eq!(actual.stdout, expected.stdout);
    assert_eq!(actual.stdout, b"1\n1\n2\n1\n2\n42\n-1\nnumber\npayload\n5\n10\n");

    // ---- Admitted list payloads preserve their values at the same boundary ----
    assert_native_legacy_bytes(
        "source_union_list_payload",
        "def accept(value: int | list[int]) -> int:\n    if isinstance(value, int):\n        return 1\n    return 2\n\ndef main() -> None:\n    values = [1, 2]\n    println(accept(7))\n    println(accept(values))\n    println(len(values))\n",
        b"1\n2\n2\n",
    )?;

    // ---- Unadmitted tuple payload types stay a named refusal ----
    let refused_source = root.join("unadmitted_union.incn");
    fs::write(
        &refused_source,
        "def accept(value: int | tuple[int, int]) -> None:\n    pass\n\ndef main() -> None:\n    accept(7)\n",
    )?;
    let refused_binary = root.join("refused-union");
    let refusal = corpus::source_command(
        &fixture.driver_binary("debug"),
        &refused_source,
        &refused_binary,
        &fixture.sysroot,
        &closure,
    )
    .output()?;
    assert!(!refusal.status.success());
    assert!(
        String::from_utf8_lossy(&refusal.stderr).contains("unsupported Body IR union member"),
        "{}",
        String::from_utf8_lossy(&refusal.stderr)
    );
    assert!(!refused_binary.exists());
    Ok(())
}

/// Prove admitted string methods and Unicode lengths against legacy output.
#[test]
fn direct_route_string_methods_match_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_string_methods(
        &fixture.driver_binary("debug"),
        &fixture.scratch("string-methods")?,
        &fixture.sysroot,
        &fixture.formatting,
    )
}

/// Compare booleans with legacy, including evaluation order and retained owners.
#[test]
fn direct_route_booleans_match_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("booleans")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"def probe(label: str, value: bool) -> bool:
    println(label)
    return value

def main() -> None:
    println(false and probe("skipped-and", true))
    println(true or probe("skipped-or", false))
    println(true and probe("selected-and", true))
    println(false or probe("selected-or", true))
    println(not false)
"#,
    )
}

/// Compare builtins with legacy, including evaluation order and retained owners.
#[test]
fn direct_route_builtins_match_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("builtins")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"def main() -> None:
    values = [3, -2, 1]
    ordered = sorted(values)
    println(ordered[0])
    println(ordered[2])
    println(sum(values))
    println(abs(-12))
    println(2 ** 10)
    println(2.0 ** 3.0)
    small: f32 = 1.1
    exponent: f32 = 2.0
    println(small ** exponent)
    println(bool(0))
    println(bool(-2))
    println(bool(0.0))
    println(bool(""))
    println(bool("x"))
    println(bool(values))
    merged = values + [8, 9]
    println(len(merged))
    println(len(values))
"#,
    )
}

/// A helper re-export and a qualified call retain their canonical bodies and byte-identical legacy output.
#[test]
fn direct_route_modules_match_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let root = fixture.scratch("modules")?;
    let source = root.join("src/main.incn");
    fs::create_dir_all(root.join("src"))?;
    fs::write(
        &source,
        "from facade import exported as compute\nimport helper\n\ndef calculate(a: int, b: int) -> int:\n  return a - b\n\ndef main() -> None:\n  println(compute(b=2, a=40))\n  println(helper.calculate(3, 4))\n  println(calculate(9, 2))\n",
    )?;
    fs::write(
        root.join("src/helper.incn"),
        "def hidden(a: int) -> int:\n  return a\n\npub def calculate(a: int, b: int) -> int:\n  return hidden(a) + b\n",
    )?;
    fs::create_dir_all(root.join("src/facade"))?;
    fs::write(
        root.join("src/facade/__init__.incn"),
        "pub from helper import calculate as exported\n",
    )?;
    let closure = corpus::runtime_closure(&fixture.formatting)?;
    let native = root.join("native");
    success(
        &corpus::source_command(
            &fixture.driver_binary("debug"),
            &source,
            &native,
            &fixture.sysroot,
            &closure,
        )
        .output()?,
        "native three-module compilation",
    );
    let legacy_root = root.join("legacy");
    success(
        &support::repo_command()
            .current_dir(&root)
            .arg("build")
            .arg(&source)
            .arg(&legacy_root)
            .output()?,
        "legacy three-module compilation",
    );
    let expected = Command::new(legacy_root.join("oven/release/main")).output()?;
    let actual = Command::new(native).output()?;
    success(&expected, "legacy three-module execution");
    success(&actual, "native three-module execution");
    assert_eq!(actual.stdout, b"42\n7\n7\n");
    assert_eq!(actual.stdout, expected.stdout);
    assert_eq!(actual.stderr, expected.stderr);
    assert_eq!(actual.status.code(), expected.status.code());
    Ok(())
}

/// Published two-module library bodies retain function, nominal, enum, and adopted-method output.
#[test]
fn direct_route_packages_match_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let root = fixture.scratch("packages")?;
    let library = root.join("deps/library");
    fs::create_dir_all(library.join("src"))?;
    fs::create_dir_all(root.join("src"))?;
    fs::write(
        root.join("loaf.toml"),
        "[project]\nname = \"package_consumer\"\nversion = \"0.1.0\"\n[project.scripts]\nmain = \"src/main.incn\"\n[dependencies]\nlibrary = { path = \"deps/library\" }\n",
    )?;
    fs::write(
        library.join("loaf.toml"),
        "[project]\nname = \"library\"\nversion = \"0.1.0\"\n",
    )?;
    fs::write(
        library.join("src/lib.incn"),
        "pub from values import Reading, Counter, Signal, add, BASE, Answer, classify\n",
    )?;
    fs::write(
        library.join("src/values.incn"),
        "pub const BASE: int = 2\n\npub type Answer = int | str\n\npub def classify(value: Answer) -> int:\n    if isinstance(value, int):\n        return 1\n    return 2\n\npub trait Reading:\n    def get(self) -> int: ...\n\n    def doubled(self) -> int:\n        return self.get() + self.get()\n\npub model Counter with Reading:\n    pub value: int\n\n    def get(self) -> int:\n        return self.value\n\npub enum Signal:\n    Ready\n    Waiting\n\npub def add(value: int) -> int:\n    return value + BASE\n",
    )?;
    let source = root.join("src/main.incn");
    fs::write(
        &source,
        "from pub::library import Counter, Signal, add, BASE, Answer, classify\n\ndef main() -> None:\n    println(add(40))\n    counter = Counter(value=7)\n    println(counter.get())\n    println(counter.doubled())\n    println(BASE)\n    answer: Answer = 7\n    println(classify(answer))\n    signal = Signal.Ready\n    match signal:\n        Signal.Ready => println(1)\n        Signal.Waiting => println(2)\n",
    )?;
    let mut publish = support::cli_project::configured_incan_command(&library, &["oven", "bake", "--project", "."]);
    support::configure_explicit_oven_bake_command(&mut publish)?;
    success(&publish.output()?, "package publication");
    let closure = corpus::runtime_closure(&fixture.formatting)?;
    let native = root.join("native");
    success(
        &corpus::source_command(
            &fixture.driver_binary("debug"),
            &source,
            &native,
            &fixture.sysroot,
            &closure,
        )
        .current_dir(&root)
        .output()?,
        "package native compilation",
    );
    let legacy = root.join("legacy");
    success(
        &support::repo_command()
            .current_dir(&root)
            .arg("build")
            .arg(&source)
            .arg(&legacy)
            .output()?,
        "package legacy compilation",
    );
    let expected = Command::new(legacy.join("oven/release/package_consumer")).output()?;
    let actual = Command::new(native).output()?;
    success(&expected, "package legacy execution");
    success(&actual, "package native execution");
    assert_eq!(actual.stdout, b"42\n7\n14\n2\n1\n1\n");
    assert_eq!(actual.stdout, expected.stdout);
    assert_eq!(actual.stderr, expected.stderr);
    assert_eq!(actual.status.code(), expected.status.code());
    Ok(())
}

/// Published generic functions and receiver methods execute with the consumer's checked concrete arguments.
#[test]
fn published_generic_declarations_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let root = fixture.scratch("published-generic-declarations")?;
    let project = root.join("builtin-parity");
    let provider = project.join("deps/generic_items");
    fs::create_dir_all(provider.join("src"))?;
    fs::write(
        project.join("loaf.toml"),
        "[project]\nname = \"probe\"\nversion = \"0.1.0\"\n[dependencies]\ngeneric_items = { path = \"deps/generic_items\" }\n",
    )?;
    fs::write(
        provider.join("loaf.toml"),
        "[project]\nname = \"generic_items\"\nversion = \"0.1.0\"\n",
    )?;
    fs::write(
        provider.join("src/lib.incn"),
        r#"pub model Holder[T]:
    pub value: T

    def get(self) -> T:
        return self.value

    def first[U](self, values: list[U]) -> U:
        return values[0]

pub def first[T](values: list[T]) -> T:
    return values[0]
"#,
    )?;
    let mut publish = support::cli_project::configured_incan_command(&provider, &["oven", "bake", "--project", "."]);
    support::configure_explicit_oven_bake_command(&mut publish)?;
    publish.env("INCAN_OVEN_BAKE_PROFILES", "debug,release");
    success(&publish.output()?, "generic package publication");
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &root,
        &fixture.sysroot,
        &fixture.formatting,
        r#"from pub::generic_items import Holder, first

def main() -> None:
    holder = Holder[int](value=7)
    println(holder.get())
    println(holder.first(["first", "second"]))
    println(first([42, 43]))
"#,
    )
}

/// A checked package module binding selects callable bodies without requesting a namespace fragment.
#[test]
fn direct_route_package_module_binding_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let root = fixture.scratch("package-module-binding")?;
    let library = root.join("deps/widgets");
    fs::create_dir_all(library.join("src"))?;
    fs::create_dir_all(root.join("src"))?;
    fs::write(
        root.join("loaf.toml"),
        "[project]\nname = 'package_module_binding'\nversion = '0.1.0'\n[dependencies]\nwidgets = { path = 'deps/widgets' }\n",
    )?;
    fs::write(
        library.join("loaf.toml"),
        "[project]\nname = 'widgets'\nversion = '0.1.0'\n",
    )?;
    fs::write(
        library.join("src/lib.incn"),
        "pub def label(value: str) -> str:\n    return value\n",
    )?;
    fs::write(
        root.join("src/main.incn"),
        "import pub::widgets as widgets_alias\n\ndef main() -> None:\n    println(widgets_alias.label('aliased'))\n",
    )?;
    let mut publish = support::cli_project::configured_incan_command(&library, &["oven", "bake", "--project", "."]);
    support::configure_explicit_oven_bake_command(&mut publish)?;
    success(&publish.output()?, "module binding package publication");
    assert_package_routes_match(fixture, &root, "package_module_binding", b"aliased\n")
}

/// A sibling-module model's canonical identity survives public package method calls without source recovery.
#[test]
fn direct_route_package_sibling_model_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let root = fixture.scratch("package-sibling-model")?;
    let area = support::fixtures_dir().join("behavior/lowering_dependencies");
    let behavior = support::behavior_fixtures::discover(&area)?
        .into_iter()
        .find(|case| case.name == "package_inherent_methods_issue1174")
        .ok_or("package sibling model fixture is missing")?;
    support::behavior_fixtures::materialize(&behavior, &root)?;
    for provider in &behavior.providers {
        let mut publish = support::cli_project::configured_incan_command(
            &root.join(&provider.path),
            &["oven", "bake", "--project", "."],
        );
        support::configure_explicit_oven_bake_command(&mut publish)?;
        success(&publish.output()?, "sibling model package publication");
    }
    assert_package_routes_match(
        fixture,
        &root,
        "package_inherent_methods_issue1174",
        b"base=10 tax=2\ntotal=12\n",
    )
}

/// Complete the unchanged package enum corpus case, including qualified model payloads and sibling modules.
#[test]
fn direct_route_package_enum_surfaces_match_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let root = fixture.scratch("package-enum-surfaces")?;
    let area = support::fixtures_dir().join("behavior/cli_dependencies");
    let behavior = support::behavior_fixtures::discover(&area)?
        .into_iter()
        .find(|case| case.name == "dependency_enums_sharing_variant_names")
        .ok_or("package enum surfaces fixture is missing")?;
    support::behavior_fixtures::materialize(&behavior, &root)?;
    for provider in &behavior.providers {
        let mut publish = support::cli_project::configured_incan_command(
            &root.join(&provider.path),
            &["oven", "bake", "--project", "."],
        );
        support::configure_explicit_oven_bake_command(&mut publish)?;
        success(&publish.output()?, "enum surfaces package publication");
    }
    assert_package_routes_match(fixture, &root, "dependency_enums_sharing_variant_names",
        b"enter intro\nleave\nenter\nleave\nentering\nleaving\nintro at gate 1\ngone\nnorth at gate 4\nenter 1,2\nleave done\nstart\nstart\nstart 4\n2\n3\n")
}

/// Published newtypes retain explicit comparison derives and checker-selected ownership clones.
#[test]
fn direct_route_package_derived_newtype_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let root = fixture.scratch("package-derived-newtype")?;
    let library = root.join("deps/ids");
    fs::create_dir_all(library.join("src"))?;
    fs::create_dir_all(root.join("src"))?;
    fs::write(
        root.join("loaf.toml"),
        "[project]\nname = 'package_derived_newtype'\nversion = '0.1.0'\n[dependencies]\nids = { path = 'deps/ids' }\n",
    )?;
    fs::write(
        library.join("loaf.toml"),
        "[project]\nname = 'ids'\nversion = '0.1.0'\n",
    )?;
    fs::write(
        library.join("src/lib.incn"),
        "@derive(Clone, Eq)\npub newtype EntryId = str\n\n@derive(Clone, Eq)\npub newtype Count = int\n\npub def text(value: EntryId) -> str:\n    return value.0\n",
    )?;
    fs::write(
        root.join("src/main.incn"),
        "from pub::ids import EntryId, Count, text\n\ndef main() -> None:\n    value = EntryId('entry')\n    println(text(value))\n    println(value == EntryId('entry'))\n    count = Count(7)\n    println(count == Count(7))\n",
    )?;
    let mut publish = support::cli_project::configured_incan_command(&library, &["oven", "bake", "--project", "."]);
    support::configure_explicit_oven_bake_command(&mut publish)?;
    success(&publish.output()?, "derived newtype package publication");
    assert_package_routes_match(fixture, &root, "package_derived_newtype", b"entry\ntrue\ntrue\n")
}

/// Published scalar enums retain checked derives and their canonical raw values across package calls.
#[test]
fn direct_route_package_derived_value_enum_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let root = fixture.scratch("package-derived-value-enum")?;
    let library = root.join("deps/flows");
    fs::create_dir_all(library.join("src"))?;
    fs::create_dir_all(root.join("src"))?;
    fs::write(
        root.join("loaf.toml"),
        "[project]\nname = 'package_derived_value_enum'\nversion = '0.1.0'\n[dependencies]\nflows = { path = 'deps/flows' }\n",
    )?;
    fs::write(
        library.join("loaf.toml"),
        "[project]\nname = 'flows'\nversion = '0.1.0'\n",
    )?;
    fs::write(
        library.join("src/lib.incn"),
        "@derive(Clone, Eq)\npub enum Phase(str):\n    Enter = 'entering'\n    Leave = 'leaving'\n\npub def phase() -> Phase:\n    return Phase.Enter\n",
    )?;
    fs::write(
        root.join("src/main.incn"),
        "from pub::flows import Phase, phase\n\ndef main() -> None:\n    value = phase()\n    println(value.value())\n    println(value == Phase.Enter)\n    println(Phase.Leave.value())\n",
    )?;
    let mut publish = support::cli_project::configured_incan_command(&library, &["oven", "bake", "--project", "."]);
    support::configure_explicit_oven_bake_command(&mut publish)?;
    success(&publish.output()?, "derived value enum package publication");
    assert_package_routes_match(
        fixture,
        &root,
        "package_derived_value_enum",
        b"entering\ntrue\nleaving\n",
    )
}

/// Prove functions declared in another module used as callable values byte-identical to legacy: imported functions
/// passed back into their module's own higher-order function and into a local one, held in a local, and a `mut`
/// function passed as a callable whose input is `mut`, so its change reaches the caller.
#[test]
fn imported_function_values_match_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let root = fixture.scratch("imported-function-values")?;
    let source = root.join("src/main.incn");
    fs::create_dir_all(root.join("src"))?;
    fs::write(
        root.join("src/tools.incn"),
        r#"pub class Counter:
    pub value: int

pub def plus_one(value: int) -> int:
    return value + 1

pub def double(value: int) -> int:
    return value * 2

pub def map_owned(items: list[int], f: (int) -> int) -> list[int]:
    mut out: list[int] = []
    for item in items:
        out.append(f(item))
    return out

pub def grow(mut counter: Counter, by: int) -> int:
    counter.value += by
    return counter.value

pub def apply(step: (mut Counter, int) -> int, mut counter: Counter) -> int:
    return step(counter, 2)
"#,
    )?;
    fs::write(
        &source,
        r#"from tools import Counter, plus_one, double, map_owned, grow, apply

def twice(f: (int) -> int, value: int) -> int:
    return f(f(value))

def main() -> None:
    println(map_owned([1, 2, 3], plus_one))
    g = plus_one
    println(g(4))
    println(twice(plus_one, 5))
    println(twice(double, 3))
    mut counter = Counter(value=1)
    println(apply(grow, counter))
    println(counter.value)
"#,
    )?;
    let closure = corpus::runtime_closure(&fixture.formatting)?;
    let native = root.join("native");
    success(
        &corpus::source_command(
            &fixture.driver_binary("debug"),
            &source,
            &native,
            &fixture.sysroot,
            &closure,
        )
        .output()?,
        "native imported function value compilation",
    );
    let legacy_root = root.join("legacy");
    success(
        &support::repo_command()
            .current_dir(&root)
            .arg("build")
            .arg(&source)
            .arg(&legacy_root)
            .output()?,
        "legacy imported function value compilation",
    );
    let expected = Command::new(legacy_root.join("oven/release/main")).output()?;
    let actual = Command::new(native).output()?;
    success(&expected, "legacy imported function value execution");
    success(&actual, "native imported function value execution");
    assert_eq!(actual.stdout, b"[2, 3, 4]\n5\n7\n12\n3\n3\n");
    assert_eq!(actual.stdout, expected.stdout);
    assert_eq!(actual.stderr, expected.stderr);
    assert_eq!(actual.status.code(), expected.status.code());
    Ok(())
}

/// Compare a published package consumer's complete native output and exit status against the legacy route.
fn assert_package_routes_match(
    fixture: &DriverFixture,
    root: &Path,
    binary_name: &str,
    stdout: &[u8],
) -> Result<(), Box<dyn std::error::Error>> {
    let source = root.join("src/main.incn");
    let closure = corpus::runtime_closure(&fixture.formatting)?;
    let native = root.join("native");
    success(
        &corpus::source_command(
            &fixture.driver_binary("debug"),
            &source,
            &native,
            &fixture.sysroot,
            &closure,
        )
        .current_dir(root)
        .output()?,
        "package native compilation",
    );
    let legacy = root.join("legacy");
    success(
        &support::repo_command()
            .current_dir(root)
            .arg("build")
            .arg(&source)
            .arg(&legacy)
            .output()?,
        "package legacy compilation",
    );
    let expected = Command::new(legacy.join("oven/release").join(binary_name)).output()?;
    let actual = Command::new(native).output()?;
    success(&expected, "package legacy execution");
    success(&actual, "package native execution");
    assert_eq!(actual.stdout, stdout);
    assert_eq!(actual.stdout, expected.stdout);
    assert_eq!(actual.stderr, expected.stderr);
    assert_eq!(actual.status.code(), expected.status.code());
    Ok(())
}

/// Frozen and owned union members retain their identities while isinstance recognizes both as source str.
#[test]
fn direct_route_frozen_union_strings_match_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("release"),
        &fixture.scratch("frozen-union-strings")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"const NAME: str = "policy"
const FROZEN: FrozenStr = "frozen"
const EMPTY: str = ""

type Label = FrozenStr | str | int

def classify(value: Label) -> str:
    """Match every string storage without widening the integer arm."""
    if isinstance(value, str):
        return "text"
    if isinstance(value, int):
        return "int"
    return "other"

def choose(which: int) -> Label:
    """Construct each retained union member at its checked boundary."""
    if which == 0:
        return FROZEN
    if which == 1:
        owned: str = "owned"
        return owned
    return 7

def frozen_text(value: FrozenStr | int) -> bool:
    """Accept an evaluated static str constant in the frozen member."""
    return isinstance(value, str)

def owned_text(value: str | int) -> bool:
    """Convert a frozen value at an owned-string union boundary."""
    return isinstance(value, str)

def frozen_target(value: Label) -> bool:
    """Retain legacy's symmetric string identity for a frozen target."""
    return isinstance(value, FrozenStr)

def present(value: Option[FrozenStr]) -> str:
    """Observe a frozen payload through the existing standard Option carrier."""
    match value:
        Some(text) => return str(text)
        None => return "missing"

def frozen_place(value: FrozenStr) -> None:
    """Observe actual frozen place parsing and empty-text truthiness separately from static constants."""
    println(bool(value))

def parsed_place(value: FrozenStr) -> None:
    """Parse borrowed frozen places using the legacy conversion helpers."""
    println(int(value))
    println(float(value))

def main() -> None:
    """Compare copied constants, both string union tags, and nonstring counterexamples."""
    println(isinstance(FROZEN, str))
    println(bool(NAME))
    println(bool(EMPTY))
    frozen_place(EMPTY)
    parsed_place("12")
    println(classify(NAME))
    println(classify("literal"))
    println(classify(choose(0)))
    println(classify(choose(1)))
    println(classify(choose(2)))
    println(frozen_text(NAME))
    println(frozen_text(7))
    println(owned_text(FROZEN))
    println(owned_text(7))
    println(frozen_target(choose(0)))
    println(frozen_target(choose(1)))
    println(frozen_target(choose(2)))
    println(present(Some(NAME)))
    println(present(None))
    values: list[str] = [FROZEN]
    words: dict[str, str] = {"k": FROZEN}
    println(values[0])
    println(words["k"])
"#,
    )
}

/// Prove builtin text methods and every numeric resize policy through checked destinations.
#[test]
fn direct_route_non_list_methods_match_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("release"),
        &fixture.scratch("non-list-methods")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"def clone_text(value: str) -> str:
    """Return an owned observation while retaining the source."""
    return value.to_string()

def capped(value: i16) -> i8:
    """Return the checked saturating target carrier."""
    return value.saturating_resize()

def main() -> None:
    """Observe canonical text methods and all four resizing policies."""
    text = "héllo"
    println(clone_text(text))
    println(text)
    println("hello".startswith("he"))
    println("hello".endswith("lo!"))
    println(text.startswith("hé"))
    println(text.endswith("lo"))
    small: i8 = 120
    wide: int = small.resize()
    println(wide)
    over: int = 300
    maybe: Option[i8] = over.try_resize()
    match maybe:
        Some(value) => println(value)
        None => println("none")
    fits: int = 12
    present: Option[i8] = fits.try_resize()
    match present:
        Some(value) => println(value)
        None => println("none")
    wrapped: i8 = over.wrapping_resize()
    println(wrapped)
    saturated: i8 = over.saturating_resize()
    println(saturated)
    println(capped(240))
"#,
    )
}

/// Prove borrowed collection equality, contextual empty operands, and source preservation against legacy.
#[test]
fn direct_route_collection_equality_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("release"),
        &fixture.scratch("collection-equality")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"def is_empty(values: list[str]) -> bool:
    """Compare a borrowed parameter with its checked empty literal."""
    return values == []

def is_not_empty(values: list[str]) -> bool:
    """Keep contextual element types with the literal in the other operand position."""
    return [] != values

def main() -> None:
    """Compare each admitted collection while retaining its original storage."""
    println(is_empty([]))
    println(is_empty(["value"]))
    println(is_not_empty(["value"]))
    println(is_not_empty([]))
    mut words = ["a", "b"]
    println(words == ["a", "b"])
    println(words != ["a", "c"])
    words.append("c")
    println(len(words))
    floats = [1.0, 2.0]
    println(floats == [1.0, 2.0])
    println(floats != [1.0, 3.0])
    nested = [[1], [2]]
    println(nested == [[1], [2]])
    pairs = [(1, "a"), (2, "b")]
    println(pairs == [(1, "a"), (2, "b")])
    keys = {"a", "b"}
    println(keys == {"b", "a"})
    println(keys != {"a"})
    println(len(keys))
    values = {"x": 1.0, "y": 2.0}
    println(values == {"y": 2.0, "x": 1.0})
    println(values != {"x": 1.0, "y": 3.0})
    println(len(values))
"#,
    )
}

/// Prove tuple construction, typed signatures, constant projections, and simultaneous unpacking against legacy.
#[test]
fn tuple_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let root = fixture.scratch("tuples")?;
    let source = root.join("tuples.incn");
    fs::write(
        &source,
        r#"model Boxed:
    value: int

def swap(value: tuple[int, int]) -> tuple[int, int]:
    return (value[1], value[0])

def identity(value: tuple[int, str]) -> tuple[int, str]:
    return value

def main() -> None:
    mut a = 3
    mut b = 7
    a, b = b, a
    pair: tuple[int, int] = swap((a, b))
    x, y = pair
    println(x)
    println(y)
    println(pair)
    text_pair = identity((9, "hello\n\"tuple\""))
    number, text = text_pair
    println(text_pair)
    println(number)
    println(text)
    println(text_pair[-1])
    println((true, 1.5))
    mut boxed = Boxed(value=0)
    boxed.value, a = pair
    println(boxed.value)
    println(a)
"#,
    )?;
    let native = root.join("native");
    let closure = corpus::runtime_closure(&fixture.formatting)?;
    success(
        &corpus::source_command(
            &fixture.driver_binary("debug"),
            &source,
            &native,
            &fixture.sysroot,
            &closure,
        )
        .output()?,
        "native tuple compilation",
    );
    let legacy_root = root.join("legacy");
    success(
        &support::repo_command()
            .arg("build")
            .arg(&source)
            .arg(&legacy_root)
            .output()?,
        "legacy tuple compilation",
    );
    let expected = Command::new(legacy_root.join("oven/release/tuples")).output()?;
    let actual = Command::new(native).output()?;
    success(&expected, "legacy tuple execution");
    success(&actual, "native tuple execution");
    assert_eq!(actual.stdout, expected.stdout);
    assert!(actual.stdout.starts_with(b"3\n7\n(3, 7)\n"));
    for (name, source_text, kind) in [
        (
            "function_value",
            "def apply(value: (int) -> int) -> int:\n    return value(1)\n\ndef main() -> None:\n    println(1)\n",
            "Function",
        ),
        (
            "singleton",
            "def main() -> None:\n    println((42,))\n",
            "singleton Tuple",
        ),
    ] {
        let refused_source = root.join(format!("{name}.incn"));
        fs::write(&refused_source, source_text)?;
        let refused_output = root.join(format!("{name}-native"));
        let refused = corpus::source_command(
            &fixture.driver_binary("debug"),
            &refused_source,
            &refused_output,
            &fixture.sysroot,
            &closure,
        )
        .output()?;
        assert!(!refused.status.success());
        assert!(
            String::from_utf8_lossy(&refused.stderr).contains(kind),
            "{}",
            String::from_utf8_lossy(&refused.stderr)
        );
        assert!(!refused_output.exists());
    }
    Ok(())
}

/// Model method aliases call their checked target bodies and preserve receiver changes and argument order.
#[test]
fn model_method_aliases_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    check_declaration_case(
        "model_method_aliases",
        r#"model Counter:
    value: int
    current = read
    advance = add
    combined = combine

    def read(self) -> int:
        return self.value

    def add(mut self, amount: int) -> None:
        self.value += amount

    def combine(self, first: int, second: int) -> int:
        return self.value + first * 10 + second

def main() -> None:
    mut counter = Counter(value=10)
    println(counter.current())
    counter.advance(amount=3)
    println(counter.read())
    counter.add(2)
    println(counter.current())
    println(counter.combined(second=2, first=1))
    println(counter.combine(1, 2))
"#,
        None,
    )
}

/// Source Partial wrappers preserve overrideable presets, residual defaults, and callable aliases.
#[test]
fn source_partial_function_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    check_declaration_case(
        "source_partial_function",
        r#"def suffix() -> str:
    println("default evaluated")
    return "text"

pub def route(method: str, path: str, tail: str = suffix()) -> str:
    return method + path + tail

pub get = partial route(method="GET")
renamed = get

def scale(k: int, n: int) -> int:
    return k * n

negative = partial scale(k=-2)

def main() -> None:
    println(get(path="/health"))
    println(get("GET", "/ready", tail="!"))
    println(get(method="POST", path="/x", tail="!"))
    println(renamed(path="/alias", tail="!"))
    println(get(path="/again"))
    println(negative(n=3))
    println(negative(k=4, n=3))
"#,
        None,
    )
}

/// Prove local partials byte-identical to legacy: a preset captured from a local and then overridden by name, a
/// declaration default evaluated inside the partial each time a call leaves it out, text presets and defaults that
/// stay owned by the partial across calls and a move into another binding, and float and bool defaults left out
/// together.
#[test]
fn local_partial_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("local-partial")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"def route(method: int, path: int, content_type: int = 3) -> int:
    return method * 100 + path * 10 + content_type

def suffix() -> str:
    println("default evaluated")
    return "text"

def join(method: str, path: str, tail: str = suffix()) -> str:
    return method + path + tail

def scaled(factor: float, value: float = 2.5, enabled: bool = true) -> float:
    if enabled:
        return factor * value
    return 0.0

def main() -> None:
    mut method = 1
    get = partial route(method=method)
    method = 9
    println(get(4))
    println(get(method=7, path=2, content_type=5))
    println(get(path=6, content_type=0))
    println(method)
    prefix = "GET"
    join_get = partial join(method=prefix)
    println(join_get("/health"))
    println(join_get(path="/x", method="POST"))
    println(join_get("/ready", tail="!"))
    println(prefix)
    held = join_get
    println(held(path="/moved"))
    half = partial scaled(factor=0.5)
    println(half())
    println(half(value=4.0, enabled=false))
    println(half(enabled=true))
"#,
    )
}

/// Prove list defaults kept by local partials byte-identical to legacy: an empty and a non-empty default list left to
/// the partial, which builds and views it on every call that omits it, beside a list literal and a caller's list
/// passed in its place, which the partial only views.
#[test]
fn partial_list_default_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("partial-list-default")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"def keep(prefix: int, items: list[float] = []) -> int:
    return prefix + len(items)

def total(base: int, values: list[int] = [1, 2, 3]) -> int:
    mut sum = base
    for value in values:
        sum += value
    return sum

def main() -> None:
    deferred = partial keep(prefix=1)
    println(deferred())
    println(deferred(items=[1.5, 2.5]))
    adder = partial total(base=10)
    println(adder())
    println(adder(values=[5]))
    mine = [4, 4]
    println(adder(values=mine))
    println(mine)
    println(adder(base=0))
"#,
    )
}

/// Prove borrowed values handed over as owned ones byte-identical to legacy: a model field passed to a by-value
/// parameter, `self` and a field of `self` returned from a model or class method, list, nested list and bytes
/// parameters returned whole, and list and bytes literal defaults returned from the call that omits them.
#[test]
fn borrowed_value_handover_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("borrowed-value-handover")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"model Cursor:
    position: int

model Wrapper:
    cursor: Cursor
    label: str

    def snapshot(self) -> Wrapper:
        return self

    def current(self) -> Cursor:
        return self.cursor

class Account:
    pub owner: str
    pub balance: int

    def copy(self) -> Account:
        return self

def advance(cursor: Cursor) -> Cursor:
    return Cursor(position=cursor.position + 1)

def keep(payload: bytes = b"x") -> bytes:
    return payload

def first(items: list[int] = [4, 5, 6]) -> list[int]:
    return items

def nested(rows: list[list[str]]) -> list[list[str]]:
    return rows

def main() -> None:
    other = Wrapper(cursor=Cursor(position=1), label="w")
    moved = advance(other.cursor)
    println(f"{moved.position} {other.cursor.position}")
    copy = other.snapshot()
    println(f"{copy.cursor.position} {copy.label} {other.cursor.position} {other.label}")
    println(other.current().position)
    account = Account(owner="ann", balance=5)
    twin = account.copy()
    println(f"{twin.owner} {twin.balance} {account.owner}")
    data = b"abc"
    println(len(keep(data)))
    println(len(keep()))
    println(len(data))
    nums = [1, 2]
    kept = first(nums)
    println(f"{kept} {nums}")
    grid = [["a"], ["b", "c"]]
    println(len(nested(grid)))
    println(grid)
    println(first())
    println(len(keep(b"")))
"#,
    )
}

/// Prove `isinstance` over an `Option` byte-identical to legacy: `Some` of the tested type, text and frozen text alike,
/// `None`, a payload that can never be the tested type, an `Option` of a union tested for each member and for a type
/// it never holds from a local and a parameter, and `Option` model fields.
#[test]
fn option_isinstance_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("option-isinstance")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"const NAME: FrozenStr = "policy"

model Slot:
    value: Option[int]
    label: Option[str]

def maybe(flag: bool) -> Option[str]:
    if flag:
        return Some("x")
    return None

def maybe_frozen(flag: bool) -> Option[FrozenStr]:
    if flag:
        return Some(NAME)
    return None

def maybe_int(flag: bool) -> Option[int]:
    if flag:
        return Some(3)
    return None

def maybe_union(pick: int) -> Option[str | int]:
    if pick == 0:
        return Some("text")
    if pick == 1:
        return Some(4)
    return None

def is_text(value: Option[str | int]) -> bool:
    return isinstance(value, str)

def main() -> None:
    println(isinstance(maybe(true), str))
    println(isinstance(maybe(false), str))
    println(isinstance(maybe_frozen(true), str))
    println(isinstance(maybe_frozen(false), str))
    println(isinstance(maybe_int(true), str))
    println(isinstance(maybe_int(true), int))
    println(isinstance(maybe_int(false), int))
    for pick in range(3):
        held = maybe_union(pick)
        println(f"{isinstance(held, str)} {isinstance(held, int)} {isinstance(held, float)} {is_text(held)}")
    slot = Slot(value=Some(5), label=Some("a"))
    println(f"{isinstance(slot.value, int)} {isinstance(slot.value, str)} {isinstance(slot.label, str)}")
    empty = Slot(value=None, label=None)
    println(f"{isinstance(empty.value, int)} {isinstance(empty.label, str)}")
"#,
    )
}

/// Prove match guards that make the last use of an owned value, and alternations nested as tuple items, byte-identical
/// to legacy: a guard taking a text parameter after a tuple alternation, a literal alternation and a carrier arm, with
/// the arms that skip it; an alternation inside a tuple item, at the top of an arm and below a top-level alternation,
/// each binding through the first alternative that matches; and a match value that moves an owner in one arm.
#[test]
fn guard_and_nested_alternation_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("guard-and-nested-alternation")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"def approved(name: str) -> bool:
    return len(name) > 2

def confirm(pair: tuple[int, str], reviewer: str) -> str:
    match pair:
        (1, "yes") | (1, "y") if approved(reviewer) => return "confirmed"
        _ => return "unconfirmed"

def small(n: int, label: str) -> str:
    match n:
        1 | 2 if approved(label) => return "small"
        _ => return "other"

def tag_of(value: Option[int], note: str) -> str:
    match value:
        Some(n) if approved(note) => return f"some {n}"
        Some(n) => return f"plain {n}"
        None => return "none"

def code(pair: tuple[Result[int, int], int]) -> str:
    match pair:
        (Ok(1), n) | (Ok(n) | Err(n), _) if n != 0 => return f"{n}"
        _ => return "other"

def either(pair: tuple[Result[int, int], str]) -> str:
    match pair:
        (Ok(n) | Err(n), "x") => return f"x {n}"
        (_, label) => return label

def describe(n: int, text: str) -> str:
    result = match n:
        0 => text
        _ => "taken"
    return result

def main() -> None:
    println(confirm((1, "y"), "ada"))
    println(confirm((1, "yes"), "al"))
    println(confirm((0, "yes"), "ada"))
    println(small(2, "label"))
    println(small(2, "ab"))
    println(small(5, "label"))
    println(tag_of(Some(3), "note"))
    println(tag_of(Some(4), "n"))
    println(tag_of(None, "note"))
    println(code((Ok(1), 0)))
    println(code((Ok(2), 0)))
    println(code((Ok(1), 5)))
    println(code((Err(3), 0)))
    println(code((Err(0), 0)))
    println(either((Err(7), "x")))
    println(either((Ok(8), "y")))
    println(describe(0, "kept"))
    println(describe(1, "kept"))
"#,
    )
}

/// Prove collection constructors over a source, and owned models passed into callable objects, byte-identical to
/// legacy: `list()` of a list parameter, a `mut` list parameter, text, a dictionary's keys and a generator, `dict()` of
/// a dictionary, each leaving its source intact; a capturing `map_err` closure and a capturing closure argument that
/// each take a model by value.
#[test]
fn collection_constructor_sources_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("collection-constructor-sources")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"model Failure:
    detail: str

    def message(self) -> str:
        return self.detail

def numbers(limit: int) -> Generator[int]:
    for value in range(limit):
        yield value

def copied(items: list[str]) -> list[str]:
    return list(items)

def from_mut(mut items: list[str]) -> list[str]:
    items.append("z")
    return list(items)

def lookup(key: str) -> Result[str, Failure]:
    return Err(Failure(detail=key))

def describe(key: str, code: int) -> str:
    return f"{key}:{code}"

def fetch(key: str) -> Result[str, str]:
    return lookup(key).map_err((failure) => describe(key, len(failure.message())))

def apply(f: (Failure) -> int, value: Failure) -> int:
    return f(value)

def main() -> None:
    words = ["ab", "c"]
    println(f"{copied(words):?} {len(words)}")
    mut more = ["x"]
    println(f"{from_mut(more):?} {more:?}")
    println(f"{list("hey"):?}")
    keyed: dict[str, int] = {"k": 1, "j": 2}
    keys = list(keyed)
    println(len(keys))
    println(f"{list(numbers(3)):?}")
    table: dict[str, int] = {"a": 1, "b": 2}
    copy = dict(table)
    println(f"{len(copy)} {copy["b"]} {len(table)}")
    match fetch("key"):
        Ok(value) => println(value)
        Err(error) => println(error)
    width = 3
    println(apply((failure) => len(failure.detail) + width, Failure(detail="four")))
"#,
    )
}

/// Prove source enum variants spelled like the builtin carriers' variants, and set, dictionary and list payloads,
/// byte-identical to legacy: `Maybe.None` and `Outcome.Ok` patterns select the source enum's own variants beside a
/// builtin `Option` match, and payload bindings copy their collections out of the matched value, which stays usable.
#[test]
fn source_enum_variant_patterns_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("source-enum-variant-patterns")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"enum Maybe:
    Some(int)
    None

enum Outcome:
    Ok(int)
    Err(str)

enum Container:
    Empty
    Items(list[int])
    Tags(set[str])
    Counts(dict[str, int])

def unwrap_or(value: Maybe, fallback: int) -> int:
    match value:
        Maybe.Some(x) => return x
        Maybe.None => return fallback

def is_none(value: Maybe) -> bool:
    match value:
        case Maybe.None:
            return true
        case _:
            return false

def describe(outcome: Outcome) -> str:
    match outcome:
        Outcome.Ok(n) => return f"ok {n}"
        Outcome.Err(reason) => return f"err {reason}"

def size(container: Container) -> int:
    match container:
        Container.Empty => return 0
        Container.Items(items) => return len(items)
        Container.Tags(tags) => return len(tags)
        Container.Counts(counts) => return len(counts)

def main() -> None:
    println(unwrap_or(Maybe.Some(4), 0))
    println(unwrap_or(Maybe.None, 9))
    println(is_none(Maybe.None))
    println(is_none(Maybe.Some(1)))
    maybe: Option[int] = None
    match maybe:
        Some(v) => println(v)
        None => println("builtin none")
    println(describe(Outcome.Ok(1)))
    println(describe(Outcome.Err("bad")))
    println(size(Container.Empty))
    println(size(Container.Items([1, 2, 3])))
    println(size(Container.Tags({"a", "b"})))
    counts = Container.Counts({"x": 1})
    println(size(counts))
    println(size(counts))
"#,
    )
}

/// Prove member patterns over an optional union byte-identical to legacy: each member and `None` selected in a
/// `match` over a parameter, a member pattern with a nested branch, and a member pattern beside a wildcard over a loop
/// variable.
#[test]
fn optional_union_member_pattern_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("optional-union-member-pattern")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"def describe(value: int | str | None) -> str:
    match value:
        int(number) => return f"int {number}"
        str(text) => return f"str {text}"
        None => return "missing"

def flags(value: Option[bool | int]) -> str:
    match value:
        bool(flag) =>
            if flag:
                return "yes"
            return "no"
        int(count) => return f"count {count + 1}"
        None => return "absent"

def first_text(values: list[int | str | None]) -> str:
    for value in values:
        match value:
            str(text) => return text
            _ => pass
    return "none"

def main() -> None:
    println(describe(3))
    println(describe("three"))
    println(describe(None))
    println(flags(Some(true)))
    println(flags(Some(false)))
    println(flags(Some(4)))
    println(flags(None))
    println(first_text([1, None, "found", "later"]))
    println(first_text([None]))
"#,
    )
}

/// Prove `mut` parameters reached through callable values byte-identical to legacy: a function, a closure and a
/// function-pointer local passed as a callable type whose input is `mut`, an inferred and an annotated callable local,
/// a `mut` parameter handed on to another `mut` parameter, and `mut` list and text inputs; every change reaches the
/// caller's value.
#[test]
fn mutable_callable_input_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("mutable-callable-input")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"class Counter:
    pub value: int

def grow(mut counter: Counter, by: int) -> int:
    counter.value += by
    return counter.value

def grow_twice(mut counter: Counter) -> int:
    grow(counter, 1)
    return grow(counter, 1)

def apply(step: (mut Counter, int) -> int, mut counter: Counter) -> int:
    println(step(counter, 2))
    return step(counter, 3)

def push(mut items: list[int], value: int) -> int:
    items.append(value)
    return len(items)

def extend(step: (mut list[int], int) -> int, mut items: list[int]) -> int:
    return step(items, 9)

def shout(mut text: str) -> int:
    println(text + "!")
    return len(text)

def main() -> None:
    mut counter = Counter(value=1)
    println(apply(grow, counter))
    println(counter.value)
    println(apply((c, by) => c.value + by, counter))
    println(counter.value)
    g = grow
    println(g(counter, 10))
    handler: (mut Counter, int) -> int = grow
    println(handler(counter, 100))
    println(apply(handler, counter))
    println(grow_twice(counter))
    println(counter.value)
    mut items = [1, 2]
    println(extend(push, items))
    println(extend((xs, value) => len(xs) + value, items))
    println(items)
    mut word = "hey"
    loud: (mut str) -> int = shout
    println(loud(word))
    println(word)
"#,
    )
}

/// List display preserves legacy Debug spelling, escaping, nesting, and repeated owner reads.
#[test]
fn list_display_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    check_declaration_case(
        "list_display",
        r#"def main() -> None:
    values = [1, 2]
    println(values)
    println(f"values={values}")
    println(values)
    println([true, false])
    println([1.0, -2.5])
    words = ["quoted\"", "line\n", "é"]
    println(words)
    println(words)
    println([[1, 2], [3]])
    println([1] + [2])
"#,
        None,
    )
}

/// Derived Display uses the same Debug structure in println, string conversion, and interpolation as legacy.
#[test]
fn model_display_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    check_declaration_case(
        "model_display",
        r#"@derive(Display)
model Record:
    number: int
    label: str

def main() -> None:
    record = Record(number=7, label="quoted\"\n")
    println(record)
    println(f"record={record}")
    println(str(record))
    println(f"{record:?}")
    println(record.label)
"#,
        None,
    )
}

/// JSON module bundles, imported aliases, and qualified derives serialize through the legacy serde boundary.
#[test]
fn model_json_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    check_declaration_case(
        "model_json",
        r#"from std.serde import json
from std.serde.json import Serialize as JsonSerialize

@derive(json)
model Record:
    number: int
    label: str

@derive(JsonSerialize)
model Alias:
    enabled: bool

@derive(json.Serialize)
model Qualified:
    value: float

@derive(json.Serialize)
model Custom:
    number: int

    def to_json(self) -> str:
        return "custom"

def next_value() -> str:
    println("evaluated")
    return "line\né"

def main() -> None:
    record = Record(number=7, label="quoted\"\n")
    alias = Alias(enabled=true)
    qualified = Qualified(value=2.5)
    custom = Custom(number=9)
    println(record.to_json())
    println(json_stringify(record))
    println(json_stringify(123))
    println(json_stringify(None))
    println(json_stringify(9223372036854775807))
    println(json_stringify(-9223372036854775807))
    println(json_stringify(next_value()))
    println(std.builtins.json_stringify(next_value()))
    println(json_stringify(-7))
    println(json_stringify(2.5))
    println(json_stringify(true))
    println(json_stringify("escaped\"\n"))
    println(alias.to_json())
    println(qualified.to_json())
    println(custom.to_json())
    println(json_stringify(custom))
    println(record.to_json())
    println(record.label)
"#,
        None,
    )
}

/// Prove lazy primitive-list zip snapshots, tuple items, and independent aliases against legacy.
#[test]
fn direct_route_tuple_zip_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("tuple-zip")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"def main() -> None:
    """Observe equal, unequal, empty, and independently aliased zip inputs."""
    left = [1, 2, 3]
    right = [10, 20]
    pairs = zip(left, right)
    alias = pairs
    again = pairs
    for a, b in alias:
        println(a + b)
    for a, b in pairs:
        println(a + b)
    for a, b in again:
        println(a + b)
    for a, b in zip([4], [5]):
        println(a + b)
    empty: list[int] = []
    for a, b in zip(empty, right):
        println(a + b)
    for a, b in zip([7, 8], ["x"]):
        println(a)
        println(b)
"#,
    )?;
    Ok(())
}

/// Prove zip over structured items byte-identical to legacy: a stored zip of a list of lists with a list of ints read
/// through a parameter, a list of tuple lists zipped with a list of tuples and read through nested loops and fields,
/// a zip over a list read out of another list and a tuple's list field, and a zip of text with lists.
#[test]
fn structured_zip_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("structured-zip")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"def typed_zip_target(left: list[list[int]], right: list[int]) -> int:
    pairs = zip(left, right)
    mut total = 0
    for typed_zip_left, typed_zip_right in pairs:
        total += len(typed_zip_left) + typed_zip_right
    return total

def main() -> None:
    println(typed_zip_target([[1, 2], [3]], [10, 20]))
    left = [[(1, "one")], [(2, "two")]]
    right = [("first", true), ("second", false)]
    mut total = 0
    for group, pair in zip(left, right):
        for member in group:
            total += member.0
            println(member.1)
        if pair.1:
            total += 10
        println(pair.0)
    println(total)
    groups = [[(1, "one"), (2, "two")]]
    labels = ([true, false], 0)
    for numbered, flag in zip(groups[0], labels.0):
        println(numbered.1)
        if flag:
            println(numbered.0)
    names = ["a", "bc"]
    sizes = [[1], [2, 3]]
    for name, size in zip(names, sizes):
        println(f"{name} {len(size)}")
"#,
    )
}

/// Prove loops over text and bytes byte-identical to legacy: a literal, a local and a parameter iterated as
/// one-character strings, including a multi-byte character and empty text, and bytes literals and locals iterated as
/// their unsigned values widened to `int`.
#[test]
fn text_and_bytes_iteration_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("text-bytes-iteration")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"def letters(text: str) -> int:
    mut count = 0
    for letter in text:
        if letter != "a":
            count += 1
    return count

def main() -> None:
    mut marked: list[str] = []
    for ch in "hey":
        marked.append(ch + ".")
    println(len(marked))
    println(marked[2])
    word = "añb"
    for letter in word:
        println(letter)
    println(word)
    println(letters("banana"))
    mut sum = 0
    for b in b"AB":
        sum += b
    println(sum)
    data = b"\x01\xff"
    for value in data:
        println(value)
    println(len(data))
    for skipped in "":
        println(skipped)
    println("done")
"#,
    )
}

/// Prove stored and immediate builtin enumeration, deferred yields, and tuple-string ownership against legacy.
#[test]
fn direct_route_enumerate_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("enumerate")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"def words() -> list[str]:
    """Show that the enumeration source is evaluated once."""
    println("source")
    return ["é", "cat"]

def generated() -> Generator[int]:
    """Enumerate inside a lazy producer."""
    for index, value in enumerate([5, 6]):
        yield index + value

def main() -> None:
    """Observe stored pairs, source preservation, Unicode, empty inputs, and deferred enumeration."""
    source = words()
    stored = enumerate(source)
    alias = stored
    for index, value in stored:
        println(index)
        println(value)
    for pair in alias:
        println(pair.0)
        println(pair.1)
    println(source[0])
    for index, value in enumerate("é猫"):
        println(index)
        println(value)
    for index, value in enumerate([1.5, 2.5]):
        println(index)
        println(value)
    for index, value in enumerate([true, false]):
        println(index)
        println(value)
    empty: list[int] = []
    println(len(enumerate(empty)))
    collected = generated().collect()
    println(collected[0])
    println(collected[1])
    for number, label in zip([7], ["zip"]):
        println(number)
        println(label)
"#,
    )?;
    Ok(())
}

/// Supertrait slots dispatch to the body legacy expansion selects, proven where two candidates compete: the adopter's
/// own method over a subtrait default, and the slot trait's own default over a subtrait default. Defaults that call a
/// supertrait slot reach each adopter's implementation.
#[test]
fn direct_route_supertraits_match_legacy() -> Result<(), Box<dyn std::error::Error>> {
    check_project_case(
        "supertraits",
        r#"
trait Named:
    def name(self) -> str: ...


trait Tag with Named:
    def tag(self) -> str: ...

    def loud(self) -> str:
        """Call a slot of this trait from its default."""
        return self.tag() + "!"

    def size(self) -> int:
        """Call a supertrait slot from a subtrait default."""
        return len(self.tag()) + len(self.name())

    def greet(self, other: Self) -> str:
        """Call a supertrait slot on another value of the adopting type."""
        return "hi " + other.name().upper()


trait Root:
    def label(self) -> str: ...

    def show(self) -> str:
        """Observe which body fills the label slot for each adopter."""
        return "[" + self.label() + "]"


trait Child with Root:
    def label(self) -> str:
        """Fill the supertrait slot for adopters without their own label."""
        return "child"


model Label with Tag:
    text: str

    def name(self) -> str:
        """Implement the supertrait slot directly."""
        return "label"

    def tag(self) -> str:
        """Implement the subtrait slot directly."""
        return self.text


model Plain with Child:
    value: int


class Custom with Child:
    value: int

    def label(self) -> str:
        """Take the slot over from the subtrait default."""
        return "own"


trait Base:
    def label(self) -> str:
        """Fill this slot with the trait's own default."""
        return "base"

    def frame(self) -> str:
        """Observe that the slot keeps its own default over a subtrait's."""
        return "{" + self.label() + "}"


trait Derived with Base:
    def label(self) -> str:
        """Refine the slot without taking it over from the supertrait default."""
        return "derived"


model Layered with Derived:
    value: int


def main() -> None:
    """Dispatch every supertrait slot through concrete receivers."""
    label = Label(text="x")
    println(label.loud())
    println(label.size())
    println(label.greet(Label(text="y")))
    println(label.name())
    println(Plain(value=1).show())
    println(Plain(value=1).label())
    println(Custom(value=2).show())
    println(Custom(value=2).label())
    println(Layered(value=3).frame())
"#,
        b"x!\n6\nhi LABEL\nlabel\n[child]\nchild\n[own]\nown\n{base}\n",
    )
}

/// An imported trait's defaults run for an adopter in another module: they construct and return the trait module's own
/// model and enum, call the trait module's private helper and the adopter's own methods, and keep a local binding that
/// shadows a model name. A local trait sharing an imported trait's short name keeps each adopter on its own trait.
#[test]
fn imported_trait_default_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    check_project_modules_case(
        "imported_trait_default",
        r#"import protocol
from shapes import Measured


trait Same:
    def value(self) -> Option[int]:
        return Some(1)


model Card with Measured:
    side: int

    def width(self) -> int:
        return self.side

    def height(self) -> int:
        return self.side + 1


model Local with Same:
    amount: int


model Remote with protocol.Same:
    amount: int


def show(label: str, value: Option[int]) -> None:
    match value:
        Some(inner) => println(f"{label} {inner}")
        None => println(f"{label} none")


def main() -> None:
    card = Card(side=3)
    println(card.extent().area())
    println(card.is_top())
    println(card.size().n)
    println(card.count())
    show("local", Local(amount=1).value())
    show("remote", Remote(amount=2).value())
"#,
        &[
            (
                "shapes.incn",
                r#"from units import Size


pub model Extent:
    pub width: int
    pub height: int

    def area(self) -> int:
        return self.width * self.height


pub enum Corner:
    Top
    Bottom


def doubled(n: int) -> int:
    return n * 2


pub trait Measured:
    def width(self) -> int: ...

    def height(self) -> int: ...

    def extent(self) -> Extent:
        return Extent(width=self.width(), height=self.height())

    def corner(self) -> Corner:
        return Corner.Top

    def is_top(self) -> bool:
        match self.corner():
            Corner.Top => return true
            _ => return false

    def size(self) -> Size:
        return Size(n=doubled(self.width()))

    def count(self) -> int:
        Extent = 5
        return Extent
"#,
            ),
            ("units.incn", "pub model Size:\n    pub n: int\n"),
            (
                "protocol.incn",
                "pub trait Same:\n    def value(self) -> Option[int]:\n        return None\n",
            ),
        ],
        b"12\ntrue\n6\n5\nlocal 1\nremote none\n",
    )
}

/// A generic adopter whose inherited default still carries open trait arguments refuses before native compilation,
/// rather than surfacing a driver error for an invalid implementation record.
#[test]
fn open_generic_trait_default_is_refused() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let root = fixture.scratch("open-trait-default")?;
    let source = root.join("cell.incn");
    fs::write(
        &source,
        "trait Read[T]:\n    def value(self) -> T: ...\n    def get(self) -> T:\n        return self.value()\n\nmodel Cell[T] with Read[T]:\n    item: T\n    def value(self) -> T:\n        return self.item\n\ndef main() -> None:\n    println(Cell(item=7).get())\n",
    )?;
    let closure = corpus::runtime_closure(&fixture.formatting)?;
    let binary = root.join("native");
    let refused = corpus::source_command(
        &fixture.driver_binary("debug"),
        &source,
        &binary,
        &fixture.sysroot,
        &closure,
    )
    .output()?;
    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("unsupported Body IR open trait default type arguments"),
        "{}",
        String::from_utf8_lossy(&refused.stderr)
    );
    assert!(!binary.exists());
    Ok(())
}

/// Checked Clone calls keep builtin values, union carriers and Option carriers independent of their original fields.
#[test]
fn builtin_and_carrier_clone_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("builtin-carrier-clone")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"from std.derives.copying import Clone

@derive(Clone)
model Left:
    value: int

@derive(Clone)
model Right:
    value: str

type Choice = Union[Left, Right]

model Packet:
    choice: Choice
    maybe: Option[Left]

def copied[T with Clone](value: T) -> T:
    return value.clone()

def main() -> None:
    packet = Packet(choice=Left(value=1), maybe=Some(Left(value=2)))
    choice = packet.choice.clone()
    maybe = packet.maybe.clone()
    text = "text"
    println(copied(text))
    number = 7
    println(copied(number))
    println(maybe.unwrap().value)
    match choice:
        Left(left) => println(left.value)
        Right(right) => println(right.value)
"#,
    )
}

/// An explicit `clone()` the checker selects from the standard library's Clone copies a model, a class, an enum and a
/// newtype through each one's derived implementation, on a value and on `self`, and the copy is independent.
#[test]
fn derived_clone_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    check_project_case(
        "derived_clone",
        r#"from std.derives.copying import Clone


@derive(Clone)
model Counter:
    value: int
    label: str

    def copied(self) -> Counter:
        return self.clone()


class Box:
    pub size: int


enum Color:
    Red
    Blue


type Name = newtype str


def main() -> None:
    original = Counter(value=1, label="a")
    mut copy = original.clone()
    copy.value = 5
    println(f"{original.value} {copy.value} {copy.label}")
    println(original.copied().value)
    println(Box(size=2).clone().size)
    println(Color.Red.clone() == Color.Red)
    println(Name("n").clone().0)
"#,
        b"1 5 a\n1\n2\ntrue\nn\n",
    )
}

/// A trait default with its own type parameters runs for an adopter at each call's checked arguments: one reads an
/// element and a slice of a list of the parameter, another returns the argument it was given.
#[test]
fn trait_generic_method_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    check_project_case(
        "trait_generic_method",
        r#"trait Picker:
    def pick[K](self, items: list[K]) -> K:
        return items[0]

    def tail[K](self, items: list[K]) -> list[K]:
        return items[1:]


trait Echo:
    def echo[T with Clone](self, value: T) -> T:
        return value


model Chooser with Picker, Echo:
    id: int


def main() -> None:
    chooser = Chooser(id=1)
    println(chooser.pick([5, 6]))
    println(chooser.pick(["z"]))
    println(len(chooser.tail([7, 8, 9])))
    println(chooser.echo(true))
    println(chooser.echo("text"))
"#,
        b"5\nz\n2\ntrue\ntext\n",
    )
}

/// A declared type's own method returning `Self` returns that type, whether a class rebuilds itself in a method named
/// `clone` or a model returns a moved copy, and its callers read the returned value's fields.
#[test]
fn own_method_self_result_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    check_project_case(
        "own_method_self_result",
        r#"pub class ActiveRegistration:
    pub logical_name: str
    pub rank: int

    def clone(self) -> Self:
        return ActiveRegistration(logical_name=self.logical_name, rank=self.rank)


model Point:
    x: int

    def moved(self, dx: int) -> Self:
        return Point(x=self.x + dx)


def main() -> None:
    reg = ActiveRegistration(logical_name="orders", rank=1)
    copied = reg.clone()
    println(copied.logical_name)
    println(f"{reg.logical_name} {copied.rank}")
    println(Point(x=1).moved(2).moved(3).x)
"#,
        b"orders\norders 1\n6\n",
    )
}

/// An explicit comparison dunder the checker selects from the standard library's Eq or Ord is the operator its derive
/// implements, on a model, on `self`, on a model with type parameters and on a newtype; a fieldless enum deriving Ord
/// orders its variants by declaration, through a dunder and through the operators.
#[test]
fn comparison_dunders_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    check_project_case(
        "comparison_dunders",
        r#"@derive(Eq, Ord)
model Score:
    points: int

    def at_least(self, other: Score) -> bool:
        return self.__ge__(other)


@derive(Eq, Ord)
enum Level:
    Low
    High


@derive(Eq, Ord)
model Pair[T]:
    left: T
    right: T


@derive(Eq, Ord)
type Rank = newtype int


def main() -> None:
    a = Score(points=2)
    b = Score(points=3)
    println(a.__ge__(b))
    println(a.__le__(b))
    println(a.__gt__(b))
    println(a.__lt__(b))
    println(a.__ne__(b))
    println(a.__eq__(b))
    println(a.__ge__(b) == (a >= b))
    println(b.at_least(a))
    println(Level.High.__gt__(Level.Low))
    println(Level.Low < Level.High)
    println(Level.High <= Level.Low)
    println(Pair(left=1, right=2).__lt__(Pair(left=1, right=3)))
    println(Rank(1).__ge__(Rank(2)))
"#,
        b"false\ntrue\nfalse\ntrue\ntrue\nfalse\ntrue\ntrue\ntrue\ntrue\nfalse\ntrue\nfalse\n",
    )?;
    // The checker chose the plain operator while the operand was a type parameter; at a text instantiation that
    // operator needs a runtime operation it never selected, so the native route refuses by name.
    check_project_refusal(
        "comparison_dunder_text_instance",
        r#"from std.derives.comparison import Ord


def less[T with Ord](a: T, b: T) -> bool:
    return a.__lt__(b)


def main() -> None:
    println(less("a", "b"))
"#,
        "unsupported Body IR binary operator on text or nominal operands",
    )
}

/// A value received through a standard callable bound is called like the function or closure it holds: a function
/// item, a closure, and a two-argument closure through `Callable2`.
#[test]
fn callable_bound_call_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    check_project_case(
        "callable_bound_call",
        r#"from std.traits.callable import Callable1, Callable2


def add_seven(value: int) -> int:
    return value + 7


def run_std[F with Callable1[int, int]](func: F, value: int) -> int:
    return func(value)


def combine[F with Callable2[int, int, int]](func: F, left: int, right: int) -> int:
    return func(left, right)


def main() -> None:
    println(run_std((value) => value * 3, 3))
    println(run_std(add_seven, 3))
    println(combine((left, right) => left * right, 6, 7))
"#,
        b"9\n10\n42\n",
    )
}

/// Through an Eq, Ord or Display bound, a standard slot does what the type gives it: a number compares by value, text
/// by its equality, an Ord adopter's own `__lt__` runs as written, and `__str__` renders a number and a derived
/// Display.
#[test]
fn std_protocol_slot_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    check_project_case(
        "std_protocol_slot",
        r#"from std.derives.comparison import Eq, Ord


model Reversed with Ord:
    value: int

    def __eq__(self, other: Self) -> bool:
        return self.value == other.value

    def __lt__(self, other: Self) -> bool:
        return self.value > other.value


@derive(Display)
model Point:
    x: int


def differ[T with Eq](a: T, b: T) -> bool:
    return a.__ne__(b)


def equal[T with Eq](a: T, b: T) -> bool:
    return a.__eq__(b)


def less[T with Ord](a: T, b: T) -> bool:
    return a.__lt__(b)


def at_most[T with Ord](a: T, b: T) -> bool:
    return a.__le__(b)


def text[T with Display](value: T) -> str:
    return value.__str__()


def main() -> None:
    println(differ(1, 2))
    println(equal("a", "a"))
    println(less(1, 2))
    println(at_most(2, 2))
    println(less(Reversed(value=2), Reversed(value=1)))
    println(text(3))
    println(text(Point(x=1)))
"#,
        b"true\ntrue\ntrue\ntrue\ntrue\n3\nPoint { x: 1 }\n",
    )
}

/// A local trait whose supertrait comes from the standard library serves that supertrait's slot from its default: a
/// `Clone` supertrait's `clone()` copies through the adopter's derived Clone, on the default's `self` and through a
/// bound on the subtrait.
#[test]
fn imported_supertrait_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    check_project_case(
        "imported_supertrait",
        r#"trait Copyable with Clone:
    def dup(self) -> Self:
        return self.clone()


model Copied with Copyable:
    value: int


def twin[T with Copyable](value: T) -> T:
    return value.dup()


def main() -> None:
    original = Copied(value=5)
    mut copy = original.dup()
    copy.value = 6
    println(f"{original.value} {copy.value}")
    println(twin(Copied(value=7)).value)
"#,
        b"5 6\n7\n",
    )
}

/// Result's builtin combinators call their callback on one side and pass the other side through: a named function, a
/// callable model, a capturing closure and a closure over a model payload all run, a receiver read again afterwards is
/// unchanged, and a source model named `Box` does not shadow a boxed closure.
#[test]
fn result_combinator_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    check_project_case(
        "result_combinator",
        r#"from std.traits.callable import Callable1


model Box:
    tag: str


model Observer with Callable1[int, None]:
    def __call__(self, value: int) -> None:
        println(f"observer {value}")


def double(value: int) -> int:
    return value * 2


def shout(message: str) -> str:
    return f"{message}!"


def lower(value: int) -> Result[int, str]:
    if value == 0:
        return Err("zero")
    return Ok(value - 1)


def recover(message: str) -> Result[int, str]:
    return Ok(len(message))


def observe_box(item: Box) -> None:
    println(f"box {item.tag}")


def show(value: Result[int, str]) -> str:
    match value:
        Ok(number) => return f"Ok({number})"
        Err(message) => return f"Err({message})"


def main() -> None:
    ok: Result[int, str] = Ok(4)
    bad: Result[int, str] = Err("bad")
    println(show(ok.map(double)))
    println(show(bad.map_err(shout)))
    println(show(ok.and_then(lower)))
    println(show(bad.or_else(recover)))
    println(show(ok))
    observer = Observer()
    ok.inspect(observer)
    suffix = "?"
    bad.inspect_err((message) => println(message + suffix))
    boxed: Result[Box, str] = Ok(Box(tag="t"))
    boxed.inspect(observe_box)
    boxed.inspect((item) => println(item.tag + "!"))
"#,
        b"Ok(8)\nErr(bad!)\nOk(3)\nOk(3)\nOk(4)\nobserver 4\nbad?\nbox t\nt!\n",
    )
}

/// A single-module program whose model and enum adopt the standard library's Ord validates their imported protocol
/// slots, which their own `__eq__` and `__lt__` fill, as a project module's do: the dunders and the `<` operator run.
#[test]
fn single_module_std_protocol_slot_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    check_project_case(
        "single_module_std_protocol_slot",
        r#"from std.derives.comparison import Ord


model Ordered with Ord:
    v: int

    def __eq__(self, other: Self) -> bool:
        return self.v == other.v

    def __lt__(self, other: Self) -> bool:
        return self.v < other.v


enum Level with Ord:
    Low
    High

    def __eq__(self, other: Self) -> bool:
        return self.rank() == other.rank()

    def __lt__(self, other: Self) -> bool:
        return self.rank() < other.rank()

    def rank(self) -> int:
        match self:
            Level.Low => return 0
            Level.High => return 1


def main() -> None:
    println(Ordered(v=1).__lt__(Ordered(v=2)))
    println(Ordered(v=1) < Ordered(v=2))
    println(Level.High.__lt__(Level.Low))
    println(Level.Low.__eq__(Level.Low))
    println(Level.High < Level.Low)
"#,
        b"true\ntrue\nfalse\ntrue\nfalse\n",
    )
}

/// An Ord adopter that imports only Ord reaches the `__ne__` default of the Eq supertrait, the body the SDK publishes,
/// on a model, on an enum and through a type parameter bounded by Ord; the default calls the adopter's own `__eq__`.
#[test]
fn std_supertrait_default_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    check_project_case(
        "std_supertrait_default",
        r#"from std.derives.comparison import Ord


model Ordered with Ord:
    v: int

    def __eq__(self, other: Self) -> bool:
        return self.v == other.v

    def __lt__(self, other: Self) -> bool:
        return self.v < other.v


enum Level with Ord:
    Low
    High

    def __eq__(self, other: Self) -> bool:
        return self.rank() == other.rank()

    def __lt__(self, other: Self) -> bool:
        return self.rank() < other.rank()

    def rank(self) -> int:
        match self:
            Level.Low => return 0
            Level.High => return 1


def differ[T with Ord](a: T, b: T) -> bool:
    return a.__ne__(b)


def main() -> None:
    println(Ordered(v=1).__ne__(Ordered(v=2)))
    println(Ordered(v=1).__ne__(Ordered(v=1)))
    println(Level.Low.__ne__(Level.High))
    println(differ(Ordered(v=3), Ordered(v=3)))
"#,
        b"true\nfalse\ntrue\nfalse\n",
    )
}

/// Published Ord defaults compare imported adopters under aliased and qualified trait spellings, including a
/// user override; stdout must be byte-identical to the frozen legacy route.
#[test]
fn imported_comparison_defaults_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let root = fixture.scratch("imported_comparison_defaults")?;
    let corpus_root = support::fixtures_dir()
        .join("behavior/lowering_declarations/comparison_dunders_of_an_adopter_from_another_module");
    let project = root.join("builtin-parity");
    fs::create_dir_all(&project)?;
    fs::copy(corpus_root.join("scores.incn"), project.join("scores.incn"))?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &root,
        &fixture.sysroot,
        &fixture.formatting,
        &fs::read_to_string(corpus_root.join("main.incn"))?,
    )
}

/// Compile a single-module program in the minimal project layout and require the native route to refuse it by name.
fn check_project_refusal(name: &str, text: &str, reason: &str) -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let root = fixture.scratch(name)?;
    let main = support::cli_project::write_minimal_project(&root, name, "")?;
    fs::write(&main, text)?;
    let native = root.join("native");
    let closure = corpus::runtime_closure(&fixture.formatting)?;
    let mut command = corpus::source_command(
        &fixture.driver_binary("debug"),
        &main,
        &native,
        &fixture.sysroot,
        &closure,
    );
    command.current_dir(&root);
    let refused = command.output()?;
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(!refused.status.success(), "{stderr}");
    assert!(stderr.contains(reason), "{stderr}");
    assert!(!native.exists());
    Ok(())
}

/// Compare a single-module program in the minimal project layout behavior fixtures run in, pinning its output.
///
/// The fixture harness runs a single-file program as `src/main.incn` of a minimal project through `incan run`, and
/// that is the legacy reference. A standalone legacy build of the same file is not: it qualifies a call to a
/// supertrait's method on a concrete adopter as `crate::main::Trait::method`, which only resolves in the project
/// layout.
fn check_project_case(name: &str, text: &str, expected_stdout: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    check_project_modules_case(name, text, &[], expected_stdout)
}

/// Imported generic Json is the user's model storage, independently of the stdlib wrapper's field shortcut.
#[test]
fn imported_user_json_field_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    check_project_modules_case(
        "imported_user_json_field",
        "from models import Json\n\ndef read(wrapper: Json[int]) -> int:\n  return wrapper.value\n\ndef main() -> None:\n  println(read(Json[int](value=5)))\n",
        &[("models.incn", "pub model Json[T]:\n  pub value: T\n")],
        b"5\n",
    )
}

/// A call returning a provider's private model alias retains the declared carrier in the consumer's native frame.
#[test]
fn imported_model_alias_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    check_project_modules_case(
        "imported_model_alias",
        "from factory import make_aliased\nfrom models import Account\n\ndef main() -> None:\n  value = make_aliased()\n  println(\"x\")\n",
        &[
            ("models.incn", "pub model Account:\n  pub type_ [alias=\"type\"]: str\n"),
            (
                "factory.incn",
                "from models import Account\nfrom models import Account as A\n\npub def make_aliased() -> A:\n  return A(type=\"x\")\n",
            ),
        ],
        b"x\n",
    )
}

/// Compare a program of several modules in the minimal project layout, pinning its output.
///
/// `modules` names further source files beside `src/main.incn`, each with its text, as a multi-module behavior
/// fixture lays them out.
fn check_project_modules_case(
    name: &str,
    text: &str,
    modules: &[(&str, &str)],
    expected_stdout: &[u8],
) -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let root = fixture.scratch(name)?;
    let main = support::cli_project::write_minimal_project(&root, name, "")?;
    fs::write(&main, text)?;
    let source = main.parent().ok_or("project main has no source directory")?;
    for (file, module) in modules {
        fs::write(source.join(file), module)?;
    }
    let native = root.join("native");
    let mut closure = corpus::runtime_closure(&fixture.formatting)?;
    closure.extend_with(&sdk_closure(fixture)?)?;
    let mut command = corpus::source_command(
        &fixture.driver_binary("debug"),
        &main,
        &native,
        &fixture.sysroot,
        &closure,
    );
    command.current_dir(&root);
    success(&command.output()?, "native project compilation");
    let expected = support::repo_command()
        .current_dir(&root)
        .args(["run", "src/main.incn"])
        .output()?;
    let actual = Command::new(native).output()?;
    success(&expected, "legacy project execution");
    success(&actual, "native project execution");
    assert_eq!(actual.stdout, expected.stdout);
    assert_eq!(actual.stdout, expected_stdout);
    Ok(())
}

/// A stdlib model is the SDK crate's own type: constructed, returned as a Result error, read by field and asked a
/// method, while stdlib functions and methods run in the crates that compiled them.
#[test]
fn stdlib_layouts_and_crate_calls_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    check_project_case(
        "stdlib_layouts_and_crate_calls",
        r#"from std.uuid import UuidError
from std.datetime.civil.naive import ordinal_key_byte


def check(text: str) -> Result[int, UuidError]:
    if len(text) < 3:
        return Err(UuidError(kind="invalid_length", detail="short"))
    return Ok(len(text))


def describe(error: UuidError) -> str:
    return f"{error.kind}: {error.detail}"


def main() -> None:
    match check("ab"):
        Ok(count) => println(f"ok {count}")
        Err(error) => println(describe(error))
    match check("abc"):
        Ok(count) => println(f"ok {count}")
        Err(error) => println(describe(error))
    error = UuidError(kind="invalid_bytes", detail="raw input")
    println(error.message())
    println(len(ordinal_key_byte(7)))
"#,
        b"invalid_length: short\nok 3\nraw input\n1\n",
    )
}

/// An `Ok(...)` or `Err(...)` whose payload is one member of the union its destination's side declares is that
/// union's member, as a call argument and as a returned value.
#[test]
fn result_constructors_into_union_sides_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    check_project_case(
        "result_constructors_into_union_sides",
        r#"def kind(value: Result[int | str, str]) -> str:
    match value:
        Ok(member) =>
            if isinstance(member, int):
                return f"int {member}"
            if isinstance(member, str):
                return f"text {member}"
            return "other"
        Err(message) => return f"error {message}"


def made(flag: bool) -> Result[int | str, str]:
    if flag:
        return Ok(7)
    return Err("none")


def main() -> None:
    println(kind(Ok("fine")))
    println(kind(Ok(3)))
    println(kind(Err("bad")))
    println(kind(made(true)))
    println(kind(made(false)))
"#,
        b"text fine\nint 3\nerror bad\nint 7\nerror none\n",
    )
}

/// A union side that names a newtype takes that newtype's checked identity when an `Ok(...)` member widens into it.
#[test]
fn result_constructors_into_nominal_union_sides_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    check_project_case(
        "result_constructors_into_nominal_union_sides",
        r#"@derive(Clone)
type Kind = newtype str


def kind(value: Result[Kind | str, str]) -> str:
    match value:
        Ok(member) =>
            if isinstance(member, Kind):
                return f"kind {member.0}"
            if isinstance(member, str):
                return f"text {member}"
            return "other"
        Err(message) => return f"error {message}"


def main() -> None:
    println(kind(Ok("fine")))
    println(kind(Ok(Kind("k"))))
    println(kind(Err("bad")))
"#,
        b"text fine\nkind k\nerror bad\n",
    )
}

/// An Option method called on a model field reads that field's carrier and leaves the model whole, for a copied
/// payload and for an owned text payload alike.
#[test]
fn carrier_methods_on_model_fields_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    check_project_case(
        "carrier_methods_on_model_fields",
        r#"model Counter:
    count: Option[int] = None
    name: Option[str] = None


def main() -> None:
    mut counter = Counter(count=1)
    println(counter.count.unwrap_or(0))
    println(counter.name.unwrap_or("none"))
    counter.name = "b"
    println(counter.name.unwrap_or("none"))
    println(counter.count.unwrap_or(0))
"#,
        b"1\nnone\nb\n1\n",
    )
}

/// A stdlib model built with omitted fields takes its published defaults, and a stdlib error type that reaches the
/// program only through a stdlib method's result keeps its own type; a local model's Option field default works too.
#[test]
fn stdlib_defaults_and_implied_types_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    check_project_case(
        "stdlib_defaults_and_implied_types",
        r#"from std.io import BytesIO, IoError


model Entry:
    pub name: str
    pub note: Option[str] = None


def describe(entry: Entry) -> str:
    match entry.note:
        Some(text) => return text
        None => return "none"


def main() -> None:
    error = IoError(kind="bad", detail="detail")
    println(f"{error.kind} {error.operation} {error.position}")
    match BytesIO(b"AB").read_exact(4):
        Ok(data) => println(len(data))
        Err(err) => println(err.kind)
    match BytesIO(b"AB").read_exact(2):
        Ok(data) => println(len(data))
        Err(err) => println(err.kind)
    println(describe(Entry(name="a")))
    println(describe(Entry(name="b", note=Some("given"))))
"#,
        b"bad io -1\nunexpected_eof\n2\nnone\ngiven\n",
    )
}

/// A stdlib enum is matched and passed as the SDK crate's own enum, and a stdlib newtype is built, read and called
/// as the SDK crate's own struct; both declarations come from the representations the SDK packages published.
#[test]
fn stdlib_enums_and_newtypes_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    check_project_case(
        "stdlib_enums_and_newtypes",
        r#"from std.fs import Path
from std.io import Endian


def order_name(order: Endian) -> str:
    match order:
        Endian.Little => return "little"
        Endian.Big => return "big"


def main() -> None:
    println(order_name(Endian.Little))
    println(order_name(Endian.Big))
    config = Path("config").joinpath("app.toml")
    println(config.0)
    println(f"{config.name()} {config.suffix()} {config.parent().0}")
"#,
        b"little\nbig\nconfig/app.toml\napp.toml .toml config\n",
    )
}

/// A generic stdlib function runs as its SDK crate item instantiated with each call's checked type arguments, so one
/// function serves an integer, a text and a model instantiation in the same program, and returns its parameter type.
#[test]
fn stdlib_generic_functions_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    check_project_case(
        "stdlib_generic_functions",
        r#"from std.testing import assert_eq, assert_is_some


@derive(Eq, Debug)
model Pair:
    x: int


def main() -> None:
    assert_eq(1 + 1, 2, "sum")
    assert_eq("a", "a", "text")
    assert_eq(Pair(x=1), Pair(x=1), "pair")
    value = assert_is_some(Some(5), "present")
    println(value)
    println("ok")
"#,
        b"5\nok\n",
    )
}

/// Stdlib Result combinators retain all checked type arguments, including the callback, and apply that callback only
/// to the matching variant; their native output is byte-identical to legacy for both Result variants.
#[test]
fn stdlib_result_combinators_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("stdlib-result-combinators")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"from std.result import map, map_err, and_then


def parse(text: str) -> Result[int, str]:
    if text == "7":
        return Ok(7)
    return Err(f"bad {text}")


def double(value: int) -> int:
    println("double")
    return value * 2


def shout(error: str) -> str:
    println("shout")
    return f"{error}!"


def reparse(value: int) -> Result[int, str]:
    println("reparse")
    return parse(str(value))


def show(result: Result[int, str]) -> None:
    match result:
        Ok(value) => println(value)
        Err(error) => println(error)


def main() -> None:
    show(map(parse("7"), double))
    show(map(parse("x"), double))
    show(map_err(parse("x"), shout))
    show(map_err(parse("7"), shout))
    show(and_then(parse("7"), reparse))
    show(and_then(parse("y"), reparse))
"#,
    )
}

/// Stdlib static methods and generic assertion helpers pass their declaration-owned omitted defaults in the three
/// fixtures this defaults slice admits, with output byte-identical to legacy.
#[test]
fn stdlib_omitted_defaults_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    for (name, source) in [
        (
            "condition-defaults",
            include_str!(
                "../../incan_test_support/fixtures/behavior/emit_calls_and_builtins/struct_literals_in_condition_positions.incn"
            ),
        ),
        (
            "static-method-defaults",
            include_str!("../../incan_test_support/fixtures/behavior/snapshots_stdlib/imported_stdlib_defaults.incn"),
        ),
        (
            "assertion-defaults",
            include_str!("../../incan_test_support/fixtures/behavior/snapshots_stdlib/std_testing_assertions.incn"),
        ),
    ] {
        corpus::check_builtin_source(
            &fixture.driver_binary("debug"),
            &fixture.scratch(name)?,
            &fixture.sysroot,
            &fixture.formatting,
            source,
        )?;
    }
    Ok(())
}

/// Expanding the imported const default reaches the collection-equality boundary, which remains a named refusal
/// until that independent slice retains shared reads for owned bytes returned by stdlib calls.
#[test]
fn stdlib_const_default_reaches_collection_equality_refusal() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let root = fixture.scratch("stdlib-const-default-boundary")?;
    let source = root.join("default.incn");
    fs::write(
        &source,
        include_str!(
            "../../incan_test_support/fixtures/behavior/cli_values_and_calls/stdlib_const_default_omitted.incn"
        ),
    )?;
    let closure = corpus::runtime_closure(&fixture.formatting)?;
    let binary = root.join("native");
    let refused = corpus::source_command(
        &fixture.driver_binary("debug"),
        &source,
        &binary,
        &fixture.sysroot,
        &closure,
    )
    .output()?;
    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stderr)
            .contains("unsupported Body IR collection equality without retained shared read"),
        "{}",
        String::from_utf8_lossy(&refused.stderr)
    );
    assert!(!binary.exists());
    Ok(())
}

/// Same-named methods targeting distinct traits on a raw newtype run their selected implementation with output
/// byte-identical to legacy.
#[test]
fn newtype_trait_targets_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("newtype-trait-targets")?,
        &fixture.sysroot,
        &fixture.formatting,
        include_str!(
            "../../incan_test_support/fixtures/behavior/snapshots_traits_and_generics/newtype_trait_targets.incn"
        ),
    )
}

/// Retaining a generic newtype layout does not authorize an unresolved native parameter carrier; both pending
/// generic fixtures keep their named refusals and produce no binary.
#[test]
fn generic_newtypes_retain_parameter_carrier_refusals() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let closure = corpus::runtime_closure(&fixture.formatting)?;
    for (name, source, expected) in [
        (
            "generic-newtype",
            include_str!(
                "../../incan_test_support/fixtures/behavior/snapshots_newtypes_and_serde/newtype_generic.incn"
            ),
            "unsupported Body IR type Box[TypeVar]",
        ),
        (
            "generic-newtype-methods",
            include_str!(
                "../../incan_test_support/fixtures/behavior/snapshots_traits_and_generics/generic_methods.incn"
            ),
            "unsupported Body IR type Wrapper[int]",
        ),
    ] {
        let root = fixture.scratch(name)?;
        let input = root.join("generic.incn");
        fs::write(&input, source)?;
        let binary = root.join("native");
        let refused = corpus::source_command(
            &fixture.driver_binary("debug"),
            &input,
            &binary,
            &fixture.sysroot,
            &closure,
        )
        .output()?;
        assert!(!refused.status.success());
        assert!(
            String::from_utf8_lossy(&refused.stderr).contains(expected),
            "{}",
            String::from_utf8_lossy(&refused.stderr)
        );
        assert!(!binary.exists());
    }
    Ok(())
}

/// A call to an overloaded stdlib function runs the overload the checker selected by arity, as that overload's own SDK
/// crate item instantiated with the call's type argument, beside a call to a function with one declaration.
#[test]
fn stdlib_overloaded_functions_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    check_project_case(
        "stdlib_overloaded_functions",
        r#"from std.environ import EnvironError, get_as, get_optional

const KEY: str = "INCAN_DIRECT_ROUTE_UNSET_OVERLOAD_KEY"


def read() -> Result[None, EnvironError]:
    optional: Option[int] = get_as[int](KEY)?
    match optional:
        Some(value) => println(value)
        None => println("absent")
    defaulted: int = get_as[int](KEY, 7)?
    println(defaulted)
    match get_optional(KEY):
        Some(value) => println(value)
        None => println("fallback")
    return Ok(None)


def main() -> None:
    match read():
        Ok(_) => pass
        Err(error) => println(error.message())
"#,
        b"absent\n7\nfallback\n",
    )
}

/// A stdlib method's `Result` error that its module imports from another stdlib module is that crate's own type: a
/// failed construction's error is read by field beside a successful construction's fields.
#[test]
fn stdlib_imported_signature_types_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    check_project_case(
        "stdlib_imported_signature_types",
        r#"from std.datetime.civil.naive import Date


def show(year: int, month: int, day: int) -> None:
    match Date.from_fields(year, month, day):
        Ok(date) => println(f"{date.year}-{date.month}-{date.day}")
        Err(error) => println(error.detail)


def main() -> None:
    show(2024, 2, 29)
    show(2024, 13, 1)
    show(2023, 2, 29)
"#,
        b"2024-2-29\nmonth must be in 1..12\nday is out of range for month\n",
    )
}

/// A stdlib class whose crate also has a same-named constructor function, such as `EnvironError`, is the crate's own
/// struct: here it is the error side of a stdlib call and of a `Result` main.
#[test]
fn stdlib_class_beside_its_constructor_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    check_project_case(
        "stdlib_class_beside_its_constructor",
        r#"from std.environ import EnvironError, args


def subcommand(arguments: list[str]) -> str:
    if len(arguments) < 2:
        return "help"
    return arguments[1]


def main() -> Result[None, EnvironError]:
    arguments = args()?
    println(subcommand(arguments))
    return Ok(None)
"#,
        b"help\n",
    )
}

/// A `main` returning `Result[None, E]` is the binary's entry point as legacy's is: its output before an `Err` is kept,
/// and the error is reported on stderr with a failing exit code.
#[test]
fn result_main_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let (actual, _expected) = declaration_case_outputs(
        "result_main_err",
        r#"def main() -> Result[None, str]:
    println("before failure")
    return Err("failed on purpose")
"#,
    )?;
    assert_eq!(actual.stdout, b"before failure\n");
    assert_eq!(actual.stderr, b"Error: \"failed on purpose\"\n");
    assert_eq!(actual.status.code(), Some(1));
    let (actual, _expected) = declaration_case_outputs(
        "result_main_ok",
        "def main() -> Result[None, str]:\n    println(\"done\")\n    return Ok(None)\n",
    )?;
    assert_eq!(actual.stdout, b"done\n");
    assert_eq!(actual.status.code(), Some(0));
    Ok(())
}

/// Same-module functions sharing one name are distinct overloads: each call runs the one the checker selected by
/// parameter type or arity.
#[test]
fn local_overloaded_functions_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    check_declaration_case(
        "local_overloaded_functions",
        r#"def convert(value: int) -> int:
    return value + 1


def convert(value: str) -> str:
    return value + "!"


def convert(value: int, step: int) -> int:
    return value + step


def main() -> None:
    println(convert(1))
    println(convert("a"))
    println(convert(1, 10))
"#,
        None,
    )
}

/// `list.repeat` builds a list of clones of its value through legacy's runtime helper: scalars, text read from a
/// binding that stays usable, nested lists, models, a zero count and arguments written by name.
#[test]
fn list_repeat_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    check_declaration_case(
        "list_repeat_helper",
        r#"@derive(Clone)
model Point:
    x: int


def main() -> None:
    ints: List[int] = list.repeat(-1, 4)
    seed = "seed"
    words: list[str] = list.repeat(seed, 2)
    empty: List[int] = list.repeat(0, 0)
    nested: list[list[int]] = list.repeat([1], 2)
    named: list[int] = list.repeat(count=3, value=7)
    points = list.repeat(Point(x=4), 2)
    println(f"{ints[0]} {len(ints)} {words[1]} {seed} {len(empty)} {nested[1][0]} {named[2]} {points[1].x}")
"#,
        None,
    )
}

/// The adopted math unit is an explicit receipt-selected dependency on both native routes.
#[test]
fn direct_route_native_sdk_math_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    check_project_case(
        "native_sdk_math",
        "from std.math import sqrt\n\ndef main() -> None:\n    println(sqrt(81.0))\n",
        b"9.0\n",
    )
}

/// Explicit Debug formatting preserves scalar spelling, text escaping, and repeated list reads.
#[test]
fn scalar_and_list_debug_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    check_declaration_case(
        "scalar_and_list_debug",
        r#"def show(mut items: list[int]) -> None:
    println(f"{items:?}")
    items.append(3)
    println(f"{items:?}")
    println(f"{items:?}")

def main() -> None:
    text = "quoted \"text\"\nnext"
    whole: float = 1.0
    negative: float = -0.0
    fraction: float = 1.5
    flag = true
    count = -7
    small: u8 = 255
    println(f"{text:?} {text:?}")
    println(text)
    println(f"{whole:?} {negative:?} {fraction:?}")
    println(f"{flag:?} {count:?} {small:?}")
    println(f"{1.5:?} {false:?} {42:?}")
    mut items = [1, 2]
    show(items)
    println(f"{items:?}")
    nested = [[1], [2, 3]]
    println(f"{nested:?}")
    println(f"{nested:?}")
"#,
        None,
    )
}

/// Canonical nested field paths preserve reads, writes, receiver borrowing, and constructor evaluation order.
#[test]
fn nested_model_fields_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    check_declaration_case(
        "nested_model_fields",
        r#"model Leaf:
    text: str
    count: int

model Middle:
    leaf: Leaf

model Holder:
    middle: Middle

    def text(self) -> str:
        return self.middle.leaf.text

    def rename(mut self, text: str) -> None:
        self.middle.leaf.text = text
        self.middle.leaf.count += 1

def effect(label: str) -> str:
    println(label)
    return label

def main() -> None:
    mut value = Holder(middle=Middle(leaf=Leaf(count=3, text=effect("first"))))
    println(value.middle.leaf.text)
    println(value.text())
    value.middle.leaf.text = effect("second")
    println(value.middle.leaf.text)
    value.rename("third")
    println(value.middle.leaf.text)
    println(value.middle.leaf.count)
    println(value.text())
"#,
        None,
    )
}

/// Model list fields use legacy index and slice semantics while preserving their owner and copied results.
#[test]
fn model_list_projection_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    check_declaration_case(
        "model_list_projections",
        r#"
model Bucket:
    values: list[int]
    words: list[str]
    rows: list[list[int]]

    def last(self) -> int:
        return self.values[-1]

def bound(label: str, value: int) -> int:
    println(label)
    return value

def main() -> None:
    bucket = Bucket(values=[40, 2, 7, 9], words=["first", "second"], rows=[[1, 2], [3, 4]])
    println(bucket.values[0])
    println(bucket.last())
    println(bucket.values[bound("index", -2)])
    println(bucket.values[bound("start", 0):bound("end", 3):bound("step", 2)])
    println(bucket.values[:])
    println(bucket.values[::-1])
    println(bucket.values[-3:-1])
    println(bucket.values[50:100])
    println(bucket.words[0])
    mut copied = bucket.words[:1]
    copied[0] = "changed"
    println(copied)
    println(bucket.words)
    println(bucket.rows[1][-1])
    println(bucket.rows[:1])
    println(bucket.values)
"#,
        None,
    )
}

/// List element reads in values, tuples, text helpers, numeric members and `len` copy the element like legacy does.
#[test]
fn list_element_read_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("list-element-reads")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"model Label:
    text: str

def score(mut values: list[int]) -> int:
    values[0] = 40
    pair = (values[0], values[-1])
    return pair.0 + pair.1

def find(words: list[str], size: int) -> int:
    mut index = 0
    while index < len(words):
        if len(words[index]) == size:
            return index
        index += 1
    return -1

def lookup(rows: list[list[str]], key: str) -> int:
    for row in rows:
        if row[0] == key:
            return len(row)
    return 0

def finite(values: list[float]) -> bool:
    return values[0].is_finite() and not values[-1].is_nan()

def main() -> None:
    mut values = [1, 2]
    println(score(values))
    println(values)
    println(find(["a", "bb", "ccc"], 2))
    rows = [[], ["x"]]
    println(len(rows[0]))
    println(len(rows[1]))
    deep = [[[]], [["x"]]]
    println(len(deep[1][0]))
    println(lookup([["a", "1"], ["b", "2", "3"]], "b"))
    println(finite([0.5, 1.5]))
    label = Label(text="a")
    names: list[str] = ["a", "b"]
    println(names[0] == label.text)
    println(names[1] != label.text)
    mut cells = [10, 20, 30]
    cells[0], fresh = (cells[2], cells[0])
    println(f"{cells[0]} {fresh}")
    cells[1], cells[2] = (cells[2], cells[1])
    println(cells)
"#,
    )
}

/// Element writes after a model field and `mut self` calls on list and dictionary elements change the stored element,
/// as legacy's in-place element borrows do, while a shared receiver reads a copy.
#[test]
fn list_element_mutation_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("list-element-mutations")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"class Counter:
    pub count: int

    def bump(mut self) -> None:
        self.count += 1

    def total(self) -> int:
        return self.count

class Grid:
    pub cells: list[int]
    pub counters: list[Counter]

    def swap_cells(mut self) -> None:
        self.cells[0], self.cells[1] = (self.cells[1], self.cells[0])

    def bump_last(mut self) -> None:
        self.counters[-1].bump()

class Wrap:
    pub inner: Counter

def main() -> None:
    mut grid = Grid(cells=[7, 8], counters=[Counter(count=0), Counter(count=5)])
    grid.swap_cells()
    println(grid.cells)
    grid.cells[1] = 3
    println(grid.cells[1])
    grid.bump_last()
    grid.counters[0].bump()
    println(grid.counters[0].count)
    println(grid.counters[1].total())
    mut counters = [Counter(count=0)]
    counters[0].bump()
    counters[-1].bump()
    println(counters[0].count)
    mut wraps = [Wrap(inner=Counter(count=0))]
    wraps[0].inner.bump()
    println(wraps[0].inner.count)
    mut table: dict[str, Counter] = {"a": Counter(count=0)}
    table["a"].bump()
    println(table["a"].count)
    mut rows = [[Counter(count=1)]]
    rows[0][0].bump()
    println(rows[0][0].count)
"#,
    )
}

/// `isinstance` over an Option is true only for a present payload of the target type, through a union payload too.
#[test]
fn option_isinstance_over_frozen_text_unions_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("option-isinstance-frozen-unions")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"const NAME: FrozenStr = "frozen"

def maybe(flag: bool) -> Option[str]:
    if flag:
        return Some("text")
    return None

def maybe_frozen(flag: bool) -> Option[FrozenStr]:
    if flag:
        return Some(NAME)
    return None

def frozen_or_int(flag: bool) -> FrozenStr | int:
    if flag:
        return NAME
    return 7

def maybe_union(flag: bool, text: bool) -> Option[FrozenStr | int]:
    if flag:
        return Some(frozen_or_int(text))
    return None

def number(value: int | FrozenStr | None) -> bool:
    return isinstance(value, int)

def main() -> None:
    println(number(5))
    println(number(NAME))
    println(number(None))
    println(isinstance(maybe(true), str))
    println(isinstance(maybe(false), str))
    println(isinstance(maybe_frozen(true), str))
    println(isinstance(maybe_frozen(false), str))
    println(isinstance(maybe_union(true, true), str))
    println(isinstance(maybe_union(true, false), str))
    println(isinstance(maybe_union(true, false), int))
    println(isinstance(maybe_union(false, true), str))
"#,
    )
}

/// A const read through a module binding is that module's value, read directly and passed on as an argument.
#[test]
fn module_member_constant_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    check_project_modules_case(
        "module_member_constants",
        "import labels\n\n\ndef shout(text: str) -> str:\n    return text.upper()\n\n\ndef main() -> None:\n    println(labels.DEFAULT_LABEL)\n    println(labels.LIMIT + 1)\n    println(labels.width())\n    println(shout(labels.DEFAULT_LABEL))\n",
        &[(
            "labels.incn",
            "pub const DEFAULT_LABEL: str = \"orders\"\npub const LIMIT: int = 3\n\n\npub def width() -> int:\n    return len(DEFAULT_LABEL)\n",
        )],
        b"orders\n4\n6\nORDERS\n",
    )
}

/// Compile one program natively and pin its output to the bytes legacy records for it, as a behavior fixture does.
fn check_native_case(name: &str, text: &str, expected_stdout: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let root = fixture.scratch(name)?;
    let source = root.join(format!("{name}.incn"));
    fs::write(&source, text)?;
    let native = root.join("native");
    let closure = corpus::runtime_closure(&fixture.formatting)?;
    success(
        &corpus::source_command(
            &fixture.driver_binary("debug"),
            &source,
            &native,
            &fixture.sysroot,
            &closure,
        )
        .output()?,
        "native compilation",
    );
    let actual = Command::new(native).output()?;
    success(&actual, "native execution");
    assert_eq!(actual.stdout, expected_stdout);
    Ok(())
}

/// A model adopting the std.serde.json protocol traits with `with`, by imported or module-qualified name, serializes
/// through serde as legacy's derived serde does and keeps its own static `from_json`.
#[test]
fn adopted_json_protocol_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    check_native_case(
        "adopted_json_protocol",
        r#"from std.serde import json
from std.serde.json import Serialize, Deserialize


model Payload with Serialize:
    value: int


model Tagged with json.Serialize:
    label: str
    count: int


model Restored with Serialize, Deserialize:
    value: int

    def from_json(json_str: str) -> Result[Restored, str]:
        return Ok(Restored(value=len(json_str)))


def main() -> None:
    println(Payload(value=1).to_json())
    println(Tagged(label="x", count=2).to_json())
    println(Restored(value=3).to_json())
    match Restored.from_json("{}"):
        case Ok(restored):
            println(restored.value)
        case Err(message):
            println(message)
"#,
        b"{\"value\":1}\n{\"label\":\"x\",\"count\":2}\n{\"value\":3}\n2\n",
    )
}

/// A union value, and a model holding one bare and inside an Option, Debug-format the held member as legacy's
/// hand-written union `Debug` does, never a variant name.
#[test]
fn union_member_debug_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("union-member-debug")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"model Holder:
    value: int | str
    maybe: Option[int | str]

def show(value: int | str) -> None:
    println(f"{value:?}")
    println(f"[{value:?}]")

def main() -> None:
    show(7)
    show("text")
    println(f"{Holder(value=7, maybe=Some("x")):?}")
    println(f"{Holder(value="text", maybe=None):?}")
    println(f"{Holder(value=-1, maybe=Some(2)):?}")
"#,
    )
}

/// `Type.default()` and `T.default()` under a `Default` bound build the model from its declared field defaults, and a
/// field declaring none takes its type's default, a nested derived default included, as legacy's derived Default does.
#[test]
fn derived_default_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("derived-default")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"@derive(Default)
model Settings:
    theme: str = "dark"
    font_size: int = 14
    retries: int
    ratio: float
    tags: list[str]
    counts: dict[str, int]
    note: Option[str]

@derive(Default)
model Inner:
    x: int = 5

@derive(Default)
model Outer:
    inner: Inner
    flag: bool

def make[T with Default]() -> T:
    return T.default()

def main() -> None:
    settings = Settings.default()
    println(settings.theme)
    println(settings.font_size)
    println(settings.retries)
    println(settings.ratio)
    println(len(settings.tags))
    println(len(settings.counts))
    println(settings.note is None)
    made: Settings = make()
    println(made.theme)
    println(Outer.default().inner.x)
    println(Outer.default().flag)
"#,
    )
}

/// A model deriving serde's Deserialize parses through `Type.from_json` with legacy's serde decoding, and
/// `json_stringify` writes a list through legacy's serde boundary, pinned to the output legacy records for them.
#[test]
fn derived_json_parse_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    check_native_case(
        "derived_json_parse",
        r#"from std.serde import json


@derive(json)
model Config:
    port: int
    name: str


def main() -> None:
    match Config.from_json("{\"port\":7,\"name\":\"api\"}"):
        case Ok(config):
            println(config.port)
            println(config.name)
            println(config.to_json())
        case Err(message):
            println(message)
    println(json_stringify([1, 2]))
    values = [3, 4, 5]
    println(json_stringify(values))
    println(len(values))
"#,
        b"7\napi\n{\"port\":7,\"name\":\"api\"}\n[1,2]\n[3,4,5]\n3\n",
    )
}

/// A plain newtype adopting the std.serde.json Serialize trait serializes as its wrapped value through serde's
/// newtype-struct form, pinned to the output legacy records for it.
#[test]
fn adopted_serialize_newtype_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    check_native_case(
        "adopted_serialize_newtype",
        r#"from std.serde.json import Serialize


type Wrapped = newtype str with Serialize
type Count = newtype int with Serialize


def main() -> None:
    println(Wrapped("wrapped").to_json())
    count = Count(3)
    println(count.to_json())
    println(count.0)
"#,
        b"\"wrapped\"\n3\n3\n",
    )
}

/// A model whose Eq and Hash come from `@rust.derive(...)` is a set element as in legacy: equal values collapse, and
/// membership and insertion hash the whole model.
#[test]
fn rust_derived_hashable_model_set_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("rust-derived-hashable-set")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"@rust.derive(PartialEq, Eq, Hash)
model Key:
    id: int
    label: str

def main() -> None:
    mut seen: set[Key] = {Key(id=1, label="a"), Key(id=1, label="a"), Key(id=2, label="a")}
    println(len(seen))
    println(Key(id=2, label="a") in seen)
    println(Key(id=2, label="b") in seen)
    seen.add(Key(id=3, label="c"))
    seen.add(Key(id=1, label="a"))
    println(len(seen))
"#,
    )
}

/// A model deriving Copy copies where legacy's does, and one deriving Eq and Hash keys a dictionary: equal keys share
/// an entry and membership hashes the whole model.
#[test]
fn copy_model_and_model_dictionary_key_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("copy-model-dictionary-key")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"@derive(Eq, Hash, Copy)
model Cell:
    row: int
    col: int

def total(cell: Cell) -> int:
    return cell.row + cell.col

def main() -> None:
    a = Cell(row=1, col=2)
    copied = a
    println(f"{copied.row} {a.col}")
    println(total(a) + total(a))
    mut seen: dict[Cell, int] = {}
    seen[a] = 1
    seen[Cell(row=1, col=2)] = 2
    seen[Cell(row=3, col=4)] = 3
    println(len(seen))
    println(Cell(row=3, col=4) in seen)
    println(Cell(row=4, col=3) in seen)
"#,
    )
}

/// An enum's own methods, the trait slot it fills and an associated constructor returning `Self` run as legacy's impl
/// blocks do, through a concrete receiver, a temporary and a list element.
#[test]
fn enum_methods_and_trait_adoption_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("enum-methods-and-traits")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"trait Labeled:
    def label(self) -> str

enum Signal with Labeled:
    Start
    Stop

    def label(self) -> str:
        match self:
            Signal.Start => return "start"
            Signal.Stop => return "stop"

    def rank(self) -> int:
        match self:
            Signal.Start => return 0
            Signal.Stop => return 1

    def default() -> Self:
        return Signal.Start

def main() -> None:
    signal = Signal.Stop
    println(signal.label())
    println(Signal.default().label())
    println(Signal.Start.rank() + signal.rank())
    signals = [Signal.Start, Signal.Stop]
    println(signals[1].rank())
"#,
    )
}

/// A JSON model writes and reads its aliased fields under their aliases, as legacy's `#[serde(rename)]` does, while
/// construction and field access accept either name; the canonical name is not a wire key. Pinned to the output legacy
/// records for it.
#[test]
fn serde_field_alias_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    check_native_case(
        "serde_field_alias",
        r#"from std.serde import json


@derive(json)
model Account:
    type_ [alias="type"]: str
    class_ as "class": str
    name: str

    def kind(self) -> str:
        return self.type


def main() -> None:
    account = Account(type="premium", class="gold", name="Ada")
    println(account.to_json())
    println(account.kind())
    match Account.from_json("{\"type\":\"basic\",\"class\":\"silver\",\"name\":\"Bo\"}"):
        Ok(decoded) => println(f"{decoded.type_} {decoded.class} {decoded.name}")
        Err(error) => println(error)
    match Account.from_json("{\"type_\":\"basic\",\"class\":\"silver\",\"name\":\"Bo\"}"):
        Ok(decoded) => println(decoded.type_)
        Err(error) => println(error)
"#,
        b"{\"type\":\"premium\",\"class\":\"gold\",\"name\":\"Ada\"}\npremium\nbasic silver Bo\nJSONDecodeError: missing field `type` at line 1 column 46\n",
    )
}

/// A keyed lookup in a dictionary with model keys renders a missing key through Display, which a model lacks, so the
/// direct route refuses it by name rather than handing rustc an unsatisfiable bound.
#[test]
fn model_dictionary_key_lookup_is_refused() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let root = fixture.scratch("model-dictionary-key-lookup")?;
    let source = root.join("lookup.incn");
    fs::write(
        &source,
        "@derive(Eq, Hash)\nmodel Cell:\n    row: int\n\ndef main() -> None:\n    mut seen: dict[Cell, int] = {}\n    seen[Cell(row=1)] = 1\n    println(seen[Cell(row=1)])\n",
    )?;
    let closure = corpus::runtime_closure(&fixture.formatting)?;
    let binary = root.join("native");
    let refused = corpus::source_command(
        &fixture.driver_binary("debug"),
        &source,
        &binary,
        &fixture.sysroot,
        &closure,
    )
    .output()?;
    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stderr)
            .contains("unsupported Body IR keyed lookup in a dictionary with model keys"),
        "{}",
        String::from_utf8_lossy(&refused.stderr)
    );
    assert!(!binary.exists());
    Ok(())
}

/// Static dictionaries and sets keep the entries an index write, `insert` and `add` commit across calls, and every
/// later read sees them.
#[test]
fn static_hashed_collection_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("static-hashed-collections")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"static counts: dict[str, int] = {}
static totals: dict[str, int] = {"start": 1}
static seen: set[int] = Set()

def record(name: str) -> None:
    counts[name] = counts.get(name, 0) + 1

def lookup(name: str) -> int:
    match counts.get(name):
        Some(value) => return value
        None => return -1

def mark(value: int) -> None:
    seen.add(value)

def main() -> None:
    record("a")
    record("a")
    record("b")
    println(counts["a"])
    println(counts.get("b", 0))
    println(counts.get("missing", 0))
    println(lookup("a"))
    println(lookup("z"))
    println(len(counts))
    totals.insert("next", 2)
    println(totals["start"] + totals["next"])
    mark(4)
    mark(4)
    mark(6)
    println(len(seen))
    println(seen.contains(4))
    println(seen.contains(5))
"#,
    )
}

/// Computed statics initialize as legacy's do (RFC 052): once each, in declaration order, on entry to the first
/// function of their module and so before `main`'s first statement, a later initializer reading an earlier static's
/// value. Reads then see the current value, whole or through a field, and a write or a collection method replaces it.
#[test]
fn computed_statics_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    check_project_case(
        "computed_statics",
        r#"model Config:
    name: str
    sizes: list[int]


def announce(label: str, value: int) -> int:
    println(f"init {label}")
    return value


static FIRST: int = announce("first", 1)
static CONFIG: Config = Config(name="base", sizes=[1, 2])
static NAME: Option[str] = Some("n")
static NAMES: list[str] = [str(FIRST)]
static SECOND: int = announce("second", FIRST + 1)


def describe() -> str:
    return f"{CONFIG.name} {len(CONFIG.sizes)}"


def remember(name: str) -> None:
    NAMES.append(name)


def main() -> None:
    println("main")
    println(FIRST + SECOND)
    println(describe())
    println(NAME.unwrap_or("x"))
    CONFIG = Config(name="next", sizes=[])
    println(describe())
    remember("a")
    remember("b")
    println(len(NAMES))
    println(NAMES[2])
"#,
        b"init first\ninit second\nmain\n3\nbase 2\nn\nnext 0\n3\nb\n",
    )
}

/// An imported module's computed static initializes when the importer first reads it, after the importer's earlier
/// statements, as legacy's read of an imported static first initializes its declaring module's statics.
#[test]
fn imported_computed_statics_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    check_project_modules_case(
        "imported_computed_statics",
        r#"from settings import LIMIT, limit_twice


def main() -> None:
    println("main")
    println(LIMIT)
    println(limit_twice())
"#,
        &[(
            "settings.incn",
            r#"def announce(value: int) -> int:
    println("init settings")
    return value


pub static LIMIT: int = announce(4)


pub def limit_twice() -> int:
    return LIMIT * 2
"#,
        )],
        b"main\ninit settings\n4\n8\n",
    )
}

/// A computed dictionary's indexed list retains its collection carrier for method calls and length reads; the key
/// is evaluated once, and later reads observe the dictionary's replacement value.
#[test]
fn computed_static_dictionary_values_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("computed-static-dictionary-values")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"static graph: dict[str, list[str]] = {}

def key(node: str) -> str:
    println(f"key {node}")
    return node

def linked(node: str) -> bool:
    return graph[key(node)].contains(node)

def main() -> None:
    graph.insert("a", ["a", "b"])
    println(linked("a"))
    graph.insert("a", ["b"])
    println(linked("a"))
    println(len(graph["a"]))
"#,
    )
}

/// Reading an imported literal static initializes every computed static in its declaring module first. The
/// initializer's observable output precedes that first read, just as it does in legacy.
#[test]
fn imported_literal_static_initializes_computed_peers_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>>
{
    check_project_modules_case(
        "imported_literal_static_initializes_computed_peers",
        r#"from settings import COUNTER

def main() -> None:
    println("main")
    println(COUNTER)
    println(COUNTER)
"#,
        &[(
            "settings.incn",
            r#"pub static COUNTER: int = 1

def setup() -> int:
    println("init settings")
    return 2

static STARTED: int = setup()
"#,
        )],
        b"main\ninit settings\n1\n1\n",
    )
}

/// Snapshot-based reads do not imply a write-back path through a dictionary element. An unproven mutation of that
/// projected static stays a named refusal, rather than reaching the adapter with a mismatched replacement carrier.
#[test]
fn computed_static_projection_mutation_is_refused() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let root = fixture.scratch("computed-static-projection-mutation")?;
    let source = root.join("mutation.incn");
    fs::write(
        &source,
        "static graph: dict[str, list[int]] = {}\n\ndef main() -> None:\n    graph[\"a\"] = [1]\n    graph[\"a\"].append(2)\n",
    )?;
    let closure = corpus::runtime_closure(&fixture.formatting)?;
    let binary = root.join("native");
    let output = corpus::source_command(
        &fixture.driver_binary("debug"),
        &source,
        &binary,
        &fixture.sysroot,
        &closure,
    )
    .output()?;
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("unsupported Body IR projected static collection method"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!binary.exists());
    Ok(())
}

/// A static model's frozen-list field reaches the collection carrier boundary after computed-static admission, so
/// the remaining refusal is pinned for the collection lane rather than being reported as an initializer refusal.
#[test]
fn computed_static_frozen_list_field_keeps_its_named_refusal() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let root = fixture.scratch("computed-static-frozen-list")?;
    let source = root.join("frozen.incn");
    fs::write(
        &source,
        "const TEXTS: FrozenList[str] = [\"alpha\", \"beta\"]\n\nmodel Metadata:\n    pub names: FrozenList[str]\n\nstatic METADATA: Metadata = Metadata(names=TEXTS)\n\ndef main() -> None:\n    println(METADATA.names.len())\n",
    )?;
    let closure = corpus::runtime_closure(&fixture.formatting)?;
    let binary = root.join("native");
    let output = corpus::source_command(
        &fixture.driver_binary("debug"),
        &source,
        &binary,
        &fixture.sysroot,
        &closure,
    )
    .output()?;
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("unsupported Body IR type FrozenList[str]"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!binary.exists());
    Ok(())
}

/// Prove hashed enum leaves byte-identical to legacy: a source enum deriving Eq and Hash is a set element and a
/// dictionary key, and `Option` and `Result` carriers over hashable payloads are set elements, read by membership,
/// length and a loop over a one-element set.
#[test]
fn hashed_enum_and_carrier_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("hashed-enum-and-carrier")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"@derive(Eq, Hash)
enum Tag:
    A
    B
    Named(str)

def count(tags: set[Tag]) -> int:
    return len(tags)

def main() -> None:
    tags = {Tag.A, Tag.B, Tag.A, Tag.Named("x"), Tag.Named("x")}
    println(count(tags))
    println(Tag.A in tags)
    println(Tag.Named("y") in tags)
    for tag in {Tag.B}:
        match tag:
            Tag.A => println("a")
            Tag.B => println("b")
            Tag.Named(name) => println(name)
    mut weights: dict[Tag, int] = {Tag.A: 1}
    weights[Tag.Named("z")] = 2
    println(Tag.Named("z") in weights)
    println(len(weights))
    for result in {Ok(6)}:
        match result:
            Ok(value) => println(value)
            Err(_) => println("error")
    names = {Some("a"), Some("a"), None}
    println(len(names))
    println(Some("a") in names)
    pairs = {Some((1, "a")), Some((1, "a")), Some((2, "b"))}
    println(len(pairs))
    println(Some((2, "b")) in pairs)
"#,
    )
}

/// A keyed lookup in a dictionary with enum keys renders a missing key through Display, which an enum lacks, so the
/// direct route refuses it by name, as it does for model keys.
#[test]
fn enum_dictionary_key_lookup_is_refused() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let root = fixture.scratch("enum-dictionary-key-lookup")?;
    let source = root.join("lookup.incn");
    fs::write(
        &source,
        "@derive(Eq, Hash)\nenum Tag:\n    A\n    B\n\ndef main() -> None:\n    seen: dict[Tag, int] = {Tag.A: 1}\n    println(seen[Tag.A])\n",
    )?;
    let closure = corpus::runtime_closure(&fixture.formatting)?;
    let binary = root.join("native");
    let refused = corpus::source_command(
        &fixture.driver_binary("debug"),
        &source,
        &binary,
        &fixture.sysroot,
        &closure,
    )
    .output()?;
    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stderr)
            .contains("unsupported Body IR keyed lookup in a dictionary with enum keys"),
        "{}",
        String::from_utf8_lossy(&refused.stderr)
    );
    assert!(!binary.exists());
    Ok(())
}

/// A model's owner arguments, adopted trait arguments and method alias keep the same checked target together as each
/// does alone; two concrete instantiations must preserve their distinct field and result types.
#[test]
fn model_shape_combinations_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("model-shape-combinations")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"trait Read[T]:
    def read(self) -> T: ...

model Cell[T with Clone] with Read[T]:
    value: T
    get = read

    def read(self) -> T:
        return self.value

def main() -> None:
    println(Cell(value=7).get())
    println(Cell(value="text").get())
"#,
    )
}

/// Self-typed arguments read the owner's declared storage slots, including the wrapped value inside newtype methods.
#[test]
fn self_field_identity_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("self-field-identity")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"type Cell = newtype int:

    def total(self, other: Self) -> int:
        return self.0 + other.0

    def __eq__(self, other: Self) -> bool:
        return self.0 == other.0

def main() -> None:
    left = Cell(2)
    right = Cell(3)
    println(left.total(right))
    println(left.__eq__(right))
"#,
    )
}

/// A method partial closes its owner independently at each checked instantiation and honors an explicit preset
/// override.
#[test]
fn generic_model_method_partial_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("generic-model-method-partial")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"model Wrapper[T]:
    value: T

    def tag(self, prefix: str) -> str:
        return prefix

    alias = tag
    tagged = partial alias(prefix="wrapped")

def main() -> None:
    println(Wrapper(value=1).tagged())
    println(Wrapper(value="text").tagged())
    println(Wrapper(value=2).tagged(prefix="other"))
"#,
    )
}

/// TOML-derived models retain native layout and serde derives; serialization still refuses until the native marker
/// adoption is represented, pinning that next-stage boundary instead of silently omitting it. The layout output is
/// pinned to the existing TOML fixture's recorded legacy first line; this runner cannot link legacy's serde derives.
#[test]
fn model_toml_derive_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let root = fixture.scratch("model-toml-derive")?;
    check_native_case(
        "model-toml-derived-layout",
        r#"from std import toml

@derive(toml)
model Project:
    name: str

def main() -> None:
    println(Project(name="demo").name)
"#,
        b"demo\n",
    )?;
    let source = root.join("serialize.incn");
    fs::write(
        &source,
        r#"from std import toml

@derive(toml)
model Project:
    name: str

def main() -> None:
    match toml.serialize(Project(name="demo")):
        Ok(text) => println(text.strip())
        Err(error) => println(error.message())
"#,
    )?;
    let closure = corpus::runtime_closure(&fixture.formatting)?;
    let binary = root.join("serialize-native");
    let refused = corpus::source_command(
        &fixture.driver_binary("debug"),
        &source,
        &binary,
        &fixture.sysroot,
        &closure,
    )
    .output()?;
    let diagnostic = String::from_utf8(refused.stderr)?;
    assert!(!refused.status.success());
    assert!(
        diagnostic.contains("unsupported native instantiation")
            && diagnostic.contains("a type argument has no native impl for the callee's bounds"),
        "{diagnostic}"
    );
    assert!(!binary.exists());
    Ok(())
}

/// A descriptor model's fields and comparison preserve their values. A constant projection pins the next static
/// storage boundary rather than claiming model constant materialization is implemented by derive admission.
#[test]
fn model_descriptor_derive_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let root = fixture.scratch("model-descriptor-derive")?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &root,
        &fixture.sysroot,
        &fixture.formatting,
        r#"@derive(Clone, Eq, Descriptor)
model Version:
    major: int
    minor: int

def main() -> None:
    initial = Version(major=0, minor=1)
    println(f"{initial.major}.{initial.minor}")
    println(initial == Version(major=0, minor=1))
"#,
    )?;
    let source = root.join("constant.incn");
    fs::write(
        &source,
        "@derive(Clone, Eq, Descriptor)\nmodel Version:\n    major: int\n    minor: int\n\nconst INITIAL: Version = Version(major=0, minor=1)\n\ndef main() -> None:\n    println(INITIAL.major)\n",
    )?;
    let closure = corpus::runtime_closure(&fixture.formatting)?;
    let binary = root.join("constant-native");
    let refused = corpus::source_command(
        &fixture.driver_binary("debug"),
        &source,
        &binary,
        &fixture.sysroot,
        &closure,
    )
    .output()?;
    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("unsupported Body IR static projection"),
        "{}",
        String::from_utf8_lossy(&refused.stderr)
    );
    assert!(!binary.exists());
    Ok(())
}

/// A value enum reached only through another declaration's serde derives stays refused by name until that propagation
/// has raw-value parity evidence; ordinary enum serde derives must never silently serialize its variant names.
#[test]
fn propagated_value_enum_serde_is_refused() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let root = fixture.scratch("propagated-value-enum-serde")?;
    let source = root.join("payload.incn");
    fs::write(
        &source,
        "from std.serde import json\n\n@derive(json)\nmodel Payload:\n    tag: Tag\n\nenum Tag(str):\n    A = \"alpha\"\n    B = \"beta\"\n\ndef main() -> None:\n    println(Payload(tag=Tag.A).to_json())\n",
    )?;
    let closure = corpus::runtime_closure(&fixture.formatting)?;
    let binary = root.join("native");
    let refused = corpus::source_command(
        &fixture.driver_binary("debug"),
        &source,
        &binary,
        &fixture.sysroot,
        &closure,
    )
    .output()?;
    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stderr)
            .contains("unsupported Body IR serde derive propagation to value enum Tag"),
        "{}",
        String::from_utf8_lossy(&refused.stderr)
    );
    assert!(!binary.exists());
    Ok(())
}

/// A serde model's derives reach the enums and newtypes its fields name, directly and through a list, an `Option` and
/// a dictionary, as legacy propagates them, so the whole payload writes and reads. Pinned to the output legacy records.
#[test]
fn serde_derive_propagation_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    check_native_case(
        "serde_derive_propagation",
        r#"from std.serde import json


@derive(json)
model Payload:
    tag: Tag
    tags: list[Tag]
    shape: Option[Shape]
    kinds: dict[str, Shape]
    id: UserId


enum Tag:
    A
    B


enum Shape:
    Circle(int)
    Square(int, int)


type UserId = newtype int


def main() -> None:
    payload = Payload(tag=Tag.B, tags=[Tag.A, Tag.B], shape=Some(Shape.Square(2, 3)), kinds={"k": Shape.Circle(1)}, id=UserId(7))
    encoded = payload.to_json()
    println(encoded)
    match Payload.from_json(encoded):
        Ok(decoded) => println(decoded.to_json())
        Err(error) => println(error)
    match Payload.from_json("{\"tag\":\"C\",\"tags\":[],\"shape\":null,\"kinds\":{},\"id\":1}"):
        Ok(_) => println("decoded")
        Err(error) => println(error)
"#,
        b"{\"tag\":\"B\",\"tags\":[\"A\",\"B\"],\"shape\":{\"Square\":[2,3]},\"kinds\":{\"k\":{\"Circle\":1}},\"id\":7}\n{\"tag\":\"B\",\"tags\":[\"A\",\"B\"],\"shape\":{\"Square\":[2,3]},\"kinds\":{\"k\":{\"Circle\":1}},\"id\":7}\nJSONDecodeError: unknown variant `C`, expected `A` or `B` at line 1 column 10\n",
    )
}

/// A JSON value enum writes and reads its raw value, never its variant name, as legacy's hand-written serde impls do:
/// a text and an integer backing inside a JSON model, accepted and refused. Pinned to the output legacy records.
#[test]
fn value_enum_serde_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    check_native_case(
        "value_enum_serde",
        r#"from std.serde import json


@derive(json)
enum Env(str):
    Dev = "development"
    Prod = "production"


@derive(json)
enum Status(int):
    Ok = 200
    NotFound = 404


@derive(json)
model Deployment:
    env: Env
    status: Status


def main() -> None:
    println(Deployment(env=Env.Prod, status=Status.NotFound).to_json())
    match Deployment.from_json("{\"env\":\"development\",\"status\":200}"):
        Ok(parsed) => println(f"{parsed.env.value()} {parsed.status.value()} {parsed.env.message()}")
        Err(error) => println(error)
    match Deployment.from_json("{\"env\":\"Dev\",\"status\":200}"):
        Ok(_) => println("accepted")
        Err(error) => println(error)
    match Deployment.from_json("{\"env\":\"production\",\"status\":7}"):
        Ok(_) => println("accepted")
        Err(error) => println(error)
"#,
        b"{\"env\":\"production\",\"status\":404}\ndevelopment 200 Dev\nJSONDecodeError: invalid value for Env: Dev at line 1 column 12\nJSONDecodeError: invalid value for Status: 7 at line 1 column 31\n",
    )
}

/// Mutable list arguments and loops through container elements change the retained storage, with output identical
/// to legacy for nested dictionaries, list elements, and nominal fields.
#[test]
fn projected_list_mutation_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("projected-list-mutation")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"class Holder:
    pub rows: list[list[int]]

class Registry:
    pub groups: dict[str, list[int]]

    def add(mut self, value: int) -> None:
        self.groups["a"].append(value)

def main() -> None:
    mut holders = [Holder(rows=[[1], [2]])]
    for row in holders[0].rows:
        row.append(3)
    mut by_name: dict[str, Holder] = {"a": Holder(rows=[[4]])}
    for row in by_name["a"].rows:
        row.append(5)
    mut nested: dict[str, dict[str, list[int]]] = {"a": {"b": []}}
    nested["a"]["b"].append(7)
    mut rows: list[dict[str, list[int]]] = [{"a": []}]
    rows[0]["a"].append(8)
    mut registry = Registry(groups={"a": []})
    registry.add(9)
    registry.groups["a"].append(10)
    mut grid: dict[str, list[list[int]]] = {"a": [[], []]}
    for row in grid["a"]:
        row.append(11)
    mut lists: list[list[int]] = [[1], [2]]
    lists[-1].append(3)
    println(holders[0].rows)
    println(by_name["a"].rows)
    println(nested["a"]["b"])
    println(rows[0]["a"])
    println(registry.groups["a"])
    println(grid["a"])
    println(lists)
"#,
    )
}

/// Model snapshots persist across repeated closure calls and source reassignment, including a generic model and a
/// shared `self` receiver. The output stays byte-identical to legacy.
#[test]
fn model_capture_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("model-capture")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"model User:
    name: str

    def label(self, prefix: str, suffix: str) -> str:
        return prefix + self.name + suffix

model Box[T]:
    value: T

    def pair(self, other: T, sep: str) -> list[T]:
        return [self.value, other]

class Greeter:
    pub greeting: str

    def greet(self, name: str, punct: str) -> str:
        return self.greeting + " " + name + punct

    def make(self) -> str:
        hello: (str) -> str = (name) => self.greet(name, "!")
        return hello("you") + " " + hello("me")

def main() -> None:
    mut user = User(name="ann")
    label: (str, str) -> str = (prefix, suffix) => user.label(prefix, suffix)
    show = () => user.name
    user = User(name="bob")
    println(label("x", "!"))
    println(label("y", "?"))
    println(show())
    println(show())
    println(user.name)
    b = Box(value=1)
    joined: (int) -> list[int] = (other) => b.pair(other, "-")
    println(joined(2))
    println(joined(3))
    g = Greeter(greeting="hi")
    println(g.make())
"#,
    )
}

/// A list's contains method uses legacy membership comparisons through static defaults and retained local lists.
#[test]
fn list_contains_method_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("list-contains-method")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"static ITEMS: list[str] = ["a"]

def has(value: str = "a") -> bool:
    return ITEMS.contains(value)

def main() -> None:
    println(has())
    println(has("b"))
    values = [1, 2, 3]
    println(values.contains(2))
    println(values.contains(4))
    println(len(values))
    floats = [1.5, 2.5]
    println(floats.contains(1.5))
    println(floats.contains(0.0))
"#,
    )
}

/// Defaults that read module-owned statics retain their declaring module across calls from a different module.
#[test]
fn static_collection_defaults_across_modules_match_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let root = fixture.scratch("static-collection-defaults")?;
    let project = root.join("builtin-parity");
    fs::create_dir_all(&project)?;
    for (name, source) in [
        (
            "helpers",
            include_str!(
                "../../incan_test_support/fixtures/behavior/cli_modules_and_declarations/default_static_reads_across_modules/helpers.incn"
            ),
        ),
        (
            "caller",
            include_str!(
                "../../incan_test_support/fixtures/behavior/cli_modules_and_declarations/default_static_reads_across_modules/caller.incn"
            ),
        ),
        (
            "config",
            include_str!(
                "../../incan_test_support/fixtures/behavior/cli_modules_and_declarations/default_static_reads_across_modules/config.incn"
            ),
        ),
    ] {
        fs::write(project.join(format!("{name}.incn")), source)?;
    }
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &root,
        &fixture.sysroot,
        &fixture.formatting,
        include_str!(
            "../../incan_test_support/fixtures/behavior/cli_modules_and_declarations/default_static_reads_across_modules/main.incn"
        ),
    )
}

/// Byte literals, returned buffers and parameters compare through shared storage without consuming later reads.
#[test]
fn bytes_comparison_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("bytes-io-comparison")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"def same(left: bytes, right: bytes) -> bool:
    return left == right

def buffer() -> bytes:
    return b"ab"

def retained() -> bytes:
    values = b""
    equal = values == b""
    println(equal)
    return values

def main() -> None:
    data = b"ab"
    println(data == b"ab")
    println(data != b"ac")
    println(same(data, buffer()))
    println(len(data))
    println(len(retained()))
"#,
    )
}

/// Once bytes equality retains its shared reads, the I/O fixture reaches its traits-owned imported method refusal.
#[test]
fn bytes_io_trait_method_refusal_is_retained() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let root = fixture.scratch("bytes-io-trait-refusal")?;
    let source = root.join("io.incn");
    fs::write(
        &source,
        include_str!(
            "../../incan_test_support/fixtures/behavior/ownership_parameters_and_receivers/bytes_io_read_through_a_guard.incn"
        ),
    )?;
    let closure = corpus::runtime_closure(&fixture.formatting)?;
    let binary = root.join("native");
    let refused = corpus::source_command(
        &fixture.driver_binary("debug"),
        &source,
        &binary,
        &fixture.sysroot,
        &closure,
    )
    .output()?;
    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("unsupported Body IR nonlocal or trait-object Method"),
        "{}",
        String::from_utf8_lossy(&refused.stderr)
    );
    assert!(!binary.exists());
    Ok(())
}

/// Indexing a tuple copied from a collection and iterating a list slice retain legacy output and source storage.
#[test]
fn collection_projection_reads_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("collection-projection-reads")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"def main() -> None:
    pairs = [(1, "a"), (2, "b")]
    println(pairs[0][0])
    println(pairs[-1][-1])
    rows = [1, 2, 3, 4]
    for value in rows[1:3]:
        println(value)
    for value in rows[::-1]:
        println(value)
    words = ["a", "b", "c"]
    for word in words[1:]:
        println(word)
    println(len(rows))
    println(pairs[0][1])
"#,
    )
}

/// Settled match payloads remain initialized when a list or tuple projection exposes their reads.
#[test]
fn settled_match_projection_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("settled-match-projection")?,
        &fixture.sysroot,
        &fixture.formatting,
        include_str!(
            "../../incan_test_support/fixtures/behavior/emit_calls_and_builtins/match_arms_and_loop_values_share_one_type.incn"
        ),
    )
}

/// Dictionary entry writes through a generic model's field change that retained field rather than a copied map.
#[test]
fn generic_dictionary_field_mutation_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("generic-dictionary-field-mutation")?,
        &fixture.sysroot,
        &fixture.formatting,
        r#"class Index[K, V]:
    entries: dict[K, V]

    def put(mut self, key: K, value: V) -> None:
        self.entries[key] = value

    def size(self) -> int:
        return len(self.entries)

def fill[K, V](mut index: Index[K, V], key: K, value: V) -> int:
    index.put(key, value)
    return index.size()

def main() -> None:
    mut index: Index[str, int] = Index(entries={})
    println(fill(index, "a", 1))
    println(fill(index, "b", 2))
"#,
    )
}

/// Keep the calls-owned contextual constructor refusal explicit after generic dictionary fields become writable.
#[test]
fn contextual_generic_collection_refusal_is_retained() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let root = fixture.scratch("contextual-generic-collection-refusal")?;
    let source = root.join("generic.incn");
    fs::write(
        &source,
        include_str!(
            "../../incan_test_support/fixtures/behavior/ownership_numerics_and_bounds/generic_functions_hash_through_dict_and_set_types.incn"
        ),
    )?;
    let closure = corpus::runtime_closure(&fixture.formatting)?;
    let binary = root.join("native");
    let refused = corpus::source_command(
        &fixture.driver_binary("debug"),
        &source,
        &binary,
        &fixture.sysroot,
        &closure,
    )
    .output()?;
    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("unsupported Body IR collection leaf ? in distinct"),
        "{}",
        String::from_utf8_lossy(&refused.stderr)
    );
    assert!(!binary.exists());
    Ok(())
}

/// An environment argument list can be sliced for iteration while its original storage remains readable.
#[test]
fn environment_argument_slice_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("environment-argument-slice")?,
        &fixture.sysroot,
        &fixture.formatting,
        include_str!("../../incan_test_support/fixtures/behavior/snapshots_stdlib/std_environ_args.incn"),
    )
}

/// A stored hasher's returned bytes compare with one-shot digest bytes without moving comparison operands.
#[test]
fn stored_hasher_comparison_output_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    corpus::check_builtin_source(
        &fixture.driver_binary("debug"),
        &fixture.scratch("stored-hasher-comparison")?,
        &fixture.sysroot,
        &fixture.formatting,
        include_str!(
            "../../incan_test_support/fixtures/behavior/snapshots_collections_and_strings/storable_sha256_hasher.incn"
        ),
    )
}

/// Keep the classes-owned method-on-value partial refusal explicit while ordinary model captures are admitted.
#[test]
fn model_method_partial_body_ir_refusal_is_retained() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = driver_fixture()?;
    let root = fixture.scratch("model-method-partial-refusal")?;
    let source = root.join("partial.incn");
    fs::write(
        &source,
        include_str!(
            "../../incan_test_support/fixtures/behavior/codegen_functions_and_aliases/local_partials_of_methods_of_values.incn"
        ),
    )?;
    let closure = corpus::runtime_closure(&fixture.formatting)?;
    let binary = root.join("native");
    let refused = corpus::source_command(
        &fixture.driver_binary("debug"),
        &source,
        &binary,
        &fixture.sysroot,
        &closure,
    )
    .output()?;
    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("unsupported Body IR type Unknown in make local 1"),
        "{}",
        String::from_utf8_lossy(&refused.stderr)
    );
    assert!(!binary.exists());
    Ok(())
}
