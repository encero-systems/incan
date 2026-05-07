//! Surface/runtime/interop types vocabulary.
//!
//! These types are part of the language surface (documented, user-facing), but are not "core"
//! builtin types like `int`/`str` and do not belong in `lang::types::*` registries.
//!
//! Each entry carries explicit ownership metadata so stdlib/runtime-facing vocabulary can be filtered without
//! hard-coded side tables.

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

    // Race helpers
    RaceArm,

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

    // Validation
    ValidationError,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SurfaceTypeKind {
    Named,
    Generic,
}

/// The implementation owner responsible for a surface type's semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SurfaceTypeOwner {
    Runtime,
    Stdlib,
    Interop,
}

/// Coarse feature bucket used by compiler and tooling consumers that need grouped surface types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SurfaceTypeCategory {
    AsyncSync,
    AsyncTask,
    AsyncRace,
    AsyncChannel,
    RustInterop,
    Web,
    Reflection,
    Validation,
}

/// Ownership and import-scope metadata for a surface type spelling.
#[derive(Debug, Clone, Copy)]
pub struct SurfaceTypeOwnership {
    pub owner: SurfaceTypeOwner,
    pub category: SurfaceTypeCategory,
    pub stdlib_module_path: Option<&'static str>,
    pub rationale: &'static str,
}

/// Metadata for a surface type spelling.
#[derive(Debug, Clone, Copy)]
pub struct SurfaceTypeInfo {
    pub kind: SurfaceTypeKind,
    pub ownership: SurfaceTypeOwnership,
    pub item: LangItemInfo<SurfaceTypeId>,
}

const RUNTIME_ASYNC_SYNC: SurfaceTypeOwnership = runtime(
    "std.async.sync",
    SurfaceTypeCategory::AsyncSync,
    "Runtime-backed synchronization primitive re-exported through `std.async.sync`; it is not a language builtin.",
);
const RUNTIME_ASYNC_TASK: SurfaceTypeOwnership = runtime(
    "std.async.task",
    SurfaceTypeCategory::AsyncTask,
    "Runtime task vocabulary surfaced through `std.async.task`; name lookup requires the stdlib module import.",
);
const RUNTIME_ASYNC_RACE: SurfaceTypeOwnership = runtime(
    "std.async.race",
    SurfaceTypeCategory::AsyncRace,
    "Runtime race helper vocabulary surfaced through `std.async.race`; name lookup requires the stdlib module import.",
);
const RUNTIME_ASYNC_CHANNEL: SurfaceTypeOwnership = runtime(
    "std.async.channel",
    SurfaceTypeCategory::AsyncChannel,
    "Runtime channel vocabulary surfaced through `std.async.channel`; name lookup requires the stdlib module import.",
);
const INTEROP_RUST: SurfaceTypeOwnership = interop(
    SurfaceTypeCategory::RustInterop,
    "Globally available Rust interop bridge type; no stdlib import owns its name.",
);
const STDLIB_WEB: SurfaceTypeOwnership = stdlib(
    "std.web",
    SurfaceTypeCategory::Web,
    "Web stdlib facade type owned by `std.web`; it is predeclared in core only so compiler passes share a stable spelling.",
);
const STDLIB_REFLECTION: SurfaceTypeOwnership = stdlib(
    "std.reflection",
    SurfaceTypeCategory::Reflection,
    "Reflection stdlib metadata type owned by `std.reflection`; core records the spelling for compiler-generated `__fields__()` results.",
);
const STDLIB_VALIDATION: SurfaceTypeOwnership = SurfaceTypeOwnership {
    owner: SurfaceTypeOwner::Stdlib,
    category: SurfaceTypeCategory::Validation,
    stdlib_module_path: None,
    rationale: "Globally available validated-newtype error type owned by the validation stdlib/runtime surface.",
};

