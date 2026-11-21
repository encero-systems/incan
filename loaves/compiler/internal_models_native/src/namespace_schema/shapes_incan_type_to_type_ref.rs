// Shape descriptions of the namespace closure, `incan_type` to `type_ref` in generated alphabetical order. Included by
// `namespace_schema.rs`, translated mechanically from the reference module's generated descriptions.

fn shape_incan_type() -> Description {
    enum_schema(
        b"\xb0\xe0\xf3\x6c\x4c\x22\x00",
        "checked_namespace.IncanType",
        vec![
            variant_schema(b"\x63\x00\xef\x80\x0b\xb8", "Never", &[]),
            variant_schema(
                b"\x09\x56\x17\x3c\xa4\x15",
                "Primitive",
                &[ModelField::new(
                    b"\xcc\x1d\x0d\xe5\xa7\x73",
                    "arg0",
                    "enum@64e3bf3b133a",
                )],
            ),
            variant_schema(
                b"\x86\x05\x60\x5a\xe0\x26",
                "Named",
                &[ModelField::new(b"\xcc\x1d\x0d\xe5\xa7\x73", "arg0", "string")],
            ),
            variant_schema(
                b"\x02\x28\xc6\xd4\x8e\xcf",
                "Generic",
                &[
                    ModelField::new(b"\xca\xe6\x62\x17\x2f\xd4", "base", "string"),
                    ModelField::new(b"\x09\x07\x72\xcf\x40\x68", "args", "list<enum@b0e0f36c4c22>"),
                ],
            ),
            variant_schema(
                b"\x18\x51\x7d\x33\x2d\x1b",
                "Decimal",
                &[
                    ModelField::new(b"\x68\xc2\xf9\xee\x31\x47", "precision", "u8"),
                    ModelField::new(b"\xf4\x69\x80\x2a\x31\x44", "scale", "u8"),
                ],
            ),
            variant_schema(
                b"\xc8\x03\x71\x03\x02\xd5",
                "Function",
                &[
                    ModelField::new(b"\xa2\x0b\x52\xfa\xe5\x7c", "params", "list<record@1a386ae738bb>"),
                    ModelField::new(b"\x40\x7d\xdf\x95\xec\x74", "return_type", "enum@b0e0f36c4c22"),
                ],
            ),
            variant_schema(
                b"\xeb\x90\x0d\x77\x64\xd0",
                "TypeToken",
                &[ModelField::new(
                    b"\xcc\x1d\x0d\xe5\xa7\x73",
                    "arg0",
                    "enum@b0e0f36c4c22",
                )],
            ),
            variant_schema(
                b"\xf1\x36\xee\x78\xc3\x3b",
                "Tuple",
                &[ModelField::new(
                    b"\xcc\x1d\x0d\xe5\xa7\x73",
                    "arg0",
                    "list<enum@b0e0f36c4c22>",
                )],
            ),
            variant_schema(
                b"\xc7\xa4\xa1\x41\x4b\xe4",
                "TypeVar",
                &[ModelField::new(b"\xcc\x1d\x0d\xe5\xa7\x73", "arg0", "string")],
            ),
            variant_schema(b"\x80\x30\x24\x93\xf2\xd4", "SelfType", &[]),
            variant_schema(
                b"\x9b\x6e\x88\xe8\x94\x3a",
                "Ref",
                &[ModelField::new(
                    b"\xcc\x1d\x0d\xe5\xa7\x73",
                    "arg0",
                    "enum@b0e0f36c4c22",
                )],
            ),
            variant_schema(
                b"\xff\xc4\x90\xa6\xa7\xb3",
                "RefMut",
                &[ModelField::new(
                    b"\xcc\x1d\x0d\xe5\xa7\x73",
                    "arg0",
                    "enum@b0e0f36c4c22",
                )],
            ),
            variant_schema(
                b"\xe7\x75\x7f\x0d\xdf\x56",
                "RustInteropPath",
                &[ModelField::new(b"\xcc\x1d\x0d\xe5\xa7\x73", "arg0", "string")],
            ),
            variant_schema(
                b"\x6f\xc7\x42\xf9\xdc\xbd",
                "NativeBuiltin",
                &[ModelField::new(
                    b"\xcc\x1d\x0d\xe5\xa7\x73",
                    "arg0",
                    "enum@474e41b7ad42",
                )],
            ),
            variant_schema(
                b"\x54\x32\x46\x58\xe2\xeb",
                "Unsupported",
                &[ModelField::new(b"\xcc\x1d\x0d\xe5\xa7\x73", "arg0", "string")],
            ),
            variant_schema(b"\x27\x40\x0f\xf4\xb8\x09", "Infer", &[]),
            variant_schema(b"\xb7\x64\xcd\xc0\xea\xb7", "Unknown", &[]),
            variant_schema(
                b"\x30\x3f\xb1\x29\xac\xe5",
                "NativeArray",
                &[
                    ModelField::new(b"\x44\x59\x29\x26\x72\x09", "element", "enum@b0e0f36c4c22"),
                    ModelField::new(b"\x0f\x82\xac\xa6\x6a\xf9", "length", "u64"),
                ],
            ),
            variant_schema(
                b"\x5c\x7a\x25\xe2\x9c\x52",
                "NativeTraitObject",
                &[
                    ModelField::new(b"\xa0\xaf\x9f\x86\x5b\xf6", "path", "string"),
                    ModelField::new(b"\x27\xce\x1d\x1b\xf4\x27", "send", "boolean"),
                    ModelField::new(b"\x75\xc7\x5e\xfe\x32\x7a", "sync", "boolean"),
                ],
            ),
            variant_schema(
                b"\xa6\x9d\x40\x08\x35\xc9",
                "NativePointer",
                &[
                    ModelField::new(b"\x1d\xa8\x05\xf3\xd0\x3b", "mutable", "boolean"),
                    ModelField::new(b"\x8a\xb1\xb3\x81\x08\x88", "pointee", "enum@b0e0f36c4c22"),
                ],
            ),
            variant_schema(b"\xed\x5c\x31\x43\x46\x46", "NativeCChar", &[]),
            variant_schema(
                b"\x28\xe3\xe4\x79\xe0\x87",
                "Trait",
                &[ModelField::new(
                    b"\xcc\x1d\x0d\xe5\xa7\x73",
                    "arg0",
                    "record@28cdd429f7ec",
                )],
            ),
        ],
    )
}

