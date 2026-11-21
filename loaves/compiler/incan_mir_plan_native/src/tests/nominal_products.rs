//! Keyed plan products name models and enums by declaration identity, never by a position in the unit's tables.

use super::support::{block, local, returning};
use crate::body::Parameter;
use crate::nominal::{
    NominalKind, NominalReference, ProductNominal, UnitNominals, describe_nominals, resolve_nominals, unit_nominals,
};
use crate::plan::{ExternalFunction, Function};
use crate::records::function::{decode_product, encode_product, product_identities, product_nominals};
use crate::types::{ListLeaf, PlanType, SourceSpan, source_span};

/// A product captured with `Point` at model 0 binds to model 1 after an unrelated model is declared before it.
///
/// The unrelated declaration also moves `Point`'s source position, which its native carrier name encodes; the
/// product retains neither, so the restored function takes the name `Point` now has.
#[test]
fn product_survives_an_unrelated_declaration() -> Result<(), String> {
    let span = source_span("nominal_products.incn", 3, 1);
    let before = unit(vec![point("v1")], vec![shape("v1")], &["Point_0_37"], &["Shape_39_75"]);
    let (captured, captured_runtime) = point_function(0, 0, "Point_0_37", "Shape_39_75", &span);
    let product = keyed_product(&captured, &captured_runtime, &before)?;
    assert!(product_identities(&product)? == [point("v1"), shape("v1")]);
    assert!(
        !product
            .windows(b"Point_0_37".len())
            .any(|window| window == b"Point_0_37"),
        "the product retained a position-dependent native name"
    );
    // The unrelated declarations shift both named declarations' positions; the product bytes do not depend on them.
    let after = unit(
        vec![line_model(), point("v1")],
        vec![color(), shape("v1")],
        &["Line_0_20", "Point_22_59"],
        &["Color_61_90", "Shape_92_128"],
    );
    let (moved, moved_runtime) = point_function(1, 1, "Point_22_59", "Shape_92_128", &span);
    assert!(keyed_product(&moved, &moved_runtime, &after)? == product);
    let references = resolve_nominals(&product_identities(&product)?, &after)
        .ok_or("an unrelated declaration made a reused product miss")?;
    assert!(references == [model_at(1, "Point_22_59"), enum_at(1, "Shape_92_128")]);
    let (restored, restored_runtime) = decode_product(&product, &references)?;
    assert!(
        restored == moved,
        "the reused product kept its producing unit's positions or names"
    );
    assert!(restored_runtime == moved_runtime);
    Ok(())
}

/// Changing or removing a declaration the product names refuses its reuse; an unnamed one does not.
#[test]
fn changed_declaration_makes_the_product_a_miss() -> Result<(), String> {
    let span = source_span("nominal_products.incn", 3, 1);
    let before = unit(vec![point("v1")], vec![shape("v1")], &["Point"], &["Shape"]);
    let (captured, captured_runtime) = point_function(0, 0, "Point", "Shape", &span);
    let identities = product_identities(&keyed_product(&captured, &captured_runtime, &before)?)?;
    let changed = unit(
        vec![line_model(), point("v2")],
        vec![shape("v1")],
        &["Line", "Point"],
        &["Shape"],
    );
    assert!(
        resolve_nominals(&identities, &changed).is_none(),
        "a changed model declaration was reused"
    );
    let changed_enum = unit(vec![point("v1")], vec![shape("v2")], &["Point"], &["Shape"]);
    assert!(
        resolve_nominals(&identities, &changed_enum).is_none(),
        "a changed enum declaration was reused"
    );
    let removed = unit(vec![line_model()], vec![shape("v1")], &["Line"], &["Shape"]);
    assert!(
        resolve_nominals(&identities, &removed).is_none(),
        "a removed declaration was reused"
    );
    let unrelated = unit(
        vec![point("v1"), line_model()],
        vec![shape("v1"), color()],
        &["Point", "Line"],
        &["Shape", "Color"],
    );
    assert!(resolve_nominals(&identities, &unrelated) == Some(vec![model_at(0, "Point"), enum_at(0, "Shape")]));
    Ok(())
}

