//! The three variant shapes Body IR's `StatementKind` uses: struct, tuple and unit.
pub enum Stmt {
    Assign { place: i64, value: i64 },
    Return(i64),
    Continue,
}

/// A sample statement of each shape, by index.
pub fn sample(index: i64) -> Stmt {
    match index {
        0 => Stmt::Assign { place: 3, value: 4 },
        1 => Stmt::Return(7),
        _ => Stmt::Continue,
    }
}
