//! Canonical normal-enum layouts and identity-checked native construction and pattern admission.

use incan_mir_plan_native::{EnumDeclaration as PlanEnumDeclaration, EnumVariant, PlanType, source_span};
use incan_semantics_core::IncanType;
use incan_semantics_core::body_ir::{
    ArgumentBinding, BodyIrModule, Constant, EnumDeclaration as SourceEnumDeclaration, EnumVariantTarget,
    FieldlessEnumVariantTarget, OwnershipFact, Pattern, ValueEnumBacking,
};

use crate::decls::{ViewLifetimeLayout, library_enum_item, serde_propagation, with_serde_derives};
use crate::layout_types::native_source_type;
use crate::native_type_identity::ConstructorIndex;

use crate::nominal_instances::nominal_symbol;
use crate::symbols::model_symbol;

/// A retained source enum, recognized before the model and scalar resolver runs.
pub fn enum_type(declarations: &[SourceEnumDeclaration], ty: &IncanType) -> Option<PlanType> {
    let name = match ty {
        IncanType::Named(name) => name.clone(),
        IncanType::Generic { base, args } => nominal_symbol(base, args),
        _ => return None,
    };
    declarations
        .iter()
        .position(|declaration| declaration.carrier_name() == name)
        .map(|index| PlanType::Enum(index as i64, model_symbol(&name)))
}

/// Copy checked source-order variants and derives into the native declaration plan.
///
/// A source enum also takes the serde derives a serde-deriving declaration's fields require of it. Value enums retain
/// the same checked raw values for explicit and propagated serde selections. Source Display uses its checked Debug
/// path rather than a native Rust derive. Native enums retain only the checked Copy fact verified on their SDK type.
pub fn lower_enum_declarations(
    constructors: &ConstructorIndex,
    module: &BodyIrModule,
    file: &str,
    intrinsics: &[PlanEnumDeclaration],
) -> Result<Vec<PlanEnumDeclaration>, String> {
    let reached = serde_propagation(module)?;
    let nominals = &module.nominal_declarations;
    let layouts = &module.enum_declarations;
    let mut declarations = Vec::with_capacity(layouts.len());
    let mut view_lifetimes = ViewLifetimeLayout::start(nominals, layouts);
    for declaration in layouts {
        let mut variants = Vec::with_capacity(declaration.variants.len());
        for variant in &declaration.variants {
            let mut fields = Vec::with_capacity(variant.fields.len());
            for ty in &variant.fields {
                fields.push(native_source_type(
                    constructors,
                    nominals,
                    layouts,
                    &[],
                    ty,
                    intrinsics,
                    "",
                )?);
            }
            variants.push(EnumVariant {
                name: variant.name.clone(),
                fields,
            });
        }
        // A stdlib enum is the SDK crate's own type; its derives and storage belong to that crate.
        let item = library_enum_item(declaration).to_owned();
        let mut native_arguments = Vec::new();
        if !item.is_empty() {
            for argument in &declaration.native_type_arguments {
                native_arguments.push(native_source_type(
                    constructors,
                    nominals,
                    layouts,
                    &[],
                    argument,
                    intrinsics,
                    "",
                )?);
            }
        }
        let selected = with_serde_derives(declaration.derives.clone(), &declaration.name, &reached);
        let mut derives: Vec<String> = Vec::new();
        if item.is_empty() {
            derives = selected.into_iter().filter(|derive| derive != "Display").collect();
        } else if declaration.derives.iter().any(|derive| derive == "Copy") {
            derives.push("Copy".into());
        }
        let (raw_kind, raw_values) = value_enum_raw_values(module, declaration)?;
        let mut view_lifetime_count = 0;
        if item.is_empty() {
            view_lifetime_count = view_lifetimes.enum_declaration(declaration, nominals, layouts);
        }
        let mut planned = PlanEnumDeclaration::new(
            model_symbol(&declaration.carrier_name()),
            declaration.public || !item.is_empty(),
            variants,
            derives,
            source_span(file, 1, 1),
        );
        planned.view_lifetime_count = view_lifetime_count;
        planned.library_item = item;
        planned.library_type_arguments = native_arguments;
        planned.raw_kind = raw_kind;
        planned.raw_values = raw_values;
        declarations.push(planned);
    }
    Ok(declarations)
}

