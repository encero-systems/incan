//! Explicit checked native demand facts; missing old metadata remains unknown (#1337/#1698).

use std::collections::{BTreeMap, BTreeSet};

use incan_frontend::ParsedModule;
use incan_frontend::ast::{CallArg, Declaration, Expr, FunctionDecl, ImportKind, Literal, ParamKind, Statement, Type};
use incan_provider::ProviderPlan;
use incan_provider::inventory::provider_source_used_module_paths;
use incan_provider::requirements::ProjectRequirements;
use oven_model::manifest::ProjectManifest;
use serde::{Deserialize, Serialize};

use super::{invalid, source_relative};
use crate::error::CliResult;
use crate::rust_inspect_workspace::collect_rust_inspect_derive_probe_paths;

mod ordinary_source;
use ordinary_source::OrdinarySourceDemands;

/// Missing facts are unknown, never a declaration that ABI or macro requirements are empty.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckedNativeDemands {
    #[serde(default)]
    observed: Option<NativeDemandFacts>,
}

/// Versioned compiler-captured facts associated with the same successfully checked loaded module closure.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct NativeDemandFacts {
    schema_version: u32,
    source_modules: BTreeMap<String, Vec<String>>,
    used_module_paths: BTreeSet<Vec<String>>,
    public_dependency_modules: BTreeSet<Vec<String>>,
    provider_contracts: BTreeMap<String, String>,
    rust_abi_queries: BTreeSet<String>,
    /// Exact item imports covered by the scalar sysroot walk; absent old facts grant no expanded coverage.
    #[serde(default)]
    scalar_sysroot_imports: Option<BTreeSet<String>>,
    /// Exact scalar item imports from sysroot or manifest-declared Rust dependencies; absent old facts stay narrow.
    #[serde(default)]
    scalar_native_imports: Option<BTreeSet<String>>,
    /// Manifest-declared aliases actually imported by the checked source, excluding sysroot.
    #[serde(default)]
    declared_native_crates: Option<BTreeSet<String>>,
    rust_derive_probe_paths: BTreeSet<String>,
    vocab_manifest: ManifestDemand,
    c_manifest: ManifestDemand,
    dependency_aliases: BTreeSet<String>,
    stdlib_facets: BTreeSet<String>,
    physical_projections: bool,
    unsupported_source: BTreeSet<String>,
    /// Loaded ordinary source has explicit native imports and an independently selected own-source namespace.
    /// This is input coverage, not evidence of successful type checking or a native execution grant.
    #[serde(default)]
    ordinary_source: Option<OrdinarySourceDemands>,
}

/// Distinguish explicit observed absence from a missing serialized manifest-demand field.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "manifest", deny_unknown_fields)]
enum ManifestDemand {
    Absent,
    Declared(serde_json::Value),
}

impl NativeDemandFacts {
    /// Keep new coverage coupled and consistent with the retained sysroot projection and imported crate roots.
    fn validate_scalar_coverage(&self) -> CliResult<()> {
        match (&self.scalar_native_imports, &self.declared_native_crates) {
            (None, None) => Ok(()),
            (Some(imports), Some(crates)) => {
                let sysroot = imports
                    .iter()
                    .filter(|path| path.starts_with("std::"))
                    .cloned()
                    .collect();
                let declared = imports
                    .iter()
                    .filter(|path| !path.starts_with("std::"))
                    .map(|path| path.split_once("::").map(|(root, _)| root.to_string()))
                    .collect::<Option<BTreeSet<_>>>();
                if self.scalar_sysroot_imports.as_ref() != Some(&sysroot) || declared.as_ref() != Some(crates) {
                    return Err(invalid(
                        "scalar native imports disagree with their sysroot and declared crate coverage",
                    ));
                }
                Ok(())
            }
            _ => Err(invalid("scalar native import coverage is incomplete")),
        }
    }
}

