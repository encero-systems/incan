//! The provider contract the checked program is typed against: the immutable compiled-provider catalog and its
//! active projection, the SDK inventory and its descriptors, and package feature plans. Loading, building and
//! publishing providers is the provider crate's; this half only describes them.

pub mod error;
mod features;
mod plan;
mod sdk;

pub use features::*;
pub use plan::*;
pub use sdk::*;
