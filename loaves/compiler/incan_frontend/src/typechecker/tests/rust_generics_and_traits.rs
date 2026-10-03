//! Explicit type arguments on Rust methods and associated calls (owner generics, metadata-declared arity, compiled
//! re-export receivers), Rust extension traits and associated types (#834), `@rust.derive`, and sealed re-export
//! identity under the Oven authority.

use super::*;

#[cfg(feature = "rust_inspect")]
#[test]
fn invalid_sealed_oven_authority_refuses_cached_or_ambient_identity_lookup() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = seeded_rust_inspect_workspace()?;
    let manifest_dir = tmp.path().to_path_buf();
    fs::write(
        manifest_dir.join(rust_inspect::OVEN_DIRECT_INSPECTION_AUTHORITY_FILE),
        "{ not valid json",
    )?;

    let mut checker = TypeChecker::new();
    checker.set_rust_inspect_manifest_dir(manifest_dir.clone());
    assert!(matches!(
        checker.rust_inspect_registry_source_authority,
        RustInspectRegistrySourceAuthority::Invalid
    ));
    checker.rust_inspect_cache.insert_test_item(
        &manifest_dir,
        RustItemMetadata {
            canonical_path: "demo::Thing".to_string(),
            definition_path: Some("demo::Thing".to_string()),
            visibility: RustVisibility::Public,
            kind: RustItemKind::Type(RustTypeInfo {
                type_params: Vec::new(),
                type_param_defaults: Vec::new(),
                mutable_reference_type_params: Vec::new(),
                expanded_derive_traits: Vec::new(),
                has_const_params: false,
                alias_target: None,
                metadata_completeness: Default::default(),
                methods: Vec::new(),
                implemented_traits: Vec::new(),
                fields: Vec::new(),
                variants: Vec::new(),
            }),
        },
    )?;

    assert!(checker.rust_item_metadata_for_path("demo::Thing").is_none());
    assert!(checker.rust_item_metadata_for_path_blocking("demo::Thing").is_none());
    Ok(())
}

#[cfg(feature = "rust_inspect")]
#[test]
fn sealed_rust_reexport_identity_crosses_the_call_boundary_without_accepting_distinct_types()
-> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let root = tmp.path().join("probe");
    let facade = tmp.path().join("facade-0.1.0");
    let inner = tmp.path().join("inner-0.1.0");
    fs::create_dir_all(root.join("src"))?;
    fs::create_dir_all(facade.join("src"))?;
    fs::create_dir_all(inner.join("src"))?;
    fs::write(
        root.join("Cargo.toml"),
        r#"[package]
name = "sealed_identity_probe"
version = "0.1.0"
edition = "2021"

[dependencies]
facade = "0.1.0"
"#,
    )?;
    fs::write(root.join("src/lib.rs"), "pub fn probe() {}\n")?;
    fs::write(
        root.join("Cargo.lock"),
        r#"version = 3

[[package]]
name = "sealed_identity_probe"
version = "0.1.0"
dependencies = ["facade 0.1.0"]

[[package]]
name = "facade"
version = "0.1.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "facade-checksum"
dependencies = ["inner 0.1.0"]

[[package]]
name = "inner"
version = "0.1.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "inner-checksum"
"#,
    )?;
    fs::write(
        facade.join("Cargo.toml"),
        r#"[package]
name = "facade"
version = "0.1.0"
edition = "2021"

[dependencies]
inner = "0.1.0"
"#,
    )?;
    fs::write(
        facade.join("src/lib.rs"),
        "pub use inner::Vec2;\npub struct Other;\npub fn consume(_value: Vec2) {}\n",
    )?;
    fs::write(
        inner.join("Cargo.toml"),
        r#"[package]
name = "inner"
version = "0.1.0"
edition = "2021"
"#,
    )?;
    fs::write(inner.join("src/lib.rs"), "pub struct Vec2;\n")?;
    fs::write(root.join(rust_inspect::OVEN_DIRECT_INSPECTION_MARKER), "sealed\n")?;
    let authority = format!(
        r#"{{"schema_version":2,"source_validation":"sealed_oven_selection","sources":[{{"package":"facade","version":"0.1.0","registry":"registry+https://github.com/rust-lang/crates.io-index","checksum":"facade-checksum","features":[],"source_root":{},"source_digest":"sha256:facade"}},{{"package":"inner","version":"0.1.0","registry":"registry+https://github.com/rust-lang/crates.io-index","checksum":"inner-checksum","features":[],"source_root":{},"source_digest":"sha256:inner"}}]}}"#,
        serde_json::to_string(&facade)?,
        serde_json::to_string(&inner)?,
    );
    fs::write(
        root.join(rust_inspect::OVEN_DIRECT_INSPECTION_AUTHORITY_FILE),
        authority,
    )?;

    let accepted = r#"
from rust::facade import Vec2, consume

def pass_value(value: Vec2) -> None:
  consume(value)
"#;
    let accepted_tokens = lexer::lex(accepted).map_err(|errors| format!("accepted lex failed: {errors:?}"))?;
    let accepted_ast =
        parser::parse(&accepted_tokens).map_err(|errors| format!("accepted parse failed: {errors:?}"))?;
    let mut accepted_checker = TypeChecker::new();
    accepted_checker.set_rust_inspect_manifest_dir(root.clone());
    accepted_checker
        .check_program(&accepted_ast)
        .map_err(|errors| format!("facade Vec2 should satisfy inner Vec2 boundary: {errors:?}"))?;

    let rejected = r#"
from rust::facade import Other, consume

def reject_value(value: Other) -> None:
  consume(value)
