//! Preflight validation of the Incan-owned scalar plan. No rustc type crosses this module.

use incan_lang::lang::derives::{self, DeriveId};
use incan_lang::lang::surface::constructors::{self, ConstructorId};
use incan_lang::lang::types::collections::{self, CollectionTypeId};
use internal_models::{CanonicalSymbolId, DeclarationKind, SymbolNamespace, SymbolOrigin};
use std::collections::BTreeSet;

use crate::error::PlanError;
use crate::plan::{
    BinaryOp, Callee, CalleeKind, Carrier, Constant, Function, GenericLayoutType, ListLeaf, Operand, OperandKind,
    Place, Plan, PlanType, Projection, RvalueKind, SizedNumeric, SourceSpan, StatementKind, TerminatorKind, UnaryOp,
    Unwind, list_leaf_type, range_field_types, tuple_element_type,
};

mod calls_and_terminators;
mod function_bodies;
mod nominal_layouts;
mod rvalues;
mod signature_types;
mod type_declarations;

use calls_and_terminators::*;
use function_bodies::*;
pub(crate) use nominal_layouts::*;
use rvalues::*;
use signature_types::*;
use type_declarations::*;

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
#[derive(Clone, Debug, PartialEq, Eq)]
enum Scalar {
    CChar,
    RawPointer(bool, Vec<Scalar>),
    // An owned heap box around exactly one referent.
    Boxed(Vec<Scalar>),
    NativeStr,
    NativeSlice(Vec<Scalar>),
    NativeArray(Vec<Scalar>, i64),
    NativeArrayRef(Vec<Scalar>, i64),
    NativeArrayMutRef(Vec<Scalar>, i64),
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
    Never,
    EnumTag,
    Enum(i64),
    EnumRef(i64),
    EnumMutRef(i64),
    CheckedInt,
    Tuple(Vec<Scalar>),
    /// A shared reference keeps every tuple field's layout and owns none of its values.
    TupleRef(Vec<Scalar>),
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
    /// Canonical source trait identity carried only inside a type token.
    Trait(CanonicalSymbolId),
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
    NativeClosure(Vec<Scalar>, bool, bool),
    NativeTraitObject(String, bool, bool),
    SharedCallable(Vec<Scalar>),
    ClosureOnce(Vec<Scalar>),
    ClosureRef(Vec<Scalar>),
    BorrowingClosure(Vec<Scalar>),
    BorrowingClosureRef(Vec<Scalar>),
    Future(Vec<Scalar>),
}

/// Comparison mirror of scalar and tuple leaves; the public plan remains Incan-authored.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Leaf {
    /// One exact shared-reference carrier, never an owned copy of its referent.
    Reference(Vec<Scalar>),
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
    /// An exact-width scalar, compared through the plan's canonical numeric mapping.
    Numeric(Numeric),
    Unit,
    Decimal,
    FrozenStr,
    FrozenBytes,
    FrozenCollection(String, Vec<Scalar>),
    /// One zero-sized token with its represented checked carrier.
    TypeToken(Vec<Scalar>),
    /// An owned nested list preserves every supplied leaf until canonical arity validation.
    List(Vec<Leaf>, i64),
    /// An owned nested dictionary preserves key/value arity instead of indexing an unchecked vector.
    Dict(Vec<Leaf>),
    /// An owned nested set preserves its element arity for validation.
    Set(Vec<Leaf>),
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
            ListLeaf::Reference(elements) => Leaf::Reference(elements.iter().map(scalar).collect()),
            ListLeaf::U8 => Leaf::U8,
            ListLeaf::USize => Leaf::USize,
            ListLeaf::Numeric(kind) => Leaf::Numeric(numeric(kind)),
            ListLeaf::Unit => Leaf::Unit,
            ListLeaf::Decimal => Leaf::Decimal,
            ListLeaf::FrozenStr => Leaf::FrozenStr,
            ListLeaf::FrozenBytes => Leaf::FrozenBytes,
            ListLeaf::FrozenCollection(name, elements) => {
                Leaf::FrozenCollection(name.clone(), elements.iter().map(scalar).collect())
            }
            ListLeaf::FunctionPointer(signature) => Leaf::FunctionPointer(signature.iter().map(scalar).collect()),
            ListLeaf::TypeToken(represented) => Leaf::TypeToken(represented.iter().map(scalar).collect()),
            ListLeaf::List(elements, depth) => Leaf::List(elements.iter().map(Leaf::of).collect(), *depth),
            ListLeaf::Dict(elements) => Leaf::Dict(elements.iter().map(Leaf::of).collect()),
            ListLeaf::Set(elements) => Leaf::Set(elements.iter().map(Leaf::of).collect()),
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
        PlanType::CChar => Scalar::CChar,
        PlanType::RawPointer(mutable, elements) => Scalar::RawPointer(*mutable, elements.iter().map(scalar).collect()),
        PlanType::Boxed(elements) => Scalar::Boxed(elements.iter().map(scalar).collect()),
        PlanType::NativeStr => Scalar::NativeStr,
        PlanType::NativeSlice(elements) => Scalar::NativeSlice(elements.iter().map(scalar).collect()),
        PlanType::NativeArray(elements, length) => Scalar::NativeArray(elements.iter().map(scalar).collect(), *length),
        PlanType::NativeArrayRef(elements, length) => {
            Scalar::NativeArrayRef(elements.iter().map(scalar).collect(), *length)
        }
        PlanType::NativeArrayMutRef(elements, length) => {
            Scalar::NativeArrayMutRef(elements.iter().map(scalar).collect(), *length)
        }
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
        PlanType::Tuple(elements) => Scalar::Tuple(tuple_scalars(elements)),
        PlanType::TupleRef(elements) => Scalar::TupleRef(tuple_scalars(elements)),
        PlanType::TupleMutRef(elements) => Scalar::TupleMutRef(tuple_scalars(elements)),
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
        PlanType::Never => Scalar::Never,
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
        PlanType::Trait(identity) => Scalar::Trait(identity.clone()),
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
        PlanType::NativeTraitObject(path, send, sync) => Scalar::NativeTraitObject(path.clone(), *send, *sync),
        PlanType::NativeClosure(signature, send, sync) => {
            Scalar::NativeClosure(signature.iter().map(scalar).collect(), *send, *sync)
        }
        PlanType::SharedCallable(signature) => Scalar::SharedCallable(signature.iter().map(scalar).collect()),
        PlanType::ClosureOnce(signature) => Scalar::ClosureOnce(signature.iter().map(scalar).collect()),
        PlanType::ClosureRef(signature) => Scalar::ClosureRef(signature.iter().map(scalar).collect()),
        PlanType::BorrowingClosure(signature) => Scalar::BorrowingClosure(signature.iter().map(scalar).collect()),
        PlanType::BorrowingClosureRef(signature) => Scalar::BorrowingClosureRef(signature.iter().map(scalar).collect()),
        PlanType::FunctionItem(name, signature) => {
            Scalar::FunctionItem(name.clone(), signature.iter().map(scalar).collect())
        }
    }
}

