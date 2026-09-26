//! A trait declared in another source module reaches its adopters with that module's facts: a default body expanded
//! into the adopter names the trait module's own types by their defining path (#1759), and a trait adopted through a
//! derive bundle counts as adopted, so a direct call is routed to the method the derived implementation provides
//! (#1792).

use super::*;

/// Parse one module, keeping fixture failures as ordinary test errors.
fn parse_module(source: &str, context: &str) -> Result<ast::Program, String> {
    let tokens = lexer::lex(source).map_err(|errors| format!("{context} lex failed: {errors:?}"))?;
    parser::parse(&tokens).map_err(|errors| format!("{context} parse failed: {errors:?}"))
}

/// Check `main` against one dependency module and lower it the way multi-module codegen does.
///
/// The dependency is seeded as a source module under `dependency_path`, which is also the Rust module it is emitted
/// as, so the lowered program sees the dependency's traits exactly as a project build does.
fn lower_main_with_dependency(
    main: &str,
    dependency_name: &str,
    dependency_path: &[&str],
    dependency: &str,
) -> Result<IrProgram, String> {
    let dependency = parse_module(dependency, dependency_name)?;
    let program = parse_module(main, "main")?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["main".to_string()]));
    checker
        .check_with_imports(&program, &[(dependency_name, &dependency)])
        .map_err(|errors| format!("main should typecheck: {errors:?}"))?;
    let mut lowering = AstLowering::new_with_type_info(checker.type_info().clone());
    lowering
        .seed_dependency_trait_decls(&[(
            dependency_name,
            &dependency,
            Some(dependency_path.iter().map(|segment| segment.to_string()).collect()),
        )])
        .map_err(|errors| format!("dependency trait seeding failed: {errors:?}"))?;
    lowering
        .lower_program(&program)
        .map_err(|errors| format!("main lowering failed: {errors:?}"))
}

/// Return the method `name` of the impl of `trait_name` for `target`.
fn trait_impl_method<'a>(
    ir: &'a IrProgram,
    target: &str,
    trait_name: &str,
    name: &str,
) -> Result<&'a IrFunction, String> {
    ir.declarations
        .iter()
        .find_map(|decl| match &decl.kind {
            IrDeclKind::Impl(implementation)
                if implementation.target_type == target && implementation.trait_name.as_deref() == Some(trait_name) =>
            {
                implementation.methods.iter().find(|method| method.name == name)
            }
            _ => None,
        })
        .ok_or_else(|| format!("no `{name}` in the impl of `{trait_name}` for `{target}`"))
}

/// Return the name of the struct a function's final `return` constructs.
fn returned_struct_name(function: &IrFunction) -> Result<&str, String> {
    match function.body.last() {
        Some(IrStmt {
            kind:
                IrStmtKind::Return(Some(TypedExpr {
                    kind: IrExprKind::Struct { name, fields, .. },
                    ..
                })),
            ..
        }) if fields.iter().all(|(field, _)| !field.is_empty()) => Ok(name.as_str()),
        other => Err(format!(
            "`{}` must end in a named-field construction, got {other:?}",
            function.name
        )),
    }
}

const SHAPES: &str = r#"
pub model Extent:
    pub width: int
    pub height: int

    def area(self) -> int:
        return self.width * self.height


pub trait Measured:
    def width(self) -> int: ...

    def height(self) -> int: ...

    def extent(self) -> Extent:
        return Extent(width=self.width(), height=self.height())
"#;

/// #1759: an imported trait's default that returns and constructs a model of its own module names that model by
/// the module's path in the adopter, whether the adopter imports only the trait or the model as well. Unqualified,
/// the name was out of scope in the first program and read as a function call in the second.
#[test]
fn imported_trait_default_names_its_module_types_by_path_issue1759() -> Result<(), String> {
    for imports in ["from shapes import Measured", "from shapes import Extent, Measured"] {
        let main = format!(
            "{imports}\n\n\nmodel Card with Measured:\n    size: int\n\n    def width(self) -> int:\n        return self.size\n\n    def height(self) -> int:\n        return self.size + 1\n\n\ndef main() -> None:\n    println(Card(size=3).extent().area())\n"
        );
        let ir = lower_main_with_dependency(&main, "shapes", &["shapes"], SHAPES)?;
        let extent = trait_impl_method(&ir, "Card", "Measured", "extent")?;
        assert_eq!(
            extent.return_type,
            IrType::Struct("crate::shapes::Extent".to_string()),
            "{imports}: the expanded default's return type names the trait module's model"
        );
        assert_eq!(
            returned_struct_name(extent)?,
            "crate::shapes::Extent",
            "{imports}: the expanded default constructs the trait module's model by named fields"
        );
    }
    Ok(())
}

/// #1759: the path follows the module the trait is declared in, including a nested one, and a type the adopter
/// declares under the same name does not replace it inside the expansion.
#[test]
fn imported_trait_default_type_path_follows_the_declaring_module_issue1759() -> Result<(), String> {
    let main = "from shapes import Measured\n\n\nmodel Extent:\n    label: str\n\n\nmodel Card with Measured:\n    size: int\n\n    def width(self) -> int:\n        return self.size\n\n    def height(self) -> int:\n        return self.size\n\n\ndef main() -> None:\n    println(Card(size=2).size)\n";
    let ir = lower_main_with_dependency(main, "shapes", &["geometry", "shapes"], SHAPES)?;
    let extent = trait_impl_method(&ir, "Card", "Measured", "extent")?;
    assert_eq!(
        extent.return_type,
        IrType::Struct("crate::geometry::shapes::Extent".to_string())
    );
    assert_eq!(returned_struct_name(extent)?, "crate::geometry::shapes::Extent");
    Ok(())
}

