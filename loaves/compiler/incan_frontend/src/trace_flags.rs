//! Opt-in diagnostic switches, each read from the environment once per process.

use std::sync::OnceLock;

/// Whether `INCAN_TRACE_RUST_FACTS` is set. The variable is read on the first call and the answer serves every
/// Rust-facts trace site after it, in the checker, the session and the driver.
pub fn trace_rust_facts() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("INCAN_TRACE_RUST_FACTS").is_some())
}
