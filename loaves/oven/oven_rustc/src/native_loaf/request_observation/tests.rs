//! Producer-backed complete-request freshness and original-owner controls.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::super::tests::replace_owned_fixture;
use super::super::{
    NativeLoafFacet, NativeLoafGraph, NativeLoafPreparation, NativeLoafPreparationReport, NativeLoafPreparationRequest,
    prepare_native_loafs, prepare_resolved_native_loafs,
};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// Two independent real local units; one remains outside the selected consumer's physical closure.
struct Fixture {
    root: tempfile::TempDir,
    rustc: PathBuf,
    target: String,
    lock: PathBuf,
    document: PathBuf,
    output: PathBuf,
    facets: [NativeLoafFacet; 2],
}

impl Fixture {
    /// Prepare ordinary authored Loafs and the active native compiler, with no registry or fabricated native outputs.
    fn new() -> TestResult<Self> {
        let root = tempfile::tempdir()?;
        let lock = root.path().join("lock.json");
        fs::write(&lock, r#"{"schema":"incan.oven.loaf-resolution/2","units":[]}"#)?;
        for name in ["selected", "unrelated"] {
            let project = root.path().join(name);
            fs::create_dir_all(project.join("src"))?;
            fs::write(
                project.join("loaf.toml"),
                format!(
                    "[project]\nname='{name}'\nversion='1.0.0'\n[project.features]\nextra=[]\n[rust]\nname='{name}'\ntype='lib'\nedition='2024'\n"
                ),
            )?;
            fs::write(project.join("src/lib.rs"), "pub fn value() -> u8 { 1 }\n")?;
        }
        let facets = ["selected", "unrelated"].map(|name| NativeLoafFacet {
            project: name.into(),
            features: Vec::new(),
            domain: "target".to_string(),
        });
        let document = root.path().join("graph.json");
        fs::write(
            &document,
            serde_json::to_vec(&serde_json::json!({
                "index_commit": "0000000000000000000000000000000000000000",
                "registry_lock": "lock.json", "facets": facets,
            }))?,
        )?;
        let rustc = crate::rustc::resolve_active_rustc()?;
        let target = crate::rustc::rustc_host_target(&rustc)?;
        let output = root.path().join("native");
        Ok(Self {
            root,
            rustc,
            target,
            lock,
            document,
            output,
            facets,
        })
    }

    /// Supply the actual direct request, which legitimately has no resolved graph document.
    fn request(&self) -> NativeLoafPreparationRequest<'_> {
        NativeLoafPreparationRequest {
            lock: &self.lock,
            blobs: self.root.path(),
            output: &self.output,
            rustc: &self.rustc,
            index: self.root.path(),
            index_commit: "0000000000000000000000000000000000000000",
            target: &self.target,
            profile: "debug",
            facet_owner: self.root.path(),
            facets: &self.facets,
        }
    }

    /// Invoke the real resolved entrypoint against the same supplied lock/facets, without any SDK catalog.
    fn resolved(&self) -> TestResult<NativeLoafPreparation> {
        Ok(prepare_resolved_native_loafs(
            &self.document,
            self.root.path(),
            self.root.path(),
            &self.output,
            &self.rustc,
            &self.target,
            "debug",
        )?)
    }
}

