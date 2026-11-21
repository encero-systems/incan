//! Tests of installed compiler record sets: sealed range selection, family reads, payload validation and immutable
//! identity refusals.

mod capabilities;
mod member_families;
mod namespace_records;
mod type_scopes;
mod typechecker_reads;

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use sha2::{Digest, Sha256};

use super::declaration_records::read_declaration_in;
use super::runtime_bindings::read_runtime_bindings_in;
use super::thread_selection;
use super::{
    Metadata, Projection, RecordEntry, RecordReads, RecordSet, open_value, read_group, read_summary, read_surface_in,
};
use crate::api_metadata::{
    CHECKED_API_METADATA_SCHEMA_VERSION, CheckedApiMetadataPackage, collect_checked_api_metadata,
};
use crate::library_manifest::executable_surface::{ExecutableSurface, runtime_bindings};
use crate::library_manifest::{ExecutableRepresentationExport, LibraryContractMetadata, LibraryManifest};

/// The canonical lowercase SHA-256 spelling of `bytes`.
fn content_name(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Install `values` as the records of one Loaf group named `name` in a fresh payload directory under `root`.
fn record_set(root: &Path, name: &str, values: &[(&str, Vec<u8>)]) -> Result<RecordSet, Box<dyn std::error::Error>> {
    fs::create_dir(root.join("payloads"))?;
    let mut entries = Vec::new();
    for (family, value) in values {
        let digest = content_name(value);
        fs::write(root.join("payloads").join(&digest), value)?;
        entries.push(RecordEntry {
            key: format!(
                "loaf/debug/{name}/{family}/00000000/{}",
                content_name(family.as_bytes())
            ),
            revision: digest.clone(),
            value: digest,
        });
    }
    Ok(RecordSet::new(root.to_path_buf(), entries))
}

#[test]
fn a_record_group_is_read_and_authenticated_once_per_process() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let metadata = LibraryManifest::encode_compiler_record_value(&Metadata {
        name: "probe_loaf".to_owned(),
        version: "0.1.0".to_owned(),
        incan_version: "0.6.0".to_owned(),
        manifest_format: 1,
        manifest_digest: content_name(b"probe_loaf manifest"),
        vocabulary: false,
        provider_operations: false,
        native_unions: false,
        rust_paths: BTreeSet::new(),
        rust_abi_cache_declared: false,
        rust_abi_cache_digest: None,
    })?;
    let contract = LibraryManifest::encode_compiler_record_value(&LibraryContractMetadata::default())?;
    let expected = RecordReads {
        records: 2,
        bytes: metadata.len() + contract.len(),
    };
    let set = record_set(
        root.path(),
        "probe_loaf",
        &[("metadata", metadata), ("contract-metadata", contract)],
    )?;
    let dir = root.path().join("debug");

    let (first, first_digest) =
        read_group(&set, &dir, "probe_loaf", Projection::Complete)?.ok_or("the group is installed")?;
    assert_eq!(set.reads(), expected, "the first read opens each record once");
    let (second, second_digest) =
        read_group(&set, &dir, "probe_loaf", Projection::Complete)?.ok_or("the group is installed")?;
    assert_eq!(first_digest, second_digest);
    assert_eq!(first.name, second.name);
    assert_eq!(
        set.reads(),
        expected,
        "a group this process already authenticated is not read or hashed again"
    );
    Ok(())
}

