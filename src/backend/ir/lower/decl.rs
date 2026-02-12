//! Declaration lowering for AST to IR conversion.
//!
//! This module handles lowering of all declaration types: functions, models,
//! classes, enums, newtypes, traits, and imports.

use std::collections::HashMap;

use super::super::decl::{
    EnumVariant, FunctionParam, IrDecl, IrDeclKind, IrEnum, IrFunction, IrImpl, IrStruct, IrTrait, StructField,
    VariantFields, Visibility,
};
use super::super::types::IrType;
use super::super::{IrSpan, Mutability};
use super::AstLowering;
use super::errors::LoweringError;
use crate::frontend::ast::{self, Spanned};
use incan_core::lang::decorators::{self, DecoratorId};
use incan_core::lang::derives::{self, DeriveId};
use incan_core::lang::keywords::{self, KeywordId};

impl AstLowering {
    /// Map frontend visibility (`pub` / private) to IR visibility for Rust emission.
    fn map_visibility(vis: crate::frontend::ast::Visibility) -> Visibility {
        match vis {
            crate::frontend::ast::Visibility::Private => Visibility::Private,
            crate::frontend::ast::Visibility::Public => Visibility::Public,
        }
    }

    /// Lower a declaration to IR.
    ///
    /// # Parameters
    ///
    /// * `decl` - The AST declaration to lower
    ///
    /// # Returns
    ///
    /// The corresponding IR declaration.
    ///
    /// # Errors
    ///
    /// Returns `LoweringError` if the declaration cannot be lowered.
    pub(super) fn lower_declaration(&mut self, decl: &ast::Declaration) -> Result<IrDecl, LoweringError> {
        let kind = match decl {
            ast::Declaration::Function(f) => IrDeclKind::Function(self.lower_function(f)?),
            ast::Declaration::Const(c) => {
                let value = self.lower_expr_spanned(&c.value)?;
                // RFC 008: In const context, annotations imply frozen/static types.
                // Prefer frozen annotation if present; otherwise use the initializer type.
                let ty = if let Some(ann) = &c.ty {
                    self.lower_const_annotation_type(&ann.node)
                } else if !matches!(value.ty, IrType::Unknown) {
                    value.ty.clone()
                } else {
                    IrType::Unknown
                };
                let visibility = match c.visibility {
                    ast::Visibility::Public => Visibility::Public,
                    ast::Visibility::Private => Visibility::Private,
                };
                IrDeclKind::Const {
                    visibility,
                    name: c.name.clone(),
                    ty,
                    value,
                }
            }
            ast::Declaration::Model(m) => {
                let struct_ir = self.lower_model(m)?;
                // Register struct name for constructor detection
                self.struct_names
                    .insert(struct_ir.name.clone(), IrType::Struct(struct_ir.name.clone()));
                IrDeclKind::Struct(struct_ir)
            }
            ast::Declaration::Class(c) => {
                let struct_ir = self.lower_class(c)?;
                // Register struct name for constructor detection
                self.struct_names
                    .insert(struct_ir.name.clone(), IrType::Struct(struct_ir.name.clone()));
                IrDeclKind::Struct(struct_ir)
            }
            ast::Declaration::Enum(e) => {
                let enum_ir = self.lower_enum(e)?;
                // Register enum name for type resolution
                self.enum_names
                    .insert(enum_ir.name.clone(), IrType::Enum(enum_ir.name.clone()));
                IrDeclKind::Enum(enum_ir)
            }
            ast::Declaration::Newtype(n) => {
                // Note: newtype checked construction hook selection is done in `lower_program` when we see the full
                // newtype declaration.
                let struct_ir = self.lower_newtype(n)?;
                // Register struct name for constructor detection
                self.struct_names
                    .insert(struct_ir.name.clone(), IrType::Struct(struct_ir.name.clone()));
                IrDeclKind::Struct(struct_ir)
            }
            ast::Declaration::Import(i) => self.lower_import(i),
            ast::Declaration::Trait(t) => IrDeclKind::Trait(self.lower_trait(t)?),
            ast::Declaration::Docstring(_) => {
                // Skip docstrings in codegen
                return Err(LoweringError {
                    message: "Docstrings are not lowered to IR".to_string(),
                    span: IrSpan::default(),
                });
            }
        };
        Ok(IrDecl::new(kind))
    }

