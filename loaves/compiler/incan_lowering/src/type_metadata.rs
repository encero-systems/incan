//! Close checked generated type metadata against the same retained nominal declaration used for storage.

use incan_mir_plan_native::{Constant, PlanType, RvalueKind, SourceSpan, assign as plan_assign};
use incan_semantics_core::SemanticSourceTargetKind;
use incan_semantics_core::body_ir::{Place as SourcePlace, TypeMetadataOperation, TypeMetadataTarget};

use crate::checked_values::scalar_place;
use crate::frame::{Builder, drop_owned, literal, place_type, read_place, track_rvalue_moves};
use crate::models::model_type;
use crate::reflection::emit_field_metadata;
use crate::text_plan::owned_literal;
use crate::type_parameters::substitute;

/// Resolve only a frontend-approved pure metadata operation; missing layouts or reflection policies refuse.
pub fn lower_type_metadata(
    builder: &mut Builder,
    target: &TypeMetadataTarget,
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    if target.member.kind != SemanticSourceTargetKind::Method {
        return Err("unsupported Body IR type metadata without a generated method identity".into());
    }
    if !matches!(
        target.owner.kind,
        SemanticSourceTargetKind::GenericBinder
            | SemanticSourceTargetKind::Model
            | SemanticSourceTargetKind::Class
            | SemanticSourceTargetKind::Newtype
    ) {
        return Err("unsupported Body IR type metadata owner".into());
    }
    let closed = substitute(&target.receiver_type, &builder.type_parameters, &builder.type_arguments)?;
    let PlanType::Model(owner, _) = model_type(builder.constructors(), builder.nominals(), &closed, "")? else {
        return Err("unsupported Body IR type metadata without a retained nominal layout".into());
    };
    let declaration = &builder.nominals()[owner as usize];
    if !declaration.type_parameters.is_empty() {
        return Err("unsupported Body IR type metadata on an unclosed nominal layout".into());
    }
    let (value, result) = match target.operation {
        TypeMetadataOperation::Fields => {
            if target.member.declaration_name != "__fields__" {
                return Err("unsupported Body IR field metadata contract".into());
            }
            emit_field_metadata(builder, owner, destination, span)?;
            return Ok(());
        }
        TypeMetadataOperation::ClassName => {
            let class = matches!(
                declaration.canonical.kind,
                SemanticSourceTargetKind::Model | SemanticSourceTargetKind::Class
            );
            if target.member.declaration_name != "__class_name__" || !class {
                return Err("unsupported Body IR class-name metadata contract".into());
            }
            let class_name = declaration.canonical.declaration_name.to_string();
            let name = owned_literal(builder, &class_name, span);
            (read_place(name, true, span), PlanType::String)
        }
        TypeMetadataOperation::FieldCount => {
            if target.member.declaration_name != "__fields__" {
                return Err("unsupported Body IR field-count metadata contract".into());
            }
            let Some(fields) = &declaration.reflection_fields else {
                return Err("unsupported Body IR field-count metadata without checked visibility".into());
            };
            (literal(Constant::Int(fields.len() as i64), span), PlanType::Int)
        }
    };
    let Some(place) = destination else {
        return Err("unsupported Body IR discarded type metadata".into());
    };
    let output = scalar_place(builder, place, span)?;
    if place_type(&builder.locals, &output) != result {
        return Err("invalid Body IR type metadata result carrier".into());
    }
    let text = result == PlanType::String;
    if text {
        drop_owned(builder, output.local, span);
    }
    track_rvalue_moves(builder, &RvalueKind::Use(value.clone()));
    let local = output.local;
    builder.emit(plan_assign(output, RvalueKind::Use(value), span));
    if text {
        builder.owned.push(local);
    }
    Ok(())
}
