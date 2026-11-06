//! Rust path and crate description, primitive types, public items, constructors and path resolution.

use super::*;

/// Describe named paths and bounded signature dependencies, closing unambiguous selected public field layouts
/// transitively. Field records can be reached through nested Options and collections without a source import of each
/// record. Their types need the same metadata as an imported owner; signature expansion stays bounded independently. A
/// path shared by distinct native crate versions grants no transitive facts without one selected crate binding.
pub(super) fn describe_paths(
    tcx: TyCtxt<'_>,
    paths: &BTreeSet<String>,
    depth: usize,
) -> BTreeMap<String, RustItemMetadata> {
    let mut facts = BTreeMap::new();
    let mut pending = Vec::new();
    for path in paths {
        if let Some(def) = resolve(tcx, path, false) {
            pending.push((path.clone(), def));
        } else if let Some(prefix) = restricted_prefix(tcx, path) {
            facts.insert(prefix.clone(), visibility_fence(&prefix));
        }
    }
    for _ in 0..=depth {
        let mut named = HashSet::new();
        while let Some((path, def)) = pending.pop() {
            if api_item_is_described(&facts, &path) {
                continue;
            }
            pending.extend(
                layout_dependencies(tcx, def)
                    .into_iter()
                    .filter(|dependency| has_selected_native_crate(tcx, *dependency))
                    .map(|dependency| (visible_path(tcx, dependency), dependency)),
            );
            pending.extend(public_associated_constants(tcx, def, &path));
            let metadata = describe(tcx, def, &path, &mut named);
            for (constructor_path, constructor) in enum_constructors(tcx, def, &path) {
                let metadata = describe(tcx, constructor, &constructor_path, &mut named);
                facts.entry(constructor_path).or_insert(metadata);
            }
            facts.insert(path, metadata);
        }
        pending = named
            .into_iter()
            .filter(|def| has_selected_native_crate(tcx, *def))
            .map(|def| (visible_path(tcx, def), def))
            .filter(|(path, _)| !api_item_is_described(&facts, path))
            .collect();
    }
    close_api_dependencies(tcx, &mut facts, pending.into_iter().map(|(_, def)| def).collect());
    facts
}

/// Retain dependency-only types and traits to a fixed point without making private paths source-visible. Public
/// foreign ADTs remain the responsibility of their own crate cache; only unnameable type dependencies expand here.
pub(super) fn close_api_dependencies(
    tcx: TyCtxt<'_>,
    facts: &mut BTreeMap<String, RustItemMetadata>,
    mut pending: HashSet<DefId>,
) {
    let mut visited = HashSet::new();
    while let Some(def) = pending.iter().next().copied() {
        pending.remove(&def);
        if !visited.insert(def) || !has_selected_native_crate(tcx, def) {
            continue;
        }
        let path = visible_path(tcx, def);
        let retained = match tcx.def_kind(def) {
            DefKind::Trait => true,
            DefKind::Struct | DefKind::Enum | DefKind::Union | DefKind::TyAlias => {
                resolve(tcx, &path, false) != Some(def)
            }
            _ => false,
        };
        if !retained || api_item_is_described(facts, &path) {
            continue;
        }
        let metadata = describe(tcx, def, &path, &mut pending);
        facts.insert(path, metadata);
    }
}

/// A visibility fence proves source inaccessibility without admitting a module's children or an item's API.
pub(super) fn visibility_fence(path: &str) -> RustItemMetadata {
    RustItemMetadata {
        canonical_path: path.to_owned(),
        definition_path: None,
        visibility: RustVisibility::Restricted,
        kind: RustItemKind::Unsupported {
            description: "restricted Rust namespace".into(),
        },
    }
}

/// Visibility-only facts do not replace a type's full metadata when an admitted API reaches that dependency.
pub(super) fn api_item_is_described(facts: &BTreeMap<String, RustItemMetadata>, path: &str) -> bool {
    facts.get(path).is_some_and(|metadata| {
        metadata.visibility != RustVisibility::Restricted || !matches!(&metadata.kind, RustItemKind::Unsupported { .. })
    })
}

