use super::*;

/// Construct a nongeneric, lifetime-free type fixture for metadata shape classification tests.
fn dummy_type_metadata(path: &str) -> RustItemMetadata {
    RustItemMetadata {
        canonical_path: path.to_string(),
        definition_path: Some(path.to_string()),
        visibility: RustVisibility::Public,
        kind: RustItemKind::Type(RustTypeInfo {
            type_params: Vec::new(),
            type_param_defaults: Vec::new(),
            mutable_reference_type_params: Vec::new(),
            expanded_derive_traits: Vec::new(),
            has_const_params: false,
            has_lifetime_params: false,
            alias_target: None,
            metadata_completeness: Default::default(),
            methods: Vec::new(),
            implemented_traits: Vec::new(),
            fields: Vec::new(),
            variants: Vec::new(),
        }),
    }
}

#[test]
fn collection_family_matches_supported_map_and_set_paths() {
    for (path, expected) in [
        ("std::collections::HashMap", RustCollectionFamily::HashMap),
        ("hashbrown::HashMap", RustCollectionFamily::HashMap),
        ("hashbrown::map::HashMap<K, V>", RustCollectionFamily::HashMap),
        ("std::collections::BTreeMap", RustCollectionFamily::BTreeMap),
        ("std::collections::HashSet", RustCollectionFamily::HashSet),
        ("hashbrown::set::HashSet<T>", RustCollectionFamily::HashSet),
        ("std::collections::BTreeSet", RustCollectionFamily::BTreeSet),
        ("std::collections::HashMap<String, i64>", RustCollectionFamily::HashMap),
    ] {
        let meta = dummy_type_metadata(path);
        assert_eq!(meta.collection_family(), Some(expected), "path `{path}`");
    }
}

#[test]
fn collection_family_matches_incan_and_imported_type_names() {
    for (name, expected) in [
        ("Dict", RustCollectionFamily::HashMap),
        ("HashMap", RustCollectionFamily::HashMap),
        ("Set", RustCollectionFamily::HashSet),
        ("BTreeMap", RustCollectionFamily::BTreeMap),
        ("BTreeSet", RustCollectionFamily::BTreeSet),
    ] {
        assert_eq!(
            RustCollectionFamily::for_type_name(name),
            Some(expected),
            "name `{name}`"
        );
    }
}

#[test]
fn collection_family_reports_lookup_methods_that_preserve_arg_shape() {
    assert!(RustCollectionFamily::HashMap.preserves_lookup_arg_shape("get"));
    assert!(RustCollectionFamily::HashMap.preserves_lookup_arg_shape("contains_key"));
    assert!(!RustCollectionFamily::HashMap.preserves_lookup_arg_shape("insert"));
    assert!(RustCollectionFamily::HashSet.preserves_lookup_arg_shape("contains"));
    assert!(!RustCollectionFamily::HashSet.preserves_lookup_arg_shape("insert"));
}

/// Preserve nested structural argument boundaries, including function pointers inside iterator storage.
#[test]
fn parse_rust_type_shape_text_classifies_shared_structural_shapes() {
    let shape = parse_rust_type_shape_text(
        "&'static Result<Vec<u8>, demo::Error>",
        |path| (path == "demo::Error").then(|| "demo::Error".to_string()),
        RustTypeShapePathFallback::Unknown,
    );

    assert_eq!(
        shape,
        RustTypeShape::Ref(Box::new(RustTypeShape::Result(
            Box::new(RustTypeShape::Bytes),
            Box::new(RustTypeShape::RustPath {
                path: "demo::Error".to_string(),
                args: Vec::new(),
            }),
        )))
    );
    assert_eq!(
        split_top_level_rust_args(
            "TakeWhile<Map<Repeat<i64>, fn(i64) -> Option<i64>>, fn(&Option<i64>) -> bool>, fn(Option<i64>) -> i64"
        ),
        vec![
            "TakeWhile<Map<Repeat<i64>, fn(i64) -> Option<i64>>, fn(&Option<i64>) -> bool>",
            "fn(Option<i64>) -> i64",
        ],
    );
}

