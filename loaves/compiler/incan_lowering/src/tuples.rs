//! Tuple admission and checked constant field selection for the direct route.

use incan_lang::lang::types::collections::{CollectionTypeId, from_str as collection_type};
use incan_mir_plan_native::{
    FieldProjection, Place, PlanType, Projection, SourceSpan, TupleElement, tuple_element_type,
};
use incan_semantics_core::IncanPrimitiveType;
use incan_semantics_core::IncanType;
use incan_semantics_core::body_ir::{Constant, Operand, PlaceElem};

use crate::frame::appended_field;
use crate::numerics::{numeric_type, sized_numeric};

/// Name every canonical type kind even when its legacy Display implementation is empty.
pub fn type_name(ty: &IncanType) -> String {
    match ty {
        IncanType::NativeCChar | IncanType::NativePointer { .. } => ty.to_string(),
        IncanType::Tuple(elements) => {
            let names: Vec<String> = elements.iter().map(type_name).collect();
            format!("Tuple[{}]", names.join(", "))
        }
        IncanType::Function { .. } => "Function".into(),
        IncanType::Primitive(value) => value.to_string(),
        IncanType::Named(name) => format!("Named[{name}]"),
        IncanType::Trait(identity) => format!("Trait[{}]", identity.render_compact()),
        IncanType::Generic { base, args } => {
            let names: Vec<String> = args.iter().map(type_name).collect();
            format!("{base}[{}]", names.join(", "))
        }
        IncanType::Decimal { .. } => "Decimal".into(),
        IncanType::TypeToken(_) => "TypeToken".into(),
        IncanType::TypeVar(_) => "TypeVar".into(),
        IncanType::Ref(_) => "Ref".into(),
        IncanType::RefMut(_) => "RefMut".into(),
        IncanType::RustInteropPath(_) => "RustInteropPath".into(),
        IncanType::NativeArray { .. } => "NativeArray".into(),
        IncanType::NativeBuiltin(_) | IncanType::NativeTraitObject { .. } => ty.to_string(),
        IncanType::Unsupported(description) => format!("Unsupported[{description}]"),
        IncanType::Never => "Never".into(),
        IncanType::SelfType => "SelfType".into(),
        IncanType::Infer => "Infer".into(),
        IncanType::Unknown => "Unknown".into(),
    }
}

/// Annotation-shaped tuples through the same collection registry the Body IR frontend uses.
pub fn generic_tuple_type(ty: &IncanType) -> Result<PlanType, String> {
    if let IncanType::Generic { base, args } = ty
        && collection_type(base) == Some(CollectionTypeId::Tuple)
    {
        return tuple_type(args);
    }
    Err(format!("unsupported Body IR type {}", type_name(ty)))
}

/// Admit only tuple elements whose native scalar layout is already supported.
pub fn tuple_type(elements: &[IncanType]) -> Result<PlanType, String> {
    // Legacy emits a singleton literal as a parenthesized scalar; do not admit a different native layout.
    if elements.len() == 1 {
        return Err("a one-element tuple cannot be represented as a scalar".into());
    }
    let mut types = Vec::with_capacity(elements.len());
    for element in elements {
        let refused = || format!("unsupported Body IR tuple element type {}", type_name(element));
        let IncanType::Primitive(primitive) = element else {
            return Err(refused());
        };
        types.push(match primitive {
            IncanPrimitiveType::Int => TupleElement::Int,
            IncanPrimitiveType::Float => TupleElement::Float,
            IncanPrimitiveType::Numeric(kind) => match sized_numeric(&numeric_type(*kind)?) {
                Some(carrier) => TupleElement::Numeric(carrier),
                None => return Err(refused()),
            },
            IncanPrimitiveType::Bool => TupleElement::Bool,
            IncanPrimitiveType::Str => TupleElement::String,
            IncanPrimitiveType::FrozenStr => TupleElement::FrozenStr,
            IncanPrimitiveType::Unit => TupleElement::Unit,
            _ => return Err(refused()),
        });
    }
    Ok(PlanType::Tuple(types))
}

/// A resolved tuple element's scalar, text, collection, nominal, function-pointer or nested tuple layout.
pub fn tuple_element(ty: &PlanType, source: &IncanType) -> Result<TupleElement, String> {
    if let Some(kind) = sized_numeric(ty) {
        return Ok(TupleElement::Numeric(kind));
    }
    Ok(match ty {
        PlanType::Int => TupleElement::Int,
        PlanType::Float => TupleElement::Float,
        PlanType::Bool => TupleElement::Bool,
        PlanType::Unit => TupleElement::Unit,
        PlanType::String => TupleElement::String,
        PlanType::StrRef => TupleElement::Reference(false, vec![PlanType::NativeStr]),
        PlanType::StringRef => TupleElement::Reference(false, vec![PlanType::String]),
        PlanType::StringMutRef => TupleElement::Reference(true, vec![PlanType::String]),
        PlanType::FrozenStr => TupleElement::FrozenStr,
        PlanType::FrozenBytes => TupleElement::FrozenBytes,
        PlanType::StaticText => TupleElement::StaticText,
        PlanType::StaticBytes => TupleElement::StaticBytes,
        PlanType::FrozenCollection(name, elements) => TupleElement::FrozenCollection(name.clone(), elements.clone()),
        PlanType::List(leaf, depth) => TupleElement::List(leaf.clone(), *depth),
        PlanType::Dict(key, value) => TupleElement::Dict(key.clone(), value.clone()),
        PlanType::Set(leaf) => TupleElement::Set(leaf.clone()),
        PlanType::Model(owner, name) => TupleElement::Model(*owner, name.clone()),
        PlanType::Enum(owner, name) => TupleElement::Enum(*owner, name.clone()),
        PlanType::Tuple(elements) => TupleElement::Tuple(elements.clone()),
        PlanType::FunctionPointer(signature) => TupleElement::FunctionPointer(signature.clone()),
        PlanType::ScalarRef(elements) => TupleElement::Reference(false, elements.clone()),
        PlanType::ScalarMutRef(elements) => TupleElement::Reference(true, elements.clone()),
        PlanType::ModelRef(index, name) => TupleElement::ModelRef(*index, name.clone()),
        PlanType::ModelMutRef(index, name) => TupleElement::ModelMutRef(*index, name.clone()),
        PlanType::EnumRef(index, name) => TupleElement::Reference(false, vec![PlanType::Enum(*index, name.clone())]),
        PlanType::EnumMutRef(index, name) => TupleElement::Reference(true, vec![PlanType::Enum(*index, name.clone())]),
        PlanType::SliceRef(elements) => TupleElement::SliceRef(elements.clone()),
        PlanType::SliceMutRef(elements) => TupleElement::SliceMutRef(elements.clone()),
        _ => {
            return Err(format!(
                "unsupported Body IR tuple element type {} with carrier {ty:?}",
                type_name(source)
            ));
        }
    })
}

