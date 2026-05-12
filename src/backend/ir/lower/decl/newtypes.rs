//! Newtype declaration lowering.

use super::super::super::decl::{IrStruct, StructField, Visibility};
use super::super::AstLowering;
use super::super::errors::LoweringError;
use crate::frontend::ast;
use incan_core::lang::derives::{self, DeriveId};

impl AstLowering {
    /// Lower a newtype declaration to tuple struct.
    pub(in crate::backend::ir::lower) fn lower_newtype(
        &mut self,
        n: &ast::NewtypeDecl,
    ) -> Result<IrStruct, LoweringError> {
        // Newtype compiles to a tuple struct: struct UserId(i64);
        // Use "0" as the field name to trigger tuple struct emission
        let underlying_ty = self.lower_type(&n.underlying.node);
        let fields = vec![StructField {
            name: "0".to_string(),
            ty: underlying_ty.clone(),
            visibility: Visibility::Public,
            default: None,
            alias: None,
            description: None,
        }];

        // ---- Derives: auto-derive Debug (always), Copy/Clone for Copy types ----
        // Newtypes auto-derive only Debug by default; external types (e.g., Axum extractors) may not support
        // Clone/PartialEq, so we stay conservative.
        let debug = derives::as_str(DeriveId::Debug).to_string();
        let mut auto_derives = vec![debug];
        if underlying_ty.is_copy() {
            auto_derives.push(derives::as_str(DeriveId::Clone).to_string());
            auto_derives.push(derives::as_str(DeriveId::Copy).to_string());
        }

        // ---- Derives: user-specified via @derive(...) decorators ----
        let (mut user_derives, derive_rust_modules) = self.extract_derives(&n.decorators);
        self.extend_derives_with_adopted_serde_traits(&mut user_derives, &n.traits);

        // Merge: auto-derives first, then user derives (skip duplicates)
        let mut derives = auto_derives;
        for d in user_derives {
            if !derives.contains(&d) {
                derives.push(d);
            }
        }

        // Note: serde derives for newtypes are added post-lowering by `add_serde_to_newtypes` in codegen.rs, which
        // selectively adds only the derives that are actually needed.
        Ok(IrStruct {
            name: n.name.clone(),
            fields,
            derives,
            visibility: Self::map_visibility(n.visibility),
            type_params: Self::lower_type_params(&n.type_params),
            derive_rust_modules,
            lint_allows: self.extract_rust_lint_allows(&n.decorators),
        })
    }
}
