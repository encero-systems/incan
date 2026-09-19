//! Trait-qualified Rust calls borrow their explicit receiver the way the trait declares it (#1375).
//!
//! `Mac.update(self.handle, data)` names the trait and passes the receiver as the first argument. The typechecker
//! records the declared receiver mode as that argument's Rust boundary coercion; these tests pin that emission
//! consumes the recorded fact for `&mut self`, `&self` and by-value receivers alike, with no method or trait name
//! deciding the borrow.

use crate::IrCodegen;
use incan_frontend::test_support::seeded_rust_inspect_workspace;
use incan_frontend::typechecker::TypeChecker;
use incan_frontend::{lexer, parser};
use incan_lang::interop::{
    RustFunctionSig, RustItemKind, RustItemMetadata, RustParam, RustTraitAssoc, RustTraitInfo, RustTypeInfo,
    RustVisibility,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// A class holding a foreign handle, updated and finished through trait-qualified calls.
const SOURCE: &str = r#"
from rust::demo import Engine, Mac

pub class Signer:
    handle: Engine

    def update(mut self, chunk: bytes) -> None:
        Mac.update(self.handle, chunk.as_slice())

    def is_ready(self) -> bool:
        return Mac.is_ready(self.handle)

    def finish(self) -> bytes:
        return Mac.finalize(self.handle)
"#;

/// A trait method signature whose first parameter is the declared receiver.
fn trait_method(name: &str, receiver: &str, params: &[(&str, &str)], return_type: &str) -> RustTraitAssoc {
    let mut all_params = vec![RustParam {
        name: Some("self".to_string()),
        type_display: receiver.to_string(),
    }];
    all_params.extend(params.iter().map(|(name, ty)| RustParam {
        name: Some((*name).to_string()),
        type_display: (*ty).to_string(),
    }));
    RustTraitAssoc::Function {
        name: name.to_string(),
        signature: RustFunctionSig {
            receiver_contract: None,
            type_params: Vec::new(),
            params: all_params,
            return_type: return_type.to_string(),
            is_async: false,
            is_unsafe: false,
        },
    }
}

/// Generate Rust for [`SOURCE`] with the trait's three methods declared through inspected metadata.
fn generate_with_trait_metadata() -> Result<String, Box<dyn std::error::Error>> {
    let tokens = lexer::lex(SOURCE).map_err(|errors| format!("lex: {errors:?}"))?;
    let ast = parser::parse(&tokens).map_err(|errors| format!("parse: {errors:?}"))?;
    let workspace = seeded_rust_inspect_workspace()?;
    let manifest_dir = workspace.path().to_path_buf();
    let mut checker = TypeChecker::new();
    checker.set_rust_inspect_manifest_dir(manifest_dir.clone());
    checker
        .rust_inspect_cache
        .insert_test_item(
            &manifest_dir,
            RustItemMetadata {
                canonical_path: "demo::Engine".to_string(),
                definition_path: Some("demo::Engine".to_string()),
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
        .map_err(|error| format!("seed engine metadata: {error}"))?;
    checker
        .rust_inspect_cache
        .insert_test_item(
            &manifest_dir,
            RustItemMetadata {
                canonical_path: "demo::Mac".to_string(),
                definition_path: Some("demo::Mac".to_string()),
                visibility: RustVisibility::Public,
                kind: RustItemKind::Trait(RustTraitInfo {
                    items: vec![
                        trait_method("update", "&mut self", &[("data", "&[u8]")], "()"),
                        trait_method("is_ready", "&self", &[], "bool"),
                        trait_method("finalize", "self", &[], "Vec<u8>"),
                    ],
                    derive_macro: None,
                }),
            },
        )
        .map_err(|error| format!("seed trait metadata: {error}"))?;
    checker
        .check_program(&ast)
        .map_err(|errors| format!("check: {errors:?}"))?;
    let mut codegen = IrCodegen::new();
    codegen.set_prechecked_type_info(checker.type_info().clone(), std::collections::HashMap::new());
    Ok(codegen
        .try_generate(&ast)
        .map_err(|error| format!("generate: {error:?}"))?)
}

/// Each receiver mode the trait declares is the borrow the generated call passes.
#[test]
fn trait_qualified_calls_pass_the_declared_receiver_mode() -> TestResult {
    let rust = generate_with_trait_metadata()?;
    assert!(
        rust.contains("Mac::update(&mut self.handle, chunk.as_slice())"),
        "`&mut self` must become an exclusive borrow of the explicit receiver:\n{rust}"
    );
    assert!(
        rust.contains("Mac::is_ready(&self.handle)"),
        "`&self` must become a shared borrow of the explicit receiver:\n{rust}"
    );
    assert!(
        rust.contains("Mac::finalize(self.handle"),
        "a by-value `self` receiver must be passed without a borrow:\n{rust}"
    );
    assert!(
        !rust.contains("Mac::update(&self.handle"),
        "the `&mut self` receiver must never degrade to a shared borrow:\n{rust}"
    );
    Ok(())
}
