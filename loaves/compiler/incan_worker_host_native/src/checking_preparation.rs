//! Compiler-owned preparation for a previously witnessed single-owner checking contribution.

use std::collections::HashSet;

use incan_worker_host_bindings::{Lookup, owned};
use internal_models_native::activation_envelopes::{
    DEMAND_FAMILY, decode_demand_page, encode_prepared_inputs, encode_ready_work, envelope_key,
};
use internal_models_native::checker_records::CHECKER_PROFILE;
use internal_models_native::namespace_order::{
    checking_basis_key, checking_completion_receipt, decode_namespace_order, read_checking_basis,
};
use internal_models_native::read_receipts::decode_read_receipt;
use internal_models_native::record_scope::{
    ReadReceipt, ReadScope, ReadWitness, RecordKey, RecordRead, RecordRequest, RecordSelection, checked_row_request,
};
use internal_models_native::record_service::{
    InputSnapshot, PreparedInputs, ProducerProfile, ProducerRank, ProducerStage, RankCertificate, ReadyWork, RecordRoot,
};
use internal_models_native::selection_bindings::read_binding;
use internal_models_native::store_record_source::encode_record_key;

use crate::record_protocol::{Prepare, Prepared};
use crate::record_snapshot::{ResolvedSnapshot, digest, resolve_snapshot};

/// The checked scalar selector families that fix the prepared declaration environment.
const SELECTOR_FAMILIES: [&str; 4] = [
    "declaration-slot",
    "owner-census",
    "scope-name",
    "declaration-signature",
];
/// The four project scalar families an independently closed owner reads.
const CLOSED_FAMILIES: [&str; 4] = ["owner-source", "module-header", "owner-census", "declaration"];
const UNSUPPORTED_CLOSED: &str = "unsupported-closed-checking-inputs";

/// The supplier receipt and namespace certificate one preparation is bound to.
struct Certified {
    basis_root: String,
    evidence: String,
    rank: i64,
}

/// Require one preparation input inside the exact leased snapshot.
fn required_record(snapshot: &ResolvedSnapshot, key: &str) -> Result<Vec<u8>, String> {
    match snapshot.read(key).map_err(|error| error.message())? {
        Lookup::Found(value) => Ok(owned(&value)),
        _ => Err(format!("checking preparation is missing {key}")),
    }
}

/// One supplier receipt, verified against its content root before it is decoded.
fn verified_receipt(snapshot: &ResolvedSnapshot, root: &str, differs: &str) -> Result<ReadReceipt, String> {
    let encoded = required_record(snapshot, &format!("incan.engine.receipt\t{root}"))?;
    if digest(&encoded) != root {
        return Err(differs.to_owned());
    }
    decode_read_receipt(&encoded)
}

/// Whether a supplier selects the whole scalar value of a project record.
fn full_value(request: &RecordRequest) -> bool {
    request.selection == RecordSelection::Field("value".to_owned())
}

/// Validate current selector evidence before staging the rank, exact supplier requests and ready-work page.
pub fn prepare_checking(home: &str, request: &Prepare) -> Result<Prepared, String> {
    if request.producer_profile != CHECKER_PROFILE {
        return Err("unsupported checking producer profile".to_owned());
    }
    let resolved = resolve_snapshot(home, &request.snapshot).map_err(|error| error.message())?;
    let demand_bytes = required_record(&resolved, &envelope_key(DEMAND_FAMILY, &request.demand_root)?)?;
    if digest(&demand_bytes) != request.demand_root {
        return Err("checking demand differs from its content root".to_owned());
    }
    let demand = decode_demand_page(&demand_bytes)?;
    if demand.keys.len() > 1 {
        return prepare_closed_owner(&resolved, request, &demand.keys);
    }
    let output = match demand.keys.as_slice() {
        [output] if output.family == "local" || output.family == "final" => output.clone(),
        _ => return Err("checking preparation requires one local or final output".to_owned()),
    };
    if request.stage != "check-body" && request.stage != output.family {
        return Err("checking demand does not belong to the requested stage".to_owned());
    }
    let pointer = required_record(&resolved, &checking_basis_key(&output)?)?;
    let basis_root = read_checking_basis(&output, &pointer)?;
    let basis = verified_receipt(
        &resolved,
        &basis_root,
        "checking supplier evidence differs from its receipt root",
    )?;
    if basis.scope.profile != request.producer_profile || basis.preparation.output != output {
        return Err("checking supplier evidence belongs to another output or profile".to_owned());
    }
    let evidence = basis.preparation.rank_certificate.clone();
    let certificate = required_record(&resolved, &format!("incan.compiler.namespace-order\t{evidence}"))?;
    let order = decode_namespace_order(&certificate, &output.origin)?;
    if evidence != order.evidence {
        return Err("checking namespace order differs from its content root".to_owned());
    }
    let [module] = order.members.as_slice() else {
        return Err("checking preparation requires the compiler's joint namespace producer".to_owned());
    };
    let scope = ReadScope {
        snapshot: request.snapshot.clone(),
        profile: request.producer_profile.clone(),
    };
    let reads = supplier_reads(&resolved, &scope, &output, module, &basis)?;
    let dependency = if output.family == "final" { 1 } else { 0 };
    let certified = Certified {
        basis_root,
        evidence,
        rank: order.rank,
    };
    stage_preparation(
        &resolved,
        request,
        &output,
        (ProducerStage::CheckBody, dependency),
        reads,
        &certified,
    )
}

