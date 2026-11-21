// Selector and shape spellings: a node spelled with bound definitions kept unexpanded, nominal records and variants
// named, and recursion refused. Included by `semantic_hash.rs`, which `incan_codegraph` also compiles by path.

/// Return the selector spelling N; bound definitions never expand into it.
pub(super) fn selector_spelling(node: &Node) -> Option<String> {
    spell(node, true, &mut Vec::new())
}

/// Return the structural spelling S; a bare nominal selector supplies no shape.
pub(super) fn shape_spelling(node: &Node) -> Option<String> {
    spell(node, false, &mut Vec::new())
}

/// Spell one node as N (nominal) or S (structural) under the active definition stack.
fn spell(node: &Node, nominal: bool, active: &mut Vec<String>) -> Option<String> {
    match node {
        Node::Scalar(kind) => Some(kind.clone()),
        Node::Decimal(precision, scale) => (*precision > 0 && *precision <= 38 && *scale >= 0 && scale <= precision)
            .then(|| format!("decimal<{precision},{scale}>")),
        Node::Container(kind, items) => container(kind, items, nominal, active),
        Node::Nominal(kind, identity, arguments) => {
            if !nominal || identity.len() != 6 {
                return None;
            }
            let stem = format!("{kind}@{}", hex_string(identity));
            if arguments.is_empty() {
                return Some(stem);
            }
            let parts: Option<Vec<String>> = arguments
                .iter()
                .map(|argument| spell(argument, true, &mut Vec::new()))
                .collect();
            Some(format!("{stem}<{}>", parts?.join(",")))
        }
        Node::Record(key, fields) => {
            enter(key, active)?;
            let result = record_fields(fields, nominal, active);
            active.pop();
            result
        }
        Node::Enum(key, variants) => {
            enter(key, active)?;
            let result = variant_fields(variants, nominal, active);
            active.pop();
            result
        }
        Node::Reference(item) => {
            if nominal {
                Some(format!("ref<{}>", spell(item, true, active)?))
            } else {
                Some("reference".to_string())
            }
        }
        Node::Recursive(key) => {
            let offset = active.iter().rev().position(|item| item == key)?;
            Some(format!("^{offset}"))
        }
    }
}

/// Push one active definition, refusing a duplicate on the current path.
fn enter(key: &str, active: &mut Vec<String>) -> Option<()> {
    if active.iter().any(|item| item == key) {
        return None;
    }
    active.push(key.to_string());
    Some(())
}

/// Validate constructor arity and sort unions by hashed spelling with full tie-breaks.
fn container(kind: &str, items: &[Node], nominal: bool, active: &mut Vec<String>) -> Option<String> {
    let arity = match kind {
        "list" | "set" | "option" => items.len() == 1,
        "map" => items.len() == 2,
        "union" => !items.is_empty(),
        "tuple" => true,
        _ => false,
    };
    if !arity {
        return None;
    }
    let mut parts = items
        .iter()
        .map(|item| spell(item, nominal, active))
        .collect::<Option<Vec<String>>>()?;
    if kind == "union" {
        let mut keyed = Vec::with_capacity(parts.len());
        for part in parts {
            if nominal && keyed.iter().any(|(_, seen)| seen == &part) {
                return None;
            }
            keyed.push((hex_string(&Sha256::digest(part.as_bytes())), part));
        }
        keyed.sort();
        parts = keyed.into_iter().map(|(_, part)| part).collect();
    }
    Some(format!("{kind}<{}>", parts.join(",")))
}

/// Spell identity-sorted fields with their canonical flag and absence code.
fn record_fields(fields: &[Shape], nominal: bool, active: &mut Vec<String>) -> Option<String> {
    let mut ordered: Vec<(String, &Shape)> = Vec::with_capacity(fields.len());
    for field in fields {
        let identity = hex_string(&field.identity);
        if field.identity.len() != 6 || ordered.iter().any(|(seen, _)| seen == &identity) {
            return None;
        }
        ordered.push((identity, field));
    }
    ordered.sort_by(|left, right| left.0.cmp(&right.0));
    let mut parts = Vec::with_capacity(ordered.len());
    for (identity, field) in ordered {
        let spelling = spell(&field.storage, nominal, active)?;
        let canonical = if field.canonical { '1' } else { '0' };
        parts.push(format!("{identity}:{spelling}:{canonical}:{}", field.absent));
    }
    Some(format!("record{{{}}}", parts.join(";")))
}

/// Spell identity-sorted variants with their payload records.
fn variant_fields(variants: &[(Vec<u8>, Vec<Shape>)], nominal: bool, active: &mut Vec<String>) -> Option<String> {
    let mut ordered: Vec<(String, &Vec<Shape>)> = Vec::with_capacity(variants.len());
    for (identity, fields) in variants {
        let name = hex_string(identity);
        if identity.len() != 6 || ordered.iter().any(|(seen, _)| seen == &name) {
            return None;
        }
        ordered.push((name, fields));
    }
    ordered.sort_by(|left, right| left.0.cmp(&right.0));
    let mut parts = Vec::with_capacity(ordered.len());
    for (identity, fields) in ordered {
        parts.push(format!("{identity}:{}", record_fields(fields, nominal, active)?));
    }
    Some(format!("enum{{{}}}", parts.join(";")))
}
