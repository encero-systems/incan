//! Referenced descriptions of the native-plan profile's `layout_type_records` shapes.

use crate::wire::{Shape, ShapeBody, field, variant};

pub(crate) static SHAPE_LAYOUT_TYPE_RECORDS_STORED_INSTANCE_REQUEST: Shape = Shape {
    selector: b"x\x18\xc1\xb2k\x0b\x00",
    path: "incan.native-plan/5.layout_type_records.StoredInstanceRequest",
    body: ShapeBody::Record(&[
        field(b"\xab\xcd\x0e\xe1\xfc,", "declaration", "string"),
        field(b"\xb1\x92\x01}\xa4\x7f", "arguments", "list<enum@d306f4a06aec>"),
        field(b"\xfc\x7f\x8co\xaf\xff", "owned_callable_slots", "list<i64>"),
        field(b"\x9b\xf1\x17\x8c4{", "original_callable", "boolean"),
    ]),
};

pub(crate) static SHAPE_LAYOUT_TYPE_RECORDS_STORED_LAYOUT_CARRIER: Shape = Shape {
    selector: b".\x04\x85d\xff\xe2\x00",
    path: "incan.native-plan/5.layout_type_records.StoredLayoutCarrier",
    body: ShapeBody::Enum(&[
        variant(b"\xdcV\xdd&\x87\xa3", "Declared", &[]),
        variant(b"E\xaa\xac\xba~\xa1", "Option", &[]),
        variant(b"n}P\xe8OG", "Result", &[]),
    ]),
};

pub(crate) static SHAPE_LAYOUT_TYPE_RECORDS_STORED_LAYOUT_CHECKED_CALLABLE_PARAM: Shape = Shape {
    selector: b"\xed\x84w>\x19\x8f\x00",
    path: "incan.native-plan/5.layout_type_records.StoredLayoutCheckedCallableParam",
    body: ShapeBody::Record(&[
        field(b"\x82\xa3S\x7f\xf0\xdb", "name", "option<string>"),
        field(b"O\x9d\xb7\xf8\xe5\x06", "ty", "enum@d306f4a06aec"),
        field(b"\x0b'\xc1X\xfe\xb3", "kind", "enum@fcecb7d46f3c"),
        field(b"h*w|#(", "has_default", "boolean"),
        field(b"\xc2\xea58V\xa6", "is_partial_preset", "boolean"),
        field(b"\xc5}!\xc0\xf9[", "is_mut", "boolean"),
        field(b"\xa6\xcb\xca1|\x1a", "native_owned", "boolean"),
        field(b"+#{X\xd0r", "input_access", "enum@925ce5e8c3e4"),
    ]),
};

pub(crate) static SHAPE_LAYOUT_TYPE_RECORDS_STORED_LAYOUT_CHECKED_CALLABLE_PARAM_KIND: Shape = Shape {
    selector: b"\xfc\xec\xb7\xd4o<\x00",
    path: "incan.native-plan/5.layout_type_records.StoredLayoutCheckedCallableParamKind",
    body: ShapeBody::Enum(&[
        variant(b"\xa7$\x8e\xebE\xeb", "Normal", &[]),
        variant(b"\xed\xe4!\xe1h\xfb", "RestPositional", &[]),
        variant(b"\xff\xad\x057D!", "RestKeyword", &[]),
    ]),
};

pub(crate) static SHAPE_LAYOUT_TYPE_RECORDS_STORED_LAYOUT_CHECKED_INPUT_ACCESS: Shape = Shape {
    selector: b"\x92\x5c\xe5\xe8\xc3\xe4\x00",
    path: "incan.native-plan/5.layout_type_records.StoredLayoutCheckedInputAccess",
    body: ShapeBody::Enum(&[
        variant(b"\xd4a\xcb\xb4\x90f", "Inferred", &[]),
        variant(b"\xe3\xc4\xb3\x9dmP", "Shared", &[]),
        variant(b"H\x83\xb7\xdf\xff\xb0", "Exclusive", &[]),
        variant(b"\x17\xb7`\xc4\x1c\xd2", "Owned", &[]),
        variant(b"1\xca\xf1d\xbb&", "InferredShared", &[]),
    ]),
};

