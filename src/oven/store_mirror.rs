//! Read-only mirrors of the Oven store, consulted on a selection miss before any bake is considered.
//!
//! RFC 125 describes a mirror as a copy of the registry's artifact directories, and the Loaf store as the only cache
//! and the only offline source. That makes a mirror the simplest possible thing: another store root, in the same
//! `entries/` layout, that this machine may read but never writes. A fresh checkout or CI runner that syncs such a
//! directory first can then admit a whole sealed closure — the standard library, the compiler-suite foundation — by
//! copying it, instead of running the compatibility baker to reproduce it. The same `INCAN_OVEN_MIRRORS` list also
//! names Loaf envelope roots (see `loaf_mirror`); each reader recognises its own layout and ignores the rest.
//!
//! Nothing is trusted from the mirror. A candidate is selected under the mirror's own manager lock and active lease,
//! its admitted record is revalidated, and it enters the local store only through the same verifying publication
//! a store-to-store package import uses, which reads every file and proves it against the record as it goes. A
//! mirror entry that has been altered is refused with an integrity error, exactly as a tampered local entry would be.
//! An entry is imported under its publisher's original receipt, so provenance survives the copy; a legacy entry that
//! carries no witness receipt cannot be imported, because there is nothing to import it under.
//!
//! Only whole entries are ever imported. Every rlib inside a closure was compiled against the other rlibs in that
//! same closure, so mixing units from two publishers is never valid; copying the entry as a unit keeps that invariant
//! without inspecting a single artifact.

use std::ffi::OsString;
use std::path::PathBuf;

use super::OvenReceipt;
use super::store::{
    OvenArtifactKind, OvenArtifactManifest, OvenArtifactMaterializedFile, OvenArtifactPublishRequest, OvenStore,
    OvenStoreError, PublishedOvenStore,
};

/// Environment variable listing mirror roots, separated the way `PATH` is on the host.
pub(crate) const MIRRORS_ENV: &str = "INCAN_OVEN_MIRRORS";

