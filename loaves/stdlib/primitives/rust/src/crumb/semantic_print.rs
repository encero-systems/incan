//! Native canonical COMIC printing from the corpus's typed document notation.
//!
//! This mirrors `std.primitives.crumb.text_printer.print_comic` over the descriptions and values the Incan corpus
//! adapter builds: descriptions first in path order, then consecutive root values of one type as a table.

use serde_json::Value as Json;
use sha2::{Digest, Sha256};

use super::super::{binary_literal, quote_text, spaces};

/// One admitted text type; nominal names stay paths, exactly as the text grammar resolves them.
#[derive(Clone, PartialEq)]
pub(super) enum Type {
    Scalar(String),
    Nominal(String),
    Decimal(i64, i64),
    List(Box<Type>),
    Set(Box<Type>),
    Option(Box<Type>),
    Map(Box<Type>, Box<Type>),
    Union(Vec<Type>),
    Reference(Box<Type>),
}

/// One inert declaration property value.
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

/// One declared field: its name-derived identity, storage type and absence facts.
pub(super) struct FieldShape {
    pub(super) identity: Vec<u8>,
    pub(super) storage: Type,
    pub(super) canonical: bool,
    pub(super) absent: char,
    pub(super) name: String,
    pub(super) renamed: Vec<String>,
}

/// One declared enum variant.
pub(super) struct Variant {
    pub(super) identity: Vec<u8>,
    pub(super) fields: Vec<FieldShape>,
    pub(super) name: String,
    pub(super) renamed: Vec<String>,
}

/// One text declaration with its kind, shape and ordered members.
pub(super) struct Description {
    pub(super) path: String,
    pub(super) kind: String,
    pub(super) record: bool,
    pub(super) properties: Vec<(String, Meta)>,
    pub(super) fields: Vec<FieldShape>,
    pub(super) variants: Vec<Variant>,
}

/// One registered profile kind's printing order facts.
pub(super) struct Registration {
    pub(super) kind: String,
    pub(super) record: bool,
    pub(super) property_order: Option<Vec<String>>,
    pub(super) groups: Vec<(String, Vec<String>)>,
}

/// One logical text value, retaining exact integer spellings and float bits.
pub(super) enum Item {
    Absent,
    Some(Box<Item>),
    Boolean(bool),
    Integer(String),
    Float(i64, Vec<u8>),
    Decimal(String, i64),
    Text(String),
    Binary(Vec<u8>),
    Withheld,
    List(Vec<Item>),
    Set(Vec<Item>),
    Map(Vec<(Item, Item)>),
    Record(Vec<(Vec<u8>, Item)>),
    Enum(Vec<u8>, Vec<(Vec<u8>, Item)>),
    Union(Type, Box<Item>),
    Reference(Vec<u8>, Vec<u8>),
}

/// Print one text-print corpus document canonically.
pub(super) fn text_print(document: &Json) -> Option<Vec<u8>> {
    let registrations = registrations(document.get("context"))?;
    let source = document.get("document")?;
    let mut descriptions = Vec::new();
    for item in source.get("descriptions")?.as_array()? {
        descriptions.push(description(item, &registrations)?);
    }
    let mut roots = Vec::new();
    for item in source.get("values")?.as_array()? {
        let path = item.get("type")?.as_str()?;
        let owner = single(&descriptions, path)?;
        roots.push((Type::Nominal(owner.path.clone()), value(item, &descriptions)?));
    }
    print(&descriptions, &roots, &registrations).map(String::into_bytes)
}

/// Translate the case's explicit local kind registrations.
pub(super) fn registrations(context: Option<&Json>) -> Option<Vec<Registration>> {
    let Some(context) = context else {
        return Some(Vec::new());
    };
    let mut result = Vec::new();
    for item in context.get("kind_registrations")?.as_array()? {
        let property_order = match item.get("property_order") {
            Some(order) if !order.is_null() => Some(strings(order)?),
            _ => None,
        };
        let mut groups = Vec::new();
        if let Some(values) = item.get("property_groups") {
            for (name, order) in values.as_object()? {
                groups.push((name.clone(), strings(order)?));
            }
        }
        result.push(Registration {
            kind: item.get("kind")?.as_str()?.to_string(),
            record: match item.get("shape")?.as_str()? {
                "Record" => true,
                "Enum" => false,
                _ => return None,
            },
            property_order,
            groups,
        });
    }
    Some(result)
}

