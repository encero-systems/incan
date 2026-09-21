//! A callback host that lends one mutable `Ui` to the closure it is given, twice, then reports the count, after the
//! `egui` shape the retired ownership test was written for.

/// A counter the callback may change.
pub struct Ui {
    /// The count, `1` when the host starts.
    pub n: i64,
}

/// Run `paint` twice over one `Ui` starting at `1`, then return its count.
pub fn with_ui<F: FnMut(&mut Ui)>(mut paint: F) -> i64 {
    let mut ui = Ui { n: 1 };
    paint(&mut ui);
    paint(&mut ui);
    ui.n
}
