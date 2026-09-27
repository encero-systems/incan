//! Signature checks for `@route` handlers (`std.web.routing.route`).
//!
//! The route decorator lowers to a passthrough Rust attribute whose expansion registers the decorated function as an
//! HTTP handler. That registration has two demands the language never spelled out before #1721 and #1722: the
//! handler's return value must have a response form, and every handler parameter must be supplied by the request,
//! either by a `{name}` segment of the route path or by a typed extractor. A program that broke either rule checked
//! and then failed in the build, at the registration. This module makes both rules checker diagnostics.
//!
//! What crosses this boundary: the checker recognizes the route decorator the same way lowering does (a stdlib
//! `@rust.extern` function bound to the web macros crate, [`stdlib::STDLIB_WEB_MACROS_CRATE`]), reads the path
//! literal the same way the macro does ([`route_capture_names`]), and classifies the declared types by what it can
//! prove. A type it cannot classify, such as a Rust-origin type, is left to the build: the diagnostics here refuse only
//! shapes that are certainly wrong, never shapes that are merely unknown.
//!
//! The same rule covers the payload of the JSON-carrying wrappers (#1768): a `Json[T]`, `Query[T]` or `Path[T]`
//! parameter decodes the request into `T`, and a `Json[T]` return encodes `T`, so a declared `T` with no JSON form of
//! its own is refused rather than left to fail the registration.

use crate::ast::{Decorator, DecoratorArg, Expr, FunctionDecl, Literal, ParamKind, Spanned};
use crate::diagnostics::errors;
use crate::symbols::{ResolvedType, TypeInfo};
use incan_lang::lang::derives;
use incan_lang::lang::stdlib::{self, StdlibJsonTraitId};
use incan_lang::lang::surface::types::{self as surface_types, SurfaceTypeId};
use incan_lang::lang::types::collections::CollectionTypeId;

use super::TypeChecker;
use super::collect::decorators::resolve_decorator_path;
use super::helpers::collection_type_id;

/// What the checker can say about a declared type's role at a route boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RouteTypeShape {
    /// The type certainly has the role (a response form, or an extractor).
    Fits,
    /// The type certainly lacks the role: a scalar, a collection, or a plain nominal type.
    DoesNotFit,
    /// The checker cannot classify the type; the build decides.
    Unknown,
}

/// The two roles a declared type can play in a handler signature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RouteRole {
    /// The handler's return type, which becomes the HTTP response.
    Response,
    /// A handler parameter the request must supply through a typed extractor.
    Extractor,
}

/// Which way a route's JSON payload crosses the handler boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JsonPayloadDirection {
    /// The request is decoded into the payload: a `Json[T]`, `Query[T]` or `Path[T]` parameter.
    Decode,
    /// The payload is encoded as the response body: a `Json[T]` return.
    Encode,
}

impl JsonPayloadDirection {
    /// The `std.serde.json` protocol a payload needs to cross in this direction.
    fn protocol(self) -> StdlibJsonTraitId {
        match self {
            Self::Decode => StdlibJsonTraitId::Deserialize,
            Self::Encode => StdlibJsonTraitId::Serialize,
        }
    }
}

/// Extract the ordered capture names from a route path, spelled as the route macro reads them.
///
/// A capture is a whole path segment written `{name}`; a leading `*` marks a wildcard capture and is not part of
/// the name. Segments that are not captures, and malformed braces, contribute nothing: the macro binds nothing for
/// them either, so the checker must not either.
pub(super) fn route_capture_names(path: &str) -> Vec<String> {
    path.split('/')
        .filter_map(|segment| segment.strip_prefix('{')?.strip_suffix('}'))
        .map(|capture| capture.strip_prefix('*').unwrap_or(capture).to_string())
        .collect()
}

/// Return the route path when the decorator's first positional argument is a string literal.
///
/// Only a literal path tells the checker which segments the route captures. Any other spelling leaves the parameter
/// check to the build, which is where the macro reads the path too.
fn route_path_literal(decorator: &Decorator) -> Option<&str> {
    let DecoratorArg::Positional(path) = decorator.args.first()? else {
        return None;
    };
    match &path.node {
        Expr::Literal(Literal::String(path)) => Some(path.as_str()),
        _ => None,
    }
}

