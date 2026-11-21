//! Resolve complete checked carrier and collection types against one retained native layout context.

use incan_lang::lang::types::collections::{CollectionTypeId, as_str as collection_name, from_str as collection_type};
use incan_lang::lang::types::ownership::{OwnershipMode, from_str as ownership_mode};
use incan_mir_plan_native::{
    Carrier, EnumDeclaration, ListLeaf, PlanType, TupleElement, carrier_index, carrier_position, is_carrier_index,
    tuple_element_type,
};
use incan_semantics_core::body_ir::{EnumDeclaration as SourceEnumDeclaration, NominalDeclaration};
use incan_semantics_core::{
    IncanCallableParam, IncanCallableParamKind, IncanType, RuntimeBuiltinType, SemanticSourceTargetKind,
};

use crate::closures::semantic_input;
use crate::compiler_record_adapters::{authored_plan_symbol_id, semantic_key_matches_native};
use crate::decls::{library_enum_item, library_item};
use crate::enums::enum_type;
use crate::lists::{exclusive_reference, list_parameter_type};
use crate::models::{hashed_element, model_type};
use crate::native_type_identity::{ConstructorIndex, OWNED_CALLABLE_STORAGE, constructor_index};
use crate::numerics::sized_numeric;
use crate::tuples::tuple_element;

/// Require a retained nominal carrier and unchanged loan mode; rustc proves exact type equality before any move.
pub fn native_alias_boundary(nominals: &[NominalDeclaration], source: &PlanType, target: &PlanType) -> bool {
    if alias_reference_mode(source) != alias_reference_mode(target) {
        return false;
    }
    retained_nominal_carrier(nominals, source) || retained_nominal_carrier(nominals, target)
}

/// Keep shared and exclusive loans distinct from owners when forwarding exact native aliases.
fn alias_reference_mode(ty: &PlanType) -> i64 {
    match ty {
        PlanType::ModelRef(..)
        | PlanType::EnumRef(..)
        | PlanType::TupleRef(_)
        | PlanType::ZipIteratorRef(..)
        | PlanType::ListRef(..)
        | PlanType::SetRef(_)
        | PlanType::DictRef(..)
        | PlanType::ScalarRef(_)
        | PlanType::SliceRef(_)
        | PlanType::StringRef
        | PlanType::StrRef
        | PlanType::FrozenCollectionRef(..) => 1,
        PlanType::ModelMutRef(..)
        | PlanType::EnumMutRef(..)
        | PlanType::TupleMutRef(_)
        | PlanType::ZipIteratorMutRef(..)
        | PlanType::ListMutRef(..)
        | PlanType::SetMutRef(_)
        | PlanType::DictMutRef(..)
        | PlanType::ScalarMutRef(_)
        | PlanType::SliceMutRef(_)
        | PlanType::StringMutRef
        | PlanType::GeneratorMutRef(..) => 2,
        _ => 0,
    }
}

/// Require a compiled nominal alias or a closed source template whose consumer owns its native declaration.
fn retained_nominal_carrier(nominals: &[NominalDeclaration], ty: &PlanType) -> bool {
    match ty {
        PlanType::Model(owner, _) | PlanType::ModelRef(owner, _) | PlanType::ModelMutRef(owner, _) => {
            let declaration = &nominals[*owner as usize];
            if !library_item(declaration).is_empty() {
                return true;
            }
            declaration.type_parameter_count != 0
        }
        _ => false,
    }
}

/// Resolve a callable component without source declarations.
pub fn context_free_type(ty: &IncanType) -> Result<PlanType, String> {
    let constructors = constructor_index(&[]);
    native_source_type(&constructors, &[], &[], &[], ty, &[], "")
}