/// #1759: the type paths are a fact of the trait's module, so a module without traits contributes none and a trait
/// module lists its models, classes, enums and newtypes, and not its traits or transparent aliases.
#[test]
fn source_module_type_paths_list_the_module_nominal_types_issue1759() -> Result<(), String> {
    let module = parse_module(
        "pub model Extent:\n    pub n: int\n\nclass Frame:\n    n: int\n\nenum Corner:\n    Top\n    Bottom\n\ntype Meters = newtype int\n\ntype Alias = int\n\ntrait Shape:\n    def n(self) -> int: ...\n",
        "shapes",
    )?;
    let paths = AstLowering::source_module_type_paths(&module, &["pkg".to_string(), "shapes".to_string()]);
    let mut names = paths.keys().map(String::as_str).collect::<Vec<_>>();
    names.sort_unstable();
    assert_eq!(names, ["Corner", "Extent", "Frame", "Meters"]);
    assert_eq!(
        paths.get("Extent").map(|path| path.join("::")),
        Some("crate::pkg::shapes::Extent".to_string())
    );
    Ok(())
}

const CODEC: &str = r#"
__derives__ = [Encode]


@rust.derive("Debug")
pub trait Encode:
    def tag(self) -> str:
        return "encoded"
"#;

/// Return the method name and dispatch of every method call the lowered `main` makes, in source order.
fn main_method_calls(ir: &IrProgram) -> Result<Vec<(String, Option<IrMethodDispatch>)>, String> {
    let main = ir
        .declarations
        .iter()
        .find_map(|decl| match &decl.kind {
            IrDeclKind::Function(function) if function.name == "main" => Some(function),
            _ => None,
        })
        .ok_or("missing lowered `main`")?;
    let mut calls = Vec::new();
    collect_method_calls(&main.body, &mut calls);
    if calls.is_empty() {
        return Err(format!("`main` makes no method call: {:?}", main.body));
    }
    Ok(calls)
}

/// Collect the method calls made anywhere in a statement list, in source order.
fn collect_method_calls(stmts: &[IrStmt], calls: &mut Vec<(String, Option<IrMethodDispatch>)>) {
    for stmt in stmts {
        match &stmt.kind {
            IrStmtKind::Expr(expr) | IrStmtKind::Return(Some(expr)) => collect_expr_method_calls(expr, calls),
            IrStmtKind::Let { value, .. } => collect_expr_method_calls(value, calls),
            _ => {}
        }
    }
}

/// Collect the method calls inside one expression, receiver first.
fn collect_expr_method_calls(expr: &TypedExpr, calls: &mut Vec<(String, Option<IrMethodDispatch>)>) {
    match &expr.kind {
        IrExprKind::MethodCall {
            receiver,
            method,
            dispatch,
            args,
            ..
        } => {
            collect_expr_method_calls(receiver, calls);
            calls.push((method.clone(), dispatch.clone()));
            for arg in args {
                collect_expr_method_calls(&arg.expr, calls);
            }
        }
        IrExprKind::BuiltinCall { args, .. } => {
            for arg in args {
                collect_expr_method_calls(arg, calls);
            }
        }
        IrExprKind::Call { args, .. } => {
            for arg in args {
                collect_expr_method_calls(&arg.expr, calls);
            }
        }
        IrExprKind::Block { stmts, value } => {
            collect_method_calls(stmts, calls);
            if let Some(value) = value {
                collect_expr_method_calls(value, calls);
            }
        }
        _ => {}
    }
}

/// #1792: a trait adopted through a user module's derive bundle, under the plain and the aliased import binding,
/// counts as adopted by the model, so `item.tag()` is routed to the recoverable method the derived implementation
/// provides instead of a Rust method lookup that needs the trait in scope.
#[test]
fn derive_bundle_trait_counts_as_adopted_for_direct_calls_issue1792() -> Result<(), String> {
    let main = "import codec\nimport codec as formats\n\n\n@derive(codec)\nmodel Item:\n    value: int\n\n\n@derive(formats)\nmodel Other:\n    value: int\n\n\ndef main() -> None:\n    item = Item(value=1)\n    println(item.tag())\n    other = Other(value=2)\n    println(other.tag())\n";
    let ir = lower_main_with_dependency(main, "codec", &["codec"], CODEC)?;
    let calls = main_method_calls(&ir)?;
    assert_eq!(calls.len(), 2, "expected the two `tag()` calls, got {calls:?}");
    for (method, dispatch) in &calls {
        assert!(
            matches!(dispatch, Some(IrMethodDispatch::SourceProjection(_))),
            "a derived trait's method must be routed to its recoverable projection, got `{method}` with {dispatch:?}"
        );
        assert_ne!(method, "tag", "the call names the projection, not the trait slot");
    }
    Ok(())
}
