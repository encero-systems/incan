//! Query spelling helpers; dependency and source selection belong to Oven.

/// Return the first path segment that identifies the crate for a canonical path.
pub(crate) fn crate_name_for_path(canonical_path: &str) -> &str {
    canonical_path.split("::").next().unwrap_or(canonical_path)
}
