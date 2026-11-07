//! Generic and concrete model, enum and static layouts, and library argument models.

use super::*;

/// Admit generic ADT binders and field types before AST injection, rejecting collisions with concrete layouts and
/// functions. Native layout verification separately validates every concrete alias against its instantiated ADT.
pub(super) fn validate_generic_models(plan: &Plan) -> Result<(), PlanError> {
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
                    derives::from_str(name),
                    Some(
                        DeriveId::Clone
                            | DeriveId::Debug
                            | DeriveId::Display
                            | DeriveId::PartialEq
                            | DeriveId::Eq
                            | DeriveId::PartialOrd
                            | DeriveId::Ord
                            | DeriveId::Hash
                    )
                ) && !(name.contains("::") && valid_library_item(name))
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

/// Admit generic enum templates independently of closed aliases; their variants remain source ordered and exact.
pub(super) fn validate_generic_enums(plan: &Plan) -> Result<(), PlanError> {
    let mut names: BTreeSet<_> = plan
        .functions
        .iter()
        .map(|value| value.name.as_str())
        .chain(plan.models.iter().map(|value| value.name.as_str()))
        .chain(plan.enums.iter().map(|value| value.name.as_str()))
        .chain(plan.generic_models.iter().map(|value| value.name.as_str()))
        .collect();
    for value in &plan.generic_enums {
        span(&value.span)?;
        let parameters = value.parameters.iter().map(String::as_str).collect::<BTreeSet<_>>();
        let mut variants = BTreeSet::new();
        if !identifier(&value.name)
            || !names.insert(&value.name)
            || parameters.is_empty()
            || parameters.len() != value.parameters.len()
            || parameters.iter().any(|name| !identifier(name))
            || value.variants.is_empty()
            || value.derives.iter().any(|name| {
                !matches!(
                    derives::from_str(name),
                    Some(
                        DeriveId::Copy
                            | DeriveId::Clone
                            | DeriveId::Debug
                            | DeriveId::PartialEq
                            | DeriveId::Eq
                            | DeriveId::PartialOrd
                            | DeriveId::Ord
                            | DeriveId::Hash
                    )
                ) && !(name.contains("::") && valid_library_item(name))
            })
            || value.variants.iter().any(|variant| {
                !identifier(&variant.name)
                    || !variants.insert(&variant.name)
                    || variant
                        .fields
                        .iter()
                        .any(|field| !generic_field_type(field, &parameters))
            })
        {
            return Err(PlanError::Invalid {
                function: value.name.clone(),
                reason: "invalid generic enum declaration".into(),
            });
        }
    }
    Ok(())
}

