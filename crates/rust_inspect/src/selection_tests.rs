use std::fs;

use crate::selection::{InspectionSourceInput, SelectedInspectionSysrootInput, ValidatedInspectionProject};
use crate::selection_test_support::{InspectionFixture, byte_digest};
use crate::{RustMetadataError, RustWorkspace, extract_rust_item};

/// Attach a small compiler-selected core/std graph while retaining its physical source lease.
fn attach_json_sysroot(fixture: &mut InspectionFixture) -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
    let sysroot = tempfile::tempdir()?;
    let root = sysroot.path().canonicalize()?;
    let core_dir = root.join("core");
    let std_dir = root.join("std");
    fs::create_dir(&core_dir)?;
    fs::create_dir(&std_dir)?;
    let core_module = core_dir.join("lib.rs");
    let std_module = std_dir.join("lib.rs");
    fs::write(
        &core_module,
        "#![no_core]\npub mod prelude { pub mod rust_2021 { pub struct CorePreludeMarker; } }\n",
    )?;
    fs::write(
        &std_module,
        "#![no_std]\npub mod prelude { pub mod rust_2021 { pub struct SelectedPreludeMarker; } }\n",
    )?;
    fs::write(root.join("Cargo.toml"), "invalid sysroot cargo manifest: do not read\n")?;
    let project_json = serde_json::to_vec(&serde_json::json!({ "crates": [
        {
            "display_name": "core", "root_module": core_module, "edition": "2021", "deps": [],
            "cfg": [], "env": {}, "is_workspace_member": false,
            "source": { "include_dirs": [core_dir], "exclude_dirs": [] }
        },
        {
            "display_name": "std", "root_module": std_module, "edition": "2021",
            "deps": [{"crate": 0, "name": "core"}],
            "cfg": [], "env": {}, "is_workspace_member": false,
            "source": { "include_dirs": [std_dir], "exclude_dirs": [] }
        }
    ] }))?;
    fixture.inputs.sources.push(InspectionSourceInput {
        root: root.clone(),
        digest: super::digest_oven_source_tree(&root)?,
    });
    fixture.inputs.sysroot = Some(SelectedInspectionSysrootInput {
        project_digest: byte_digest(&project_json),
        project_json,
        source_root: root,
    });
    Ok(sysroot)
}

/// Verify path only load refuses without reading or creating inputs.
#[test]
fn path_only_load_refuses_without_reading_or_creating_inputs() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let missing = directory.path().join("absent");
    for result in [
        RustWorkspace::load(&missing, &|_| {}),
        RustWorkspace::load_with_options(&missing, &|_| {}, true),
    ] {
        assert!(matches!(
            result,
            Err(RustMetadataError::SelectedInputUnavailable { .. })
        ));
    }
    assert!(!missing.exists());
    Ok(())
}

/// Verify selected neutral loader extracts metadata and cleans temporary projection.
#[test]
fn selected_neutral_loader_extracts_metadata_and_cleans_temporary_projection() -> Result<(), Box<dyn std::error::Error>>
{
    let fixture = InspectionFixture::new("#![no_std]\npub struct Thing { pub number: u64 }\n")?;
    let workspace = fixture.load()?;
    let metadata = extract_rust_item(&workspace, "demo::Thing")?;
    let incan_core::interop::RustItemKind::Type(ty) = metadata.kind else {
        return Err("selected struct was not extracted".into());
    };
    assert_eq!(ty.fields.len(), 1);
    assert_eq!(ty.fields[0].name, "number");
    assert_eq!(fs::read_dir(fixture.output.path())?.count(), 0);
    assert_eq!(
        fs::read_to_string(fixture.source.path().join("Cargo.toml"))?,
        "invalid cargo manifest: do not read\n"
    );
    Ok(())
}

