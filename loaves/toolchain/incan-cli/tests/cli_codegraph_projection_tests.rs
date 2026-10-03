#![allow(dead_code)] // Split test binaries intentionally use different parts of shared support fragments.

//! `inspect codegraph`, `inspect rust`, checked-binding facts, and the semantic projections that share a project
//! identity.
//!
//! One of nine roots split out of the former `tests/cli_integration.rs`. The shared command surface lives in
//! `incan_test_support::cli_project`.

include!("support/cli_codegraph_and_inspection_tests_root.rs");

use incan_test_support as support;

#[test]
fn inspect_codegraph_exports_multifile_imports_and_public_symbols() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let src_dir = tmp.path().join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(
        tmp.path().join("loaf.toml"),
        r#"[project]
name = "graph_demo"
version = "0.1.0"
"#,
    )?;
    fs::write(
        src_dir.join("helpers.incn"),
        r#"pub model Widget:
    pub value: int

pub def make_widget(value: int) -> Widget:
    return Widget(value=value)
"#,
    )?;
    let main_path = src_dir.join("main.incn");
    fs::write(
        &main_path,
        r#"import helpers
from helpers import make_widget

enum Signal:
    Ready

def local_value() -> int:
    return 3

def qualified_value() -> int:
    return helpers.make_widget(std.builtins.len([1, 2])).value

def ready() -> Signal:
    return Signal.Ready()

pub def entrypoint() -> int:
    return make_widget(local_value()).value
"#,
    )?;

    let first = run_incan(
        tmp.path(),
        &[
            "inspect",
            "codegraph",
            main_path.to_str().ok_or("main path was not valid UTF-8")?,
            "--format",
            "jsonl",
        ],
    )?;
    assert_success(&first, "incan inspect codegraph");
    let second = run_incan(
        tmp.path(),
        &[
            "inspect",
            "codegraph",
            main_path.to_str().ok_or("main path was not valid UTF-8")?,
            "--format",
            "jsonl",
        ],
    )?;
    assert_success(&second, "second incan inspect codegraph");
    assert_eq!(first.stdout, second.stdout, "codegraph JSONL should be deterministic");

    let records = parse_jsonl_stdout(&first)?;
    assert_codegraph_record_contract(&records);
    assert_eq!(records[0]["record"], serde_json::json!("header"));
    assert_eq!(records[0]["package"]["name"], serde_json::json!("graph_demo"));
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("import")
            && record["path"] == serde_json::json!("helpers")
            && record["items"].as_array().is_some_and(|items| {
                items
                    .iter()
                    .any(|item| item.as_str().is_some_and(|value| value == "make_widget"))
            })
    }));
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("declaration")
            && record["kind"] == serde_json::json!("function")
            && record["name"] == serde_json::json!("entrypoint")
            && record["visibility"] == serde_json::json!("public")
    }));
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("export")
            && record["name"] == serde_json::json!("entrypoint")
            && record["kind"] == serde_json::json!("declaration")
    }));
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("containment")
            && record["kind"] == serde_json::json!("module_contains_declaration")
    }));
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("call")
            && record["kind"] == serde_json::json!("function")
            && record["callee"] == serde_json::json!("make_widget")
            && record["argument_count"] == serde_json::json!(1)
            && record["target_id"].as_str().is_some_and(|target_id| {
                records.iter().any(|candidate| {
                    candidate["record"] == serde_json::json!("declaration")
                        && candidate["id"] == serde_json::json!(target_id)
                        && candidate["name"] == serde_json::json!("make_widget")
                })
            })
            && record["canonical_identity"]["declaration_name"] == serde_json::json!("make_widget")
            && record["canonical_identity"]["origin"]["kind"] == serde_json::json!("module")
            && record["provenance"] == serde_json::json!("checked")
    }));
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("call")
            && record["callee"] == serde_json::json!("helpers.make_widget")
            && record["canonical_identity"]["declaration_name"] == serde_json::json!("make_widget")
            && record["canonical_identity"]["origin"]["kind"] == serde_json::json!("module")
            && record["target_id"].as_str().is_some_and(|target_id| {
                records.iter().any(|candidate| {
                    candidate["record"] == serde_json::json!("declaration")
                        && candidate["id"] == serde_json::json!(target_id)
                        && candidate["name"] == serde_json::json!("make_widget")
                })
            })
            && record["provenance"] == serde_json::json!("checked")
    }));
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("call")
            && record["callee"] == serde_json::json!("std.builtins.len")
            && record["canonical_identity"]["declaration_name"] == serde_json::json!("len")
            && record["canonical_identity"]["origin"]["kind"] == serde_json::json!("builtin")
            && record["target_id"] == serde_json::Value::Null
            && record["provenance"] == serde_json::json!("checked")
    }));
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("call")
            && record["callee"] == serde_json::json!("Signal.Ready")
            && record["canonical_identity"]["declaration_name"] == serde_json::json!("Ready")
            && record["canonical_identity"]["declaration_kind"] == serde_json::json!("variant")
            && record["canonical_identity"]["origin"]["kind"] == serde_json::json!("module")
            && record["target_id"] == serde_json::Value::Null
            && record["provenance"] == serde_json::json!("checked")
    }));
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("call")
            && record["kind"] == serde_json::json!("function")
            && record["callee"] == serde_json::json!("local_value")
            && record["argument_count"] == serde_json::json!(0)
            && record["target_id"].as_str().is_some_and(|target_id| {
                records.iter().any(|candidate| {
                    candidate["record"] == serde_json::json!("declaration")
                        && candidate["id"] == serde_json::json!(target_id)
                        && candidate["name"] == serde_json::json!("local_value")
                })
            })
            && record["canonical_identity"]["declaration_name"] == serde_json::json!("local_value")
            && record["provenance"] == serde_json::json!("checked")
    }));
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("reference")
            && record["kind"] == serde_json::json!("identifier")
            && record["name"] == serde_json::json!("make_widget")
            && record["target_id"].as_str().is_some_and(|target_id| {
                records.iter().any(|candidate| {
                    candidate["record"] == serde_json::json!("declaration")
                        && candidate["id"] == serde_json::json!(target_id)
                        && candidate["name"] == serde_json::json!("make_widget")
                })
            })
            && record["canonical_identity"]["declaration_name"] == serde_json::json!("make_widget")
            && record["provenance"] == serde_json::json!("checked")
    }));
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("reference")
            && record["kind"] == serde_json::json!("identifier")
            && record["name"] == serde_json::json!("local_value")
            && record["target_id"].as_str().is_some_and(|target_id| {
                records.iter().any(|candidate| {
                    candidate["record"] == serde_json::json!("declaration")
                        && candidate["id"] == serde_json::json!(target_id)
                        && candidate["name"] == serde_json::json!("local_value")
                })
            })
            && record["canonical_identity"]["declaration_name"] == serde_json::json!("local_value")
            && record["provenance"] == serde_json::json!("checked")
    }));
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("reference")
            && record["kind"] == serde_json::json!("field")
            && record["name"] == serde_json::json!("value")
            && record["target_id"] == serde_json::Value::Null
            && record["canonical_identity"]["declaration_name"] == serde_json::json!("value")
            && record["canonical_identity"]["namespace"] == serde_json::json!("member")
            && record["provenance"] == serde_json::json!("checked")
    }));
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("containment")
            && record["kind"] == serde_json::json!("declaration_contains_call")
    }));

    let directory = run_incan(
        tmp.path(),
        &[
            "inspect",
            "codegraph",
            src_dir.to_str().ok_or("src directory path was not valid UTF-8")?,
            "--format",
            "jsonl",
        ],
    )?;
    assert_success(&directory, "directory incan inspect codegraph");
    let directory_records = parse_jsonl_stdout(&directory)?;
    assert!(directory_records.iter().any(|record| {
        record["record"] == serde_json::json!("call")
            && record["callee"] == serde_json::json!("local_value")
            && record["target_id"].as_str().is_some_and(|target_id| {
                directory_records.iter().any(|candidate| {
                    candidate["record"] == serde_json::json!("declaration")
                        && candidate["id"] == serde_json::json!(target_id)
                        && candidate["name"] == serde_json::json!("local_value")
                })
            })
            && record["provenance"] == serde_json::json!("checked")
    }));

    Ok(())
}