pub(crate) static SHAPE_LAYOUT_TYPE_RECORDS_STORED_LAYOUT_CHECKED_NUMERIC_TYPE: Shape = Shape {
    selector: b"\x12y\xc7\x00\xa7\xfe\x00",
    path: "incan.native-plan/5.layout_type_records.StoredLayoutCheckedNumericType",
    body: ShapeBody::Enum(&[
        variant(b"1Fb\x8c\x11\x8c", "I8", &[]),
        variant(b"\xb9\xff?nq\xda", "I16", &[]),
        variant(b"\xbe-,\xd01}", "I32", &[]),
        variant(b"\xc1\xc7\xe0-\xd6\xcb", "I64", &[]),
        variant(b"G\xeck\xdaM\xa3", "I128", &[]),
        variant(b"4s\xd3\x8a\x12A", "U8", &[]),
        variant(b"\xef\x09\xbc\x80\xcdb", "U16", &[]),
        variant(b"\x80\x07$\x86\xf2\xea", "U32", &[]),
        variant(b"\x84\xb8\x9e\x22ET", "U64", &[]),
        variant(b"\x0c\x80\x0d\xc4\xf9I", "U128", &[]),
        variant(b"\x82%Z\xeel\xcc", "F32", &[]),
        variant(b"\xa4M\x7ft\xc8)", "F64", &[]),
        variant(b"\xec|D\x91\xf6\x86", "ISize", &[]),
        variant(b"\xbaL\xadu\xa6Z", "USize", &[]),
        variant(b"I\xf4\x11\xf0\xa1\xa7", "Bool", &[]),
    ]),
};

pub(crate) static SHAPE_LAYOUT_TYPE_RECORDS_STORED_LAYOUT_CHECKED_PRIMITIVE_TYPE: Shape = Shape {
    selector: b"\x86u\xd0\x84h\xe2\x00",
    path: "incan.native-plan/5.layout_type_records.StoredLayoutCheckedPrimitiveType",
    body: ShapeBody::Enum(&[
        variant(b"\x0b_`\x80p\xc6", "Int", &[]),
        variant(b"\x1ai<\x00\xc4\x0c", "Float", &[]),
        variant(
            b"6u-l\xd1G",
            "Numeric",
            &[field(b"_\xec\xebf\xff\xc8", "0", "enum@1279c700a7fe")],
        ),
        variant(b"I\xf4\x11\xf0\xa1\xa7", "Bool", &[]),
        variant(b"\x80\x84\xa5\x1b<d", "Str", &[]),
        variant(b"M\x80\x000\x1f\xcc", "Bytes", &[]),
        variant(b"O\xab\xe1W\xe8\xf9", "FrozenStr", &[]),
        variant(b"\xa5\x1aU\xd1;\xee", "FrozenBytes", &[]),
        variant(b"NTY`\xf1\xbf", "Unit", &[]),
    ]),
};

pub(crate) static SHAPE_LAYOUT_TYPE_RECORDS_STORED_LAYOUT_CHECKED_RUNTIME_BUILTIN: Shape = Shape {
    selector: b"\xfd\xe1\x8a[\xba0\x00",
    path: "incan.native-plan/5.layout_type_records.StoredLayoutCheckedRuntimeBuiltin",
    body: ShapeBody::Enum(&[variant(b"\xbd\xfa\xef\x14\xbe\x91", "ValidationError", &[])]),
};

pub(crate) static SHAPE_LAYOUT_TYPE_RECORDS_STORED_LAYOUT_CHECKED_TYPE: Shape = Shape {
    selector: b"\xd3\x06\xf4\xa0j\xec\x00",
    path: "incan.native-plan/5.layout_type_records.StoredLayoutCheckedType",
    body: ShapeBody::Enum(&[
        variant(b"c\x00\xef\x80\x0b\xb8", "Never", &[]),
        variant(
            b"\x09V\x17<\xa4\x15",
            "Primitive",
            &[field(b"_\xec\xebf\xff\xc8", "0", "enum@8675d08468e2")],
        ),
        variant(
            b"\x86\x05`Z\xe0&",
            "Named",
            &[field(b"_\xec\xebf\xff\xc8", "0", "string")],
        ),
        variant(
            b"\x02(\xc6\xd4\x8e\xcf",
            "Generic",
            &[
                field(b"_\xec\xebf\xff\xc8", "0", "string"),
                field(b"k\x86\xb2s\xff4", "1", "list<enum@d306f4a06aec>"),
            ],
        ),
        variant(
            b"\x18Q}3-\x1b",
            "Decimal",
            &[
                field(b"_\xec\xebf\xff\xc8", "0", "u8"),
                field(b"k\x86\xb2s\xff4", "1", "u8"),
            ],
        ),
        variant(
            b"\xc8\x03q\x03\x02\xd5",
            "Function",
            &[
                field(b"_\xec\xebf\xff\xc8", "0", "list<record@ed84773e198f>"),
                field(b"k\x86\xb2s\xff4", "1", "list<enum@d306f4a06aec>"),
            ],
        ),
        variant(
            b"\xeb\x90\x0dwd\xd0",
            "TypeToken",
            &[field(b"_\xec\xebf\xff\xc8", "0", "list<enum@d306f4a06aec>")],
        ),
        variant(
            b"\xf16\xeex\xc3;",
            "Tuple",
            &[field(b"_\xec\xebf\xff\xc8", "0", "list<enum@d306f4a06aec>")],
        ),
        variant(
            b"\xc7\xa4\xa1AK\xe4",
            "TypeVar",
            &[field(b"_\xec\xebf\xff\xc8", "0", "string")],
        ),
        variant(b"\x800$\x93\xf2\xd4", "SelfType", &[]),
        variant(
            b"\x9bn\x88\xe8\x94:",
            "Ref",
            &[field(b"_\xec\xebf\xff\xc8", "0", "list<enum@d306f4a06aec>")],
        ),
        variant(
            b"\xff\xc4\x90\xa6\xa7\xb3",
            "RefMut",
            &[field(b"_\xec\xebf\xff\xc8", "0", "list<enum@d306f4a06aec>")],
        ),
        variant(
            b"\xe7u\x7f\x0d\xdfV",
            "RustInteropPath",
            &[field(b"_\xec\xebf\xff\xc8", "0", "string")],
        ),
        variant(
            b"o\xc7B\xf9\xdc\xbd",
            "NativeBuiltin",
            &[field(b"_\xec\xebf\xff\xc8", "0", "enum@fde18a5bba30")],
        ),
        variant(
            b"T2FX\xe2\xeb",
            "Unsupported",
            &[field(b"_\xec\xebf\xff\xc8", "0", "string")],
        ),
        variant(b"'@\x0f\xf4\xb8\x09", "Infer", &[]),
        variant(b"\xb7d\xcd\xc0\xea\xb7", "Unknown", &[]),
        variant(
            b"0?\xb1)\xac\xe5",
            "NativeArray",
            &[
                field(b"_\xec\xebf\xff\xc8", "0", "list<enum@d306f4a06aec>"),
                field(b"k\x86\xb2s\xff4", "1", "u64"),
            ],
        ),
        variant(
            b"\x5cz%\xe2\x9cR",
            "NativeTraitObject",
            &[
                field(b"_\xec\xebf\xff\xc8", "0", "string"),
                field(b"k\x86\xb2s\xff4", "1", "boolean"),
                field(b"\xd4s^:&^", "2", "boolean"),
            ],
        ),
        variant(
            b"\xa6\x9d@\x085\xc9",
            "NativePointer",
            &[
                field(b"_\xec\xebf\xff\xc8", "0", "boolean"),
                field(b"k\x86\xb2s\xff4", "1", "list<enum@d306f4a06aec>"),
            ],
        ),
        variant(b"\xed\x5c1CFF", "NativeCChar", &[]),
    ]),
};