/// Complete observation retains original owners outside selected roots and changes when that unselected source changes.
#[test]
fn dev7_native_request_observation_direct_complete_first_repeat_and_unselected_change() -> TestResult {
    let fixture = Fixture::new()?;
    let prepared = prepare_native_loafs(&fixture.request())?;
    assert_eq!(prepared.report().compiled.len(), 2);
    let observed = prepared.request_observation()?;
    let original_digest = observed.verified_digest()?.to_string();
    let selected = prepared
        .graph()
        .units()
        .values()
        .find(|unit| unit.record().source.loaf == "selected")
        .ok_or("selected unit missing")?;
    let selected_identity = selected.identity().to_string();
    let physical = prepared.graph().select(&[selected.declared_root("renamed")?])?;
    assert_eq!(physical.graph().units().len(), 1);
    assert_eq!(observed.graph().units().len(), 2);
    for (identity, original) in prepared.graph().units() {
        let retained = observed
            .graph()
            .units()
            .get(identity)
            .ok_or("full original owner missing")?;
        assert!(Arc::ptr_eq(original, retained));
        assert!(Arc::ptr_eq(&original.record_owner, &retained.record_owner));
        assert!(Arc::ptr_eq(&original.native_owner, &retained.native_owner));
    }
    let cloned = observed.clone();
    drop(prepared);
    assert_eq!(cloned.verified_digest()?, original_digest);
    let repeated = prepare_native_loafs(&fixture.request())?;
    assert!(repeated.report().compiled.is_empty());
    assert_eq!(repeated.report().reused.len(), 2);
    assert_eq!(repeated.request_observation()?.verified_digest()?, original_digest);
    let unrelated = fixture.root.path().join("unrelated/src/lib.rs");
    let original_source = fs::read(&unrelated)?;
    replace_owned_fixture(&unrelated, b"pub fn value() -> u8 { 2 }\n")?;
    assert!(observed.verified_digest().is_err());
    assert!(cloned.verify().is_err());
    let changed = prepare_native_loafs(&fixture.request())?;
    assert_eq!(changed.report().compiled.len(), 1);
    assert_eq!(changed.report().reused.len(), 1);
    let changed_observation = changed.request_observation()?;
    assert_ne!(changed_observation.verified_digest()?, original_digest);
    assert!(changed_observation.graph().units().contains_key(&selected_identity));
    replace_owned_fixture(&unrelated, &original_source)?;
    assert_eq!(observed.verified_digest()?, original_digest);
    assert!(changed_observation.verify().is_err());
    Ok(())
}

/// Resolved graph, exact lock bytes, selected features and local topology all remain current inputs at handoffs.
#[test]
fn dev7_native_request_observation_resolved_lock_graph_features_and_source_topology() -> TestResult {
    let fixture = Fixture::new()?;
    let prepared = fixture.resolved()?;
    let observed = prepared.request_observation()?;
    let digest = observed.verified_digest()?.to_string();
    let repeated = fixture.resolved()?;
    assert!(repeated.report().compiled.is_empty());
    assert_eq!(repeated.request_observation()?.verified_digest()?, digest);
    for document in [&fixture.document, &fixture.lock] {
        let original = fs::read(document)?;
        let mut changed = original.clone();
        changed.push(b'\n');
        replace_owned_fixture(document, &changed)?;
        assert!(
            observed.verify().is_err(),
            "changed request document accepted: {}",
            document.display()
        );
        replace_owned_fixture(document, &original)?;
        assert_eq!(observed.verified_digest()?, digest);
    }
    let graph_bytes = fs::read(&fixture.document)?;
    let mut graph: serde_json::Value = serde_json::from_slice(&graph_bytes)?;
    graph["facets"][1]["features"] = serde_json::json!(["extra"]);
    replace_owned_fixture(&fixture.document, &serde_json::to_vec(&graph)?)?;
    assert!(observed.verify().is_err());
    replace_owned_fixture(&fixture.document, &graph_bytes)?;
    let added = fixture.root.path().join("unrelated/src/new_member.rs");
    fs::write(&added, "pub const VALUE: u8 = 3;\n")?;
    assert!(observed.verify().is_err());
    fs::remove_file(&added)?;
    assert_eq!(observed.verified_digest()?, digest);
    let source = fixture.root.path().join("unrelated/src/lib.rs");
    let moved = fixture.root.path().join("removed-source");
    fs::rename(&source, &moved)?;
    assert!(observed.verify().is_err());
    fs::rename(&moved, &source)?;
    assert_eq!(observed.verified_digest()?, digest);
    let declaration = fixture.root.path().join("unrelated/loaf.toml");
    let original = fs::read(&declaration)?;
    let changed = String::from_utf8(original.clone())?.replace("extra=[]", "extra=['other']\nother=[]");
    replace_owned_fixture(&declaration, changed.as_bytes())?;
    assert!(observed.verify().is_err());
    replace_owned_fixture(&declaration, &original)?;
    assert_eq!(observed.verified_digest()?, digest);
    Ok(())
}

