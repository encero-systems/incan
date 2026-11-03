//! Model hashing, nested carrier and retained type validation, frozen values, and derive rules.

use super::*;

/// Resolve a checked nominal index without signed conversions or ambient name lookup.
pub(super) fn model(plan: &Plan, index: i64) -> Option<&crate::plan::ModelDeclaration> {
    usize::try_from(index).ok().and_then(|index| plan.models.get(index))
}

/// Require local Hash and Eq witnesses, or defer a linked type's implementations to native callee-bound checks.
/// A library alias never synthesizes implementations; its owning rlib is the authority, as for native comparison.
pub(super) fn hashable_model(plan: &Plan, index: i64) -> bool {
    model(plan, index).is_some_and(|declaration| {
        !declaration.library_item.is_empty()
            || (declaration
                .derives
                .iter()
                .any(|derive| derives::from_str(derive) == Some(DeriveId::Hash))
                && (declaration
                    .derives
                    .iter()
                    .any(|derive| derives::from_str(derive) == Some(DeriveId::Eq))
                    || crate::comparison_bridges::has_equality(plan, index)))
    })
}

/// Admit a local Hash layout or a linked layout without inventing traits; native callee bounds remain required.
pub(super) fn hashed_model_layout(plan: &Plan, index: i64) -> bool {
    model(plan, index).is_some_and(|declaration| {
        !declaration.library_item.is_empty()
            || declaration
                .derives
                .iter()
                .any(|derive| derives::from_str(derive) == Some(DeriveId::Hash))
    })
}

/// Whether the planned enum at `index` hashes, the authority a hashed collection element needs.
///
/// A source enum must derive Eq and Hash. An `Option` or `Result` carrier aliases the standard type, which hashes when
/// every payload does; a payload enum must precede its carrier, as nested carriers are planned first, so the check
/// always terminates. A stdlib enum's derives belong to its compiled crate and never qualify.
pub(super) fn hashable_enum(plan: &Plan, index: i64) -> bool {
    let Some(declaration) = enum_declaration(plan, index) else {
        return false;
    };
    if !declaration.library_item.is_empty() {
        return false;
    }
    if declaration.carrier == Carrier::Declared {
        return [DeriveId::Eq, DeriveId::Hash].iter().all(|required| {
            declaration
                .derives
                .iter()
                .any(|derive| derives::from_str(derive) == Some(*required))
        });
    }
    declaration.carrier != Carrier::Declared
        && declaration
            .variants
            .iter()
            .flat_map(|variant| &variant.fields)
            .all(|field| hashable_payload(plan, &scalar(field), index))
}

