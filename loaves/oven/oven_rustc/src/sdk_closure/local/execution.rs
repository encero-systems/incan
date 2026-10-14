//! Local graph projection and assembly around the shared bounded native executor.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use super::{
    CompileContext, Error, LocalFacetSelection, SdkCompiledClosure, SdkCompiledUnit, compile_local_with_owners,
    compiler_closure_digest, local_dependency_demands, rustc_host_target, rustc_identity,
};
use oven_store::store::OvenStore;

/// One completed local operation retains its original owner until ordered graph assembly.
pub(super) struct LocalCompilation {
    /// A new owner, or none when the exact owner already belongs to the incoming closure.
    pub(super) unit: Option<Box<SdkCompiledUnit>>,
    /// Whether the canonical unit compiler reused its existing native artifact.
    pub(super) reused: bool,
    /// Stable source/domain/profile report label.
    pub(super) label: String,
    /// Unit preparation elapsed time, excluding scheduler waits.
    pub(super) seconds: f64,
}

/// Project declared local waits once and preserve the previous dependency-ordered reporting sequence.
fn schedule(
    selections: &[LocalFacetSelection],
    owner: &Path,
    target: &str,
) -> Result<(Vec<usize>, Vec<Vec<usize>>), Error> {
    let mut names = BTreeMap::new();
    let mut declarations = Vec::with_capacity(selections.len());
    for (index, selection) in selections.iter().enumerate() {
        if selection.target.as_deref().is_some_and(|selected| selected != target)
            || (!selection.target_predicates.is_empty() && selection.target.is_none())
        {
            return Err("local predicate selection does not bind the requested native target".into());
        }
        let declaration: toml::Value = toml::from_str(&std::fs::read_to_string(
            owner.join(&selection.project).join("loaf.toml"),
        )?)?;
        let name = declaration
            .get("project")
            .and_then(|project| project.get("name"))
            .and_then(toml::Value::as_str)
            .ok_or("selected local unit has no Loaf project name")?;
        if names
            .insert((name.to_string(), selection.domain.clone()), index)
            .is_some()
        {
            return Err(format!("duplicate selected local facet {name}").into());
        }
        declarations.push(declaration);
    }
    let dependencies = selections
        .iter()
        .zip(&declarations)
        .map(|(selection, declaration)| {
            Ok(
                local_dependency_demands(declaration, &selection.features, &selection.target_predicates)?
                    .iter()
                    .filter_map(|demand| {
                        names
                            .get(&(demand.loaf.clone(), selection.domain.clone()))
                            .or_else(|| names.get(&(demand.loaf.clone(), "host".to_string())))
                            .copied()
                    })
                    .collect::<Vec<_>>(),
            )
        })
        .collect::<Result<Vec<_>, Error>>()?;
    let mut pending: BTreeSet<_> = (0..selections.len()).collect();
    let mut order = Vec::with_capacity(selections.len());
    while !pending.is_empty() {
        let before = pending.len();
        for index in pending.clone() {
            if dependencies[index]
                .iter()
                .any(|dependency| pending.contains(dependency))
            {
                continue;
            }
            pending.remove(&index);
            order.push(index);
        }
        if pending.len() == before {
            return Err("cycle in selected local Loaf facets".into());
        }
    }
    Ok((order, dependencies))
}

/// Borrow original registry and completed local owners while the one executor runs dependency-ready local units.
#[allow(clippy::too_many_arguments)]
pub(super) fn compile_local_graph(
    closure: &mut SdkCompiledClosure,
    selections: &[LocalFacetSelection],
    owner: &Path,
    output: &Path,
    rustc: &Path,
    profile: &str,
    store: &OvenStore,
    jobs: usize,
) -> Result<(), Error> {
    if !matches!(profile, "debug" | "release") {
        return Err("local native profile must be debug or release".into());
    }
    if selections.is_empty() {
        return Ok(());
    }
    let started = std::time::Instant::now();
    let previous_seconds = closure.report.seconds;
    let target = rustc_host_target(rustc)?;
    let (order, dependencies) = schedule(selections, owner, &target)?;
    let toolchain = rustc_identity(rustc)?;
    let context = CompileContext {
        rustc,
        target: &target,
        toolchain: &toolchain,
        output,
        store,
        compiler_digest: compiler_closure_digest(rustc, &target)?,
        compiler_executable: oven_store::store::digest_regular_file(&rustc.canonicalize()?)?.1,
        profile,
        unit_codegen: &[],
    };
    let jobs = jobs.max(1).min(selections.len());
    let _trace = tracing::trace_span!(target: "incan::invocation", "oven_rustc::compile_local_dependencies", units = selections.len(), jobs).entered();
    let results = super::super::compile::execute_ready(&order, &dependencies, jobs, |index, outcomes| {
        if dependencies[index]
            .iter()
            .any(|dependency| !matches!(outcomes[*dependency].get(), Some(Ok(_))))
        {
            return Err("local dependency was refused or failed".into());
        }
        // Only preceding graph positions are eligible, independently of thread completion order. The original
        // closure and completion slots retain all owners until scoped execution has joined every worker.
        let mut available = closure.units.iter().collect::<Vec<_>>();
        for previous in order.iter().take_while(|previous| **previous != index) {
            if let Some(Ok(LocalCompilation { unit: Some(unit), .. })) = outcomes[*previous].get() {
                available.push(unit.as_ref());
            }
        }
        let selection = &selections[index];
        compile_local_with_owners(
            &available,
            &owner.join(&selection.project),
            &selection.features,
            &selection.domain,
            &selection.target_predicates,
            &context,
        )
    })?;
    let mut results = results;
    for index in order {
        match results[index].take().ok_or("local native unit has no completion")? {
            Ok(result) => append_compilation(closure, result)?,
            Err(error) => closure
                .report
                .failed
                .push(format!("local facet {}: {error}", selections[index].project.display())),
        }
    }
    closure.report.seconds = previous_seconds + started.elapsed().as_secs_f64();
    Ok(())
}

/// Append a completed owner once and bind inspection indices to its exact already retained destinations.
pub(super) fn append_compilation(closure: &mut SdkCompiledClosure, result: LocalCompilation) -> Result<(), Error> {
    if let Some(mut unit) = result.unit {
        let dependencies = unit
            .physical_edges
            .iter()
            .map(|edge| {
                let index = closure
                    .units
                    .iter()
                    .position(|dependency| dependency.entry_identity() == edge.destination().store_identity)
                    .ok_or("local physical dependency has no inspection index")?;
                Ok(serde_json::json!({"crate": index, "name": edge.alias()}))
            })
            .collect::<Result<Vec<_>, Error>>()?;
        *unit
            .inspection
            .get_mut("deps")
            .ok_or("local unit has no inspection dependencies")? = serde_json::Value::Array(dependencies);
        closure.units.push(*unit);
    }
    if result.reused {
        closure.report.reused.push(result.label);
    } else {
        closure.report.compiled.push(result.label);
    }
    closure.report.seconds += result.seconds;
    Ok(())
}

#[cfg(test)]
mod tests;
