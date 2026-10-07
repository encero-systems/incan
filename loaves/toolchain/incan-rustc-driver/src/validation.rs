//! Preflight validation of the Incan-owned scalar plan. No rustc type crosses this module.

use std::collections::BTreeSet;

use crate::error::PlanError;
use crate::plan::{
    BinaryOp, Callee, CalleeKind, Constant, Function, ListLeaf, Operand, OperandKind, Place, Plan, PlanType,
    Projection, RvalueKind, SizedNumeric, SourceSpan, StatementKind, TerminatorKind, UnaryOp, Unwind,
    tuple_element_type,
};

/// Copyable comparison mirror of the Incan-owned sized carrier enum.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Numeric {
    I8,
    I16,
    I32,
    I128,
    U8,
    U16,
    U32,
    U64,
    U128,
    F32,
    F64,
    ISize,
    USize,
}

/// Preserve each named carrier across the plan validation boundary.
fn numeric(kind: &SizedNumeric) -> Numeric {
    match kind {
        SizedNumeric::I8 => Numeric::I8,
        SizedNumeric::I16 => Numeric::I16,
        SizedNumeric::I32 => Numeric::I32,
        SizedNumeric::I128 => Numeric::I128,
        SizedNumeric::U8 => Numeric::U8,
        SizedNumeric::U16 => Numeric::U16,
        SizedNumeric::U32 => Numeric::U32,
        SizedNumeric::U64 => Numeric::U64,
        SizedNumeric::U128 => Numeric::U128,
        SizedNumeric::F32 => Numeric::F32,
        SizedNumeric::F64 => Numeric::F64,
        SizedNumeric::ISize => Numeric::ISize,
        SizedNumeric::USize => Numeric::USize,
    }
}

/// A local comparison vocabulary; the public types remain the Incan Loaf's own types.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Scalar {
    UnitFunction,
    Int,
    Float,
    I8,
    I16,
    I32,
    I128,
    ISize,
    USize,
    U8,
    U16,
    U32,
    U64,
    U128,
    F32,
    F64,
    CheckedNumeric(Numeric),
    Bool,
    Unit,
    EnumTag,
    Enum(i64),
    EnumRef(i64),
    CheckedInt,
    Tuple(Vec<Scalar>),
    String,
    StringRef,
    Decimal,
    StrRef,
    StringArray(i64),
    StrArray(i64),
    StringArrayRef(i64),
    StrArrayRef(i64),
    Model(i64),
    ModelRef(i64),
    ModelMutRef(i64),
    StringSlice,
    StrSlice,
    List(Leaf, i64),
    ListRef(Leaf, i64),
    ListMutRef(Leaf, i64),
    Set(Leaf),
    SetRef(Leaf),
    SetMutRef(Leaf),
    Dict(Leaf, Leaf),
    DictRef(Leaf, Leaf),
    DictMutRef(Leaf, Leaf),
    ZipIterator(Leaf, Leaf),
    ZipIteratorRef(Leaf, Leaf),
    ZipIteratorMutRef(Leaf, Leaf),
    Generator(Leaf, i64),
    GeneratorMutRef(Leaf, i64),
    GeneratorYield(Leaf, i64),
    GeneratorYieldRef(Leaf, i64),
}

/// Comparison mirror of scalar and tuple leaves; the public plan remains Incan-authored.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Leaf {
    Int,
    Float,
    Bool,
    Str,
    Tuple(Vec<Scalar>),
    Model(i64),
    U8,
    Unit,
    Decimal,
}

impl Leaf {
    /// Mirror one plan leaf.
    fn of(leaf: &ListLeaf) -> Self {
        match leaf {
            ListLeaf::Int => Leaf::Int,
            ListLeaf::Float => Leaf::Float,
            ListLeaf::Bool => Leaf::Bool,
            ListLeaf::Str => Leaf::Str,
            ListLeaf::Tuple(elements) => Leaf::Tuple(elements.iter().map(|element| scalar(&tuple_element_type(element.clone()))).collect()),
            ListLeaf::Model(index, _) => Leaf::Model(*index),
            ListLeaf::U8 => Leaf::U8,
            ListLeaf::Unit => Leaf::Unit,
            ListLeaf::Decimal => Leaf::Decimal,
        }
    }
}

/// Mirror a flat tuple component at the iterator boundary.
fn tuple_leaf(element: &crate::plan::TupleElement) -> Leaf {
    Leaf::Tuple(vec![scalar(&tuple_element_type(element.clone()))])
}

/// Compare source-authored types without requiring a Rust derive on Incan types.
fn scalar(ty: &PlanType) -> Scalar {
    match ty {
        PlanType::UnitFunction => Scalar::UnitFunction,
        PlanType::ZipIterator(left, right) => Scalar::ZipIterator(tuple_leaf(left), tuple_leaf(right)),
        PlanType::ZipIteratorRef(left, right) => Scalar::ZipIteratorRef(tuple_leaf(left), tuple_leaf(right)),
        PlanType::ZipIteratorMutRef(left, right) => Scalar::ZipIteratorMutRef(tuple_leaf(left), tuple_leaf(right)),
        PlanType::EnumTag => Scalar::EnumTag,
        PlanType::Generator(leaf, depth) => Scalar::Generator(Leaf::of(leaf), *depth),
        PlanType::GeneratorMutRef(leaf, depth) => Scalar::GeneratorMutRef(Leaf::of(leaf), *depth),
        PlanType::GeneratorYield(leaf, depth) => Scalar::GeneratorYield(Leaf::of(leaf), *depth),
        PlanType::GeneratorYieldRef(leaf, depth) => Scalar::GeneratorYieldRef(Leaf::of(leaf), *depth),
        PlanType::Enum(index, _) => Scalar::Enum(*index),
        PlanType::EnumRef(index, _) => Scalar::EnumRef(*index),
        PlanType::Model(index, _) => Scalar::Model(*index),
        PlanType::ModelMutRef(index, _) => Scalar::ModelMutRef(*index),
        PlanType::ModelRef(index, _) => Scalar::ModelRef(*index),
        PlanType::List(leaf, depth) => Scalar::List(Leaf::of(leaf), *depth),
        PlanType::ListRef(leaf, depth) => Scalar::ListRef(Leaf::of(leaf), *depth),
        PlanType::ListMutRef(leaf, depth) => Scalar::ListMutRef(Leaf::of(leaf), *depth),
        PlanType::Set(leaf) => Scalar::Set(Leaf::of(leaf)),
        PlanType::SetRef(leaf) => Scalar::SetRef(Leaf::of(leaf)),
        PlanType::SetMutRef(leaf) => Scalar::SetMutRef(Leaf::of(leaf)),
        PlanType::Dict(key, value) => Scalar::Dict(Leaf::of(key), Leaf::of(value)),
        PlanType::DictRef(key, value) => Scalar::DictRef(Leaf::of(key), Leaf::of(value)),
        PlanType::DictMutRef(key, value) => Scalar::DictMutRef(Leaf::of(key), Leaf::of(value)),
        PlanType::Tuple(elements) => Scalar::Tuple(
            elements
                .iter()
                .map(|element| scalar(&tuple_element_type(element.clone())))
                .collect(),
        ),
        PlanType::Int => Scalar::Int,
        PlanType::Float => Scalar::Float,
        PlanType::I8 => Scalar::I8,
        PlanType::I16 => Scalar::I16,
        PlanType::I32 => Scalar::I32,
        PlanType::I128 => Scalar::I128,
        PlanType::U8 => Scalar::U8,
        PlanType::U16 => Scalar::U16,
        PlanType::U32 => Scalar::U32,
        PlanType::U64 => Scalar::U64,
        PlanType::U128 => Scalar::U128,
        PlanType::F32 => Scalar::F32,
        PlanType::F64 => Scalar::F64,
        PlanType::CheckedNumeric(ty) => Scalar::CheckedNumeric(numeric(ty)),
        PlanType::ISize => Scalar::ISize,
        PlanType::USize => Scalar::USize,
        PlanType::Bool => Scalar::Bool,
        PlanType::Unit => Scalar::Unit,
        PlanType::CheckedInt => Scalar::CheckedInt,
        PlanType::String => Scalar::String,
        PlanType::Decimal => Scalar::Decimal,
        PlanType::StringRef => Scalar::StringRef,
        PlanType::StrRef => Scalar::StrRef,
        PlanType::StringArray(count) => Scalar::StringArray(*count),
        PlanType::StrArray(count) => Scalar::StrArray(*count),
        PlanType::StringArrayRef(count) => Scalar::StringArrayRef(*count),
        PlanType::StrArrayRef(count) => Scalar::StrArrayRef(*count),
        PlanType::StringSlice => Scalar::StringSlice,
        PlanType::StrSlice => Scalar::StrSlice,
    }
}