/// Require an ordered array of strings.
fn strings(value: &Json) -> Option<Vec<String>> {
    value
        .as_array()?
        .iter()
        .map(|item| item.as_str().map(str::to_string))
        .collect()
}

/// Resolve a kind's shape through the core kinds and the case's registrations.
pub(super) fn kind_shape(kind: &str, registrations: &[Registration]) -> Option<bool> {
    match kind {
        "Record" => Some(true),
        "Enum" => Some(false),
        _ => registrations
            .iter()
            .rev()
            .find(|item| item.kind == kind)
            .map(|item| item.record),
    }
}

/// Return a registered kind's ordering facts, none for a core kind, and refuse an unknown kind.
pub(super) fn registration<'a>(kind: &str, registrations: &'a [Registration]) -> Option<Option<&'a Registration>> {
    kind_shape(kind, registrations)?;
    if matches!(kind, "Record" | "Enum") {
        return Some(None);
    }
    Some(registrations.iter().rev().find(|item| item.kind == kind))
}

/// Translate one declaration without deriving any order from expected text.
fn description(value: &Json, registrations: &[Registration]) -> Option<Description> {
    let kind = value.get("kind")?.as_str()?.to_string();
    let record = kind_shape(&kind, registrations)?;
    let mut properties = Vec::new();
    if let Some(items) = value.get("properties") {
        for item in items.as_array()? {
            properties.push((item.get("name")?.as_str()?.to_string(), meta(item.get("value")?)?));
        }
    }
    let mut fields = Vec::new();
    let mut variants = Vec::new();
    if record {
        for item in value.get("fields")?.as_array()? {
            fields.push(field_shape(item)?);
        }
    } else {
        for item in value.get("variants")?.as_array()? {
            let mut payload = Vec::new();
            for field in item.get("fields")?.as_array()? {
                payload.push(field_shape(field)?);
            }
            let name = item.get("name")?.as_str()?.to_string();
            variants.push(Variant {
                identity: member_identity(&name),
                fields: payload,
                name,
                renamed: Vec::new(),
            });
        }
    }
    Some(Description {
        path: value.get("path")?.as_str()?.to_string(),
        kind,
        record,
        properties,
        fields,
        variants,
    })
}

/// Translate one field's type, absence and canonical facts; its identity derives from its earliest name.
fn field_shape(value: &Json) -> Option<FieldShape> {
    let name = value.get("name")?.as_str()?.to_string();
    Some(FieldShape {
        identity: member_identity(&name),
        storage: parse_type(value.get("type")?.as_str()?)?,
        canonical: value.get("canonical")?.as_bool()?,
        absent: match value.get("absent")?.as_str()? {
            "required" => 'r',
            "none" => 'n',
            "empty" => 'e',
            _ => return None,
        },
        name,
        renamed: Vec::new(),
    })
}

/// The six-octet member identity of an exact UTF-8 name.
pub(super) fn member_identity(name: &str) -> Vec<u8> {
    Sha256::digest(name.as_bytes())[..6].to_vec()
}

/// Translate one closed property value.
fn meta(value: &Json) -> Option<Meta> {
    Some(match value.get("kind")?.as_str()? {
        "symbol" => Meta::Symbol(value.get("name")?.as_str()?.to_string()),
        "string" => Meta::Text(value.get("value")?.as_str()?.to_string()),
        "boolean" => Meta::Boolean(value.get("value")?.as_bool()?),
        "int" => Meta::Integer(value.get("value")?.as_str()?.parse().ok()?),
        "null" => Meta::Null,
        "binary" => Meta::Binary(hex(value.get("hex")?.as_str()?)?),
        "list" => Meta::List(
            value
                .get("items")?
                .as_array()?
                .iter()
                .map(meta)
                .collect::<Option<_>>()?,
        ),
        "property_group" => {
            let mut entries = Vec::new();
            for item in value.get("properties")?.as_array()? {
                entries.push((item.get("name")?.as_str()?.to_string(), meta(item.get("value")?)?));
            }
            Meta::Map(entries)
        }
        _ => return None,
    })
}

/// Decode strict hexadecimal octets.
fn hex(value: &str) -> Option<Vec<u8>> {
    if !value.len().is_multiple_of(2) {
        return None;
    }
    (0..value.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(value.get(index..index + 2)?, 16).ok())
        .collect()
}