/// Return whether a surface web type plays `role` in a handler signature.
///
/// `Json` is both: a `Json[T]` parameter reads the body and a `Json[T]` return serializes the response. The other
/// wrappers are one or the other, and the remaining surface types (`App`, the async primitives) are neither.
fn surface_web_type_shape(id: SurfaceTypeId, role: RouteRole) -> RouteTypeShape {
    match (role, id) {
        (RouteRole::Response, SurfaceTypeId::Json | SurfaceTypeId::Html | SurfaceTypeId::Response)
        | (
            RouteRole::Extractor,
            SurfaceTypeId::Json
            | SurfaceTypeId::Query
            | SurfaceTypeId::Path
            | SurfaceTypeId::Body
            | SurfaceTypeId::Request,
        ) => RouteTypeShape::Fits,
        (
            RouteRole::Response,
            SurfaceTypeId::Query | SurfaceTypeId::Path | SurfaceTypeId::Body | SurfaceTypeId::Request,
        )
        | (RouteRole::Extractor, SurfaceTypeId::Response | SurfaceTypeId::Html) => RouteTypeShape::DoesNotFit,
        _ => RouteTypeShape::Unknown,
    }
}

/// The `std.web.macros` derive that grants `role` to a wrapper type.
fn role_derive(role: RouteRole) -> &'static str {
    match role {
        RouteRole::Response => stdlib::STDLIB_WEB_INTO_RESPONSE_TRAIT,
        RouteRole::Extractor => stdlib::STDLIB_WEB_FROM_REQUEST_PARTS_TRAIT,
    }
}

/// Combine the shapes of a compound type's parts: one certain misfit decides, otherwise the parts must all fit.
///
/// `all_fit_means_fits` says whether a compound whose every part fits is itself a fit (a `Result` of responses is a
/// response) or merely not refused (an `Option` of an extractor is an extractor only for some inner types, which the
/// checker does not model, so it stays unknown).
fn combine_part_shapes(parts: impl Iterator<Item = RouteTypeShape>, all_fit_means_fits: bool) -> RouteTypeShape {
    let mut all_fit = true;
    for part in parts {
        match part {
            RouteTypeShape::DoesNotFit => return RouteTypeShape::DoesNotFit,
            RouteTypeShape::Unknown => all_fit = false,
            RouteTypeShape::Fits => {}
        }
    }
    if all_fit && all_fit_means_fits {
        RouteTypeShape::Fits
    } else {
        RouteTypeShape::Unknown
    }
}

