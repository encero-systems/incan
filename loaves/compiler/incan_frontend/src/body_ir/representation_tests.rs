//! Exact representation-operation comparisons over the same checked seed. These are migration baselines, not
//! producers: only this harness assembles a whole module to compare the independent native operations.

use super::*;
use crate::typechecker::{CheckedBodyInputError, TypeCheckInfo, TypeChecker};
use std::sync::atomic::{AtomicUsize, Ordering};

mod baselines;
mod carriers;
mod comparison;
mod equality;
mod metadata;
mod read_patterns;
mod seed;
mod shared_loans;

use baselines::{assert_free_function_input_baseline, assert_settled_function_seeds, capture_module_baseline};
use carriers::{
    assert_builtin_method_identities, assert_frozen_literal_bound_method_identity,
    assert_generic_newtype_provider_methods, assert_generic_view_carrier_storage,
    assert_independently_retained_newtype_return, assert_native_provider_field_layout,
    assert_nested_generic_static_call_arguments, assert_nested_generic_static_call_bound_mismatch,
    assert_nominal_carrier_maps_stay_shared,
};
use comparison::assert_same_module;
use equality::assert_equality_contract;
use read_patterns::assert_declared_record_namespace;
use seed::checked_seed;
use shared_loans::{
    assert_complete_shared_read_shapes, assert_lent_and_moved_read, assert_projected_shared_loans, assert_shared_loans,
};

/// Compare complete serialized bodies and ordered union layouts after independent local operations, including an
/// empty sibling, repeated shapes and nested callable unions. Ordered values and coordinates are never removed.
#[test]
fn local_representation_operations_match_module_passes() -> Result<(), Box<dyn std::error::Error>> {
    let source = "trait Readable:\n    def value(self) -> int: ...\n    def again(self) -> Self:\n        return self\nmodel Value with Readable:\n    number: int\n    def value(self) -> int:\n        return self.number\ndef make() -> Readable:\n    return Value(number=1)\ndef forward() -> Readable:\n    return make()\ndef through(value: Readable) -> int:\n    return value.value()\ndef first() -> int:\n    value = forward()\n    return value.again().value()\ndef sibling() -> int:\n    return 2\n";
    let (program, facts, mut seed) = checked_seed(source)?;
    let callable = super::IncanType::Function {
        params: Vec::new(),
        return_type: Box::new(super::IncanType::Primitive(super::IncanPrimitiveType::Int)),
    };
    let shape = super::IncanType::Generic {
        base: "Union".to_owned(),
        args: vec![
            callable.clone(),
            super::IncanType::Primitive(super::IncanPrimitiveType::Str),
        ],
    };
    let carriers = opaque_returns::collect(&seed, &facts);
    let legacy_carriers = legacy_opaque_returns::collect(&seed, &facts);
    assert_eq!(carriers.len(), 1);
    let mut expected = seed.clone();
    super::legacy_trait_values::retain_inputs(&mut expected, &program, &facts, &[]);
    super::trait_values::retain_inputs(&mut seed, &program, &facts, &[]);
    assert_same_module(&expected, &seed)?;
    let through = seed
        .bodies
        .iter()
        .find(|body| body.name == "through")
        .ok_or("missing trait input body")?;
    assert_eq!(through.type_parameters.len(), 1);
    assert!(matches!(through.params[0].ty, super::IncanType::TypeVar(_)));
    super::legacy_trait_returns::retain_returns(&mut expected, &program, &facts, &[]);
    super::trait_returns::retain_returns(&mut seed, &program, &facts, &[]);
    assert_same_module(&expected, &seed)?;
    let forward = seed
        .bodies
        .iter()
        .find(|body| body.name == "forward")
        .ok_or("missing trait return body")?;
    assert!(matches!(&forward.return_type, super::IncanType::Named(name)
        if forward.named_type_identities.get(name).is_some_and(|identity| identity.declaration_name == "Value")));
    legacy_opaque_returns::retain(&mut expected, &legacy_carriers);
    for body in &mut seed.bodies {
        opaque_returns::retain_body(body, &carriers);
    }
    assert_same_module(&expected, &seed)?;

    seed.bodies
        .iter_mut()
        .find(|body| body.name == "first")
        .ok_or("missing first body")?
        .return_type = super::IncanType::Tuple(vec![shape.clone(), shape.clone()]);
    seed.nominal_declarations[0].field_types = vec![shape.clone()];
    seed.bodies
        .iter_mut()
        .find(|body| body.name == "sibling")
        .ok_or("missing sibling body")?
        .return_type = super::IncanType::Generic {
        base: "Union".to_owned(),
        args: vec![shape, callable],
    };

    let mut legacy = seed.clone();
    legacy_union_layouts::retain(&mut legacy);
    let mut independent = Vec::new();
    for body in &seed.bodies {
        union_layouts::append_types(&mut independent, union_layouts::body_types(body));
    }
    for owner in &seed.nominal_declarations {
        union_layouts::append_types(&mut independent, union_layouts::storage_types(owner.field_types.iter()));
    }
    let independent = independent
        .into_iter()
        .filter_map(union_layouts::layout)
        .collect::<Vec<_>>();
    assert_eq!(
        postcard::to_stdvec(&legacy.native_union_declarations)?,
        postcard::to_stdvec(&independent)?
    );
    assert_eq!(independent.len(), 2);

    assert!(union_layouts::storage_types(std::iter::empty()).is_empty());
    assert_settled_function_seeds()?;
    assert_free_function_input_baseline()?;
    assert_shared_loans()?;
    assert_projected_shared_loans()?;
    assert_complete_shared_read_shapes()?;
    assert_lent_and_moved_read()?;
    assert_equality_contract()?;
    assert_builtin_method_identities()?;
    assert_generic_view_carrier_storage()?;
    assert_nested_generic_static_call_arguments()?;
    assert_frozen_literal_bound_method_identity()?;
    assert_native_provider_field_layout()?;
    assert_declared_record_namespace()?;
    assert_independently_retained_newtype_return()?;
    assert_generic_newtype_provider_methods()?;
    assert_nested_generic_static_call_bound_mismatch()?;
    assert_nominal_carrier_maps_stay_shared()?;
    let mut changed = seed.clone();
    changed.bodies[0].native_attributes.push(bir::NativeAttribute {
        path: "selected::attribute".to_owned(),
        arguments: Vec::new(),
    });
    assert_eq!(postcard::to_stdvec(&seed)?, postcard::to_stdvec(&changed)?);
    let difference = assert_same_module(&seed, &changed)
        .err()
        .ok_or("baseline omitted an invocation-only field")?;
    assert!(difference.to_string().contains("native_attributes"));
    Ok(())
}