/// Describe a refusal in the containing source function.
fn invalid(function: &Function, reason: impl Into<String>) -> PlanError {
    PlanError::Invalid {
        function: function.name.clone(),
        reason: reason.into(),
    }
}

/// Validate the entire crate before loading source files or invoking rustc.
///
/// This pass checks references, signatures, scalar expression types, and cleanup edges. It is not an ownership
/// checker: stage 3c must only submit checked bodies, and rustc's borrow checker remains a backstop.
pub fn validate(plan: &Plan) -> Result<(), PlanError> {
    span(&plan.span)?;
    let mut names = BTreeSet::new();
    for function in &plan.functions {
        if !identifier(&function.name) || !names.insert(&function.name) {
            return Err(invalid(function, "function name is invalid or duplicated"));
        }
    }
    validate_models(plan)?;
    validate_enums(plan)?;
    let mut paths = BTreeSet::new();
    for external in &plan.externals {
        span(&external.span)?;
        let segments: Vec<_> = external.path.split("::").collect();
        if segments.len() < 2
            || !segments.iter().all(|segment| identifier(segment))
            || !paths.insert((
                external.path.clone(),
                external.type_arguments.iter().map(scalar).collect::<Vec<_>>(),
            ))
        {
            return Err(PlanError::Invalid {
                function: external.path.clone(),
                reason: "invalid or duplicate canonical external path".into(),
            });
        }
        if external
            .parameters
            .iter()
            .any(|ty| !external_signature_type(scalar(ty)))
            || !external_signature_type(scalar(&external.return_type))
        {
            return Err(PlanError::Invalid {
                function: external.path.clone(),
                reason: "checked pairs and formatting arrays cannot cross a function signature".into(),
            });
        }
    }
    let main = plan
        .functions
        .iter()
        .find(|function| function.name == "main")
        .ok_or_else(|| PlanError::UnknownCallee("main".into()))?;
    if !main.parameters.is_empty() || scalar(&main.return_type) != Scalar::Unit {
        return Err(invalid(
            main,
            "native binary main must take no parameters and return unit",
        ));
    }
    for function in &plan.functions {
        validate_function(plan, function)?;
    }
    Ok(())
}

/// Only ordinary source identifiers are admitted; paths are built as AST nodes, never parsed as source.
fn identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|ch| ch.is_ascii_alphabetic() || ch == '_')
        && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        && !matches!(name, "self" | "Self" | "super" | "crate")
}

/// Source coordinates are one-based and must have a source file name.
fn span(source: &SourceSpan) -> Result<(), PlanError> {
    if source.file.is_empty() || source.line < 1 || source.column < 1 {
        return Err(PlanError::Invalid {
            function: source.file.clone(),
            reason: "source span requires a file and positive line and column".into(),
        });
    }
    Ok(())
}

/// Check the parameter-local convention and all nodes in one function.
fn validate_function(plan: &Plan, function: &Function) -> Result<(), PlanError> {
    span(&function.span)?;
    if function.blocks.is_empty() || function.blocks[0].cleanup {
        return Err(invalid(
            function,
            "entry block must exist and cannot be a cleanup block",
        ));
    }
    if function.locals.len() <= function.parameters.len() {
        return Err(invalid(function, "locals must include return place and all parameters"));
    }
    if scalar(&function.locals[0].ty) != scalar(&function.return_type)
        || !source_signature_type(scalar(&function.return_type))
    {
        return Err(invalid(function, "return local must match a scalar return type"));
    }
    let mut parameters = BTreeSet::new();
    for (index, parameter) in function.parameters.iter().enumerate() {
        span(&parameter.span)?;
        if !identifier(&parameter.name) || !parameters.insert(&parameter.name) {
            return Err(invalid(function, "parameter name is invalid or duplicated"));
        }
        if !(source_signature_type(scalar(&parameter.ty))
            || matches!(scalar(&parameter.ty), Scalar::ModelRef(_) | Scalar::ModelMutRef(_)))
            || scalar(&parameter.ty) != scalar(&function.locals[index + 1].ty)
        {
            return Err(invalid(function, "parameter local differs from its declaration"));
        }
    }
    for local in &function.locals {
        span(&local.span)?;
        validate_model_type(plan, &local.ty)?;
        validate_array_length(function, scalar(&local.ty))?;
    }
    for block in &function.blocks {
        span(&block.span)?;
        for statement in &block.statements {
            span(&statement.span)?;
            match &statement.kind {
                StatementKind::Assign(destination, value) => {
                    span(&value.span)?;
                    let expected = place(plan, function, destination)?;
                    let result = rvalue(plan, function, &value.kind, expected.clone())?;
                    require(function, expected, result, "assignment")?;
                }
                StatementKind::StorageLive(index) | StatementKind::StorageDead(index) => {
                    local(function, *index)?;
                    if *index
                        <= i64::try_from(function.parameters.len())
                            .map_err(|_| invalid(function, "too many parameters"))?
                    {
                        return Err(invalid(
                            function,
                            "storage markers cannot target return place or parameters",
                        ));
                    }
                }
            }
        }
        span(&block.terminator.span)?;
        terminator(plan, function, &block.terminator.kind, block.cleanup)?;
    }
    Ok(())
}

