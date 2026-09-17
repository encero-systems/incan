#[cfg(feature = "admission")]
use native_consumer_core::Admission;

#[cfg(feature = "admission")]
/// Attempt the former public all-fields constructor surface for a private-required model.
pub fn forge_private_required_model() {
    let _ = Admission(false);
}

#[cfg(feature = "mixed")]
use native_consumer_core::Mixed;

#[cfg(feature = "mixed")]
/// Attempt the former public all-fields constructor surface for a mixed-visibility model.
pub fn forge_mixed_model() {
    let _ = Mixed("forged".to_string(), false);
}

#[cfg(feature = "defaulted")]
use native_consumer_core::Defaulted;

#[cfg(feature = "defaulted")]
/// Attempt to override a default-backed private input on the public provider bridge.
pub fn forge_defaulted_private_field() {
    let _ = Defaulted(false, "forged".to_string());
}