/// Whether a carrier payload hashes: a nonfloating scalar or text, a tuple of those, or a hashable model or an
/// earlier hashable enum.
pub(super) fn hashable_payload(plan: &Plan, ty: &Scalar, carrier: i64) -> bool {
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
pub(super) fn checked_list_leaf_type(leaf: &ListLeaf) -> Result<PlanType, PlanError> {
    let valid = match leaf {
        ListLeaf::Reference(elements) => matches!(
            elements.as_slice(),
            [PlanType::EnumRef(..)
                | PlanType::ModelRef(..)
                | PlanType::StringRef
                | PlanType::StrRef
                | PlanType::ScalarRef(_)
                | PlanType::SliceRef(_)]
        ),
        ListLeaf::List(elements, _) => elements.len() == 1,
        ListLeaf::Set(elements) => elements.len() == 1,
        ListLeaf::Dict(elements) => elements.len() == 2,
        _ => true,
    };
    if !valid {
        return Err(PlanError::Invalid {
            function: "collection leaf".into(),
            reason: "collection leaf has invalid child arity or reference carrier".into(),
        });
    }
    Ok(list_leaf_type(leaf.clone()))
}

/// Whether a native array or slice may hold this element: any source signature type, or a sized text reference
/// (`[&str; N]`), which source signatures reach only through parameters.
pub(super) fn native_sequence_element(element: &Scalar) -> bool {
    source_signature_type(element) || matches!(element, Scalar::StrRef)
}

/// Validate retained nominal identities and the exact shape of nested and borrowed native carriers before MIR
/// construction.
pub(super) fn validate_model_type(plan: &Plan, ty: &PlanType) -> Result<(), PlanError> {
    if let PlanType::RawPointer(_, elements) = ty {
        let [pointee] = elements.as_slice() else {
            return Err(PlanError::Invalid {
                function: "raw pointer".into(),
                reason: "pointer requires one exact pointee".into(),
            });
        };
        return validate_model_type(plan, pointee);
    }
    if let PlanType::Boxed(elements) = ty {
        let [referent] = elements.as_slice() else {
            return Err(PlanError::Invalid {
                function: "boxed field".into(),
                reason: "a box holds exactly one referent".into(),
            });
        };
        return validate_model_type(plan, referent);
    }
    if let PlanType::NativeTraitObject(path, _, _) = ty
        && (path.is_empty() || !valid_library_item(path))
    {
        return Err(PlanError::Invalid {
            function: "native trait object".into(),
            reason: "invalid checked principal path".into(),
        });
    }
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

    validate_nested_carrier_components(plan, ty)?;
    validate_retained_type_declaration(plan, ty)
}

/// Validate nested carriers before the enclosing nominal or callable declaration. Element traversal retains source
/// order and exact borrowed/owned forms; leaf conversion follows the plan's existing arity validation.
pub(super) fn validate_nested_carrier_components(plan: &Plan, ty: &PlanType) -> Result<(), PlanError> {
    match ty {
        PlanType::NativeArray(elements, length)
        | PlanType::NativeArrayRef(elements, length)
        | PlanType::NativeArrayMutRef(elements, length) => {
            validate_native_array_type(plan, elements, *length)?;
        }
        PlanType::NativeSlice(elements) | PlanType::SliceRef(elements) | PlanType::SliceMutRef(elements) => {
            validate_native_slice_type(plan, elements)?;
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
                    reason: "scalar reference requires one admitted scalar, Rust trait object or reference referent"
                        .into(),
                });
            }
            for element in elements {
                validate_model_type(plan, element)?;
            }
        }
        PlanType::FrozenCollection(name, elements) | PlanType::FrozenCollectionRef(name, elements) => {
            let arity = match name.as_str() {
                name if [CollectionTypeId::FrozenList, CollectionTypeId::FrozenSet]
                    .iter()
                    .any(|id| name == collections::as_str(*id)) =>
                {
                    1
                }
                name if name == collections::as_str(CollectionTypeId::FrozenDict) => 2,
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
        PlanType::Tuple(elements) | PlanType::TupleRef(elements) | PlanType::TupleMutRef(elements) => {
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
    Ok(())
}

/// Require a single admitted element and nonnegative length before recursively validating a native array's carrier.
pub(super) fn validate_native_array_type(plan: &Plan, elements: &[PlanType], length: i64) -> Result<(), PlanError> {
    if length < 0 || elements.len() != 1 {
        return Err(PlanError::Invalid {
            function: "native array".into(),
            reason: "array requires one element type and a nonnegative length".into(),
        });
    }
    for element in elements {
        if !native_sequence_element(&scalar(element)) {
            return Err(PlanError::Unsupported {
                function: "native array".into(),
                reason: "unsupported array element".into(),
            });
        }
        validate_model_type(plan, element)?;
    }
    Ok(())
}

/// Check native slice arity and payload admission before recursively validating its retained element type.
pub(super) fn validate_native_slice_type(plan: &Plan, elements: &[PlanType]) -> Result<(), PlanError> {
    let [element] = elements else {
        return Err(PlanError::Invalid {
            function: "native slice".into(),
            reason: "slice requires one element".into(),
        });
    };
    if !native_payload_type(&scalar(element)) {
        return Err(PlanError::Unsupported {
            function: "native slice".into(),
            reason: format!("unsupported slice element {:?}", scalar(element)),
        });
    }
    validate_model_type(plan, element)
}

/// Verify the enclosing tuple, nominal or callable declaration after its nested carriers. Callable shape admission
/// still precedes recursive signature validation, and retained names must agree with their plan declarations.
pub(super) fn validate_retained_type_declaration(plan: &Plan, ty: &PlanType) -> Result<(), PlanError> {
    if matches!(ty, PlanType::Tuple(_)) && !native_payload_type(&scalar(ty)) {
        return Err(PlanError::Invalid {
            function: "tuple".into(),
            reason: "tuple elements must retain admitted source or native payload storage".into(),
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
                reason: format!(
                    "function item signature differs from its declaration: retained {actual:?}, declared {expected:?}"
                ),
            });
        }
    }
    if let PlanType::Closure(signature)
    | PlanType::NativeClosure(signature, _, _)
    | PlanType::ClosureOnce(signature)
    | PlanType::ClosureRef(signature)
    | PlanType::BorrowingClosure(signature)
    | PlanType::BorrowingClosureRef(signature)
    | PlanType::SharedCallable(signature) = ty
        && !callable_object_signature(&signature.iter().map(scalar).collect::<Vec<_>>())
    {
        return Err(PlanError::Invalid {
            function: "callable object".into(),
            reason: "callable-object signature requires admitted inputs and a final return type".into(),
        });
    }
    if let PlanType::Closure(signature)
    | PlanType::NativeClosure(signature, _, _)
    | PlanType::ClosureOnce(signature)
    | PlanType::ClosureRef(signature)
    | PlanType::BorrowingClosure(signature)
    | PlanType::BorrowingClosureRef(signature)
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
pub(super) fn declared_function_item_signature(plan: &Plan, name: &str) -> Result<Vec<Scalar>, PlanError> {
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
pub(super) fn validate_frozen_values(
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

/// Validate the shape-specific derive vocabulary without accepting duplicate selections or reflection on tuples.
/// A tuple newtype carries only the automatic derives its checker proved for the underlying value; owning a native
/// non-Clone value never requires a Clone selection. Debug follows the checker's selection, including private native
/// adapters and callable fields that omit it; the adapter never reconstructs that policy. Serde and Copy selections fit
/// either shape and remain subject to rustc's trait checks. Qualified macro paths are checked selections from native
/// dependency facts; rustc validates their derive namespace and expands them on the real type.
pub(super) fn valid_nominal_derives(derives: &[String], tuple: bool) -> bool {
    let required = [
        derives::as_str(DeriveId::Clone),
        derives::FIELD_INFO_DERIVE_NAME,
        derives::INCAN_CLASS_DERIVE_NAME,
    ];
    (tuple
        || required
            .iter()
            .all(|required| derives.iter().any(|derive| derive == required)))
        && derives.iter().all(|derive| match derives::from_str(derive) {
            Some(
                DeriveId::Debug
                | DeriveId::Clone
                | DeriveId::Eq
                | DeriveId::PartialEq
                | DeriveId::Hash
                | DeriveId::Ord
                | DeriveId::PartialOrd
                | DeriveId::Default
                | DeriveId::Display
                | DeriveId::Copy,
            ) => true,
            _ if matches!(derive.as_str(), "serde::Serialize" | "serde::Deserialize") => true,
            _ if [derives::FIELD_INFO_DERIVE_NAME, derives::INCAN_CLASS_DERIVE_NAME].contains(&derive.as_str()) => {
                !tuple
            }
            _ => derive.contains("::") && valid_library_item(derive),
        })
        && derives.iter().collect::<BTreeSet<_>>().len() == derives.len()
}

/// A compiled layout names an item below its owning crate; a provider's own generic instantiations name `crate`.
pub(super) fn valid_library_item(library_item: &str) -> bool {
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
pub(super) fn valid_serde_names(declaration: &crate::plan::ModelDeclaration, tuple: bool) -> bool {
    declaration.field_serde_names.is_empty()
        || (!tuple
            && declaration.field_serde_names.len() == declaration.fields.len()
            && declaration
                .derives
                .iter()
                .any(|derive| matches!(derive.as_str(), "serde::Serialize" | "serde::Deserialize")))
}
