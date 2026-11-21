// Shape descriptions of the namespace closure, `access_mode` to `incan_primitive_type` in generated alphabetical order.
// Included by `namespace_schema.rs`, translated mechanically from the reference module's generated descriptions.

fn shape_access_mode() -> Description {
    model_description(
        b"\x62\x2f\x44\x95\x5c\x42\x00",
        "checked_namespace.AccessMode",
        &[
            ModelField::new(b"\xe6\x42\xb1\x29\x01\xa6", "mode", "enum@f3f860857e6c"),
            ModelField::new(b"\x69\x85\xca\x1f\x4d\xaa", "component", "record@fb497dd98dfa"),
            ModelField::new(b"\x34\x2c\x8c\x61\xe6\x79", "underlying", "enum@20bf1c617420"),
        ],
    )
}

fn shape_api_alias() -> Description {
    model_description(
        b"\x38\x96\xed\xb4\x16\x0b\x00",
        "checked_namespace.ApiAlias",
        &[
            ModelField::new(b"\x82\xa3\x53\x7f\xf0\xdb", "name", "string"),
            ModelField::new(b"\x79\xbf\xb0\xe2\xba\x76", "anchor", "record@f4a092c4acca"),
            ModelField::new(b"\xab\x80\x1b\xe6\x56\xe0", "target_path", "list<string>"),
            ModelField::new(b"\x2a\x56\xe0\x44\x58\x75", "is_public", "boolean"),
            ModelField::new(
                b"\x99\x5c\x29\x0c\xb5\x9e",
                "projected_function",
                "option<record@7b0db3092499>",
            ),
            ModelField::new(
                b"\xf7\xea\x57\x60\x4f\x28",
                "projected_type",
                "option<enum@20bf1c617420>",
            ),
        ],
    )
}

fn shape_api_callable_metadata() -> Description {
    model_description(
        b"\x51\xa0\x86\xd2\x00\x6d\x00",
        "checked_namespace.ApiCallableMetadata",
        &[
            ModelField::new(b"\x01\xb8\x01\x6f\xdc\xe4", "ownership", "option<enum@b8dfa1e04174>"),
            ModelField::new(b"\x82\xa3\x53\x7f\xf0\xdb", "name", "string"),
            ModelField::new(b"\x79\xbf\xb0\xe2\xba\x76", "anchor", "record@f4a092c4acca"),
            ModelField::new(b"\x75\xe7\xe9\xde\xad\x7e", "type_params", "list<record@80549e0eb490>"),
            ModelField::new(b"\x81\xba\xe8\x76\xb7\x05", "receiver", "option<enum@01e4c4811dd1>"),
            ModelField::new(b"\xa2\x0b\x52\xfa\xe5\x7c", "params", "list<record@5e91aed5a32f>"),
            ModelField::new(b"\x40\x7d\xdf\x95\xec\x74", "return_type", "enum@20bf1c617420"),
            ModelField::new(b"\x4e\x13\xcb\x86\xec\x33", "is_async", "boolean"),
        ],
    )
}

fn shape_api_class() -> Description {
    model_description(
        b"\x1c\x0f\x94\x55\x99\xfd\x00",
        "checked_namespace.ApiClass",
        &[
            ModelField::new(b"\x82\xa3\x53\x7f\xf0\xdb", "name", "string"),
            ModelField::new(b"\x79\xbf\xb0\xe2\xba\x76", "anchor", "record@f4a092c4acca"),
            ModelField::non_canonical(b"\x6e\x50\xd9\x8d\x24\x31", "docstring", "option<string>"),
            ModelField::non_canonical(
                b"\xf7\xea\x2f\x6a\xdd\xd6",
                "docstring_sections",
                "option<record@fefefb4a64ba>",
            ),
            ModelField::new(b"\x4c\xc3\x59\x9f\x82\x5d", "decorators", "list<record@3681cf30afc4>"),
            ModelField::new(b"\x75\xe7\xe9\xde\xad\x7e", "type_params", "list<record@80549e0eb490>"),
            ModelField::new(b"\x8e\x7c\xe7\x6b\x88\xcc", "extends", "option<string>"),
            ModelField::new(b"\x9e\x27\x94\x08\x61\xd7", "traits", "list<string>"),
            ModelField::new(
                b"\x81\x2f\xff\x9c\x3c\xf4",
                "trait_adoptions",
                "list<record@520b906148ca>",
            ),
            ModelField::new(b"\xa8\x8f\xe6\xec\x61\x15", "derives", "list<string>"),
            ModelField::new(b"\xbf\xe5\xd6\x97\x16\x2d", "fields", "list<record@54738c6fe61a>"),
            ModelField::new(b"\x96\xad\xb9\x24\x52\x86", "properties", "list<record@65a8153729b5>"),
            ModelField::new(b"\x3f\x38\x67\x31\x3a\x9d", "methods", "list<record@24cedfeea4a7>"),
        ],
    )
}

fn shape_api_const() -> Description {
    model_description(
        b"\x53\xb6\xe9\x9d\x46\x9b\x00",
        "checked_namespace.ApiConst",
        &[
            ModelField::new(b"\x82\xa3\x53\x7f\xf0\xdb", "name", "string"),
            ModelField::new(b"\x79\xbf\xb0\xe2\xba\x76", "anchor", "record@f4a092c4acca"),
            ModelField::new(b"\x4f\x9d\xb7\xf8\xe5\x06", "ty", "enum@20bf1c617420"),
            ModelField::new(b"\xcd\x42\x40\x4d\x52\xad", "value", "option<enum@609cc189aa5a>"),
        ],
    )
}