/// Locate the first inaccessible module child in an authored path. Missing paths remain unknown rather than being
/// mislabeled private; lookup uses the exact native crate and rustc visibility, never a spelling convention.
pub(super) fn restricted_prefix(tcx: TyCtxt<'_>, path: &str) -> Option<String> {
    let mut segments = path.split("::").peekable();
    let root = segments.next()?;
    let mut current = crate::native_crates::root(tcx, root)?;
    let mut prefix = root.to_owned();
    while let Some(segment) = segments.next() {
        if tcx.def_kind(current) != DefKind::Mod {
            return None;
        }
        let child = tcx
            .module_children(current)
            .iter()
            .filter(|child| child.ident.name.as_str() == segment)
            .filter_map(|child| child.res.opt_def_id().map(|def| (child, def)))
            .min_by_key(|(_, def)| {
                if segments.peek().is_none() {
                    resolution_rank(tcx.def_kind(*def))
                } else {
                    match tcx.def_kind(*def) {
                        DefKind::Mod => 0,
                        DefKind::Struct | DefKind::Enum | DefKind::Union => 1,
                        _ => 2,
                    }
                }
            })?;
        prefix.push_str("::");
        prefix.push_str(segment);
        if !child.0.vis.is_public() {
            return Some(prefix);
        }
        current = child.1;
    }
    None
}

/// Admit transitive facts only from the native crate selected by an explicit canonical binding or a unique loaded
/// crate name. Distinct versions cannot share a path-indexed entry: choosing the first HashSet member would publish
/// another version's fields or traits. Use the definition's crate identity, preserving standard-library reexports.
pub(super) fn has_selected_native_crate(tcx: TyCtxt<'_>, definition: DefId) -> bool {
    let name = tcx.crate_name(definition.krate);
    crate::native_crates::root(tcx, name.as_str()).is_some_and(|root| root.krate == definition.krate)
}

/// Collect public field and enum payload owners, including generic arguments, without expanding method signatures.
/// Opaque private fields grant no access. The caller deduplicates item identities so recursive layouts terminate.
pub(super) fn layout_dependencies(tcx: TyCtxt<'_>, def: DefId) -> HashSet<DefId> {
    let mut named = HashSet::new();
    match tcx.def_kind(def) {
        DefKind::Struct | DefKind::Enum | DefKind::Union => {
            let owner = tcx.adt_def(def);
            for field in owner
                .all_fields()
                .filter(|field| owner.is_enum() || field.vis.is_public())
            {
                note_named(
                    tcx.type_of(field.did).instantiate_identity().skip_normalization(),
                    &mut named,
                );
            }
        }
        DefKind::TyAlias => {
            note_named(tcx.type_of(def).instantiate_identity().skip_normalization(), &mut named);
        }
        _ => {}
    }
    named
}

