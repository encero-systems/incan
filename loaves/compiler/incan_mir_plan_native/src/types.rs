//! Plan value types and source spans: scalars, sized numerics, list leaves, tuple elements and their projections.

use serde::{Deserialize, Serialize};

use crate::record_types::CanonicalSymbolId;
use crate::span_file::SpanFile;

/// One tuple element's layout: a scalar, owned or frozen text, an owned collection, or a model or enum carrier.
/// A Copy function pointer, exact scalar, nominal or slice reference, or nested tuple retains its full layout.
///
/// A list keeps its leaf and depth, a dictionary its key/value leaves, and a set its element leaf. An enum carrier (an
/// `Option`, a `Result` or a source enum) keeps its retained declaration index and name, and a nested tuple keeps its
/// own element layout, exactly as the matching [`PlanType`] does. A model retains its declaration identity, and a
/// function pointer retains its exact signature.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum TupleElement {
    Int,
    Float,
    Numeric(SizedNumeric),
    Bool,
    Unit,
    String,
    List(ListLeaf, i64),
    Dict(ListLeaf, ListLeaf),
    Set(ListLeaf),
    Model(i64, String),
    Enum(i64, String),
    Tuple(Vec<TupleElement>),
    FunctionPointer(Vec<PlanType>),
    Reference(bool, Vec<PlanType>),
    ModelRef(i64, String),
    ModelMutRef(i64, String),
    SliceRef(Vec<PlanType>),
    SliceMutRef(Vec<PlanType>),
    FrozenStr,
    /// Exact native frozen storage remains Copy inside a tuple, including nested frozen collections.
    StaticText,
    StaticBytes,
    FrozenBytes,
    FrozenCollection(String, Vec<PlanType>),
}

/// One-based location retaining a shared file handle, so copying a span never copies its path text. The handle
/// serializes as its path, and decoding a plan restores one shared handle per file.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceSpan {
    pub file: SpanFile,
    pub line: i64,
    pub column: i64,
}

/// Scalar, flat tuple, Copy function pointer, and exact retained nominal/carrier leaves; hash keys still require
/// admitted Eq and Hash authority.
///
/// `List`, `Set` and `Dict` are owned collection values held by another collection, such as a dictionary's list values
/// or a list's dictionaries. A list of lists keeps one leaf with a greater depth, so `List` never leads a
/// [`PlanType::List`]. `FrozenStr` and `FrozenBytes` keep dependency-owned Copy wrappers; ordinary strings and bytes
/// own their read data. A variant cannot hold its own type directly, so `List` holds its element leaf as the only item
/// of a list, `Set` holds one element leaf, and `Dict` holds its key and value leaves, in that order, as the two items
/// of one.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ListLeaf {
    Int,
    Float,
    Bool,
    Str,
    Tuple(Vec<TupleElement>),
    Model(i64, String),
    Enum(i64, String),
    U8,
    Unit,
    Decimal,
    FrozenStr,
    FrozenBytes,
    /// An immutable collection wrapper stored as one element of an ordinary owned collection.
    FrozenCollection(String, Vec<PlanType>),
    FunctionPointer(Vec<PlanType>),
    /// A zero-sized type token preserves its represented checked carrier when stored in a collection.
    TypeToken(Vec<PlanType>),
    List(Vec<ListLeaf>, i64),
    Dict(Vec<ListLeaf>),
    Set(Vec<ListLeaf>),
    /// Preserve the native pointer-width unsigned carrier in collection storage.
    USize,
    /// Exact-width numeric storage uses the same carrier table as scalar overflow results.
    Numeric(SizedNumeric),
    /// A local collection may hold an exact shared or exclusive reference without copying its referent.
    Reference(Vec<PlanType>),
}

/// Exact sized carrier retained by an overflow pair.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum SizedNumeric {
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

