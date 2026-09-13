//! Frozen fork of `cargo-platform` 0.3.2, vendored from crates.io under MIT OR Apache-2.0.
//!
//! # Why this is forked rather than depended on
//!
//! Oven is removing Cargo from its publisher, not just from its consumer path. A locked Rust dependency graph still
//! has to be projected onto one target, which means deciding which `[target.'cfg(...)'.dependencies]` edges apply --
//! and that decision is a grammar, not a heuristic. `cfg(any(unix, target_os = "hermit"))` means exactly what Cargo
//! says it means, and a near-miss is a wrong answer in both directions: an edge kept that Cargo drops leaves an
//! unbuildable unit in the plan, one dropped that Cargo keeps produces a link failure far from its cause.
//!
//! Depending on the crate would have left that grammar resolved from crates.io on every build, which is the coupling
//! being removed. Reimplementing it would have meant maintaining a second, subtly different parser. Freezing the
//! source is the third option and the right one: the same reasoning that pins this toolchain to Rust 1.98 applies to
//! the grammar it evaluates.
//!
//! # What was taken
//!
//! `src/lib.rs`, `src/cfg.rs` and `src/error.rs` verbatim -- 678 lines, no dependencies of its own, no `unsafe`, no
//! filesystem, network or process access. The upstream tests and example are not carried; this crate's own tests
//! cover the surface Oven uses.
//!
//! # Updating it
//!
//! This is frozen deliberately. Re-syncing means diffing against a newer `cargo-platform` release and taking the
//! change knowingly, the same way a toolchain bump is taken -- never an automatic version range.

//! Platform definition used by Cargo.
//!
//! This defines a [`Platform`] type which is used in Cargo to specify a target platform.
//! There are two kinds, a named target like `x86_64-apple-darwin`, and a "cfg expression"
//! like `cfg(any(target_os = "macos", target_os = "ios"))`.
//!
//! See `examples/matches.rs` for an example of how to match against a `Platform`.
//!
//! > This crate is maintained by the Cargo team for use by the wider
//! > ecosystem. This crate follows semver compatibility for its APIs.
//!
//! [`Platform`]: enum.Platform.html

#![allow(clippy::explicit_auto_deref, clippy::while_let_on_iterator)]
// Carried verbatim from upstream, which predates this repository's stricter lint set. The two lints above are style
// preferences, not defects, and editing vendored source to satisfy them would make the fork harder to diff against a
// later `cargo-platform` release -- which is the one maintenance operation this crate exists to keep cheap. Any
// genuine change here should be taken from upstream, not written locally.

use std::str::FromStr;
use std::{fmt, path::Path};

mod cfg;
mod error;

use cfg::KEYWORDS;
pub use cfg::{Cfg, CfgExpr, Ident};
pub use error::{ParseError, ParseErrorKind};

/// Platform definition.
#[derive(Eq, PartialEq, Hash, Ord, PartialOrd, Clone, Debug)]
pub enum Platform {
    /// A named platform, like `x86_64-apple-darwin`.
    Name(String),
    /// A cfg expression, like `cfg(windows)`.
    Cfg(CfgExpr),
}

impl Platform {
    /// Returns whether the Platform matches the given target and cfg.
    ///
    /// The named target and cfg values should be obtained from `rustc`.
    pub fn matches(&self, name: &str, cfg: &[Cfg]) -> bool {
        match *self {
            Platform::Name(ref p) => p == name,
            Platform::Cfg(ref p) => p.matches(cfg),
        }
    }

    fn validate_named_platform(name: &str) -> Result<(), ParseError> {
        if let Some(ch) = name
            .chars()
            .find(|&c| !(c.is_alphanumeric() || c == '_' || c == '-' || c == '.'))
        {
            if name.chars().any(|c| c == '(') {
                return Err(ParseError::new(
                    name,
                    ParseErrorKind::InvalidTarget(
                        "unexpected `(` character, cfg expressions must start with `cfg(`".to_string(),
                    ),
                ));
            }
            return Err(ParseError::new(
                name,
                ParseErrorKind::InvalidTarget(format!("unexpected character {} in target name", ch)),
            ));
        }
        Ok(())
    }