#[test]
fn inspect_codegraph_project_directory_exports_sources_outside_src() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    fs::create_dir_all(tmp.path().join("src"))?;
    fs::create_dir_all(tmp.path().join("tests"))?;
    fs::write(
        tmp.path().join("loaf.toml"),
        "[project]\nname = \"whole_project_graph\"\n\n[sdk]\nprofile = \"minimal\"\n",
    )?;
    fs::write(
        tmp.path().join("src/main.incn"),
        "pub def answer() -> int:\n    return 42\n",
    )?;
    fs::write(
        tmp.path().join("tests/test_main.incn"),
        "def outside_source_root() -> int:\n    return 42\n",
    )?;

    let output = run_incan(
        tmp.path(),
        &[
            "inspect",
            "codegraph",
            tmp.path().to_str().ok_or("project path was not valid UTF-8")?,
            "--format",
            "jsonl",
        ],
    )?;
    assert_success(&output, "project-directory codegraph export with tests outside src");
    let records = parse_jsonl_stdout(&output)?;
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("file")
            && record["path"]
                .as_str()
                .is_some_and(|path| path.ends_with("tests/test_main.incn"))
    }));
    Ok(())
}

#[test]
fn inspect_codegraph_distinguishes_sibling_binding_stable_identities_issue1629()
-> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let src_dir = tmp.path().join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(
        tmp.path().join("loaf.toml"),
        r#"[project]
name = "stable_binding_graph"
version = "0.1.0"
"#,
    )?;
    let main_path = src_dir.join("main.incn");
    fs::write(
        &main_path,
        r#"def left(value: int) -> int:
    return value

def right(value: int) -> int:
    return value

def sibling_blocks() -> int:
    mut total = 0
    for value in [1]:
        total += value
    for value in [2]:
        total += value
    return total
"#,
    )?;

    let output = run_incan(
        tmp.path(),
        &[
            "inspect",
            "codegraph",
            main_path.to_str().ok_or("main path was not valid UTF-8")?,
            "--format",
            "jsonl",
        ],
    )?;
    assert_success(&output, "stable binding identity codegraph export");
    let records = parse_jsonl_stdout(&output)?;

    let left = value_references_owned_by(&records, "left");
    let right = value_references_owned_by(&records, "right");
    assert_eq!(
        left.len(),
        1,
        "left parameter reference was not projected once: {left:#?}"
    );
    assert_eq!(
        right.len(),
        1,
        "right parameter reference was not projected once: {right:#?}"
    );
    assert_ne!(
        left[0]["canonical_identity"], right[0]["canonical_identity"],
        "same-named sibling parameters must retain distinct checked identities"
    );

    let sibling_blocks = value_references_owned_by(&records, "sibling_blocks");
    assert_eq!(
        sibling_blocks.len(),
        2,
        "the two sibling loop bindings must each have one projected reference: {sibling_blocks:#?}"
    );
    assert_ne!(
        sibling_blocks[0]["canonical_identity"], sibling_blocks[1]["canonical_identity"],
        "same-named bindings in sibling lexical blocks must retain distinct checked identities"
    );
    for reference in [left[0], right[0], sibling_blocks[0], sibling_blocks[1]] {
        assert!(
            reference["stable_identity"].is_object(),
            "checked binding reference was missing its edit-stable identity: {reference}"
        );
    }

    let mut collisions = Vec::new();
    if left[0]["stable_identity"] == right[0]["stable_identity"] {
        collisions.push("same-named parameters owned by sibling functions");
    }
    if sibling_blocks[0]["stable_identity"] == sibling_blocks[1]["stable_identity"] {
        collisions.push("same-named bindings in sibling lexical blocks of one function");
    }
    assert!(
        collisions.is_empty(),
        "distinct checked bindings collapsed to the same edit-stable identity: {}",
        collisions.join(", ")
    );

    let before = [
        left[0]["stable_identity"].clone(),
        right[0]["stable_identity"].clone(),
        sibling_blocks[0]["stable_identity"].clone(),
        sibling_blocks[1]["stable_identity"].clone(),
    ];
    fs::write(
        &main_path,
        r#"# Unrelated root movement and comments must not rekey checked bindings.
def right(value: int) -> int:
    return value

def sibling_blocks() -> int:
    mut total = 0
    mut unrelated = 3
    for value in [1]:
        total += value
    for value in [2]:
        total += value
    return total + unrelated

def left(value: int) -> int:
    return value
"#,
    )?;
    let moved_output = run_incan(
        tmp.path(),
        &[
            "inspect",
            "codegraph",
            main_path.to_str().ok_or("main path was not valid UTF-8")?,
            "--format",
            "jsonl",
        ],
    )?;
    assert_success(&moved_output, "moved stable binding identity codegraph export");
    let moved_records = parse_jsonl_stdout(&moved_output)?;
    let moved_left = value_references_owned_by(&moved_records, "left");
    let moved_right = value_references_owned_by(&moved_records, "right");
    let moved_blocks = value_references_owned_by(&moved_records, "sibling_blocks");
    assert_eq!(moved_left.len(), 1);
    assert_eq!(moved_right.len(), 1);
    assert_eq!(moved_blocks.len(), 2);
    let after = [
        moved_left[0]["stable_identity"].clone(),
        moved_right[0]["stable_identity"].clone(),
        moved_blocks[0]["stable_identity"].clone(),
        moved_blocks[1]["stable_identity"].clone(),
    ];
    assert_eq!(
        before, after,
        "comments, root reordering, and an unrelated differently-named local must preserve stable binding identities"
    );

    Ok(())
}

