//! The rustc driver callbacks that declare planned items and verify the compilation.

use super::*;

impl Callbacks {
    /// Replace the session's plan with one refined against native metadata; MIR building reads the replacement.
    pub(super) fn replace_plan(&mut self, tcx: TyCtxt<'_>, plan: Plan) {
        self.plan = Arc::new(plan);
        match PLAN.lock() {
            Ok(mut slot) => *slot = Some(Arc::new(ActivePlan::new(Arc::clone(&self.plan)))),
            Err(_) => tcx.dcx().fatal(DriverError::State.to_string()),
        }
    }

    /// Declare checked callables before their program entries, retaining typed function-span failures across fatal
    /// errors. A program groups its callables by source file, one module each, re-exported at the crate root:
    /// rustc forms codegen units per module, so one root module would optimize the whole unit on a single thread.
    pub(super) fn declare_callables(
        &self,
        compiler: &interface::Compiler,
        krate: &mut ast::Crate,
        sources: &Sources<'_>,
    ) {
        let mut grouped: std::collections::BTreeMap<&str, Vec<Box<ast::Item>>> = std::collections::BTreeMap::new();
        for function in &self.plan.functions {
            let span = match sources.span(&function.span) {
                Ok(span) => span,
                Err(error) => {
                    let message = error.to_string();
                    if let Ok(mut failure) = FAILURE.lock() {
                        *failure = Some(error);
                    }
                    compiler.sess.dcx().fatal(message);
                }
            };
            let mut item = match declarations::function(&self.plan, function, span) {
                Ok(item) => item,
                Err(error) => compiler.sess.dcx().fatal(error.to_string()),
            };
            if !function.attributes.is_empty() {
                item.attrs.extend(crate::native_attributes::attributes(
                    &compiler.sess.psess.attr_id_generator,
                    function,
                    span,
                ));
                item.tokens = Some(crate::native_attributes::function_tokens(&self.plan, function, span));
            }
            if self.plan.tests.contains(&function.name) {
                item.attrs.push(crate::test_unit::attribute(
                    &compiler.sess.psess.attr_id_generator,
                    span,
                ));
            }
            if self.partition {
                grouped.entry(function.span.file.as_str()).or_default().push(item);
            } else {
                krate.items.push(item);
            }
        }
        for (index, items) in grouped.into_values().enumerate() {
            krate
                .items
                .extend(declarations::partition_module(index, items, krate.spans.inner_span));
        }
        if !self.plan.programs.is_empty() {
            krate.items.push(
                match crate::program_entries::dispatcher(&self.plan, krate.spans.inner_span) {
                    Ok(item) => item,
                    Err(error) => compiler.sess.dcx().fatal(error.to_string()),
                },
            );
        }
    }

    /// Declare compiler-owned bridges before publishing the library's public root names.
    pub(super) fn declare_bridges_and_exports(&self, compiler: &interface::Compiler, krate: &mut ast::Crate) {
        match native_bridge_items(&self.plan, krate.spans.inner_span) {
            Ok(items) => krate.items.extend(items),
            Err(error) => compiler.sess.dcx().fatal(error.to_string()),
        }
        // A library unit re-exports its public names at the crate root (see `library_exports`).
        match crate::library_exports::items(&self.plan, &self.library_facades, krate.spans.inner_span) {
            Ok(items) => krate.items.extend(items),
            Err(error) => compiler.sess.dcx().fatal(error.to_string()),
        }
    }

    /// Declare static storage before gathering the native roots required by every declaration already emitted.
    pub(super) fn declare_storage_and_externs(
        &self,
        compiler: &interface::Compiler,
        krate: &mut ast::Crate,
        sources: &Sources<'_>,
    ) {
        match native_static_items(&self.plan, sources) {
            Ok(items) => krate.items.extend(items),
            Err(error) => compiler.sess.dcx().fatal(error.to_string()),
        }
        let mut roots = native_roots(&self.plan);
        roots.extend(crate::native_crates::declaration_roots(krate));
        // `std` is already declared by the prelude injection; a second `extern crate std;` would collide with it.
        for root in roots
            .into_iter()
            .chain(self.shared.iter().map(String::as_str))
            .filter(|root| !matches!(*root, "std" | "crate"))
        {
            krate
                .items
                .push(declarations::external_crate(root, krate.spans.inner_span));
        }
    }
}

