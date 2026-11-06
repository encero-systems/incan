//! Rustc-observed native test inputs retained in the existing output record.
//!
//! The host only captures compiler observations and guards physical reads. It does not discover Rust syntax,
//! resolve dependencies or introduce another cache. This closes the native-test host gap tracked in #2207;
//! shared compilation policy remains owned by `workspaces/oven/src/native_compilation.incn`.

use super::{
    Command, OvenRustcError, Path, PathBuf, digest_bytes, digest_regular_file, fs, parse_rustc_diagnostics,
    resolve_compile_environment_value,
};
use crate::rustc::direct_compiler::{OvenRustcDepInfoObservation, parse_rustc_dep_info};
use oven_model::observed_inputs::InputObservations;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Exact consumed bytes and environment digests; absent evidence cannot authorize native-test reuse.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct NativeTestInputs {
    files: BTreeMap<PathBuf, String>,
    environment: BTreeMap<String, Option<String>>,
}

impl NativeTestInputs {
    /// Reconcile only previously consumed inputs through the existing metadata-sensitive file digester.
    pub(super) fn current(&self, source: &Path, declared: &BTreeMap<String, String>, isolated: bool) -> bool {
        if self.files.is_empty() {
            return false;
        }
        // ---- Consumed file contents ----
        let mut observations = InputObservations::default();
        let files_current = self.files.iter().all(|(path, digest)| {
            observations.observe(path);
            digest_regular_file(path, "native test input").is_ok_and(|current| current == *digest)
        });

        // ---- Consumed compile-time environment ----
        let environment_current = self.environment.iter().all(|(name, expected)| {
            let value = if let Some(value) = declared.get(name) {
                match resolve_compile_environment_value(name, value, source) {
                    Ok(value) => Some(value.into_os_string()),
                    Err(_) => return false,
                }
            } else if !isolated
                && !super::super::environment::direct_rustc_excludes_inherited_environment(name.as_ref())
            {
                std::env::var_os(name)
            } else if isolated && name == "PATH" {
                Some("/usr/bin:/bin".into())
            } else {
                None
            };
            value.map(|value| digest_bytes(value.as_encoded_bytes())) == *expected
        });
        files_current && environment_current && observations.supports_retention() && observations.current()
    }
}

/// Original pre-compilation observations, retained until actual Rustc consumption is checked.
pub(super) struct PendingNativeTestInputs {
    inputs: NativeTestInputs,
    observation: OvenRustcDepInfoObservation,
    physical: InputObservations,
    environment: BTreeMap<String, PathBuf>,
}

impl PendingNativeTestInputs {
    /// Discover with Rustc before observing bytes, then freeze the same environment for the actual compiler.
    ///
    /// The discovery pass emits no code. Unknown files cannot be certified by hashing them after compilation:
    /// the actual pass must report precisely this set while these original physical observations remain current.
    pub(super) fn discover(
        command: &mut Command,
        source: &Path,
        source_digest: &str,
        isolated: bool,
    ) -> Result<Self, OvenRustcError> {
        // ---- Dependency discovery with the exact compiler configuration ----
        let environment = freeze_environment(command, isolated)?;
        let temporary = tempfile::tempdir().map_err(|source_error| OvenRustcError::Io {
            path: std::env::temp_dir(),
            source: source_error,
        })?;
        let dep_info = temporary.path().join("inputs.d");
        let mut discovery = Command::new(command.get_program());
        discovery.args(command.get_args()).env_clear().envs(&environment);
        if let Some(cwd) = command.get_current_dir() {
            discovery.current_dir(cwd);
        }
        discovery.arg(super::super::direct_compiler::joined_os_argument(
            "--emit=dep-info=",
            &dep_info,
            None,
        ));
        let result = {
            let _trace = tracing::trace_span!(target: "incan::invocation", "oven_rustc::discover_native_test_inputs", input = %source.display()).entered();
            discovery.output().map_err(|source_error| OvenRustcError::Io {
                path: PathBuf::from(discovery.get_program()),
                source: source_error,
            })?
        };
        if !result.status.success() {
            return Err(OvenRustcError::CompilationFailed {
                report: parse_rustc_diagnostics(&result.stdout, &result.stderr).with_invocation(&discovery),
            });
        }
        let bytes = fs::read(&dep_info).map_err(|source| OvenRustcError::Io { path: dep_info, source })?;
        let observation = parse_rustc_dep_info(&bytes, &environment)?;

        // ---- Observe consumed inputs before the actual compilation ----
        let mut physical = InputObservations::default();
        let mut files = BTreeMap::new();
        for path in &observation.files {
            if !path.is_absolute() {
                return Err(input_error("Rustc reported a non-absolute consumed input"));
            }
            physical.observe(path);
            files.insert(path.clone(), digest_regular_file(path, "native test input")?);
        }

        // ---- Bind the observations to the admitted root and upcoming compiler pass ----
        if files.get(source).map(String::as_str) != Some(source_digest) {
            return Err(input_error("root source changed after receipt admission"));
        }
        if !physical.supports_retention() || !physical.current() {
            return Err(input_error(
                "consumed input changed or cannot be safely observed before compilation",
            ));
        }
        let inputs = NativeTestInputs {
            files,
            environment: observation
                .environment
                .iter()
                .map(|value| (value.name.clone(), value.value_digest.clone()))
                .collect(),
        };
        command.arg("--emit=dep-info=-,link");
        Ok(Self {
            inputs,
            observation,
            physical,
            environment,
        })
    }