/// Use retained carrier identity before recursively resolving collection elements; never reopen source syntax.
pub fn native_source_type(
    constructors: &ConstructorIndex,
    nominals: &[NominalDeclaration],
    enums: &[SourceEnumDeclaration],
    unions: &[IncanType],
    ty: &IncanType,
    intrinsics: &[EnumDeclaration],
    source_file: &str,
) -> Result<PlanType, String> {
    let resolve = |inner: &IncanType, file: &str| {
        native_source_type(constructors, nominals, enums, unions, inner, intrinsics, file)
    };
    // Only Option, Result and Union carriers are intrinsic; each borrowed carrier compares its key in place.
    if intrinsic_carrier(ty) {
        for (index, declaration) in intrinsics.iter().enumerate() {
            if semantic_key_matches_native(&declaration.semantic_key, ty)? {
                return Ok(PlanType::Enum(carrier_index(index as i64), declaration.name.clone()));
            }
        }
    }
    if let Some(result) = enum_type(enums, ty) {
        return Ok(result);
    }
    match ty {
        IncanType::NativeCChar => return Ok(PlanType::CChar),
        IncanType::NativePointer { mutable, pointee } => {
            return Ok(PlanType::RawPointer(*mutable, vec![resolve(pointee, source_file)?]));
        }
        IncanType::NativeTraitObject { path, send, sync } => {
            return Ok(PlanType::NativeTraitObject(path.clone(), *send, *sync));
        }
        IncanType::NativeArray { element, length } => {
            let count = *length;
            if count > 9_223_372_036_854_775_807u64 {
                return Err("unsupported Body IR native array length exceeds the plan integer range".into());
            }
            return Ok(PlanType::NativeArray(
                vec![resolve(element, source_file)?],
                count as i64,
            ));
        }
        IncanType::RustInteropPath(path) => {
            if path == "str" {
                return Ok(PlanType::NativeStr);
            }
        }
        IncanType::Ref(inner) => {
            if is_native_str(inner) {
                return Ok(PlanType::StrRef);
            }
            return shared_receiver_type(resolve(inner, source_file)?);
        }
        IncanType::RefMut(inner) => return mutable_receiver_type(resolve(inner, source_file)?),
        IncanType::TypeToken(represented) => {
            return Ok(PlanType::TypeToken(vec![resolve(represented, source_file)?]));
        }
        IncanType::Trait(identity) => {
            return Ok(PlanType::Trait(authored_plan_symbol_id(
                identity,
                source_file,
                &constructors.source_files,
                &constructors.package_versions,
            )?));
        }
        IncanType::Unsupported(description) => return Err(format!("unsupported Body IR type {description}")),
        IncanType::NativeBuiltin(RuntimeBuiltinType::ValidationError) => return Ok(PlanType::ValidationError),
        IncanType::Function { params, return_type } => {
            return source_function_type(constructors, nominals, enums, unions, params, return_type, intrinsics);
        }
        IncanType::Generic { base, args } => {
            if let Some(result) = generic_source_type(constructors, nominals, enums, unions, base, args, intrinsics)? {
                return Ok(result);
            }
        }
        IncanType::Tuple(elements) => {
            return layout_tuple_type(constructors, nominals, enums, unions, elements, intrinsics);
        }
        _ => {}
    }
    model_type(constructors, nominals, ty, "")
}

