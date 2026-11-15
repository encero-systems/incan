//! Native Crumb row envelopes: the header, the inline/reference description dictionary, typed rows and the seal.
//!
//! This mirrors `row_reader.read_rows` (with `description_read`, `descriptions`, `description_dictionary`,
//! `metadata_read`, `selectors` and `cell_read`) and `row_writer.write_rows`. Decoding reads the literal envelope, or
//! every proper prefix when the case asks for them, and positions every refusal as the Incan reader does; encoding
//! writes the decoded descriptions and records back. Root records and typed cells live in `semantic_row_cells.rs`.

use serde_json::Value as Json;
use sha2::{Digest, Sha256};

use super::columns::{self, Budget, Cursor, Refused};
use super::hash::{self, Node, Projection, Value};
use super::row_cells::read_root;
use super::row_cursor::{Place, Positioned, Refusal, Span, at, bounded_count, read_text};

/// One complete D metadata tree.
#[derive(Clone)]
pub(super) enum Meta {
    Null,
    Boolean(bool),
    Integer(i128),
    Text(String),
    Binary(Vec<u8>),
    Symbol(String),
    List(Vec<Meta>),
    Map(Vec<(String, Meta)>),
}

/// One admitted field in identity order, with its declaration position and original metadata.
#[derive(Clone)]
pub(super) struct StoredField {
    pub(super) position: usize,
    pub(super) metadata: Meta,
    pub(super) identity: Vec<u8>,
    pub(super) storage: Node,
    pub(super) canonical: bool,
    pub(super) absent: char,
}

/// One admitted enum variant and its identity-ordered payload fields.
#[derive(Clone)]
pub(super) struct StoredVariant {
    pub(super) position: usize,
    pub(super) identity: Vec<u8>,
    pub(super) metadata: Meta,
    pub(super) fields: Vec<StoredField>,
}

/// One admitted description: its complete metadata, written mode, kind, K, D digest and members.
#[derive(Clone)]
pub(super) struct Stored {
    pub(super) metadata: Meta,
    pub(super) inline: bool,
    pub(super) record: bool,
    pub(super) key: Vec<u8>,
    pub(super) selector: Node,
    pub(super) digest: Vec<u8>,
    pub(super) fields: Vec<StoredField>,
    pub(super) variants: Vec<StoredVariant>,
    /// For a reference entry, the position of the supplied description it resolved to.
    pub(super) supplied: Option<usize>,
}

/// One decoded record: its description slot and logical value.
#[derive(Clone)]
pub(super) struct Record {
    pub(super) slot: usize,
    pub(super) value: Value,
    /// The row frame's envelope offset and length, for reading it again on request.
    pub(super) origin: usize,
    pub(super) length: usize,
}

/// A decoded envelope: the K-ordered dictionary, records, seal status and the input bytes it occupies.
#[derive(Clone)]
pub(super) struct Rows {
    pub(super) dictionary: Vec<Stored>,
    pub(super) records: Vec<Record>,
    pub(super) sealed: bool,
    pub(super) consumed: usize,
}

/// What a row read admits besides its bytes: supplied reference descriptions, registered profile kinds with their
/// record (true) or enum (false) shapes, whether the envelope must be sealed, and whether it must end the input.
pub(super) struct ReadInputs<'a> {
    pub(super) supplied: &'a [Meta],
    pub(super) kinds: &'a [(String, bool)],
    pub(super) require_sealed: bool,
    pub(super) exact: bool,
}

impl<'a> ReadInputs<'a> {
    /// A corpus case's inputs: its supplied descriptions, no profile kinds, and one exact envelope.
    fn case(supplied: &'a [Meta], document: &Json) -> Self {
        Self {
            supplied,
            kinds: &[],
            require_sealed: require_sealed(document),
            exact: true,
        }
    }
}

/// Decode a row case: one status and records, or a positioned refusal, per decoder execution.
pub(super) fn decode(document: &Json) -> Option<Vec<u8>> {
    let (data, supplied) = row_inputs(document)?;
    let lengths: Vec<usize> = if document.get("transform").is_some() {
        (0..data.len()).collect()
    } else {
        vec![data.len()]
    };
    let mut output = Vec::new();
    for length in lengths {
        let mut budget = row_budget(document)?;
        match read_rows(&data[..length], &ReadInputs::case(&supplied, document), &mut budget) {
            Ok(rows) => {
                output.push(0);
                output.push(u8::from(rows.sealed));
                hash::unsigned(&mut output, rows.records.len() as u128);
                for record in &rows.records {
                    hash::frame(&mut output, &rows.dictionary[record.slot].key);
                    hash::frame(&mut output, &hash::value_preimage(&record.value, Projection::Object)?);
                }
            }
            Err(refusal) => {
                output.push(1);
                refusal_bytes(&refusal, &mut output);
            }
        }
    }
    Some(output)
}

/// A refusal's comparison bytes: its category, envelope offset, and stored field and variant when known.
fn refusal_bytes(refusal: &Refusal, output: &mut Vec<u8>) {
    hash::frame(output, refusal.category.as_bytes());
    hash::unsigned(output, refusal.offset as u128);
    for member in [&refusal.field, &refusal.variant] {
        match member {
            Some(identity) => {
                output.push(1);
                hash::frame(output, identity);
            }
            None => output.push(0),
        }
    }
}

