//! A host that lends its configuration to callbacks behind a borrowed `Send` trait object, after the shape of
//! rustc's driver callbacks.

use std::cell::RefCell;
use std::rc::Rc;

/// Settings a callback may change before the host reports them.
pub struct Config {
    /// Flags the host prints after its callbacks ran.
    pub crate_cfg: Vec<String>,
    /// Names borrowed for the whole program, a field whose carrier a caller cannot hold.
    pub extra_symbols: Vec<&'static str>,
    /// The host's own state, private all the way down.
    pub registry: Registry,
}

/// State only the host reads.
pub struct Registry {
    entries: Rc<RefCell<Vec<Box<dyn Fn() -> usize>>>>,
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
    let mut config = Config {
        crate_cfg: Vec::new(),
        extra_symbols: vec!["host"],
        registry: Registry {
            entries: Rc::new(RefCell::new(vec![Box::new(|| 1)])),
        },
    };
    callbacks.config(&mut config);
    let registered = config.registry.entries.borrow().iter().map(|entry| entry()).sum::<usize>();
    println!(
        "{} [{}] {} {registered}",
        args.join(" "),
        config.crate_cfg.join(","),
        config.extra_symbols.len()
    );
}

/// Ask a reporter another thread could share for its description.
pub fn announce(reporter: &(dyn Reporter + Sync)) -> String {
    reporter.report()
}
