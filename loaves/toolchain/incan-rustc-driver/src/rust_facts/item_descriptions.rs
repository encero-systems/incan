//! Rust item, type, alias, signature, trait bound, trait implementation, trait and module descriptions.

use super::*;

/// Fold signature constants under their declaring item's environment without normalizing aliases or opaque types.
/// Concrete array lengths must cross the metadata boundary as values; unresolved generic constants stay explicit.
pub(super) struct SignatureConstants<'tcx> {
    tcx: TyCtxt<'tcx>,
    environment: ty::TypingEnv<'tcx>,
}

impl<'tcx> TypeFolder<TyCtxt<'tcx>> for SignatureConstants<'tcx> {
    /// Supply the same native session that owns the signature and its constant expressions.
    fn cx(&self) -> TyCtxt<'tcx> {
        self.tcx
    }

    /// Evaluate a constant when its declaration environment proves it, preserving unsupported generic expressions.
    fn fold_const(&mut self, value: ty::Const<'tcx>) -> ty::Const<'tcx> {
        self.tcx
            .try_normalize_erasing_regions(self.environment, ty::Unnormalized::new_wip(value))
            .unwrap_or(value)
    }
}

/// Render a native input or output with evaluated constants while retaining the written type and borrow structure.
pub(super) fn signature_display<'tcx>(tcx: TyCtxt<'tcx>, item: DefId, value: Ty<'tcx>) -> String {
    let value = value.fold_with(&mut SignatureConstants {
        tcx,
        environment: ty::TypingEnv::non_body_analysis(tcx, item),
    });
    display(value)
}

/// One item's metadata; the nominal types its signatures name join `named`.
pub(super) fn describe(tcx: TyCtxt<'_>, def: DefId, path: &str, named: &mut HashSet<DefId>) -> RustItemMetadata {
    let kind = match tcx.def_kind(def) {
        DefKind::Struct | DefKind::Enum | DefKind::Union => RustItemKind::Type(type_info(tcx, def, named)),
        DefKind::TyAlias => RustItemKind::Type(alias_info(tcx, def, named)),
        DefKind::Fn | DefKind::Ctor(_, rustc_hir::def::CtorKind::Fn) => {
            RustItemKind::Function(signature(tcx, def, named))
        }
        DefKind::Const { .. }
        | DefKind::AssocConst { .. }
        | DefKind::Static { .. }
        | DefKind::Ctor(_, rustc_hir::def::CtorKind::Const) => RustItemKind::Constant {
            type_display: display(tcx.type_of(def).instantiate_identity().skip_normalization()),
        },
        DefKind::Trait => {
            let mut info = trait_info(tcx, def, named);
            if resolve(tcx, path, true).is_some() {
                info.derive_macro = Some(RustMacroInfo::with_forms(vec![RustMacroForm::Derive]));
            }
            RustItemKind::Trait(info)
        }
        DefKind::Macro(kinds) if !macro_forms(kinds).is_empty() => {
            RustItemKind::Macro(RustMacroInfo::with_forms(macro_forms(kinds)))
        }
        DefKind::Mod => RustItemKind::Module(module_info(tcx, def)),
        other => RustItemKind::Unsupported {
            description: format!("{other:?}"),
        },
    };
    let definition = visible_path(tcx, def);
    RustItemMetadata {
        canonical_path: path.to_owned(),
        definition_path: (definition != path).then_some(definition),
        visibility: if matches!(&kind, RustItemKind::Type(_) | RustItemKind::Trait(_))
            && resolve(tcx, path, false) != Some(def)
        {
            RustVisibility::Restricted
        } else {
            RustVisibility::Public
        },
        kind,
    }
}