/// Encode a row case: decode the complete input, then write its descriptions and records back.
pub(super) fn encode(document: &Json) -> Option<Vec<u8>> {
    let (data, supplied) = row_inputs(document)?;
    let mut budget = row_budget(document)?;
    match read_rows(&data, &ReadInputs::case(&supplied, document), &mut budget) {
        Ok(rows) => super::row_write::write_rows(&rows),
        Err(_) => Some(Vec::new()),
    }
}

/// The literal input and the supplied reference descriptions.
fn row_inputs(document: &Json) -> Option<(Vec<u8>, Vec<Meta>)> {
    let data = columns::hex(document.get("input_hex")?.as_str()?)?;
    let mut supplied = Vec::new();
    if let Some(items) = document.get("supplied_descriptions") {
        for item in items.as_array()? {
            supplied.push(notation(item)?);
        }
    }
    Some((data, supplied))
}

/// Whether the case requires a sealed envelope.
fn require_sealed(document: &Json) -> bool {
    document.get("require_sealed").and_then(Json::as_bool).unwrap_or(false)
}

/// The row decoder's one allowance, tightened by the case's limits.
fn row_budget(document: &Json) -> Option<Budget> {
    let (mut bytes, mut elements, mut depth) = (16_777_216, 1_048_576, 128);
    if let Some(limits) = document.get("limits") {
        if let Some(value) = limits.get("bytes") {
            bytes = value.as_u64()?;
        }
        if let Some(value) = limits.get("elements") {
            elements = value.as_u64()?;
        }
        if let Some(value) = limits.get("depth") {
            depth = value.as_u64()?;
        }
    }
    Some(Budget::new(bytes, elements, depth))
}

/// Translate tagged metadata notation into a D tree.
pub(super) fn notation(value: &Json) -> Option<Meta> {
    Some(match value.get("kind")?.as_str()? {
        "null" => Meta::Null,
        "boolean" => Meta::Boolean(value.get("value")?.as_bool()?),
        "int" => Meta::Integer(value.get("value")?.as_str()?.parse().ok()?),
        "string" => Meta::Text(value.get("value")?.as_str()?.to_string()),
        "binary" => Meta::Binary(columns::hex(value.get("hex")?.as_str()?)?),
        "symbol" => Meta::Symbol(value.get("name")?.as_str()?.to_string()),
        "list" => Meta::List(
            value
                .get("items")?
                .as_array()?
                .iter()
                .map(notation)
                .collect::<Option<_>>()?,
        ),
        "map" => {
            let mut entries = Vec::new();
            for pair in value.get("entries")?.as_array()? {
                let pair = pair.as_array()?;
                if pair.len() != 2 || pair[0].get("kind")?.as_str()? != "string" {
                    return None;
                }
                entries.push((pair[0].get("value")?.as_str()?.to_string(), notation(&pair[1])?));
            }
            Meta::Map(entries)
        }
        _ => return None,
    })
}

/// Check the header and frame, then admit the dictionary, every row and any seal; an exact read refuses any bytes
/// after the envelope, and a prefix read reports how many it occupies.
pub(super) fn read_rows(data: &[u8], inputs: &ReadInputs, budget: &mut Budget) -> Positioned<Rows> {
    let mut cursor = Span::new(data, 0);
    let magic = cursor.take(4, budget)?;
    budget.bytes(4).place(0)?;
    if magic != b"CRMB" {
        return Err(at("bad_magic", 0));
    }
    let (codec, profile) = (cursor.length(budget)?, cursor.length(budget)?);
    if codec != 1 || profile != 1 {
        return Err(at("unsupported_profile", 4));
    }
    let flags = cursor.byte(budget)?;
    if flags & 254 != 0 {
        return Err(at("reserved_flags", cursor.absolute() - 1));
    }
    let payload = cursor.frame(budget)?;
    let payload_origin = cursor.absolute() - payload.len();
    if inputs.exact {
        cursor.finished()?;
    }
    let sealed = flags == 1;
    if inputs.require_sealed && !sealed {
        return Err(at("seal_required", 6));
    }
    let mut body = Span::new(payload, payload_origin);
    let dictionary = read_entries(&mut body, inputs.supplied, inputs.kinds, budget)?;
    let records = read_records(&mut body, &dictionary, sealed, budget)?;
    body.finished()?;
    Ok(Rows {
        dictionary,
        records,
        sealed,
        consumed: cursor.consumed(),
    })
}

/// Read every row, feeding object contributions to the seal, then verify its count and digest.
fn read_records(
    cursor: &mut Span,
    dictionary: &[Stored],
    sealed: bool,
    budget: &mut Budget,
) -> Positioned<Vec<Record>> {
    let count_offset = cursor.absolute();
    let count = cursor.length(budget)?;
    budget.elements(count as u64).place(count_offset)?;
    if count > cursor.remaining() {
        return Err(at("truncated", cursor.absolute() + cursor.remaining()));
    }
    let mut seal = b"crumb1\x00\x04".to_vec();
    hash::unsigned(&mut seal, count as u128);
    if sealed {
        hash::charge_seal_start(count, budget).place(count_offset)?;
    }
    let mut records = Vec::with_capacity(count);
    for _ in 0..count {
        let offset = cursor.absolute();
        let slot = cursor.length(budget)?;
        if slot >= dictionary.len() {
            return Err(at("description_index", offset));
        }
        let raw = cursor.frame(budget)?;
        let origin = cursor.absolute() - raw.len();
        let value = read_root(&dictionary[slot], raw, origin, dictionary, budget)?;
        if sealed {
            let mut contribution = Vec::new();
            hash::emit_object_charged(&value, &mut contribution, budget).place(origin)?;
            hash::frame(&mut seal, &contribution);
        }
        budget.bytes(64).place(origin)?;
        records.push(Record {
            slot,
            value,
            origin,
            length: raw.len(),
        });
    }
    if sealed {
        let offset = cursor.absolute();
        if cursor.length(budget)? != count {
            return Err(at("seal_count", offset));
        }
        let expected = copied(cursor.take(32, budget)?, budget).place(offset)?;
        hash::charge_seal_digest(budget).place(offset)?;
        if Sha256::digest(&seal).as_slice() != expected {
            return Err(at("seal_mismatch", offset));
        }
    }
    Ok(records)
}

