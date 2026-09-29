//! The variants a checked module publishes for its enums: every published surface (the checked export, the checked
//! API and the library manifest) records each enum's own variants, whatever other enum shares a variant name.

use std::collections::BTreeMap;

use super::helpers::parse_program;
use super::*;

/// One enum's variants as a published surface records them: each variant's name and payload spellings.
type PublishedVariants = Vec<(String, Vec<String>)>;

/// Spell a manifest or API payload type the way the checker spells the resolved type it came from.
fn type_ref_spelling(ty: &TypeRef) -> String {
    match ty {
        TypeRef::Named { name, .. } => name.clone(),
        TypeRef::TypeParam { name } => format!("type parameter {name}"),
        other => format!("{other:?}"),
    }
}

/// Check `source` as the module `lib` of the package `flows`, beside the sibling modules `siblings`, and return the
/// variants each public enum publishes, after asserting that the checked export, the checked API and the library
/// manifest agree on them.
fn published_variants(source: &str, siblings: &[(&str, &str)]) -> Result<BTreeMap<String, PublishedVariants>, String> {
    let program = parse_program(source, "enum variant exports");
    let sibling_programs = siblings
        .iter()
        .map(|(name, source)| (*name, parse_program(source, name)))
        .collect::<Vec<_>>();
    let dependencies = sibling_programs
        .iter()
        .map(|(name, program)| (*name, program))
        .collect::<Vec<_>>();
    let module_path = vec!["lib".to_string()];
    let mut checker = TypeChecker::new();
    checker.set_current_package_identity(Some("flows".to_string()));
    checker.set_current_module_path(Some(module_path.clone()));
    checker
        .check_with_imports(&program, &dependencies)
        .map_err(|errors| format!("the library should check: {errors:?}"))?;
    let exports = collect_checked_public_exports(&program, &checker);
    let mut checked = BTreeMap::new();
    for export in &exports {
        if let CheckedExportKind::Enum(enumeration) = &export.kind {
            let variants = enumeration
                .variants
                .iter()
                .map(|variant| {
                    (
                        variant.name.clone(),
                        variant.fields.iter().map(ToString::to_string).collect::<Vec<_>>(),
                    )
                })
                .collect::<Vec<_>>();
            checked.insert(enumeration.name.clone(), variants);
        }
    }
    let api = collect_checked_api_metadata(&program, &checker, module_path);
    let mut api_variants = BTreeMap::new();
    for declaration in &api.declarations {
        if let ApiDeclaration::Enum(enumeration) = declaration {
            let variants = enumeration
                .variants
                .iter()
                .map(|variant| {
                    (
                        variant.name.clone(),
                        variant.fields.iter().map(type_ref_spelling).collect::<Vec<_>>(),
                    )
                })
                .collect::<Vec<_>>();
            api_variants.insert(enumeration.name.clone(), variants);
        }
    }
    let manifest = LibraryManifest::from_checked_exports("flows", "0.1.0", &exports);
    let manifest_variants = manifest
        .exports
        .enums
        .iter()
        .map(|enumeration| {
            let variants = enumeration
                .variants
                .iter()
                .map(|variant| {
                    (
                        variant.name.clone(),
                        variant.fields.iter().map(type_ref_spelling).collect::<Vec<_>>(),
                    )
                })
                .collect::<Vec<_>>();
            (enumeration.name.clone(), variants)
        })
        .collect::<BTreeMap<_, _>>();
    if api_variants != checked || manifest_variants != checked {
        return Err(format!(
            "the published surfaces disagree:\nchecked export: {checked:?}\nchecked API: {api_variants:?}\nmanifest: {manifest_variants:?}"
        ));
    }
    Ok(checked)
}

/// The variants `name` should publish, spelled as `published_variants` returns them.
fn variants(entries: &[(&str, &[&str])]) -> PublishedVariants {
    entries
        .iter()
        .map(|(name, fields)| {
            (
                (*name).to_string(),
                fields.iter().map(|field| (*field).to_string()).collect::<Vec<_>>(),
            )
        })
        .collect()
}

/// #1561: enums that share variant names publish their own payloads, in either declaration order. `Action.Enter`
/// used to publish the payload of the `Enter` the module bound first (`ActionKind.Enter`, none), and
/// `SignalKind.Start` the payload of `Signal.Start`.
#[test]
fn enums_sharing_a_variant_name_publish_their_own_payloads_issue1561() -> Result<(), String> {
    let published = published_variants(
        r#"
@derive(Clone, Eq)
pub enum ActionKind(str):
    Enter = "enter"
    Leave = "leave"


@derive(Clone)
pub model EnterPayload:
    step: str


@derive(Clone)
pub enum Action:
    Enter(EnterPayload)
    Leave


@derive(Clone)
pub enum Move:
    Enter(int, int)
    Leave(str)


@derive(Clone)
pub enum Signal:
    Start(int)
    Stop


@derive(Clone, Eq)
pub enum SignalKind(str):
    Start = "start"
    Stop = "stop"
"#,
        &[],
    )?;
    let expected = BTreeMap::from([
        ("ActionKind".to_string(), variants(&[("Enter", &[]), ("Leave", &[])])),
        (
            "Action".to_string(),
            variants(&[("Enter", &["EnterPayload"]), ("Leave", &[])]),
        ),
        (
            "Move".to_string(),
            variants(&[("Enter", &["int", "int"]), ("Leave", &["str"])]),
        ),
        ("Signal".to_string(), variants(&[("Start", &["int"]), ("Stop", &[])])),
        ("SignalKind".to_string(), variants(&[("Start", &[]), ("Stop", &[])])),
    ]);
    assert_eq!(published, expected);
    Ok(())
}

/// #1561: an enum publishes its own payloads when an enum imported from a sibling module shares a variant name with
/// it, and a payload type declared after the enum publishes as that type.
#[test]
fn enum_variants_publish_their_own_payloads_beside_imported_enums_issue1561() -> Result<(), String> {
    let kinds = r#"
@derive(Clone, Eq)
pub enum ActionKind(str):
    Enter = "enter"
    Leave = "leave"


@derive(Clone)
pub enum Move:
    Enter(int, int)
    Leave(str)
"#;
    let published = published_variants(
        r#"
from kinds import ActionKind, Move


@derive(Clone)
pub enum Action:
    Enter(EnterPayload)
    Leave(ActionKind)


@derive(Clone)
pub model EnterPayload:
    step: str


pub def kind_of(action: Action) -> ActionKind:
    match action:
        Action.Enter(_) => return ActionKind.Enter
        Action.Leave(kind) => return kind


pub def steps(step: Move) -> int:
    match step:
        Move.Enter(x, y) => return x + y
        Move.Leave(_) => return 0
"#,
        &[("kinds", kinds)],
    )?;
    let expected = BTreeMap::from([(
        "Action".to_string(),
        variants(&[("Enter", &["EnterPayload"]), ("Leave", &["ActionKind"])]),
    )]);
    assert_eq!(published, expected);
    Ok(())
}
