//! Source-coordinate edits applied only at a stored plan record's declared span fields.
//!
//! A keyed product's spans are owner-relative, and each span that locates a Body IR node is stored as that node's
//! ordinal under the file `<owner>`. Reading a product through a [`WirePositions`] rewrites each span as it is read:
//! relocating a declaration to another file and line, indexing source coordinates by node ordinal, or placing node
//! ordinals at this revision's coordinates.

use std::collections::HashMap;

/// One coordinate mapping applied to every span a product read meets.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum PositionChange {
    /// Keep every span as stored.
    #[default]
    Preserve,
    /// Move spans of `old_file` from `origin` to `new_file` starting at `start`: `(old_file, origin, new_file, start)`.
    Relocate(String, i64, String, i64),
    /// Replace each owner-relative `line:column` key by its node ordinal.
    Index(HashMap<String, i64>),
    /// Relocate spans of a source file to `<owner>` from `origin`, then index them: `(ordinals, source, origin)`.
    IndexFrom(HashMap<String, i64>, String, i64),
    /// Place each node ordinal at this revision's owner-relative `(line, column)`.
    Place(Vec<(i64, i64)>),
    /// Place node ordinals, then relocate: `(positions, old_file, origin, new_file, start)`.
    PlaceAt(Vec<(i64, i64)>, String, i64, String, i64),
}

/// Apply source-coordinate edits only at declared native span fields.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WirePositions {
    pub change: PositionChange,
}

impl WirePositions {
    /// Read spans exactly as stored.
    pub fn preserve() -> Self {
        Self::default()
    }

    /// Read spans through one coordinate change.
    pub fn new(change: PositionChange) -> Self {
        Self { change }
    }

    /// The span a stored `(file, line, column)` reads as under this mapping.
    pub fn apply(&self, file: String, line: i64, column: i64) -> Result<(String, i64, i64), String> {
        match &self.change {
            PositionChange::Preserve => Ok((file, line, column)),
            PositionChange::Relocate(old_file, origin, new_file, start) => {
                relocate_span(file, line, column, old_file, *origin, new_file, *start)
            }
            PositionChange::Index(ordinals) => index_span(file, line, column, ordinals),
            PositionChange::IndexFrom(ordinals, source, origin) => {
                let (relative_file, relative_line, relative_column) =
                    relocate_span(file, line, column, source, *origin, "<owner>", 1)?;
                index_span(relative_file, relative_line, relative_column, ordinals)
            }
            PositionChange::Place(positions) => place_span(file, line, column, positions),
            PositionChange::PlaceAt(positions, old_file, origin, new_file, start) => {
                let (placed_file, placed_line, placed_column) = place_span(file, line, column, positions)?;
                relocate_span(
                    placed_file,
                    placed_line,
                    placed_column,
                    old_file,
                    *origin,
                    new_file,
                    *start,
                )
            }
        }
    }
}

/// Replace an owner-relative coordinate by its node ordinal; an unknown coordinate after the first line refuses.
fn index_span(
    file: String,
    line: i64,
    column: i64,
    ordinals: &HashMap<String, i64>,
) -> Result<(String, i64, i64), String> {
    if file == "<owner>" {
        if let Some(ordinal) = ordinals.get(&format!("{}:{column}", line - 1)) {
            return Ok((file, 0, ordinal + 1));
        }
        if line > 1 {
            return Err("plan-function span locates no Body IR node".to_owned());
        }
    }
    Ok((file, line, column))
}

/// Place a stored node ordinal at its node's current owner-relative coordinate.
fn place_span(file: String, line: i64, column: i64, positions: &[(i64, i64)]) -> Result<(String, i64, i64), String> {
    if file == "<owner>" && line == 0 {
        let node = usize::try_from(column - 1).ok().and_then(|index| positions.get(index));
        let Some((node_line, node_column)) = node else {
            return Err("plan-function product names a body position the current body does not have".to_owned());
        };
        return Ok((file, node_line + 1, *node_column));
    }
    Ok((file, line, column))
}

/// Move a span of `old_file` from `origin` to `new_file` at `start`, refusing another file or a line before it.
fn relocate_span(
    file: String,
    line: i64,
    column: i64,
    old_file: &str,
    origin: i64,
    new_file: &str,
    start: i64,
) -> Result<(String, i64, i64), String> {
    if file != old_file {
        return Err("plan-function position map refuses a mixed-source frame".to_owned());
    }
    let moved = line - origin + start;
    if moved < 1 {
        return Err("plan-function position map precedes its owner".to_owned());
    }
    Ok((new_file.to_owned(), moved, column))
}