/// Original record/native payloads and outputs are rechecked even for units outside the physical consumer subset.
#[test]
fn dev7_native_request_observation_refuses_original_owner_corruption_and_substitution() -> TestResult {
    let fixture = Fixture::new()?;
    let prepared = prepare_native_loafs(&fixture.request())?;
    let observed = prepared.request_observation()?;
    let digest = observed.verified_digest()?.to_string();
    let unit = observed
        .graph()
        .units()
        .values()
        .find(|unit| unit.record().source.loaf == "unrelated")
        .ok_or("unrelated unit missing")?;
    let record_payload = payload_path(&unit.record_owner.artifact_root)?;
    let native_payload = payload_path(&unit.native_owner.artifact_root)?;
    let output = unit.output()?;
    for path in [&record_payload, &native_payload, &output] {
        let original = fs::read(path)?;
        let mut changed = original.clone();
        *changed.first_mut().ok_or("original owner bytes missing")? ^= 1;
        replace_owned_fixture(path, &changed)?;
        assert!(
            observed.verify().is_err(),
            "original owner corruption accepted: {}",
            path.display()
        );
        replace_owned_fixture(path, &original)?;
        assert_eq!(observed.verified_digest()?, digest);
    }
    let retained = fixture.root.path().join("original-record-payload");
    fs::rename(&record_payload, &retained)?;
    assert!(observed.verify().is_err());
    fs::write(&record_payload, b"a substituted record payload")?;
    assert!(observed.verify().is_err());
    fs::remove_file(&record_payload)?;
    fs::rename(&retained, &record_payload)?;
    assert_eq!(observed.verified_digest()?, digest);
    Ok(())
}

/// Synthetic and rooted-only preparations cannot turn arbitrary graph DTOs into complete producer authority.
#[test]
fn dev7_native_request_observation_refuses_synthetic_preparation() -> TestResult {
    let synthetic = NativeLoafPreparation {
        graph: NativeLoafGraph::default(),
        observation: None,
        report: NativeLoafPreparationReport {
            compiled: Vec::new(),
            reused: Vec::new(),
            seconds: 0.0,
        },
    };
    assert!(synthetic.request_observation().is_err());
    Ok(())
}

/// Even a genuine empty producer request retains exact compiler/target/profile authority.
#[test]
fn dev7_native_request_observation_empty_request_binds_compiler_intent() -> TestResult {
    let fixture = Fixture::new()?;
    let request = NativeLoafPreparationRequest {
        facets: &[],
        ..fixture.request()
    };
    let prepared = prepare_native_loafs(&request)?;
    assert!(prepared.graph().units().is_empty());
    assert!(prepared.report().compiled.is_empty());
    let observed = prepared.request_observation()?;
    let intent = oven_store::OvenBuildIntent {
        target: fixture.target.clone(),
        profile: "debug".to_string(),
        toolchain: crate::rustc::rustc_identity(&fixture.rustc)?,
        features: Vec::new(),
    };
    observed.verify_intent(&fixture.rustc, &intent)?;
    for wrong in [
        oven_store::OvenBuildIntent {
            target: "wrong-target".to_string(),
            ..intent.clone()
        },
        oven_store::OvenBuildIntent {
            profile: "release".to_string(),
            ..intent.clone()
        },
        oven_store::OvenBuildIntent {
            toolchain: "wrong-toolchain".to_string(),
            ..intent.clone()
        },
    ] {
        assert!(observed.verify_intent(&fixture.rustc, &wrong).is_err());
    }
    let other_compiler = fixture.root.path().join("equal-byte-rustc");
    fs::copy(&fixture.rustc, &other_compiler)?;
    assert!(observed.verify_intent(&other_compiler, &intent).is_err());
    let repeated = prepare_native_loafs(&request)?;
    assert_eq!(
        repeated.request_observation()?.verified_digest()?,
        observed.verified_digest()?
    );
    Ok(())
}