/// Owner-specific predecessors one supplier receipt witnessed.
#[derive(Default)]
struct Predecessors {
    owner_sources: i64,
    own_declarations: i64,
    own_locals: i64,
    headers: i64,
    declaration_slots: HashSet<String>,
    declarations: HashSet<String>,
}

impl Predecessors {
    /// Count one witnessed supplier, refusing any family preparation cannot carry forward.
    fn witness(&mut self, supplier: &RecordRequest, output: &RecordKey, module: &str) -> Result<(), String> {
        let key = &supplier.key;
        if !full_value(supplier) || key.origin != output.origin {
            return Err("checking preparation requires a full project scalar input".to_owned());
        }
        match key.family.as_str() {
            "owner-source" => {
                if key.identity != module {
                    return Err("checking source does not belong to its certified namespace".to_owned());
                }
                self.owner_sources += 1;
            }
            "module-header" => {
                if key.identity != module || key.facet != "module" {
                    return Err("checking header does not belong to its certified namespace".to_owned());
                }
                self.headers += 1;
            }
            "declaration" => {
                self.declarations.insert(key.identity.clone());
                if key.identity == output.identity {
                    self.own_declarations += 1;
                }
            }
            "local" => {
                if key.identity != output.identity || output.family != "final" {
                    return Err("checking preparation selected an unrelated local predecessor".to_owned());
                }
                self.own_locals += 1;
            }
            family if SELECTOR_FAMILIES.contains(&family) => {}
            family => return Err(format!("checking preparation requires compiler discovery for {family}")),
        }
        Ok(())
    }

    /// Require exactly the owner's source, header and declaration, and the declaration behind every call slot.
    fn complete(&self, output: &RecordKey) -> Result<(), String> {
        if self.owner_sources != 1
            || self.own_declarations != 1
            || self.headers != 1
            || (output.family == "final" && self.own_locals != 1)
        {
            return Err("checking preparation requires complete owner-specific predecessors".to_owned());
        }
        if self
            .declaration_slots
            .iter()
            .any(|identity| !self.declarations.contains(identity))
        {
            return Err("checking preparation requires the declaration selected by each call slot".to_owned());
        }
        Ok(())
    }
}

/// Re-read every witnessed supplier under the current snapshot and keep its exact request.
fn supplier_reads(
    resolved: &ResolvedSnapshot,
    scope: &ReadScope,
    output: &RecordKey,
    module: &str,
    basis: &ReadReceipt,
) -> Result<Vec<RecordRequest>, String> {
    let source = resolved.record_source(scope);
    let mut reads = Vec::new();
    let mut seen = Predecessors::default();
    for witness in &basis.witnesses {
        let ReadWitness::Found(prior, _) = witness else {
            return Err(
                "checking preparation requires a new compiler selector for exclusion or member evidence".to_owned(),
            );
        };
        let supplier = &prior.request;
        seen.witness(supplier, output, module)?;
        let RecordRead::Found(current) = source.read(scope, supplier) else {
            return Err("checking supplier is unfinished or outside its preparation".to_owned());
        };
        let family = supplier.key.family.as_str();
        if family == "local" || family == "declaration" {
            // The selected predecessor may have grown since preparation of the old receipt.
            reads.push(checked_row_request(&supplier.key, current.value.len() as i64)?);
            continue;
        }
        // Only typed checker values can vary after selection. A source, census or declaration-slot change requires
        // fresh supplier discovery before this owner may be scheduled.
        if current.evidence.semantic_digest != prior.semantic_digest {
            return Err(format!("checking supplier discovery is stale for {family}"));
        }
        if family == "declaration-slot" {
            let identity = read_binding(&encode_record_key(&supplier.key)?, &current.value)?;
            seen.declaration_slots.insert(identity);
        }
        reads.push(supplier.clone());
    }
    seen.complete(output)?;
    Ok(reads)
}