pub const SURFACE_TYPES: &[SurfaceTypeInfo] = &[
    // Async primitives
    info(
        SurfaceTypeId::Mutex,
        "Mutex",
        SurfaceTypeKind::Generic,
        RUNTIME_ASYNC_SYNC,
        "Async/runtime mutex.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        SurfaceTypeId::RwLock,
        "RwLock",
        SurfaceTypeKind::Generic,
        RUNTIME_ASYNC_SYNC,
        "Async/runtime read-write lock.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        SurfaceTypeId::Semaphore,
        "Semaphore",
        SurfaceTypeKind::Named,
        RUNTIME_ASYNC_SYNC,
        "Async/runtime semaphore.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        SurfaceTypeId::Barrier,
        "Barrier",
        SurfaceTypeKind::Named,
        RUNTIME_ASYNC_SYNC,
        "Async/runtime barrier.",
        RFC::_000,
        Since(0, 1),
    ),
    // Task handles
    info(
        SurfaceTypeId::JoinHandle,
        "JoinHandle",
        SurfaceTypeKind::Generic,
        RUNTIME_ASYNC_TASK,
        "Handle to a spawned task.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        SurfaceTypeId::TaskJoinError,
        "TaskJoinError",
        SurfaceTypeKind::Named,
        RUNTIME_ASYNC_TASK,
        "Error returned when a spawned task fails to join.",
        RFC::_000,
        Since(0, 1),
    ),
    // Race helpers
    info(
        SurfaceTypeId::RaceArm,
        "RaceArm",
        SurfaceTypeKind::Generic,
        RUNTIME_ASYNC_RACE,
        "Packaged async race branch.",
        RFC::_039,
        Since(0, 3),
    ),
    // Channels
    info(
        SurfaceTypeId::Sender,
        "Sender",
        SurfaceTypeKind::Generic,
        RUNTIME_ASYNC_CHANNEL,
        "Bounded channel sender.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        SurfaceTypeId::Receiver,
        "Receiver",
        SurfaceTypeKind::Generic,
        RUNTIME_ASYNC_CHANNEL,
        "Bounded channel receiver.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        SurfaceTypeId::OneshotSender,
        "OneshotSender",
        SurfaceTypeKind::Generic,
        RUNTIME_ASYNC_CHANNEL,
        "Oneshot channel sender.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        SurfaceTypeId::OneshotReceiver,
        "OneshotReceiver",
        SurfaceTypeKind::Generic,
        RUNTIME_ASYNC_CHANNEL,
        "Oneshot channel receiver.",
        RFC::_000,
        Since(0, 1),
    ),
    // Interop
    info(
        SurfaceTypeId::Vec,
        "Vec",
        SurfaceTypeKind::Generic,
        INTEROP_RUST,
        "Rust interop `Vec<T>`.",
        RFC::_005,
        Since(0, 1),
    ),
    info(
        SurfaceTypeId::HashMap,
        "HashMap",
        SurfaceTypeKind::Generic,
        INTEROP_RUST,
        "Rust interop `HashMap<K, V>`.",
        RFC::_005,
        Since(0, 1),
    ),
    // Web
    info(
        SurfaceTypeId::App,
        "App",
        SurfaceTypeKind::Named,
        STDLIB_WEB,
        "Web application handle for running an HTTP server.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        SurfaceTypeId::Response,
        "Response",
        SurfaceTypeKind::Named,
        STDLIB_WEB,
        "HTTP response builder for web handlers.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        SurfaceTypeId::Html,
        "Html",
        SurfaceTypeKind::Named,
        STDLIB_WEB,
        "HTML response wrapper for web handlers.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        SurfaceTypeId::Json,
        "Json",
        SurfaceTypeKind::Generic,
        STDLIB_WEB,
        "JSON response/extractor wrapper for web handlers.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        SurfaceTypeId::Query,
        "Query",
        SurfaceTypeKind::Generic,
        STDLIB_WEB,
        "Query-string extractor wrapper for web handlers.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        SurfaceTypeId::Path,
        "Path",
        SurfaceTypeKind::Generic,
        STDLIB_WEB,
        "Path-parameter extractor wrapper for web handlers.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        SurfaceTypeId::Body,
        "Body",
        SurfaceTypeKind::Generic,
        STDLIB_WEB,
        "Request body extractor wrapper for web handlers.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        SurfaceTypeId::Request,
        "Request",
        SurfaceTypeKind::Named,
        STDLIB_WEB,
        "Full HTTP request access for web handlers.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        SurfaceTypeId::FieldInfo,
        "FieldInfo",
        SurfaceTypeKind::Named,
        STDLIB_REFLECTION,
        "Field metadata record returned by __fields__().",
        RFC::_021,
        Since(0, 1),
    ),
    info(
        SurfaceTypeId::ValidationError,
        "ValidationError",
        SurfaceTypeKind::Named,
        STDLIB_VALIDATION,
        "Structured validation error used by validated newtypes.",
        RFC::_017,
        Since(0, 3),
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
    info_for(id).ownership.stdlib_module_path
}

/// Whether this surface type is globally available without an explicit import.
pub fn is_global(id: SurfaceTypeId) -> bool {
    stdlib_module_path(id).is_none()
}

/// Return the implementation owner responsible for this surface type's semantics.
#[must_use]
pub fn owner(id: SurfaceTypeId) -> SurfaceTypeOwner {
    info_for(id).ownership.owner
}

/// Return the coarse feature bucket for this surface type.
#[must_use]
pub fn category(id: SurfaceTypeId) -> SurfaceTypeCategory {
    info_for(id).ownership.category
}

/// Iterate over all surface types with the given implementation owner.
pub fn types_for_owner(owner: SurfaceTypeOwner) -> impl Iterator<Item = &'static SurfaceTypeInfo> {
    SURFACE_TYPES.iter().filter(move |t| t.ownership.owner == owner)
}

