//! Native-input selection and lease-retention regression tests.

use super::*;

#[cfg(unix)]
#[test]
fn native_input_view_refuses_selected_symlink_after_admission() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = native_input_fixture("debug")?;
    let view = super::super::native_input::OvenNativeInputView::from_store_payload(&fixture.owner)?;
    let selected = fixture.owner.artifact_root.join("release/deps/libserde_fixture.rlib");
    let foreign = fixture.root.path().join("foreign.rlib");
    fs::write(&foreign, b"registry bytes")?;
    fs::remove_file(&selected)?;
    std::os::unix::fs::symlink(&foreign, &selected)?;
    let candidate = view
        .registry_candidates("generated-root")?
        .pop()
        .ok_or("leaf absent")?
        .with_alias("dependency".to_string())?;
    assert!(view.attach_for_source("generated-root", &[candidate]).is_err());
    Ok(())
}

#[test]
fn native_input_view_exposes_original_facts_without_path_profile_selection() -> Result<(), Box<dyn std::error::Error>> {
    let debug = native_input_fixture("debug")?;
    let release = native_input_fixture("release")?;
    for fixture in [&debug, &release] {
        let view = super::super::native_input::OvenNativeInputView::from_store_payload(&fixture.owner)?;
        assert_eq!(view.identity(), fixture.owner.manifest.identity);
        assert_eq!(view.receipt_identity(), Some(fixture.receipt.identity.as_str()));
        assert_eq!(view.build_unit_identity(), fixture.receipt.build_unit_identity);
        assert_eq!(view.intent(), &fixture.receipt.intent);
        let candidates = view.registry_candidates("generated-root")?;
        let candidate = candidates.first().ok_or("registry candidate absent")?;
        assert!(candidate.artifact().relative_path.starts_with("release/"));
        assert_eq!(
            candidate.registry_leaf().ok_or("registry facts absent")?.version,
            "1.2.3"
        );
        assert!(view.named_candidates("new-consumer-source-key").is_err());
        let named = view.named_candidates("generated-root")?;
        assert_eq!(
            named
                .iter()
                .map(|item| item.artifact().crate_name.as_str())
                .collect::<Vec<_>>(),
            vec!["runtime"]
        );
        assert!(named.iter().all(|item| item.registry_leaf().is_none()));
    }
    Ok(())
}

#[test]
fn native_input_view_attaches_selected_alias_and_preserves_role_closure() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = native_input_fixture("debug")?;
    let view = super::super::native_input::OvenNativeInputView::from_store_payload(&fixture.owner)?;
    let before =
        super::super::trusted_artifact_plan_for_source_evidence(&view.plan, &view.artifacts, "generated-root")?;
    let chosen = view
        .registry_candidates("generated-root")?
        .pop()
        .ok_or("registry candidate absent")?
        .with_alias("json_alias".to_string())?;
    let existing = view
        .named_candidates("generated-root")?
        .pop()
        .ok_or("named runtime absent")?
        .with_alias("runtime".to_string())?;
    let attached = view.attach_for_source("generated-root", &[chosen, existing])?;
    let plan = attached.artifact_plan();
    assert_eq!(plan.dependency_search_paths, before.dependency_search_paths);
    assert_eq!(plan.native_search_paths, before.native_search_paths);
    assert_eq!(plan.compile_environment, before.compile_environment);
    assert_eq!(plan.externs.len(), 2);
    assert_eq!(plan.externs[1].0, "json_alias");
    assert_eq!(
        plan.caller_owned_library_digests.get("json_alias"),
        Some(&digest_bytes(b"registry bytes"))
    );
    assert!(
        !plan
            .externs
            .iter()
            .any(|(name, _)| name == "private" || name == "transitive")
    );
    assert_eq!(view.plan.externs.len(), 2, "attachment must not mutate the foundation");
    Ok(())
}

#[test]
fn native_input_view_refuses_foreign_foundation_role_and_alias_collision() -> Result<(), Box<dyn std::error::Error>> {
    let first = native_input_fixture("debug")?;
    let second = native_input_fixture("debug")?;
    let view = super::super::native_input::OvenNativeInputView::from_store_payload(&first.owner)?;
    let other = super::super::native_input::OvenNativeInputView::from_store_payload(&second.owner)?;
    let foreign = other
        .registry_candidates("generated-root")?
        .pop()
        .ok_or("foreign leaf absent")?
        .with_alias("dependency".to_string())?;
    assert!(view.attach_for_source("generated-root", &[foreign]).is_err());
    let wrong_role = view
        .named_candidates("compiler-helper")?
        .pop()
        .ok_or("helper absent")?
        .with_alias("dependency".to_string())?;
    assert!(view.attach_for_source("generated-root", &[wrong_role]).is_err());
    let colliding = view
        .registry_candidates("generated-root")?
        .pop()
        .ok_or("leaf absent")?
        .with_alias("runtime".to_string())?;
    assert!(view.attach_for_source("generated-root", &[colliding]).is_err());
    let invalid_alias = view.registry_candidates("generated-root")?.pop().ok_or("leaf absent")?;
    assert!(invalid_alias.with_alias("not an identifier".to_string()).is_err());
    let excluded = view
        .registry_candidates("compiler-helper")?
        .pop()
        .ok_or("leaf facts absent")?
        .with_alias("dependency".to_string())?;
    assert!(
        view.attach_for_source("compiler-helper", &[excluded]).is_err(),
        "an excluded physical directory must not be restored"
    );
    assert!(!first.root.path().join("output").exists());
    Ok(())
}

