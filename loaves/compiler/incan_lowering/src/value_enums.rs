//! Identity-checked value-enum construction and generated scalar getter admission.

use incan_mir_plan_native::{PlanType, SourceSpan};
use incan_semantics_core::body_ir::{
    ArgumentElement, BodyIrModule, EnumDeclaration, MethodTarget, NamedCallableTarget, Operand as SourceOperand,
    ValueEnumDeclaration, ValueEnumVariantTarget,
};
use incan_semantics_core::{SemanticSourceTargetKind, SymbolNamespace};

use crate::frame::{Builder, place_type};
use crate::models::{PlaceTables, model_place};

/// Require both the canonical raw-value member and its identical native unit layout.
pub fn value_variant(
    layouts: &[EnumDeclaration],
    declarations: &[ValueEnumDeclaration],
    target: &ValueEnumVariantTarget,
) -> Result<(usize, usize), String> {
    for declaration in declarations {
        if declaration.canonical != target.enum_canonical
            || declaration.direct_declaration_id != target.enum_declaration_id
        {
            continue;
        }
        if declaration.name != target.enum_name {
            return Err("invalid Body IR value enum owner spelling".into());
        }
        for (slot, variant) in declaration.variants.iter().enumerate() {
            if variant.canonical != target.variant_canonical
                || variant.direct_declaration_id != target.variant_declaration_id
            {
                continue;
            }
            if variant.name != target.variant_name {
                return Err("invalid Body IR value enum member spelling".into());
            }
            for (owner, layout) in layouts.iter().enumerate() {
                if layout.canonical == declaration.canonical {
                    validate_layout(layout, declaration)?;
                    return Ok((owner, slot));
                }
            }
        }
    }
    Err("unsupported Body IR unretained value enum variant".into())
}

/// Cross-check source order and unit carriers before using native tags to select raw values.
pub fn validate_layout(layout: &EnumDeclaration, declaration: &ValueEnumDeclaration) -> Result<(), String> {
    if layout.canonical != declaration.canonical
        || layout.direct_declaration_id != declaration.direct_declaration_id
        || layout.canonical.declaration_name != declaration.name
        || layout.variants.len() != declaration.variants.len()
    {
        return Err("invalid Body IR value enum native layout".into());
    }
    for (native, raw) in layout.variants.iter().zip(&declaration.variants) {
        if native.canonical != raw.canonical
            || native.direct_declaration_id != raw.direct_declaration_id
            || native.name != raw.name
            || !native.fields.is_empty()
        {
            return Err("invalid Body IR value enum native member layout".into());
        }
    }
    Ok(())
}

/// Admit the getter under its owner's span; declared methods and authenticated SDK trait calls retain their own paths.
pub fn value_method_owner(
    module: &BodyIrModule,
    builder: &Builder,
    target: &MethodTarget,
    argument: &ArgumentElement,
    span: &SourceSpan,
) -> Result<Option<usize>, String> {
    if target.loaf_trait.is_some() {
        return Ok(None);
    }
    let index = receiver_layout(module, builder, argument, span)?;
    value_getter_owner(module, target, index)
}

/// The value enum whose generated `value()` getter `target` names, for a receiver whose enum layout is `index`.
pub fn value_getter_owner(module: &BodyIrModule, target: &MethodTarget, index: i64) -> Result<Option<usize>, String> {
    if target.loaf_trait.is_some() || index < 0 || index as usize >= module.enum_declarations.len() {
        return Ok(None);
    }
    let layout = &module.enum_declarations[index as usize];
    for (owner, declaration) in module.value_enum_declarations.iter().enumerate() {
        if layout.canonical != declaration.canonical {
            continue;
        }
        validate_layout(layout, declaration)?;
        if let Some(identity) = &target.canonical
            && identity.declaration_span != declaration.canonical.declaration_span
        {
            return Ok(None);
        }
        validate_getter(declaration, target)?;
        return Ok(Some(owner));
    }
    Ok(None)
}

