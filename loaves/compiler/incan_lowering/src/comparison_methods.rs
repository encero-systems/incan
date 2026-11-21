//! Native comparison bridge symbols selected exclusively from checked standard trait implementations.

use incan_mir_plan_native::{EnumDeclaration, PlanType};
use incan_semantics_core::body_ir::{Body, BodyIrModule, CallableParamDefault, MethodTarget};
use incan_semantics_core::{
    CanonicalSymbolId, IncanPrimitiveType, IncanType, SemanticSourceTargetKind, SymbolNamespace, SymbolOrigin,
};

use crate::classes::{frame_type, function_name, parameter_local_index};
use crate::decls::{library_enum_item, library_item};
use crate::native_type_identity::ConstructorIndex;

const ORDERING: [&str; 4] = ["__lt__", "__le__", "__gt__", "__ge__"];

/// Require the checked member identity of the standard comparison protocol, in source or SDK form.
pub fn standard_comparison_slot(identity: &CanonicalSymbolId, name: &str) -> bool {
    if identity.kind != SemanticSourceTargetKind::Method
        || identity.namespace != SymbolNamespace::Member
        || identity.declaration_name != name
    {
        return false;
    }
    match &identity.origin {
        SymbolOrigin::Module(path) => {
            path.len() == 3 && path[0] == "std" && path[1] == "derives" && path[2] == "comparison"
        }
        SymbolOrigin::Package { library, module_path } => {
            library == "incan_stdlib_primitives"
                && module_path.len() == 2
                && module_path[0] == "derives"
                && module_path[1] == "comparison"
        }
        _ => false,
    }
}

/// One borrowed equality body, and all four owned ordering bodies when a checked source Ord slot is supplied.
pub fn comparison_functions(
    constructors: &ConstructorIndex,
    module: &BodyIrModule,
    intrinsics: &[EnumDeclaration],
) -> Result<Vec<Vec<String>>, String> {
    let mut owners = Vec::with_capacity(module.nominal_declarations.len());
    for (owner, declaration) in module.nominal_declarations.iter().enumerate() {
        if !library_item(declaration).is_empty() {
            owners.push(Vec::new());
            continue;
        }
        let mut symbols = Vec::new();
        if declaration.comparison_equality.is_some() {
            symbols.push(comparison_body(
                constructors,
                module,
                owner,
                "__eq__",
                true,
                intrinsics,
            )?);
            if source_slot(module, owner, "__lt__") {
                for name in ORDERING {
                    symbols.push(comparison_body(constructors, module, owner, name, false, intrinsics)?);
                }
            }
        }
        owners.push(symbols);
    }
    Ok(owners)
}

/// A retained source body filling this owner's canonical standard slot authorizes a bridge.
fn source_slot(module: &BodyIrModule, owner: usize, name: &str) -> bool {
    let identity = &module.nominal_declarations[owner].canonical;
    for implementation in &module.trait_implementations {
        if implementation.owner == *identity && standard_comparison_slot(&implementation.method, name) {
            return implementation.implementation != implementation.method;
        }
    }
    false
}

/// Resolve a selected non-generic implementation with its exact shared receiver and legacy other-operand ABI.
fn comparison_body(
    constructors: &ConstructorIndex,
    module: &BodyIrModule,
    owner: usize,
    name: &str,
    borrowed: bool,
    intrinsics: &[EnumDeclaration],
) -> Result<String, String> {
    let nominals = &module.nominal_declarations;
    let enums = &module.enum_declarations;
    let declaration = &nominals[owner];
    let mut selected: Option<CanonicalSymbolId> = None;
    for implementation in &module.trait_implementations {
        if implementation.owner != declaration.canonical || !standard_comparison_slot(&implementation.method, name) {
            continue;
        }
        match &selected {
            Some(previous) => {
                if *previous != implementation.implementation {
                    return Err("unsupported Body IR ambiguous native comparison implementation".into());
                }
            }
            None => selected = Some(implementation.implementation.clone()),
        }
    }
    let Some(identity) = selected else {
        return Err(format!("unsupported Body IR unavailable native comparison slot {name}"));
    };
    if name == "__eq__" && nominals[owner].comparison_equality.as_ref() != Some(&identity) {
        return Err("unsupported Body IR mismatched native equality witness".into());
    }
    for body in &module.bodies {
        if body.canonical.as_ref() != Some(&identity) || body.params.len() != 2 {
            continue;
        }
        let first = &body.locals[parameter_local_index(body, 0)];
        if let Ok(receiver) = frame_type(constructors, nominals, enums, first, intrinsics, "")
            && receiver == PlanType::ModelRef(owner as i64, declaration.name.clone())
        {
            validate_comparison_body(constructors, module, body, owner, borrowed, intrinsics)?;
            return Ok(function_name(module, body));
        }
    }
    Err(format!(
        "unsupported Body IR unavailable published native comparison body {name}"
    ))
}

