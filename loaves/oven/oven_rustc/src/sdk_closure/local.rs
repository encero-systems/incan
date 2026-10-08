//! Local SDK Rust facets compiled against the retained seed, without Cargo metadata.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use super::{
    CompileContext, Error, PreparedUnit, SdkCompiledClosure, SdkCompiledUnit, SdkLockedUnit, compile_unit,
    compiler_closure_digest, inspection_unit,
};
use crate::rustc::{rustc_host_target, rustc_identity};
use oven_store::store::{OvenStore, OvenStoreLimits};

/// One already selected local compile unit; feature resolution belongs to the authored graph's resolver.
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalFacetSelection {
    /// Loaf directory, relative to the selection document's owner.
    pub project: std::path::PathBuf,
    /// Complete selected local features, including expanded feature aliases.
    pub features: Vec<String>,
    /// Compilation domain, either host or target.
    pub domain: String,
}

/// Compile a selected local graph in dependency order, preserving successful branches and named failures.
///
/// The graph is explicit input. This operational boundary reads only Loaf declarations and sources, never Cargo
/// manifests or locks, and never changes registry selections already admitted by the enclosing closure.
pub fn compile_local_sdk_facets(
    closure: &mut SdkCompiledClosure,
    selections: &[LocalFacetSelection],
    owner: &Path,
    output: &Path,
    rustc: &Path,
) -> Result<(), Error> {
    let mut pending: BTreeSet<_> = (0..selections.len()).collect();
    let mut names = BTreeMap::new();
    for (index, selection) in selections.iter().enumerate() {
        let declaration: toml::Value = toml::from_str(&std::fs::read_to_string(
            owner.join(&selection.project).join("loaf.toml"),
        )?)?;
        let name = declaration
            .get("rust")
            .and_then(|rust| rust.get("name"))
            .and_then(toml::Value::as_str)
            .ok_or("selected local unit has no Rust name")?;
        if names
            .insert((name.to_string(), selection.domain.clone()), index)
            .is_some()
        {
            return Err(format!("duplicate selected local facet {name}").into());
        }
    }
    while !pending.is_empty() {
        let mut progressed = false;
        for index in pending.clone() {
            let selection = &selections[index];
            let project = owner.join(&selection.project);
            let declaration: toml::Value = toml::from_str(&std::fs::read_to_string(project.join("loaf.toml"))?)?;
            let (_, active) = local_feature_selection(&declaration, &selection.features)?;
            let dependencies = declaration.get("dependencies").and_then(toml::Value::as_table);
            let waits = dependencies
                .into_iter()
                .flatten()
                .filter(|(alias, _)| active.contains_key(*alias))
                .any(|(_, dependency)| {
                    dependency
                        .get("loaf")
                        .and_then(toml::Value::as_str)
                        .is_some_and(|loaf| {
                            names
                                .get(&(loaf.to_string(), selection.domain.clone()))
                                .or_else(|| names.get(&(loaf.to_string(), "host".to_string())))
                                .is_some_and(|dependency| pending.contains(dependency))
                        })
                });
            if waits {
                continue;
            }
            pending.remove(&index);
            progressed = true;
            if let Err(error) =
                compile_local_sdk_facet(closure, &project, &selection.features, &selection.domain, output, rustc)
            {
                closure
                    .report
                    .failed
                    .push(format!("local facet {}: {error}", selection.project.display()));
            }
        }
        if !progressed {
            return Err("cycle in selected local Loaf facets".into());
        }
    }
    Ok(())
}

/// Recompute a local facet's portable source snapshot identity for source-current consumer admission.
pub fn local_sdk_facet_source_digest(project: &Path, output: &Path) -> Result<String, Error> {
    std::fs::create_dir_all(output)?;
    let snapshot = tempfile::Builder::new().prefix("sdk-local-check-").tempdir_in(output)?;
    Ok(prepare_local_unit(project, snapshot.path(), &[], "target")?
        .binding
        .archive_digest)
}

