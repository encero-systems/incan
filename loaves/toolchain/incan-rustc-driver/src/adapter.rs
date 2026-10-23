//! The only rustc-private boundary: Incan declarations enter as AST items and bodies enter as MIR.

use crate::error::PlanError;
use crate::identity::DriverIdentity;
use crate::plan::{
    CalleeKind, Constant, Operand, OperandKind, Plan, PlanType, RvalueKind, StatementKind, TerminatorKind,
};
use crate::spans::Sources;
use crate::{bodies, callees, declarations, types, union_debug, validation, value_enum_serde};
use incan_driver::unit::UnitKind;
use rustc_ast as ast;
use rustc_data_structures::steal::Steal;
use rustc_hir::def_id::LocalDefId;
use rustc_interface::interface;
use rustc_middle::mir::Body;
use rustc_middle::ty::{Ty, TyCtxt};
use rustc_session::config::Input;
use rustc_span::FileName;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Compilation is serialized until stage 3d supplies a resident driver with request isolation.
static COMPILATION: Mutex<()> = Mutex::new(());
/// Query providers are function pointers, so the invocation's source-owned plan has an explicit scoped slot.
static PLAN: Mutex<Option<Arc<Plan>>> = Mutex::new(None);
/// Typed errors raised while loading metadata or source locations survive rustc's fatal-error boundary.
static FAILURE: Mutex<Option<PlanError>> = Mutex::new(None);

/// Preserve a typed plan refusal when an admitted external signature cannot name its native representation.
fn native_type_or_refuse<'tcx>(tcx: TyCtxt<'tcx>, ty: &PlanType) -> Ty<'tcx> {
    match types::native_type(tcx, ty) {
        Ok(ty) => ty,
        Err(error) => refuse(tcx, error),
    }
}

