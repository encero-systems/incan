//! Completed-output reuse for the pinned source-to-Body-IR driver, using the ordinary project Store and freshness path.

use std::path::{Path, PathBuf};

use crate::backend::selection::{BackendKind, finalize_receipt, select_backend};
use crate::build::bake::publish_prepared_project_output;
use crate::build::output_materialization::{
    materialize_completed_executable_output, verify_stored_project_output_native,
};
use crate::build::output_paths::{
    project_relative_entrypoint, project_root_for_completed_output, validated_project_output_relative_path,
};
use crate::build::output_selection::select_baked_project_output_with_source_authority;
use crate::build::source_authority::{
    baked_project_lock_dependencies_fingerprint, digest_baked_project_source_authority,
    ordinary_preparation_identity_digest,
};
use crate::build::{
    OVEN_PROJECT_OUTPUT_ARTIFACT_PATH, OvenBakeProjectTarget, OvenProjectOutputBakeFile, OvenProjectOutputBakeRequest,
};
use crate::error::{CliError, CliResult};
use crate::oven_store::open_default_oven_store;
use oven_model::manifest::{LOAF_MANIFEST_FILENAME, ProjectManifest};
use oven_store::{OvenGeneratedProjectRequest, digest_bytes, receipt_generated_project};

/// Exact caller-selected source, output and native resolution inputs for one pinned driver invocation.
#[derive(Clone, Copy)]
pub struct NativeSourceOutputRequest<'a> {
    /// Original entrypoint whose manifest-backed project owns the compiled source graph.
    pub source: &'a Path,
    /// Caller projection; optional reuse applies only inside the owning project.
    pub output: &'a Path,
    /// Crate identity passed to embedded rustc.
    pub crate_name: &'a str,
    /// Explicit logical package override; registry subjects and completed-output reuse retain these exact bytes.
    pub package_identity: Option<&'a str>,
    /// Canonical sysroot already verified against the driver's build-bound evidence.
    pub sysroot: &'a Path,
    /// Pinned compiler host target.
    pub target: &'a str,
    /// Complete compiler identity already checked by driver startup; receipt whitespace normalization applies.
    pub toolchain: &'a str,
    /// Exact ordinary native linker and admitted platform inputs used by embedded rustc.
    pub link_identity: &'a str,
    /// Ordered explicit crate aliases and files; actual bytes or absence participate in authority.
    pub externs: &'a [(String, PathBuf)],
    /// Ordered native search directories; their absence or immediate file members participate in authority.
    pub search_paths: &'a [PathBuf],
}

/// One source/current-invocation observation retained through lookup or successful native compilation.
///
/// The driver has one fixed optimization/debug contract, named `native-mir` in output intent. It is independent of
/// ordinary debug/release Rust-emission outputs. Standalone sources and projections outside the project still compile,
/// but decline this project-output optimization.
pub struct NativeSourceOutput<'a> {
    request: NativeSourceOutputRequest<'a>,
    project_root: PathBuf,
    source: PathBuf,
    relative_output: String,
    authority: String,
    preparation: String,
}

const PROFILE: &str = "native-mir";

impl<'a> NativeSourceOutput<'a> {
    /// Observe the existing source/dependency graph and exact native arguments before frontend work.
    pub fn observe(request: NativeSourceOutputRequest<'a>) -> CliResult<Option<Self>> {
        // Generated receipts normalize surrounding whitespace; lookup and invocation identity must agree with them.
        let request = NativeSourceOutputRequest {
            toolchain: request.toolchain.trim(),
            ..request
        };
        let source = request.source.canonicalize().map_err(failure)?;
        let Some(project_root) = project_root_for_completed_output(&source)? else {
            return Ok(None);
        };
        let output = std::path::absolute(request.output).map_err(failure)?;
        let Some(relative_output) = project_relative_entrypoint(&project_root, &output) else {
            return Ok(None);
        };
        validated_project_output_relative_path(&relative_output, "native driver projection")?;
        // Outputs outside reserved generated trees would become inputs to their own next invocation.
        if !relative_output.starts_with("target/") {
            return Ok(None);
        }
        let (authority, preparation) = observe_authority(&request, &project_root, &relative_output)?;
        Ok(Some(Self {
            request,
            project_root,
            source,
            relative_output,
            authority,
            preparation,
        }))
    }

