//! A Loaf's checked declarations as records of their own (RFC 179): each declaration's signature, each nominal
//! declaration's layout, and the defaults of its parameters and fields, projected from the documentation-free checked
//! API so a consumer that needs one of them reads that record alone, and each record's semantic digest leaves
//! documentation out.

use serde_json::{Map, Value};

use crate::api_metadata::CheckedApiMetadataPackage;

/// The member fields of a parameter or field entry that state its default.
const DEFAULT_FIELDS: [&str; 3] = ["has_default", "default_is_empty", "default"];

/// The documentation fields a documentation-free checked API leaves empty.
const DOCUMENTATION_FIELDS: [&str; 2] = ["docstring", "docstring_sections"];

/// The member lists that make up a nominal declaration's layout.
const LAYOUT_FIELDS: [&str; 2] = ["fields", "variants"];

/// The declaration facts that keep their own record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeclarationFacet {
    /// What a caller checks a use against: kind, type parameters, parameters, result, methods and adoptions.
    Signature,
    /// The fields or variants of a nominal declaration.
    Layout,
    /// Which parameters and fields have a default, and the defaults themselves.
    Defaults,
}

/// One declaration fact: its facet, the declaration's module-qualified name and the projected value.
#[derive(Clone, Debug, PartialEq)]
pub struct DeclarationFact {
    pub facet: DeclarationFacet,
    pub name: String,
    pub value: Value,
}

/// Every declaration fact of `api`, in declaration order: each declaration's signature, then its layout and defaults
/// when it has them. `api` carries no documentation (`LibraryContractMetadata::split_documentation`).
pub fn declaration_facts(api: &CheckedApiMetadataPackage) -> Result<Vec<DeclarationFact>, serde_json::Error> {
    let mut facts = Vec::new();
    for module in &api.modules {
        for declaration in &module.declarations {
            let mut signature = serde_json::to_value(declaration)?;
            let simple = signature
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let name = qualified_name(&module.module_path, &simple);
            let mut defaults = Vec::new();
            without_documentation(&mut signature);
            take_defaults(&mut signature, &simple, &mut defaults);
            let layout = take_layout(&mut signature);
            facts.push(DeclarationFact {
                facet: DeclarationFacet::Signature,
                name: name.clone(),
                value: signature,
            });
            if let Some(layout) = layout {
                facts.push(DeclarationFact {
                    facet: DeclarationFacet::Layout,
                    name: name.clone(),
                    value: layout,
                });
            }
            if !defaults.is_empty() {
                facts.push(DeclarationFact {
                    facet: DeclarationFacet::Defaults,
                    name,
                    value: Value::Array(defaults),
                });
            }
        }
    }
    Ok(facts)
}

/// The name a declaration's records are addressed by: its module path and its own name.
pub fn qualified_name(module_path: &[String], name: &str) -> String {
    module_path
        .iter()
        .map(String::as_str)
        .chain(std::iter::once(name))
        .collect::<Vec<_>>()
        .join("::")
}

/// Move the default fields of every member entry under `value` into `defaults`, each named by its member path.
fn take_defaults(value: &mut Value, member: &str, defaults: &mut Vec<Value>) {
    let Value::Object(fields) = value else {
        return;
    };
    let mut taken = Map::new();
    for field in DEFAULT_FIELDS {
        if let Some(found) = fields.remove(field) {
            taken.insert(field.to_owned(), found);
        }
    }
    if !taken.is_empty() {
        taken.insert("member".to_owned(), Value::String(member.to_owned()));
        defaults.push(Value::Object(taken));
    }
    for (key, child) in fields.iter_mut() {
        match child {
            Value::Array(items) => {
                for item in items {
                    let name = item.get("name").and_then(Value::as_str).unwrap_or_default().to_owned();
                    take_defaults(item, &format!("{member}.{key}.{name}"), defaults);
                }
            }
            Value::Object(_) => take_defaults(child, &format!("{member}.{key}"), defaults),
            _ => {}
        }
    }
}

/// Move a nominal declaration's fields or variants out of its signature into its layout.
fn take_layout(signature: &mut Value) -> Option<Value> {
    let Value::Object(fields) = signature else {
        return None;
    };
    let mut layout = Map::new();
    for field in LAYOUT_FIELDS {
        if let Some(members) = fields.remove(field) {
            layout.insert(field.to_owned(), members);
        }
    }
    if layout.is_empty() {
        return None;
    }
    for field in ["kind", "name", "type_params"] {
        if let Some(value) = fields.get(field) {
            layout.insert(field.to_owned(), value.clone());
        }
    }
    Some(Value::Object(layout))
}

/// Remove the emptied documentation fields of a declaration and of its members.
fn without_documentation(value: &mut Value) {
    match value {
        Value::Object(fields) => {
            for field in DOCUMENTATION_FIELDS {
                fields.remove(field);
            }
            fields.values_mut().for_each(without_documentation);
        }
        Value::Array(items) => items.iter_mut().for_each(without_documentation),
        _ => {}
    }
}
