//! Generated schema descriptions for the compiler-owned native-plan profile. Transitional: the plan lane's
//! incan_mir_plan_native replaces it.
//!
//! Translated mechanically from the reference module's generated descriptions; field order, identities and
//! storage spellings are unchanged.

use crate::crumb_rows::{ModelField, model_description};
use crate::plan_wire_rows::{enum_schema, variant_schema};
use internal_models_bindings::Description;

fn schema_stored_constant() -> Description {
    enum_schema(
        b"\xbc\x85PCe\xc3\x00",
        "incan.native-plan/5.body_records.StoredConstant",
        vec![
            variant_schema(
                b"\x0b_`\x80p\xc6",
                "Int",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "i64")],
            ),
            variant_schema(
                b"\x1ai<\x00\xc4\x0c",
                "Float",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "fp64")],
            ),
            variant_schema(
                b"I\xf4\x11\xf0\xa1\xa7",
                "Bool",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "boolean")],
            ),
            variant_schema(b"NTY`\xf1\xbf", "Unit", &[]),
            variant_schema(
                b"q\x98\x8cM\x8e\x08",
                "Text",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "string")],
            ),
            variant_schema(
                b"6u-l\xd1G",
                "Numeric",
                &[
                    ModelField::new(b"_\xec\xebf\xff\xc8", "0", "string"),
                    ModelField::new(b"k\x86\xb2s\xff4", "1", "enum@234b6c9b62c7"),
                ],
            ),
            variant_schema(
                b"\xeb\x90\x0dwd\xd0",
                "TypeToken",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "list<enum@234b6c9b62c7>")],
            ),
        ],
    )
}

fn schema_stored_field_step() -> Description {
    model_description(
        b"[\x17\xf9\xc5r\xab\x00",
        "incan.native-plan/5.body_records.StoredFieldStep",
        &[
            ModelField::new(b"eX\x83\x831\xb7", "slot", "i64"),
            ModelField::new(b"O\x9d\xb7\xf8\xe5\x06", "ty", "enum@234b6c9b62c7"),
            ModelField::new(b"\xcb{\xf5bB\x0b", "variant", "i64"),
            ModelField::new(b"3\x8e0\xd7\xaa\x17", "dereferences", "i64"),
        ],
    )
}

fn schema_stored_operand() -> Description {
    model_description(
        b"Y\x93\xb8%\xe2#\x00",
        "incan.native-plan/5.body_records.StoredOperand",
        &[
            ModelField::new(b"\x0b'\xc1X\xfe\xb3", "kind", "enum@da16cd5be698"),
            ModelField::new(b"7\xa0\x02Z\xa1\xf2", "span", "record@533bfbed0457"),
        ],
    )
}

fn schema_stored_place() -> Description {
    model_description(
        b"\x11\xe5\xb2\xef\xd5?\x00",
        "incan.native-plan/5.body_records.StoredPlace",
        &[
            ModelField::new(b"%\xbf\x8e\x1a#\x93", "local", "i64"),
            ModelField::new(b"\x1b%\x0e\xa1\x99\xbe", "projection", "enum@7b687d4be52b"),
            ModelField::new(b"7\xa0\x02Z\xa1\xf2", "span", "record@533bfbed0457"),
        ],
    )
}

fn schema_stored_projection() -> Description {
    enum_schema(
        b"{h}K\xe5+\x00",
        "incan.native-plan/5.body_records.StoredProjection",
        vec![
            variant_schema(b"\xfdU\x05`\x93\x8c", "Whole", &[]),
            variant_schema(b"\x8e7\x95=#\xda", "Value", &[]),
            variant_schema(b"\x17\xb8\xdc\xf5>K", "Overflow", &[]),
            variant_schema(
                b"\x04I\xf5\xd2N?",
                "NumericValue",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "enum@234b6c9b62c7")],
            ),
            variant_schema(
                b"\xc0\x01\x8e\x9e\x9f\x16",
                "VariantField",
                &[
                    ModelField::new(b"_\xec\xebf\xff\xc8", "0", "i64"),
                    ModelField::new(b"k\x86\xb2s\xff4", "1", "i64"),
                    ModelField::new(b"\xd4s^:&^", "2", "enum@234b6c9b62c7"),
                ],
            ),
            variant_schema(
                b"\xf4_\xc1\xdf\xdc\x96",
                "Field",
                &[
                    ModelField::new(b"_\xec\xebf\xff\xc8", "0", "i64"),
                    ModelField::new(b"k\x86\xb2s\xff4", "1", "enum@234b6c9b62c7"),
                ],
            ),
            variant_schema(
                b"\x12/\x9b\xd7h\xa6",
                "Deref",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "enum@234b6c9b62c7")],
            ),
            variant_schema(
                b"\x84\xdd\xaa\x99\xf0\x90",
                "DerefField",
                &[
                    ModelField::new(b"_\xec\xebf\xff\xc8", "0", "i64"),
                    ModelField::new(b"k\x86\xb2s\xff4", "1", "enum@234b6c9b62c7"),
                ],
            ),
            variant_schema(
                b"aoH\xc1\xdb\x19",
                "Fields",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "list<record@5b17f9c572ab>")],
            ),
            variant_schema(
                b"g\x93\xd8#\xbc\xbe",
                "DerefFields",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "list<record@5b17f9c572ab>")],
            ),
        ],
    )
}

fn schema_stored_read() -> Description {
    enum_schema(
        b"\xda\x16\xcd[\xe6\x98\x00",
        "incan.native-plan/5.body_records.StoredRead",
        vec![
            variant_schema(
                b"\xe2\x1f\x93_\x11\xd7",
                "Copy",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "record@11e5b2efd53f")],
            ),
            variant_schema(
                b"n\xcc=\xf6\xbf\xfd",
                "Move",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "record@11e5b2efd53f")],
            ),
            variant_schema(
                b"\xa6#\x0c*\x81\xab",
                "Literal",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "enum@bc85504365c3")],
            ),
        ],
    )
}

fn schema_stored_rvalue() -> Description {
    enum_schema(
        b"\x01-\xdf\x05\x08<\x00",
        "incan.native-plan/5.body_records.StoredRvalue",
        vec![
            variant_schema(
                b"\xc3m\x81\x9e{\xc6",
                "Use",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "record@5993b825e223")],
            ),
            variant_schema(
                b"\xee\x1c\x82w\xca\xf5",
                "Binary",
                &[
                    ModelField::new(b"_\xec\xebf\xff\xc8", "0", "i64"),
                    ModelField::new(b"k\x86\xb2s\xff4", "1", "record@5993b825e223"),
                    ModelField::new(b"\xd4s^:&^", "2", "record@5993b825e223"),
                ],
            ),
            variant_schema(
                b"~\x98\x83P\x9d\x02",
                "Unary",
                &[
                    ModelField::new(b"_\xec\xebf\xff\xc8", "0", "i64"),
                    ModelField::new(b"k\x86\xb2s\xff4", "1", "record@5993b825e223"),
                ],
            ),
            variant_schema(
                b"9\x15+\x9f}\xe3",
                "IntToFloat",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "record@5993b825e223")],
            ),
            variant_schema(
                b"\x8b\xc2\xe2\xc7a\x1f",
                "FloatToInt",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "record@5993b825e223")],
            ),
            variant_schema(
                b"01\xa6\xb1\xeb^",
                "BoolToInt",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "record@5993b825e223")],
            ),
            variant_schema(
                b"\xf0\xcf9\xd0\xbe>",
                "Array",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "list<record@5993b825e223>")],
            ),
            variant_schema(
                b".\xdf\xb1h\xb3\x8b",
                "Borrow",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "record@11e5b2efd53f")],
            ),
            variant_schema(
                b"\xecWL\xe3\xa7d",
                "UnsizeSlice",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "record@5993b825e223")],
            ),
            variant_schema(
                b"\xdd\xb0\x0b=6\xb4",
                "NativeAlias",
                &[
                    ModelField::new(b"_\xec\xebf\xff\xc8", "0", "record@5993b825e223"),
                    ModelField::new(b"k\x86\xb2s\xff4", "1", "enum@234b6c9b62c7"),
                    ModelField::new(b"\xd4s^:&^", "2", "enum@234b6c9b62c7"),
                ],
            ),
            variant_schema(
                b"\x0fAV\x17\xca$",
                "FrozenText",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "record@5993b825e223")],
            ),
            variant_schema(
                b"\xa5\x1aU\xd1;\xee",
                "FrozenBytes",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "list<i64>")],
            ),
            variant_schema(
                b"\xdeJ\xe4\xd3\x93\xf0",
                "NumericCast",
                &[
                    ModelField::new(b"_\xec\xebf\xff\xc8", "0", "record@5993b825e223"),
                    ModelField::new(b"k\x86\xb2s\xff4", "1", "enum@234b6c9b62c7"),
                    ModelField::new(b"\xd4s^:&^", "2", "enum@234b6c9b62c7"),
                ],
            ),
            variant_schema(
                b"T\x10\x8d\xaf\xaa\xc5",
                "Enum",
                &[
                    ModelField::new(b"_\xec\xebf\xff\xc8", "0", "i64"),
                    ModelField::new(b"k\x86\xb2s\xff4", "1", "i64"),
                    ModelField::new(b"\xd4s^:&^", "2", "list<record@5993b825e223>"),
                ],
            ),
            variant_schema(
                b"x\xbc\x9c&\x03\x1a",
                "Discriminant",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "record@11e5b2efd53f")],
            ),
            variant_schema(
                b"\xe7#\xb0\xbd\x03\xf7",
                "TagToInt",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "record@5993b825e223")],
            ),
            variant_schema(
                b"^,aL#\xf0",
                "Model",
                &[
                    ModelField::new(b"_\xec\xebf\xff\xc8", "0", "i64"),
                    ModelField::new(b"k\x86\xb2s\xff4", "1", "list<record@5993b825e223>"),
                ],
            ),
            variant_schema(
                b"\xf16\xeex\xc3;",
                "Tuple",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "list<record@5993b825e223>")],
            ),
            variant_schema(
                b"]\xe7J\x81\xb1\x92",
                "Range",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "list<record@5993b825e223>")],
            ),
            variant_schema(
                b"\xa87!2_\xde",
                "MutBorrow",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "record@11e5b2efd53f")],
            ),
            variant_schema(
                b"ivP2h\xd9",
                "IsVariant",
                &[
                    ModelField::new(b"_\xec\xebf\xff\xc8", "0", "record@11e5b2efd53f"),
                    ModelField::new(b"k\x86\xb2s\xff4", "1", "i64"),
                ],
            ),
            variant_schema(
                b"S\xe4\x16\xfd%\xcf",
                "ReifyFunction",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "record@5993b825e223")],
            ),
            variant_schema(
                b"\xc4\xe4\xd4\xc6~\xe2",
                "FunctionAddress",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "record@5993b825e223")],
            ),
            variant_schema(
                b"s!\xe9q\x11\xd7",
                "ClosureObject",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "record@11e5b2efd53f")],
            ),
            variant_schema(
                b"^H\x983G\x01",
                "PlannedFunctionItem",
                &[
                    ModelField::new(b"_\xec\xebf\xff\xc8", "0", "string"),
                    ModelField::new(b"k\x86\xb2s\xff4", "1", "record@533bfbed0457"),
                ],
            ),
        ],
    )
}

