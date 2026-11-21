//! Projections of the native-plan profile's `function_records` shapes onto Crumb values.
//!
//! Each projection writes or reads one stored shape in its declared field and alternative order; record fields
//! are read from their identity-ordered slots. A shape here and its description in `wire_schema.rs` change
//! together.

use crate::records::prelude::*;

/// The stored form of `StoredBlock` in the native-plan profile.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct StoredBlock {
    pub(crate) statements: Vec<StoredStatement>,
    pub(crate) exit: StoredExit,
    pub(crate) exit_span: StoredSpan,
    pub(crate) cleanup: bool,
    pub(crate) span: StoredSpan,
}

/// Project one `StoredBlock` onto its schema-admitted Crumb record.
pub(crate) fn wire_stored_block(value: &StoredBlock) -> Result<Wire, String> {
    Ok(Wire::Record(vec![
        required(b"U\xa8\x1c\xf2\x1eq", list(&value.statements, wire_stored_statement)?),
        required(b"\xe5\x96\x89\x9f\x11K", wire_stored_exit(&value.exit)?),
        required(b"\x927\x9a<f\xb7", wire_stored_span(&value.exit_span)?),
        required(b"a\x14\x96\xf4\x12\xca", Wire::Boolean(value.cleanup)),
        required(b"7\xa0\x02Z\xa1\xf2", wire_stored_span(&value.span)?),
    ]))
}

/// Read one `StoredBlock` record in its identity-ordered field slots.
pub(crate) fn read_stored_block(value: &Wire, positions: &WirePositions) -> Result<StoredBlock, String> {
    let fields = wire_fields(value, 5)?;
    Ok(StoredBlock {
        statements: read_list(&fields[1].value, |item0| read_stored_statement(item0, positions))?,
        exit: read_stored_exit(&fields[4].value, positions)?,
        exit_span: read_stored_span(&fields[3].value, positions)?,
        cleanup: boolean_value(&fields[2].value)?,
        span: read_stored_span(&fields[0].value, positions)?,
    })
}

/// The stored form of `StoredExit` in the native-plan profile.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum StoredExit {
    Goto(i64),
    Switch(StoredOperand, i64, i64),
    Call(
        StoredTarget,
        StoredSpan,
        Vec<StoredOperand>,
        StoredPlace,
        i64,
        StoredUnwind,
    ),
    Drop(StoredPlace, i64, StoredUnwind),
    Overflow(StoredOperand, i64, StoredOperand, StoredOperand, i64, StoredUnwind),
    Return,
    Unreachable,
    Resume,
}

/// Project one `StoredExit` alternative onto its identity and positional payload.
pub(crate) fn wire_stored_exit(value: &StoredExit) -> Result<Wire, String> {
    Ok(match value {
        StoredExit::Goto(field0) => alternative(b"\xf1\x07\x91\x11\xa1\xc1", vec![Wire::integer(*field0)]),
        StoredExit::Switch(field0, field1, field2) => alternative(
            b"9\x92\x1at\x0b\xf2",
            vec![
                wire_stored_operand(field0)?,
                Wire::integer(*field1),
                Wire::integer(*field2),
            ],
        ),
        StoredExit::Call(field0, field1, field2, field3, field4, field5) => alternative(
            b"\xd6\xe6E\xb7\xd2\xb2",
            vec![
                wire_stored_target(field0)?,
                wire_stored_span(field1)?,
                list(field2, wire_stored_operand)?,
                wire_stored_place(field3)?,
                Wire::integer(*field4),
                wire_stored_unwind(field5)?,
            ],
        ),
        StoredExit::Drop(field0, field1, field2) => alternative(
            b"\xc8\xaf\x9a\xe3zH",
            vec![
                wire_stored_place(field0)?,
                Wire::integer(*field1),
                wire_stored_unwind(field2)?,
            ],
        ),
        StoredExit::Overflow(field0, field1, field2, field3, field4, field5) => alternative(
            b"\x17\xb8\xdc\xf5>K",
            vec![
                wire_stored_operand(field0)?,
                Wire::integer(*field1),
                wire_stored_operand(field2)?,
                wire_stored_operand(field3)?,
                Wire::integer(*field4),
                wire_stored_unwind(field5)?,
            ],
        ),
        StoredExit::Return => unit(b"k\x01C\xd08d"),
        StoredExit::Unreachable => unit(b"\xab\xaaF\xad\xb4\xe1"),
        StoredExit::Resume => unit(b"\xd6@\xc7B\x1d\xa0"),
    })
}

