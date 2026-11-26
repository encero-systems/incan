//! Referenced descriptions of the native-plan profile's `layout_records` shapes.

use crate::wire::{Shape, ShapeBody, field};

pub(crate) static SHAPE_LAYOUT_RECORDS_STORED_LAYOUT_ENUM_DECLARATION: Shape = Shape {
    selector: b"\x22x[\xf6\xa0\xa6\x00",
    path: "incan.native-plan/5.layout_records.StoredLayoutEnumDeclaration",
    body: ShapeBody::Record(&[
        field(b"\x82\xa3S\x7f\xf0\xdb", "name", "string"),
        field(b"\xef\xa1\xf3u\xd7a", "public", "boolean"),
        field(b"\xda\xd5i\xe1\x01\xad", "variants", "list<record@1f5612691518>"),
        field(b"\xa8\x8f\xe6\xeca\x15", "derives", "list<string>"),
        field(b"7\xa0\x02Z\xa1\xf2", "span", "record@e9253e623e14"),
        field(b"a5&\xd6\xca\x1f", "view_lifetime_count", "i64"),
        field(b"X\x03X~g\x91", "carrier", "enum@2e048564ffe2"),
        field(b"\x18&\xb5\x1c\x02V", "semantic_key", "option<enum@d306f4a06aec>"),
        field(b"p6\x9c\xac\xa4\xd8", "source_type", "string"),
        field(b"\xef\x06\xc8!DT", "member_debug", "boolean"),
        field(b"\x8a\xac\xe8\x82\x8f\xe6", "library_item", "string"),
        field(b".\xeeG\xf93\xc4", "library_type_arguments", "list<enum@bf18bf9e46ed>"),
        field(b"\x1d\x91\xf5\x83}\xa5", "raw_kind", "string"),
        field(b"\x0fF&\xf8\xea\xb1", "raw_values", "list<string>"),
        field(b";\xbbc\xd8xg", "comparison_methods", "list<string>"),
        field(b"\xb5\x1b\x90e\x9ax", "closure_items", "list<string>"),
    ]),
};

pub(crate) static SHAPE_LAYOUT_RECORDS_STORED_LAYOUT_ENUM_VARIANT: Shape = Shape {
    selector: b"\x1fV\x12i\x15\x18\x00",
    path: "incan.native-plan/5.layout_records.StoredLayoutEnumVariant",
    body: ShapeBody::Record(&[
        field(b"\x82\xa3S\x7f\xf0\xdb", "name", "string"),
        field(b"\xbf\xe5\xd6\x97\x16-", "fields", "list<enum@bf18bf9e46ed>"),
    ]),
};

pub(crate) static SHAPE_LAYOUT_RECORDS_STORED_LAYOUT_FALLIBLE_ITERATOR_METHODS: Shape = Shape {
    selector: b"@\xee\xce\xa2\xfc\x97\x00",
    path: "incan.native-plan/5.layout_records.StoredLayoutFallibleIteratorMethods",
    body: ShapeBody::Record(&[
        field(b"J3\xea\xcd_\xa6", "item", "enum@bf18bf9e46ed"),
        field(b"\xca\x00\xfc\xcf\xb4\x08", "error", "enum@bf18bf9e46ed"),
        field(b"\xc6\xc1\xc9\xa9\xc8T", "next", "string"),
    ]),
};

pub(crate) static SHAPE_LAYOUT_RECORDS_STORED_LAYOUT_FIELD_LIFECYCLE: Shape = Shape {
    selector: b"\x09\xce$\xfb\xd6\x95\x00",
    path: "incan.native-plan/5.layout_records.StoredLayoutFieldLifecycle",
    body: ShapeBody::Record(&[
        field(b"kZ=\xed\x01j", "since", "option<string>"),
        field(b"\xf3C>\xc5V\xe6", "retire_in", "option<string>"),
        field(b"\xabS\x0a\x13\xe4Y", "message", "option<string>"),
        field(b"\xa7\xe2\xd2n\x8d\x15", "by", "option<string>"),
        field(b"JP(F\xd0p", "issue", "option<string>"),
    ]),
};