    pub fn check_cfg_attributes(&self, warnings: &mut Vec<String>) {
        fn check_cfg_expr(expr: &CfgExpr, warnings: &mut Vec<String>) {
            match *expr {
                CfgExpr::Not(ref e) => check_cfg_expr(e, warnings),
                CfgExpr::All(ref e) | CfgExpr::Any(ref e) => {
                    for e in e {
                        check_cfg_expr(e, warnings);
                    }
                }
                CfgExpr::Value(ref e) => match e {
                    Cfg::Name(name) => match name.as_str() {
                        "test" | "debug_assertions" | "proc_macro" =>
                            warnings.push(format!(
                                "Found `{}` in `target.'cfg(...)'.dependencies`. \
                                 This value is not supported for selecting dependencies \
                                 and will not work as expected. \
                                 To learn more visit \
                                 https://doc.rust-lang.org/cargo/reference/specifying-dependencies.html#platform-specific-dependencies",
                                 name
                            )),
                        _ => (),
                    },
                    Cfg::KeyPair(name, _) => if name.as_str() == "feature" {
                        warnings.push(String::from(
                            "Found `feature = ...` in `target.'cfg(...)'.dependencies`. \
                             This key is not supported for selecting dependencies \
                             and will not work as expected. \
                             Use the [features] section instead: \
                             https://doc.rust-lang.org/cargo/reference/features.html"
                        ))
                    },
                }
                CfgExpr::True | CfgExpr::False => {},
            }
        }

        if let Platform::Cfg(cfg) = self {
            check_cfg_expr(cfg, warnings);
        }
    }

    pub fn check_cfg_keywords(&self, warnings: &mut Vec<String>, path: &Path) {
        fn check_cfg_expr(expr: &CfgExpr, warnings: &mut Vec<String>, path: &Path) {
            match *expr {
                CfgExpr::Not(ref e) => check_cfg_expr(e, warnings, path),
                CfgExpr::All(ref e) | CfgExpr::Any(ref e) => {
                    for e in e {
                        check_cfg_expr(e, warnings, path);
                    }
                }
                CfgExpr::True | CfgExpr::False => {}
                CfgExpr::Value(ref e) => match e {
                    Cfg::Name(name) | Cfg::KeyPair(name, _) => {
                        if !name.raw && KEYWORDS.contains(&name.as_str()) {
                            warnings.push(format!(
                                "[{}] future-incompatibility: `cfg({e})` is deprecated as `{name}` is a keyword \
                                 and not an identifier and should not have have been accepted in this position.\n \
                                 | this was previously accepted by Cargo but is being phased out; it will become a hard error in a future release!\n \
                                 |\n \
                                 | help: use raw-idents instead: `cfg(r#{name})`",
                                 path.display()
                            ));
                        }
                    }
                },
            }
        }

        if let Platform::Cfg(cfg) = self {
            check_cfg_expr(cfg, warnings, path);
        }
    }
}

// Upstream carries `serde` impls for `Platform` behind its `serde_core` dependency. They are dropped in this fork:
// Oven parses platform selectors out of manifest TOML and never serialises one, so carrying them would reintroduce
// the only external dependency this crate otherwise has. Restore them from upstream if a caller ever needs them.

impl FromStr for Platform {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Platform, ParseError> {
        if let Some(s) = s.strip_prefix("cfg(").and_then(|s| s.strip_suffix(')')) {
            s.parse().map(Platform::Cfg)
        } else {
            Platform::validate_named_platform(s)?;
            Ok(Platform::Name(s.to_string()))
        }
    }
}

impl fmt::Display for Platform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Platform::Name(ref n) => n.fmt(f),
            Platform::Cfg(ref e) => write!(f, "cfg({})", e),
        }
    }
}