#[test]
fn inspect_codegraph_keeps_one_identity_through_alias_reexport_and_without_a_local_record()
-> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let src_dir = tmp.path().join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(
        tmp.path().join("loaf.toml"),
        r#"[project]
name = "identity_graph"
version = "0.1.0"
"#,
    )?;
    fs::write(
        src_dir.join("provider.incn"),
        r#"pub def helper() -> int:
    return 7

pub run = alias helper
"#,
    )?;
    fs::write(
        src_dir.join("facade.incn"),
        r#"pub from provider import run as h
"#,
    )?;
    let main_path = src_dir.join("main.incn");
    fs::write(
        &main_path,
        r#"from facade import h as run_helper

def entrypoint() -> int:
    print("identity")
    return (run_helper)()
"#,
    )?;

    let output = run_incan(
        tmp.path(),
        &[
            "inspect",
            "codegraph",
            src_dir.to_str().ok_or("source path was not valid UTF-8")?,
            "--format",
            "jsonl",
        ],
    )?;
    assert_success(&output, "identity-backed incan inspect codegraph");
    let records = parse_jsonl_stdout(&output)?;

    let provider = records
        .iter()
        .find(|record| {
            record["record"] == serde_json::json!("declaration") && record["name"] == serde_json::json!("helper")
        })
        .ok_or("provider declaration was absent")?;
    let provider_identity = &provider["canonical_identity"];
    let provider_stable_identity = &provider["stable_identity"];
    assert_eq!(provider_identity["declaration_name"], serde_json::json!("helper"));
    assert_eq!(provider["provenance"], serde_json::json!("checked"));

    let declaration_alias = records
        .iter()
        .find(|record| {
            record["record"] == serde_json::json!("declaration") && record["name"] == serde_json::json!("run")
        })
        .ok_or("provider declaration alias was absent")?;
    assert_eq!(&declaration_alias["canonical_identity"], provider_identity);
    assert_eq!(&declaration_alias["stable_identity"], provider_stable_identity);
    assert_ne!(declaration_alias["id"], provider["id"]);

    let reexport = records
        .iter()
        .find(|record| record["record"] == serde_json::json!("export") && record["name"] == serde_json::json!("h"))
        .ok_or("facade re-export record was absent")?;
    assert_eq!(&reexport["canonical_identity"], provider_identity);
    assert_eq!(&reexport["stable_identity"], provider_stable_identity);
    assert_eq!(reexport["provenance"], serde_json::json!("checked"));

    let aliased_import = records
        .iter()
        .find(|record| {
            record["record"] == serde_json::json!("import")
                && record["bindings"].as_array().is_some_and(|bindings| {
                    bindings
                        .iter()
                        .any(|binding| binding["local_name"] == serde_json::json!("run_helper"))
                })
        })
        .ok_or("consumer alias import record was absent")?;
    let aliased_binding = aliased_import["bindings"]
        .as_array()
        .and_then(|bindings| {
            bindings
                .iter()
                .find(|binding| binding["local_name"] == serde_json::json!("run_helper"))
        })
        .ok_or("consumer alias binding was absent")?;
    assert_eq!(&aliased_binding["canonical_identity"], provider_identity);
    assert_eq!(&aliased_binding["stable_identity"], provider_stable_identity);
    assert_eq!(aliased_import["provenance"], serde_json::json!("checked"));

    for record_kind in ["reference", "call"] {
        let aliased = records
            .iter()
            .find(|record| {
                record["record"] == serde_json::json!(record_kind)
                    && (record["name"] == serde_json::json!("run_helper")
                        || record["callee"] == serde_json::json!("run_helper"))
            })
            .ok_or_else(|| format!("aliased {record_kind} record was absent"))?;
        assert_eq!(
            &aliased["canonical_identity"], provider_identity,
            "every spelling must retain the original provider identity"
        );
        assert_eq!(
            &aliased["stable_identity"], provider_stable_identity,
            "every spelling must retain the original provider stable identity"
        );
        assert_eq!(
            aliased["target_id"], provider["id"],
            "graph-local linkage must select the canonical declaration rather than its alias binding"
        );
        assert_eq!(aliased["provenance"], serde_json::json!("checked"));
    }

    let builtin = records
        .iter()
        .find(|record| record["record"] == serde_json::json!("call") && record["callee"] == serde_json::json!("print"))
        .ok_or("builtin call record was absent")?;
    assert_eq!(builtin["target_id"], serde_json::Value::Null);
    assert_eq!(
        builtin["canonical_identity"]["origin"]["kind"],
        serde_json::json!("builtin")
    );
    assert_eq!(
        builtin["canonical_identity"]["declaration_name"],
        serde_json::json!("print")
    );
    assert_eq!(
        builtin["provenance"],
        serde_json::json!("checked"),
        "a missing graph-local declaration must not erase compiler-proven identity"
    );

    Ok(())
}

