//! Local and imported trait implementations, slot implementations and trait argument witnesses.

use super::*;

/// Find the trait this module declares under `name`.
pub(super) fn local_trait<'program>(program: &'program ast::Program, name: &str) -> Option<&'program ast::TraitDecl> {
    program.declarations.iter().find_map(|item| match &item.node {
        ast::Declaration::Trait(trait_decl) if trait_decl.name == name => Some(trait_decl),
        _ => None,
    })
}

/// Expand one adopted local trait into its local supertraits, instantiating each supertrait clause the checker
/// resolved with the adopting instantiation's arguments.
pub(super) fn local_supertraits<'program>(
    program: &'program ast::Program,
    type_info: &TypeCheckInfo,
    root: &'program ast::TraitDecl,
    arguments: &[ResolvedType],
) -> Vec<(&'program ast::TraitDecl, Vec<ResolvedType>)> {
    let mut reached = Vec::new();
    let mut seen = HashSet::new();
    let mut work = vec![(root, arguments.to_vec())];
    while let Some((declaration, arguments)) = work.pop() {
        let names = declaration
            .type_params
            .iter()
            .map(|parameter| parameter.name.clone())
            .collect::<Vec<_>>();
        let substitution = crate::resolved_type_subst::type_param_subst_map(&names, &arguments);
        for (name, clause) in type_info
            .traits
            .direct_supertraits
            .get(&declaration.name)
            .into_iter()
            .flatten()
        {
            let Some(supertrait) = local_trait(program, name) else {
                continue;
            };
            let instantiated = clause
                .iter()
                .map(|argument| crate::resolved_type_subst::substitute_resolved_type(argument, &substitution))
                .collect::<Vec<_>>();
            if seen.insert((name.clone(), format!("{instantiated:?}"))) {
                reached.push((supertrait, instantiated.clone()));
                work.push((supertrait, instantiated));
            }
        }
    }
    reached
}

/// Collect the local traits one adopter implements: each direct adoption at its checked arguments, then the local
/// supertraits those reach.
///
/// Bare clauses consume the separate checked Body IR witness; written clauses keep their existing explicit snapshot.
/// A generic trait adopted without recorded arguments contributes nothing. Distinct checked instantiations retain
/// separate dispatch records; their slot arguments distinguish the same declaration's overload selections.
pub(super) fn adopted_local_traits<'program>(
    program: &'program ast::Program,
    type_info: &TypeCheckInfo,
    adoptions: &[ast::Spanned<ast::TraitBound>],
) -> Vec<AdoptedTrait<'program>> {
    let mut adopted = Vec::new();
    for adoption in adoptions {
        let Some(declaration) = local_trait(program, &adoption.node.name) else {
            continue;
        };
        let arguments = if declaration.type_params.is_empty() {
            Vec::new()
        } else {
            match type_info
                .traits
                .body_ir_adoption_type_args
                .get(&adoption.span)
                .or_else(|| type_info.traits.adoption_type_args.get(&adoption.span))
            {
                Some(arguments) if arguments.len() == declaration.type_params.len() => arguments.clone(),
                _ => continue,
            }
        };
        let supertraits = local_supertraits(program, type_info, declaration, &arguments);
        adopted.push(AdoptedTrait {
            declaration,
            arguments,
            direct_ancestors: Some(
                supertraits
                    .iter()
                    .map(|(trait_decl, _)| trait_decl.name.clone())
                    .collect(),
            ),
        });
        adopted.extend(supertraits.into_iter().map(|(declaration, arguments)| AdoptedTrait {
            declaration,
            arguments,
            direct_ancestors: None,
        }));
    }
    adopted
}