fn shape_api_declaration() -> Description {
    enum_schema(
        b"\x97\x08\x42\x8a\x03\xcc\x00",
        "checked_namespace.ApiDeclaration",
        vec![
            variant_schema(
                b"\xc8\x03\x71\x03\x02\xd5",
                "Function",
                &[ModelField::new(
                    b"\xcc\x1d\x0d\xe5\xa7\x73",
                    "arg0",
                    "record@e1b4063da2e5",
                )],
            ),
            variant_schema(
                b"\x5e\x2c\x61\x4c\x23\xf0",
                "Model",
                &[ModelField::new(
                    b"\xcc\x1d\x0d\xe5\xa7\x73",
                    "arg0",
                    "record@c9f3a34ad89f",
                )],
            ),
            variant_schema(
                b"\x4f\x3a\x9b\xd0\x03\x97",
                "Class",
                &[ModelField::new(
                    b"\xcc\x1d\x0d\xe5\xa7\x73",
                    "arg0",
                    "record@1c0f945599fd",
                )],
            ),
            variant_schema(
                b"\x28\xe3\xe4\x79\xe0\x87",
                "Trait",
                &[ModelField::new(
                    b"\xcc\x1d\x0d\xe5\xa7\x73",
                    "arg0",
                    "record@e632e45386a4",
                )],
            ),
            variant_schema(
                b"\x54\x10\x8d\xaf\xaa\xc5",
                "Enum",
                &[ModelField::new(
                    b"\xcc\x1d\x0d\xe5\xa7\x73",
                    "arg0",
                    "record@66ae2f01311c",
                )],
            ),
            variant_schema(
                b"\x2e\x7e\x82\x4e\x59\x50",
                "Newtype",
                &[ModelField::new(
                    b"\xcc\x1d\x0d\xe5\xa7\x73",
                    "arg0",
                    "record@a58be0c98603",
                )],
            ),
            variant_schema(
                b"\x6b\x9d\x65\x54\x6f\xf3",
                "TypeAlias",
                &[ModelField::new(
                    b"\xcc\x1d\x0d\xe5\xa7\x73",
                    "arg0",
                    "record@899dc20868a6",
                )],
            ),
            variant_schema(
                b"\x10\x71\xe7\xae\x66\xc8",
                "Const",
                &[ModelField::new(
                    b"\xcc\x1d\x0d\xe5\xa7\x73",
                    "arg0",
                    "record@53b6e99d469b",
                )],
            ),
            variant_schema(
                b"\x1d\x2c\x73\xdc\x01\xf2",
                "Static",
                &[ModelField::new(
                    b"\xcc\x1d\x0d\xe5\xa7\x73",
                    "arg0",
                    "record@f30e61c46bc4",
                )],
            ),
            variant_schema(
                b"\xb1\x9e\x02\xe9\x50\x2b",
                "Alias",
                &[ModelField::new(
                    b"\xcc\x1d\x0d\xe5\xa7\x73",
                    "arg0",
                    "record@3896edb4160b",
                )],
            ),
            variant_schema(
                b"\xa4\xd5\x0f\xb8\x54\x03",
                "Partial",
                &[ModelField::new(
                    b"\xcc\x1d\x0d\xe5\xa7\x73",
                    "arg0",
                    "record@ef97293775f5",
                )],
            ),
        ],
    )
}

fn shape_api_docstring() -> Description {
    model_description(
        b"\xfe\xfe\xfb\x4a\x64\xba\x00",
        "checked_namespace.ApiDocstring",
        &[
            ModelField::new(b"\x76\x1b\x7a\xd8\xad\x43", "summary", "option<string>"),
            ModelField::new(b"\xa2\x0b\x52\xfa\xe5\x7c", "params", "list<record@ea6acaa3e08b>"),
            ModelField::new(b"\x15\x3b\x2a\x38\xcb\x4a", "returns", "option<record@2d7f97f31682>"),
            ModelField::new(b"\xbf\xe5\xd6\x97\x16\x2d", "fields", "list<record@ea6acaa3e08b>"),
            ModelField::new(b"\x19\x45\x9a\x61\xf7\xd5", "aliases", "list<record@ea6acaa3e08b>"),
            ModelField::new(b"\x4c\xc3\x59\x9f\x82\x5d", "decorators", "list<record@ea6acaa3e08b>"),
        ],
    )
}

fn shape_api_docstring_entry() -> Description {
    model_description(
        b"\xea\x6a\xca\xa3\xe0\x8b\x00",
        "checked_namespace.ApiDocstringEntry",
        &[
            ModelField::new(b"\x82\xa3\x53\x7f\xf0\xdb", "name", "string"),
            ModelField::new(b"\xc9\x04\x6f\x7a\x37\xad", "description", "string"),
        ],
    )
}

fn shape_api_docstring_return() -> Description {
    model_description(
        b"\x2d\x7f\x97\xf3\x16\x82\x00",
        "checked_namespace.ApiDocstringReturn",
        &[
            ModelField::new(b"\x4f\x9d\xb7\xf8\xe5\x06", "ty", "option<string>"),
            ModelField::new(b"\xc9\x04\x6f\x7a\x37\xad", "description", "string"),
        ],
    )
}

fn shape_api_enum() -> Description {
    model_description(
        b"\x66\xae\x2f\x01\x31\x1c\x00",
        "checked_namespace.ApiEnum",
        &[
            ModelField::new(b"\x82\xa3\x53\x7f\xf0\xdb", "name", "string"),
            ModelField::new(b"\x79\xbf\xb0\xe2\xba\x76", "anchor", "record@f4a092c4acca"),
            ModelField::non_canonical(b"\x6e\x50\xd9\x8d\x24\x31", "docstring", "option<string>"),
            ModelField::non_canonical(
                b"\xf7\xea\x2f\x6a\xdd\xd6",
                "docstring_sections",
                "option<record@fefefb4a64ba>",
            ),
            ModelField::new(b"\x4c\xc3\x59\x9f\x82\x5d", "decorators", "list<record@3681cf30afc4>"),
            ModelField::new(b"\x75\xe7\xe9\xde\xad\x7e", "type_params", "list<record@80549e0eb490>"),
            ModelField::new(b"\x9e\x27\x94\x08\x61\xd7", "traits", "list<string>"),
            ModelField::new(
                b"\x81\x2f\xff\x9c\x3c\xf4",
                "trait_adoptions",
                "list<record@520b906148ca>",
            ),
            ModelField::new(b"\x21\xf5\x02\x67\xa8\xad", "value_type", "option<enum@e7ec50d67734>"),
            ModelField::new(b"\xda\xd5\x69\xe1\x01\xad", "variants", "list<record@e6d8ddf01bb1>"),
            ModelField::new(
                b"\x39\xce\x61\x5d\xb6\x80",
                "variant_aliases",
                "list<record@1676180aeebd>",
            ),
            ModelField::new(b"\x3f\x38\x67\x31\x3a\x9d", "methods", "list<record@24cedfeea4a7>"),
            ModelField::new(b"\xa8\x8f\xe6\xec\x61\x15", "derives", "list<string>"),
        ],
    )
}