/// Refuse signed, oversized, or dangling local indices before constructing rustc index types.
fn local(function: &Function, index: i64) -> Result<Scalar, PlanError> {
    usize::try_from(index)
        .ok()
        .and_then(|index| function.locals.get(index))
        .map(|local| scalar(&local.ty))
        .ok_or_else(|| PlanError::UnknownLocal {
            function: function.name.clone(),
            index,
        })
}

/// Resolve checked arithmetic, list dereferences, and nominal projections, verifying reference shape and canonical
/// pointee or field type.
fn place(plan: &Plan, function: &Function, value: &Place) -> Result<Scalar, PlanError> {
    span(&value.span)?;
    let ty = local(function, value.local)?;
    match &value.projection {
        Projection::Whole => Ok(ty),
        Projection::VariantField(variant, slot, field_type) => {
            let Scalar::Enum(owner) = ty else {
                return Err(invalid(function, "variant projection requires an enum owner"));
            };
            let declaration = enum_declaration(plan, owner).ok_or_else(|| invalid(function, "unknown enum owner"))?;
            let field = usize::try_from(*variant)
                .ok()
                .and_then(|index| declaration.variants.get(index))
                .and_then(|variant| usize::try_from(*slot).ok().and_then(|index| variant.fields.get(index)))
                .ok_or_else(|| invalid(function, "unknown enum payload field"))?;
            require(function, scalar(field_type), scalar(field), "enum payload projection")?;
            Ok(scalar(field_type))
        }
        Projection::Deref(field_type) => {
            let pointee = match ty {
                Scalar::ModelRef(owner) | Scalar::ModelMutRef(owner) => Scalar::Model(owner),
                Scalar::ZipIteratorRef(left, right) | Scalar::ZipIteratorMutRef(left, right) => {
                    Scalar::ZipIterator(left, right)
                }
                Scalar::GeneratorMutRef(leaf, depth) => Scalar::Generator(leaf, depth),
                Scalar::ListRef(leaf, depth) | Scalar::ListMutRef(leaf, depth) => Scalar::List(leaf, depth),
                Scalar::SetRef(leaf) | Scalar::SetMutRef(leaf) => Scalar::Set(leaf),
                Scalar::DictRef(key, value) | Scalar::DictMutRef(key, value) => Scalar::Dict(key, value),
                _ => {
                    return Err(invalid(
                        function,
                        "dereference requires a collection or nominal reference",
                    ));
                }
            };
            require(function, scalar(field_type), &pointee, "dereferenced owner")?;
            Ok(pointee)
        }
        Projection::Field(slot, field_type) | Projection::DerefField(slot, field_type) => {
            if let Scalar::Tuple(elements) = &ty {
                let field = usize::try_from(*slot)
                    .ok()
                    .and_then(|slot| elements.get(slot))
                    .ok_or_else(|| invalid(function, "unknown tuple field"))?;
                require(
                    function,
                    scalar(field_type),
                    field.clone(),
                    "projected tuple field type",
                )?;
                return Ok(field.clone());
            }
            let owner = match (&value.projection, ty) {
                (Projection::Field(..), Scalar::Model(owner)) => owner,
                (Projection::DerefField(..), Scalar::ModelRef(owner) | Scalar::ModelMutRef(owner)) => owner,
                _ => {
                    return Err(invalid(function, "field projection requires a matching nominal owner"));
                }
            };
            let model = model(plan, owner).ok_or_else(|| invalid(function, "unknown model owner"))?;
            let field = usize::try_from(*slot)
                .ok()
                .and_then(|slot| model.fields.get(slot))
                .ok_or_else(|| invalid(function, "unknown model field"))?;
            require(function, scalar(field_type), scalar(&field.ty), "projected field type")?;
            Ok(scalar(field_type))
        }
        Projection::NumericValue(kind)
            if sized_numeric(&scalar(kind)).is_some_and(|kind| ty == Scalar::CheckedNumeric(kind)) =>
        {
            Ok(scalar(kind))
        }
        Projection::Overflow if matches!(ty, Scalar::CheckedNumeric(_)) => Ok(Scalar::Bool),
        Projection::Value if ty == Scalar::CheckedInt => Ok(Scalar::Int),
        Projection::Overflow if ty == Scalar::CheckedInt => Ok(Scalar::Bool),
        _ => Err(invalid(function, "only checked integer results support projections")),
    }
}

/// Check an edge's index and whether it crosses the normal/cleanup boundary correctly.
fn edge(function: &Function, index: i64, cleanup: bool) -> Result<(), PlanError> {
    let block = usize::try_from(index)
        .ok()
        .and_then(|index| function.blocks.get(index))
        .ok_or_else(|| PlanError::UnknownBlock {
            function: function.name.clone(),
            index,
        })?;
    if block.cleanup != cleanup {
        return Err(invalid(function, "edge crosses normal and cleanup control flow"));
    }
    Ok(())
}

/// Cleanup edges must name cleanup blocks; cleanup code cannot install a nested cleanup edge.
fn unwind(function: &Function, action: &Unwind, cleanup: bool) -> Result<(), PlanError> {
    match action {
        Unwind::Continue if !cleanup => Ok(()),
        Unwind::Cleanup(index) if !cleanup => edge(function, *index, true),
        Unwind::Terminate if cleanup => Ok(()),
        _ => Err(invalid(function, "cleanup code cannot unwind again")),
    }
}

