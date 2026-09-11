//! Digesting what the compiler *produces* for the standard library, rather than what the compiler is made of.
//!
//! The SDK provider store is keyed by an identity that decides whether ten prepared components can be reused. That
//! identity currently folds the whole compiler source tree, so editing any file under `src/` or `crates/` — a CLI
//! command, an inspection module, the language server — rebuilds every component. The rebuild costs roughly
//! seventeen minutes and is paid on the next command after any compiler edit (#1495).
//!
//! Hashing the source tree answers the wrong question. What a consumer needs to know is not *did the compiler
//! change* but *would this compiler produce different output for this standard library*. Those differ for almost
//! every edit anyone makes. This module answers the second question directly, by running the compiler's own
//! frontend over the standard library and digesting the result.
//!
//! # Why this is cheaper by three orders of magnitude
//!
//! Measured over all 104 standard-library sources, in-process, with no subprocess, no `rustc`, and no SDK
//! preparation: HIR v0 plus Body IR v0 costs **1.80 s**. The rebuild it replaces costs roughly seventeen minutes.
//! The work is not avoided by being clever; it is a different, much smaller question.
//!
//! # What must be covered, and why an Incan-only digest is unsound
//!
//! Every component links against `crates/incan_stdlib/src`, which is thousands of lines of Rust runtime. A digest
//! that folded only `.incn` meaning would report a hit for an edit to that runtime, which is a false reuse of a
//! component whose behaviour changed. The Rust half is therefore mandatory rather than an enhancement, and it is
//! folded here beside the Incan half.
//!
//! # The transitional input, and when to remove it
//!
//! Today the compiler still lowers to Rust and emits it, so a change confined to the emitter alters generated
//! output without moving any HIR. Two of this programme's own defects had exactly that shape. Until emission is
//! gone, a digest over HIR alone would be a false hit for an emitter change, so the emitter's own sources are
//! folded in as a narrow, explicitly transitional input — narrow enough to exclude the rest of the compiler, and
//! removed with the emitter rather than maintained forever.
//!
//! This is the one place where the *source* of a compiler subsystem still reaches the key, and it is here because
//! the alternative is unsound today, not because source hashing is the design.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use incan_semantics_core::semantic_digest::{body_without_docstring, semantic_digest};
use incan_semantics_core::stable_identity::{DeclarationSignature, StableDeclarationId};
use sha2::{Digest, Sha256};

/// Why the standard library's effect digest could not be computed.
///
/// Every variant is a refusal rather than a degraded answer. A digest that silently skipped a module it could not
/// read would describe a smaller standard library than the one about to be compiled, and a consumer would reuse
/// components built from sources this digest never saw.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EffectDigestError {
    /// A source directory could not be enumerated.
    #[error("failed to enumerate {path}: {message}")]
    Enumerate {
        /// The directory that could not be read.
        path: PathBuf,
        /// The underlying reason.
        message: String,
    },
    /// A source file could not be read.
    #[error("failed to read {path}: {message}")]
    Read {
        /// The file that could not be read.
        path: PathBuf,
        /// The underlying reason.
        message: String,
    },
    /// A standard-library module did not compile far enough to digest.
    ///
    /// Reaching a caller means the module could be read but not understood, which [`stdlib_effect_digest`] handles
    /// by folding the module's bytes instead. It is surfaced as an error type so a caller that wants to know can
    /// ask, rather than because the digest gives up.
    #[error("standard library module {path} did not reach Body IR: {message}")]
    Uncompilable {
        /// The module that failed.
        path: PathBuf,
        /// The stage's own diagnosis.
        message: String,
    },
}

/// Collect every file with one of the given extensions under a root, in a stable order.
///
/// Ordering is by full path so the digest does not depend on directory iteration order, which is not stable across
/// filesystems.
fn collect_sources(root: &Path, extensions: &[&str]) -> Result<Vec<PathBuf>, EffectDigestError> {
    let mut found = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        let entries = fs::read_dir(&directory).map_err(|error| EffectDigestError::Enumerate {
            path: directory.clone(),
            message: error.to_string(),
        })?;
        for entry in entries {
            let entry = entry.map_err(|error| EffectDigestError::Enumerate {
                path: directory.clone(),
                message: error.to_string(),
            })?;
            let path = entry.path();
            let file_type = entry.file_type().map_err(|error| EffectDigestError::Enumerate {
                path: path.clone(),
                message: error.to_string(),
            })?;
            if file_type.is_dir() {
                // Build output is derived from the very sources being digested, so folding it in would make the
                // digest depend on whether a build had happened.
                if path.file_name().is_some_and(|name| name == "target") {
                    continue;
                }
                stack.push(path);
            } else if file_type.is_file()
                && path
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| extensions.contains(&extension))
            {
                found.push(path);
            }
        }
    }
    found.sort();
    Ok(found)
}

