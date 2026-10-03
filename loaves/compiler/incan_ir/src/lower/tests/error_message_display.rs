//! An `Error` adopter with no `__str__` is displayed through its `message()`: an f-string `{value}` part, `str(value)`
//! and `print(value)` hand the emitter the same `message()` call a written `value.message()` lowers to (#1778).

use super::*;
use crate::expr::{BuiltinFn, FormatPart, IrMethodDispatch};

/// A module with one `Error` adopter that displays through `message()` and one that defines `__str__`.
const ERROR_DISPLAY_SOURCE: &str = r#"
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


def written(failure: ParseFailure) -> str:
    return failure.message()


def interpolated(failure: ParseFailure) -> str:
    return f"error {failure}"


def converted(failure: ParseFailure) -> str:
    return str(failure)


def printed(failure: ParseFailure) -> None:
    println(failure)


def debugged(failure: ParseFailure) -> str:
    return f"{failure:?}"


def tagged(failure: TaggedFailure) -> str:
    return f"{failure}"
"#;

/// Return the body of the named function.
fn function_body<'a>(ir: &'a IrProgram, name: &str) -> Result<&'a [IrStmt], String> {
    ir.declarations
        .iter()
        .find_map(|decl| match &decl.kind {
            IrDeclKind::Function(function) if function.name == name => Some(function.body.as_slice()),
            _ => None,
        })
        .ok_or_else(|| format!("missing function `{name}`"))
}

/// Return the expression the named function's last statement returns or evaluates.
fn final_expr<'a>(ir: &'a IrProgram, name: &str) -> Result<&'a TypedExpr, String> {
    match function_body(ir, name)?.last().map(|stmt| &stmt.kind) {
        Some(IrStmtKind::Return(Some(expr)) | IrStmtKind::Expr(expr)) => Ok(expr),
        other => Err(format!("`{name}` must end in one expression, got {other:?}")),
    }
}

/// Return the method a `message()`-shaped call names on the parameter `failure`, or `None` for any other shape.
fn message_call_method(expr: &TypedExpr) -> Option<&str> {
    let IrExprKind::MethodCall {
        receiver, method, args, ..
    } = &expr.kind
    else {
        return None;
    };
    let receives_failure = matches!(&receiver.kind, IrExprKind::Var { name, .. } if name == "failure");
    (receives_failure && args.is_empty() && expr.ty == IrType::String).then_some(method.as_str())
}

/// Return the single interpolated expression of an f-string the named function returns.
fn interpolated_expr<'a>(ir: &'a IrProgram, name: &str) -> Result<&'a TypedExpr, String> {
    let IrExprKind::Format { parts } = &final_expr(ir, name)?.kind else {
        return Err(format!("`{name}` must return an f-string"));
    };
    parts
        .iter()
        .find_map(|part| match part {
            FormatPart::Expr { expr, .. } => Some(expr),
            FormatPart::Literal(_) => None,
        })
        .ok_or_else(|| format!("`{name}` interpolates nothing"))
}

/// Return the single argument of the builtin call the named function ends with.
fn builtin_argument<'a>(ir: &'a IrProgram, name: &str, builtin: BuiltinFn) -> Result<&'a TypedExpr, String> {
    match &final_expr(ir, name)?.kind {
        IrExprKind::BuiltinCall { func, args } if *func == builtin => match args.as_slice() {
            [arg] => Ok(arg),
            other => Err(format!("`{name}` must pass one argument, got {other:?}")),
        },
        other => Err(format!("`{name}` must end in `{builtin:?}`, got {other:?}")),
    }
}

/// #1778: the three display positions of an `Error` adopter with no `__str__` render its `message()`, spelled exactly
/// as the written `failure.message()` call is.
#[test]
fn error_adopter_without_str_displays_through_message_issue1778() -> Result<(), String> {
    let ir = lower_checked_source(ERROR_DISPLAY_SOURCE)?;
    let written = message_call_parts(final_expr(&ir, "written")?)
        .ok_or("the written `failure.message()` must lower to a method call")?;

    let displays = [
        ("interpolated", interpolated_expr(&ir, "interpolated")?),
        ("converted", builtin_argument(&ir, "converted", BuiltinFn::Str)?),
        ("printed", builtin_argument(&ir, "printed", BuiltinFn::Print)?),
    ];
    for (name, operand) in displays {
        let parts = message_call_parts(operand)
            .ok_or_else(|| format!("`{name}` must display `failure` through `message()`, got {operand:?}"))?;
        assert_eq!(
            parts, written,
            "`{name}` must lower to the call a written `failure.message()` lowers to"
        );
    }
    Ok(())
}