/// Infer a source operand's exact scalar representation.
fn operand(plan: &Plan, function: &Function, value: &Operand) -> Result<Scalar, PlanError> {
    span(&value.span)?;
    match &value.kind {
        OperandKind::Copy(value) => {
            let ty = place(plan, function, value)?;
            if matches!(ty, Scalar::Enum(index) if enum_declaration(plan, index).is_none_or(|value| !value.derives.iter().any(|name| name == "Copy")))
            {
                return Err(invalid(function, "non-Copy enum cannot be copied"));
            }
            if owns_values(&ty)
                || matches!(
                    ty,
                    Scalar::String
                        | Scalar::ZipIterator(_, _)
                        | Scalar::ZipIteratorMutRef(_, _)
                        | Scalar::Generator(_, _)
                        | Scalar::GeneratorMutRef(_, _)
                        | Scalar::GeneratorYield(_, _)
                        | Scalar::StringArray(_)
                        | Scalar::Model(_)
                        | Scalar::List(_, _)
                        | Scalar::ListMutRef(_, _)
                        | Scalar::Set(_)
                        | Scalar::SetMutRef(_)
                        | Scalar::Dict(_, _)
                        | Scalar::DictMutRef(_, _)
                )
            {
                return Err(invalid(function, "owned formatting values cannot be copied"));
            }
            Ok(ty)
        }
        OperandKind::Move(value) => place(plan, function, value),
        OperandKind::Literal(value) => Ok(match value {
            Constant::Int(_) => Scalar::Int,
            Constant::Numeric(_, ty) => scalar(ty),
            Constant::Float(_) => Scalar::Float,
            Constant::Bool(_) => Scalar::Bool,
            Constant::Unit => Scalar::Unit,
            Constant::Text(_) => Scalar::StrRef,
        }),
    }
}

/// Reject implicit copies of any recursively owned value, including tuple string fields.
fn owns_values(ty: &Scalar) -> bool {
    match ty {
        Scalar::String | Scalar::StringArray(_) | Scalar::Model(_) => true,
        Scalar::Tuple(elements) => elements.iter().any(owns_values),
        _ => false,
    }
}

/// Keep type mismatches explicit rather than allowing rustc MIR validation to ICE.
fn require(
    function: &Function,
    actual: Scalar,
    expected: impl std::borrow::Borrow<Scalar>,
    context: &str,
) -> Result<(), PlanError> {
    let expected = expected.borrow();
    if &actual != expected {
        return Err(invalid(
            function,
            format!("{context}: expected {expected:?}, found {actual:?}"),
        ));
    }
    Ok(())
}

/// Validate scalar and aggregate rvalues against their exact destination layout.
fn rvalue(plan: &Plan, function: &Function, value: &RvalueKind, expected: Scalar) -> Result<Scalar, PlanError> {
    match value {
        RvalueKind::UnitFunction(name) => {
            let callback = plan
                .functions
                .iter()
                .find(|value| &value.name == name)
                .ok_or_else(|| PlanError::UnknownCallee(name.clone()))?;
            if !callback.parameters.is_empty() || scalar(&callback.return_type) != Scalar::Unit {
                return Err(invalid(
                    function,
                    "executor callback must take no arguments and return unit",
                ));
            }
            Ok(Scalar::UnitFunction)
        }
        RvalueKind::Use(value) => operand(plan, function, value),
        RvalueKind::NumericCast(value, source, target) => {
            let source = scalar(source);
            let target = scalar(target);
            require(
                function,
                operand(plan, function, value)?,
                source.clone(),
                "numeric cast source",
            )?;
            let numeric = |ty: &Scalar| sized_numeric(ty).is_some() || matches!(ty, Scalar::Int | Scalar::Float);
            if !numeric(&source) || !numeric(&target) {
                return Err(invalid(function, "numeric cast requires numeric carriers"));
            }
            Ok(target)
        }
        RvalueKind::Discriminant(value) => {
            if !matches!(place(plan, function, value)?, Scalar::Enum(_)) {
                return Err(invalid(function, "discriminant requires an enum"));
            }
            Ok(Scalar::EnumTag)
        }
        RvalueKind::TagToInt(value) => {
            require(
                function,
                operand(plan, function, value)?,
                Scalar::EnumTag,
                "discriminant conversion",
            )?;
            Ok(Scalar::Int)
        }
        RvalueKind::Enum(owner, variant, elements) => {
            require(
                function,
                expected.clone(),
                Scalar::Enum(*owner),
                "enum aggregate destination",
            )?;
            let declaration =
                enum_declaration(plan, *owner).ok_or_else(|| invalid(function, "unknown constructed enum"))?;
            let variant = usize::try_from(*variant)
                .ok()
                .and_then(|index| declaration.variants.get(index))
                .ok_or_else(|| invalid(function, "unknown constructed variant"))?;
            if variant.fields.len() != elements.len() {
                return Err(invalid(function, "enum payload count differs from declaration"));
            }
            for (element, field) in elements.iter().zip(&variant.fields) {
                require(
                    function,
                    operand(plan, function, element)?,
                    scalar(field),
                    "enum payload",
                )?;
            }
            Ok(expected)
        }
        RvalueKind::IntToFloat(value) => {
            require(function, operand(plan, function, value)?, Scalar::Int, "int-to-float")?;
            Ok(Scalar::Float)
        }
        RvalueKind::FloatToInt(value) => {
            require(function, operand(plan, function, value)?, Scalar::Float, "float-to-int")?;
            Ok(Scalar::Int)
        }
        RvalueKind::BoolToInt(value) => {
            require(function, operand(plan, function, value)?, Scalar::Bool, "bool-to-int")?;
            Ok(Scalar::Int)
        }
        RvalueKind::Unary(op, value) => {
            let ty = operand(plan, function, value)?;
            match op {
                UnaryOp::Not if ty == Scalar::Bool => Ok(ty),
                UnaryOp::Negate
                    if matches!(
                        ty,
                        Scalar::Int
                            | Scalar::Float
                            | Scalar::I8
                            | Scalar::I16
                            | Scalar::I32
                            | Scalar::I128
                            | Scalar::F32
                            | Scalar::F64
                    ) =>
                {
                    Ok(ty)
                }
                _ => Err(invalid(function, "invalid unary operand type")),
            }
        }
        RvalueKind::Binary(op, left, right) => {
            let ty = operand(plan, function, left)?;
            require(function, operand(plan, function, right)?, ty.clone(), "binary operands")?;
            binary_result(function, op, ty, expected)
        }
        RvalueKind::Array(elements) => validate_array(plan, function, elements, expected),
        RvalueKind::Tuple(elements) => {
            let Scalar::Tuple(fields) = &expected else {
                return Err(invalid(function, "tuple aggregate requires a tuple destination"));
            };
            if fields.len() != elements.len() {
                return Err(invalid(function, "tuple element count differs from destination"));
            }
            for (element, field) in elements.iter().zip(fields) {
                require(
                    function,
                    operand(plan, function, element)?,
                    field.clone(),
                    "tuple element",
                )?;
            }
            Ok(expected)
        }
        RvalueKind::Model(index, elements) => {
            require(
                function,
                expected.clone(),
                Scalar::Model(*index),
                "model aggregate destination",
            )?;
            let model = model(plan, *index).ok_or_else(|| invalid(function, "unknown constructed model"))?;
            if elements.len() != model.fields.len() {
                return Err(invalid(function, "model field count differs from declaration"));
            }
            for (element, field) in elements.iter().zip(&model.fields) {
                require(
                    function,
                    operand(plan, function, element)?,
                    scalar(&field.ty),
                    "model field",
                )?;
            }
            Ok(expected)
        }
        RvalueKind::MutBorrow(value) => borrow_result(plan, function, value, true),
        RvalueKind::Borrow(value) => borrow_result(plan, function, value, false),
        RvalueKind::UnsizeSlice(value) => match operand(plan, function, value)? {
            Scalar::StringArrayRef(_) => Ok(Scalar::StringSlice),
            Scalar::StrArrayRef(_) => Ok(Scalar::StrSlice),
            _ => Err(invalid(
                function,
                "slice coercion requires a shared formatting-array reference",
            )),
        },
    }
}

