//! Model declaration lowering.

use super::super::super::decl::{IrStruct, IrStructKind, StructField};
use super::super::AstLowering;
use super::super::errors::LoweringError;
use incan_frontend::ast;
use incan_lang::lang::derives::{self, DeriveId};

impl AstLowering {
    /// Lower a model declaration to struct.
    pub(in crate::lower) fn lower_model(&mut self, m: &ast::ModelDecl) -> Result<IrStruct, LoweringError> {
        // RFC 021: Register field aliases for alias-aware resolution in expressions.
        self.register_field_aliases(&m.name, &m.fields);

        let checked_visibilities = self
            .type_info
            .as_ref()
            .and_then(|info| info.declarations.model_field_visibilities.get(&m.name))
            .cloned()
            .ok_or_else(|| LoweringError {
                message: format!(
                    "model `{}` reached lowering without typechecker-owned field visibility",
                    m.name
                ),
                span: Default::default(),
            })?;

        // A field type naming one of the model's own type parameters is a generic, not a nominal type. Lowering it
        // without them in scope resolves `Elem` in `items: list[Elem]` to `IrType::Struct("Elem")`, which every
        // later stage reads as a concrete type that happens not to exist. The call-site seed guard
        // (`is_unresolved_call_seed_type`) then cannot see that it needs substituting, because it treats `Struct`
        // as resolved, and an empty collection literal is emitted as `Vec::<Elem>::new()` in a scope where `Elem`
        // is not bound. See #1507.
        let type_param_names: std::collections::HashSet<&str> =
            m.type_params.iter().map(|param| param.name.as_str()).collect();

        let mut fields: Vec<StructField> = Vec::new();
        for f in &m.fields {
            let visibility = checked_visibilities
                .get(&f.node.name)
                .copied()
                .ok_or_else(|| LoweringError {
                    message: format!(
                        "checked model `{}` has no field visibility for `{}`",
                        m.name, f.node.name
                    ),
                    span: f.span.into(),
                })?;
            let default = f
                .node
                .default
                .as_ref()
                .map(|d| self.lower_expr_spanned(d))
                .transpose()?;
            fields.push(StructField {
                name: f.node.name.clone(),
                ty: self.lower_type_with_type_params(&f.node.ty.node, Some(&type_param_names)),
                surface_type_name: self.qualified_field_surface_type_name(&f.node.ty.node, f.span),
                visibility: Self::map_visibility(visibility),
                is_type_private: self.type_info.as_ref().is_some_and(|info| {
                    info.declarations
                        .model_type_private_fields
                        .contains(&(m.name.clone(), f.node.name.clone()))
                }),
                default,
                alias: f.node.metadata.alias.clone(),
                description: f.node.metadata.description.clone(),
            });
        }

        let (mut derives, derive_rust_modules) = self.extract_derives(&m.decorators);
        self.extend_derives_with_adopted_serde_traits(&mut derives, &m.traits);

        let debug = derives::as_str(DeriveId::Debug);
        let clone = derives::as_str(DeriveId::Clone);

        // Models always get Debug and Clone by default
        if !derives.iter().any(|d| d == debug) {
            derives.push(debug.to_string());
        }
        if !derives.iter().any(|d| d == clone) {
            derives.push(clone.to_string());
        }
        // Models always get FieldInfo for reflection.
        if !derives.iter().any(|d| d == derives::FIELD_INFO_DERIVE_NAME) {
            derives.push(derives::FIELD_INFO_DERIVE_NAME.to_string());
        }
        // Models always get IncanClass for __class_name__() and __fields__() methods.
        if !derives.iter().any(|d| d == derives::INCAN_CLASS_DERIVE_NAME) {
            derives.push(derives::INCAN_CLASS_DERIVE_NAME.to_string());
        }

        let type_params = self.lower_type_params(&m.type_params);
        let phantom_type_params = Self::phantom_type_params(&type_params, &fields);
        Ok(IrStruct {
            kind: IrStructKind::Model,
            name: m.name.clone(),
            docstring: m.docstring.clone(),
            fields,
            derives,
            visibility: self.map_type_visibility(m.visibility),
            type_params,
            phantom_type_params,
            derive_rust_modules,
            lint_allows: self.extract_rust_lint_allows(&m.decorators),
        })
    }

    /// Return the reflected type name for a model field whose annotation carries a module-qualified spelling.
    ///
    /// A model field's `FieldInfo.type_name` normally falls back to the lowered IR type's Incan spelling, which for
    /// a local or directly imported type is the declaration name. A qualified annotation (`errors.TomlError`,
    /// `Option[toml.TomlError]`) lowers to the Rust module path the emitter has to spell, and that path must not
    /// reach reflection (#1437). So the checked field type is rendered the way a class field already renders its
    /// own -- the declaration name, which is what the direct import of the same declaration reflects. Fields without
    /// a qualified spelling keep the existing fallback, so their reflected names do not change.
    fn qualified_field_surface_type_name(&self, ty: &ast::Type, span: ast::Span) -> Option<String> {
        if !Self::type_has_qualified_spelling(ty) {
            return None;
        }
        let resolved = self
            .type_info
            .as_ref()?
            .declarations
            .model_field_types
            .get(&(span.start, span.end))?;
        Some(incan_frontend::symbols::field_surface_type_name(ty, resolved))
    }

    /// Return whether an annotation contains a module-qualified spelling (`mod.Type` or `mod.Box[T]`) anywhere.
    fn type_has_qualified_spelling(ty: &ast::Type) -> bool {
        match ty {
            ast::Type::Dotted(_) | ast::Type::DottedGeneric(..) => true,
            ast::Type::Generic(_, args) | ast::Type::Tuple(args) => {
                args.iter().any(|arg| Self::type_has_qualified_spelling(&arg.node))
            }
            ast::Type::Function(params, ret) => {
                params
                    .iter()
                    .any(|param| Self::type_has_qualified_spelling(&param.node))
                    || Self::type_has_qualified_spelling(&ret.node)
            }
            ast::Type::Ref(inner) | ast::Type::RefMut(inner) | ast::Type::MutParam(inner) => {
                Self::type_has_qualified_spelling(&inner.node)
            }
            ast::Type::Simple(_)
            | ast::Type::Qualified(_)
            | ast::Type::ConstrainedPrimitive(..)
            | ast::Type::IntLiteral(_)
            | ast::Type::Unit
            | ast::Type::SelfType
            | ast::Type::Infer => false,
        }
    }
}