"#;
    let rejected_tokens = lexer::lex(rejected).map_err(|errors| format!("rejected lex failed: {errors:?}"))?;
    let rejected_ast =
        parser::parse(&rejected_tokens).map_err(|errors| format!("rejected parse failed: {errors:?}"))?;
    let mut rejected_checker = TypeChecker::new();
    rejected_checker.set_rust_inspect_manifest_dir(root);
    let errors = rejected_checker.check_program(&rejected_ast).err().ok_or_else(|| {
        std::io::Error::other("a distinct Rust nominal type must not satisfy the facade Vec2 boundary")
    })?;
    assert!(
        errors.iter().any(|error| error.message.contains("Type mismatch")),
        "expected a Rust boundary mismatch for facade::Other, got {errors:?}"
    );
    Ok(())
}

#[cfg(feature = "rust_inspect")]
#[test]
fn test_rust_extension_trait_method_call_records_selected_import_binding() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
from rust::demo import AlphaRender, BetaRender, Widget

def f(w: Widget) -> None:
  _ = w.render()
"#;
    let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("lex failed: {errs:?}")))?;
    let ast = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("parse failed: {errs:?}")))?;
    let mut checker = TypeChecker::new();
    let tmp = seeded_rust_inspect_workspace()?;
    let manifest_dir = tmp.path().to_path_buf();
    checker.set_rust_inspect_manifest_dir(manifest_dir.clone());
    for trait_name in ["AlphaRender", "BetaRender"] {
        checker
            .rust_inspect_cache
            .insert_test_item(
                &manifest_dir,
                RustItemMetadata {
                    canonical_path: format!("demo::{trait_name}"),
                    definition_path: Some(format!("demo::{trait_name}")),
                    visibility: RustVisibility::Public,
                    kind: RustItemKind::Trait(RustTraitInfo {
                        items: vec![RustTraitAssoc::Function {
                            name: "render".to_string(),
                            signature: RustFunctionSig {
                                receiver_contract: None,
                                type_params: Vec::new(),
                                params: vec![RustParam {
                                    name: Some("self".to_string()),
                                    type_display: "&self".to_string(),
                                }],
                                return_type: "String".to_string(),
                                is_async: false,
                                is_unsafe: false,
                            },
                        }],
                        derive_macro: None,
                    }),
                },
            )
            .map_err(|err| std::io::Error::other(format!("seed trait metadata: {err}")))?;
    }
    checker
        .rust_inspect_cache
        .insert_test_item(
            &manifest_dir,
            RustItemMetadata {
                canonical_path: "demo::Widget".to_string(),
                definition_path: Some("demo::Widget".to_string()),
                visibility: RustVisibility::Public,
                kind: RustItemKind::Type(RustTypeInfo {
                    type_params: Vec::new(),
                    type_param_defaults: Vec::new(),
                    mutable_reference_type_params: Vec::new(),
                    expanded_derive_traits: Vec::new(),
                    has_const_params: false,
                    alias_target: None,
                    metadata_completeness: Default::default(),
                    methods: Vec::new(),
                    implemented_traits: vec![RustImplementedTrait {
                        path: "demo::AlphaRender".to_string(),
                        mutable_reference: false,
                    }],
                    fields: Vec::new(),
                    variants: Vec::new(),
                }),
            },
        )
        .map_err(|err| std::io::Error::other(format!("seed type metadata: {err}")))?;

    checker
        .check_program(&ast)
        .map_err(|errs| std::io::Error::other(format!("typecheck failed: {errs:?}")))?;
    let uses = &checker.type_info().rust.method_trait_import_uses;
    assert!(
        uses.values()
            .any(|import_use| import_use.binding == "AlphaRender" && import_use.method == "render"),
        "expected AlphaRender import use, got {uses:?}"
    );
    assert!(
        !uses.values().any(|import_use| import_use.binding == "BetaRender"),
        "BetaRender should not be selected for Widget.render(): {uses:?}"
    );
    Ok(())
}

#[cfg(feature = "rust_inspect")]
#[test]
fn rust_extension_trait_method_type_args_require_metadata_declared_arity_issue834()
-> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
from rust::demo import Device, DeviceTrait

def open(device: Device) -> None:
  device.build_output_stream[f32]()
"#;
    let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("lex failed: {errs:?}")))?;
    let ast = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("parse failed: {errs:?}")))?;
    let mut checker = TypeChecker::new();
    let tmp = seeded_rust_inspect_workspace()?;
    let manifest_dir = tmp.path().to_path_buf();
    checker.set_rust_inspect_manifest_dir(manifest_dir.clone());
    checker.rust_inspect_cache.insert_test_item(
        &manifest_dir,
        RustItemMetadata {
            canonical_path: "demo::DeviceTrait".to_string(),
            definition_path: Some("demo::DeviceTrait".to_string()),
            visibility: RustVisibility::Public,
            kind: RustItemKind::Trait(RustTraitInfo {
                items: vec![RustTraitAssoc::Function {
                    name: "build_output_stream".to_string(),
                    signature: RustFunctionSig {
                        receiver_contract: None,
                        type_params: vec!["T".to_string(), "D".to_string(), "E".to_string()],
                        params: vec![RustParam {
                            name: Some("self".to_string()),
                            type_display: "&self".to_string(),
                        }],
                        return_type: "()".to_string(),
                        is_async: false,
                        is_unsafe: false,
                    },
                }],
                derive_macro: None,
            }),
        },
    )?;
    checker.rust_inspect_cache.insert_test_item(
        &manifest_dir,
        RustItemMetadata {
            canonical_path: "demo::Device".to_string(),
            definition_path: Some("demo::Device".to_string()),
            visibility: RustVisibility::Public,
            kind: RustItemKind::Type(RustTypeInfo {
                type_params: Vec::new(),
                type_param_defaults: Vec::new(),
                mutable_reference_type_params: Vec::new(),
                expanded_derive_traits: Vec::new(),
                has_const_params: false,
                alias_target: None,
                metadata_completeness: Default::default(),
                methods: Vec::new(),
                implemented_traits: vec![RustImplementedTrait {
                    path: "demo::DeviceTrait".to_string(),
                    mutable_reference: false,
                }],
                fields: Vec::new(),
                variants: Vec::new(),
            }),
        },
    )?;

    let errors = checker
        .check_program(&ast)
        .expect_err("the partial imported Rust trait-method turbofish must be rejected");
    assert!(
        errors.iter().any(|error| error
            .message
            .contains("build_output_stream expects 3 explicit type argument(s), got 1")),
        "expected imported Rust trait-method generic arity diagnostic, got {errors:?}"
    );
    Ok(())
}

