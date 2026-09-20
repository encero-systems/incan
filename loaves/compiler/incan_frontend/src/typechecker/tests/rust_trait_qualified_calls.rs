//! Receiver mode of trait-qualified Rust calls comes from trait metadata, never from a name (#1375).
//!
//! `Trait.method(receiver, args...)` passes its receiver explicitly. The Rust declaration's receiver decides whether
//! that argument is borrowed exclusively, shared, or moved; these tests pin that the typechecker records that mode
//! as the first argument's boundary coercion when metadata declares it, and refuses the call when it cannot know.

use super::*;
use incan_lang::interop::{RustItemKind, RustItemMetadata, RustTraitAssoc, RustTraitInfo, RustTypeInfo};

/// One Incan class holding a foreign handle whose `update` is called through the trait path.
const TRAIT_QUALIFIED_UPDATE: &str = r#"
from rust::demo import Engine, Mac

pub class Signer:
    handle: Engine

    def update(mut self, chunk: bytes) -> None:
        Mac.update(self.handle, chunk.as_slice())
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

/// Seed inspected metadata for a foreign handle type and a trait with the given associated functions.
fn checker_with_seeded_trait(
    items: Vec<RustTraitAssoc>,
) -> Result<(TypeChecker, tempfile::TempDir), Box<dyn std::error::Error>> {
    let mut checker = TypeChecker::new();
    let workspace = seeded_rust_inspect_workspace()?;
    let manifest_dir = workspace.path().to_path_buf();
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
        .map_err(|error| std::io::Error::other(format!("seed engine metadata: {error}")))?;
    checker
        .rust_inspect_cache
        .insert_test_item(
            &manifest_dir,
            RustItemMetadata {
                canonical_path: "demo::Mac".to_string(),
                definition_path: Some("demo::Mac".to_string()),
                visibility: RustVisibility::Public,
                kind: RustItemKind::Trait(RustTraitInfo {
                    items,
                    derive_macro: None,
                }),
            },
        )
        .map_err(|error| std::io::Error::other(format!("seed trait metadata: {error}")))?;
    Ok((checker, workspace))
}

/// Typecheck `source` with the seeded checker and return the recorded first-argument coercion of the call.
fn check_and_find_receiver_coercion(
    checker: &mut TypeChecker,
    source: &str,
) -> Result<Option<RustArgCoercionInfo>, Box<dyn std::error::Error>> {
    let tokens = lexer::lex(source).map_err(|errors| std::io::Error::other(format!("lex failed: {errors:?}")))?;
    let ast = parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("parse failed: {errors:?}")))?;
    checker.check_program(&ast).map_err(|errors| {
        std::io::Error::other(format!("expected the trait-qualified call to typecheck: {errors:?}"))
    })?;
    let receiver_start = source
        .find("self.handle")
        .ok_or("fixture must pass `self.handle` as the receiver")?;
    let receiver_end = receiver_start + "self.handle".len();
    Ok(checker
        .type_info()
        .rust
        .arg_coercions
        .get(&(receiver_start, receiver_end))
        .cloned())
}

/// `&mut self` in the trait declaration becomes an exclusive borrow of the explicit receiver argument.
#[test]
fn mutable_trait_receiver_records_exclusive_borrow() -> Result<(), Box<dyn std::error::Error>> {
    let (mut checker, _workspace) =
        checker_with_seeded_trait(vec![trait_method("update", "&mut self", &[("data", "&[u8]")], "()")])?;
    let coercion = check_and_find_receiver_coercion(&mut checker, TRAIT_QUALIFIED_UPDATE)?
        .ok_or("expected a receiver coercion for the trait-qualified call")?;
    assert_eq!(coercion.kind, RustArgCoercionKind::Borrow { mutable: true });
    assert_eq!(coercion.rust_target_type, "&mut self");
    assert!(
        matches!(coercion.target_type, ResolvedType::RefMut(_)),
        "{:?}",
        coercion.target_type
    );
    Ok(())
}