/// Verify missing corrupt and unsupported projection inputs fail before output.
#[test]
fn missing_corrupt_and_unsupported_projection_inputs_fail_before_output() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = InspectionFixture::new("#![no_std]\npub struct Thing;\n")?;
    let mut wrong_digest = fixture.inputs.clone();
    wrong_digest.project_digest = byte_digest(b"different projection");
    assert!(matches!(
        ValidatedInspectionProject::validate(wrong_digest),
        Err(RustMetadataError::InvalidSelectedInput { .. })
    ));
    let mut no_roots = fixture.inputs.clone();
    no_roots.sources.clear();
    assert!(matches!(
        ValidatedInspectionProject::validate(no_roots),
        Err(RustMetadataError::SelectedInputUnavailable { .. })
    ));
    let mut wrong_target = fixture.inputs.clone();
    wrong_target.target_spec_digest = byte_digest(b"different target");
    assert!(matches!(
        ValidatedInspectionProject::validate(wrong_target),
        Err(RustMetadataError::InvalidSelectedInput { .. })
    ));
    let mut missing_query = fixture.inputs.clone();
    missing_query
        .query_roots
        .insert("foreign".to_string(), fixture.output.path().join("lib.rs"));
    assert!(matches!(
        ValidatedInspectionProject::validate(missing_query),
        Err(RustMetadataError::InvalidSelectedInput { .. })
    ));
    assert_eq!(fs::read_dir(fixture.output.path())?.count(), 0);
    Ok(())
}

/// Verify projection does not infer missing units or choose ambiguous units.
#[test]
fn projection_does_not_infer_missing_units_or_choose_ambiguous_units() -> Result<(), Box<dyn std::error::Error>> {
    let mut fixture = InspectionFixture::new("#![no_std]\npub struct Thing;\n")?;
    let original: serde_json::Value = serde_json::from_slice(&fixture.inputs.project_json)?;
    let mut missing = original.clone();
    missing["crates"][0]["deps"] = serde_json::json!([{"crate": 4, "name": "absent"}]);
    fixture.set_project(missing)?;
    assert!(matches!(
        fixture.load(),
        Err(RustMetadataError::InvalidSelectedInput { .. })
    ));
    let mut cyclic = original.clone();
    cyclic["crates"][0]["deps"] = serde_json::json!([{"crate": 0, "name": "self_cycle"}]);
    fixture.set_project(cyclic)?;
    assert!(matches!(
        fixture.load(),
        Err(RustMetadataError::InvalidSelectedInput { .. })
    ));
    let mut ambient_target = original.clone();
    ambient_target["crates"][0]["target"] = serde_json::json!("aarch64-apple-darwin");
    fixture.set_project(ambient_target)?;
    assert!(matches!(
        fixture.load(),
        Err(RustMetadataError::UnsupportedSelectedOperation { .. })
    ));
    let mut duplicate = original.clone();
    duplicate["crates"]
        .as_array_mut()
        .ok_or("crates absent")?
        .push(original["crates"][0].clone());
    fixture.set_project(duplicate)?;
    assert!(matches!(
        fixture.load(),
        Err(RustMetadataError::UnsupportedSelectedOperation { .. })
    ));
    let mut macro_unit = original.clone();
    macro_unit["crates"][0]["is_proc_macro"] = serde_json::json!(true);
    fixture.set_project(macro_unit)?;
    assert!(matches!(
        fixture.load(),
        Err(RustMetadataError::UnsupportedSelectedOperation { .. })
    ));
    let mut implicit_sysroot = original;
    implicit_sysroot["sysroot_src"] = serde_json::json!(fixture.source.path().canonicalize()?);
    fixture.set_project(implicit_sysroot)?;
    assert!(matches!(
        fixture.load(),
        Err(RustMetadataError::InvalidSelectedInput { .. })
    ));
    assert_eq!(fs::read_dir(fixture.output.path())?.count(), 0);
    Ok(())
}

