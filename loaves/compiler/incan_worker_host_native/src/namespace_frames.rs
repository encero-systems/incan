//! Resolve a leased namespace frame through the store, which authenticates the frame against its selected digest.

use std::collections::HashMap;

use incan_worker_host_bindings::{
    Location, Lookup, MappedFile, MappedView, disk_root, read_frame_cached, store_error_text,
};
use internal_models_native::namespace_references::NamespaceReference;
use internal_models_native::records::encode_record_key;

/// One compiler request's Bakery anchor and its retained segment mappings.
pub struct NamespaceFrameReader {
    directory: String,
    mappings: HashMap<String, MappedFile>,
}

impl NamespaceFrameReader {
    /// An initially unmapped reader for an Oven-leased Bakery root.
    pub fn create(directory: &str) -> Self {
        Self {
            directory: directory.to_owned(),
            mappings: HashMap::new(),
        }
    }

    /// Open one previously admitted frame reference beneath the compiler's Bakery anchor.
    pub fn read(&mut self, reference: &NamespaceReference) -> Result<MappedView, String> {
        let key = encode_record_key(&reference.key)?;
        if reference.segment.len() != 64 {
            return Err("namespace frame segment has an invalid length".to_owned());
        }
        if !reference
            .segment
            .bytes()
            .all(|digit| matches!(digit, b'0'..=b'9' | b'a'..=b'f'))
        {
            return Err("namespace frame segment has an invalid name".to_owned());
        }
        let location = Location {
            segment: reference.segment.clone(),
            offset: reference.offset,
            length: reference.length,
            digest: reference.frame_digest.clone(),
            tombstone: false,
        };
        let root = disk_root(self.directory.clone());
        // The store verifies the whole frame, key and payload, against the frame digest Oven selected. That one
        // verification authenticates the payload, so it is not hashed a second time for its evaluation digest.
        match read_frame_cached(&root, key, &location, &mut self.mappings).map_err(|error| store_error_text(&error))? {
            Lookup::Found(view) => Ok(view),
            _ => Err("selected namespace frame is missing".to_owned()),
        }
    }
}

impl std::fmt::Debug for NamespaceFrameReader {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("NamespaceFrameReader")
            .field("directory", &self.directory)
            .field("mapped_segments", &self.mappings.len())
            .finish()
    }
}

/// The request-local native reader.
pub fn namespace_frame_reader(directory: &str) -> NamespaceFrameReader {
    NamespaceFrameReader::create(directory)
}

/// Read one frame through the reader's existing segment mappings.
pub fn read_namespace_frame(
    reader: &mut NamespaceFrameReader,
    reference: &NamespaceReference,
) -> Result<MappedView, String> {
    reader.read(reference)
}
