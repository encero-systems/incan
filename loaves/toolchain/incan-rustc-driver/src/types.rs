//! Mapping admitted plan types to the pinned rustc representation.

use crate::error::PlanError;
use crate::plan::{ListLeaf, PlanType, SizedNumeric, list_leaf_type, range_field_types, tuple_element_type};
use rustc_middle::ty::{ExistentialPredicateStableCmpExt as _, Ty, TyCtxt};

/// Construct an unsized trait object from its principal arguments followed by declaration-ordered associated
/// equalities. Auto traits and const arguments require a richer checked carrier and refuse here.
pub(crate) fn native_trait_object<'tcx>(
    tcx: TyCtxt<'tcx>,
    principal: rustc_hir::def_id::DefId,
    arguments: &[PlanType],
) -> Result<Ty<'tcx>, PlanError> {
    let invalid = || PlanError::Unsupported {
        function: tcx.def_path_str(principal),
        reason: "native trait object requires a non-auto dyn-compatible principal and complete plain type arguments"
            .into(),
    };
    let parameters = &tcx.generics_of(principal).own_params;
    let associated = tcx
        .associated_item_def_ids(principal)
        .iter()
        .copied()
        .filter(|item| tcx.def_kind(*item) == rustc_hir::def::DefKind::AssocTy)
        .collect::<Vec<_>>();
    let parameter_count = parameters
        .iter()
        .filter(|parameter| matches!(parameter.kind, rustc_middle::ty::GenericParamDefKind::Type { .. }))
        .count()
        .saturating_sub(1);
    if tcx.trait_is_auto(principal)
        || !tcx.is_dyn_compatible(principal)
        || parameters
            .iter()
            .any(|parameter| matches!(parameter.kind, rustc_middle::ty::GenericParamDefKind::Const { .. }))
        || parameter_count + associated.len() != arguments.len()
    {
        return Err(invalid());
    }
    let arguments = arguments
        .iter()
        .map(|argument| native_type(tcx, argument))
        .collect::<Result<Vec<_>, _>>()?;
    let mut values = arguments[..parameter_count].iter();
    let mut principal_arguments: Vec<rustc_middle::ty::GenericArg<'tcx>> = Vec::new();
    for parameter in parameters.iter().filter(|parameter| parameter.index != 0) {
        match parameter.kind {
            rustc_middle::ty::GenericParamDefKind::Lifetime => principal_arguments.push(tcx.lifetimes.re_erased.into()),
            rustc_middle::ty::GenericParamDefKind::Type { .. } => {
                principal_arguments.push((*values.next().ok_or_else(invalid)?).into());
            }
            rustc_middle::ty::GenericParamDefKind::Const { .. } => return Err(invalid()),
        }
    }
    let mut predicates = vec![rustc_middle::ty::Binder::dummy(
        rustc_middle::ty::ExistentialPredicate::Trait(rustc_middle::ty::ExistentialTraitRef::new(
            tcx,
            principal,
            principal_arguments.iter().copied(),
        )),
    )];
    for (definition, value) in associated.iter().zip(&arguments[parameter_count..]) {
        predicates.push(rustc_middle::ty::Binder::dummy(
            rustc_middle::ty::ExistentialPredicate::Projection(rustc_middle::ty::ExistentialProjection::new(
                tcx,
                *definition,
                principal_arguments.iter().copied(),
                (*value).into(),
            )),
        ));
    }
    predicates.sort_by(|left, right| left.skip_binder().stable_cmp(tcx, &right.skip_binder()));
    let predicates = tcx.mk_poly_existential_predicates(&predicates);
    Ok(Ty::new_dynamic(tcx, predicates, tcx.lifetimes.re_erased))
}