    /// Select and restore the existing completed-output loaf before checking, lowering or native compilation.
    pub fn try_reuse(&self) -> CliResult<bool> {
        let _invocation = tracing::trace_span!(target: "incan::invocation", "incan_driver::reuse_native_source_output", input = %self.source.display()).entered();
        let store = open_default_oven_store()?;
        let Some(output) = select_baked_project_output_with_source_authority(
            &store,
            &self.project_root,
            &self.source,
            OvenBakeProjectTarget::Executable,
            PROFILE,
            &self.authority,
            Some((self.request.target, self.request.toolchain)),
        )?
        else {
            return Ok(false);
        };
        if output.payload.backend_receipt.executed_backend != BackendKind::BodyIr {
            return Ok(false);
        }
        verify_stored_project_output_native(&output)?;
        materialize_completed_executable_output(&self.project_root, &output, &output.payload.backend_receipt)?;
        tracing::trace!(target: "incan::invocation", decision = "reuse_native_source_output", input = %output.identity);
        Ok(true)
    }

    /// Re-observe inputs and publish a successful native result through the shared ordinary output publisher.
    pub fn publish(&self) -> CliResult<()> {
        let (authority, preparation) = observe_authority(&self.request, &self.project_root, &self.relative_output)?;
        if authority != self.authority || preparation != self.preparation {
            return Err(failure(
                "native source inputs changed during compilation; completed output was not published",
            ));
        }
        let declaration = ProjectManifest::load(&self.project_root.join(LOAF_MANIFEST_FILENAME)).map_err(failure)?;
        let project = declaration
            .project
            .as_ref()
            .ok_or_else(|| failure("native source project identity missing"))?;
        let receipt = receipt_generated_project(
            &OvenGeneratedProjectRequest::new(
                &self.project_root,
                project.name.as_deref().unwrap_or("native-source"),
                project.version.as_deref().unwrap_or("0.0.0"),
                self.request.target,
                self.request.toolchain,
                PROFILE,
                Vec::new(),
            )
            .with_generated_source("native-incan-entry", &self.source)
            .with_build_unit_input("native-source-invocation", &self.authority),
        )
        .map_err(failure)?;
        let native = super::file_freshness::digest_file(self.request.output).map_err(failure)?;
        let backend = finalize_receipt(
            &select_backend(BackendKind::BodyIr, &self.authority),
            native,
            incan_frontend::diagnostics::DIAGNOSTIC_SCHEMA_VERSION,
        )
        .map_err(failure)?;
        publish_prepared_project_output(
            OvenProjectOutputBakeRequest {
                project_root: &self.project_root,
                entrypoint: &self.source,
                target: OvenBakeProjectTarget::Executable,
                receipt: &receipt,
                plan_identity: self.authority.clone(),
                profile: PROFILE,
                source_authority_digest: &self.authority,
                lock_dependencies_fingerprint: baked_project_lock_dependencies_fingerprint(&self.project_root)?,
                files: vec![OvenProjectOutputBakeFile {
                    source_path: self.request.output.to_path_buf(),
                    caller_relative_path: self.relative_output.clone(),
                    output_relative_path: OVEN_PROJECT_OUTPUT_ARTIFACT_PATH.into(),
                }],
                inspection_authority: None,
                required_project_loafs: Vec::new(),
                package_loaf_store_relative_path: None,
                backend_receipt: backend,
                build_report: None,
            },
            &self.preparation,
        )?;
        Ok(())
    }
}

