//! Retain bounded native workers and pure responses through the compilation owner's lifetime.

use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender};
use std::time::Duration;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::VocabDesugarPassError;
use super::surface::{self, Output};
use crate::ast;
use crate::loaf_index::LoafIndex;

/// Every framed invocation, including blocked input and output, shares this deadline.
const INVOCATION_TIMEOUT: Duration = Duration::from_secs(10);
/// Refuse unbounded provider output before JSON allocation or decoding.
const MAX_FRAME_BYTES: u64 = 8 * 1024 * 1024;

/// One admitted executable identity selected through the consumer's resolved dependency index.
#[derive(Clone)]
struct Provider {
    /// The providing Loaf's published name.
    name: String,
    /// Compiled vocabulary unit identity, independent of its artifact location.
    identity: String,
    /// Verified executable under the providing artifact directory.
    path: PathBuf,
    /// Output position from the published declaration descriptor.
    target: incan_vocab::DesugarTarget,
}

/// One node waiting for its module's batched invocation.
struct Pending {
    /// The provider the parser selected for this node.
    provider: Provider,
    /// The public structured request.
    request: Value,
    /// The source node's enclosing range.
    span: ast::Span,
    /// Request identity including the vocabulary unit.
    digest: String,
}

/// Compilation-owner state; workers and responses are keyed by immutable vocabulary unit identity.
#[derive(Default)]
pub struct NativeDesugarerRuntime {
    /// Live workers owned by this runtime and reaped when it is dropped.
    workers: HashMap<String, Worker>,
    /// Only validated responses and node-local errors are retained in memory.
    responses: HashMap<String, Result<Output, VocabDesugarPassError>>,
    /// Module nodes collected by the same traversal that later rewrites them.
    pending: Vec<Pending>,
    /// Collection visits source nodes without invoking providers or using placeholder output.
    collecting: bool,
    /// Unit identities explicitly admitted by the build's provider policy.
    admitted: Vec<String>,
    /// The consumer's response and build-evidence directory.
    directory: PathBuf,
    /// Consumer root used to keep evidence module paths repository-scoped.
    project_root: PathBuf,
    /// The host policy whose admission decision this owner records.
    policy: String,
}

impl std::fmt::Debug for NativeDesugarerRuntime {
    /// Expose retained counts without executable paths, request bodies or compiler-private state.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativeDesugarerRuntime")
            .field("workers", &self.workers.len())
            .field("responses", &self.responses.len())
            .finish_non_exhaustive()
    }
}

impl NativeDesugarerRuntime {
    /// Create an empty owner; no process starts before an admitted vocabulary node requests expansion.
    pub fn new() -> Result<Self, VocabDesugarPassError> {
        Ok(Self::default())
    }

    /// Set the build's explicit admission decision and consumer root for repository-scoped evidence.
    pub fn configure(&mut self, admitted: Vec<String>, project_root: PathBuf, policy: String) {
        self.admitted = admitted;
        self.directory = project_root.join("target/incan/vocab-responses");
        self.project_root = project_root;
        self.policy = policy;
    }

    /// Begin collecting a fresh module, leaving unrelated workers and pure responses retained.
    pub(super) fn begin_module(&mut self) {
        self.pending.clear();
        self.responses.retain(|_, response| response.is_ok());
        self.collecting = true;
    }

    /// Invoke each provider once for the module's uncached nodes, then make validated outputs available to rewriting.
    pub(super) fn finish_module(&mut self, module: &str) {
        self.collecting = false;
        let pending = std::mem::take(&mut self.pending);
        let mut batches: HashMap<String, Vec<Pending>> = HashMap::new();
        for item in pending {
            if self.responses.contains_key(&item.digest) {
                self.evidence(&item.provider, module, "reused");
                continue;
            }
            if let Some(reply) = self.read_cached(&item) {
                self.evidence(&item.provider, module, "reused");
                self.responses.insert(item.digest, reply);
                continue;
            }
            batches.entry(item.provider.identity.clone()).or_default().push(item);
        }
        for items in batches.into_values() {
            self.invoke_batch(items, module);
        }
    }

