//! Decode one local-family or final-family activation into its source and checked inputs: exactly one module header,
//! one named owner source and the owner's own value, with the other declarations as an immutable environment.

use internal_models_native::checker_record_models::{CheckedFamily, CodecFault};
use internal_models_native::checker_records::{CHECKER_PROFILE, decode_record, native_fields};
use internal_models_native::record_scope::{ReadReceipt, ReadScope, RecordRead, RecordReads, RecordRequest};
use internal_models_native::record_service::prepared_reader;
use internal_models_native::store_record_source::StoreRecordSource;

use crate::checked_activation::{
    ResolvedCheckingActivation, projected_owner_source, resolve_checking_activation, stage_checking_result,
};
use crate::record_protocol::{Evaluate, Produced};

/// Decoded values passed to the frontend's single-owner local entry, plus their completed read receipt.
pub struct LocalActivationInputs {
    pub source: String,
    pub owner: String,
    pub declaration: Vec<Vec<String>>,
    pub environment: Vec<Vec<Vec<String>>>,
    pub environment_names: Vec<String>,
    pub environment_identities: Vec<String>,
    pub receipt: ReadReceipt,
}

/// Decoded values passed to the frontend's single-owner final entry, plus their completed read receipt.
pub struct FinalActivationInputs {
    pub source: String,
    pub owner: String,
    pub local: Vec<Vec<String>>,
    pub environment: Vec<Vec<Vec<String>>>,
    pub environment_names: Vec<String>,
    pub environment_identities: Vec<String>,
    pub receipt: ReadReceipt,
}

/// What one activation family reads besides its own value: the header, the owner source and the environment.
struct Shared {
    kind: &'static str,
    module_header: Option<String>,
    owner_source: Option<String>,
    owner: String,
    environment: Vec<Vec<Vec<String>>>,
    environment_names: Vec<String>,
    environment_identities: Vec<String>,
}

impl Shared {
    fn new(kind: &'static str) -> Self {
        Self {
            kind,
            module_header: None,
            owner_source: None,
            owner: String::new(),
            environment: Vec::new(),
            environment_names: Vec::new(),
            environment_identities: Vec::new(),
        }
    }

    /// Take a module header or owner source; `Ok(false)` leaves the request to the family's own handling.
    fn source_input(&mut self, request: &RecordRequest, found: &[u8]) -> Result<bool, String> {
        let kind = self.kind;
        let key = &request.key;
        if key.family == "module-header" && key.facet == "module" {
            if self.module_header.is_some() {
                return Err(format!("{kind} activation requires one module header"));
            }
            let text = String::from_utf8(found.to_vec()).map_err(|_| format!("{kind} module header is not UTF-8"))?;
            self.module_header = Some(text);
            return Ok(true);
        }
        if key.family == "owner-source" && !key.facet.is_empty() {
            if self.owner_source.is_some() {
                return Err(format!("{kind} activation requires one named owner source"));
            }
            let text = String::from_utf8(found.to_vec()).map_err(|_| format!("{kind} owner source is not UTF-8"))?;
            self.owner_source = Some(text);
            self.owner = key.facet.clone();
            return Ok(true);
        }
        Ok(false)
    }

    /// Keep one environment declaration's fields, its name and its stable identity.
    fn environment_value(&mut self, request: &RecordRequest, fields: Vec<Vec<String>>) {
        self.environment.push(fields);
        self.environment_names.push(request.key.facet.clone());
        self.environment_identities.push(request.key.identity.clone());
    }
}

/// The scalar inputs preparation used to select typed declarations; they contribute no decoded value.
fn selector_family(family: &str) -> bool {
    matches!(
        family,
        "owner-census" | "scope-name" | "declaration-slot" | "declaration-signature"
    )
}

/// Open the prepared reader and the activation's store source.
fn open_reads(activation: &ResolvedCheckingActivation, kind: &str) -> Result<(RecordReads, StoreRecordSource), String> {
    let prepared = &activation.prepared;
    let reader = prepared_reader(&activation.ready, &prepared.snapshot, prepared)
        .map_err(|_| format!("invalid {kind} activation preparation"))?;
    let scope = ReadScope {
        snapshot: prepared.snapshot.root.digest.clone(),
        profile: activation.ready.profile.identity.digest.clone(),
    };
    Ok((reader, activation.snapshot.record_source(&scope)))
}

/// One required input's value; every other read outcome refuses the activation.
fn found_value(
    reader: &mut RecordReads,
    source: &StoreRecordSource,
    request: &RecordRequest,
    kind: &str,
) -> Result<Vec<u8>, String> {
    match reader.read(source, request) {
        RecordRead::Found(value) => Ok(value.value),
        RecordRead::Deleted(_) => Err(format!("{kind} activation input was deleted")),
        RecordRead::Absent(_) => Err(format!("{kind} activation input is absent")),
        RecordRead::Members(_) => Err(format!("{kind} activation input cannot be a member page")),
        RecordRead::Failed(_) => Err(format!("{kind} activation record read failed")),
    }
}

/// Resolve and decode one Evaluate message's roots before opening its scoped store reader.
pub fn resolve_local_activation(home: &str, evaluation: &Evaluate) -> Result<LocalActivationInputs, String> {
    read_local_activation(&resolve_checking_activation(home, evaluation)?)
}

