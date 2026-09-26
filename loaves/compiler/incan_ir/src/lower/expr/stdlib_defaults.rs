//! Stdlib parameter defaults expanded at a caller.
//!
//! A stdlib function's source declaration carries its defaults as written in the declaring module. A caller that
//! omits the argument receives that expression at its own call site, where a bare const spelling from the declaring
//! module names nothing (#1771). The stdlib loader records the canonical path of each such const; this module spells
//! the default through it.

use std::collections::HashMap;

use super::super::super::TypedExpr;
use super::super::super::expr::{IrExprKind, VarAccess, VarRefKind};
use super::super::super::types::IrType;
use super::super::AstLowering;
use super::super::errors::LoweringError;
use incan_frontend::ast;
use incan_frontend::symbols::ResolvedType;
use incan_lang::lang::stdlib;

/// The root of a crate-relative Rust path.
const CRATE_ROOT_SEGMENT: &str = "crate";

impl AstLowering {
    /// Lower one stdlib parameter default for expansion at a caller.
    ///
    /// A default that is exactly a const name the loader resolved is spelled through the const's canonical path; any
    /// other default lowers as written, which is correct for literals and every other shape that does not depend on
    /// the declaring module's bindings.
    pub(in crate::lower) fn lower_stdlib_param_default(
        &mut self,
        default: Option<&ast::Spanned<ast::Expr>>,
        default_const_paths: &HashMap<String, Vec<String>>,
    ) -> Result<Option<TypedExpr>, LoweringError> {
        if let Some(ast::Expr::Ident(name)) = default.map(|default| &default.node)
            && let Some(path) = default_const_paths.get(name)
            && let Some(qualified) = self.stdlib_const_path_expr(path)
        {
            return Ok(Some(qualified));
        }
        self.lower_param_default_expr(default)
    }

    /// Build a value expression naming the stdlib const at a canonical `std.*` path.
    ///
    /// A const owned by a compiled SDK provider is reached through the provider's crate, the same spelling a
    /// provider-owned default carries in its checked metadata. Otherwise the stdlib module is compiled into the
    /// current crate and the const is reached through the crate's own `__incan_std` module, where the emitter also
    /// spells the calls into that module. The expression is typed by the const's declared type in its `const`
    /// representation, so a `str` const stays a static string until a use site asks for an owned one.
    fn stdlib_const_path_expr(&mut self, path: &[String]) -> Option<TypedExpr> {
        let (name, module_path) = path.split_last()?;
        if module_path.first().map(String::as_str) != Some(stdlib::STDLIB_ROOT) {
            return None;
        }
        let mut expr = match self.sdk_provider_crate_for_module(module_path) {
            Some(provider_crate) => self.compiled_provider_path_expr(&provider_crate, path)?,
            None => Self::crate_stdlib_path_expr(path),
        };
        expr.ty = self
            .stdlib_cache
            .lookup_constant(module_path, name)
            .map(|constant| self.stdlib_const_ir_type(&constant.ty))
            .unwrap_or(IrType::Unknown);
        Some(expr)
    }

    /// Build `crate::__incan_std::<module>::<name>` for a stdlib path compiled into the current crate.
    fn crate_stdlib_path_expr(path: &[String]) -> TypedExpr {
        let root = TypedExpr::new(
            IrExprKind::Var {
                name: CRATE_ROOT_SEGMENT.to_string(),
                access: VarAccess::Read,
                ref_kind: VarRefKind::ExternalName,
            },
            IrType::Unknown,
        );
        std::iter::once(stdlib::INCAN_STD_NAMESPACE)
            .chain(
                path.iter()
                    .map(String::as_str)
                    .skip_while(|segment| *segment == stdlib::STDLIB_ROOT),
            )
            .fold(root, |object, segment| {
                TypedExpr::new(
                    IrExprKind::Field {
                        object: Box::new(object),
                        field: segment.to_string(),
                    },
                    IrType::Unknown,
                )
            })
    }

    /// Return the IR type a stdlib const of the given declared type has, mirroring the `const` annotation mapping.
    fn stdlib_const_ir_type(&self, ty: &ResolvedType) -> IrType {
        match ty {
            ResolvedType::Str => IrType::StaticStr,
            ResolvedType::Bytes => IrType::StaticBytes,
            other => self.lower_resolved_type(other),
        }
    }
}
