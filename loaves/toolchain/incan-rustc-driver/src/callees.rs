//! Canonical path resolution against rustc metadata, never generated function names.

use crate::error::PlanError;
use crate::plan::{Callee, CalleeKind};
use rustc_hir::def::DefKind;
use rustc_hir::def_id::DefId;
use rustc_middle::ty::TyCtxt;
use rustc_middle::ty::fast_reject::SimplifiedType;

/// Resolve a retained public module constant, inherent associated constant or unit-variant constructor. Inherent
/// constants use the same metadata lookup as checking; statics and trait constants still require their own
/// contracts, and a variant must belong to the exact public enum and expose a public constant constructor.
pub fn constant(tcx: TyCtxt<'_>, path: &str) -> Result<DefId, PlanError> {
    if let Ok(constant) = library_item(tcx, path, DefKind::Const { is_type_const: false }) {
        return Ok(constant);
    }
    if let Some(constant) = crate::rust_facts::resolve(tcx, path, false)
        .filter(|def| matches!(tcx.def_kind(*def), DefKind::AssocConst { .. }))
    {
        return Ok(constant);
    }
    let (owner, name) = path
        .rsplit_once("::")
        .ok_or_else(|| PlanError::UnknownCallee(path.into()))?;
    let owner = library_item(tcx, owner, DefKind::Enum)?;
    enum_constructor(tcx, owner, name)
        .filter(|constructor| {
            matches!(
                tcx.def_kind(*constructor),
                DefKind::Ctor(_, rustc_hir::def::CtorKind::Const)
            )
        })
        .ok_or_else(|| PlanError::UnknownCallee(path.into()))
}

/// Resolve a planned source name, canonical external path, or explicit runtime operation to its exact function.
/// Explicit plan arguments are checked separately.
pub fn resolve(tcx: TyCtxt<'_>, callee: &Callee) -> Result<DefId, PlanError> {
    match &callee.kind {
        CalleeKind::Planned(name) => planned(tcx, name),
        CalleeKind::InstallPanicHook(signature) => planned(tcx, &crate::panic_hooks::name(signature)),
        CalleeKind::Foreign(declaration) => crate::foreign::resolve(tcx, &declaration.name),
        CalleeKind::FormatValue(_, debug) => planned(tcx, crate::formatting::name(*debug)),
        CalleeKind::Value(_) => Err(PlanError::UnknownCallee(
            "local callable has no declaration callee".into(),
        )),
        CalleeKind::CallClosure(_) => fn_call(tcx),
        CalleeKind::CloneClosure(signature) => crate::cloneable_closures::clone_method(tcx, signature),
        CalleeKind::CallClosureOnce(_) => fn_once_call(tcx),
        CalleeKind::ClosureBody(..) | CalleeKind::ClosureBodyOnce(..) => Err(PlanError::UnknownCallee(
            "closure constructors keep rustc's own body".into(),
        )),
        CalleeKind::SharedCallableBody(_) => Err(PlanError::UnknownCallee(
            "shared callable constructors keep rustc's own body".into(),
        )),
        CalleeKind::Instantiated(path, _) => external(tcx, path),
        CalleeKind::FutureMethod(..) => Err(PlanError::UnknownCallee(
            "unsupported SDK method future outside its boxing shim".into(),
        )),
        CalleeKind::SpawnGenerator(_, leaf, depth) => generator_method(tcx, "Generator", "spawn", leaf, *depth),
        CalleeKind::YieldGenerator(leaf, depth) => generator_method(tcx, "GeneratorYield", "yield_value", leaf, *depth),
        CalleeKind::CollectGenerator(leaf, depth) => generator_method(tcx, "Generator", "collect", leaf, *depth),
        CalleeKind::External(path) => external(tcx, path),
        CalleeKind::CloneModel(_, _) | CalleeKind::CloneEnum(_, _) => clone_method(tcx),
        CalleeKind::StaticLoad(slot) => planned(tcx, &crate::statics::load_name(*slot)),
        CalleeKind::StaticStore(slot) => planned(tcx, &crate::statics::store_name(*slot)),
        CalleeKind::StaticForce(slot) => planned(tcx, &crate::statics::force_name(*slot)),
        CalleeKind::ModuleInitialized(slot) => planned(tcx, &crate::statics::initialized_name(*slot)),
        CalleeKind::ModuleComplete(slot) => planned(tcx, &crate::statics::complete_name(*slot)),
        CalleeKind::FieldMetadata(owner, _) => planned(tcx, &crate::field_metadata::name(*owner)),
        CalleeKind::StaticMethod(slot, method) => planned(tcx, &crate::statics::method_name(*slot, method)),
    }
}