fn schema_body_records_span() -> Description {
    model_description(
        b"S;\xfb\xed\x04W\x00",
        "incan.native-plan/5.body_records.StoredSpan",
        &[
            ModelField::new(b";\x9c5\x8f6\xf0", "file", "string"),
            ModelField::new(b"8\xa9\xc1\xe7!X", "line", "i64"),
            ModelField::new(b"\x88\x1d_\xb6\x00\x90", "column", "i64"),
        ],
    )
}

fn schema_stored_statement() -> Description {
    model_description(
        b" D\xfc\xbf0\xe3\x00",
        "incan.native-plan/5.body_records.StoredStatement",
        &[
            ModelField::new(b"\x0b'\xc1X\xfe\xb3", "kind", "enum@ff331f2df5c8"),
            ModelField::new(b"7\xa0\x02Z\xa1\xf2", "span", "record@533bfbed0457"),
        ],
    )
}

fn schema_stored_statement_kind() -> Description {
    enum_schema(
        b"\xff3\x1f-\xf5\xc8\x00",
        "incan.native-plan/5.body_records.StoredStatementKind",
        vec![
            variant_schema(
                b"\x8e\xce\x89\x5c\x9b2",
                "Assign",
                &[
                    ModelField::new(b"_\xec\xebf\xff\xc8", "0", "record@11e5b2efd53f"),
                    ModelField::new(b"k\x86\xb2s\xff4", "1", "enum@012ddf05083c"),
                    ModelField::new(b"\xd4s^:&^", "2", "record@533bfbed0457"),
                ],
            ),
            variant_schema(
                b"\xb6J\xc0_\x17\xe6",
                "Live",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "i64")],
            ),
            variant_schema(
                b"\xec\x9b\x10\xa4\xa7\x9c",
                "Dead",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "i64")],
            ),
        ],
    )
}

fn schema_stored_type() -> Description {
    enum_schema(
        b"#Kl\x9bb\xc7\x00",
        "incan.native-plan/5.body_records.StoredType",
        vec![
            variant_schema(b"\x0b_`\x80p\xc6", "Int", &[]),
            variant_schema(b"\x1ai<\x00\xc4\x0c", "Float", &[]),
            variant_schema(b"I\xf4\x11\xf0\xa1\xa7", "Bool", &[]),
            variant_schema(b"NTY`\xf1\xbf", "Unit", &[]),
            variant_schema(b"\xc3\xf45\xc7(Q", "CheckedInt", &[]),
            variant_schema(b"c\x00\xef\x80\x0b\xb8", "Never", &[]),
            variant_schema(b"q\x98\x8cM\x8e\x08", "Text", &[]),
            variant_schema(b"\xd0Y?\xda\x9f\xb6", "LiteralTextView", &[]),
            variant_schema(b"pL \xb1\x8c\x8e", "TextView", &[]),
            variant_schema(
                b"\xd1\xbf\xdd\xae\x10V",
                "TextValues",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "i64")],
            ),
            variant_schema(
                b"\xf7\xbfU\x17\x8c\x06",
                "LiteralTextValues",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "i64")],
            ),
            variant_schema(
                b"/\x98A\xdf\x82\x04",
                "TextValuesView",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "i64")],
            ),
            variant_schema(
                b"\xe6Zg\x16y\x07",
                "LiteralTextValuesView",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "i64")],
            ),
            variant_schema(b"\xdb\x01{)\xdaG", "TextSequence", &[]),
            variant_schema(b"\x931\xa0y+U", "LiteralTextSequence", &[]),
            variant_schema(
                b"\xa5\x11\x90\x91a\xe1",
                "Layout",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "enum@bf18bf9e46ed")],
            ),
        ],
    )
}

fn schema_stored_block() -> Description {
    model_description(
        b"\x13\xd4\x002\xb2\xad\x00",
        "incan.native-plan/5.function_records.StoredBlock",
        &[
            ModelField::new(b"U\xa8\x1c\xf2\x1eq", "statements", "list<record@2044fcbf30e3>"),
            ModelField::new(b"\xe5\x96\x89\x9f\x11K", "exit", "enum@b2c544231912"),
            ModelField::new(b"\x927\x9a<f\xb7", "exit_span", "record@533bfbed0457"),
            ModelField::new(b"a\x14\x96\xf4\x12\xca", "cleanup", "boolean"),
            ModelField::new(b"7\xa0\x02Z\xa1\xf2", "span", "record@533bfbed0457"),
        ],
    )
}

fn schema_stored_exit() -> Description {
    enum_schema(
        b"\xb2\xc5D#\x19\x12\x00",
        "incan.native-plan/5.function_records.StoredExit",
        vec![
            variant_schema(
                b"\xf1\x07\x91\x11\xa1\xc1",
                "Goto",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "i64")],
            ),
            variant_schema(
                b"9\x92\x1at\x0b\xf2",
                "Switch",
                &[
                    ModelField::new(b"_\xec\xebf\xff\xc8", "0", "record@5993b825e223"),
                    ModelField::new(b"k\x86\xb2s\xff4", "1", "i64"),
                    ModelField::new(b"\xd4s^:&^", "2", "i64"),
                ],
            ),
            variant_schema(
                b"\xd6\xe6E\xb7\xd2\xb2",
                "Call",
                &[
                    ModelField::new(b"_\xec\xebf\xff\xc8", "0", "enum@575be152b314"),
                    ModelField::new(b"k\x86\xb2s\xff4", "1", "record@533bfbed0457"),
                    ModelField::new(b"\xd4s^:&^", "2", "list<record@5993b825e223>"),
                    ModelField::new(b"N\x07@\x85b\xbe", "3", "record@11e5b2efd53f"),
                    ModelField::new(b"K\x22ww\xd4\xdd", "4", "i64"),
                    ModelField::new(b"\xef-\x12}\xe3{", "5", "enum@0a95e35810e0"),
                ],
            ),
            variant_schema(
                b"\xc8\xaf\x9a\xe3zH",
                "Drop",
                &[
                    ModelField::new(b"_\xec\xebf\xff\xc8", "0", "record@11e5b2efd53f"),
                    ModelField::new(b"k\x86\xb2s\xff4", "1", "i64"),
                    ModelField::new(b"\xd4s^:&^", "2", "enum@0a95e35810e0"),
                ],
            ),
            variant_schema(
                b"\x17\xb8\xdc\xf5>K",
                "Overflow",
                &[
                    ModelField::new(b"_\xec\xebf\xff\xc8", "0", "record@5993b825e223"),
                    ModelField::new(b"k\x86\xb2s\xff4", "1", "i64"),
                    ModelField::new(b"\xd4s^:&^", "2", "record@5993b825e223"),
                    ModelField::new(b"N\x07@\x85b\xbe", "3", "record@5993b825e223"),
                    ModelField::new(b"K\x22ww\xd4\xdd", "4", "i64"),
                    ModelField::new(b"\xef-\x12}\xe3{", "5", "enum@0a95e35810e0"),
                ],
            ),
            variant_schema(b"k\x01C\xd08d", "Return", &[]),
            variant_schema(b"\xab\xaaF\xad\xb4\xe1", "Unreachable", &[]),
            variant_schema(b"\xd6@\xc7B\x1d\xa0", "Resume", &[]),
        ],
    )
}

fn schema_stored_external() -> Description {
    model_description(
        b"t\xaa^\x89\x16D\x00",
        "incan.native-plan/5.function_records.StoredExternal",
        &[
            ModelField::new(b"\xa0\xaf\x9f\x86[\xf6", "path", "string"),
            ModelField::new(b"\xb1\x92\x01}\xa4\x7f", "arguments", "list<enum@234b6c9b62c7>"),
            ModelField::new(b"\xf5(!*Im", "parameters", "list<enum@234b6c9b62c7>"),
            ModelField::new(b"\xf6\xa2\x14\xf7\xa5\xfc", "result", "enum@234b6c9b62c7"),
            ModelField::new(b"7\xa0\x02Z\xa1\xf2", "span", "record@533bfbed0457"),
        ],
    )
}

fn schema_stored_function() -> Description {
    model_description(
        b"\x03\xa6%\x81$\xed\x00",
        "incan.native-plan/5.function_records.StoredFunction",
        &[
            ModelField::new(b"\xdf\x0a\xd6\xe48\x80", "schema", "i64"),
            ModelField::new(b"\x82\xa3S\x7f\xf0\xdb", "name", "string"),
            ModelField::new(b"\xf5(!*Im", "parameters", "list<record@47088f12e059>"),
            ModelField::new(b"\xf6\xa2\x14\xf7\xa5\xfc", "result", "enum@234b6c9b62c7"),
            ModelField::new(b"\xe9\xe8\xd4\xb1\x93\xcf", "locals", "list<record@4a004ae53e8b>"),
            ModelField::new(b"*\x12\xda\x17\xd2|", "blocks", "list<record@13d40032b2ad>"),
            ModelField::new(b"7\xa0\x02Z\xa1\xf2", "span", "record@533bfbed0457"),
            ModelField::new(b"8\x05\xee\xed\xa5\xe1", "lifetimes", "list<list<string>>"),
            ModelField::new(b"\xbe\x9a\xa7IM\x82", "loans", "list<record@147b521c5e6a>"),
            ModelField::new(b"\x01\x8a\x9c\xec\xfa\x93", "retained_result_source", "i64"),
        ],
    )
}

