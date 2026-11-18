//! Canonical JSON activation envelopes shared by the engine and compiler worker.

use crate::record_scope::{MemberSelection, PayloadSelection, RecordKey, RecordRequest, RecordSelection, valid_key};
use crate::record_service::{
    DemandPage, InputSnapshot, PreparedInputs, ProducerProfile, ProducerRank, ProducerStage, RankCertificate,
    ReadyWork, RecordRoot,
};
use internal_models_bindings::NativeJsonValue as JsonValue;
use serde_json::{Value, json};

pub const DEMAND_SCHEMA: &str = "incan.engine.demand-page/1";
pub const PREPARED_SCHEMA: &str = "incan.engine.prepared-inputs/1";
pub const WORK_SCHEMA: &str = "incan.engine.ready-work/1";
pub const DEMAND_FAMILY: &str = "incan.engine.demand-page";
pub const PREPARED_FAMILY: &str = "incan.engine.prepared-inputs";
pub const WORK_FAMILY: &str = "incan.engine.ready-work";

/// Use the installed JSON number and syntax rules once; borrow fields from the resulting tree.
pub(crate) fn document(bytes: &[u8], utf8_error: &str) -> Result<Value, String> {
    let source = std::str::from_utf8(bytes).map_err(|_| utf8_error.to_owned())?;
    let parsed = serde_json::from_str(source).map_err(internal_models_bindings::json_decode_error_string)?;
    JsonValue::from_serde(parsed)
        .map(JsonValue::into_serde)
        .map_err(|e| e.message())
}
/// Serialize the schema's sorted-key JSON object directly to its wire bytes.
pub(crate) fn encode(mut value: Value) -> Result<Vec<u8>, String> {
    value.sort_all_objects();
    serde_json::to_vec(&value).map_err(|e| format!("TypeError: {e}"))
}
/// Strict object access preserves the source diagnostic for a wrong shape or missing field.
pub(crate) fn required<'a>(v: &'a Value, key: &str) -> Result<&'a Value, String> {
    v.as_object()
        .ok_or_else(|| "TypeError: expected JSON object".to_owned())?
        .get(key)
        .ok_or_else(|| format!("KeyError: missing JSON object key `{key}`"))
}
/// Borrow text directly while inspecting a tag or a key.
pub(crate) fn text<'a>(v: &'a Value, key: &str) -> Result<&'a str, String> {
    required(v, key)?
        .as_str()
        .ok_or_else(|| "TypeError: expected JSON string".to_owned())
}
/// Read a strictly integral signed value.
pub(crate) fn integer(v: &Value, key: &str) -> Result<i64, String> {
    required(v, key)?
        .as_i64()
        .ok_or_else(|| "TypeError: expected JSON int".to_owned())
}
/// Borrow the array storage while decoding each item.
pub(crate) fn items<'a>(v: &'a Value, key: &str) -> Result<&'a [Value], String> {
    required(v, key)?
        .as_array()
        .map(Vec::as_slice)
        .ok_or_else(|| "TypeError: expected JSON array".to_owned())
}
/// Verify the envelope schema before inspecting its payload.
fn activation(bytes: &[u8], schema: &str) -> Result<Value, String> {
    let v = document(bytes, "compiler activation envelope is not UTF-8")?;
    if text(&v, "schema")? != schema {
        return Err("unsupported compiler activation envelope schema".to_owned());
    }
    Ok(v)
}
/// Encode all six independent key dimensions.
fn key_value(k: &RecordKey) -> Value {
    json!({"family":k.family,"identity":k.identity,"identity_profile":k.identity_profile,"origin":k.origin,"facet":k.facet,"context":k.context})
}
/// Parse the complete key before checking nonempty authority.
fn read_key(v: &Value) -> Result<RecordKey, String> {
    let k = RecordKey {
        family: text(v, "family")?.to_owned(),
        identity: text(v, "identity")?.to_owned(),
        identity_profile: text(v, "identity_profile")?.to_owned(),
        origin: text(v, "origin")?.to_owned(),
        facet: text(v, "facet")?.to_owned(),
        context: text(v, "context")?.to_owned(),
    };
    if !valid_key(&k) {
        return Err("compiler record key authority fields must be nonempty".to_owned());
    }
    Ok(k)
}
/// Resolve a lower-case digest only under one of the three engine-owned families.
pub fn envelope_key(family: &str, root: &str) -> Result<String, String> {
    if !matches!(family, DEMAND_FAMILY | PREPARED_FAMILY | WORK_FAMILY)
        || root.len() != 64
        || !root.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err("invalid compiler activation envelope root".to_owned());
    }
    Ok(format!("{family}\t{root}"))
}
macro_rules! stages {
    ($($stage:ident => $wire:literal),+ $(,)?) => {
        /// Return the stable spelling independent of the enum's ordinal.
        fn stage_wire(stage:&ProducerStage)->&'static str { match stage { $(ProducerStage::$stage=>$wire,)+ } }
        /// Read the closed stage vocabulary.
        fn read_stage(value:&str)->Result<ProducerStage,String>{ match value { $($wire=>Ok(ProducerStage::$stage),)+ _=>Err("unknown compiler producer stage".to_owned()) } }
    };
}
stages! {Discover=>"discover",Signature=>"signature",Layout=>"layout",Default=>"default",CheckBody=>"check-body",
BodyIr=>"body-ir",Instance=>"instance",LoweringDemand=>"lowering-demand",PlanFunction=>"plan-function",NativeInterface=>"native-interface"}
/// Return the stable wire spelling to callers that own the result.
pub fn stage_name(stage: &ProducerStage) -> String {
    stage_wire(stage).to_owned()
}

