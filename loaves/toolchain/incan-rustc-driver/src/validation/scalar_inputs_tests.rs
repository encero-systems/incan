//! Exact exclusive scalar parameter carriers preserve their checked referent and shape.

use super::{Scalar, frozen_copy_error, source_parameter_type, source_return_type};

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

/// Frozen carriers retain source copy behavior only through Clone, so invalid native Copy plans name their owner.
#[test]
fn frozen_native_copies_name_the_source_declaration() {
    for carrier in [
        Scalar::FrozenStr,
        Scalar::FrozenBytes,
        Scalar::FrozenCollection("FrozenList".into(), vec![Scalar::FrozenStr]),
    ] {
        let error = frozen_copy_error("fully_perform", &carrier).to_string();
        assert!(error.starts_with("invalid plan in `fully_perform`:"));
        assert!(error.contains("must be cloned instead of copied"));
    }
}
