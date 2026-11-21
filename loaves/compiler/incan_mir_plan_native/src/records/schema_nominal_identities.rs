//! Referenced descriptions of the native-plan profile's `nominal_identities` shapes.

use crate::wire::{Shape, ShapeBody, field, variant};

pub(crate) static SHAPE_NOMINAL_IDENTITIES_NOMINAL_KIND: Shape = Shape {
    selector: b"\xab\xff/G\x8c,\x00",
    path: "incan.native-plan/5.nominal_identities.NominalKind",
    body: ShapeBody::Enum(&[
        variant(b"^,aL#\xf0", "Model", &[]),
        variant(b"T\x10\x8d\xaf\xaa\xc5", "Enum", &[]),
    ]),
};

pub(crate) static SHAPE_NOMINAL_IDENTITIES_PRODUCT_NOMINAL: Shape = Shape {
    selector: b"[\x08V#\xfe\xf4\x00",
    path: "incan.native-plan/5.nominal_identities.ProductNominal",
    body: ShapeBody::Record(&[
        field(b"\x0b'\xc1X\xfe\xb3", "kind", "enum@abff2f478c2c"),
        field(b"h\x9fjbs\x84", "identity", "string"),
        field(b"\x0b\xf4t\x89cc", "digest", "string"),
    ]),
};