/// `&self` in the trait declaration becomes a shared borrow; the method name plays no part.
#[test]
fn shared_trait_receiver_records_shared_borrow() -> Result<(), Box<dyn std::error::Error>> {
    let (mut checker, _workspace) =
        checker_with_seeded_trait(vec![trait_method("update", "&self", &[("data", "&[u8]")], "()")])?;
    let coercion = check_and_find_receiver_coercion(&mut checker, TRAIT_QUALIFIED_UPDATE)?
        .ok_or("expected a receiver coercion for the trait-qualified call")?;
    assert_eq!(coercion.kind, RustArgCoercionKind::Borrow { mutable: false });
    assert!(
        matches!(coercion.target_type, ResolvedType::Ref(_)),
        "{:?}",
        coercion.target_type
    );
    Ok(())
}

/// A by-value `self` receiver is moved: no borrow is recorded for the explicit receiver.
#[test]
fn owned_trait_receiver_records_no_borrow() -> Result<(), Box<dyn std::error::Error>> {
    let (mut checker, _workspace) =
        checker_with_seeded_trait(vec![trait_method("update", "self", &[("data", "&[u8]")], "()")])?;
    assert!(check_and_find_receiver_coercion(&mut checker, TRAIT_QUALIFIED_UPDATE)?.is_none());
    Ok(())
}

/// The call-site parameter list keeps the receiver in first position so arguments and parameters stay aligned.
#[test]
fn trait_qualified_call_site_params_start_with_the_receiver() -> Result<(), Box<dyn std::error::Error>> {
    let (mut checker, _workspace) =
        checker_with_seeded_trait(vec![trait_method("update", "&mut self", &[("data", "&[u8]")], "()")])?;
    check_and_find_receiver_coercion(&mut checker, TRAIT_QUALIFIED_UPDATE)?;
    let call_start = TRAIT_QUALIFIED_UPDATE
        .find("Mac.update(")
        .ok_or("fixture must contain the trait-qualified call")?;
    let params = checker
        .type_info()
        .calls
        .call_site_callable_params
        .iter()
        .find(|((start, _), _)| *start == call_start)
        .map(|(_, params)| params.clone())
        .ok_or("expected call-site parameters for the trait-qualified call")?;
    assert_eq!(params.len(), 2, "{params:?}");
    assert_eq!(params[0].name.as_deref(), Some("self"));
    assert!(matches!(params[0].ty, ResolvedType::RefMut(_)), "{:?}", params[0].ty);
    assert_eq!(params[1].name.as_deref(), Some("data"));
    Ok(())
}

/// `Self` in a trait method's result is the receiver's type, not the trait, and a projection through a method type
/// parameter the call never binds stays permissive rather than becoming a nonsense `rust::S::Ok`.
#[test]
fn trait_qualified_results_resolve_self_to_the_receiver_and_keep_projections_open()
-> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
from rust::demo import Engine, Mac

pub class Signer:
    handle: Engine

    def clone_engine(self) -> Engine:
        return Mac.duplicate(self.handle)

    def render(self) -> str:
        rendered: str = Mac.render(self.handle, "sink")
        return rendered
"#;
    let (mut checker, _workspace) =
        checker_with_seeded_trait(vec![trait_method("duplicate", "&self", &[], "Self"), {
            let RustTraitAssoc::Function { name, mut signature } =
                trait_method("render", "&self", &[("sink", "S")], "Result<S::Ok, S::Error>")
            else {
                return Err("trait_method always builds a function item".into());
            };
            signature.type_params = vec!["S".to_string()];
            RustTraitAssoc::Function { name, signature }
        }])?;
    let tokens = lexer::lex(source).map_err(|errors| std::io::Error::other(format!("lex failed: {errors:?}")))?;
    let ast = parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("parse failed: {errors:?}")))?;
    checker.check_program(&ast).map_err(|errors| {
        std::io::Error::other(format!("expected both trait-qualified calls to typecheck: {errors:?}"))
    })?;
    let duplicate_start = source
        .find("Mac.duplicate(self.handle)")
        .ok_or("fixture must call Mac.duplicate")?;
    let duplicate_ty = checker
        .type_info()
        .expr_type(Span::new(
            duplicate_start,
            duplicate_start + "Mac.duplicate(self.handle)".len(),
        ))
        .cloned()
        .ok_or("expected a recorded result type for Mac.duplicate")?;
    assert_eq!(duplicate_ty, ResolvedType::RustPath("demo::Engine".to_string()));
    Ok(())
}