/// Typed refusals and native compiler failures remain distinct at the executable boundary.
#[derive(Debug, thiserror::Error)]
pub enum DriverError {
    /// The scalar plan violates its preflight contract.
    #[error(transparent)]
    Plan(#[from] PlanError),
    /// rustc refused the compilation after valid preflight input.
    #[error("native compilation failed")]
    Compilation,
    /// A previous failed invocation left synchronization state unusable.
    #[error("driver state is unavailable")]
    State,
    /// Crate or dependency arguments violate the native invocation contract.
    #[error("invalid native invocation: {0}")]
    Invocation(String),
}

/// Read the active plan without retaining the slot lock while rustc resolves callees.
fn active_plan() -> Result<Arc<Plan>, DriverError> {
    PLAN.lock()
        .map_err(|_| DriverError::State)?
        .as_ref()
        .map(Arc::clone)
        .ok_or(DriverError::State)
}

/// Retain a typed adapter error, then abort this rustc invocation through its diagnostic boundary.
fn refuse(tcx: TyCtxt<'_>, error: PlanError) -> ! {
    let message = error.to_string();
    if let Ok(mut failure) = FAILURE.lock() {
        *failure = Some(error);
    }
    tcx.dcx().fatal(message)
}

/// Supply native aliases' exact metadata types, independently of their placeholder AST declarations. Concrete
/// Option/Result aliases also use retained payload types, including erased native references that cannot be spelled
/// as a source-owned alias lifetime. Ordinary source aliases keep rustc's provider.
fn native_alias_type<'tcx>(tcx: TyCtxt<'tcx>, def: LocalDefId) -> rustc_middle::ty::EarlyBinder<'tcx, Ty<'tcx>> {
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
        .find(|value| !value.carrier.is_empty() && name.as_str() == value.name)
    {
        let path = if carrier.carrier == "Option" {
            "std::option::Option"
        } else {
            "std::result::Result"
        };
        let native = match callees::library_item(tcx, path, rustc_hir::def::DefKind::Enum) {
            Ok(native) => native,
            Err(error) => refuse(tcx, error),
        };
        let fields = carrier
            .variants
            .iter()
            .flat_map(|variant| variant.fields.iter().cloned())
            .collect::<Vec<_>>();
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

/// Override only source-named planned functions; all compiler-generated items retain rustc's provider.
fn mir_built<'tcx>(tcx: TyCtxt<'tcx>, def: LocalDefId) -> &'tcx Steal<Body<'tcx>> {
    let plan = match active_plan() {
        Ok(plan) => plan,
        Err(error) => tcx.dcx().fatal(error.to_string()),
    };
    let name = tcx.opt_item_name(def.to_def_id());
    let function = plan.functions.iter().find(|function| {
        tcx.def_kind(def) == rustc_hir::def::DefKind::Fn
            && tcx.parent(def.to_def_id()).is_crate_root()
            && name.is_some_and(|name| name.as_str() == function.name)
    });
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

/// Inject the prevalidated declarations after parsing an empty Incan crate root.
struct Callbacks<'manifest> {
    plan: Arc<Plan>,
    manifest: Option<&'manifest incan_frontend::library_manifest::LibraryManifest>,
}

impl rustc_driver::Callbacks for Callbacks<'_> {
    /// Install the source-free root and the MIR query override; cap lints that only see placeholder bodies.
    fn config(&mut self, config: &mut interface::Config) {
        config.input = Input::Str {
            name: FileName::Custom(self.plan.span.file.clone()),
            input: String::new(),
        };
        config.override_queries = Some(|_session, providers| {
            providers.queries.mir_built = mir_built;
            providers.queries.type_of = native_alias_type;
        });
    }

    /// Add function signatures and load the declared external crates, without parsing generated source.
    fn after_crate_root_parsing(
        &mut self,
        compiler: &interface::Compiler,
        krate: &mut ast::Crate,
    ) -> rustc_driver::Compilation {
        let sources = Sources::new(compiler.sess.source_map());
        let span = match sources.span(&self.plan.span) {
            Ok(span) => span,
            Err(error) => compiler.sess.dcx().fatal(error.to_string()),
        };
        match crate::cloneable_closures::items(&self.plan, span) {
            Ok(items) => krate.items.extend(items),
            Err(error) => compiler.sess.dcx().fatal(error.to_string()),
        }
        for output in crate::futures::outputs(&self.plan) {
            krate.items.push(crate::futures::alias(&output, krate.spans.inner_span));
            krate
                .items
                .push(crate::futures::borrowed_alias(&output, krate.spans.inner_span));
        }
        for value in &self.plan.enums {
            let span = match sources.span(&value.span) {
                Ok(span) => span,
                Err(error) => compiler.sess.dcx().fatal(error.to_string()),
            };
            krate
                .items
                .extend(enum_items(value, &compiler.sess.psess.attr_id_generator, span));
        }
        for model in &self.plan.generic_models {
            let span = match sources.span(&model.span) {
                Ok(span) => span,
                Err(error) => compiler.sess.dcx().fatal(error.to_string()),
            };
            krate.items.extend(crate::generic_layouts::items(
                model,
                &compiler.sess.psess.attr_id_generator,
                span,
            ));
        }
        for model in &self.plan.models {
            let span = match sources.span(&model.span) {
                Ok(span) => span,
                Err(error) => compiler.sess.dcx().fatal(error.to_string()),
            };
            krate.items.extend(declarations::model_items(
                model,
                &compiler.sess.psess.attr_id_generator,
                span,
            ));
            if !model.checked_deserialize.is_empty() {
                krate.items.push(crate::checked_json::forwarding_impl(model, span));
            }
            if !model.string_conversion_root.is_empty() {
                krate
                    .items
                    .push(crate::checked_json::source_forwarding_impl(model, span));
            }
        }
        for function in &self.plan.functions {
            let span = match sources.span(&function.span) {
                Ok(span) => span,
                Err(error) => {
                    let message = error.to_string();
                    if let Ok(mut failure) = FAILURE.lock() {
                        *failure = Some(error);
                    }
                    compiler.sess.dcx().fatal(message);
                }
            };
            let mut item = declarations::function(function, span);
            if !function.attributes.is_empty() {
                item.attrs.extend(crate::native_attributes::attributes(
                    &compiler.sess.psess.attr_id_generator,
                    function,
                    span,
                ));
                item.tokens = Some(crate::native_attributes::function_tokens(function, span));
            }
            if self.plan.tests.contains(&function.name) {
                item.attrs.push(crate::test_unit::attribute(
                    &compiler.sess.psess.attr_id_generator,
                    span,
                ));
            }
            krate.items.push(item);
        }
        krate
            .items
            .extend(crate::shared_callables::items(&self.plan, krate.spans.inner_span));
        match crate::field_metadata::items(&self.plan, krate.spans.inner_span) {
            Ok(items) => krate.items.extend(items),
            Err(error) => compiler.sess.dcx().fatal(error.to_string()),
        }
        match crate::comparison_bridges::items(&self.plan, krate.spans.inner_span) {
            Ok(items) => krate.items.extend(items),
            Err(error) => compiler.sess.dcx().fatal(error.to_string()),
        }
        match crate::fallible_bridges::items(&self.plan, krate.spans.inner_span) {
            Ok(items) => krate.items.extend(items),
            Err(error) => compiler.sess.dcx().fatal(error.to_string()),
        }
        match crate::fallible_bridges::ordinary_items(&self.plan, krate.spans.inner_span) {
            Ok(items) => krate.items.extend(items),
            Err(error) => compiler.sess.dcx().fatal(error.to_string()),
        }
        match crate::callable_bridges::items(&self.plan, krate.spans.inner_span) {
            Ok(items) => krate.items.extend(items),
            Err(error) => compiler.sess.dcx().fatal(error.to_string()),
        }
        // A library unit re-exports its public names at the crate root (see `library_exports`).
        match crate::library_exports::items(&self.plan, self.manifest, krate.spans.inner_span) {
            Ok(items) => krate.items.extend(items),
            Err(error) => compiler.sess.dcx().fatal(error.to_string()),
        }
        // Computed statics keep rustc's own MIR for their storage and accessors (see `statics`).
        for cell in &self.plan.statics {
            let span = match sources.span(&cell.span) {
                Ok(span) => span,
                Err(error) => compiler.sess.dcx().fatal(error.to_string()),
            };
            krate.items.extend(crate::statics::items(cell, span));
            match crate::statics::method_items(&self.plan, cell, span) {
                Ok(items) => krate.items.extend(items),
                Err(error) => compiler.sess.dcx().fatal(error.to_string()),
            }
        }
        let mut roots: BTreeSet<_> = self
            .plan
            .externals
            .iter()
            .filter_map(|external| crate::native_crates::callee_root(&external.path))
            .chain(
                self.plan
                    .models
                    .iter()
                    .map(|model| &model.library_item)
                    .chain(self.plan.enums.iter().map(|value| &value.library_item))
                    .filter_map(|item| item.split("::").next())
                    .filter(|root| !root.is_empty()),
            )
            .collect();
        roots.extend(constant_crate_roots(&self.plan));
        if self.plan.functions.iter().any(|function| {
            function.locals.iter().any(|local| {
                matches!(
                    local.ty,
                    crate::plan::PlanType::Generator(..)
                        | crate::plan::PlanType::GeneratorMutRef(..)
                        | crate::plan::PlanType::GeneratorYield(..)
                        | crate::plan::PlanType::GeneratorYieldRef(..)
                )
            })
        }) {
            roots.insert("incan_std_core");
        }
        for function in &self.plan.functions {
            if let Some(callee) = &function.future_callee {
                let path = match &callee.kind {
                    crate::plan::CalleeKind::External(path)
                    | crate::plan::CalleeKind::Instantiated(path, _)
                    | crate::plan::CalleeKind::FutureMethod(path, _) => Some(path),
                    _ => None,
                };
                if let Some(root) = path.and_then(|path| crate::native_crates::callee_root(path)) {
                    roots.insert(root);
                }
            }
        }
        // `std` is already declared by the prelude injection; a second `extern crate std;` would collide with it.
        for root in roots.into_iter().filter(|root| !matches!(*root, "std" | "crate")) {
            krate
                .items
                .push(declarations::external_crate(root, krate.spans.inner_span));
        }
        rustc_driver::Compilation::Continue
    }

    /// Verify external signatures and synthesized clone bounds against native metadata before building planned MIR.
    fn after_expansion(&mut self, _compiler: &interface::Compiler, tcx: TyCtxt<'_>) -> rustc_driver::Compilation {
        if let Err(error) = verify_library_argument_profiles(tcx, &self.plan) {
            refuse(tcx, error);
        }
        match instantiate_generic_externals(tcx, &self.plan) {
            Ok(Some(plan)) => {
                self.plan = Arc::new(plan);
                match PLAN.lock() {
                    Ok(mut slot) => *slot = Some(Arc::clone(&self.plan)),
                    Err(_) => tcx.dcx().fatal(DriverError::State.to_string()),
                }
            }
            Ok(None) => {}
            Err(error) => refuse(tcx, error),
        }
        for model in self.plan.models.iter().filter(|model| !model.library_item.is_empty()) {
            if let Err(error) = verify_library_layout(tcx, model) {
                refuse(tcx, error);
            }
        }
        for value in self.plan.enums.iter().filter(|value| !value.library_item.is_empty()) {
            if let Err(error) = verify_library_enum(tcx, value) {
                refuse(tcx, error);
            }
        }
        if let Err(error) = crate::cloneable_closures::verify_captures(tcx, &self.plan) {
            refuse(tcx, error);
        }
        if let Err(error) = crate::statics::verify_method_results(tcx, &self.plan) {
            refuse(tcx, error);
        }
        if let Err(error) = verify_planned_clone_bounds(tcx, &self.plan) {
            refuse(tcx, error);
        }
        for external in &self.plan.externals {
            let def = match callees::external(tcx, &external.path) {
                Ok(def) => def,
                Err(error) => refuse(tcx, error),
            };
            let args = match callees::arguments(tcx, def, &external.type_arguments) {
                Ok(args) => args,
                Err(error) => refuse(tcx, error),
            };
            if let Err(error) = callees::verify_bounds(tcx, def, args) {
                refuse(tcx, error);
            }
            if let Err(error) = callees::verify_return_abi(tcx, def, args, &external.return_type, &external.path) {
                refuse(tcx, error);
            }
            let signature = tcx
                .fn_sig(def)
                .instantiate(tcx, args)
                .map(|signature| tcx.instantiate_bound_regions_with_erased(signature));
            let raw_signature = signature.skip_normalization();
            // A hidden concrete factory layout is not the SDK method's opaque return ABI. Borrow checking still
            // requires the opaque type, so revealing its layout here would admit a broken MIR call destination.
            if has_opaque_return_abi(tcx, raw_signature.output())
                && raw_signature.output() != native_type_or_refuse(tcx, &external.return_type)
            {
                refuse(
                    tcx,
                    PlanError::Unsupported {
                        function: external.path.clone(),
                        reason: "opaque SDK trait return ABI differs from its concrete carrier".into(),
                    },
                );
            }
            // Ordinary associated outputs still normalize under the checked concrete Self implementation.
            let signature =
                tcx.normalize_erasing_regions(rustc_middle::ty::TypingEnv::fully_monomorphized(), signature);
            let parameters: Vec<_> = external
                .parameters
                .iter()
                .map(|ty| tcx.erase_and_anonymize_regions(native_type_or_refuse(tcx, ty)))
                .collect();
            if signature.inputs() != parameters
                || signature.output()
                    != tcx.erase_and_anonymize_regions(native_type_or_refuse(tcx, &external.return_type))
                || signature.c_variadic()
                || signature.safety().is_unsafe()
            {
                refuse(
                    tcx,
                    PlanError::Invalid {
                        function: external.path.clone(),
                        reason: "external metadata differs from the admitted safe scalar signature".into(),
                    },
                );
            }
        }
        rustc_driver::Compilation::Continue
    }
}

