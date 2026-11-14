//! Native COMIC reading: declarations and typed values over document descriptions.
//!
//! This mirrors `text_reader` and `text_values` over the tokens of `semantic_scan.rs`. An accepted document is
//! compared through its canonical print; a refusal through its typed category and one-based scalar line and column.

use serde_json::Value as Json;

use super::super::{decode_text, parse_decimal, parse_float, parse_integer};
use super::print::{
    self, Description, FieldShape, Item, Meta, Registration, Type, Variant, kind_shape, last_segment, member_identity,
    type_spelling,
};
use super::scan::{Failure, Kind, Read, Token, require_name, require_path, scan};

const STORE_KEYS: [&str; 5] = [
    "___id",
    "___revision",
    "___model_hash",
    "___canonical_hash",
    "___object_hash",
];

/// The fully scanned token stream and a cursor over it.
struct Tokens {
    items: Vec<Token>,
    index: usize,
}

/// Read one text-read case: the canonical print of the accepted document, or its refusal.
pub(super) fn text_read(document: &Json) -> Option<Vec<u8>> {
    let registrations = print::registrations(document.get("context"))?;
    let admitted: Vec<String> = match document
        .get("context")
        .and_then(|context| context.get("admitted_type_names"))
    {
        Some(names) => names
            .as_array()?
            .iter()
            .map(|name| name.as_str().map(str::to_string))
            .collect::<Option<_>>()?,
        None => Vec::new(),
    };
    let source = hex(document.get("input_hex")?.as_str()?)?;
    let mut output = Vec::new();
    match read(&source, &registrations, &admitted) {
        Ok((descriptions, roots)) => {
            let printed = print::print(&descriptions, &roots, &registrations)?;
            output.push(0);
            super::hash::frame(&mut output, printed.as_bytes());
        }
        Err((kind, line, column)) => {
            output.push(1);
            super::hash::frame(&mut output, kind.as_bytes());
            super::hash::unsigned(&mut output, line as u128);
            super::hash::unsigned(&mut output, column as u128);
        }
    }
    Some(output)
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

type Document = (Vec<Description>, Vec<(Type, Item)>);

/// Read one UTF-8 document: declarations and root entries in source order, then final resolution.
fn read(source: &[u8], registrations: &[Registration], admitted: &[String]) -> Read<Document> {
    let decoded = decode_text(&source.to_vec());
    match decoded.kind {
        0 => {}
        1 => return Err(("invalid_utf8", 1, 1)),
        _ => return Err(("bidirectional_control", decoded.line, decoded.column)),
    }
    let characters: Vec<char> = decoded
        .scalars
        .iter()
        .filter_map(|scalar| scalar.text.chars().next())
        .collect();
    let mut tokens = Tokens {
        items: scan(&characters)?,
        index: 0,
    };
    let mut descriptions: Vec<Description> = Vec::new();
    let mut roots = Vec::new();
    while tokens.peek().kind != Kind::End {
        let lookahead = tokens.peek_at(1).text.clone();
        if lookahead == "=" {
            let position = at(tokens.peek());
            let description = parse_description(&mut tokens, registrations)?;
            if descriptions
                .iter()
                .any(|item| item.path == description.path && item.kind == description.kind)
            {
                return Err(("duplicate_declaration_kind", position.1, position.2));
            }
            descriptions.push(description);
        } else if lookahead == "<<" {
            let resolved = resolve_descriptions(&descriptions, admitted)?;
            roots.extend(parse_root_values(&mut tokens, &resolved)?);
        } else {
            return Err(fail("invalid_entry", tokens.peek()));
        }
    }
    Ok((resolve_descriptions(&descriptions, admitted)?, roots))
}

/// The refusal position of one token.
fn at(token: &Token) -> Failure {
    ("", token.line, token.column)
}

/// One refusal at a token's starting position.
fn fail(kind: &'static str, token: &Token) -> Failure {
    (kind, token.line, token.column)
}

impl Tokens {
    fn peek(&self) -> &Token {
        &self.items[self.index]
    }

    fn peek_at(&self, distance: isize) -> &Token {
        let selected = (self.index as isize + distance).max(0) as usize;
        &self.items[selected.min(self.items.len() - 1)]
    }

    fn take(&mut self) -> Token {
        let token = self.items[self.index].clone();
        if token.kind != Kind::End {
            self.index += 1;
        }
        token
    }

    fn accepts(&mut self, spelling: &str) -> bool {
        if self.peek().text != spelling {
            return false;
        }
        self.take();
        true
    }

    fn require(&mut self, spelling: &str) -> Read<Token> {
        let token = self.peek();
        if token.text != spelling {
            if token.kind == Kind::End {
                return Err(fail("unexpected_end", token));
            }
            return Err(fail("invalid_token", token));
        }
        Ok(self.take())
    }
}

/// Consume one nonempty word token.
fn word(tokens: &mut Tokens, kind: &'static str) -> Read<Token> {
    let token = tokens.peek();
    if token.kind != Kind::Word || token.text.is_empty() {
        return Err(fail(kind, token));
    }
    Ok(tokens.take())
}

/// Consume and admit one name word.
fn name_word(tokens: &mut Tokens, kind: &'static str) -> Read<String> {
    let token = word(tokens, kind)?;
    require_name(&token.text, token.line, token.column, kind)
}

/// Consume and admit one path word.
fn path_word(tokens: &mut Tokens, kind: &'static str) -> Read<String> {
    let token = word(tokens, kind)?;
    require_path(&token.text, token.line, token.column, kind)
}

/// Read one core or registered declaration.
fn parse_description(tokens: &mut Tokens, registrations: &[Registration]) -> Read<Description> {
    let path = path_word(tokens, "invalid_entry")?;
    tokens.require("=")?;
    let kind_token = tokens.peek().clone();
    let kind = path_word(tokens, "unknown_kind")?;
    let record = kind_shape(&kind, registrations).ok_or(fail("unknown_kind", &kind_token))?;
    tokens.require("{")?;
    let (mut properties, mut fields, mut variants) = (Vec::new(), Vec::new(), Vec::new());
    while !tokens.accepts("}") {
        if tokens.peek().kind == Kind::End {
            return Err(fail("unexpected_end", tokens.peek()));
        }
        if tokens.peek_at(1).text == "=" {
            let position = tokens.peek().clone();
            let property = parse_property(tokens)?;
            refuse_duplicate(&properties, &property, &position)?;
            properties.push(property);
        } else if record {
            fields.push(parse_field(tokens)?);
        } else {
            variants.push(parse_variant(tokens)?);
        }
        tokens.accepts(",");
    }
    Ok(Description {
        path,
        kind,
        record,
        properties,
        fields,
        variants,
    })
}

/// Refuse a duplicate property key.
fn refuse_duplicate(properties: &[(String, Meta)], candidate: &(String, Meta), position: &Token) -> Read<()> {
    if properties.iter().any(|(key, _)| *key == candidate.0) {
        return Err(fail("invalid_metadata", position));
    }
    Ok(())
}

/// Read an optional bracketed property list.
fn bracketed_properties(tokens: &mut Tokens) -> Read<Vec<(String, Meta)>> {
    let mut properties = Vec::new();
    if tokens.accepts("[") {
        while !tokens.accepts("]") {
            let position = tokens.peek().clone();
            let property = parse_property(tokens)?;
            refuse_duplicate(&properties, &property, &position)?;
            properties.push(property);
            tokens.accepts(",");
        }
    }
    Ok(properties)
}

/// Read one field, projecting canonical participation, absence and rename history.
fn parse_field(tokens: &mut Tokens) -> Read<FieldShape> {
    let name_token = tokens.peek().clone();
    let name = name_word(tokens, "invalid_fields")?;
    if name.starts_with("___") {
        return Err(fail("reserved_field", &name_token));
    }
    let properties = bracketed_properties(tokens)?;
    tokens.require(":")?;
    let storage = parse_type(tokens)?;
    let mut absent = 'r';
    if tokens.accepts("=") {
        if tokens.accepts("None") {
            absent = 'n';
        } else if tokens.accepts("[") {
            tokens.require("]")?;
            absent = 'e';
        } else if tokens.accepts("{") {
            tokens.require("}")?;
            absent = 'e';
        } else {
            return Err(fail("invalid_absent_value", tokens.peek()));
        }
    }
    let mut canonical = true;
    for (key, value) in &properties {
        if key == "canonical" {
            match value {
                Meta::Boolean(flag) => canonical = *flag,
                _ => return Err(fail("invalid_metadata", &name_token)),
            }
        }
    }
    let renamed = rename_history(&properties, &name_token)?;
    let identity = member_identity(renamed.first().unwrap_or(&name));
    Ok(FieldShape {
        identity,
        storage,
        canonical,
        absent,
        name,
        renamed,
    })
}

/// Read one enum variant, its rename history and optional payload fields.
fn parse_variant(tokens: &mut Tokens) -> Read<Variant> {
    let name_token = tokens.peek().clone();
    let name = name_word(tokens, "unknown_variant")?;
    let properties = bracketed_properties(tokens)?;
    let mut fields = Vec::new();
    if tokens.accepts("(") {
        while !tokens.accepts(")") {
            fields.push(parse_field(tokens)?);
            if tokens.accepts(")") {
                break;
            }
            tokens.require(",")?;
        }
    }
    let renamed = rename_history(&properties, &name_token)?;
    let identity = member_identity(renamed.first().unwrap_or(&name));
    Ok(Variant {
        identity,
        fields,
        name,
        renamed,
    })
}

/// Read one inert property.
fn parse_property(tokens: &mut Tokens) -> Read<(String, Meta)> {
    let position = tokens.peek().clone();
    let name = name_word(tokens, "invalid_property")?;
    if name.contains('.') {
        return Err(fail("invalid_property", &position));
    }
    tokens.require("=")?;
    Ok((name, parse_property_value(tokens)?))
}

/// Read symbols, scalars, lists and nested property groups without evaluation.
fn parse_property_value(tokens: &mut Tokens) -> Read<Meta> {
    let token = tokens.peek().clone();
    Ok(match token.kind {
        Kind::Text => Meta::Text(tokens.take().text),
        Kind::Binary => Meta::Binary(tokens.take().data),
        _ if token.text == "true" || token.text == "false" => Meta::Boolean(tokens.take().text == "true"),
        _ if token.text == "None" => {
            tokens.take();
            Meta::Null
        }
        Kind::Number => {
            let number = tokens.take();
            Meta::Integer(number.text.parse().map_err(|_| fail("invalid_property", &number))?)
        }
        _ if tokens.accepts("[") => {
            let mut items = Vec::new();
            while !tokens.accepts("]") {
                items.push(parse_property_value(tokens)?);
                if tokens.accepts("]") {
                    break;
                }
                tokens.require(",")?;
            }
            Meta::List(items)
        }
        _ if tokens.accepts("{") => {
            let mut properties = Vec::new();
            while !tokens.accepts("}") {
                let position = tokens.peek().clone();
                let property = parse_property(tokens)?;
                refuse_duplicate(&properties, &property, &position)?;
                properties.push(property);
                tokens.accepts(",");
            }
            Meta::Map(properties)
        }
        Kind::Word => Meta::Symbol(name_word(tokens, "invalid_property")?),
        _ => return Err(fail("invalid_property", &token)),
    })
}

/// Read an optional earliest-first rename list.
fn rename_history(properties: &[(String, Meta)], position: &Token) -> Read<Vec<String>> {
    for (key, value) in properties {
        if key == "renamed_from" {
            let Meta::List(items) = value else {
                return Err(fail("invalid_metadata", position));
            };
            let mut names = Vec::with_capacity(items.len());
            for item in items {
                match item {
                    Meta::Symbol(name) | Meta::Text(name) => names.push(name.clone()),
                    _ => return Err(fail("invalid_metadata", position)),
                }
            }
            return Ok(names);
        }
    }
    Ok(Vec::new())
}

/// Parse one complete type: `|`-separated primaries.
fn parse_type(tokens: &mut Tokens) -> Read<Type> {
    let mut members = vec![type_primary(tokens)?];
    while tokens.accepts("|") {
        members.push(type_primary(tokens)?);
    }
    if members.len() == 1 {
        return Ok(members.remove(0));
    }
    Ok(Type::Union(members))
}

/// Parse one scalar, named type or parameterized core constructor, then an optional `?`.
fn type_primary(tokens: &mut Tokens) -> Read<Type> {
    let token = tokens.peek().clone();
    if token.kind != Kind::Word && token.kind != Kind::Number {
        return Err(fail("invalid_type", &token));
    }
    tokens.take();
    let name = require_path(&token.text, token.line, token.column, "invalid_type")?;
    let mut result = match name.as_str() {
        "decimal" => {
            tokens.require("<")?;
            let precision_token = tokens.take();
            tokens.require(",")?;
            let scale_token = tokens.take();
            tokens.require(">")?;
            let (precision, scale) = (plain_unsigned(&precision_token.text), plain_unsigned(&scale_token.text));
            match (precision, scale) {
                (Some(precision), Some(scale)) if precision > 0 && precision <= 38 && scale <= precision => {
                    Type::Decimal(precision, scale)
                }
                _ => return Err(fail("invalid_type", &precision_token)),
            }
        }
        "ref" => Type::Reference(Box::new(type_argument(tokens)?)),
        "list" => Type::List(Box::new(type_argument(tokens)?)),
        "set" => Type::Set(Box::new(type_argument(tokens)?)),
        "option" => Type::Option(Box::new(type_argument(tokens)?)),
        "map" => {
            tokens.require("<")?;
            let key = parse_type(tokens)?;
            tokens.require(",")?;
            let item = parse_type(tokens)?;
            tokens.require(">")?;
            Type::Map(Box::new(key), Box::new(item))
        }
        scalar if is_scalar(scalar) => Type::Scalar(name),
        _ => Type::Nominal(name),
    };
    if tokens.accepts("?") {
        result = Type::Option(Box::new(result));
    }
    Ok(result)
}

/// Parse one `<type>` constructor argument.
fn type_argument(tokens: &mut Tokens) -> Read<Type> {
    tokens.require("<")?;
    let item = parse_type(tokens)?;
    tokens.require(">")?;
    Ok(item)
}

/// Canonical nonempty decimal digits without leading zeros.
fn plain_unsigned(value: &str) -> Option<i64> {
    if value.is_empty()
        || (value.len() > 1 && value.starts_with('0'))
        || !value.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    value.parse().ok()
}

/// The scalar spellings that parse as scalar types.
fn is_scalar(name: &str) -> bool {
    matches!(
        name,
        "boolean"
            | "i8"
            | "i16"
            | "i32"
            | "i64"
            | "i128"
            | "u8"
            | "u16"
            | "u32"
            | "u64"
            | "u128"
            | "fp32"
            | "fp64"
            | "string"
            | "binary"
            | "secret_string"
            | "secret_bytes"
    )
}

/// Resolve every field type of every declaration against the complete visible set.
fn resolve_descriptions(descriptions: &[Description], admitted: &[String]) -> Read<Vec<Description>> {
    let mut resolved = Vec::with_capacity(descriptions.len());
    for description in descriptions {
        let fields = resolve_fields(&description.fields, descriptions, admitted)?;
        let mut variants = Vec::with_capacity(description.variants.len());
        for variant in &description.variants {
            variants.push(Variant {
                identity: variant.identity.clone(),
                fields: resolve_fields(&variant.fields, descriptions, admitted)?,
                name: variant.name.clone(),
                renamed: variant.renamed.clone(),
            });
        }
        resolved.push(Description {
            path: description.path.clone(),
            kind: description.kind.clone(),
            record: description.record,
            properties: description
                .properties
                .iter()
                .map(|(key, value)| (key.clone(), value.copy()))
                .collect(),
            fields,
            variants,
        });
    }
    Ok(resolved)
}

/// Resolve the storage types of one field table.
fn resolve_fields(fields: &[FieldShape], descriptions: &[Description], admitted: &[String]) -> Read<Vec<FieldShape>> {
    let mut resolved = Vec::with_capacity(fields.len());
    for field in fields {
        resolved.push(FieldShape {
            identity: field.identity.clone(),
            storage: resolve_node(&field.storage, descriptions, admitted)?,
            canonical: field.canonical,
            absent: field.absent,
            name: field.name.clone(),
            renamed: field.renamed.clone(),
        });
    }
    Ok(resolved)
}

/// Resolve a parsed path through one declaration or one admitted logical type name.
fn resolve_node(node: &Type, descriptions: &[Description], admitted: &[String]) -> Read<Type> {
    let ambiguous = ("ambiguous_or_unknown_type", 1, 1);
    Ok(match node {
        Type::Nominal(path) => {
            let exact = descriptions.iter().filter(|item| item.path == *path).count();
            let mut short: Vec<&String> = descriptions
                .iter()
                .filter(|item| item.path != *path && last_segment(&item.path) == path)
                .map(|item| &item.path)
                .collect();
            match exact {
                1 => return Ok(Type::Nominal(path.clone())),
                0 => {}
                _ => return Err(ambiguous),
            }
            let admitted_exact = admitted.iter().filter(|name| *name == path).count();
            short.extend(
                admitted
                    .iter()
                    .filter(|name| *name != path && last_segment(name) == path),
            );
            match (admitted_exact, short.len()) {
                (1, _) => Type::Nominal(path.clone()),
                (0, 1) => Type::Nominal(short[0].clone()),
                _ => return Err(ambiguous),
            }
        }
        Type::List(item) => Type::List(Box::new(resolve_node(item, descriptions, admitted)?)),
        Type::Set(item) => Type::Set(Box::new(resolve_node(item, descriptions, admitted)?)),
        Type::Option(item) => Type::Option(Box::new(resolve_node(item, descriptions, admitted)?)),
        Type::Reference(item) => Type::Reference(Box::new(resolve_node(item, descriptions, admitted)?)),
        Type::Map(key, item) => Type::Map(
            Box::new(resolve_node(key, descriptions, admitted)?),
            Box::new(resolve_node(item, descriptions, admitted)?),
        ),
        Type::Union(items) => Type::Union(
            items
                .iter()
                .map(|item| resolve_node(item, descriptions, admitted))
                .collect::<Read<_>>()?,
        ),
        _ => node.clone(),
    })
}

impl Meta {
    /// Copy one property value.
    fn copy(&self) -> Meta {
        match self {
            Meta::Null => Meta::Null,
            Meta::Boolean(flag) => Meta::Boolean(*flag),
            Meta::Integer(number) => Meta::Integer(*number),
            Meta::Text(text) => Meta::Text(text.clone()),
            Meta::Binary(data) => Meta::Binary(data.clone()),
            Meta::Symbol(name) => Meta::Symbol(name.clone()),
            Meta::List(items) => Meta::List(items.iter().map(Meta::copy).collect()),
            Meta::Map(entries) => Meta::Map(entries.iter().map(|(key, value)| (key.clone(), value.copy())).collect()),
        }
    }
}

/// Read one root value or table after requiring its unique declaration.
fn parse_root_values(tokens: &mut Tokens, descriptions: &[Description]) -> Read<Vec<(Type, Item)>> {
    let type_token = tokens.take();
    let path = require_path(
        &type_token.text,
        type_token.line,
        type_token.column,
        "unknown_description",
    )?;
    let description = description_at(descriptions, &path, &type_token)?;
    tokens.require("<<")?;
    let root = Type::Nominal(description.path.clone());
    let mut values = Vec::new();
    if tokens.accepts("[") {
        while !tokens.accepts("]") {
            if tokens.peek().kind == Kind::End {
                return Err(fail("unexpected_end", tokens.peek()));
            }
            values.push((root.clone(), parse_described(description, descriptions, tokens)?));
            if tokens.accepts("]") {
                break;
            }
            tokens.require(",")?;
        }
    } else {
        values.push((root, parse_described(description, descriptions, tokens)?));
    }
    Ok(values)
}

/// Require exactly one value-bearing declaration at a path.
fn description_at<'a>(descriptions: &'a [Description], path: &str, token: &Token) -> Read<&'a Description> {
    let found: Vec<&Description> = descriptions.iter().filter(|item| item.path == path).collect();
    match found.len() {
        0 => Err(fail("unknown_description", token)),
        1 => Ok(found[0]),
        _ => Err(fail("value_type_no_single_declaration", token)),
    }
}