/// A struct, enum or union: type parameters and defaults, inherent methods with native impl binders and self types,
/// native fields and variants. Private slots retain storage and order but grant no source access. Construction
/// separately requires an exhaustive struct with all fields public. Method binders remain distinct from the owner's
/// parameters.
pub(super) fn type_info(tcx: TyCtxt<'_>, def: DefId, named: &mut HashSet<DefId>) -> RustTypeInfo {
    let (type_params, type_param_defaults) = type_parameters(tcx, def);
    let methods = inherent_methods(tcx, &crate::callees::owner_inherent_impls(tcx, def), named);
    let adt = tcx.adt_def(def);
    let fields = if adt.is_struct() {
        adt.all_fields()
            .map(|field| {
                let ty = tcx.type_of(field.did).instantiate_identity().skip_normalization();
                if field.vis.is_public() {
                    note_named(ty, named);
                }
                RustFieldInfo {
                    name: field.name.to_string(),
                    type_display: display(ty),
                    type_shape: type_shape(tcx, ty),
                }
            })
            .collect()
    } else {
        Vec::new()
    };
    let variants = if adt.is_enum() {
        adt.variants()
            .iter()
            .map(|variant| {
                let mut fields = Vec::new();
                let mut field_type_displays = Vec::new();
                let mut field_carriers = Vec::new();
                for field in &variant.fields {
                    let ty = tcx.type_of(field.did).instantiate_identity().skip_normalization();
                    note_named(ty, named);
                    field_type_displays.push(display(ty));
                    let (shape, carrier) = match ty.kind() {
                        ty::Adt(owner, args) if owner.is_box() => {
                            (type_shape(tcx, args.type_at(0)), RustPayloadCarrier::Boxed)
                        }
                        _ => (type_shape(tcx, ty), RustPayloadCarrier::Direct),
                    };
                    fields.push(shape);
                    field_carriers.push(carrier);
                }
                RustVariantInfo {
                    name: variant.name.to_string(),
                    fields,
                    field_type_displays,
                    field_names: if variant.ctor_kind().is_none() {
                        variant.fields.iter().map(|field| field.name.to_string()).collect()
                    } else {
                        Vec::new()
                    },
                    field_carriers,
                }
            })
            .collect()
    } else {
        Vec::new()
    };
    RustTypeInfo {
        metadata_completeness: layout_completeness(adt),
        has_const_params: tcx
            .generics_of(def)
            .own_params
            .iter()
            .any(|parameter| matches!(parameter.kind, ty::GenericParamDefKind::Const { .. })),
        has_lifetime_params: has_lifetime_parameters(tcx, def),
        type_params,
        type_param_defaults,
        methods,
        implemented_traits: trait_impls(tcx, tcx.type_of(def).instantiate_identity().skip_normalization(), named),
        fields,
        variants,
        ..RustTypeInfo::default()
    }
}

/// Keep private slots as native layout facts while refusing source access and incomplete construction.
pub(super) fn layout_completeness(adt: ty::AdtDef<'_>) -> RustTypeMetadataCompleteness {
    if !adt.is_struct() {
        return RustTypeMetadataCompleteness::CompleteWithoutConstruction;
    }
    let private = adt
        .all_fields()
        .filter(|field| !field.vis.is_public())
        .map(|field| field.name.to_string())
        .collect::<Vec<_>>();
    if !private.is_empty() {
        return RustTypeMetadataCompleteness::CompleteWithPrivateFields(private);
    }
    if adt.non_enum_variant().is_field_list_non_exhaustive() {
        return RustTypeMetadataCompleteness::CompleteWithoutConstruction;
    }
    RustTypeMetadataCompleteness::Complete
}

/// Retain alias binders and candidate implementations on its actual target. An alias introduces no new native
/// owner; the checker expands it for selection while executable storage may keep the public alias identity.
pub(super) fn alias_info(tcx: TyCtxt<'_>, def: DefId, named: &mut HashSet<DefId>) -> RustTypeInfo {
    let target = tcx.type_of(def).instantiate_identity().skip_normalization();
    note_named(target, named);
    let (type_params, type_param_defaults) = type_parameters(tcx, def);
    RustTypeInfo {
        metadata_completeness: match target.kind() {
            ty::Adt(adt, _) => layout_completeness(*adt),
            _ => RustTypeMetadataCompleteness::CompleteWithoutConstruction,
        },
        has_const_params: tcx
            .generics_of(def)
            .own_params
            .iter()
            .any(|parameter| matches!(parameter.kind, ty::GenericParamDefKind::Const { .. })),
        has_lifetime_params: has_lifetime_parameters(tcx, def),
        type_params,
        type_param_defaults,
        alias_target: Some(display(target)),
        methods: inherent_methods(tcx, &crate::callees::type_inherent_impls(tcx, target), named),
        implemented_traits: trait_impls(tcx, target, named),
        ..RustTypeInfo::default()
    }
}

/// Retain lifetime presence before the extractor elides their labels from the source type-argument vocabulary.
pub(super) fn has_lifetime_parameters(tcx: TyCtxt<'_>, def: DefId) -> bool {
    tcx.generics_of(def)
        .own_params
        .iter()
        .any(|parameter| matches!(parameter.kind, ty::GenericParamDefKind::Lifetime))
}