/// Verify source changes and foreign paths never fall back to adjacent files.
#[test]
fn source_changes_and_foreign_paths_never_fall_back_to_adjacent_files() -> Result<(), Box<dyn std::error::Error>> {
    let mut fixture = InspectionFixture::new("#![no_std]\npub struct Thing;\n")?;
    let validated = ValidatedInspectionProject::validate(fixture.inputs.clone())?;
    fs::write(fixture.module()?, "#![no_std]\npub struct Changed;\n")?;
    assert!(matches!(
        RustWorkspace::load_selected(&validated, fixture.output.path(), &|_| {}),
        Err(RustMetadataError::InvalidSelectedInput { .. })
    ));
    let other = InspectionFixture::new("#![no_std]\npub struct Foreign;\n")?;
    let mut project: serde_json::Value = serde_json::from_slice(&fixture.inputs.project_json)?;
    project["crates"][0]["root_module"] = serde_json::json!(other.module()?);
    fixture.set_project(project)?;
    assert!(matches!(
        fixture.load(),
        Err(RustMetadataError::InvalidSelectedInput { .. })
    ));
    assert_eq!(fs::read_dir(fixture.output.path())?.count(), 0);
    Ok(())
}

/// Verify two same named packages follow explicit query bindings.
#[test]
fn two_same_named_packages_follow_explicit_query_bindings() -> Result<(), Box<dyn std::error::Error>> {
    let mut first = InspectionFixture::new("#![no_std]\npub struct Item { pub left: u32 }\n")?;
    let second = InspectionFixture::new("#![no_std]\npub struct Item { pub right: bool }\n")?;
    let mut project: serde_json::Value = serde_json::from_slice(&first.inputs.project_json)?;
    let other: serde_json::Value = serde_json::from_slice(&second.inputs.project_json)?;
    project["crates"]
        .as_array_mut()
        .ok_or("crates absent")?
        .push(other["crates"][0].clone());
    first.set_project(project)?;
    first.inputs.sources.extend(second.inputs.sources.clone());
    first.inputs.query_roots.clear();
    first.inputs.query_roots.insert("left".to_string(), first.module()?);
    first.inputs.query_roots.insert("right".to_string(), second.module()?);
    let workspace = first.load()?;
    for (alias, expected_field) in [("left", "left"), ("right", "right")] {
        let metadata = extract_rust_item(&workspace, &format!("{alias}::Item"))?;
        let incan_core::interop::RustItemKind::Type(ty) = metadata.kind else {
            return Err("selected item was not a type".into());
        };
        assert_eq!(ty.fields.len(), 1);
        assert_eq!(ty.fields[0].name, expected_field);
    }
    assert!(workspace.crate_by_name("demo").is_none());
    Ok(())
}

/// Verify temporary root must not overlap selected inputs.
#[test]
fn temporary_root_must_not_overlap_selected_inputs() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = InspectionFixture::new("#![no_std]\npub struct Thing;\n")?;
    let validated = ValidatedInspectionProject::validate(fixture.inputs.clone())?;
    assert!(matches!(
        RustWorkspace::load_selected(&validated, fixture.source.path(), &|_| {}),
        Err(RustMetadataError::InvalidSelectedInput { .. })
    ));
    assert_eq!(fs::read_dir(fixture.source.path())?.count(), 2);
    Ok(())
}

/// Include and path attributes cannot make the analysis database read an unselected source tree.
#[test]
fn selected_source_files_contain_include_and_path_attribute_resolution() -> Result<(), Box<dyn std::error::Error>> {
    let foreign = tempfile::tempdir()?;
    let outside = foreign.path().canonicalize()?.join("foreign.rs");
    fs::write(&outside, "pub struct Foreign { pub forbidden: bool }\n")?;
    let source = format!(
        "#![no_std]\n#[rustc_builtin_macro] macro_rules! include {{ () => {{}} }}\ninclude!(\"allowed.rs\");\ninclude!({outside:?});\n#[path = {outside:?}] mod escaped;\npub use escaped::Foreign as Escaped;\npub struct Local;\n"
    );
    let mut fixture = InspectionFixture::new(&source)?;
    fs::write(fixture.source.path().join("allowed.rs"), "pub struct Included;\n")?;
    fixture.inputs.sources[0].digest = super::digest_oven_source_tree(fixture.source.path())?;
    let workspace = fixture.load()?;
    assert!(extract_rust_item(&workspace, "demo::Local").is_ok());
    assert!(
        extract_rust_item(&workspace, "demo::Included").is_ok(),
        "positive include expansion must run"
    );
    for query in ["demo::Foreign", "demo::Escaped", "demo::escaped::Foreign"] {
        assert!(matches!(
            extract_rust_item(&workspace, query),
            Err(RustMetadataError::PathNotResolved(_))
        ));
    }
    let selected_root = fixture.source.path().canonicalize()?;
    for (_, path) in workspace.vfs.iter() {
        let path = path.as_path().ok_or("unexpected virtual input")?;
        let file_path: &std::path::Path = path.as_ref();
        assert!(file_path.starts_with(&selected_root), "unselected VFS input: {path}");
    }
    assert_eq!(
        fs::read_to_string(&outside)?,
        "pub struct Foreign { pub forbidden: bool }\n"
    );
    Ok(())
}

