//! Canonical Crumb envelopes shared by the graph publisher and the selected compiler worker.

use crate::crumb_rows::{ModelField, model_description, required_value};
use crate::graph_crumb_fields::{
    KEY_TYPE, REQUEST_TYPE, decode_graph_row, encode_graph_row, integer, items, key_value, number, read_key,
    read_request, request_value, text,
};
use crate::record_service::{
    DemandPage, InputSnapshot, PreparedInputs, ProducerProfile, ProducerRank, ProducerStage, RankCertificate,
    ReadyWork, RecordRoot,
};
use internal_models_bindings::{Description, Value};

pub const DEMAND_SCHEMA: &str = "incan.engine.demand-page/2";
pub const PREPARED_SCHEMA: &str = "incan.engine.prepared-inputs/2";
pub const WORK_SCHEMA: &str = "incan.engine.ready-work/2";
pub const DEMAND_FAMILY: &str = "incan.engine.demand-page";
pub const PREPARED_FAMILY: &str = "incan.engine.prepared-inputs";
pub const WORK_FAMILY: &str = "incan.engine.ready-work";

const DEMAND: &[u8] = b"demand\x00";
const PREPARED: &[u8] = b"prepar\x00";
const READY: &[u8] = b"ready_\x00";

/// The exact sequence of demanded logical keys.
fn demand_description() -> Description {
    let storage = format!("list<{KEY_TYPE}>");
    model_description(DEMAND, DEMAND_SCHEMA, &[ModelField::new(b"keys__", "keys", &storage)])
}

/// Write one typed demand page without assigning producer meaning to its keys.
pub fn encode_demand_page(value: &DemandPage) -> Result<Vec<u8>, String> {
    let keys = Value::List(value.keys.iter().map(key_value).collect());
    encode_graph_row(DEMAND, &demand_description(), vec![required_value(b"keys__", keys)])
}

/// Decode every logical key under the exact demand-page schema.
pub fn decode_demand_page(encoded: &[u8]) -> Result<DemandPage, String> {
    let fields = decode_graph_row(encoded, DEMAND, &demand_description(), 1)?;
    let keys = items(&fields[0].value)?
        .iter()
        .map(read_key)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(DemandPage { keys })
}

/// Separate the output, authority, request sequence, rank and selector evidence into independently typed fields.
fn prepared_description() -> Description {
    let reads = format!("list<{REQUEST_TYPE}>");
    model_description(
        PREPARED,
        PREPARED_SCHEMA,
        &[
            ModelField::new(b"dep___", "rank_dependency", "i64"),
            ModelField::new(b"eviden", "rank_evidence", "string"),
            ModelField::new(b"namesp", "rank_namespace", "i64"),
            ModelField::new(b"output", "output", KEY_TYPE),
            ModelField::new(b"profil", "profile", "string"),
            ModelField::new(b"reads_", "required_reads", &reads),
            ModelField::new(b"select", "selector_witnesses", "string"),
            ModelField::new(b"snapsh", "snapshot", "string"),
            ModelField::new(b"stage_", "rank_stage", "string"),
        ],
    )
}

/// Encode the certified rank and read requests of one prepared graph activation.
pub fn encode_prepared_inputs(value: &PreparedInputs) -> Result<Vec<u8>, String> {
    let rank = &value.rank_certificate.rank;
    let reads = Value::List(value.required_reads.iter().map(request_value).collect());
    encode_graph_row(
        PREPARED,
        &prepared_description(),
        vec![
            required_value(b"dep___", integer(rank.dependency)),
            required_value(b"eviden", Value::Text(value.rank_certificate.evidence.digest.clone())),
            required_value(b"namesp", integer(rank.namespace)),
            required_value(b"output", key_value(&value.output)),
            required_value(b"profil", Value::Text(value.profile.identity.digest.clone())),
            required_value(b"reads_", reads),
            required_value(b"select", Value::Text(value.selector_witnesses.digest.clone())),
            required_value(b"snapsh", Value::Text(value.snapshot.root.digest.clone())),
            required_value(b"stage_", Value::Text(stage_name(&rank.stage))),
        ],
    )
}

