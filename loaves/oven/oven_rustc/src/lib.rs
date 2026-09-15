//! Oven's bakers and executors: direct-Rustc planning and execution, Loaf envelopes and their mirrors, native test
//! and contract evaluation, interop shims, and the hidden `legacy_cargo` baker that is the one place Cargo runs.
//!
//! Nothing here names a compiler crate. `interop` and `legacy_cargo` stay modules of this crate until their edges
//! into `loaf` and `rustc` are cut; they become `oven_interop` and `oven_cargo_compat` then.

pub mod interop;
pub mod legacy_cargo;
pub mod loaf;
pub mod loaf_mirror;
pub mod native_contract;
pub mod native_test;
pub mod plan;
pub mod rustc;