/// Form the exact borrowed carrier, rejecting mutable reborrows through a shared owner before native MIR construction.
fn borrow_result(plan: &Plan, function: &Function, value: &Place, mutable: bool) -> Result<Scalar, PlanError> {
    let ty = place(plan, function, value)?;
    if mutable {
        return match ty {
            Scalar::ZipIterator(left, right) => Ok(Scalar::ZipIteratorMutRef(left, right)),
            Scalar::Generator(leaf, depth) => Ok(Scalar::GeneratorMutRef(leaf, depth)),
            Scalar::Model(index) => {
                if matches!(local(function, value.local)?, Scalar::ModelRef(_)) {
                    return Err(invalid(function, "cannot mutably reborrow a shared receiver"));
                }
                Ok(Scalar::ModelMutRef(index))
            }
            Scalar::List(leaf, depth) => {
                if matches!(local(function, value.local)?, Scalar::ListRef(..)) {
                    return Err(invalid(function, "cannot mutably reborrow a shared list"));
                }
                Ok(Scalar::ListMutRef(leaf, depth))
            }
            Scalar::Set(leaf) => {
                if matches!(local(function, value.local)?, Scalar::SetRef(_)) {
                    return Err(invalid(function, "cannot mutably reborrow a shared set"));
                }
                Ok(Scalar::SetMutRef(leaf))
            }
            Scalar::Dict(key, item) => {
                if matches!(local(function, value.local)?, Scalar::DictRef(..)) {
                    return Err(invalid(function, "cannot mutably reborrow a shared dictionary"));
                }
                Ok(Scalar::DictMutRef(key, item))
            }
            _ => Err(invalid(
                function,
                "mutable borrow requires a collection or nominal value",
            )),
        };
    }
    match ty {
        Scalar::ZipIterator(left, right) => Ok(Scalar::ZipIteratorRef(left, right)),
        Scalar::GeneratorYield(leaf, depth) => Ok(Scalar::GeneratorYieldRef(leaf, depth)),
        Scalar::Enum(index) => Ok(Scalar::EnumRef(index)),
        Scalar::Model(index) => Ok(Scalar::ModelRef(index)),
        Scalar::List(leaf, depth) => Ok(Scalar::ListRef(leaf, depth)),
        Scalar::Set(leaf) => Ok(Scalar::SetRef(leaf)),
        Scalar::Dict(key, value) => Ok(Scalar::DictRef(key, value)),
        Scalar::String => Ok(Scalar::StringRef),
        Scalar::StringArray(count) => Ok(Scalar::StringArrayRef(count)),
        Scalar::StrArray(count) => Ok(Scalar::StrArrayRef(count)),
        _ => Err(invalid(function, "shared borrow requires an owned formatting value")),
    }
}

/// Source signatures expose scalars, owned text, models, and tuples of admitted types; formatting views stay internal.
fn source_signature_type(ty: Scalar) -> bool {
    if let Scalar::Tuple(elements) = &ty {
        return elements.iter().all(|element| {
            matches!(
                element,
                Scalar::Int | Scalar::Float | Scalar::Bool | Scalar::Unit | Scalar::String
            )
        });
    }
    sized_numeric(&ty).is_some()
        || matches!(
            ty,
            Scalar::Int
                | Scalar::UnitFunction
                | Scalar::ZipIterator(_, _)
                | Scalar::ZipIteratorMutRef(_, _)
                | Scalar::Generator(_, _)
                | Scalar::GeneratorMutRef(_, _)
                | Scalar::GeneratorYield(_, _)
                | Scalar::Float
                | Scalar::Bool
                | Scalar::Unit
                | Scalar::String
                | Scalar::Decimal
                | Scalar::Model(_)
                | Scalar::Enum(_)
                | Scalar::List(_, _)
                | Scalar::ListRef(_, _)
                | Scalar::ListMutRef(_, _)
                | Scalar::Set(_)
                | Scalar::SetRef(_)
                | Scalar::SetMutRef(_)
                | Scalar::Dict(_, _)
                | Scalar::DictRef(_, _)
                | Scalar::DictMutRef(_, _)
        )
}

/// Runtime signatures additionally admit shared text and slice views, whose regions metadata checking erases.
fn external_signature_type(ty: Scalar) -> bool {
    source_signature_type(ty.clone())
        || matches!(
            ty,
            Scalar::ZipIteratorRef(_, _)
                | Scalar::StringRef
                | Scalar::StrRef
                | Scalar::StringSlice
                | Scalar::StrSlice
                | Scalar::EnumRef(_)
                | Scalar::ModelRef(_)
                | Scalar::GeneratorYieldRef(_, _)
        )
}

