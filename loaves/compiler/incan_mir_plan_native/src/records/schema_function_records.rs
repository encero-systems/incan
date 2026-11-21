//! Referenced descriptions of the native-plan profile's `function_records` shapes.

use crate::wire::{Shape, ShapeBody, field, variant};

pub(crate) static SHAPE_FUNCTION_RECORDS_STORED_BLOCK: Shape = Shape {
    selector: b"\x13\xd4\x002\xb2\xad\x00",
    path: "incan.native-plan/5.function_records.StoredBlock",
    body: ShapeBody::Record(&[
        field(b"U\xa8\x1c\xf2\x1eq", "statements", "list<record@2044fcbf30e3>"),
        field(b"\xe5\x96\x89\x9f\x11K", "exit", "enum@b2c544231912"),
        field(b"\x927\x9a<f\xb7", "exit_span", "record@533bfbed0457"),
        field(b"a\x14\x96\xf4\x12\xca", "cleanup", "boolean"),
        field(b"7\xa0\x02Z\xa1\xf2", "span", "record@533bfbed0457"),
    ]),
};

pub(crate) static SHAPE_FUNCTION_RECORDS_STORED_EXIT: Shape = Shape {
    selector: b"\xb2\xc5D#\x19\x12\x00",
    path: "incan.native-plan/5.function_records.StoredExit",
    body: ShapeBody::Enum(&[
        variant(
            b"\xf1\x07\x91\x11\xa1\xc1",
            "Goto",
            &[field(b"_\xec\xebf\xff\xc8", "0", "i64")],
        ),
        variant(
            b"9\x92\x1at\x0b\xf2",
            "Switch",
            &[
                field(b"_\xec\xebf\xff\xc8", "0", "record@5993b825e223"),
                field(b"k\x86\xb2s\xff4", "1", "i64"),
                field(b"\xd4s^:&^", "2", "i64"),
            ],
        ),
        variant(
            b"\xd6\xe6E\xb7\xd2\xb2",
            "Call",
            &[
                field(b"_\xec\xebf\xff\xc8", "0", "enum@575be152b314"),
                field(b"k\x86\xb2s\xff4", "1", "record@533bfbed0457"),
                field(b"\xd4s^:&^", "2", "list<record@5993b825e223>"),
                field(b"N\x07@\x85b\xbe", "3", "record@11e5b2efd53f"),
                field(b"K\x22ww\xd4\xdd", "4", "i64"),
                field(b"\xef-\x12}\xe3{", "5", "enum@0a95e35810e0"),
            ],
        ),
        variant(
            b"\xc8\xaf\x9a\xe3zH",
            "Drop",
            &[
                field(b"_\xec\xebf\xff\xc8", "0", "record@11e5b2efd53f"),
                field(b"k\x86\xb2s\xff4", "1", "i64"),
                field(b"\xd4s^:&^", "2", "enum@0a95e35810e0"),
            ],
        ),
        variant(
            b"\x17\xb8\xdc\xf5>K",
            "Overflow",
            &[
                field(b"_\xec\xebf\xff\xc8", "0", "record@5993b825e223"),
                field(b"k\x86\xb2s\xff4", "1", "i64"),
                field(b"\xd4s^:&^", "2", "record@5993b825e223"),
                field(b"N\x07@\x85b\xbe", "3", "record@5993b825e223"),
                field(b"K\x22ww\xd4\xdd", "4", "i64"),
                field(b"\xef-\x12}\xe3{", "5", "enum@0a95e35810e0"),
            ],
        ),
        variant(b"k\x01C\xd08d", "Return", &[]),
        variant(b"\xab\xaaF\xad\xb4\xe1", "Unreachable", &[]),
        variant(b"\xd6@\xc7B\x1d\xa0", "Resume", &[]),
    ]),
};