/// Actual host compilation remains usable when full semantic observation lacks independent host-stdlib binding.
///
/// The real active compiler must have wasm32-wasip1 rust-std installed; absence fails rather than skipping a pass.
#[test]
fn dev7_native_request_observation_unsupported_host_semantics_preserves_physical_preparation() -> TestResult {
    let fixture = Fixture::new()?;
    let target = "wasm32-wasip1";
    assert_ne!(fixture.target, target);
    crate::sdk_closure::compiler_closure_digest(&fixture.rustc, target)?;
    let index = fixture.root.path().join("host-index");
    let blobs = fixture.root.path().join("host-blobs");
    fs::create_dir_all(index.join("index/crates-io"))?;
    fs::create_dir_all(&blobs)?;
    let declaration =
        b"[project]\nname='crates-io/hostonly'\nversion='1.0.0'\n[rust]\nname='hostonly'\ntype='lib'\nedition='2024'\n";
    let archive = source_archive(&[
        ("loaf.toml", declaration),
        ("src/lib.rs", b"pub fn value() -> u8 { 1 }\n"),
    ]);
    let digest = oven_store::digest_bytes(&archive);
    fs::write(
        blobs.join(format!(
            "{}.tar",
            digest.strip_prefix("sha256:").ok_or("archive digest missing")?
        )),
        archive,
    )?;
    fs::write(index.join("hostonly.toml"), declaration)?;
    fs::write(
        index.join("index/crates-io/hostonly"),
        serde_json::to_vec(&serde_json::json!({
            "vers":"1.0.0", "cksum":digest, "manifest":"hostonly.toml", "features":{},
        }))?,
    )?;
    git(&index, &["init", "--quiet"])?;
    git(&index, &["add", "."])?;
    git(
        &index,
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "--quiet",
            "-m",
            "host source authority",
        ],
    )?;
    let commit = String::from_utf8(git(&index, &["rev-parse", "HEAD"])?)?
        .trim()
        .to_string();
    fs::write(
        &fixture.lock,
        serde_json::to_vec(&serde_json::json!({
            "schema":"incan.oven.loaf-resolution/2", "units":[{
                "loaf":"crates-io/hostonly", "version":"1.0.0", "archive_digest":digest,
                "domain":"host", "features":[], "target_predicates":[], "edges":[],
            }],
        }))?,
    )?;
    let request = NativeLoafPreparationRequest {
        index: &index,
        index_commit: &commit,
        blobs: &blobs,
        facets: &[],
        target,
        ..fixture.request()
    };
    let prepared = prepare_native_loafs(&request)?;
    assert_eq!(prepared.report().compiled.len(), 1);
    assert_eq!(prepared.graph().units().len(), 1);
    let unit = prepared
        .graph()
        .units()
        .values()
        .next()
        .ok_or("actual host native unit missing")?;
    assert_eq!(unit.record().source.domain, "host");
    assert_eq!(unit.record().recipe.intent.target, fixture.target);
    assert!(unit.output()?.is_file());
    let error = prepared
        .request_observation()
        .err()
        .ok_or("unsupported host semantic observation granted")?;
    assert!(
        error
            .to_string()
            .contains("independently bound host standard-library authority"),
        "{error}"
    );
    let repeated = prepare_native_loafs(&request)?;
    assert!(repeated.report().compiled.is_empty());
    assert_eq!(repeated.report().reused.len(), 1);
    assert!(repeated.request_observation().is_err());
    Ok(())
}

/// Locate the original owner payload only within its actual admitted immutable entry.
fn payload_path(artifact_root: &Path) -> TestResult<PathBuf> {
    Ok(artifact_root
        .parent()
        .ok_or("original owner entry missing")?
        .join("payload"))
}

