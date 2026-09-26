//! The callable surface of a public partial imported from a provider's checked API.
//!
//! A public partial's exported parameters mark every defaulted parameter, but carry a default expression for none of
//! them: a preset reaches the call from the partial's preset metadata, while a parameter the partial leaves open keeps
//! the default its target declares, which the partial's export does not restate. A consumer calling
//! `default_build()` over `pub default_build = partial build(size=3)` and `pub def build(size: int, label: str =
//! "flat")` must therefore read `label`'s default from `build` (#1760).

use std::collections::HashSet;

use super::super::AstLowering;
use incan_frontend::api_metadata::{ApiPartial, CheckedApiMetadataPackage, partial_export_from_api};
use incan_frontend::library_manifest::FunctionExport;

/// How many partial-over-partial hops a residual default is followed through before the search gives up.
///
/// The checker refuses partial cycles, so this bounds only a malformed provider manifest.
const MAX_PARTIAL_TARGET_HOPS: usize = 16;

impl AstLowering {
    /// Convert one checked API partial into the callable export a call through it is planned against.
    ///
    /// Each parameter the partial leaves open and marks as defaulted takes the default its target declares; presets
    /// stay without a default expression, because the call receives their values from the partial's own preset
    /// metadata. `module_path` is the checked module that declares the partial, against which a target the partial
    /// names unqualified is resolved first.
    pub(in crate::lower::expr) fn api_partial_function_export(
        api: &CheckedApiMetadataPackage,
        module_path: &[String],
        partial: &ApiPartial,
        hops: usize,
    ) -> FunctionExport {
        let exported = partial_export_from_api(partial);
        let mut function = FunctionExport {
            name: exported.name,
            emitted_name: None,
            type_params: exported.type_params,
            params: exported.params,
            return_type: exported.return_type,
            is_async: exported.is_async,
        };
        let presets = partial
            .presets
            .iter()
            .map(|preset| preset.name.as_str())
            .collect::<HashSet<_>>();
        let is_open_default = |name: &str, has_default: bool, has_expression: bool| {
            has_default && !has_expression && !presets.contains(name)
        };
        if hops >= MAX_PARTIAL_TARGET_HOPS
            || !function
                .params
                .iter()
                .any(|param| is_open_default(param.name.as_str(), param.has_default, param.default.is_some()))
        {
            return function;
        }
        let Some(target) = Self::api_partial_target_export(api, module_path, &partial.target_path, hops + 1) else {
            return function;
        };
        for param in &mut function.params {
            if !is_open_default(param.name.as_str(), param.has_default, param.default.is_some()) {
                continue;
            }
            if let Some(target_param) = target
                .params
                .iter()
                .find(|target_param| target_param.name == param.name)
            {
                param.default = target_param.default.clone();
            }
        }
        function
    }

    /// Resolve the callable a partial targets: first within the partial's own module, then as the path is written.
    fn api_partial_target_export(
        api: &CheckedApiMetadataPackage,
        module_path: &[String],
        target_path: &[String],
        hops: usize,
    ) -> Option<FunctionExport> {
        let module_local = (target_path.len() == 1).then(|| {
            let mut path = module_path.to_vec();
            path.extend(target_path.iter().cloned());
            path
        });
        module_local
            .and_then(|path| Self::api_function_export_for_target_path_within(api, &path, hops))
            .or_else(|| Self::api_function_export_for_target_path_within(api, target_path, hops))
    }
}
