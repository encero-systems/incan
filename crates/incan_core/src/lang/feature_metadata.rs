//! Source-local metadata for the generated feature inventory.
//!
//! Product-level capabilities often span syntax, type checking, generated Rust, and stdlib source. This module keeps
//! the documentation metadata with a source surface that owns the capability while still validating it before the
//! generated reference is written. The temporary registry bridge in [`super::features`] exists only for entries that
//! have not yet moved to a source owner.

use std::collections::BTreeMap;
use std::fmt::{Display, Formatter};
use std::fs;
use std::path::{Path, PathBuf};

use super::features::{FeatureCategory, FeatureDescriptor};
use super::registry::{Since, Stability};

const SOURCE_ROOTS: &[&str] = &["src", "crates"];
const BEGIN_MARKER: &str = "incan-feature: begin";
const END_MARKER: &str = "incan-feature: end";

/// An owned reference link parsed from a source-local feature block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeatureReference {
    /// Human-readable link label.
    pub label: String,
    /// Reference path relative to the generated feature-inventory page.
    pub path: String,
}

/// A validated feature descriptor ready for generated-reference rendering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeatureInventoryEntry {
    /// Stable, source-owned capability identifier.
    pub id: String,
    /// Reader-facing feature name.
    pub name: String,
    /// Broad docs grouping.
    pub category: FeatureCategory,
    /// First release that exposed this capability.
    pub since: Since,
    /// RFC that introduced the capability.
    pub introduced_in_rfc: String,
    /// Lifecycle status presented to users.
    pub stability: Stability,
    /// How users activate or access the feature.
    pub activation: String,
    /// Present-tense capability summary.
    pub summary: String,
    /// Preferred source forms.
    pub canonical_forms: Vec<String>,
    /// Guidance about the less suitable alternative.
    pub prefer_over: String,
    /// Authored reference links.
    pub references: Vec<FeatureReference>,
}

impl From<FeatureDescriptor> for FeatureInventoryEntry {
    fn from(feature: FeatureDescriptor) -> Self {
        Self {
            id: format!("{:?}", feature.id),
            name: feature.name.to_string(),
            category: feature.category,
            since: feature.since,
            introduced_in_rfc: feature.introduced_in_rfc.to_string(),
            stability: feature.stability,
            activation: feature.activation.to_string(),
            summary: feature.summary.to_string(),
            canonical_forms: feature.canonical_forms.iter().map(|form| (*form).to_string()).collect(),
            prefer_over: feature.prefer_over.to_string(),
            references: feature
                .references
                .iter()
                .map(|reference| FeatureReference {
                    label: reference.label.to_string(),
                    path: reference.path.to_string(),
                })
                .collect(),
        }
    }
}
/// Error returned when source-local feature metadata is malformed or cannot be discovered.
#[derive(Debug)]
pub struct FeatureMetadataError {
    message: String,
}

