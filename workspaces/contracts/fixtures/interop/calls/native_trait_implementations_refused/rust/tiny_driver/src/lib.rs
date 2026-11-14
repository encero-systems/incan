//! Two traits a class may implement: one whose method has a default and takes a mutable receiver, one whose method
//! has no default.

/// Settings a callback may change.
pub struct Config {
    /// Flags a callback adds.
    pub crate_cfg: Vec<String>,
}

/// What a host calls, with a default.
pub trait Callbacks {
    /// Change the configuration.
    fn config(&mut self, _config: &mut Config) {}
}

/// Something that describes itself, with no default.
pub trait Reporter {
    /// The description.
    fn report(&self) -> String;
}