fn shape_lifecycle_metadata() -> Description {
    model_description(
        b"\x7f\x6d\xcc\xb1\x34\xb0\x00",
        "checked_namespace.LifecycleMetadata",
        &[
            ModelField::new(b"\x6b\x5a\x3d\xed\x01\x6a", "since", "option<string>"),
            ModelField::new(b"\xf3\x43\x3e\xc5\x56\xe6", "retire_in", "option<string>"),
            ModelField::new(b"\xab\x53\x0a\x13\xe4\x59", "message", "option<string>"),
            ModelField::new(b"\xa7\xe2\xd2\x6e\x8d\x15", "by", "option<string>"),
            ModelField::new(b"\x4a\x50\x28\x46\xd0\x70", "issue", "option<string>"),
        ],
    )
}

fn shape_loaf_identity() -> Description {
    model_description(
        b"\x97\x79\xa0\x56\xd0\x87\x00",
        "checked_namespace.LoafIdentity",
        &[
            ModelField::new(b"\x82\xa3\x53\x7f\xf0\xdb", "name", "string"),
            ModelField::new(b"\x5c\xa4\xf3\x85\x0c\xcc", "version", "string"),
            ModelField::new(b"\x0b\xf4\x74\x89\x63\x63", "digest", "string"),
            ModelField::new(b"\x54\x8c\xa5\x89\x7f\x6a", "feature_projection", "list<string>"),
        ],
    )
}

fn shape_made_private_export() -> Description {
    model_description(
        b"\xae\xee\x71\xbb\x1d\x3f\x00",
        "checked_namespace.MadePrivateExport",
        &[
            ModelField::new(b"\x6b\x5a\x3d\xed\x01\x6a", "since", "option<string>"),
            ModelField::new(b"\xf3\x43\x3e\xc5\x56\xe6", "retire_in", "option<string>"),
            ModelField::new(b"\xab\x53\x0a\x13\xe4\x59", "message", "option<string>"),
            ModelField::new(b"\xa7\xe2\xd2\x6e\x8d\x15", "by", "option<string>"),
            ModelField::new(b"\x4a\x50\x28\x46\xd0\x70", "issue", "option<string>"),
            ModelField::new(b"\x95\x71\x3e\x9c\xbd\xd1", "replacement", "option<string>"),
        ],
    )
}

fn shape_member_visibility_export() -> Description {
    enum_schema(
        b"\x5d\x56\x09\xe2\x5e\x89\x00",
        "checked_namespace.MemberVisibilityExport",
        vec![
            variant_schema(b"\xc6\x3e\xb6\x72\x0c\x6e", "Private", &[]),
            variant_schema(b"\x59\x19\x35\xb1\x5b\x1c", "Public", &[]),
        ],
    )
}

fn shape_namespace_compiled_type() -> Description {
    model_description(
        b"\x48\x86\x34\x67\x37\x70\x00",
        "checked_namespace.NamespaceCompiledType",
        &[
            ModelField::new(b"\xed\x93\x6e\x25\x5c\x4b", "dependency_key", "string"),
            ModelField::new(b"\x85\x7d\xc6\xeb\xc7\xd2", "source_path", "list<string>"),
            ModelField::new(b"\xaa\x7c\x07\xd6\x0f\x68", "scope_binding", "boolean"),
            ModelField::new(b"\x0d\xee\xb8\xfa\x1d\xbb", "canonical", "option<record@28cdd429f7ec>"),
            ModelField::new(b"\xdd\x21\x5c\x07\x2f\x9f", "selected_provider", "option<u32>"),
        ],
    )
}

fn shape_namespace_member_identities() -> Description {
    model_description(
        b"\x6a\x52\x90\x80\x58\x47\x00",
        "checked_namespace.NamespaceMemberIdentities",
        &[
            ModelField::new(
                b"\xda\xd5\x69\xe1\x01\xad",
                "variants",
                "map<string,record@28cdd429f7ec>",
            ),
            ModelField::new(b"\xbf\xe5\xd6\x97\x16\x2d", "fields", "map<string,record@28cdd429f7ec>"),
            ModelField::new(
                b"\x96\xad\xb9\x24\x52\x86",
                "properties",
                "map<string,record@28cdd429f7ec>",
            ),
            ModelField::new(
                b"\x3f\x38\x67\x31\x3a\x9d",
                "methods",
                "map<string,record@28cdd429f7ec>",
            ),
        ],
    )
}

