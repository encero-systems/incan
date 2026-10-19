//! Incan compiler CLI entry point

/// Initialize the shared logging/diagnostic boundary before running the compiler command.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    incan_cli::invocation_trace::initialize()?;

    // Compilation recurses over the AST, so run it on a stack sized for deeply nested expressions rather than
    // the default main-thread stack, which a long operator chain can exhaust outright.
    incan_frontend::compiler_stack::run_on_compiler_stack(incan_cli::run);
    Ok(())
}