/// Translate admitted scalar, model, collection, callable, and uninhabited types to their canonical native
/// representations.
///
/// List references preserve source parameter borrowing; checked pairs and other shared references are body-internal.
pub fn native_type<'tcx>(tcx: TyCtxt<'tcx>, ty: &PlanType) -> Result<Ty<'tcx>, PlanError> {
    Ok(match ty {
        PlanType::TypeToken(elements) => type_token_type(tcx, elements)?,
        PlanType::ScalarRef(elements) => scalar_reference_type(tcx, elements, false)?,
        PlanType::ScalarMutRef(elements) => scalar_reference_type(tcx, elements, true)?,
        PlanType::Future(arguments) => {
            let [output] = arguments.as_slice() else {
                return Err(PlanError::Unsupported {
                    function: "future".into(),
                    reason: "future output arity".into(),
                });
            };
            enum_type(tcx, &crate::futures::alias_name(output))?
        }
        PlanType::ZipIterator(left, right) => zip_type(tcx, left, right)?,
        PlanType::ZipIteratorRef(left, right) => {
            Ty::new_imm_ref(tcx, tcx.lifetimes.re_erased, zip_type(tcx, left, right)?)
        }
        PlanType::ZipIteratorMutRef(left, right) => {
            Ty::new_mut_ref(tcx, tcx.lifetimes.re_erased, zip_type(tcx, left, right)?)
        }
        PlanType::EnumTag => tcx.types.isize,
        PlanType::Generator(leaf, depth) => generator_type(tcx, "Generator", leaf, *depth)?,
        PlanType::GeneratorMutRef(leaf, depth) => Ty::new_mut_ref(
            tcx,
            tcx.lifetimes.re_erased,
            generator_type(tcx, "Generator", leaf, *depth)?,
        ),
        PlanType::GeneratorYield(leaf, depth) => generator_type(tcx, "GeneratorYield", leaf, *depth)?,
        PlanType::GeneratorYieldRef(leaf, depth) => Ty::new_imm_ref(
            tcx,
            tcx.lifetimes.re_erased,
            generator_type(tcx, "GeneratorYield", leaf, *depth)?,
        ),
        PlanType::Enum(_, name) => enum_type(tcx, name)?,
        PlanType::EnumRef(_, name) => Ty::new_imm_ref(tcx, tcx.lifetimes.re_erased, enum_type(tcx, name)?),
        PlanType::EnumMutRef(_, name) => Ty::new_mut_ref(tcx, tcx.lifetimes.re_erased, enum_type(tcx, name)?),
        PlanType::Model(_, name) => model_type(tcx, name)?,
        PlanType::CallableArguments(inputs) => Ty::new_tup(
            tcx,
            &inputs
                .iter()
                .map(|input| native_type(tcx, input))
                .collect::<Result<Vec<_>, _>>()?,
        ),
        PlanType::Tuple(elements) => Ty::new_tup(
            tcx,
            &elements
                .iter()
                .map(|element| native_type(tcx, &tuple_element_type(element.clone())))
                .collect::<Result<Vec<_>, _>>()?,
        ),
        PlanType::TupleRef(elements) => Ty::new_imm_ref(
            tcx,
            tcx.lifetimes.re_erased,
            native_type(tcx, &PlanType::Tuple(elements.clone()))?,
        ),
        PlanType::TupleMutRef(elements) => Ty::new_mut_ref(
            tcx,
            tcx.lifetimes.re_erased,
            native_type(tcx, &PlanType::Tuple(elements.clone()))?,
        ),
        PlanType::RangeValue => Ty::new_tup(
            tcx,
            &range_field_types()
                .iter()
                .map(|field| native_type(tcx, field))
                .collect::<Result<Vec<_>, _>>()?,
        ),
        PlanType::ModelMutRef(_, name) => Ty::new_mut_ref(tcx, tcx.lifetimes.re_erased, model_type(tcx, name)?),
        PlanType::ModelRef(_, name) => Ty::new_imm_ref(tcx, tcx.lifetimes.re_erased, model_type(tcx, name)?),
        PlanType::List(leaf, depth) => list_type(tcx, leaf, *depth)?,
        PlanType::ListIntoIter(leaf, depth) => list_into_iter_type(tcx, leaf, *depth)?,
        PlanType::ListIntoIterMutRef(leaf, depth) => {
            Ty::new_mut_ref(tcx, tcx.lifetimes.re_erased, list_into_iter_type(tcx, leaf, *depth)?)
        }
        PlanType::ListRef(leaf, depth) => Ty::new_imm_ref(tcx, tcx.lifetimes.re_erased, list_type(tcx, leaf, *depth)?),
        PlanType::ListMutRef(leaf, depth) => {
            Ty::new_mut_ref(tcx, tcx.lifetimes.re_erased, list_type(tcx, leaf, *depth)?)
        }
        PlanType::Set(leaf) => hash_collection_type(tcx, "HashSet", &[leaf])?,
        PlanType::SetRef(leaf) => Ty::new_imm_ref(
            tcx,
            tcx.lifetimes.re_erased,
            hash_collection_type(tcx, "HashSet", &[leaf])?,
        ),
        PlanType::SetMutRef(leaf) => Ty::new_mut_ref(
            tcx,
            tcx.lifetimes.re_erased,
            hash_collection_type(tcx, "HashSet", &[leaf])?,
        ),
        PlanType::Dict(key, value) => hash_collection_type(tcx, "HashMap", &[key, value])?,
        PlanType::DictRef(key, value) => Ty::new_imm_ref(
            tcx,
            tcx.lifetimes.re_erased,
            hash_collection_type(tcx, "HashMap", &[key, value])?,
        ),
        PlanType::DictMutRef(key, value) => Ty::new_mut_ref(
            tcx,
            tcx.lifetimes.re_erased,
            hash_collection_type(tcx, "HashMap", &[key, value])?,
        ),
        PlanType::Int => tcx.types.i64,
        PlanType::Float => tcx.types.f64,
        PlanType::I8 => tcx.types.i8,
        PlanType::I16 => tcx.types.i16,
        PlanType::I32 => tcx.types.i32,
        PlanType::I128 => tcx.types.i128,
        PlanType::U8 => tcx.types.u8,
        PlanType::U16 => tcx.types.u16,
        PlanType::U32 => tcx.types.u32,
        PlanType::U64 => tcx.types.u64,
        PlanType::U128 => tcx.types.u128,
        PlanType::F32 => tcx.types.f32,
        PlanType::F64 => tcx.types.f64,
        PlanType::CheckedNumeric(ty) => Ty::new_tup(tcx, &[numeric_type(tcx, ty), tcx.types.bool]),
        PlanType::ISize => tcx.types.isize,
        PlanType::USize => tcx.types.usize,
        PlanType::Bool => tcx.types.bool,
        PlanType::FunctionPointer(signature) => function_pointer_type(tcx, signature)?,
        PlanType::Closure(signature) => Ty::new_box(tcx, callable_object_type(tcx, signature)?),
        PlanType::SharedCallable(signature) => model_type(tcx, &crate::shared_callables::name(signature))?,
        PlanType::ClosureOnce(signature) => Ty::new_box(tcx, consuming_callable_object_type(tcx, signature)?),
        PlanType::ClosureRef(signature) => {
            Ty::new_imm_ref(tcx, tcx.lifetimes.re_erased, callable_object_type(tcx, signature)?)
        }
        PlanType::FunctionItem(name, _) => {
            let definition = if name.contains("::") {
                crate::callees::external(tcx, name)?
            } else {
                crate::callees::planned(tcx, name)?
            };
            Ty::new_fn_def(tcx, definition, tcx.mk_args(&[]))
        }
        PlanType::Unit => tcx.types.unit,
        PlanType::Never => tcx.types.never,
        PlanType::UnitFunction => Ty::new_fn_ptr(
            tcx,
            rustc_middle::ty::Binder::dummy(tcx.mk_fn_sig_safe_rust_abi([], tcx.types.unit)),
        ),
        PlanType::CheckedInt => Ty::new_tup(tcx, &[tcx.types.i64, tcx.types.bool]),
        PlanType::String => string_type(tcx)?,
        PlanType::NativeStr => tcx.types.str_,
        PlanType::NativeSlice(elements) | PlanType::SliceRef(elements) | PlanType::SliceMutRef(elements) => {
            let [element] = elements.as_slice() else {
                return Err(PlanError::Unsupported {
                    function: "native slice".into(),
                    reason: "slice requires one element".into(),
                });
            };
            let slice = Ty::new_slice(tcx, native_type(tcx, element)?);
            match ty {
                PlanType::SliceRef(_) => Ty::new_imm_ref(tcx, tcx.lifetimes.re_erased, slice),
                PlanType::SliceMutRef(_) => Ty::new_mut_ref(tcx, tcx.lifetimes.re_erased, slice),
                _ => slice,
            }
        }
        PlanType::Decimal => decimal_type(tcx)?,
        PlanType::ValidationError => validation_error_type(tcx)?,
        PlanType::ValidationErrorRef => Ty::new_imm_ref(tcx, tcx.lifetimes.re_erased, validation_error_type(tcx)?),
        PlanType::FrozenStr => frozen_type(tcx)?,
        PlanType::FrozenBytes => frozen_bytes_type(tcx)?,
        PlanType::FrozenCollection(name, elements) => frozen_collection_type(tcx, name, elements)?,
        PlanType::FrozenCollectionRef(name, elements) => Ty::new_imm_ref(
            tcx,
            tcx.lifetimes.re_erased,
            frozen_collection_type(tcx, name, elements)?,
        ),
        PlanType::StaticText => Ty::new_imm_ref(tcx, tcx.lifetimes.re_static, tcx.types.str_),
        PlanType::StaticBytes => Ty::new_imm_ref(tcx, tcx.lifetimes.re_static, Ty::new_slice(tcx, tcx.types.u8)),
        PlanType::StringRef => Ty::new_imm_ref(tcx, tcx.lifetimes.re_erased, string_type(tcx)?),
        PlanType::StringMutRef => Ty::new_mut_ref(tcx, tcx.lifetimes.re_erased, string_type(tcx)?),
        PlanType::StrRef => Ty::new_imm_ref(tcx, tcx.lifetimes.re_erased, tcx.types.str_),
        PlanType::StringArray(count) => array_type(tcx, string_type(tcx)?, *count)?,
        PlanType::NativeArray(elements, count) => native_array_type(tcx, elements, *count)?,
        PlanType::NativeArrayRef(elements, count) => {
            Ty::new_imm_ref(tcx, tcx.lifetimes.re_erased, native_array_type(tcx, elements, *count)?)
        }
        PlanType::NativeArrayMutRef(elements, count) => {
            Ty::new_mut_ref(tcx, tcx.lifetimes.re_erased, native_array_type(tcx, elements, *count)?)
        }
        PlanType::StrArray(count) => array_type(tcx, native_type(tcx, &PlanType::StrRef)?, *count)?,
        PlanType::StringArrayRef(count) => Ty::new_imm_ref(
            tcx,
            tcx.lifetimes.re_erased,
            array_type(tcx, string_type(tcx)?, *count)?,
        ),
        PlanType::StrArrayRef(count) => Ty::new_imm_ref(
            tcx,
            tcx.lifetimes.re_erased,
            array_type(tcx, native_type(tcx, &PlanType::StrRef)?, *count)?,
        ),
        PlanType::StringSlice => Ty::new_imm_ref(tcx, tcx.lifetimes.re_erased, Ty::new_slice(tcx, string_type(tcx)?)),
        PlanType::StrSlice => Ty::new_imm_ref(
            tcx,
            tcx.lifetimes.re_erased,
            Ty::new_slice(tcx, native_type(tcx, &PlanType::StrRef)?),
        ),
    })
}

