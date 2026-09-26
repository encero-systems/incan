//! The checker records which display operands of an `Error` adopter render through `message()` (#1778): an f-string
//! `{value}` part, `str(value)` and each `print` argument of a type that adopts `Error` and defines no `__str__`.

use super::*;

/// Return the text of every operand the checker recorded as rendering through `message()`, in source order.
fn recorded_error_displays(source: &str) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let info = typecheck_info_for_module(source, vec!["main".to_string()], "error display program")?;
    let mut spans = info.calls.error_message_displays.keys().copied().collect::<Vec<_>>();
    spans.sort_unstable();
    spans
        .into_iter()
        .map(|(start, end)| {
            source
                .get(start..end)
                .map(str::to_string)
                .ok_or_else(|| format!("recorded span {start}..{end} lies outside the source").into())
        })
        .collect()
}

/// A local `Error` adopter with no `__str__` renders `message()` in all three display positions; a debug
/// interpolation and an adopter that defines `__str__` are not recorded.
#[test]
fn error_adopter_without_str_is_recorded_in_every_display_position_issue1778() -> Result<(), Box<dyn std::error::Error>>
{
    let source = r#"
from std.traits.error import Error


model ParseFailure with Error:
    detail: str

    def message(self) -> str:
        return self.detail


model TaggedFailure with Error:
    detail: str

    def message(self) -> str:
        return self.detail

    def __str__(self) -> str:
        return "tagged"


def main() -> None:
    failure = ParseFailure(detail="bad digit")
    tagged = TaggedFailure(detail="kept")
    println(f"error {failure}")
    println(str(failure))
    println(failure)
    println(f"{failure:?}")
    println(f"{tagged}")
    println(str(tagged))
    println(tagged)
"#;
    // An f-string part's span covers its braces.
    assert_eq!(
        recorded_error_displays(source)?,
        ["{failure}", "failure", "failure"],
        "the f-string part, the `str` argument and the `println` argument of `ParseFailure` render `message()`"
    );
    Ok(())
}

/// The standard library's own `IoError` adopts `Error` with no `__str__`, so the issue's program renders its message.
#[test]
fn stdlib_io_error_interpolation_is_recorded_issue1778() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
from std.io import BytesIO


def main() -> None:
    match BytesIO(b"ab").read(1):
        Ok(data) => println(len(data))
        Err(error) => println(f"error {error}")
"#;
    assert_eq!(recorded_error_displays(source)?, ["{error}"]);
    Ok(())
}

/// The recorded fact is the call a written `failure.message()` resolves to: the same declaration and dispatch.
#[test]
fn recorded_error_display_is_the_written_message_call_issue1778() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
from std.traits.error import Error


model ParseFailure with Error:
    detail: str

    def message(self) -> str:
        return self.detail


def written(failure: ParseFailure) -> str:
    return failure.message()


def render(failure: ParseFailure) -> str:
    return f"{failure}"
"#;
    let info = typecheck_info_for_module(source, vec!["main".to_string()], "error display identity")?;
    let call_start = source
        .find("failure.message()")
        .ok_or("the written call is in the source")?;
    let call_span = Span::new(call_start, call_start + "failure.message()".len());
    let operand_start = source.find("{failure}").ok_or("the interpolation is in the source")?;
    let display = info
        .error_message_display(Span::new(operand_start, operand_start + "{failure}".len()))
        .ok_or("the interpolation must render `message()`")?;
    let identity = display
        .identity
        .as_ref()
        .ok_or("the display must name the `message` declaration")?;
    assert_eq!(identity.declaration_name, "message");
    assert_eq!(identity.kind, SemanticSourceTargetKind::Method);
    assert_eq!(
        display.identity.as_ref(),
        info.resolved_identity(call_span),
        "the display names the declaration the written call names"
    );
    assert_eq!(
        display.dispatch.as_ref(),
        info.resolved_method_call(call_span).map(|call| &call.dispatch),
        "the display dispatches as the written call does"
    );
    Ok(())
}

/// A value of a type parameter bounded by `Error`, and an adopter whose `message` is a trait default, render their
/// `message()` too; both reach it through trait dispatch, exactly as a written call does.
#[test]
fn generic_and_trait_default_error_displays_resolve_through_trait_dispatch_issue1778()
-> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
from std.traits.error import Error


trait AppError with Error:
    def message(self) -> str:
        return "application failure"


model Timeout with AppError:
    seconds: int


def describe[E with Error](failure: E) -> str:
    return f"{failure}"


def timeout_text(timeout: Timeout) -> str:
    return str(timeout)
"#;
    let info = typecheck_info_for_module(source, vec!["main".to_string()], "trait-dispatched error displays")?;
    // An f-string part's span covers its braces; the `str` argument's is the name alone.
    for operand in ["{failure}", "timeout"] {
        let span = source
            .rfind(operand)
            .ok_or_else(|| format!("`{operand}` is not in the source"))?;
        let display = info
            .error_message_display(Span::new(span, span + operand.len()))
            .ok_or_else(|| {
                format!(
                    "`{operand}` must render its `message()`: {:?}",
                    info.calls.error_message_displays
                )
            })?;
        assert!(
            matches!(display.dispatch, Some(ResolvedMethodDispatch::Trait { .. })),
            "`{operand}` reaches `message` through a trait: {display:?}"
        );
    }
    Ok(())
}

/// A type parameter bounded by `Error` and by a trait that supplies `__str__` displays itself.
#[test]
fn error_bound_beside_a_str_supplying_bound_displays_itself_issue1778() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
from std.traits.error import Error


trait Labeled:
    def __str__(self) -> str:
        return "labeled"


def describe[E with (Error, Labeled)](failure: E) -> str:
    return f"{failure}"
"#;
    assert_eq!(recorded_error_displays(source)?, Vec::<String>::new());
    Ok(())
}

/// `self` inside a default method of a trait that extends `Error` renders its `message()`, recorded as a trait-`Self`
/// display that lowering decides per adopter; a trait that supplies `__str__` keeps `self`'s own display.
#[test]
fn self_in_an_error_trait_default_is_recorded_as_a_trait_self_display_issue1778()
-> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
from std.traits.error import Error


trait Reported with Error:
    def report(self) -> str:
        return f"reported: {self}"


trait Labeled with Error:
    def __str__(self) -> str:
        return "labeled"

    def label(self) -> str:
        return f"label: {self}"
"#;
    let info = typecheck_info_for_module(source, vec!["main".to_string()], "trait self displays")?;
    let displays = info.calls.error_message_displays.values().collect::<Vec<_>>();
    let [display] = displays.as_slice() else {
        return Err(format!("only `Reported`'s `{{self}}` renders `message()`, got {displays:?}").into());
    };
    assert!(display.receiver_is_trait_self, "{display:?}");
    assert_eq!(display.identity, None);
    assert_eq!(display.dispatch, None);
    let start = source.find("{self}").ok_or("the interpolation is in the source")?;
    assert!(
        info.error_message_display(Span::new(start, start + "{self}".len()))
            .is_some(),
        "the display is recorded at `Reported`'s interpolation"
    );
    Ok(())
}