fn shape_native_union_export() -> Description {
    model_description(
        b"\x5a\xab\x40\xc5\xd8\x37\x00",
        "checked_namespace.NativeUnionExport",
        &[
            ModelField::new(b"\x4c\x10\x29\x69\x7e\xe3", "owner", "enum@7a23bd1d95e4"),
            ModelField::new(b"\x8b\xc1\x77\xc0\x83\x61", "rust_name", "string"),
            ModelField::new(b"\x17\x37\x3c\xa1\xc7\x63", "members", "list<enum@20bf1c617420>"),
            ModelField::new(
                b"\xdd\xf4\xb4\xb7\xf4\x7f",
                "closure_members",
                "list<enum@b0e0f36c4c22>",
            ),
            ModelField::new(b"\xb5\x1b\x90\x65\x9a\x78", "closure_items", "list<string>"),
            ModelField::new(
                b"\x79\x12\x97\xbd\x1a\x04",
                "semantic_type",
                "option<enum@b0e0f36c4c22>",
            ),
            ModelField::new(
                b"\x2e\x2e\x4f\xe5\x0a\x59",
                "local_nominals",
                "map<string,record@f1bb070a87e5>",
            ),
            ModelField::new(
                b"\x7b\xc5\xae\xc2\x71\xa4",
                "checked_projection",
                "option<record@aec2fcd976fb>",
            ),
        ],
    )
}

fn shape_native_union_owner_export() -> Description {
    enum_schema(
        b"\x7a\x23\xbd\x1d\x95\xe4\x00",
        "checked_namespace.NativeUnionOwnerExport",
        vec![
            variant_schema(b"\xfc\x09\x1e\x4a\x29\x7a", "ContainingArtifact", &[]),
            variant_schema(
                b"\xdb\x38\x68\xc3\xed\x48",
                "SelectedArtifact",
                &[ModelField::new(
                    b"\xcc\x1d\x0d\xe5\xa7\x73",
                    "arg0",
                    "record@9779a056d087",
                )],
            ),
        ],
    )
}

fn shape_native_union_projection() -> Description {
    model_description(
        b"\xae\xc2\xfc\xd9\x76\xfb\x00",
        "checked_namespace.NativeUnionProjection",
        &[
            ModelField::new(b"\x61\x59\x6f\xfb\xf7\xea", "dependency_root", "string"),
            ModelField::new(b"\xf9\x21\xf8\x06\xf9\xfb", "rust_owner", "string"),
            ModelField::new(b"\x17\x37\x3c\xa1\xc7\x63", "members", "list<enum@20bf1c617420>"),
            ModelField::new(
                b"\xc5\x43\xb3\x37\xe4\x62",
                "nominal_origins",
                "map<string,record@4b2b68504951>",
            ),
        ],
    )
}

fn shape_newtype_constraint_export() -> Description {
    model_description(
        b"\x54\x6b\x40\xd9\x54\x99\x00",
        "checked_namespace.NewtypeConstraintExport",
        &[
            ModelField::new(b"\x2c\x70\xe1\x2b\x7a\x06", "key", "enum@900c4712553d"),
            ModelField::new(b"\xcd\x42\x40\x4d\x52\xad", "value", "i64"),
            ModelField::new(b"\xa5\xfd\xb3\x0d\x22\xd6", "repr", "string"),
        ],
    )
}

fn shape_newtype_constraint_key_export() -> Description {
    enum_schema(
        b"\x90\x0c\x47\x12\x55\x3d\x00",
        "checked_namespace.NewtypeConstraintKeyExport",
        vec![
            variant_schema(b"\x95\x4e\xdb\xbc\x5f\x5c", "Ge", &[]),
            variant_schema(b"\x12\xb5\xd5\x58\xbc\xfd", "Gt", &[]),
            variant_schema(b"\xa2\xfb\xc7\x54\xb9\xee", "Le", &[]),
            variant_schema(b"\xc2\x8e\x01\xd0\x9a\xdf", "Lt", &[]),
        ],
    )
}

fn shape_nominal_type_origin_export() -> Description {
    model_description(
        b"\x4b\x2b\x68\x50\x49\x51\x00",
        "checked_namespace.NominalTypeOriginExport",
        &[
            ModelField::new(b"\x5c\x4c\x19\x64\x34\x0a", "provider", "record@9779a056d087"),
            ModelField::new(b"\x0d\xee\xb8\xfa\x1d\xbb", "canonical", "record@f1bb070a87e5"),
        ],
    )
}

fn shape_numeric_type_id() -> Description {
    enum_schema(
        b"\x6c\x50\x2a\xc2\x36\xf1\x00",
        "checked_namespace.NumericTypeId",
        vec![
            variant_schema(b"\x31\x46\x62\x8c\x11\x8c", "I8", &[]),
            variant_schema(b"\xb9\xff\x3f\x6e\x71\xda", "I16", &[]),
            variant_schema(b"\xbe\x2d\x2c\xd0\x31\x7d", "I32", &[]),
            variant_schema(b"\xc1\xc7\xe0\x2d\xd6\xcb", "I64", &[]),
            variant_schema(b"\x47\xec\x6b\xda\x4d\xa3", "I128", &[]),
            variant_schema(b"\x34\x73\xd3\x8a\x12\x41", "U8", &[]),
            variant_schema(b"\xef\x09\xbc\x80\xcd\x62", "U16", &[]),
            variant_schema(b"\x80\x07\x24\x86\xf2\xea", "U32", &[]),
            variant_schema(b"\x84\xb8\x9e\x22\x45\x54", "U64", &[]),
            variant_schema(b"\x0c\x80\x0d\xc4\xf9\x49", "U128", &[]),
            variant_schema(b"\x82\x25\x5a\xee\x6c\xcc", "F32", &[]),
            variant_schema(b"\xa4\x4d\x7f\x74\xc8\x29", "F64", &[]),
            variant_schema(b"\xec\x7c\x44\x91\xf6\x86", "ISize", &[]),
            variant_schema(b"\xba\x4c\xad\x75\xa6\x5a", "USize", &[]),
            variant_schema(b"\x49\xf4\x11\xf0\xa1\xa7", "Bool", &[]),
        ],
    )
}

