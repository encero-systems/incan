//! The checker's side of the compiler's record reads: the same three outcomes the Incan lowering's `records` module
//! answers, over the Loaf index the session already owns. It decides nothing; it maps one lookup to one outcome and
//! keeps the witness.

use std::sync::{Arc, Mutex};

use incan_lang::interop::RustItemMetadata;

use crate::loaf_index::{LoafIndex, LoafIndexError};

/// The family of a Rust ABI record, keyed by its canonical Rust path.
pub const RUST_ABI: &str = "rust-abi";

/// What one read found.
pub enum RecordRead<T> {
    Found(T),
    Absent,
    Failed(LoafIndexError),
}

/// One read and whether it found a record; an absent read is a dependency too.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadWitness {
    pub family: &'static str,
    pub identity: String,
    pub found: bool,
}

/// The reads one compilation made through this adapter.
#[derive(Default)]
pub struct RecordReads {
    witnesses: Mutex<Vec<ReadWitness>>,
}

impl RecordReads {
    /// Read one Rust ABI record by canonical path and keep the witness.
    pub fn rust_abi(&self, loaves: &LoafIndex, path: &str) -> RecordRead<Arc<RustItemMetadata>> {
        let read = match loaves.rust_abi_item(path) {
            Some(item) => RecordRead::Found(item),
            None => match loaves.validate_demands() {
                Ok(()) => RecordRead::Absent,
                Err(error) => RecordRead::Failed(error),
            },
        };
        let found = matches!(read, RecordRead::Found(_));
        if let Ok(mut witnesses) = self.witnesses.lock() {
            witnesses.push(ReadWitness {
                family: RUST_ABI,
                identity: path.to_owned(),
                found,
            });
        }
        read
    }

    /// Every witness so far, in read order.
    pub fn witnesses(&self) -> Vec<ReadWitness> {
        self.witnesses
            .lock()
            .map(|witnesses| witnesses.clone())
            .unwrap_or_default()
    }
}