/// The standard library's `IoError` adopts `Error` with no `__str__`: its interpolation names the same `message`
/// method a written `error.message()` call names.
#[test]
fn stdlib_io_error_interpolation_names_the_written_message_method_issue1778() -> Result<(), String> {
    let ir = lower_checked_source(
        r#"
from std.io import IoError


def written(failure: IoError) -> str:
    return failure.message()


def interpolated(failure: IoError) -> str:
    return f"error {failure}"
"#,
    )?;
    let written = message_call_parts(final_expr(&ir, "written")?)
        .ok_or("the written `failure.message()` must lower to a method call")?;
    let operand = interpolated_expr(&ir, "interpolated")?;
    let parts = message_call_parts(operand)
        .ok_or_else(|| format!("`interpolated` must display `failure` through `message()`, got {operand:?}"))?;
    assert_eq!(parts, written);
    Ok(())
}

/// Return the method and dispatch of a `message()`-shaped call on the parameter `failure`, or `None` for any other
/// shape.
fn message_call_parts(expr: &TypedExpr) -> Option<(&str, Option<&IrMethodDispatch>)> {
    let method = message_call_method(expr)?;
    let IrExprKind::MethodCall { dispatch, .. } = &expr.kind else {
        return None;
    };
    Some((method, dispatch.as_ref()))
}

/// The builtin spelled through its namespace, `std.builtins.println(failure)` and `std.builtins.str(failure)`,
/// displays an `Error` adopter through `message()` like the bare builtin does.
#[test]
fn namespaced_builtins_display_error_adopters_through_message_issue1778() -> Result<(), String> {
    let ir = lower_checked_source(
        r#"
from std.traits.error import Error


model ParseFailure with Error:
    detail: str

    def message(self) -> str:
        return self.detail


def written(failure: ParseFailure) -> str:
    return failure.message()


def converted(failure: ParseFailure) -> str:
    return std.builtins.str(failure)


def printed(failure: ParseFailure) -> None:
    std.builtins.println(failure)
"#,
    )?;
    let written = message_call_parts(final_expr(&ir, "written")?)
        .ok_or("the written `failure.message()` must lower to a method call")?;
    for (name, builtin) in [("converted", BuiltinFn::Str), ("printed", BuiltinFn::Print)] {
        let operand = builtin_argument(&ir, name, builtin)?;
        let parts = message_call_parts(operand)
            .ok_or_else(|| format!("`{name}` must display `failure` through `message()`, got {operand:?}"))?;
        assert_eq!(parts, written, "`{name}` lowers to the written call");
    }
    Ok(())
}

/// A value of a type parameter bounded by `Error`, and an adopter whose `message` is a trait default, display through
/// the same trait-dispatched call a written `failure.message()` lowers to.
///
/// The trait-default shape itself (`trait AppError with Error` redeclaring `message` with a default) does not build,
/// with or without a display, because its supertrait implementation and inherent projections are emitted twice
/// (#1825); this pins only that the display adds nothing a written call does not.
#[test]
fn generic_and_trait_default_adopters_display_through_trait_dispatch_issue1778() -> Result<(), String> {
    let ir = lower_checked_source(
        r#"
from std.traits.error import Error


trait AppError with Error:
    def message(self) -> str:
        return "application failure"


model Timeout with AppError:
    seconds: int


def timeout_written(failure: Timeout) -> str:
    return failure.message()


def timeout_interpolated(failure: Timeout) -> str:
    return f"{failure}"


def describe_written[E with Error](failure: E) -> str:
    return failure.message()


def describe_interpolated[E with Error](failure: E) -> str:
    return f"{failure}"
"#,
    )?;
    for (written, interpolated) in [
        ("timeout_written", "timeout_interpolated"),
        ("describe_written", "describe_interpolated"),
    ] {
        let expected = message_call_parts(final_expr(&ir, written)?)
            .ok_or_else(|| format!("`{written}` must lower to a `message()` call"))?;
        assert!(
            expected.1.is_some(),
            "`{written}` reaches `message` through a trait dispatch: {expected:?}"
        );
        let operand = interpolated_expr(&ir, interpolated)?;
        let actual = message_call_parts(operand)
            .ok_or_else(|| format!("`{interpolated}` must display through `message()`, got {operand:?}"))?;
        assert_eq!(actual, expected, "`{interpolated}` lowers to the written call");
    }
    Ok(())
}