/// Resolve the `Clone::clone` trait method through the language item, never through a source-spelled path.
pub fn clone_method(tcx: TyCtxt<'_>) -> Result<DefId, PlanError> {
    let trait_id = tcx
        .lang_items()
        .clone_trait()
        .ok_or_else(|| PlanError::UnknownCallee("Clone".into()))?;
    tcx.associated_item_def_ids(trait_id)
        .iter()
        .copied()
        .find(|def| tcx.item_name(*def).as_str() == "clone")
        .ok_or_else(|| PlanError::UnknownCallee("Clone::clone".into()))
}

/// Select an inherent runtime method from the exact ADT, rather than from a source-spelled method name.
fn generator_method(
    tcx: TyCtxt<'_>,
    owner: &str,
    member: &str,
    leaf: &crate::plan::ListLeaf,
    depth: i64,
) -> Result<DefId, PlanError> {
    let ty = crate::types::generator_type(tcx, owner, leaf, depth)?;
    let rustc_middle::ty::Adt(adt, _) = ty.kind() else {
        return Err(PlanError::UnknownCallee(owner.into()));
    };
    tcx.inherent_impls(adt.did())
        .iter()
        .flat_map(|implementation| tcx.associated_item_def_ids(*implementation))
        .copied()
        .find(|def| tcx.item_name(*def).as_str() == member)
        .ok_or_else(|| PlanError::UnknownCallee(format!("{owner}::{member}")))
}

/// Select `Fn::call` from the language item, the one method a borrowed callable object is invoked through.
pub fn fn_call(tcx: TyCtxt<'_>) -> Result<DefId, PlanError> {
    let fn_trait = tcx
        .lang_items()
        .fn_trait()
        .ok_or_else(|| PlanError::UnknownCallee("Fn".into()))?;
    tcx.associated_item_def_ids(fn_trait)
        .iter()
        .copied()
        .find(|def| tcx.item_name(*def).as_str() == "call")
        .ok_or_else(|| PlanError::UnknownCallee("Fn::call".into()))
}

/// Resolve FnOnce::call_once through its language item for a moved owned callable receiver.
pub fn fn_once_call(tcx: TyCtxt<'_>) -> Result<DefId, PlanError> {
    let trait_id = tcx
        .lang_items()
        .fn_once_trait()
        .ok_or_else(|| PlanError::UnknownCallee("FnOnce".into()))?;
    tcx.associated_item_def_ids(trait_id)
        .iter()
        .copied()
        .find(|def| tcx.item_name(*def).as_str() == "call_once")
        .ok_or_else(|| PlanError::UnknownCallee("FnOnce::call_once".into()))
}

/// Resolve only a source-local function item admitted by the plan's canonical frontend identity.
pub fn planned(tcx: TyCtxt<'_>, name: &str) -> Result<DefId, PlanError> {
    tcx.hir_crate_items(())
        .free_items()
        .map(|item| item.owner_id.to_def_id())
        .find(|def| {
            tcx.def_kind(*def) == DefKind::Fn && tcx.opt_item_name(*def).is_some_and(|symbol| symbol.as_str() == name)
        })
        .ok_or_else(|| PlanError::UnknownCallee(name.into()))
}

