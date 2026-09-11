//! The property #1495 turns on: a digest of what the compiler produces, not of what the compiler is made of.
//!
//! The SDK provider store is keyed today by a hash of the whole compiler source tree, so any edit under `src/` or
//! `crates/` rebuilds all ten components at a cost of roughly seventeen minutes. These tests pin the two things a
//! replacement has to get right — it must move for a change the compiler would emit differently, and hold still
//! for one it would not — and record what it costs against the real standard library.

use incan::inspect::effect_digest::{module_effect_digest, stdlib_effect_digest};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

/// Write one throwaway `.incn` module and digest it.
fn digest_module(directory: &Path, name: &str, source: &str) -> Result<String, Box<dyn std::error::Error>> {
    let path = directory.join(format!("{name}.incn"));
    fs::write(&path, source)?;
    Ok(module_effect_digest(&path)?)
}

#[test]
fn a_comment_does_not_move_a_module_effect_digest() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let plain = digest_module(
        tmp.path(),
        "probe",
        "def area(w: int, h: int) -> int:\n    return w * h\n",
    )?;
    let commented = digest_module(
        tmp.path(),
        "probe",
        "# a comment the compiler cannot emit\ndef area(w: int, h: int) -> int:\n    # nor this one\n    return w * h\n",
    )?;
    assert_eq!(
        plain, commented,
        "a comment cannot change what the compiler emits, so it must not cost a rebuild"
    );
    Ok(())
}

#[test]
fn a_docstring_does_not_move_a_module_effect_digest() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let plain = digest_module(
        tmp.path(),
        "probe",
        "def area(w: int, h: int) -> int:\n    return w * h\n",
    )?;
    let documented = digest_module(
        tmp.path(),
        "probe",
        "def area(w: int, h: int) -> int:\n    \"\"\"Area of a rectangle.\"\"\"\n    return w * h\n",
    )?;
    assert_eq!(plain, documented, "documentation is not compiled output");
    Ok(())
}

#[test]
fn a_changed_body_moves_a_module_effect_digest() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let product = digest_module(
        tmp.path(),
        "probe",
        "def area(w: int, h: int) -> int:\n    return w * h\n",
    )?;
    let sum = digest_module(
        tmp.path(),
        "probe",
        "def area(w: int, h: int) -> int:\n    return w + h\n",
    )?;
    assert_ne!(product, sum, "a changed operator is a changed artifact");
    Ok(())
}

#[test]
fn the_digest_covers_the_rust_runtime_every_component_links_against() -> TestResult {
    // An Incan-only digest would report a hit for an edit to the Rust runtime, which is a false reuse of a
    // component whose behaviour changed. This is the guard against that being quietly dropped.
    let tmp = tempfile::tempdir()?;
    let stdlib = tmp.path().join("stdlib");
    let runtime = tmp.path().join("runtime");
    let emitter = tmp.path().join("emitter");
    for directory in [&stdlib, &runtime, &emitter] {
        fs::create_dir_all(directory)?;
    }
    fs::write(
        stdlib.join("probe.incn"),
        "def area(w: int, h: int) -> int:\n    return w * h\n",
    )?;
    fs::write(emitter.join("emit.rs"), "pub fn emit() -> u8 { 1 }\n")?;

    fs::write(runtime.join("frozen.rs"), "pub fn limit() -> u8 { 1 }\n")?;
    let before = stdlib_effect_digest(&stdlib, &runtime, &emitter)?;
    fs::write(runtime.join("frozen.rs"), "pub fn limit() -> u8 { 2 }\n")?;
    let after = stdlib_effect_digest(&stdlib, &runtime, &emitter)?;
    assert_ne!(before, after, "an edit to the linked Rust runtime must invalidate");
    Ok(())
}

#[test]
fn the_digest_covers_the_emitter_while_the_emitter_still_exists() -> TestResult {
    // Transitional, and deliberately so: a change confined to emission alters generated Rust without moving any
    // HIR. When emission is gone this test goes with it.
    let tmp = tempfile::tempdir()?;
    let stdlib = tmp.path().join("stdlib");
    let runtime = tmp.path().join("runtime");
    let emitter = tmp.path().join("emitter");
    for directory in [&stdlib, &runtime, &emitter] {
        fs::create_dir_all(directory)?;
    }
    fs::write(
        stdlib.join("probe.incn"),
        "def area(w: int, h: int) -> int:\n    return w * h\n",
    )?;
    fs::write(runtime.join("frozen.rs"), "pub fn limit() -> u8 { 1 }\n")?;

    fs::write(emitter.join("emit.rs"), "pub fn emit() -> u8 { 1 }\n")?;
    let before = stdlib_effect_digest(&stdlib, &runtime, &emitter)?;
    fs::write(emitter.join("emit.rs"), "pub fn emit() -> u8 { 2 }\n")?;
    let after = stdlib_effect_digest(&stdlib, &runtime, &emitter)?;
    assert_ne!(
        before, after,
        "an emitter change alters generated Rust without moving HIR"
    );
    Ok(())
}

#[test]
fn the_real_standard_library_digests_deterministically_and_cheaply() -> TestResult {
    let root = repo_root();
    let stdlib = root.join("crates/incan_stdlib/stdlib");
    let runtime = root.join("crates/incan_stdlib/src");
    let emitter = root.join("src/backend/ir/emit");

    let started = Instant::now();
    let first = incan::compiler_stack::run_on_compiler_stack({
        let (stdlib, runtime, emitter) = (stdlib.clone(), runtime.clone(), emitter.clone());
        move || stdlib_effect_digest(&stdlib, &runtime, &emitter).map_err(|error| error.to_string())
    })?;
    let elapsed = started.elapsed();

    let second = incan::compiler_stack::run_on_compiler_stack({
        let (stdlib, runtime, emitter) = (stdlib.clone(), runtime.clone(), emitter.clone());
        move || stdlib_effect_digest(&stdlib, &runtime, &emitter).map_err(|error| error.to_string())
    })?;

    println!("EFFECT-DIGEST {first} in {} ms", elapsed.as_millis());
    assert_eq!(first, second, "the digest must not depend on iteration order or run");
    assert!(
        first.starts_with("sha256:"),
        "the digest must be a labelled sha256, got {first}"
    );
    // The rebuild this replaces costs roughly seventeen minutes. A bound two orders of magnitude below that is
    // loose enough to survive a loaded machine and tight enough to fail if the cheap path is ever lost.
    assert!(
        elapsed.as_secs() < 60,
        "digesting the standard library took {elapsed:?}, which is no longer cheap enough to run per command"
    );
    Ok(())
}
