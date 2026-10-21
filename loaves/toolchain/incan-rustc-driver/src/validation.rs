//! Preflight validation of the Incan-owned scalar plan. No rustc type crosses this module.

use std::collections::BTreeSet;

use crate::error::PlanError;
use crate::plan::{
    BinaryOp, Callee, CalleeKind, Constant, Function, GenericLayoutType, ListLeaf, Operand, OperandKind, Place, Plan,
    PlanType, Projection, RvalueKind, SizedNumeric, SourceSpan, StatementKind, TerminatorKind, UnaryOp, Unwind,
    list_leaf_type, range_field_types, tuple_element_type,
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
    NativeStr,
    NativeSlice(Vec<Scalar>),
    SliceRef(Vec<Scalar>),
    SliceMutRef(Vec<Scalar>),
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
    EnumMutRef(i64),
    CheckedInt,
    Tuple(Vec<Scalar>),
    TupleMutRef(Vec<Scalar>),
    String,
    StringRef,
    ScalarRef(Vec<Scalar>),
    ScalarMutRef(Vec<Scalar>),
    StringMutRef,
    Decimal,
    FrozenStr,
    FrozenBytes,
    FrozenCollection(String, Vec<Scalar>),
    FrozenCollectionRef(String, Vec<Scalar>),
    TypeToken(Vec<Scalar>),
    StaticText,
    StaticBytes,
    ValidationError,
    ValidationErrorRef,
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
    ListIntoIter(Leaf, i64),
    ListIntoIterMutRef(Leaf, i64),
    Set(Leaf),
    SetRef(Leaf),
    SetMutRef(Leaf),
    Dict(Leaf, Leaf),
    DictRef(Leaf, Leaf),
    DictMutRef(Leaf, Leaf),
    RangeValue,
    ZipIterator(Leaf, Leaf),
    ZipIteratorRef(Leaf, Leaf),
    ZipIteratorMutRef(Leaf, Leaf),
    Generator(Leaf, i64),
    GeneratorMutRef(Leaf, i64),
    GeneratorYield(Leaf, i64),
    GeneratorYieldRef(Leaf, i64),
    FunctionPointer(Vec<Scalar>),
    FunctionItem(String, Vec<Scalar>),
    Closure(Vec<Scalar>),
    SharedCallable(Vec<Scalar>),
    ClosureOnce(Vec<Scalar>),
    ClosureRef(Vec<Scalar>),
    Future(Vec<Scalar>),
}

/// Comparison mirror of scalar and tuple leaves; the public plan remains Incan-authored.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Leaf {
    FunctionPointer(Vec<Scalar>),
    Int,
    Float,
    Bool,
    Str,
    Tuple(Vec<Scalar>),
    Model(i64),
    Enum(i64),
    U8,
    USize,
    Unit,
    Decimal,
    FrozenStr,
    FrozenBytes,
    /// An owned nested list preserves every supplied leaf until canonical arity validation.
    List(Vec<Leaf>, i64),
    /// An owned nested dictionary preserves key/value arity instead of indexing an unchecked vector.
    Dict(Vec<Leaf>),
}

