//! Graph executable provenance and nominal-scope attachments around the shared checked Body IR.

use super::{Cell, Project, read_fields, restore_executable_module_value, to_atom, to_cell};
use incan_semantics_core::body_ir::{Body, BodyIrModule, SharedIdentities};
use incan_semantics_core::executable_representation::{
    EXECUTABLE_REPRESENTATION_VERSION, EncodedOwnerRepresentation, MAX_OWNER_COORDINATES, executable_owner_scopes,
    executable_scope_directory,
};
use incan_semantics_core::{CompilerNodeId, encode_incan_symbol_identity};
use internal_models::NamespaceAtom;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// Borrowed graph provenance captured from the same activation that built this checked owner.
pub struct ExecutableOwnerInputs<'a> {
    /// Canonical module reached by the checked backend.
    pub module: &'a CompilerNodeId,
    /// Project-relative source path supplying the declaration.
    pub source: &'a str,
    /// Stable selected graph identity of the owner.
    pub identity: &'a str,
    /// Exact physical-input evidence when this body qualifies for scalar placement.
    pub physical_inputs: Option<[u8; 32]>,
    /// Stable syntax-path locations. The optional physical witness authenticates them.
    pub coordinates: &'a [(u32, u32)],
    /// The existing typed receipt for any checked executable calls made by this owner.
    pub call_witness: Option<&'a [u8]>,
    /// Complete digest of the independently published nonempty scope in this owner module.
    pub scope_digest: Option<&'a str>,
}

/// A typed executable owner with its nominal scope reference ready for snapshot-bound restoration.
pub struct OpenExecutableOwner {
    /// Native checked owner and exact physical provenance, with the outer nominal scope left empty.
    pub frame: EncodedOwnerRepresentation,
    /// Scope digest authenticated through the selected graph snapshot, or the empty scope when absent.
    pub scope_digest: Option<String>,
}

/// One checked module's deduplicated nominal scope values and their ordered owner attachments.
pub struct ExecutableScopeRecords {
    /// One digest per first-use slot, with no separate record for the empty map.
    pub digests: Vec<Option<String>>,
    /// The original shared maps in the same first-use order as their wire digests.
    pub bindings: Vec<SharedIdentities>,
    /// Scope slots in the order of bodies followed by model, enum and alias declarations.
    pub slots: Vec<usize>,
    /// Exact typed source-module records by complete value digest.
    pub values: BTreeMap<String, Vec<u8>>,
}

impl ExecutableScopeRecords {
    /// Serialize each distinct nonempty map once for the matching namespace and standalone owners.
    pub fn pack(source: &str, module: &mut BodyIrModule) -> Result<Self, String> {
        let source = source.to_owned();
        let (maps, slots) = executable_scope_directory(module);
        let mut digests = Vec::with_capacity(maps.len());
        let mut values = BTreeMap::new();
        for map in &maps {
            if map.is_empty() {
                digests.push(None);
                continue;
            }
            let NamespaceAtom::Mapping(entries) = to_atom(map.project()) else {
                return Err("expected checked executable scope mapping".into());
            };
            let encoded = internal_models::encode_executable_scope(&source, &entries)?;
            let digest = hex::encode(Sha256::digest(&encoded));
            if values.insert(digest.clone(), encoded).is_some() {
                return Err("distinct executable scopes share an encoded identity".into());
            }
            digests.push(Some(digest));
        }
        Ok(Self {
            digests,
            bindings: maps,
            slots,
            values,
        })
    }
}

/// Bind a selected scope value to its exact digest and owning module before restoring its immutable map.
pub fn open_graph_executable_scope(encoded: &[u8], source: &str, digest: &str) -> Result<SharedIdentities, String> {
    if hex::encode(Sha256::digest(encoded)) != digest {
        return Err("checked executable scope disagrees with its selected digest".into());
    }
    let entries = internal_models::decode_executable_scope(&encoded.to_vec(), &source.to_owned())?;
    restore_executable_scope_entries(entries)
}

/// Keep the selected scope encoded in the leased store view until decoding its requested bindings.
pub fn open_mapped_graph_executable_scope(
    reader: &internal_models::PreparedWireReader,
    view: &internal_models::MappedView,
    source: &str,
    digest: &str,
) -> Result<SharedIdentities, String> {
    if hex::encode(Sha256::digest(view.0.as_bytes())) != digest {
        return Err("checked executable scope disagrees with its selected digest".into());
    }
    let entries = internal_models::decode_executable_scope_view(reader, view, &source.to_owned())?;
    restore_executable_scope_entries(entries)
}

