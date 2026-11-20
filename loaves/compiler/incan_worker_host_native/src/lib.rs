//! The compiler worker host in Rust, side by side with the Incan `incan_worker_host` until its swap.
//!
//! The wire protocols come first: `incan.compiler-worker/1` compile documents, `incan.compiler-worker/2` record
//! activations, and the version-three typed Crumb packets a `cw3-` worker generation speaks. Every frame is
//! byte-identical to the reference worker host's, refusal texts included.

#![forbid(unsafe_code)]

mod json_text;
pub mod protocol;
pub mod record_protocol;
pub mod worker_packets;

#[cfg(test)]
mod wire_golden;
#[cfg(test)]
mod wire_tests;
