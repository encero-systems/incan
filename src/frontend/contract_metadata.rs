//! RFC 048 canonical model bundle metadata.
//!
//! This module owns the compiler-facing schema for contract-backed models and the deterministic projection from a
//! checked bundle into ordinary Incan `model` source.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::format::format_source;
use crate::frontend::ast::{Declaration, Program};
use crate::frontend::{lexer, parser};

/// Stable schema version for RFC 048 canonical model bundles.
pub const CONTRACT_MODEL_BUNDLE_SCHEMA_VERSION: u32 = 1;
/// Stable schema version for the `.incnlib` RFC 048 metadata envelope.
pub const CONTRACT_METADATA_SCHEMA_VERSION: u32 = 1;

/// RFC 048 metadata carried by a package or artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContractMetadataPackage {
    /// Metadata envelope schema version.
    pub schema_version: u32,
    /// Canonical model bundles available from this package or artifact.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub model_bundles: Vec<CanonicalModelBundle>,
}

impl Default for ContractMetadataPackage {
    fn default() -> Self {
        Self {
            schema_version: CONTRACT_METADATA_SCHEMA_VERSION,
            model_bundles: Vec::new(),
        }
    }
}

impl ContractMetadataPackage {
    /// Create a contract metadata package for canonical model bundles.
    pub fn new(model_bundles: Vec<CanonicalModelBundle>) -> Self {
        Self {
            schema_version: CONTRACT_METADATA_SCHEMA_VERSION,
            model_bundles,
        }
    }

    /// Validate the package envelope and every contained bundle.
    pub fn validate(&self) -> Result<(), ContractMetadataError> {
        if self.schema_version != CONTRACT_METADATA_SCHEMA_VERSION {
            return Err(ContractMetadataError::Invalid {
                bundle: "metadata package".to_string(),
                message: format!(
                    "unsupported schema_version {} (expected {})",
                    self.schema_version, CONTRACT_METADATA_SCHEMA_VERSION
                ),
            });
        }
        validate_bundle_set(&self.model_bundles)
    }
}

/// Canonical structural description of one model-shaped contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CanonicalModelBundle {
    /// Bundle schema version.
    pub schema_version: u32,
    /// Stable artifact-facing model identifier.
    pub stable_model_id: Option<String>,
    /// Incan nominal type spelling.
    pub logical_type_name: String,
    /// Ordered field list.
    pub fields: Vec<CanonicalModelField>,
    /// Whether the bundle is publishable artifact metadata.
    #[serde(default = "default_publishable")]
    pub publishable: bool,
    /// Optional provenance or producer metadata that does not affect type identity.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub provenance: BTreeMap<String, String>,
}

/// One canonical model field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CanonicalModelField {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
    #[serde(default)]
    pub nullable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, String>,
}

/// Return the default publishable contract metadata for a package.
fn default_publishable() -> bool {
    true
}

/// Errors surfaced while reading, validating, or projecting RFC 048 model bundles.
#[derive(Debug, Error)]
pub enum ContractMetadataError {
    #[error("failed to read {path}: {source}")]
    Read { path: PathBuf, source: std::io::Error },
    #[error("failed to parse {path}: {message}")]
    Parse { path: PathBuf, message: String },
    #[error("invalid checked model bundle `{bundle}`: {message}")]
    Invalid { bundle: String, message: String },
    #[error("failed to project checked model bundle `{bundle}` to Incan source: {message}")]
    Emit { bundle: String, message: String },
}