impl ManifestDemand {
    /// Record actual declaration presence while preserving its complete typed manifest payload.
    fn capture<T: Serialize>(value: Option<&T>) -> CliResult<Self> {
        value.map_or(Ok(Self::Absent), |value| {
            serde_json::to_value(value)
                .map(Self::Declared)
                .map_err(|error| invalid(error.to_string()))
        })
    }
}

/// Capture real loaded AST, derive collector, ABI queries, manifest interop and exact provider-use facts.
///
/// The final contract caller invokes this only after successful checking. An earlier call may conservatively refuse
/// an unsupported parsed shape, but cannot certify a checked metadata owner or grant macro/namespace authority.
pub(crate) fn capture_checked_native_demands(
    project: &ProjectManifest,
    modules: &[ParsedModule],
    requirements: &ProjectRequirements,
    provider_plan: &ProviderPlan,
    rust_abi_queries: &BTreeSet<String>,
) -> CliResult<CheckedNativeDemands> {
    // ---- Actual provider usage ----
    let used_module_paths = provider_source_used_module_paths(modules);
    let public_dependency_modules = modules
        .iter()
        .flat_map(|module| &module.ast.declarations)
        .filter_map(|declaration| {
            let Declaration::Import(import) = &declaration.node else {
                return None;
            };
            match &import.kind {
                ImportKind::PubLibrary { library, path } | ImportKind::PubFrom { library, path, .. } => {
                    let mut canonical = vec!["pub".to_string(), library.clone()];
                    canonical.extend(path.iter().cloned());
                    Some(canonical)
                }
                _ => None,
            }
        })
        .collect::<BTreeSet<_>>();
    let selected =
        provider_plan.project_module_usage(used_module_paths.union(&public_dependency_modules).cloned().collect());
    let mut provider_contracts = BTreeMap::new();
    for record in selected
        .records()
        .filter(|record| !selected.used_modules(record).is_empty())
    {
        let contract = serde_json::json!({
            "identity": record.identity, "authority": record.authority,
            "namespace_claims": record.namespace_claims, "available": record.available,
            "enabled": record.enabled, "implementation_facets": record.implementation_facets,
            "checked_manifest": record.manifest.as_ref().map(|manifest| manifest.to_json_string()).transpose().map_err(|error| invalid(error.to_string()))?,
            "used_modules": selected.used_modules(record),
        });
        provider_contracts.insert(
            record.identity.stable_key(),
            oven_store::digest_bytes(&serde_json::to_vec(&contract).map_err(|error| invalid(error.to_string()))?),
        );
    }
    // ---- Authored structure proof ----
    let mut source_modules = BTreeMap::new();
    let mut unsupported_source = BTreeSet::new();
    let mut scalar_sysroot_imports = BTreeSet::new();
    let mut scalar_native_imports = BTreeSet::new();
    let mut declared_native_crates = BTreeSet::new();
    let declared_rust_crates = project
        .rust_dependencies()
        .keys()
        .filter(|name| !incan_lang::lang::stdlib::facets::is_facet(name))
        .cloned()
        .collect::<BTreeSet<_>>();
    for module in modules {
        let path = source_relative(project.project_root(), &module.file_path)?;
        if source_modules
            .insert(path.clone(), module.path_segments.clone())
            .is_some()
        {
            return Err(invalid("native demand source module is repeated"));
        }
        if module.ast.rust_module_path.is_some() {
            unsupported_source.insert(format!("{path}:rust.module"));
        }
        let mut imported = BTreeMap::new();
        for declaration in &module.ast.declarations {
            if let Declaration::Import(import) = &declaration.node
                && let ImportKind::RustFrom {
                    crate_name,
                    path: rust_path,
                    items,
                    version,
                    features,
                } = &import.kind
                && (crate_name == "std" || declared_rust_crates.contains(crate_name))
                && version.is_none()
                && features.is_empty()
            {
                for item in items {
                    let canonical = std::iter::once(crate_name.as_str())
                        .chain(rust_path.iter().map(String::as_str))
                        .chain(std::iter::once(item.name.as_str()))
                        .collect::<Vec<_>>()
                        .join("::");
                    if imported
                        .insert(item.alias.as_ref().unwrap_or(&item.name).clone(), canonical.clone())
                        .is_some()
                    {
                        unsupported_source.insert(format!("{path}:ambiguous-import"));
                    }
                    if crate_name == "std" {
                        scalar_sysroot_imports.insert(canonical.clone());
                    } else {
                        declared_native_crates.insert(crate_name.clone());
                    }
                    scalar_native_imports.insert(canonical);
                }
            }
        }
        let imported_names = imported.keys().cloned().collect();
        for declaration in &module.ast.declarations {
            let supported = match &declaration.node {
                Declaration::Docstring(_) => true,
                Declaration::Function(function) => scalar_function(function, &imported_names),
                Declaration::Import(import) => matches!(&import.kind, ImportKind::RustFrom {
                    crate_name, version, features, items, ..
                } if (crate_name == "std" || declared_rust_crates.contains(crate_name))
                    && version.is_none() && features.is_empty() && !items.is_empty()),
                _ => false,
            };
            if !supported {
                unsupported_source.insert(format!("{path}:{}..{}", declaration.span.start, declaration.span.end));
            }
        }
    }
    // ---- Complete observed demand contract ----
    Ok(CheckedNativeDemands {
        observed: Some(NativeDemandFacts {
            schema_version: 1,
            source_modules,
            used_module_paths,
            public_dependency_modules,
            provider_contracts,
            rust_abi_queries: rust_abi_queries.clone(),
            scalar_sysroot_imports: Some(scalar_sysroot_imports),
            scalar_native_imports: Some(scalar_native_imports),
            declared_native_crates: Some(declared_native_crates),
            rust_derive_probe_paths: collect_rust_inspect_derive_probe_paths(modules).into_iter().collect(),
            vocab_manifest: ManifestDemand::capture(project.vocab())?,
            c_manifest: ManifestDemand::capture(project.interop_c())?,
            dependency_aliases: requirements
                .dependencies
                .iter()
                .chain(&requirements.sdk_path_dependencies)
                .map(|dependency| dependency.crate_name.clone())
                .collect(),
            stdlib_facets: requirements.stdlib_facets.iter().cloned().collect(),
            physical_projections: !requirements.sdk_dependency_rebindings.is_empty()
                || !requirements.sdk_artifact_projections.is_empty(),
            unsupported_source,
            ordinary_source: Some(OrdinarySourceDemands::capture(project, modules, provider_plan)?),
        }),
    })
}