/// Mirror tuple fields once through the plan's existing tuple-element conversion. The enclosing scalar retains the
/// distinction between owned tuples and shared or exclusive references; this helper changes no ownership policy.
fn tuple_scalars(elements: &[crate::plan::TupleElement]) -> Vec<Scalar> {
    elements
        .iter()
        .map(|element| scalar(&tuple_element_type(element.clone())))
        .collect()
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
    validate_declarations(plan, library)?;
    for function in &plan.functions {
        validate_callable(plan, function)?;
    }
    Ok(())
}

/// Collect each function's first adapter refusal after the shared declaration context has passed validation.
/// A declaration failure is returned alone because individual function checks require that context.
pub(crate) fn refusals(plan: &Plan, library: bool) -> Vec<PlanError> {
    if let Err(error) = validate_declarations(plan, library) {
        return vec![error];
    }
    plan.functions
        .iter()
        .filter_map(|function| match validate_callable(plan, function) {
            Ok(()) => None,
            Err(error @ PlanError::Invalid { .. }) => Some(error),
            Err(error) => Some(invalid(function, error.to_string())),
        })
        .collect()
}

/// Validate names, declarations, extern signatures and the entrypoint before any callable body is inspected.
fn validate_declarations(plan: &Plan, library: bool) -> Result<(), PlanError> {
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
    validate_generic_enums(plan)?;
    library_model_order(plan)?;
    crate::checked_json::validate_roots(plan)?;
    crate::checked_json::validate_source_roots(plan)?;
    validate_value_layouts(plan)?;
    validate_statics(plan)?;
    let mut paths = Vec::new();
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
        let path_key = (
            external.path.clone(),
            external.type_arguments.iter().map(scalar).collect::<Vec<_>>(),
        );
        if incan_semantics_core::body_ir::rust_item_identity(&external.path).is_none()
            || !segments
                .iter()
                .filter(|segment| **segment != "[]")
                .all(|segment| identifier(segment))
            || paths.contains(&path_key)
        {
            return Err(PlanError::Invalid {
                function: external.path.clone(),
                reason: "invalid or duplicate canonical external path".into(),
            });
        }
        paths.push(path_key);
        if external
            .parameters
            .iter()
            .any(|ty| !external_signature_type(&scalar(ty)))
            || (!matches!(external.return_type, PlanType::Never)
                && !external_signature_type(&scalar(&external.return_type)))
        {
            return Err(PlanError::Invalid {
                function: external.path.clone(),
                reason: "checked pairs and formatting arrays cannot cross a function signature".into(),
            });
        }
    }
    // A library unit has no entry point; an executable must have exactly the binary main Rust accepts.
    if !library && plan.programs.is_empty() {
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
    for name in &plan.programs {
        let entry = plan
            .functions
            .iter()
            .find(|function| function.name == *name)
            .ok_or_else(|| PlanError::UnknownCallee(name.clone()))?;
        if !entry.parameters.is_empty()
            || (scalar(&entry.return_type) != Scalar::Unit && !entry_result(plan, &entry.return_type))
        {
            return Err(invalid(
                entry,
                "selected program entry must take no parameters and return unit or a unit Result",
            ));
        }
    }
    Ok(())
}

/// Apply the same closure, callable-storage and body checks in ordinary compilation and diagnostic enumeration.
fn validate_callable(plan: &Plan, function: &Function) -> Result<(), PlanError> {
    validate_closure_constructor(function)?;
    crate::shared_callables::validate(function)?;
    validate_function(plan, function)
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
    declaration.carrier == Carrier::Result
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
/// Valid locations borrow their shared file handle; an owned refusal retains its own filename text.
fn span(source: &SourceSpan) -> Result<(), PlanError> {
    if source.file.is_empty() || source.line < 1 || source.column < 1 {
        return Err(PlanError::Invalid {
            function: source.file.as_str().to_owned(),
            reason: "source span requires a file and positive line and column".into(),
        });
    }
    Ok(())
}

#[cfg(test)]
#[path = "validation/scalar_inputs_tests.rs"]
mod scalar_inputs_tests;
