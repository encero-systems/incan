//! Temporary trait-owned capability bridges.
//!
//! RFC 101 needs `std.collections.OrdinalKey` to accept deterministic language types such as `str`, `bytes`, exact
//! integers, decimal, and value enums before RFC 098/099 provide the full source-level conformance-family machinery.
//! This registry keeps that debt explicit and data-driven: the trait contract is still authored in Incan source, while
//! the compiler recognizes a narrow set of proven type families that satisfy that trait contract for v0.3.

use crate::lang::types::numerics::{self, NumericFamily, NumericTypeId};

/// Stable identifier for a temporary trait-owned capability family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TraitCapabilityId {
    StableOrdinalKey,
}

/// Source type categories that can participate in temporary trait-owned capability bridges.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TraitCapabilityType {
    Int,
    Bool,
    Str,
    Bytes,
    Numeric(NumericTypeId),
    Decimal,
    ValueEnum,
}

/// A reusable set of types accepted by one temporary capability family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TraitCapabilityTypeSet {
    DeterministicOrdinalKeys,
}

/// Bridge-only helper hooks for a temporary trait-owned capability family.
#[derive(Debug, Clone, Copy)]
pub struct TraitCapabilityBridgeHooks {
    /// Optional hot-path hash method exposed by the source trait.
    pub hash_method: &'static str,
    /// Optional hot-path exact-byte comparison method exposed by the source trait.
    pub bytes_equal_method: &'static str,
    /// Helper function used by default source methods that need a qualified path after import.
    pub default_hash_helper: &'static str,
}

/// Metadata for one temporary trait-owned capability family.
#[derive(Debug, Clone, Copy)]
pub struct TraitCapabilityInfo {
    /// Stable identity for dispatching temporary bridge-specific backend behavior.
    pub id: TraitCapabilityId,
    /// Canonical module path that owns the source trait contract.
    pub module_path: &'static [&'static str],
    /// Source trait name that owns the capability contract.
    pub trait_name: &'static str,
    /// Required trait methods that must be present before the bridge is allowed to attach to a source trait.
    pub required_methods: &'static [&'static str],
    /// Imported item names that make downstream modules need generated support for this capability family.
    pub import_trigger_items: &'static [&'static str],
    /// Registry-backed family of source types accepted by this temporary capability.
    pub type_set: TraitCapabilityTypeSet,
    /// Optional bridge hooks for capability-specific default/hot-path methods.
    pub bridge_hooks: Option<TraitCapabilityBridgeHooks>,
}

/// Registry of source-authored trait contracts with temporary capability bridges.
pub const TRAIT_CAPABILITIES: &[TraitCapabilityInfo] = &[TraitCapabilityInfo {
    id: TraitCapabilityId::StableOrdinalKey,
    module_path: &["std", "collections"],
    trait_name: "OrdinalKey",
    required_methods: &["ordinal_bytes", "ordinal_encoding", "from_ordinal_bytes"],
    import_trigger_items: &["OrdinalMap", "OrdinalKey"],
    type_set: TraitCapabilityTypeSet::DeterministicOrdinalKeys,
    bridge_hooks: Some(TraitCapabilityBridgeHooks {
        hash_method: "ordinal_hash",
        bytes_equal_method: "ordinal_bytes_equal",
        default_hash_helper: "_ordinal_hash",
    }),
}];

/// Return the temporary `std.collections.OrdinalKey` capability bridge.
pub fn stable_ordinal_key() -> &'static TraitCapabilityInfo {
    &TRAIT_CAPABILITIES[0]
}

/// Return the registry entry for a trait path, if it has a temporary capability bridge.
pub fn for_trait_path(module_path: &[String], trait_name: &str) -> Option<&'static TraitCapabilityInfo> {
    TRAIT_CAPABILITIES
        .iter()
        .find(|info| info.trait_name == trait_name && module_path_matches(info, module_path))
}

/// Return whether a source import path matches the module that owns a capability contract.
pub fn module_path_matches(info: &TraitCapabilityInfo, module_path: &[String]) -> bool {
    module_path.len() == info.module_path.len()
        && module_path
            .iter()
            .map(String::as_str)
            .zip(info.module_path.iter().copied())
            .all(|(left, right)| left == right)
}

/// Return whether importing one item triggers downstream generated support for the capability.
pub fn import_triggers_capability(info: &TraitCapabilityInfo, item_name: &str) -> bool {
    info.import_trigger_items.contains(&item_name)
}

/// Return whether one registered capability bridge supports the supplied source type category.
pub fn supports_type(info: &TraitCapabilityInfo, ty: TraitCapabilityType) -> bool {
    match info.type_set {
        TraitCapabilityTypeSet::DeterministicOrdinalKeys => match ty {
            TraitCapabilityType::Int
            | TraitCapabilityType::Bool
            | TraitCapabilityType::Str
            | TraitCapabilityType::Bytes
            | TraitCapabilityType::Decimal
            | TraitCapabilityType::ValueEnum => true,
            TraitCapabilityType::Numeric(id) => {
                let info = numerics::info_for(id);
                info.bit_width.is_some()
                    && matches!(
                        info.family,
                        NumericFamily::SignedInteger | NumericFamily::UnsignedInteger
                    )
            }
        },
    }
}
