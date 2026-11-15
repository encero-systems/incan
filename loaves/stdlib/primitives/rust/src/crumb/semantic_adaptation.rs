//! Native written-to-requested record adaptation over the corpus notation.
//!
//! This mirrors `std.primitives.crumb.admission.plan_adaptation`: both field tables are checked before matching,
//! fields match by exact identity in requested order, and only a declared absence fills a missing field. Accepted
//! plans are applied to the tagged values and compared through the object, canonical and model digests.

use serde_json::{Map, Value as Json};
use sha2::{Digest, Sha256};

use super::hash::{self, Field, Node, Projection, Shape, Value};

/// One structural conversion step, retaining only what value reconstruction needs.
enum Plan {
    Identity,
    Widen(String),
    Wrap(Box<Plan>),
    Optional(Box<Plan>),
    Container(String, Vec<Plan>),
    Nested,
}

/// One requested field's written source or admitted absence.
struct FieldPlan<'a> {
    requested: &'a Shape,
    source: Option<usize>,
    conversion: Plan,
}

/// A structural refusal: a stable category and the field path it names.
struct Refusal {
    kind: &'static str,
    path: String,
}

type Planned<T> = Result<T, Refusal>;

/// Plan, apply and compare one adaptation case; a refusal encodes its category and path.
pub(super) fn adaptation(document: &Json) -> Option<Vec<u8>> {
    let written = document.get("written")?;
    let requested = document.get("requested")?;
    let source = hash::shapes(written)?;
    let target = hash::shapes(requested)?;
    let nominal = (written.get("nominal")?.as_str()?, requested.get("nominal")?.as_str()?);
    let plans = match plan_adaptation(nominal, &source, &target) {
        Ok(plans) => plans,
        Err(refusal) => {
            let mut encoded = vec![1];
            hash::frame(&mut encoded, refusal.kind.as_bytes());
            hash::frame(&mut encoded, refusal.path.as_bytes());
            return Some(encoded);
        }
    };
    let values = document.get("values")?.as_array()?;
    let mut output = Vec::with_capacity(plans.len());
    for plan in &plans {
        output.push(match plan.source {
            Some(index) => convert(values.get(index)?, &plan.conversion)?,
            None => absent_value(plan.requested)?,
        });
    }
    let before = record(&source, values)?;
    let after = record(&target, &output)?;
    let after_object = hash::value_preimage(&after, Projection::Object)?;
    let mut encoded = vec![0];
    hash::frame(&mut encoded, &after_object);
    encoded.push(changed(
        &hash::value_preimage(&before, Projection::Object)?,
        &after_object,
    ));
    encoded.push(changed(
        &hash::value_preimage(&before, Projection::Canonical)?,
        &hash::value_preimage(&after, Projection::Canonical)?,
    ));
    encoded.push(changed(
        &hash::model_preimage(&Node::Record("source".to_string(), source))?,
        &hash::model_preimage(&Node::Record("target".to_string(), target))?,
    ));
    Some(encoded)
}

/// One when the two preimages' digests differ, zero when they are the same.
fn changed(left: &[u8], right: &[u8]) -> u8 {
    u8::from(Sha256::digest(left) != Sha256::digest(right))
}

/// Attach declared field facts to tagged values in declaration order.
fn record(shapes: &[Shape], items: &[Json]) -> Option<Value> {
    if shapes.len() != items.len() {
        return None;
    }
    let mut fields = Vec::with_capacity(items.len());
    for (shape, item) in shapes.iter().zip(items) {
        fields.push(Field {
            identity: shape.identity.clone(),
            canonical: shape.canonical,
            absent: shape.absent,
            value: hash::logical(item)?,
        });
    }
    Some(Value::Record(fields))
}

/// Check the nominal binding and both tables, then match requested fields by identity.
fn plan_adaptation<'a>(
    nominal: (&str, &str),
    written: &[Shape],
    requested: &'a [Shape],
) -> Planned<Vec<FieldPlan<'a>>> {
    if nominal.0.is_empty() || nominal.1.is_empty() {
        return Err(refusal("InvalidType", "/nominal"));
    }
    if nominal.0 != nominal.1 {
        return Err(refusal("NominalMismatch", "/nominal"));
    }
    check_fields(written, true, "")?;
    check_fields(requested, false, "")?;
    plan_fields(written, requested, "", &mut Vec::new())
}

/// Plan one checked table, refusing only a missing required field.
fn plan_fields<'a>(
    written: &[Shape],
    requested: &'a [Shape],
    path: &str,
    active: &mut Vec<(String, String)>,
) -> Planned<Vec<FieldPlan<'a>>> {
    let mut plans = Vec::with_capacity(requested.len());
    for target in requested {
        let identity = hash::hex_string(&target.identity);
        let field_path = format!("{path}/fields/{identity}");
        match written.iter().rposition(|field| field.identity == target.identity) {
            Some(index) => {
                let conversion = compatible(&written[index].storage, &target.storage, &field_path, active)?;
                plans.push(FieldPlan {
                    requested: target,
                    source: Some(index),
                    conversion,
                });
            }
            None if target.absent == 'r' => return Err(refusal("MissingField", &field_path)),
            None => plans.push(FieldPlan {
                requested: target,
                source: None,
                conversion: Plan::Identity,
            }),
        }
    }
    Ok(plans)
}