/// Find exactly one declaration at a path, whatever its kind.
fn single<'a>(descriptions: &'a [Description], path: &str) -> Option<&'a Description> {
    let mut found = descriptions.iter().filter(|item| item.path == path);
    let first = found.next()?;
    found.next().is_none().then_some(first)
}

/// Translate one fully tagged text value through the independent descriptions.
fn value(value: &Json, descriptions: &[Description]) -> Option<Item> {
    Some(match value.get("kind")?.as_str()? {
        "none" => Item::Absent,
        "some" => Item::Some(Box::new(self::value(value.get("value")?, descriptions)?)),
        "boolean" => Item::Boolean(value.get("value")?.as_bool()?),
        "int" => Item::Integer(integer_spelling(value)?),
        "float" => Item::Float(value.get("width")?.as_i64()?, hex(value.get("bits")?.as_str()?)?),
        "decimal" => Item::Decimal(
            value.get("coefficient")?.as_str()?.parse::<i128>().ok()?.to_string(),
            value.get("scale")?.as_i64()?,
        ),
        "string" => Item::Text(value.get("value")?.as_str()?.to_string()),
        "binary" => Item::Binary(hex(value.get("hex")?.as_str()?)?),
        "withheld" => Item::Withheld,
        "list" => Item::List(values(value.get("items")?, descriptions)?),
        "set" => Item::Set(values(value.get("items")?, descriptions)?),
        "map" => {
            let mut entries = Vec::new();
            for pair in value.get("entries")?.as_array()? {
                let pair = pair.as_array()?;
                if pair.len() != 2 {
                    return None;
                }
                entries.push((
                    self::value(&pair[0], descriptions)?,
                    self::value(&pair[1], descriptions)?,
                ));
            }
            Item::Map(entries)
        }
        "record" => {
            let owner = single(descriptions, value.get("type")?.as_str()?)?;
            let source = value.get("fields")?;
            Item::Record(stored_fields(&owner.fields, source, descriptions)?)
        }
        "enum" => {
            let owner = single(descriptions, value.get("type")?.as_str()?)?;
            let name = value.get("variant")?.as_str()?;
            let variant = owner.variants.iter().find(|item| item.name == name)?;
            let payload = stored_fields(&variant.fields, value.get("fields")?, descriptions)?;
            Item::Enum(variant.identity.clone(), payload)
        }
        "union" => {
            let name = value.get("member")?.as_str()?;
            let member = if SCALARS.contains(&name) || name == "reference" {
                Type::Scalar(name.to_string())
            } else {
                Type::Nominal(single(descriptions, name)?.path.clone())
            };
            Item::Union(member, Box::new(self::value(value.get("value")?, descriptions)?))
        }
        "reference" => {
            single(descriptions, value.get("target_type")?.as_str()?)?;
            Item::Reference(Vec::new(), value.get("identity")?.as_str()?.as_bytes().to_vec())
        }
        _ => return None,
    })
}

/// Translate an ordered value sequence.
fn values(items: &Json, descriptions: &[Description]) -> Option<Vec<Item>> {
    items.as_array()?.iter().map(|item| value(item, descriptions)).collect()
}

/// Order a fixture's named fields by their declaration, keyed by identity.
fn stored_fields(declared: &[FieldShape], source: &Json, descriptions: &[Description]) -> Option<Vec<(Vec<u8>, Item)>> {
    let mut result = Vec::with_capacity(declared.len());
    for field in declared {
        result.push((field.identity.clone(), value(source.get(&field.name)?, descriptions)?));
    }
    Some(result)
}

/// Parse an exact integer the way the fixture adapter does and spell it back in decimal.
fn integer_spelling(value: &Json) -> Option<String> {
    let spelling = value.get("value")?.as_str()?;
    let unsigned = value
        .get("type")
        .and_then(Json::as_str)
        .is_some_and(|kind| kind.starts_with('u'));
    if unsigned {
        return Some(spelling.parse::<u128>().ok()?.to_string());
    }
    match spelling.parse::<i128>() {
        Ok(number) => Some(number.to_string()),
        Err(_) => Some(spelling.parse::<u128>().ok()?.to_string()),
    }
}

/// Parse one COMIC type spelling: unions of primaries with constructors and an optional suffix.
fn parse_type(spelling: &str) -> Option<Type> {
    let tokens = tokenize(spelling)?;
    let mut index = 0;
    let result = union_type(&tokens, &mut index)?;
    (index == tokens.len()).then_some(result)
}

