//! A plan product carries the generic instance requests its lowering made, authenticated with its function.
//!
//! The reference keys these products by a records key with a node position map; placing positions is
//! `record_positions` work that is not yet in Rust, so these use the ordinary key, whose products carry requests the
//! same way.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use incan_mir_plan_native::record_types::{CheckedPrimitiveType, CheckedType};
use incan_mir_plan_native::{
    Function, Local, PlanType, SourceSpan, TerminatorKind, block, decode_instance_request, decode_plan_function_record,
    encode_instance_request, encode_plan_function_record, source_span, unit_nominals,
};

use crate::scalar_store::{retain_scalar, retained_scalar, scalar_inputs};

/// A unit function whose block and return sit on the owner's first body line.
fn entry_function(name: &str, owner: &SourceSpan) -> Function {
    let block_span = source_span("requests.incn", 11, 5);
    let locals = vec![Local {
        ty: PlanType::Unit,
        span: owner.clone(),
    }];
    let blocks = vec![block(Vec::new(), TerminatorKind::Return, &block_span)];
    Function::new(
        name.to_owned(),
        Vec::new(),
        PlanType::Unit,
        locals,
        blocks,
        owner.clone(),
    )
}

/// A fresh private product directory for one test.
fn products(test: &str) -> Result<PathBuf, String> {
    let directory = std::env::temp_dir().join(format!("incan-lowering-instance-{test}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    Ok(directory)
}

/// The single retained product record.
fn only_product(products: &Path) -> Result<PathBuf, String> {
    for entry in std::fs::read_dir(products).map_err(|error| error.to_string())? {
        let path = entry.map_err(|error| error.to_string())?.path();
        if path.to_string_lossy().ends_with(".incnfunction") {
            return Ok(path);
        }
    }
    Err("no plan product was retained".into())
}

fn instance_row(argument: CheckedType) -> Result<Vec<u8>, String> {
    encode_instance_request("generic.pick", &[argument], &[1, 3], true)
}

fn text(directory: &Path) -> String {
    directory.to_string_lossy().into_owned()
}

/// A reused caller still schedules the instances its lowering requested, in request order.
#[test]
fn a_caller_product_returns_its_instance_requests() -> Result<(), String> {
    let directory = products("requests")?;
    let owner = source_span("requests.incn", 10, 1);
    let function = entry_function("main", &owner);
    let inputs = scalar_inputs("sha256:checked", &function.name, &owner, false, "reads");
    let requests = vec![
        instance_row(CheckedType::Primitive(CheckedPrimitiveType::Int))?,
        instance_row(CheckedType::Primitive(CheckedPrimitiveType::Str))?,
    ];
    assert_eq!(&requests[0][..4], b"CRMB");
    let (declaration, arguments, slots, original) = decode_instance_request(&requests[0])?;
    assert_eq!(declaration, "generic.pick");
    assert!(arguments == vec![CheckedType::Primitive(CheckedPrimitiveType::Int)]);
    assert!(slots == vec![1, 3] && original);
    retain_scalar(&text(&directory), &inputs, &function, &[], &[], &[], &[], &requests)?;
    let unit = unit_nominals(Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let Some((_, _, _, _, restored)) =
        retained_scalar(&text(&directory), &inputs, &unit, &BTreeSet::new(), Some(&owner), &[])
    else {
        return Err("a caller product with instance requests was a miss".into());
    };
    assert!(
        restored == requests,
        "the reused caller lost or reordered its instance requests"
    );
    let _ = std::fs::remove_dir_all(&directory);
    Ok(())
}

/// The private plan-function record is one Crumb envelope, never a JSON document with an embedded product.
#[test]
fn a_retained_product_is_a_crumb_row() -> Result<(), String> {
    let directory = products("crumb")?;
    let owner = source_span("requests.incn", 10, 1);
    let function = entry_function("main", &owner);
    let inputs = scalar_inputs("sha256:checked", &function.name, &owner, false, "reads");
    retain_scalar(&text(&directory), &inputs, &function, &[], &[], &[], &[], &[])?;
    let encoded = std::fs::read(only_product(&directory)?).map_err(|error| error.to_string())?;
    assert_eq!(&decode_plan_function_record(&encoded)?.function[..4], b"CRMB");
    assert_eq!(
        &encoded[..4],
        b"CRMB",
        "a retained plan-function product is not a Crumb envelope"
    );
    let _ = std::fs::remove_dir_all(&directory);
    Ok(())
}

/// A product's requests are authenticated with its function, so an edited request is never scheduled.
#[test]
fn an_altered_instance_request_makes_the_product_a_miss() -> Result<(), String> {
    let directory = products("altered")?;
    let owner = source_span("requests.incn", 10, 1);
    let function = entry_function("main", &owner);
    let inputs = scalar_inputs("sha256:checked", &function.name, &owner, false, "reads");
    let requests = vec![instance_row(CheckedType::Primitive(CheckedPrimitiveType::Int))?];
    retain_scalar(&text(&directory), &inputs, &function, &[], &[], &[], &[], &requests)?;
    let path = only_product(&directory)?;
    let mut record = decode_plan_function_record(&std::fs::read(&path).map_err(|error| error.to_string())?)?;
    record.requests = vec![instance_row(CheckedType::Primitive(CheckedPrimitiveType::Str))?];
    std::fs::write(&path, encode_plan_function_record(&record)?).map_err(|error| error.to_string())?;
    let unit = unit_nominals(Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let restored = retained_scalar(&text(&directory), &inputs, &unit, &BTreeSet::new(), Some(&owner), &[]);
    assert!(restored.is_none(), "an edited request was trusted");
    let _ = std::fs::remove_dir_all(&directory);
    Ok(())
}
