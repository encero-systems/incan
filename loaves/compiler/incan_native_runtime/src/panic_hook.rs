//! Payload-only panic reporting through the standard native hook.

use std::io::Write;

/// Install canonical reporting: print a text payload without location decoration, preserving unwinding and status.
pub(crate) fn initialize_panic_hook() {
    std::panic::set_hook(Box::new(|info| report_panic(info.payload_as_str())));
}

/// Print a text payload, or the fallback line for any other payload.
fn report_panic(message: Option<&str>) {
    match message {
        Some(message) => write_panic_line(message),
        None => write_panic_line("generated program panicked"),
    }
}

/// Write one complete diagnostic line with the standard stderr I/O failure behavior.
fn write_panic_line(value: &str) {
    let mut bytes = value.as_bytes().to_vec();
    bytes.push(b'\n');
    if let Err(error) = std::io::stderr().write_all(&bytes) {
        std::panic::panic_any(format!("failed printing to stderr: {error}"));
    }
}