pub(crate) static SHAPE_LAYOUT_TYPE_RECORDS_STORED_LAYOUT_LIST_LEAF: Shape = Shape {
    selector: b"\xdd\x8d\xb5\x93\xbd\xc0\x00",
    path: "incan.native-plan/5.layout_type_records.StoredLayoutListLeaf",
    body: ShapeBody::Enum(&[
        variant(b"\x0b_`\x80p\xc6", "Int", &[]),
        variant(b"\x1ai<\x00\xc4\x0c", "Float", &[]),
        variant(b"I\xf4\x11\xf0\xa1\xa7", "Bool", &[]),
        variant(b"\x80\x84\xa5\x1b<d", "Str", &[]),
        variant(
            b"\xf16\xeex\xc3;",
            "Tuple",
            &[field(b"_\xec\xebf\xff\xc8", "0", "list<enum@0f36ea9a02c1>")],
        ),
        variant(
            b"^,aL#\xf0",
            "Model",
            &[
                field(b"_\xec\xebf\xff\xc8", "0", "i64"),
                field(b"k\x86\xb2s\xff4", "1", "string"),
            ],
        ),
        variant(
            b"T\x10\x8d\xaf\xaa\xc5",
            "Enum",
            &[
                field(b"_\xec\xebf\xff\xc8", "0", "i64"),
                field(b"k\x86\xb2s\xff4", "1", "string"),
            ],
        ),
        variant(b"4s\xd3\x8a\x12A", "U8", &[]),
        variant(b"NTY`\xf1\xbf", "Unit", &[]),
        variant(b"\x18Q}3-\x1b", "Decimal", &[]),
        variant(b"O\xab\xe1W\xe8\xf9", "FrozenStr", &[]),
        variant(b"\xa5\x1aU\xd1;\xee", "FrozenBytes", &[]),
        variant(
            b"`ar\x85\xa4\xcb",
            "FrozenCollection",
            &[
                field(b"_\xec\xebf\xff\xc8", "0", "string"),
                field(b"k\x86\xb2s\xff4", "1", "list<enum@bf18bf9e46ed>"),
            ],
        ),
        variant(
            b"\xa7\x0a\xca\xf0X`",
            "FunctionPointer",
            &[field(b"_\xec\xebf\xff\xc8", "0", "list<enum@bf18bf9e46ed>")],
        ),
        variant(
            b"\xeb\x90\x0dwd\xd0",
            "TypeToken",
            &[field(b"_\xec\xebf\xff\xc8", "0", "list<enum@bf18bf9e46ed>")],
        ),
        variant(
            b"o /T\xa7\xb2",
            "List",
            &[
                field(b"_\xec\xebf\xff\xc8", "0", "list<enum@dd8db593bdc0>"),
                field(b"k\x86\xb2s\xff4", "1", "i64"),
            ],
        ),
        variant(
            b"\x96\x11\xd3\xd8\x00\xfc",
            "Dict",
            &[field(b"_\xec\xebf\xff\xc8", "0", "list<enum@dd8db593bdc0>")],
        ),
        variant(
            b"\xb6\xf6\xf3\xad\x07\xb3",
            "Set",
            &[field(b"_\xec\xebf\xff\xc8", "0", "list<enum@dd8db593bdc0>")],
        ),
        variant(b"\xbaL\xadu\xa6Z", "USize", &[]),
        variant(
            b"6u-l\xd1G",
            "Numeric",
            &[field(b"_\xec\xebf\xff\xc8", "0", "enum@9dabba6a3395")],
        ),
        variant(
            b"q\xbf\x90\x93_\xc2",
            "Reference",
            &[field(b"_\xec\xebf\xff\xc8", "0", "list<enum@bf18bf9e46ed>")],
        ),
    ]),
};