/// Restore exact prepared work without resolving or evaluating its requested records.
pub fn decode_prepared_inputs(encoded: &[u8]) -> Result<PreparedInputs, String> {
    let fields = decode_graph_row(encoded, PREPARED, &prepared_description(), 9)?;
    let requests = items(&fields[5].value)?
        .iter()
        .map(read_request)
        .collect::<Result<Vec<_>, _>>()?;
    let rank = ProducerRank {
        namespace: number(&fields[2].value)?,
        stage: read_stage(&text(&fields[8].value)?)?,
        dependency: number(&fields[0].value)?,
    };
    let root = |index: usize| -> Result<RecordRoot, String> {
        Ok(RecordRoot {
            digest: text(&fields[index].value)?,
        })
    };
    Ok(PreparedInputs {
        output: read_key(&fields[3].value)?,
        snapshot: InputSnapshot { root: root(7)? },
        profile: ProducerProfile { identity: root(4)? },
        required_reads: requests,
        rank_certificate: RankCertificate {
            rank,
            evidence: root(1)?,
        },
        selector_witnesses: root(6)?,
    })
}

/// The output and producer selected to perform ready work.
fn work_description() -> Description {
    model_description(
        READY,
        WORK_SCHEMA,
        &[
            ModelField::new(b"output", "output", KEY_TYPE),
            ModelField::new(b"profil", "profile", "string"),
            ModelField::new(b"stage_", "stage", "string"),
        ],
    )
}

/// Encode a selected output, producer profile and stage as one Crumb record.
pub fn encode_ready_work(value: &ReadyWork) -> Result<Vec<u8>, String> {
    encode_graph_row(
        READY,
        &work_description(),
        vec![
            required_value(b"output", key_value(&value.output)),
            required_value(b"profil", Value::Text(value.profile.identity.digest.clone())),
            required_value(b"stage_", Value::Text(stage_name(&value.stage))),
        ],
    )
}

/// Reject a foreign selector, field type or unknown stage.
pub fn decode_ready_work(encoded: &[u8]) -> Result<ReadyWork, String> {
    let fields = decode_graph_row(encoded, READY, &work_description(), 3)?;
    Ok(ReadyWork {
        output: read_key(&fields[0].value)?,
        profile: ProducerProfile {
            identity: RecordRoot {
                digest: text(&fields[1].value)?,
            },
        },
        stage: read_stage(&text(&fields[2].value)?)?,
    })
}

/// Resolve one content root only inside its engine-owned envelope family.
pub fn envelope_key(family: &str, root: &str) -> Result<String, String> {
    let supported = family == DEMAND_FAMILY || family == PREPARED_FAMILY || family == WORK_FAMILY;
    if !supported || !valid_digest(root) {
        return Err("invalid compiler activation envelope root".into());
    }
    Ok(format!("{family}\t{root}"))
}

/// Encode the closed producer-stage set independently of enum ordinals.
pub fn stage_name(stage: &ProducerStage) -> String {
    match stage {
        ProducerStage::Discover => "discover",
        ProducerStage::Signature => "signature",
        ProducerStage::Layout => "layout",
        ProducerStage::Default => "default",
        ProducerStage::CheckBody => "check-body",
        ProducerStage::BodyIr => "body-ir",
        ProducerStage::Instance => "instance",
        ProducerStage::LoweringDemand => "lowering-demand",
        ProducerStage::PlanFunction => "plan-function",
        ProducerStage::NativeInterface => "native-interface",
    }
    .to_owned()
}

/// Decode the closed producer-stage spelling without silently widening the current profile.
fn read_stage(value: &str) -> Result<ProducerStage, String> {
    Ok(match value {
        "discover" => ProducerStage::Discover,
        "signature" => ProducerStage::Signature,
        "layout" => ProducerStage::Layout,
        "default" => ProducerStage::Default,
        "check-body" => ProducerStage::CheckBody,
        "body-ir" => ProducerStage::BodyIr,
        "instance" => ProducerStage::Instance,
        "lowering-demand" => ProducerStage::LoweringDemand,
        "plan-function" => ProducerStage::PlanFunction,
        "native-interface" => ProducerStage::NativeInterface,
        _ => return Err("unknown compiler producer stage".into()),
    })
}

/// Whether one root is a lowercase SHA-256 digest.
fn valid_digest(value: &str) -> bool {
    value.chars().count() == 64 && value.chars().all(|character| "0123456789abcdef".contains(character))
}
