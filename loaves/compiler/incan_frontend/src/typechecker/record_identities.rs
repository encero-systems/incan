//! Compiler-derived typed-record identities and declaration refusals (RFC 165).

use std::collections::HashMap;

use incan_lang::lang::conventions::{RECORD_MEMBER_IDENTITY_BYTES, STORE_RESERVED_NAME_PREFIX};
use sha2::{Digest, Sha256};

use crate::ast::{EnumDecl, ModelDecl, Span};
use crate::diagnostics::errors;

use super::TypeChecker;

type RecordMemberIdentity = [u8; RECORD_MEMBER_IDENTITY_BYTES];
const _: () = assert!(RECORD_MEMBER_IDENTITY_BYTES <= 32);

impl TypeChecker {
    /// Validate the store-reserved prefix and compiler-derived identities of active model fields.
    pub(super) fn validate_model_record_identities(&mut self, model: &ModelDecl) {
        let mut members_by_identity = HashMap::new();
        for field in &model.fields {
            if !self.member_binding_is_active(field.span) {
                continue;
            }
            let name = field.node.name.as_str();
            if name.starts_with(STORE_RESERVED_NAME_PREFIX) {
                self.errors.push(errors::store_reserved_model_field(name, field.span));
            }
            self.refuse_record_identity_collision(
                "model",
                &model.name,
                "field",
                &mut members_by_identity,
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
        name: &'name str,
        span: Span,
    ) {
        let identity = record_member_identity(name);
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

/// Compute the compiler-owned RFC 165 identity for one field or enum variant name.
fn record_member_identity(name: &str) -> RecordMemberIdentity {
    let digest = Sha256::digest(name.as_bytes());
    let mut identity = [0; RECORD_MEMBER_IDENTITY_BYTES];
    identity.copy_from_slice(&digest[..RECORD_MEMBER_IDENTITY_BYTES]);
    identity
}

#[cfg(test)]
mod tests {
    use super::record_member_identity;

    #[test]
    fn record_member_identities_use_the_registered_sha256_width() {
        assert_eq!(hex::encode(record_member_identity("customer")), "b6c45863875e");
        assert_eq!(
            record_member_identity("fieldbbea15ef9eff"),
            record_member_identity("field3d4624c2d3a2")
        );
    }
}