/// Admit the checker-generated `from_value()` member only for its exact value enum owner.
pub fn value_enum_from_value_owner(
    module: &BodyIrModule,
    target: &NamedCallableTarget,
) -> Result<Option<usize>, String> {
    let Some(identity) = &target.canonical else {
        return Ok(None);
    };
    if identity.declaration_name != "from_value"
        || identity.namespace != SymbolNamespace::Member
        || identity.kind != SemanticSourceTargetKind::Method
    {
        return Ok(None);
    }
    for (owner, declaration) in module.value_enum_declarations.iter().enumerate() {
        let declaration = &declaration.canonical;
        if identity.origin == declaration.origin
            && identity.scope_discriminant == declaration.scope_discriminant
            && identity.declaration_span == declaration.declaration_span
        {
            if !target.type_args.is_empty() {
                return Err("unsupported Body IR generic value enum from_value".into());
            }
            return Ok(Some(owner));
        }
    }
    Ok(None)
}

/// Admit the checker-generated `message()` member only for its exact source enum owner, value enums included.
///
/// The generated member carries its owner's origin, scope and declaration span; a method an enum declares itself has
/// its own span and keeps the ordinary method path.
pub fn enum_message_layout(
    module: &BodyIrModule,
    builder: &Builder,
    target: &MethodTarget,
    argument: &ArgumentElement,
    span: &SourceSpan,
) -> Result<Option<usize>, String> {
    let Some(identity) = &target.canonical else {
        return Ok(None);
    };
    if identity.declaration_name != "message"
        || identity.namespace != SymbolNamespace::Member
        || identity.kind != SemanticSourceTargetKind::Method
    {
        return Ok(None);
    }
    let index = receiver_layout(module, builder, argument, span)?;
    if index < 0 || index as usize >= module.enum_declarations.len() {
        return Ok(None);
    }
    let owner = &module.enum_declarations[index as usize].canonical;
    if identity.origin != owner.origin
        || identity.scope_discriminant != owner.scope_discriminant
        || identity.declaration_span != owner.declaration_span
    {
        return Ok(None);
    }
    if !target.type_args.is_empty() {
        return Err("unsupported Body IR generic enum message".into());
    }
    Ok(Some(index as usize))
}

/// Read the enum carrier from the receiver place; other carriers keep their ordinary method path.
///
/// The frame's slots start at its offset in the shared locals table, which is borrowed rather than copied per call.
fn receiver_layout(
    module: &BodyIrModule,
    builder: &Builder,
    argument: &ArgumentElement,
    span: &SourceSpan,
) -> Result<i64, String> {
    if let Some(SourceOperand::Place(read)) = argument.as_one() {
        let tables = PlaceTables {
            constructors: builder.constructors(),
            declarations: &module.nominal_declarations,
            enums: &module.enum_declarations,
            unions: &builder.unions,
            intrinsics: &builder.intrinsics,
        };
        let mut source = model_place(tables, &builder.locals, &read.place, span, builder.local_offset)?;
        source.local += builder.local_offset;
        if let PlanType::Enum(index, _) = place_type(&builder.locals, &source) {
            return Ok(index);
        }
    }
    Ok(-1)
}

/// Check the synthetic member's complete owner identity rather than trusting the call-site spelling.
fn validate_getter(declaration: &ValueEnumDeclaration, target: &MethodTarget) -> Result<(), String> {
    let Some(identity) = &target.canonical else {
        return Err("unsupported Body IR unproven value enum method".into());
    };
    if identity.namespace != SymbolNamespace::Member
        || identity.kind != SemanticSourceTargetKind::Method
        || identity.declaration_name != "value"
        || identity.origin != declaration.canonical.origin
        || identity.scope_discriminant != declaration.canonical.scope_discriminant
        || identity.declaration_span != declaration.canonical.declaration_span
    {
        return Err("unsupported Body IR value enum method target".into());
    }
    if !target.type_args.is_empty() {
        return Err("unsupported Body IR generic value enum method".into());
    }
    Ok(())
}