#[cfg(feature = "rust_inspect")]
#[test]
fn test_rust_trait_method_unbound_generic_return_stays_unknown_for_source_typing()
-> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
from rust::demo import Rng, ThreadRng

def choose(rng: ThreadRng, items: List[str]) -> str:
  index = rng.gen_range(0..len(items))
  return items[index]
"#;
    let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("lex failed: {errs:?}")))?;
    let ast = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("parse failed: {errs:?}")))?;
    let gen_range_expr = "rng.gen_range(0..len(items))";
    let gen_range_start = source
        .find(gen_range_expr)
        .ok_or_else(|| std::io::Error::other("missing gen_range expression in fixture"))?;
    let gen_range_span = Span::new(gen_range_start, gen_range_start + gen_range_expr.len());
    let mut checker = TypeChecker::new();
    let tmp = seeded_rust_inspect_workspace()?;
    let manifest_dir = tmp.path().to_path_buf();
    checker.set_rust_inspect_manifest_dir(manifest_dir.clone());
    checker
        .rust_inspect_cache
        .insert_test_item(
            &manifest_dir,
            RustItemMetadata {
                canonical_path: "demo::Rng".to_string(),
                definition_path: Some("demo::Rng".to_string()),
                visibility: RustVisibility::Public,
                kind: RustItemKind::Trait(RustTraitInfo {
                    items: vec![RustTraitAssoc::Function {
                        name: "gen_range".to_string(),
                        signature: RustFunctionSig {
                            receiver_contract: None,
                            type_params: Vec::new(),
                            params: vec![
                                RustParam {
                                    name: Some("self".to_string()),
                                    type_display: "&self".to_string(),
                                },
                                RustParam {
                                    name: Some("range".to_string()),
                                    type_display: "R".to_string(),
                                },
                            ],
                            return_type: "T".to_string(),
                            is_async: false,
                            is_unsafe: false,
                        },
                    }],
                    derive_macro: None,
                }),
            },
        )
        .map_err(|err| std::io::Error::other(format!("seed trait metadata: {err}")))?;
    checker
        .rust_inspect_cache
        .insert_test_item(
            &manifest_dir,
            RustItemMetadata {
                canonical_path: "demo::ThreadRng".to_string(),
                definition_path: Some("demo::ThreadRng".to_string()),
                visibility: RustVisibility::Public,
                kind: RustItemKind::Type(RustTypeInfo {
                    type_params: Vec::new(),
                    type_param_defaults: Vec::new(),
                    mutable_reference_type_params: Vec::new(),
                    expanded_derive_traits: Vec::new(),
                    has_const_params: false,
                    alias_target: None,
                    metadata_completeness: Default::default(),
                    methods: Vec::new(),
                    implemented_traits: vec![RustImplementedTrait {
                        path: "demo::Rng".to_string(),
                        mutable_reference: false,
                    }],
                    fields: Vec::new(),
                    variants: Vec::new(),
                }),
            },
        )
        .map_err(|err| std::io::Error::other(format!("seed receiver metadata: {err}")))?;

    checker
        .check_program(&ast)
        .map_err(|errs| std::io::Error::other(format!("typecheck failed: {errs:?}")))?;
    let uses = &checker.type_info().rust.method_trait_import_uses;
    assert!(
        uses.values()
            .any(|import_use| import_use.binding == "Rng" && import_use.method == "gen_range"),
        "expected Rng import use to be retained for gen_range, got {uses:?}"
    );
    assert_eq!(
        checker.type_info().expr_type(gen_range_span),
        Some(&ResolvedType::Unknown),
        "expected unbound generic Rust method return to stay unknown for source typing"
    );
    Ok(())
}

#[cfg(feature = "rust_inspect")]
#[test]
fn test_imported_rust_trait_associated_type_missing_is_rejected() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
from rust::demo import Iterable

type Items = newtype int with Iterable
"#;
    let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("lex failed: {errs:?}")))?;
    let ast = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("parse failed: {errs:?}")))?;
    let mut checker = TypeChecker::new();
    let tmp = seeded_rust_inspect_workspace()?;
    let manifest_dir = tmp.path().to_path_buf();
    checker.set_rust_inspect_manifest_dir(manifest_dir.clone());
    checker
        .rust_inspect_cache
        .insert_test_item(
            &manifest_dir,
            RustItemMetadata {
                canonical_path: "demo::Iterable".to_string(),
                definition_path: Some("demo::Iterable".to_string()),
                visibility: RustVisibility::Public,
                kind: RustItemKind::Trait(RustTraitInfo {
                    items: vec![RustTraitAssoc::TypeAlias {
                        name: "Item".to_string(),
                    }],
                    derive_macro: None,
                }),
            },
        )
        .map_err(|err| std::io::Error::other(format!("seed trait metadata: {err}")))?;
    let Err(errs) = checker.check_program(&ast) else {
        return Err(std::io::Error::other("expected missing associated type diagnostic").into());
    };
    assert!(
        errs.iter().any(|err| err
            .message
            .contains("Trait 'demo::Iterable' requires associated type 'Item'")),
        "expected missing associated type diagnostic, got {errs:?}"
    );
    Ok(())
}

#[test]
fn test_rust_derive_accepts_imported_rust_derive_binding() {
    let source = r#"
from rust::serde import Serialize

@rust.derive(Serialize)
model Payload:
  value: int
"#;
    assert_check_ok(source);
}