pub(crate) static SHAPE_FUNCTION_RECORDS_STORED_EXTERNAL: Shape = Shape {
    selector: b"t\xaa^\x89\x16D\x00",
    path: "incan.native-plan/5.function_records.StoredExternal",
    body: ShapeBody::Record(&[
        field(b"\xa0\xaf\x9f\x86[\xf6", "path", "string"),
        field(b"\xb1\x92\x01}\xa4\x7f", "arguments", "list<enum@234b6c9b62c7>"),
        field(b"\xf5(!*Im", "parameters", "list<enum@234b6c9b62c7>"),
        field(b"\xf6\xa2\x14\xf7\xa5\xfc", "result", "enum@234b6c9b62c7"),
        field(b"7\xa0\x02Z\xa1\xf2", "span", "record@533bfbed0457"),
    ]),
};

pub(crate) static SHAPE_FUNCTION_RECORDS_STORED_FUNCTION: Shape = Shape {
    selector: b"\x03\xa6%\x81$\xed\x00",
    path: "incan.native-plan/5.function_records.StoredFunction",
    body: ShapeBody::Record(&[
        field(b"\xdf\x0a\xd6\xe48\x80", "schema", "i64"),
        field(b"\x82\xa3S\x7f\xf0\xdb", "name", "string"),
        field(b"\xf5(!*Im", "parameters", "list<record@47088f12e059>"),
        field(b"\xf6\xa2\x14\xf7\xa5\xfc", "result", "enum@234b6c9b62c7"),
        field(b"\xe9\xe8\xd4\xb1\x93\xcf", "locals", "list<record@4a004ae53e8b>"),
        field(b"*\x12\xda\x17\xd2|", "blocks", "list<record@13d40032b2ad>"),
        field(b"7\xa0\x02Z\xa1\xf2", "span", "record@533bfbed0457"),
        field(b"8\x05\xee\xed\xa5\xe1", "lifetimes", "list<list<string>>"),
        field(b"\xbe\x9a\xa7IM\x82", "loans", "list<record@147b521c5e6a>"),
        field(b"\x01\x8a\x9c\xec\xfa\x93", "retained_result_source", "i64"),
    ]),
};

pub(crate) static SHAPE_FUNCTION_RECORDS_STORED_LOAN: Shape = Shape {
    selector: b"\x14{R\x1c^j\x00",
    path: "incan.native-plan/5.function_records.StoredLoan",
    body: ShapeBody::Record(&[
        field(b"eX\x83\x831\xb7", "slot", "i64"),
        field(b"\xa0\xaf\x9f\x86[\xf6", "path", "list<i64>"),
        field(b"c\x906\x1d\xfd\xff", "parameter", "i64"),
        field(b"O\x9d\xb7\xf8\xe5\x06", "ty", "enum@234b6c9b62c7"),
    ]),
};

pub(crate) static SHAPE_FUNCTION_RECORDS_STORED_PARAMETER: Shape = Shape {
    selector: b"G\x08\x8f\x12\xe0Y\x00",
    path: "incan.native-plan/5.function_records.StoredParameter",
    body: ShapeBody::Record(&[
        field(b"\x82\xa3S\x7f\xf0\xdb", "name", "string"),
        field(b"eX\x83\x831\xb7", "slot", "record@4a004ae53e8b"),
    ]),
};

pub(crate) static SHAPE_FUNCTION_RECORDS_STORED_PRODUCT: Shape = Shape {
    selector: b"G\xc7\x7f$\xca!\x00",
    path: "incan.native-plan/5.function_records.StoredProduct",
    body: ShapeBody::Record(&[
        field(b"\xdf\x0a\xd6\xe48\x80", "schema", "i64"),
        field(b"x\xf9\xac\x01\x8eU", "function", "record@03a6258124ed"),
        field(b"\xd9,j\x81\xb2\xff", "runtime", "list<record@74aa5e891644>"),
        field(b"wYS3\xedF", "nominals", "list<record@5b085623fef4>"),
        field(b"YL\xfd&\x07\xd4", "frames", "list<record@03a6258124ed>"),
    ]),
};

pub(crate) static SHAPE_FUNCTION_RECORDS_STORED_SLOT: Shape = Shape {
    selector: b"J\x00J\xe5>\x8b\x00",
    path: "incan.native-plan/5.function_records.StoredSlot",
    body: ShapeBody::Record(&[
        field(b"O\x9d\xb7\xf8\xe5\x06", "ty", "enum@234b6c9b62c7"),
        field(b"7\xa0\x02Z\xa1\xf2", "span", "record@533bfbed0457"),
    ]),
};