impl rustc_driver::Callbacks for Callbacks {
    /// Install the source-free root and the MIR query override. `INCAN_NATIVE_TIMING` opts into rustc's phase timings
    /// into the trace sink without enabling unstable command-line options or changing compilation policy.
    /// The source map owns one filename copied from the plan's shared file handle.
    fn config(&mut self, config: &mut interface::Config) {
        self.timing.mark("rustc_options");
        let mut search = std::mem::take(&mut self.search);
        config
            .opts
            .search_paths
            .retain(|path| !search.iter().any(|held| held.kind == path.kind && held.dir == path.dir));
        search.append(&mut config.opts.search_paths);
        config.opts.search_paths = search;
        if !self.features.is_empty() {
            // Only a unit whose description declares unstable features may enable them, and only the declared ones.
            config.opts.unstable_features = rustc_feature::UnstableFeatures::Allow;
        }
        self.timing.configure(config);
        config.input = Input::Str {
            name: FileName::Custom(SYNTHETIC_SOURCE_NAME.to_owned()),
            input: String::new(),
        };
        config.override_queries = Some(|_session, providers| {
            crate::native_timing::provide_codegen(providers);
            providers.queries.mir_built = mir_built;
            providers.queries.type_of = native_alias_type;
            providers.queries.fn_sig = native_function_signature;
        });
    }

    /// Add source-backed layouts, callable signatures, public bridges, and external crates in dependency order.
    /// Structural slice owners use the implicit standard core crate rather than introducing another extern.
    fn after_crate_root_parsing(
        &mut self,
        compiler: &interface::Compiler,
        krate: &mut ast::Crate,
    ) -> rustc_driver::Compilation {
        self.timing.mark("rustc_session");
        let sources = Sources::new(compiler.sess.source_map());

        // ---- Declared unstable features ----
        for feature in &self.features {
            krate.attrs.push(ast::attr::mk_attr_nested_word(
                &compiler.sess.psess.attr_id_generator,
                ast::AttrStyle::Inner,
                ast::Safety::Default,
                rustc_span::sym::feature,
                rustc_span::Symbol::intern(feature),
                krate.spans.inner_span,
            ));
        }

        // ---- Source-backed native declarations ----
        if let Err(error) =
            add_source_backed_declarations(&self.plan, &sources, &compiler.sess.psess.attr_id_generator, krate)
        {
            compiler.sess.dcx().fatal(error.to_string());
        }

        // ---- Callable declarations and canonical entries ----
        self.declare_callables(compiler, krate, &sources);

        // ---- Compiler-owned bridges and public library names ----
        self.declare_bridges_and_exports(compiler, krate);

        // ---- Computed storage and exact native crate roots ----
        self.declare_storage_and_externs(compiler, krate, &sources);
        self.timing.mark("incan_declarations");
        rustc_driver::Compilation::Continue
    }