fn schema_stored_loan() -> Description {
    model_description(
        b"\x14{R\x1c^j\x00",
        "incan.native-plan/5.function_records.StoredLoan",
        &[
            ModelField::new(b"eX\x83\x831\xb7", "slot", "i64"),
            ModelField::new(b"\xa0\xaf\x9f\x86[\xf6", "path", "list<i64>"),
            ModelField::new(b"c\x906\x1d\xfd\xff", "parameter", "i64"),
            ModelField::new(b"O\x9d\xb7\xf8\xe5\x06", "ty", "enum@234b6c9b62c7"),
        ],
    )
}

fn schema_stored_parameter() -> Description {
    model_description(
        b"G\x08\x8f\x12\xe0Y\x00",
        "incan.native-plan/5.function_records.StoredParameter",
        &[
            ModelField::new(b"\x82\xa3S\x7f\xf0\xdb", "name", "string"),
            ModelField::new(b"eX\x83\x831\xb7", "slot", "record@4a004ae53e8b"),
        ],
    )
}

fn schema_stored_product() -> Description {
    model_description(
        b"G\xc7\x7f$\xca!\x00",
        "incan.native-plan/5.function_records.StoredProduct",
        &[
            ModelField::new(b"\xdf\x0a\xd6\xe48\x80", "schema", "i64"),
            ModelField::new(b"x\xf9\xac\x01\x8eU", "function", "record@03a6258124ed"),
            ModelField::new(b"\xd9,j\x81\xb2\xff", "runtime", "list<record@74aa5e891644>"),
            ModelField::new(b"wYS3\xedF", "nominals", "list<record@5b085623fef4>"),
            ModelField::new(b"YL\xfd&\x07\xd4", "frames", "list<record@03a6258124ed>"),
        ],
    )
}

fn schema_stored_slot() -> Description {
    model_description(
        b"J\x00J\xe5>\x8b\x00",
        "incan.native-plan/5.function_records.StoredSlot",
        &[
            ModelField::new(b"O\x9d\xb7\xf8\xe5\x06", "ty", "enum@234b6c9b62c7"),
            ModelField::new(b"7\xa0\x02Z\xa1\xf2", "span", "record@533bfbed0457"),
        ],
    )
}

fn schema_stored_target() -> Description {
    enum_schema(
        b"W[\xe1R\xb3\x14\x00",
        "incan.native-plan/5.function_records.StoredTarget",
        vec![
            variant_schema(
                b"\xfbo|\x93\xe6\xbf",
                "Planned",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "string")],
            ),
            variant_schema(
                b"h\xc1\x14\xea\x9c\x8c",
                "External",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "string")],
            ),
            variant_schema(
                b"\x12\xfds\xf8Q\xe0",
                "Instantiated",
                &[
                    ModelField::new(b"_\xec\xebf\xff\xc8", "0", "string"),
                    ModelField::new(b"k\x86\xb2s\xff4", "1", "list<enum@234b6c9b62c7>"),
                ],
            ),
            variant_schema(
                b"\xc8\x948\xc6\xc9\xb9",
                "CloneModel",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "i64")],
            ),
            variant_schema(
                b"P\x03\xf3_;\xc1",
                "CloneEnum",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "i64")],
            ),
            variant_schema(
                b"\x81@\xe21\x84\x11",
                "ClosureBody",
                &[
                    ModelField::new(b"_\xec\xebf\xff\xc8", "0", "string"),
                    ModelField::new(b"k\x86\xb2s\xff4", "1", "i64"),
                ],
            ),
            variant_schema(
                b"q9EI8$",
                "ClosureBodyOnce",
                &[
                    ModelField::new(b"_\xec\xebf\xff\xc8", "0", "string"),
                    ModelField::new(b"k\x86\xb2s\xff4", "1", "i64"),
                ],
            ),
            variant_schema(
                b"\xb7i\xac\x8f\xcd\xa0",
                "CallClosure",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "list<enum@234b6c9b62c7>")],
            ),
            variant_schema(
                b"\xc4i\xacQ\xb1\x89",
                "CallClosureOnce",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "list<enum@234b6c9b62c7>")],
            ),
            variant_schema(
                b"\xa3C\xf89\x18\xe2",
                "CallBorrowingClosure",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "list<enum@234b6c9b62c7>")],
            ),
            variant_schema(
                b"\xd76^\xdcH\xed",
                "SharedCallableBody",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "list<enum@234b6c9b62c7>")],
            ),
            variant_schema(
                b"\x07\xb9v\xcd\xac\xf6",
                "CloneClosure",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "list<enum@234b6c9b62c7>")],
            ),
            variant_schema(
                b"\x8e7\x95=#\xda",
                "Value",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "record@5993b825e223")],
            ),
        ],
    )
}

fn schema_stored_unwind() -> Description {
    enum_schema(
        b"\x0a\x95\xe3X\x10\xe0\x00",
        "incan.native-plan/5.function_records.StoredUnwind",
        vec![
            variant_schema(b"1\xfb\xef\x16%\x94", "Continue", &[]),
            variant_schema(
                b"\x9f\x1b#{]\xc3",
                "Cleanup",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "i64")],
            ),
            variant_schema(b"\xf9\x13\xf0\x92\x10\xe8", "Terminate", &[]),
        ],
    )
}

fn schema_stored_layout_enum_declaration() -> Description {
    model_description(
        b"\x22x[\xf6\xa0\xa6\x00",
        "incan.native-plan/5.layout_records.StoredLayoutEnumDeclaration",
        &[
            ModelField::new(b"\x82\xa3S\x7f\xf0\xdb", "name", "string"),
            ModelField::new(b"\xef\xa1\xf3u\xd7a", "public", "boolean"),
            ModelField::new(b"\xda\xd5i\xe1\x01\xad", "variants", "list<record@1f5612691518>"),
            ModelField::new(b"\xa8\x8f\xe6\xeca\x15", "derives", "list<string>"),
            ModelField::new(b"7\xa0\x02Z\xa1\xf2", "span", "record@e9253e623e14"),
            ModelField::new(b"a5&\xd6\xca\x1f", "view_lifetime_count", "i64"),
            ModelField::new(b"X\x03X~g\x91", "carrier", "enum@2e048564ffe2"),
            ModelField::new(b"\x18&\xb5\x1c\x02V", "semantic_key", "option<enum@d306f4a06aec>"),
            ModelField::new(b"p6\x9c\xac\xa4\xd8", "source_type", "string"),
            ModelField::new(b"\xef\x06\xc8!DT", "member_debug", "boolean"),
            ModelField::new(b"\x8a\xac\xe8\x82\x8f\xe6", "library_item", "string"),
            ModelField::new(b".\xeeG\xf93\xc4", "library_type_arguments", "list<enum@bf18bf9e46ed>"),
            ModelField::new(b"\x1d\x91\xf5\x83}\xa5", "raw_kind", "string"),
            ModelField::new(b"\x0fF&\xf8\xea\xb1", "raw_values", "list<string>"),
            ModelField::new(b";\xbbc\xd8xg", "comparison_methods", "list<string>"),
            ModelField::new(b"\xb5\x1b\x90e\x9ax", "closure_items", "list<string>"),
        ],
    )
}

fn schema_stored_layout_enum_variant() -> Description {
    model_description(
        b"\x1fV\x12i\x15\x18\x00",
        "incan.native-plan/5.layout_records.StoredLayoutEnumVariant",
        &[
            ModelField::new(b"\x82\xa3S\x7f\xf0\xdb", "name", "string"),
            ModelField::new(b"\xbf\xe5\xd6\x97\x16-", "fields", "list<enum@bf18bf9e46ed>"),
        ],
    )
}

fn schema_stored_layout_fallible_iterator_methods() -> Description {
    model_description(
        b"@\xee\xce\xa2\xfc\x97\x00",
        "incan.native-plan/5.layout_records.StoredLayoutFallibleIteratorMethods",
        &[
            ModelField::new(b"J3\xea\xcd_\xa6", "item", "enum@bf18bf9e46ed"),
            ModelField::new(b"\xca\x00\xfc\xcf\xb4\x08", "error", "enum@bf18bf9e46ed"),
            ModelField::new(b"\xc6\xc1\xc9\xa9\xc8T", "next", "string"),
        ],
    )
}

fn schema_stored_layout_field_lifecycle() -> Description {
    model_description(
        b"\x09\xce$\xfb\xd6\x95\x00",
        "incan.native-plan/5.layout_records.StoredLayoutFieldLifecycle",
        &[
            ModelField::new(b"kZ=\xed\x01j", "since", "option<string>"),
            ModelField::new(b"\xf3C>\xc5V\xe6", "retire_in", "option<string>"),
            ModelField::new(b"\xabS\x0a\x13\xe4Y", "message", "option<string>"),
            ModelField::new(b"\xa7\xe2\xd2n\x8d\x15", "by", "option<string>"),
            ModelField::new(b"JP(F\xd0p", "issue", "option<string>"),
        ],
    )
}

