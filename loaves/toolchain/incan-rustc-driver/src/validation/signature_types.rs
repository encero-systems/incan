//! Scalar shapes admitted in source, external and callable-object signatures.

use super::*;

/// Report whether a local only lends its referent for reading, so nothing reached through it may be borrowed mutably.
pub(super) fn shared_reference(ty: &Scalar) -> bool {
    matches!(
        ty,
        Scalar::StringRef
            | Scalar::NativeArrayRef(_, _)
            | Scalar::TupleRef(_)
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
            | Scalar::BorrowingClosureRef(_)
    )
}

/// Source inputs additionally admit checked borrows of owned text and exact scalar referents, including exclusive
/// inputs. Owned return and stored-value admission remain governed by `source_signature_type` rather than this
/// parameter-only boundary.
pub(super) fn source_parameter_type(ty: &Scalar) -> bool {
    if let Scalar::ScalarRef(elements) | Scalar::ScalarMutRef(elements) = ty {
        return matches!(elements.as_slice(), [inner] if scalar_referent(inner));
    }
    source_signature_type(ty)
        || matches!(
            ty,
            Scalar::StringRef | Scalar::ModelRef(_) | Scalar::ModelMutRef(_) | Scalar::EnumRef(_)
        )
}

/// Preserve explicit shared element returns and checked exclusive model reborrows under the signature's existing
/// lifetime-contract guard.
pub(super) fn source_return_type(ty: &Scalar) -> bool {
    if let Scalar::ScalarRef(elements) = ty {
        return matches!(elements.as_slice(), [inner] if scalar_referent(inner));
    }
    source_signature_type(ty) || matches!(ty, Scalar::ModelRef(_) | Scalar::ModelMutRef(_) | Scalar::EnumRef(_))
}

/// Admit source storage, owned data, retained nominal values and callable signatures.
///
/// Exact native slice references may cross closed source parameters without becoming owned vectors. Function pointer
/// inputs retain the native-call borrowing contract, including scalar, text, collection and model references; results
/// keep the source contract for returned storage. Formatting-only views remain internal.
/// Metadata-validated native references retain their exact carrier, and uninhabited results preserve divergence.
pub(super) fn source_signature_type(ty: &Scalar) -> bool {
    if let Scalar::NativeArray(elements, length) = ty {
        return *length >= 0 && matches!(elements.as_slice(), [element] if source_signature_type(element));
    }
    if let Scalar::TypeToken(elements) = ty {
        return matches!(
            elements.as_slice(),
            [Scalar::Int | Scalar::Float | Scalar::Bool | Scalar::String]
        );
    }
    if let Scalar::SliceRef(elements) | Scalar::SliceMutRef(elements) = ty {
        return matches!(elements.as_slice(), [element] if source_signature_type(element));
    }

    if let Scalar::Future(outputs) = ty {
        return matches!(outputs.as_slice(), [output] if source_signature_type(output));
    }
    if let Scalar::Boxed(referents) = ty {
        return matches!(referents.as_slice(), [referent] if source_signature_type(referent));
    }
    if let Scalar::TupleRef(elements) | Scalar::TupleMutRef(elements) = ty {
        return source_signature_type(&Scalar::Tuple(elements.clone()));
    }
    if let Scalar::FunctionPointer(signature) = ty {
        let Some((result, inputs)) = signature.split_last() else {
            return false;
        };
        return source_signature_type(result) && inputs.iter().all(external_signature_type);
    }
    if let Scalar::Closure(signature)
    | Scalar::ClosureOnce(signature)
    | Scalar::ClosureRef(signature)
    | Scalar::BorrowingClosure(signature)
    | Scalar::BorrowingClosureRef(signature)
    | Scalar::NativeClosure(signature, _, _) = ty
    {
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
                    | Scalar::Dict(_, _)
                    | Scalar::Set(_)
                    | Scalar::Model(_)
                    | Scalar::Enum(_)
            ) || (matches!(
                element,
                Scalar::ScalarRef(_)
                    | Scalar::ScalarMutRef(_)
                    | Scalar::StrRef
                    | Scalar::StringRef
                    | Scalar::StringMutRef
                    | Scalar::EnumRef(_)
                    | Scalar::EnumMutRef(_)
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
                | Scalar::Never
                | Scalar::ZipIterator(_, _)
                | Scalar::ZipIteratorRef(_, _)
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
                | Scalar::FrozenCollectionRef(_, _)
                | Scalar::StaticText
                | Scalar::StaticBytes
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
pub(super) fn external_signature_type(ty: &Scalar) -> bool {
    if let Scalar::NativeArray(elements, length)
    | Scalar::NativeArrayRef(elements, length)
    | Scalar::NativeArrayMutRef(elements, length) = ty
    {
        return *length >= 0 && matches!(elements.as_slice(), [element] if native_sequence_element(element));
    }
    if let Scalar::SliceRef(elements) | Scalar::SliceMutRef(elements) = ty {
        return matches!(elements.as_slice(), [element] if native_payload_type(element));
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

/// The scalar reference carrier borrows primitive storage, a checked tuple, a function pointer, a Rust trait object or
/// a sized shared reference. Borrowing a reference preserves its referent and does not make that referent writable;
/// exclusive reference values do not acquire copying authority.
pub(super) fn scalar_referent(ty: &Scalar) -> bool {
    if shared_reference(ty) || matches!(ty, Scalar::NativeTraitObject(..)) {
        return true;
    }
    if matches!(ty, Scalar::FunctionPointer(_) | Scalar::Tuple(_)) {
        return source_signature_type(ty);
    }
    sized_numeric(ty).is_some()
        || matches!(
            ty,
            Scalar::Int
                | Scalar::Float
                | Scalar::Bool
                | Scalar::Decimal
                | Scalar::FrozenStr
                | Scalar::FrozenBytes
                | Scalar::StrRef
                | Scalar::Unit
                | Scalar::StaticText
                | Scalar::StaticBytes
        )
}

/// Callable objects accept scalars of any numeric carrier, text, owned models and enums, shared model and collection
/// views and the exclusive borrows a `mut` parameter receives as inputs; their result is a scalar, owned text, an owned
/// list, set or dictionary, an owned model, newtype or enum, or an owned generator, which a generator expression's
/// capturing step returns. Tuple inputs use the same recursively validated by-value layout as source function
/// parameters, including captured generator steps over enumerate and zip items. Collection results retain their full
/// layout and transfer ownership to the caller; borrowed collection results remain outside this signature contract.
/// Owned model inputs use the same recursively validated layout as ordinary by-value parameters; external metadata
/// still checks the exact native carrier and trait bounds.
pub(super) fn callable_object_signature(signature: &[Scalar]) -> bool {
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
                        | Scalar::ModelRef(_)
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
pub(super) fn callable_object_value(ty: &Scalar) -> bool {
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
