//! Field reads, variant reads and nominal constructions name their owner by declaration identity in keyed products.

use super::support::{assign_at, block, copy_at, literal, local, place_at, returning};
use crate::body::{Callee, CalleeKind, FieldProjection, Projection, RvalueKind, Terminator, TerminatorKind, Unwind};
use crate::nominal::{NominalKind, ProductNominal, describe_nominals, resolve_nominals, unit_nominals};
use crate::plan::Function;
use crate::records::function::{decode_product, encode_product, product_identities, product_nominals};
use crate::types::{PlanType, SourceSpan, source_span};

/// A product that constructs `Point` and `Shape`, reads their fields and payload, tests and clones them, binds to
/// their current positions after unrelated declarations move them, and is byte-identical to a fresh capture there.
#[test]
fn reads_and_constructions_bind_to_current_owners() -> Result<(), String> {
    let span = source_span("nominal_operations.incn", 3, 1);
    let before = unit_nominals(
        vec![point()],
        vec![shape()],
        vec!["Point_0_37".to_owned()],
        vec!["Shape_39_75".to_owned()],
    );
    let captured = operations(0, 0, "Point_0_37", "Shape_39_75", &span);
    let named = describe_nominals(&product_nominals(&captured, &[], &[])?, &before)?;
    let product = encode_product(&captured, &[], &named, &[])?;
    let after = unit_nominals(
        vec![nominal(NominalKind::Model, "model:app::Line", "line-v1"), point()],
        vec![nominal(NominalKind::Enum, "enum:app::Color", "color-v1"), shape()],
        vec!["Line_0_20".to_owned(), "Point_22_59".to_owned()],
        vec!["Color_61_90".to_owned(), "Shape_92_128".to_owned()],
    );
    let moved = operations(1, 1, "Point_22_59", "Shape_92_128", &span);
    let moved_named = describe_nominals(&product_nominals(&moved, &[], &[])?, &after)?;
    let fresh = encode_product(&moved, &[], &moved_named, &[])?;
    assert!(
        fresh == product,
        "the product depends on its owners' positions or native names"
    );
    let references = resolve_nominals(&product_identities(&product)?, &after)
        .ok_or("unrelated declarations made a reused product miss")?;
    let (restored, _runtime) = decode_product(&product, &references)?;
    assert!(
        restored == moved,
        "the reused product kept its producing unit's owner positions"
    );
    Ok(())
}

/// Construct a `Point` and a `Shape` payload, read a field, the payload and the tag, test a variant, clone `Point`.
fn operations(
    model_position: i64,
    enum_position: i64,
    point_name: &str,
    shape_name: &str,
    span: &SourceSpan,
) -> Function {
    let point_type = PlanType::Model(model_position, point_name.to_owned());
    let shape_type = PlanType::Enum(enum_position, shape_name.to_owned());
    let whole = || Projection::Whole;
    let statements = vec![
        assign_at(
            1,
            whole(),
            RvalueKind::Model(model_position, vec![literal(3, span), literal(4, span)]),
            span,
        ),
        assign_at(
            2,
            whole(),
            RvalueKind::Enum(enum_position, 1, vec![literal(5, span)]),
            span,
        ),
        assign_at(
            3,
            whole(),
            RvalueKind::Use(copy_at(1, Projection::Field(0, PlanType::Int), span)),
            span,
        ),
        assign_at(
            3,
            whole(),
            RvalueKind::Use(copy_at(2, Projection::VariantField(1, 0, PlanType::Int), span)),
            span,
        ),
        assign_at(
            3,
            whole(),
            RvalueKind::Use(copy_at(
                1,
                Projection::Fields(vec![FieldProjection::new(1, PlanType::Int)]),
                span,
            )),
            span,
        ),
        assign_at(4, whole(), RvalueKind::Discriminant(place_at(2, whole(), span)), span),
        assign_at(5, whole(), RvalueKind::IsVariant(place_at(2, whole(), span), 1), span),
        assign_at(
            6,
            whole(),
            RvalueKind::Tuple(vec![copy_at(3, whole(), span), copy_at(3, whole(), span)]),
            span,
        ),
    ];
    let clone = TerminatorKind::Call(
        Callee {
            kind: CalleeKind::CloneModel(model_position, point_name.to_owned()),
            span: span.clone(),
        },
        vec![copy_at(1, whole(), span)],
        place_at(7, whole(), span),
        1,
        Unwind::Continue,
    );
    Function::new(
        "operate".to_owned(),
        Vec::new(),
        PlanType::Unit,
        vec![
            local(PlanType::Unit, span),
            local(point_type.clone(), span),
            local(shape_type, span),
            local(PlanType::Int, span),
            local(PlanType::EnumTag, span),
            local(PlanType::Bool, span),
            local(PlanType::Tuple(Vec::new()), span),
            local(point_type, span),
        ],
        vec![
            block(
                statements,
                Terminator {
                    kind: clone,
                    span: span.clone(),
                },
                span,
            ),
            block(Vec::new(), returning(span), span),
        ],
        span.clone(),
    )
}

/// One described declaration.
fn nominal(kind: NominalKind, identity: &str, digest: &str) -> ProductNominal {
    ProductNominal {
        kind,
        identity: identity.to_owned(),
        digest: digest.to_owned(),
    }
}

/// The `Point` model declaration.
fn point() -> ProductNominal {
    nominal(NominalKind::Model, "model:app::Point", "point-v1")
}

/// The `Shape` enum declaration.
fn shape() -> ProductNominal {
    nominal(NominalKind::Enum, "enum:app::Shape", "shape-v1")
}