/// Admit only concrete bool functions with the validated source owner's two native input carriers.
fn validate_comparison_body(
    constructors: &ConstructorIndex,
    module: &BodyIrModule,
    body: &Body,
    owner: usize,
    borrowed: bool,
    intrinsics: &[EnumDeclaration],
) -> Result<(), String> {
    let nominals = &module.nominal_declarations;
    let enums = &module.enum_declarations;
    if !body.type_parameters.is_empty() || body.params.len() != 2 {
        return Err("unsupported Body IR generic or nonbinary native comparison body".into());
    }
    let name = nominals[owner].name.clone();
    let reference = PlanType::ModelRef(owner as i64, name.clone());
    let first = &body.locals[parameter_local_index(body, 0)];
    let second = &body.locals[parameter_local_index(body, 1)];
    let mut expected = PlanType::Model(owner as i64, name);
    if borrowed {
        expected = reference.clone();
        if !matches!(body.params[1].default, CallableParamDefault::Required) {
            return Err("unsupported Body IR native equality with a defaulted other operand".into());
        }
    }
    if frame_type(constructors, nominals, enums, first, intrinsics, "")? != reference
        || frame_type(constructors, nominals, enums, second, intrinsics, "")? != expected
    {
        return Err("unsupported Body IR native comparison input ABI".into());
    }
    if body.return_type != IncanType::Primitive(IncanPrimitiveType::Bool) {
        return Err("unsupported Body IR native comparison result ABI".into());
    }
    Ok(())
}

/// Keep owned source operands for exact own equality calls or a checked enum slot dispatching to that body.
pub fn source_equality_value(module: &BodyIrModule, target: &MethodTarget, body: &Body) -> Option<PlanType> {
    body.canonical.as_ref()?;
    for (owner, declaration) in module.nominal_declarations.iter().enumerate() {
        if target.canonical == body.canonical && declaration.comparison_equality == body.canonical {
            return Some(PlanType::Model(owner as i64, declaration.name.clone()));
        }
    }
    for (owner, declaration) in module.enum_declarations.iter().enumerate() {
        for implementation in &module.trait_implementations {
            if implementation.owner == declaration.canonical
                && standard_comparison_slot(&implementation.method, "__eq__")
                && body.canonical.as_ref() == Some(&implementation.implementation)
                && (target.canonical == body.canonical || target.canonical.as_ref() == Some(&implementation.method))
                && body.params.len() == 2
                && body.params[1].ty == IncanType::Ref(Box::new(IncanType::Named(declaration.name.clone())))
            {
                return Some(PlanType::Enum(owner as i64, declaration.name.clone()));
            }
        }
    }
    None
}

