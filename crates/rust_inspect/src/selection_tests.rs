use std::fs;

use crate::selection::ValidatedInspectionProject;
use crate::selection_test_support::{InspectionFixture, byte_digest};
use crate::{RustMetadataError, RustWorkspace, extract_rust_item};

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
