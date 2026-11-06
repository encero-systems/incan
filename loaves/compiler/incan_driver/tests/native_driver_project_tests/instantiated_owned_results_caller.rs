//! Checked stdlib instantiations retain owned carrier results through normal and unwinding exits.

use super::{TestResult, plan, validation};
use incan_frontend::api_metadata::{
    CHECKED_API_METADATA_SCHEMA_VERSION, CheckedApiMetadataPackage, collect_checked_api_metadata,
    materialize_checked_api_public_namespaces,
};
use incan_frontend::body_ir::{build_body_ir_module_v0, library_call_delegations};
use incan_frontend::library_exports::collect_checked_public_exports;
use incan_frontend::library_manifest::{LibraryIdentityGraph, LibraryManifest};
use incan_frontend::library_manifest_index::LibraryManifestIndex;
use incan_frontend::provider::{
    NamespaceAuthority, ProviderIdentity, ProviderPlan, ProviderProvenance, ProviderRecord,
};
use incan_frontend::{lexer, parser, typechecker::TypeChecker};
use incan_mir_lowering::caller::incan::lower_module;
use std::collections::BTreeSet;
use std::sync::Arc;

const SOURCE: &str = r#""""Exercise the lifetime of a discarded stdlib Result return."""

from std.result import map


def double(value: int) -> int:
    """Provide the combinator's checked callback type."""
    return value * 2


def main() -> None:
    """Discard the returned text-carrying Result before another call can unwind."""
    failure: Result[int, str] = Err("owned")
    map(failure, double)
    println("after")
"#;

/// Publish the real std.result source's checked API and identities, rather than fabricate delegated signatures.
fn result_provider() -> TestResult<ProviderPlan> {
    let tokens = lexer::lex(include_str!("result.incn")).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let package = "incan_stdlib_core";
    let module_path = vec!["result".to_owned()];
    let mut checker = TypeChecker::new();
    checker.set_standard_library_source(true);
    checker.set_current_package_identity(Some(package.to_owned()));
    checker.set_current_module_path(Some(module_path.clone()));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let exports = collect_checked_public_exports(&program, &checker);
    let mut api = CheckedApiMetadataPackage {
        schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
        package: None,
        modules: vec![collect_checked_api_metadata(&program, &checker, module_path.clone())],
        public_namespaces: Vec::new(),
    };
    materialize_checked_api_public_namespaces(&mut api)?;
    let mut graph = LibraryIdentityGraph::from_checked_exports(package, &[]);
    graph.extend_checked_api_exports(package, &api, &[(module_path, exports)])?;
    let mut manifest = LibraryManifest::new(package, "0.6.0-dev.6");
    manifest.contract_metadata.api = Some(api);
    manifest.contract_metadata.identity_graph = graph;
    let claims = BTreeSet::from([vec!["std".to_owned(), "result".to_owned()]]);
    Ok(ProviderPlan::new(
        LibraryManifestIndex::default(),
        vec![ProviderRecord {
            identity: ProviderIdentity {
                name: package.to_owned(),
                version: "0.6.0-dev.6".to_owned(),
                digest: "in-memory:checked-result-source".to_owned(),
                feature_projection: BTreeSet::new(),
            },
            provenance: ProviderProvenance::Sdk {
                sdk_identity: "in-memory:checked-result-source".to_owned(),
                component_id: "stdlib-core".to_owned(),
                inventory_path: None,
            },
            authority: NamespaceAuthority::SdkReserved,
            namespace_claims: claims.clone(),
            available: true,
            enabled: true,
            manifest: Some(Arc::new(manifest)),
            artifact: None,
            implementation_facets: Vec::new(),
        }],
        claims,
    )?)
}

/// Lower actual checked consumer source using crate-call records collected from its real provider identities.
fn checked_map_plan() -> TestResult<(plan::Plan, String)> {
    let tokens = lexer::lex(SOURCE).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let module_path = vec!["dev7_instantiated_ownership".to_owned()];
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(module_path.clone()));
    checker.set_provider_plan(Arc::new(result_provider()?));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let records = library_call_delegations(checker.type_info());
    let map_path = records
        .iter()
        .find(|record| record.canonical.declaration_name == "map")
        .and_then(|record| record.rust_path())
        .ok_or("missing genuine checked std.result.map crate-call record")?;
    let mut module = build_body_ir_module_v0(&program, &module_path, checker.type_info());
    module.stdlib_delegations.extend(records);
    let lowered = lower_module(
        &module,
        SOURCE.to_owned(),
        "dev7_instantiated_ownership.incn".to_owned(),
    )?;
    validation::validate(&lowered)?;
    Ok((lowered, map_path))
}