/// Read a column envelope's dictionary from its body cursor, keeping only a refusal's category.
pub(super) fn read_dictionary(
    cursor: &mut Cursor,
    supplied: &[Meta],
    budget: &mut Budget,
) -> Result<Vec<Stored>, Refused> {
    let mut body = Span::new(cursor.unread(), 0);
    let entries = read_entries(&mut body, supplied, &[], budget).map_err(|refusal| refusal.category)?;
    cursor.skip(body.consumed());
    Ok(entries)
}

/// Read the dictionary in strict K order, admitting inline and supplied reference descriptions.
fn read_entries(
    cursor: &mut Span,
    supplied: &[Meta],
    kinds: &[(String, bool)],
    budget: &mut Budget,
) -> Positioned<Vec<Stored>> {
    let count = bounded_count(cursor, budget)?;
    let mut entries: Vec<Stored> = Vec::with_capacity(count);
    for index in 0..count {
        let offset = cursor.absolute();
        let mut resolved = None;
        let (source, inline) = match cursor.byte(budget)? {
            0 => {
                let view = cursor.frame(budget)?;
                (
                    inline_description(view, cursor.absolute() - view.len(), kinds, budget)?,
                    true,
                )
            }
            1 => {
                let key = copied(cursor.frame(budget)?, budget).place(offset)?;
                parse_key(&key, true, budget)
                    .map_err(|kind| invalid_as(kind, "invalid_selector"))
                    .place(offset)?;
                let digest = copied(cursor.take(32, budget)?, budget).place(offset)?;
                let (position, source) = referenced(&key, &digest, supplied, kinds, budget).place(offset)?;
                resolved = Some(position);
                (source, false)
            }
            _ => return Err(at("unsupported_profile", offset)),
        };
        validate_selectors(&source, kinds, budget).place(offset)?;
        let mut checked = admit(source, inline, kinds, budget)
            .map_err(|kind| invalid_as(kind, "invalid_description"))
            .place(offset)?;
        checked.supplied = resolved;
        budget.bytes(2 * checked.key.len() as u64).place(offset)?;
        if entries.iter().any(|entry| entry.key == checked.key) {
            return Err(at("duplicate_selector", offset));
        }
        if index > 0 && hash::hex_string(&checked.key) < hash::hex_string(&entries[index - 1].key) {
            return Err(at("description_order", offset));
        }
        entries.push(checked);
    }
    settle_dictionary(&entries, kinds, budget).place(cursor.absolute())?;
    Ok(entries)
}

/// Repeat ordinary dictionary admission and account for its ordered index before following dependencies.
fn settle_dictionary(entries: &[Stored], kinds: &[(String, bool)], budget: &mut Budget) -> Result<(), Refused> {
    for entry in entries {
        budget.elements(1)?;
        budget.bytes(16)?;
        admit(entry.metadata.clone(), entry.inline, kinds, budget)?;
        budget.elements(1)?;
        budget.bytes(2 * entry.key.len() as u64)?;
    }
    for entry in entries {
        charge_copy(&entry.metadata, budget)?;
    }
    check_dictionary(entries, budget)
}

/// Admit a writer's complete D descriptions, preserve their requested modes, and order them by exact K.
pub(super) fn admit_written(
    sources: Vec<(Meta, bool)>,
    kinds: &[(String, bool)],
    budget: &mut Budget,
) -> Result<Vec<Stored>, Refused> {
    let mut entries = Vec::with_capacity(sources.len());
    for (metadata, inline) in sources {
        budget.elements(1)?;
        budget.bytes(16)?;
        let entry = admit(metadata, inline, kinds, budget)?;
        budget.elements(1)?;
        budget.bytes(2 * entry.key.len() as u64)?;
        if entries.iter().any(|known: &Stored| known.key == entry.key) {
            return Err("duplicate_selector");
        }
        entries.push(entry);
    }
    entries.sort_by(|left, right| left.key.cmp(&right.key));
    for entry in &entries {
        charge_copy(&entry.metadata, budget)?;
    }
    check_dictionary(&entries, budget)?;
    Ok(entries)
}

/// Every K and field selector must name descriptions the dictionary holds, with the same record or enum kind.
fn check_dictionary(entries: &[Stored], budget: &mut Budget) -> Result<(), Refused> {
    for entry in entries {
        check_references(&entry.selector, entries, budget).map_err(|kind| invalid_as(kind, "invalid_selector"))?;
        for field in entry
            .fields
            .iter()
            .chain(entry.variants.iter().flat_map(|variant| &variant.fields))
        {
            check_references(&field.storage, entries, budget).map_err(|kind| invalid_as(kind, "invalid_selector"))?;
        }
    }
    Ok(())
}