/// Compile one local Loaf's Rust facet and append its exact source/alias graph to the retained SDK closure.
///
/// Dependencies must already be selected in the closure. No resolution, build script, or Cargo reader is invoked.
/// The private source snapshot includes only the Loaf declaration and Rust source directory. Every source byte and
/// selected dependency output participates in the receipt; the returned closure retains the published store lease.
pub fn compile_local_sdk_facet(
    closure: &mut SdkCompiledClosure,
    project: &Path,
    features: &[String],
    domain: &str,
    output: &Path,
    rustc: &Path,
) -> Result<(), Error> {
    let target = rustc_host_target(rustc)?;
    compile_local_sdk_facet_for_target(closure, project, features, domain, output, rustc, &target)
}

/// Compile one local Loaf's Rust facet for an explicit target triple into a closure compiled for that same target.
///
/// A cross-target closure, such as the wasm32-wasip1 inputs of a vocabulary desugarer, needs its local facets built
/// for its own target; the receipt binds the triple, so a host build and a cross build never share an identity.
pub fn compile_local_sdk_facet_for_target(
    closure: &mut SdkCompiledClosure,
    project: &Path,
    features: &[String],
    domain: &str,
    output: &Path,
    rustc: &Path,
    target: &str,
) -> Result<(), Error> {
    if !matches!(domain, "host" | "target") {
        return Err("local SDK facet domain must be host or target".into());
    }
    let started = std::time::Instant::now();
    std::fs::create_dir_all(output)?;
    let snapshot = tempfile::Builder::new().prefix("sdk-local-").tempdir_in(output)?;
    let mut unit = prepare_local_unit(project, snapshot.path(), features, domain)?;
    if let Some(selected) = closure
        .units
        .iter()
        .find(|selected| selected.binding.loaf == unit.binding.loaf && selected.binding.domain == domain)
    {
        if selected.binding.version != unit.binding.version
            || selected.binding.archive_digest != unit.binding.archive_digest
            || selected.binding.features != unit.binding.features
            || selected.owner.manifest.intent.target != target
            || selected.owner.manifest.intent.toolchain != rustc_identity(rustc)?
        {
            return Err(format!(
                "local facet {} conflicts with an already selected binding",
                unit.binding.loaf
            )
            .into());
        }
        selected.owner.verify_admitted_payload()?;
        closure
            .report
            .reused
            .push(format!("{} {} {domain} debug", unit.binding.loaf, unit.binding.version));
        return Ok(());
    }
    let (source, lease) =
        super::environment::stable_sources(&unit.root, &unit.binding, &unit.about, unit.primary, None)?;
    unit.root = source;
    unit._source_lease = Some(lease);
    let edges = selected_local_edges(&unit, closure)?;
    let externs = edges
        .iter()
        .map(|(alias, index)| (alias.clone(), closure.units[*index].output.clone()))
        .collect();
    let searches = closure
        .units
        .iter()
        .filter_map(|unit| unit.output.parent().map(Path::to_path_buf))
        .collect();
    let toolchain = rustc_identity(rustc)?;
    let store = OvenStore::new(
        output.join("store"),
        OvenStoreLimits::new(4 * 1024 * 1024 * 1024, 4 * 1024 * 1024 * 1024, 4 * 1024 * 1024 * 1024),
    );
    let context = CompileContext {
        rustc,
        target,
        toolchain: &toolchain,
        output,
        store: &store,
        compiler_digest: compiler_closure_digest(rustc, target)?,
        profile: "debug",
    };
    let (path, reused, owner) = compile_unit(&unit, &context, externs, searches)?;
    let dependencies = edges
        .iter()
        .map(|(name, index)| serde_json::json!({"crate": index, "name": name}))
        .collect();
    let inspection = inspection_unit(&unit, &owner.artifact_root.join("source"), dependencies)?;
    let label = format!("{} {} {domain} debug", unit.binding.loaf, unit.binding.version);
    closure.units.push(SdkCompiledUnit {
        binding: unit.binding,
        output: path,
        owner,
        inspection,
    });
    if reused {
        closure.report.reused.push(label);
    } else {
        closure.report.compiled.push(label);
    }
    closure.report.seconds += started.elapsed().as_secs_f64();
    Ok(())
}