/// Reject synthesized nominal clones without a native Clone implementation before rustc constructs their MIR.
fn verify_planned_clone_bounds(tcx: TyCtxt<'_>, plan: &Plan) -> Result<(), PlanError> {
    for block in plan.functions.iter().flat_map(|function| &function.blocks) {
        let TerminatorKind::Call(callee, ..) = &block.terminator.kind else {
            continue;
        };
        if matches!(callee.kind, CalleeKind::CloneEnum(..) | CalleeKind::CloneModel(..)) {
            let (definition, arguments) = crate::terminators::call_instance(tcx, callee)?;
            callees::verify_bounds(tcx, definition, arguments)?;
        }
    }
    Ok(())
}

/// Load the explicit crate aliases of retained native constants even when their value types name no native owner.
/// Body IR materializes each constant read into a Use assignment before other operations consume that temporary.
fn constant_crate_roots(plan: &Plan) -> impl Iterator<Item = &str> {
    plan.functions
        .iter()
        .flat_map(|function| &function.blocks)
        .flat_map(|block| &block.statements)
        .filter_map(|statement| {
            let StatementKind::Assign(_, value) = &statement.kind else {
                return None;
            };
            let RvalueKind::Use(Operand {
                kind: OperandKind::Literal(Constant::RustItem(path, _)),
                ..
            }) = &value.kind
            else {
                return None;
            };
            path.split("::").next()
        })
}

