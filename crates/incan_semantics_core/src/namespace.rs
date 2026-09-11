//! The namespace a declaration belongs to, as distinct from the file that declares it.
//!
//! A module path names where a declaration *is*. A namespace names where a consumer can *reach* it. Those are the
//! same thing only because every source file is currently its own module, and that coincidence is the reason a
//! purely internal reorganisation looks like a change to everyone downstream: move a private helper from
//! `hash/_core.incn` to `hash/_streaming.incn` and its module path changes, so its identity changes, so the unit
//! rebakes and every dependent recompiles — for an edit no consumer could observe.
//!
//! This module supplies the missing distinction. A module whose name marks it internal is not a namespace of its
//! own; it is a detail of the nearest enclosing namespace. Declarations inside it belong to that namespace, and
//! moving one between two internal modules of the same namespace does not move it at all.
//!
//! # Why this is scoping, not flattening
//!
//! The alternative — a single flat namespace with location dropped from identity — does not survive contact with a
//! real standard library. Measured across 1,252 declarations, flattening produces 47 collision classes over 167
//! declarations, and the worst of them are deliberate: `compress` and `decompress` across eight codec modules,
//! `encode` and `decode` across five base-N modules, and the `read` capability across `clock`, `env` and `fs` all
//! have byte-identical signatures, because presenting one interface is the entire point. Nothing left in the
//! identity can separate them.
//!
//! Scoping to the enclosing namespace costs none of that, because every one of those classes is *cross*-namespace.
//! Two declarations colliding *within* one namespace would be a duplicate definition, which the author sees where
//! they wrote it.
//!
//! So a declaration keeps its module path. What this adds is a coarser, reorganisation-stable scope beside it.
//!
//! # The rule, and its limits
//!
//! A path segment marks a module internal when it begins with a single underscore. A leading double underscore is
//! excluded because those names are reserved for compiler-facing modules such as `__init__`, which are structural
//! rather than private.
//!
//! This is a naming convention promoted to a contract, and the standard library already relies on it: `hash/_core`,
//! `hash/_hmac`, `hash/_streaming`, `encoding/_shared`, `compression/_auto`, and `regex/_replacement` are all
//! internal to their parents today. Whether module visibility should eventually be declared outright rather than
//! spelled is a language question this does not settle; if it is, this is the one place the rule has to change.

/// Whether one module path segment names a module internal to its parent namespace.
///
/// A single leading underscore marks a module internal. A double underscore does not: `__init__` and its relatives
/// are structural names the compiler owns, not authors' private modules.
pub fn is_internal_module_segment(segment: &str) -> bool {
    segment.starts_with('_') && !segment.starts_with("__")
}

/// The nearest enclosing namespace a module path belongs to.
///
/// Truncation happens at the *first* internal segment rather than the last, because a public module nested inside an
/// internal one is not reachable either — `hash/_core/detail` is as unreachable as `hash/_core`, so both belong to
/// `hash`. A path with no internal segment is its own namespace and is returned unchanged.
pub fn enclosing_namespace(module_path: &[String]) -> &[String] {
    match module_path
        .iter()
        .position(|segment| is_internal_module_segment(segment))
    {
        Some(index) => &module_path[..index],
        None => module_path,
    }
}

/// Whether a module path is reachable from outside the namespace that encloses it.
///
/// Equivalent to the path being its own namespace. Kept as its own name because callers asking "may a consumer
/// import this?" should not have to phrase it as a comparison.
pub fn is_namespace_root(module_path: &[String]) -> bool {
    !module_path.iter().any(|segment| is_internal_module_segment(segment))
}

#[cfg(test)]
mod tests {
    #![deny(clippy::expect_used, clippy::unwrap_used)]

    use super::*;

    fn path(segments: &[&str]) -> Vec<String> {
        segments.iter().map(|segment| (*segment).to_string()).collect()
    }

    #[test]
    fn a_single_underscore_marks_a_module_internal_and_a_double_one_does_not() {
        assert!(is_internal_module_segment("_core"));
        assert!(is_internal_module_segment("_"));
        assert!(!is_internal_module_segment("__init__"));
        assert!(!is_internal_module_segment("core"));
        assert!(!is_internal_module_segment("core_"));
    }

    #[test]
    fn an_internal_module_belongs_to_its_parent_namespace() {
        assert_eq!(
            enclosing_namespace(&path(&["std", "hash", "_core"])),
            path(&["std", "hash"])
        );
        assert_eq!(
            enclosing_namespace(&path(&["std", "encoding", "_shared"])),
            path(&["std", "encoding"])
        );
    }

    #[test]
    fn a_public_module_is_its_own_namespace() {
        assert_eq!(
            enclosing_namespace(&path(&["std", "encoding", "hex"])),
            path(&["std", "encoding", "hex"])
        );
        assert!(is_namespace_root(&path(&["std", "encoding", "hex"])));
        assert!(!is_namespace_root(&path(&["std", "encoding", "_shared"])));
    }

    #[test]
    fn a_public_module_nested_inside_an_internal_one_is_still_internal() {
        // Truncating at the last internal segment instead of the first would make this `std::hash::_core`, naming a
        // namespace no consumer can reach and reintroducing exactly the instability this exists to remove.
        assert_eq!(
            enclosing_namespace(&path(&["std", "hash", "_core", "detail"])),
            path(&["std", "hash"])
        );
    }

    #[test]
    fn moving_a_declaration_between_internal_modules_does_not_change_its_namespace() {
        // The property the whole module exists for: this is a reorganisation no consumer can observe, and it must
        // not move anything a consumer keys on.
        assert_eq!(
            enclosing_namespace(&path(&["std", "hash", "_core"])),
            enclosing_namespace(&path(&["std", "hash", "_streaming"]))
        );
    }

    #[test]
    fn splitting_a_namespace_into_internal_parts_keeps_the_namespace() {
        // `hash.incn` becoming `hash/_core.incn` plus `hash/_hmac.incn`, with the public surface re-exported.
        let before = path(&["std", "hash"]);
        for after in [path(&["std", "hash", "_core"]), path(&["std", "hash", "_hmac"])] {
            assert_eq!(enclosing_namespace(&after), before.as_slice());
        }
    }

    #[test]
    fn an_internal_module_at_the_root_belongs_to_the_root_namespace() {
        assert_eq!(
            enclosing_namespace(&path(&["_private"])),
            Vec::<String>::new().as_slice()
        );
    }
}
