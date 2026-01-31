//! IR-based code generation facade
//!
//! This module provides `IrCodegen`, a unified API for generating Rust code from Incan AST using the IR pipeline:
//!
//! ```text
//! AST → AstLowering → IR → IrEmitter (quote!) → prettyplease → RustSource
//! ```
//!
//! ## Usage
//!
//! ```rust,ignore
//! use incan::backend::IrCodegen;
//!
//! // Fallible API (recommended):
//! let codegen = IrCodegen::new();
//! let rust_code = codegen.try_generate(&ast)?;
//!
//! // Convenience API (returns error comments on failure):
//! let mut codegen = IrCodegen::new();
//! let rust_code = codegen.generate(&ast);
//! ```
//!
//! ## Error Handling
//!
//! The `try_generate*` family of methods return `Result<_, GenerationError>`,
//! allowing callers to handle lowering and emission errors explicitly.
//! The `generate*` methods are convenience wrappers that return error comments
//! on failure (useful for debugging but not recommended for production).

use std::collections::{HashMap, HashSet};
use std::env;

use crate::frontend::ast::Program;
use crate::frontend::diagnostics::CompileError;

use super::emit::RouteSpec;
use super::scanners::{
    check_for_this_import as scan_check_for_this_import, collect_routes as scan_collect_routes,
    collect_rust_crates as scan_collect_rust_crates, detect_async_usage, detect_list_helpers_usage, detect_serde_usage,
    detect_web_usage,
};
use super::{AstLowering, EmitError, EmitService, IrEmitter, LoweringErrors};

fn collect_model_field_aliases(main: &Program, deps: &[(&str, &Program)]) -> HashMap<String, HashMap<String, String>> {
    use crate::frontend::ast::Declaration;

    let mut out: HashMap<String, HashMap<String, String>> = HashMap::new();

    let mut visit = |p: &Program| {
        for decl in &p.declarations {
            let Declaration::Model(m) = &decl.node else {
                continue;
            };

            let mut map: HashMap<String, String> = HashMap::new();
            for f in &m.fields {
                if let Some(alias) = &f.node.metadata.alias {
                    map.insert(alias.clone(), f.node.name.clone());
                }
            }

            if !map.is_empty() {
                out.entry(m.name.clone()).or_default().extend(map);
            }
        }
    };

    visit(main);
    for (_, dep) in deps {
        visit(dep);
    }

    out
}

/// Error during Rust code generation.
///
/// This error type wraps all possible errors that can occur during code generation,
/// including AST lowering errors and IR emission errors.
///
/// ## Examples
///
/// ```rust,ignore
/// use incan::backend::{IrCodegen, GenerationError};
///
/// let codegen = IrCodegen::new();
/// match codegen.try_generate(&ast) {
///     Ok(code) => println!("{}", code),
///     Err(GenerationError::Lowering(errors)) => {
///         for err in errors.iter() {
///             eprintln!("Lowering error: {}", err);
///         }
///     }
///     Err(GenerationError::Emission(e)) => eprintln!("Emission failed: {}", e),
/// }
/// ```
#[derive(Debug)]
pub enum GenerationError {
    /// Errors during frontend typechecking.
    TypeCheck(Vec<CompileError>),
    /// Errors during AST to IR lowering (may contain multiple errors)
    Lowering(LoweringErrors),
    /// Error during IR to Rust emission
    Emission(EmitError),
}

impl std::fmt::Display for GenerationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GenerationError::TypeCheck(errs) => {
                if errs.is_empty() {
                    write!(f, "typecheck failed")
                } else {
                    // We intentionally avoid rich source formatting here (no file/source context at this layer).
                    write!(f, "typecheck failed ({} errors): {}", errs.len(), errs[0].message)
                }
            }
            GenerationError::Lowering(e) => write!(f, "{}", e),
            GenerationError::Emission(e) => write!(f, "emission error: {}", e),
        }
    }
}

impl std::error::Error for GenerationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            GenerationError::TypeCheck(_) => None,
            GenerationError::Lowering(e) => Some(e),
            GenerationError::Emission(e) => Some(e),
        }
    }
}

impl From<LoweringErrors> for GenerationError {
    fn from(e: LoweringErrors) -> Self {
        GenerationError::Lowering(e)
    }
}

impl From<EmitError> for GenerationError {
    fn from(e: EmitError) -> Self {
        GenerationError::Emission(e)
    }
}