/// Read one `StoredExit` alternative by its identity, requiring its exact payload arity.
pub(crate) fn read_stored_exit(value: &Wire, positions: &WirePositions) -> Result<StoredExit, String> {
    let (identity, fields) = wire_variant(value)?;
    read_stored_exit_alternative(identity, fields, positions)
}

/// Restore one `StoredExit` alternative from part 0 of its identity table.
fn read_stored_exit_alternative(
    identity: &[u8],
    fields: &[WireField],
    positions: &WirePositions,
) -> Result<StoredExit, String> {
    Ok(match identity {
        b"\xf1\x07\x91\x11\xa1\xc1" if fields.len() == 1 => StoredExit::Goto(number(&fields[0].value)?),
        b"9\x92\x1at\x0b\xf2" if fields.len() == 3 => StoredExit::Switch(
            read_stored_operand(&fields[0].value, positions)?,
            number(&fields[1].value)?,
            number(&fields[2].value)?,
        ),
        b"\xd6\xe6E\xb7\xd2\xb2" if fields.len() == 6 => StoredExit::Call(
            read_stored_target(&fields[2].value, positions)?,
            read_stored_span(&fields[3].value, positions)?,
            read_list(&fields[4].value, |item0| read_stored_operand(item0, positions))?,
            read_stored_place(&fields[1].value, positions)?,
            number(&fields[0].value)?,
            read_stored_unwind(&fields[5].value, positions)?,
        ),
        b"\xc8\xaf\x9a\xe3zH" if fields.len() == 3 => StoredExit::Drop(
            read_stored_place(&fields[0].value, positions)?,
            number(&fields[1].value)?,
            read_stored_unwind(&fields[2].value, positions)?,
        ),
        b"\x17\xb8\xdc\xf5>K" if fields.len() == 6 => StoredExit::Overflow(
            read_stored_operand(&fields[2].value, positions)?,
            number(&fields[3].value)?,
            read_stored_operand(&fields[4].value, positions)?,
            read_stored_operand(&fields[1].value, positions)?,
            number(&fields[0].value)?,
            read_stored_unwind(&fields[5].value, positions)?,
        ),
        b"k\x01C\xd08d" if fields.is_empty() => StoredExit::Return,
        b"\xab\xaaF\xad\xb4\xe1" if fields.is_empty() => StoredExit::Unreachable,
        b"\xd6@\xc7B\x1d\xa0" if fields.is_empty() => StoredExit::Resume,
        _ => return Err("unknown or incomplete StoredExit alternative".to_owned()),
    })
}

/// The stored form of `StoredExternal` in the native-plan profile.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct StoredExternal {
    pub(crate) path: String,
    pub(crate) arguments: Vec<StoredType>,
    pub(crate) parameters: Vec<StoredType>,
    pub(crate) result: StoredType,
    pub(crate) span: StoredSpan,
}

/// Project one `StoredExternal` onto its schema-admitted Crumb record.
pub(crate) fn wire_stored_external(value: &StoredExternal) -> Result<Wire, String> {
    Ok(Wire::Record(vec![
        required(b"\xa0\xaf\x9f\x86[\xf6", Wire::Text(value.path.clone())),
        required(b"\xb1\x92\x01}\xa4\x7f", list(&value.arguments, wire_stored_type)?),
        required(b"\xf5(!*Im", list(&value.parameters, wire_stored_type)?),
        required(b"\xf6\xa2\x14\xf7\xa5\xfc", wire_stored_type(&value.result)?),
        required(b"7\xa0\x02Z\xa1\xf2", wire_stored_span(&value.span)?),
    ]))
}