impl FeatureMetadataError {
    /// Construct an error with source-oriented context.
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl Display for FeatureMetadataError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for FeatureMetadataError {}

/// Load the legacy migration bridge and all source-local feature metadata under a workspace root.
///
/// The returned order is stable: legacy entries retain their historical order and source-local entries follow in
/// lexicographic `(id, source path)` order. Duplicate ids fail generation rather than making the generated page depend
/// on filesystem traversal order.
pub fn load_feature_inventory(root: &Path) -> Result<Vec<FeatureInventoryEntry>, FeatureMetadataError> {
    let mut inventory: Vec<FeatureInventoryEntry> = super::features::FEATURES.iter().copied().map(Into::into).collect();
    let source_features = discover_source_features(root)?;
    inventory.extend(source_features);

    let mut ids = BTreeMap::new();
    let mut names = BTreeMap::new();
    for feature in &inventory {
        if let Some(previous) = ids.insert(feature.id.as_str(), feature.name.as_str()) {
            return Err(FeatureMetadataError::new(format!(
                "duplicate feature metadata id {:?}: {:?} and {:?}",
                feature.id, previous, feature.name
            )));
        }
        if let Some(previous) = names.insert(feature.name.as_str(), feature.id.as_str()) {
            return Err(FeatureMetadataError::new(format!(
                "duplicate feature metadata name {:?}: {:?} and {:?}",
                feature.name, previous, feature.id
            )));
        }
    }

    Ok(inventory)
}

/// Discover and validate feature blocks from supported Rust and Incan source roots.
pub fn discover_source_features(root: &Path) -> Result<Vec<FeatureInventoryEntry>, FeatureMetadataError> {
    let mut files = Vec::new();
    for relative_root in SOURCE_ROOTS {
        collect_source_files(&root.join(relative_root), &mut files)?;
    }
    files.sort();

    let mut features = Vec::new();
    for path in files {
        let source = fs::read_to_string(&path)
            .map_err(|error| FeatureMetadataError::new(format!("read {}: {error}", path.display())))?;
        let relative_path = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
        features.extend(parse_feature_blocks(&relative_path, &source)?);
    }
    features.sort_by(|left, right| left.id.cmp(&right.id).then_with(|| left.name.cmp(&right.name)));
    Ok(features)
}

/// Parse every source-local feature block in one source file.
pub fn parse_feature_blocks(path: &Path, source: &str) -> Result<Vec<FeatureInventoryEntry>, FeatureMetadataError> {
    let mut features = Vec::new();
    let mut block: Option<RawFeatureBlock> = None;

    for (index, line) in source.lines().enumerate() {
        let line_number = index + 1;
        let Some(comment) = comment_text(path, line) else {
            if let Some(active) = &block {
                return Err(active.error(line_number, "feature metadata must remain inside source comments"));
            }
            continue;
        };
        let content = comment.trim();

        if content == BEGIN_MARKER {
            if block.is_some() {
                return Err(FeatureMetadataError::new(format!(
                    "{}:{line_number}: nested {BEGIN_MARKER} marker",
                    path.display()
                )));
            }
            block = Some(RawFeatureBlock::new(path, line_number));
            continue;
        }
        if content == END_MARKER {
            let Some(active) = block.take() else {
                return Err(FeatureMetadataError::new(format!(
                    "{}:{line_number}: {END_MARKER} without a matching begin marker",
                    path.display()
                )));
            };
            features.push(active.finish(line_number)?);
            continue;
        }
        if let Some(active) = block.as_mut() {
            active.push(content, line_number)?;
        }
    }

    if let Some(active) = block {
        return Err(active.error(active.start_line, "missing incan-feature: end marker"));
    }
    Ok(features)
}

/// Recursively collect source files that can carry an Incan feature metadata block.
fn collect_source_files(root: &Path, files: &mut Vec<PathBuf>) -> Result<(), FeatureMetadataError> {
    if !root.exists() {
        return Ok(());
    }
    let entries =
        fs::read_dir(root).map_err(|error| FeatureMetadataError::new(format!("read {}: {error}", root.display())))?;
    for entry in entries {
        let entry =
            entry.map_err(|error| FeatureMetadataError::new(format!("read {} entry: {error}", root.display())))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|error| FeatureMetadataError::new(format!("inspect {}: {error}", path.display())))?;
        if file_type.is_dir() {
            collect_source_files(&path, files)?;
        } else if matches!(
            path.extension().and_then(|extension| extension.to_str()),
            Some("incn" | "rs")
        ) {
            files.push(path);
        }
    }
    Ok(())
}

/// Return source-comment contents using the syntax of the scanned source file.
fn comment_text<'a>(path: &Path, line: &'a str) -> Option<&'a str> {
    let trimmed = line.trim_start();
    match path.extension().and_then(|extension| extension.to_str()) {
        Some("incn") => trimmed.strip_prefix('#'),
        Some("rs") => trimmed.strip_prefix("//"),
        _ => None,
    }
}

/// Mutable source block state retained until all required fields are validated.
struct RawFeatureBlock {
    path: PathBuf,
    start_line: usize,
    fields: BTreeMap<String, Vec<(String, usize)>>,
}

impl RawFeatureBlock {
    /// Create a new block at the marker that opened it.
    fn new(path: &Path, start_line: usize) -> Self {
        Self {
            path: path.to_path_buf(),
            start_line,
            fields: BTreeMap::new(),
        }
    }

    /// Record one `key = "value"` metadata line.
    fn push(&mut self, content: &str, line_number: usize) -> Result<(), FeatureMetadataError> {
        if content.is_empty() {
            return Ok(());
        }
        let Some((key, raw_value)) = content.split_once('=') else {
            return Err(self.error(line_number, "metadata must use key = \"value\" syntax"));
        };
        let key = key.trim();
        if key.is_empty() {
            return Err(self.error(line_number, "metadata key cannot be empty"));
        }
        let value = parse_quoted_value(raw_value.trim()).map_err(|message| self.error(line_number, message))?;
        self.fields
            .entry(key.to_string())
            .or_default()
            .push((value, line_number));
        Ok(())
    }