/// Resolve an exact path or one unambiguous final path segment.
fn resolve_named<'a>(descriptions: &'a [Description], spelling: &str, token: &Token) -> Read<&'a Description> {
    let exact: Vec<&Description> = descriptions.iter().filter(|item| item.path == spelling).collect();
    let short: Vec<&Description> = descriptions
        .iter()
        .filter(|item| item.path != spelling && last_segment(&item.path) == spelling)
        .collect();
    match (exact.len(), short.len()) {
        (1, _) => Ok(exact[0]),
        (0, 1) => Ok(short[0]),
        (0, _) => Err(fail("ambiguous_or_unknown_type", token)),
        _ => Err(fail("value_type_no_single_declaration", token)),
    }
}

/// Read one record or enum through its declaration.
fn parse_described(description: &Description, descriptions: &[Description], tokens: &mut Tokens) -> Read<Item> {
    if description.record {
        if tokens.peek().text != "(" {
            return Err(fail("type_mismatch", tokens.peek()));
        }
        return Ok(Item::Record(parse_cells(&description.fields, descriptions, tokens)?));
    }
    parse_enum(description, descriptions, tokens)
}

/// Apply positions first, then names or indexes, and materialize every declared absence.
fn parse_cells(fields: &[FieldShape], descriptions: &[Description], tokens: &mut Tokens) -> Read<Vec<(Vec<u8>, Item)>> {
    tokens.require("(")?;
    let mut values: Vec<Option<Item>> = fields.iter().map(|_| None).collect();
    let (mut positional, mut saw_key) = (0usize, false);
    while !tokens.accepts(")") {
        if tokens.peek().kind == Kind::End {
            return Err(fail("unexpected_end", tokens.peek()));
        }
        let key_token = tokens.peek().clone();
        let mut selected = positional;
        if tokens.peek_at(1).text == "=" {
            saw_key = true;
            let key = tokens.take().text;
            tokens.take();
            selected = if !canonical_unsigned(&key) && !key.starts_with("___") {
                let name = require_name(&key, key_token.line, key_token.column, "unknown_field")?;
                field_index(fields, &name, &key_token)?
            } else {
                field_index(fields, &key, &key_token)?
            };
        } else if saw_key {
            return Err(fail("positional_after_key", &key_token));
        } else {
            positional += 1;
            if selected >= fields.len() {
                let kind = if key_token.text.starts_with("___") {
                    "unknown_store_key"
                } else {
                    "field_index_out_of_range"
                };
                return Err(fail(kind, &key_token));
            }
        }
        if values[selected].is_some() {
            return Err(fail("duplicate_field", &key_token));
        }
        values[selected] = Some(parse_value(&fields[selected].storage, descriptions, tokens)?);
        if tokens.accepts(")") {
            break;
        }
        tokens.require(",")?;
        if tokens.accepts(")") {
            break;
        }
    }
    let closing = tokens.peek_at(-1).clone();
    let mut result = Vec::with_capacity(fields.len());
    for (field, value) in fields.iter().zip(values) {
        let value = match value {
            Some(value) => value,
            None => absent_value(field).ok_or(fail("missing_required_field", &closing))?,
        };
        result.push((field.identity.clone(), value));
    }
    Ok(result)
}

