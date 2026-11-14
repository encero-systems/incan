//! Source module import resolution, source roots, and logical module paths.

use super::*;

/// Resolve every import declaration in `program` through the canonical source-module resolver.
///
/// `base_dir` is the directory of `program`'s source file. `source_root` is optional but should be supplied by CLI and
/// test-runner flows that already resolved the project source root; when it is absent, manifest/layout discovery is
/// used as a fallback for crate-root imports and source-root fallback behavior.
///
/// `from std.<namespace> import <item>, ...` depends on each item that is a stdlib submodule, and on the namespace's
/// own module only when some item is not a submodule. An import of submodules alone reads nothing from the namespace's
/// own module, and a namespace such as `std.derives` has none: `from std.derives import comparison` names
/// `std.derives.comparison` only.
pub fn resolve_program_source_imports(
    program: &Program,
    base_dir: &Path,
    source_root: Option<&Path>,
) -> Vec<ResolvedProgramSourceImport> {
    let current_source_file = program.source_path.as_deref().map(Path::new);
    program
        .declarations
        .iter()
        .flat_map(|decl| {
            let Declaration::Import(import) = &decl.node else {
                return Vec::new();
            };
            let StdlibNamespaceItems {
                submodules,
                only_submodules,
            } = stdlib_namespace_items(import);
            // The module the import names, unless it names only submodules of a stdlib namespace.
            let named_module = (!only_submodules).then(|| ResolvedProgramSourceImport {
                span: decl.span,
                resolution: resolve_source_module_import_from_source_file(
                    base_dir,
                    source_root,
                    current_source_file,
                    import,
                ),
            });
            named_module
                .into_iter()
                .chain(submodules.into_iter().map(|module_path| ResolvedProgramSourceImport {
                    span: decl.span,
                    resolution: SourceModuleImportResolution::Stdlib { module_path },
                }))
                .collect()
        })
        .collect()
}

/// The items of one import that are stdlib submodules of the namespace it imports from.
pub(super) struct StdlibNamespaceItems {
    /// The module path of each item of `from std.<namespace> import ...` that is a submodule of that namespace, in
    /// item order; empty for any other import.
    submodules: Vec<Vec<String>>,
    /// Whether the import names submodules and nothing else, so it does not depend on the namespace's own module.
    only_submodules: bool,
}

/// Classify the items of a `from std.<namespace> import ...` declaration as that namespace's submodules or members.
pub(super) fn stdlib_namespace_items(import: &ImportDecl) -> StdlibNamespaceItems {
    let ImportKind::From { module, items } = &import.kind else {
        return StdlibNamespaceItems {
            submodules: Vec::new(),
            only_submodules: false,
        };
    };
    if module.parent_levels != 0
        || module.is_absolute
        || module.segments.first().map(String::as_str) != Some(stdlib::STDLIB_ROOT)
    {
        return StdlibNamespaceItems {
            submodules: Vec::new(),
            only_submodules: false,
        };
    }
    let submodules = items
        .iter()
        .map(|item| {
            let mut item_module_path = module.segments.clone();
            item_module_path.push(item.name.clone());
            item_module_path
        })
        .filter(|item_module_path| stdlib::is_known_stdlib_module(item_module_path))
        .map(|item_module_path| stdlib_module_named_by(&item_module_path))
        .collect::<Vec<_>>();
    let only_submodules = !submodules.is_empty() && submodules.len() == items.len();
    StdlibNamespaceItems {
        submodules,
        only_submodules,
    }
}

/// Return the stdlib module a `std...` import path names: a namespace's `prelude` path (`std.async.prelude`) names the
/// namespace's own module (`std.async`), as the checker, the provider module-use set and lowering read it (#1561).
///
/// A source graph collects the module under that one identity, so a compiled SDK that claims `std.async` serves an
/// import of `std.async.prelude` instead of the graph mounting `async/prelude.incn` from source beside the SDK crate.
pub(super) fn stdlib_module_named_by(path: &[String]) -> Vec<String> {
    stdlib::stdlib_prelude_module_namespace(path).map_or_else(|| path.to_vec(), <[String]>::to_vec)
}

/// Resolve one import declaration into local source, stdlib source, or external/non-source classification.
pub fn resolve_source_module_import(
    base_dir: &Path,
    source_root: Option<&Path>,
    import: &ImportDecl,
) -> SourceModuleImportResolution {
    resolve_source_module_import_from_source_file(base_dir, source_root, None, import)
}