/// Collect exact drop locals along one cleanup chain, rejecting malformed indices, cycles, and noncleanup blocks.
fn cleanup_drops(function: &plan::Function, start: i64) -> TestResult<Vec<i64>> {
    let mut index = usize::try_from(start)?;
    let mut visited = BTreeSet::new();
    let mut drops = Vec::new();
    loop {
        if !visited.insert(index) {
            return Err("cycle in instantiated-result cleanup".into());
        }
        let block = function.blocks.get(index).ok_or("missing cleanup block")?;
        if !block.cleanup {
            return Err("instantiated-result unwind entered a normal block".into());
        }
        match &block.terminator.kind {
            plan::TerminatorKind::Drop(place, next, _) => {
                if !matches!(place.projection, plan::Projection::Whole) {
                    return Err("cleanup drop must name a whole owner".into());
                }
                drops.push(place.local);
                index = usize::try_from(*next)?;
            }
            plan::TerminatorKind::Goto(next) => index = usize::try_from(*next)?,
            plan::TerminatorKind::UnwindResume => return Ok(drops),
            _ => return Err("unexpected instantiated-result cleanup terminator".into()),
        }
    }
}

/// Follow return-value moves to their final owner and require that owner's normal and subsequent-call unwind drops.
fn check_result_lifetime(function: &plan::Function, call_index: usize) -> TestResult<()> {
    let call = function.blocks.get(call_index).ok_or("missing instantiated call")?;
    let plan::TerminatorKind::Call(_, _, destination, continuation, _) = &call.terminator.kind else {
        return Err("expected instantiated-result call".into());
    };
    let mut owner = destination.local;
    let mut moved = BTreeSet::new();
    let mut index = usize::try_from(*continuation)?;
    let mut visited = BTreeSet::new();
    let mut normal_drops = 0;
    let mut checked_print = false;
    loop {
        if !visited.insert(index) {
            return Err("cycle on instantiated-result normal path".into());
        }
        let block = function.blocks.get(index).ok_or("missing normal successor")?;
        if block.cleanup {
            return Err("normal instantiated-result successor is cleanup".into());
        }
        for statement in &block.statements {
            if let plan::StatementKind::Assign(target, value) = &statement.kind
                && let plan::RvalueKind::Use(operand) = &value.kind
                && let plan::OperandKind::Move(source) = &operand.kind
                && source.local == owner
                && matches!(source.projection, plan::Projection::Whole)
            {
                if !matches!(target.projection, plan::Projection::Whole) || normal_drops != 0 {
                    return Err("returned carrier moved into a projection or after dropping".into());
                }
                if target.local != owner {
                    moved.insert(owner);
                    owner = target.local;
                }
            }
        }
        match &block.terminator.kind {
            plan::TerminatorKind::Call(callee, _, _, next, unwind) => {
                if matches!(&callee.kind, plan::CalleeKind::External(path) if path == "incan_native_runtime::println_text")
                {
                    if checked_print || normal_drops != 0 {
                        return Err("expected one following println with the returned owner still live".into());
                    }
                    let plan::Unwind::Cleanup(cleanup) = unwind else {
                        return Err("following println has no cleanup chain".into());
                    };
                    let drops = cleanup_drops(function, *cleanup)?;
                    if drops.iter().filter(|local| **local == owner).count() != 1 {
                        return Err(
                            "following println cleanup must drop the final map-result owner exactly once".into(),
                        );
                    }
                    if drops.iter().any(|local| moved.contains(local)) {
                        return Err("following println cleanup drops a moved return intermediary".into());
                    }
                    checked_print = true;
                }
                index = usize::try_from(*next)?;
            }
            plan::TerminatorKind::Drop(place, next, _) => {
                if moved.contains(&place.local) {
                    return Err("normal path drops a moved return intermediary".into());
                }
                if place.local == owner {
                    if !matches!(place.projection, plan::Projection::Whole) {
                        return Err("normal result drop must name the whole carrier".into());
                    }
                    normal_drops += 1;
                }
                index = usize::try_from(*next)?;
            }
            plan::TerminatorKind::Goto(next) => index = usize::try_from(*next)?,
            plan::TerminatorKind::Return => {
                if !checked_print || normal_drops != 1 {
                    return Err("discarded map result must drop exactly once on the normal return path".into());
                }
                return Ok(());
            }
            _ => return Err("unexpected instantiated-result normal terminator".into()),
        }
    }
}