/// Stage the prepared-input and ready-work envelopes and return the prepared root.
fn stage_preparation(
    resolved: &ResolvedSnapshot,
    request: &Prepare,
    output: &RecordKey,
    (stage, dependency): (ProducerStage, i64),
    required_reads: Vec<RecordRequest>,
    certified: &Certified,
) -> Result<Prepared, String> {
    let profile = ProducerProfile {
        identity: RecordRoot {
            digest: request.producer_profile.clone(),
        },
    };
    let ready = ReadyWork {
        output: output.clone(),
        stage,
        profile: profile.clone(),
    };
    let prepared = PreparedInputs {
        output: output.clone(),
        snapshot: InputSnapshot {
            root: RecordRoot {
                digest: request.snapshot.clone(),
            },
        },
        profile,
        required_reads,
        selector_witnesses: RecordRoot {
            digest: certified.basis_root.clone(),
        },
        rank_certificate: RankCertificate {
            rank: ProducerRank {
                namespace: certified.rank,
                stage,
                dependency,
            },
            evidence: RecordRoot {
                digest: certified.evidence.clone(),
            },
        },
    };
    let prepared_root = resolved
        .stage_checking_envelopes(&encode_prepared_inputs(&prepared)?, &encode_ready_work(&ready)?)
        .map_err(|error| error.message())?;
    Ok(Prepared { prepared_root })
}

/// Bind a staged predecessor value to the completed worker receipt that actually produced it, by the predecessor's
/// encoded record key.
pub fn completed_checker_key(encoded_key: &str) -> String {
    format!("incan.compiler.completed-checking\t{}", digest(encoded_key.as_bytes()))
}

/// Verify a completed earlier stage under the same source, rank evidence and authenticated content.
fn current_predecessor(
    resolved: &ResolvedSnapshot,
    scope: &ReadScope,
    key: &RecordKey,
    certified: &Certified,
) -> Result<RecordRequest, String> {
    let value = required_record(resolved, &encode_record_key(key)?)?;
    let completion = required_record(resolved, &completed_checker_key(&encode_record_key(key)?))?;
    let receipt = decode_read_receipt(&checking_completion_receipt(&completion, &value)?)?;
    let produced = &receipt.preparation.output;
    if produced.family != key.family
        || produced.identity != key.identity
        || produced.origin != key.origin
        || produced.identity_profile != key.identity_profile
        || produced.context != "current"
    {
        return Err("closed owner selected a predecessor belonging to another work item".to_owned());
    }
    if receipt.scope.profile != scope.profile
        || receipt.preparation.selector_witnesses != certified.basis_root
        || receipt.preparation.rank_certificate != certified.evidence
    {
        return Err("closed owner selected a predecessor belonging to another compiler preparation".to_owned());
    }
    let source = resolved.record_source(scope);
    for witness in &receipt.witnesses {
        let ReadWitness::Found(prior, _) = witness else {
            return Err("closed owner predecessor has unsupported supplier discovery".to_owned());
        };
        let RecordRead::Found(current) = source.read(scope, &prior.request) else {
            return Err("closed owner predecessor lost a required input".to_owned());
        };
        if prior.semantic_digest != current.evidence.semantic_digest {
            return Err("closed owner predecessor belongs to an earlier input revision".to_owned());
        }
    }
    checked_row_request(key, value.len() as i64)
}

