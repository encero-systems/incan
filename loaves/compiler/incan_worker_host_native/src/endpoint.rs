//! A worker's socket file and what happens when a caller leaves.
//!
//! The worker's socket file is the platform's `SocketFile`: the worker removes only the file it created, so a
//! replacement worker's socket at the same path survives. A caller that leaves while its activation compiles can no
//! longer receive the result, so the worker withdraws its socket and exits, as the short-lived command would have
//! been ended with it.

use incan_worker_host_bindings::{SocketFile, withdraw};

use crate::protocol::PANIC_STATUS;

/// What the compiler entry watches while an activation compiles: the caller's socket descriptor, lent for the
/// activation, and the worker's socket file, which a caller that left makes the worker withdraw.
#[derive(Clone)]
pub struct CallerWatch {
    pub descriptor: i64,
    pub socket: SocketFile,
}

/// End the worker whose caller left during an activation: withdraw its socket, so no later request dials it, and exit
/// with the status a panicked command reports.
pub fn abandon(watch: &CallerWatch) {
    withdraw(&watch.socket);
    std::process::exit(PANIC_STATUS as i32);
}