/// The mirror roots a command may consult, in the order given. Absent or empty means no mirrors.
pub(crate) fn configured_mirrors<E>(env: E) -> Vec<PathBuf>
where
    E: Fn(&str) -> Option<OsString>,
{
    env(MIRRORS_ENV)
        .filter(|value| !value.is_empty())
        .map(|value| {
            std::env::split_paths(&value)
                .filter(|path| !path.as_os_str().is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// One entry imported from a mirror, for reporting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MirrorImport {
    /// The mirror root the entry came from.
    pub(crate) mirror: PathBuf,
    /// The manifest the local store admitted.
    pub(crate) manifest: OvenArtifactManifest,
}

/// Import every entry matching `matches` from the first mirror that has any, through verified publication.
///
/// Mirrors are consulted in order and the first one offering a match is the only one used, so two mirrors that both
/// carry the same closure cannot produce two imports. A mirror that is unreadable — missing layout, no lock files —
/// is an error rather than a silent skip: a misconfigured mirror path should be noticed, not quietly turned into a
/// bake. An entry the local store already admitted is returned as it stands; the verifying publication is
/// idempotent for identical content.
///
/// The receipt an entry is imported under is its publisher's witness receipt when the entry carries one. Kinds that
/// carry none — a compiler-suite family is published as a batch under the caller's receipt — are imported under
/// `caller_receipt`, and only when the entry was published for that exact receipt or is a direct-rustc plan, which
/// is reusable across compatible receipts by construction. That is the same rule the package-Loaf copy applies.
/// Entries of one family are imported foundation first, so a member that refers to another is never admitted
/// before the entry it names.
pub(crate) fn import_matching_from_mirrors<F>(
    store: &OvenStore,
    mirrors: &[PathBuf],
    caller_receipt: Option<&OvenReceipt>,
    matches: F,
) -> Result<Vec<MirrorImport>, OvenStoreError>
where
    F: Fn(&OvenArtifactManifest) -> bool,
{
    for mirror in mirrors {
        // One mirror list serves every mirrored root kind. A Loaf envelope root, or a path that is not a store
        // at all, has no `entries/` and is simply not a store mirror; only a store that exists is read.
        if !mirror.join("entries").is_dir() {
            continue;
        }
        let mut candidates = PublishedOvenStore::new(mirror).select_payloads_matching_for_execution(&matches)?;
        if candidates.is_empty() {
            continue;
        }
        candidates.sort_by_key(|candidate| family_import_rank(candidate.manifest.kind));
        let mut imported = Vec::with_capacity(candidates.len());
        for candidate in candidates {
            candidate.verify_admitted_record()?;
            let Some(receipt) =
                import_receipt_for(&candidate.manifest, candidate.original_native_receipt(), caller_receipt)
            else {
                // Nothing to publish this entry under: no witness, and no caller receipt it was published for. It is
                // left to the local bake. Not an error: a mirror may hold such entries beside importable ones.
                continue;
            };
            let admitted = candidate.admitted_materialized_files().to_vec();
            let (manifest, artifact_root, payload, _lease) = candidate.into_parts();
            let materialized_files = manifest
                .materialized_files
                .iter()
                .map(|file| OvenArtifactMaterializedFile {
                    source_path: artifact_root.join(&file.relative_path),
                    relative_path: file.relative_path.clone(),
                })
                .collect();
            let published = store.publish_verified_import(
                &OvenArtifactPublishRequest {
                    receipt,
                    domain: manifest.domain.clone(),
                    kind: manifest.kind,
                    payload,
                    materialized_files,
                },
                &admitted,
            )?;
            imported.push(MirrorImport {
                mirror: mirror.clone(),
                manifest: published,
            });
        }
        if !imported.is_empty() {
            return Ok(imported);
        }
    }
    Ok(Vec::new())
}

/// The receipt to publish one mirror entry under, or `None` when there is no honest choice.
fn import_receipt_for(
    manifest: &OvenArtifactManifest,
    witness: Option<&OvenReceipt>,
    caller_receipt: Option<&OvenReceipt>,
) -> Option<OvenReceipt> {
    if let Some(witness) = witness {
        return Some(witness.clone());
    }
    let caller = caller_receipt?;
    let compatible = manifest.build_unit_identity == caller.build_unit_identity && manifest.intent == caller.intent;
    let same_or_reusable =
        manifest.kind == OvenArtifactKind::DirectRustcPlan || manifest.receipt_identity == caller.identity;
    (compatible && same_or_reusable).then(|| caller.clone())
}

/// Import order within one family: whatever others refer to comes first.
fn family_import_rank(kind: OvenArtifactKind) -> u8 {
    match kind {
        OvenArtifactKind::CompilerTestSuiteFoundation => 0,
        OvenArtifactKind::CompilerTestSuiteShard => 1,
        _ => 2,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use super::*;
    use crate::oven::store::tests::{request, write_project};
    use crate::oven::store::{OvenArtifactKind, OvenStoreError, OvenStoreLimits};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn limits() -> OvenStoreLimits {
        OvenStoreLimits::new(1_000_000, 1_000_000, 1_000_000)
    }

    /// Publish one closure with a materialized file into a store that will serve as the mirror.
    fn publish_into_mirror(
        mirror_root: &Path,
        project: &Path,
        payload: &[u8],
    ) -> Result<OvenArtifactManifest, Box<dyn std::error::Error>> {
        let staged = project.join("native.rlib");
        fs::write(&staged, b"native artifact")?;
        let mut publication = request(project, "mirror-owner", payload)?;
        // A direct-rustc closure is the kind a mirror exists for, and the kind that carries the witness receipt an
        // import is published under.
        publication.kind = OvenArtifactKind::DirectRustcPlan;
        publication.materialized_files.push(OvenArtifactMaterializedFile {
            source_path: staged,
            relative_path: "lib/native.rlib".to_string(),
        });
        let mirror = OvenStore::new(mirror_root, limits());
        Ok(mirror.publish(&publication)?)
    }

    #[test]
    fn configured_mirrors_splits_like_a_path_list() -> TestResult {
        let none = configured_mirrors(|_| None);
        assert!(none.is_empty());
        let empty = configured_mirrors(|_| Some(OsString::new()));
        assert!(empty.is_empty());
        let joined = std::env::join_paths([Path::new("/a"), Path::new("/b")])?;
        let two = configured_mirrors(|name| {
            if name == MIRRORS_ENV {
                Some(joined.clone())
            } else {
                None
            }
        });
        assert_eq!(two, vec![PathBuf::from("/a"), PathBuf::from("/b")]);
        Ok(())
    }

    #[test]
    fn a_miss_is_answered_from_the_mirror_and_then_served_locally() -> TestResult {
        let mirror_root = tempfile::tempdir()?;
        let local_root = tempfile::tempdir()?;
        let project = tempfile::tempdir()?;
        write_project(project.path())?;
        let published = publish_into_mirror(mirror_root.path(), project.path(), b"closure payload")?;
        let local = OvenStore::new(local_root.path(), limits());

        let before = local.select_payloads_matching_for_execution(|m| m.identity == published.identity)?;
        assert!(before.is_empty(), "the local store starts empty");

        let imported = import_matching_from_mirrors(&local, &[mirror_root.path().to_path_buf()], None, |m| {
            m.identity == published.identity
        })?;
        let selected = local.select_payloads_matching_for_execution(|m| m.identity == published.identity)?;
        assert_eq!(imported.len(), 1, "one entry was imported");
        assert_eq!(imported[0].mirror, mirror_root.path());
        assert_eq!(
            imported[0].manifest.identity, published.identity,
            "content identity survives the copy"
        );
        assert_eq!(selected.len(), 1, "and it now selects locally");
        assert_eq!(selected[0].payload, b"closure payload");
        assert!(
            local
                .entry_root_for_tests(&published.identity)
                .join("artifacts/lib/native.rlib")
                .is_file(),
            "the materialized closure was copied"
        );

        // Importing again is idempotent for identical content: the same entry, still one.
        let imported_again = import_matching_from_mirrors(&local, &[mirror_root.path().to_path_buf()], None, |m| {
            m.identity == published.identity
        })?;
        assert_eq!(imported_again.len(), 1);
        assert_eq!(imported_again[0].manifest.identity, published.identity);
        let again = local.select_payloads_matching_for_execution(|m| m.identity == published.identity)?;
        assert_eq!(again.len(), 1);
        Ok(())
    }

    #[test]
    fn a_tampered_mirror_entry_is_refused_and_nothing_is_admitted() -> TestResult {
        let mirror_root = tempfile::tempdir()?;
        let local_root = tempfile::tempdir()?;
        let project = tempfile::tempdir()?;
        write_project(project.path())?;
        let published = publish_into_mirror(mirror_root.path(), project.path(), b"closure payload")?;
        let artifact = OvenStore::new(mirror_root.path(), limits())
            .entry_root_for_tests(&published.identity)
            .join("artifacts/lib/native.rlib");
        fs::remove_file(&artifact)?;
        fs::write(&artifact, b"altered on the mirror")?;

        let local = OvenStore::new(local_root.path(), limits());
        let result = import_matching_from_mirrors(&local, &[mirror_root.path().to_path_buf()], None, |m| {
            m.identity == published.identity
        });
        assert!(
            matches!(result, Err(OvenStoreError::Integrity { .. })),
            "got {result:?}"
        );
        let after = local.select_payloads_matching_for_execution(|m| m.identity == published.identity)?;
        assert!(after.is_empty(), "a refused import leaves no entry behind");
        Ok(())
    }

    #[test]
    fn mirrors_are_consulted_in_order_and_only_the_first_hit_is_used() -> TestResult {
        let empty_mirror = tempfile::tempdir()?;
        let mirror_root = tempfile::tempdir()?;
        let local_root = tempfile::tempdir()?;
        let project = tempfile::tempdir()?;
        write_project(project.path())?;
        // An empty but valid mirror store first: it must be skipped, not treated as a failure.
        OvenStore::new(empty_mirror.path(), limits()).ensure_layout_for_tests()?;
        let published = publish_into_mirror(mirror_root.path(), project.path(), b"closure payload")?;
        let local = OvenStore::new(local_root.path(), limits());
        let imported = import_matching_from_mirrors(
            &local,
            &[empty_mirror.path().to_path_buf(), mirror_root.path().to_path_buf()],
            None,
            |m| m.identity == published.identity,
        )?;
        assert_eq!(imported.len(), 1);
        assert_eq!(imported[0].mirror, mirror_root.path());
        Ok(())
    }

    #[test]
    fn a_root_without_entries_is_not_a_store_mirror_and_is_skipped() -> TestResult {
        // The same list names Loaf envelope roots; those have no `entries/` and are another reader's business.
        let envelope_root = tempfile::tempdir()?;
        fs::write(envelope_root.path().join("envelope.json"), b"{}")?;
        let local_root = tempfile::tempdir()?;
        let local = OvenStore::new(local_root.path(), limits());
        let imported = import_matching_from_mirrors(&local, &[envelope_root.path().to_path_buf()], None, |_| true)?;
        assert!(imported.is_empty());
        assert!(
            !envelope_root.path().join("entries").exists(),
            "a skipped root is never touched"
        );
        Ok(())
    }

    #[test]
    fn a_witness_less_family_imports_under_the_callers_receipt_foundation_first() -> TestResult {
        let mirror_root = tempfile::tempdir()?;
        let local_root = tempfile::tempdir()?;
        let project = tempfile::tempdir()?;
        write_project(project.path())?;
        let mirror = OvenStore::new(mirror_root.path(), limits());
        let mut suite = request(project.path(), "suite-owner", b"suite payload")?;
        suite.kind = OvenArtifactKind::CompilerTestSuite;
        let mut foundation = request(project.path(), "suite-owner", b"foundation payload")?;
        foundation.kind = OvenArtifactKind::CompilerTestSuiteFoundation;
        // Publish the suite before the foundation so the mirror's own order cannot be what makes the import order.
        let suite_manifest = mirror.publish(&suite)?;
        let foundation_manifest = mirror.publish(&foundation)?;
        let receipt = suite.receipt.clone();
        let family = |m: &OvenArtifactManifest| {
            matches!(
                m.kind,
                OvenArtifactKind::CompilerTestSuite | OvenArtifactKind::CompilerTestSuiteFoundation
            ) && m.build_unit_identity == receipt.build_unit_identity
        };
        let local = OvenStore::new(local_root.path(), limits());

        // Without a caller receipt there is nothing to import a witness-less entry under.
        let none = import_matching_from_mirrors(&local, &[mirror_root.path().to_path_buf()], None, family)?;
        assert!(none.is_empty());

        let imported =
            import_matching_from_mirrors(&local, &[mirror_root.path().to_path_buf()], Some(&receipt), family)?;
        let kinds = imported.iter().map(|entry| entry.manifest.kind).collect::<Vec<_>>();
        assert_eq!(
            kinds,
            vec![
                OvenArtifactKind::CompilerTestSuiteFoundation,
                OvenArtifactKind::CompilerTestSuite
            ],
            "foundation first, whatever order the mirror published them in"
        );
        assert_eq!(imported[0].manifest.identity, foundation_manifest.identity);
        assert_eq!(imported[1].manifest.identity, suite_manifest.identity);
        assert_eq!(
            imported[1].manifest.receipt_identity, receipt.identity,
            "published under the caller's receipt"
        );
        Ok(())
    }
}
