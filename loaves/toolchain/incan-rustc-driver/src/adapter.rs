//! The only rustc-private boundary: Incan declarations enter as AST items and bodies enter as MIR.

use crate::error::PlanError;
use crate::identity::DriverIdentity;
use crate::native_crates::native_roots;
use crate::plan::{CalleeKind, Plan, PlanType, TerminatorKind};
use crate::spans::Sources;
use crate::{bodies, callees, declarations, types, union_debug, validation, value_enum_serde};
use incan_driver::unit::UnitKind;
use incan_lang::lang::derives::{self, DeriveId};
use rustc_ast as ast;
use rustc_data_structures::steal::Steal;
use rustc_hir::def_id::LocalDefId;
use rustc_interface::interface;
use rustc_middle::mir::Body;
use rustc_middle::ty::{Ty, TyCtxt};
use rustc_session::config::Input;
use rustc_span::FileName;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Compilation is serialized: `PLAN` and `FAILURE` are process-global slots, also in the resident worker.
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

/// Retain source type-parameter loan relationships and independently bind other opaque native inputs before rustc
/// computes implied outlives bounds. Only injected source functions and native callback bridges use this provider;
/// external metadata stays with rustc's separate provider, and body-local native storage remains region-erased.
fn native_function_signature<'tcx>(
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
fn planned_function<'plan>(
    tcx: TyCtxt<'_>,
    def: LocalDefId,
    plan: &'plan Plan,
) -> Option<&'plan crate::plan::Function> {
    let name = tcx.opt_item_name(def.to_def_id());
    plan.functions.iter().find(|function| {
        tcx.def_kind(def) == rustc_hir::def::DefKind::Fn
            && tcx.parent(def.to_def_id()).is_crate_root()
            && name.is_some_and(|name| name.as_str() == function.name)
    })
}

/// Override only source-named planned functions; all compiler-generated items retain rustc's provider.
fn mir_built<'tcx>(tcx: TyCtxt<'tcx>, def: LocalDefId) -> &'tcx Steal<Body<'tcx>> {
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

/// Inject the prevalidated declarations after parsing an empty Incan crate root.
struct Callbacks<'manifest> {
    plan: Arc<Plan>,
    manifest: Option<&'manifest incan_frontend::library_manifest::LibraryManifest>,
    search: Vec<rustc_session::search_paths::SearchPath>,
    shared: Vec<String>,
    development: bool,
    loaded: Vec<(PathBuf, String)>,
    timing: crate::native_timing::Timing,
}

impl Callbacks<'_> {
    /// Declare checked callables before their program entries, retaining typed function-span failures across fatal
    /// errors.
    fn declare_callables(&self, compiler: &interface::Compiler, krate: &mut ast::Crate, sources: &Sources<'_>) {
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
            let mut item = match declarations::function(function, span) {
                Ok(item) => item,
                Err(error) => compiler.sess.dcx().fatal(error.to_string()),
            };
            if !function.attributes.is_empty() {
                item.attrs.extend(crate::native_attributes::attributes(
                    &compiler.sess.psess.attr_id_generator,
                    function,
                    span,
                ));
                item.tokens = Some(crate::native_attributes::function_tokens(&self.plan, function, span));
            }
            if self.plan.tests.contains(&function.name) {
                item.attrs.push(crate::test_unit::attribute(
                    &compiler.sess.psess.attr_id_generator,
                    span,
                ));
            }
            krate.items.push(item);
        }
        if !self.plan.programs.is_empty() {
            krate.items.push(
                match crate::program_entries::dispatcher(&self.plan, krate.spans.inner_span) {
                    Ok(item) => item,
                    Err(error) => compiler.sess.dcx().fatal(error.to_string()),
                },
            );
        }
    }

    /// Declare compiler-owned bridges before publishing the library's public root names.
    fn declare_bridges_and_exports(&self, compiler: &interface::Compiler, krate: &mut ast::Crate) {
        match native_bridge_items(&self.plan, krate.spans.inner_span) {
            Ok(items) => krate.items.extend(items),
            Err(error) => compiler.sess.dcx().fatal(error.to_string()),
        }
        // A library unit re-exports its public names at the crate root (see `library_exports`).
        match crate::library_exports::items(&self.plan, self.manifest, krate.spans.inner_span) {
            Ok(items) => krate.items.extend(items),
            Err(error) => compiler.sess.dcx().fatal(error.to_string()),
        }
    }

    /// Declare static storage before gathering the native roots required by every declaration already emitted.
    fn declare_storage_and_externs(
        &self,
        compiler: &interface::Compiler,
        krate: &mut ast::Crate,
        sources: &Sources<'_>,
    ) {
        match native_static_items(&self.plan, sources) {
            Ok(items) => krate.items.extend(items),
            Err(error) => compiler.sess.dcx().fatal(error.to_string()),
        }
        let mut roots = native_roots(&self.plan);
        roots.extend(crate::native_crates::declaration_roots(krate));
        // `std` is already declared by the prelude injection; a second `extern crate std;` would collide with it.
        for root in roots
            .into_iter()
            .chain(self.shared.iter().map(String::as_str))
            .filter(|root| !matches!(*root, "std" | "crate"))
        {
            krate
                .items
                .push(declarations::external_crate(root, krate.spans.inner_span));
        }
    }
}

