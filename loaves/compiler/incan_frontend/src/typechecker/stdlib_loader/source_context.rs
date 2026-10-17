//! Command-owned source inputs for the standard-library metadata cache (#1337/#1698).
//!
//! Legacy readers retain their existing ambient discovery. Genuine source publication retains its exact capability
//! and member owners and checks freshness at checker/cache handoffs. A standard source publisher retains its genuine
//! selected standard dependencies for inherited trait defaults. Ordinary admitted consumers stay metadata-only for
//! checking and Body IR; only their Rust-emission lowering, which expands inherited standard trait defaults and
//! source-defined constructor hooks into every adopter, retains the admitted standard dependencies' sources through
//! those dependencies' namespace issuers. Nothing is ever read ambiently.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use oven_model::toolchain_layout::CompilerOwnedSourceMember;

use crate::provider::ProviderPlan;
use crate::provider::source_policy::TrustedStandardSourcePublication;

/// Explicit source-read policy: an absent own-source capability never implies ambient legacy permission.
#[derive(Clone, Debug, Default)]
enum StdlibSourceSelection {
    #[default]
    Legacy,
    OrdinaryCheckedOnly,
    StandardSource {
        own: Arc<TrustedStandardSourcePublication>,
        dependencies: Vec<Arc<TrustedStandardSourcePublication>>,
        plan_identity: u64,
    },
    /// An ordinary admitted consumer: no own publication, only the retained sources of its admitted standard
    /// dependencies, so lowering can expand their trait defaults and constructor hooks into the consumer's adopters.
    AdmittedDependencies {
        dependencies: Vec<Arc<TrustedStandardSourcePublication>>,
        plan_identity: u64,
    },
}

impl StdlibSourceSelection {
    /// Every retained publication readable under this selection, the own publication first. The ambient and
    /// checked-only selections retain nothing.
    fn retained_sources(&self) -> Vec<&Arc<TrustedStandardSourcePublication>> {
        match self {
            Self::Legacy | Self::OrdinaryCheckedOnly => Vec::new(),
            Self::StandardSource { own, dependencies, .. } => std::iter::once(own).chain(dependencies).collect(),
            Self::AdmittedDependencies { dependencies, .. } => dependencies.iter().collect(),
        }
    }
}

/// Original member owners and refusal state shared by every cache clone within the same source context.
#[derive(Default)]
struct RetainedSourceMembers {
    members: HashMap<String, Arc<CompilerOwnedSourceMember>>,
    failure: Option<String>,
    /// Ambient source discovery has no complete retained observation contract.
    legacy_lookup: bool,
}

/// Original source selection and members behind one command's parsed metadata, independent of cache entry spelling.
#[derive(Clone, Default)]
pub(super) struct StdlibSourceInputs {
    selection: StdlibSourceSelection,
    retained: Arc<Mutex<RetainedSourceMembers>>,
    #[cfg(test)]
    parses: usize,
}

impl std::fmt::Debug for StdlibSourceInputs {
    /// Describe selection and retained module names without requiring original file handles to be printable.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut debug = formatter.debug_struct("StdlibSourceInputs");
        debug.field("selection", &self.selection);
        match self.retained.lock() {
            Ok(retained) => {
                debug.field("members", &retained.members.keys().collect::<Vec<_>>());
                debug.field("failure", &retained.failure);
            }
            Err(_) => {
                debug.field("failure", &"retained source registry is poisoned");
            }
        }
        debug.finish()
    }
}

/// The consumer binding a cache to a plan, which decides what an ordinary admitted consumer may read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum StdlibSourcePurpose {
    /// Type checking and Body IR: an admitted consumer reads checked package metadata only.
    Checking,
    /// Rust-emission lowering: an admitted consumer also reads its admitted standard dependencies' original sources,
    /// because lowering expands their inherited trait defaults and constructor hooks into the consumer's adopters.
    Emission,
}