#[test]
fn inspect_codegraph_exports_checked_registry_facts() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let src_dir = tmp.path().join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(
        tmp.path().join("loaf.toml"),
        r#"[project]
name = "registry_graph"
version = "0.1.0"
"#,
    )?;
    let main_path = src_dir.join("main.incn");
    fs::write(
        &main_path,
        r#"from std.registry import Registry, SubjectKind, describe

@derive(Clone, Eq)
type FunctionId = newtype str

@derive(Descriptor)
model FunctionSpec:
    summary: str

pub static functions: Registry[FunctionId, FunctionSpec] = Registry.define(
    subjects=[SubjectKind.Function],
)

@describe(functions, FunctionId("normalize"), FunctionSpec(summary="Normalize text"))
pub def normalize(value: str) -> str:
    return value
"#,
    )?;

    let output = run_incan(
        tmp.path(),
        &[
            "inspect",
            "codegraph",
            main_path.to_str().ok_or("main path was not valid UTF-8")?,
            "--format",
            "jsonl",
        ],
    )?;
    assert_success(&output, "registry codegraph export");
    let records = parse_jsonl_stdout(&output)?;
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("registry")
            && record["registry_identity"] == serde_json::json!("main::functions")
            && record["registry_public"] == serde_json::json!(true)
            && record["subject_kind"] == serde_json::json!("function")
            && record["subject_identity"] == serde_json::json!("main.normalize")
            && record["key"]["kind"] == serde_json::json!("newtype")
            && record["descriptor"]["kind"] == serde_json::json!("model")
            && record["registration_span"].is_object()
            && record["subject_span"].is_object()
            && record["provenance"] == serde_json::json!("checked")
    }));
    Ok(())
}