/// Read one `StoredExternal` record in its identity-ordered field slots.
pub(crate) fn read_stored_external(value: &Wire, positions: &WirePositions) -> Result<StoredExternal, String> {
    let fields = wire_fields(value, 5)?;
    Ok(StoredExternal {
        path: text(&fields[1].value)?,
        arguments: read_list(&fields[2].value, |item0| read_stored_type(item0, positions))?,
        parameters: read_list(&fields[3].value, |item0| read_stored_type(item0, positions))?,
        result: read_stored_type(&fields[4].value, positions)?,
        span: read_stored_span(&fields[0].value, positions)?,
    })
}

/// The stored form of `StoredFunction` in the native-plan profile.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct StoredFunction {
    pub(crate) schema: i64,
    pub(crate) name: String,
    pub(crate) parameters: Vec<StoredParameter>,
    pub(crate) result: StoredType,
    pub(crate) locals: Vec<StoredSlot>,
    pub(crate) blocks: Vec<StoredBlock>,
    pub(crate) span: StoredSpan,
    pub(crate) lifetimes: Vec<Vec<String>>,
    pub(crate) loans: Vec<StoredLoan>,
    pub(crate) retained_result_source: i64,
}

/// Project one `StoredFunction` onto its schema-admitted Crumb record.
pub(crate) fn wire_stored_function(value: &StoredFunction) -> Result<Wire, String> {
    Ok(Wire::Record(vec![
        required(b"\xdf\x0a\xd6\xe48\x80", Wire::integer(value.schema)),
        required(b"\x82\xa3S\x7f\xf0\xdb", Wire::Text(value.name.clone())),
        required(b"\xf5(!*Im", list(&value.parameters, wire_stored_parameter)?),
        required(b"\xf6\xa2\x14\xf7\xa5\xfc", wire_stored_type(&value.result)?),
        required(b"\xe9\xe8\xd4\xb1\x93\xcf", list(&value.locals, wire_stored_slot)?),
        required(b"*\x12\xda\x17\xd2|", list(&value.blocks, wire_stored_block)?),
        required(b"7\xa0\x02Z\xa1\xf2", wire_stored_span(&value.span)?),
        required(
            b"8\x05\xee\xed\xa5\xe1",
            list(&value.lifetimes, |item0| {
                list(item0, |item1| Ok(Wire::Text(item1.clone())))
            })?,
        ),
        required(b"\xbe\x9a\xa7IM\x82", list(&value.loans, wire_stored_loan)?),
        required(b"\x01\x8a\x9c\xec\xfa\x93", Wire::integer(value.retained_result_source)),
    ]))
}

/// Read one `StoredFunction` record in its identity-ordered field slots.
pub(crate) fn read_stored_function(value: &Wire, positions: &WirePositions) -> Result<StoredFunction, String> {
    let fields = wire_fields(value, 10)?;
    Ok(StoredFunction {
        schema: number(&fields[6].value)?,
        name: text(&fields[4].value)?,
        parameters: read_list(&fields[8].value, |item0| read_stored_parameter(item0, positions))?,
        result: read_stored_type(&fields[9].value, positions)?,
        locals: read_list(&fields[7].value, |item0| read_stored_slot(item0, positions))?,
        blocks: read_list(&fields[1].value, |item0| read_stored_block(item0, positions))?,
        span: read_stored_span(&fields[2].value, positions)?,
        lifetimes: read_list(&fields[3].value, |item0| read_list(item0, text))?,
        loans: read_list(&fields[5].value, |item0| read_stored_loan(item0, positions))?,
        retained_result_source: number(&fields[0].value)?,
    })
}

/// The stored form of `StoredLoan` in the native-plan profile.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct StoredLoan {
    pub(crate) slot: i64,
    pub(crate) path: Vec<i64>,
    pub(crate) parameter: i64,
    pub(crate) ty: StoredType,
}

/// Project one `StoredLoan` onto its schema-admitted Crumb record.
pub(crate) fn wire_stored_loan(value: &StoredLoan) -> Result<Wire, String> {
    Ok(Wire::Record(vec![
        required(b"eX\x83\x831\xb7", Wire::integer(value.slot)),
        required(
            b"\xa0\xaf\x9f\x86[\xf6",
            list(&value.path, |item0| Ok(Wire::integer(*item0)))?,
        ),
        required(b"c\x906\x1d\xfd\xff", Wire::integer(value.parameter)),
        required(b"O\x9d\xb7\xf8\xe5\x06", wire_stored_type(&value.ty)?),
    ]))
}