/// Every public item of the named crates, under each public module path that reaches it. Standard roots also retain
/// primitive impl facts, whose scalar carriers have no public item DefId for the module walker.
pub(super) fn describe_crates(tcx: TyCtxt<'_>, crates: &BTreeSet<String>) -> BTreeMap<String, RustItemMetadata> {
    let mut facts = BTreeMap::new();
    let mut named = HashSet::new();
    let mut described = HashMap::new();
    for name in crates {
        let Some(krate) = crate::native_crates::root(tcx, name) else {
            continue;
        };
        for (path, def) in public_items(tcx, krate, name) {
            if resolve(tcx, &path, false).is_none() {
                facts.insert(path.clone(), visibility_fence(&path));
                continue;
            }
            let template = described
                .entry(def)
                .or_insert_with(|| describe(tcx, def, &path, &mut named));
            let mut metadata = template.clone();
            metadata.canonical_path = path.clone();
            let definition = visible_path(tcx, def);
            metadata.definition_path = (definition != path).then_some(definition);
            if let RustItemKind::Trait(info) = &mut metadata.kind {
                info.derive_macro =
                    resolve(tcx, &path, true).map(|_| RustMacroInfo::with_forms(vec![RustMacroForm::Derive]));
            }
            if matches!(metadata.kind, RustItemKind::Macro(_))
                && facts
                    .get(&path)
                    .is_some_and(|existing: &RustItemMetadata| !matches!(existing.kind, RustItemKind::Macro(_)))
            {
                continue;
            }
            facts.insert(path, metadata);
        }
    }
    if crates
        .iter()
        .any(|name| matches!(name.as_str(), "std" | "core" | "alloc"))
    {
        describe_primitives(tcx, &mut facts, &mut named);
        describe_str(tcx, &mut facts, &mut named);
        let element = Ty::new_param(tcx, 0, rustc_span::Symbol::intern("T"));
        facts.insert(
            "[]".into(),
            RustItemMetadata {
                canonical_path: "[]".into(),
                definition_path: None,
                visibility: RustVisibility::Public,
                kind: RustItemKind::Type(RustTypeInfo {
                    type_params: vec!["T".into()],
                    type_param_defaults: vec![None],
                    methods: inherent_methods(
                        tcx,
                        tcx.incoherent_impls(ty::fast_reject::SimplifiedType::Slice),
                        &mut named,
                    ),
                    implemented_traits: trait_impls(tcx, Ty::new_slice(tcx, element), &mut named),
                    ..RustTypeInfo::default()
                }),
            },
        );
    }
    close_api_dependencies(tcx, &mut facts, named);
    facts
}

/// Publish the same unsized text owner under its primitive re-exports. Their methods and trait facts come from
/// rustc's primitive impl bucket, just as the bare `str` path does; the alias never becomes a nominal empty type.
pub(super) fn describe_str(
    tcx: TyCtxt<'_>,
    facts: &mut BTreeMap<String, RustItemMetadata>,
    named: &mut HashSet<DefId>,
) {
    let info = RustTypeInfo {
        methods: inherent_methods(tcx, tcx.incoherent_impls(ty::fast_reject::SimplifiedType::Str), named),
        implemented_traits: trait_impls(tcx, tcx.types.str_, named),
        ..RustTypeInfo::default()
    };
    publish_primitive(facts, "str", info);
}

/// Publish primitive inherent methods and trait facts under native and standard public paths. Primitive items have no
/// DefId for the public module walker; their methods, impl templates and associated types come from rustc's exact
/// scalar type. Public primitive re-exports are transparent aliases to that scalar, never new nominal layouts.
pub(super) fn describe_primitives(
    tcx: TyCtxt<'_>,
    facts: &mut BTreeMap<String, RustItemMetadata>,
    named: &mut HashSet<DefId>,
) {
    for (name, native) in primitive_types(tcx) {
        let info = RustTypeInfo {
            methods: inherent_methods(tcx, crate::callees::primitive_inherent_impls(tcx, native), named),
            implemented_traits: trait_impls(tcx, native, named),
            ..RustTypeInfo::default()
        };
        publish_primitive(facts, name, info);
    }
}

/// Publish one primitive's metadata under its native and public reexport identities. Each alias retains the same
/// inherent methods and impl facts; a public primitive path never creates a different nominal carrier.
pub(super) fn publish_primitive(facts: &mut BTreeMap<String, RustItemMetadata>, name: &str, info: RustTypeInfo) {
    for path in [
        name.to_owned(),
        format!("std::primitive::{name}"),
        format!("core::primitive::{name}"),
    ] {
        let definition_path = (path != name).then(|| name.to_owned());
        let mut selected = info.clone();
        selected.alias_target = definition_path.clone();
        facts.insert(
            path.clone(),
            RustItemMetadata {
                canonical_path: path,
                definition_path,
                visibility: RustVisibility::Public,
                kind: RustItemKind::Type(selected),
            },
        );
    }
}

