//! Newtype declaration lowering.

use super::super::super::decl::{IrStruct, IrStructKind, StructField, Visibility};
use super::super::AstLowering;
use super::super::errors::LoweringError;
use incan_frontend::ast;
use incan_lang::lang::derives::{self, DeriveId};

impl AstLowering {
    /// Return the derives a newtype gets automatically, before `Copy`: `Debug` and `Clone` as the typechecker recorded
    /// them from the underlying type (#1754).
    ///
    /// The typechecker's derive relation gives a newtype `Clone` when its underlying type implements it and `Debug`
    /// unless its underlying type is known to lack it; lowering spells exactly that, so a newtype implements what the
    /// checker assumed of it. Without a checked record (a declaration lowered without its module's typecheck facts)
    /// only `Debug` is derived, the conservative spelling used before the relation existed.
    fn newtype_automatic_derives(&self, name: &str) -> Vec<String> {
        self.type_info
            .as_ref()
            .and_then(|info| info.declarations.newtype_construction.get(name))
            .map(|checked| checked.automatic_derives.clone())
            .unwrap_or_else(|| vec![derives::as_str(DeriveId::Debug).to_string()])
    }

    /// Lower a newtype declaration to tuple struct.
    pub(in crate::lower) fn lower_newtype(&mut self, n: &ast::NewtypeDecl) -> Result<IrStruct, LoweringError> {
        // Newtype compiles to a tuple struct: struct UserId(i64);
        // Use "0" as the field name to trigger tuple struct emission
        let underlying_ty = self.lower_type(&n.underlying.node);
        let fields = vec![StructField {
            name: "0".to_string(),
            ty: underlying_ty.clone(),
            surface_type_name: None,
            visibility: Visibility::Public,
            is_type_private: false,
            default: None,
            alias: None,
            description: None,
        }];

        // ---- Derives: automatic Debug and Clone as the typechecker decided them, Copy for Copy types ----
        let mut auto_derives = self.newtype_automatic_derives(&n.name);
        let clone = derives::as_str(DeriveId::Clone).to_string();
        if underlying_ty.is_copy() {
            if !auto_derives.contains(&clone) {
                auto_derives.push(clone);
            }
            auto_derives.push(derives::as_str(DeriveId::Copy).to_string());
        }

        // ---- Derives: user-specified via @derive(...) decorators ----
        let (mut user_derives, derive_rust_modules) = self.extract_derives(&n.decorators);
        self.extend_derives_with_adopted_serde_traits(&mut user_derives, &n.traits);

        // Merge: auto-derives first, then user derives, skipping one that names a derive already present under another
        // spelling (`Clone` and `@rust.derive("std::clone::Clone")` are the same derive, and naming it twice is E0119).
        let mut derives = auto_derives;
        for d in user_derives {
            if !derives.iter().any(|existing| Self::same_derive(existing, &d)) {
                derives.push(d);
            }
        }

        // Note: serde derives for newtypes are added post-lowering by `add_serde_to_newtypes` in codegen.rs, which
        // selectively adds only the derives that are actually needed.
        let type_params = self.lower_type_params(&n.type_params);
        let phantom_type_params = Self::phantom_type_params(&type_params, &fields);
        Ok(IrStruct {
            kind: IrStructKind::Newtype,
            name: n.name.clone(),
            docstring: n.docstring.clone(),
            fields,
            derives,
            visibility: self.map_type_visibility(n.visibility),
            type_params,
            phantom_type_params,
            derive_rust_modules,
            lint_allows: self.extract_rust_lint_allows(&n.decorators),
        })
    }
}