/// Identify tuple locals without guessing their layout from a source expression.
pub fn is_tuple_type(ty: &PlanType) -> bool {
    matches!(ty, PlanType::Tuple(_) | PlanType::TupleMutRef(_))
}

/// Accept a checker-proven structural field or a constant tuple index, including negative indices.
pub fn tuple_slot(projection: &PlaceElem, count: i64) -> Result<i64, String> {
    let mut slot = match projection {
        PlaceElem::Field { name, structural, .. } => {
            if !structural {
                return Err("unsupported Body IR nonstructural tuple field".into());
            }
            name.parse::<i64>().map_err(|_| format!("invalid tuple field {name}"))?
        }
        PlaceElem::Index(index) => match index.as_ref() {
            Operand::Constant(Constant::Int(value)) => *value,
            _ => return Err("unsupported Body IR nonconstant tuple index".into()),
        },
        _ => return Err("unsupported Body IR tuple slice".into()),
    };
    if matches!(projection, PlaceElem::Index(_)) && slot < 0 {
        slot += count;
    }
    if slot < 0 || slot >= count {
        return Err("unsupported Body IR tuple index outside layout".into());
    }
    Ok(slot)
}

/// Track a tuple as one owner only when at least one element requires destruction.
pub fn tuple_has_owners(ty: &PlanType) -> bool {
    match ty {
        PlanType::Tuple(elements) => elements.iter().any(element_has_owner),
        _ => false,
    }
}

/// Whether a tuple holds a list, carrier or tuple element, which only a whole-tuple Debug can spell.
pub fn tuple_debug_formatted(ty: &PlanType) -> bool {
    match ty {
        PlanType::Tuple(elements) => elements.iter().any(carrier_element),
        _ => false,
    }
}

/// Whether a tuple element carries a list, nominal value or nested tuple requiring its retained layout.
pub fn carrier_element(element: &TupleElement) -> bool {
    matches!(
        element,
        TupleElement::List(..)
            | TupleElement::Dict(..)
            | TupleElement::Set(_)
            | TupleElement::Model(..)
            | TupleElement::Enum(..)
            | TupleElement::Tuple(_)
    )
}

/// Whether a list, nominal or dependency-owned validation error keeps its enclosing tuple initialized.
pub fn owned_carrier(ty: &PlanType) -> bool {
    matches!(
        ty,
        PlanType::ValidationError
            | PlanType::List(..)
            | PlanType::Dict(..)
            | PlanType::Set(_)
            | PlanType::Model(..)
            | PlanType::Enum(..)
    )
}

/// Whether a tuple element may own storage; function-pointer fields remain Copy values.
pub fn element_has_owner(element: &TupleElement) -> bool {
    match element {
        TupleElement::String
        | TupleElement::List(..)
        | TupleElement::Dict(..)
        | TupleElement::Set(_)
        | TupleElement::Model(..)
        | TupleElement::Enum(..) => true,
        TupleElement::Tuple(elements) => elements.iter().any(element_has_owner),
        _ => false,
    }
}

/// Project the admitted tuple layout in canonical element order, after any field path that reached the tuple.
pub fn tuple_fields(value: &Place, ty: &PlanType, span: &SourceSpan) -> Result<Vec<Place>, String> {
    let mut owner = value.clone();
    let mut layout = ty.clone();
    if let PlanType::TupleRef(elements) | PlanType::TupleMutRef(elements) = ty {
        layout = PlanType::Tuple(elements.clone());
        if value.projection != Projection::Whole {
            return Err("unsupported Body IR projected tuple reference".into());
        }
        owner = Place {
            local: value.local,
            projection: Projection::Deref(layout.clone()),
            span: span.clone(),
        };
    }
    let mut fields = Vec::new();
    if let PlanType::Tuple(elements) = &layout {
        for (slot, element) in elements.iter().enumerate() {
            let element = tuple_element_type(element);
            if owner.projection == Projection::Whole {
                fields.push(Place {
                    local: owner.local,
                    projection: Projection::Field(slot as i64, element),
                    span: span.clone(),
                });
            } else {
                fields.push(appended_field(
                    &owner,
                    FieldProjection::new(slot as i64, element),
                    span,
                )?);
            }
        }
    }
    Ok(fields)
}
