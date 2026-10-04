use std::fs;
use std::path::Path;
use std::process::Command;

use incan_test_support as support;
use oven_store::OvenReceipt;

fn copy_tree(source: &Path, destination: &Path) -> Result<(), Box<dyn std::error::Error>> {
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

fn bake(project: &Path, home: &Path) -> Result<std::process::Output, Box<dyn std::error::Error>> {
    Ok(support::repo_command()
        .args(["oven", "bake", "--project"])
        .arg(project)
        .env("INCAN_HOME", home)
        .env("INCAN_NO_BANNER", "1")
        .output()?)
}

fn assert_success(output: &std::process::Output, operation: &str) {
    assert!(
        output.status.success(),
        "{operation} failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn read_receipt(path: &Path) -> Result<OvenReceipt, Box<dyn std::error::Error>> {
    Ok(serde_json::from_slice(&fs::read(path)?)?)
}

#[test]
fn one_bake_builds_and_receipts_a_rust_caller_of_an_incan_library() -> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let fixture = support::repo_root().join("loaves/compiler/incan_driver/tests/fixtures/rust_caller_project");
    let project = temporary.path().join("project");
    copy_tree(&fixture, &project)?;
    let consumer = project.join("consumer");
    let home = temporary.path().join("home");

    let first = bake(&consumer, &home)?;
    assert_success(&first, "project Rust-unit bake");
    let binary = consumer.join("target/rust/debug/typed-boundary-consumer");
    let run = Command::new(&binary).output()?;
    assert_success(&run, "project Rust binary");
    assert_eq!(
        String::from_utf8(run.stdout)?,
        "returned: label=bootstrap version=3\nreturned: assign dst=7 value=42\nreturned: return\nconstructed: label=rust-built version=4\ndrops completed\n"
    );

    let receipt_path = consumer.join("target/rust/receipts/typed-boundary-consumer-debug.json");
    let first_receipt = read_receipt(&receipt_path)?;
    let library_source = project.join("library/src/lib.incn");
    let mut changed = fs::read_to_string(&library_source)?;
    changed.push_str("\n\npub def added() -> int:\n    return 9\n");
    fs::write(&library_source, changed)?;
    let consumer_source = consumer.join("src/main.rs");
    let mut changed_consumer = fs::read_to_string(&consumer_source)?;
    changed_consumer.push_str("\nconst _: fn() -> i64 = typed_boundary::caller::incan::added;\n");
    fs::write(consumer_source, changed_consumer)?;
    let second = bake(&consumer, &home)?;
    assert_success(&second, "caller-identity mutation bake");
    let second_receipt = read_receipt(&receipt_path)?;
    assert_ne!(first_receipt.build_unit_identity, second_receipt.build_unit_identity);
    Ok(())
}

#[test]
fn bake_refuses_missing_and_nonrepresentable_caller_exports_by_name() -> Result<(), Box<dyn std::error::Error>> {
    for (name, replacement, expected) in [
        ("missing", "missing", "missing or is not public"),
        ("async", "later", "async functions"),
    ] {
        let temporary = tempfile::tempdir()?;
        let fixture = support::repo_root().join("loaves/compiler/incan_driver/tests/fixtures/rust_caller_project");
        let project = temporary.path().join(name);
        copy_tree(&fixture, &project)?;
        let consumer_source = project.join("consumer/src/main.rs");
        let source = fs::read_to_string(&consumer_source)?.replace("make_plan", replacement);
        fs::write(&consumer_source, source)?;
        if name == "async" {
            let library_source = project.join("library/src/lib.incn");
            let mut source = fs::read_to_string(&library_source)?;
            source.insert_str(0, "import std.async\n\n");
            source.push_str("\n\npub async def later() -> int:\n    return 1\n");
            fs::write(library_source, source)?;
        }
        let output = bake(&project.join("consumer"), &temporary.path().join("home"))?;
        assert!(!output.status.success(), "{name} caller export unexpectedly baked");
        let diagnostic = String::from_utf8_lossy(&output.stderr);
        assert!(
            diagnostic.contains(replacement) && diagnostic.contains(expected),
            "{diagnostic}"
        );
    }
    Ok(())
}

/// A caller import and a source change inside a module both participate in selection and source receipt identity.
#[test]
fn caller_modules_are_selected_and_bound_into_the_source_receipt() -> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let fixture = support::repo_root().join("loaves/compiler/incan_driver/tests/fixtures/rust_caller_project");
    let project = temporary.path().join("project");
    copy_tree(&fixture, &project)?;
    let consumer = project.join("consumer");
    fs::write(
        consumer.join("src/main.rs"),
        "mod boundary;\nfn main() { println!(\"{}\", boundary::label()); }\n",
    )?;
    let module = consumer.join("src/boundary.rs");
    fs::write(
        &module,
        "use typed_boundary::caller::incan::make_plan;\npub fn label() -> String { make_plan().label }\n",
    )?;
    let home = temporary.path().join("home");
    assert_success(&bake(&consumer, &home)?, "split caller bake");
    let receipt_path = consumer.join("target/rust/receipts/typed-boundary-consumer-debug.json");
    let first = read_receipt(&receipt_path)?;
    let binary = consumer.join("target/rust/debug/typed-boundary-consumer");
    assert_eq!(
        String::from_utf8(Command::new(&binary).output()?.stdout)?,
        "bootstrap\n"
    );
    fs::write(
        &module,
        "use typed_boundary::caller::incan::make_plan;\npub fn label() -> String { format!(\"module: {}\", make_plan().label) }\n",
    )?;
    assert_success(&bake(&consumer, &home)?, "changed caller module bake");
    let second = read_receipt(&receipt_path)?;
    assert_eq!(first.build_unit_identity, second.build_unit_identity);
    assert_ne!(first.identity, second.identity);
    assert_eq!(
        String::from_utf8(Command::new(&binary).output()?.stdout)?,
        "module: bootstrap\n"
    );
    Ok(())
}
