//! Surface/runtime/interop types vocabulary.
//!
//! These types are part of the language surface (documented, user-facing), but are not "core" builtin types like
//! `int`/`str` and do not belong in `lang::types::*` registries.
//!
//! Each entry carries explicit ownership metadata so stdlib/runtime-facing vocabulary can be filtered without
//! hard-coded side tables.

use crate::lang::derives::DeriveId;
use crate::lang::registry::{LangItemInfo, RFC, RfcId, Since, Stability};
use crate::lang::stdlib::facets;

/// Stable identifier for a surface type. TODO: given RFC 023 approach, we should move/remove some of these types.
/// Stdlibs should be able to define their own types.
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
    /// Whether a value of this type can be neither copied nor cloned, so a use that takes it by value moves the one
    /// value out of the place that holds it.
    ///
    /// Set only where the runtime type is known to implement neither `Copy` nor `Clone` and the checker relies on it:
    /// a `for` loop whose body hands such an item on by value takes the items out of the list it iterates (#1844).
    /// An unset flag does not claim that the type can be cloned.
    pub not_cloneable: bool,
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
const STDLIB_ASYNC_CHANNEL: SurfaceTypeOwnership = stdlib(
    "std.async.channel",
    SurfaceTypeCategory::AsyncChannel,
    "Channel handle declared by `std.async.channel` as a newtype over its runtime type, the type `channel()` and \
     `oneshot()` return; core records the spelling so compiler passes share it, and name lookup requires the stdlib \
     module import.",
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
    not_cloneable(info(
        SurfaceTypeId::JoinHandle,
        "JoinHandle",
        SurfaceTypeKind::Generic,
        RUNTIME_ASYNC_TASK,
        "Handle to a spawned task.",
        RFC::_000,
        Since(0, 1),
    )),
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
        STDLIB_ASYNC_CHANNEL,
        "Bounded channel sender.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        SurfaceTypeId::Receiver,
        "Receiver",
        SurfaceTypeKind::Generic,
        STDLIB_ASYNC_CHANNEL,
        "Bounded channel receiver.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        SurfaceTypeId::OneshotSender,
        "OneshotSender",
        SurfaceTypeKind::Generic,
        STDLIB_ASYNC_CHANNEL,
        "Oneshot channel sender.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        SurfaceTypeId::OneshotReceiver,
        "OneshotReceiver",
        SurfaceTypeKind::Generic,
        STDLIB_ASYNC_CHANNEL,
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

/// Canonical Incan name of the checked range type (`"Range"`).
///
/// This name is compiler-synthesized rather than source-spellable: `TypeChecker::check_range_expr` is its only
/// producer, and writing `Range` in source resolves nothing. Because no declaration can introduce it, the producer
/// and every consumer agree only by spelling the same string -- so they share this constant rather than three
/// literals that drift silently.
///
/// Distinct from the `range()` builtin's iterator return type, which is a plain `Named("Range")` and deliberately
/// keeps its own iteration path.
pub const RANGE_TYPE_NAME: &str = "Range";

/// Canonical Incan name of the semaphore acquire error type (`"SemaphoreAcquireError"`).
pub const SEMAPHORE_ACQUIRE_ERROR_TYPE_NAME: &str = "SemaphoreAcquireError";

/// Canonical Incan name of the semaphore permit type (`"SemaphorePermit"`).
pub const SEMAPHORE_PERMIT_TYPE_NAME: &str = "SemaphorePermit";

/// Return the stdlib module path that owns this surface type, if it is not globally available.
///
/// This is used by the compiler to enforce RFC 022 “explicit imports” for stdlib-scoped types (e.g. `App`, `Mutex`,
/// `FieldInfo`). Rust interop types like `Vec`/`HashMap` remain globally available and return `None`.
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

/// Whether a field read on a value of this surface type reads a field of the one type it wraps.
///
/// `Json[T]` and `Query[T]` are the web extractor wrappers: `body.name` on a `Json[User]` reads the `name` field of the
/// `User` it carries, and `body.value` is that `User`. The checker resolves such a field on `T`, and the generated read
/// goes through the wrapper, so the field is borrowed storage however the wrapper itself is owned.
#[must_use]
pub fn field_access_reads_wrapped_value(id: SurfaceTypeId) -> bool {
    matches!(id, SurfaceTypeId::Json | SurfaceTypeId::Query)
}

/// Whether a value of this surface type can be neither copied nor cloned; see [`SurfaceTypeInfo::not_cloneable`].
#[must_use]
pub fn is_not_cloneable(id: SurfaceTypeId) -> bool {
    info_for(id).not_cloneable
}

/// Return the runtime-owned surface type whose runtime Rust type `rust_path` names, generic arguments allowed.
///
/// A stdlib provider's checked API names a runtime type by its Rust path where source spells the surface type:
/// `spawn` returns `incan_std_async::task::JoinHandle<T>` for `JoinHandle[T]`. The runtime type of a surface type
/// declared in `std.<namespace>.<rest>` lives at `<facet>[::<namespace>]::<rest>::<Name>`, the location runtime
/// re-exports use, so a path names the surface type exactly when it spells that location.
#[must_use]
pub fn from_runtime_rust_path(rust_path: &str) -> Option<SurfaceTypeId> {
    let path = rust_path.strip_prefix("::").unwrap_or(rust_path);
    let base = path.split_once('<').map_or(path, |(base, _)| base).trim_end();
    let id = from_str(base.rsplit("::").next()?)?;
    runtime_rust_path_segments(id)?
        .into_iter()
        .eq(base.split("::"))
        .then_some(id)
}

/// Return the segments of the Rust path a runtime-owned surface type's runtime type lives at.
fn runtime_rust_path_segments(id: SurfaceTypeId) -> Option<Vec<&'static str>> {
    let ownership = info_for(id).ownership;
    if ownership.owner != SurfaceTypeOwner::Runtime {
        return None;
    }
    let mut module = ownership.stdlib_module_path?.split('.').skip(1);
    let namespace = module.next()?;
    let mut segments = vec![facets::for_namespace(namespace)];
    if !facets::namespace_is_facet_root(namespace) {
        segments.push(namespace);
    }
    segments.extend(module);
    segments.push(as_str(id));
    Some(segments)
}

