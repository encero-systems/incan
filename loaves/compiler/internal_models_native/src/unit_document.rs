//! Private compiler units use a closed Crumb schema; the public JSON field projection remains shared with
//! describe-unit.

use crate::crumb_rows::{ModelField, model_description, required_value};
use crate::graph_crumb_fields::{integer as signed_value, items, number, text, tuple_items};
use crate::plan_wire_rows::{read_wire, write_wire};
use internal_models_bindings::{Description, JsonValue, NativeJsonValue, Value, ValueField, json_native, json_value};
use serde_json::Value as Json;
use std::collections::HashMap;

/// The six-byte selector stays fixed when unit fields are added or declarations are reordered.
struct UnitField {
    name: &'static str,
    selector: &'static [u8],
    item: &'static str,
    storage: &'static str,
    multiple: bool,
    presence: &'static str,
}

const fn f(
    name: &'static str,
    selector: &'static [u8],
    item: &'static str,
    storage: &'static str,
    multiple: bool,
    presence: &'static str,
) -> UnitField {
    UnitField {
        name,
        selector,
        item,
        storage,
        multiple,
        presence,
    }
}

fn fields(kind: &str) -> Result<&'static [UnitField], String> {
    Ok(match kind {
        "unit_descriptor" => {
            const FIELDS: &[UnitField] = &[
                f("active_features", b"f00000", "text", "list<string>", true, "required"),
                f(
                    "c_abi_target",
                    b"f00001",
                    "unit_target",
                    "option<record@756e69747467>",
                    false,
                    "omitted",
                ),
                f("c_headers", b"f00002", "text", "list<string>", true, "required"),
                f("checked_context", b"f00003", "text", "option<string>", false, "omitted"),
                f(
                    "checked_products",
                    b"f00004",
                    "text",
                    "option<string>",
                    false,
                    "omitted",
                ),
                f(
                    "cloves",
                    b"f00005",
                    "unit_clove",
                    "option<list<record@756e6974636c>>",
                    true,
                    "omitted",
                ),
                f("contract_models", b"f00006", "text", "list<string>", true, "required"),
                f(
                    "covered_by",
                    b"f00007",
                    "bindings",
                    "list<tuple<string,string>>",
                    false,
                    "required",
                ),
                f("declared_features", b"f00008", "text", "list<string>", true, "required"),
                f(
                    "excluded_sources",
                    b"f00009",
                    "text",
                    "option<list<string>>",
                    true,
                    "omitted",
                ),
                f(
                    "externs",
                    b"f00010",
                    "native_crate",
                    "list<record@756e69746578>",
                    true,
                    "required",
                ),
                f("kind", b"f00011", "text", "string", false, "required"),
                f(
                    "loaves",
                    b"f00012",
                    "unit_loaf",
                    "list<record@756e69746c66>",
                    true,
                    "required",
                ),
                f(
                    "not_prepared_loaves",
                    b"f00013",
                    "text",
                    "list<string>",
                    true,
                    "required",
                ),
                f("package", b"f00014", "text", "string", false, "required"),
                f("profile", b"f00015", "text", "string", false, "required"),
                f(
                    "program_checks",
                    b"f00016",
                    "boolean",
                    "option<boolean>",
                    false,
                    "omitted",
                ),
                f("program_entries", b"f00017", "text", "list<string>", true, "required"),
                f("project_library", b"f00018", "text", "option<string>", false, "omitted"),
                f("project_root", b"f00019", "text", "string", false, "required"),
                f("registry_id", b"f00020", "text", "option<string>", false, "omitted"),
                f("rust_abi", b"f00021", "text", "list<string>", true, "required"),
                f("rust_crates", b"f00022", "text", "list<string>", true, "required"),
                f("schema", b"f00023", "text", "string", false, "required"),
                f("search", b"f00024", "text", "list<string>", true, "required"),
                f("search_index", b"f00025", "text", "option<string>", false, "nullable"),
                f("source", b"f00026", "text", "string", false, "required"),
                f("source_root", b"f00027", "text", "string", false, "required"),
                f("test_batch", b"f00028", "text", "option<list<string>>", true, "omitted"),
                f(
                    "toolchain_access",
                    b"f00029",
                    "unit_access",
                    "option<record@756e69746163>",
                    false,
                    "omitted",
                ),
                f("trust_stop", b"f00030", "text", "string", false, "required"),
                f("version", b"f00031", "text", "string", false, "required"),
                f("vocab", b"f00032", "text", "option<string>", false, "nullable"),
                f("vocab_admissions", b"f00033", "text", "list<string>", true, "required"),
                f("vocab_desugarer", b"f00034", "boolean", "boolean", false, "required"),
                f("vocab_entry", b"f00035", "text", "option<string>", false, "nullable"),
                f("vocab_policy", b"f00036", "text", "string", false, "required"),
                f("vocabulary", b"f00037", "boolean", "boolean", false, "required"),
            ];
            FIELDS
        }
        "unit_loaf" => {
            const FIELDS: &[UnitField] = &[
                f("dir", b"f00000", "text", "string", false, "required"),
                f("key", b"f00001", "text", "string", false, "required"),
                f("name", b"f00002", "text", "string", false, "required"),
                f("source_root", b"f00003", "text", "option<string>", false, "omitted"),
                f("standard", b"f00004", "boolean", "boolean", false, "required"),
                f("toolchain_provided", b"f00005", "boolean", "boolean", false, "required"),
            ];
            FIELDS
        }
        "native_crate" => {
            const FIELDS: &[UnitField] = &[
                f("name", b"f00000", "text", "string", false, "required"),
                f("path", b"f00001", "text", "string", false, "required"),
            ];
            FIELDS
        }
        "unit_clove" => {
            const FIELDS: &[UnitField] = &[
                f("compiler", b"f00000", "text", "string", false, "required"),
                f(
                    "configurations",
                    b"f00001",
                    "clove_configuration",
                    "list<record@756e69746366>",
                    true,
                    "required",
                ),
                f("cplusplus", b"f00002", "boolean", "boolean", false, "required"),
                f("include", b"f00003", "text", "list<string>", true, "required"),
                f("library", b"f00004", "text", "string", false, "required"),
                f("name", b"f00005", "text", "string", false, "required"),
                f("public_headers", b"f00006", "text", "list<string>", true, "required"),
                f("root", b"f00007", "text", "string", false, "required"),
            ];
            FIELDS
        }
        "clove_configuration" => {
            const FIELDS: &[UnitField] = &[
                f("archive", b"f00000", "text", "string", false, "required"),
                f("definitions", b"f00001", "text", "list<string>", true, "required"),
                f("identity", b"f00002", "text", "string", false, "required"),
                f("name", b"f00003", "text", "string", false, "required"),
            ];
            FIELDS
        }
        "unit_target" => {
            const FIELDS: &[UnitField] = &[
                f("api_level", b"f00000", "u32", "option<i64>", false, "omitted"),
                f("definitions", b"f00001", "text", "list<string>", true, "required"),
                f(
                    "deployment_target",
                    b"f00002",
                    "text",
                    "option<string>",
                    false,
                    "omitted",
                ),
                f(
                    "sdk",
                    b"f00003",
                    "unit_capability",
                    "option<record@756e69746370>",
                    false,
                    "omitted",
                ),
                f("target", b"f00004", "text", "string", false, "required"),
                f(
                    "toolchain",
                    b"f00005",
                    "unit_capability",
                    "option<record@756e69746370>",
                    false,
                    "omitted",
                ),
            ];
            FIELDS
        }
        "unit_capability" => {
            const FIELDS: &[UnitField] = &[
                f("capability", b"f00000", "text", "string", false, "required"),
                f("version", b"f00001", "text", "string", false, "required"),
            ];
            FIELDS
        }
        "unit_access" => {
            const FIELDS: &[UnitField] = &[
                f(
                    "sysroot_dependencies",
                    b"f00000",
                    "text",
                    "list<string>",
                    true,
                    "required",
                ),
                f(
                    "toolchain_components",
                    b"f00001",
                    "text",
                    "list<string>",
                    true,
                    "required",
                ),
                f("unstable_features", b"f00002", "text", "list<string>", true, "required"),
            ];
            FIELDS
        }
        _ => return Err("unknown compiler unit record kind".into()),
    })
}