/// Resolve a compiled layout through public module children. Its exported item may be an alias of the native ADT;
/// only rustc's type metadata can identify the struct or enum behind that alias.
///
/// Each segment selects the public child of that name and the kind it must be: a module on the way, the declared
/// kind at the end. A class with field defaults compiles a constructor function under its own name beside its struct,
/// one in the value namespace and one in the type namespace, so a name alone does not select the layout.
pub fn library_item(tcx: TyCtxt<'_>, path: &str, kind: DefKind) -> Result<DefId, PlanError> {
    let reason = match kind {
        DefKind::Enum => "Loaf layout names no public enum",
        DefKind::Const { .. } => "Rust value names no public constant",
        _ => "Loaf layout names no public struct",
    };
    public_item(tcx, path, &[kind], reason)
}

/// Resolve an opaque carrier's public type item without granting field or variant projections. An alias keeps its
/// own metadata parameters; field-bearing queries use [`library_item`] to select the underlying ADT. A trait principal
/// requires the native object constructor rather than a nominal `type_of` query and grants no field projection.
pub fn native_type_item(tcx: TyCtxt<'_>, path: &str) -> Result<DefId, PlanError> {
    public_item(
        tcx,
        path,
        &[DefKind::Struct, DefKind::Enum, DefKind::TyAlias, DefKind::Trait],
        "Loaf layout names no public type",
    )
}

/// Walk one public module path, selecting a final item only from the caller's admitted namespace kinds.
fn public_item(tcx: TyCtxt<'_>, path: &str, kinds: &[DefKind], reason: &str) -> Result<DefId, PlanError> {
    let unknown = || PlanError::Invalid {
        function: path.into(),
        reason: reason.into(),
    };
    let mut segments = path.split("::").peekable();
    let root = segments.next().ok_or_else(unknown)?;
    let mut current = crate::native_crates::root(tcx, root).ok_or_else(unknown)?;
    while let Some(segment) = segments.next() {
        if tcx.def_kind(current) != DefKind::Mod {
            return Err(unknown());
        }
        let wanted = if segments.peek().is_some() {
            &[DefKind::Mod]
        } else {
            kinds
        };
        current = public_layout_child(tcx, current, segment, wanted).ok_or_else(unknown)?;
    }
    if !kinds.contains(&tcx.def_kind(current)) {
        return Err(unknown());
    }
    Ok(current)
}

/// Resolve a public module item in its owning query domain. Local injected ADTs are HIR items; the external-only
/// `module_children` query must never receive the current crate's definitions.
fn public_layout_child(tcx: TyCtxt<'_>, module: DefId, name: &str, kinds: &[DefKind]) -> Option<DefId> {
    if module.is_local() {
        return tcx
            .hir_crate_items(())
            .free_items()
            .map(|item| item.owner_id.to_def_id())
            .filter(|def| {
                tcx.parent(*def) == module
                    && tcx.opt_item_name(*def).is_some_and(|symbol| symbol.as_str() == name)
                    && tcx.visibility(*def).is_public()
            })
            .find_map(|def| admitted_layout_definition(tcx, def, kinds));
    }
    tcx.module_children(module)
        .iter()
        .filter(|child| child.ident.name.as_str() == name && child.vis.is_public())
        .filter_map(|child| child.res.opt_def_id())
        .find_map(|def| admitted_layout_definition(tcx, def, kinds))
}

/// Preserve an admitted item's own parameters before considering an alias's underlying ADT for field access.
fn admitted_layout_definition(tcx: TyCtxt<'_>, definition: DefId, kinds: &[DefKind]) -> Option<DefId> {
    if kinds.contains(&tcx.def_kind(definition)) {
        return Some(definition);
    }
    kinds.iter().find_map(|kind| layout_definition(tcx, definition, *kind))
}