/// Keep resource limits, and replace any other primitive failure with the boundary's category.
fn invalid_as(kind: Refused, fallback: Refused) -> Refused {
    if matches!(kind, "truncated" | "noncanonical_integer" | "integer_overflow") {
        return kind;
    }
    if kind == "limit_exceeded" { kind } else { fallback }
}

/// Every named field dependency must exist in the dictionary with the same record or enum kind.
fn check_references(node: &Node, entries: &[Stored], budget: &mut Budget) -> Result<(), Refused> {
    budget.elements(1)?;
    budget.enter()?;
    let result = match node {
        Node::Nominal(kind, _, arguments) => {
            let key =
                hash::closed_selector_charged(node, budget).map_err(|kind| invalid_as(kind, "invalid_selector"))?;
            budget.elements(1)?;
            budget.bytes(2 * key.len() as u64)?;
            let entry = entries
                .iter()
                .find(|entry| entry.key == key)
                .ok_or("invalid_selector")?;
            if entry.record != (kind == "record") {
                return Err("invalid_selector");
            }
            arguments
                .iter()
                .try_for_each(|argument| check_references(argument, entries, budget))
        }
        Node::Container(_, arguments) => arguments
            .iter()
            .try_for_each(|argument| check_references(argument, entries, budget)),
        _ => Ok(()),
    };
    budget.leave();
    result
}

/// Resolve the exact supplied K and complete D digest.
fn referenced(
    key: &[u8],
    digest: &[u8],
    supplied: &[Meta],
    kinds: &[(String, bool)],
    budget: &mut Budget,
) -> Result<(usize, Meta), Refused> {
    let mut found_key = false;
    for (position, candidate) in supplied.iter().enumerate() {
        budget.elements(1)?;
        let Some(Meta::Binary(raw_key)) = get(candidate, "nominal") else {
            return Err("invalid_description");
        };
        if raw_key != key {
            continue;
        }
        found_key = true;
        validate_selectors(candidate, kinds, budget)?;
        let parsed =
            admit(candidate.clone(), false, kinds, budget).map_err(|kind| invalid_as(kind, "invalid_description"))?;
        if parsed.digest == digest {
            return Ok((position, candidate.clone()));
        }
    }
    Err(if found_key {
        "description_mismatch"
    } else {
        "missing_description"
    })
}

/// Recover an inline description: its framed header plus the identity-ordered member tables.
fn inline_description(view: &[u8], origin: usize, kinds: &[(String, bool)], budget: &mut Budget) -> Positioned<Meta> {
    let mut cursor = Span::new(view, origin);
    let header_view = cursor.frame(budget)?;
    let header = read_metadata(header_view, cursor.absolute() - header_view.len(), budget)?;
    let record = record_kind(&header, kinds, budget)
        .place(origin)?
        .ok_or_else(|| at("invalid_description", origin))?;
    let members = read_members(&mut cursor, !record, budget)?;
    cursor.finished()?;
    with_members(header, if record { "fields" } else { "variants" }, members, budget).place(origin)
}

/// Read a member table: declaration positions form a permutation and identities ascend strictly.
fn read_members(cursor: &mut Span, variants: bool, budget: &mut Budget) -> Positioned<Vec<Meta>> {
    let count = bounded_count(cursor, budget)?;
    let mut members: Vec<Option<Meta>> = vec![None; count];
    let mut previous: Option<Vec<u8>> = None;
    for _ in 0..count {
        let offset = cursor.absolute();
        let position = cursor.length(budget)?;
        if position >= count || members[position].is_some() {
            return Err(at("member_position", offset));
        }
        let view = cursor.frame(budget)?;
        let origin = cursor.absolute() - view.len();
        let member = if variants {
            read_variant(view, origin, budget)?
        } else {
            read_metadata(view, origin, budget)?
        };
        let Some(Meta::Binary(identity)) = get_charged(&member, "id", budget).place(offset)? else {
            return Err(at("invalid_description", offset));
        };
        if identity.len() != 6 {
            return Err(at("invalid_description", offset));
        }
        budget.bytes(12).place(offset)?;
        if members
            .iter()
            .flatten()
            .any(|seen| matches!(get(seen, "id"), Some(Meta::Binary(other)) if other == identity))
        {
            return Err(at("duplicate_member", offset));
        }
        if previous.as_ref().is_some_and(|previous| identity < previous) {
            return Err(at("member_order", offset));
        }
        previous = Some(identity.clone());
        members[position] = Some(member);
    }
    let ordered: Vec<Meta> = members.into_iter().flatten().collect();
    for member in &ordered {
        charge_copy(member, budget).place(cursor.absolute())?;
    }
    Ok(ordered)
}

/// An enum variant: a framed header plus its anonymous payload field table.
fn read_variant(view: &[u8], origin: usize, budget: &mut Budget) -> Positioned<Meta> {
    let mut cursor = Span::new(view, origin);
    let header_view = cursor.frame(budget)?;
    let header = read_metadata(header_view, cursor.absolute() - header_view.len(), budget)?;
    let fields = read_members(&mut cursor, false, budget)?;
    cursor.finished()?;
    with_members(header, "fields", fields, budget).place(origin)
}

