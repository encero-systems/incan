//! The refusal a construct receives while its lowering is not yet in Rust.
//!
//! Every call names the Incan module and function it replaces, so the corpus replay shows exactly which translation
//! a mismatched case needs next.

/// The refusal for one untranslated step of the lowering.
pub fn untranslated(step: &str) -> String {
    format!("unsupported Body IR construct awaiting the Rust lowering of {step}")
}
