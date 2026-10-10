//! Portable dependency identities from the native producer's exact authored source mapping.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use super::{
    Error, digest_local_facet_sources, local_dependency_demands, local_facet_sources, native_required_features,
};

/// Digest an authored native Loaf and its active path dependencies without reading Cargo declarations.
///
/// Source enumeration is shared with publication, including compiler-owned embedded inputs. Path coordinates are
/// replaced by recursive source identities in canonical declaration facts, so relocation preserves the identity.
/// Inactive optional edges retain their declaration without accessing unavailable source trees. Features and target
/// forms obey the native producer's existing admission rules; this does not resolve a registry or acquire outputs.
pub fn local_native_dependency_source_digest(
    project: &Path,
    features: &[String],
    default_features: bool,
) -> Result<String, Error> {
    source_digest(
        project,
        features,
        default_features,
        &mut BTreeSet::new(),
        &mut BTreeMap::new(),
    )
}

/// Memoize one feature-qualified source closure during this observation, refusing cycles before reading children.
fn source_digest(
    project: &Path,
    features: &[String],
    default_features: bool,
    visiting: &mut BTreeSet<PathBuf>,
    resolved: &mut BTreeMap<(PathBuf, Vec<String>), String>,
) -> Result<String, Error> {
    if !std::fs::symlink_metadata(project)?.is_dir() {
        return Err("authored native dependency root is not a plain directory".into());
    }
    let project = project.canonicalize()?;
    let mut sources = local_facet_sources(&project)?;
    let features = native_required_features(&sources.manifest, features, default_features)?
        .into_iter()
        .collect::<Vec<_>>();
    let key = (project.clone(), features.clone());
    if let Some(digest) = resolved.get(&key) {
        return Ok(digest.clone());
    }
    if !visiting.insert(project.clone()) {
        return Err("authored native path dependency source graph contains a cycle".into());
    }
    let mut demands = BTreeMap::new();
    for demand in local_dependency_demands(&sources.manifest, &features)? {
        if demands.insert(demand.alias.clone(), demand).is_some() {
            return Err("authored native dependencies repeat a normalized Rust alias".into());
        }
    }
    let mut paths = BTreeMap::new();
    if let Some(dependencies) = sources.manifest.get("dependencies").and_then(toml::Value::as_table) {
        for (alias, declaration) in dependencies {
            let Some(path) = declaration.get("path") else {
                continue;
            };
            let path = path.as_str().ok_or("native dependency path must be a string")?;
            let defaults = match declaration.get("default-features") {
                None => true,
                Some(value) => value
                    .as_bool()
                    .ok_or("native dependency default-feature policy must be a boolean")?,
            };
            let normalized_alias = alias.replace('-', "_");
            let digest = if let Some(demand) = demands.get(&normalized_alias) {
                let child = project.join(path);
                let child_features = demand.required.iter().cloned().collect::<Vec<_>>();
                source_digest(&child, &child_features, defaults, visiting, resolved)?
            } else {
                "inactive-optional-native-source".to_string()
            };
            paths.insert(alias.clone(), digest);
        }
    }
    let mut declaration: toml::Value = toml::from_str(&sources.declaration)?;
    normalize_paths(&mut declaration, &paths)?;
    normalize_paths(&mut sources.manifest, &paths)?;
    sources.declaration = oven_model::digest::canonical_toml_string(&declaration)?;
    let digest = digest_local_facet_sources(&sources, |path| Ok(oven_store::store::digest_regular_file(path)?.1))?;
    visiting.remove(&project);
    resolved.insert(key, digest.clone());
    Ok(digest)
}

