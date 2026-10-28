//! Producer-owned callable headers for private checked-product replay.
//!
//! These records preserve accepted declaration facts, not a mutable checker or an accepted implementation body.
//! They deliberately refuse declarations whose source modifiers or native callback projections the header cannot
//! represent. A caller must use ordinary checking on that refusal. Import overlays and resolution read witnesses
//! remain separate: this codec alone never authorizes skipping a body check.

use incan_semantics_core::stable_identity::{
    DeclarationSignature, StableDeclarationId, StableDeclarationLocation, StableIdentityProjection,
};
use incan_semantics_core::{CanonicalSymbolId, IncanType, SymbolOrigin};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::ast::{FunctionDecl, Span, Visibility};
use crate::library_exports::{
    CheckedExportIdentity, CheckedExportKind, CheckedFunctionExport, CheckedNamedExport, checked_function_export,
};
use crate::library_manifest::{CheckedExportReplay, LibraryManifestError};
use crate::typechecker::TypeChecker;

/// One checked free-function header, including private visibility and declaration-time mutation contracts.
/// The canonical identity is a current-compilation locator; `identity` is the existing edit-stable authority.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckedCallableInterface {
    schema: u32,
    identity: StableDeclarationId,
    is_public: bool,
    mutation: Vec<Option<bool>>,
    replay: CheckedExportReplay,
}

/// Authenticate the complete private payload before decoding it, as the checked-source artifact owner does.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct InterfaceRecord {
    schema: u32,
    digest: String,
}

impl CheckedCallableInterface {
    /// Capture one successfully checked free function without making a private declaration public.
    /// The ordinary export encoder owns callable shape; supplemental facts stay outside its public contract.
    /// Decorators, surface modifiers and consumer-selected native callback shapes currently require source checking.
    pub fn from_checked(
        declaration: &FunctionDecl,
        span: Span,
        checker: &TypeChecker,
    ) -> Result<Self, LibraryManifestError> {
        if !declaration.decorators.is_empty()
            || !declaration.surface_modifiers.is_empty()
            || checker
                .type_info()
                .rust
                .function_param_type_displays
                .contains_key(&declaration.name)
        {
            return Err(invalid(
                "callable interface requires unprojected source declaration facts",
            ));
        }
        let function = checked_function_export(declaration, checker, span)
            .ok_or_else(|| invalid("callable interface has no checked function shape"))?;
        let canonical = checker
            .type_info()
            .declarations
            .function_bindings_by_span
            .get(&span)
            .and_then(|binding| binding.identity.clone())
            .ok_or_else(|| invalid("callable interface has no checked declaration identity"))?;
        let identity = checked_identity(&canonical, &function, checker)?;
        let SymbolOrigin::Package { library, .. } = &canonical.origin else {
            return Err(invalid(
                "callable interface requires its checked producer package identity",
            ));
        };
        let package = library.clone();
        let mut source_path = checker.current_module_path.clone().unwrap_or_default();
        source_path.push(declaration.name.clone());
        let export = CheckedNamedExport {
            name: declaration.name.clone(),
            identity: CheckedExportIdentity::direct(source_path).with_canonical(Some(canonical)),
            kind: CheckedExportKind::Function(function.clone()),
        };
        let replay = CheckedExportReplay::from_checked(&package, "0.0.0", &export)?;
        let restored = replay.to_checked()?;
        if !matches!(&restored.kind, CheckedExportKind::Function(restored) if restored == &function) {
            return Err(invalid("callable interface round trip changed a checked function fact"));
        }
        let interface = Self {
            schema: 1,
            identity,
            is_public: declaration.visibility == Visibility::Public,
            mutation: declaration
                .params
                .iter()
                .map(|parameter| {
                    checker
                        .type_info()
                        .declarations
                        .mut_param_marker(parameter.span, &parameter.node.name)
                })
                .collect(),
            replay,
        };
        interface.validate()?;
        Ok(interface)
    }

    /// Borrow the stable declaration identity; source positions never participate in this key.
    pub fn identity(&self) -> &StableDeclarationId {
        &self.identity
    }

    /// Preserve visibility independently of the manifest transport used to encode the callable's shape.
    pub fn is_public(&self) -> bool {
        self.is_public
    }

    /// Borrow declaration-order mutation evidence. Absence remains unproven; it never becomes an inferred borrow.
    pub fn mutation_contract(&self) -> &[Option<bool>] {
        &self.mutation
    }

