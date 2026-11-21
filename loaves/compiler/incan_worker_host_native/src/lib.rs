//! The compiler worker host in Rust, side by side with the Incan `incan_worker_host` until its swap.
//!
//! The host listens on the worker's private socket, admits one request at a time and applies the worker's lifecycle;
//! the platform's sockets and trace channel are reached through `incan_worker_host_bindings`. The wire protocols are
//! the `incan.compiler-worker/1` compile documents, the `incan.compiler-worker/2` record activations, and the
//! version-three typed Crumb packets a `cw3-` worker generation speaks. Every frame is byte-identical to the reference
//! worker host's, refusal texts included.

#![forbid(unsafe_code)]

pub mod activation_inputs;
pub mod checked_activation;
pub mod checking_preparation;
pub mod endpoint;
pub mod host;
mod json_text;
pub mod lifecycle;
pub mod namespace_frames;
pub mod protocol;
pub mod record_directory;
pub mod record_protocol;
pub mod record_snapshot;
pub mod worker_packets;

#[cfg(test)]
mod activation_tests;
#[cfg(test)]
mod host_tests;
#[cfg(test)]
mod snapshot_tests;
#[cfg(test)]
mod store_tests;
#[cfg(test)]
mod wire_golden;
#[cfg(test)]
mod wire_tests;
