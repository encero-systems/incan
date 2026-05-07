//! Raw transport structs and conversions for `.incnlib` payloads.
//!
//! These types mirror the serialized JSON shape. They stay private to the module so the rest of the compiler deals in
//! validated semantic types instead of transport details.

use serde::{Deserialize, Serialize};

use super::{
    AliasExport, ClassExport, ConstExport, DslSurface, EnumExport, FunctionExport, LibraryContractMetadata,
    LibraryExports, LibraryManifest, LibraryManifestError, LibraryRustAbi, ModelExport, NewtypeExport, PartialExport,
    SoftKeywordActivation, SoftKeywordExports, StaticExport, TraitExport, TypeAliasExport, VocabDesugarerArtifact,
    VocabExports, VocabKeywordRegistration, VocabProviderManifest,
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct RawLibraryManifest {
    pub(super) name: String,
    pub(super) version: String,
    pub(super) incan_version: String,
    pub(super) manifest_format: u32,
    pub(super) exports: RawLibraryExports,
    #[serde(default)]
    pub(super) vocab: Option<RawVocabExports>,
    pub(super) soft_keywords: RawSoftKeywordExports,
    #[serde(default)]
    pub(super) contract_metadata: LibraryContractMetadata,
    #[serde(default)]
    pub(super) rust_abi: Option<LibraryRustAbi>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub(super) struct RawLibraryExports {
    #[serde(default)]
    pub(super) aliases: Vec<AliasExport>,
    #[serde(default)]
    pub(super) partials: Vec<PartialExport>,
    #[serde(default)]
    pub(super) models: Vec<ModelExport>,
    #[serde(default)]
    pub(super) classes: Vec<ClassExport>,
    #[serde(default)]
    pub(super) functions: Vec<FunctionExport>,
    #[serde(default)]
    pub(super) traits: Vec<TraitExport>,
    #[serde(default)]
    pub(super) enums: Vec<EnumExport>,
    #[serde(default)]
    pub(super) type_aliases: Vec<TypeAliasExport>,
    #[serde(default)]
    pub(super) newtypes: Vec<NewtypeExport>,
    #[serde(default)]
    pub(super) consts: Vec<ConstExport>,
    #[serde(default)]
    pub(super) statics: Vec<StaticExport>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub(super) struct RawSoftKeywordExports {
    #[serde(default)]
    pub(super) activations: Vec<SoftKeywordActivation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct RawVocabExports {
    pub(super) crate_path: String,
    pub(super) package_name: String,
    #[serde(default)]
    pub(super) keyword_registrations: Vec<VocabKeywordRegistration>,
    #[serde(default)]
    pub(super) dsl_surfaces: Vec<DslSurface>,
    #[serde(default)]
    pub(super) provider_manifest: VocabProviderManifest,
    #[serde(default)]
    pub(super) desugarer_artifact: Option<VocabDesugarerArtifact>,
}

impl RawLibraryManifest {
    /// Convert the compiler-facing manifest model into the serialized manifest transport shape.
    pub(super) fn from_semantic(semantic: &LibraryManifest) -> Self {
        Self {
            name: semantic.name.clone(),
            version: semantic.version.clone(),
            incan_version: semantic.incan_version.clone(),
            manifest_format: semantic.manifest_format,
            exports: RawLibraryExports {
                aliases: semantic.exports.aliases.clone(),
                partials: semantic.exports.partials.clone(),
                models: semantic.exports.models.clone(),
                classes: semantic.exports.classes.clone(),
                functions: semantic.exports.functions.clone(),
                traits: semantic.exports.traits.clone(),
                enums: semantic.exports.enums.clone(),
                type_aliases: semantic.exports.type_aliases.clone(),
                newtypes: semantic.exports.newtypes.clone(),
                consts: semantic.exports.consts.clone(),
                statics: semantic.exports.statics.clone(),
            },
            vocab: semantic.vocab.as_ref().map(|vocab| RawVocabExports {
                crate_path: vocab.crate_path.clone(),
                package_name: vocab.package_name.clone(),
                keyword_registrations: vocab.keyword_registrations.clone(),
                dsl_surfaces: vocab.dsl_surfaces.clone(),
                provider_manifest: vocab.provider_manifest.clone(),
                desugarer_artifact: vocab.desugarer_artifact.clone(),
            }),
            soft_keywords: RawSoftKeywordExports {
                activations: semantic.soft_keywords.activations.clone(),
            },
            contract_metadata: semantic.contract_metadata.clone(),
            rust_abi: semantic.rust_abi.clone(),
        }
    }

    /// Decode a validated raw manifest into the compiler-facing manifest model.
    pub(super) fn into_semantic(self) -> Result<LibraryManifest, LibraryManifestError> {
        Ok(LibraryManifest {
            name: self.name,
            version: self.version,
            incan_version: self.incan_version,
            manifest_format: self.manifest_format,
            exports: LibraryExports {
                aliases: self.exports.aliases,
                partials: self.exports.partials,
                models: self.exports.models,
                classes: self.exports.classes,
                functions: self.exports.functions,
                traits: self.exports.traits,
                enums: self.exports.enums,
                type_aliases: self.exports.type_aliases,
                newtypes: self.exports.newtypes,
                consts: self.exports.consts,
                statics: self.exports.statics,
            },
            vocab: self.vocab.map(|vocab| VocabExports {
                crate_path: vocab.crate_path,
                package_name: vocab.package_name,
                keyword_registrations: vocab.keyword_registrations,
                dsl_surfaces: vocab.dsl_surfaces,
                provider_manifest: vocab.provider_manifest,
                desugarer_artifact: vocab.desugarer_artifact,
            }),
            soft_keywords: SoftKeywordExports {
                activations: self.soft_keywords.activations,
            },
            contract_metadata: self.contract_metadata,
            rust_abi: self.rust_abi,
        })
    }
}