fn shape_api_enum_variant() -> Description {
    model_description(
        b"\xe6\xd8\xdd\xf0\x1b\xb1\x00",
        "checked_namespace.ApiEnumVariant",
        &[
            ModelField::new(b"\x82\xa3\x53\x7f\xf0\xdb", "name", "string"),
            ModelField::new(b"\x0d\xee\xb8\xfa\x1d\xbb", "canonical", "option<record@f1bb070a87e5>"),
            ModelField::new(b"\xbf\xe5\xd6\x97\x16\x2d", "fields", "list<enum@20bf1c617420>"),
            ModelField::new(b"\xcd\x42\x40\x4d\x52\xad", "value", "option<enum@1b2f920fa3ca>"),
        ],
    )
}

fn shape_api_enum_variant_alias() -> Description {
    model_description(
        b"\x16\x76\x18\x0a\xee\xbd\x00",
        "checked_namespace.ApiEnumVariantAlias",
        &[
            ModelField::new(b"\x82\xa3\x53\x7f\xf0\xdb", "name", "string"),
            ModelField::new(b"\x34\xa0\x40\x05\xbc\xaf", "target", "string"),
        ],
    )
}

fn shape_api_function() -> Description {
    model_description(
        b"\xe1\xb4\x06\x3d\xa2\xe5\x00",
        "checked_namespace.ApiFunction",
        &[
            ModelField::new(b"\x01\xb8\x01\x6f\xdc\xe4", "ownership", "option<enum@b8dfa1e04174>"),
            ModelField::new(b"\x82\xa3\x53\x7f\xf0\xdb", "name", "string"),
            ModelField::new(b"\x79\xbf\xb0\xe2\xba\x76", "anchor", "record@f4a092c4acca"),
            ModelField::non_canonical(b"\x6e\x50\xd9\x8d\x24\x31", "docstring", "option<string>"),
            ModelField::non_canonical(
                b"\xf7\xea\x2f\x6a\xdd\xd6",
                "docstring_sections",
                "option<record@fefefb4a64ba>",
            ),
            ModelField::new(b"\x4c\xc3\x59\x9f\x82\x5d", "decorators", "list<record@3681cf30afc4>"),
            ModelField::new(b"\x75\xe7\xe9\xde\xad\x7e", "type_params", "list<record@80549e0eb490>"),
            ModelField::new(b"\xa2\x0b\x52\xfa\xe5\x7c", "params", "list<record@5e91aed5a32f>"),
            ModelField::new(b"\x40\x7d\xdf\x95\xec\x74", "return_type", "enum@20bf1c617420"),
            ModelField::new(b"\x4e\x13\xcb\x86\xec\x33", "is_async", "boolean"),
        ],
    )
}

fn shape_api_method() -> Description {
    model_description(
        b"\x24\xce\xdf\xee\xa4\xa7\x00",
        "checked_namespace.ApiMethod",
        &[
            ModelField::new(b"\x01\xb8\x01\x6f\xdc\xe4", "ownership", "option<enum@b8dfa1e04174>"),
            ModelField::new(b"\x82\xa3\x53\x7f\xf0\xdb", "name", "string"),
            ModelField::new(b"\x0d\xee\xb8\xfa\x1d\xbb", "canonical", "option<record@f1bb070a87e5>"),
            ModelField::new(b"\x7d\x2f\xb3\xda\x93\xa0", "visibility", "enum@5d5609e25e89"),
            ModelField::new(b"\x2a\x7e\x64\x03\x5b\x19", "alias_of", "option<string>"),
            ModelField::new(b"\x79\xbf\xb0\xe2\xba\x76", "anchor", "record@f4a092c4acca"),
            ModelField::non_canonical(b"\x6e\x50\xd9\x8d\x24\x31", "docstring", "option<string>"),
            ModelField::non_canonical(
                b"\xf7\xea\x2f\x6a\xdd\xd6",
                "docstring_sections",
                "option<record@fefefb4a64ba>",
            ),
            ModelField::new(b"\x4c\xc3\x59\x9f\x82\x5d", "decorators", "list<record@3681cf30afc4>"),
            ModelField::new(b"\x75\xe7\xe9\xde\xad\x7e", "type_params", "list<record@80549e0eb490>"),
            ModelField::new(b"\x81\xba\xe8\x76\xb7\x05", "receiver", "option<enum@01e4c4811dd1>"),
            ModelField::new(b"\xa2\x0b\x52\xfa\xe5\x7c", "params", "list<record@5e91aed5a32f>"),
            ModelField::new(b"\x40\x7d\xdf\x95\xec\x74", "return_type", "enum@20bf1c617420"),
            ModelField::new(b"\x4e\x13\xcb\x86\xec\x33", "is_async", "boolean"),
            ModelField::new(b"\x44\xaa\x6c\xc3\x56\x75", "has_body", "boolean"),
            ModelField::new(
                b"\x74\x5f\x56\x2a\xc4\x0d",
                "made_private",
                "option<record@aeee71bb1d3f>",
            ),
        ],
    )
}

fn shape_api_model() -> Description {
    model_description(
        b"\xc9\xf3\xa3\x4a\xd8\x9f\x00",
        "checked_namespace.ApiModel",
        &[
            ModelField::new(b"\x82\xa3\x53\x7f\xf0\xdb", "name", "string"),
            ModelField::new(b"\x79\xbf\xb0\xe2\xba\x76", "anchor", "record@f4a092c4acca"),
            ModelField::non_canonical(b"\x6e\x50\xd9\x8d\x24\x31", "docstring", "option<string>"),
            ModelField::non_canonical(
                b"\xf7\xea\x2f\x6a\xdd\xd6",
                "docstring_sections",
                "option<record@fefefb4a64ba>",
            ),
            ModelField::new(b"\x4c\xc3\x59\x9f\x82\x5d", "decorators", "list<record@3681cf30afc4>"),
            ModelField::new(b"\x75\xe7\xe9\xde\xad\x7e", "type_params", "list<record@80549e0eb490>"),
            ModelField::new(b"\x9e\x27\x94\x08\x61\xd7", "traits", "list<string>"),
            ModelField::new(
                b"\x81\x2f\xff\x9c\x3c\xf4",
                "trait_adoptions",
                "list<record@520b906148ca>",
            ),
            ModelField::new(b"\xa8\x8f\xe6\xec\x61\x15", "derives", "list<string>"),
            ModelField::new(b"\xbf\xe5\xd6\x97\x16\x2d", "fields", "list<record@54738c6fe61a>"),
            ModelField::new(b"\x96\xad\xb9\x24\x52\x86", "properties", "list<record@65a8153729b5>"),
            ModelField::new(b"\x3f\x38\x67\x31\x3a\x9d", "methods", "list<record@24cedfeea4a7>"),
        ],
    )
}