impl rustc_driver::Callbacks for Callbacks<'_> {
    /// Install the source-free root and the MIR query override. `INCAN_NATIVE_TIMING` opts into rustc's phase timings
    /// on stderr without enabling unstable command-line options or changing compilation policy.
    /// The source map owns one filename copied from the plan's shared file handle.
    fn config(&mut self, config: &mut interface::Config) {
        self.timing.mark("rustc_options");
        let mut search = std::mem::take(&mut self.search);
        config
            .opts
            .search_paths
            .retain(|path| !search.iter().any(|held| held.kind == path.kind && held.dir == path.dir));
        search.append(&mut config.opts.search_paths);
        config.opts.search_paths = search;
        self.timing.configure(config);
        config.input = Input::Str {
            name: FileName::Custom(self.plan.span.file.as_str().to_owned()),
            input: String::new(),
        };
        config.override_queries = Some(|_session, providers| {
            providers.queries.mir_built = mir_built;
            providers.queries.type_of = native_alias_type;
            providers.queries.fn_sig = native_function_signature;
        });
    }

    /// Add source-backed layouts, callable signatures, public bridges, and external crates in dependency order.
    /// Structural slice owners use the implicit standard core crate rather than introducing another extern.
    fn after_crate_root_parsing(
        &mut self,
        compiler: &interface::Compiler,
        krate: &mut ast::Crate,
    ) -> rustc_driver::Compilation {
        self.timing.mark("rustc_session");
        let sources = Sources::new(compiler.sess.source_map());

        // ---- Source-backed native declarations ----
        if let Err(error) =
            add_source_backed_declarations(&self.plan, &sources, &compiler.sess.psess.attr_id_generator, krate)
        {
            compiler.sess.dcx().fatal(error.to_string());
        }

        // ---- Callable declarations and canonical entries ----
        self.declare_callables(compiler, krate, &sources);

        // ---- Compiler-owned bridges and public library names ----
        self.declare_bridges_and_exports(compiler, krate);

        // ---- Computed storage and exact native crate roots ----
        self.declare_storage_and_externs(compiler, krate, &sources);
        self.timing.mark("incan_declarations");
        rustc_driver::Compilation::Continue
    }

    /// Verify external signatures and synthesized clone bounds against native metadata before building planned MIR.
    fn after_expansion(&mut self, _compiler: &interface::Compiler, tcx: TyCtxt<'_>) -> rustc_driver::Compilation {
        self.timing.mark("rustc_expansion");
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
        self.timing.mark("incan_native_verification");
        rustc_driver::Compilation::Continue
    }

    /// Snapshot dynamic library generations after analysis, before codegen and linking can publish an executable.
    fn after_analysis(&mut self, _compiler: &interface::Compiler, tcx: TyCtxt<'_>) -> rustc_driver::Compilation {
        self.timing.mark("rustc_analysis");
        if self.development {
            self.loaded = match crate::native_shared::loaded(tcx) {
                Ok(loaded) => loaded,
                Err(error) => tcx.dcx().fatal(error.to_string()),
            };
        }
        self.timing.mark("dynamic_library_snapshot");
        rustc_driver::Compilation::Continue
    }
}

