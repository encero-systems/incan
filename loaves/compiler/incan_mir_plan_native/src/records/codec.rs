//! Loss-check the complete native interface for ordinary programs through the existing function codec.

use crate::nominal::{NominalKind, NominalReference, ProductNominal};
use crate::plan::{Function, Plan};
use crate::records::function::{decode_product, decode_runtime, encode_product, encode_runtime, product_nominals};
use crate::records::layout::{
    decode_enums, decode_models, decode_zip_carriers, encode_enums, encode_models, encode_zip_carriers,
};
use crate::records::plan_function::encode_entry_record;
use crate::types::source_span;
use crate::wire::{
    Schema, Shape, ShapeBody, ShapeField, Wire, decode_graph_row, encode_graph_row, items, number, required, text,
    tuple_items,
};

/// The root selector of a unit's native interface.
const PLAN_ROOT: &[u8] = b"mirpln\x00";

/// A unit interface preserves all native tables, reference ordinals and the plan's diagnostic span.
static NATIVE_PLAN: Shape = Shape {
    selector: PLAN_ROOT,
    path: "incan.native-plan/5",
    body: ShapeBody::Record(&[
        ShapeField {
            identity: b"enums_",
            name: "enums",
            storage: "list<binary>",
        },
        ShapeField {
            identity: b"functs",
            name: "functions",
            storage: "list<binary>",
        },
        ShapeField {
            identity: b"models",
            name: "models",
            storage: "list<binary>",
        },
        ShapeField {
            identity: b"progra",
            name: "programs",
            storage: "list<string>",
        },
        ShapeField {
            identity: b"refers",
            name: "references",
            storage: "list<list<tuple<string,i64,string>>>",
        },
        ShapeField {
            identity: b"runtim",
            name: "runtime",
            storage: "list<binary>",
        },
        ShapeField {
            identity: b"span__",
            name: "span",
            storage: "tuple<string,i64,i64>",
        },
        ShapeField {
            identity: b"zipcar",
            name: "zip_carriers",
            storage: "list<binary>",
        },
    ]),
};
static NATIVE_PLAN_SCHEMA: Schema = Schema::new(&[&NATIVE_PLAN]);

/// Every nonempty field admitted by the bounded ordinary-program profile.
struct StoredPlan {
    functions: Vec<Vec<u8>>,
    references: Vec<Vec<NominalReference>>,
    runtime: Vec<Vec<u8>>,
    programs: Vec<String>,
    models: Vec<Vec<u8>>,
    enums: Vec<Vec<u8>>,
    zip_carriers: Vec<Vec<u8>>,
    file: String,
    line: i64,
    column: i64,
}

/// Bind the standalone entry under the same typed row the graph retains.
pub fn encode_entry_binding(entry: &str, inputs: &[String]) -> Result<Vec<u8>, String> {
    encode_entry_record(entry, inputs)
}

/// Encode one complete plan or explicitly refuse a field the codec cannot preserve.
///
/// Each function's product names its models and enums by their interface declaration names.
pub fn encode_record_plan(plan: &Plan) -> Result<Vec<u8>, String> {
    let mut functions = Vec::with_capacity(plan.functions.len());
    let mut references = Vec::with_capacity(plan.functions.len());
    for function in &plan.functions {
        let named = product_nominals(function, &[], &[])?;
        functions.push(encode_product(function, &[], &interface_nominals(plan, &named)?, &[])?);
        references.push(named);
    }
    encode_record_plan_products(plan, functions, references)
}

/// Encode a complete interface while retaining its keyed function-product bytes in declaration order.
///
/// A product's runtime list is the lowering demand its declaration recorded. The unit's resolved runtime inventory is
/// a separate interface field, so a keyed product never carries another declaration's requirements. A product names
/// declarations, never positions; `references` binds each product's ordinals to this interface's own tables, and is
/// empty only when no product names a model or enum.
pub fn encode_record_plan_products(
    plan: &Plan,
    functions: Vec<Vec<u8>>,
    references: Vec<Vec<NominalReference>>,
) -> Result<Vec<u8>, String> {
    let counts = [
        ("statics", plan.statics.len()),
        ("exports", plan.exports.len()),
        ("tests", plan.tests.len()),
        ("generic models", plan.generic_models.len()),
        ("generic enums", plan.generic_enums.len()),
    ];
    if let Some((field, count)) = counts.iter().find(|(_, count)| *count != 0) {
        return Err(format!("native-interface refuses {field}: {count}"));
    }
    if plan.functions.is_empty() {
        return Err("native-interface refuses an empty plan".to_owned());
    }
    if functions.len() != plan.functions.len() {
        return Err("native-interface product count differs from its plan functions".to_owned());
    }
    if !references.is_empty() && references.len() != functions.len() {
        return Err("native-interface product references differ from its plan functions".to_owned());
    }
    let references = if references.is_empty() {
        vec![Vec::new(); functions.len()]
    } else {
        references
    };
    let stored = StoredPlan {
        functions,
        references,
        runtime: encode_runtime(&plan.externals)?,
        programs: plan.programs.clone(),
        models: encode_models(&plan.models)?,
        enums: encode_enums(&plan.enums)?,
        zip_carriers: encode_zip_carriers(&plan.zip_carriers)?,
        file: plan.span.file.as_str().to_owned(),
        line: plan.span.line,
        column: plan.span.column,
    };
    let encoded = write_plan(stored)?;
    if decode_record_plan(&encoded)? != *plan {
        return Err("native-interface codec changed complete native plan facts".to_owned());
    }
    Ok(encoded)
}

