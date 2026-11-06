//! Bounded execution of the existing resolved native graph, retaining original source and Store leases.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{OnceLock, mpsc};

use super::{
    CompileContext, Error, PreparedUnit, SdkClosureReport, SdkCompiledClosure, SdkCompiledUnit, active_edges,
    binding_label, compile_unit, inspection_unit, physical_edges, selected_native_bindings,
};

/// A command's completion slots hold the original owners until the complete closure is returned.
enum Outcome {
    /// A declared build script has no admitted fact; independent branches may still proceed.
    Refused,
    /// Compilation, publication or a required dependency failed.
    Failed(String),
    /// The exact published or reused native owner, together with its work classification.
    Compiled { unit: Box<SdkCompiledUnit>, reused: bool },
}

/// Project resolved edges once, preserving the previous serial traversal's stable inspection order.
struct Schedule {
    edges: Vec<Vec<(String, usize)>>,
    order: Vec<usize>,
    positions: Vec<usize>,
    closures: Vec<BTreeSet<usize>>,
}

impl Schedule {
    /// Validate cycles before spawning workers and retain exact aliases, never resolving dependencies again.
    fn new(units: &[PreparedUnit]) -> Result<Self, Error> {
        let edges = units
            .iter()
            .map(|unit| {
                if unit.build_script && unit.fact.is_none() {
                    Ok(Vec::new())
                } else {
                    active_edges(unit, units)
                }
            })
            .collect::<Result<Vec<_>, Error>>()?;
        let mut pending: BTreeSet<usize> = (0..units.len()).collect();
        let mut order = Vec::with_capacity(units.len());
        let mut positions = vec![0; units.len()];
        let mut closures = vec![BTreeSet::new(); units.len()];
        while !pending.is_empty() {
            let before = pending.len();
            for index in pending.clone() {
                if edges[index].iter().any(|(_, dependency)| pending.contains(dependency)) {
                    continue;
                }
                let mut closure = BTreeSet::new();
                for (_, dependency) in &edges[index] {
                    closure.insert(*dependency);
                    closure.extend(&closures[*dependency]);
                }
                closures[index] = closure;
                positions[index] = order.len();
                order.push(index);
                pending.remove(&index);
            }
            if pending.len() == before {
                return Err("cycle in locked SDK closure".into());
            }
        }
        Ok(Self {
            edges,
            order,
            positions,
            closures,
        })
    }
}

/// Compile independent dependencies with a conservative process bound; unchanged owner requests skip this path.
pub(super) fn compile_units(units: &[PreparedUnit], context: &CompileContext<'_>) -> Result<SdkCompiledClosure, Error> {
    compile_units_with_jobs(units, context, worker_bound())
}

/// Use one conservative compiler process ceiling for both registry and local native graphs.
pub(super) fn worker_bound() -> usize {
    std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(1)
        .min(4)
}

/// Refill a free worker as soon as its dependencies finish, without a barrier across unrelated branches.
///
/// This replaces the serial executor rather than adding a parallel cache or resolver. Workers borrow the exact
/// prepared sources and original dependency owners. Completion order never selects a dependency or changes the
/// inspection/report order, and all workers finish before an error releases their borrowed source leases.
fn compile_units_with_jobs(
    units: &[PreparedUnit],
    context: &CompileContext<'_>,
    jobs: usize,
) -> Result<SdkCompiledClosure, Error> {
    let schedule = Schedule::new(units)?;
    let dependencies = schedule
        .edges
        .iter()
        .map(|edges| edges.iter().map(|(_, index)| *index).collect())
        .collect::<Vec<Vec<usize>>>();
    let jobs = jobs.max(1).min(units.len().max(1));
    let _trace = tracing::trace_span!(target: "incan::invocation", "oven_rustc::compile_native_dependencies", units = units.len(), jobs).entered();
    let results = execute_ready(&schedule.order, &dependencies, jobs, |index, outcomes| {
        if units[index].build_script && units[index].fact.is_none() {
            return Ok(Outcome::Refused);
        }
        let (unit, reused) = compile_selected(index, units, context, &schedule, outcomes)?;
        Ok(Outcome::Compiled {
            unit: Box::new(unit),
            reused,
        })
    })?;
    let outcomes = results
        .into_iter()
        .map(|slot| {
            let outcome = match slot.into_inner() {
                Some(Ok(outcome)) => outcome,
                Some(Err(error)) => Outcome::Failed(error),
                None => Outcome::Failed("native unit has no completion".to_string()),
            };
            OnceLock::from(outcome)
        })
        .collect();
    finish_units(units, context, &schedule, outcomes)
}

