//! Rust facts from rustc's own crate metadata (H9): the checker's view of the Rust items a unit names.
//!
//! One rustc session over a stub crate that links the unit's externs resolves each requested path through public
//! module children, as [`crate::callees`] resolves a callee, and describes the item in the checker's metadata shape: a
//! type with its generics, public inherent methods, fields and variants; a function's signature; a constant's type; a
//! trait's functions; a module's children. The nominal types a described signature names are described too, to
//! `depth` steps, so the checker can call on a call's result.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use incan_lang::interop::{
    RustAssociatedTypeBinding, RustFieldInfo, RustFunctionSig, RustImplementedTrait, RustItemKind, RustItemMetadata,
    RustMacroForm, RustMacroInfo, RustMethodSig, RustModuleChild, RustModuleChildKind, RustModuleInfo, RustParam,
    RustPayloadCarrier, RustTraitAssoc, RustTraitBound, RustTraitInfo, RustTypeInfo, RustTypeShape, RustVariantInfo,
    RustVisibility,
};
use rustc_hir::def::{DefKind, MacroKinds};
use rustc_hir::def_id::DefId;
use rustc_interface::interface;
use rustc_middle::ty::{self, Ty, TyCtxt, TypeFoldable, TypeFolder};
use rustc_session::config::Input;
use rustc_span::{FileName, sym};

/// What one extraction reads: what to describe, and the crates and directories that resolve it.
pub struct RustFactsRequest<'a> {
    pub sysroot: &'a Path,
    pub externs: &'a [(String, PathBuf)],
    pub search: &'a [PathBuf],
    pub scope: RustFactsScope,
}

/// What one extraction describes.
pub enum RustFactsScope {
    /// The named paths, and the nominal types their signatures name `depth` steps out: the paths a unit imports that
    /// no ABI cache covers.
    Paths { paths: BTreeSet<String>, depth: usize },
    /// Every public item of the named crates: their ABI cache.
    Crates(BTreeSet<String>),
}

impl RustFactsScope {
    /// The crates the extraction reads, which the stub names (`std`, `core` and `alloc` are always loaded).
    fn roots(&self) -> BTreeSet<String> {
        let names: Box<dyn Iterator<Item = &str>> = match self {
            Self::Paths { paths, .. } => Box::new(paths.iter().filter_map(|path| path.split("::").next())),
            Self::Crates(crates) => Box::new(crates.iter().map(String::as_str)),
        };
        names
            .filter(|root| !matches!(*root, "std" | "core" | "alloc"))
            .map(str::to_owned)
            .collect()
    }
}

/// The compiler's rustc as a session's Rust facts source: the unit's own crates, read for the paths no ABI cache holds.
#[derive(Debug)]
pub struct RustcFacts {
    /// Exact native generation that identifies a bounded fallback extraction cache.
    pub unit: incan_driver::unit::UnitDescription,
    pub sysroot: PathBuf,
    pub externs: Vec<(String, PathBuf)>,
    pub search: Vec<PathBuf>,
}

impl incan_driver::session::RustFactsSource for RustcFacts {
    /// Reuse the exact bounded extraction for this request and native generation, after shipped ABI lookup.
    fn facts(&self, paths: &BTreeSet<String>) -> Result<BTreeMap<String, RustItemMetadata>, String> {
        incan_driver::rust_abi::cached_path_facts(&self.unit, paths, || self.extract_paths(paths))
            .map_err(|error| error.message)
    }
}

