//! Dependency parameter defaults that construct one of the dependency's public models or classes.
//!
//! A dependency carries such a default in its manifest as a call of the type's public path (#1771), the same record a
//! helper-function default has. A consumer that omits the argument receives the construction at its own call site as
//! a call whose callee is the type, reached through the dependency's public path, so the construction does not rely
//! on the consumer importing the type. The fields the call leaves out take the defaults the type declares.

use super::super::super::TypedExpr;
use super::super::super::expr::{IrCallArg, IrCallArgKind, IrExprKind, VarAccess, VarRefKind};
use super::super::super::types::IrType;
use super::super::AstLowering;
use incan_frontend::api_metadata::ApiDeclaration;
use incan_frontend::library_manifest::ParamDefaultCallArgExport;
use incan_frontend::library_manifest_index::LibraryManifestIndexEntry;

impl AstLowering {
    /// Return whether an exported default's call path names a model or class that `library` declares.
    ///
    /// A path that also names a function of the library is a helper call, whatever else shares its name.
    pub(in crate::lower) fn pub_default_path_names_constructed_type(&self, library: &str, path: &[String]) -> bool {
        let Some(type_name) = path.last() else {
            return false;
        };
        if self.pub_function_export_for_path(library, path).is_some() {
            return false;
        }
        let Some(plan) = self.provider_plan.as_deref() else {
            return false;
        };
        let index = plan.library_manifest_index();
        let Some(LibraryManifestIndexEntry::Loaded { manifest, .. }) = index.get(library) else {
            return false;
        };
        manifest.exports.models.iter().any(|model| &model.name == type_name)
            || manifest.exports.classes.iter().any(|class| &class.name == type_name)
            || manifest.contract_metadata.api.iter().any(|api| {
                api.modules
                    .iter()
                    .flat_map(|module| &module.declarations)
                    .any(|declaration| match declaration {
                        ApiDeclaration::Model(model) => &model.name == type_name,
                        ApiDeclaration::Class(class) => &class.name == type_name,
                        _ => false,
                    })
            })
    }

    /// Lower an exported default that constructs a model or class of `library` as a call of the type's public path.
    ///
    /// The callee is the type itself, so emission constructs it from the dependency's published constructor metadata,
    /// which supplies the declared defaults of the fields the call leaves out. Returns `None` when an argument is not
    /// a named field or does not lower, so the default is not materialized.
    pub(in crate::lower) fn lower_pub_default_construction(
        &mut self,
        library: &str,
        path: &[String],
        args: &[ParamDefaultCallArgExport],
    ) -> Option<TypedExpr> {
        let type_name = path.last()?.clone();
        let args = args
            .iter()
            .map(|arg| {
                Some(IrCallArg {
                    name: Some(arg.name.clone()?),
                    kind: IrCallArgKind::Named,
                    expr: self.lower_pub_default_expr(library, &arg.value)?,
                })
            })
            .collect::<Option<Vec<_>>>()?;
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
