//! Actual checker entry counts for the declaration/body/default boundary. These are activation-local observations,
//! not a cache, trace channel or authority for deciding which declarations must be checked.

/// Work performed since the current program check started. Imports prepared before the program reset are excluded;
/// caller finalization and ordinary body checking have distinct counters so a future record adapter cannot hide a
/// second local check inside deferred settlement.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct CheckWork {
    /// Named concrete callable local checks, counted independently from deferred settlement.
    pub body_local_checks: usize,
    /// Declaration/default/initializer producers entered separately from statement bodies.
    pub declaration_local_checks: usize,
    /// Declaration outputs finalized against their own pending decisions.
    pub declaration_finalizations: usize,
    /// Completed declaration/default results reused independently from their statement bodies.
    pub declaration_view_reuses: usize,
    /// Top-level function declaration-validation entries, including functions with invalid or unreachable bodies.
    pub function_declarations: usize,
    /// Method declaration-validation entries, including abstract declarations without a body.
    pub method_declarations: usize,
    /// Computed-property receiver and return validation entries, separate from statement checking.
    pub property_declarations: usize,
    /// Computed-property statement entries; abstract properties do not contribute.
    pub property_bodies: usize,
    /// Top-level function body-check entries.
    pub function_bodies: usize,
    /// Method body-check entries; abstract declarations do not contribute.
    pub method_bodies: usize,
    /// Callable parameter default expressions passed to the existing expression checker. Field defaults are separate.
    pub callable_defaults: usize,
    /// Model and class field defaults checked under their existing nominal declaration rules.
    pub field_defaults: usize,
    /// Module mutation-argument settlement entries; this counts settlement, not local body checks.
    pub mutation_settlements: usize,
    /// Forwarding SCCs closed by the effect producer.
    pub mutation_components: usize,
    /// Parameter members visited while closing effect SCCs.
    pub mutation_member_visits: usize,
    /// Local outputs finalized against immutable callee effects.
    pub body_finalizations: usize,
    /// Completed sibling values reused without local checking or finalization.
    pub body_view_reuses: usize,
    /// Recorded mutation arguments, also defining their legacy traversal order.
    pub mutation_arguments: usize,
    /// Recorded dictionary lookups, also defining their legacy traversal order.
    pub dictionary_lookups: usize,
    /// Deferred dictionary refusal passes.
    pub dictionary_settlements: usize,
    /// Settled diagnostics stored as anchored semantic values.
    pub stored_diagnostics: usize,
    /// Stored source ranges anchored inside a node at node-relative offsets rather than at node boundaries.
    pub node_relative_anchors: usize,
    /// Stored source ranges outside every declaration, which keep their coordinates.
    pub detached_anchors: usize,
    /// Deferred lexical open-value destination passes.
    pub open_value_settlements: usize,
}
