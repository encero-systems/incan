//! The Incan home: the one directory under which every Incan-owned store, cache, reading and toolchain lives.
//!
//! `INCAN_HOME` names it when set and non-empty; otherwise it is `.incan` under the user's home (`HOME`, or
//! `USERPROFILE` on Windows). Every Incan-owned path derives from [`incan_home`]; no other code reads these variables.

use std::env;
use std::ffi::OsString;
use std::path::PathBuf;

/// The environment variable that names the Incan home.
pub const INCAN_HOME_ENV: &str = "INCAN_HOME";

/// The Incan home of this process, or `None` when neither `INCAN_HOME` nor a user home is set.
pub fn incan_home() -> Option<PathBuf> {
    incan_home_from(env::var_os(INCAN_HOME_ENV), user_home())
}

/// The Incan home from explicit inputs: `incan_home` when non-empty, otherwise `.incan` under `user_home`.
pub fn incan_home_from(incan_home: Option<OsString>, user_home: Option<OsString>) -> Option<PathBuf> {
    incan_home
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            user_home
                .filter(|path| !path.is_empty())
                .map(|path| PathBuf::from(path).join(".incan"))
        })
}

/// The user's home directory: `HOME`, or `USERPROFILE` on Windows.
pub fn user_home() -> Option<OsString> {
    env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .filter(|path| !path.is_empty())
}

/// One named cache directory under the Incan home: `<home>/cache/<name>`.
pub fn cache_dir(name: &str) -> Option<PathBuf> {
    Some(incan_home()?.join("cache").join(name))
}