/// Resolve a store key, an input-only declaration index or an exact field name.
fn field_index(fields: &[FieldShape], key: &str, token: &Token) -> Read<usize> {
    if key.starts_with("___") {
        let kind = if STORE_KEYS.contains(&key) {
            "unsupported_store_key"
        } else {
            "unknown_store_key"
        };
        return Err(fail(kind, token));
    }
    if canonical_unsigned(key) {
        let index: usize = key.parse().map_err(|_| fail("field_index_out_of_range", token))?;
        if index >= fields.len() {
            return Err(fail("field_index_out_of_range", token));
        }
        return Ok(index);
    }
    fields
        .iter()
        .position(|field| field.name == key)
        .ok_or(fail("unknown_field", token))
}

/// Nonempty decimal digits without leading zeros.
fn canonical_unsigned(value: &str) -> bool {
    plain_unsigned(value).is_some()
}

/// A declared absent value, never a constructor default.
fn absent_value(field: &FieldShape) -> Option<Item> {
    match (field.absent, &field.storage) {
        ('n', _) => Some(Item::Absent),
        ('e', Type::List(_)) => Some(Item::List(Vec::new())),
        ('e', Type::Set(_)) => Some(Item::Set(Vec::new())),
        ('e', Type::Map(..)) => Some(Item::Map(Vec::new())),
        _ => None,
    }
}