/// Select the body that fills one trait slot for an adopter, with the type arguments its default needs.
///
/// The adopter's own method wins, then the slot trait's default, then the default a directly adopted subtrait of the
/// slot trait declares, taking those subtraits by name: the order legacy trait expansion fills a slot in. A targeted
/// method answers only the checker-recorded slot of its target, never another trait's same-named slot.
pub(super) fn slot_implementation(
    type_info: &TypeCheckInfo,
    adopted: &[AdoptedTrait<'_>],
    slot_trait: &AdoptedTrait<'_>,
    slot: &ast::Spanned<ast::MethodDecl>,
    methods: &[ast::Spanned<ast::MethodDecl>],
    owner: &CanonicalSymbolId,
) -> Option<(CanonicalSymbolId, Vec<bir::TraitTypeArgument>)> {
    let identity = type_info
        .traits
        .method_identities
        .get(&(slot_trait.declaration.name.clone(), slot.node.name.clone()))?;
    let selected_name = checked_slot_method_name(type_info, owner, &slot.node.name);
    let mut candidates = methods
        .iter()
        .filter(|candidate| {
            candidate.node.name == selected_name && method_fills_trait_slot(type_info, &candidate.node, identity)
        })
        .collect::<Vec<_>>();
    if candidates.len() > 1 {
        candidates.retain(|candidate| instantiated_slot_signature_matches(type_info, slot_trait, slot, candidate));
    }
    match candidates.as_slice() {
        [candidate] => type_info
            .declarations
            .method_bindings_by_span
            .get(&candidate.span)
            .and_then(|binding| binding.identity.clone())
            .map(|identity| (identity, Vec::new())),
        [] if slot.node.body.is_some() => type_info
            .traits
            .method_identities
            .get(&(slot_trait.declaration.name.clone(), slot.node.name.clone()))
            .map(|identity| (identity.clone(), slot_trait.type_arguments())),
        [] => {
            let mut subtraits = adopted
                .iter()
                .filter(|candidate| {
                    candidate
                        .direct_ancestors
                        .as_ref()
                        .is_some_and(|ancestors| ancestors.contains(&slot_trait.declaration.name))
                })
                .collect::<Vec<_>>();
            subtraits.sort_by(|left, right| left.declaration.name.cmp(&right.declaration.name));
            subtraits.into_iter().find_map(|subtrait| {
                subtrait
                    .declaration
                    .methods
                    .iter()
                    .any(|method| method.node.name == slot.node.name && method.node.body.is_some())
                    .then(|| {
                        type_info
                            .traits
                            .method_identities
                            .get(&(subtrait.declaration.name.clone(), slot.node.name.clone()))
                    })
                    .flatten()
                    .map(|identity| (identity.clone(), subtrait.type_arguments()))
            })
        }
        _ => None,
    }
}

/// Select among checked overloads using the slot's instantiated signature contract. The checker already proved
/// adoption compatibility; Body IR consumes its resolved signatures instead of resolving annotations again.
pub(super) fn instantiated_slot_signature_matches(
    facts: &TypeCheckInfo,
    adopted: &AdoptedTrait<'_>,
    slot: &ast::Spanned<ast::MethodDecl>,
    candidate: &ast::Spanned<ast::MethodDecl>,
) -> bool {
    let bindings = &facts.declarations.method_bindings_by_span;
    let Some(expected) = bindings.get(&slot.span) else {
        return false;
    };
    let Some(actual) = bindings.get(&candidate.span) else {
        return false;
    };
    let parameters = adopted
        .declaration
        .type_params
        .iter()
        .map(|parameter| parameter.name.clone())
        .collect::<Vec<_>>();
    let substitution = crate::resolved_type_subst::type_param_subst_map(&parameters, &adopted.arguments);
    expected.params.len() == actual.params.len()
        && instantiate_declared_slot_type(&expected.return_type, &substitution) == actual.return_type
        && expected.params.iter().zip(&actual.params).all(|(expected, actual)| {
            expected.kind == actual.kind && instantiate_declared_slot_type(&expected.ty, &substitution) == actual.ty
        })
}

/// Substitute only the declaring trait's binders, including legacy Named placeholders retained by checked abstract
/// signatures. A same-spelled nominal outside that declaration's parameter map never becomes a binder.
pub(super) fn instantiate_declared_slot_type(
    ty: &ResolvedType,
    bindings: &HashMap<String, ResolvedType>,
) -> ResolvedType {
    crate::resolved_type_subst::map_resolved_type_leaves(ty, &|leaf| match leaf {
        ResolvedType::Named(name) | ResolvedType::TypeVar(name) => bindings.get(name).cloned(),
        _ => None,
    })
}

/// Match an explicit method target through its checked slot identity; unqualified methods can fill any compatible
/// slot. Explicit target arguments still refuse because no checked target instantiation is retained for that clause.
pub(super) fn method_fills_trait_slot(
    type_info: &TypeCheckInfo,
    method: &ast::MethodDecl,
    slot: &CanonicalSymbolId,
) -> bool {
    let Some(target) = &method.trait_target else {
        return true;
    };
    target.node.type_args.is_empty()
        && type_info
            .traits
            .method_identities
            .get(&(target.node.name.clone(), method.name.clone()))
            == Some(slot)
}

/// Retain checked source derive slots, selecting a unique local override or the retained default body identity.
/// Compiled standard slots remain SDK-owned and never masquerade as source trait implementations.
pub(super) fn derived_trait_implementations(
    type_info: &TypeCheckInfo,
    methods: &[ast::Spanned<ast::MethodDecl>],
    owner: &CanonicalSymbolId,
) -> Vec<bir::TraitImplementation> {
    type_info
        .declarations
        .body_ir_derived_methods
        .get(&owner.declaration_name)
        .into_iter()
        .flatten()
        .filter_map(|method| {
            if type_info.traits.is_standard_slot(&method.method) || !is_executable_source_trait_slot(&method.method) {
                return None;
            }
            let mut candidates = methods
                .iter()
                .filter(|candidate| candidate.node.name == method.method.declaration_name);
            let implementation = match (candidates.next(), candidates.next()) {
                (Some(candidate), None) => type_info
                    .declarations
                    .method_bindings_by_span
                    .get(&candidate.span)
                    .and_then(|binding| binding.identity.clone()),
                (None, None) if method.has_default => Some(method.method.clone()),
                _ => None,
            }?;
            Some(bir::TraitImplementation {
                owner: owner.clone(),
                method: method.method.clone(),
                implementation,
                slot_arguments: method
                    .type_arguments
                    .iter()
                    .map(|(_, argument)| semantic_type_from_resolved(argument))
                    .collect(),
                type_arguments: method
                    .type_arguments
                    .iter()
                    .map(|(parameter, argument)| bir::TraitTypeArgument {
                        parameter: parameter.clone(),
                        argument: semantic_type_from_resolved(argument),
                    })
                    .collect(),
            })
        })
        .collect()
}

/// Snapshot checked local trait adoption arguments independently from the body that fills each method slot.
/// Own methods carry no default-body arguments, so their slot records cannot substitute for this checked adoption.
pub(in crate::body_ir) fn collect_trait_argument_witnesses(
    program: &ast::Program,
    facts: &TypeCheckInfo,
    nominals: &[bir::NominalDeclaration],
) -> std::collections::BTreeMap<(CanonicalSymbolId, CanonicalSymbolId), Vec<IncanType>> {
    let mut witnesses = std::collections::BTreeMap::new();
    for declaration in &program.declarations {
        let adoptions = match &declaration.node {
            ast::Declaration::Model(model) => &model.traits,
            ast::Declaration::Class(class) => &class.traits,
            ast::Declaration::Newtype(value) => &value.traits,
            _ => continue,
        };
        let Some(identity) = facts.declarations.declaration_identities.get(&declaration.span) else {
            continue;
        };
        let Some(owner) = nominals.iter().find(|owner| &owner.canonical == identity) else {
            continue;
        };
        for adopted in adopted_local_traits(program, facts, adoptions) {
            let Some(trait_owner) = program
                .declarations
                .iter()
                .find_map(|declaration| match &declaration.node {
                    ast::Declaration::Trait(value) if std::ptr::eq(value, adopted.declaration) => {
                        facts.declarations.declaration_identities.get(&declaration.span)
                    }
                    _ => None,
                })
            else {
                continue;
            };
            let arguments = adopted
                .arguments
                .iter()
                .map(|argument| parameter_types::checked_parameter_type(argument, facts, &owner.type_parameters))
                .collect();
            witnesses.insert((identity.clone(), trait_owner.clone()), arguments);
        }
    }
    witnesses
}

/// Retain trait slots and their checked concrete implementation identities for each admitted adopter.
///
/// Successful typechecking already proves adoption and method compatibility. This registry covers the adopter's local
/// traits at their checked instantiations and every local supertrait those reach; each slot keeps the body that fills
/// it and the type arguments that body's trait was adopted at. Imported traits without type parameters keep their own
/// slots under the checked provider identities (see [`imported_trait_implementations`]). A class's Rust trait slots
/// keep the methods checking bound to them; the native compiler, not this registry, proves that implementation.
/// Unrecorded instantiations and overloaded implementations never gain a guessed target. A refused checked newtype
/// constructor cannot contribute an implementation without its owner layout, nor can an enum whose layout was not
/// retained.
pub(in crate::body_ir) fn collect_local_trait_implementations(
    program: &ast::Program,
    type_info: &TypeCheckInfo,
    nominal_declarations: &[bir::NominalDeclaration],
    enum_declarations: &[bir::EnumDeclaration],
) -> Vec<bir::TraitImplementation> {
    let mut implementations = Vec::new();
    for declaration in &program.declarations {
        let (adoptions, methods, properties) = match &declaration.node {
            ast::Declaration::Model(model) if is_direct_replacement_checked_model(model, type_info) => (
                model.traits.as_slice(),
                model.methods.as_slice(),
                model.properties.as_slice(),
            ),
            ast::Declaration::Class(class) if is_direct_replacement_checked_class(class, type_info) => (
                class.traits.as_slice(),
                class.methods.as_slice(),
                class.properties.as_slice(),
            ),
            ast::Declaration::Newtype(newtype) if is_direct_replacement_checked_newtype(newtype, type_info) => {
                (newtype.traits.as_slice(), newtype.methods.as_slice(), &[][..])
            }
            ast::Declaration::Enum(value) if is_direct_native_enum(value) => {
                (value.traits.as_slice(), value.methods.as_slice(), &[][..])
            }
            _ => continue,
        };
        let Some(owner) = type_info.declarations.declaration_identities.get(&declaration.span) else {
            continue;
        };
        let retained = nominal_declarations
            .iter()
            .any(|declaration| &declaration.canonical == owner)
            || enum_declarations
                .iter()
                .any(|declaration| &declaration.canonical == owner);
        if !retained {
            continue;
        }
        implementations.extend(imported_trait_implementations(
            program, type_info, adoptions, methods, owner,
        ));
        implementations.extend(derived_trait_implementations(type_info, methods, owner));
        implementations.extend(
            type_info
                .declarations
                .native_trait_implementations
                .iter()
                .filter(|implementation| &implementation.owner == owner)
                .cloned(),
        );
        let adopted = adopted_local_traits(program, type_info, adoptions);
        implementations.extend(property_implementations(type_info, &adopted, properties, owner));
        for slot_trait in &adopted {
            for slot in &slot_trait.declaration.methods {
                let Some(method) = type_info
                    .traits
                    .method_identities
                    .get(&(slot_trait.declaration.name.clone(), slot.node.name.clone()))
                else {
                    continue;
                };
                if implementations.iter().any(|existing: &bir::TraitImplementation| {
                    &existing.owner == owner
                        && &existing.method == method
                        && existing.slot_arguments
                            == slot_trait
                                .arguments
                                .iter()
                                .map(semantic_type_from_resolved)
                                .collect::<Vec<_>>()
                }) {
                    continue;
                }
                let Some((implementation, type_arguments)) =
                    slot_implementation(type_info, &adopted, slot_trait, slot, methods, owner)
                else {
                    continue;
                };
                implementations.push(bir::TraitImplementation {
                    owner: owner.clone(),
                    method: method.clone(),
                    implementation,
                    slot_arguments: slot_trait.arguments.iter().map(semantic_type_from_resolved).collect(),
                    type_arguments,
                });
            }
        }
    }
    implementations
}

/// Retain a concrete getter filling a checked local trait property requirement, including local supertraits.
/// Property defaults are not admitted; ambiguous or absent source getters never acquire a guessed implementation.
pub(super) fn property_implementations(
    facts: &TypeCheckInfo,
    adopted: &[AdoptedTrait<'_>],
    properties: &[ast::Spanned<ast::PropertyDecl>],
    owner: &CanonicalSymbolId,
) -> Vec<bir::TraitImplementation> {
    let mut implementations = Vec::new();
    for trait_info in adopted {
        for slot in &trait_info.declaration.properties {
            if slot.node.body.is_some() {
                continue;
            }
            let Some(method) = facts
                .declarations
                .body_ir_property_bindings
                .get(&slot.span)
                .and_then(|binding| binding.callable.identity.as_ref())
            else {
                continue;
            };
            let mut candidates = properties
                .iter()
                .filter(|property| property.node.name == slot.node.name && property.node.body.is_some());
            let Some(property) = candidates.next() else {
                continue;
            };
            if candidates.next().is_some() {
                continue;
            }
            let Some(implementation) = facts
                .declarations
                .body_ir_property_bindings
                .get(&property.span)
                .and_then(|binding| binding.callable.identity.as_ref())
            else {
                continue;
            };
            if implementations
                .iter()
                .any(|existing: &bir::TraitImplementation| &existing.method == method)
            {
                continue;
            }
            implementations.push(bir::TraitImplementation {
                owner: owner.clone(),
                method: method.clone(),
                implementation: implementation.clone(),
                slot_arguments: trait_info.arguments.iter().map(semantic_type_from_resolved).collect(),
                type_arguments: Vec::new(),
            });
        }
    }
    implementations
}

/// Retain the slots of one adopter's imported traits, under checked provider identities.
///
/// Each slot keeps the body that fills it: the adopter's own method, or the imported trait's default. A project or
/// package trait's default lives in the module that declares the trait. A standard-library or compiled SDK protocol
/// brings its own slots and those of its standard supertraits ([`adopted_slots`]): the adopter's own method fills one
/// with or without type parameters. Defaults retain exact checked adoption arguments and resolve only through the
/// declaring package's published executable representation; a missing body still refuses at its call. Generic
/// supertrait instantiations without retained arguments remain excluded. Records are sorted by canonical slot identity.
pub(super) fn imported_trait_implementations(
    program: &ast::Program,
    type_info: &TypeCheckInfo,
    adoptions: &[ast::Spanned<ast::TraitBound>],
    methods: &[ast::Spanned<ast::MethodDecl>],
    owner: &CanonicalSymbolId,
) -> Vec<bir::TraitImplementation> {
    let mut implementations: Vec<bir::TraitImplementation> = Vec::new();
    for adoption in adoptions {
        let name = &adoption.node.name;
        if local_trait(program, name).is_some() {
            continue;
        }
        let source_comparison = adopted_slots(type_info, name)
            .into_iter()
            .any(|(_, slot_name, method)| {
                matches!(slot_name, "__eq__" | "__lt__")
                    && super::source_comparison::is_standard_slot(method, slot_name)
                    && imported_slot_implementation(type_info, method, slot_name, methods, false, owner).is_some()
            });
        for (trait_key, slot_name, method) in adopted_slots(type_info, name) {
            if implementations.iter().any(|existing| &existing.method == method) {
                continue;
            }
            let protocol = type_info.traits.is_standard_slot(method);
            if !protocol && !is_executable_source_trait_slot(method) {
                continue;
            }
            let Some(type_arguments) = imported_slot_arguments(type_info, adoption, trait_key) else {
                continue;
            };
            // Source comparison bridges retain the defaults they must forward; derive-only protocols leave those slots
            // on the native derive.
            let defaults = source_comparison || !(protocol && derive_backed_protocol(type_info, trait_key, method));
            if let Some(implementation) =
                imported_slot_implementation(type_info, method, slot_name, methods, defaults, owner)
            {
                implementations.push(bir::TraitImplementation {
                    owner: owner.clone(),
                    method: method.clone(),
                    implementation,
                    slot_arguments: type_arguments.iter().map(|binding| binding.argument.clone()).collect(),
                    type_arguments,
                });
            }
        }
    }
    implementations.sort_by(|left, right| left.method.cmp(&right.method));
    implementations
}

/// Close a direct imported trait only from checked adoption arguments and its recorded declaration binders.
/// Inherited standard slots retain an empty frame only along the checked argument-free edges selected by
/// [`standard_supertrait_keys`]; their provider-qualified keys need not have direct-adoption binder records.
/// Generic inherited slots lack a retained instantiation here and remain excluded.
pub(super) fn imported_slot_arguments(
    type_info: &TypeCheckInfo,
    adoption: &ast::Spanned<ast::TraitBound>,
    trait_key: &str,
) -> Option<Vec<bir::TraitTypeArgument>> {
    if trait_key != adoption.node.name {
        return standard_supertrait_keys(type_info, &adoption.node.name)
            .iter()
            .any(|key| key == trait_key)
            .then(Vec::new);
    }
    let parameters = type_info.traits.type_params.get(trait_key)?;
    if parameters.is_empty() {
        return adoption.node.type_args.is_empty().then(Vec::new);
    }
    let key = adoption.span;
    let arguments = type_info
        .traits
        .body_ir_adoption_type_args
        .get(&key)
        .or_else(|| type_info.traits.adoption_type_args.get(&key))?;
    if parameters.len() != arguments.len() {
        return None;
    }
    Some(
        parameters
            .iter()
            .zip(arguments)
            .map(|(parameter, argument)| bir::TraitTypeArgument {
                parameter: parameter.clone(),
                argument: semantic_type_from_resolved(argument),
            })
            .collect(),
    )
}

/// Whether the standard protocol a slot belongs to declares its behavior through a Rust derive (`@rust.derive`), as
/// `std.serde.json.Serialize` does.
///
/// Such a protocol's defaults are what the derive provides, not bodies an adopter takes, exactly as legacy derives
/// them. The trait is named by the module its slot's identity names and the last segment of its key, the
/// module-qualified key its derive paths are recorded under.
pub(super) fn derive_backed_protocol(type_info: &TypeCheckInfo, name: &str, slot: &CanonicalSymbolId) -> bool {
    let module_path = match &slot.origin {
        incan_semantics_core::SymbolOrigin::Package { module_path, .. } => module_path.as_slice(),
        incan_semantics_core::SymbolOrigin::Module(path) if path.first().map(String::as_str) == Some("std") => {
            &path[1..]
        }
        _ => return false,
    };
    let trait_name = name.rsplit('.').next().unwrap_or(name);
    let key = std::iter::once("std")
        .chain(module_path.iter().map(String::as_str))
        .chain(std::iter::once(trait_name))
        .collect::<Vec<_>>()
        .join(".");
    type_info
        .derivations
        .trait_rust_derive_paths
        .get(&key)
        .is_some_and(|paths| !paths.is_empty())
}

/// The slots one adopted trait brings, each with the trait key it is recorded under, its spelling and its identity.
///
/// A trait brings its own slots. A standard-library protocol also brings the slots of its standard supertraits without
/// type arguments, which the program need not import: those are recorded under the provider's module-qualified trait
/// key ([`standard_supertrait_keys`]), with the provider identities the SDK publishes their bodies under.
pub(super) fn adopted_slots<'facts>(
    type_info: &'facts TypeCheckInfo,
    name: &str,
) -> Vec<(&'facts str, &'facts str, &'facts CanonicalSymbolId)> {
    let mut keys = vec![name.to_owned()];
    if type_info.traits.is_standard_trait(name) {
        keys.extend(standard_supertrait_keys(type_info, name));
    }
    let mut slots = type_info
        .traits
        .method_identities
        .iter()
        .filter(|((trait_key, _), _)| keys.contains(trait_key))
        .map(|((trait_key, slot_name), method)| (trait_key.as_str(), slot_name.as_str(), method))
        .collect::<Vec<_>>();
    slots.sort_by(|left, right| left.2.cmp(right.2));
    slots
}

/// The retained keys of a standard trait's standard supertraits without type arguments, transitively.
///
/// Dependency-only slots use their provider-qualified key, while an explicitly imported supertrait can retain only its
/// visible binding. Both forms must carry the checked standard slot identities from the registry's module. A source
/// trait with the same spelling grants no inherited standard slots.
pub(super) fn standard_supertrait_keys(type_info: &TypeCheckInfo, name: &str) -> Vec<String> {
    let mut keys = Vec::new();
    let mut pending = vec![name.to_owned()];
    let mut seen = std::collections::HashSet::new();
    while let Some(current) = pending.pop() {
        if !seen.insert(current.clone()) {
            continue;
        }
        for (supertrait, arguments) in type_info.traits.direct_supertraits.get(&current).into_iter().flatten() {
            if !arguments.is_empty() {
                continue;
            }
            let Some(segments) = incan_lang::lang::stdlib::trait_method_module_segments(supertrait) else {
                continue;
            };
            let Some(key) = retained_standard_trait_key(type_info, supertrait, &segments) else {
                continue;
            };
            keys.push(key);
            pending.push(supertrait.clone());
        }
    }
    keys
}

/// Resolve an already checked standard edge to its actual slot key, requiring the registered declaring module and an
/// active standard binding.
pub(super) fn retained_standard_trait_key(
    type_info: &TypeCheckInfo,
    name: &str,
    segments: &[String],
) -> Option<String> {
    if segments.first().map(String::as_str) != Some("std") {
        return None;
    }
    for key in [format!("{}.{name}", segments.join(".")), name.to_owned()] {
        if !type_info.traits.is_standard_trait(&key) {
            continue;
        }
        let matches_module = type_info
            .traits
            .method_identities
            .iter()
            .filter(|((owner, _), _)| owner == &key)
            .all(|(_, identity)| match &identity.origin {
                incan_semantics_core::SymbolOrigin::Module(path) => path.as_slice() == segments,
                incan_semantics_core::SymbolOrigin::Package { module_path, .. } => {
                    module_path.as_slice() == &segments[1..]
                }
                _ => false,
            });
        if matches_module {
            return Some(key);
        }
    }
    None
}

/// Select the body that fills one imported slot: the adopter's only method of that name, else the slot's default when
/// `defaults` admits it.
pub(super) fn imported_slot_implementation(
    type_info: &TypeCheckInfo,
    method: &CanonicalSymbolId,
    slot_name: &str,
    methods: &[ast::Spanned<ast::MethodDecl>],
    defaults: bool,
    owner: &CanonicalSymbolId,
) -> Option<CanonicalSymbolId> {
    let selected_name = checked_slot_method_name(type_info, owner, slot_name);
    let candidates = methods
        .iter()
        .filter(|candidate| {
            candidate.node.name == selected_name && method_fills_trait_slot(type_info, &candidate.node, method)
        })
        .collect::<Vec<_>>();
    match candidates.as_slice() {
        [candidate] => type_info
            .declarations
            .method_bindings_by_span
            .get(&candidate.span)
            .and_then(|binding| binding.identity.clone()),
        [] if defaults && type_info.traits.default_method_identities.contains(method) => Some(method.clone()),
        _ => None,
    }
}

/// Follow the checker's same-owner alias projection before selecting the physical method body for a trait slot.
///
/// Only the recorded alias map participates; source spellings do not introduce a new relationship. The finite walk
/// also keeps a corrupted cyclic map from hanging the bridge, although ordinary checked input excludes cycles.
pub(in crate::body_ir) fn checked_slot_method_name<'facts>(
    type_info: &'facts TypeCheckInfo,
    owner: &CanonicalSymbolId,
    slot_name: &'facts str,
) -> &'facts str {
    let Some(aliases) = type_info
        .declarations
        .type_method_rebindings
        .get(&owner.declaration_name)
    else {
        return slot_name;
    };
    let mut name = slot_name;
    for _ in 0..=aliases.len() {
        let Some(target) = aliases.get(name) else {
            return name;
        };
        name = target;
    }
    slot_name
}

/// Whether a trait slot can be filled by a body of the project or of a package dependency.
///
/// The standard library's and Rust's protocols describe derive and runtime contracts, not bodies the project's modules
/// supply. Ordinary project and package identities remain eligible; an import alias never changes this boundary.
pub(super) fn is_executable_source_trait_slot(identity: &CanonicalSymbolId) -> bool {
    match &identity.origin {
        incan_semantics_core::SymbolOrigin::Module(path) => !incan_lang::lang::stdlib::is_any_stdlib_path(path),
        incan_semantics_core::SymbolOrigin::Package { .. } => true,
        incan_semantics_core::SymbolOrigin::Builtin | incan_semantics_core::SymbolOrigin::RustCrate(_) => false,
    }
}
