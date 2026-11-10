//! Exact exclusive scalar parameter carriers preserve their checked referent and shape.

use super::{Leaf, Scalar, source_parameter_type, source_return_type};

/// Source parameters preserve exact exclusive scalar referents and shared string callable inputs.
#[test]
fn source_parameters_keep_exact_borrowed_referents() {
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
    for input in [Scalar::StringRef, Scalar::StrRef] {
        assert!(source_parameter_type(&Scalar::BorrowingClosure(vec![
            input,
            Scalar::Int
        ])));
    }
    assert!(source_parameter_type(&Scalar::BorrowingClosure(vec![
        Scalar::List(Leaf::Int, 1),
        Scalar::Int,
    ])));
}

/// Malformed arity and owned text cannot acquire scalar-input admission.
#[test]
fn exclusive_scalar_inputs_refuse_malformed_referents() {
    for elements in [vec![], vec![Scalar::Int, Scalar::Bool], vec![Scalar::String]] {
        assert!(!source_parameter_type(&Scalar::ScalarMutRef(elements)));
    }
}