/// Bind source, preparation, logical package, argument order, explicit native bytes and the selected standard-library
/// directory.
fn observe_authority(
    request: &NativeSourceOutputRequest<'_>,
    root: &Path,
    output: &str,
) -> CliResult<(String, String)> {
    let preparation = ordinary_preparation_identity_digest()?;
    let source = digest_baked_project_source_authority(root)?;
    let externs = request
        .externs
        .iter()
        .map(|(name, path)| {
            let (original, present) = native_input_coordinate(path)?;
            let digest = present
                .then(|| super::file_freshness::digest_file(&original).map_err(failure))
                .transpose()?;
            Ok((name, path, original, digest))
        })
        .collect::<CliResult<Vec<_>>>()?;
    let standard = request.sysroot.join("lib/rustlib").join(request.target).join("lib");
    let self_contained = standard.join("self-contained");
    let mut directories = std::collections::BTreeMap::new();
    let mut resolved_search_paths = Vec::new();
    for path in request
        .externs
        .iter()
        .filter_map(|(_, path)| path.parent())
        .chain(request.search_paths.iter().map(PathBuf::as_path))
        .chain(std::iter::once(standard.as_path()))
        .chain(self_contained.is_dir().then_some(self_contained.as_path()))
    {
        let (original, present) = native_input_coordinate(path)?;
        resolved_search_paths.push(original.clone());
        if let std::collections::btree_map::Entry::Vacant(entry) = directories.entry(original) {
            let digest = present
                .then(|| super::file_freshness::digest_native_directory(entry.key()).map_err(failure))
                .transpose()?;
            entry.insert(digest);
        }
    }
    let bytes = serde_json::to_vec(&(
        "incan-native-source-output/3",
        source,
        &preparation,
        request.crate_name,
        request.package_identity,
        output,
        request.sysroot,
        request.target,
        request.toolchain,
        request.link_identity,
        externs,
        directories,
        resolved_search_paths,
    ))
    .map_err(failure)?;
    Ok((digest_bytes(&bytes), preparation))
}

/// Retain missing native arguments as absence: rustc tolerates unused missing externs and search directories.
///
/// A later appearance changes the observed identity. Existing inputs keep their canonical owner, and errors other
/// than absence remain failures rather than authorizing reuse with unknown bytes.
fn native_input_coordinate(path: &Path) -> CliResult<(PathBuf, bool)> {
    match path.canonicalize() {
        Ok(original) => Ok((original, true)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok((std::path::absolute(path).map_err(failure)?, false))
        }
        Err(error) => Err(failure(format!(
            "cannot observe native input {}: {error}",
            path.display()
        ))),
    }
}

