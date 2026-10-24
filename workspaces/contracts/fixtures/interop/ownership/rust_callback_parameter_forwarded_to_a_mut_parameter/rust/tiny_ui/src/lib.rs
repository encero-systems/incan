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

/// Retain an owned callback after its Incan creating frame returns.
pub struct StoredCallback {
    callback: Box<dyn Fn() -> String + Send + Sync + 'static>,
}

impl StoredCallback {
    /// Box the checked environment with the same storage bounds the host retains.
    pub fn new<F: Fn() -> String + Send + Sync + 'static>(callback: F) -> Self {
        Self {
            callback: Box::new(callback),
        }
    }

    /// Invoke the retained callback without consuming its owned captures.
    pub fn call(&self) -> String {
        (self.callback)()
    }
}
