//! Diagnostics through the records engine's overlay request (14.1), selected with `INCAN_LSP_ENGINE`.
//!
//! `INCAN_LSP_ENGINE=overlay` keeps one Oven beside the server running `incan check --overlay-serve`: it holds one
//! engine connection, and each analysis is one JSON line in and one `incan.overlay-diagnostics/1` line out.
//! `INCAN_LSP_ENGINE=overlay-process` starts `incan check SOURCE --overlay BUFFER ...` once per analysis instead.
//! Either way the server keeps only the diagnostics of the document it asked about, and only when the answer names the
//! version it asked for.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::Mutex;

use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, Location, NumberOrString, Position, Range, Url};

/// The environment selection that routes diagnostics through the engine's overlay request.
pub(super) const ENGINE_SELECTION: &str = "INCAN_LSP_ENGINE";
/// The document an overlay request answers with.
const OVERLAY_SCHEMA: &str = "incan.overlay-diagnostics/1";
/// The document an overlay definition request answers with.
const DEFINITION_SCHEMA: &str = "incan.overlay-definition/1";
/// How long typing must pause before the in-process analysis behind hover and navigation runs on the overlay route.
pub(super) const ANALYSIS_PAUSE_MS: u64 = 300;
/// Requests of one program share its editor view's overlay root, so this server sends them one at a time; the lock
/// also owns the persistent Oven, when one is running.
static REQUESTS: Mutex<Option<OverlayServer>> = Mutex::new(None);

/// One persistent `incan check --overlay-serve` and its request and answer pipes.
struct OverlayServer {
    child: Child,
    requests: ChildStdin,
    answers: BufReader<ChildStdout>,
}

impl Drop for OverlayServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Whether this server answers diagnostics through the engine's overlay request.
pub(super) fn selected() -> bool {
    std::env::var(ENGINE_SELECTION).is_ok_and(|value| value == "overlay" || value == "overlay-process")
}

/// Whether each analysis starts its own Oven instead of using the persistent one.
fn per_process() -> bool {
    std::env::var(ENGINE_SELECTION).is_ok_and(|value| value == "overlay-process")
}

/// Ask the engine for one document's diagnostics with `source` as its unsaved buffer.
pub(super) fn diagnostics(path: &Path, source: &str, version: i32) -> Result<Vec<Diagnostic>, String> {
    let mut server = REQUESTS.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let buffer = buffer_path(path);
    std::fs::write(&buffer, source)
        .map_err(|error| format!("cannot write overlay buffer {}: {error}", buffer.display()))?;
    let version = version.max(0);
    let answered = if per_process() {
        process_answer(path, &buffer, version, &Ask::Diagnostics)
    } else {
        served_answer(&mut server, path, &buffer, version, &Ask::Diagnostics)
    };
    let _ = std::fs::remove_file(&buffer);
    document_diagnostics(path, &answered?, version)
}

/// Build the saved document in its editor view, so the project's next build of the same bytes reuses the unit.
pub(super) fn saved(path: &Path) -> Result<(), String> {
    let mut server = REQUESTS.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let answered = if per_process() {
        process_answer(path, path, 0, &Ask::Build)?
    } else {
        served_answer(&mut server, path, path, 0, &Ask::Build)?
    };
    if let Some(reason) = answered["error"].as_str() {
        return Err(reason.to_string());
    }
    if answered["ok"] != true {
        return Err("the saved document's build overlay did not build".into());
    }
    Ok(())
}

/// The signature of the declaration named at one-based `line` and `column` of `source`, answered from the editor
/// view's records for an imported declaration, or from the buffer itself for one of its own; `None` names nothing.
pub(super) fn hover(path: &Path, source: &str, line: u32, column: u32) -> Result<Option<String>, String> {
    let mut server = REQUESTS.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let buffer = buffer_path(path);
    std::fs::write(&buffer, source)
        .map_err(|error| format!("cannot write overlay buffer {}: {error}", buffer.display()))?;
    let ask = Ask::Hover(line, column);
    let answered = if per_process() {
        process_answer(path, &buffer, 0, &ask)
    } else {
        served_answer(&mut server, path, &buffer, 0, &ask)
    };
    let _ = std::fs::remove_file(&buffer);
    let answered = answered?;
    if let Some(reason) = answered["error"].as_str() {
        return Err(reason.to_string());
    }
    Ok(answered["hover"].as_str().map(str::to_string))
}

/// The declaration at one-based `line` and `column` of `source`, resolved by the editor view's source projections.
pub(super) fn definition(path: &Path, source: &str, line: u32, column: u32) -> Result<Option<Location>, String> {
    let mut server = REQUESTS.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let buffer = buffer_path(path);
    std::fs::write(&buffer, source)
        .map_err(|error| format!("cannot write overlay buffer {}: {error}", buffer.display()))?;
    let ask = Ask::Definition(line, column);
    let answered = if per_process() {
        process_answer(path, &buffer, 0, &ask)
    } else {
        served_answer(&mut server, path, &buffer, 0, &ask)
    };
    let _ = std::fs::remove_file(&buffer);
    let answered = answered?;
    if let Some(reason) = answered["error"].as_str() {
        return Err(reason.to_string());
    }
    if answered["schema"] != DEFINITION_SCHEMA {
        return Err("overlay definition answer names another schema".into());
    }
    let Some(definition) = answered["definition"].as_object() else {
        return Ok(None);
    };
    let named = definition
        .get("path")
        .and_then(serde_json::Value::as_str)
        .ok_or("overlay definition has no path")?;
    let uri = Url::from_file_path(named).map_err(|()| format!("overlay definition path is not absolute: {named}"))?;
    let line = definition
        .get("line")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(1)
        .saturating_sub(1) as u32;
    let column = definition
        .get("column")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(1)
        .saturating_sub(1) as u32;
    let position = Position::new(line, column);
    Ok(Some(Location::new(uri, Range::new(position, position))))
}

