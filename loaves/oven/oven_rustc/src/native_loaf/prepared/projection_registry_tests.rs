//! Real archive, pinned index and native execution controls for cold registry consumer projection.

use super::projection_tests::{run_consumer, with_fixture};
use super::{NativeLoafConsumerRequest, prepare_declared_native_loafs_in_store};
use oven_model::manifest::ProjectManifest;
use std::path::Path;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// Preserve exact registry edges and codegen receipts, skip an unrelated failing archive, and refuse broken edges.
#[test]
fn cold_registry_consumer_preserves_edges_codegen_and_runtime() -> TestResult {
    with_fixture(|root, original, store| {
        let mut lock = write_registry(root)?;
        let document: serde_json::Value = serde_json::from_slice(&std::fs::read(root.join("graph.json"))?)?;
        let pin = document["index_commit"].as_str().ok_or("fixture pin missing")?;
        let mut transport = crate::sdk_closure::current_inputs::PinnedIndexBatch::open(root, pin)?;
        let batched = transport.work().requests > 0;
        transport.finish()?;
        std::fs::write(
            root.join("loaf.toml"),
            "[project]\nname='consumer'\nversion='1.0.0'\n[dependencies]\nparent={loaf='crates-io/parent',version='^1.0'}\n",
        )?;
        let manifest = ProjectManifest::load(&root.join("loaf.toml"))?;
        let dependencies = manifest.rust_dependency_values();
        let request = NativeLoafConsumerRequest {
            dependencies: &dependencies,
            ..*original
        };
        let mut identity = None;
        for repeat in [false, true] {
            let prepared = prepare_declared_native_loafs_in_store(&request, store)?;
            assert_eq!(prepared.report().compiled.len(), if repeat { 0 } else { 2 });
            assert_eq!(prepared.report().prepared_units, if repeat { 0 } else { 2 });
            assert_eq!(prepared.report().preparation_calls, usize::from(!repeat));
            let work = &prepared.report().preparation_index_reads;
            if repeat {
                assert_eq!(work.processes, 0);
                assert_eq!(work.file_requests, 0);
            } else {
                assert_eq!(work.processes, if batched { 3 } else { 11 });
                assert_eq!(work.requests, if batched { 5 } else { 0 });
                assert_eq!(work.file_requests, 4);
                assert_eq!(work.blob_reads, 4);
                assert_eq!(work.cache_hits, 0);
                assert!(work.blob_bytes > 0);
            }
            assert_eq!(prepared.closure().graph().units().len(), 2);
            assert_eq!(run_consumer(root, request.rustc, prepared.closure())?, "7");
            let parent = prepared
                .closure()
                .graph()
                .units()
                .get(
                    prepared
                        .closure()
                        .roots()
                        .get("parent")
                        .ok_or("registry parent absent")?,
                )
                .ok_or("registry parent owner absent")?;
            let policy: serde_json::Value = serde_json::from_str(
                parent
                    .record()
                    .recipe
                    .sources
                    .build_unit_inputs
                    .get("sdk-codegen-options")
                    .ok_or("codegen receipt absent")?,
            )?;
            assert_eq!(policy, lock["unit_codegen"][0]["options"]);
            assert_eq!(parent.record().dependencies.len(), 1);
            assert_eq!(parent.record().dependencies[0].alias, "child");
            if repeat {
                assert_eq!(identity.as_deref(), Some(parent.identity()));
            } else {
                identity = Some(parent.identity().to_string());
            }
        }
        let original_lock = lock.clone();
        for missing in [true, false] {
            lock = original_lock.clone();
            if missing {
                lock["units"].as_array_mut().ok_or("units absent")?.remove(0);
            } else {
                let child = lock["units"][0].clone();
                lock["units"].as_array_mut().ok_or("units absent")?.push(child);
            }
            std::fs::write(root.join("lock.json"), serde_json::to_vec(&lock)?)?;
            let error = prepare_declared_native_loafs_in_store(&request, store)
                .err()
                .ok_or("broken edge admitted")?;
            assert!(
                error
                    .to_string()
                    .contains("current native resolution edge is missing or ambiguous"),
                "{error}"
            );
        }
        Ok(())
    })
}