/// IR-based Rust code generator
///
/// This is the unified entrypoint for code generation. It uses the typed IR and syn/quote for code emission.
pub struct IrCodegen<'a> {
    /// The current program being generated
    current_program: Option<&'a Program>,
    /// Dependency modules to include before main
    dependency_modules: Vec<(&'a str, &'a Program)>,
    /// Whether serde is needed (for Serialize/Deserialize derives)
    needs_serde: bool,
    /// Whether tokio is needed (for async runtime)
    needs_tokio: bool,
    /// Whether axum web framework is needed
    needs_axum: bool,
    /// Collected routes from @route decorators
    routes: Vec<RouteSpec>,
    /// Whether generating in test mode (emit #[test] attributes)
    test_mode: bool,
    /// Specific test function to mark with #[test] (if any)
    test_function: Option<String>,
    /// Fixtures available for test functions (name -> (has_teardown, dependencies))
    fixtures: HashMap<String, (bool, Vec<String>)>,
    /// Rust crates imported via `import rust::` or `from rust::`
    rust_crates: HashSet<String>,
    /// Whether to emit the Zen of Incan at the start of main (set by `import this`)
    emit_zen_in_main: bool,
    /// Whether list helper functions are needed (for remove, count, index)
    needs_list_helpers: bool,
    /// Functions imported from external Rust crates (name -> true for external)
    external_rust_functions: HashSet<String>,
}

impl<'a> IrCodegen<'a> {
    /// Create a new IR-based code generator
    pub fn new() -> Self {
        Self {
            current_program: None,
            dependency_modules: Vec::new(),
            needs_serde: false,
            external_rust_functions: HashSet::new(),
            needs_tokio: false,
            needs_axum: false,
            routes: Vec::new(),
            test_mode: false,
            test_function: None,
            fixtures: HashMap::new(),
            rust_crates: HashSet::new(),
            emit_zen_in_main: false,
            needs_list_helpers: false,
        }
    }

    /// Get the Rust crates imported via `import rust::` or `from rust::`
    pub fn rust_crates(&self) -> &HashSet<String> {
        &self.rust_crates
    }

    /// Register a fixture for test code generation
    pub fn add_fixture(&mut self, name: &str, has_teardown: bool, dependencies: Vec<String>) {
        self.fixtures.insert(name.to_string(), (has_teardown, dependencies));
    }

    /// Enable test mode (emit #[test] attributes)
    pub fn set_test_mode(&mut self, enabled: bool) {
        self.test_mode = enabled;
    }

    /// Set specific test function to mark with #[test]
    pub fn set_test_function(&mut self, name: &str) {
        self.test_function = Some(name.to_string());
    }

    /// Check if serde is needed
    pub fn needs_serde(&self) -> bool {
        self.needs_serde
    }

    /// Check if tokio is needed
    pub fn needs_tokio(&self) -> bool {
        self.needs_tokio
    }

    /// Check if axum is needed
    pub fn needs_axum(&self) -> bool {
        self.needs_axum
    }

    /// Add a dependency module (for multi-file compilation)
    pub fn add_module(&mut self, module_name: &'a str, module_ast: &'a Program) {
        self.dependency_modules.push((module_name, module_ast));
    }

    // =========================================================================
    // Feature Detection
    // =========================================================================