/// Replace only authored dependency coordinates with the source facts computed from the producer's active edges.
fn normalize_paths(manifest: &mut toml::Value, paths: &BTreeMap<String, String>) -> Result<(), Error> {
    let Some(dependencies) = manifest.get_mut("dependencies").and_then(toml::Value::as_table_mut) else {
        return Ok(());
    };
    for (alias, digest) in paths {
        let dependency = dependencies
            .get_mut(alias)
            .and_then(toml::Value::as_table_mut)
            .ok_or("native dependency declaration changed during source mapping")?;
        dependency.insert("path".to_string(), toml::Value::String(digest.clone()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::local_native_dependency_source_digest;
    use std::path::Path;

    /// Author the same native input contract at any fixture coordinate, without a Cargo declaration.
    fn write_leaf(root: &Path, name: &str) -> Result<(), Box<dyn std::error::Error>> {
        std::fs::create_dir_all(root.join("src"))?;
        std::fs::write(
            root.join("loaf.toml"),
            format!("[project]\nname='{name}'\nversion='1.0.0'\n[rust]\nname='{name}'\ntype='lib'\nedition='2024'\n"),
        )?;
        std::fs::write(root.join("src/lib.rs"), "pub fn ready() -> bool { false }\n")?;
        Ok(())
    }

    /// Absolute path relocation preserves identities, while nested source and declaration changes invalidate them.
    #[test]
    fn authored_native_dependency_digest_relocates_and_tracks_active_sources() -> Result<(), Box<dyn std::error::Error>>
    {
        let fixture = tempfile::tempdir()?;
        let mut digests = Vec::new();
        for location in ["one", "two"] {
            let root = fixture.path().join(location);
            let leaf = root.join("leaf");
            let consumer = root.join("consumer");
            write_leaf(&leaf, "leaf")?;
            write_leaf(&consumer, "consumer")?;
            let mut declaration = std::fs::read_to_string(consumer.join("loaf.toml"))?;
            declaration.push_str(&format!(
                "[dependencies]\nleaf={{loaf='leaf',path='{}'}}\n",
                leaf.display()
            ));
            std::fs::write(consumer.join("loaf.toml"), declaration)?;
            digests.push(local_native_dependency_source_digest(&consumer, &[], true)?);
            std::fs::create_dir_all(consumer.join("target/debug"))?;
            std::fs::write(consumer.join("target/debug/output"), "mutable output")?;
            std::fs::write(consumer.join("Cargo.toml"), "poisoned compatibility input")?;
            assert_eq!(
                digests.last(),
                Some(&local_native_dependency_source_digest(&consumer, &[], true)?)
            );
        }
        assert_eq!(digests[0], digests[1]);
        let consumer = fixture.path().join("two/consumer");
        let source = fixture.path().join("two/leaf/src/lib.rs");
        std::fs::write(&source, "pub fn ready() -> bool { true }\n")?;
        let changed = local_native_dependency_source_digest(&consumer, &[], true)?;
        assert_ne!(digests[1], changed);
        std::fs::write(&source, "pub fn ready() -> bool { false }\n")?;
        assert_eq!(digests[1], local_native_dependency_source_digest(&consumer, &[], true)?);
        std::fs::write(
            fixture.path().join("two/leaf/src/embedded.txt"),
            "embedded source input",
        )?;
        assert_ne!(digests[1], local_native_dependency_source_digest(&consumer, &[], true)?);
        Ok(())
    }

    /// Publisher-owned embedded files participate even when they lie outside the native Loaf's source directory.
    #[test]
    fn authored_native_dependency_digest_covers_embedded_inputs() -> Result<(), Box<dyn std::error::Error>> {
        let fixture = tempfile::tempdir()?;
        let project = fixture.path().join("loaves/kernel/incan_lang");
        write_leaf(&project, "incan_lang")?;
        for component in [
            "async",
            "codecs",
            "compression",
            "core",
            "data",
            "interop",
            "observability",
            "system",
            "testing",
            "web",
        ] {
            let root = fixture.path().join("loaves/stdlib").join(component);
            std::fs::create_dir_all(&root)?;
            std::fs::write(root.join("loaf.toml"), "embedded declaration")?;
        }
        let before = local_native_dependency_source_digest(&project, &[], true)?;
        std::fs::write(
            fixture.path().join("loaves/stdlib/system/loaf.toml"),
            "changed embedded declaration",
        )?;
        assert_ne!(before, local_native_dependency_source_digest(&project, &[], true)?);
        std::fs::remove_file(fixture.path().join("loaves/stdlib/system/loaf.toml"))?;
        assert!(local_native_dependency_source_digest(&project, &[], true).is_err());
        Ok(())
    }

    /// Optional paths are observed only when the producer's feature fixed point activates their native edge.
    #[test]
    fn authored_native_dependency_digest_respects_optional_feature_activation() -> Result<(), Box<dyn std::error::Error>>
    {
        let fixture = tempfile::tempdir()?;
        let project = fixture.path().join("consumer");
        write_leaf(&project, "consumer")?;
        let mut declaration = std::fs::read_to_string(project.join("loaf.toml"))?;
        declaration.push_str(
            "[project.features]\nextra=['dep:leaf']\n[dependencies]\nleaf={loaf='leaf',path='../leaf',optional=true}\n",
        );
        std::fs::write(project.join("loaf.toml"), declaration)?;
        let inactive = local_native_dependency_source_digest(&project, &[], true)?;
        assert!(local_native_dependency_source_digest(&project, &["extra".into()], true).is_err());
        write_leaf(&fixture.path().join("leaf"), "leaf")?;
        assert_eq!(inactive, local_native_dependency_source_digest(&project, &[], true)?);
        let active = local_native_dependency_source_digest(&project, &["extra".into()], true)?;
        assert_ne!(inactive, active);
        std::fs::write(
            fixture.path().join("leaf/src/lib.rs"),
            "pub fn ready() -> bool { true }\n",
        )?;
        assert_eq!(inactive, local_native_dependency_source_digest(&project, &[], true)?);
        assert_ne!(
            active,
            local_native_dependency_source_digest(&project, &["extra".into()], true)?
        );
        Ok(())
    }

    /// Cycles, non-plain inputs and unresolved target forms cannot yield an apparently complete source identity.
    #[test]
    fn authored_native_dependency_digest_refuses_cycles_links_and_unknown_targets()
    -> Result<(), Box<dyn std::error::Error>> {
        let fixture = tempfile::tempdir()?;
        let project = fixture.path().join("consumer");
        write_leaf(&project, "consumer")?;
        let original = std::fs::read_to_string(project.join("loaf.toml"))?;
        std::fs::write(
            project.join("loaf.toml"),
            format!("{original}[dependencies]\nself_ref={{loaf='consumer',path='.'}}\n"),
        )?;
        assert!(local_native_dependency_source_digest(&project, &[], true).is_err());
        std::fs::write(
            project.join("loaf.toml"),
            format!("{original}[dependencies]\nconditional=[{{loaf='leaf',path='../leaf',target='cfg(unix)'}}]\n"),
        )?;
        assert!(local_native_dependency_source_digest(&project, &[], true).is_err());
        std::fs::write(project.join("loaf.toml"), &original)?;
        write_leaf(&fixture.path().join("leaf"), "leaf")?;
        std::fs::write(
            project.join("loaf.toml"),
            format!("{original}[dependencies]\nleaf={{loaf='leaf',path='../leaf',default-features='false'}}\n"),
        )?;
        assert!(
            local_native_dependency_source_digest(&project, &[], true)
                .err()
                .ok_or("malformed default-feature policy was accepted")?
                .to_string()
                .contains("must be a boolean")
        );
        std::fs::write(
            project.join("loaf.toml"),
            format!(
                "{original}[dependencies]\nleaf-a={{loaf='leaf',path='../leaf'}}\nleaf_a={{loaf='leaf',path='../leaf'}}\n"
            ),
        )?;
        assert!(
            local_native_dependency_source_digest(&project, &[], true)
                .err()
                .ok_or("duplicate normalized alias was accepted")?
                .to_string()
                .contains("normalized Rust alias")
        );
        std::fs::write(project.join("loaf.toml"), &original)?;
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(project.join("src/lib.rs"), project.join("src/linked.rs"))?;
            assert!(local_native_dependency_source_digest(&project, &[], true).is_err());
            std::fs::remove_file(project.join("src/linked.rs"))?;
            let linked = fixture.path().join("linked-root");
            std::os::unix::fs::symlink(&project, &linked)?;
            assert!(local_native_dependency_source_digest(&linked, &[], true).is_err());
        }
        Ok(())
    }
}
