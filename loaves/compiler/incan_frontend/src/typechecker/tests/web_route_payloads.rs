//! Route payloads have a JSON form (#1768): a `Json[T]`, `Query[T]` or `Path[T]` parameter decodes the request into
//! `T` and a `Json[T]` return encodes `T`, so a model without `@derive(json)` in those positions is refused
//! (`INCAN-T0112`) instead of failing the route registration in the build.

use super::*;

/// Collect the messages of the diagnostics carrying `INCAN-T0112`, in report order.
fn payload_refusals(errors: &[CompileError]) -> Vec<String> {
    errors
        .iter()
        .filter(|error| error.stable_code() == Some("INCAN-T0112"))
        .map(|error| error.message.clone())
        .collect()
}

/// The #1768 handler: its `Search` payload has no JSON form, so both the `Query[Search]` parameter and the
/// `Json[Search]` return are refused, and the `Path[int]` capture is not.
#[test]
fn route_payload_without_json_derive_is_refused_issue1768() -> Result<(), String> {
    let source = r#"
import std.async
from std.web import Json, Query, Path, route, GET


model Search:
    q: str


@route("/snapshot/{id}", method=GET)
async def snapshot(id: Path[int], query: Query[Search]) -> Json[Search]:
    return Json(query.value)
"#;
    let errors = check_str_err(source, "a payload without a JSON form must be refused");
    assert_eq!(
        payload_refusals(&errors),
        vec![
            "Route handler 'snapshot' uses 'Json[Search]', but 'Search' has no JSON form",
            "Route handler 'snapshot' uses 'Query[Search]', but 'Search' has no JSON form",
        ],
        "the return and the query parameter are each refused once; got {:?}",
        errors.iter().map(|error| &error.message).collect::<Vec<_>>()
    );
    let refused = errors
        .iter()
        .find(|error| error.stable_code() == Some("INCAN-T0112"))
        .ok_or("the refusal must carry its stable code")?;
    assert!(
        refused
            .hints
            .iter()
            .any(|hint| hint.contains("'@derive(json)'") && hint.contains("'Search'")),
        "the remedy names the payload and the derive, got {:?}",
        refused.hints
    );
    Ok(())
}

/// The documented surface: payloads that derive `json`, a `Result` of a JSON response, a scalar `Path` payload and
/// the `Html` response are all accepted.
#[test]
fn route_payloads_with_a_json_form_are_accepted_issue1768() -> Result<(), String> {
    let source = r#"
import std.async
from std.web import Json, Html, Query, Path, route, GET, POST
from std.serde import json


@derive(json)
model Search:
    q: str


@derive(Clone, json)
model Reply:
    text: str


@route("/snapshot/{id}", method=GET)
async def snapshot(id: Path[int], query: Query[Search]) -> Json[Search]:
    return Json(query.value)


@route("/reply", methods=[POST])
async def reply(body: Json[Reply]) -> Result[Json[Reply], str]:
    return Ok(Json(body.value))


@route("/page", method=GET)
async def page() -> Html:
    return Html("<p>ok</p>")
"#;
    check_str(source).map_err(|errors| {
        format!(
            "the documented surface must check, got {:?}",
            errors.iter().map(|error| &error.message).collect::<Vec<_>>()
        )
    })
}

/// A payload returned inside a `Result` is judged like a direct `Json[T]` return, and builtin derives give it no JSON
/// form.
#[test]
fn route_payload_inside_a_result_with_only_builtin_derives_is_refused_issue1768() {
    let source = r#"
import std.async
from std.web import Json, route, GET


@derive(Clone, Eq)
model Reply:
    text: str


@route("/reply", method=GET)
async def reply() -> Result[Json[Reply], str]:
    return Ok(Json(Reply(text="ok")))
"#;
    let errors = check_str_err(source, "a Result of a payload without a JSON form must be refused");
    assert_eq!(
        payload_refusals(&errors),
        vec!["Route handler 'reply' uses 'Json[Reply]', but 'Reply' has no JSON form"]
    );
}

/// A payload built from other types needs each of them to have a JSON form: `Json[list[Search]]` is refused when
/// `Search` has none, naming `Search`, while the same shape over a model that derives `json` is accepted.
#[test]
fn nested_route_payload_without_json_form_is_refused_issue1768() -> Result<(), String> {
    let source = r#"
import std.async
from std.web import Json, route, GET
from std.serde import json


model Search:
    q: str


@derive(json)
model Reply:
    text: str


@route("/searches", method=GET)
async def searches() -> Json[list[Search]]:
    return Json([Search(q="incan")])


@route("/replies", method=GET)
async def replies() -> Json[dict[str, list[Reply]]]:
    return Json({"all": [Reply(text="ok")]})
"#;
    let errors = check_str_err(source, "a nested payload without a JSON form must be refused");
    let refusals = payload_refusals(&errors);
    let [refusal] = refusals.as_slice() else {
        return Err(format!("expected exactly one INCAN-T0112 refusal, got {refusals:?}"));
    };
    assert!(
        refusal.starts_with("Route handler 'searches' uses 'Json[")
            && refusal.ends_with("but 'Search' has no JSON form"),
        "the refusal names the handler, the declared wrapper and the nested model, got {refusal:?}"
    );
    Ok(())
}