const KINDS: [(&[u8], &str); 8] = [
    (b"unitdf\x00", "unit_descriptor"),
    (b"unitlf\x00", "unit_loaf"),
    (b"unitex\x00", "native_crate"),
    (b"unitcl\x00", "unit_clove"),
    (b"unitcf\x00", "clove_configuration"),
    (b"unittg\x00", "unit_target"),
    (b"unitcp\x00", "unit_capability"),
    (b"unitac\x00", "unit_access"),
];

fn descriptions() -> Result<Vec<Description>, String> {
    KINDS
        .iter()
        .map(|(selector, kind)| {
            let model: Vec<ModelField> = fields(kind)?
                .iter()
                .map(|value| ModelField::new(value.selector, value.name, value.storage))
                .collect();
            Ok(model_description(
                selector,
                &format!("incan.compiler-unit/2/{kind}"),
                &model,
            ))
        })
        .collect()
}

fn type_error(detail: &str) -> String {
    format!("TypeError: {detail}")
}

fn expect_object(document: &Json) -> Result<&serde_json::Map<String, Json>, String> {
    document.as_object().ok_or_else(|| type_error("expected JSON object"))
}

fn project_object(kind: &str, document: &Json) -> Result<Value, String> {
    let object = expect_object(document)?;
    let mut output: Vec<ValueField> = Vec::new();
    let mut consumed = 0;
    for spec in fields(kind)? {
        let mut value = Value::NoneValue;
        if let Some(selected) = object.get(spec.name) {
            consumed += 1;
            if selected.is_null() && spec.presence == "omitted" {
                return Err(format!(
                    "compiler unit optional field {kind}.{} must be omitted or present with its declared type",
                    spec.name
                ));
            }
            if !selected.is_null() || spec.presence == "required" {
                value = project_field(spec, selected)?;
                if spec.presence != "required" {
                    value = Value::Some(vec![value]);
                }
            }
        } else if spec.presence != "omitted" {
            return Err(format!("missing compiler unit field {kind}.{}", spec.name));
        }
        output.push(required_value(spec.selector, value));
    }
    if consumed != object.len() {
        return Err(format!("unknown compiler unit field in {kind}"));
    }
    Ok(Value::Record(output))
}

