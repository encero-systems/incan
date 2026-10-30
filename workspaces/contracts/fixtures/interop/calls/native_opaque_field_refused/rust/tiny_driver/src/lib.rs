//! A configuration with a field whose borrowed carrier a caller cannot hold, lent to a callback.

/// Settings a callback may change.
pub struct Config {
    /// Flags a callback adds.
    pub crate_cfg: Vec<String>,
    /// Names borrowed for the whole program.
    pub extra_symbols: Vec<&'static str>,
}

/// What the host calls. Every method has a default.
pub trait Callbacks {
    /// Change the configuration.
    fn config(&mut self, _config: &mut Config) {}
}

/// Run the host once over `callbacks`.
pub fn run_compiler(args: &[String], callbacks: &mut (dyn Callbacks + Send)) {
    let mut config = Config {
        crate_cfg: Vec::new(),
        extra_symbols: vec!["host"],
    };
    callbacks.config(&mut config);
    println!("{} [{}]", args.join(" "), config.crate_cfg.join(","));
}
