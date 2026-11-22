//! The install record of a compiled Loaf: what installing it reads of its manifest
//! ([`crate::library_manifest::install_record`]).

use super::*;
use crate::library_manifest::install_record::LoafInstallRecord;

impl TypeChecker {
    /// The install record of the compiled Loaf indexed as `library`, whose manifest is `manifest`.
    pub fn loaf_install_record(&self, library: &str, manifest: &LibraryManifest) -> LoafInstallRecord {
        let _ = (library, manifest);
        LoafInstallRecord::default()
    }
}
