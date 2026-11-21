//! Referenced descriptions of the native-plan profile's `body_records` shapes.

use crate::wire::{Shape, ShapeBody, field, variant};

pub(crate) static SHAPE_BODY_RECORDS_STORED_CONSTANT: Shape = Shape {
    selector: b"\xbc\x85PCe\xc3\x00",
    path: "incan.native-plan/5.body_records.StoredConstant",
    body: ShapeBody::Enum(&[
        variant(b"\x0b_`\x80p\xc6", "Int", &[field(b"_\xec\xebf\xff\xc8", "0", "i64")]),
        variant(
            b"\x1ai<\x00\xc4\x0c",
            "Float",
            &[field(b"_\xec\xebf\xff\xc8", "0", "fp64")],
        ),
        variant(
            b"I\xf4\x11\xf0\xa1\xa7",
            "Bool",
            &[field(b"_\xec\xebf\xff\xc8", "0", "boolean")],
        ),
        variant(b"NTY`\xf1\xbf", "Unit", &[]),
        variant(
            b"q\x98\x8cM\x8e\x08",
            "Text",
            &[field(b"_\xec\xebf\xff\xc8", "0", "string")],
        ),
        variant(
            b"6u-l\xd1G",
            "Numeric",
            &[
                field(b"_\xec\xebf\xff\xc8", "0", "string"),
                field(b"k\x86\xb2s\xff4", "1", "enum@234b6c9b62c7"),
            ],
        ),
        variant(
            b"\xeb\x90\x0dwd\xd0",
            "TypeToken",
            &[field(b"_\xec\xebf\xff\xc8", "0", "list<enum@234b6c9b62c7>")],
        ),
    ]),
};

pub(crate) static SHAPE_BODY_RECORDS_STORED_FIELD_STEP: Shape = Shape {
    selector: b"[\x17\xf9\xc5r\xab\x00",
    path: "incan.native-plan/5.body_records.StoredFieldStep",
    body: ShapeBody::Record(&[
        field(b"eX\x83\x831\xb7", "slot", "i64"),
        field(b"O\x9d\xb7\xf8\xe5\x06", "ty", "enum@234b6c9b62c7"),
        field(b"\xcb{\xf5bB\x0b", "variant", "i64"),
        field(b"3\x8e0\xd7\xaa\x17", "dereferences", "i64"),
    ]),
};

pub(crate) static SHAPE_BODY_RECORDS_STORED_FROZEN_VALUE: Shape = Shape {
    selector: b"\xe3F!u\xe3\xf6\x00",
    path: "incan.native-plan/5.body_records.StoredFrozenValue",
    body: ShapeBody::Enum(&[
        variant(
            b"C\x81\x9c`\xb3\xbd",
            "Scalar",
            &[field(b"_\xec\xebf\xff\xc8", "0", "enum@bc85504365c3")],
        ),
        variant(
            b"M\x80\x000\x1f\xcc",
            "Bytes",
            &[field(b"_\xec\xebf\xff\xc8", "0", "list<i64>")],
        ),
        variant(
            b"{y\x07\x08\xff\x1b",
            "Collection",
            &[
                field(b"_\xec\xebf\xff\xc8", "0", "enum@234b6c9b62c7"),
                field(b"k\x86\xb2s\xff4", "1", "list<enum@e3462175e3f6>"),
            ],
        ),
    ]),
};

pub(crate) static SHAPE_BODY_RECORDS_STORED_OPERAND: Shape = Shape {
    selector: b"Y\x93\xb8%\xe2#\x00",
    path: "incan.native-plan/5.body_records.StoredOperand",
    body: ShapeBody::Record(&[
        field(b"\x0b'\xc1X\xfe\xb3", "kind", "enum@da16cd5be698"),
        field(b"7\xa0\x02Z\xa1\xf2", "span", "record@533bfbed0457"),
    ]),
};

pub(crate) static SHAPE_BODY_RECORDS_STORED_PLACE: Shape = Shape {
    selector: b"\x11\xe5\xb2\xef\xd5?\x00",
    path: "incan.native-plan/5.body_records.StoredPlace",
    body: ShapeBody::Record(&[
        field(b"%\xbf\x8e\x1a#\x93", "local", "i64"),
        field(b"\x1b%\x0e\xa1\x99\xbe", "projection", "enum@7b687d4be52b"),
        field(b"7\xa0\x02Z\xa1\xf2", "span", "record@533bfbed0457"),
    ]),
};

