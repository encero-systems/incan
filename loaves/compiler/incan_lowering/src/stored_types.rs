//! Canonical stored-type selectors from checked types and their retained producer declarations.

use incan_semantics_core::body_ir::{BodyIrModule, NominalDeclaration, nominal_carrier_key};
use incan_semantics_core::record_identity::RecordPackageIdentity;
use incan_semantics_core::{
    CanonicalSymbolId, IncanPrimitiveType, IncanType, NumericTypeId, SemanticSourceTargetKind, SymbolOrigin,
};
use sha2::{Digest, Sha256};

use crate::native_type_identity::{ConstructorIndex, named_positions};
use crate::nominal_instances::nominal_symbol;
use crate::type_parameters::substitute;

/// Spell a stored type without reading diagnostic type names or expanding a nominal's fields.
pub fn stored_type(module: &BodyIrModule, constructors: &ConstructorIndex, ty: &IncanType) -> Option<String> {
    type_selector(module, constructors, ty, &[])
}

fn type_selector(
    module: &BodyIrModule,
    constructors: &ConstructorIndex,
    ty: &IncanType,
    unwrapping: &[String],
) -> Option<String> {
    match ty {
        IncanType::Primitive(primitive) => primitive_selector(primitive),
        IncanType::Decimal { precision, scale } => Some(format!("decimal<{precision},{scale}>")),
        IncanType::Tuple(elements) => composite_selector(module, constructors, "tuple", elements, unwrapping),
        IncanType::Named(name) => nominal_selector(module, constructors, name, &[], unwrapping),
        IncanType::Generic { base, args } => {
            if ty.is_union() {
                return composite_selector(module, constructors, "union", args, unwrapping);
            }
            let kind = match base.as_str() {
                "List" | "FrozenList" | "$FrozenList" => "list",
                "Set" | "FrozenSet" | "$FrozenSet" => "set",
                "Dict" | "FrozenDict" | "$FrozenDict" => "map",
                "Tuple" => "tuple",
                "Option" => "option",
                _ => return nominal_selector(module, constructors, base, args, unwrapping),
            };
            composite_selector(module, constructors, kind, args, unwrapping)
        }
        _ => None,
    }
}

/// Map the checked builtin vocabulary, refusing target-dependent integer widths.
fn primitive_selector(primitive: &IncanPrimitiveType) -> Option<String> {
    let name = match primitive {
        IncanPrimitiveType::Bool => "boolean",
        IncanPrimitiveType::Int => "i64",
        IncanPrimitiveType::Float => "fp64",
        IncanPrimitiveType::Str | IncanPrimitiveType::FrozenStr => "string",
        IncanPrimitiveType::Bytes | IncanPrimitiveType::FrozenBytes => "binary",
        IncanPrimitiveType::Unit => "tuple<>",
        IncanPrimitiveType::Numeric(kind) => match kind {
            NumericTypeId::Bool => "boolean",
            NumericTypeId::I8 => "i8",
            NumericTypeId::I16 => "i16",
            NumericTypeId::I32 => "i32",
            NumericTypeId::I64 => "i64",
            NumericTypeId::I128 => "i128",
            NumericTypeId::U8 => "u8",
            NumericTypeId::U16 => "u16",
            NumericTypeId::U32 => "u32",
            NumericTypeId::U64 => "u64",
            NumericTypeId::U128 => "u128",
            NumericTypeId::F32 => "fp32",
            NumericTypeId::F64 => "fp64",
            NumericTypeId::ISize | NumericTypeId::USize => return None,
        },
    };
    Some(name.to_owned())
}

fn selectors(
    module: &BodyIrModule,
    constructors: &ConstructorIndex,
    arguments: &[IncanType],
    unwrapping: &[String],
) -> Option<Vec<String>> {
    arguments
        .iter()
        .map(|argument| type_selector(module, constructors, argument, unwrapping))
        .collect()
}

/// Apply the canonical container grammar; unions sort by spelling digest, then spelling on a collision.
fn composite_selector(
    module: &BodyIrModule,
    constructors: &ConstructorIndex,
    kind: &str,
    arguments: &[IncanType],
    unwrapping: &[String],
) -> Option<String> {
    let names = selectors(module, constructors, arguments, unwrapping)?;
    if kind == "union" {
        let mut keyed: Vec<(String, String)> = names
            .into_iter()
            .map(|name| (hex::encode(Sha256::digest(name.as_bytes())), name))
            .collect();
        keyed.sort();
        let ordered: Vec<String> = keyed.into_iter().map(|(_, name)| name).collect();
        return Some(format!("union<{}>", ordered.join(",")));
    }
    Some(format!("{kind}<{}>", names.join(",")))
}

