//! Expression and call formatting: fallible adapter chains, augmented assignment, rest parameters and call unpacking,
//! collection literal spreads, leading-dot fluent chains and the comments inside them, expression vocab blocks,
//! generator and comprehension clause shapes, RFC 028 operator spellings, untyped closure parameters, `race_for` and
//! `loop` values, union annotations, f-string escapes and debug markers, numeric literal spellings, and long
//! constructor calls.

use super::*;

fn format_source_with_query_vocab(source: &str) -> Result<String, FormatError> {
    let tokens = lexer::lex(source).map_err(|errs| {
        FormatError::SyntaxError(errs.iter().map(|e| e.message.clone()).collect::<Vec<_>>().join("\n"))
    })?;
    let metadata = incan_vocab::VocabRegistration::new()
        .with_surface(
            incan_vocab::DslSurface::on_import("analytics.query")
                .with_declaration(
                    incan_vocab::DeclarationSurface::named("query")
                        .with_clause_body()
                        .desugars_to_expression()
                        .with_clauses([
                            incan_vocab::ClauseSurface::expr("FROM").required(),
                            incan_vocab::ClauseSurface::expr_list("SELECT").required(),
                            incan_vocab::ClauseSurface::expr("WHERE").optional(),
                            incan_vocab::ClauseSurface::expr_list("GROUP BY").optional(),
                            incan_vocab::ClauseSurface::expr("ORDER BY").optional(),
                            incan_vocab::ClauseSurface::expr("LIMIT").optional(),
                        ]),
                )
                .with_scoped_surface(
                    incan_vocab::ScopedSurfaceDescriptor::leading_dot_path("query.field")
                        .in_declaration_body("query")
                        .with_receiver(incan_vocab::ScopedSurfaceReceiver::OwningDeclaration),
                )
                .with_scoped_surface(
                    incan_vocab::ScopedSurfaceDescriptor::leading_dot_path("query.select.field")
                        .in_clause_body("query", "SELECT")
                        .with_receiver(incan_vocab::ScopedSurfaceReceiver::clause("FROM")),
                )
                .with_scoped_surface(
                    incan_vocab::ScopedSurfaceDescriptor::leading_dot_path("query.group.field")
                        .in_clause_body("query", "GROUP")
                        .with_receiver(incan_vocab::ScopedSurfaceReceiver::clause("FROM")),
                )
                .with_scoped_surface(
                    incan_vocab::ScopedSurfaceDescriptor::leading_dot_path("query.where.field")
                        .in_clause_body("query", "WHERE")
                        .with_receiver(incan_vocab::ScopedSurfaceReceiver::clause("FROM")),
                )
                .with_scoped_surface(
                    incan_vocab::ScopedSurfaceDescriptor::leading_dot_path("query.order.field")
                        .in_clause_body("query", "ORDER")
                        .with_receiver(incan_vocab::ScopedSurfaceReceiver::clause("FROM")),
                ),
        )
        .metadata();
    let mut keyword_map = std::collections::HashMap::new();
    keyword_map.insert("analytics".to_string(), metadata.keyword_registrations);
    let mut surface_map = std::collections::HashMap::new();
    surface_map.insert("analytics".to_string(), metadata.dsl_surfaces);
    let ast = parser::parse_with_context_and_surfaces(&tokens, None, Some(&keyword_map), Some(&surface_map)).map_err(
        |errs| {
            let mut msg = String::new();
            for err in &errs {
                msg.push_str(&diagnostics::format_error("<input>", source, err));
            }
            FormatError::SyntaxError(msg)
        },
    )?;

    format_parsed_source_with_config(source, &ast, FormatConfig::default())
}