pub(crate) static SHAPE_LAYOUT_TYPE_RECORDS_STORED_LAYOUT_PLAN_TYPE: Shape = Shape {
    selector: b"\xbf\x18\xbf\x9eF\xed\x00",
    path: "incan.native-plan/5.layout_type_records.StoredLayoutPlanType",
    body: ShapeBody::Enum(&[
        variant(b"\x0b_`\x80p\xc6", "Int", &[]),
        variant(b"\x1ai<\x00\xc4\x0c", "Float", &[]),
        variant(b"1Fb\x8c\x11\x8c", "I8", &[]),
        variant(b"\xb9\xff?nq\xda", "I16", &[]),
        variant(b"\xbe-,\xd01}", "I32", &[]),
        variant(b"G\xeck\xdaM\xa3", "I128", &[]),
        variant(b"\xec|D\x91\xf6\x86", "ISize", &[]),
        variant(b"\xbaL\xadu\xa6Z", "USize", &[]),
        variant(b"4s\xd3\x8a\x12A", "U8", &[]),
        variant(b"\xef\x09\xbc\x80\xcdb", "U16", &[]),
        variant(b"\x80\x07$\x86\xf2\xea", "U32", &[]),
        variant(b"\x84\xb8\x9e\x22ET", "U64", &[]),
        variant(b"\x0c\x80\x0d\xc4\xf9I", "U128", &[]),
        variant(b"\x82%Z\xeel\xcc", "F32", &[]),
        variant(b"\xa4M\x7ft\xc8)", "F64", &[]),
        variant(b"I\xf4\x11\xf0\xa1\xa7", "Bool", &[]),
        variant(b"NTY`\xf1\xbf", "Unit", &[]),
        variant(
            b"\xa0?\xf2#F\x9f",
            "CheckedNumeric",
            &[field(b"_\xec\xebf\xff\xc8", "0", "enum@9dabba6a3395")],
        ),
        variant(b"c\x00\xef\x80\x0b\xb8", "Never", &[]),
        variant(b"\xe6\xf5\xc36\x16\xf4", "UnitFunction", &[]),
        variant(b"\x03\xa6\xc5\x01j\x86", "EnumTag", &[]),
        variant(b"\xc3\xf45\xc7(Q", "CheckedInt", &[]),
        variant(
            b"\xf16\xeex\xc3;",
            "Tuple",
            &[field(b"_\xec\xebf\xff\xc8", "0", "list<enum@0f36ea9a02c1>")],
        ),
        variant(b"\xb2\xef#\x0e\x7fO", "String", &[]),
        variant(
            b"\xbd\xb4\xa2v\x05;",
            "TupleRef",
            &[field(b"_\xec\xebf\xff\xc8", "0", "list<enum@0f36ea9a02c1>")],
        ),
        variant(
            b"\xaeV\xd77[\xff",
            "TupleMutRef",
            &[field(b"_\xec\xebf\xff\xc8", "0", "list<enum@0f36ea9a02c1>")],
        ),
        variant(b"\xbfkl\x9f\xdb\xc5", "StringRef", &[]),
        variant(
            b"\x97 \xde\x22\xd7\xd4",
            "ScalarRef",
            &[field(b"_\xec\xebf\xff\xc8", "0", "list<enum@bf18bf9e46ed>")],
        ),
        variant(
            b"\xcbeb\x95hA",
            "ScalarMutRef",
            &[field(b"_\xec\xebf\xff\xc8", "0", "list<enum@bf18bf9e46ed>")],
        ),
        variant(b"\xa45\x1e\xa3g\xb4", "StringMutRef", &[]),
        variant(b"\x18Q}3-\x1b", "Decimal", &[]),
        variant(b"O\xab\xe1W\xe8\xf9", "FrozenStr", &[]),
        variant(b"\xa5\x1aU\xd1;\xee", "FrozenBytes", &[]),
        variant(
            b"`ar\x85\xa4\xcb",
            "FrozenCollection",
            &[
                field(b"_\xec\xebf\xff\xc8", "0", "string"),
                field(b"k\x86\xb2s\xff4", "1", "list<enum@bf18bf9e46ed>"),
            ],
        ),
        variant(
            b"T\xf9p\x0cZ3",
            "FrozenCollectionRef",
            &[
                field(b"_\xec\xebf\xff\xc8", "0", "string"),
                field(b"k\x86\xb2s\xff4", "1", "list<enum@bf18bf9e46ed>"),
            ],
        ),
        variant(
            b"\xeb\x90\x0dwd\xd0",
            "TypeToken",
            &[field(b"_\xec\xebf\xff\xc8", "0", "list<enum@bf18bf9e46ed>")],
        ),
        variant(b"\xd5\xdf\x07x\xa5\x0a", "StaticText", &[]),
        variant(b"_\x93\x11xcg", "StaticBytes", &[]),
        variant(b"\xbd\xfa\xef\x14\xbe\x91", "ValidationError", &[]),
        variant(b"\x1ejG\x9a\xae\xf1", "ValidationErrorRef", &[]),
        variant(b"(+u\x8f\x05\x8a", "StrRef", &[]),
        variant(
            b"\x85~Q\x14\x0d\x18",
            "StringArray",
            &[field(b"_\xec\xebf\xff\xc8", "0", "i64")],
        ),
        variant(
            b"\x8a\xe3R\x0a\xe0\xcb",
            "StrArray",
            &[field(b"_\xec\xebf\xff\xc8", "0", "i64")],
        ),
        variant(
            b"\xeb2P\xd3\xfc<",
            "StringArrayRef",
            &[field(b"_\xec\xebf\xff\xc8", "0", "i64")],
        ),
        variant(
            b"\xb1\x90\x07D|\x13",
            "StrArrayRef",
            &[field(b"_\xec\xebf\xff\xc8", "0", "i64")],
        ),
        variant(
            b"T\x10\x8d\xaf\xaa\xc5",
            "Enum",
            &[
                field(b"_\xec\xebf\xff\xc8", "0", "i64"),
                field(b"k\x86\xb2s\xff4", "1", "string"),
            ],
        ),
        variant(
            b"\xc0\xa35\x8f\x22\x01",
            "EnumRef",
            &[
                field(b"_\xec\xebf\xff\xc8", "0", "i64"),
                field(b"k\x86\xb2s\xff4", "1", "string"),
            ],
        ),
        variant(
            b"zq\xac~]|",
            "EnumMutRef",
            &[
                field(b"_\xec\xebf\xff\xc8", "0", "i64"),
                field(b"k\x86\xb2s\xff4", "1", "string"),
            ],
        ),
        variant(
            b"^,aL#\xf0",
            "Model",
            &[
                field(b"_\xec\xebf\xff\xc8", "0", "i64"),
                field(b"k\x86\xb2s\xff4", "1", "string"),
            ],
        ),
        variant(
            b"C\xc3U\x17\xae\x15",
            "ModelRef",
            &[
                field(b"_\xec\xebf\xff\xc8", "0", "i64"),
                field(b"k\x86\xb2s\xff4", "1", "string"),
            ],
        ),
        variant(
            b"\xb0\x02\x8e\xa2$\x98",
            "ModelMutRef",
            &[
                field(b"_\xec\xebf\xff\xc8", "0", "i64"),
                field(b"k\x86\xb2s\xff4", "1", "string"),
            ],
        ),
        variant(b"\x10\x99\xe21t\xe8", "StringSlice", &[]),
        variant(b"\xb7\xe7\x93\x84\xb2\xfb", "StrSlice", &[]),
        variant(
            b"o /T\xa7\xb2",
            "List",
            &[
                field(b"_\xec\xebf\xff\xc8", "0", "enum@dd8db593bdc0"),
                field(b"k\x86\xb2s\xff4", "1", "i64"),
            ],
        ),
        variant(
            b"W\x16\x82\x93!\xd4",
            "ListRef",
            &[
                field(b"_\xec\xebf\xff\xc8", "0", "enum@dd8db593bdc0"),
                field(b"k\x86\xb2s\xff4", "1", "i64"),
            ],
        ),
        variant(
            b"/@\x05\xe8\xd2Y",
            "ListMutRef",
            &[
                field(b"_\xec\xebf\xff\xc8", "0", "enum@dd8db593bdc0"),
                field(b"k\x86\xb2s\xff4", "1", "i64"),
            ],
        ),
        variant(
            b"\xfb\xa4\xbc\xc3,\x87",
            "ListIntoIter",
            &[
                field(b"_\xec\xebf\xff\xc8", "0", "enum@dd8db593bdc0"),
                field(b"k\x86\xb2s\xff4", "1", "i64"),
            ],
        ),
        variant(
            b"W\xdeO;\xa8\x13",
            "ListIntoIterMutRef",
            &[
                field(b"_\xec\xebf\xff\xc8", "0", "enum@dd8db593bdc0"),
                field(b"k\x86\xb2s\xff4", "1", "i64"),
            ],
        ),
        variant(
            b"\xb6\xf6\xf3\xad\x07\xb3",
            "Set",
            &[field(b"_\xec\xebf\xff\xc8", "0", "enum@dd8db593bdc0")],
        ),
        variant(
            b"\x8a\xc6\x9b\x0e\xb7\xbb",
            "SetRef",
            &[field(b"_\xec\xebf\xff\xc8", "0", "enum@dd8db593bdc0")],
        ),
        variant(
            b"\xcaydbR ",
            "SetMutRef",
            &[field(b"_\xec\xebf\xff\xc8", "0", "enum@dd8db593bdc0")],
        ),
        variant(
            b"\x96\x11\xd3\xd8\x00\xfc",
            "Dict",
            &[
                field(b"_\xec\xebf\xff\xc8", "0", "enum@dd8db593bdc0"),
                field(b"k\x86\xb2s\xff4", "1", "enum@dd8db593bdc0"),
            ],
        ),
        variant(
            b"\xc2\x1fVr6\xdd",
            "DictRef",
            &[
                field(b"_\xec\xebf\xff\xc8", "0", "enum@dd8db593bdc0"),
                field(b"k\x86\xb2s\xff4", "1", "enum@dd8db593bdc0"),
            ],
        ),
        variant(
            b"b\xbe\xe3\xf3\xd1\xdb",
            "DictMutRef",
            &[
                field(b"_\xec\xebf\xff\xc8", "0", "enum@dd8db593bdc0"),
                field(b"k\x86\xb2s\xff4", "1", "enum@dd8db593bdc0"),
            ],
        ),
        variant(b"\xaf\xd5\xff\xa5\xa9\x15", "RangeValue", &[]),
        variant(
            b"\xda\x10<v\xc1;",
            "ZipIterator",
            &[
                field(b"_\xec\xebf\xff\xc8", "0", "enum@0f36ea9a02c1"),
                field(b"k\x86\xb2s\xff4", "1", "enum@0f36ea9a02c1"),
            ],
        ),
        variant(
            b"\xc8O\xb0\x89\x03!",
            "ZipIteratorRef",
            &[
                field(b"_\xec\xebf\xff\xc8", "0", "enum@0f36ea9a02c1"),
                field(b"k\x86\xb2s\xff4", "1", "enum@0f36ea9a02c1"),
            ],
        ),
        variant(
            b"|\x0a\x10\xd6\xf4\x8f",
            "ZipIteratorMutRef",
            &[
                field(b"_\xec\xebf\xff\xc8", "0", "enum@0f36ea9a02c1"),
                field(b"k\x86\xb2s\xff4", "1", "enum@0f36ea9a02c1"),
            ],
        ),
        variant(
            b"\x1a\xde`\xab\xe9?",
            "Generator",
            &[
                field(b"_\xec\xebf\xff\xc8", "0", "enum@dd8db593bdc0"),
                field(b"k\x86\xb2s\xff4", "1", "i64"),
            ],
        ),
        variant(
            b"A\xe1l\xc9\xd0E",
            "GeneratorMutRef",
            &[
                field(b"_\xec\xebf\xff\xc8", "0", "enum@dd8db593bdc0"),
                field(b"k\x86\xb2s\xff4", "1", "i64"),
            ],
        ),
        variant(
            b"\xb9H#:\xc32",
            "GeneratorYield",
            &[
                field(b"_\xec\xebf\xff\xc8", "0", "enum@dd8db593bdc0"),
                field(b"k\x86\xb2s\xff4", "1", "i64"),
            ],
        ),
        variant(
            b"=-X\x17O\xe0",
            "GeneratorYieldRef",
            &[
                field(b"_\xec\xebf\xff\xc8", "0", "enum@dd8db593bdc0"),
                field(b"k\x86\xb2s\xff4", "1", "i64"),
            ],
        ),
        variant(
            b"\xa7\x0a\xca\xf0X`",
            "FunctionPointer",
            &[field(b"_\xec\xebf\xff\xc8", "0", "list<enum@bf18bf9e46ed>")],
        ),
        variant(
            b"\xff\x92\x01\x14t\x9f",
            "FunctionItem",
            &[
                field(b"_\xec\xebf\xff\xc8", "0", "string"),
                field(b"k\x86\xb2s\xff4", "1", "list<enum@bf18bf9e46ed>"),
            ],
        ),
        variant(
            b"\xe9\x0da}+U",
            "Closure",
            &[field(b"_\xec\xebf\xff\xc8", "0", "list<enum@bf18bf9e46ed>")],
        ),
        variant(
            b"Y\x0f\xc7\xc6\x8e\x97",
            "NativeClosure",
            &[
                field(b"_\xec\xebf\xff\xc8", "0", "list<enum@bf18bf9e46ed>"),
                field(b"k\x86\xb2s\xff4", "1", "boolean"),
                field(b"\xd4s^:&^", "2", "boolean"),
            ],
        ),
        variant(
            b"\x5cz%\xe2\x9cR",
            "NativeTraitObject",
            &[
                field(b"_\xec\xebf\xff\xc8", "0", "string"),
                field(b"k\x86\xb2s\xff4", "1", "boolean"),
                field(b"\xd4s^:&^", "2", "boolean"),
            ],
        ),
        variant(
            b"V\xfe9\xc0\xf7\xb9",
            "SharedCallable",
            &[field(b"_\xec\xebf\xff\xc8", "0", "list<enum@bf18bf9e46ed>")],
        ),
        variant(
            b"\xd3\x9eqf\x1f.",
            "ClosureOnce",
            &[field(b"_\xec\xebf\xff\xc8", "0", "list<enum@bf18bf9e46ed>")],
        ),
        variant(
            b"\x7f&q\xb2)\xed",
            "ClosureRef",
            &[field(b"_\xec\xebf\xff\xc8", "0", "list<enum@bf18bf9e46ed>")],
        ),
        variant(
            b"`\xa1v\x08\xb0\xf4",
            "BorrowingClosure",
            &[field(b"_\xec\xebf\xff\xc8", "0", "list<enum@bf18bf9e46ed>")],
        ),
        variant(
            b"\xec\xdb\xa9E\xf7\xde",
            "BorrowingClosureRef",
            &[field(b"_\xec\xebf\xff\xc8", "0", "list<enum@bf18bf9e46ed>")],
        ),
        variant(
            b"\xb7\xe5\xaa\xe2\xfbm",
            "CallableArguments",
            &[field(b"_\xec\xebf\xff\xc8", "0", "list<enum@bf18bf9e46ed>")],
        ),
        variant(
            b"acl\xde\xf5G",
            "Future",
            &[field(b"_\xec\xebf\xff\xc8", "0", "list<enum@bf18bf9e46ed>")],
        ),
        variant(b"\x89\xd2qrhj", "NativeStr", &[]),
        variant(
            b"\x1f\x04\x02YL\xb8",
            "NativeSlice",
            &[field(b"_\xec\xebf\xff\xc8", "0", "list<enum@bf18bf9e46ed>")],
        ),
        variant(
            b"\xa1\xf0\x0b\xce\x97\xfb",
            "SliceRef",
            &[field(b"_\xec\xebf\xff\xc8", "0", "list<enum@bf18bf9e46ed>")],
        ),
        variant(
            b"\xabf`]\xf9\xcc",
            "SliceMutRef",
            &[field(b"_\xec\xebf\xff\xc8", "0", "list<enum@bf18bf9e46ed>")],
        ),
        variant(
            b"0?\xb1)\xac\xe5",
            "NativeArray",
            &[
                field(b"_\xec\xebf\xff\xc8", "0", "list<enum@bf18bf9e46ed>"),
                field(b"k\x86\xb2s\xff4", "1", "i64"),
            ],
        ),
        variant(
            b"x?\xbd{\xe2\x8f",
            "NativeArrayRef",
            &[
                field(b"_\xec\xebf\xff\xc8", "0", "list<enum@bf18bf9e46ed>"),
                field(b"k\x86\xb2s\xff4", "1", "i64"),
            ],
        ),
        variant(
            b"o\xda\x01L\xed0",
            "NativeArrayMutRef",
            &[
                field(b"_\xec\xebf\xff\xc8", "0", "list<enum@bf18bf9e46ed>"),
                field(b"k\x86\xb2s\xff4", "1", "i64"),
            ],
        ),
        variant(b"D,\xa28\xaa\xb8", "CChar", &[]),
        variant(
            b"'\xee\xe0\xd7P=",
            "RawPointer",
            &[
                field(b"_\xec\xebf\xff\xc8", "0", "boolean"),
                field(b"k\x86\xb2s\xff4", "1", "list<enum@bf18bf9e46ed>"),
            ],
        ),
        variant(
            b"\x1e\x82j\xeb<\xa2",
            "Boxed",
            &[field(b"_\xec\xebf\xff\xc8", "0", "list<enum@bf18bf9e46ed>")],
        ),
    ]),
};