/// A product cannot retain a model position the caller did not name by declaration identity.
#[test]
fn product_refuses_a_nominal_without_an_identity() -> Result<(), String> {
    let span = source_span("nominal_products.incn", 3, 1);
    let (captured, captured_runtime) = point_function(0, 0, "Point", "Shape", &span);
    match encode_product(&captured, &captured_runtime, &[point("v1")], &[]) {
        Ok(_) => Err("a product retained an enum position without its declaration identity".to_owned()),
        Err(message) => {
            assert!(message.contains("without a declaration identity"), "{message}");
            Ok(())
        }
    }
}

/// Capture a product the way lowering does: name every referenced position, then encode.
fn keyed_product(function: &Function, runtime: &[ExternalFunction], unit: &UnitNominals) -> Result<Vec<u8>, String> {
    let references = product_nominals(function, runtime, &[])?;
    encode_product(function, runtime, &describe_nominals(&references, unit)?, &[])
}

/// A function over `Point` and `Shape` at the given positions, with a runtime signature that also names `Point`.
fn point_function(
    model_position: i64,
    enum_position: i64,
    point_name: &str,
    shape_name: &str,
    span: &SourceSpan,
) -> (Function, Vec<ExternalFunction>) {
    let point_type = PlanType::Model(model_position, point_name.to_owned());
    let shape_type = PlanType::Enum(enum_position, shape_name.to_owned());
    let function = Function::new(
        "describe".to_owned(),
        vec![
            Parameter {
                name: "point".to_owned(),
                ty: point_type.clone(),
                span: span.clone(),
            },
            Parameter {
                name: "shape".to_owned(),
                ty: shape_type.clone(),
                span: span.clone(),
            },
        ],
        PlanType::Unit,
        vec![
            local(PlanType::Unit, span),
            local(point_type, span),
            local(shape_type, span),
            local(
                PlanType::List(ListLeaf::Model(model_position, point_name.to_owned()), 1),
                span,
            ),
        ],
        vec![block(Vec::new(), returning(span), span)],
        span.clone(),
    );
    let runtime = vec![ExternalFunction {
        path: "incan_std_primitives::inspect".to_owned(),
        type_arguments: Vec::new(),
        parameters: vec![PlanType::ModelRef(model_position, point_name.to_owned())],
        return_type: PlanType::String,
        span: span.clone(),
    }];
    (function, runtime)
}

/// Describe unit tables from declarations and their native names.
fn unit(
    models: Vec<ProductNominal>,
    enums: Vec<ProductNominal>,
    model_names: &[&str],
    enum_names: &[&str],
) -> UnitNominals {
    let owned = |names: &[&str]| names.iter().map(|name| (*name).to_owned()).collect();
    unit_nominals(models, enums, owned(model_names), owned(enum_names))
}

/// The `Point` model declaration at one checked meaning.
fn point(version: &str) -> ProductNominal {
    ProductNominal {
        kind: NominalKind::Model,
        identity: "model:app::Point".to_owned(),
        digest: format!("point-{version}"),
    }
}

/// The `Shape` enum declaration at one checked meaning.
fn shape(version: &str) -> ProductNominal {
    ProductNominal {
        kind: NominalKind::Enum,
        identity: "enum:app::Shape".to_owned(),
        digest: format!("shape-{version}"),
    }
}

/// An unrelated model declaration.
fn line_model() -> ProductNominal {
    ProductNominal {
        kind: NominalKind::Model,
        identity: "model:app::Line".to_owned(),
        digest: "line-v1".to_owned(),
    }
}

/// An unrelated enum declaration.
fn color() -> ProductNominal {
    ProductNominal {
        kind: NominalKind::Enum,
        identity: "enum:app::Color".to_owned(),
        digest: "color-v1".to_owned(),
    }
}

/// The current unit's model at one position.
fn model_at(index: i64, name: &str) -> NominalReference {
    NominalReference {
        kind: NominalKind::Model,
        index,
        name: name.to_owned(),
    }
}

/// The current unit's enum at one position.
fn enum_at(index: i64, name: &str) -> NominalReference {
    NominalReference {
        kind: NominalKind::Enum,
        index,
        name: name.to_owned(),
    }
}