fn assert_comment_line_immediately_before(
    lines: &[&str],
    stmt_label: &str,
    line_matches: impl Fn(&str) -> bool,
    comment_needle: &str,
) -> Result<(), FormatError> {
    let idx = lines.iter().position(|&l| line_matches(l)).ok_or_else(|| {
        FormatError::SyntaxError(format!(
            "missing formatted line for {stmt_label} (comment-anchoring regression)"
        ))
    })?;
    assert!(
        idx > 0 && lines[idx - 1].contains(comment_needle),
        "expected comment {comment_needle:?} on the line immediately before {stmt_label}; lines={lines:?}"
    );
    Ok(())
}

#[test]
fn test_format_source_simple_function() {
    let source = r#"def foo() -> int:
  return 42
"#;
    let result = format_source(source);
    assert!(result.is_ok());
}

#[test]
fn test_format_source_preserves_fallible_loop_adapter_chain() -> Result<(), FormatError> {
    let source = r#"def consume(stream: FallibleIterator[int, str]) -> Result[None, str]:
    for value in stream.map(double).map_err(normalize_error)?:
        println(value)
    return Ok(None)
"#;

    assert_eq!(format_source(source)?, source);
    Ok(())
}

#[test]
fn test_format_source_preserves_field_and_index_augmented_assignment() -> Result<(), FormatError> {
    let source = r#"model Counter:
    value: int

    def increment(mut self) -> None:
        self.value += 1


def increment_first(mut values: list[int]) -> None:
    values[0] += 1
"#;

    assert_eq!(format_source(source)?, source);
    Ok(())
}

#[test]
fn test_format_source_rest_params_and_call_unpacking() -> Result<(), FormatError> {
    let source = r#"def collect(prefix: str, *items: int, **labels: str) -> int:
  return collect(prefix,*items,**labels)
"#;
    let formatted = format_source(source)?;
    assert_eq!(
        formatted,
        r#"def collect(prefix: str, *items: int, **labels: str) -> int:
    return collect(prefix, *items, **labels)
"#
    );
    Ok(())
}

#[test]
fn test_format_source_collection_literal_spread_entries() -> Result<(), FormatError> {
    let source = r#"def f(xs: list[int], headers: dict[str, str]) -> None:
  values=[1,*xs,4]
  merged={"accept":"json",**headers}
"#;
    let formatted = format_source(source)?;
    assert_eq!(
        formatted,
        r#"def f(xs: list[int], headers: dict[str, str]) -> None:
    values = [1, *xs, 4]
    merged = {"accept": "json", **headers}
"#
    );
    Ok(())
}

#[test]
fn test_format_source_accepts_leading_dot_fluent_chain() -> Result<(), FormatError> {
    let source = r#"def high_value_orders(orders: DataFrame) -> DataFrame:
    enriched = orders
        .with_column("region_norm", upper(trim(col("region"))))
        .with_column("status_looks_clean",regexp_like(col("status_norm"), "^[a-z]+$"))
    return enriched
"#;
    let expected = r#"def high_value_orders(orders: DataFrame) -> DataFrame:
    enriched = orders
        .with_column("region_norm", upper(trim(col("region"))))
        .with_column("status_looks_clean", regexp_like(col("status_norm"), "^[a-z]+$"))
    return enriched
"#;
    assert_eq!(format_source(source)?, expected);
    assert_eq!(format_source(expected)?, expected);
    Ok(())
}

#[test]
fn test_format_source_keeps_comments_inside_leading_dot_fluent_chain() -> Result<(), FormatError> {
    let source = r#"def high_value_orders(orders: DataFrame) -> DataFrame:
    enriched = orders
        # a valid placement for a comment
        .with_column("region_norm", upper(trim(col("region"))))
        .with_column("status_looks_clean",regexp_like(col("status_norm"), "^[a-z]+$"))
    return enriched
"#;
    let expected = r#"def high_value_orders(orders: DataFrame) -> DataFrame:
    enriched = orders
        # a valid placement for a comment
        .with_column("region_norm", upper(trim(col("region"))))
        .with_column("status_looks_clean", regexp_like(col("status_norm"), "^[a-z]+$"))
    return enriched
"#;
    assert_eq!(format_source(source)?, expected);
    assert_eq!(format_source(expected)?, expected);
    Ok(())
}

