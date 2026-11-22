//! A namespace header carries a checked namespace record's admission facts, and nothing else.

use crate::checker_record_models::{GraphNamespaceValue, SettledCallRead};
use crate::checker_records::encode_graph_namespace_value;
use crate::namespace_headers::{
    NamespaceHeaderValue, decode_namespace_header, encode_namespace_header, namespace_header_of,
};

fn namespace() -> GraphNamespaceValue {
    GraphNamespaceValue {
        module: "src/domain.incn".into(),
        interface: b"interface bytes".to_vec(),
        body_ir: b"body ir bytes".to_vec(),
        lowering_inputs: vec![
            vec!["domain#decl.0..12".into(), "body:a".into()],
            vec!["domain#decl.14..30".into(), "body:b".into()],
        ],
        package_roots: vec!["pkg-a".into(), "pkg-b".into()],
        call_reads: vec![SettledCallRead {
            reader: "domain#decl.0..12".into(),
            target: "pkg.mod::f".into(),
            digest: format!("sha256:{}", "ab".repeat(32)),
        }],
        call_reads_known: true,
        identity_proof: b"proof bytes".to_vec(),
    }
}

/// A namespace record's header carries its admission facts exactly and none of its interface or Body IR bytes.
#[test]
fn a_namespace_header_carries_the_records_admission_facts_without_its_payloads() -> Result<(), String> {
    let value = namespace();
    let record = encode_graph_namespace_value(&value).map_err(|fault| format!("{fault:?}"))?;
    let header = namespace_header_of(&record)?.ok_or("a current namespace record has no header")?;
    assert!(header.len() < record.len());
    for payload in [&value.interface, &value.body_ir] {
        assert!(!header.windows(payload.len()).any(|window| window == payload.as_slice()));
    }
    let expected = NamespaceHeaderValue {
        module: value.module.clone(),
        identity_proof: value.identity_proof.clone(),
        lowering_inputs: value.lowering_inputs.clone(),
        package_roots: value.package_roots.clone(),
        call_reads: value.call_reads.clone(),
    };
    assert_eq!(decode_namespace_header(&header)?, expected);
    assert_eq!(encode_namespace_header(&expected)?, header);
    Ok(())
}

/// A record without the compiler's identity proof has no header, and a damaged header is refused.
#[test]
fn a_namespace_header_needs_a_proof_and_refuses_damage() -> Result<(), String> {
    let mut older = namespace();
    older.identity_proof.clear();
    let record = encode_graph_namespace_value(&older).map_err(|fault| format!("{fault:?}"))?;
    assert!(namespace_header_of(&record)?.is_none());
    let record = encode_graph_namespace_value(&namespace()).map_err(|fault| format!("{fault:?}"))?;
    let header = namespace_header_of(&record)?.ok_or("missing header")?;
    assert!(decode_namespace_header(&header[..header.len() - 1]).is_err());
    let mut unordered = decode_namespace_header(&header)?;
    unordered.lowering_inputs.reverse();
    assert!(encode_namespace_header(&unordered).is_err());
    Ok(())
}

/// A namespace header is its own record beside the namespace frame, under the same module identity, and Oven offers
/// its frame address to a compiler request in the same bounded reference form, on a page of its own.
#[test]
fn a_namespace_header_has_its_own_key_and_reference_page() -> Result<(), String> {
    use crate::namespace_references::{
        NamespaceReference, decode_namespace_references, encode_namespace_references, namespace_header_key,
        namespace_header_reference_filename, namespace_key, namespace_reference_filename,
    };
    let header = namespace_header_key("app", "src/domain.incn");
    let frame = namespace_key("app", "src/domain.incn");
    assert_eq!(header.identity, frame.identity);
    assert_eq!(header.identity_profile, frame.identity_profile);
    assert_ne!(header.family, frame.family);
    assert_ne!(namespace_header_reference_filename(), namespace_reference_filename());
    let reference = NamespaceReference {
        key: header,
        segment: "ab".repeat(32),
        offset: 0,
        length: 4096,
        frame_digest: vec![7; 32],
        value_digest: "cd".repeat(32),
    };
    let page = encode_namespace_references(std::slice::from_ref(&reference))?;
    assert!(decode_namespace_references(&page)? == vec![reference]);
    Ok(())
}