/// Read one `StoredLoan` record in its identity-ordered field slots.
pub(crate) fn read_stored_loan(value: &Wire, positions: &WirePositions) -> Result<StoredLoan, String> {
    let fields = wire_fields(value, 4)?;
    Ok(StoredLoan {
        slot: number(&fields[2].value)?,
        path: read_list(&fields[3].value, number)?,
        parameter: number(&fields[1].value)?,
        ty: read_stored_type(&fields[0].value, positions)?,
    })
}

/// The stored form of `StoredParameter` in the native-plan profile.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct StoredParameter {
    pub(crate) name: String,
    pub(crate) slot: StoredSlot,
}

/// Project one `StoredParameter` onto its schema-admitted Crumb record.
pub(crate) fn wire_stored_parameter(value: &StoredParameter) -> Result<Wire, String> {
    Ok(Wire::Record(vec![
        required(b"\x82\xa3S\x7f\xf0\xdb", Wire::Text(value.name.clone())),
        required(b"eX\x83\x831\xb7", wire_stored_slot(&value.slot)?),
    ]))
}

/// Read one `StoredParameter` record in its identity-ordered field slots.
pub(crate) fn read_stored_parameter(value: &Wire, positions: &WirePositions) -> Result<StoredParameter, String> {
    let fields = wire_fields(value, 2)?;
    Ok(StoredParameter {
        name: text(&fields[1].value)?,
        slot: read_stored_slot(&fields[0].value, positions)?,
    })
}

/// The stored form of `StoredProduct` in the native-plan profile.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct StoredProduct {
    pub(crate) schema: i64,
    pub(crate) function: StoredFunction,
    pub(crate) runtime: Vec<StoredExternal>,
    pub(crate) nominals: Vec<ProductNominal>,
    pub(crate) frames: Vec<StoredFunction>,
}

/// Project one `StoredProduct` onto its schema-admitted Crumb record.
pub(crate) fn wire_stored_product(value: &StoredProduct) -> Result<Wire, String> {
    Ok(Wire::Record(vec![
        required(b"\xdf\x0a\xd6\xe48\x80", Wire::integer(value.schema)),
        required(b"x\xf9\xac\x01\x8eU", wire_stored_function(&value.function)?),
        required(b"\xd9,j\x81\xb2\xff", list(&value.runtime, wire_stored_external)?),
        required(b"wYS3\xedF", list(&value.nominals, wire_product_nominal)?),
        required(b"YL\xfd&\x07\xd4", list(&value.frames, wire_stored_function)?),
    ]))
}

/// Read one `StoredProduct` record in its identity-ordered field slots.
pub(crate) fn read_stored_product(value: &Wire, positions: &WirePositions) -> Result<StoredProduct, String> {
    let fields = wire_fields(value, 5)?;
    Ok(StoredProduct {
        schema: number(&fields[4].value)?,
        function: read_stored_function(&fields[2].value, positions)?,
        runtime: read_list(&fields[3].value, |item0| read_stored_external(item0, positions))?,
        nominals: read_list(&fields[1].value, |item0| read_product_nominal(item0, positions))?,
        frames: read_list(&fields[0].value, |item0| read_stored_function(item0, positions))?,
    })
}

/// The stored form of `StoredSlot` in the native-plan profile.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct StoredSlot {
    pub(crate) ty: StoredType,
    pub(crate) span: StoredSpan,
}

/// Project one `StoredSlot` onto its schema-admitted Crumb record.
pub(crate) fn wire_stored_slot(value: &StoredSlot) -> Result<Wire, String> {
    Ok(Wire::Record(vec![
        required(b"O\x9d\xb7\xf8\xe5\x06", wire_stored_type(&value.ty)?),
        required(b"7\xa0\x02Z\xa1\xf2", wire_stored_span(&value.span)?),
    ]))
}

/// Read one `StoredSlot` record in its identity-ordered field slots.
pub(crate) fn read_stored_slot(value: &Wire, positions: &WirePositions) -> Result<StoredSlot, String> {
    let fields = wire_fields(value, 2)?;
    Ok(StoredSlot {
        ty: read_stored_type(&fields[1].value, positions)?,
        span: read_stored_span(&fields[0].value, positions)?,
    })
}