impl Leaf {
    /// Mirror one plan leaf.
    fn of(leaf: &ListLeaf) -> Self {
        match leaf {
            ListLeaf::Int => Leaf::Int,
            ListLeaf::Float => Leaf::Float,
            ListLeaf::Bool => Leaf::Bool,
            ListLeaf::Str => Leaf::Str,
            ListLeaf::Tuple(elements) => Leaf::Tuple(
                elements
                    .iter()
                    .map(|element| scalar(&tuple_element_type(element.clone())))
                    .collect(),
            ),
            ListLeaf::Model(index, _) => Leaf::Model(*index),
            ListLeaf::Enum(index, _) => Leaf::Enum(*index),
            ListLeaf::U8 => Leaf::U8,
            ListLeaf::USize => Leaf::USize,
            ListLeaf::Unit => Leaf::Unit,
            ListLeaf::Decimal => Leaf::Decimal,
            ListLeaf::FrozenStr => Leaf::FrozenStr,
            ListLeaf::FrozenBytes => Leaf::FrozenBytes,
            ListLeaf::FunctionPointer(signature) => Leaf::FunctionPointer(signature.iter().map(scalar).collect()),
            ListLeaf::List(elements, depth) => Leaf::List(elements.iter().map(Leaf::of).collect(), *depth),
            ListLeaf::Dict(elements) => Leaf::Dict(elements.iter().map(Leaf::of).collect()),
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
        PlanType::NativeStr => Scalar::NativeStr,
        PlanType::NativeSlice(elements) => Scalar::NativeSlice(elements.iter().map(scalar).collect()),
        PlanType::SliceRef(elements) => Scalar::SliceRef(elements.iter().map(scalar).collect()),
        PlanType::SliceMutRef(elements) => Scalar::SliceMutRef(elements.iter().map(scalar).collect()),
        PlanType::ScalarRef(elements) => Scalar::ScalarRef(elements.iter().map(scalar).collect()),
        PlanType::ScalarMutRef(elements) => Scalar::ScalarMutRef(elements.iter().map(scalar).collect()),
        PlanType::Future(outputs) => Scalar::Future(outputs.iter().map(scalar).collect()),
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
        PlanType::EnumMutRef(index, _) => Scalar::EnumMutRef(*index),
        PlanType::Model(index, _) => Scalar::Model(*index),
        PlanType::ModelMutRef(index, _) => Scalar::ModelMutRef(*index),
        PlanType::ModelRef(index, _) => Scalar::ModelRef(*index),
        PlanType::List(leaf, depth) => Scalar::List(Leaf::of(leaf), *depth),
        PlanType::ListRef(leaf, depth) => Scalar::ListRef(Leaf::of(leaf), *depth),
        PlanType::ListMutRef(leaf, depth) => Scalar::ListMutRef(Leaf::of(leaf), *depth),
        PlanType::ListIntoIter(leaf, depth) => Scalar::ListIntoIter(Leaf::of(leaf), *depth),
        PlanType::ListIntoIterMutRef(leaf, depth) => Scalar::ListIntoIterMutRef(Leaf::of(leaf), *depth),
        PlanType::Set(leaf) => Scalar::Set(Leaf::of(leaf)),
        PlanType::SetRef(leaf) => Scalar::SetRef(Leaf::of(leaf)),
        PlanType::SetMutRef(leaf) => Scalar::SetMutRef(Leaf::of(leaf)),
        PlanType::Dict(key, value) => Scalar::Dict(Leaf::of(key), Leaf::of(value)),
        PlanType::DictRef(key, value) => Scalar::DictRef(Leaf::of(key), Leaf::of(value)),
        PlanType::DictMutRef(key, value) => Scalar::DictMutRef(Leaf::of(key), Leaf::of(value)),
        PlanType::RangeValue => Scalar::RangeValue,
        PlanType::CallableArguments(inputs) => Scalar::Tuple(inputs.iter().map(scalar).collect()),
        PlanType::Tuple(elements) => Scalar::Tuple(
            elements
                .iter()
                .map(|element| scalar(&tuple_element_type(element.clone())))
                .collect(),
        ),
        PlanType::TupleMutRef(elements) => Scalar::TupleMutRef(
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
        PlanType::ValidationError => Scalar::ValidationError,
        PlanType::ValidationErrorRef => Scalar::ValidationErrorRef,
        PlanType::FrozenStr => Scalar::FrozenStr,
        PlanType::FrozenBytes => Scalar::FrozenBytes,
        PlanType::FrozenCollection(name, elements) => {
            Scalar::FrozenCollection(name.clone(), elements.iter().map(scalar).collect())
        }
        PlanType::FrozenCollectionRef(name, elements) => {
            Scalar::FrozenCollectionRef(name.clone(), elements.iter().map(scalar).collect())
        }
        PlanType::TypeToken(elements) => Scalar::TypeToken(elements.iter().map(scalar).collect()),
        PlanType::StaticBytes => Scalar::StaticBytes,
        PlanType::StaticText => Scalar::StaticText,
        PlanType::StringRef => Scalar::StringRef,
        PlanType::StringMutRef => Scalar::StringMutRef,
        PlanType::StrRef => Scalar::StrRef,
        PlanType::StringArray(count) => Scalar::StringArray(*count),
        PlanType::StrArray(count) => Scalar::StrArray(*count),
        PlanType::StringArrayRef(count) => Scalar::StringArrayRef(*count),
        PlanType::StrArrayRef(count) => Scalar::StrArrayRef(*count),
        PlanType::StringSlice => Scalar::StringSlice,
        PlanType::StrSlice => Scalar::StrSlice,
        PlanType::FunctionPointer(signature) => Scalar::FunctionPointer(signature.iter().map(scalar).collect()),
        PlanType::Closure(signature) => Scalar::Closure(signature.iter().map(scalar).collect()),
        PlanType::SharedCallable(signature) => Scalar::SharedCallable(signature.iter().map(scalar).collect()),
        PlanType::ClosureOnce(signature) => Scalar::ClosureOnce(signature.iter().map(scalar).collect()),
        PlanType::ClosureRef(signature) => Scalar::ClosureRef(signature.iter().map(scalar).collect()),
        PlanType::FunctionItem(name, signature) => {
            Scalar::FunctionItem(name.clone(), signature.iter().map(scalar).collect())
        }
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
pub fn validate(plan: &Plan, library: bool) -> Result<(), PlanError> {
    span(&plan.span)?;
    let mut names = BTreeSet::new();
    for function in &plan.functions {
        if !identifier(&function.name) || !names.insert(&function.name) {
            return Err(invalid(function, "function name is invalid or duplicated"));
        }
    }
    validate_models(plan)?;
    validate_enums(plan)?;
    validate_generic_models(plan)?;
    library_model_order(plan)?;
    crate::checked_json::validate_roots(plan)?;
    crate::checked_json::validate_source_roots(plan)?;
    validate_value_layouts(plan)?;
    validate_statics(plan)?;
    let mut paths = BTreeSet::new();
    for external in &plan.externals {
        span(&external.span)?;
        for ty in external
            .parameters
            .iter()
            .chain(std::iter::once(&external.return_type))
            .chain(&external.type_arguments)
        {
            validate_model_type(plan, ty)?;
        }
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
            .any(|ty| !external_signature_type(&scalar(ty)))
            || !external_signature_type(&scalar(&external.return_type))
        {
            return Err(PlanError::Invalid {
                function: external.path.clone(),
                reason: "checked pairs and formatting arrays cannot cross a function signature".into(),
            });
        }
    }
    // A library unit has no entry point; an executable must have exactly the binary main Rust accepts.
    if !library {
        let main = plan
            .functions
            .iter()
            .find(|function| function.name == "main")
            .ok_or_else(|| PlanError::UnknownCallee("main".into()))?;
        if !main.parameters.is_empty()
            || (scalar(&main.return_type) != Scalar::Unit && !entry_result(plan, &main.return_type))
        {
            return Err(invalid(
                main,
                "native binary main must take no parameters and return unit or a unit Result",
            ));
        }
    }
    for function in &plan.functions {
        validate_closure_constructor(function)?;
        crate::shared_callables::validate(function)?;
        validate_function(plan, function)?;
    }
    Ok(())
}

/// Whether a binary main's result is a standard `Result` with a unit success and a text or SDK-owned error.
///
/// Rust's entry point reports such a result itself, printing an error through its `Debug` and failing the exit code,
/// which is what legacy's generated main relies on; both admitted error kinds implement `Debug`.
fn entry_result(plan: &Plan, ty: &PlanType) -> bool {
    let PlanType::Enum(index, _) = ty else {
        return false;
    };
    let Some(declaration) = enum_declaration(plan, *index) else {
        return false;
    };
    let [success, failure] = declaration.variants.as_slice() else {
        return false;
    };
    let unit_success = success.fields.is_empty() || success.fields.as_slice() == [PlanType::Unit];
    declaration.carrier == "Result"
        && unit_success
        && match failure.fields.as_slice() {
            [PlanType::String] => true,
            [PlanType::Model(index, _)] => model(plan, *index).is_some_and(|model| !model.library_item.is_empty()),
            [PlanType::Enum(index, _)] => {
                enum_declaration(plan, *index).is_some_and(|value| !value.library_item.is_empty())
            }
            _ => false,
        }
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
    if std::iter::once(&function.return_type)
        .chain(function.parameters.iter().map(|parameter| &parameter.ty))
        .any(|ty| borrowed_alias_in_source_type(plan, ty, &mut BTreeSet::new()))
    {
        return Err(invalid(
            function,
            "native borrowed carrier in a source signature requires a retained lifetime contract",
        ));
    }
    validate_future_constructor(plan, function)?;
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
        || !source_signature_type(&scalar(&function.return_type))
    {
        return Err(invalid(function, "return local must match a scalar return type"));
    }
    let mut parameters = BTreeSet::new();
    for (index, parameter) in function.parameters.iter().enumerate() {
        span(&parameter.span)?;
        if !identifier(&parameter.name) || !parameters.insert(&parameter.name) {
            return Err(invalid(function, "parameter name is invalid or duplicated"));
        }
        if !(source_signature_type(&scalar(&parameter.ty))
            || matches!(
                scalar(&parameter.ty),
                Scalar::ModelRef(_) | Scalar::ModelMutRef(_) | Scalar::EnumRef(_)
            ))
            || scalar(&parameter.ty) != scalar(&function.locals[index + 1].ty)
        {
            return Err(invalid(function, "parameter local differs from its declaration"));
        }
    }
    for local in &function.locals {
        if matches!(local.ty, PlanType::NativeStr | PlanType::NativeSlice(_)) {
            return Err(invalid(function, "unsized native referent cannot occupy a value frame"));
        }
        span(&local.span)?;
        validate_model_type(plan, &local.ty)?;
        validate_array_length(plan, function, scalar(&local.ty))?;
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
                    require(function, result, expected, "assignment")?;
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

/// A boxing shim retains an exact parameter frame and a proven source coroutine or canonical native callee.
fn validate_future_constructor(plan: &Plan, function: &Function) -> Result<(), PlanError> {
    let Some(callee) = &function.future_callee else {
        return Ok(());
    };
    let PlanType::Future(outputs) = &function.return_type else {
        return Err(invalid(function, "unsupported future constructor return carrier"));
    };
    let [output] = outputs.as_slice() else {
        return Err(invalid(function, "unsupported future constructor output arity"));
    };
    if crate::futures::borrowed_captures(function) > 1 {
        return Err(invalid(
            function,
            "unsupported future constructor with multiple borrowed captures",
        ));
    }
    if function.is_async
        || function.locals.len() != function.parameters.len() + 1
        || function.blocks.len() != 1
        || !function.blocks[0].statements.is_empty()
        || !matches!(function.blocks[0].terminator.kind, TerminatorKind::Unreachable)
    {
        return Err(invalid(function, "unsupported future boxing constructor frame"));
    }
    match &callee.kind {
        CalleeKind::Planned(name) => {
            let target = plan
                .functions
                .iter()
                .find(|target| target.name == *name)
                .ok_or_else(|| invalid(function, "unsupported missing future source declaration"))?;
            if !target.is_async
                || target.return_type != *output
                || target.parameters.len() != function.parameters.len()
                || target
                    .parameters
                    .iter()
                    .zip(&function.parameters)
                    .any(|(target, parameter)| target.ty != parameter.ty)
            {
                return Err(invalid(
                    function,
                    "unsupported future source signature differs from its constructor",
                ));
            }
        }
        CalleeKind::External(path) | CalleeKind::Instantiated(path, _) | CalleeKind::FutureMethod(path, _) => {
            if path.split("::").count() < 2 || !path.split("::").all(identifier) {
                return Err(invalid(function, "unsupported future native callee path"));
            }
            if let CalleeKind::Instantiated(_, arguments) | CalleeKind::FutureMethod(_, arguments) = &callee.kind {
                for argument in arguments {
                    validate_model_type(plan, argument)?;
                }
            }
            if matches!(callee.kind, CalleeKind::FutureMethod(..))
                && (path.split("::").count() < 3 || crate::futures::borrowed_captures(function) != 1)
            {
                return Err(invalid(
                    function,
                    "unsupported SDK method future without an exact receiver",
                ));
            }
        }
        _ => {
            return Err(invalid(function, "unsupported future constructor callee kind"));
        }
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

/// Require a field path's root to have its explicit owned shape or a nominal receiver reference to that shape.
/// Shared enum receivers permit payload reads; mutable reborrows remain guarded by `borrow_result`.
fn field_owner(function: &Function, ty: Scalar, dereference: bool) -> Result<Scalar, PlanError> {
    if !dereference {
        return Ok(ty);
    }
    match ty {
        Scalar::ModelRef(owner) | Scalar::ModelMutRef(owner) => Ok(Scalar::Model(owner)),
        Scalar::EnumRef(owner) | Scalar::EnumMutRef(owner) => Ok(Scalar::Enum(owner)),
        Scalar::TupleMutRef(elements) => Ok(Scalar::Tuple(elements)),
        _ => Err(invalid(
            function,
            "field dereference requires a nominal receiver reference",
        )),
    }
}

/// Validate one field against its retained tuple or nominal layout before allowing another path step.
fn projected_field(
    plan: &Plan,
    function: &Function,
    owner: Scalar,
    slot: i64,
    field_type: &PlanType,
) -> Result<Scalar, PlanError> {
    validate_model_type(plan, field_type)?;
    let expected = match owner {
        Scalar::Tuple(elements) => usize::try_from(slot)
            .ok()
            .and_then(|slot| elements.get(slot))
            .cloned()
            .ok_or_else(|| invalid(function, "unknown tuple field"))?,
        Scalar::Model(owner) => {
            let model = model(plan, owner).ok_or_else(|| invalid(function, "unknown model owner"))?;
            let field = usize::try_from(slot)
                .ok()
                .and_then(|slot| model.fields.get(slot))
                .ok_or_else(|| invalid(function, "unknown model field"))?;
            scalar(&field.ty)
        }
        Scalar::RangeValue => usize::try_from(slot)
            .ok()
            .and_then(|slot| range_field_types().get(slot).map(scalar))
            .ok_or_else(|| invalid(function, "unknown range field"))?,
        _ => {
            return Err(invalid(
                function,
                "field projection requires a matching nominal, tuple or range owner",
            ));
        }
    };
    require(function, scalar(field_type), &expected, "projected field type")?;
    Ok(expected)
}

/// Validate each enum downcast against the exact owner, active variant slot, and retained payload field type.
fn projected_variant_field(
    plan: &Plan,
    function: &Function,
    owner: Scalar,
    variant: i64,
    slot: i64,
    field_type: &PlanType,
) -> Result<Scalar, PlanError> {
    validate_model_type(plan, field_type)?;
    let Scalar::Enum(owner) = owner else {
        return Err(invalid(function, "variant projection requires an enum owner"));
    };
    let declaration = enum_declaration(plan, owner).ok_or_else(|| invalid(function, "unknown enum owner"))?;
    let field = usize::try_from(variant)
        .ok()
        .and_then(|index| declaration.variants.get(index))
        .and_then(|variant| usize::try_from(slot).ok().and_then(|index| variant.fields.get(index)))
        .ok_or_else(|| invalid(function, "unknown enum payload field"))?;
    require(function, scalar(field_type), scalar(field), "enum payload projection")?;
    Ok(scalar(field))
}

/// Resolve checked arithmetic, list dereferences, and nominal projections, verifying reference shape and canonical
/// pointee or field type.
fn place(plan: &Plan, function: &Function, value: &Place) -> Result<Scalar, PlanError> {
    span(&value.span)?;
    let ty = local(function, value.local)?;
    match &value.projection {
        Projection::Whole => Ok(ty),
        Projection::VariantField(variant, slot, field_type) => {
            validate_model_type(plan, field_type)?;
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
                Scalar::SliceRef(elements) | Scalar::SliceMutRef(elements) => Scalar::NativeSlice(elements),
                Scalar::ScalarRef(elements) | Scalar::ScalarMutRef(elements) => {
                    let [inner] = elements.as_slice() else {
                        return Err(invalid(function, "unsupported scalar reference arity"));
                    };
                    inner.clone()
                }
                Scalar::ValidationErrorRef => Scalar::ValidationError,
                Scalar::FrozenCollectionRef(name, elements) => Scalar::FrozenCollection(name, elements),
                Scalar::ModelRef(owner) | Scalar::ModelMutRef(owner) => Scalar::Model(owner),
                Scalar::EnumRef(owner) => Scalar::Enum(owner),
                Scalar::ZipIteratorRef(left, right) | Scalar::ZipIteratorMutRef(left, right) => {
                    Scalar::ZipIterator(left, right)
                }
                Scalar::GeneratorMutRef(leaf, depth) => Scalar::Generator(leaf, depth),
                Scalar::ListIntoIterMutRef(leaf, depth) => Scalar::ListIntoIter(leaf, depth),
                Scalar::StringMutRef => Scalar::String,
                Scalar::EnumMutRef(owner) => Scalar::Enum(owner),
                Scalar::TupleMutRef(elements) => Scalar::Tuple(elements),
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
            let owner = field_owner(function, ty, matches!(value.projection, Projection::DerefField(..)))?;
            projected_field(plan, function, owner, *slot, field_type)
        }
        Projection::Fields(fields) | Projection::DerefFields(fields) => {
            if fields.is_empty() {
                return Err(invalid(function, "field path must retain at least one field"));
            }
            let mut owner = field_owner(function, ty, matches!(value.projection, Projection::DerefFields(_)))?;
            for field in fields {
                owner = match field.variant {
                    -1 => projected_field(plan, function, owner, field.slot, &field.ty)?,
                    variant if variant >= 0 => {
                        projected_variant_field(plan, function, owner, variant, field.slot, &field.ty)?
                    }
                    _ => return Err(invalid(function, "invalid field path variant")),
                };
            }
            Ok(owner)
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
                        | Scalar::ListIntoIter(_, _)
                        | Scalar::ListIntoIterMutRef(_, _)
                        | Scalar::GeneratorYield(_, _)
                        | Scalar::StringArray(_)
                        | Scalar::Model(_)
                        | Scalar::StringMutRef
                        | Scalar::SliceMutRef(_)
                        | Scalar::EnumMutRef(_)
                        | Scalar::TupleMutRef(_)
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
            Constant::RustItem(_, ty) => scalar(ty),
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
        Scalar::ScalarMutRef(_) | Scalar::SliceMutRef(_) | Scalar::ModelMutRef(_) => true,
        Scalar::String
        | Scalar::StringArray(_)
        | Scalar::Model(_)
        | Scalar::ClosureOnce(_)
        | Scalar::Closure(_)
        | Scalar::SharedCallable(_)
        | Scalar::ValidationError
        | Scalar::Future(_) => true,
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
        RvalueKind::FrozenText(value) => {
            if !matches!(value.kind, OperandKind::Literal(Constant::Text(_))) {
                return Err(invalid(function, "FrozenStr construction requires static literal text"));
            }
            require(
                function,
                operand(plan, function, value)?,
                Scalar::StrRef,
                "frozen literal",
            )?;
            Ok(Scalar::FrozenStr)
        }
        RvalueKind::FrozenBytes(values) => {
            if values.iter().any(|value| u8::try_from(*value).is_err()) {
                return Err(invalid(
                    function,
                    "FrozenBytes construction requires unsigned literal bytes",
                ));
            }
            Ok(Scalar::FrozenBytes)
        }
        RvalueKind::FrozenCollection(ty, values) => {
            validate_frozen_values(plan, function, ty, values)?;
            Ok(scalar(ty))
        }
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
        RvalueKind::FunctionItem(callee) => {
            let name = match &callee.kind {
                CalleeKind::Planned(name) | CalleeKind::External(name) => name,
                _ => return Err(invalid(function, "function item requires a retained declaration")),
            };
            let (mut parameters, result) = signature(plan, function, callee)?;
            parameters.push(result);
            Ok(Scalar::FunctionItem(name.clone(), parameters))
        }
        RvalueKind::ClosureObject(value) => match place(plan, function, value)? {
            Scalar::Closure(signature) if matches!(value.projection, Projection::Whole) => {
                Ok(Scalar::ClosureRef(signature))
            }
            _ => Err(invalid(
                function,
                "callable-object borrow requires a whole owned closure",
            )),
        },
        RvalueKind::ReifyFunction(value) => {
            let Scalar::FunctionItem(_, signature) = operand(plan, function, value)? else {
                return Err(invalid(function, "function-pointer coercion requires a function item"));
            };
            Ok(Scalar::FunctionPointer(signature))
        }
        RvalueKind::IntToFloat(value) => {
            require(function, operand(plan, function, value)?, &Scalar::Int, "int-to-float")?;
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
                UnaryOp::Invert if integer(&ty) => Ok(ty),
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
            let count = operand(plan, function, right)?;
            if matches!(op, BinaryOp::ShiftLeft | BinaryOp::ShiftRight) {
                return shift_result(function, ty, &count);
            }
            require(function, count, ty.clone(), "binary operands")?;
            binary_result(function, op, ty, expected)
        }
        RvalueKind::Array(elements) => validate_array(plan, function, elements, &expected),
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
        RvalueKind::Range(elements) => {
            require(function, expected, Scalar::RangeValue, "range aggregate destination")?;
            let fields = range_field_types();
            if elements.len() != fields.len() {
                return Err(invalid(function, "range aggregate requires every declared range field"));
            }
            for (element, field) in elements.iter().zip(&fields) {
                require(
                    function,
                    operand(plan, function, element)?,
                    &scalar(field),
                    "range field",
                )?;
            }
            Ok(Scalar::RangeValue)
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
                    &scalar(&field.ty),
                    "model field",
                )?;
            }
            Ok(Scalar::Model(*index))
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
            Scalar::Tuple(elements) => {
                if shared_reference(&local(function, value.local)?) {
                    return Err(invalid(
                        function,
                        "cannot mutably borrow a tuple through a shared reference",
                    ));
                }
                Ok(Scalar::TupleMutRef(elements))
            }
            Scalar::Generator(leaf, depth) => Ok(Scalar::GeneratorMutRef(leaf, depth)),
            Scalar::ListIntoIter(leaf, depth) => Ok(Scalar::ListIntoIterMutRef(leaf, depth)),
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
            Scalar::String => {
                if shared_reference(&local(function, value.local)?) {
                    return Err(invalid(
                        function,
                        "cannot mutably borrow text through a shared reference",
                    ));
                }
                Ok(Scalar::StringMutRef)
            }
            Scalar::Enum(index) => {
                if shared_reference(&local(function, value.local)?) {
                    return Err(invalid(
                        function,
                        "cannot mutably borrow an enum through a shared reference",
                    ));
                }
                Ok(Scalar::EnumMutRef(index))
            }
            ty if scalar_referent(&ty) => {
                if shared_reference(&local(function, value.local)?) {
                    return Err(invalid(function, "cannot mutably reborrow a shared scalar"));
                }
                Ok(Scalar::ScalarMutRef(vec![ty]))
            }
            Scalar::NativeSlice(elements) => {
                if shared_reference(&local(function, value.local)?) {
                    return Err(invalid(function, "cannot mutably reborrow a shared slice"));
                }
                Ok(Scalar::SliceMutRef(elements))
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
        Scalar::NativeSlice(elements) => Ok(Scalar::SliceRef(elements)),
        Scalar::ValidationError => Ok(Scalar::ValidationErrorRef),
        Scalar::FrozenCollection(name, elements) => Ok(Scalar::FrozenCollectionRef(name, elements)),
        Scalar::StringArray(count) => Ok(Scalar::StringArrayRef(count)),
        Scalar::StrArray(count) => Ok(Scalar::StrArrayRef(count)),
        ty if scalar_referent(&ty) => Ok(Scalar::ScalarRef(vec![ty])),
        _ => Err(invalid(function, "shared borrow requires an owned formatting value")),
    }
}

/// Report whether a local only lends its referent for reading, so nothing reached through it may be borrowed mutably.
fn shared_reference(ty: &Scalar) -> bool {
    matches!(
        ty,
        Scalar::StringRef
            | Scalar::SliceRef(_)
            | Scalar::ScalarRef(_)
            | Scalar::FrozenCollectionRef(_, _)
            | Scalar::ValidationErrorRef
            | Scalar::StrRef
            | Scalar::EnumRef(_)
            | Scalar::ModelRef(_)
            | Scalar::ListRef(_, _)
            | Scalar::SetRef(_)
            | Scalar::DictRef(_, _)
            | Scalar::ZipIteratorRef(_, _)
            | Scalar::GeneratorYieldRef(_, _)
            | Scalar::ClosureRef(_)
    )
}

/// Source signatures expose scalars, owned text, explicit shared text and validation-error references, and the
/// exclusive text reference a `mut str` parameter receives, models, callables, and tuples of admitted types; formatting
/// views stay internal. A function pointer's inputs also admit shared and exclusive model references retained by method
/// decorators, as receiver frames do.
fn source_signature_type(ty: &Scalar) -> bool {
    if let Scalar::TypeToken(elements) = ty {
        return matches!(
            elements.as_slice(),
            [Scalar::Int | Scalar::Float | Scalar::Bool | Scalar::String]
        );
    }

    if let Scalar::Future(outputs) = ty {
        return matches!(outputs.as_slice(), [output] if source_signature_type(output));
    }
    if let Scalar::TupleMutRef(elements) = ty {
        return source_signature_type(&Scalar::Tuple(elements.clone()));
    }
    if let Scalar::FunctionPointer(signature) = ty {
        let Some((result, inputs)) = signature.split_last() else {
            return false;
        };
        return source_signature_type(result)
            && inputs.iter().all(|input| {
                source_signature_type(input) || matches!(input, Scalar::ModelRef(_) | Scalar::ModelMutRef(_))
            });
    }
    if let Scalar::Closure(signature) | Scalar::ClosureOnce(signature) | Scalar::ClosureRef(signature) = ty {
        return callable_object_signature(signature);
    }
    if let Scalar::SharedCallable(signature) = ty {
        return signature.len() == 2 && callable_object_signature(signature);
    }
    if let Scalar::Tuple(elements) = ty {
        return elements.iter().all(|element| {
            matches!(
                element,
                Scalar::Int
                    | Scalar::Float
                    | Scalar::Bool
                    | Scalar::Unit
                    | Scalar::String
                    | Scalar::FrozenStr
                    | Scalar::List(_, _)
                    | Scalar::Model(_)
                    | Scalar::Enum(_)
            ) || (matches!(
                element,
                Scalar::ScalarRef(_)
                    | Scalar::ScalarMutRef(_)
                    | Scalar::ModelRef(_)
                    | Scalar::ModelMutRef(_)
                    | Scalar::SliceRef(_)
                    | Scalar::SliceMutRef(_)
            ) && external_signature_type(element))
                || (matches!(element, Scalar::Tuple(_)) && source_signature_type(element))
                || (matches!(element, Scalar::FunctionPointer(_)) && source_signature_type(element))
        });
    }
    sized_numeric(ty).is_some()
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
                | Scalar::StringRef
                | Scalar::StringMutRef
                | Scalar::EnumMutRef(_)
                | Scalar::Decimal
                | Scalar::FrozenStr
                | Scalar::FrozenBytes
                | Scalar::FrozenCollection(_, _)
                | Scalar::ValidationError
                | Scalar::ValidationErrorRef
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

/// Runtime signatures additionally admit exact scalar references and shared text and slice views, whose regions
/// metadata checking erases, and the exclusive model and enum references a checked element reborrow returns.
/// Concrete owned list iterators and their polling references cross only these native trait-call boundaries.
fn external_signature_type(ty: &Scalar) -> bool {
    if let Scalar::SliceRef(elements) | Scalar::SliceMutRef(elements) = ty {
        return matches!(elements.as_slice(), [element] if source_signature_type(element));
    }
    if let Scalar::ScalarRef(elements) | Scalar::ScalarMutRef(elements) = ty {
        return matches!(elements.as_slice(), [inner] if scalar_referent(inner));
    }
    source_signature_type(ty)
        || matches!(
            ty,
            Scalar::ZipIteratorRef(_, _)
                | Scalar::ListIntoIter(_, _)
                | Scalar::ListIntoIterMutRef(_, _)
                | Scalar::ValidationErrorRef
                | Scalar::FrozenCollectionRef(_, _)
                | Scalar::StringRef
                | Scalar::StrRef
                | Scalar::StaticText
                | Scalar::StaticBytes
                | Scalar::StringSlice
                | Scalar::StrSlice
                | Scalar::EnumRef(_)
                | Scalar::ModelRef(_)
                | Scalar::ModelMutRef(_)
                | Scalar::EnumMutRef(_)
                | Scalar::GeneratorYieldRef(_, _)
        )
}

/// The scalar reference carrier only borrows exact numeric, Boolean, decimal or frozen primitive storage.
fn scalar_referent(ty: &Scalar) -> bool {
    sized_numeric(ty).is_some()
        || matches!(
            ty,
            Scalar::Int | Scalar::Float | Scalar::Bool | Scalar::Decimal | Scalar::FrozenStr | Scalar::FrozenBytes
        )
}

/// Callable objects accept scalars of any numeric carrier, text, owned models and enums, shared collection views and
/// the exclusive borrows a `mut` parameter receives as inputs; their result is a scalar, owned text, an owned list, set
/// or dictionary, an owned model, newtype or enum, or an owned generator, which a generator expression's capturing step
/// returns. Tuple inputs use the same recursively validated by-value layout as source function parameters, including
/// captured generator steps over enumerate and zip items. Collection results retain their full layout and transfer
/// ownership to the caller; borrowed collection results remain outside this signature contract.
/// Owned model inputs use the same recursively validated layout as ordinary by-value parameters; external metadata
/// still checks the exact native carrier and trait bounds.
fn callable_object_signature(signature: &[Scalar]) -> bool {
    let Some((result, inputs)) = signature.split_last() else {
        return false;
    };
    (callable_object_value(result)
        || matches!(
            result,
            Scalar::List(_, _)
                | Scalar::Set(_)
                | Scalar::Dict(_, _)
                | Scalar::Generator(_, _)
                | Scalar::Model(_)
                | Scalar::Enum(_)
        ))
        && inputs.iter().all(|ty| {
            callable_object_value(ty)
                || (matches!(ty, Scalar::Tuple(_)) && source_signature_type(ty))
                || matches!(
                    ty,
                    Scalar::ListRef(_, _)
                        | Scalar::Model(_)
                        | Scalar::SetRef(_)
                        | Scalar::DictRef(_, _)
                        | Scalar::ListMutRef(_, _)
                        | Scalar::SetMutRef(_)
                        | Scalar::DictMutRef(_, _)
                        | Scalar::StringMutRef
                        | Scalar::ModelMutRef(_)
                        | Scalar::EnumMutRef(_)
                        | Scalar::Enum(_)
                )
        })
}

/// Report whether a value crosses a boxed closure's call boundary by value: a scalar of any numeric carrier, or text.
fn callable_object_value(ty: &Scalar) -> bool {
    matches!(
        ty,
        Scalar::Int
            | Scalar::Float
            | Scalar::I8
            | Scalar::I16
            | Scalar::I32
            | Scalar::I128
            | Scalar::ISize
            | Scalar::USize
            | Scalar::U8
            | Scalar::U16
            | Scalar::U32
            | Scalar::U64
            | Scalar::U128
            | Scalar::F32
            | Scalar::F64
            | Scalar::Bool
            | Scalar::Unit
            | Scalar::String
    )
}

/// Pin a closure constructor to the one shape its native body implements: rustc builds that body from the injected
/// boxing expression, so any other statement or edge in the plan would silently not execute.
fn validate_closure_constructor(function: &Function) -> Result<(), PlanError> {
    let Some((_, count)) = crate::closures::constructor(function) else {
        return Ok(());
    };
    let refused = || {
        invalid(
            function,
            "closure constructor must box its captures in exactly one call",
        )
    };
    let [first, second] = function.blocks.as_slice() else {
        return Err(refused());
    };
    if !first.statements.is_empty()
        || !second.statements.is_empty()
        || first.cleanup
        || second.cleanup
        || !matches!(second.terminator.kind, TerminatorKind::Return)
        || !matches!(function.return_type, PlanType::Closure(_) | PlanType::ClosureOnce(_))
        || usize::try_from(count).ok() != Some(function.parameters.len())
    {
        return Err(refused());
    }
    let TerminatorKind::Call(_, arguments, destination, 1, Unwind::Continue) = &first.terminator.kind else {
        return Err(refused());
    };
    let consumes_environment = matches!(function.return_type, PlanType::ClosureOnce(_));
    let TerminatorKind::Call(callee, ..) = &first.terminator.kind else {
        return Err(refused());
    };
    if consumes_environment != matches!(callee.kind, CalleeKind::ClosureBodyOnce(..))
        || (consumes_environment && function.parameters.is_empty())
    {
        return Err(refused());
    }
    // Each capture comes from its own parameter. Consuming environments move every capture; reusable environments move
    // owned text, lists, and models and copy scalars.
    let forwarded = arguments.iter().enumerate().all(|(index, argument)| {
        let (moved, place) = match &argument.kind {
            OperandKind::Copy(place) => (false, place),
            OperandKind::Move(place) => (true, place),
            _ => return false,
        };
        let owned = function
            .parameters
            .get(index)
            .is_some_and(|parameter| crate::closures::owned_capture(&parameter.ty));
        moved == (owned || consumes_environment)
            && matches!(place.projection, Projection::Whole)
            && usize::try_from(place.local).ok() == Some(index + 1)
    });
    if !forwarded
        || arguments.len() != function.parameters.len()
        || destination.local != 0
        || !matches!(destination.projection, Projection::Whole)
    {
        return Err(refused());
    }
    Ok(())
}

/// Reject unsupported hashed leaves and invalid dimensions before constructing native types, including unused locals.
///
/// A model's physical hashed storage requires its declared Hash layout (see [`hashed_model_layout`]); actual Eq and
/// Hash implementations are verified against native callee bounds before MIR. Enum payload derivation keeps the
/// stricter [`hashable_model`] and [`hashable_enum`] authority. Lowering separately refuses keyed dictionary lookups
/// whose runtime also bounds Display.
fn validate_array_length(plan: &Plan, function: &Function, ty: Scalar) -> Result<(), PlanError> {
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
        | Scalar::DictMutRef(Leaf::Tuple(elements), _)
            if elements.contains(&Scalar::Float) =>
        {
            Err(invalid(function, "floating-point tuple hashed keys lack Eq and Hash"))
        }
        Scalar::Set(Leaf::Enum(index))
        | Scalar::SetRef(Leaf::Enum(index))
        | Scalar::SetMutRef(Leaf::Enum(index))
        | Scalar::Dict(Leaf::Enum(index), _)
        | Scalar::DictRef(Leaf::Enum(index), _)
        | Scalar::DictMutRef(Leaf::Enum(index), _)
            if hashable_enum(plan, index) =>
        {
            Ok(())
        }
        Scalar::Set(Leaf::Enum(_))
        | Scalar::SetRef(Leaf::Enum(_))
        | Scalar::SetMutRef(Leaf::Enum(_))
        | Scalar::Dict(Leaf::Enum(_), _)
        | Scalar::DictRef(Leaf::Enum(_), _)
        | Scalar::DictMutRef(Leaf::Enum(_), _) => Err(invalid(
            function,
            "enum hashed keys require retained Eq and Hash authority",
        )),
        Scalar::Set(Leaf::Model(index))
        | Scalar::SetRef(Leaf::Model(index))
        | Scalar::SetMutRef(Leaf::Model(index))
        | Scalar::Dict(Leaf::Model(index), _)
        | Scalar::DictRef(Leaf::Model(index), _)
        | Scalar::DictMutRef(Leaf::Model(index), _)
            if hashed_model_layout(plan, index) =>
        {
            Ok(())
        }
        Scalar::Set(Leaf::Model(_))
        | Scalar::SetRef(Leaf::Model(_))
        | Scalar::SetMutRef(Leaf::Model(_))
        | Scalar::Dict(Leaf::Model(_), _)
        | Scalar::DictRef(Leaf::Model(_), _)
        | Scalar::DictMutRef(Leaf::Model(_), _) => Err(invalid(
            function,
            "model hashed storage requires retained Hash layout authority",
        )),
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
        Scalar::List(_, depth)
        | Scalar::ListRef(_, depth)
        | Scalar::ListMutRef(_, depth)
        | Scalar::ListIntoIter(_, depth)
        | Scalar::ListIntoIterMutRef(_, depth)
            if depth < 1 =>
        {
            Err(invalid(function, "list depth must be positive"))
        }
        _ => Ok(()),
    }
}

/// Check a formatting array's exact length and each element against its declared destination type.
fn validate_array(
    plan: &Plan,
    function: &Function,
    values: &[Operand],
    expected: &Scalar,
) -> Result<Scalar, PlanError> {
    let (element, count) = match expected {
        Scalar::StringArray(count) => (Scalar::String, *count),
        Scalar::StrArray(count) => (Scalar::StrRef, *count),
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
            &element,
            "formatting array element",
        )?;
    }
    Ok(match element {
        Scalar::String => Scalar::StringArray(count),
        _ => Scalar::StrArray(count),
    })
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
            BinaryOp::BitAnd | BinaryOp::BitOr | BinaryOp::BitXor if sized_integer(&ty) => Ok(ty),
            _ => Err(invalid(function, "unsupported sized numeric operator")),
        };
    }
    match op {
        BinaryOp::BitAnd | BinaryOp::BitOr | BinaryOp::BitXor if ty == Scalar::Int => Ok(Scalar::Int),
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

/// A shift keeps its left operand's integer type and counts with any integer type, as Rust's own shifts do.
fn shift_result(function: &Function, ty: Scalar, count: &Scalar) -> Result<Scalar, PlanError> {
    if integer(&ty) && integer(count) {
        Ok(ty)
    } else {
        Err(invalid(function, "shift operands must be integers"))
    }
}

/// Resolve a function reference against source-named declarations before invoking rustc.
fn signature(plan: &Plan, function: &Function, callee: &Callee) -> Result<(Vec<Scalar>, Scalar), PlanError> {
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
                generator_element(leaf, *depth)?,
            ],
            Scalar::Unit,
        )),
        CalleeKind::CollectGenerator(leaf, depth) => Ok((
            vec![Scalar::Generator(Leaf::of(leaf), *depth)],
            Scalar::List(Leaf::of(leaf), depth + 1),
        )),
        CalleeKind::CallClosure(signature) => {
            let signature: Vec<_> = signature.iter().map(scalar).collect();
            if !callable_object_signature(&signature) {
                return Err(invalid(
                    function,
                    "callable-object call requires admitted inputs and a result",
                ));
            }
            let (result, inputs) = signature
                .split_last()
                .ok_or_else(|| invalid(function, "callable-object call has no result type"))?;
            Ok((
                vec![Scalar::ClosureRef(signature.clone()), Scalar::Tuple(inputs.to_vec())],
                result.clone(),
            ))
        }
        CalleeKind::CallClosureOnce(signature) => {
            let signature: Vec<_> = signature.iter().map(scalar).collect();
            if !callable_object_signature(&signature) {
                return Err(invalid(function, "consuming callable-object signature is unsupported"));
            }
            let (result, inputs) = signature
                .split_last()
                .ok_or_else(|| invalid(function, "consuming callable-object has no result"))?;
            Ok((
                vec![Scalar::ClosureOnce(signature.clone()), Scalar::Tuple(inputs.to_vec())],
                result.clone(),
            ))
        }
        CalleeKind::CloneClosure(signature) => {
            let signature: Vec<_> = signature.iter().map(scalar).collect();
            if !callable_object_signature(&signature) {
                return Err(invalid(
                    function,
                    "callable-object clone requires admitted inputs and a result",
                ));
            }
            Ok((vec![Scalar::ClosureRef(signature.clone())], Scalar::Closure(signature)))
        }
        CalleeKind::ClosureBody(name, count) | CalleeKind::ClosureBodyOnce(name, count) => {
            let lifted = plan
                .functions
                .iter()
                .find(|lifted| &lifted.name == name)
                .ok_or_else(|| PlanError::UnknownCallee(name.clone()))?;
            let count = usize::try_from(*count)
                .ok()
                .filter(|count| *count <= lifted.parameters.len())
                .ok_or_else(|| invalid(function, "closure capture count exceeds the lifted parameters"))?;
            let mut object: Vec<_> = lifted.parameters[count..]
                .iter()
                .map(|parameter| scalar(&parameter.ty))
                .collect();
            object.push(scalar(&lifted.return_type));
            Ok((
                lifted.parameters[..count]
                    .iter()
                    .map(|parameter| scalar(&parameter.ty))
                    .collect(),
                if matches!(callee.kind, CalleeKind::ClosureBodyOnce(..)) {
                    Scalar::ClosureOnce(object)
                } else {
                    Scalar::Closure(object)
                },
            ))
        }
        CalleeKind::SharedCallableBody(signature) => Ok((
            vec![Scalar::Closure(signature.iter().map(scalar).collect())],
            Scalar::SharedCallable(signature.iter().map(scalar).collect()),
        )),
        CalleeKind::Value(value) => {
            let mut signature = match operand(plan, function, value)? {
                Scalar::FunctionPointer(signature) | Scalar::FunctionItem(_, signature) => signature,
                _ => return Err(invalid(function, "local call requires a callable type")),
            };
            let result = signature
                .pop()
                .ok_or_else(|| invalid(function, "local callable has no result type"))?;
            Ok((signature, result))
        }
        CalleeKind::StaticLoad(slot) => Ok((Vec::new(), scalar(&crate::statics::cell(plan, *slot)?.ty))),
        CalleeKind::StaticStore(slot) => Ok((vec![scalar(&crate::statics::cell(plan, *slot)?.ty)], Scalar::Unit)),
        CalleeKind::StaticForce(slot) => {
            crate::statics::cell(plan, *slot)?;
            Ok((Vec::new(), Scalar::Unit))
        }
        CalleeKind::FieldMetadata(owner, info) => {
            Ok((Vec::new(), scalar(&crate::field_metadata::result(plan, *owner, *info)?)))
        }
        CalleeKind::StaticMethod(slot, name) => {
            let method = crate::statics::method_target(plan, *slot, name)?;
            Ok((
                method
                    .parameters
                    .iter()
                    .skip(1)
                    .map(|parameter| scalar(&parameter.ty))
                    .collect(),
                scalar(&method.return_type),
            ))
        }
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
        CalleeKind::FutureMethod(..) => Err(invalid(
            function,
            "unsupported SDK method future outside its boxing shim",
        )),
        CalleeKind::External(path) | CalleeKind::Instantiated(path, _) => {
            let external = plan
                .externals
                .iter()
                .find(|external| {
                    &external.path == path
                        && match &callee.kind {
                            CalleeKind::Instantiated(_, types) => {
                                external.type_arguments.iter().map(scalar).eq(types.iter().map(scalar))
                            }
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
fn generator_element(leaf: &ListLeaf, depth: i64) -> Result<Scalar, PlanError> {
    let element = checked_list_leaf_type(leaf)?;
    if depth != 0 {
        return Ok(Scalar::List(Leaf::of(leaf), depth));
    }
    Ok(scalar(&element))
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
        TerminatorKind::Await(future, destination, target, cancelled, action, drop_action) => {
            if !function.is_async || cleanup {
                return Err(invalid(function, "unsupported await outside a coroutine body"));
            }
            let future_type = place(plan, function, future)?;
            match future_type {
                Scalar::Future(outputs) => {
                    let [output] = outputs.as_slice() else {
                        return Err(invalid(function, "unsupported future output arity"));
                    };
                    require(
                        function,
                        place(plan, function, destination)?,
                        output,
                        "await destination",
                    )?;
                }
                Scalar::Model(_) => {
                    // The checked suspension retains the output; coroutine expansion verifies Future::poll against it.
                    place(plan, function, destination)?;
                }
                _ => {
                    return Err(invalid(function, "unsupported await without an SDK future carrier"));
                }
            }
            edge(function, *target, false)?;
            edge(function, *cancelled, false)?;
            unwind(function, action, false)?;
            unwind(function, drop_action, false)
        }
        TerminatorKind::CoroutineDrop if function.is_async && !cleanup => Ok(()),
        TerminatorKind::CoroutineDrop => Err(invalid(function, "unsupported coroutine drop outside a coroutine body")),
        TerminatorKind::Goto(target) => edge(function, *target, cleanup),
        TerminatorKind::SwitchBool(condition, false_target, true_target) => {
            require(
                function,
                operand(plan, function, condition)?,
                &Scalar::Bool,
                "switch condition",
            )?;
            edge(function, *false_target, cleanup)?;
            edge(function, *true_target, cleanup)
        }
        TerminatorKind::Call(callee, arguments, destination, target, action) => {
            if matches!(callee.kind, CalleeKind::SpawnGenerator(..)) && !arguments.is_empty() {
                validate_generator_constructor(function, arguments)?;
            }
            let (parameters, result) = signature(plan, function, callee)?;
            if parameters.len() != arguments.len() {
                return Err(invalid(
                    function,
                    "call argument count differs from the callee signature",
                ));
            }
            for (argument, parameter) in arguments.iter().zip(parameters) {
                require(
                    function,
                    operand(plan, function, argument)?,
                    &parameter,
                    "call argument",
                )?;
            }
            require(
                function,
                place(plan, function, destination)?,
                &result,
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
                &Scalar::Bool,
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

/// Whether the planned model retains Hash and either a derived Eq or a fully validated native equality bridge.
fn hashable_model(plan: &Plan, index: i64) -> bool {
    model(plan, index).is_some_and(|declaration| {
        declaration.derives.iter().any(|derive| derive == "Hash")
            && (declaration.derives.iter().any(|derive| derive == "Eq")
                || crate::comparison_bridges::has_equality(plan, index))
    })
}

/// Admit physical model storage without inventing an equality implementation; native callee bounds remain required.
fn hashed_model_layout(plan: &Plan, index: i64) -> bool {
    model(plan, index).is_some_and(|declaration| declaration.derives.iter().any(|derive| derive == "Hash"))
}

/// Whether the planned enum at `index` hashes, the authority a hashed collection element needs.
///
/// A source enum must derive Eq and Hash. An `Option` or `Result` carrier aliases the standard type, which hashes when
/// every payload does; a payload enum must precede its carrier, as nested carriers are planned first, so the check
/// always terminates. A stdlib enum's derives belong to its compiled crate and never qualify.
fn hashable_enum(plan: &Plan, index: i64) -> bool {
    let Some(declaration) = enum_declaration(plan, index) else {
        return false;
    };
    if !declaration.library_item.is_empty() {
        return false;
    }
    if declaration.carrier.is_empty() {
        return ["Eq", "Hash"]
            .iter()
            .all(|required| declaration.derives.iter().any(|derive| derive == required));
    }
    matches!(declaration.carrier.as_str(), "Option" | "Result")
        && declaration
            .variants
            .iter()
            .flat_map(|variant| &variant.fields)
            .all(|field| hashable_payload(plan, &scalar(field), index))
}

/// Whether a carrier payload hashes: a nonfloating scalar or text, a tuple of those, or a hashable model or an
/// earlier hashable enum.
fn hashable_payload(plan: &Plan, ty: &Scalar, carrier: i64) -> bool {
    match ty {
        Scalar::Int
        | Scalar::Bool
        | Scalar::Unit
        | Scalar::U8
        | Scalar::String
        | Scalar::Decimal
        | Scalar::FrozenStr => true,
        Scalar::Tuple(elements) => elements.iter().all(|element| hashable_payload(plan, element, carrier)),
        Scalar::Model(index) => hashable_model(plan, *index),
        Scalar::Enum(index) => *index < carrier && hashable_enum(plan, *index),
        _ => false,
    }
}

/// Refuse malformed collection payloads before the Incan-owned projection indexes their exact child slots.
/// Recursive type checking remains in `validate_model_type`; this boundary neither truncates extra children nor
/// changes the existing carrier recursion or nominal cycle rules.
fn checked_list_leaf_type(leaf: &ListLeaf) -> Result<PlanType, PlanError> {
    let valid = match leaf {
        ListLeaf::List(elements, _) => elements.len() == 1,
        ListLeaf::Dict(elements) => elements.len() == 2,
        _ => true,
    };
    if !valid {
        return Err(PlanError::Invalid {
            function: "collection leaf".into(),
            reason: "nested collection leaf requires exactly one list child or two dictionary children".into(),
        });
    }
    Ok(list_leaf_type(leaf.clone()))
}

/// Validate retained nominal identities and the exact shape of nested and borrowed native carriers before MIR
/// construction.
fn validate_model_type(plan: &Plan, ty: &PlanType) -> Result<(), PlanError> {
    if let PlanType::TypeToken(elements) = ty
        && !matches!(
            elements.as_slice(),
            [PlanType::Int | PlanType::Float | PlanType::Bool | PlanType::String]
        )
    {
        return Err(PlanError::Invalid {
            function: "TypeToken".into(),
            reason: "unsupported TypeToken represented type or arity".into(),
        });
    }

    match ty {
        PlanType::NativeSlice(elements) | PlanType::SliceRef(elements) | PlanType::SliceMutRef(elements) => {
            let [element] = elements.as_slice() else {
                return Err(PlanError::Invalid {
                    function: "native slice".into(),
                    reason: "slice requires one element".into(),
                });
            };
            if !source_signature_type(&scalar(element)) {
                return Err(PlanError::Unsupported {
                    function: "native slice".into(),
                    reason: "unsupported slice element".into(),
                });
            }
            validate_model_type(plan, element)?;
        }
        PlanType::Future(outputs) => {
            let [output] = outputs.as_slice() else {
                return Err(PlanError::Invalid {
                    function: "future".into(),
                    reason: "unsupported future output arity".into(),
                });
            };
            if !source_signature_type(&scalar(output)) {
                return Err(PlanError::Invalid {
                    function: "future".into(),
                    reason: "unsupported future output carrier".into(),
                });
            }
            validate_model_type(plan, output)?;
        }
        PlanType::ScalarRef(elements) | PlanType::ScalarMutRef(elements) => {
            if !matches!(elements.as_slice(), [inner] if scalar_referent(&scalar(inner))) {
                return Err(PlanError::Unsupported {
                    function: "scalar reference".into(),
                    reason: "scalar reference requires one numeric, Boolean, decimal or frozen referent".into(),
                });
            }
        }
        PlanType::FrozenCollection(name, elements) | PlanType::FrozenCollectionRef(name, elements) => {
            let arity = match name.as_str() {
                "FrozenList" | "FrozenSet" => 1,
                "FrozenDict" => 2,
                _ => 0,
            };
            if arity == 0 || elements.len() != arity {
                return Err(PlanError::Invalid {
                    function: "frozen collection".into(),
                    reason: "unsupported frozen collection storage arity".into(),
                });
            }
            for element in elements {
                validate_model_type(plan, element)?;
            }
        }
        PlanType::CallableArguments(inputs) => {
            let mut signature: Vec<_> = inputs.iter().map(scalar).collect();
            signature.push(Scalar::Unit);
            if !callable_object_signature(&signature) {
                return Err(PlanError::Invalid {
                    function: "callable arguments".into(),
                    reason: "argument tuple contains an unsupported callable input".into(),
                });
            }
            for input in inputs {
                validate_model_type(plan, input)?;
            }
        }
        PlanType::List(leaf, _)
        | PlanType::ListRef(leaf, _)
        | PlanType::ListMutRef(leaf, _)
        | PlanType::ListIntoIter(leaf, _)
        | PlanType::ListIntoIterMutRef(leaf, _)
        | PlanType::Set(leaf)
        | PlanType::SetRef(leaf)
        | PlanType::SetMutRef(leaf)
        | PlanType::Generator(leaf, _)
        | PlanType::GeneratorMutRef(leaf, _)
        | PlanType::GeneratorYield(leaf, _)
        | PlanType::GeneratorYieldRef(leaf, _) => {
            validate_model_type(plan, &checked_list_leaf_type(leaf)?)?;
        }
        PlanType::Dict(key, value) | PlanType::DictRef(key, value) | PlanType::DictMutRef(key, value) => {
            validate_model_type(plan, &checked_list_leaf_type(key)?)?;
            validate_model_type(plan, &checked_list_leaf_type(value)?)?;
        }
        PlanType::Tuple(elements) | PlanType::TupleMutRef(elements) => {
            for element in elements {
                validate_model_type(plan, &tuple_element_type(element.clone()))?;
            }
        }
        PlanType::ZipIterator(left, right)
        | PlanType::ZipIteratorRef(left, right)
        | PlanType::ZipIteratorMutRef(left, right) => {
            validate_model_type(plan, &tuple_element_type(left.clone()))?;
            validate_model_type(plan, &tuple_element_type(right.clone()))?;
        }
        _ => {}
    }
    if matches!(ty, PlanType::Tuple(_)) && !source_signature_type(&scalar(ty)) {
        return Err(PlanError::Invalid {
            function: "tuple".into(),
            reason: "tuple elements must retain admitted scalar, owned or frozen text, list, nominal, function-pointer or nested tuple layouts".into(),
        });
    }
    if let PlanType::Enum(index, name) | PlanType::EnumRef(index, name) | PlanType::EnumMutRef(index, name) = ty {
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
    if let PlanType::FunctionItem(name, signature) = ty {
        let actual: Vec<_> = signature.iter().map(scalar).collect();
        let expected = declared_function_item_signature(plan, name)?;
        if actual != expected {
            return Err(PlanError::Invalid {
                function: name.clone(),
                reason: "function item signature differs from its declaration".into(),
            });
        }
    }
    if let PlanType::Closure(signature)
    | PlanType::ClosureOnce(signature)
    | PlanType::ClosureRef(signature)
    | PlanType::SharedCallable(signature) = ty
        && !callable_object_signature(&signature.iter().map(scalar).collect::<Vec<_>>())
    {
        return Err(PlanError::Invalid {
            function: "callable object".into(),
            reason: "callable-object signature requires admitted inputs and a final return type".into(),
        });
    }
    if let PlanType::Closure(signature)
    | PlanType::ClosureOnce(signature)
    | PlanType::ClosureRef(signature)
    | PlanType::SharedCallable(signature) = ty
    {
        for component in signature {
            validate_model_type(plan, component)?;
        }
    }
    if let PlanType::FunctionPointer(signature) = ty {
        if !source_signature_type(&scalar(ty)) {
            return Err(PlanError::Invalid {
                function: "function pointer".into(),
                reason: "callable signature requires admitted inputs and a final return type".into(),
            });
        }
        for component in signature {
            validate_model_type(plan, component)?;
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

/// Read the exact signature of a retained local or compiled function item. External declarations undergo the same
/// metadata verification as ordinary calls before MIR construction; a missing declaration grants no item type.
fn declared_function_item_signature(plan: &Plan, name: &str) -> Result<Vec<Scalar>, PlanError> {
    if let Some(function) = plan.functions.iter().find(|function| function.name == name) {
        return Ok(function
            .parameters
            .iter()
            .map(|parameter| scalar(&parameter.ty))
            .chain(std::iter::once(scalar(&function.return_type)))
            .collect());
    }
    let external = plan
        .externals
        .iter()
        .find(|external| external.path == name && external.type_arguments.is_empty())
        .ok_or_else(|| PlanError::UnknownCallee(name.into()))?;
    Ok(external
        .parameters
        .iter()
        .map(scalar)
        .chain(std::iter::once(scalar(&external.return_type)))
        .collect())
}

/// Require static data to match every retained storage argument before native allocation or declaration injection.
fn validate_frozen_values(
    plan: &Plan,
    function: &Function,
    ty: &PlanType,
    values: &[crate::plan::FrozenValue],
) -> Result<(), PlanError> {
    use crate::plan::FrozenValue;
    validate_model_type(plan, ty)?;
    let PlanType::FrozenCollection(_, elements) = ty else {
        return Err(invalid(function, "unsupported noncollection frozen data"));
    };
    if !values.len().is_multiple_of(elements.len()) {
        return Err(invalid(function, "unsupported incomplete frozen collection data"));
    }
    for (index, value) in values.iter().enumerate() {
        let element = &elements[index % elements.len()];
        match (element, value) {
            (PlanType::Int, FrozenValue::Scalar(Constant::Int(_)))
            | (PlanType::Bool, FrozenValue::Scalar(Constant::Bool(_)))
            | (PlanType::StaticText, FrozenValue::Scalar(Constant::Text(_))) => {}
            (PlanType::FrozenStr, FrozenValue::Scalar(Constant::Text(_))) => {}
            (PlanType::StaticBytes | PlanType::FrozenBytes, FrozenValue::Bytes(bytes))
                if bytes.iter().all(|byte| u8::try_from(*byte).is_ok()) => {}
            (PlanType::FrozenCollection(..), FrozenValue::Collection(child, data))
                if scalar(element) == scalar(child) =>
            {
                validate_frozen_values(plan, function, child, data)?
            }
            _ => return Err(invalid(function, "unsupported frozen literal storage type")),
        }
    }
    Ok(())
}

/// Validate the shape-specific derive vocabulary without accepting duplicate selections or reflection on tuples. Serde
/// derives fit either shape (a tuple wrapper serializes as its wrapped value), and so does Copy, which rustc admits
/// only when every field is Copy. A newtype retains only the automatic derives its underlying type supports; a
/// non-Clone native permit must not acquire Clone merely by being wrapped.
fn valid_nominal_derives(derives: &[String], tuple: bool) -> bool {
    let required: &[&str] = if tuple {
        &[]
    } else {
        &["Debug", "Clone", "FieldInfo", "IncanClass"]
    };
    required
        .iter()
        .all(|required| derives.iter().any(|derive| derive == required))
        && derives.iter().all(|derive| match derive.as_str() {
            "Debug" | "Clone" | "Eq" | "PartialEq" | "Hash" | "Ord" | "PartialOrd" | "Default" | "Display" => true,
            "serde::Serialize" | "serde::Deserialize" | "Copy" => true,
            "FieldInfo" | "IncanClass" => !tuple,
            _ => false,
        })
        && derives.iter().collect::<BTreeSet<_>>().len() == derives.len()
}

/// A compiled layout names an item below its owning crate; a provider's own generic instantiations name `crate`.
fn valid_library_item(library_item: &str) -> bool {
    if library_item.is_empty() {
        return true;
    }
    let segments: Vec<_> = library_item.split("::").collect();
    segments.len() >= 2
        && segments
            .iter()
            .enumerate()
            .all(|(index, segment)| (index == 0 && *segment == "crate") || identifier(segment))
}

/// Accept serde field names only as one entry per named field of a declaration that derives serde, the only derive
/// that reads them.
fn valid_serde_names(declaration: &crate::plan::ModelDeclaration, tuple: bool) -> bool {
    declaration.field_serde_names.is_empty()
        || (!tuple
            && declaration.field_serde_names.len() == declaration.fields.len()
            && declaration
                .derives
                .iter()
                .any(|derive| matches!(derive.as_str(), "serde::Serialize" | "serde::Deserialize")))
}

/// Admit generic ADT binders and field types before AST injection, rejecting collisions with concrete layouts and
/// functions. Native layout verification separately validates every concrete alias against its instantiated ADT.
fn validate_generic_models(plan: &Plan) -> Result<(), PlanError> {
    let mut names: BTreeSet<_> = plan
        .functions
        .iter()
        .map(|value| value.name.as_str())
        .chain(plan.models.iter().map(|value| value.name.as_str()))
        .chain(plan.enums.iter().map(|value| value.name.as_str()))
        .collect();
    for model in &plan.generic_models {
        span(&model.span)?;
        let parameters = model.parameters.iter().map(String::as_str).collect::<BTreeSet<_>>();
        let mut fields = BTreeSet::new();
        if !identifier(&model.name)
            || !identifier(&model.source_name)
            || !names.insert(&model.name)
            || parameters.is_empty()
            || parameters.len() != model.parameters.len()
            || parameters.iter().any(|name| !identifier(name))
            || model.derives.iter().any(|name| {
                !matches!(
                    name.as_str(),
                    "Clone" | "Debug" | "PartialEq" | "Eq" | "PartialOrd" | "Ord" | "Hash"
                )
            })
            || model.fields.iter().any(|field| {
                !identifier(&field.name) || !fields.insert(&field.name) || !generic_field_type(&field.ty, &parameters)
            })
        {
            return Err(PlanError::Invalid {
                function: model.name.clone(),
                reason: "invalid generic ADT declaration".into(),
            });
        }
    }
    Ok(())
}

/// A generic field may mention only its declared binders, complete concrete carriers, and explicit nominal paths.
fn generic_field_type(ty: &GenericLayoutType, parameters: &BTreeSet<&str>) -> bool {
    match ty {
        GenericLayoutType::Parameter(name) => parameters.contains(name.as_str()),
        GenericLayoutType::Concrete(ty) => source_signature_type(&scalar(ty)),
        GenericLayoutType::Tuple(elements) => elements.iter().all(|element| generic_field_type(element, parameters)),
        GenericLayoutType::FunctionPointer(signature) => {
            !signature.is_empty() && signature.iter().all(|element| generic_field_type(element, parameters))
        }
        GenericLayoutType::Applied(name, arguments) => {
            name.split("::").enumerate().all(|(index, segment)| {
                (index == 0 && matches!(segment, "crate" | "core" | "std" | "alloc")) || identifier(segment)
            }) && arguments
                .iter()
                .all(|argument| generic_field_type(argument, parameters))
        }
    }
}

/// Validate model and newtype metadata before declaration injection; size cycles are checked separately.
/// Function-pointer fields keep their complete validated signatures and Copy storage. Captured callable objects remain
/// a named unsupported refusal distinct from malformed metadata.
fn validate_models(plan: &Plan) -> Result<(), PlanError> {
    let mut names: BTreeSet<_> = plan.functions.iter().map(|function| function.name.clone()).collect();
    for declaration in &plan.models {
        span(&declaration.span)?;
        let tuple = declaration.fields.len() == 1 && declaration.fields[0].name == "0";
        // A stdlib layout's derives belong to the crate that compiled it; the plan records none of its own.
        let derives_valid = if declaration.library_item.is_empty() {
            valid_nominal_derives(&declaration.derives, tuple)
        } else {
            declaration.derives.is_empty()
        };
        if !identifier(&declaration.name)
            || !names.insert(declaration.name.clone())
            || (!declaration.source_name.is_empty() && !identifier(&declaration.source_name))
            || (!declaration.source_name.is_empty()
                && declaration.source_name != declaration.name
                && !names.insert(crate::declarations::model_module_name(declaration)))
            || declaration.fields.len() != declaration.field_public.len()
            || !valid_serde_names(declaration, tuple)
            || !derives_valid
            || !valid_library_item(&declaration.library_item)
            || (declaration.library_item.is_empty() && !declaration.library_type_arguments.is_empty())
        {
            return Err(PlanError::Invalid {
                function: declaration.name.clone(),
                reason: "invalid plain model declaration".into(),
            });
        }
        for argument in &declaration.library_type_arguments {
            if !matches!(argument, PlanType::NativeStr | PlanType::NativeSlice(_))
                && !external_signature_type(&scalar(argument))
            {
                return Err(PlanError::Invalid {
                    function: declaration.name.clone(),
                    reason: "unsupported SDK alias type argument".into(),
                });
            }
            validate_model_type(plan, argument)?;
        }
        let mut fields = BTreeSet::new();
        if declaration.library_item.is_empty() && !declaration.library_type_arguments.is_empty() {
            return Err(PlanError::Unsupported {
                function: declaration.name.clone(),
                reason: "native type arguments on a source-owned model".into(),
            });
        }
        for field in &declaration.fields {
            if declaration.library_item.is_empty()
                && borrowed_alias_in_source_type(plan, &field.ty, &mut BTreeSet::new())
            {
                return Err(PlanError::Unsupported {
                    function: declaration.name.clone(),
                    reason: "native borrowed carrier in a source field requires a retained lifetime contract".into(),
                });
            }
            span(&field.span)?;
            if matches!(field.ty, PlanType::Closure(_) | PlanType::ClosureRef(_)) {
                return Err(PlanError::Invalid {
                    function: declaration.name.clone(),
                    reason: "unsupported callable model field declaration".into(),
                });
            }
            if (!tuple && !identifier(&field.name))
                || !fields.insert(&field.name)
                || !source_signature_type(&scalar(&field.ty))
            {
                return Err(PlanError::Invalid {
                    function: declaration.name.clone(),
                    reason: "invalid model field declaration".into(),
                });
            }
            if matches!(
                field.ty,
                PlanType::Closure(_) | PlanType::ClosureOnce(_) | PlanType::ClosureRef(_)
            ) {
                return Err(PlanError::Invalid {
                    function: declaration.name.clone(),
                    reason: "unsupported callable model field declaration".into(),
                });
            }
            validate_model_type(plan, &field.ty)?;
        }
    }
    Ok(())
}

/// Order native aliases after their argument dependencies, including referents, before metadata type queries run.
/// Local loans still require native MIR checking; aliases introduce no source lifetime binders.
pub(crate) fn library_model_order(plan: &Plan) -> Result<Vec<usize>, PlanError> {
    let mut visiting = BTreeSet::new();
    let mut complete = BTreeSet::new();
    let mut ordered = Vec::new();
    for index in 0..plan.models.len() {
        visit_library_arguments(plan, index, &mut visiting, &mut complete, &mut ordered)?;
    }
    Ok(ordered)
}

/// Resolve only declared argument edges before querying an alias's native type or evaluating its bounds.
fn visit_library_arguments(
    plan: &Plan,
    index: usize,
    visiting: &mut BTreeSet<usize>,
    complete: &mut BTreeSet<usize>,
    ordered: &mut Vec<usize>,
) -> Result<(), PlanError> {
    if complete.contains(&index) {
        return Ok(());
    }
    let declaration = plan.models.get(index).ok_or_else(|| PlanError::Unsupported {
        function: "stdlib layout argument".into(),
        reason: "argument names no retained model".into(),
    })?;
    if !visiting.insert(index) {
        return Err(PlanError::Unsupported {
            function: declaration.name.clone(),
            reason: "recursive stdlib layout type arguments".into(),
        });
    }
    let mut dependencies = BTreeSet::new();
    for argument in &declaration.library_type_arguments {
        library_argument_models(plan, argument, &mut BTreeSet::new(), &mut dependencies)?;
    }
    for dependency in dependencies {
        visit_library_arguments(plan, dependency, visiting, complete, ordered)?;
    }
    visiting.remove(&index);
    complete.insert(index);
    ordered.push(index);
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

/// The ordinary and sized integer carriers, the only operands of bit operations.
fn integer(ty: &Scalar) -> bool {
    *ty == Scalar::Int || sized_integer(ty)
}

/// Resolve a source enum layout by its validated plan index.
fn enum_declaration(plan: &Plan, index: i64) -> Option<&crate::plan::EnumDeclaration> {
    usize::try_from(index).ok().and_then(|index| plan.enums.get(index))
}

/// Reject malformed enum layouts and unsupported derives; validate exact forward references before size-cycle checks.
/// Native Option/Result carriers and library enums retain their checked borrowed payloads; source-owned enum fields
/// remain limited to source signature types.
fn validate_enums(plan: &Plan) -> Result<(), PlanError> {
    let mut names: BTreeSet<_> = plan
        .functions
        .iter()
        .map(|value| &value.name)
        .chain(plan.models.iter().map(|value| &value.name))
        .collect();
    for declaration in &plan.enums {
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
        if (declaration.member_debug && !member_debug_layout(declaration))
            || (!declaration.raw_kind.is_empty() && !raw_value_layout(declaration))
            || !identifier(&declaration.name)
            || !names.insert(&declaration.name)
            || declaration.variants.is_empty()
            || !valid_library_item(&declaration.library_item)
            || (declaration.library_item.is_empty() && !declaration.library_type_arguments.is_empty())
            || (!declaration.library_item.is_empty()
                && (!declaration.carrier.is_empty()
                    || declaration.derives.iter().any(|derive| derive != "Clone")
                    || !declaration.raw_kind.is_empty()))
            || declaration.derives.iter().any(|name| match name.as_str() {
                "Debug" | "Clone" | "Copy" | "PartialEq" | "Eq" | "PartialOrd" | "Ord" | "Hash" => false,
                "serde::Serialize" | "serde::Deserialize" => {
                    !declaration.carrier.is_empty() || declaration.member_debug
                }
                _ => true,
            })
        {
            return Err(error());
        }
        for argument in &declaration.library_type_arguments {
            if !source_signature_type(&scalar(argument)) {
                return Err(error());
            }
            validate_model_type(plan, argument)?;
        }
        let mut variants = BTreeSet::new();
        for variant in &declaration.variants {
            if !identifier(&variant.name) || !variants.insert(&variant.name) {
                return Err(error());
            }
            for field in &variant.fields {
                validate_model_type(plan, field)?;
                let native = !declaration.carrier.is_empty() || !declaration.library_item.is_empty();
                if !(source_signature_type(&scalar(field)) || (native && external_signature_type(&scalar(field)))) {
                    return Err(error());
                }
            }
        }
    }
    Ok(())
}

/// Admit a member-delegating `Debug` only on a union layout: an ordinary enum that derives no `Debug` of its own and
/// whose every variant holds exactly the one member the generated impl formats.
fn member_debug_layout(declaration: &crate::plan::EnumDeclaration) -> bool {
    declaration.carrier.is_empty()
        && declaration.name.starts_with("__IncanUnion")
        && !declaration.derives.iter().any(|name| name == "Debug")
        && declaration.variants.iter().all(|variant| variant.fields.len() == 1)
}

/// Require each computed static cell to own a distinct slot and to name a planned initializer that takes no
/// parameters and returns exactly the cell's value type.
fn validate_statics(plan: &Plan) -> Result<(), PlanError> {
    let mut slots = BTreeSet::new();
    for cell in &plan.statics {
        span(&cell.span)?;
        let initializer = plan
            .functions
            .iter()
            .find(|function| function.name == cell.initializer)
            .ok_or_else(|| PlanError::UnknownCallee(cell.initializer.clone()))?;
        if !slots.insert(cell.slot) || !initializer.parameters.is_empty() || initializer.return_type != cell.ty {
            return Err(invalid(
                initializer,
                "static cell slot or initializer signature is invalid",
            ));
        }
    }
    Ok(())
}

/// Admit raw values only on a value enum layout: an ordinary unit-only enum with one distinct raw value per variant,
/// each an `i64` for an integer backing, which its serde impls write and read.
fn raw_value_layout(declaration: &crate::plan::EnumDeclaration) -> bool {
    let kind_valid = match declaration.raw_kind.as_str() {
        "str" => true,
        "int" => declaration.raw_values.iter().all(|raw| raw.parse::<i64>().is_ok()),
        _ => false,
    };
    kind_valid
        && declaration.carrier.is_empty()
        && !declaration.member_debug
        && declaration.raw_values.len() == declaration.variants.len()
        && declaration.variants.iter().all(|variant| variant.fields.is_empty())
        && declaration.raw_values.iter().collect::<BTreeSet<_>>().len() == declaration.raw_values.len()
}

/// Reject infinitely sized by-value nominal/enum layouts while allowing cycles behind collections and references.
fn validate_value_layouts(plan: &Plan) -> Result<(), PlanError> {
    let mut visiting = BTreeSet::new();
    let mut complete = BTreeSet::new();
    for (index, declaration) in plan.models.iter().enumerate() {
        visit_value_layout(plan, false, index, &declaration.name, &mut visiting, &mut complete)?;
    }
    for (index, declaration) in plan.enums.iter().enumerate() {
        visit_value_layout(plan, true, index, &declaration.name, &mut visiting, &mut complete)?;
    }
    Ok(())
}

/// Walk only direct ownership edges; collection leaves are validated separately and do not affect native size.
fn visit_value_layout(
    plan: &Plan,
    is_enum: bool,
    index: usize,
    name: &str,
    visiting: &mut BTreeSet<(bool, usize)>,
    complete: &mut BTreeSet<(bool, usize)>,
) -> Result<(), PlanError> {
    let key = (is_enum, index);
    if complete.contains(&key) {
        return Ok(());
    }
    if !visiting.insert(key) {
        return Err(PlanError::Invalid {
            function: name.into(),
            reason: "recursive by-value layout".into(),
        });
    }
    let fields: Vec<&PlanType> = if is_enum {
        plan.enums[index]
            .variants
            .iter()
            .flat_map(|variant| &variant.fields)
            .collect()
    } else {
        plan.models[index].fields.iter().map(|field| &field.ty).collect()
    };
    for field in fields {
        let (owner_is_enum, owner, owner_name) = match field {
            PlanType::Enum(owner, name) => (true, *owner, name),
            PlanType::Model(owner, name) => (false, *owner, name),
            _ => continue,
        };
        let owner = usize::try_from(owner).map_err(|_| PlanError::Invalid {
            function: owner_name.clone(),
            reason: "invalid by-value layout owner".into(),
        })?;
        visit_value_layout(plan, owner_is_enum, owner, owner_name, visiting, complete)?;
    }
    visiting.remove(&key);
    complete.insert(key);
    Ok(())
}

/// Walk owned argument shapes before AST materialization; an alias cannot invent lifetimes for borrowed components.
fn library_argument_models(
    plan: &Plan,
    ty: &PlanType,
    enums: &mut BTreeSet<i64>,
    models: &mut BTreeSet<usize>,
) -> Result<(), PlanError> {
    match ty {
        PlanType::Model(index, _) | PlanType::ModelRef(index, _) | PlanType::ModelMutRef(index, _) => {
            models.insert(usize::try_from(*index).map_err(|_| PlanError::Unsupported {
                function: "stdlib layout argument".into(),
                reason: "argument model index is negative".into(),
            })?);
        }
        PlanType::Enum(index, _) | PlanType::EnumRef(index, _) | PlanType::EnumMutRef(index, _) => {
            if enums.insert(*index) {
                if let Some(declaration) = enum_declaration(plan, *index) {
                    for field in declaration.variants.iter().flat_map(|variant| &variant.fields) {
                        library_argument_models(plan, field, enums, models)?;
                    }
                }
            }
        }
        PlanType::List(leaf, _)
        | PlanType::ListRef(leaf, _)
        | PlanType::ListMutRef(leaf, _)
        | PlanType::Set(leaf)
        | PlanType::SetRef(leaf)
        | PlanType::SetMutRef(leaf)
        | PlanType::Generator(leaf, _)
        | PlanType::GeneratorYield(leaf, _) => {
            library_argument_models(plan, &checked_list_leaf_type(leaf)?, enums, models)?;
        }
        PlanType::Dict(left, right) | PlanType::DictRef(left, right) | PlanType::DictMutRef(left, right) => {
            library_argument_models(plan, &checked_list_leaf_type(left)?, enums, models)?;
            library_argument_models(plan, &checked_list_leaf_type(right)?, enums, models)?;
        }
        PlanType::ZipIterator(left, right) => {
            library_argument_models(plan, &tuple_element_type(left.clone()), enums, models)?;
            library_argument_models(plan, &tuple_element_type(right.clone()), enums, models)?;
        }
        PlanType::Tuple(elements) => {
            for element in elements {
                library_argument_models(plan, &tuple_element_type(element.clone()), enums, models)?;
            }
        }
        PlanType::CallableArguments(signature)
        | PlanType::ScalarRef(signature)
        | PlanType::ScalarMutRef(signature)
        | PlanType::NativeSlice(signature)
        | PlanType::SliceRef(signature)
        | PlanType::SliceMutRef(signature)
        | PlanType::Closure(signature)
        | PlanType::ClosureOnce(signature)
        | PlanType::SharedCallable(signature)
        | PlanType::Future(signature)
        | PlanType::FrozenCollection(_, signature)
        | PlanType::TypeToken(signature)
        | PlanType::FunctionPointer(signature)
        | PlanType::FunctionItem(_, signature) => {
            for component in signature {
                library_argument_models(plan, component, enums, models)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Find erased loans inside native aliases before a source signature or field could expose them without lifetime
/// binders. Local adapters remain subject to native signature and MIR borrow checking; this gate proves no loan scope.
fn borrowed_alias_in_source_type(plan: &Plan, ty: &PlanType, seen: &mut BTreeSet<(bool, i64)>) -> bool {
    match ty {
        PlanType::Model(index, _) | PlanType::ModelRef(index, _) | PlanType::ModelMutRef(index, _) => {
            if !seen.insert((false, *index)) {
                return false;
            }
            let Some(model) = usize::try_from(*index).ok().and_then(|index| plan.models.get(index)) else {
                return true;
            };
            model.library_type_arguments.iter().any(|argument| {
                library_argument_is_borrowed(argument) || borrowed_alias_in_source_type(plan, argument, seen)
            }) || model
                .fields
                .iter()
                .any(|field| borrowed_alias_in_source_type(plan, &field.ty, seen))
        }
        PlanType::Enum(index, _) | PlanType::EnumRef(index, _) | PlanType::EnumMutRef(index, _) => {
            if !seen.insert((true, *index)) {
                return false;
            }
            let Some(declaration) = enum_declaration(plan, *index) else {
                return true;
            };
            declaration.library_type_arguments.iter().any(|argument| {
                library_argument_is_borrowed(argument) || borrowed_alias_in_source_type(plan, argument, seen)
            }) || declaration
                .variants
                .iter()
                .flat_map(|variant| &variant.fields)
                .any(|field| {
                    ((!declaration.carrier.is_empty() || !declaration.library_item.is_empty())
                        && library_argument_is_borrowed(field))
                        || borrowed_alias_in_source_type(plan, field, seen)
                })
        }
        PlanType::List(leaf, _)
        | PlanType::ListRef(leaf, _)
        | PlanType::ListMutRef(leaf, _)
        | PlanType::Set(leaf)
        | PlanType::SetRef(leaf)
        | PlanType::SetMutRef(leaf)
        | PlanType::Generator(leaf, _)
        | PlanType::GeneratorYield(leaf, _) => {
            checked_list_leaf_type(leaf).map_or(true, |element| borrowed_alias_in_source_type(plan, &element, seen))
        }
        PlanType::Dict(left, right) | PlanType::DictRef(left, right) | PlanType::DictMutRef(left, right) => {
            [left, right].into_iter().any(|leaf| {
                checked_list_leaf_type(leaf).map_or(true, |element| borrowed_alias_in_source_type(plan, &element, seen))
            })
        }
        PlanType::Tuple(elements) | PlanType::TupleMutRef(elements) => elements
            .iter()
            .any(|element| borrowed_alias_in_source_type(plan, &tuple_element_type(element.clone()), seen)),
        PlanType::NativeSlice(elements)
        | PlanType::SliceRef(elements)
        | PlanType::SliceMutRef(elements)
        | PlanType::ScalarRef(elements)
        | PlanType::ScalarMutRef(elements)
        | PlanType::CallableArguments(elements)
        | PlanType::Future(elements)
        | PlanType::FunctionPointer(elements)
        | PlanType::FunctionItem(_, elements)
        | PlanType::Closure(elements)
        | PlanType::ClosureOnce(elements)
        | PlanType::ClosureRef(elements)
        | PlanType::SharedCallable(elements)
        | PlanType::FrozenCollection(_, elements)
        | PlanType::FrozenCollectionRef(_, elements)
        | PlanType::TypeToken(elements) => elements
            .iter()
            .any(|element| borrowed_alias_in_source_type(plan, element, seen)),
        _ => false,
    }
}

/// Identify a native reference held as a generic argument, whose source-facing alias would need lifetime binders.
fn library_argument_is_borrowed(ty: &PlanType) -> bool {
    matches!(
        ty,
        PlanType::ScalarRef(..)
            | PlanType::ScalarMutRef(..)
            | PlanType::SliceRef(..)
            | PlanType::SliceMutRef(..)
            | PlanType::FrozenCollectionRef(..)
            | PlanType::ModelRef(..)
            | PlanType::ModelMutRef(..)
            | PlanType::EnumRef(..)
            | PlanType::EnumMutRef(..)
            | PlanType::StringRef
            | PlanType::StringMutRef
            | PlanType::StrRef
            | PlanType::ValidationErrorRef
            | PlanType::StringArrayRef(..)
            | PlanType::StrArrayRef(..)
            | PlanType::StringSlice
            | PlanType::StrSlice
            | PlanType::ListRef(..)
            | PlanType::ListMutRef(..)
            | PlanType::SetRef(..)
            | PlanType::SetMutRef(..)
            | PlanType::DictRef(..)
            | PlanType::DictMutRef(..)
            | PlanType::ZipIteratorRef(..)
            | PlanType::ZipIteratorMutRef(..)
            | PlanType::GeneratorMutRef(..)
            | PlanType::GeneratorYieldRef(..)
            | PlanType::ClosureRef(..)
    )
}
