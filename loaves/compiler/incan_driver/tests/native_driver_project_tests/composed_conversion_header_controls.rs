// Genuine source-conversion plans exercise the actual adapter's independent header policy.

/// Bind the foreign graph's sole actual root to the eligible leaf header while its wrappers retain no header grant.
pub fn check_foreign(plan: &Plan) -> TestResult<()> {
    validate_source_roots(plan)?;
    for name in ["Wrapper", "Envelope"] {
        let owner = plan
            .models
            .iter()
            .find(|owner| owner.source_name == name)
            .ok_or("wrapper layout absent")?;
        assert_eq!(owner.string_conversion_root, "");
        assert_eq!(owner.string_conversion_trait, "");
    }
    let port = plan
        .models
        .iter()
        .find(|owner| owner.source_name == "Port")
        .ok_or("Port layout absent")?;
    assert!(!port.string_conversion_root.is_empty());
    assert_eq!(
        port.string_conversion_trait,
        "incan_stdlib_core::traits::convert::TryFrom"
    );
    let mut roots = plan
        .functions
        .iter()
        .filter(|function| function.name.starts_with("__incan_string_try_from_"));
    let root = roots.next().ok_or("Port conversion root absent")?;
    assert!(roots.next().is_none());
    assert_eq!(root.name, port.string_conversion_root);
    Ok(())
}

/// Admit the unchanged plan before independently damaging each source-header selection axis.
pub fn check(original: &Plan) -> TestResult<()> {
    validate_source_roots(original)?;
    let owner = original
        .models
        .iter()
        .position(|model| model.source_name == "Port")
        .ok_or("source-conversion owner missing")?;
    for damage in ["trait", "missing-trait", "missing-root", "root-prefix"] {
        let mut candidate = original.clone();
        match damage {
            "trait" => candidate.models[owner].string_conversion_trait = "other::TryFrom".to_owned(),
            "missing-trait" => candidate.models[owner].string_conversion_trait.clear(),
            "missing-root" => candidate.models[owner].string_conversion_root.clear(),
            "root-prefix" => candidate.models[owner].string_conversion_root = "main".to_owned(),
            _ => return Err("unknown source-header damage".into()),
        }
        match validate_source_roots(&candidate) {
            Err(PlanError::Invalid { reason, .. })
                if reason == "unsupported source string conversion forwarding contract" => {}
            Err(error) => return Err(format!("{damage}: wrong source-header boundary: {error}").into()),
            Ok(()) => return Err(format!("{damage}: source-header policy admitted damage").into()),
        }
    }
    Ok(())
}
