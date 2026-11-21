//! Checked generic result requirements: a substituted Result or Option must have provably independent owned storage.

use std::collections::BTreeMap;

use incan_semantics_core::body_ir::{Body, EnumDeclaration, NominalDeclaration, nominal_carrier_key};
use incan_semantics_core::{CanonicalSymbolId, IncanType, SemanticSourceTargetKind, SymbolOrigin};

use crate::decls::ViewLifetimeLayout;

/// Unresolved or dependent substitutions retain a refusal, never implicit independent storage.
pub fn validate_result_requirements(
    body: &Body,
    declarations: &[NominalDeclaration],
    enums: &[EnumDeclaration],
) -> Result<(), String> {
    let Some(profile) = &body.checked_input_profile else {
        return Ok(());
    };
    if profile.result_requirements.is_empty() {
        if profile.version == 3 || profile.version == 4 {
            return Err("generic result profile lost its dependency requirements".into());
        }
        return Ok(());
    }
    let mut layouts = ViewLifetimeLayout::start(declarations, enums);
    for (anchor, ty) in &profile.result_requirements {
        if !independent_result_with_layout(ty, &body.named_type_identities, declarations, enums, &mut layouts) {
            return Err(format!(
                "generic result needs retained dependency facts in {} at {}",
                body.name, anchor.start
            ));
        }
    }
    Ok(())
}

/// Prove one retained Result/Option requirement through exact declaration identities and owned storage only.
pub fn independent_result_with_layout(
    ty: &IncanType,
    identities: &BTreeMap<String, CanonicalSymbolId>,
    declarations: &[NominalDeclaration],
    enums: &[EnumDeclaration],
    layouts: &mut ViewLifetimeLayout,
) -> bool {
    let mut proof = Proof {
        declarations,
        enums,
        layouts,
        visiting: Vec::new(),
        complete: Vec::new(),
    };
    match ty {
        IncanType::Generic { base, args } if base == "Result" => {
            args.len() == 2 && args.iter().all(|argument| proof.payload(argument, identities))
        }
        IncanType::Generic { base, args } if base == "Option" => args.len() == 1 && proof.payload(&args[0], identities),
        _ => false,
    }
}

/// The declaration tables and the recursion state of one independence proof.
struct Proof<'a> {
    declarations: &'a [NominalDeclaration],
    enums: &'a [EnumDeclaration],
    layouts: &'a mut ViewLifetimeLayout,
    visiting: Vec<CanonicalSymbolId>,
    complete: Vec<CanonicalSymbolId>,
}