/// A receiver-less trait function such as `Deserialize.deserialize(input)` keeps `Self` open for context inference.
#[test]
fn receiverless_trait_function_keeps_self_permissive() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
from rust::demo import Engine, Mac

pub def rebuild(seed: int) -> Result[Engine, str]:
    result: Result[Engine, str] = Mac.rebuild(seed)
    return result
"#;
    let (mut checker, _workspace) = checker_with_seeded_trait(vec![RustTraitAssoc::Function {
        name: "rebuild".to_string(),
        signature: RustFunctionSig {
            receiver_contract: None,
            type_params: vec!["D".to_string()],
            params: vec![RustParam {
                name: Some("seed".to_string()),
                type_display: "D".to_string(),
            }],
            return_type: "Result<Self, D::Error>".to_string(),
            is_async: false,
            is_unsafe: false,
        },
    }])?;
    let tokens = lexer::lex(source).map_err(|errors| std::io::Error::other(format!("lex failed: {errors:?}")))?;
    let ast = parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("parse failed: {errors:?}")))?;
    checker
        .check_program(&ast)
        .map_err(|errors| std::io::Error::other(format!("expected the receiver-less call to typecheck: {errors:?}")))?;
    Ok(())
}

/// A method the inspected trait does not declare is refused at the source call.
#[test]
fn trait_qualified_call_to_undeclared_method_is_refused() -> Result<(), Box<dyn std::error::Error>> {
    let (mut checker, _workspace) = checker_with_seeded_trait(vec![trait_method("finalize", "self", &[], "Vec<u8>")])?;
    let tokens = lexer::lex(TRAIT_QUALIFIED_UPDATE)
        .map_err(|errors| std::io::Error::other(format!("lex failed: {errors:?}")))?;
    let ast = parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("parse failed: {errors:?}")))?;
    let errors = checker
        .check_program(&ast)
        .err()
        .ok_or("expected the undeclared trait method to be refused")?;
    assert!(
        errors.iter().any(|error| error
            .message
            .contains("does not declare an associated function `update`")),
        "{errors:?}"
    );
    Ok(())
}

/// Without a signature the receiver mode is unknowable, so the call is refused instead of assuming `&self`.
///
/// `hmac::Mac` is in the compiler's fallback trait vocabulary, so the typechecker knows `Mac.update` is a
/// trait-qualified call even though no metadata was inspected.
#[test]
fn trait_qualified_call_without_signature_is_refused() {
    let errors = check_str_err(
        r#"
from rust::hmac import Hmac, Mac

pub class Signer:
    handle: Hmac

    def update(mut self, chunk: bytes) -> None:
        Mac.update(self.handle, chunk.as_slice())
"#,
        "a trait-qualified call with no receiver signature must not typecheck",
    );
    assert!(
        errors.iter().any(|error| {
            error
                .message
                .contains("Cannot determine the receiver of `rust::hmac::Mac.update`")
        }),
        "{errors:?}"
    );
}

/// Method syntax on the value receiver stays available when trait metadata is missing.
#[test]
fn method_syntax_on_the_receiver_value_is_still_accepted_without_metadata() {
    assert_check_ok(
        r#"
from rust::hmac import Hmac, Mac

pub class Signer:
    handle: Hmac

    def update(mut self, chunk: bytes) -> None:
        self.handle.update(chunk.as_slice())
"#,
    );
}