/// Identify opaque return ABIs before normalization can reveal their implementation layout. Ordinary associated
/// projections remain eligible for concrete normalization; an implicit return-position `impl Trait` associated type
/// keeps the opaque boundary its caller must satisfy before constructing MIR.
fn has_opaque_return_abi(tcx: TyCtxt<'_>, output: rustc_middle::ty::Ty<'_>) -> bool {
    match output.kind() {
        rustc_middle::ty::Alias(_, alias) => match alias.kind {
            rustc_middle::ty::Opaque { .. } => true,
            rustc_middle::ty::Projection { def_id } => tcx.is_impl_trait_in_trait(def_id),
            _ => false,
        },
        _ => false,
    }
}

/// Check a retained native layout against its crate's metadata at the plan's explicit type arguments, including their
/// bounds. Fields keep their names and instantiated types in order; a field marked public must be public in metadata.
/// An opaque carrier grants no projection; a field-bearing layout requires a native struct after alias instantiation.
fn verify_library_layout(tcx: TyCtxt<'_>, model: &crate::plan::ModelDeclaration) -> Result<(), PlanError> {
    let invalid = |reason: &str| PlanError::Unsupported {
        function: model.library_item.clone(),
        reason: reason.into(),
    };
    let def = callees::native_type_item(tcx, &model.library_item)?;
    if tcx.def_kind(def) == rustc_hir::def::DefKind::Trait {
        types::native_trait_object(tcx, def, &model.library_type_arguments)?;
        return if model.fields.is_empty() {
            Ok(())
        } else {
            Err(invalid("native trait object grants no field layout"))
        };
    }
    if !callees::arity_admits(tcx, def, model.library_type_arguments.len()) {
        return Err(invalid(
            "unsupported stdlib layout type argument count differs from metadata",
        ));
    }
    let arguments = callees::arguments(tcx, def, &model.library_type_arguments)?;
    callees::verify_bounds(tcx, def, arguments)?;
    if model.derives.iter().any(|derive| derive == "Copy") {
        let native = tcx.type_of(def).instantiate(tcx, arguments).skip_normalization();
        if !tcx.type_is_copy_modulo_regions(rustc_middle::ty::TypingEnv::fully_monomorphized(), native) {
            return Err(invalid("native Copy capability differs from metadata"));
        }
    }
    if model.fields.is_empty() {
        return Ok(());
    }
    let native = tcx.type_of(def).instantiate(tcx, arguments).skip_normalization();
    let rustc_middle::ty::Adt(adt, arguments) = native.kind() else {
        return Err(invalid("native field layout requires a struct"));
    };
    if !adt.is_struct() {
        return Err(invalid("native field layout requires a struct"));
    }
    let declared = &adt.non_enum_variant().fields;
    if declared.len() != model.fields.len()
        && crate::generic_layouts::marker_type(tcx, *adt, arguments, model.fields.len()).is_none()
    {
        return Err(invalid("stdlib layout field count differs from metadata"));
    }
    for (slot, (field, planned)) in declared.iter().zip(&model.fields).enumerate() {
        if model.field_public.get(slot) == Some(&true) && !field.vis.is_public() {
            return Err(invalid("public native field is private in metadata"));
        }
        if field.name.as_str() != planned.name
            || field.ty(tcx, arguments).skip_normalization() != crate::types::native_type(tcx, &planned.ty)?
        {
            return Err(invalid(&format!(
                "stdlib layout field `{}` at slot {slot} differs from metadata: native {}, planned {:?}",
                planned.name,
                field.ty(tcx, arguments).skip_normalization(),
                planned.ty,
            )));
        }
    }
    Ok(())
}

