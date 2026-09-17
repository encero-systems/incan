//! Checked programs whose lowering, generated Rust or refusal the backend must reproduce exactly.

use std::collections::HashMap;

use incan_lang::interop::{
    RustFunctionSig, RustImplementedTrait, RustItemKind, RustItemMetadata, RustParam, RustTraitAssoc, RustTraitInfo,
    RustTypeInfo, RustVisibility,
};

use incan_frontend::ast::ParamKind;
use incan_frontend::symbols::{CallableParam, ResolvedType};
use incan_frontend::test_support::seeded_rust_inspect_workspace;
use incan_frontend::typechecker::TypeChecker;
use incan_frontend::{lexer, parser};

#[cfg(feature = "rust_inspect")]
#[test]
fn rust_trait_associated_call_uses_callable_shape_from_dependency_namespace_source()
-> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
from rust::prost import Message
from rust::prost_types import FileDescriptorSet, ProducerPlan

def main() -> None:
  producer = ProducerPlan.new()
  encoded = producer.encode_to_vec()
  match FileDescriptorSet.decode(encoded):
    Ok(_) => println("ok")
    Err(_) => println("err")
"#;
    let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("lex failed: {errs:?}")))?;
    let ast = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("parse failed: {errs:?}")))?;
    let tmp = tempfile::tempdir()?;
    let root = tmp.path().join("generated_lock");
    let prost = tmp.path().join("prost");
    let prost_types = tmp.path().join("prost-types");
    for dir in [&root, &prost, &prost_types] {
        std::fs::create_dir_all(dir.join("src"))?;
    }
    std::fs::write(
        root.join("Cargo.toml"),
        r#"[package]
name = "probe"
version = "0.1.0"
edition = "2021"

[dependencies]
prost = { path = "../prost" }
prost_types = { package = "prost-types", path = "../prost-types" }
"#,
    )?;
    std::fs::write(root.join("src/main.rs"), "fn main() {}\n")?;
    std::fs::write(
        prost.join("Cargo.toml"),
        "[package]\nname = \"prost\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )?;
    std::fs::write(
        prost.join("src/lib.rs"),
        r#"pub trait Buf {}

pub struct DecodeError;

pub trait Message: Sized {
    fn decode(buf: impl Buf) -> Result<Self, DecodeError>;
}
"#,
    )?;
    std::fs::write(
        prost_types.join("Cargo.toml"),
        r#"[package]
name = "prost-types"
version = "0.1.0"
edition = "2021"

[dependencies]
prost = { path = "../prost" }
"#,
    )?;
    std::fs::write(
        prost_types.join("src/lib.rs"),
        r#"pub struct ProducerPlan;

impl ProducerPlan {
    pub fn new() -> Self {
        Self
    }

    pub fn encode_to_vec(&self) -> Vec<u8> {
        Vec::new()
    }
}

pub struct FileDescriptorSet;
"#,
    )?;

    let mut checker = TypeChecker::new();
    checker.set_rust_inspect_manifest_dir(root.clone());
    checker
        .check_program(&ast)
        .map_err(|errs| std::io::Error::other(format!("typecheck failed: {errs:?}")))?;
    let call_start = source
        .find("FileDescriptorSet.decode(encoded)")
        .ok_or_else(|| std::io::Error::other("test source should contain decode call"))?;
    let call_span = incan_frontend::ast::Span::new(call_start, call_start + "FileDescriptorSet.decode(encoded)".len());
    let params = checker
        .type_info()
        .call_site_callable_params(call_span)
        .ok_or_else(|| std::io::Error::other("source-extracted decode call should record params"))?;
    assert_eq!(params.len(), 1);
    assert_eq!(params[0].ty, ResolvedType::TypeVar("implBuf".to_string()));
    let encoded_arg_start = source
        .rfind("encoded)")
        .ok_or_else(|| std::io::Error::other("test source should contain encoded argument"))?;
    let encoded_arg_span = incan_frontend::ast::Span::new(encoded_arg_start, encoded_arg_start + "encoded".len());
    assert_eq!(
        checker.type_info().expr_type(encoded_arg_span),
        Some(&ResolvedType::Bytes)
    );
    let mut codegen = crate::IrCodegen::new();
    codegen.set_rust_inspect_manifest_dir(root);
    codegen.set_prechecked_type_info(checker.type_info().clone(), HashMap::new());
    let generated = codegen
        .try_generate(&ast)
        .map_err(|error| std::io::Error::other(format!("codegen failed: {error:?}")))?;
    assert!(
        generated.contains("FileDescriptorSet::decode((encoded).as_slice())"),
        "source-extracted Buf metadata should adapt owned bytes; got:\n{generated}"
    );
    Ok(())
}