/// Append source-backed carrier and nominal declarations in their established order. The shared source cache maps
/// each declaration span before construction; typed failures return to the callback's existing fatal boundary.
fn add_source_backed_declarations(
    plan: &Plan,
    sources: &Sources<'_>,
    generator: &ast::attr::AttrIdGenerator,
    krate: &mut ast::Crate,
) -> Result<(), PlanError> {
    let span = sources.span(&plan.span)?;
    krate.items.extend(crate::cloneable_closures::items(plan, span)?);
    krate.items.extend(crate::formatting::items(plan, span));
    krate.items.extend(crate::zip_carriers::items(plan, span)?);
    for output in crate::futures::outputs(plan) {
        krate.items.push(crate::futures::alias(&output, krate.spans.inner_span));
        krate
            .items
            .push(crate::futures::borrowed_alias(&output, krate.spans.inner_span));
    }
    for value in &plan.enums {
        let span = sources.span(&value.span)?;
        krate.items.extend(enum_items(value, generator, span));
    }
    for model in &plan.generic_models {
        let span = sources.span(&model.span)?;
        krate
            .items
            .extend(crate::generic_layouts::items(model, generator, span));
    }
    for model in &plan.models {
        let span = sources.span(&model.span)?;
        krate.items.extend(declarations::model_items(model, generator, span));
        if !model.checked_deserialize.is_empty() {
            krate.items.push(crate::checked_json::forwarding_impl(model, span));
        }
    }
    krate
        .items
        .extend(crate::foreign::items(plan, generator, krate.spans.inner_span)?);
    Ok(())
}

/// Keep the declaration order of compiler-owned adapters while propagating their typed preflight refusals.
fn native_bridge_items(plan: &Plan, span: rustc_span::Span) -> Result<Vec<Box<ast::Item>>, PlanError> {
    let mut items = crate::shared_callables::items(plan, span);
    items.extend(crate::panic_hooks::items(plan, span)?);
    items.extend(crate::field_metadata::items(plan, span)?);
    items.extend(crate::comparison_bridges::items(plan, span)?);
    items.extend(crate::fallible_bridges::items(plan, span)?);
    items.extend(crate::fallible_bridges::ordinary_items(plan, span)?);
    items.extend(crate::callable_bridges::items(plan, span)?);
    Ok(items)
}

