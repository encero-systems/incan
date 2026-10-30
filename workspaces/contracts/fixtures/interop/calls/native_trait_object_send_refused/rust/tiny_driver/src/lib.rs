//! A host that borrows its callbacks mutably, behind a `Send` trait object.

/// What the host calls. Every method has a default.
pub trait Callbacks {
    /// Run before the host reports.
    fn config(&mut self) {}
}

/// Run the host once over `callbacks`.
pub fn run_compiler(args: &[String], callbacks: &mut (dyn Callbacks + Send)) {
    callbacks.config();
    println!("{}", args.join(" "));
}