/// Translate a preflight-validated single scalar referent, rejecting malformed arity before constructing its native
/// reference.
fn scalar_reference_type<'tcx>(tcx: TyCtxt<'tcx>, elements: &[PlanType], mutable: bool) -> Result<Ty<'tcx>, PlanError> {
    let [inner] = elements else {
        return Err(PlanError::Unsupported {
            function: "scalar reference".into(),
            reason: "scalar reference requires exactly one referent".into(),
        });
    };
    let inner = native_type(tcx, inner)?;
    Ok(if mutable {
        Ty::new_mut_ref(tcx, tcx.lifetimes.re_erased, inner)
    } else {
        Ty::new_imm_ref(tcx, tcx.lifetimes.re_erased, inner)
    })
}

/// Resolve the explicitly bound dependency's immutable wrapper and preserve its exact static storage arguments.
/// A transitive copy with the same crate name cannot replace the owner used by the AST declaration.
pub fn frozen_collection_type<'tcx>(
    tcx: TyCtxt<'tcx>,
    name: &str,
    elements: &[PlanType],
) -> Result<Ty<'tcx>, PlanError> {
    let expected = match name {
        "FrozenList" | "FrozenSet" => 1,
        "FrozenDict" => 2,
        _ => {
            return Err(PlanError::UnknownCallee(format!(
                "unsupported frozen collection {name}"
            )));
        }
    };
    if elements.len() != expected {
        return Err(PlanError::UnknownCallee(format!(
            "unsupported frozen collection {name} arity"
        )));
    }
    let mut definition = crate::native_crates::root(tcx, "incan_std_core")
        .ok_or_else(|| PlanError::UnknownCallee("incan_std_core".into()))?;
    for segment in ["frozen", name] {
        definition = tcx
            .module_children(definition)
            .iter()
            .find(|child| child.ident.name.as_str() == segment && child.vis.is_public())
            .and_then(|child| child.res.opt_def_id())
            .ok_or_else(|| PlanError::UnknownCallee(format!("incan_std_core::frozen::{name}")))?;
    }
    let arguments = elements
        .iter()
        .map(|element| {
            let ty = native_type(tcx, element)?;
            Ok(ty.into())
        })
        .collect::<Result<Vec<_>, PlanError>>()?;
    Ok(Ty::new_adt(tcx, tcx.adt_def(definition), tcx.mk_args(&arguments)))
}