/// A debug interpolation and a type that defines `__str__` keep their own formatting.
#[test]
fn debug_interpolation_and_str_adopters_keep_their_own_display_issue1778() -> Result<(), String> {
    let ir = lower_checked_source(ERROR_DISPLAY_SOURCE)?;
    for name in ["debugged", "tagged"] {
        let operand = interpolated_expr(&ir, name)?;
        assert!(
            matches!(&operand.kind, IrExprKind::Var { name, .. } if name == "failure"),
            "`{name}` must interpolate `failure` itself, got {operand:?}"
        );
    }
    Ok(())
}

/// Return the method of `trait_name`'s implementation for `type_name`, as expanded into that adopter.
fn trait_impl_method<'a>(
    ir: &'a IrProgram,
    type_name: &str,
    trait_name: &str,
    method: &str,
) -> Result<&'a IrFunction, String> {
    ir.declarations
        .iter()
        .find_map(|decl| match &decl.kind {
            IrDeclKind::Impl(implementation)
                if implementation.target_type == type_name
                    && implementation.trait_name.as_deref() == Some(trait_name) =>
            {
                implementation.methods.iter().find(|function| function.name == method)
            }
            _ => None,
        })
        .ok_or_else(|| format!("`{type_name}` has no `{trait_name}.{method}` implementation"))
}

/// `{self}` in a default method of a trait that extends `Error` is decided per adopter as the default is expanded:
/// an adopter without a `Display` lowers it to exactly the written `self.message()` in the same default, and one with
/// a `__str__` -- its own, or supplied by another adopted trait -- displays itself.
#[test]
fn self_in_an_error_trait_default_displays_per_adopter_issue1778() -> Result<(), String> {
    let ir = lower_checked_source(
        r#"
from std.traits.error import Error


trait Loudness:
    def __str__(self) -> str:
        return "LOUDNESS"


trait Reported with Error:
    def report(self) -> str:
        return f"reported: {self}"

    def written(self) -> str:
        return self.message()


model DiskFull with Reported:
    path: str

    def message(self) -> str:
        return f"disk full: {self.path}"


model Loud with Reported:
    def message(self) -> str:
        return "loud"

    def __str__(self) -> str:
        return "LOUD"


model Shouting with Reported, Loudness:
    def message(self) -> str:
        return "shouting"
"#,
    )?;
    let interpolated = |type_name: &str| -> Result<&TypedExpr, String> {
        let report = trait_impl_method(&ir, type_name, "Reported", "report")?;
        let returned = match report.body.last().map(|stmt| &stmt.kind) {
            Some(IrStmtKind::Return(Some(expr))) => expr,
            other => return Err(format!("`{type_name}.report` must end in a return, got {other:?}")),
        };
        let IrExprKind::Format { parts } = &returned.kind else {
            return Err(format!(
                "`{type_name}.report` must return an f-string, got {returned:?}"
            ));
        };
        parts
            .iter()
            .find_map(|part| match part {
                FormatPart::Expr { expr, .. } => Some(expr),
                FormatPart::Literal(_) => None,
            })
            .ok_or_else(|| format!("`{type_name}.report` interpolates nothing"))
    };
    let written = {
        let written = trait_impl_method(&ir, "DiskFull", "Reported", "written")?;
        match written.body.last().map(|stmt| &stmt.kind) {
            Some(IrStmtKind::Return(Some(expr))) => expr,
            other => return Err(format!("`DiskFull.written` must end in a return, got {other:?}")),
        }
    };
    let disk_full = interpolated("DiskFull")?;
    let self_message_parts = |expr: &'_ TypedExpr| match &expr.kind {
        IrExprKind::MethodCall {
            receiver,
            method,
            dispatch,
            args,
            ..
        } if args.is_empty() && matches!(&receiver.kind, IrExprKind::Var { name, .. } if name == "self") => {
            Some((method.clone(), dispatch.clone()))
        }
        _ => None,
    };
    let expected = self_message_parts(written).ok_or_else(|| {
        format!("the written `self.message()` must lower to a method call on `self`, got {written:?}")
    })?;
    assert_eq!(expected.0, "message");
    assert_eq!(
        self_message_parts(disk_full),
        Some(expected),
        "`DiskFull` has no `Display`, so `{{self}}` lowers to the written `self.message()`, got {disk_full:?}"
    );
    for adopter in ["Loud", "Shouting"] {
        let operand = interpolated(adopter)?;
        assert!(
            matches!(&operand.kind, IrExprKind::Var { name, .. } if name == "self"),
            "`{adopter}` has a `__str__` (its own, or from an adopted trait), so `{{self}}` displays itself, got \
             {operand:?}"
        );
    }
    Ok(())
}