impl CheckedNativeDemands {
    /// Admit loaded ordinary source for checking while keeping every native, provider and macro demand explicit.
    /// Old scalar metadata retains its narrower coverage; no source fact alone certifies a checked publication.
    pub(crate) fn require_ordinary_source_inspection(&self) -> CliResult<()> {
        let facts = self
            .observed
            .as_ref()
            .ok_or_else(|| invalid("ordinary native demand coverage is unknown"))?;
        match &facts.ordinary_source {
            Some(source) => source.require_inspection(facts),
            None => self.require_source_inspection(),
        }
    }

    /// Require an actually observed, dependency-free scalar-function closure with no additional native demand.
    /// Wider valid source remains unsupported by this initial authority slice, rather than gaining fake empty demand.
    pub(crate) fn require_support_only(&self) -> CliResult<()> {
        let facts = self
            .observed
            .as_ref()
            .ok_or_else(|| invalid("ordinary native demand coverage is unknown"))?;
        facts.validate_scalar_coverage()?;
        if facts.schema_version != 1
            || facts.source_modules.is_empty()
            || !facts.used_module_paths.is_empty()
            || !facts.public_dependency_modules.is_empty()
            || !facts.provider_contracts.is_empty()
            || !facts.rust_abi_queries.is_empty()
            || facts
                .scalar_sysroot_imports
                .as_ref()
                .is_some_and(|paths| !paths.is_empty())
            || facts
                .scalar_native_imports
                .as_ref()
                .is_some_and(|paths| !paths.is_empty())
            || facts
                .declared_native_crates
                .as_ref()
                .is_some_and(|crates| !crates.is_empty())
            || !facts.rust_derive_probe_paths.is_empty()
            || !matches!(facts.vocab_manifest, ManifestDemand::Absent)
            || !matches!(facts.c_manifest, ManifestDemand::Absent)
            || !facts.dependency_aliases.is_empty()
            || !facts.stdlib_facets.is_empty()
            || facts.physical_projections
            || !facts.unsupported_source.is_empty()
        {
            return Err(invalid(
                "ordinary native support-only coverage does not admit these checked demands",
            ));
        }
        Ok(())
    }

