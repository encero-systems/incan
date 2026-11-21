//! Bind keyed native products to source declarations and closed specializations through their checked versions.

use incan_mir_plan_native::{
    EnumDeclaration, ExternalFunction, Function, NominalKind, NominalReference, ProductNominal, UnitNominals,
    describe_nominals, encode_checked_layout, product_nominals, unit_nominals,
};
use incan_semantics_core::CanonicalSymbolId;
use incan_semantics_core::body_ir::StaticDeclaration;
use incan_semantics_core::semantic_digest::{enum_layout_witness, model_layout_witness, semantic_digest};
use incan_semantics_core::stable_identity::StableDeclarationId;
use std::sync::Arc;

use sha2::{Digest, Sha256};

use crate::program_context::ProgramContext;
use crate::scalar_store::retain_scalar;
use crate::symbols::model_symbol;

/// The described rows of a program's model, enum and static tables, each row for the declaration at its position.
///
/// A program's tables only grow within one planning (`retain_nominal_tables`, `extend_program_context`), so a row
/// stays the description of its position, and a table described again describes only the rows it gained.
#[derive(Clone, Default)]
pub struct DescribedNominals {
    models: Vec<(ProductNominal, String)>,
    enums: Vec<(ProductNominal, String)>,
    statics: Vec<ProductNominal>,
}

/// How many model, enum and static rows of the program are described.
pub fn described_rows(program: &ProgramContext) -> (usize, usize, usize) {
    let described = &program.described;
    (described.models.len(), described.enums.len(), described.statics.len())
}

/// Describe the unit's tables only for a request that reads or retains plan products.
pub fn product_nominal_table(
    program: &mut ProgramContext,
    intrinsics: &[EnumDeclaration],
    products: Option<&str>,
) -> UnitNominals {
    if products.is_none() {
        return unit_nominals(Vec::new(), Vec::new(), Vec::new(), Vec::new());
    }
    unit_product_nominals(program, intrinsics)
}

/// Describe every model and enum position of the program's tables, then each intrinsic carrier at its carrier position.
///
/// A declaration without a stable identity is described with an empty identity, which a product refuses to name.
pub fn unit_product_nominals(program: &mut ProgramContext, intrinsics: &[EnumDeclaration]) -> UnitNominals {
    describe_new_rows(program);
    let described = &program.described;
    let (models, model_names) = described.models.iter().cloned().unzip();
    let (enums, enum_names) = described.enums.iter().cloned().unzip();
    let mut unit = unit_nominals(models, enums, model_names, enum_names).with_statics(described.statics.clone());
    sync_carriers(&mut unit, intrinsics);
    unit
}

/// Describe the rows each of the program's tables gained since it was last described. Rows beyond a table's length
/// cannot occur within one planning; such a table is described afresh.
fn describe_new_rows(program: &mut ProgramContext) {
    let (nominals, enums, statics) = (program.nominals.clone(), program.enums.clone(), program.statics.clone());
    let current = &program.described;
    if current.models.len() == nominals.len()
        && current.enums.len() == enums.len()
        && current.statics.len() == statics.len()
    {
        return;
    }
    let described = Arc::make_mut(&mut program.described);
    if described.models.len() > nominals.len()
        || described.enums.len() > enums.len()
        || described.statics.len() > statics.len()
    {
        *described = DescribedNominals::default();
    }
    for declaration in &nominals[described.models.len()..] {
        let witness = model_layout_witness(declaration).map_err(|error| error.to_string());
        let row = source_nominal(NominalKind::Model, &declaration.canonical, witness);
        described.models.push((row, model_symbol(&declaration.name)));
    }
    for declaration in &enums[described.enums.len()..] {
        let carrier = declaration.carrier_name();
        let witness = enum_layout_witness(declaration).map_err(|error| error.to_string());
        let row = source_nominal(NominalKind::Enum, &declaration.canonical, witness);
        described.enums.push((row, model_symbol(&carrier)));
    }
    let start = described.statics.len();
    described.statics.extend(statics[start..].iter().map(static_nominal));
}