#[test]
fn test_format_source_wraps_long_method_chain_as_leading_dot_fluent_chain() -> Result<(), FormatError> {
    let source = r#"def high_value_orders(orders: DataFrame) -> DataFrame:
    enriched = orders.with_column("region_norm", upper(trim(col("region")))).with_column("status_norm", lower(trim(col("status")))).with_column("gross_amount", round(mul(col("quantity"), col("unit_price")), 2))
    return enriched
"#;
    let expected = r#"def high_value_orders(orders: DataFrame) -> DataFrame:
    enriched = orders
        .with_column("region_norm", upper(trim(col("region"))))
        .with_column("status_norm", lower(trim(col("status"))))
        .with_column("gross_amount", round(mul(col("quantity"), col("unit_price")), 2))
    return enriched
"#;
    let config = FormatConfig::new().with_line_length(120);
    assert_eq!(format_source_with_config(source, config.clone())?, expected);
    assert_eq!(format_source_with_config(expected, config)?, expected);
    Ok(())
}

#[test]
fn test_format_source_keeps_phase_comments_before_wrapped_fluent_chain() -> Result<(), FormatError> {
    let source = r#"def test_case() -> None:
    # -- Arrange --
    base = Frame.new()

    # -- Act --
    projected: Frame = base.with_column("double_id", mul(col("id"), 2)).with_column(
        "triple_id",
        mul(col("id"), 3),
    )
    output_cols = projected.columns()

    # -- Assert --
    assert output_cols == ["id", "double_id", "triple_id"], "derived columns should preserve append order"
"#;
    let config = FormatConfig::new().with_line_length(88);
    let formatted = format_source_with_config(source, config.clone())?;
    assert!(
        formatted.contains("    # -- Arrange --\n    base = Frame.new()"),
        "expected arrange comment to stay attached to setup; got:\n{formatted}"
    );
    assert!(
        formatted.contains(
            "    # -- Act --\n    projected: Frame = base\n        .with_column(\"double_id\", mul(col(\"id\"), 2))\n        .with_column(\"triple_id\", mul(col(\"id\"), 3))"
        ),
        "expected act comment to stay attached to wrapped fluent chain; got:\n{formatted}"
    );
    assert!(
        formatted.contains("    # -- Assert --\n    assert output_cols =="),
        "expected assert comment to stay attached to assertion; got:\n{formatted}"
    );
    assert!(
        !formatted.contains("    # -- Act --\n\n    # -- Assert --"),
        "formatter floated phase comments away from their anchors; got:\n{formatted}"
    );

    assert_eq!(format_source_with_config(&formatted, config)?, formatted);
    Ok(())
}

#[test]
fn test_format_source_preserves_expression_vocab_brace_shape_and_comments() -> Result<(), FormatError> {
    let source = r#"import pub::analytics

def high_value_orders(orders: DataFrame) -> DataFrame:
    paid = orders
    # The query block can reference ordinary local Incan values, so `paid` stays a normal lazy frame binding.
    return query {
        # source table
        FROM paid
        SELECT
            .order_id as order_id
            # customer projection
            .customer_id as customer_id
        GROUP BY
            .region_norm,
            .channel
        WHERE .net_amount > 100
        ORDER BY desc(.net_amount)
        LIMIT 8
    }
"#;
    let formatted = format_source_with_query_vocab(source)?;

    assert!(
        formatted.contains("    return query {\n"),
        "expected expression vocab block to keep brace form; got:\n{formatted}"
    );
    assert!(
        !formatted.contains("return query:") && !formatted.contains("FROM:") && !formatted.contains("ORDER BY:"),
        "formatter must not rewrite expression vocab blocks through colon form; got:\n{formatted}"
    );
    assert!(
        formatted.contains("        FROM paid\n"),
        "expected single-expression clauses to stay no-colon and inline; got:\n{formatted}"
    );
    assert!(
        formatted.contains("        SELECT\n            .order_id as order_id\n            # customer projection\n            .customer_id as customer_id\n"),
        "expected SELECT body and nested comment to stay attached; got:\n{formatted}"
    );
    assert!(
        formatted.contains("        GROUP BY\n            .region_norm,\n            .channel\n"),
        "expected expression-list comma shape to be preserved from source; got:\n{formatted}"
    );
    assert!(
        formatted.contains("    # The query block can reference ordinary local Incan values"),
        "expected leading query comment to stay attached before the block; got:\n{formatted}"
    );

    assert_eq!(format_source_with_query_vocab(&formatted)?, formatted);
    Ok(())
}

