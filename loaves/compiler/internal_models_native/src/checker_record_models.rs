//! Checker, graph and compiled-body values shared by the canonical checker-record codecs.

use internal_models_bindings::MappedView;

/// A schema-owned field: node is a structural role or stable binding reference; values follow its typed grammar.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CanonicalField {
    pub family: String,
    pub node: String,
    pub selector: String,
    pub values: Vec<String>,
}

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
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckedRecord {
    pub profile: String,
    pub owner: String,
    pub root: String,
    pub family: CheckedFamily,
    pub fields: Vec<CanonicalField>,
}

/// One graph-selected owner's checker values and opaque kernel-owned executable frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphBodyValue {
    pub identity: String,
    pub local: Vec<u8>,
    pub r#final: Vec<u8>,
    pub body_ir: Vec<u8>,
}

/// One validated graph-selected owner projected into native checker fields plus its opaque Body IR frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphBodyFields {
    pub identity: String,
    pub local: Vec<Vec<String>>,
    pub final_fields: Vec<Vec<String>>,
    pub body_ir: Vec<u8>,
}

/// One checked caller and target with the target's settled executable input contract.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SettledCallRead {
    pub reader: String,
    pub target: String,
    pub digest: String,
}

/// One graph-selected editable namespace with its interface, Body IR and authenticated lowering witnesses.
///
/// Defaults: `lowering_inputs`, `package_roots` and `call_reads` empty, `call_reads_known` true, `identity_proof`
/// empty.
#[derive(Clone, Debug, PartialEq, Eq)]
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
#[derive(Clone, Debug, PartialEq, Eq)]
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

/// One exact package or project declaration record witnessed by a compiled owner.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CompiledBodyDependencyRead {
    pub package: Option<String>,
    pub module: String,
    pub name: String,
    pub identity: String,
    pub reference_subject: String,
}

/// One checked owner that cannot enter the record fast path, retaining absence when identity production failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompiledBodyFallback {
    pub module: String,
    pub name: String,
    pub identity: Option<String>,
    pub reason: String,
}

/// The executable declaration that supplies one positional access contract.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SettledCallContract {
    pub target: String,
    pub digest: String,
    pub reader: String,
    pub owner: String,
}

/// One installable checked namespace plus the lowering witnesses produced by the same completed check.
///
/// Defaults: every field after `dependencies` is empty, `call_contracts` is `None`. `published_record` is the exact
/// durable namespace record produced by a compiler that can keep its decoded body.
#[derive(Clone, Debug, PartialEq, Eq)]
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

/// Every namespace, owner value and explicit full-source fallback awaiting independent binary framing.
///
/// Default: `namespaces` empty.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CompiledBodyOutputs {
    pub namespaces: Vec<CompiledNamespaceValue>,
    pub values: Vec<CompiledBodyValue>,
    pub fallbacks: Vec<CompiledBodyFallback>,
}
