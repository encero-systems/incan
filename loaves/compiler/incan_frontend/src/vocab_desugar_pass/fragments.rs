//! Bind RFC 081's owned fragment artifacts to the public `std.vocab` syntax schema without serializing private AST.

use incan_semantics_core::SurfaceFeatureKey;
use serde_json::{Value, json};

use super::surface::{expression, source_span};
use crate::ast;

/// Retain descriptor identity, source range, lexical tree and holes for an admitted native lowering hook.
pub(super) fn fragment(fragment: &ast::EmbeddedFragmentExpr, span: ast::Span) -> Result<Value, String> {
    let SurfaceFeatureKey::ScopedDslSurface {
        dependency_key,
        descriptor_key,
    } = &fragment.key
    else {
        return Err("embedded fragment has no vocabulary descriptor identity".into());
    };
    Ok(
        json!({"span": source_span(span), "dependency_key": dependency_key, "descriptor_key": descriptor_key,
        "submode": fragment.submode, "nodes": nodes(&fragment.nodes)?, "source_text": fragment.source_text,
        "layout_sensitive": fragment.layout_sensitive}),
    )
}

/// Translate each lexical node at its own source range; holes use the ordinary spanned expression boundary.
fn nodes(items: &[ast::Spanned<ast::EmbeddedNode>]) -> Result<Vec<Value>, String> {
    items.iter().map(node).collect()
}

/// Project one RFC 081 production into its explicitly versioned public alternative.
fn node(value: &ast::Spanned<ast::EmbeddedNode>) -> Result<Value, String> {
    let kind = match &value.node {
        ast::EmbeddedNode::Text(value) => json!({"Text": value}),
        ast::EmbeddedNode::Hole(value) => json!({"Hole": expression(value)?}),
        ast::EmbeddedNode::Element(element) => json!({"Element": {
            "span": source_span(value.span), "name": element.name, "attributes": attributes(&element.attrs, value.span)?,
            "children": nodes(&element.children)?, "self_closing": element.self_closing,
        }}),
        ast::EmbeddedNode::EntityRef(value) => json!({"EntityRef": value}),
        ast::EmbeddedNode::Comment(value) => json!({"Comment": value}),
        ast::EmbeddedNode::StyleRule(rule) => {
            json!({"StyleRule": [nodes(&rule.selectors)?, nodes(&rule.declarations)?]})
        }
        ast::EmbeddedNode::Declaration(declaration) => {
            json!({"Declaration": [declaration.property, nodes(&declaration.value)?]})
        }
        ast::EmbeddedNode::Value(value) => json!({"Value": literal(value)}),
        ast::EmbeddedNode::Regex { pattern, flags } => json!({"Regex": [pattern, flags]}),
        ast::EmbeddedNode::TypeShape(shape) => json!({"TypeShape": type_shape(shape, value.span)}),
    };
    Ok(json!({"span": source_span(value.span), "kind": kind}))
}

/// Preserve attribute values as lexical nodes; bare attributes inherit their element's available source range.
fn attributes(items: &[ast::EmbeddedAttr], anchor: ast::Span) -> Result<Vec<Value>, String> {
    items
        .iter()
        .map(|attribute| {
            let span = attribute.value.as_ref().map(|value| value.span).unwrap_or(anchor);
            let values = attribute
                .value
                .as_ref()
                .map(node)
                .transpose()?
                .into_iter()
                .collect::<Vec<_>>();
            Ok(json!({"span": source_span(span), "name": attribute.name, "value": values}))
        })
        .collect()
}

/// Encode style and selector literals with the same alternatives as the public Incan enum.
fn literal(value: &ast::EmbeddedValue) -> Value {
    match value {
        ast::EmbeddedValue::Dimension { number, unit } => json!({"Dimension": [number, unit]}),
        ast::EmbeddedValue::Color(value) => json!({"Color": value}),
        ast::EmbeddedValue::CustomPropertyRef(value) => json!({"CustomPropertyRef": value}),
        ast::EmbeddedValue::Ident(value) => json!({"Ident": value}),
        ast::EmbeddedValue::StringLit(value) => json!({"StringLit": value}),
        ast::EmbeddedValue::Number(value) => json!({"Number": value}),
        ast::EmbeddedValue::Selector(value) => json!({"Selector": value}),
    }
}

/// Preserve type-position structure at the finest range the parser retains for that production.
fn type_shape(shape: &ast::EmbeddedTypeShape, span: ast::Span) -> Value {
    let kind = match shape {
        ast::EmbeddedTypeShape::Name(path) => json!({"Name": path}),
        ast::EmbeddedTypeShape::Generic(base, args) => {
            let mut parts = vec![type_shape(base, span)];
            parts.extend(args.iter().map(|shape| type_shape(shape, span)));
            json!({"Generic": parts})
        }
        ast::EmbeddedTypeShape::Nullable(inner) => json!({"Nullable": [type_shape(inner, span)]}),
        ast::EmbeddedTypeShape::Array(inner) => json!({"Array": [type_shape(inner, span)]}),
        ast::EmbeddedTypeShape::Union(parts) => {
            json!({"Union": parts.iter().map(|shape| type_shape(shape, span)).collect::<Vec<_>>()})
        }
    };
    json!({"span": source_span(span), "kind": kind})
}
