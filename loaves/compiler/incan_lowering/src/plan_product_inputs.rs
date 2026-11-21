//! The checked inputs that key a body's plan product, shared by ordinary frames and closed generic instances.

use std::collections::BTreeMap;

use incan_semantics_core::body_ir::{Body, BodyIrModule};
use incan_semantics_core::semantic_digest::semantic_digest;
use incan_semantics_core::{CompilerNodeId, SymbolNamespace};
use sha2::{Digest, Sha256};

use crate::async_::evaluator_name;
use crate::body_positions::body_position_map;
use crate::checked_values::location;
use crate::scalar_store::{record_scalar_inputs, scalar_inputs};

/// A body's product key inputs and, for a records key, its node position map.
pub type ProductInputs = (Vec<String>, Vec<(i64, i64)>);

/// The immutable producer profile for this request's selected product directory.
pub fn producer_generation(products: Option<&str>) -> Option<String> {
    let directory = products?;
    std::fs::read(std::path::Path::new(directory).join("producer-profile"))
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
}

/// Retain exact checked input only for frames whose complete read witnesses the source producer proved.
///
/// A records key is the checked meaning; the body's node positions return beside it as the product's position map. A
/// body no consumer checks, such as a toolchain runtime generic, keys by `unchecked` in place of a witness. `Ok(None)`
/// is a body with no product.
#[allow(clippy::too_many_arguments)]
pub fn ordinary_product_inputs(
    context: &BodyIrModule,
    body: &Body,
    source: &str,
    file: &str,
    overflow_checks: bool,
    products: Option<&str>,
    admitted: &BTreeMap<CompilerNodeId, String>,
    records: bool,
    generation: Option<&str>,
    unchecked: Option<&str>,
) -> Result<Option<ProductInputs>, String> {
    if products.is_none() {
        return Ok(None);
    }
    let witness = match admitted.get(&body.direct_call_id) {
        Some(value) => value.clone(),
        None => match unchecked {
            Some(value) => value.to_owned(),
            None => return Ok(None),
        },
    };
    let span = location(&body.span, source, file);
    let name = evaluator_name(context, body);
    if records {
        // A records key is the checked meaning; positions travel beside it, and without a producer profile the body
        // has no product.
        let Ok(meaning) = semantic_digest(body) else {
            return Ok(None);
        };
        let positions = body_position_map(body, source, file, &span);
        let Some(profile) = generation else {
            return Ok(None);
        };
        let mut inputs = record_scalar_inputs(&meaning, &name, &span, overflow_checks, &witness, positions.len());
        inputs.push(profile.to_owned());
        return Ok(Some((inputs, positions)));
    }
    // The cold scalar witness includes all checked fields and physical positions; only its digest enters the graph.
    let Ok(encoded) = postcard::to_allocvec(body) else {
        return Ok(None);
    };
    let exact = format!("sha256:{}", hex::encode(Sha256::digest(&encoded)));
    Ok(Some((
        scalar_inputs(&exact, &name, &span, overflow_checks, &witness),
        Vec::new(),
    )))
}

/// Whether the compiled graph records this body as an owner: a module-level declaration, never a member.
pub fn graph_owner(body: &Body) -> bool {
    body.canonical
        .as_ref()
        .is_some_and(|identity| identity.namespace == SymbolNamespace::OrdinaryLexical)
}
