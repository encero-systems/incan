//! Planned native types, the active plan, refusals, and the native alias, signature and MIR queries.

use super::*;

/// Preserve a typed plan refusal when an admitted external signature cannot name its native representation.
pub(super) fn native_type_or_refuse<'tcx>(tcx: TyCtxt<'tcx>, ty: &PlanType) -> Ty<'tcx> {
    match types::native_type(tcx, ty) {
        Ok(ty) => ty,
        Err(error) => refuse(tcx, error),
    }
}

/// Make the process-global compilation slots usable again after a contained panic: clear the poison the panic left
/// on them and drop the plan and failure of the compilation it ended.
pub(crate) fn recover() {
    COMPILATION.clear_poison();
    PLAN.clear_poison();
    FAILURE.clear_poison();
    if let Ok(mut plan) = PLAN.lock() {
        *plan = None;
    }
    if let Ok(mut failure) = FAILURE.lock() {
        *failure = None;
    }
}

/// Read the active plan without retaining the slot lock while rustc resolves callees.
pub(super) fn active_plan() -> Result<Arc<Plan>, DriverError> {
    PLAN.lock()
        .map_err(|_| DriverError::State)?
        .as_ref()
        .map(Arc::clone)
        .ok_or(DriverError::State)
}

/// Retain a typed adapter error, then abort this rustc invocation through its diagnostic boundary.
pub(super) fn refuse(tcx: TyCtxt<'_>, error: PlanError) -> ! {
    let message = error.to_string();
    if let Ok(mut failure) = FAILURE.lock() {
        *failure = Some(error);
    }
    tcx.dcx().fatal(message)
}

/// Supply native aliases' exact metadata types, independently of their placeholder AST declarations. Concrete
/// Option/Result aliases also use retained payload types, including erased native references that cannot be spelled
/// as a source-owned alias lifetime. Ordinary source aliases keep rustc's provider.
pub(super) fn native_alias_type<'tcx>(
    tcx: TyCtxt<'tcx>,
    def: LocalDefId,
) -> rustc_middle::ty::EarlyBinder<'tcx, Ty<'tcx>> {
    if tcx.def_kind(def) != rustc_hir::def::DefKind::TyAlias || !tcx.parent(def.to_def_id()).is_crate_root() {
        return (rustc_interface::DEFAULT_QUERY_PROVIDERS.queries.type_of)(tcx, def);
    }
    let plan = match active_plan() {
        Ok(plan) => plan,
        Err(error) => tcx.dcx().fatal(error.to_string()),
    };
    let name = tcx.item_name(def.to_def_id());
    if let Some(carrier) = plan
        .enums
        .iter()
        .find(|value| value.carrier != Carrier::Declared && name.as_str() == value.name)
    {
        let (segments, fields) = crate::declarations::standard_carrier(carrier);
        let path = format!("std::{}", segments.join("::"));
        let native = match callees::library_item(tcx, &path, rustc_hir::def::DefKind::Enum) {
            Ok(native) => native,
            Err(error) => refuse(tcx, error),
        };
        let arguments = match callees::arguments(tcx, native, &fields) {
            Ok(arguments) => arguments,
            Err(error) => refuse(tcx, error),
        };
        return rustc_middle::ty::EarlyBinder::bind(
            tcx,
            tcx.type_of(native).instantiate(tcx, arguments).skip_normalization(),
        );
    }
    let record = plan
        .models
        .iter()
        .filter(|model| !model.library_item.is_empty())
        .map(|model| {
            (
                &model.name,
                &model.library_item,
                &model.library_type_arguments,
                rustc_hir::def::DefKind::Struct,
            )
        })
        .chain(
            plan.enums
                .iter()
                .filter(|value| !value.library_item.is_empty())
                .map(|value| {
                    (
                        &value.name,
                        &value.library_item,
                        &value.library_type_arguments,
                        rustc_hir::def::DefKind::Enum,
                    )
                }),
        )
        .find(|(alias, _, _, _)| name.as_str() == alias.as_str());
    let Some((_, path, types, kind)) = record else {
        return (rustc_interface::DEFAULT_QUERY_PROVIDERS.queries.type_of)(tcx, def);
    };
    let selected = if kind == rustc_hir::def::DefKind::Struct {
        callees::native_type_item(tcx, path)
    } else {
        callees::library_item(tcx, path, kind)
    };
    let native = match selected {
        Ok(def) => def,
        Err(error) => refuse(tcx, error),
    };
    if tcx.def_kind(native) == rustc_hir::def::DefKind::Trait {
        let object = match types::native_trait_object(tcx, native, types) {
            Ok(object) => object,
            Err(error) => refuse(tcx, error),
        };
        return rustc_middle::ty::EarlyBinder::bind(tcx, object);
    }
    let arguments = match callees::arguments(tcx, native, types) {
        Ok(arguments) => arguments,
        Err(error) => refuse(tcx, error),
    };
    rustc_middle::ty::EarlyBinder::bind(
        tcx,
        tcx.type_of(native).instantiate(tcx, arguments).skip_normalization(),
    )
}