/// Keep computed storage and its accessors in declaration order; rustc retains their ordinary MIR providers.
fn native_static_items(plan: &Plan, sources: &Sources<'_>) -> Result<Vec<Box<ast::Item>>, PlanError> {
    let mut items = Vec::new();
    for cell in &plan.statics {
        let span = sources.span(&cell.span)?;
        items.extend(crate::statics::items(cell, span));
        items.extend(crate::statics::method_items(plan, cell, span)?);
    }
    Ok(items)
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
/// Generic templates never authorize storage; retained Copy and Clone capabilities must hold for the concrete type.
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
    if value
        .derives
        .iter()
        .any(|derive| derives::from_str(derive) == Some(DeriveId::Copy))
    {
        let native = rustc_middle::ty::Ty::new_adt(tcx, owner, arguments);
        if !tcx.type_is_copy_modulo_regions(rustc_middle::ty::TypingEnv::fully_monomorphized(), native) {
            return Err(invalid("native enum Copy capability differs from metadata"));
        }
    }
    if value
        .derives
        .iter()
        .any(|derive| derives::from_str(derive) == Some(DeriveId::Clone))
    {
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
    let mut inferred = std::collections::BTreeMap::new();
    for external in &plan.externals {
        if !external.type_arguments.is_empty() {
            continue;
        }
        let def = callees::external(tcx, &external.path)?;
        if callees::arity_admits(tcx, def, 0) {
            continue;
        }
        inferred.insert(external.path.clone(), callees::inferred_arguments(tcx, def, external)?);
    }
    if inferred.is_empty() {
        return Ok(None);
    }
    let mut instantiated = plan.clone();
    for external in &mut instantiated.externals {
        if external.type_arguments.is_empty()
            && let Some(arguments) = inferred.get(&external.path)
        {
            external.type_arguments.clone_from(arguments);
        }
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
/// validation. A library manifest lends its already checked public facade names for aliases of the canonical native
/// items. Executables leave debug information in their object files instead of packing a macOS bundle during each edit.
/// A development store derives shared carriers from those same native identities and records loader witnesses;
/// ordinary builds and release compilation do not select that policy.
/// `overflow_checks` is the unit profile's integer overflow policy, the one the lowering already applied to the plan;
/// rustc applies the same policy to the arithmetic it compiles from Rust crates, such as inlined standard library code.
pub fn compile(
    plan: Plan,
    crate_name: &str,
    output: &Path,
    identity: &DriverIdentity,
    externs: &[(String, PathBuf)],
    dependency_search_paths: &[PathBuf],
    rustc_args: &[String],
    shared_store: Option<&Path>,
    kind: UnitKind,
    overflow_checks: bool,
    manifest: Option<&incan_frontend::library_manifest::LibraryManifest>,
) -> Result<(), DriverError> {
    let _invocation = tracing::trace_span!(target: "incan::invocation", "incan_rustc_driver::native_compile", input = %output.display(), functions = plan.functions.len()).entered();
    let mut timing = crate::native_timing::Timing::requested();

    // ---- Validated source plan ----
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
    timing.mark("validated_plan");

    // ---- Optional shared native dependencies ----
    let _compilation = COMPILATION.lock().map_err(|_| DriverError::State)?;
    if shared_store.is_some() && kind == UnitKind::Library {
        return Err(DriverError::Invocation(
            "shared native linking requires a development executable".into(),
        ));
    }
    let shared = match shared_store {
        Some(store) => {
            let mut roots = native_roots(&plan);
            if kind == UnitKind::Test {
                roots.insert("test");
            }
            crate::native_shared::libraries(roots, externs, dependency_search_paths, identity, store)?
        }
        None => Vec::new(),
    };
    timing.mark("shared_dependencies");

    // ---- Fresh rustc session and planned MIR ----
    let plan = Arc::new(plan);
    *PLAN.lock().map_err(|_| DriverError::State)? = Some(Arc::clone(&plan));
    *FAILURE.lock().map_err(|_| DriverError::State)? = None;
    let mut args = compilation_arguments(crate_name, output, identity, kind, overflow_checks, &plan.span.file);
    let all = externs.iter().chain(&shared).cloned().collect::<Vec<_>>();
    for (name, path) in &all {
        args.push("--extern".into());
        args.push(format!("{name}={}", path.display()));
    }
    let search = crate::native_search::dependencies(&all, dependency_search_paths);
    if shared_store.is_some() {
        args.extend(["-Cprefer-dynamic".into(), "-Crpath".into()]);
        // Rustc already constructs the native Darwin invocation; bypass the C driver's extra process for edits.
        if identity.target.ends_with("-apple-darwin") {
            args.push("-Clinker=/usr/bin/ld".into());
        }
    }
    args.extend_from_slice(rustc_args);
    timing.mark("native_arguments");
    let mut callbacks = Callbacks {
        plan,
        manifest,
        search,
        shared: shared.iter().map(|(name, _)| name.clone()).collect(),
        development: shared_store.is_some(),
        loaded: Vec::new(),
        timing,
    };
    let result = rustc_driver::catch_fatal_errors(|| rustc_driver::run_compiler(&args, &mut callbacks));
    callbacks.timing.mark("rustc_codegen_link_teardown");
    *PLAN.lock().map_err(|_| DriverError::State)? = None;
    if let Some(error) = FAILURE.lock().map_err(|_| DriverError::State)?.take() {
        return Err(DriverError::Plan(error));
    }

    // ---- Completed loader witnesses ----
    result.map_err(|_| DriverError::Compilation)?;
    if shared_store.is_some() {
        crate::native_shared::write_witness(output, &callbacks.loaded)?;
    }
    callbacks.timing.mark("loader_witness");
    Ok(())
}

/// Preserve the ordinary native argument sequence; optional development linkage is appended only by its caller.
/// Overflow checks follow the unit profile: on in debug, off in release.
fn compilation_arguments(
    crate_name: &str,
    output: &Path,
    identity: &DriverIdentity,
    kind: UnitKind,
    overflow_checks: bool,
    source: &str,
) -> Vec<String> {
    let overflow = if overflow_checks { "yes" } else { "no" };
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
        format!("overflow-checks={overflow}"),
        "-C".into(),
        "opt-level=3".into(),
        "-C".into(),
        "debuginfo=2".into(),
        "-o".into(),
        output.to_string_lossy().into_owned(),
        // A named input is never read: the callback installs the source-free root without consuming stdin.
        source.into(),
    ];
    if kind == UnitKind::Library {
        args.extend(["--crate-type".into(), "lib".into()]);
    }
    if kind == UnitKind::Test {
        args.push("--test".into());
    }
    if kind != UnitKind::Library {
        args.push("-Csplit-debuginfo=unpacked".into());
    }
    args
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
        } else if generics.parent_count != 0 {
            Some("native layout retains generics from an enclosing declaration")
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