/// Check the concrete SDK enum's argument count, bounds, ordered variants and instantiated payload types against
/// native metadata. Variant indices select rustc's own constructors and discriminants, including explicit tags.
/// Generic templates never authorize storage.
fn verify_library_enum(tcx: TyCtxt<'_>, value: &crate::plan::EnumDeclaration) -> Result<(), PlanError> {
    let invalid = |reason: &str| PlanError::Invalid {
        function: value.library_item.clone(),
        reason: reason.into(),
    };
    let def = callees::library_item(tcx, &value.library_item, rustc_hir::def::DefKind::Enum)?;
    if !callees::arity_admits(tcx, def, value.library_type_arguments.len()) {
        return Err(invalid(
            "unsupported stdlib enum type argument count differs from metadata",
        ));
    }
    let owner = tcx.adt_def(def);
    let declared = owner.variants();
    if declared.len() != value.variants.len() {
        return Err(invalid("unsupported stdlib enum variant count differs from metadata"));
    }
    let arguments = callees::arguments(tcx, def, &value.library_type_arguments)?;
    callees::verify_bounds(tcx, def, arguments)?;
    if value.derives.iter().any(|derive| derive == "Clone") {
        let owner = rustc_middle::ty::Ty::new_adt(tcx, tcx.adt_def(def), arguments);
        callees::verify_bounds(tcx, callees::clone_method(tcx)?, tcx.mk_args(&[owner.into()]))?;
    }
    for (variant, planned) in declared.iter().zip(&value.variants) {
        if variant.name.as_str() != planned.name || variant.fields.len() != planned.fields.len() {
            return Err(invalid("unsupported stdlib enum variant differs from metadata"));
        }
        for (field, ty) in variant.fields.iter().zip(&planned.fields) {
            if field.ty(tcx, arguments).skip_normalization() != crate::types::native_type(tcx, ty)? {
                return Err(invalid("unsupported stdlib enum payload differs from metadata"));
            }
        }
    }
    Ok(())
}

