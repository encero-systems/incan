//! Ported from internal_models/tests: constructor rows, checked types, checking bases and import closures, plan
//! entries, overload members, checker publication counts and manifest caches.

use crate::checked_types::{
    CheckedCallableParam, CheckedCallableParamKind, CheckedInputAccess, CheckedNumericType, CheckedPrimitiveType,
    CheckedRuntimeBuiltin, CheckedType, valid_checked_type,
};
use crate::checker_publication::{CheckerRecordCounts, decode_checker_counts, encode_checker_counts};
use crate::checker_record_models::CodecFault;
use crate::checker_records::{CHECKER_PROFILE, from_native_final_fields};
use crate::compiler_records::{CanonicalSymbolId, DeclarationKind, HirSourceSpan, SymbolNamespace, SymbolOrigin};
use crate::manifest_cache::{
    ManifestInventory, decode_manifest_cache, decode_manifest_inventory, encode_manifest_cache,
    encode_manifest_inventory,
};
use crate::namespace_order::{
    certified_namespace_order, checking_basis_key, encode_checking_basis, encode_import_closures, read_checking_basis,
    read_import_closures,
};
use crate::overload_members::{
    encode_method_lookups, encode_method_members, method_member_request, read_method_lookups, read_method_members,
};
use crate::record_scope::{ReadScope, RecordKey, RecordRead};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| (*item).to_owned()).collect()
}

/// Name why one final record holding a single constructor row is refused, or return empty when it validates.
fn refused(family: &str, selector: &str, values: &[&str]) -> String {
    let root = format!("sha256:{}", "a".repeat(64));
    let mut row = strings(&[family, "node/Function/body/0/span", selector]);
    row.extend(strings(values));
    let rows = vec![
        strings(&["header", "function:main::value", &root, CHECKER_PROFILE]),
        row,
        strings(&["status", "", "refused", "false"]),
    ];
    match from_native_final_fields(&rows) {
        Ok(_) => String::new(),
        Err(CodecFault::Unsupported(name)) => name,
        Err(CodecFault::Malformed) => "malformed".into(),
        Err(_) => "other".into(),
    }
}

#[test]
fn constructor_rows_refuse_a_malformed_binding() {
    assert_eq!(refused("constructor", "field-binding", &["2", "1", "0", "1", "1"]), "");
    assert_eq!(refused("constructor", "field-binding", &["2", "2", "1", "0", "0"]), "");
    assert_eq!(refused("requirement", "constructor-field", &["field", "owner"]), "");
    assert_eq!(
        refused("constructor", "field-binding", &["2", "1", "0", "0"]),
        "malformed"
    );
    assert_eq!(
        refused("constructor", "field-binding", &["2", "2", "0", "0", "0"]),
        "malformed"
    );
    assert_eq!(
        refused("constructor", "field-binding", &["3", "1", "0", "2", "2", "1"]),
        "malformed"
    );
    assert_eq!(
        refused("constructor", "field-binding", &["1", "1", "00", "0"]),
        "malformed"
    );
    assert_eq!(
        refused("constructor", "field-binding", &["1", "2", "0", "0"]),
        "malformed"
    );
    assert_eq!(refused("requirement", "constructor-field", &["field", ""]), "malformed");
    assert_eq!(refused("constructor", "arguments", &["0", "0", "0"]), "malformed");
}

#[test]
fn checked_types_retain_every_alternative() {
    let trait_identity = CanonicalSymbolId {
        namespace: SymbolNamespace::OrdinaryLexical,
        origin: SymbolOrigin::Module(strings(&["contracts"])),
        declaration_name: "Readable".into(),
        kind: DeclarationKind::Trait,
        scope_discriminant: None,
        declaration_span: HirSourceSpan {
            file: "src/main.incn".into(),
            start: 4,
            end: 12,
            expansion: None,
        },
    };
    let parameter = |ty: CheckedType| CheckedCallableParam {
        name: Some("value".into()),
        ty,
        kind: CheckedCallableParamKind::Normal,
        has_default: true,
        is_partial_preset: true,
        is_mut: true,
        native_owned: true,
        input_access: CheckedInputAccess::Exclusive,
    };
    let var = || CheckedType::TypeVar("T".into());
    let types = vec![
        CheckedType::Never,
        CheckedType::Primitive(CheckedPrimitiveType::Numeric(CheckedNumericType::I64)),
        CheckedType::Named("Invoice".into()),
        CheckedType::Generic("Result".into(), vec![var(), var()]),
        CheckedType::Decimal(38, 4),
        CheckedType::Function(vec![parameter(var())], vec![var()]),
        CheckedType::TypeToken(vec![CheckedType::Named("Invoice".into())]),
        CheckedType::Tuple(vec![CheckedType::Primitive(CheckedPrimitiveType::Bool)]),
        var(),
        CheckedType::SelfType,
        CheckedType::Ref(vec![var()]),
        CheckedType::RefMut(vec![var()]),
        CheckedType::RustInteropPath("std::path::PathBuf".into()),
        CheckedType::NativeBuiltin(CheckedRuntimeBuiltin::ValidationError),
        CheckedType::Unsupported("opaque".into()),
        CheckedType::Infer,
        CheckedType::Unknown,
        CheckedType::NativeArray(vec![CheckedType::Primitive(CheckedPrimitiveType::Bytes)], u64::MAX),
        CheckedType::NativeTraitObject("std::fmt::Debug".into(), true, false),
        CheckedType::NativePointer(true, vec![CheckedType::NativeCChar]),
        CheckedType::NativeCChar,
        CheckedType::Trait(trait_identity),
    ];
    assert_eq!(types.len(), 22);
    assert!(types.iter().all(valid_checked_type));
    assert_ne!(var(), CheckedType::Named("T".into()));
    assert_ne!(CheckedType::Ref(vec![var()]), CheckedType::RefMut(vec![var()]));
    let owner_then_method = CheckedType::Function(vec![parameter(var())], vec![CheckedType::Ref(vec![var()])]);
    let method_then_owner = CheckedType::Function(vec![parameter(CheckedType::Ref(vec![var()]))], vec![var()]);
    assert_ne!(owner_then_method, method_then_owner);
}