    /// Encode a validated header through the existing manifest transport, without debug-string serialization.
    pub fn to_bytes(&self) -> Result<Vec<u8>, LibraryManifestError> {
        self.validate()?;
        let payload = serde_json::to_vec(self).map_err(|error| invalid(&error.to_string()))?;
        encode_record(&payload)
    }

    /// Decode and validate all supplemental facts. Unknown, missing or malformed records are ordinary misses to
    /// the owner of a private product store; this method never turns a damaged cache into a source diagnostic.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, LibraryManifestError> {
        let boundary = bytes
            .iter()
            .position(|byte| *byte == b'\n')
            .ok_or_else(|| invalid("missing callable interface payload boundary"))?;
        let record: InterfaceRecord =
            serde_json::from_slice(&bytes[..boundary]).map_err(|error| invalid(&error.to_string()))?;
        let payload = &bytes[boundary + 1..];
        if record.schema != 1 || record.digest != hex::encode(Sha256::digest(payload)) {
            return Err(invalid("callable interface payload digest or schema differs"));
        }
        let interface: Self = serde_json::from_slice(payload).map_err(|error| invalid(&error.to_string()))?;
        interface.validate()?;
        Ok(interface)
    }

    /// Recover the accepted callable shape. Consumers must separately validate input and resolution witnesses
    /// before installing it into a checker; the canonical locator still belongs to the producer's generation.
    pub fn function(&self) -> Result<CheckedFunctionExport, LibraryManifestError> {
        self.validate()
    }

    /// Validate the complete header and its checked stable identity before exposing any retained facts.
    fn validate(&self) -> Result<CheckedFunctionExport, LibraryManifestError> {
        if self.schema != 1 {
            return Err(invalid("unsupported checked callable interface schema"));
        }
        let export = self.replay.to_checked()?;
        let CheckedExportKind::Function(function) = export.kind else {
            return Err(invalid("callable interface contains another declaration kind"));
        };
        let canonical = export
            .identity
            .canonical
            .as_ref()
            .ok_or_else(|| invalid("callable interface lost its canonical identity"))?;
        if function.params.len() != self.mutation.len()
            || self.identity.signature.is_none()
            || self.identity.location != StableDeclarationLocation::ModuleLevel
            || canonical.scope_discriminant.is_some()
            || self.identity.namespace != canonical.namespace
            || self.identity.origin != canonical.origin
            || self.identity.declaration_name != canonical.declaration_name
            || self.identity.kind != canonical.kind
        {
            return Err(invalid("callable interface identity or mutation inventory differs"));
        }
        Ok(function)
    }
}