/// Require exact identities, stored order where written, unique members and admissible absence.
fn check_fields(fields: &[Shape], ordered: bool, prefix: &str) -> Planned<()> {
    let mut seen: Vec<String> = Vec::with_capacity(fields.len());
    for (index, field) in fields.iter().enumerate() {
        if field.identity.len() != 6 {
            return Err(refusal("IdentityWidth", &format!("{prefix}/fields/{index}/id")));
        }
        let identity = hash::hex_string(&field.identity);
        let path = format!("{prefix}/fields/{identity}");
        if seen.contains(&identity) {
            return Err(refusal("DuplicateIdentity", &path));
        }
        if ordered && seen.last().is_some_and(|previous| identity < *previous) {
            return Err(refusal("IdentityOrder", &format!("{prefix}/fields")));
        }
        seen.push(identity);
        check_nested(&field.storage, ordered, &path)?;
        if prefix.is_empty()
            && (hash::selector_spelling(&field.storage).is_none() || hash::shape_spelling(&field.storage).is_none())
        {
            return Err(refusal("InvalidType", &path));
        }
        if !allows_absence(&field.storage, field.absent) {
            return Err(refusal("InvalidType", &path));
        }
    }
    Ok(())
}

/// Only outer option and list, set or map emptiness have implicit stored values.
fn allows_absence(node: &Node, policy: char) -> bool {
    match (policy, node) {
        ('r', _) => true,
        ('n', Node::Container(kind, _)) => kind == "option",
        ('e', Node::Container(kind, _)) => matches!(kind.as_str(), "list" | "set" | "map"),
        _ => false,
    }
}