/// Certify the finite signature, local and settlement stages of an independently closed project function.
fn prepare_closed_owner(
    resolved: &ResolvedSnapshot,
    request: &Prepare,
    keys: &[RecordKey],
) -> Result<Prepared, String> {
    let [output, prior, predecessors @ ..] = keys else {
        return Err("closed owner demand has inconsistent stable identity".to_owned());
    };
    if output.context != "current"
        || prior.family != "local"
        || prior.origin != output.origin
        || prior.identity != output.identity
        || prior.identity_profile != output.identity_profile
    {
        return Err("closed owner demand has inconsistent stable identity".to_owned());
    }
    let (stage, dependency) = match (output.family.as_str(), request.stage.as_str(), keys.len()) {
        ("declaration", "signature", 2) => (ProducerStage::Signature, 0),
        ("local", "check-body", 3) => (ProducerStage::CheckBody, 0),
        ("final", "check-body", 4) => (ProducerStage::CheckBody, 1),
        _ => return Err("invalid closed owner stage or prerequisite arity".to_owned()),
    };
    let basis_root = read_checking_basis(prior, &required_record(resolved, &checking_basis_key(prior)?)?)?;
    let basis = verified_receipt(resolved, &basis_root, "closed owner basis differs from its root")?;
    let evidence = basis.preparation.rank_certificate.clone();
    if basis.preparation.output != *prior || basis.scope.profile != request.producer_profile {
        return Err("closed owner basis belongs to another activation".to_owned());
    }
    let encoded = required_record(resolved, &format!("incan.compiler.namespace-order\t{evidence}"))?;
    let certificate = decode_namespace_order(&encoded, &output.origin)?;
    let module = match certificate.members.as_slice() {
        [module] if certificate.evidence == evidence => module,
        _ => return Err("closed owner requires an independent namespace rank".to_owned()),
    };
    let scope = ReadScope {
        snapshot: request.snapshot.clone(),
        profile: request.producer_profile.clone(),
    };
    let mut inputs = closed_inputs(resolved, &scope, output, module, &basis)?;
    let certified = Certified {
        basis_root,
        evidence,
        rank: certificate.rank,
    };
    for (key, family) in predecessors.iter().zip(["declaration", "local"]) {
        if key.family != family
            || key.origin != output.origin
            || key.identity != output.identity
            || key.identity_profile != output.identity_profile
            || key.facet != output.facet
        {
            return Err("closed owner received a crossed predecessor".to_owned());
        }
        inputs.push(current_predecessor(resolved, &scope, key, &certified)?);
    }
    stage_preparation(resolved, request, output, (stage, dependency), inputs, &certified)
}

/// Select the closed owner's exact project scalars from its basis receipt under the current snapshot.
fn closed_inputs(
    resolved: &ResolvedSnapshot,
    scope: &ReadScope,
    output: &RecordKey,
    module: &str,
    basis: &ReadReceipt,
) -> Result<Vec<RecordRequest>, String> {
    let source = resolved.record_source(scope);
    let mut inputs = Vec::new();
    let mut families: HashSet<String> = HashSet::new();
    let mut scope_names: HashSet<String> = HashSet::new();
    for witness in &basis.witnesses {
        let ReadWitness::Found(previous, _) = witness else {
            return Err(UNSUPPORTED_CLOSED.to_owned());
        };
        let mut supplier = previous.request.clone();
        let family = supplier.key.family.clone();
        if family == "scope-name" {
            if supplier.key.facet.is_empty() || !scope_names.insert(supplier.key.facet.clone()) {
                return Err(UNSUPPORTED_CLOSED.to_owned());
            }
        } else if families.contains(&family) || !CLOSED_FAMILIES.contains(&family.as_str()) {
            return Err(UNSUPPORTED_CLOSED.to_owned());
        }
        families.insert(family.clone());
        if !full_value(&supplier) || supplier.key.origin != output.origin {
            return Err("closed owner requires an exact project scalar".to_owned());
        }
        if family == "declaration" {
            if supplier.key.identity != output.identity || supplier.key.facet != output.facet {
                return Err("closed owner declaration belongs to another function".to_owned());
            }
            if output.family != "declaration" {
                continue;
            }
            supplier.key.family = "prior-declaration".to_owned();
        } else if supplier.key.identity != module || (family == "owner-source" && supplier.key.facet != output.facet) {
            return Err("closed owner source belongs to a different function or module".to_owned());
        }
        let RecordRead::Found(current) = source.read(scope, &supplier) else {
            return Err("closed owner lost its selected input".to_owned());
        };
        if family != "owner-source" && current.evidence.semantic_digest != previous.semantic_digest {
            return Err(UNSUPPORTED_CLOSED.to_owned());
        }
        inputs.push(supplier);
    }
    if families.len() != 4 || (!scope_names.is_empty() && !scope_names.contains(&output.facet)) {
        return Err(UNSUPPORTED_CLOSED.to_owned());
    }
    Ok(inputs)
}