    /// Obtain a validated expansion, or collect its public request during the traversal's preparation phase.
    pub(super) fn desugar_node(
        &mut self,
        loaves: &LoafIndex,
        block: &ast::VocabBlockStmt,
        span: ast::Span,
        module: Option<&str>,
    ) -> Result<Output, VocabDesugarPassError> {
        let provider = resolve_provider(loaves, block)?;
        let failure = |message| native_error(&provider, message);
        if !self.admitted.contains(&provider.identity) {
            self.evidence(&provider, module.unwrap_or(""), "denied");
            return Err(failure(
                "provider policy denied this vocabulary unit; a locked dependency and admission are required".into(),
            ));
        }
        let request = surface::request(block, span, module.unwrap_or("")).map_err(failure)?;
        let digest = request_digest(&provider.identity, &request, span);
        if self.collecting {
            self.pending.push(Pending {
                provider,
                request,
                span,
                digest,
            });
            return Err(VocabDesugarPassError::Resolution {
                keyword: block.keyword.clone(),
                message: "collecting module vocabulary requests".into(),
            });
        }
        self.responses
            .get(&digest)
            .cloned()
            .unwrap_or_else(|| Err(failure("module vocabulary request was not prepared".into())))
    }

    /// Run one framed batch under the invocation deadline, preserving a separate diagnostic for every source node.
    fn invoke_batch(&mut self, items: Vec<Pending>, module: &str) {
        let Some(first) = items.first() else {
            return;
        };
        let provider = first.provider.clone();
        let packet = json!({"requests": items.iter().map(|item| &item.request).collect::<Vec<_>>()});
        let result = self.worker_reply(&provider, &packet);
        let replies = result.and_then(|reply| {
            reply
                .get("responses")
                .and_then(Value::as_array)
                .filter(|values| values.len() == items.len())
                .cloned()
                .ok_or_else(|| "malformed response: batch reply count differs from its request".to_string())
        });
        let mut failed = false;
        for (index, item) in items.into_iter().enumerate() {
            let reply = match &replies {
                Ok(replies) => validate_reply(&item.provider, &replies[index], item.span, &item.digest),
                Err(message) => Err(native_error(&provider, message.clone())),
            };
            failed |= reply.is_err();
            if let Ok(replies) = &replies {
                if reply.is_ok() {
                    self.write_cached(&item.digest, &replies[index]);
                }
            }
            self.evidence(&provider, module, "computed");
            self.responses.insert(item.digest, reply);
        }
        if failed {
            self.workers.remove(&provider.identity);
        }
    }

    /// Retain the native worker through its owner, withdrawing and reaping it after any invocation failure.
    fn worker_reply(&mut self, provider: &Provider, request: &Value) -> Result<Value, String> {
        if !self.workers.contains_key(&provider.identity) {
            self.workers
                .insert(provider.identity.clone(), Worker::start(&provider.path)?);
        }
        let result = self
            .workers
            .get_mut(&provider.identity)
            .ok_or_else(|| "worker owner lost its process".to_string())?
            .invoke(request);
        if result.is_err() {
            self.workers.remove(&provider.identity);
        }
        result
    }

    /// Revalidate cached public output against this request before accepting persistence as evidence of reuse.
    fn read_cached(&self, item: &Pending) -> Option<Result<Output, VocabDesugarPassError>> {
        let bytes = fs::read(self.directory.join(format!("{}.json", item.digest))).ok()?;
        let value = serde_json::from_slice(&bytes).ok()?;
        let reply = validate_reply(&item.provider, &value, item.span, &item.digest);
        reply.is_ok().then_some(reply)
    }

    /// Atomically retain a validated pure response; optional persistence failure never changes compilation semantics.
    fn write_cached(&self, digest: &str, reply: &Value) {
        if self.directory.as_os_str().is_empty() || fs::create_dir_all(&self.directory).is_err() {
            return;
        }
        let staged = self.directory.join(format!("{digest}.{}.tmp", std::process::id()));
        if let Ok(bytes) = serde_json::to_vec(reply) {
            if fs::write(&staged, bytes).is_ok() {
                let _ = fs::rename(&staged, self.directory.join(format!("{digest}.json")));
            }
        }
    }

    /// Append repository-scoped admission and reuse evidence, mirroring it through Oven's trace destination.
    fn evidence(&self, provider: &Provider, module: &str, decision: &str) {
        let path = Path::new(module);
        let module = path
            .strip_prefix(&self.project_root)
            .unwrap_or_else(|_| Path::new(path.file_name().unwrap_or_default()))
            .to_string_lossy();
        let admitted = decision != "denied";
        let record = json!({"provider": provider.name, "unit_identity": provider.identity, "module": module,
            "policy": self.policy, "admitted": admitted, "capabilities": [], "response": decision});
        if fs::create_dir_all(&self.directory).is_ok() {
            if let Ok(mut file) = OpenOptions::new()
                .create(true)
                .append(true)
                .open(self.directory.join("evidence.jsonl"))
            {
                let encoded = format!("{record}\n");
                let _ = file.write_all(encoded.as_bytes());
            }
        }
        let line = format!(
            "oven: vocabulary {} unit {} module {}: {} (policy {}; admitted {})",
            provider.name, provider.identity, module, decision, self.policy, admitted
        );
        match std::env::var("OVEN_TRACE").as_deref() {
            Ok("-") => eprintln!("{line}"),
            Ok(path) if !path.is_empty() => {
                if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
                    let _ = writeln!(file, "{line}");
                }
            }
            _ => {}
        }
    }
}