#[test]
fn inspect_codegraph_attaches_facade_paths_to_checked_registry_facts() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let src_dir = tmp.path().join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(
        tmp.path().join("loaf.toml"),
        "[project]\nname = \"registry_graph_facade\"\nversion = \"0.1.0\"\n",
    )?;
    fs::write(
        src_dir.join("feature.incn"),
        r#"from std.registry import Registry, SubjectKind, describe

@derive(Clone, Eq)
pub type FunctionId = newtype str

@derive(Descriptor)
pub model FunctionSpec:
    pub summary: str

pub static functions: Registry[FunctionId, FunctionSpec] = Registry.define(
    subjects=[SubjectKind.Function],
)

@describe(functions, FunctionId("normalize"), FunctionSpec(summary="Normalize text"))
pub def normalize(value: str) -> str:
    return value
"#,
    )?;
    fs::write(
        src_dir.join("main.incn"),
        r#"pub from crate.feature import functions as public_functions
pub from crate.feature import normalize as public_normalize
"#,
    )?;

    let output = run_incan(
        tmp.path(),
        &[
            "inspect",
            "codegraph",
            src_dir
                .join("main.incn")
                .to_str()
                .ok_or("main path was not valid UTF-8")?,
            "--format",
            "jsonl",
        ],
    )?;
    assert_success(&output, "registry facade codegraph export");
    let records = parse_jsonl_stdout(&output)?;
    let registry = records
        .iter()
        .find(|record| {
            record["record"] == serde_json::json!("registry")
                && record["registry_identity"] == serde_json::json!("feature::functions")
        })
        .ok_or("missing checked feature registry record")?;
    assert_eq!(registry["subject_identity"], serde_json::json!("feature.normalize"));
    let reexport_paths = registry["reexport_paths"]
        .as_array()
        .ok_or("checked registry record must expose facade projections")?;
    assert_eq!(
        reexport_paths
            .iter()
            .map(|projection| projection["path"].clone())
            .collect::<Vec<_>>(),
        vec![
            serde_json::json!(["main", "public_functions"]),
            serde_json::json!(["main", "public_normalize"]),
        ]
    );
    assert!(
        reexport_paths.iter().all(|path| path["span"].is_object()),
        "facade projections must retain their public-import anchors: {registry}"
    );
    Ok(())
}

#[test]
fn inspect_codegraph_projects_the_selected_incan_package_features() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(
        tmp.path(),
        "feature_graph_demo",
        r#"

[project.features]
alpha = []
beta = []
"#,
    )?;
    fs::write(
        &main_path,
        r#"when feature("alpha"):
    pub def alpha_entrypoint() -> str:
        return "alpha"

when feature("beta"):
    pub def beta_entrypoint() -> str:
        return "beta"
"#,
    )?;
    let main_arg = main_path.to_str().ok_or("main path was not valid UTF-8")?;

    for (selected, expected, excluded) in [
        ("alpha", "alpha_entrypoint", "beta_entrypoint"),
        ("beta", "beta_entrypoint", "alpha_entrypoint"),
    ] {
        let output = run_incan(
            tmp.path(),
            &[
                "inspect",
                "codegraph",
                main_arg,
                "--format",
                "jsonl",
                "--no-default-features",
                "--features",
                selected,
            ],
        )?;
        assert_success(&output, &format!("codegraph projection for package feature {selected}"));
        let records = parse_jsonl_stdout(&output)?;
        let header = records.first().ok_or("codegraph did not emit a header")?;
        let semantic_context = header["semantic_contexts"]
            .as_array()
            .and_then(|contexts| contexts.first())
            .ok_or("codegraph header did not project semantic context")?;
        let package = semantic_context["packages"]
            .as_array()
            .and_then(|packages| packages.first())
            .ok_or("codegraph semantic context did not project package features")?;
        assert_eq!(package["active_features"], serde_json::json!([selected]));
        assert!(semantic_context["providers"].as_array().is_some_and(|providers| {
            providers.iter().any(|provider| {
                provider["provenance"]["kind"] == serde_json::json!("sdk")
                    && provider["enabled"] == serde_json::json!(true)
                    && provider["manifest_path"].as_str().is_some()
            })
        }));
        assert!(records.iter().any(|record| {
            record["record"] == serde_json::json!("declaration") && record["name"] == serde_json::json!(expected)
        }));
        assert!(
            records.iter().all(|record| {
                record["record"] != serde_json::json!("declaration") || record["name"] != serde_json::json!(excluded)
            }),
            "codegraph for `{selected}` retained inactive declaration `{excluded}`"
        );
    }

    Ok(())
}

