//! Oven's direct-Rustc planning and execution, Loaf envelopes and their mirrors, native test and contract
//! evaluation, and the Cargo-free wire contract the native route reads.
//!
//! Nothing here names a compiler crate, and nothing here runs Cargo: the interop shims are `oven_interop` and the
//! explicit compatibility baker is `oven_cargo_compat`, both over this crate, never under it.

pub mod loaf;
pub mod loaf_mirror;
pub mod native_contract;
pub mod native_test;
pub mod plan;
pub mod rustc;