#[test]
fn a_payload_that_still_parses_but_differs_from_its_content_name_is_refused() -> Result<(), Box<dyn std::error::Error>>
{
    let root = tempfile::tempdir()?;
    fs::create_dir(root.path().join("payloads"))?;
    let original = br#"{"name":"BindingDeclaration"}"#;
    let digest = content_name(original);
    let payload = root.path().join("payloads").join(&digest);
    fs::write(&payload, original)?;
    assert_eq!(open_value(root.path(), &digest)?, original);

    // One byte changed in place: the value still decodes, but it is no longer the content its name promises.
    fs::write(&payload, br#"{"name":"BindingXeclaration"}"#)?;
    let refusal = match open_value(root.path(), &digest) {
        Ok(_) => return Err("a damaged compiler-record payload was accepted".into()),
        Err(reason) => reason,
    };
    assert!(refusal.contains(&digest), "the refusal names the payload: {refusal}");
    assert!(
        refusal.contains("does not match its content name"),
        "the refusal says why: {refusal}"
    );
    Ok(())
}

/// The metadata record of the probe Loaf.
fn probe_metadata() -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    Ok(LibraryManifest::encode_compiler_record_value(&Metadata {
        name: "probe_loaf".to_owned(),
        version: "0.1.0".to_owned(),
        incan_version: "0.6.0".to_owned(),
        manifest_format: 1,
        manifest_digest: content_name(b"probe_loaf manifest"),
        vocabulary: false,
        provider_operations: false,
        native_unions: false,
        rust_paths: BTreeSet::new(),
        rust_abi_cache_declared: false,
        rust_abi_cache_digest: None,
    })?)
}

/// Contract metadata whose checked API documents one function.
fn documented_contract() -> Result<LibraryContractMetadata, Box<dyn std::error::Error>> {
    let source = "pub def price() -> int:\n    \"\"\"The list price.\"\"\"\n    return 1999\n";
    let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = crate::typechecker::TypeChecker::new();
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let module = collect_checked_api_metadata(&program, &checker, vec!["prices".to_owned()]);
    Ok(LibraryContractMetadata {
        api: Some(CheckedApiMetadataPackage {
            schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
            package: None,
            modules: vec![module],
            public_namespaces: Vec::new(),
        }),
        ..LibraryContractMetadata::default()
    })
}

/// The probe Loaf installed with its contract facts and their documentation as separate records, the contract
/// metadata it was split from, and what reading only the facts reads.
fn documented_set(
    root: &Path,
) -> Result<(RecordSet, LibraryContractMetadata, RecordReads), Box<dyn std::error::Error>> {
    let original = documented_contract()?;
    let (facts, documentation) = original.split_documentation();
    let metadata = probe_metadata()?;
    let contract = LibraryManifest::encode_compiler_record_value(&facts)?;
    let documentation = LibraryManifest::encode_compiler_record_value(&documentation)?;
    let facts_read = RecordReads {
        records: 2,
        bytes: metadata.len() + contract.len(),
    };
    let set = record_set(
        root,
        "probe_loaf",
        &[
            ("metadata", metadata),
            ("contract-metadata", contract),
            ("documentation", documentation),
        ],
    )?;
    Ok((set, original, facts_read))
}

#[test]
fn a_compile_reads_no_documentation_records() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let (set, _, facts_read) = documented_set(root.path())?;
    let dir = root.path().join("debug");
    for projection in [Projection::Complete, Projection::Execution] {
        read_group(&set, &dir, "probe_loaf", projection)?.ok_or("the group is installed")?;
    }
    assert_eq!(
        set.reads(),
        facts_read,
        "compiles open the metadata and contract facts, never the documentation"
    );
    Ok(())
}

#[test]
fn a_documented_read_restores_the_documentation_records() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let (set, original, _) = documented_set(root.path())?;
    let dir = root.path().join("debug");
    let (compiled, _) = read_group(&set, &dir, "probe_loaf", Projection::Complete)?.ok_or("the group is installed")?;
    let (documented, _) =
        read_group(&set, &dir, "probe_loaf", Projection::Documented)?.ok_or("the group is installed")?;
    assert_ne!(
        compiled.contract_metadata, original,
        "the compile's manifest carries no documentation"
    );
    assert_eq!(
        documented.contract_metadata, original,
        "a documented read gives back the checked API with its documentation"
    );
    Ok(())
}

#[test]
fn an_inventory_summary_is_read_once_per_process() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let metadata = probe_metadata()?;
    let expected = RecordReads {
        records: 1,
        bytes: metadata.len(),
    };
    let set = record_set(root.path(), "probe_loaf", &[("metadata", metadata)])?;
    let dir = root.path().join("debug");
    for _ in 0..2 {
        let summary = read_summary(&set, &dir, "probe_loaf")?.ok_or("the group is installed")?;
        assert_eq!(summary.identity.name, "probe_loaf");
    }
    assert_eq!(
        set.reads(),
        expected,
        "a summary this process already authenticated is not read or hashed again"
    );
    Ok(())
}

