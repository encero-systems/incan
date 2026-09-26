//! A `pub::` dependency method's union result crosses into the consumer's own union positions (#1797).

use super::packages::{TestResult, parse, provider_plan_of, publish_package};
use crate::IrCodegen;
use std::sync::Arc;

/// The `querykit` provider: one model whose methods return `int | str` bare, in a list and in an option.
const QUERYKIT: &str = r#"
pub model Box:
    pub value: int

    def answer(self) -> int | str:
        if self.value > 0:
            return self.value
        return "empty"

    def answers(self) -> list[int | str]:
        return [self.answer()]

    def maybe(self) -> Option[int | str]:
        if self.value > 0:
            return Some(self.answer())
        return None
"#;

/// A consumer passing each result to its own `int | str` positions and matching the optional one by member.
const CONSUMER: &str = r#"
from pub::querykit import Box


def show(value: int | str, label: str) -> str:
    match value:
        int(number) => return f"{label} {number}"
        str(text) => return f"{label} {text}"


def show_first(values: list[int | str]) -> str:
    return show(values[0], "first")


def main() -> None:
    println(show(Box(value=3).answer(), "answer"))
    println(show_first(Box(value=5).answers()))
    match Box(value=7).maybe():
        int(number) => println(f"maybe {number}")
        str(text) => println(f"maybe {text}")
        None => println("none")
"#;

/// Return `code` without whitespace, so assertions do not depend on how the Rust is laid out.
fn compact(code: &str) -> String {
    code.chars().filter(|character| !character.is_whitespace()).collect()
}

/// #1797: the consumer converts each dependency method result from the provider's `int | str` wrapper into its own,
/// bare and element by element, and matches the optional result through the provider's wrapper. The provider and the
/// consumer define a wrapper of the same name, so the conversion is what keeps rustc from seeing two distinct types
/// meet (E0308).
#[test]
fn dependency_method_union_results_convert_into_the_consumers_unions_issue1797() -> TestResult {
    let (querykit, querykit_code) = publish_package("querykit", QUERYKIT, &[])?;
    let wrapper = querykit_code
        .lines()
        .find_map(|line| line.trim().strip_prefix("pub enum __IncanUnion"))
        .and_then(|tail| tail.split_whitespace().next())
        .map(|hash| format!("__IncanUnion{hash}"))
        .ok_or_else(|| format!("the provider defines no union wrapper:\n{querykit_code}"))?;
    let consumer = parse(CONSUMER)?;
    let mut codegen = IrCodegen::new();
    codegen.set_provider_plan(Arc::new(provider_plan_of(&[&querykit])?));
    let code = codegen.try_generate(&consumer)?;
    let compacted = compact(&code);

    let provider_arm =
        format!("::querykit::{wrapper}::V0(__incan_union_value)=>{{{wrapper}::V0(__incan_union_value)}}");
    assert!(
        compacted.contains(&format!(".answer(){{{provider_arm}")),
        "`answer()` converts into the consumer's wrapper:\n{code}"
    );
    assert!(
        compacted.contains(&format!(
            ".answers()).into_iter().map(|__incan_item|match__incan_item{{{provider_arm}"
        )),
        "`answers()` converts element by element:\n{code}"
    );
    assert_eq!(
        compacted.matches(&format!("Some(::querykit::{wrapper}::")).count(),
        2,
        "both member arms over `maybe()` match the provider's wrapper:\n{code}"
    );
    Ok(())
}
