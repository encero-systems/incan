//! Surface/runtime/interop types vocabulary.
//!
//! These types are part of the language surface (documented, user-facing), but are not “core”
//! builtin types like `int`/`str` and do not belong in `lang::types::*` registries.

use crate::lang::registry::{LangItemInfo, RFC, RfcId, Since, Stability};

/// Stable identifier for a surface type.
/// TODO: given RFC 023 approach, we should move/remove some of these types. Stdlibs should be able to define their own
/// types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SurfaceTypeId {
    // Async primitives
    Mutex,
    RwLock,
    Semaphore,
    Barrier,

    // Task handles
    JoinHandle,
    TaskJoinError,

    // Channels
    Sender,
    Receiver,
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
    Path,
    Body,
    Request,

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
    info(
        SurfaceTypeId::TaskJoinError,
        "TaskJoinError",
        &[],
        SurfaceTypeKind::Named,
        "Error returned when a spawned task fails to join.",
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
        SurfaceTypeId::Path,
        "Path",
        &[],
        SurfaceTypeKind::Generic,
        "Path-parameter extractor wrapper for web handlers.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        SurfaceTypeId::Body,
        "Body",
        &[],
        SurfaceTypeKind::Generic,
        "Request body extractor wrapper for web handlers.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        SurfaceTypeId::Request,
        "Request",
        &[],
        SurfaceTypeKind::Named,
        "Full HTTP request access for web handlers.",
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

/// Canonical Incan name of the task join error type (`"TaskJoinError"`).
///
/// Used by the typechecker when wrapping `await JoinHandle[T]` in `Result[T, TaskJoinError]` to avoid scattering the
/// literal string.
pub const TASK_JOIN_ERROR_TYPE_NAME: &str = "TaskJoinError";

/// Canonical Incan name of the semaphore acquire error type (`"SemaphoreAcquireError"`).
pub const SEMAPHORE_ACQUIRE_ERROR_TYPE_NAME: &str = "SemaphoreAcquireError";

/// Canonical Incan name of the semaphore permit type (`"SemaphorePermit"`).
pub const SEMAPHORE_PERMIT_TYPE_NAME: &str = "SemaphorePermit";

/// Return the stdlib module path that owns this surface type, if it is not globally available.
///
/// This is used by the compiler to enforce RFC 022 “explicit imports” for stdlib-scoped types
/// (e.g. `App`, `Mutex`, `FieldInfo`). Rust interop types like `Vec`/`HashMap` remain globally
/// available and return `None`.
pub fn stdlib_module_path(id: SurfaceTypeId) -> Option<&'static str> {
    match id {
        // Async primitives
        SurfaceTypeId::Mutex | SurfaceTypeId::RwLock | SurfaceTypeId::Semaphore | SurfaceTypeId::Barrier => {
            Some("std.async.sync")
        }

        // Task handles
        SurfaceTypeId::JoinHandle | SurfaceTypeId::TaskJoinError => Some("std.async.task"),

        // Channels
        SurfaceTypeId::Sender
        | SurfaceTypeId::Receiver
        | SurfaceTypeId::OneshotSender
        | SurfaceTypeId::OneshotReceiver => Some("std.async.channel"),

        // Web
        SurfaceTypeId::App
        | SurfaceTypeId::Response
        | SurfaceTypeId::Html
        | SurfaceTypeId::Json
        | SurfaceTypeId::Query
        | SurfaceTypeId::Path
        | SurfaceTypeId::Body
        | SurfaceTypeId::Request => Some("std.web"),

        // Reflection
        SurfaceTypeId::FieldInfo => Some("std.reflection"),

        // Interop types are globally available.
        SurfaceTypeId::Vec | SurfaceTypeId::HashMap => None,
    }
}

/// Whether this surface type is globally available without an explicit import.
pub fn is_global(id: SurfaceTypeId) -> bool {
    stdlib_module_path(id).is_none()
}

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

/// Return the metadata entry for a surface type.
///
/// The lookup is exhaustive over the closed enum, so adding a surface type requires updating this match at compile
/// time.
pub fn info_for(id: SurfaceTypeId) -> SurfaceTypeInfo {
    match id {
        SurfaceTypeId::Mutex => SURFACE_TYPES[0],
        SurfaceTypeId::RwLock => SURFACE_TYPES[1],
        SurfaceTypeId::Semaphore => SURFACE_TYPES[2],
        SurfaceTypeId::Barrier => SURFACE_TYPES[3],
        SurfaceTypeId::JoinHandle => SURFACE_TYPES[4],
        SurfaceTypeId::TaskJoinError => SURFACE_TYPES[5],
        SurfaceTypeId::Sender => SURFACE_TYPES[6],
        SurfaceTypeId::Receiver => SURFACE_TYPES[7],
        SurfaceTypeId::OneshotSender => SURFACE_TYPES[8],
        SurfaceTypeId::OneshotReceiver => SURFACE_TYPES[9],
        SurfaceTypeId::Vec => SURFACE_TYPES[10],
        SurfaceTypeId::HashMap => SURFACE_TYPES[11],
        SurfaceTypeId::App => SURFACE_TYPES[12],
        SurfaceTypeId::Response => SURFACE_TYPES[13],
        SurfaceTypeId::Html => SURFACE_TYPES[14],
        SurfaceTypeId::Json => SURFACE_TYPES[15],
        SurfaceTypeId::Query => SURFACE_TYPES[16],
        SurfaceTypeId::Path => SURFACE_TYPES[17],
        SurfaceTypeId::Body => SURFACE_TYPES[18],
        SurfaceTypeId::Request => SURFACE_TYPES[19],
        SurfaceTypeId::FieldInfo => SURFACE_TYPES[20],
    }
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