    /// Admit scalar sysroot and declared Rust item uses only when every import has a promised complete ABI query.
    /// This grants source inspection, never provider, derive, vocabulary or C execution authority. Final publication
    /// and replay must additionally validate every promised ABI item against the original checked metadata owner.
    pub(crate) fn require_source_inspection(&self) -> CliResult<()> {
        if self.require_support_only().is_ok() {
            return Ok(());
        }
        let facts = self
            .observed
            .as_ref()
            .ok_or_else(|| invalid("ordinary native demand coverage is unknown"))?;
        facts.validate_scalar_coverage()?;
        let imports = match (&facts.scalar_native_imports, &facts.declared_native_crates) {
            (Some(imports), Some(_)) => imports,
            (None, None) => facts
                .scalar_sysroot_imports
                .as_ref()
                .ok_or_else(|| invalid("ordinary sysroot source coverage is unknown"))?,
            _ => return Err(invalid("ordinary native source coverage is incomplete")),
        };
        if facts.schema_version != 1
            || facts.source_modules.is_empty()
            || imports.is_empty()
            || imports != &facts.rust_abi_queries
            || imports.iter().any(|path| {
                !path.starts_with("std::")
                    && !facts
                        .declared_native_crates
                        .as_ref()
                        .is_some_and(|crates| path.split_once("::").is_some_and(|(root, _)| crates.contains(root)))
            })
            || !facts.used_module_paths.is_empty()
            || !facts.public_dependency_modules.is_empty()
            || !facts.provider_contracts.is_empty()
            || !facts.rust_derive_probe_paths.is_empty()
            || !matches!(facts.vocab_manifest, ManifestDemand::Absent)
            || !matches!(facts.c_manifest, ManifestDemand::Absent)
            || !facts.dependency_aliases.is_empty()
            || !facts.stdlib_facets.is_empty()
            || facts.physical_projections
            || !facts.unsupported_source.is_empty()
        {
            return Err(invalid(
                "ordinary source inspection lacks complete checked demand coverage",
            ));
        }
        Ok(())
    }

    /// Require exact declared-crate coverage for ordinary replay; legacy sysroot facts grant no foreign imports.
    pub(super) fn require_declared_crates(&self, crates: &BTreeSet<String>) -> CliResult<()> {
        let facts = self
            .observed
            .as_ref()
            .ok_or_else(|| invalid("ordinary native demand coverage is unknown"))?;
        facts.validate_scalar_coverage()?;
        match &facts.declared_native_crates {
            Some(observed) if observed == crates => Ok(()),
            None if crates.is_empty() => Ok(()),
            _ => Err(invalid(
                "ordinary native source imports lack exact declared crate coverage",
            )),
        }
    }

    /// Validate the recorded facts against their same checked planning contract without treating unknown as empty.
    pub(super) fn validate_contract(
        &self,
        modules: &BTreeMap<String, Vec<String>>,
        used: &BTreeSet<Vec<String>>,
        abi: &BTreeSet<String>,
        dependencies: &BTreeSet<String>,
        facets: &BTreeSet<String>,
        source_inline_crates: &BTreeSet<String>,
    ) -> CliResult<()> {
        if let Some(facts) = &self.observed {
            facts.validate_scalar_coverage()?;
            if let Some(source) = &facts.ordinary_source {
                source.validate_contract(source_inline_crates)?;
            }
            if facts
                .declared_native_crates
                .as_ref()
                .is_some_and(|crates| !crates.is_subset(source_inline_crates))
            {
                return Err(invalid(
                    "native source imports disagree with their checked Rust crate declarations",
                ));
            }
        }
        if let Some(facts) = &self.observed
            && (facts.schema_version != 1
                || &facts.source_modules != modules
                || !facts.used_module_paths.is_subset(used)
                || &facts.rust_abi_queries != abi
                || &facts.dependency_aliases != dependencies
                || &facts.stdlib_facets != facets)
        {
            return Err(invalid(
                "native demand facts disagree with their checked planning contract",
            ));
        }
        Ok(())
    }
}

