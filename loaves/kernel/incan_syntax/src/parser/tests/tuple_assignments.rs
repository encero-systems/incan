//! Tuple assignments and tuple unpacking: the value side written as a bare comma-separated tuple (`a, b = b, a`)
//! builds the same tuple as the parenthesized spelling, for plain names, index targets and field targets, except in an
//! item of a braced vocab body, where a comma separates items (#1789).

use super::*;

/// Return the value side of a tuple unpacking or tuple assignment statement.
fn tuple_value<'s>(stmt: &'s Spanned<Statement>, what: &str) -> Result<&'s Spanned<Expr>, Vec<CompileError>> {
    match &stmt.node {
        Statement::TupleUnpack(unpack) => Ok(&unpack.value),
        Statement::TupleAssign(assign) => Ok(&assign.value),
        _ => Err(vec![CompileError::new(
            format!("parser test internal error: expected {what} to be a tuple unpacking or tuple assignment"),
            stmt.span,
        )]),
    }
}

/// Return the elements of a tuple expression.
fn tuple_elements<'e>(expr: &'e Spanned<Expr>, what: &str) -> Result<&'e [Spanned<Expr>], Vec<CompileError>> {
    match &expr.node {
        Expr::Tuple(elements) => Ok(elements.as_slice()),
        _ => Err(vec![CompileError::new(
            format!("parser test internal error: expected {what} to be a tuple"),
            expr.span,
        )]),
    }
}

/// Return the targets of a tuple assignment statement.
fn tuple_targets<'s>(stmt: &'s Spanned<Statement>, what: &str) -> Result<&'s [Spanned<Expr>], Vec<CompileError>> {
    match &stmt.node {
        Statement::TupleAssign(assign) => Ok(assign.targets.as_slice()),
        _ => Err(vec![CompileError::new(
            format!("parser test internal error: expected {what} to be a tuple assignment"),
            stmt.span,
        )]),
    }
}

#[test]
fn test_tuple_unpack_takes_a_bare_tuple_value_like_the_parenthesized_one_issue1789() -> Result<(), Vec<CompileError>> {
    let source = r#"
def swap(a: int, b: int, pair: tuple[int, int]) -> None:
  a, b = b, a
  a, b = (b, a)
  mut low, high = 1, 2
  left, right = pair
"#;
    let program = parse_str(source)?;
    let func = require_function_decl(&program.declarations[0])?;
    assert_eq!(func.body.len(), 4, "each tuple statement stays one statement");

    // ---- `a, b = b, a` builds the same tuple as `a, b = (b, a)` ----
    let bare_value = tuple_value(&func.body[0], "the bare swap")?;
    let bare = tuple_elements(bare_value, "the bare swap value")?;
    let parenthesized = tuple_elements(
        tuple_value(&func.body[1], "the parenthesized swap")?,
        "the parenthesized swap value",
    )?;
    assert!(matches!(bare, [first, second]
        if matches!(&first.node, Expr::Ident(name) if name == "b")
            && matches!(&second.node, Expr::Ident(name) if name == "a")));
    assert!(
        bare.iter()
            .map(|element| &element.node)
            .eq(parenthesized.iter().map(|element| &element.node)),
        "the bare and parenthesized values must hold the same elements: {bare:?} vs {parenthesized:?}"
    );
    assert_eq!(bare_value.span, require_source_span(source, "b, a", 0)?);

    // ---- A declaring unpack takes the bare spelling too ----
    let Statement::TupleUnpack(declared) = &func.body[2].node else {
        return Err(vec![CompileError::new(
            "parser test internal error: expected the declaring unpack".to_string(),
            func.body[2].span,
        )]);
    };
    assert!(matches!(declared.binding, BindingKind::Mutable));
    assert_eq!(declared.names, vec!["low".to_string(), "high".to_string()]);
    let declared_elements = tuple_elements(&declared.value, "the declaring unpack value")?;
    assert_eq!(declared_elements.len(), 2);
    assert!(
        declared_elements
            .iter()
            .all(|element| matches!(element.node, Expr::Literal(Literal::Int(_))))
    );

    // ---- A single value is not wrapped in a one-element tuple ----
    let single = tuple_value(&func.body[3], "the single-value unpack")?;
    assert!(matches!(&single.node, Expr::Ident(name) if name == "pair"));
    Ok(())
}

