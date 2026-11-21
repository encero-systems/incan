//! Borrowed scalar frame storage reads its value while logical reference values retain their native carrier.

use incan_mir_plan_native::{Local, Place, PlanType, Projection, source_span};

use crate::scalar_reads::scalar_storage_place;

/// Numeric, Boolean and unit reads dereference the original slot without a clone or temporary slot.
#[test]
fn borrowed_scalar_storage_reads_the_same_place() -> Result<(), String> {
    let span = source_span("scalar_reads.incn", 3, 7);
    let source = Place {
        local: 0,
        projection: Projection::Whole,
        span: span.clone(),
    };
    let references: Vec<i64> = Vec::new();
    for ty in [
        PlanType::Int,
        PlanType::Float,
        PlanType::I32,
        PlanType::U16,
        PlanType::Bool,
        PlanType::Unit,
        PlanType::Decimal,
    ] {
        for storage in [
            PlanType::ScalarRef(vec![ty.clone()]),
            PlanType::ScalarMutRef(vec![ty.clone()]),
        ] {
            let locals = vec![Local {
                ty: storage,
                span: span.clone(),
            }];
            let expected = Place {
                local: 0,
                projection: Projection::Deref(ty.clone()),
                span: span.clone(),
            };
            assert!(scalar_storage_place(&locals, &references, source.clone())? == expected);
            assert!(scalar_storage_place(&locals, &[0], source.clone())? == source);
        }
    }
    Ok(())
}

/// Already dereferenced exclusive storage and logical reference fields keep their complete checked paths.
#[test]
fn scalar_reads_preserve_existing_projections_and_other_carriers() -> Result<(), String> {
    let span = source_span("scalar_reads.incn", 4, 2);
    let source = Place {
        local: 0,
        projection: Projection::Whole,
        span: span.clone(),
    };
    let references: Vec<i64> = Vec::new();
    let local = |ty: PlanType| Local { ty, span: span.clone() };
    for ty in [
        PlanType::Int,
        PlanType::StringRef,
        PlanType::ScalarRef(vec![PlanType::StringRef]),
        PlanType::ScalarMutRef(vec![PlanType::StringRef]),
    ] {
        assert!(scalar_storage_place(&[local(ty)], &references, source.clone())? == source);
    }
    let locals = vec![local(PlanType::ScalarMutRef(vec![PlanType::Int]))];
    let projected = Place {
        local: 0,
        projection: Projection::Deref(PlanType::Int),
        span: span.clone(),
    };
    assert!(scalar_storage_place(&locals, &references, projected.clone())? == projected);
    let field = Place {
        local: 0,
        projection: Projection::Field(1, PlanType::ScalarRef(vec![PlanType::Int])),
        span: span.clone(),
    };
    assert!(scalar_storage_place(&locals, &references, field.clone())? == field);
    let invalid: [Vec<PlanType>; 2] = [Vec::new(), vec![PlanType::Int, PlanType::Bool]];
    for elements in invalid {
        for storage in [
            PlanType::ScalarRef(elements.clone()),
            PlanType::ScalarMutRef(elements.clone()),
        ] {
            assert!(scalar_storage_place(&[local(storage)], &references, source.clone()).is_err());
        }
    }
    Ok(())
}