impl CanonicalModelBundle {
    /// Validate the model bundle before it is used for typechecking, emit, or artifact publication.
    pub fn validate(&self) -> Result<(), ContractMetadataError> {
        let bundle_name = self.bundle_name();
        if self.schema_version != CONTRACT_MODEL_BUNDLE_SCHEMA_VERSION {
            return Err(ContractMetadataError::Invalid {
                bundle: bundle_name,
                message: format!(
                    "unsupported schema_version {} (expected {})",
                    self.schema_version, CONTRACT_MODEL_BUNDLE_SCHEMA_VERSION
                ),
            });
        }
        if self.logical_type_name.trim().is_empty() {
            return Err(ContractMetadataError::Invalid {
                bundle: bundle_name,
                message: "logical_type_name cannot be empty".to_string(),
            });
        }
        validate_identifier(&self.logical_type_name, "logical_type_name", &bundle_name)?;
        if self.publishable || self.stable_model_id.is_some() {
            match self.stable_model_id.as_deref().map(str::trim) {
                Some(value) if !value.is_empty() => {}
                _ => {
                    return Err(ContractMetadataError::Invalid {
                        bundle: bundle_name,
                        message: if self.publishable {
                            "publishable bundles require stable_model_id".to_string()
                        } else {
                            "stable_model_id cannot be empty when present".to_string()
                        },
                    });
                }
            }
        }
        if self.fields.is_empty() {
            return Err(ContractMetadataError::Invalid {
                bundle: bundle_name,
                message: "fields cannot be empty".to_string(),
            });
        }

        let mut seen_fields = HashSet::new();
        let mut seen_aliases = HashSet::new();
        for field in &self.fields {
            validate_identifier(&field.name, "field name", &bundle_name)?;
            if !seen_fields.insert(field.name.as_str()) {
                return Err(ContractMetadataError::Invalid {
                    bundle: bundle_name,
                    message: format!("duplicate field `{}`", field.name),
                });
            }
            validate_type_spelling(&field.ty, &bundle_name, &field.name)?;
            if let Some(alias) = field.alias.as_deref() {
                if alias.trim().is_empty() {
                    return Err(ContractMetadataError::Invalid {
                        bundle: bundle_name,
                        message: format!("field `{}` alias cannot be empty", field.name),
                    });
                }
                if !seen_aliases.insert(alias) {
                    return Err(ContractMetadataError::Invalid {
                        bundle: bundle_name,
                        message: format!("duplicate field alias `{alias}`"),
                    });
                }
            }
            if let Some(description) = field.description.as_deref()
                && description.trim().is_empty()
            {
                return Err(ContractMetadataError::Invalid {
                    bundle: bundle_name,
                    message: format!("field `{}` description cannot be empty", field.name),
                });
            }
            for key in field.metadata.keys() {
                if key.trim().is_empty() {
                    return Err(ContractMetadataError::Invalid {
                        bundle: bundle_name,
                        message: format!("field `{}` metadata keys cannot be empty", field.name),
                    });
                }
            }
        }

        Ok(())
    }

    /// Deterministically project this canonical bundle to formatted Incan `model` source.
    pub fn emit_incan_model_source(&self) -> Result<String, ContractMetadataError> {
        self.validate()?;
        let mut source = String::new();
        source.push_str("pub model ");
        source.push_str(&self.logical_type_name);
        source.push_str(":\n");
        for field in &self.fields {
            source.push_str("    ");
            source.push_str(&field.name);
            let metadata = field_metadata_source(field);
            if !metadata.is_empty() {
                source.push(' ');
                source.push_str(&metadata);
            }
            source.push_str(": ");
            source.push_str(&field_type_source(field));
            source.push('\n');
        }
        format_source(&source).map_err(|error| ContractMetadataError::Emit {
            bundle: self.bundle_name(),
            message: error.to_string(),
        })
    }

    /// Return the package bundle name used in contract metadata.
    fn bundle_name(&self) -> String {
        if self.logical_type_name.trim().is_empty() {
            "<unnamed>".to_string()
        } else {
            self.logical_type_name.clone()
        }
    }
}

/// Validate an identifier used in contract metadata.
fn validate_identifier(value: &str, label: &str, bundle: &str) -> Result<(), ContractMetadataError> {
    let mut chars = value.chars();
    let valid_start = chars.next().is_some_and(|ch| ch == '_' || ch.is_ascii_alphabetic());
    let valid_rest = chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric());
    if valid_start && valid_rest {
        Ok(())
    } else {
        Err(ContractMetadataError::Invalid {
            bundle: bundle.to_string(),
            message: format!("{label} `{value}` must be an Incan identifier"),
        })
    }
}

/// Validate a type spelling used in contract metadata.
fn validate_type_spelling(ty: &str, bundle: &str, field: &str) -> Result<(), ContractMetadataError> {
    let trimmed = ty.trim();
    if trimmed.is_empty() {
        return Err(ContractMetadataError::Invalid {
            bundle: bundle.to_string(),
            message: format!("field `{field}` type cannot be empty"),
        });
    }
    if matches!(trimmed, "_" | "Unknown" | "unknown" | "Opaque" | "opaque") {
        return Err(ContractMetadataError::Invalid {
            bundle: bundle.to_string(),
            message: format!("field `{field}` type `{trimmed}` is not a complete Incan type"),
        });
    }
    Ok(())
}