/// Reject unsupported hashed leaves and invalid dimensions before constructing native types, including unused locals.
fn validate_array_length(function: &Function, ty: Scalar) -> Result<(), PlanError> {
    match ty {
        Scalar::Generator(_, depth)
        | Scalar::GeneratorMutRef(_, depth)
        | Scalar::GeneratorYield(_, depth)
        | Scalar::GeneratorYieldRef(_, depth)
            if depth < 0 =>
        {
            Err(invalid(function, "generator element depth must be nonnegative"))
        }
        Scalar::Set(Leaf::Tuple(elements))
        | Scalar::SetRef(Leaf::Tuple(elements))
        | Scalar::SetMutRef(Leaf::Tuple(elements))
        | Scalar::Dict(Leaf::Tuple(elements), _)
        | Scalar::DictRef(Leaf::Tuple(elements), _)
        | Scalar::DictMutRef(Leaf::Tuple(elements), _) if elements.contains(&Scalar::Float) =>
            Err(invalid(function, "floating-point tuple hashed keys lack Eq and Hash")),
        Scalar::Dict(_, Leaf::Tuple(_))
        | Scalar::DictRef(_, Leaf::Tuple(_))
        | Scalar::DictMutRef(_, Leaf::Tuple(_)) => Err(invalid(function, "tuple dictionary values are not admitted")),
        Scalar::Set(Leaf::Model(_))
        | Scalar::SetRef(Leaf::Model(_))
        | Scalar::SetMutRef(Leaf::Model(_))
        | Scalar::Dict(Leaf::Model(_), _)
        | Scalar::DictRef(Leaf::Model(_), _)
        | Scalar::DictMutRef(Leaf::Model(_), _)
        | Scalar::Dict(_, Leaf::Model(_))
        | Scalar::DictRef(_, Leaf::Model(_))
        | Scalar::DictMutRef(_, Leaf::Model(_)) => Err(invalid(function, "model leaves are admitted only in lists")),
        Scalar::Set(Leaf::Float)
        | Scalar::SetRef(Leaf::Float)
        | Scalar::SetMutRef(Leaf::Float)
        | Scalar::Dict(Leaf::Float, _)
        | Scalar::DictRef(Leaf::Float, _)
        | Scalar::DictMutRef(Leaf::Float, _) => Err(invalid(function, "floating-point hashed keys lack Eq and Hash")),
        Scalar::StringArray(count)
        | Scalar::StrArray(count)
        | Scalar::StringArrayRef(count)
        | Scalar::StrArrayRef(count)
            if count < 0 =>
        {
            Err(invalid(function, "formatting array length must be nonnegative"))
        }
        Scalar::List(_, depth) | Scalar::ListRef(_, depth) | Scalar::ListMutRef(_, depth) if depth < 1 => {
            Err(invalid(function, "list depth must be positive"))
        }
        _ => Ok(()),
    }
}

/// Check a formatting array's exact length and each element against its declared destination type.
fn validate_array(plan: &Plan, function: &Function, values: &[Operand], expected: Scalar) -> Result<Scalar, PlanError> {
    let (element, count) = match expected {
        Scalar::StringArray(count) => (Scalar::String, count),
        Scalar::StrArray(count) => (Scalar::StrRef, count),
        _ => {
            return Err(invalid(
                function,
                "array expression requires a formatting-array destination",
            ));
        }
    };
    if usize::try_from(count).ok() != Some(values.len()) {
        return Err(invalid(function, "formatting array length differs from its elements"));
    }
    for value in values {
        require(
            function,
            operand(plan, function, value)?,
            element.clone(),
            "formatting array element",
        )?;
    }
    Ok(expected)
}

/// Sized integer arithmetic retains its carrier for legacy release wrapping; an explicit pair destination requests
/// checked arithmetic.
fn binary_result(function: &Function, op: &BinaryOp, ty: Scalar, expected: Scalar) -> Result<Scalar, PlanError> {
    if let Some(kind) = sized_numeric(&ty) {
        return match op {
            BinaryOp::Add | BinaryOp::Subtract | BinaryOp::Multiply if sized_integer(&ty) => {
                let checked = Scalar::CheckedNumeric(kind);
                Ok(if expected == checked { checked } else { ty })
            }
            BinaryOp::Add | BinaryOp::Subtract | BinaryOp::Multiply | BinaryOp::Divide
                if matches!(ty, Scalar::F32 | Scalar::F64) =>
            {
                Ok(ty)
            }
            BinaryOp::Equal
            | BinaryOp::NotEqual
            | BinaryOp::Less
            | BinaryOp::LessEqual
            | BinaryOp::Greater
            | BinaryOp::GreaterEqual => Ok(Scalar::Bool),
            _ => Err(invalid(function, "unsupported sized numeric operator")),
        };
    }
    match op {
        BinaryOp::Add | BinaryOp::Subtract | BinaryOp::Multiply if ty == Scalar::Int => Ok(Scalar::CheckedInt),
        BinaryOp::Add | BinaryOp::Subtract | BinaryOp::Multiply | BinaryOp::Divide if ty == Scalar::Float => {
            Ok(Scalar::Float)
        }
        BinaryOp::Equal | BinaryOp::NotEqual if matches!(ty, Scalar::Int | Scalar::Float | Scalar::Bool) => {
            Ok(Scalar::Bool)
        }
        BinaryOp::Less | BinaryOp::LessEqual | BinaryOp::Greater | BinaryOp::GreaterEqual
            if matches!(ty, Scalar::Int | Scalar::Float) =>
        {
            Ok(Scalar::Bool)
        }
        _ => Err(invalid(function, "invalid binary operand type")),
    }
}

/// Resolve a function reference against source-named declarations before invoking rustc.
fn signature(plan: &Plan, callee: &Callee) -> Result<(Vec<Scalar>, Scalar), PlanError> {
    span(&callee.span)?;
    match &callee.kind {
        CalleeKind::SpawnGenerator(name, leaf, depth) => {
            let producer = plan
                .functions
                .iter()
                .find(|function| &function.name == name)
                .ok_or_else(|| PlanError::UnknownCallee(name.clone()))?;
            if producer.parameters.is_empty()
                || scalar(&producer.parameters[0].ty) != Scalar::GeneratorYield(Leaf::of(leaf), *depth)
                || scalar(&producer.return_type) != Scalar::Unit
            {
                return Err(invalid(
                    producer,
                    "generator producer signature differs from yield handle contract",
                ));
            }
            Ok((
                producer.parameters[1..]
                    .iter()
                    .map(|parameter| scalar(&parameter.ty))
                    .collect(),
                Scalar::Generator(Leaf::of(leaf), *depth),
            ))
        }
        CalleeKind::YieldGenerator(leaf, depth) => Ok((
            vec![
                Scalar::GeneratorYieldRef(Leaf::of(leaf), *depth),
                generator_element(leaf, *depth),
            ],
            Scalar::Unit,
        )),
        CalleeKind::CollectGenerator(leaf, depth) => Ok((
            vec![Scalar::Generator(Leaf::of(leaf), *depth)],
            Scalar::List(Leaf::of(leaf), depth + 1),
        )),
        CalleeKind::Planned(name) => {
            let function = plan
                .functions
                .iter()
                .find(|function| &function.name == name)
                .ok_or_else(|| PlanError::UnknownCallee(name.clone()))?;
            Ok((
                function
                    .parameters
                    .iter()
                    .map(|parameter| scalar(&parameter.ty))
                    .collect(),
                scalar(&function.return_type),
            ))
        }
        CalleeKind::CloneEnum(index, name) => {
            let declaration = enum_declaration(plan, *index).ok_or_else(|| PlanError::UnknownCallee(name.clone()))?;
            if declaration.name != *name || !declaration.derives.iter().any(|derive| derive == "Clone") {
                return Err(PlanError::UnknownCallee(name.clone()));
            }
            Ok((vec![Scalar::EnumRef(*index)], Scalar::Enum(*index)))
        }
        CalleeKind::CloneModel(index, name) => {
            let declaration = model(plan, *index).ok_or_else(|| PlanError::UnknownCallee(name.clone()))?;
            if declaration.name != *name {
                return Err(PlanError::UnknownCallee(name.clone()));
            }
            Ok((vec![Scalar::ModelRef(*index)], Scalar::Model(*index)))
        }
        CalleeKind::External(path) | CalleeKind::Instantiated(path, _) | CalleeKind::InstantiatedPair(path, _, _) => {
            let external = plan
                .externals
                .iter()
                .find(|external| {
                    &external.path == path
                        && match &callee.kind {
                            CalleeKind::Instantiated(_, ty) => {
                                external.type_arguments.len() == 1 && scalar(&external.type_arguments[0]) == scalar(ty)
                            }
                            CalleeKind::InstantiatedPair(_, key, value) => external
                                .type_arguments
                                .iter()
                                .map(scalar)
                                .eq([scalar(key), scalar(value)]),
                            _ => external.type_arguments.is_empty(),
                        }
                })
                .ok_or_else(|| PlanError::UnknownCallee(path.clone()))?;
            Ok((
                external.parameters.iter().map(scalar).collect(),
                scalar(&external.return_type),
            ))
        }
    }
}