impl TypeChecker {
    /// Return the function's `@route` decorator when it carries one.
    ///
    /// The decorator is recognized by what it resolves to, not by how it was imported: a stdlib function named
    /// [`stdlib::STDLIB_WEB_ROUTE_DECORATOR`] that is `@rust.extern` and bound to
    /// [`stdlib::STDLIB_WEB_MACROS_CRATE`]. That is the same test lowering applies before emitting the passthrough
    /// attribute, so the checker and the emitter agree on which functions are route handlers.
    fn route_decorator<'a>(&mut self, decorators: &'a [Spanned<Decorator>]) -> Option<&'a Spanned<Decorator>> {
        decorators.iter().find(|decorator| {
            let resolved = resolve_decorator_path(&decorator.node, &self.symbols);
            let Some((name, module)) = resolved.split_last() else {
                return false;
            };
            if name != stdlib::STDLIB_WEB_ROUTE_DECORATOR || module.is_empty() {
                return false;
            }
            self.stdlib_cache
                .lookup_function_meta(module, name)
                .is_some_and(|meta| {
                    meta.is_rust_extern && meta.rust_module_path.as_deref() == Some(stdlib::STDLIB_WEB_MACROS_CRATE)
                })
        })
    }

    /// Check a function's signature against the route registration it declares (#1721, #1722).
    ///
    /// Nothing happens for a function without the route decorator. For a handler, the declared return type must have
    /// a response form, and every ordinary parameter must be bound by a `{name}` capture of the path or supplied by
    /// an extractor type. `return_type` and `param_types` are the resolved annotations `check_function` already
    /// computed, so the classification here never resolves a type a second time.
    pub(super) fn check_route_handler_signature(
        &mut self,
        func: &FunctionDecl,
        return_type: &ResolvedType,
        param_types: &[ResolvedType],
    ) {
        let Some(decorator) = self.route_decorator(&func.decorators) else {
            return;
        };

        // ---- Return type: the response the route sends ----
        if self.route_type_shape(return_type, RouteRole::Response) == RouteTypeShape::DoesNotFit {
            self.errors.push(errors::route_handler_return_not_response(
                &func.name,
                &return_type.to_string(),
                func.return_type.span,
            ));
        }

        // ---- JSON payloads: what the wrappers decode from the request and encode as the response (#1768) ----
        if let Some((wrapper, payload)) =
            self.route_payload_without_json_form(return_type, JsonPayloadDirection::Encode)
        {
            self.errors.push(errors::route_payload_without_json_form(
                &func.name,
                &wrapper,
                &payload,
                func.return_type.span,
            ));
        }
        for (param, param_ty) in func.params.iter().zip(param_types) {
            if param.node.kind != ParamKind::Normal {
                continue;
            }
            if let Some((wrapper, payload)) =
                self.route_payload_without_json_form(param_ty, JsonPayloadDirection::Decode)
            {
                self.errors.push(errors::route_payload_without_json_form(
                    &func.name, &wrapper, &payload, param.span,
                ));
            }
        }

        // ---- Parameters: what the request supplies ----
        let Some(path) = route_path_literal(&decorator.node) else {
            return;
        };
        let captures = route_capture_names(path);
        for (param, param_ty) in func.params.iter().zip(param_types) {
            if param.node.kind != ParamKind::Normal || captures.contains(&param.node.name) {
                continue;
            }
            if self.route_type_shape(param_ty, RouteRole::Extractor) == RouteTypeShape::DoesNotFit {
                self.errors.push(errors::route_handler_parameter_unbound(
                    &func.name,
                    &param.node.name,
                    path,
                    &captures,
                    param.span,
                ));
            }
        }
    }

    /// Classify a declared type's fitness for `role`, refusing only what is certainly wrong.
    ///
    /// Numbers, booleans, functions, type tokens and the frozen collections certainly lack both roles. Strings, bytes
    /// and the unit type are responses (the runtime sends them as bodies); as parameters they are left unknown, since
    /// a text or bytes body is something a request can supply. Tuples, `Result` and `Option` are judged by their
    /// parts, collections by their kind, nominal types by their derives and the web surface types by identity. Rust-
    /// origin types, type parameters and anything else unresolved are unknown and left to the build.
    fn route_type_shape(&self, ty: &ResolvedType, role: RouteRole) -> RouteTypeShape {
        match ty {
            ResolvedType::Str
            | ResolvedType::FrozenStr
            | ResolvedType::Bytes
            | ResolvedType::FrozenBytes
            | ResolvedType::Unit
            | ResolvedType::Never => match role {
                RouteRole::Response => RouteTypeShape::Fits,
                RouteRole::Extractor => RouteTypeShape::Unknown,
            },
            ResolvedType::Int
            | ResolvedType::Float
            | ResolvedType::Numeric(_)
            | ResolvedType::Bool
            | ResolvedType::FrozenList(_)
            | ResolvedType::FrozenDict(_, _)
            | ResolvedType::FrozenSet(_)
            | ResolvedType::Function(_, _)
            | ResolvedType::TypeToken(_) => RouteTypeShape::DoesNotFit,
            // A tuple of nothing but classifiable Incan types is never a response or an extractor; a tuple with a
            // Rust-origin part (a status code beside a body) may be one, so only a certain misfit refuses it.
            ResolvedType::Tuple(items) => {
                combine_part_shapes(items.iter().map(|item| self.route_type_shape(item, role)), false)
            }
            ResolvedType::Generic(name, args) => self.generic_route_type_shape(name, args, role),
            ResolvedType::Named(name) => self.named_route_type_shape(name, role),
            ResolvedType::TypeVar(_)
            | ResolvedType::SelfType
            | ResolvedType::Ref(_)
            | ResolvedType::RefMut(_)
            | ResolvedType::RustPath(_)
            | ResolvedType::CallSiteInfer
            | ResolvedType::Unknown => RouteTypeShape::Unknown,
        }
    }

    /// Classify a generic type: collections by their kind, then a generic nominal, then a web surface type.
    ///
    /// A `Result` of responses is a response; `Option` and `Result` parameters are extractors for some inner types
    /// the checker does not model, so they are refused only when a part certainly is not one. A list over an
    /// exact-width numeric is left unknown, since `list[u8]` is a byte body; every other list, dict, set and
    /// generator is refused.
    fn generic_route_type_shape(&self, name: &str, args: &[ResolvedType], role: RouteRole) -> RouteTypeShape {
        let parts = || args.iter().map(|arg| self.route_type_shape(arg, role));
        match collection_type_id(name) {
            Some(CollectionTypeId::Result) => combine_part_shapes(parts(), role == RouteRole::Response),
            Some(CollectionTypeId::Option) => combine_part_shapes(parts(), false),
            Some(CollectionTypeId::Tuple) => combine_part_shapes(parts(), false),
            Some(CollectionTypeId::List) if matches!(args, [ResolvedType::Numeric(_)]) => RouteTypeShape::Unknown,
            Some(
                CollectionTypeId::List
                | CollectionTypeId::Dict
                | CollectionTypeId::Set
                | CollectionTypeId::FrozenList
                | CollectionTypeId::FrozenDict
                | CollectionTypeId::FrozenSet
                | CollectionTypeId::Generator,
            ) => RouteTypeShape::DoesNotFit,
            None => self.named_route_type_shape(name, role),
        }
    }

    /// Classify a nominal type by its declaration, then by the web surface registry.
    ///
    /// A local declaration decides first, so a model that takes a surface spelling for itself (`model Response`) is
    /// judged by its own derives rather than by the name: it has the role when it derives the `std.web.macros` trait
    /// that grants it, under whatever name the program imported that trait. A `rusttype` is the Rust type itself, so
    /// whether it has the role is the Rust type's business and stays unknown. A name with no local declaration is a
    /// web surface type only when the registry knows it and the program brought it into scope; a type parameter,
    /// whose symbol is a builtin placeholder, reaches the same unknown outcome.
    fn named_route_type_shape(&self, name: &str, role: RouteRole) -> RouteTypeShape {
        let derives = match self.lookup_type_info(name) {
            Some(TypeInfo::Model(model)) => Some(&model.derives),
            Some(TypeInfo::Class(class)) => Some(&class.derives),
            Some(TypeInfo::Enum(enum_info)) => Some(&enum_info.derives),
            Some(TypeInfo::Newtype(newtype)) if newtype.is_rusttype => return RouteTypeShape::Unknown,
            Some(TypeInfo::Newtype(newtype)) => Some(&newtype.derives),
            Some(TypeInfo::Builtin | TypeInfo::TypeAlias) | None => None,
        };
        if let Some(derives) = derives {
            let granting = role_derive(role);
            let derives_role = derives.iter().any(|derive| {
                self.trait_bound_source_name(derive)
                    .as_deref()
                    .unwrap_or(derive.as_str())
                    == granting
            });
            return if derives_role {
                RouteTypeShape::Fits
            } else {
                RouteTypeShape::DoesNotFit
            };
        }
        match surface_types::from_str(name) {
            Some(id) if self.lookup_symbol(name).is_some() => surface_web_type_shape(id, role),
            _ => RouteTypeShape::Unknown,
        }
    }

    /// Return the wrapper and offending type spellings when a declared route type carries a JSON payload that
    /// certainly has no JSON form for `direction`, or `None` otherwise.
    ///
    /// The JSON-carrying wrappers are the web surface types the program brought into scope: `Json[T]` in both
    /// directions, and `Query[T]` and `Path[T]` as decoded parameters. A `Result` or `Option` return is judged by the
    /// responses it carries. The payload is searched through the types it is built from (`Json[list[Search]]` needs
    /// `Search` to have a JSON form); see [`Self::type_without_json_form`].
    fn route_payload_without_json_form(
        &self,
        ty: &ResolvedType,
        direction: JsonPayloadDirection,
    ) -> Option<(String, String)> {
        let ResolvedType::Generic(name, args) = ty else {
            return None;
        };
        if direction == JsonPayloadDirection::Encode
            && matches!(
                collection_type_id(name),
                Some(CollectionTypeId::Result | CollectionTypeId::Option)
            )
        {
            return args
                .iter()
                .find_map(|arg| self.route_payload_without_json_form(arg, direction));
        }
        let carries_json_payload = match surface_types::from_str(name) {
            Some(SurfaceTypeId::Json) => true,
            Some(SurfaceTypeId::Query | SurfaceTypeId::Path) => direction == JsonPayloadDirection::Decode,
            _ => false,
        };
        let is_web_surface_binding = matches!(
            self.lookup_type_info(name),
            Some(TypeInfo::Builtin | TypeInfo::TypeAlias) | None
        ) && self.lookup_symbol(name).is_some();
        let [payload] = args.as_slice() else {
            return None;
        };
        if !carries_json_payload || !is_web_surface_binding {
            return None;
        }
        self.type_without_json_form(payload, direction)
            .map(|offending| (ty.to_string(), offending))
    }

    /// Return the spelling of the first type inside `ty` that certainly cannot cross as JSON in `direction`, or `None`
    /// when every part of it has, or may have, a JSON form.
    ///
    /// Collections, `Option`, `Result`, tuples and the frozen collections cross as JSON exactly when their elements
    /// do, so the search descends into them; a generic model or class that has a JSON form still needs its type
    /// arguments to have one. The misfits are decided by [`Self::nominal_certainly_lacks_json_form`]; every other leaf
    /// -- a scalar, an enum, a newtype, a Rust-origin or unresolved type -- is left to the build.
    fn type_without_json_form(&self, ty: &ResolvedType, direction: JsonPayloadDirection) -> Option<String> {
        match ty {
            ResolvedType::Named(name) => self
                .nominal_certainly_lacks_json_form(name, direction)
                .then(|| name.clone()),
            ResolvedType::Generic(name, args) => {
                if collection_type_id(name).is_none() && self.nominal_certainly_lacks_json_form(name, direction) {
                    return Some(name.clone());
                }
                args.iter().find_map(|arg| self.type_without_json_form(arg, direction))
            }
            ResolvedType::Tuple(items) => items
                .iter()
                .find_map(|item| self.type_without_json_form(item, direction)),
            ResolvedType::FrozenList(inner) | ResolvedType::FrozenSet(inner) => {
                self.type_without_json_form(inner, direction)
            }
            ResolvedType::FrozenDict(key, value) => self
                .type_without_json_form(key, direction)
                .or_else(|| self.type_without_json_form(value, direction)),
            _ => None,
        }
    }

    /// Return whether a nominal type is a local model or class that certainly cannot cross as JSON in `direction`.
    ///
    /// A model or class has a JSON form through `@derive(json)` (which adopts both `std.serde.json` traits), through
    /// adopting the needed `Serialize` or `Deserialize` trait, or through a Rust derive. The type is refused only
    /// when every one of its derives is a builtin derive the compiler knows supplies no JSON, it has no Rust derive,
    /// and none of its adopted traits comes from `std.serde.json` or is spelled as the needed protocol. Every other
    /// nominal type -- a subclass, an enum, a newtype, a Rust-origin or unresolved type -- is left to the build.
    fn nominal_certainly_lacks_json_form(&self, name: &str, direction: JsonPayloadDirection) -> bool {
        let (declared_derives, trait_adoptions) = match self.lookup_type_info(name) {
            Some(TypeInfo::Model(model)) => (&model.derives, &model.trait_adoptions),
            Some(TypeInfo::Class(class)) if class.extends.is_none() => (&class.derives, &class.trait_adoptions),
            _ => return false,
        };
        if declared_derives
            .iter()
            .any(|derive| derives::from_str(derive).is_none())
            || self.local_rust_derive_paths.contains_key(name)
        {
            return false;
        }
        let needed = direction.protocol();
        !trait_adoptions.iter().any(|adoption| {
            adoption
                .module_path
                .as_deref()
                .is_some_and(stdlib::is_stdlib_json_trait_module_path)
                || std::iter::once(adoption.name.as_str())
                    .chain(adoption.source_name.as_deref())
                    .any(|spelling| stdlib::stdlib_json_trait_id(spelling) == Some(needed))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{RouteTypeShape, combine_part_shapes, route_capture_names};

    #[test]
    fn capture_names_follow_the_route_macro() {
        assert_eq!(
            route_capture_names("/posts/{year}/{month}"),
            vec!["year".to_string(), "month".to_string()]
        );
        assert_eq!(route_capture_names("/files/{*path}"), vec!["path".to_string()]);
        assert!(route_capture_names("/things").is_empty());
        assert!(route_capture_names("/").is_empty());
        assert!(
            route_capture_names("/broken/{id").is_empty(),
            "an unclosed brace is not a capture for the macro either"
        );
    }

    #[test]
    fn a_compound_type_is_refused_only_on_a_certain_misfit() {
        let fits = [RouteTypeShape::Fits, RouteTypeShape::Fits];
        assert_eq!(combine_part_shapes(fits.into_iter(), true), RouteTypeShape::Fits);
        assert_eq!(combine_part_shapes(fits.into_iter(), false), RouteTypeShape::Unknown);
        let unknown_part = [RouteTypeShape::Fits, RouteTypeShape::Unknown];
        assert_eq!(
            combine_part_shapes(unknown_part.into_iter(), true),
            RouteTypeShape::Unknown
        );
        let misfit = [RouteTypeShape::Unknown, RouteTypeShape::DoesNotFit];
        assert_eq!(
            combine_part_shapes(misfit.into_iter(), true),
            RouteTypeShape::DoesNotFit
        );
        assert_eq!(combine_part_shapes(std::iter::empty(), true), RouteTypeShape::Fits);
    }
}