/// Return the source text used for exported field metadata.
fn field_metadata_source(field: &CanonicalModelField) -> String {
    let mut pairs = Vec::new();
    if let Some(alias) = field.alias.as_deref() {
        pairs.push(format!("alias=\"{}\"", escape_incan_string(alias)));
    }
    if let Some(description) = field.description.as_deref() {
        pairs.push(format!("description=\"{}\"", escape_incan_string(description)));
    }
    if pairs.is_empty() {
        String::new()
    } else {
        format!("[{}]", pairs.join(", "))
    }
}

/// Return the source text used for exported field type metadata.
fn field_type_source(field: &CanonicalModelField) -> String {
    let ty = field.ty.trim();
    if field.nullable && !ty.starts_with("Option[") {
        format!("Option[{ty}]")
    } else {
        ty.to_string()
    }
}

/// Escape text for use in an Incan string literal.
fn escape_incan_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Read one canonical model bundle or metadata package from a JSON file.
pub fn read_model_bundles_from_json(path: &Path) -> Result<Vec<CanonicalModelBundle>, ContractMetadataError> {
    let content = fs::read_to_string(path).map_err(|source| ContractMetadataError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    if let Ok(package) = serde_json::from_str::<ContractMetadataPackage>(&content) {
        package.validate()?;
        return Ok(package.model_bundles);
    }
    let bundle: CanonicalModelBundle =
        serde_json::from_str(&content).map_err(|error| ContractMetadataError::Parse {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;
    bundle.validate()?;
    Ok(vec![bundle])
}

/// Read project-declared model bundles from manifest-relative paths.
pub fn read_project_model_bundles(
    project_root: &Path,
    bundle_paths: &[String],
) -> Result<Vec<CanonicalModelBundle>, ContractMetadataError> {
    let mut bundles = Vec::new();
    for configured_path in bundle_paths {
        let path = if Path::new(configured_path).is_absolute() {
            PathBuf::from(configured_path)
        } else {
            project_root.join(configured_path)
        };
        bundles.extend(read_model_bundles_from_json(&path)?);
    }
    validate_bundle_set(&bundles)?;
    Ok(bundles)
}

/// Validate that one set of bundles can be materialized into a single compilation scope.
pub fn validate_bundle_set(bundles: &[CanonicalModelBundle]) -> Result<(), ContractMetadataError> {
    let mut names = HashSet::new();
    let mut ids = HashSet::new();
    for bundle in bundles {
        bundle.validate()?;
        if !names.insert(bundle.logical_type_name.as_str()) {
            return Err(ContractMetadataError::Invalid {
                bundle: bundle.logical_type_name.clone(),
                message: "duplicate logical_type_name in bundle set".to_string(),
            });
        }
        if let Some(stable_id) = bundle.stable_model_id.as_deref()
            && !ids.insert(stable_id)
        {
            return Err(ContractMetadataError::Invalid {
                bundle: bundle.logical_type_name.clone(),
                message: format!("duplicate stable_model_id `{stable_id}` in bundle set"),
            });
        }
    }
    Ok(())
}

/// Prepend materialized model declarations to a parsed program.
pub fn materialize_contract_models(
    program: &mut Program,
    bundles: &[CanonicalModelBundle],
) -> Result<(), ContractMetadataError> {
    if bundles.is_empty() {
        return Ok(());
    }
    validate_bundle_set(bundles)?;
    validate_no_source_collisions(program, bundles)?;

    let mut synthetic_source = String::new();
    for bundle in bundles {
        synthetic_source.push_str(&bundle.emit_incan_model_source()?);
        synthetic_source.push('\n');
    }
    let tokens = lexer::lex(&synthetic_source).map_err(|errors| ContractMetadataError::Emit {
        bundle: "bundle set".to_string(),
        message: format!("{errors:?}"),
    })?;
    let mut synthetic_program = parser::parse(&tokens).map_err(|errors| ContractMetadataError::Emit {
        bundle: "bundle set".to_string(),
        message: format!("{errors:?}"),
    })?;
    synthetic_program.declarations.append(&mut program.declarations);
    synthetic_program.rust_module_path = program.rust_module_path.take();
    synthetic_program.warnings.append(&mut program.warnings);
    *program = synthetic_program;
    Ok(())
}

/// Validate that generated contract metadata has no source-name collisions.
fn validate_no_source_collisions(
    program: &Program,
    bundles: &[CanonicalModelBundle],
) -> Result<(), ContractMetadataError> {
    let names: HashSet<&str> = bundles.iter().map(|bundle| bundle.logical_type_name.as_str()).collect();
    for declaration in &program.declarations {
        let name = match &declaration.node {
            Declaration::Model(model) => Some(model.name.as_str()),
            Declaration::Class(class) => Some(class.name.as_str()),
            Declaration::Trait(trait_decl) => Some(trait_decl.name.as_str()),
            Declaration::Enum(enum_decl) => Some(enum_decl.name.as_str()),
            Declaration::Newtype(newtype) => Some(newtype.name.as_str()),
            Declaration::TypeAlias(alias) => Some(alias.name.as_str()),
            _ => None,
        };
        if let Some(name) = name
            && names.contains(name)
        {
            return Err(ContractMetadataError::Invalid {
                bundle: name.to_string(),
                message: "logical_type_name collides with a source declaration".to_string(),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::ast::{Declaration, Visibility};

    fn order_bundle() -> CanonicalModelBundle {
        CanonicalModelBundle {
            schema_version: CONTRACT_MODEL_BUNDLE_SCHEMA_VERSION,
            stable_model_id: Some("orders.summary".to_string()),
            logical_type_name: "OrderSummary".to_string(),
            publishable: true,
            provenance: BTreeMap::new(),
            fields: vec![
                CanonicalModelField {
                    name: "order_id".to_string(),
                    ty: "str".to_string(),
                    nullable: false,
                    alias: Some("orderId".to_string()),
                    description: Some("Stable order identifier".to_string()),
                    metadata: BTreeMap::new(),
                },
                CanonicalModelField {
                    name: "coupon_code".to_string(),
                    ty: "str".to_string(),
                    nullable: true,
                    alias: None,
                    description: None,
                    metadata: BTreeMap::new(),
                },
            ],
        }
    }

    #[test]
    fn emits_formatted_model_source_from_bundle() -> Result<(), Box<dyn std::error::Error>> {
        let source = order_bundle().emit_incan_model_source()?;
        assert!(source.contains("pub model OrderSummary:"));
        assert!(source.contains("order_id [alias=\"orderId\", description=\"Stable order identifier\"]: str"));
        assert!(source.contains("coupon_code: Option[str]"));
        Ok(())
    }

    #[test]
    fn materializes_contract_model_declaration_before_source() -> Result<(), Box<dyn std::error::Error>> {
        let tokens = lexer::lex("def read(order: OrderSummary) -> str:\n    return order.order_id\n")
            .map_err(|errors| format!("lex failed: {errors:?}"))?;
        let mut program = parser::parse(&tokens).map_err(|errors| format!("parse failed: {errors:?}"))?;
        materialize_contract_models(&mut program, &[order_bundle()])?;
        let Some(first) = program.declarations.first() else {
            return Err("expected materialized declaration".into());
        };
        match &first.node {
            Declaration::Model(model) => {
                assert_eq!(model.visibility, Visibility::Public);
                assert_eq!(model.name, "OrderSummary");
            }
            other => return Err(format!("expected model declaration, got {other:?}").into()),
        }
        Ok(())
    }

    #[test]
    fn rejects_source_name_collision() -> Result<(), Box<dyn std::error::Error>> {
        let tokens =
            lexer::lex("model OrderSummary:\n    id: str\n").map_err(|errors| format!("lex failed: {errors:?}"))?;
        let mut program = parser::parse(&tokens).map_err(|errors| format!("parse failed: {errors:?}"))?;
        let error = materialize_contract_models(&mut program, &[order_bundle()]);
        assert!(error.is_err(), "expected source declaration collision");
        Ok(())
    }

    #[test]
    fn rejects_publishable_bundle_without_stable_id() {
        let mut bundle = order_bundle();
        bundle.stable_model_id = None;
        let error = bundle.validate();
        assert!(error.is_err(), "expected missing stable_model_id error");
    }

    #[test]
    fn rejects_metadata_package_schema_version_mismatch() {
        let package = ContractMetadataPackage {
            schema_version: CONTRACT_METADATA_SCHEMA_VERSION + 1,
            model_bundles: vec![order_bundle()],
        };
        let error = package.validate();
        assert!(error.is_err(), "expected metadata package schema version error");
    }
}