fn checked_key(family: &str) -> RecordKey {
    RecordKey {
        family: family.into(),
        identity_profile: "stable".into(),
        origin: "project".into(),
        identity: "owner".into(),
        facet: "checked".into(),
        context: "first".into(),
    }
}

#[test]
fn checking_basis_requires_the_matching_output_and_digest() -> Result<(), String> {
    let (local, settled) = (checked_key("local"), checked_key("final"));
    let root: String = Sha256::digest(b"completed receipt")
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    assert!(checking_basis_key(&local)?.starts_with("incan.compiler.checking-basis/2\t"));
    let encoded = encode_checking_basis(&local, &root)?;
    assert!(encoded.starts_with(b"CRMB") && read_checking_basis(&local, &encoded)? == root);
    assert!(read_checking_basis(&settled, &encoded).is_err());
    assert!(read_checking_basis(&local, root.as_bytes()).is_err());
    assert!(encode_checking_basis(&local, &root[..12]).is_err());
    Ok(())
}

#[test]
fn compiler_import_closures_certify_modules_without_namespace_bodies() -> Result<(), String> {
    let rows = vec![strings(&["helpers.incn"]), strings(&["main.incn", "helpers.incn"])];
    let imports: HashMap<String, Vec<String>> = read_import_closures(&encode_import_closures(&rows)?)?;
    let order = certified_namespace_order("app", &imports)?;
    assert_eq!((order["helpers.incn"].rank, order["main.incn"].rank), (0, 1));
    let set = |names: &[&str]| names.iter().map(|name| (*name).to_owned()).collect::<HashSet<String>>();
    assert!(order["main.incn"].covers(&set(&["helpers.incn", "main.incn"]))?);
    assert!(!order["main.incn"].covers(&set(&["unrelated.incn"]))?);
    assert!(!order["helpers.incn"].covers(&set(&["main.incn", "helpers.incn"]))?);
    assert!(read_import_closures(&encode_import_closures(&[strings(&["main.incn", "absent.incn"])])?).is_err());
    assert!(
        read_import_closures(&encode_import_closures(&[
            strings(&["main.incn"]),
            strings(&["main.incn"])
        ])?)
        .is_err()
    );
    Ok(())
}

#[test]
fn only_a_complete_scoped_lookup_can_authenticate_overload_members() -> Result<(), String> {
    let rows = vec![strings(&[
        "reader",
        "main",
        "Describable",
        "describe",
        "signature:int",
        "signature:str",
    ])];
    let lookups = read_method_lookups(&encode_method_lookups(&rows)?)?;
    let candidates = lookups.groups["reader"][0].candidates.clone();
    assert_eq!(candidates, strings(&["signature:int", "signature:str"]));
    let member_key = |origin: &str| RecordKey {
        family: "overload-members".into(),
        identity_profile: "stable".into(),
        origin: origin.into(),
        identity: "main".into(),
        facet: "Describable.describe".into(),
        context: "instance".into(),
    };
    let scope = ReadScope {
        snapshot: "snapshot".into(),
        profile: "checking".into(),
    };
    let page = encode_method_members(&member_key("app"), &candidates)?;
    let RecordRead::Members(read) = read_method_members(&scope, &method_member_request(&member_key("app")), &page)
    else {
        return Err("missing lookup receipt".into());
    };
    assert!(read.value == b"signature:int\nsignature:str" && read.continuation == "end");
    let foreign = read_method_members(&scope, &method_member_request(&member_key("other-app")), &page);
    assert!(matches!(foreign, RecordRead::Failed(_)));
    assert!(encode_method_members(&member_key("app"), &strings(&["signature:str", "signature:int"])).is_err());
    assert!(read_method_lookups(&encode_method_lookups(&[strings(&["reader", "main"])])?).is_err());
    Ok(())
}