/// Restore one exact bounded native plan without resolving names or lowering a body.
pub fn decode_record_plan(encoded: &[u8]) -> Result<Plan, String> {
    let stored = read_plan(encoded)?;
    if stored.functions.is_empty() || stored.references.len() != stored.functions.len() {
        return Err("unsupported native-interface schema".to_owned());
    }
    let mut functions: Vec<Function> = Vec::with_capacity(stored.functions.len());
    for (product, references) in stored.functions.iter().zip(&stored.references) {
        let (function, _demand) = decode_product(product, references)?;
        functions.push(function);
    }
    let models = decode_models(&stored.models)?;
    let enums = decode_enums(&stored.enums)?;
    let externals = decode_runtime(&stored.runtime)?;
    let zip_carriers = decode_zip_carriers(&stored.zip_carriers)?;
    let mut plan = Plan::new(
        enums,
        models,
        functions,
        externals,
        source_span(&stored.file, stored.line, stored.column),
    );
    plan.programs = stored.programs;
    plan.zip_carriers = zip_carriers;
    Ok(plan)
}

/// Expose the whole interface's ordered function contributions for byte-equivalence checks.
pub fn record_plan_products(encoded: &[u8]) -> Result<Vec<Vec<u8>>, String> {
    Ok(read_plan(encoded)?.functions)
}

/// Name each referenced interface declaration by its native declaration name.
fn interface_nominals(plan: &Plan, references: &[NominalReference]) -> Result<Vec<ProductNominal>, String> {
    let mut nominals = Vec::with_capacity(references.len());
    for reference in references {
        let position = usize::try_from(reference.index).ok();
        let name = if reference.kind == NominalKind::Model {
            position
                .and_then(|index| plan.models.get(index))
                .map(|model| model.name.clone())
                .ok_or_else(|| "native-interface function names a missing model".to_owned())?
        } else {
            position
                .and_then(|index| plan.enums.get(index))
                .map(|declaration| declaration.name.clone())
                .ok_or_else(|| "native-interface function names a missing enum".to_owned())?
        };
        nominals.push(ProductNominal {
            kind: reference.kind,
            identity: name.clone(),
            digest: name,
        });
    }
    Ok(nominals)
}

/// Keep table order and each element's exact bytes of UTF-8 text.
fn text_list(values: &[String]) -> Wire {
    Wire::List(values.iter().map(|value| Wire::Text(value.clone())).collect())
}

/// Read one ordered textual table without coercing or dropping a field.
fn read_text_list(value: &Wire) -> Result<Vec<String>, String> {
    items(value)?.iter().map(text).collect()
}

/// Keep each binary table entry in order.
fn binary_list(values: Vec<Vec<u8>>) -> Wire {
    Wire::List(values.into_iter().map(Wire::Binary).collect())
}

/// Read one ordered binary table, refusing any other element kind.
fn read_binary_list(value: &Wire) -> Result<Vec<Vec<u8>>, String> {
    items(value)?
        .iter()
        .map(|item| match item {
            Wire::Binary(data) => Ok(data.clone()),
            _ => Err("native plan table requires a typed Crumb payload".to_owned()),
        })
        .collect()
}

/// Serialize each independently typed native table with its full ordinal and position information.
fn write_plan(plan: StoredPlan) -> Result<Vec<u8>, String> {
    let groups = plan
        .references
        .iter()
        .map(|references| {
            Wire::List(
                references
                    .iter()
                    .map(|reference| {
                        Wire::Tuple(vec![
                            Wire::Text(reference.kind.value().to_owned()),
                            Wire::integer(reference.index),
                            Wire::Text(reference.name.clone()),
                        ])
                    })
                    .collect(),
            )
        })
        .collect();
    encode_graph_row(
        PLAN_ROOT,
        &NATIVE_PLAN_SCHEMA,
        vec![
            required(b"enums_", binary_list(plan.enums)),
            required(b"functs", binary_list(plan.functions)),
            required(b"models", binary_list(plan.models)),
            required(b"progra", text_list(&plan.programs)),
            required(b"refers", Wire::List(groups)),
            required(b"runtim", binary_list(plan.runtime)),
            required(
                b"span__",
                Wire::Tuple(vec![
                    Wire::Text(plan.file),
                    Wire::integer(plan.line),
                    Wire::integer(plan.column),
                ]),
            ),
            required(b"zipcar", binary_list(plan.zip_carriers)),
        ],
    )
}

/// Admit the whole interface before selecting carriers, functions and declarations by their original positions.
fn read_plan(encoded: &[u8]) -> Result<StoredPlan, String> {
    let fields = decode_graph_row(encoded, PLAN_ROOT, &NATIVE_PLAN_SCHEMA, 8)?;
    let mut references = Vec::new();
    for group in items(&fields[4].value)? {
        let mut members = Vec::new();
        for member in items(group)? {
            let part = tuple_items(member, 3)?;
            let kind = match text(&part[0])?.as_str() {
                "model" => NominalKind::Model,
                "enum" => NominalKind::Enum,
                _ => return Err("invalid native interface nominal kind".to_owned()),
            };
            members.push(NominalReference {
                kind,
                index: number(&part[1])?,
                name: text(&part[2])?,
            });
        }
        references.push(members);
    }
    let span = tuple_items(&fields[6].value, 3)?;
    Ok(StoredPlan {
        enums: read_binary_list(&fields[0].value)?,
        functions: read_binary_list(&fields[1].value)?,
        models: read_binary_list(&fields[2].value)?,
        programs: read_text_list(&fields[3].value)?,
        references,
        runtime: read_binary_list(&fields[5].value)?,
        file: text(&span[0])?,
        line: number(&span[1])?,
        column: number(&span[2])?,
        zip_carriers: read_binary_list(&fields[7].value)?,
    })
}