fn shape_api_newtype() -> Description {
    model_description(
        b"\xa5\x8b\xe0\xc9\x86\x03\x00",
        "checked_namespace.ApiNewtype",
        &[
            ModelField::new(b"\x82\xa3\x53\x7f\xf0\xdb", "name", "string"),
            ModelField::new(b"\x79\xbf\xb0\xe2\xba\x76", "anchor", "record@f4a092c4acca"),
            ModelField::non_canonical(b"\x6e\x50\xd9\x8d\x24\x31", "docstring", "option<string>"),
            ModelField::non_canonical(
                b"\xf7\xea\x2f\x6a\xdd\xd6",
                "docstring_sections",
                "option<record@fefefb4a64ba>",
            ),
            ModelField::new(b"\x4c\xc3\x59\x9f\x82\x5d", "decorators", "list<record@3681cf30afc4>"),
            ModelField::new(b"\x75\xe7\xe9\xde\xad\x7e", "type_params", "list<record@80549e0eb490>"),
            ModelField::new(b"\x9e\x27\x94\x08\x61\xd7", "traits", "list<string>"),
            ModelField::new(
                b"\x81\x2f\xff\x9c\x3c\xf4",
                "trait_adoptions",
                "list<record@520b906148ca>",
            ),
            ModelField::new(b"\xa8\x8f\xe6\xec\x61\x15", "derives", "list<string>"),
            ModelField::new(b"\x46\xc9\x03\x9d\x0e\x71", "is_rusttype", "boolean"),
            ModelField::new(b"\x34\x2c\x8c\x61\xe6\x79", "underlying", "enum@20bf1c617420"),
            ModelField::new(b"\x98\x43\xd5\x51\x67\xfc", "checked_constructor", "option<string>"),
            ModelField::new(b"\xe2\x25\x06\x3b\x94\x4e", "constraints", "list<record@546b40d95499>"),
            ModelField::new(b"\x49\xb1\x73\x28\x06\x31", "implicit_coercion_enabled", "boolean"),
            ModelField::new(b"\x3f\x38\x67\x31\x3a\x9d", "methods", "list<record@24cedfeea4a7>"),
        ],
    )
}

fn shape_api_partial() -> Description {
    model_description(
        b"\xef\x97\x29\x37\x75\xf5\x00",
        "checked_namespace.ApiPartial",
        &[
            ModelField::new(b"\x82\xa3\x53\x7f\xf0\xdb", "name", "string"),
            ModelField::new(b"\x79\xbf\xb0\xe2\xba\x76", "anchor", "record@f4a092c4acca"),
            ModelField::new(b"\xab\x80\x1b\xe6\x56\xe0", "target_path", "list<string>"),
            ModelField::new(b"\xc6\x63\xf7\xf6\x0c\x47", "target_kind", "enum@73f5c6309ed9"),
            ModelField::new(b"\x47\x00\x15\x3d\x9b\xe6", "presets", "list<record@29479cd61f71>"),
            ModelField::new(b"\x75\xe7\xe9\xde\xad\x7e", "type_params", "list<record@80549e0eb490>"),
            ModelField::new(b"\xa2\x0b\x52\xfa\xe5\x7c", "params", "list<record@5e91aed5a32f>"),
            ModelField::new(b"\x40\x7d\xdf\x95\xec\x74", "return_type", "enum@20bf1c617420"),
            ModelField::new(b"\x4e\x13\xcb\x86\xec\x33", "is_async", "boolean"),
        ],
    )
}

fn shape_api_projected_function() -> Description {
    model_description(
        b"\x7b\x0d\xb3\x09\x24\x99\x00",
        "checked_namespace.ApiProjectedFunction",
        &[
            ModelField::new(b"\x85\x7d\xc6\xeb\xc7\xd2", "source_path", "list<string>"),
            ModelField::new(b"\xc1\x58\x49\xab\x11\xdd", "callable", "record@51a086d2006d"),
            ModelField::new(b"\x4c\xc3\x59\x9f\x82\x5d", "decorators", "list<record@3681cf30afc4>"),
        ],
    )
}

fn shape_api_property() -> Description {
    model_description(
        b"\x65\xa8\x15\x37\x29\xb5\x00",
        "checked_namespace.ApiProperty",
        &[
            ModelField::new(b"\x82\xa3\x53\x7f\xf0\xdb", "name", "string"),
            ModelField::new(b"\x0d\xee\xb8\xfa\x1d\xbb", "canonical", "option<record@f1bb070a87e5>"),
            ModelField::new(b"\x40\x7d\xdf\x95\xec\x74", "return_type", "enum@20bf1c617420"),
        ],
    )
}

fn shape_api_static() -> Description {
    model_description(
        b"\xf3\x0e\x61\xc4\x6b\xc4\x00",
        "checked_namespace.ApiStatic",
        &[
            ModelField::new(b"\x82\xa3\x53\x7f\xf0\xdb", "name", "string"),
            ModelField::new(b"\x79\xbf\xb0\xe2\xba\x76", "anchor", "record@f4a092c4acca"),
            ModelField::new(b"\x4f\x9d\xb7\xf8\xe5\x06", "ty", "enum@20bf1c617420"),
        ],
    )
}