/// Write a length-independent header followed by its exact payload, without normalizing or reinterpreting bytes.
fn encode_record(payload: &[u8]) -> Result<Vec<u8>, LibraryManifestError> {
    let record = InterfaceRecord {
        schema: 1,
        digest: hex::encode(Sha256::digest(payload)),
    };
    let mut bytes = serde_json::to_vec(&record).map_err(|error| invalid(&error.to_string()))?;
    bytes.push(b'\n');
    bytes.extend_from_slice(payload);
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::Declaration;

    /// Exercise the same parser and successful checker used by a source-unit producer, including private names.
    fn interfaces(source: &str, module: &str) -> Result<Vec<CheckedCallableInterface>, String> {
        let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
        let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
        let mut checker = TypeChecker::new();
        checker.set_current_module_path(Some(vec![module.to_owned()]));
        checker.set_current_package_identity(Some("interface_test".to_owned()));
        checker
            .check_program(&program)
            .map_err(|errors| format!("{errors:?}"))?;
        program
            .declarations
            .iter()
            .filter_map(|declaration| match &declaration.node {
                Declaration::Function(function) => Some(CheckedCallableInterface::from_checked(
                    function,
                    declaration.span,
                    &checker,
                )),
                _ => None,
            })
            .map(|result| result.map_err(|error| error.to_string()))
            .collect()
    }

    /// Private headers retain keyword/default/mutation facts through source movement, while overloads and owners
    /// remain distinct. Malformed supplemental fields and lossy projections cannot authorize header replay.
    #[test]
    fn callable_interfaces_preserve_checked_facts() -> Result<(), Box<dyn std::error::Error>> {
        let source = "def read(mut value: int, scale: int = 2) -> int:\n    return value * scale\n\npub def main() -> int:\n    return read(3)\n";
        let original = interfaces(source, "lib")?;
        let moved = interfaces(&format!("# shift declaration locators\n\n{source}"), "lib")?;
        let other = interfaces(source, "other")?;
        assert_eq!(original.len(), 2);
        for ((header, moved), other) in original.iter().zip(&moved).zip(&other) {
            let restored = CheckedCallableInterface::from_bytes(&header.to_bytes()?)?;
            assert_eq!(restored.identity(), header.identity());
            assert_eq!(restored.function()?, header.function()?);
            assert_eq!(restored.mutation_contract(), header.mutation_contract());
            assert_eq!(restored.identity(), moved.identity());
            assert_ne!(restored.identity(), other.identity());
        }
        let private = original.first().ok_or("missing private header")?;
        assert!(!private.is_public());
        assert_eq!(private.function()?.params[1].name.as_deref(), Some("scale"));
        assert!(private.function()?.params[1].has_default);
        assert_eq!(private.mutation_contract().len(), 2);
        for edited in [
            source.replace("value * scale", "value + scale"),
            source.replace("scale", "factor"),
            source.replace("= 2", "= 4"),
        ] {
            let edited = interfaces(&edited, "lib")?;
            assert_eq!(private.identity(), edited[0].identity());
        }
        let body_edit = interfaces(&source.replace("value * scale", "value + scale"), "lib")?;
        assert_eq!(private.function()?, body_edit[0].function()?);
        let keyword_edit = interfaces(&source.replace("scale", "factor"), "lib")?;
        assert_ne!(private.function()?, keyword_edit[0].function()?);
        let default_edit = interfaces(&source.replace("= 2", "= 4"), "lib")?;
        assert_ne!(private.function()?, default_edit[0].function()?);
        assert!(original.get(1).ok_or("missing public header")?.is_public());
        let encoded = private.to_bytes()?;
        let boundary = encoded.iter().position(|byte| *byte == b'\n').ok_or("missing header")?;
        let mut malformed: serde_json::Value = serde_json::from_slice(&encoded[boundary + 1..])?;
        malformed["schema"] = 99.into();
        assert!(CheckedCallableInterface::from_bytes(&encode_record(&serde_json::to_vec(&malformed)?)?).is_err());
        malformed["schema"] = 1.into();
        malformed["mutation"] = serde_json::json!([]);
        assert!(CheckedCallableInterface::from_bytes(&encode_record(&serde_json::to_vec(&malformed)?)?).is_err());
        let mut damaged = encoded;
        damaged.push(b' ');
        assert!(CheckedCallableInterface::from_bytes(&damaged).is_err());
        assert!(CheckedCallableInterface::from_bytes(b"truncated").is_err());
        let overloads = interfaces(
            "def read(value: int) -> int:\n    return value\n\ndef read(value: str) -> str:\n    return value\n",
            "lib",
        )?;
        assert_eq!(overloads.len(), 2);
        assert_ne!(overloads[0].identity(), overloads[1].identity());
        Ok(())
    }
}

/// Project only accepting-checker types and nominal bindings. Scoped owners require their producer's full lexical
/// correspondence; this free-function codec refuses them rather than replacing a scope index with a guess.
fn checked_identity(
    canonical: &CanonicalSymbolId,
    function: &CheckedFunctionExport,
    checker: &TypeChecker,
) -> Result<StableDeclarationId, LibraryManifestError> {
    let info = checker.type_info();
    let parameters: Vec<IncanType> = function
        .params
        .iter()
        .map(|parameter| info.semantic_type(&parameter.ty))
        .collect();
    let result = info.semantic_type(&function.return_type);
    let signature = DeclarationSignature::from_retained_callable_types(
        parameters.iter(),
        &result,
        &info.declarations.named_type_identities,
        &StableIdentityProjection::default(),
    )
    .ok_or_else(|| invalid("callable interface has unproven nominal signature identities"))?;
    StableDeclarationId::from_canonical(canonical, Some(signature), None)
        .ok_or_else(|| invalid("callable interface requires a producer-owned lexical context"))
}

/// Use the existing typed manifest boundary failure, so a cache owner can safely choose its ordinary miss path.
fn invalid(message: &str) -> LibraryManifestError {
    LibraryManifestError::Invalid(message.to_owned())
}