#[cfg(feature = "rust_inspect")]
#[test]
fn test_rusttype_bodyless_rust_trait_forwarding_uses_metadata_and_skips_impl() -> Result<(), Box<dyn std::error::Error>>
{
    let source = r#"
from rust::demo import RustThing, Labelled

type Thing = rusttype RustThing with Labelled
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
                canonical_path: "demo::Labelled".to_string(),
                definition_path: Some("demo::Labelled".to_string()),
                visibility: RustVisibility::Public,
                kind: RustItemKind::Trait(RustTraitInfo {
                    items: vec![],
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
                canonical_path: "demo::RustThing".to_string(),
                definition_path: Some("demo::RustThing".to_string()),
                visibility: RustVisibility::Public,
                kind: RustItemKind::Type(RustTypeInfo {
                    type_params: Vec::new(),
                    type_param_defaults: Vec::new(),
                    mutable_reference_type_params: Vec::new(),
                    expanded_derive_traits: Vec::new(),
                    has_const_params: false,
                    alias_target: None,
                    metadata_completeness: Default::default(),
                    methods: vec![],
                    implemented_traits: vec![RustImplementedTrait {
                        path: "demo::Labelled".to_string(),
                        mutable_reference: false,
                    }],
                    fields: vec![],
                    variants: vec![],
                }),
            },
        )
        .map_err(|err| std::io::Error::other(format!("seed type metadata: {err}")))?;

    checker
        .check_program(&ast)
        .map_err(|errs| std::io::Error::other(format!("typecheck failed: {errs:?}")))?;
    assert!(
        checker
            .type_info()
            .rust
            .rusttype_forwarded_trait_adoptions
            .contains(&("Thing".to_string(), "Labelled".to_string())),
        "expected metadata-proven rusttype forwarding to be recorded"
    );

    let mut lowering = incan_ir::AstLowering::new_with_type_info(checker.type_info().clone());
    let ir = lowering
        .lower_program(&ast)
        .map_err(|errs| std::io::Error::other(format!("lowering failed: {errs:?}")))?;
    assert!(
        !ir.declarations.iter().any(|decl| matches!(
            &decl.kind,
            incan_ir::IrDeclKind::Impl(impl_block)
                if impl_block.target_type == "Thing" && impl_block.trait_name.as_deref() == Some("Labelled")
        )),
        "rusttype forwarding through a type alias must not emit an orphan impl"
    );
    Ok(())
}

#[cfg(feature = "rust_inspect")]
#[test]
fn test_rust_extension_trait_associated_call_records_param_shape_without_receiver_trait_impl_metadata()
-> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
from rust::demo import Message
from rust::datafusion_substrait::substrait::proto import Plan as ConsumerPlan

