//! The `std.web` surface types lower to the Axum shapes they re-export (#1768): `Json[T].value` and `Query[T].value`
//! read the extractor's tuple field `0` as `T`, and the `Html` response is `Html<String>`.

use super::*;

/// A module using the documented `std.web` surface: a `Query` payload read through `.value`, and `Html` as the
/// declared return type of a route handler and of an ordinary function.
const WEB_SURFACE_SOURCE: &str = r#"
import std.async
from std.web import Json, Html, Query, Path, route, GET
from std.serde import json


@derive(json)
model Search:
    q: str


@route("/snapshot/{id}", method=GET)
async def snapshot(id: Path[int], query: Query[Search]) -> Json[Search]:
    return Json(query.value)


@route("/page", method=GET)
async def page() -> Html:
    return Html("<p>ok</p>")


def render_html(value: str) -> Html:
    return Html(value)
"#;

/// Return the named function's declaration.
fn function<'a>(ir: &'a IrProgram, name: &str) -> Result<&'a IrFunction, String> {
    ir.declarations
        .iter()
        .find_map(|decl| match &decl.kind {
            IrDeclKind::Function(function) if function.name == name => Some(function),
            _ => None,
        })
        .ok_or_else(|| format!("missing function `{name}`"))
}

/// Return the first field read inside an expression, looking through calls and the wrappers lowering adds.
fn first_field_read(expr: &TypedExpr) -> Option<&TypedExpr> {
    match &expr.kind {
        IrExprKind::Field { .. } => Some(expr),
        IrExprKind::Call { func, args, .. } => {
            first_field_read(func).or_else(|| args.iter().find_map(|arg| first_field_read(&arg.expr)))
        }
        IrExprKind::MethodCall { receiver, args, .. } => {
            first_field_read(receiver).or_else(|| args.iter().find_map(|arg| first_field_read(&arg.expr)))
        }
        IrExprKind::InteropCoerce { expr, .. } => first_field_read(expr),
        IrExprKind::Block { value: Some(value), .. } => first_field_read(value),
        _ => None,
    }
}

/// #1768: `query.value` on a `Query[Search]` parameter reads the extractor's tuple field `0`, typed as the payload.
#[test]
fn web_extractor_value_reads_the_payload_field_issue1768() -> Result<(), String> {
    let ir = lower_checked_source(WEB_SURFACE_SOURCE)?;
    let returned = match function(&ir, "snapshot")?.body.last().map(|stmt| &stmt.kind) {
        Some(IrStmtKind::Return(Some(expr))) => expr,
        other => return Err(format!("`snapshot` must end in a return, got {other:?}")),
    };
    let read = first_field_read(returned).ok_or_else(|| format!("`snapshot` reads no field: {returned:?}"))?;
    let IrExprKind::Field { object, field } = &read.kind else {
        return Err(format!("expected a field read, got {read:?}"));
    };
    assert!(
        matches!(&object.kind, IrExprKind::Var { name, .. } if name == "query"),
        "the payload is read from the `query` parameter, got {object:?}"
    );
    assert_eq!(field, "0", "the Axum extractor keeps its payload in tuple field `0`");
    assert_eq!(
        read.ty,
        IrType::Struct("Search".to_string()),
        "the read is typed as the payload"
    );
    Ok(())
}

/// #1768: the `Html` response is generic over its body in Rust, so a route handler's and an ordinary function's
/// declared `Html` return type lowers to `Html<String>`.
#[test]
fn std_web_html_return_type_lowers_with_its_text_body_issue1768() -> Result<(), String> {
    let ir = lower_checked_source(WEB_SURFACE_SOURCE)?;
    let html = IrType::NamedGeneric("Html".to_string(), vec![IrType::String]);
    for name in ["page", "render_html"] {
        let function = function(&ir, name)?;
        assert_eq!(function.return_type, html, "`{name}` must return `Html<String>`");
        let returned = match function.body.last().map(|stmt| &stmt.kind) {
            Some(IrStmtKind::Return(Some(expr))) => expr,
            other => return Err(format!("`{name}` must end in a return, got {other:?}")),
        };
        let IrExprKind::Struct {
            name: constructed,
            fields,
            ..
        } = &returned.kind
        else {
            return Err(format!(
                "`{name}` must return an `Html(...)` construction, got {returned:?}"
            ));
        };
        assert_eq!(constructed, "Html");
        assert!(
            matches!(fields.as_slice(), [(field, body)] if field.is_empty() && body.ty == IrType::String),
            "`{name}`'s `Html(...)` wraps its text positionally, got {fields:?}"
        );
        assert_eq!(
            returned.ty, html,
            "`{name}`'s `Html(...)` construction is typed as the text response"
        );
    }
    Ok(())
}