/// A source value enum's backing and raw values in variant order; an SDK enum keeps its serialization in its crate.
fn value_enum_raw_values(
    module: &BodyIrModule,
    declaration: &SourceEnumDeclaration,
) -> Result<(String, Vec<String>), String> {
    if !library_enum_item(declaration).is_empty() {
        return Ok((String::new(), Vec::new()));
    }
    for value_enum in &module.value_enum_declarations {
        if value_enum.canonical != declaration.canonical {
            continue;
        }
        let mut raw_values = Vec::with_capacity(value_enum.variants.len());
        for variant in &value_enum.variants {
            match &variant.raw_value {
                Constant::Str(text) => raw_values.push(text.clone()),
                Constant::Int(number) => raw_values.push(number.to_string()),
                _ => return Err("invalid Body IR value enum raw value".into()),
            }
        }
        return Ok(match value_enum.backing {
            ValueEnumBacking::Str => ("str".into(), raw_values),
            ValueEnumBacking::Int => ("int".into(), raw_values),
        });
    }
    Ok((String::new(), Vec::new()))
}

/// Require an enum-plan scrutinee; scalar, Option and Result matches stay named refusals.
pub fn enum_owner(ty: &PlanType) -> Result<i64, String> {
    match ty {
        PlanType::Enum(index, _) => Ok(*index),
        _ => Err("unsupported Body IR Match outside source enums".into()),
    }
}

/// Resolve construction by exact variant and owner identities; written owner aliases remain diagnostic spelling.
pub fn variant_index(
    declarations: &[SourceEnumDeclaration],
    target: &EnumVariantTarget,
    expected: &PlanType,
) -> Result<(usize, usize), String> {
    for (owner, declaration) in declarations.iter().enumerate() {
        match expected {
            PlanType::Enum(selected, _) => {
                if owner as i64 != *selected {
                    continue;
                }
            }
            _ => return Err("unsupported Body IR enum constructor destination".into()),
        }
        for (slot, variant) in declaration.variants.iter().enumerate() {
            if variant.canonical != target.variant_canonical {
                continue;
            }
            if target.enum_canonical.as_ref() != Some(&declaration.canonical) || target.variant_name != variant.name {
                return Err("invalid Body IR enum constructor identity".into());
            }
            validate_binding(&target.binding, variant.fields.len())?;
            return Ok((owner, slot));
        }
    }
    Err("unsupported Body IR unretained enum constructor".into())
}

/// Resolve the older fieldless constructor vocabulary against the same canonical layout.
pub fn unit_variant(
    declarations: &[SourceEnumDeclaration],
    target: &FieldlessEnumVariantTarget,
) -> Result<(usize, usize), String> {
    for (owner, declaration) in declarations.iter().enumerate() {
        if declaration.canonical != target.enum_canonical
            || declaration.direct_declaration_id != target.enum_declaration_id
        {
            continue;
        }
        for (slot, variant) in declaration.variants.iter().enumerate() {
            if variant.canonical == target.variant_canonical
                && variant.direct_declaration_id == target.variant_declaration_id
                && variant.fields.is_empty()
            {
                if declaration.canonical.declaration_name != target.enum_name || variant.name != target.variant_name {
                    return Err("invalid Body IR fieldless enum spelling".into());
                }
                return Ok((owner, slot));
            }
        }
    }
    Err("unsupported Body IR unretained fieldless enum variant".into())
}