/// Resolve one import declaration while retaining the importing source-file identity when it is known.
///
/// A located local module that is the importing file is classified as [`SourceModuleImportResolution::SelfImport`].
/// This keeps accidental self imports from entering the source graph and becoming a recursive module.
pub fn resolve_source_module_import_from_source_file(
    base_dir: &Path,
    source_root: Option<&Path>,
    current_source_file: Option<&Path>,
    import: &ImportDecl,
) -> SourceModuleImportResolution {
    let Some((path, candidates)) = source_import_candidates(import) else {
        return SourceModuleImportResolution::External;
    };

    if path.parent_levels == 0
        && !path.is_absolute
        && path
            .segments
            .first()
            .is_some_and(|segment| segment == stdlib::STDLIB_ROOT)
    {
        return SourceModuleImportResolution::Stdlib {
            module_path: stdlib_module_named_by(&path.segments),
        };
    }

    let identity_root = source_root.map(Path::to_path_buf);
    let can_use_root_import = !path.is_absolute && path.parent_levels == 0;

    let primary_base = primary_import_base_dir(base_dir, source_root, path);
    if let Some(module_ref) = resolve_first_source_candidate(&primary_base, identity_root.as_deref(), &candidates) {
        return classify_local_source_import(module_ref, current_source_file, &path.segments, can_use_root_import);
    }

    if !path.is_absolute
        && path.parent_levels == 0
        && let Some(root) = identity_root
        && root != primary_base
        && let Some(module_ref) = resolve_first_source_candidate(&root, Some(&root), &candidates)
    {
        return classify_local_source_import(module_ref, current_source_file, &path.segments, can_use_root_import);
    }

    SourceModuleImportResolution::External
}

/// Classify a located source module while enforcing the no-self-import invariant.
pub(super) fn classify_local_source_import(
    module_ref: SourceModuleRef,
    current_source_file: Option<&Path>,
    import_path: &[String],
    can_use_root_import: bool,
) -> SourceModuleImportResolution {
    if current_source_file.is_some_and(|source_file| source_files_match(source_file, &module_ref.file_path)) {
        return SourceModuleImportResolution::SelfImport {
            module_ref,
            import_path: import_path.to_vec(),
            can_use_root_import,
        };
    }
    SourceModuleImportResolution::Local(module_ref)
}

/// Return whether two source paths name the same file, accounting for canonical and relative spellings.
pub(super) fn source_files_match(left: &Path, right: &Path) -> bool {
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
}

/// Render the common diagnostic for an accidental source-module self import.
pub fn self_import_diagnostic_message(
    module_ref: &SourceModuleRef,
    import_path: &[String],
    can_use_root_import: bool,
) -> String {
    let message = format!(
        "module `{}` imports itself; remove this import",
        module_ref.path_segments.join(".")
    );
    if can_use_root_import && module_ref.path_segments != import_path {
        format!(
            "{message} or use `crate::{}` to import a project-root module",
            import_path.join(".")
        )
    } else {
        message
    }
}

/// Resolve an `import` / `from ... import ...` into an on-disk Incan module file path.
///
/// This compatibility wrapper returns only local source files. New orchestration code should prefer
/// [`resolve_source_module_import`] or [`resolve_program_source_imports`] so stdlib and external imports are classified
/// explicitly.
pub fn resolve_import_path(base_dir: &Path, import: &ImportDecl) -> Option<PathBuf> {
    match resolve_source_module_import(base_dir, None, import) {
        SourceModuleImportResolution::Local(module_ref) => Some(module_ref.file_path),
        SourceModuleImportResolution::SelfImport { .. }
        | SourceModuleImportResolution::Stdlib { .. }
        | SourceModuleImportResolution::External => None,
    }
}

