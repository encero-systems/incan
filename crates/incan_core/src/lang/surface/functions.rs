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

/// Return the metadata entry for a surface function.
///
/// The lookup is exhaustive over the closed enum, so adding a function requires updating this match at compile time.
pub fn info_for(id: SurfaceFnId) -> SurfaceFnInfo {
    match id {
        SurfaceFnId::SleepMs => SURFACE_FUNCTIONS[0],
        SurfaceFnId::Timeout => SURFACE_FUNCTIONS[1],
        SurfaceFnId::TimeoutMs => SURFACE_FUNCTIONS[2],
        SurfaceFnId::SelectTimeout => SURFACE_FUNCTIONS[3],
        SurfaceFnId::YieldNow => SURFACE_FUNCTIONS[4],
        SurfaceFnId::Spawn => SURFACE_FUNCTIONS[5],
        SurfaceFnId::SpawnBlocking => SURFACE_FUNCTIONS[6],
        SurfaceFnId::Channel => SURFACE_FUNCTIONS[7],
        SurfaceFnId::UnboundedChannel => SURFACE_FUNCTIONS[8],
        SurfaceFnId::Oneshot => SURFACE_FUNCTIONS[9],
    }
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
