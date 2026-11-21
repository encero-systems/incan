//! Typed Crumb vocabulary for private checked callable products and their exact selection witnesses.
//!
//! Translated mechanically from the reference module's generated descriptions; field order, identities and
//! storage spellings are unchanged.

use crate::crumb_rows::{ModelField, model_description};
use crate::namespace_schema::namespace_descriptions;
use crate::plan_wire_rows::{enum_schema, variant_schema};
use internal_models_bindings::Description;

pub const CALLABLE_PRODUCT: &[u8] = b"\xbe\x74\x60\x00\x53\x41\x00";
fn shape_callable_inventory() -> Description {
    model_description(
        b"\x37\x21\x48\x67\xb8\x28\x00",
        "callable_products.CallableInventory",
        &[
            ModelField::new(
                b"\x93\x59\xf3\xbb\x2e\xfa",
                "namespace",
                "list<tuple<record@74a6fa8e9339,string>>",
            ),
            ModelField::new(
                b"\xba\x52\x85\x16\x1b\xa6",
                "environment",
                "list<tuple<record@74a6fa8e9339,string>>",
            ),
            ModelField::new(b"\x18\x1f\xdd\x46\xfc\x4a", "origin", "enum@ca6e161f17e2"),
            ModelField::new(
                b"\xdc\x23\x16\xf8\xd4\x37",
                "callables",
                "list<tuple<record@165278c9ea7f,string>>",
            ),
            ModelField::new(b"\x87\xd0\x5c\xd0\x88\x17", "entries", "list<record@3c3b4811ead7>"),
        ],
    )
}

fn shape_callable_header() -> Description {
    model_description(
        b"\x16\x52\x78\xc9\xea\x7f\x00",
        "callable_products.CallableHeader",
        &[
            ModelField::new(b"\x68\x9f\x6a\x62\x73\x84", "identity", "record@cc1467c513c3"),
            ModelField::new(b"\x0d\xee\xb8\xfa\x1d\xbb", "canonical", "record@28cdd429f7ec"),
            ModelField::new(b"\x2a\x56\xe0\x44\x58\x75", "is_public", "boolean"),
            ModelField::new(b"\xd3\x33\x6a\x33\x4a\x5e", "mutation", "list<option<boolean>>"),
            ModelField::new(b"\xbc\x4a\x71\x18\x08\x70", "package", "string"),
            ModelField::new(b"\x5c\xa4\xf3\x85\x0c\xcc", "version", "string"),
            ModelField::new(b"\x85\x7d\xc6\xeb\xc7\xd2", "source_path", "list<string>"),
            ModelField::new(b"\xcd\x2a\x7c\x4b\x4b\x13", "replay_identity", "record@28cdd429f7ec"),
            ModelField::new(b"\x78\xf9\xac\x01\x8e\x55", "function", "record@bc0afdf0b3ec"),
        ],
    )
}

fn shape_callable_owner() -> Description {
    model_description(
        b"\x3c\x3b\x48\x11\xea\xd7\x00",
        "callable_products.CallableOwner",
        &[
            ModelField::new(b"\x1e\x05\x84\xa2\x5d\x9f", "header", "record@165278c9ea7f"),
            ModelField::new(b"\x1a\x2f\xc2\x6d\xc7\xea", "signature", "string"),
            ModelField::new(b"\xe4\x09\xbf\xc6\xfe\x79", "meaning", "string"),
            ModelField::new(b"\x85\xb3\x5e\xae\xe3\x2a", "header_source", "string"),
            ModelField::new(b"\x41\xcf\x67\x94\xba\x42", "source", "string"),
            ModelField::new(b"\x35\x66\xcf\x23\xec\xb6", "reads", "list<record@8e98fd62dead>"),
            ModelField::new(b"\xab\xcd\x0e\xe1\xfc\x2c", "declaration", "binary"),
            ModelField::new(b"\x25\xbf\x8e\x1a\x23\x93", "local", "binary"),
            ModelField::new(b"\x75\x20\x3e\x01\xaa\xd7", "final_value", "binary"),
            ModelField::new(
                b"\x67\x18\x3e\x4a\xb6\x71",
                "contexts",
                "list<tuple<tuple<record@28cdd429f7ec,record@28cdd429f7ec>,u32>>",
            ),
            ModelField::new(b"\x23\x0d\x83\x58\xdc\x8e", "body", "binary"),
        ],
    )
}

