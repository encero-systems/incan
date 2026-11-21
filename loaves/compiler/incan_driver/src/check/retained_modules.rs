//! Resident diagnostics for one editor project and one authenticated compiler context.

use std::collections::{BTreeSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;

use incan_frontend::diagnostics::CompileError;
use incan_frontend::parsed_module::ParsedModule;
use incan_frontend::typechecker::ModuleDependencyInterface;
use sha2::{Digest, Sha256};

use crate::modules::SourceModuleImports;
use crate::unit::UnitDescription;

const MAX_MODULES: usize = 512;
const MAX_SOURCE_BYTES: usize = 8 * 1024 * 1024;

/// The checked declaration interface and warnings the next diagnostic request actually consumes.
pub(crate) struct RetainedModule {
    pub(crate) interface: Arc<ModuleDependencyInterface>,
    pub(crate) warnings: Vec<CompileError>,
}

/// One resident editor view. It retains only successful checked interfaces, plus the source and import witnesses that choose them.
#[derive(Default)]
pub struct RetainedModuleChecks {
    context: Option<String>,
    rust_paths: BTreeSet<String>,
    names: Vec<(PathBuf, Vec<String>)>,
    imports: Vec<Vec<usize>>,
    readers: Vec<Vec<usize>>,
    digests: Vec<[u8; 32]>,
    entries: Vec<Option<RetainedModule>>,
    pending: Vec<(usize, RetainedModule)>,
}

impl RetainedModuleChecks {
    /// Authenticate the current graph and stop retaining the old records reached by changed module inputs.
    pub(crate) fn prepare(
        &mut self,
        unit: &UnitDescription,
        modules: &[ParsedModule],
        imports: &SourceModuleImports,
        rust_paths: BTreeSet<String>,
    ) -> bool {
        self.pending.clear();
        let bytes = modules
            .iter()
            .map(|module| module.source.len())
            .sum::<usize>();
        let Some(context) = unit
            .checked_context
            .as_ref()
            .filter(|_| modules.len() <= MAX_MODULES && bytes <= MAX_SOURCE_BYTES)
        else {
            *self = Self::default();
            return false;
        };
        let same_graph = self.names.len() == modules.len()
            && self
                .names
                .iter()
                .zip(modules)
                .all(|((file, path), module)| {
                    file == &module.file_path && path == &module.path_segments
                })
            && self.imports == imports.direct_edges();
        let digests: Vec<[u8; 32]> = modules
            .iter()
            .map(|module| Sha256::digest(module.source.as_bytes()).into())
            .collect();
        if self.context.as_ref() != Some(context) || self.rust_paths != rust_paths || !same_graph {
            self.context = Some(context.clone());
            self.rust_paths = rust_paths;
            self.names = modules
                .iter()
                .map(|module| (module.file_path.clone(), module.path_segments.clone()))
                .collect();
            self.imports = imports.direct_edges().to_vec();
            self.readers = vec![Vec::new(); modules.len()];
            for (reader, providers) in self.imports.iter().enumerate() {
                for &provider in providers {
                    self.readers[provider].push(reader);
                }
            }
            self.entries = (0..modules.len()).map(|_| None).collect();
        } else {
            let changed = self
                .digests
                .iter()
                .zip(&digests)
                .map(|(was, now)| was != now)
                .collect();
            self.invalidate(changed);
        }
        self.digests = digests;
        tracing::trace!(target: "incan::invocation", modules = modules.len(), source_bytes = bytes,
            retained = self.entries.iter().flatten().count(), "editor checked-module selection");
        true
    }

    /// Follow reverse edges once from each changed module, including cycles.
    fn invalidate(&mut self, mut changed: Vec<bool>) {
        let mut queue: VecDeque<usize> = changed
            .iter()
            .enumerate()
            .filter_map(|(index, &dirty)| dirty.then_some(index))
            .collect();
        while let Some(index) = queue.pop_front() {
            self.entries[index] = None;
            for &reader in &self.readers[index] {
                if !std::mem::replace(&mut changed[reader], true) {
                    queue.push_back(reader);
                }
            }
        }
    }

    /// Borrow the successful interface and warnings retained under the current module witness.
    pub(crate) fn get(&self, index: usize) -> Option<&RetainedModule> {
        self.entries[index].as_ref()
    }

    /// Stage one newly produced interface until the complete request succeeds.
    pub(crate) fn stage(
        &mut self,
        index: usize,
        interface: &Arc<ModuleDependencyInterface>,
        warnings: Vec<CompileError>,
    ) {
        self.pending.push((
            index,
            RetainedModule {
                interface: Arc::clone(interface),
                warnings,
            },
        ));
    }

    /// Keep successes from a complete checked unit. After a refusal earlier valid, unaffected entries remain eligible.
    pub(crate) fn settle(&mut self, passed: bool) {
        for (index, value) in self.pending.drain(..) {
            if passed {
                self.entries[index] = Some(value);
            }
        }
    }
}