/// Compare the yielded element with the same primitive/list vocabulary used for ordinary values.
fn generator_element(leaf: &ListLeaf, depth: i64) -> Scalar {
    if depth != 0 {
        return Scalar::List(Leaf::of(leaf), depth);
    }
    scalar(&crate::plan::list_leaf_type(leaf.clone()))
}

/// Captured spawn is a constructor boundary, not an arbitrary closure operation in an ordinary planned body.
/// Its source-ordered owned parameters supply the entire environment exactly once.
fn validate_generator_constructor(function: &Function, arguments: &[Operand]) -> Result<(), PlanError> {
    if function.parameters.len() != arguments.len()
        || function.locals.len() != arguments.len() + 1
        || function.blocks.iter().any(|block| !block.statements.is_empty())
    {
        return Err(invalid(
            function,
            "captured generator spawn requires an exact constructor frame",
        ));
    }
    for (index, argument) in arguments.iter().enumerate() {
        let OperandKind::Move(place) = &argument.kind else {
            return Err(invalid(function, "generator capture must move its owned parameter"));
        };
        if usize::try_from(place.local).map_err(|_| invalid(function, "negative generator capture slot"))? != index + 1
            || !matches!(place.projection, Projection::Whole)
        {
            return Err(invalid(
                function,
                "generator capture order differs from constructor parameters",
            ));
        }
    }
    Ok(())
}

/// Validate terminator values and every normal and unwinding successor.
fn terminator(plan: &Plan, function: &Function, value: &TerminatorKind, cleanup: bool) -> Result<(), PlanError> {
    match value {
        TerminatorKind::Goto(target) => edge(function, *target, cleanup),
        TerminatorKind::SwitchBool(condition, false_target, true_target) => {
            require(
                function,
                operand(plan, function, condition)?,
                Scalar::Bool,
                "switch condition",
            )?;
            edge(function, *false_target, cleanup)?;
            edge(function, *true_target, cleanup)
        }
        TerminatorKind::Call(callee, arguments, destination, target, action) => {
            if matches!(callee.kind, CalleeKind::SpawnGenerator(..)) && !arguments.is_empty() {
                validate_generator_constructor(function, arguments)?;
            }
            let (parameters, result) = signature(plan, callee)?;
            if parameters.len() != arguments.len() {
                return Err(invalid(
                    function,
                    "call argument count differs from the callee signature",
                ));
            }
            for (argument, parameter) in arguments.iter().zip(parameters) {
                require(function, operand(plan, function, argument)?, parameter, "call argument")?;
            }
            require(
                function,
                place(plan, function, destination)?,
                result,
                "call destination",
            )?;
            edge(function, *target, cleanup)?;
            unwind(function, action, cleanup)
        }
        TerminatorKind::Drop(value, target, action) => {
            place(plan, function, value)?;
            edge(function, *target, cleanup)?;
            unwind(function, action, cleanup)
        }
        TerminatorKind::AssertOverflow(condition, op, left, right, target, action) => {
            require(
                function,
                operand(plan, function, condition)?,
                Scalar::Bool,
                "overflow flag",
            )?;
            let left_type = operand(plan, function, left)?;
            if left_type != Scalar::Int && !sized_integer(&left_type) {
                return Err(invalid(function, "overflow assertion requires an integer carrier"));
            }
            require(
                function,
                operand(plan, function, right)?,
                left_type,
                "overflow right operand",
            )?;
            if !matches!(op, BinaryOp::Add | BinaryOp::Subtract | BinaryOp::Multiply) {
                return Err(invalid(
                    function,
                    "overflow assertion requires add, subtract, or multiply",
                ));
            }
            edge(function, *target, cleanup)?;
            unwind(function, action, cleanup)
        }
        TerminatorKind::Return if cleanup => Err(invalid(function, "cleanup block cannot return normally")),
        TerminatorKind::UnwindResume if !cleanup => Err(invalid(function, "unwind resume requires a cleanup block")),
        TerminatorKind::Return | TerminatorKind::Unreachable | TerminatorKind::UnwindResume => Ok(()),
    }
}

/// Resolve a checked nominal index without signed conversions or ambient name lookup.
fn model(plan: &Plan, index: i64) -> Option<&crate::plan::ModelDeclaration> {
    usize::try_from(index).ok().and_then(|index| plan.models.get(index))
}

