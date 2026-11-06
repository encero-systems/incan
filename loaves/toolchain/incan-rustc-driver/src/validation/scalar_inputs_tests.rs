//! Exact exclusive scalar parameter carriers preserve their checked referent and shape.

use super::{Scalar, source_parameter_type, source_return_type};

/// Exclusive scalar parameters and returned reborrows preserve one supported scalar or reference value.
#[test]
fn exclusive_scalar_inputs_keep_exact_referents() {
    for referent in [
        Scalar::Int,
        Scalar::Float,
        Scalar::Bool,
        Scalar::I32,
        Scalar::Decimal,
        Scalar::Unit,
        Scalar::StringMutRef,
        Scalar::ScalarMutRef(vec![Scalar::Int]),
    ] {
        let borrowed = Scalar::ScalarMutRef(vec![referent]);
        assert!(source_parameter_type(&borrowed));
        assert!(source_return_type(&borrowed));
    }
}

/// Malformed arity and owned text cannot acquire scalar-input admission.
#[test]
fn exclusive_scalar_inputs_refuse_malformed_referents() {
    for elements in [vec![], vec![Scalar::Int, Scalar::Bool], vec![Scalar::String]] {
        assert!(!source_parameter_type(&Scalar::ScalarMutRef(elements)));
    }
}