/// Read a public layout item's exact ADT definition, following a type alias through metadata without interpreting
/// the export's spelling. A module segment and a nonalias ADT must retain their declared kind.
fn layout_definition(tcx: TyCtxt<'_>, definition: DefId, expected: DefKind) -> Option<DefId> {
    if tcx.def_kind(definition) == expected {
        return Some(definition);
    }
    if tcx.def_kind(definition) != DefKind::TyAlias || !matches!(expected, DefKind::Struct | DefKind::Enum) {
        return None;
    }
    let rustc_middle::ty::Adt(adt, _) = tcx
        .type_of(definition)
        .instantiate_identity()
        .skip_normalization()
        .kind()
    else {
        return None;
    };
    (tcx.def_kind(adt.did()) == expected).then_some(adt.did())
}

/// Walk only public module children, rejecting nonfunction callees before constructing MIR.
///
/// A segment inside a module selects by kind as well as name, as [`library_item`] does: a class with field defaults
/// compiles a constructor function under its own name beside its struct, so `Class::method` must step through the
/// struct while a call of `Class` itself selects the function. A trait path selects its declared associated function;
/// concrete Self arguments and bounds remain subject to native signature validation.
/// A structural slice owner selects one public builtin impl item through the same rustc index used by extraction.
pub fn external(tcx: TyCtxt<'_>, path: &str) -> Result<DefId, PlanError> {
    // A public primitive alias names the primitive itself; scalar buckets resolve first, text and slices below.
    let primitive = path
        .strip_prefix("std::primitive::")
        .or_else(|| path.strip_prefix("core::primitive::"));
    if let Some(definition) = primitive.and_then(|primitive| primitive_path(tcx, primitive)) {
        return Ok(definition);
    }
    let path = primitive.unwrap_or(path);
    if let Some((owner, method)) = path.split_once("::") {
        let native = match owner {
            "f32" => Some(tcx.types.f32),
            "f64" => Some(tcx.types.f64),
            "u8" => Some(tcx.types.u8),
            _ => None,
        };
        if let Some(native) = native {
            return inherent_function_in(tcx, primitive_inherent_impls(tcx, native), method)
                .ok_or_else(|| PlanError::UnknownCallee(path.into()));
        }
    }
    if let Some(method) = path.strip_prefix("[]::") {
        return primitive_function(tcx, SimplifiedType::Slice, method)
            .ok_or_else(|| PlanError::UnknownCallee(path.into()));
    }
    if let Some(method) = path.strip_prefix("str::") {
        return primitive_function(tcx, SimplifiedType::Str, method)
            .ok_or_else(|| PlanError::UnknownCallee(path.into()));
    }
    let mut segments = path.split("::").peekable();
    let root = segments.next().ok_or_else(|| PlanError::UnknownCallee(path.into()))?;
    let mut current = crate::native_crates::root(tcx, root).ok_or_else(|| PlanError::UnknownCallee(path.into()))?;
    while let Some(segment) = segments.next() {
        let last = segments.peek().is_none();
        current = match tcx.def_kind(current) {
            DefKind::Mod => tcx
                .module_children(current)
                .iter()
                .filter(|child| child.ident.name.as_str() == segment && child.vis.is_public())
                .filter_map(|child| child.res.opt_def_id())
                .find(|child| callee_path_step(tcx.def_kind(*child), last))
                .ok_or_else(|| PlanError::UnknownCallee(path.into()))?,
            DefKind::Struct | DefKind::Enum => inherent_function(tcx, current, segment)
                .or_else(|| enum_constructor(tcx, current, segment))
                .ok_or_else(|| PlanError::UnknownCallee(path.into()))?,
            DefKind::Trait => trait_function(tcx, current, segment).ok_or_else(|| PlanError::Unsupported {
                function: path.into(),
                reason: "the SDK trait publishes no method with this name".into(),
            })?,
            _ => return Err(PlanError::UnknownCallee(path.into())),
        };
    }
    if !matches!(
        tcx.def_kind(current),
        DefKind::Fn | DefKind::AssocFn | DefKind::Ctor(_, rustc_hir::def::CtorKind::Fn)
    ) {
        return Err(PlanError::UnknownCallee(path.into()));
    }
    Ok(current)
}

