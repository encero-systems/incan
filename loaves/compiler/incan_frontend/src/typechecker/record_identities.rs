//! Compiler-derived typed-record identities and declaration refusals (RFC 165).

use std::collections::HashMap;

use incan_lang::lang::conventions::STORE_RESERVED_NAME_PREFIX;
use incan_semantics_core::record_identity::{RecordMemberIdentity, record_member_identity};

use crate::ast::{EnumDecl, ModelDecl, Span};
use crate::diagnostics::errors;

use super::TypeChecker;

impl TypeChecker {
    /// Validate the store-reserved prefix and compiler-derived identities of active model fields.
    pub(super) fn validate_model_record_identities(&mut self, model: &ModelDecl) {
        let mut members_by_identity = HashMap::new();
        for field in &model.fields {
            if !self.member_binding_is_active(field.span) {
                continue;
            }
            let name = field.node.name.as_str();
            let identity_name = field
                .node
                .metadata
                .renamed_from
                .first()
                .map_or(name, |earlier| earlier.node.as_str());
            if name.starts_with(STORE_RESERVED_NAME_PREFIX) {
                self.errors.push(errors::store_reserved_model_field(name, field.span));
            }
            self.refuse_record_identity_collision(
                "model",
                &model.name,
                "field",
                &mut members_by_identity,
                identity_name,
                name,
                field.span,
            );
        }
    }

    /// Validate compiler-derived identities of active enum variants.
    pub(super) fn validate_enum_record_identities(&mut self, en: &EnumDecl) {
        let mut members_by_identity = HashMap::new();
        for variant in &en.variants {
            if !self.member_binding_is_active(variant.span) {
                continue;
            }
            self.refuse_record_identity_collision(
                "enum",
                &en.name,
                "variant",
                &mut members_by_identity,
                &variant.node.name,
                &variant.node.name,
                variant.span,
            );
        }
    }

    /// Record one member identity, refusing it when a distinct earlier member already owns that identity.
    fn refuse_record_identity_collision<'name>(
        &mut self,
        owner_kind: &str,
        owner_name: &str,
        member_kind: &str,
        members_by_identity: &mut HashMap<RecordMemberIdentity, (&'name str, Span)>,
        identity_name: &str,
        name: &'name str,
        span: Span,
    ) {
        let identity = record_member_identity(identity_name);
        if let Some((first_name, first_span)) = members_by_identity.get(&identity)
            && *first_name != name
        {
            self.errors.push(errors::record_member_identity_collision(
                owner_kind,
                owner_name,
                member_kind,
                first_name,
                *first_span,
                name,
                span,
                &hex::encode(identity),
            ));
            return;
        }
        members_by_identity.insert(identity, (name, span));
    }
}

#[cfg(test)]
mod tests {
    use crate::{lexer, parser};

    use super::TypeChecker;

    #[test]
    fn renamed_field_collisions_use_the_earliest_name() -> Result<(), String> {
        let source = r#"
model Collision:
    current [renamed_from="fieldbbea15ef9eff"]: int
    field3d4624c2d3a2: int
"#;
        let tokens = lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
        let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
        let errors = TypeChecker::new()
            .check_program(&program)
            .err()
            .ok_or("colliding earliest names were accepted")?;
        assert!(errors.iter().any(|error| error.stable_code() == Some("INCAN-T0128")));
        Ok(())
    }
}