/// Split a type spelling into dotted words and single punctuation marks.
fn tokenize(spelling: &str) -> Option<Vec<String>> {
    let mut tokens = Vec::new();
    let mut word = String::new();
    for character in spelling.chars() {
        if character.is_alphanumeric() || character == '_' || character == '.' {
            word.push(character);
            continue;
        }
        if !word.is_empty() {
            tokens.push(std::mem::take(&mut word));
        }
        match character {
            '<' | '>' | ',' | '|' | '?' => tokens.push(character.to_string()),
            ' ' => {}
            _ => return None,
        }
    }
    if !word.is_empty() {
        tokens.push(word);
    }
    Some(tokens)
}

/// Parse one or more `|`-separated primaries.
fn union_type(tokens: &[String], index: &mut usize) -> Option<Type> {
    let mut members = vec![primary(tokens, index)?];
    while accept(tokens, index, "|") {
        members.push(primary(tokens, index)?);
    }
    if members.len() == 1 {
        return members.pop();
    }
    Some(Type::Union(members))
}

/// Parse one scalar, named type or parameterized core constructor, then an optional `?`.
fn primary(tokens: &[String], index: &mut usize) -> Option<Type> {
    let name = tokens.get(*index)?.clone();
    if !name.chars().next()?.is_alphanumeric() && !name.starts_with('_') {
        return None;
    }
    *index += 1;
    let mut result = match name.as_str() {
        "decimal" => {
            expect(tokens, index, "<")?;
            let precision: i64 = tokens.get(*index)?.parse().ok()?;
            *index += 1;
            expect(tokens, index, ",")?;
            let scale: i64 = tokens.get(*index)?.parse().ok()?;
            *index += 1;
            expect(tokens, index, ">")?;
            if precision <= 0 || precision > 38 || scale < 0 || scale > precision {
                return None;
            }
            Type::Decimal(precision, scale)
        }
        "ref" => Type::Reference(Box::new(argument(tokens, index)?)),
        "list" => Type::List(Box::new(argument(tokens, index)?)),
        "set" => Type::Set(Box::new(argument(tokens, index)?)),
        "option" => Type::Option(Box::new(argument(tokens, index)?)),
        "map" => {
            expect(tokens, index, "<")?;
            let key = union_type(tokens, index)?;
            expect(tokens, index, ",")?;
            let item = union_type(tokens, index)?;
            expect(tokens, index, ">")?;
            Type::Map(Box::new(key), Box::new(item))
        }
        scalar if SCALARS.contains(&scalar) => Type::Scalar(name),
        _ => Type::Nominal(name),
    };
    if accept(tokens, index, "?") {
        result = Type::Option(Box::new(result));
    }
    Some(result)
}

/// The scalar spellings that are not parsed as named types.
const SCALARS: [&str; 17] = [
    "boolean",
    "i8",
    "i16",
    "i32",
    "i64",
    "i128",
    "u8",
    "u16",
    "u32",
    "u64",
    "u128",
    "fp32",
    "fp64",
    "string",
    "binary",
    "secret_string",
    "secret_bytes",
];

/// Parse one `<type>` constructor argument.
fn argument(tokens: &[String], index: &mut usize) -> Option<Type> {
    expect(tokens, index, "<")?;
    let item = union_type(tokens, index)?;
    expect(tokens, index, ">")?;
    Some(item)
}

/// Consume one punctuation mark when it is next.
fn accept(tokens: &[String], index: &mut usize, mark: &str) -> bool {
    let found = tokens.get(*index).is_some_and(|token| token == mark);
    if found {
        *index += 1;
    }
    found
}

/// Require one punctuation mark.
fn expect(tokens: &[String], index: &mut usize, mark: &str) -> Option<()> {
    accept(tokens, index, mark).then_some(())
}

/// Return the canonical spelling of one admitted text type.
pub(super) fn type_spelling(value: &Type) -> String {
    match value {
        Type::Scalar(kind) => kind.clone(),
        Type::Nominal(path) => path.clone(),
        Type::Decimal(precision, scale) => format!("decimal<{precision}, {scale}>"),
        Type::List(item) => format!("list<{}>", type_spelling(item)),
        Type::Set(item) => format!("set<{}>", type_spelling(item)),
        Type::Map(key, item) => format!("map<{}, {}>", type_spelling(key), type_spelling(item)),
        Type::Option(item) if matches!(**item, Type::Option(_)) => format!("option<{}>", type_spelling(item)),
        Type::Option(item) => format!("{}?", type_spelling(item)),
        Type::Union(items) => items.iter().map(type_spelling).collect::<Vec<_>>().join(" | "),
        Type::Reference(item) => format!("ref<{}>", type_spelling(item)),
    }
}

