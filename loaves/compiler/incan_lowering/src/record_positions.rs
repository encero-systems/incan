//! Separate plan products from their source positions, and relocate products without lowering their bodies again.
//!
//! A product's spans are owner-relative, and each span that locates a Body IR node is stored as that node's ordinal.
//! The body's position map, its nodes' owner-relative lines and columns, is a request-local fact: a comment or
//! docstring that moves lines inside a body changes the map, never the product.

use std::collections::HashMap;

use incan_mir_plan_native::{
    PlacedRecord, PositionChange, WirePositions, place_product_record, transform_product_positions,
};

/// Translate every typed function, frame and runtime span together.
pub fn relocate_product(encoded: &[u8], file: &str, line: i64) -> Result<Vec<u8>, String> {
    let change = PositionChange::Relocate(String::new(), 0, file.to_owned(), line);
    transform_product_positions(encoded, &WirePositions::new(change), &mut Vec::new())
}

/// Replace each known source coordinate by its first Body IR node ordinal.
pub fn index_positions(encoded: &[u8], positions: &[(i64, i64)]) -> Result<Vec<u8>, String> {
    let change = PositionChange::Index(node_ordinals(positions));
    transform_product_positions(encoded, &WirePositions::new(change), &mut Vec::new())
}

/// Move source spans into the owner's node coordinate space in one typed traversal.
pub fn indexed_product(encoded: &[u8], positions: &[(i64, i64)], file: &str, line: i64) -> Result<Vec<u8>, String> {
    let change = PositionChange::IndexFrom(node_ordinals(positions), file.to_owned(), line);
    transform_product_positions(encoded, &WirePositions::new(change), &mut Vec::new())
}

/// Keep the first Body IR node at each coordinate, including coordinates shared by nested expressions.
fn node_ordinals(positions: &[(i64, i64)]) -> HashMap<String, i64> {
    let mut ordinals = HashMap::new();
    for (index, (line, column)) in positions.iter().enumerate() {
        ordinals.entry(format!("{line}:{column}")).or_insert(index as i64);
    }
    ordinals
}

/// Place the stable node ordinals of an admitted product at this revision's coordinates.
pub fn place_positions(encoded: &[u8], positions: &[(i64, i64)]) -> Result<Vec<u8>, String> {
    let change = PositionChange::Place(positions.to_vec());
    transform_product_positions(encoded, &WirePositions::new(change), &mut Vec::new())
}

/// Place the Body IR ordinals and relocate the declaration after admitting its stored schema once.
pub fn positioned_product(
    encoded: &[u8],
    positions: &[(i64, i64)],
    file: &str,
    line: i64,
    frame_names: &mut Vec<String>,
) -> Result<Vec<u8>, String> {
    let change = PositionChange::PlaceAt(positions.to_vec(), String::new(), 0, file.to_owned(), line);
    transform_product_positions(encoded, &WirePositions::new(change), frame_names)
}

/// Place a product as [`positioned_product`] does, keeping the typed record a restoring consumer reads, so the
/// placed product is neither encoded nor decoded again.
pub fn placed_record(encoded: &[u8], positions: &[(i64, i64)], file: &str, line: i64) -> Result<PlacedRecord, String> {
    let change = PositionChange::PlaceAt(positions.to_vec(), String::new(), 0, file.to_owned(), line);
    place_product_record(encoded, &WirePositions::new(change))
}
