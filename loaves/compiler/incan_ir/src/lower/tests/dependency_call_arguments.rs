//! Arguments of calls into a `pub::` dependency: the defaults an imported partial's target declares for the
//! parameters the partial leaves open (#1760), and the dependency's own union for a `Some(member)` argument (#1743).

use std::collections::HashMap;
use std::sync::Arc;

use super::*;
use crate::decl::FunctionParamDefault;
use crate::expr::IrCallArg;
use incan_frontend::api_metadata::{
    CHECKED_API_METADATA_SCHEMA_VERSION, CheckedApiMetadataPackage, collect_checked_api_metadata,
    materialize_api_alias_projections, materialize_checked_api_public_namespaces,
};
use incan_frontend::library_exports::collect_checked_public_exports;
use incan_frontend::library_manifest::LibraryManifest;
use incan_frontend::library_manifest_index::{
    LibraryArtifactMetadata, LibraryManifestIndex, LibraryManifestIndexEntry,
};
use incan_frontend::provider::ProviderPlan;

/// The dependency's library name, as consumers import it through `pub::`.
const LIBRARY: &str = "modulelib";

/// Check each provider module and publish the dependency's manifest the way a library build does: exports from the
/// root module, checked API metadata for every module, and the identity graph extended over the public namespaces.
fn provider_index(modules: &[(&[&str], &str)]) -> Result<LibraryManifestIndex, String> {
    let mut api_modules = Vec::new();
    let mut exports_by_module = Vec::new();
    let mut root_exports = Vec::new();
    for (module_path, source) in modules {
        let module_path = module_path
            .iter()
            .map(|segment| segment.to_string())
            .collect::<Vec<_>>();
        let tokens = lexer::lex(source).map_err(|errors| format!("provider lex failed: {errors:?}"))?;
        let program = parser::parse(&tokens).map_err(|errors| format!("provider parse failed: {errors:?}"))?;
        let mut checker = TypeChecker::new();
        checker.set_current_module_path(Some(module_path.clone()));
        checker
            .check_program(&program)
            .map_err(|errors| format!("provider module {module_path:?} failed to check: {errors:?}"))?;
        let exports = collect_checked_public_exports(&program, &checker);
        api_modules.push(collect_checked_api_metadata(&program, &checker, module_path.clone()));
        if module_path == ["lib"] {
            root_exports = exports.clone();
        }
        exports_by_module.push((module_path, exports));
    }
    materialize_api_alias_projections(&mut api_modules);
    let mut api = CheckedApiMetadataPackage {
        schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
        package: None,
        modules: api_modules,
        public_namespaces: Vec::new(),
    };
    materialize_checked_api_public_namespaces(&mut api).map_err(|error| format!("{error:?}"))?;
    let mut manifest = LibraryManifest::from_checked_exports(LIBRARY, "0.1.0", &root_exports);
    manifest
        .contract_metadata
        .identity_graph
        .extend_checked_api_exports(LIBRARY, &api, &exports_by_module)
        .map_err(|error| format!("{error:?}"))?;
    manifest.contract_metadata.api = Some(api);
    Ok(LibraryManifestIndex::from_entries(HashMap::from([(
        LIBRARY.to_string(),
        LibraryManifestIndexEntry::Loaded {
            manifest: Box::new(manifest),
            metadata: LibraryArtifactMetadata::from_crate_root(
                LIBRARY,
                LIBRARY,
                std::env::temp_dir().join("incan_dependency_call_arguments"),
            ),
        },
    )])))
}

/// Check and lower a consumer against the dependency index, keeping the lowered program.
fn lower_consumer(source: &str, index: LibraryManifestIndex) -> Result<IrProgram, String> {
    let tokens = lexer::lex(source).map_err(|errors| format!("consumer lex failed: {errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("consumer parse failed: {errors:?}"))?;
    let mut checker = TypeChecker::new();
    checker.set_library_manifest_index(index.clone());
    checker
        .check_program(&program)
        .map_err(|errors| format!("consumer failed to check: {errors:?}"))?;
    let mut lowering = AstLowering::new_with_type_info(checker.type_info().clone());
    lowering.set_provider_plan(Some(Arc::new(ProviderPlan::for_library_index(index))));
    lowering
        .lower_program(&program)
        .map_err(|errors| format!("consumer failed to lower: {errors:?}"))
}

/// Return the call the named function returns, with the callable signature it carries.
fn returned_call<'a>(
    ir: &'a IrProgram,
    function_name: &str,
) -> Result<(&'a [IrCallArg], &'a FunctionSignature), String> {
    let function = ir
        .declarations
        .iter()
        .find_map(|decl| match &decl.kind {
            IrDeclKind::Function(function) if function.name == function_name => Some(function),
            _ => None,
        })
        .ok_or_else(|| format!("missing function `{function_name}`"))?;
    let Some(IrStmt {
        kind: IrStmtKind::Return(Some(call)),
        ..
    }) = function.body.last()
    else {
        return Err(format!("`{function_name}` must end in a return of the call"));
    };
    match &call.kind {
        IrExprKind::Call {
            args,
            callable_signature: Some(signature),
            ..
        } => Ok((args.as_slice(), signature)),
        _ => Err(format!(
            "`{function_name}` must return a call carrying its signature, got {call:?}"
        )),
    }
}

/// Return the default expression the call the named function returns fills in for `param`.
fn call_default<'a>(ir: &'a IrProgram, function_name: &str, param: &str) -> Result<&'a TypedExpr, String> {
    let (_, signature) = returned_call(ir, function_name)?;
    let declared = signature
        .params
        .iter()
        .find(|declared| declared.name == param)
        .ok_or_else(|| format!("`{function_name}`'s call must keep `{param}`: {signature:?}"))?;
    match &declared.default {
        Some(FunctionParamDefault::Source(default)) => Ok(default.as_ref()),
        other => Err(format!(
            "`{function_name}`'s call must fill `{param}` from its target's default, got {other:?} in {signature:?}"
        )),
    }
}