/// Append the recovered member list to a header that must not already contain it.
fn with_members(header: Meta, name: &str, members: Vec<Meta>, budget: &mut Budget) -> Result<Meta, Refused> {
    let Meta::Map(mut entries) = header else {
        return Err("invalid_description");
    };
    if entries.iter().any(|(key, _)| key == name) {
        return Err("invalid_description");
    }
    for (key, value) in &entries {
        charge_text(key, budget)?;
        charge_copy(value, budget)?;
    }
    entries.push((name.to_string(), Meta::List(members)));
    Ok(Meta::Map(entries))
}

/// Check K and every field N before admission, keeping malformed selectors distinguishable.
fn validate_selectors(source: &Meta, kinds: &[(String, bool)], budget: &mut Budget) -> Result<(), Refused> {
    let record = record_kind(source, kinds, budget)?.ok_or("invalid_description")?;
    let Some(Meta::Binary(key)) = get_charged(source, "nominal", budget)? else {
        return Err("invalid_description");
    };
    parse_key(key, record, budget).map_err(|kind| invalid_as(kind, "invalid_selector"))?;
    let tables: Vec<&Meta> = if record {
        vec![source]
    } else {
        let Some(Meta::List(variants)) = get_charged(source, "variants", budget)? else {
            return Err("invalid_description");
        };
        variants.iter().collect()
    };
    for table in tables {
        let Some(Meta::List(fields)) = get_charged(table, "fields", budget)? else {
            return Err("invalid_description");
        };
        for field in fields {
            let Some(Meta::Text(name)) = get_charged(field, "type", budget)? else {
                return Err("invalid_description");
            };
            parse_type(name, budget).map_err(|kind| invalid_as(kind, "invalid_selector"))?;
        }
    }
    Ok(())
}

/// One named property of a metadata map.
fn get<'a>(value: &'a Meta, name: &str) -> Option<&'a Meta> {
    let Meta::Map(entries) = value else { return None };
    entries.iter().find(|(key, _)| key == name).map(|(_, value)| value)
}

/// Read one metadata property through the same scan and retained-copy charges as `metadata_values.get`.
fn get_charged<'a>(value: &'a Meta, name: &str, budget: &mut Budget) -> Result<Option<&'a Meta>, Refused> {
    let Meta::Map(entries) = value else { return Ok(None) };
    for (key, value) in entries {
        budget.elements(1)?;
        if key == name {
            charge_copy(value, budget)?;
            return Ok(Some(value));
        }
    }
    Ok(None)
}

/// Copy retained bytes under the same allowance as `encoding_cursor.copied`.
fn copied(view: &[u8], budget: &mut Budget) -> Result<Vec<u8>, Refused> {
    budget.bytes(view.len() as u64)?;
    Ok(view.to_vec())
}

/// Reserve a UTF-8 upper bound for one retained metadata name or text value.
fn charge_text(text: &str, budget: &mut Budget) -> Result<(), Refused> {
    (0..4).try_for_each(|_| budget.bytes(text.len() as u64))
}

/// Bound one complete retained metadata subtree before copying it.
fn charge_copy(value: &Meta, budget: &mut Budget) -> Result<(), Refused> {
    budget.elements(1)?;
    budget.enter()?;
    let result = match value {
        Meta::Null | Meta::Boolean(_) => budget.bytes(1),
        Meta::Integer(_) => budget.bytes(16),
        Meta::Text(text) | Meta::Symbol(text) => charge_text(text, budget),
        Meta::Binary(data) => budget.bytes(data.len() as u64),
        Meta::List(children) => children.iter().try_for_each(|child| charge_copy(child, budget)),
        Meta::Map(entries) => entries.iter().try_for_each(|(key, child)| {
            budget.elements(1)?;
            charge_text(key, budget)?;
            charge_copy(child, budget)
        }),
    };
    budget.leave();
    result
}

/// Whether a description's kind has the record (true) or enum (false) shape: Record and Enum are built in, a
/// registered profile kind has its registered shape, and any other kind has none.
fn record_kind(metadata: &Meta, kinds: &[(String, bool)], budget: &mut Budget) -> Result<Option<bool>, Refused> {
    let Some(Meta::Text(kind)) = get_charged(metadata, "kind", budget)? else {
        return Ok(None);
    };
    budget.elements(1)?;
    charge_text(kind, budget)?;
    Ok(match kind.as_str() {
        "Record" => Some(true),
        "Enum" => Some(false),
        _ => kinds.iter().find(|(name, _)| name == kind).map(|(_, record)| *record),
    })
}

/// Admit one complete description: its D digest, kind, K, path, properties and member tables.
fn admit(metadata: Meta, inline: bool, kinds: &[(String, bool)], budget: &mut Budget) -> Result<Stored, Refused> {
    let mut encoded = b"crumb1\x00\x05".to_vec();
    budget.bytes(8)?;
    encode_metadata(&metadata, &mut encoded, budget)?;
    let digest = Sha256::digest(&encoded).to_vec();
    budget.bytes(32)?;
    let record = record_kind(&metadata, kinds, budget)?.ok_or("invalid_description")?;
    let Some(Meta::Binary(key)) = get_charged(&metadata, "nominal", budget)? else {
        return Err("invalid_description");
    };
    let key = key.clone();
    let selector = parse_key(&key, record, budget)?;
    if !matches!(get_charged(&metadata, "path", budget)?, Some(Meta::Text(_)))
        || !matches!(get_charged(&metadata, "properties", budget)?, Some(Meta::Map(_)))
    {
        return Err("invalid_description");
    }
    let (mut fields, mut variants) = (Vec::new(), Vec::new());
    if record {
        let Some(Meta::List(items)) = get_charged(&metadata, "fields", budget)? else {
            return Err("invalid_description");
        };
        fields = admit_fields(items, budget)?;
    } else {
        let Some(Meta::List(items)) = get_charged(&metadata, "variants", budget)? else {
            return Err("invalid_description");
        };
        variants = admit_variants(items, budget)?;
    }
    charge_copy(&metadata, budget)?;
    Ok(Stored {
        metadata,
        inline,
        record,
        key,
        selector,
        digest,
        fields,
        variants,
        supplied: None,
    })
}

