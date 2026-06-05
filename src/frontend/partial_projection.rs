//! Shared helpers for partial callable projection.
//!
//! A partial preset is a callable projection, not an ordinary wrapper with independent semantics. Frontend checks and
//! lowering both need the same argument merge rule: preset keywords behave like defaults, and explicit call-site
//! keywords override them.

use incan_syntax::ast::{CallArg, Expr, Spanned};

/// Borrowed preset argument used by partial projection helpers.
pub(crate) struct PartialPresetRef<'a> {
    /// Keyword supplied by the partial declaration.
    pub(crate) name: &'a str,
    /// Value supplied by the partial declaration.
    pub(crate) value: &'a Spanned<Expr>,
}

/// Merge partial preset keywords with call-site arguments using the runtime partial-call rule.
///
/// Returns `None` for positional or unpacked call-site arguments because projection expansion needs an explicit keyword
/// map. Runtime wrapper calls use the same merge rule before Rust emission because generated Rust has no source-level
/// default parameters.
pub(crate) fn merge_named_partial_args<'a>(
    presets: impl IntoIterator<Item = PartialPresetRef<'a>>,
    call_args: &[CallArg],
) -> Option<Vec<CallArg>> {
    let mut merged: Vec<CallArg> = presets
        .into_iter()
        .map(|preset| CallArg::Named(preset.name.to_string(), preset.value.clone()))
        .collect();

    for arg in call_args {
        let CallArg::Named(name, value) = arg else {
            return None;
        };
        if let Some(existing) = merged.iter_mut().find(|candidate| match candidate {
            CallArg::Named(existing_name, _) => existing_name == name,
            _ => false,
        }) {
            *existing = CallArg::Named(name.clone(), value.clone());
        } else {
            merged.push(CallArg::Named(name.clone(), value.clone()));
        }
    }

    Some(merged)
}