/// Retain source type-parameter loan relationships and independently bind other opaque native inputs before rustc
/// computes implied outlives bounds. Only injected source functions and native callback bridges use this provider;
/// external metadata stays with rustc's separate provider, and body-local native storage remains region-erased.
pub(super) fn native_function_signature<'tcx>(
    tcx: TyCtxt<'tcx>,
    def: LocalDefId,
) -> rustc_middle::ty::EarlyBinder<'tcx, rustc_middle::ty::PolyFnSig<'tcx>> {
    let plan = match active_plan() {
        Ok(plan) => plan,
        Err(error) => tcx.dcx().fatal(error.to_string()),
    };
    (rustc_interface::DEFAULT_QUERY_PROVIDERS.queries.fn_sig)(tcx, def).map_bound(|signature| {
        let sites = planned_function(tcx, def, &plan).map_or(&[][..], |function| function.generic_loans.as_slice());
        let retained = match crate::generic_loans::signature(tcx, signature, sites) {
            Ok(signature) => signature,
            Err(error) => refuse(tcx, error),
        };
        crate::native_loans::signature(tcx, retained)
    })
}

/// Select only the plan's source-named root functions; synthesized closures retain their separate contracts.
pub(super) fn planned_function<'plan>(
    tcx: TyCtxt<'_>,
    def: LocalDefId,
    plan: &'plan Plan,
) -> Option<&'plan crate::plan::Function> {
    let name = tcx.opt_item_name(def.to_def_id());
    plan.functions.iter().find(|function| {
        tcx.def_kind(def) == rustc_hir::def::DefKind::Fn
            && declared_at_root(tcx, def)
            && name.is_some_and(|name| name.as_str() == function.name)
    })
}

/// A planned callable is declared at the crate root or in one of the root's callable partition modules.
pub(super) fn declared_at_root(tcx: TyCtxt<'_>, def: LocalDefId) -> bool {
    let parent = tcx.parent(def.to_def_id());
    parent.is_crate_root()
        || (tcx.def_kind(parent) == rustc_hir::def::DefKind::Mod
            && tcx.parent(parent).is_crate_root()
            && tcx
                .opt_item_name(parent)
                .is_some_and(|name| name.as_str().starts_with(declarations::PARTITION_MODULE_PREFIX)))
}

/// Override only source-named planned functions; all compiler-generated items retain rustc's provider.
pub(super) fn mir_built<'tcx>(tcx: TyCtxt<'tcx>, def: LocalDefId) -> &'tcx Steal<Body<'tcx>> {
    let plan = match active_plan() {
        Ok(plan) => plan,
        Err(error) => tcx.dcx().fatal(error.to_string()),
    };
    let function = planned_function(tcx, def, &plan);
    match function {
        // A closure constructor keeps rustc's own body, built from its injected boxing expression.
        Some(function)
            if crate::closures::constructor(function).is_some()
                || crate::shared_callables::constructor(function).is_some()
                || function.is_async
                || function.future_callee.is_some() =>
        {
            (rustc_interface::DEFAULT_QUERY_PROVIDERS.queries.mir_built)(tcx, def)
        }
        Some(function) => match if crate::captured_generators::producer(function).is_some() {
            crate::captured_generators::constructor(tcx, def, function)
        } else {
            bodies::body(tcx, def, function)
        } {
            Ok(body) => tcx.alloc_steal_mir(body),
            Err(error) => refuse(tcx, error),
        },
        None => {
            let parent = tcx.parent(def.to_def_id());
            let coroutine = plan.functions.iter().find(|function| {
                function.is_async
                    && tcx.def_kind(def) == rustc_hir::def::DefKind::Closure
                    && tcx
                        .opt_item_name(parent)
                        .is_some_and(|name| name.as_str() == function.name)
            });
            if let Some(function) = coroutine {
                return match crate::coroutines::body(tcx, def, function) {
                    Ok(body) => tcx.alloc_steal_mir(body),
                    Err(error) => refuse(tcx, error),
                };
            }
            let constructor = plan.functions.iter().find(|function| {
                tcx.def_kind(def) == rustc_hir::def::DefKind::Closure
                    && tcx
                        .opt_item_name(parent)
                        .is_some_and(|name| name.as_str() == function.name)
                    && crate::captured_generators::producer(function).is_some()
            });
            let closure = plan.functions.iter().find(|function| {
                tcx.def_kind(def) == rustc_hir::def::DefKind::Closure
                    && tcx
                        .opt_item_name(parent)
                        .is_some_and(|name| name.as_str() == function.name)
                    && crate::closures::constructor(function).is_some()
            });
            match (constructor, closure) {
                (Some(function), _) => match crate::captured_generators::callback(tcx, def, function) {
                    Ok(body) => tcx.alloc_steal_mir(body),
                    Err(error) => refuse(tcx, error),
                },
                (None, Some(function)) => match crate::closures::callback(tcx, def, function) {
                    Ok(body) => tcx.alloc_steal_mir(body),
                    Err(error) => refuse(tcx, error),
                },
                (None, None) => (rustc_interface::DEFAULT_QUERY_PROVIDERS.queries.mir_built)(tcx, def),
            }
        }
    }
}
