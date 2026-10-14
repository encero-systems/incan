//! Explicit native plans use the existing owner for preparation and the CLI for program execution.

use incan_driver::error::{CliError, CliResult};
use oven_rustc::rustc::{OvenStoredDirectRustcRunRequest, bake_stored_direct_rustc_run};
use oven_store::OvenReceipt;
use oven_store::store::{OvenStore, OvenStoreLease, OvenStoreLimits};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Transport coordinates are untrusted inputs to the canonical receipt-bound executor, never reusable authority.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct StoredNativeBuildRequest {
    store: PathBuf,
    limits: OvenStoreLimits,
    plan_identity: String,
    receipt: OvenReceipt,
    rustc: PathBuf,
    source: PathBuf,
    output: PathBuf,
    crate_name: String,
    edition: String,
    source_evidence_key: String,
}

/// Presentation returned to the client; this report grants no Store access and owns no execution lease.
#[derive(Serialize, Deserialize)]
pub(crate) struct StoredNativeBuildReport {
    /// Prepared local executable; the client retains execution and presentation.
    pub output: PathBuf,
    /// Canonical executor evidence, never inferred from the transport.
    pub cargo_process_started: bool,
    /// Whether the existing output receipt admitted this executable unchanged.
    pub reused: bool,
}

impl StoredNativeBuildRequest {
    /// The owner shares the client's working directory but accepts only explicit native path coordinates.
    pub(super) fn validate_paths(&self) -> CliResult<()> {
        if [&self.store, &self.rustc, &self.source, &self.output]
            .iter()
            .any(|path| !path.is_absolute())
        {
            return Err(CliError::failure("owner requires absolute stored native build paths"));
        }
        Ok(())
    }

    /// Reconcile receipt, source, compiler, selected artifacts and output through the existing executor.
    /// Its process-local guarded observations survive between requests in the existing owner. The temporary bake
    /// lease can end after preparation because the submitting client already holds its own original Store lease.
    pub(super) fn prepare(&self) -> CliResult<StoredNativeBuildReport> {
        self.validate_paths()?;
        let store = OvenStore::new(&self.store, self.limits);
        let bake = bake_stored_direct_rustc_run(&OvenStoredDirectRustcRunRequest {
            store: &store,
            plan_identity: self.plan_identity.clone(),
            receipt: self.receipt.clone(),
            rustc: self.rustc.clone(),
            source: self.source.clone(),
            output: self.output.clone(),
            crate_name: self.crate_name.clone(),
            edition: self.edition.clone(),
            source_evidence_key: self.source_evidence_key.clone(),
        })
        .map_err(|error| CliError::failure(error.to_string()))?;
        Ok(StoredNativeBuildReport {
            output: bake.output.clone(),
            cargo_process_started: bake.cargo_process_started,
            reused: bake.reused,
        })
    }
}

/// Prepare a selected plan in the same owner as ordinary build/test, retaining the client's original execution lease.
/// The caller must keep the returned lease alive until its local child exits. Program arguments and terminal handles
/// never cross the owner protocol. Non-Unix hosts execute the same preparation in process.
pub(crate) fn prepare_stored_native(
    request: OvenStoredDirectRustcRunRequest<'_>,
) -> CliResult<(StoredNativeBuildReport, OvenStoreLease)> {
    let absolute = |path: &Path| std::path::absolute(path).map_err(|error| CliError::failure(error.to_string()));
    let stored = StoredNativeBuildRequest {
        store: absolute(request.store.root())?,
        limits: *request.store.limits(),
        plan_identity: request.plan_identity,
        receipt: request.receipt,
        rustc: absolute(&request.rustc)?,
        source: absolute(&request.source)?,
        output: absolute(&request.output)?,
        crate_name: request.crate_name,
        edition: request.edition,
        source_evidence_key: request.source_evidence_key,
    };
    stored.validate_paths()?;
    let (_, _, _, lease) = request
        .store
        .select_payload_for_execution(&stored.plan_identity)
        .map_err(|error| CliError::failure(error.to_string()))?;
    let intent = super::NativeBuildIntent {
        native_build: super::NativeBuildRequest::Stored(stored),
    };
    #[cfg(unix)]
    let result = super::unix::submit(
        &std::env::current_exe().map_err(|error| CliError::failure(error.to_string()))?,
        serde_json::to_value(intent).map_err(|error| CliError::failure(error.to_string()))?,
        |_| {},
    )?;
    #[cfg(not(unix))]
    let result = intent
        .native_build
        .execute(&mut incan_driver::build::executable::ExecutableBuildOwner::default())?;
    let report = serde_json::from_value(result).map_err(|error| CliError::failure(error.to_string()))?;
    Ok((report, lease))
}

#[cfg(test)]
mod tests {
    use super::StoredNativeBuildRequest;
    use oven_store::store::OvenStoreLimits;

    /// Relative wire coordinates are rejected before the owner can touch a Store, probe a compiler or write output.
    #[test]
    fn stored_native_intent_refuses_relative_paths_before_preparation() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        std::fs::write(
            root.path().join("Cargo.toml"),
            r#"[package]
name = "native-owner-test"
version = "0.1.0"
"#,
        )?;
        std::fs::write(root.path().join("Cargo.lock"), "version = 4\n")?;
        let receipt = oven_store::import_frozen_project(&oven_store::OvenImportRequest::new(
            root.path(),
            "aarch64-apple-darwin",
            "rustc fixture",
            "debug",
            Vec::new(),
        ))?;
        let request = StoredNativeBuildRequest {
            store: root.path().join("absent-store"),
            limits: OvenStoreLimits::new(1024, 1024, 1024),
            plan_identity: "unselected".to_string(),
            receipt,
            rustc: root.path().join("absent-rustc"),
            source: root.path().join("absent-source"),
            output: root.path().join("absent-output"),
            crate_name: "fixture".to_string(),
            edition: "2024".to_string(),
            source_evidence_key: "source".to_string(),
        };
        let encoded = serde_json::to_value(&request)?;
        for field in ["store", "rustc", "source", "output"] {
            let mut invalid = encoded.clone();
            invalid[field] = serde_json::json!("relative");
            let invalid: super::super::NativeBuildRequest = serde_json::from_value(invalid)?;
            let mut owner = incan_driver::build::executable::ExecutableBuildOwner::default();
            let Err(error) = invalid.execute(&mut owner) else {
                return Err(format!("relative {field} was admitted").into());
            };
            assert!(error.message.contains("absolute stored native build paths"));
            assert!(!request.store.exists());
            assert!(!request.output.exists());
        }
        let mut ambiguous = encoded;
        ambiguous["declaration"] = serde_json::json!("/foreign/declaration");
        assert!(serde_json::from_value::<super::super::NativeBuildRequest>(ambiguous).is_err());
        Ok(())
    }
}