pub(crate) static SHAPE_FUNCTION_RECORDS_STORED_TARGET: Shape = Shape {
    selector: b"W[\xe1R\xb3\x14\x00",
    path: "incan.native-plan/5.function_records.StoredTarget",
    body: ShapeBody::Enum(&[
        variant(
            b"\xfbo|\x93\xe6\xbf",
            "Planned",
            &[field(b"_\xec\xebf\xff\xc8", "0", "string")],
        ),
        variant(
            b"h\xc1\x14\xea\x9c\x8c",
            "External",
            &[field(b"_\xec\xebf\xff\xc8", "0", "string")],
        ),
        variant(
            b"\x12\xfds\xf8Q\xe0",
            "Instantiated",
            &[
                field(b"_\xec\xebf\xff\xc8", "0", "string"),
                field(b"k\x86\xb2s\xff4", "1", "list<enum@234b6c9b62c7>"),
            ],
        ),
        variant(
            b"\xc8\x948\xc6\xc9\xb9",
            "CloneModel",
            &[field(b"_\xec\xebf\xff\xc8", "0", "i64")],
        ),
        variant(
            b"P\x03\xf3_;\xc1",
            "CloneEnum",
            &[field(b"_\xec\xebf\xff\xc8", "0", "i64")],
        ),
        variant(
            b"\x81@\xe21\x84\x11",
            "ClosureBody",
            &[
                field(b"_\xec\xebf\xff\xc8", "0", "string"),
                field(b"k\x86\xb2s\xff4", "1", "i64"),
            ],
        ),
        variant(
            b"q9EI8$",
            "ClosureBodyOnce",
            &[
                field(b"_\xec\xebf\xff\xc8", "0", "string"),
                field(b"k\x86\xb2s\xff4", "1", "i64"),
            ],
        ),
        variant(
            b"\xb7i\xac\x8f\xcd\xa0",
            "CallClosure",
            &[field(b"_\xec\xebf\xff\xc8", "0", "list<enum@234b6c9b62c7>")],
        ),
        variant(
            b"\xc4i\xacQ\xb1\x89",
            "CallClosureOnce",
            &[field(b"_\xec\xebf\xff\xc8", "0", "list<enum@234b6c9b62c7>")],
        ),
        variant(
            b"\xa3C\xf89\x18\xe2",
            "CallBorrowingClosure",
            &[field(b"_\xec\xebf\xff\xc8", "0", "list<enum@234b6c9b62c7>")],
        ),
        variant(
            b"\xd76^\xdcH\xed",
            "SharedCallableBody",
            &[field(b"_\xec\xebf\xff\xc8", "0", "list<enum@234b6c9b62c7>")],
        ),
        variant(
            b"\x07\xb9v\xcd\xac\xf6",
            "CloneClosure",
            &[field(b"_\xec\xebf\xff\xc8", "0", "list<enum@234b6c9b62c7>")],
        ),
        variant(
            b"\x8e7\x95=#\xda",
            "Value",
            &[field(b"_\xec\xebf\xff\xc8", "0", "record@5993b825e223")],
        ),
        variant(
            b"\xd6\x8a\xf6\x95\xfc\x92",
            "FormatValue",
            &[
                field(b"_\xec\xebf\xff\xc8", "0", "enum@234b6c9b62c7"),
                field(b"k\x86\xb2s\xff4", "1", "boolean"),
            ],
        ),
    ]),
};

pub(crate) static SHAPE_FUNCTION_RECORDS_STORED_UNWIND: Shape = Shape {
    selector: b"\x0a\x95\xe3X\x10\xe0\x00",
    path: "incan.native-plan/5.function_records.StoredUnwind",
    body: ShapeBody::Enum(&[
        variant(b"1\xfb\xef\x16%\x94", "Continue", &[]),
        variant(
            b"\x9f\x1b#{]\xc3",
            "Cleanup",
            &[field(b"_\xec\xebf\xff\xc8", "0", "i64")],
        ),
        variant(b"\xf9\x13\xf0\x92\x10\xe8", "Terminate", &[]),
    ]),
};