pub(crate) static SHAPE_LAYOUT_RECORDS_STORED_LAYOUT_FIELD_METADATA: Shape = Shape {
    selector: b"[\xf1\x88\xbeZK\x00",
    path: "incan.native-plan/5.layout_records.StoredLayoutFieldMetadata",
    body: ShapeBody::Record(&[
        field(b"\x82\xa3S\x7f\xf0\xdb", "name", "string"),
        field(b"E\xb0f\xb7\x9aM", "record_identity", "list<i64>"),
        field(b"\xbd\xeb\x90\xf5Pa", "record_canonical", "boolean"),
        field(b"\xcdE\x7f.X\x1d", "renamed_from", "list<string>"),
        field(
            b"\xcfZ4\xb47\x09",
            "renamed_from_lifecycle",
            "option<record@09ce24fbd695>",
        ),
        field(b"ZE$YP;", "position", "i64"),
        field(b"\xda\xd0\x8fy8\xe2", "default_is_empty", "boolean"),
        field(b"\x92\x7f\x03~3p", "stored_type", "option<string>"),
        field(b"\x1a\x0aj6\xca\x0a", "alias", "option<string>"),
        field(b"\xc9\x04oz7\xad", "description", "option<string>"),
        field(b"%\x01\xa7\xdd\xe4V", "type_name", "string"),
        field(b"h*w|#(", "has_default", "boolean"),
    ]),
};

pub(crate) static SHAPE_LAYOUT_RECORDS_STORED_LAYOUT_ITERATOR_METHODS: Shape = Shape {
    selector: b"\xc3\xc7\x1c\x06&0\x00",
    path: "incan.native-plan/5.layout_records.StoredLayoutIteratorMethods",
    body: ShapeBody::Record(&[
        field(b"J3\xea\xcd_\xa6", "item", "enum@bf18bf9e46ed"),
        field(b"\xc6\xc1\xc9\xa9\xc8T", "next", "string"),
    ]),
};

pub(crate) static SHAPE_LAYOUT_RECORDS_STORED_LAYOUT_MODEL_DECLARATION: Shape = Shape {
    selector: b"\x9c\xff\xb7\x00\xc4\x1f\x00",
    path: "incan.native-plan/5.layout_records.StoredLayoutModelDeclaration",
    body: ShapeBody::Record(&[
        field(b"\x82\xa3S\x7f\xf0\xdb", "name", "string"),
        field(b"\xef\xa1\xf3u\xd7a", "public", "boolean"),
        field(b"\xbf\xe5\xd6\x97\x16-", "fields", "list<record@815e81b91483>"),
        field(b"\x0b\xc43\xf3\xaa\x0b", "field_public", "list<boolean>"),
        field(b"\xa8\x8f\xe6\xeca\x15", "derives", "list<string>"),
        field(b"7\xa0\x02Z\xa1\xf2", "span", "record@e9253e623e14"),
        field(b"a5&\xd6\xca\x1f", "view_lifetime_count", "i64"),
        field(b"*\x12\xf5\x01h\xc9", "source_name", "string"),
        field(b"\x8a\xac\xe8\x82\x8f\xe6", "library_item", "string"),
        field(b".\xeeG\xf93\xc4", "library_type_arguments", "list<enum@bf18bf9e46ed>"),
        field(b"=\x15#\x88O\xbb", "field_serde_names", "list<string>"),
        field(b"\x16\x1d\xbe\xd0\x94`", "checked_deserialize", "string"),
        field(
            b"\x1bO\xa3\x96\xb0\xa9",
            "reflection_metadata",
            "option<list<record@5bf188be5a4b>>",
        ),
        field(b"xX]\xef\x10\xd5", "field_metadata_carrier", "boolean"),
        field(b";\xbbc\xd8xg", "comparison_methods", "list<string>"),
        field(
            b"\xa9\xe5\xb8^:\xf8",
            "fallible_iterator",
            "option<record@40eecea2fc97>",
        ),
        field(b"\xef\x9b\xae\xdf\xf2\xf4", "iterator", "option<record@c3c71c062630>"),
        field(b"\xde*\xfbgVR", "callable_method", "string"),
        field(b"Zv\x81SH\xae", "callable_signature", "list<enum@bf18bf9e46ed>"),
        field(b"\xe1\xc2\xa1\x8c\xea\xaf", "string_conversion_root", "string"),
        field(b"\x9d\x9e\x1a\x05i\x17", "string_conversion_trait", "string"),
        field(
            b"\xd2y,\xbf\x1f&",
            "native_trait_adoptions",
            "list<record@fff9792bdf5c>",
        ),
        field(b"\x99\xc6EI\x8d\x81", "unavailable_native_fields", "list<i64>"),
    ]),
};