/// Read one value through its declared slot.
fn parse_value(storage: &Type, descriptions: &[Description], tokens: &mut Tokens) -> Read<Item> {
    match storage {
        Type::Scalar(kind) => parse_scalar(kind, tokens),
        Type::Decimal(precision, scale) => {
            let token = tokens.take();
            let parsed = parse_decimal(token.text.clone(), *precision as u8, *scale as u8);
            match parsed.kind {
                0 => Ok(Item::Decimal(parsed.coefficient.to_string(), *scale)),
                1 => Err(fail("type_mismatch", &token)),
                2 => Err(fail("decimal_scale_mismatch", &token)),
                _ => Err(fail("decimal_precision", &token)),
            }
        }
        Type::Option(inner) => {
            if tokens.accepts("None") {
                return Ok(Item::Absent);
            }
            if tokens.accepts("Some") {
                tokens.require("(")?;
                let value = parse_value(inner, descriptions, tokens)?;
                tokens.require(")")?;
                return Ok(Item::Some(Box::new(value)));
            }
            Ok(Item::Some(Box::new(parse_value(inner, descriptions, tokens)?)))
        }
        Type::List(item) => {
            tokens.require("[")?;
            let mut items = Vec::new();
            while !tokens.accepts("]") {
                items.push(parse_value(item, descriptions, tokens)?);
                if tokens.accepts("]") {
                    break;
                }
                tokens.require(",")?;
            }
            Ok(Item::List(items))
        }
        Type::Set(item) => parse_set(item, descriptions, tokens),
        Type::Map(key, item) => parse_map(key, item, descriptions, tokens),
        Type::Union(members) => parse_union(members, descriptions, tokens),
        Type::Reference(target) => {
            let token = tokens.take();
            if token.kind != Kind::Word {
                return Err(fail("type_mismatch", &token));
            }
            let Type::Nominal(path) = &**target else {
                return Err(fail("invalid_type", &token));
            };
            resolve_named(descriptions, path, &token)?;
            Ok(Item::Reference(Vec::new(), token.text.into_bytes()))
        }
        Type::Nominal(path) => {
            let description = resolve_named(descriptions, path, tokens.peek())?;
            parse_described(description, descriptions, tokens)
        }
    }
}

