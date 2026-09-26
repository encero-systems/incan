//! Class declaration lowering, including inherited field/method collection.

use super::super::super::Mutability;
use super::super::super::decl::{IrStruct, IrStructKind, StructField};
use super::super::super::types::IrType;
use super::super::AstLowering;
use super::super::errors::LoweringError;
use incan_frontend::ast::{self, Spanned};
use incan_frontend::rust_type_display;
use incan_lang::lang::derives::{self, DeriveId};

impl AstLowering {
    /// Lower a checked class declaration into its flattened struct layout.
    ///
    /// Class fields, defaults, visibility, and provider ownership come exclusively from the typechecker's parent-first
    /// layout so imported compiled parents remain semantically identical to source parents.
    pub(in crate::lower) fn lower_class(&mut self, c: &ast::ClassDecl) -> Result<IrStruct, LoweringError> {
        let Some(type_info) = self.type_info.as_ref() else {
            return Err(LoweringError {
                message: format!(
                    "class `{}` reached lowering without typechecker-owned layout artifacts",
                    c.name
                ),
                span: Default::default(),
            });
        };
        let Some(layout) = type_info.declarations.class_layouts.get(&c.name).cloned() else {
            return Err(LoweringError {
                message: format!("checked class `{}` has no lowering layout artifact", c.name),
                span: Default::default(),
            });
        };
        if !layout.missing_fields.is_empty() {
            return Err(LoweringError {
                message: format!(
                    "checked class `{}` layout is missing ordered fields: {}",
                    c.name,
                    layout.missing_fields.join(", ")
                ),
                span: Default::default(),
            });
        }
        if !layout.unmaterializable_defaults.is_empty() {
            return Err(LoweringError {
                message: format!(
                    "checked class `{}` has unmaterializable inherited defaults: {}",
                    c.name,
                    layout.unmaterializable_defaults.join(", ")
                ),
                span: Default::default(),
            });
        }
        let type_params = layout.type_params.clone();
        let fields = layout
            .fields
            .into_iter()
            .map(|field| {
                let default = match field.default.as_ref() {
                    Some(incan_frontend::typechecker::ClassFieldDefaultInfo::Source(default)) => {
                        Some(self.lower_expr_spanned(default)?)
                    }
                    Some(incan_frontend::typechecker::ClassFieldDefaultInfo::PublicDependency { library, value }) => {
                        let Some(default) = incan_frontend::library_manifest::param_default_from_checked(value) else {
                            return Err(LoweringError {
                                message: format!(
                                    "checked class `{}` default field `{}` is not materializable",
                                    c.name, field.name
                                ),
                                span: Default::default(),
                            });
                        };
                        let Some(lowered) = self.lower_pub_default_expr(library, &default) else {
                            return Err(LoweringError {
                                message: format!(
                                    "checked class `{}` default field `{}` could not be lowered",
                                    c.name, field.name
                                ),
                                span: Default::default(),
                            });
                        };
                        Some(lowered)
                    }
                    None => None,
                };
                Ok(StructField {
                    name: field.name,
                    ty: {
                        let mut emission_ty = field.ty;
                        rust_type_display::normalize_for_emission(
                            &mut emission_ty,
                            field.provider_library.as_deref(),
                            &type_params,
                        )
                        .map_err(|message| LoweringError {
                            message,
                            span: Default::default(),
                        })?;
                        self.lower_resolved_declaration_type(&emission_ty)
                    },
                    surface_type_name: field.surface_type_name,
                    visibility: Self::map_visibility(field.visibility),
                    is_type_private: matches!(field.visibility, incan_frontend::ast::Visibility::Private),
                    default,
                    alias: field.alias,
                    description: field.description,
                })
            })
            .collect::<Result<Vec<_>, LoweringError>>()?;

        let (mut derives, derive_rust_modules) = self.extract_derives(&c.decorators);
        self.extend_derives_with_adopted_serde_traits(&mut derives, &c.traits);

        let debug = derives::as_str(DeriveId::Debug);
        let clone = derives::as_str(DeriveId::Clone);

        // Classes normally get Debug by default. Direct Rust state can be opaque to Debug, so private adapter classes
        // with Rust-import fields opt out of the automatic derive unless the user requested it explicitly.
        let has_opaque_rust_field = c.name.starts_with('_')
            && c.fields
                .iter()
                .any(|f| self.type_uses_direct_rust_import(&f.node.ty.node));
        if !has_opaque_rust_field && !derives.iter().any(|d| d == debug) {
            derives.push(debug.to_string());
        }
        if !derives.iter().any(|d| d == clone) {
            derives.push(clone.to_string());
        }
        // Classes always get FieldInfo for reflection.
        if !derives.iter().any(|d| d == derives::FIELD_INFO_DERIVE_NAME) {
            derives.push(derives::FIELD_INFO_DERIVE_NAME.to_string());
        }
        // Classes always get IncanClass for __class_name__() and __fields__() methods.
        if !derives.iter().any(|d| d == derives::INCAN_CLASS_DERIVE_NAME) {
            derives.push(derives::INCAN_CLASS_DERIVE_NAME.to_string());
        }

        let type_params = self.lower_type_params(&c.type_params);
        let phantom_type_params = Self::phantom_type_params(&type_params, &fields);
        Ok(IrStruct {
            kind: IrStructKind::Class,
            name: c.name.clone(),
            docstring: c.docstring.clone(),
            fields,
            derives,
            visibility: self.map_type_visibility(c.visibility),
            type_params,
            phantom_type_params,
            derive_rust_modules,
            lint_allows: self.extract_rust_lint_allows(&c.decorators),
        })
    }