impl Proof<'_> {
    /// Accept the exhaustive owned payload grammar; every unlisted semantic type remains dependent.
    fn payload(&mut self, ty: &IncanType, identities: &BTreeMap<String, CanonicalSymbolId>) -> bool {
        match ty {
            IncanType::Primitive(_) => true,
            IncanType::Named(name) => match identities.get(name) {
                Some(identity) => self.named(name, identity),
                None => false,
            },
            IncanType::Generic { base, args } if base == "List" || base == "Option" => {
                args.len() == 1 && self.payload(&args[0], identities)
            }
            IncanType::Generic { base, args } if base == "Result" => {
                args.len() == 2 && args.iter().all(|argument| self.payload(argument, identities))
            }
            _ => false,
        }
    }

    /// Resolve one Named key to exactly one canonical source declaration and prove its recursive owned closure.
    fn named(&mut self, name: &str, identity: &CanonicalSymbolId) -> bool {
        if !source_nominal_identity(identity) {
            return false;
        }
        let mut canonical_matches = 0;
        let mut carrier_matches = 0;
        let mut nominal: Option<usize> = None;
        let mut enum_index: Option<usize> = None;
        for (index, declaration) in self.declarations.iter().enumerate() {
            let expected = canonical_carrier_name(&declaration.canonical, &declaration.name);
            if expected == name && declaration.carrier_name() == expected {
                carrier_matches += 1;
            }
            if &declaration.canonical == identity {
                canonical_matches += 1;
                nominal = Some(index);
            }
        }
        for (index, declaration) in self.enums.iter().enumerate() {
            let expected = canonical_carrier_name(&declaration.canonical, &declaration.name);
            if expected == name && declaration.carrier_name() == expected {
                carrier_matches += 1;
            }
            if &declaration.canonical == identity {
                canonical_matches += 1;
                enum_index = Some(index);
            }
        }
        if canonical_matches != 1 || carrier_matches != 1 {
            return false;
        }
        if !self.selected(name, nominal, enum_index) {
            return false;
        }
        // A cycle or cached success closes only after this occurrence's spelling and declaration were reauthenticated.
        if self.complete.contains(identity) || self.visiting.contains(identity) {
            return true;
        }
        self.visiting.push(identity.clone());
        let accepted = self.closure(nominal, enum_index);
        self.visiting.pop();
        if accepted {
            self.complete.push(identity.clone());
        }
        accepted
    }

    /// The selected declaration is a closed source model, class, newtype or enum carried under this exact spelling.
    fn selected(&self, name: &str, nominal: Option<usize>, enum_index: Option<usize>) -> bool {
        if let Some(index) = nominal {
            let declaration = &self.declarations[index];
            let expected = canonical_carrier_name(&declaration.canonical, &declaration.name);
            return expected == name
                && declaration.carrier_name() == expected
                && matches!(
                    declaration.canonical.kind,
                    SemanticSourceTargetKind::Model
                        | SemanticSourceTargetKind::Class
                        | SemanticSourceTargetKind::Newtype
                )
                && declaration.type_parameters.is_empty()
                && declaration.type_parameter_count == 0
                && declaration.native_type_arguments.is_empty();
        }
        if let Some(index) = enum_index {
            let declaration = &self.enums[index];
            let expected = canonical_carrier_name(&declaration.canonical, &declaration.name);
            return expected == name
                && declaration.carrier_name() == expected
                && declaration.canonical.kind == SemanticSourceTargetKind::Enum
                && declaration.type_parameters.is_empty()
                && declaration.native_type_arguments.is_empty();
        }
        false
    }

    /// Prove the selected declaration's complete zero-lifetime layout and every field's independence.
    fn closure(&mut self, nominal: Option<usize>, enum_index: Option<usize>) -> bool {
        let declarations = self.declarations;
        let enums = self.enums;
        if let Some(index) = nominal {
            let declaration = &declarations[index];
            if !self
                .layouts
                .selected_nominal_is_complete_zero(declaration, declarations, enums)
            {
                return false;
            }
            return declaration
                .field_types
                .iter()
                .all(|field| self.payload(field, &declaration.named_type_identities));
        }
        if let Some(index) = enum_index {
            let declaration = &enums[index];
            if !self
                .layouts
                .selected_enum_is_complete_zero(declaration, declarations, enums)
            {
                return false;
            }
            return declaration.variants.iter().all(|variant| {
                variant
                    .fields
                    .iter()
                    .all(|field| self.payload(field, &declaration.named_type_identities))
            });
        }
        false
    }
}

/// Use the identity-derived carrier whenever the declaration has one; source aliases never replace this key.
fn canonical_carrier_name(identity: &CanonicalSymbolId, fallback: &str) -> String {
    nominal_carrier_key(identity).unwrap_or_else(|| fallback.to_string())
}

/// Admit only authored source nominal identities; a Rust/native/builtin identity never selects source storage.
fn source_nominal_identity(identity: &CanonicalSymbolId) -> bool {
    if !matches!(
        identity.kind,
        SemanticSourceTargetKind::Model
            | SemanticSourceTargetKind::Class
            | SemanticSourceTargetKind::Newtype
            | SemanticSourceTargetKind::Enum
    ) {
        return false;
    }
    matches!(identity.origin, SymbolOrigin::Module(_) | SymbolOrigin::Package { .. })
}