/// Actual pinned registry facts/generated members and named edges remain part of the complete producer observation.
#[test]
fn dev7_native_request_observation_registry_facts_edges_archives_and_pin() -> TestResult {
    use oven_model::manifest::{RustFactCompileEnvironment, RustFactOut, RustFactRecord};

    let fixture = Fixture::new()?;
    let index = fixture.root.path().join("index");
    let blobs = fixture.root.path().join("blobs");
    fs::create_dir_all(index.join("index/crates-io"))?;
    fs::create_dir_all(&blobs)?;
    let generated = b"pub fn value() -> u8 { 7 }\n";
    let fact = RustFactRecord {
        toolchain: crate::rustc::rustc_identity(&fixture.rustc)?,
        target: fixture.target.clone(),
        profile: "debug".to_string(),
        features: Vec::new(),
        cfg: vec!["actual_generated_fact".to_string()],
        out: vec![RustFactOut {
            name: "generated.rs".to_string(),
            path: "facts/generated.rs".to_string(),
            digest: oven_store::digest_bytes(generated),
        }],
        environment: vec![RustFactCompileEnvironment {
            name: "GENERATED_INPUT".to_string(),
            literal: None,
            out: Some("generated.rs".to_string()),
        }],
        link: Vec::new(),
        tool: Vec::new(),
        harvested_from: None,
    };
    let mut bindings = Vec::new();
    let mut archives = Vec::new();
    for name in ["child", "parent", "unrelated"] {
        let dependencies = if name == "parent" {
            "[dependencies]\nrenamed={loaf='crates-io/child',version='1.0.0'}\n"
        } else {
            ""
        };
        let mut manifest: toml::Value = toml::from_str(&format!(
            "[project]\nname='crates-io/{name}'\nversion='1.0.0'\n[rust]\nname='{name}_native'\ntype='lib'\nedition='2024'\n{dependencies}"
        ))?;
        if name == "unrelated" {
            manifest
                .get_mut("rust")
                .and_then(toml::Value::as_table_mut)
                .ok_or("registry Rust facet missing")?
                .insert(
                    "facts".to_string(),
                    toml::Value::Array(vec![toml::Value::try_from(&fact)?]),
                );
            let generated_path = index.join("crates-io/unrelated/1.0.0/facts/generated.rs");
            fs::create_dir_all(generated_path.parent().ok_or("generated parent missing")?)?;
            fs::write(generated_path, generated)?;
        }
        let declaration = toml::to_string(&manifest)?;
        let source = match name {
            "parent" => "pub fn value() -> u8 { renamed::value() }\n",
            "unrelated" => {
                "#[cfg(not(actual_generated_fact))] compile_error!(\"fact not applied\"); include!(env!(\"GENERATED_INPUT\"));\n"
            }
            _ => "pub fn value() -> u8 { 1 }\n",
        };
        let archive = source_archive(&[("loaf.toml", declaration.as_bytes()), ("src/lib.rs", source.as_bytes())]);
        let digest = oven_store::digest_bytes(&archive);
        let archive_path = blobs.join(format!(
            "{}.tar",
            digest.strip_prefix("sha256:").ok_or("archive digest missing")?
        ));
        fs::write(&archive_path, archive)?;
        archives.push(archive_path);
        let manifest_path = format!("{name}.toml");
        fs::write(index.join(&manifest_path), declaration)?;
        fs::write(
            index.join(format!("index/crates-io/{name}")),
            serde_json::to_vec(&serde_json::json!({
                "vers":"1.0.0", "cksum":digest, "manifest":manifest_path, "features":{},
            }))?,
        )?;
        bindings.push(serde_json::json!({
            "loaf":format!("crates-io/{name}"), "version":"1.0.0", "archive_digest":digest,
            "domain":"target", "features":[], "target_predicates":[],
            "edges":if name == "parent" { vec![serde_json::json!({"dependency_key":"renamed", "loaf":"crates-io/child", "version":"1.0.0", "domain":"target"})] } else { Vec::new() },
        }));
    }
    git(&index, &["init", "--quiet"])?;
    git(&index, &["add", "."])?;
    git(
        &index,
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "--quiet",
            "-m",
            "native fact authority",
        ],
    )?;
    let commit = String::from_utf8(git(&index, &["rev-parse", "HEAD"])?)?
        .trim()
        .to_string();
    fs::write(
        &fixture.lock,
        serde_json::to_vec(&serde_json::json!({"schema":"incan.oven.loaf-resolution/2", "units":bindings}))?,
    )?;
    let request = NativeLoafPreparationRequest {
        index: &index,
        index_commit: &commit,
        blobs: &blobs,
        facets: &[],
        ..fixture.request()
    };
    let prepared = prepare_native_loafs(&request)?;
    assert_eq!(prepared.report().compiled.len(), 3);
    let observed = prepared.request_observation()?;
    let digest = observed.verified_digest()?.to_string();
    let parent = prepared
        .graph()
        .units()
        .values()
        .find(|unit| unit.record().source.loaf == "crates-io/parent")
        .ok_or("parent missing")?;
    assert_eq!(parent.record().dependencies.len(), 1);
    assert_eq!(parent.record().dependencies[0].alias, "renamed");
    assert_eq!(
        prepared
            .graph()
            .select(&[parent.declared_root("root_alias")?])?
            .graph()
            .units()
            .len(),
        2
    );
    assert_eq!(observed.graph().units().len(), 3);
    let unrelated = observed
        .graph()
        .units()
        .values()
        .find(|unit| unit.record().source.loaf == "crates-io/unrelated")
        .ok_or("fact unit missing")?;
    assert_eq!(
        unrelated
            .record()
            .recipe
            .sources
            .build_unit_inputs
            .get("sdk-build-fact"),
        Some(&serde_json::to_string(&fact)?)
    );
    let generated_path = unrelated
        .native_owner
        .artifact_root
        .join("source/.oven-out/generated.rs");
    assert_eq!(fs::read(&generated_path)?, generated);
    replace_owned_fixture(&generated_path, b"pub fn value() -> u8 { 8 }\n")?;
    assert!(observed.verify().is_err());
    replace_owned_fixture(&generated_path, generated)?;
    assert_eq!(observed.verified_digest()?, digest);
    let archive = &archives[2];
    let original = fs::read(archive)?;
    replace_owned_fixture(archive, b"corrupt source archive")?;
    assert!(observed.verify().is_err());
    replace_owned_fixture(archive, &original)?;
    assert_eq!(observed.verified_digest()?, digest);
    // Mutable index worktree content is not authority: every fact read still uses the original pinned commit.
    fs::write(index.join("unrelated.toml"), "hostile uncommitted manifest")?;
    assert_eq!(observed.verified_digest()?, digest);
    let repeated = prepare_native_loafs(&request)?;
    assert!(repeated.report().compiled.is_empty());
    assert_eq!(repeated.report().reused.len(), 3);
    assert_eq!(repeated.request_observation()?.verified_digest()?, digest);
    Ok(())
}

/// Write only deterministic regular tar members in the existing admitted uncompressed source-archive format.
fn source_archive(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut archive = Vec::new();
    for (name, contents) in files {
        let mut header = [0u8; 512];
        header[..name.len()].copy_from_slice(name.as_bytes());
        header[124..136].copy_from_slice(format!("{:011o}\0", contents.len()).as_bytes());
        header[148..156].fill(b' ');
        header[156] = b'0';
        let checksum: usize = header.iter().map(|byte| usize::from(*byte)).sum();
        header[148..156].copy_from_slice(format!("{checksum:06o}\0 ").as_bytes());
        archive.extend_from_slice(&header);
        archive.extend_from_slice(contents);
        archive.resize(archive.len().div_ceil(512) * 512, 0);
    }
    archive.resize(archive.len() + 1024, 0);
    archive
}

/// Run Git only in the task-owned temporary pinned-index fixture, retaining actual process diagnostics.
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
