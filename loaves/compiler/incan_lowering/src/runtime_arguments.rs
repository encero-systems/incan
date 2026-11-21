//! Native type arguments of a governed runtime call, read back as the checked types its Incan body closes over.

use incan_lang::lang::types::numerics::NumericTypeId;
use incan_mir_plan_native::{Carrier, EnumDeclaration, PlanType, carrier_position, list_leaf_type, tuple_element_type};
use incan_semantics_core::body_ir::BodyIrModule;
use incan_semantics_core::{
    IncanCallableParam, IncanCallableParamKind, IncanInputAccess, IncanPrimitiveType, IncanType, RuntimeBuiltinType,
    SemanticSourceTargetKind, function_type, generic,
};

use crate::collections::is_owned_collection;
use crate::compiler_record_adapters::semantic_key_matches_native;
use crate::layout_types::carrier_name;
use crate::nominal_instances::{instance_arguments, template_base};
use crate::pending::untranslated;

/// The tables a native argument resolves its owners against.
pub struct RuntimeTypes<'a> {
    pub module: &'a BodyIrModule,
    pub intrinsics: &'a [EnumDeclaration],
    pub unions: &'a [IncanType],
}

/// Retain closed carriers against shared layout inventories without copying tables or reopening checked bodies.
pub fn runtime_argument(types: &RuntimeTypes<'_>, ty: &PlanType) -> Result<IncanType, String> {
    if let Some(primitive) = primitive_argument(ty) {
        return Ok(IncanType::Primitive(primitive));
    }
    let reference = |inner: &PlanType| -> Result<IncanType, String> {
        Ok(IncanType::Ref(Box::new(runtime_argument(types, inner)?)))
    };
    let exclusive = |inner: &PlanType| -> Result<IncanType, String> {
        Ok(IncanType::RefMut(Box::new(runtime_argument(types, inner)?)))
    };
    Ok(match ty {
        PlanType::StaticText => generic("<native-static-text>".into(), Vec::new()),
        PlanType::StaticBytes => generic("<native-static-bytes>".into(), Vec::new()),
        PlanType::Decimal => generic("<native-decimal>".into(), Vec::new()),
        PlanType::ValidationError => IncanType::NativeBuiltin(RuntimeBuiltinType::ValidationError),
        PlanType::ScalarRef(elements) => {
            if elements.len() != 1 {
                return Err("invalid governed runtime scalar reference arity".into());
            }
            reference(&elements[0])?
        }
        PlanType::ScalarMutRef(elements) => {
            if elements.len() != 1 {
                return Err("invalid governed runtime mutable scalar reference arity".into());
            }
            exclusive(&elements[0])?
        }
        PlanType::StringRef => IncanType::Ref(Box::new(IncanType::Primitive(IncanPrimitiveType::Str))),
        PlanType::StringMutRef => IncanType::RefMut(Box::new(IncanType::Primitive(IncanPrimitiveType::Str))),
        PlanType::StrRef => IncanType::Ref(Box::new(IncanType::RustInteropPath("str".into()))),
        PlanType::FrozenCollection(name, elements) => {
            let mut arguments = Vec::with_capacity(elements.len());
            for element in elements {
                arguments.push(runtime_argument(types, element)?);
            }
            generic(name.clone(), arguments)
        }
        PlanType::FrozenCollectionRef(name, elements) => {
            reference(&PlanType::FrozenCollection(name.clone(), elements.clone()))?
        }
        PlanType::Future(outputs) => {
            if outputs.len() != 1 {
                return Err("invalid governed runtime future output arity".into());
            }
            generic("Awaitable".into(), vec![runtime_argument(types, &outputs[0])?])
        }
        PlanType::Model(owner, _) => {
            let declaration = &types.module.nominal_declarations[*owner as usize];
            if declaration.type_parameter_count != 0 && declaration.canonical.kind != SemanticSourceTargetKind::RustItem
            {
                generic(template_base(declaration), instance_arguments(declaration)?)
            } else {
                IncanType::Named(declaration.name.clone())
            }
        }
        PlanType::ModelRef(owner, name) => reference(&PlanType::Model(*owner, name.clone()))?,
        PlanType::ModelMutRef(owner, name) => exclusive(&PlanType::Model(*owner, name.clone()))?,
        PlanType::Enum(owner, name) => runtime_enum_argument(types, *owner, name)?,
        PlanType::EnumRef(owner, name) => IncanType::Ref(Box::new(runtime_enum_argument(types, *owner, name)?)),
        PlanType::EnumMutRef(owner, name) => IncanType::RefMut(Box::new(runtime_enum_argument(types, *owner, name)?)),
        PlanType::Tuple(elements) => {
            let mut fields = Vec::with_capacity(elements.len());
            for element in elements {
                fields.push(runtime_argument(types, &tuple_element_type(element))?);
            }
            IncanType::Tuple(fields)
        }
        PlanType::TupleRef(elements) => reference(&PlanType::Tuple(elements.clone()))?,
        PlanType::FunctionPointer(signature) | PlanType::Closure(signature) => {
            runtime_callable_argument(types, signature)?
        }
        PlanType::TypeToken(represented) => {
            if represented.len() != 1 {
                return Err("invalid governed runtime type token arity".into());
            }
            IncanType::TypeToken(Box::new(runtime_argument(types, &represented[0])?))
        }
        PlanType::Trait(_) => return Err(untranslated("runtime_calls::runtime_argument trait")),
        PlanType::Generator(leaf, depth) => generic(
            "Generator".into(),
            vec![nested_lists(runtime_argument(types, &list_leaf_type(leaf))?, *depth)],
        ),
        PlanType::List(leaf, depth) => nested_lists(runtime_argument(types, &list_leaf_type(leaf))?, *depth),
        PlanType::ListRef(leaf, depth) => reference(&PlanType::List(leaf.clone(), *depth))?,
        PlanType::ListMutRef(leaf, depth) => exclusive(&PlanType::List(leaf.clone(), *depth))?,
        PlanType::Set(leaf) => generic("Set".into(), vec![runtime_argument(types, &list_leaf_type(leaf))?]),
        PlanType::Dict(key, value) => {
            let key_type = runtime_argument(types, &list_leaf_type(key))?;
            let value_type = runtime_argument(types, &list_leaf_type(value))?;
            generic("Dict".into(), vec![key_type, value_type])
        }
        _ => {
            return Err(format!(
                "unsupported governed runtime type argument representation {ty:?}"
            ));
        }
    })
}