/// The generic constructors with a native layout of their own; anything else is a nominal or a refusal.
fn generic_source_type(
    constructors: &ConstructorIndex,
    nominals: &[NominalDeclaration],
    enums: &[SourceEnumDeclaration],
    unions: &[IncanType],
    base: &str,
    arguments: &[IncanType],
    intrinsics: &[EnumDeclaration],
) -> Result<Option<PlanType>, String> {
    let resolve = |inner: &IncanType| native_source_type(constructors, nominals, enums, unions, inner, intrinsics, "");
    if let Some(value) = explicit_ownership_type(constructors, nominals, enums, unions, base, arguments, intrinsics)? {
        return Ok(Some(value));
    }
    if base == OWNED_CALLABLE_STORAGE && arguments.len() == 1 {
        return match resolve(&arguments[0])? {
            PlanType::FunctionPointer(signature) => Ok(Some(PlanType::Closure(signature))),
            _ => Err("invalid owned callable storage signature".into()),
        };
    }
    if base == "[]" && arguments.len() == 1 {
        return Ok(Some(PlanType::NativeSlice(vec![resolve(&arguments[0])?])));
    }
    if base == "Awaitable" && arguments.len() == 1 {
        return Ok(Some(PlanType::Future(vec![resolve(&arguments[0])?])));
    }
    if base == "List" && arguments.len() == 1 {
        let element = resolve(&arguments[0])?;
        return Ok(Some(match element {
            PlanType::List(leaf, depth) => PlanType::List(leaf, depth + 1),
            other => PlanType::List(collection_leaf(&other)?, 1),
        }));
    }
    if matches!(
        base,
        "FrozenList" | "FrozenSet" | "FrozenDict" | "$FrozenList" | "$FrozenSet" | "$FrozenDict"
    ) {
        return frozen_collection_type(constructors, nominals, enums, unions, base, arguments, intrinsics).map(Some);
    }
    if base == "Set" && arguments.len() == 1 {
        return Ok(Some(PlanType::Set(hashed_leaf(
            constructors,
            nominals,
            enums,
            &arguments[0],
            intrinsics,
        )?)));
    }
    if base == "Dict" && arguments.len() == 2 {
        let key = hashed_leaf(constructors, nominals, enums, &arguments[0], intrinsics)?;
        let value = resolve(&arguments[1])?;
        return Ok(Some(PlanType::Dict(key, collection_leaf(&value)?)));
    }
    if base == "Iterator"
        && arguments.len() == 1
        && let Some(pair) = zip_pair_type(constructors, nominals, enums, unions, &arguments[0], intrinsics)
    {
        return Ok(Some(pair));
    }
    if (base == "Iterator" || base == "Generator") && arguments.len() == 1 {
        if let Some(iterator) = numeric_newtype_iterator(constructors, nominals, &arguments[0]) {
            return Ok(Some(iterator));
        }
        match resolve(&arguments[0])? {
            PlanType::Model(owner, name) => return Ok(Some(PlanType::Generator(ListLeaf::Model(owner, name), 0))),
            PlanType::Tuple(elements) => return Ok(Some(PlanType::Generator(ListLeaf::Tuple(elements), 0))),
            PlanType::Enum(owner, name) => return Ok(Some(PlanType::Generator(ListLeaf::Enum(owner, name), 0))),
            PlanType::List(leaf, depth) => return Ok(Some(PlanType::Generator(leaf, depth))),
            _ => {}
        }
    }
    if collection_type(base) == Some(CollectionTypeId::Tuple) {
        return layout_tuple_type(constructors, nominals, enums, unions, arguments, intrinsics).map(Some);
    }
    Ok(None)
}

/// Lower one complete callable signature through the same retained nominal and input-access context.
fn source_function_type(
    constructors: &ConstructorIndex,
    nominals: &[NominalDeclaration],
    enums: &[SourceEnumDeclaration],
    unions: &[IncanType],
    parameters: &[IncanCallableParam],
    result: &IncanType,
    intrinsics: &[EnumDeclaration],
) -> Result<PlanType, String> {
    let mut signature = Vec::with_capacity(parameters.len() + 1);
    for parameter in parameters {
        let input = native_source_type(constructors, nominals, enums, unions, &parameter.ty, intrinsics, "")?;
        signature.push(semantic_input(
            input,
            parameter.kind,
            parameter.is_mut,
            parameter.native_owned,
            parameter.input_access,
        )?);
    }
    signature.push(native_source_type(
        constructors,
        nominals,
        enums,
        unions,
        result,
        intrinsics,
        "",
    )?);
    Ok(PlanType::FunctionPointer(signature))
}

/// Lower one canonical ownership wrapper to the physical reference mode selected by its registry identity.
fn explicit_ownership_type(
    constructors: &ConstructorIndex,
    nominals: &[NominalDeclaration],
    enums: &[SourceEnumDeclaration],
    unions: &[IncanType],
    constructor: &str,
    arguments: &[IncanType],
    intrinsics: &[EnumDeclaration],
) -> Result<Option<PlanType>, String> {
    let Some(mode) = ownership_mode(constructor) else {
        return Ok(None);
    };
    if arguments.len() != 1 {
        return Err("invalid Body IR ownership mode arity".into());
    }
    let value = native_source_type(constructors, nominals, enums, unions, &arguments[0], intrinsics, "")?;
    Ok(Some(match mode {
        OwnershipMode::Borrowed => shared_receiver_type(value)?,
        OwnershipMode::BorrowedMut | OwnershipMode::Reference => mutable_receiver_type(value)?,
        OwnershipMode::Owned => value,
    }))
}