/// Require flat scalar/text tuple layouts and consistent nominal declaration identities.
fn validate_model_type(plan: &Plan, ty: &PlanType) -> Result<(), PlanError> {
    if matches!(ty, PlanType::Tuple(_)) && !source_signature_type(scalar(ty)) {
        return Err(PlanError::Invalid {
            function: "tuple".into(),
            reason: "tuple elements must be admitted scalars or owned text".into(),
        });
    }
    if let PlanType::Enum(index, name) | PlanType::EnumRef(index, name) = ty {
        if enum_declaration(plan, *index).is_none_or(|declaration| declaration.name != *name) {
            return Err(PlanError::Invalid {
                function: name.clone(),
                reason: "enum type differs from its declaration".into(),
            });
        }
    }
    if let PlanType::CheckedNumeric(id) = ty {
        if matches!(id, SizedNumeric::F32 | SizedNumeric::F64) {
            return Err(PlanError::Invalid {
                function: "checked numeric type".into(),
                reason: "overflow pairs require an admitted sized integer carrier".into(),
            });
        }
    }
    if let PlanType::Model(index, name) | PlanType::ModelRef(index, name) | PlanType::ModelMutRef(index, name) = ty {
        if model(plan, *index).is_none_or(|declaration| declaration.name != *name) {
            return Err(PlanError::Invalid {
                function: name.clone(),
                reason: "nominal type differs from its declaration".into(),
            });
        }
    }
    Ok(())
}

/// Validate named model layouts and single-slot newtypes, rejecting cyclic layouts before invoking rustc.
fn validate_models(plan: &Plan) -> Result<(), PlanError> {
    let mut names: BTreeSet<_> = plan.functions.iter().map(|function| &function.name).collect();
    for (index, declaration) in plan.models.iter().enumerate() {
        span(&declaration.span)?;
        let tuple = declaration.fields.len() == 1 && declaration.fields[0].name == "0";
        let derives_valid = if tuple {
            declaration.derives == ["Debug", "Clone"] || declaration.derives == ["Debug", "Clone", "Copy"]
        } else {
            ["Debug", "Clone", "FieldInfo", "IncanClass"].iter().all(|required| declaration.derives.iter().any(|derive| derive == required))
                && declaration.derives.iter().all(|derive| matches!(derive.as_str(), "Debug" | "Clone" | "FieldInfo" | "IncanClass" | "Eq" | "PartialEq" | "Hash" | "Ord" | "PartialOrd" | "Default" | "Display" | "serde::Serialize" | "serde::Deserialize"))
                && declaration.derives.iter().collect::<BTreeSet<_>>().len() == declaration.derives.len()
        };
        if !identifier(&declaration.name)
            || !names.insert(&declaration.name)
            || declaration.fields.len() != declaration.field_public.len()
            || !derives_valid
        {
            return Err(PlanError::Invalid {
                function: declaration.name.clone(),
                reason: "invalid plain model declaration".into(),
            });
        }
        let mut fields = BTreeSet::new();
        for field in &declaration.fields {
            span(&field.span)?;
            if (!tuple && !identifier(&field.name))
                || !fields.insert(&field.name)
                || !source_signature_type(scalar(&field.ty))
            {
                return Err(PlanError::Invalid {
                    function: declaration.name.clone(),
                    reason: "invalid model field declaration".into(),
                });
            }
            validate_model_type(plan, &field.ty)?;
            if let PlanType::Model(owner, _) = &field.ty {
                if usize::try_from(*owner).ok().is_some_and(|owner| owner >= index) {
                    return Err(PlanError::Invalid {
                        function: declaration.name.clone(),
                        reason: "unsupported recursive or forward model field".into(),
                    });
                }
            }
        }
    }
    Ok(())
}

/// Identify named sized carriers; ordinary scalars have no sized identity.
fn sized_numeric(ty: &Scalar) -> Option<Numeric> {
    match ty {
        Scalar::I8 => Some(Numeric::I8),
        Scalar::I16 => Some(Numeric::I16),
        Scalar::I32 => Some(Numeric::I32),
        Scalar::I128 => Some(Numeric::I128),
        Scalar::U8 => Some(Numeric::U8),
        Scalar::U16 => Some(Numeric::U16),
        Scalar::U32 => Some(Numeric::U32),
        Scalar::U64 => Some(Numeric::U64),
        Scalar::U128 => Some(Numeric::U128),
        Scalar::F32 => Some(Numeric::F32),
        Scalar::F64 => Some(Numeric::F64),
        Scalar::ISize => Some(Numeric::ISize),
        Scalar::USize => Some(Numeric::USize),
        _ => None,
    }
}

/// Sized integer carriers can produce a checked arithmetic pair; floats cannot.
fn sized_integer(ty: &Scalar) -> bool {
    sized_numeric(&ty).is_some() && !matches!(ty, Scalar::F32 | Scalar::F64)
}

/// Resolve a source enum layout by its validated plan index.
fn enum_declaration(plan: &Plan, index: i64) -> Option<&crate::plan::EnumDeclaration> {
    usize::try_from(index).ok().and_then(|index| plan.enums.get(index))
}

/// Reject malformed layouts, unsupported derives and forward or recursive payload layouts before rustc runs.
fn validate_enums(plan: &Plan) -> Result<(), PlanError> {
    let mut names: BTreeSet<_> = plan
        .functions
        .iter()
        .map(|value| &value.name)
        .chain(plan.models.iter().map(|value| &value.name))
        .collect();
    for (index, declaration) in plan.enums.iter().enumerate() {
        span(&declaration.span)?;
        let error = || PlanError::Invalid {
            function: declaration.name.clone(),
            reason: "invalid or unsupported enum declaration".into(),
        };
        if !declaration.carrier.is_empty() {
            let variants = &declaration.variants;
            let valid = match declaration.carrier.as_str() {
                "Option" => {
                    variants.len() == 2
                        && variants[0].name == "None"
                        && variants[0].fields.is_empty()
                        && variants[1].name == "Some"
                        && variants[1].fields.len() == 1
                }
                "Result" => {
                    variants.len() == 2
                        && variants[0].name == "Ok"
                        && variants[0].fields.len() == 1
                        && variants[1].name == "Err"
                        && variants[1].fields.len() == 1
                }
                _ => false,
            };
            if !valid || declaration.source_type.is_empty() || !declaration.name.starts_with("__IncanCarrier") {
                return Err(error());
            }
        }
        if !identifier(&declaration.name)
            || !names.insert(&declaration.name)
            || declaration.variants.is_empty()
            || declaration.derives.iter().any(|name| {
                !matches!(
                    name.as_str(),
                    "Debug" | "Clone" | "Copy" | "PartialEq" | "Eq" | "PartialOrd" | "Ord" | "Hash"
                )
            })
        {
            return Err(error());
        }
        let mut variants = BTreeSet::new();
        for variant in &declaration.variants {
            if !identifier(&variant.name) || !variants.insert(&variant.name) {
                return Err(error());
            }
            for field in &variant.fields {
                validate_model_type(plan, field)?;
                if !source_signature_type(scalar(field)) {
                    return Err(error());
                }
                if let PlanType::Enum(owner, _) = field {
                    if usize::try_from(*owner).ok().is_none_or(|owner| owner >= index) {
                        return Err(error());
                    }
                }
            }
        }
    }
    Ok(())
}