impl RustcFacts {
    /// Read uncovered paths from exactly the unit's declared native artifacts, retaining the existing depth bound
    /// and missing-artifact refusals. Persistent storage must never broaden this extraction's scope.
    fn extract_paths(&self, paths: &BTreeSet<String>) -> Result<BTreeMap<String, RustItemMetadata>, String> {
        let started = std::time::Instant::now();

        // ---- Linked native paths ----
        let (linked, unavailable): (BTreeSet<_>, BTreeSet<_>) = paths.iter().cloned().partition(|path| {
            let root = path.split("::").next().unwrap_or(path);
            matches!(root, "std" | "core" | "alloc") || self.externs.iter().any(|(name, _)| name == root)
        });
        let mut facts = extract(RustFactsRequest {
            sysroot: &self.sysroot,
            externs: &self.externs,
            search: &self.search,
            scope: RustFactsScope::Paths {
                paths: linked,
                depth: 1,
            },
        })?;
        // ---- Missing artifact diagnostics ----
        // Keep missing-artifact refusals at the imported item, so they do not mask independent source diagnostics.
        for path in unavailable {
            let root = path.split("::").next().unwrap_or(&path);
            let description = format!("Rust facts require an exact `{root}` artifact in the unit's native closure");
            facts.insert(
                path.clone(),
                RustItemMetadata {
                    canonical_path: path,
                    definition_path: None,
                    visibility: RustVisibility::Public,
                    kind: RustItemKind::Unsupported { description },
                },
            );
        }
        // ---- Optional extraction trace ----
        if std::env::var_os("INCAN_TRACE_RUST_FACTS").is_some() {
            eprintln!(
                "rust facts: {} uncovered paths, {} items in {:?}",
                paths.len(),
                facts.len(),
                started.elapsed()
            );
        }
        Ok(facts)
    }
}

/// Describe what `request.scope` names, keyed by path.
pub fn extract(request: RustFactsRequest<'_>) -> Result<BTreeMap<String, RustItemMetadata>, String> {
    let roots = request.scope.roots();
    for root in &roots {
        let mut artifacts = request.externs.iter().filter(|(name, _)| name == root);
        let Some((_, artifact)) = artifacts.next() else {
            return Err(format!(
                "Rust facts require an exact `{root}` artifact in the unit's native closure"
            ));
        };
        if artifacts.any(|(_, other)| other != artifact) {
            return Err(format!("Rust facts received conflicting --extern artifacts for {root}"));
        }
    }
    let stub = roots
        .iter()
        .map(|root| format!("extern crate {root};\n"))
        .collect::<String>();
    let mut args = vec![
        "incan-rustc-driver".to_owned(),
        "--sysroot".to_owned(),
        request.sysroot.to_string_lossy().into_owned(),
        "--crate-name".to_owned(),
        "incan_rust_facts".to_owned(),
        "--crate-type".to_owned(),
        "lib".to_owned(),
        "--edition".to_owned(),
        "2024".to_owned(),
        "--cap-lints".to_owned(),
        "allow".to_owned(),
        "incan_rust_facts.rs".to_owned(),
    ];
    for (name, path) in request.externs {
        args.push("--extern".to_owned());
        args.push(format!("{name}={}", path.display()));
    }
    // Direct roots come only from --extern; directories locate their hash-qualified transitive dependencies.
    for directory in request.search {
        args.push("-L".to_owned());
        args.push(format!("dependency={}", directory.display()));
    }
    let mut callbacks = FactsCallbacks {
        stub,
        scope: request.scope,
        facts: BTreeMap::new(),
    };
    rustc_driver::catch_fatal_errors(|| rustc_driver::run_compiler(&args, &mut callbacks))
        .map_err(|_| "rustc could not load the crates the Rust facts name".to_owned())?;
    Ok(callbacks.facts)
}

/// The stub crate session: install the stub source, describe the scope once the crates are loaded, then stop.
struct FactsCallbacks {
    stub: String,
    scope: RustFactsScope,
    facts: BTreeMap<String, RustItemMetadata>,
}

impl rustc_driver::Callbacks for FactsCallbacks {
    fn config(&mut self, config: &mut interface::Config) {
        config.input = Input::Str {
            name: FileName::Custom("incan_rust_facts".into()),
            input: self.stub.clone(),
        };
    }

    fn after_expansion(&mut self, _compiler: &interface::Compiler, tcx: TyCtxt<'_>) -> rustc_driver::Compilation {
        self.facts = match &self.scope {
            RustFactsScope::Paths { paths, depth } => describe_paths(tcx, paths, *depth),
            RustFactsScope::Crates(crates) => describe_crates(tcx, crates),
        };
        rustc_driver::Compilation::Stop
    }
}