#[test]
fn test_rust_derive_rejects_unresolved_third_party_derive() {
    let source = r#"
@rust.derive(Serialize)
model Payload:
  value: int
"#;
    let errs = check_str_err(source, "unresolved @rust.derive should fail");
    assert!(
        errs.iter()
            .any(|err| err.message.contains("Rust derive 'Serialize' is not resolved")),
        "Expected unresolved Rust derive diagnostic; got: {errs:?}"
    );
}

#[test]
fn test_rust_derive_conflicts_with_explicit_trait_adoption() {
    let source = r#"
@rust.derive(Display)
model Label with Display:
  value: str

  def __str__(self) -> str:
    return self.value
"#;
    let errs = check_str_err(source, "@rust.derive should conflict with matching with adoption");
    assert!(
        errs.iter().any(|err| err
            .message
            .contains("@rust.derive(Display) conflicts with explicit `with Display`")),
        "Expected Rust derive/adoption conflict diagnostic; got: {errs:?}"
    );
}

#[test]
fn test_rust_derive_on_rusttype_reports_alias_lowering_blocker() {
    let source = r#"
@rust.derive(Clone)
type ExternalId = rusttype int
"#;
    let errs = check_str_err(
        source,
        "@rust.derive on rusttype should report the current lowering blocker",
    );
    assert!(
        errs.iter().any(|err| err
            .message
            .contains("@rust.derive is not supported on rusttype declarations yet")),
        "Expected rusttype derive blocker diagnostic; got: {errs:?}"
    );
}

#[cfg(feature = "rust_inspect")]
#[test]
fn rust_method_explicit_type_args_require_the_metadata_declared_arity() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
from rust::demo import Device

def partial(device: Device) -> None:
  device.build_output_stream[int]()

def complete(device: Device) -> None:
  device.build_output_stream[int, _, _]()
"#;
    let ast = parse_program(source, "Rust method generic arity");
    let tmp = seeded_rust_inspect_workspace()?;
    let manifest_dir = tmp.path().to_path_buf();
    let mut checker = TypeChecker::new();
    checker.set_rust_inspect_manifest_dir(manifest_dir.clone());
    checker.rust_inspect_cache.insert_test_item(
        &manifest_dir,
        RustItemMetadata {
            canonical_path: "demo::Device".to_string(),
            definition_path: Some("demo::Device".to_string()),
            visibility: RustVisibility::Public,
            kind: RustItemKind::Type(RustTypeInfo {
                type_params: Vec::new(),
                type_param_defaults: Vec::new(),
                mutable_reference_type_params: Vec::new(),
                expanded_derive_traits: Vec::new(),
                has_const_params: false,
                alias_target: None,
                metadata_completeness: Default::default(),
                methods: vec![RustMethodSig {
                    name: "build_output_stream".to_string(),
                    signature: RustFunctionSig {
                        receiver_contract: None,
                        type_params: vec!["T".to_string(), "D".to_string(), "E".to_string()],
                        params: vec![RustParam {
                            name: Some("self".to_string()),
                            type_display: "&self".to_string(),
                        }],
                        return_type: "()".to_string(),
                        is_async: false,
                        is_unsafe: false,
                    },
                }],
                implemented_traits: Vec::new(),
                fields: Vec::new(),
                variants: Vec::new(),
            }),
        },
    )?;

    let errors = checker
        .check_program(&ast)
        .expect_err("the partial Rust method turbofish must be rejected");
    assert!(
        errors.iter().any(|error| error
            .message
            .contains("build_output_stream expects 3 explicit type argument(s), got 1")),
        "expected Rust method generic arity diagnostic, got {errors:?}"
    );
    assert!(
        !errors.iter().any(|error| error.message.contains("got 3")),
        "the full Rust method turbofish should remain accepted, got {errors:?}"
    );
    Ok(())
}

#[cfg(feature = "rust_inspect")]
#[test]
fn rust_method_type_args_contextualize_inline_borrowed_slice_callback_issue835()
-> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
from rust::demo import Device

def run(device: Device) -> None:
  device.build_output_stream[f32, _, _]((_data, _info) => ())
"#;
    let ast = parse_program(source, "Rust borrowed-slice callback specialization");
    let tmp = seeded_rust_inspect_workspace()?;
    let manifest_dir = tmp.path().to_path_buf();
    let mut checker = TypeChecker::new();
    checker.set_rust_inspect_manifest_dir(manifest_dir.clone());
    checker.rust_inspect_cache.insert_test_item(
        &manifest_dir,
        RustItemMetadata {
            canonical_path: "demo::Device".to_string(),
            definition_path: Some("demo::Device".to_string()),
            visibility: RustVisibility::Public,
            kind: RustItemKind::Type(RustTypeInfo {
                type_params: Vec::new(),
                type_param_defaults: Vec::new(),
                mutable_reference_type_params: Vec::new(),
                expanded_derive_traits: Vec::new(),
                has_const_params: false,
                alias_target: None,
                metadata_completeness: Default::default(),
                methods: vec![RustMethodSig {
                    name: "build_output_stream".to_string(),
                    signature: RustFunctionSig {
                        receiver_contract: None,
                        type_params: vec!["T".to_string(), "D".to_string(), "E".to_string()],
                        params: vec![
                            RustParam {
                                name: Some("self".to_string()),
                                type_display: "&self".to_string(),
                            },
                            RustParam {
                                name: Some("callback".to_string()),
                                type_display: "impl FnMut(&mut [T], &demo::OutputCallbackInfo)".to_string(),
                            },
                        ],
                        return_type: "()".to_string(),
                        is_async: false,
                        is_unsafe: false,
                    },
                }],
                implemented_traits: Vec::new(),
                fields: Vec::new(),
                variants: Vec::new(),
            }),
        },
    )?;

    checker
        .check_program(&ast)
        .map_err(|errors| format!("expected contextual callback typing to succeed, got {errors:?}"))?;
    assert_eq!(
        checker.type_info.rust.closure_param_type_displays.values().next(),
        Some(&vec!["&mut [f32]".to_string(), "&demo::OutputCallbackInfo".to_string(),]),
        "expected explicit method type arguments to specialize the emitted borrowed-slice callback display"
    );
    Ok(())
}