/// Admit a field table and order it by identity, keeping declaration positions.
fn admit_fields(values: &[Meta], budget: &mut Budget) -> Result<Vec<StoredField>, Refused> {
    let mut fields: Vec<StoredField> = Vec::with_capacity(values.len());
    for (position, value) in values.iter().enumerate() {
        budget.elements(1)?;
        budget.bytes(28)?;
        charge_copy(value, budget)?;
        let identity = member_identity(value, budget)?;
        if fields.iter().any(|field| field.identity == identity) {
            return Err("invalid_description");
        }
        let (Some(Meta::Text(name)), Some(Meta::Boolean(canonical)), Some(Meta::Text(absent))) = (
            get_charged(value, "type", budget)?,
            get_charged(value, "canonical", budget)?,
            get_charged(value, "absent", budget)?,
        ) else {
            return Err("invalid_description");
        };
        let storage = parse_type(name, budget)?;
        let absent = match absent.as_str() {
            "required" => 'r',
            "none" => 'n',
            "empty" => 'e',
            _ => return Err("invalid_description"),
        };
        let allowed = match (absent, &storage) {
            ('r', _) => true,
            ('n', Node::Container(kind, _)) => kind == "option",
            ('e', Node::Container(kind, _)) => matches!(kind.as_str(), "list" | "set" | "map"),
            _ => false,
        };
        if !allowed {
            return Err("invalid_description");
        }
        if !matches!(get_charged(value, "name", budget)?, Some(Meta::Text(_)))
            || !matches!(get_charged(value, "properties", budget)?, Some(Meta::Map(_)))
        {
            return Err("invalid_description");
        }
        fields.push(StoredField {
            position,
            metadata: value.clone(),
            identity,
            storage,
            canonical: *canonical,
            absent,
        });
    }
    fields.sort_by(|left, right| left.identity.cmp(&right.identity));
    for field in &fields {
        charge_copy(&field.metadata, budget)?;
        budget.bytes(16)?;
    }
    Ok(fields)
}

/// Admit variants and their payload tables, ordered by identity.
fn admit_variants(values: &[Meta], budget: &mut Budget) -> Result<Vec<StoredVariant>, Refused> {
    let mut variants: Vec<StoredVariant> = Vec::with_capacity(values.len());
    for (position, value) in values.iter().enumerate() {
        budget.elements(1)?;
        budget.bytes(28)?;
        charge_copy(value, budget)?;
        let identity = member_identity(value, budget)?;
        if variants.iter().any(|variant| variant.identity == identity) {
            return Err("invalid_description");
        }
        let Some(Meta::List(items)) = get_charged(value, "fields", budget)? else {
            return Err("invalid_description");
        };
        let fields = admit_fields(items, budget)?;
        variants.push(StoredVariant {
            position,
            identity,
            metadata: value.clone(),
            fields,
        });
    }
    variants.sort_by(|left, right| left.identity.cmp(&right.identity));
    for variant in &variants {
        charge_copy(&variant.metadata, budget)?;
    }
    Ok(variants)
}

/// A member's six-octet identity, with its name and properties map checked.
fn member_identity(value: &Meta, budget: &mut Budget) -> Result<Vec<u8>, Refused> {
    match (
        get_charged(value, "id", budget)?,
        get_charged(value, "name", budget)?,
        get_charged(value, "properties", budget)?,
    ) {
        (Some(Meta::Binary(identity)), Some(Meta::Text(_)), Some(Meta::Map(_))) if identity.len() == 6 => {
            Ok(identity.clone())
        }
        _ => Err("invalid_description"),
    }
}

/// Read a complete K: identity, argument count and framed canonical argument spellings.
pub(super) fn parse_key(key: &[u8], record: bool, budget: &mut Budget) -> Result<Node, Refused> {
    if key.len() < 7 {
        return Err("invalid_cell");
    }
    budget.bytes(key.len() as u64)?;
    let mut cursor = Cursor::new(&key[6..]);
    let count = cursor.length(budget).map_err(|kind| invalid_as(kind, "invalid_cell"))?;
    if count > cursor.remaining() {
        return Err("invalid_cell");
    }
    let mut arguments = Vec::with_capacity(count);
    for _ in 0..count {
        budget.elements(1)?;
        let raw = cursor.frame(budget).map_err(|kind| invalid_as(kind, "invalid_cell"))?;
        let spelling = std::str::from_utf8(raw).map_err(|_| "invalid_cell")?;
        arguments.push(parse_type(spelling, budget)?);
    }
    if cursor.remaining() != 0 {
        return Err("invalid_cell");
    }
    let kind = if record { "record" } else { "enum" };
    let node = Node::Nominal(kind.to_string(), key[..6].to_vec(), arguments);
    if hash::closed_selector_charged(&node, budget)? != key {
        return Err("invalid_cell");
    }
    Ok(node)
}