/// Retain native frozen payloads directly and source frozen text or bytes through their static storage ABI.
fn frozen_collection_type(
    constructors: &ConstructorIndex,
    nominals: &[NominalDeclaration],
    enums: &[SourceEnumDeclaration],
    unions: &[IncanType],
    constructor: &str,
    arguments: &[IncanType],
    intrinsics: &[EnumDeclaration],
) -> Result<PlanType, String> {
    let native = constructor.starts_with('$');
    let mut elements = Vec::with_capacity(arguments.len());
    for argument in arguments {
        let mut element = native_source_type(constructors, nominals, enums, unions, argument, intrinsics, "")?;
        if !native && element == PlanType::String {
            element = PlanType::StaticText;
        } else if !native && element == PlanType::List(ListLeaf::U8, 1) {
            element = PlanType::StaticBytes;
        }
        elements.push(element);
    }
    if native {
        return Ok(PlanType::FrozenCollection(constructor[1..].to_owned(), elements));
    }
    Ok(PlanType::FrozenCollection(constructor.to_owned(), elements))
}

/// Use the Body IR field's owning callable fact at every construction and projection of its closed owner.
///
/// A class field whose layout reaches its own class by value, as `next: Option[Node]` does, is stored boxed; the
/// checker has already refused a class that holds itself with no `Option` in between.
pub fn stored_field_type(
    constructors: &ConstructorIndex,
    nominals: &[NominalDeclaration],
    enums: &[SourceEnumDeclaration],
    unions: &[IncanType],
    declaration: &NominalDeclaration,
    slot: usize,
    intrinsics: &[EnumDeclaration],
) -> Result<PlanType, String> {
    let ty = native_source_type(
        constructors,
        nominals,
        enums,
        unions,
        &declaration.field_types[slot],
        intrinsics,
        "",
    )?;
    if declaration.owns_callable_field(slot as i64) {
        return match ty {
            PlanType::FunctionPointer(signature) => Ok(PlanType::Closure(signature)),
            _ => Err("invalid Body IR owned callable field without a function signature".into()),
        };
    }
    if declaration.canonical.kind == SemanticSourceTargetKind::Class {
        let mut path = Vec::new();
        if layout_reaches(
            constructors,
            nominals,
            enums,
            unions,
            intrinsics,
            declaration,
            &ty,
            &mut path,
        ) {
            return Ok(PlanType::Boxed(vec![ty]));
        }
    }
    Ok(ty)
}

/// Whether a stored value holds `owner` inline: through nominal fields, standard enum payloads and tuple elements.
#[allow(clippy::too_many_arguments)]
fn layout_reaches(
    constructors: &ConstructorIndex,
    nominals: &[NominalDeclaration],
    enums: &[SourceEnumDeclaration],
    unions: &[IncanType],
    intrinsics: &[EnumDeclaration],
    owner: &NominalDeclaration,
    ty: &PlanType,
    path: &mut Vec<i64>,
) -> bool {
    match ty {
        PlanType::Model(index, _) => {
            let reached = &nominals[*index as usize];
            if reached.canonical == owner.canonical {
                return true;
            }
            if path.contains(index) {
                return false;
            }
            path.push(*index);
            for field in &reached.field_types {
                if let Ok(stored) = native_source_type(constructors, nominals, enums, unions, field, intrinsics, "")
                    && layout_reaches(constructors, nominals, enums, unions, intrinsics, owner, &stored, path)
                {
                    return true;
                }
            }
            false
        }
        PlanType::Enum(index, _) => {
            if !is_carrier_index(*index) || carrier_position(*index) as usize >= intrinsics.len() {
                return false;
            }
            for variant in &intrinsics[carrier_position(*index) as usize].variants {
                for payload in &variant.fields {
                    if layout_reaches(constructors, nominals, enums, unions, intrinsics, owner, payload, path) {
                        return true;
                    }
                }
            }
            false
        }
        PlanType::Tuple(elements) => elements.iter().any(|element| {
            let stored = tuple_element_type(element);
            layout_reaches(constructors, nominals, enums, unions, intrinsics, owner, &stored, path)
        }),
        _ => false,
    }
}

fn is_native_str(ty: &IncanType) -> bool {
    matches!(ty, IncanType::RustInteropPath(path) if path == "str")
}

