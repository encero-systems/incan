//! Surface/runtime/interop types vocabulary.
//!
//! These types are part of the language surface (documented, user-facing), but are not “core”
//! builtin types like `int`/`str` and do not belong in `lang::types::*` registries.

use crate::lang::registry::{LangItemInfo, RFC, RfcId, Since, Stability};

/// Stable identifier for a surface type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SurfaceTypeId {
    // Async primitives
    Mutex,
    RwLock,
    Semaphore,
    Barrier,

    // Task handles
    JoinHandle,

    // Channels
    Sender,
    Receiver,
    UnboundedSender,
    UnboundedReceiver,
    OneshotSender,
    OneshotReceiver,

    // Interop types
    Vec,
    HashMap,

    // Web
    App,
    Response,
    Html,
    Json,
    Query,

    // Reflection
    FieldInfo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SurfaceTypeKind {
    Named,
    Generic,
}

#[derive(Debug, Clone, Copy)]
pub struct SurfaceTypeInfo {
    pub kind: SurfaceTypeKind,
    pub item: LangItemInfo<SurfaceTypeId>,
}

pub const SURFACE_TYPES: &[SurfaceTypeInfo] = &[
    // Async primitives
    info(
        SurfaceTypeId::Mutex,
        "Mutex",
        &[],
        SurfaceTypeKind::Generic,
        "Async/runtime mutex.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        SurfaceTypeId::RwLock,
        "RwLock",
        &[],
        SurfaceTypeKind::Generic,
        "Async/runtime read-write lock.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        SurfaceTypeId::Semaphore,
        "Semaphore",
        &[],
        SurfaceTypeKind::Named,
        "Async/runtime semaphore.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        SurfaceTypeId::Barrier,
        "Barrier",
        &[],
        SurfaceTypeKind::Named,
        "Async/runtime barrier.",
        RFC::_000,
        Since(0, 1),
    ),
    // Task handles
    info(
        SurfaceTypeId::JoinHandle,
        "JoinHandle",
        &[],
        SurfaceTypeKind::Generic,
        "Handle to a spawned task.",
        RFC::_000,
        Since(0, 1),
    ),
    // Channels
    info(
        SurfaceTypeId::Sender,
        "Sender",
        &[],
        SurfaceTypeKind::Generic,
        "Bounded channel sender.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        SurfaceTypeId::Receiver,
        "Receiver",
        &[],
        SurfaceTypeKind::Generic,
        "Bounded channel receiver.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        SurfaceTypeId::UnboundedSender,
        "UnboundedSender",
        &[],
        SurfaceTypeKind::Generic,
        "Unbounded channel sender.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        SurfaceTypeId::UnboundedReceiver,
        "UnboundedReceiver",
        &[],
        SurfaceTypeKind::Generic,
        "Unbounded channel receiver.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        SurfaceTypeId::OneshotSender,
        "OneshotSender",
        &[],
        SurfaceTypeKind::Generic,
        "Oneshot channel sender.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        SurfaceTypeId::OneshotReceiver,
        "OneshotReceiver",
        &[],
        SurfaceTypeKind::Generic,
        "Oneshot channel receiver.",
        RFC::_000,
        Since(0, 1),
    ),
    // Interop
    info(
        SurfaceTypeId::Vec,
        "Vec",
        &[],
        SurfaceTypeKind::Generic,
        "Rust interop `Vec<T>`.",
        RFC::_005,
        Since(0, 1),
    ),
    info(
        SurfaceTypeId::HashMap,
        "HashMap",
        &[],
        SurfaceTypeKind::Generic,
        "Rust interop `HashMap<K, V>`.",
        RFC::_005,
        Since(0, 1),
    ),
    // Web
    info(
        SurfaceTypeId::App,
        "App",
        &[],
        SurfaceTypeKind::Named,
        "Web application handle for running an HTTP server.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        SurfaceTypeId::Response,
        "Response",
        &[],
        SurfaceTypeKind::Named,
        "HTTP response builder for web handlers.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        SurfaceTypeId::Html,
        "Html",
        &[],
        SurfaceTypeKind::Named,
        "HTML response wrapper for web handlers.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        SurfaceTypeId::Json,
        "Json",
        &[],
        SurfaceTypeKind::Generic,
        "JSON response/extractor wrapper for web handlers.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        SurfaceTypeId::Query,
        "Query",
        &[],
        SurfaceTypeKind::Generic,
        "Query-string extractor wrapper for web handlers.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        SurfaceTypeId::FieldInfo,
        "FieldInfo",
        &[],
        SurfaceTypeKind::Named,
        "Field metadata record returned by __fields__().",
        RFC::_021,
        Since(0, 1),
    ),
];

pub fn from_str(name: &str) -> Option<SurfaceTypeId> {
    if let Some(t) = SURFACE_TYPES.iter().find(|t| t.item.canonical == name) {
        return Some(t.item.id);
    }
    SURFACE_TYPES
        .iter()
        .find(|t| {
            let aliases: &[&str] = t.item.aliases;
            aliases.contains(&name)
        })
        .map(|t| t.item.id)
}

pub fn as_str(id: SurfaceTypeId) -> &'static str {
    info_for(id).item.canonical
}

pub fn info_for(id: SurfaceTypeId) -> &'static SurfaceTypeInfo {
    SURFACE_TYPES
        .iter()
        .find(|t| t.item.id == id)
        .expect("surface type info missing")
}

const fn info(
    id: SurfaceTypeId,
    canonical: &'static str,
    aliases: &'static [&'static str],
    kind: SurfaceTypeKind,
    description: &'static str,
    introduced_in_rfc: RfcId,
    since: Since,
) -> SurfaceTypeInfo {
    SurfaceTypeInfo {
        kind,
        item: LangItemInfo {
            id,
            canonical,
            aliases,
            description,
            introduced_in_rfc,
            since,
            stability: Stability::Stable,
            examples: &[],
        },
    }
}