fn shape_ownership_contract() -> Description {
    model_description(
        b"\x42\xf2\xe7\x99\x85\xb2\x00",
        "checked_namespace.OwnershipContract",
        &[
            ModelField::new(b"\x8c\x08\xb3\xc1\xc9\x90", "schema_version", "u32"),
            ModelField::new(b"\x00\xc2\x5d\x63\x01\xa7", "modes", "list<record@622f44955c42>"),
            ModelField::new(
                b"\x30\xaf\xfa\x51\x78\x4a",
                "result_source",
                "option<record@31922dd77001>",
            ),
        ],
    )
}

fn shape_ownership_export() -> Description {
    enum_schema(
        b"\xb8\xdf\xa1\xe0\x41\x74\x00",
        "checked_namespace.OwnershipExport",
        vec![
            variant_schema(
                b"\x0e\xfd\x92\xa3\x35\x90",
                "Checked",
                &[ModelField::new(
                    b"\xcc\x1d\x0d\xe5\xa7\x73",
                    "arg0",
                    "record@42f2e79985b2",
                )],
            ),
            variant_schema(b"\x54\x32\x46\x58\xe2\xeb", "Unsupported", &[]),
        ],
    )
}

fn shape_ownership_mode() -> Description {
    enum_schema(
        b"\xf3\xf8\x60\x85\x7e\x6c\x00",
        "checked_namespace.OwnershipMode",
        vec![
            variant_schema(b"\xc8\x8d\x22\x41\x30\x95", "Borrowed", &[]),
            variant_schema(b"\xce\x70\x4e\xe8\x66\x0e", "BorrowedMut", &[]),
            variant_schema(b"\x17\xb7\x60\xc4\x1c\xd2", "Owned", &[]),
            variant_schema(b"\x71\xbf\x90\x93\x5f\xc2", "Reference", &[]),
        ],
    )
}

fn shape_param_default_call_arg_export() -> Description {
    model_description(
        b"\x37\x4d\x05\x46\x8e\x14\x00",
        "checked_namespace.ParamDefaultCallArgExport",
        &[
            ModelField::new(b"\x82\xa3\x53\x7f\xf0\xdb", "name", "option<string>"),
            ModelField::new(b"\xcd\x42\x40\x4d\x52\xad", "value", "enum@002cab1e9f8c"),
        ],
    )
}

fn shape_param_default_call_signature_export() -> Description {
    model_description(
        b"\xbc\x6d\xac\x92\x63\xb8\x00",
        "checked_namespace.ParamDefaultCallSignatureExport",
        &[
            ModelField::new(b"\xa2\x0b\x52\xfa\xe5\x7c", "params", "list<record@5e91aed5a32f>"),
            ModelField::new(b"\x40\x7d\xdf\x95\xec\x74", "return_type", "enum@20bf1c617420"),
        ],
    )
}

fn shape_param_default_dict_entry_export() -> Description {
    model_description(
        b"\x30\x1e\x27\x35\x04\x05\x00",
        "checked_namespace.ParamDefaultDictEntryExport",
        &[
            ModelField::new(b"\x2c\x70\xe1\x2b\x7a\x06", "key", "enum@002cab1e9f8c"),
            ModelField::new(b"\xcd\x42\x40\x4d\x52\xad", "value", "enum@002cab1e9f8c"),
        ],
    )
}

fn shape_param_default_export() -> Description {
    enum_schema(
        b"\x00\x2c\xab\x1e\x9f\x8c\x00",
        "checked_namespace.ParamDefaultExport",
        vec![
            variant_schema(
                b"\x0b\x5f\x60\x80\x70\xc6",
                "Int",
                &[ModelField::new(b"\xcc\x1d\x0d\xe5\xa7\x73", "arg0", "i64")],
            ),
            variant_schema(
                b"\x1a\x69\x3c\x00\xc4\x0c",
                "Float",
                &[ModelField::new(b"\xcc\x1d\x0d\xe5\xa7\x73", "arg0", "string")],
            ),
            variant_schema(
                b"\x49\xf4\x11\xf0\xa1\xa7",
                "Bool",
                &[ModelField::new(b"\xcc\x1d\x0d\xe5\xa7\x73", "arg0", "boolean")],
            ),
            variant_schema(
                b"\xb2\xef\x23\x0e\x7f\x4f",
                "String",
                &[ModelField::new(b"\xcc\x1d\x0d\xe5\xa7\x73", "arg0", "string")],
            ),
            variant_schema(
                b"\x4d\x80\x00\x30\x1f\xcc",
                "Bytes",
                &[ModelField::new(b"\xcc\x1d\x0d\xe5\xa7\x73", "arg0", "binary")],
            ),
            variant_schema(b"\xdc\x93\x7b\x59\x89\x26", "None", &[]),
            variant_schema(
                b"\x6f\x20\x2f\x54\xa7\xb2",
                "List",
                &[ModelField::new(
                    b"\xcc\x1d\x0d\xe5\xa7\x73",
                    "arg0",
                    "list<enum@002cab1e9f8c>",
                )],
            ),
            variant_schema(
                b"\x96\x11\xd3\xd8\x00\xfc",
                "Dict",
                &[ModelField::new(
                    b"\xcc\x1d\x0d\xe5\xa7\x73",
                    "arg0",
                    "list<record@301e27350405>",
                )],
            ),
            variant_schema(
                b"\x4f\x04\x11\xeb\xaa\xd2",
                "ConstRef",
                &[ModelField::new(b"\xcc\x1d\x0d\xe5\xa7\x73", "arg0", "list<string>")],
            ),
            variant_schema(
                b"\xd6\xe6\x45\xb7\xd2\xb2",
                "Call",
                &[
                    ModelField::new(b"\xa0\xaf\x9f\x86\x5b\xf6", "path", "list<string>"),
                    ModelField::new(b"\x09\x07\x72\xcf\x40\x68", "args", "list<record@374d05468e14>"),
                    ModelField::new(b"\x1a\x2f\xc2\x6d\xc7\xea", "signature", "option<record@bc6dac9263b8>"),
                ],
            ),
            variant_schema(
                b"\x5b\x36\x60\xd0\x20\x9a",
                "MethodCall",
                &[
                    ModelField::new(b"\xee\x88\x23\xd9\xd4\xb2", "receiver_path", "list<string>"),
                    ModelField::new(b"\x5b\x7e\x6b\xf2\xdc\x4a", "method", "string"),
                    ModelField::new(b"\x09\x07\x72\xcf\x40\x68", "args", "list<record@374d05468e14>"),
                ],
            ),
            variant_schema(b"\x54\x32\x46\x58\xe2\xeb", "Unsupported", &[]),
        ],
    )
}