/// Resolve a primitive's public inherent function from rustc's own impl bucket and canonical primitive identity.
/// Method spelling alone grants no signature; the ordinary native signature and predicate checks still apply.
fn primitive_path(tcx: TyCtxt<'_>, path: &str) -> Option<DefId> {
    let (owner, method) = path.split_once("::")?;
    let (_, native) = crate::rust_facts::primitive_types(tcx)
        .into_iter()
        .find(|(name, _)| *name == owner)?;
    let bucket = match native.kind() {
        rustc_middle::ty::Bool => SimplifiedType::Bool,
        rustc_middle::ty::Char => SimplifiedType::Char,
        rustc_middle::ty::Int(kind) => SimplifiedType::Int(*kind),
        rustc_middle::ty::Uint(kind) => SimplifiedType::Uint(*kind),
        rustc_middle::ty::Float(kind) => SimplifiedType::Float(*kind),
        _ => return None,
    };
    primitive_function(tcx, bucket, method)
}

/// Select the exact public constructor definition of a tuple or unit enum variant. The caller distinguishes
/// function and constant constructors; named-field and non-exhaustive private constructors remain ineligible.
pub(crate) fn enum_constructor(tcx: TyCtxt<'_>, owner: DefId, name: &str) -> Option<DefId> {
    if tcx.def_kind(owner) != DefKind::Enum {
        return None;
    }
    let variant = tcx
        .adt_def(owner)
        .variants()
        .iter()
        .find(|variant| variant.name.as_str() == name)?;
    if variant.is_field_list_non_exhaustive() {
        return None;
    }
    variant
        .ctor
        .map(|(_, constructor)| constructor)
        .filter(|constructor| tcx.visibility(*constructor).is_public())
}

/// Whether a module child of this kind can be one step of a callee path: a module or a method's owner type on the way,
/// a function at the end.
fn callee_path_step(kind: DefKind, last: bool) -> bool {
    if last {
        kind == DefKind::Fn
    } else {
        matches!(kind, DefKind::Mod | DefKind::Struct | DefKind::Enum | DefKind::Trait)
    }
}

/// Select one associated function declared by the exact trait. The trait's public path supplies visibility; the
/// caller separately validates its concrete Self argument and every native bound before constructing MIR.
fn trait_function(tcx: TyCtxt<'_>, owner: DefId, name: &str) -> Option<DefId> {
    let mut found = tcx
        .associated_item_def_ids(owner)
        .iter()
        .copied()
        .filter(|item| tcx.def_kind(*item) == DefKind::AssocFn && tcx.item_name(*item).as_str() == name);
    let selected = found.next()?;
    found.next().is_none().then_some(selected)
}

/// Select one public inherent associated function from the exact nominal owner. The caller instantiates the owner's
/// type parameters separately before validating the signature.
fn inherent_function(tcx: TyCtxt<'_>, owner: DefId, name: &str) -> Option<DefId> {
    inherent_function_in(tcx, &owner_inherent_impls(tcx, owner), name)
}

/// Every inherent impl rustc's method lookup reaches for a nominal owner: the impls in its defining crate and the
/// incoherent ones another crate attaches to it. `std::io::Error` is defined below `std`, which supplies its
/// OS-backed constructors (`new`, `other`, `last_os_error`) as incoherent impls.
pub fn owner_inherent_impls(tcx: TyCtxt<'_>, owner: DefId) -> Vec<DefId> {
    let mut implementations = tcx.inherent_impls(owner).to_vec();
    implementations.extend_from_slice(tcx.incoherent_impls(SimplifiedType::Adt(owner)));
    implementations
}