/// What executable resolution may read of a reached Loaf: its metadata, descriptor and public identities.
const SURFACE_BOUND: usize = 4096;

/// A library whose executable surface is small beside its contract metadata: a function, a model with a method,
/// a descriptor, and a checked API repeated until the contract metadata is well over the bound.
fn surfaced_library() -> Result<LibraryManifest, Box<dyn std::error::Error>> {
    let source = "pub def price() -> int:\n    return 1999\n\npub model Money:\n    cents: int\n\n    pub def doubled(self) -> int:\n        return self.cents * 2\n";
    surfaced_library_for("prices", source)
}

fn surfaced_library_for(name: &str, source: &str) -> Result<LibraryManifest, Box<dyn std::error::Error>> {
    let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = crate::typechecker::TypeChecker::new();
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let exports = crate::library_exports::collect_checked_public_exports(&program, &checker);
    let mut manifest = LibraryManifest::from_checked_exports(name, "0.1.0", &exports);
    let module = collect_checked_api_metadata(&program, &checker, vec![name.to_owned()]);
    manifest.contract_metadata.api = Some(CheckedApiMetadataPackage {
        schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
        package: None,
        modules: vec![module; 200],
        public_namespaces: Vec::new(),
    });
    manifest.contract_metadata.executable_representation = Some(ExecutableRepresentationExport {
        representation_version: 1,
        content_digest: content_name(b"prices bodies"),
    });
    Ok(manifest)
}

/// Install every compiler record of `manifest` as its Loaf group in a fresh payload directory under `root`.
fn installed_library(root: &Path, manifest: &LibraryManifest) -> Result<RecordSet, Box<dyn std::error::Error>> {
    fs::create_dir(root.join("payloads"))?;
    let mut entries = Vec::new();
    for record in manifest.compiler_records(&content_name(b"prices manifest"))?.records {
        let digest = content_name(&record.value);
        fs::write(root.join("payloads").join(&digest), &record.value)?;
        entries.push(RecordEntry {
            key: format!("loaf/debug/{}/{}", manifest.name, record.key()),
            revision: digest.clone(),
            value: digest,
        });
    }
    Ok(RecordSet::new(root.to_path_buf(), entries))
}

#[test]
fn executable_resolution_reads_only_a_reached_loafs_executable_surface() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let manifest = surfaced_library()?;
    let set = installed_library(root.path(), &manifest)?;
    let (surface, _) = read_surface_in(&set, &root.path().join("debug"), "prices")?.ok_or("the Loaf is installed")?;
    assert_eq!(
        *surface,
        ExecutableSurface::of(&manifest),
        "the surface the manifest describes"
    );
    let read = set.reads();
    assert!(
        read.bytes <= SURFACE_BOUND,
        "executable resolution read {} records, {} bytes; the bound is {SURFACE_BOUND}",
        read.records,
        read.bytes
    );
    Ok(())
}