fn shape_api_trait() -> Description {
    model_description(
        b"\xe6\x32\xe4\x53\x86\xa4\x00",
        "checked_namespace.ApiTrait",
        &[
            ModelField::new(b"\x82\xa3\x53\x7f\xf0\xdb", "name", "string"),
            ModelField::new(b"\x79\xbf\xb0\xe2\xba\x76", "anchor", "record@f4a092c4acca"),
            ModelField::non_canonical(b"\x6e\x50\xd9\x8d\x24\x31", "docstring", "option<string>"),
            ModelField::non_canonical(
                b"\xf7\xea\x2f\x6a\xdd\xd6",
                "docstring_sections",
                "option<record@fefefb4a64ba>",
            ),
            ModelField::new(b"\x4c\xc3\x59\x9f\x82\x5d", "decorators", "list<record@3681cf30afc4>"),
            ModelField::new(b"\x75\xe7\xe9\xde\xad\x7e", "type_params", "list<record@80549e0eb490>"),
            ModelField::new(b"\x4b\x4a\x86\x41\x6d\x45", "supertraits", "list<record@520b906148ca>"),
            ModelField::new(b"\xb9\x01\x4e\xaa\xb6\x26", "requires", "list<record@54738c6fe61a>"),
            ModelField::new(b"\x3f\x38\x67\x31\x3a\x9d", "methods", "list<record@24cedfeea4a7>"),
        ],
    )
}

fn shape_api_type_alias() -> Description {
    model_description(
        b"\x89\x9d\xc2\x08\x68\xa6\x00",
        "checked_namespace.ApiTypeAlias",
        &[
            ModelField::new(b"\x82\xa3\x53\x7f\xf0\xdb", "name", "string"),
            ModelField::new(b"\x79\xbf\xb0\xe2\xba\x76", "anchor", "record@f4a092c4acca"),
            ModelField::new(b"\x28\xfc\xea\x90\x7b\x58", "type_alias", "record@2fa968c701a3"),
        ],
    )
}

fn shape_canonical_identity_export() -> Description {
    model_description(
        b"\xf1\xbb\x07\x0a\x87\xe5\x00",
        "checked_namespace.CanonicalIdentityExport",
        &[
            ModelField::new(b"\x93\x59\xf3\xbb\x2e\xfa", "namespace", "enum@2fcac8fdb6a9"),
            ModelField::new(b"\x18\x1f\xdd\x46\xfc\x4a", "origin", "enum@25182230ed42"),
            ModelField::new(b"\xd6\x3b\xda\xbe\x07\xa6", "declaration_name", "string"),
            ModelField::new(b"\x0b\x27\xc1\x58\xfe\xb3", "kind", "string"),
            ModelField::new(b"\xbb\xf0\x73\xbf\x62\xdc", "declaration_span", "record@1880e9670151"),
        ],
    )
}

fn shape_canonical_identity_namespace_export() -> Description {
    enum_schema(
        b"\x2f\xca\xc8\xfd\xb6\xa9\x00",
        "checked_namespace.CanonicalIdentityNamespaceExport",
        vec![
            variant_schema(b"\x2f\x9f\xca\x1f\xf4\xf3", "OrdinaryLexical", &[]),
            variant_schema(b"\x7c\x96\x8f\xb7\x1f\x50", "Member", &[]),
            variant_schema(b"\xa8\xfc\x20\xe8\xc8\x26", "ModulePath", &[]),
        ],
    )
}

fn shape_canonical_identity_origin_export() -> Description {
    enum_schema(
        b"\x25\x18\x22\x30\xed\x42\x00",
        "checked_namespace.CanonicalIdentityOriginExport",
        vec![
            variant_schema(
                b"\x59\xde\x12\x1d\xb1\xb8",
                "Package",
                &[
                    ModelField::new(b"\xb7\x18\xf1\x35\x4f\x72", "library", "string"),
                    ModelField::new(b"\x81\x51\x94\x99\x5b\xb8", "module_path", "list<string>"),
                ],
            ),
            variant_schema(
                b"\x6e\xa0\x66\x89\xdd\x52",
                "RustCrate",
                &[ModelField::new(b"\xa0\xaf\x9f\x86\x5b\xf6", "path", "list<string>")],
            ),
            variant_schema(b"\xa6\xa5\x60\x71\xa4\x2b", "Builtin", &[]),
        ],
    )
}

fn shape_canonical_identity_span_export() -> Description {
    model_description(
        b"\x18\x80\xe9\x67\x01\x51\x00",
        "checked_namespace.CanonicalIdentitySpanExport",
        &[
            ModelField::new(b"\x16\x7f\x44\x8f\x11\xf3", "expansion", "option<binary>"),
            ModelField::non_canonical(b"\xa2\xf1\xbb\x14\xfa\xae", "start", "u64"),
            ModelField::non_canonical(b"\xe6\xb7\xae\xff\x05\xe2", "end", "u64"),
        ],
    )
}

fn shape_canonical_symbol_id() -> Description {
    model_description(
        b"\x28\xcd\xd4\x29\xf7\xec\x00",
        "checked_namespace.CanonicalSymbolId",
        &[
            ModelField::new(b"\x93\x59\xf3\xbb\x2e\xfa", "namespace", "enum@b3ba1ed741db"),
            ModelField::new(b"\x18\x1f\xdd\x46\xfc\x4a", "origin", "enum@ca6e161f17e2"),
            ModelField::new(b"\xd6\x3b\xda\xbe\x07\xa6", "declaration_name", "string"),
            ModelField::new(b"\x0b\x27\xc1\x58\xfe\xb3", "kind", "enum@a9554703fa2e"),
            ModelField::new(
                b"\xe4\x71\xce\x42\x54\x5e",
                "scope_discriminant",
                "option<record@8e43e8a12056>",
            ),
            ModelField::new(b"\xbb\xf0\x73\xbf\x62\xdc", "declaration_span", "record@74a6fa8e9339"),
        ],
    )
}

fn shape_checked_api_metadata() -> Description {
    model_description(
        b"\x3b\xa3\x24\x83\x9c\x88\x00",
        "checked_namespace.CheckedApiMetadata",
        &[
            ModelField::new(b"\x8c\x08\xb3\xc1\xc9\x90", "schema_version", "u32"),
            ModelField::new(b"\xaf\x91\x10\x67\x29\x7c", "derivable_traits", "list<string>"),
            ModelField::new(b"\x81\x51\x94\x99\x5b\xb8", "module_path", "list<string>"),
            ModelField::new(b"\x35\xe2\xc3\xde\x69\x80", "declarations", "list<enum@9708428a03cc>"),
        ],
    )
}