/// Restore only a nonempty nominal map because an empty map has no independent scope record.
fn restore_executable_scope_entries(entries: Vec<internal_models::NamespaceEntry>) -> Result<SharedIdentities, String> {
    let scope = SharedIdentities::restore(to_cell(NamespaceAtom::Mapping(entries))?)?;
    if scope.is_empty() {
        return Err("the empty executable scope does not have a separate record".into());
    }
    Ok(scope)
}

/// Read the exact six-byte Crumb member id as the projection ABI's integer.
const fn field(name: &[u8; 6]) -> u64 {
    u64::from_be_bytes([0, 0, name[0], name[1], name[2], name[3], name[4], name[5]])
}

const OWNER_FIELDS: [u64; 11] = [
    field(b"bodyir"),
    field(b"calls_"),
    field(b"canon_"),
    field(b"coords"),
    field(b"ident_"),
    field(b"module"),
    field(b"name__"),
    field(b"physic"),
    field(b"reader"),
    field(b"scope_"),
    field(b"source"),
];
const NAMESPACE_FIELDS: [u64; 4] = [field(b"bodyir"), field(b"scopes"), field(b"slots_"), field(b"source")];

/// Keep the full digest of a scope distinct from missing scope authority.
fn valid_scope_digest(digest: &str) -> bool {
    digest.len() == 64
        && digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

/// Reject incomplete or oversized physical placement rather than allowing its coordinates to become authority.
fn validate_coordinates(physical_inputs: Option<[u8; 32]>, coordinates: &[(u32, u32)]) -> Result<(), String> {
    if coordinates.len() > MAX_OWNER_COORDINATES
        || (!coordinates.is_empty() && physical_inputs.is_none())
        || coordinates.iter().any(|(start, end)| start > end)
    {
        return Err("invalid checked executable owner coordinates".into());
    }
    Ok(())
}

/// Read the graph's canonical lowering identity from the checked producer, leaving absence explicit.
fn lowering_identity(body: &Body) -> String {
    body.canonical
        .as_ref()
        .map(encode_incan_symbol_identity)
        .unwrap_or_default()
}

/// Both modes require the current source, stable owner and an executable Body with no retired ABI payload.
fn validate_owner_provenance(input: &ExecutableOwnerInputs<'_>, body: &Body) -> Result<(), String> {
    if input.source.is_empty() || input.identity.is_empty() || body.retired_rust_facade_wire.is_some() {
        return Err("invalid checked executable owner provenance".into());
    }
    Ok(())
}

/// Encode an owner and its physical attachments. Only the outer scope is moved out of the body for projection.
pub fn encode_graph_executable_owner(input: ExecutableOwnerInputs<'_>, body: &mut Body) -> Result<Vec<u8>, String> {
    validate_owner_provenance(&input, body)?;
    let scope_present = !body.named_type_identities.is_empty();
    if input.scope_digest.is_some() != scope_present
        || input.scope_digest.is_some_and(|digest| !valid_scope_digest(digest))
    {
        return Err("checked executable owner scope disagrees with its digest".into());
    }
    validate_coordinates(input.physical_inputs, input.coordinates)?;
    let scope = std::mem::take(&mut body.named_type_identities);
    let body_value = body.project();
    body.named_type_identities = scope;
    encode_owner_projection(input, body, body_value)
}

/// Project the private callable's checked nominal bindings inline through the existing authored owner codec.
pub(crate) fn encode_inline_executable_owner(
    module: &CompilerNodeId,
    source: &str,
    identity: &str,
    body: &Body,
) -> Result<Vec<u8>, String> {
    let input = ExecutableOwnerInputs {
        module,
        source,
        identity,
        physical_inputs: None,
        coordinates: &[],
        call_witness: None,
        scope_digest: None,
    };
    validate_owner_provenance(&input, body)?;
    encode_owner_projection(input, body, body.project())
}

/// Assemble the shared owner root after the caller supplies its exact checked nominal scope placement.
fn encode_owner_projection(input: ExecutableOwnerInputs<'_>, body: &Body, body_value: Cell) -> Result<Vec<u8>, String> {
    let values = [
        body_value,
        input.call_witness.map(<[u8]>::to_vec).project(),
        Cell::Text(lowering_identity(body)),
        <(u32, u32)>::project_list(input.coordinates),
        Cell::Text(input.identity.to_owned()),
        input.module.project(),
        body.name.project(),
        input.physical_inputs.project(),
        Cell::Text(body.direct_call_id.path().to_owned()),
        input.scope_digest.map(str::to_owned).project(),
        Cell::Text(input.source.to_owned()),
    ];
    internal_models::encode_executable_owner_frame(&to_atom(Cell::Record(
        OWNER_FIELDS.into_iter().zip(values).collect(),
    )))
}

/// The caller chooses where its checked nominal bindings live before interpreting any owner fields.
enum OwnerScope {
    Graph,
    Inline,
}

/// Admit one full checked owner, including its settled call witness, without looking up unselected scope values.
pub fn open_graph_executable_owner(encoded: &[u8]) -> Result<OpenExecutableOwner, String> {
    decode_owner_projection(
        internal_models::decode_executable_owner_frame(&encoded.to_vec())?,
        OwnerScope::Graph,
    )
}

/// Admit the caller's standalone value and refuse graph-only attachments, even for an empty inline scope.
pub(crate) fn open_inline_executable_owner(encoded: &Vec<u8>) -> Result<EncodedOwnerRepresentation, String> {
    let opened = decode_owner_projection(
        internal_models::decode_executable_owner_frame(encoded)?,
        OwnerScope::Inline,
    )?;
    Ok(opened.frame)
}

/// Restore the existing shared provenance and the Body in the scope placement chosen by the caller.
fn decode_owner_projection(value: NamespaceAtom, placement: OwnerScope) -> Result<OpenExecutableOwner, String> {
    let Cell::Record(fields) = to_cell(value)? else {
        return Err("expected checked executable owner".into());
    };
    let [
        body,
        calls,
        canonical,
        coordinates,
        identity,
        module,
        name,
        physical,
        reader,
        scope,
        source,
    ] = read_fields(fields, OWNER_FIELDS)?;
    let body = Body::restore(body)?;
    let source = String::restore(source)?;
    let identity = String::restore(identity)?;
    let reader = String::restore(reader)?;
    let call_witness = Option::<Vec<u8>>::restore(calls)?;
    let scope_digest = Option::<String>::restore(scope)?;
    let coordinates = Vec::<(u32, u32)>::restore(coordinates)?;
    let physical_inputs = Option::<[u8; 32]>::restore(physical)?;
    if source.is_empty()
        || identity.is_empty()
        || String::restore(name)? != body.name
        || reader != body.direct_call_id.path()
        || String::restore(canonical)? != lowering_identity(&body)
    {
        return Err("checked executable owner disagrees with its provenance".into());
    }
    match placement {
        OwnerScope::Graph
            if !body.named_type_identities.is_empty()
                || scope_digest
                    .as_deref()
                    .is_some_and(|digest| !valid_scope_digest(digest)) =>
        {
            return Err("checked executable owner disagrees with its provenance".into());
        }
        OwnerScope::Inline
            if scope_digest.is_some()
                || call_witness.is_some()
                || physical_inputs.is_some()
                || !coordinates.is_empty() =>
        {
            return Err("standalone callable owner contains graph attachments".into());
        }
        _ => {}
    }
    validate_coordinates(physical_inputs, &coordinates)?;
    if let Some(witness) = &call_witness {
        internal_models::decode_owner_call_reads(&identity, &reader, &source, witness)?;
    }
    Ok(OpenExecutableOwner {
        frame: EncodedOwnerRepresentation {
            version: EXECUTABLE_REPRESENTATION_VERSION,
            module: CompilerNodeId::restore(module)?,
            source,
            identity,
            physical_inputs,
            coordinates,
            call_witness,
            body,
        },
        scope_digest,
    })
}

/// Validate first-use order and cardinality without materializing or opening any scope.
fn validate_scope_slots(scopes: &[Option<String>], slots: &[usize]) -> Result<(), String> {
    let mut used = 0;
    for &slot in slots {
        if slot >= scopes.len() || slot > used {
            return Err("executable namespace names an invalid nominal scope slot".into());
        }
        if slot == used {
            used += 1;
        }
    }
    let unique: BTreeSet<_> = scopes.iter().collect();
    if used != scopes.len()
        || unique.len() != scopes.len()
        || unique.into_iter().flatten().any(|digest| !valid_scope_digest(digest))
    {
        return Err("executable namespace has an invalid nominal scope directory".into());
    }
    Ok(())
}

/// Share directory scopes across exactly the outer bodies, models, enums and aliases of a selected module.
pub fn encode_graph_executable_namespace(
    source: &str,
    module: &mut BodyIrModule,
    scopes: &[Option<String>],
    slots: &[usize],
) -> Result<Vec<u8>, String> {
    validate_scope_slots(scopes, slots)?;
    let empty = SharedIdentities::default();
    let original: Vec<_> = executable_owner_scopes(module)
        .map(|scope| std::mem::replace(scope, Arc::clone(&empty)))
        .collect();
    let valid = !source.is_empty()
        && original.len() == slots.len()
        && original
            .iter()
            .zip(slots)
            .all(|(scope, slot)| scopes[*slot].is_some() != scope.is_empty());
    let body = valid.then(|| module.project());
    for (scope, original) in executable_owner_scopes(module).zip(original) {
        *scope = original;
    }
    let body = body.ok_or("executable namespace scope directory does not cover its exact owners")?;
    let values = [
        body,
        Option::<String>::project_list(scopes),
        usize::project_list(slots),
        Cell::Text(source.to_owned()),
    ];
    internal_models::encode_executable_namespace_frame(&to_atom(Cell::Record(
        NAMESPACE_FIELDS.into_iter().zip(values).collect(),
    )))
}

/// Install the exact namespace's independently admitted scope map into each checked owner before lowering.
pub fn restore_graph_executable_namespace(
    value: NamespaceAtom,
    expected_source: &str,
    mut resolve: impl FnMut(&str) -> Result<SharedIdentities, String>,
) -> Result<BodyIrModule, String> {
    let Cell::Record(fields) = to_cell(value)? else {
        return Err("expected checked executable namespace".into());
    };
    let [body, scopes, slots, source] = read_fields(fields, NAMESPACE_FIELDS)?;
    let source = String::restore(source)?;
    if source.is_empty() || source != expected_source {
        return Err("executable namespace belongs to another source module".into());
    }
    let mut module = restore_executable_module_value(body)?;
    let scopes = Vec::<Option<String>>::restore(scopes)?;
    let slots = Vec::<usize>::restore(slots)?;
    validate_scope_slots(&scopes, &slots)?;
    if executable_owner_scopes(&mut module).count() != slots.len()
        || executable_owner_scopes(&mut module).any(|scope| !scope.is_empty())
    {
        return Err("executable namespace has conflicting scope ownership".into());
    }
    let maps = scopes
        .into_iter()
        .map(|scope| match scope {
            Some(digest) => resolve(&digest),
            None => Ok(SharedIdentities::default()),
        })
        .collect::<Result<Vec<_>, _>>()?;
    for (scope, slot) in executable_owner_scopes(&mut module).zip(slots) {
        *scope = Arc::clone(&maps[slot]);
    }
    Ok(module)
}

/// Check scoped owner and namespace restoration using the same checked input as the codec and package controls.
#[cfg(test)]
pub(super) fn verify_executable_frames(mut module: BodyIrModule) -> Result<(), String> {
    let source = "src/domain.incn";
    let identity = "function:domain::total";
    let packed = ExecutableScopeRecords::pack(source, &mut module)?;
    let body = module.bodies.first_mut().ok_or("missing executable frame test body")?;
    let slot = packed.slots.first().ok_or("missing executable owner scope slot")?;
    let scope = packed.digests.get(*slot).and_then(Option::as_deref);
    let coords = [(1, 7), (9, 12)];
    let reader = body.direct_call_id.path().to_owned();
    let witness = internal_models::encode_owner_call_reads(identity, &reader, source, &[])?;
    let frame = encode_graph_executable_owner(
        ExecutableOwnerInputs {
            module: &module.module_id,
            source,
            identity,
            physical_inputs: Some([17; 32]),
            coordinates: &coords,
            call_witness: Some(&witness),
            scope_digest: scope,
        },
        body,
    )?;
    let mut opened = open_graph_executable_owner(&frame)?;
    assert_eq!(opened.scope_digest.as_deref(), scope);
    assert_eq!(opened.frame.physical_inputs, Some([17; 32]));
    assert_eq!(opened.frame.coordinates, coords);
    assert_eq!(opened.frame.call_witness.as_deref(), Some(witness.as_slice()));
    assert!(opened.frame.body.named_type_identities.is_empty());
    if let Some(digest) = scope {
        let value = packed.values.get(digest).ok_or("missing executable scope value")?;
        assert!(open_graph_executable_scope(value, "src/other.incn", digest).is_err());
        assert!(open_graph_executable_scope(value, source, "different-digest").is_err());
        opened.frame.body.named_type_identities = open_graph_executable_scope(value, source, digest)?;
    }
    assert_eq!(&opened.frame.body, body);
    assert!(open_graph_executable_owner(&frame[..frame.len() / 2]).is_err());
    let bytes = encode_graph_executable_namespace(source, &mut module, &packed.digests, &packed.slots)?;
    assert!(internal_models::decode_executable_namespace_frame(&frame).is_err());
    let value = internal_models::decode_executable_namespace_frame(&bytes)?;
    let mut reads = BTreeSet::new();
    let restored = restore_graph_executable_namespace(value, source, |digest| {
        assert!(reads.insert(digest.to_owned()));
        let bytes = packed.values.get(digest).ok_or("missing namespace test scope")?;
        open_graph_executable_scope(bytes, source, digest)
    })?;
    assert_eq!(reads.len(), packed.values.len());
    assert_eq!(restored, module);
    Ok(())
}