/// #1760: `default_build()` calls `pub default_build = partial build(size=3)` without `label`, which `build` declares
/// as `label: str = "flat"`. The partial's export marks `label` as defaulted without restating the default, so the call
/// reads it from `build`; the same holds for a partial reached through a nested namespace.
#[test]
fn imported_partial_call_fills_its_targets_residual_default_issue1760() -> Result<(), String> {
    let index = provider_index(&[
        (
            &["hyperquant", "index"],
            r#"
pub const DEFAULT_SIZE: int = 4


pub def build_index(size: int, label: str = "idx") -> str:
    return f"{label} of size {size}"


pub default_index = partial build_index(size=DEFAULT_SIZE)
"#,
        ),
        (
            &["lib"],
            r#"
pub def build(size: int, label: str = "flat") -> str:
    return f"{label} of size {size}"


pub default_build = partial build(size=3)
"#,
        ),
    ])?;
    let ir = lower_consumer(
        r#"
from pub::modulelib import default_build, hyperquant


def root_partial() -> str:
    return default_build()


def nested_partial() -> str:
    return hyperquant.default_index()
"#,
        index,
    )?;

    for (function_name, expected) in [("root_partial", "flat"), ("nested_partial", "idx")] {
        let default = call_default(&ir, function_name, "label")?;
        assert!(
            matches!(&default.kind, IrExprKind::Literal(crate::expr::Literal::StaticStr(value)) if value == expected),
            "`{function_name}` fills `label` with its target's `{expected}`, got {default:?}"
        );
    }
    Ok(())
}

/// #1743: `describe(Some("plain"))` passes `Some(member)` where `pub::modulelib`'s `describe` takes
/// `Option[Kind | str]`. The constructor is instantiated at that union, and the union is the one the dependency
/// declares, so the constructor's parameter carries the dependency as its owner rather than a consumer-local union.
#[test]
fn some_member_argument_takes_the_dependency_owned_union_issue1743() -> Result<(), String> {
    let index = provider_index(&[(
        &["lib"],
        r#"
@derive(Clone)
pub type Kind = newtype str


pub def describe(value: Option[Kind | str]) -> str:
    return "described"
"#,
    )])?;
    let ir = lower_consumer(
        r#"
from pub::modulelib import Kind, describe


def plain() -> str:
    return describe(Some("plain"))


def kind() -> str:
    return describe(Some(Kind("k")))
"#,
        index,
    )?;

    for function_name in ["plain", "kind"] {
        let (args, _) = returned_call(&ir, function_name)?;
        let [argument] = args else {
            return Err(format!("`{function_name}` passes one argument, got {args:?}"));
        };
        let IrExprKind::Call {
            callable_signature: Some(constructor),
            ..
        } = &argument.expr.kind
        else {
            return Err(format!(
                "`{function_name}` passes a `Some` call carrying its instantiation, got {:?}",
                argument.expr
            ));
        };
        let [payload] = constructor.params.as_slice() else {
            return Err(format!("`Some` takes one payload: {constructor:?}"));
        };
        assert!(
            matches!(&payload.ty, IrType::ExternalUnion { library, .. } if library == LIBRARY),
            "`{function_name}`: the payload is injected into the dependency's union, got {:?}",
            payload.ty
        );
        assert!(
            matches!(&argument.expr.ty, IrType::Option(inner) if matches!(inner.as_ref(), IrType::ExternalUnion { .. })),
            "`{function_name}`: the argument is an option of the dependency's union, got {:?}",
            argument.expr.ty
        );
    }
    Ok(())
}
