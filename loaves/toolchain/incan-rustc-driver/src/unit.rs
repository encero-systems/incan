//! Declared unit permissions are session options, never environment variables.

use crate::adapter::DriverError;
use crate::identity;
use rustc_feature::UnstableFeatures;
use rustc_interface::interface;
use std::path::Path;

/// The already-built driver may compile a Rust unit with an explicit, identity-bearing permission selection.
struct UnitCallbacks {
    rustc_private: bool,
}

impl rustc_driver::Callbacks for UnitCallbacks {
    /// Grant only the caller's declared unit permission through rustc's tracked session option.
    fn config(&mut self, config: &mut interface::Config) {
        if self.rustc_private {
            config.opts.unstable_features = UnstableFeatures::Allow;
            config.opts.unstable_opts.allow_features = Some(vec!["rustc_private".into()]);
        }
    }
}

/// Compile one Oven-selected Rust unit under the exact build-bound toolchain.
///
/// Oven builds the initial driver with a declared, receipt-bound crate-scoped bootstrap grant. Later units use this session callback; their capability list must enter the unit's RFC 124 build identity before launch.
pub fn compile(args: &[String], sysroot: &Path, features: &[String]) -> Result<(), DriverError> {
    identity::verify(sysroot)?;
    if features.len() > 1 || features.iter().any(|feature| feature != "rustc_private") {
        return Err(DriverError::Invocation(
            "only the distinct rustc_private unit permission is admitted".into(),
        ));
    }
    let mut arguments = vec![
        "incan-rustc-driver".into(),
        "--sysroot".into(),
        sysroot.to_string_lossy().into_owned(),
    ];
    if args
        .iter()
        .any(|arg| arg == "--sysroot" || arg.starts_with("--sysroot="))
    {
        return Err(DriverError::Invocation(
            "unit arguments cannot override the selected sysroot".into(),
        ));
    }
    arguments.extend_from_slice(args);
    rustc_driver::catch_fatal_errors(|| {
        rustc_driver::run_compiler(
            &arguments,
            &mut UnitCallbacks {
                rustc_private: !features.is_empty(),
            },
        )
    })
    .map_err(|_| DriverError::Compilation)
}
