//! Real native compilation proves cold consumer preparation follows only its declared forward closure.

use super::{
    Current, NativeLoafConsumerReport, NativeLoafConsumerRequest, prepare_declared_native_loafs_in_store, projection,
};
use crate::native_loaf::NativeLoafClosure;
use oven_model::manifest::ProjectManifest;
use oven_store::store::{OvenStore, OvenStoreLimits};
use std::path::Path;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// An unrelated failing facet does not compile; selected child edits and restoration retain exact runtime behavior.
#[test]
fn cold_native_consumer_compiles_only_declared_transitive_units() -> TestResult {
    with_fixture(|root, request, store| {
        let mut original = None;
        for (phase, value, compilations, preparations) in [
            ("first", 7, 2, 1),
            ("repeat", 7, 0, 0),
            ("unrelated-edit", 7, 0, 0),
            ("child-edit", 9, 2, 1),
            ("restored", 7, 0, 1),
        ] {
            if phase == "unrelated-edit" {
                std::fs::write(root.join("unused/src/lib.rs"), "compile_error!(\"still unrelated\");\n")?;
            }
            if phase == "child-edit" || phase == "restored" {
                std::fs::write(
                    root.join("child/src/lib.rs"),
                    format!("pub const VALUE: u8 = {value};\n"),
                )?;
            }
            let prepared = prepare_declared_native_loafs_in_store(request, store)?;
            assert_eq!(prepared.report().compiled.len(), compilations, "{phase}");
            assert_eq!(prepared.report().preparation_calls, preparations, "{phase}");
            assert_eq!(prepared.report().prepared_units, preparations * 2, "{phase}");
            assert_eq!(prepared.closure().graph().units().len(), 2, "{phase}");
            assert_eq!(
                run_consumer(root, request.rustc, prepared.closure())?,
                value.to_string()
            );
            let identity = prepared
                .closure()
                .roots()
                .get("parent")
                .ok_or("declared parent root missing")?;
            match phase {
                "first" => original = Some(identity.clone()),
                "repeat" | "unrelated-edit" | "restored" => assert_eq!(Some(identity), original.as_ref()),
                _ => assert_ne!(Some(identity), original.as_ref()),
            }
        }
        assert!(!root.join("native/store").exists());
        Ok(())
    })
}

/// Cold traversal considers every original compatible child, so projection cannot silently resolve ambiguity.
#[test]
fn cold_native_consumer_refuses_transitive_ambiguity() -> TestResult {
    with_fixture(|root, request, store| {
        let mut graph: serde_json::Value = serde_json::from_slice(&std::fs::read(request.graph)?)?;
        graph["facets"]
            .as_array_mut()
            .ok_or("facets absent")?
            .push(serde_json::json!({"project":"child","features":[],"domain":"target"}));
        std::fs::write(request.graph, serde_json::to_vec(&graph)?)?;
        let error = prepare_declared_native_loafs_in_store(request, store)
            .err()
            .ok_or("ambiguous child admitted")?;
        assert!(error.to_string().contains("ambiguous"), "{error}");
        assert!(!root.join("native/consumer-hints").exists());
        Ok(())
    })
}

/// Missing selected features and unauthenticated resolution formats refuse before the producer can compile.
#[test]
fn cold_native_consumer_refuses_missing_features_and_resolution_edges() -> TestResult {
    with_fixture(|root, request, store| {
        let parent = root.join("parent/loaf.toml");
        let original = std::fs::read_to_string(&parent)?;
        std::fs::write(
            &parent,
            original.replace("path='../child'", "path='../child',features=['chosen']"),
        )?;
        let child = root.join("child/loaf.toml");
        std::fs::write(
            &child,
            format!("{}\n[project.features]\nchosen=[]\n", std::fs::read_to_string(&child)?),
        )?;
        let error = prepare_declared_native_loafs_in_store(request, store)
            .err()
            .ok_or("missing child feature admitted")?;
        assert!(
            error
                .to_string()
                .contains("has no selected target binding with its required features"),
            "{error}"
        );
        std::fs::write(parent, original)?;
        std::fs::write(
            root.join("lock.json"),
            r#"{"schema":"incan.oven.loaf-resolution/1","units":[]}"#,
        )?;
        let error = prepare_declared_native_loafs_in_store(request, store)
            .err()
            .ok_or("unauthenticated edges admitted")?;
        assert!(error.to_string().contains("authenticated resolution edges"), "{error}");
        Ok(())
    })
}