/// The primitive checked type of a scalar carrier, if it is one.
fn primitive_argument(ty: &PlanType) -> Option<IncanPrimitiveType> {
    let numeric = |id: NumericTypeId| Some(IncanPrimitiveType::Numeric(id));
    match ty {
        PlanType::Int => Some(IncanPrimitiveType::Int),
        PlanType::Float => Some(IncanPrimitiveType::Float),
        PlanType::I8 => numeric(NumericTypeId::I8),
        PlanType::I16 => numeric(NumericTypeId::I16),
        PlanType::I32 => numeric(NumericTypeId::I32),
        PlanType::I128 => numeric(NumericTypeId::I128),
        PlanType::ISize => numeric(NumericTypeId::ISize),
        PlanType::USize => numeric(NumericTypeId::USize),
        PlanType::U8 => numeric(NumericTypeId::U8),
        PlanType::U16 => numeric(NumericTypeId::U16),
        PlanType::U32 => numeric(NumericTypeId::U32),
        PlanType::U64 => numeric(NumericTypeId::U64),
        PlanType::U128 => numeric(NumericTypeId::U128),
        PlanType::F32 => numeric(NumericTypeId::F32),
        PlanType::F64 => numeric(NumericTypeId::F64),
        PlanType::Bool => Some(IncanPrimitiveType::Bool),
        PlanType::Unit => Some(IncanPrimitiveType::Unit),
        PlanType::String => Some(IncanPrimitiveType::Str),
        PlanType::FrozenStr => Some(IncanPrimitiveType::FrozenStr),
        PlanType::FrozenBytes => Some(IncanPrimitiveType::FrozenBytes),
        _ => None,
    }
}

/// Wrap an element in `depth` list layers.
fn nested_lists(element: IncanType, depth: i64) -> IncanType {
    let mut nested = element;
    for _ in 0..depth {
        nested = generic("List".into(), vec![nested]);
    }
    nested
}

/// Preserve every native callable input and result; the instance's owned slots separately retain environments.
fn runtime_callable_argument(types: &RuntimeTypes<'_>, signature: &[PlanType]) -> Result<IncanType, String> {
    let Some((result, inputs)) = signature.split_last() else {
        return Err("invalid governed runtime callable signature without a result".into());
    };
    let mut parameters = Vec::with_capacity(inputs.len());
    for input in inputs {
        parameters.push(IncanCallableParam {
            native_owned: is_owned_collection(input),
            input_access: IncanInputAccess::Inferred,
            name: None,
            ty: runtime_argument(types, input)?,
            kind: IncanCallableParamKind::Normal,
            has_default: false,
            is_partial_preset: false,
            is_mut: false,
        });
    }
    Ok(function_type(parameters, runtime_argument(types, result)?))
}

/// Read enum owners and closed union shapes from retained inventories, preserving Option and Result payloads.
fn runtime_enum_argument(types: &RuntimeTypes<'_>, owner: i64, name: &str) -> Result<IncanType, String> {
    if owner < 0 {
        return Err("invalid governed runtime enum owner".into());
    }
    if let Some(source_enum) = types.module.enum_declarations.get(owner as usize) {
        return Ok(IncanType::Named(source_enum.name.clone()));
    }
    let slot = carrier_position(owner);
    let Some(declaration) = types
        .intrinsics
        .get(slot as usize)
        .filter(|declaration| declaration.name == name)
    else {
        return Err("invalid governed runtime intrinsic enum owner".into());
    };
    if declaration.carrier != Carrier::Option && declaration.carrier != Carrier::Result {
        for union in types.unions {
            if semantic_key_matches_native(&declaration.semantic_key, union)? {
                return Ok(union.clone());
            }
        }
        return Err("unsupported governed runtime intrinsic enum argument".into());
    }
    let mut arguments = Vec::new();
    for variant in &declaration.variants {
        for field in &variant.fields {
            arguments.push(runtime_argument(types, field)?);
        }
    }
    if (declaration.carrier == Carrier::Option && arguments.len() != 1)
        || (declaration.carrier == Carrier::Result && arguments.len() != 2)
    {
        return Err("invalid governed runtime intrinsic payload arity".into());
    }
    match carrier_name(&declaration.carrier) {
        Some(carrier) => Ok(generic(carrier, arguments)),
        None => Err("invalid governed runtime intrinsic carrier".into()),
    }
}