    /// Lower a function declaration.
    ///
    /// # Parameters
    ///
    /// * `f` - The AST function declaration
    ///
    /// # Returns
    ///
    /// The corresponding IR function.
    pub(super) fn lower_function(&mut self, f: &ast::FunctionDecl) -> Result<IrFunction, LoweringError> {
        self.scopes.push(HashMap::new());

        let params: Vec<FunctionParam> = f
            .params
            .iter()
            .map(|p| {
                let base_ty = self.lower_type(&p.node.ty.node);
                // For mutable parameters, wrap in RefMut to track that it's a &mut reference
                let ty = if p.node.is_mut {
                    IrType::RefMut(Box::new(base_ty.clone()))
                } else {
                    base_ty.clone()
                };
                if let Some(scope) = self.scopes.last_mut() {
                    scope.insert(p.node.name.clone(), ty.clone());
                }
                // Track mutable parameters
                if p.node.is_mut {
                    self.mutable_vars.insert(p.node.name.clone(), true);
                }
                FunctionParam {
                    name: p.node.name.clone(),
                    ty: base_ty, // Store the base type in the param (emit will add &mut)
                    mutability: if p.node.is_mut {
                        Mutability::Mutable
                    } else {
                        Mutability::Immutable
                    },
                    is_self: false,
                }
            })
            .collect();

        let return_type = self.lower_type(&f.return_type.node);
        let body = self.lower_statements(&f.body)?;
        self.scopes.pop();

        // RFC 023: detect @rust.extern decorator to mark this function as externally-backed.
        let is_extern = Self::has_rust_extern_decorator(&f.decorators);

        Ok(IrFunction {
            name: f.name.clone(),
            params,
            return_type,
            body,
            is_async: f.is_async,
            visibility: Self::map_visibility(f.visibility),
            type_params: f.type_params.clone(),
            is_extern,
        })
    }

    /// RFC 023: Check if a decorator list contains `@rust.extern`.
    ///
    /// Used during lowering to mark functions whose body is provided by a Rust backing module.
    /// Uses `from_segments` on the full decorator path (e.g. `["rust", "extern"]`) since the `name` field only stores
    /// the last segment.
    fn has_rust_extern_decorator(decorators_list: &[Spanned<ast::Decorator>]) -> bool {
        decorators_list
            .iter()
            .any(|d| decorators::from_segments(&d.node.path.segments) == Some(DecoratorId::RustExtern))
    }

    /// Extract derives from decorators.
    ///
    /// Parses `@derive(Serialize, Deserialize)` decorators and returns the list of derive names. Also adds prerequisite
    /// derives (e.g., Eq requires PartialEq).
    pub(super) fn extract_derives(&self, decorators: &[Spanned<ast::Decorator>]) -> Vec<String> {
        let mut derives = Vec::new();

        for decorator in decorators {
            if decorators::from_str(decorator.node.name.as_str()) == Some(DecoratorId::Derive) {
                // Extract derive arguments: @derive(Serialize, Deserialize)
                for arg in &decorator.node.args {
                    if let ast::DecoratorArg::Positional(expr) = arg {
                        // Handle simple identifier expressions
                        if let ast::Expr::Ident(name) = &expr.node {
                            derives.push(name.clone());
                        }
                    }
                }
            }
        }

        fn has(derives: &[String], name: &str) -> bool {
            derives.iter().any(|d| d == name)
        }

        // Add prerequisite derives automatically
        // Eq requires PartialEq
        let eq = derives::as_str(DeriveId::Eq);
        let partial_eq = derives::as_str(DeriveId::PartialEq);
        if has(&derives, eq) && !has(&derives, partial_eq) {
            derives.push(partial_eq.to_string());
        }
        // Ord requires PartialOrd and Eq (and thus PartialEq)
        let ord = derives::as_str(DeriveId::Ord);
        let partial_ord = derives::as_str(DeriveId::PartialOrd);
        if has(&derives, ord) {
            if !has(&derives, partial_ord) {
                derives.push(partial_ord.to_string());
            }
            if !has(&derives, eq) {
                derives.push(eq.to_string());
            }
            if !has(&derives, partial_eq) {
                derives.push(partial_eq.to_string());
            }
        }

        derives
    }

