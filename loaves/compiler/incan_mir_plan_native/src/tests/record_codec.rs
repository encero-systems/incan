//! Complete native-interface round trips include SDK layouts and selected entries, not only function instructions.

use super::support::empty_function;
use crate::body::Parameter;
use crate::plan::{Carrier, EnumDeclaration, EnumVariant, ExternalFunction, ModelDeclaration, Plan};
use crate::record_types::{CheckedPrimitiveType, CheckedType};
use crate::records::codec::{
    decode_record_plan, encode_record_plan, encode_record_plan_products, record_plan_products,
};
use crate::records::function::encode_product;
use crate::types::{ListLeaf, PlanType, source_span};
use crate::wire::read_wire;

/// Concrete collection fields and typed enum witnesses survive the exact complete-plan loss check.
#[test]
fn complete_plan_round_trip() -> Result<(), String> {
    let span = source_span("sdk_layout.incn", 4, 2);
    let function = empty_function("main", PlanType::Unit, &span);
    let mut plan = Plan::new(Vec::new(), Vec::new(), vec![function], Vec::new(), span.clone());
    plan.programs = vec!["main".to_owned()];
    let mut model = ModelDeclaration::new(
        "SDKRecord".to_owned(),
        true,
        vec![Parameter {
            name: "ids".to_owned(),
            ty: PlanType::List(ListLeaf::Int, 1),
            span: span.clone(),
        }],
        vec![true],
        vec!["Clone".to_owned()],
        span.clone(),
    );
    model.source_name = "SDKRecord".to_owned();
    model.library_item = "sdk::SDKRecord".to_owned();
    plan.models.push(model);
    let mut optional = EnumDeclaration::new(
        "OptionalText".to_owned(),
        false,
        vec![
            EnumVariant {
                name: "None".to_owned(),
                fields: Vec::new(),
            },
            EnumVariant {
                name: "Some".to_owned(),
                fields: vec![PlanType::String],
            },
        ],
        Vec::new(),
        span.clone(),
    );
    optional.carrier = Carrier::Option;
    optional.semantic_key = Some(CheckedType::Generic(
        "Option".to_owned(),
        vec![CheckedType::Primitive(CheckedPrimitiveType::Str)],
    ));
    plan.enums.push(optional);
    assert!(decode_record_plan(&encode_record_plan(&plan)?)? == plan);
    plan.tests.push("unsupported entry".to_owned());
    assert!(
        encode_record_plan(&plan).is_err(),
        "native-interface dropped a collected test entry"
    );
    Ok(())
}

/// A runtime demand owned by the second declaration stays on that exact keyed product in the whole plan.
#[test]
fn whole_plan_keeps_keyed_product_bytes() -> Result<(), String> {
    let span = source_span("keyed_products.incn", 1, 1);
    let first = empty_function("first", PlanType::Unit, &span);
    let second = empty_function("second", PlanType::Unit, &span);
    let runtime = ExternalFunction {
        path: "incan_native_runtime::println_text".to_owned(),
        type_arguments: Vec::new(),
        parameters: vec![PlanType::String],
        return_type: PlanType::Unit,
        span: span.clone(),
    };
    let second_runtime = vec![runtime];
    let keyed = vec![
        encode_product(&first, &[], &[], &[])?,
        encode_product(&second, &second_runtime, &[], &[])?,
    ];
    let plan = Plan::new(Vec::new(), Vec::new(), vec![first, second], second_runtime, span);
    let encoded = encode_record_plan_products(&plan, keyed.clone(), Vec::new())?;
    assert!(encoded.starts_with(b"CRMB"));
    assert!(keyed[0].starts_with(b"CRMB") && keyed[1].starts_with(b"CRMB"));
    assert!(record_plan_products(&encoded)? == keyed);
    let schema = &crate::records::wire_schema::STORED_PRODUCT_SCHEMA;
    assert!(
        read_wire(&keyed[0], b"mirpln\x00", schema).is_err(),
        "a function product was accepted under the unit plan selector"
    );
    assert!(
        encode_record_plan_products(&plan, vec![keyed[0].clone(), b"{}".to_vec()], Vec::new()).is_err(),
        "a textual child reached native-plan publication"
    );
    Ok(())
}