fn shape_checked_namespace_interface() -> Description {
    model_description(
        b"\x78\xd2\xc9\xd3\x8a\xe9\x00",
        "checked_namespace.CheckedNamespaceInterface",
        &[
            ModelField::new(NAMESPACE_SCHEMA, "schema", "u32"),
            ModelField::new(NAMESPACE_MODULE_NAME, "module_name", "string"),
            ModelField::new(NAMESPACE_MODULE_PATH, "module_path", "list<string>"),
            ModelField::new(b"\x14\xc2\x52\x9e\xb4\x49", "api", "record@3ba324839c88"),
            ModelField::new(
                b"\x13\xc7\x35\x13\x60\xf0",
                "identities",
                "map<string,record@28cdd429f7ec>",
            ),
            ModelField::new(
                b"\x17\x37\x3c\xa1\xc7\x63",
                "members",
                "map<string,record@6a5290805847>",
            ),
            ModelField::new(
                b"\x6d\x7c\x78\x90\xae\xf6",
                "constructor_defaults",
                "list<tuple<record@28cdd429f7ec,list<option<enum@154e90b7aa31>>>>",
            ),
            ModelField::new(
                b"\x04\x3a\x98\x79\x96\x75",
                "nominal_scope",
                "map<string,record@28cdd429f7ec>",
            ),
            ModelField::new(b"\x81\xb5\x3d\x1a\xc8\x77", "provider_carriers", "option<list<string>>"),
            ModelField::new(b"\x91\x72\x25\x88\xbb\x63", "compiled_libraries", "list<string>"),
            ModelField::new(
                b"\xf6\x0c\x79\xee\x37\x14",
                "compiled_providers",
                "list<record@9779a056d087>",
            ),
            ModelField::new(
                b"\x16\x79\x6a\x34\x8a\xef",
                "compiled_types",
                "map<string,record@488634673770>",
            ),
        ],
    )
}

fn shape_decorator_arg_metadata() -> Description {
    enum_schema(
        b"\x1c\xe4\x23\x0b\x70\xb4\x00",
        "checked_namespace.DecoratorArgMetadata",
        vec![
            variant_schema(
                b"\x8a\x26\xb9\x66\x1a\xb4",
                "Positional",
                &[ModelField::new(
                    b"\xcd\x42\x40\x4d\x52\xad",
                    "value",
                    "enum@dc407c002e08",
                )],
            ),
            variant_schema(
                b"\x86\x05\x60\x5a\xe0\x26",
                "Named",
                &[
                    ModelField::new(b"\x82\xa3\x53\x7f\xf0\xdb", "name", "string"),
                    ModelField::new(b"\xcd\x42\x40\x4d\x52\xad", "value", "enum@dc407c002e08"),
                ],
            ),
        ],
    )
}

fn shape_decorator_call_arg_metadata() -> Description {
    enum_schema(
        b"\x27\x26\x60\x14\xd2\x7b\x00",
        "checked_namespace.DecoratorCallArgMetadata",
        vec![
            variant_schema(
                b"\x8a\x26\xb9\x66\x1a\xb4",
                "Positional",
                &[ModelField::new(
                    b"\xcd\x42\x40\x4d\x52\xad",
                    "value",
                    "enum@dc407c002e08",
                )],
            ),
            variant_schema(
                b"\x86\x05\x60\x5a\xe0\x26",
                "Named",
                &[
                    ModelField::new(b"\x82\xa3\x53\x7f\xf0\xdb", "name", "string"),
                    ModelField::new(b"\xcd\x42\x40\x4d\x52\xad", "value", "enum@dc407c002e08"),
                ],
            ),
            variant_schema(
                b"\xd0\xa1\x64\xad\xad\x9a",
                "PositionalUnpack",
                &[ModelField::new(
                    b"\xcd\x42\x40\x4d\x52\xad",
                    "value",
                    "enum@dc407c002e08",
                )],
            ),
            variant_schema(
                b"\x9f\x92\x91\x0a\x5e\xce",
                "KeywordUnpack",
                &[ModelField::new(
                    b"\xcd\x42\x40\x4d\x52\xad",
                    "value",
                    "enum@dc407c002e08",
                )],
            ),
        ],
    )
}

fn shape_decorator_dict_entry() -> Description {
    model_description(
        b"\x28\x6b\xa6\x8b\x36\x27\x00",
        "checked_namespace.DecoratorDictEntry",
        &[
            ModelField::new(b"\x2c\x70\xe1\x2b\x7a\x06", "key", "enum@dc407c002e08"),
            ModelField::new(b"\xcd\x42\x40\x4d\x52\xad", "value", "enum@dc407c002e08"),
        ],
    )
}

fn shape_decorator_metadata() -> Description {
    model_description(
        b"\x36\x81\xcf\x30\xaf\xc4\x00",
        "checked_namespace.DecoratorMetadata",
        &[
            ModelField::new(b"\xa0\xaf\x9f\x86\x5b\xf6", "path", "list<string>"),
            ModelField::new(b"\x2a\x12\xf5\x01\x68\xc9", "source_name", "string"),
            ModelField::new(b"\x79\xbf\xb0\xe2\xba\x76", "anchor", "record@74a6fa8e9339"),
            ModelField::new(b"\xb0\x6b\xd0\xa0\xbb\xa8", "type_args", "list<enum@20bf1c617420>"),
            ModelField::new(b"\x09\x07\x72\xcf\x40\x68", "args", "list<enum@1ce4230b70b4>"),
            ModelField::new(
                b"\x80\xb1\x8c\x02\x9f\xb2",
                "decorated_callable",
                "option<record@51a086d2006d>",
            ),
        ],
    )
}

