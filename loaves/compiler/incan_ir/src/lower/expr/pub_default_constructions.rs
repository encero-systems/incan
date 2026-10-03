//! Dependency parameter defaults and partial presets that construct one of the dependency's public models or classes.
//!
//! A dependency carries such a default in its manifest as a call of the type's public path (#1771), the same record a
//! helper-function default has, and such a preset as a model literal. A consumer that omits the argument, or calls the
//! partial, receives the construction at its own call site as a call whose callee is the type, reached through the
//! dependency's public path, so the construction does not rely on the consumer importing the type. The fields the call
//! leaves out take the defaults the type declares.

use super::super::super::TypedExpr;
use super::super::super::expr::{IrCallArg, IrCallArgKind, IrExprKind, VarAccess, VarRefKind};
use super::super::super::types::IrType;
use super::super::AstLowering;
use incan_frontend::api_metadata::ApiDeclaration;
use incan_frontend::library_exports::CheckedPresetValue;
use incan_frontend::library_manifest::{FieldExport, ParamDefaultCallArgExport, ParamDefaultExport};
use incan_frontend::library_manifest_index::LibraryManifestIndexEntry;

impl AstLowering {
    /// Return whether an exported default's call path names a model or class that `library` declares.
    ///
    /// A path that also names a function of the library is a helper call, whatever else shares its name.
    pub(in crate::lower) fn pub_default_path_names_constructed_type(&self, library: &str, path: &[String]) -> bool {
        self.pub_constructed_type_fields(library, path).is_some()
    }

    /// Return the declared fields of the model or class of `library` an exported call path names, if it names one.
    fn pub_constructed_type_fields(&self, library: &str, path: &[String]) -> Option<Vec<FieldExport>> {
        let type_name = path.last()?;
        if self.pub_function_export_for_path(library, path).is_some() {
            return None;
        }
        let index = self.provider_plan.as_deref()?.library_manifest_index();
        let Some(LibraryManifestIndexEntry::Loaded { manifest, .. }) = index.get(library) else {
            return None;
        };
        let exported = manifest
            .exports
            .models
            .iter()
            .find(|model| &model.name == type_name)
            .map(|model| model.fields.clone())
            .or_else(|| {
                manifest
                    .exports
                    .classes
                    .iter()
                    .find(|class| &class.name == type_name)
                    .map(|class| class.fields.clone())
            });
        exported.or_else(|| {
            manifest.contract_metadata.api.iter().find_map(|api| {
                api.modules
                    .iter()
                    .flat_map(|module| &module.declarations)
                    .find_map(|declaration| match declaration {
                        ApiDeclaration::Model(model) if &model.name == type_name => Some(model.fields.clone()),
                        ApiDeclaration::Class(class) if &class.name == type_name => Some(class.fields.clone()),
                        _ => None,
                    })
            })
        })
    }

    /// Type a const reference passed as a construction's field argument by the const's own representation.
    ///
    /// An exported const reference carries no type, and a `str` or `bytes` const is a static value that a field of that
    /// type takes as a converted copy. The field's declared type says which representation the const has.
    fn type_const_field_argument(&self, library: &str, fields: &[FieldExport], field: &str, argument: &mut TypedExpr) {
        let Some(declared) = fields.iter().find(|candidate| candidate.name == field) else {
            return;
        };
        argument.ty = match self.lower_pub_manifest_type_ref(library, &declared.ty) {
            IrType::String => IrType::StaticStr,
            IrType::Bytes => IrType::StaticBytes,
            _ => return,
        };
    }

    /// Lower an exported default that constructs a model or class of `library` as a call of the type's public path.
    ///
    /// Returns `None` when an argument is not a named field or does not lower, so the default is not materialized.
    pub(in crate::lower) fn lower_pub_default_construction(
        &mut self,
        library: &str,
        path: &[String],
        args: &[ParamDefaultCallArgExport],
    ) -> Option<TypedExpr> {
        let fields = self.pub_constructed_type_fields(library, path)?;
        let args = args
            .iter()
            .map(|arg| {
                let name = arg.name.clone()?;
                let mut expr = self.lower_pub_default_expr(library, &arg.value)?;
                if matches!(arg.value, ParamDefaultExport::ConstRef(_)) {
                    self.type_const_field_argument(library, &fields, &name, &mut expr);
                }
                Some(IrCallArg {
                    name: Some(name),
                    kind: IrCallArgKind::Named,
                    expr,
                })
            })
            .collect::<Option<Vec<_>>>()?;
        self.pub_construction_call(library, path, args)
    }

    /// Lower a preset of a dependency's partial that constructs one of the dependency's models or classes.
    ///
    /// A preset records the constructed type by the name the consumer's checker bound it under, either `Card` or the
    /// dependency-qualified `pub::shapes::Card`. Returns `None` when the name is not a model or class of `library`, or
    /// a field does not lower.
    pub(in crate::lower) fn lower_pub_preset_construction(
        &mut self,
        library: &str,
        type_name: &str,
        fields: &[(String, CheckedPresetValue)],
    ) -> Option<TypedExpr> {
        let dependency_prefix = format!("pub::{library}::");
        let path = type_name
            .strip_prefix(&dependency_prefix)
            .unwrap_or(type_name)
            .split("::")
            .map(str::to_string)
            .collect::<Vec<_>>();
        let declared = self.pub_constructed_type_fields(library, &path)?;
        let args = fields
            .iter()
            .map(|(field, value)| {
                let mut expr = self.lower_external_partial_preset(library, value)?;
                if matches!(value, CheckedPresetValue::ConstRef(_)) {
                    self.type_const_field_argument(library, &declared, field, &mut expr);
                }
                Some(IrCallArg {
                    name: Some(field.clone()),
                    kind: IrCallArgKind::Named,
                    expr,
                })
            })
            .collect::<Option<Vec<_>>>()?;
        self.pub_construction_call(library, &path, args)
    }

    /// Build a construction of the model or class at `path` in `library` from named field arguments.
    ///
    /// The callee is the type itself, reached through the dependency's public path, so emission constructs it from the
    /// dependency's published constructor metadata, which supplies the declared defaults of the fields the call leaves
    /// out, whether or not the consumer imports the type.
    fn pub_construction_call(&self, library: &str, path: &[String], args: Vec<IrCallArg>) -> Option<TypedExpr> {
        let type_name = path.last()?.clone();
        let ty = self.pub_external_type(library, IrType::Struct(type_name.clone()));
        let mut canonical_path = vec!["pub".to_string(), library.to_string()];
        canonical_path.extend(path.iter().cloned());
        Some(TypedExpr::new(
            IrExprKind::Call {
                func: Box::new(TypedExpr::new(
                    IrExprKind::Var {
                        name: type_name,
                        access: VarAccess::Read,
                        ref_kind: VarRefKind::TypeName,
                    },
                    ty.clone(),
                )),
                type_args: Vec::new(),
                args,
                callable_signature: None,
                canonical_path: Some(canonical_path),
            },
            ty,
        ))
    }
}