fn shape_callable_read() -> Description {
    model_description(
        b"\x8e\x98\xfd\x62\xde\xad\x00",
        "callable_products.CallableRead",
        &[
            ModelField::new(b"\x82\xa3\x53\x7f\xf0\xdb", "name", "string"),
            ModelField::new(b"\x18\x1f\xdd\x46\xfc\x4a", "origin", "enum@ca6e161f17e2"),
            ModelField::new(
                b"\xba\x15\x9d\xbf\x9d\x2f",
                "candidates",
                "list<tuple<record@cc1467c513c3,string>>",
            ),
            ModelField::new(
                b"\x03\x16\xe1\x80\x93\x18",
                "signatures",
                "list<tuple<record@28cdd429f7ec,string>>",
            ),
            ModelField::new(
                b"\x32\x38\x15\x23\xe6\x31",
                "inputs",
                "list<tuple<record@74a6fa8e9339,string>>",
            ),
        ],
    )
}

fn shape_function_export() -> Description {
    model_description(
        b"\xbc\x0a\xfd\xf0\xb3\xec\x00",
        "callable_products.FunctionExport",
        &[
            ModelField::new(b"\x01\xb8\x01\x6f\xdc\xe4", "ownership", "option<enum@b8dfa1e04174>"),
            ModelField::new(b"\x82\xa3\x53\x7f\xf0\xdb", "name", "string"),
            ModelField::new(b"\xd4\x1a\x29\xea\x21\x71", "emitted_name", "option<string>"),
            ModelField::new(b"\x75\xe7\xe9\xde\xad\x7e", "type_params", "list<record@80549e0eb490>"),
            ModelField::new(b"\xa2\x0b\x52\xfa\xe5\x7c", "params", "list<record@5e91aed5a32f>"),
            ModelField::new(b"\x40\x7d\xdf\x95\xec\x74", "return_type", "enum@20bf1c617420"),
            ModelField::new(b"\x4e\x13\xcb\x86\xec\x33", "is_async", "boolean"),
        ],
    )
}

fn shape_stable_declaration_id() -> Description {
    model_description(
        b"\xcc\x14\x67\xc5\x13\xc3\x00",
        "callable_products.StableDeclarationId",
        &[
            ModelField::new(b"\x93\x59\xf3\xbb\x2e\xfa", "namespace", "enum@b3ba1ed741db"),
            ModelField::new(b"\x18\x1f\xdd\x46\xfc\x4a", "origin", "enum@ca6e161f17e2"),
            ModelField::new(b"\xd6\x3b\xda\xbe\x07\xa6", "declaration_name", "string"),
            ModelField::new(b"\x0b\x27\xc1\x58\xfe\xb3", "kind", "enum@a9554703fa2e"),
            ModelField::new(b"\xe6\xea\xea\x18\xe8\x85", "location", "enum@b857b76c296c"),
            ModelField::new(b"\x1a\x2f\xc2\x6d\xc7\xea", "signature", "option<string>"),
        ],
    )
}

fn shape_location() -> Description {
    enum_schema(
        b"\xb8\x57\xb7\x6c\x29\x6c\x00",
        "callable_products.StableDeclarationLocation",
        vec![
            variant_schema(b"\x60\x95\x02\x21\x2e\xb7", "ModuleLevel", &[]),
            variant_schema(
                b"\xa5\xe4\x74\x4f\x2c\xd8",
                "Nested",
                &[
                    ModelField::new(b"\x4c\x10\x29\x69\x7e\xe3", "owner", "record@cc1467c513c3"),
                    ModelField::new(b"\x50\x25\x25\x9b\x76\xb5", "binding_ordinal", "u32"),
                ],
            ),
        ],
    )
}

/// The closed description closure of a checked callable publication.
pub fn callable_descriptions() -> Vec<Description> {
    let envelope = model_description(
        CALLABLE_PRODUCT,
        "callable_products.Publication",
        &[
            ModelField::new(b"digest", "checksum", "binary"),
            ModelField::new(b"produc", "product", "record@37214867b828"),
        ],
    );
    let mut descriptions = namespace_descriptions();
    descriptions.extend([
        envelope,
        shape_location(),
        shape_callable_inventory(),
        shape_callable_header(),
        shape_callable_owner(),
        shape_callable_read(),
        shape_function_export(),
        shape_stable_declaration_id(),
    ]);
    descriptions
}
