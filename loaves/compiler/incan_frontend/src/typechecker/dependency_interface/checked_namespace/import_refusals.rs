//! RFC 176 import refusals, checked against a provider's source (cold) and against its published namespace (warm).

use super::*;

/// One source module of the program under test: its cache key, its logical path and its source.
struct Module {
    key: &'static str,
    path: &'static [&'static str],
    source: &'static str,
}

fn program(source: &str) -> Result<Program, String> {
    let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))
}

fn segments(path: &[&str]) -> Vec<String> {
    path.iter().map(|segment| (*segment).to_owned()).collect()
}

/// A checker for the module at `path` that knows every module's logical path.
fn configured(path: &[&str], modules: &[Module]) -> TypeChecker {
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(segments(path)));
    for module in modules {
        checker.register_dependency_module_path_segments(module.key, segments(module.path));
    }
    checker
}

/// The program's providers, parsed in order, each published after the providers before it.
struct Providers {
    sources: Vec<(&'static str, Program)>,
    published: Vec<Arc<ModuleDependencyInterface>>,
    empty: Program,
}

impl Providers {
    fn new(modules: &[Module]) -> Result<Self, String> {
        let mut sources: Vec<(&'static str, Program)> = Vec::new();
        let mut published = Vec::new();
        for module in modules {
            let parsed = program(module.source)?;
            let dependencies = sources.iter().map(|(key, program)| (*key, program)).collect::<Vec<_>>();
            let mut checker = configured(module.path, modules);
            checker
                .check_with_imports(&parsed, &dependencies)
                .map_err(|errors| format!("{}: {errors:?}", module.key))?;
            let retained =
                CheckedNamespaceInterface::from_checked(module.key, &segments(module.path), &parsed, &checker)?
                    .ok_or_else(|| format!("{} published no namespace", module.key))?;
            published.push(CheckedNamespaceInterface::from_wire_value(retained.wire_value()?)?.installable()?);
            sources.push((module.key, parsed));
        }
        let empty = program("")?;
        Ok(Self {
            sources,
            published,
            empty,
        })
    }

    /// Check `source` as the module at `path`, from the providers' source or, warm, from their published namespaces.
    fn check(&self, path: &[&str], modules: &[Module], source: &Program, warm: bool) -> Result<(), Vec<CompileError>> {
        let mut consumer = configured(path, modules);
        if warm {
            consumer.select_dependency_interfaces(self.published.iter().map(Arc::clone).collect());
            let dependencies = self
                .sources
                .iter()
                .map(|(key, _)| (*key, &self.empty))
                .collect::<Vec<_>>();
            return consumer.check_with_imports(source, &dependencies);
        }
        let dependencies = self.sources.iter().map(|(key, program)| (*key, program)).collect::<Vec<_>>();
        consumer.check_with_imports(source, &dependencies)
    }
}

/// The one error a refused import reports, on the named route.
fn single_refusal(route: &str, result: Result<(), Vec<CompileError>>) -> Result<CompileError, String> {
    let errors = result
        .err()
        .ok_or_else(|| format!("{route}: the import of a non-pub import was accepted"))?;
    let [error] = errors.as_slice() else {
        let messages = errors.iter().map(|error| error.message.as_str()).collect::<Vec<_>>();
        return Err(format!("{route}: expected one refusal, got {messages:?}"));
    };
    assert_eq!(error.stable_code(), Some("INCAN-I0001"), "{route}");
    Ok(error.clone())
}

/// RFC 176: a name one module imports without `pub` is no export of that module, so importing it from there is
/// refused with `INCAN-I0001`, whether the importer is checked against the module's source or against its published
/// namespace, with one error whose hint names the module that exports the name (Oven's `toolchain` took
/// `PayloadAdmission` from `unit_publication`, which imports it from `payload_types`). The exporting module and a
/// `pub from` re-export of it stay importable on both routes.
#[test]
fn a_non_pub_import_is_refused_from_source_and_from_the_published_namespace() -> Result<(), String> {
    let modules = [
        Module {
            key: "payload_types",
            path: &["payload_types"],
            source: "pub model PayloadAdmission:\n    pub identity: str\n",
        },
        Module {
            key: "unit_publication",
            path: &["unit_publication"],
            source: "from payload_types import PayloadAdmission\n\npub def admitted(identity: str) -> PayloadAdmission:\n    return PayloadAdmission(identity=identity)\n",
        },
        Module {
            key: "facade",
            path: &["facade"],
            source: "pub from payload_types import PayloadAdmission\n",
        },
    ];
    let providers = Providers::new(&modules)?;
    let toolchain = |source_module: &str| {
        program(&format!(
            "from {source_module} import PayloadAdmission\nfrom unit_publication import admitted\n\npub model Toolchain:\n    pub payloads: list[PayloadAdmission]\n\npub def first() -> Toolchain:\n    return Toolchain(payloads=[admitted(\"unit\")])\n"
        ))
    };

    for warm in [false, true] {
        let route = if warm { "warm" } else { "cold" };
        let refused = providers.check(&["toolchain"], &modules, &toolchain("unit_publication")?, warm);
        let error = single_refusal(route, refused)?;
        assert_eq!(
            error.message, "`PayloadAdmission` is not exported by `unit_publication`",
            "{route}"
        );
        assert!(
            error
                .hints()
                .iter()
                .any(|hint| hint.contains("`payload_types`") && hint.contains("`facade`")),
            "{route}: the hint names the modules that export the name: {:?}",
            error.hints()
        );
        for exporter in ["payload_types", "facade"] {
            providers
                .check(&["toolchain"], &modules, &toolchain(exporter)?, warm)
                .map_err(|errors| format!("{route} from {exporter}: {errors:?}"))?;
        }
    }
    Ok(())
}

/// A namespace's own module (its prelude) that imports a submodule's name without `pub` does not export it (std's
/// `tempfile` took `File` from `std.fs`, whose prelude imported it from `std.fs.file`). The refusal names the module
/// in Incan spelling, and its hint asks for the `pub from` line in the namespace, not an import from the submodule,
/// because the namespace is the import path its package documents. With that line, the import is accepted.
#[test]
fn a_name_a_namespace_imports_without_pub_is_refused_with_a_hint_to_re_export_it() -> Result<(), String> {
    const FILE: &str = "pub model File:\n    pub name: str\n";
    let importer = program("from pkg.store import File\n\npub def file_name(file: File) -> str:\n    return file.name\n")?;
    let namespace = |prelude: &'static str| {
        [
            Module {
                key: "pkg_store_file",
                path: &["pkg", "store", "file"],
                source: FILE,
            },
            Module {
                key: "pkg_store",
                path: &["pkg", "store"],
                source: prelude,
            },
        ]
    };

    let modules = namespace("from pkg.store.file import File\n");
    let providers = Providers::new(&modules)?;
    for warm in [false, true] {
        let route = if warm { "warm" } else { "cold" };
        let error = single_refusal(route, providers.check(&["tempfile"], &modules, &importer, warm))?;
        assert_eq!(error.message, "`File` is not exported by `pkg.store`", "{route}");
        assert_eq!(
            error.hints(),
            ["Re-export it from `pkg.store` with `pub from pkg.store.file import File` in that namespace's prelude"],
            "{route}"
        );
    }

    let modules = namespace("pub from pkg.store.file import File\n");
    let providers = Providers::new(&modules)?;
    for warm in [false, true] {
        let route = if warm { "warm" } else { "cold" };
        providers
            .check(&["tempfile"], &modules, &importer, warm)
            .map_err(|errors| format!("{route}: the re-exported name was refused: {errors:?}"))?;
    }
    Ok(())
}
