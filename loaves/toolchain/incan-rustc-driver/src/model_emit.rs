//! Contract-backed model emit (`--emit-model`): one canonical model bundle as formatted Incan model source or as its
//! bundle JSON, read from checked model metadata. Oven selects the metadata: a bundle JSON file, or the `.incnlib` a
//! package's checked source interface publishes.

use std::path::Path;

use incan_frontend::contract_metadata::{CanonicalModelBundle, read_model_bundles_from_json};
use incan_frontend::library_manifest::LibraryManifest;
use incan_frontend::loaf_index::LIBRARY_MANIFEST_EXTENSION;

/// Print the bundle `MODEL` names, by logical type name or stable model id, in `FORMAT` (`incan` or `json`). `LABEL`
/// names the inspected input in a refusal; it defaults to `PATH`.
pub(crate) fn emit(arguments: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let (path, model, format, label) = match arguments {
        [path, model, format] => (path, model, format, path),
        [path, model, format, label] => (path, model, format, label),
        _ => return Err("usage: incan-rustc-driver --emit-model PATH MODEL incan|json [LABEL]".into()),
    };
    if format.as_str() != "incan" && format.as_str() != "json" {
        return Err(format!("model format is incan or json, got `{format}`").into());
    }
    let bundles = model_bundles(Path::new(path))?;
    let Some(bundle) = bundles
        .iter()
        .find(|bundle| bundle.logical_type_name == *model || bundle.stable_model_id.as_deref() == Some(model.as_str()))
    else {
        let mut available = bundles
            .iter()
            .map(|bundle| bundle.logical_type_name.clone())
            .collect::<Vec<_>>();
        available.sort();
        let listed = if available.is_empty() {
            "none".to_owned()
        } else {
            available.join(", ")
        };
        return Err(format!(
            "model `{model}` was not found in checked model metadata for {label} (available: {listed})"
        )
        .into());
    };
    if format.as_str() == "incan" {
        print!("{}", bundle.emit_incan_model_source()?);
    } else {
        println!("{}", serde_json::to_string_pretty(bundle)?);
    }
    Ok(())
}

/// The canonical model bundles of a bundle JSON file, or of a library manifest's checked contract metadata.
fn model_bundles(path: &Path) -> Result<Vec<CanonicalModelBundle>, Box<dyn std::error::Error>> {
    let manifest = path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension == LIBRARY_MANIFEST_EXTENSION);
    if manifest {
        return Ok(LibraryManifest::read_from_path(path)?
            .contract_metadata
            .models
            .model_bundles);
    }
    Ok(read_model_bundles_from_json(path)?)
}