/// A program's own type named `Html` keeps its own spelling.
#[test]
fn a_local_type_named_html_is_not_the_web_response_issue1768() -> Result<(), String> {
    let ir = lower_checked_source(
        r#"
model Html:
    body: str


def render(value: str) -> Html:
    return Html(body=value)
"#,
    )?;
    assert_eq!(function(&ir, "render")?.return_type, IrType::Struct("Html".to_string()));
    Ok(())
}

/// `Html` imported under another name is still the text response, spelled under the local name.
#[test]
fn aliased_std_web_html_lowers_with_its_text_body_issue1768() -> Result<(), String> {
    let ir = lower_checked_source(
        r#"
from std.web import Html as Page


def render(value: str) -> Page:
    return Page(value)
"#,
    )?;
    assert_eq!(
        function(&ir, "render")?.return_type,
        IrType::NamedGeneric("Page".to_string(), vec![IrType::String])
    );
    Ok(())
}

/// Known limitation (#1824): a module-qualified `web.Html` never reaches lowering. `std.web` provides `Html` as a
/// compiler surface type rather than a declaration, so the checker refuses the spelling, naming that limitation,
/// instead of letting a bare `Html` reach the build.
#[test]
fn qualified_std_web_html_is_a_known_checker_refusal_issue1824() -> Result<(), String> {
    for source in [
        "import std.web as web\nfrom std.web import Html\n\n\ndef render(value: str) -> web.Html:\n    return Html(value)\n",
        "from std import web\nfrom std.web import Html\n\n\ndef render(value: str) -> web.Html:\n    return Html(value)\n",
    ] {
        match lower_checked_source(source) {
            Err(message)
                if message.contains("`web.Html` cannot be written through its module")
                    && message.contains("provides `Html` as a built-in type") => {}
            other => return Err(format!("`web.Html` must be refused as a surface type, got {other:?}")),
        }
    }
    Ok(())
}

/// A `Json` the program declares after using it keeps its own `value` field.
///
/// The checker does not resolve that forward spelling in a parameter annotation, so the program never reaches the
/// build; lowering is still pinned on the facts the check produced, because its guard is the `std.web` binding, not
/// declaration order.
#[test]
fn a_local_json_declared_after_use_keeps_its_value_field_issue1768() -> Result<(), String> {
    let ir = lower_source(
        r#"
def read(wrapper: Json[int]) -> int:
    return wrapper.value


model Json[T]:
    value: T
"#,
    )
    .map_err(|errors| format!("lowering failed: {errors:?}"))?;
    let returned = match function(&ir, "read")?.body.last().map(|stmt| &stmt.kind) {
        Some(IrStmtKind::Return(Some(expr))) => expr,
        other => return Err(format!("`read` must end in a return, got {other:?}")),
    };
    let read = first_field_read(returned).ok_or_else(|| format!("`read` reads no field: {returned:?}"))?;
    assert!(
        matches!(&read.kind, IrExprKind::Field { field, .. } if field == "value"),
        "a local `Json` keeps its `value` field, got {read:?}"
    );
    Ok(())
}

/// A `Json` imported from another module keeps its own `value` field.
#[test]
fn an_imported_json_keeps_its_value_field_issue1768() -> Result<(), String> {
    let parse = |source: &str| -> Result<ast::Program, String> {
        let tokens = lexer::lex(source).map_err(|errors| format!("lexer failed: {errors:?}"))?;
        parser::parse(&tokens).map_err(|errors| format!("parser failed: {errors:?}"))
    };
    let models = parse("pub model Json[T]:\n    value: T\n")?;
    let main = parse("from models import Json\n\n\ndef read(wrapper: Json[int]) -> int:\n    return wrapper.value\n")?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["main".to_string()]));
    checker
        .check_with_imports(&main, &[("models", &models)])
        .map_err(|errors| format!("the consumer must check: {errors:?}"))?;
    let mut lowering = AstLowering::new_with_type_info(checker.type_info().clone());
    let ir = lowering
        .lower_program(&main)
        .map_err(|errors| format!("lowering failed: {errors:?}"))?;
    let returned = match function(&ir, "read")?.body.last().map(|stmt| &stmt.kind) {
        Some(IrStmtKind::Return(Some(expr))) => expr,
        other => return Err(format!("`read` must end in a return, got {other:?}")),
    };
    let read = first_field_read(returned).ok_or_else(|| format!("`read` reads no field: {returned:?}"))?;
    assert!(
        matches!(&read.kind, IrExprKind::Field { field, .. } if field == "value"),
        "an imported `Json` keeps its `value` field, got {read:?}"
    );
    Ok(())
}