/// What the compiler records about one surface type's implementation of a builtin derive.
///
/// A surface type is realized by a runtime struct or by a stdlib newtype, and the answer mirrors that declaration: the
/// struct's own trait implementations, or the newtype's derive list. The typechecker's derive relation reads it for
/// every question it asks of a surface type (the automatic `Clone` and `Debug` of a field's type, #1754; `Eq` and
/// `Hash` of a set element or dict key, #1758; a clone the checker requires).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceDeriveSupport {
    /// The realization implements the derive for every type argument.
    Implements,
    /// The realization implements the derive exactly when its type arguments do, as Rust's `Vec` does.
    FollowsTypeArguments,
    /// The realization does not implement the derive, whatever its type arguments.
    Missing,
    /// Nothing is recorded: the compiler makes no claim, and each consumer keeps its own policy for an unknown type.
    NotRecorded,
}

/// Return what is recorded about this surface type's implementation of `derive`.
///
/// Answers are recorded for `Clone`, `Debug`, `Eq` and `Hash`; every other derive is
/// [`SurfaceDeriveSupport::NotRecorded`]. The match is exhaustive over the closed enum, so a new surface type states
/// its answers when it is added. The `Clone` answers of the stdlib newtypes restate their `@derive(Clone)`
/// declarations, and a test reads those declarations and fails when the two disagree.
#[must_use]
pub fn derive_support(id: SurfaceTypeId, derive: DeriveId) -> SurfaceDeriveSupport {
    use SurfaceDeriveSupport::{FollowsTypeArguments, Implements, Missing, NotRecorded};
    if !matches!(
        derive,
        DeriveId::Clone | DeriveId::Debug | DeriveId::Eq | DeriveId::Hash
    ) {
        return NotRecorded;
    }
    let is_eq_or_hash = matches!(derive, DeriveId::Eq | DeriveId::Hash);
    match id {
        // A task handle owns its task and a race arm its pending future: the runtime structs implement none of these.
        SurfaceTypeId::JoinHandle | SurfaceTypeId::RaceArm => Missing,
        // Generic `@derive(Clone)` stdlib newtypes over shared runtime state. A derive on a generic type bounds its
        // parameter, so `Clone` and the automatic `Debug` hold exactly when the element type's do.
        SurfaceTypeId::Mutex | SurfaceTypeId::RwLock | SurfaceTypeId::Sender => {
            if is_eq_or_hash {
                Missing
            } else {
                FollowsTypeArguments
            }
        }
        // Non-generic `@derive(Clone)` stdlib newtypes; the runtime types they wrap implement `Debug`.
        SurfaceTypeId::Semaphore | SurfaceTypeId::Barrier => {
            if is_eq_or_hash {
                Missing
            } else {
                Implements
            }
        }
        // Generic stdlib newtypes without `@derive(Clone)`: they carry the automatic `Debug` only.
        SurfaceTypeId::Receiver | SurfaceTypeId::OneshotSender | SurfaceTypeId::OneshotReceiver => {
            if derive == DeriveId::Debug {
                FollowsTypeArguments
            } else {
                Missing
            }
        }
        // The runtime join error derives `Clone` and implements `Debug`, and nothing else.
        SurfaceTypeId::TaskJoinError => {
            if is_eq_or_hash {
                Missing
            } else {
                Implements
            }
        }
        SurfaceTypeId::Vec => FollowsTypeArguments,
        // A hash map implements `Hash` for no arguments.
        SurfaceTypeId::HashMap => {
            if derive == DeriveId::Hash {
                Missing
            } else {
                FollowsTypeArguments
            }
        }
        // These web owners contain runtime state that provides no `Clone`; a kept dict lookup therefore cannot copy
        // one out of the map (#1830). Other derive capabilities remain unspecified here.
        SurfaceTypeId::App | SurfaceTypeId::Response | SurfaceTypeId::Request if derive == DeriveId::Clone => Missing,
        SurfaceTypeId::App
        | SurfaceTypeId::Response
        | SurfaceTypeId::Html
        | SurfaceTypeId::Json
        | SurfaceTypeId::Query
        | SurfaceTypeId::Path
        | SurfaceTypeId::Body
        | SurfaceTypeId::Request
        | SurfaceTypeId::FieldInfo
        | SurfaceTypeId::ValidationError => NotRecorded,
    }
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
        not_cloneable: false,
    }
}