    /// Return whether a source type annotation contains a direct Rust import.
    pub(in crate::lower) fn type_uses_direct_rust_import(&self, ty: &ast::Type) -> bool {
        match ty {
            ast::Type::Simple(name) | ast::Type::ConstrainedPrimitive(name, _) => {
                self.rust_import_aliases.contains_key(name)
            }
            ast::Type::Qualified(segments) => segments
                .first()
                .is_some_and(|name| self.rust_import_aliases.contains_key(name)),
            ast::Type::Dotted(_) => false,
            ast::Type::Generic(base, args) => {
                self.rust_import_aliases.contains_key(base)
                    || args.iter().any(|arg| self.type_uses_direct_rust_import(&arg.node))
            }
            ast::Type::DottedGeneric(_, args) => args.iter().any(|arg| self.type_uses_direct_rust_import(&arg.node)),
            ast::Type::Function(params, ret) => {
                params
                    .iter()
                    .any(|param| self.type_uses_direct_rust_import(&param.node))
                    || self.type_uses_direct_rust_import(&ret.node)
            }
            ast::Type::Ref(inner) | ast::Type::RefMut(inner) | ast::Type::MutParam(inner) => {
                self.type_uses_direct_rust_import(&inner.node)
            }
            ast::Type::Tuple(items) => items.iter().any(|item| self.type_uses_direct_rust_import(&item.node)),
            ast::Type::Unit | ast::Type::SelfType | ast::Type::IntLiteral(_) | ast::Type::Infer => false,
        }
    }

    /// Return whether the outer annotation itself names one imported Rust type.
    ///
    /// This intentionally differs from [`Self::type_uses_direct_rust_import`]. A mutable Incan container or tuple
    /// that happens to contain a Rust value still follows the ordinary borrowed-parameter ABI; only a top-level Rust
    /// handle needs an owned mutable binding.
    pub(in crate::lower) fn type_is_top_level_direct_rust_import(&self, ty: &ast::Type) -> bool {
        match ty {
            ast::Type::Simple(name) | ast::Type::ConstrainedPrimitive(name, _) | ast::Type::Generic(name, _) => {
                self.rust_import_aliases.contains_key(name)
            }
            ast::Type::Qualified(segments) => segments
                .first()
                .is_some_and(|name| self.rust_import_aliases.contains_key(name)),
            ast::Type::Dotted(_)
            | ast::Type::DottedGeneric(_, _)
            | ast::Type::Function(_, _)
            | ast::Type::Ref(_)
            | ast::Type::RefMut(_)
            | ast::Type::MutParam(_)
            | ast::Type::Tuple(_)
            | ast::Type::Unit
            | ast::Type::SelfType
            | ast::Type::IntLiteral(_)
            | ast::Type::Infer => false,
        }
    }