#[cfg(feature = "rust_inspect")]
#[test]
fn rust_associated_calls_specialize_receiver_generics_explicitly_and_contextually()
-> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
from rust::demo import Factory

def explicit() -> Factory[f32]:
  return Factory.new[f32]()

def contextual() -> Factory[f32]:
  value: Factory[f32] = Factory.new()
  return value

def direct_contextual() -> Factory[f32]:
  return Factory.new()
"#;
    let ast = parse_program(source, "Rust associated receiver generics");
    let tmp = seeded_rust_inspect_workspace()?;
    let manifest_dir = tmp.path().to_path_buf();
    let mut checker = TypeChecker::new();
    checker.set_rust_inspect_manifest_dir(manifest_dir.clone());
    checker.rust_inspect_cache.insert_test_item(
        &manifest_dir,
        RustItemMetadata {
            canonical_path: "demo::Factory".to_string(),
            definition_path: Some("demo::Factory".to_string()),
            visibility: RustVisibility::Public,
            kind: RustItemKind::Type(RustTypeInfo {
                type_params: vec!["T".to_string()],
                type_param_defaults: Vec::new(),
                mutable_reference_type_params: Vec::new(),
                expanded_derive_traits: Vec::new(),
                has_const_params: false,
                alias_target: None,
                metadata_completeness: Default::default(),
                methods: vec![RustMethodSig {
                    name: "new".to_string(),
                    signature: RustFunctionSig {
                        receiver_contract: None,
                        type_params: Vec::new(),
                        params: Vec::new(),
                        return_type: "Self".to_string(),
                        is_async: false,
                        is_unsafe: false,
                    },
                }],
                implemented_traits: Vec::new(),
                fields: Vec::new(),
                variants: Vec::new(),
            }),
        },
    )?;

    checker
        .check_program(&ast)
        .map_err(|errors| format!("expected receiver-generic associated calls to typecheck, got {errors:?}"))?;
    Ok(())
}

#[cfg(feature = "rust_inspect")]
#[test]
fn rust_associated_receiver_type_args_require_owner_arity() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
from rust::demo import Factory

def invalid() -> Factory[f32]:
  return Factory.new[f32, str]()
"#;
    let ast = parse_program(source, "Rust associated receiver generic arity");
    let tmp = seeded_rust_inspect_workspace()?;
    let manifest_dir = tmp.path().to_path_buf();
    let mut checker = TypeChecker::new();
    checker.set_rust_inspect_manifest_dir(manifest_dir.clone());
    checker.rust_inspect_cache.insert_test_item(
        &manifest_dir,
        RustItemMetadata {
            canonical_path: "demo::Factory".to_string(),
            definition_path: Some("demo::Factory".to_string()),
            visibility: RustVisibility::Public,
            kind: RustItemKind::Type(RustTypeInfo {
                type_params: vec!["T".to_string()],
                type_param_defaults: Vec::new(),
                mutable_reference_type_params: Vec::new(),
                expanded_derive_traits: Vec::new(),
                has_const_params: false,
                alias_target: None,
                metadata_completeness: Default::default(),
                methods: vec![RustMethodSig {
                    name: "new".to_string(),
                    signature: RustFunctionSig {
                        receiver_contract: None,
                        type_params: Vec::new(),
                        params: Vec::new(),
                        return_type: "Self".to_string(),
                        is_async: false,
                        is_unsafe: false,
                    },
                }],
                implemented_traits: Vec::new(),
                fields: Vec::new(),
                variants: Vec::new(),
            }),
        },
    )?;

    let errors = match checker.check_program(&ast) {
        Ok(_) => return Err("receiver-generic associated calls must enforce owner arity".into()),
        Err(errors) => errors,
    };
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("new expects 1 explicit type argument(s), got 2")),
        "expected owner-generic arity diagnostic, got {errors:?}"
    );
    Ok(())
}

#[cfg(feature = "rust_inspect")]
#[test]
fn rust_associated_calls_apply_owner_generics_to_parameters_and_returns() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
from rust::demo import Factory

def explicit() -> Factory[i64, str]:
  return Factory.new[i64, str](1, "marker")

def contextual() -> Factory[i64, str]:
  return Factory.new(7, "marker")

def owner_value() -> str:
  return Factory.first[str, i64]("value")

def accept_factory(value: Factory[i64, str]) -> None:
  pass

def parameter_context() -> None:
  accept_factory(Factory.new(7, "marker"))
