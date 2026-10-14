//! Incan compiler CLI entry point

/// Initialize the shared logging/diagnostic boundary before running the compiler command.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    if let Some(exit_code) = oven_cargo_compat::run_legacy_native_trace_wrapper() {
        std::process::exit(exit_code);
    }
    if let Some(exit_code) = oven_cargo_compat::run_legacy_rustc_trace_wrapper() {
        std::process::exit(exit_code);
    }
    incan_cli::invocation_trace::initialize()?;

    // Compilation recurses over the AST, so run it on a stack sized for deeply nested expressions rather than
    // the default main-thread stack, which a long operator chain can exhaust outright.
    incan_frontend::compiler_stack::run_on_compiler_stack(|| {
        incan_cli::run();
    });
    Ok(())
}