/// What one overlay request asks the engine for.
enum Ask {
    Diagnostics,
    Build,
    Hover(u32, u32),
    Definition(u32, u32),
}

/// One answer from the persistent Oven, starting it when none runs and once more if it stopped answering.
fn served_answer(
    server: &mut Option<OverlayServer>,
    path: &Path,
    buffer: &Path,
    version: i32,
    ask: &Ask,
) -> Result<serde_json::Value, String> {
    let mut request = serde_json::json!({"source": path, "buffer": buffer, "version": version});
    match ask {
        Ask::Diagnostics => {}
        Ask::Build => request["build"] = serde_json::json!(true),
        Ask::Hover(line, column) => request["hover"] = serde_json::json!([line, column]),
        Ask::Definition(line, column) => request["definition"] = serde_json::json!([line, column]),
    }
    let request = request.to_string();
    let mut last = String::new();
    for _ in 0..2 {
        if server.is_none() {
            *server = Some(start_server()?);
        }
        let Some(running) = server.as_mut() else {
            continue;
        };
        match exchange(running, &request) {
            Ok(answer) => return Ok(answer),
            Err(reason) => {
                last = reason;
                *server = None;
            }
        }
    }
    Err(last)
}

/// Start `incan check --overlay-serve` beside this server.
fn start_server() -> Result<OverlayServer, String> {
    let mut child = Command::new(oven()?)
        .arg("check")
        .arg("--overlay-serve")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("cannot start the overlay server: {error}"))?;
    let requests = child.stdin.take().ok_or("the overlay server has no request pipe")?;
    let answers = BufReader::new(child.stdout.take().ok_or("the overlay server has no answer pipe")?);
    Ok(OverlayServer {
        child,
        requests,
        answers,
    })
}

/// Send one request line and read its answer line.
fn exchange(server: &mut OverlayServer, request: &str) -> Result<serde_json::Value, String> {
    writeln!(server.requests, "{request}").map_err(|error| format!("overlay server request failed: {error}"))?;
    server
        .requests
        .flush()
        .map_err(|error| format!("overlay server request failed: {error}"))?;
    let mut line = String::new();
    let read = server
        .answers
        .read_line(&mut line)
        .map_err(|error| format!("overlay server answer failed: {error}"))?;
    if read == 0 {
        return Err("the overlay server stopped".into());
    }
    serde_json::from_str(&line).map_err(|error| format!("overlay server answered no document ({error})"))
}

/// One answer from an Oven started for this analysis alone.
fn process_answer(path: &Path, buffer: &Path, version: i32, ask: &Ask) -> Result<serde_json::Value, String> {
    let mut command = Command::new(oven()?);
    command
        .arg("check")
        .arg(path)
        .arg("--overlay")
        .arg(buffer)
        .arg("--document-version")
        .arg(version.to_string())
        .arg("--format")
        .arg("json");
    match ask {
        Ask::Diagnostics => {}
        Ask::Build => {
            command.arg("--build");
        }
        Ask::Hover(line, column) => {
            command.arg("--hover").arg(format!("{line}:{column}"));
        }
        Ask::Definition(line, column) => {
            command.arg("--definition").arg(format!("{line}:{column}"));
        }
    }
    let output = command
        .output()
        .map_err(|error| format!("cannot run the overlay request: {error}"))?;
    serde_json::from_slice(&output.stdout).map_err(|error| {
        format!(
            "overlay request answered no document ({error}): {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
    })
}

/// The answer's diagnostics for `path`, when the answer is for the version asked about.
fn document_diagnostics(path: &Path, document: &serde_json::Value, version: i32) -> Result<Vec<Diagnostic>, String> {
    if let Some(reason) = document["error"].as_str() {
        return Err(reason.to_string());
    }
    if document["schema"] != OVERLAY_SCHEMA || document["version"] != i64::from(version) {
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

/// One answered diagnostic over its one-based start and end, as an LSP diagnostic over that range.
fn lsp_diagnostic(entry: &serde_json::Value) -> Diagnostic {
    let position = |line: &str, column: &str| {
        Position::new(
            entry[line].as_u64().unwrap_or(1).saturating_sub(1) as u32,
            entry[column].as_u64().unwrap_or(1).saturating_sub(1) as u32,
        )
    };
    let start = position("line", "column");
    let mut end = position("end_line", "end_column");
    if entry["end_line"].is_null() || end <= start {
        end = Position::new(start.line, start.character + 1);
    }
    let severity = match entry["severity"].as_str() {
        Some("warning") => DiagnosticSeverity::WARNING,
        _ => DiagnosticSeverity::ERROR,
    };
    let code = entry["code"].as_str().unwrap_or("overlay").to_string();
    Diagnostic {
        range: Range::new(start, end),
        severity: Some(severity),
        code: Some(NumberOrString::String(code)),
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