#[test]
fn parse_rust_type_shape_text_keeps_source_and_generated_fallbacks_distinct() {
    assert_eq!(
        parse_rust_type_shape_text("missing_type", |_| None, RustTypeShapePathFallback::Unknown),
        RustTypeShape::Unknown,
    );
    assert_eq!(
        parse_rust_type_shape_text("missing_type", |_| None, RustTypeShapePathFallback::RustPath),
        RustTypeShape::RustPath {
            path: "missing_type".to_string(),
            args: Vec::new(),
        },
    );
    assert_eq!(
        parse_rust_type_shape_text("T", |_| None, RustTypeShapePathFallback::RustPath),
        RustTypeShape::TypeParam("T".to_string()),
    );
}

#[test]
fn parse_rust_type_shape_text_preserves_never_type() {
    let shape = parse_rust_type_shape_text(RUST_NEVER_TYPE_DISPLAY, |_| None, RustTypeShapePathFallback::RustPath);

    assert_eq!(shape, RustTypeShape::Never);
    assert_eq!(render_rust_type_shape(&shape), RUST_NEVER_TYPE_DISPLAY);
}

fn normalize_probe_type(text: &str) -> Option<String> {
    let normalized = text.trim().replace(' ', "");
    if let Some(inner) = normalized.strip_prefix("&mut") {
        return normalize_probe_type(inner).map(|inner| format!("&mut {inner}"));
    }
    if let Some(inner) = normalized.strip_prefix('&') {
        return normalize_probe_type(inner).map(|inner| format!("&{inner}"));
    }
    Some(match normalized.as_str() {
        "Data" => "source_dep::audio::Data".to_string(),
        "OutputCallbackInfo" => "source_dep::audio::OutputCallbackInfo".to_string(),
        other => other.to_string(),
    })
}

#[test]
fn rust_source_callable_bound_for_type_param_reads_inline_generic_bounds() {
    let source = r#"
pub fn run_inline<D: FnMut(&mut Data, &OutputCallbackInfo) + Send + 'static>(callback: D) {
    let _ = callback;
}
"#;

    assert_eq!(
        rust_source_callable_bound_for_type_param(source, "D", normalize_probe_type).as_deref(),
        Some("impl FnMut(&mut source_dep::audio::Data, &source_dep::audio::OutputCallbackInfo)")
    );
}

#[test]
fn rust_source_callable_bound_for_type_param_reads_where_bounds() {
    let source = r#"
pub fn run_where<D, E>(callback: D, error: E)
where
    D: FnMut(&mut Data, &OutputCallbackInfo) + Send + 'static,
    E: FnMut(String),
{
    let _ = callback;
    let _ = error;
}
"#;

    assert_eq!(
        rust_source_callable_bound_for_type_param(source, "D", normalize_probe_type).as_deref(),
        Some("impl FnMut(&mut source_dep::audio::Data, &source_dep::audio::OutputCallbackInfo)")
    );
    assert_eq!(
        rust_source_callable_bound_for_type_param(source, "E", normalize_probe_type).as_deref(),
        Some("impl FnMut(String)")
    );
}

#[test]
fn rust_source_type_param_has_as_fd_bound_reads_inline_and_where_bounds() {
    let inline = r#"
pub fn lock<Fd: std::os::fd::AsFd>(fd: Fd) {
    let _ = fd;
}
"#;
    let where_bound = r#"
pub fn lock<Fd>(fd: Fd)
where
    Fd: AsFd + Send,
{
    let _ = fd;
}
"#;

    assert!(rust_source_type_param_has_as_fd_bound(inline, "Fd"));
    assert!(rust_source_type_param_has_as_fd_bound(where_bound, "Fd"));
    assert!(!rust_source_type_param_has_as_fd_bound(inline, "Other"));
}

#[test]
fn rust_source_borrowed_type_param_bound_display_preserves_shared_and_mutable_bounds() {
    let shared = r#"
pub fn shared<S: AsRef<OsStr> + ?Sized>(path: &S) {}
"#;
    let mutable = r#"
pub fn mutable<S>(path: &mut S)
where
    S: AsMut<[u8]> + Send,
{}
"#;

    assert_eq!(
        rust_source_borrowed_type_param_bound_display(shared, "&S").as_deref(),
        Some("&impl AsRef<OsStr>")
    );
    assert_eq!(
        rust_source_borrowed_type_param_bound_display(mutable, "&mut S").as_deref(),
        Some("&mut impl AsMut<[u8]> + Send")
    );
}