/// Canonical primitive identities shared by metadata extraction and native inherent-method resolution.
pub(crate) fn primitive_types(tcx: TyCtxt<'_>) -> [(&'static str, Ty<'_>); 16] {
    let types = [
        tcx.types.bool,
        tcx.types.char,
        tcx.types.i8,
        tcx.types.i16,
        tcx.types.i32,
        tcx.types.i64,
        tcx.types.i128,
        tcx.types.isize,
        tcx.types.u8,
        tcx.types.u16,
        tcx.types.u32,
        tcx.types.u64,
        tcx.types.u128,
        tcx.types.usize,
        tcx.types.f32,
        tcx.types.f64,
    ];
    std::array::from_fn(|index| (PRIMITIVE_NAMES[index], types[index]))
}

/// Canonical scalar names in the same order as their rustc types above.
pub(super) const PRIMITIVE_NAMES: [&str; 16] = [
    "bool", "char", "i8", "i16", "i32", "i64", "i128", "isize", "u8", "u16", "u32", "u64", "u128", "usize", "f32",
    "f64",
];

/// Whether a bare owner denotes a Rust primitive rather than a crate root.
pub(crate) fn primitive_owner(name: &str) -> bool {
    PRIMITIVE_NAMES.contains(&name) || matches!(name, "str" | "[]")
}

/// Public items and visibility-only children of a crate's public module tree. Private modules are never walked;
/// their fences let source admission refuse a private prefix without publishing its contents.
pub(super) fn public_items(tcx: TyCtxt<'_>, root: DefId, name: &str) -> Vec<(String, DefId)> {
    let mut items = Vec::new();
    let mut walked = HashSet::new();
    let mut pending = vec![(name.to_owned(), root)];
    while let Some((path, module)) = pending.pop() {
        if !walked.insert(module) {
            continue;
        }
        for child in tcx.module_children(module).iter() {
            let Some(def) = child.res.opt_def_id() else {
                continue;
            };
            let child_path = format!("{path}::{}", child.ident.name);
            if !child.vis.is_public() {
                if resolve(tcx, &child_path, false).is_none() {
                    items.push((child_path, def));
                }
                continue;
            }
            items.extend(enum_constructors(tcx, def, &child_path));
            match tcx.def_kind(def) {
                DefKind::Mod => {
                    items.push((child_path.clone(), def));
                    pending.push((child_path, def));
                }
                DefKind::Struct
                | DefKind::Enum
                | DefKind::Union
                | DefKind::TyAlias
                | DefKind::Trait
                | DefKind::Fn
                | DefKind::Const { .. }
                | DefKind::Static { .. } => {
                    items.extend(public_associated_constants(tcx, def, &child_path));
                    items.push((child_path, def));
                }
                DefKind::Macro(kinds) if !macro_forms(kinds).is_empty() => items.push((child_path, def)),
                _ => {}
            }
        }
    }
    items
}

/// The invocation forms a macro definition supports that source declarations can name: derive and attribute.
/// A definition of only function-like macros has no such form.
pub(super) fn macro_forms(kinds: MacroKinds) -> Vec<RustMacroForm> {
    let mut forms = Vec::new();
    if kinds.contains(MacroKinds::DERIVE) {
        forms.push(RustMacroForm::Derive);
    }
    if kinds.contains(MacroKinds::ATTR) {
        forms.push(RustMacroForm::Attribute);
    }
    forms
}

/// Expose the exact public tuple and unit constructors with an enum in either a full ABI or fallback extraction.
/// Both paths use the same visibility and non-exhaustive-construction boundary as executable callee resolution.
pub(super) fn enum_constructors(tcx: TyCtxt<'_>, owner: DefId, path: &str) -> Vec<(String, DefId)> {
    if tcx.def_kind(owner) != DefKind::Enum {
        return Vec::new();
    }
    tcx.adt_def(owner)
        .variants()
        .iter()
        .filter_map(|variant| {
            crate::callees::enum_constructor(tcx, owner, variant.name.as_str())
                .map(|constructor| (format!("{path}::{}", variant.name), constructor))
        })
        .collect()
}