/// One planned enum as it is injected: its declaration with derive attributes, then the impls legacy writes by hand.
///
/// A union prints the member it holds through its own `Debug`; a value enum's serde form is its raw values, so its
/// serde derives become impls beside it instead of attributes, while any other enum derives serde as legacy does.
fn enum_items(
    value: &crate::plan::EnumDeclaration,
    generator: &ast::attr::AttrIdGenerator,
    span: rustc_span::Span,
) -> Vec<Box<ast::Item>> {
    let value_enum = !value.raw_kind.is_empty();
    let mut item = declarations::enum_declaration(value, span);
    if value.carrier.is_empty() && value.library_item.is_empty() {
        for derive in value
            .derives
            .iter()
            .filter(|derive| !(value_enum && derive.starts_with("serde::")))
        {
            item.attrs.push(declarations::derive_attribute(generator, derive, span));
        }
    }
    let mut items = vec![item];
    if value.member_debug {
        items.push(union_debug::member_debug_impl(value, span));
    }
    if value_enum && value.derives.iter().any(|derive| derive == "serde::Serialize") {
        items.push(value_enum_serde::serialize_impl(value, span));
    }
    if value_enum && value.derives.iter().any(|derive| derive == "serde::Deserialize") {
        items.push(value_enum_serde::deserialize_impl(value, span));
    }
    items
}