impl StdlibSourceInputs {
    /// Bind the original source Arc, invalidating metadata when authority changes and verifying reused member owners.
    pub(super) fn bind(&mut self, plan: &ProviderPlan, purpose: StdlibSourcePurpose) -> Result<bool, String> {
        let next = if let Some(source) = plan.standard_source_publication() {
            let dependencies = self.retained_standard_dependencies(plan, Some(source))?;
            StdlibSourceSelection::StandardSource {
                own: Arc::clone(source),
                dependencies,
                plan_identity: plan.semantic_projection_identity(),
            }
        } else if plan.is_admitted_library_context() {
            let dependencies = match purpose {
                StdlibSourcePurpose::Checking => Vec::new(),
                StdlibSourcePurpose::Emission => self.retained_standard_dependencies(plan, None)?,
            };
            if dependencies.is_empty() {
                StdlibSourceSelection::OrdinaryCheckedOnly
            } else {
                StdlibSourceSelection::AdmittedDependencies {
                    dependencies,
                    plan_identity: plan.semantic_projection_identity(),
                }
            }
        } else {
            StdlibSourceSelection::Legacy
        };
        let same = match (&self.selection, &next) {
            (StdlibSourceSelection::Legacy, StdlibSourceSelection::Legacy)
            | (StdlibSourceSelection::OrdinaryCheckedOnly, StdlibSourceSelection::OrdinaryCheckedOnly) => true,
            (
                StdlibSourceSelection::StandardSource {
                    own: current,
                    plan_identity: current_plan,
                    ..
                },
                StdlibSourceSelection::StandardSource {
                    own: next,
                    plan_identity: next_plan,
                    ..
                },
            ) => Arc::ptr_eq(current, next) && current_plan == next_plan,
            (
                StdlibSourceSelection::AdmittedDependencies {
                    dependencies: current,
                    plan_identity: current_plan,
                },
                StdlibSourceSelection::AdmittedDependencies {
                    dependencies: next,
                    plan_identity: next_plan,
                },
            ) => {
                current_plan == next_plan
                    && current.len() == next.len()
                    && current
                        .iter()
                        .zip(next)
                        .all(|(current, next)| Arc::ptr_eq(current, next))
            }
            _ => false,
        };
        if !same {
            *self = Self {
                selection: next,
                ..Self::default()
            };
        }
        plan.verify_standard_source_publication()?;
        self.verify_members()?;
        Ok(!same)
    }

    /// Retain the original source of every admitted standard dependency through its retained namespace issuer.
    ///
    /// `publisher` is the compiling standard package's own publication when it has one; an ordinary consumer has
    /// none and retains the same issuer sources without that cross-check. Nothing ambient is consulted, and a
    /// standard record without a retained issuer refuses the whole selection.
    fn retained_standard_dependencies(
        &mut self,
        plan: &ProviderPlan,
        publisher: Option<&TrustedStandardSourcePublication>,
    ) -> Result<Vec<Arc<TrustedStandardSourcePublication>>, String> {
        let mut dependencies = Vec::new();
        for record in plan.active_sdk_records() {
            if !matches!(
                record.provenance,
                crate::provider::ProviderProvenance::StandardPublication { .. }
            ) {
                continue;
            }
            let namespace =
                crate::provider::namespaces::SelectedProviderNamespace::from_plan(plan, &record.identity.stable_key())
                    .map_err(|error| self.refuse_selection(error))?;
            let source = match publisher {
                Some(publisher) => namespace.source_for_standard_publisher(publisher),
                None => namespace.source_for_admitted_consumer(),
            }
            .map_err(|error| self.refuse_selection(error))?;
            dependencies.push(source);
        }
        Ok(dependencies)
    }

    /// A failed issuer transfer drops the previous selection, so even a legacy cache cannot leak ambient source.
    fn refuse_selection(&mut self, error: String) -> String {
        *self = Self {
            selection: StdlibSourceSelection::OrdinaryCheckedOnly,
            retained: Arc::new(Mutex::new(RetainedSourceMembers {
                failure: Some(error.clone()),
                ..RetainedSourceMembers::default()
            })),
            #[cfg(test)]
            parses: 0,
        };
        error
    }

    /// Revalidate exact original declaration/member bytes at a checker or cache handoff, without reparsing them.
    pub(super) fn verify(&self) -> Result<(), String> {
        for source in self.selection.retained_sources() {
            source.verify().map_err(|error| error.to_string())?;
        }
        self.verify_members()
    }

    /// Check original member owners without rereading an already verified declaration at the same handoff.
    fn verify_members(&self) -> Result<(), String> {
        let retained = self
            .retained
            .lock()
            .map_err(|_| "retained source registry is poisoned".to_string())?;
        for member in retained.members.values() {
            member.verified_bytes().map_err(|error| error.to_string())?;
        }
        if let Some(error) = &retained.failure {
            return Err(error.clone());
        }
        Ok(())
    }

    /// Retain generated consumers only when every metadata source has its original verifiable owner.
    pub(super) fn supports_generation_reuse(&self) -> bool {
        self.retained.lock().is_ok_and(|retained| !retained.legacy_lookup) && self.verify().is_ok()
    }