#[test]
fn test_tuple_assign_takes_a_bare_tuple_value_for_index_and_field_targets_issue1789() -> Result<(), Vec<CompileError>> {
    let source = r#"
def swap_items(arr: list[int], i: int, j: int) -> None:
  arr[i], arr[j] = arr[j], arr[i]
  arr[i], arr[j] = (arr[j], arr[i])

class Grid:
  width: int
  height: int

  def swap(mut self) -> None:
    self.width, self.height = self.height, self.width
    self.width, self.height = (self.height, self.width)
"#;
    let program = parse_str(source)?;

    // ---- Index targets: `arr[i], arr[j] = arr[j], arr[i]` ----
    let func = require_function_decl(&program.declarations[0])?;
    assert_eq!(func.body.len(), 2, "each swap stays one statement");
    for stmt in &func.body {
        let targets = tuple_targets(stmt, "an index swap")?;
        let elements = tuple_elements(tuple_value(stmt, "an index swap")?, "an index swap value")?;
        assert_eq!(targets.len(), 2);
        assert_eq!(elements.len(), 2);
        assert!(
            targets
                .iter()
                .chain(elements)
                .all(|expr| matches!(expr.node, Expr::Index(_, _))),
            "every target and value element is an index expression: {:?}",
            stmt.node
        );
    }
    assert_eq!(
        tuple_value(&func.body[0], "the bare index swap")?.span,
        require_source_span(source, "arr[j], arr[i]", 0)?
    );

    // ---- Field targets: `self.width, self.height = self.height, self.width` ----
    let class = require_class_decl(&program.declarations[1])?;
    let Some(body) = class.methods[0].node.body.as_ref() else {
        return Err(vec![CompileError::new(
            "parser test internal error: expected a concrete method body".to_string(),
            class.methods[0].span,
        )]);
    };
    assert_eq!(body.len(), 2, "each swap stays one statement");
    for stmt in body {
        let targets = tuple_targets(stmt, "a field swap")?;
        let elements = tuple_elements(tuple_value(stmt, "a field swap")?, "a field swap value")?;
        assert!(matches!(targets, [first, second]
            if matches!(&first.node, Expr::Field(_, name) if name == "width")
                && matches!(&second.node, Expr::Field(_, name) if name == "height")));
        assert!(matches!(elements, [first, second]
            if matches!(&first.node, Expr::Field(_, name) if name == "height")
                && matches!(&second.node, Expr::Field(_, name) if name == "width")));
    }
    Ok(())
}

/// Return the items of a braced `query { ... }` assigned by `stmt` that come before its first clause.
fn braced_leading_items(stmt: &Spanned<Statement>) -> Result<&[Spanned<Statement>], String> {
    let Statement::Assignment(assign) = &stmt.node else {
        return Err(format!("expected an assignment, got {:?}", stmt.node));
    };
    let Expr::VocabBlock(block) = &assign.value.node else {
        return Err(format!("expected a braced vocab block, got {:?}", assign.value.node));
    };
    let clauses_start = block
        .body
        .iter()
        .position(|item| matches!(item.node, Statement::VocabBlock(_)))
        .unwrap_or(block.body.len());
    Ok(&block.body[..clauses_start])
}

#[test]
fn test_braced_body_item_takes_no_bare_tuple_value_issue1789() -> Result<(), Box<dyn std::error::Error>> {
    // A comma separates the items of a braced body, so a tuple unpacking there ends at its first value expression:
    // `a, b = pair, c = 3` is two items, and `a, b = x, y` is the unpacking `a, b = x` followed by the item `y`.
    let source = "import pub::analytics\n\ndef configure() -> None:\n  first = query { a, b = pair, c = 3 FROM orders SELECT total }\n  second = query { a, b = x, y FROM orders SELECT total }\n";
    let tokens = crate::lexer::lex(source).map_err(|errs| format!("lex errors: {errs:?}"))?;
    let metadata = incan_vocab::VocabRegistration::new()
        .with_surface(
            incan_vocab::DslSurface::on_import("analytics.query").with_declaration(
                incan_vocab::DeclarationSurface::named("query")
                    .with_clause_body()
                    .desugars_to_expression()
                    .with_clauses([
                        incan_vocab::ClauseSurface::expr("FROM").required(),
                        incan_vocab::ClauseSurface::expr_list("SELECT").required(),
                    ]),
            ),
        )
        .metadata();
    let mut keyword_map = std::collections::HashMap::new();
    keyword_map.insert("analytics".to_string(), metadata.keyword_registrations);
    let mut surface_map = std::collections::HashMap::new();
    surface_map.insert("analytics".to_string(), metadata.dsl_surfaces);
    let program = crate::parser::parse_with_context_and_surfaces(&tokens, None, Some(&keyword_map), Some(&surface_map))
        .map_err(|errs| format!("parse errors: {errs:?}"))?;
    let func = require_function_decl(&program.declarations[1]).map_err(|errs| format!("{errs:?}"))?;

    let first = braced_leading_items(&func.body[0])?;
    assert!(
        matches!(first, [unpack, assign]
            if matches!(&unpack.node, Statement::TupleUnpack(tu)
                if tu.names == ["a", "b"] && matches!(&tu.value.node, Expr::Ident(name) if name == "pair"))
                && matches!(&assign.node, Statement::Assignment(a) if a.name == "c")),
        "expected `a, b = pair` and `c = 3`, got {first:?}"
    );

    let second = braced_leading_items(&func.body[1])?;
    assert!(
        matches!(second, [unpack, item]
            if matches!(&unpack.node, Statement::TupleUnpack(tu)
                if matches!(&tu.value.node, Expr::Ident(name) if name == "x"))
                && matches!(&item.node, Statement::Expr(expr) if matches!(&expr.node, Expr::Ident(name) if name == "y"))),
        "expected `a, b = x` and the item `y`, got {second:?}"
    );
    Ok(())
}
