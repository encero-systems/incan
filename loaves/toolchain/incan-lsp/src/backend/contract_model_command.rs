//! The contract model emit command.

use super::*;

/// Parse model-emit command arguments from either one object argument or positional `[uriOrPath, model, format?]`.
pub(super) fn parse_emit_contract_model_command_args(
    mut args: Vec<serde_json::Value>,
) -> std::result::Result<EmitContractModelCommandArgs, String> {
    if args.len() == 1 && args[0].is_object() {
        return serde_json::from_value(args.remove(0)).map_err(|error| error.to_string());
    }
    if args.len() < 2 {
        return Err(format!(
            "{EMIT_CONTRACT_MODEL_COMMAND} expects either an object argument or positional [uriOrPath, model, format?]"
        ));
    }
    let uri = args[0]
        .as_str()
        .ok_or_else(|| "first positional argument must be a URI or path string".to_string())?
        .to_string();
    let model = args[1]
        .as_str()
        .ok_or_else(|| "second positional argument must be a model name or stable model id".to_string())?
        .to_string();
    let format = args.get(2).and_then(|value| value.as_str()).map(str::to_string);
    Ok(EmitContractModelCommandArgs {
        uri: Some(uri),
        path: None,
        model,
        format,
    })
}

/// Convert a file URI or raw client path into a local filesystem path.
pub(super) fn path_from_lsp_uri_or_path(value: &str) -> std::result::Result<PathBuf, String> {
    if let Ok(uri) = Url::parse(value)
        && uri.scheme() == "file"
    {
        return uri
            .to_file_path()
            .map_err(|_| format!("file URI `{value}` could not be converted to a local path"));
    }
    Ok(PathBuf::from(value))
}

/// Load model bundles for the LSP command from a project, source path, bundle JSON file, or library artifact.
pub(super) fn collect_lsp_contract_model_bundles(
    path: &Path,
    profile: &str,
) -> std::result::Result<Vec<CanonicalModelBundle>, String> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| format!("failed to determine current directory: {error}"))?
            .join(path)
    };
    if absolute.is_file()
        && absolute
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension == "json")
    {
        return read_model_bundles_from_json(&absolute).map_err(|error| error.to_string());
    }
    if absolute.is_file()
        && absolute
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension == LIBRARY_MANIFEST_EXTENSION)
    {
        let manifest = incan_frontend::library_manifest::LibraryManifest::read_from_path(&absolute)
            .map_err(|error| error.to_string())?;
        let bundles = manifest.contract_metadata.models.model_bundles;
        if bundles.is_empty() {
            return Err(format!(
                "artifact {} does not carry checked model metadata",
                absolute.display()
            ));
        }
        return Ok(bundles);
    }

    // Description needs an existing file in the project, not parsed source. A directory request uses its manifest
    // as that anchor; Oven remains the sole reader of project settings, including model bundle declarations.
    let anchor = if absolute.is_dir() {
        absolute.join("loaf.toml")
    } else {
        absolute
    };
    let unit = analysis::unit_description(&anchor, profile).map_err(|error| error.message)?;
    read_project_model_bundles(&unit.project_root, &unit.contract_models).map_err(|error| error.to_string())
}

/// Resolve a single LSP model-emit bundle by logical type name or stable model id.
pub(super) fn find_lsp_contract_model_bundle(
    path: &Path,
    model: &str,
    profile: &str,
) -> std::result::Result<CanonicalModelBundle, String> {
    let bundles = collect_lsp_contract_model_bundles(path, profile)?;
    bundles
        .into_iter()
        .find(|bundle| bundle.logical_type_name == model || bundle.stable_model_id.as_deref() == Some(model))
        .ok_or_else(|| {
            format!(
                "model `{model}` was not found in checked model metadata for {}",
                path.display()
            )
        })
}

/// Build the JSON-RPC response payload for model source or bundle JSON emission.
pub(super) fn emit_contract_model_command_payload(
    path: &Path,
    model: &str,
    format: ContractModelCommandFormat,
    profile: &str,
) -> std::result::Result<serde_json::Value, String> {
    let bundle = find_lsp_contract_model_bundle(path, model, profile)?;
    match format {
        ContractModelCommandFormat::Incan => {
            let source = bundle.emit_incan_model_source().map_err(|error| error.to_string())?;
            Ok(json!({
                "format": format.as_str(),
                "model": bundle.logical_type_name,
                "stableModelId": bundle.stable_model_id,
                "source": source
            }))
        }
        ContractModelCommandFormat::Json => Ok(json!({
            "format": format.as_str(),
            "model": bundle.logical_type_name,
            "stableModelId": bundle.stable_model_id,
            "bundle": bundle
        })),
    }
}