/// Read one core scalar with exact integer widths, float bits and secret withholding.
fn parse_scalar(kind: &str, tokens: &mut Tokens) -> Read<Item> {
    let token = tokens.peek().clone();
    match kind {
        "boolean" => {
            if token.text != "true" && token.text != "false" {
                return Err(fail("type_mismatch", &token));
            }
            tokens.take();
            Ok(Item::Boolean(token.text == "true"))
        }
        "fp32" | "fp64" => {
            tokens.take();
            let width = if kind == "fp32" { 32 } else { 64 };
            let parsed = parse_float(token.text.clone(), width);
            match parsed.kind {
                0 => Ok(Item::Float(i64::from(width), parsed.data)),
                1 => Err(fail("invalid_float", &token)),
                _ => Err(fail("type_mismatch", &token)),
            }
        }
        "string" => match token.kind {
            Kind::Text => Ok(Item::Text(tokens.take().text)),
            Kind::Marker => Err(fail("unsupported_marker", &token)),
            _ => Err(fail("type_mismatch", &token)),
        },
        "binary" => match token.kind {
            Kind::Binary => Ok(Item::Binary(tokens.take().data)),
            _ => Err(fail("type_mismatch", &token)),
        },
        "secret_string" | "secret_bytes" => {
            if token.text != "<secret>" {
                return Err(fail("unsupported_marker", &token));
            }
            tokens.take();
            Ok(Item::Withheld)
        }
        _ => parse_integer_token(kind, tokens, &token),
    }
}