    /// Lower a model declaration to struct.
    pub(super) fn lower_model(&mut self, m: &ast::ModelDecl) -> Result<IrStruct, LoweringError> {
        // RFC 021: Register field aliases for alias-aware resolution in expressions.
        self.register_field_aliases(&m.name, &m.fields);

        let mut fields: Vec<StructField> = Vec::new();
        for f in &m.fields {
            let default = f
                .node
                .default
                .as_ref()
                .map(|d| self.lower_expr_spanned(d))
                .transpose()?;
            fields.push(StructField {
                name: f.node.name.clone(),
                ty: self.lower_type(&f.node.ty.node),
                visibility: Self::map_visibility(f.node.visibility),
                default,
                alias: f.node.metadata.alias.clone(),
                description: f.node.metadata.description.clone(),
            });
        }

        let mut derives = self.extract_derives(&m.decorators);

        let debug = derives::as_str(DeriveId::Debug);
        let clone = derives::as_str(DeriveId::Clone);
        // Models always get Debug and Clone by default
        if !derives.iter().any(|d| d == debug) {
            derives.push(debug.to_string());
        }
        if !derives.iter().any(|d| d == clone) {
            derives.push(clone.to_string());
        }
        // Models always get FieldInfo for reflection
        if !derives.contains(&"FieldInfo".to_string()) {
            derives.push("FieldInfo".to_string());
        }
        // Models always get IncanClass for __class__() and __fields__() methods
        if !derives.contains(&"IncanClass".to_string()) {
            derives.push("IncanClass".to_string());
        }

        Ok(IrStruct {
            name: m.name.clone(),
            fields,
            derives,
            visibility: Self::map_visibility(m.visibility),
            type_params: m.type_params.clone(),
        })
    }

    /// Lower a class declaration to struct.
    pub(super) fn lower_class(&mut self, c: &ast::ClassDecl) -> Result<IrStruct, LoweringError> {
        let mut fields: Vec<StructField> = Vec::new();

        // If class extends a parent, include parent fields first
        if let Some(parent_name) = &c.extends {
            self.collect_inherited_fields(parent_name, &mut fields)?;
        }

        // Add this class's own fields
        for f in &c.fields {
            let default = f
                .node
                .default
                .as_ref()
                .map(|d| self.lower_expr_spanned(d))
                .transpose()?;
            fields.push(StructField {
                name: f.node.name.clone(),
                ty: self.lower_type(&f.node.ty.node),
                visibility: Self::map_visibility(f.node.visibility),
                default,
                alias: f.node.metadata.alias.clone(),
                description: f.node.metadata.description.clone(),
            });
        }

        let mut derives = self.extract_derives(&c.decorators);

        let debug = derives::as_str(DeriveId::Debug);
        let clone = derives::as_str(DeriveId::Clone);
        // Classes always get Debug and Clone by default
        if !derives.iter().any(|d| d == debug) {
            derives.push(debug.to_string());
        }
        if !derives.iter().any(|d| d == clone) {
            derives.push(clone.to_string());
        }
        // Classes always get FieldInfo for reflection
        if !derives.contains(&"FieldInfo".to_string()) {
            derives.push("FieldInfo".to_string());
        }
        // Classes always get IncanClass for __class__() and __fields__() methods
        if !derives.contains(&"IncanClass".to_string()) {
            derives.push("IncanClass".to_string());
        }

        Ok(IrStruct {
            name: c.name.clone(),
            fields,
            derives,
            visibility: Self::map_visibility(c.visibility),
            type_params: c.type_params.clone(),
        })
    }

    /// Recursively collect all inherited fields from parent classes.
    pub(super) fn collect_inherited_fields(
        &mut self,
        class_name: &str,
        fields: &mut Vec<StructField>,
    ) -> Result<(), LoweringError> {
        // Clone to avoid borrowing `self.class_decls` across recursive calls and expression lowering.
        let parent_class = self.class_decls.get(class_name).cloned();
        if let Some(parent_class) = parent_class {
            // First, collect grandparent fields if any
            if let Some(grandparent_name) = &parent_class.extends {
                self.collect_inherited_fields(grandparent_name, fields)?;
            }

            // Then add parent's own fields
            for f in &parent_class.fields {
                let default = f
                    .node
                    .default
                    .as_ref()
                    .map(|d| self.lower_expr_spanned(d))
                    .transpose()?;
                fields.push(StructField {
                    name: f.node.name.clone(),
                    ty: self.lower_type(&f.node.ty.node),
                    visibility: Self::map_visibility(f.node.visibility),
                    default,
                    alias: f.node.metadata.alias.clone(),
                    description: f.node.metadata.description.clone(),
                });
            }
        }
        Ok(())
    }