/// Selected module files remain visible through ordinary path attributes.
#[test]
fn selected_path_attribute_keeps_explicit_source_members_visible() -> Result<(), Box<dyn std::error::Error>> {
    let mut fixture =
        InspectionFixture::new("#![no_std]\n#[path = \"generated.rs\"] mod included;\npub use included::Generated;\n")?;
    fs::write(
        fixture.source.path().join("generated.rs"),
        "pub struct Generated { pub number: u32 }\n",
    )?;
    fixture.inputs.sources[0].digest = super::digest_oven_source_tree(fixture.source.path())?;
    let workspace = fixture.load()?;
    let metadata = extract_rust_item(&workspace, "demo::Generated")?;
    let incan_core::interop::RustItemKind::Type(ty) = metadata.kind else {
        return Err("selected generated type absent".into());
    };
    assert_eq!(ty.fields.len(), 1);
    assert_eq!(ty.fields[0].name, "number");
    Ok(())
}

/// A selected tree cannot grant an unselected file by following a symbolic link during VFS traversal.
#[cfg(unix)]
#[test]
fn selected_source_symlink_refuses_before_database_loading() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = InspectionFixture::new("#![no_std]\npub struct Local;\n")?;
    let foreign = tempfile::tempdir()?;
    let outside = foreign.path().join("foreign.rs");
    fs::write(&outside, "pub struct Foreign;\n")?;
    std::os::unix::fs::symlink(&outside, fixture.source.path().join("foreign.rs"))?;
    assert!(matches!(
        fixture.load(),
        Err(RustMetadataError::InvalidSelectedInput { .. })
    ));
    assert_eq!(fs::read_dir(fixture.output.path())?.count(), 0);
    Ok(())
}

/// A JSON-backed selected sysroot supplies language crates, consumer edges and the edition prelude.
#[test]
fn selected_json_sysroot_supplies_consumer_language_semantics() -> Result<(), Box<dyn std::error::Error>> {
    use ra_ap_hir::{Crate, ModuleDef, PathResolution, Semantics};
    use ra_ap_ide_db::base_db::{CrateOrigin, LangCrateOrigin};
    use ra_ap_syntax::AstNode as _;

    let mut fixture = InspectionFixture::new("pub struct Consumer { pub selected: SelectedPreludeMarker }\n")?;
    let sysroot = attach_json_sysroot(&mut fixture)?;
    let consumer_project: serde_json::Value = serde_json::from_slice(&fixture.inputs.project_json)?;
    assert_eq!(consumer_project["crates"][0]["deps"], serde_json::json!([]));

    let workspace = fixture.load()?;
    let db = workspace.db();
    let core = Crate::all(db)
        .into_iter()
        .find(|krate| krate.origin(db) == CrateOrigin::Lang(LangCrateOrigin::Core))
        .ok_or("selected core crate was not loaded with its language origin")?;
    let std = Crate::all(db)
        .into_iter()
        .find(|krate| krate.origin(db) == CrateOrigin::Lang(LangCrateOrigin::Std))
        .ok_or("selected std crate was not loaded with its language origin")?;
    let consumer = workspace
        .crate_by_name("demo")
        .ok_or("selected consumer crate was not loaded")?;
    let loaded_root = |krate: Crate| -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
        let path = workspace.vfs.file_path(krate.root_file(db));
        let path = path.as_path().ok_or("selected crate root was not a physical path")?;
        let path: &std::path::Path = path.as_ref();
        Ok(path.to_path_buf())
    };
    let sysroot_root = sysroot.path().canonicalize()?;
    assert_eq!(loaded_root(core)?, sysroot_root.join("core/lib.rs"));
    assert_eq!(loaded_root(std)?, sysroot_root.join("std/lib.rs"));

    let dependencies = consumer
        .dependencies(db)
        .into_iter()
        .map(|dependency| (dependency.name.as_str().to_owned(), dependency.krate.origin(db)))
        .collect::<std::collections::BTreeMap<_, _>>();
    assert_eq!(
        dependencies,
        std::collections::BTreeMap::from([
            ("core".to_string(), CrateOrigin::Lang(LangCrateOrigin::Core)),
            ("std".to_string(), CrateOrigin::Lang(LangCrateOrigin::Std)),
        ])
    );

    let semantics = Semantics::new(db);
    let source = semantics.parse_guess_edition(consumer.root_file(db));
    let selected_path = source
        .syntax()
        .descendants()
        .filter_map(ra_ap_syntax::ast::Path::cast)
        .find(|path| path.syntax().text().to_string() == "SelectedPreludeMarker")
        .ok_or("consumer prelude path was absent from its selected source")?;
    let resolution = semantics
        .resolve_path(&selected_path)
        .ok_or("selected std prelude did not resolve in the consumer")?;
    let PathResolution::Def(ModuleDef::Adt(marker)) = resolution else {
        return Err(format!("selected std prelude resolved to the wrong item: {resolution:?}").into());
    };
    assert_eq!(marker.name(db).as_str(), "SelectedPreludeMarker");
    assert_eq!(
        marker.module(db).krate(db).origin(db),
        CrateOrigin::Lang(LangCrateOrigin::Std)
    );
    Ok(())
}