/// Map one canonical integer parse to the declared width or its refusal.
fn parse_integer_token(kind: &str, tokens: &mut Tokens, token: &Token) -> Read<Item> {
    let signed = kind.starts_with('i');
    let Some(bits) = kind.get(1..).and_then(|bits| bits.parse::<u8>().ok()) else {
        return Err(fail("type_mismatch", token));
    };
    tokens.take();
    if matches!(token.text.as_str(), "inf" | "-inf" | "nan") {
        return Err(fail("type_mismatch", token));
    }
    let parsed = parse_integer(token.text.clone(), signed, bits);
    match parsed.kind {
        0 if signed => Ok(Item::Integer(parsed.signed.to_string())),
        0 => Ok(Item::Integer(parsed.unsigned.to_string())),
        1 => Err(fail("invalid_integer", token)),
        _ => Err(fail("integer_out_of_range", token)),
    }
}

/// Read a set, refusing duplicates and placing values in stored scalar order.
fn parse_set(item: &Type, descriptions: &[Description], tokens: &mut Tokens) -> Read<Item> {
    tokens.require("{")?;
    let mut items: Vec<Item> = Vec::new();
    while !tokens.accepts("}") {
        let value = parse_value(item, descriptions, tokens)?;
        let key = order_key(&value);
        if items.iter().any(|existing| order_key(existing) == key) {
            return Err(fail("duplicate_key", tokens.peek_at(-1)));
        }
        let index = items
            .iter()
            .position(|existing| compare(existing, &value) >= 0)
            .unwrap_or(items.len());
        items.insert(index, value);
        if tokens.accepts("}") {
            break;
        }
        tokens.require(",")?;
    }
    Ok(Item::Set(items))
}