/// Extract type binders and their declaration-order defaults identically for nominal owners and aliases.
pub(super) fn type_parameters(tcx: TyCtxt<'_>, def: DefId) -> (Vec<String>, Vec<Option<String>>) {
    let generics = tcx.generics_of(def);
    let parameters = generics
        .own_params
        .iter()
        .filter(|parameter| matches!(parameter.kind, ty::GenericParamDefKind::Type { .. }))
        .collect::<Vec<_>>();
    let type_param_defaults = parameters
        .iter()
        .map(|parameter| match parameter.kind {
            ty::GenericParamDefKind::Type { has_default: true, .. } => Some(display(
                tcx.type_of(parameter.def_id)
                    .instantiate_identity()
                    .skip_normalization(),
            )),
            _ => None,
        })
        .collect();
    (
        parameters.iter().map(|parameter| parameter.name.to_string()).collect(),
        type_param_defaults,
    )
}

/// Read public methods from metadata-selected impls, preserving impl binders and exact self types for candidate
/// checking.
pub(super) fn inherent_methods(
    tcx: TyCtxt<'_>,
    implementations: &[DefId],
    named: &mut HashSet<DefId>,
) -> Vec<RustMethodSig> {
    implementations
        .iter()
        .flat_map(|implementation| tcx.associated_item_def_ids(*implementation).iter().copied())
        .filter(|item| tcx.def_kind(*item) == DefKind::AssocFn && tcx.visibility(*item).is_public())
        .map(|item| RustMethodSig {
            name: tcx.item_name(item).to_string(),
            signature: signature(tcx, item, named),
            impl_type_params: tcx
                .generics_of(item)
                .parent
                .map(|parent| {
                    tcx.generics_of(parent)
                        .own_params
                        .iter()
                        .filter(|parameter| matches!(parameter.kind, ty::GenericParamDefKind::Type { .. }))
                        .map(|parameter| parameter.name.to_string())
                        .collect()
                })
                .unwrap_or_default(),
            impl_self_type: tcx
                .generics_of(item)
                .parent
                .map(|parent| display(tcx.type_of(parent).instantiate_identity().skip_normalization())),
        })
        .collect()
}

/// Retain rustc-proven auto traits for closed owners, including primitives whose auto impls have no impl item.
/// Open owners keep their explicit conditional templates; a missing proof never becomes a negative trait fact.
pub(super) fn closed_auto_trait_impls<'tcx>(
    tcx: TyCtxt<'tcx>,
    owner: Ty<'tcx>,
    named: &mut HashSet<DefId>,
) -> Vec<RustImplementedTrait> {
    use rustc_infer::infer::TyCtxtInferExt;
    use rustc_middle::ty::TypeVisitableExt;
    use rustc_trait_selection::traits::query::evaluate_obligation::InferCtxtExt;

    if owner.has_non_region_param() || owner.has_infer() {
        return Vec::new();
    }
    let (infcx, environment) = tcx
        .infer_ctxt()
        .build_with_typing_env(ty::TypingEnv::fully_monomorphized());
    [sym::Send, sym::Sync]
        .into_iter()
        .filter_map(|name| {
            let trait_id = tcx.get_diagnostic_item(name)?;
            let reference = ty::TraitRef::new(tcx, trait_id, [owner]);
            let obligation = rustc_infer::traits::Obligation::new(
                tcx,
                rustc_middle::traits::ObligationCause::dummy(),
                environment,
                reference,
            );
            if !infcx.predicate_must_hold_modulo_regions(&obligation) {
                return None;
            }
            named.insert(trait_id);
            Some(RustImplementedTrait {
                path: visible_path(tcx, trait_id),
                self_type: display(owner),
                mutable_reference: false,
                type_params: Vec::new(),
                type_arguments: Vec::new(),
                bounds: Vec::new(),
                associated_type_bindings: Vec::new(),
            })
        })
        .collect()
}

