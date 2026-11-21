//! A closed generic instance's plan product keeps the generic loans its signature carries.

use super::support::empty_function;
use crate::plan::GenericLoan;
use crate::records::function::{decode_product, encode_product, product_nominals};
use crate::types::{PlanType, source_span};

/// Each type parameter occurrence in a closed instance's signature survives its product with its concrete type.
#[test]
fn instance_product_keeps_its_generic_loans() -> Result<(), String> {
    let span = source_span("generic.incn", 1, 1);
    let mut instance = empty_function("pick_3_105_110_116", PlanType::Int, &span);
    instance.generic_loans = vec![
        GenericLoan {
            slot: 0,
            path: vec![0],
            parameter: 0,
            ty: PlanType::Int,
        },
        GenericLoan {
            slot: 1,
            path: Vec::new(),
            parameter: 0,
            ty: PlanType::String,
        },
    ];
    let references = product_nominals(&instance, &[], &[])?;
    let product = encode_product(&instance, &[], &[], &[])?;
    let (restored, _runtime) = decode_product(&product, &references)?;
    assert!(
        restored == instance,
        "the instance's generic loans did not survive its product"
    );
    Ok(())
}