/// Read a map, refusing duplicate keys and placing pairs in stored key order.
fn parse_map(key_type: &Type, value_type: &Type, descriptions: &[Description], tokens: &mut Tokens) -> Read<Item> {
    tokens.require("{")?;
    let mut entries: Vec<(Item, Item)> = Vec::new();
    while !tokens.accepts("}") {
        let key_position = tokens.peek().clone();
        let key = parse_value(key_type, descriptions, tokens)?;
        tokens.require(":")?;
        let value = parse_value(value_type, descriptions, tokens)?;
        let order = order_key(&key);
        if entries.iter().any(|(existing, _)| order_key(existing) == order) {
            return Err(fail("duplicate_key", &key_position));
        }
        let index = entries
            .iter()
            .position(|(existing, _)| compare(existing, &key) >= 0)
            .unwrap_or(entries.len());
        entries.insert(index, (key, value));
        if tokens.accepts("}") {
            break;
        }
        tokens.require(",")?;
    }
    Ok(Item::Map(entries))
}

/// A collision-free key for the supported primitive text cases.
fn order_key(value: &Item) -> String {
    match value {
        Item::Boolean(flag) => if *flag { "b1" } else { "b0" }.to_string(),
        Item::Integer(spelling) => format!("i{spelling}"),
        Item::Text(text) => format!("s{text}"),
        Item::Binary(data) => format!("x{}", data.iter().map(|byte| format!("{byte:02x}")).collect::<String>()),
        Item::Absent => "n".to_string(),
        _ => "unsupported".to_string(),
    }
}

