//! A struct whose optional field holds another struct of this crate.

/// How deep a badge is scoped.
#[derive(Debug, Clone, PartialEq)]
pub struct Scope {
    /// The nesting depth.
    pub depth: i64,
}

/// A badge, scoped or not.
#[derive(Debug, Clone, PartialEq)]
pub struct Badge {
    /// The badge number.
    pub id: i64,
    /// The scope, when the badge has one.
    pub scope: Option<Scope>,
}

/// A badge ranked at a level another crate defines.
pub struct Ranked {
    /// The badge number.
    pub id: i64,
    /// The level, a type of another crate.
    pub level: levels::Level,
}