/// Return import path metadata plus candidate source-module paths for an import.
///
/// `from a.b import C` has one source module candidate, `a.b`. `import a.b.C` is ambiguous in existing syntax because
/// it can mean a module import or an item-style import; try the full path first, then the parent module. Keeping both
/// candidates here prevents CLI, LSP, and test-runner paths from drifting on that ambiguity.
pub(super) fn source_import_candidates(import: &ImportDecl) -> Option<(&ImportPath, Vec<Vec<String>>)> {
    match &import.kind {
        ImportKind::From { module, .. } if !module.segments.is_empty() => Some((module, vec![module.segments.clone()])),
        ImportKind::Module(path) if !path.segments.is_empty() => {
            let mut candidates = vec![path.segments.clone()];
            if path.segments.len() > 1 {
                let parent = path.segments[..path.segments.len() - 1].to_vec();
                if parent != path.segments {
                    candidates.push(parent);
                }
            }
            Some((path, candidates))
        }
        ImportKind::RustCrate { .. }
        | ImportKind::RustFrom { .. }
        | ImportKind::PubLibrary { .. }
        | ImportKind::PubFrom { .. }
        | ImportKind::Python(_)
        | ImportKind::Module(_)
        | ImportKind::From { .. } => None,
    }
}

/// Determine the primary base directory for resolving an import path.
pub(super) fn primary_import_base_dir(base_dir: &Path, source_root: Option<&Path>, path: &ImportPath) -> PathBuf {
    if path.is_absolute {
        return source_root
            .map(Path::to_path_buf)
            .unwrap_or_else(|| discover_source_root_from_layout(base_dir));
    }

    let mut base = base_dir.to_path_buf();
    for _ in 0..path.parent_levels {
        base = base.parent().map(Path::to_path_buf).unwrap_or(base);
    }
    base
}

/// Infer a source root from conventional project layout when no manifest-driven root is available.
pub(super) fn discover_source_root_from_layout(base_dir: &Path) -> PathBuf {
    let mut project_root = base_dir.to_path_buf();
    while !project_root.join("loaf.toml").exists() && !project_root.join("src").exists() {
        if !project_root.pop() {
            break;
        }
    }
    if project_root.join("src").exists() {
        project_root.join("src")
    } else {
        project_root
    }
}

/// Resolve a module path under `source_root` to its source file, the way a crate-root import of it resolves; a
/// standard package reaches its own `std.<root>` modules this way.
pub fn resolve_source_module_path(source_root: &Path, segments: &[String]) -> Option<SourceModuleRef> {
    resolve_first_source_candidate(source_root, Some(source_root), &[segments.to_vec()])
}

/// Resolve the first candidate path that exists under `base`.
pub(super) fn resolve_first_source_candidate(
    base: &Path,
    identity_root: Option<&Path>,
    candidates: &[Vec<String>],
) -> Option<SourceModuleRef> {
    for candidate in candidates {
        if candidate.is_empty() {
            continue;
        }
        if let Some(file_path) = resolve_module_path_from_base(base, candidate) {
            let logical_segments = identity_root
                .and_then(|root| logical_module_segments_from_file(root, &file_path))
                .or_else(|| logical_module_segments_from_file(base, &file_path))
                .unwrap_or_else(|| candidate.clone());
            return Some(SourceModuleRef::new(file_path, logical_segments));
        }
    }
    None
}

/// Canonicalize source-module path segments.
///
/// `mod`, `__init__`, and directory-local `prelude` files are file-layout entrypoints for directory-backed modules,
/// not semantic module names. Normalize them away so the logical module identity stays consistent anywhere the
/// compiler converts source-backed module paths into logical module IDs. A root-level `prelude.incn` remains a
/// distinct module.
pub fn canonicalize_source_module_segments(segments: &[String]) -> Vec<String> {
    match segments.last().map(String::as_str) {
        Some("mod" | "__init__") => segments[..segments.len().saturating_sub(1)].to_vec(),
        Some("prelude") if segments.len() > 1 => segments[..segments.len().saturating_sub(1)].to_vec(),
        _ => segments.to_vec(),
    }
}

/// Canonicalize a library declaration's source-root spelling for identity comparison and publication.
///
/// The checker collects a library entrypoint under `main`, while its published declaration metadata uses `lib`.
/// These roots name the same declaration domain; nested module paths already have one stable spelling.
pub fn canonicalize_library_declaration_module_path(module_path: &[String]) -> Vec<String> {
    match module_path {
        [root] if root == "main" => vec!["lib".to_string()],
        _ => module_path.to_vec(),
    }
}

/// Return logical module identities for one source import in source-resolution order.
///
/// Bare imports first resolve beside the importing module and then from the source root. Absolute imports resolve from
/// the root, while relative imports walk upward from the importing module's directory. Keeping this identity-only
/// projection beside the on-disk resolver lets typechecking and later compiler stages consume the same module graph
/// without reopening the filesystem.
pub fn logical_source_import_candidates(current_module_path: &[String], path: &ImportPath) -> Vec<Vec<String>> {
    logical_source_path_candidates(
        current_module_path,
        &path.segments,
        path.is_absolute,
        path.parent_levels,
    )
}

