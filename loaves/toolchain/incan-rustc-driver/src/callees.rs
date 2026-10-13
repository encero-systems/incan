//! Canonical path resolution against rustc metadata, never generated function names.

use crate::error::PlanError;
use crate::plan::{Callee, CalleeKind};
use rustc_hir::def::DefKind;
use rustc_hir::def_id::DefId;
use rustc_middle::ty::TyCtxt;

/// Resolve a planned source name, canonical external path, or explicit runtime operation to its exact function.
/// Explicit plan arguments are checked separately.
pub fn resolve(tcx: TyCtxt<'_>, callee: &Callee) -> Result<DefId, PlanError> {
    match &callee.kind {
        CalleeKind::Planned(name) => planned(tcx, name),
        CalleeKind::Value(_) => Err(PlanError::UnknownCallee(
            "local callable has no declaration callee".into(),
        )),
        CalleeKind::CallClosure(_) => fn_call(tcx),
        CalleeKind::ClosureBody(..) => Err(PlanError::UnknownCallee(
            "closure constructors keep rustc's own body".into(),
        )),
        CalleeKind::Instantiated(path, _) => external(tcx, path),
        CalleeKind::SpawnGenerator(_, leaf, depth) => generator_method(tcx, "Generator", "spawn", leaf, *depth),
        CalleeKind::YieldGenerator(leaf, depth) => generator_method(tcx, "GeneratorYield", "yield_value", leaf, *depth),
        CalleeKind::CollectGenerator(leaf, depth) => generator_method(tcx, "Generator", "collect", leaf, *depth),
        CalleeKind::External(path) => external(tcx, path),
        CalleeKind::CloneModel(_, _) | CalleeKind::CloneEnum(_, _) => clone_method(tcx),
        CalleeKind::StaticLoad(slot) => planned(tcx, &crate::statics::load_name(*slot)),
        CalleeKind::StaticStore(slot) => planned(tcx, &crate::statics::store_name(*slot)),
        CalleeKind::StaticForce(slot) => planned(tcx, &crate::statics::force_name(*slot)),
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

/// Resolve a stdlib layout's crate item through public module children; a model layout names a struct and an enum
/// layout names an enum, so `kind` is the item kind the plan declared.
///
/// Each segment selects the public child of that name and the kind it must be: a module on the way, the declared
/// kind at the end. A class with field defaults compiles a constructor function under its own name beside its struct,
/// one in the value namespace and one in the type namespace, so a name alone does not select the layout.
pub fn library_item(tcx: TyCtxt<'_>, path: &str, kind: DefKind) -> Result<DefId, PlanError> {
    let unknown = || PlanError::Invalid {
        function: path.into(),
        reason: match kind {
            DefKind::Enum => "stdlib layout names no public enum",
            _ => "stdlib layout names no public struct",
        }
        .into(),
    };
    let mut segments = path.split("::").peekable();
    let root = segments.next().ok_or_else(unknown)?;
    let krate = tcx
        .crates(())
        .iter()
        .find(|krate| tcx.crate_name(**krate).as_str() == root)
        .ok_or_else(unknown)?;
    let mut current = krate.as_def_id();
    while let Some(segment) = segments.next() {
        let wanted = if segments.peek().is_some() { DefKind::Mod } else { kind };
        current = tcx
            .module_children(current)
            .iter()
            .filter(|child| child.ident.name.as_str() == segment && child.vis.is_public())
            .filter_map(|child| child.res.opt_def_id())
            .find(|child| tcx.def_kind(*child) == wanted)
            .ok_or_else(unknown)?;
    }
    if tcx.def_kind(current) != kind {
        return Err(unknown());
    }
    Ok(current)
}

/// Walk only public module children, rejecting nonfunction callees before constructing MIR.
///
/// A segment inside a module selects by kind as well as name, as [`library_item`] does: a class with field defaults
/// compiles a constructor function under its own name beside its struct, so `Class::method` must step through the
/// struct while a call of `Class` itself selects the function. A trait path selects its declared associated function;
/// concrete Self arguments and bounds remain subject to native signature validation.
pub fn external(tcx: TyCtxt<'_>, path: &str) -> Result<DefId, PlanError> {
    let mut segments = path.split("::").peekable();
    let root = segments.next().ok_or_else(|| PlanError::UnknownCallee(path.into()))?;
    let krate = tcx
        .crates(())
        .iter()
        .find(|krate| tcx.crate_name(**krate).as_str() == root)
        .ok_or_else(|| PlanError::UnknownCallee(path.into()))?;
    let mut current = krate.as_def_id();
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
            DefKind::Struct | DefKind::Enum => {
                inherent_function(tcx, current, segment).ok_or_else(|| PlanError::UnknownCallee(path.into()))?
            }
            DefKind::Trait => trait_function(tcx, current, segment).ok_or_else(|| PlanError::Unsupported {
                function: path.into(),
                reason: "the SDK trait publishes no method with this name".into(),
            })?,
            _ => return Err(PlanError::UnknownCallee(path.into())),
        };
    }
    if !matches!(tcx.def_kind(current), DefKind::Fn | DefKind::AssocFn) {
        return Err(PlanError::UnknownCallee(path.into()));
    }
    Ok(current)
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
    let mut found = tcx.inherent_impls(owner).iter().flat_map(|implementation| {
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
pub fn arguments<'tcx>(
    tcx: TyCtxt<'tcx>,
    def: DefId,
    types: &[crate::plan::PlanType],
) -> Result<rustc_middle::ty::GenericArgsRef<'tcx>, PlanError> {
    if tcx.generics_of(def).count() != types.len() {
        return Err(PlanError::Invalid {
            function: tcx.def_path_str(def),
            reason: "generic argument count differs from metadata".into(),
        });
    }
    let arguments = types
        .iter()
        .map(|ty| crate::types::native_type(tcx, ty).map(Into::into))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(tcx.mk_args(&arguments))
}

/// Refuse an instantiation whose type arguments do not satisfy the callee's where-clauses, before any MIR exists.
///
/// The borrow checker proves a callee's bounds again and reports a failure as an internal compiler error rather than
/// a diagnostic, so an unsatisfied bound must stop here. The checker already proved the call's bounds for its source
/// types, so a failure means the direct route emits no native impl for one of them (a newtype's `TryFrom[str]`, for
/// one): an unsupported route gap rather than an invalid plan.
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
        let obligation = rustc_infer::traits::Obligation::new(
            tcx,
            rustc_middle::traits::ObligationCause::dummy(),
            param_env,
            clause.skip_normalization(),
        );
        if !infcx.predicate_must_hold_modulo_regions(&obligation) {
            return Err(PlanError::Unsupported {
                function: tcx.def_path_str(def),
                reason: "a type argument has no native impl for the callee's bounds".into(),
            });
        }
    }
    Ok(())
}