#[test]
fn compiler_runtime_member_roundtrip_preserves_original_role_and_rechecks_bytes()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = native_input_fixture("debug")?;
    let view = super::super::native_input::OvenNativeInputView::from_store_payload(&fixture.owner)?;
    let members = view.named_candidates("generated-root")?;
    let member = members.first().ok_or("original runtime member absent")?;
    let request = serde_json::json!({
        "grant_id": "original-member-0",
        "plan_identity": view.identity(),
        "source_role": member.source_role,
        "extern_name": member.artifact().crate_name,
        "artifact_digest": member.artifact().digest,
    });
    let decoded: serde_json::Value = serde_json::from_slice(&serde_json::to_vec(&request)?)?;
    assert_eq!(decoded["source_role"], "generated-root");
    assert_eq!(decoded["extern_name"], "runtime");
    assert_eq!(decoded["artifact_digest"], digest_bytes(b"runtime bytes"));
    // The selector returns an ID, never replacement member bytes or an executable path.
    let selected_ids: Vec<String> = serde_json::from_slice(br#"["original-member-0"]"#)?;
    let mut table = BTreeMap::from([(
        "original-member-0".to_string(),
        members.into_iter().next().ok_or("member absent")?,
    )]);
    let selected = selected_ids
        .into_iter()
        .map(|id| {
            table
                .remove(&id)
                .ok_or("unknown or repeated member ID")?
                .with_alias("runtime".to_string())
                .map_err(Into::into)
        })
        .collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?;
    assert!(table.remove("child-invented-member").is_none());
    let attached = view.attach_for_source("generated-root", &selected)?;
    assert!(
        attached
            .artifact_plan()
            .externs
            .iter()
            .any(|(name, _)| name == "runtime")
    );
    assert!(
        !attached
            .artifact_plan()
            .externs
            .iter()
            .any(|(name, _)| name == "private")
    );
    assert!(view.attach_for_source("compiler-helper", &selected).is_err());
    // Admission seals materialized files read-only; replace this test-owned entry rather than writing
    // through the seal.
    let runtime_member = fixture.owner.artifact_root.join("release/deps/libruntime.rlib");
    fs::remove_file(&runtime_member)?;
    fs::write(&runtime_member, b"changed named runtime")?;
    assert!(matches!(
        view.attach_for_source("generated-root", &selected),
        Err(OvenRustcError::ArtifactDigestMismatch { .. })
    ));
    Ok(())
}

#[test]
fn native_input_view_queries_are_read_free_but_attachment_rechecks_selected_bytes()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = native_input_fixture("debug")?;
    let view = super::super::native_input::OvenNativeInputView::from_store_payload(&fixture.owner)?;
    let original_plan = view.plan.clone();
    // Admission seals materialized files read-only, so replacing this test-owned entry models the external
    // mutation under test rather than writing through the seal.
    let file = fixture.owner.artifact_root.join("release/deps/libserde_fixture.rlib");
    fs::remove_file(&file)?;
    fs::write(&file, b"changed after admission")?;
    let chosen = view
        .registry_candidates("generated-root")?
        .pop()
        .ok_or("facts changed after physical mutation")?
        .with_alias("dependency".to_string())?;
    assert!(matches!(
        view.attach_for_source("generated-root", &[chosen]),
        Err(OvenRustcError::ArtifactDigestMismatch { .. })
    ));
    assert_eq!(view.plan, original_plan);
    assert!(!fixture.root.path().join("output").exists());
    Ok(())
}

#[test]
fn native_input_view_rejects_mutated_payload_manifest_and_coordinated_retargeting()
-> Result<(), Box<dyn std::error::Error>> {
    let mut first = native_input_fixture("debug")?;
    let second = native_input_fixture("debug")?;
    assert_eq!(first.owner.manifest, second.owner.manifest);
    assert_eq!(first.owner.payload, second.owner.payload);
    assert_ne!(first.owner.artifact_root, second.owner.artifact_root);
    let original_payload = first.owner.payload.clone();
    first.owner.payload.push(b' ');
    assert!(super::super::native_input::OvenNativeInputView::from_store_payload(&first.owner).is_err());
    first.owner.payload = original_payload;
    let original_manifest = first.owner.manifest.clone();
    first.owner.manifest.intent.profile = "release".to_string();
    assert!(super::super::native_input::OvenNativeInputView::from_store_payload(&first.owner).is_err());
    first.owner.manifest = original_manifest;
    assert!(super::super::native_input::OvenNativeInputView::from_store_payload(&first.owner).is_ok());
    first.owner.manifest = second.owner.manifest.clone();
    first.owner.artifact_root = second.owner.artifact_root.clone();
    first.owner.payload = second.owner.payload.clone();
    assert!(
        super::super::native_input::OvenNativeInputView::from_store_payload(&first.owner).is_err(),
        "even identical valid records cannot retarget the original selected lease to another root"
    );
    Ok(())
}

#[test]
fn native_input_view_keeps_real_store_lease_through_attached_plan_use() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = native_input_fixture("debug")?;
    let bounded = OvenStore::new(
        fixture.root.path().join("store"),
        OvenStoreLimits::new(1, 4 * 1024 * 1024, 4 * 1024 * 1024),
    );
    let view = super::super::native_input::OvenNativeInputView::from_store_payload(&fixture.owner)?;
    let attached = view.attach_for_source("generated-root", &[])?;
    bounded.prune()?;
    let held = fixture.store.inspect()?;
    assert_eq!(held.entries.len(), 1);
    assert_eq!(held.active_lease_physical_bytes, held.physical_bytes);
    assert!(attached.artifact_plan().externs[0].1.is_file());
    drop(attached);
    drop(view);
    drop(fixture.owner);
    bounded.prune()?;
    assert!(bounded.inspect()?.entries.is_empty());
    Ok(())
}