/// Parse one complete canonical selector spelling N.
fn parse_type(text: &str, budget: &mut Budget) -> Result<Node, Refused> {
    budget.bytes(4 * text.len() as u64)?;
    let mut position = 0;
    let node = type_node(text.as_bytes(), &mut position, budget)?;
    if position != text.len() || hash::charged_selector_spelling(&node, budget)?.as_str() != text {
        return Err("invalid_cell");
    }
    Ok(node)
}

/// Parse one selector node, consuming shared depth.
fn type_node(text: &[u8], position: &mut usize, budget: &mut Budget) -> Result<Node, Refused> {
    budget.elements(1)?;
    budget.enter()?;
    let result = type_node_inner(text, position, budget);
    budget.leave();
    result
}

/// Parse constructors, decimals and nominal identities; final spelling admits scalar names.
fn type_node_inner(text: &[u8], position: &mut usize, budget: &mut Budget) -> Result<Node, Refused> {
    let start = *position;
    while *position < text.len() && !matches!(text[*position], b'<' | b'>' | b',') {
        budget.elements(1)?;
        *position += 1;
    }
    if *position == start {
        return Err("invalid_cell");
    }
    let name = std::str::from_utf8(&text[start..*position])
        .map_err(|_| "invalid_cell")?
        .to_string();
    budget.bytes(4 * name.len() as u64)?;
    if name == "decimal" {
        let mut parts = Vec::new();
        for delimiter in *b"<,>" {
            if text.get(*position) != Some(&delimiter) {
                return Err("invalid_cell");
            }
            *position += 1;
            if delimiter != b'>' {
                let begin = *position;
                while *position < text.len() && text[*position].is_ascii_digit() {
                    budget.elements(1)?;
                    *position += 1;
                }
                let digits = std::str::from_utf8(&text[begin..*position]).map_err(|_| "invalid_cell")?;
                budget.bytes(4 * digits.len() as u64)?;
                budget.elements(digits.len() as u64)?;
                parts.push(digits.parse::<i64>().map_err(|_| "invalid_cell")?);
            }
        }
        return Ok(Node::Decimal(parts[0], parts[1]));
    }
    let mut arguments = Vec::new();
    if text.get(*position) == Some(&b'<') {
        *position += 1;
        if text.get(*position) == Some(&b'>') {
            *position += 1;
        } else {
            loop {
                arguments.push(type_node(text, position, budget)?);
                match text.get(*position) {
                    Some(b'>') => {
                        *position += 1;
                        break;
                    }
                    Some(b',') => *position += 1,
                    _ => return Err("invalid_cell"),
                }
            }
        }
    }
    if let Some((kind, identity)) = name.split_once('@')
        && matches!(kind, "record" | "enum")
    {
        budget.bytes(6)?;
        let identity = columns::hex(identity)
            .filter(|bytes| bytes.len() == 6)
            .ok_or("invalid_cell")?;
        return Ok(Node::Nominal(kind.to_string(), identity, arguments));
    }
    match name.as_str() {
        "ref" if arguments.len() == 1 => Ok(Node::Reference(Box::new(arguments.remove(0)))),
        "list" | "set" | "option" | "map" | "tuple" | "union" => Ok(Node::Container(name, arguments)),
        _ if arguments.is_empty() && hash::SCALARS.contains(&name.as_str()) => Ok(Node::Scalar(name)),
        _ => Err("invalid_cell"),
    }
}

/// Decode one complete D-encoded description a caller supplies, outside any request's allowance.
pub(super) fn read_description(encoded: &[u8]) -> Option<Meta> {
    let mut unlimited = Budget::new(u64::MAX, u64::MAX, u64::MAX);
    read_metadata(encoded, 0, &mut unlimited).ok()
}

/// Read one complete D frame, which begins `origin` bytes into the envelope, with no trailing bytes.
fn read_metadata(view: &[u8], origin: usize, budget: &mut Budget) -> Positioned<Meta> {
    let mut cursor = Span::new(view, origin);
    let value = read_node(&mut cursor, budget)?;
    cursor.finished()?;
    Ok(value)
}

/// Read one tagged D node under the shared depth allowance.
fn read_node(cursor: &mut Span, budget: &mut Budget) -> Positioned<Meta> {
    let offset = cursor.absolute();
    budget.elements(1).place(offset)?;
    budget.enter().place(offset)?;
    let result = read_node_inner(cursor, budget);
    budget.leave();
    result
}

/// Decode one D tag and payload; map keys must already be unique and ascending.
fn read_node_inner(cursor: &mut Span, budget: &mut Budget) -> Positioned<Meta> {
    let offset = cursor.absolute();
    Ok(match cursor.byte(budget)? {
        0 => Meta::Null,
        1 => match cursor.byte(budget)? {
            flag @ (0 | 1) => Meta::Boolean(flag == 1),
            _ => return Err(at("invalid_description", offset)),
        },
        2 => Meta::Integer(columns::unzigzag(&cursor.groups(budget)?).ok_or_else(|| at("integer_overflow", offset))?),
        3 => Meta::Text(read_text(cursor, budget)?),
        4 => {
            let raw = cursor.frame(budget)?;
            Meta::Binary(copied(raw, budget).place(offset)?)
        }
        5 => {
            let count = bounded_count(cursor, budget)?;
            Meta::List(
                (0..count)
                    .map(|_| read_node(cursor, budget))
                    .collect::<Result<_, _>>()?,
            )
        }
        6 => read_map(cursor, budget)?,
        7 => Meta::Symbol(read_text(cursor, budget)?),
        _ => return Err(at("invalid_description", offset)),
    })
}

