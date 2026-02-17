//! Newtype declaration lowering.

use super::super::super::decl::{IrStruct, StructField, Visibility};
use super::super::super::types::IrType;
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
        // Newtypes auto-derive Debug, Clone
        // Only add Copy if underlying type is Copy (int, float, bool)
        let debug = derives::as_str(DeriveId::Debug).to_string();
        let clone = derives::as_str(DeriveId::Clone).to_string();
        let partial_eq = derives::as_str(DeriveId::PartialEq).to_string();
        let eq = derives::as_str(DeriveId::Eq).to_string();
        let mut derives = vec![debug, clone, partial_eq];
        if !matches!(underlying_ty, IrType::Float) {
            derives.push(eq);
        }
        if underlying_ty.is_copy() {
            derives.push(derives::as_str(DeriveId::Copy).to_string());
        }
        // Note: Serialize/Deserialize derives for newtypes are added post-lowering by `add_serde_to_newtypes` in
        // codegen.rs, which selectively adds only the derives that are actually needed (Serialize, Deserialize, or
        // both).
        Ok(IrStruct {
            name: n.name.clone(),
            fields,
            derives,
            visibility: Self::map_visibility(n.visibility),
            type_params: vec![],
        })
    }
}