"#;
    let ast = parse_program(source, "Rust associated owner substitution");
    let tmp = seeded_rust_inspect_workspace()?;
    let manifest_dir = tmp.path().to_path_buf();
    let mut checker = TypeChecker::new();
    checker.set_rust_inspect_manifest_dir(manifest_dir.clone());
    checker.rust_inspect_cache.insert_test_item(
        &manifest_dir,
        RustItemMetadata {
            canonical_path: "demo::Factory".to_string(),
            definition_path: Some("demo::Factory".to_string()),
            visibility: RustVisibility::Public,
            kind: RustItemKind::Type(RustTypeInfo {
                type_params: vec!["T".to_string(), "U".to_string()],
                type_param_defaults: Vec::new(),
                mutable_reference_type_params: Vec::new(),
                expanded_derive_traits: Vec::new(),
                has_const_params: false,
                alias_target: None,
                metadata_completeness: Default::default(),
                methods: vec![
                    RustMethodSig {
                        name: "new".to_string(),
                        signature: RustFunctionSig {
                            receiver_contract: None,
                            type_params: Vec::new(),
                            params: vec![
                                RustParam {
                                    name: Some("value".to_string()),
                                    type_display: "T".to_string(),
                                },
                                RustParam {
                                    name: Some("marker".to_string()),
                                    type_display: "U".to_string(),
                                },
                            ],
                            return_type: "Self".to_string(),
                            is_async: false,
                            is_unsafe: false,
                        },
                    },
                    RustMethodSig {
                        name: "first".to_string(),
                        signature: RustFunctionSig {
                            receiver_contract: None,
                            type_params: Vec::new(),
                            params: vec![RustParam {
                                name: Some("value".to_string()),
                                type_display: "T".to_string(),
                            }],
                            return_type: "T".to_string(),
                            is_async: false,
                            is_unsafe: false,
                        },
                    },
                ],
                implemented_traits: Vec::new(),
                fields: Vec::new(),
                variants: Vec::new(),
            }),
        },
    )?;

    checker
        .check_program(&ast)
        .map_err(|errors| format!("expected owner-generic parameters and returns to specialize, got {errors:?}"))?;
    let specialized_calls = checker
        .type_info()
        .calls
        .call_site_callable_params
        .values()
        .filter(|params| {
            matches!(
                params.as_slice(),
                [
                    CallableParam {
                        ty: ResolvedType::Int,
                        ..
                    },
                    CallableParam {
                        ty: ResolvedType::Str,
                        ..
                    }
                ]
            )
        })
        .count();
    assert!(
        specialized_calls >= 3,
        "expected explicit, return-context, and parameter-context calls to preserve exact owner-specialized parameters; \
         got {:?}",
        checker.type_info().calls.call_site_callable_params
    );
    Ok(())
}

#[cfg(feature = "rust_inspect")]
fn receiver_factory_manifest(library_name: &str, value_type: &str) -> LibraryManifest {
    let target_path = vec!["rust".to_string(), "demo".to_string(), "PairFactory".to_string()];
    let checked_export = CheckedNamedExport {
        name: "PairFactory".to_string(),
        identity: CheckedExportIdentity::reexport(target_path.clone(), target_path.clone()),
        kind: CheckedExportKind::Alias(CheckedAliasExport {
            name: "PairFactory".to_string(),
            target_path,
            projected_type: None,
            projected_function: None,
        }),
    };
    let mut manifest = LibraryManifest::from_checked_exports(library_name, "0.1.0", &[checked_export]);
    manifest.rust_abi = LibraryRustAbi::from_items(vec![RustItemMetadata {
        canonical_path: "demo::PairFactory".to_string(),
        definition_path: Some("demo::PairFactory".to_string()),
        visibility: RustVisibility::Public,
        kind: RustItemKind::Type(RustTypeInfo {
            type_params: vec!["T".to_string(), "U".to_string()],
            type_param_defaults: Vec::new(),
            mutable_reference_type_params: Vec::new(),
            expanded_derive_traits: Vec::new(),
            has_const_params: false,
            alias_target: None,
            metadata_completeness: Default::default(),
            methods: vec![RustMethodSig {
                name: "new".to_string(),
                signature: RustFunctionSig {
                    receiver_contract: None,
                    type_params: Vec::new(),
                    params: vec![
                        RustParam {
                            name: Some("value".to_string()),
                            type_display: value_type.to_string(),
                        },
                        RustParam {
                            name: Some("marker".to_string()),
                            type_display: "U".to_string(),
                        },
                    ],
                    return_type: "demo::PairFactory<T, U>".to_string(),
                    is_async: false,
                    is_unsafe: false,
                },
            }],
            implemented_traits: Vec::new(),
            fields: Vec::new(),
            variants: Vec::new(),
        }),
    }]);
    manifest
}

#[cfg(feature = "rust_inspect")]
#[test]
fn compiled_library_rust_reexport_restores_receiver_generic_metadata() -> Result<(), Box<dyn std::error::Error>> {
    let manifest = receiver_factory_manifest("receiver_factory_api", "T");
    let index = LibraryManifestIndex::from_entries(HashMap::from([(
        "receiver_factory_api".to_string(),
        LibraryManifestIndexEntry::Loaded {
            manifest: Box::new(manifest),
            metadata: LibraryArtifactMetadata::from_crate_root(
                "receiver_factory_api",
                "receiver_factory_api",
                synthetic_artifact_root("receiver_factory_api"),
            ),
        },
    )]));
    let source = r#"
from pub::receiver_factory_api import PairFactory

def accept_pair(value: PairFactory[i64, str]) -> None:
  pass

def run() -> None:
  accept_pair(PairFactory.new(7, "marker"))
"#;

    check_str_with_library_index(source, index)
        .map_err(|errors| format!("expected compiled Rust reexport metadata to typecheck, got {errors:?}"))?;
    Ok(())
}

#[cfg(feature = "rust_inspect")]
#[test]
fn compiled_rust_reexport_uses_selected_provider_abi_for_method_resolution() -> Result<(), Box<dyn std::error::Error>> {
    let conflicting = receiver_factory_manifest("a_conflicting_factory", "str");
    let selected = receiver_factory_manifest("z_selected_factory", "T");
    let index = LibraryManifestIndex::from_entries(HashMap::from([
        (
            "a_conflicting_factory".to_string(),
            LibraryManifestIndexEntry::Loaded {
                manifest: Box::new(conflicting),
                metadata: LibraryArtifactMetadata::from_crate_root(
                    "a_conflicting_factory",
                    "a_conflicting_factory",
                    synthetic_artifact_root("a_conflicting_factory"),
                ),
            },
        ),
        (
            "z_selected_factory".to_string(),
            LibraryManifestIndexEntry::Loaded {
                manifest: Box::new(selected),
                metadata: LibraryArtifactMetadata::from_crate_root(
                    "z_selected_factory",
                    "z_selected_factory",
                    synthetic_artifact_root("z_selected_factory"),
                ),
            },
        ),
    ]));
    let source = r#"
from pub::z_selected_factory import PairFactory

def run() -> PairFactory[i64, str]:
  return PairFactory.new(7, "marker")
"#;

    check_str_with_library_index(source, index)
        .map_err(|errors| format!("expected the selected provider ABI to own method resolution, got {errors:?}"))?;
    Ok(())
}