    /// Refuse publication if compiler consumption or any observed input changed during compilation.
    pub(super) fn finish(self, dep_info: &[u8]) -> Result<NativeTestInputs, OvenRustcError> {
        let actual = parse_rustc_dep_info(dep_info, &self.environment)?;
        if actual != self.observation || !self.physical.current() {
            return Err(input_error(
                "consumed inputs changed during compilation; retry the command",
            ));
        }
        Ok(self.inputs)
    }
}

/// Freeze the command's actual inherited/overridden environment for both discovery and compilation.
fn freeze_environment(command: &mut Command, isolated: bool) -> Result<BTreeMap<String, PathBuf>, OvenRustcError> {
    let mut environment = BTreeMap::new();
    if !isolated {
        for (name, value) in std::env::vars_os() {
            let name = name
                .into_string()
                .map_err(|_| input_error("non-UTF-8 environment name"))?;
            environment.insert(name, PathBuf::from(value));
        }
    }
    for (name, value) in command.get_envs() {
        let name = name
            .to_str()
            .ok_or_else(|| input_error("non-UTF-8 environment override"))?;
        if let Some(value) = value {
            environment.insert(name.to_string(), PathBuf::from(value));
        } else {
            environment.remove(name);
        }
    }
    command.env_clear().envs(&environment);
    Ok(environment)
}

/// Keep unsupported observations and compilation races in the existing typed input-error surface.
fn input_error(message: &str) -> OvenRustcError {
    OvenRustcError::InvalidInput {
        field: "native test consumed inputs",
        message: message.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::{Command, NativeTestInputs, PendingNativeTestInputs, digest_regular_file, fs};
    use std::collections::BTreeMap;

    /// Exercise actual Rustc dep-info before publication, preserving the original observations across a race.
    #[test]
    fn compiled_native_test_refuses_post_compile_input_edit() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let root = project.path().canonicalize()?;
        let source = root.join("test.rs");
        let data = root.join("text");
        fs::write(
            &source,
            "#[test] fn selected() { assert_eq!(include_str!(\"text\"), \"42\"); }",
        )?;
        fs::write(&data, "42")?;
        let rustc = crate::rustc::resolve_active_rustc()?;
        let mut command = Command::new(&rustc);
        command.arg("--test").arg(&source).arg("-o").arg(root.join("test"));
        let pending = PendingNativeTestInputs::discover(
            &mut command,
            &source,
            &digest_regular_file(&source, "test source")?,
            false,
        )?;
        let output = command.output()?;
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        fs::write(&data, "43")?;
        assert!(
            pending.finish(&output.stdout).is_err(),
            "changed data must not receive an output record"
        );
        Ok(())
    }

    /// Environment observations retain only digests and compare the actual declared override rather than ambient state.
    #[test]
    fn consumed_environment_and_missing_evidence_refuse_reuse() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let root = project.path().canonicalize()?;
        let source = root.join("test.rs");
        fs::write(
            &source,
            "#[test] fn selected() { assert_eq!(env!(\"OVEN_CONSUMED_INPUT_CONTROL\"), \"42\"); }",
        )?;
        let rustc = crate::rustc::resolve_active_rustc()?;
        let mut command = Command::new(&rustc);
        command.arg("--test").arg(&source).arg("-o").arg(root.join("test"));
        command.env("OVEN_CONSUMED_INPUT_CONTROL", "42");
        let pending = PendingNativeTestInputs::discover(
            &mut command,
            &source,
            &digest_regular_file(&source, "test source")?,
            false,
        )?;
        let output = command.output()?;
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        let inputs = pending.finish(&output.stdout)?;
        let mut declared = BTreeMap::from([("OVEN_CONSUMED_INPUT_CONTROL".into(), "42".into())]);
        assert!(inputs.current(&source, &declared, false));
        declared.insert("OVEN_CONSUMED_INPUT_CONTROL".into(), "43".into());
        assert!(!inputs.current(&source, &declared, false));
        assert!(
            !NativeTestInputs {
                files: BTreeMap::new(),
                environment: BTreeMap::new()
            }
            .current(&source, &declared, false)
        );
        Ok(())
    }
}
