//! The evidence of per-declaration reuse: which keyed frames a lowering call restored or lowered, read back from the
//! tracing channel, and which bodies differ between two captures of the same unit.

use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;

use incan_semantics_core::CompilerNodeId;
use incan_semantics_core::body_ir::{Body, BodyIrModule};

use crate::classes::body_module_identity;

/// Reads the lowering's frame records this process appended to the tracing channel since the last read.
pub struct FrameTrace {
    path: PathBuf,
    offset: u64,
}

/// The keyed frames one call restored from their products, lowered and kept, or lowered without a product: ordinary
/// frames by `module::name`, instances by plan function name.
#[derive(Default)]
pub struct FrameOutcomes {
    pub reused: Vec<String>,
    pub lowered: Vec<String>,
    /// Frames lowered whose product could not be kept, so a later call lowers them again.
    pub unkept: Vec<String>,
}

impl FrameTrace {
    /// The channel file of this process, when `INCAN_ENGINE_TRACE` names a directory.
    pub fn open() -> Option<Self> {
        let directory = std::env::var("INCAN_ENGINE_TRACE")
            .ok()
            .filter(|value| !value.is_empty())?;
        let path = PathBuf::from(directory).join(format!("engine-compiler-{}.jsonl", std::process::id()));
        let offset = std::fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
        Some(Self { path, offset })
    }

    /// The frame records appended since the last read.
    pub fn take(&mut self) -> FrameOutcomes {
        let mut outcomes = FrameOutcomes::default();
        let Ok(mut file) = std::fs::File::open(&self.path) else {
            return outcomes;
        };
        let mut text = String::new();
        if file.seek(SeekFrom::Start(self.offset)).is_err() || file.read_to_string(&mut text).is_err() {
            return outcomes;
        }
        self.offset += text.len() as u64;
        for line in text.lines() {
            let Ok(record) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            if record.get("operation").and_then(|value| value.as_str()) != Some("lowering_product") {
                continue;
            }
            let Some(key) = record.get("key").and_then(|value| value.as_str()) else {
                continue;
            };
            let mut parts = key.splitn(3, ' ');
            let (Some(outcome), Some(kind), Some(name)) = (parts.next(), parts.next(), parts.next()) else {
                continue;
            };
            let entry = format!("{kind} {name}");
            match outcome {
                "reused" => outcomes.reused.push(entry),
                "lowered" => outcomes.lowered.push(entry),
                "unkept" => outcomes.unkept.push(entry),
                _ => {}
            }
        }
        outcomes
    }
}

/// One body of the edited capture whose checked input differs from the base capture's body at the same place.
pub struct BodyChange {
    /// The body's `module::name`, as the lowering traces it.
    pub function: String,
    pub module: String,
    /// The body's source start in the base and edited captures; `None` for a body the base does not have.
    pub base_start: Option<usize>,
    pub edited_start: usize,
    /// Whether the Body IR itself differs, as opposed to only the checker's witness.
    pub body_differs: bool,
}

/// The bodies of `edited` whose Body IR or checker witness differs from the matching body of `base`.
///
/// Bodies match by module, source name and their ordinal among same-named bodies of that module.
pub fn changed_bodies(
    base: &[BodyIrModule],
    base_witnesses: &[(CompilerNodeId, String)],
    edited: &[BodyIrModule],
    edited_witnesses: &[(CompilerNodeId, String)],
) -> Vec<BodyChange> {
    let base_witnesses: BTreeMap<_, _> = base_witnesses.iter().cloned().collect();
    let edited_witnesses: BTreeMap<_, _> = edited_witnesses.iter().cloned().collect();
    let mut base_bodies: BTreeMap<(String, String, usize), &Body> = BTreeMap::new();
    for module in base {
        for (key, body) in keyed_bodies(module) {
            base_bodies.insert(key, body);
        }
    }
    let mut changes = Vec::new();
    for module in edited {
        for (key, body) in keyed_bodies(module) {
            let matched = base_bodies.get(&key).copied();
            let body_differs = match matched {
                Some(before) => postcard::to_allocvec(before).ok() != postcard::to_allocvec(body).ok(),
                None => true,
            };
            let witness_differs = matched.is_some_and(|before| {
                base_witnesses.get(&before.direct_call_id) != edited_witnesses.get(&body.direct_call_id)
            });
            if body_differs || witness_differs {
                changes.push(BodyChange {
                    function: format!("{}::{}", body_module_identity(body).unwrap_or_default(), body.name),
                    module: key.0.clone(),
                    base_start: matched.map(|before| before.span.start),
                    edited_start: body.span.start,
                    body_differs,
                });
            }
        }
    }
    changes
}

fn keyed_bodies(module: &BodyIrModule) -> Vec<((String, String, usize), &Body)> {
    let path = module.module_id.path();
    let mut ordinals: BTreeMap<&str, usize> = BTreeMap::new();
    let mut keyed = Vec::with_capacity(module.bodies.len());
    for body in &module.bodies {
        let ordinal = ordinals.entry(body.name.as_str()).or_default();
        keyed.push(((path.to_string(), body.name.clone(), *ordinal), body));
        *ordinal += 1;
    }
    keyed
}
