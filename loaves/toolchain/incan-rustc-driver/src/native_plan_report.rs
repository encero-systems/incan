//! Opt-in native-plan and monomorphization measurements for one completed analysis.

use crate::plan::{CalleeKind, Function, Plan, TerminatorKind, Unwind};
use rustc_middle::mono::MonoItem;
use rustc_middle::ty::{InstanceKind, ShimKind, TyCtxt};
use std::collections::BTreeMap;
use std::fmt::Write;

/// One unique monomorphized symbol and every codegen unit that contains it.
struct MonoRecord {
    category: &'static str,
    definition: String,
    size_estimate: usize,
    codegen_units: Vec<String>,
}

/// Write the requested plan census after rustc analysis, forcing the same cached monomorphization query codegen uses.
/// `INCAN_NATIVE_PLAN_REPORT` names the output; ordinary compilations do not run the query early or write a report.
pub(crate) fn write(plan: &Plan, tcx: TyCtxt<'_>) -> Result<(), String> {
    let Some(path) = std::env::var_os("INCAN_NATIVE_PLAN_REPORT") else {
        return Ok(());
    };
    let mut report = String::from("incan-native-plan-report\t2\n");
    write_plan(plan, &mut report)?;
    write_monomorphization(tcx, &mut report)?;
    std::fs::write(&path, report).map_err(|error| format!("cannot write native plan report: {error}"))
}