/// Snapshot a local Rust facet under the adopted executor's file-root convention.
fn prepare_local_unit(
    project: &Path,
    snapshot: &Path,
    features: &[String],
    domain: &str,
) -> Result<PreparedUnit, Error> {
    let declaration = std::fs::read_to_string(project.join("loaf.toml"))?;
    let mut manifest: toml::Value = toml::from_str(&declaration)?;
    let project_section = manifest.get("project").ok_or("local Loaf has no project section")?;
    let version = project_section
        .get("version")
        .and_then(toml::Value::as_str)
        .ok_or("local Loaf has no project version")?
        .to_string();
    let facet = manifest
        .get_mut("rust")
        .and_then(toml::Value::as_table_mut)
        .ok_or("local Loaf has no Rust facet")?;
    let name = facet
        .get("name")
        .and_then(toml::Value::as_str)
        .ok_or("local Loaf has no Rust crate name")?
        .to_string();
    if facet.get("build").is_some() || facet.get("build-script").and_then(toml::Value::as_bool) == Some(true) {
        return Err("local SDK facets cannot declare a build script".into());
    }
    let source_root = facet
        .get("source")
        .and_then(|value| value.get("root"))
        .and_then(toml::Value::as_str)
        .unwrap_or("");
    let mut relative = Path::new(source_root).join("src");
    if relative
        .components()
        .any(|part| !matches!(part, std::path::Component::Normal(_) | std::path::Component::CurDir))
    {
        return Err("local SDK Rust source must stay inside its Loaf".into());
    }
    let project = project.canonicalize()?;
    let source = project.join(&relative).canonicalize()?;
    if !source.starts_with(&project) {
        return Err("local SDK Rust source escapes its Loaf".into());
    }
    if name == "incan_lang" {
        snapshot_language_catalog(&project, snapshot)?;
        relative = Path::new("loaves/kernel/incan_lang").join(relative);
    }
    if name == "incan_emit" {
        snapshot_emitter_text(&project, snapshot)?;
        relative = Path::new("loaves/compiler/incan_emit").join(relative);
    }
    copy_local_sources(&source, &snapshot.join(&relative))?;
    facet.insert(
        "source".to_string(),
        toml::Value::Table(toml::map::Map::from_iter([(
            "root".to_string(),
            toml::Value::String(relative.join("lib.rs").to_string_lossy().into_owned()),
        )])),
    );
    std::fs::write(snapshot.join(".oven-authored-loaf.toml"), declaration)?;
    std::fs::write(snapshot.join("loaf.toml"), toml::to_string(&manifest)?)?;
    let (features, _) = local_feature_selection(&manifest, features)?;
    Ok(PreparedUnit {
        about: serde_json::Value::Null,
        primary: true,
        _source_lease: None,
        binding: SdkLockedUnit {
            loaf: name,
            version,
            archive_digest: oven_store::digest_source_tree(snapshot)?,
            domain: domain.to_string(),
            features: features.into_iter().collect(),
            target_predicates: Vec::new(),
            edges: None,
        },
        manifest,
        root: snapshot.to_path_buf(),
        build_script: false,
        fact: None,
        fact_out: Vec::new(),
    })
}

/// Preserve the emitter's embedded standard-library text at its authored include geometry.
fn snapshot_emitter_text(project: &Path, snapshot: &Path) -> Result<(), Error> {
    let loaves = project
        .parent()
        .and_then(Path::parent)
        .ok_or("emitter has no owning loaves directory")?;
    let source = loaves.join("stdlib/zen.txt");
    if !std::fs::symlink_metadata(&source)?.is_file() {
        return Err("embedded emitter text is not a plain file".into());
    }
    let destination = snapshot.join("loaves/stdlib/zen.txt");
    std::fs::create_dir_all(destination.parent().ok_or("embedded emitter text has no parent")?)?;
    std::fs::copy(source, destination)?;
    Ok(())
}