/// Borrow the exact retained carrier, preserving an existing reference as a sized referent for native calls.
pub fn shared_receiver_type(ty: PlanType) -> Result<PlanType, String> {
    match &ty {
        PlanType::Tuple(elements) => return Ok(PlanType::TupleRef(elements.clone())),
        PlanType::ZipIterator(left, right) => return Ok(PlanType::ZipIteratorRef(left.clone(), right.clone())),
        PlanType::StrRef
        | PlanType::StringRef
        | PlanType::StringMutRef
        | PlanType::ModelRef(..)
        | PlanType::ModelMutRef(..)
        | PlanType::EnumRef(..)
        | PlanType::EnumMutRef(..)
        | PlanType::TupleRef(_)
        | PlanType::TupleMutRef(_)
        | PlanType::ScalarRef(_)
        | PlanType::ScalarMutRef(_) => return Ok(PlanType::ScalarRef(vec![ty])),
        PlanType::FrozenCollection(name, elements) => {
            return Ok(PlanType::FrozenCollectionRef(name.clone(), elements.clone()));
        }
        PlanType::NativeArray(elements, length) => return Ok(PlanType::NativeArrayRef(elements.clone(), *length)),
        PlanType::SliceRef(_) => return Ok(PlanType::ScalarRef(vec![ty])),
        PlanType::StrArray(count) => return Ok(PlanType::StrArrayRef(*count)),
        PlanType::NativeSlice(elements) => return Ok(PlanType::SliceRef(elements.clone())),
        PlanType::FunctionPointer(_) | PlanType::NativeTraitObject(..) | PlanType::TypeToken(_) => {
            return Ok(PlanType::ScalarRef(vec![ty]));
        }
        PlanType::Model(index, name) => return Ok(PlanType::ModelRef(*index, name.clone())),
        PlanType::Enum(index, name) => return Ok(PlanType::EnumRef(*index, name.clone())),
        PlanType::ValidationError => return Ok(PlanType::ValidationErrorRef),
        PlanType::String => return Ok(PlanType::StringRef),
        _ => {}
    }
    let collection = list_parameter_type(&ty, false);
    if collection != ty {
        return Ok(collection);
    }
    if matches!(
        ty,
        PlanType::Int
            | PlanType::Float
            | PlanType::I8
            | PlanType::I16
            | PlanType::I32
            | PlanType::I128
            | PlanType::ISize
            | PlanType::USize
            | PlanType::U8
            | PlanType::U16
            | PlanType::U32
            | PlanType::U64
            | PlanType::U128
            | PlanType::F32
            | PlanType::F64
            | PlanType::Bool
            | PlanType::Unit
            | PlanType::Decimal
            | PlanType::FrozenStr
            | PlanType::FrozenBytes
            | PlanType::StaticText
            | PlanType::StaticBytes
    ) {
        return Ok(PlanType::ScalarRef(vec![ty]));
    }
    Err(format!("unsupported shared standard library receiver type {ty:?}"))
}

/// Reuse exclusive source parameter storage for native references, retaining scalar reference carriers explicitly.
pub fn mutable_receiver_type(ty: PlanType) -> Result<PlanType, String> {
    match &ty {
        PlanType::Generator(leaf, depth) => return Ok(PlanType::GeneratorMutRef(leaf.clone(), *depth)),
        PlanType::NativeArray(elements, length) => return Ok(PlanType::NativeArrayMutRef(elements.clone(), *length)),
        PlanType::NativeSlice(elements) => return Ok(PlanType::SliceMutRef(elements.clone())),
        _ => {}
    }
    if let Some(reference) = exclusive_reference(&ty) {
        return Ok(reference);
    }
    match shared_receiver_type(ty)? {
        PlanType::ScalarRef(elements) => Ok(PlanType::ScalarMutRef(elements)),
        _ => Err("unsupported Body IR mutable callable receiver carrier".into()),
    }
}

/// Translate checked callable parameter kinds into the same native collection storage their Body IR frames retain.
pub fn callable_parameter_type(
    constructors: &ConstructorIndex,
    nominals: &[NominalDeclaration],
    enums: &[SourceEnumDeclaration],
    unions: &[IncanType],
    parameter: &IncanCallableParam,
    intrinsics: &[EnumDeclaration],
) -> Result<PlanType, String> {
    let element = native_source_type(constructors, nominals, enums, unions, &parameter.ty, intrinsics, "")?;
    match parameter.kind {
        IncanCallableParamKind::Normal => Ok(element),
        IncanCallableParamKind::RestPositional => match element {
            PlanType::List(leaf, depth) => Ok(PlanType::List(leaf, depth + 1)),
            other => Ok(PlanType::List(collection_leaf(&other)?, 1)),
        },
        IncanCallableParamKind::RestKeyword => Ok(PlanType::Dict(ListLeaf::Str, collection_leaf(&element)?)),
    }
}