/// Encode one exact field, range, or membership selector.
pub(crate) fn selection_value(selection: &RecordSelection) -> Value {
    match selection {
        RecordSelection::Field(name) => json!({"kind":"field","name":name}),
        RecordSelection::Payload(p) => {
            json!({"kind":"payload","reference":p.reference,"offset":p.offset,"length":p.length})
        }
        RecordSelection::Members(m) => {
            json!({"kind":"members","selector":m.selector,"order":m.order,"lower":m.lower,"upper":m.upper,"continuation":m.continuation})
        }
    }
}
/// Decode only the selection shape. Activation and receipt callers enforce their different request checks.
pub(crate) fn read_selection(v: &Value, unknown: &str) -> Result<RecordSelection, String> {
    Ok(match text(v, "kind")? {
        "field" => RecordSelection::Field(text(v, "name")?.to_owned()),
        "payload" => RecordSelection::Payload(PayloadSelection {
            reference: text(v, "reference")?.to_owned(),
            offset: integer(v, "offset")?,
            length: integer(v, "length")?,
        }),
        "members" => RecordSelection::Members(MemberSelection {
            selector: text(v, "selector")?.to_owned(),
            order: text(v, "order")?.to_owned(),
            lower: text(v, "lower")?.to_owned(),
            upper: text(v, "upper")?.to_owned(),
            continuation: text(v, "continuation")?.to_owned(),
        }),
        _ => return Err(unknown.to_owned()),
    })
}
/// Read one prepared request; the producer validates the upper bound at activation.
fn read_request(v: &Value) -> Result<RecordRequest, String> {
    let selection = read_selection(required(v, "selection")?, "unknown compiler record selection")?;
    let byte_limit = integer(v, "byte_limit")?;
    if byte_limit < 0 {
        return Err("compiler record byte limit must be nonnegative".to_owned());
    }
    Ok(RecordRequest {
        key: read_key(required(v, "key")?)?,
        selection,
        byte_limit,
    })
}
/// Encode a page in supplied key order.
pub fn encode_demand_page(page: &DemandPage) -> Result<Vec<u8>, String> {
    encode(json!({"schema":DEMAND_SCHEMA,"keys":page.keys.iter().map(key_value).collect::<Vec<_>>()}))
}
/// Decode every logical key of one demand page.
pub fn decode_demand_page(bytes: &[u8]) -> Result<DemandPage, String> {
    let v = activation(bytes, DEMAND_SCHEMA)?;
    Ok(DemandPage {
        keys: items(&v, "keys")?.iter().map(read_key).collect::<Result<_, _>>()?,
    })
}
/// Encode a ready work item.
pub fn encode_ready_work(work: &ReadyWork) -> Result<Vec<u8>, String> {
    encode(
        json!({"schema":WORK_SCHEMA,"output":key_value(&work.output),"stage":stage_wire(&work.stage),"profile":work.profile.identity.digest}),
    )
}
/// Decode one selected output, exact stage and producer profile.
pub fn decode_ready_work(bytes: &[u8]) -> Result<ReadyWork, String> {
    let v = activation(bytes, WORK_SCHEMA)?;
    Ok(ReadyWork {
        output: read_key(required(&v, "output")?)?,
        stage: read_stage(text(&v, "stage")?)?,
        profile: ProducerProfile {
            identity: root(&v, "profile")?,
        },
    })
}
/// Recover a root from the named JSON string.
fn root(v: &Value, key: &str) -> Result<RecordRoot, String> {
    Ok(RecordRoot {
        digest: text(v, key)?.to_owned(),
    })
}
/// Write the complete prepared inputs and rank certificate.
pub fn encode_prepared_inputs(p: &PreparedInputs) -> Result<Vec<u8>, String> {
    let requests: Vec<_> = p
        .required_reads
        .iter()
        .map(|r| json!({"key":key_value(&r.key),"byte_limit":r.byte_limit,"selection":selection_value(&r.selection)}))
        .collect();
    let rank = &p.rank_certificate.rank;
    encode(
        json!({"schema":PREPARED_SCHEMA,"output":key_value(&p.output),"snapshot":p.snapshot.root.digest,
        "profile":p.profile.identity.digest,"required_reads":requests,"rank_namespace":rank.namespace,"rank_stage":stage_wire(&rank.stage),
        "rank_dependency":rank.dependency,"rank_evidence":p.rank_certificate.evidence.digest,"selector_witnesses":p.selector_witnesses.digest}),
    )
}
/// Preserve the original order of request and certificate diagnostics.
pub fn decode_prepared_inputs(bytes: &[u8]) -> Result<PreparedInputs, String> {
    let v = activation(bytes, PREPARED_SCHEMA)?;
    let required_reads = items(&v, "required_reads")?
        .iter()
        .map(read_request)
        .collect::<Result<_, _>>()?;
    Ok(PreparedInputs {
        output: read_key(required(&v, "output")?)?,
        snapshot: InputSnapshot {
            root: root(&v, "snapshot")?,
        },
        profile: ProducerProfile {
            identity: root(&v, "profile")?,
        },
        required_reads,
        rank_certificate: RankCertificate {
            rank: ProducerRank {
                namespace: integer(&v, "rank_namespace")?,
                stage: read_stage(text(&v, "rank_stage")?)?,
                dependency: integer(&v, "rank_dependency")?,
            },
            evidence: root(&v, "rank_evidence")?,
        },
        selector_witnesses: root(&v, "selector_witnesses")?,
    })
}
