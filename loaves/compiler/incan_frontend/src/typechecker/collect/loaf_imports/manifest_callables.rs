//! Function and partial infos, partial projections, and preset values from manifests.

use super::*;

impl TypeChecker {
    /// Convert one manifest function export into semantic function metadata.
    pub(in crate::typechecker::collect) fn function_info_from_manifest(&self, export: &FunctionExport) -> FunctionInfo {
        FunctionInfo {
            params: self.params_from_manifest(&export.params),
            return_type: resolved_type_from_manifest_type_ref(&export.return_type),
            is_async: export.is_async,
            type_params: export.type_params.iter().map(|param| param.name.clone()).collect(),
            type_param_bounds: self.type_param_bounds_from_manifest(&export.type_params),
            type_param_bound_details: self.type_param_bound_details_from_manifest(&export.type_params),
            emitted_name: export.emitted_name.clone(),
        }
    }

    /// Convert one manifest partial export into callable metadata for consumers.
    ///
    /// The partial's leftover defaulted parameters are completed from its target in the manifest's checked API, so a
    /// call may omit one exactly when its default can be filled in (#1760).
    pub(in crate::typechecker::collect) fn partial_info_from_manifest(
        &self,
        manifest: &LibraryManifest,
        export: &PartialExport,
    ) -> FunctionInfo {
        self.partial_info_from_export(&manifest_partial_with_target_defaults(
            manifest.contract_metadata.api.as_ref(),
            export,
        ))
    }

    /// Convert one completed partial export into callable metadata for consumers.
    pub(in crate::typechecker::collect) fn partial_info_from_export(&self, export: &PartialExport) -> FunctionInfo {
        FunctionInfo {
            params: self.params_from_manifest(&export.params),
            return_type: resolved_type_from_manifest_type_ref(&export.return_type),
            is_async: export.is_async,
            type_params: export.type_params.iter().map(|param| param.name.clone()).collect(),
            type_param_bounds: self.type_param_bounds_from_manifest(&export.type_params),
            type_param_bound_details: self.type_param_bound_details_from_manifest(&export.type_params),
            emitted_name: None,
        }
    }

    /// Convert one public-manifest export into partial projection metadata when it denotes a partial callable.
    pub(in crate::typechecker::collect) fn partial_projection_from_manifest_export(
        &self,
        manifest: &LibraryManifest,
        local_name: &str,
        export: &ManifestExportRef<'_>,
        imported_type_aliases: &HashMap<String, String>,
        span: Span,
    ) -> Option<PartialProjectionInfo> {
        match export {
            ManifestExportRef::Partial(export) => {
                Self::partial_projection_from_manifest_partial(export, local_name, imported_type_aliases, span)
            }
            ManifestExportRef::Alias(export) => {
                let context = PartialProjectionAliasContext {
                    manifest,
                    local_name,
                    imported_type_aliases,
                    span,
                };
                self.partial_projection_from_manifest_alias(&context, export, &mut HashSet::new())
            }
            _ => None,
        }
    }

    /// Follow manifest aliases so a public alias to a partial keeps the same projection under the alias name.
    pub(in crate::typechecker::collect) fn partial_projection_from_manifest_alias(
        &self,
        context: &PartialProjectionAliasContext<'_>,
        alias: &AliasExport,
        visited: &mut HashSet<String>,
    ) -> Option<PartialProjectionInfo> {
        let identity_target = Self::manifest_identity_target_path(context.manifest, &alias.name);
        let target_path = identity_target.unwrap_or(alias.target_path.as_slice());
        let target_name = target_path.last()?;
        if !visited.insert(target_name.clone()) {
            return None;
        }
        if let Some(ApiDeclaration::Partial(item)) =
            Self::api_declaration_for_target_path(context.manifest, target_path)
        {
            let export = partial_export_from_api(item);
            return Self::partial_projection_from_manifest_partial(
                &export,
                context.local_name,
                context.imported_type_aliases,
                context.span,
            );
        }
        match Self::find_manifest_export(context.manifest, target_name)? {
            ManifestExportRef::Partial(export) => Self::partial_projection_from_manifest_partial(
                export,
                context.local_name,
                context.imported_type_aliases,
                context.span,
            ),
            ManifestExportRef::Alias(next_alias) => {
                self.partial_projection_from_manifest_alias(context, next_alias, visited)
            }
            _ => None,
        }
    }