/// Read the exact inherent impl bucket for a primitive carrier, shared by metadata extraction and executable callee
/// selection.
pub(crate) fn primitive_inherent_impls<'tcx>(tcx: TyCtxt<'tcx>, native: rustc_middle::ty::Ty<'tcx>) -> &'tcx [DefId] {
    let Some(bucket) = rustc_middle::ty::fast_reject::simplify_type(
        tcx,
        native,
        rustc_middle::ty::fast_reject::TreatParams::InstantiateWithInfer,
    ) else {
        return &[];
    };
    tcx.incoherent_impls(bucket)
}

/// Resolve a retained native slice or `str` method through the same primitive impl bucket rustc uses for method
/// lookup. Only one public inherent declaration may select the callee; native arguments and predicates are verified
/// later.
fn primitive_function(tcx: TyCtxt<'_>, bucket: SimplifiedType, name: &str) -> Option<DefId> {
    inherent_function_in(tcx, tcx.incoherent_impls(bucket), name)
}

/// Select one public inherent function from exact impl owners, refusing ambiguous same-named declarations.
fn inherent_function_in(tcx: TyCtxt<'_>, implementations: &[DefId], name: &str) -> Option<DefId> {
    let mut found = implementations.iter().flat_map(|implementation| {
        tcx.associated_item_def_ids(*implementation)
            .iter()
            .copied()
            .filter(|item| {
                tcx.def_kind(*item) == DefKind::AssocFn
                    && tcx.item_name(*item).as_str() == name
                    && tcx.visibility(*item).is_public()
            })
    });
    let selected = found.next()?;
    found.next().is_none().then_some(selected)
}

/// Bind a generic external function's type parameters from the plan's declared signature, as rustc's inference does
/// for legacy's forwarding call. Every type parameter must be one whole parameter or the return type.
pub fn inferred_arguments(
    tcx: TyCtxt<'_>,
    def: DefId,
    external: &crate::plan::ExternalFunction,
) -> Result<Vec<crate::plan::PlanType>, PlanError> {
    let invalid = |reason: &str| PlanError::Invalid {
        function: external.path.clone(),
        reason: reason.into(),
    };
    let signature = tcx
        .fn_sig(def)
        .instantiate_identity()
        .skip_normalization()
        .skip_binder();
    if signature.inputs().len() != external.parameters.len() {
        return Err(invalid("external parameter count differs from metadata"));
    }
    let mut bound: Vec<Option<crate::plan::PlanType>> = vec![None; tcx.generics_of(def).count()];
    let declared = external.parameters.iter().chain(std::iter::once(&external.return_type));
    let actual = signature
        .inputs()
        .iter()
        .copied()
        .chain(std::iter::once(signature.output()));
    for (ty, planned) in actual.zip(declared) {
        if let rustc_middle::ty::Param(parameter) = ty.kind() {
            let slot = usize::try_from(parameter.index)
                .ok()
                .and_then(|index| bound.get_mut(index))
                .ok_or_else(|| invalid("external generic parameter is outside its declaration"))?;
            if slot.as_ref().is_some_and(|previous| previous != planned) {
                return Err(invalid("external generic parameter binds two plan types"));
            }
            *slot = Some(planned.clone());
        }
    }
    bound
        .into_iter()
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| invalid("external generic parameter is not a whole parameter or return type"))
}