def f(encoded: bytes) -> None:
  _ = ConsumerPlan.decode(encoded)
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
                canonical_path: "demo::Message".to_string(),
                definition_path: Some("demo::Message".to_string()),
                visibility: RustVisibility::Public,
                kind: RustItemKind::Trait(RustTraitInfo {
                    items: vec![RustTraitAssoc::Function {
                        name: "decode".to_string(),
                        signature: RustFunctionSig {
                            receiver_contract: None,
                            type_params: Vec::new(),
                            params: vec![RustParam {
                                name: Some("buf".to_string()),
                                type_display: "implBuf".to_string(),
                            }],
                            return_type: "Self".to_string(),
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
                canonical_path: "datafusion_substrait::substrait::proto::Plan".to_string(),
                definition_path: Some("substrait::proto::Plan".to_string()),
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
        )
        .map_err(|err| std::io::Error::other(format!("seed receiver metadata: {err}")))?;

    checker
        .check_program(&ast)
        .map_err(|errs| std::io::Error::other(format!("typecheck failed: {errs:?}")))?;
    let call_start = source
        .find("ConsumerPlan.decode(encoded)")
        .ok_or_else(|| std::io::Error::other("test source should contain decode call"))?;
    let call_span = incan_frontend::ast::Span::new(call_start, call_start + "ConsumerPlan.decode(encoded)".len());
    let uses = &checker.type_info().rust.method_trait_import_uses;
    assert!(
        uses.values()
            .any(|import_use| import_use.binding == "Message" && import_use.method == "decode"),
        "expected Message import use for unresolved receiver metadata, got {uses:?}"
    );
    let params = checker
        .type_info()
        .call_site_callable_params(call_span)
        .ok_or_else(|| std::io::Error::other("decode call should record params on the full call span"))?;
    assert_eq!(
        params,
        &[CallableParam {
            name: Some("buf".to_string()),
            ty: ResolvedType::TypeVar("implBuf".to_string()),
            kind: ParamKind::Normal,
            has_default: false,
            is_partial_preset: false,
        }],
        "expected exact call-span decode parameter shape when receiver metadata lacks the trait edge"
    );
    assert!(
        checker
            .type_info()
            .calls
            .call_site_callable_params
            .values()
            .any(|params| params.len() == 1 && params[0].ty == ResolvedType::TypeVar("implBuf".to_string())),
        "expected trait-provided decode parameter shape without receiver metadata, got {:?}",
        checker.type_info().calls.call_site_callable_params
    );
    assert!(
        checker.type_info().rust.arg_coercions.is_empty(),
        "expected unresolved receiver trait signature to avoid borrow coercions, got {:?}",
        checker.type_info().rust.arg_coercions
    );
    Ok(())
}

#[cfg(feature = "rust_inspect")]
#[test]
fn test_rust_extension_trait_associated_call_records_param_shape() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
from rust::demo import FileDescriptorSet, Message

def f(encoded: bytes) -> None:
  _ = FileDescriptorSet.decode(encoded)
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
                canonical_path: "demo::Message".to_string(),
                definition_path: Some("demo::Message".to_string()),
                visibility: RustVisibility::Public,
                kind: RustItemKind::Trait(RustTraitInfo {
                    items: vec![RustTraitAssoc::Function {
                        name: "decode".to_string(),
                        signature: RustFunctionSig {
                            receiver_contract: None,
                            type_params: Vec::new(),
                            params: vec![RustParam {
                                name: Some("buf".to_string()),
                                type_display: "implBuf".to_string(),
                            }],
                            return_type: "Self".to_string(),
                            is_async: false,
                            is_unsafe: false,
                        },
                    }],
                    derive_macro: None,
                }),
            },
        )
        .map_err(|err| std::io::Error::other(format!("seed trait metadata: {err}")))?;
    let path = "demo::FileDescriptorSet";
    checker
        .rust_inspect_cache
        .insert_test_item(
            &manifest_dir,
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
                    alias_target: None,
                    metadata_completeness: Default::default(),
                    methods: Vec::new(),
                    implemented_traits: vec![RustImplementedTrait {
                        path: "demo::Message".to_string(),
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
    let call_start = source
        .find("FileDescriptorSet.decode(encoded)")
        .ok_or_else(|| std::io::Error::other("test source should contain decode call"))?;
    let call_span = incan_frontend::ast::Span::new(call_start, call_start + "FileDescriptorSet.decode(encoded)".len());
    let uses = &checker.type_info().rust.method_trait_import_uses;
    assert!(
        uses.values()
            .any(|import_use| import_use.binding == "Message" && import_use.method == "decode"),
        "expected Message import use, got {uses:?}"
    );
    let params = checker
        .type_info()
        .call_site_callable_params(call_span)
        .ok_or_else(|| std::io::Error::other("decode call should record params on the full call span"))?;
    assert_eq!(
        params,
        &[CallableParam {
            name: Some("buf".to_string()),
            ty: ResolvedType::TypeVar("implBuf".to_string()),
            kind: ParamKind::Normal,
            has_default: false,
            is_partial_preset: false,
        }],
        "expected exact call-span decode parameter shape"
    );
    assert!(
        checker
            .type_info()
            .calls
            .call_site_callable_params
            .values()
            .any(|params| params.len() == 1 && params[0].ty == ResolvedType::TypeVar("implBuf".to_string())),
        "expected trait-provided decode parameter shape to be recorded, got {:?}",
        checker.type_info().calls.call_site_callable_params
    );
    assert!(
        checker.type_info().rust.arg_coercions.is_empty(),
        "expected trait-provided impl Trait decode to avoid borrow coercions, got {:?}",
        checker.type_info().rust.arg_coercions
    );
    Ok(())
}

use incan_semantics_core::{HirSourceSpan, body_ir as bir};

use incan_frontend::test_support::{
    build_with_top_level_declaration_injected_after_typecheck, fixture_top_level_vocab_declaration,
};

#[test]
fn an_undesugared_top_level_vocab_declaration_refuses_every_executable_body() -> Result<(), Box<dyn std::error::Error>>
{
    let module = build_with_top_level_declaration_injected_after_typecheck(
        "def main() -> int:\n    return 1\n\ndef helper() -> int:\n    return 2\n",
        &["m"],
        fixture_top_level_vocab_declaration(),
    )?;

    for body in &module.bodies {
        let Some(statement) = body.block.stmts.first() else {
            return Err(Box::from(format!("expected a contract refusal in `{}`", body.name)));
        };
        let bir::StatementKind::Unsupported { description } = &statement.kind else {
            return Err(Box::from(format!(
                "the first statement of `{}` must reject the raw top-level declaration: {statement:?}",
                body.name
            )));
        };
        assert!(description.contains("top-level vocab block"), "{description}");
        assert!(
            description.contains("Body IR input-contract violation"),
            "{description}"
        );
        assert_eq!(statement.span, HirSourceSpan::new(40, 60));
    }

    let error = crate::replacement::prepare_free_function_execution(&module, "main", &[])
        .err()
        .ok_or("a raw top-level vocabulary declaration must stop direct-execution preparation")?;
    let crate::replacement::ReplacementExecutionError::Unsupported { description, span, .. } = error else {
        return Err(Box::from(format!("unexpected direct-execution result: {error}")));
    };
    assert!(description.contains("top-level vocab block"), "{description}");
    assert_eq!(span, HirSourceSpan::new(40, 60));
    Ok(())
}