/// Resolve the bound core dependency's actual generator ADT and retain its element type. Transitive copies do not
/// replace the explicit crate alias used by the generator's declaration and runtime calls.
pub fn generator_type<'tcx>(tcx: TyCtxt<'tcx>, name: &str, leaf: &ListLeaf, depth: i64) -> Result<Ty<'tcx>, PlanError> {
    let mut definition = crate::native_crates::root(tcx, "incan_std_core")
        .ok_or_else(|| PlanError::UnknownCallee("incan_std_core".into()))?;
    for segment in ["iter", name] {
        definition = tcx
            .module_children(definition)
            .iter()
            .find(|child| child.ident.name.as_str() == segment && child.vis.is_public())
            .and_then(|child| child.res.opt_def_id())
            .ok_or_else(|| PlanError::UnknownCallee(format!("incan_std_core::iter::{name}")))?;
    }
    let element = if depth == 0 {
        collection_leaf_type(tcx, leaf)?
    } else {
        list_type(tcx, leaf, depth)?
    };
    Ok(Ty::new_adt(
        tcx,
        tcx.adt_def(definition),
        tcx.mk_args(&[element.into()]),
    ))
}

/// Build a safe Rust-ABI function pointer from its checked inputs and final return entry.
fn function_pointer_type<'tcx>(tcx: TyCtxt<'tcx>, signature: &[PlanType]) -> Result<Ty<'tcx>, PlanError> {
    let (result, parameters) = signature.split_last().ok_or_else(|| PlanError::Invalid {
        function: "function pointer".into(),
        reason: "callable signature has no return type".into(),
    })?;
    let (inputs, bound) = bound_callable_inputs(tcx, parameters)?;
    let signature = tcx.mk_fn_sig_safe_rust_abi(inputs, native_type(tcx, result)?);
    Ok(Ty::new_fn_ptr(
        tcx,
        rustc_middle::ty::Binder::bind_with_vars(signature, tcx.mk_bound_variable_kinds(&bound)),
    ))
}