/// Iterate over all surface types in the given feature bucket.
pub fn types_in_category(category: SurfaceTypeCategory) -> impl Iterator<Item = &'static SurfaceTypeInfo> {
    SURFACE_TYPES.iter().filter(move |t| t.ownership.category == category)
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
        SurfaceTypeId::RaceArm => SURFACE_TYPES[6],
        SurfaceTypeId::Sender => SURFACE_TYPES[7],
        SurfaceTypeId::Receiver => SURFACE_TYPES[8],
        SurfaceTypeId::OneshotSender => SURFACE_TYPES[9],
        SurfaceTypeId::OneshotReceiver => SURFACE_TYPES[10],
        SurfaceTypeId::Vec => SURFACE_TYPES[11],
        SurfaceTypeId::HashMap => SURFACE_TYPES[12],
        SurfaceTypeId::App => SURFACE_TYPES[13],
        SurfaceTypeId::Response => SURFACE_TYPES[14],
        SurfaceTypeId::Html => SURFACE_TYPES[15],
        SurfaceTypeId::Json => SURFACE_TYPES[16],
        SurfaceTypeId::Query => SURFACE_TYPES[17],
        SurfaceTypeId::Path => SURFACE_TYPES[18],
        SurfaceTypeId::Body => SURFACE_TYPES[19],
        SurfaceTypeId::Request => SURFACE_TYPES[20],
        SurfaceTypeId::FieldInfo => SURFACE_TYPES[21],
        SurfaceTypeId::ValidationError => SURFACE_TYPES[22],
    }
}

/// Build a surface type registry entry.
const fn info(
    id: SurfaceTypeId,
    canonical: &'static str,
    kind: SurfaceTypeKind,
    ownership: SurfaceTypeOwnership,
    description: &'static str,
    introduced_in_rfc: RfcId,
    since: Since,
) -> SurfaceTypeInfo {
    SurfaceTypeInfo {
        kind,
        ownership,
        item: LangItemInfo {
            id,
            canonical,
            aliases: &[],
            description,
            introduced_in_rfc,
            since,
            stability: Stability::Stable,
            examples: &[],
        },
    }
}

/// Build ownership metadata for a runtime-backed type exposed through a stdlib module.
const fn runtime(
    stdlib_module_path: &'static str,
    category: SurfaceTypeCategory,
    rationale: &'static str,
) -> SurfaceTypeOwnership {
    SurfaceTypeOwnership {
        owner: SurfaceTypeOwner::Runtime,
        category,
        stdlib_module_path: Some(stdlib_module_path),
        rationale,
    }
}

/// Build ownership metadata for a stdlib-owned type that core keeps as shared vocabulary.
const fn stdlib(
    stdlib_module_path: &'static str,
    category: SurfaceTypeCategory,
    rationale: &'static str,
) -> SurfaceTypeOwnership {
    SurfaceTypeOwnership {
        owner: SurfaceTypeOwner::Stdlib,
        category,
        stdlib_module_path: Some(stdlib_module_path),
        rationale,
    }
}

/// Build ownership metadata for a globally available Rust interop bridge type.
const fn interop(category: SurfaceTypeCategory, rationale: &'static str) -> SurfaceTypeOwnership {
    SurfaceTypeOwnership {
        owner: SurfaceTypeOwner::Interop,
        category,
        stdlib_module_path: None,
        rationale,
    }
}