/// Instantiate explicit plan arguments after checking their count against the resolved metadata declaration.
///
/// Trailing parameters the plan omits take the item's own defaults (`HashMap`'s `S = RandomState`, `Arc`'s
/// `A = Global`), instantiated over the arguments before them, as rustc does for a written path.
/// Lifetime parameters receive erased regions and never consume a retained type argument; const parameters refuse.
pub fn arguments<'tcx>(
    tcx: TyCtxt<'tcx>,
    def: DefId,
    types: &[crate::plan::PlanType],
) -> Result<rustc_middle::ty::GenericArgsRef<'tcx>, PlanError> {
    let generics = tcx.generics_of(def);
    if !arity_admits(tcx, def, types.len()) {
        return Err(PlanError::Invalid {
            function: tcx.def_path_str(def),
            reason: "generic argument count differs from metadata".into(),
        });
    }
    let mut types = types.iter();
    let mut arguments = Vec::new();
    for index in 0..generics.count() {
        let parameter = generics.param_at(index, tcx);
        let argument = match parameter.kind {
            rustc_middle::ty::GenericParamDefKind::Lifetime => tcx.lifetimes.re_erased.into(),
            rustc_middle::ty::GenericParamDefKind::Type { .. } => match types.next() {
                Some(ty) => crate::types::native_type(tcx, ty)?.into(),
                None => parameter
                    .default_value(tcx)
                    .map(|default| default.instantiate(tcx, tcx.mk_args(&arguments)).skip_normalization())
                    .or_else(|| owner_parameter_default(tcx, def, index, &arguments))
                    .ok_or_else(|| PlanError::Invalid {
                        function: tcx.def_path_str(def),
                        reason: "omitted generic argument has no default".into(),
                    })?,
            },
            rustc_middle::ty::GenericParamDefKind::Const { .. } => {
                return Err(PlanError::Unsupported {
                    function: tcx.def_path_str(def),
                    reason: "native const arguments require a retained value contract".into(),
                });
            }
        };
        arguments.push(argument);
    }
    Ok(tcx.mk_args(&arguments))
}

/// Recover an inherent impl parameter's default from its exact receiver ADT, such as Vec's allocator.
/// Impl generics do not repeat the ADT's defaults. Map the impl parameter through Self's actual arguments instead
/// of guessing an allocator by name; earlier explicit bindings instantiate any default referring to them.
fn owner_parameter_default<'tcx>(
    tcx: TyCtxt<'tcx>,
    def: DefId,
    index: usize,
    supplied: &[rustc_middle::ty::GenericArg<'tcx>],
) -> Option<rustc_middle::ty::GenericArg<'tcx>> {
    let implementation = tcx.impl_of_assoc(def)?;
    let mut arguments = rustc_middle::ty::GenericArgs::identity_for_item(tcx, implementation).to_vec();
    for (slot, argument) in arguments.iter_mut().zip(supplied) {
        *slot = *argument;
    }
    let owner = tcx
        .type_of(implementation)
        .instantiate(tcx, tcx.mk_args(&arguments))
        .skip_normalization();
    let rustc_middle::ty::Adt(owner, owner_arguments) = owner.kind() else {
        return None;
    };
    let position = owner_arguments.iter().position(|argument| {
        argument.as_type().is_some_and(|ty| {
            matches!(ty.kind(), rustc_middle::ty::Param(parameter)
            if usize::try_from(parameter.index).ok() == Some(index))
        })
    })?;
    tcx.generics_of(owner.did())
        .param_at(position, tcx)
        .default_value(tcx)
        .map(|default| default.instantiate(tcx, owner_arguments).skip_normalization())
}

/// Whether `written` type arguments instantiate `def`: lifetimes consume no slot, omitted types need defaults, and
/// const arguments require a separate value contract.
pub fn arity_admits(tcx: TyCtxt<'_>, def: DefId, written: usize) -> bool {
    let generics = tcx.generics_of(def);
    let mut supplied = 0;
    for index in 0..generics.count() {
        let parameter = generics.param_at(index, tcx);
        match parameter.kind {
            rustc_middle::ty::GenericParamDefKind::Lifetime => continue,
            rustc_middle::ty::GenericParamDefKind::Const { .. } => return false,
            rustc_middle::ty::GenericParamDefKind::Type { .. } if supplied < written => supplied += 1,
            rustc_middle::ty::GenericParamDefKind::Type { .. }
                if parameter.default_value(tcx).is_some()
                    || owner_parameter_default(tcx, def, index, &[]).is_some() => {}
            _ => return false,
        }
    }
    supplied == written
}