    /// Lower a parameter's source mutability to how it is passed.
    ///
    /// A `mut` parameter is passed the way the checker's marker says (#1790, #1773): a marked parameter, whose changes
    /// reach the caller, as [`Mutability::Mutable`] (`&mut T`), an unmarked one (an `int`, `float` or `bool`, also
    /// through an alias, or a Rust handle) by value as [`Mutability::OwnedMutable`], `mut name: T`. The decision never
    /// reads the parameter's IR type. A parameter the checker recorded no marker for keeps the rule from its
    /// annotation: a direct Rust handle by value, anything else as `&mut T`.
    pub(in crate::lower) fn lower_parameter_mutability(&self, param: &ast::Spanned<ast::Param>) -> Mutability {
        if !param.node.is_mut {
            return Mutability::Immutable;
        }
        let marker = self
            .type_info
            .as_ref()
            .and_then(|info| info.declarations.mut_param_marker(param.span, &param.node.name));
        match marker {
            Some(true) => Mutability::Mutable,
            Some(false) => Mutability::OwnedMutable,
            None if self.type_is_top_level_direct_rust_import(&param.node.ty.node) => Mutability::OwnedMutable,
            None => Mutability::Mutable,
        }
    }

    /// Apply a frontend-recorded ownership projection to one mutable imported Rust generic.
    ///
    /// The typechecker has already inspected the foreign generic contract and keyed its structural decision by this
    /// annotation span. Lowering deliberately does not consult provider/type names or project configuration.
    pub(in crate::lower) fn apply_mutable_rust_type_argument_projections(
        &self,
        is_mut: bool,
        source_ty: &Spanned<ast::Type>,
        lowered_ty: IrType,
    ) -> IrType {
        if !is_mut {
            return lowered_ty;
        }
        let Some(projections) = self.type_info.as_ref().and_then(|type_info| {
            type_info
                .rust
                .mutable_reference_type_argument_projections
                .get(&(source_ty.span.start, source_ty.span.end))
        }) else {
            return lowered_ty;
        };
        let IrType::NamedGeneric(name, mut arguments) = lowered_ty else {
            return lowered_ty;
        };
        for projection in projections {
            if let Some(argument) = arguments.get_mut(projection.argument_position) {
                *argument = Self::project_mutable_rust_reference_paths(
                    argument.clone(),
                    projection.reference_leaf_paths.as_slice(),
                );
            }
        }
        IrType::NamedGeneric(name, arguments)
    }

    /// Project the exact tuple leaves the frontend proved need a mutable Rust reference.
    fn project_mutable_rust_reference_paths(ty: IrType, paths: &[Vec<usize>]) -> IrType {
        if paths.iter().any(Vec::is_empty) {
            return match ty {
                IrType::Ref(_) | IrType::RefMut(_) => ty,
                other => IrType::RefMut(Box::new(other)),
            };
        }
        let IrType::Tuple(mut items) = ty else {
            return ty;
        };
        for (index, item) in items.iter_mut().enumerate() {
            let child_paths = paths
                .iter()
                .filter_map(|path| {
                    path.first()
                        .filter(|first| **first == index)
                        .map(|_| path[1..].to_vec())
                })
                .collect::<Vec<_>>();
            if !child_paths.is_empty() {
                *item = Self::project_mutable_rust_reference_paths(item.clone(), child_paths.as_slice());
            }
        }
        IrType::Tuple(items)
    }

    /// Recursively collect all methods from this class and parent classes.
    pub(in crate::lower) fn collect_inherited_methods(
        &self,
        class_name: &str,
        methods: &mut Vec<Spanned<ast::MethodDecl>>,
    ) -> Result<(), LoweringError> {
        if let Some(class) = self.class_decls.get(class_name) {
            // First, collect grandparent methods if any
            if let Some(parent_name) = &class.extends {
                self.collect_inherited_methods(parent_name, methods)?;
            }

            // Then add/override with this class's own methods. Remove inherited methods shadowed by this class, but
            // keep same-name overloads declared together in the class.
            let local_method_names: std::collections::HashSet<&str> =
                class.methods.iter().map(|m| m.node.name.as_str()).collect();
            methods.retain(|existing| !local_method_names.contains(existing.node.name.as_str()));
            methods.extend(class.methods.iter().cloned());
        }
        Ok(())
    }

    /// Recursively collect all computed properties from this class and parent classes.
    pub(in crate::lower) fn collect_inherited_properties(
        &self,
        class_name: &str,
        properties: &mut Vec<Spanned<ast::PropertyDecl>>,
    ) -> Result<(), LoweringError> {
        if let Some(class) = self.class_decls.get(class_name) {
            if let Some(parent_name) = &class.extends {
                self.collect_inherited_properties(parent_name, properties)?;
            }

            for property in &class.properties {
                properties.retain(|existing| existing.node.name != property.node.name);
                properties.push(property.clone());
            }
        }
        Ok(())
    }
}
