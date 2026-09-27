//! A stdlib call that omits a parameter whose default names a stdlib const receives that const through its canonical
//! path, not the declaring module's bare spelling (#1771).

use super::*;
use crate::decl::FunctionParamDefault;
use crate::expr::VarRefKind;

/// Return the call the named function returns.
fn returned_call<'a>(ir: &'a IrProgram, name: &str) -> Result<&'a TypedExpr, String> {
    let function = ir
        .declarations
        .iter()
        .find_map(|decl| match &decl.kind {
            IrDeclKind::Function(function) if function.name == name => Some(function),
            _ => None,
        })
        .ok_or_else(|| format!("missing function `{name}`"))?;
    match function.body.last() {
        Some(IrStmt {
            kind: IrStmtKind::Return(Some(expr)),
            ..
        }) => Ok(expr),
        other => Err(format!("`{name}` must end in a return of the call, got {other:?}")),
    }
}

/// Spell a path expression as its segments, or `None` for any other expression.
fn path_segments(expr: &TypedExpr) -> Option<Vec<String>> {
    match &expr.kind {
        IrExprKind::Var {
            name,
            ref_kind: VarRefKind::ExternalName,
            ..
        } => Some(vec![name.clone()]),
        IrExprKind::Field { object, field } => {
            let mut segments = path_segments(object)?;
            segments.push(field.clone());
            Some(segments)
        }
        _ => None,
    }
}

/// Return the source default a signature carries for the named parameter.
fn source_default<'a>(signature: &'a FunctionSignature, name: &str) -> Result<&'a TypedExpr, String> {
    let param = signature
        .params
        .iter()
        .find(|param| param.name == name)
        .ok_or_else(|| format!("the signature must keep `{name}`: {signature:?}"))?;
    match &param.default {
        Some(FunctionParamDefault::Source(default)) => Ok(default.as_ref()),
        other => Err(format!("`{name}` must carry its source default, got {other:?}")),
    }
}

/// #1771: `reader_digest(reader, "sha256")` omits `chunk_size`, whose default `DEFAULT_CHUNK_SIZE` is a const that
/// `std.hash._streaming` imports from `std.hash._core`. The default is expanded at this call site, so it names the
/// const where it is declared, typed as the `int` it is; the literal `length` default stays as written.
#[test]
fn omitted_stdlib_default_names_the_const_through_its_declaring_module_issue1771() -> Result<(), String> {
    let ir = lower_checked_source(
        r#"
from std.io import BytesIO
from std.hash import HashError, reader_digest


def digest() -> Result[bytes, HashError]:
    return reader_digest(BytesIO(b"abc"), "sha256")
"#,
    )?;
    let call = returned_call(&ir, "digest")?;
    let IrExprKind::Call {
        callable_signature: Some(signature),
        ..
    } = &call.kind
    else {
        return Err(format!(
            "`digest` must return a call carrying its stdlib signature, got {call:?}"
        ));
    };
    let chunk_size = source_default(signature, "chunk_size")?;
    assert_eq!(
        path_segments(chunk_size),
        Some(
            ["crate", "__incan_std", "hash", "_core", "DEFAULT_CHUNK_SIZE"]
                .map(str::to_string)
                .to_vec()
        ),
        "the const default is spelled where the const is declared: {chunk_size:?}"
    );
    assert_eq!(chunk_size.ty, IrType::Int, "the const default keeps the const's type");

    let length = source_default(signature, "length")?;
    assert!(
        matches!(length.kind, IrExprKind::Int(0)),
        "a literal default is left as written: {length:?}"
    );
    Ok(())
}