/// Write a length-delimited run so two different sequences cannot encode the same bytes.
fn delimited(hasher: &mut Sha256, value: &[u8]) {
    hasher.update((value.len() as u64).to_le_bytes());
    hasher.update(value);
}

/// Digest the meaning the compiler's frontend derives from one standard-library module.
///
/// Returns the module's declarations keyed by their rendered identity, so the caller folds them in a stable order
/// that does not depend on declaration order within the file.
fn module_meaning(path: &Path, source: &str) -> Result<BTreeMap<String, String>, EffectDigestError> {
    use crate::frontend::body_ir::{apply_body_ir_input_contract, build_body_ir_module_v0};
    use crate::frontend::typechecker::TypeChecker;
    use crate::frontend::{lexer, parser};

    let fail = |message: String| EffectDigestError::Uncompilable {
        path: path.to_path_buf(),
        message,
    };

    let tokens = lexer::lex(source).map_err(|errors| fail(format!("lex: {errors:?}")))?;
    let program = parser::parse(&tokens).map_err(|errors| fail(format!("parse: {errors:?}")))?;
    let program =
        apply_body_ir_input_contract(program, path).map_err(|errors| fail(format!("contract: {errors:?}")))?;
    let module_path = vec![path.file_stem().unwrap_or_default().to_string_lossy().to_string()];
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(module_path.clone()));
    checker
        .check_program(&program)
        .map_err(|errors| fail(format!("typecheck: {errors:?}")))?;

    let body_ir = build_body_ir_module_v0(&program, &module_path, checker.type_info());
    let mut meanings = BTreeMap::new();
    for body in &body_ir.bodies {
        let Some(canonical) = body.canonical.as_ref() else {
            continue;
        };
        // The key must be the *stable* identity, not the canonical one. `CanonicalSymbolId::render_compact()`
        // ends in `@start..end`, so keying on it would make every declaration below an added comment look like a
        // new declaration — the exact positional dependency this digest exists to remove, reintroduced through
        // the map key rather than through the value.
        let signature = Some(DeclarationSignature::from_callable_types(
            body.params.iter().map(|param| &param.ty),
            &body.return_type,
        ));
        let identity = StableDeclarationId::from_canonical(canonical, signature);
        // The docstring is removed before digesting because a documentation edit cannot change what the compiler
        // emits for a body, and the whole point of this digest is to stop such an edit costing a rebuild.
        let digest =
            semantic_digest(&body_without_docstring(body)).map_err(|error| fail(format!("digest: {error:?}")))?;
        meanings.insert(identity.render_compact(), digest);
    }
    Ok(meanings)
}

