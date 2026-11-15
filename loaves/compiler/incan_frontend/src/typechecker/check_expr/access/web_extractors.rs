//! Preserve extractor forwarding as checked native tuple and source payload field identities.

use super::TypeChecker;
use crate::ast::{Expr, Span, Spanned};
use crate::symbols::ResolvedType;
use incan_lang::interop::RustItemKind;
use incan_lang::lang::surface::types::SurfaceTypeId;
use incan_semantics_core::{SemanticSourceTargetKind, body_ir};

impl TypeChecker {
    /// Select a wrapper only through its active standard import's original native identity and concrete payload.
    /// Equal-spelled source declarations and unrelated one-field Rust structs grant no forwarding operation.
    fn checked_extractor_type(&self, source: &ResolvedType) -> Option<(String, ResolvedType)> {
        let path = self.rust_public_type_path(&self.rust_canonical_path_for_receiver_type(source)?);
        let selected = [SurfaceTypeId::Json, SurfaceTypeId::Query].into_iter().any(|surface| {
            self.surface_type_identity(surface)
                .and_then(|identity| body_ir::native_nominal_item(&identity))
                .is_some_and(|owner| self.rust_public_type_path(&owner) == path)
        });
        if !selected {
            return None;
        }
        let arguments = match source {
            ResolvedType::RustPath(display) => Self::rust_generic_base_and_args(display)?
                .1
                .into_iter()
                .filter(|argument| !Self::rust_generic_arg_is_lifetime(argument))
                .map(|argument| self.native_rust_signature_type(argument, &path))
                .collect(),
            ResolvedType::Generic(_, arguments) => arguments.clone(),
            _ => return None,
        };
        let [payload] = arguments.as_slice() else {
            return None;
        };
        Some((path, payload.clone()))
    }

    /// Retain the inspected public tuple slot before the already checked payload member, or the slot alone for value.
    /// The accepting checker owns this path; Body IR neither recognizes wrapper names nor reconstructs native layout.
    pub(super) fn resolve_checked_extractor_field(
        &mut self,
        base: &Spanned<Expr>,
        source: &ResolvedType,
        field: &str,
        span: Span,
    ) -> Option<ResolvedType> {
        let (path, payload) = self.checked_extractor_type(source)?;
        let metadata = self.rust_item_metadata_for_complete_type(&path)?;
        let RustItemKind::Type(info) = &metadata.kind else {
            return None;
        };
        let [slot] = info.fields.as_slice() else {
            return None;
        };
        if slot.name != "0" {
            return None;
        }
        self.retain_rust_layout(&path, info);
        let owner = body_ir::rust_item_identity(&path)?;
        let slot = Self::synthetic_member_identity(&owner, "0", SemanticSourceTargetKind::Field);
        let mut projections = vec![slot.clone()];
        let result = if field == "value" {
            self.type_info.record_resolved_identity(span, slot);
            payload
        } else {
            let result = self.check_nominal_field(base, &payload, field, span);
            projections.push(self.type_info.resolved_identity(span)?.clone());
            result
        };
        self.type_info.rust.field_projection_paths.insert(span, projections);
        Some(result)
    }
}