/// The JSON sysroot path remains functional when every ambient Rust tool entrypoint is trapped.
#[cfg(unix)]
#[test]
fn selected_json_sysroot_does_not_spawn_ambient_tools() -> Result<(), Box<dyn std::error::Error>> {
    use std::os::unix::fs::PermissionsExt;
    let guard = tempfile::tempdir()?;
    let marker = guard.path().join("unexpected-tool-call");
    let trap = guard.path().join("tool-trap");
    fs::write(
        &trap,
        "#!/bin/sh\nprintf '%s\\n' \"$0 $*\" >> \"$INCAN_INSPECTION_TOOL_MARKER\"\nexit 91\n",
    )?;
    fs::set_permissions(&trap, fs::Permissions::from_mode(0o755))?;
    let bin = guard.path().join("bin");
    fs::create_dir(&bin)?;
    for tool in ["cargo", "rustc", "rustup"] {
        fs::hard_link(&trap, bin.join(tool))?;
    }
    let output = std::process::Command::new(std::env::current_exe()?)
        .args([
            "--exact",
            "loader::tests::selected_json_sysroot_supplies_consumer_language_semantics",
            "--nocapture",
        ])
        .env("INCAN_INSPECTION_TOOL_MARKER", &marker)
        .env("CARGO", &trap)
        .env("RUSTC", &trap)
        .env("CARGO_HOME", &bin)
        .env("RUSTUP_HOME", &bin)
        .env("PATH", &bin)
        .output()?;
    assert!(
        output.status.success(),
        "isolated selected-sysroot loader failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("1 passed; 0 failed"),
        "isolated harness did not execute exactly one passing selected-sysroot probe"
    );
    if marker.exists() {
        return Err(format!(
            "selected-sysroot loader invoked an ambient tool: {}",
            fs::read_to_string(&marker)?
        )
        .into());
    }
    Ok(())
}