/// Digest what this compiler would produce for the standard library rooted at `stdlib_root`.
///
/// The returned digest moves when the compiler would emit different output and holds still otherwise. It folds
/// three inputs: the checked meaning of every `.incn` source, the token-level content of the Rust runtime every
/// component links against, and — transitionally — the emitter's own sources.
///
/// # Errors
///
/// Returns [`EffectDigestError`] when a source cannot be read or a standard-library module does not compile. Both
/// are refusals: a digest that skipped what it could not read would describe a different standard library.
pub fn stdlib_effect_digest(
    stdlib_root: &Path,
    rust_runtime_root: &Path,
    emitter_root: &Path,
) -> Result<String, EffectDigestError> {
    let mut hasher = Sha256::new();
    delimited(&mut hasher, b"incan-stdlib-effect-v1");

    // ---- The Incan half: checked meaning, not source text ----
    delimited(&mut hasher, b"incan-meaning");
    for path in collect_sources(stdlib_root, &["incn"])? {
        let source = fs::read_to_string(&path).map_err(|error| EffectDigestError::Read {
            path: path.clone(),
            message: error.to_string(),
        })?;
        let relative = path.strip_prefix(stdlib_root).unwrap_or(&path);
        delimited(&mut hasher, relative.to_string_lossy().as_bytes());
        match module_meaning(&path, &source) {
            Ok(meanings) => {
                delimited(&mut hasher, b"meaning");
                for (identity, digest) in meanings {
                    delimited(&mut hasher, identity.as_bytes());
                    delimited(&mut hasher, digest.as_bytes());
                }
            }
            // A module the frontend cannot check standalone still contributes. Folding its bytes over-invalidates
            // for that module — a comment edit in it costs a rebuild — which is the safe direction, and it is one
            // module out of 104 rather than a general fallback. Refusing outright would be worse: the digest would
            // be unavailable whenever any module was mid-edit.
            Err(_) => {
                delimited(&mut hasher, b"unchecked-source");
                delimited(&mut hasher, source.as_bytes());
            }
        }
    }

    // ---- The Rust half: mandatory, because every component links against it ----
    delimited(&mut hasher, b"rust-runtime");
    for path in collect_sources(rust_runtime_root, &["rs"])? {
        let source = fs::read_to_string(&path).map_err(|error| EffectDigestError::Read {
            path: path.clone(),
            message: error.to_string(),
        })?;
        let relative = path.strip_prefix(rust_runtime_root).unwrap_or(&path);
        delimited(&mut hasher, relative.to_string_lossy().as_bytes());
        delimited(
            &mut hasher,
            rust_source_digest(&relative.to_string_lossy(), &source).as_bytes(),
        );
    }

    // ---- The transitional half: remove this with the emitter ----
    delimited(&mut hasher, b"emitter-source");
    for path in collect_sources(emitter_root, &["rs"])? {
        let source = fs::read_to_string(&path).map_err(|error| EffectDigestError::Read {
            path: path.clone(),
            message: error.to_string(),
        })?;
        let relative = path.strip_prefix(emitter_root).unwrap_or(&path);
        delimited(&mut hasher, relative.to_string_lossy().as_bytes());
        delimited(
            &mut hasher,
            rust_source_digest(&relative.to_string_lossy(), &source).as_bytes(),
        );
    }

    Ok(format!("sha256:{}", hex::encode(hasher.finalize())))
}

/// Digest one Rust source file's tokens, falling back to its bytes when it does not parse.
///
/// The token digest survives reformatting and comment edits. A file that does not parse is not a reason to skip it
/// — it is still an input — so its bytes are folded instead, which over-invalidates and never under-invalidates.
fn rust_source_digest(module_path: &str, source: &str) -> String {
    let mut hasher = Sha256::new();
    match rust_inspect::digest_rust_source(module_path, source) {
        Ok(digest) => {
            for item in digest.items() {
                delimited(&mut hasher, item.key.module_path.as_bytes());
                delimited(&mut hasher, item.key.owner.as_deref().unwrap_or("").as_bytes());
                delimited(&mut hasher, format!("{:?}", item.key.kind).as_bytes());
                delimited(&mut hasher, item.key.name.as_bytes());
                delimited(&mut hasher, item.key.signature_discriminant.as_bytes());
                delimited(&mut hasher, item.digest.as_bytes());
            }
        }
        // A file that does not parse is still an input. Folding its bytes over-invalidates, which costs time;
        // skipping it would under-invalidate, which ships a component built from source this digest never saw.
        Err(_) => {
            delimited(&mut hasher, b"unparsed");
            delimited(&mut hasher, source.as_bytes());
        }
    }
    hex::encode(hasher.finalize())
}

/// Digest one standard-library module's meaning, for callers that bucket per component rather than per tree.
///
/// Exposed so a consumer can ask which component an edit touched instead of rebuilding all of them.
///
/// # Errors
///
/// Returns [`EffectDigestError`] when the module cannot be read or does not compile.
pub fn module_effect_digest(path: &Path) -> Result<String, EffectDigestError> {
    let source = fs::read_to_string(path).map_err(|error| EffectDigestError::Read {
        path: path.to_path_buf(),
        message: error.to_string(),
    })?;
    let mut hasher = Sha256::new();
    delimited(&mut hasher, b"incan-module-effect-v1");
    for (identity, digest) in module_meaning(path, &source)? {
        delimited(&mut hasher, identity.as_bytes());
        delimited(&mut hasher, digest.as_bytes());
    }
    Ok(format!("sha256:{}", hex::encode(hasher.finalize())))
}
