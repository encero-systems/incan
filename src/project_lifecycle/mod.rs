//! Project lifecycle support shared by project-aware commands.
//!
//! This module contains pure policy helpers for lifecycle commands. CLI parsing, manifest I/O, and filesystem updates
//! live outside this boundary so the policy can be tested independently.

pub mod env;
pub mod version;