/// Describe named paths and bounded signature dependencies, closing every selected public field layout transitively.
/// Field records can be reached through nested Options and collections without a source import of each record.
/// Their types need the same metadata as an imported owner; signature expansion stays bounded independently.
fn describe_paths(tcx: TyCtxt<'_>, paths: &BTreeSet<String>, depth: usize) -> BTreeMap<String, RustItemMetadata> {
    let mut facts = BTreeMap::new();
    let mut pending = paths
        .iter()
        .filter_map(|path| resolve(tcx, path, false).map(|def| (path.clone(), def)))
        .collect::<Vec<_>>();
    for _ in 0..=depth {
        let mut named = HashSet::new();
        while let Some((path, def)) = pending.pop() {
            if facts.contains_key(&path) {
                continue;
            }
            pending.extend(
                layout_dependencies(tcx, def)
                    .into_iter()
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
            .map(|def| (visible_path(tcx, def), def))
            .filter(|(path, _)| !facts.contains_key(path))
            .collect();
    }
    facts
}

/// Collect public field and enum payload owners, including generic arguments, without expanding method signatures.
/// Opaque private fields grant no access. The caller deduplicates item identities so recursive layouts terminate.
fn layout_dependencies(tcx: TyCtxt<'_>, def: DefId) -> HashSet<DefId> {
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
fn describe_crates(tcx: TyCtxt<'_>, crates: &BTreeSet<String>) -> BTreeMap<String, RustItemMetadata> {
    let mut facts = BTreeMap::new();
    let mut named = HashSet::new();
    let mut described = HashMap::new();
    for name in crates {
        let Some(krate) = crate::native_crates::root(tcx, name) else {
            continue;
        };
        for (path, def) in public_items(tcx, krate, name) {
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
    // Trait references use definition identities, including traits reached through a public re-export.
    for def in named.into_iter().filter(|def| tcx.def_kind(*def) == DefKind::Trait) {
        let path = visible_path(tcx, def);
        facts
            .entry(path.clone())
            .or_insert_with(|| describe(tcx, def, &path, &mut HashSet::new()));
    }
    facts
}

/// Publish the same unsized text owner under its primitive re-exports. Their methods and trait facts come from
/// rustc's primitive impl bucket, just as the bare `str` path does; the alias never becomes a nominal empty type.
fn describe_str(tcx: TyCtxt<'_>, facts: &mut BTreeMap<String, RustItemMetadata>, named: &mut HashSet<DefId>) {
    let info = RustTypeInfo {
        methods: inherent_methods(tcx, tcx.incoherent_impls(ty::fast_reject::SimplifiedType::Str), named),
        implemented_traits: trait_impls(tcx, tcx.types.str_, named),
        ..RustTypeInfo::default()
    };
    for path in ["str", "std::primitive::str", "core::primitive::str"] {
        let definition_path = (path != "str").then(|| "str".to_owned());
        let mut selected = info.clone();
        selected.alias_target = definition_path.clone();
        facts.insert(
            path.to_owned(),
            RustItemMetadata {
                canonical_path: path.to_owned(),
                definition_path,
                visibility: RustVisibility::Public,
                kind: RustItemKind::Type(selected),
            },
        );
    }
}

/// Publish primitive trait facts under their native and standard public paths. Primitive items have no DefId for
/// the public module walker; their impl templates and associated types still come from rustc's exact scalar type.
/// Public primitive re-exports are transparent aliases to that scalar, never new nominal layouts.
fn describe_primitives(tcx: TyCtxt<'_>, facts: &mut BTreeMap<String, RustItemMetadata>, named: &mut HashSet<DefId>) {
    let primitives = [
        ("bool", tcx.types.bool),
        ("char", tcx.types.char),
        ("i8", tcx.types.i8),
        ("i16", tcx.types.i16),
        ("i32", tcx.types.i32),
        ("i64", tcx.types.i64),
        ("i128", tcx.types.i128),
        ("isize", tcx.types.isize),
        ("u8", tcx.types.u8),
        ("u16", tcx.types.u16),
        ("u32", tcx.types.u32),
        ("u64", tcx.types.u64),
        ("u128", tcx.types.u128),
        ("usize", tcx.types.usize),
        ("f32", tcx.types.f32),
        ("f64", tcx.types.f64),
    ];
    for (name, native) in primitives {
        let info = RustTypeInfo {
            implemented_traits: trait_impls(tcx, native, named),
            ..RustTypeInfo::default()
        };
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
}

/// The public items of one crate's module tree, each module walked once under the first path that reaches it.
fn public_items(tcx: TyCtxt<'_>, root: DefId, name: &str) -> Vec<(String, DefId)> {
    let mut items = Vec::new();
    let mut walked = HashSet::new();
    let mut pending = vec![(name.to_owned(), root)];
    while let Some((path, module)) = pending.pop() {
        if !walked.insert(module) {
            continue;
        }
        for child in tcx.module_children(module).iter().filter(|child| child.vis.is_public()) {
            let Some(def) = child.res.opt_def_id() else {
                continue;
            };
            let child_path = format!("{path}::{}", child.ident.name);
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
fn macro_forms(kinds: MacroKinds) -> Vec<RustMacroForm> {
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
fn enum_constructors(tcx: TyCtxt<'_>, owner: DefId, path: &str) -> Vec<(String, DefId)> {
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
fn public_associated_constants(tcx: TyCtxt<'_>, owner: DefId, path: &str) -> Vec<(String, DefId)> {
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
fn resolution_rank(kind: DefKind) -> u8 {
    match kind {
        DefKind::Struct | DefKind::Enum | DefKind::Union | DefKind::TyAlias | DefKind::Trait => 0,
        DefKind::Fn | DefKind::Const { .. } | DefKind::Static { .. } => 1,
        DefKind::Mod => 2,
        _ => 3,
    }
}

/// The path rustc shows a definition under from outside its crate (`std::fs::File`, not its definition path).
fn visible_path(tcx: TyCtxt<'_>, def: DefId) -> String {
    ty::print::with_no_trimmed_paths!(tcx.def_path_str(def))
}

/// A type's display as the checker parses it: full visible paths, no trimming.
fn display(ty: Ty<'_>) -> String {
    ty::print::with_no_trimmed_paths!(ty.to_string())
}

/// Fold signature constants under their declaring item's environment without normalizing aliases or opaque types.
/// Concrete array lengths must cross the metadata boundary as values; unresolved generic constants stay explicit.
struct SignatureConstants<'tcx> {
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
fn signature_display<'tcx>(tcx: TyCtxt<'tcx>, item: DefId, value: Ty<'tcx>) -> String {
    let value = value.fold_with(&mut SignatureConstants {
        tcx,
        environment: ty::TypingEnv::non_body_analysis(tcx, item),
    });
    display(value)
}

/// One item's metadata; the nominal types its signatures name join `named`.
fn describe(tcx: TyCtxt<'_>, def: DefId, path: &str, named: &mut HashSet<DefId>) -> RustItemMetadata {
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
        visibility: RustVisibility::Public,
        kind,
    }
}

/// A struct, enum or union: type parameters and defaults, inherent methods with native impl binders and self types,
/// public fields and variants. Method binders remain distinct from the owner's potentially defaulted parameters.
fn type_info(tcx: TyCtxt<'_>, def: DefId, named: &mut HashSet<DefId>) -> RustTypeInfo {
    let (type_params, type_param_defaults) = type_parameters(tcx, def);
    let methods = inherent_methods(tcx, &crate::callees::owner_inherent_impls(tcx, def), named);
    let adt = tcx.adt_def(def);
    let fields = if adt.is_struct() {
        adt.all_fields()
            .filter(|field| field.vis.is_public())
            .map(|field| {
                let ty = tcx.type_of(field.did).instantiate_identity().skip_normalization();
                note_named(ty, named);
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

/// Retain alias binders and candidate implementations on its actual target. An alias introduces no new native
/// owner; the checker expands it for selection while executable storage may keep the public alias identity.
fn alias_info(tcx: TyCtxt<'_>, def: DefId, named: &mut HashSet<DefId>) -> RustTypeInfo {
    let target = tcx.type_of(def).instantiate_identity().skip_normalization();
    note_named(target, named);
    let (type_params, type_param_defaults) = type_parameters(tcx, def);
    RustTypeInfo {
        has_lifetime_params: has_lifetime_parameters(tcx, def),
        type_params,
        type_param_defaults,
        alias_target: Some(display(target)),
        implemented_traits: trait_impls(tcx, target, named),
        ..RustTypeInfo::default()
    }
}

/// Retain lifetime presence before the extractor elides their labels from the source type-argument vocabulary.
fn has_lifetime_parameters(tcx: TyCtxt<'_>, def: DefId) -> bool {
    tcx.generics_of(def)
        .own_params
        .iter()
        .any(|parameter| matches!(parameter.kind, ty::GenericParamDefKind::Lifetime))
}

/// Extract type binders and their declaration-order defaults identically for nominal owners and aliases.
fn type_parameters(tcx: TyCtxt<'_>, def: DefId) -> (Vec<String>, Vec<Option<String>>) {
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
fn inherent_methods(tcx: TyCtxt<'_>, implementations: &[DefId], named: &mut HashSet<DefId>) -> Vec<RustMethodSig> {
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

/// Project a metadata-selected field type into the checker's structural carrier without parsing its display.
/// Nominal paths and type parameters stay distinct; references retain the payload's semantic type for matching.
/// Sized primitives keep their native names; only Rust's i64 and f64 use the Incan scalar shapes.
fn type_shape<'tcx>(tcx: TyCtxt<'tcx>, field: Ty<'tcx>) -> RustTypeShape {
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
fn signature(tcx: TyCtxt<'_>, def: DefId, named: &mut HashSet<DefId>) -> RustFunctionSig {
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
fn trait_bounds(tcx: TyCtxt<'_>, def: DefId, named: &mut HashSet<DefId>) -> Vec<RustTraitBound> {
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
fn trait_impls<'tcx>(tcx: TyCtxt<'tcx>, owner: Ty<'tcx>, named: &mut HashSet<DefId>) -> Vec<RustImplementedTrait> {
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
    implementations.sort_by_cached_key(|implementation| tcx.def_path_str(*implementation));
    implementations
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
        .collect()
}

/// Extract one positive implementation once for both owner and trait ABIs. Const binders remain unsupported;
/// associated definitions and complete retained trait predicates keep their native declaration identities. Nominal
/// associated outputs join the same metadata closure as method returns, so their public methods remain inspectable.
fn trait_impl_template(
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
fn trait_implementations(tcx: TyCtxt<'_>, owner: DefId, named: &mut HashSet<DefId>) -> Vec<RustImplementedTrait> {
    let templates = tcx.trait_impls_of(owner);
    let mut implementations = templates
        .blanket_impls()
        .iter()
        .chain(templates.non_blanket_impls().values().flatten())
        .copied()
        .collect::<Vec<_>>();
    implementations.sort_by_cached_key(|implementation| tcx.def_path_str(*implementation));
    implementations
        .into_iter()
        .filter_map(|implementation| trait_impl_template(tcx, implementation, named))
        .collect()
}

/// Eliminate impossible blanket candidates for scalar and unsized primitives. A slice element remains unconstrained
/// during extraction, so applicable conditional templates survive for the checker to instantiate and rustc to verify.
/// A reference retains its Sized carrier while its slice element remains an inference variable.
fn concrete_impl_may_apply<'tcx>(tcx: TyCtxt<'tcx>, implementation: DefId, owner: Ty<'tcx>) -> bool {
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
fn receiver_display(input: Ty<'_>) -> String {
    match input.kind() {
        ty::Ref(_, _, ty::Mutability::Not) => "&self".to_owned(),
        ty::Ref(_, _, ty::Mutability::Mut) => "&mut self".to_owned(),
        ty::Adt(..) | ty::Param(_) => "self".to_owned(),
        _ => display(input),
    }
}

/// Retain the nominal types and dynamic trait principals a signature names for the next extraction step.
fn note_named(ty: Ty<'_>, named: &mut HashSet<DefId>) {
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

/// Retain a trait's bounds and associated signatures in the same named-dependency closure as ordinary functions.
fn trait_info(tcx: TyCtxt<'_>, def: DefId, named: &mut HashSet<DefId>) -> RustTraitInfo {
    RustTraitInfo {
        bounds: trait_bounds(tcx, def, named),
        implementations: trait_implementations(tcx, def, named),
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
fn module_info(tcx: TyCtxt<'_>, def: DefId) -> RustModuleInfo {
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
