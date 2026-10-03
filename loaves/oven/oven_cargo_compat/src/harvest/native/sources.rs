//! Portable adoption of native sources, include trees, and object arguments.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use oven_model::manifest::{
    RustFactArgument, RustFactArtifact, RustFactArtifactKind, RustFactArtifactMember, RustFactLinkObject,
};

use super::super::super::{OvenLegacyNativeInvocation, digest_bytes};
use super::argv::{TypedArgument, classify_argument};
use super::authority::{CompilerAuthority, portable_beneath};
use super::environment::{CommonCompileEnvironment, resolve_observed_path};

/// Adopted crate sources, include trees, object declarations, and compiler-owner paths.
pub(super) struct NativeSources<'a> {
    environment: &'a CommonCompileEnvironment,
    authority: &'a CompilerAuthority,
    sources: BTreeMap<String, RustFactArtifact>,
    objects: Vec<RustFactLinkObject>,
    object_paths: BTreeMap<std::path::PathBuf, String>,
    owner_paths: BTreeSet<String>,
}

impl<'a> NativeSources<'a> {
    /// Start one source adoption under a validated compile environment and compiler authority.
    pub(super) fn new(environment: &'a CommonCompileEnvironment, authority: &'a CompilerAuthority) -> Self {
        Self {
            environment,
            authority,
            sources: BTreeMap::new(),
            objects: Vec::new(),
            object_paths: BTreeMap::new(),
            owner_paths: BTreeSet::from([
                authority.executable_path.clone(),
                authority.resource_path.clone(),
                authority.sysroot_path.clone(),
            ]),
        }
    }

    /// Adopt one plain compile into its portable source, argv, and object declarations.
    pub(super) fn adopt_compile(
        &mut self,
        invocation: &OvenLegacyNativeInvocation,
        source: &Path,
        output: &Path,
    ) -> Result<(), String> {
        if source.starts_with(&self.environment.out_dir) {
            return Err("compile source is generated inside OUT_DIR and cannot be adopted".to_string());
        }
        let source_relative = portable_beneath(&self.environment.manifest_dir, source, "compile source")?;
        let source_name = logical_artifact_name("source", &source_relative);
        insert_source(
            &mut self.sources,
            source_name.clone(),
            RustFactArtifact {
                name: source_name.clone(),
                kind: RustFactArtifactKind::File,
                path: source_relative,
                digest: digest_bytes(
                    &fs::read(source).map_err(|error| format!("cannot read compile source: {error}"))?,
                ),
                members: Vec::new(),
            },
        )?;
        let object_name = super::stable_object_name(output)?;
        if self
            .object_paths
            .insert(output.to_path_buf(), object_name.clone())
            .is_some()
            || self.objects.iter().any(|object| object.name == object_name)
        {
            return Err(format!(
                "native object name `{object_name}` collides after stripping the cc hash prefix"
            ));
        }

        let mut arguments = self.implicit_authority_arguments(invocation);
        let mut index = 0usize;
        while index < invocation.arguments.len() {
            let classified = classify_argument(&invocation.arguments, index)?;
            match classified.argument {
                TypedArgument::Source(_) => {
                    arguments.push(RustFactArgument::Literal {
                        literal: "-c".to_string(),
                    });
                    arguments.push(RustFactArgument::Input {
                        input: source_name.clone(),
                    });
                }
                TypedArgument::Output(_) => {
                    arguments.push(RustFactArgument::Literal {
                        literal: "-o".to_string(),
                    });
                    arguments.push(RustFactArgument::Output {
                        output: object_name.clone(),
                    });
                }
                TypedArgument::Path { flag, value } => {
                    if let Some(flag) = flag {
                        arguments.push(RustFactArgument::Literal {
                            literal: flag.to_string(),
                        });
                    }
                    arguments.push(path_argument(
                        value,
                        invocation,
                        &self.environment.manifest_dir,
                        &self.environment.out_dir,
                        &self.authority.owner_root,
                        &mut self.sources,
                        &mut self.owner_paths,
                    )?);
                }
                TypedArgument::Literal(literal) => {
                    arguments.push(RustFactArgument::Literal {
                        literal: literal.to_string(),
                    });
                }
            }
            index += classified.consumed;
        }
        self.objects.push(RustFactLinkObject {
            name: object_name,
            language: super::source_language(source)?,
            arguments,
        });
        Ok(())
    }

    /// Complete adoption and return objects, declared inputs, and owner members.
    pub(super) fn finish(self) -> (Vec<RustFactLinkObject>, Vec<RustFactArtifact>, BTreeSet<String>) {
        (self.objects, self.sources.into_values().collect(), self.owner_paths)
    }