#[cfg(feature = "rust_inspect")]
#[test]
fn rust_associated_context_rejects_arguments_that_conflict_with_owner_generics()
-> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
from rust::demo import Factory

def invalid() -> Factory[i64]:
  return Factory.new("text")
"#;
    let ast = parse_program(source, "Rust associated owner mismatch");
    let tmp = seeded_rust_inspect_workspace()?;
    let manifest_dir = tmp.path().to_path_buf();
    let mut checker = TypeChecker::new();
    checker.set_rust_inspect_manifest_dir(manifest_dir.clone());
    checker.rust_inspect_cache.insert_test_item(
        &manifest_dir,
        RustItemMetadata {
            canonical_path: "demo::Factory".to_string(),
            definition_path: Some("demo::Factory".to_string()),
            visibility: RustVisibility::Public,
            kind: RustItemKind::Type(RustTypeInfo {
                type_params: vec!["T".to_string()],
                type_param_defaults: Vec::new(),
                mutable_reference_type_params: Vec::new(),
                expanded_derive_traits: Vec::new(),
                has_const_params: false,
                alias_target: None,
                metadata_completeness: Default::default(),
                methods: vec![RustMethodSig {
                    name: "new".to_string(),
                    signature: RustFunctionSig {
                        receiver_contract: None,
                        type_params: Vec::new(),
                        params: vec![RustParam {
                            name: Some("value".to_string()),
                            type_display: "T".to_string(),
                        }],
                        return_type: "Self".to_string(),
                        is_async: false,
                        is_unsafe: false,
                    },
                }],
                implemented_traits: Vec::new(),
                fields: Vec::new(),
                variants: Vec::new(),
            }),
        },
    )?;

    let errors = match checker.check_program(&ast) {
        Ok(_) => return Err("contextual owner specialization must validate its parameter types".into()),
        Err(errors) => errors,
    };
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("expected 'i64'") && error.message.contains("found 'str'")),
        "expected contextual owner mismatch diagnostic, got {errors:?}"
    );
    Ok(())
}

#[cfg(feature = "rust_inspect")]
#[test]
fn rust_associated_receiver_specialization_rejects_const_generic_owners() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
from rust::demo import Factory

def invalid() -> Factory[f32]:
  return Factory.new[f32]()

def contextual() -> Factory[f32]:
  return Factory.new()
"#;
    let ast = parse_program(source, "Rust associated const-generic owner");
    let tmp = seeded_rust_inspect_workspace()?;
    let manifest_dir = tmp.path().to_path_buf();
    let mut checker = TypeChecker::new();
    checker.set_rust_inspect_manifest_dir(manifest_dir.clone());
    checker.rust_inspect_cache.insert_test_item(
        &manifest_dir,
        RustItemMetadata {
            canonical_path: "demo::Factory".to_string(),
            definition_path: Some("demo::Factory".to_string()),
            visibility: RustVisibility::Public,
            kind: RustItemKind::Type(RustTypeInfo {
                type_params: vec!["T".to_string()],
                type_param_defaults: Vec::new(),
                mutable_reference_type_params: Vec::new(),
                expanded_derive_traits: Vec::new(),
                has_const_params: true,
                alias_target: None,
                metadata_completeness: Default::default(),
                methods: vec![RustMethodSig {
                    name: "new".to_string(),
                    signature: RustFunctionSig {
                        receiver_contract: None,
                        type_params: Vec::new(),
                        params: Vec::new(),
                        return_type: "Self".to_string(),
                        is_async: false,
                        is_unsafe: false,
                    },
                }],
                implemented_traits: Vec::new(),
                fields: Vec::new(),
                variants: Vec::new(),
            }),
        },
    )?;

    let errors = match checker.check_program(&ast) {
        Ok(_) => return Err("const-generic receiver specialization must fail closed".into()),
        Err(errors) => errors,
    };
    let const_generic_errors = errors
        .iter()
        .filter(|error| error.message.contains("has const generic parameters"))
        .count();
    assert_eq!(
        const_generic_errors, 2,
        "expected explicit and contextual const-generic receiver diagnostics, got {errors:?}"
    );
    Ok(())
}

// ---- #1720: open owner type arguments on a Rust associated call ----

/// A `HashMap`-shaped type item with the given parameters and defaults, carrying `new` (result `result_display`)
/// and `insert(&mut self, k: K, v: V)`.
fn hashmap_type_info(type_params: &[&str], defaults: &[Option<&str>], result_display: &str) -> RustTypeInfo {
    RustTypeInfo {
        type_params: type_params.iter().map(|param| param.to_string()).collect(),
        type_param_defaults: defaults.iter().map(|default| default.map(str::to_string)).collect(),
        mutable_reference_type_params: Vec::new(),
        expanded_derive_traits: Vec::new(),
        has_const_params: false,
        alias_target: None,
        metadata_completeness: Default::default(),
        methods: vec![
            RustMethodSig {
                name: "new".to_string(),
                signature: RustFunctionSig {
                    receiver_contract: None,
                    type_params: Vec::new(),
                    params: Vec::new(),
                    return_type: result_display.to_string(),
                    is_async: false,
                    is_unsafe: false,
                },
            },
            RustMethodSig {
                name: "insert".to_string(),
                signature: RustFunctionSig {
                    receiver_contract: None,
                    type_params: Vec::new(),
                    params: vec![
                        RustParam {
                            name: Some("self".to_string()),
                            type_display: "&mut self".to_string(),
                        },
                        RustParam {
                            name: Some("k".to_string()),
                            type_display: "K".to_string(),
                        },
                        RustParam {
                            name: Some("v".to_string()),
                            type_display: "V".to_string(),
                        },
                    ],
                    return_type: "Option<V>".to_string(),
                    is_async: false,
                    is_unsafe: false,
                },
            },
        ],
        implemented_traits: Vec::new(),
        fields: vec![],
        variants: vec![],
    }
}