fn shape_param_export() -> Description {
    model_description(
        b"\x5e\x91\xae\xd5\xa3\x2f\x00",
        "checked_namespace.ParamExport",
        &[
            ModelField::new(b"\x82\xa3\x53\x7f\xf0\xdb", "name", "string"),
            ModelField::new(b"\x4f\x9d\xb7\xf8\xe5\x06", "ty", "enum@20bf1c617420"),
            ModelField::new(b"\xb0\x68\x00\x7f\x7f\x6b", "surface_type_name", "option<string>"),
            ModelField::new(b"\x0b\x27\xc1\x58\xfe\xb3", "kind", "enum@98ca241812a0"),
            ModelField::new(b"\x68\x2a\x77\x7c\x23\x28", "has_default", "boolean"),
            ModelField::new(b"\xc5\x7d\x21\xc0\xf9\x5b", "is_mut", "boolean"),
            ModelField::new(b"\x37\xa8\xee\xc1\xce\x19", "default", "option<enum@002cab1e9f8c>"),
        ],
    )
}

fn shape_param_kind_export() -> Description {
    enum_schema(
        b"\x98\xca\x24\x18\x12\xa0\x00",
        "checked_namespace.ParamKindExport",
        vec![
            variant_schema(b"\xa7\x24\x8e\xeb\x45\xeb", "Normal", &[]),
            variant_schema(b"\xed\xe4\x21\xe1\x68\xfb", "RestPositional", &[]),
            variant_schema(b"\xff\xad\x05\x37\x44\x21", "RestKeyword", &[]),
        ],
    )
}

fn shape_partial_preset_export() -> Description {
    model_description(
        b"\x29\x47\x9c\xd6\x1f\x71\x00",
        "checked_namespace.PartialPresetExport",
        &[
            ModelField::new(b"\x82\xa3\x53\x7f\xf0\xdb", "name", "string"),
            ModelField::new(b"\x4f\x9d\xb7\xf8\xe5\x06", "ty", "enum@20bf1c617420"),
            ModelField::new(b"\xcd\x42\x40\x4d\x52\xad", "value", "enum@72296e3e41d6"),
        ],
    )
}

fn shape_partial_target_kind_export() -> Description {
    enum_schema(
        b"\x73\xf5\xc6\x30\x9e\xd9\x00",
        "checked_namespace.PartialTargetKindExport",
        vec![
            variant_schema(b"\xc8\x03\x71\x03\x02\xd5", "Function", &[]),
            variant_schema(b"\x7d\x5e\x92\x8d\x2e\xe0", "ModelConstructor", &[]),
            variant_schema(b"\xfd\x19\x84\xcc\x16\xfb", "ClassConstructor", &[]),
            variant_schema(b"\x72\x4d\xfd\x19\xe1\x0b", "NewtypeConstructor", &[]),
            variant_schema(b"\xa4\xd5\x0f\xb8\x54\x03", "Partial", &[]),
            variant_schema(b"\xb7\x64\xcd\xc0\xea\xb7", "Unknown", &[]),
        ],
    )
}

fn shape_preset_dict_entry_export() -> Description {
    model_description(
        b"\x03\xf9\x61\x50\x0b\x9a\x00",
        "checked_namespace.PresetDictEntryExport",
        &[
            ModelField::new(b"\x2c\x70\xe1\x2b\x7a\x06", "key", "enum@72296e3e41d6"),
            ModelField::new(b"\xcd\x42\x40\x4d\x52\xad", "value", "enum@72296e3e41d6"),
        ],
    )
}

fn shape_preset_model_field_export() -> Description {
    model_description(
        b"\x14\xd7\x34\x18\x52\x7c\x00",
        "checked_namespace.PresetModelFieldExport",
        &[
            ModelField::new(b"\x82\xa3\x53\x7f\xf0\xdb", "name", "string"),
            ModelField::new(b"\xcd\x42\x40\x4d\x52\xad", "value", "enum@72296e3e41d6"),
        ],
    )
}