fn schema_stored_layout_field_metadata() -> Description {
    model_description(
        b"[\xf1\x88\xbeZK\x00",
        "incan.native-plan/5.layout_records.StoredLayoutFieldMetadata",
        &[
            ModelField::new(b"\x82\xa3S\x7f\xf0\xdb", "name", "string"),
            ModelField::new(b"E\xb0f\xb7\x9aM", "record_identity", "list<i64>"),
            ModelField::new(b"\xbd\xeb\x90\xf5Pa", "record_canonical", "boolean"),
            ModelField::new(b"\xcdE\x7f.X\x1d", "renamed_from", "list<string>"),
            ModelField::new(
                b"\xcfZ4\xb47\x09",
                "renamed_from_lifecycle",
                "option<record@09ce24fbd695>",
            ),
            ModelField::new(b"ZE$YP;", "position", "i64"),
            ModelField::new(b"\xda\xd0\x8fy8\xe2", "default_is_empty", "boolean"),
            ModelField::new(b"\x92\x7f\x03~3p", "stored_type", "option<string>"),
            ModelField::new(b"\x1a\x0aj6\xca\x0a", "alias", "option<string>"),
            ModelField::new(b"\xc9\x04oz7\xad", "description", "option<string>"),
            ModelField::new(b"%\x01\xa7\xdd\xe4V", "type_name", "string"),
            ModelField::new(b"h*w|#(", "has_default", "boolean"),
        ],
    )
}

fn schema_stored_layout_iterator_methods() -> Description {
    model_description(
        b"\xc3\xc7\x1c\x06&0\x00",
        "incan.native-plan/5.layout_records.StoredLayoutIteratorMethods",
        &[
            ModelField::new(b"J3\xea\xcd_\xa6", "item", "enum@bf18bf9e46ed"),
            ModelField::new(b"\xc6\xc1\xc9\xa9\xc8T", "next", "string"),
        ],
    )
}

fn schema_stored_layout_model_declaration() -> Description {
    model_description(
        b"\x9c\xff\xb7\x00\xc4\x1f\x00",
        "incan.native-plan/5.layout_records.StoredLayoutModelDeclaration",
        &[
            ModelField::new(b"\x82\xa3S\x7f\xf0\xdb", "name", "string"),
            ModelField::new(b"\xef\xa1\xf3u\xd7a", "public", "boolean"),
            ModelField::new(b"\xbf\xe5\xd6\x97\x16-", "fields", "list<record@815e81b91483>"),
            ModelField::new(b"\x0b\xc43\xf3\xaa\x0b", "field_public", "list<boolean>"),
            ModelField::new(b"\xa8\x8f\xe6\xeca\x15", "derives", "list<string>"),
            ModelField::new(b"7\xa0\x02Z\xa1\xf2", "span", "record@e9253e623e14"),
            ModelField::new(b"a5&\xd6\xca\x1f", "view_lifetime_count", "i64"),
            ModelField::new(b"*\x12\xf5\x01h\xc9", "source_name", "string"),
            ModelField::new(b"\x8a\xac\xe8\x82\x8f\xe6", "library_item", "string"),
            ModelField::new(b".\xeeG\xf93\xc4", "library_type_arguments", "list<enum@bf18bf9e46ed>"),
            ModelField::new(b"=\x15#\x88O\xbb", "field_serde_names", "list<string>"),
            ModelField::new(b"\x16\x1d\xbe\xd0\x94`", "checked_deserialize", "string"),
            ModelField::new(
                b"\x1bO\xa3\x96\xb0\xa9",
                "reflection_metadata",
                "option<list<record@5bf188be5a4b>>",
            ),
            ModelField::new(b"xX]\xef\x10\xd5", "field_metadata_carrier", "boolean"),
            ModelField::new(b";\xbbc\xd8xg", "comparison_methods", "list<string>"),
            ModelField::new(
                b"\xa9\xe5\xb8^:\xf8",
                "fallible_iterator",
                "option<record@40eecea2fc97>",
            ),
            ModelField::new(b"\xef\x9b\xae\xdf\xf2\xf4", "iterator", "option<record@c3c71c062630>"),
            ModelField::new(b"\xde*\xfbgVR", "callable_method", "string"),
            ModelField::new(b"Zv\x81SH\xae", "callable_signature", "list<enum@bf18bf9e46ed>"),
            ModelField::new(b"\xe1\xc2\xa1\x8c\xea\xaf", "string_conversion_root", "string"),
            ModelField::new(b"\x9d\x9e\x1a\x05i\x17", "string_conversion_trait", "string"),
            ModelField::new(
                b"\xd2y,\xbf\x1f&",
                "native_trait_adoptions",
                "list<record@fff9792bdf5c>",
            ),
            ModelField::new(b"\x99\xc6EI\x8d\x81", "unavailable_native_fields", "list<i64>"),
        ],
    )
}

fn schema_stored_layout_native_trait_adoption() -> Description {
    model_description(
        b"\xff\xf9y+\xdf\x5c\x00",
        "incan.native-plan/5.layout_records.StoredLayoutNativeTraitAdoption",
        &[
            ModelField::new(b"\xa0\xaf\x9f\x86[\xf6", "path", "string"),
            ModelField::new(b"?8g1:\x9d", "methods", "list<record@1cbc87da9cee>"),
        ],
    )
}

fn schema_stored_layout_native_trait_method() -> Description {
    model_description(
        b"\x1c\xbc\x87\xda\x9c\xee\x00",
        "incan.native-plan/5.layout_records.StoredLayoutNativeTraitMethod",
        &[
            ModelField::new(b"\x82\xa3S\x7f\xf0\xdb", "name", "string"),
            ModelField::new(b"x\xf9\xac\x01\x8eU", "function", "string"),
        ],
    )
}

fn schema_stored_layout_parameter() -> Description {
    model_description(
        b"\x81^\x81\xb9\x14\x83\x00",
        "incan.native-plan/5.layout_records.StoredLayoutParameter",
        &[
            ModelField::new(b"\x82\xa3S\x7f\xf0\xdb", "name", "string"),
            ModelField::new(b"O\x9d\xb7\xf8\xe5\x06", "ty", "enum@bf18bf9e46ed"),
            ModelField::new(b"7\xa0\x02Z\xa1\xf2", "span", "record@e9253e623e14"),
        ],
    )
}

fn schema_stored_layout_zip_carrier() -> Description {
    model_description(
        b">i\xa6U\x16 \x00",
        "incan.native-plan/5.layout_records.StoredLayoutZipCarrier",
        &[
            ModelField::new(b"6\x0f\x84\x03YB", "left", "enum@0f36ea9a02c1"),
            ModelField::new(b"'\x04/Nn\xca", "right", "enum@0f36ea9a02c1"),
            ModelField::new(b"4\xa0@\x05\xbc\xaf", "target", "enum@bf18bf9e46ed"),
        ],
    )
}

fn schema_layout_records_span() -> Description {
    model_description(
        b"\xe9%>b>\x14\x00",
        "incan.native-plan/5.layout_records.StoredSpan",
        &[
            ModelField::new(b";\x9c5\x8f6\xf0", "file", "string"),
            ModelField::new(b"8\xa9\xc1\xe7!X", "line", "i64"),
            ModelField::new(b"\x88\x1d_\xb6\x00\x90", "column", "i64"),
        ],
    )
}

fn schema_stored_instance_request() -> Description {
    model_description(
        b"x\x18\xc1\xb2k\x0b\x00",
        "incan.native-plan/5.layout_type_records.StoredInstanceRequest",
        &[
            ModelField::new(b"\xab\xcd\x0e\xe1\xfc,", "declaration", "string"),
            ModelField::new(b"\xb1\x92\x01}\xa4\x7f", "arguments", "list<enum@d306f4a06aec>"),
            ModelField::new(b"\xfc\x7f\x8co\xaf\xff", "owned_callable_slots", "list<i64>"),
            ModelField::new(b"\x9b\xf1\x17\x8c4{", "original_callable", "boolean"),
        ],
    )
}

fn schema_stored_layout_carrier() -> Description {
    enum_schema(
        b".\x04\x85d\xff\xe2\x00",
        "incan.native-plan/5.layout_type_records.StoredLayoutCarrier",
        vec![
            variant_schema(b"\xdcV\xdd&\x87\xa3", "Declared", &[]),
            variant_schema(b"E\xaa\xac\xba~\xa1", "Option", &[]),
            variant_schema(b"n}P\xe8OG", "Result", &[]),
        ],
    )
}

fn schema_stored_layout_checked_callable_param() -> Description {
    model_description(
        b"\xed\x84w>\x19\x8f\x00",
        "incan.native-plan/5.layout_type_records.StoredLayoutCheckedCallableParam",
        &[
            ModelField::new(b"\x82\xa3S\x7f\xf0\xdb", "name", "option<string>"),
            ModelField::new(b"O\x9d\xb7\xf8\xe5\x06", "ty", "enum@d306f4a06aec"),
            ModelField::new(b"\x0b'\xc1X\xfe\xb3", "kind", "enum@fcecb7d46f3c"),
            ModelField::new(b"h*w|#(", "has_default", "boolean"),
            ModelField::new(b"\xc2\xea58V\xa6", "is_partial_preset", "boolean"),
            ModelField::new(b"\xc5}!\xc0\xf9[", "is_mut", "boolean"),
            ModelField::new(b"\xa6\xcb\xca1|\x1a", "native_owned", "boolean"),
            ModelField::new(b"+#{X\xd0r", "input_access", "enum@925ce5e8c3e4"),
        ],
    )
}

fn schema_stored_layout_checked_callable_param_kind() -> Description {
    enum_schema(
        b"\xfc\xec\xb7\xd4o<\x00",
        "incan.native-plan/5.layout_type_records.StoredLayoutCheckedCallableParamKind",
        vec![
            variant_schema(b"\xa7$\x8e\xebE\xeb", "Normal", &[]),
            variant_schema(b"\xed\xe4!\xe1h\xfb", "RestPositional", &[]),
            variant_schema(b"\xff\xad\x057D!", "RestKeyword", &[]),
        ],
    )
}

