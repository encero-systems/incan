//! Native Rust enum formatting and checked derived Display for source enums.

use incan_mir_plan_native::{Operand, Place, PlanType, RvalueKind, SourceSpan, assign as plan_assign};
use incan_semantics_core::SymbolOrigin;
use incan_semantics_core::body_ir::Operand as SourceOperand;

use crate::checked_values::scalar_place;
use crate::decls::library_enum_item;
use crate::element_places::is_element_place;
use crate::formatting::native_format_text;
use crate::frame::{Builder, place_type, read_place, temporary};

/// Use native Display for foreign layouts and retained derived Display for whole source enums.
pub fn display_enum_operand(
    builder: &mut Builder,
    value: &SourceOperand,
    span: &SourceSpan,
) -> Result<Option<Place>, String> {
    if let Some(text) = native_enum_text(builder, value, false, span)? {
        return Ok(Some(text));
    }
    let SourceOperand::Place(read) = value else {
        return Ok(None);
    };
    if !read.place.projection.is_empty() || read.place.local_id().is_none() {
        return Ok(None);
    }
    let place = scalar_place(builder, &read.place, span)?;
    let PlanType::Enum(owner, name) = place_type(&builder.locals, &place) else {
        return Ok(None);
    };
    if owner < 0 || owner as usize >= builder.enums().len() {
        return Ok(None);
    }
    let declaration = &builder.enums()[owner as usize];
    if !library_enum_item(declaration).is_empty() {
        let argument = enum_reference(builder, place, owner, &name, span);
        return Ok(Some(native_format_text(builder, argument, false, span)?));
    }
    if !declaration.derives.iter().any(|derive| derive == "Display") {
        return Ok(None);
    }
    Ok(Some(borrowed_enum_debug(builder, place, owner, &name, span)?))
}

/// Borrow an exact native enum local or structural field for Rust Display or Debug; trait validation stays in rustc.
pub fn native_enum_text(
    builder: &mut Builder,
    value: &SourceOperand,
    debug: bool,
    span: &SourceSpan,
) -> Result<Option<Place>, String> {
    let SourceOperand::Place(read) = value else {
        return Ok(None);
    };
    if read.place.local_id().is_none() || is_element_place(builder, &read.place)? {
        return Ok(None);
    }
    let place = scalar_place(builder, &read.place, span)?;
    let argument = match place_type(&builder.locals, &place) {
        PlanType::Enum(owner, name) => {
            if !native_enum_owner(builder, owner) {
                return Ok(None);
            }
            enum_reference(builder, place, owner, &name, span)
        }
        PlanType::EnumRef(owner, _) => {
            if !native_enum_owner(builder, owner) {
                return Ok(None);
            }
            read_place(place, false, span)
        }
        _ => return Ok(None),
    };
    Ok(Some(native_format_text(builder, argument, debug, span)?))
}

/// Require an authenticated Rust enum layout, independently of compiled source enum aliases and their derives.
pub fn native_enum_owner(builder: &Builder, owner: i64) -> bool {
    if owner < 0 || owner as usize >= builder.enums().len() {
        return false;
    }
    let declaration = &builder.enums()[owner as usize];
    if library_enum_item(declaration).is_empty() {
        return false;
    }
    matches!(declaration.canonical.origin, SymbolOrigin::RustCrate(_))
}

/// Format an enum through its retained Debug implementation without cloning or consuming its payload.
pub fn borrowed_enum_debug(
    builder: &mut Builder,
    value: Place,
    owner: i64,
    name: &str,
    span: &SourceSpan,
) -> Result<Place, String> {
    let argument = enum_reference(builder, value, owner, name, span);
    native_format_text(builder, argument, true, span)
}

/// Borrow the original enum storage for a native formatter without cloning or consuming its payload.
fn enum_reference(builder: &mut Builder, value: Place, owner: i64, name: &str, span: &SourceSpan) -> Operand {
    let reference = temporary(builder, PlanType::EnumRef(owner, name.to_owned()), span);
    builder.emit(plan_assign(reference.clone(), RvalueKind::Borrow(value), span));
    read_place(reference, false, span)
}
