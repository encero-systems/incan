//! Streaming, independently owned compiler output frames.

use crate::CodecFault;
use crate::checker_records::{CHECKER_PROFILE, CheckedFamily, MAX_FIELDS, decode_owned_record};
use crate::crumb_rows::{Fields, Rows, Writer};
pub use crate::frame_prefix::compiled_body_frame_size;
use crate::graph_records::{MAX_TRANSPORT, read_pairs, valid_pairs, write_pairs};
use crate::row_shapes::{FALLBACK, NAMESPACE, VALUE};
use CodecFault::{Limit, LimitExceeded, Malformed};
use std::collections::HashSet;

/// One installable checked namespace plus the lowering witnesses produced by the same completed check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompiledNamespaceValue {
    pub module: String,
    pub interface: Vec<u8>,
    pub body_ir: Vec<u8>,
    pub dependencies: Vec<String>,
    pub lowering_inputs: Vec<Vec<String>>,
}
/// One compiler-produced checker triple and kernel-owned Body IR frame with its projected source owner.
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
}
/// One exact package or project declaration record witnessed by a compiled owner.
#[derive(Clone, Debug, PartialEq, Eq)]
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
/// Every namespace, owner value and explicit full-source fallback awaiting independent binary framing.
#[derive(Clone, Default, Debug, PartialEq, Eq)]
pub struct CompiledBodyOutputs {
    pub namespaces: Vec<CompiledNamespaceValue>,
    pub values: Vec<CompiledBodyValue>,
    pub fallbacks: Vec<CompiledBodyFallback>,
}
/// One independently bounded compiler output frame, never a whole-program transport envelope.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CompiledBodyOutput {
    Namespace(CompiledNamespaceValue),
    Value(CompiledBodyValue),
    Fallback(CompiledBodyFallback),
}

