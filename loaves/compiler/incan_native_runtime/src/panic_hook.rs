//! Canonical language panic reporting through the standard native hook.

use std::io::Write;
use std::panic::{self, PanicHookInfo};

/// Install canonical reporting before native entry execution, preserving unwinding and exit status.
pub(crate) fn initialize_panic_hook() {
    panic::set_hook(Box::new(report_panic));
}

/// Report the payload and an available authored Incan call site in one stderr lock.
fn report_panic(info: &PanicHookInfo<'_>) {
    let message = info.payload_as_str().unwrap_or("generated program panicked");
    let location = info.location().filter(|location| location.file().ends_with(".incn"));
    let mut stderr = std::io::stderr().lock();
    let result = match location {
        Some(location) => writeln!(stderr, "{message}\n  --> {location}"),
        None => writeln!(stderr, "{message}"),
    };
    if let Err(error) = result {
        panic::panic_any(format!("failed printing to stderr: {error}"));
    }
}