    /// Validate and convert the completed block into a generated-reference entry.
    fn finish(self, end_line: usize) -> Result<FeatureInventoryEntry, FeatureMetadataError> {
        let id = self.required_scalar("id")?;
        validate_id(&id).map_err(|message| self.error(end_line, message))?;
        let name = self.required_scalar("name")?;
        let since_text = self.required_scalar("since")?;
        let since = parse_since(&since_text).map_err(|message| self.error(end_line, message))?;
        let introduced_in_rfc = self.required_scalar("rfc")?;
        validate_rfc(&introduced_in_rfc).map_err(|message| self.error(end_line, message))?;
        let category =
            parse_category(&self.required_scalar("category")?).map_err(|message| self.error(end_line, message))?;
        let stability =
            parse_stability(&self.required_scalar("stability")?).map_err(|message| self.error(end_line, message))?;
        let activation = self.required_scalar("activation")?;
        let summary = self.required_scalar("summary")?;
        let prefer_over = self.required_scalar("prefer_over")?;
        let canonical_forms = self.required_many("canonical")?;
        let references = self
            .required_many("reference")?
            .into_iter()
            .map(|reference| parse_reference(&reference).map_err(|message| self.error(end_line, message)))
            .collect::<Result<Vec<_>, _>>()?;

        for (key, values) in &self.fields {
            if !matches!(
                key.as_str(),
                "id" | "name"
                    | "since"
                    | "rfc"
                    | "category"
                    | "stability"
                    | "activation"
                    | "summary"
                    | "prefer_over"
                    | "canonical"
                    | "reference"
            ) {
                let line_number = values.first().map(|(_, line)| *line).unwrap_or(self.start_line);
                return Err(self.error(line_number, format!("unknown feature metadata field {key:?}")));
            }
        }

        Ok(FeatureInventoryEntry {
            id,
            name,
            category,
            since,
            introduced_in_rfc,
            stability,
            activation,
            summary,
            canonical_forms,
            prefer_over,
            references,
        })
    }

    /// Return one required, non-empty scalar field.
    fn required_scalar(&self, key: &str) -> Result<String, FeatureMetadataError> {
        let values = self
            .fields
            .get(key)
            .ok_or_else(|| self.error(self.start_line, format!("missing required {key:?} metadata")))?;
        if values.len() != 1 {
            return Err(self.error(values[0].1, format!("{key:?} must appear exactly once")));
        }
        if values[0].0.trim().is_empty() {
            return Err(self.error(values[0].1, format!("{key:?} cannot be empty")));
        }
        Ok(values[0].0.clone())
    }

    /// Return one or more required, non-empty repeated fields.
    fn required_many(&self, key: &str) -> Result<Vec<String>, FeatureMetadataError> {
        let values = self
            .fields
            .get(key)
            .ok_or_else(|| self.error(self.start_line, format!("missing required {key:?} metadata")))?;
        if values.iter().any(|(value, _)| value.trim().is_empty()) {
            return Err(self.error(values[0].1, format!("{key:?} cannot be empty")));
        }
        Ok(values.iter().map(|(value, _)| value.clone()).collect())
    }

    /// Attach a source path and line number to a metadata validation error.
    fn error(&self, line_number: usize, message: impl Display) -> FeatureMetadataError {
        FeatureMetadataError::new(format!("{}:{line_number}: {message}", self.path.display()))
    }
}

/// Parse one quoted metadata value while allowing only explicit quote and backslash escapes.
fn parse_quoted_value(value: &str) -> Result<String, &'static str> {
    let Some(value) = value.strip_prefix('"').and_then(|value| value.strip_suffix('"')) else {
        return Err("metadata values must be double-quoted");
    };

    let mut decoded = String::new();
    let mut characters = value.chars();
    while let Some(character) = characters.next() {
        if character == '"' {
            return Err("metadata values cannot contain an unescaped double quote");
        }
        if character != '\\' {
            decoded.push(character);
            continue;
        }
        match characters.next() {
            Some('"') => decoded.push('"'),
            Some('\\') => decoded.push('\\'),
            Some(_) => return Err(r#"metadata values support only \" and \\ escapes"#),
            None => return Err("metadata values cannot end with an escape"),
        }
    }
    Ok(decoded)
}

/// Validate a lower-case dotted feature identifier.
fn validate_id(id: &str) -> Result<(), &'static str> {
    if id.is_empty() || id.starts_with('.') || id.ends_with('.') || id.contains("..") {
        return Err("id must be a non-empty dotted identifier");
    }
    if id
        .chars()
        .all(|character| character.is_ascii_lowercase() || character.is_ascii_digit() || matches!(character, '.' | '-'))
    {
        Ok(())
    } else {
        Err("id may contain only lowercase ASCII letters, digits, dots, and hyphens")
    }
}