#[test]
fn sealed_ranges_read_only_the_requested_trait_and_refuse_changed_bytes_and_wrong_membership()
-> Result<(), Box<dyn std::error::Error>> {
    use crate::executable_resolution::resolve_loaf_declarations;
    use crate::library_manifest::executable_ranges::{RANGE, export_ranges, record_key};
    use crate::library_manifest::executable_surface::own_public_identities;
    use incan_semantics_core::body_ir::BodyIrModule;
    use incan_semantics_core::encode_incan_symbol_identity;
    use incan_semantics_core::executable_representation::{
        DeclarationCoverage, SurfaceReader, build_surface, representation_version,
    };
    let root = tempfile::tempdir()?;
    let name = "prices";
    let dir = root.path().join("debug");
    fs::create_dir(&dir)?;
    let mut manifest = surfaced_library_for(name, "pub trait First:\n    pass\n\npub trait Other:\n    pass\n")?;
    let public = own_public_identities(&manifest);
    let first = public
        .iter()
        .find(|identity| identity.declaration_name == "First")
        .ok_or("missing First")?;
    let other = public
        .iter()
        .find(|identity| identity.declaration_name == "Other")
        .ok_or("missing Other")?;
    let mut module = BodyIrModule::declarations_only(name.into());
    module.trait_declarations = vec![first.clone(), other.clone()];
    let bytes = build_surface(&[module], name, &manifest.version, &public, &BTreeSet::new())?;
    manifest.contract_metadata.executable_representation = Some(ExecutableRepresentationExport {
        representation_version: representation_version(&bytes)?,
        content_digest: content_name(&bytes),
    });
    let path = dir.join("prices.incnbody");
    fs::write(&path, &bytes)?;
    let mut set = installed_library(root.path(), &manifest)?;
    let first_key = record_key(RANGE, &encode_incan_symbol_identity(first));
    let other_key = record_key(RANGE, &encode_incan_symbol_identity(other));
    let mut other_content = None;
    for (key, value) in export_ranges(&manifest, &content_name(b"prices manifest"), &bytes)? {
        let digest = content_name(&value);
        if key == other_key {
            other_content = Some(digest.clone());
        }
        fs::write(root.path().join("payloads").join(&digest), value)?;
        set.entries.push(RecordEntry {
            key: format!("loaf/debug/{name}/{key}"),
            revision: digest.clone(),
            value: digest,
        });
    }
    let mut wrong_entries = set
        .entries
        .iter()
        .map(|row| RecordEntry {
            key: row.key.clone(),
            revision: row.revision.clone(),
            value: row.value.clone(),
        })
        .collect::<Vec<_>>();
    let other_content = other_content.ok_or("missing second range publication")?;
    let first_row = wrong_entries
        .iter_mut()
        .find(|e| e.key.ends_with(&first_key))
        .ok_or("missing first range publication")?;
    first_row.revision = other_content.clone();
    first_row.value = other_content;
    let selected = thread_selection::select(RecordSet::new(root.path().into(), set.entries));
    let index = crate::loaf_index::LoafIndex::deferred([(name.to_owned(), name.to_owned(), dir.clone(), false)])?;
    let roots = BTreeSet::from([first.clone()]);
    let run = |index: &crate::loaf_index::LoafIndex| resolve_loaf_declarations(index, &roots, &BTreeSet::new());
    let accepted = run(&index)?;
    assert_eq!(accepted.decoded_declarations, 1);
    let reader = SurfaceReader::open(&bytes)?;
    let address = |id| -> Result<(usize, u64), Box<dyn std::error::Error>> {
        let DeclarationCoverage::Covered { offset, length, .. } = reader.index().coverage(id)? else {
            return Err("trait was not covered".into());
        };
        Ok((
            bytes.len() - reader.payload().len() + usize::try_from(*offset)?,
            *length,
        ))
    };
    let (first_offset, first_len) = address(first)?;
    let (other_offset, _) = address(other)?;
    assert_eq!(accepted.payload_bytes_read, first_len);
    assert_eq!(accepted.content_bytes_verified, first_len);
    let keys = thread_selection::read_keys();
    assert_eq!(keys.iter().filter(|key| key.contains("/executable-range/")).count(), 1);
    assert!(keys.iter().any(|key| key.ends_with(&first_key)));
    assert!(!keys.iter().any(|key| key.contains("/public-identities/")));
    let mut changed = bytes.clone();
    changed[other_offset] ^= 1;
    fs::write(&path, &changed)?;
    assert_eq!(run(&index)?.decoded_declarations, 1);
    changed[first_offset] ^= 1;
    fs::write(&path, &changed)?;
    assert!(
        run(&index)
            .expect_err("changed selected fragment must fail")
            .to_string()
            .contains("sealed digest")
    );
    fs::write(&path, &bytes)?;
    drop(selected);
    let _substituted = thread_selection::select(RecordSet::new(root.path().into(), wrong_entries));
    let fresh = crate::loaf_index::LoafIndex::deferred([(name.to_owned(), name.to_owned(), dir, false)])?;
    assert!(
        run(&fresh)
            .expect_err("substituted membership must fail")
            .to_string()
            .contains("another declaration")
    );
    Ok(())
}

