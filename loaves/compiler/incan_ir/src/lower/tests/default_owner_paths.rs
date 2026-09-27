//! A const or function a parameter default names is spelled through the module that declares it, because a caller in
//! another module receives the default at its own call site (#1771).

use super::*;
use crate::decl::FunctionParamDefault;
use crate::expr::VarRefKind;
use incan_semantics_core::SymbolOrigin;

/// Source whose defaults and method-partial preset name the module's own consts and function.
const MODULE_SOURCE: &str = r#"
const SIZE: int = 2
const PREFIX: str = "size"


def _unit() -> str:
    return "cm"


pub def measure(n: int = SIZE, unit: str = _unit()) -> str:
    return f"{n}{unit}"


pub def doubled() -> int:
    return SIZE * 2


pub model Ruler:
    pub length: int

    def label(self, prefix: str) -> str:
        return prefix

    short = partial label(prefix=PREFIX)
"#;

/// Check and lower `source` as the module whose declarations the crate places at `rust_path` below its root.
fn lower_module_at(source: &str, rust_path: &[&str]) -> Result<IrProgram, String> {
    let tokens = lexer::lex(source).map_err(|errors| format!("lexer failed: {errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("parser failed: {errors:?}"))?;
    let mut checker = TypeChecker::new();
    checker
        .check_program(&program)
        .map_err(|errors| format!("typechecker failed: {errors:?}"))?;
    let mut lowering = AstLowering::new_with_type_info(checker.type_info().clone());
    // A checker without a module path gives its declarations the anonymous module's origin.
    lowering.set_source_module_rust_paths(HashMap::from([(
        SymbolOrigin::Module(Vec::new()),
        rust_path.iter().map(ToString::to_string).collect(),
    )]));
    lowering
        .lower_program(&program)
        .map_err(|errors| format!("lowering failed: {errors:?}"))
}

/// Return the source default of parameter `param` of the function or method named `callable`.
fn param_default<'a>(ir: &'a IrProgram, callable: &str, param: &str) -> Result<&'a TypedExpr, String> {
    let function = ir
        .declarations
        .iter()
        .flat_map(|decl| match &decl.kind {
            IrDeclKind::Function(function) => vec![function],
            IrDeclKind::Impl(impl_decl) => impl_decl.methods.iter().collect(),
            _ => Vec::new(),
        })
        .find(|function| function.name == callable)
        .ok_or_else(|| format!("missing callable `{callable}`"))?;
    match function
        .params
        .iter()
        .find(|candidate| candidate.name == param)
        .map(|candidate| &candidate.default)
    {
        Some(Some(FunctionParamDefault::Source(default))) => Ok(default.as_ref()),
        other => Err(format!(
            "`{callable}({param})` must carry a source default, got {other:?}"
        )),
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

/// Return the canonical callee path of a call expression.
fn call_path(expr: &TypedExpr) -> Result<Option<&[String]>, String> {
    match &expr.kind {
        IrExprKind::Call { canonical_path, .. } => Ok(canonical_path.as_deref()),
        other => Err(format!("expected a call, got {other:?}")),
    }
}

/// Every const a default or a method-partial preset reads is spelled as a crate path to the declaring module, typed
/// as the const is, and a function a default calls carries that module's path as its canonical callee path. The same
/// const read in a function body keeps its own spelling: only a default reaches another module's call sites.
#[test]
fn module_items_a_default_names_are_spelled_through_their_module_issue1771() -> Result<(), String> {
    let ir = lower_module_at(MODULE_SOURCE, &["shop", "ruler"])?;
    let size = param_default(&ir, "measure", "n")?;
    assert_eq!(
        path_segments(size),
        Some(
            vec!["crate", "shop", "ruler", "SIZE"]
                .into_iter()
                .map(String::from)
                .collect()
        ),
        "`SIZE` is reached through the module that declares it: {size:?}"
    );
    assert_eq!(size.ty, IrType::Int, "the path keeps the const's type");
    assert_eq!(
        call_path(param_default(&ir, "measure", "unit")?)?,
        Some(["shop".to_string(), "ruler".to_string(), "_unit".to_string()].as_slice()),
        "`_unit()` is called through the module that declares it"
    );
    assert_eq!(
        path_segments(param_default(&ir, "short", "prefix")?),
        Some(
            vec!["crate", "shop", "ruler", "PREFIX"]
                .into_iter()
                .map(String::from)
                .collect()
        ),
        "a method partial's preset is a default of the method it generates"
    );
    let doubled = ir
        .declarations
        .iter()
        .find_map(|decl| match &decl.kind {
            IrDeclKind::Function(function) if function.name == "doubled" => Some(function),
            _ => None,
        })
        .ok_or("missing function `doubled`")?;
    let Some(IrStmt {
        kind: IrStmtKind::Return(Some(returned)),
        ..
    }) = doubled.body.last()
    else {
        return Err(format!("`doubled` must end in a return, got {:?}", doubled.body));
    };
    assert!(
        matches!(&returned.kind, IrExprKind::BinOp { left, .. }
            if matches!(&left.kind, IrExprKind::Var { name, .. } if name == "SIZE")),
        "a function body reads `SIZE` by its own name: {returned:?}"
    );
    Ok(())
}

/// The crate-root module's const is reached through the root's empty path; its function keeps the callee as written,
/// since a canonical callee path spells only a module below the root. A module the crate does not map keeps every name
/// as written.
#[test]
fn crate_root_and_unmapped_module_defaults_issue1771() -> Result<(), String> {
    let root = lower_module_at(MODULE_SOURCE, &[])?;
    assert_eq!(
        path_segments(param_default(&root, "measure", "n")?),
        Some(vec!["crate".to_string(), "SIZE".to_string()]),
        "the root's const is reached from the crate root"
    );
    assert_eq!(
        call_path(param_default(&root, "measure", "unit")?)?,
        None,
        "the root's function keeps its callee"
    );

    let unmapped = lower_checked_source(MODULE_SOURCE)?;
    let size = param_default(&unmapped, "measure", "n")?;
    assert!(
        matches!(&size.kind, IrExprKind::Var { name, .. } if name == "SIZE"),
        "an unmapped module keeps `SIZE` as written: {size:?}"
    );
    Ok(())
}