fn shape_decorator_value() -> Description {
    enum_schema(
        b"\xdc\x40\x7c\x00\x2e\x08\x00",
        "checked_namespace.DecoratorValue",
        vec![
            variant_schema(
                b"\xa6\x23\x0c\x2a\x81\xab",
                "Literal",
                &[ModelField::new(
                    b"\xcd\x42\x40\x4d\x52\xad",
                    "value",
                    "enum@609cc189aa5a",
                )],
            ),
            variant_schema(
                b"\x4f\x04\x11\xeb\xaa\xd2",
                "ConstRef",
                &[
                    ModelField::new(b"\x82\xa3\x53\x7f\xf0\xdb", "name", "string"),
                    ModelField::new(b"\xcd\x42\x40\x4d\x52\xad", "value", "option<enum@609cc189aa5a>"),
                ],
            ),
            variant_schema(
                b"\xbf\x49\x48\xa3\xdf\x1f",
                "SymbolRef",
                &[ModelField::new(b"\xa0\xaf\x9f\x86\x5b\xf6", "path", "list<string>")],
            ),
            variant_schema(
                b"\x6f\x20\x2f\x54\xa7\xb2",
                "List",
                &[ModelField::new(
                    b"\x5f\x3c\x4f\x85\x80\xd3",
                    "items",
                    "list<enum@dc407c002e08>",
                )],
            ),
            variant_schema(
                b"\x96\x11\xd3\xd8\x00\xfc",
                "Dict",
                &[ModelField::new(
                    b"\x87\xd0\x5c\xd0\x88\x17",
                    "entries",
                    "list<record@286ba68b3627>",
                )],
            ),
            variant_schema(
                b"\xd6\xe6\x45\xb7\xd2\xb2",
                "Call",
                &[
                    ModelField::new(b"\x92\xca\x4c\x0c\xed\x62", "callee", "list<string>"),
                    ModelField::new(b"\xb0\x6b\xd0\xa0\xbb\xa8", "type_args", "list<enum@20bf1c617420>"),
                    ModelField::new(b"\x09\x07\x72\xcf\x40\x68", "args", "list<enum@27266014d27b>"),
                ],
            ),
            variant_schema(
                b"\xba\xad\xdf\x70\xfb\x5d",
                "Type",
                &[ModelField::new(b"\x4f\x9d\xb7\xf8\xe5\x06", "ty", "enum@20bf1c617420")],
            ),
            variant_schema(
                b"\x54\x32\x46\x58\xe2\xeb",
                "Unsupported",
                &[ModelField::new(b"\x8f\xcf\xcc\xf6\x75\xb1", "reason", "string")],
            ),
        ],
    )
}

fn shape_default_value() -> Description {
    enum_schema(
        b"\x15\x4e\x90\xb7\xaa\x31\x00",
        "checked_namespace.DefaultValue",
        vec![
            variant_schema(b"\xa6\x23\x0c\x2a\x81\xab", "Literal", &[]),
            variant_schema(
                b"\x7a\x39\x33\xdf\x8d\x02",
                "Constructor",
                &[ModelField::new(
                    b"\xcc\x1d\x0d\xe5\xa7\x73",
                    "arg0",
                    "record@28cdd429f7ec",
                )],
            ),
        ],
    )
}

fn shape_enum_value_export() -> Description {
    enum_schema(
        b"\x1b\x2f\x92\x0f\xa3\xca\x00",
        "checked_namespace.EnumValueExport",
        vec![
            variant_schema(
                b"\x80\x84\xa5\x1b\x3c\x64",
                "Str",
                &[ModelField::new(b"\xcc\x1d\x0d\xe5\xa7\x73", "arg0", "string")],
            ),
            variant_schema(
                b"\x0b\x5f\x60\x80\x70\xc6",
                "Int",
                &[ModelField::new(b"\xcc\x1d\x0d\xe5\xa7\x73", "arg0", "i64")],
            ),
        ],
    )
}

fn shape_enum_value_type_export() -> Description {
    enum_schema(
        b"\xe7\xec\x50\xd6\x77\x34\x00",
        "checked_namespace.EnumValueTypeExport",
        vec![
            variant_schema(b"\x80\x84\xa5\x1b\x3c\x64", "Str", &[]),
            variant_schema(b"\x0b\x5f\x60\x80\x70\xc6", "Int", &[]),
        ],
    )
}

fn shape_field_export() -> Description {
    model_description(
        b"\x54\x73\x8c\x6f\xe6\x1a\x00",
        "checked_namespace.FieldExport",
        &[
            ModelField::new(b"\x82\xa3\x53\x7f\xf0\xdb", "name", "string"),
            ModelField::new(b"\x0d\xee\xb8\xfa\x1d\xbb", "canonical", "option<record@f1bb070a87e5>"),
            ModelField::new(b"\x45\xb0\x66\xb7\x9a\x4d", "record_identity", "string"),
            ModelField::new(b"\xbd\xeb\x90\xf5\x50\x61", "record_canonical", "boolean"),
            ModelField::new(b"\xcd\x45\x7f\x2e\x58\x1d", "renamed_from", "list<string>"),
            ModelField::new(
                b"\xcf\x5a\x34\xb4\x37\x09",
                "renamed_from_lifecycle",
                "option<record@7f6dccb134b0>",
            ),
            ModelField::new(b"\xda\xd0\x8f\x79\x38\xe2", "default_is_empty", "boolean"),
            ModelField::new(b"\x4f\x9d\xb7\xf8\xe5\x06", "ty", "enum@20bf1c617420"),
            ModelField::new(b"\xb0\x68\x00\x7f\x7f\x6b", "surface_type_name", "option<string>"),
            ModelField::new(b"\x7d\x2f\xb3\xda\x93\xa0", "visibility", "enum@5d5609e25e89"),
            ModelField::new(b"\x68\x2a\x77\x7c\x23\x28", "has_default", "boolean"),
            ModelField::new(b"\x37\xa8\xee\xc1\xce\x19", "default", "option<enum@002cab1e9f8c>"),
            ModelField::new(b"\x1a\x0a\x6a\x36\xca\x0a", "alias", "option<string>"),
            ModelField::new(b"\xc9\x04\x6f\x7a\x37\xad", "description", "option<string>"),
            ModelField::new(
                b"\x74\x5f\x56\x2a\xc4\x0d",
                "made_private",
                "option<record@aeee71bb1d3f>",
            ),
        ],
    )
}

fn shape_hir_source_span() -> Description {
    model_description(
        b"\x74\xa6\xfa\x8e\x93\x39\x00",
        "checked_namespace.HirSourceSpan",
        &[
            ModelField::non_canonical(b"\xa2\xf1\xbb\x14\xfa\xae", "start", "u64"),
            ModelField::non_canonical(b"\xe6\xb7\xae\xff\x05\xe2", "end", "u64"),
            ModelField::new(b"\x16\x7f\x44\x8f\x11\xf3", "expansion", "option<binary>"),
        ],
    )
}