#[test]
fn test_format_source_generator_expression_full_clause_shape() -> Result<(), FormatError> {
    let source = r#"def run(xs: list[int], ys: list[int]) -> Generator[int]:
  return (x*y for x in xs if x>0 for y in ys if y>x)
"#;
    let expected = r#"def run(xs: list[int], ys: list[int]) -> Generator[int]:
    return (x * y for x in xs if x > 0 for y in ys if y > x)
"#;
    let formatted = format_source(source)?;
    assert_eq!(formatted, expected);
    assert_eq!(format_source(&formatted)?, expected);
    Ok(())
}

#[test]
fn test_format_source_list_comprehension_tuple_target_omits_parentheses() -> Result<(), FormatError> {
    let source = r#"def labels(values: list[str]) -> list[str]:
  return [f"{idx}:{value}" for idx, value in enumerate(values)]
"#;
    let expected = r#"def labels(values: list[str]) -> list[str]:
    return [f"{idx}:{value}" for idx, value in enumerate(values)]
"#;
    let formatted = format_source(source)?;
    assert_eq!(formatted, expected);
    assert_eq!(format_source(&formatted)?, expected);
    Ok(())
}

#[test]
fn test_format_source_rfc028_operator_spellings() -> Result<(), FormatError> {
    let source = r#"def ops(a: Any, b: Any, c: Any) -> None:
  mat=a@b
  piped=a|>b<|c
  bits=~a&b|c^a<<b>>c
  a @= b
  a &= b
  a |= b
  a ^= b
  a <<= b
  a >>= b
"#;
    let formatted = format_source(source)?;
    assert_eq!(
        formatted,
        r#"def ops(a: Any, b: Any, c: Any) -> None:
    mat = a @ b
    piped = a |> b <| c
    bits = ~a & b | c ^ a << b >> c
    a @= b
    a &= b
    a |= b
    a ^= b
    a <<= b
    a >>= b
"#
    );
    Ok(())
}

#[test]
fn test_format_source_preserves_untyped_closure_params() -> Result<(), FormatError> {
    let source = r#"pub def registered[F](_function_ref: str) -> (F) -> F:
    return (func) => func
"#;
    let formatted = format_source(source)?;
    let expected = r#"pub def registered[F](_function_ref: str) -> (F) -> F:
    return (func) => func
"#;
    assert_eq!(formatted, expected);
    Ok(())
}

#[test]
fn test_format_source_race_for_expression_round_trip() -> Result<(), FormatError> {
    let source = r#"import std.async

async def run() -> int:
    return race for value:
        await fast() => value
        await slow() =>
            return value
"#;
    let formatted = assert_format_round_trip_lex_parse(source)?;
    assert!(
        formatted.contains("race for value:"),
        "expected formatter to preserve race header; got: {formatted}"
    );
    assert!(
        formatted.contains("await fast() => value"),
        "expected formatter to preserve inline race arm; got: {formatted}"
    );
    assert!(
        formatted.contains("await slow() =>\n            return value"),
        "expected formatter to preserve block race arm; got: {formatted}"
    );
    Ok(())
}