/// Bind each borrowed input independently so a callable accepts a fresh caller loan on every invocation.
///
/// Body-local references use erased regions, but signature references must agree with rustc's higher-ranked AST
/// declarations. Nested callable signatures retain their own binders through `native_type`.
fn bound_callable_inputs<'tcx>(
    tcx: TyCtxt<'tcx>,
    parameters: &[PlanType],
) -> Result<(Vec<Ty<'tcx>>, Vec<rustc_middle::ty::BoundVariableKind<'tcx>>), PlanError> {
    use rustc_middle::ty::{BoundRegion, BoundRegionKind, BoundVar, BoundVariableKind, INNERMOST, Region, TyKind};
    let mut inputs = Vec::new();
    let mut bound = Vec::new();
    for parameter in parameters {
        let native = native_type(tcx, parameter)?;
        let input = if let TyKind::Ref(_, pointee, mutability) = native.kind() {
            let var = BoundVar::from_usize(bound.len());
            let kind = BoundRegionKind::Anon;
            bound.push(BoundVariableKind::Region(kind));
            let region = Region::new_bound(tcx, INNERMOST, BoundRegion { var, kind });
            match mutability {
                rustc_hir::Mutability::Mut => Ty::new_mut_ref(tcx, region, *pointee),
                rustc_hir::Mutability::Not => Ty::new_imm_ref(tcx, region, *pointee),
            }
        } else {
            native
        };
        inputs.push(input);
    }
    Ok((inputs, bound))
}

/// Recover the exact injected dyn alias, including rustc's Fn supertrait and bound-input lifetime lowering.
pub fn callable_object_type<'tcx>(tcx: TyCtxt<'tcx>, signature: &[PlanType]) -> Result<Ty<'tcx>, PlanError> {
    model_type(tcx, &crate::cloneable_closures::alias_name(signature))
}

/// Build the exact dyn FnOnce object whose owned box transfers its environment during invocation.
pub fn consuming_callable_object_type<'tcx>(tcx: TyCtxt<'tcx>, signature: &[PlanType]) -> Result<Ty<'tcx>, PlanError> {
    callable_object_type_for_kind(tcx, signature, true)
}

/// Preserve one signature and output projection while selecting the declared shared or consuming trait contract.
fn callable_object_type_for_kind<'tcx>(
    tcx: TyCtxt<'tcx>,
    signature: &[PlanType],
    consumes_environment: bool,
) -> Result<Ty<'tcx>, PlanError> {
    let (result, parameters) = signature.split_last().ok_or_else(|| PlanError::Invalid {
        function: "callable object".into(),
        reason: "callable signature has no return type".into(),
    })?;
    let missing = |item: &str| PlanError::UnknownCallee(item.into());
    let fn_trait = if consumes_environment {
        tcx.lang_items().fn_once_trait().ok_or_else(|| missing("FnOnce"))?
    } else {
        tcx.lang_items().fn_trait().ok_or_else(|| missing("Fn"))?
    };
    let output = tcx
        .lang_items()
        .fn_once_output()
        .ok_or_else(|| missing("FnOnce::Output"))?;
    let (inputs, bound) = bound_callable_inputs(tcx, parameters)?;
    let arguments = Ty::new_tup(tcx, &inputs);
    let bound = tcx.mk_bound_variable_kinds(&bound);
    let predicates = tcx.mk_poly_existential_predicates(&[
        rustc_middle::ty::Binder::bind_with_vars(
            rustc_middle::ty::ExistentialPredicate::Trait(rustc_middle::ty::ExistentialTraitRef::new(
                tcx,
                fn_trait,
                [arguments],
            )),
            bound,
        ),
        rustc_middle::ty::Binder::bind_with_vars(
            rustc_middle::ty::ExistentialPredicate::Projection(rustc_middle::ty::ExistentialProjection::new(
                tcx,
                output,
                [arguments],
                native_type(tcx, result)?.into(),
            )),
            bound,
        ),
    ]);
    Ok(Ty::new_dynamic(tcx, predicates, tcx.lifetimes.re_static))
}