/// Give each generic external declared without type arguments its inferred ones, and its calls the matching
/// instantiated callee, so signature verification and MIR construction see one explicit instantiation.
fn instantiate_generic_externals(tcx: TyCtxt<'_>, plan: &Plan) -> Result<Option<Plan>, PlanError> {
    let mut instantiated = plan.clone();
    let mut inferred = std::collections::BTreeMap::new();
    for external in &mut instantiated.externals {
        if !external.type_arguments.is_empty() {
            continue;
        }
        let def = callees::external(tcx, &external.path)?;
        if callees::arity_admits(tcx, def, 0) {
            continue;
        }
        external.type_arguments = callees::inferred_arguments(tcx, def, external)?;
        inferred.insert(external.path.clone(), external.type_arguments.clone());
    }
    if inferred.is_empty() {
        return Ok(None);
    }
    for block in instantiated
        .functions
        .iter_mut()
        .flat_map(|function| function.blocks.iter_mut())
    {
        let TerminatorKind::Call(callee, ..) = &mut block.terminator.kind else {
            continue;
        };
        let CalleeKind::External(path) = &callee.kind else {
            continue;
        };
        let Some(types) = inferred.get(path) else {
            continue;
        };
        callee.kind = CalleeKind::Instantiated(path.clone(), types.clone());
    }
    Ok(Some(instantiated))
}

