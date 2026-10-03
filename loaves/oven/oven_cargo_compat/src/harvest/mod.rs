//! Harvest: what the compatibility publisher observed for each registry unit, written as incan.pub proposals.
//!
//! RFC 119 makes adoption a harvest, not a hand edit: a crates.io package gains its declared build facts by running
//! one coordinated Cargo build for the selected closure on the publisher's machine and reading Cargo's build-script
//! output records into canonical proposals. The capture that build leaves behind
//! (`OvenLegacyCargoSelectedUnitCapture`) is the observation; this module turns it into one proposal per registry
//! package, version and exact selection, and one refusal per unit whose observation cannot be proven or retained.
//! Admitting a proposal is `incan-pub add-fact`'s job, in a separate, reviewable step; nothing here writes into a
//! registry.
//!
//! Every fact proposed here is one `LoafRegistryAuthority::resolve` will later compare with a fresh observation
//! (`check_observation`), so the shape emitted must be the shape the reader compares: `features` and `cfg` sorted and
//! unique, `out.name` exactly the retained member path, `out.path` relative to the proposal file.

mod model;
mod native;
mod observe;
mod output;
mod tool;

pub use model::*;
pub use model::{ambient_harvest_hazards, harvest_notes_for_checkout};
pub(crate) use native::attach_native_link_records;
pub use observe::harvest_registry_units;
pub use output::{
    canonical_proposal_bytes, canonical_refusals_bytes, harvest_refusals_file_name, harvest_registry_units_to_dir,
    proposal_directory_names, write_harvest_report,
};

#[cfg(test)]
use model::link_record_from_observation;
#[cfg(test)]
use native::{NativeCompiler, native_link_work_from_observations, stable_object_name};
#[cfg(test)]
use observe::admission_refusal;
#[cfg(test)]
use tool::tool_record_from_observation;

#[cfg(test)]
mod tests;
