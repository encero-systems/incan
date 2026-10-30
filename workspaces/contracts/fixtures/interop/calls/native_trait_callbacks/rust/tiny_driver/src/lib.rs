//! A host that lends its configuration to callbacks behind a borrowed `Send` trait object, after the shape of
//! rustc's driver callbacks.

/// Settings a callback may change before the host reports them.
pub struct Config {
    /// Flags the host prints after its callbacks ran.
    pub crate_cfg: Vec<String>,
}

/// What the host calls. Every method has a default, as rustc's callbacks do.
pub trait Callbacks {
    /// Change the configuration before the run.
    fn config(&mut self, _config: &mut Config) {}
}

/// Something that describes itself, with no default.
pub trait Reporter {
    /// The description.
    fn report(&self) -> String;
}

/// The host's own callbacks, keeping every default.
#[derive(Default)]
pub struct Quiet;

impl Callbacks for Quiet {}

/// Run the host once: lend a fresh configuration to `callbacks`, then print the arguments and every flag.
pub fn run_compiler(args: &[String], callbacks: &mut (dyn Callbacks + Send)) {
    let mut config = Config { crate_cfg: Vec::new() };
    callbacks.config(&mut config);
    println!("{} [{}]", args.join(" "), config.crate_cfg.join(","));
}

/// Ask a reporter another thread could share for its description.
pub fn announce(reporter: &(dyn Reporter + Sync)) -> String {
    reporter.report()
}