/// Preserve the owning error's diagnostic at the shared compiler command boundary.
fn failure(error: impl std::fmt::Display) -> CliError {
    CliError::failure(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::{NativeSourceOutput, NativeSourceOutputRequest, failure};
    use crate::error::CliResult;
    use std::fs;
    use std::path::Path;

    /// A fresh output binds an explicit logical package independently of its otherwise identical native invocation.
    fn check_package_context_reuse(
        request: NativeSourceOutputRequest<'_>,
        root: &Path,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let output = root.join("target/package-native");
        fs::write(&output, b"explicit package native output")?;
        let packaged = NativeSourceOutputRequest {
            output: &output,
            package_identity: Some("explicit-package"),
            ..request
        };
        let observe = |request| -> CliResult<NativeSourceOutput<'_>> {
            NativeSourceOutput::observe(request)?
                .ok_or_else(|| failure("package output optimization unexpectedly declined"))
        };
        observe(packaged)?.publish()?;
        assert!(observe(packaged)?.try_reuse()?);
        assert!(
            !observe(NativeSourceOutputRequest {
                package_identity: None,
                ..packaged
            })?
            .try_reuse()?
        );
        assert!(
            !observe(NativeSourceOutputRequest {
                package_identity: Some("other-package"),
                ..packaged
            })?
            .try_reuse()?
        );
        assert_eq!(fs::read(output)?, b"explicit package native output");
        Ok(())
    }

    /// The common output path restores bytes and refuses changed sources, argument order, native files and sysroot.
    #[test]
    fn native_source_reuse_binds_sources_arguments_and_native_inputs() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let root = temp.path().canonicalize()?;
        fs::create_dir_all(root.join("src"))?;
        fs::create_dir_all(root.join("target/native"))?;
        fs::write(
            root.join("loaf.toml"),
            "[project]\nname='native_source_reuse_control'\nversion='0.1.0'\n",
        )?;
        let source = root.join("src/main.incn");
        let child = root.join("src/child.incn");
        fs::write(&source, "def main() -> None:\n    println(1)\n")?;
        fs::write(&child, "pub def value() -> int:\n    return 1\n")?;
        let output = root.join("target/native/program");
        fs::write(&output, b"first native output")?;
        let target = super::super::output_paths::native_project_output_target().ok_or("unsupported test host")?;
        let sysroot = root.join("target/sysroot");
        let standard = sysroot.join("lib/rustlib").join(target).join("lib");
        fs::create_dir_all(&standard)?;
        let standard_file = standard.join("std.rlib");
        fs::write(&standard_file, b"standard bytes")?;
        let search = root.join("target/search");
        fs::create_dir(&search)?;
        let native = search.join("native.rlib");
        fs::write(&native, b"native bytes")?;
        let externs = [("native".into(), native.clone())];
        let searches = [search.clone()];
        let request = NativeSourceOutputRequest {
            source: &source,
            output: &output,
            crate_name: "native_reuse",
            package_identity: None,
            sysroot: &sysroot,
            target,
            toolchain: "test pinned compiler\n",
            link_identity: "test pinned native link closure",
            externs: &externs,
            search_paths: &searches,
        };
        let observe = |request| -> CliResult<NativeSourceOutput<'_>> {
            NativeSourceOutput::observe(request)?
                .ok_or_else(|| failure("project output optimization unexpectedly declined"))
        };
        let first = observe(request)?;
        first.publish()?;
        let before = fs::metadata(&output)?;
        assert!(observe(request)?.try_reuse()?);
        let after = fs::metadata(&output)?;
        assert_eq!(before.modified()?, after.modified()?);
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            assert_eq!(before.ino(), after.ino());
        }
        fs::write(&output, b"tampered caller output")?;
        assert!(observe(request)?.try_reuse()?);
        assert_eq!(fs::read(&output)?, b"first native output");

        // Native dependency search is flat. Its archived source subtree is not a compiler input.
        fs::create_dir_all(search.join("source/src"))?;
        fs::write(search.join("source/src/archived.rs"), b"unused archived source")?;
        assert!(observe(request)?.try_reuse()?);

        let implicit = NativeSourceOutputRequest {
            search_paths: &[],
            ..request
        };
        observe(implicit)?.publish()?;
        let implicit_extra = search.join("implicit.rlib");
        fs::write(&implicit_extra, b"additional implicit native candidate")?;
        assert!(!observe(implicit)?.try_reuse()?);
        fs::remove_file(implicit_extra)?;
        assert!(observe(implicit)?.try_reuse()?);

        fs::write(&child, "pub def value() -> int:\n    return 2\n")?;
        assert!(!observe(request)?.try_reuse()?);
        assert!(first.publish().is_err());
        fs::write(&child, "pub def value() -> int:\n    return 1\n")?;
        assert!(observe(request)?.try_reuse()?);
        assert!(
            !observe(NativeSourceOutputRequest {
                link_identity: "changed pinned native link closure",
                ..request
            })?
            .try_reuse()?
        );
        assert!(
            !observe(NativeSourceOutputRequest {
                crate_name: "changed",
                ..request
            })?
            .try_reuse()?
        );
        check_package_context_reuse(request, &root)?;
        fs::write(&native, b"changed native bytes")?;
        assert!(!observe(request)?.try_reuse()?);
        fs::write(&native, b"native bytes")?;
        let extra = search.join("extra.rlib");
        fs::write(&extra, b"additional native candidate")?;
        assert!(!observe(request)?.try_reuse()?);
        fs::remove_file(extra)?;
        fs::write(&standard_file, b"changed standard bytes")?;
        assert!(!observe(request)?.try_reuse()?);
        fs::write(&standard_file, b"standard bytes")?;
        assert!(observe(request)?.try_reuse()?);

        // rustc accepts unused missing --extern and -L inputs; their absence still participates in intent.
        let missing = root.join("target/missing/native.rlib");
        let missing_directory = root.join("target/missing-search");
        let optional_externs = [("unused".into(), missing.clone())];
        let optional_searches = [missing_directory.clone()];
        let optional = NativeSourceOutputRequest {
            externs: &optional_externs,
            search_paths: &optional_searches,
            ..request
        };
        observe(optional)?.publish()?;
        assert!(observe(optional)?.try_reuse()?);
        fs::create_dir(&missing_directory)?;
        assert!(!observe(optional)?.try_reuse()?);
        fs::remove_dir(&missing_directory)?;
        assert!(observe(optional)?.try_reuse()?);
        fs::create_dir_all(missing.parent().ok_or("missing native parent")?)?;
        fs::write(missing, b"newly available extern")?;
        assert!(!observe(optional)?.try_reuse()?);
        Ok(())
    }
}