pub(crate) static SHAPE_BODY_RECORDS_STORED_PROJECTION: Shape = Shape {
    selector: b"{h}K\xe5+\x00",
    path: "incan.native-plan/5.body_records.StoredProjection",
    body: ShapeBody::Enum(&[
        variant(b"\xfdU\x05`\x93\x8c", "Whole", &[]),
        variant(b"\x8e7\x95=#\xda", "Value", &[]),
        variant(b"\x17\xb8\xdc\xf5>K", "Overflow", &[]),
        variant(
            b"\x04I\xf5\xd2N?",
            "NumericValue",
            &[field(b"_\xec\xebf\xff\xc8", "0", "enum@234b6c9b62c7")],
        ),
        variant(
            b"\xc0\x01\x8e\x9e\x9f\x16",
            "VariantField",
            &[
                field(b"_\xec\xebf\xff\xc8", "0", "i64"),
                field(b"k\x86\xb2s\xff4", "1", "i64"),
                field(b"\xd4s^:&^", "2", "enum@234b6c9b62c7"),
            ],
        ),
        variant(
            b"\xf4_\xc1\xdf\xdc\x96",
            "Field",
            &[
                field(b"_\xec\xebf\xff\xc8", "0", "i64"),
                field(b"k\x86\xb2s\xff4", "1", "enum@234b6c9b62c7"),
            ],
        ),
        variant(
            b"\x12/\x9b\xd7h\xa6",
            "Deref",
            &[field(b"_\xec\xebf\xff\xc8", "0", "enum@234b6c9b62c7")],
        ),
        variant(
            b"\x84\xdd\xaa\x99\xf0\x90",
            "DerefField",
            &[
                field(b"_\xec\xebf\xff\xc8", "0", "i64"),
                field(b"k\x86\xb2s\xff4", "1", "enum@234b6c9b62c7"),
            ],
        ),
        variant(
            b"aoH\xc1\xdb\x19",
            "Fields",
            &[field(b"_\xec\xebf\xff\xc8", "0", "list<record@5b17f9c572ab>")],
        ),
        variant(
            b"g\x93\xd8#\xbc\xbe",
            "DerefFields",
            &[field(b"_\xec\xebf\xff\xc8", "0", "list<record@5b17f9c572ab>")],
        ),
    ]),
};

pub(crate) static SHAPE_BODY_RECORDS_STORED_READ: Shape = Shape {
    selector: b"\xda\x16\xcd[\xe6\x98\x00",
    path: "incan.native-plan/5.body_records.StoredRead",
    body: ShapeBody::Enum(&[
        variant(
            b"\xe2\x1f\x93_\x11\xd7",
            "Copy",
            &[field(b"_\xec\xebf\xff\xc8", "0", "record@11e5b2efd53f")],
        ),
        variant(
            b"n\xcc=\xf6\xbf\xfd",
            "Move",
            &[field(b"_\xec\xebf\xff\xc8", "0", "record@11e5b2efd53f")],
        ),
        variant(
            b"\xa6#\x0c*\x81\xab",
            "Literal",
            &[field(b"_\xec\xebf\xff\xc8", "0", "enum@bc85504365c3")],
        ),
    ]),
};

