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
    RustMethodSig, RustModuleChild, RustModuleChildKind, RustModuleInfo, RustParam, RustTraitAssoc, RustTraitBound,
    RustTraitInfo, RustTypeInfo, RustTypeShape, RustVariantInfo, RustVisibility,
};
use rustc_hir::def::DefKind;
use rustc_hir::def_id::DefId;
use rustc_interface::interface;
use rustc_middle::ty::{self, Ty, TyCtxt};
use rustc_session::config::Input;
use rustc_span::FileName;

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
    pub sysroot: PathBuf,
    pub externs: Vec<(String, PathBuf)>,
    pub search: Vec<PathBuf>,
}

impl incan_driver::session::RustFactsSource for RustcFacts {
    /// Read uncovered paths from exactly the unit's declared native artifacts.
    fn facts(&self, paths: &BTreeSet<String>) -> Result<BTreeMap<String, RustItemMetadata>, String> {
        let started = std::time::Instant::now();
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

/// The named paths, then the nominal types their signatures name, `depth` steps out.
fn describe_paths(tcx: TyCtxt<'_>, paths: &BTreeSet<String>, depth: usize) -> BTreeMap<String, RustItemMetadata> {
    let mut facts = BTreeMap::new();
    let mut pending = paths
        .iter()
        .filter_map(|path| resolve(tcx, path).map(|def| (path.clone(), def)))
        .collect::<Vec<_>>();
    for _ in 0..=depth {
        let mut named = HashSet::new();
        for (path, def) in std::mem::take(&mut pending) {
            if facts.contains_key(&path) {
                continue;
            }
            let metadata = describe(tcx, def, &path, &mut named);
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

/// Every public item of the named crates, under each public module path that reaches it.
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
            facts.insert(path, metadata);
        }
    }
    if crates
        .iter()
        .any(|name| matches!(name.as_str(), "std" | "core" | "alloc"))
    {
        facts.insert(
            "str".into(),
            RustItemMetadata {
                canonical_path: "str".into(),
                definition_path: None,
                visibility: RustVisibility::Public,
                kind: RustItemKind::Type(RustTypeInfo {
                    implemented_traits: trait_impls(tcx, tcx.types.str_, &mut named),
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
                | DefKind::Static { .. } => items.push((child_path, def)),
                _ => {}
            }
        }
    }
    items
}

/// Resolve a path through its crate's public module children; the last segment prefers a type, then a value.
fn resolve(tcx: TyCtxt<'_>, path: &str) -> Option<DefId> {
    let mut segments = path.split("::").peekable();
    let root = segments.next()?;
    let mut current = crate::native_crates::root(tcx, root)?;
    while let Some(segment) = segments.next() {
        let last = segments.peek().is_none();
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
                .min_by_key(|child| resolution_rank(tcx.def_kind(*child)))?
        } else {
            children
                .into_iter()
                .find(|child| tcx.def_kind(*child) == DefKind::Mod)?
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

/// One item's metadata; the nominal types its signatures name join `named`.
fn describe(tcx: TyCtxt<'_>, def: DefId, path: &str, named: &mut HashSet<DefId>) -> RustItemMetadata {
    let kind = match tcx.def_kind(def) {
        DefKind::Struct | DefKind::Enum | DefKind::Union => RustItemKind::Type(type_info(tcx, def, named)),
        DefKind::TyAlias => RustItemKind::Type(RustTypeInfo {
            alias_target: Some(display(tcx.type_of(def).instantiate_identity().skip_normalization())),
            ..RustTypeInfo::default()
        }),
        DefKind::Fn => RustItemKind::Function(signature(tcx, def, named)),
        DefKind::Const { .. } | DefKind::Static { .. } => RustItemKind::Constant {
            type_display: display(tcx.type_of(def).instantiate_identity().skip_normalization()),
        },
        DefKind::Trait => RustItemKind::Trait(trait_info(tcx, def, named)),
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
    let methods = tcx
        .inherent_impls(def)
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
        .collect();
    let adt = tcx.adt_def(def);
    let fields = if adt.is_struct() {
        adt.all_fields()
            .filter(|field| field.vis.is_public())
            .map(|field| RustFieldInfo {
                name: field.name.to_string(),
                type_display: display(tcx.type_of(field.did).instantiate_identity().skip_normalization()),
                type_shape: RustTypeShape::Unknown,
            })
            .collect()
    } else {
        Vec::new()
    };
    let variants = if adt.is_enum() {
        adt.variants()
            .iter()
            .map(|variant| RustVariantInfo {
                name: variant.name.to_string(),
                fields: variant.fields.iter().map(|_| RustTypeShape::Unknown).collect(),
                field_carriers: Vec::new(),
            })
            .collect()
    } else {
        Vec::new()
    };
    RustTypeInfo {
        type_params: parameters.iter().map(|parameter| parameter.name.to_string()).collect(),
        type_param_defaults,
        methods,
        implemented_traits: trait_impls(tcx, tcx.type_of(def).instantiate_identity().skip_normalization(), named),
        fields,
        variants,
        ..RustTypeInfo::default()
    }
}

/// A function's signature: parameters with a method's receiver first, as written (`&self`, `&mut self`, `self`).
/// Async signatures expose the declared future output; the separate async flag retains the pending-call boundary.
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
                display(*input)
            };
            RustParam {
                name: (receiver && index == 0).then(|| "self".to_owned()),
                type_display,
            }
        })
        .collect();
    let is_async = tcx.asyncness(def).is_async();
    let output = if is_async {
        async_output(tcx, signature.output()).unwrap_or(signature.output())
    } else {
        signature.output()
    };
    note_named(output, named);
    let type_params = tcx
        .generics_of(def)
        .own_params
        .iter()
        .filter(|parameter| matches!(parameter.kind, ty::GenericParamDefKind::Type { .. }))
        .map(|parameter| parameter.name.to_string())
        .collect();
    RustFunctionSig {
        // No receiver contract: a missing fact authorizes no inferred borrowing.
        receiver_contract: None,
        bounds: trait_bounds(tcx, def),
        type_params,
        params,
        return_type: display(output),
        is_async,
        is_unsafe: signature.safety().is_unsafe(),
    }
}

/// Preserve trait predicates and projection equalities without flattening Fn inputs or inventing an output.
fn trait_bounds(tcx: TyCtxt<'_>, def: DefId) -> Vec<RustTraitBound> {
    let predicates = tcx.predicates_of(def).instantiate_identity(tcx).predicates;
    let mut bounds = Vec::new();
    for clause in &predicates {
        let Some(predicate) = clause.as_trait_clause() else {
            continue;
        };
        let reference = predicate.skip_binder().trait_ref;
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
fn trait_impls<'tcx>(tcx: TyCtxt<'tcx>, owner: Ty<'tcx>, named: &mut HashSet<DefId>) -> Vec<RustImplementedTrait> {
    let mut implementations = HashSet::new();
    for trait_id in tcx
        .all_traits_including_private()
        .filter(|id| tcx.visibility(*id).is_public())
    {
        tcx.for_each_relevant_impl(trait_id, owner, |implementation| {
            implementations.insert(implementation);
        });
    }
    let mut implementations = implementations.into_iter().collect::<Vec<_>>();
    implementations.sort_by_cached_key(|implementation| tcx.def_path_str(*implementation));
    implementations
        .into_iter()
        .filter_map(|implementation| {
            if tcx.impl_polarity(implementation) != ty::ImplPolarity::Positive {
                return None;
            }
            if owner == tcx.types.str_ && !concrete_impl_may_apply(tcx, implementation, owner) {
                return None;
            }
            let reference = tcx
                .impl_trait_ref(implementation)
                .instantiate_identity()
                .skip_normalization();
            if tcx
                .generics_of(implementation)
                .own_params
                .iter()
                .any(|parameter| matches!(parameter.kind, ty::GenericParamDefKind::Const { .. }))
            {
                return None;
            }
            let associated_type_bindings = tcx
                .associated_item_def_ids(implementation)
                .iter()
                .copied()
                .filter(|item| tcx.def_kind(*item) == DefKind::AssocTy)
                .filter_map(|item| {
                    Some(RustAssociatedTypeBinding {
                        name: tcx.opt_item_name(item)?.to_string(),
                        value_path: display(tcx.type_of(item).instantiate_identity().skip_normalization()),
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
                bounds: trait_bounds(tcx, implementation),
                associated_type_bindings,
            })
        })
        .collect()
}

/// Read an async function's declared `Future::Output` from its opaque return bounds, preserving generic arguments.
/// No hidden future layout is revealed, and an unproven output keeps the original opaque signature for refusal.
fn async_output<'tcx>(tcx: TyCtxt<'tcx>, output: Ty<'tcx>) -> Option<Ty<'tcx>> {
    let ty::Alias(_, alias) = output.kind() else {
        return None;
    };
    let ty::Opaque { def_id, .. } = alias.kind else {
        return None;
    };
    let output_item = tcx.lang_items().future_output()?;
    tcx.explicit_item_bounds(def_id)
        .iter_instantiated_copied(tcx, alias.args)
        .find_map(|bound| {
            let (clause, _) = bound.skip_normalization();
            let projection = clause.as_projection_clause()?.skip_binder();
            if !matches!(projection.projection_term.kind, ty::AliasTermKind::ProjectionTy { def_id } if def_id == output_item) {
                return None;
            }
            projection.term.as_type()
        })
}

/// Eliminate impossible blanket candidates for the concrete unsized primitive. Nominal generic owners retain their
/// conditional templates because future instantiations may satisfy bounds their declaration does not assume.
fn concrete_impl_may_apply<'tcx>(tcx: TyCtxt<'tcx>, implementation: DefId, owner: Ty<'tcx>) -> bool {
    use rustc_infer::infer::TyCtxtInferExt;
    use rustc_trait_selection::traits::{ObligationCtxt, query::evaluate_obligation::InferCtxtExt};
    let (infcx, param_env) = tcx
        .infer_ctxt()
        .build_with_typing_env(ty::TypingEnv::fully_monomorphized());
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

/// The nominal types a signature names, for the next extraction step.
fn note_named(ty: Ty<'_>, named: &mut HashSet<DefId>) {
    for argument in ty.walk() {
        if let Some(inner) = argument.as_type()
            && let ty::Adt(adt, _) = inner.kind()
        {
            named.insert(adt.did());
        }
    }
}

/// A trait's associated functions and their signatures.
fn trait_info(tcx: TyCtxt<'_>, def: DefId, named: &mut HashSet<DefId>) -> RustTraitInfo {
    RustTraitInfo {
        is_in_prelude: resolve(tcx, "std::prelude::rust_2024").is_some_and(|prelude| {
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
