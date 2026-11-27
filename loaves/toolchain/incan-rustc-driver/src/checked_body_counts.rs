//! Project native checker observations into the canonical Incan reporting schema.

use incan_driver::check::CheckedBodies;
use incan_driver::error::{CliError, CliResult};
use std::io::Write;
use std::path::Path;

pub(crate) fn encode(bodies: &CheckedBodies, decoded: usize) -> CliResult<Vec<u8>> {
    let count = |n| i64::try_from(n).map_err(|_| CliError::failure("checked body count exceeds i64"));
    let checked: usize = bodies.modules.iter().map(|module| module.checked.len()).sum();
    let reused: usize = bodies.modules.iter().map(|module| module.reused.len()).sum();
    internal_models::encode_checked_body_counts(&internal_models::CheckedBodyCounts {
        checked: count(checked)?,
        reused: count(reused)?,
        decoded: count(decoded)?,
    })
    .map_err(CliError::failure)
}

/// Publish the original native activation's tally at its private, previously unused output path.
pub(crate) fn publish_provider(report: &Path, descriptor: &str, evaluations: usize) -> Result<(), String> {
    let attempt = report.to_str().ok_or("provider body tally path is not UTF-8")?;
    let evaluations = i64::try_from(evaluations).map_err(|_| "provider body tally exceeds i64")?;
    let bytes = internal_models::encode_provider_body_tally(descriptor.to_owned(), attempt.to_owned(), evaluations)?;
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(report)
        .and_then(|mut file| file.write_all(&bytes))
        .map_err(|error| format!("cannot write provider body tally {}: {error}", report.display()))
}

#[cfg(test)]
mod tests {
    use incan_driver::check::ModuleBodies;

    use super::*;

    #[test]
    fn private_counts_preserve_public_check_totals_and_physical_decodes() -> Result<(), Box<dyn std::error::Error>> {
        let observations = CheckedBodies {
            modules: vec![
                ModuleBodies {
                    module: "app".into(),
                    checked: vec!["entry".into()],
                    reused: vec!["one".into(), "two".into()],
                },
                ModuleBodies {
                    module: "model".into(),
                    checked: vec!["read".into()],
                    reused: Vec::new(),
                },
            ],
        };
        let public: serde_json::Value = serde_json::from_str(&observations.to_json()?)?;
        let encoded = encode(&observations, 1)?;
        let private = internal_models::decode_checked_body_counts(&encoded)?;
        assert_eq!(public["checked"].as_i64(), Some(private.checked));
        assert_eq!(public["reused"].as_i64(), Some(private.reused));
        assert_eq!((private.checked, private.reused, private.decoded), (2, 2, 1));
        assert!(internal_models::decode_checked_body_counts(&serde_json::to_vec(&public)?).is_err());
        Ok(())
    }
}