pub(crate) static SHAPE_LAYOUT_TYPE_RECORDS_STORED_LAYOUT_SIZED_NUMERIC: Shape = Shape {
    selector: b"\x9d\xab\xbaj3\x95\x00",
    path: "incan.native-plan/5.layout_type_records.StoredLayoutSizedNumeric",
    body: ShapeBody::Enum(&[
        variant(b"1Fb\x8c\x11\x8c", "I8", &[]),
        variant(b"\xb9\xff?nq\xda", "I16", &[]),
        variant(b"\xbe-,\xd01}", "I32", &[]),
        variant(b"G\xeck\xdaM\xa3", "I128", &[]),
        variant(b"4s\xd3\x8a\x12A", "U8", &[]),
        variant(b"\xef\x09\xbc\x80\xcdb", "U16", &[]),
        variant(b"\x80\x07$\x86\xf2\xea", "U32", &[]),
        variant(b"\x84\xb8\x9e\x22ET", "U64", &[]),
        variant(b"\x0c\x80\x0d\xc4\xf9I", "U128", &[]),
        variant(b"\x82%Z\xeel\xcc", "F32", &[]),
        variant(b"\xa4M\x7ft\xc8)", "F64", &[]),
        variant(b"\xec|D\x91\xf6\x86", "ISize", &[]),
        variant(b"\xbaL\xadu\xa6Z", "USize", &[]),
    ]),
};