#[test]
fn native_lowering_reads_a_loafs_runtime_bindings_by_name() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let manifest = surfaced_library()?;
    let set = installed_library(root.path(), &manifest)?;
    let dir = root.path().join("debug");
    // The index admits an installed Loaf by its inventory summary before anything demands its records.
    read_summary(&set, &dir, "prices")?.ok_or("the Loaf is installed")?;
    let indexed = set.reads();
    let (bindings, _) = read_runtime_bindings_in(&set, &dir, "prices")?.ok_or("the Loaf is installed")?;
    let expected = runtime_bindings(&manifest);
    assert!(!expected.is_empty(), "`prices::price` is a runtime callable");
    assert_eq!(*bindings, expected, "the bindings the manifest describes");
    let (records, bytes) = (set.reads().records - indexed.records, set.reads().bytes - indexed.bytes);
    assert_eq!(
        records, 1,
        "native lowering read {records} records, {bytes} bytes; it needs the runtime bindings record alone"
    );
    Ok(())
}

#[test]
fn both_parser_vocabularies_share_one_validated_registration_without_reading_the_contract()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let mut manifest = surfaced_library()?;
    let registration = super::VocabExports {
        metadata_version: incan_vocab::VOCAB_METADATA_VERSION,
        unit_identity: content_name(b"checked registration"),
        crate_path: "src/lib.incn".to_string(),
        package_name: manifest.name.clone(),
        keyword_registrations: Vec::new(),
        dsl_surfaces: Vec::new(),
        provider_manifest: Default::default(),
        desugarer_artifact: None,
    };
    manifest.vocab = Some(registration.clone());
    let selected = thread_selection::select(installed_library(root.path(), &manifest)?);
    let index = crate::loaf_index::LoafIndex::deferred([(
        manifest.name.clone(),
        manifest.name.clone(),
        root.path().join("debug"),
        false,
    )])?;
    let before = selected.0.reads();
    let _ = index.imported_vocab();
    let _ = index.imported_dsl_surfaces();
    index.validate_demands()?;
    // The resident worker opens independent parse and compile sessions against the same immutable set.
    let second = crate::loaf_index::LoafIndex::deferred([(
        manifest.name.clone(),
        manifest.name.clone(),
        root.path().join("debug"),
        false,
    )])?;
    let _ = second.imported_vocab();
    second.validate_demands()?;
    let after = selected.0.reads();
    assert_eq!(after.records - before.records, 1);
    assert_eq!(
        after.bytes - before.bytes,
        registration.encode_compiler_record_value()?.len()
    );
    assert!(
        !thread_selection::read_keys()
            .iter()
            .any(|key| key.contains("/contract-metadata/"))
    );
    let mut unsupported = registration;
    unsupported.metadata_version += 1;
    assert!(super::VocabExports::decode_compiler_record_value(&unsupported.encode_compiler_record_value()?).is_err());
    Ok(())
}

#[test]
fn runtime_planning_reads_only_the_individually_requested_facade() -> Result<(), Box<dyn std::error::Error>> {
    let name = "incan_native_runtime";
    let root = tempfile::tempdir()?;
    let manifest = surfaced_library_for(
        name,
        "pub def used() -> int:\n    return 1\n\npub def unused() -> int:\n    return 2\n",
    )?;
    let selected = thread_selection::select(installed_library(root.path(), &manifest)?);
    let dir = root.path().join("debug");
    let index = crate::loaf_index::LoafIndex::deferred([(name.to_owned(), name.to_owned(), dir, false)])?;
    let baseline = selected.0.reads();
    assert!(index.can_query_runtime_bindings(name));
    assert!(!index.can_query_runtime_bindings("different-package"));
    let used = format!("{name}::used");
    let unused = format!("{name}::unused");
    assert_eq!(selected.0.reads(), baseline);
    let binding = index.runtime_binding(name, &used)?.ok_or("expected the named facade")?;
    assert_eq!(binding.path, used);
    assert_eq!(index.runtime_binding(name, &used)?, Some(binding.clone()));
    assert!(index.runtime_binding(name, "incan_native_runtime::missing")?.is_none());
    let read = selected.0.reads();
    assert_eq!(read.records - baseline.records, 1);
    assert_eq!(
        read.bytes - baseline.bytes,
        LibraryManifest::encode_compiler_record_value(&binding)?.len()
    );
    assert!(
        !thread_selection::read_keys()
            .iter()
            .any(|key| key.contains("/runtime-bindings/") || key.ends_with(&content_name(unused.as_bytes())))
    );
    Ok(())
}