/// Project a metadata-selected field type into the checker's structural carrier without parsing its display.
/// Nominal paths and type parameters stay distinct; references retain the payload's semantic type for matching.
/// Sized primitives keep their native names; only Rust's i64 and f64 use the Incan scalar shapes.
pub(super) fn type_shape<'tcx>(tcx: TyCtxt<'tcx>, field: Ty<'tcx>) -> RustTypeShape {
    match field.kind() {
        ty::Never => RustTypeShape::Never,
        ty::Bool => RustTypeShape::Bool,
        ty::Int(ty::IntTy::I64) => RustTypeShape::Int,
        ty::Float(ty::FloatTy::F64) => RustTypeShape::Float,
        ty::Int(_) | ty::Uint(_) | ty::Float(_) => RustTypeShape::RustPath {
            path: display(field),
            args: Vec::new(),
        },
        ty::Str => RustTypeShape::Str,
        ty::Param(parameter) => RustTypeShape::TypeParam(parameter.name.to_string()),
        ty::Ref(_, inner, _) => RustTypeShape::Ref(Box::new(type_shape(tcx, *inner))),
        ty::Tuple(elements) if elements.is_empty() => RustTypeShape::Unit,
        ty::Tuple(elements) => RustTypeShape::Tuple(elements.iter().map(|element| type_shape(tcx, element)).collect()),
        ty::Slice(element) if *element == tcx.types.u8 => RustTypeShape::Bytes,
        ty::Adt(owner, _) if Some(owner.did()) == tcx.lang_items().string() => RustTypeShape::Str,
        ty::Adt(owner, arguments) if Some(owner.did()) == tcx.lang_items().option_type() => {
            RustTypeShape::Option(Box::new(type_shape(tcx, arguments.type_at(0))))
        }
        ty::Adt(owner, arguments) if tcx.is_diagnostic_item(sym::Result, owner.did()) => RustTypeShape::Result(
            Box::new(type_shape(tcx, arguments.type_at(0))),
            Box::new(type_shape(tcx, arguments.type_at(1))),
        ),
        ty::Adt(owner, arguments)
            if tcx.is_diagnostic_item(sym::Vec, owner.did()) && arguments.type_at(0) == tcx.types.u8 =>
        {
            RustTypeShape::Bytes
        }
        ty::Adt(owner, arguments) => RustTypeShape::RustPath {
            path: visible_path(tcx, owner.did()),
            args: arguments.types().map(|argument| type_shape(tcx, argument)).collect(),
        },
        _ => RustTypeShape::Unknown,
    }
}

/// Extract the exact signature of a function or tuple-variant constructor. A method retains its written receiver;
/// a constructor inherits the enum's type parameters so ordinary call inference can close its payload and result.
/// Async functions retain their logical Future::Output, with construction and polling checked at their ABI boundary.
pub(super) fn signature(tcx: TyCtxt<'_>, def: DefId, named: &mut HashSet<DefId>) -> RustFunctionSig {
    let signature = tcx
        .fn_sig(def)
        .instantiate_identity()
        .skip_normalization()
        .skip_binder();
    let receiver = tcx.def_kind(def) == DefKind::AssocFn && tcx.associated_item(def).is_method();
    let params = signature
        .inputs()
        .iter()
        .enumerate()
        .map(|(index, input)| {
            note_named(*input, named);
            let type_display = if receiver && index == 0 {
                receiver_display(*input)
            } else {
                signature_display(tcx, def, *input)
            };
            RustParam {
                name: (receiver && index == 0).then(|| "self".to_owned()),
                type_display,
            }
        })
        .collect();
    let is_async = !matches!(tcx.def_kind(def), DefKind::Ctor(..)) && tcx.asyncness(def).is_async();
    let output = if is_async {
        tcx.get_impl_future_output_ty(signature.output())
            .unwrap_or(signature.output())
    } else {
        signature.output()
    };
    note_named(output, named);
    let generics = tcx.generics_of(def);
    let first = if matches!(tcx.def_kind(def), DefKind::Ctor(..)) {
        0
    } else {
        generics.parent_count
    };
    let type_params = (first..generics.count())
        .map(|index| generics.param_at(index, tcx))
        .filter(|parameter| matches!(parameter.kind, ty::GenericParamDefKind::Type { .. }))
        .map(|parameter| parameter.name.to_string())
        .collect();
    RustFunctionSig {
        // No receiver contract: a missing fact authorizes no inferred borrowing.
        receiver_contract: None,
        bounds: trait_bounds(tcx, def, named),
        type_params,
        params,
        return_type: signature_display(tcx, def, output),
        is_async,
        is_unsafe: signature.safety().is_unsafe(),
    }
}