/// Convert a stable request and unit identity to the pure-response reuse key.
fn request_digest(identity: &str, request: &Value, span: ast::Span) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"incan.vocab.response/2\0");
    hasher.update(identity.as_bytes());
    hasher.update(request.to_string().as_bytes());
    if let Some(expansion) = span.expansion {
        hasher.update(expansion);
    }
    hex::encode(hasher.finalize())
}

/// Select the published worker by the parser's dependency identity, refusing old executable formats and path escapes.
fn resolve_provider(loaves: &LoafIndex, block: &ast::VocabBlockStmt) -> Result<Provider, VocabDesugarPassError> {
    let refusal = |message| VocabDesugarPassError::Resolution {
        keyword: block.keyword.clone(),
        message,
    };
    let key = &block.keyword_binding.dependency_key;
    let entry = loaves
        .entry(key)
        .ok_or_else(|| refusal(format!("unknown dependency key `{key}`")))?;
    let vocab = entry
        .artifact
        .manifest
        .vocab
        .as_ref()
        .ok_or_else(|| refusal(format!("dependency `{key}` has no vocab payload")))?;
    let artifact = vocab
        .desugarer_artifact
        .as_ref()
        .ok_or_else(|| refusal(format!("dependency `{key}` has no desugarer artifact")))?;
    if !matches!(artifact.artifact_kind, incan_vocab::DesugarerArtifactKind::NativeWorker) {
        return Err(refusal(
            "WebAssembly vocabulary desugarers are no longer supported; the Loaf must be rebuilt under RFC 160".into(),
        ));
    }
    let relative = Path::new(&artifact.relative_path);
    if relative.components().any(|part| !matches!(part, Component::Normal(_))) || relative.as_os_str().is_empty() {
        return Err(refusal(
            "desugarer artifact path escapes the dependency artifact root".into(),
        ));
    }
    let root = entry
        .artifact
        .dir
        .canonicalize()
        .map_err(|error| refusal(error.to_string()))?;
    let path = root
        .join(relative)
        .canonicalize()
        .map_err(|error| refusal(error.to_string()))?;
    if !path.starts_with(root) {
        return Err(refusal(
            "desugarer artifact resolves outside the dependency artifact root".into(),
        ));
    }
    let bytes = fs::read(&path).map_err(|error| refusal(error.to_string()))?;
    if hex::encode(Sha256::digest(bytes)) != artifact.sha256 {
        return Err(refusal("native desugarer artifact checksum mismatch".into()));
    }
    Ok(Provider {
        name: vocab.package_name.clone(),
        identity: artifact.unit_identity.clone(),
        path,
        target: vocab
            .dsl_surfaces
            .iter()
            .flat_map(|surface| &surface.declarations)
            .find_map(|declaration| declaration_target(declaration, block))
            .ok_or_else(|| refusal("native desugarer requires a registered output position".into()))?,
    })
}

/// Select the authored output position from the canonical declaration descriptors, including nested declarations.
fn declaration_target(
    declaration: &incan_vocab::DeclarationSurface,
    block: &ast::VocabBlockStmt,
) -> Option<incan_vocab::DesugarTarget> {
    if declaration.keyword == block.keyword && declaration.compound_tokens == block.keyword_binding.compound_tokens {
        return Some(declaration.desugars_to);
    }
    declaration
        .declarations
        .iter()
        .find_map(|child| declaration_target(child, block))
}

/// Decode a response or provider error only after validating every supplied source range.
fn validate_reply(
    provider: &Provider,
    reply: &Value,
    span: ast::Span,
    request_identity: &str,
) -> Result<Output, VocabDesugarPassError> {
    let failure = |message| native_error(provider, message);
    surface::validate_spans(reply, span).map_err(failure)?;
    if reply.as_object().is_none_or(|fields| fields.len() != 1) {
        return Err(failure(
            "malformed response: expected exactly one result variant".into(),
        ));
    }
    if let Some(response) = reply.get("Ok") {
        let output = surface::response(response, span, request_identity).map_err(failure)?;
        let position_matches = matches!(
            (&output, provider.target),
            (Output::Expression(_), incan_vocab::DesugarTarget::Expression)
                | (Output::Statements(_), incan_vocab::DesugarTarget::Statements)
                | (Output::Declarations(_), incan_vocab::DesugarTarget::Declarations)
        );
        return if position_matches {
            Ok(output)
        } else {
            Err(failure(
                "wrong output position for the registered vocabulary declaration".into(),
            ))
        };
    }
    if let Some(error) = reply.get("Err") {
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .ok_or_else(|| failure("malformed provider error".into()))?;
        let reported = error.get("span").and_then(|value| {
            Some(ast::Span::new(
                value.get("start")?.as_u64()?.try_into().ok()?,
                value.get("end")?.as_u64()?.try_into().ok()?,
            ))
        });
        return Err(VocabDesugarPassError::Native {
            provider: provider.name.clone(),
            message: message.into(),
            span: reported,
        });
    }
    Err(failure("malformed response: expected one Ok or Err result".into()))
}