#[test]
fn an_aggregate_only_runtime_keeps_its_published_facade_and_native_absences() -> Result<(), Box<dyn std::error::Error>>
{
    let name = "incan_native_runtime";
    let root = tempfile::tempdir()?;
    let manifest = surfaced_library_for(name, "pub def used() -> int:\n    return 1\n")?;
    let mut set = installed_library(root.path(), &manifest)?;
    set.entries.retain(|entry| !entry.key.contains("/runtime-binding/"));
    let selected = thread_selection::select(set);
    let dir = root.path().join("debug");
    let index = crate::loaf_index::LoafIndex::deferred([(name.to_owned(), name.to_owned(), dir, false)])?;
    assert!(index.can_query_runtime_bindings(name));
    let binding = index
        .runtime_binding(name, "incan_native_runtime::used")?
        .ok_or("missing legacy binding")?;
    assert_eq!(binding.path, "incan_native_runtime::used");
    assert!(
        index
            .runtime_binding(name, "incan_native_runtime::unpublished")?
            .is_none()
    );
    assert_eq!(
        thread_selection::read_keys()
            .iter()
            .filter(|key| key.contains("/runtime-bindings/"))
            .count(),
        1
    );
    drop(selected);
    Ok(())
}

#[test]
fn an_aggregate_only_runtime_refuses_disagreeing_facade_owners() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let name = "incan_native_runtime";
    let manifest = surfaced_library_for(name, "pub def used() -> int:\n    return 1\n")?;
    let mut set = installed_library(root.path(), &manifest)?;
    set.entries.retain(|entry| !entry.key.contains("/runtime-binding/"));
    let mut bindings = runtime_bindings(&manifest);
    let original = bindings
        .iter()
        .find(|row| row.path == "incan_native_runtime::used")
        .ok_or("missing facade")?;
    let mut other = original.clone();
    other.canonical.declaration_name = "distinct_declaration".to_owned();
    bindings.push(other);
    let bytes = LibraryManifest::encode_compiler_record_value(&bindings)?;
    let digest = content_name(&bytes);
    fs::write(root.path().join("payloads").join(&digest), bytes)?;
    let aggregate = set
        .entries
        .iter_mut()
        .find(|entry| entry.key.contains("/runtime-bindings/"))
        .ok_or("missing aggregate")?;
    aggregate.value = digest.clone();
    aggregate.revision = digest;
    let _selected = thread_selection::select(set);
    let dir = root.path().join("debug");
    let index = crate::loaf_index::LoafIndex::deferred([(name.to_owned(), name.to_owned(), dir, false)])?;
    assert!(index.runtime_binding(name, "incan_native_runtime::used").is_err());
    Ok(())
}

#[test]
fn a_consumer_reads_one_declarations_signature_record_alone() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    // One copy of the checked API module, so `prices::price` names one declaration.
    let mut manifest = surfaced_library()?;
    if let Some(api) = manifest.contract_metadata.api.as_mut() {
        api.modules.truncate(1);
    }
    let set = installed_library(root.path(), &manifest)?;
    let dir = root.path().join("debug");
    // The index admits an installed Loaf by its inventory summary before anything demands its records.
    read_summary(&set, &dir, "prices")?.ok_or("the Loaf is installed")?;
    let indexed = set.reads();
    let (signatures, _) = read_declaration_in(&set, &dir, "prices", "declaration-signature", "prices::price")?
        .ok_or("the signature record is installed")?;
    assert_eq!(signatures.len(), 1, "one declaration named `prices::price`");
    assert_eq!(signatures[0]["name"], "price", "the signature of `price`");
    let (records, bytes) = (set.reads().records - indexed.records, set.reads().bytes - indexed.bytes);
    assert_eq!(
        records, 1,
        "the consumer read {records} records, {bytes} bytes; it needs the signature record alone"
    );
    Ok(())
}