/// Decode exactly one source and one matching declaration; other declaration values form the immutable environment.
pub fn read_local_activation(activation: &ResolvedCheckingActivation) -> Result<LocalActivationInputs, String> {
    let output = &activation.ready.output;
    let (mut reader, source) = open_reads(activation, "local")?;
    let mut shared = Shared::new("local");
    let mut declaration: Option<Vec<Vec<String>>> = None;
    for request in &activation.prepared.required_reads {
        let found = found_value(&mut reader, &source, request, "local")?;
        if shared.source_input(request, &found)? {
            continue;
        }
        let key = &request.key;
        let declared =
            key.family == "declaration" || (output.family == "declaration" && key.family == "prior-declaration");
        if declared && !key.context.is_empty() {
            let record = decode_record(&found, CHECKER_PROFILE, &key.identity, &key.context)
                .map_err(|_| "local declaration record is invalid".to_owned())?;
            if record.family != CheckedFamily::Declaration {
                return Err("local activation requires declaration-family environment values".to_owned());
            }
            let fields = native_fields(&record).map_err(|_| "local declaration fields are invalid".to_owned())?;
            if key.identity == output.identity {
                if declaration.is_some() {
                    return Err("local activation has more than one own declaration".to_owned());
                }
                declaration = Some(fields);
            } else {
                shared.environment_value(request, fields);
            }
        } else if !selector_family(&key.family) {
            return Err("local activation prepared an unsupported input family".to_owned());
        }
    }
    let receipt = reader
        .finish()
        .map_err(|_| "local activation read receipt is incomplete".to_owned())?;
    match (shared.module_header, shared.owner_source, declaration) {
        (Some(header), Some(owner_source), Some(declaration)) => Ok(LocalActivationInputs {
            source: if output.family == "declaration" {
                owner_source
            } else {
                projected_owner_source(&header, &owner_source)
            },
            owner: shared.owner,
            declaration,
            environment: shared.environment,
            environment_names: shared.environment_names,
            environment_identities: shared.environment_identities,
            receipt,
        }),
        _ => Err("local activation is missing module header, owner source or declaration input".to_owned()),
    }
}

/// Stage one produced value and its canonical evidence pages, then return only their content roots.
pub fn stage_local_result(
    home: &str,
    snapshot: &str,
    value: &[u8],
    receipt: &ReadReceipt,
    semantic: &str,
) -> Result<Produced, String> {
    stage_checking_result(home, snapshot, value, receipt, semantic)
}

/// Name the exact local-codec refusal without leaking input values into worker diagnostics.
fn codec_fault_name(fault: CodecFault) -> String {
    match fault {
        CodecFault::Malformed => "malformed".to_owned(),
        CodecFault::ProfileMismatch => "profile mismatch".to_owned(),
        CodecFault::OwnerMismatch => "owner mismatch".to_owned(),
        CodecFault::BodyChanged => "body changed".to_owned(),
        CodecFault::Unsupported(_) => "unsupported".to_owned(),
        CodecFault::Limit => "limit".to_owned(),
        CodecFault::LimitExceeded(..) => "limit exceeded".to_owned(),
        CodecFault::Encoding(reason) => reason,
    }
}

/// Decode only one source and its matching local value; declarations supply the identities needed to rebind it.
pub fn read_final_activation(activation: &ResolvedCheckingActivation) -> Result<FinalActivationInputs, String> {
    let output = &activation.ready.output;
    let (mut reader, source) = open_reads(activation, "final")?;
    let mut shared = Shared::new("final");
    let mut local: Option<Vec<Vec<String>>> = None;
    for request in &activation.prepared.required_reads {
        let found = found_value(&mut reader, &source, request, "final")?;
        if shared.source_input(request, &found)? {
            continue;
        }
        let key = &request.key;
        if key.family == "local" && key.identity == output.identity && !key.context.is_empty() {
            if local.is_some() {
                return Err("final activation has more than one own local input".to_owned());
            }
            let record = decode_record(&found, CHECKER_PROFILE, &key.identity, &key.context)
                .map_err(|fault| format!("final local record is invalid: {}", codec_fault_name(fault)))?;
            if record.family != CheckedFamily::Local {
                return Err("final activation requires a local-family input".to_owned());
            }
            local = Some(native_fields(&record).map_err(|_| "final local fields are invalid".to_owned())?);
        } else if key.family == "declaration" && !key.context.is_empty() {
            let record = decode_record(&found, CHECKER_PROFILE, &key.identity, &key.context)
                .map_err(|_| "final declaration record is invalid".to_owned())?;
            if record.family != CheckedFamily::Declaration {
                return Err("final activation requires declaration-family environment values".to_owned());
            }
            if key.identity != output.identity {
                let fields = native_fields(&record).map_err(|_| "final declaration fields are invalid".to_owned())?;
                shared.environment_value(request, fields);
            }
        } else if !selector_family(&key.family) {
            return Err("final activation prepared an unsupported input family".to_owned());
        }
    }
    let receipt = reader
        .finish()
        .map_err(|_| "final activation read receipt is incomplete".to_owned())?;
    match (shared.module_header, shared.owner_source, local) {
        (Some(header), Some(owner_source), Some(local)) => Ok(FinalActivationInputs {
            source: projected_owner_source(&header, &owner_source),
            owner: shared.owner,
            local,
            environment: shared.environment,
            environment_names: shared.environment_names,
            environment_identities: shared.environment_identities,
            receipt,
        }),
        _ => Err("final activation is missing module header, owner source or local input".to_owned()),
    }
}