fn schema_stored_layout_checked_input_access() -> Description {
    enum_schema(
        b"\x92\x5c\xe5\xe8\xc3\xe4\x00",
        "incan.native-plan/5.layout_type_records.StoredLayoutCheckedInputAccess",
        vec![
            variant_schema(b"\xd4a\xcb\xb4\x90f", "Inferred", &[]),
            variant_schema(b"\xe3\xc4\xb3\x9dmP", "Shared", &[]),
            variant_schema(b"H\x83\xb7\xdf\xff\xb0", "Exclusive", &[]),
            variant_schema(b"\x17\xb7`\xc4\x1c\xd2", "Owned", &[]),
            variant_schema(b"1\xca\xf1d\xbb&", "InferredShared", &[]),
        ],
    )
}

fn schema_stored_layout_checked_numeric_type() -> Description {
    enum_schema(
        b"\x12y\xc7\x00\xa7\xfe\x00",
        "incan.native-plan/5.layout_type_records.StoredLayoutCheckedNumericType",
        vec![
            variant_schema(b"1Fb\x8c\x11\x8c", "I8", &[]),
            variant_schema(b"\xb9\xff?nq\xda", "I16", &[]),
            variant_schema(b"\xbe-,\xd01}", "I32", &[]),
            variant_schema(b"\xc1\xc7\xe0-\xd6\xcb", "I64", &[]),
            variant_schema(b"G\xeck\xdaM\xa3", "I128", &[]),
            variant_schema(b"4s\xd3\x8a\x12A", "U8", &[]),
            variant_schema(b"\xef\x09\xbc\x80\xcdb", "U16", &[]),
            variant_schema(b"\x80\x07$\x86\xf2\xea", "U32", &[]),
            variant_schema(b"\x84\xb8\x9e\x22ET", "U64", &[]),
            variant_schema(b"\x0c\x80\x0d\xc4\xf9I", "U128", &[]),
            variant_schema(b"\x82%Z\xeel\xcc", "F32", &[]),
            variant_schema(b"\xa4M\x7ft\xc8)", "F64", &[]),
            variant_schema(b"\xec|D\x91\xf6\x86", "ISize", &[]),
            variant_schema(b"\xbaL\xadu\xa6Z", "USize", &[]),
            variant_schema(b"I\xf4\x11\xf0\xa1\xa7", "Bool", &[]),
        ],
    )
}

fn schema_stored_layout_checked_primitive_type() -> Description {
    enum_schema(
        b"\x86u\xd0\x84h\xe2\x00",
        "incan.native-plan/5.layout_type_records.StoredLayoutCheckedPrimitiveType",
        vec![
            variant_schema(b"\x0b_`\x80p\xc6", "Int", &[]),
            variant_schema(b"\x1ai<\x00\xc4\x0c", "Float", &[]),
            variant_schema(
                b"6u-l\xd1G",
                "Numeric",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "enum@1279c700a7fe")],
            ),
            variant_schema(b"I\xf4\x11\xf0\xa1\xa7", "Bool", &[]),
            variant_schema(b"\x80\x84\xa5\x1b<d", "Str", &[]),
            variant_schema(b"M\x80\x000\x1f\xcc", "Bytes", &[]),
            variant_schema(b"O\xab\xe1W\xe8\xf9", "FrozenStr", &[]),
            variant_schema(b"\xa5\x1aU\xd1;\xee", "FrozenBytes", &[]),
            variant_schema(b"NTY`\xf1\xbf", "Unit", &[]),
        ],
    )
}

fn schema_stored_layout_checked_runtime_builtin() -> Description {
    enum_schema(
        b"\xfd\xe1\x8a[\xba0\x00",
        "incan.native-plan/5.layout_type_records.StoredLayoutCheckedRuntimeBuiltin",
        vec![variant_schema(b"\xbd\xfa\xef\x14\xbe\x91", "ValidationError", &[])],
    )
}

fn schema_stored_layout_checked_type() -> Description {
    enum_schema(
        b"\xd3\x06\xf4\xa0j\xec\x00",
        "incan.native-plan/5.layout_type_records.StoredLayoutCheckedType",
        vec![
            variant_schema(b"c\x00\xef\x80\x0b\xb8", "Never", &[]),
            variant_schema(
                b"\x09V\x17<\xa4\x15",
                "Primitive",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "enum@8675d08468e2")],
            ),
            variant_schema(
                b"\x86\x05`Z\xe0&",
                "Named",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "string")],
            ),
            variant_schema(
                b"\x02(\xc6\xd4\x8e\xcf",
                "Generic",
                &[
                    ModelField::new(b"_\xec\xebf\xff\xc8", "0", "string"),
                    ModelField::new(b"k\x86\xb2s\xff4", "1", "list<enum@d306f4a06aec>"),
                ],
            ),
            variant_schema(
                b"\x18Q}3-\x1b",
                "Decimal",
                &[
                    ModelField::new(b"_\xec\xebf\xff\xc8", "0", "u8"),
                    ModelField::new(b"k\x86\xb2s\xff4", "1", "u8"),
                ],
            ),
            variant_schema(
                b"\xc8\x03q\x03\x02\xd5",
                "Function",
                &[
                    ModelField::new(b"_\xec\xebf\xff\xc8", "0", "list<record@ed84773e198f>"),
                    ModelField::new(b"k\x86\xb2s\xff4", "1", "list<enum@d306f4a06aec>"),
                ],
            ),
            variant_schema(
                b"\xeb\x90\x0dwd\xd0",
                "TypeToken",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "list<enum@d306f4a06aec>")],
            ),
            variant_schema(
                b"\xf16\xeex\xc3;",
                "Tuple",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "list<enum@d306f4a06aec>")],
            ),
            variant_schema(
                b"\xc7\xa4\xa1AK\xe4",
                "TypeVar",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "string")],
            ),
            variant_schema(b"\x800$\x93\xf2\xd4", "SelfType", &[]),
            variant_schema(
                b"\x9bn\x88\xe8\x94:",
                "Ref",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "list<enum@d306f4a06aec>")],
            ),
            variant_schema(
                b"\xff\xc4\x90\xa6\xa7\xb3",
                "RefMut",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "list<enum@d306f4a06aec>")],
            ),
            variant_schema(
                b"\xe7u\x7f\x0d\xdfV",
                "RustInteropPath",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "string")],
            ),
            variant_schema(
                b"o\xc7B\xf9\xdc\xbd",
                "NativeBuiltin",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "enum@fde18a5bba30")],
            ),
            variant_schema(
                b"T2FX\xe2\xeb",
                "Unsupported",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "string")],
            ),
            variant_schema(b"'@\x0f\xf4\xb8\x09", "Infer", &[]),
            variant_schema(b"\xb7d\xcd\xc0\xea\xb7", "Unknown", &[]),
            variant_schema(
                b"0?\xb1)\xac\xe5",
                "NativeArray",
                &[
                    ModelField::new(b"_\xec\xebf\xff\xc8", "0", "list<enum@d306f4a06aec>"),
                    ModelField::new(b"k\x86\xb2s\xff4", "1", "u64"),
                ],
            ),
            variant_schema(
                b"\x5cz%\xe2\x9cR",
                "NativeTraitObject",
                &[
                    ModelField::new(b"_\xec\xebf\xff\xc8", "0", "string"),
                    ModelField::new(b"k\x86\xb2s\xff4", "1", "boolean"),
                    ModelField::new(b"\xd4s^:&^", "2", "boolean"),
                ],
            ),
            variant_schema(
                b"\xa6\x9d@\x085\xc9",
                "NativePointer",
                &[
                    ModelField::new(b"_\xec\xebf\xff\xc8", "0", "boolean"),
                    ModelField::new(b"k\x86\xb2s\xff4", "1", "list<enum@d306f4a06aec>"),
                ],
            ),
            variant_schema(b"\xed\x5c1CFF", "NativeCChar", &[]),
        ],
    )
}

fn schema_stored_layout_list_leaf() -> Description {
    enum_schema(
        b"\xdd\x8d\xb5\x93\xbd\xc0\x00",
        "incan.native-plan/5.layout_type_records.StoredLayoutListLeaf",
        vec![
            variant_schema(b"\x0b_`\x80p\xc6", "Int", &[]),
            variant_schema(b"\x1ai<\x00\xc4\x0c", "Float", &[]),
            variant_schema(b"I\xf4\x11\xf0\xa1\xa7", "Bool", &[]),
            variant_schema(b"\x80\x84\xa5\x1b<d", "Str", &[]),
            variant_schema(
                b"\xf16\xeex\xc3;",
                "Tuple",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "list<enum@0f36ea9a02c1>")],
            ),
            variant_schema(
                b"^,aL#\xf0",
                "Model",
                &[
                    ModelField::new(b"_\xec\xebf\xff\xc8", "0", "i64"),
                    ModelField::new(b"k\x86\xb2s\xff4", "1", "string"),
                ],
            ),
            variant_schema(
                b"T\x10\x8d\xaf\xaa\xc5",
                "Enum",
                &[
                    ModelField::new(b"_\xec\xebf\xff\xc8", "0", "i64"),
                    ModelField::new(b"k\x86\xb2s\xff4", "1", "string"),
                ],
            ),
            variant_schema(b"4s\xd3\x8a\x12A", "U8", &[]),
            variant_schema(b"NTY`\xf1\xbf", "Unit", &[]),
            variant_schema(b"\x18Q}3-\x1b", "Decimal", &[]),
            variant_schema(b"O\xab\xe1W\xe8\xf9", "FrozenStr", &[]),
            variant_schema(b"\xa5\x1aU\xd1;\xee", "FrozenBytes", &[]),
            variant_schema(
                b"`ar\x85\xa4\xcb",
                "FrozenCollection",
                &[
                    ModelField::new(b"_\xec\xebf\xff\xc8", "0", "string"),
                    ModelField::new(b"k\x86\xb2s\xff4", "1", "list<enum@bf18bf9e46ed>"),
                ],
            ),
            variant_schema(
                b"\xa7\x0a\xca\xf0X`",
                "FunctionPointer",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "list<enum@bf18bf9e46ed>")],
            ),
            variant_schema(
                b"\xeb\x90\x0dwd\xd0",
                "TypeToken",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "list<enum@bf18bf9e46ed>")],
            ),
            variant_schema(
                b"o /T\xa7\xb2",
                "List",
                &[
                    ModelField::new(b"_\xec\xebf\xff\xc8", "0", "list<enum@dd8db593bdc0>"),
                    ModelField::new(b"k\x86\xb2s\xff4", "1", "i64"),
                ],
            ),
            variant_schema(
                b"\x96\x11\xd3\xd8\x00\xfc",
                "Dict",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "list<enum@dd8db593bdc0>")],
            ),
            variant_schema(
                b"\xb6\xf6\xf3\xad\x07\xb3",
                "Set",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "list<enum@dd8db593bdc0>")],
            ),
            variant_schema(b"\xbaL\xadu\xa6Z", "USize", &[]),
            variant_schema(
                b"6u-l\xd1G",
                "Numeric",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "enum@9dabba6a3395")],
            ),
            variant_schema(
                b"q\xbf\x90\x93_\xc2",
                "Reference",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "list<enum@bf18bf9e46ed>")],
            ),
        ],
    )
}

