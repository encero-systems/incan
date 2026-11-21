//! The evidence of per-declaration reuse: which keyed frames a lowering call restored or lowered, read back from the
//! tracing channel, and which bodies differ between two captures of the same unit.

use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;

use incan_mir_plan_native::decode_plan_function_record;
use incan_semantics_core::CompilerNodeId;
use incan_semantics_core::body_ir::{Body, BodyIrModule};

use crate::SourceInputs;
use crate::checked_values::location;
use crate::classes::body_module_identity;
use crate::lowering::{PlanContexts, plan_contexts, source_file_index};
use crate::native_type_identity::constructor_index_with_packages as index_constructors;
use crate::plan_product_inputs::{ordinary_product_inputs, producer_generation};
use crate::program_entries::{no_emitted_frames, ordinary_body};
use crate::record_positions::{indexed_product, positioned_product};
use crate::scalar_store::scalar_path;

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

/// What the records-key check found for one records call: the bodies it keyed, the keys whose product the call's
/// directory holds, and the stored products that survive placing and re-indexing at this request's positions.
#[derive(Default)]
pub struct RecordsKeys {
    pub keyed: usize,
    pub stored: usize,
    pub round_trips: usize,
    pub failures: Vec<String>,
    /// Each stored product's body as `module::name` and its product file name.
    pub stored_products: Vec<(String, String)>,
}

/// Key every ordinary body of a records call as the lowering keys it, and check each stored product against its key.
///
/// The context, owners and ordinary-body selection are prepared as `unit_plan` prepares them. A product is checked by
/// reading it under the key computed here (so its file name proves the key, body position count included), placing its
/// node ordinals at this body's positions and indexing it again, which must give back the stored bytes.
pub fn check_records_keys(
    modules: &[BodyIrModule],
    inputs: &SourceInputs,
    entry: i64,
    kind: &str,
    products: &str,
) -> Result<RecordsKeys, String> {
    if entry < 0 || entry as usize >= modules.len() {
        return Err("invalid Body IR project entry".into());
    }
    let source_files = source_file_index(modules, &inputs.files)?;
    let empty_templates = index_constructors(&[], &source_files, &inputs.package_versions);
    let PlanContexts { owners, context, .. } = plan_contexts(
        modules,
        &inputs.sources,
        &inputs.files,
        entry,
        false,
        &source_files,
        &inputs.package_versions,
        &modules[entry as usize],
        &empty_templates,
    )?;
    let admitted: BTreeMap<CompilerNodeId, String> = inputs.scalar_bodies.iter().cloned().collect();
    let generation = producer_generation(Some(products));
    let mut module_bodies: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (position, body) in context.bodies.iter().enumerate() {
        module_bodies
            .entry(body_module_identity(body)?)
            .or_default()
            .push(position);
    }
    let mut emitted = no_emitted_frames();
    let mut report = RecordsKeys::default();
    for (index, owner) in owners.iter().enumerate() {
        for position in module_bodies.get(owner).cloned().unwrap_or_default() {
            let body = &context.bodies[position];
            if !ordinary_body(&context, body, &emitted, kind) {
                continue;
            }
            emitted.record(body, position);
            let frame = (&inputs.sources[index], &inputs.files[index]);
            let keyed = ordinary_product_inputs(
                &context,
                body,
                frame.0,
                frame.1,
                inputs.overflow_checks,
                Some(products),
                &admitted,
                true,
                generation.as_deref(),
                None,
            )?;
            if let Some((key, positions)) = keyed {
                report.keyed += 1;
                let label = format!("{}::{}", body_module_identity(body).unwrap_or_default(), body.name);
                check_stored_product(
                    products,
                    (&label, &key),
                    &positions,
                    &location(&body.span, frame.0, frame.1),
                    &mut report,
                );
            }
        }
    }
    Ok(report)
}

/// Read one product under its key, then place and re-index it at the body's positions.
fn check_stored_product(
    products: &str,
    (label, key): (&str, &[String]),
    positions: &[(i64, i64)],
    owner: &incan_mir_plan_native::SourceSpan,
    report: &mut RecordsKeys,
) {
    let Ok(path) = scalar_path(products, key) else {
        return;
    };
    let Ok(encoded) = std::fs::read(&path) else {
        return;
    };
    report.stored += 1;
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    report.stored_products.push((label.to_owned(), file_name));
    let file = owner.file.to_string();
    let checked = decode_plan_function_record(&encoded).and_then(|record| {
        if record.inputs != key {
            return Err("stored inputs differ from the key".to_owned());
        }
        let placed = positioned_product(&record.function, positions, &file, owner.line, &mut Vec::new())?;
        let indexed = indexed_product(&placed, positions, &file, owner.line)?;
        if indexed == record.function {
            Ok(())
        } else {
            Err("placing and re-indexing changed the stored product".to_owned())
        }
    });
    match checked {
        Ok(()) => report.round_trips += 1,
        Err(reason) => report.failures.push(format!("{}: {reason}", path.display())),
    }
}
