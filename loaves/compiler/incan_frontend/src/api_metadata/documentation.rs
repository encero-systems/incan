//! Documentation kept apart from checked API compile facts (RFC 179).

use serde::{Deserialize, Serialize};

use super::{ApiDocstring, CheckedApiMetadataPackage};
use crate::library_manifest::LibraryContractMetadata;

/// The documentation of one checked API declaration or method, kept apart from its compile facts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiDocumentation {
    /// Position of the documented item among the package's declarations and methods, in declaration order.
    pub position: usize,
    /// Source anchor identity of the documented declaration or method.
    pub anchor: String,
    pub docstring: Option<String>,
    pub docstring_sections: Option<ApiDocstring>,
}

impl CheckedApiMetadataPackage {
    /// Take every docstring out of the package, leaving its compile facts; only documented items are listed.
    pub fn take_documentation(&mut self) -> Vec<ApiDocumentation> {
        Vec::new()
    }

    /// Put documentation taken by [`Self::take_documentation`] back.
    pub fn restore_documentation(&mut self, _documentation: Vec<ApiDocumentation>) -> Result<(), String> {
        Ok(())
    }
}

impl LibraryContractMetadata {
    /// Split into the compile facts and the documentation of their checked API, which compiles never read.
    pub fn split_documentation(&self) -> (Self, Vec<ApiDocumentation>) {
        let mut facts = self.clone();
        let documentation = facts
            .api
            .as_mut()
            .map(CheckedApiMetadataPackage::take_documentation)
            .unwrap_or_default();
        (facts, documentation)
    }

    /// Restore documentation split off by [`Self::split_documentation`].
    pub fn restore_documentation(&mut self, documentation: Vec<ApiDocumentation>) -> Result<(), String> {
        match self.api.as_mut() {
            Some(api) => api.restore_documentation(documentation),
            None if documentation.is_empty() => Ok(()),
            None => Err("documentation names a checked API the contract metadata does not carry".to_owned()),
        }
    }
}