fn shape_preset_value_export() -> Description {
    enum_schema(
        b"\x72\x29\x6e\x3e\x41\xd6\x00",
        "checked_namespace.PresetValueExport",
        vec![
            variant_schema(
                b"\x0b\x5f\x60\x80\x70\xc6",
                "Int",
                &[ModelField::new(b"\xcc\x1d\x0d\xe5\xa7\x73", "arg0", "i64")],
            ),
            variant_schema(
                b"\x1a\x69\x3c\x00\xc4\x0c",
                "Float",
                &[ModelField::new(b"\xcc\x1d\x0d\xe5\xa7\x73", "arg0", "string")],
            ),
            variant_schema(
                b"\x49\xf4\x11\xf0\xa1\xa7",
                "Bool",
                &[ModelField::new(b"\xcc\x1d\x0d\xe5\xa7\x73", "arg0", "boolean")],
            ),
            variant_schema(
                b"\xb2\xef\x23\x0e\x7f\x4f",
                "String",
                &[ModelField::new(b"\xcc\x1d\x0d\xe5\xa7\x73", "arg0", "string")],
            ),
            variant_schema(
                b"\x4d\x80\x00\x30\x1f\xcc",
                "Bytes",
                &[ModelField::new(b"\xcc\x1d\x0d\xe5\xa7\x73", "arg0", "binary")],
            ),
            variant_schema(b"\xdc\x93\x7b\x59\x89\x26", "None", &[]),
            variant_schema(
                b"\x6f\x20\x2f\x54\xa7\xb2",
                "List",
                &[ModelField::new(
                    b"\xcc\x1d\x0d\xe5\xa7\x73",
                    "arg0",
                    "list<enum@72296e3e41d6>",
                )],
            ),
            variant_schema(
                b"\xb6\xf6\xf3\xad\x07\xb3",
                "Set",
                &[ModelField::new(
                    b"\xcc\x1d\x0d\xe5\xa7\x73",
                    "arg0",
                    "list<enum@72296e3e41d6>",
                )],
            ),
            variant_schema(
                b"\xf1\x36\xee\x78\xc3\x3b",
                "Tuple",
                &[ModelField::new(
                    b"\xcc\x1d\x0d\xe5\xa7\x73",
                    "arg0",
                    "list<enum@72296e3e41d6>",
                )],
            ),
            variant_schema(
                b"\x96\x11\xd3\xd8\x00\xfc",
                "Dict",
                &[ModelField::new(
                    b"\xcc\x1d\x0d\xe5\xa7\x73",
                    "arg0",
                    "list<record@03f961500b9a>",
                )],
            ),
            variant_schema(
                b"\x4f\x04\x11\xeb\xaa\xd2",
                "ConstRef",
                &[ModelField::new(b"\xcc\x1d\x0d\xe5\xa7\x73", "arg0", "list<string>")],
            ),
            variant_schema(
                b"\xa5\xd9\x17\x8b\xbe\x92",
                "ModelLiteral",
                &[
                    ModelField::new(b"\x82\xa3\x53\x7f\xf0\xdb", "name", "string"),
                    ModelField::new(b"\xbf\xe5\xd6\x97\x16\x2d", "fields", "list<record@14d73418527c>"),
                ],
            ),
            variant_schema(b"\x54\x32\x46\x58\xe2\xeb", "Unsupported", &[]),
        ],
    )
}

fn shape_receiver_export() -> Description {
    enum_schema(
        b"\x01\xe4\xc4\x81\x1d\xd1\x00",
        "checked_namespace.ReceiverExport",
        vec![
            variant_schema(b"\x1e\x40\x8d\xc2\x74\x22", "Immutable", &[]),
            variant_schema(b"\xa0\xba\x6d\x8a\x37\x4a", "Mutable", &[]),
        ],
    )
}

fn shape_result_dependency() -> Description {
    model_description(
        b"\x31\x92\x2d\xd7\x70\x01\x00",
        "checked_namespace.ResultDependency",
        &[
            ModelField::new(b"\x65\x58\x83\x83\x31\xb7", "slot", "enum@c1d24d183cca"),
            ModelField::new(b"\xbf\xe5\xd6\x97\x16\x2d", "fields", "list<record@f1bb070a87e5>"),
            ModelField::new(b"\xee\x3c\x65\xb5\xc7\xf4", "aggregate", "boolean"),
        ],
    )
}

fn shape_runtime_builtin_type() -> Description {
    enum_schema(
        b"\x47\x4e\x41\xb7\xad\x42\x00",
        "checked_namespace.RuntimeBuiltinType",
        vec![variant_schema(b"\xbd\xfa\xef\x14\xbe\x91", "ValidationError", &[])],
    )
}

fn shape_safe_metadata_value() -> Description {
    enum_schema(
        b"\x60\x9c\xc1\x89\xaa\x5a\x00",
        "checked_namespace.SafeMetadataValue",
        vec![
            variant_schema(
                b"\x0b\x5f\x60\x80\x70\xc6",
                "Int",
                &[ModelField::new(b"\xcc\x1d\x0d\xe5\xa7\x73", "arg0", "i64")],
            ),
            variant_schema(
                b"\x1a\x69\x3c\x00\xc4\x0c",
                "Float",
                &[ModelField::new(b"\xcc\x1d\x0d\xe5\xa7\x73", "arg0", "fp64")],
            ),
            variant_schema(
                b"\x49\xf4\x11\xf0\xa1\xa7",
                "Bool",
                &[ModelField::new(b"\xcc\x1d\x0d\xe5\xa7\x73", "arg0", "boolean")],
            ),
            variant_schema(
                b"\xb2\xef\x23\x0e\x7f\x4f",
                "String",
                &[ModelField::new(b"\xcc\x1d\x0d\xe5\xa7\x73", "arg0", "string")],
            ),
            variant_schema(
                b"\x4d\x80\x00\x30\x1f\xcc",
                "Bytes",
                &[ModelField::new(b"\xcc\x1d\x0d\xe5\xa7\x73", "arg0", "binary")],
            ),
            variant_schema(b"\xdc\x93\x7b\x59\x89\x26", "None", &[]),
        ],
    )
}

fn shape_scope_discriminant() -> Description {
    model_description(
        b"\x8e\x43\xe8\xa1\x20\x56\x00",
        "checked_namespace.ScopeDiscriminant",
        &[ModelField::new(b"\xcd\x42\x40\x4d\x52\xad", "value", "u64")],
    )
}