fn schema_stored_layout_plan_type() -> Description {
    enum_schema(
        b"\xbf\x18\xbf\x9eF\xed\x00",
        "incan.native-plan/5.layout_type_records.StoredLayoutPlanType",
        vec![
            variant_schema(b"\x0b_`\x80p\xc6", "Int", &[]),
            variant_schema(b"\x1ai<\x00\xc4\x0c", "Float", &[]),
            variant_schema(b"1Fb\x8c\x11\x8c", "I8", &[]),
            variant_schema(b"\xb9\xff?nq\xda", "I16", &[]),
            variant_schema(b"\xbe-,\xd01}", "I32", &[]),
            variant_schema(b"G\xeck\xdaM\xa3", "I128", &[]),
            variant_schema(b"\xec|D\x91\xf6\x86", "ISize", &[]),
            variant_schema(b"\xbaL\xadu\xa6Z", "USize", &[]),
            variant_schema(b"4s\xd3\x8a\x12A", "U8", &[]),
            variant_schema(b"\xef\x09\xbc\x80\xcdb", "U16", &[]),
            variant_schema(b"\x80\x07$\x86\xf2\xea", "U32", &[]),
            variant_schema(b"\x84\xb8\x9e\x22ET", "U64", &[]),
            variant_schema(b"\x0c\x80\x0d\xc4\xf9I", "U128", &[]),
            variant_schema(b"\x82%Z\xeel\xcc", "F32", &[]),
            variant_schema(b"\xa4M\x7ft\xc8)", "F64", &[]),
            variant_schema(b"I\xf4\x11\xf0\xa1\xa7", "Bool", &[]),
            variant_schema(b"NTY`\xf1\xbf", "Unit", &[]),
            variant_schema(
                b"\xa0?\xf2#F\x9f",
                "CheckedNumeric",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "enum@9dabba6a3395")],
            ),
            variant_schema(b"c\x00\xef\x80\x0b\xb8", "Never", &[]),
            variant_schema(b"\xe6\xf5\xc36\x16\xf4", "UnitFunction", &[]),
            variant_schema(b"\x03\xa6\xc5\x01j\x86", "EnumTag", &[]),
            variant_schema(b"\xc3\xf45\xc7(Q", "CheckedInt", &[]),
            variant_schema(
                b"\xf16\xeex\xc3;",
                "Tuple",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "list<enum@0f36ea9a02c1>")],
            ),
            variant_schema(b"\xb2\xef#\x0e\x7fO", "String", &[]),
            variant_schema(
                b"\xbd\xb4\xa2v\x05;",
                "TupleRef",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "list<enum@0f36ea9a02c1>")],
            ),
            variant_schema(
                b"\xaeV\xd77[\xff",
                "TupleMutRef",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "list<enum@0f36ea9a02c1>")],
            ),
            variant_schema(b"\xbfkl\x9f\xdb\xc5", "StringRef", &[]),
            variant_schema(
                b"\x97 \xde\x22\xd7\xd4",
                "ScalarRef",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "list<enum@bf18bf9e46ed>")],
            ),
            variant_schema(
                b"\xcbeb\x95hA",
                "ScalarMutRef",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "list<enum@bf18bf9e46ed>")],
            ),
            variant_schema(b"\xa45\x1e\xa3g\xb4", "StringMutRef", &[]),
            variant_schema(b"\x18Q}3-\x1b", "Decimal", &[]),
            variant_schema(b"O\xab\xe1W\xe8\xf9", "FrozenStr", &[]),
            variant_schema(b"\xa5\x1aU\xd1;\xee", "FrozenBytes", &[]),
            variant_schema(
                b"`ar\x85\xa4\xcb",
                "FrozenCollection",
                &[
                    ModelField::new(b"_\xec\xebf\xff\xc8", "0", "string"),
                    ModelField::new(b"k\x86\xb2s\xff4", "1", "list<enum@bf18bf9e46ed>"),
                ],
            ),
            variant_schema(
                b"T\xf9p\x0cZ3",
                "FrozenCollectionRef",
                &[
                    ModelField::new(b"_\xec\xebf\xff\xc8", "0", "string"),
                    ModelField::new(b"k\x86\xb2s\xff4", "1", "list<enum@bf18bf9e46ed>"),
                ],
            ),
            variant_schema(
                b"\xeb\x90\x0dwd\xd0",
                "TypeToken",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "list<enum@bf18bf9e46ed>")],
            ),
            variant_schema(b"\xd5\xdf\x07x\xa5\x0a", "StaticText", &[]),
            variant_schema(b"_\x93\x11xcg", "StaticBytes", &[]),
            variant_schema(b"\xbd\xfa\xef\x14\xbe\x91", "ValidationError", &[]),
            variant_schema(b"\x1ejG\x9a\xae\xf1", "ValidationErrorRef", &[]),
            variant_schema(b"(+u\x8f\x05\x8a", "StrRef", &[]),
            variant_schema(
                b"\x85~Q\x14\x0d\x18",
                "StringArray",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "i64")],
            ),
            variant_schema(
                b"\x8a\xe3R\x0a\xe0\xcb",
                "StrArray",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "i64")],
            ),
            variant_schema(
                b"\xeb2P\xd3\xfc<",
                "StringArrayRef",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "i64")],
            ),
            variant_schema(
                b"\xb1\x90\x07D|\x13",
                "StrArrayRef",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "i64")],
            ),
            variant_schema(
                b"T\x10\x8d\xaf\xaa\xc5",
                "Enum",
                &[
                    ModelField::new(b"_\xec\xebf\xff\xc8", "0", "i64"),
                    ModelField::new(b"k\x86\xb2s\xff4", "1", "string"),
                ],
            ),
            variant_schema(
                b"\xc0\xa35\x8f\x22\x01",
                "EnumRef",
                &[
                    ModelField::new(b"_\xec\xebf\xff\xc8", "0", "i64"),
                    ModelField::new(b"k\x86\xb2s\xff4", "1", "string"),
                ],
            ),
            variant_schema(
                b"zq\xac~]|",
                "EnumMutRef",
                &[
                    ModelField::new(b"_\xec\xebf\xff\xc8", "0", "i64"),
                    ModelField::new(b"k\x86\xb2s\xff4", "1", "string"),
                ],
            ),
            variant_schema(
                b"^,aL#\xf0",
                "Model",
                &[
                    ModelField::new(b"_\xec\xebf\xff\xc8", "0", "i64"),
                    ModelField::new(b"k\x86\xb2s\xff4", "1", "string"),
                ],
            ),
            variant_schema(
                b"C\xc3U\x17\xae\x15",
                "ModelRef",
                &[
                    ModelField::new(b"_\xec\xebf\xff\xc8", "0", "i64"),
                    ModelField::new(b"k\x86\xb2s\xff4", "1", "string"),
                ],
            ),
            variant_schema(
                b"\xb0\x02\x8e\xa2$\x98",
                "ModelMutRef",
                &[
                    ModelField::new(b"_\xec\xebf\xff\xc8", "0", "i64"),
                    ModelField::new(b"k\x86\xb2s\xff4", "1", "string"),
                ],
            ),
            variant_schema(b"\x10\x99\xe21t\xe8", "StringSlice", &[]),
            variant_schema(b"\xb7\xe7\x93\x84\xb2\xfb", "StrSlice", &[]),
            variant_schema(
                b"o /T\xa7\xb2",
                "List",
                &[
                    ModelField::new(b"_\xec\xebf\xff\xc8", "0", "enum@dd8db593bdc0"),
                    ModelField::new(b"k\x86\xb2s\xff4", "1", "i64"),
                ],
            ),
            variant_schema(
                b"W\x16\x82\x93!\xd4",
                "ListRef",
                &[
                    ModelField::new(b"_\xec\xebf\xff\xc8", "0", "enum@dd8db593bdc0"),
                    ModelField::new(b"k\x86\xb2s\xff4", "1", "i64"),
                ],
            ),
            variant_schema(
                b"/@\x05\xe8\xd2Y",
                "ListMutRef",
                &[
                    ModelField::new(b"_\xec\xebf\xff\xc8", "0", "enum@dd8db593bdc0"),
                    ModelField::new(b"k\x86\xb2s\xff4", "1", "i64"),
                ],
            ),
            variant_schema(
                b"\xfb\xa4\xbc\xc3,\x87",
                "ListIntoIter",
                &[
                    ModelField::new(b"_\xec\xebf\xff\xc8", "0", "enum@dd8db593bdc0"),
                    ModelField::new(b"k\x86\xb2s\xff4", "1", "i64"),
                ],
            ),
            variant_schema(
                b"W\xdeO;\xa8\x13",
                "ListIntoIterMutRef",
                &[
                    ModelField::new(b"_\xec\xebf\xff\xc8", "0", "enum@dd8db593bdc0"),
                    ModelField::new(b"k\x86\xb2s\xff4", "1", "i64"),
                ],
            ),
            variant_schema(
                b"\xb6\xf6\xf3\xad\x07\xb3",
                "Set",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "enum@dd8db593bdc0")],
            ),
            variant_schema(
                b"\x8a\xc6\x9b\x0e\xb7\xbb",
                "SetRef",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "enum@dd8db593bdc0")],
            ),
            variant_schema(
                b"\xcaydbR ",
                "SetMutRef",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "enum@dd8db593bdc0")],
            ),
            variant_schema(
                b"\x96\x11\xd3\xd8\x00\xfc",
                "Dict",
                &[
                    ModelField::new(b"_\xec\xebf\xff\xc8", "0", "enum@dd8db593bdc0"),
                    ModelField::new(b"k\x86\xb2s\xff4", "1", "enum@dd8db593bdc0"),
                ],
            ),
            variant_schema(
                b"\xc2\x1fVr6\xdd",
                "DictRef",
                &[
                    ModelField::new(b"_\xec\xebf\xff\xc8", "0", "enum@dd8db593bdc0"),
                    ModelField::new(b"k\x86\xb2s\xff4", "1", "enum@dd8db593bdc0"),
                ],
            ),
            variant_schema(
                b"b\xbe\xe3\xf3\xd1\xdb",
                "DictMutRef",
                &[
                    ModelField::new(b"_\xec\xebf\xff\xc8", "0", "enum@dd8db593bdc0"),
                    ModelField::new(b"k\x86\xb2s\xff4", "1", "enum@dd8db593bdc0"),
                ],
            ),
            variant_schema(b"\xaf\xd5\xff\xa5\xa9\x15", "RangeValue", &[]),
            variant_schema(
                b"\xda\x10<v\xc1;",
                "ZipIterator",
                &[
                    ModelField::new(b"_\xec\xebf\xff\xc8", "0", "enum@0f36ea9a02c1"),
                    ModelField::new(b"k\x86\xb2s\xff4", "1", "enum@0f36ea9a02c1"),
                ],
            ),
            variant_schema(
                b"\xc8O\xb0\x89\x03!",
                "ZipIteratorRef",
                &[
                    ModelField::new(b"_\xec\xebf\xff\xc8", "0", "enum@0f36ea9a02c1"),
                    ModelField::new(b"k\x86\xb2s\xff4", "1", "enum@0f36ea9a02c1"),
                ],
            ),
            variant_schema(
                b"|\x0a\x10\xd6\xf4\x8f",
                "ZipIteratorMutRef",
                &[
                    ModelField::new(b"_\xec\xebf\xff\xc8", "0", "enum@0f36ea9a02c1"),
                    ModelField::new(b"k\x86\xb2s\xff4", "1", "enum@0f36ea9a02c1"),
                ],
            ),
            variant_schema(
                b"\x1a\xde`\xab\xe9?",
                "Generator",
                &[
                    ModelField::new(b"_\xec\xebf\xff\xc8", "0", "enum@dd8db593bdc0"),
                    ModelField::new(b"k\x86\xb2s\xff4", "1", "i64"),
                ],
            ),
            variant_schema(
                b"A\xe1l\xc9\xd0E",
                "GeneratorMutRef",
                &[
                    ModelField::new(b"_\xec\xebf\xff\xc8", "0", "enum@dd8db593bdc0"),
                    ModelField::new(b"k\x86\xb2s\xff4", "1", "i64"),
                ],
            ),
            variant_schema(
                b"\xb9H#:\xc32",
                "GeneratorYield",
                &[
                    ModelField::new(b"_\xec\xebf\xff\xc8", "0", "enum@dd8db593bdc0"),
                    ModelField::new(b"k\x86\xb2s\xff4", "1", "i64"),
                ],
            ),
            variant_schema(
                b"=-X\x17O\xe0",
                "GeneratorYieldRef",
                &[
                    ModelField::new(b"_\xec\xebf\xff\xc8", "0", "enum@dd8db593bdc0"),
                    ModelField::new(b"k\x86\xb2s\xff4", "1", "i64"),
                ],
            ),
            variant_schema(
                b"\xa7\x0a\xca\xf0X`",
                "FunctionPointer",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "list<enum@bf18bf9e46ed>")],
            ),
            variant_schema(
                b"\xff\x92\x01\x14t\x9f",
                "FunctionItem",
                &[
                    ModelField::new(b"_\xec\xebf\xff\xc8", "0", "string"),
                    ModelField::new(b"k\x86\xb2s\xff4", "1", "list<enum@bf18bf9e46ed>"),
                ],
            ),
            variant_schema(
                b"\xe9\x0da}+U",
                "Closure",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "list<enum@bf18bf9e46ed>")],
            ),
            variant_schema(
                b"Y\x0f\xc7\xc6\x8e\x97",
                "NativeClosure",
                &[
                    ModelField::new(b"_\xec\xebf\xff\xc8", "0", "list<enum@bf18bf9e46ed>"),
                    ModelField::new(b"k\x86\xb2s\xff4", "1", "boolean"),
                    ModelField::new(b"\xd4s^:&^", "2", "boolean"),
                ],
            ),
            variant_schema(
                b"\x5cz%\xe2\x9cR",
                "NativeTraitObject",
                &[
                    ModelField::new(b"_\xec\xebf\xff\xc8", "0", "string"),
                    ModelField::new(b"k\x86\xb2s\xff4", "1", "boolean"),
                    ModelField::new(b"\xd4s^:&^", "2", "boolean"),
                ],
            ),
            variant_schema(
                b"V\xfe9\xc0\xf7\xb9",
                "SharedCallable",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "list<enum@bf18bf9e46ed>")],
            ),
            variant_schema(
                b"\xd3\x9eqf\x1f.",
                "ClosureOnce",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "list<enum@bf18bf9e46ed>")],
            ),
            variant_schema(
                b"\x7f&q\xb2)\xed",
                "ClosureRef",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "list<enum@bf18bf9e46ed>")],
            ),
            variant_schema(
                b"`\xa1v\x08\xb0\xf4",
                "BorrowingClosure",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "list<enum@bf18bf9e46ed>")],
            ),
            variant_schema(
                b"\xec\xdb\xa9E\xf7\xde",
                "BorrowingClosureRef",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "list<enum@bf18bf9e46ed>")],
            ),
            variant_schema(
                b"\xb7\xe5\xaa\xe2\xfbm",
                "CallableArguments",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "list<enum@bf18bf9e46ed>")],
            ),
            variant_schema(
                b"acl\xde\xf5G",
                "Future",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "list<enum@bf18bf9e46ed>")],
            ),
            variant_schema(b"\x89\xd2qrhj", "NativeStr", &[]),
            variant_schema(
                b"\x1f\x04\x02YL\xb8",
                "NativeSlice",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "list<enum@bf18bf9e46ed>")],
            ),
            variant_schema(
                b"\xa1\xf0\x0b\xce\x97\xfb",
                "SliceRef",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "list<enum@bf18bf9e46ed>")],
            ),
            variant_schema(
                b"\xabf`]\xf9\xcc",
                "SliceMutRef",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "list<enum@bf18bf9e46ed>")],
            ),
            variant_schema(
                b"0?\xb1)\xac\xe5",
                "NativeArray",
                &[
                    ModelField::new(b"_\xec\xebf\xff\xc8", "0", "list<enum@bf18bf9e46ed>"),
                    ModelField::new(b"k\x86\xb2s\xff4", "1", "i64"),
                ],
            ),
            variant_schema(
                b"x?\xbd{\xe2\x8f",
                "NativeArrayRef",
                &[
                    ModelField::new(b"_\xec\xebf\xff\xc8", "0", "list<enum@bf18bf9e46ed>"),
                    ModelField::new(b"k\x86\xb2s\xff4", "1", "i64"),
                ],
            ),
            variant_schema(
                b"o\xda\x01L\xed0",
                "NativeArrayMutRef",
                &[
                    ModelField::new(b"_\xec\xebf\xff\xc8", "0", "list<enum@bf18bf9e46ed>"),
                    ModelField::new(b"k\x86\xb2s\xff4", "1", "i64"),
                ],
            ),
            variant_schema(b"D,\xa28\xaa\xb8", "CChar", &[]),
            variant_schema(
                b"'\xee\xe0\xd7P=",
                "RawPointer",
                &[
                    ModelField::new(b"_\xec\xebf\xff\xc8", "0", "boolean"),
                    ModelField::new(b"k\x86\xb2s\xff4", "1", "list<enum@bf18bf9e46ed>"),
                ],
            ),
            variant_schema(
                b"\x1e\x82j\xeb<\xa2",
                "Boxed",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "list<enum@bf18bf9e46ed>")],
            ),
        ],
    )
}

