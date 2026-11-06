//! Genuine checked serde plans reject independently damaged native forwarding contracts.

use super::{TestResult, checked_source, lower_module, plan, validation};

const SOURCE: &str = r#"from std.serde.json import Deserialize

@derive(Clone, Deserialize)
type CheckedText = newtype str:
    def from_underlying(value: str) -> Result[Self, ValidationError]:
        return Ok(CheckedText(value))

@derive(Clone, Deserialize)
type OtherText = newtype str

def main() -> None:
    pass
"#;

/// Lower actual checked source, validate its ingress, then require eight independent forwarding ABI refusals.
pub(super) fn check() -> TestResult<()> {
    let module = checked_source(SOURCE, "checked_json_admission")?;
    let original = lower_module(&module, SOURCE.to_owned(), "checked_json_admission.incn".to_owned())?;
    validation::validate(&original)?;
    let owner = original
        .models
        .iter()
        .position(|model| model.source_name == "CheckedText")
        .ok_or("checked model absent")?;
    let other = original
        .models
        .iter()
        .position(|model| model.source_name == "OtherText")
        .ok_or("control model absent")?;
    let (root, result) = ingress_root(&original, owner)?;
    for damage in [
        "parameter",
        "local",
        "return-local",
        "payload",
        "lookalike",
        "owner-field",
        "missing-root",
        "foreign-owner",
    ] {
        let mut candidate = original.clone();
        match damage {
            "parameter" => {
                candidate.functions[root].parameters[0].ty = plan::PlanType::StringMutRef;
            }
            "local" => candidate.functions[root].locals[1].ty = plan::PlanType::StringMutRef,
            "return-local" => candidate.functions[root].locals[0].ty = plan::PlanType::String,
            "payload" => {
                candidate.enums[result].variants[0].fields[0] =
                    plan::PlanType::Model(other.try_into()?, candidate.models[other].name.clone())
            }
            "lookalike" => {
                candidate.enums[result].carrier.clear();
                candidate.enums[result].derives = vec!["Debug".to_owned(), "Clone".to_owned()];
            }
            "owner-field" => candidate.models[owner].fields[0].ty = plan::PlanType::Int,
            "missing-root" => {
                candidate.functions.remove(root);
            }
            "foreign-owner" => {
                candidate.models[owner].library_item = "provider::OtherText".to_owned();
                candidate.models[owner].derives.clear();
            }
            _ => return Err("unknown checked JSON ABI control".into()),
        }
        require_forwarding_refusal(&candidate, damage)?;
    }
    println!("checked JSON admission: 1 genuine plan and 8 native forwarding ABI refusals passed");
    Ok(())
}

/// Locate the generated root by its actual standard Result contract, using no future model field in red.
fn ingress_root(plan: &plan::Plan, owner: usize) -> TestResult<(usize, usize)> {
    let expected = plan::PlanType::Model(owner.try_into()?, plan.models[owner].name.clone());
    let mut found = None;
    for (index, function) in plan.functions.iter().enumerate() {
        let plan::PlanType::Enum(result, name) = &function.return_type else {
            continue;
        };
        let Some(result_index) = usize::try_from(*result).ok() else {
            continue;
        };
        let Some(declaration) = plan.enums.get(result_index) else {
            continue;
        };
        if declaration.name == *name
            && declaration.carrier == "Result"
            && declaration.variants.len() == 2
            && declaration.variants[0].fields == [expected.clone()]
            && declaration.variants[1].fields == [plan::PlanType::String]
        {
            if found.is_some() {
                return Err("checked JSON root is not unique".into());
            }
            found = Some((index, result_index));
        }
    }
    found.ok_or_else(|| "checked JSON root absent".into())
}

/// The named forwarding check must reject first, rather than rely on a later MIR mismatch or rustc failure.
fn require_forwarding_refusal(plan: &plan::Plan, label: &str) -> TestResult<()> {
    match validation::validate(plan) {
        Err(error)
            if error
                .to_string()
                .contains("unsupported checked Deserialize forwarding contract") =>
        {
            Ok(())
        }
        Err(error) => Err(format!("{label}: wrong admission boundary: {error}").into()),
        Ok(()) => Err(format!("{label}: damaged forwarding contract was admitted").into()),
    }
}