    /// Verify external signatures and synthesized clone bounds against native metadata before building planned MIR.
    fn after_expansion(&mut self, _compiler: &interface::Compiler, tcx: TyCtxt<'_>) -> rustc_driver::Compilation {
        self.timing.mark("rustc_expansion");
        if let Err(error) = verify_library_argument_profiles(tcx, &self.plan) {
            refuse(tcx, error);
        }
        match instantiate_generic_externals(tcx, &self.plan) {
            Ok(Some(plan)) => self.replace_plan(tcx, plan),
            Ok(None) => {}
            Err(error) => refuse(tcx, error),
        }
        match verify_library_layouts(tcx, &self.plan) {
            Ok(Some(plan)) => self.replace_plan(tcx, plan),
            Ok(None) => {}
            Err(error) => refuse(tcx, error),
        }
        for value in self.plan.enums.iter().filter(|value| !value.library_item.is_empty()) {
            if let Err(error) = verify_library_enum(tcx, value) {
                refuse(tcx, error);
            }
        }
        if let Err(error) = crate::cloneable_closures::verify_captures(tcx, &self.plan) {
            refuse(tcx, error);
        }
        if let Err(error) = crate::statics::verify_method_results(tcx, &self.plan) {
            refuse(tcx, error);
        }
        if let Err(error) = verify_planned_clone_bounds(tcx, &self.plan) {
            refuse(tcx, error);
        }
        for external in &self.plan.externals {
            let def = match callees::planned_external(tcx, external) {
                Ok(def) => def,
                Err(error) => refuse(tcx, error),
            };
            let args = match callees::external_arguments(tcx, def, external) {
                Ok(args) => args,
                Err(error) => refuse(tcx, error),
            };
            if let Err(error) = callees::verify_bounds(tcx, def, args) {
                refuse(tcx, error);
            }
            if let Err(error) = callees::verify_return_abi(tcx, def, args, &external.return_type, &external.path) {
                refuse(tcx, error);
            }
            let signature = tcx
                .fn_sig(def)
                .instantiate(tcx, args)
                .map(|signature| tcx.instantiate_bound_regions_with_erased(signature));
            let raw_signature = signature.skip_normalization();
            // A hidden concrete factory layout is not the SDK method's opaque return ABI. Borrow checking still
            // requires the opaque type, so revealing its layout here would admit a broken MIR call destination.
            if has_opaque_return_abi(tcx, raw_signature.output())
                && raw_signature.output() != native_type_or_refuse(tcx, &external.return_type)
            {
                refuse(
                    tcx,
                    PlanError::Unsupported {
                        function: external.path.clone(),
                        reason: "opaque SDK trait return ABI differs from its concrete carrier".into(),
                    },
                );
            }
            // Ordinary associated outputs still normalize under the checked concrete Self implementation.
            let signature =
                tcx.normalize_erasing_regions(rustc_middle::ty::TypingEnv::fully_monomorphized(), signature);
            let parameters: Vec<_> = external
                .parameters
                .iter()
                .map(|ty| tcx.erase_and_anonymize_regions(native_type_or_refuse(tcx, ty)))
                .collect();
            let output = tcx.erase_and_anonymize_regions(native_type_or_refuse(tcx, &external.return_type));
            if signature.inputs() != parameters
                || signature.output() != output
                || signature.c_variadic()
                || signature.safety().is_unsafe()
            {
                refuse(
                    tcx,
                    PlanError::Invalid {
                        function: external.path.clone(),
                        reason: format!(
                            "external metadata differs from the admitted safe scalar signature: native {signature:?}, admitted {parameters:?} -> {output:?}"
                        ),
                    },
                );
            }
        }
        self.timing.mark("incan_native_verification");
        rustc_driver::Compilation::Continue
    }

    /// Snapshot dynamic library generations after analysis, before codegen and linking can publish an executable.
    fn after_analysis(&mut self, _compiler: &interface::Compiler, tcx: TyCtxt<'_>) -> rustc_driver::Compilation {
        self.timing.mark("rustc_analysis");
        if let Err(error) = crate::native_plan_report::write(&self.plan, tcx) {
            tcx.dcx().fatal(error);
        }
        if self.development {
            self.loaded = match crate::native_shared::loaded(tcx) {
                Ok(loaded) => loaded,
                Err(error) => tcx.dcx().fatal(error.to_string()),
            };
        }
        self.timing.mark("dynamic_library_snapshot");
        crate::native_timing::record_front_end(self.rustc_started.elapsed());
        self.codegen_started = Some(std::time::Instant::now());
        rustc_driver::Compilation::Continue
    }
}