/// Print descriptions in path order, then consecutive same-type roots as one group.
pub(super) fn print(
    descriptions: &[Description],
    roots: &[(Type, Item)],
    registrations: &[Registration],
) -> Option<String> {
    let mut entries = Vec::new();
    for description in ordered_descriptions(descriptions) {
        let registered = registration(&description.kind, registrations)?;
        entries.push(render_description(description, registered)?);
    }
    let mut index = 0;
    while index < roots.len() {
        if !matches!(roots[index].1, Item::Record(_) | Item::Enum(..)) {
            return None;
        }
        let storage = type_spelling(&roots[index].0);
        let mut end = index + 1;
        while end < roots.len() && type_spelling(&roots[end].0) == storage {
            end += 1;
        }
        entries.push(render_root_group(&roots[index..end], &storage, descriptions)?);
        index = end;
    }
    Some(entries.join("\n\n") + "\n")
}

/// Insert each description before the first already placed one whose key is not smaller.
fn ordered_descriptions(descriptions: &[Description]) -> Vec<&Description> {
    let key = |description: &Description| format!("{}\\u{{0}}{}", description.path, description.kind);
    let mut result: Vec<&Description> = Vec::with_capacity(descriptions.len());
    for description in descriptions {
        let own = key(description);
        let selected = result
            .iter()
            .position(|placed| key(placed) >= own)
            .unwrap_or(result.len());
        result.insert(selected, description);
    }
    result
}

/// Render one description inline when it fits in 120 columns, otherwise one member per line.
fn render_description(description: &Description, registered: Option<&Registration>) -> Option<String> {
    let mut items = Vec::new();
    let order = registered.and_then(|item| item.property_order.as_deref());
    for (key, value) in ordered_properties(&description.properties, order) {
        items.push(render_property(key, value, registered));
    }
    if description.record {
        for field in &description.fields {
            items.push(render_field(field)?);
        }
    } else {
        for variant in &description.variants {
            items.push(render_variant(variant)?);
        }
    }
    let prefix = format!("{} = {} ", description.path, description.kind);
    let inline = format!("{prefix}{{ {} }}", items.join(", "));
    if inline.len() <= 120 {
        return Some(inline);
    }
    let lines: Vec<String> = items.iter().map(|item| format!("    {item}")).collect();
    Some(format!("{prefix}{{\n{}\n}}", lines.join("\n")))
}

/// Apply a declared property order, retaining unknown properties in read order afterward.
fn ordered_properties<'a>(properties: &'a [(String, Meta)], order: Option<&[String]>) -> Vec<(&'a str, &'a Meta)> {
    let all = || properties.iter().map(|(key, value)| (key.as_str(), value));
    let Some(names) = order else {
        return all().collect();
    };
    let mut result: Vec<(&str, &Meta)> = Vec::new();
    for name in names {
        result.extend(all().filter(|(key, _)| key == name));
    }
    result.extend(all().filter(|(key, _)| !names.iter().any(|name| name == key)));
    result
}

/// Render one variant and its named payload fields.
fn render_variant(variant: &Variant) -> Option<String> {
    let mut result = variant.name.clone();
    if !variant.renamed.is_empty() {
        result.push_str(&format!(" [renamed_from = [{}]]", variant.renamed.join(", ")));
    }
    if variant.fields.is_empty() {
        return Some(result);
    }
    let fields = variant.fields.iter().map(render_field).collect::<Option<Vec<_>>>()?;
    Some(format!("{result}({})", fields.join(", ")))
}

