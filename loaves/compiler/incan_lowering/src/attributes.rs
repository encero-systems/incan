//! Native attribute macros applied to a declaration, copied in written order.

use incan_mir_plan_native::{AttributeArgument, AttributeValue, NativeAttribute};
use incan_semantics_core::body_ir::{Body, NativeAttribute as SourceAttribute, NativeAttributeValue};

/// Copy each application's macro path and written arguments; rustc, not the plan, interprets them.
pub fn native_attributes(body: &Body) -> Vec<NativeAttribute> {
    body.native_attributes.iter().map(native_attribute).collect()
}

/// One application: the macro's canonical path and its arguments in written order.
fn native_attribute(attribute: &SourceAttribute) -> NativeAttribute {
    let arguments = attribute
        .arguments
        .iter()
        .map(|argument| AttributeArgument {
            name: argument.name.clone().unwrap_or_default(),
            value: attribute_value(&argument.value),
        })
        .collect();
    NativeAttribute {
        path: attribute.path.clone(),
        arguments,
    }
}

/// One literal, or a list of them, in the plan's spelling.
fn attribute_value(value: &NativeAttributeValue) -> AttributeValue {
    match value {
        NativeAttributeValue::Text(text) => AttributeValue::Text(text.clone()),
        NativeAttributeValue::Int(number) => AttributeValue::Int(*number),
        NativeAttributeValue::Float(number) => AttributeValue::Float(*number),
        NativeAttributeValue::Bool(flag) => AttributeValue::Bool(*flag),
        NativeAttributeValue::List(items) => AttributeValue::List(items.iter().map(attribute_value).collect()),
    }
}