/// Refuse an oversized owner before allocating its transport buffer.
fn room(bytes: usize) -> Result<(), CodecFault> {
    if bytes > MAX_TRANSPORT as usize {
        return Err(LimitExceeded(
            "compiled-output-frame-bytes".into(),
            i64::try_from(bytes).map_err(|_| Limit)?,
            MAX_TRANSPORT,
        ));
    }
    Ok(())
}
/// Prefix one admitted row with its exact little-endian payload length.
fn frame(out: Writer) -> Result<Vec<u8>, CodecFault> {
    let mut payload = out.finish()?;
    let size = payload.len();
    room(size)?;
    let prefix = (size as u64).to_le_bytes();
    if payload.capacity() - size >= prefix.len() {
        payload.resize(size + prefix.len(), 0);
        payload.copy_within(..size, prefix.len());
        payload[..prefix.len()].copy_from_slice(&prefix);
        return Ok(payload);
    }
    let mut encoded = Vec::with_capacity(size + prefix.len());
    encoded.extend_from_slice(&prefix);
    encoded.extend_from_slice(&payload);
    Ok(encoded)
}
/// Compare the exact newline-joined byte sequence, including any embedded newlines.
fn ordered<const N: usize>(previous: Option<[&str; N]>, current: [&str; N]) -> bool {
    /// Walk the bytes of the joined key without allocating a concatenated string.
    fn bytes<'a, const N: usize>(parts: [&'a str; N]) -> impl Iterator<Item = u8> + 'a {
        parts
            .into_iter()
            .enumerate()
            .flat_map(|(i, s)| (i != 0).then_some(b'\n').into_iter().chain(s.bytes()))
    }
    match previous {
        None => bytes(current).next().is_some(),
        Some(p) => bytes(p).lt(bytes(current)),
    }
}
/// Authenticate all three checker records and verify their stage and body relationships.
fn checked_value(v: &CompiledBodyValue) -> Result<(), CodecFault> {
    let declaration = decode_owned_record(&v.declaration, CHECKER_PROFILE, &v.identity)?;
    let local = decode_owned_record(&v.local, CHECKER_PROFILE, &v.identity)?;
    let settled = decode_owned_record(&v.r#final, CHECKER_PROFILE, &v.identity)?;
    if declaration.family != CheckedFamily::Declaration
        || local.family != CheckedFamily::Local
        || settled.family != CheckedFamily::Final
        || local.root != settled.root
        || local.owner != declaration.owner
        || v.body_ir.is_empty()
    {
        return Err(Malformed);
    }
    Ok(())
}
/// Require strictly increasing, nonempty dependency module names.
fn checked_deps(deps: &[String]) -> Result<(), CodecFault> {
    if deps.len() > MAX_FIELDS || deps.first().is_some_and(String::is_empty) || deps.windows(2).any(|p| p[0] >= p[1]) {
        return Err(Malformed);
    }
    Ok(())
}
/// Write one checked namespace and its ordered dependency and lowering witnesses.
fn encode_namespace(v: &CompiledNamespaceValue) -> Result<Vec<u8>, CodecFault> {
    if v.module.is_empty()
        || v.interface.is_empty()
        || v.body_ir.is_empty()
        || v.dependencies.len() > MAX_FIELDS
        || v.lowering_inputs.len() > MAX_FIELDS
    {
        return Err(Malformed);
    }
    room(
        v.interface.len()
            + v.body_ir.len()
            + v.module.len()
            + v.dependencies.iter().map(String::len).sum::<usize>()
            + v.lowering_inputs.iter().flatten().map(String::len).sum::<usize>(),
    )?;
    checked_deps(&v.dependencies)?;
    valid_pairs(&v.lowering_inputs)?;
    let mut out = Writer::new(&NAMESPACE, MAX_TRANSPORT * 2)?;
    out.record();
    out.binary(b"bodyir", &v.body_ir);
    out.list(b"deps__", v.dependencies.len())?;
    for dep in &v.dependencies {
        out.text_node(dep.to_owned())
    }
    out.binary(b"iface_", &v.interface);
    write_pairs(&mut out, &v.lowering_inputs)?;
    out.text(b"module", &v.module);
    frame(out)
}
impl CompiledBodyDependencyRead {
    /// Borrow the dependency key components in their wire order.
    fn parts(&self) -> [&str; 5] {
        [
            self.package.as_deref().unwrap_or_default(),
            &self.module,
            &self.name,
            &self.identity,
            &self.reference_subject,
        ]
    }
}
/// Validate reference names and canonical ordering across dependency reads.
fn checked_read_order(reads: &[CompiledBodyDependencyRead]) -> Result<(), CodecFault> {
    let mut previous = None;
    for read in reads {
        let parts = read.parts();
        if parts[1..].contains(&"") || !ordered(previous, parts) {
            return Err(Malformed);
        }
        previous = Some(parts);
    }
    Ok(())
}
/// Encode a checked value and its dependency reads after authenticating its three records.
fn encode_value(v: &CompiledBodyValue) -> Result<Vec<u8>, CodecFault> {
    if v.module.is_empty() || v.name.is_empty() || v.identity.is_empty() {
        return Err(Malformed);
    }
    checked_value(v)?;
    let fixed = v.declaration.len()
        + v.local.len()
        + v.r#final.len()
        + v.body_ir.len()
        + v.module.len()
        + v.name.len()
        + v.identity.len();
    room(
        fixed
            + v.dependency_reads
                .iter()
                .flat_map(CompiledBodyDependencyRead::parts)
                .map(str::len)
                .sum::<usize>(),
    )?;
    checked_read_order(&v.dependency_reads)?;
    let mut out = Writer::new(&VALUE, MAX_TRANSPORT * 2)?;
    out.record();
    out.binary(b"bodyir", &v.body_ir);
    out.binary(b"declar", &v.declaration);
    out.list(b"deps__", v.dependency_reads.len() * 6)?;
    for read in &v.dependency_reads {
        let parts = read.parts();
        out.text_node(if parts[0].is_empty() { "none" } else { "some" }.into());
        for s in parts {
            out.text_node(s.into())
        }
    }
    out.binary(b"final_", &v.r#final);
    out.text(b"ident_", &v.identity);
    out.binary(b"local_", &v.local);
    out.text(b"module", &v.module);
    out.text(b"name__", &v.name);
    frame(out)
}
/// Validate the explicit full-source decision and its optional identity.
fn check_fallback(v: &CompiledBodyFallback) -> Result<(), CodecFault> {
    if v.module.is_empty()
        || v.name.is_empty()
        || v.reason.is_empty()
        || v.reason.len() > 65536
        || v.identity.as_deref() == Some("")
    {
        return Err(Malformed);
    }
    Ok(())
}
/// Encode an independently framed full-source decision.
fn encode_fallback(v: &CompiledBodyFallback) -> Result<Vec<u8>, CodecFault> {
    check_fallback(v)?;
    let mut out = Writer::new(&FALLBACK, MAX_TRANSPORT * 2)?;
    out.record();
    out.optional_text(b"ident_", v.identity.as_deref());
    out.text(b"module", &v.module);
    out.text(b"name__", &v.name);
    out.text(b"reason", &v.reason);
    frame(out)
}
/// Emit checked namespaces, values and explicit fallbacks in their original order.
pub fn encode_compiled_body_outputs(outputs: &CompiledBodyOutputs) -> Result<Vec<Vec<u8>>, CodecFault> {
    let count = outputs.namespaces.len() + outputs.values.len() + outputs.fallbacks.len();
    if count > MAX_FIELDS {
        return Err(LimitExceeded(
            "compiled-output-frame-count".into(),
            i64::try_from(count).map_err(|_| Limit)?,
            MAX_FIELDS as i64,
        ));
    }
    let mut frames = Vec::with_capacity(count);
    let mut last = "";
    for ns in &outputs.namespaces {
        if ns.module.as_str() <= last {
            return Err(Malformed);
        }
        frames.push(encode_namespace(ns)?);
        last = &ns.module;
    }
    let mut prev = None;
    let mut owned = HashSet::new();
    for v in &outputs.values {
        let key = [v.module.as_str(), v.identity.as_str()];
        if !ordered(prev, key) {
            return Err(Malformed);
        }
        frames.push(encode_value(v)?);
        if !outputs.fallbacks.is_empty() {
            owned.insert(key);
        }
        prev = Some(key);
    }
    let mut prev = None;
    for f in &outputs.fallbacks {
        let id = f.identity.as_deref().unwrap_or_default();
        let key = [f.module.as_str(), f.name.as_str(), id];
        if !ordered(prev, key) || (!id.is_empty() && owned.contains(&[f.module.as_str(), id])) {
            return Err(Malformed);
        }
        frames.push(encode_fallback(f)?);
        prev = Some(key);
    }
    Ok(frames)
}

/// Verify the streaming delimiter and decode one complete owner.
pub fn decode_compiled_body_frame(encoded: &[u8]) -> Result<CompiledBodyOutput, CodecFault> {
    let (prefix, payload) = encoded.split_at_checked(8).ok_or(Malformed)?;
    let size = compiled_body_frame_size(prefix)?;
    if payload.len() != size as usize {
        return Err(Malformed);
    }
    decode_compiled_body_payload(payload)
}
/// Decode a previously bounded Crumb row using its admitted selector.
pub fn decode_compiled_body_payload(data: &[u8]) -> Result<CompiledBodyOutput, CodecFault> {
    if data.is_empty() || data.len() > MAX_TRANSPORT as usize {
        return Err(Limit);
    }
    let mut rows = Rows::admit(data, &[&NAMESPACE, &VALUE, &FALLBACK], MAX_TRANSPORT)?;
    if rows.len() != 1 {
        return Err(Malformed);
    }
    let (key, mut f) = rows.row(0)?;
    if key == NAMESPACE.selector {
        f.count(5)?;
        let body_ir = f.binary()?;
        let dependencies = f.texts()?;
        checked_deps(&dependencies)?;
        let interface = f.binary()?;
        let pairs = f.texts()?;
        if pairs.len() / 2 > MAX_FIELDS {
            return Err(Malformed);
        }
        let lowering_inputs = read_pairs(pairs)?;
        let module = f.text()?;
        if module.is_empty() || body_ir.is_empty() || interface.is_empty() {
            return Err(Malformed);
        }
        return Ok(CompiledBodyOutput::Namespace(CompiledNamespaceValue {
            module,
            interface,
            body_ir,
            dependencies,
            lowering_inputs,
        }));
    }
    if key == VALUE.selector {
        return Ok(CompiledBodyOutput::Value(decode_value(f)?));
    }
    if key != FALLBACK.selector {
        return Err(Malformed);
    }
    f.count(4)?;
    let v = CompiledBodyFallback {
        identity: f.optional_text()?,
        module: f.text()?,
        name: f.text()?,
        reason: f.text()?,
    };
    check_fallback(&v)?;
    Ok(CompiledBodyOutput::Fallback(v))
}

/// Reconstruct and authenticate one callable owner with its ordered dependency reads.
fn decode_value(mut f: Fields) -> Result<CompiledBodyValue, CodecFault> {
    f.count(8)?;
    let body_ir = f.binary()?;
    let declaration = f.binary()?;
    let tokens = f.texts()?;
    if tokens.len() % 6 != 0 || tokens.len() / 6 > MAX_FIELDS {
        return Err(Malformed);
    }
    let mut dependency_reads = Vec::with_capacity(tokens.len() / 6);
    let mut it = tokens.into_iter();
    while let Some(marker) = it.next() {
        let name = it.next().ok_or(Malformed)?;
        let package = match (marker.as_str(), name.is_empty()) {
            ("none", true) => None,
            ("some", false) => Some(name),
            _ => return Err(Malformed),
        };
        dependency_reads.push(CompiledBodyDependencyRead {
            package,
            module: it.next().ok_or(Malformed)?,
            name: it.next().ok_or(Malformed)?,
            identity: it.next().ok_or(Malformed)?,
            reference_subject: it.next().ok_or(Malformed)?,
        });
    }
    checked_read_order(&dependency_reads)?;
    let v = CompiledBodyValue {
        body_ir,
        declaration,
        dependency_reads,
        r#final: f.binary()?,
        identity: f.text()?,
        local: f.binary()?,
        module: f.text()?,
        name: f.text()?,
    };
    if v.module.is_empty() || v.identity.is_empty() || v.name.is_empty() {
        return Err(Malformed);
    }
    checked_value(&v)?;
    Ok(v)
}
