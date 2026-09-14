//! What Oven knows about a project's lock before the generic lock model moves here: its filename.

/// The generated project lockfile's filename, resolved relative to a project or workspace root.
///
/// This names the *project* lock that records the resolved dependency, provider, and interop graph. RFC 117 makes
/// `oven.lock` that file and states plainly that `incan.lock` is not read afterwards, so there is no compatibility
/// path: a lock left behind by an older toolchain is inert state, not an input.
///
/// It is distinct from the artifact-store coordination lock and from the publication sibling this module derives from
/// a lock's own filename; renaming this constant must not be taken to rename either of those.
pub const LOCK_FILENAME: &str = "oven.lock";