/// Selected sysroot bytes, graph shape and physical root all fail closed before loading.
#[test]
fn selected_json_sysroot_rejects_unbound_convenience_and_empty_inputs() -> Result<(), Box<dyn std::error::Error>> {
    let mut fixture = InspectionFixture::new("pub struct Consumer;\n")?;
    let sysroot = attach_json_sysroot(&mut fixture)?;

    let mut wrong_digest = fixture.inputs.clone();
    wrong_digest
        .sysroot
        .as_mut()
        .ok_or("selected sysroot input absent")?
        .project_digest = byte_digest(b"different sysroot projection");
    assert!(matches!(
        ValidatedInspectionProject::validate(wrong_digest),
        Err(RustMetadataError::InvalidSelectedInput { .. })
    ));

    let mut convenience = fixture.inputs.clone();
    let selected = convenience.sysroot.as_mut().ok_or("selected sysroot input absent")?;
    let mut project: serde_json::Value = serde_json::from_slice(&selected.project_json)?;
    project["sysroot_src"] = serde_json::json!(sysroot.path().canonicalize()?);
    selected.project_json = serde_json::to_vec(&project)?;
    selected.project_digest = byte_digest(&selected.project_json);
    assert!(matches!(
        ValidatedInspectionProject::validate(convenience),
        Err(RustMetadataError::InvalidSelectedInput { .. })
    ));

    let mut outside = fixture.inputs.clone();
    outside
        .sysroot
        .as_mut()
        .ok_or("selected sysroot input absent")?
        .source_root = fixture.output.path().canonicalize()?;
    assert!(matches!(
        ValidatedInspectionProject::validate(outside),
        Err(RustMetadataError::InvalidSelectedInput { .. })
    ));

    let mut empty = fixture.inputs.clone();
    let selected = empty.sysroot.as_mut().ok_or("selected sysroot input absent")?;
    selected.project_json = serde_json::to_vec(&serde_json::json!({"crates": []}))?;
    selected.project_digest = byte_digest(&selected.project_json);
    assert!(matches!(
        ValidatedInspectionProject::validate(empty),
        Err(RustMetadataError::SelectedInputUnavailable { .. })
    ));
    assert_eq!(fs::read_dir(fixture.output.path())?.count(), 0);
    Ok(())
}

/// The physical loader works with empty ambient tool/cache roots and never invokes the supplied trap executables.
#[cfg(unix)]
#[test]
fn selected_neutral_loader_does_not_spawn_ambient_tools() -> Result<(), Box<dyn std::error::Error>> {
    const CHILD: &str = "INCAN_INSPECTION_PROCESS_BOUNDARY_TEST";
    if std::env::var_os(CHILD).is_some() {
        let fixture = InspectionFixture::new("#![no_std]\npub struct Local { pub number: u64 }\n")?;
        let workspace = fixture.load()?;
        assert!(extract_rust_item(&workspace, "demo::Local").is_ok());
        return Ok(());
    }
    use std::os::unix::fs::PermissionsExt;
    let guard = tempfile::tempdir()?;
    let marker = guard.path().join("unexpected-tool-call");
    let trap = guard.path().join("tool-trap");
    fs::write(
        &trap,
        "#!/bin/sh\nprintf '%s\\n' \"$0 $*\" >> \"$INCAN_INSPECTION_TOOL_MARKER\"\nexit 91\n",
    )?;
    fs::set_permissions(&trap, fs::Permissions::from_mode(0o755))?;
    let empty = guard.path().join("empty");
    fs::create_dir(&empty)?;
    let output = std::process::Command::new(std::env::current_exe()?)
        .args([
            "--exact",
            "loader::tests::selected_neutral_loader_does_not_spawn_ambient_tools",
            "--nocapture",
        ])
        .env(CHILD, "1")
        .env("INCAN_INSPECTION_TOOL_MARKER", &marker)
        .env("CARGO", &trap)
        .env("RUSTC", &trap)
        .env("CARGO_HOME", &empty)
        .env("RUSTUP_HOME", &empty)
        .env("PATH", &empty)
        .output()?;
    assert!(
        output.status.success(),
        "isolated loader failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("1 passed; 0 failed"),
        "isolated harness did not execute exactly one passing probe"
    );
    assert!(
        !marker.exists(),
        "loader invoked an ambient tool: {}",
        fs::read_to_string(&marker).unwrap_or_default()
    );
    Ok(())
}