#[test]
fn transient_sdk_profile_is_shared_by_check_and_provider_inspection() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "sdk_profile_override", "")?;
    let main_arg = main_path.to_str().ok_or("main path was not valid UTF-8")?;

    let minimal_core = run_incan(tmp.path(), &["check", main_arg, "--sdk-profile", "minimal"])?;
    assert_success(
        &minimal_core,
        "minimal SDK profile check using only core language surface",
    );
    let minimal_codegraph = run_incan(
        tmp.path(),
        &[
            "inspect",
            "codegraph",
            main_arg,
            "--format",
            "jsonl",
            "--sdk-profile",
            "minimal",
        ],
    )?;
    assert_success(
        &minimal_codegraph,
        "minimal SDK profile codegraph using only core language surface",
    );

    fs::write(
        &main_path,
        r#"from std.fs.path import Path

def main() -> None:
    _ = Path("profile")
"#,
    )?;

    let minimal = run_incan(tmp.path(), &["check", main_arg, "--sdk-profile", "minimal"])?;
    assert_failure(&minimal, "minimal SDK profile check using std.fs");
    let minimal_stderr = String::from_utf8_lossy(&minimal.stderr);
    assert!(
        minimal_stderr.contains("stdlib-system") && minimal_stderr.contains("disabled"),
        "minimal profile should diagnose the disabled std.fs component:\n{minimal_stderr}"
    );
    let minimal_json = run_incan(
        tmp.path(),
        &["check", main_arg, "--format", "json", "--sdk-profile", "minimal"],
    )?;
    assert_failure(&minimal_json, "minimal SDK profile JSON diagnostic using std.fs");
    let minimal_json = parse_json_stdout(&minimal_json)?;
    assert_eq!(minimal_json["diagnostics"][0]["code"], serde_json::json!("INCAN-I0101"));

    let default = run_incan(tmp.path(), &["check", main_arg, "--sdk-profile", "default"])?;
    assert_success(&default, "default SDK profile check using std.fs");

    for command in ["build", "run"] {
        let output = run_incan(tmp.path(), &[command, main_arg, "--sdk-profile", "minimal"])?;
        assert_failure(&output, &format!("{command} using disabled std.fs component"));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("stdlib-system") && stderr.contains("disabled"),
            "{command} should use the transient provider projection:\n{stderr}"
        );
    }

    let inspection = run_incan(
        tmp.path(),
        &[
            "inspect",
            "providers",
            main_arg,
            "--format",
            "json",
            "--sdk-profile",
            "minimal",
        ],
    )?;
    assert_success(&inspection, "provider inspection with transient minimal SDK profile");
    let report = parse_json_stdout(&inspection)?;
    assert_eq!(report["sdk"]["profile"], serde_json::json!("minimal"));
    let components = report["sdk"]["components"]
        .as_array()
        .ok_or("provider report did not contain SDK components")?;
    let system = components
        .iter()
        .find(|component| component["id"] == serde_json::json!("stdlib-system"))
        .ok_or("provider report did not contain stdlib-system")?;
    assert_eq!(system["available"], serde_json::json!(true));
    assert_eq!(system["enabled"], serde_json::json!(false));
    let providers = report["providers"]
        .as_array()
        .ok_or("provider report did not contain providers")?;
    let system_provider = providers
        .iter()
        .find(|provider| provider["provenance"]["component_id"] == serde_json::json!("stdlib-system"))
        .ok_or("provider report did not contain the stdlib-system provider")?;
    assert_eq!(system_provider["available"], serde_json::json!(true));
    assert_eq!(system_provider["enabled"], serde_json::json!(false));
    assert_eq!(system_provider["used"], serde_json::json!(false));
    assert!(system_provider["provider_dependencies"].is_array());

    Ok(())
}

#[test]
fn lock_and_feature_inspection_record_transient_semantic_selections() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(
        tmp.path(),
        "semantic_selection_lock",
        r#"

[project.features]
default = ["alpha"]
alpha = []
beta = []
"#,
    )?;
    let main_arg = main_path.to_str().ok_or("main path was not valid UTF-8")?;
    let selection_args = [
        "--no-default-features",
        "--features",
        "beta",
        "--sdk-profile",
        "minimal",
    ];

    let inspection = run_incan(
        tmp.path(),
        &[
            "inspect",
            "features",
            main_arg,
            "--format",
            "json",
            selection_args[0],
            selection_args[1],
            selection_args[2],
            selection_args[3],
            selection_args[4],
        ],
    )?;
    assert_success(&inspection, "feature inspection with transient semantic selections");
    let report = parse_json_stdout(&inspection)?;
    let package = report["packages"]
        .as_array()
        .and_then(|packages| packages.first())
        .ok_or("feature report did not contain the root package")?;
    assert_eq!(package["active_features"], serde_json::json!(["beta"]));
    assert_eq!(package["reasons"]["beta"][0]["kind"], serde_json::json!("requested"));

    let lock = run_incan(
        tmp.path(),
        &[
            "lock",
            main_arg,
            selection_args[0],
            selection_args[1],
            selection_args[2],
            selection_args[3],
            selection_args[4],
        ],
    )?;
    assert_success(&lock, "semantic lock generation with transient selections");
    let lock: toml::Value = toml::from_str(&fs::read_to_string(tmp.path().join("oven.lock"))?)?;
    assert_eq!(lock["semantic"]["sdk"]["profile"].as_str(), Some("minimal"));
    let locked_package = lock["semantic"]["packages"]
        .as_array()
        .and_then(|packages| packages.first())
        .ok_or("semantic lock did not contain the root package")?;
    assert_eq!(
        locked_package["active_features"].as_array(),
        Some(&vec![toml::Value::String("beta".to_string())])
    );

    Ok(())
}