pub(crate) static SHAPE_LAYOUT_RECORDS_STORED_LAYOUT_NATIVE_TRAIT_ADOPTION: Shape = Shape {
    selector: b"\xff\xf9y+\xdf\x5c\x00",
    path: "incan.native-plan/5.layout_records.StoredLayoutNativeTraitAdoption",
    body: ShapeBody::Record(&[
        field(b"\xa0\xaf\x9f\x86[\xf6", "path", "string"),
        field(b"?8g1:\x9d", "methods", "list<record@1cbc87da9cee>"),
    ]),
};

pub(crate) static SHAPE_LAYOUT_RECORDS_STORED_LAYOUT_NATIVE_TRAIT_METHOD: Shape = Shape {
    selector: b"\x1c\xbc\x87\xda\x9c\xee\x00",
    path: "incan.native-plan/5.layout_records.StoredLayoutNativeTraitMethod",
    body: ShapeBody::Record(&[
        field(b"\x82\xa3S\x7f\xf0\xdb", "name", "string"),
        field(b"x\xf9\xac\x01\x8eU", "function", "string"),
    ]),
};

pub(crate) static SHAPE_LAYOUT_RECORDS_STORED_LAYOUT_PARAMETER: Shape = Shape {
    selector: b"\x81^\x81\xb9\x14\x83\x00",
    path: "incan.native-plan/5.layout_records.StoredLayoutParameter",
    body: ShapeBody::Record(&[
        field(b"\x82\xa3S\x7f\xf0\xdb", "name", "string"),
        field(b"O\x9d\xb7\xf8\xe5\x06", "ty", "enum@bf18bf9e46ed"),
        field(b"7\xa0\x02Z\xa1\xf2", "span", "record@e9253e623e14"),
    ]),
};

pub(crate) static SHAPE_LAYOUT_RECORDS_STORED_LAYOUT_ZIP_CARRIER: Shape = Shape {
    selector: b">i\xa6U\x16 \x00",
    path: "incan.native-plan/5.layout_records.StoredLayoutZipCarrier",
    body: ShapeBody::Record(&[
        field(b"6\x0f\x84\x03YB", "left", "enum@0f36ea9a02c1"),
        field(b"'\x04/Nn\xca", "right", "enum@0f36ea9a02c1"),
        field(b"4\xa0@\x05\xbc\xaf", "target", "enum@bf18bf9e46ed"),
    ]),
};

pub(crate) static SHAPE_LAYOUT_RECORDS_STORED_SPAN: Shape = Shape {
    selector: b"\xe9%>b>\x14\x00",
    path: "incan.native-plan/5.layout_records.StoredSpan",
    body: ShapeBody::Record(&[
        field(b";\x9c5\x8f6\xf0", "file", "string"),
        field(b"8\xa9\xc1\xe7!X", "line", "i64"),
        field(b"\x88\x1d_\xb6\x00\x90", "column", "i64"),
    ]),
};