/// An integer newtype's exact owned generator leaf, through the same canonical layout as its list items.
fn numeric_newtype_iterator(
    constructors: &ConstructorIndex,
    nominals: &[NominalDeclaration],
    element: &IncanType,
) -> Option<PlanType> {
    let IncanType::Named(name) = element else {
        return None;
    };
    for (index, declaration) in nominals.iter().enumerate() {
        if declaration.name != *name || declaration.canonical.kind != SemanticSourceTargetKind::Newtype {
            continue;
        }
        if !declaration.type_parameters.is_empty() || declaration.fields != ["0"] || declaration.field_types.len() != 1
        {
            return None;
        }
        if declaration.field_identities != [declaration.canonical.clone()] {
            return None;
        }
        return match model_type(constructors, nominals, &declaration.field_types[0], "") {
            Ok(PlanType::Int) => Some(PlanType::Generator(ListLeaf::Model(index as i64, name.clone()), 0)),
            _ => None,
        };
    }
    None
}

/// Admit an enum set element or dictionary key only when its retained layout hashes.
fn hashed_leaf(
    constructors: &ConstructorIndex,
    nominals: &[NominalDeclaration],
    enums: &[SourceEnumDeclaration],
    ty: &IncanType,
    intrinsics: &[EnumDeclaration],
) -> Result<ListLeaf, String> {
    if let Some(found) = enum_type(enums, ty) {
        return match found {
            PlanType::Enum(index, name) => {
                let declaration = &enums[index as usize];
                if !library_enum_item(declaration).is_empty() {
                    return Err(format!("unsupported Body IR stdlib enum hashed key {name}"));
                }
                let derived = |derive: &str| declaration.derives.iter().any(|retained| retained == derive);
                if !derived("Eq") || !derived("Hash") {
                    return Err(format!(
                        "unsupported Body IR enum hashed key {name} without derived Eq and Hash"
                    ));
                }
                Ok(ListLeaf::Enum(index, name))
            }
            _ => Err(format!("unsupported Body IR enum hashed key {ty}")),
        };
    }
    if let IncanType::Generic { base, args } = ty
        && standard_carrier(base)
    {
        for argument in args {
            hashed_leaf(constructors, nominals, enums, argument, intrinsics)?;
        }
        for (index, carrier) in intrinsics.iter().enumerate() {
            if carrier.carrier != Carrier::Declared && semantic_key_matches_native(&carrier.semantic_key, ty)? {
                return Ok(ListLeaf::Enum(carrier_index(index as i64), carrier.name.clone()));
            }
        }
        return Err(format!("unsupported Body IR unretained carrier hashed key {ty}"));
    }
    hashed_element(nominals, ty)
}

/// Only Option, Result and Union can name a retained intrinsic carrier.
fn intrinsic_carrier(ty: &IncanType) -> bool {
    match ty {
        IncanType::Generic { base, .. } => standard_carrier(base) || ty.is_union(),
        _ => false,
    }
}

/// Whether a checked generic base names the standard Option or Result, read through the collection vocabulary.
pub fn standard_carrier(base: &str) -> bool {
    matches!(
        collection_type(base),
        Some(CollectionTypeId::Option | CollectionTypeId::Result)
    )
}

/// A standard carrier's canonical name; a declared enum has none.
pub fn carrier_name(carrier: &Carrier) -> Option<String> {
    match carrier {
        Carrier::Option => Some(collection_name(CollectionTypeId::Option).to_owned()),
        Carrier::Result => Some(collection_name(CollectionTypeId::Result).to_owned()),
        Carrier::Declared => None,
    }
}