/// Scalar and source nominal types plus the owned text, fixed arrays, and shared slices used by formatting.
///
/// `EnumTag` is rustc's native discriminant type and stays body-internal; `TagToInt` explicitly converts it to source
/// int. Sized numeric variants preserve their exact Rust carriers. `CheckedNumeric` stores its named sized carrier; its
/// value projection retains the carrier type. Tuple elements retain their ordered types and own any string elements.
/// Array lengths are exact nonnegative counts. `CheckedInt` and arrays are body temporaries; shared slice variants
/// already include their reference.
///
/// `StringRef` borrows an owned String, `StringMutRef` is the exclusive borrow a `mut str` parameter receives, and
/// `StrRef` is a string literal's shared str view. `ScalarRef` and `ScalarMutRef` borrow one sized scalar, checked
/// tuple, function pointer or shared reference with explicit shared or exclusive intent. `List` stores its leaf element
/// and a positive nesting depth; `ListRef` and `ListMutRef` borrow that same Vec representation. `ModelRef`,
/// `ModelMutRef`, `EnumMutRef` and `TupleMutRef` are the shared and exclusive nominal borrows a receiver, a `mut`
/// parameter or an in-place loop item holds. `FunctionPointer` retains its inputs followed by exactly one return
/// entry; `FunctionItem` keeps that signature and the proven source declaration's name. `Closure` owns a boxed
/// callable object holding its captures by value; `BorrowingClosure` holds shared references to its captures.
/// `CallableArguments` is the body-internal argument tuple of a closure call.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum PlanType {
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
    Bool,
    Unit,
    CheckedNumeric(SizedNumeric),
    /// A checked diverging result has no value and gives its native call no normal continuation.
    Never,
    /// A source-free executor callback; ordinary source callable values stay outside this carrier.
    UnitFunction,
    EnumTag,
    CheckedInt,
    Tuple(Vec<TupleElement>),
    String,
    TupleRef(Vec<TupleElement>),
    TupleMutRef(Vec<TupleElement>),
    StringRef,
    /// Exactly one primitive, frozen scalar or sized shared reference; the shared borrow owns no storage.
    ScalarRef(Vec<PlanType>),
    ScalarMutRef(Vec<PlanType>),
    StringMutRef,
    Decimal,
    FrozenStr,
    FrozenBytes,
    /// Dependency-owned frozen collection and its exact static element types; dictionaries carry key then value.
    FrozenCollection(String, Vec<PlanType>),
    /// A shared view of the same baked storage; comparisons never consume or clone its elements.
    FrozenCollectionRef(String, Vec<PlanType>),
    /// The existing SDK's zero-sized marker with exactly one checked represented type.
    TypeToken(Vec<PlanType>),
    /// Exact static string storage, including generic arguments, distinct from a borrowed native `StrRef` view.
    StaticText,
    StaticBytes,
    /// Dependency-owned builtin validation storage; the reference borrows its message and optional code.
    ValidationError,
    ValidationErrorRef,
    StrRef,
    StringArray(i64),
    StrArray(i64),
    StringArrayRef(i64),
    StrArrayRef(i64),
    Enum(i64, String),
    EnumRef(i64, String),
    EnumMutRef(i64, String),
    Model(i64, String),
    ModelRef(i64, String),
    ModelMutRef(i64, String),
    StringSlice,
    StrSlice,
    /// A homogeneous list: its admitted leaf element and nesting depth.
    List(ListLeaf, i64),
    ListRef(ListLeaf, i64),
    ListMutRef(ListLeaf, i64),
    /// Concrete `Vec::IntoIter` owns the checked list's items; its mutable view polls without Clone.
    ListIntoIter(ListLeaf, i64),
    ListIntoIterMutRef(ListLeaf, i64),
    Set(ListLeaf),
    SetRef(ListLeaf),
    SetMutRef(ListLeaf),
    Dict(ListLeaf, ListLeaf),
    DictRef(ListLeaf, ListLeaf),
    DictMutRef(ListLeaf, ListLeaf),
    /// A source range value: four Copy scalar fields in declared range field order.
    RangeValue,
    /// Lazy zip over independent owned list snapshots; references retain poll authority.
    ZipIterator(TupleElement, TupleElement),
    ZipIteratorRef(TupleElement, TupleElement),
    ZipIteratorMutRef(TupleElement, TupleElement),
    /// Lazy runtime generators retain the admitted immediate element representation.
    Generator(ListLeaf, i64),
    GeneratorMutRef(ListLeaf, i64),
    GeneratorYield(ListLeaf, i64),
    GeneratorYieldRef(ListLeaf, i64),
    FunctionPointer(Vec<PlanType>),
    FunctionItem(String, Vec<PlanType>),
    Closure(Vec<PlanType>),
    /// A Rust callback environment retains exactly its checked Send and Sync obligations.
    NativeClosure(Vec<PlanType>, bool, bool),
    /// A checked nongeneric Rust trait principal and its retained Send/Sync bounds.
    NativeTraitObject(String, bool, bool),
    /// A one-argument SDK callable shares an immutable primitive-capture environment.
    SharedCallable(Vec<PlanType>),
    /// An owned FnOnce object whose invocation transfers its environment instead of borrowing it.
    ClosureOnce(Vec<PlanType>),
    ClosureRef(Vec<PlanType>),
    /// An owned callable object holding shared references to its borrowed captures; it is neither cloned nor returned.
    BorrowingClosure(Vec<PlanType>),
    BorrowingClosureRef(Vec<PlanType>),
    CallableArguments(Vec<PlanType>),
    /// Exactly one output carrier; the owned pinned future executes only when polled.
    Future(Vec<PlanType>),
    /// Rust's unsized str is permitted as a native type argument; source text remains owned String.
    NativeStr,
    /// An unsized native slice has one exact element carrier; references retain their own shared/exclusive intent.
    NativeSlice(Vec<PlanType>),
    SliceRef(Vec<PlanType>),
    SliceMutRef(Vec<PlanType>),
    /// A native array preserves one element carrier and its checker-evaluated fixed length.
    NativeArray(Vec<PlanType>, i64),
    NativeArrayRef(Vec<PlanType>, i64),
    NativeArrayMutRef(Vec<PlanType>, i64),
    /// Exact target C character category, used by verified foreign pointer signatures.
    CChar,
    /// A compiler-managed raw pointer with exactly one pointee carrier; source arithmetic never creates one.
    RawPointer(bool, Vec<PlanType>),
    /// A class field that reaches its own class by value is stored on the heap: exactly one owned referent.
    Boxed(Vec<PlanType>),
    /// Canonical source-trait identity admitted only as a `TypeToken`'s represented type. This remains last so older
    /// serialized plan discriminants keep their existing values.
    Trait(CanonicalSymbolId),
}