fn shape_semantic_source_target_kind() -> Description {
    enum_schema(
        b"\xa9\x55\x47\x03\xfa\x2e\x00",
        "checked_namespace.SemanticSourceTargetKind",
        vec![
            variant_schema(b"\xc8\x03\x71\x03\x02\xd5", "Function", &[]),
            variant_schema(b"\x5e\x2c\x61\x4c\x23\xf0", "Model", &[]),
            variant_schema(b"\x4f\x3a\x9b\xd0\x03\x97", "Class", &[]),
            variant_schema(b"\x2e\x7e\x82\x4e\x59\x50", "Newtype", &[]),
            variant_schema(b"\x92\xe4\x38\x49\x9e\x72", "Rusttype", &[]),
            variant_schema(b"\x54\x10\x8d\xaf\xaa\xc5", "Enum", &[]),
            variant_schema(b"\x6b\x9d\x65\x54\x6f\xf3", "TypeAlias", &[]),
            variant_schema(b"\xa4\xd5\x0f\xb8\x54\x03", "Partial", &[]),
            variant_schema(b"\x3f\x19\xfe\x84\xa2\xde", "Variant", &[]),
            variant_schema(b"\x28\xe3\xe4\x79\xe0\x87", "Trait", &[]),
            variant_schema(b"\x5f\xaf\x58\xa6\x9d\xb5", "Capability", &[]),
            variant_schema(b"\xf4\x5f\xc1\xdf\xdc\x96", "Field", &[]),
            variant_schema(b"\x52\xa0\xf9\xb6\x5b\x27", "Method", &[]),
            variant_schema(b"\x40\x04\x0c\xb1\x81\xf7", "Property", &[]),
            variant_schema(b"\x10\x71\xe7\xae\x66\xc8", "Const", &[]),
            variant_schema(b"\x1d\x2c\x73\xdc\x01\xf2", "Static", &[]),
            variant_schema(b"\x8c\x31\xe6\xe7\x22\x30", "Local", &[]),
            variant_schema(b"\xdd\x06\x6e\xc1\xbf\x1d", "Parameter", &[]),
            variant_schema(b"\x86\xce\xc0\x81\xa5\x28", "Receiver", &[]),
            variant_schema(b"\xd6\x69\x9c\x8e\x0e\x17", "GenericBinder", &[]),
            variant_schema(b"\xf1\x48\x22\x4a\xe4\x4f", "Module", &[]),
            variant_schema(b"\xd2\xe4\x75\xb0\x0b\xf0", "RustItem", &[]),
            variant_schema(b"\xa6\xa5\x60\x71\xa4\x2b", "Builtin", &[]),
            variant_schema(
                b"\xf9\x7e\x9d\xa0\xe3\xb8",
                "Other",
                &[ModelField::new(b"\xcc\x1d\x0d\xe5\xa7\x73", "arg0", "string")],
            ),
        ],
    )
}

fn shape_source_anchor() -> Description {
    model_description(
        b"\xf4\xa0\x92\xc4\xac\xca\x00",
        "checked_namespace.SourceAnchor",
        &[
            ModelField::new(b"\xa5\x61\x45\x27\x0c\xe6", "id", "string"),
            ModelField::new(b"\x37\xa0\x02\x5a\xa1\xf2", "span", "record@74a6fa8e9339"),
        ],
    )
}

fn shape_source_slot() -> Description {
    enum_schema(
        b"\xc1\xd2\x4d\x18\x3c\xca\x00",
        "checked_namespace.SourceSlot",
        vec![
            variant_schema(
                b"\xdd\x06\x6e\xc1\xbf\x1d",
                "Parameter",
                &[ModelField::new(b"\xcc\x1d\x0d\xe5\xa7\x73", "arg0", "u64")],
            ),
            variant_schema(b"\x86\xce\xc0\x81\xa5\x28", "Receiver", &[]),
        ],
    )
}

fn shape_symbol_namespace() -> Description {
    enum_schema(
        b"\xb3\xba\x1e\xd7\x41\xdb\x00",
        "checked_namespace.SymbolNamespace",
        vec![
            variant_schema(b"\x2f\x9f\xca\x1f\xf4\xf3", "OrdinaryLexical", &[]),
            variant_schema(b"\x7c\x96\x8f\xb7\x1f\x50", "Member", &[]),
            variant_schema(b"\xa8\xfc\x20\xe8\xc8\x26", "ModulePath", &[]),
        ],
    )
}

fn shape_symbol_origin() -> Description {
    enum_schema(
        b"\xca\x6e\x16\x1f\x17\xe2\x00",
        "checked_namespace.SymbolOrigin",
        vec![
            variant_schema(
                b"\xf1\x48\x22\x4a\xe4\x4f",
                "Module",
                &[ModelField::new(b"\xcc\x1d\x0d\xe5\xa7\x73", "arg0", "list<string>")],
            ),
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
                &[ModelField::new(b"\xcc\x1d\x0d\xe5\xa7\x73", "arg0", "list<string>")],
            ),
            variant_schema(b"\xa6\xa5\x60\x71\xa4\x2b", "Builtin", &[]),
        ],
    )
}

fn shape_type_alias_export() -> Description {
    model_description(
        b"\x2f\xa9\x68\xc7\x01\xa3\x00",
        "checked_namespace.TypeAliasExport",
        &[
            ModelField::new(b"\x82\xa3\x53\x7f\xf0\xdb", "name", "string"),
            ModelField::new(b"\x75\xe7\xe9\xde\xad\x7e", "type_params", "list<record@80549e0eb490>"),
            ModelField::new(b"\x34\xa0\x40\x05\xbc\xaf", "target", "enum@20bf1c617420"),
        ],
    )
}