fn project_field(spec: &UnitField, document: &Json) -> Result<Value, String> {
    if spec.multiple {
        let elements = document.as_array().ok_or_else(|| type_error("expected JSON array"))?;
        let values = elements
            .iter()
            .map(|element| project_item(spec.item, element))
            .collect::<Result<_, _>>()?;
        return Ok(Value::List(values));
    }
    project_item(spec.item, document)
}

fn project_item(kind: &str, document: &Json) -> Result<Value, String> {
    match kind {
        "text" => Ok(Value::Text(
            document
                .as_str()
                .ok_or_else(|| type_error("expected JSON string"))?
                .to_owned(),
        )),
        "boolean" => Ok(Value::Boolean(
            document.as_bool().ok_or_else(|| type_error("expected JSON bool"))?,
        )),
        "u32" => {
            let n = document.as_i64().ok_or_else(|| type_error("expected JSON int"))?;
            if !(0..=4_294_967_295).contains(&n) {
                return Err("compiler unit integer outside u32".into());
            }
            Ok(signed_value(n))
        }
        "bindings" => {
            let binding = expect_object(document)?;
            let mut names: Vec<&String> = binding.keys().collect();
            names.sort();
            let pairs = names
                .into_iter()
                .map(|name| {
                    let value = binding[name]
                        .as_str()
                        .ok_or_else(|| type_error("expected JSON string"))?;
                    Ok(Value::Tuple(vec![
                        Value::Text(name.clone()),
                        Value::Text(value.to_owned()),
                    ]))
                })
                .collect::<Result<_, String>>()?;
            Ok(Value::List(pairs))
        }
        _ => project_object(kind, document),
    }
}

