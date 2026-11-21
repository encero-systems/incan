//! Snapshot descriptors staged the way Oven stages them, then resolved, read and written through by the Rust worker
//! host over a real store; ported from Oven's `test_compiler_snapshot.incn`.

use std::path::{Path, PathBuf};

use incan_worker_host_bindings::{
    Lookup, Snapshot, StoreError, begin, commit, disk_root, fork, get, lease_token, owned, put, store_error_text,
    store_identity, store_snapshot,
};

use crate::record_snapshot::{
    SNAPSHOT_FORMAT_PROFILE, SnapshotFaultKind, SnapshotHandle, StoreView, canonical_roots, digest,
    encode_snapshot_record, encode_store_binding, portable_store_view, resolve_snapshot, select_store_view,
    snapshot_record_key, staged_key,
};

const ALLOWED: &str = "compiler.input\tallowed";
const NO_DESCRIPTOR: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/// One test's private directory tree, removed when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Result<Self, String> {
        let path = std::env::temp_dir().join(format!("iwhn-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).map_err(|error| error.to_string())?;
        Ok(Self(path))
    }

    fn dir(&self, name: &str) -> Result<PathBuf, String> {
        let path = self.0.join(name);
        std::fs::create_dir_all(&path).map_err(|error| error.to_string())?;
        Ok(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A staged descriptor digest whose selected view and publication stay leased while the value lives.
struct Staged {
    digest: String,
    _selected: Snapshot,
    _published: Snapshot,
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn store_text(error: StoreError) -> String {
    store_error_text(&error)
}

fn open(store: &Path, view: &str) -> Result<Snapshot, String> {
    store_snapshot(disk_root(text(store)), view.to_owned()).map_err(store_text)
}

fn write(view: &Snapshot, entries: &[(&str, &[u8])]) -> Result<Snapshot, String> {
    let mut transaction = begin(view);
    for (key, value) in entries {
        transaction = put(transaction, (*key).to_owned(), &value.to_vec(), 0);
    }
    commit(transaction).map_err(store_text)
}

fn found_text(found: Lookup) -> Result<String, String> {
    match found {
        Lookup::Found(value) => String::from_utf8(owned(&value)).map_err(|error| error.to_string()),
        Lookup::Deleted => Err("snapshot read selected a tombstone".to_owned()),
        Lookup::Absent => Err("snapshot read selected absence".to_owned()),
    }
}

/// The home's own view of `store`, bound once through `<home>/bakery.view` as Oven's `home_view` binds it.
fn home_view(home: &Path, store: &Path, identity: &str) -> Result<String, String> {
    let descriptor = home.join("bakery.view");
    if descriptor.is_file() {
        let bound = select_store_view(&text(&descriptor))?.binding;
        if bound.store_id != identity {
            return Err(format!("{} binds another bakery", text(&descriptor)));
        }
        return Ok(bound.home_view);
    }
    let view = format!("home-{}", &digest(text(home).as_bytes())[..32]);
    write(&open(store, &view)?, &[("incan.home-view", view.as_bytes())])?;
    let binding = StoreView {
        store_id: identity.to_owned(),
        locator: text(store),
        home_view: view.clone(),
    };
    std::fs::write(home.join("bakery.view.crumb"), encode_store_binding(&binding)?)
        .map_err(|error| error.to_string())?;
    std::fs::write(&descriptor, portable_store_view(&binding)?).map_err(|error| error.to_string())?;
    Ok(view)
}

/// Stage one content-named descriptor in the home's own view, as Oven's `stage_snapshot` stages it.
fn stage_snapshot(
    home: &Path,
    store: &Path,
    selected: &Snapshot,
    roots: &[&str],
    profile: &str,
) -> Result<Staged, String> {
    let identity = store_identity(&disk_root(text(store))).map_err(store_text)?;
    let roots: Vec<String> = roots.iter().map(|root| (*root).to_owned()).collect();
    let handle = SnapshotHandle {
        store_id: identity.clone(),
        generation: selected.generation,
        manifest: selected.digest.clone(),
        view: selected.view.clone(),
        view_revision: selected.index.clone(),
        allowed_roots: canonical_roots(&roots)?,
        lease: lease_token(selected),
        format_profile: profile.to_owned(),
    };
    let encoded = encode_snapshot_record(&handle)?;
    let digest = digest(&encoded);
    let view = home_view(home, store, &identity)?;
    let published = write(&open(store, &view)?, &[(&snapshot_record_key(&digest), &encoded)])?;
    Ok(Staged {
        digest,
        _selected: selected.clone(),
        _published: published,
    })
}

fn fault_kind<T>(result: Result<T, crate::record_snapshot::SnapshotFault>) -> Option<SnapshotFaultKind> {
    result.err().map(|fault| fault.kind)
}

fn bound(home: &Path) -> Result<StoreView, String> {
    Ok(select_store_view(&text(&home.join("bakery.view")))?.binding)
}

#[test]
fn snapshot_descriptors_are_staged_resolved_and_scoped_by_digest() -> Result<(), String> {
    let scratch = Scratch::new("scoped")?;
    let home = scratch.dir("home")?;
    let store = scratch.dir("bakery")?;
    let base = write(&open(&store, "head")?, &[("home", b"ready")])?;
    let project = fork(&base, "project".to_owned()).map_err(store_text)?;
    let project = write(&project, &[(ALLOWED, b"old"), ("compiler.other\toutside", b"outside")])?;

    let staged = stage_snapshot(&home, &store, &project, &["compiler.input"], SNAPSHOT_FORMAT_PROFILE)?;
    assert_eq!(staged.digest.len(), 64);
    let opened = resolve_snapshot(&text(&home), &staged.digest).map_err(|fault| fault.message())?;
    assert_eq!(
        found_text(opened.read(ALLOWED).map_err(|fault| fault.message())?)?,
        "old"
    );

    let project = write(&project, &[(ALLOWED, b"new")])?;
    let retained = found_text(opened.read(ALLOWED).map_err(|fault| fault.message())?)?;
    assert_eq!(retained, "old", "worker followed the mutable project head");
    assert_eq!(
        fault_kind(opened.read("compiler.other\toutside")),
        Some(SnapshotFaultKind::RootOutside)
    );
    assert_eq!(
        fault_kind(resolve_snapshot(&text(&home), NO_DESCRIPTOR)),
        Some(SnapshotFaultKind::MissingDescriptor)
    );

    let output_view = resolve_snapshot(&text(&home), &staged.digest).map_err(|fault| fault.message())?;
    let outputs = output_view
        .stage_compiler_outputs(b"local-value", b"read-receipt", b"", b"evaluations=1")
        .map_err(|fault| fault.message())?;
    for (kind, root, expected) in [
        ("value", &outputs.value, b"local-value".as_slice()),
        ("receipt", &outputs.receipt, b"read-receipt".as_slice()),
        ("counter", &outputs.counter, b"evaluations=1".as_slice()),
    ] {
        let stored = get(&outputs.snapshot, staged_key(kind, root)).map_err(store_text)?;
        assert_eq!(stored.as_deref(), Some(expected));
    }

    let wrong = stage_snapshot(
        &home,
        &store,
        &project,
        &["compiler.input"],
        "incan.compiler-records/wrong",
    )?;
    assert_eq!(
        fault_kind(resolve_snapshot(&text(&home), &wrong.digest)),
        Some(SnapshotFaultKind::ProfileMismatch)
    );
    Ok(())
}

#[test]
fn two_homes_over_one_bakery_keep_their_own_views() -> Result<(), String> {
    let scratch = Scratch::new("two-homes")?;
    let store = scratch.dir("bakery")?;
    let first = scratch.dir("first")?;
    let second = scratch.dir("second")?;
    let base = write(&open(&store, "head")?, &[("seed", b"ready")])?;
    let project = fork(&base, "project".to_owned()).map_err(store_text)?;
    let project = write(&project, &[(ALLOWED, b"old")])?;
    // The two descriptors differ in their roots, so they are different content-named records.
    let first_staged = stage_snapshot(&first, &store, &project, &["compiler.input"], SNAPSHOT_FORMAT_PROFILE)?;
    let second_roots = ["compiler.input", "compiler.other"];
    let second_staged = stage_snapshot(&second, &store, &project, &second_roots, SNAPSHOT_FORMAT_PROFILE)?;
    // One store, two views.
    let first_binding = bound(&first)?;
    let second_binding = bound(&second)?;
    assert_eq!(first_binding.store_id, second_binding.store_id);
    assert_eq!(first_binding.locator, second_binding.locator);
    assert_ne!(
        first_binding.home_view, second_binding.home_view,
        "two homes share one home view"
    );
    // A home resolves only its own descriptors.
    assert_eq!(
        fault_kind(resolve_snapshot(&text(&second), &first_staged.digest)),
        Some(SnapshotFaultKind::MissingDescriptor)
    );
    // The first home's project advances; the second home still reads its own revision.
    let project = write(&project, &[(ALLOWED, b"new")])?;
    let _restaged = stage_snapshot(&first, &store, &project, &["compiler.input"], SNAPSHOT_FORMAT_PROFILE)?;
    let opened = resolve_snapshot(&text(&second), &second_staged.digest).map_err(|fault| fault.message())?;
    assert_eq!(
        found_text(opened.read(ALLOWED).map_err(|fault| fault.message())?)?,
        "old"
    );
    // Staging again keeps a home's view, and a copied portable descriptor names the same view.
    assert_eq!(bound(&first)?.home_view, first_binding.home_view);
    let copy = scratch.dir("copy")?;
    std::fs::copy(first.join("bakery.view"), copy.join("bakery.view")).map_err(|error| error.to_string())?;
    let copied = stage_snapshot(&copy, &store, &project, &["compiler.input"], SNAPSHOT_FORMAT_PROFILE)?;
    assert_eq!(bound(&copy)?.home_view, first_binding.home_view);
    let reopened = resolve_snapshot(&text(&copy), &copied.digest).map_err(|fault| fault.message())?;
    assert_eq!(
        found_text(reopened.read(ALLOWED).map_err(|fault| fault.message())?)?,
        "new"
    );
    Ok(())
}
