//! Run native entry futures on the canonical async executor.

use std::io::Write;

use incan_std_async::runtime::{RuntimeInitError, block_on};
use incan_std_primitives::task::RuntimeFuture;

use crate::panic_hook::initialize_panic_hook;

/// Run a native entry callback inside the canonical executor.
pub fn run_async_entry(entry: fn()) {
    run_future_entry(async move { entry() });
}

/// Run an owned unit future, reporting executor initialization failure before exiting with status 1.
pub fn run_future_entry<F: RuntimeFuture<()>>(future: F) {
    initialize_panic_hook();
    if let Err(error) = block_on(future) {
        std::process::exit(report_runtime_error(&error));
    }
}

/// Return the program's Result to native main; executor initialization errors keep their separate exit boundary.
pub fn run_result_future_entry<E, F: RuntimeFuture<Result<(), E>>>(future: F) -> Result<(), E> {
    initialize_panic_hook();
    match block_on(future) {
        Ok(value) => value,
        Err(error) => std::process::exit(report_runtime_error(&error)),
    }
}

/// Write the initialization error once and supply its failure exit status.
fn report_runtime_error(error: &RuntimeInitError) -> i32 {
    let _ = std::io::stderr().write_all(format!("{}\n", error.message()).as_bytes());
    1
}
