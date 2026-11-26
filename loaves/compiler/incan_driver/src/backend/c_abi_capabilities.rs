//! The toolchain and SDK capability requirements of a declared interop target, checked against what a target-selected
//! verification selects.
//!
//! A package declares compatible capabilities (`toolchain = { capability = "apple-clang", version = ">=17" }`); it
//! never names an installation. The verification's selected compiler is the explicit `INCAN_C_ABI_CLANG`, or the
//! platform's Clang for the target's SDK. Its identity is read from that compiler and SDK themselves, never from a
//! search of the host: the compiler's own `--version`, the SDK the platform tool selected for the target, and an
//! Android NDK's own `source.properties` and `meta/platforms.json` beside the NDK Clang.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use semver::{Version, VersionReq};

use crate::unit::UnitCapabilityRequirement;

/// One selected capability: its name and version, read from the selected tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectedCapability {
    /// The capability's name, in the vocabulary packages declare (`apple-clang`, `clang`, `android-ndk`, `macosx`,
    /// `iphoneos`, `iphonesimulator`, `android`).
    pub capability: String,
    /// Its version, as the tool reports it.
    pub version: String,
}

impl SelectedCapability {
    fn shown(&self) -> String {
        format!("`{} {}`", self.capability, self.version)
    }
}

/// The selected compiler's capability, from its own `--version` and, for an NDK Clang, the NDK's own release file.
pub fn selected_toolchain(executable: &Path) -> Option<SelectedCapability> {
    if let Some(ndk) = ndk_root(executable)
        && let Some(revision) = property(&ndk.join("source.properties"), "Pkg.Revision")
    {
        return Some(SelectedCapability {
            capability: "android-ndk".to_string(),
            version: revision,
        });
    }
    let output = Command::new(executable).arg("--version").output().ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let first = text.lines().next()?;
    let (_, after) = first.split_once("clang version ")?;
    let version = after
        .split(|character: char| character.is_whitespace() || character == '-' || character == '(')
        .next()?
        .to_string();
    let capability = if first.starts_with("Apple clang") {
        "apple-clang"
    } else if first.starts_with("Android") {
        "android-ndk"
    } else {
        "clang"
    };
    Some(SelectedCapability {
        capability: capability.to_string(),
        version,
    })
}

/// The Android platform an NDK Clang compiles against: the newest API level its sysroot carries.
pub fn selected_android_platform(executable: &Path) -> Option<SelectedCapability> {
    let ndk = ndk_root(executable)?;
    let text = fs::read_to_string(ndk.join("meta/platforms.json")).ok()?;
    let platforms: serde_json::Value = serde_json::from_str(&text).ok()?;
    let max = platforms.get("max")?.as_u64()?;
    Some(SelectedCapability {
        capability: "android".to_string(),
        version: max.to_string(),
    })
}

/// The NDK an NDK Clang belongs to: `<ndk>/toolchains/llvm/prebuilt/<host>/bin/<clang>`.
fn ndk_root(executable: &Path) -> Option<PathBuf> {
    let resolved = executable.canonicalize().ok()?;
    let bin = resolved.parent()?;
    let host = bin.parent()?;
    let prebuilt = host.parent()?;
    let llvm = prebuilt.parent()?;
    let toolchains = llvm.parent()?;
    let root = toolchains.parent()?;
    let shaped = bin.file_name()? == "bin"
        && prebuilt.file_name()? == "prebuilt"
        && llvm.file_name()? == "llvm"
        && toolchains.file_name()? == "toolchains";
    (shaped && root.join("source.properties").is_file()).then(|| root.to_path_buf())
}

/// One `key = value` line of a Java-properties file.
fn property(path: &Path, key: &str) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    text.lines().find_map(|line| {
        let (name, value) = line.split_once('=')?;
        (name.trim() == key).then(|| value.trim().to_string())
    })
}

/// A reported version as a semantic version: `27.2` reads as `27.2.0`; anything after the third number is dropped.
fn semantic(version: &str) -> Option<Version> {
    let mut parts = version
        .split('.')
        .map(|part| part.parse::<u64>().ok())
        .collect::<Option<Vec<_>>>()?;
    if parts.is_empty() {
        return None;
    }
    parts.resize(3.max(parts.len()), 0);
    Some(Version::new(parts[0], parts[1], parts[2]))
}

/// Require one declared capability of the selected one, refusing a missing or incompatible capability by name. `kind`
/// is `toolchain` or `SDK`.
pub fn require(
    target: &str,
    kind: &str,
    requirement: &UnitCapabilityRequirement,
    selected: Option<&SelectedCapability>,
    missing: &str,
) -> Result<(), String> {
    let wanted = if requirement.version.is_empty() {
        format!("`{}`", requirement.capability)
    } else {
        format!("`{}` (`{}`)", requirement.capability, requirement.version)
    };
    let prefix = format!("interop target `{target}` requires {kind} capability {wanted}");
    let selected_kind = if kind == "toolchain" { "C toolchain" } else { kind };
    let Some(selected) = selected else {
        return Err(format!("{prefix}; {missing}"));
    };
    if selected.capability != requirement.capability {
        return Err(format!(
            "{prefix}; the selected {selected_kind} is {}",
            selected.shown()
        ));
    }
    if requirement.version.is_empty() {
        return Ok(());
    }
    let compatible = VersionReq::parse(&requirement.version)
        .map_err(|error| format!("{prefix}; the requirement is not a version requirement: {error}"))?;
    match semantic(&selected.version) {
        Some(version) if compatible.matches(&version) => Ok(()),
        Some(_) => Err(format!(
            "{prefix}; the selected {selected_kind} is {}",
            selected.shown()
        )),
        None => Err(format!(
            "{prefix}; the selected {selected_kind} reports version `{}`, which is not a version",
            selected.version
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn requirement(capability: &str, version: &str) -> UnitCapabilityRequirement {
        UnitCapabilityRequirement {
            capability: capability.to_string(),
            version: version.to_string(),
        }
    }

    fn selected(capability: &str, version: &str) -> SelectedCapability {
        SelectedCapability {
            capability: capability.to_string(),
            version: version.to_string(),
        }
    }

    #[test]
    fn a_compatible_capability_is_accepted_and_others_are_refused_by_name() {
        let apple = selected("apple-clang", "21.0.0");
        assert!(require("t", "toolchain", &requirement("apple-clang", ">=17"), Some(&apple), "").is_ok());
        assert!(require("t", "toolchain", &requirement("apple-clang", ""), Some(&apple), "").is_ok());
        let older = require(
            "t",
            "toolchain",
            &requirement("apple-clang", ">=17, <18"),
            Some(&apple),
            "",
        );
        assert!(older.is_err_and(|message| message.contains("`apple-clang` (`>=17, <18`)")));
        let other = require("t", "toolchain", &requirement("clang", ">=17"), Some(&apple), "");
        assert!(other.is_err_and(|message| message.contains("the selected C toolchain is `apple-clang 21.0.0`")));
        let none = require("t", "SDK", &requirement("iphoneos", ""), None, "none is selected");
        assert!(none.is_err_and(|message| message.ends_with("none is selected")));
        let sdk = selected("macosx", "27.2");
        assert!(require("t", "SDK", &requirement("macosx", ">=26"), Some(&sdk), "").is_ok());
    }
}
