//! The only rustc-private boundary: Incan declarations enter as AST items and bodies enter as MIR.

use crate::error::PlanError;
use crate::identity::{self, IdentityError};
use crate::plan::Plan;
use crate::spans::Sources;
use crate::{bodies, callees, declarations, types, validation};
use rustc_ast as ast;
use rustc_data_structures::steal::Steal;
use rustc_hir::def_id::LocalDefId;
use rustc_interface::interface;
use rustc_middle::mir::Body;
use rustc_middle::ty::TyCtxt;
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

/// Typed refusals and native compiler failures remain distinct at the executable boundary.
#[derive(Debug, thiserror::Error)]
pub enum DriverError {
    /// The scalar plan violates its preflight contract.
    #[error(transparent)]
    Plan(#[from] PlanError),
    /// Build-bound toolchain evidence does not match the runtime.
    #[error(transparent)]
    Identity(#[from] IdentityError),
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

/// Override only source-named planned functions; all compiler-generated items retain rustc's provider.
fn mir_built<'tcx>(tcx: TyCtxt<'tcx>, def: LocalDefId) -> &'tcx Steal<Body<'tcx>> {
    let plan = match active_plan() {
        Ok(plan) => plan,
        Err(error) => tcx.dcx().fatal(error.to_string()),
    };
    let name = tcx.opt_item_name(def.to_def_id());
    let function = plan
        .functions
        .iter()
        .find(|function| name.is_some_and(|name| name.as_str() == function.name));
    match function {
        Some(function) => match bodies::body(tcx, def, function) {
            Ok(body) => tcx.alloc_steal_mir(body),
            Err(error) => refuse(tcx, error),
        },
        None => (rustc_interface::DEFAULT_QUERY_PROVIDERS.queries.mir_built)(tcx, def),
    }
}

/// Inject the prevalidated declarations after parsing an empty Incan crate root.
struct Callbacks {
    plan: Arc<Plan>,
}

impl rustc_driver::Callbacks for Callbacks {
    /// Install the source-free root and the MIR query override; cap lints that only see placeholder bodies.
    fn config(&mut self, config: &mut interface::Config) {
        config.input = Input::Str {
            name: FileName::Custom(self.plan.span.file.clone()),
            input: String::new(),
        };
        config.override_queries = Some(|_session, providers| providers.queries.mir_built = mir_built);
    }

    /// Add function signatures and load the declared external crates, without parsing generated source.
    fn after_crate_root_parsing(
        &mut self,
        compiler: &interface::Compiler,
        krate: &mut ast::Crate,
    ) -> rustc_driver::Compilation {
        let sources = Sources::new(compiler.sess.source_map());
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
            krate.items.push(declarations::function(function, span));
        }
        let roots: BTreeSet<_> = self
            .plan
            .externals
            .iter()
            .filter_map(|external| external.path.split("::").next())
            .collect();
        for root in roots {
            krate
                .items
                .push(declarations::external_crate(root, krate.spans.inner_span));
        }
        rustc_driver::Compilation::Continue
    }

    /// Verify admitted external signatures against canonical dependency metadata before any planned MIR is built.
    fn after_expansion(&mut self, _compiler: &interface::Compiler, tcx: TyCtxt<'_>) -> rustc_driver::Compilation {
        for external in &self.plan.externals {
            let def = match callees::external(tcx, &external.path) {
                Ok(def) => def,
                Err(error) => refuse(tcx, error),
            };
            let signature = tcx.fn_sig(def).instantiate_identity().skip_binder();
            let parameters: Vec<_> = external
                .parameters
                .iter()
                .map(|ty| types::native_type(tcx, ty))
                .collect();
            if signature.inputs() != parameters
                || signature.output() != types::native_type(tcx, &external.return_type)
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

/// Compile a plan supplied directly by an Incan caller function into a native binary.
///
/// Preflight and exact toolchain/library identity verification precede rustc. External crates are explicit rlib
/// paths; their metadata resolves canonical callees. No generated Rust source or ambient unstable grant is used.
pub fn compile(
    plan: Plan,
    crate_name: &str,
    output: &Path,
    sysroot: &Path,
    externs: &[(String, PathBuf)],
    dependency_search_paths: &[PathBuf],
) -> Result<(), DriverError> {
    validation::validate(&plan)?;
    let identity = identity::verify(sysroot)?;
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
        "debuginfo=2".into(),
        "-o".into(),
        output.to_string_lossy().into_owned(),
        "-".into(),
    ];
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
    let result = rustc_driver::catch_fatal_errors(|| rustc_driver::run_compiler(&args, &mut Callbacks { plan }));
    *PLAN.lock().map_err(|_| DriverError::State)? = None;
    if let Some(error) = FAILURE.lock().map_err(|_| DriverError::State)?.take() {
        return Err(DriverError::Plan(error));
    }
    result.map_err(|_| DriverError::Compilation)
}