/// Replacing source bytes at the same selected path requires a new binding and exposes the new metadata.
#[test]
fn selected_source_replacement_changes_binding_and_metadata_issue911() -> Result<(), Box<dyn std::error::Error>> {
    let mut fixture = InspectionFixture::new("#![no_std]\npub struct Item { pub before: u8 }\n")?;
    let before = fixture.validate()?;
    let workspace = RustWorkspace::load_selected(&before, fixture.output.path(), &|_| {})?;
    let incan_core::interop::RustItemKind::Type(original) = extract_rust_item(&workspace, "demo::Item")?.kind else {
        return Err("original selected type absent".into());
    };
    assert_eq!(original.fields[0].name, "before");
    fs::write(fixture.module()?, "#![no_std]\npub struct Item { pub after: bool }\n")?;
    assert!(matches!(
        RustWorkspace::load_selected(&before, fixture.output.path(), &|_| {}),
        Err(RustMetadataError::InvalidSelectedInput { .. })
    ));
    fixture.inputs.sources[0].digest = super::digest_oven_source_tree(fixture.source.path())?;
    let after = fixture.validate()?;
    assert_ne!(before.fingerprint(), after.fingerprint());
    let workspace = RustWorkspace::load_selected(&after, fixture.output.path(), &|_| {})?;
    let incan_core::interop::RustItemKind::Type(replacement) = extract_rust_item(&workspace, "demo::Item")?.kind else {
        return Err("replacement selected type absent".into());
    };
    assert_eq!(replacement.fields[0].name, "after");
    assert_eq!(fs::read_dir(fixture.output.path())?.count(), 0);
    Ok(())
}

/// A Rust-safe query spelling binds its selected unit without generating or consulting a Cargo dependency key.
#[test]
fn selected_rust_safe_alias_preserves_its_explicit_package_binding() -> Result<(), Box<dyn std::error::Error>> {
    let mut fixture = InspectionFixture::new("#![no_std]\npub struct Conversion;\n")?;
    let mut project: serde_json::Value = serde_json::from_slice(&fixture.inputs.project_json)?;
    project["crates"][0]["display_name"] = serde_json::json!("datafusion-substrait");
    fixture.set_project(project)?;
    fixture.inputs.query_roots.clear();
    fixture
        .inputs
        .query_roots
        .insert("datafusion_substrait".to_string(), fixture.module()?);
    let workspace = fixture.load()?;
    assert!(extract_rust_item(&workspace, "datafusion_substrait::Conversion").is_ok());
    assert!(matches!(
        extract_rust_item(&workspace, "datafusion-substrait::Conversion"),
        Err(RustMetadataError::CrateNotFound(_))
    ));
    assert_eq!(fs::read_dir(fixture.output.path())?.count(), 0);
    assert_eq!(
        fs::read_to_string(fixture.source.path().join("Cargo.toml"))?,
        "invalid cargo manifest: do not read\n"
    );
    Ok(())
}

