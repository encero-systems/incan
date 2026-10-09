//! Actual native input callers retain byte identities while reusing only replacement-sensitive observations.

use super::{Error, tests::unit, unit_plan};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

/// Exercise each real caller in a child with its own persistent cache, without mutating process-global variables.
pub(crate) fn assert_file_observation(
    exact: &str,
    names: [&str; 2],
    observe: impl Fn(&Path, &Path) -> Result<BTreeMap<String, String>, Error>,
) -> Result<(), Error> {
    const CONTROL_ROOT: &str = "INCAN_DEV7_NATIVE_DIGEST_CONTROL_ROOT";
    if let Some(root) = std::env::var_os(CONTROL_ROOT) {
        let root = Path::new(&root);
        let linker = root.join("rust-lld");
        let runtime = root.join("libLLVM.dylib");
        let first_runtime = root.join("runtime-original");
        let second_runtime = root.join("runtime-replacement");
        fs::write(&linker, b"native one")?;
        fs::write(&first_runtime, b"runtime one")?;
        fs::write(&second_runtime, b"runtime two")?;
        std::os::unix::fs::symlink(&first_runtime, &runtime)?;
        let expected = |native: &[u8], llvm: &[u8]| {
            BTreeMap::from([
                (names[0].to_string(), oven_store::digest_bytes(native)),
                (names[1].to_string(), oven_store::digest_bytes(llvm)),
            ])
        };
        let initial = observe(&linker, &runtime)?;
        assert_eq!(initial, expected(b"native one", b"runtime one"));
        assert_eq!(observe(&linker, &runtime)?, initial);
        let modified = fs::metadata(&linker)?.modified()?;
        fs::write(&linker, b"native two")?;
        fs::OpenOptions::new()
            .write(true)
            .open(&linker)?
            .set_times(fs::FileTimes::new().set_modified(modified))?;
        assert_eq!(fs::metadata(&linker)?.modified()?, modified);
        assert_eq!(observe(&linker, &runtime)?, expected(b"native two", b"runtime one"));
        fs::remove_file(&runtime)?;
        std::os::unix::fs::symlink(&second_runtime, &runtime)?;
        let retargeted = observe(&linker, &runtime)?;
        assert_eq!(retargeted, expected(b"native two", b"runtime two"));
        assert_eq!(observe(&linker, &runtime)?, retargeted);
        fs::remove_file(&runtime)?;
        std::os::unix::fs::symlink(root.join("missing-runtime"), &runtime)?;
        let error = observe(&linker, &runtime)
            .err()
            .ok_or("dangling native runtime was accepted")?;
        assert!(error.to_string().contains(&runtime.display().to_string()), "{error}");
        fs::remove_file(&linker)?;
        let error = observe(&linker, &runtime)
            .err()
            .ok_or("missing native linker was accepted")?;
        assert!(error.to_string().contains(&linker.display().to_string()), "{error}");
        return Ok(());
    }
    let root = tempfile::tempdir()?;
    let output = std::process::Command::new(std::env::current_exe()?)
        .args(["--exact", exact, "--nocapture"])
        .env(CONTROL_ROOT, root.path())
        .env("INCAN_HOME", root.path().join("incan-home"))
        .env("INCAN_OVEN_TRACE_FILE_DIGESTS", "1")
        .output()?;
    assert!(
        output.status.success(),
        "child failed: {} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
    let observations = String::from_utf8(output.stderr)?
        .lines()
        .filter_map(|line| line.strip_prefix("Oven file digest: "))
        .map(serde_json::from_str::<serde_json::Value>)
        .collect::<Result<Vec<_>, _>>()?;
    let reads = observations
        .iter()
        .map(|record| {
            record["input_bytes_read"]
                .as_u64()
                .ok_or("missing byte-read observation")
        })
        .collect::<Result<Vec<_>, _>>()?;
    // Cold reads, unchanged reuse, preserved-mtime edit, symlink retarget, unchanged reuse, then dangling-link refusal.
    assert_eq!(reads, [10, 11, 0, 0, 10, 0, 0, 11, 0, 0, 0]);
    for record in observations {
        assert_eq!(record["scheme"], "raw-sha256-v1");
        let path = record["path"].as_str().ok_or("missing observed path")?;
        assert!(Path::new(path).starts_with(root.path()));
    }
    Ok(())
}

/// Actual extern planning preserves renamed inputs, reobserves mutations and reuses unchanged content hashes.
#[test]
fn dev7_unit_plan_observation_preserves_identity_and_freshness() -> Result<(), Error> {
    assert_file_observation(
        "sdk_closure::digest_reuse_tests::dev7_unit_plan_observation_preserves_identity_and_freshness",
        ["renamed_native", "renamed_runtime"],
        |linker, llvm| {
            let mut prepared = unit("crates-io/control", "target", "[rust]\nname='control'", &[])?;
            prepared.root = linker.parent().ok_or("control parent missing")?.to_path_buf();
            let intent = oven_store::OvenBuildIntent {
                target: "control-target".to_string(),
                toolchain: "control-compiler".to_string(),
                profile: "debug".to_string(),
                features: Vec::new(),
            };
            let (plan, _) = unit_plan(
                &prepared,
                &intent,
                vec![
                    ("renamed_native".to_string(), linker.to_path_buf()),
                    ("renamed_runtime".to_string(), llvm.to_path_buf()),
                ],
                Vec::new(),
            )?;
            assert_eq!(plan.externs.len(), 2);
            assert_eq!(plan.externs[0].0, "renamed_native");
            assert_eq!(plan.externs[1].0, "renamed_runtime");
            assert_eq!(plan.dependency_search_paths, vec![prepared.root.clone(), prepared.root]);
            Ok(plan.caller_owned_library_digests)
        },
    )
}