/// Return public inherent constants under their owner's exported path, preserving the exact associated item.
/// Trait constants and generic instantiations need a selected impl and are not inferred by this lookup.
pub(super) fn public_associated_constants(tcx: TyCtxt<'_>, owner: DefId, path: &str) -> Vec<(String, DefId)> {
    if !matches!(tcx.def_kind(owner), DefKind::Struct | DefKind::Enum | DefKind::Union) {
        return Vec::new();
    }
    tcx.inherent_impls(owner)
        .iter()
        .flat_map(|implementation| tcx.associated_item_def_ids(*implementation).iter().copied())
        .filter(|item| matches!(tcx.def_kind(*item), DefKind::AssocConst { .. }) && tcx.visibility(*item).is_public())
        .map(|item| (format!("{path}::{}", tcx.item_name(item)), item))
        .collect()
}

/// Resolve a path through public module children and inherent constants. Ordinary lookup prefers types over values and
/// macros; derive lookup selects only the macro namespace so a same-named trait and derive retain independent facts. A
/// type may precede a final associated-constant segment; competing impl constants remain unresolved rather than
/// selecting one by iteration order.
pub(crate) fn resolve(tcx: TyCtxt<'_>, path: &str, derive_macro: bool) -> Option<DefId> {
    let mut segments = path.split("::").peekable();
    let root = segments.next()?;
    let mut current = crate::native_crates::root(tcx, root)?;
    while let Some(segment) = segments.next() {
        let last = segments.peek().is_none();
        if tcx.def_kind(current) != DefKind::Mod {
            if !last {
                return None;
            }
            if tcx.def_kind(current) == DefKind::Enum
                && let Some(constructor) = crate::callees::enum_constructor(tcx, current, segment)
            {
                return Some(constructor);
            }
            let mut constants = public_associated_constants(tcx, current, "")
                .into_iter()
                .filter(|(_, item)| tcx.item_name(*item).as_str() == segment);
            let (_, selected) = constants.next()?;
            return constants.next().is_none().then_some(selected);
        }
        let children = tcx
            .module_children(current)
            .iter()
            .filter(|child| child.ident.name.as_str() == segment && child.vis.is_public())
            .filter_map(|child| child.res.opt_def_id())
            .collect::<Vec<_>>();
        current = if last {
            children
                .iter()
                .copied()
                .filter(|child| {
                    !derive_macro
                        || matches!(tcx.def_kind(*child), DefKind::Macro(kinds) if kinds.contains(MacroKinds::DERIVE))
                })
                .min_by_key(|child| resolution_rank(tcx.def_kind(*child)))?
        } else {
            children.into_iter().min_by_key(|child| match tcx.def_kind(*child) {
                DefKind::Mod => 0,
                DefKind::Struct | DefKind::Enum | DefKind::Union => 1,
                _ => 2,
            })?
        };
    }
    Some(current)
}

/// The order a path's last segment prefers its namespaces in: types, then values, then modules.
pub(super) fn resolution_rank(kind: DefKind) -> u8 {
    match kind {
        DefKind::Struct | DefKind::Enum | DefKind::Union | DefKind::TyAlias | DefKind::Trait => 0,
        DefKind::Fn | DefKind::Const { .. } | DefKind::Static { .. } => 1,
        DefKind::Mod => 2,
        _ => 3,
    }
}

/// The path rustc shows a definition under from outside its crate (`std::fs::File`, not its definition path).
pub(super) fn visible_path(tcx: TyCtxt<'_>, def: DefId) -> String {
    ty::print::with_no_trimmed_paths!(tcx.def_path_str(def))
}

/// A type's display as the checker parses it: full visible paths, no trimming.
pub(super) fn display(ty: Ty<'_>) -> String {
    ty::print::with_no_trimmed_paths!(ty.to_string())
}