/// Walk primitive functions and explicit imported native calls without granting generic or implicit dispatch.
fn scalar_function(function: &FunctionDecl, imported: &BTreeSet<String>) -> bool {
    if !function.decorators.is_empty()
        || !function.surface_modifiers.is_empty()
        || !function.type_params.is_empty()
        || !scalar_type(&function.return_type.node)
    {
        return false;
    }
    // ---- Primitive signature and defaults ----
    let mut bindings = BTreeSet::new();
    for parameter in &function.params {
        if parameter.node.kind != ParamKind::Normal
            || !scalar_type(&parameter.node.ty.node)
            || parameter
                .node
                .default
                .as_ref()
                .is_some_and(|value| !scalar_expression(&value.node, &BTreeSet::new(), imported))
            || !bindings.insert(parameter.node.name.clone())
        {
            return false;
        }
    }
    // ---- Every body statement and expression ----
    for statement in &function.body {
        match &statement.node {
            Statement::Return(value)
                if value
                    .as_ref()
                    .is_none_or(|value| scalar_expression(&value.node, &bindings, imported)) => {}
            Statement::Assignment(value)
                if value.ty.as_ref().is_none_or(|ty| scalar_type(&ty.node))
                    && scalar_expression(&value.value.node, &bindings, imported) =>
            {
                bindings.insert(value.name.clone());
            }
            Statement::Expr(value) if matches!(value.node, Expr::Literal(Literal::String(_))) => {}
            Statement::Pass => {}
            _ => return false,
        }
    }
    true
}

/// Limit this initial proof to compiler primitive scalar annotations, with no nominal or generic dispatch.
fn scalar_type(ty: &Type) -> bool {
    matches!(ty, Type::Unit) || matches!(ty, Type::Simple(name) if matches!(name.as_str(), "int" | "float" | "bool"))
}

/// Walk all admitted scalar expression children; unknown expression kinds never infer empty native requirements.
fn scalar_expression(expression: &Expr, bindings: &BTreeSet<String>, imported: &BTreeSet<String>) -> bool {
    match expression {
        Expr::Literal(Literal::Int(_) | Literal::Float(_) | Literal::Bool(_)) => true,
        Expr::Ident(name) => bindings.contains(name) || imported.contains(name),
        Expr::Paren(inner) | Expr::Unary(_, inner) => scalar_expression(&inner.node, bindings, imported),
        Expr::Call(callee, types, args)
            if types.is_empty()
                && matches!(&callee.node, Expr::Ident(name) if imported.contains(name) && !bindings.contains(name)) =>
        {
            args.iter().all(
                |arg| matches!(arg, CallArg::Positional(value) if scalar_expression(&value.node, bindings, imported)),
            )
        }
        Expr::Binary(left, op, right)
            if !matches!(
                op,
                incan_frontend::ast::BinaryOp::MatMul
                    | incan_frontend::ast::BinaryOp::PipeForward
                    | incan_frontend::ast::BinaryOp::PipeBackward
                    | incan_frontend::ast::BinaryOp::In
                    | incan_frontend::ast::BinaryOp::NotIn
                    | incan_frontend::ast::BinaryOp::Is
                    | incan_frontend::ast::BinaryOp::IsNot
            ) =>
        {
            scalar_expression(&left.node, bindings, imported) && scalar_expression(&right.node, bindings, imported)
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests;
