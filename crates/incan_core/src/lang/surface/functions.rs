//! Prelude / surface functions (non-syntax vocabulary).
//!
//! This registry covers *globally available* helper functions that are not “core builtins” in the
//! narrow sense, but are part of the language’s standard surface (especially async/time/channel
//! helpers).

use crate::lang::registry::{LangItemInfo, RFC, RfcId, Since, Stability};

/// Stable identifier for a surface function.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SurfaceFnId {
    // Time / async helpers
    SleepMs,
    Timeout,
    TimeoutMs,
    SelectTimeout,
    YieldNow,

    // Task helpers
    Spawn,
    SpawnBlocking,

    // Channels
    Channel,
    UnboundedChannel,
    Oneshot,
}

/// Metadata for a surface function.
pub type SurfaceFnInfo = LangItemInfo<SurfaceFnId>;

pub const SURFACE_FUNCTIONS: &[SurfaceFnInfo] = &[
    info(
        SurfaceFnId::SleepMs,
        // TODO: consider mergeing sleep and sleep_ms or at least moving them together...
        "sleep_ms",
        &[],
        "Sleep for N milliseconds.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        SurfaceFnId::Timeout,
        "timeout",
        &[],
        "Run an async operation with a timeout.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        SurfaceFnId::TimeoutMs,
        "timeout_ms",
        &[],
        "Run an async operation with a timeout in milliseconds.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        SurfaceFnId::SelectTimeout,
        "select_timeout",
        &[],
        "Select between futures with a timeout.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        SurfaceFnId::YieldNow,
        "yield_now",
        &[],
        "Yield execution back to the async scheduler.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        SurfaceFnId::Spawn,
        "spawn",
        &[],
        "Spawn an async task.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        SurfaceFnId::SpawnBlocking,
        "spawn_blocking",
        &[],
        "Spawn a blocking task on a dedicated thread pool.",
        RFC::_004,
        Since(0, 1),
    ),
    info(
        SurfaceFnId::Channel,
        "channel",
        &[],
        "Create a bounded channel (sender, receiver).",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        SurfaceFnId::UnboundedChannel,
        "unbounded_channel",
        &[],
        "Create an unbounded channel (sender, receiver).",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        SurfaceFnId::Oneshot,
        "oneshot",
        &[],
        "Create a oneshot channel (sender, receiver).",
        RFC::_000,
        Since(0, 1),
    ),
];

pub fn from_str(name: &str) -> Option<SurfaceFnId> {
    if let Some(f) = SURFACE_FUNCTIONS.iter().find(|f| f.canonical == name) {
        return Some(f.id);
    }
    SURFACE_FUNCTIONS
        .iter()
        .find(|f| {
            let aliases: &[&str] = f.aliases;
            aliases.contains(&name)
        })
        .map(|f| f.id)
}

pub fn as_str(id: SurfaceFnId) -> &'static str {
    info_for(id).canonical
}

pub fn info_for(id: SurfaceFnId) -> &'static SurfaceFnInfo {
    SURFACE_FUNCTIONS
        .iter()
        .find(|f| f.id == id)
        .expect("INVARIANT: surface function info missing")
}

const fn info(
    id: SurfaceFnId,
    canonical: &'static str,
    aliases: &'static [&'static str],
    description: &'static str,
    introduced_in_rfc: RfcId,
    since: Since,
) -> SurfaceFnInfo {
    LangItemInfo {
        id,
        canonical,
        aliases,
        description,
        introduced_in_rfc,
        since,
        stability: Stability::Stable,
        examples: &[],
    }
}