/// Describe one static cell by its stable declaration identity and the digest of its whole checked declaration: its
/// type and its initializer decide how a reader loads it and whether the reader first initializes its module.
fn static_nominal(declaration: &StaticDeclaration) -> ProductNominal {
    let identity = StableDeclarationId::from_canonical(&declaration.canonical, None, None);
    match (identity, semantic_digest(declaration)) {
        (Some(stable), Ok(digest)) => ProductNominal {
            kind: NominalKind::Static,
            identity: format!("static:{}", stable.render_compact()),
            digest,
        },
        _ => ProductNominal {
            kind: NominalKind::Static,
            identity: String::new(),
            digest: String::new(),
        },
    }
}

/// Describe intrinsic carriers lowering admitted since the table was described, each at its carrier position.
pub fn sync_carriers(unit: &mut UnitNominals, intrinsics: &[EnumDeclaration]) {
    while unit.carriers.len() < intrinsics.len() {
        let declaration = &intrinsics[unit.carriers.len()];
        unit.append_carrier(carrier_nominal(declaration), declaration.name.clone());
    }
}

/// Retain a fresh product, with the helper frames and generic instance requests its lowering made, naming each model
/// and enum it uses by declaration; return their current positions.
#[allow(clippy::too_many_arguments)]
pub fn retain_named_scalar(
    directory: &str,
    inputs: &[String],
    function: &Function,
    runtime: &[ExternalFunction],
    unit: &UnitNominals,
    positions: &[(i64, i64)],
    frames: &[Function],
    requests: &[Vec<u8>],
) -> Result<Vec<NominalReference>, String> {
    let references = product_nominals(function, runtime, frames)?;
    let nominals = describe_nominals(&references, unit)?;
    retain_scalar(
        directory, inputs, function, runtime, &nominals, positions, frames, requests,
    )?;
    Ok(references)
}

/// Combine the existing stable declaration identity with its closed specialization and complete checked version.
fn source_nominal(
    kind: NominalKind,
    canonical: &CanonicalSymbolId,
    witness: Result<(String, String), String>,
) -> ProductNominal {
    let unnamed = ProductNominal {
        kind,
        identity: String::new(),
        digest: String::new(),
    };
    let Some(stable) = StableDeclarationId::from_canonical(canonical, None, None) else {
        return unnamed;
    };
    let Ok((specialization, version)) = witness else {
        return unnamed;
    };
    ProductNominal {
        kind,
        identity: format!("{}#{specialization}", stable.render_compact()),
        digest: version,
    }
}

/// Name a standard carrier by the checked type it lays out; its variants and derives are its checked meaning.
///
/// The native name numbers carriers in admission order, which another module's types change, so it never names one.
/// A carrier without an encodable checked type stays unnamed, and a product refuses to name it.
pub fn carrier_nominal(declaration: &EnumDeclaration) -> ProductNominal {
    let unnamed = ProductNominal {
        kind: NominalKind::Enum,
        identity: String::new(),
        digest: String::new(),
    };
    let Ok(text) = carrier_type_text(declaration) else {
        return unnamed;
    };
    let mut meaning = vec![text.clone()];
    for variant in &declaration.variants {
        meaning.push(format!("{}/{}", variant.name, variant.fields.len()));
    }
    meaning.extend(declaration.derives.iter().cloned());
    match semantic_digest(&meaning) {
        Ok(digest) => ProductNominal {
            kind: NominalKind::Enum,
            identity: format!("carrier#{text}"),
            digest,
        },
        Err(_) => unnamed,
    }
}

/// Encode the exact checked type a carrier lays out; its nominal names carry no source span.
fn carrier_type_text(declaration: &EnumDeclaration) -> Result<String, String> {
    let Some(key) = &declaration.semantic_key else {
        return Err("carrier has no checked type".into());
    };
    let stored = encode_checked_layout(key)?;
    Ok(format!("sha256:{}", hex::encode(Sha256::digest(&stored))))
}