/// Refuse an instantiation whose type arguments do not satisfy the callee's where-clauses, before any MIR exists.
///
/// The borrow checker proves a callee's bounds again and reports a failure as an internal compiler error rather than
/// a diagnostic, so an unsatisfied bound must stop here. The checker already proved the call's bounds for its source
/// types, so a failure means the direct route emits no native impl for one of them (a newtype's `TryFrom[str]`, for
/// one): an unsupported route gap rather than an invalid plan. Normalize concrete associated outputs before testing
/// each obligation, so equivalent `IntoIterator::Item` and `Iterator::Item` projections satisfy equality bounds.
pub fn verify_bounds<'tcx>(
    tcx: TyCtxt<'tcx>,
    def: DefId,
    args: rustc_middle::ty::GenericArgsRef<'tcx>,
) -> Result<(), PlanError> {
    use rustc_infer::infer::TyCtxtInferExt;
    use rustc_trait_selection::traits::query::evaluate_obligation::InferCtxtExt;
    let (infcx, param_env) = tcx
        .infer_ctxt()
        .build_with_typing_env(rustc_middle::ty::TypingEnv::fully_monomorphized());
    for (clause, _) in tcx.predicates_of(def).instantiate(tcx, args) {
        let predicate = tcx
            .try_normalize_erasing_regions(rustc_middle::ty::TypingEnv::fully_monomorphized(), clause)
            .map_err(|error| PlanError::Unsupported {
                function: tcx.def_path_str(def),
                reason: format!("callee bound has no concrete native projection; arguments {args:?}; {error:?}"),
            })?;
        let obligation = rustc_infer::traits::Obligation::new(
            tcx,
            rustc_middle::traits::ObligationCause::dummy(),
            param_env,
            predicate,
        );
        if !infcx.predicate_must_hold_modulo_regions(&obligation) {
            return Err(PlanError::Unsupported {
                function: tcx.def_path_str(def),
                reason: format!(
                    "a type argument has no native impl for the callee's bounds; arguments {args:?}; predicate {clause:?}"
                ),
            });
        }
    }
    Ok(())
}

/// Check opaque results before revealing layouts, including the concrete implementation behind a trait call.
///
/// A trait's RPITIT projection can normalize to a concrete carrier even though its selected implementation returns
/// an opaque type. That carrier cannot be used as the MIR call destination outside the defining scope. Bounds must
/// already be verified before resolving the implementation; ordinary associated outputs retain their existing
/// normalization path.
pub fn verify_return_abi<'tcx>(
    tcx: TyCtxt<'tcx>,
    def: DefId,
    args: rustc_middle::ty::GenericArgsRef<'tcx>,
    planned: &crate::plan::PlanType,
    path: &str,
) -> Result<(), PlanError> {
    let unsupported = |reason: &str| PlanError::Unsupported {
        function: path.into(),
        reason: reason.into(),
    };
    let expected = crate::types::native_type(tcx, planned)?;
    let mut candidates = vec![(def, args)];
    let instance =
        rustc_middle::ty::Instance::try_resolve(tcx, rustc_middle::ty::TypingEnv::fully_monomorphized(), def, args)
            .map_err(|_| unsupported("callee native instance could not be resolved"))?;
    if let Some(instance) = instance
        && let rustc_middle::ty::InstanceKind::Item(selected) = instance.def
    {
        candidates.push((selected, instance.args));
    }
    for (selected, arguments) in candidates {
        let signature = tcx.instantiate_bound_regions_with_erased(
            tcx.fn_sig(selected).instantiate(tcx, arguments).skip_normalization(),
        );
        if matches!(signature.output().kind(), rustc_middle::ty::Alias(_, alias) if alias.is_opaque())
            && signature.output() != expected
        {
            return Err(unsupported(
                "opaque SDK trait return ABI differs from its concrete carrier",
            ));
        }
    }
    Ok(())
}