    /// Add explicit sysroot and resource arguments when the captured compiler relied on lookup defaults.
    fn implicit_authority_arguments(&self, invocation: &OvenLegacyNativeInvocation) -> Vec<RustFactArgument> {
        let has_resource_directory = invocation
            .arguments
            .iter()
            .any(|argument| argument == "-resource-dir" || argument.starts_with("-resource-dir="));
        let has_sysroot = invocation.arguments.iter().any(|argument| {
            matches!(argument.as_str(), "-isysroot" | "--sysroot") || argument.starts_with("--sysroot=")
        });
        let mut arguments = Vec::new();
        if !has_sysroot {
            arguments.push(RustFactArgument::Literal {
                literal: "-isysroot".to_string(),
            });
            arguments.push(RustFactArgument::Owner {
                owner: self.authority.sysroot_path.clone(),
            });
        }
        if !has_resource_directory {
            arguments.push(RustFactArgument::Literal {
                literal: "-resource-dir".to_string(),
            });
            arguments.push(RustFactArgument::Owner {
                owner: self.authority.resource_path.clone(),
            });
        }
        arguments
    }
}

/// Convert one path-taking compiler argument into crate input or compiler-owner authority.
pub(super) fn path_argument(
    value: &str,
    invocation: &OvenLegacyNativeInvocation,
    manifest_dir: &Path,
    out_dir: &Path,
    owner_root: &Path,
    sources: &mut BTreeMap<String, RustFactArtifact>,
    owner_paths: &mut BTreeSet<String>,
) -> Result<RustFactArgument, String> {
    let path = resolve_observed_path(&invocation.working_directory, value)?;
    if path.starts_with(out_dir) {
        return Err("compiler input is generated inside OUT_DIR and cannot be adopted".to_string());
    }
    if path.starts_with(manifest_dir) {
        let relative = if path == manifest_dir {
            ".".to_string()
        } else {
            portable_beneath(manifest_dir, &path, "crate input")?
        };
        let name = logical_artifact_name("include", &relative);
        let artifact = if path.is_dir() {
            tree_artifact(&name, &relative, &path)?
        } else {
            RustFactArtifact {
                name: name.clone(),
                kind: RustFactArtifactKind::File,
                path: relative,
                digest: digest_bytes(&fs::read(&path).map_err(|error| format!("cannot read crate input: {error}"))?),
                members: Vec::new(),
            }
        };
        insert_source(sources, name.clone(), artifact)?;
        return Ok(RustFactArgument::Input { input: name });
    }
    if path.starts_with(owner_root) {
        let relative = portable_beneath(owner_root, &path, "compiler owner input")?;
        owner_paths.insert(relative.clone());
        return Ok(RustFactArgument::Owner { owner: relative });
    }
    Err("absolute compiler input is outside the crate and compiler owner".to_string())
}

/// Insert one source while refusing logical-name collisions with different declarations.
fn insert_source(
    sources: &mut BTreeMap<String, RustFactArtifact>,
    name: String,
    artifact: RustFactArtifact,
) -> Result<(), String> {
    if let Some(previous) = sources.insert(name.clone(), artifact.clone())
        && previous != artifact
    {
        return Err(format!("source name `{name}` collides after portable normalization"));
    }
    Ok(())
}

/// Inventory one crate-relative include tree with the executor's canonical tree digest.
fn tree_artifact(name: &str, relative: &str, root: &Path) -> Result<RustFactArtifact, String> {
    /// Visit regular descendants while retaining paths relative to the declared tree root.
    fn visit(root: &Path, directory: &Path, members: &mut Vec<RustFactArtifactMember>) -> Result<(), String> {
        let mut entries = fs::read_dir(directory)
            .map_err(|error| format!("cannot read include tree: {error}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("cannot read include tree: {error}"))?;
        entries.sort_by_key(std::fs::DirEntry::file_name);
        for entry in entries {
            let path = entry.path();
            let kind = entry
                .file_type()
                .map_err(|error| format!("cannot inspect include tree entry: {error}"))?;
            if kind.is_symlink() {
                return Err("crate include tree contains a symlink".to_string());
            }
            if kind.is_dir() {
                visit(root, &path, members)?;
            } else if kind.is_file() {
                members.push(RustFactArtifactMember {
                    path: portable_beneath(root, &path, "include tree member")?,
                    digest: digest_bytes(
                        &fs::read(&path).map_err(|error| format!("cannot read include tree member: {error}"))?,
                    ),
                });
            } else {
                return Err("crate include tree contains a special file".to_string());
            }
        }
        Ok(())
    }
    let mut members = Vec::new();
    visit(root, root, &mut members)?;
    members.sort_by(|left, right| left.path.cmp(&right.path));
    let bytes = serde_json::to_vec(&("incan.oven.publisher-artifact-tree/1", &members))
        .map_err(|error| format!("cannot digest include tree: {error}"))?;
    Ok(RustFactArtifact {
        name: name.to_string(),
        kind: RustFactArtifactKind::Tree,
        path: relative.to_string(),
        digest: digest_bytes(&bytes),
        members,
    })
}

/// Derive a conservative logical artifact name from a portable path.
pub(super) fn logical_artifact_name(prefix: &str, path: &str) -> String {
    format!(
        "{prefix}-{}",
        path.bytes()
            .map(|byte| {
                if byte.is_ascii_alphanumeric() {
                    char::from(byte)
                } else {
                    '-'
                }
            })
            .collect::<String>()
    )
}