    /// Scan a program for external Rust function imports
    fn collect_external_rust_functions(&mut self, program: &Program) {
        use crate::frontend::ast::{Declaration, ImportKind};

        for decl in &program.declarations {
            if let Declaration::Import(import) = &decl.node {
                match &import.kind {
                    // from rust::crate import items
                    ImportKind::RustFrom { items, .. } => {
                        for item in items {
                            let func_name = item.alias.as_ref().unwrap_or(&item.name);
                            self.external_rust_functions.insert(func_name.clone());
                        }
                    }
                    // Legacy: from rust::crate import items (parsed as From with rust:: module)
                    ImportKind::From { module, items } => {
                        if !module.segments.is_empty() && module.segments.first() == Some(&"rust".to_string()) {
                            for item in items {
                                let func_name = item.alias.as_ref().unwrap_or(&item.name);
                                self.external_rust_functions.insert(func_name.clone());
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    /// Scan a program for Serialize/Deserialize derives
    pub fn scan_for_serde(&mut self, program: &Program) {
        if detect_serde_usage(program) {
            self.needs_serde = true;
        }
    }

    /// Scan a program for async usage
    pub fn scan_for_async(&mut self, program: &Program) {
        if detect_async_usage(program) {
            self.needs_tokio = true;
        }
    }

    /// Scan a program for web framework usage
    pub fn scan_for_web(&mut self, program: &Program) {
        if detect_web_usage(program) {
            self.needs_axum = true;
            self.needs_tokio = true;
            self.needs_serde = true;
        }
    }

    /// Scan a program for list helper usage (remove, count, index)
    pub fn scan_for_list_helpers(&mut self, program: &Program) {
        if detect_list_helpers_usage(program) {
            self.needs_list_helpers = true;
        }
    }

    // (helper methods removed in favor of centralized scanners)

    /// Collect routes from @route decorators
    fn collect_routes(&mut self, program: &Program) {
        let collected = scan_collect_routes(program);
        for (handler_name, path, methods, unknown_methods, is_async) in collected {
            self.routes.push(RouteSpec {
                handler_name,
                path,
                methods,
                unknown_methods,
                is_async,
            });
        }
    }

    /// Collect rust crates from imports
    fn collect_rust_crates(&mut self, program: &Program) {
        let crates = scan_collect_rust_crates(program);
        for c in crates {
            self.rust_crates.insert(c);
        }
    }

    /// Check for `import this`
    fn check_for_this_import(&mut self, program: &Program) {
        if scan_check_for_this_import(program) {
            self.emit_zen_in_main = true;
        }
    }

    // =========================================================================
    // Code Generation - Main Entry Points
    // =========================================================================

    /// Generate Rust code from an Incan program (single-file mode)
    ///
    /// This is the main entry point for code generation. It:
    /// 1. Scans for feature usage (serde, async, web, etc.)
    /// 2. Lowers the AST to IR
    /// 3. Emits Rust code using syn/quote
    /// 4. Formats with prettyplease
    ///
    /// **Note**: This is a convenience method that returns error comments on failure.
    /// For production use, prefer [`try_generate`](Self::try_generate) which returns
    /// a proper `Result`.
    #[tracing::instrument(skip_all)]
    pub fn generate(mut self, program: &'a Program) -> String {
        match self.try_generate_internal(program) {
            Ok(code) => code,
            Err(e) => format!("// Generation error: {}\n", e),
        }
    }

    /// Generate Rust code from an Incan program (single-file mode, fallible)
    ///
    /// This is the recommended entry point for code generation. It:
    /// 1. Scans for feature usage (serde, async, web, etc.)
    /// 2. Lowers the AST to IR
    /// 3. Emits Rust code using syn/quote
    /// 4. Formats with prettyplease
    ///
    /// ## Errors
    ///
    /// Returns `GenerationError::Lowering` if AST lowering fails, or
    /// `GenerationError::Emission` if IR emission fails.
    ///
    /// ## Examples
    ///
    /// ```rust,ignore
    /// use incan::backend::IrCodegen;
    ///
    /// let codegen = IrCodegen::new();
    /// let rust_code = codegen.try_generate(&ast)?;
    /// ```
    #[tracing::instrument(skip_all)]
    pub fn try_generate(mut self, program: &'a Program) -> Result<String, GenerationError> {
        self.try_generate_internal(program)
    }

    /// Internal implementation of try_generate (takes &mut self)
    fn try_generate_internal(&mut self, program: &'a Program) -> Result<String, GenerationError> {
        self.current_program = Some(program);

        // Scan for features
        self.scan_for_serde(program);
        self.scan_for_async(program);
        self.scan_for_web(program);
        self.scan_for_list_helpers(program);
        self.collect_routes(program);
        self.collect_rust_crates(program);
        self.check_for_this_import(program);
        self.collect_external_rust_functions(program);

        // Scan dependencies
        for (_, dep_ast) in &self.dependency_modules.clone() {
            self.scan_for_serde(dep_ast);
            self.scan_for_async(dep_ast);
            self.scan_for_web(dep_ast);
            self.scan_for_list_helpers(dep_ast);
            self.collect_routes(dep_ast);
            self.collect_rust_crates(dep_ast);
        }

        // Use the IR pipeline: AST → IR → Rust
        self.try_generate_via_ir(program, &HashSet::new())
    }

    /// Generate code via the IR pipeline (fallible version)
    fn try_generate_via_ir(
        &self,
        program: &Program,
        internal_module_roots: &HashSet<String>,
    ) -> Result<String, GenerationError> {
        let deps: Vec<(&str, &Program)> = self
            .dependency_modules
            .iter()
            .map(|(name, ast)| (*name, *ast))
            .collect();

        // RFC 021: Make alias-aware lowering work across module boundaries by seeding alias maps
        // for models declared in dependency modules as well.
        let global_aliases = collect_model_field_aliases(program, &deps);

        // Typecheck to obtain reusable type information for lowering.
        //
        // Strict policy: if typechecking fails, do NOT proceed to lowering/codegen.
        let type_info_opt = {
            use crate::frontend::typechecker::TypeChecker;
            let mut tc = TypeChecker::new();
            match tc.check_with_imports(program, &deps) {
                Ok(()) => tc.type_info().clone(),
                Err(errs) => return Err(GenerationError::TypeCheck(errs)),
            }
        };

        // Lower AST to IR using typechecker output when available
        let mut lowering = AstLowering::new_with_type_info(type_info_opt);
        lowering.seed_struct_field_aliases(global_aliases.clone());
        let ir_program = lowering.lower_program(program)?;

        // Build unified function registry including imported module functions
        let mut unified_registry = ir_program.function_registry.clone();
        for (_, dep_ast) in &self.dependency_modules {
            // For dependencies, use best-effort lowering without type info to
            // preserve prior behavior and avoid redundant typechecking.
            let mut dep_lowering = AstLowering::new();
            dep_lowering.seed_struct_field_aliases(global_aliases.clone());
            let dep_ir = dep_lowering.lower_program(dep_ast)?;
            unified_registry.merge(&dep_ir.function_registry);
        }

        // Emit IR to Rust code
        let use_emit_service = env::var("INCAN_EMIT_SERVICE").ok().as_deref() == Some("1");
        if use_emit_service {
            let mut svc = EmitService::new_from_program(&ir_program);
            // Configure inner emitter
            let inner = svc.inner_mut();
            inner.set_internal_module_roots(internal_module_roots.clone());
            if self.emit_zen_in_main {
                inner.set_emit_zen(true);
            }
            inner.set_routes(self.routes.clone());
            inner.set_needs_serde(self.needs_serde);
            inner.set_needs_tokio(self.needs_tokio);
            inner.set_needs_axum(self.needs_axum);
            inner.set_external_rust_functions(self.external_rust_functions.clone());
            Ok(svc.emit_program(&ir_program)?)
        } else {
            let mut emitter = IrEmitter::new(&unified_registry);
            emitter.set_internal_module_roots(internal_module_roots.clone());
            if self.emit_zen_in_main {
                emitter.set_emit_zen(true);
            }
            emitter.set_routes(self.routes.clone());
            emitter.set_needs_serde(self.needs_serde);
            emitter.set_needs_tokio(self.needs_tokio);
            emitter.set_needs_axum(self.needs_axum);
            emitter.set_external_rust_functions(self.external_rust_functions.clone());
            Ok(emitter.emit_program(&ir_program)?)
        }
    }

    /// Generate Rust code for a dependency module (not the main module)
    ///
    /// **Note**: This is a convenience method that returns error comments on failure.
    /// For production use, prefer [`try_generate_module`](Self::try_generate_module).
    pub fn generate_module(&mut self, module_name: &str, program: &Program) -> String {
        match self.try_generate_module(module_name, program) {
            Ok(code) => code,
            Err(e) => format!("// Generation error: {}\n", e),
        }
    }

    /// Generate Rust code for a dependency module (not the main module, fallible)
    ///
    /// ## Errors
    ///
    /// Returns `GenerationError::Lowering` if AST lowering fails, or
    /// `GenerationError::Emission` if IR emission fails.
    pub fn try_generate_module(&mut self, _module_name: &str, program: &Program) -> Result<String, GenerationError> {
        // Use the IR pipeline for module generation too
        let mut lowering = AstLowering::new();
        let ir_program = lowering.lower_program(program)?;

        // Best-effort: treat registered dependency module names as internal roots.
        // (This is most relevant for the non-nested multi-file API.)
        let internal_roots: HashSet<String> = self
            .dependency_modules
            .iter()
            .map(|(name, _)| (*name).to_string())
            .collect();

        let use_emit_service = env::var("INCAN_EMIT_SERVICE").ok().as_deref() == Some("1");
        if use_emit_service {
            let mut svc = EmitService::new_from_program(&ir_program);
            svc.inner_mut().set_internal_module_roots(internal_roots);
            Ok(svc.emit_program(&ir_program)?)
        } else {
            let mut emitter = IrEmitter::new(&ir_program.function_registry);
            emitter.set_internal_module_roots(internal_roots);
            if self.emit_zen_in_main {
                emitter.set_emit_zen(true);
            }
            emitter.set_needs_serde(self.needs_serde);
            emitter.set_needs_tokio(self.needs_tokio);
            emitter.set_needs_axum(self.needs_axum);
            Ok(emitter.emit_program(&ir_program)?)
        }
    }

    /// Generate Rust code for a multi-file project
    ///
    /// **Note**: This is a convenience method that returns error comments on failure.
    /// For production use, prefer [`try_generate_multi_file`](Self::try_generate_multi_file).
    pub fn generate_multi_file(
        mut self,
        program: &'a Program,
        module_names: &[&str],
    ) -> (String, HashMap<String, String>) {
        match self.try_generate_multi_file_internal(program, module_names) {
            Ok(result) => result,
            Err(e) => (format!("// Generation error: {}\n", e), HashMap::new()),
        }
    }

    /// Generate Rust code for a multi-file project (fallible)
    ///
    /// ## Errors
    ///
    /// Returns `GenerationError::Lowering` if AST lowering fails for any module, or
    /// `GenerationError::Emission` if IR emission fails for any module.
    pub fn try_generate_multi_file(
        mut self,
        program: &'a Program,
        module_names: &[&str],
    ) -> Result<(String, HashMap<String, String>), GenerationError> {
        self.try_generate_multi_file_internal(program, module_names)
    }

    fn try_generate_multi_file_internal(
        &mut self,
        program: &'a Program,
        module_names: &[&str],
    ) -> Result<(String, HashMap<String, String>), GenerationError> {
        self.current_program = Some(program);

        // Scan all modules for features
        self.scan_for_serde(program);
        self.scan_for_async(program);
        self.scan_for_web(program);
        self.scan_for_list_helpers(program);
        self.collect_routes(program);
        self.collect_rust_crates(program);

        for (_, dep_ast) in &self.dependency_modules.clone() {
            self.scan_for_serde(dep_ast);
            self.scan_for_async(dep_ast);
            self.scan_for_web(dep_ast);
            self.scan_for_list_helpers(dep_ast);
            self.collect_routes(dep_ast);
            self.collect_rust_crates(dep_ast);
        }

        let internal_roots: HashSet<String> = module_names.iter().map(|s| (*s).to_string()).collect();

        // Generate main file
        let main_code = self.try_generate_via_ir(program, &internal_roots)?;

        let deps: Vec<(&str, &Program)> = self
            .dependency_modules
            .iter()
            .map(|(name, ast)| (*name, *ast))
            .collect();
        let global_aliases = collect_model_field_aliases(program, &deps);

        // Generate module files
        let mut modules = HashMap::new();
        for (name, ast) in &self.dependency_modules {
            if module_names.contains(name) {
                let mut lowering = AstLowering::new();
                lowering.seed_struct_field_aliases(global_aliases.clone());
                let ir = lowering.lower_program(ast)?;
                let use_emit_service = env::var("INCAN_EMIT_SERVICE").ok().as_deref() == Some("1");
                let module_code = if use_emit_service {
                    let mut svc = EmitService::new_from_program(&ir);
                    svc.inner_mut().set_internal_module_roots(internal_roots.clone());
                    svc.emit_program(&ir)?
                } else {
                    let mut emitter = IrEmitter::new(&ir.function_registry);
                    emitter.set_internal_module_roots(internal_roots.clone());
                    emitter.emit_program(&ir)?
                };
                modules.insert(name.to_string(), module_code);
            }
        }

        Ok((main_code, modules))
    }

    /// Generate Rust code for a multi-file project with nested module paths
    ///
    /// **Note**: This is a convenience method that returns error comments on failure.
    /// For production use, prefer [`try_generate_multi_file_nested`](Self::try_generate_multi_file_nested).
    pub fn generate_multi_file_nested(
        mut self,
        program: &'a Program,
        module_paths: &[Vec<String>],
    ) -> (String, HashMap<Vec<String>, String>) {
        match self.try_generate_multi_file_nested_internal(program, module_paths) {
            Ok(result) => result,
            Err(e) => (format!("// Generation error: {}\n", e), HashMap::new()),
        }
    }

    /// Generate Rust code for a multi-file project with nested module paths (fallible)
    ///
    /// ## Errors
    ///
    /// Returns `GenerationError::Lowering` if AST lowering fails for any module, or
    /// `GenerationError::Emission` if IR emission fails for any module.
    pub fn try_generate_multi_file_nested(
        mut self,
        program: &'a Program,
        module_paths: &[Vec<String>],
    ) -> Result<(String, HashMap<Vec<String>, String>), GenerationError> {
        self.try_generate_multi_file_nested_internal(program, module_paths)
    }

    fn try_generate_multi_file_nested_internal(
        &mut self,
        program: &'a Program,
        module_paths: &[Vec<String>],
    ) -> Result<(String, HashMap<Vec<String>, String>), GenerationError> {
        self.current_program = Some(program);

        // Scan all modules for features
        self.scan_for_serde(program);
        self.scan_for_async(program);
        self.scan_for_web(program);
        self.scan_for_list_helpers(program);
        self.collect_routes(program);
        self.collect_rust_crates(program);

        for (_, dep_ast) in &self.dependency_modules.clone() {
            self.scan_for_serde(dep_ast);
            self.scan_for_async(dep_ast);
            self.scan_for_web(dep_ast);
            self.scan_for_list_helpers(dep_ast);
            self.collect_routes(dep_ast);
            self.collect_rust_crates(dep_ast);
        }

        let internal_roots: HashSet<String> = module_paths.iter().filter_map(|p| p.first().cloned()).collect();

        // Generate main file
        let main_code = self.try_generate_via_ir(program, &internal_roots)?;

        let deps: Vec<(&str, &Program)> = self
            .dependency_modules
            .iter()
            .map(|(name, ast)| (*name, *ast))
            .collect();
        let global_aliases = collect_model_field_aliases(program, &deps);

        // Generate module files by path
        let mut modules = HashMap::new();
        for (name, ast) in &self.dependency_modules {
            // Find matching path by comparing joined segments with module name
            // Module name is path segments joined with "_" (e.g., "db_models")
            for path in module_paths {
                let path_name = path.join("_");
                if path_name == *name {
                    let mut lowering = AstLowering::new();
                    lowering.seed_struct_field_aliases(global_aliases.clone());
                    let ir = lowering.lower_program(ast)?;
                    let use_emit_service = env::var("INCAN_EMIT_SERVICE").ok().as_deref() == Some("1");
                    let module_code = if use_emit_service {
                        let mut svc = EmitService::new_from_program(&ir);
                        svc.inner_mut().set_internal_module_roots(internal_roots.clone());
                        svc.emit_program(&ir)?
                    } else {
                        let mut emitter = IrEmitter::new(&ir.function_registry);
                        emitter.set_internal_module_roots(internal_roots.clone());
                        emitter.emit_program(&ir)?
                    };
                    modules.insert(path.clone(), module_code);
                    break;
                }
            }
        }

        Ok((main_code, modules))
    }
}

impl Default for IrCodegen<'_> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::frontend::{lexer, parser};

    fn generate(source: &str) -> String {
        let tokens = lexer::lex(source).unwrap();
        let ast = parser::parse(&tokens).unwrap();
        IrCodegen::new().try_generate(&ast).unwrap()
    }

    /// Parse an Incan program into an AST
    fn parse_program(source: &str) -> Program {
        let tokens = lexer::lex(source).unwrap();
        parser::parse(&tokens).unwrap()
    }

    fn db_module_program() -> Program {
        parse_program(
            r#"
model Database:
  id: int
"#,
        )
    }

    fn main_module_program() -> Program {
        parse_program(
            r#"
def main() -> None:
  return
"#,
        )
    }

    fn generate_nested_store_code(store_source: &str) -> String {
        let db_module = db_module_program();
        let store_module = parse_program(store_source);
        let main_module = main_module_program();

        let mut codegen = IrCodegen::new();
        codegen.add_module("db_schema", &db_module);
        codegen.add_module("store_json_store", &store_module);

        let db_path = vec!["db".to_string(), "schema".to_string()];
        let store_path = vec!["store".to_string(), "json_store".to_string()];
        let (_main_code, rust_modules) = codegen
            .try_generate_multi_file_nested(&main_module, &[db_path.clone(), store_path.clone()])
            .unwrap();

        rust_modules.get(&store_path).unwrap().to_string()
    }

    fn generate_non_nested_store_code(store_source: &str, db_module_name: &str) -> String {
        let db_module = db_module_program();
        let store_module = parse_program(store_source);
        let main_module = main_module_program();

        let mut codegen = IrCodegen::new();
        codegen.add_module(db_module_name, &db_module);
        codegen.add_module("store", &store_module);

        let (_main_code, modules) = codegen
            .try_generate_multi_file(&main_module, &[db_module_name, "store"])
            .unwrap();

        modules.get("store").unwrap().to_string()
    }

    #[test]
    fn test_simple_function() {
        let code = generate(
            r#"
def add(a: int, b: int) -> int:
  return a + b
"#,
        );
        assert!(code.contains("fn add(a: i64, b: i64) -> i64"));
        assert!(code.contains("a + b"));
    }

    #[test]
    fn test_model_generation() {
        let code = generate(
            r#"
model User:
  name: str
  age: int
"#,
        );
        assert!(code.contains("struct User"));
        assert!(code.contains("name: String"));
        assert!(code.contains("age: i64"));
    }

    #[test]
    fn test_async_detection() {
        let source = r#"
async def fetch() -> str:
  return "hello"
"#;
        let tokens = lexer::lex(source).unwrap();
        let ast = parser::parse(&tokens).unwrap();
        let mut codegen = IrCodegen::new();
        codegen.scan_for_async(&ast);
        assert!(codegen.needs_tokio());
    }

    #[test]
    fn test_serde_detection() {
        let source = r#"
@derive(Serialize, Deserialize)
model Config:
  name: str
"#;
        let tokens = lexer::lex(source).unwrap();
        let ast = parser::parse(&tokens).unwrap();
        let mut codegen = IrCodegen::new();
        codegen.scan_for_serde(&ast);
        assert!(codegen.needs_serde());
    }

    #[test]
    fn test_serde_detection_single_derive() {
        let source = r#"
@derive(Serialize)
model User:
  id: int
"#;
        let tokens = lexer::lex(source).unwrap();
        let ast = parser::parse(&tokens).unwrap();
        let mut codegen = IrCodegen::new();
        codegen.scan_for_serde(&ast);
        assert!(codegen.needs_serde());
    }

    #[test]
    fn test_no_serde_when_not_used() {
        let source = r#"
@derive(Clone, Debug)
model User:
  id: int
"#;
        let tokens = lexer::lex(source).unwrap();
        let ast = parser::parse(&tokens).unwrap();
        let mut codegen = IrCodegen::new();
        codegen.scan_for_serde(&ast);
        assert!(!codegen.needs_serde());
    }

    #[test]
    fn test_serde_detection_json_stringify_builtin() {
        let source = r#"
def main() -> None:
  _ = json_stringify(123)
"#;
        let tokens = lexer::lex(source).unwrap();
        let ast = parser::parse(&tokens).unwrap();
        let mut codegen = IrCodegen::new();
        codegen.scan_for_serde(&ast);
        assert!(codegen.needs_serde());
    }

    #[test]
    fn test_no_async_when_not_used() {
        let source = r#"
def fetch() -> str:
  return "hello"
"#;
        let tokens = lexer::lex(source).unwrap();
        let ast = parser::parse(&tokens).unwrap();
        let mut codegen = IrCodegen::new();
        codegen.scan_for_async(&ast);
        assert!(!codegen.needs_tokio());
    }

    #[test]
    fn test_fstring_generation() {
        let code = generate(
            r#"
def greet(name: str) -> str:
  return f"Hello, {name}!"
"#,
        );
        assert!(code.contains(r#"incan_stdlib::strings::fstring"#));
        assert!(code.contains(r#"["Hello, ", "!"]"#));
    }

    #[test]
    fn test_struct_instantiation() {
        let code = generate(
            r#"
model Point:
  x: int
  y: int

def main() -> None:
  p = Point(x=10, y=20)
"#,
        );
        assert!(code.contains("Point {"));
        assert!(code.contains("x: 10"));
        assert!(code.contains("y: 20"));
    }

    #[test]
    fn test_enum_generation() {
        let code = generate(
            r#"
enum Status:
  Active
  Inactive
"#,
        );
        assert!(code.contains("enum Status"));
        assert!(code.contains("Active"));
        assert!(code.contains("Inactive"));
    }

    #[test]
    fn test_multi_file_imports_use_crate_prefix() {
        let store_code = generate_nested_store_code(
            r#"
from db.schema import Database
"#,
        );
        assert!(store_code.contains("use crate::db::schema::Database;"));
        assert!(!store_code.contains("use db::schema::Database;"));
    }

    #[test]
    fn test_multi_file_model_aliases_work_across_modules() {
        // DB module defines a model with an alias. Store module should be able to use the alias
        // in member access and constructor calls and still emit canonical Rust field names.
        let db_module = parse_program(
            r#"
model Account:
  type_ [alias="type"]: str
"#,
        );
        let store_module = parse_program(
            r#"
from db.schema import Account

def get_type(a: Account) -> str:
  return a.type

def make() -> Account:
  return Account(type="x")
"#,
        );
        let main_module = main_module_program();

        let mut codegen = IrCodegen::new();
        codegen.add_module("db_schema", &db_module);
        codegen.add_module("store_json_store", &store_module);

        let db_path = vec!["db".to_string(), "schema".to_string()];
        let store_path = vec!["store".to_string(), "json_store".to_string()];
        let (_main_code, rust_modules) = codegen
            .try_generate_multi_file_nested(&main_module, &[db_path.clone(), store_path.clone()])
            .unwrap();
        let store_code = rust_modules.get(&store_path).unwrap().to_string();

        assert!(
            store_code.contains(".type_"),
            "expected canonical field access; got:\n{store_code}"
        );
        assert!(
            store_code.contains("Account { type_:"),
            "expected canonical struct field init; got:\n{store_code}"
        );
        assert!(
            !store_code.contains(".type;"),
            "should not emit Rust keyword field access"
        );
        assert!(
            !store_code.contains("Account { type:"),
            "should not emit Rust keyword field init"
        );
    }

    #[test]
    fn test_multi_file_model_aliases_work_with_import_alias() {
        let db_module = parse_program(
            r#"
model Account:
  type_ [alias="type"]: str
"#,
        );
        let store_module = parse_program(
            r#"
from db.schema import Account as A

def get_type(a: A) -> str:
  return a.type

def make() -> A:
  return A(type="x")
"#,
        );
        let main_module = main_module_program();

        let mut codegen = IrCodegen::new();
        codegen.add_module("db_schema", &db_module);
        codegen.add_module("store_json_store", &store_module);

        let db_path = vec!["db".to_string(), "schema".to_string()];
        let store_path = vec!["store".to_string(), "json_store".to_string()];
        let (_main_code, rust_modules) = codegen
            .try_generate_multi_file_nested(&main_module, &[db_path.clone(), store_path.clone()])
            .unwrap();
        let store_code = rust_modules.get(&store_path).unwrap().to_string();

        assert!(
            store_code.contains(".type_"),
            "expected canonical field access; got:\n{store_code}"
        );
        assert!(
            store_code.contains("A { type_:"),
            "expected canonical struct field init; got:\n{store_code}"
        );
    }

    #[test]
    fn test_multi_file_self_alias_resolution_in_dependency_module() {
        let db_module = parse_program(
            r#"
model Account:
  type_ [alias="type"]: str

  def get_type(self) -> str:
    return self.type
"#,
        );
        let main_module = main_module_program();

        let mut codegen = IrCodegen::new();
        codegen.add_module("db_schema", &db_module);

        let db_path = vec!["db".to_string(), "schema".to_string()];
        let (_main_code, rust_modules) = codegen
            .try_generate_multi_file_nested(&main_module, std::slice::from_ref(&db_path))
            .unwrap();
        let db_code = rust_modules.get(&db_path).unwrap().to_string();

        assert!(
            db_code.contains("self.type_"),
            "expected canonical field access in dependency module; got:\n{db_code}"
        );
        assert!(
            !db_code.contains("self.type;"),
            "should not emit Rust keyword field access"
        );
    }

    #[test]
    fn test_rust_imports_do_not_use_crate_prefix() {
        let code = generate(
            r#"
from rust::time import Duration
"#,
        );
        assert!(code.contains("use time::Duration;"));
        assert!(!code.contains("use crate::time::Duration;"));
    }

    #[test]
    fn test_rust_style_external_crate_import_is_not_forced_under_crate() {
        let code = generate(
            r#"
import serde::Serialize
"#,
        );
        assert!(code.contains("use serde::Serialize;"));
        assert!(!code.contains("use crate::serde::Serialize;"));
    }

    #[test]
    fn test_relative_from_import_uses_super_prefix() {
        let store_code = generate_nested_store_code(
            r#"
from ..db.schema import Database
"#,
        );
        assert!(store_code.contains("use super::db::schema::Database;"));
        assert!(!store_code.contains("use crate::db::schema::Database;"));
    }

    #[test]
    fn test_multi_file_imports_rust_style_module_import_uses_crate_prefix() {
        let store_code = generate_nested_store_code(
            r#"
import db::schema::Database
"#,
        );
        assert!(store_code.contains("use crate::db::schema::Database;"));
        assert!(!store_code.contains("use db::schema::Database;"));
    }

    #[test]
    fn test_non_nested_multi_file_api_sets_internal_module_roots() {
        let store_code = generate_non_nested_store_code(
            r#"
from db import Database
"#,
            "db",
        );
        assert!(store_code.contains("use crate::db::Database;"));
        assert!(!store_code.contains("use db::Database;"));
    }

    #[test]
    fn test_non_nested_multi_file_nested_modules_use_crate_prefix() {
        let store_code = generate_non_nested_store_code(
            r#"
from db.schema import Database
"#,
            "db_schema",
        );
        assert!(store_code.contains("use crate::db::schema::Database;"));
        assert!(!store_code.contains("use db::schema::Database;"));
    }
}