#[test]
fn checker_publication_round_trip() -> Result<(), String> {
    let counted = CheckerRecordCounts {
        all_declarations: 3,
        reusable_declarations: 2,
        summary: "2/3 reusable declarations".into(),
        artifact_refusals: HashMap::from([("a".to_owned(), 2), ("z".to_owned(), 1)]),
        restored: strings(&["main"]),
        restored_body_ir: strings(&["other"]),
    };
    let encoded = encode_checker_counts(&counted)?;
    assert!(encoded.starts_with(b"CRMB"));
    let restored = decode_checker_counts(&encoded)?;
    assert_eq!(restored, counted);
    let repeated = CheckerRecordCounts {
        all_declarations: 2,
        reusable_declarations: 2,
        restored_body_ir: strings(&["main", "main"]),
        ..CheckerRecordCounts::default()
    };
    assert!(encode_checker_counts(&repeated).is_err());
    assert_eq!(encode_checker_counts(&restored)?, encoded);
    assert!(decode_checker_counts(br#"{"schema":"incan.checker-reuse/1"}"#).is_err());
    let excessive = CheckerRecordCounts {
        all_declarations: 1,
        reusable_declarations: 2,
        ..CheckerRecordCounts::default()
    };
    assert!(encode_checker_counts(&excessive).is_err());
    Ok(())
}

/// A facts record round-trips a payload close to its 128 MiB bound.
///
/// The decoded facts of a dependency whose manifest embeds a large Rust ABI (internal_models: 176 MB of JSON) are
/// retained in this record. Its codec work budget must cover every byte of a record the family admits, or the cache is
/// never written and each compiler request parses the JSON again.
#[test]
fn a_facts_record_near_its_bound_round_trips() -> Result<(), String> {
    let guard = Sha256::digest(b"observed source and compiler").to_vec();
    let digest = Sha256::digest(b"public manifest").to_vec();
    let payload: Vec<u8> = (0..120 * 1024 * 1024).map(|index: usize| (index % 251) as u8).collect();
    let facts = encode_manifest_cache("facts", &guard, &digest, &payload)?;
    let recovered = decode_manifest_cache("facts", &guard, &facts)?;
    assert!(recovered.digest == digest && recovered.payload == payload);
    Ok(())
}

#[test]
fn retained_manifest_evidence() -> Result<(), String> {
    let guard = Sha256::digest(b"observed source and compiler").to_vec();
    let other = Sha256::digest(b"changed source or compiler").to_vec();
    let digest = Sha256::digest(b"public manifest").to_vec();
    let identity = encode_manifest_cache("identity", &guard, &digest, b"")?;
    assert!(identity.starts_with(b"CRMB"));
    assert_eq!(decode_manifest_cache("identity", &guard, &identity)?.digest, digest);
    assert!(decode_manifest_cache("identity", &other, &identity).is_err());
    assert!(decode_manifest_cache("facts", &guard, &identity).is_err());
    assert!(decode_manifest_cache("identity", &guard, b"{\"version\":1}").is_err());
    let row = ManifestInventory {
        name: "example".into(),
        version: "1.2.3".into(),
        features: strings(&["a", "b"]),
        vocabulary: true,
        operations: false,
        native_unions: true,
        rust_paths: strings(&["native::A", "native::B"]),
        declares_abi_cache: true,
        has_abi_cache: false,
        abi_digest: Some("sha256:abc".into()),
    };
    let summary = encode_manifest_inventory(&row)?;
    let envelope = encode_manifest_cache("inventory", &guard, &digest, &summary)?;
    let recovered = decode_manifest_cache("inventory", &guard, &envelope)?;
    let decoded = decode_manifest_inventory(&recovered.payload)?;
    assert!(recovered.digest == digest && encode_manifest_inventory(&decoded)? == summary);
    assert_eq!(decoded, row);
    let payload = b"selected native facts";
    let facts = encode_manifest_cache("facts", &guard, &digest, payload)?;
    assert_eq!(decode_manifest_cache("facts", &guard, &facts)?.payload, payload);
    assert!(decode_manifest_cache("facts", &other, &facts).is_err());
    let index = facts
        .windows(payload.len())
        .position(|window| window == payload)
        .ok_or("payload not stored")?;
    let mut damaged = facts[..index].to_vec();
    damaged.extend_from_slice(b"changed! native facts");
    damaged.extend_from_slice(&facts[index + payload.len()..]);
    assert!(decode_manifest_cache("facts", &guard, &damaged).is_err());
    Ok(())
}
