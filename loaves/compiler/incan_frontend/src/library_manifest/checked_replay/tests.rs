//! Real checked-export replay and corrupted published-contract controls for #1337/#1698.

use super::CheckedExportReplay;
use crate::library_exports::collect_checked_public_exports;
use crate::library_exports::{CheckedExportKind, CheckedNamedExport, CheckedParamDefault, CheckedParamDefaultArg};
use crate::library_manifest::ExportIdentityKind;
use crate::typechecker::TypeChecker;

/// Obtain declaration authority from the frontend rather than inventing canonical test identities.
fn checked(source: &str) -> Result<Vec<CheckedNamedExport>, String> {
    let tokens = crate::lexer::lex(source).map_err(|errors| format!("lex: {errors:?}"))?;
    let program = crate::parser::parse(&tokens).map_err(|errors| format!("parse: {errors:?}"))?;
    let mut checker = TypeChecker::new();
    checker.set_current_package_identity(Some("ordinary".to_string()));
    checker.set_current_module_path(Some(vec!["lib".to_string()]));
    checker
        .check_program(&program)
        .map_err(|errors| format!("check: {errors:?}"))?;
    Ok(collect_checked_public_exports(&program, &checker))
}

/// Round-trip through the serialized replay payload before invoking its actual consumer.
fn replay(export: &CheckedNamedExport) -> Result<CheckedNamedExport, String> {
    let record = CheckedExportReplay::from_checked("ordinary", "1.0.0", export).map_err(|error| error.to_string())?;
    let bytes = serde_json::to_vec(&record).map_err(|error| error.to_string())?;
    let decoded: CheckedExportReplay = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    decoded.to_checked().map_err(|error| error.to_string())
}

#[test]
fn ordinary_checked_replay_preserves_callable_defaults_mutability_and_identity() -> Result<(), String> {
    let exports = checked("pub def scale(mut value: int, factor: int = 3) -> int:\n  return value * factor\n")?;
    let original = exports.first().ok_or("missing scale export")?;
    let restored = replay(original)?;
    let (CheckedExportKind::Function(original), CheckedExportKind::Function(restored_function)) =
        (&original.kind, &restored.kind)
    else {
        return Err("function shape lost".into());
    };
    assert_eq!(original.params, restored_function.params);
    assert_eq!(original.param_defaults, restored_function.param_defaults);
    assert_eq!(original.return_type, restored_function.return_type);
    assert!(restored.identity.canonical.is_some());
    assert_eq!(restored.identity.source_path, vec!["lib", "scale"]);
    Ok(())
}

#[test]
fn ordinary_checked_replay_preserves_members_bounds_and_partial_bindings() -> Result<(), String> {
    let exports = checked(
        r#"
pub model Counter:
    value: int = 4
    def read(self, fallback: int = 2) -> int:
        return self.value + fallback

pub enum State:
    Ready
    Count(int)

pub def label(size: int, text: str, suffix: str = "!") -> str:
    return text

pub small = partial label(size=3)
"#,
    )?;
    for export in &exports {
        let restored = replay(export)?;
        assert_eq!(restored.name, export.name);
        assert_eq!(restored.identity.source_path, export.identity.source_path);
        match (&export.kind, &restored.kind) {
            (CheckedExportKind::Model(a), CheckedExportKind::Model(b)) => {
                assert_eq!(a.fields.len(), b.fields.len());
                assert_eq!(a.fields[0].default, b.fields[0].default);
                assert_eq!(a.methods[0].params, b.methods[0].params);
                assert_eq!(a.methods[0].param_defaults, b.methods[0].param_defaults);
                assert_eq!(a.methods[0].receiver, b.methods[0].receiver);
                assert!(b.fields[0].canonical.is_some());
                assert!(b.methods[0].canonical.is_some());
            }
            (CheckedExportKind::Enum(a), CheckedExportKind::Enum(b)) => {
                assert_eq!(a.variants.len(), b.variants.len());
                assert_eq!(a.variants[1].fields, b.variants[1].fields);
                assert!(b.variants.iter().all(|variant| variant.canonical.is_some()));
            }
            (CheckedExportKind::Partial(a), CheckedExportKind::Partial(b)) => {
                assert_eq!(a.params, b.params);
                assert_eq!(a.presets.len(), b.presets.len());
                assert_eq!(a.target_path, b.target_path);
            }
            (CheckedExportKind::Function(_), CheckedExportKind::Function(_)) => {}
            _ => return Err("export classification changed".into()),
        }
    }
    Ok(())
}