/// Recover one admitted leaf's exact representation for lowering and adapter consumers.
///
/// A nested collection leaf holds its element leaves as list items; a malformed leaf with a missing item becomes the
/// unit type rather than panicking.
pub fn list_leaf_type(leaf: &ListLeaf) -> PlanType {
    match leaf {
        ListLeaf::Int => PlanType::Int,
        ListLeaf::Float => PlanType::Float,
        ListLeaf::Bool => PlanType::Bool,
        ListLeaf::Str => PlanType::String,
        ListLeaf::Tuple(elements) => PlanType::Tuple(elements.clone()),
        ListLeaf::Model(index, name) => PlanType::Model(*index, name.clone()),
        ListLeaf::Enum(index, name) => PlanType::Enum(*index, name.clone()),
        ListLeaf::U8 => PlanType::U8,
        ListLeaf::USize => PlanType::USize,
        ListLeaf::Numeric(kind) => sized_plan_type(kind),
        ListLeaf::Unit => PlanType::Unit,
        ListLeaf::Decimal => PlanType::Decimal,
        ListLeaf::FrozenStr => PlanType::FrozenStr,
        ListLeaf::FrozenBytes => PlanType::FrozenBytes,
        ListLeaf::FrozenCollection(name, elements) => PlanType::FrozenCollection(name.clone(), elements.clone()),
        ListLeaf::FunctionPointer(signature) => PlanType::FunctionPointer(signature.clone()),
        ListLeaf::TypeToken(represented) => PlanType::TypeToken(represented.clone()),
        ListLeaf::List(element, depth) => match element.first() {
            Some(first) => PlanType::List(first.clone(), *depth),
            None => PlanType::Unit,
        },
        ListLeaf::Dict(entry) => match (entry.first(), entry.get(1)) {
            (Some(key), Some(value)) => PlanType::Dict(key.clone(), value.clone()),
            _ => PlanType::Unit,
        },
        ListLeaf::Set(element) => match element.first() {
            Some(first) => PlanType::Set(first.clone()),
            None => PlanType::Unit,
        },
        ListLeaf::Reference(element) => match element.first() {
            Some(first) => first.clone(),
            None => PlanType::Unit,
        },
    }
}

