//! Global compiler contributors for the exact installation profile and package roster admitted to a source unit.

/// Only packages contributing parser vocabulary or provider operations. `vocabulary` and `operations` default to
/// empty.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompilerContributors {
    pub profile: String,
    pub scope: String,
    pub vocabulary: Vec<String>,
    pub operations: Vec<String>,
}