/// Retain the language registry's embedded Loaf declarations with their authored relative include geometry.
///
/// `incan_lang/src/lang/stdlib.rs` embeds these ten files. They are explicit compile inputs, and preserving their
/// layout keeps both rustc and inspection inside the immutable snapshot without rewriting source text.
fn snapshot_language_catalog(project: &Path, snapshot: &Path) -> Result<(), Error> {
    let loaves = project
        .parent()
        .and_then(Path::parent)
        .ok_or("language Loaf has no owning loaves directory")?;
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
        let source = loaves.join("stdlib").join(component).join("loaf.toml");
        if !std::fs::symlink_metadata(&source)?.is_file() {
            return Err("embedded stdlib declaration is not a plain file".into());
        }
        let destination = snapshot.join("loaves/stdlib").join(component).join("loaf.toml");
        std::fs::create_dir_all(destination.parent().ok_or("embedded declaration has no parent")?)?;
        std::fs::copy(source, destination)?;
    }
    Ok(())
}

/// Copy only Rust sources, refusing links and special files before hashing or publication.
fn copy_local_sources(source: &Path, destination: &Path) -> Result<(), Error> {
    if !std::fs::symlink_metadata(source)?.is_dir() {
        return Err("local Rust source root is not a plain directory".into());
    }
    std::fs::create_dir_all(destination)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let path = entry.path();
        let destination = destination.join(entry.file_name());
        if kind.is_dir() {
            copy_local_sources(&path, &destination)?;
        } else if kind.is_file() {
            if matches!(
                entry.file_name().to_str(),
                Some("Cargo.toml" | "Cargo.lock" | "build.rs")
            ) {
                return Err("local SDK source tree contains forbidden Cargo metadata or a build script".into());
            }
            std::fs::copy(path, destination)?;
        } else {
            return Err("local SDK source tree contains a link or special file".into());
        }
    }
    Ok(())
}

/// Select declared aliases from the frozen closure, rejecting missing, ambiguous, or feature-incomplete bindings.
fn selected_local_edges(unit: &PreparedUnit, closure: &SdkCompiledClosure) -> Result<Vec<(String, usize)>, Error> {
    let mut edges = Vec::new();
    let Some(dependencies) = unit.manifest.get("dependencies").and_then(toml::Value::as_table) else {
        return Ok(edges);
    };
    let (_, activated) = local_feature_selection(&unit.manifest, &unit.binding.features)?;
    for (alias, declaration) in dependencies {
        if declaration.get("optional").and_then(toml::Value::as_bool) == Some(true) && !activated.contains_key(alias) {
            continue;
        }
        if declaration.is_array() || declaration.get("target").is_some() {
            return Err(format!("local dependency {alias} needs resolved target predicates").into());
        }
        let loaf = declaration
            .get("loaf")
            .and_then(toml::Value::as_str)
            .ok_or("local dependency must name its Loaf")?;
        let version = declaration
            .get("version")
            .and_then(toml::Value::as_str)
            .map(semver::VersionReq::parse)
            .transpose()?;
        let mut required = declaration
            .get("features")
            .and_then(toml::Value::as_array)
            .into_iter()
            .flatten()
            .map(|value| value.as_str().ok_or("dependency feature must be a string"))
            .collect::<Result<BTreeSet<_>, _>>()?;
        if let Some(features) = activated.get(alias) {
            required.extend(features.iter().map(String::as_str));
        }
        let candidates = closure
            .units
            .iter()
            .enumerate()
            .filter(|(_, selected)| {
                let domain = if selected
                    .inspection
                    .get("is_proc_macro")
                    .and_then(serde_json::Value::as_bool)
                    == Some(true)
                {
                    "host"
                } else {
                    unit.binding.domain.as_str()
                };
                selected.binding.loaf == loaf && selected.binding.domain == domain
            })
            .filter(|(_, selected)| {
                version.as_ref().is_none_or(|requirement| {
                    semver::Version::parse(&selected.binding.version).is_ok_and(|version| requirement.matches(&version))
                }) && required
                    .iter()
                    .all(|feature| selected.binding.features.iter().any(|selected| selected == feature))
            })
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        match candidates.as_slice() {
            [index] => edges.push((alias.replace('-', "_"), *index)),
            [] => {
                return Err(format!(
                    "local SDK dependency {alias} ({loaf}) has no selected {} binding with its required features",
                    unit.binding.domain
                )
                .into());
            }
            _ => return Err(format!("local SDK dependency {alias} ({loaf}) has ambiguous selected bindings").into()),
        }
    }
    Ok(edges)
}

