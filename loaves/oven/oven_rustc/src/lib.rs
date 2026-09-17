//! Oven's bakers and executors: direct-Rustc planning and execution, Loaf envelopes and their mirrors, native test
//! and contract evaluation, and the hidden `legacy_cargo` baker that is the one place Cargo runs.
//!
//! Nothing here names a compiler crate. Interop shims are `oven_interop`, over this crate; `legacy_cargo` stays a
//! module of this crate until its edges into `loaf` and `rustc` are cut, and becomes `oven_cargo_compat` then.

pub mod legacy_cargo;
pub mod loaf;
pub mod loaf_mirror;
pub mod native_contract;
pub mod native_test;
pub mod plan;
pub mod rustc;
