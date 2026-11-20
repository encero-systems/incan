//! A carrier keeps its lowering position when source enums are appended, and publication folds it into the plan.

use super::support::{block, local, returning};
use crate::carriers::fold_carrier_positions;
use crate::nominal::{
    NominalKind, NominalReference, ProductNominal, carrier_index, describe_nominals, resolve_nominals, unit_nominals,
};
use crate::plan::{Carrier, EnumDeclaration, EnumVariant, Function, Plan};
use crate::record_types::{CanonicalSymbolId, DeclarationKind, HirSourceSpan, SymbolNamespace, SymbolOrigin};
use crate::types::{PlanType, SourceSpan, source_span};

/// A described enum or carrier declaration.
fn nominal(identity: &str) -> ProductNominal {
    ProductNominal {
        kind: NominalKind::Enum,
        identity: identity.to_owned(),
        digest: format!("{identity}-digest"),
    }
}

/// An enum declaration with the given variants.
fn declaration(name: &str, variants: Vec<EnumVariant>, derives: &[&str], span: &SourceSpan) -> EnumDeclaration {
    EnumDeclaration::new(
        name.to_owned(),
        false,
        variants,
        derives.iter().map(|derive| (*derive).to_owned()).collect(),
        span.clone(),
    )
}

/// A variant with the given payload.
fn variant(name: &str, fields: Vec<PlanType>) -> EnumVariant {
    EnumVariant {
        name: name.to_owned(),
        fields,
    }
}

/// A unit whose second lowering attempt holds one more source enum names its carrier at the same position.
///
/// 1. With one source enum, the admitted carrier is at its carrier position.
/// 2. With a second source enum, as a module resolved later adds, the carrier stays there and resolves there.
#[test]
fn an_appended_source_enum_moves_no_carrier() -> Result<(), String> {
    let carrier = nominal("carrier#Option[int]");
    let mut before = unit_nominals(
        Vec::new(),
        vec![nominal("enum#Tone")],
        Vec::new(),
        vec!["Tone".to_owned()],
    );
    before.append_carrier(carrier.clone(), "__IncanCarrier0".to_owned());
    let mut after = unit_nominals(
        Vec::new(),
        vec![nominal("enum#Tone"), nominal("enum#Pace")],
        Vec::new(),
        vec!["Tone".to_owned(), "Pace".to_owned()],
    );
    after.append_carrier(carrier.clone(), "__IncanCarrier0".to_owned());
    let resolved = resolve_nominals(std::slice::from_ref(&carrier), &before).ok_or("the carrier was not described")?;
    assert!(
        resolved[0].index == carrier_index(0),
        "carrier at {}",
        resolved[0].index
    );
    let moved = resolve_nominals(std::slice::from_ref(&carrier), &after)
        .ok_or("an appended source enum made the carrier a miss")?;
    assert!(
        moved[0].index == carrier_index(0),
        "an appended source enum moved the carrier"
    );
    let reference = NominalReference {
        kind: NominalKind::Enum,
        index: carrier_index(0),
        name: "__IncanCarrier0".to_owned(),
    };
    let named = describe_nominals(&[reference], &after)?;
    assert!(named == [carrier]);
    Ok(())
}

/// A frame naming a carrier by its lowering position names its plan position after the publication fold.
#[test]
fn publication_folds_each_carrier_after_the_source_enums() -> Result<(), String> {
    let span = source_span("carrier_positions.incn", 1, 1);
    let carrier = carrier_index(0);
    let function = Function::new(
        "main".to_owned(),
        Vec::new(),
        PlanType::Enum(carrier, "__IncanCarrier0".to_owned()),
        vec![
            local(PlanType::Enum(carrier, "__IncanCarrier0".to_owned()), &span),
            local(PlanType::Enum(0, "Tone".to_owned()), &span),
        ],
        vec![block(Vec::new(), returning(&span), &span)],
        span.clone(),
    );
    let tone = declaration("Tone", vec![variant("Low", Vec::new())], &[], &span);
    let mut option = declaration(
        "__IncanCarrier0",
        vec![
            variant("None", Vec::new()),
            variant("Some", vec![PlanType::Enum(0, "Tone".to_owned())]),
        ],
        &["Clone"],
        &span,
    );
    option.carrier = Carrier::Option;
    let plan = Plan::new(vec![tone, option], Vec::new(), vec![function], Vec::new(), span);
    let folded = fold_carrier_positions(&plan, 1)?;
    assert!(folded.functions[0].return_type == PlanType::Enum(1, "__IncanCarrier0".to_owned()));
    assert!(folded.functions[0].locals[0].ty == PlanType::Enum(1, "__IncanCarrier0".to_owned()));
    assert!(
        folded.functions[0].locals[1].ty == PlanType::Enum(0, "Tone".to_owned()),
        "a source enum position must be kept"
    );
    assert!(folded.enums[1].variants[1].fields == [PlanType::Enum(0, "Tone".to_owned())]);
    // Folding a published plan again changes nothing.
    assert!(fold_carrier_positions(&folded, 1)? == folded);
    Ok(())
}

/// A frame holding a source trait's type token still folds: a trait type names no carrier position.
#[test]
fn publication_keeps_a_trait_type_a_frame_names() -> Result<(), String> {
    let span = source_span("carrier_positions.incn", 1, 1);
    let readable = PlanType::Trait(CanonicalSymbolId {
        namespace: SymbolNamespace::OrdinaryLexical,
        origin: SymbolOrigin::Module(vec!["contracts".to_owned()]),
        declaration_name: "Readable".to_owned(),
        kind: DeclarationKind::Trait,
        scope_discriminant: None,
        declaration_span: HirSourceSpan {
            file: "src/contracts.incn".to_owned(),
            start: 4,
            end: 12,
            expansion: None,
        },
    });
    let token = PlanType::TypeToken(vec![readable]);
    let carrier = carrier_index(0);
    let function = Function::new(
        "main".to_owned(),
        Vec::new(),
        PlanType::Unit,
        vec![
            local(PlanType::Unit, &span),
            local(token.clone(), &span),
            local(PlanType::Enum(carrier, "__IncanCarrier0".to_owned()), &span),
        ],
        vec![block(Vec::new(), returning(&span), &span)],
        span.clone(),
    );
    let mut option = declaration(
        "__IncanCarrier0",
        vec![variant("None", Vec::new()), variant("Some", vec![PlanType::Int])],
        &["Clone"],
        &span,
    );
    option.carrier = Carrier::Option;
    let plan = Plan::new(vec![option], Vec::new(), vec![function], Vec::new(), span);
    let folded = fold_carrier_positions(&plan, 0)?;
    assert!(
        folded.functions[0].locals[1].ty == token,
        "the fold changed a trait's type token"
    );
    assert!(folded.functions[0].locals[2].ty == PlanType::Enum(0, "__IncanCarrier0".to_owned()));
    Ok(())
}