/// Validate every nested table, preserving written order instead of sorting corruption away.
fn check_nested(node: &Node, ordered: bool, path: &str) -> Planned<()> {
    match node {
        Node::Nominal(..) => Err(refusal("MissingDescription", path)),
        Node::Container(_, items) => items.iter().try_for_each(|item| check_nested(item, ordered, path)),
        Node::Record(_, fields) => check_fields(fields, ordered, path),
        Node::Enum(_, variants) => {
            let mut seen: Vec<String> = Vec::with_capacity(variants.len());
            for (identity, fields) in variants {
                if identity.len() != 6 {
                    return Err(refusal("IdentityWidth", &format!("{path}/variants")));
                }
                let name = hash::hex_string(identity);
                if seen.contains(&name) {
                    return Err(refusal("DuplicateIdentity", &format!("{path}/variants/{name}")));
                }
                if ordered && seen.last().is_some_and(|previous| name < *previous) {
                    return Err(refusal("IdentityOrder", &format!("{path}/variants")));
                }
                check_fields(fields, ordered, &format!("{path}/variants/{name}"))?;
                seen.push(name);
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// Plan one source-to-target conversion; nominal equality precedes nested shape adaptation.
fn compatible(source: &Node, target: &Node, path: &str, active: &mut Vec<(String, String)>) -> Planned<Plan> {
    if matches!(source, Node::Nominal(..)) || matches!(target, Node::Nominal(..)) {
        return Err(refusal("MissingDescription", path));
    }
    if let Node::Container(kind, items) = target
        && kind == "option"
    {
        if let Node::Container(original_kind, original) = source
            && original_kind == "option"
        {
            return Ok(Plan::Optional(Box::new(compatible(
                &original[0],
                &items[0],
                path,
                active,
            )?)));
        }
        return Ok(Plan::Wrap(Box::new(compatible(source, &items[0], path, active)?)));
    }
    let incompatible = || Err(refusal("IncompatibleType", path));
    match (source, target) {
        (Node::Record(key, fields), Node::Record(target_key, target_fields)) => {
            active.push((key.clone(), target_key.clone()));
            let result = plan_fields(fields, target_fields, path, active);
            active.pop();
            result.map(|_| Plan::Nested)
        }
        (Node::Enum(key, variants), Node::Enum(target_key, target_variants)) => {
            active.push((key.clone(), target_key.clone()));
            let result = plan_variants(variants, target_variants, path, active);
            active.pop();
            result.map(|_| Plan::Nested)
        }
        (Node::Recursive(key), Node::Recursive(target_key)) => {
            if active.iter().any(|(left, right)| left == key && right == target_key) {
                Ok(Plan::Nested)
            } else {
                incompatible()
            }
        }
        (Node::Container(kind, original), Node::Container(target_kind, items)) => {
            if kind == "union" && target_kind == "union" {
                return plan_members(original, items, path, active).map(|_| Plan::Nested);
            }
            if kind != target_kind || items.len() != original.len() {
                return incompatible();
            }
            let mut steps = Vec::with_capacity(items.len());
            for (item, target_item) in original.iter().zip(items) {
                steps.push(compatible(item, target_item, path, active)?);
            }
            Ok(Plan::Container(kind.clone(), steps))
        }
        (Node::Scalar(name), Node::Scalar(target_name)) if name == target_name => Ok(Plan::Identity),
        (Node::Scalar(name), Node::Scalar(target_name)) if widening(name, target_name) => {
            Ok(Plan::Widen(target_name.clone()))
        }
        (Node::Decimal(precision, scale), Node::Decimal(target_precision, target_scale))
            if precision == target_precision && scale == target_scale =>
        {
            Ok(Plan::Identity)
        }
        _ => incompatible(),
    }
}

/// Integer range inclusion and exact binary32 widening, independent of any stored value.
fn widening(source: &str, target: &str) -> bool {
    if source == "fp32" && target == "fp64" {
        return true;
    }
    let (Some((source_signed, source_bits)), Some((target_signed, target_bits))) =
        (integer_facts(source), integer_facts(target))
    else {
        return false;
    };
    if source_signed == target_signed {
        return source_bits <= target_bits;
    }
    !source_signed && target_signed && source_bits < target_bits
}

/// Signedness and bit width of one integer scalar spelling.
fn integer_facts(name: &str) -> Option<(bool, u32)> {
    let signed = name.starts_with('i');
    if !signed && !name.starts_with('u') {
        return None;
    }
    let bits: u32 = name[1..].parse().ok()?;
    matches!(bits, 8 | 16 | 32 | 64 | 128).then_some((signed, bits))
}

/// Every written variant needs a requested identity; payloads follow ordinary field rules.
fn plan_variants(
    written: &[(Vec<u8>, Vec<Shape>)],
    requested: &[(Vec<u8>, Vec<Shape>)],
    path: &str,
    active: &mut Vec<(String, String)>,
) -> Planned<()> {
    for (identity, fields) in written {
        let variant_path = format!("{path}/variants/{}", hash::hex_string(identity));
        let Some((_, target_fields)) = requested.iter().rev().find(|(target, _)| target == identity) else {
            return Err(refusal("IncompatibleType", &variant_path));
        };
        plan_fields(fields, target_fields, &variant_path, active)?;
    }
    Ok(())
}

/// Match closed union discriminators, never a same-shaped alternative.
fn plan_members(written: &[Node], requested: &[Node], path: &str, active: &mut Vec<(String, String)>) -> Planned<()> {
    let mut targets = Vec::with_capacity(requested.len());
    for member in requested {
        targets.push(hash::selector_spelling(member).ok_or_else(|| refusal("InvalidType", path))?);
    }
    for member in written {
        let name = hash::selector_spelling(member).ok_or_else(|| refusal("InvalidType", path))?;
        let Some(index) = targets.iter().rposition(|target| *target == name) else {
            return Err(refusal("IncompatibleType", path));
        };
        compatible(member, &requested[index], path, active)?;
    }
    Ok(())
}

/// Apply one plan to a tagged value, preserving exact integer strings and optional layers.
fn convert(value: &Json, plan: &Plan) -> Option<Json> {
    match plan {
        Plan::Identity => Some(value.clone()),
        Plan::Widen(target) => {
            let mut object = value.as_object()?.clone();
            object.insert("type".to_string(), Json::String(target.clone()));
            Some(Json::Object(object))
        }
        Plan::Wrap(child) => Some(some(convert(value, child)?)),
        Plan::Optional(child) => {
            if value.get("kind")?.as_str()? == "none" {
                return Some(value.clone());
            }
            Some(some(convert(value.get("value")?, child)?))
        }
        Plan::Container(kind, children) if matches!(kind.as_str(), "list" | "set") => {
            let mut items = Vec::new();
            for item in value.get("items")?.as_array()? {
                items.push(convert(item, children.first()?)?);
            }
            Some(collection(kind, items))
        }
        _ => None,
    }
}

/// Materialize only the absence the plan admitted, never a nonempty default.
fn absent_value(requested: &Shape) -> Option<Json> {
    match (requested.absent, &requested.storage) {
        ('n', _) => Some(tagged("none")),
        ('e', Node::Container(kind, _)) if matches!(kind.as_str(), "list" | "set") => {
            Some(collection(kind, Vec::new()))
        }
        _ => None,
    }
}

/// Wrap one present value in an explicit optional layer.
fn some(value: Json) -> Json {
    let mut object = Map::new();
    object.insert("kind".to_string(), Json::String("some".to_string()));
    object.insert("value".to_string(), value);
    Json::Object(object)
}

/// A tagged list or set notation with its items.
fn collection(kind: &str, items: Vec<Json>) -> Json {
    let mut object = Map::new();
    object.insert("kind".to_string(), Json::String(kind.to_string()));
    object.insert("items".to_string(), Json::Array(items));
    Json::Object(object)
}

/// A payload-free tagged notation.
fn tagged(kind: &str) -> Json {
    let mut object = Map::new();
    object.insert("kind".to_string(), Json::String(kind.to_string()));
    Json::Object(object)
}

/// Build one refusal for a path.
fn refusal(kind: &'static str, path: &str) -> Refusal {
    Refusal {
        kind,
        path: path.to_string(),
    }
}