/// The admitted active SDK edge supports both metadata and direct native compilation without rewriting providers.
#[test]
fn selected_sdk_dependency_projection_compiles_all_three_units_issue911() -> Result<(), Box<dyn std::error::Error>> {
    use std::path::Path;
    use std::process::Command;

    let runtime = InspectionFixture::new("#![no_std]\npub fn value() -> u8 { 3 }\n")?;
    let mut provider = InspectionFixture::new("#![no_std]\npub fn value() -> u8 { issue911_runtime::value() }\n")?;
    let mut probe = InspectionFixture::new("#![no_std]\npub fn value() -> u8 { issue911_compiled::value() }\n")?;
    let absent_sdk = probe.output.path().join("absent-old-sdk");
    fs::write(
        provider.source.path().join("Cargo.toml"),
        format!(
            "[package]\nname = \"issue911_compiled\"\nversion = \"0.1.0\"\n[dependencies.issue911_runtime]\npath = {:?}\n",
            absent_sdk.to_string_lossy()
        ),
    )?;
    provider.inputs.sources[0].digest = super::digest_oven_source_tree(provider.source.path())?;
    let runtime_project: serde_json::Value = serde_json::from_slice(&runtime.inputs.project_json)?;
    let provider_project: serde_json::Value = serde_json::from_slice(&provider.inputs.project_json)?;
    let probe_project: serde_json::Value = serde_json::from_slice(&probe.inputs.project_json)?;
    let mut selected = serde_json::json!({"crates": [
        runtime_project["crates"][0], provider_project["crates"][0], probe_project["crates"][0]
    ]});
    for index in 0..3 {
        selected["crates"][index]["edition"] = serde_json::json!("2024");
    }
    selected["crates"][1]["deps"] = serde_json::json!([{"crate": 0, "name": "issue911_runtime"}]);
    selected["crates"][2]["deps"] = serde_json::json!([{"crate": 1, "name": "issue911_compiled"}]);
    probe.set_project(selected)?;
    probe.inputs.sources.extend(runtime.inputs.sources.clone());
    probe.inputs.sources.extend(provider.inputs.sources.clone());
    probe.inputs.query_roots = std::collections::BTreeMap::from([
        ("issue911_runtime".to_string(), runtime.module()?),
        ("issue911_compiled".to_string(), provider.module()?),
        ("issue911_probe".to_string(), probe.module()?),
    ]);
    let validated = probe.validate()?;
    let workspace = RustWorkspace::load_selected(&validated, probe.output.path(), &|_| {})?;
    for name in ["issue911_runtime", "issue911_compiled", "issue911_probe"] {
        let incan_core::interop::RustItemKind::Function(function) =
            extract_rust_item(&workspace, &format!("{name}::value"))?.kind
        else {
            return Err(format!("selected function `{name}::value` absent").into());
        };
        assert_eq!(function.return_type, "u8");
    }
    let before = [&runtime, &provider, &probe]
        .map(|fixture| super::digest_oven_source_tree(fixture.source.path()))
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?;
    let rustc = std::env::var_os("INCAN_OVEN_COMPILER_SUITE_RUSTC")
        .or_else(|| std::env::var_os("RUSTC"))
        .unwrap_or_else(|| "rustc".into());
    let compile = |source: &Path,
                   name: &str,
                   output: &Path,
                   externs: &[(&str, &Path)]|
     -> Result<(), Box<dyn std::error::Error>> {
        let mut command = Command::new(&rustc);
        command
            .args(["--edition", "2024", "--crate-name", name, "--crate-type", "lib"])
            .arg(source)
            .arg("-o")
            .arg(output)
            .env_remove("CARGO")
            .env_remove("CARGO_MANIFEST_DIR")
            .env_remove("CARGO_MANIFEST_PATH");
        for (dependency, artifact) in externs {
            let parent = artifact.parent().ok_or("selected dependency output has no parent")?;
            command
                .arg("-L")
                .arg(format!("dependency={}", parent.display()))
                .arg("--extern")
                .arg(format!("{dependency}={}", artifact.display()));
        }
        let result = command.output()?;
        assert!(
            result.status.success(),
            "selected direct-rustc unit `{name}` failed:\n{}{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(
            output.is_file(),
            "selected unit `{name}` did not produce its native output"
        );
        Ok(())
    };
    let runtime_output = probe.output.path().join("libissue911_runtime.rlib");
    let provider_output = probe.output.path().join("libissue911_compiled.rlib");
    let probe_output = probe.output.path().join("libissue911_probe.rlib");
    compile(&runtime.module()?, "issue911_runtime", &runtime_output, &[])?;
    compile(
        &provider.module()?,
        "issue911_compiled",
        &provider_output,
        &[("issue911_runtime", runtime_output.as_path())],
    )?;
    compile(
        &probe.module()?,
        "issue911_probe",
        &probe_output,
        &[("issue911_compiled", provider_output.as_path())],
    )?;
    for (fixture, digest) in [&runtime, &provider, &probe].into_iter().zip(before) {
        assert_eq!(super::digest_oven_source_tree(fixture.source.path())?, digest);
    }
    assert!(!absent_sdk.exists());
    fs::write(provider.module()?, "#![no_std]\npub fn corrupt() {}\n")?;
    assert!(matches!(
        RustWorkspace::load_selected(&validated, probe.output.path(), &|_| {}),
        Err(RustMetadataError::InvalidSelectedInput { .. })
    ));
    assert_eq!(
        fs::read_to_string(provider.module()?)?,
        "#![no_std]\npub fn corrupt() {}\n",
        "inspection must refuse changed immutable inputs, not silently rewrite them"
    );
    Ok(())
}