/// Module partials keep their full positional signature and ordinary defaults; captured-slot flags belong only
/// to local partial expressions. Neither checked input nor a forged published preset may change that contract.
#[test]
fn ordinary_checked_replay_refuses_inconsistent_partial_bindings() -> Result<(), String> {
    let exports = checked(
        "pub def label(size: int, text: str, suffix: str = \"!\") -> str:\n  return text\n\npub small = partial label(size=3)\n",
    )?;
    let original = exports
        .iter()
        .find(|export| export.name == "small")
        .ok_or("missing small")?;
    let valid = CheckedExportReplay::from_checked("ordinary", "1.0.0", original).map_err(|error| error.to_string())?;
    for corruption in 0..7 {
        let mut changed = original.clone();
        let CheckedExportKind::Partial(partial) = &mut changed.kind else {
            return Err("missing partial".into());
        };
        match corruption {
            0 => partial.params[0].is_partial_preset = true,
            1 => partial.params[0].name = None,
            2 => partial.params[0].kind = crate::ast::ParamKind::RestPositional,
            3 => partial.params[0].has_default = false,
            4 => partial.presets[0].name = "missing".into(),
            5 => partial.presets[0].ty = partial.params[1].ty.clone(),
            _ => {
                let trailing = partial.params.last_mut().ok_or("missing trailing parameter")?;
                trailing.kind = crate::ast::ParamKind::RestPositional;
                trailing.has_default = false;
            }
        }
        assert!(CheckedExportReplay::from_checked("ordinary", "1.0.0", &changed).is_err());
    }
    for corruption in 0..4 {
        let mut changed = valid.clone();
        let partial = changed
            .projection
            .exports
            .partials
            .first_mut()
            .ok_or("missing published partial")?;
        match corruption {
            0 => partial.params[0].has_default = false,
            1 => partial.presets[0].name = "missing".into(),
            2 => partial.presets[0].ty = partial.params[1].ty.clone(),
            _ => {
                let trailing = partial
                    .params
                    .last_mut()
                    .ok_or("missing published trailing parameter")?;
                trailing.kind = crate::library_manifest::ParamKindExport::RestPositional;
                trailing.has_default = false;
            }
        }
        let bytes = serde_json::to_vec(&changed).map_err(|error| error.to_string())?;
        let decoded: CheckedExportReplay = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
        assert!(decoded.to_checked().is_err());
    }
    Ok(())
}

#[test]
fn ordinary_checked_replay_refuses_missing_authority_wrong_kind_and_version() -> Result<(), String> {
    let exports = checked("pub def value() -> int:\n  return 1\n")?;
    let export = exports.first().ok_or("missing export")?;
    let valid = CheckedExportReplay::from_checked("ordinary", "1.0.0", export).map_err(|error| error.to_string())?;
    let mut missing = valid.clone();
    missing.projection.contract_metadata.identity_graph.exports[0].canonical = None;
    assert!(missing.to_checked().is_err());
    let mut wrong_kind = valid.clone();
    wrong_kind.projection.contract_metadata.identity_graph.exports[0].kind = ExportIdentityKind::Model;
    assert!(wrong_kind.to_checked().is_err());
    let mut wrong_version = valid.clone();
    wrong_version.schema_version += 1;
    assert!(wrong_version.to_checked().is_err());
    let mut missing_shape = valid;
    missing_shape.projection.exports.functions.clear();
    assert!(missing_shape.to_checked().is_err());
    Ok(())
}

#[test]
fn ordinary_checked_replay_refuses_unrepresented_checked_defaults() -> Result<(), String> {
    let exports = checked("pub def label(text: str = \"a\" + \"b\") -> str:\n  return text\n")?;
    let export = exports.first().ok_or("missing export")?;
    assert!(CheckedExportReplay::from_checked("ordinary", "1.0.0", export).is_err());
    Ok(())
}

/// Corrupt a real checked default's nested callable facts before encoding; neither unnamed nor preset parameters
/// may disappear, including when the default call is nested under list/dict containers or other call arguments.
#[test]
fn ordinary_checked_replay_refuses_lossy_nested_default_signatures() -> Result<(), String> {
    let exports = checked(
        "pub def scale(value: int) -> int:\n  return value\n\npub def answer(value: int = scale(3)) -> int:\n  return value\n",
    )?;
    let original = exports
        .iter()
        .find(|export| export.name == "answer")
        .ok_or("missing answer")?;
    let restored = replay(original)?;
    let (CheckedExportKind::Function(function), CheckedExportKind::Function(replayed)) =
        (&original.kind, &restored.kind)
    else {
        return Err("missing callable".into());
    };
    assert_eq!(function.param_defaults, replayed.param_defaults);
    for unnamed in [true, false] {
        for container in 0..4 {
            let mut changed = original.clone();
            let CheckedExportKind::Function(function) = &mut changed.kind else {
                return Err("missing callable".into());
            };
            let default = function
                .param_defaults
                .first_mut()
                .and_then(Option::as_mut)
                .ok_or("missing default")?;
            let CheckedParamDefault::Call {
                signature: Some(signature),
                ..
            } = default
            else {
                return Err("missing checked nested call signature".into());
            };
            let parameter = signature.params.first_mut().ok_or("missing nested parameter")?;
            if unnamed {
                parameter.name = None;
            } else {
                parameter.is_partial_preset = true;
            }
            let nested = default.clone();
            *default = match container {
                0 => nested,
                1 => CheckedParamDefault::List(vec![nested]),
                2 => CheckedParamDefault::Dict(vec![(CheckedParamDefault::String("value".into()), nested)]),
                _ => CheckedParamDefault::Call {
                    path: vec!["scale".into()],
                    args: vec![CheckedParamDefaultArg {
                        name: None,
                        value: nested,
                    }],
                    signature: None,
                },
            };
            assert!(CheckedExportReplay::from_checked("ordinary", "1.0.0", &changed).is_err());
        }
    }
    Ok(())
}
