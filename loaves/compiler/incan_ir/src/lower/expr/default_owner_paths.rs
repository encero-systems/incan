//! Names in parameter defaults, spelled through the modules that declare them.
//!
//! A caller that omits an argument receives the parameter's default at its own call site, which can be in another
//! module of the crate. A name in the default means what it means in the module that declares the callable (#1771),
//! and two modules can each declare the name. The checked identity of each name records the module that declares it,
//! also when the name was imported, so a const the default reads is spelled as a crate path to that module and a
//! function the default calls carries that module's path as its canonical callee path.

use std::collections::HashMap;

use super::super::super::TypedExpr;
use super::super::super::expr::{IrExprKind, VarRefKind};
use super::super::AstLowering;
use super::calls::canonical_path_naming_selected_overload;
use incan_frontend::ast;
use incan_semantics_core::{SemanticSourceTargetKind, SymbolOrigin};

impl AstLowering {
    /// Record the Rust module path below the crate root of each source module compiled into this crate.
    ///
    /// The map is keyed by the origin the checked identities of a module's declarations carry; the crate-root module
    /// maps to the empty path.
    pub fn set_source_module_rust_paths(&mut self, paths: HashMap<SymbolOrigin, Vec<String>>) {
        self.source_module_rust_paths = paths;
    }

    /// Record the Rust module path below the crate root of one more source module compiled into this crate.
    pub fn add_source_module_rust_path(&mut self, origin: SymbolOrigin, rust_path: Vec<String>) {
        self.source_module_rust_paths.insert(origin, rust_path);
    }

    /// Spell a const that a parameter default reads as a crate path to the module that declares it.
    ///
    /// `lowered` is the reference as lowered at `span`; the path keeps its type. Returns `None` outside a source
    /// parameter default and for every name that is not a module-level const of a module compiled into this crate.
    pub(in crate::lower) fn default_owner_const_path(
        &self,
        name: &str,
        span: ast::Span,
        lowered: &TypedExpr,
    ) -> Option<TypedExpr> {
        if !matches!(
            lowered.kind,
            IrExprKind::Var {
                ref_kind: VarRefKind::Value,
                ..
            }
        ) {
            return None;
        }
        let (module_path, declaration_name) =
            self.default_name_owner(name, span, &[SemanticSourceTargetKind::Const])?;
        let mut spelled = Self::crate_path_expr(
            module_path
                .iter()
                .map(String::as_str)
                .chain(std::iter::once(declaration_name.as_str())),
        );
        spelled.ty = lowered.ty.clone();
        Some(spelled)
    }

    /// Return the canonical path of a function that a parameter default calls, through the module that declares it.
    ///
    /// The path names the overload the call selected, as an imported callee's path does. A function of the crate-root
    /// module has no module path to spell, so its call keeps the callee as written.
    pub(in crate::lower) fn default_owner_callee_path(
        &self,
        callee: &ast::Spanned<ast::Expr>,
        selected_overload_name: Option<&str>,
    ) -> Option<Vec<String>> {
        let ast::Expr::Ident(name) = &callee.node else {
            return None;
        };
        let (mut path, declaration_name) = self.default_name_owner(
            name,
            callee.span,
            &[SemanticSourceTargetKind::Function, SemanticSourceTargetKind::Partial],
        )?;
        if path.is_empty() {
            return None;
        }
        path.push(declaration_name);
        Some(canonical_path_naming_selected_overload(path, selected_overload_name))
    }

    /// Resolve the module-level name at `span` in a source parameter default to its declaring module's Rust path and
    /// its declared name.
    ///
    /// The name must be bound at module level where the default is written, by a declaration of that module or by an
    /// import, and its checked identity must be one of `kinds` and belong to a module compiled into this crate.
    fn default_name_owner(
        &self,
        name: &str,
        span: ast::Span,
        kinds: &[SemanticSourceTargetKind],
    ) -> Option<(Vec<String>, String)> {
        if self.param_default_depth == 0
            || self.sdk_provider_build
            || self.active_imported_trait_defaults.last().copied().unwrap_or(false)
        {
            return None;
        }
        let info = self.type_info.as_ref()?;
        let identity = info.resolved_identity(span)?;
        if !kinds.contains(&identity.kind) || identity.scope_discriminant.is_some() {
            return None;
        }
        let imported = self.import_aliases.contains_key(name) || info.import_binding_path(name).is_some();
        if !imported && identity.declaration_name != name {
            return None;
        }
        let module_path = self.source_module_rust_paths.get(&identity.origin)?;
        Some((module_path.clone(), identity.declaration_name.clone()))
    }
}