/// Render field metadata, storage type and explicit absent value.
fn render_field(field: &FieldShape) -> Option<String> {
    let mut result = field.name.clone();
    let mut metadata = Vec::new();
    if !field.renamed.is_empty() {
        metadata.push(format!("renamed_from = [{}]", field.renamed.join(", ")));
    }
    if !field.canonical {
        metadata.push("canonical = false".to_string());
    }
    if !metadata.is_empty() {
        result.push_str(&format!(" [{}]", metadata.join(", ")));
    }
    result.push_str(": ");
    result.push_str(&type_spelling(&field.storage));
    match (field.absent, &field.storage) {
        ('n', _) => result.push_str(" = None"),
        ('e', Type::List(_)) => result.push_str(" = []"),
        ('e', Type::Set(_) | Type::Map(..)) => result.push_str(" = {}"),
        ('e', _) => return None,
        _ => {}
    }
    Some(result)
}

/// Render one property, ordering a registered nested group when known.
fn render_property(key: &str, value: &Meta, registered: Option<&Registration>) -> String {
    let group = registered.and_then(|item| item.groups.iter().rev().find(|(name, _)| name == key));
    let order = group.map(|(_, order)| order.as_slice());
    format!("{key} = {}", render_property_value(value, order, registered))
}

/// Render closed property values without resolving symbols.
fn render_property_value(value: &Meta, order: Option<&[String]>, registered: Option<&Registration>) -> String {
    match value {
        Meta::Null => "None".to_string(),
        Meta::Symbol(name) => name.clone(),
        Meta::Text(text) => quote_text(text.clone()),
        Meta::Binary(data) => binary_literal(data.clone()),
        Meta::Boolean(flag) => flag.to_string(),
        Meta::Integer(number) => number.to_string(),
        Meta::List(items) => {
            let rendered: Vec<String> = items
                .iter()
                .map(|item| render_property_value(item, None, registered))
                .collect();
            format!("[{}]", rendered.join(", "))
        }
        Meta::Map(entries) => {
            let rendered: Vec<String> = ordered_properties(entries, order)
                .into_iter()
                .map(|(key, item)| render_property(key, item, registered))
                .collect();
            format!("{{ {} }}", rendered.join(", "))
        }
    }
}

/// Render one root value alone, or two or more as a canonical table of rows.
fn render_root_group(roots: &[(Type, Item)], storage: &str, descriptions: &[Description]) -> Option<String> {
    if let [(slot, item)] = roots {
        return Some(format!(
            "{storage} << {}",
            render_value(item, slot, false, 0, descriptions)?
        ));
    }
    let mut rows = Vec::with_capacity(roots.len());
    for (slot, item) in roots {
        rows.push(format!("    {},", render_value(item, slot, true, 4, descriptions)?));
    }
    Some(format!("{storage} << [\n{}\n]", rows.join("\n")))
}

/// Render one value through its declared slot so optional and union layers stay explicit.
fn render_value(item: &Item, slot: &Type, row: bool, indent: usize, descriptions: &[Description]) -> Option<String> {
    Some(match item {
        Item::Absent => "None".to_string(),
        Item::Some(inner) => {
            let Type::Option(inner_slot) = slot else { return None };
            let rendered = render_value(inner, inner_slot, row, indent, descriptions)?;
            match **inner {
                Item::Absent | Item::Some(_) => format!("Some({rendered})"),
                _ => rendered,
            }
        }
        Item::Boolean(flag) => flag.to_string(),
        Item::Integer(spelling) => spelling.clone(),
        Item::Float(width, bits) => float_spelling(*width, bits)?,
        Item::Decimal(coefficient, scale) => decimal_spelling(coefficient, *scale),
        Item::Text(text) => quote_text(text.clone()),
        Item::Binary(data) => binary_literal(data.clone()),
        Item::Withheld => "<secret>".to_string(),
        Item::List(items) => {
            let Type::List(item_slot) = slot else { return None };
            render_list(items, item_slot, indent, descriptions)?
        }
        Item::Set(items) => {
            let Type::Set(item_slot) = slot else { return None };
            let rendered = items
                .iter()
                .map(|item| render_value(item, item_slot, false, indent, descriptions))
                .collect::<Option<Vec<_>>>()?;
            format!("{{{}}}", rendered.join(", "))
        }
        Item::Map(entries) => {
            let Type::Map(key_slot, value_slot) = slot else {
                return None;
            };
            let mut rendered = Vec::with_capacity(entries.len());
            for (key, value) in entries {
                let key = render_value(key, key_slot, false, indent, descriptions)?;
                rendered.push(format!(
                    "{key}: {}",
                    render_value(value, value_slot, false, indent, descriptions)?
                ));
            }
            format!("{{{}}}", rendered.join(", "))
        }
        Item::Record(fields) => render_record(fields, slot, row, indent, descriptions)?,
        Item::Enum(variant, fields) => render_enum(variant, fields, slot, slot, row, indent, descriptions)?,
        Item::Union(member, inner) => render_union(member, inner, slot, indent, descriptions)?,
        Item::Reference(target, identity) => {
            let Type::Reference(target_slot) = slot else {
                return None;
            };
            if !matches!(**target_slot, Type::Nominal(_)) {
                return None;
            }
            if !target.is_empty() {
                return None;
            }
            String::from_utf8(identity.clone()).ok()?
        }
    })
}