/// The tuple a callable object's call takes, holding each checked input in order.
pub fn callable_arguments_type<'tcx>(tcx: TyCtxt<'tcx>, parameters: &[PlanType]) -> Result<Ty<'tcx>, PlanError> {
    let inputs = parameters
        .iter()
        .map(|ty| native_type(tcx, ty))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Ty::new_tup(tcx, &inputs))
}

/// Resolve the real standard String definition rather than manufacturing an ADT layout.
fn string_type(tcx: TyCtxt<'_>) -> Result<Ty<'_>, PlanError> {
    let definition = tcx.lang_items().string().ok_or_else(|| PlanError::Invalid {
        function: "String".into(),
        reason: "the native dependency closure has no String language item".into(),
    })?;
    Ok(tcx.type_of(definition).instantiate_identity().skip_normalization())
}

/// Recover Decimal128 from the canonical runtime constructor's return type, preserving its dependency identity.
fn decimal_type(tcx: TyCtxt<'_>) -> Result<Ty<'_>, PlanError> {
    let definition = crate::callees::external(tcx, "incan_native_runtime::decimal_from_parts")?;
    Ok(tcx.fn_sig(definition).instantiate_identity().skip_binder().output())
}

/// Recover the dependency-owned builtin error from its canonical Incan wrapper, preserving its actual Rust identity.
fn validation_error_type(tcx: TyCtxt<'_>) -> Result<Ty<'_>, PlanError> {
    let definition = crate::callees::external(tcx, "incan_native_runtime::make_validation_error")?;
    Ok(tcx.fn_sig(definition).instantiate_identity().skip_binder().output())
}

/// Recover the dependency-owned FrozenStr identity from its canonical observation wrapper.
fn frozen_type(tcx: TyCtxt<'_>) -> Result<Ty<'_>, PlanError> {
    let definition = crate::callees::external(tcx, "incan_native_runtime::format_frozen")?;
    tcx.fn_sig(definition)
        .instantiate_identity()
        .skip_binder()
        .inputs()
        .first()
        .copied()
        .ok_or_else(|| PlanError::Invalid {
            function: "FrozenStr".into(),
            reason: "the frozen observation wrapper has no carrier argument".into(),
        })
}

/// Recover the dependency-owned static byte wrapper from the canonical length observation function's input.
fn frozen_bytes_type(tcx: TyCtxt<'_>) -> Result<Ty<'_>, PlanError> {
    let definition = crate::callees::external(tcx, "incan_native_runtime::length_frozen_bytes")?;
    tcx.fn_sig(definition)
        .instantiate_identity()
        .skip_binder()
        .inputs()
        .first()
        .copied()
        .ok_or_else(|| PlanError::Invalid {
            function: "FrozenBytes".into(),
            reason: "the frozen byte observation wrapper has no carrier argument".into(),
        })
}

/// Translate a checked exact array length without a signed or truncating conversion.
fn array_type<'tcx>(tcx: TyCtxt<'tcx>, element: Ty<'tcx>, count: i64) -> Result<Ty<'tcx>, PlanError> {
    let count = u64::try_from(count).map_err(|_| PlanError::Invalid {
        function: "array".into(),
        reason: "array length must be nonnegative".into(),
    })?;
    Ok(Ty::new_array(tcx, element, count))
}

/// Preserve the checked native element and fixed count without interpreting the source array display again.
fn native_array_type<'tcx>(tcx: TyCtxt<'tcx>, elements: &[PlanType], count: i64) -> Result<Ty<'tcx>, PlanError> {
    let [element] = elements else {
        return Err(PlanError::Invalid {
            function: "native array".into(),
            reason: "array requires exactly one element type".into(),
        });
    };
    array_type(tcx, native_type(tcx, element)?, count)
}

/// Resolve an injected model or its concrete layout alias; aliases retain the same underlying source-named ADT.
pub fn model_type<'tcx>(tcx: TyCtxt<'tcx>, name: &str) -> Result<Ty<'tcx>, PlanError> {
    let definition = tcx
        .hir_crate_items(())
        .free_items()
        .map(|item| item.owner_id.to_def_id())
        .find(|def| {
            matches!(
                tcx.def_kind(*def),
                rustc_hir::def::DefKind::Struct | rustc_hir::def::DefKind::TyAlias
            ) && tcx.opt_item_name(*def).is_some_and(|symbol| symbol.as_str() == name)
        })
        .ok_or_else(|| PlanError::Invalid {
            function: name.into(),
            reason: "model declaration is missing".into(),
        })?;
    Ok(tcx.type_of(definition).instantiate_identity().skip_normalization())
}

/// Select the primitive leaf and wrap it in the standard Vec ADT once per retained list dimension.
fn list_type<'tcx>(tcx: TyCtxt<'tcx>, leaf: &ListLeaf, depth: i64) -> Result<Ty<'tcx>, PlanError> {
    let mut element = collection_leaf_type(tcx, leaf)?;
    if depth < 1 {
        return Err(PlanError::Invalid {
            function: "list".into(),
            reason: "list depth must be positive".into(),
        });
    }
    let definition = tcx
        .get_diagnostic_item(rustc_span::sym::Vec)
        .ok_or_else(|| PlanError::Invalid {
            function: "list".into(),
            reason: "native closure has no standard Vec definition".into(),
        })?;
    for _ in 0..depth {
        element = Ty::new_adt(
            tcx,
            tcx.adt_def(definition),
            rustc_middle::ty::GenericArgs::for_item(tcx, definition, |parameter, _| {
                if parameter.index == 0 {
                    element.into()
                } else {
                    tcx.type_of(parameter.def_id)
                        .instantiate_identity()
                        .skip_normalization()
                        .into()
                }
            }),
        );
    }
    Ok(element)
}