/// Parse the `major.minor` source compatibility version.
fn parse_since(value: &str) -> Result<Since, &'static str> {
    let Some((major, minor)) = value.split_once('.') else {
        return Err("since must use major.minor format");
    };
    if minor.contains('.') {
        return Err("since must use major.minor format");
    }
    let major = major
        .parse::<u16>()
        .map_err(|_| "since major must be an unsigned integer")?;
    let minor = minor
        .parse::<u16>()
        .map_err(|_| "since minor must be an unsigned integer")?;
    Ok(Since(major, minor))
}

/// Validate the RFC reference format used by generated language registries.
fn validate_rfc(value: &str) -> Result<(), &'static str> {
    let Some(number) = value.strip_prefix("RFC ") else {
        return Err("rfc must use RFC NNN format");
    };
    if number.len() == 3 && number.chars().all(|character| character.is_ascii_digit()) {
        Ok(())
    } else {
        Err("rfc must use RFC NNN format")
    }
}

/// Parse the public category vocabulary rather than accepting arbitrary generated table text.
fn parse_category(value: &str) -> Result<FeatureCategory, &'static str> {
    match value {
        "syntax" => Ok(FeatureCategory::Syntax),
        "type-system" => Ok(FeatureCategory::TypeSystem),
        "stdlib" => Ok(FeatureCategory::Stdlib),
        "interop" => Ok(FeatureCategory::Interop),
        "testing" => Ok(FeatureCategory::Testing),
        "async" => Ok(FeatureCategory::Async),
        "tooling" => Ok(FeatureCategory::Tooling),
        "libraries" => Ok(FeatureCategory::Libraries),
        _ => Err("category must be one of syntax, type-system, stdlib, interop, testing, async, tooling, or libraries"),
    }
}

/// Parse the stable lifecycle vocabulary used by language registries.
fn parse_stability(value: &str) -> Result<Stability, &'static str> {
    match value {
        "stable" => Ok(Stability::Stable),
        "draft" => Ok(Stability::Draft),
        "deprecated" => Ok(Stability::Deprecated),
        _ => Err("stability must be one of stable, draft, or deprecated"),
    }
}