/// The stored form of `StoredTarget` in the native-plan profile.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum StoredTarget {
    Planned(String),
    External(String),
    Instantiated(String, Vec<StoredType>),
    CloneModel(i64),
    CloneEnum(i64),
    ClosureBody(String, i64),
    ClosureBodyOnce(String, i64),
    CallClosure(Vec<StoredType>),
    CallClosureOnce(Vec<StoredType>),
    CallBorrowingClosure(Vec<StoredType>),
    SharedCallableBody(Vec<StoredType>),
    CloneClosure(Vec<StoredType>),
    Value(StoredOperand),
    FormatValue(StoredType, bool),
}

/// Project one `StoredTarget` alternative onto its identity and positional payload.
pub(crate) fn wire_stored_target(value: &StoredTarget) -> Result<Wire, String> {
    Ok(match value {
        StoredTarget::Planned(field0) => alternative(b"\xfbo|\x93\xe6\xbf", vec![Wire::Text(field0.clone())]),
        StoredTarget::External(field0) => alternative(b"h\xc1\x14\xea\x9c\x8c", vec![Wire::Text(field0.clone())]),
        StoredTarget::Instantiated(field0, field1) => alternative(
            b"\x12\xfds\xf8Q\xe0",
            vec![Wire::Text(field0.clone()), list(field1, wire_stored_type)?],
        ),
        StoredTarget::CloneModel(field0) => alternative(b"\xc8\x948\xc6\xc9\xb9", vec![Wire::integer(*field0)]),
        StoredTarget::CloneEnum(field0) => alternative(b"P\x03\xf3_;\xc1", vec![Wire::integer(*field0)]),
        StoredTarget::ClosureBody(field0, field1) => alternative(
            b"\x81@\xe21\x84\x11",
            vec![Wire::Text(field0.clone()), Wire::integer(*field1)],
        ),
        StoredTarget::ClosureBodyOnce(field0, field1) => {
            alternative(b"q9EI8$", vec![Wire::Text(field0.clone()), Wire::integer(*field1)])
        }
        StoredTarget::CallClosure(field0) => {
            alternative(b"\xb7i\xac\x8f\xcd\xa0", vec![list(field0, wire_stored_type)?])
        }
        StoredTarget::CallClosureOnce(field0) => {
            alternative(b"\xc4i\xacQ\xb1\x89", vec![list(field0, wire_stored_type)?])
        }
        StoredTarget::CallBorrowingClosure(field0) => {
            alternative(b"\xa3C\xf89\x18\xe2", vec![list(field0, wire_stored_type)?])
        }
        StoredTarget::SharedCallableBody(field0) => {
            alternative(b"\xd76^\xdcH\xed", vec![list(field0, wire_stored_type)?])
        }
        StoredTarget::CloneClosure(field0) => {
            alternative(b"\x07\xb9v\xcd\xac\xf6", vec![list(field0, wire_stored_type)?])
        }
        StoredTarget::Value(field0) => alternative(b"\x8e7\x95=#\xda", vec![wire_stored_operand(field0)?]),
        StoredTarget::FormatValue(field0, field1) => alternative(
            b"\xd6\x8a\xf6\x95\xfc\x92",
            vec![wire_stored_type(field0)?, Wire::Boolean(*field1)],
        ),
    })
}

/// Read one `StoredTarget` alternative by its identity, requiring its exact payload arity.
pub(crate) fn read_stored_target(value: &Wire, positions: &WirePositions) -> Result<StoredTarget, String> {
    let (identity, fields) = wire_variant(value)?;
    read_stored_target_alternative(identity, fields, positions)
}