#[test]
fn rust_source_callable_bound_for_type_param_accepts_qualified_fn_traits() {
    let source = r#"
pub fn run_qualified<D: std::ops::FnOnce(Data) -> OutputCallbackInfo>(callback: D);
"#;

    assert_eq!(
        rust_source_callable_bound_for_type_param(source, "D", normalize_probe_type).as_deref(),
        Some("impl FnOnce(source_dep::audio::Data) -> source_dep::audio::OutputCallbackInfo")
    );
}

#[test]
fn rust_source_callable_bound_for_type_param_skips_incidental_fn_tokens() {
    let source = r#"
const SAMPLE: &str = "fn ";

pub fn run_inline<D: FnMut(&mut Data, &OutputCallbackInfo)>(callback: D) {
    let _ = callback;
}
"#;

    assert_eq!(
        rust_source_callable_bound_for_type_param(source, "D", normalize_probe_type).as_deref(),
        Some("impl FnMut(&mut source_dep::audio::Data, &source_dep::audio::OutputCallbackInfo)")
    );
}

#[test]
fn rust_display_is_callable_bound_detects_only_callable_bound_displays() {
    assert!(rust_display_is_callable_bound(
        "impl FnMut(&mut demo::Data, &demo::Info) + Send"
    ));
    assert!(rust_display_is_callable_bound("dyn std::ops::Fn(String)"));
    assert!(!rust_display_is_callable_bound("impl Buf"));
    assert!(!rust_display_is_callable_bound("implBuf"));
}

/// Metadata-free standard signatures retain their named input and Result output without incidental metadata.
#[test]
fn metadata_free_function_signature_describes_stable_std_result_surfaces() -> Result<(), String> {
    let signature = metadata_free_function_signature("std::fs::metadata")
        .ok_or("std::fs::metadata should have a metadata-free signature")?;

    assert_eq!(signature.return_type, "std::io::Result<std::fs::Metadata>");
    assert_eq!(signature.params.len(), 1);
    assert_eq!(signature.params[0].name.as_deref(), Some("path"));
    assert_eq!(signature.params[0].type_display, "impl AsRef<std::path::Path>");
    assert!(!signature.is_async);
    assert!(!signature.is_unsafe);
    assert!(metadata_free_function_signature("std::fs::remove_file").is_none());
    Ok(())
}

#[test]
fn compiler_owned_function_signatures_preserve_owned_stdlib_slice_results() -> Result<(), String> {
    let expected = [
        (
            "incan_std_core::strings::str_slice_byte_range",
            vec![(Some("s"), "&str"), (Some("start"), "i64"), (Some("end"), "i64")],
        ),
        (
            "incan_std_core::strings::str_slice_from_byte_offset",
            vec![(Some("s"), "&str"), (Some("start"), "i64")],
        ),
    ];

    for (path, expected_params) in expected {
        let signature = compiler_owned_function_signature(path)
            .ok_or_else(|| format!("{path} should have a compiler-owned signature"))?;
        assert!(signature.type_params.is_empty(), "{path}");
        assert_eq!(signature.return_type, "String", "{path}");
        assert_eq!(signature.params.len(), expected_params.len(), "{path}");
        for (actual, (expected_name, expected_type)) in signature.params.iter().zip(expected_params) {
            assert_eq!(actual.name.as_deref(), expected_name, "{path}");
            assert_eq!(actual.type_display, expected_type, "{path}");
        }
        assert!(!signature.is_async, "{path}");
        assert!(!signature.is_unsafe, "{path}");
    }
    assert!(compiler_owned_function_signature("demo::str_slice_byte_range").is_none());
    Ok(())
}

#[test]
fn metadata_free_function_signature_preserves_stdin_receiver_type() -> Result<(), String> {
    let Some(signature) = metadata_free_function_signature("std::io::stdin") else {
        return Err("std::io::stdin should have a metadata-free signature".to_string());
    };

    assert_eq!(signature.return_type, "std::io::Stdin");
    assert!(signature.params.is_empty());
    assert!(!signature.is_async);
    assert!(!signature.is_unsafe);
    Ok(())
}

#[test]
fn metadata_free_function_signature_preserves_stdout_receiver_type() -> Result<(), String> {
    let Some(signature) = metadata_free_function_signature("std::io::stdout") else {
        return Err("std::io::stdout should have a metadata-free signature".to_string());
    };

    assert_eq!(signature.return_type, "std::io::Stdout");
    assert!(signature.params.is_empty());
    assert!(!signature.is_async);
    assert!(!signature.is_unsafe);
    Ok(())
}