/// Return logical source identities from qualifier components shared by AST and lowered IR imports.
pub fn logical_source_path_candidates(
    current_module_path: &[String],
    path_segments: &[String],
    is_absolute: bool,
    parent_levels: usize,
) -> Vec<Vec<String>> {
    let segments = canonicalize_source_module_segments(path_segments);
    if segments.is_empty() {
        return Vec::new();
    }
    if is_absolute {
        return vec![segments];
    }
    if current_module_path.is_empty() {
        return if parent_levels == 0 { vec![segments] } else { Vec::new() };
    }

    let Some(levels_from_module) = parent_levels.checked_add(1) else {
        return Vec::new();
    };
    let Some(mut relative) = current_module_path
        .len()
        .checked_sub(levels_from_module)
        .map(|keep| current_module_path[..keep].to_vec())
    else {
        return Vec::new();
    };
    relative.extend(segments.iter().cloned());

    if parent_levels == 0 && relative != segments {
        vec![relative, segments]
    } else {
        vec![relative]
    }
}

/// Derive the logical module path segments for an on-disk source module relative to `base`.
///
/// Examples:
/// - `src/foo.incn` => `["foo"]`
/// - `src/foo/bar.incn` => `["foo", "bar"]`
/// - `src/foo/mod.incn` => `["foo"]`
/// - `src/foo/bar/mod.incn` => `["foo", "bar"]`
/// - `src/foo/prelude.incn` => `["foo"]`
pub fn logical_module_segments_from_file(base: &Path, module_file: &Path) -> Option<Vec<String>> {
    let relative = if let Ok(relative) = module_file.strip_prefix(base) {
        relative.to_path_buf()
    } else {
        let canonical_base = base.canonicalize().ok()?;
        let canonical_file = module_file.canonicalize().ok()?;
        canonical_file.strip_prefix(&canonical_base).ok()?.to_path_buf()
    };
    let mut segments = Vec::new();

    for component in relative.components() {
        let part = component.as_os_str().to_str()?;
        let path = Path::new(part);
        let stem = path.file_stem().and_then(|stem| stem.to_str()).unwrap_or(part);
        segments.push(stem.to_string());
    }

    Some(canonicalize_source_module_segments(&segments))
}

/// Resolves an Incan module file under `base` from import path segments (e.g. `foo.bar` → `foo/bar`).
///
/// Tries, in order: `segments.incn`, `segments.incan`, `segments/mod.incn`, `segments/mod.incan`,
/// `segments/__init__.incn`, `segments/__init__.incan`. Returns the first path that exists on disk, canonicalized when
/// possible. Returns `None` if none match.
pub(super) fn resolve_module_path_from_base(base: &Path, path: &[String]) -> Option<PathBuf> {
    // Build file path from segments
    let mut file_path = base.to_path_buf();
    for segment in path {
        file_path = file_path.join(segment);
    }

    // Try .incn extension first (preferred)
    let mut with_ext = file_path.clone();
    with_ext.set_extension("incn");
    if with_ext.exists() {
        return Some(with_ext.canonicalize().unwrap_or(with_ext));
    }

    // Try .incan extension (legacy/alternate)
    with_ext.set_extension("incan");
    if with_ext.exists() {
        return Some(with_ext.canonicalize().unwrap_or(with_ext));
    }

    // Try as directory with mod.incn
    let mod_file = file_path.join("mod.incn");
    if mod_file.exists() {
        return Some(mod_file.canonicalize().unwrap_or(mod_file));
    }

    // Try as directory with mod.incan (legacy/alternate)
    let mod_file_legacy = file_path.join("mod.incan");
    if mod_file_legacy.exists() {
        return Some(mod_file_legacy.canonicalize().unwrap_or(mod_file_legacy));
    }

    // Try as directory with __init__.incn (Python-style entrypoint)
    let init_file = file_path.join("__init__.incn");
    if init_file.exists() {
        return Some(init_file.canonicalize().unwrap_or(init_file));
    }

    // Try as directory with __init__.incan (legacy/alternate)
    let init_file_legacy = file_path.join("__init__.incan");
    if init_file_legacy.exists() {
        return Some(init_file_legacy.canonicalize().unwrap_or(init_file_legacy));
    }

    None
}
