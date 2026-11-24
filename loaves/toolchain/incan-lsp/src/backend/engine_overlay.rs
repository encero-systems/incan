//! Versioned diagnostics and saved builds through Incan's persistent editor overlay.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::Mutex;

use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString, Position, Range};

/// The document an overlay request answers with.
const OVERLAY_SCHEMA: &str = "incan.overlay-diagnostics/1";
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

/// Ask the engine for one document's diagnostics with `source` as its unsaved buffer.
pub(super) fn diagnostics(
    path: &Path,
    source: &str,
    version: i32,
    profile: &str,
    open_sources: &HashMap<PathBuf, String>,
) -> Result<Vec<Diagnostic>, String> {
    let mut server = REQUESTS.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let buffer = buffer_path(path);
    std::fs::write(&buffer, source)
        .map_err(|error| format!("cannot write overlay buffer {}: {error}", buffer.display()))?;
    let version = version.max(0);
    let answered = served_answer(
        &mut server,
        path,
        &buffer,
        version,
        profile,
        Ask::Diagnostics(open_sources),
    );
    let _ = std::fs::remove_file(&buffer);
    document_diagnostics(path, &answered?, version)
}

/// Build the saved document in its editor view, so the project's next build of the same bytes reuses the unit.
pub(super) fn saved(path: &Path, profile: &str) -> Result<(), String> {
    let mut server = REQUESTS.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let answered = served_answer(&mut server, path, path, 0, profile, Ask::Build)?;
    if let Some(reason) = answered["error"].as_str() {
        return Err(reason.to_string());
    }
    if answered["ok"] != true {
        return Err("the saved document's build overlay did not build".into());
    }
    Ok(())
}

/// What one overlay request asks the engine for.
enum Ask<'a> {
    Diagnostics(&'a HashMap<PathBuf, String>),
    Build,
}

/// One answer from the persistent Oven, starting it when none runs and once more if it stopped answering.
fn served_answer(
    server: &mut Option<OverlayServer>,
    path: &Path,
    buffer: &Path,
    version: i32,
    profile: &str,
    ask: Ask<'_>,
) -> Result<serde_json::Value, String> {
    let mut request = serde_json::json!({"source": path, "buffer": buffer, "version": version});
    match ask {
        Ask::Diagnostics(open_sources) => {
            let siblings = open_sources
                .iter()
                .filter(|(open, _)| open.as_path() != path)
                .collect::<HashMap<_, _>>();
            request["open_buffers"] =
                serde_json::to_value(siblings).map_err(|error| format!("cannot encode editor buffers: {error}"))?;
        }
        Ask::Build => request["build"] = serde_json::json!(true),
    }
    let request = request.to_string();
    let mut last = String::new();
    for _ in 0..2 {
        if server.is_none() {
            *server = Some(start_server(profile)?);
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

/// Start the adjacent Incan overlay server with the LSP session's artifact profile.
fn start_server(profile: &str) -> Result<OverlayServer, String> {
    let mut child = Command::new(oven()?)
        .arg("check")
        .arg("--overlay-serve")
        .arg("--profile")
        .arg(profile)
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
    if entry["end_line"].is_null() || end < start {
        end = Position::new(start.line, start.character + 1);
    }
    let severity = match entry["severity"].as_str() {
        Some("warning") => DiagnosticSeverity::WARNING,
        Some("information") => DiagnosticSeverity::INFORMATION,
        _ => DiagnosticSeverity::ERROR,
    };
    let code = entry["code"]
        .as_str()
        .filter(|code| !code.is_empty())
        .map(|code| NumberOrString::String(code.to_owned()));
    Diagnostic {
        range: Range::new(start, end),
        severity: Some(severity),
        code,
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