/// Preserve trait predicates and projection equalities, retaining their named traits and types for fallback
/// extraction's closure.
pub(super) fn trait_bounds(tcx: TyCtxt<'_>, def: DefId, named: &mut HashSet<DefId>) -> Vec<RustTraitBound> {
    let predicates = tcx.predicates_of(def).instantiate_identity(tcx).predicates;
    let mut bounds = Vec::new();
    for clause in &predicates {
        let Some(predicate) = clause.as_trait_clause() else {
            continue;
        };
        let reference = predicate.skip_binder().trait_ref;
        named.insert(reference.def_id);
        for argument in reference.args.types() {
            note_named(argument, named);
        }
        bounds.push(RustTraitBound {
            self_type: display(reference.self_ty()),
            path: visible_path(tcx, reference.def_id),
            type_arguments: reference.args.types().skip(1).map(display).collect(),
            associated_type_bindings: Vec::new(),
        });
    }
    for clause in &predicates {
        let Some(projection) = clause.skip_normalization().as_projection_clause() else {
            continue;
        };
        let projection = projection.skip_binder();
        let Some(value) = projection.term.as_type() else {
            continue;
        };
        named.insert(projection.trait_def_id(tcx));
        note_named(value, named);
        let Some(name) = tcx.opt_item_name(projection.def_id()) else {
            continue;
        };
        let path = visible_path(tcx, projection.trait_def_id(tcx));
        let self_type = display(projection.self_ty());
        let arguments = projection
            .projection_term
            .args
            .types()
            .skip(1)
            .map(display)
            .collect::<Vec<_>>();
        let binding = RustAssociatedTypeBinding {
            name: name.to_string(),
            value_path: display(value),
        };
        if let Some(bound) = bounds
            .iter_mut()
            .find(|bound| bound.path == path && bound.self_type == self_type && bound.type_arguments == arguments)
        {
            bound.associated_type_bindings.push(binding);
        } else {
            bounds.push(RustTraitBound {
                self_type,
                path,
                type_arguments: arguments,
                associated_type_bindings: vec![binding],
            });
        }
    }
    bounds
}

/// Retain candidate impl templates once per nominal owner. Predicates remain explicit: a blanket impl is never
/// unconditional trait evidence, and rustc verifies the selected closed trait reference before native emission.
/// Primitive filtering considers the same owned and borrowed receivers used to collect candidates.
pub(super) fn trait_impls<'tcx>(
    tcx: TyCtxt<'tcx>,
    owner: Ty<'tcx>,
    named: &mut HashSet<DefId>,
) -> Vec<RustImplementedTrait> {
    let mut implementations = HashSet::new();
    let receivers = [
        owner,
        Ty::new_imm_ref(tcx, tcx.lifetimes.re_erased, owner),
        Ty::new_mut_ref(tcx, tcx.lifetimes.re_erased, owner),
    ];
    for trait_id in tcx
        .all_traits_including_private()
        .filter(|id| tcx.visibility(*id).is_public())
    {
        for receiver in receivers {
            tcx.for_each_relevant_impl(trait_id, receiver, |implementation| {
                implementations.insert(implementation);
            });
        }
    }
    let mut implementations = implementations.into_iter().collect::<Vec<_>>();
    sort_trait_implementations(tcx, &mut implementations);
    let mut result: Vec<_> = implementations
        .into_iter()
        .filter_map(|implementation| {
            if tcx.impl_polarity(implementation) != ty::ImplPolarity::Positive {
                return None;
            }
            if matches!(
                owner.kind(),
                ty::Bool | ty::Char | ty::Int(_) | ty::Uint(_) | ty::Float(_) | ty::Str | ty::Slice(_)
            ) && !receivers
                .into_iter()
                .any(|receiver| concrete_impl_may_apply(tcx, implementation, receiver))
            {
                return None;
            }
            trait_impl_template(tcx, implementation, named)
        })
        .collect();
    result.extend(closed_auto_trait_impls(tcx, owner, named));
    result
}