/// Record aggregate plan nodes and rank authored functions by their exact retained body-node counts.
fn write_plan(plan: &Plan, report: &mut String) -> Result<(), String> {
    let mut functions = Vec::with_capacity(plan.functions.len());
    let mut total_locals = 0;
    let mut total_blocks = 0;
    let mut total_statements = 0;
    let mut total_calls = 0;
    let mut total_drops = 0;
    let mut total_cleanup_blocks = 0;
    let mut total_cleanup_drops = 0;
    let mut total_cleanup_roots = 0;
    let mut total_unique_cleanup_roots = 0;
    let mut total_cleanup_chain_steps = 0;
    let mut maximum_cleanup_chain = 0;
    let mut total_model_clones = 0;
    let mut total_enum_clones = 0;
    let mut total_closure_clones = 0;
    for function in &plan.functions {
        let blocks = function.blocks.len();
        let statements = function
            .blocks
            .iter()
            .map(|block| block.statements.len())
            .sum::<usize>();
        let calls = function
            .blocks
            .iter()
            .filter(|block| matches!(block.terminator.kind, TerminatorKind::Call(..)))
            .count();
        let drops = function
            .blocks
            .iter()
            .filter(|block| matches!(block.terminator.kind, TerminatorKind::Drop(..)))
            .count();
        let cleanup = cleanup_metrics(function);
        let mut model_clones = 0;
        let mut enum_clones = 0;
        let mut closure_clones = 0;
        for block in &function.blocks {
            let TerminatorKind::Call(callee, ..) = &block.terminator.kind else {
                continue;
            };
            match &callee.kind {
                CalleeKind::CloneModel(..) => model_clones += 1,
                CalleeKind::CloneEnum(..) => enum_clones += 1,
                CalleeKind::CloneClosure(..) => closure_clones += 1,
                _ => {}
            }
        }
        total_locals += function.locals.len();
        total_blocks += blocks;
        total_statements += statements;
        total_calls += calls;
        total_drops += drops;
        total_cleanup_blocks += cleanup.blocks;
        total_cleanup_drops += cleanup.drops;
        total_cleanup_roots += cleanup.roots;
        total_unique_cleanup_roots += cleanup.unique_roots;
        total_cleanup_chain_steps += cleanup.chain_steps;
        maximum_cleanup_chain = maximum_cleanup_chain.max(cleanup.maximum_chain);
        total_model_clones += model_clones;
        total_enum_clones += enum_clones;
        total_closure_clones += closure_clones;
        functions.push((
            statements + blocks + function.locals.len(),
            statements,
            blocks,
            function.locals.len(),
            calls,
            drops,
            cleanup.blocks,
            cleanup.drops,
            cleanup.roots,
            cleanup.unique_roots,
            cleanup.chain_steps,
            cleanup.maximum_chain,
            model_clones,
            enum_clones,
            closure_clones,
            function.span.file.as_str(),
            function.name.as_str(),
        ));
    }
    functions.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.16.cmp(right.16)));
    writeln!(
        report,
        "plan\tfunctions\t{}\tlocals\t{total_locals}\tblocks\t{total_blocks}\tstatements\t{total_statements}\tcalls\t{total_calls}\tdrops\t{total_drops}\tcleanup-blocks\t{total_cleanup_blocks}\tcleanup-drops\t{total_cleanup_drops}\tcleanup-roots\t{total_cleanup_roots}\tunique-cleanup-roots\t{total_unique_cleanup_roots}\tcleanup-chain-steps\t{total_cleanup_chain_steps}\tmaximum-cleanup-chain\t{maximum_cleanup_chain}\tmodel-clones\t{total_model_clones}\tenum-clones\t{total_enum_clones}\tclosure-clones\t{total_closure_clones}",
        plan.functions.len(),
    )
    .map_err(|error| error.to_string())?;
    for (
        nodes,
        statements,
        blocks,
        locals,
        calls,
        drops,
        cleanup_blocks,
        cleanup_drops,
        cleanup_roots,
        unique_cleanup_roots,
        cleanup_chain_steps,
        maximum_cleanup_chain,
        model_clones,
        enum_clones,
        closure_clones,
        file,
        name,
    ) in functions
    {
        writeln!(
            report,
            "plan-function\t{nodes}\tstatements\t{statements}\tblocks\t{blocks}\tlocals\t{locals}\tcalls\t{calls}\tdrops\t{drops}\tcleanup-blocks\t{cleanup_blocks}\tcleanup-drops\t{cleanup_drops}\tcleanup-roots\t{cleanup_roots}\tunique-cleanup-roots\t{unique_cleanup_roots}\tcleanup-chain-steps\t{cleanup_chain_steps}\tmaximum-cleanup-chain\t{maximum_cleanup_chain}\tmodel-clones\t{model_clones}\tenum-clones\t{enum_clones}\tclosure-clones\t{closure_clones}\t{file}\t{name}",
        )
        .map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// Physical cleanup nodes and the rooted chain lengths that fallible terminators can enter.
struct CleanupMetrics {
    blocks: usize,
    drops: usize,
    roots: usize,
    unique_roots: usize,
    chain_steps: usize,
    maximum_chain: usize,
}

/// Count cleanup storage separately from ordinary drops and follow every unwind root through its drop tail.
fn cleanup_metrics(function: &Function) -> CleanupMetrics {
    let blocks = function.blocks.iter().filter(|block| block.cleanup).count();
    let drops = function
        .blocks
        .iter()
        .filter(|block| block.cleanup && matches!(block.terminator.kind, TerminatorKind::Drop(..)))
        .count();
    let mut roots = Vec::new();
    for block in &function.blocks {
        match &block.terminator.kind {
            TerminatorKind::Call(_, _, _, _, action)
            | TerminatorKind::Drop(_, _, action)
            | TerminatorKind::AssertOverflow(_, _, _, _, _, action) => retain_cleanup_root(action, &mut roots),
            TerminatorKind::Await(_, _, _, _, action, drop_action) => {
                retain_cleanup_root(action, &mut roots);
                retain_cleanup_root(drop_action, &mut roots);
            }
            _ => {}
        }
    }
    let unique_roots = roots.iter().copied().collect::<std::collections::BTreeSet<_>>().len();
    let mut chain_steps = 0;
    let mut maximum_chain = 0;
    for root in &roots {
        let length = cleanup_chain_length(function, *root);
        chain_steps += length;
        maximum_chain = maximum_chain.max(length);
    }
    CleanupMetrics {
        blocks,
        drops,
        roots: roots.len(),
        unique_roots,
        chain_steps,
        maximum_chain,
    }
}

/// Retain only explicit cleanup actions; continue and terminate actions do not enter an owned-value chain.
fn retain_cleanup_root(action: &Unwind, roots: &mut Vec<i64>) {
    if let Unwind::Cleanup(target) = action {
        roots.push(*target);
    }
}

/// Follow one validated cleanup chain until its resume block, refusing malformed indices or cycles in diagnostic code.
fn cleanup_chain_length(function: &Function, root: i64) -> usize {
    let mut current = root;
    let mut visited = std::collections::BTreeSet::new();
    let mut length = 0;
    while visited.insert(current) {
        let Ok(index) = usize::try_from(current) else {
            break;
        };
        let Some(block) = function.blocks.get(index) else {
            break;
        };
        if !block.cleanup {
            break;
        }
        match &block.terminator.kind {
            TerminatorKind::Drop(_, target, _) => {
                length += 1;
                current = *target;
            }
            TerminatorKind::UnwindResume => break,
            _ => break,
        }
    }
    length
}

/// Record rustc's codegen partitions and rank unique mono items by rustc's MIR-derived size estimate.
fn write_monomorphization(tcx: TyCtxt<'_>, report: &mut String) -> Result<(), String> {
    let partitions = tcx.collect_and_partition_mono_items(());
    let mut records: BTreeMap<String, MonoRecord> = BTreeMap::new();
    let mut placements = 0;
    for unit in partitions.codegen_units {
        writeln!(
            report,
            "codegen-unit\t{}\titems\t{}\tsize-estimate\t{}",
            unit.name(),
            unit.items().len(),
            unit.size_estimate(),
        )
        .map_err(|error| error.to_string())?;
        for (item, data) in unit.items() {
            placements += 1;
            let symbol = item.symbol_name(tcx).to_string();
            let (category, definition) = mono_identity(tcx, *item);
            let record = records.entry(symbol).or_insert_with(|| MonoRecord {
                category,
                definition,
                size_estimate: data.size_estimate,
                codegen_units: Vec::new(),
            });
            record.size_estimate = record.size_estimate.max(data.size_estimate);
            record.codegen_units.push(unit.name().to_string());
        }
    }
    let drop_glue = records.values().filter(|record| record.category == "drop-glue").count();
    let generic = records.values().filter(|record| record.category == "generic").count();
    writeln!(
        report,
        "mono\tunique\t{}\tplacements\t{placements}\tdrop-glue\t{drop_glue}\tgeneric\t{generic}\tcodegen-units\t{}",
        records.len(),
        partitions.codegen_units.len(),
    )
    .map_err(|error| error.to_string())?;
    let mut ranked = records.into_iter().collect::<Vec<_>>();
    ranked.sort_by(|left, right| {
        right
            .1
            .size_estimate
            .cmp(&left.1.size_estimate)
            .then_with(|| left.0.cmp(&right.0))
    });
    for (symbol, record) in ranked {
        writeln!(
            report,
            "mono-item\t{}\t{}\tplacements\t{}\t{}\t{}\t{}",
            record.size_estimate,
            record.category,
            record.codegen_units.len(),
            record.codegen_units.join(","),
            record.definition,
            symbol,
        )
        .map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// Give each mono item a stable diagnostic category and a readable defining path.
fn mono_identity(tcx: TyCtxt<'_>, item: MonoItem<'_>) -> (&'static str, String) {
    match item {
        MonoItem::Fn(instance) => {
            let category = match instance.def {
                InstanceKind::Shim(ShimKind::DropGlue(_, _)) => "drop-glue",
                InstanceKind::Item(_) if instance.args.non_erasable_generics().next().is_some() => "generic",
                InstanceKind::Item(_) => "function",
                _ => "shim",
            };
            (category, tcx.def_path_str(instance.def_id()))
        }
        MonoItem::Static(definition) => ("static", tcx.def_path_str(definition)),
        MonoItem::GlobalAsm(item) => ("global-asm", format!("{item:?}")),
    }
}