/// Mark a surface type entry whose values can be neither copied nor cloned.
const fn not_cloneable(entry: SurfaceTypeInfo) -> SurfaceTypeInfo {
    SurfaceTypeInfo {
        not_cloneable: true,
        ..entry
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_handle_and_race_arm_lack_every_recorded_derive() {
        for id in [SurfaceTypeId::JoinHandle, SurfaceTypeId::RaceArm] {
            for derive in [DeriveId::Clone, DeriveId::Debug, DeriveId::Eq, DeriveId::Hash] {
                assert_eq!(
                    derive_support(id, derive),
                    SurfaceDeriveSupport::Missing,
                    "{} must be recorded as lacking {}",
                    as_str(id),
                    crate::lang::derives::as_str(derive)
                );
            }
        }
    }

    #[test]
    fn web_owned_handles_lack_clone_issue1830() {
        for id in [SurfaceTypeId::App, SurfaceTypeId::Response, SurfaceTypeId::Request] {
            assert_eq!(
                derive_support(id, DeriveId::Clone),
                SurfaceDeriveSupport::Missing,
                "{} must be recorded as non-Clone",
                as_str(id)
            );
        }
    }

    #[test]
    fn runtime_rust_path_names_the_surface_type_at_its_facet_location() {
        for path in [
            "incan_std_async::task::JoinHandle<T>",
            "incan_std_async::task::JoinHandle<i64>",
            "::incan_std_async::task::JoinHandle",
        ] {
            assert_eq!(from_runtime_rust_path(path), Some(SurfaceTypeId::JoinHandle), "{path}");
        }
        assert_eq!(
            from_runtime_rust_path("incan_std_async::sync::Mutex<i64>"),
            Some(SurfaceTypeId::Mutex)
        );
        assert_eq!(
            from_runtime_rust_path("incan_std_async::channel::Sender<String>"),
            None,
            "a channel handle is the stdlib newtype, not its runtime type"
        );
        for path in [
            "incan_std_async::sync::JoinHandle<T>",
            "incan_std_async::JoinHandle<T>",
            "tokio::task::JoinHandle<T>",
            "incan_std_core::task::JoinHandle<T>",
            "incan_std_async::task::Unknown<T>",
        ] {
            assert_eq!(from_runtime_rust_path(path), None, "{path}");
        }
    }

    /// `std.async.channel` declares each channel handle as a newtype over its runtime type and `channel()` returns the
    /// newtype, so an import of the handle binds the newtype rather than re-exporting the runtime type.
    #[test]
    fn channel_handles_are_the_stdlib_newtypes_channel_returns() {
        for id in [
            SurfaceTypeId::Sender,
            SurfaceTypeId::Receiver,
            SurfaceTypeId::OneshotSender,
            SurfaceTypeId::OneshotReceiver,
        ] {
            assert_eq!(owner(id), SurfaceTypeOwner::Stdlib, "{}", as_str(id));
            assert_eq!(stdlib_module_path(id), Some("std.async.channel"), "{}", as_str(id));
        }
    }

    #[test]
    fn channel_and_lock_handles_follow_their_stdlib_newtype_derives() {
        for id in [SurfaceTypeId::Mutex, SurfaceTypeId::Sender] {
            assert_eq!(
                derive_support(id, DeriveId::Clone),
                SurfaceDeriveSupport::FollowsTypeArguments
            );
            assert_eq!(derive_support(id, DeriveId::Hash), SurfaceDeriveSupport::Missing);
        }
        assert_eq!(
            derive_support(SurfaceTypeId::Semaphore, DeriveId::Clone),
            SurfaceDeriveSupport::Implements
        );
        for id in [
            SurfaceTypeId::Receiver,
            SurfaceTypeId::OneshotSender,
            SurfaceTypeId::OneshotReceiver,
        ] {
            assert_eq!(derive_support(id, DeriveId::Clone), SurfaceDeriveSupport::Missing);
            assert_eq!(
                derive_support(id, DeriveId::Debug),
                SurfaceDeriveSupport::FollowsTypeArguments
            );
        }
    }

    /// Return whether `source` declares `name` as a `pub type ... = newtype`, and if so whether an `@derive(...)`
    /// naming `Clone` decorates it.
    fn stdlib_newtype_derives_clone(source: &str, name: &str) -> Option<bool> {
        let lines = source.lines().collect::<Vec<_>>();
        let index = lines.iter().position(|line| {
            line.strip_prefix("pub type ")
                .and_then(|rest| rest.strip_prefix(name))
                .is_some_and(|rest| rest.starts_with(['[', ' ']) && rest.contains("= newtype "))
        })?;
        let decorators = lines.get(..index)?;
        Some(
            decorators
                .iter()
                .rev()
                .take_while(|line| line.trim_start().starts_with('@'))
                .filter_map(|line| line.trim().strip_prefix("@derive("))
                .any(|args| args.trim_end_matches(')').split(',').any(|arg| arg.trim() == "Clone")),
        )
    }

    /// The `Clone` answers for stdlib newtypes are copied from their `@derive(Clone)` declarations; this fails when a
    /// declaration and the registry disagree, or when a declaration the registry describes can no longer be found.
    #[test]
    fn clone_answers_match_the_stdlib_newtype_declarations() -> Result<(), String> {
        let stdlib_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../stdlib");
        let mut checked = Vec::new();
        for info in SURFACE_TYPES {
            let Some(segments) = info
                .ownership
                .stdlib_module_path
                .and_then(|module| module.strip_prefix("std."))
                .map(|module| module.split('.').collect::<Vec<_>>())
            else {
                continue;
            };
            let Some(facet) = segments.first() else {
                continue;
            };
            let path = stdlib_root
                .join(facet)
                .join("src")
                .join(format!("{}.incn", segments.join("/")));
            let Ok(source) = std::fs::read_to_string(&path) else {
                continue;
            };
            let name = as_str(info.item.id);
            let Some(declared) = stdlib_newtype_derives_clone(&source, name) else {
                continue;
            };
            let recorded = matches!(
                derive_support(info.item.id, DeriveId::Clone),
                SurfaceDeriveSupport::Implements | SurfaceDeriveSupport::FollowsTypeArguments
            );
            if declared != recorded {
                return Err(format!(
                    "{name}: {} declares @derive(Clone) = {declared}, but derive_support records Clone = {recorded}",
                    path.display()
                ));
            }
            checked.push(name);
        }
        for expected in [
            "Mutex",
            "RwLock",
            "Semaphore",
            "Barrier",
            "Sender",
            "Receiver",
            "OneshotSender",
            "OneshotReceiver",
        ] {
            if !checked.contains(&expected) {
                return Err(format!(
                    "the stdlib newtype declaration of {expected} was not found; checked: {checked:?}"
                ));
            }
        }
        Ok(())
    }

    #[test]
    fn interop_collections_follow_their_arguments_and_unverified_types_are_not_recorded() {
        assert_eq!(
            derive_support(SurfaceTypeId::Vec, DeriveId::Hash),
            SurfaceDeriveSupport::FollowsTypeArguments
        );
        assert_eq!(
            derive_support(SurfaceTypeId::HashMap, DeriveId::Clone),
            SurfaceDeriveSupport::FollowsTypeArguments
        );
        assert_eq!(
            derive_support(SurfaceTypeId::HashMap, DeriveId::Hash),
            SurfaceDeriveSupport::Missing
        );
        assert_eq!(
            derive_support(SurfaceTypeId::Html, DeriveId::Clone),
            SurfaceDeriveSupport::NotRecorded
        );
        assert_eq!(
            derive_support(SurfaceTypeId::Mutex, DeriveId::Default),
            SurfaceDeriveSupport::NotRecorded
        );
    }
}