    /// Recursively collect all methods from this class and parent classes.
    pub(super) fn collect_inherited_methods(
        &self,
        class_name: &str,
        methods: &mut Vec<Spanned<ast::MethodDecl>>,
    ) -> Result<(), LoweringError> {
        if let Some(class) = self.class_decls.get(class_name) {
            // First, collect grandparent methods if any
            if let Some(parent_name) = &class.extends {
                self.collect_inherited_methods(parent_name, methods)?;
            }

            // Then add/override with this class's own methods
            // If a method with the same name exists, remove it first (child overrides parent)
            for m in &class.methods {
                // Remove any existing method with the same name
                methods.retain(|existing| existing.node.name != m.node.name);
                // Add the new method
                methods.push(m.clone());
            }
        }
        Ok(())
    }

    /// Lower a newtype declaration to tuple struct.
    pub(super) fn lower_newtype(&mut self, n: &ast::NewtypeDecl) -> Result<IrStruct, LoweringError> {
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

    /// Lower model methods into an impl block.
    pub(super) fn lower_model_methods(
        &mut self,
        type_name: &str,
        methods: &[Spanned<ast::MethodDecl>],
    ) -> Result<IrImpl, LoweringError> {
        let prev = self.current_impl_type.replace(type_name.to_string());
        // IMPORTANT: always restore `current_impl_type` even if lowering fails, since lowering continues after
        // collecting errors.
        let lowered = methods
            .iter()
            .map(|m| self.lower_method(&m.node))
            .collect::<Result<Vec<_>, LoweringError>>();
        self.current_impl_type = prev;
        let lowered_methods = lowered?;

        Ok(IrImpl {
            target_type: type_name.to_string(),
            trait_name: None,
            methods: lowered_methods,
        })
    }

    /// Lower trait implementation for a class.
    ///
    /// Only methods matching trait signatures go in `impl Trait for Type`.
    pub(super) fn lower_trait_impl(
        &mut self,
        type_name: &str,
        trait_name: &str,
        impl_methods: &[Spanned<ast::MethodDecl>],
    ) -> Result<IrImpl, LoweringError> {
        // Avoid holding an immutable borrow of `self` across lowering calls.
        let trait_decl = self.trait_decls.get(trait_name).cloned().ok_or_else(|| LoweringError {
            message: format!("Unknown trait '{trait_name}'"),
            span: IrSpan::default(),
        })?;
        let trait_methods = trait_decl.methods;

        let mut methods: Vec<IrFunction> = Vec::new();
        for trait_method in &trait_methods {
            let method_name = trait_method.node.name.as_str();

            // Prefer the implementing type's override, if present.
            let mut found_override: Option<&ast::MethodDecl> = None;
            for m in impl_methods {
                if m.node.name == method_name {
                    found_override = Some(&m.node);
                    break;
                }
            }
            if let Some(m) = found_override {
                methods.push(self.lower_impl_method_for_trait(m)?);
                continue;
            }

            // Otherwise, expand a default method body into the impl (RFC 000: defaults may assume adopter fields).
            if trait_method.node.body.is_some() {
                methods.push(self.lower_impl_method_for_trait(&trait_method.node)?);
                continue;
            }

            // Required trait method with no default implementation.
            return Err(LoweringError {
                message: format!(
                    "Type '{type_name}' does not implement required method '{method_name}' for trait '{trait_name}'"
                ),
                span: IrSpan::default(),
            });
        }

        Ok(IrImpl {
            target_type: type_name.to_string(),
            trait_name: Some(trait_name.to_string()),
            methods,
        })
    }

    fn lower_impl_method_for_trait(&mut self, m: &ast::MethodDecl) -> Result<IrFunction, LoweringError> {
        self.scopes.push(HashMap::new());

        // Handle receiver (self) parameter
        let mut params = Vec::new();
        if let Some(receiver) = &m.receiver {
            params.push(FunctionParam {
                name: "self".to_string(),
                ty: IrType::SelfType,
                mutability: match receiver {
                    ast::Receiver::Immutable => Mutability::Immutable,
                    ast::Receiver::Mutable => Mutability::Mutable,
                },
                is_self: true,
            });
        }

        // Add regular parameters
        let other_params: Vec<FunctionParam> = m
            .params
            .iter()
            .map(|p| {
                let base_ty = self.lower_type(&p.node.ty.node);
                FunctionParam {
                    name: p.node.name.clone(),
                    ty: base_ty,
                    mutability: if p.node.is_mut {
                        Mutability::Mutable
                    } else {
                        Mutability::Immutable
                    },
                    is_self: false,
                }
            })
            .collect();
        params.extend(other_params);

        let return_type = self.lower_type(&m.return_type.node);
        let body = if let Some(ref body_stmts) = m.body {
            self.lower_statements(body_stmts)?
        } else {
            vec![]
        };

        self.scopes.pop();

        Ok(IrFunction {
            name: m.name.clone(),
            params,
            return_type,
            body,
            is_async: m.is_async,
            visibility: Visibility::Private,
            type_params: vec![],
            is_extern: false,
        })
    }

    /// Lower class methods into an impl block.
    pub(super) fn lower_class_methods(
        &mut self,
        type_name: &str,
        methods: &[Spanned<ast::MethodDecl>],
    ) -> Result<IrImpl, LoweringError> {
        let prev = self.current_impl_type.replace(type_name.to_string());
        // IMPORTANT: always restore `current_impl_type` even if lowering fails, since lowering
        // continues after collecting errors.
        let lowered = methods
            .iter()
            .map(|m| self.lower_method(&m.node))
            .collect::<Result<Vec<_>, LoweringError>>();
        self.current_impl_type = prev;
        let lowered_methods = lowered?;

        Ok(IrImpl {
            target_type: type_name.to_string(),
            trait_name: None,
            methods: lowered_methods,
        })
    }

    /// Lower a method declaration into a function.
    pub(super) fn lower_method(&mut self, m: &ast::MethodDecl) -> Result<IrFunction, LoweringError> {
        self.scopes.push(HashMap::new());

        let mut params: Vec<FunctionParam> = Vec::new();

        // Add self parameter if receiver is present
        if let Some(receiver) = m.receiver {
            let is_mut = matches!(receiver, ast::Receiver::Mutable);
            params.push(FunctionParam {
                name: "self".to_string(),
                ty: IrType::Unknown, // Will be determined by impl context
                mutability: if is_mut {
                    Mutability::Mutable
                } else {
                    Mutability::Immutable
                },
                is_self: true,
            });
            // Add self to scope
            if let Some(scope) = self.scopes.last_mut() {
                scope.insert("self".to_string(), IrType::Unknown);
            }
        }

        // Add regular parameters
        let other_params: Vec<FunctionParam> = m
            .params
            .iter()
            .map(|p| {
                let base_ty = self.lower_type(&p.node.ty.node);
                // For mutable parameters, wrap in RefMut
                let ty = if p.node.is_mut {
                    IrType::RefMut(Box::new(base_ty.clone()))
                } else {
                    base_ty.clone()
                };
                if let Some(scope) = self.scopes.last_mut() {
                    scope.insert(p.node.name.clone(), ty.clone());
                }
                // Track mutable parameters
                if p.node.is_mut {
                    self.mutable_vars.insert(p.node.name.clone(), true);
                }
                FunctionParam {
                    name: p.node.name.clone(),
                    ty: base_ty,
                    mutability: if p.node.is_mut {
                        Mutability::Mutable
                    } else {
                        Mutability::Immutable
                    },
                    is_self: p.node.name == keywords::as_str(KeywordId::SelfKw),
                }
            })
            .collect();
        params.extend(other_params);

        let return_type = self.lower_type(&m.return_type.node);
        let body = if let Some(ref body_stmts) = m.body {
            self.lower_statements(body_stmts)?
        } else {
            // Abstract method with no body
            vec![]
        };
        self.scopes.pop();

        Ok(IrFunction {
            name: m.name.clone(),
            params,
            return_type,
            body,
            is_async: m.is_async,
            visibility: Visibility::Private,
            type_params: vec![],
            is_extern: false,
        })
    }

    /// Lower a trait declaration.
    pub(super) fn lower_trait(&mut self, t: &ast::TraitDecl) -> Result<IrTrait, LoweringError> {
        let methods: Vec<IrFunction> = t
            .methods
            .iter()
            .map(|m| {
                self.scopes.push(HashMap::new());

                // Handle receiver (self) parameter
                let mut params = Vec::new();
                if let Some(receiver) = &m.node.receiver {
                    params.push(FunctionParam {
                        name: "self".to_string(),
                        ty: IrType::SelfType,
                        mutability: match receiver {
                            ast::Receiver::Immutable => Mutability::Immutable,
                            ast::Receiver::Mutable => Mutability::Mutable,
                        },
                        is_self: true,
                    });
                }

                // Add regular parameters
                let other_params: Vec<FunctionParam> = m
                    .node
                    .params
                    .iter()
                    .map(|p| {
                        let ty = self.lower_type(&p.node.ty.node);
                        FunctionParam {
                            name: p.node.name.clone(),
                            ty,
                            mutability: if p.node.is_mut {
                                Mutability::Mutable
                            } else {
                                Mutability::Immutable
                            },
                            is_self: false,
                        }
                    })
                    .collect();
                params.extend(other_params);

                let return_type = self.lower_type(&m.node.return_type.node);
                // IMPORTANT: We intentionally do NOT emit trait method bodies into the Rust trait itself.
                // Default methods are expanded into each adopting `impl Trait for Type` block during lowering,
                // which allows bodies to assume adopter fields (RFC 000) without generating invalid Rust
                // trait default methods like `self.name`.
                let body = vec![];

                self.scopes.pop();

                Ok(IrFunction {
                    name: m.node.name.clone(),
                    params,
                    return_type,
                    body,
                    is_async: m.node.is_async,
                    visibility: Visibility::Private,
                    type_params: vec![],
                    is_extern: false,
                })
            })
            .collect::<Result<Vec<_>, LoweringError>>()?;

        Ok(IrTrait {
            name: t.name.clone(),
            methods,
            visibility: Self::map_visibility(t.visibility),
        })
    }

    /// Lower an enum declaration.
    pub(super) fn lower_enum(&mut self, e: &ast::EnumDecl) -> Result<IrEnum, LoweringError> {
        let variants = e
            .variants
            .iter()
            .map(|v| {
                let fields = if v.node.fields.is_empty() {
                    VariantFields::Unit
                } else {
                    VariantFields::Tuple(v.node.fields.iter().map(|t| self.lower_type(&t.node)).collect())
                };
                EnumVariant {
                    name: v.node.name.clone(),
                    fields,
                }
            })
            .collect();

        // Extract user-specified derives from decorators
        let mut derives = self.extract_derives(&e.decorators);

        // Enums always get Debug, Clone, PartialEq by default (if not already specified)
        let debug = derives::as_str(DeriveId::Debug);
        let clone = derives::as_str(DeriveId::Clone);
        let partial_eq = derives::as_str(DeriveId::PartialEq);
        if !derives.iter().any(|d| d == debug) {
            derives.push(debug.to_string());
        }
        if !derives.iter().any(|d| d == clone) {
            derives.push(clone.to_string());
        }
        if !derives.iter().any(|d| d == partial_eq) {
            derives.push(partial_eq.to_string());
        }

        Ok(IrEnum {
            name: e.name.clone(),
            variants,
            derives,
            visibility: Self::map_visibility(e.visibility),
            type_params: e.type_params.clone(),
        })
    }

    /// Lower an import declaration.
    pub(super) fn lower_import(&self, i: &ast::ImportDecl) -> IrDeclKind {
        let (path, ast_items) = match &i.kind {
            ast::ImportKind::Module(p) => (p.segments.clone(), vec![]),
            ast::ImportKind::From { module, items } => (module.segments.clone(), items.clone()),
            ast::ImportKind::RustCrate { crate_name, path, .. } => {
                let mut segs = vec![crate_name.clone()];
                segs.extend(path.clone());
                (segs, vec![])
            }
            ast::ImportKind::RustFrom {
                crate_name,
                path,
                items,
                ..  // Ignore version and features
            } => {
                let mut segs = vec![crate_name.clone()];
                segs.extend(path.clone());
                (segs, items.clone())
            }
            ast::ImportKind::Python(s) => (vec![s.clone()], vec![]),
        };

        let qualifier = match &i.kind {
            ast::ImportKind::Module(p) => {
                if p.parent_levels > 0 {
                    super::super::decl::IrImportQualifier::Super(p.parent_levels)
                } else if p.is_absolute {
                    super::super::decl::IrImportQualifier::Crate
                } else {
                    super::super::decl::IrImportQualifier::Auto
                }
            }
            ast::ImportKind::From { module, .. } => {
                if module.parent_levels > 0 {
                    super::super::decl::IrImportQualifier::Super(module.parent_levels)
                } else if module.is_absolute {
                    super::super::decl::IrImportQualifier::Crate
                } else {
                    super::super::decl::IrImportQualifier::Auto
                }
            }
            _ => super::super::decl::IrImportQualifier::None,
        };

        // Convert AST import items to IR import items
        let ir_items: Vec<super::super::decl::IrImportItem> = ast_items
            .iter()
            .map(|item| super::super::decl::IrImportItem {
                name: item.name.clone(),
                alias: item.alias.clone(),
            })
            .collect();

        IrDeclKind::Import {
            qualifier,
            path,
            alias: i.alias.clone(),
            items: ir_items,
        }
    }
}