/// Select enum bridges from checked standard slots and their retained borrowed equality ABI, never variant order.
pub fn enum_comparison_functions(
    constructors: &ConstructorIndex,
    module: &BodyIrModule,
    intrinsics: &[EnumDeclaration],
) -> Result<Vec<Vec<String>>, String> {
    let mut owners = Vec::with_capacity(module.enum_declarations.len());
    for (owner, declaration) in module.enum_declarations.iter().enumerate() {
        let mut symbols = Vec::new();
        if library_enum_item(declaration).is_empty() && declaration.type_parameters.is_empty() {
            for implementation in &module.trait_implementations {
                if implementation.owner != declaration.canonical
                    || !standard_comparison_slot(&implementation.method, "__eq__")
                {
                    continue;
                }
                if implementation.implementation == implementation.method {
                    continue;
                }
                symbols.push(enum_comparison_body(
                    constructors,
                    module,
                    owner,
                    "__eq__",
                    true,
                    intrinsics,
                )?);
                let ordered = module.trait_implementations.iter().any(|slot| {
                    slot.owner == declaration.canonical
                        && standard_comparison_slot(&slot.method, "__lt__")
                        && slot.implementation != slot.method
                });
                if ordered {
                    for name in ORDERING {
                        symbols.push(enum_comparison_body(
                            constructors,
                            module,
                            owner,
                            name,
                            false,
                            intrinsics,
                        )?);
                    }
                }
                break;
            }
        }
        owners.push(symbols);
    }
    Ok(owners)
}

/// Attach checked source bridges and remove only native comparison derives replaced by those exact bodies.
pub fn attach_enum_comparisons(
    constructors: &ConstructorIndex,
    module: &BodyIrModule,
    declarations: &mut [EnumDeclaration],
    intrinsics: &[EnumDeclaration],
) -> Result<(), String> {
    let selected = enum_comparison_functions(constructors, module, intrinsics)?;
    for owner in 0..module.enum_declarations.len() {
        declarations[owner].comparison_methods = selected[owner].clone();
        if !selected[owner].is_empty() {
            declarations[owner]
                .derives
                .retain(|derive| !matches!(derive.as_str(), "PartialEq" | "Eq" | "PartialOrd" | "Ord"));
        }
    }
    Ok(())
}

/// Require the unique checked enum slot body and its exact two-input bool ABI before planning a native bridge.
fn enum_comparison_body(
    constructors: &ConstructorIndex,
    module: &BodyIrModule,
    owner: usize,
    name: &str,
    borrowed: bool,
    intrinsics: &[EnumDeclaration],
) -> Result<String, String> {
    let nominals = &module.nominal_declarations;
    let enums = &module.enum_declarations;
    let declaration = &enums[owner];
    let mut selected: Option<CanonicalSymbolId> = None;
    for implementation in &module.trait_implementations {
        if implementation.owner != declaration.canonical || !standard_comparison_slot(&implementation.method, name) {
            continue;
        }
        match &selected {
            Some(previous) => {
                if *previous != implementation.implementation {
                    return Err("unsupported Body IR ambiguous enum comparison implementation".into());
                }
            }
            None => selected = Some(implementation.implementation.clone()),
        }
    }
    let Some(identity) = selected else {
        return Err(format!("unsupported Body IR unavailable enum comparison slot {name}"));
    };
    for body in &module.bodies {
        if body.canonical.as_ref() != Some(&identity) || body.params.len() != 2 {
            continue;
        }
        if !body.type_parameters.is_empty() || body.return_type != IncanType::Primitive(IncanPrimitiveType::Bool) {
            return Err("unsupported Body IR generic or nonboolean enum comparison body".into());
        }
        let reference = PlanType::EnumRef(owner as i64, declaration.name.clone());
        let other = if borrowed {
            reference.clone()
        } else {
            PlanType::Enum(owner as i64, declaration.name.clone())
        };
        let first = &body.locals[parameter_local_index(body, 0)];
        let second = &body.locals[parameter_local_index(body, 1)];
        if frame_type(constructors, nominals, enums, first, intrinsics, "")? != reference
            || frame_type(constructors, nominals, enums, second, intrinsics, "")? != other
        {
            continue;
        }
        return match body.params[1].default {
            CallableParamDefault::Required => Ok(function_name(module, body)),
            _ => Err("unsupported Body IR defaulted enum comparison operand".into()),
        };
    }
    Err(format!(
        "unsupported Body IR unavailable published enum comparison body {name}"
    ))
}