/// Require a real three-parameter stdlib instantiation returning a text-bearing Result before checking its lifetime.
pub(super) fn check() -> TestResult<()> {
    let (lowered, path) = checked_map_plan()?;
    let mut found = 0;
    for function in &lowered.functions {
        for (index, block) in function.blocks.iter().enumerate() {
            let plan::TerminatorKind::Call(callee, _, destination, _, _) = &block.terminator.kind else {
                continue;
            };
            let plan::CalleeKind::Instantiated(called, types) = &callee.kind else {
                continue;
            };
            if called != &path {
                continue;
            }
            if types.len() < 3 || !matches!(destination.projection, plan::Projection::Whole) {
                return Err("expected checked map instantiation with all type parameters and a whole result".into());
            }
            let local = function
                .locals
                .get(usize::try_from(destination.local)?)
                .ok_or("missing map output local")?;
            let plan::PlanType::Enum(owner, _) = &local.ty else {
                return Err("expected map to return a Result enum carrier".into());
            };
            let declaration = lowered
                .enums
                .get(usize::try_from(*owner)?)
                .ok_or("missing returned Result layout")?;
            if declaration.carrier != "Result"
                || !declaration.variants.iter().any(|variant| {
                    variant.name == "Err" && variant.fields.iter().any(|ty| matches!(ty, plan::PlanType::String))
                })
            {
                return Err("checked map result must own its String error payload".into());
            }
            check_result_lifetime(function, index)?;
            found += 1;
        }
    }
    if found != 1 {
        return Err(format!("expected exactly one checked std.result.map call, got {found}").into());
    }
    println!("instantiated result ownership: normal and unwind drops passed");
    Ok(())
}

/// Follow the straight-line checked source's normal control-flow path without mistaking cleanup blocks for execution.
fn formatting_normal_blocks(function: &plan::Function) -> TestResult<Vec<usize>> {
    let mut index = 0;
    let mut visited = BTreeSet::new();
    let mut blocks = Vec::new();
    loop {
        if !visited.insert(index) {
            return Err("cycle in validation-format normal path".into());
        }
        let block = function
            .blocks
            .get(index)
            .ok_or("missing validation-format normal block")?;
        if block.cleanup {
            return Err("validation-format normal path entered cleanup".into());
        }
        blocks.push(index);
        let next = match &block.terminator.kind {
            plan::TerminatorKind::Call(_, _, _, next, _)
            | plan::TerminatorKind::Drop(_, next, _)
            | plan::TerminatorKind::Goto(next) => *next,
            plan::TerminatorKind::Return => return Ok(blocks),
            _ => return Err("unexpected validation-format normal terminator".into()),
        };
        index = usize::try_from(next)?;
    }
}

/// Read the actual runtime call's destination and unwind edge, rejecting a source path that lost its formatter.
fn formatting_call<'a>(
    function: &'a plan::Function,
    index: usize,
    path: &str,
) -> TestResult<(&'a plan::Place, &'a plan::Unwind)> {
    let block = function
        .blocks
        .get(index)
        .ok_or("missing validation-format call block")?;
    let plan::TerminatorKind::Call(callee, _, destination, _, unwind) = &block.terminator.kind else {
        return Err("expected validation-format runtime call".into());
    };
    if !matches!(&callee.kind, plan::CalleeKind::External(selected) if selected == path)
        || !matches!(destination.projection, plan::Projection::Whole)
    {
        return Err(format!("expected whole-result runtime call to {path}").into());
    }
    Ok((destination, unwind))
}

/// Require the two successful formatter outputs to move in order into the exact array the f-string runtime borrows.
fn formatting_array_owner(
    function: &plan::Function,
    normal: &[usize],
    second_position: usize,
    outputs: &[i64; 2],
) -> TestResult<(i64, usize)> {
    let mut found = None;
    for (position, index) in normal.iter().enumerate() {
        let block = function
            .blocks
            .get(*index)
            .ok_or("missing formatting-array normal block")?;
        for statement in &block.statements {
            let plan::StatementKind::Assign(destination, value) = &statement.kind else {
                continue;
            };
            let plan::RvalueKind::Array(elements) = &value.kind else {
                continue;
            };
            let local = function
                .locals
                .get(usize::try_from(destination.local)?)
                .ok_or("missing array local")?;
            if !matches!(local.ty, plan::PlanType::StringArray(2)) {
                continue;
            }
            if found.is_some() || position <= second_position || elements.len() != 2 {
                return Err("expected one two-string array after the second successful formatter call".into());
            }
            for (element, expected) in elements.iter().zip(outputs) {
                if !matches!(&element.kind, plan::OperandKind::Move(source)
                    if source.local == *expected && matches!(source.projection, plan::Projection::Whole))
                {
                    return Err("formatted strings must move into their array in source order".into());
                }
            }
            if !matches!(destination.projection, plan::Projection::Whole) {
                return Err("formatted string array must have a whole owner".into());
            }
            found = Some((destination.local, position));
        }
    }
    found.ok_or_else(|| "missing ownership transfer into the formatted string array".into())
}