/// Require exactly one value-bearing declaration at a path.
fn description_at<'a>(descriptions: &'a [Description], path: &str) -> Option<&'a Description> {
    single(descriptions, path)
}

/// Render record cells by name for values and by position until the first gap for rows.
fn render_record(
    fields: &[(Vec<u8>, Item)],
    slot: &Type,
    row: bool,
    indent: usize,
    descriptions: &[Description],
) -> Option<String> {
    let description = description_at(descriptions, &type_spelling(slot))?;
    let mut cells = Vec::new();
    let mut gap = false;
    for field in &description.fields {
        let stored = identity_field(fields, &field.identity)?;
        if is_absent(stored, field) {
            gap = true;
            continue;
        }
        let rendered = render_value(stored, &field.storage, row, indent + 4, descriptions)?;
        if row && !gap {
            cells.push(rendered);
        } else {
            cells.push(format!("{}={rendered}", field.name));
        }
    }
    Some(cell_list(&cells, indent))
}

/// Render a variant name, qualified only when a union holds two variants of that name, plus its payload.
fn render_enum(
    identity: &[u8],
    fields: &[(Vec<u8>, Item)],
    slot: &Type,
    ambiguity_slot: &Type,
    row: bool,
    indent: usize,
    descriptions: &[Description],
) -> Option<String> {
    let storage = type_spelling(slot);
    let description = description_at(descriptions, &storage)?;
    let variant = description.variants.iter().find(|item| item.identity == identity)?;
    let mut name = variant.name.clone();
    if ambiguous_variant(ambiguity_slot, &variant.name, descriptions) {
        name = format!("{}.{}", last_segment(&storage), variant.name);
    }
    if variant.fields.is_empty() {
        return Some(name);
    }
    let mut cells = Vec::new();
    for field in &variant.fields {
        let stored = identity_field(fields, &field.identity)?;
        if !is_absent(stored, field) {
            let rendered = render_value(stored, &field.storage, row, indent + 4, descriptions)?;
            cells.push(if row {
                rendered
            } else {
                format!("{}={rendered}", field.name)
            });
        }
    }
    Some(name + &cell_list(&cells, indent))
}

/// Render enum members directly and every other union member behind its selector.
fn render_union(
    member: &Type,
    item: &Item,
    slot: &Type,
    indent: usize,
    descriptions: &[Description],
) -> Option<String> {
    let Type::Union(members) = slot else { return None };
    let spelling = type_spelling(member);
    let member_slot = members.iter().find(|candidate| type_spelling(candidate) == spelling)?;
    if let Item::Enum(variant, fields) = item {
        return render_enum(variant, fields, member_slot, slot, false, indent, descriptions);
    }
    let short = last_segment(&spelling);
    let count = members
        .iter()
        .filter(|candidate| last_segment(&type_spelling(candidate)) == short)
        .count();
    let selector = if count > 1 { spelling.clone() } else { short.to_string() };
    Some(format!(
        "{selector} << {}",
        render_value(item, member_slot, false, indent, descriptions)?
    ))
}

/// Break multi-row lists into one canonical row per line.
fn render_list(items: &[Item], slot: &Type, indent: usize, descriptions: &[Description]) -> Option<String> {
    let multiline = items.len() > 1 && items.iter().all(row_shaped);
    if !multiline {
        let rendered = items
            .iter()
            .map(|item| render_value(item, slot, false, indent, descriptions))
            .collect::<Option<Vec<_>>>()?;
        return Some(format!("[{}]", rendered.join(", ")));
    }
    let mut lines = Vec::with_capacity(items.len());
    for item in items {
        lines.push(format!(
            "{}{},",
            spaces(indent as i64 + 4),
            render_value(item, slot, true, indent + 4, descriptions)?
        ));
    }
    Some(format!("[\n{}\n{}]", lines.join("\n"), spaces(indent as i64)))
}