/// Construct a provider-named failure, anchored by the caller at the vocabulary node.
fn native_error(provider: &Provider, message: String) -> VocabDesugarPassError {
    VocabDesugarPassError::Native {
        provider: provider.name.clone(),
        message,
        span: None,
    }
}

/// A retained process with independent pipe pumps so blocked writes and reads share one invocation deadline.
struct Worker {
    /// Child owned and reaped by this worker.
    child: Child,
    /// Input pump; only one invocation is outstanding per worker.
    input: Sender<Vec<u8>>,
    /// Framed output or a pipe/protocol failure.
    output: Receiver<Result<Value, String>>,
}

impl Worker {
    /// Start a host-native worker with no inherited environment, stdin protocol and drained private stderr.
    fn start(path: &Path) -> Result<Self, String> {
        let mut child = Command::new(path)
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("cannot start native worker: {error}"))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "worker input pipe is missing".to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "worker output pipe is missing".to_string())?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| "worker error pipe is missing".to_string())?;
        let (input, pending) = mpsc::channel();
        let (completed, output) = mpsc::sync_channel(1);
        let write_failure = completed.clone();
        std::thread::spawn(move || pump_input(stdin, pending, write_failure));
        std::thread::spawn(move || pump_output(stdout, completed));
        std::thread::spawn(move || {
            let _ = std::io::copy(&mut BufReader::new(stderr), &mut std::io::sink());
        });
        Ok(Self { child, input, output })
    }

    /// Exchange one module batch within ten seconds, distinguishing signal crashes, exits and malformed frames.
    fn invoke(&mut self, request: &Value) -> Result<Value, String> {
        let mut bytes = serde_json::to_vec(request).map_err(|error| error.to_string())?;
        bytes.push(b'\n');
        self.input
            .send(bytes)
            .map_err(|_| "native worker exited before reading its request".to_string())?;
        match self.output.recv_timeout(INVOCATION_TIMEOUT) {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(message)) => match self.child.try_wait().map_err(|error| error.to_string())? {
                Some(status) if !status.success() => Err(if status.code().is_none() {
                    format!("native desugarer crashed ({status})")
                } else {
                    format!("native desugarer exited with failure ({status})")
                }),
                _ => Err(message),
            },
            Err(RecvTimeoutError::Timeout) => Err("native desugarer exceeded its 10 s invocation time bound".into()),
            Err(RecvTimeoutError::Disconnected) => Err("native desugarer exited without a response".into()),
        }
    }
}

impl Drop for Worker {
    /// Stop and reap exactly the process owned by this worker, including after a deadline or malformed response.
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Write queued frames outside the deadline-owning thread so a worker that ignores stdin cannot block the compiler.
fn pump_input(mut pipe: impl Write, pending: Receiver<Vec<u8>>, completed: SyncSender<Result<Value, String>>) {
    for bytes in pending {
        if let Err(error) = pipe.write_all(&bytes).and_then(|_| pipe.flush()) {
            let _ = completed.send(Err(format!("native worker input failed: {error}")));
            break;
        }
    }
}

/// Read bounded newline-delimited response frames; stdout is exclusively the worker protocol.
fn pump_output(pipe: impl Read, completed: SyncSender<Result<Value, String>>) {
    let mut reader = BufReader::new(pipe);
    loop {
        let mut bytes = Vec::new();
        let result = Read::take(&mut reader, MAX_FRAME_BYTES + 1).read_until(b'\n', &mut bytes);
        let frame = match result {
            Ok(0) => Err("native desugarer exited without a response".into()),
            Ok(count) if u64::try_from(count).unwrap_or(u64::MAX) > MAX_FRAME_BYTES => {
                Err("malformed response: frame exceeds 8 MiB".into())
            }
            Ok(_) => serde_json::from_slice(&bytes).map_err(|error| format!("malformed response: {error}")),
            Err(error) => Err(format!("native worker output failed: {error}")),
        };
        let failed = frame.is_err();
        if completed.send(frame).is_err() || failed {
            break;
        }
    }
}