fn schema_stored_layout_sized_numeric() -> Description {
    enum_schema(
        b"\x9d\xab\xbaj3\x95\x00",
        "incan.native-plan/5.layout_type_records.StoredLayoutSizedNumeric",
        vec![
            variant_schema(b"1Fb\x8c\x11\x8c", "I8", &[]),
            variant_schema(b"\xb9\xff?nq\xda", "I16", &[]),
            variant_schema(b"\xbe-,\xd01}", "I32", &[]),
            variant_schema(b"G\xeck\xdaM\xa3", "I128", &[]),
            variant_schema(b"4s\xd3\x8a\x12A", "U8", &[]),
            variant_schema(b"\xef\x09\xbc\x80\xcdb", "U16", &[]),
            variant_schema(b"\x80\x07$\x86\xf2\xea", "U32", &[]),
            variant_schema(b"\x84\xb8\x9e\x22ET", "U64", &[]),
            variant_schema(b"\x0c\x80\x0d\xc4\xf9I", "U128", &[]),
            variant_schema(b"\x82%Z\xeel\xcc", "F32", &[]),
            variant_schema(b"\xa4M\x7ft\xc8)", "F64", &[]),
            variant_schema(b"\xec|D\x91\xf6\x86", "ISize", &[]),
            variant_schema(b"\xbaL\xadu\xa6Z", "USize", &[]),
        ],
    )
}