#[test]
fn locked_build_rejects_package_feature_or_sdk_projection_drift() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(
        tmp.path(),
        "semantic_lock_drift",
        r#"

[project.features]
alpha = []
beta = []
"#,
    )?;
    let main_arg = main_path.to_str().ok_or("main path was not valid UTF-8")?;

    let lock = run_incan(
        tmp.path(),
        &[
            "lock",
            main_arg,
            "--no-default-features",
            "--features",
            "alpha",
            "--sdk-profile",
            "minimal",
        ],
    )?;
    assert_success(&lock, "lock semantic alpha/minimal projection");

    for (feature, profile) in [("beta", "minimal"), ("alpha", "default")] {
        let build = run_incan(
            tmp.path(),
            &[
                "build",
                main_arg,
                "--locked",
                "--no-default-features",
                "--features",
                feature,
                "--sdk-profile",
                profile,
            ],
        )?;
        assert_failure(
            &build,
            &format!("locked build with drifted {feature}/{profile} projection"),
        );
        let stderr = String::from_utf8_lossy(&build.stderr);
        assert!(
            stderr.contains("oven.lock") && stderr.contains("out of date") && stderr.contains("Run `incan lock`"),
            "locked projection drift should fail as stale lock state:\n{stderr}"
        );
    }

    Ok(())
}

#[test]
fn codegraph_importer_example_consumes_compiler_jsonl_issue776() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let source_dir = tmp.path().join("source");
    fs::create_dir_all(&source_dir)?;
    fs::write(
        source_dir.join("loaf.toml"),
        r#"[project]
name = "codegraph_importer_source"
version = "0.1.0"
"#,
    )?;
    let source_main = source_dir.join("main.incn");
    fs::write(
        &source_main,
        r#"from std.registry import Registry, SubjectKind, describe

@derive(Clone, Eq)
type FunctionId = newtype str

@derive(Descriptor)
model FunctionSpec:
    summary: str

pub static functions: Registry[FunctionId, FunctionSpec] = Registry.define(
    subjects=[SubjectKind.Function],
)

@describe(functions, FunctionId("greet"), FunctionSpec(summary="Greet a named user"))
pub def greet(name: str) -> str:
    return f"hello {name}"

def main() -> None:
    println(greet("Incan"))
"#,
    )?;

    let graph = run_incan(
        &source_dir,
        &[
            "inspect",
            "codegraph",
            source_main.to_str().ok_or("source path was not valid UTF-8")?,
            "--format",
            "jsonl",
        ],
    )?;
    assert_success(&graph, "compiler codegraph export for importer example");
    let graph_records = parse_jsonl_stdout(&graph)?;
    assert_codegraph_record_contract(&graph_records);

    let importer_dir = tmp.path().join("importer");
    let importer_src = importer_dir.join("src");
    fs::create_dir_all(&importer_src)?;
    fs::write(
        importer_dir.join("loaf.toml"),
        fs::read_to_string(support::repo_root().join("examples/pro/codegraph_importer/loaf.toml"))?,
    )?;
    fs::write(
        importer_src.join("importer.incn"),
        fs::read_to_string(support::repo_root().join("examples/pro/codegraph_importer/src/importer.incn"))?,
    )?;
    fs::write(
        importer_src.join("main.incn"),
        fs::read_to_string(support::repo_root().join("examples/pro/codegraph_importer/src/main.incn"))?,
    )?;
    fs::write(importer_dir.join("codegraph.jsonl"), &graph.stdout)?;

    let first = run_incan(&importer_dir, &["run", "src/main.incn"])?;
    assert_success(&first, "Incan-authored codegraph importer example");
    let second = run_incan(&importer_dir, &["run", "src/main.incn"])?;
    assert_success(&second, "second Incan-authored codegraph importer example");
    assert_eq!(first.stdout, second.stdout, "importer summary must be deterministic");

    let summary = parse_json_stdout(&first)?;
    assert_eq!(summary["schema_version"], serde_json::json!(8));
    assert_eq!(summary["mode"], serde_json::json!("strict"));
    assert_eq!(summary["metadata_record_count"], serde_json::json!(1));
    assert!(
        summary["fact_count"].as_i64().is_some_and(|count| count > 0),
        "importer must observe compiler-owned graph facts: {summary}"
    );
    assert!(
        summary["declaration_count"].as_i64().is_some_and(|count| count > 0),
        "importer must preserve declaration records without parsing source itself: {summary}"
    );
    assert!(
        summary["registry_count"].as_i64().is_some_and(|count| count > 0),
        "importer must preserve compiler-checked typed registry facts: {summary}"
    );

    fs::write(
        importer_dir.join("codegraph.jsonl"),
        concat!(
            r#"{"record":"header","schema_version":1,"mode":"strict","degraded":false}"#,
            "\n",
            r#"{"record":"file","degraded":false}"#,
            "\n",
        ),
    )?;
    let legacy = run_incan(&importer_dir, &["run", "src/main.incn"])?;
    assert_success(&legacy, "schema-v1 codegraph importer compatibility");
    let legacy_summary = parse_json_stdout(&legacy)?;
    assert_eq!(legacy_summary["schema_version"], serde_json::json!(1));
    assert_eq!(legacy_summary["file_count"], serde_json::json!(1));

    Ok(())
}