/// Require complete positional payload binding; spreads and omitted payload slots remain refused.
pub fn validate_binding(binding: &ArgumentBinding, count: usize) -> Result<(), String> {
    let ArgumentBinding::Resolved {
        arguments,
        defaulted_slots,
    } = binding
    else {
        return Err("unsupported Body IR unresolved enum payload binding".into());
    };
    if !defaulted_slots.is_empty() || arguments.len() != count {
        return Err("unsupported Body IR incomplete enum payload binding".into());
    }
    for (index, argument) in arguments.iter().enumerate() {
        if argument.slot != index {
            return Err("unsupported Body IR reordered enum payload binding".into());
        }
    }
    Ok(())
}

/// Resolve exact variant identities while allowing checked owner aliases; payload layout and ownership stay explicit.
pub fn pattern_variant(declaration: &SourceEnumDeclaration, pattern: &Pattern) -> Result<i64, String> {
    match pattern {
        Pattern::Wildcard => return Ok(-1),
        Pattern::Enum {
            canonical,
            variant: written,
            fields,
            ..
        } => {
            for (slot, variant) in declaration.variants.iter().enumerate() {
                if canonical.as_ref() == Some(&variant.canonical) {
                    // The exact variant identity above proves the owner; reexports can rename its written path. Retain
                    // the member-spelling check without treating the source owner alias as a carrier name.
                    let member = written.split("::").last().unwrap_or_default();
                    if member != variant.name || fields.len() != variant.fields.len() {
                        return Err("invalid Body IR enum pattern layout".into());
                    }
                    validate_payload_patterns(fields)?;
                    return Ok(slot as i64);
                }
            }
        }
        Pattern::FieldlessEnumVariant(target) => {
            if target.enum_canonical != declaration.canonical
                || target.enum_declaration_id != declaration.direct_declaration_id
            {
                return Err("invalid Body IR fieldless enum pattern owner".into());
            }
            for (slot, variant) in declaration.variants.iter().enumerate() {
                if target.variant_canonical == variant.canonical
                    && target.variant_declaration_id == variant.direct_declaration_id
                    && variant.fields.is_empty()
                {
                    return Ok(slot as i64);
                }
            }
        }
        _ => {}
    }
    Err("unsupported Body IR enum pattern".into())
}

/// Admit payload patterns whose tests and bindings are represented: bindings with a known ownership, literals, nested
/// carrier and tuple patterns, and alternations of those; anything else stays refused by name.
pub fn validate_payload_patterns(patterns: &[Pattern]) -> Result<(), String> {
    for pattern in patterns {
        validate_payload_pattern(pattern)?;
    }
    Ok(())
}

/// Admit one payload pattern, recursing into the payloads a nested pattern names.
fn validate_payload_pattern(pattern: &Pattern) -> Result<(), String> {
    match pattern {
        Pattern::Wildcard | Pattern::Literal(_) | Pattern::FieldlessEnumVariant(_) => Ok(()),
        // A moved payload binding reads like a cloned one: legacy matches a cloned enum by value. A payload its arm
        // changes in place is borrowed exclusively from the scrutinee instead.
        Pattern::Var(binding) => match binding.fact {
            OwnershipFact::Copy
            | OwnershipFact::Clone
            | OwnershipFact::Borrow
            | OwnershipFact::Move
            | OwnershipFact::MutBorrow => Ok(()),
            _ => Err("unsupported Body IR enum payload binding ownership".into()),
        },
        Pattern::Enum { fields, .. }
        | Pattern::Result { fields, .. }
        | Pattern::UnionMember { fields, .. }
        | Pattern::Tuple(fields)
        | Pattern::Or(fields) => validate_payload_patterns(fields),
        Pattern::Nominal { fields, .. } => {
            for (_, field_pattern) in fields {
                validate_payload_pattern(field_pattern)?;
            }
            Ok(())
        }
        _ => Err("unsupported Body IR nested enum payload pattern".into()),
    }
}