/// Extract one positive implementation once for both owner and trait ABIs. Const binders remain unsupported;
/// associated definitions and complete retained trait predicates keep their native declaration identities. Nominal
/// associated outputs join the same metadata closure as method returns, so their public methods remain inspectable.
pub(super) fn trait_impl_template(
    tcx: TyCtxt<'_>,
    implementation: DefId,
    named: &mut HashSet<DefId>,
) -> Option<RustImplementedTrait> {
    if tcx.impl_polarity(implementation) != ty::ImplPolarity::Positive
        || tcx
            .generics_of(implementation)
            .own_params
            .iter()
            .any(|parameter| matches!(parameter.kind, ty::GenericParamDefKind::Const { .. }))
    {
        return None;
    }
    let reference = tcx
        .impl_trait_ref(implementation)
        .instantiate_identity()
        .skip_normalization();
    let associated_type_bindings = tcx
        .associated_item_def_ids(implementation)
        .iter()
        .copied()
        .filter(|item| tcx.def_kind(*item) == DefKind::AssocTy)
        .filter_map(|item| {
            let value = tcx.type_of(item).instantiate_identity().skip_normalization();
            note_named(value, named);
            Some(RustAssociatedTypeBinding {
                name: tcx.opt_item_name(item)?.to_string(),
                value_path: display(value),
            })
        })
        .collect();
    named.insert(reference.def_id);
    Some(RustImplementedTrait {
        path: visible_path(tcx, reference.def_id),
        mutable_reference: matches!(reference.self_ty().kind(), ty::Ref(_, _, ty::Mutability::Mut)),
        self_type: display(reference.self_ty()),
        type_params: tcx
            .generics_of(implementation)
            .own_params
            .iter()
            .filter(|parameter| matches!(parameter.kind, ty::GenericParamDefKind::Type { .. }))
            .map(|parameter| parameter.name.to_string())
            .collect(),
        type_arguments: reference.args.types().skip(1).map(display).collect(),
        bounds: trait_bounds(tcx, implementation, named),
        associated_type_bindings,
    })
}

/// Include positive trait templates from both blanket and concrete self-type buckets, independently of any cached
/// implementor. Native predicates still decide each closed instantiation's applicability.
pub(super) fn trait_implementations(
    tcx: TyCtxt<'_>,
    owner: DefId,
    named: &mut HashSet<DefId>,
) -> Vec<RustImplementedTrait> {
    let templates = tcx.trait_impls_of(owner);
    let mut implementations = templates
        .blanket_impls()
        .iter()
        .chain(templates.non_blanket_impls().values().flatten())
        .copied()
        .collect::<Vec<_>>();
    sort_trait_implementations(tcx, &mut implementations);
    implementations
        .into_iter()
        .filter_map(|implementation| trait_impl_template(tcx, implementation, named))
        .collect()
}

/// Order impl facts by their public path and stable native identity. Distinct versions of a crate can expose the same
/// path; its text alone leaves ties in randomized set or map iteration order and produces different ABI cache bytes.
pub(super) fn sort_trait_implementations(tcx: TyCtxt<'_>, implementations: &mut [DefId]) {
    implementations
        .sort_by_cached_key(|implementation| (tcx.def_path_str(*implementation), tcx.def_path_hash(*implementation)));
}

/// Eliminate impossible blanket candidates for scalar and unsized primitives. A slice element remains unconstrained
/// during extraction, so applicable conditional templates survive for the checker to instantiate and rustc to verify.
/// A reference retains its Sized carrier while its slice element remains an inference variable.
pub(super) fn concrete_impl_may_apply<'tcx>(tcx: TyCtxt<'tcx>, implementation: DefId, owner: Ty<'tcx>) -> bool {
    use rustc_infer::infer::TyCtxtInferExt;
    use rustc_trait_selection::traits::{ObligationCtxt, query::evaluate_obligation::InferCtxtExt};
    let (infcx, param_env) = tcx
        .infer_ctxt()
        .build_with_typing_env(ty::TypingEnv::fully_monomorphized());
    // A slice template's element is not constrained yet. Retain impls that can apply to some element while still
    // rejecting impossible Sized blanket implementations on the unsized slice itself.
    let owner = match owner.kind() {
        ty::Slice(_) => Ty::new_slice(tcx, infcx.next_ty_var(rustc_span::DUMMY_SP)),
        ty::Ref(region, inner, mutable) if matches!(inner.kind(), ty::Slice(_)) => Ty::new_ref(
            tcx,
            *region,
            Ty::new_slice(tcx, infcx.next_ty_var(rustc_span::DUMMY_SP)),
            *mutable,
        ),
        _ => owner,
    };
    let ocx = ObligationCtxt::new(&infcx);
    let arguments = infcx.fresh_args_for_item(rustc_span::DUMMY_SP, implementation);
    let reference = tcx
        .impl_trait_ref(implementation)
        .instantiate(tcx, arguments)
        .skip_normalization();
    let cause = rustc_middle::traits::ObligationCause::dummy();
    if ocx.eq(&cause, param_env, reference.self_ty(), owner).is_err() {
        return false;
    }
    tcx.predicates_of(implementation)
        .instantiate(tcx, arguments)
        .into_iter()
        .all(|(clause, _)| {
            let obligation =
                rustc_infer::traits::Obligation::new(tcx, cause.clone(), param_env, clause.skip_normalization());
            infcx.predicate_may_hold(&obligation)
        })
}