/// Resolve each tuple element in the retained layout context, so a list or carrier element keeps its identity.
fn layout_tuple_type(
    constructors: &ConstructorIndex,
    nominals: &[NominalDeclaration],
    enums: &[SourceEnumDeclaration],
    unions: &[IncanType],
    elements: &[IncanType],
    intrinsics: &[EnumDeclaration],
) -> Result<PlanType, String> {
    if elements.len() == 1 {
        return Err("a one-element tuple cannot be represented as a scalar".into());
    }
    let mut types: Vec<TupleElement> = Vec::with_capacity(elements.len());
    for element in elements {
        let resolved = native_source_type(constructors, nominals, enums, unions, element, intrinsics, "")?;
        types.push(tuple_element(&resolved, element)?);
    }
    Ok(PlanType::Tuple(types))
}

/// Admit a checked pair iterator, such as `zip`'s, through its element tuple's retained layout.
fn zip_pair_type(
    constructors: &ConstructorIndex,
    nominals: &[NominalDeclaration],
    enums: &[SourceEnumDeclaration],
    unions: &[IncanType],
    element: &IncanType,
    intrinsics: &[EnumDeclaration],
) -> Option<PlanType> {
    match element {
        IncanType::Tuple(elements) => pair_layout(constructors, nominals, enums, unions, elements, intrinsics),
        IncanType::Generic { base, args } if collection_type(base) == Some(CollectionTypeId::Tuple) => {
            pair_layout(constructors, nominals, enums, unions, args, intrinsics)
        }
        _ => None,
    }
}

fn pair_layout(
    constructors: &ConstructorIndex,
    nominals: &[NominalDeclaration],
    enums: &[SourceEnumDeclaration],
    unions: &[IncanType],
    elements: &[IncanType],
    intrinsics: &[EnumDeclaration],
) -> Option<PlanType> {
    if elements.len() != 2 {
        return None;
    }
    match layout_tuple_type(constructors, nominals, enums, unions, elements, intrinsics) {
        Ok(PlanType::Tuple(pair)) => Some(PlanType::ZipIterator(pair[0].clone(), pair[1].clone())),
        _ => None,
    }
}

/// Keep exact source nominal and intrinsic carrier identities alongside the existing primitive leaves.
///
/// An owned list, set or dictionary held by another collection is a collection leaf.
pub fn collection_leaf(ty: &PlanType) -> Result<ListLeaf, String> {
    if let Some(kind) = sized_numeric(ty)
        && *ty != PlanType::U8
        && *ty != PlanType::USize
    {
        return Ok(ListLeaf::Numeric(kind));
    }
    Ok(match ty {
        PlanType::Int => ListLeaf::Int,
        PlanType::Float => ListLeaf::Float,
        PlanType::Bool => ListLeaf::Bool,
        PlanType::String => ListLeaf::Str,
        PlanType::FrozenStr => ListLeaf::FrozenStr,
        PlanType::FrozenBytes => ListLeaf::FrozenBytes,
        PlanType::FrozenCollection(name, elements) => ListLeaf::FrozenCollection(name.clone(), elements.clone()),
        PlanType::U8 => ListLeaf::U8,
        PlanType::USize => ListLeaf::USize,
        PlanType::Unit => ListLeaf::Unit,
        PlanType::Decimal => ListLeaf::Decimal,
        PlanType::Tuple(elements) => ListLeaf::Tuple(elements.clone()),
        PlanType::Model(owner, name) => ListLeaf::Model(*owner, name.clone()),
        PlanType::Enum(owner, name) => ListLeaf::Enum(*owner, name.clone()),
        PlanType::StringRef
        | PlanType::StringMutRef
        | PlanType::StrRef
        | PlanType::ScalarRef(_)
        | PlanType::ScalarMutRef(_)
        | PlanType::SliceRef(_)
        | PlanType::EnumRef(..)
        | PlanType::EnumMutRef(..)
        | PlanType::ModelRef(..)
        | PlanType::ModelMutRef(..) => ListLeaf::Reference(vec![ty.clone()]),
        PlanType::FunctionPointer(signature) => ListLeaf::FunctionPointer(signature.clone()),
        PlanType::TypeToken(represented) => ListLeaf::TypeToken(represented.clone()),
        PlanType::List(leaf, depth) => ListLeaf::List(vec![leaf.clone()], *depth),
        PlanType::Dict(key, value) => ListLeaf::Dict(vec![key.clone(), value.clone()]),
        PlanType::Set(element) => ListLeaf::Set(vec![element.clone()]),
        _ => return Err(format!("unsupported Body IR collection element representation {ty:?}")),
    })
}