/// Integers compare numerically; every other supported key by its order key.
fn compare(left: &Item, right: &Item) -> i8 {
    let ordering = match (left, right) {
        (Item::Integer(a), Item::Integer(b)) => match (a.parse::<i128>(), b.parse::<i128>()) {
            (Ok(a), Ok(b)) => a.cmp(&b),
            _ => numeric_text_order(a, b),
        },
        _ => order_key(left).cmp(&order_key(right)),
    };
    ordering as i8
}

/// Order nonnegative decimal spellings beyond `i128` by length, then digits.
fn numeric_text_order(left: &str, right: &str) -> std::cmp::Ordering {
    left.len().cmp(&right.len()).then_with(|| left.cmp(right))
}

/// Select a union member from an explicit selector or an unambiguous qualified enum variant.
fn parse_union(members: &[Type], descriptions: &[Description], tokens: &mut Tokens) -> Read<Item> {
    let token = tokens.peek().clone();
    let member = if tokens.peek_at(1).text == "<<" {
        let selector_token = tokens.take();
        let selector = require_path(
            &selector_token.text,
            selector_token.line,
            selector_token.column,
            "ambiguous_union",
        )?;
        tokens.take();
        let found: Vec<&Type> = members
            .iter()
            .filter(|member| {
                let spelling = type_spelling(member);
                spelling == selector || last_segment(&spelling) == selector
            })
            .collect();
        if found.len() != 1 {
            return Err(fail("ambiguous_union", &token));
        }
        found[0].clone()
    } else {
        qualified_variant_member(members, &token.text, descriptions).ok_or(fail("ambiguous_union", &token))?
    };
    let value = parse_value(&member, descriptions, tokens)?;
    let resolved = match &member {
        Type::Nominal(path) => Type::Nominal(resolve_named(descriptions, path, &token)?.path.clone()),
        other => other.clone(),
    };
    Ok(Item::Union(resolved, Box::new(value)))
}

/// The sole enum union member admitted by `EnumName.Variant`.
fn qualified_variant_member(members: &[Type], spelling: &str, descriptions: &[Description]) -> Option<Type> {
    let parts: Vec<&str> = spelling.split('.').collect();
    if parts.len() < 2 {
        return None;
    }
    let (enum_name, variant) = (parts[parts.len() - 2], parts[parts.len() - 1]);
    let found: Vec<&Type> = members
        .iter()
        .filter(|member| match member {
            Type::Nominal(path) => {
                last_segment(path) == enum_name
                    && descriptions.iter().any(|item| {
                        item.path == *path
                            && !item.record
                            && item.variants.iter().any(|candidate| candidate.name == variant)
                    })
            }
            _ => false,
        })
        .collect();
    (found.len() == 1).then(|| found[0].clone())
}

/// Read one enum variant name, accepting an owning-enum qualifier, and its payload cells.
fn parse_enum(description: &Description, descriptions: &[Description], tokens: &mut Tokens) -> Read<Item> {
    let token = tokens.take();
    let path = require_path(&token.text, token.line, token.column, "unknown_variant")?;
    let parts: Vec<&str> = path.split('.').collect();
    let mut name = path.as_str();
    if parts.len() > 1 {
        if parts[parts.len() - 2] != last_segment(&description.path) {
            return Err(fail("unknown_variant", &token));
        }
        name = parts[parts.len() - 1];
    }
    let variant = description
        .variants
        .iter()
        .find(|item| item.name == name)
        .ok_or(fail("unknown_variant", &token))?;
    let fields = if !variant.fields.is_empty() {
        parse_cells(&variant.fields, descriptions, tokens)?
    } else if tokens.peek().text == "(" {
        parse_cells(&[], descriptions, tokens)?
    } else {
        Vec::new()
    };
    Ok(Item::Enum(variant.identity.clone(), fields))
}
