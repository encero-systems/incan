//! Pass rustc's actual regular codegen results to the existing development link.

use super::stable_link::{CompletedObject, CompletedObjects, PreviousObject};
use rustc_codegen_ssa::traits::CodegenBackend;
use rustc_codegen_ssa::{CompiledModules, CrateInfo, ModuleKind, TargetConfig};
use rustc_interface::interface::Config;
use rustc_metadata::EncodedMetadata;
use rustc_metadata::creader::MetadataLoaderDyn;
use rustc_middle::dep_graph::{WorkProductId, WorkProductMap};
use rustc_middle::ty::TyCtxt;
use rustc_middle::util::Providers;
use rustc_session::config::{CrateType, OutputFilenames, PrintRequest};
use rustc_session::{EarlyDiagCtxt, Session};
use rustc_span::Symbol;
use std::any::Any;
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::os::unix::fs::MetadataExt;
use std::sync::Mutex;

/// Keep the selected backend and add the regular-object result to this invocation's private handoff.
pub(super) fn install(config: &mut Config, handoff: File, previous: BTreeMap<String, PreviousObject>) {
    let selected = config.make_codegen_backend.take();
    config.make_codegen_backend = Some(Box::new(move |session| {
        let backend = match selected {
            Some(make_backend) => make_backend(session),
            None => rustc_interface::util::get_codegen_backend(
                &EarlyDiagCtxt::new(session.opts.error_format),
                &session.opts.sysroot,
                session.opts.unstable_opts.codegen_backend.as_deref(),
                &session.target,
            ),
        };
        Box::new(CompletedCodegen {
            backend,
            handoff: Mutex::new(Some(PendingLink {
                file: handoff,
                entries: Vec::new(),
                previous,
            })),
        })
    }));
}

/// Delegate the entire backend contract, observing only the successful join result.
struct CompletedCodegen {
    backend: Box<dyn CodegenBackend>,
    handoff: Mutex<Option<PendingLink>>,
}

struct PendingLink {
    file: File,
    entries: Vec<CompletedObject>,
    previous: BTreeMap<String, PreviousObject>,
}

impl CodegenBackend for CompletedCodegen {
    fn name(&self) -> &'static str {
        self.backend.name()
    }

    fn init(&self, session: &Session) {
        self.backend.init(session);
    }

    fn print(&self, request: &PrintRequest, out: &mut String, session: &Session) {
        self.backend.print(request, out, session);
    }

    fn target_config(&self, session: &Session) -> TargetConfig {
        self.backend.target_config(session)
    }

    fn supported_crate_types(&self, session: &Session) -> Vec<CrateType> {
        self.backend.supported_crate_types(session)
    }

    fn print_passes(&self) {
        self.backend.print_passes();
    }

    fn print_version(&self) {
        self.backend.print_version();
    }

    fn replaced_intrinsics(&self) -> Vec<Symbol> {
        self.backend.replaced_intrinsics()
    }

    fn fallback_intrinsics(&self) -> Vec<Symbol> {
        self.backend.fallback_intrinsics()
    }

    fn thin_lto_supported(&self) -> bool {
        self.backend.thin_lto_supported()
    }

    fn has_zstd(&self) -> bool {
        self.backend.has_zstd()
    }

    fn has_mnemonic(&self, session: &Session, mnemonic: &str) -> bool {
        self.backend.has_mnemonic(session, mnemonic)
    }

    fn metadata_loader(&self) -> Box<MetadataLoaderDyn> {
        self.backend.metadata_loader()
    }

    fn provide(&self, providers: &mut Providers) {
        self.backend.provide(providers);
    }

    fn target_cpu(&self, session: &Session) -> String {
        self.backend.target_cpu(session)
    }

    fn codegen_crate<'tcx>(&self, tcx: TyCtxt<'tcx>) -> Box<dyn Any> {
        self.backend.codegen_crate(tcx)
    }

    fn join_codegen(
        &self,
        ongoing: Box<dyn Any>,
        session: &Session,
        outputs: &OutputFilenames,
        crate_info: &CrateInfo,
    ) -> (CompiledModules, WorkProductMap) {
        let (modules, products) = self.backend.join_codegen(ongoing, session, outputs, crate_info);
        if let Ok(mut output) = self.handoff.lock()
            && let Some(pending) = output.as_mut()
        {
            pending.entries = completed_objects(&modules, &products);
        }
        (modules, products)
    }

    fn print_pass_timings(&self) {
        self.backend.print_pass_timings();
    }

    fn print_statistics(&self) {
        self.backend.print_statistics();
    }

    fn print_statistics_json(&self) -> String {
        self.backend.print_statistics_json()
    }

    fn link(
        &self,
        session: &Session,
        modules: CompiledModules,
        crate_info: CrateInfo,
        metadata: EncodedMetadata,
        outputs: &OutputFilenames,
    ) {
        if let Ok(mut output) = self.handoff.lock()
            && let Some(pending) = output.take()
            && let Some(finalized) = session.incr_comp_session_dir_opt()
        {
            let completed = CompletedObjects {
                finalized: finalized.to_path_buf(),
                entries: pending.entries,
                previous: pending.previous,
            };
            let mut writer = BufWriter::new(pending.file);
            // The ordinary linker treats an incomplete optional handoff as an ordinary cache miss.
            if serde_json::to_writer(&mut writer, &completed).is_ok() {
                let _ = writer.flush();
            }
        }
        self.backend.link(session, modules, crate_info, metadata, outputs);
    }
}

/// Resolve each cache transfer through the backend's saved regular "o" slot, never through an assembly or dwo path.
fn completed_objects(modules: &CompiledModules, products: &WorkProductMap) -> Vec<CompletedObject> {
    modules
        .modules
        .iter()
        .filter_map(|module| {
            if module.kind != ModuleKind::Regular {
                return None;
            }
            let path = module.object.as_ref()?;
            let saved = products
                .get(&WorkProductId::from_cgu_name(&module.name))
                .and_then(|product| product.saved_files.get("o"));
            let loaded = saved.and_then(|slot| {
                module
                    .links_from_incr_cache
                    .iter()
                    .find(|source| source.file_name().is_some_and(|name| name == slot.as_str()))
            });
            let observed = std::fs::symlink_metadata(path).ok()?;
            if !observed.is_file() {
                return None;
            }
            Some(CompletedObject {
                group: module.name.clone(),
                source: path.clone(),
                source_identity: super::stable_link::file_identity(&observed),
                cached_source: loaded.cloned(),
                same_cache_file: loaded.is_some_and(|slot| {
                    std::fs::symlink_metadata(slot).is_ok_and(|identity| {
                        identity.is_file() && identity.dev() == observed.dev() && identity.ino() == observed.ino()
                    })
                }),
                saved_name: saved.cloned()?,
                device: observed.dev(),
                inode: observed.ino(),
                length: observed.len(),
                modified: (observed.mtime(), observed.mtime_nsec()),
            })
        })
        .collect()
}
