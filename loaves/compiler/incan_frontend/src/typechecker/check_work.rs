//! Actual checker entry counts for the declaration/body/default boundary. These are activation-local observations,
//! not a cache, trace channel or authority for deciding which declarations must be checked.

/// Work performed since the current program check started. Imports prepared before the program reset are excluded;
/// caller finalization and ordinary body checking have distinct counters so a future record adapter cannot hide a
/// second local check inside deferred settlement.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct CheckWork {
    /// Named concrete callable local checks, counted independently from deferred settlement.
    pub body_local_checks: usize,
    /// Top-level function declaration-validation entries, including functions with invalid or unreachable bodies.
    pub function_declarations: usize,
    /// Method declaration-validation entries, including abstract declarations without a body.
    pub method_declarations: usize,
    /// Top-level function body-check entries.
    pub function_bodies: usize,
    /// Method body-check entries; abstract declarations do not contribute.
    pub method_bodies: usize,
    /// Callable parameter default expressions passed to the existing expression checker. Field defaults are separate.
    pub callable_defaults: usize,
    /// Module mutation-argument settlement entries; this counts settlement, not local body checks.
    pub mutation_settlements: usize,
    /// Deferred dictionary refusal passes.
    pub dictionary_settlements: usize,
    /// Deferred lexical open-value destination passes.
    pub open_value_settlements: usize,
}