fn shape_implementation_associated_type_export() -> Description {
    model_description(
        b"\x94\x24\x68\xd9\x78\xce\x00",
        "checked_namespace.ImplementationAssociatedTypeExport",
        &[
            ModelField::new(b"\x82\xa3\x53\x7f\xf0\xdb", "name", "string"),
            ModelField::new(b"\x4f\x9d\xb7\xf8\xe5\x06", "ty", "enum@20bf1c617420"),
        ],
    )
}

fn shape_implementation_trait_bound_export() -> Description {
    model_description(
        b"\xd7\x4a\x78\x83\x4f\xde\x00",
        "checked_namespace.ImplementationTraitBoundExport",
        &[
            ModelField::new(b"\xaf\x70\xee\xf6\x56\x07", "trait_path", "string"),
            ModelField::new(b"\xb0\x6b\xd0\xa0\xbb\xa8", "type_args", "list<enum@20bf1c617420>"),
            ModelField::new(
                b"\xee\xaa\x8f\x65\x6b\x2b",
                "associated_types",
                "list<record@942468d978ce>",
            ),
            ModelField::new(b"\x18\x1f\xdd\x46\xfc\x4a", "origin", "enum@67c4e649ba25"),
        ],
    )
}

fn shape_implementation_trait_bound_origin_export() -> Description {
    enum_schema(
        b"\x67\xc4\xe6\x49\xba\x25\x00",
        "checked_namespace.ImplementationTraitBoundOriginExport",
        vec![
            variant_schema(b"\xef\x66\x91\x54\x5d\x2c", "Standard", &[]),
            variant_schema(b"\xed\x98\x49\x10\x39\x30", "RustCapability", &[]),
            variant_schema(b"\xee\x5b\xff\x56\x22\x6d", "SourceCallable", &[]),
        ],
    )
}

fn shape_implementation_type_param_export() -> Description {
    model_description(
        b"\x6c\xc5\x0e\x51\x0d\xce\x00",
        "checked_namespace.ImplementationTypeParamExport",
        &[
            ModelField::new(b"\x82\xa3\x53\x7f\xf0\xdb", "name", "string"),
            ModelField::new(b"\x6b\xa9\x23\x48\x98\xef", "bounds", "list<record@d74a78834fde>"),
        ],
    )
}

fn shape_incan_callable_param() -> Description {
    model_description(
        b"\x1a\x38\x6a\xe7\x38\xbb\x00",
        "checked_namespace.IncanCallableParam",
        &[
            ModelField::new(b"\x82\xa3\x53\x7f\xf0\xdb", "name", "option<string>"),
            ModelField::new(b"\x4f\x9d\xb7\xf8\xe5\x06", "ty", "enum@b0e0f36c4c22"),
            ModelField::new(b"\x0b\x27\xc1\x58\xfe\xb3", "kind", "enum@adaec880f697"),
            ModelField::new(b"\x68\x2a\x77\x7c\x23\x28", "has_default", "boolean"),
            ModelField::new(b"\xc2\xea\x35\x38\x56\xa6", "is_partial_preset", "boolean"),
            ModelField::new(b"\xc5\x7d\x21\xc0\xf9\x5b", "is_mut", "boolean"),
            ModelField::new(b"\xa6\xcb\xca\x31\x7c\x1a", "native_owned", "boolean"),
            ModelField::new(b"\x2b\x23\x7b\x58\xd0\x72", "input_access", "enum@e1e24db15d4d"),
        ],
    )
}

fn shape_incan_callable_param_kind() -> Description {
    enum_schema(
        b"\xad\xae\xc8\x80\xf6\x97\x00",
        "checked_namespace.IncanCallableParamKind",
        vec![
            variant_schema(b"\xa7\x24\x8e\xeb\x45\xeb", "Normal", &[]),
            variant_schema(b"\xed\xe4\x21\xe1\x68\xfb", "RestPositional", &[]),
            variant_schema(b"\xff\xad\x05\x37\x44\x21", "RestKeyword", &[]),
        ],
    )
}

fn shape_incan_input_access() -> Description {
    enum_schema(
        b"\xe1\xe2\x4d\xb1\x5d\x4d\x00",
        "checked_namespace.IncanInputAccess",
        vec![
            variant_schema(b"\xd4\x61\xcb\xb4\x90\x66", "Inferred", &[]),
            variant_schema(b"\xe3\xc4\xb3\x9d\x6d\x50", "Shared", &[]),
            variant_schema(b"\x48\x83\xb7\xdf\xff\xb0", "Exclusive", &[]),
            variant_schema(b"\x17\xb7\x60\xc4\x1c\xd2", "Owned", &[]),
            variant_schema(b"\x31\xca\xf1\x64\xbb\x26", "InferredShared", &[]),
        ],
    )
}

fn shape_incan_primitive_type() -> Description {
    enum_schema(
        b"\x64\xe3\xbf\x3b\x13\x3a\x00",
        "checked_namespace.IncanPrimitiveType",
        vec![
            variant_schema(b"\x0b\x5f\x60\x80\x70\xc6", "Int", &[]),
            variant_schema(b"\x1a\x69\x3c\x00\xc4\x0c", "Float", &[]),
            variant_schema(
                b"\x36\x75\x2d\x6c\xd1\x47",
                "Numeric",
                &[ModelField::new(
                    b"\xcc\x1d\x0d\xe5\xa7\x73",
                    "arg0",
                    "enum@6c502ac236f1",
                )],
            ),
            variant_schema(b"\x49\xf4\x11\xf0\xa1\xa7", "Bool", &[]),
            variant_schema(b"\x80\x84\xa5\x1b\x3c\x64", "Str", &[]),
            variant_schema(b"\x4d\x80\x00\x30\x1f\xcc", "Bytes", &[]),
            variant_schema(b"\x4f\xab\xe1\x57\xe8\xf9", "FrozenStr", &[]),
            variant_schema(b"\xa5\x1a\x55\xd1\x3b\xee", "FrozenBytes", &[]),
            variant_schema(b"\x4e\x54\x59\x60\xf1\xbf", "Unit", &[]),
        ],
    )
}