/// Parse one `label | path` reference field.
fn parse_reference(value: &str) -> Result<FeatureReference, &'static str> {
    let Some((label, path)) = value.split_once(" | ") else {
        return Err("reference must use \"label | path\" format");
    };
    if label.trim().is_empty() || path.trim().is_empty() {
        return Err("reference label and path cannot be empty");
    }
    Ok(FeatureReference {
        label: label.to_string(),
        path: path.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
        process,
        sync::atomic::{AtomicU64, Ordering},
    };

    use super::{FeatureMetadataError, load_feature_inventory, parse_feature_blocks};

    const VALID_BLOCK: &str = r#"
# incan-feature: begin
# id = "std.example"
# name = "`std.example` source-local feature"
# since = "0.5"
# rfc = "RFC 072"
# category = "stdlib"
# stability = "stable"
# activation = "Import from `std.example`."
# summary = "A source-local capability description."
# canonical = "from std.example import example"
# canonical = "example()"
# prefer_over = "A project-local wrapper."
# reference = "std.example | stdlib/example.md"
# incan-feature: end
"#;

    static NEXT_TEST_ROOT: AtomicU64 = AtomicU64::new(0);

    struct TestRoot {
        path: PathBuf,
    }

    impl TestRoot {
        fn new() -> std::io::Result<Self> {
            let sequence = NEXT_TEST_ROOT.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!("incan-feature-metadata-{}-{sequence}", process::id()));
            fs::create_dir_all(&path)?;
            Ok(Self { path })
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TestRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn source_block(id: &str, name: &str) -> String {
        VALID_BLOCK
            .replace("# ", "// ")
            .replace("std.example", id)
            .lines()
            .map(|line| {
                if line.starts_with("// name = ") {
                    format!("// name = \"{name}\"")
                } else {
                    line.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn write_source_block(root: &Path, relative_path: &str, id: &str, name: &str) -> std::io::Result<()> {
        let path = root.join(relative_path);
        let parent = path.parent().expect("source path should have a parent");
        fs::create_dir_all(parent)?;
        fs::write(path, source_block(id, name))
    }

    /// Source-local metadata accepts a complete block and preserves repeated fields.
    #[test]
    fn parses_complete_feature_metadata_block() -> Result<(), FeatureMetadataError> {
        let features = parse_feature_blocks(Path::new("stdlib/example.incn"), VALID_BLOCK)?;
        assert_eq!(features.len(), 1);
        assert_eq!(features[0].id, "std.example");
        assert_eq!(features[0].canonical_forms.len(), 2);
        Ok(())
    }

    /// Rust owners use the same schema with Rust comment syntax.
    #[test]
    fn parses_rust_source_feature_metadata_block() -> Result<(), FeatureMetadataError> {
        let source = VALID_BLOCK.replace("# ", "// ");
        let features = parse_feature_blocks(Path::new("src/example.rs"), &source)?;
        assert_eq!(features.len(), 1);
        assert_eq!(features[0].id, "std.example");
        Ok(())
    }

    /// Required fields fail with source context rather than silently disappearing from generated docs.
    #[test]
    fn rejects_missing_required_feature_metadata() {
        let source = VALID_BLOCK.replacen("# summary = \"A source-local capability description.\"\n", "", 1);
        let error = match parse_feature_blocks(Path::new("stdlib/example.incn"), &source) {
            Ok(_) => panic!("missing summary metadata should fail"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("missing required \"summary\" metadata"));
    }

    /// Duplicate scalar fields fail before a generated reference can choose an arbitrary value.
    #[test]
    fn rejects_duplicate_scalar_feature_metadata() {
        let source = VALID_BLOCK.replacen(
            "# name = \"`std.example` source-local feature\"\n",
            "# name = \"`std.example` source-local feature\"\n# name = \"Duplicate\"\n",
            1,
        );
        let error = match parse_feature_blocks(Path::new("stdlib/example.incn"), &source) {
            Ok(_) => panic!("duplicate name metadata should fail"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("\"name\" must appear exactly once"));
    }

    /// Duplicate ids in distinct source owners fail rather than depending on discovery order.
    #[test]
    fn rejects_duplicate_feature_ids_across_source_files() -> Result<(), Box<dyn std::error::Error>> {
        let root = TestRoot::new()?;
        write_source_block(root.path(), "src/first.rs", "std.duplicate", "First source capability")?;
        write_source_block(
            root.path(),
            "src/second.rs",
            "std.duplicate",
            "Second source capability",
        )?;

        let error = load_feature_inventory(root.path()).expect_err("duplicate source ids should fail");
        assert!(
            error
                .to_string()
                .contains("duplicate feature metadata id \"std.duplicate\"")
        );
        assert!(error.to_string().contains("First source capability"));
        assert!(error.to_string().contains("Second source capability"));
        Ok(())
    }

    /// Duplicate names in distinct source owners fail even when their stable ids differ.
    #[test]
    fn rejects_duplicate_feature_names_across_source_files() -> Result<(), Box<dyn std::error::Error>> {
        let root = TestRoot::new()?;
        write_source_block(root.path(), "src/first.rs", "std.first", "Shared source capability")?;
        write_source_block(root.path(), "src/second.rs", "std.second", "Shared source capability")?;

        let error = load_feature_inventory(root.path()).expect_err("duplicate source names should fail");
        assert!(
            error
                .to_string()
                .contains("duplicate feature metadata name \"Shared source capability\"")
        );
        assert!(error.to_string().contains("std.first"));
        assert!(error.to_string().contains("std.second"));
        Ok(())
    }

    /// The source-local inventory cannot shadow a legacy bridge entry while migration remains incomplete.
    #[test]
    fn rejects_source_feature_name_that_collides_with_legacy_bridge() -> Result<(), Box<dyn std::error::Error>> {
        let root = TestRoot::new()?;
        write_source_block(
            root.path(),
            "src/shadow.rs",
            "std.shadow",
            "Namespaced stdlib imports and decorators",
        )?;

        let error = load_feature_inventory(root.path()).expect_err("legacy bridge name collision should fail");
        assert!(
            error
                .to_string()
                .contains("duplicate feature metadata name \"Namespaced stdlib imports and decorators\"")
        );
        assert!(error.to_string().contains("NamespacedStdlib"));
        assert!(error.to_string().contains("std.shadow"));
        Ok(())
    }

    /// References require an explicit label/path separator so generated links cannot be ambiguous.
    #[test]
    fn rejects_malformed_feature_reference() {
        let source = VALID_BLOCK.replacen("std.example | stdlib/example.md", "stdlib/example.md", 1);
        let error = match parse_feature_blocks(Path::new("stdlib/example.incn"), &source) {
            Ok(_) => panic!("malformed reference metadata should fail"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("reference must use"));
    }
}