/// Resolve the uniquely selected standard Vec iterator, preserving the immediate list element and default allocator.
fn list_into_iter_type<'tcx>(tcx: TyCtxt<'tcx>, leaf: &ListLeaf, depth: i64) -> Result<Ty<'tcx>, PlanError> {
    if depth < 1 {
        return Err(PlanError::Invalid {
            function: "list iterator".into(),
            reason: "unsupported nonpositive list depth".into(),
        });
    }
    let mut definition =
        crate::native_crates::root(tcx, "alloc").ok_or_else(|| PlanError::UnknownCallee("alloc".into()))?;
    for segment in ["vec", "IntoIter"] {
        definition = tcx
            .module_children(definition)
            .iter()
            .find(|child| child.ident.name.as_str() == segment && child.vis.is_public())
            .and_then(|child| child.res.opt_def_id())
            .ok_or_else(|| PlanError::UnknownCallee("alloc::vec::IntoIter".into()))?;
    }
    let element = if depth == 1 {
        collection_leaf_type(tcx, leaf)?
    } else {
        list_type(tcx, leaf, depth - 1)?
    };
    Ok(Ty::new_adt(
        tcx,
        tcx.adt_def(definition),
        rustc_middle::ty::GenericArgs::for_item(tcx, definition, |parameter, _| {
            if parameter.index == 0 {
                element.into()
            } else {
                tcx.type_of(parameter.def_id)
                    .instantiate_identity()
                    .skip_normalization()
                    .into()
            }
        }),
    ))
}

/// Resolve the named sized carrier retained in an overflow pair.
fn numeric_type<'tcx>(tcx: TyCtxt<'tcx>, kind: &SizedNumeric) -> Ty<'tcx> {
    match kind {
        SizedNumeric::I8 => tcx.types.i8,
        SizedNumeric::I16 => tcx.types.i16,
        SizedNumeric::I32 => tcx.types.i32,
        SizedNumeric::I128 => tcx.types.i128,
        SizedNumeric::U8 => tcx.types.u8,
        SizedNumeric::U16 => tcx.types.u16,
        SizedNumeric::U32 => tcx.types.u32,
        SizedNumeric::U64 => tcx.types.u64,
        SizedNumeric::U128 => tcx.types.u128,
        SizedNumeric::F32 => tcx.types.f32,
        SizedNumeric::F64 => tcx.types.f64,
        SizedNumeric::ISize => tcx.types.isize,
        SizedNumeric::USize => tcx.types.usize,
    }
}