pub(crate) static SHAPE_LAYOUT_TYPE_RECORDS_STORED_LAYOUT_TUPLE_ELEMENT: Shape = Shape {
    selector: b"\x0f6\xea\x9a\x02\xc1\x00",
    path: "incan.native-plan/5.layout_type_records.StoredLayoutTupleElement",
    body: ShapeBody::Enum(&[
        variant(b"\x0b_`\x80p\xc6", "Int", &[]),
        variant(b"\x1ai<\x00\xc4\x0c", "Float", &[]),
        variant(
            b"6u-l\xd1G",
            "Numeric",
            &[field(b"_\xec\xebf\xff\xc8", "0", "enum@9dabba6a3395")],
        ),
        variant(b"I\xf4\x11\xf0\xa1\xa7", "Bool", &[]),
        variant(b"NTY`\xf1\xbf", "Unit", &[]),
        variant(b"\xb2\xef#\x0e\x7fO", "String", &[]),
        variant(
            b"o /T\xa7\xb2",
            "List",
            &[
                field(b"_\xec\xebf\xff\xc8", "0", "enum@dd8db593bdc0"),
                field(b"k\x86\xb2s\xff4", "1", "i64"),
            ],
        ),
        variant(
            b"\x96\x11\xd3\xd8\x00\xfc",
            "Dict",
            &[
                field(b"_\xec\xebf\xff\xc8", "0", "enum@dd8db593bdc0"),
                field(b"k\x86\xb2s\xff4", "1", "enum@dd8db593bdc0"),
            ],
        ),
        variant(
            b"\xb6\xf6\xf3\xad\x07\xb3",
            "Set",
            &[field(b"_\xec\xebf\xff\xc8", "0", "enum@dd8db593bdc0")],
        ),
        variant(
            b"^,aL#\xf0",
            "Model",
            &[
                field(b"_\xec\xebf\xff\xc8", "0", "i64"),
                field(b"k\x86\xb2s\xff4", "1", "string"),
            ],
        ),
        variant(
            b"T\x10\x8d\xaf\xaa\xc5",
            "Enum",
            &[
                field(b"_\xec\xebf\xff\xc8", "0", "i64"),
                field(b"k\x86\xb2s\xff4", "1", "string"),
            ],
        ),
        variant(
            b"\xf16\xeex\xc3;",
            "Tuple",
            &[field(b"_\xec\xebf\xff\xc8", "0", "list<enum@0f36ea9a02c1>")],
        ),
        variant(
            b"\xa7\x0a\xca\xf0X`",
            "FunctionPointer",
            &[field(b"_\xec\xebf\xff\xc8", "0", "list<enum@bf18bf9e46ed>")],
        ),
        variant(
            b"q\xbf\x90\x93_\xc2",
            "Reference",
            &[
                field(b"_\xec\xebf\xff\xc8", "0", "boolean"),
                field(b"k\x86\xb2s\xff4", "1", "list<enum@bf18bf9e46ed>"),
            ],
        ),
        variant(
            b"C\xc3U\x17\xae\x15",
            "ModelRef",
            &[
                field(b"_\xec\xebf\xff\xc8", "0", "i64"),
                field(b"k\x86\xb2s\xff4", "1", "string"),
            ],
        ),
        variant(
            b"\xb0\x02\x8e\xa2$\x98",
            "ModelMutRef",
            &[
                field(b"_\xec\xebf\xff\xc8", "0", "i64"),
                field(b"k\x86\xb2s\xff4", "1", "string"),
            ],
        ),
        variant(
            b"\xa1\xf0\x0b\xce\x97\xfb",
            "SliceRef",
            &[field(b"_\xec\xebf\xff\xc8", "0", "list<enum@bf18bf9e46ed>")],
        ),
        variant(
            b"\xabf`]\xf9\xcc",
            "SliceMutRef",
            &[field(b"_\xec\xebf\xff\xc8", "0", "list<enum@bf18bf9e46ed>")],
        ),
        variant(b"O\xab\xe1W\xe8\xf9", "FrozenStr", &[]),
        variant(b"\xd5\xdf\x07x\xa5\x0a", "StaticText", &[]),
        variant(b"_\x93\x11xcg", "StaticBytes", &[]),
        variant(b"\xa5\x1aU\xd1;\xee", "FrozenBytes", &[]),
        variant(
            b"`ar\x85\xa4\xcb",
            "FrozenCollection",
            &[
                field(b"_\xec\xebf\xff\xc8", "0", "string"),
                field(b"k\x86\xb2s\xff4", "1", "list<enum@bf18bf9e46ed>"),
            ],
        ),
    ]),
};
