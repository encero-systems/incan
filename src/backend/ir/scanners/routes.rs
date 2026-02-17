//! Route collection from web decorators.

use crate::frontend::ast::{self as ast, Declaration, DecoratorArg, Expr, Literal, Program};
use incan_core::lang::decorators::{self, DecoratorId};
use incan_core::lang::http;

use super::decorators::{collect_import_aliases, resolve_decorator_id};

/// Collected route metadata: (handler, path, methods, unknown_methods, is_async, module_path_segments).
pub type RouteScan = (
    String,
    String,
    Vec<http::HttpMethodId>,
    Vec<String>,
    bool,
    Option<Vec<String>>,
);

/// Collect routes from `@route` decorators.
///
/// The `module_path_segments` parameter should be `None` for the main module, or `Some(&["api", "routes"])`
/// for nested submodules.
pub fn collect_routes(program: &Program, module_path_segments: Option<&[String]>) -> Vec<RouteScan> {
    let aliases = collect_import_aliases(program);
    let mut routes = Vec::new();
    for decl in &program.declarations {
        if let Declaration::Function(func) = &decl.node {
            for dec in &func.decorators {
                if resolve_decorator_id(&dec.node, &aliases) == Some(DecoratorId::Route) {
                    let mut path = String::new();
                    let mut methods = vec![http::HttpMethodId::Get];
                    let mut unknown_methods: Vec<String> = Vec::new();
                    for arg in &dec.node.args {
                        match arg {
                            DecoratorArg::Positional(expr) => {
                                if !path.is_empty() {
                                    continue;
                                }
                                if let Expr::Literal(Literal::String(s)) = &expr.node {
                                    path = s.clone();
                                }
                            }
                            DecoratorArg::Named(name, value) => {
                                if name != decorators::ROUTE_METHODS_ARG {
                                    continue;
                                }
                                let ast::DecoratorArgValue::Expr(expr) = value else {
                                    continue;
                                };
                                let Expr::List(items) = &expr.node else { continue };

                                let mut method_strings = Vec::new();
                                for item in items {
                                    match &item.node {
                                        Expr::Literal(Literal::String(s)) => method_strings.push(s.clone()),
                                        Expr::Ident(name) => method_strings.push(name.clone()),
                                        _ => {}
                                    }
                                }

                                if method_strings.is_empty() {
                                    continue;
                                }

                                let mut selected: Vec<http::HttpMethodId> = Vec::new();
                                for method in method_strings {
                                    if let Some(id) = http::from_str(method.as_str()) {
                                        if !selected.contains(&id) {
                                            selected.push(id);
                                        }
                                    } else {
                                        unknown_methods.push(method);
                                    }
                                }

                                if !selected.is_empty() {
                                    methods = selected;
                                }
                            }
                        }
                    }
                    if !path.is_empty() {
                        routes.push((
                            func.name.clone(),
                            path,
                            methods,
                            unknown_methods,
                            func.is_async(),
                            module_path_segments.map(|segs| segs.to_vec()),
                        ));
                    }
                }
            }
        }
    }
    routes
}