/// Check genuine source lowering: each formatter output owns once until it moves into the f-string value array.
pub(super) fn check_validation_format() -> TestResult<()> {
    const SOURCE: &str =
        "def main() -> None:\n    error = ValidationError(\"owned\")\n    text = f\"{error} {error}\"\n";
    const FORMATTER: &str = "incan_native_runtime::format_validation_error";
    let module = super::checked_source(SOURCE, "dev7_validation_format_ownership")?;
    let lowered = lower_module(
        &module,
        SOURCE.to_owned(),
        "dev7_validation_format_ownership.incn".to_owned(),
    )?;
    let function = lowered
        .functions
        .iter()
        .find(|body| body.name == "main")
        .ok_or("missing formatting main")?;
    let normal = formatting_normal_blocks(function)?;
    let calls = normal
        .iter()
        .enumerate()
        .filter_map(|(position, index)| {
            let block = function.blocks.get(*index)?;
            match &block.terminator.kind {
                plan::TerminatorKind::Call(callee, _, _, _, _)
                    if matches!(&callee.kind, plan::CalleeKind::External(path) if path == FORMATTER) =>
                {
                    Some((position, *index))
                }
                _ => None,
            }
        })
        .collect::<Vec<_>>();
    if calls.len() != 2 {
        return Err(format!(
            "expected exactly two checked error formatter calls, got {}",
            calls.len()
        )
        .into());
    }
    let (first, _) = formatting_call(function, calls[0].1, FORMATTER)?;
    let (second, unwind) = formatting_call(function, calls[1].1, FORMATTER)?;
    let outputs = [first.local, second.local];
    if first.local == second.local {
        return Err("two f-string fields must retain distinct formatter outputs".into());
    }
    for output in outputs {
        let local = function
            .locals
            .get(usize::try_from(output)?)
            .ok_or("missing formatter result local")?;
        if !matches!(local.ty, plan::PlanType::String) {
            return Err("formatter output must retain its owned String representation".into());
        }
    }
    let plan::Unwind::Cleanup(start) = unwind else {
        return Err("second formatter must clean up the first successful formatter's live output".into());
    };
    let drops = cleanup_drops(function, *start)?;
    let first_drops = drops.iter().filter(|local| **local == first.local).count();
    if first_drops != 1 {
        return Err(format!(
            "second formatter cleanup must drop first formatter output exactly once, got {first_drops}"
        )
        .into());
    }
    if drops.contains(&second.local) {
        return Err("second formatter unwind cannot drop its uninitialized output".into());
    }

    let (array, array_position) = formatting_array_owner(function, &normal, calls[1].0, &outputs)?;
    let fstrings = normal.iter().enumerate().filter_map(|(position, index)| {
        let block = function.blocks.get(*index)?;
        match &block.terminator.kind {
            plan::TerminatorKind::Call(callee, _, _, _, _)
                if matches!(&callee.kind, plan::CalleeKind::External(path) if path == "incan_std_core::strings::fstring") =>
            {
                Some((position, *index))
            }
            _ => None,
        }
    }).collect::<Vec<_>>();
    let [(fstring_position, fstring_index)] = fstrings.as_slice() else {
        return Err("expected exactly one f-string runtime call".into());
    };
    if *fstring_position < array_position {
        return Err("f-string runtime call must follow formatted-value ownership transfer".into());
    }
    let (_, unwind) = formatting_call(function, *fstring_index, "incan_std_core::strings::fstring")?;
    let plan::Unwind::Cleanup(start) = unwind else {
        return Err("f-string runtime call must clean up its owned value array".into());
    };
    let drops = cleanup_drops(function, *start)?;
    if drops.iter().filter(|local| **local == array).count() != 1 || outputs.iter().any(|output| drops.contains(output))
    {
        return Err("f-string cleanup must drop its array once and never its moved formatter outputs".into());
    }
    let mut array_drops = 0;
    for index in normal {
        let block = function
            .blocks
            .get(index)
            .ok_or("missing formatting normal-drop block")?;
        if let plan::TerminatorKind::Drop(place, _, _) = &block.terminator.kind {
            if outputs.contains(&place.local) {
                return Err("normal formatting path drops a formatter output transferred to its array".into());
            }
            if place.local == array {
                array_drops += 1;
            }
        }
    }
    if array_drops != 1 {
        return Err(format!("formatted value array must drop once normally, got {array_drops}").into());
    }
    validation::validate(&lowered)?;
    println!("validation formatting ownership: second-call cleanup and array transfer passed");
    Ok(())
}
