//! Types declared later in the same module resolve in earlier signatures, so declaration order does not matter
//! within a module (#1780).

use super::*;

/// Check a source that must be accepted, returning its diagnostics as the failure.
fn accepted(source: &str, context: &str) -> Result<(), String> {
    check_str(source).map_err(|errors| {
        format!(
            "{context}: {:?}",
            errors.iter().map(|error| &error.message).collect::<Vec<_>>()
        )
    })
}

/// #1780: a method returning a model declared after its owner exposes that model's fields at the call site.
#[test]
fn method_returning_a_later_model_keeps_its_fields_issue1780() -> Result<(), String> {
    accepted(
        r#"
model Maker:
    def make(self) -> Made:
        return Made(x=1)


model Made:
    x: int


def main() -> None:
    println(Maker().make().x)
"#,
        "a later-declared model must resolve in an earlier method's return type",
    )
}

/// #1780: fields, parameters, properties and free-function signatures that name a later declaration resolve to it,
/// and a type parameter that shares the later declaration's name still stays a type parameter.
#[test]
fn earlier_signatures_resolve_later_declarations_issue1780() -> Result<(), String> {
    accepted(
        r#"
model Holder:
    inner: Inner

    def describe(self, extra: Inner) -> int:
        return self.inner.value + extra.value


def build() -> Inner:
    return Inner(value=2)


def first[Inner](items: list[Inner]) -> Inner:
    return items[0]


model Inner:
    value: int


def main() -> None:
    holder = Holder(inner=build())
    println(holder.inner.value)
    println(holder.describe(Inner(value=3)))
    println(build().value)
    println(first([5, 6]))
"#,
        "later-declared types must resolve in earlier fields and signatures",
    )
}

/// #1780: once resolved, a later model in an earlier signature is checked like any other model, so a missing field
/// is still refused.
#[test]
fn later_model_fields_are_checked_after_resolution_issue1780() -> Result<(), String> {
    match check_str(
        r#"
model Maker:
    def make(self) -> Made:
        return Made(x=1)


model Made:
    x: int


def main() -> None:
    println(Maker().make().y)
"#,
    ) {
        Ok(()) => Err("the checker accepted a field the later model does not declare".to_string()),
        Err(errors) if errors.iter().any(|error| error.message.contains("has no field 'y'")) => Ok(()),
        Err(errors) => Err(format!(
            "expected a missing-field diagnostic for 'y', got: {:?}",
            errors.iter().map(|error| &error.message).collect::<Vec<_>>()
        )),
    }
}

/// #1780: an inline test module resolves its own later declarations the same way the module does.
#[test]
fn test_module_resolves_later_declarations_issue1780() -> Result<(), String> {
    accepted(
        r#"
module tests:
    model Maker:
        def make(self) -> Made:
            return Made(x=1)

    model Made:
        x: int

    def reads() -> int:
        return Maker().make().x
"#,
        "an inline test module must resolve a later-declared model's fields",
    )
}
