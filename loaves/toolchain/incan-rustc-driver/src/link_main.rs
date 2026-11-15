//! The development linker: gives each object and archive input a content-derived path, then runs `ld`.
//!
//! A small program of its own, installed beside the driver and never relinked per edit: a development link starts it
//! instead of the driver, so it pays neither the driver's start nor the first-run check of a new executable.
#![deny(clippy::unwrap_used, clippy::expect_used)]

#[path = "adapter/stable_link.rs"]
mod stable_link;

use std::process::ExitCode;

/// Rewrite the link inputs and run `ld`, preserving its status; a failure before `ld` reports itself and exits 1.
fn main() -> ExitCode {
    if let Err(error) = incan_driver::invocation_trace::initialize() {
        eprintln!("incan-link: {error}");
        return ExitCode::from(1);
    }
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    match stable_link::run(&arguments) {
        Ok(0) => ExitCode::SUCCESS,
        Ok(code) => ExitCode::from(u8::try_from(code).unwrap_or(1).max(1)),
        Err(error) => {
            eprintln!("incan-link: {error}");
            ExitCode::from(1)
        }
    }
}
