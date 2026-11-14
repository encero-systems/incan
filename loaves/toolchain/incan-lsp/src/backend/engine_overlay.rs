//! Diagnostics through the records engine's overlay request (14.1), selected with `INCAN_LSP_ENGINE=overlay`.
//!
//! The server writes the unsaved buffer to a private file and asks Oven, beside it, for
//! `incan check SOURCE --overlay BUFFER --document-version N --format json`. Oven answers from the engine's editor view
//! with one `incan.overlay-diagnostics/1` document; the server keeps only the diagnostics of the document it asked
//! about, and only when the answer names the version it asked for.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString, Position, Range};

/// The environment selection that routes diagnostics through the engine's overlay request.
pub(super) const ENGINE_SELECTION: &str = "INCAN_LSP_ENGINE";
/// The document an overlay request answers with.
const OVERLAY_SCHEMA: &str = "incan.overlay-diagnostics/1";
/// Requests of one program share its editor view's overlay root, so this server sends them one at a time.
static REQUESTS: Mutex<()> = Mutex::new(());

/// Whether this server answers diagnostics through the engine's overlay request.
pub(super) fn selected() -> bool {
    std::env::var(ENGINE_SELECTION).is_ok_and(|value| value == "overlay")
}

/// Ask the engine for one document's diagnostics with `source` as its unsaved buffer.
pub(super) fn diagnostics(path: &Path, source: &str, version: i32) -> Result<Vec<Diagnostic>, String> {
    let _turn = REQUESTS.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let buffer = buffer_path(path);
    std::fs::write(&buffer, source)
        .map_err(|error| format!("cannot write overlay buffer {}: {error}", buffer.display()))?;
    let output = Command::new(oven()?)
        .arg("check")
        .arg(path)
        .arg("--overlay")
        .arg(&buffer)
        .arg("--document-version")
        .arg(version.max(0).to_string())
        .arg("--format")
        .arg("json")
        .output()
        .map_err(|error| format!("cannot run the overlay request: {error}"))?;
    let _ = std::fs::remove_file(&buffer);
    let document: serde_json::Value = serde_json::from_slice(&output.stdout).map_err(|error| {
        format!(
            "overlay request answered no document ({error}): {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
    })?;
    if document["schema"] != OVERLAY_SCHEMA || document["version"] != i64::from(version.max(0)) {
        return Err("overlay answer names another schema or document version".into());
    }
    let wanted = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let mut found = Vec::new();
    for entry in document["diagnostics"].as_array().into_iter().flatten() {
        let named = PathBuf::from(entry["path"].as_str().unwrap_or_default());
        if named.canonicalize().unwrap_or(named) != wanted {
            continue;
        }
        found.push(lsp_diagnostic(entry));
    }
    Ok(found)
}

/// One answered diagnostic at its one-based line and column, as an LSP diagnostic at that position.
fn lsp_diagnostic(entry: &serde_json::Value) -> Diagnostic {
    let line = entry["line"].as_u64().unwrap_or(1).saturating_sub(1) as u32;
    let column = entry["column"].as_u64().unwrap_or(1).saturating_sub(1) as u32;
    let severity = match entry["severity"].as_str() {
        Some("warning") => DiagnosticSeverity::WARNING,
        _ => DiagnosticSeverity::ERROR,
    };
    let position = Position::new(line, column);
    Diagnostic {
        range: Range::new(position, Position::new(line, column + 1)),
        severity: Some(severity),
        code: Some(NumberOrString::String("overlay".into())),
        source: Some("incan".into()),
        message: entry["message"].as_str().unwrap_or_default().to_string(),
        ..Default::default()
    }
}

/// The private file that holds one document's unsaved buffer for the request.
fn buffer_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    std::env::temp_dir().join(format!("incan-lsp-overlay-{}-{name}", std::process::id()))
}

/// The Oven installed beside this server.
fn oven() -> Result<PathBuf, String> {
    let server = std::env::current_exe().map_err(|error| format!("cannot locate the language server: {error}"))?;
    let directory = server.parent().ok_or("the language server has no directory")?;
    Ok(directory.join("incan"))
}