/// A physical subset cannot grant complete semantic authority; the full producer still observes unrelated units.
#[test]
fn cold_native_consumer_preserves_complete_producer_boundary() -> TestResult {
    with_fixture(|_, request, store| {
        let mut current = Current::read(request, &mut NativeLoafConsumerReport::default())?;
        let prepared = projection::prepare(request, store, &mut current)?;
        assert_eq!(prepared.graph().units().len(), 2);
        let error = prepared
            .request_observation()
            .err()
            .ok_or("projected preparation granted complete authority")?;
        assert!(
            error
                .to_string()
                .contains("complete native producer request observation is unavailable"),
            "{error}"
        );
        let error = crate::native_loaf::preparation::prepare_resolved_native_loafs_in_store(
            request.graph,
            request.index,
            request.blobs,
            request.output,
            request.rustc,
            request.target,
            request.profile,
            store,
        )
        .err()
        .ok_or("complete producer skipped supplied failing facet")?;
        assert!(error.to_string().contains("unrelated native facet compiled"), "{error}");
        Ok(())
    })
}

/// Supply one isolated real compiler fixture and bounded Store to each physical preparation scenario.
pub(super) fn with_fixture(
    run: impl FnOnce(&Path, &NativeLoafConsumerRequest<'_>, &OvenStore) -> TestResult,
) -> TestResult {
    let fixture = tempfile::tempdir()?;
    let root = fixture.path().canonicalize()?;
    write_fixture(&root)?;
    let rustc = crate::rustc::resolve_active_rustc()?;
    let target = crate::rustc::rustc_host_target(&rustc)?;
    let manifest = ProjectManifest::load(&root.join("loaf.toml"))?;
    let dependencies = manifest.rust_dependency_values();
    let store = OvenStore::new(
        root.join("store"),
        OvenStoreLimits::new(64 * 1024 * 1024, 64 * 1024 * 1024, 64 * 1024 * 1024),
    );
    let request = NativeLoafConsumerRequest {
        graph: &root.join("graph.json"),
        index: &root,
        blobs: &root,
        output: &root.join("native"),
        rustc: &rustc,
        target: &target,
        profile: "debug",
        dependencies: &dependencies,
        declaration_owner: &root,
        domain: "target",
    };
    run(&root, &request, &store)
}

/// Author selected path dependencies beside a valid but uncompilable unrelated package.
fn write_fixture(root: &Path) -> TestResult {
    for (name, dependencies, source) in [
        (
            "parent",
            "[dependencies]\nchild={loaf='child',path='../child'}\n",
            "pub fn value() -> u8 { child::VALUE }\n",
        ),
        ("child", "", "pub const VALUE: u8 = 7;\n"),
        ("unused", "", "compile_error!(\"unrelated native facet compiled\");\n"),
    ] {
        std::fs::create_dir_all(root.join(name).join("src"))?;
        std::fs::write(
            root.join(name).join("loaf.toml"),
            format!(
                "[project]\nname='{name}'\nversion='1.0.0'\n[rust]\nname='{name}'\ntype='lib'\nedition='2024'\n{dependencies}"
            ),
        )?;
        std::fs::write(root.join(name).join("src/lib.rs"), source)?;
    }
    std::fs::write(
        root.join("loaf.toml"),
        "[project]\nname='consumer'\nversion='1.0.0'\n[dependencies]\nparent={loaf='parent',path='parent'}\n",
    )?;
    std::fs::write(
        root.join("lock.json"),
        r#"{"schema":"incan.oven.loaf-resolution/2","units":[]}"#,
    )?;
    std::fs::write(
        root.join("graph.json"),
        serde_json::to_vec(&serde_json::json!({
            "index_commit":"0000000000000000000000000000000000000000", "registry_lock":"lock.json",
            "facets":[{"project":"parent","features":[],"domain":"target"},{"project":"child","features":[],"domain":"target"},{"project":"unused","features":[],"domain":"target"}]
        }))?,
    )?;
    Ok(())
}

/// Link and execute a real consumer using only the selected original native owners.
pub(super) fn run_consumer(root: &Path, rustc: &Path, closure: &NativeLoafClosure) -> TestResult<String> {
    let source = root.join("consumer.rs");
    std::fs::write(&source, "fn main() { print!(\"{}\", parent::value()); }\n")?;
    let output = root.join("consumer");
    let parent = closure
        .graph()
        .units()
        .get(closure.roots().get("parent").ok_or("parent root absent")?)
        .ok_or("parent unit absent")?;
    let mut command = std::process::Command::new(rustc);
    command
        .arg(&source)
        .arg("--edition=2024")
        .arg("-o")
        .arg(&output)
        .arg("--extern")
        .arg(format!(
            "parent={}",
            parent
                .native_owner
                .artifact_root
                .join(&parent.record().native.relative_path)
                .display()
        ));
    for unit in closure.graph().units().values() {
        command
            .arg("-L")
            .arg(format!("dependency={}", unit.native_owner.artifact_root.display()));
    }
    let compilation = command.output()?;
    if !compilation.status.success() {
        return Err(String::from_utf8_lossy(&compilation.stderr).into_owned().into());
    }
    let execution = std::process::Command::new(output).output()?;
    if !execution.status.success() {
        return Err(String::from_utf8_lossy(&execution.stderr).into_owned().into());
    }
    String::from_utf8(execution.stdout).map_err(Into::into)
}