fn read_object(kind: &str, stored: &Value) -> Result<NativeJsonValue, String> {
    let specs = fields(kind)?;
    let Value::Record(encoded) = stored else {
        return Err("expected compiler unit object".into());
    };
    if encoded.len() != specs.len() {
        return Err("compiler unit field count differs".into());
    }
    let mut output: HashMap<String, NativeJsonValue> = HashMap::new();
    for (spec, field) in specs.iter().zip(encoded) {
        if field.identity != spec.selector {
            return Err("compiler unit field selector differs".into());
        }
        if spec.presence == "required" {
            output.insert(spec.name.to_owned(), read_field(spec, &field.value)?);
            continue;
        }
        match &field.value {
            Value::NoneValue => {
                if spec.presence == "nullable" {
                    output.insert(spec.name.to_owned(), NativeJsonValue::null());
                }
            }
            Value::Some(inside) => {
                let [inside] = inside.as_slice() else {
                    return Err("invalid optional compiler unit field".into());
                };
                output.insert(spec.name.to_owned(), read_field(spec, inside)?);
            }
            _ => return Err("expected optional compiler unit field".into()),
        }
    }
    Ok(NativeJsonValue::object(output))
}

fn read_field(spec: &UnitField, value: &Value) -> Result<NativeJsonValue, String> {
    if spec.multiple {
        let values = items(value)?
            .iter()
            .map(|element| read_item(spec.item, element))
            .collect::<Result<_, _>>()?;
        return Ok(NativeJsonValue::array(values));
    }
    read_item(spec.item, value)
}

fn read_item(kind: &str, value: &Value) -> Result<NativeJsonValue, String> {
    match kind {
        "text" => Ok(NativeJsonValue::string(text(value)?)),
        "boolean" => match value {
            Value::Boolean(flag) => Ok(NativeJsonValue::bool(*flag)),
            _ => Err("expected compiler unit Boolean".into()),
        },
        "u32" => {
            let n = number(value)?;
            if !(0..=4_294_967_295).contains(&n) {
                return Err("compiler unit integer outside u32".into());
            }
            Ok(NativeJsonValue::int(n))
        }
        "bindings" => {
            let mut result = HashMap::new();
            let mut previous: Option<String> = None;
            for pair in items(value)? {
                let values = tuple_items(pair, 2)?;
                let name = text(&values[0])?;
                if previous.as_ref().is_some_and(|before| name <= *before) {
                    return Err("compiler unit bindings require unique sorted keys".into());
                }
                result.insert(name.clone(), NativeJsonValue::string(text(&values[1])?));
                previous = Some(name);
            }
            Ok(NativeJsonValue::object(result))
        }
        _ => read_object(kind, value),
    }
}

/// Project all compiler input fields into a typed row without serializing a JSON document.
pub fn encode_unit_document(document: &JsonValue) -> Result<Vec<u8>, String> {
    let native = json_native(document);
    let projected = project_object("unit_descriptor", native.as_serde())?;
    write_wire(b"unitdf\x00", &descriptions()?, projected)
}

/// Restore the closed field projection for the compiler's existing unit validator.
pub fn decode_unit_document(encoded: &[u8]) -> Result<JsonValue, String> {
    let stored = read_wire(encoded, b"unitdf\x00", &descriptions()?)?;
    Ok(json_value(read_object("unit_descriptor", &stored)?))
}