fn schema_stored_layout_tuple_element() -> Description {
    enum_schema(
        b"\x0f6\xea\x9a\x02\xc1\x00",
        "incan.native-plan/5.layout_type_records.StoredLayoutTupleElement",
        vec![
            variant_schema(b"\x0b_`\x80p\xc6", "Int", &[]),
            variant_schema(b"\x1ai<\x00\xc4\x0c", "Float", &[]),
            variant_schema(
                b"6u-l\xd1G",
                "Numeric",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "enum@9dabba6a3395")],
            ),
            variant_schema(b"I\xf4\x11\xf0\xa1\xa7", "Bool", &[]),
            variant_schema(b"NTY`\xf1\xbf", "Unit", &[]),
            variant_schema(b"\xb2\xef#\x0e\x7fO", "String", &[]),
            variant_schema(
                b"o /T\xa7\xb2",
                "List",
                &[
                    ModelField::new(b"_\xec\xebf\xff\xc8", "0", "enum@dd8db593bdc0"),
                    ModelField::new(b"k\x86\xb2s\xff4", "1", "i64"),
                ],
            ),
            variant_schema(
                b"\x96\x11\xd3\xd8\x00\xfc",
                "Dict",
                &[
                    ModelField::new(b"_\xec\xebf\xff\xc8", "0", "enum@dd8db593bdc0"),
                    ModelField::new(b"k\x86\xb2s\xff4", "1", "enum@dd8db593bdc0"),
                ],
            ),
            variant_schema(
                b"\xb6\xf6\xf3\xad\x07\xb3",
                "Set",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "enum@dd8db593bdc0")],
            ),
            variant_schema(
                b"^,aL#\xf0",
                "Model",
                &[
                    ModelField::new(b"_\xec\xebf\xff\xc8", "0", "i64"),
                    ModelField::new(b"k\x86\xb2s\xff4", "1", "string"),
                ],
            ),
            variant_schema(
                b"T\x10\x8d\xaf\xaa\xc5",
                "Enum",
                &[
                    ModelField::new(b"_\xec\xebf\xff\xc8", "0", "i64"),
                    ModelField::new(b"k\x86\xb2s\xff4", "1", "string"),
                ],
            ),
            variant_schema(
                b"\xf16\xeex\xc3;",
                "Tuple",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "list<enum@0f36ea9a02c1>")],
            ),
            variant_schema(
                b"\xa7\x0a\xca\xf0X`",
                "FunctionPointer",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "list<enum@bf18bf9e46ed>")],
            ),
            variant_schema(
                b"q\xbf\x90\x93_\xc2",
                "Reference",
                &[
                    ModelField::new(b"_\xec\xebf\xff\xc8", "0", "boolean"),
                    ModelField::new(b"k\x86\xb2s\xff4", "1", "list<enum@bf18bf9e46ed>"),
                ],
            ),
            variant_schema(
                b"C\xc3U\x17\xae\x15",
                "ModelRef",
                &[
                    ModelField::new(b"_\xec\xebf\xff\xc8", "0", "i64"),
                    ModelField::new(b"k\x86\xb2s\xff4", "1", "string"),
                ],
            ),
            variant_schema(
                b"\xb0\x02\x8e\xa2$\x98",
                "ModelMutRef",
                &[
                    ModelField::new(b"_\xec\xebf\xff\xc8", "0", "i64"),
                    ModelField::new(b"k\x86\xb2s\xff4", "1", "string"),
                ],
            ),
            variant_schema(
                b"\xa1\xf0\x0b\xce\x97\xfb",
                "SliceRef",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "list<enum@bf18bf9e46ed>")],
            ),
            variant_schema(
                b"\xabf`]\xf9\xcc",
                "SliceMutRef",
                &[ModelField::new(b"_\xec\xebf\xff\xc8", "0", "list<enum@bf18bf9e46ed>")],
            ),
            variant_schema(b"O\xab\xe1W\xe8\xf9", "FrozenStr", &[]),
            variant_schema(b"\xd5\xdf\x07x\xa5\x0a", "StaticText", &[]),
            variant_schema(b"_\x93\x11xcg", "StaticBytes", &[]),
            variant_schema(b"\xa5\x1aU\xd1;\xee", "FrozenBytes", &[]),
            variant_schema(
                b"`ar\x85\xa4\xcb",
                "FrozenCollection",
                &[
                    ModelField::new(b"_\xec\xebf\xff\xc8", "0", "string"),
                    ModelField::new(b"k\x86\xb2s\xff4", "1", "list<enum@bf18bf9e46ed>"),
                ],
            ),
        ],
    )
}

fn schema_nominal_kind() -> Description {
    enum_schema(
        b"\xab\xff/G\x8c,\x00",
        "incan.native-plan/5.nominal_identities.NominalKind",
        vec![
            variant_schema(b"^,aL#\xf0", "Model", &[]),
            variant_schema(b"T\x10\x8d\xaf\xaa\xc5", "Enum", &[]),
        ],
    )
}

fn schema_product_nominal() -> Description {
    model_description(
        b"[\x08V#\xfe\xf4\x00",
        "incan.native-plan/5.nominal_identities.ProductNominal",
        &[
            ModelField::new(b"\x0b'\xc1X\xfe\xb3", "kind", "enum@abff2f478c2c"),
            ModelField::new(b"h\x9fjbs\x84", "identity", "string"),
            ModelField::new(b"\x0b\xf4t\x89cc", "digest", "string"),
        ],
    )
}

pub fn stored_product_descriptions() -> Vec<Description> {
    vec![
        schema_stored_constant(),
        schema_stored_field_step(),
        schema_stored_operand(),
        schema_stored_place(),
        schema_stored_projection(),
        schema_stored_read(),
        schema_stored_rvalue(),
        schema_body_records_span(),
        schema_stored_statement(),
        schema_stored_statement_kind(),
        schema_stored_type(),
        schema_stored_block(),
        schema_stored_exit(),
        schema_stored_external(),
        schema_stored_function(),
        schema_stored_loan(),
        schema_stored_parameter(),
        schema_stored_product(),
        schema_stored_slot(),
        schema_stored_target(),
        schema_stored_unwind(),
        schema_stored_layout_list_leaf(),
        schema_stored_layout_plan_type(),
        schema_stored_layout_sized_numeric(),
        schema_stored_layout_tuple_element(),
        schema_nominal_kind(),
        schema_product_nominal(),
    ]
}

pub fn stored_function_descriptions() -> Vec<Description> {
    vec![
        schema_stored_constant(),
        schema_stored_field_step(),
        schema_stored_operand(),
        schema_stored_place(),
        schema_stored_projection(),
        schema_stored_read(),
        schema_stored_rvalue(),
        schema_body_records_span(),
        schema_stored_statement(),
        schema_stored_statement_kind(),
        schema_stored_type(),
        schema_stored_block(),
        schema_stored_exit(),
        schema_stored_function(),
        schema_stored_loan(),
        schema_stored_parameter(),
        schema_stored_slot(),
        schema_stored_target(),
        schema_stored_unwind(),
        schema_stored_layout_list_leaf(),
        schema_stored_layout_plan_type(),
        schema_stored_layout_sized_numeric(),
        schema_stored_layout_tuple_element(),
    ]
}

pub fn stored_external_descriptions() -> Vec<Description> {
    vec![
        schema_body_records_span(),
        schema_stored_type(),
        schema_stored_external(),
        schema_stored_layout_list_leaf(),
        schema_stored_layout_plan_type(),
        schema_stored_layout_sized_numeric(),
        schema_stored_layout_tuple_element(),
    ]
}

pub fn stored_layout_model_declaration_descriptions() -> Vec<Description> {
    vec![
        schema_stored_layout_fallible_iterator_methods(),
        schema_stored_layout_field_lifecycle(),
        schema_stored_layout_field_metadata(),
        schema_stored_layout_iterator_methods(),
        schema_stored_layout_model_declaration(),
        schema_stored_layout_native_trait_adoption(),
        schema_stored_layout_native_trait_method(),
        schema_stored_layout_parameter(),
        schema_layout_records_span(),
        schema_stored_layout_list_leaf(),
        schema_stored_layout_plan_type(),
        schema_stored_layout_sized_numeric(),
        schema_stored_layout_tuple_element(),
    ]
}

pub fn stored_layout_enum_declaration_descriptions() -> Vec<Description> {
    vec![
        schema_stored_layout_enum_declaration(),
        schema_stored_layout_enum_variant(),
        schema_layout_records_span(),
        schema_stored_layout_carrier(),
        schema_stored_layout_checked_callable_param(),
        schema_stored_layout_checked_callable_param_kind(),
        schema_stored_layout_checked_input_access(),
        schema_stored_layout_checked_numeric_type(),
        schema_stored_layout_checked_primitive_type(),
        schema_stored_layout_checked_runtime_builtin(),
        schema_stored_layout_checked_type(),
        schema_stored_layout_list_leaf(),
        schema_stored_layout_plan_type(),
        schema_stored_layout_sized_numeric(),
        schema_stored_layout_tuple_element(),
    ]
}

pub fn stored_layout_zip_carrier_descriptions() -> Vec<Description> {
    vec![
        schema_stored_layout_zip_carrier(),
        schema_stored_layout_list_leaf(),
        schema_stored_layout_plan_type(),
        schema_stored_layout_sized_numeric(),
        schema_stored_layout_tuple_element(),
    ]
}

pub fn stored_layout_checked_type_descriptions() -> Vec<Description> {
    vec![
        schema_stored_layout_checked_callable_param(),
        schema_stored_layout_checked_callable_param_kind(),
        schema_stored_layout_checked_input_access(),
        schema_stored_layout_checked_numeric_type(),
        schema_stored_layout_checked_primitive_type(),
        schema_stored_layout_checked_runtime_builtin(),
        schema_stored_layout_checked_type(),
    ]
}

pub fn stored_instance_request_descriptions() -> Vec<Description> {
    vec![
        schema_stored_instance_request(),
        schema_stored_layout_checked_callable_param(),
        schema_stored_layout_checked_callable_param_kind(),
        schema_stored_layout_checked_input_access(),
        schema_stored_layout_checked_numeric_type(),
        schema_stored_layout_checked_primitive_type(),
        schema_stored_layout_checked_runtime_builtin(),
        schema_stored_layout_checked_type(),
    ]
}