/// Resolve the standard hashed ADT and instantiate its default hasher from metadata.
fn hash_collection_type<'tcx>(tcx: TyCtxt<'tcx>, name: &str, leaves: &[&ListLeaf]) -> Result<Ty<'tcx>, PlanError> {
    let definition = tcx
        .get_diagnostic_item(rustc_span::Symbol::intern(name))
        .ok_or_else(|| PlanError::Invalid {
            function: name.into(),
            reason: "native closure has no standard hashed collection definition".into(),
        })?;
    let elements = leaves
        .iter()
        .map(|leaf| collection_leaf_type(tcx, leaf))
        .collect::<Result<Vec<_>, _>>()?;
    let arguments = rustc_middle::ty::GenericArgs::for_item(tcx, definition, |parameter, arguments| {
        match usize::try_from(parameter.index)
            .ok()
            .and_then(|index| elements.get(index))
        {
            Some(element) => (*element).into(),
            None => tcx
                .type_of(parameter.def_id)
                .instantiate(tcx, arguments)
                .skip_normalization()
                .into(),
        }
    });
    Ok(Ty::new_adt(tcx, tcx.adt_def(definition), arguments))
}

/// Map the checked scalar or tuple leaf once for all collection layouts.
fn collection_leaf_type<'tcx>(tcx: TyCtxt<'tcx>, leaf: &ListLeaf) -> Result<Ty<'tcx>, PlanError> {
    native_type(tcx, &list_leaf_type(leaf.clone()))
}

/// Resolve an injected source enum or concrete standard-carrier alias by its validated plan declaration name.
pub fn enum_type<'tcx>(tcx: TyCtxt<'tcx>, name: &str) -> Result<Ty<'tcx>, PlanError> {
    let definition = tcx
        .hir_crate_items(())
        .free_items()
        .map(|item| item.owner_id.to_def_id())
        .find(|def| {
            matches!(
                tcx.def_kind(*def),
                rustc_hir::def::DefKind::Enum | rustc_hir::def::DefKind::TyAlias
            ) && tcx.opt_item_name(*def).is_some_and(|symbol| symbol.as_str() == name)
        })
        .ok_or_else(|| PlanError::Invalid {
            function: name.into(),
            reason: "enum declaration is missing".into(),
        })?;
    Ok(tcx.type_of(definition).instantiate_identity().skip_normalization())
}

/// Resolve the explicitly bound Incan runtime's lazy zip wrapper with its checked flat element arguments.
fn zip_type<'tcx>(
    tcx: TyCtxt<'tcx>,
    left: &crate::plan::TupleElement,
    right: &crate::plan::TupleElement,
) -> Result<Ty<'tcx>, PlanError> {
    let root = crate::native_crates::root(tcx, "incan_native_runtime")
        .ok_or_else(|| PlanError::UnknownCallee("incan_native_runtime".into()))?;
    let definition = tcx
        .module_children(root)
        .iter()
        .find(|child| child.ident.name.as_str() == "ZipLists" && child.vis.is_public())
        .and_then(|child| child.res.opt_def_id())
        .ok_or_else(|| PlanError::UnknownCallee("incan_native_runtime::ZipLists".into()))?;
    let left = native_type(tcx, &tuple_element_type(left.clone()))?;
    let right = native_type(tcx, &tuple_element_type(right.clone()))?;
    Ok(Ty::new_adt(
        tcx,
        tcx.adt_def(definition),
        tcx.mk_args(&[left.into(), right.into()]),
    ))
}

/// Resolve the bound core dependency's TypeToken carrier with its exact represented native type. A same-named
/// transitive core artifact cannot supply the carrier for the explicit SDK declaration.
pub fn type_token_type<'tcx>(tcx: TyCtxt<'tcx>, elements: &[PlanType]) -> Result<Ty<'tcx>, PlanError> {
    let [element] = elements else {
        return Err(PlanError::UnknownCallee("unsupported TypeToken arity".into()));
    };
    let mut definition = crate::native_crates::root(tcx, "incan_std_core")
        .ok_or_else(|| PlanError::UnknownCallee("unsupported TypeToken SDK owner".into()))?;
    for segment in ["reflection", "TypeToken"] {
        definition = tcx
            .module_children(definition)
            .iter()
            .find(|child| child.ident.name.as_str() == segment && child.vis.is_public())
            .and_then(|child| child.res.opt_def_id())
            .ok_or_else(|| PlanError::UnknownCallee("unsupported TypeToken SDK declaration".into()))?;
    }
    Ok(Ty::new_adt(
        tcx,
        tcx.adt_def(definition),
        tcx.mk_args(&[native_type(tcx, element)?.into()]),
    ))
}