/// Shipped ABI metadata for `std::collections::HashMap` the way the checker sees it once a provider has recorded
/// the inspected item: `K` and `V` are the owner's open type parameters.
fn hashmap_library_index(name: &str) -> LibraryManifestIndex {
    library_index_with_rust_abi_item(
        name,
        RustItemMetadata {
            canonical_path: "std::collections::HashMap".to_string(),
            definition_path: Some("std::collections::HashMap".to_string()),
            visibility: RustVisibility::Public,
            kind: RustItemKind::Type(hashmap_type_info(
                &["K", "V"],
                &[None, None],
                "std::collections::HashMap<K, V>",
            )),
        },
    )
}

#[test]
fn open_rust_owner_type_args_skip_defaults_and_argument_fixed_parameters_issue1720()
-> Result<(), Box<dyn std::error::Error>> {
    // `HashMap<K, V, S = RandomState>`: `new` leaves `K` and `V` open and never the defaulted hasher; a
    // constructor whose argument names the parameters, or whose result is a plain scalar, leaves nothing open.
    let with_hasher = hashmap_type_info(
        &["K", "V", "S"],
        &[None, None, Some("std::hash::RandomState")],
        "std::collections::HashMap<K, V, std::hash::RandomState>",
    );
    let new_sig = with_hasher
        .methods
        .iter()
        .find(|method| method.name == "new")
        .map(|method| method.signature.clone());
    let Some(new_sig) = new_sig else {
        return Err("the fixture declares `new`".into());
    };
    assert_eq!(
        TypeChecker::open_rust_owner_type_args(&new_sig, &with_hasher),
        vec!["K".to_string(), "V".to_string()]
    );

    let as_self = RustFunctionSig {
        return_type: "Self".to_string(),
        ..new_sig.clone()
    };
    assert_eq!(
        TypeChecker::open_rust_owner_type_args(&as_self, &with_hasher),
        vec!["K".to_string(), "V".to_string()],
        "`Self` stands for the owner with every parameter"
    );

    let from_pairs = RustFunctionSig {
        params: vec![RustParam {
            name: Some("pairs".to_string()),
            type_display: "Vec<(K, V)>".to_string(),
        }],
        ..new_sig.clone()
    };
    assert!(
        TypeChecker::open_rust_owner_type_args(&from_pairs, &with_hasher).is_empty(),
        "an argument that names the parameters fixes them"
    );

    let capacity = RustFunctionSig {
        return_type: "usize".to_string(),
        ..new_sig
    };
    assert!(
        TypeChecker::open_rust_owner_type_args(&capacity, &with_hasher).is_empty(),
        "a result that carries no parameter leaves nothing open"
    );
    Ok(())
}

#[test]
fn untyped_hashmap_new_bound_to_an_unread_local_is_refused_issue1720() -> Result<(), Box<dyn std::error::Error>> {
    // The program from #1720: `untyped` is never read, so nothing later could fix `K` and `V`; the explicit
    // spelling beside it is fine, and a bare call binds nothing at all.
    let source = r#"
from rust::std::collections import HashMap

def main() -> None:
    mut untyped = HashMap.new()
    mut typed = HashMap.new[str, int]()
    HashMap.new()
    println("ok")
"#;
    let errors = check_str_with_library_index_err(
        source,
        hashmap_library_index("hashmap_open_generics"),
        "an untyped HashMap.new() that nothing reads must be refused",
    )?;
    let refused = errors
        .iter()
        .filter(|error| error.stable_code() == Some("INCAN-T0105"))
        .map(|error| (error.message.clone(), error.notes.clone(), error.hints.clone()))
        .collect::<Vec<_>>();
    assert_eq!(
        refused
            .iter()
            .map(|(message, _, _)| message.as_str())
            .collect::<Vec<_>>(),
        vec![
            "Cannot infer the type arguments 'K, V' of 'HashMap.new()'",
            "Cannot infer the type arguments 'K, V' of 'HashMap.new()'",
        ],
        "the discarded call is refused where it stands and the unread binding when its block ends; got {errors:?}"
    );
    let (_, discarded_notes, _) = &refused[0];
    assert!(
        discarded_notes.iter().any(|note| note.contains("not bound to a name")),
        "the bare statement is reported as a discarded value, got: {discarded_notes:?}"
    );
    let (_, binding_notes, binding_hints) = &refused[1];
    assert!(
        binding_notes
            .iter()
            .any(|note| note.contains("'untyped' is never read")),
        "the binding is reported as unread, got: {binding_notes:?}"
    );
    assert!(
        binding_hints.iter().any(|hint| hint.contains("HashMap.new[str, int]()")
            && hint.contains("untyped: HashMap[str, int] = HashMap.new()")),
        "the hint spells both remedies, got: {binding_hints:?}"
    );
    Ok(())
}

#[test]
fn untyped_hashmap_new_with_a_later_reader_or_an_annotation_is_accepted_issue1720()
-> Result<(), Box<dyn std::error::Error>> {
    // The documented `count_words` shape: a later insert gives Rust what it needs, and an annotated binding fixes
    // the arguments on its own. Neither is the checker's to refuse.
    let source = r#"
from rust::std::collections import HashMap

def count_words(words: list[str]) -> int:
    mut counts = HashMap.new()
    for word in words:
        counts.insert(word, 1)
    return len(counts)

def main() -> None:
    annotated: HashMap[str, int] = HashMap.new()
    println(count_words(["a"]))
    println(len(annotated))
"#;
    check_str_with_library_index(source, hashmap_library_index("hashmap_read_later")).map_err(|errors| {
        std::io::Error::other(format!(
            "a binding a later statement reads must stay accepted: {:?}",
            errors.iter().map(|error| &error.message).collect::<Vec<_>>()
        ))
    })?;
    Ok(())
}