    /// Reconstruct import-visible projection metadata from serialized partial preset metadata.
    pub(in crate::typechecker::collect) fn partial_projection_from_manifest_partial(
        export: &PartialExport,
        local_name: &str,
        imported_type_aliases: &HashMap<String, String>,
        span: Span,
    ) -> Option<PartialProjectionInfo> {
        let mut target_path = export.target_path.clone();
        if let Some(target_name) = target_path.last_mut()
            && let Some(local_alias) = imported_type_aliases.get(target_name)
        {
            target_name.clone_from(local_alias);
        }
        Some(PartialProjectionInfo {
            name: local_name.to_string(),
            target_path,
            target_kind: Self::partial_projection_target_kind_from_manifest(export.target_kind),
            presets: export
                .presets
                .iter()
                .map(|preset| {
                    Some(PartialProjectionPreset {
                        name: preset.name.clone(),
                        value: Self::manifest_preset_value_expr(&preset.value, imported_type_aliases, span)?,
                    })
                })
                .collect::<Option<Vec<_>>>()?,
        })
    }

    /// Convert manifest partial target kind metadata into frontend projection vocabulary.
    pub(in crate::typechecker::collect) fn partial_projection_target_kind_from_manifest(
        kind: PartialTargetKindExport,
    ) -> PartialProjectionTargetKind {
        match kind {
            PartialTargetKindExport::Function => PartialProjectionTargetKind::Function,
            PartialTargetKindExport::ModelConstructor => PartialProjectionTargetKind::ModelConstructor,
            PartialTargetKindExport::ClassConstructor => PartialProjectionTargetKind::ClassConstructor,
            PartialTargetKindExport::NewtypeConstructor => PartialProjectionTargetKind::NewtypeConstructor,
            PartialTargetKindExport::Partial | PartialTargetKindExport::Unknown => PartialProjectionTargetKind::Unknown,
        }
    }

    /// Rebuild a metadata-safe preset value as a synthetic AST expression.
    pub(in crate::typechecker::collect) fn manifest_preset_value_expr(
        value: &PresetValueExport,
        imported_type_aliases: &HashMap<String, String>,
        span: Span,
    ) -> Option<Spanned<Expr>> {
        let expr = match value {
            PresetValueExport::Int(value) => Expr::Literal(Literal::Int(IntLiteral::synthetic(*value))),
            PresetValueExport::Float(value) => Expr::Literal(Literal::Float(FloatLiteral {
                value: value.parse().ok()?,
                repr: value.clone(),
                suffix: None,
            })),
            PresetValueExport::Bool(value) => Expr::Literal(Literal::Bool(*value)),
            PresetValueExport::String(value) => Expr::Literal(Literal::String(value.clone())),
            PresetValueExport::Bytes(value) => Expr::Literal(Literal::Bytes(value.clone())),
            PresetValueExport::None => Expr::Literal(Literal::None),
            PresetValueExport::List(values) => Expr::List(
                values
                    .iter()
                    .map(|item| {
                        Self::manifest_preset_value_expr(item, imported_type_aliases, span).map(ListEntry::Element)
                    })
                    .collect::<Option<Vec<_>>>()?,
            ),
            PresetValueExport::Set(values) => Expr::Set(
                values
                    .iter()
                    .map(|item| Self::manifest_preset_value_expr(item, imported_type_aliases, span))
                    .collect::<Option<Vec<_>>>()?,
            ),
            PresetValueExport::Tuple(values) => Expr::Tuple(
                values
                    .iter()
                    .map(|item| Self::manifest_preset_value_expr(item, imported_type_aliases, span))
                    .collect::<Option<Vec<_>>>()?,
            ),
            PresetValueExport::Dict(entries) => Expr::Dict(
                entries
                    .iter()
                    .map(|entry| {
                        Some(DictEntry::Pair(
                            Self::manifest_preset_value_expr(&entry.key, imported_type_aliases, span)?,
                            Self::manifest_preset_value_expr(&entry.value, imported_type_aliases, span)?,
                        ))
                    })
                    .collect::<Option<Vec<_>>>()?,
            ),
            PresetValueExport::ConstRef(path) => Self::manifest_const_ref_expr(path, span)?.node,
            PresetValueExport::ModelLiteral { name, fields } => {
                let constructor = imported_type_aliases.get(name).cloned().unwrap_or_else(|| name.clone());
                Expr::Call(
                    Box::new(Spanned::new(Expr::Ident(constructor), span)),
                    Vec::new(),
                    fields
                        .iter()
                        .map(|field| {
                            Some(CallArg::Named(
                                Spanned::new(field.name.clone(), span),
                                Self::manifest_preset_value_expr(&field.value, imported_type_aliases, span)?,
                            ))
                        })
                        .collect::<Option<Vec<_>>>()?,
                )
            }
            PresetValueExport::Unsupported => return None,
        };
        Some(Spanned::new(expr, span))
    }

    /// Build an identifier/field expression for a serialized const reference path.
    pub(in crate::typechecker::collect) fn manifest_const_ref_expr(
        path: &[String],
        span: Span,
    ) -> Option<Spanned<Expr>> {
        let (first, rest) = path.split_first()?;
        let mut expr = Spanned::new(Expr::Ident(first.clone()), span);
        for segment in rest {
            expr = Spanned::new(Expr::Field(Box::new(expr), segment.clone()), span);
        }
        Some(expr)
    }
}