/// Whether a direct list element participates in record-row layout.
fn row_shaped(item: &Item) -> bool {
    match item {
        Item::Record(_) => true,
        Item::Enum(_, fields) => !fields.is_empty(),
        Item::Union(_, inner) => row_shaped(inner),
        _ => false,
    }
}

/// Keep a cell list inline unless a nested break or the 120-column limit requires one per line.
fn cell_list(cells: &[String], indent: usize) -> String {
    let inline = format!("({})", cells.join(", "));
    let multiline = spaces(indent as i64).len() + inline.len() > 120 || cells.iter().any(|cell| cell.contains('\n'));
    if !multiline {
        return inline;
    }
    let lines: Vec<String> = cells
        .iter()
        .map(|cell| format!("{}{cell},", spaces(indent as i64 + 4)))
        .collect();
    format!("(\n{}\n{})", lines.join("\n"), spaces(indent as i64))
}

/// Find the single stored field with a declared identity.
fn identity_field<'a>(fields: &'a [(Vec<u8>, Item)], identity: &[u8]) -> Option<&'a Item> {
    let mut found = fields.iter().filter(|(stored, _)| stored == identity);
    let first = found.next()?;
    found.next().is_none().then_some(&first.1)
}

/// Compare a field only with its declared outer absent value.
fn is_absent(item: &Item, field: &FieldShape) -> bool {
    match (field.absent, item) {
        ('n', Item::Absent) => true,
        ('e', Item::List(items) | Item::Set(items)) => items.is_empty(),
        ('e', Item::Map(entries)) => entries.is_empty(),
        _ => false,
    }
}

/// Whether two enum members of a union declare a variant with this name.
fn ambiguous_variant(slot: &Type, name: &str, descriptions: &[Description]) -> bool {
    let Type::Union(members) = slot else {
        return false;
    };
    let mut count = 0;
    for member in members {
        if let Type::Nominal(path) = member {
            for description in descriptions.iter().filter(|item| item.path == *path && !item.record) {
                count += description
                    .variants
                    .iter()
                    .filter(|variant| variant.name == name)
                    .count();
            }
        }
    }
    count > 1
}

/// The final dotted path segment.
pub(super) fn last_segment(path: &str) -> &str {
    path.rsplit('.').next().unwrap_or(path)
}

/// Insert the declared decimal point and retain every required fractional zero.
fn decimal_spelling(coefficient: &str, scale: i64) -> String {
    let (sign, mut digits) = match coefficient.strip_prefix('-') {
        Some(rest) => ("-", rest.to_string()),
        None => ("", coefficient.to_string()),
    };
    let scale = usize::try_from(scale).unwrap_or(0);
    while digits.len() <= scale {
        digits.insert(0, '0');
    }
    if scale == 0 {
        return format!("{sign}{digits}d");
    }
    let split = digits.len() - scale;
    format!("{sign}{}.{}d", &digits[..split], &digits[split..])
}

/// Derive shortest round-tripping digits from exact IEEE bits.
fn float_spelling(width: i64, bits: &[u8]) -> Option<String> {
    let spelling = match (width, bits.len()) {
        (32, 4) => f32::from_bits(u32::from_be_bytes(bits.try_into().ok()?)).to_string(),
        (64, 8) => f64::from_bits(u64::from_be_bytes(bits.try_into().ok()?)).to_string(),
        _ => return None,
    };
    Some(normalize_float(&spelling))
}

/// Keep settled non-finite spellings and require a point for finite integer-looking values.
fn normalize_float(spelling: &str) -> String {
    if spelling == "nan" || spelling == "NaN" {
        return "nan".to_string();
    }
    if let Some((mantissa, exponent)) = spelling.split_once('e') {
        let (sign, digits) = match exponent.strip_prefix('-') {
            Some(rest) => ("-", rest),
            None => ("", exponent.strip_prefix('+').unwrap_or(exponent)),
        };
        let trimmed = digits.trim_start_matches('0');
        let digits = if trimmed.is_empty() {
            &digits[digits.len().saturating_sub(1)..]
        } else {
            trimmed
        };
        return format!("{mantissa}e{sign}{digits}");
    }
    if spelling == "inf" || spelling == "-inf" || spelling.contains('.') {
        return spelling.to_string();
    }
    format!("{spelling}.0")
}
