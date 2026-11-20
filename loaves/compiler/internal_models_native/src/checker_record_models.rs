//! Checker, graph and compiled-body values shared by the canonical checker-record codecs.

use internal_models_bindings::MappedView;

/// A schema-owned field: node is a structural role or stable binding reference; values follow its typed grammar.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct CanonicalField {
    pub family: String,
    pub node: String,
    pub selector: String,
    pub values: Vec<String>,
}
crate::incan_debug::incan_debug!(CanonicalField {
    family,
    node,
    selector,
    values
});

/// Infrastructure/capability refusals; none is a source-language diagnostic or a legacy fallback request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CodecFault {
    Malformed,
    ProfileMismatch,
    OwnerMismatch,
    BodyChanged,
    Unsupported(String),
    Limit,
    LimitExceeded(String, i64, i64),
    Encoding(String),
}

/// Declaration/default output, pending local output and settled output have distinct contracts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CheckedFamily {
    Declaration,
    Local,
    Final,
}

/// One checked owner; owner is its checked stable source identity, root its position-free semantic syntax.
#[derive(Clone, PartialEq, Eq)]
pub struct CheckedRecord {
    pub profile: String,
    pub owner: String,
    pub root: String,
    pub family: CheckedFamily,
    pub fields: Vec<CanonicalField>,
}
crate::incan_debug::incan_debug!(CheckedRecord {
    profile,
    owner,
    root,
    family,
    fields
});

/// One graph-selected owner's checker values and opaque kernel-owned executable frame.
#[derive(Clone, PartialEq, Eq)]
pub struct GraphBodyValue {
    pub identity: String,
    pub local: Vec<u8>,
    pub r#final: Vec<u8>,
    pub body_ir: Vec<u8>,
}
crate::incan_debug::incan_debug!(GraphBodyValue {
    identity,
    local,
    r#final,
    body_ir
});

/// One validated graph-selected owner projected into native checker fields plus its opaque Body IR frame.
#[derive(Clone, PartialEq, Eq)]
pub struct GraphBodyFields {
    pub identity: String,
    pub local: Vec<Vec<String>>,
    pub final_fields: Vec<Vec<String>>,
    pub body_ir: Vec<u8>,
}
crate::incan_debug::incan_debug!(GraphBodyFields {
    identity,
    local,
    final_fields,
    body_ir
});

/// One checked caller and target with the target's settled executable input contract.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct SettledCallRead {
    pub reader: String,
    pub target: String,
    pub digest: String,
}
crate::incan_debug::incan_debug!(SettledCallRead { reader, target, digest });

/// One graph-selected editable namespace with its interface, Body IR and authenticated lowering witnesses.
///
/// Defaults: `lowering_inputs`, `package_roots` and `call_reads` empty, `call_reads_known` true, `identity_proof`
/// empty.
#[derive(Clone, PartialEq, Eq)]
pub struct GraphNamespaceValue {
    pub module: String,
    pub interface: Vec<u8>,
    pub body_ir: Vec<u8>,
    pub lowering_inputs: Vec<Vec<String>>,
    pub package_roots: Vec<String>,
    pub call_reads: Vec<SettledCallRead>,
    pub call_reads_known: bool,
    pub identity_proof: Vec<u8>,
}
crate::incan_debug::incan_debug!(GraphNamespaceValue {
    module,
    interface,
    body_ir,
    lowering_inputs,
    package_roots,
    call_reads,
    call_reads_known,
    identity_proof
});

/// A checked namespace whose large interface and executable payloads retain the admitted envelope.
pub struct GraphNamespaceView {
    pub module: String,
    pub interface: MappedView,
    pub body_ir: MappedView,
    pub lowering_inputs: Vec<Vec<String>>,
    pub package_roots: Vec<String>,
    pub call_reads: Vec<SettledCallRead>,
    pub call_reads_known: bool,
    pub identity_proof: Vec<u8>,
}

/// One compiler-produced checker triple and kernel-owned Body IR frame with its projected source owner.
///
/// Defaults: `dependency_reads` and `scope_names` empty. `scope_names` is nonempty only when the frontend proves
/// that a scalar body can consult these project names alone.
#[derive(Clone, PartialEq, Eq)]
pub struct CompiledBodyValue {
    pub module: String,
    pub name: String,
    pub identity: String,
    pub declaration: Vec<u8>,
    pub local: Vec<u8>,
    pub r#final: Vec<u8>,
    pub body_ir: Vec<u8>,
    pub dependency_reads: Vec<CompiledBodyDependencyRead>,
    pub scope_names: Vec<String>,
}
crate::incan_debug::incan_debug!(CompiledBodyValue {
    module,
    name,
    identity,
    declaration,
    local,
    r#final,
    body_ir,
    dependency_reads,
    scope_names
});

/// One exact package or project declaration record witnessed by a compiled owner.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct CompiledBodyDependencyRead {
    pub package: Option<String>,
    pub module: String,
    pub name: String,
    pub identity: String,
    pub reference_subject: String,
}
crate::incan_debug::incan_debug!(CompiledBodyDependencyRead {
    package,
    module,
    name,
    identity,
    reference_subject
});

/// One checked owner that cannot enter the record fast path, retaining absence when identity production failed.
#[derive(Clone, PartialEq, Eq)]
pub struct CompiledBodyFallback {
    pub module: String,
    pub name: String,
    pub identity: Option<String>,
    pub reason: String,
}
crate::incan_debug::incan_debug!(CompiledBodyFallback {
    module,
    name,
    identity,
    reason
});

/// The executable declaration that supplies one positional access contract.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct SettledCallContract {
    pub target: String,
    pub digest: String,
    pub reader: String,
    pub owner: String,
}
crate::incan_debug::incan_debug!(SettledCallContract {
    target,
    digest,
    reader,
    owner
});

/// One installable checked namespace plus the lowering witnesses produced by the same completed check.
///
/// Defaults: every field after `dependencies` is empty, `call_contracts` is `None`. `published_record` is the exact
/// durable namespace record produced by a compiler that can keep its decoded body.
#[derive(Clone, PartialEq, Eq)]
pub struct CompiledNamespaceValue {
    pub module: String,
    pub interface: Vec<u8>,
    pub body_ir: Vec<u8>,
    pub dependencies: Vec<String>,
    pub lowering_inputs: Vec<Vec<String>>,
    pub package_roots: Vec<String>,
    pub call_reads: Vec<SettledCallRead>,
    pub semantic_interface: Vec<u8>,
    pub member_witnesses: Vec<Vec<String>>,
    pub call_contracts: Option<Vec<SettledCallContract>>,
    pub identity_proof: Vec<u8>,
    pub published_record: Vec<u8>,
}
crate::incan_debug::incan_debug!(CompiledNamespaceValue {
    module,
    interface,
    body_ir,
    dependencies,
    lowering_inputs,
    package_roots,
    call_reads,
    semantic_interface,
    member_witnesses,
    call_contracts,
    identity_proof,
    published_record
});

/// Every namespace, owner value and explicit full-source fallback awaiting independent binary framing.
///
/// Default: `namespaces` empty.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct CompiledBodyOutputs {
    pub namespaces: Vec<CompiledNamespaceValue>,
    pub values: Vec<CompiledBodyValue>,
    pub fallbacks: Vec<CompiledBodyFallback>,
}
crate::incan_debug::incan_debug!(CompiledBodyOutputs {
    namespaces,
    values,
    fallbacks
});