/// Restore one `StoredTarget` alternative from part 0 of its identity table.
fn read_stored_target_alternative(
    identity: &[u8],
    fields: &[WireField],
    positions: &WirePositions,
) -> Result<StoredTarget, String> {
    Ok(match identity {
        b"\xfbo|\x93\xe6\xbf" if fields.len() == 1 => StoredTarget::Planned(text(&fields[0].value)?),
        b"h\xc1\x14\xea\x9c\x8c" if fields.len() == 1 => StoredTarget::External(text(&fields[0].value)?),
        b"\x12\xfds\xf8Q\xe0" if fields.len() == 2 => StoredTarget::Instantiated(
            text(&fields[0].value)?,
            read_list(&fields[1].value, |item0| read_stored_type(item0, positions))?,
        ),
        b"\xc8\x948\xc6\xc9\xb9" if fields.len() == 1 => StoredTarget::CloneModel(number(&fields[0].value)?),
        b"P\x03\xf3_;\xc1" if fields.len() == 1 => StoredTarget::CloneEnum(number(&fields[0].value)?),
        b"\x81@\xe21\x84\x11" if fields.len() == 2 => {
            StoredTarget::ClosureBody(text(&fields[0].value)?, number(&fields[1].value)?)
        }
        b"q9EI8$" if fields.len() == 2 => {
            StoredTarget::ClosureBodyOnce(text(&fields[0].value)?, number(&fields[1].value)?)
        }
        b"\xb7i\xac\x8f\xcd\xa0" if fields.len() == 1 => {
            StoredTarget::CallClosure(read_list(&fields[0].value, |item0| read_stored_type(item0, positions))?)
        }
        b"\xc4i\xacQ\xb1\x89" if fields.len() == 1 => {
            StoredTarget::CallClosureOnce(read_list(&fields[0].value, |item0| read_stored_type(item0, positions))?)
        }
        b"\xa3C\xf89\x18\xe2" if fields.len() == 1 => {
            StoredTarget::CallBorrowingClosure(read_list(&fields[0].value, |item0| read_stored_type(item0, positions))?)
        }
        b"\xd76^\xdcH\xed" if fields.len() == 1 => {
            StoredTarget::SharedCallableBody(read_list(&fields[0].value, |item0| read_stored_type(item0, positions))?)
        }
        b"\x07\xb9v\xcd\xac\xf6" if fields.len() == 1 => {
            StoredTarget::CloneClosure(read_list(&fields[0].value, |item0| read_stored_type(item0, positions))?)
        }
        b"\x8e7\x95=#\xda" if fields.len() == 1 => {
            StoredTarget::Value(read_stored_operand(&fields[0].value, positions)?)
        }
        b"\xd6\x8a\xf6\x95\xfc\x92" if fields.len() == 2 => StoredTarget::FormatValue(
            read_stored_type(&fields[0].value, positions)?,
            boolean_value(&fields[1].value)?,
        ),
        _ => return Err("unknown or incomplete StoredTarget alternative".to_owned()),
    })
}

/// The stored form of `StoredUnwind` in the native-plan profile.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum StoredUnwind {
    Continue,
    Cleanup(i64),
    Terminate,
}

/// Project one `StoredUnwind` alternative onto its identity and positional payload.
pub(crate) fn wire_stored_unwind(value: &StoredUnwind) -> Result<Wire, String> {
    Ok(match value {
        StoredUnwind::Continue => unit(b"1\xfb\xef\x16%\x94"),
        StoredUnwind::Cleanup(field0) => alternative(b"\x9f\x1b#{]\xc3", vec![Wire::integer(*field0)]),
        StoredUnwind::Terminate => unit(b"\xf9\x13\xf0\x92\x10\xe8"),
    })
}

/// Read one `StoredUnwind` alternative by its identity, requiring its exact payload arity.
pub(crate) fn read_stored_unwind(value: &Wire, positions: &WirePositions) -> Result<StoredUnwind, String> {
    let (identity, fields) = wire_variant(value)?;
    read_stored_unwind_alternative(identity, fields, positions)
}

/// Restore one `StoredUnwind` alternative from part 0 of its identity table.
fn read_stored_unwind_alternative(
    identity: &[u8],
    fields: &[WireField],
    _positions: &WirePositions,
) -> Result<StoredUnwind, String> {
    Ok(match identity {
        b"1\xfb\xef\x16%\x94" if fields.is_empty() => StoredUnwind::Continue,
        b"\x9f\x1b#{]\xc3" if fields.len() == 1 => StoredUnwind::Cleanup(number(&fields[0].value)?),
        b"\xf9\x13\xf0\x92\x10\xe8" if fields.is_empty() => StoredUnwind::Terminate,
        _ => return Err("unknown or incomplete StoredUnwind alternative".to_owned()),
    })
}