/// Recover the exact scalar carrier shared by collection leaves and sized overflow pairs.
pub fn sized_plan_type(kind: &SizedNumeric) -> PlanType {
    match kind {
        SizedNumeric::I8 => PlanType::I8,
        SizedNumeric::I16 => PlanType::I16,
        SizedNumeric::I32 => PlanType::I32,
        SizedNumeric::I128 => PlanType::I128,
        SizedNumeric::U8 => PlanType::U8,
        SizedNumeric::U16 => PlanType::U16,
        SizedNumeric::U32 => PlanType::U32,
        SizedNumeric::U64 => PlanType::U64,
        SizedNumeric::U128 => PlanType::U128,
        SizedNumeric::F32 => PlanType::F32,
        SizedNumeric::F64 => PlanType::F64,
        SizedNumeric::ISize => PlanType::ISize,
        SizedNumeric::USize => PlanType::USize,
    }
}

/// Return `RangeValue`'s native field carriers in declared range field order: start, end, step, then inclusivity.
pub fn range_field_types() -> Vec<PlanType> {
    vec![PlanType::Int, PlanType::Int, PlanType::Int, PlanType::Bool]
}

/// Recover a tuple field's exact local type from the admitted layout.
pub fn tuple_element_type(element: &TupleElement) -> PlanType {
    match element {
        TupleElement::Int => PlanType::Int,
        TupleElement::Float => PlanType::Float,
        TupleElement::Numeric(kind) => sized_plan_type(kind),
        TupleElement::Bool => PlanType::Bool,
        TupleElement::Unit => PlanType::Unit,
        TupleElement::String => PlanType::String,
        TupleElement::List(leaf, depth) => PlanType::List(leaf.clone(), *depth),
        TupleElement::Dict(key, value) => PlanType::Dict(key.clone(), value.clone()),
        TupleElement::Set(leaf) => PlanType::Set(leaf.clone()),
        TupleElement::Model(owner, name) => PlanType::Model(*owner, name.clone()),
        TupleElement::Enum(owner, name) => PlanType::Enum(*owner, name.clone()),
        TupleElement::Tuple(elements) => PlanType::Tuple(elements.clone()),
        TupleElement::FunctionPointer(signature) => PlanType::FunctionPointer(signature.clone()),
        TupleElement::Reference(mutable, elements) => reference_type(*mutable, elements),
        TupleElement::ModelRef(index, name) => PlanType::ModelRef(*index, name.clone()),
        TupleElement::ModelMutRef(index, name) => PlanType::ModelMutRef(*index, name.clone()),
        TupleElement::SliceRef(elements) => PlanType::SliceRef(elements.clone()),
        TupleElement::SliceMutRef(elements) => PlanType::SliceMutRef(elements.clone()),
        TupleElement::FrozenStr => PlanType::FrozenStr,
        TupleElement::StaticText => PlanType::StaticText,
        TupleElement::StaticBytes => PlanType::StaticBytes,
        TupleElement::FrozenBytes => PlanType::FrozenBytes,
        TupleElement::FrozenCollection(name, elements) => PlanType::FrozenCollection(name.clone(), elements.clone()),
    }
}

/// The local type of a tuple element that borrows one referent, shared or exclusive.
///
/// A string and an enum have reference types of their own, and a native `str` is the string-view type; any other
/// referent is a scalar reference.
fn reference_type(mutable: bool, referent: &[PlanType]) -> PlanType {
    if let [only] = referent {
        match only {
            PlanType::String if mutable => return PlanType::StringMutRef,
            PlanType::String => return PlanType::StringRef,
            PlanType::Enum(owner, name) if mutable => return PlanType::EnumMutRef(*owner, name.clone()),
            PlanType::Enum(owner, name) => return PlanType::EnumRef(*owner, name.clone()),
            _ => {}
        }
    }
    if mutable {
        return PlanType::ScalarMutRef(referent.to_vec());
    }
    if referent == [PlanType::NativeStr] {
        return PlanType::StrRef;
    }
    PlanType::ScalarRef(referent.to_vec())
}

/// Own the file text behind a shared handle that remains valid after the creating scope returns.
pub fn source_span(file: &str, line: i64, column: i64) -> SourceSpan {
    SourceSpan {
        file: SpanFile::new(file.to_owned()),
        line,
        column,
    }
}

crate::structural_debug!(SourceSpan, "SourceSpan", file, line, column);
