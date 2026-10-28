//! Compile a Rust producer without constructing its optional inspection graph.

use rustc_interface::interface;

/// Run Oven's exact native Rust arguments through the same pinned compiler policy as the graph-producing route.
/// Graph export remains a separate observer request; native completeness requires only rustc's successful outputs.
pub(crate) fn compile(arguments: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let _span = tracing::trace_span!(target: "incan::invocation", "incan_rustc_driver::compile_rust").entered();
    let mut args = vec!["incan-rustc-driver".to_owned()];
    args.extend_from_slice(arguments);
    rustc_driver::catch_fatal_errors(|| rustc_driver::run_compiler(&args, &mut NativeCompiler))
        .map_err(|_| "Rust compilation failed".into())
}

/// Preserve the existing custom driver's tracked configuration while omitting graph queries entirely.
struct NativeCompiler;

impl rustc_driver::Callbacks for NativeCompiler {
    /// Keep metadata compatible with the existing driver. The pinned rustc executable's TimePassesCallbacks turns
    /// on tracked path trimming; switching to that executable changes crate hashes even for identical arguments.
    fn config(&mut self, config: &mut interface::Config) {
        config.opts.trimmed_def_paths = false;
        crate::native_timing::Timing::requested().configure(config);
    }
}