pub(crate) static SHAPE_BODY_RECORDS_STORED_RVALUE: Shape = Shape {
    selector: b"\x01-\xdf\x05\x08<\x00",
    path: "incan.native-plan/5.body_records.StoredRvalue",
    body: ShapeBody::Enum(&[
        variant(
            b"\xc3m\x81\x9e{\xc6",
            "Use",
            &[field(b"_\xec\xebf\xff\xc8", "0", "record@5993b825e223")],
        ),
        variant(
            b"\xee\x1c\x82w\xca\xf5",
            "Binary",
            &[
                field(b"_\xec\xebf\xff\xc8", "0", "i64"),
                field(b"k\x86\xb2s\xff4", "1", "record@5993b825e223"),
                field(b"\xd4s^:&^", "2", "record@5993b825e223"),
            ],
        ),
        variant(
            b"~\x98\x83P\x9d\x02",
            "Unary",
            &[
                field(b"_\xec\xebf\xff\xc8", "0", "i64"),
                field(b"k\x86\xb2s\xff4", "1", "record@5993b825e223"),
            ],
        ),
        variant(
            b"9\x15+\x9f}\xe3",
            "IntToFloat",
            &[field(b"_\xec\xebf\xff\xc8", "0", "record@5993b825e223")],
        ),
        variant(
            b"\x8b\xc2\xe2\xc7a\x1f",
            "FloatToInt",
            &[field(b"_\xec\xebf\xff\xc8", "0", "record@5993b825e223")],
        ),
        variant(
            b"01\xa6\xb1\xeb^",
            "BoolToInt",
            &[field(b"_\xec\xebf\xff\xc8", "0", "record@5993b825e223")],
        ),
        variant(
            b"\xf0\xcf9\xd0\xbe>",
            "Array",
            &[field(b"_\xec\xebf\xff\xc8", "0", "list<record@5993b825e223>")],
        ),
        variant(
            b".\xdf\xb1h\xb3\x8b",
            "Borrow",
            &[field(b"_\xec\xebf\xff\xc8", "0", "record@11e5b2efd53f")],
        ),
        variant(
            b"\xecWL\xe3\xa7d",
            "UnsizeSlice",
            &[field(b"_\xec\xebf\xff\xc8", "0", "record@5993b825e223")],
        ),
        variant(
            b"\xdd\xb0\x0b=6\xb4",
            "NativeAlias",
            &[
                field(b"_\xec\xebf\xff\xc8", "0", "record@5993b825e223"),
                field(b"k\x86\xb2s\xff4", "1", "enum@234b6c9b62c7"),
                field(b"\xd4s^:&^", "2", "enum@234b6c9b62c7"),
            ],
        ),
        variant(
            b"\x0fAV\x17\xca$",
            "FrozenText",
            &[field(b"_\xec\xebf\xff\xc8", "0", "record@5993b825e223")],
        ),
        variant(
            b"\xa5\x1aU\xd1;\xee",
            "FrozenBytes",
            &[field(b"_\xec\xebf\xff\xc8", "0", "list<i64>")],
        ),
        variant(
            b"\xdeJ\xe4\xd3\x93\xf0",
            "NumericCast",
            &[
                field(b"_\xec\xebf\xff\xc8", "0", "record@5993b825e223"),
                field(b"k\x86\xb2s\xff4", "1", "enum@234b6c9b62c7"),
                field(b"\xd4s^:&^", "2", "enum@234b6c9b62c7"),
            ],
        ),
        variant(
            b"T\x10\x8d\xaf\xaa\xc5",
            "Enum",
            &[
                field(b"_\xec\xebf\xff\xc8", "0", "i64"),
                field(b"k\x86\xb2s\xff4", "1", "i64"),
                field(b"\xd4s^:&^", "2", "list<record@5993b825e223>"),
            ],
        ),
        variant(
            b"x\xbc\x9c&\x03\x1a",
            "Discriminant",
            &[field(b"_\xec\xebf\xff\xc8", "0", "record@11e5b2efd53f")],
        ),
        variant(
            b"\xe7#\xb0\xbd\x03\xf7",
            "TagToInt",
            &[field(b"_\xec\xebf\xff\xc8", "0", "record@5993b825e223")],
        ),
        variant(
            b"^,aL#\xf0",
            "Model",
            &[
                field(b"_\xec\xebf\xff\xc8", "0", "i64"),
                field(b"k\x86\xb2s\xff4", "1", "list<record@5993b825e223>"),
            ],
        ),
        variant(
            b"\xf16\xeex\xc3;",
            "Tuple",
            &[field(b"_\xec\xebf\xff\xc8", "0", "list<record@5993b825e223>")],
        ),
        variant(
            b"]\xe7J\x81\xb1\x92",
            "Range",
            &[field(b"_\xec\xebf\xff\xc8", "0", "list<record@5993b825e223>")],
        ),
        variant(
            b"\xa87!2_\xde",
            "MutBorrow",
            &[field(b"_\xec\xebf\xff\xc8", "0", "record@11e5b2efd53f")],
        ),
        variant(
            b"ivP2h\xd9",
            "IsVariant",
            &[
                field(b"_\xec\xebf\xff\xc8", "0", "record@11e5b2efd53f"),
                field(b"k\x86\xb2s\xff4", "1", "i64"),
            ],
        ),
        variant(
            b"S\xe4\x16\xfd%\xcf",
            "ReifyFunction",
            &[field(b"_\xec\xebf\xff\xc8", "0", "record@5993b825e223")],
        ),
        variant(
            b"\xc4\xe4\xd4\xc6~\xe2",
            "FunctionAddress",
            &[field(b"_\xec\xebf\xff\xc8", "0", "record@5993b825e223")],
        ),
        variant(
            b"s!\xe9q\x11\xd7",
            "ClosureObject",
            &[field(b"_\xec\xebf\xff\xc8", "0", "record@11e5b2efd53f")],
        ),
        variant(
            b"^H\x983G\x01",
            "PlannedFunctionItem",
            &[
                field(b"_\xec\xebf\xff\xc8", "0", "string"),
                field(b"k\x86\xb2s\xff4", "1", "record@533bfbed0457"),
            ],
        ),
        variant(
            b"`ar\x85\xa4\xcb",
            "FrozenCollection",
            &[
                field(b"_\xec\xebf\xff\xc8", "0", "enum@234b6c9b62c7"),
                field(b"k\x86\xb2s\xff4", "1", "list<enum@e3462175e3f6>"),
            ],
        ),
    ]),
};