    /// Read only an owned pinned module, retaining its original file owner; foreign metadata comes from providers.
    pub(super) fn read_module(&mut self, module: &[String]) -> Option<String> {
        let source = match &self.selection {
            StdlibSourceSelection::Legacy => {
                let relative = incan_lang::lang::stdlib::stdlib_stub_path(module)?;
                // A local/non-stdlib name cannot consult ambient stdlib files. Registered legacy lookups,
                // including missing files, still prevent reuse because their discovery has no original owner.
                self.retained.lock().ok()?.legacy_lookup = true;
                tracing::trace!(target: "incan::invocation", decision = "legacy_metadata_lookup", module = ?module);
                let path = super::find_stdlib_file(&relative)?;
                return std::fs::read_to_string(&path)
                    .map_err(|error| {
                        tracing::debug!(path = %path.display(), %error, "failed to read stdlib file");
                    })
                    .ok();
            }
            StdlibSourceSelection::OrdinaryCheckedOnly => return None,
            StdlibSourceSelection::StandardSource { .. } | StdlibSourceSelection::AdmittedDependencies { .. } => {
                Arc::clone(self.selection.retained_sources().into_iter().find(|source| {
                    module
                        .get(1)
                        .is_some_and(|root| source.namespace_roots().contains(&root.as_str()))
                })?)
            }
        };
        if module.first().map(String::as_str) != Some("std")
            || !module
                .get(1)
                .is_some_and(|root| source.namespace_roots().contains(&root.as_str()))
        {
            return None;
        }
        let key = module.join(".");
        let result = (|| {
            let mut retained = self
                .retained
                .lock()
                .map_err(|_| "retained source registry is poisoned".to_string())?;
            if let Some(error) = &retained.failure {
                return Err(error.clone());
            }
            let member = if let Some(member) = retained.members.get(&key) {
                Arc::clone(member)
            } else {
                let member = match source.open_source_module(module) {
                    Ok(member) => Arc::new(member),
                    Err(crate::provider::source_policy::TrustedStandardSourcePublicationError::Layout(
                        oven_model::toolchain_layout::CompilerOwnedSourceLayoutError::Io { source: error, .. },
                    )) if error.kind() == std::io::ErrorKind::NotFound
                        && module.len() > 2
                        && !incan_lang::lang::stdlib::is_known_stdlib_module(module)
                        && !incan_lang::lang::stdlib::stdlib_prelude_module_namespace(module)
                            .is_some_and(incan_lang::lang::stdlib::is_known_stdlib_module) =>
                    {
                        // Metadata resolution also probes declaration-qualified paths. An absent unregistered
                        // candidate is a lookup miss, not a broken owned module. Pinned namespace roots, registered
                        // modules and all invalid/symlinked coordinates retain the strict
                        // refusal below.
                        return Ok(None);
                    }
                    Err(error) => {
                        let error = error.to_string();
                        retained.failure = Some(error.clone());
                        return Err(error);
                    }
                };
                retained.members.insert(key, Arc::clone(&member));
                member
            };
            let bytes = member.verified_bytes().map_err(|error| error.to_string())?;
            String::from_utf8(bytes.to_vec()).map(Some).map_err(|error| {
                let error = error.to_string();
                retained.failure = Some(error.clone());
                error
            })
        })();
        match result {
            Ok(text) => text,
            Err(error) => {
                tracing::debug!(module_path = %module.join("."), %error, "retained source read refused");
                None
            }
        }
    }

    /// Match package-origin source identities against pinned ownership rather than an ambient catalog/manifest.
    pub(super) fn owns_package_identity(&self, module: &[String], library: &str) -> Option<bool> {
        let source = match &self.selection {
            StdlibSourceSelection::Legacy => return None,
            StdlibSourceSelection::OrdinaryCheckedOnly => return Some(false),
            StdlibSourceSelection::StandardSource { .. } | StdlibSourceSelection::AdmittedDependencies { .. } => {
                let Some(source) = self.selection.retained_sources().into_iter().find(|source| {
                    module
                        .get(1)
                        .is_some_and(|root| source.namespace_roots().contains(&root.as_str()))
                }) else {
                    return Some(false);
                };
                source
            }
        };
        let policy = module.get(1).and_then(|root| {
            incan_lang::lang::standard_packages::STANDARD_PACKAGE_NAMESPACE_POLICIES
                .iter()
                .find(|policy| policy.namespace_roots.contains(&root.as_str()))
        });
        Some(
            module
                .get(1)
                .is_some_and(|root| source.namespace_roots().contains(&root.as_str()))
                && policy.is_some_and(|policy| policy.package_name == library),
        )
    }

    /// Count an actual parse demanded by retained source metadata, excluding legacy readers and cached lookups.
    pub(super) fn record_parse(&mut self) {
        #[cfg(test)]
        if matches!(
            self.selection,
            StdlibSourceSelection::StandardSource { .. } | StdlibSourceSelection::AdmittedDependencies { .. }
        ) {
            self.parses += 1;
        }
    }

    /// Actual retained-source parser calls for focused JEC controls.
    #[cfg(test)]
    pub(super) fn parses(&self) -> usize {
        self.parses
    }
}