fn selected_carrier(base: &str, arguments: &[IncanType]) -> String {
    if arguments.is_empty() {
        return base.to_owned();
    }
    nominal_symbol(base, arguments)
}

/// Select a canonical retained carrier, never an equal display name or an import binding.
fn nominal_selector(
    module: &BodyIrModule,
    constructors: &ConstructorIndex,
    base: &str,
    arguments: &[IncanType],
    unwrapping: &[String],
) -> Option<String> {
    let carrier = selected_carrier(base, arguments);
    if let Some(position) = constructors.owners.get(&carrier) {
        let declaration = &module.nominal_declarations[*position];
        if declaration.carrier_name() == carrier {
            return retained_nominal_selector(module, constructors, declaration, unwrapping);
        }
    }
    for position in named_positions(constructors, &carrier) {
        let declaration = &module.nominal_declarations[position];
        if declaration.carrier_name() == carrier {
            return retained_nominal_selector(module, constructors, declaration, unwrapping);
        }
    }
    // A checked use may refer to a published generic template whose native carrier stays provider-owned.
    if let Some(position) = constructors.generic_owners.get(base) {
        let declaration = &module.nominal_declarations[*position];
        if declaration.type_parameters.len() != arguments.len() {
            return None;
        }
        if declaration.canonical.kind == SemanticSourceTargetKind::Newtype {
            if unwrapping.contains(&carrier) || declaration.field_types.len() != 1 {
                return None;
            }
            let inner = substitute(&declaration.field_types[0], &declaration.type_parameters, arguments).ok()?;
            let mut nested = unwrapping.to_vec();
            nested.push(carrier);
            return type_selector(module, constructors, &inner, &nested);
        }
        if declaration.canonical.kind != SemanticSourceTargetKind::Model {
            return None;
        }
        return declared_selector(
            module,
            constructors,
            "record",
            &declaration.canonical,
            &declaration.record_package_identity,
            arguments,
            unwrapping,
        );
    }
    for declaration in &module.enum_declarations {
        if declaration.carrier_name() == carrier {
            return declared_selector(
                module,
                constructors,
                "enum",
                &declaration.canonical,
                &declaration.record_package_identity,
                &declaration.native_type_arguments,
                unwrapping,
            );
        }
        if nominal_carrier_key(&declaration.canonical).as_deref() == Some(base)
            && declaration.type_parameters.len() == arguments.len()
        {
            return declared_selector(
                module,
                constructors,
                "enum",
                &declaration.canonical,
                &declaration.record_package_identity,
                arguments,
                unwrapping,
            );
        }
    }
    None
}

/// Use a retained closed model or transparent newtype; an open template has no closed selector.
fn retained_nominal_selector(
    module: &BodyIrModule,
    constructors: &ConstructorIndex,
    declaration: &NominalDeclaration,
    unwrapping: &[String],
) -> Option<String> {
    if declaration.type_parameters.len() != declaration.native_type_arguments.len() {
        return None;
    }
    let carrier = declaration.carrier_name();
    if declaration.canonical.kind == SemanticSourceTargetKind::Newtype {
        if unwrapping.contains(&carrier) || declaration.field_types.len() != 1 {
            return None;
        }
        let mut nested = unwrapping.to_vec();
        nested.push(carrier);
        return type_selector(module, constructors, &declaration.field_types[0], &nested);
    }
    if declaration.canonical.kind != SemanticSourceTargetKind::Model {
        return None;
    }
    declared_selector(
        module,
        constructors,
        "record",
        &declaration.canonical,
        &declaration.record_package_identity,
        &declaration.native_type_arguments,
        unwrapping,
    )
}

/// Hash only the producer namespace and canonical declaration path, then append closed type selectors.
fn declared_selector(
    module: &BodyIrModule,
    constructors: &ConstructorIndex,
    kind: &str,
    identity: &CanonicalSymbolId,
    package: &Option<RecordPackageIdentity>,
    arguments: &[IncanType],
    unwrapping: &[String],
) -> Option<String> {
    let namespace = package.as_ref()?;
    let mut path: Vec<String> = match &identity.origin {
        SymbolOrigin::Module(parts) => parts.clone(),
        SymbolOrigin::Package { module_path, .. } => module_path.clone(),
        _ => return None,
    };
    path.push(identity.declaration_name.clone());
    let mut hasher = Sha256::new();
    for component in [
        namespace.registry_id.clone(),
        namespace.scoped_name.clone(),
        path.join("."),
    ] {
        hasher.update(component.as_bytes());
        hasher.update(b"\x00");
    }
    let digest = hex::encode(hasher.finalize());
    let selector = format!("{kind}@{}", &digest[..12]);
    let names = selectors(module, constructors, arguments, unwrapping)?;
    if names.is_empty() {
        return Some(selector);
    }
    Some(format!("{selector}<{}>", names.join(",")))
}