pub(crate) static SHAPE_BODY_RECORDS_STORED_SPAN: Shape = Shape {
    selector: b"S;\xfb\xed\x04W\x00",
    path: "incan.native-plan/5.body_records.StoredSpan",
    body: ShapeBody::Record(&[
        field(b";\x9c5\x8f6\xf0", "file", "string"),
        field(b"8\xa9\xc1\xe7!X", "line", "i64"),
        field(b"\x88\x1d_\xb6\x00\x90", "column", "i64"),
    ]),
};

pub(crate) static SHAPE_BODY_RECORDS_STORED_STATEMENT: Shape = Shape {
    selector: b" D\xfc\xbf0\xe3\x00",
    path: "incan.native-plan/5.body_records.StoredStatement",
    body: ShapeBody::Record(&[
        field(b"\x0b'\xc1X\xfe\xb3", "kind", "enum@ff331f2df5c8"),
        field(b"7\xa0\x02Z\xa1\xf2", "span", "record@533bfbed0457"),
    ]),
};

pub(crate) static SHAPE_BODY_RECORDS_STORED_STATEMENT_KIND: Shape = Shape {
    selector: b"\xff3\x1f-\xf5\xc8\x00",
    path: "incan.native-plan/5.body_records.StoredStatementKind",
    body: ShapeBody::Enum(&[
        variant(
            b"\x8e\xce\x89\x5c\x9b2",
            "Assign",
            &[
                field(b"_\xec\xebf\xff\xc8", "0", "record@11e5b2efd53f"),
                field(b"k\x86\xb2s\xff4", "1", "enum@012ddf05083c"),
                field(b"\xd4s^:&^", "2", "record@533bfbed0457"),
            ],
        ),
        variant(
            b"\xb6J\xc0_\x17\xe6",
            "Live",
            &[field(b"_\xec\xebf\xff\xc8", "0", "i64")],
        ),
        variant(
            b"\xec\x9b\x10\xa4\xa7\x9c",
            "Dead",
            &[field(b"_\xec\xebf\xff\xc8", "0", "i64")],
        ),
    ]),
};

pub(crate) static SHAPE_BODY_RECORDS_STORED_TYPE: Shape = Shape {
    selector: b"#Kl\x9bb\xc7\x00",
    path: "incan.native-plan/5.body_records.StoredType",
    body: ShapeBody::Enum(&[
        variant(b"\x0b_`\x80p\xc6", "Int", &[]),
        variant(b"\x1ai<\x00\xc4\x0c", "Float", &[]),
        variant(b"I\xf4\x11\xf0\xa1\xa7", "Bool", &[]),
        variant(b"NTY`\xf1\xbf", "Unit", &[]),
        variant(b"\xc3\xf45\xc7(Q", "CheckedInt", &[]),
        variant(b"c\x00\xef\x80\x0b\xb8", "Never", &[]),
        variant(b"q\x98\x8cM\x8e\x08", "Text", &[]),
        variant(b"\xd0Y?\xda\x9f\xb6", "LiteralTextView", &[]),
        variant(b"pL \xb1\x8c\x8e", "TextView", &[]),
        variant(
            b"\xd1\xbf\xdd\xae\x10V",
            "TextValues",
            &[field(b"_\xec\xebf\xff\xc8", "0", "i64")],
        ),
        variant(
            b"\xf7\xbfU\x17\x8c\x06",
            "LiteralTextValues",
            &[field(b"_\xec\xebf\xff\xc8", "0", "i64")],
        ),
        variant(
            b"/\x98A\xdf\x82\x04",
            "TextValuesView",
            &[field(b"_\xec\xebf\xff\xc8", "0", "i64")],
        ),
        variant(
            b"\xe6Zg\x16y\x07",
            "LiteralTextValuesView",
            &[field(b"_\xec\xebf\xff\xc8", "0", "i64")],
        ),
        variant(b"\xdb\x01{)\xdaG", "TextSequence", &[]),
        variant(b"\x931\xa0y+U", "LiteralTextSequence", &[]),
        variant(
            b"\xa5\x11\x90\x91a\xe1",
            "Layout",
            &[field(b"_\xec\xebf\xff\xc8", "0", "enum@bf18bf9e46ed")],
        ),
    ]),
};