/// Expanded local features paired with active dependency aliases and their forwarded feature requests.
type LocalFeatureSelection = (BTreeSet<String>, BTreeMap<String, BTreeSet<String>>);

/// Expand local aliases while retaining per-dependency feature requests.
fn local_feature_selection(manifest: &toml::Value, requested: &[String]) -> Result<LocalFeatureSelection, Error> {
    let definitions = manifest
        .get("project")
        .and_then(|project| project.get("features"))
        .and_then(toml::Value::as_table);
    let dependencies = manifest.get("dependencies").and_then(toml::Value::as_table);
    let mut enabled: BTreeSet<String> = requested.iter().cloned().collect();
    let mut activated: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    if let Some(dependencies) = dependencies {
        for (alias, declaration) in dependencies {
            if declaration.get("optional").and_then(toml::Value::as_bool) != Some(true) {
                activated.insert(alias.clone(), BTreeSet::new());
            }
        }
    }
    loop {
        let before = (enabled.clone(), activated.clone());
        for feature in &before.0 {
            if dependencies.is_some_and(|dependencies| dependencies.contains_key(feature)) {
                activated.entry(feature.clone()).or_default();
            }
            if let Some(values) = definitions.and_then(|definitions| definitions.get(feature)) {
                let values = values
                    .as_array()
                    .ok_or("local Rust facet requires compact feature declarations")?;
                for value in values {
                    let value = value.as_str().ok_or("feature member must be a string")?;
                    if let Some(alias) = value.strip_prefix("dep:") {
                        activated.entry(alias.to_string()).or_default();
                    } else if let Some((alias, child)) = value.split_once('/') {
                        if let Some(alias) = alias.strip_suffix('?') {
                            if let Some(features) = activated.get_mut(alias) {
                                features.insert(child.to_string());
                            }
                        } else {
                            activated
                                .entry(alias.to_string())
                                .or_default()
                                .insert(child.to_string());
                        }
                    } else {
                        enabled.insert(value.to_string());
                    }
                }
            }
        }
        if before == (enabled.clone(), activated.clone()) {
            break;
        }
    }
    Ok((enabled, activated))
}

#[cfg(test)]
mod tests {
    use super::{
        LocalFacetSelection, compile_local_sdk_facet, compile_local_sdk_facets, local_feature_selection,
        prepare_local_unit,
    };
    use crate::sdk_closure::{SdkClosureReport, SdkCompiledClosure};
    use std::collections::BTreeMap;

    /// Strong optional activation and weak forwarding share a fixed point, while inactive optional inputs stay absent.
    #[test]
    fn local_features_preserve_optional_and_weak_requests() -> Result<(), Box<dyn std::error::Error>> {
        let manifest = toml::from_str(
            "[project.features]\ndefault=['bridge']\nbridge=['optional?/extra','dep:optional','plain/child']\n[dependencies]\noptional={loaf='crates-io/optional',optional=true}\nplain={loaf='crates-io/plain'}\nunused={loaf='crates-io/unused',optional=true}\n",
        )?;
        let (enabled, active) = local_feature_selection(&manifest, &["default".to_string()])?;
        assert!(enabled.contains("bridge"));
        assert!(
            active
                .get("optional")
                .is_some_and(|features| features.contains("extra"))
        );
        assert!(active.get("plain").is_some_and(|features| features.contains("child")));
        assert!(!active.contains_key("unused"));
        let (_, inactive) = local_feature_selection(&manifest, &[])?;
        assert!(!inactive.contains_key("optional"));
        Ok(())
    }