/// Compile a plan supplied directly by an Incan caller function into a native binary.
///
/// The caller supplies the exact toolchain/library identity already verified before checking. External crates are
/// explicit rlib paths; their metadata resolves canonical callees. No generated Rust source or ambient unstable grant
/// is used.
/// `INCAN_DIRECT_VALIDATION_REPORT` optionally records each function's first preflight refusal before normal
/// validation. `INCAN_DIRECT_PLAN_REPORT` optionally records the complete plan for native ownership diagnostics.
/// A library manifest lends its already checked public facade names for aliases of the canonical native items.
pub fn compile(
    plan: Plan,
    crate_name: &str,
    output: &Path,
    identity: &DriverIdentity,
    externs: &[(String, PathBuf)],
    dependency_search_paths: &[PathBuf],
    rustc_args: &[String],
    kind: UnitKind,
    manifest: Option<&incan_frontend::library_manifest::LibraryManifest>,
) -> Result<(), DriverError> {
    let _invocation = tracing::trace_span!(target: "incan::invocation", "incan_rustc_driver::native_compile", input = %output.display(), functions = plan.functions.len()).entered();
    if let Some(path) = std::env::var_os("INCAN_DIRECT_PLAN_REPORT") {
        std::fs::write(path, format!("{plan:#?}"))
            .map_err(|error| DriverError::Invocation(format!("cannot write native plan report: {error}")))?;
    }
    if let Some(path) = std::env::var_os("INCAN_DIRECT_VALIDATION_REPORT") {
        let mut report = String::new();
        for refusal in validation::refusals(&plan, kind != UnitKind::Program) {
            report.push_str(&refusal.to_string().replace('\n', " "));
            report.push('\n');
        }
        std::fs::write(path, report)
            .map_err(|error| DriverError::Invocation(format!("cannot write native validation report: {error}")))?;
    }
    validation::validate(&plan, kind != UnitKind::Program)?;
    if crate_name.is_empty() || !crate_name.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_') {
        return Err(DriverError::Invocation("crate name must be a Rust identifier".into()));
    }
    let _compilation = COMPILATION.lock().map_err(|_| DriverError::State)?;
    let plan = Arc::new(plan);
    *PLAN.lock().map_err(|_| DriverError::State)? = Some(Arc::clone(&plan));
    *FAILURE.lock().map_err(|_| DriverError::State)? = None;
    let mut args = vec![
        "incan-rustc-driver".into(),
        "--sysroot".into(),
        identity.sysroot.to_string_lossy().into_owned(),
        "--crate-name".into(),
        crate_name.into(),
        "--edition".into(),
        "2024".into(),
        "--cap-lints".into(),
        "allow".into(),
        "-C".into(),
        "overflow-checks=yes".into(),
        "-C".into(),
        "opt-level=3".into(),
        "-C".into(),
        "debuginfo=2".into(),
        "-o".into(),
        output.to_string_lossy().into_owned(),
        // rustc reads stdin to the end for `-`; a named input is never read, since `config` installs the source-free
        // root.
        plan.span.file.clone(),
    ];
    // A library unit is an rlib its dependents link; an executable keeps rustc's default binary crate type.
    if kind == UnitKind::Library {
        args.push("--crate-type".into());
        args.push("lib".into());
    }
    if kind == UnitKind::Test {
        args.push("--test".into());
    }
    for (name, path) in externs {
        args.push("--extern".into());
        args.push(format!("{name}={}", path.display()));
        if let Some(parent) = path.parent() {
            args.push("-L".into());
            args.push(format!("dependency={}", parent.display()));
        }
    }
    for path in dependency_search_paths {
        args.push("-L".into());
        args.push(format!("dependency={}", path.display()));
    }
    args.extend_from_slice(rustc_args);
    let result =
        rustc_driver::catch_fatal_errors(|| rustc_driver::run_compiler(&args, &mut Callbacks { plan, manifest }));
    *PLAN.lock().map_err(|_| DriverError::State)? = None;
    if let Some(error) = FAILURE.lock().map_err(|_| DriverError::State)?.take() {
        return Err(DriverError::Plan(error));
    }
    result.map_err(|_| DriverError::Compilation)
}

/// Check every foreign alias's arity and parameter kinds before any native type query can expand an alias.
/// Bounds follow argument dependency order, so an invalid child cannot surface as a Rust alias diagnostic first.
fn verify_library_argument_profiles(tcx: TyCtxt<'_>, plan: &Plan) -> Result<(), PlanError> {
    for model in plan.models.iter().filter(|model| !model.library_item.is_empty()) {
        let def = callees::native_type_item(tcx, &model.library_item)?;
        if tcx.def_kind(def) == rustc_hir::def::DefKind::Trait {
            types::native_trait_object(tcx, def, &model.library_type_arguments)?;
            continue;
        }
        let generics = tcx.generics_of(def);
        let reason = if !callees::arity_admits(tcx, def, model.library_type_arguments.len()) {
            Some("stdlib layout type argument count differs from metadata")
        } else if generics.parent_count != 0
            || generics
                .own_params
                .iter()
                .any(|parameter| matches!(&parameter.kind, rustc_middle::ty::GenericParamDefKind::Const { .. }))
        {
            Some("native const arguments require a retained value contract")
        } else {
            None
        };
        if let Some(reason) = reason {
            return Err(PlanError::Unsupported {
                function: model.library_item.clone(),
                reason: reason.into(),
            });
        }
    }
    for index in validation::library_model_order(plan)? {
        let model = &plan.models[index];
        if model.library_item.is_empty() {
            continue;
        }
        let def = callees::native_type_item(tcx, &model.library_item)?;
        if tcx.def_kind(def) == rustc_hir::def::DefKind::Trait {
            continue;
        }
        let arguments = callees::arguments(tcx, def, &model.library_type_arguments)?;
        callees::verify_bounds(tcx, def, arguments)?;
    }
    Ok(())
}
