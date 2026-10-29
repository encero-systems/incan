//! Producer-owned callable headers and admitted private body products.
//!
//! Callable headers preserve accepted declaration facts without retaining a mutable checker. Body products add an
//! explicitly bounded acceptance for authored scalar expressions and witnessed source calls. Import overlays, general
//! resolution witnesses, generated syntax and unsupported effects retain ordinary source checking. A header alone
//! never authorizes skipping a body check.

use std::collections::BTreeMap;

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
use crate::library_manifest::{CheckedExportReplay, LibraryManifestError, TypeRef, VisitTypeRefs};
use crate::typechecker::TypeChecker;

mod body_products;
mod callable_module;

pub use body_products::{CallableBodyInputs, CallableBodyProducts, ValidatedCallableBodies};
pub use callable_module::{CallableLookupWitness, CheckedCallableModule};

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

/// Independent declaration-contract and checked-body digests under the producer's edit-stable identity.
/// Computing this record after checking establishes meaning; it does not supply a pre-check cache-selection key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckedDeclarationFingerprints {
    /// The declaration identity, including its checked overload discriminator.
    pub identity: StableDeclarationId,
    /// Complete callable contract, including defaults, parameter roles, visibility and mutation evidence.
    pub signature: String,
    /// Checked implementation meaning projected through the retained nominal and lexical correspondences.
    pub body: String,
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
        // ---- Admission: retain the accepting checker's declaration authority ----
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

        // ---- Projection: prove that the ordinary transport preserves the complete callable shape ----
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

        // ---- Private facts: retain visibility and declaration-order mutation evidence ----
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
        let payload = decode_record(bytes)?;
        let interface: Self = serde_json::from_slice(payload).map_err(|error| invalid(&error.to_string()))?;
        interface.validate()?;
        Ok(interface)
    }

    /// Recover the accepted callable shape. Consumers must separately validate input and resolution witnesses
    /// before installing it into a checker; the canonical locator still belongs to the producer's generation.
    pub fn function(&self) -> Result<CheckedFunctionExport, LibraryManifestError> {
        self.validate()
    }

    /// Fingerprint the full accepted callable contract, independently of its overload discriminator and emitted
    /// frame name. Nominal leaves use the accepting checker's bindings; an unproven carrier is a cache miss.
    pub fn signature_digest(
        &self,
        bindings: &BTreeMap<String, CanonicalSymbolId>,
        projection: &StableIdentityProjection,
    ) -> Result<String, LibraryManifestError> {
        self.validate()?;
        let mut function = self.replay.callable_contract()?;
        function.emitted_name = None;
        let mut complete = true;
        function.visit_type_refs(&mut |ty| match ty {
            TypeRef::Named { name, .. } | TypeRef::Applied { name, .. } => match projection.type_name(name, bindings) {
                Some(stable) => *name = stable,
                None => complete = false,
            },
            TypeRef::NativeUnion(_) | TypeRef::Unknown => complete = false,
            _ => {}
        });
        if !complete {
            return Err(invalid("callable signature has unproven semantic type leaves"));
        }
        incan_semantics_core::semantic_digest::semantic_digest(&(
            "incan.checked-callable-contract/1",
            &self.identity,
            self.is_public,
            &self.mutation,
            function,
        ))
        .map_err(|error| invalid(&error.to_string()))
    }

    /// Rebind a retained header to an independently accepted current header. This transfers only the root canonical
    /// locator; nested types and defaults must already be identical. More general carrier replay requires typed
    /// correspondence and refuses here, even if two semantic fingerprints happen to match.
    pub fn replay_at(&self, current: &Self) -> Result<CheckedNamedExport, LibraryManifestError> {
        if self.identity != current.identity
            || self.is_public != current.is_public
            || self.mutation != current.mutation
            || self.function()? != current.function()?
        {
            return Err(invalid(
                "callable replay requires an unchanged complete checked contract",
            ));
        }
        let mut retained = self.replay.to_checked()?;
        retained.identity = current.replay.to_checked()?.identity;
        Ok(retained)
    }

    /// Pair the complete callable digest with its checked implementation digest. The body must name this exact
    /// current declaration; specialized frames and native macros need their own private-product supplements and
    /// refuse here because ordinary Body serialization intentionally skips those facts.
    pub fn fingerprints(
        &self,
        body: &mut incan_semantics_core::body_ir::Body,
        projection: &StableIdentityProjection,
    ) -> Result<CheckedDeclarationFingerprints, LibraryManifestError> {
        let canonical = self.replay.to_checked()?.identity.canonical;
        if canonical.is_none()
            || body.canonical != canonical
            || !body.nominal_instantiation.is_empty()
            || body.trait_default_instantiation.is_some()
            || !body.native_attributes.is_empty()
        {
            return Err(invalid(
                "callable fingerprints require the exact unspecialized checked source body",
            ));
        }
        let signature = self.signature_digest(&body.named_type_identities, projection)?;
        let digest = incan_semantics_core::semantic_digest::body_semantic_digest(body, projection)
            .map_err(|error| invalid(&error.to_string()))?;
        Ok(CheckedDeclarationFingerprints {
            identity: self.identity.clone(),
            signature,
            body: digest,
        })
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

/// Authenticate a private interface payload before any shape or supplemental facts are decoded.
fn decode_record(bytes: &[u8]) -> Result<&[u8], LibraryManifestError> {
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
    Ok(payload)
}

#[cfg(test)]
mod tests {
    use super::{CallableBodyProducts, CheckedCallableInterface, CheckedCallableModule, encode_record};
    use crate::ast::{Declaration, Program};
    use crate::typechecker::TypeChecker;

    /// Exercise the same parser and successful checker used by a source-unit producer, including private names.
    fn interfaces(source: &str, module: &str) -> Result<Vec<CheckedCallableInterface>, String> {
        let (program, checker) = checked(source, module)?;
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

    /// Keep module/header tests on the same accepted parser and checker inputs instead of inventing fact records.
    fn checked(source: &str, module: &str) -> Result<(Program, TypeChecker), String> {
        let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
        let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
        let mut checker = TypeChecker::new();
        checker.set_current_module_path(Some(vec![module.to_owned()]));
        checker.set_current_package_identity(Some("interface_test".to_owned()));
        checker
            .check_program(&program)
            .map_err(|errors| format!("{errors:?}"))?;
        Ok((program, checker))
    }

    /// Private headers retain keyword/default/mutation facts through source movement, while overloads and owners
    /// remain distinct. Malformed supplemental fields and lossy projections cannot authorize header replay.
    #[test]
    fn callable_interfaces_preserve_checked_facts() -> Result<(), Box<dyn std::error::Error>> {
        // ---- Source generations: preserve facts while declaration coordinates and owners change ----
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

        // ---- Contract edits: distinguish implementation changes from keyword names and defaults ----
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

        // ---- Invalid records: reject incompatible fields and damaged transport bytes ----
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

        // ---- Overloads: keep same-named declarations distinct by their checked signatures ----
        let overloads = interfaces(
            "def read(value: int) -> int:\n    return value\n\ndef read(value: str) -> str:\n    return value\n",
            "lib",
        )?;
        assert_eq!(overloads.len(), 2);
        assert_ne!(overloads[0].identity(), overloads[1].identity());
        callable_namespace_witnesses(source)?;
        nominal_signature_witnesses()?;
        declaration_fingerprint_edits(source)?;
        scalar_body_product_edits()?;
        scalar_call_product_edits()?;
        scalar_local_product_edits()?;
        Ok(())
    }

    /// Exercise real lookup additions/removals, complete contract edits and replay into moved canonical locators.
    fn callable_namespace_witnesses(source: &str) -> Result<(), Box<dyn std::error::Error>> {
        // ---- Namespace capture: keep every overload and private callable through the exact private codec ----
        let capture = |source: &str, module: &str| -> Result<CheckedCallableModule, Box<dyn std::error::Error>> {
            let (program, checker) = checked(source, module)?;
            Ok(CheckedCallableModule::from_checked(&program, &checker)?)
        };
        let original = capture(source, "lib")?;
        let encoded = original.to_bytes()?;
        let retained = CheckedCallableModule::from_bytes(&encoded)?;
        let read = retained.lookup("read");
        let absent = retained.lookup("absent");
        let moved = capture(&format!("# moved root locators\n\n{source}"), "lib")?;
        let body_edit = capture(&source.replace("value * scale", "value + scale"), "lib")?;
        for current in [&moved, &body_edit] {
            assert!(current.validates(&read));
            assert!(current.validates(&absent));
            let replayed = retained.replay_at(current)?;
            let expected = current.replay_at(current)?;
            assert_eq!(replayed.len(), expected.len());
            for (replayed, expected) in replayed.iter().zip(expected) {
                assert_eq!(replayed.identity, expected.identity);
                let (
                    crate::library_exports::CheckedExportKind::Function(replayed),
                    crate::library_exports::CheckedExportKind::Function(expected),
                ) = (&replayed.kind, &expected.kind)
                else {
                    return Err("replayed callable changed declaration kind".into());
                };
                assert_eq!(replayed, expected);
            }
        }
        assert_ne!(
            retained.replay_at(&retained)?[0].identity,
            retained.replay_at(&moved)?[0].identity
        );

        // ---- Invalidations: full contracts and positive/negative candidate sets are independent of selection ----
        for edited in [source.replace("scale", "factor"), source.replace("= 2", "= 4")] {
            let edited = capture(&edited, "lib")?;
            assert!(!edited.validates(&read));
            assert!(edited.validates(&absent));
            assert!(retained.replay_at(&edited).is_err());
        }
        let added = capture(&format!("{source}\ndef absent() -> int:\n    return 0\n"), "lib")?;
        assert!(!added.validates(&absent));
        assert!(added.validates(&read));
        assert!(!capture(source, "other")?.validates(&absent));
        let single = "def read(value: int) -> int:\n    return value\n";
        let single_module = capture(single, "lib")?;
        let overloads = capture(
            &format!("{single}\ndef read(value: str) -> str:\n    return value\n"),
            "lib",
        )?;
        assert!(!overloads.validates(&single_module.lookup("read")));
        assert!(!single_module.validates(&overloads.lookup("read")));

        // ---- Refusals: a different namespace category cannot masquerade as an absent callable ----
        let (model, checker) = checked("model Hidden:\n    value: int\n", "lib")?;
        assert!(CheckedCallableModule::from_checked(&model, &checker).is_err());
        let mut damaged = encoded;
        damaged.push(b' ');
        assert!(CheckedCallableModule::from_bytes(&damaged).is_err());
        let mut malformed = serde_json::to_value(&retained)?;
        let entries = malformed["callables"]
            .as_array_mut()
            .ok_or("missing callable inventory")?;
        entries.push(entries[0].clone());
        assert!(CheckedCallableModule::from_bytes(&encode_record(&serde_json::to_vec(&malformed)?)?).is_err());
        Ok(())
    }

    /// Nominal type positions project through checked identities without rewriting literals or source spellings.
    fn nominal_signature_witnesses() -> Result<(), Box<dyn std::error::Error>> {
        let source = "model Hidden:\n    value: int\n\ndef read(value: Hidden) -> Hidden:\n    return value\n";
        let fingerprint = |source: &str| -> Result<String, Box<dyn std::error::Error>> {
            let (program, checker) = checked(source, "lib")?;
            let declaration = program.declarations.last().ok_or("missing nominal callable")?;
            let Declaration::Function(function) = &declaration.node else {
                return Err("expected nominal callable".into());
            };
            let header = CheckedCallableInterface::from_checked(function, declaration.span, &checker)?;
            Ok(header.signature_digest(
                &checker.type_info().declarations.named_type_identities,
                &incan_semantics_core::stable_identity::StableIdentityProjection::default(),
            )?)
        };
        assert_eq!(
            fingerprint(source)?,
            fingerprint(&format!("# move nominal declarations\n{source}"))?
        );
        assert_ne!(fingerprint(source)?, fingerprint(&source.replace("Hidden", "Other"))?);
        Ok(())
    }

    /// A private implementation edit changes one body key and no declaration contract; locators do not become keys.
    fn declaration_fingerprint_edits(source: &str) -> Result<(), Box<dyn std::error::Error>> {
        let fingerprints =
            |source: &str| -> Result<Vec<super::CheckedDeclarationFingerprints>, Box<dyn std::error::Error>> {
                let (program, checker) = checked(source, "lib")?;
                let providers = crate::body_ir::ProviderOperationCatalog::new();
                let mut module = crate::body_ir::build_body_ir_module(
                    &program,
                    &["lib".to_owned()],
                    checker.type_info(),
                    &crate::body_ir::BodyIrContext {
                        modules: &[],
                        provider_operations: &providers,
                        registry_package: None,
                        library_package: Some("interface_test"),
                    },
                );
                let headers = interfaces(source, "lib")?;
                let mut projection = incan_semantics_core::stable_identity::StableIdentityProjection::default();
                let facts = checker.type_info().semantic_fact_store(&["lib".to_owned()]);
                for subject in facts.subjects() {
                    if let (Some(identity), Some(context)) = (
                        facts.symbol_identities_for(subject).next(),
                        facts.stable_declaration_contexts_for(subject).next(),
                    ) {
                        projection.contexts.insert(identity.clone(), context.clone());
                    }
                }
                for (header, body) in headers.iter().zip(&module.bodies) {
                    if let (Some(canonical), Some(signature)) = (&body.canonical, &header.identity.signature) {
                        projection.signatures.insert(canonical.clone(), signature.clone());
                    }
                }
                let mut products = Vec::new();
                for (header, body) in headers.iter().zip(&mut module.bodies) {
                    products.push(header.fingerprints(body, &projection)?);
                }
                assert_eq!(products.len(), headers.len());
                Ok(products)
            };
        let original = fingerprints(source)?;
        let edited = fingerprints(&source.replace("value * scale", "value + scale"))?;
        assert_eq!(original.len(), 2);
        assert_eq!(
            original
                .iter()
                .zip(&edited)
                .filter(|(old, new)| old.body != new.body)
                .count(),
            1
        );
        assert!(
            original
                .iter()
                .zip(&edited)
                .all(|(old, new)| old.signature == new.signature)
        );
        assert_eq!(original, fingerprints(&format!("# move all locators\n\n{source}"))?);
        let signature_edit = fingerprints(&source.replace("= 2", "= 4"))?;
        assert_eq!(
            original
                .iter()
                .zip(&signature_edit)
                .filter(|(old, new)| old.signature != new.signature)
                .count(),
            1
        );
        Ok(())
    }

    /// Reuse two untouched scalar bodies while the third is independently checked and built from its edited input.
    /// Unsupported reads, changed owners, moved locators and corrupt products remain safe misses.
    fn scalar_body_product_edits() -> Result<(), Box<dyn std::error::Error>> {
        let source = "def first(value: int) -> int:\n    return value * 2\n\ndef second(value: int) -> int:\n    return value + 3\n\npub def third(value: int) -> int:\n    return value - 4\n";
        let (program, checker) = checked(source, "lib")?;
        let module = scalar_module(&program, &checker);
        let products = CallableBodyProducts::from_checked(&program, source, &checker, &module)?;
        let products = CallableBodyProducts::from_bytes(&products.to_bytes()?)?;
        let edited = source.replace("value * 2", "value * 7");
        let (current, cold) = checked(&edited, "lib")?;
        let selection = products.select(&current, &edited, "interface_test", &["lib".to_owned()])?;
        assert_eq!(selection.len(), 2);
        let mut warm = TypeChecker::new();
        warm.set_current_module_path(Some(vec!["lib".to_owned()]));
        warm.set_current_package_identity(Some("interface_test".to_owned()));
        warm.set_callable_body_reuse(selection);
        warm.check_program(&current).map_err(|errors| format!("{errors:?}"))?;
        assert_eq!(warm.type_info().reused_callable_bodies.len(), 2);
        assert_eq!(scalar_module(&current, &warm), scalar_module(&current, &cold));
        let retained_again =
            CallableBodyProducts::from_checked(&current, &edited, &warm, &scalar_module(&current, &warm))?;
        assert_eq!(
            retained_again
                .select(&current, &edited, "interface_test", &["lib".to_owned()])?
                .len(),
            3
        );
        let late_edit = source.replace("value - 4", "value - 8");
        let (late_program, late_cold) = checked(&late_edit, "lib")?;
        let mut late_warm = TypeChecker::new();
        late_warm.set_current_module_path(Some(vec!["lib".to_owned()]));
        late_warm.set_current_package_identity(Some("interface_test".to_owned()));
        late_warm.set_callable_body_reuse(products.select(
            &late_program,
            &late_edit,
            "interface_test",
            &["lib".to_owned()],
        )?);
        late_warm
            .check_program(&late_program)
            .map_err(|errors| format!("{errors:?}"))?;
        assert_eq!(
            scalar_module(&late_program, &late_warm),
            scalar_module(&late_program, &late_cold)
        );
        assert!(
            products
                .select(&current, &edited, "other", &["lib".to_owned()])?
                .is_empty()
        );

        // ---- Invalidations: unsupported syntax and current source coordinates never borrow an old acceptance ----
        let moved_source = format!("# move locators\n{source}");
        let (moved, _) = checked(&moved_source, "lib")?;
        assert!(
            products
                .select(&moved, &moved_source, "interface_test", &["lib".to_owned()])?
                .is_empty()
        );
        for invalid in [
            source.replace("value * 2", "missing()"),
            source.replace("value * 2", "missing"),
            format!("from std.math import sqrt\n{source}"),
            source.replace("value: int", "mut value: int"),
        ] {
            let tokens = crate::lexer::lex(&invalid).map_err(|errors| format!("{errors:?}"))?;
            let parsed = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
            if let Ok(selection) = products.select(&parsed, &invalid, "interface_test", &["lib".to_owned()]) {
                assert!(selection.is_empty());
            }
        }
        let mut damaged = products.to_bytes()?;
        damaged.pop();
        assert!(CallableBodyProducts::from_bytes(&damaged).is_err());
        let mut expanded = program.clone();
        if let Some(declaration) = expanded.declarations.first_mut() {
            if let Declaration::Function(function) = &mut declaration.node {
                if let Some(statement) = function.body.first_mut() {
                    statement.span.expansion = Some([7; 32]);
                }
            }
        }
        assert!(
            products
                .select(&expanded, source, "interface_test", &["lib".to_owned()])?
                .is_empty()
        );
        let mut sourceless = program.clone();
        if let Some(declaration) = sourceless.declarations.first_mut() {
            if let Declaration::Function(function) = &mut declaration.node {
                if let Some(statement) = function.body.first_mut() {
                    if let crate::ast::Statement::Return(Some(value)) = &mut statement.node {
                        value.span = crate::ast::Span::default();
                    }
                }
            }
        }
        assert!(
            products
                .select(&sourceless, source, "interface_test", &["lib".to_owned()])?
                .is_empty()
        );
        Ok(())
    }

    /// Retain complete source-call reads, preserve edited/cold IR and invalidate only consumed signatures when
    /// physical locators stay fixed. Missing candidates or incomplete retained reads never authorize a hit.
    fn scalar_call_product_edits() -> Result<(), Box<dyn std::error::Error>> {
        let source = "\"\"\"Scalar module.\"\"\"\ndef first(value: int) -> int:\n    return value * 2\n\ndef second(value: int) -> int:\n    return value + 3\n\ndef third(value: int) -> int:\n    return value - 4\n\npub def answer(value: int) -> int:\n    return first(value) + second(value) + third(value)\n";
        let (program, checker) = checked(source, "lib")?;
        let products =
            CallableBodyProducts::from_checked(&program, source, &checker, &scalar_module(&program, &checker))?;
        let products = CallableBodyProducts::from_bytes(&products.to_bytes()?)?;
        let edited = source.replace("value * 2", "value * 7");
        let (current, cold) = checked(&edited, "lib")?;
        let selection = products.select(&current, &edited, "interface_test", &["lib".to_owned()])?;
        assert_eq!(
            selection.len(),
            3,
            "the unchanged caller must retain its checked signature reads"
        );
        let mut warm = TypeChecker::new();
        warm.set_current_module_path(Some(vec!["lib".to_owned()]));
        warm.set_current_package_identity(Some("interface_test".to_owned()));
        warm.set_callable_body_reuse(selection);
        warm.check_program(&current).map_err(|errors| format!("{errors:?}"))?;
        assert_eq!(warm.type_info().reused_callable_bodies.len(), 3);
        assert_eq!(scalar_module(&current, &warm), scalar_module(&current, &cold));
        let retained = CallableBodyProducts::from_checked(&current, &edited, &warm, &scalar_module(&current, &warm))?;
        assert_eq!(retained.native_inputs()?.len(), 4);

        // ---- Invalidations: a consumed header changes while unrelated implementations stay accepted ----
        let changed = source.replacen("-> int", "-> f64", 1);
        let tokens = crate::lexer::lex(&changed).map_err(|errors| format!("{errors:?}"))?;
        let signature = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
        let selection = products.select(&signature, &changed, "interface_test", &["lib".to_owned()])?;
        assert_eq!(
            selection.len(),
            2,
            "a changed callee signature must invalidate its unchanged caller"
        );
        let mut rejected = TypeChecker::new();
        rejected.set_current_module_path(Some(vec!["lib".to_owned()]));
        rejected.set_current_package_identity(Some("interface_test".to_owned()));
        rejected.set_callable_body_reuse(selection);
        assert!(rejected.check_program(&signature).is_err());
        let missing_source = source.replace("first(value) +", "other(value) +");
        let tokens = crate::lexer::lex(&missing_source).map_err(|errors| format!("{errors:?}"))?;
        let missing = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
        let selection = products.select(&missing, &missing_source, "interface_test", &["lib".to_owned()])?;
        assert_eq!(
            selection.len(),
            3,
            "a missing candidate must leave the caller on the full path"
        );
        let added_source = format!("{source}\ndef fourth() -> int:\n    return 0\n");
        let (added, _) = checked(&added_source, "lib")?;
        assert!(
            products
                .select(&added, &added_source, "interface_test", &["lib".to_owned()])?
                .is_empty()
        );

        // ---- Transport: losing a read cannot turn an incomplete product into an accepted caller ----
        let mut record: serde_json::Value = serde_json::from_slice(super::decode_record(&products.to_bytes()?)?)?;
        let reads = record
            .get_mut("entries")
            .and_then(serde_json::Value::as_array_mut)
            .and_then(|entries| entries.get_mut(3))
            .and_then(|entry| entry.get_mut("reads"))
            .ok_or("the caller product has no retained read field")?;
        *reads = serde_json::json!([]);
        let damaged = encode_record(&serde_json::to_vec(&record)?)?;
        if let Ok(incomplete) = CallableBodyProducts::from_bytes(&damaged) {
            assert_eq!(
                incomplete
                    .select(&program, source, "interface_test", &["lib".to_owned()])?
                    .len(),
                3
            );
        }
        Ok(())
    }

    /// Exercise immutable locals and primitive mutation through the same complete product replay boundary.
    fn scalar_local_product_edits() -> Result<(), Box<dyn std::error::Error>> {
        let source = "def first(value: int) -> int:\n    \"\"\"Compute the first value.\"\"\"\n    doubled = value * 2\n    result = doubled + 1\n    return result\n\ndef second(value: int) -> int:\n    \"\"\"Compute the second value.\"\"\"\n    result: int = value + 3\n    return result\n\ndef third(value: int) -> int:\n    \"\"\"Compute the third value.\"\"\"\n    result = value - 4\n    return result\n\npub def answer(value: int) -> int:\n    \"\"\"Combine the private results.\"\"\"\n    initial = first(value)\n    remaining = second(value) + third(value)\n    return initial + remaining\n";
        scalar_local_source_edits(source)?;
        let mutable = source
            .replace("first(value:", "first(mut value:")
            .replace("doubled = value * 2", "value = value * 2\n    mut doubled = value")
            .replace(
                "result: int = value + 3",
                "mut result: int = value + 3\n    result = result + 1\n    result = result - 1",
            );
        scalar_local_source_edits(&mutable)
    }

    /// Replay documented scalar locals with exact canonical identities, mutation facts and witnessed calls.
    /// Unknown reassignment targets, shadowing bindings, forward reads and generated local names refuse capture.
    fn scalar_local_source_edits(source: &str) -> Result<(), Box<dyn std::error::Error>> {
        let (program, checker) = checked(source, "lib")?;
        let products =
            CallableBodyProducts::from_checked(&program, source, &checker, &scalar_module(&program, &checker))?;
        let products = CallableBodyProducts::from_bytes(&products.to_bytes()?)?;
        let edited = source.replace("value * 2", "value * 7");
        let (current, cold) = checked(&edited, "lib")?;
        let mut warm = TypeChecker::new();
        warm.set_current_module_path(Some(vec!["lib".to_owned()]));
        warm.set_current_package_identity(Some("interface_test".to_owned()));
        let selected = products.select(&current, &edited, "interface_test", &["lib".to_owned()])?;
        assert_eq!(selected.len(), 3);
        warm.set_callable_body_reuse(selected);
        warm.check_program(&current).map_err(|errors| format!("{errors:?}"))?;
        assert_eq!(warm.type_info().reused_callable_bodies.len(), 3);
        assert_eq!(scalar_module(&current, &warm), scalar_module(&current, &cold));
        let current_products =
            CallableBodyProducts::from_checked(&current, &edited, &warm, &scalar_module(&current, &warm))?;
        assert_eq!(
            current_products
                .select(&current, &edited, "interface_test", &["lib".to_owned()])?
                .len(),
            4
        );

        // ---- Refusals: isolate grammar guards while retaining the exact old physical input ----
        for kind in [crate::ast::BindingKind::Let, crate::ast::BindingKind::Reassign] {
            let mut changed = program.clone();
            let assignment = first_local_assignment(&mut changed)?;
            assignment.binding = kind;
            assignment.name = if kind == crate::ast::BindingKind::Let {
                "value"
            } else {
                "unbound"
            }
            .to_owned();
            assert!(
                products
                    .select(&changed, source, "interface_test", &["lib".to_owned()])?
                    .is_empty()
            );
        }
        let mut generated = program.clone();
        let assignment = first_local_assignment(&mut generated)?;
        assignment.name_span = crate::ast::Span::default();
        assert!(
            products
                .select(&generated, source, "interface_test", &["lib".to_owned()])?
                .is_empty()
        );
        let forward = source.replace("value * 2", "missing");
        let tokens = crate::lexer::lex(&forward).map_err(|errors| format!("{errors:?}"))?;
        let parsed = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
        let mut rejected = TypeChecker::new();
        rejected.set_current_module_path(Some(vec!["lib".to_owned()]));
        rejected.set_current_package_identity(Some("interface_test".to_owned()));
        if let Ok(selected) = products.select(&parsed, &forward, "interface_test", &["lib".to_owned()]) {
            rejected.set_callable_body_reuse(selected);
        }
        assert!(rejected.check_program(&parsed).is_err());
        Ok(())
    }

    /// Locate the first scalar local fixture binding without depending on its documentation's statement position.
    fn first_local_assignment(
        program: &mut Program,
    ) -> Result<&mut crate::ast::AssignmentStmt, Box<dyn std::error::Error>> {
        program
            .declarations
            .iter_mut()
            .find_map(|declaration| {
                let Declaration::Function(function) = &mut declaration.node else {
                    return None;
                };
                function
                    .body
                    .iter_mut()
                    .find_map(|statement| match &mut statement.node {
                        crate::ast::Statement::Assignment(assignment) => Some(assignment),
                        _ => None,
                    })
            })
            .ok_or_else(|| "local fixture has no assignment".into())
    }

    /// Build the same scalar test module through the ordinary Body IR producer, including its complete context.
    fn scalar_module(program: &Program, checker: &TypeChecker) -> incan_semantics_core::body_ir::BodyIrModule {
        let providers = crate::body_ir::ProviderOperationCatalog::new();
        crate::body_ir::build_body_ir_module(
            program,
            &["lib".to_owned()],
            checker.type_info(),
            &crate::body_ir::BodyIrContext {
                modules: &[],
                provider_operations: &providers,
                registry_package: None,
                library_package: Some("interface_test"),
            },
        )
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