#[test]
fn inspect_codegraph_tolerant_directory_keeps_parseable_facts_and_diagnostics() -> Result<(), Box<dyn std::error::Error>>
{
    let tmp = tempfile::tempdir()?;
    fs::write(
        tmp.path().join("ok.incn"),
        r#"pub def ok() -> int:
    return 1
"#,
    )?;
    let nested = tmp.path().join("nested");
    fs::create_dir_all(&nested)?;
    fs::write(
        nested.join("extra.incn"),
        r#"pub def extra() -> int:
    return 2
"#,
    )?;
    fs::write(tmp.path().join("broken.incn"), "def broken(:\n")?;

    let strict = run_incan(
        tmp.path(),
        &[
            "inspect",
            "codegraph",
            tmp.path().to_str().ok_or("directory path was not valid UTF-8")?,
            "--format",
            "jsonl",
        ],
    )?;
    assert_failure(&strict, "strict incan inspect codegraph should reject broken source");

    let tolerant = run_incan(
        tmp.path(),
        &[
            "inspect",
            "codegraph",
            tmp.path().to_str().ok_or("directory path was not valid UTF-8")?,
            "--format",
            "jsonl",
            "--allow-errors",
        ],
    )?;
    assert_success(&tolerant, "tolerant incan inspect codegraph");
    let records = parse_jsonl_stdout(&tolerant)?;
    assert_codegraph_record_contract(&records);
    assert_eq!(records[0]["degraded"], serde_json::json!(true));
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("declaration")
            && record["name"] == serde_json::json!("ok")
            && record["provenance"] == serde_json::json!("syntax")
    }));
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("module")
            && record["module_path"] == serde_json::json!(["nested", "extra"])
    }));
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("diagnostic")
            && record["code"] == serde_json::json!("INCAN-P0001")
            && record["phase"] == serde_json::json!("parse")
    }));

    Ok(())
}

#[test]
fn inspect_codegraph_strict_directory_rejects_semantic_diagnostics() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    fs::write(
        tmp.path().join("bad.incn"),
        r#"pub def bad() -> int:
    return missing()
"#,
    )?;

    let strict = run_incan(
        tmp.path(),
        &[
            "inspect",
            "codegraph",
            tmp.path().to_str().ok_or("directory path was not valid UTF-8")?,
            "--format",
            "jsonl",
        ],
    )?;
    assert_failure(
        &strict,
        "strict incan inspect codegraph should reject directory typecheck diagnostics",
    );
    let strict_stderr = String::from_utf8_lossy(&strict.stderr);
    assert!(
        strict_stderr.contains("Unknown symbol 'missing'"),
        "expected strict directory codegraph to report typecheck diagnostic, got:\n{strict_stderr}"
    );

    let tolerant = run_incan(
        tmp.path(),
        &[
            "inspect",
            "codegraph",
            tmp.path().to_str().ok_or("directory path was not valid UTF-8")?,
            "--format",
            "jsonl",
            "--allow-errors",
        ],
    )?;
    assert_success(
        &tolerant,
        "tolerant incan inspect codegraph should keep syntax facts for directory typecheck diagnostics",
    );
    let records = parse_jsonl_stdout(&tolerant)?;
    assert_codegraph_record_contract(&records);
    assert_eq!(records[0]["degraded"], serde_json::json!(true));
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("declaration")
            && record["name"] == serde_json::json!("bad")
            && record["provenance"] == serde_json::json!("syntax")
            && record["canonical_identity"] == serde_json::Value::Null
            && record["degraded"] == serde_json::json!(true)
    }));
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("call")
            && record["callee"] == serde_json::json!("missing")
            && record["target_id"] == serde_json::Value::Null
            && record["canonical_identity"] == serde_json::Value::Null
            && record["provenance"] == serde_json::json!("syntax")
            && record["degraded"] == serde_json::json!(true)
    }));
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("diagnostic")
            && record["code"] == serde_json::json!("INCAN-T0001")
            && record["phase"] == serde_json::json!("typecheck")
            && record["message"] == serde_json::json!("Unknown symbol 'missing'")
    }));

    Ok(())
}
