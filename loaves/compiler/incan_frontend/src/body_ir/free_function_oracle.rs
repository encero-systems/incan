//! Free-function input migration oracle. Whole-module facets are deliberately confined to this comparison adapter;
//! successful comparisons prove settled-local input equivalence, not keyed reads, record authentication or reuse.

use super::{BodyIrLoweringFacts, TypeCheckInfo, ast, bir, lower_function_body};
use sha2::{Digest, Sha256};
use std::fs::OpenOptions;
use std::io::Write;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Verify every physical free-function seed in a compatibility module. This deliberately builds the module and
/// exists only as a migration oracle; the production keyed producer must never call it to obtain inputs.
pub fn verify_module(
    program: &ast::Program,
    module_path: &[String],
    info: &TypeCheckInfo,
    context: &super::BodyIrContext<'_>,
    directory: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error>> {
    // ---- Seeds: compare settled inputs with module construction ----
    let mut result = Ok(());
    let mut expected = super::build_module_seed(program, module_path, info, context);
    let mut actual = super::build_module_seed_observed(
        program,
        module_path,
        info,
        context,
        &mut |function, span, facts, expected| {
            if result.is_err() {
                return None;
            }
            match compare_inputs(function, span, facts, expected, directory) {
                Ok(body) => Some(body),
                Err(error) => {
                    result = Err(error);
                    None
                }
            }
        },
    );
    result?;
    // ---- Representations: retain the same finalization schedule ----
    super::retain_module_representations(&mut expected, program, info, context.modules);
    super::retain_module_representations(&mut actual, program, info, context.modules);

    // ---- Evidence: compare each physical free-function output ----
    for declaration in &program.declarations {
        let ast::Declaration::Function(function) = &declaration.node else {
            continue;
        };
        if info
            .declarations
            .native_attribute_externs
            .contains_key(&declaration.span)
        {
            continue;
        }
        let select = |body: &&bir::Body| body.span == super::hir_span(declaration.span) && body.name == function.name;
        let before = expected
            .bodies
            .iter()
            .find(select)
            .ok_or("missing legacy free-function body")?;
        let after = actual
            .bodies
            .iter()
            .find(select)
            .ok_or("missing candidate free-function body")?;
        record_comparison(before, after, "final", directory)?;
    }
    Ok(())
}

/// Keep the complete native and publication comparisons independent; serde intentionally omits native attributes.
/// Each completed report is published by rename; an interrupted compiler can leave only an unselected temporary
/// file, never a truncated JSON row among the oracle's accepted observations.
fn compare_inputs(
    function: &ast::FunctionDecl,
    span: ast::Span,
    facts: &BodyIrLoweringFacts<'_, '_>,
    expected: &bir::Body,
    directory: &std::path::Path,
) -> Result<bir::Body, Box<dyn std::error::Error>> {
    let owners = facts
        .type_info
        .body_ir_oracle_inputs
        .as_ref()
        .ok_or("missing oracle owner capture")?;
    let (checked, declaration) = owners.owners(span).ok_or("missing settled free-function owners")?;
    let mut info = shared_inputs(facts.type_info);
    checked.append_lowering_inputs(declaration, &mut info)?;
    append_default_inputs(facts, owners, &mut info)?;
    complete_publication_inputs(facts.type_info, &mut info);
    let selected = BodyIrLoweringFacts {
        type_info: &info,
        ..*facts
    };
    let actual = lower_function_body(function, span, &selected);
    record_comparison(expected, &actual, "seed", directory)?;
    Ok(actual)
}

/// Compare complete native fields and publication bytes at either boundary, retaining both streams on failure.
/// Ordered containers, source coordinates and invocation-only native attributes are never normalized away.
pub(super) fn record_comparison(
    expected: &bir::Body,
    actual: &bir::Body,
    phase: &str,
    directory: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error>> {
    static COMPARISON: AtomicUsize = AtomicUsize::new(0);
    // ---- Streams: keep native fields and publication independent ----
    let before = format!("{expected:#?}");
    let after = format!("{actual:#?}");
    let expected_bytes = postcard::to_stdvec(expected)?;
    let actual_bytes = postcard::to_stdvec(actual)?;
    let difference = first_difference(&before, &after);
    let same = expected == actual && difference.is_none() && expected_bytes == actual_bytes;
    let sequence = COMPARISON.fetch_add(1, Ordering::Relaxed);
    std::fs::create_dir_all(directory)?;
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let prefix = format!("{}-{sequence:06}-{timestamp}", std::process::id());

    // ---- Differences: retain both complete streams ----
    if !same {
        std::fs::write(directory.join(format!("{prefix}-expected.native.txt")), &before)?;
        std::fs::write(directory.join(format!("{prefix}-actual.native.txt")), &after)?;
        std::fs::write(
            directory.join(format!("{prefix}-expected.publication.bin")),
            &expected_bytes,
        )?;
        std::fs::write(
            directory.join(format!("{prefix}-actual.publication.bin")),
            &actual_bytes,
        )?;
    }

    // ---- Receipt: bind the result and digests to this comparison ----
    let row = serde_json::json!({
        "schema": "incan.body-ir-free-input-oracle/1",
        "identity": expected.decl_id, "name": expected.name, "phase": phase,
        "span": [expected.span.start, expected.span.end], "comparison": prefix,
        "equal": same, "native_equal": expected == actual && before == after,
        "publication_equal": expected_bytes == actual_bytes,
        "native_bytes": before.len(), "publication_bytes": expected_bytes.len(),
        "native_sha256": hex::encode(Sha256::digest(before.as_bytes())),
        "publication_sha256": hex::encode(Sha256::digest(&expected_bytes)),
        "first_difference": difference,
    });
    let temporary = directory.join(format!("{prefix}.tmp"));
    let mut report = OpenOptions::new().create_new(true).write(true).open(&temporary)?;
    let encoded = serde_json::to_vec(&row)?;
    report.write_all(&encoded)?;
    report.write_all(b"\n")?;
    drop(report);
    std::fs::rename(temporary, directory.join(format!("{prefix}.jsonl")))?;
    if !same {
        return Err(format!("free-function {phase} comparison: {row}").into());
    }
    Ok(())
}

/// Report the first structural line, without sorting containers, erasing coordinates or normalizing either body.
fn first_difference(before: &str, after: &str) -> Option<String> {
    let mut left = before.lines();
    let mut right = after.lines();
    let mut line = 0;
    loop {
        let pair = (left.next(), right.next());
        if pair == (None, None) {
            return None;
        }
        line += 1;
        if pair.0 != pair.1 {
            return Some(format!("line {line}: {pair:?}"));
        }
    }
}

/// Copy only the compatibility earlier facets. This oracle adapter intentionally leaves local expression, call,
/// reference and protocol tables empty until the two selected settled owners supply them. These broad earlier
/// facets are not a production BodyFactsView and their copy costs are not producer read measurements.
fn shared_inputs(source: &TypeCheckInfo) -> TypeCheckInfo {
    let mut target = TypeCheckInfo {
        declarations: source.declarations.clone(),
        derivations: source.derivations.clone(),
        consts: source.consts.clone(),
        registry: source.registry.clone(),
        import_bindings: source.import_bindings.clone(),
        body_ir_runtime_types: source.body_ir_runtime_types.clone(),
        body_ir_newtypes: source.body_ir_newtypes.clone(),
        ..TypeCheckInfo::default()
    };
    target.c_abi.enum_values = source.c_abi.enum_values.clone();
    target.c_abi.bindings = source.c_abi.bindings.clone();
    target.c_abi.verified_bindings = source.c_abi.verified_bindings.clone();
    target.calls.compiler_generated_member_identities = source.calls.compiler_generated_member_identities.clone();
    target.calls.closure_holding_params = source.calls.closure_holding_params.clone();
    target.calls.closure_returning_callables = source.calls.closure_returning_callables.clone();
    target.calls.error_message_display_types = source.calls.error_message_display_types.clone();
    target.rust.storage_types = source.rust.storage_types.clone();
    target.rust.owner_parameters = source.rust.owner_parameters.clone();
    target.rust.checked_layouts = source.rust.checked_layouts.clone();
    target.rust.checked_enums = source.rust.checked_enums.clone();
    target.rust.declared_conversions = source.rust.declared_conversions.clone();
    target.rust.rusttype_canonical_paths = source.rust.rusttype_canonical_paths.clone();
    target.rust.item_public_paths = source.rust.item_public_paths.clone();
    target.rust.trait_imports = source.rust.trait_imports.clone();
    target.rust.rusttype_forwarded_trait_adoptions = source.rust.rusttype_forwarded_trait_adoptions.clone();
    target.rust.native_callable_parameters = source.rust.native_callable_parameters.clone();
    target.rust.function_param_type_displays = source.rust.function_param_type_displays.clone();
    target.rust.type_aliases = source.rust.type_aliases.clone();
    target.traits.direct_supertraits = source.traits.direct_supertraits.clone();
    target.traits.type_params = source.traits.type_params.clone();
    target.traits.method_identities = source.traits.method_identities.clone();
    target.traits.default_method_identities = source.traits.default_method_identities.clone();
    target.traits.standard_method_identities = source.traits.standard_method_identities.clone();
    target.traits.adoption_type_args = source.traits.adoption_type_args.clone();
    target.traits.body_ir_adoption_type_args = source.traits.body_ir_adoption_type_args.clone();
    target
}

/// Select post-check publication values by this owner's recorded keys. Native calls are frozen only after local
/// settlement, and accepted empty-list projections are published after the module success gate. Reading their final
/// values is separate from checking and never imports an unrelated body's local expression or call tables.
fn complete_publication_inputs(source: &TypeCheckInfo, target: &mut TypeCheckInfo) {
    for span in target.expressions.expr_types.keys() {
        if let Some(ty) = source.expressions.body_ir_expr_types.get(span) {
            target.expressions.body_ir_expr_types.insert(*span, ty.clone());
        }
    }
    for identity in target
        .references
        .resolved_write_identities
        .values()
        .chain(target.references.resolved_identities.values())
    {
        if let Some(ty) = source.expressions.body_ir_binding_types.get(identity) {
            target
                .expressions
                .body_ir_binding_types
                .insert(identity.clone(), ty.clone());
        }
    }
    target.rust.checked_calls = target
        .rust
        .checked_calls
        .keys()
        .filter_map(|key| source.rust.checked_calls.get(key).map(|value| (*key, value.clone())))
        .collect();
    target.rust.await_conversions = target
        .rust
        .await_conversions
        .keys()
        .filter_map(|key| {
            source
                .rust
                .await_conversions
                .get(key)
                .map(|value| (*key, value.clone()))
        })
        .collect();
    target.rust.argument_adjustments = target
        .rust
        .argument_adjustments
        .keys()
        .filter_map(|key| {
            source
                .rust
                .argument_adjustments
                .get(key)
                .map(|value| (*key, value.clone()))
        })
        .collect();
    target.rust.element_adjustments = target
        .rust
        .element_adjustments
        .keys()
        .filter_map(|key| {
            source
                .rust
                .element_adjustments
                .get(key)
                .map(|value| (*key, value.clone()))
        })
        .collect();
    target.rust.field_adjustments = target
        .rust
        .field_adjustments
        .keys()
        .filter_map(|key| {
            source
                .rust
                .field_adjustments
                .get(key)
                .map(|value| (*key, value.clone()))
        })
        .collect();
    target.rust.checked_function_values = target
        .rust
        .checked_function_values
        .keys()
        .filter_map(|key| {
            source
                .rust
                .checked_function_values
                .get(key)
                .map(|value| (*key, value.clone()))
        })
        .collect();
    target.rust.iteration_hooks = target
        .rust
        .iteration_hooks
        .keys()
        .filter_map(|key| source.rust.iteration_hooks.get(key).map(|value| (*key, value.clone())))
        .collect();
}

/// Follow selected local callable declarations with defaults, including defaults' own earlier references. This
/// compatibility adapter selects declaration outputs only; it never joins a callee's statement result. Real record
/// production must obtain these default projections through the shared scoped reader rather than this native source.
fn append_default_inputs(
    facts: &BodyIrLoweringFacts<'_, '_>,
    owners: &crate::typechecker::BodyIrOracleInputs,
    target: &mut TypeCheckInfo,
) -> Result<(), Box<dyn std::error::Error>> {
    // ---- Suppliers: follow canonical references to retained source defaults ----
    let declared = facts
        .type_info
        .declarations
        .declaration_identities
        .values()
        .chain(facts.type_info.declarations.member_declaration_identities.values())
        .collect::<std::collections::HashSet<_>>();
    let mut visited = std::collections::BTreeSet::new();
    loop {
        let mut selected = std::collections::BTreeMap::new();
        for identity in target
            .references
            .resolved_identities
            .values()
            .filter(|identity| declared.contains(identity))
        {
            let Some(recipes) = facts.function_default_sources.get(&identity.declaration_name) else {
                continue;
            };
            for expression in recipes.iter().filter_map(|recipe| recipe.default.as_ref()) {
                let declaration = owners
                    .default_owner(expression.span)
                    .ok_or("missing or ambiguous checked default expression owner")?;
                if !visited.contains(&declaration.owner()) {
                    selected.insert(declaration.owner(), declaration);
                }
            }
        }

        // ---- Frontier: stop once every selected default writer has been included ----
        if selected.is_empty() {
            return Ok(());
        }

        // ---- Inputs: join declaration outputs without statement results ----
        for (span, declaration) in selected {
            visited.insert(span);
            declaration.append_declaration_inputs(target)?;
        }
    }
}