#[test]
fn test_format_source_union_annotations_round_trip_as_canonical_generic() -> Result<(), FormatError> {
    let source = r#"def parse(values: List[int | str], maybe: int | str | None) -> int | str:
    return values[0]
"#;
    let formatted = assert_format_round_trip_lex_parse(source)?;
    assert!(
        formatted.contains("values: List[Union[int, str]]"),
        "expected nested pipe annotation to format as canonical Union generic; got: {formatted}"
    );
    assert!(
        formatted.contains("maybe: Union[int, str, None]"),
        "expected None-containing pipe annotation to remain parseable in formatted AST form; got: {formatted}"
    );
    assert!(
        formatted.contains("-> Union[int, str]:"),
        "expected return pipe annotation to format as canonical Union generic; got: {formatted}"
    );
    Ok(())
}

/// Regression (GitHub #289): escaped newlines inside f-strings must stay textual (`\\n`) after formatting.
#[test]
fn test_format_source_preserves_fstring_escaped_newline() -> Result<(), FormatError> {
    let source = "def main() -> str:\n    return f\"a\\n{1}\"\n";
    let formatted = assert_format_round_trip_lex_parse(source)?;
    assert!(
        formatted.contains(r#"f"a\n{1}""#),
        "expected formatter to preserve escaped newline text in f-string, got: {formatted}"
    );
    assert!(
        !formatted.contains("f\"a\n{1}\""),
        "formatter must not materialize a physical newline in f-string output, got: {formatted}"
    );
    Ok(())
}

/// Regression (GitHub #625): f-string debug markers are semantic and must survive formatting.
#[test]
fn test_format_source_preserves_fstring_debug_marker() -> Result<(), FormatError> {
    let source = "def main(columns: list[str]) -> str:\n    return f\"columns: {columns:?}\"\n";
    let formatted = assert_format_round_trip_lex_parse(source)?;
    assert!(
        formatted.contains(r#"f"columns: {columns:?}""#),
        "expected formatter to preserve f-string debug marker, got: {formatted}"
    );
    Ok(())
}

/// Regression (GitHub #250): `f64::Display` drops `.0` for whole numbers, which broke
/// `normalize_code_for_match` anchors in [`reattach_comments`] and flushed standalone `#` lines to EOF.
///
/// Covers `120.0`, distinct `1E6` / `1e6` exponents, `1_000.0`, and underscored int `1_000`, each with a standalone
/// comment on the line above. See also [`test_format_source_preserves_numeric_literal_source_substring`].
#[test]
fn test_format_source_preserves_float_spelling_for_comment_anchors() -> Result<(), FormatError> {
    let source = r#"def main() -> None:
    """Docstring."""
    # Comment before float line.
    x = 120.0
    # Comment before int line.
    y = 1
    # Comment before E float line.
    z = 1E6
    # Comment before e float line.
    w = 1e6
    # Comment before underscore float line.
    v = 1_000.0
    # Comment before underscore int line.
    u = 1_000
"#;
    let formatted = format_source(source)?;
    assert!(
        formatted.contains("120.0"),
        "expected 120.0 spelling preserved; got: {formatted:?}"
    );
    assert!(
        formatted.contains("z = 1E6"),
        "expected uppercase E on z line; got: {formatted:?}"
    );
    assert!(
        formatted.contains("w = 1e6"),
        "expected lowercase e on w line; got: {formatted:?}"
    );
    assert!(
        formatted.contains("v = 1_000.0"),
        "expected underscores on v line; got: {formatted:?}"
    );
    assert!(
        formatted.contains("u = 1_000"),
        "expected underscores on int u line; got: {formatted:?}"
    );

    let lines: Vec<&str> = formatted.lines().collect();
    assert_comment_line_immediately_before(
        &lines,
        "x = 120.0",
        |l| l.trim_start().starts_with("x = "),
        "# Comment before float",
    )?;
    assert_comment_line_immediately_before(
        &lines,
        "y = 1",
        |l| l.trim_start().starts_with("y = "),
        "# Comment before int",
    )?;
    assert_comment_line_immediately_before(
        &lines,
        "z = 1E6",
        |l| l.trim_start().starts_with("z = "),
        "# Comment before E float",
    )?;
    assert_comment_line_immediately_before(
        &lines,
        "w = 1e6",
        |l| l.trim_start().starts_with("w = "),
        "# Comment before e float",
    )?;
    assert_comment_line_immediately_before(
        &lines,
        "v = 1_000.0",
        |l| l.trim_start().starts_with("v = "),
        "# Comment before underscore float",
    )?;
    assert_comment_line_immediately_before(
        &lines,
        "u = 1_000",
        |l| l.trim_start().starts_with("u = "),
        "# Comment before underscore int",
    )?;
    Ok(())
}

/// [`IntLiteral::repr`](incan_syntax::ast::IntLiteral) / [`FloatLiteral::repr`](incan_syntax::ast::FloatLiteral)
/// use the lexer source slice (`_`, `E`/`e` preserved).
#[test]
fn test_format_source_preserves_numeric_literal_source_substring() -> Result<(), FormatError> {
    let source = r#"def f() -> None:
    a = 1_200.0
    b = 1E6
    c = 1_000
    d = 1e6
    e = 1000.0
    f = 19.99d
"#;
    let formatted = format_source(source)?;
    assert!(
        formatted.contains("1_200.0"),
        "expected underscore separators preserved in float literal; got: {formatted:?}"
    );
    assert!(
        formatted.contains("b = 1E6"),
        "expected uppercase E on b line; got: {formatted:?}"
    );
    assert!(
        formatted.contains("c = 1_000"),
        "expected underscore separators preserved in int literal; got: {formatted:?}"
    );
    assert!(
        formatted.contains("d = 1e6"),
        "expected lowercase e on d line; got: {formatted:?}"
    );
    assert!(
        formatted.contains("e = 1000.0"),
        "expected plain 1000.0 preserved; got: {formatted:?}"
    );
    assert!(
        formatted.contains("f = 19.99d"),
        "expected decimal literal suffix preserved; got: {formatted:?}"
    );
    Ok(())
}

#[test]
fn test_format_long_constructor_call_wraps_args() -> Result<(), FormatError> {
    let source = r#"def build_schema() -> Schema:
    return CarrierSchema(declared_columns=declared_columns(), planned_columns=planned_columns(), resolved_columns=resolved_columns())
"#;
    let config = FormatConfig::new().with_line_length(60).with_trailing_commas(true);
    let result = format_source_with_config(source, config)?;
    let expected = r#"def build_schema() -> Schema:
    return CarrierSchema(
        declared_columns=declared_columns(),
        planned_columns=planned_columns(),
        resolved_columns=resolved_columns(),
    )
"#;
    assert_eq!(result, expected);
    Ok(())
}

#[test]
fn test_format_long_constructor_call_wraps_without_trailing_comma_when_disabled() -> Result<(), FormatError> {
    let source = r#"def build_schema() -> Schema:
    return CarrierSchema(declared_columns=declared_columns(), planned_columns=planned_columns(), resolved_columns=resolved_columns())
"#;
    let config = FormatConfig::new().with_line_length(60).with_trailing_commas(false);
    let result = format_source_with_config(source, config)?;
    let expected = r#"def build_schema() -> Schema:
    return CarrierSchema(
        declared_columns=declared_columns(),
        planned_columns=planned_columns(),
        resolved_columns=resolved_columns()
    )
"#;
    assert_eq!(result, expected);
    Ok(())
}

#[test]
fn test_format_source_loop_expression_with_break_value_round_trip() -> Result<(), FormatError> {
    let source = r#"def run() -> int:
    return loop:
        break 42
"#;
    let formatted = format_source(source)?;
    assert_eq!(formatted.trim_end(), source.trim_end());
    Ok(())
}
