//! Materialize checked immutable numeric newtype values through the retained constructor layout.

use incan_mir_plan_native::{Operand, PlanType, RvalueKind, SourceSpan, assign as plan_assign};
use incan_semantics_core::body_ir::Constant as SourceConstant;

use crate::frame::{Builder, literal, read_place, temporary};
use crate::models::constructor_index;
use crate::numerics::numeric_constant;

/// Require one exact canonical owner, physical address, tuple slot and numeric width before producing a value.
pub fn numeric_newtype_literal(
    builder: &mut Builder,
    value: &SourceConstant,
    span: &SourceSpan,
) -> Result<Operand, String> {
    let SourceConstant::NumericNewtype(constant) = value else {
        return Err("unsupported Body IR non-newtype constant".into());
    };
    let mut found: Option<usize> = None;
    for (index, declaration) in builder.nominals().iter().enumerate() {
        if constant.constructor.canonical.as_ref() != Some(&declaration.canonical) {
            continue;
        }
        if found.is_some() {
            return Err("unsupported Body IR ambiguous numeric newtype constant layout".into());
        }
        if !constant.matches_layout(declaration) {
            return Err("unsupported Body IR invalid numeric newtype constant layout".into());
        }
        found = Some(index);
    }
    let Some(found) = found else {
        return Err("unsupported Body IR unretained numeric newtype constant layout".into());
    };
    let storage = PlanType::Model(found as i64, builder.nominals()[found].name.to_string());
    let owner = constructor_index(builder.nominals(), &constant.constructor, 1, &storage)?;
    let payload = literal(numeric_constant(&constant.value)?, span);
    let output = temporary(builder, storage, span);
    builder.emit(plan_assign(
        output.clone(),
        RvalueKind::Model(owner as i64, vec![payload]),
        span,
    ));
    Ok(read_place(output, true, span))
}
