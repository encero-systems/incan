//! Exact exclusive scalar parameter carriers preserve their checked referent and shape.

use super::{Scalar, source_parameter_type, source_return_type};

/// Exclusive scalar parameters borrow one supported referent; they do not introduce reference-valued returns.
#[test]
fn exclusive_scalar_inputs_keep_exact_referents() {
    for referent in [
        Scalar::Int,
        Scalar::Float,
        Scalar::Bool,
        Scalar::I32,
        Scalar::Decimal,
        Scalar::Unit,
    ] {
        let borrowed = Scalar::ScalarMutRef(vec![referent]);
        assert!(source_parameter_type(&borrowed));
        assert!(!source_return_type(&borrowed));
    }
}

/// Malformed arity, owned text and nested exclusive references cannot acquire scalar-input admission.
#[test]
fn exclusive_scalar_inputs_refuse_malformed_referents() {
    for elements in [
        vec![],
        vec![Scalar::Int, Scalar::Bool],
        vec![Scalar::String],
        vec![Scalar::ScalarMutRef(vec![Scalar::Int])],
    ] {
        assert!(!source_parameter_type(&Scalar::ScalarMutRef(elements)));
    }
}