/// A generic field may mention only its declared binders, complete concrete carriers, and explicit nominal paths.
pub(super) fn generic_field_type(ty: &GenericLayoutType, parameters: &BTreeSet<&str>) -> bool {
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
/// Function-pointer fields keep Copy storage, while reusable captured callables own their environments. Borrowed
/// and consuming callable fields require separate lifetime and invocation contracts and remain refused.
pub(super) fn validate_models(plan: &Plan) -> Result<(), PlanError> {
    let mut names: BTreeSet<_> = plan.functions.iter().map(|function| function.name.clone()).collect();
    for declaration in &plan.models {
        span(&declaration.span)?;
        let tuple = declaration.fields.len() == 1 && declaration.fields[0].name == "0";
        // A native Copy capability is verified against the linked type; no derives are synthesized on its alias.
        let derives_valid = if declaration.library_item.is_empty() {
            valid_nominal_derives(&declaration.derives, tuple)
        } else {
            declaration.derives.is_empty() || declaration.derives == ["Copy"]
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
            || declaration.view_lifetime_count < 0
            || (!declaration.library_item.is_empty() && declaration.view_lifetime_count != 0)
        {
            return Err(PlanError::Invalid {
                function: declaration.name.clone(),
                reason: "invalid plain model declaration".into(),
            });
        }
        for argument in &declaration.library_type_arguments {
            if !matches!(
                argument,
                PlanType::NativeStr | PlanType::NativeSlice(_) | PlanType::NativeTraitObject(..)
            ) && !external_signature_type(&scalar(argument))
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
        if !opaque_native_fields_valid(declaration) {
            return Err(PlanError::Invalid {
                function: declaration.name.clone(),
                reason: "invalid opaque native field slots".into(),
            });
        }
        for field in &declaration.fields {
            if declaration.library_item.is_empty()
                && declaration.view_lifetime_count == 0
                && borrowed_alias_in_source_type(plan, &field.ty, &mut BTreeSet::new())
            {
                return Err(PlanError::Unsupported {
                    function: declaration.name.clone(),
                    reason: "native borrowed carrier in a source field requires a retained lifetime contract".into(),
                });
            }
            span(&field.span)?;
            // A closed generic carrier may retain native references when lowering supplied the exact hidden lifetime
            // inventory. A linked alias instead obtains that inventory from its exact metadata type.
            let field_type = scalar(&field.ty);
            let retained_view_field = (declaration.view_lifetime_count > 0 || !declaration.library_item.is_empty())
                && external_signature_type(&field_type);
            if (!tuple && !identifier(&field.name))
                || !fields.insert(&field.name)
                || (!source_signature_type(&field_type) && !retained_view_field)
            {
                return Err(PlanError::Invalid {
                    function: declaration.name.clone(),
                    reason: "invalid model field declaration".into(),
                });
            }
            if matches!(
                field.ty,
                PlanType::ClosureOnce(_)
                    | PlanType::ClosureRef(_)
                    | PlanType::BorrowingClosure(_)
                    | PlanType::BorrowingClosureRef(_)
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

/// Only a native layout keeps opaque field slots, each named once, within its fields and with a unit placeholder.
pub(super) fn opaque_native_fields_valid(declaration: &crate::plan::ModelDeclaration) -> bool {
    let slots = &declaration.unavailable_native_fields;
    (slots.is_empty() || !declaration.library_item.is_empty())
        && slots.iter().enumerate().all(|(position, slot)| {
            !slots[..position].contains(slot)
                && usize::try_from(*slot)
                    .ok()
                    .and_then(|index| declaration.fields.get(index))
                    .is_some_and(|field| field.ty == PlanType::Unit)
        })
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
pub(super) fn visit_library_arguments(
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
pub(super) fn sized_numeric(ty: &Scalar) -> Option<Numeric> {
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
pub(super) fn sized_integer(ty: &Scalar) -> bool {
    sized_numeric(&ty).is_some() && !matches!(ty, Scalar::F32 | Scalar::F64)
}

/// The ordinary and sized integer carriers, the only operands of bit operations.
pub(super) fn integer(ty: &Scalar) -> bool {
    *ty == Scalar::Int || sized_integer(ty)
}

/// Resolve a source enum layout by its validated plan index.
pub(super) fn enum_declaration(plan: &Plan, index: i64) -> Option<&crate::plan::EnumDeclaration> {
    usize::try_from(index).ok().and_then(|index| plan.enums.get(index))
}

/// Reject malformed enum layouts and unsupported derives; validate exact forward references before size-cycle checks.
/// Native Option/Result carriers and library enums retain their checked borrowed payloads; source-owned enum fields
/// remain limited to source signature types. Library Copy and Clone markers are capabilities verified against rustc,
/// not requests to synthesize derive implementations on the native alias.
pub(super) fn validate_enums(plan: &Plan) -> Result<(), PlanError> {
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
        if declaration.carrier != Carrier::Declared {
            let variants = &declaration.variants;
            let valid = match declaration.carrier {
                Carrier::Option => {
                    variants.len() == 2
                        && variants[0].name == constructors::as_str(ConstructorId::None)
                        && variants[0].fields.is_empty()
                        && variants[1].name == constructors::as_str(ConstructorId::Some)
                        && variants[1].fields.len() == 1
                }
                Carrier::Result => {
                    variants.len() == 2
                        && variants[0].name == constructors::as_str(ConstructorId::Ok)
                        && variants[0].fields.len() == 1
                        && variants[1].name == constructors::as_str(ConstructorId::Err)
                        && variants[1].fields.len() == 1
                }
                Carrier::Declared => false,
            };
            if !valid || declaration.semantic_key.is_none() || !declaration.name.starts_with("__IncanCarrier") {
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
                && (declaration.carrier != Carrier::Declared
                    || declaration
                        .derives
                        .iter()
                        .any(|derive| !matches!(derives::from_str(derive), Some(DeriveId::Clone | DeriveId::Copy)))
                    || !declaration.raw_kind.is_empty()))
            || declaration.derives.iter().any(|name| match derives::from_str(name) {
                Some(
                    DeriveId::Debug
                    | DeriveId::Clone
                    | DeriveId::Copy
                    | DeriveId::PartialEq
                    | DeriveId::Eq
                    | DeriveId::PartialOrd
                    | DeriveId::Ord
                    | DeriveId::Hash,
                ) => false,
                _ if matches!(name.as_str(), "serde::Serialize" | "serde::Deserialize") => {
                    declaration.carrier != Carrier::Declared || declaration.member_debug
                }
                _ => !(name.contains("::") && valid_library_item(name)),
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
                let native = declaration.carrier != Carrier::Declared || !declaration.library_item.is_empty();
                let field_type = scalar(field);
                if !(source_signature_type(&field_type) || (native && native_payload_type(&field_type))) {
                    return Err(PlanError::Invalid {
                        function: declaration.name.clone(),
                        reason: format!("unsupported enum payload carrier {field_type:?}"),
                    });
                }
            }
        }
    }
    Ok(())
}

/// Native payloads and their projected tuple and slice storage retain exact native signature carriers.
/// Source-owned enum admission remains separate; rustc still verifies the actual native payload type at every use.
pub(super) fn native_payload_type(ty: &Scalar) -> bool {
    if external_signature_type(ty) {
        return true;
    }
    match ty {
        Scalar::Tuple(elements) | Scalar::TupleRef(elements) | Scalar::TupleMutRef(elements) => {
            elements.iter().all(native_payload_type)
        }
        _ => false,
    }
}

/// Admit a member-delegating `Debug` only on a union layout: an ordinary enum that derives no `Debug` of its own and
/// whose every variant holds exactly the one member the generated impl formats.
pub(super) fn member_debug_layout(declaration: &crate::plan::EnumDeclaration) -> bool {
    declaration.carrier == Carrier::Declared
        && declaration.name.starts_with("__IncanUnion")
        && !declaration
            .derives
            .iter()
            .any(|name| derives::from_str(name) == Some(DeriveId::Debug))
        && declaration.variants.iter().all(|variant| variant.fields.len() == 1)
}

/// Require each computed static cell to own a distinct slot and to name a planned initializer that takes no
/// parameters and returns exactly the cell's value type.
pub(super) fn validate_statics(plan: &Plan) -> Result<(), PlanError> {
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
pub(super) fn raw_value_layout(declaration: &crate::plan::EnumDeclaration) -> bool {
    let kind_valid = match declaration.raw_kind.as_str() {
        "str" => true,
        "int" => declaration.raw_values.iter().all(|raw| raw.parse::<i64>().is_ok()),
        _ => false,
    };
    kind_valid
        && declaration.carrier == Carrier::Declared
        && !declaration.member_debug
        && declaration.raw_values.len() == declaration.variants.len()
        && declaration.variants.iter().all(|variant| variant.fields.is_empty())
        && declaration.raw_values.iter().collect::<BTreeSet<_>>().len() == declaration.raw_values.len()
}

/// Reject infinitely sized by-value nominal/enum layouts while allowing cycles behind collections and references.
pub(super) fn validate_value_layouts(plan: &Plan) -> Result<(), PlanError> {
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
pub(super) fn visit_value_layout(
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

/// Collect model aliases required by an argument before AST materialization. A native enum alias depends on its type
/// arguments, not its payload fields; a Box-linked native enum must not become an invented alias dependency cycle.
pub(super) fn library_argument_models(
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
                    if !declaration.library_item.is_empty() {
                        for argument in &declaration.library_type_arguments {
                            library_argument_models(plan, argument, enums, models)?;
                        }
                        return Ok(());
                    }
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
        | PlanType::NativeArray(signature, _)
        | PlanType::NativeArrayRef(signature, _)
        | PlanType::NativeArrayMutRef(signature, _)
        | PlanType::SliceRef(signature)
        | PlanType::SliceMutRef(signature)
        | PlanType::Closure(signature)
        | PlanType::NativeClosure(signature, _, _)
        | PlanType::ClosureOnce(signature)
        | PlanType::BorrowingClosure(signature)
        | PlanType::BorrowingClosureRef(signature)
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

/// The Option/Result carrier a function returns when the result carries a returned view's loan.
///
/// The declaration spells it as the standard enum over its payloads, so the retained lifetime contract binds each
/// payload reference; validation then checks the payloads instead of the alias. Without a retained contract the
/// carrier stays an alias whose erased loan has no binder, and is refused.
pub(crate) fn carried_output<'plan>(
    plan: &'plan Plan,
    function: &Function,
) -> Option<&'plan crate::plan::EnumDeclaration> {
    if function.signature_lifetimes.is_empty() {
        return None;
    }
    let PlanType::Enum(index, _) = &function.return_type else {
        return None;
    };
    let carrier = enum_declaration(plan, *index)?;
    if carrier.carrier == Carrier::Declared {
        return None;
    }
    crate::declarations::standard_carrier(carrier)
        .1
        .iter()
        .any(library_argument_is_borrowed)
        .then_some(carrier)
}

/// Find erased loans inside native aliases before a source signature or field could expose them without lifetime
/// binders. Local adapters remain subject to native signature and MIR borrow checking; this gate proves no loan scope.
pub(super) fn borrowed_alias_in_source_type(plan: &Plan, ty: &PlanType, seen: &mut BTreeSet<(bool, i64)>) -> bool {
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
                    ((declaration.carrier != Carrier::Declared || !declaration.library_item.is_empty())
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
        PlanType::Tuple(elements) | PlanType::TupleRef(elements) | PlanType::TupleMutRef(elements) => elements
            .iter()
            .any(|element| borrowed_alias_in_source_type(plan, &tuple_element_type(element.clone()), seen)),
        PlanType::NativeSlice(elements)
        | PlanType::NativeArray(elements, _)
        | PlanType::NativeArrayRef(elements, _)
        | PlanType::NativeArrayMutRef(elements, _)
        | PlanType::SliceRef(elements)
        | PlanType::SliceMutRef(elements)
        | PlanType::ScalarRef(elements)
        | PlanType::ScalarMutRef(elements)
        | PlanType::CallableArguments(elements)
        | PlanType::Future(elements)
        | PlanType::FunctionPointer(elements)
        | PlanType::FunctionItem(_, elements)
        | PlanType::Closure(elements)
        | PlanType::NativeClosure(elements, _, _)
        | PlanType::ClosureOnce(elements)
        | PlanType::ClosureRef(elements)
        | PlanType::BorrowingClosure(elements)
        | PlanType::BorrowingClosureRef(elements)
        | PlanType::SharedCallable(elements)
        | PlanType::FrozenCollection(_, elements)
        | PlanType::FrozenCollectionRef(_, elements)
        | PlanType::TypeToken(elements) => elements
            .iter()
            .any(|element| borrowed_alias_in_source_type(plan, element, seen)),
        _ => false,
    }
}

/// Identify a native reference held inside a generic argument, including tuple and collection storage. Callable
/// signatures have their own binders and do not make the enclosing native alias borrow those inputs.
pub(super) fn library_argument_is_borrowed(ty: &PlanType) -> bool {
    match ty {
        PlanType::Tuple(elements) => {
            return elements
                .iter()
                .any(|element| library_argument_is_borrowed(&tuple_element_type(element.clone())));
        }
        PlanType::List(leaf, _) | PlanType::Set(leaf) => {
            return checked_list_leaf_type(leaf).map_or(true, |element| library_argument_is_borrowed(&element));
        }
        PlanType::Dict(key, value) => {
            return [key, value].into_iter().any(|leaf| {
                checked_list_leaf_type(leaf).map_or(true, |element| library_argument_is_borrowed(&element))
            });
        }
        PlanType::NativeArray(elements, _)
        | PlanType::NativeSlice(elements)
        | PlanType::FrozenCollection(_, elements) => return elements.iter().any(library_argument_is_borrowed),
        _ => {}
    }
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
            | PlanType::BorrowingClosure(..)
            | PlanType::BorrowingClosureRef(..)
    )
}