/// Execute one already ordered graph with a bounded worker set and original borrowed input owners.
///
/// Registry and local producers supply their existing selection/compilation functions. This coordinator owns only
/// readiness and scoped lifetime; it never resolves edges, reconstructs owners, or admits cached results. Failed
/// dependencies are complete slots so the producer can report their refusal without blocking independent branches.
pub(super) fn execute_ready<T: Send + Sync>(
    order: &[usize],
    dependencies: &[Vec<usize>],
    jobs: usize,
    execute: impl Fn(usize, &[OnceLock<Result<T, String>>]) -> Result<T, Error> + Sync,
) -> Result<Vec<OnceLock<Result<T, String>>>, Error> {
    let outcomes: Vec<OnceLock<Result<T, String>>> = (0..dependencies.len()).map(|_| OnceLock::new()).collect();
    let jobs = jobs.max(1);
    std::thread::scope(|scope| -> Result<(), Error> {
        let (sender, receiver) = mpsc::channel();
        let mut workers = BTreeMap::new();
        let mut completed = 0;
        while completed < dependencies.len() {
            for &index in order {
                if outcomes[index].get().is_some() || workers.contains_key(&index) {
                    continue;
                }
                if dependencies[index]
                    .iter()
                    .any(|dependency| outcomes[*dependency].get().is_none())
                {
                    continue;
                }
                if workers.len() == jobs {
                    break;
                }
                let sender = sender.clone();
                let outcomes = &outcomes;
                let execute = &execute;
                let dispatcher = tracing::dispatcher::get_default(Clone::clone);
                let parent = tracing::Span::current();
                let worker = std::thread::Builder::new()
                    .name(format!("oven-native-{index}"))
                    .spawn_scoped(scope, move || {
                        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            tracing::dispatcher::with_default(&dispatcher, || {
                                let _parent = parent.enter();
                                execute(index, outcomes).map_err(|error| error.to_string())
                            })
                        }))
                        .unwrap_or_else(|_| Err("native compiler worker panicked".to_string()));
                        let result = outcomes[index]
                            .set(outcome)
                            .map(|()| index)
                            .map_err(|_| "native unit completed twice");
                        // The receiver can disappear only after coordinator failure; scoped joining still retains
                        // owners.
                        let _ = sender.send(result);
                    })?;
                workers.insert(index, worker);
            }
            if workers.is_empty() {
                if completed == dependencies.len() {
                    break;
                }
                return Err("native dependency scheduler made no progress".into());
            }
            let index = receiver.recv()??;
            workers
                .remove(&index)
                .ok_or("unknown native worker completion")?
                .join()
                .map_err(|_| "native compiler worker panicked outside execution")?;
            completed += 1;
        }
        Ok(())
    })?;
    Ok(outcomes)
}

/// Borrow a successfully completed dependency under the exact original Store lease.
fn selected(outcomes: &[OnceLock<Result<Outcome, String>>], index: usize) -> Result<&SdkCompiledUnit, Error> {
    match outcomes.get(index).and_then(OnceLock::get) {
        Some(Ok(Outcome::Compiled { unit, .. })) => Ok(unit),
        _ => Err("dependency was refused or failed".into()),
    }
}

/// Compile one ready node through the unchanged receipt, source lease, native compiler and Store publisher.
fn compile_selected(
    index: usize,
    units: &[PreparedUnit],
    context: &CompileContext<'_>,
    schedule: &Schedule,
    outcomes: &[OnceLock<Result<Outcome, String>>],
) -> Result<(SdkCompiledUnit, bool), Error> {
    let unit = &units[index];
    let dependencies = schedule.edges[index]
        .iter()
        .map(|(name, dependency)| Ok((name.as_str(), selected(outcomes, *dependency)?)))
        .collect::<Result<Vec<_>, Error>>()?;
    let externs = dependencies
        .iter()
        .map(|(name, unit)| (name.to_string(), unit.output.clone()))
        .collect();
    let searches = schedule.closures[index]
        .iter()
        .map(|dependency| {
            selected(outcomes, *dependency)?
                .output
                .parent()
                .map(std::path::Path::to_path_buf)
                .ok_or_else(|| "compiled dependency has no artifact directory".into())
        })
        .collect::<Result<Vec<_>, Error>>()?;
    let bindings = selected_native_bindings(&dependencies)?;
    let (output, reused, owner, reproduced_receipt) = compile_unit(unit, context, externs, searches, &bindings)?;
    let physical_edges = physical_edges::capture(&owner, &reproduced_receipt, &dependencies)?;
    let inspection_dependencies = schedule.edges[index]
        .iter()
        .map(|(name, dependency)| serde_json::json!({"crate": schedule.positions[*dependency], "name": name}))
        .collect();
    let inspection = inspection_unit(unit, &owner.artifact_root().join("source"), inspection_dependencies)?;
    Ok((
        SdkCompiledUnit {
            binding: unit.binding.clone(),
            output,
            owner,
            inspection,
            physical_edges,
            reproduced_receipt,
        },
        reused,
    ))
}

/// Omit failed nodes and compact inspection indices without changing any native receipt, bytes or lease.
fn finish_units(
    units: &[PreparedUnit],
    context: &CompileContext<'_>,
    schedule: &Schedule,
    mut outcomes: Vec<OnceLock<Outcome>>,
) -> Result<SdkCompiledClosure, Error> {
    let indices: BTreeMap<usize, usize> = schedule
        .order
        .iter()
        .copied()
        .filter(|index| matches!(outcomes[*index].get(), Some(Outcome::Compiled { .. })))
        .enumerate()
        .map(|(position, index)| (index, position))
        .collect();
    let mut report = SdkClosureReport::default();
    let mut selected_units = Vec::with_capacity(indices.len());
    for &index in &schedule.order {
        let label = binding_label(&units[index].binding, context.profile);
        match outcomes[index].take().ok_or("native unit has no completion")? {
            Outcome::Refused => report.refused.push(label),
            Outcome::Failed(error) => report.failed.push(format!("{label}: {error}")),
            Outcome::Compiled { mut unit, reused } => {
                let dependencies = schedule.edges[index]
                    .iter()
                    .map(|(name, dependency)| {
                        let position = indices
                            .get(dependency)
                            .ok_or("compiled dependency has no inspection index")?;
                        Ok(serde_json::json!({"crate": position, "name": name}))
                    })
                    .collect::<Result<Vec<_>, Error>>()?;
                *unit
                    .inspection
                    .get_mut("deps")
                    .ok_or("compiled unit has no inspection dependencies")? = serde_json::Value::Array(dependencies);
                selected_units.push(*unit);
                if reused {
                    report.reused.push(label);
                } else {
                    report.compiled.push(label);
                }
            }
        }
    }
    Ok(SdkCompiledClosure {
        report,
        units: selected_units,
        auxiliary_targets: BTreeMap::new(),
    })
}

#[cfg(test)]
mod tests;