/// Author deterministic ordinary registry inputs whose unrelated source fails if the producer ever compiles it.
fn write_registry(root: &Path) -> TestResult<serde_json::Value> {
    std::fs::create_dir_all(root.join("index/crates-io"))?;
    let mut units = Vec::new();
    for name in ["child", "parent", "unused"] {
        let project = root.join(name);
        let manifest = std::fs::read_to_string(project.join("loaf.toml"))?
            .replace(
                &format!("name='{name}'\nversion"),
                &format!("name='crates-io/{name}'\nversion"),
            )
            .replace("loaf='child',path='../child'", "loaf='crates-io/child',version='^1.0'");
        std::fs::write(project.join("loaf.toml"), &manifest)?;
        let archive = root.join(format!("{name}.tar"));
        let output = std::process::Command::new("tar")
            .env("COPYFILE_DISABLE", "1")
            .args(["--format=ustar", "-cf"])
            .arg(&archive)
            .arg("-C")
            .arg(&project)
            .args(["loaf.toml", "src/lib.rs"])
            .output()?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).into_owned().into());
        }
        let digest = oven_store::digest_bytes(&std::fs::read(&archive)?);
        std::fs::rename(
            &archive,
            root.join(format!(
                "{}.tar",
                digest.strip_prefix("sha256:").ok_or("digest prefix absent")?
            )),
        )?;
        std::fs::write(root.join(format!("{name}.toml")), &manifest)?;
        std::fs::write(
            root.join(format!("index/crates-io/{name}")),
            serde_json::to_vec(&serde_json::json!({
                "vers":"1.0.0", "cksum":digest, "manifest":format!("{name}.toml"), "features":{}
            }))?,
        )?;
        let edges = if name == "parent" {
            serde_json::json!([{"dependency_key":"child","loaf":"crates-io/child","version":"1.0.0","domain":"target"}])
        } else {
            serde_json::json!([])
        };
        units.push(
            serde_json::json!({"loaf":format!("crates-io/{name}"),"version":"1.0.0","archive_digest":digest,
            "domain":"target","features":[],"target_predicates":[],"edges":edges}),
        );
    }
    git(root, &["init", "--quiet"])?;
    git(root, &["add", "index", "child.toml", "parent.toml", "unused.toml"])?;
    git(
        root,
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "--quiet",
            "-m",
            "test cold native registry projection",
        ],
    )?;
    let commit = String::from_utf8(git(root, &["rev-parse", "HEAD"])?)?
        .trim()
        .to_string();
    std::fs::write(
        root.join("graph.json"),
        serde_json::to_vec(&serde_json::json!({
            "index_commit":commit,"registry_lock":"lock.json","facets":[]
        }))?,
    )?;
    let lock = serde_json::json!({"schema":"incan.oven.loaf-resolution/2","units":units,"unit_codegen":[
        {"loaf":"crates-io/parent","version":"1.0.0","domain":"target","options":{"opt_level":"1","debug_assertions":true,"overflow_checks":true}},
        {"loaf":"crates-io/unused","version":"1.0.0","domain":"target","options":{"opt_level":"0","debug_assertions":false,"overflow_checks":false}}
    ]});
    std::fs::write(root.join("lock.json"), serde_json::to_vec(&lock)?)?;
    Ok(lock)
}

/// Run Git only inside the temporary fixture index and retain command diagnostics on failure.
fn git(root: &Path, arguments: &[&str]) -> TestResult<Vec<u8>> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(arguments)
        .output()?;
    if !output.status.success() {
        return Err(format!("fixture git {arguments:?}: {}", String::from_utf8_lossy(&output.stderr)).into());
    }
    Ok(output.stdout)
}