fn shape_type_bound_export() -> Description {
    model_description(
        b"\x52\x0b\x90\x61\x48\xca\x00",
        "checked_namespace.TypeBoundExport",
        &[
            ModelField::new(b"\x82\xa3\x53\x7f\xf0\xdb", "name", "string"),
            ModelField::new(b"\x2a\x12\xf5\x01\x68\xc9", "source_name", "option<string>"),
            ModelField::new(b"\x81\x51\x94\x99\x5b\xb8", "module_path", "option<list<string>>"),
            ModelField::new(b"\xb0\x6b\xd0\xa0\xbb\xa8", "type_args", "list<enum@20bf1c617420>"),
            ModelField::new(
                b"\xb5\x5b\x6a\x25\xc2\xa7",
                "implementation_type_params",
                "list<record@6cc50e510dce>",
            ),
            ModelField::new(b"\x20\x4e\xd5\x5e\x1a\xf9", "inferred", "boolean"),
        ],
    )
}

fn shape_type_component() -> Description {
    model_description(
        b"\xfb\x49\x7d\xd9\x8d\xfa\x00",
        "checked_namespace.TypeComponent",
        &[
            ModelField::new(b"\x63\x90\x36\x1d\xfd\xff", "parameter", "option<u64>"),
            ModelField::new(b"\x10\x91\x00\x87\xde\x24", "children", "list<u64>"),
        ],
    )
}

fn shape_type_param_export() -> Description {
    model_description(
        b"\x80\x54\x9e\x0e\xb4\x90\x00",
        "checked_namespace.TypeParamExport",
        &[
            ModelField::new(b"\x82\xa3\x53\x7f\xf0\xdb", "name", "string"),
            ModelField::new(b"\x6b\xa9\x23\x48\x98\xef", "bounds", "list<record@520b906148ca>"),
        ],
    )
}

fn shape_type_ref() -> Description {
    enum_schema(
        b"\x20\xbf\x1c\x61\x74\x20\x00",
        "checked_namespace.TypeRef",
        vec![
            variant_schema(
                b"\x86\x05\x60\x5a\xe0\x26",
                "Named",
                &[
                    ModelField::new(b"\x82\xa3\x53\x7f\xf0\xdb", "name", "string"),
                    ModelField::new(b"\x18\x1f\xdd\x46\xfc\x4a", "origin", "option<record@4b2b68504951>"),
                ],
            ),
            variant_schema(
                b"\x0c\x79\xa9\xc2\x22\x84",
                "Applied",
                &[
                    ModelField::new(b"\x82\xa3\x53\x7f\xf0\xdb", "name", "string"),
                    ModelField::new(b"\x09\x07\x72\xcf\x40\x68", "args", "list<enum@20bf1c617420>"),
                    ModelField::new(b"\x18\x1f\xdd\x46\xfc\x4a", "origin", "option<record@4b2b68504951>"),
                ],
            ),
            variant_schema(
                b"\xc8\x03\x71\x03\x02\xd5",
                "Function",
                &[
                    ModelField::new(b"\xa2\x0b\x52\xfa\xe5\x7c", "params", "list<enum@20bf1c617420>"),
                    ModelField::new(b"\x40\x7d\xdf\x95\xec\x74", "return_type", "enum@20bf1c617420"),
                ],
            ),
            variant_schema(
                b"\xeb\x90\x0d\x77\x64\xd0",
                "TypeToken",
                &[ModelField::new(
                    b"\x33\xbf\x6f\xbd\x7c\xd8",
                    "inner",
                    "enum@20bf1c617420",
                )],
            ),
            variant_schema(
                b"\xf1\x36\xee\x78\xc3\x3b",
                "Tuple",
                &[ModelField::new(
                    b"\xb0\xb1\x78\x93\xa5\x13",
                    "elements",
                    "list<enum@20bf1c617420>",
                )],
            ),
            variant_schema(
                b"\x80\x75\x61\xa2\x90\x4e",
                "TypeParam",
                &[ModelField::new(b"\x82\xa3\x53\x7f\xf0\xdb", "name", "string")],
            ),
            variant_schema(b"\x80\x30\x24\x93\xf2\xd4", "SelfType", &[]),
            variant_schema(
                b"\x9b\x6e\x88\xe8\x94\x3a",
                "Ref",
                &[ModelField::new(
                    b"\x33\xbf\x6f\xbd\x7c\xd8",
                    "inner",
                    "enum@20bf1c617420",
                )],
            ),
            variant_schema(
                b"\x44\x2d\xe1\x4c\xd6\x78",
                "RustPath",
                &[ModelField::new(b"\xa0\xaf\x9f\x86\x5b\xf6", "path", "string")],
            ),
            variant_schema(b"\xb7\x64\xcd\xc0\xea\xb7", "Unknown", &[]),
            variant_schema(
                b"\xda\x42\x6b\x7c\xa6\x34",
                "NativeUnion",
                &[ModelField::new(
                    b"\xcc\x1d\x0d\xe5\xa7\x73",
                    "arg0",
                    "record@5aab40c5d837",
                )],
            ),
            variant_schema(
                b"\x43\x19\xf9\x30\x2d\x74",
                "MutParam",
                &[ModelField::new(
                    b"\x33\xbf\x6f\xbd\x7c\xd8",
                    "inner",
                    "enum@20bf1c617420",
                )],
            ),
            variant_schema(
                b"\xc7\xd3\xac\xc8\x26\xa8",
                "Ownership",
                &[
                    ModelField::new(b"\xe6\x42\xb1\x29\x01\xa6", "mode", "enum@f3f860857e6c"),
                    ModelField::new(b"\x33\xbf\x6f\xbd\x7c\xd8", "inner", "enum@20bf1c617420"),
                ],
            ),
            variant_schema(
                b"\x28\xe3\xe4\x79\xe0\x87",
                "Trait",
                &[ModelField::new(
                    b"\x68\x9f\x6a\x62\x73\x84",
                    "identity",
                    "record@28cdd429f7ec",
                )],
            ),
        ],
    )
}
