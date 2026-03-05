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
        // Newtypes auto-derive only Debug by default
        // This is conservative - external types (like Axum extractors) may not support Clone/PartialEq
        // Users can explicitly add derives if needed: @derive(Clone, PartialEq)
        let debug = derives::as_str(DeriveId::Debug).to_string();
        let mut derives = vec![debug];
        // Add Copy only if underlying type is Copy (int, float, bool)
        if underlying_ty.is_copy() {
            let copy = derives::as_str(DeriveId::Copy).to_string();
            let clone = derives::as_str(DeriveId::Clone).to_string();
            derives.push(clone);
            derives.push(copy);
        }
        // Note: Serialize/Deserialize derives for newtypes are added post-lowering by `add_serde_to_newtypes` in
        // codegen.rs, which selectively adds only the derives that are actually needed (Serialize, Deserialize, or
        // both).
        Ok(IrStruct {
            name: n.name.clone(),
            fields,
            derives,
            visibility: Self::map_visibility(n.visibility),
            type_params: Self::lower_type_params(&n.type_params),
            derive_rust_modules: std::collections::HashMap::new(),
        })
    }
}