/// A method receiver as written: `&self`, `&mut self`, or `self` (and a typed receiver as its type).
pub(super) fn receiver_display(input: Ty<'_>) -> String {
    match input.kind() {
        ty::Ref(_, _, ty::Mutability::Not) => "&self".to_owned(),
        ty::Ref(_, _, ty::Mutability::Mut) => "&mut self".to_owned(),
        ty::Adt(..) | ty::Param(_) => "self".to_owned(),
        _ => display(input),
    }
}

/// Retain the nominal types and dynamic trait principals a signature names for the next extraction step.
pub(super) fn note_named(ty: Ty<'_>, named: &mut HashSet<DefId>) {
    for argument in ty.walk() {
        if let Some(inner) = argument.as_type() {
            match inner.kind() {
                ty::Adt(adt, _) => {
                    named.insert(adt.did());
                }
                ty::Dynamic(predicates, _) => {
                    if let Some(principal) = predicates.principal() {
                        named.insert(principal.skip_binder().def_id);
                    }
                }
                _ => {}
            }
        }
    }
}

/// Retain a trait's bounds and associated signatures as API dependencies. The impl index keeps its candidate
/// templates, but does not make every implementation's unrelated owner a reachable API type.
pub(super) fn trait_info(tcx: TyCtxt<'_>, def: DefId, named: &mut HashSet<DefId>) -> RustTraitInfo {
    RustTraitInfo {
        bounds: trait_bounds(tcx, def, named),
        implementations: trait_implementations(tcx, def, &mut HashSet::new()),
        is_in_prelude: resolve(tcx, "std::prelude::rust_2024", false).is_some_and(|prelude| {
            tcx.module_children(prelude)
                .iter()
                .any(|child| child.res.opt_def_id() == Some(def))
        }),
        type_params: tcx
            .generics_of(def)
            .own_params
            .iter()
            .filter(|parameter| parameter.index != 0 && matches!(parameter.kind, ty::GenericParamDefKind::Type { .. }))
            .map(|parameter| parameter.name.to_string())
            .collect(),
        items: tcx
            .associated_item_def_ids(def)
            .iter()
            .copied()
            .filter_map(|item| {
                let name = tcx.opt_item_name(item)?.to_string();
                match tcx.def_kind(item) {
                    DefKind::AssocFn => Some(RustTraitAssoc::Function {
                        name,
                        signature: signature(tcx, item, named),
                    }),
                    DefKind::AssocTy => Some(RustTraitAssoc::TypeAlias { name }),
                    DefKind::AssocConst { .. } => Some(RustTraitAssoc::Constant {
                        name,
                        type_display: display(tcx.type_of(item).instantiate_identity().skip_normalization()),
                    }),
                    _ => None,
                }
            })
            .collect(),
        derive_macro: None,
    }
}

/// A module's public children and what each one is.
pub(super) fn module_info(tcx: TyCtxt<'_>, def: DefId) -> RustModuleInfo {
    RustModuleInfo {
        children: tcx
            .module_children(def)
            .iter()
            .filter(|child| child.vis.is_public())
            .map(|child| RustModuleChild {
                name: child.ident.name.to_string(),
                kind_hint: match child.res.opt_def_id().map(|item| tcx.def_kind(item)) {
                    Some(DefKind::Mod) => RustModuleChildKind::Module,
                    Some(DefKind::Struct | DefKind::Enum | DefKind::Union | DefKind::TyAlias) => {
                        RustModuleChildKind::Type
                    }
                    Some(DefKind::Fn) => RustModuleChildKind::Function,
                    Some(DefKind::Const { .. } | DefKind::Static { .. }) => RustModuleChildKind::Constant,
                    Some(DefKind::Trait) => RustModuleChildKind::Trait,
                    _ => RustModuleChildKind::Other,
                },
            })
            .collect(),
    }
}