    /// The native executor orders a selected local graph and refuses Cargo metadata as an execution input.
    #[test]
    fn local_graph_compiles_dependencies_before_consumers() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let store = tempfile::tempdir()?;
        for name in ["consumer", "leaf"] {
            let project = root.path().join(name);
            std::fs::create_dir_all(project.join("src"))?;
            std::fs::write(
                project.join("loaf.toml"),
                format!(
                    "[project]\nname='{name}'\nversion='1.0.0'\n[rust]\nname='{name}'\ntype='lib'\nedition='2024'\n{}",
                    if name == "consumer" {
                        "[dependencies]\nleaf={loaf='leaf',path='../leaf'}\nabsent={loaf='absent',optional=true,path='../absent'}\n"
                    } else {
                        ""
                    }
                ),
            )?;
            std::fs::write(
                project.join("src/lib.rs"),
                if name == "consumer" {
                    "pub fn value() -> u8 { leaf::value() }"
                } else {
                    "pub fn value() -> u8 { 42 }"
                },
            )?;
            std::fs::write(project.join("Cargo.toml"), "poisoned Cargo input")?;
        }
        let selections: Vec<_> = ["consumer", "leaf"]
            .into_iter()
            .map(|name| LocalFacetSelection {
                project: name.into(),
                features: Vec::new(),
                domain: "target".to_string(),
            })
            .collect();
        let mut closure = SdkCompiledClosure {
            report: SdkClosureReport::default(),
            units: Vec::new(),
            auxiliary_targets: BTreeMap::new(),
        };
        let rustc = crate::rustc::resolve_active_rustc()?;
        compile_local_sdk_facets(&mut closure, &selections, root.path(), store.path(), &rustc)?;
        assert!(closure.report.failed.is_empty(), "{:?}", closure.report.failed);
        assert_eq!(closure.units.len(), 2);
        assert_eq!(closure.units[0].binding.loaf, "leaf");
        assert_eq!(closure.units[1].inspection["deps"][0]["name"].as_str(), Some("leaf"));
        Ok(())
    }

    /// Separate materialization and store roots must produce the same adopted identity and exact rlib bytes.
    #[test]
    fn local_unit_is_reproducible_across_roots() -> Result<(), Box<dyn std::error::Error>> {
        let rustc = crate::rustc::resolve_active_rustc()?;
        let mut results = Vec::new();
        for _ in 0..2 {
            let root = tempfile::tempdir()?;
            let project = root.path().join("project");
            std::fs::create_dir_all(project.join("src"))?;
            std::fs::write(
                project.join("loaf.toml"),
                "[project]\nname='portable'\nversion='1.0.0'\n[rust]\nname='portable'\ntype='lib'\nedition='2024'\n",
            )?;
            std::fs::write(
                project.join("src/lib.rs"),
                "pub fn location() -> &'static str { file!() } pub fn package_dir() -> &'static str { env!(\"CARGO_MANIFEST_DIR\") } pub fn value() -> u8 { 42 }",
            )?;
            let mut closure = SdkCompiledClosure {
                report: SdkClosureReport::default(),
                units: Vec::new(),
                auxiliary_targets: std::collections::BTreeMap::new(),
            };
            compile_local_sdk_facet(
                &mut closure,
                &project,
                &[],
                "target",
                &root.path().join("store"),
                &rustc,
            )?;
            let unit = &closure.units[0];
            results.push((
                unit.owner.manifest.build_unit_identity.clone(),
                std::fs::read(&unit.output)?,
            ));
        }
        assert_eq!(results[0].0, results[1].0);
        assert_eq!(results[0].1, results[1].1);
        Ok(())
    }

    /// Real direct compilation reuses a receipt, while changing a nested Rust module invalidates that receipt.
    #[test]
    fn local_receipts_reuse_and_track_nested_sources() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let store = tempfile::tempdir()?;
        std::fs::create_dir(project.path().join("src"))?;
        std::fs::write(
            project.path().join("loaf.toml"),
            "[project]\nname='local'\nversion='1.0.0'\n[rust]\nname='local'\ntype='lib'\nedition='2024'\n",
        )?;
        std::fs::write(project.path().join("src/lib.rs"), "mod nested; pub use nested::value;")?;
        std::fs::write(project.path().join("src/nested.rs"), "pub fn value() -> u8 { 1 }")?;
        std::fs::write(project.path().join("Cargo.toml"), "invalid TOML")?;
        std::fs::write(project.path().join("build.rs"), "compile_error!(\"must not run\");")?;
        let rustc = crate::rustc::resolve_active_rustc()?;
        let mut first = SdkCompiledClosure {
            report: SdkClosureReport::default(),
            units: Vec::new(),
            auxiliary_targets: std::collections::BTreeMap::new(),
        };
        compile_local_sdk_facet(&mut first, project.path(), &[], "target", store.path(), &rustc)?;
        assert_eq!(first.report.compiled.len(), 1);
        let mut second = SdkCompiledClosure {
            report: SdkClosureReport::default(),
            units: Vec::new(),
            auxiliary_targets: std::collections::BTreeMap::new(),
        };
        compile_local_sdk_facet(&mut second, project.path(), &[], "target", store.path(), &rustc)?;
        assert_eq!(second.report.reused.len(), 1);
        assert_eq!(first.units[0].compiled_identity(), second.units[0].compiled_identity());
        std::fs::write(project.path().join("src/nested.rs"), "pub fn value() -> u8 { 2 }")?;
        let mut third = SdkCompiledClosure {
            report: SdkClosureReport::default(),
            units: Vec::new(),
            auxiliary_targets: std::collections::BTreeMap::new(),
        };
        compile_local_sdk_facet(&mut third, project.path(), &[], "target", store.path(), &rustc)?;
        assert_eq!(third.report.compiled.len(), 1);
        assert_ne!(first.units[0].compiled_identity(), third.units[0].compiled_identity());
        let root = third.units[0].source_root();
        assert_eq!(
            std::fs::read_to_string(root.join("src/nested.rs"))?,
            "pub fn value() -> u8 { 2 }"
        );
        assert!(!root.join("Cargo.toml").exists());
        assert!(!root.join("build.rs").exists());
        Ok(())
    }

    /// Local source identity changes with module bytes and ignores neighboring Cargo metadata completely.
    #[test]
    fn local_snapshot_identity_covers_source_and_avoids_cargo() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        std::fs::create_dir(project.path().join("src"))?;
        std::fs::write(
            project.path().join("loaf.toml"),
            "[project]\nname='local'\nversion='1.0.0'\n[rust]\nname='local'\ntype='lib'\nedition='2024'\n",
        )?;
        std::fs::write(project.path().join("src/lib.rs"), "pub fn value() -> u8 { 1 }")?;
        std::fs::write(project.path().join("Cargo.toml"), "invalid TOML")?;
        std::fs::write(project.path().join("build.rs"), "compile_error!(\"must not run\");")?;
        let first = tempfile::tempdir()?;
        let initial = prepare_local_unit(project.path(), first.path(), &[], "target")?;
        assert!(!first.path().join("Cargo.toml").exists());
        std::fs::write(project.path().join("src/lib.rs"), "pub fn value() -> u8 { 2 }")?;
        let second = tempfile::tempdir()?;
        let changed = prepare_local_unit(project.path(), second.path(), &[], "target")?;
        assert_ne!(initial.binding.archive_digest, changed.binding.archive_digest);
        Ok(())
    }
}