/// Read a D map whose property-name keys must already be unique and in UTF-8 byte order.
fn read_map(cursor: &mut Span, budget: &mut Budget) -> Positioned<Meta> {
    let count = bounded_count(cursor, budget)?;
    let mut entries: Vec<(String, Meta)> = Vec::with_capacity(count);
    for _ in 0..count {
        let offset = cursor.absolute();
        let key = read_text(cursor, budget)?;
        budget.bytes(8 * key.len() as u64).place(offset)?;
        if entries
            .last()
            .is_some_and(|(previous, _)| key.as_bytes() <= previous.as_bytes())
        {
            return Err(at("invalid_description", offset));
        }
        let value = read_node(cursor, budget)?;
        entries.push((key, value));
    }
    Ok(Meta::Map(entries))
}

/// Encode D: tags, sorted unique property-name map keys and exact framing.
pub(super) fn encode_metadata(value: &Meta, output: &mut Vec<u8>, budget: &mut Budget) -> Result<(), Refused> {
    budget.elements(1)?;
    budget.enter()?;
    let result = encode_metadata_inner(value, output, budget);
    budget.leave();
    result
}

/// Encode one D node.
fn encode_metadata_inner(value: &Meta, output: &mut Vec<u8>, budget: &mut Budget) -> Result<(), Refused> {
    match value {
        Meta::Null => hash_write(output, &[0], budget)?,
        Meta::Boolean(flag) => hash_write(output, &[1, u8::from(*flag)], budget)?,
        Meta::Integer(number) => {
            hash_write(output, &[2], budget)?;
            hash_zigzag(output, number.unsigned_abs(), *number < 0, budget)?;
        }
        Meta::Text(text) => {
            hash_write(output, &[3], budget)?;
            hash_text(output, text, budget)?;
        }
        Meta::Binary(data) => {
            hash_write(output, &[4], budget)?;
            hash_frame(output, data, budget)?;
        }
        Meta::Symbol(name) => {
            hash_write(output, &[7], budget)?;
            hash_text(output, name, budget)?;
        }
        Meta::List(items) => {
            hash_write(output, &[5], budget)?;
            hash_unsigned(output, items.len(), budget)?;
            for item in items {
                encode_metadata(item, output, budget)?;
            }
        }
        Meta::Map(entries) => {
            let mut keyed: Vec<&(String, Meta)> = entries.iter().collect();
            for (key, _) in entries {
                budget.elements(1)?;
                budget.bytes(16)?;
                budget.bytes(12 * key.len() as u64)?;
            }
            keyed.sort_by(|left, right| left.0.as_bytes().cmp(right.0.as_bytes()));
            if keyed.windows(2).any(|pair| pair[0].0 == pair[1].0) || !keyed.iter().all(|(key, _)| property_name(key)) {
                return Err("invalid_description");
            }
            hash_write(output, &[6], budget)?;
            hash_unsigned(output, keyed.len(), budget)?;
            for (key, item) in keyed {
                hash_text(output, key, budget)?;
                encode_metadata(item, output, budget)?;
            }
        }
    }
    Ok(())
}

/// Charge bytes before feeding them to one metadata hash output.
fn hash_write(output: &mut Vec<u8>, data: &[u8], budget: &mut Budget) -> Result<(), Refused> {
    budget.bytes(data.len() as u64)?;
    output.extend_from_slice(data);
    Ok(())
}

/// Encode a shortest unsigned value, charging its codec work and its output write.
fn hash_unsigned(output: &mut Vec<u8>, value: usize, budget: &mut Budget) -> Result<(), Refused> {
    let encoded = crate::crumb::encode_native_unsigned(value as u128);
    budget.elements(encoded.element_charge)?;
    budget.bytes(encoded.byte_charge)?;
    hash_write(output, &encoded.data, budget)
}

/// Frame known bytes through the charged metadata output.
fn hash_frame(output: &mut Vec<u8>, data: &[u8], budget: &mut Budget) -> Result<(), Refused> {
    hash_unsigned(output, data.len(), budget)?;
    hash_write(output, data, budget)
}

/// Charge a UTF-8 upper bound before framing one metadata text value or map key.
fn hash_text(output: &mut Vec<u8>, text: &str, budget: &mut Budget) -> Result<(), Refused> {
    budget.bytes(4 * text.len() as u64)?;
    hash_frame(output, text.as_bytes(), budget)
}

/// Encode one signed metadata integer, charging its codec work and output write.
fn hash_zigzag(output: &mut Vec<u8>, magnitude: u128, negative: bool, budget: &mut Budget) -> Result<(), Refused> {
    let mut encoded = Vec::new();
    hash::zigzag(&mut encoded, magnitude, negative);
    budget.elements(encoded.len() as u64)?;
    budget.bytes(encoded.len() as u64)?;
    hash_write(output, &encoded, budget)
}

/// One identifier-shaped property name.
fn property_name(value: &str) -> bool {
    crate::unicode_names::is_xid_name(value) && crate::unicode_names::first_nfc_difference(value) < 0
}
